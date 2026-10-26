//! The projection with its kept history.
//!
//! Every state the fold puts is kept, valid from a position, beside the
//! states before it; a [`History`] answers the state valid at any
//! position as the latest one put at or before it. Nothing is replaced
//! but a state put again at the same position, which is the same state
//! folded twice.
//!
//! The fold writes inside a transaction: the puts between `begin` and
//! `commit` are held as [`Put`]s and applied together when it commits,
//! with the position the projection then reflects. A rollback drops them,
//! so a view never sees half an append.
//!
//! Every put carries its own position (a record's `position`, the `at`
//! of everything else), and its state is valid from there, so a view at
//! a position inside a multi-event append sees each state from the event
//! that made it.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use lakelands_api::engine::{About, Applied, Choose, Chosen, Declared};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{
    FieldName, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName,
};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::Edge;
use lakelands_api::read::Direction;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines::{self, Elements};

/// The states of one thing, each valid from a position, oldest first.
#[derive(Clone, Debug)]
pub(crate) struct History<T>(Vec<(Position, T)>);

impl<T> Default for History<T> {
    fn default() -> Self {
        History(Vec::new())
    }
}

impl<T> History<T> {
    /// Adds a state valid from `from`, keeping every state before it. A
    /// state put again at the same position replaces the one there.
    pub(crate) fn put(&mut self, from: Position, state: T) {
        let i = self.0.partition_point(|(p, _)| *p < from);
        match self.0.get_mut(i) {
            Some((p, old)) if *p == from => *old = state,
            _ => self.0.insert(i, (from, state)),
        }
    }

    /// The state valid at `at`: the latest put at or before it. Nothing is
    /// valid in a view of an empty projection.
    pub(crate) fn at(&self, at: Option<Position>) -> Option<&T> {
        self.since(at).map(|(_, t)| t)
    }

    /// [`Self::at`] with the position the state is valid from.
    pub(crate) fn since(&self, at: Option<Position>) -> Option<(Position, &T)> {
        let at = at?;
        let i = self.0.partition_point(|(p, _)| *p <= at);
        i.checked_sub(1).map(|i| (self.0[i].0, &self.0[i].1))
    }

    /// The latest state put.
    pub(crate) fn latest(&self) -> Option<&T> {
        self.0.last().map(|(_, t)| t)
    }
}

/// A record as the projection keeps it: in the state form the other
/// engines keep, which shares one provenance block among the fields that
/// have it, inline or, where the write shares it, kept once among the
/// [`Bases`], with every field that is a
/// vector of numbers apart as its elements, not as a value each.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    ty: TypeName,
    key: RecordKey,
    since: Position,
    state: Box<str>,
    apart: Vec<(FieldName, Elements)>,
    /// The id of the base its state names.
    base: Option<i64>,
}

impl Held {
    fn of(r: &StoredRecord, bases: &mut Bases) -> Held {
        let apart: Vec<_> = (r.fields.iter())
            .filter_map(|(f, v)| Some((f.clone(), Elements::of(v)?)))
            .collect();
        let named = apart.iter().map(|(f, _)| (f.clone(), r.position)).collect();
        let mut base = None;
        let state = engines::record_state(r, &named, &mut |b| {
            let shared = bases.2.contains(b);
            Ok(shared.then(|| *base.insert(bases.id(b))))
        });
        let state = state.expect("a base is kept").into();
        let (ty, key, since) = (r.ty.clone(), r.key.clone(), r.position);
        Held {
            ty,
            key,
            since,
            state,
            apart,
            base,
        }
    }

    pub(crate) fn record(&self, bases: &Bases) -> StoredRecord {
        let state =
            engines::decode_record(self.ty.as_str(), self.key.as_str(), self.since, &self.state);
        let (mut r, apart) = state.expect("a state this projection wrote reads");
        if let Some(based) = apart.base {
            let block = bases.0.get(&based.id).map(String::as_str);
            engines::rebase(&mut r, based, block).expect("a base this projection kept reads");
        }
        for (field, e) in &self.apart {
            r.fields.insert(field.clone(), e.value());
        }
        r
    }
}

