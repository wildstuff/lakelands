//! The store behind the contract's traits: [`Catalog`] for callers,
//! [`LakeStore`] for the host, [`ChangeFeed`] for consumers, and [`open`].
//!
//! Every call names a principal of this lake; a principal of another lake
//! holds no grant here. Engine work goes through [`crate::exec::run`], or
//! [`crate::exec::write`] where it takes the writer, and nowhere else.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::engine::OpenMode;
use lakelands_api::location::Location;
use lakelands_api::ontology::FieldDecl;
use lakelands_api::principal::WriteContext;
use lakelands_api::query::{Hits, Pattern, QueryResult, Search, SearchMode};
use lakelands_api::seam::{
    ChangeFeed, FEED_EXAMINED, FeedBatch, FeedFilter, Seams, VerbInvocation,
};
use lakelands_api::write::{
    AgeReport, Change, CompactReport, Declaration, ForgetReport, ForgetSetReport, IngestOpts,
    Migration, MigrationReport, Outcome, Plan, Proposal, ProposalFilter, ProposedWrite,
    PurgeReport, Recompute, RemoveReport, Selection, ValueTypeMigration, ValueTypeReport,
    WriteOpts,
};
use lakelands_api::{
    Catalog, Error, Find, Found, Get, LakeId, LakeStore, Patch, Position, Principal, ProposalId,
    ReadAt, Record, RecordKey, RelationName, Reply, Result, SourceId, Timestamp, TypeName,
    VerbName, Verification, Version,
};

use lakelands_api::error::{Case, Exchange, Remote, Seam, Situation};
use lakelands_core::{feed, rights};

use crate::batch::Asked;
use crate::exec::{self, Turn};
use crate::ingest;
use crate::lake::{Lake, owing};
use crate::reads::{Judged, Pulled, Reader};
use crate::writes::{self, Call, External, MadeDue, When};

/// When a read reads: its axes, and the clock a cursor carries from its
/// first page where it continues one.
type Clocked = (ReadAt, Option<Timestamp>);

/// A writer as a write sees it ([`Doors::seen`]).
type Seen = (Principal, Option<usize>, Option<writes::Shown>);

/// Open a lake at a location. The host passes the engines it compiled in
/// (and any seams it binds) in `seams`; the store depends on no engine.
pub async fn open(location: Location, seams: Seams, mode: OpenMode) -> Result<Arc<dyn LakeStore>> {
    let lake = opened(location, seams, mode).await?;
    Ok(Arc::new(Store { lake }))
}

/// The lake [`open`] opens.
pub(crate) async fn opened(location: Location, seams: Seams, mode: OpenMode) -> Result<Arc<Lake>> {
    let lake = exec::run(move || Lake::open(&location, seams, mode)).await?;
    let lake = Arc::new(lake);
    let owing = lake.clone();
    exec::write(&lake, move |t| {
        crate::backed::make_waiting(&owing, t);
        Ok(())
    })
    .await?;
    Ok(lake)
}

/// Build the projection this release lays out beside the lake at
/// `location`, ahead of the writer's open that upgrades it
/// ([`Lake::build_ahead`]).
pub async fn build_ahead(location: Location, seams: Seams) -> Result<crate::lake::Built> {
    exec::run(move || Lake::build_ahead(&location, seams)).await
}

/// How many bytes the projection of the lake at `location` takes, about
/// what a build ahead beside it takes, measured without opening it
/// ([`EngineOpener::room`](lakelands_api::engine::EngineOpener::room)).
/// Refused as unbound by an engine that keeps no scratch.
pub async fn room_ahead(location: Location, seams: Seams) -> Result<u64> {
    exec::run(move || crate::lake::opener(&location, &seams)?.room(&location)).await
}

/// Remove the scratch built ahead beside the lake at `location`
/// ([`Lake::discard_ahead`]).
pub async fn discard_ahead(location: Location, seams: Seams) -> Result<bool> {
    exec::run(move || Lake::discard_ahead(&location, seams)).await
}

/// The log format the newest event of the lake at `location` is written
/// in, read from the log's ends alone, nothing opened or folded; `None`
/// where there is no lake. A lake of a format this release does not read
/// is refused as an open refuses it. Below the format the release writes,
/// a writer's open upgrades it (`docs/log-format.md` §6).
pub async fn written_in(location: Location, seams: Seams) -> Result<Option<u32>> {
    exec::run(move || {
        let ends = crate::lake::opener(&location, &seams)?.log_ends(&location)?;
        let Some(ends) = ends else { return Ok(None) };
        crate::log::one_format(&location.lake, Some(&ends)).map(Some)
    })
    .await
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
    writes::granted(who.lake == lake.id)?;
    // The writer is a part of an authored event's id, whose parts NUL
    // separates.
    if who.id.contains('\0') {
        return Err(Error::invalid(Case::WriterNul));
    }
    Ok(())
}

/// The store facade of one lake.
pub struct Store {
    pub(crate) lake: Arc<Lake>,
}

