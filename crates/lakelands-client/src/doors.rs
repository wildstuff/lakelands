//! The catalog over REST: each door is its route (`docs/rest.md`), its
//! arguments in the path, the query, the headers and the body, and its
//! answer the contract's own type.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

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
use tokio::time::Instant;

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
    ///
    /// Only a client that ran may call the answer silent. The bound is
    /// waited out as three quarters of it and a last quarter, which
    /// begins when the client sees the three quarters have passed, and
    /// begins again when its own timer fires more than a quarter late.
    /// So a silent server is refused at exactly the bound, and a client
    /// that did not run for a while (a suspended process, a stalled
    /// machine) hears the server for one more quarter of the bound of
    /// running time before it may give up. One stop it cannot tell from
    /// listening: one that begins in the last quarter and ends within a
    /// quarter after the bound. So a client stopped for up to just under
    /// two quarters of the bound (15 s of 30 s) may still call a working
    /// server silent; a longer stop, or one that began earlier, never.
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
            after: None,
            last: None,
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
    /// Fires when the stretch of silence being waited out has passed.
    silent: Pin<Box<tokio::time::Sleep>>,
    /// Since when the answer is silent after the upload.
    after: Option<Instant>,
    /// The last quarter of the bound: the silence it belongs to, by the
    /// moment that silence began, and when it ends.
    last: Option<(Instant, Instant)>,
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

    /// Whether the silence that began at `since` has lasted the bound, as
    /// a client that ran may say it. Not yet, and a timer is set:
    /// `Pending`. A timer fired, and the answer is to be looked at again:
    /// `false`.
    fn silent_for_the_bound(&mut self, since: Instant, cx: &mut Context<'_>) -> Poll<bool> {
        let quarter = self.wire.silence / 4;
        let first = since + (self.wire.silence - quarter);
        let now = Instant::now();
        let ends = match self.last.filter(|(of, _)| *of == since) {
            None if now < first => {
                self.last = None;
                first
            }
            Some((_, ends)) if now < ends => ends,
            Some((_, ends)) if now.duration_since(ends) <= quarter => return Poll::Ready(true),
            // Three quarters have passed in silence, or the last quarter's
            // timer came so late that the client cannot have run in it:
            // the last quarter begins now.
            _ => {
                let ends = now + quarter;
                self.last = Some((since, ends));
                ends
            }
        };
        self.silent.as_mut().reset(ends);
        self.silent.as_mut().poll(cx).map(|()| false)
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
                    let mut sending = !this.upload.ended.load(Ordering::SeqCst);
                    if sending {
                        // Hear when the upload moves or ends.
                        *this
                            .upload
                            .waiting
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
                        sending = !this.upload.ended.load(Ordering::SeqCst);
                    }
                    let since = if sending {
                        this.after = None;
                        let taken = this.upload.taken();
                        // Waiting on the source is the caller's pace.
                        if taken.on_source {
                            return Poll::Pending;
                        }
                        // A record is ready, the connection has not taken
                        // it and the answer is quiet: the server stopped
                        // reading.
                        taken.at.max(this.spoke)
                    } else {
                        *this.after.get_or_insert_with(Instant::now)
                    };
                    match this.silent_for_the_bound(since, cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(false) => continue,
                        Poll::Ready(true) => {}
                    }
                    this.done = true;
                    let secs = this.wire.silence.as_secs();
                    let e = this.wire.unavailable(match sending {
                        true => Exchange::Stalled { secs },
                        false => Exchange::SilentAfter { secs },
                    });
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => this.ended = true,
                Poll::Ready(Some(Ok(frame))) => {
                    this.after = None;
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
    use std::time::Duration;

    use futures_core::Stream;
    use lakelands_api::error::Error;
    use lakelands_api::location::Limits;
    use lakelands_api::record::SourceRecord;
    use lakelands_api::{LakeStore, Principal, WriteContext};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::time::Instant;

    use crate::{DEFAULT_MAX_ANSWER_BYTES, RemoteConfig, RemoteLake};

    // These tests run on a paused clock, over an in-memory connection:
    // time passes only when every task waits on it, so a silence is as
    // long as the test says, on any machine (lakelands#757).

    /// The silence bound of these tests.
    const SILENCE: Duration = Duration::from_secs(1);

    /// How often a working server sends an empty line: a tenth of the bound.
    const ALIVE: Duration = Duration::from_millis(100);

    /// How long the client and its server stand still in the tests that
    /// stop them: the clock jumps this far in one step.
    const STOPPED: Duration = Duration::from_secs(2);

    /// The configuration of these tests' clients.
    fn config() -> RemoteConfig {
        RemoteConfig {
            url: "http://door.test".into(),
            lake: "finance".into(),
            keys: BTreeMap::from([("importer".to_owned(), "k-importer".into())]),
            feed_key: None,
            ca_file: None,
            timeout_secs: 30,
            max_answer_bytes: DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        }
    }

    /// A client, and the server's end of its one connection.
    fn remote() -> (RemoteLake, DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(64 << 10);
        (
            RemoteLake::connect_over(config(), SILENCE, ours).unwrap(),
            theirs,
        )
    }

    /// The server reads the request's head and answers an ingest's.
    async fn head(s: &mut DuplexStream) {
        let mut seen = Vec::new();
        let mut b = [0u8; 1];
        while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
            seen.push(b[0]);
        }
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                    lakelands-position: 3\r\ntransfer-encoding: chunked\r\n\r\n";
        s.write_all(head.as_bytes()).await.unwrap();
    }

    /// The server reads the upload to its last chunk, then answers
    /// `line` and ends.
    async fn answer(s: &mut DuplexStream, line: &str) {
        let mut buf = vec![0u8; 64 << 10];
        let mut tail = Vec::new();
        while !tail.ends_with(b"0\r\n\r\n") {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            let keep = tail.len().saturating_sub(8);
            tail.drain(..keep);
        }
        let reply = format!("{:x}\r\n{line}\r\n0\r\n\r\n", line.len());
        s.write_all(reply.as_bytes()).await.unwrap();
    }

    /// The server says it works: an empty line.
    async fn alive(s: &mut DuplexStream) {
        s.write_all(b"1\r\n\n\r\n").await.unwrap();
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

    /// What an ingest of `records` answers, and how long it took.
    async fn ingest(
        lake: &RemoteLake,
        records: lakelands_api::catalog::SourceStream,
    ) -> (Vec<Result<u32, Error>>, Duration) {
        let asked = Instant::now();
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(&importer(), &"bank".into(), records)
            .await
            .expect("the ingest starts");
        let out = lakelands_mock::stream::collect(&mut commits).await;
        let out = out.into_iter().map(|c| c.map(|c| c.records)).collect();
        (out, asked.elapsed())
    }

    /// The one refusal an ingest ended with, as its message.
    fn unavailable(out: &[Result<u32, Error>]) -> String {
        let [Err(Error::Unavailable { situation })] = out else {
            panic!("expected one unavailable, got {out:?}");
        };
        let message = situation.to_string();
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(!message.contains("door.test"), "{message}");
        message
    }

    /// The upload has ended and the answer's head came, then nothing: the
    /// stream ends `Unavailable` when it has been silent for exactly the
    /// bound, naming where, and does not hang.
    #[tokio::test(start_paused = true)]
    async fn an_ingest_answer_silent_past_its_bound_after_the_upload_is_unavailable() {
        let (lake, mut s) = remote();
        tokio::spawn(async move {
            head(&mut s).await;
            // Nothing more is said, and the connection is held.
            tokio::time::sleep(SILENCE * 60).await;
        });
        let (out, waited) = ingest(&lake, lakelands_mock::stream::of(vec![record()])).await;
        let message = unavailable(&out);
        assert!(message.contains("after the records ended"), "{message}");
        assert_eq!(waited, SILENCE);
    }

    /// The server answered the head and stopped reading while records
    /// were still being sent, and says nothing: the stream ends
    /// `Unavailable` when the connection has taken nothing for exactly
    /// the bound, not a quarter of it later, and does not hang.
    #[tokio::test(start_paused = true)]
    async fn an_upload_a_silent_server_stops_reading_is_unavailable_at_the_bound() {
        let (lake, mut s) = remote();
        tokio::spawn(async move {
            head(&mut s).await;
            // Nothing more is read, and the connection is held.
            tokio::time::sleep(SILENCE * 60).await;
        });
        let (out, waited) = ingest(&lake, Box::pin(Endless)).await;
        let message = unavailable(&out);
        assert!(
            message.contains("while records were being sent"),
            "{message}"
        );
        assert_eq!(waited, SILENCE);
    }

    /// The refusal of an ingest whose server answered the head and then
    /// neither read nor said anything, the client standing still for
    /// `stopped` from `at` on, and how long it took: with `sending`, of
    /// records without end, else of one record.
    async fn stopped_then_silent(sending: bool, at: Duration, stopped: Duration) -> Duration {
        let (lake, mut s) = remote();
        tokio::spawn(async move {
            head(&mut s).await;
            tokio::time::sleep(at).await;
            tokio::time::advance(stopped).await;
            tokio::time::sleep(SILENCE * 60).await;
        });
        let records = match sending {
            true => Box::pin(Endless) as lakelands_api::catalog::SourceStream,
            false => lakelands_mock::stream::of(vec![record()]),
        };
        let (out, waited) = ingest(&lake, records).await;
        let (message, said) = (
            unavailable(&out),
            ["after the records ended", "while records"],
        );
        assert!(message.contains(said[usize::from(sending)]), "{message}");
        waited
    }

    /// The client did not run for a while, and the server is really
    /// silent: it is refused one quarter of the bound after the client
    /// ran again, neither at once nor never, with either refusal. Stopped
    /// for two bounds before the last quarter, the quarter begins at the
    /// wake-up; stopped in the last quarter for half a bound, its timer
    /// is more than a quarter late, and the quarter begins again
    /// (lakelands#757).
    #[tokio::test(start_paused = true)]
    async fn a_client_that_did_not_run_gives_a_silent_server_up_a_quarter_after_it_ran_again() {
        for sending in [true, false] {
            for (at, stopped) in [(SILENCE / 2, STOPPED), (SILENCE * 4 / 5, SILENCE / 2)] {
                let waited = stopped_then_silent(sending, at, stopped).await;
                assert_eq!(
                    waited,
                    at + stopped + SILENCE / 4,
                    "sending: {sending}, stopped for {stopped:?} at {at:?}"
                );
            }
        }
    }

    /// A bound whose quarter is under the timer's step is refused when
    /// the client is made: every timer would be late, and nothing judged.
    #[test]
    fn a_silence_bound_under_four_milliseconds_is_refused() {
        let made = |silence| RemoteLake::connect_with_silence(config(), silence).map(drop);
        assert!(made(Duration::from_micros(3_999)).is_err());
        assert!(made(Duration::from_millis(4)).is_ok());
    }

    /// A server that stops reading while a window commits, for three
    /// bounds, and sends an empty line meanwhile; then it reads the rest
    /// and answers. With `stopped`, client and server stand still for two
    /// bounds in the middle of it.
    async fn holds(mut s: DuplexStream, stopped: bool) {
        head(&mut s).await;
        for line in 0..30 {
            tokio::time::sleep(ALIVE).await;
            if stopped && line == 10 {
                tokio::time::advance(STOPPED).await;
            }
            alive(&mut s).await;
        }
        answer(&mut s, r#"{"position":5,"records":512,"duplicates":0}"#).await;
    }

    /// A server that stops reading while a window commits, for several
    /// times the bound, and sends an empty line meanwhile, is working: the
    /// upload waits for it and the answer comes through (lakelands#166).
    #[tokio::test(start_paused = true)]
    async fn an_upload_the_server_holds_while_it_answers_empty_lines_is_waited_for() {
        let (lake, s) = remote();
        tokio::spawn(holds(s, false));
        let (out, waited) = ingest(&lake, Box::pin(Many(512))).await;
        assert_eq!(out, [Ok(512)]);
        assert_eq!(waited, SILENCE * 3);
    }

    /// The client did not run for two bounds while a working server held
    /// its upload: its timer has fired when it runs again, before the
    /// server's next line has reached it. It does not call the server
    /// stalled on its own absence: the line comes within the last quarter
    /// it waits, and the answer comes through (lakelands#757).
    #[tokio::test(start_paused = true)]
    async fn a_client_that_did_not_run_while_the_server_held_its_upload_hears_it() {
        let (lake, s) = remote();
        tokio::spawn(holds(s, true));
        let (out, waited) = ingest(&lake, Box::pin(Many(512))).await;
        assert_eq!(out, [Ok(512)]);
        assert_eq!(waited, SILENCE * 3 + STOPPED);
    }

    /// The server takes the upload slowly, a few records every four
    /// fifths of the bound, and says nothing: each time, the client is in
    /// the last quarter of its bound. There, once, the client does not run
    /// for two bounds. The last quarter's timer is far too late to say the
    /// client was listening, so the quarter begins again, the server takes
    /// its next records within it, and the answer comes through
    /// (lakelands#757).
    #[tokio::test(start_paused = true)]
    async fn a_client_that_did_not_run_in_its_last_quarter_while_the_upload_flows_waits_it_again() {
        let (lake, mut s) = remote();
        tokio::spawn(async move {
            head(&mut s).await;
            let mut some = vec![0u8; 256 << 10];
            for round in 0..6 {
                tokio::time::sleep(SILENCE * 4 / 5).await;
                if round == 3 {
                    tokio::time::advance(STOPPED).await;
                }
                s.read_exact(&mut some).await.unwrap();
            }
            answer(&mut s, r#"{"position":5,"records":64,"duplicates":0}"#).await;
        });
        let (out, waited) = ingest(&lake, Box::pin(Many(64))).await;
        assert_eq!(out, [Ok(64)]);
        assert_eq!(waited, SILENCE * 4 / 5 * 6 + STOPPED);
    }

    /// While the source still has records to give, its pace is the
    /// caller's: a source slower than the bound is waited for, and the
    /// answer that follows the upload comes through.
    #[tokio::test(start_paused = true)]
    async fn a_source_slower_than_the_bound_is_waited_for() {
        let (lake, mut s) = remote();
        tokio::spawn(async move {
            head(&mut s).await;
            answer(&mut s, r#"{"position":5,"records":1,"duplicates":0}"#).await;
        });
        let slow = Slow {
            wait: Box::pin(tokio::time::sleep(SILENCE * 2)),
            record: Some(record()),
        };
        let (out, waited) = ingest(&lake, Box::pin(slow)).await;
        assert_eq!(out, [Ok(1)]);
        assert_eq!(waited, SILENCE * 2);
    }
}