/// The bases the records' states name, each kept once by its id, and
/// those the transaction being applied shares ([`engines::shared_bases`]).
#[derive(Debug, Default)]
pub(crate) struct Bases(
    BTreeMap<i64, String>,
    BTreeMap<String, i64>,
    HashSet<String>,
);

impl Bases {
    /// The id `b` is kept under, kept now if it was not.
    fn id(&mut self, b: &str) -> i64 {
        if let Some(id) = self.1.get(b) {
            return *id;
        }
        let id = self.0.last_key_value().map_or(1, |(id, _)| id + 1);
        self.0.insert(id, b.to_owned());
        self.1.insert(b.to_owned(), id);
        id
    }
}

/// One write of the fold, held until its transaction commits.
#[derive(Clone, Debug)]
pub(crate) enum Put {
    /// A declaration in force from its `since`.
    Declaration(Declared),
    /// A record's whole state from its `position`.
    Record(StoredRecord),
    /// The one state of a key of a head-only name, or its end: it takes
    /// the place of whatever the key held, and nothing indexes it.
    Head(StoredRecord),
    /// Every key of a head-only name ended at once, at `at`.
    HeadsEnded { ty: TypeName, at: Position },
    /// The bases no record's state names.
    DroppedBases,
    /// A proposal's state from `at`.
    Proposal { proposal: Proposal, at: Position },
    /// Membership of a record in an object set from `at`.
    Membership {
        set: String,
        ty: TypeName,
        key: RecordKey,
        member: bool,
        at: Position,
    },
    /// A seam's recorded output, from `at`.
    Output {
        seam: String,
        input: Digest,
        output: Value,
        at: Position,
    },
    /// An event applied, with what the feed's index keeps of it.
    Applied(Applied),
    /// Every state of a record, its memberships and its rows, deleted.
    DroppedRecord(TypeName, RecordKey),
    /// Every state of a seam's output for one input, deleted.
    DroppedOutput(String, Digest),
}

impl Put {
    /// The position the put's state is valid from.
    fn own_position(&self) -> Position {
        match self {
            Put::Declaration(d) => d.since,
            Put::Record(r) | Put::Head(r) => r.position,
            Put::Applied(a) => a.at,
            Put::DroppedRecord(..) | Put::DroppedOutput(..) | Put::DroppedBases => {
                Position::GENESIS
            }
            Put::Membership { at, .. }
            | Put::HeadsEnded { at, .. }
            | Put::Proposal { at, .. }
            | Put::Output { at, .. } => *at,
        }
    }
}

/// One lake's projection: every kept state, and how far it reflects the
/// log.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) declarations: BTreeMap<String, History<Declared>>,
    pub(crate) records: BTreeMap<(TypeName, RecordKey), History<Held>>,
    pub(crate) bases: Bases,
    pub(crate) proposals: BTreeMap<ProposalId, History<Proposal>>,
    /// Each proposal's id by its position, newest last (lakelands#1063).
    pub(crate) by_position: BTreeMap<Position, ProposalId>,
    pub(crate) members: BTreeMap<(String, TypeName, RecordKey), History<bool>>,
    pub(crate) outputs: BTreeMap<(String, Digest), History<Value>>,
    pub(crate) applied: BTreeMap<EventId, Position>,
    /// The feed's index of the log: every applied position's kind and
    /// what its event is about, and the positions of each kind, type, key
    /// and source.
    pub(crate) routes: Routes,
    pub(crate) folded: Option<Position>,
    /// Per type, every record state put, by position and key: the keys a
    /// page at an earlier position reads apart from the order index.
    changes: BTreeMap<TypeName, BTreeSet<(Position, RecordKey)>>,
    /// The order index of the head (`engines::order_rows`): per type and
    /// declared field, the head records holding it, by sort key and key.
    pub(crate) sorted: BTreeMap<(TypeName, FieldName), SortedRows>,
    /// The fields of each type's order index the lake chose, kept beside
    /// the log (`Chosen`): the fold keeps their rows, a read takes only
    /// the ones kept.
    chosen: BTreeMap<TypeName, Chosen>,
    /// Each head record's rows in `sorted`, to take them out again.
    sorted_rows: BTreeMap<(TypeName, RecordKey), RecordRows>,
    /// The ends of the head's link records (`sys.link.*`): per link type,
    /// end field (`from`, `to`) and the key it names, the link records
    /// holding it. A walk over link records reads what it reaches.
    pub(crate) link_ends: BTreeMap<(TypeName, FieldName, String), BTreeSet<RecordKey>>,
    /// Each head link record's entries in `link_ends`.
    link_end_rows: BTreeMap<(TypeName, RecordKey), Vec<(FieldName, String)>>,
    /// The edges into each record over relations carried by a reference
    /// field (`engines::links_of` of every head state), by source and
    /// relation. A walk `In` over them reads what it reaches.
    pub(crate) in_edges: BTreeMap<(TypeName, RecordKey), BTreeMap<EdgeFrom, Edge>>,
    /// Each head record's entries in `in_edges`: the target and the edge.
    in_edge_rows: BTreeMap<(TypeName, RecordKey), Vec<(Target, EdgeFrom)>>,
}

