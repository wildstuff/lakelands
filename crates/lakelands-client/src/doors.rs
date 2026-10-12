//! The catalog over REST: each door is its route (`docs/rest.md`), its
//! arguments in the path, the query, the headers and the body, and its
//! answer the contract's own type.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use http_body_util::BodyExt;
use hyper::Method;
use hyper::body::{Body, Frame, Incoming};
use hyper::header::HeaderValue;
use lakelands_api::Catalog;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::error::{Case, Detail, Error, Exchange, Result, Segment};
use lakelands_api::expr::Condition;
use lakelands_api::id::{
    LakeId, ProposalId, RecordKey, RelationName, SourceId, TypeName, VerbName,
};
use lakelands_api::ontology::FieldDecl;
use lakelands_api::principal::Principal;
use lakelands_api::query::{Hits, Pattern, QueryResult, Search};
use lakelands_api::read::{Find, Found, Get, ReadAt, Reply};
use lakelands_api::record::{Patch, Record};
use lakelands_api::write::{
    Committed, Declaration, Migration, MigrationReport, Outcome, Proposal, ProposalFilter,
    ProposedWrite, Recompute, ValueTypeMigration, ValueTypeReport, WriteOpts,
};
use serde_json::{Value, json};

use crate::wire::{Call, Wire, query_of, seg};

/// The doors of a lake another process serves.
pub(crate) struct RemoteCatalog {
    wire: Arc<Wire>,
}

impl RemoteCatalog {
    pub(crate) fn new(wire: Arc<Wire>) -> RemoteCatalog {
        RemoteCatalog { wire }
    }

    async fn go<T: serde::de::DeserializeOwned>(&self, who: &Principal, call: Call) -> Result<T> {
        let key = self.wire.key_of(who)?;
        self.wire.exchange(key, call).await
    }
}

fn record(ty: &TypeName, key: &RecordKey) -> Result<String> {
    Ok(format!(
        "types/{}/records/{}",
        seg(ty.as_str(), Segment::Type)?,
        seg(key.as_str(), Segment::Key)?
    ))
}

fn link(ty: &TypeName, key: &RecordKey, relation: &RelationName, to: &RecordKey) -> Result<String> {
    Ok(format!(
        "{}/links/{}/{}",
        record(ty, key)?,
        seg(relation.as_str(), Segment::Relation)?,
        seg(to.as_str(), Segment::To)?
    ))
}

/// The members of a value, as JSON, less those the path carries.
fn members<T: serde::Serialize>(v: &T, less: &[&str]) -> Value {
    let mut v = serde_json::to_value(v).expect("contract types serialise");
    if let Some(o) = v.as_object_mut() {
        for k in less {
            o.remove(*k);
        }
    }
    v
}

/// The compare-and-set and idempotency headers a write's options stand
/// for. `PUT` says a record that must not exist yet as
/// `If-None-Match: *`; every other write names the version it expects.
/// An idempotency key a header cannot carry (a line break, say) is
/// refused before anything is sent.
fn write_headers(mut call: Call, opts: &WriteOpts, put: bool) -> Result<Call> {
    match opts.expected_version {
        Some(v) if v.0 == 0 && put => call = call.header("if-none-match", "*".into()),
        Some(v) => call = call.header("if-match", format!("\"{}\"", v.0)),
        None => {}
    }
    if let Some(k) = &opts.idempotency_key {
        if HeaderValue::from_str(k.as_str()).is_err() {
            return Err(Error::invalid(Case::IdempotencyKeyHeader));
        }
        call = call.header("idempotency-key", k.as_str().to_owned());
    }
    Ok(call)
}

fn at_query(at: &ReadAt) -> String {
    query_of(&members(at, &[]))
}

