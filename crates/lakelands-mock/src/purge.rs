//! Purging a source over the mock's log: the store's rule for whose
//! records they are, its refusal of a co-owned record, and one append of
//! `record_retired` events; from one type where the purge names one, by
//! the store's rule for what that reads. The mock computes no reactions.

use lakelands_api::error::{Error, Missing, Refusal, Result};
use lakelands_api::event::EventBody;
use lakelands_api::id::{FieldName, Position, SourceId, TypeName, Version};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{PurgeMoved, PurgeReference, PurgeReport, RecordRef, TypeRetired};

use crate::state::State;
use crate::{Batch, Draft, Log, Shared};

fn brought_by(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta
            .get(f)
            .is_some_and(|m| m.origin == Origin::Source && m.source.as_ref() == Some(source))
    })
}

/// As the store (050): a field that declares an `initial` value makes no
/// owner, whoever wrote it last.
fn co_owned(decl: &TypeDecl, r: &StoredRecord, source: &SourceId) -> bool {
    let starts = |f: &FieldName| crate::state::field(decl, f).is_some_and(|d| d.initial.is_some());
    r.fields.keys().filter(|f| !starts(f)).any(|f| {
        r.meta.get(f).is_some_and(|m| match m.origin {
            Origin::Authored => true,
            Origin::Source => m.source.as_ref() != Some(source),
            _ => false,
        })
    })
}

/// How many records a refusal or a report names.
const NAMED: usize = 10;

/// The id of the preflight of a purge of `source`, from `ty` where one is
/// named, read at `head`: it fits no other purge and no other head, as
/// the store's does not.
fn id(source: &SourceId, ty: Option<&TypeName>, head: Position) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("purge {source} from {ty:?} at {}", head.0));
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(message: String) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid { message },
    }
}

/// The types a purge from `ty` reads: `ty` and the link types of the
/// relations it declares. A type that is not declared is not found; a
/// view, and a type that keeps no per-field provenance, are refused.
fn named(st: &State, ty: &TypeName) -> Result<Vec<TypeName>> {
    let missing = Missing::Type { name: ty.clone() };
    let decl = &st.types.get(ty).ok_or(Error::NotFound { missing })?.decl;
    if decl.view.is_some() {
        return Err(invalid(format!(
            "`{ty}` is a view; it holds no records a purge could take"
        )));
    }
    if !decl.field_meta {
        return Err(invalid(format!(
            "`{ty}` keeps no per-field provenance; a purge cannot read who brought its records in"
        )));
    }
    let links = (decl.relations.iter()).map(|r| crate::links::name(ty, &r.name));
    Ok(links.chain([ty.clone()]).collect())
}