/// The feed's index of the log (`lakelands_core::feed`).
#[derive(Debug, Default)]
pub(crate) struct Routes {
    pub(crate) events: BTreeMap<Position, (&'static str, Vec<About>)>,
    pub(crate) kinds: BTreeMap<&'static str, BTreeSet<Position>>,
    pub(crate) types: BTreeMap<TypeName, BTreeSet<Position>>,
    pub(crate) keys: BTreeMap<RecordKey, BTreeSet<Position>>,
    pub(crate) sources: BTreeMap<SourceId, BTreeSet<Position>>,
}

impl Routes {
    fn put(&mut self, a: Applied) {
        self.kinds.entry(a.kind).or_default().insert(a.at);
        for about in &a.about {
            if let Some(ty) = &about.ty {
                self.types.entry(ty.clone()).or_default().insert(a.at);
            }
            if let Some(key) = &about.key {
                self.keys.entry(key.clone()).or_default().insert(a.at);
            }
        }
        if let Some(source) = a.source {
            self.sources.entry(source).or_default().insert(a.at);
        }
        self.events.insert(a.at, (a.kind, a.about));
    }
}

/// An edge's source and relation, which tell it apart among the edges
/// into one record.
type EdgeFrom = (TypeName, RecordKey, RelationName);

/// The record an edge leads to.
type Target = (TypeName, RecordKey);

/// One record's rows of the order index: field, then sort key.
type RecordRows = Vec<(FieldName, Vec<u8>)>;

/// One field's rows of the order index: sort key, then record key.
pub(crate) type SortedRows = BTreeSet<(Vec<u8>, RecordKey)>;

impl Projection {
    /// Applies one transaction's puts and records that the projection now
    /// reflects the log through `through`. Checked first, applied whole:
    /// a commit behind the projection, or a put valid only after
    /// `through`, refuses the transaction and changes nothing.
    pub(crate) fn commit(&mut self, puts: Vec<Put>, through: Position) -> Result<()> {
        let late = puts.iter().map(Put::own_position).find(|p| *p > through);
        if let Some(refused) = engines::commit_refusal(self.folded, late, through) {
            return Err(refused);
        }
        let states = puts.iter().filter_map(|put| match put {
            Put::Record(r) | Put::Head(r) => Some(r),
            _ => None,
        });
        self.bases.2 = engines::shared_bases(states);
        let mut records = BTreeSet::new();
        let mut types = BTreeSet::new();
        for put in puts {
            match &put {
                Put::DroppedRecord(ty, key) => drop(records.insert((ty.clone(), key.clone()))),
                Put::Record(r) => {
                    records.insert((r.ty.clone(), r.key.clone()));
                    // The setting's write rebuilds the index it asks for.
                    if let Some(ty) = lakelands_core::engines::sort_index_type(r) {
                        types.extend(lakelands_core::engines::ordered_names(&ty));
                    }
                }
                Put::Declaration(d) => {
                    if let Declaration::Type(t) = &d.declaration {
                        types.insert(t.name.clone());
                    }
                }
                _ => {}
            }
            self.apply(put);
        }
        // A host's setting that names a field wins over the lake's choice.
        for ty in &types {
            if !self.named_fields(ty).is_empty() {
                self.chosen.remove(ty);
            }
        }
        // The order index follows the head: every record whose latest
        // state the transaction put, every record of a type it declared
        // again, and of a type and its kept name whose `sort_index.<type>`
        // setting it put.
        for (ty, key) in records {
            if !types.contains(&ty) {
                self.index(&ty, &key);
            }
        }
        // A whole type, a page of keys at a time, so a rebuild that refolds
        // a declaration or a setting holds a page, not the type.
        for ty in &types {
            let mut after = None;
            while let Some((last, _)) = self.index_page(ty, after.as_ref(), 1024) {
                after = Some(last);
            }
        }
        self.bases.2.clear();
        self.folded = Some(through);
        Ok(())
    }

