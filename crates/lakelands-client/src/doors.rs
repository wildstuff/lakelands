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
use lakelands_api::read::{CONTEXT_HEADER, PRINCIPAL_HEADER};
use lakelands_api::read::{Find, Found, Get, ReadAt, Reply};
use lakelands_api::record::{Patch, Record};
use lakelands_api::write::{
    Change, Committed, Declaration, IngestOpts, Migration, MigrationReport, Outcome, Plan,
    Proposal, ProposalFilter, ProposedWrite, Recompute, ValueTypeMigration, ValueTypeReport,
    WriteOpts,
};
use lakelands_api::{Catalog, WriteContext};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::ApiKey;
use crate::wire::{Call, Wire, query_of, seg};

/// The doors of a lake another process serves.
pub(crate) struct RemoteCatalog {
    wire: Arc<Wire>,
    /// A forwarding handle's write context, as the link's header carries
    /// it; every call names its principal beside it.
    context: Option<String>,
}

impl RemoteCatalog {
    pub(crate) fn new(wire: Arc<Wire>) -> RemoteCatalog {
        RemoteCatalog {
            wire,
            context: None,
        }
    }

    /// The doors over a forwarding handle's wire, carrying `context`.
    pub(crate) fn forwarding(wire: Arc<Wire>, context: &WriteContext) -> RemoteCatalog {
        let context = serde_json::to_string(context).expect("a write context prints");
        RemoteCatalog {
            wire,
            context: Some(context),
        }
    }

    /// The key `who` acts with, and `call` as `who` makes it.
    fn as_who<'w>(&'w self, who: &Principal, mut call: Call) -> Result<(&'w ApiKey, Call)> {
        let key = self.wire.key_of(who)?;
        if let Some(context) = &self.context {
            if HeaderValue::from_str(&who.id).is_err() {
                return Err(crate::wire::not_granted());
            }
            call = call
                .header(PRINCIPAL_HEADER, who.id.clone())
                .header(CONTEXT_HEADER, context.clone());
        }
        Ok((key, call))
    }

    async fn go<T: serde::de::DeserializeOwned>(&self, who: &Principal, call: Call) -> Result<T> {
        let (key, call) = self.as_who(who, call)?;
        self.wire.exchange(key, call).await
    }

    /// An ingest whose source is held in `slot` and read only once the
    /// answer's head says the door took the call; refused, the source is
    /// still in `slot`, unread, for the next door.
    pub(crate) async fn ingest_held(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        slot: Arc<Mutex<Option<SourceStream>>>,
    ) -> Result<CommitStream> {
        let gate = Arc::new(Gate::default());
        let held = Held {
            slot,
            gate: gate.clone(),
        };
        self.upload(who, source, opts, Box::pin(held), Some(gate))
            .await
    }

    async fn upload(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        records: SourceStream,
        gate: Option<Arc<Gate>>,
    ) -> Result<CommitStream> {
        let path = format!("ingest/{}", seg(source.as_str(), Segment::Source)?);
        let upload = Arc::new(Upload::default());
        let body = Ndjson {
            records,
            upload: upload.clone(),
            since: 0,
            drained: false,
        }
        .boxed_unsync();
        let call = Call::new(Method::POST, path.clone())
            .query(crate::wire::query_of(&serde_json::json!(opts)))
            .stream("application/x-ndjson", body);
        let (key, call) = self.as_who(who, call)?;
        let r = self.wire.send(key, call).await?;
        if !r.status().is_success() {
            // An ingest refused outright has no stream to end: its
            // upload ends here.
            upload.stop();
            return Err(self.wire.failure(r).await);
        }
        if let Some(gate) = gate {
            gate.open();
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
        }))
    }
}

/// Whether a held source may be read: once the answer's head has come.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
    waiting: Mutex<Option<Waker>>,
}

impl Gate {
    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        wake(&self.waiting);
    }
}

/// A source held for a door that may refuse the call: nothing is taken
/// from it before its gate opens.
struct Held {
    slot: Arc<Mutex<Option<SourceStream>>>,
    gate: Arc<Gate>,
}

