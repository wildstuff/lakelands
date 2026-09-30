//! What every check needs: principals, a small ontology, and reading
//! back.
//!
//! Stated in contract types only, so that a check reads as the rule it
//! checks and an engine is never measured by anything but its doors.
//! The helpers that can fail answer a [`Failure`] naming what was being
//! done; [`Must`] does the same for any call a check makes itself.

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Access, Catalog, Committed, Error, FieldName, Find, Found, Get, Grants, LakeStore, Outcome,
    Patch, Position, Principal, PrincipalKind, ReadAt, Record, RecordKey, SourceId, SourceRecord,
    SourceStream, TypeName, Value, WriteContext, WriteOpts,
};
use rust_decimal::Decimal;

use super::Failure;

/// Turns the result of a contract call into the check's own result.
pub trait Must<T> {
    /// The value, or a failure saying that `what` failed and why.
    fn must(self, what: &str) -> Result<T, Failure>;

    /// The error, or a failure saying that `what` should have been
    /// refused and was not.
    fn must_fail(self, what: &str) -> Result<Error, Failure>;
}

impl<T: Debug> Must<T> for lakelands_api::Result<T> {
    fn must(self, what: &str) -> Result<T, Failure> {
        self.map_err(|e| Failure::new(format!("{what} failed: {e} ({e:?})")))
    }

    fn must_fail(self, what: &str) -> Result<Error, Failure> {
        match self {
            Ok(v) => Err(Failure::new(format!(
                "{what} should have been refused, but it answered {v:?}"
            ))),
            Err(e) => Ok(e),
        }
    }
}

/// The limits every lake of the suite is opened with: the defaults, as
/// [`Harness`](super::Harness) requires. A check that meets one reads it
/// here; [`default_limits`] holds every lake to it before any check runs.
pub fn limits() -> lakelands_api::location::Limits {
    lakelands_api::location::Limits::default()
}

/// Fail unless `store` was opened with the default limits, so a harness
/// that opens lakes otherwise fails here, naming the limits, and never
/// in a check that happens to meet one.
pub fn default_limits(store: &dyn LakeStore) -> Result<(), Failure> {
    let got = store.limits();
    if got == limits() {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "the harness opened the lake with limits {got:?}; the suite runs on the \
             defaults, {:?}",
            limits()
        )))
    }
}

/// The doors of `store` for a direct caller.
pub fn doors(store: &dyn LakeStore) -> Arc<dyn Catalog> {
    store.catalog(WriteContext::default())
}

/// The lake's owner: a person holding every grant.
pub fn owner(store: &dyn LakeStore) -> Principal {
    principal(store, PrincipalKind::Human, "owner", Grants::owner())
}

/// A principal of the lake with the given grants.
pub fn principal(
    store: &dyn LakeStore,
    kind: PrincipalKind,
    id: &str,
    grants: Grants,
) -> Principal {
    Principal {
        kind,
        id: id.to_owned(),
        lake: store.lake().clone(),
        grants,
    }
}

/// An agent that reads everything and may write only `fields` of `ty`.
pub fn field_writer(store: &dyn LakeStore, id: &str, ty: &str, fields: &[&str]) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    grants.types.insert(
        TypeName::from(ty),
        TypeGrants {
            default: None,
            fields: fields
                .iter()
                .map(|f| (FieldName::from(*f), Access::Write))
                .collect(),
        },
    );
    principal(store, PrincipalKind::Agent, id, grants)
}

/// A field of a bare storage kind, authored, optional, with every other
/// member at its default.
pub fn field(name: &str, kind: StorageKind) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: FieldName::from(name),
        ty: FieldType::kind(kind),
        required: false,
        authority: Default::default(),
        derived: None,
        function: None,
        depends_on: Vec::new(),
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