/// The doors of one lake, bound to a host's write context.
#[derive(Clone)]
pub struct Doors {
    lake: Arc<Lake>,
    context: WriteContext,
}

/// One read of `lake` by `who` at `at`, under the clock a cursor carries
/// where it continues one, answered by `f`. One that ranks is given the
/// head (025): as it stands, or at exactly the position `ranks` names.
fn reading<T>(
    lake: &Lake,
    who: Principal,
    (at, clock): Clocked,
    ranks: Option<Option<Position>>,
    f: impl FnOnce(&Reader<'_>) -> Result<T>,
) -> Result<Reply<T>> {
    // The list comes from the view the read reads, so that a rename is in
    // its shape and in its grants, or in neither. A search at the head is
    // read at the position its rankings were read at.
    let head = ranks.flatten();
    let view = lake.engine.view(at.as_of.or(head))?;
    let shape = lake.shape(view.as_ref())?;
    let then = view.position().unwrap_or(Position::GENESIS);
    // A search as of a position judges its rankings at the head, and a
    // holder's its candidates too, its grants followed through the head's
    // shape.
    let to_head = at.as_of.is_some() && ranks.is_some();
    let at_head = to_head.then(|| crate::reads::Head::new(lake, &who, then, head));
    let at_head = at_head.transpose()?;
    // A record held now is not there at any position to a reader without
    // the grant, and one released since is (006, extended).
    let held_now = at.as_of.is_some() && !who.grants.restricted;
    let view = match held_now {
        true => Box::new(crate::reads::HeldNow::new(lake, view, at_head.as_ref())?),
        false => view,
    };
    let renames = at.as_of.is_none().then_some(&shape.renames[..]);
    // A reference to a type it is not shown is a field it may not know of,
    // by the types the read reads (024, extended).
    let (sight, _) = lake.sight(who, at.as_of, renames, shape.as_ref())?;
    let mut r = Reader::open(lake, &sight, (at, clock), view, shape.clone())?;
    (r.head, r.held_now) = (at_head, held_now);
    let data = f(&r)?;
    Ok(Reply {
        position: r.position,
        data,
    })
}

impl Doors {
    /// The doors of `lake` under `context`.
    pub(crate) fn new(lake: &Arc<Lake>, context: WriteContext) -> Doors {
        let lake = lake.clone();
        Doors { lake, context }
    }

    /// One record write decided and appended alone ([`crate::batch`]).
    pub(crate) async fn alone(
        &self,
        who: &Principal,
        write: writes::Write,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.write_settled(who, move |c| c.record(&write, opts))
            .await
    }

    /// What the bound reranker scores a window at, by key. A read: nothing
    /// is recorded, and a window with no candidate asks nothing. A failure,
    /// or other scores than candidates, is `Unavailable`; the stage is
    /// never skipped for the first stage's order (067). With the order the
    /// scores give, the read that answers it: at the window's position and
    /// on its clock, where the scored keys are the window's records.
    async fn rerank(
        &self,
        at: Clocked,
        window: Reply<crate::reads::Window>,
    ) -> Result<(crate::reads::Staged, Clocked)> {
        let Some(stage) = self.lake.rerank() else {
            return Err(Error::unbound(Seam::Rerank));
        };
        let (position, window) = (window.position, window.data);
        let pinned = (
            ReadAt {
                as_of: Some(position),
                ..at.0
            },
            Some(window.now),
        );
        let scored = |scores: &[f64]| crate::reads::Staged::scored(&window, scores, stage.model());
        if window.candidates.is_empty() {
            return Ok((scored(&[]), pinned));
        }
        let call = lakelands_api::seam::RerankCall {
            lake: self.lake.id.clone(),
            ty: window.ty.clone(),
            query: window.query.clone(),
            candidates: window.candidates.clone(),
        };
        let failed = |exchange| {
            let remote = Remote::Reranker {
                model: stage.model().to_owned(),
            };
            Error::unavailable(Situation::Remote { remote, exchange })
        };
        let scores = (stage.rerank(&call).await).map_err(|e| match e {
            e @ Error::Unavailable { .. } => e,
            other => failed(Exchange::Failed {
                cause: Box::new(other),
            }),
        })?;
        if scores.len() != call.candidates.len() {
            let (answered, asked) = (scores.len() as u64, call.candidates.len() as u64);
            return Err(failed(Exchange::Scores { answered, asked }));
        }
        if scores.iter().any(|s| !s.is_finite()) {
            return Err(failed(Exchange::ScoreNotFinite));
        }
        Ok((scored(&scores), pinned))
    }

    async fn read<T, F>(&self, who: &Principal, at: Clocked, f: F) -> Result<Reply<T>>
    where
        T: Send + 'static,
        F: FnOnce(&Reader<'_>) -> Result<T> + Send + 'static,
    {
        same_lake(&self.lake, who)?;
        let (lake, who) = (self.lake.clone(), who.clone());
        exec::run(move || reading(&lake, who, at, None, f)).await
    }

    /// [`Self::read`] for a read that may rank, given the head when
    /// `ranks`. `f` answers it, or hands back a pull of its rankings that
    /// was read at another position than that head: a write landed between
    /// the two. The read is then made again with the head at exactly that
    /// position and the pull to judge there, so that a search's candidates
    /// and its judgement are of one state of the head (025). Nothing is
    /// asked twice unless a declaration of the searched type changed in
    /// between, and no read is refused for the writes beside it.
    async fn read_to<T, F>(
        &self,
        who: &Principal,
        at: Clocked,
        ranks: bool,
        f: F,
    ) -> Result<Reply<T>>
    where
        T: Send + 'static,
        F: Fn(&Reader<'_>, Option<Pulled>) -> Result<Judged<T>> + Send + 'static,
    {
        same_lake(&self.lake, who)?;
        let (lake, who) = (self.lake.clone(), who.clone());
        exec::run(move || {
            let mut pulled: Option<Pulled> = None;
            loop {
                let head = pulled.as_ref().and_then(Pulled::position);
                let judge = |r: &Reader<'_>| f(r, pulled.take());
                let read = reading(&lake, who.clone(), at, ranks.then_some(head), judge)?;
                match read.data {
                    Ok(data) => {
                        let position = read.position;
                        return Ok(Reply { position, data });
                    }
                    Err(moved) => pulled = Some(moved),
                }
            }
        })
        .await
    }

    async fn write<T, F>(&self, who: &Principal, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Call<'_>) -> Result<T> + Send + 'static,
    {
        let (who, followed, shown) = self.seen(who).await?;
        let context = self.context;
        exec::write(&self.lake, move |lake| {
            f(&Call {
                lake,
                who: &who,
                context,
                followed,
                shown,
            })
        })
        .await
    }

    /// `who` as a write sees it: its grants followed, how far, and what it
    /// is shown.
    async fn seen(&self, who: &Principal) -> Result<Seen> {
        same_lake(&self.lake, who)?;
        let (lake, who) = (self.lake.clone(), who.clone());
        // A reference to a type it is not shown is a field it may not know
        // of, by the head's types; the writer judges it again under its
        // lock (024, extended). The head is read only for one whose grants
        // hide something once they are followed: following may hide what
        // the grants as given do not. A read, made before the write waits
        // its turn, as the writer's own judgement is not.
        exec::run(move || {
            let none = Vec::<lakelands_api::engine::Declared>::new();
            let (mut sight, mut followed) = lake.sight(who.clone(), None, None, &none)?;
            let head = || lake.engine.view(None).and_then(|v| lake.shape(v.as_ref()));
            let head = rights::hides_some(sight.who()).then(head).transpose()?;
            if let Some(head) = &head {
                let renames = Some(&head.renames[..]);
                (sight, followed) = lake.sight(who.clone(), None, renames, head.as_ref())?;
            }
            let shown = head.as_ref().map(|head| writes::Shown {
                asked: who,
                versions: head.ontology.versions(),
                hidden: sight.hidden().to_vec(),
            });
            Ok((sight.who().clone(), followed, shown))
        })
        .await
    }

    /// Record writes, queued as one entry ([`writes::write_many`]), each
    /// answered as [`Self::write_settled`] answers.
    pub(crate) async fn records(&self, writes: Vec<Asked>) -> Vec<Result<Outcome>> {
        let (mut jobs, mut asked) = (Vec::new(), Vec::new());
        for (context, who, write, opts) in writes {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let seen = self.seen(&who).await.map(|(judged, followed, shown)| {
                let answer = tx;
                let job = writes::Job {
                    who: judged,
                    context,
                    followed,
                    shown,
                    write,
                    opts,
                    answer,
                };
                jobs.push(job);
            });
            asked.push((who, seen.map(|()| rx)));
        }
        if !jobs.is_empty() {
            exec::records(&self.lake, jobs);
        }
        let mut answers = Vec::new();
        for (who, rx) in asked {
            let answer = match rx {
                Ok(rx) => rx.await.unwrap_or_else(|_| Err(exec::halted(true))),
                Err(e) => Err(e),
            };
            let by = MadeDue::Call { keyed: false };
            answers.push(match answer {
                Ok((outcome, own)) => self.handed(&who, ((outcome, None), own), by).await,
                Err(e) => Err(e),
            });
        }
        answers
    }

    /// One record write, as [`Self::records`] answers it.
    async fn record(
        &self,
        who: &Principal,
        write: writes::Write,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let mut answers = (self.records(vec![(self.context, who.clone(), write, opts)])).await;
        answers.remove(0)
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
        ((mut outcome, external), own): ((Outcome, Option<External>), BTreeSet<Position>),
        by: MadeDue,
    ) -> Result<Outcome> {
        let last = crate::backed::settle(&self.lake, &own).await;
        let last = match &external {
            Some(_) => last.map_err(|e| writes::not_handed(e, true, by))?,
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
            Some(ext) => Ok(writes::with_output(outcome, self.execute(ext, by).await?)),
            None => Ok(outcome),
        }
    }

    /// A write that can make an external verb due, with its drain, its
    /// hand-out and the recording of the answer, run in a task of its own
    /// that the caller awaits: a caller who goes away, as a request whose
    /// client closed its connection does, does not stop it between the
    /// call's append and its answer's recording.
    async fn detached<F>(&self, who: &Principal, by: MadeDue, f: F) -> Result<Outcome>
    where
        F: FnOnce(&Call<'_>) -> Result<(Outcome, Option<External>)> + Send + 'static,
    {
        let (doors, who) = (self.clone(), who.clone());
        let task = tokio::spawn(async move {
            let made = doors.write(&who, |c| owing(|| f(c))).await?;
            doors.handed(&who, made, by).await
        });
        task.await.unwrap_or_else(|e| Err(exec::stopped(&e)))
    }

    /// A write whose outcome is [`Doors::handed`]'s.
    async fn write_settled<F>(&self, who: &Principal, f: F) -> Result<Outcome>
    where
        F: FnOnce(&Call<'_>) -> Result<Outcome> + Send + 'static,
    {
        let (outcome, own) = self.write(who, |c| owing(|| f(c))).await?;
        let by = MadeDue::Call { keyed: false };
        self.handed(who, ((outcome, None), own), by).await
    }

    /// Hand an external verb to its executor and record the answer; the
    /// recorded output is returned, the error's wire form when the executor failed.
    /// A failure before the hand-out is [`writes::not_handed`]'s.
    async fn execute(&self, ext: External, by: MadeDue) -> Result<(Position, serde_json::Value)> {
        let not_handed = |e| writes::not_handed(e, true, by);
        let executor = writes::bound_executor(&self.lake, &ext.executor).map_err(not_handed)?;
        let executor = executor.clone();
        // The executor is the host's own code: it receives the record whole,
        // unmasked by the grants of whoever made the verb due, as it stood
        // at the position that made it due, where its call was judged: a
        // later write, a retirement included, does not reach it (013). The
        // caller sees only the outcome and the recorded output. A verb on
        // its type has no record to hand it.
        let record = match &ext.key {
            Some(key) => {
                let get = Get {
                    key: key.clone(),
                    with_meta: false,
                    at: ReadAt {
                        as_of: Some(ext.position),
                        valid_at: None,
                        clock: None,
                    },
                    params: Default::default(),
                };
                let host = self.lake.system("host");
                Some(
                    self.get(&host, &ext.ty, get)
                        .await
                        .map_err(not_handed)?
                        .data,
                )
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
            // The error as the wire carries it: a refusal keeps its case. One
            // whose wire form cannot be written, or holds an integer beyond
            // ±(2^53 − 1) the log does not write, is recorded as its sentence,
            // the one-member form, which loses nothing: an error is always
            // recorded, and a retry answers it.
            Err(e) => serde_json::to_value(&e)
                .ok()
                .filter(|wire| lakelands_core::canonical::integer_too_large(wire).is_none())
                .unwrap_or_else(|| serde_json::json!({ "error": e.to_string() })),
        };
        // An answer is recorded as the executor gave it, and the log writes
        // no integer beyond ±(2^53 − 1) (the log format §1.2): one that
        // holds such a number is not recorded. The executor ran, so its
        // answer is lost but for this sentence, and a retry is unanswered.
        if let Some(n) = lakelands_core::canonical::integer_too_large(&output) {
            let cause = Box::new(Error::invalid(Case::IntegerTooLarge { n: n.clone() }));
            let answered = output.clone();
            return Err(Error::unavailable(Situation::AnswerLost {
                answered,
                cause,
            }));
        }
        let recorded = output.clone();
        let record = move |t: &Turn<'_>| Ok(writes::record_output(t, &ext, &input, output));
        let at = exec::write(&self.lake, record).await?;
        // The executor ran: where its answer reached the log or is held, a
        // retry answers it; where neither, it is lost but for this sentence.
        let at = at.map_err(|(e, held)| match e {
            e @ Error::Unavailable { .. } => {
                let (answered, cause) = (recorded.clone(), Box::new(e));
                Error::unavailable(match held {
                    true => Situation::AnswerHeld {
                        answered,
                        retry: by.retry().resent(),
                        cause,
                    },
                    false => Situation::AnswerLost { answered, cause },
                })
            }
            e => e,
        })?;
        Ok((at, recorded))
    }
}

#[async_trait]
impl Catalog for Doors {
    fn lake(&self) -> &LakeId {
        &self.lake.id
    }

    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        let asked = who.clone();
        self.read(who, (at, None), move |r| r.describe(&asked))
            .await
    }

    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        let (ty, asked) = (ty.clone(), who.clone());
        self.read(who, (at, None), move |r| r.describe_type(&asked, &ty))
            .await
    }

    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        let at = cursor_at(find.at, find.cursor.as_ref())?;
        let ty = ty.clone();
        let found = self.read(who, at, move |r| r.find(&ty, &find)).await;
        // A page sorted whole may have made the lake choose an index; it
        // builds it beside the writer, in chunks (041).
        if self.lake.choosing_due() {
            let lake = self.lake.clone();
            tokio::spawn(async move { crate::choose::choose_orders(&lake).await });
        }
        found
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        let ty = ty.clone();
        self.read(who, (get.at, None), move |r| r.get(&ty, &get))
            .await
    }

    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let write = writes::Write::Upsert(ty.clone(), key.clone(), patch);
        self.record(who, write, opts).await
    }

    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        let write = writes::Write::Retire(ty.clone(), key.clone());
        self.record(who, write, opts).await
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
        self.record(
            who,
            writes::Write::Link(ty, key, relation, to, fields),
            opts,
        )
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
        self.record(who, writes::Write::Unlink(ty, key, relation, to), opts)
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
        let by = MadeDue::Call {
            keyed: opts.idempotency_key.is_some(),
        };
        let call = move |c: &Call<'_>| c.call(&verb, key.as_ref(), payload, opts);
        self.detached(who, by, call).await
    }

    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        let at = cursor_at(pattern.at, pattern.cursor.as_ref())?;
        // A start by search ranks as `search` does (025).
        // A start with a second stage ranks nothing once its window is
        // read: its keys are read where the window was, and judged there
        // (067), not at the head.
        let ranks = pattern.start.search.is_some() && pattern.start.rerank.is_none();
        // A start with a second stage ranks its window through the bound
        // reranker on a first page, as `search` does (067).
        let (mut at, mut scores) = (at, None);
        if pattern.start.rerank.is_some() && pattern.cursor.is_none() {
            let p = pattern.clone();
            let window = self.read_to(who, at, true, move |r, pulled| r.query_window(&p, pulled));
            let (scored, pinned) = self.rerank(at, window.await?).await?;
            (scores, at) = (Some(scored), pinned);
        }
        self.read_to(who, at, ranks, move |r, pulled| {
            if let Some(staged) = &scores {
                r.stage(staged.clone());
            }
            r.query(&pattern, pulled)
        })
        .await
    }

    async fn search(&self, who: &Principal, mut search: Search) -> Result<Reply<Hits>> {
        let at = cursor_at(search.at, search.cursor.as_ref())?;
        // A hybrid search without a caller's vector embeds its text through
        // the bound embedding backend, if the host bound one. This is a
        // read: nothing is recorded. It is judged for its caller first, by
        // the read's shape, as the read below judges it again: one refused
        // calls nothing and names no model. A later page of a second stage
        // answers the keys its cursor carries and embeds nothing (067).
        let staged = search.rerank.is_some() && search.cursor.is_some();
        let unvectored = matches!(search.mode, SearchMode::Hybrid { vector: None, .. });
        let (embed, mut judged) = (self.lake.embed().filter(|_| unvectored && !staged), None);
        let sent = search.rerank.is_some().then(|| search.mode.clone());
        if embed.is_some() {
            let s = search.clone();
            // The engines index the head's vectors only, so a search at any
            // position embeds by the head's declaration, as `searched`
            // judged it there (025).
            let head = move |r: &Reader<'_>, _| {
                let (.., mut ranks) = r.searched(&s)?;
                let field = s.mode.vectors().first().map(|v| v.0.clone());
                Ok(Ok(field.and_then(|f| ranks.fields.remove(&f))))
            };
            judged = Some(self.read_to(who, at, true, head).await?.data);
        }
        if let (
            SearchMode::Hybrid {
                text,
                field,
                vector: vector @ None,
                ..
            },
            Some(embed),
            Some(declared),
        ) = (&mut search.mode, embed, judged)
        {
            // A field declared as the embedding of a model is searched with
            // that model's vectors only: another bound model, or one making
            // vectors of other dimensions, is a configuration to fix, not
            // weather.
            let declared = declared.and_then(|f| {
                let e = f.embed.as_ref()?;
                Some((e.model.clone(), f.ty.dimensions.unwrap_or(0)))
            });
            if let Some((model, dimensions)) = declared
                && (model != embed.model() || dimensions != embed.dimensions())
            {
                let bound = Some(lakelands_api::error::Bound {
                    model: embed.model().to_owned(),
                    dimensions: embed.dimensions(),
                });
                let (ty, field) = (search.ty.clone(), field.to_string());
                return Err(Error::unbound(Seam::Embedding {
                    model,
                    dimensions,
                    ty,
                    field,
                    bound,
                }));
            }
            let mut vectors =
                embed
                    .embed(std::slice::from_ref(text))
                    .await
                    .map_err(|e| match e {
                        e @ Error::Unavailable { .. } => e,
                        other => {
                            let cause = Box::new(other);
                            backend_failed(embed.model(), Exchange::Failed { cause })
                        }
                    })?;
            // A vector the backend made with no direction is its fault, not
            // the caller's: weather, as a wrong count of vectors is.
            let exchange = match &vectors[..] {
                [one] => {
                    lakelands_api::query::no_direction(one).map(|why| Exchange::NoDirection { why })
                }
                _ => {
                    let (answered, asked) = (vectors.len() as u64, 1);
                    Some(Exchange::Vectors { answered, asked })
                }
            };
            if let Some(exchange) = exchange {
                return Err(backend_failed(embed.model(), exchange));
            }
            *vector = vectors.pop();
        }
        // The second stage ranks a window of the first on a first page, and
        // a cursor carries its order after (067).
        let (mut at, mut scores) = (at, None);
        if search.rerank.is_some() && search.cursor.is_none() {
            let s = search.clone();
            let window = self.read_to(who, at, true, move |r, pulled| r.rerank_window(&s, pulled));
            let (scored, pinned) = self.rerank(at, window.await?).await?;
            (scores, at) = (Some(scored), pinned);
        }
        // The read that answers a second stage's keys ranks nothing: they
        // are read and judged at the window's or the cursor's position, not
        // at the head (067). Its cursor digests the mode as its caller sent
        // it, which a later page, embedding nothing, sends again.
        if let Some(sent) = sent {
            search.mode = sent;
        }
        let ranks = search.rerank.is_none();
        self.read_to(who, at, ranks, move |r, pulled| {
            if let Some(staged) = &scores {
                r.stage(staged.clone());
            }
            r.search(&search, pulled)
        })
        .await
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        self.write(who, move |c| c.propose(write, why)).await
    }

    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        let (p, asked) = (proposal.clone(), who.clone());
        let accept = move |c: &Call<'_>| c.accept(&p, &asked, None);
        self.detached(who, MadeDue::Accepted, accept).await
    }

    async fn accept_planned(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        plan_id: &str,
    ) -> Result<Outcome> {
        let (p, asked, id) = (proposal.clone(), who.clone(), plan_id.to_owned());
        let accept = move |c: &Call<'_>| c.accept(&p, &asked, Some(&id));
        self.detached(who, MadeDue::Accepted, accept).await
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        // A rejected call is recorded as its output, after the append (064).
        let (p, asked) = (proposal.clone(), who.clone());
        self.write_settled(who, move |c| c.reject(&p, &asked, reason))
            .await
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        // A proposed migration is judged with the grants as they stood
        // where it was proposed: the listing follows them there.
        let asked = who.clone();
        self.read(who, (filter.at, None), move |r| {
            r.proposals(&filter, &asked)
        })
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
        m: ValueTypeMigration,
    ) -> Result<Reply<ValueTypeReport>> {
        self.write(who, move |c| {
            crate::writes::granted(c.who.grants.schema && !rights::holds_only(c.who))?;
            let w = c.writer()?;
            let view = c.lake.engine.view(None)?;
            let report = c
                .preflight_value_type(&w, view.as_ref(), &m, (None, c.judging(When::Plan)))?
                .report;
            c.renames_shown(&w.ontology, &m, &report)?;
            Ok(Reply {
                position: view.position().unwrap_or(Position::GENESIS),
                data: c.shown_value_type(&w.ontology, report),
            })
        })
        .await
    }

    async fn plan(&self, who: &Principal, change: Change) -> Result<Reply<Plan>> {
        self.write(who, move |c| c.plan(change)).await
    }

    async fn enact(&self, who: &Principal, change: Change, plan_id: &str) -> Result<Outcome> {
        let id = plan_id.to_owned();
        self.write_settled(who, move |c| c.enact(change, &id)).await
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        let id = report_id.to_owned();
        self.write_settled(who, move |c| c.migrate_value_type_apply(m, &id))
            .await
    }

    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        self.ingest_with(who, source, IngestOpts::default(), records)
            .await
    }

    async fn ingest_with(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        records: SourceStream,
    ) -> Result<CommitStream> {
        same_lake(&self.lake, who)?;
        // The context first, as on every write door, and before the stream
        // is taken.
        writes::check_context(&self.lake, &self.context)?;
        writes::granted(who.grants.ingest)?;
        writes::check_source(source)?;
        if let Some(d) = &opts.delivery {
            writes::check_delivery(d)?;
        }
        Ok(ingest::run(
            self.lake.clone(),
            who.clone(),
            self.context,
            source.clone(),
            (opts.delivery, opts.sealed_apart),
            records,
        ))
    }
}

