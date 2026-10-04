//! Purging a source: every live record one source brought in, retired as
//! one act (`docs/decisions/039-a-source-is-retracted-as-one.md`).
//!
//! Who brought a record in is read from its per-field provenance, over
//! the fields that hold a value: a record belongs to the purge when one
//! of them came from the source. It is co-owned when a field that holds a value came from
//! another hand, a principal's authored write or another source; a
//! derived or system value (a computed field, a backed output, an aging
//! move) is the lake's own and makes no owner. One co-owned record
//! refuses the whole purge, and nothing is appended. A type that keeps no
//! per-field provenance cannot be read this way; it is not searched, and
//! the report names it.
//!
//! [`run`] holds the writer's lock for the whole purge and appends one
//! `record_retired` per record, each reacting as a `retire` does, in one
//! append: the act is one. That append is held in memory before it lands.

use std::collections::BTreeSet;

use lakelands_api::engine::ProjectionView;
use lakelands_api::record::{Origin, StoredRecord, Value};
use lakelands_api::write::{PurgeReference, PurgeReport, RecordRef, TypeRetired};
use lakelands_api::{
    FieldName, Position, RecordKey, RelationName, Result, SourceId, TypeName, WriteContext,
};

use lakelands_core::{invalid, links};

use crate::lake::{Lake, every};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::writes::{self, Call, Pending};

/// How many co-owned records a refusal names, and how many referring
/// records a report names per relation.
const NAMED: usize = 10;

/// The fields of a type that name a record, each with the type named.
type Names<'a> = Vec<(&'a FieldName, &'a TypeName)>;

/// Whether `r` came in, at least in part, from `source`: a field that
/// holds a value came from it. A record whose values from the source were
/// all cleared since holds none of it.
fn brought_by(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta
            .get(f)
            .is_some_and(|m| m.origin == Origin::Source && m.source.as_ref() == Some(source))
    })
}

/// Whether a field of `r` that holds a value came from another hand than
/// `source`: a principal's write, or another source. A field cleared since
/// holds no value and co-owns nothing.
fn co_owned(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta.get(f).is_some_and(|m| match m.origin {
            Origin::Authored => true,
            Origin::Source => m.source.as_ref() != Some(source),
            _ => false,
        })
    })
}

/// What a purge of `source` takes at `view`: its records, in type then
/// key order, and the types it could not search. One co-owned record
/// refuses it whole. The purge and its preflight both select here.
fn selection(
    view: &dyn ProjectionView,
    ontology: &Ontology,
    source: &SourceId,
) -> Result<(Vec<StoredRecord>, Vec<TypeName>)> {
    let (mut purged, mut unsearched, mut shared) = (Vec::new(), Vec::new(), Vec::new());
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl.view.is_some() {
            continue;
        }
        if !decl.field_meta {
            unsearched.push(name.clone());
            continue;
        }
        for r in view.scan(name, &every(false))? {
            if !brought_by(&r, source) {
                continue;
            }
            if co_owned(&r, source) {
                shared.push(format!("`{}` `{}`", r.ty, r.key));
            } else {
                purged.push(r);
            }
        }
    }
    if !shared.is_empty() {
        let n = shared.len();
        shared.truncate(NAMED);
        return Err(invalid(format!(
            "the purge of `{source}` is refused whole: {n} record(s) it brought in were also \
             written by another hand (a principal, or another source), and a purge never \
             half-removes a record; retire them or clear those fields, then purge again. \
             First: {}",
            shared.join(", ")
        )));
    }
    Ok((purged, unsearched))
}

/// The report of a purge that takes `purged`: what it retires per type,
/// and per relation the live records it does not take that refer into it,
/// by a `via` field or as a link record with an end inside. Each type
/// that can refer is read once, a page at a time.
fn report<'a>(
    view: &dyn ProjectionView,
    ontology: &'a Ontology,
    source: &SourceId,
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
    let inside = |target: &TypeName, v: Option<&Value>| {
        let key = v.and_then(Value::as_str).map(RecordKey::from);
        key.is_some_and(|k| taken.contains(&(target, &k)))
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
    Ok(PurgeReport {
        source: source.clone(),
        retired: purged.len() as u32,
        types,
        unsearched,
        position: view.position().unwrap_or(Position::GENESIS),
        references,
        report_id: String::new(),
    })
}

/// The id of the preflight of a purge of `source` read at `head`: the
/// report is a function of the two, so a purge that repeats it at
/// another head is stale.
fn id(source: &SourceId, head: Option<Position>) -> String {
    let head = head.map_or(0, |p| p.0);
    lakelands_core::canonical::sha256_hex(&serde_json::json!({"purge": source, "head": head}))
}

/// What [`run`] would do, appending nothing, under the id a purge may
/// repeat.
pub fn preflight(lake: &Lake, source: &SourceId) -> Result<PurgeReport> {
    let view = lake.engine.view(None)?;
    let ontology = Ontology::from_declared(&view.declarations()?);
    let (purged, unsearched) = selection(view.as_ref(), &ontology, source)?;
    let mut report = report(view.as_ref(), &ontology, source, &purged, unsearched)?;
    report.report_id = id(source, view.position());
    Ok(report)
}

/// Retire every live record `source` brought in, in one append; with a
/// preflight's `report_id`, only if the lake has not moved since.
pub fn run(lake: &Lake, source: &SourceId, report_id: Option<&str>) -> Result<PurgeReport> {
    let system = lake.system("lakelands");
    let call = Call {
        lake,
        who: &system,
        context: WriteContext::default(),
    };
    let mut w = lake.writer();
    let view = lake.engine.view(None)?;
    if report_id.is_some_and(|given| given != id(source, view.position())) {
        return Err(invalid(
            "the lake moved since that preflight report; run the preflight again",
        ));
    }
    let (purged, unsearched) = selection(view.as_ref(), &w.ontology, source)?;
    let mut report = report(view.as_ref(), &w.ontology, source, &purged, unsearched)?;
    let now = lake.clock(&mut w);
    let mut pending = Pending::default();
    let mut drafts = Vec::new();
    for before in purged {
        let body = writes::retirement(&before.ty, &before.key, before.version);
        let d = Draft::system(now, body);
        let at = writes::position_after(&w, drafts.len());
        drafts.extend(call.retiring(&w, view.as_ref(), &mut pending, &before, d, at, now)?);
    }
    drop(view);
    if !drafts.is_empty() {
        lake.append(&mut w, drafts)?;
    }
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    Ok(report)
}