/// A type whose records are keyed by the writer, with these fields.
pub fn type_decl(name: &str, fields: Vec<FieldDecl>) -> TypeDecl {
    TypeDecl {
        annotations: Default::default(),
        name: TypeName::from(name),
        key: Vec::new(),
        fields,
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    }
}

/// Declares a type as `who`.
pub async fn declare(cat: &dyn Catalog, who: &Principal, ty: TypeDecl) -> Result<Outcome, Failure> {
    let what = format!("declaring type `{}`", ty.name);
    cat.declare(who, Declaration::Type(ty)).await.must(&what)
}

/// A patch from a JSON object; anything but an object is an empty patch.
pub fn patch(fields: Value) -> Patch {
    match fields {
        Value::Object(map) => map.into_iter().map(|(k, v)| (FieldName(k), v)).collect(),
        _ => Patch::new(),
    }
}

/// Upserts `fields` into one record as `who`, with no options.
pub async fn upsert(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
    fields: Value,
) -> Result<Outcome, Failure> {
    let what = format!("upserting `{ty}` `{key}` as `{}`", who.writer());
    cat.upsert(
        who,
        &TypeName::from(ty),
        &RecordKey::from(key),
        patch(fields),
        WriteOpts::default(),
    )
    .await
    .must(&what)
}

/// A `get` of one record, with provenance, at `as_of` (the head when
/// `None`).
pub fn get_at(key: &str, as_of: Option<Position>) -> Get {
    Get {
        key: RecordKey::from(key),
        with_meta: true,
        at: ReadAt {
            as_of,
            valid_at: None,
        },
    }
}

/// Reads one record at the head, with provenance.
pub async fn read(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
) -> Result<Record, Failure> {
    let what = format!("reading `{ty}` `{key}`");
    let reply = cat
        .get(who, &TypeName::from(ty), get_at(key, None))
        .await
        .must(&what)?;
    Ok(reply.data)
}

/// Every record `find` answers for `ty` at the head, following cursors
/// until the last page.
pub async fn all(cat: &dyn Catalog, who: &Principal, ty: &str) -> Result<Vec<Record>, Failure> {
    all_found(cat, who, ty, Find::default()).await
}

/// Every record `find` answers for `ty` with this request, following
/// cursors until the last page (at most a thousand pages).
pub async fn all_found(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    mut find: Find,
) -> Result<Vec<Record>, Failure> {
    let mut out = Vec::new();
    for _ in 0..1000 {
        let reply = cat
            .find(who, &TypeName::from(ty), find.clone())
            .await
            .must(&format!("finding `{ty}` records"))?;
        match reply.data {
            Found::Records { rows, next_cursor } => {
                out.extend(rows);
                match next_cursor {
                    Some(cursor) => find.cursor = Some(cursor),
                    None => return Ok(out),
                }
            }
            Found::Groups { .. } => {
                return Err(Failure::new(format!(
                    "finding `{ty}` records without an aggregate answered groups"
                )));
            }
        }
    }
    Err(Failure::new(format!(
        "finding `{ty}` records never reached a last page"
    )))
}

/// The value of `field` in `record`, if it holds one.
pub fn value<'r>(record: &'r Record, field: &str) -> Option<&'r Value> {
    record.fields.get(field)
}

/// A decimal value (a string of digits) as an exact number; `None` for
/// anything else.
pub fn decimal(v: Option<&Value>) -> Option<Decimal> {
    v.and_then(Value::as_str)
        .and_then(|s| Decimal::from_str(s).ok())
}

/// An exact decimal from a literal the check itself writes.
pub fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap_or_default()
}

/// One record from a source.
pub fn source_record(ty: &str, key: &str, fields: Value) -> SourceRecord {
    SourceRecord {
        upstream: None,
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        fields: patch(fields),
        seen_at: None,
    }
}

/// A source stream over records already in hand.
pub fn stream(records: Vec<SourceRecord>) -> SourceStream {
    Box::pin(Records(records.into_iter()))
}

struct Records(std::vec::IntoIter<SourceRecord>);

