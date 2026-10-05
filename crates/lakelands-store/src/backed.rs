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
//! **An embedding** (`FieldDecl::embed`) is computed the same way, by the
//! bound [`EmbedBackend`](lakelands_api::seam::EmbedBackend): its call is
//! `{"texts": [text]}`, the text its `from` fields join to, under the seam
//! `embed:<model>`. With no text, the field is cleared and keeps its
//! metadata without an `error`, and nothing is due. The drain sends the
//! due texts of one model in batches of at most [`EMBED_BATCH`], and
//! records each vector on its own.
//!
//! **Failure.** A backing that fails is recorded as `{"error": "<why>"}`.
//! That output, or one that is not a value of the field's type, clears the
//! field and puts the reason in its metadata's `error`. A cleared field
//! without `error` and without metadata is one whose output is not
//! recorded yet. A settle forced by a declaration calls again for an input
//! whose recorded output is a failure; the later output holds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::ProjectionView;
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{EmbedDecl, FieldDecl, TypeDecl};
use lakelands_api::record::{FieldMeta, Origin, Patch};
use lakelands_api::seam::FunctionCall;
use lakelands_api::write::FunctionDecl;
use lakelands_api::{FieldName, LakeId, Position, RecordKey, Result, TypeName, Value};
use lakelands_core::unavailable;
use serde_json::json;

use crate::backing::EXPR;
use crate::exec;
use crate::lake::Lake;
use crate::log::Draft;
use crate::ontology::{Entry, Ontology};

/// At most this many texts go to the embedding backend in one request.
/// The drain holds one batch of vectors at a time, however many records a
/// backfill makes due, so its memory stays bounded; the backend may split
/// a batch further.
pub const EMBED_BATCH: usize = 64;

/// What a due call asks of which seam.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// A function backing of this kind, with the call it receives.
    Function {
        /// The backing kind.
        kind: String,
        /// The call, as the backing receives it.
        call: FunctionCall,
    },
    /// The embedding backend, which must name this model and make vectors
    /// of these dimensions.
    Embed {
        /// The model.
        model: String,
        /// The field's dimensions.
        dimensions: u32,
        /// The text to embed.
        text: String,
    },
}

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
    /// What computes it, as the log names its procedure.
    pub procedure: Procedure,
    /// The seam the output is recorded under.
    pub seam: String,
    /// The call's digest.
    pub input: Digest,
    /// The call.
    pub call: Call,
    /// Whether a recorded failure for the call is called again: the
    /// settle was forced by a declaration.
    pub retry: bool,
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
    pub staged: Option<&'a Recorded>,
    /// The position the reader reads before: a recomputation kept as a
    /// step reads what was recorded before its own event, whenever it is
    /// asked. `None` for every other reader, which reads the last.
    pub before: Option<Position>,
    /// The calls made due, when the caller keeps them.
    pub due: Option<&'a mut Vec<Due>>,
}

/// The outputs one transaction recorded, by seam and input: every
/// recording with its position, in the order of the log.
pub type Recorded = BTreeMap<(String, Digest), Vec<(Position, Value)>>;

impl Outputs<'_> {
    /// What the lake recorded for a call, if anything: the transaction's
    /// last recording the reader may read, else the view's.
    fn recorded(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        let staged = self
            .staged
            .and_then(|s| s.get(&(seam.to_owned(), input.clone())));
        let read = |(at, _): &&(Position, Value)| self.before.is_none_or(|before| *at < before);
        match staged.and_then(|all| all.iter().rev().find(read)) {
            Some((_, v)) => Ok(Some(v.clone())),
            None => self.view.output(seam, input),
        }
    }
}

/// True for a function computed outside the store.
pub fn is_backed(f: &FunctionDecl) -> bool {
    f.backing != EXPR
}

