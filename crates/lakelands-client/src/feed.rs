//! The change feed over REST: one `poll` opens `GET …/feed` and reads its
//! server-sent events until it has `limit` of them, or until the stream
//! says it caught up (`: next <position>`), then closes it.
//!
//! The exchange's timeout covers connecting and the answer's head; after
//! that, a stream silent for longer than two of the server's keep-alives
//! is taken for dead and answered `Unavailable`, so a poll never hangs on
//! a connection that went away.

use std::sync::Arc;

use async_trait::async_trait;
use http_body_util::BodyExt;
use hyper::Method;
use lakelands_api::error::{Error, Result};
use lakelands_api::event::Event;
use lakelands_api::id::Position;
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter};

use crate::wire::{Call, Wire, enc, position_of};

/// The change feed of a lake another process serves.
pub(crate) struct RemoteFeed {
    wire: Arc<Wire>,
}

impl RemoteFeed {
    pub(crate) fn new(wire: Arc<Wire>) -> RemoteFeed {
        RemoteFeed { wire }
    }
}

/// What one block of the stream said.
#[derive(Debug, PartialEq)]
enum Said {
    Event(Box<Event>),
    Failed(Error),
    Next(Position),
    Nothing,
}

/// A block the contract does not send.
#[derive(Debug, PartialEq)]
struct NotSse;

/// What a block of lines says, by the rules of server-sent events: a
/// field's value loses one leading space, `data` lines join with `\n`,
/// a line starting `:` is a comment. The marker `: next <position>`
/// counts only as a comment in a block that holds nothing else, and a
/// block without `data` dispatches nothing.
fn block(lines: &[String]) -> std::result::Result<Said, NotSse> {
    let (mut kind, mut data, mut next, mut fields) = (None, None::<String>, None, false);
    for l in lines {
        if let Some(c) = l.strip_prefix(':') {
            let c = c.strip_prefix(' ').unwrap_or(c);
            if let Some(n) = c.strip_prefix("next ") {
                next = Some(n.parse::<u64>().map_err(|_| NotSse)?);
            }
            continue;
        }
        fields = true;
        let (field, value) = match l.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (l.as_str(), ""),
        };
        match field {
            "event" => kind = Some(value.to_owned()),
            "data" => match &mut data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => data = Some(value.to_owned()),
            },
            _ => {}
        }
    }
    if let (Some(n), false) = (next, fields) {
        return Ok(Said::Next(Position(n)));
    }
    match (kind.as_deref(), data) {
        (Some("error"), Some(d)) => serde_json::from_str(&d)
            .map(Said::Failed)
            .map_err(|_| NotSse),
        (_, Some(d)) => serde_json::from_str(&d)
            .map(|e| Said::Event(Box::new(e)))
            .map_err(|_| NotSse),
        (_, None) => Ok(Said::Nothing),
    }
}

/// The stream's bytes, cut into lines and blocks as they arrive: a line
/// ends at `\r\n`, `\n` or `\r`, and an empty line ends a block.
#[derive(Default)]
struct Blocks {
    buf: Vec<u8>,
    lines: Vec<String>,
    /// The bytes of the block so far, lines taken included.
    held: usize,
    /// The last line ended at `\r`, so a `\n` that comes next is part of
    /// that end.
    after_cr: bool,
}

impl Blocks {
    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        self.held += data.len();
    }

    /// The next whole block, if one has arrived.
    fn next(&mut self) -> std::result::Result<Option<Vec<String>>, NotSse> {
        loop {
            if self.after_cr && !self.buf.is_empty() {
                self.after_cr = false;
                if self.buf[0] == b'\n' {
                    self.buf.remove(0);
                }
            }
            let Some(i) = self.buf.iter().position(|b| *b == b'\r' || *b == b'\n') else {
                return Ok(None);
            };
            self.after_cr = self.buf[i] == b'\r';
            let raw: Vec<u8> = self.buf.drain(..=i).collect();
            let line = String::from_utf8(raw[..i].to_vec()).map_err(|_| NotSse)?;
            if line.is_empty() {
                self.held = self.buf.len();
                if !self.lines.is_empty() {
                    return Ok(Some(std::mem::take(&mut self.lines)));
                }
            } else {
                self.lines.push(line);
            }
        }
    }
}