#[async_trait]
impl Catalog for RemoteCatalog {
    fn lake(&self) -> &LakeId {
        &self.wire.lake
    }

    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        let call = Call::new(Method::GET, "describe".into()).query(at_query(&at));
        self.go(who, call).await
    }

    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        let call = Call::new(
            Method::GET,
            format!("describe/{}", seg(ty.as_str(), Segment::Type)?),
        )
        .query(at_query(&at));
        self.go(who, call).await
    }

    /// `where` goes in the query in its text form; the typed form has no
    /// query form and is refused before anything is sent. `after` and
    /// `aggregate` go as JSON, and whatever they may be refused for the
    /// lake refuses.
    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        if let Some(Condition::Expr(_)) = find.filter {
            return Err(Error::invalid(Case::FindWhereText));
        }
        let call = Call::new(
            Method::GET,
            format!("types/{}/records", seg(ty.as_str(), Segment::Type)?),
        )
        .query(query_of(&members(&find, &[])));
        self.go(who, call).await
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        let call =
            Call::new(Method::GET, record(ty, &get.key)?).query(query_of(&members(&get, &["key"])));
        self.go(who, call).await
    }

    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let call = Call::new(Method::PUT, record(ty, key)?).json(&members(&patch, &[]));
        self.go(who, write_headers(call, &opts, true)?).await
    }

    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let call = Call::new(Method::DELETE, record(ty, key)?);
        self.go(who, write_headers(call, &opts, false)?).await
    }

    async fn link(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let call =
            Call::new(Method::PUT, link(ty, key, relation, to)?).json(&members(&fields, &[]));
        self.go(who, write_headers(call, &opts, true)?).await
    }

    async fn unlink(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let call = Call::new(Method::DELETE, link(ty, key, relation, to)?);
        self.go(who, write_headers(call, &opts, false)?).await
    }

    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        // A verb on its type takes the keyless route. Whether the verb
        // takes a key is the server's to say, not the client's.
        let verb = seg(verb.as_str(), Segment::Verb)?;
        let path = match key {
            Some(key) => format!("verbs/{verb}/{}", seg(key.as_str(), Segment::Key)?),
            None => format!("verbs/{verb}"),
        };
        let call = Call::new(Method::POST, path).json(&members(&payload, &[]));
        self.go(who, write_headers(call, &opts, false)?).await
    }

    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        let call = Call::new(Method::POST, "query".into()).json(&members(&pattern, &[]));
        self.go(who, call).await
    }

    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>> {
        let call = Call::new(Method::POST, "search".into()).json(&members(&search, &[]));
        self.go(who, call).await
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        let call =
            Call::new(Method::POST, "proposals".into()).json(&json!({"write": write, "why": why}));
        self.go(who, call).await
    }

    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        let path = format!(
            "proposals/{}/accept",
            seg(proposal.as_str(), Segment::Proposal)?
        );
        self.go(who, Call::new(Method::POST, path)).await
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        let path = format!(
            "proposals/{}/reject",
            seg(proposal.as_str(), Segment::Proposal)?
        );
        let call = Call::new(Method::POST, path).json(&json!({ "reason": reason }));
        self.go(who, call).await
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        let call =
            Call::new(Method::GET, "proposals".into()).query(query_of(&members(&filter, &[])));
        self.go(who, call).await
    }

    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        let call = Call::new(Method::POST, "schema/declare".into())
            .json(&json!({ "declaration": declaration }));
        self.go(who, call).await
    }

    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        let call = Call::new(Method::POST, "schema/apply".into())
            .json(&json!({ "declarations": declarations }));
        self.go(who, call).await
    }

    async fn recompute(&self, who: &Principal, what: Recompute) -> Result<Outcome> {
        let call = Call::new(Method::POST, "schema/recompute".into()).json(&json!(what));
        self.go(who, call).await
    }

    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        let call = Call::new(Method::POST, "schema/add_field".into())
            .json(&json!({ "type": ty, "field": field }));
        self.go(who, call).await
    }

    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        let call = Call::new(Method::POST, "schema/migrate".into()).json(&json!(migration));
        self.go(who, call).await
    }

    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        let mut body = json!(migration);
        body["report_id"] = json!(report_id);
        let call = Call::new(Method::POST, "schema/migrate/apply".into()).json(&body);
        self.go(who, call).await
    }

    async fn migrate_value_type(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
    ) -> Result<Reply<ValueTypeReport>> {
        let body = json!({ "value_type": m.to, "renames": m.renames });
        let call = Call::new(Method::POST, "schema/migrate".into()).json(&body);
        self.go(who, call).await
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        let body = json!({ "value_type": m.to, "renames": m.renames, "report_id": report_id });
        let call = Call::new(Method::POST, "schema/migrate/apply".into()).json(&body);
        self.go(who, call).await
    }

    /// The records go as an NDJSON body as the source yields them; the
    /// answer's lines come back as they are written, one per window. A
    /// line that carries an error ends the stream with it, and what came
    /// before it is committed.
    ///
    /// The answer is held to the feed's silence bound, in two phases.
    /// While records are sent, the bound counts only while the
    /// connection holds a record it has not taken and the answer says
    /// nothing — the server stopped reading, and does not say it works:
    /// any line of the answer, the empty line it sends while a window
    /// commits included, starts the bound again. Time spent waiting on
    /// the source is the caller's pace and never counts, however slow. Once the source has ended, an answer
    /// silent for the bound is taken for dead. Either way the stream ends
    /// with `Unavailable`. A line longer than `max_answer_bytes` ends it
    /// the same way.
    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        let key = self.wire.key_of(who)?;
        let path = format!("ingest/{}", seg(source.as_str(), Segment::Source)?);
        let upload = Arc::new(Upload::default());
        let body = Ndjson {
            records,
            upload: upload.clone(),
        }
        .boxed_unsync();
        let call = Call::new(Method::POST, path.clone()).stream("application/x-ndjson", body);
        let r = self.wire.send(key, call).await?;
        if !r.status().is_success() {
            return Err(self.wire.failure(r).await);
        }
        Ok(Box::pin(Commits {
            silent: Box::pin(tokio::time::sleep(self.wire.silence)),
            armed: false,
            spoke: Instant::now(),
            wire: self.wire.clone(),
            body: r.into_body(),
            upload,
            buf: Vec::new(),
            ended: false,
            done: false,
        }))
    }
}

