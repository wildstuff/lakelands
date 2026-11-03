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

use lakelands_api::engine::{
    About, Applied, Choose, Chosen, Declared, KeyRange, SeriesPair, SeriesState,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{
    FieldName, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName,
};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::quarantine::{QuarantineId, Quarantined};
use lakelands_api::query::Edge;
use lakelands_api::read::{Direction, ThinnedAct};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::time::Timestamp;
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
    /// A record's state put beside the writer: a `sort_index.<type>`
    /// setting marks what it asks for as building instead of walking the
    /// type in this transaction (099).
    Live(StoredRecord),
    /// The one state of a key of a head-only name, or its end: it takes
    /// the place of whatever the key held, and nothing indexes it.
    Head(StoredRecord),
    /// Every key of a head-only name ended at once, at `at`.
    HeadsEnded { ty: TypeName, at: Position },
    /// An entry of a view's order at `key`, none for one deleted: the
    /// order holds one entry a key (record 098, F11).
    Order {
        name: TypeName,
        key: RecordKey,
        entry: Option<Vec<u8>>,
    },
    /// Every entry of a view's order ended at once.
    OrdersEnded { name: TypeName },
    /// The bases no record's state names.
    DroppedBases,
    /// A proposal's state from `at`.
    Proposal { proposal: Proposal, at: Position },
    /// A quarantined record's state from `at`.
    Quarantined { row: Box<Quarantined>, at: Position },
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
    /// A thinning act's row from `at`, keeping the `at` of a row already
    /// begun.
    ThinnedAct { act: ThinnedAct, at: Position },
    /// A thinned record's tombstone from `at`.
    Thinned {
        ty: TypeName,
        key: RecordKey,
        before: Timestamp,
        at: Position,
    },
}

impl Put {
    /// The position the put's state is valid from.
    fn own_position(&self) -> Position {
        match self {
            Put::Declaration(d) => d.since,
            Put::Record(r) | Put::Live(r) | Put::Head(r) => r.position,
            Put::Applied(a) => a.at,
            Put::Order { .. } | Put::OrdersEnded { .. } => Position::GENESIS,
            Put::DroppedRecord(..) | Put::DroppedOutput(..) | Put::DroppedBases => {
                Position::GENESIS
            }
            Put::Membership { at, .. }
            | Put::HeadsEnded { at, .. }
            | Put::Proposal { at, .. }
            | Put::Quarantined { at, .. }
            | Put::Output { at, .. }
            | Put::ThinnedAct { at, .. }
            | Put::Thinned { at, .. } => *at,
        }
    }
}

/// One lake's projection: every kept state, and how far it reflects the
/// log.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) declarations: BTreeMap<String, History<Declared>>,
    pub(crate) records: BTreeMap<(TypeName, RecordKey), History<Held>>,
    /// The entries of each view's order, by name and key: one an entry,
    /// and no state, since a read of an order is at the head alone
    /// (record 098, F11).
    pub(crate) orders: BTreeMap<(TypeName, RecordKey), Vec<u8>>,
    pub(crate) bases: Bases,
    pub(crate) proposals: BTreeMap<ProposalId, History<Proposal>>,
    /// Each proposal's id by its position, newest last (lakelands#1063).
    pub(crate) by_position: BTreeMap<Position, ProposalId>,
    pub(crate) quarantined: BTreeMap<QuarantineId, History<Quarantined>>,
    /// Each quarantined record's id by every type and key its rows stood
    /// at: those a write at a key may close.
    pub(crate) quarantined_at: BTreeMap<(TypeName, RecordKey), BTreeSet<QuarantineId>>,
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
    /// The series pairs of each type and whether each is whole, in place of
    /// the `series.<type>` meta row: the fold keeps their rows from a
    /// pair's `Start` (097 §The series index).
    series: BTreeMap<TypeName, SeriesState>,
    /// Each head record's rows in `sorted`, to take them out again.
    sorted_rows: BTreeMap<(TypeName, RecordKey), RecordRows>,
    /// The ends of the head's link records (`sys.link.*`): per link type,
    /// end field (`from`, `to`) and the key it names, the link records
    /// holding it. A walk over link records reads what it reaches.
    pub(crate) link_ends: BTreeMap<(TypeName, FieldName, String), BTreeSet<RecordKey>>,
    /// Each head link record's entries in `link_ends`.
    link_end_rows: BTreeMap<(TypeName, RecordKey), Vec<(FieldName, String)>>,
    /// The edges into each record over relations carried by a field, one
    /// key or an array (`engines::links_of` of every head state), by source and
    /// relation. A walk `In` over them reads what it reaches.
    pub(crate) in_edges: BTreeMap<(TypeName, RecordKey), BTreeMap<EdgeFrom, Edge>>,
    /// Each head record's entries in `in_edges`: the target and the edge.
    in_edge_rows: BTreeMap<(TypeName, RecordKey), Vec<(Target, EdgeFrom)>>,
    /// The exact lookup of the head (057, 097 §The memory engine): per
    /// type, field and value token, the head records holding it.
    pub(crate) lookup: BTreeMap<(TypeName, FieldName, String), BTreeSet<RecordKey>>,
    /// Each head record's entries in `lookup`, to take them out again.
    lookup_rows: BTreeMap<(TypeName, RecordKey), Vec<(FieldName, String)>>,
    /// The thinning acts, one a type, field and horizon (`thinned_types`):
    /// the position each began at and the act's row, moved by each part.
    pub(crate) thinned_acts: BTreeMap<(TypeName, FieldName, Timestamp), (Position, ThinnedAct)>,
    /// The records an act took (`thinned`), each key's horizon and the
    /// position its act began at.
    pub(crate) thinned: BTreeMap<(TypeName, RecordKey), (Timestamp, Position)>,
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

