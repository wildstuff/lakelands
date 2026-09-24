//! Fields computed by a function whose backing reaches outside the lake.
//!
//! The fold never calls a backing. It decides, from the log alone, when
//! such a field is *due*: its record was written and the function's
//! inputs changed (a record written for the first time included), or what
//! computes it was declared again. A due field whose call has an output
//! recorded in the lake is filled from it in the same event; otherwise it
//! is cleared and the call is handed out as a [`Due`]. After the append,
//! [`settle`] calls the backings for the calls that are due, one call at
//! a time per lake, and records each answer as `output_recorded`, whose
//! fold fills the field (`docs/decisions/022-functions-are-declarations.md`,
//! extended; `docs/log-format.md` §2.5).
//!
//! **The call** is `{ "function", "inputs", "config" }`: every declared
//! input by name with the record's value of that field (`null` where the
//! record holds none), and the declared configuration (`null` when none).
//! Its digest is the SHA-256 of its canonical bytes; with the seam
//! `function:<kind>:<function>` it keys the recorded output.
//!
//! **Failure.** A backing that fails is recorded as `{"error": "<why>"}`.
//! That output, or one that is not a value of the field's type, clears the
//! field and puts the reason in its metadata's `error`. A cleared field
//! without `error` is one whose output is not recorded yet.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::ProjectionView;
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{FieldDecl, TypeDecl};
use lakelands_api::record::{FieldMeta, Origin, Patch};
use lakelands_api::seam::FunctionCall;
use lakelands_api::write::FunctionDecl;
use lakelands_api::{FieldName, LakeId, Position, RecordKey, Result, TypeName, Value};
use serde_json::json;

use crate::backing::EXPR;
use crate::exec;
use crate::lake::Lake;
use crate::log::Draft;
use crate::ontology::{Entry, Ontology};

/// One call the fold made due: a field it cleared because no output is
/// recorded for the call its record makes now.
#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    /// The record's type.
    pub ty: TypeName,
    /// The record's key.
    pub key: RecordKey,
    /// The field the output fills.
    pub field: FieldName,
    /// The backing kind.
    pub kind: String,
    /// The function, as the log names its procedure.
    pub procedure: Procedure,
    /// The seam the output is recorded under.
    pub seam: String,
    /// The call's digest.
    pub input: Digest,
    /// The call, as the backing receives it.
    pub call: FunctionCall,
    /// The event that made it due.
    pub cause: Position,
    /// That event's context, which the output and what it causes carry.
    pub context: lakelands_api::WriteContext,
}

/// Where the fold looks for recorded outputs, and where it hands out the
/// calls it makes due. The write path, which sees the record a write will
/// produce before it appends, looks in the same place and hands out
/// nothing.
pub struct Outputs<'a> {
    /// The projection before the append.
    pub view: &'a dyn ProjectionView,
    /// Outputs recorded earlier in the same fold, not in the view yet.
    pub staged: Option<&'a BTreeMap<(String, Digest), Value>>,
    /// The calls made due, when the caller keeps them.
    pub due: Option<&'a mut Vec<Due>>,
}

impl Outputs<'_> {
    /// What the lake recorded for a call, if anything.
    fn recorded(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        if let Some(v) = self
            .staged
            .and_then(|s| s.get(&(seam.to_owned(), input.clone())))
        {
            return Ok(Some(v.clone()));
        }
        self.view.output(seam, input)
    }
}

/// True for a function computed outside the store.
pub fn is_backed(f: &FunctionDecl) -> bool {
    f.backing != EXPR
}

/// The seam a function's outputs are recorded under.
pub fn seam(f: &FunctionDecl) -> String {
    format!("function:{}:{}", f.backing, f.name)
}