#[async_trait]
impl ChangeFeed for RemoteFeed {
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch> {
        let wire = &self.wire;
        if limit == 0 {
            let head = wire.head().await?;
            return Ok(FeedBatch {
                events: Vec::new(),
                next: from,
                head,
            });
        }
        let key = wire.feed_key()?;
        let words = |w: &[String]| enc(&w.join(","));
        let mut query = format!("from={}", from.0);
        if !filter.kinds.is_empty() {
            query.push_str(&format!("&kinds={}", words(&filter.kinds)));
        }
        if !filter.types.is_empty() {
            let types: Vec<String> = filter.types.iter().map(|t| t.as_str().to_owned()).collect();
            query.push_str(&format!("&types={}", words(&types)));
        }
        let path = "feed".to_owned();
        let call = Call::new(Method::GET, path.clone())
            .query(query)
            .header("accept", "text/event-stream".into());
        let r = wire.send(key, call).await?;
        if !r.status().is_success() {
            return Err(wire.failure(&path, r).await);
        }
        let head = position_of(&r);
        let mut body = r.into_body();
        let head =
            head.ok_or_else(|| wire.unavailable(&path, "answered the feed without its position"))?;
        let mut events: Vec<Event> = Vec::new();
        let mut blocks = Blocks::default();
        let not_sse = |_| wire.unavailable(&path, "sent a block that is not the contract's");
        let next = 'read: loop {
            while let Some(lines) = blocks.next().map_err(not_sse)? {
                match block(&lines).map_err(not_sse)? {
                    Said::Event(e) => {
                        events.push(*e);
                        if events.len() >= limit as usize {
                            break 'read Position(events[events.len() - 1].position.0 + 1);
                        }
                    }
                    Said::Next(n) => break 'read n,
                    Said::Failed(e) => return Err(e),
                    Said::Nothing => {}
                }
            }
            // A block that never ends must not grow without bound.
            if blocks.held > wire.cap {
                return Err(wire.too_long(&path));
            }
            let frame = match tokio::time::timeout(wire.silence, body.frame()).await {
                Err(_) => {
                    return Err(wire.unavailable(
                        &path,
                        format_args!(
                            "sent nothing for {} seconds, not even a keep-alive",
                            wire.silence.as_secs()
                        ),
                    ));
                }
                Ok(None) => return Err(wire.unavailable(&path, "ended the feed's stream")),
                Ok(Some(Err(e))) => {
                    return Err(wire.unavailable(&path, format_args!("cut the feed off: {e}")));
                }
                Ok(Some(Ok(frame))) => frame,
            };
            if let Ok(data) = frame.into_data() {
                blocks.push(&data);
            }
        };
        // The stream's head is the one it opened at; what it sent since
        // may lie past it.
        let head = Position(head.0.max(next.0.saturating_sub(1)));
        Ok(FeedBatch { events, next, head })
    }

    /// The server keeps no consumer state; a consumer keeps its own
    /// position.
    async fn ack(&self, _consumer: &str, _through: Position) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use lakelands_api::LakeStore;
    use lakelands_api::error::Error;
    use lakelands_api::id::Position;
    use lakelands_api::location::Limits;
    use lakelands_api::seam::FeedFilter;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::{RemoteConfig, RemoteLake};

    use super::{Blocks, NotSse, Said, block};

    /// Every whole block, as the bytes arrive in these pieces.
    fn blocks(pieces: &[&[u8]]) -> Vec<Vec<String>> {
        let mut b = Blocks::default();
        let mut out = Vec::new();
        for p in pieces {
            b.push(p);
            while let Some(lines) = b.next().unwrap() {
                out.push(lines);
            }
        }
        out
    }

    fn lines(ls: &[&str]) -> Vec<String> {
        ls.iter().map(|l| l.to_string()).collect()
    }

    /// A block ends at an empty line, whether lines end with `\n`,
    /// `\r\n` or `\r`, and a `\r\n` split across two pieces is one end.
    #[test]
    fn a_block_ends_at_an_empty_line_whatever_ends_its_lines() {
        let want = vec![lines(&["event: a", "data: 1"]), lines(&[": next 4"])];
        for pieces in [
            &[&b"event: a\ndata: 1\n\n: next 4\n\n"[..]][..],
            &[b"event: a\r\ndata: 1\r\n\r\n: next 4\r\n\r\n"],
            &[b"event: a\rdata: 1\r\r: next 4\r\r"],
            &[
                b"event: a\r",
                b"\ndata: 1\r",
                b"\n\r",
                b"\n: next 4\r\n\r\n",
            ],
        ] {
            assert_eq!(blocks(pieces), want, "{pieces:?}");
        }
        // Nothing is dispatched before its block ends.
        assert!(blocks(&[b"data: 1\n"]).is_empty());
    }

    /// `data` lines join with a line break; a field loses one leading
    /// space; a block without `data` says nothing.
    #[test]
    fn data_lines_join_and_a_block_without_data_says_nothing() {
        let e = r#"{"error":"unavailable","message":"a\nb"}"#;
        let (first, second) = e.split_at(20);
        let joined = block(&lines(&[
            "event: error",
            &format!("data:{first}"),
            &format!("data: {second}"),
        ]));
        // The two halves join with `\n`, which falls inside a JSON string
        // here, so the text is not the error's any more.
        assert_eq!(joined, Err(NotSse));
        let split = r#"{"error":"unavailable","#;
        let rest = r#""message":"m"}"#;
        assert_eq!(
            block(&lines(&[
                "event: error",
                &format!("data: {split}"),
                &format!("data: {rest}")
            ])),
            Ok(Said::Failed(lakelands_api::error::Error::Unavailable {
                message: "m".into()
            }))
        );
        assert_eq!(block(&lines(&["event: x", "id: 3"])), Ok(Said::Nothing));
        assert_eq!(block(&lines(&[": keep-alive"])), Ok(Said::Nothing));
    }

    /// The marker is `: next <position>` in a block of comments only; in
    /// a block that carries fields it is not the marker.
    #[test]
    fn the_marker_is_a_comment_of_its_own() {
        assert_eq!(block(&lines(&[": next 30"])), Ok(Said::Next(Position(30))));
        assert_eq!(block(&lines(&[":next 30"])), Ok(Said::Next(Position(30))));
        assert_eq!(block(&lines(&[": next thirty"])), Err(NotSse));
        assert_eq!(block(&lines(&["event: x", ": next 30"])), Ok(Said::Nothing));
        assert_eq!(block(&lines(&[": nextish"])), Ok(Said::Nothing));
    }

    /// A stream that sends its head and one keep-alive, then nothing, is
    /// taken for dead once it has been silent for the bound: the poll
    /// answers `Unavailable` naming where, and does not hang.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_feed_silent_past_its_bound_is_unavailable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                head.push(b[0]);
            }
            let keep_alive = ": keep-alive\n\n";
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nlakelands-position: 3\r\n\
                 transfer-encoding: chunked\r\n\r\n{:x}\r\n{keep_alive}\r\n",
                keep_alive.len()
            );
            s.write_all(reply.as_bytes()).await.unwrap();
            // Hold the connection open, and say nothing more.
            tokio::time::sleep(Duration::from_secs(60)).await;
            drop(s);
        });
        let config = RemoteConfig {
            url: format!("http://{addr}"),
            lake: "finance".into(),
            keys: BTreeMap::from([("owner".to_owned(), "k-owner".into())]),
            feed_key: Some("owner".into()),
            ca_file: None,
            timeout_secs: 30,
            max_answer_bytes: crate::DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        };
        let silence = Duration::from_millis(300);
        let remote = RemoteLake::connect_with_silence(config, silence).unwrap();
        let asked = Instant::now();
        let e = remote
            .feed()
            .poll(Position(0), &FeedFilter::default(), 10)
            .await
            .unwrap_err();
        let waited = asked.elapsed();
        let Error::Unavailable { message } = &e else {
            panic!("expected unavailable, got {e:?}");
        };
        assert!(
            message.contains(&format!("{addr}/lakes/finance/feed")),
            "{message}"
        );
        assert!(!message.contains("k-owner"), "{message}");
        assert!(
            waited >= silence && waited < Duration::from_secs(5),
            "answered after {waited:?}"
        );
    }
}
