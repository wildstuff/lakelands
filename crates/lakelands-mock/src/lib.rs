//! An in-memory lake for the doors' tests.
//!
//! The CLI and the server are tested against this crate before the real
//! store exists, and keep being tested against it afterwards, so that a
//! door's behaviour is pinned to the contract rather than to one store.
//! It is a crate of its own and a dev-dependency of the door crates —
//! never a cargo feature: features are one per engine.
//!
//! [`MockLake`] implements [`LakeStore`], and the catalog it hands out
//! implements [`Catalog`]. A lake begins as every lake does: `lake_created`
//! at position 0 (fence 0), the creator's `lease_taken` at 1,
//! `sys.setting` declared at 2 and `sys.aging_rule` at 3. The mock keeps one thing, the log: a list of
//! events by position. Every read folds the log up to the position it is
//! read at, so `as_of` sees exactly the superseded state and a rebuild
//! changes nothing. What the doors can observe follows the contract
//! literally:
//!
//! - `upsert` is a sparse patch: an absent field is unchanged, `null`
//!   clears it; a field declared `on_input_change: clear` is cleared in
//!   the same event that changes one of its inputs;
//! - grants fail closed: one field the principal may not write refuses
//!   the whole write, a field of another authority is refused as the
//!   wrong door, declaring a `sys.` type is reserved (its records are
//!   written under grants like any other), and what the principal may
//!   not see is absent from `describe`, the tools and every read;
//! - `declare` refuses a reference to a type the lake does not hold;
//!   `apply` checks a set as one (a reference may point forward within
//!   it, tool names are checked over all of it), appends it in one append
//!   answered at its last position, or refuses it whole naming every
//!   problem;
//! - a write refuses an empty patch; on a create, the key fields a patch
//!   leaves out are read from the key, and a key must agree with its key
//!   fields; a revision is a declaration unless it reshapes stored records
//!   (027);
//! - `expected_version` answers `Conflict` and writes nothing; an
//!   idempotency key answers the first outcome and appends nothing;
//! - every reply carries a position, and every write advances it;
//! - verbs run by declared risk: `low` with a write effect runs, anything
//!   else is proposed; `propose`, `accept`, `reject` and `proposals` keep
//!   proposals in the log;
//! - `ingest` is content-addressed (source, type, key, the version before
//!   and the values, never when a record was seen), a record that would
//!   change nothing is a duplicate, within a window too; and the change
//!   feed is the log.
//!
//! It names what the log format names as the store does: a type's schema
//! hash is SHA-256 over its shape's canonical bytes, and a proposal's id
//! is its event's id, from its lake and position or its writer and
//! idempotency key.
//!
//! What it does not do, because no door test needs it: it persists
//! nothing, hashes no event's canonical bytes (the `prev` and `hash`
//! members are filled with stable stand-ins, and `verify` checks
//! nothing), hashes a view's conditions as written rather than typed, computes no
//! derived fields, views, triggers or object sets, binds no verb executor
//! (an external verb is refused as `unbound` when called), does not aggregate,
//! walks relations one hop over reference fields only, ranks search by
//! word count or cosine, and evaluates the expression functions listed in
//! its `expr` module and no others. Its clock is deterministic: an event's
//! `known_at` is one second per position after 2026-01-01, so that
//! transcripts of a door stay byte-stable.
//!
//! The door-facing behaviours are written once, in [`checks`], against
//! `Arc<dyn LakeStore>`, so that the same checks run on this mock and on
//! the real store, over both engines (`tests/checks_on_the_store.rs`).
//!
//! A refusal's sentence is not part of the contract; its kind and field
//! are. The mock words its refusals its own way, and no door test pins a
//! refusal's sentence.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::event::{Digest, Event, EventBody, EventId, LOG_FORMAT_VERSION, Procedure};
use lakelands_api::id::{IdempotencyKey, LakeId, Position, ProposalId, SourceId};
use lakelands_api::location::{Limits, Location};
use lakelands_api::principal::{Principal, WriteContext};
use lakelands_api::record::Origin;
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter};
use lakelands_api::time::Timestamp;
use lakelands_api::{Catalog, LakeStore, Verification};

mod aging;
pub mod checks;
mod doors;
mod expr;
mod links;
mod state;
pub mod stream;
mod value;

/// The writer recorded for events the lake appends by itself.
const SYSTEM_WRITER: &str = "system:lakelands";

/// An in-memory lake. Cheap to clone; clones share one log.
#[derive(Clone)]
pub struct MockLake {
    shared: Arc<Shared>,
}

