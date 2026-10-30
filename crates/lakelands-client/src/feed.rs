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
use lakelands_api::error::{Case, Detail, Error, Exchange, Result};
use lakelands_api::event::Event;
use lakelands_api::id::Position;
use lakelands_api::seam::{ChangeFeed, FEED_EXAMINED, FeedBatch, FeedFilter};

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
    /// An event, whether its batch names it overruled, and the last
    /// position of its append where the stream says it.
    Event(Box<Event>, bool, Option<Position>),
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
/// counts only as a comment in a block that holds nothing else, the
/// markers `: overruled` and `: append <position>` mark the block's
/// event, and a block without `data` dispatches nothing.
fn block(lines: &[String]) -> std::result::Result<Said, NotSse> {
    let (mut kind, mut data, mut next, mut fields) = (None, None::<String>, None, false);
    let (mut overruled, mut append) = (false, None);
    for l in lines {
        if let Some(c) = l.strip_prefix(':') {
            let c = c.strip_prefix(' ').unwrap_or(c);
            if let Some(n) = c.strip_prefix("next ") {
                next = Some(n.parse::<u64>().map_err(|_| NotSse)?);
            }
            if let Some(n) = c.strip_prefix("append ") {
                append = Some(Position(n.parse::<u64>().map_err(|_| NotSse)?));
            }
            overruled |= c == "overruled";
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
            .map(|e| Said::Event(Box::new(e), overruled, append))
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
                overruled: Vec::new(),
                appends: Vec::new(),
            });
        }
        // An empty key would be an empty `key=`, which the door refuses;
        // it is refused here, before anything is sent.
        if filter.keys.iter().any(|k| k.as_str().is_empty()) {
            return Err(Error::Refused {
                field: Some("keys".into()),
                reason: lakelands_api::Refusal::Invalid(Box::new(Case::FilterKeyEmpty.into())),
            });
        }
        let key = wire.feed_key()?;
        // As the store's poll, at most `FEED_EXAMINED` events; the server
        // examines up to the head, one bounded poll at a time.
        let limit = limit.min(FEED_EXAMINED);
        let words = |w: &[String]| enc(&w.join(","));
        let mut query = format!("from={}", from.0);
        if !filter.kinds.is_empty() {
            query.push_str(&format!("&kinds={}", words(&filter.kinds)));
        }
        if !filter.types.is_empty() {
            let types: Vec<String> = filter.types.iter().map(|t| t.as_str().to_owned()).collect();
            query.push_str(&format!("&types={}", words(&types)));
        }
        // One `key` per key: a key may hold a comma.
        for k in &filter.keys {
            query.push_str(&format!("&key={}", enc(k.as_str())));
        }
        let mut stream = Stream::open(wire, key, query).await?;
        let (mut events, mut overruled): (Vec<Event>, Vec<Position>) = (Vec::new(), Vec::new());
        let mut appends: Vec<Position> = Vec::new();
        let next = loop {
            match stream.said().await? {
                Said::Event(e, o, end) => {
                    if o {
                        overruled.push(e.position);
                    }
                    if let Some(end) = end.filter(|end| appends.last() != Some(end)) {
                        appends.push(end);
                    }
                    events.push(*e);
                    if events.len() >= limit as usize {
                        break Position(events[events.len() - 1].position.0 + 1);
                    }
                }
                Said::Next(n) => break n,
                Said::Failed(e) => return Err(e),
                Said::Nothing => {}
            }
        };
        let head = stream.head;
        // The stream's head is the one it opened at; what it sent since
        // may lie past it.
        let head = Position(head.0.max(next.0.saturating_sub(1)));
        Ok(FeedBatch {
            events,
            next,
            head,
            overruled,
            appends,
        })
    }

    /// The server keeps no consumer state; a consumer keeps its own
    /// position.
    async fn ack(&self, _consumer: &str, _through: Position) -> Result<()> {
        Ok(())
    }

    /// The feed's stream from `after + 1`, held open until it sends an
    /// event or a `next` past that, or until `within` has passed: the
    /// server's tail waits on its lake for the next append
    /// (lakelands#1061).
    async fn wait(&self, after: Position, within: std::time::Duration) -> Result<Position> {
        let wire = &self.wire;
        let key = wire.feed_key()?;
        let query = format!("from={}", after.next().0);
        let mut stream = Stream::open(wire, key, query).await?;
        let past = async {
            loop {
                match stream.said().await? {
                    Said::Event(e, ..) => return Ok(e.position),
                    Said::Next(n) if n > after.next() => return Ok(Position(n.0 - 1)),
                    Said::Failed(e) => return Err(e),
                    Said::Next(_) | Said::Nothing => {}
                }
            }
        };
        let head = tokio::time::timeout(within, past).await;
        let head = head.unwrap_or(Ok(Position(0)))?;
        Ok(head.max(stream.head))
    }
}

/// The feed's stream, as the server sends it from the position asked.
struct Stream<'w> {
    wire: &'w Wire,
    body: hyper::body::Incoming,
    blocks: Blocks,
    /// The head the server read as the stream began.
    head: Position,
}