/// `at`, moved to the position a cursor names when there is one, with the
/// clock it carries.
fn cursor_at(mut at: ReadAt, cursor: Option<&lakelands_api::Cursor>) -> Result<Clocked> {
    let Some(c) = cursor else {
        return Ok((at, None));
    };
    #[derive(serde::Deserialize)]
    struct At {
        at: Position,
        #[serde(default)]
        now: Option<Timestamp>,
    }
    let bytes: Option<Vec<u8>> = (0..c.0.len())
        .step_by(2)
        .map(|i| {
            c.0.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect();
    let a = bytes.and_then(|b| serde_json::from_slice::<At>(&b).ok());
    let a = a.ok_or_else(|| Error::invalid(lakelands_api::error::Case::CursorForeign))?;
    at.as_of = Some(a.at);
    Ok((at, a.now))
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
        Arc::new(Doors::new(&self.lake, context))
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
        exec::write(&self.lake, |t| t.rebuild()).await
    }

    fn reclaim_owed(&self) -> Option<String> {
        self.lake.reclaim_owed()
    }

    fn rebuilt_at_open(&self) -> Option<String> {
        self.lake.rebuilt_at_open()
    }

    async fn restrict(&self, ty: &TypeName, key: &RecordKey, reason: String) -> Result<Position> {
        let (ty, key) = (ty.clone(), key.clone());
        exec::write(&self.lake, move |t| {
            writes::restrict(t, &ty, &key, Some(reason))
        })
        .await
    }

    async fn unrestrict(&self, ty: &TypeName, key: &RecordKey) -> Result<Position> {
        let (ty, key) = (ty.clone(), key.clone());
        exec::write(&self.lake, move |t| writes::restrict(t, &ty, &key, None)).await
    }

    async fn age(&self, today: Timestamp) -> Result<AgeReport> {
        let age = move |t: &Turn<'_>| owing(|| crate::aging::run(t, today));
        let (mut report, own) = exec::write(&self.lake, age).await?;
        // The moves' backed fields settle like any write's; the report
        // names the position from which what they filled is visible.
        if let Some(last) = crate::backed::settle_after(&self.lake, &own).await {
            report.position = report.position.max(last);
        }
        Ok(report)
    }

    async fn purge_preflight(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
    ) -> Result<PurgeReport> {
        writes::check_source(source)?;
        // It decides the retirements through the purge's own decision,
        // which is made at a turn: it takes no writer, but writes wait for
        // it as for the purge it foretells.
        let (source, ty) = (source.clone(), ty.cloned());
        let preflight = move |t: &Turn<'_>| crate::purge::preflight(t, &source, ty.as_ref());
        exec::write(&self.lake, preflight).await
    }

    async fn purge_source(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
        id: Option<&str>,
    ) -> Result<PurgeReport> {
        writes::check_source(source)?;
        let (source, ty, id) = (source.clone(), ty.cloned(), id.map(str::to_owned));
        let purge =
            move |t: &Turn<'_>| owing(|| crate::purge::run(t, &source, ty.as_ref(), id.as_deref()));
        let (mut report, own) = exec::write(&self.lake, purge).await?;
        // The retirements' backed fields settle like any write's.
        if let Some(last) = crate::backed::settle_after(&self.lake, &own).await {
            report.position = report.position.max(last);
        }
        Ok(report)
    }

    async fn upsert_preflight(
        &self,
        who: &Principal,
        context: WriteContext,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        expected: Option<Version>,
    ) -> Result<()> {
        let doors = Doors {
            lake: self.lake.clone(),
            context,
        };
        let (ty, key) = (ty.clone(), key.clone());
        let judge = move |c: &Call<'_>| c.upsert_preflight(&ty, &key, &patch, expected);
        doors.write(who, judge).await
    }

    async fn forget_source(&self, source: &SourceId, id: Option<&str>) -> Result<ForgetReport> {
        writes::check_source(source)?;
        let (source, id) = (source.clone(), id.map(str::to_owned));
        exec::write(&self.lake, move |t| {
            crate::forget::run(t, &source, id.as_deref())
        })
        .await
    }

    async fn forget_preflight(&self, source: &SourceId) -> Result<ForgetReport> {
        writes::check_source(source)?;
        let (lake, source) = (self.lake.clone(), source.clone());
        exec::run(move || crate::forget::preflight(&lake, &source)).await
    }

    async fn forget_sources(
        &self,
        sources: &[SourceId],
        id: Option<&str>,
    ) -> Result<ForgetSetReport> {
        sources.iter().try_for_each(writes::check_source)?;
        let (sources, id) = (sources.to_vec(), id.map(str::to_owned));
        exec::write(&self.lake, move |t| {
            crate::forget::run_set(t, &sources, id.as_deref())
        })
        .await
    }

    async fn forget_sources_preflight(&self, sources: &[SourceId]) -> Result<ForgetSetReport> {
        sources.iter().try_for_each(writes::check_source)?;
        let (lake, sources) = (self.lake.clone(), sources.to_vec());
        exec::run(move || crate::forget::preflight_set(&lake, &sources)).await
    }

    async fn remove_values(
        &self,
        source: &SourceId,
        selection: &Selection,
        shredded: bool,
        id: Option<&str>,
    ) -> Result<RemoveReport> {
        writes::check_source(source)?;
        let (source, id) = (source.clone(), id.map(str::to_owned));
        let selection = selection.clone();
        let remove = move |t: &Turn<'_>| {
            crate::remove::run(t, &source, &selection, (shredded, id.as_deref()))
        };
        exec::write(&self.lake, remove).await
    }

    async fn remove_values_preflight(
        &self,
        source: &SourceId,
        selection: &Selection,
    ) -> Result<RemoveReport> {
        writes::check_source(source)?;
        let (lake, source, selection) = (self.lake.clone(), source.clone(), selection.clone());
        exec::run(move || crate::remove::preflight(&lake, &source, &selection)).await
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
        exec::write(&self.lake, |t| t.compact()).await
    }

    fn bind_embed(&self, backend: Arc<dyn lakelands_api::seam::EmbedBackend>) -> Result<()> {
        self.lake.bind_embed(backend);
        // The sweep takes the writer: in the lake's queue, so the bind
        // answers at once.
        let lake = self.lake.clone();
        exec::write_soon(&self.lake, move |t| crate::backed::make_waiting(&lake, t));
        Ok(())
    }

    fn bind_rerank(&self, backend: Arc<dyn lakelands_api::seam::Reranker>) -> Result<()> {
        self.lake.bind_rerank(backend);
        Ok(())
    }

    async fn checkpoint(&self) -> Result<Position> {
        exec::write(&self.lake, |t| t.checkpoint()).await
    }

    async fn close(&self) -> Result<()> {
        exec::write(&self.lake, |t| t.close()).await
    }
}