impl Stream for Held {
    type Item = lakelands_api::record::SourceRecord;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.gate.open.load(Ordering::SeqCst) {
            *self.gate.waiting.lock().unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
            if !self.gate.open.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
        }
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            Some(s) => s.as_mut().poll_next(cx),
            None => Poll::Ready(None),
        }
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

    async fn accept_planned(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        plan_id: &str,
    ) -> Result<Outcome> {
        let path = format!(
            "proposals/{}/accept_planned",
            seg(proposal.as_str(), Segment::Proposal)?
        );
        let call = Call::new(Method::POST, path).json(&json!({ "plan_id": plan_id }));
        self.go(who, call).await
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

    async fn plan(&self, who: &Principal, change: Change) -> Result<Reply<Plan>> {
        let call = Call::new(Method::POST, "schema/plan".into()).json(&json!(change));
        self.go(who, call).await
    }

    async fn enact(&self, who: &Principal, change: Change, plan_id: &str) -> Result<Outcome> {
        let mut body = json!(change);
        body["plan_id"] = json!(plan_id);
        let call = Call::new(Method::POST, "schema/enact".into()).json(&body);
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

    /// [`Catalog::ingest_with`] under no options.
    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        self.ingest_with(who, source, IngestOpts::default(), records)
            .await
    }

    /// The records go as an NDJSON body as the source yields them; the
    /// answer's lines come back as they are written, one per window. A
    /// line that carries an error ends the stream with it, and what came
    /// before it is committed.
    ///
    /// The answer is read while the records are sent, at least every
    /// 512 KiB of them, so a line reaches the caller while the upload
    /// flows. (The upload yields after every 256 KiB it has handed over,
    /// and the connection reads at the latest at every second yield.)
    /// When the answer's stream ends, the upload ends at its next record:
    /// the source is not asked again, the body ends with its last chunk,
    /// and the source is dropped at the connection's next turn, which is
    /// at once unless the server has stopped reading.
    /// That holds for every ending, an error line, a refusal below and a
    /// caller that drops the stream alike, and for an ingest refused
    /// before any line: nobody would hear what the rest is answered.
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
    async fn ingest_with(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        records: SourceStream,
    ) -> Result<CommitStream> {
        self.upload(who, source, opts, records, None).await
    }
}

/// How much of an upload the body hands over before it yields, which
/// lets the connection read the answer. A source that is always ready,
/// to a server that always reads, never lets the connection off its
/// writing by itself. The connection takes the body up once more before
/// it reads, so the answer is read about every two of these.
const READ_EVERY: usize = 256 << 10;

/// How the upload goes: whether the answer's stream has ended, whether
/// the source has, when the connection last took a record, whether the
/// source is what it waits on, and who waits to hear of a change: the
/// answer's stream of the upload, and the upload, while it waits on its
/// source, of the stream's end.
struct Upload {
    over: AtomicBool,
    ended: AtomicBool,
    taken: Mutex<Taken>,
    waiting: Mutex<Option<Waker>>,
    sending: Mutex<Option<Waker>>,
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
            over: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            taken: Mutex::new(Taken {
                at: Instant::now(),
                on_source: false,
            }),
            waiting: Mutex::new(None),
            sending: Mutex::new(None),
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
            wake(&self.waiting);
        }
    }

    fn end(&self) {
        self.ended.store(true, Ordering::SeqCst);
        wake(&self.waiting);
    }

    /// The answer's stream has ended: nobody hears what more records
    /// would be answered, so the upload ends at its next record.
    fn stop(&self) {
        self.over.store(true, Ordering::SeqCst);
        wake(&self.sending);
    }
}

fn wake(waiting: &Mutex<Option<Waker>>) {
    let waiting = waiting.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(w) = waiting {
        w.wake();
    }
}

/// A source stream as an NDJSON body, one record per line.
struct Ndjson {
    records: SourceStream,
    upload: Arc<Upload>,
    /// The bytes handed over since the connection was last sent to read.
    since: usize,
    /// Whether the source ended while a frame was filled.
    drained: bool,
}