/// Whether a type has a field computed outside the lake: an embedding, or
/// a field computed by a function with a backing. Such a type is settled
/// again at every `declared` of it (022, extended).
pub fn computed_outside(o: &crate::ontology::Ontology, t: &TypeDecl) -> bool {
    let backed = |n: &str| o.functions.get(n).is_some_and(|f| is_backed(&f.decl));
    (t.fields.iter()).any(|f| f.embed.is_some() || f.function.as_deref().is_some_and(backed))
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

/// Whether a recorded output is a failure: `{"error": "<why>"}`.
fn is_failure(output: &Value) -> bool {
    matches!(output, Value::Object(m) if m.len() == 1 && matches!(m.get("error"), Some(Value::String(_))))
}

/// The seam an embedding's vectors are recorded under.
pub fn embed_seam(model: &str) -> String {
    format!("embed:{model}")
}

/// The text an embedding's `from` fields join to: their values in the
/// declared order, one blank line between them, empty ones left out.
/// `None` when no field holds any.
pub fn embed_text(e: &EmbedDecl, fields: &BTreeMap<FieldName, Value>) -> Option<String> {
    let parts: Vec<&str> = e
        .from
        .iter()
        .filter_map(|f| fields.get(f)?.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The call an embedding makes of the backend for one text.
pub fn embed_call(text: &str) -> Value {
    json!({ "texts": [text] })
}

/// What computes a field outside the fold: a backed function, or an
/// embedding declared by the type at a version.
#[derive(Clone, Copy)]
enum Computer<'o> {
    Function(&'o Entry<FunctionDecl>),
    Embed(&'o EmbedDecl, u32),
}

impl Computer<'_> {
    /// Its name as a message and the log's procedure give it: the
    /// function's, or the model's.
    fn name(&self) -> &str {
        match self {
            Computer::Function(f) => &f.decl.name,
            Computer::Embed(e, _) => &e.model,
        }
    }

    /// The version of the declaration that computes it.
    fn version(&self) -> String {
        match self {
            Computer::Function(f) => f.version.to_string(),
            Computer::Embed(_, v) => v.to_string(),
        }
    }

    /// Whether a field's metadata comes from this declaration. A
    /// function's version is its own; an embedding's is the type's, which
    /// moves with every declaration of the type, so an embedding's value
    /// under the same seam and call is the same whatever the version, and
    /// its metadata keeps the version that produced it.
    fn same_version(&self, m: &FieldMeta) -> bool {
        match self {
            Computer::Function(_) => m.procedure_version.as_deref() == Some(&*self.version()),
            Computer::Embed(..) => m.procedure_version.is_some(),
        }
    }

    fn seam(&self) -> String {
        match self {
            Computer::Function(f) => seam(&f.decl),
            Computer::Embed(e, _) => embed_seam(&e.model),
        }
    }

    /// The call a record's fields make; `None` for an embedding with no
    /// text, which calls nothing.
    fn call(&self, fields: &BTreeMap<FieldName, Value>) -> Option<Value> {
        match self {
            Computer::Function(f) => Some(call_of(&f.decl, fields)),
            Computer::Embed(e, _) => embed_text(e, fields).map(|t| embed_call(&t)),
        }
    }
}

/// The fields of a type computed outside the fold, each with what
/// computes it.
fn backed_fields<'o>(
    ontology: &'o Ontology,
    decl: &'o TypeDecl,
) -> impl Iterator<Item = (&'o FieldDecl, Computer<'o>)> + 'o {
    let version = ontology.types.get(&decl.name).map_or(0, |t| t.version);
    decl.fields.iter().filter_map(move |f| {
        if let Some(e) = &f.embed {
            return Some((f, Computer::Embed(e, version)));
        }
        let func = ontology.functions.get(f.function.as_deref()?)?;
        is_backed(&func.decl).then_some((f, Computer::Function(func)))
    })
}

/// The metadata of a value an output filled, or of a field it cleared.
fn meta(event: &Event, version: String, error: Option<String>) -> FieldMeta {
    FieldMeta {
        upstream: None,
        procedure_version: Some(version),
        error,
        ..crate::record::meta_of(event, Origin::Derived)
    }
}

/// Put an output into a record's field: its value, or no value and the
/// reason. Answers whether anything changed.
#[allow(clippy::too_many_arguments)]
fn put(
    ontology: &Ontology,
    decl: &TypeDecl,
    field: &FieldDecl,
    computer: Computer<'_>,
    output: &Value,
    event: &Event,
    fields: &mut BTreeMap<FieldName, Value>,
    metas: &mut BTreeMap<FieldName, FieldMeta>,
) -> bool {
    let answer = read_output(ontology, decl, field, computer.name(), output);
    let old = metas.get(&field.name);
    // The field holds the value and no error, or no value and this error.
    let (value, error) = (answer.as_ref().ok(), answer.as_ref().err());
    let same = fields.get(&field.name) == value
        && old.is_some_and(|m| m.error.as_ref() == error && computer.same_version(m));
    if same {
        return false;
    }
    let error = match answer {
        Ok(v) => {
            fields.insert(field.name.clone(), v);
            None
        }
        Err(why) => {
            fields.remove(&field.name);
            Some(why)
        }
    };
    metas.insert(field.name.clone(), meta(event, computer.version(), error));
    true
}

/// The fields a computed field reads: a derived expression's reads, the
/// inputs of the function that computes it, or the fields an embedding
/// is made from. `None` for a field the fold does not compute.
pub(crate) fn computed_reads(o: &Ontology, f: &FieldDecl) -> Option<BTreeSet<FieldName>> {
    if let Some(e) = &f.embed {
        return Some(e.from.iter().cloned().collect());
    }
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
/// [`computed_order`]. A backed field is settled when its call changed
/// since `before` (`None`: a record written for the first time), with
/// `force` (what computes it was declared again), or when it holds
/// neither a value nor an error (it was waiting). Settling fills it from a
/// recorded output, or clears it and makes the call due. With `force`, a
/// recorded failure does not fill it: the call is made due again.
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
    let backed: BTreeMap<FieldName, (&FieldDecl, Computer<'_>)> = backed_fields(ontology, decl)
        .map(|(f, c)| (f.name.clone(), (f, c)))
        .collect();
    let mut changed = Vec::new();
    if backed.is_empty() {
        return Ok(changed);
    }
    let order = computed_order(ontology, decl).map_err(|why| {
        unavailable(format!(
            "the fold cannot order the computed fields of `{}`: {why}",
            decl.name
        ))
    })?;
    let mut waiting: BTreeSet<FieldName> = BTreeSet::new();
    for (name, reads) in order {
        let reads_waiting = reads.iter().any(|r| waiting.contains(r));
        let Some((field, computer)) = backed.get(&name).copied() else {
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
        let call = computer.call(fields);
        let moved = before.is_none_or(|b| computer.call(b) != call);
        let unsettled =
            !fields.contains_key(&name) && metas.get(&name).is_none_or(|m| m.error.is_none());
        if !force && !moved && !unsettled {
            continue;
        }
        let Some(call) = call else {
            // An embedding with no text: no value, and metadata without an
            // error, which says there is nothing to embed.
            let already = !fields.contains_key(&name)
                && metas
                    .get(&name)
                    .is_some_and(|m| m.error.is_none() && computer.same_version(m));
            if !already {
                fields.remove(&name);
                metas.insert(name.clone(), meta(event, computer.version(), None));
                changed.push(name.clone());
            }
            continue;
        };
        let seam = computer.seam();
        let input = digest(&call);
        let recorded = outputs.recorded(&seam, &input)?;
        let retry = force && recorded.as_ref().is_some_and(is_failure);
        if let (Some(output), false) = (recorded, retry) {
            if put(
                ontology, decl, field, computer, &output, event, fields, metas,
            ) {
                changed.push(name.clone());
            }
            continue;
        }
        // No output yet (or a failure a declaration retries): the field is
        // cleared, without an error, until one is recorded.
        let had = fields.remove(&name).is_some();
        let had_meta = metas.remove(&name).is_some();
        if had || had_meta {
            changed.push(name.clone());
        }
        waiting.insert(name.clone());
        if let Some(due) = outputs.due.as_deref_mut() {
            let call = match computer {
                Computer::Function(f) => Call::Function {
                    kind: f.decl.backing.clone(),
                    call: FunctionCall {
                        lake: lake.clone(),
                        function: f.decl.name.clone(),
                        inputs: call["inputs"].clone(),
                        config: f.decl.config.clone(),
                    },
                },
                Computer::Embed(e, _) => Call::Embed {
                    model: e.model.clone(),
                    dimensions: field.ty.dimensions.unwrap_or(0),
                    text: call["texts"][0].as_str().unwrap_or_default().to_owned(),
                },
            };
            due.push(Due {
                ty: decl.name.clone(),
                key: key.clone(),
                field: name,
                procedure: Procedure {
                    name: computer.name().to_owned(),
                    version: computer.version(),
                },
                seam,
                input,
                call,
                retry,
                cause: event.position,
                context: event.context,
            });
        }
    }
    Ok(changed)
}

/// Put a recorded output into the field it names, when the field is
/// still computed by what the seam names and the record's call has the
/// recorded digest now. Answers whether the field changed.
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
    let Some((f, computer)) = backed_fields(ontology, decl).find(|(f, _)| &f.name == field) else {
        return false;
    };
    if computer.seam() != seam_name
        || computer.call(fields).map(|c| digest(&c)).as_ref() != Some(input)
    {
        return false;
    }
    put(ontology, decl, f, computer, output, event, fields, metas)
}

/// Whether a record still makes the call a due names: it is live, and
/// the field is computed by the same seam with the same digest.
fn still_wanted(
    w: &crate::lake::Writer,
    record: Option<&lakelands_api::record::StoredRecord>,
    d: &Due,
) -> bool {
    record.is_some_and(|r| {
        !r.retired
            && w.ontology.ty(&r.ty).ok().is_some_and(|decl| {
                backed_fields(&w.ontology, decl)
                    .find(|(f, _)| f.name == d.field)
                    .is_some_and(|(_, c)| {
                        c.seam() == d.seam
                            && c.call(&r.fields).map(|c| digest(&c)) == Some(d.input.clone())
                    })
            })
    })
}

/// What one drain has done: the dues it took, and the calls it retried.
#[derive(Default)]
struct Drain {
    taken: BTreeSet<(TypeName, RecordKey, FieldName, Digest)>,
    retried: BTreeSet<(String, Digest)>,
}

impl Drain {
    /// Whether a due is new in this drain.
    fn take(&mut self, d: &Due) -> bool {
        self.taken.insert((
            d.ty.clone(),
            d.key.clone(),
            d.field.clone(),
            d.input.clone(),
        ))
    }

    /// Whether a recorded output answers a due: a value always, a failure
    /// unless the due retries it and this drain has not retried it yet.
    fn answers(&mut self, d: &Due, recorded: &Value) -> bool {
        !(d.retry && is_failure(recorded) && self.retried.insert((d.seam.clone(), d.input.clone())))
    }
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
        let mut drain = Drain::default();
        while let Some(d) = due.pop_front() {
            // An embedding goes with the other due texts of its model and
            // dimensions, up to one batch.
            let batch = match &d.call {
                Call::Embed {
                    model, dimensions, ..
                } => {
                    let (model, dimensions) = (model.clone(), *dimensions);
                    let mut batch = vec![d];
                    let mut texts: BTreeSet<Digest> = [batch[0].input.clone()].into();
                    let mut rest = std::collections::VecDeque::new();
                    for x in due.drain(..) {
                        let same = matches!(
                            &x.call,
                            Call::Embed { model: m, dimensions: n, .. }
                                if *m == model && *n == dimensions
                        );
                        if same && (texts.contains(&x.input) || texts.len() < EMBED_BATCH) {
                            texts.insert(x.input.clone());
                            batch.push(x);
                        } else {
                            rest.push_back(x);
                        }
                    }
                    due = rest;
                    batch
                }
                Call::Function { .. } => vec![d],
            };
            let settled = match batch[0].call {
                Call::Embed { .. } => settle_embeddings(lake, &mut drain, &batch).await,
                Call::Function { .. } => settle_one(lake, &mut drain, &batch[0]).await,
            };
            if let Err(e) = settled {
                // The lake failed under the drain: what was not recorded
                // stays due, for the next drain.
                let mut w = lake.writer();
                let rest: Vec<Due> = batch.into_iter().chain(due).collect();
                w.due.splice(0..0, rest);
                return Err(e);
            }
        }
    }
    let answered = drained || lake.drains.load(Ordering::Acquire) != seen;
    Ok(answered.then(|| lake.head()))
}

/// The record a due names and the output recorded for its call, as the
/// lake holds them now.
async fn current(
    lake: &Arc<Lake>,
    d: &Due,
) -> Result<(Option<lakelands_api::record::StoredRecord>, Option<Value>)> {
    let l = lake.clone();
    let (seam, input, ty, key) = (d.seam.clone(), d.input.clone(), d.ty.clone(), d.key.clone());
    exec::run(move || {
        let view = l.engine.view(None)?;
        Ok((view.get(&ty, &key)?, view.output(&seam, &input)?))
    })
    .await
}

async fn record_async(lake: &Arc<Lake>, d: &Due, output: Value, known: bool) -> Result<()> {
    let (l, d) = (lake.clone(), d.clone());
    exec::run(move || record(&l, d, output, known)).await?;
    Ok(())
}

/// Make one due function call, unless it is a repeat in this drain, its
/// kind is not bound, or its record moved on; and record the answer.
async fn settle_one(lake: &Arc<Lake>, drain: &mut Drain, d: &Due) -> Result<()> {
    let Call::Function { kind, call } = &d.call else {
        return Ok(());
    };
    if !drain.take(d) {
        return Ok(());
    }
    let Some(backing) = lake
        .seams
        .backings
        .iter()
        .find(|b| b.kind() == kind)
        .cloned()
    else {
        return Ok(());
    };
    let (record, recorded) = current(lake, d).await?;
    // The record moved on (or went) while the call waited: its fields are
    // due under other inputs, or not at all.
    if !still_wanted(&lake.writer(), record.as_ref(), d) {
        return Ok(());
    }
    let (output, known) = match recorded {
        Some(o) if drain.answers(d, &o) => (o, true),
        _ => match backing.call(call).await {
            Ok(v) => (v, false),
            Err(e) => (json!({ "error": why(&e) }), false),
        },
    };
    record_async(lake, d, output, known).await
}

/// Make the due embeddings of one model, at most [`EMBED_BATCH`] texts, in
/// one request, and record each vector for the field it fills. A due whose
/// output is recorded meanwhile is filled from it; one whose record moved
/// on is dropped. Nothing is called when the bound backend is gone or names
/// another model or other dimensions: the fields wait. A request that
/// fails records the failure for each text it carried.
async fn settle_embeddings(lake: &Arc<Lake>, drain: &mut Drain, batch: &[Due]) -> Result<()> {
    let mut asked: Vec<&Due> = Vec::new();
    // The calls this batch makes: every due of one of them waits for its
    // answer, never for the output recorded before it, which for a retry
    // is the failure being retried.
    let mut asking: BTreeSet<Digest> = BTreeSet::new();
    for d in batch {
        if !drain.take(d) {
            continue;
        }
        let (record, recorded) = current(lake, d).await?;
        if !still_wanted(&lake.writer(), record.as_ref(), d) {
            continue;
        }
        match recorded {
            Some(o) if !asking.contains(&d.input) && drain.answers(d, &o) => {
                record_async(lake, d, o, true).await?
            }
            _ => {
                asking.insert(d.input.clone());
                asked.push(d);
            }
        }
    }
    let Some(Call::Embed {
        model, dimensions, ..
    }) = asked.first().map(|d| &d.call)
    else {
        return Ok(());
    };
    let Some(backend) = lake
        .seams
        .embed
        .as_ref()
        .filter(|b| b.model() == model && b.dimensions() == *dimensions)
        .cloned()
    else {
        return Ok(());
    };
    // One text per digest, in the order they fell due.
    let mut texts: Vec<String> = Vec::new();
    let mut at: BTreeMap<Digest, usize> = BTreeMap::new();
    for d in &asked {
        if let Call::Embed { text, .. } = &d.call
            && !at.contains_key(&d.input)
        {
            at.insert(d.input.clone(), texts.len());
            texts.push(text.clone());
        }
    }
    let outputs: Vec<Value> = match backend.embed(&texts).await {
        Ok(vectors) if vectors.len() == texts.len() => vectors
            .iter()
            .map(|v| serde_json::to_value(v).expect("a vector serialises"))
            .collect(),
        Ok(vectors) => {
            let why = format!(
                "the embedding backend `{model}` answered {} vectors for {} texts",
                vectors.len(),
                texts.len()
            );
            vec![json!({ "error": why }); texts.len()]
        }
        Err(e) => vec![json!({ "error": why(&e) }); texts.len()],
    };
    // The first due of a text records its output; a later due of the same
    // text in the batch records it again only if it fills something.
    let mut recorded: BTreeSet<Digest> = BTreeSet::new();
    for d in asked {
        let output = outputs[at[&d.input]].clone();
        let known = !recorded.insert(d.input.clone());
        record_async(lake, d, output, known).await?;
    }
    Ok(())
}

/// A backing's failure, in its own words.
pub(crate) fn why(e: &lakelands_api::Error) -> String {
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
        let staged: Recorded = [(
            (d.seam.clone(), d.input.clone()),
            vec![(at, output.clone())],
        )]
        .into();
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
                before: None,
                due: None,
            },
        )?;
        if let Some(after) = after {
            fills = true;
            let host = lake.system("lakelands");
            let call = crate::writes::Call {
                lake,
                who: &host,
                context,
                followed: None,
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
mod tests;
