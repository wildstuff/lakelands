//! Fields computed by a function whose backing reaches outside the lake.
//!
//! The fold never calls a backing. It decides, from the log alone, when
//! such a field is *due*: its record was written and the function's
//! inputs changed (a record written for the first time included), or what
//! computes it was declared again. A due field whose call has an output
//! recorded in the lake is filled from it in the same event; otherwise it
//! is cleared, and the append leaves its record [`Owed`]: the record, or
//! for a declaration every record of the type, as one sweep. After the
//! append, [`settle`] reads the calls from the records as they are then,
//! makes them a batch at a time per lake, and records each answer as
//! `output_recorded`, whose fold fills the field
//! (`docs/decisions/022-functions-are-declarations.md`, extended;
//! `docs/log-format.md` §2.5).
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
//! records each vector on its own. A batch the backend refuses for what
//! it carries is asked again in halves, so a text it refuses fails alone.
//!
//! **Failure.** A backing that fails is recorded as `{"error": "<why>"}`.
//! That output, or one that is not a value of the field's type, clears the
//! field and puts the reason in its metadata's `error`. A cleared field
//! without `error` and without metadata is one whose output is not
//! recorded yet. A settle forced by a declaration calls again for an input
//! whose recorded output is a failure; the later output holds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lakelands_api::engine::{After, ProjectionView, Scan};
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{EmbedDecl, FieldDecl, TypeDecl};
use lakelands_api::record::{FieldMeta, Origin, Patch};
use lakelands_api::seam::{EmbedBackend, FunctionCall};
use lakelands_api::write::FunctionDecl;
use lakelands_api::{
    FieldName, LakeId, Position, RecordKey, Result, Timestamp, TypeName, Value, WriteContext,
};
use lakelands_core::unavailable;
use serde_json::json;

use crate::backing::EXPR;
use crate::exec;
use crate::lake::Lake;
use crate::log::Draft;
use crate::ontology::{Entry, Ontology};

/// At most this many calls go into one batch, and at most this many texts
/// to the embedding backend in one request; a sweep reads this many
/// records a page. The drain holds one batch at a time, however many
/// records a backfill makes due, so its memory stays bounded; the backend
/// may split a batch further.
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

/// One call a record makes due: a field cleared because no output is
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
    pub context: WriteContext,
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
    /// Whether a field that a recorded output changes is handed out too:
    /// the drain reads a record so, and records that output for it.
    pub waiting: bool,
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
/// is made from. `None` for a field the fold does not compute, and for an
/// expression whose reads cannot be told. A derived expression is read as
/// its declaration is checked, in a write, where `previous` and `changed`
/// read the field they name.
pub(crate) fn computed_reads(o: &Ontology, f: &FieldDecl) -> Option<BTreeSet<FieldName>> {
    if let Some(e) = &f.embed {
        return Some(e.from.iter().cloned().collect());
    }
    if let Some(func) = f.function.as_deref().and_then(|n| o.functions.get(n)) {
        return Some(func.decl.inputs.iter().map(|i| i.name.clone()).collect());
    }
    let e = f.derived.as_ref()?;
    let e = crate::declare::condition(e, lakelands_core::expr::Place::Write, "derived").ok()?;
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
            let filled = put(
                ontology, decl, field, computer, &output, event, fields, metas,
            );
            if filled {
                changed.push(name.clone());
            }
            if !(filled && outputs.waiting) {
                continue;
            }
        } else {
            // No output yet (or a failure a declaration retries): the field
            // is cleared, without an error, until one is recorded.
            let had = fields.remove(&name).is_some();
            let had_meta = metas.remove(&name).is_some();
            if had || had_meta {
                changed.push(name.clone());
            }
            waiting.insert(name.clone());
        }
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

/// What an append left owed to the backings, until the drain has recorded
/// it: one record, or every record of a type. It holds no call: the drain
/// makes the calls from the record as it is then.
#[derive(Clone, Debug, PartialEq)]
pub struct Owed {
    /// The records' type.
    pub ty: TypeName,
    /// The event behind it, or the one that event names as its cause, as
    /// a recorded output's fill names the write the output answers. A
    /// write waits for the entries of its own causes.
    pub cause: Position,
    /// That event's context, which the outputs and what they cause carry.
    pub context: WriteContext,
    /// The record, or the sweep over the type.
    pub of: Of,
    /// How often a migration moved it: a batch read before leaves it as
    /// the migration did.
    pub moved: u32,
}

