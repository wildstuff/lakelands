//! Purging a source: every live record one source brought in, retired as
//! one act (`docs/decisions/039-a-source-is-retracted-as-one.md`), in
//! parts beside the writer (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
//!
//! Who brought a record in is read from its per-field provenance, over
//! the fields that hold a value: a record belongs to the purge when one
//! of them came from the source. It is co-owned when another hand made it,
//! or when a field that holds a value came from another hand, a
//! principal's authored write or another source; a derived or system
//! value (a computed field, a backed output, an aging move) is the lake's
//! own and makes no owner, and neither does a field that declares an
//! `initial` value on a record the source made: the report's `moved`
//! counts the records taken with an author's write in one. Who made a
//! record is read from the write at its `made` position, as removing
//! values and forgetting read it. One co-owned record refuses the whole
//! purge, and nothing is appended. A type that keeps no
//! per-field provenance cannot be read this way; it is not searched, and
//! the report names it.
//!
//! A purge may name one type. It then reads that type and the link types
//! of the relations it declares, and judges co-ownership there alone: the
//! unit that is never split is the record, not the source. A type it
//! cannot purge from is refused, where the purge of a source whole passes
//! over it.
//!
//! **The act.** The walk judges on a view off the writer; the first turn
//! folds the tail past that view into the walk's selection under the
//! writer — what the source made since is taken in, and what another hand
//! has come to hold refuses the act — and appends the `source_purged`,
//! recording the act's position. The parts that follow take at most
//! `part_events` of the records still to retire, read again at their live
//! version as the part takes them, and append their retirements; an append
//! holds no more than `part_events` events, the last one's `act_whole`
//! among them, but for one record's retirements, which are never split.
//! The list [`PURGED`] says which acts are whole, so a writer's open finds
//! a purge a failure left and takes it up again: from the act's position,
//! retiring what of its selection is still live and appending the
//! `act_whole`.
//!
//! **The report's position.** A preflight's report names no head: its
//! `report_id` is the position the report was read at, then a digest of
//! the selection the purge would take, by the `made` position of each
//! record, up to that position. The act reads its selection again at that
//! position and digests it the same way; a selection that differs is a
//! report the log has moved past, and the purge is refused stale. What the
//! source made after that position is taken in — it is what the source
//! brought in — so the act is the source's purge as the log reads it when
//! the sentence begins. A write elsewhere names none of the selection and
//! leaves the digest as it was.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::{Case, RecordName};
use lakelands_api::event::{ChangeKind, Digest, Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::{Origin, StoredRecord, Value};
use lakelands_api::write::{PurgeMoved, PurgeReference, PurgeReport, RecordRef, TypeRetired};
use lakelands_api::{
    Error, FieldMeta, FieldName, Position, RecordKey, RelationName, Result, SourceId, Timestamp,
    TypeName, WriteContext,
};

use lakelands_core::links;

use crate::exec::Turn;
use crate::lake::{Lake, PAGE, Staged, Writer, every, owing_now};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::road::{Job, Reach, Turned};
use crate::writes;

/// How many co-owned records a refusal names, and how many referring
/// records a report names per relation.
const NAMED: usize = 10;

/// The list of purges, by the position of the act's first event: whether
/// each is whole, and what it is of. A list the writer keeps for itself,
/// under the `sys.` namespace as every such list is.
pub(crate) const PURGED: &str = "sys:purged";

/// One purge, as [`PURGED`] keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PurgedAct {
    pub source: SourceId,
    pub ty: Option<TypeName>,
    /// Whether the act's `act_whole` is in the log.
    pub whole: bool,
}

/// The digest [`PURGED`] is kept under.
pub(crate) fn purged_at() -> Digest {
    crate::backed::digest(&Value::from("purged"))
}

/// [`PURGED`] as it stands at `view`, with what this append wrote before
/// it.
pub(crate) fn purged_list(
    view: &dyn ProjectionView,
    staged: &Staged,
) -> Result<BTreeMap<Position, PurgedAct>> {
    let at = (PURGED.to_owned(), purged_at());
    let list = match staged.outputs.get(&at).and_then(|l| l.last()) {
        Some((_, v)) => Some(v.clone()),
        None => view.output(&at.0, &at.1)?.map(|(_, v)| v),
    };
    let all = list
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flatten();
    Ok(all
        .filter_map(|(p, v)| {
            let at = Position(p.parse().ok()?);
            let fields = v.as_object()?;
            let source = SourceId::from(fields.get("source")?.as_str()?);
            // A purge of the source whole names no type, and the list
            // leaves the field out: that is no entry to drop.
            let ty = fields
                .get("type")
                .and_then(Value::as_str)
                .map(TypeName::from);
            let whole = fields.get("whole")?.as_bool()?;
            Some((at, PurgedAct { source, ty, whole }))
        })
        .collect())
}