impl Stream for Records {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        Poll::Ready(self.0.next())
    }
}

/// Feeds `records` from `source` and drains the answer: every window's
/// acknowledgement, or the first error, whether `ingest` refused the
/// call or a window failed.
pub async fn ingest(
    cat: &dyn Catalog,
    who: &Principal,
    source: &str,
    records: Vec<SourceRecord>,
) -> lakelands_api::Result<Vec<Committed>> {
    lakelands_api::ingest_all(cat, who, &SourceId::from(source), records).await
}

/// Fail unless the lake's head is still `before`: a refused write must
/// append nothing at all.
pub async fn head_unmoved(
    store: &dyn LakeStore,
    before: Position,
    what: &str,
) -> Result<(), Failure> {
    let after = store.head().await.must("reading the head")?;
    if after == before {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "{what} was refused, yet the head moved from {before} to {after}: \
             a refused write must append nothing"
        )))
    }
}

/// Fail unless an ingest's windows answer positions in log order and the
/// last of them is the lake's head: everything the delivery committed is
/// visible from the position its last window names.
pub async fn windows_reach_head(
    store: &dyn LakeStore,
    windows: &[Committed],
    what: &str,
) -> Result<(), Failure> {
    let head = store.head().await.must("reading the head")?;
    let Some(last) = windows.last() else {
        return Err(Failure::new(format!(
            "{what} answered no commit window at all"
        )));
    };
    if windows.windows(2).any(|w| w[1].position < w[0].position) {
        return Err(Failure::new(format!(
            "{what} answered windows out of log order: {windows:?}"
        )));
    }
    if last.position != head {
        return Err(Failure::new(format!(
            "{what} answered its last window at position {}, but the head is {head}",
            last.position
        )));
    }
    Ok(())
}

/// The records and duplicates a run of windows committed, summed.
pub fn committed(windows: &[Committed]) -> (u32, u32) {
    windows
        .iter()
        .fold((0, 0), |(r, d), w| (r + w.records, d + w.duplicates))
}

/// The position an outcome was recorded at, minus one: the last position
/// before the write.
pub fn before(outcome: &Outcome) -> Position {
    Position(outcome.position().0.saturating_sub(1))
}

/// One future of a check's concurrent callers.
pub type Caller<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Drive every caller to its end together, in the check's one task, and
/// answer their outputs in the callers' order. Each caller is polled
/// whenever the task wakes, so while one waits on the lake (the store
/// runs each write on its blocking pool, under its writer lock) the
/// others go on: the calls overlap at the store as the calls of many
/// clients do. No runtime is needed beyond the one the engine's test
/// brings.
pub async fn join_all<T>(callers: Vec<Caller<'_, T>>) -> Vec<T> {
    let mut pending: Vec<Option<Caller<'_, T>>> = callers.into_iter().map(Some).collect();
    let mut done: Vec<Option<T>> = pending.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut all = true;
        for (slot, out) in pending.iter_mut().zip(done.iter_mut()) {
            if let Some(f) = slot {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => {
                        *out = Some(v);
                        *slot = None;
                    }
                    Poll::Pending => all = false,
                }
            }
        }
        if all { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
    done.into_iter()
        .map(|v| v.expect("every caller ended"))
        .collect()
}

/// Wait `d` without a runtime: a thread sleeps and wakes the task. For a
/// check that must let the lake run on its own for a while, as a slow
/// consumer does.
pub async fn pause(d: std::time::Duration) {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut started = false;
    std::future::poll_fn(|cx| {
        if done.load(std::sync::atomic::Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !started {
            started = true;
            let (done, waker) = (done.clone(), cx.waker().clone());
            std::thread::spawn(move || {
                std::thread::sleep(d);
                done.store(true, std::sync::atomic::Ordering::Release);
                waker.wake();
            });
        }
        Poll::Pending
    })
    .await
}