/// What an [`Owed`] entry names.
#[derive(Clone, Debug, PartialEq)]
pub enum Of {
    /// The record at this key.
    Record(RecordKey),
    /// Every record of the type, a page at a time in key order, settled as
    /// a declaration settles it: a recorded failure is called again, once
    /// in the sweep.
    Sweep {
        /// The last key whose calls are recorded; `None` before the first.
        after: Option<RecordKey>,
        /// The calls whose failure it called again.
        retried: BTreeSet<(String, Digest)>,
    },
}

impl Owed {
    /// What the event `e` leaves owed of the type `ty`.
    pub fn new(e: &Event, ty: TypeName, of: Of) -> Owed {
        let (cause, context) = (e.cause.unwrap_or(e.position), e.context);
        Owed {
            ty,
            cause,
            context,
            of,
            moved: 0,
        }
    }
}

/// The seam and digest an output is recorded under.
type Key = (String, Digest);

fn key(d: &Due) -> Key {
    (d.seam.clone(), d.input.clone())
}

/// The embedding backend bound here, when it makes the call's model and
/// dimensions.
fn embedder(lake: &Lake, call: &Call) -> Option<Arc<dyn EmbedBackend>> {
    let e = lake.seams.embed.as_ref()?;
    (model(call) == Some((e.model(), e.dimensions()))).then(|| e.clone())
}

/// Whether a backing bound here makes the call.
fn bound(lake: &Lake, call: &Call) -> bool {
    match call {
        Call::Function { kind, .. } => lake.seams.backings.iter().any(|b| b.kind() == kind),
        Call::Embed { .. } => embedder(lake, call).is_some(),
    }
}

/// The records the next page of an entry holds, by key, each with the
/// calls it owes and the output the lake holds for each; and whether the
/// page ends the entry. Read under the writer taken for a write, released
/// after the page. A record owes what [`settle_fields`] makes due over it
/// as it stands, forced in a sweep, and a waiting field whose output is
/// recorded. An entry whose type is gone ends.
#[allow(clippy::type_complexity)]
pub fn page(lake: &Lake, o: &Owed) -> Result<(Vec<(RecordKey, Vec<(Due, Option<Value>)>)>, bool)> {
    let w = lake.for_write()?;
    let view = lake.engine.view(None)?;
    let Ok(decl) = w.ontology.ty(&o.ty) else {
        return Ok((Vec::new(), true));
    };
    let (rows, end) = match &o.of {
        Of::Record(key) => (view.get(&o.ty, key)?.into_iter().collect(), true),
        Of::Sweep { after, .. } => {
            let after = after.clone().map(|key| After {
                values: vec![],
                key,
            });
            let scan = Scan {
                limit: EMBED_BATCH as u32,
                after,
                ..crate::lake::every(false)
            };
            let rows = view.scan(&o.ty, &scan)?;
            let end = rows.len() < EMBED_BATCH;
            (rows, end)
        }
    };
    let draft = Draft {
        context: o.context,
        ..Draft::system(Timestamp(0), EventBody::LakeCreated { format: 0 })
    };
    let event = crate::writes::provisional(lake, o.cause, &draft);
    let mut out = Vec::new();
    for r in rows {
        let mut due = Vec::new();
        if !r.retired && !r.is_ended() {
            let (mut fields, mut metas) = (r.fields.clone(), r.meta.clone());
            let mut outputs = Outputs {
                view: view.as_ref(),
                staged: None,
                before: None,
                due: Some(&mut due),
                waiting: true,
            };
            let sweep = matches!(o.of, Of::Sweep { .. });
            settle_fields(
                &w.ontology,
                &lake.id,
                decl,
                &r.key,
                Some(&r.fields),
                &mut fields,
                &mut metas,
                &event,
                sweep,
                &mut outputs,
            )?;
        }
        let mut owes = Vec::new();
        for d in due {
            let recorded = view.output(&d.seam, &d.input)?;
            owes.push((d, recorded));
        }
        out.push((r.key, owes));
    }
    Ok((out, end))
}