/// How the upload goes: whether the source has ended, when the
/// connection last took a record, whether the source is what it waits
/// on, and who waits to hear of a change.
struct Upload {
    ended: AtomicBool,
    taken: Mutex<Taken>,
    waiting: Mutex<Option<Waker>>,
}

/// When the connection last took a record from the body, and whether it
/// is now waiting on the source rather than the source on it.
#[derive(Clone, Copy)]
struct Taken {
    at: Instant,
    on_source: bool,
}

impl Default for Upload {
    fn default() -> Upload {
        Upload {
            ended: AtomicBool::new(false),
            taken: Mutex::new(Taken {
                at: Instant::now(),
                on_source: false,
            }),
            waiting: Mutex::new(None),
        }
    }
}

impl Upload {
    fn taken(&self) -> Taken {
        *self.taken.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The connection asked for the next record: it took one, or waits
    /// on the source for it.
    fn asked(&self, on_source: bool) {
        *self.taken.lock().unwrap_or_else(|e| e.into_inner()) = Taken {
            at: Instant::now(),
            on_source,
        };
        if !on_source {
            self.wake();
        }
    }

    fn end(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.wake();
    }

    fn wake(&self) {
        let waiting = self
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(w) = waiting {
            w.wake();
        }
    }
}

/// A source stream as an NDJSON body, one record per line.
struct Ndjson {
    records: SourceStream,
    upload: Arc<Upload>,
}

impl Body for Ndjson {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, std::io::Error>>> {
        let this = self.as_mut().get_mut();
        match this.records.as_mut().poll_next(cx) {
            Poll::Pending => {
                this.upload.asked(true);
                Poll::Pending
            }
            Poll::Ready(None) => {
                this.upload.end();
                Poll::Ready(None)
            }
            Poll::Ready(Some(r)) => {
                this.upload.asked(false);
                let mut line = serde_json::to_vec(&r).expect("a record prints");
                line.push(b'\n');
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(line)))))
            }
        }
    }
}