/// What a purge of `source` would do, from `ty` alone where one is named,
/// by the store's rules: the records it takes, refused whole for a
/// co-owned one, and per relation the live records it does not take that
/// refer into it, by a `via` field or as a link record with an end inside.
fn plan<'s>(
    st: &'s State,
    head: Position,
    source: &SourceId,
    ty: Option<&TypeName>,
) -> Result<(Vec<&'s StoredRecord>, PurgeReport)> {
    let only = ty.map(|t| named(st, t)).transpose()?;
    let mut report = PurgeReport {
        moved: Vec::new(),
        source: source.clone(),
        ty: ty.cloned(),
        retired: 0,
        types: Vec::new(),
        unsearched: Vec::new(),
        position: head,
        references: Vec::new(),
        report_id: String::new(),
    };
    let mut purged = Vec::new();
    let mut others = Vec::new();
    for (name, t) in &st.types {
        if t.decl.view.is_some() || only.as_ref().is_some_and(|o| !o.contains(name)) {
            continue;
        }
        if !t.decl.field_meta {
            report.unsearched.push(name.clone());
            continue;
        }
        for r in st
            .records
            .values()
            .filter(|r| &r.ty == name && !r.retired && brought_by(r, source))
        {
            if co_owned(&t.decl, r, source) {
                others.push(format!("`{}` `{}`", r.ty, r.key));
            } else {
                purged.push(r);
            }
        }
    }
    if !others.is_empty() {
        let n = others.len();
        others.truncate(NAMED);
        let from = ty.map(|t| format!(" from `{t}`")).unwrap_or_default();
        return Err(invalid(format!(
            "the purge of `{source}`{from} is refused whole: {n} record(s) it brought in were \
             also written by another hand (a principal, or another source), and a purge never \
             half-removes a record; retire them or clear those fields, then purge again. \
             First: {}",
            others.join(", ")
        )));
    }
    for r in &purged {
        match report.types.last_mut() {
            Some(t) if t.ty == r.ty => t.retired += 1,
            _ => report.types.push(TypeRetired {
                ty: r.ty.clone(),
                retired: 1,
            }),
        }
        report.retired += 1;
        // What an author wrote in a record the purge takes is a field with
        // an `initial` value, counted per type and field.
        for f in r.fields.keys() {
            if r.meta.get(f).is_none_or(|m| m.origin != Origin::Authored) {
                continue;
            }
            let at = report
                .moved
                .iter()
                .position(|m| m.ty == r.ty && &m.field == f);
            let at = at.unwrap_or_else(|| {
                report.moved.push(PurgeMoved {
                    ty: r.ty.clone(),
                    field: f.clone(),
                    records: 0,
                    first: Vec::new(),
                });
                report.moved.len() - 1
            });
            report.moved[at].records += 1;
            if report.moved[at].first.len() < NAMED {
                report.moved[at].first.push(r.key.clone());
            }
        }
    }
    report
        .moved
        .sort_by(|a, b| (&a.ty, &a.field).cmp(&(&b.ty, &b.field)));
    let taken = |ty: &TypeName, key: Option<&str>| {
        key.is_some_and(|k| purged.iter().any(|p| &p.ty == ty && p.key.as_str() == k))
    };
    let text =
        |r: &StoredRecord, f: &str| r.fields.get(f).and_then(|v| v.as_str().map(str::to_owned));
    for (name, t) in &st.types {
        for rel in &t.decl.relations {
            if rel.derived.is_some() {
                continue;
            }
            // A `via` relation is carried by its type's records, any
            // other by the records of its link type.
            let carrier = match &rel.via {
                Some(_) => name.clone(),
                None => crate::links::name(name, &rel.name),
            };
            let mut found: Vec<&StoredRecord> = (st.records.values())
                .filter(|r| r.ty == carrier && !r.retired && !taken(&r.ty, Some(r.key.as_str())))
                .filter(|r| match &rel.via {
                    Some(via) => taken(&rel.target, text(r, via.as_str()).as_deref()),
                    None => {
                        taken(name, text(r, lakelands_api::key::FROM).as_deref())
                            || taken(&rel.target, text(r, lakelands_api::key::TO).as_deref())
                    }
                })
                .collect();
            if found.is_empty() {
                continue;
            }
            found.sort_by(|a, b| a.key.cmp(&b.key));
            let first = found.iter().take(NAMED).map(|r| RecordRef {
                ty: r.ty.clone(),
                key: r.key.clone(),
            });
            report.references.push(PurgeReference {
                ty: name.clone(),
                relation: rel.name.clone(),
                records: found.len() as u64,
                first: first.collect(),
            });
        }
    }
    Ok((purged, report))
}

/// What [`run`] would do, appending nothing.
pub(crate) fn preflight(
    log: &Log,
    source: &SourceId,
    ty: Option<&TypeName>,
) -> Result<PurgeReport> {
    let (st, head) = (State::fold(&log.events), Shared::head(log));
    let (_, mut report) = plan(&st, head, source, ty)?;
    report.report_id = id(source, ty, head);
    Ok(report)
}

/// Retire every live record `source` brought in, into `ty` alone where
/// one is named, in one append; with a preflight's `report_id`, only if
/// that report is of this purge and the lake has not moved since.
pub(crate) fn run(
    shared: &Shared,
    log: &mut Log,
    source: &SourceId,
    ty: Option<&TypeName>,
    report_id: Option<&str>,
) -> Result<PurgeReport> {
    let (st, head) = (State::fold(&log.events), Shared::head(log));
    if report_id.is_some_and(|given| given != id(source, ty, head)) {
        let message = "that report is not of this purge at this head; run the preflight again";
        return Err(invalid(message.into()));
    }
    let (purged, mut report) = plan(&st, head, source, ty)?;
    let mut batch = Batch::at(log);
    for r in purged {
        batch.push(Draft::system(EventBody::RecordRetired {
            ty: r.ty.clone(),
            key: r.key.clone(),
            version: Version(r.version.0 + 1),
        }));
    }
    if report.retired > 0 {
        shared.append(log, WriteContext::default(), batch);
    }
    report.position = Shared::head(log);
    Ok(report)
}