/// What a batch took of one entry: where it stood in the list and as it
/// was read, and as the batch leaves it once recorded.
struct Took {
    at: usize,
    read: Owed,
    left: Owed,
    done: bool,
}

/// One batch: calls of one model and dimensions, at most [`EMBED_BATCH`],
/// or one function call; each with the entry it came from and the output
/// the lake held for it when read.
#[derive(Default)]
struct Batch {
    took: Vec<Took>,
    calls: Vec<(usize, Due, Option<Value>)>,
}

/// The model and dimensions of an embedding's call; `None` for a function's.
fn model(c: &Call) -> Option<(&str, u32)> {
    match c {
        Call::Embed {
            model, dimensions, ..
        } => Some((model, *dimensions)),
        Call::Function { .. } => None,
    }
}

impl Batch {
    /// Whether a call joins the batch.
    fn fits(&self, d: &Due) -> bool {
        let room = self.calls.len() < EMBED_BATCH;
        (self.calls.first()).is_none_or(|(_, f, _)| {
            room && model(&f.call).is_some() && model(&f.call) == model(&d.call)
        })
    }

    /// Whether no call joins it any more.
    fn full(&self) -> bool {
        (self.calls.first())
            .is_some_and(|(_, d, _)| model(&d.call).is_none() || self.calls.len() >= EMBED_BATCH)
    }
}

/// Call the backings for what the write behind `own` owes, and record each
/// answer, a batch at a time, until no entry of its causes is left; then
/// make one batch of orphans, the entries no write waits for any more (as
/// a failed drain or a write whose caller went away leaves them), so they
/// advance by a batch with each write that owes something. The lane is
/// held for one batch: a write waits for the batch in flight, its own and
/// one more, not for the backlog another write waits for. A batch takes
/// those entries first, then the others in the order they were handed
/// out. An entry leaves the list once its batch is recorded, so a drain
/// that fails or is dropped loses nothing, and an answer is held until it
/// is recorded, so no input is sent to a backing twice by one handle. The
/// calls a fill makes due are owed to the same write; they end, since a
/// type's computed fields read one another without a cycle (refused at
/// declare).
///
/// `own` is what [`crate::lake::owing`] answered for the write. Answers
/// the lake's head read once nothing of it is left, from which its
/// computed fields are visible, whoever recorded them, whatever the
/// orphans' batch then answers; `None` when it owed nothing: the write's
/// own position stands.
pub async fn settle(lake: &Arc<Lake>, own: &BTreeSet<Position>) -> Result<Option<Position>> {
    if own.is_empty() {
        return Ok(None);
    }
    let _waits = Waits::new(lake, own);
    loop {
        let _lane = lake.calls.lock().await;
        let mine = |o: &Owed| own.contains(&o.cause);
        if lake.writer().owed.iter().any(mine) {
            let batch = take(lake, &mine).await?;
            make(lake, batch).await?;
            continue;
        }
        // The write's own calls are recorded, and visible from this head.
        // The orphans' batch is the drain's: its failure is not the write's.
        let head = lake.head();
        let waited = lakelands_core::lock(&lake.waiting).clone();
        let orphan = |o: &Owed| !waited.contains_key(&o.cause);
        if lake.writer().owed.iter().any(orphan)
            && let Ok(batch) = take(lake, &orphan).await
        {
            let _ = make(lake, batch).await;
        }
        return Ok(Some(head));
    }
}

/// A write waiting in [`settle`] for its causes, until it ends or is
/// dropped: while it waits, their entries are no orphans.
struct Waits<'a>(&'a Lake, &'a BTreeSet<Position>);

impl<'a> Waits<'a> {
    fn new(lake: &'a Lake, own: &'a BTreeSet<Position>) -> Waits<'a> {
        let mut waiting = lakelands_core::lock(&lake.waiting);
        for p in own {
            *waiting.entry(*p).or_default() += 1;
        }
        Waits(lake, own)
    }
}

impl Drop for Waits<'_> {
    fn drop(&mut self) {
        let mut waiting = lakelands_core::lock(&self.0.waiting);
        for p in self.1 {
            waiting.entry(*p).and_modify(|n| *n -= 1);
        }
        waiting.retain(|_, n| *n > 0);
    }
}