/// The lines of an ingest's answer, as they arrive.
struct Commits {
    /// Fires when the answer has been silent too long after the upload.
    silent: Pin<Box<tokio::time::Sleep>>,
    /// Whether `silent` counts from the last frame after the upload.
    armed: bool,
    /// When the answer last brought a frame.
    spoke: Instant,
    wire: Arc<Wire>,
    body: Incoming,
    upload: Arc<Upload>,
    buf: Vec<u8>,
    ended: bool,
    done: bool,
}

impl Commits {
    fn line(&mut self, text: &[u8]) -> Result<Committed> {
        if let Ok(c) = serde_json::from_slice::<Committed>(text) {
            return Ok(c);
        }
        self.done = true;
        Err(serde_json::from_slice::<Error>(text)
            .unwrap_or_else(|_| self.wire.unavailable(Exchange::LineNotTheContract)))
    }
}

impl Stream for Commits {
    type Item = Result<Committed>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Committed>>> {
        let this = self.get_mut();
        loop {
            if this.done {
                return Poll::Ready(None);
            }
            if let Some(i) = this.buf.iter().position(|b| *b == b'\n') {
                let text: Vec<u8> = this.buf.drain(..=i).collect();
                if text.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return Poll::Ready(Some(this.line(&text)));
            }
            if this.buf.len() > this.wire.cap {
                this.done = true;
                return Poll::Ready(Some(Err(this.wire.too_long())));
            }
            if this.ended {
                this.done = true;
                if this.buf.iter().all(u8::is_ascii_whitespace) {
                    return Poll::Ready(None);
                }
                let text = std::mem::take(&mut this.buf);
                return Poll::Ready(Some(this.line(&text)));
            }
            match Pin::new(&mut this.body).poll_frame(cx) {
                Poll::Pending => {
                    if !this.upload.ended.load(Ordering::SeqCst) {
                        // Hear when the upload moves or ends.
                        *this
                            .upload
                            .waiting
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
                        if !this.upload.ended.load(Ordering::SeqCst) {
                            this.armed = false;
                            let taken = this.upload.taken();
                            // Waiting on the source is the caller's pace.
                            if taken.on_source {
                                return Poll::Pending;
                            }
                            // A record is ready, the connection has not
                            // taken it and the answer is quiet: the server
                            // stopped reading.
                            let due = taken.at.max(this.spoke) + this.wire.silence;
                            if Instant::now() < due {
                                this.silent
                                    .as_mut()
                                    .reset(tokio::time::Instant::from_std(due));
                                if this.silent.as_mut().poll(cx).is_pending() {
                                    return Poll::Pending;
                                }
                                continue;
                            }
                            this.done = true;
                            let secs = this.wire.silence.as_secs();
                            let e = this.wire.unavailable(Exchange::Stalled { secs });
                            return Poll::Ready(Some(Err(e)));
                        }
                    }
                    if !this.armed {
                        let at = tokio::time::Instant::now() + this.wire.silence;
                        this.silent.as_mut().reset(at);
                        this.armed = true;
                    }
                    if this.silent.as_mut().poll(cx).is_pending() {
                        return Poll::Pending;
                    }
                    this.done = true;
                    let secs = this.wire.silence.as_secs();
                    let e = this.wire.unavailable(Exchange::SilentAfter { secs });
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => this.ended = true,
                Poll::Ready(Some(Ok(frame))) => {
                    this.armed = false;
                    this.spoke = Instant::now();
                    if let Ok(data) = frame.into_data() {
                        this.buf.extend_from_slice(&data);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    this.done = true;
                    let e = this.wire.unavailable(Exchange::CutOff {
                        detail: Detail::scrubbed(e),
                    });
                    return Poll::Ready(Some(Err(e)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::{Duration, Instant};

    use futures_core::Stream;
    use lakelands_api::error::Error;
    use lakelands_api::location::Limits;
    use lakelands_api::record::SourceRecord;
    use lakelands_api::{LakeStore, Principal, WriteContext};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::{DEFAULT_MAX_ANSWER_BYTES, RemoteConfig, RemoteLake};

    const SILENCE: Duration = Duration::from_millis(300);

    /// A server that answers an ingest's head at once. With `answer`, it
    /// reads the whole upload and then sends that line and ends; without,
    /// it says nothing more and holds the connection.
    async fn fake(answer: Option<&'static str>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut b = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                seen.push(b[0]);
            }
            let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                        lakelands-position: 3\r\ntransfer-encoding: chunked\r\n\r\n";
            s.write_all(head.as_bytes()).await.unwrap();
            let Some(line) = answer else {
                tokio::time::sleep(Duration::from_secs(60)).await;
                return;
            };
            // The upload's last chunk.
            seen.clear();
            while !seen.ends_with(b"0\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                seen.push(b[0]);
            }
            let reply = format!("{:x}\r\n{line}\r\n0\r\n\r\n", line.len());
            s.write_all(reply.as_bytes()).await.unwrap();
        });
        addr
    }

    fn remote(addr: std::net::SocketAddr) -> RemoteLake {
        let config = RemoteConfig {
            url: format!("http://{addr}"),
            lake: "finance".into(),
            keys: BTreeMap::from([("importer".to_owned(), "k-importer".into())]),
            feed_key: None,
            ca_file: None,
            timeout_secs: 30,
            max_answer_bytes: DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        };
        RemoteLake::connect_with_silence(config, SILENCE).unwrap()
    }

    fn importer() -> Principal {
        lakelands_mock::checks::Cast::of(&"finance".into()).importer
    }

    fn record() -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "type": "invoice", "key": "INV-1", "fields": {"amount": "10"}
        }))
        .unwrap()
    }

    /// One record, after a wait.
    struct Slow {
        wait: Pin<Box<tokio::time::Sleep>>,
        record: Option<SourceRecord>,
    }

    impl Stream for Slow {
        type Item = SourceRecord;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
            if self.wait.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            Poll::Ready(self.record.take())
        }
    }