    /// The keys of `ty` with a state put after `at`, and their states at
    /// `at`; `None` past `ceiling` states put.
    pub(crate) fn changed(
        &self,
        ty: &TypeName,
        at: Position,
        ceiling: usize,
    ) -> Option<engines::Changed> {
        let after = (Position(at.0 + 1), RecordKey::from(""));
        let put = self
            .changes
            .get(ty)
            .into_iter()
            .flat_map(|c| c.range(after.clone()..));
        let mut keys = BTreeSet::new();
        for (n, (_, key)) in put.enumerate() {
            if n >= ceiling {
                return None;
            }
            keys.insert(key.clone());
        }
        let states = (keys.iter())
            .filter_map(|k| self.record_of(ty, k, Some(at)))
            .collect();
        Some(engines::Changed { keys, states })
    }

    /// The fields of `ty` whose order index its `sort_index.<type>`
    /// setting at the head names that its declaration
    /// at the head (a kept name's view's) declares with a kind the index
    /// keeps.
    fn named_fields(&self, ty: &TypeName) -> Vec<FieldName> {
        let declared = lakelands_core::engines::kept_view(ty).unwrap_or_else(|| ty.clone());
        let Some(decl) = self.type_at(&declared, Some(Position(u64::MAX))) else {
            return Vec::new();
        };
        let setting = self
            .records
            .get(&(
                TypeName::from(lakelands_api::seam::SETTING_TYPE),
                RecordKey::from(lakelands_core::engines::sort_index_key(ty)),
            ))
            .and_then(History::latest)
            .map(|h| h.record(&self.bases));
        lakelands_core::engines::sorted_fields(
            ty,
            decl,
            setting
                .as_ref()
                .and_then(lakelands_core::engines::setting_value),
        )
    }

    /// The fields the fold keeps rows of: those the setting names, else
    /// those the lake chose, the ones being built too.
    pub(crate) fn sorted_fields(&self, ty: &TypeName) -> Vec<FieldName> {
        self.order_fields(ty, true)
    }

    /// The fields a read takes the order index for.
    pub(crate) fn read_fields(&self, ty: &TypeName) -> Vec<FieldName> {
        self.order_fields(ty, false)
    }

    fn order_fields(&self, ty: &TypeName, building: bool) -> Vec<FieldName> {
        let named = self.named_fields(ty);
        let Some(chosen) = self.chosen.get(ty).filter(|_| named.is_empty()) else {
            return named;
        };
        let Some(decl) = self.type_at(ty, Some(Position(u64::MAX))) else {
            return named;
        };
        lakelands_core::engines::order_fields(ty, decl, None, chosen, building)
    }

    /// The fields of `ty`'s order index the lake chose.
    pub(crate) fn chosen(&self, ty: &TypeName) -> Chosen {
        self.chosen.get(ty).cloned().unwrap_or_default()
    }

    /// One step of choosing an order index (`ProjectionStore::choose_order`).
    pub(crate) fn choose(&mut self, ty: &TypeName, step: &Choose) -> Option<RecordKey> {
        // A type the host's setting names fields of, a view, an undeclared
        // type and a field it does not hold with a kind the index keeps are
        // no step's, as on every engine.
        let decl = self.type_at(ty, Some(Position(u64::MAX)))?;
        let sortable = |f: &FieldName| {
            (decl.fields.iter()).any(|d| d.name == *f && lakelands_api::seam::sortable(&d.ty.kind))
        };
        if !self.named_fields(ty).is_empty()
            || decl.view.is_some()
            || matches!(step, Choose::Start(f) if !sortable(f))
        {
            return None;
        }
        let mut chosen = self.chosen.get(ty).cloned().unwrap_or_default();
        if !lakelands_core::engines::chosen_after(&mut chosen, step) {
            return None;
        }
        if let Choose::Fill { after, limit, .. } = step {
            let page = self.index_page(ty, after.as_ref(), *limit as usize);
            return page
                .filter(|(_, n)| *n == *limit as usize)
                .map(|(last, _)| last);
        }
        match chosen == Chosen::default() {
            true => self.chosen.remove(ty),
            false => self.chosen.insert(ty.clone(), chosen),
        };
        // A field dropped takes its rows out of every record's.
        if matches!(step, Choose::Drop(_)) {
            self.index_page(ty, None, usize::MAX);
        }
        None
    }

