//! The store behind the contract's traits: [`Catalog`] for callers,
//! [`LakeStore`] for the host, [`ChangeFeed`] for consumers, and [`open`].
//!
//! Every call names a principal of this lake; a principal of another lake
//! holds no grant here. Engine work goes through [`crate::exec::run`], and
//! nowhere else.

use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::location::Location;
use lakelands_api::ontology::{FieldDecl, ValueTypeDecl};
use lakelands_api::principal::WriteContext;
use lakelands_api::query::{Hits, Pattern, QueryResult, Search};
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter, Seams, VerbInvocation};
use lakelands_api::write::{
    AgeReport, CompactReport, Declaration, Migration, MigrationReport, Outcome, Proposal,
    ProposalFilter, ProposedWrite, PurgeReport, Recompute, ValueTypeReport, WriteOpts,
};
use lakelands_api::{
    Catalog, Error, Find, Found, Get, LakeId, LakeStore, Patch, Position, Principal, ProposalId,
    ReadAt, Record, RecordKey, RelationName, Reply, Result, SourceId, Timestamp, TypeName,
    VerbName, Verification,
};

use lakelands_core::{invalid, unavailable};

use crate::exec;
use crate::ingest;
use crate::lake::Lake;
use crate::reads::Reader;
use crate::writes::{self, Call, External};

/// Open a lake at a location. The host passes the engines it compiled in
/// (and any seams it binds) in `seams`; the store depends on no engine.
pub async fn open(location: Location, seams: Seams, mode: OpenMode) -> Result<Arc<dyn LakeStore>> {
    let lake = exec::run(move || Lake::open(&location, seams, mode)).await?;
    Ok(Arc::new(Store {
        lake: Arc::new(lake),
    }))
}

/// Import the export in `dir` as a new lake at `location`, on any engine
/// (`crate::import`): the lake holds the exported log, event for event,
/// with its projection folded, and is closed again; the host opens it as
/// any other lake.
pub async fn import(
    location: Location,
    seams: Seams,
    dir: std::path::PathBuf,
) -> Result<crate::import::Imported> {
    exec::run(move || crate::import::import(&location, seams, &dir)).await
}

fn same_lake(lake: &Lake, who: &Principal) -> Result<()> {
    writes::granted(who.lake == lake.id)
}

/// The store facade of one lake.
pub struct Store {
    lake: Arc<Lake>,
}

/// The doors of one lake, bound to a host's write context.
pub struct Doors {
    lake: Arc<Lake>,
    context: WriteContext,
}