impl Body for Ndjson {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, std::io::Error>>> {
        let this = self.as_mut().get_mut();
        if this.drained {
            this.upload.end();
            return Poll::Ready(None);
        }
        if this.upload.over.load(Ordering::SeqCst) {
            // The source is not asked again: the body ends, and the
            // connection drops it with its source.
            this.upload.end();
            return Poll::Ready(None);
        }
        if this.since >= READ_EVERY {
            // Pending once lets the connection read the answer. It is not
            // the source's pace: the silence bound counts on.
            this.since = 0;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        match this.records.as_mut().poll_next(cx) {
            Poll::Pending => {
                this.upload.asked(true);
                // Hear when the answer's stream ends.
                *this
                    .upload
                    .sending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
                if this.upload.over.load(Ordering::SeqCst) {
                    // It ended before this could hear of it.
                    this.upload.end();
                    return Poll::Ready(None);
                }
                Poll::Pending
            }
            Poll::Ready(None) => {
                this.upload.end();
                Poll::Ready(None)
            }
            Poll::Ready(Some(r)) => {
                this.upload.asked(false);
                // Every record the source has ready leaves in one frame, so
                // the server reads them together. That is cost, not a
                // promise: a frame may be read in parts, and where a window
                // is cut is the store's (lakelands#886).
                let (mut frame, mut next) = (Vec::new(), Some(r));
                while let Some(r) = next.take() {
                    serde_json::to_writer(&mut frame, &r).expect("a record prints");
                    frame.push(b'\n');
                    // The answer's end stops the source here too.
                    if this.since + frame.len() >= READ_EVERY
                        || this.upload.over.load(Ordering::SeqCst)
                    {
                        break;
                    }
                    match this.records.as_mut().poll_next(cx) {
                        Poll::Ready(Some(r)) => next = Some(r),
                        Poll::Ready(None) => this.drained = true,
                        Poll::Pending => {}
                    }
                }
                this.since += frame.len();
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(frame)))))
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
    /// The answer's body has ended. That the stream has is the upload's
    /// `over`: a stream cannot end and leave its upload going.
    ended: bool,
}