struct Shared {
    lake: LakeId,
    limits: Limits,
    log: Mutex<Log>,
}

#[derive(Default)]
struct Log {
    events: Vec<Event>,
    /// Who proposed each proposal, so that `accept` applies the write
    /// under the proposer's grants.
    proposers: BTreeMap<ProposalId, Principal>,
}

impl MockLake {
    /// A new, empty lake with default limits. Its log begins as every
    /// lake's does: `LakeCreated` at position 0, the creator's
    /// `LeaseTaken` at 1, `sys.setting` declared at 2 and `sys.aging_rule`
    /// at 3.
    pub fn new(lake: impl Into<LakeId>) -> MockLake {
        MockLake::with_limits(lake.into(), Limits::default())
    }

    /// A new, empty lake configured by a location. Only the lake and the
    /// limits are read; the engine named is ignored.
    pub fn open(location: &Location) -> MockLake {
        MockLake::with_limits(location.lake.clone(), location.limits)
    }

    fn with_limits(lake: LakeId, limits: Limits) -> MockLake {
        let mock = MockLake {
            shared: Arc::new(Shared {
                lake,
                limits,
                log: Mutex::new(Log::default()),
            }),
        };
        let mut batch = Batch::new(0);
        batch.push(Draft::system(EventBody::LakeCreated {
            format: LOG_FORMAT_VERSION,
        }));
        batch.push(Draft::system(EventBody::LeaseTaken {
            fence: 1,
            holder: "lakelands mock".into(),
        }));
        for t in [
            lakelands_api::seam::setting_type(),
            lakelands_api::seam::aging_rule_type(),
        ] {
            batch.push(Draft::system(EventBody::Declared {
                schema_hash: Some(value::schema_hash(&t)),
                declaration: lakelands_api::write::Declaration::Type(t),
                version: 1,
            }));
        }
        mock.shared
            .append(&mut mock.shared.log(), WriteContext::default(), batch);
        mock
    }

    /// Every event of the log, in order.
    pub fn events(&self) -> Vec<Event> {
        self.shared.log().events.clone()
    }
}

impl Shared {
    fn log(&self) -> MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn head(log: &Log) -> Position {
        Position(log.events.len() as u64 - 1)
    }

    /// Append a batch at once, filling the envelope of every event.
    fn append(&self, log: &mut Log, context: WriteContext, batch: Batch) {
        for d in batch.drafts {
            let position = Position(log.events.len() as u64);
            let prev = log
                .events
                .last()
                .map_or_else(Digest::zero, |e| e.hash.clone());
            let mut e = Event {
                v: LOG_FORMAT_VERSION,
                lake: self.lake.clone(),
                position,
                id: d
                    .id
                    .unwrap_or_else(|| EventId(format!("{:032x}", position.0))),
                prev,
                known_at: clock(position),
                writer: d.writer,
                origin: d.origin,
                source: d.source,
                seen_at: d.seen_at,
                procedure: d.procedure,
                context,
                cause: d.cause,
                fence: u64::from(position.0 != 0),
                idempotency_key: d.key,
                body: d.body,
                hash: Digest::zero(),
            };
            let bytes = serde_json::to_vec(&e).unwrap_or_default();
            e.hash = Digest(value::hex(&bytes, 4));
            log.events.push(e);
        }
    }
}

/// The mock's clock: one second per position after 2026-01-01.
fn clock(p: Position) -> Timestamp {
    let base = Timestamp::from_date(2026, 1, 1).unwrap_or(Timestamp(0));
    Timestamp(base.0 + p.0 as i64 * 1_000_000)
}

/// One event before it is appended.
struct Draft {
    body: EventBody,
    writer: String,
    origin: Origin,
    source: Option<SourceId>,
    seen_at: Option<Timestamp>,
    procedure: Option<Procedure>,
    cause: Option<Position>,
    key: Option<IdempotencyKey>,
    id: Option<EventId>,
}

impl Draft {
    fn new(who: &Principal, origin: Origin, body: EventBody) -> Draft {
        Draft {
            body,
            writer: who.writer(),
            origin,
            source: None,
            seen_at: None,
            procedure: None,
            cause: None,
            key: None,
            id: None,
        }
    }

    fn system(body: EventBody) -> Draft {
        Draft {
            writer: SYSTEM_WRITER.to_owned(),
            ..Draft::new(&system(), Origin::System, body)
        }
    }
}