impl<'w> Stream<'w> {
    async fn open(wire: &'w Wire, key: &crate::ApiKey, query: String) -> Result<Stream<'w>> {
        let call = Call::new(Method::GET, "feed".to_owned())
            .query(query)
            .header("accept", "text/event-stream".into());
        let r = wire.send(key, call).await?;
        if !r.status().is_success() {
            return Err(wire.failure(r).await);
        }
        let head = position_of(&r).ok_or_else(|| wire.unavailable(Exchange::NoPosition))?;
        let (body, blocks) = (r.into_body(), Blocks::default());
        Ok(Stream {
            wire,
            body,
            blocks,
            head,
        })
    }

    /// What the next block says, as it arrives.
    async fn said(&mut self) -> Result<Said> {
        let wire = self.wire;
        let not_sse = |_| wire.unavailable(Exchange::LineNotTheContract);
        loop {
            if let Some(lines) = self.blocks.next().map_err(not_sse)? {
                return block(&lines).map_err(not_sse);
            }
            // A block that never ends must not grow without bound.
            if self.blocks.held > wire.cap {
                return Err(wire.too_long());
            }
            let frame = match tokio::time::timeout(wire.silence, self.body.frame()).await {
                Err(_) => {
                    let secs = wire.silence.as_secs();
                    return Err(wire.unavailable(Exchange::Silent { secs }));
                }
                Ok(None) => return Err(wire.unavailable(Exchange::Ended)),
                Ok(Some(Err(e))) => {
                    let detail = Detail::scrubbed(e);
                    return Err(wire.unavailable(Exchange::CutOff { detail }));
                }
                Ok(Some(Ok(frame))) => frame,
            };
            if let Ok(data) = frame.into_data() {
                self.blocks.push(&data);
            }
        }
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
        let rest = r#""situation":{"code":"moved"}}"#;
        assert_eq!(
            block(&lines(&[
                "event: error",
                &format!("data: {split}"),
                &format!("data: {rest}")
            ])),
            Ok(Said::Failed(lakelands_api::error::Error::unavailable(
                lakelands_api::error::Situation::Moved
            )))
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

    /// `: overruled` in an event's block marks the event (004); outside
    /// one it is a comment like any other.
    #[test]
    fn the_overruled_mark_marks_its_blocks_event() {
        let e = r#"{"body":{"key":"s1","kind":"trigger_fired","trigger":"wrote","type":"stop"},"context":{"depth":0,"origin":"direct"},"fence":1,"hash":"b24bae34b1cb98a888c26a50b5d6e80cd45ba528308b96822581335f7cdff7af","id":"5d9e8a7d2730bbd0e040b3cf60c48107","known_at":"2026-10-18T05:03:32.289866Z","lake":"l","origin":"system","position":12,"prev":"f66cb5de55179e25fc2dcb5a76d7cc9840bae8d475f55ca0dbdf871517324bef","v":17,"writer":"system:lakelands"}"#;
        let data = format!("data: {e}");
        for (mark, want) in [(": overruled", true), (": keep-alive", false)] {
            let said = block(&lines(&["id: 12", mark, "event: trigger_fired", &data]));
            assert!(
                matches!(said, Ok(Said::Event(_, o, _)) if o == want),
                "{mark}: {said:?}"
            );
        }
        assert_eq!(block(&lines(&[": overruled"])), Ok(Said::Nothing));
    }

    /// `: append <position>` in an event's block names where its append
    /// ends (083); a block without it names none.
    #[test]
    fn the_append_mark_names_its_blocks_append() {
        let e = r#"{"body":{"key":"s1","kind":"trigger_fired","trigger":"wrote","type":"stop"},"context":{"depth":0,"origin":"direct"},"fence":1,"hash":"b24bae34b1cb98a888c26a50b5d6e80cd45ba528308b96822581335f7cdff7af","id":"5d9e8a7d2730bbd0e040b3cf60c48107","known_at":"2026-10-18T05:03:32.289866Z","lake":"l","origin":"system","position":12,"prev":"f66cb5de55179e25fc2dcb5a76d7cc9840bae8d475f55ca0dbdf871517324bef","v":17,"writer":"system:lakelands"}"#;
        let data = format!("data: {e}");
        for (mark, want) in [(": append 13", Some(Position(13))), (": keep-alive", None)] {
            let said = block(&lines(&["id: 12", mark, "event: trigger_fired", &data]));
            assert!(
                matches!(said, Ok(Said::Event(_, _, end)) if end == want),
                "{mark}: {said:?}"
            );
        }
        assert_eq!(block(&lines(&["id: 12", ": append x", &data])), Err(NotSse));
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
        let Error::Unavailable { situation } = &e else {
            panic!("expected unavailable, got {e:?}");
        };
        let message = situation.to_string();
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(!message.contains(&addr.to_string()), "{message}");
        assert!(!message.contains("k-owner"), "{message}");
        assert!(
            waited >= silence && waited < Duration::from_secs(5),
            "answered after {waited:?}"
        );
    }
}
