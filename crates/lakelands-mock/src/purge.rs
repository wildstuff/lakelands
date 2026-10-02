//! Purging a source over the mock's log: the store's rule for whose
//! records they are, its refusal of a co-owned record, and one append of
//! `record_retired` events. The mock computes no reactions.

use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::event::EventBody;
use lakelands_api::id::{Position, SourceId, Version};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{PurgeReport, TypeRetired};

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

/// Retire every live record `source` brought in, in one append.
pub(crate) fn run(shared: &Shared, log: &mut Log, source: &SourceId) -> Result<PurgeReport> {
    let st = State::fold(&log.events);
    let mut report = PurgeReport {
        source: source.clone(),
        retired: 0,
        types: Vec::new(),
        unsearched: Vec::new(),
        position: Position::GENESIS,
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
        return Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid {
                message: format!(
                    "the purge of `{source}` is refused whole: {} record(s) are co-owned: {}",
                    others.len(),
                    others
                        .iter()
                        .take(10)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        });
    }
    let mut batch = Batch::at(log);
    for r in purged {
        batch.push(Draft::system(EventBody::RecordRetired {
            ty: r.ty.clone(),
            key: r.key.clone(),
            version: Version(r.version.0 + 1),
        }));
        match report.types.last_mut() {
            Some(t) if t.ty == r.ty => t.retired += 1,
            _ => report.types.push(TypeRetired {
                ty: r.ty.clone(),
                retired: 1,
            }),
        }
        report.retired += 1;
    }
    if report.retired > 0 {
        shared.append(log, WriteContext::default(), batch);
    }
    report.position = Shared::head(log);
    Ok(report)
}