    /// Re-indexes up to `limit` records of `ty` after the key `after`,
    /// and answers the last key and how many it did.
    fn index_page(
        &mut self,
        ty: &TypeName,
        after: Option<&RecordKey>,
        limit: usize,
    ) -> Option<(RecordKey, usize)> {
        let from = after.cloned().unwrap_or_else(|| RecordKey::from(""));
        let keys: Vec<RecordKey> = (self.records)
            .range((ty.clone(), from)..)
            .take_while(|((t, _), _)| t == ty)
            .map(|((_, k), _)| k.clone())
            .filter(|k| after != Some(k))
            .take(limit)
            .collect();
        keys.iter().for_each(|key| self.index(ty, key));
        keys.last().map(|last| (last.clone(), keys.len()))
    }

    /// Rewrite one record's rows in the head indexes from its latest
    /// state: the order index, under its type's setting and declaration
    /// at the head; the ends of a link record; the edges it declares into
    /// other records.
    fn index(&mut self, ty: &TypeName, key: &RecordKey) {
        let id = (ty.clone(), key.clone());
        for (field, sort_key) in self.sorted_rows.remove(&id).unwrap_or_default() {
            if let Some(set) = self.sorted.get_mut(&(ty.clone(), field)) {
                set.remove(&(sort_key, key.clone()));
            }
        }
        for (field, end) in self.link_end_rows.remove(&id).unwrap_or_default() {
            let entry = (ty.clone(), field, end);
            if let Some(set) = self.link_ends.get_mut(&entry) {
                set.remove(key);
                if set.is_empty() {
                    self.link_ends.remove(&entry);
                }
            }
        }
        for (to, from) in self.in_edge_rows.remove(&id).unwrap_or_default() {
            if let Some(into) = self.in_edges.get_mut(&to) {
                into.remove(&from);
                if into.is_empty() {
                    self.in_edges.remove(&to);
                }
            }
        }
        let Some(record) = self.record_of(ty, key, Some(Position(u64::MAX))) else {
            return;
        };
        if lakelands_core::links::is_link_type(ty) {
            let ends: Vec<(FieldName, String)> =
                [lakelands_core::links::FROM, lakelands_core::links::TO]
                    .into_iter()
                    .filter_map(|f| {
                        let end = record.fields.get(f)?.as_str()?;
                        Some((FieldName::from(f), end.to_owned()))
                    })
                    .collect();
            for (field, end) in &ends {
                self.link_ends
                    .entry((ty.clone(), field.clone(), end.clone()))
                    .or_default()
                    .insert(key.clone());
            }
            if !ends.is_empty() {
                self.link_end_rows.insert(id.clone(), ends);
            }
        }
        if let Some(decl) = self.type_at(ty, Some(Position(u64::MAX))) {
            let edges = lakelands_core::engines::links_of(decl, &record, None);
            let mut rows = Vec::with_capacity(edges.len());
            for e in edges {
                let from = (e.from.0.clone(), e.from.1.clone(), e.relation.clone());
                rows.push((e.to.clone(), from.clone()));
                self.in_edges
                    .entry(e.to.clone())
                    .or_default()
                    .insert(from, e);
            }
            if !rows.is_empty() {
                self.in_edge_rows.insert(id.clone(), rows);
            }
        }
        let record = &record;
        let fields = self.sorted_fields(ty);
        let rows: RecordRows = lakelands_core::engines::order_rows(&fields, record)
            .into_iter()
            .map(|(f, k)| (f.clone(), k))
            .collect();
        for (field, sort_key) in &rows {
            self.sorted
                .entry((ty.clone(), field.clone()))
                .or_default()
                .insert((sort_key.clone(), key.clone()));
        }
        if !rows.is_empty() {
            self.sorted_rows.insert(id, rows);
        }
    }