fn system() -> Principal {
    Principal {
        kind: lakelands_api::PrincipalKind::System,
        id: "lakelands".into(),
        lake: LakeId::from(""),
        grants: Default::default(),
    }
}

/// Events planned for one append; each knows its position in advance.
struct Batch {
    next: u64,
    drafts: Vec<Draft>,
}

impl Batch {
    fn new(next: u64) -> Batch {
        Batch {
            next,
            drafts: Vec::new(),
        }
    }

    fn at(log: &Log) -> Batch {
        Batch::new(log.events.len() as u64)
    }

    /// The position the next pushed event will have.
    fn peek(&self) -> Position {
        Position(self.next + self.drafts.len() as u64)
    }

    fn push(&mut self, d: Draft) -> Position {
        let p = self.peek();
        self.drafts.push(d);
        p
    }
}

fn refused(reason: Refusal) -> Error {
    Error::Refused {
        field: None,
        reason,
    }
}

#[async_trait]
impl LakeStore for MockLake {
    fn lake(&self) -> &LakeId {
        &self.shared.lake
    }

    fn is_writer(&self) -> bool {
        true
    }

    fn limits(&self) -> Limits {
        self.shared.limits
    }

    async fn head(&self) -> Result<Position> {
        Ok(Shared::head(&self.shared.log()))
    }

    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog> {
        Arc::new(doors::MockCatalog {
            shared: self.shared.clone(),
            context,
        })
    }

    fn feed(&self) -> Arc<dyn ChangeFeed> {
        Arc::new(Feed {
            shared: self.shared.clone(),
        })
    }

    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification> {
        let log = self.shared.log();
        let head = Shared::head(&log);
        let from = from.unwrap_or(Position::GENESIS);
        let through = through.unwrap_or(head);
        if through > head || from > through {
            return Err(refused(Refusal::OutOfRange { head }));
        }
        Ok(Verification {
            from,
            through,
            hash: log.events[through.0 as usize].hash.clone(),
        })
    }

    async fn rebuild(&self) -> Result<Position> {
        // Every read folds the log, so there is no projection to rebuild.
        Ok(Shared::head(&self.shared.log()))
    }

    async fn age(&self, today: Timestamp) -> Result<lakelands_api::write::AgeReport> {
        aging::run(&self.shared, &mut self.shared.log(), today)
    }

    async fn checkpoint(&self) -> Result<Position> {
        let mut log = self.shared.log();
        let head = Shared::head(&log);
        let hash = log.events[head.0 as usize].hash.clone();
        let mut batch = Batch::at(&log);
        let p = batch.push(Draft::system(EventBody::Checkpoint {
            through: head,
            hash,
            segments: Vec::new(),
        }));
        self.shared.append(&mut log, WriteContext::default(), batch);
        Ok(p)
    }

    async fn restrict(
        &self,
        _ty: &lakelands_api::TypeName,
        _key: &lakelands_api::RecordKey,
        _reason: String,
    ) -> Result<Position> {
        // Retention holds are a host operation the doors never call; the
        // mock keeps none and says so.
        Err(lakelands_api::Error::Unbound {
            seam: "retention holds (the mock keeps none)".into(),
        })
    }

    async fn unrestrict(
        &self,
        _ty: &lakelands_api::TypeName,
        _key: &lakelands_api::RecordKey,
    ) -> Result<Position> {
        Err(lakelands_api::Error::Unbound {
            seam: "retention holds (the mock keeps none)".into(),
        })
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// The change feed: the log, read by kind and type from a position.
struct Feed {
    shared: Arc<Shared>,
}

#[async_trait]
impl ChangeFeed for Feed {
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch> {
        let log = self.shared.log();
        let head = Shared::head(&log);
        let mut events = Vec::new();
        let mut next = from;
        for e in log.events.iter().skip(from.0 as usize) {
            if events.len() >= limit as usize {
                break;
            }
            next = e.position.next();
            let kind_ok =
                filter.kinds.is_empty() || filter.kinds.iter().any(|k| k == e.body.kind());
            let type_ok = filter.types.is_empty() || {
                let body = serde_json::to_value(&e.body).unwrap_or_default();
                filter.types.iter().any(|t| body["type"] == t.as_str())
            };
            if kind_ok && type_ok {
                events.push(e.clone());
            }
        }
        Ok(FeedBatch { events, next, head })
    }

    async fn ack(&self, _consumer: &str, _through: Position) -> Result<()> {
        Ok(())
    }
}
