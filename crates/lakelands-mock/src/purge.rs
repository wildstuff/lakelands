//! Purging a source over the mock's log: the store's rule for whose
//! records they are, its refusal of a co-owned record, and one append of
//! `record_retired` events. The mock computes no reactions.

use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::event::EventBody;
use lakelands_api::id::{Position, SourceId, TypeName, Version};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{PurgeReference, PurgeReport, RecordRef, TypeRetired};

use crate::state::State;
use crate::{Batch, Draft, Log, Shared};

fn brought_by(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta
            .get(f)
            .is_some_and(|m| m.origin == Origin::Source && m.source.as_ref() == Some(source))
    })
}

fn co_owned(r: &StoredRecord, source: &SourceId) -> bool {
    r.fields.keys().any(|f| {
        r.meta.get(f).is_some_and(|m| match m.origin {
            Origin::Authored => true,
            Origin::Source => m.source.as_ref() != Some(source),
            _ => false,
        })
    })
}

/// How many records a refusal or a report names.
const NAMED: usize = 10;

/// The id of the preflight of a purge of `source` read at `head`: stale
/// once the head moves, as the store's is.
fn id(source: &SourceId, head: Position) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("purge {} at {}", source, head.0));
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// What a purge of `source` would do, by the store's rules: the records
/// it takes, refused whole for a co-owned one, and per relation the live
/// records it does not take that refer into it, by a `via` field or as a
/// link record with an end inside.
fn plan<'s>(
    st: &'s State,
    head: Position,
    source: &SourceId,
) -> Result<(Vec<&'s StoredRecord>, PurgeReport)> {
    let mut report = PurgeReport {
        source: source.clone(),
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
        if t.decl.view.is_some() {
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
            if co_owned(r, source) {
                others.push(format!("`{}` `{}`", r.ty, r.key));
            } else {
                purged.push(r);
            }
        }
    }
    if !others.is_empty() {
        let n = others.len();
        others.truncate(NAMED);
        return Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid {
                message: format!(
                    "the purge of `{source}` is refused whole: {n} record(s) it brought in were \
                     also written by another hand (a principal, or another source), and a purge \
                     never half-removes a record; retire them or clear those fields, then purge \
                     again. First: {}",
                    others.join(", ")
                ),
            },
        });
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
    }
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
pub(crate) fn preflight(log: &Log, source: &SourceId) -> Result<PurgeReport> {
    let (st, head) = (State::fold(&log.events), Shared::head(log));
    let (_, mut report) = plan(&st, head, source)?;
    report.report_id = id(source, head);
    Ok(report)
}

/// Retire every live record `source` brought in, in one append; with a
/// preflight's `report_id`, only if the lake has not moved since.
pub(crate) fn run(
    shared: &Shared,
    log: &mut Log,
    source: &SourceId,
    report_id: Option<&str>,
) -> Result<PurgeReport> {
    let (st, head) = (State::fold(&log.events), Shared::head(log));
    if report_id.is_some_and(|given| given != id(source, head)) {
        let message = "the lake moved since that preflight report; run the preflight again";
        return Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid {
                message: message.into(),
            },
        });
    }
    let (purged, mut report) = plan(&st, head, source)?;
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