/// The field a step of choosing names.
#[allow(deprecated)]
fn chosen_field(step: &Choose) -> Option<&FieldName> {
    match step {
        Choose::Start(f) | Choose::Keep(f) | Choose::Drop(f) => Some(f),
        Choose::Fill { field, .. } | Choose::DropChunk { field, .. } => Some(field),
    }
}

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
        // The types whose exact lookup the transaction moves: a
        // `lookup_index.<type>` setting's, a declaration's own and those
        // it reads the type by (057, 097 §The memory engine).
        let mut looked = BTreeSet::new();
        let mut live = Vec::new();
        // The fields each setting named before this transaction put it, so
        // a setting taken back leaves its old fields to a `Drop`.
        let mut was_named: BTreeMap<TypeName, Vec<FieldName>> = BTreeMap::new();
        // The types whose series pairs the transaction makes whole (a
        // setting written into the log) and those it marks building (one
        // put beside the writer).
        let mut series_types = BTreeSet::new();
        let mut live_series = Vec::new();
        // The types a declaration this transaction put declares again: a
        // series pair goes back to `Start`, since the declaration may have
        // dropped or changed a field its rows were filled under.
        let mut redeclared = BTreeSet::new();
        for put in puts {
            match &put {
                Put::DroppedRecord(ty, key) => drop(records.insert((ty.clone(), key.clone()))),
                Put::Record(r) => {
                    records.insert((r.ty.clone(), r.key.clone()));
                    // The setting's write rebuilds the index it asks for.
                    if let Some(ty) = lakelands_core::engines::sort_index_type(r) {
                        types.extend(lakelands_core::engines::ordered_names(&ty));
                    }
                    if let Some(ty) = lakelands_core::engines::lookup_index_type(r) {
                        looked.insert(ty);
                    }
                    if let Some(ty) = lakelands_core::engines::series_index_type(r) {
                        series_types.insert(ty);
                    }
                }
                Put::Live(r) => {
                    records.insert((r.ty.clone(), r.key.clone()));
                    // Beside the writer the setting only marks what it
                    // asks for as building; a build fills it in turns. What
                    // it named before and no longer does is read here, over
                    // the setting this put replaces.
                    if let Some(ty) = lakelands_core::engines::sort_index_type(r) {
                        for name in lakelands_core::engines::ordered_names(&ty) {
                            was_named
                                .entry(name.clone())
                                .or_insert_with(|| self.named_fields(&name));
                        }
                        live.extend(lakelands_core::engines::sort_index_type(r));
                    }
                    if let Some(ty) = lakelands_core::engines::series_index_type(r) {
                        live_series.push(ty);
                    }
                }
                Put::Declaration(d) => {
                    if let Declaration::Type(t) = &d.declaration {
                        types.insert(t.name.clone());
                        redeclared.insert(t.name.clone());
                    }
                    looked.extend(lakelands_core::engines::looked_up_by(&d.declaration));
                }
                _ => {}
            }
            self.apply(put);
        }
        // A host's setting wins over the lake's choice for the fields it
        // names, and only those: the setting and the lake share the rows
        // field by field.
        for ty in &types {
            let named = self.named_fields(ty);
            if named.is_empty() {
                continue;
            }
            if let Some(chosen) = self.chosen.get_mut(ty) {
                chosen.kept.retain(|f| !named.contains(f));
                chosen.building.retain(|f| !named.contains(f));
                chosen.dropping.retain(|f| !named.contains(f));
                if *chosen == Chosen::default() {
                    self.chosen.remove(ty);
                }
            }
        }
        // A pair's state comes before the records are indexed: a write
        // keeps a started pair's rows, and a declaration starts the pairs
        // a setting folded before it named, so the reindex writes theirs.
        for ty in redeclared {
            let mut state = self.series_state(&ty);
            if !state.kept.is_empty() {
                state.building.append(&mut state.kept);
                self.set_series(&ty, &state);
            }
            self.mark_series(&ty);
        }
        // A series setting written into the log fills its type's pairs
        // whole, at its event; one put beside the writer marks them
        // building, and a build fills them in turns.
        for ty in series_types {
            self.rebuild_series(&ty);
        }
        for ty in live_series {
            self.mark_series(&ty);
        }
        // The order index follows the head: every record whose latest
        // state the transaction put, every record of a type it declared
        // again, and of a type and its kept name whose `sort_index.<type>`
        // setting it put.
        let mut lap: Option<(TypeName, Option<BTreeSet<FieldName>>)> = None;
        for (ty, key) in records {
            if !types.contains(&ty) {
                if lap.as_ref().is_none_or(|(t, _)| *t != ty) {
                    lap = Some((ty.clone(), self.looked_fields(&ty)));
                }
                self.index(&ty, &key, lap.as_ref().and_then(|(_, l)| l.as_ref()));
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
        for ty in &looked {
            if types.contains(ty) {
                continue;
            }
            let mut after = None;
            while let Some((last, _)) = self.index_page(ty, after.as_ref(), 1024) {
                after = Some(last);
            }
        }
        // A setting put beside the writer marks the fields it asks for as
        // building: a read keeps the old road until each field's `Keep`. A
        // field it no longer names is marked dropping, and a `Drop` clears
        // its rows in parts. The setting and the lake share the rows field
        // by field: a chosen field the setting names is the setting's now.
        for ty in live {
            for name in lakelands_core::engines::ordered_names(&ty) {
                let named = self.named_fields(&name);
                let mut chosen = self.chosen.get(&name).cloned().unwrap_or_default();
                chosen.kept.retain(|f| !named.contains(f));
                chosen.building.retain(|f| !named.contains(f));
                let mut dropping = std::mem::take(&mut chosen.dropping);
                let was = was_named.get(&name).cloned().unwrap_or_default();
                for f in was {
                    if !named.contains(&f) && !dropping.contains(&f) {
                        dropping.push(f);
                    }
                }
                dropping.retain(|f| {
                    !named.contains(f) && !chosen.kept.contains(f) && !chosen.building.contains(f)
                });
                chosen.setting_building = named;
                chosen.dropping = dropping;
                match chosen == Chosen::default() {
                    true => self.chosen.remove(&name),
                    false => self.chosen.insert(name, chosen),
                };
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
        let setting = self.setting_at(ty);
        lakelands_core::engines::sorted_fields(
            ty,
            decl,
            setting
                .as_ref()
                .and_then(lakelands_core::engines::setting_value),
        )
    }

    /// `ty`'s `sort_index.<type>` setting at the head, a kept name's its
    /// view's.
    fn setting_at(&self, ty: &TypeName) -> Option<StoredRecord> {
        self.records
            .get(&(
                TypeName::from(lakelands_api::seam::SETTING_TYPE),
                RecordKey::from(lakelands_core::engines::sort_index_key(ty)),
            ))
            .and_then(History::latest)
            .map(|h| h.record(&self.bases))
    }

    /// The fields the fold keeps rows of: those the setting names, else
    /// those the lake chose, the ones being built too.
    pub(crate) fn sorted_fields(&self, ty: &TypeName) -> Vec<FieldName> {
        let mut fields = self.order_fields(ty, true);
        // A type's series pairs keep their rows beside its order fields',
        // from the pair's `Start` on; no read takes one as an order field.
        fields.extend(engines::series_fields(&self.series_state(ty)));
        fields
    }

    /// The series pairs of `ty` and whether each is whole: none where the
    /// projection holds none.
    pub(crate) fn series_state(&self, ty: &TypeName) -> SeriesState {
        self.series.get(ty).cloned().unwrap_or_default()
    }

    /// Keep `ty`'s series pairs as `state`: nothing where it holds none.
    fn set_series(&mut self, ty: &TypeName, state: &SeriesState) {
        match *state == SeriesState::default() {
            true => drop(self.series.remove(ty)),
            false => drop(self.series.insert(ty.clone(), state.clone())),
        }
    }

    /// `ty`'s `series_index.<type>` setting at the head.
    fn series_setting_at(&self, ty: &TypeName) -> Option<StoredRecord> {
        let key = engines::series_index_key(ty);
        self.records
            .get(&(
                TypeName::from(lakelands_api::seam::SETTING_TYPE),
                RecordKey::from(key),
            ))
            .and_then(History::latest)
            .map(|h| h.record(&self.bases))
    }

    /// The pairs `ty`'s series setting at the head asks for, under its
    /// declaration at the head.
    fn series_pairs(&self, ty: &TypeName) -> Vec<SeriesPair> {
        let declared = engines::kept_view(ty).unwrap_or_else(|| ty.clone());
        let Some(decl) = self.type_at(&declared, Some(Position(u64::MAX))) else {
            return Vec::new();
        };
        let setting = self.series_setting_at(ty);
        engines::series_pairs(decl, setting.as_ref().and_then(engines::setting_value))
    }

    /// Fill `ty`'s series pairs whole from its head records, and write them
    /// kept: a fold from the log makes a pair whole at its event rather
    /// than marking it for a build.
    fn rebuild_series(&mut self, ty: &TypeName) {
        let pairs = self.series_pairs(ty);
        let state = SeriesState {
            kept: pairs,
            building: Vec::new(),
        };
        self.set_series(ty, &state);
        self.walk_type(ty);
    }

    /// Mark the pairs `ty`'s series setting asks for as building, and
    /// leave those it no longer names out; a pair still named and already
    /// kept stays kept.
    fn mark_series(&mut self, ty: &TypeName) {
        let named = self.series_pairs(ty);
        let mut state = self.series_state(ty);
        // A pair the setting no longer names is taken back: its rows go in
        // the writer's turn, as a `Drop` does, so none is left without a
        // pair.
        let gone: Vec<_> = state
            .kept
            .iter()
            .chain(state.building.iter())
            .filter(|p| !named.contains(p))
            .cloned()
            .collect();
        for pair in &gone {
            let field = FieldName::from(lakelands_api::seam::series_field(
                pair.order.as_str(),
                pair.series.as_str(),
            ));
            let mut after = None;
            while let Some(last) = self.clear_series_rows(ty, &field, after.as_ref(), 4096) {
                after = Some(last);
            }
        }
        state.kept.retain(|p| named.contains(p));
        state.building.retain(|p| named.contains(p));
        for pair in named {
            if !state.kept.contains(&pair) && !state.building.contains(&pair) {
                state.building.push(pair);
            }
        }
        self.set_series(ty, &state);
    }

    /// Rewrite every head record of `ty`'s rows, in pages, so a change to
    /// its fields holds a page, not the type.
    fn walk_type(&mut self, ty: &TypeName) {
        let mut after = None;
        while let Some((last, _)) = self.index_page(ty, after.as_ref(), 1024) {
            after = Some(last);
        }
    }

    /// Clear `field`'s rows of up to `limit` head records of `ty` after
    /// `after`; the key the chunk ended at, none once the type is cleared.
    fn clear_series_rows(
        &mut self,
        ty: &TypeName,
        field: &FieldName,
        after: Option<&RecordKey>,
        limit: usize,
    ) -> Option<RecordKey> {
        let from = after.cloned().unwrap_or_else(|| RecordKey::from(""));
        let keys: Vec<RecordKey> = (self.records)
            .range((ty.clone(), from)..)
            .take_while(|((t, _), _)| t == ty)
            .map(|((_, k), _)| k.clone())
            .filter(|k| after != Some(k))
            .take(limit)
            .collect();
        for key in &keys {
            let id = (ty.clone(), key.clone());
            let mut gone = Vec::new();
            if let Some(rows) = self.sorted_rows.get_mut(&id) {
                rows.retain(|(f, sort_key)| {
                    let keep = f != field;
                    if !keep {
                        gone.push(sort_key.clone());
                    }
                    keep
                });
            }
            for sort_key in gone {
                if let Some(set) = self.sorted.get_mut(&(ty.clone(), field.clone())) {
                    set.remove(&(sort_key, key.clone()));
                }
            }
        }
        keys.last().cloned().filter(|_| keys.len() == limit)
    }

    /// One step of a series pair's build (`ProjectionStore::choose_order`).
    #[allow(deprecated)]
    fn choose_series(
        &mut self,
        ty: &TypeName,
        step: &Choose,
        field: &FieldName,
    ) -> Option<RecordKey> {
        let (order, series) = lakelands_api::seam::split_series_field(field.as_str())?;
        let pair = SeriesPair {
            series: FieldName::from(series),
            order: FieldName::from(order),
        };
        let mut state = self.series_state(ty);
        match step {
            Choose::Start(_) => {
                if !state.kept.contains(&pair) && !state.building.contains(&pair) {
                    state.building.push(pair.clone());
                }
                self.set_series(ty, &state);
                None
            }
            Choose::Fill { after, limit, .. } => {
                let page = self.index_page(ty, after.as_ref(), *limit as usize);
                page.filter(|(_, n)| *n == *limit as usize)
                    .map(|(last, _)| last)
            }
            Choose::Keep(_) => {
                state.building.retain(|p| p != &pair);
                if !state.kept.contains(&pair) {
                    state.kept.push(pair.clone());
                }
                self.set_series(ty, &state);
                None
            }
            // A pair taken back: its rows go whole in one step, or a chunk
            // a call, and its state leaves with the last.
            Choose::Drop(_) => {
                let _ = self.clear_series_rows(ty, field, None, usize::MAX);
                state.kept.retain(|p| p != &pair);
                state.building.retain(|p| p != &pair);
                self.set_series(ty, &state);
                None
            }
            Choose::DropChunk { after, limit, .. } => {
                let last = self.clear_series_rows(ty, field, after.as_ref(), *limit as usize);
                if last.is_none() {
                    state.kept.retain(|p| p != &pair);
                    state.building.retain(|p| p != &pair);
                    self.set_series(ty, &state);
                }
                last
            }
        }
    }

    /// The fields a read takes the order index for.
    pub(crate) fn read_fields(&self, ty: &TypeName) -> Vec<FieldName> {
        self.order_fields(ty, false)
    }

    fn order_fields(&self, ty: &TypeName, building: bool) -> Vec<FieldName> {
        let declared = lakelands_core::engines::kept_view(ty).unwrap_or_else(|| ty.clone());
        let Some(decl) = self.type_at(&declared, Some(Position(u64::MAX))) else {
            return Vec::new();
        };
        let setting = self.setting_at(ty);
        let chosen = self.chosen.get(ty).cloned().unwrap_or_default();
        lakelands_core::engines::order_fields(
            ty,
            decl,
            setting
                .as_ref()
                .and_then(lakelands_core::engines::setting_value),
            &chosen,
            building,
        )
    }

    /// The fields of `ty`'s order index the lake chose.
    pub(crate) fn chosen(&self, ty: &TypeName) -> Chosen {
        self.chosen.get(ty).cloned().unwrap_or_default()
    }

    /// One step of choosing an order index (`ProjectionStore::choose_order`).
    #[allow(deprecated)]
    pub(crate) fn choose(&mut self, ty: &TypeName, step: &Choose) -> Option<RecordKey> {
        // A series pair's derived field is kept in the type's `series.`
        // state, not in `Chosen`: its own road fills it from the pair
        // (097 §The series index).
        if let Some(field) = chosen_field(step).filter(|f| engines::is_series_field(f)) {
            return self.choose_series(ty, step, field);
        }
        // A kept name is built as the view it keeps; a field a host's
        // setting names is filled by the setting's own build, field by
        // field.
        let kept = lakelands_core::engines::kept_view(ty);
        let declared = self.type_at(
            &kept.clone().unwrap_or_else(|| ty.clone()),
            Some(Position(u64::MAX)),
        )?;
        if kept.is_none() && declared.view.is_some() {
            return None;
        }
        let sortable = |f: &FieldName| {
            (declared.fields.iter())
                .any(|d| d.name == *f && lakelands_api::seam::sortable(&d.ty.kind))
        };
        let mut chosen = self.chosen.get(ty).cloned().unwrap_or_default();
        // A field a host's setting names is the setting's own build's: no
        // step of the lake's touches it. Every other field shares the road.
        let named = self.named_fields(ty);
        let setting_build = matches!(
            step,
            Choose::Fill { field, .. } | Choose::Keep(field)
                if chosen.setting_building.contains(field)
        );
        let names_field = match step {
            Choose::Start(f) | Choose::Keep(f) | Choose::Drop(f) => named.contains(f),
            Choose::Fill { field, .. } | Choose::DropChunk { field, .. } => named.contains(field),
        };
        if names_field && !setting_build {
            return None;
        }
        if matches!(step, Choose::Start(f) if !sortable(f))
            || !lakelands_core::engines::chosen_after(&mut chosen, step)
        {
            return None;
        }
        if let Choose::Fill { after, limit, .. } = step {
            let page = self.index_page(ty, after.as_ref(), *limit as usize);
            return page
                .filter(|(_, n)| *n == *limit as usize)
                .map(|(last, _)| last);
        }
        // A drop clears its field's rows out of each record's, a chunk a
        // call; its last chunk, which finds no more records, takes the
        // mark away. Both write the mark, so one left by a writer that
        // went is taken up by the next. `Drop` is the whole field in one
        // step, kept for a host's build: it answers none.
        let last = match step {
            Choose::Drop(field) => {
                let _ = self.index_page(ty, None, usize::MAX);
                chosen.dropping.retain(|f| f != field);
                None
            }
            Choose::DropChunk {
                field,
                after,
                limit,
            } => {
                let page = self.index_page(ty, after.as_ref(), *limit as usize);
                let last = page
                    .filter(|(_, n)| *n == *limit as usize)
                    .map(|(last, _)| last);
                if last.is_none() {
                    chosen.dropping.retain(|f| f != field);
                }
                last
            }
            _ => None,
        };
        match chosen == Chosen::default() {
            true => self.chosen.remove(ty),
            false => self.chosen.insert(ty.clone(), chosen),
        };
        last
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
        let looked = self.looked_fields(ty);
        keys.iter()
            .for_each(|key| self.index(ty, key, looked.as_ref()));
        keys.last().map(|last| (last.clone(), keys.len()))
    }

    /// Rewrite one record's rows in the head indexes from its latest
    /// state: the order index, under its type's setting and declaration
    /// at the head; the ends of a link record; the edges it declares into
    /// other records; and the exact lookup, of the fields `looked` keeps
    /// (`None`, every one a string value, a view's), each under the kind
    /// its declaration gives it (057, 097 §A number's exact lookup).
    fn index(&mut self, ty: &TypeName, key: &RecordKey, looked: Option<&BTreeSet<FieldName>>) {
        let id = (ty.clone(), key.clone());
        for (field, token) in self.lookup_rows.remove(&id).unwrap_or_default() {
            let entry = (ty.clone(), field, token);
            if let Some(set) = self.lookup.get_mut(&entry) {
                set.remove(key);
                if set.is_empty() {
                    self.lookup.remove(&entry);
                }
            }
        }
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
        let decl = self.type_at(ty, Some(Position(u64::MAX))).cloned();
        let fields = self.sorted_fields(ty);
        let rows: RecordRows = lakelands_core::engines::order_rows(&fields, decl.as_ref(), record)
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
            self.sorted_rows.insert(id.clone(), rows);
        }
        let mut tokens = Vec::new();
        for (field, value) in &record.fields {
            if looked.is_some_and(|l| !l.contains(field)) {
                continue;
            }
            let kind = decl
                .as_ref()
                .and_then(|d| lakelands_core::engines::field_kind(d, field))
                .copied();
            if let Some(token) = lakelands_core::engines::lookup_token(value, kind) {
                self.lookup
                    .entry((ty.clone(), field.clone(), token.clone()))
                    .or_default()
                    .insert(key.clone());
                tokens.push((field.clone(), token));
            }
        }
        if !tokens.is_empty() {
            self.lookup_rows.insert(id, tokens);
        }
    }

    /// The fields of `ty` the exact lookup keeps at the head, a type it
    /// declares: `None` at an earlier position, and for a view or a name
    /// no type declares (every string field, as the SQL engines keep it).
    pub(crate) fn looked(&self, ty: &TypeName, declared: bool) -> Option<BTreeSet<FieldName>> {
        declared.then(|| self.looked_fields(ty)).flatten()
    }

    /// The fields of `ty` the exact lookup keeps at the head: those
    /// [`engines::looked_up`] decides of the declarations in force and
    /// its `lookup_index.<type>` setting's value (`None`, a view's every
    /// string field, and a name no type declares).
    fn looked_fields(&self, ty: &TypeName) -> Option<BTreeSet<FieldName>> {
        let setting = self
            .records
            .get(&(
                TypeName::from(lakelands_api::seam::SETTING_TYPE),
                RecordKey::from(lakelands_core::engines::lookup_index_key(ty)),
            ))
            .and_then(History::latest)
            .map(|h| h.record(&self.bases));
        let decls = self.declarations_at(Some(Position(u64::MAX)));
        lakelands_core::engines::looked_up(
            ty,
            decls.iter().map(|d| &d.declaration),
            setting
                .as_ref()
                .and_then(lakelands_core::engines::setting_value),
        )
    }

    /// The least key greater than every key at or below `hi`: the next byte
    /// string after it. `None` when `hi` is all `0xff`s, above every key.
    fn past(hi: &[u8]) -> Option<Vec<u8>> {
        let mut out = hi.to_vec();
        while let Some(last) = out.last_mut() {
            if *last == u8::MAX {
                out.pop();
            } else {
                *last += 1;
                return Some(out);
            }
        }
        None
    }

    /// At most `n` rows of the order index for `field` of `ty`, in the
    /// index's order for `dir` (the sort key ascending or descending, the
    /// key ascending either way), strictly after `from` and inside
    /// `window` when it bounds the field.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sorted_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        dir: Direction,
        window: Option<&engines::Window>,
        holds: Option<&dyn Fn(&RecordKey) -> bool>,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Vec<(Vec<u8>, RecordKey)> {
        use std::ops::Bound;
        let Some(set) = self.sorted.get(&(ty.clone(), field.clone())) else {
            return Vec::new();
        };
        let n = n as usize;
        let from = from.map(|(k, key)| (k.to_vec(), RecordKey::from(key)));
        // A window narrows the index's rows to its key interval; the
        // rulebook still rejects rows the filter does not admit.
        let lower = window.map_or(Bound::Unbounded, |w| {
            Bound::Included((w.lo.clone(), RecordKey::from("")))
        });
        let upper = window.map_or(Bound::Unbounded, |w| match Self::past(&w.hi) {
            Some(key) => Bound::Excluded((key, RecordKey::from(""))),
            None => Bound::Unbounded,
        });
        let in_window = |key: &[u8]| window.is_none_or(|w| w.holds(key));
        let keeps = |key: &RecordKey| holds.is_none_or(|h| h(key));
        match dir {
            Direction::Asc => {
                let start = match &from {
                    Some(f) if in_window(&f.0) => Bound::Excluded(f.clone()),
                    // Above the window: the seek starts past every row in it.
                    Some(f) if window.is_some_and(|w| f.0 > w.hi) => return Vec::new(),
                    _ => lower.clone(),
                };
                set.range((start, upper))
                    .filter(|(_, key)| keeps(key))
                    .take(n)
                    .cloned()
                    .collect()
            }
            Direction::Desc => {
                let mut out = Vec::new();
                // The rest of the run `from` is in, then the runs below it,
                // each in key order.
                let below = match &from {
                    Some((k, key)) if in_window(k) => {
                        out.extend(
                            set.range((
                                Bound::Excluded((k.clone(), key.clone())),
                                Bound::Unbounded,
                            ))
                            .take_while(|(sk, _)| sk == k && in_window(sk))
                            .filter(|(_, key)| keeps(key))
                            .take(n)
                            .cloned(),
                        );
                        Bound::Excluded((k.clone(), RecordKey::from("")))
                    }
                    // Below the window: every row the seek reaches is out
                    // of it.
                    Some((k, _)) if window.is_some_and(|w| *k < w.lo) => return Vec::new(),
                    _ => upper,
                };
                let mut run: Vec<(Vec<u8>, RecordKey)> = Vec::new();
                for row in set.range((lower, below)).rev() {
                    if out.len() >= n {
                        break;
                    }
                    if !keeps(&row.1) {
                        continue;
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

    /// How many head records of `ty` hold `token` in `field`, counted up
    /// to `n`: the size of the exact lookup's set.
    pub(crate) fn holders(&self, ty: &TypeName, field: &FieldName, token: &str, n: u64) -> u64 {
        self.lookup
            .get(&(ty.clone(), field.clone(), token.to_owned()))
            .map_or(0, |set| (set.len() as u64).min(n))
    }

    /// Whether the head record `key` of `ty` holds `token` in `field`.
    pub(crate) fn holds(
        &self,
        ty: &TypeName,
        field: &FieldName,
        token: &str,
        key: &RecordKey,
    ) -> bool {
        self.lookup
            .get(&(ty.clone(), field.clone(), token.to_owned()))
            .is_some_and(|set| set.contains(key))
    }

    /// At most `n` rows of the order index for `field` of `ty` under a
    /// token few records hold, in the index's order for `dir`: the rows
    /// of the holders alone, read from each record's own rows, sorted, and
    /// taken after `from` and inside `window` (097 §The memory engine).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn few_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        dir: Direction,
        window: Option<&engines::Window>,
        looked: &FieldName,
        token: &str,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Vec<(Vec<u8>, RecordKey)> {
        let keys = self
            .lookup
            .get(&(ty.clone(), looked.clone(), token.to_owned()));
        let mut rows: Vec<(Vec<u8>, RecordKey)> = keys
            .into_iter()
            .flatten()
            .filter_map(|key| {
                let rows = self.sorted_rows.get(&(ty.clone(), key.clone()))?;
                let (_, sort_key) = rows.iter().find(|(f, _)| f == field)?;
                Some((sort_key.clone(), key.clone()))
            })
            .filter(|(sort_key, _)| window.is_none_or(|w| w.holds(sort_key)))
            .collect();
        match dir {
            Direction::Asc => rows.sort(),
            Direction::Desc => rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1))),
        }
        let seek = from.map(|(k, key)| (k.to_vec(), RecordKey::from(key)));
        rows.into_iter()
            .filter(|row| match (&seek, dir) {
                (None, _) => true,
                (Some(f), Direction::Asc) => *row > *f,
                (Some(f), Direction::Desc) => row.0 < f.0 || (row.0 == f.0 && row.1 > f.1),
            })
            .take(n as usize)
            .collect()
    }

    /// At most `n` head records of `ty` whose `field` holds one of
    /// `tokens` in key order after `after`, the keys read once when
    /// several (`held`). The candidates the rulebook admits follow.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lookup_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        tokens: &[String],
        held: &mut Option<Vec<RecordKey>>,
        at: Option<Position>,
        after: Option<&str>,
        n: u32,
    ) -> Vec<StoredRecord> {
        let keys = held.get_or_insert_with(|| {
            let mut set = BTreeSet::new();
            for token in tokens {
                set.extend(
                    self.lookup
                        .get(&(ty.clone(), field.clone(), token.clone()))
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
            set.into_iter().collect()
        });
        let from = after.map_or(0, |a| keys.partition_point(|k| k.as_str() <= a));
        (keys[from..])
            .iter()
            .take(n as usize)
            .filter_map(|key| self.record_of(ty, key, at))
            .collect()
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
            Put::Record(r) | Put::Live(r) => {
                let (from, held) = (r.position, Held::of(&r, &mut self.bases));
                let changes = self.changes.entry(r.ty.clone()).or_default();
                let id = (r.ty.clone(), r.key.clone());
                // A kept name keeps its key's latest state alone (098).
                if lakelands_api::seam::is_kept_name(&r.ty) {
                    let was = self.records.remove(&id).unwrap_or_default();
                    for (at, _) in was.0 {
                        changes.remove(&(at, r.key.clone()));
                    }
                    if r.is_ended() {
                        return;
                    }
                }
                changes.insert((from, r.key.clone()));
                self.records.entry(id).or_default().put(from, held);
            }
            Put::Proposal { proposal, at } => {
                (self.by_position).insert(proposal.position, proposal.id.clone());
                self.proposals
                    .entry(proposal.id.clone())
                    .or_default()
                    .put(at, proposal);
            }
            Put::Quarantined { row, at } => {
                let at_key = (row.ty.clone(), row.key.clone());
                self.quarantined_at
                    .entry(at_key)
                    .or_default()
                    .insert(row.id.clone());
                (self.quarantined.entry(row.id.clone()).or_default()).put(at, *row);
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
            Put::ThinnedAct { act, at } => {
                let key = (act.ty.clone(), act.by.clone(), act.before);
                // A row already begun keeps the position it began at; each
                // part moves `through` and `gone` alone.
                self.thinned_acts
                    .entry(key)
                    .and_modify(|(_, row)| {
                        row.through = act.through;
                        row.gone = act.gone;
                    })
                    .or_insert((at, act));
            }
            Put::Thinned {
                ty,
                key,
                before,
                at,
            } => {
                self.thinned.entry((ty, key)).or_insert((before, at));
            }
            Put::Order { name, key, entry } => match entry {
                None => drop(self.orders.remove(&(name, key))),
                Some(entry) => drop(self.orders.insert((name, key), entry)),
            },
            Put::OrdersEnded { name } => {
                let from = (name.clone(), RecordKey::from(""));
                let of = self.orders.range(from..).map(|(id, _)| id);
                let ids: Vec<_> = of.take_while(|(t, _)| *t == name).cloned().collect();
                ids.iter().for_each(|id| drop(self.orders.remove(id)));
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
    /// flags, and inside `range` where the read narrowed by the key.
    pub(crate) fn records_after(
        &self,
        ty: &TypeName,
        at: Option<Position>,
        after: Option<&str>,
        range: Option<&KeyRange>,
        n: u32,
    ) -> Vec<StoredRecord> {
        use std::ops::Bound;
        let start = match after {
            Some(k) => Bound::Excluded((ty.clone(), RecordKey::from(k))),
            None => Bound::Included((ty.clone(), RecordKey::from(""))),
        };
        let (from, until) = match range {
            Some(range) => {
                let from = match &range.from {
                    Some(k) if after.is_none_or(|a| a < k.as_str()) => {
                        Bound::Included((ty.clone(), k.clone()))
                    }
                    _ => start,
                };
                let until = match &range.until {
                    Some(k) => Bound::Excluded((ty.clone(), k.clone())),
                    None => Bound::Unbounded,
                };
                (from, until)
            }
            None => (start, Bound::Unbounded),
        };
        // `BTreeMap::range` panics on an empty range whose ends meet
        // exactly and exclude it: a from past the until, or the until
        // itself, answers none before it is asked.
        let crossed = match (&from, &until) {
            (Bound::Included(a) | Bound::Excluded(a), Bound::Excluded(b)) => a >= b,
            _ => false,
        };
        if crossed {
            return Vec::new();
        }
        self.records
            .range((from, until))
            .take_while(|((t, _), _)| t == ty)
            .filter_map(|(_, h)| h.at(at))
            .take(n as usize)
            .map(|h| h.record(&self.bases))
            .collect()
    }
}

#[cfg(test)]
mod tests;