    /// At most `n` rows of the order index for `field` of `ty`, in the
    /// index's order for `dir` (the sort key ascending or descending, the
    /// key ascending either way), strictly after `from`.
    pub(crate) fn sorted_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        dir: Direction,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Vec<(Vec<u8>, RecordKey)> {
        use std::ops::Bound;
        let Some(set) = self.sorted.get(&(ty.clone(), field.clone())) else {
            return Vec::new();
        };
        let n = n as usize;
        let from = from.map(|(k, key)| (k.to_vec(), RecordKey::from(key)));
        match dir {
            Direction::Asc => {
                let start = match &from {
                    Some(f) => Bound::Excluded(f.clone()),
                    None => Bound::Unbounded,
                };
                set.range((start, Bound::Unbounded))
                    .take(n)
                    .cloned()
                    .collect()
            }
            Direction::Desc => {
                let mut out = Vec::new();
                // The rest of the run `from` is in, then the runs below it,
                // each in key order.
                let below = match &from {
                    Some((k, key)) => {
                        out.extend(
                            set.range((
                                Bound::Excluded((k.clone(), key.clone())),
                                Bound::Unbounded,
                            ))
                            .take_while(|(sk, _)| sk == k)
                            .take(n)
                            .cloned(),
                        );
                        Bound::Excluded((k.clone(), RecordKey::from("")))
                    }
                    None => Bound::Unbounded,
                };
                let mut run: Vec<(Vec<u8>, RecordKey)> = Vec::new();
                for row in set.range((Bound::Unbounded, below)).rev() {
                    if out.len() >= n {
                        break;
                    }
                    if run.first().is_some_and(|(sk, _)| *sk != row.0) {
                        out.extend(run.drain(..).rev().take(n - out.len()));
                    }
                    run.push(row.clone());
                }
                if out.len() < n {
                    out.extend(run.drain(..).rev().take(n - out.len()));
                }
                out.truncate(n);
                out
            }
        }
    }

    /// Whether the head record `key` of `ty` has a row in the order index
    /// for `field`.
    pub(crate) fn has_sorted(&self, ty: &TypeName, key: &RecordKey, field: &FieldName) -> bool {
        self.sorted_rows
            .get(&(ty.clone(), key.clone()))
            .is_some_and(|rows| rows.iter().any(|(f, _)| f == field))
    }

    fn apply(&mut self, put: Put) {
        match put {
            Put::Declaration(d) => {
                let since = d.since;
                self.declarations
                    .entry(lakelands_core::engines::declaration_key(&d.declaration))
                    .or_default()
                    .put(since, d);
            }
            Put::Record(r) => {
                let (from, held) = (r.position, Held::of(&r, &mut self.bases));
                self.changes
                    .entry(r.ty.clone())
                    .or_default()
                    .insert((from, r.key.clone()));
                self.records
                    .entry((r.ty.clone(), r.key.clone()))
                    .or_default()
                    .put(from, held);
            }
            Put::Proposal { proposal, at } => {
                (self.by_position).insert(proposal.position, proposal.id.clone());
                self.proposals
                    .entry(proposal.id.clone())
                    .or_default()
                    .put(at, proposal);
            }
            Put::Membership {
                set,
                ty,
                key,
                member,
                at,
            } => self
                .members
                .entry((set, ty, key))
                .or_default()
                .put(at, member),
            Put::Output {
                seam,
                input,
                output,
                at,
            } => self
                .outputs
                .entry((seam, input))
                .or_default()
                .put(at, output),
            Put::Applied(a) => {
                self.applied.entry(a.id.clone()).or_insert(a.at);
                self.routes.put(a);
            }
            Put::Head(r) => {
                let id = (r.ty.clone(), r.key.clone());
                match r.is_ended() {
                    true => drop(self.records.remove(&id)),
                    false => {
                        let held = Held::of(&r, &mut self.bases);
                        drop(self.records.insert(id, History(vec![(r.position, held)])))
                    }
                }
            }
            Put::DroppedRecord(ty, key) => {
                let id = (ty, key);
                let states = self.records.remove(&id).unwrap_or_default();
                if let Some(changes) = self.changes.get_mut(&id.0) {
                    for (at, _) in states.0 {
                        changes.remove(&(at, id.1.clone()));
                    }
                }
                self.members
                    .retain(|(_, ty, key), _| (ty, key) != (&id.0, &id.1));
            }
            Put::DroppedOutput(seam, input) => drop(self.outputs.remove(&(seam, input))),
            Put::DroppedBases => {
                let states = self.records.values().flat_map(|h| &h.0);
                let named: BTreeSet<i64> = states.filter_map(|(_, held)| held.base).collect();
                let Bases(by_id, ids, _) = &mut self.bases;
                by_id.retain(|id, b| {
                    let kept = named.contains(id);
                    if !kept {
                        ids.remove(b);
                    }
                    kept
                });
            }
            Put::HeadsEnded { ty, .. } => {
                let from = (ty.clone(), RecordKey::from(""));
                let of = self.records.range(from..).map(|(id, _)| id);
                let ids: Vec<_> = of.take_while(|(t, _)| *t == ty).cloned().collect();
                ids.iter().for_each(|id| drop(self.records.remove(id)));
            }
        }
    }