impl Doors {
    async fn read<T, F>(&self, who: &Principal, at: ReadAt, f: F) -> Result<Reply<T>>
    where
        T: Send + 'static,
        F: FnOnce(&Reader<'_>) -> Result<T> + Send + 'static,
    {
        same_lake(&self.lake, who)?;
        let (lake, who) = (self.lake.clone(), who.clone());
        exec::run(move || {
            // The list comes from the view the read reads, so that a rename
            // is in its shape and in its grants, or in neither.
            let view = lake.engine.view(at.as_of)?;
            let head = at.as_of.is_none().then_some(view.as_ref());
            let (who, _) = lake.followed(who, at.as_of, head)?;
            let r = Reader::open(&lake, &who, at, view)?;
            let data = f(&r)?;
            Ok(Reply {
                position: r.position,
                data,
            })
        })
        .await
    }

    async fn write<T, F>(&self, who: &Principal, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Call<'_>) -> Result<T> + Send + 'static,
    {
        same_lake(&self.lake, who)?;
        let (lake, who, context) = (self.lake.clone(), who.clone(), self.context);
        exec::run(move || {
            let (who, followed) = lake.followed(who, None, None)?;
            f(&Call {
                lake: &lake,
                who: &who,
                context,
                followed,
            })
        })
        .await
    }

    /// How often the lake's due calls were drained, read before a write.
    fn seen(&self) -> u64 {
        self.lake.drains.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Make the calls of backed functions the write made due, and answer
    /// its outcome at the position of the last output recorded, from which
    /// the filled fields are visible. Of what the write set off, it names
    /// only what [`lakelands_core::describe::named_reactions`] leaves `who` (024);
    /// `through` still covers it all. A drain that fails is not the
    /// write's failure, but an external verb is handed to its executor
    /// only after a drain that did not fail: after one that did, the
    /// writer may be behind and the executor's answer could not be
    /// recorded, so nothing is handed and [`writes::not_handed`] answers.
    async fn handed(
        &self,
        who: &Principal,
        (mut outcome, external): (Outcome, Option<External>),
        seen: u64,
        accepted: bool,
    ) -> Result<Outcome> {
        let last = crate::backed::settle(&self.lake, seen).await;
        let last = match &external {
            Some(_) => last.map_err(|e| writes::not_handed(e, true, accepted))?,
            None => last.unwrap_or(None),
        };
        let named = matches!(&outcome, Outcome::Applied { set_off, .. } if set_off.names_any());
        let lake = if named {
            Some(self.describe(who, ReadAt::default()).await?.data)
        } else {
            None
        };
        if let Outcome::Applied {
            position, set_off, ..
        } = &mut outcome
        {
            *position = (*position).max(last.unwrap_or(*position));
            set_off.through = set_off.through.filter(|t| t > position);
            if let Some(lake) = lake {
                lakelands_core::describe::named_reactions(&lake, set_off);
            }
        }
        match external {
            Some(ext) => Ok(writes::with_output(outcome, self.execute(ext).await?)),
            None => Ok(outcome),
        }
    }

    /// A write whose outcome is [`Doors::handed`]'s.
    async fn write_settled<F>(&self, who: &Principal, f: F) -> Result<Outcome>
    where
        F: FnOnce(&Call<'_>) -> Result<Outcome> + Send + 'static,
    {
        let seen = self.seen();
        let outcome = self.write(who, f).await?;
        self.handed(who, (outcome, None), seen, false).await
    }

    /// Hand an external verb to its executor and record the answer; the
    /// recorded output is returned, `{"error": …}` when the executor failed.
    async fn execute(&self, ext: External) -> Result<(Position, serde_json::Value)> {
        let executor = writes::bound_executor(&self.lake, &ext.executor)?.clone();
        // The executor is the host's own code: it receives the record whole,
        // unmasked by the grants of whoever made the verb due. The caller
        // sees only the outcome and the recorded output. A verb on its type
        // has no record to hand it.
        let record = match &ext.key {
            Some(key) => {
                let get = Get {
                    key: key.clone(),
                    with_meta: false,
                    at: ReadAt::default(),
                    params: Default::default(),
                };
                let host = self.lake.system("host");
                Some(self.get(&host, &ext.ty, get).await?.data)
            }
            None => None,
        };
        let invocation = VerbInvocation {
            lake: self.lake.id.clone(),
            verb: ext.verb.clone(),
            record,
            payload: ext.payload.clone(),
            position: ext.position,
        };
        let input = serde_json::json!({
            "verb": invocation.verb, "record": invocation.record,
            "payload": invocation.payload, "position": invocation.position,
        });
        let output = match executor.execute(&invocation).await {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };
        let lake = self.lake.clone();
        let recorded = output.clone();
        let at = exec::run(move || writes::record_output(&lake, &ext, &input, output)).await?;
        Ok((at, recorded))
    }
}

#[async_trait]
impl Catalog for Doors {
    fn lake(&self) -> &LakeId {
        &self.lake.id
    }

    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        self.read(who, at, |r| Ok(r.describe())).await
    }

    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        let ty = ty.clone();
        self.read(who, at, move |r| r.describe_type(&ty)).await
    }

    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        let at = cursor_at(find.at, find.cursor.as_ref())?;
        let ty = ty.clone();
        self.read(who, at, move |r| r.find(&ty, &find)).await
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        let ty = ty.clone();
        self.read(who, get.at, move |r| r.get(&ty, &get)).await
    }

    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let (ty, key) = (ty.clone(), key.clone());
        self.write_settled(who, move |c| c.upsert(&ty, &key, patch, opts))
            .await
    }

    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let (ty, key) = (ty.clone(), key.clone());
        self.write_settled(who, move |c| c.retire(&ty, &key, opts))
            .await
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
        let (ty, key, relation, to) = (ty.clone(), key.clone(), relation.clone(), to.clone());
        self.write_settled(who, move |c| {
            c.link(&ty, &key, &relation, &to, fields, opts)
        })
        .await
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
        let (ty, key, relation, to) = (ty.clone(), key.clone(), relation.clone(), to.clone());
        self.write_settled(who, move |c| c.unlink(&ty, &key, &relation, &to, opts))
            .await
    }

    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let (verb, key) = (verb.clone(), key.cloned());
        let seen = self.seen();
        let called = self
            .write(who, move |c| c.call(&verb, key.as_ref(), payload, opts))
            .await?;
        self.handed(who, called, seen, false).await
    }

    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        let at = cursor_at(pattern.at, pattern.cursor.as_ref())?;
        self.read(who, at, move |r| r.query(&pattern)).await
    }

    async fn search(&self, who: &Principal, mut search: Search) -> Result<Reply<Hits>> {
        // A hybrid search without a caller's vector embeds its text through
        // the bound embedding backend, if the host bound one. This is a
        // read: nothing is recorded.
        if let (
            lakelands_api::query::SearchMode::Hybrid {
                text,
                field,
                vector: vector @ None,
                ..
            },
            Some(embed),
        ) = (&mut search.mode, &self.lake.seams.embed)
        {
            // A field declared as the embedding of a model is searched with
            // that model's vectors only: another bound model, or one making
            // vectors of other dimensions, is a configuration to fix, not
            // weather.
            let declared = {
                let w = self.lake.writer();
                w.ontology.ty(&search.ty).ok().and_then(|t| {
                    let f = t.fields.iter().find(|f| &f.name == field)?;
                    let e = f.embed.as_ref()?;
                    Some((e.model.clone(), f.ty.dimensions.unwrap_or(0)))
                })
            };
            if let Some((model, dimensions)) = declared
                && (model != embed.model() || dimensions != embed.dimensions())
            {
                return Err(Error::Unbound {
                    seam: format!(
                        "the embedding model `{model}` with {dimensions} dimensions that \
                         `{}.{field}` is declared with (the bound one is `{}` with {})",
                        search.ty,
                        embed.model(),
                        embed.dimensions()
                    ),
                });
            }
            let mut vectors =
                embed
                    .embed(std::slice::from_ref(text))
                    .await
                    .map_err(|e| match e {
                        Error::Unavailable { message } => Error::Unavailable { message },
                        other => unavailable(format!(
                            "the embedding backend `{}` failed: {other}",
                            embed.model()
                        )),
                    })?;
            if vectors.len() != 1 {
                return Err(unavailable(format!(
                    "the embedding backend `{}` answered {} vectors for one text",
                    embed.model(),
                    vectors.len()
                )));
            }
            *vector = vectors.pop();
        }
        let at = cursor_at(search.at, search.cursor.as_ref())?;
        self.read(who, at, move |r| r.search(&search)).await
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        self.write(who, move |c| c.propose(write, why)).await
    }

    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        let p = proposal.clone();
        let seen = self.seen();
        let accepted = self.write(who, move |c| c.accept(&p)).await?;
        self.handed(who, accepted, seen, true).await
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        let p = proposal.clone();
        self.write(who, move |c| c.reject(&p, reason)).await
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.read(who, filter.at, move |r| r.proposals(&filter))
            .await
    }

    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        self.write_settled(who, move |c| c.declare(declaration))
            .await
    }

    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        self.write_settled(who, move |c| c.apply(declarations))
            .await
    }

    async fn recompute(&self, who: &Principal, what: Recompute) -> Result<Outcome> {
        self.write_settled(who, move |c| c.recompute(&what)).await
    }

    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        let ty = ty.clone();
        self.write_settled(who, move |c| c.add_field(&ty, field))
            .await
    }

    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        self.write(who, move |c| {
            let w = c.writer()?;
            let view = c.lake.engine.view(None)?;
            Ok(Reply {
                position: view.position().unwrap_or(Position::GENESIS),
                data: c.preflight(&w, view.as_ref(), &migration)?,
            })
        })
        .await
    }

    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        let id = report_id.to_owned();
        self.write_settled(who, move |c| c.migrate_apply(migration, &id))
            .await
    }

    async fn migrate_value_type(
        &self,
        who: &Principal,
        to: ValueTypeDecl,
    ) -> Result<Reply<ValueTypeReport>> {
        self.write(who, move |c| {
            crate::writes::granted(c.who.grants.schema)?;
            let w = c.writer()?;
            let view = c.lake.engine.view(None)?;
            Ok(Reply {
                position: view.position().unwrap_or(Position::GENESIS),
                data: c
                    .shown_value_type(&w.ontology, c.preflight_value_type(&w, view.as_ref(), &to)?),
            })
        })
        .await
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        to: ValueTypeDecl,
        report_id: &str,
    ) -> Result<Outcome> {
        let id = report_id.to_owned();
        self.write_settled(who, move |c| c.migrate_value_type_apply(to, &id))
            .await
    }

    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        same_lake(&self.lake, who)?;
        // The context first, as on every write door, and before the stream
        // is taken.
        writes::check_context(&self.lake, &self.context)?;
        writes::granted(who.grants.ingest)?;
        Ok(ingest::run(
            self.lake.clone(),
            who.clone(),
            self.context,
            source.clone(),
            records,
        ))
    }
}