impl Commits {
    fn line(&mut self, text: &[u8]) -> Result<Committed> {
        if let Ok(c) = serde_json::from_slice::<Committed>(text) {
            self.wire.saw(c.position);
            return Ok(c);
        }
        self.upload.stop();
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

/// A caller that drops the stream has no use for the rest of the upload.
impl Drop for Commits {
    fn drop(&mut self) {
        self.upload.stop();
    }
}

impl Stream for Commits {
    type Item = Result<Committed>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Committed>>> {
        let this = self.get_mut();
        loop {
            if this.upload.over.load(Ordering::SeqCst) {
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
                this.upload.stop();
                return Poll::Ready(Some(Err(this.wire.too_long())));
            }
            if this.ended {
                this.upload.stop();
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
                    this.upload.stop();
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
                    this.upload.stop();
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use futures_core::Stream;
    use lakelands_api::catalog::{CommitStream, SourceStream};
    use lakelands_api::error::Error;
    use lakelands_api::location::Limits;
    use lakelands_api::record::SourceRecord;
    use lakelands_api::{LakeStore, Principal, WriteContext};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::oneshot;
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
        remote_holding(64 << 10)
    }

    /// A client whose connection holds `bytes` the server has not read.
    fn remote_holding(bytes: usize) -> (RemoteLake, DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(bytes);
        (
            RemoteLake::connect_over(config(), SILENCE, ours).unwrap(),
            theirs,
        )
    }

    /// The server reads the request's head and answers an ingest's.
    async fn head(s: &mut DuplexStream) {
        let mut seen = Vec::new();
        let mut b = [0u8; 1];
        // Byte by byte, so that nothing of the body is taken with it, and
        // unconstrained: a task that reads this often is else set back
        // behind many turns of the connection's.
        tokio::task::unconstrained(async {
            while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                seen.push(b[0]);
            }
        })
        .await;
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                    lakelands-position: 3\r\ntransfer-encoding: chunked\r\n\r\n";
        s.write_all(head.as_bytes()).await.unwrap();
    }

    /// The server reads the upload to its last chunk, and says how much
    /// it read.
    async fn rest(s: &mut DuplexStream) -> usize {
        let mut buf = vec![0u8; 64 << 10];
        let (mut tail, mut read) = (Vec::new(), 0);
        while !tail.ends_with(b"0\r\n\r\n") {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "the upload ends with its last chunk");
            read += n;
            tail.extend_from_slice(&buf[..n]);
            let keep = tail.len().saturating_sub(8);
            tail.drain(..keep);
        }
        read
    }

    /// The server reads the upload to its last chunk, then answers
    /// `line` and ends.
    async fn answer(s: &mut DuplexStream, line: &str) {
        rest(s).await;
        let reply = format!("{:x}\r\n{line}\r\n0\r\n\r\n", line.len());
        s.write_all(reply.as_bytes()).await.unwrap();
    }

    /// The server writes one line of its answer.
    async fn says(s: &mut DuplexStream, line: &str) {
        let chunk = format!("{:x}\r\n{line}\n\r\n", line.len() + 1);
        s.write_all(chunk.as_bytes()).await.unwrap();
    }

    /// The server says it works: an empty line.
    async fn alive(s: &mut DuplexStream) {
        s.write_all(b"1\r\n\n\r\n").await.unwrap();
    }

    fn importer() -> Principal {
        lakelands_door_checks::checks::Cast::of(&"finance".into()).importer
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

    /// A large record: four of them are past what the connection takes
    /// before it reads the answer.
    fn large(n: usize) -> SourceRecord {
        serde_json::from_value(serde_json::json!({
            "type": "invoice", "key": format!("INV-{n}"),
            "fields": {"note": "n".repeat(64 << 10)}
        }))
        .unwrap()
    }

    /// Records without end, each large, so the connection's buffers fill.
    struct Endless;

    impl Stream for Endless {
        type Item = SourceRecord;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
            Poll::Ready(Some(large(1)))
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
            Poll::Ready(Some(large(self.0)))
        }
    }

    /// How many records a watched source holds: 16 MiB of them, 64 times
    /// what the connection takes before it reads the answer.
    const WATCHED: usize = 256;

    /// Large records, `left` of them, and then, if it `waits`, none for
    /// ever. It counts how often it is asked, and whoever holds the other
    /// end of `_held` hears when it is dropped.
    struct Watched {
        left: usize,
        waits: bool,
        asked: Arc<AtomicUsize>,
        _held: oneshot::Sender<()>,
    }

    impl Stream for Watched {
        type Item = SourceRecord;

        fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.left == 0 {
                return match self.waits {
                    true => Poll::Pending,
                    false => Poll::Ready(None),
                };
            }
            self.left -= 1;
            Poll::Ready(Some(large(self.left)))
        }
    }

    /// What an ingest of a watched source answers at once, how often the
    /// source was asked, and what ends when the source is dropped.
    async fn begun(
        lake: &RemoteLake,
        left: usize,
        waits: bool,
    ) -> (
        Result<CommitStream, Error>,
        Arc<AtomicUsize>,
        oneshot::Receiver<()>,
    ) {
        let (held, dropped) = oneshot::channel();
        let asked = Arc::new(AtomicUsize::new(0));
        let source = Watched {
            left,
            waits,
            asked: asked.clone(),
            _held: held,
        };
        let (catalog, who) = (lake.catalog(WriteContext::default()), importer());
        let commits = catalog.ingest(&who, &"bank".into(), Box::pin(source)).await;
        (commits, asked, dropped)
    }

    /// An ingest of a watched source, begun: its answer, how often the
    /// source was asked, and what ends when the source is dropped.
    async fn watched(
        lake: &RemoteLake,
        left: usize,
        waits: bool,
    ) -> (CommitStream, Arc<AtomicUsize>, oneshot::Receiver<()>) {
        let (commits, asked, dropped) = begun(lake, left, waits).await;
        (commits.expect("the ingest starts"), asked, dropped)
    }

    /// The answer's next line.
    async fn next(commits: &mut CommitStream) -> Option<Result<u32, Error>> {
        let line = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await;
        line.map(|c| c.map(|c| c.records))
    }

    /// What an ingest of `records` answers, and how long it took.
    async fn ingest(
        lake: &RemoteLake,
        records: SourceStream,
    ) -> (Vec<Result<u32, Error>>, Duration) {
        let asked = Instant::now();
        let mut commits = lake
            .catalog(WriteContext::default())
            .ingest(&importer(), &"bank".into(), records)
            .await
            .expect("the ingest starts");
        let out = lakelands_door_checks::stream::collect(&mut commits).await;
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
        let (out, waited) = ingest(&lake, lakelands_door_checks::stream::of(vec![record()])).await;
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
            true => Box::pin(Endless) as SourceStream,
            false => lakelands_door_checks::stream::of(vec![record()]),
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

    /// A bound whose quarter is under two steps of the timer is refused
    /// when the client is made: a timer fires up to a step late, so a
    /// quarter of one step is judged late, or not at all.
    #[test]
    fn a_silence_bound_under_eight_milliseconds_is_refused() {
        let made = |silence| RemoteLake::connect_with_silence(config(), silence).map(drop);
        assert!(made(Duration::from_micros(7_999)).is_err());
        assert!(made(Duration::from_millis(8)).is_ok());
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

    // The rows below hold what the connection reads and sends while a
    // ready source's upload flows (lakelands#825). Their connection holds
    // a whole upload, so no write ever waits for the server to read, and
    // the connection is sent to read by nothing but the upload's own
    // pauses.

    /// A line a server refuses an ingest with.
    const REFUSAL: &str = r#"{"error":"conflict","expected":1,"actual":2}"#;

    /// How often a watched source may have been asked when the caller
    /// has what the server wrote at once: an eighth of the upload. The
    /// connection takes a few records, reads, and takes a few more, and
    /// the answer's head and its line are the caller's a turn or two
    /// after they came. An upload that reads nothing has given all of it.
    const ASKED_AT_A_LINE: usize = WATCHED / 8;

    /// A client whose connection holds a whole watched upload.
    fn roomy() -> (RemoteLake, DuplexStream) {
        remote_holding(2 * WATCHED * (64 << 10))
    }

    /// Runs a row's caller as a task, as the connection and the server
    /// are. The test's own future is polled once in many turns of the
    /// tasks, and a caller there would see a line that many turns late.
    async fn as_a_task(caller: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(caller).await.expect("the caller ends");
    }

    /// A line of the answer reaches the caller while the upload flows,
    /// not when it has ended.
    #[tokio::test(start_paused = true)]
    async fn a_line_of_the_answer_is_seen_while_the_upload_flows() {
        let (lake, mut s) = roomy();
        tokio::spawn(async move {
            head(&mut s).await;
            says(&mut s, r#"{"position":5,"records":7,"duplicates":0}"#).await;
            // Nothing is read, and the connection is held.
            tokio::time::sleep(SILENCE * 60).await;
        });
        as_a_task(async move {
            let (mut commits, asked, _dropped) = watched(&lake, WATCHED, false).await;
            assert_eq!(next(&mut commits).await, Some(Ok(7)));
            let asked = asked.load(Ordering::SeqCst);
            assert!(asked <= ASKED_AT_A_LINE, "asked {asked} times");
        })
        .await;
    }

    /// A refusal ends the upload at the next record. The caller has it
    /// while the upload flows; from then on the source is not asked
    /// again, and it is dropped while the caller still holds the stream,
    /// so what it holds (a file, a cursor) is released by the refusal.
    /// The upload ends with its last chunk, so the exchange is a whole
    /// one, and the connection serves the next call.
    #[tokio::test(start_paused = true)]
    async fn a_refusal_ends_the_upload_at_the_next_record() {
        let (lake, mut s) = roomy();
        let server = tokio::spawn(async move {
            head(&mut s).await;
            says(&mut s, REFUSAL).await;
            // As the door does: it reads what still comes and drops it,
            // then ends its answer.
            let read = rest(&mut s).await;
            s.write_all(b"0\r\n\r\n").await.unwrap();
            head(&mut s).await;
            answer(&mut s, r#"{"position":5,"records":1,"duplicates":0}"#).await;
            read
        });
        as_a_task(async move {
            let (mut commits, asked, dropped) = watched(&lake, WATCHED, false).await;
            let refusal = next(&mut commits).await;
            assert!(
                matches!(refusal, Some(Err(Error::Conflict { .. }))),
                "{refusal:?}"
            );
            let then = asked.load(Ordering::SeqCst);
            assert!(then <= ASKED_AT_A_LINE, "asked {then} times");
            dropped.await.expect_err("the source is dropped");
            assert_eq!(asked.load(Ordering::SeqCst), then);
            assert_eq!(next(&mut commits).await, None);
            drop(commits);
            // Until the connection has seen the answer end.
            tokio::time::sleep(ALIVE).await;
            let (out, _) = ingest(&lake, lakelands_door_checks::stream::of(vec![record()])).await;
            assert_eq!(out, [Ok(1)]);
            let read = server.await.unwrap();
            assert!(
                read < (then + 1) * (65 << 10),
                "the server read {read} bytes"
            );
        })
        .await;
    }

    /// An ingest the server refuses outright, with a status and no line,
    /// ends its upload as a refused line does: the source is not asked
    /// again and is dropped, and the upload ends with its last chunk.
    #[tokio::test(start_paused = true)]
    async fn an_ingest_refused_outright_ends_its_upload_at_the_next_record() {
        let (lake, mut s) = roomy();
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            let mut b = [0u8; 1];
            tokio::task::unconstrained(async {
                while !seen.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap() > 0 {
                    seen.push(b[0]);
                }
            })
            .await;
            let refused = format!(
                "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{REFUSAL}",
                REFUSAL.len()
            );
            s.write_all(refused.as_bytes()).await.unwrap();
            rest(&mut s).await
        });
        as_a_task(async move {
            let (refusal, asked, dropped) = begun(&lake, WATCHED, false).await;
            let refusal = refusal.map(drop);
            assert!(
                matches!(refusal, Err(Error::Conflict { .. })),
                "{refusal:?}"
            );
            let then = asked.load(Ordering::SeqCst);
            assert!(then <= ASKED_AT_A_LINE, "asked {then} times");
            dropped.await.expect_err("the source is dropped");
            assert_eq!(asked.load(Ordering::SeqCst), then);
            let read = server.await.unwrap();
            assert!(
                read < (then + 1) * (65 << 10),
                "the server read {read} bytes"
            );
        })
        .await;
    }

    /// A source that waits for its next record when the stream ends is
    /// dropped then too, not when a record comes that may never come. The
    /// refusal comes with a window's line, in one read: when the caller
    /// takes it from the stream, nothing has arrived that would have the
    /// connection look at its upload, so the stream's end itself must.
    #[tokio::test(start_paused = true)]
    async fn a_source_that_waits_is_released_when_the_answer_ends() {
        let (lake, mut s) = roomy();
        tokio::spawn(async move {
            head(&mut s).await;
            tokio::time::sleep(ALIVE).await;
            let window = r#"{"position":5,"records":1,"duplicates":0}"#;
            says(&mut s, &format!("{window}\n{REFUSAL}")).await;
            tokio::time::sleep(SILENCE * 60).await;
        });
        as_a_task(async move {
            let (mut commits, asked, dropped) = watched(&lake, 1, true).await;
            assert_eq!(next(&mut commits).await, Some(Ok(1)));
            // Until the connection has nothing more to do.
            tokio::time::sleep(ALIVE).await;
            let refusal = next(&mut commits).await;
            assert!(
                matches!(refusal, Some(Err(Error::Conflict { .. }))),
                "{refusal:?}"
            );
            let then = asked.load(Ordering::SeqCst);
            let waited = tokio::time::timeout(SILENCE, dropped).await;
            waited
                .expect("the source is dropped by the refusal")
                .expect_err("dropped");
            assert_eq!(asked.load(Ordering::SeqCst), then);
        })
        .await;
    }

    /// A caller that drops the stream has no use for the rest: the source
    /// is not asked again, and is dropped.
    #[tokio::test(start_paused = true)]
    async fn a_dropped_stream_ends_the_upload_at_the_next_record() {
        let (lake, mut s) = roomy();
        let server = tokio::spawn(async move {
            head(&mut s).await;
            rest(&mut s).await
        });
        as_a_task(async move {
            let (commits, asked, dropped) = watched(&lake, WATCHED, false).await;
            let then = asked.load(Ordering::SeqCst);
            assert!(then <= ASKED_AT_A_LINE, "asked {then} times");
            drop(commits);
            dropped.await.expect_err("the source is dropped");
            assert_eq!(asked.load(Ordering::SeqCst), then);
            let read = server.await.unwrap();
            assert!(
                read < (then + 1) * (65 << 10),
                "the server read {read} bytes"
            );
        })
        .await;
    }

    /// A refusal of the connection's own ends the upload as the lake's
    /// does. A server that stopped reading is `Stalled` at the bound; when
    /// it reads again it finds the upload's last chunk after what the
    /// connection held, and the source was not asked again.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_upload_ends_at_the_next_record() {
        let (lake, mut s) = remote();
        let server = tokio::spawn(async move {
            head(&mut s).await;
            tokio::time::sleep(SILENCE * 2).await;
            rest(&mut s).await
        });
        as_a_task(async move {
            let asked_at = Instant::now();
            let (mut commits, asked, dropped) = watched(&lake, WATCHED, false).await;
            let refusal = next(&mut commits).await.expect("a refusal");
            let message = unavailable(&[refusal]);
            assert!(
                message.contains("while records were being sent"),
                "{message}"
            );
            assert_eq!(asked_at.elapsed(), SILENCE);
            let then = asked.load(Ordering::SeqCst);
            dropped.await.expect_err("the source is dropped");
            assert_eq!(asked.load(Ordering::SeqCst), then);
            let read = server.await.unwrap();
            assert!(
                read < (then + 1) * (65 << 10),
                "the server read {read} bytes"
            );
        })
        .await;
    }
}