/// [`settle`] after a write that is folded: a drain that fails is not that
/// write's failure. The write is answered as itself, its computed fields
/// waiting. The failure is not lost: where a recording's fold failed the
/// writer is behind, and the next write catches up first or is answered
/// `unavailable` in that fold's words; what was not recorded stays owed on
/// this handle, its answer held, and the next write's drain records it.
pub async fn settle_after(lake: &Arc<Lake>, own: &BTreeSet<Position>) -> Option<Position> {
    settle(lake, own).await.unwrap_or(None)
}

/// Read the next batch: the entries `ours` holds first, then the others,
/// entry by entry, a page each, until it is full. A record whose calls do
/// not all join stops its entry there; the batch goes on with the next.
/// A call no backing bound here makes is left out, unless its output is
/// recorded and not a failure to call again: its field waits.
async fn take(lake: &Arc<Lake>, ours: &impl Fn(&Owed) -> bool) -> Result<Batch> {
    let entries: Vec<(usize, Owed)> = {
        let w = lake.writer();
        let all = w.owed.iter().cloned().enumerate();
        let (mine, rest): (Vec<_>, Vec<_>) = all.partition(|(_, o)| ours(o));
        mine.into_iter().chain(rest).collect()
    };
    let mut batch = Batch::default();
    for (at, read) in entries {
        let (l, o) = (lake.clone(), read.clone());
        let (rows, end) = exec::run(move || page(&l, &o)).await?;
        let (t, mut left, mut whole) = (batch.took.len(), read.clone(), true);
        for (key, calls) in rows {
            for (d, recorded) in calls {
                if !bound(lake, &d.call) && (recorded.is_none() || d.retry) {
                    continue;
                }
                match batch.fits(&d) {
                    true => batch.calls.push((t, d, recorded)),
                    false => whole = false,
                }
            }
            if !whole {
                break;
            }
            if let Of::Sweep { after, .. } = &mut left.of {
                *after = Some(key);
            }
        }
        let done = end && whole;
        batch.took.push(Took {
            at,
            read,
            left,
            done,
        });
        if batch.full() {
            break;
        }
    }
    Ok(batch)
}

