//! MCP over stdio: one JSON-RPC message per line in, one per line out.
//! The session serves one lake as one principal, both fixed by whoever
//! starts the server, never by the client. The CLI's `serve --mcp` serves
//! the root's lake with it, as the principal the command acts as.
//!
//! One writer interleaves the answers with the session's notifications
//! ([`crate::watch`]), each message whole on its own line.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::{LakeStore, Principal};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use tokio::sync::mpsc;

use crate::session::{Session, code, error};
use crate::watch::QUEUE;

/// How long a caught-up stdio session waits for the lake's next append
/// before it reads the feed again.
pub const POLL: Duration = Duration::from_millis(250);

/// How often, at most, a stdio session reads the feed: one session a
/// process, so no herd of them slows the writer.
pub const FLOOR: Duration = Duration::from_millis(25);

/// Serve one session over a reader and a writer until the reader ends.
pub async fn serve_io<R, W>(
    store: Arc<dyn LakeStore>,
    who: Principal,
    reader: R,
    mut writer: W,
) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut session = Session::new(store, who, true);
    let (tx, mut notes) = mpsc::channel(QUEUE);
    let pump = tokio::spawn(session.pump(tx, POLL, FLOOR));
    let watch = session.watch();
    let mut lines = reader.lines();
    let served = async {
        loop {
            let answers = tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(msg) => session.handle(msg).await,
                        Err(e) => vec![error(
                            Value::Null,
                            code::PARSE,
                            format!("the message is not JSON: {e}"),
                        )],
                    }
                }
                Some(note) = notes.recv() => {
                    // A change made for a subscription since replaced or
                    // ended is not told.
                    if !watch.current(&note) {
                        continue;
                    }
                    vec![note.message]
                }
            };
            for a in answers {
                let mut bytes = serde_json::to_vec(&a).expect("JSON prints");
                bytes.push(b'\n');
                writer.write_all(&bytes).await?;
            }
            writer.flush().await?;
        }
        Ok::<(), io::Error>(())
    };
    let served = served.await;
    pump.abort();
    served
}

/// Serve one session on the process's standard input and output.
pub async fn serve_stdio(store: Arc<dyn LakeStore>, who: Principal) -> io::Result<()> {
    serve_io(
        store,
        who,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}