    /// Every declaration in force at `at`, in identity order.
    pub(crate) fn declarations_at(&self, at: Option<Position>) -> Vec<Declared> {
        self.declarations
            .values()
            .filter_map(|h| h.at(at).cloned())
            .collect()
    }

    /// Every state of a declaration replaced after `from` and by `to`.
    pub(crate) fn replaced(&self, from: Position, to: Position) -> Vec<Declared> {
        let ended =
            |w: &[(Position, Declared)]| (w[1].0 > from && w[1].0 <= to).then(|| w[0].1.clone());
        let each = self
            .declarations
            .values()
            .flat_map(|h| h.0.windows(2).filter_map(ended));
        each.collect()
    }

    /// The type declaration in force for `ty` at `at`.
    pub(crate) fn type_at(&self, ty: &TypeName, at: Option<Position>) -> Option<&TypeDecl> {
        match &self
            .declarations
            .get(&format!("type {ty}"))?
            .at(at)?
            .declaration
        {
            Declaration::Type(t) => Some(t),
            _ => None,
        }
    }

    /// Every type declaration in force at `at`.
    pub(crate) fn types_at(&self, at: Option<Position>) -> impl Iterator<Item = &TypeDecl> {
        self.declarations
            .values()
            .filter_map(move |h| match &h.at(at)?.declaration {
                Declaration::Type(t) => Some(t),
                _ => None,
            })
    }

    /// The record of `ty` at `key` in its state at `at`.
    pub(crate) fn record_of(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        at: Option<Position>,
    ) -> Option<StoredRecord> {
        let h = self.records.get(&(ty.clone(), key.clone()))?;
        h.at(at).map(|h| h.record(&self.bases))
    }

    /// Every record of `ty` in its state at `at`, in key order, whatever
    /// its flags.
    pub(crate) fn records_at<'a>(
        &'a self,
        ty: &'a TypeName,
        at: Option<Position>,
    ) -> impl Iterator<Item = StoredRecord> + 'a {
        self.records
            .range((ty.clone(), RecordKey::from(""))..)
            .take_while(move |((t, _), _)| t == ty)
            .filter_map(move |(_, h)| h.at(at))
            .map(|h| h.record(&self.bases))
    }

    /// At most `n` records of `ty` in their state at `at` after the key
    /// `after` (from the first when `None`), in key order, whatever their
    /// flags.
    pub(crate) fn records_after(
        &self,
        ty: &TypeName,
        at: Option<Position>,
        after: Option<&str>,
        n: u32,
    ) -> Vec<StoredRecord> {
        use std::ops::Bound;
        let from = match after {
            Some(k) => Bound::Excluded((ty.clone(), RecordKey::from(k))),
            None => Bound::Included((ty.clone(), RecordKey::from(""))),
        };
        self.records
            .range((from, Bound::Unbounded))
            .take_while(|((t, _), _)| t == ty)
            .filter_map(|(_, h)| h.at(at))
            .take(n as usize)
            .map(|h| h.record(&self.bases))
            .collect()
    }
}

#[cfg(test)]
mod tests;