/// The call a record's fields make of a function: `{ "function",
/// "inputs", "config" }`.
pub fn call_of(f: &FunctionDecl, fields: &BTreeMap<FieldName, Value>) -> Value {
    let inputs: serde_json::Map<String, Value> = f
        .inputs
        .iter()
        .map(|i| {
            (
                i.name.to_string(),
                fields.get(&i.name).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    json!({ "function": f.name, "inputs": inputs, "config": f.config })
}

/// The digest of a call: SHA-256 over its canonical bytes.
pub fn digest(call: &Value) -> Digest {
    Digest(lakelands_core::canonical::sha256_hex(call))
}

/// What a recorded output makes of a field: its value, or why it holds
/// none. `{"error": "<why>"}` is a failure; so is an output that is not a
/// value of the field's type.
pub fn read_output(
    ontology: &Ontology,
    decl: &TypeDecl,
    field: &FieldDecl,
    function: &str,
    output: &Value,
) -> std::result::Result<Value, String> {
    if let Value::Object(m) = output
        && m.len() == 1
        && let Some(Value::String(why)) = m.get("error")
    {
        return Err(why.clone());
    }
    let patch: Patch = [(field.name.clone(), output.clone())].into();
    let checked = crate::writes::check_values(ontology, decl, &patch, std::slice::from_ref(field))
        .map_err(|e| {
            format!(
                "the output of `{function}` is not a value of `{}`: {e}",
                field.name
            )
        })?;
    match checked.into_iter().next() {
        Some((_, v)) if !v.is_null() => Ok(v),
        _ => Err(format!("`{function}` answered no value")),
    }
}

/// The backed fields of a type: each field a backed function computes,
/// with that function.
fn backed_fields<'o>(
    ontology: &'o Ontology,
    decl: &'o TypeDecl,
) -> impl Iterator<Item = (&'o FieldDecl, &'o Entry<FunctionDecl>)> + 'o {
    decl.fields.iter().filter_map(|f| {
        let func = ontology.functions.get(f.function.as_deref()?)?;
        is_backed(&func.decl).then_some((f, func))
    })
}

/// The metadata of a value an output filled, or of a field it cleared.
fn meta(event: &Event, func: &Entry<FunctionDecl>, error: Option<String>) -> FieldMeta {
    FieldMeta {
        origin: Origin::Derived,
        known_at: event.known_at,
        seen_at: event.seen_at,
        position: event.position,
        writer: event.writer.clone(),
        source: event.source.clone(),
        procedure_version: Some(func.version.to_string()),
        error,
    }
}

/// Put an output into a record's field: its value, or no value and the
/// reason. Answers whether anything changed.
#[allow(clippy::too_many_arguments)]
fn put(
    ontology: &Ontology,
    decl: &TypeDecl,
    field: &FieldDecl,
    func: &Entry<FunctionDecl>,
    output: &Value,
    event: &Event,
    fields: &mut BTreeMap<FieldName, Value>,
    metas: &mut BTreeMap<FieldName, FieldMeta>,
) -> bool {
    let answer = read_output(ontology, decl, field, &func.decl.name, output);
    let version = Some(func.version.to_string());
    let old = metas.get(&field.name);
    let same = match &answer {
        Ok(v) => {
            fields.get(&field.name) == Some(v)
                && old.is_some_and(|m| m.error.is_none() && m.procedure_version == version)
        }
        Err(why) => {
            !fields.contains_key(&field.name)
                && old.is_some_and(|m| {
                    m.error.as_deref() == Some(why.as_str()) && m.procedure_version == version
                })
        }
    };
    if same {
        return false;
    }
    match answer {
        Ok(v) => {
            fields.insert(field.name.clone(), v);
            metas.insert(field.name.clone(), meta(event, func, None));
        }
        Err(why) => {
            fields.remove(&field.name);
            metas.insert(field.name.clone(), meta(event, func, Some(why)));
        }
    }
    true
}

/// The fields a computed field reads: a derived expression's reads, or
/// the inputs of the function that computes it. `None` for a field the
/// fold does not compute.
fn computed_reads(o: &Ontology, f: &FieldDecl) -> Option<BTreeSet<FieldName>> {
    if let Some(func) = f.function.as_deref().and_then(|n| o.functions.get(n)) {
        return Some(func.decl.inputs.iter().map(|i| i.name.clone()).collect());
    }
    let e = f.derived.as_ref()?;
    let e = crate::declare::condition(e, lakelands_core::expr::Place::Record, "derived").ok()?;
    Some(lakelands_core::derive::reads(&e))
}

/// A type's computed fields in the order the fold settles them, each
/// after every computed field it reads, declaration order among the rest;
/// with the computed fields each reads. One graph for derived expressions
/// and function-computed fields alike: a cycle in it is an error naming
/// its fields, and the declaration that would close it is refused.
pub fn computed_order(
    o: &Ontology,
    t: &TypeDecl,
) -> std::result::Result<Vec<(FieldName, BTreeSet<FieldName>)>, String> {
    let mut waiting: Vec<(FieldName, BTreeSet<FieldName>)> = t
        .fields
        .iter()
        .filter_map(|f| Some((f.name.clone(), computed_reads(o, f)?)))
        .collect();
    if let Some((n, _)) = waiting.iter().find(|(n, reads)| reads.contains(n)) {
        return Err(format!("the computed field `{n}` reads itself"));
    }
    let computed: BTreeSet<FieldName> = waiting.iter().map(|(n, _)| n.clone()).collect();
    for (_, reads) in &mut waiting {
        reads.retain(|r| computed.contains(r));
    }
    let mut done: BTreeSet<FieldName> = BTreeSet::new();
    let mut out = Vec::new();
    while !waiting.is_empty() {
        let Some(i) = waiting.iter().position(|(_, reads)| reads.is_subset(&done)) else {
            let cycle: Vec<String> = waiting.iter().map(|(n, _)| format!("`{n}`")).collect();
            return Err(format!(
                "computed fields read each other in a cycle: {}",
                cycle.join(", ")
            ));
        };
        let (n, reads) = waiting.remove(i);
        done.insert(n.clone());
        out.push((n, reads));
    }
    Ok(out)
}

/// One pass of the fold over a record's backed fields, in the order of
/// [`computed_order`]. A backed field is settled when its function's
/// inputs changed since `before` (`None`: a record written for the first
/// time), with `force` (what computes it was declared again), or when it
/// holds neither a value nor an error (it was waiting). Settling fills it
/// from a recorded output, or clears it and makes the call due.
///
/// A backed field that reads, directly or through derived fields, one
/// that is waiting for its output waits too: it is cleared and not
/// called, since its call would carry an input that is not computed yet
/// and record an output under that input for good. It is settled when
/// what it reads is filled, by the fill's own pass. Answers the fields it
/// changed.
#[allow(clippy::too_many_arguments)]
pub fn settle_fields(
    ontology: &Ontology,
    lake: &LakeId,
    decl: &TypeDecl,
    key: &RecordKey,
    before: Option<&BTreeMap<FieldName, Value>>,
    fields: &mut BTreeMap<FieldName, Value>,
    metas: &mut BTreeMap<FieldName, FieldMeta>,
    event: &Event,
    force: bool,
    outputs: &mut Outputs<'_>,
) -> Result<Vec<FieldName>> {
    let backed: BTreeMap<FieldName, (&FieldDecl, &Entry<FunctionDecl>)> =
        backed_fields(ontology, decl)
            .map(|(f, func)| (f.name.clone(), (f, func)))
            .collect();
    let mut changed = Vec::new();
    if backed.is_empty() {
        return Ok(changed);
    }
    let order =
        computed_order(ontology, decl).map_err(|why| lakelands_api::Error::Unavailable {
            message: format!(
                "the fold cannot order the computed fields of `{}`: {why}",
                decl.name
            ),
        })?;
    let mut waiting: BTreeSet<FieldName> = BTreeSet::new();
    for (name, reads) in order {
        let reads_waiting = reads.iter().any(|r| waiting.contains(r));
        let Some((field, func)) = backed.get(&name).copied() else {
            if reads_waiting {
                waiting.insert(name);
            }
            continue;
        };
        if reads_waiting {
            let had = fields.remove(&name).is_some();
            let had_meta = metas.remove(&name).is_some();
            if had || had_meta {
                changed.push(name.clone());
            }
            waiting.insert(name);
            continue;
        }
        let call = call_of(&func.decl, fields);
        let moved = match before {
            None => true,
            Some(b) => call_of(&func.decl, b) != call,
        };
        let unsettled =
            !fields.contains_key(&name) && metas.get(&name).is_none_or(|m| m.error.is_none());
        if !force && !moved && !unsettled {
            continue;
        }
        let seam = seam(&func.decl);
        let input = digest(&call);
        if let Some(output) = outputs.recorded(&seam, &input)? {
            if put(ontology, decl, field, func, &output, event, fields, metas) {
                changed.push(name.clone());
            }
            continue;
        }
        // No output yet: the field is cleared, without an error, until
        // one is recorded.
        let had = fields.remove(&name).is_some();
        let had_meta = metas.remove(&name).is_some();
        if had || had_meta {
            changed.push(name.clone());
        }
        waiting.insert(name.clone());
        if let Some(due) = outputs.due.as_deref_mut() {
            due.push(Due {
                ty: decl.name.clone(),
                key: key.clone(),
                field: name,
                kind: func.decl.backing.clone(),
                procedure: Procedure {
                    name: func.decl.name.clone(),
                    version: func.version.to_string(),
                },
                seam,
                input,
                call: FunctionCall {
                    lake: lake.clone(),
                    function: func.decl.name.clone(),
                    inputs: call["inputs"].clone(),
                    config: func.decl.config.clone(),
                },
                cause: event.position,
                context: event.context,
            });
        }
    }
    Ok(changed)
}

/// Put a recorded output into the field it names, when the field is
/// still computed by the function the seam names and the record's call
/// has the recorded digest now. Answers whether the field changed.
#[allow(clippy::too_many_arguments)]
pub fn put_output(
    ontology: &Ontology,
    decl: &TypeDecl,
    field: &FieldName,
    seam_name: &str,
    input: &Digest,
    output: &Value,
    event: &Event,
    fields: &mut BTreeMap<FieldName, Value>,
    metas: &mut BTreeMap<FieldName, FieldMeta>,
) -> bool {
    let Some((f, func)) = backed_fields(ontology, decl).find(|(f, _)| &f.name == field) else {
        return false;
    };
    if seam(&func.decl) != seam_name || &digest(&call_of(&func.decl, fields)) != input {
        return false;
    }
    put(ontology, decl, f, func, output, event, fields, metas)
}

/// Call the backings for the calls the lake's appends made due, and
/// record each answer, until no call is due. One call at a time per lake:
/// the lane is held throughout.
///
/// A call is skipped when its record has moved on to other inputs since it
/// was made due, or when the same field of the same record is due twice
/// for the same input. Before each call the outputs already recorded are
/// consulted again, so no input is sent to a backing twice. A call whose
/// backing kind is not bound here is not made. The calls a fill makes due
/// are made in the same drain; they end, since a type's computed fields
/// read one another without a cycle (refused at declare).
///
/// `seen` is [`Lake::drains`] as the caller read it before its write.
/// Answers the lake's head once the lane is drained when this call or any
/// other drained calls since `seen`: a neighbour may have made this
/// writer's calls, and the head is then the position from which its
/// fields are visible. `None` when no call was due.
pub async fn settle(lake: &Arc<Lake>, seen: u64) -> Result<Option<Position>> {
    // Nothing due and nobody drained since the write: the write's own
    // position stands, and it does not wait behind a neighbour's call.
    if lake.writer().due.is_empty() && lake.drains.load(Ordering::Acquire) == seen {
        return Ok(None);
    }
    let _lane = lake.calls.lock().await;
    let mut drained = false;
    loop {
        // Counted before the dues leave the writer, under its lock: a
        // writer that finds nothing due also finds the count moved.
        let due = {
            let mut w = lake.writer();
            if w.due.is_empty() {
                break;
            }
            lake.drains.fetch_add(1, Ordering::AcqRel);
            std::mem::take(&mut w.due)
        };
        drained = true;
        let mut due: std::collections::VecDeque<Due> = due.into();
        let mut taken: BTreeSet<(TypeName, RecordKey, FieldName, Digest)> = BTreeSet::new();
        while let Some(d) = due.pop_front() {
            if let Err(e) = settle_one(lake, &mut taken, &d).await {
                // The lake failed under the drain: what was not recorded
                // stays due, for the next drain.
                let mut w = lake.writer();
                let rest: Vec<Due> = std::iter::once(d).chain(due).collect();
                w.due.splice(0..0, rest);
                return Err(e);
            }
        }
    }
    let answered = drained || lake.drains.load(Ordering::Acquire) != seen;
    Ok(answered.then(|| lake.head()))
}

/// Make one due call, unless it is a repeat in this drain, its kind is not
/// bound, or its record moved on; and record the answer.
async fn settle_one(
    lake: &Arc<Lake>,
    taken: &mut BTreeSet<(TypeName, RecordKey, FieldName, Digest)>,
    d: &Due,
) -> Result<()> {
    if !taken.insert((
        d.ty.clone(),
        d.key.clone(),
        d.field.clone(),
        d.input.clone(),
    )) {
        return Ok(());
    }
    let Some(backing) = lake
        .seams
        .backings
        .iter()
        .find(|b| b.kind() == d.kind)
        .cloned()
    else {
        return Ok(());
    };
    let l = lake.clone();
    let (seam, input, ty, key) = (d.seam.clone(), d.input.clone(), d.ty.clone(), d.key.clone());
    let (current, recorded) = exec::run(move || {
        let view = l.engine.view(None)?;
        Ok((view.get(&ty, &key)?, view.output(&seam, &input)?))
    })
    .await?;
    // The record moved on (or went) while the call waited: its fields are
    // due under other inputs, or not at all.
    let wants = current.is_some_and(|r| {
        let w = lake.writer();
        !r.retired
            && w.ontology.ty(&r.ty).ok().is_some_and(|decl| {
                decl.fields
                    .iter()
                    .find(|f| f.name == d.field)
                    .and_then(|f| w.ontology.functions.get(f.function.as_deref()?))
                    .is_some_and(|func| digest(&call_of(&func.decl, &r.fields)) == d.input)
            })
    });
    if !wants {
        return Ok(());
    }
    let (output, known) = match recorded {
        Some(o) => (o, true),
        None => match backing.call(&d.call).await {
            Ok(v) => (v, false),
            Err(e) => (json!({ "error": why(&e) }), false),
        },
    };
    let (l, d) = (lake.clone(), d.clone());
    exec::run(move || record(&l, d, output, known)).await?;
    Ok(())
}

/// A backing's failure, in its own words.
fn why(e: &lakelands_api::Error) -> String {
    use lakelands_api::{Error, Refusal};
    match e {
        Error::Unavailable { message }
        | Error::Refused {
            reason: Refusal::Invalid { message } | Refusal::Constraint { message },
            ..
        } => message.clone(),
        other => other.to_string(),
    }
}

/// Record one output for the field it fills, with what the fill causes:
/// the triggers it fires and the object sets its record joins or leaves,
/// judged on the record as the fold will leave it, as for any write.
///
/// An output the lake already holds (`known`) is recorded again only when
/// it fills something: a due issued twice appends nothing the second time.
fn record(lake: &Lake, d: Due, output: Value, known: bool) -> Result<Option<Position>> {
    let mut w = lake.writer();
    let now = lake.clock(&mut w);
    let view = lake.engine.view(None)?;
    let at = Position(w.tip.as_ref().map_or(0, |t| t.0.0 + 1));
    let context = d.context;
    let draft = Draft {
        cause: Some(d.cause),
        procedure: Some(d.procedure.clone()),
        context,
        ..Draft::system(
            now,
            EventBody::OutputRecorded {
                seam: d.seam.clone(),
                input: d.input.clone(),
                output: output.clone(),
                field: Some((d.ty.clone(), d.key.clone(), d.field.clone())),
            },
        )
    };
    let mut drafts = Vec::new();
    let mut fills = false;
    if let Some(before) = view.get(&d.ty, &d.key)? {
        let event = crate::writes::provisional(lake, at, &draft);
        let staged: BTreeMap<(String, Digest), Value> =
            [((d.seam.clone(), d.input.clone()), output.clone())].into();
        let after = crate::record::fill(
            &w.ontology,
            &before,
            &d.field,
            &d.seam,
            &d.input,
            &output,
            &event,
            &w.settings,
            &mut Outputs {
                view: view.as_ref(),
                staged: Some(&staged),
                due: None,
            },
        )?;
        if let Some(after) = after {
            fills = true;
            let host = lakelands_api::Principal {
                kind: lakelands_api::PrincipalKind::System,
                id: "lakelands".into(),
                lake: lake.id.clone(),
                grants: lakelands_api::Grants::owner(),
            };
            let call = crate::writes::Call {
                lake,
                who: &host,
                context,
            };
            drafts = call.reactions(
                &w,
                view.as_ref(),
                &mut crate::writes::Pending::default(),
                Some(&before),
                &after,
                at,
                now,
            )?;
        }
    }
    drop(view);
    if known && !fills {
        return Ok(None);
    }
    drafts.insert(0, draft);
    let events = lake.append(&mut w, drafts)?;
    Ok(events.last().map(|e| e.position))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: &str) -> FieldDecl {
        serde_json::from_value(json!({"name": name, "type": {"kind": "text"}})).unwrap()
    }

    /// The vector `docs/log-format.md` §2.5 pins.
    #[test]
    fn a_calls_digest_is_the_one_the_log_format_pins() {
        let geocode = FunctionDecl {
            name: "geocode".into(),
            backing: "http".into(),
            inputs: vec![text("address"), text("country")],
            expr: None,
            config: json!({"url": "https://geo.example/lookup"}),
            description: None,
        };
        let fields: BTreeMap<FieldName, Value> = [
            ("address".into(), json!("Hauptstraße 1")),
            ("unrelated".into(), json!("not an input")),
        ]
        .into();
        let call = call_of(&geocode, &fields);
        assert_eq!(
            call,
            json!({"function": "geocode",
                   "inputs": {"address": "Hauptstraße 1", "country": null},
                   "config": {"url": "https://geo.example/lookup"}})
        );
        assert_eq!(
            digest(&call).0,
            "a4702edebf8e4f34b65be7b55b963f930ac4e6f13d0b9a8773ca942814bc77aa"
        );
        assert_eq!(seam(&geocode), "function:http:geocode");
    }
}