/// The change feed: the log, filtered by kind and type, read from a
/// position (`docs/decisions/004-the-change-feed-is-the-log.md`), each
/// batch naming the reactions in it an act overruled (065).
pub struct Feed {
    lake: Arc<Lake>,
}

#[async_trait]
impl ChangeFeed for Feed {
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch> {
        let (lake, filter) = (self.lake.clone(), filter.clone());
        exec::run(move || {
            let (mut events, mut next, head) = polled(&lake, from, &filter, limit)?;
            // A reaction the projection cannot decide yet, while a rebuild
            // runs, is left to the next poll with what follows it.
            let (overruled, cut) = crate::writes::overruled(&lake, &events)?;
            if let Some(cut) = cut {
                events.retain(|e| e.position < cut);
                next = cut;
            }
            Ok(FeedBatch {
                events,
                next,
                head,
                overruled,
            })
        })
        .await
    }

    async fn ack(&self, _consumer: &str, _through: Position) -> Result<()> {
        // The store keeps no consumer state: the consumer keeps its own
        // position.
        Ok(())
    }

    async fn wait(&self, after: Position, within: std::time::Duration) -> Result<Position> {
        Ok(self.lake.wait_past(after, within).await)
    }
}

/// The events of one poll from `from` that `filter` admits, the position
/// to poll from next and the head.
fn polled(
    lake: &Lake,
    from: Position,
    filter: &FeedFilter,
    limit: u32,
) -> Result<(Vec<lakelands_api::event::Event>, Position, Position)> {
    let head = lake.head();
    let want = limit.max(1) as usize;
    let (mut read, mut events, mut next) = (0, Vec::new(), from);
    // Up to the view's position the projection's index names the
    // events the filter admits, and only those are read and opened,
    // in one ask: where the index stops short of the view, so does
    // the poll. One that decides nothing, as while the projection
    // is rebuilt, leaves the poll to the log. A filter that names
    // nothing admits every event.
    let view = feed::seek(filter)
        .map(|_| lake.engine.view(None))
        .transpose()?;
    let indexed = view
        .as_ref()
        .and_then(|v| v.position())
        .map(|p| p.min(head));
    let mut decided = false;
    if let (Some(view), Some(p)) = (&view, indexed.filter(|p| next <= *p)) {
        let room = want.min(FEED_EXAMINED as usize) as u32;
        let m = view.matching(filter, next, p, room)?;
        read = m.positions.len();
        for e in lake.read_these(&m.positions)? {
            if feed::admits(filter, e.body.kind(), &feed::about(&e.body)) {
                events.push(e);
            }
        }
        if let Some(through) = m.through {
            (decided, next) = (true, through.next());
        }
    }
    drop(view);
    if decided && indexed.is_some_and(|p| next <= p) {
        return Ok((events, next, head));
    }
    // Past it, as while the projection is rebuilt, every event is
    // read, to `limit` matches or to `last`, the head or the bound:
    // nothing past the last position examined is read or verified.
    let bound = (FEED_EXAMINED as usize - read) as u64;
    let last = Position(next.0.saturating_add(bound).saturating_sub(1)).min(head);
    'read: while bound > 0 && next <= last && events.len() < want {
        let room = (last.0 - next.0 + 1).min(want.clamp(64, 1024) as u64);
        let batch = lake.read_events(next, room as u32)?;
        if batch.is_empty() {
            break;
        }
        for e in batch {
            if e.position > last {
                break 'read;
            }
            next = e.position.next();
            if feed::admits(filter, e.body.kind(), &feed::about(&e.body)) {
                events.push(e);
                if events.len() >= want {
                    break 'read;
                }
            }
        }
    }
    Ok((events, next, head))
}

#[cfg(test)]
mod tests;

/// An exchange with the embedding backend of `model` that failed.
fn backend_failed(model: &str, exchange: Exchange) -> Error {
    let remote = Remote::Embedding {
        model: model.to_owned(),
    };
    Error::unavailable(Situation::Remote { remote, exchange })
}