    /// The upload has ended and the answer's head came, then nothing: the
    /// stream ends `Unavailable` once it has been silent for the bound,
    /// naming where, and does not hang.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_ingest_answer_silent_past_its_bound_after_the_upload_is_unavailable() {
        let addr = fake(None).await;
        let lake = remote(addr);
        let asked = Instant::now();
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(
                &importer(),
                &"bank".into(),
                lakelands_mock::stream::of(vec![record()]),
            )
            .await
            .expect("the ingest starts");
        let out = lakelands_mock::stream::collect(&mut commits).await;
        let waited = asked.elapsed();
        let [Err(Error::Unavailable { situation })] = out.as_slice() else {
            panic!("expected one unavailable, got {out:?}");
        };
        let message = situation.to_string();
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(!message.contains(&addr.to_string()), "{message}");
        assert!(message.contains("after the records ended"), "{message}");
        assert!(
            waited >= SILENCE && waited < Duration::from_secs(5),
            "answered after {waited:?}"
        );
    }

    /// Records without end, each large, so the connection's buffers fill.
    struct Endless;

    impl Stream for Endless {
        type Item = SourceRecord;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
            Poll::Ready(Some(
                serde_json::from_value(serde_json::json!({
                    "type": "invoice", "key": "INV-1",
                    "fields": {"note": "n".repeat(64 << 10)}
                }))
                .unwrap(),
            ))
        }
    }

    /// The server answered the head and stopped reading while records
    /// were still being sent: the stream ends `Unavailable` once the
    /// connection has taken nothing for the bound, and does not hang.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_upload_the_server_stops_reading_is_unavailable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut b = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                seen.push(b[0]);
            }
            let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                        lakelands-position: 3\r\ntransfer-encoding: chunked\r\n\r\n";
            s.write_all(head.as_bytes()).await.unwrap();
            // Read nothing more, and hold the connection.
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let lake = remote(addr);
        let asked = Instant::now();
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(&importer(), &"bank".into(), Box::pin(Endless))
            .await
            .expect("the ingest starts");
        let out = lakelands_mock::stream::collect(&mut commits).await;
        let waited = asked.elapsed();
        let [Err(Error::Unavailable { situation })] = out.as_slice() else {
            panic!("expected one unavailable, got {out:?}");
        };
        let message = situation.to_string();
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(!message.contains(&addr.to_string()), "{message}");
        assert!(
            message.contains("while records were being sent"),
            "{message}"
        );
        assert!(
            waited < Duration::from_secs(10),
            "answered after {waited:?}"
        );
    }

    /// Large records, `n` of them, so the connection's buffers fill.
    struct Many(usize);

    impl Stream for Many {
        type Item = SourceRecord;

        fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
            if self.0 == 0 {
                return Poll::Ready(None);
            }
            self.0 -= 1;
            Poll::Ready(Some(
                serde_json::from_value(serde_json::json!({
                    "type": "invoice", "key": format!("INV-{}", self.0),
                    "fields": {"note": "n".repeat(64 << 10)}
                }))
                .unwrap(),
            ))
        }
    }

    /// A server that stops reading while a window commits, for several
    /// times the bound, and sends an empty line meanwhile, is working: the
    /// upload waits for it and the answer comes through (lakelands#166).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_upload_the_server_holds_while_it_answers_empty_lines_is_waited_for() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut b = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                seen.push(b[0]);
            }
            let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                        lakelands-position: 3\r\ntransfer-encoding: chunked\r\n\r\n";
            s.write_all(head.as_bytes()).await.unwrap();
            // A window commits: nothing is read, an empty line is sent.
            for _ in 0..12 {
                tokio::time::sleep(SILENCE / 3).await;
                s.write_all(b"1\r\n\n\r\n").await.unwrap();
            }
            // Then the rest of the upload is read, and answered. The
            // server goes on saying it works while it reads, as the real
            // one does after every quiet keep-alive: the buffers that
            // filled drain before the client takes its next record, and on
            // a slow machine that takes longer than the bound.
            let mut buf = vec![0u8; 64 << 10];
            let mut tail = Vec::new();
            let mut alive = tokio::time::interval(SILENCE / 3);
            loop {
                tokio::select! {
                    n = s.read(&mut buf) => {
                        let n = n.unwrap();
                        tail.extend_from_slice(&buf[..n]);
                        if n == 0 || tail.ends_with(b"0\r\n\r\n") {
                            break;
                        }
                        let keep = tail.len().saturating_sub(8);
                        tail.drain(..keep);
                    }
                    _ = alive.tick() => s.write_all(b"1\r\n\n\r\n").await.unwrap(),
                }
            }
            let line = r#"{"position":5,"records":512,"duplicates":0}"#;
            let reply = format!("{:x}\r\n{line}\r\n0\r\n\r\n", line.len());
            s.write_all(reply.as_bytes()).await.unwrap();
        });
        let lake = remote(addr);
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(&importer(), &"bank".into(), Box::pin(Many(512)))
            .await
            .expect("the ingest starts");
        let out = lakelands_mock::stream::collect(&mut commits).await;
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].as_ref().map(|c| c.records), Ok(512));
    }

    /// While the source still has records to give, its pace is the
    /// caller's: a source slower than the bound is waited for, and the
    /// answer that follows the upload comes through.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_source_slower_than_the_bound_is_waited_for() {
        let addr = fake(Some(r#"{"position":5,"records":1,"duplicates":0}"#)).await;
        let lake = remote(addr);
        let slow = Slow {
            wait: Box::pin(tokio::time::sleep(SILENCE * 3)),
            record: Some(record()),
        };
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(&importer(), &"bank".into(), Box::pin(slow))
            .await
            .expect("the ingest starts");
        let out = lakelands_mock::stream::collect(&mut commits).await;
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].as_ref().map(|c| c.records), Ok(1));
    }
}
