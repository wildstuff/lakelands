//! MCP over stdio: one JSON-RPC message per line in, one per line out.
//! The session serves one lake as one principal, both fixed by whoever
//! starts the server, never by the client. The CLI's `serve --mcp` serves
//! the root's lake with it, as the principal the command acts as.

use std::io;
use std::sync::Arc;

use lakelands_api::{LakeStore, Principal};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::session::{Session, code, error};

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
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let answers = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => session.handle(msg).await,
            Err(e) => vec![error(
                Value::Null,
                code::PARSE,
                format!("the message is not JSON: {e}"),
            )],
        };
        for a in answers {
            let mut bytes = serde_json::to_vec(&a).expect("JSON prints");
            bytes.push(b'\n');
            writer.write_all(&bytes).await?;
        }
        writer.flush().await?;
    }
    Ok(())
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