/// `at`, moved to the position a cursor names when there is one.
fn cursor_at(mut at: ReadAt, cursor: Option<&lakelands_api::Cursor>) -> Result<ReadAt> {
    let Some(c) = cursor else {
        return Ok(at);
    };
    #[derive(serde::Deserialize)]
    struct At {
        at: Position,
    }
    let bytes: Option<Vec<u8>> = (0..c.0.len())
        .step_by(2)
        .map(|i| {
            c.0.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect();
    let a = bytes.and_then(|b| serde_json::from_slice::<At>(&b).ok());
    at.as_of = Some(
        a.ok_or_else(|| invalid("the cursor is not one this lake handed out"))?
            .at,
    );
    Ok(at)
}

#[async_trait]
impl LakeStore for Store {
    fn lake(&self) -> &LakeId {
        &self.lake.id
    }

    fn is_writer(&self) -> bool {
        self.lake.is_writer()
    }

    fn limits(&self) -> lakelands_api::location::Limits {
        self.lake.limits
    }

    async fn head(&self) -> Result<Position> {
        Ok(self.lake.head())
    }

    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog> {
        Arc::new(Doors {
            lake: self.lake.clone(),
            context,
        })
    }

    fn feed(&self) -> Arc<dyn ChangeFeed> {
        Arc::new(Feed {
            lake: self.lake.clone(),
        })
    }

    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification> {
        let lake = self.lake.clone();
        exec::run(move || lake.verify(from, through)).await
    }

    async fn rebuild(&self) -> Result<Position> {
        let lake = self.lake.clone();
        exec::run(move || lake.rebuild()).await
    }

    async fn restrict(&self, ty: &TypeName, key: &RecordKey, reason: String) -> Result<Position> {
        let (lake, ty, key) = (self.lake.clone(), ty.clone(), key.clone());
        exec::run(move || writes::restrict(&lake, &ty, &key, Some(reason))).await
    }

    async fn unrestrict(&self, ty: &TypeName, key: &RecordKey) -> Result<Position> {
        let (lake, ty, key) = (self.lake.clone(), ty.clone(), key.clone());
        exec::run(move || writes::restrict(&lake, &ty, &key, None)).await
    }

    async fn age(&self, today: Timestamp) -> Result<AgeReport> {
        let lake = self.lake.clone();
        let seen = lake.drains.load(std::sync::atomic::Ordering::Acquire);
        let mut report = exec::run(move || crate::aging::run(&lake, today)).await?;
        // The moves' backed fields settle like any write's; the report
        // names the position from which what they filled is visible.
        if let Some(last) = crate::backed::settle_after(&self.lake, seen).await {
            report.position = report.position.max(last);
        }
        Ok(report)
    }

    async fn purge_preflight(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
    ) -> Result<PurgeReport> {
        let (lake, source, ty) = (self.lake.clone(), source.clone(), ty.cloned());
        exec::run(move || crate::purge::preflight(&lake, &source, ty.as_ref())).await
    }

    async fn purge_source(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
        id: Option<&str>,
    ) -> Result<PurgeReport> {
        let (lake, source, id) = (self.lake.clone(), source.clone(), id.map(str::to_owned));
        let (ty, seen) = (
            ty.cloned(),
            lake.drains.load(std::sync::atomic::Ordering::Acquire),
        );
        let purge = move || crate::purge::run(&lake, &source, ty.as_ref(), id.as_deref());
        let mut report = exec::run(purge).await?;
        // The retirements' backed fields settle like any write's.
        if let Some(last) = crate::backed::settle_after(&self.lake, seen).await {
            report.position = report.position.max(last);
        }
        Ok(report)
    }

    async fn position_at(&self, known_at: Timestamp) -> Result<Option<Position>> {
        let lake = self.lake.clone();
        exec::run(move || lake.position_at(known_at)).await
    }

    async fn vector_coverage(
        &self,
        ty: &TypeName,
    ) -> Result<Vec<lakelands_api::read::VectorCoverage>> {
        let (lake, ty) = (self.lake.clone(), ty.clone());
        exec::run(move || lake.vector_coverage(&ty)).await
    }

    async fn compact(&self) -> Result<CompactReport> {
        let lake = self.lake.clone();
        exec::run(move || lake.compact()).await
    }

    async fn checkpoint(&self) -> Result<Position> {
        let lake = self.lake.clone();
        exec::run(move || lake.checkpoint()).await
    }

    async fn close(&self) -> Result<()> {
        let lake = self.lake.clone();
        exec::run(move || lake.close()).await
    }
}

/// The change feed: the log, filtered by kind and type, read from a
/// position (`docs/decisions/004-the-change-feed-is-the-log.md`).
pub struct Feed {
    lake: Arc<Lake>,
}

/// The type an event is about, and the record, by its key: both for the
/// `record_*` events, `trigger_fired` and `object_set_changed`; the type
/// for a type's `declared` and `migrated`; the record for `verb_called`.
fn about(body: &EventBody) -> (Option<&TypeName>, Option<&RecordKey>) {
    match body {
        EventBody::RecordUpserted { ty, key, .. }
        | EventBody::RecordIngested { ty, key, .. }
        | EventBody::RecordRetired { ty, key, .. }
        | EventBody::RecordRestricted { ty, key, .. }
        | EventBody::RecordUnrestricted { ty, key, .. }
        | EventBody::TriggerFired { ty, key, .. }
        | EventBody::ObjectSetChanged { ty, key, .. } => (Some(ty), Some(key)),
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => (Some(&t.name), None),
        EventBody::Migrated { migration, .. } => (Some(&migration.to.name), None),
        EventBody::VerbCalled { key, .. } => (None, key.as_ref()),
        _ => (None, None),
    }
}

/// Whether a `migrated` event is also about one of `types`: the link
/// types its renamed relations move records between, old and new, so a
/// consumer that follows either hears of the move (035, extended).
fn moves(body: &EventBody, types: &[TypeName]) -> bool {
    let EventBody::Migrated { migration, .. } = body else {
        return false;
    };
    let moved = lakelands_core::links::moved(migration);
    (moved.iter()).any(|(old, new)| types.contains(old) || types.contains(new))
}

#[async_trait]
impl ChangeFeed for Feed {
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch> {
        let (lake, filter) = (self.lake.clone(), filter.clone());
        exec::run(move || {
            let head = lake.head();
            let want = limit.max(1) as usize;
            let mut events = Vec::new();
            let mut next = from;
            // Read in batches, and stop at `limit` matches or the head:
            // nothing past the last position examined is read or verified.
            'read: while next <= head && events.len() < want {
                let batch = lake.read_events(next, want.clamp(64, 1024) as u32)?;
                if batch.is_empty() {
                    break;
                }
                for e in batch {
                    if e.position > head {
                        break 'read;
                    }
                    next = e.position.next();
                    let kind_ok =
                        filter.kinds.is_empty() || filter.kinds.iter().any(|k| k == e.body.kind());
                    let (ty, key) = about(&e.body);
                    let type_ok = filter.types.is_empty()
                        || ty.is_some_and(|t| filter.types.contains(t))
                        || moves(&e.body, &filter.types);
                    let key_ok =
                        filter.keys.is_empty() || key.is_some_and(|k| filter.keys.contains(k));
                    if kind_ok && type_ok && key_ok {
                        events.push(e);
                        if events.len() >= want {
                            break 'read;
                        }
                    }
                }
            }
            Ok(FeedBatch { events, next, head })
        })
        .await
    }

    async fn ack(&self, _consumer: &str, _through: Position) -> Result<()> {
        // The store keeps no consumer state: the consumer keeps its own
        // position.
        Ok(())
    }
}

#[cfg(test)]
mod tests;