/// [`PURGED`] as a value.
pub(crate) fn purged_value(list: &BTreeMap<Position, PurgedAct>) -> Value {
    let all = list.iter().map(|(at, a)| {
        let mut fields = serde_json::Map::new();
        fields.insert("source".to_owned(), Value::from(a.source.as_str()));
        if let Some(ty) = &a.ty {
            fields.insert("type".to_owned(), Value::from(ty.as_str()));
        }
        fields.insert("whole".to_owned(), Value::Bool(a.whole));
        (at.0.to_string(), Value::Object(fields))
    });
    Value::Object(all.collect())
}

/// The fields of a type that name a record, each with the type named.
type Names<'a> = Vec<(&'a FieldName, &'a TypeName)>;

/// Whether `r` came in, at least in part, from `source`: a field that
/// holds a value came from it. A record whose values from the source were
/// all cleared since holds none of it.
pub(crate) fn brought_by(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta
            .get(f)
            .is_some_and(|m| m.origin == Origin::Source && m.source.as_ref() == Some(source))
    })
}

/// Whether a field of `r` that holds a value came from another hand than
/// the sources of `set`: a principal's write, or another source. A field
/// cleared since holds no value and co-owns nothing, and neither does one
/// that declares an `initial` value, whoever wrote it last: it is there
/// because the record is
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`). Who made
/// the record is [`made_by`]'s to say.
pub(crate) fn co_owned(decl: &TypeDecl, r: &StoredRecord, set: &[SourceId]) -> bool {
    let starts = |f: &FieldName| {
        decl.fields
            .iter()
            .any(|d| &d.name == f && d.initial.is_some())
    };
    r.fields.keys().filter(|f| !starts(f)).any(|f| {
        r.meta.get(f).is_some_and(|m| match m.origin {
            Origin::Authored => true,
            Origin::Source => !m.source.as_ref().is_some_and(|s| set.contains(s)),
            _ => false,
        })
    })
}

/// The `made` positions of those of `records` the sources of `set` made:
/// the write there was an ingest of one of them (069). A value one wrote
/// there that still stands says so without a read; the rest are read from
/// the log in one pass, a page at a time. A record another hand made is
/// never retired by taking a source back, whatever it holds now.
pub(crate) fn made_by(
    lake: &Lake,
    records: &[&StoredRecord],
    set: &[SourceId],
) -> Result<BTreeSet<Position>> {
    let ours = |s: Option<&SourceId>| s.is_some_and(|s| set.contains(s));
    let (mut made, mut unread) = (BTreeSet::new(), BTreeSet::new());
    for r in records {
        let wrote = |m: &FieldMeta| {
            m.position == r.made && m.origin == Origin::Source && ours(m.source.as_ref())
        };
        match r.meta.values().any(wrote) {
            true => {
                made.insert(r.made);
            }
            false => {
                unread.insert(r.made);
            }
        }
    }
    let unread: Vec<Position> = unread.into_iter().collect();
    for page in unread.chunks(PAGE as usize) {
        for e in lake.read_these(page)? {
            if matches!(e.body, EventBody::RecordIngested { .. }) && ours(e.source.as_ref()) {
                made.insert(e.position);
            }
        }
    }
    Ok(made)
}

/// The types a purge from `ty` reads: `ty` and the link types of the
/// relations of it that link records carry, whose records are nothing
/// without its own. A relation by a `via` field or derived has no link
/// type (044), and one that stands under its name from before is not
/// read. A type that holds no records, or cannot say who brought them
/// in, is refused.
fn named(ontology: &Ontology, ty: &TypeName) -> Result<BTreeSet<TypeName>> {
    let decl = ontology.ty(ty)?;
    if decl.view.is_some() {
        return Err(Error::invalid(Case::IsAView { ty: ty.clone() }));
    }
    if !decl.field_meta {
        return Err(Error::invalid(Case::NoProvenance { ty: ty.clone() }));
    }
    let carried = decl.relations.iter().filter(|r| r.carried_by_links());
    let links = carried.map(|r| links::link_type_name(ty, &r.name));
    Ok(links.chain([ty.clone()]).collect())
}

/// The types a purge reads and can take records from: those [`named`]
/// names, where one is named, and every type that keeps its provenance
/// otherwise. A write about a record of one of them is what the tail can
/// move.
fn scope_of(ontology: &Ontology, ty: Option<&TypeName>) -> Result<BTreeSet<TypeName>> {
    let only = ty.map(|t| named(ontology, t)).transpose()?;
    Ok(ontology
        .types
        .iter()
        .filter(|(name, entry)| {
            entry.decl.view.is_none()
                && entry.decl.field_meta
                && only.as_ref().is_none_or(|o| o.contains(*name))
        })
        .map(|(name, _)| name.clone())
        .collect())
}

/// The stretch of the log a purge takes: the whole selection, or the
/// selection up to a report's position, or up to an act's first event.
#[derive(Clone, Debug)]
enum Window {
    /// Every record the source brought in, as the act writes without a
    /// report.
    Whole,
    /// A preflight's position, and the digest of the selection up to it.
    Report { through: Position, digest: String },
    /// The position of the act's first event: a purge a writer's open
    /// takes up again.
    Act { at: Position },
}

/// A record the window's selection would have had, and the position of
/// the write that made it: at or before a report's position, which makes
/// the report stale, or after it, which is the act's own selection grown
/// and answers shared as a purge without an id does.
#[derive(Clone, Debug)]
struct Lost {
    name: RecordName,
    made: Position,
}

/// What a purge takes at a view: the records, the types it could not
/// search, and the records the window's selection would have had but
/// that another hand now holds.
struct Settled {
    taken: Vec<StoredRecord>,
    unsearched: Vec<TypeName>,
    /// The `made` positions of the records taken that were made inside
    /// the window, which the digest is of.
    made: BTreeSet<Position>,
    /// The records the report named that another hand now holds or that
    /// have gone.
    lost: Vec<Lost>,
    /// The records the source made after the report's position that
    /// another hand has come to hold: they are not the report's, and the
    /// act's selection is shared.
    shared: Vec<RecordName>,
}

impl Lost {
    fn of(r: &StoredRecord) -> Lost {
        Lost {
            name: RecordName::Of {
                ty: r.ty.clone(),
                key: r.key.clone(),
            },
            made: r.made,
        }
    }
}

/// What a purge takes at `view`, in type then key order, judged in
/// `window`.
///
/// A report's window is judged twice: the selection the report named is
/// read from the log as of the report's position, which is what its
/// digest is of, and the selection the act takes is read at the act's
/// view, which holds what the source made since and what another hand
/// has come to hold. A record the report named that is now shared, or
/// gone, makes the report stale; a record the source made since is taken
/// in.
fn settled(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    (source, ty): (&SourceId, Option<&TypeName>),
    window: &Window,
) -> Result<Settled> {
    if let Window::Report { through, .. } = window {
        // An id that names a position the log has not reached is no
        // report this lake can read: stale, not the engine's out-of-range.
        if *through > view.position().unwrap_or(Position::GENESIS) {
            return Err(Error::invalid(Case::PurgeReportStale));
        }
        let at = lake.engine.view(Some(*through))?;
        let past = settle_at(lake, at.as_ref(), ontology, (source, ty), &|_: Position| {
            true
        })?;
        let now = settle_at(lake, view, ontology, (source, ty), &|_: Position| true)?;
        let live: BTreeSet<(&TypeName, &RecordKey)> =
            now.taken.iter().map(|r| (&r.ty, &r.key)).collect();
        let mut lost = past.lost.clone();
        lost.extend(now.lost.iter().filter(|l| l.made <= *through).cloned());
        lost.extend(
            past.taken
                .iter()
                .filter(|r| !live.contains(&(&r.ty, &r.key)))
                .map(Lost::of),
        );
        let shared = (now.lost.into_iter())
            .filter(|l| l.made > *through)
            .map(|l| l.name)
            .collect();
        return Ok(Settled {
            taken: now.taken,
            unsearched: now.unsearched,
            made: past.made,
            lost,
            shared,
        });
    }
    let inside = |at: Position| match window {
        Window::Whole => true,
        // Not an act's, where the act is in the log.
        Window::Act { at: act } => at < *act,
        Window::Report { .. } => true,
    };
    settle_at(lake, view, ontology, (source, ty), &inside)
}

/// What a purge takes at `view`, in type then key order, of the records
/// whose `made` position `inside` admits.
fn settle_at(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    (source, ty): (&SourceId, Option<&TypeName>),
    inside: &dyn Fn(Position) -> bool,
) -> Result<Settled> {
    let only = ty.map(|t| named(ontology, t)).transpose()?;
    let mut out = Settled {
        taken: Vec::new(),
        unsearched: Vec::new(),
        made: BTreeSet::new(),
        lost: Vec::new(),
        shared: Vec::new(),
    };
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl.view.is_some() || only.as_ref().is_some_and(|o| !o.contains(name)) {
            continue;
        }
        if !decl.field_meta {
            out.unsearched.push(name.clone());
            continue;
        }
        let set = std::slice::from_ref(source);
        let brought: Vec<StoredRecord> = (view.scan(name, &every(false))?.into_iter())
            .filter(|r| brought_by(r, source))
            .collect();
        let alone: Vec<&StoredRecord> =
            brought.iter().filter(|r| !co_owned(decl, r, set)).collect();
        let made = made_by(lake, &alone, set)?;
        for r in brought {
            let takeable = !co_owned(decl, &r, set) && made.contains(&r.made);
            match (inside(r.made), takeable) {
                (true, true) => {
                    out.made.insert(r.made);
                    out.taken.push(r);
                }
                (true, false) => out.lost.push(Lost::of(&r)),
                (false, _) => {}
            }
        }
    }
    Ok(out)
}

/// The refusal a purge's judgement owes at `window`, and the digest of
/// the selection for a report.
fn judged(
    out: &Settled,
    (source, ty): (&SourceId, Option<&TypeName>),
    window: &Window,
) -> Result<()> {
    match window {
        Window::Whole => {
            let mut first: Vec<RecordName> = (out.lost.iter().map(|l| l.name.clone())).collect();
            first.extend(out.shared.iter().cloned());
            if first.is_empty() {
                Ok(())
            } else {
                let count = first.len();
                first.truncate(NAMED);
                Err(Error::invalid(Case::PurgeShared {
                    source: source.clone(),
                    ty: ty.cloned(),
                    count,
                    first,
                }))
            }
        }
        Window::Report { digest, .. } => {
            let ours = digest_of(source, ty, &out.made);
            if !out.lost.is_empty() || ours != *digest {
                return Err(Error::invalid(Case::PurgeReportStale));
            }
            if out.shared.is_empty() {
                return Ok(());
            }
            shared(source, ty, out.shared.clone())
        }
        Window::Act { .. } => Ok(()),
    }
}

/// The refusal of a purge whose selection another hand holds too.
fn shared(source: &SourceId, ty: Option<&TypeName>, mut first: Vec<RecordName>) -> Result<()> {
    let count = first.len();
    first.truncate(NAMED);
    Err(Error::invalid(Case::PurgeShared {
        source: source.clone(),
        ty: ty.cloned(),
        count,
        first,
    }))
}

/// The digest of a selection the purge takes, by the `made` position of
/// each record it retires: what a report names the position of, and what
/// the act compares at that position.
fn digest_of(source: &SourceId, ty: Option<&TypeName>, made: &BTreeSet<Position>) -> String {
    let made: Vec<u64> = made.iter().map(|p| p.0).collect();
    let of = serde_json::json!({"purge": source, "type": ty, "made": made});
    lakelands_core::canonical::sha256_hex(&of)
}

/// A preflight's `report_id`: the position the report was read at, then
/// the digest of the selection up to it.
fn token(
    source: &SourceId,
    ty: Option<&TypeName>,
    through: Position,
    made: &BTreeSet<Position>,
) -> String {
    format!("{}-{}", through.0, digest_of(source, ty, made))
}

/// The window a `report_id` names; a string that is not one is a report
/// of no purge this lake reads.
fn window_of(id: &str) -> Result<Window> {
    let (through, digest) = id
        .split_once('-')
        .ok_or_else(|| Error::invalid(Case::PurgeReportStale))?;
    let through: u64 = through
        .parse()
        .map_err(|_| Error::invalid(Case::PurgeReportStale))?;
    Ok(Window::Report {
        through: Position(through),
        digest: digest.to_owned(),
    })
}

/// The report of a purge that takes `purged`: what it retires per type,
/// and per relation the live records it does not take that refer into it,
/// by a `via` field or as a link record with an end inside. Each type
/// that can refer is read once, a page at a time.
pub(crate) fn report<'a>(
    view: &dyn ProjectionView,
    ontology: &'a Ontology,
    (source, ty): (&SourceId, Option<&TypeName>),
    purged: &[StoredRecord],
    unsearched: Vec<TypeName>,
) -> Result<PurgeReport> {
    let taken: BTreeSet<(&TypeName, &RecordKey)> = purged.iter().map(|r| (&r.ty, &r.key)).collect();
    let mut types: Vec<TypeRetired> = Vec::new();
    for r in purged {
        match types.last_mut() {
            Some(t) if t.ty == r.ty => t.retired += 1,
            _ => types.push(TypeRetired {
                ty: r.ty.clone(),
                retired: 1,
            }),
        }
    }
    // What an author wrote in a record the purge takes is a field with an
    // `initial` value: any other would have refused the purge.
    let mut moved: BTreeMap<(&TypeName, &FieldName), PurgeMoved> = BTreeMap::new();
    for r in purged {
        let authored =
            |f: &&FieldName| r.meta.get(*f).is_some_and(|m| m.origin == Origin::Authored);
        for f in r.fields.keys().filter(authored) {
            let m = moved.entry((&r.ty, f)).or_insert_with(|| PurgeMoved {
                ty: r.ty.clone(),
                field: f.clone(),
                records: 0,
                first: Vec::new(),
            });
            m.records += 1;
            if m.first.len() < NAMED {
                m.first.push(r.key.clone());
            }
        }
    }
    // A field names a record the purge takes alone or as any element of
    // its array, and its record is counted once however many it names.
    let inside = |target: &TypeName, v: Option<&Value>| {
        let keys = v.map(lakelands_core::engines::keys_of).unwrap_or_default();
        keys.into_iter()
            .any(|k| taken.contains(&(target, &RecordKey::from(k))))
    };
    let reached = |target: &TypeName| types.iter().any(|t| &t.ty == target);
    let mut references = Vec::new();
    for (name, entry) in &ontology.types {
        // What names a record here: a relation's `via` field, or, in a
        // link type, the link's two ends. Only what can name a record
        // the purge takes is read.
        let decl = &entry.decl;
        let mut found: Vec<(PurgeReference, Names<'a>)> = Vec::new();
        let mut names = |ty: TypeName, relation: RelationName, fields: Names<'a>| {
            if fields.iter().any(|(_, target)| reached(target)) {
                let at = PurgeReference {
                    ty,
                    relation,
                    records: 0,
                    first: Vec::new(),
                };
                found.push((at, fields));
            }
        };
        if let Some((owner, relation)) = links::link_type_parts(name) {
            let ends = (decl.fields.iter())
                .filter(|f| [links::FROM, links::TO].contains(&f.name.as_str()))
                .filter_map(|f| Some((&f.name, f.ty.target.as_ref()?)));
            names(owner, relation, ends.collect());
        }
        for r in &decl.relations {
            if let Some(via) = &r.via {
                names(name.clone(), r.name.clone(), vec![(via, &r.target)]);
            }
        }
        if found.is_empty() {
            continue;
        }
        writes::live(view, name, |r| {
            if taken.contains(&(name, &r.key)) {
                return Ok(());
            }
            for (at, fields) in &mut found {
                if fields.iter().any(|(f, t)| inside(t, r.fields.get(*f))) {
                    at.records += 1;
                    if at.first.len() < NAMED {
                        at.first.push(RecordRef {
                            ty: r.ty.clone(),
                            key: r.key.clone(),
                        });
                    }
                }
            }
            Ok(())
        })?;
        references.extend(found.into_iter().map(|f| f.0).filter(|f| f.records > 0));
    }
    references.sort_by(|a, b| (&a.ty, &a.relation).cmp(&(&b.ty, &b.relation)));
    let (_, quarantined) = closes(view, source, ty)?;
    Ok(PurgeReport {
        source: source.clone(),
        ty: ty.cloned(),
        retired: purged.len() as u32,
        types,
        unsearched,
        position: view.position().unwrap_or(Position::GENESIS),
        references,
        moved: moved.into_values().collect(),
        report_id: String::new(),
        set_off: 0,
        max_set_changes: 0,
        quarantined,
    })
}

/// The closes of the open quarantined records of `source`, of `ty` alone
/// where one is named, every one of which a purge closes (085), and their
/// count per type.
fn closes(
    view: &dyn ProjectionView,
    source: &SourceId,
    ty: Option<&TypeName>,
) -> Result<(
    Vec<EventBody>,
    Vec<lakelands_api::quarantine::TypeQuarantined>,
)> {
    let why = lakelands_api::quarantine::ClosedWhy::Purged;
    crate::quarantine::act_closes(view, (source, ty), why, &mut |_| Ok(true))
}

/// What [`start`] would do, appending nothing: under the `report_id` a
/// purge may repeat, with what its retirements would set off across an
/// edge, past the ceiling too, so a host learns before the act that it
/// must narrow a set or open the lake with a higher `max_set_changes`.
pub fn preflight(lake: &Lake, source: &SourceId, ty: Option<&TypeName>) -> Result<PurgeReport> {
    let view = lake.engine.view(None)?;
    let w = lake.reading(view.as_ref())?;
    let window = Window::Whole;
    let out = settled(lake, view.as_ref(), &w.ontology, (source, ty), &window)?;
    judged(&out, (source, ty), &window)?;
    let mut report = report(
        view.as_ref(),
        &w.ontology,
        (source, ty),
        &out.taken,
        out.unsearched,
    )?;
    let now = w.last_known_at;
    let (_, set_off) = retirements(lake, (&w, view.as_ref()), out.taken, now, false)?;
    (report.set_off, report.max_set_changes) = (set_off, lake.limits.max_set_changes);
    let through = view.position().unwrap_or(Position::GENESIS);
    report.report_id = token(source, ty, through, &out.made);
    Ok(report)
}

/// The drafts that retire `purged`, each with what it sets off, as the
/// system writes them, and how many memberships and firings they set off
/// across an edge; held to `max_set_changes` where `bound`.
fn retirements(
    lake: &Lake,
    (w, view): (&Writer, &dyn ProjectionView),
    purged: Vec<StoredRecord>,
    now: Timestamp,
    bound: bool,
) -> Result<(Vec<Draft>, u32)> {
    let system = lake.system("lakelands");
    let asked = writes::Asked {
        lake,
        who: &system,
        context: &WriteContext::default(),
    };
    let mut d = asked.decide(w, view, now, 0);
    d.bound = bound;
    d.retiring_as = ChangeKind::Purged;
    let mut drafts = Vec::new();
    for before in purged {
        let body = writes::retirement(&before.ty, &before.key, before.version);
        drafts.extend(d.retiring(&before, Draft::system(now, body))?);
    }
    Ok((drafts, d.pending.across as u32))
}

/// The closes of the act's first turn that fit in `room` beside its
/// `source_purged`; the rest stay owed to the parts.
fn take_closes(closes: &mut VecDeque<EventBody>, room: usize) -> Vec<EventBody> {
    let fit = room.saturating_sub(1).min(closes.len());
    closes.drain(..fit).collect()
}

/// The purges the log and [`PURGED`] leave owed: every act that is not
/// whole, taken up again by the next writer's open.
pub(crate) fn owed(_lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let owing: Vec<Box<dyn Job>> = (purged_list(view, &Staged::default())?.into_iter())
        .filter(|(_, act)| !act.whole)
        .map(|(at, act)| {
            Box::new(Purge {
                source: act.source,
                ty: act.ty,
                report_id: None,
                window: Window::Act { at },
                scope: BTreeSet::new(),
                taken: BTreeMap::new(),
                moved: BTreeSet::new(),
                named: BTreeSet::new(),
                closes: VecDeque::new(),
                closed: false,
                lost: Vec::new(),
                shared: Vec::new(),
                report: None,
                act: Some(at),
                position: at,
                own: BTreeSet::new(),
                answer: None,
            }) as Box<dyn Job>
        })
        .collect();
    Ok(owing)
}

/// What a purge answers its caller: the report of the act, and the
/// positions it wrote, whose backed fields settle after it.
pub(crate) type Reported = Result<(PurgeReport, BTreeSet<Position>)>;

/// One purge on the road: [`preflight`]'s report repeated under `report_id`,
/// or the act of a purge that names none, or an act [`owed`] found.
pub(crate) struct Purge {
    source: SourceId,
    ty: Option<TypeName>,
    report_id: Option<String>,
    window: Window,
    /// The types the act reads: a write about a record of one of them is
    /// what the tail can move.
    scope: BTreeSet<TypeName>,
    /// The records the walk judged takeable and still to retire, in type
    /// then key order, as they stood at the walk.
    taken: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// The keys the tail moved past the walk, read again under the writer
    /// in the first turn.
    moved: BTreeSet<(TypeName, RecordKey)>,
    /// The `made` positions of the records the report's selection held,
    /// which its digest is of: a loss of one of them stales the report.
    named: BTreeSet<Position>,
    /// The closes the act owes, in the order the walk found them.
    closes: VecDeque<EventBody>,
    /// Whether the act's closes were sought under the writer: a resume
    /// finds the source's open quarantines where the act left them.
    closed: bool,
    /// What the tail took from the walk's selection: the records the
    /// report named that went or another hand now holds or shares.
    lost: Vec<Lost>,
    /// What the source made after the report's position that another hand
    /// has come to hold: shared records of the act's own selection.
    shared: Vec<RecordName>,
    report: Option<PurgeReport>,
    /// The position of the act's first event; `None` until it is in the log.
    act: Option<Position>,
    /// The last position the act wrote.
    position: Position,
    own: BTreeSet<Position>,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
}

impl Job for Purge {
    /// What of the tail reaches the act: its source's writes, and every
    /// write about a record of a type it reads, which another hand's
    /// write on one of its records is.
    fn reach(&self) -> Reach {
        Reach {
            sources: vec![self.source.clone()],
            types: self.scope.iter().cloned().collect(),
            keys: BTreeSet::new(),
        }
    }

    /// The walk, off the writer: judge the selection at the view, and hold
    /// it away from the projection's own.
    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        // An act the log already began, for this purge, is taken up
        // again, whatever report the caller names.
        if let Some(at) = self.owing(view)? {
            self.window = Window::Act { at };
            self.act = Some(at);
        } else if let Some(id) = &self.report_id {
            self.window = window_of(id)?;
        }
        self.judge(lake, view)
    }

    /// The keys of the records the tail wrote about, which the first turn
    /// reads again under the writer and folds into the walk's selection
    /// (099, "The tail moves the keys").
    fn fold(&mut self, tail: &[Event]) -> Result<()> {
        for e in tail {
            for about in lakelands_core::feed::about(&e.body) {
                if let (Some(ty), Some(key)) = (about.ty, about.key)
                    && self.scope.contains(&ty)
                {
                    self.moved.insert((ty, key));
                }
            }
        }
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let turned = self.one_turn(t, w)?;
        // What the turn's append and the catch-up the writer took before
        // it left owed, which its caller settles.
        self.own.extend(owing_now());
        Ok(turned)
    }

    fn end(&mut self, ended: Result<()>) {
        let Some(answer) = self.answer.take() else {
            return;
        };
        let answered = ended.map(|()| {
            let report = self.report.take().expect("a whole purge has its report");
            (report, std::mem::take(&mut self.own))
        });
        let _ = answer.send(answered);
    }
}

impl Purge {
    /// An act the log began for this purge and has not ended, if there is
    /// one: the walk and the first turn both look, since a projection the
    /// writer had not caught up yet hides it from the walk.
    fn owing(&self, view: &dyn ProjectionView) -> Result<Option<Position>> {
        Ok(purged_list(view, &Staged::default())?
            .into_iter()
            .find(|(_, act)| !act.whole && act.source == self.source && act.ty == self.ty)
            .map(|(at, _)| at))
    }

    /// Judge the selection in `self.window` at `view`; the report and the
    /// records to retire follow. Off the writer, as the walk is: the
    /// ceiling over the selection is judged here, and each part is held to
    /// it as any append is.
    fn judge(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let ontology = Ontology::at(view)?;
        self.scope = scope_of(&ontology, self.ty.as_ref())?;
        let out = settled(
            lake,
            view,
            &ontology,
            (&self.source, self.ty.as_ref()),
            &self.window,
        )?;
        judged(&out, (&self.source, self.ty.as_ref()), &self.window)?;
        let mut report = report(
            view,
            &ontology,
            (&self.source, self.ty.as_ref()),
            &out.taken,
            out.unsearched.clone(),
        )?;
        if !out.taken.is_empty() {
            // What the retirements set off across an edge, all of it, is
            // judged before anything is appended: past the ceiling the act
            // is refused whole, naming the field that crossed it.
            let reading = lake.reading(view)?;
            let now = reading.last_known_at;
            let (_, set_off) = retirements(lake, (&reading, view), out.taken.clone(), now, true)?;
            (report.set_off, report.max_set_changes) = (set_off, lake.limits.max_set_changes);
        }
        self.report = Some(report);
        self.taken = (out.taken.into_iter())
            .map(|r| ((r.ty.clone(), r.key.clone()), r))
            .collect();
        self.named = out.made;
        self.lost.clear();
        self.shared.clear();
        Ok(())
    }

    /// The tail past the walk's view, folded into its selection under the
    /// writer: the keys it wrote are read again as they stand. What the
    /// source made since is taken in; a record the report named that went,
    /// or that another hand now holds, stales the report; one the source
    /// made after it and another hand now holds is shared.
    fn see_the_tail(
        &mut self,
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
    ) -> Result<()> {
        for (ty, key) in std::mem::take(&mut self.moved) {
            let before = self.taken.remove(&(ty.clone(), key.clone()));
            let live = view.get(&ty, &key)?;
            let named = match (&self.window, &before) {
                (Window::Report { .. }, Some(b)) => self.named.contains(&b.made).then_some(b.made),
                _ => None,
            };
            let Some(live) = live else {
                // Gone since the walk. One the report named has gone, so
                // the report is stale; what a hand made at the key since
                // is nothing to the act.
                if named.is_some() {
                    self.lost.extend(before.as_ref().map(Lost::of));
                }
                continue;
            };
            if let (Some(made), Some(b)) = (named, &before)
                && live.made != made
            {
                // The record the report named went, and a new one stands
                // at its key.
                self.lost.push(Lost::of(b));
                continue;
            }
            let decl = ontology.ty(&ty)?;
            let set = std::slice::from_ref(&self.source);
            let takeable = brought_by(&live, &self.source)
                && !co_owned(decl, &live, set)
                && (made_by(lake, std::slice::from_ref(&&live), set)?.contains(&live.made));
            let inside = match &self.window {
                Window::Act { at } => live.made < *at,
                Window::Whole | Window::Report { .. } => true,
            };
            if !inside {
                // Past an act's first event: not what the act takes up.
                continue;
            }
            if takeable {
                self.taken.insert((ty, key), live);
                continue;
            }
            let ours = before.is_some() || brought_by(&live, &self.source);
            if !ours {
                continue;
            }
            match &self.window {
                Window::Whole => self.lost.push(Lost::of(&live)),
                Window::Report { .. } if named.is_some() => self.lost.push(Lost::of(&live)),
                Window::Report { .. } => {
                    let name = RecordName::Of {
                        ty: live.ty.clone(),
                        key: live.key.clone(),
                    };
                    self.shared.push(name);
                }
                Window::Act { .. } => {}
            }
        }
        Ok(())
    }

    /// The refusal the tail's losses and shares owe, before anything is
    /// appended.
    fn refuse(&self) -> Result<()> {
        match &self.window {
            Window::Report { .. } => {
                if !self.lost.is_empty() {
                    return Err(Error::invalid(Case::PurgeReportStale));
                }
                if self.shared.is_empty() {
                    Ok(())
                } else {
                    shared(&self.source, self.ty.as_ref(), self.shared.clone())
                }
            }
            Window::Whole if !self.lost.is_empty() || !self.shared.is_empty() => {
                let mut first: Vec<RecordName> =
                    (self.lost.iter().map(|l| l.name.clone())).collect();
                first.extend(self.shared.iter().cloned());
                shared(&self.source, self.ty.as_ref(), first)
            }
            Window::Whole | Window::Act { .. } => Ok(()),
        }
    }

    /// One part: the closes the act still owes and the selection's next
    /// records, each read again as it stands now, as one append of at most
    /// `room` events, with `act_whole` in it where nothing is left and it
    /// fits. A record another hand wrote since keeps its record; one gone
    /// is dropped (099, "What stands between parts"). One record's
    /// retirements are one append's, however many of them it takes.
    fn part(
        &mut self,
        lake: &Lake,
        (w, view): (&Writer, &dyn ProjectionView),
        now: Timestamp,
        room: usize,
    ) -> Result<(Vec<Draft>, bool)> {
        let system = lake.system("lakelands");
        let asked = writes::Asked {
            lake,
            who: &system,
            context: &WriteContext::default(),
        };
        let mut drafts = Vec::new();
        while drafts.len() < room {
            let Some(body) = self.closes.pop_front() else {
                break;
            };
            drafts.push(Draft::system(now, body));
        }
        // The retirements are caused by their own position in the append,
        // after the closes the part carries in front of them.
        let mut d = asked.decide(w, view, now, drafts.len());
        d.bound = true;
        d.retiring_as = ChangeKind::Purged;
        while drafts.len() < room {
            let Some((_, before)) = self.taken.first_key_value() else {
                break;
            };
            let before = before.clone();
            let live = match view.get(&before.ty, &before.key)? {
                Some(live) if live.version == before.version => live,
                _ => {
                    self.taken.remove(&(before.ty, before.key));
                    continue;
                }
            };
            let body = writes::retirement(&live.ty, &live.key, live.version);
            let written = d.retiring(&live, Draft::system(now, body))?;
            if !drafts.is_empty() && drafts.len() + written.len() > room {
                break;
            }
            self.taken.remove(&(live.ty.clone(), live.key.clone()));
            drafts.extend(written);
        }
        let ends = self.taken.is_empty() && self.closes.is_empty();
        let whole = ends && drafts.len() < room;
        if whole {
            let act = self.act.expect("a part follows the act's beginning");
            drafts.push(Draft::system(now, EventBody::ActWhole { act }));
        }
        Ok((drafts, whole))
    }

    /// One turn beside the writer, as [`Job::turn`] describes: the act's
    /// beginning, a part of its retirements, or its end.
    fn one_turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let lake = t;
        let view = lake.engine.view(None)?;
        // The writer's catch-up may have left the projection holding an act
        // the walk off it did not see: take that act up before beginning
        // another.
        if self.act.is_none()
            && let Some(at) = self.owing(view.as_ref())?
        {
            self.window = Window::Act { at };
            self.act = Some(at);
            self.judge(lake, view.as_ref())?;
        }
        if self.act.is_none() {
            // The first turn: fold the tail the walk did not see into its
            // selection, judge what it moved, and append the act's
            // beginning. A purge that takes nothing appends nothing.
            self.see_the_tail(lake, view.as_ref(), &w.ontology)?;
            self.refuse()?;
            let now = lake.clock(w);
            let (shut, _) = closes(view.as_ref(), &self.source, self.ty.as_ref())?;
            self.closes.extend(shut);
            self.closed = true;
            if self.taken.is_empty() && self.closes.is_empty() {
                // Nothing to retire and nothing to close: nothing to
                // append, and the report stands at the head.
                let mut report = self.report.take().expect("the walk made the report");
                report.position = view.position().unwrap_or(Position::GENESIS);
                self.report = Some(report);
                return Ok(Turned::Whole);
            }
            let room = usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX);
            let act = writes::position_after(w, 0);
            let mut drafts = vec![Draft::system(
                now,
                EventBody::SourcePurged {
                    source: self.source.clone(),
                    ty: self.ty.clone(),
                },
            )];
            let fit = take_closes(&mut self.closes, room);
            drafts.extend(fit.into_iter().map(|c| Draft::system(now, c)));
            let ends = self.taken.is_empty() && self.closes.is_empty();
            let whole = ends && drafts.len() < room;
            if whole {
                drafts.push(Draft::system(now, EventBody::ActWhole { act }));
            }
            drop(view);
            lake.append(&mut *w, drafts)?;
            self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
            self.act = Some(act);
            if whole {
                let mut report = self.report.take().expect("the walk made the report");
                report.position = self.position;
                self.report = Some(report);
                return Ok(Turned::Whole);
            }
            return Ok(Turned::More);
        }
        // Another job of the same lake may have finished the act while this
        // one waited: then it appends nothing.
        if let Some(at) = self.act
            && purged_list(view.as_ref(), &Staged::default())?
                .get(&at)
                .is_some_and(|act| act.whole)
        {
            let mut report = self.report.take().expect("a whole purge has its report");
            report.position = view.position().unwrap_or(Position::GENESIS);
            self.report = Some(report);
            self.taken.clear();
            return Ok(Turned::Whole);
        }
        // An act taken up again — owed in the log, or handed over by the
        // writer's catch-up — closes what its source still has open: its
        // first append carried only the closes that fit, and a writer that
        // stopped took the rest with it.
        if !self.closed {
            let (shut, _) = closes(view.as_ref(), &self.source, self.ty.as_ref())?;
            self.closes.extend(shut);
            self.closed = true;
        }
        // The part in hand: the closes still owed and the selection's next
        // records, read again as the part takes them, in one append of at
        // most `part_events`.
        let now = lake.clock(w);
        let room = usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX);
        let (drafts, whole) = self.part(lake, (w, view.as_ref()), now, room)?;
        drop(view);
        lake.append(&mut *w, drafts)?;
        self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        if whole {
            let mut report = self.report.take().expect("an act's part has its report");
            report.position = self.position;
            self.report = Some(report);
            return Ok(Turned::Whole);
        }
        Ok(Turned::More)
    }
}

/// Send a purge of `source`, of `ty` alone where one is named, along the
/// lake's road, answering `answer` where the caller waits: the store's
/// door, which holds no writer lock while the act runs.
pub(crate) fn start(
    lake: &std::sync::Arc<Lake>,
    source: SourceId,
    ty: Option<TypeName>,
    report_id: Option<String>,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
) {
    let job = Purge {
        source,
        ty,
        report_id,
        window: Window::Whole,
        scope: BTreeSet::new(),
        taken: BTreeMap::new(),
        moved: BTreeSet::new(),
        named: BTreeSet::new(),
        closes: VecDeque::new(),
        closed: false,
        lost: Vec::new(),
        shared: Vec::new(),
        report: None,
        act: None,
        position: Position::GENESIS,
        own: BTreeSet::new(),
        answer,
    };
    crate::road::send(lake, Box::new(job));
}