/// Make a batch's calls and record each answer, then leave its entries as
/// it took them, unless a migration moved one meanwhile. A call is
/// answered by the answer held for it, else by the output recorded, else
/// by its backing; a sweep calls a failure again, recorded or held, once,
/// and notes the call in its entry before it is made. Every answer is
/// held before the first is recorded, and until its first recording.
async fn make(lake: &Arc<Lake>, batch: Batch) -> Result<()> {
    let Batch { mut took, calls } = batch;
    let held: BTreeMap<Key, Value> = {
        let w = lake.writer();
        let held = calls.iter().map(|(_, d, _)| key(d));
        held.filter_map(|k| Some((k.clone(), w.held.get(&k)?.clone())))
            .collect()
    };
    let mut answers: BTreeMap<Key, Value> = BTreeMap::new();
    // The calls whose answer the lake holds already.
    let mut known: BTreeSet<Key> = BTreeSet::new();
    let mut ask: Vec<&Due> = Vec::new();
    for (t, d, recorded) in &calls {
        let k = key(d);
        if answers.contains_key(&k) || ask.iter().any(|a| key(a) == k) {
            continue;
        }
        let retried = match &mut took[*t].left.of {
            Of::Sweep { retried, .. } => Some(retried),
            Of::Record(_) => None,
        };
        let again = retried.as_ref().is_some_and(|r| !r.contains(&k));
        let usable = |v: &&Value| !(again && is_failure(v));
        let answer = held.get(&k).filter(usable);
        match answer.or(recorded.as_ref().filter(usable)) {
            Some(v) => {
                if recorded.as_ref() == Some(v) {
                    known.insert(k.clone());
                }
                answers.insert(k, v.clone());
            }
            None => {
                if let (true, Some(r)) = (again, retried) {
                    r.insert(k);
                }
                ask.push(d);
            }
        }
    }
    {
        let mut w = lake.writer();
        for t in &mut took {
            let Of::Sweep { retried, .. } = &t.left.of else {
                continue;
            };
            if let Some(o) = w.owed.get_mut(t.at).filter(|o| **o == t.read)
                && let Of::Sweep { retried: noted, .. } = &mut o.of
            {
                noted.clone_from(retried);
                t.read = o.clone();
            }
        }
    }
    let asked: Vec<(Key, Value)> = match ask.first().map(|d| &d.call) {
        None => Vec::new(),
        Some(c @ Call::Embed { .. }) => {
            let backend = embedder(lake, c);
            let texts: Vec<String> = (ask.iter())
                .filter_map(|d| match &d.call {
                    Call::Embed { text, .. } => Some(text.clone()),
                    Call::Function { .. } => None,
                })
                .collect();
            match backend {
                Some(b) => (ask.iter().map(|d| key(d)))
                    .zip(embed_each(b.as_ref(), &texts).await)
                    .collect(),
                None => Vec::new(),
            }
        }
        Some(Call::Function { kind, call }) => {
            match lake.seams.backings.iter().find(|b| b.kind() == kind) {
                Some(b) => {
                    let output =
                        (b.call(call).await).unwrap_or_else(|e| json!({ "error": why(&e) }));
                    vec![(key(ask[0]), output)]
                }
                None => Vec::new(),
            }
        }
    };
    lake.writer().held.extend(asked.iter().cloned());
    answers.extend(asked);
    for (_, d, _) in &calls {
        let k = key(d);
        if let Some(output) = answers.get(&k) {
            let output = output.clone();
            record_async(lake, d, output, known.contains(&k)).await?;
            lake.writer().held.remove(&k);
            known.insert(k);
        }
    }
    let mut w = lake.writer();
    took.sort_by_key(|t| std::cmp::Reverse(t.at));
    for t in took {
        if w.owed.get(t.at) != Some(&t.read) {
            continue;
        }
        match t.done {
            true => drop(w.owed.remove(t.at)),
            false => w.owed[t.at] = t.left,
        }
    }
    Ok(())
}

async fn record_async(lake: &Arc<Lake>, d: &Due, output: Value, known: bool) -> Result<()> {
    let (l, d) = (lake.clone(), d.clone());
    exec::run(move || record(&l, d, output, known)).await?;
    Ok(())
}

/// The output of each text, in order. A request of several texts that the
/// backend refuses for what it carries (`Invalid`) is asked again as two
/// halves, the first first, so only a text refused alone records the
/// refusal. Any other failure, and an answer with the wrong number of
/// vectors, is weather: it ends the asking and is the output of every
/// text not answered yet.
async fn embed_each(backend: &dyn EmbedBackend, texts: &[String]) -> Vec<Value> {
    use lakelands_api::{Error, Refusal};
    let mut out: Vec<Option<Value>> = vec![None; texts.len()];
    // The spans still to ask, the next on top.
    let mut spans: Vec<_> = std::iter::once(0..texts.len()).collect();
    let mut weather = Value::Null;
    while let Some(span) = spans.pop() {
        let (from, to) = (span.start, span.end);
        let answer = backend.embed(&texts[span.clone()]).await.and_then(|v| {
            if v.len() == to - from {
                return Ok(v);
            }
            Err(unavailable(format!(
                "the embedding backend `{}` answered {} vectors for {} texts",
                backend.model(),
                v.len(),
                to - from
            )))
        });
        match answer {
            Ok(vectors) => {
                for (slot, v) in out[span].iter_mut().zip(vectors) {
                    *slot = Some(serde_json::to_value(v).expect("a vector serialises"));
                }
            }
            Err(
                e @ Error::Refused {
                    reason: Refusal::Invalid { .. },
                    ..
                },
            ) => {
                let half = from + (to - from) / 2;
                if half == from {
                    out[from] = Some(json!({ "error": why(&e) }));
                } else {
                    spans.extend([half..to, from..half]);
                }
            }
            Err(e) => {
                weather = json!({ "error": why(&e) });
                break;
            }
        }
    }
    out.into_iter()
        .map(|o| o.unwrap_or_else(|| weather.clone()))
        .collect()
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
    let mut w = lake.for_write()?;
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
                waiting: false,
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
