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

use lakelands_api::engine::Scan;
use lakelands_api::event::EventBody;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{PurgeReport, TypeRetired};
use lakelands_api::{
    Error, Grants, Position, Principal, PrincipalKind, Refusal, Result, SourceId, Version,
    WriteContext,
};

use crate::backed::Outputs;
use crate::lake::Lake;
use crate::log::Draft;
use crate::record;
use crate::writes::{self, Call, Pending};

/// How many co-owned records a refusal names.
const NAMED: usize = 10;

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

/// Retire every live record `source` brought in, in one append.
pub fn run(lake: &Lake, source: &SourceId) -> Result<PurgeReport> {
    let system = Principal {
        kind: PrincipalKind::System,
        id: "lakelands".into(),
        lake: lake.id.clone(),
        grants: Grants::owner(),
    };
    let call = Call {
        lake,
        who: &system,
        context: WriteContext::default(),
    };
    let all = Scan {
        limit: u32::MAX,
        restricted: true,
        ..Default::default()
    };
    let mut w = lake.writer();
    let view = lake.engine.view(None)?;
    let mut report = PurgeReport {
        source: source.clone(),
        retired: 0,
        types: Vec::new(),
        unsearched: Vec::new(),
        position: Position::GENESIS,
    };
    let mut purged: Vec<StoredRecord> = Vec::new();
    let mut shared: Vec<String> = Vec::new();
    for (name, entry) in &w.ontology.types {
        let decl = &entry.decl;
        if decl.view.is_some() {
            continue;
        }
        if !decl.field_meta {
            report.unsearched.push(name.clone());
            continue;
        }
        for r in view.scan(name, &all)? {
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
        return Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid {
                message: format!(
                    "the purge of `{source}` is refused whole: {n} record(s) it brought in were \
                     also written by another hand (a principal, or another source), and a purge \
                     never half-removes a record; retire them or clear those fields, then purge \
                     again. First: {}",
                    shared.join(", ")
                ),
            },
        });
    }
    let now = lake.clock(&mut w);
    let mut pending = Pending::default();
    let mut drafts = Vec::new();
    for before in purged {
        let d = Draft::system(
            now,
            EventBody::RecordRetired {
                ty: before.ty.clone(),
                key: before.key.clone(),
                version: Version(before.version.0 + 1),
            },
        );
        let cause = writes::position_after(&w, drafts.len());
        let event = writes::provisional(lake, cause, &d);
        let after = record::apply(
            &w.ontology,
            Some(&before),
            &event,
            &w.settings,
            &mut Outputs {
                view: view.as_ref(),
                staged: None,
                due: None,
            },
        )?
        .expect("a retire makes a record");
        drafts.push(d);
        drafts.extend(call.reactions(
            &w,
            view.as_ref(),
            &mut pending,
            Some(&before),
            &after,
            cause,
            now,
        )?);
        match report.types.last_mut() {
            Some(t) if t.ty == before.ty => t.retired += 1,
            _ => report.types.push(TypeRetired {
                ty: before.ty.clone(),
                retired: 1,
            }),
        }
        report.retired += 1;
    }
    drop(view);
    if !drafts.is_empty() {
        lake.append(&mut w, drafts)?;
    }
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    Ok(report)
}
