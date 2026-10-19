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
//! **Failure.** A backing that fails is recorded as `{"error": "<why>"}`,
//! and so is one whose output holds an integer beyond ±(2^53 − 1), which
//! the log does not write.
//! That output, or one that is not a value of the field's type, clears the
//! field and puts the reason in its metadata's `error`. A cleared field
//! without `error` and without metadata is one whose output is not
//! recorded yet. A settle forced by a declaration calls again for an input
//! whose recorded output is a failure; the later output holds. One forced
//! by a value type's rename does not: it makes the calls the rename moved.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lakelands_api::engine::{After, ProjectionView, Scan};
use lakelands_api::error::{Case, Exchange, Flaw, Part, Remote, Situation};
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{EmbedDecl, FieldDecl, TypeDecl};
use lakelands_api::record::{FieldMeta, Origin, Patch};
use lakelands_api::seam::{Accepted, CallCause, EmbedBackend, FunctionCall};
use lakelands_api::write::{FunctionDecl, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{
    FieldName, LakeId, Position, ProposalId, RecordKey, Result, SourceId, Timestamp, TypeName,
    Value, WriteContext,
};
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
        call: Box<FunctionCall>,
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
    /// The write that made the record ([`on_record`]).
    pub made: Position,
    /// The field the output fills.
    pub field: FieldName,
    /// What computes it, as the log names its procedure.
    pub procedure: Procedure,
    /// The seam the output is recorded under.
    pub seam: String,
    /// The call's digest; of a function above `low`, on its record
    /// ([`on_record`]).
    pub input: Digest,
    /// The key its output is sealed under, where the call reads a source's
    /// erasable values (058).
    pub sealed: Option<lakelands_api::event::Sealed>,
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
    /// The plans the records are computed by.
    pub plans: &'a crate::record::Plans,
    /// The sources' keys, where the reader has the writer's: a call that
    /// reads a source's erasable values is digested under its key (058).
    pub keys: Option<&'a crate::sealing::Keys>,
}

/// The outputs one transaction recorded, by seam and input: every
/// recording with its position, in the order of the log.
pub type Recorded = BTreeMap<(String, Digest), Vec<(Position, Value)>>;

impl Outputs<'_> {
    /// The outputs a plan reads: what `view` holds, and no call made due.
    pub(crate) fn planning<'a>(
        view: &'a dyn ProjectionView,
        plans: &'a crate::record::Plans,
        keys: &'a crate::sealing::Keys,
    ) -> Outputs<'a> {
        let (staged, before, due, waiting) = (None, None, None, false);
        Outputs {
            view,
            staged,
            before,
            due,
            waiting,
            plans,
            keys: Some(keys),
        }
    }

    /// The list [`crate::sealing::USED`] keeps for `source`: the
    /// transaction's last the reader may read, else the view's, read once
    /// a position where the reader has the keys ([`Keys::listed`]).
    ///
    /// [`Keys::listed`]: crate::sealing::Keys::listed
    fn used(&self, source: &lakelands_api::SourceId) -> Result<Option<Arc<crate::sealing::List>>> {
        use crate::sealing::{List, USED, used_by};
        let input = used_by(source);
        let staged = (self.staged).and_then(|s| s.get(&(USED.to_owned(), input.clone())));
        let read = |(at, _): &&(Position, Value)| self.before.is_none_or(|before| *at < before);
        if let Some((_, v)) = staged.and_then(|all| all.iter().rev().find(read)) {
            return Ok(Some(Arc::new(List::of(v.clone()))));
        }
        match self.keys {
            Some(keys) => keys.listed(self.view, source),
            None => Ok((self.view.output(USED, &input)?).map(|(_, v)| Arc::new(List::of(v)))),
        }
    }

    /// What the lake recorded for a call, if anything: the transaction's
    /// last recording the reader may read, else the view's.
    fn recorded(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        let staged = self
            .staged
            .and_then(|s| s.get(&(seam.to_owned(), input.clone())));
        let read = |(at, _): &&(Position, Value)| self.before.is_none_or(|before| *at < before);
        match staged.and_then(|all| all.iter().rev().find(read)) {
            Some((_, v)) => Ok(Some(v.clone())),
            None => Ok(self.view.output(seam, input)?.map(|(_, v)| v)),
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
                i.name().to_string(),
                fields.get(i.name()).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    json!({ "function": f.name, "inputs": inputs, "config": f.config })
}

/// The digest of a call: SHA-256 over its canonical bytes.
pub fn digest(call: &Value) -> Digest {
    Digest(lakelands_core::canonical::sha256_hex(call))
}

/// The digest of a call that waits for a decision, on the record that
/// makes it: SHA-256 over the canonical bytes of `{ "call", "record":
/// [type, key], "made" }`, where `made` is the position of the write that
/// made the record ([`StoredRecord::made`](lakelands_api::record::StoredRecord::made)).
/// Such a call is its record's, so its proposal, its decision and its
/// output reach no other record that makes the same call, a record made
/// again at a retired key included (064).
pub fn on_record(call: &Digest, ty: &TypeName, (key, made): (&RecordKey, Position)) -> Digest {
    digest(&json!({ "call": call, "record": [ty, key], "made": made }))
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
            let (function, field, cause) = (function.to_owned(), field.name.clone(), Box::new(e));
            Case::OutputNotValue {
                function,
                field,
                cause,
            }
            .to_string()
        })?;
    match checked.into_iter().next() {
        Some((_, v)) if !v.is_null() => Ok(v),
        _ => Err(Case::NoOutputValue {
            function: function.to_owned(),
        }
        .to_string()),
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

    /// Whether its call waits for a decision: a function above `low`.
    fn decided(&self) -> bool {
        matches!(self, Computer::Function(f) if !f.decl.risk.is_low())
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

/// What a call's digest is (058).
enum Input {
    /// SHA-256 of its canonical bytes: it reads no erasable value.
    Plain(Digest),
    /// HMAC under the key of the one source whose erasable values it
    /// reads, and that key's version, which seals its output.
    Sealed(Digest, lakelands_api::event::Sealed),
    /// It reads erasable values of two sources: no output is recorded.
    Two,
    /// It reads a source's erasable values and the reader holds no keys.
    Keyless,
}

/// Why a field computed from two sources' erasable values holds none.
const TWO_SOURCES: &str = "its inputs hold erasable values of two sources, and an output is sealed under one source's key";

/// The digest of `call`, as `computer` makes it from `fields`: by the
/// sources of the erasable values it reads, through the fields it reads
/// and those they are derived from. One source's are digested under the
/// version of its key `version` names, a recorded output's own, else the
/// oldest of those that sealed the values it reads: the last first used
/// at or before each value's position ([`crate::sealing::USED`]), as the
/// fold keeps it and a write's plan stages it for its own events. A value
/// written in plaintext, before its field was erasable, counts as sealed
/// by the version in force at its position, which only ever makes an
/// output die sooner, and as none before any; where no value names one,
/// the first is taken. So the digest is a
/// function of the log, whatever the key store holds now, and the output
/// lives as long as the oldest key period it read: destroying a version,
/// and every older one with it, takes the outputs of its values. A source
/// that sealed nothing has no key to digest under.
fn input_of(
    (decl, computer): (&TypeDecl, &Computer<'_>),
    (fields, metas): (&BTreeMap<FieldName, Value>, &BTreeMap<FieldName, FieldMeta>),
    (outputs, event): (&Outputs<'_>, &Event),
    version: Option<&lakelands_api::id::KeyId>,
    call: &Value,
) -> Result<Input> {
    let mut todo: Vec<&FieldName> = match computer {
        Computer::Function(f) => f.decl.inputs.iter().map(|i| i.name()).collect(),
        Computer::Embed(e, _) => e.from.iter().collect(),
    };
    let (mut seen, mut sources, mut read) = (BTreeSet::new(), BTreeSet::new(), Vec::new());
    while let Some(n) = todo.pop() {
        let Some(f) = decl
            .fields
            .iter()
            .find(|f| &f.name == n)
            .filter(|_| seen.insert(n))
        else {
            continue;
        };
        if f.erasable && fields.contains_key(n) {
            let m = metas.get(n).filter(|m| m.source.is_some());
            sources.extend(m.and_then(|m| m.source.clone()));
            read.extend(m.map(|m| m.position));
        }
        todo.extend(&f.depends_on);
    }
    let mut sources = sources.into_iter();
    let (source, two) = (sources.next(), sources.next().is_some());
    let Some(source) = source.filter(|_| !two) else {
        return Ok(if two {
            Input::Two
        } else {
            Input::Plain(digest(call))
        });
    };
    let list = outputs.used(&source)?;
    let firsts = list.as_ref().map_or(&[][..], |l| &l.firsts[..]);
    let sealing = (event.sealed.as_ref())
        .filter(|_| matches!(event.body, EventBody::RecordIngested { .. }))
        .filter(|s| s.source.as_ref().or(event.source.as_ref()) == Some(&source))
        .map(|s| &s.key);
    let rank = |k: &&lakelands_api::id::KeyId| firsts.iter().position(|(f, _)| f == *k);
    let oldest = (read.iter())
        .filter_map(|p| crate::sealing::sealed_at(firsts, *p))
        .min_by_key(rank);
    let first = firsts.first().map(|(k, _)| k);
    let Some(key) = version.or(oldest).or(first).or(sealing).cloned() else {
        return Ok(Input::Plain(digest(call)));
    };
    let Some(keys) = outputs.keys else {
        return Ok(Input::Keyless);
    };
    let k = keys.key(&source, &key)?;
    let source = Some(source);
    let sealed = lakelands_api::event::Sealed {
        key,
        source,
        fields: Vec::new(),
    };
    Ok(Input::Sealed(crate::sealing::digest(&k, call), sealed))
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
        return Some(func.decl.inputs.iter().map(|i| i.name().clone()).collect());
    }
    let e = f.derived.as_ref()?;
    let e = crate::declare::condition(e, lakelands_core::expr::Place::Write, &[]).ok()?;
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
) -> std::result::Result<Vec<(FieldName, BTreeSet<FieldName>)>, Case> {
    let mut waiting: Vec<(FieldName, BTreeSet<FieldName>)> = t
        .fields
        .iter()
        .filter_map(|f| Some((f.name.clone(), computed_reads(o, f)?)))
        .collect();
    if let Some((n, _)) = waiting.iter().find(|(n, reads)| reads.contains(n)) {
        return Err(Case::ComputedReadsItself { field: n.clone() });
    }
    let computed: BTreeSet<FieldName> = waiting.iter().map(|(n, _)| n.clone()).collect();
    for (_, reads) in &mut waiting {
        reads.retain(|r| computed.contains(r));
    }
    let mut done: BTreeSet<FieldName> = BTreeSet::new();
    let mut out = Vec::new();
    while !waiting.is_empty() {
        let Some(i) = waiting.iter().position(|(_, reads)| reads.is_subset(&done)) else {
            let fields = waiting.iter().map(|(n, _)| n).collect();
            return Err(Case::ComputedCycle { fields });
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
/// recorded failure does not fill it: the call is made due again, unless
/// `event` is a value type's rename, which retries nothing.
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
    (key, made): (&RecordKey, Position),
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
    let plan = outputs.plans.get(ontology, decl);
    let order = plan.settled.as_ref().map_err(|why| {
        let (ty, cause) = (
            decl.name.clone(),
            Box::new(lakelands_core::invalid(why.clone())),
        );
        lakelands_core::damaged(Part::Projection, None, Flaw::CannotOrder { ty, cause })
    })?;
    let mut waiting: BTreeSet<FieldName> = BTreeSet::new();
    for (name, reads) in order {
        let name = name.clone();
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
        let input = input_of(
            (decl, &computer),
            (fields, metas),
            (outputs, event),
            None,
            &call,
        )?;
        let (input, sealed) = match input {
            Input::Plain(input) => (input, None),
            Input::Sealed(input, sealed) => (input, Some(sealed)),
            two @ (Input::Two | Input::Keyless) => {
                fields.remove(&name);
                let error = matches!(two, Input::Two).then(|| TWO_SOURCES.to_owned());
                metas.insert(name.clone(), meta(event, computer.version(), error));
                changed.push(name.clone());
                continue;
            }
        };
        // A call that waits for a decision is its record's (064). An output
        // recorded under the call alone, while the function was `low`, is
        // read for a record whose value stands on it, and for no record
        // that makes the call anew: that would tell of another that made it.
        let (input, plain) = match computer.decided() {
            true => (on_record(&input, &decl.name, (key, made)), Some(input)),
            false => (input, None),
        };
        let mut recorded = outputs.recorded(&seam, &input)?;
        if let (None, Some(plain), false, false) = (&recorded, &plain, moved, unsettled) {
            recorded = outputs.recorded(&seam, plain)?;
        }
        // A value type's rename is forced so that a call it moved is
        // seen, and calls no recorded failure again: that call is as it was.
        let renames = matches!(event.body, EventBody::ValueTypeMigrated { .. });
        let retry = force && !renames && recorded.as_ref().is_some_and(is_failure);
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
                    call: Box::new(FunctionCall {
                        lake: lake.clone(),
                        function: f.decl.name.clone(),
                        inputs: call["inputs"].clone(),
                        config: f.decl.config.clone(),
                        cause: None,
                    }),
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
                made,
                field: name,
                procedure: Procedure {
                    name: computer.name().to_owned(),
                    version: computer.version(),
                },
                seam,
                input,
                sealed,
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
/// recorded digest now, on the record `key` made at `made` where the call
/// waits for a decision ([`on_record`]). Answers whether the field changed.
#[allow(clippy::too_many_arguments)]
pub fn put_output(
    ontology: &Ontology,
    decl: &TypeDecl,
    (key, made, field): (&RecordKey, Position, &FieldName),
    seam_name: &str,
    input: &Digest,
    output: &Value,
    event: &Event,
    fields: &mut BTreeMap<FieldName, Value>,
    metas: &mut BTreeMap<FieldName, FieldMeta>,
    outputs: &Outputs<'_>,
) -> Result<bool> {
    let Some((f, computer)) = backed_fields(ontology, decl).find(|(f, _)| &f.name == field) else {
        return Ok(false);
    };
    let Some(call) = computer
        .call(fields)
        .filter(|_| computer.seam() == seam_name)
    else {
        return Ok(false);
    };
    // Digested under the version the output names, as it was recorded.
    let version = event.sealed.as_ref().map(|s| &s.key);
    let now = match input_of(
        (decl, &computer),
        (fields, metas),
        (outputs, event),
        version,
        &call,
    )? {
        Input::Plain(d) | Input::Sealed(d, _) => d,
        Input::Two | Input::Keyless => return Ok(false),
    };
    let decided = computer
        .decided()
        .then(|| on_record(&now, &decl.name, (key, made)));
    if &now != input && decided.as_ref() != Some(input) {
        return Ok(false);
    }
    Ok(put(
        ontology, decl, f, computer, output, event, fields, metas,
    ))
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
    /// The event that set the calls off, as a backing is told of it.
    pub by: By,
    /// A reviewer's decision this entry carries out: the calls of one
    /// function above `low` it makes, or records as rejected (064).
    pub decided: Option<Decided>,
    /// Of a sweep: the calls of each function above `low` it found owed,
    /// which it proposes as one when it ends: by function, how many and
    /// the first keys.
    pub proposing: BTreeMap<String, (u64, Vec<RecordKey>)>,
}

/// The event that set an entry's calls off, as it names itself.
#[derive(Clone, Debug, PartialEq)]
pub struct By {
    /// Its position.
    pub at: Position,
    /// Its writer.
    pub writer: String,
    /// Its source, where it came from one.
    pub source: Option<SourceId>,
}

/// A reviewer's decision on a proposed call of a function above `low`.
#[derive(Clone, Debug, PartialEq)]
pub struct Decided {
    /// The function.
    pub function: String,
    /// The one call it decides; `None` for every call of a sweep.
    pub only: Option<Digest>,
    /// The decision's position.
    pub position: Position,
    /// Who decided.
    pub reviewer: String,
    /// The reason, where it rejected the calls.
    pub rejected: Option<String>,
}

/// How many keys a sweep's proposal names, the first in key order.
pub const FIRST_KEYS: usize = 10;

/// What an [`Owed`] entry names.
#[derive(Clone, Debug, PartialEq)]
pub enum Of {
    /// The record at this key.
    Record(RecordKey),
    /// Every record of the type, a page at a time in key order. After a
    /// declaration it is settled as the declaration settles it: a recorded
    /// failure is called again, once in the sweep.
    Sweep {
        /// The last key whose calls are recorded; `None` before the first.
        after: Option<RecordKey>,
        /// The calls whose failure it called again; `None` after a value
        /// type's rename, whose sweep makes the waiting calls and no other.
        retried: Option<BTreeSet<(String, Digest)>>,
    },
}

impl Owed {
    /// What the event `e` leaves owed of the type `ty`.
    pub fn new(e: &Event, ty: TypeName, of: Of) -> Owed {
        let (cause, context) = (e.cause.unwrap_or(e.position), e.context);
        let by = By {
            at: e.position,
            writer: e.writer.clone(),
            source: e.source.clone(),
        };
        Owed {
            ty,
            cause,
            context,
            of,
            moved: 0,
            by,
            decided: None,
            proposing: BTreeMap::new(),
        }
    }

    /// What the decision `e` on a proposed call leaves owed: the call, made
    /// or recorded as rejected, set off by the event `by`, the one the
    /// proposal names as its cause. Of a `compute` its record alone, since
    /// a decision holds for its record; of a sweep every record of its type
    /// the sweep owes a call. None for any other proposal (064).
    pub fn decided(e: &Event, write: &ProposedWrite, by: &Event) -> Option<Owed> {
        let (ty, function, only, of) = match write {
            ProposedWrite::Compute {
                ty,
                key,
                function,
                input,
                ..
            } => (ty, function, Some(input.clone()), Of::Record(key.clone())),
            ProposedWrite::ComputeSweep { ty, function, .. } => {
                let of = Of::Sweep {
                    after: None,
                    retried: Some(BTreeSet::new()),
                };
                (ty, function, None, of)
            }
            _ => return None,
        };
        let rejected = match &e.body {
            EventBody::WriteRejected { reason, .. } => Some(reason.clone()),
            _ => None,
        };
        let mut o = Owed::new(e, ty.clone(), of);
        o.by = By {
            at: by.position,
            writer: by.writer.clone(),
            source: by.source.clone(),
        };
        o.decided = Some(Decided {
            function: function.clone(),
            only,
            position: e.position,
            reviewer: e.writer.clone(),
            rejected,
        });
        Some(o)
    }

    /// Whether this entry carries out a decision on the call `d`.
    fn decides(&self, d: &Due) -> Option<&Decided> {
        let decided = self.decided.as_ref()?;
        let called =
            matches!(&d.call, Call::Function { call, .. } if call.function == decided.function);
        let one = decided.only.as_ref().is_none_or(|i| *i == d.input);
        (called && one).then_some(decided)
    }
}

/// The calls a record makes now, by the field each fills, made or not:
/// what an accept or a rejection compares a proposed call with (064). A
/// retired record makes none.
pub(crate) fn call_now(
    lake: &Lake,
    w: &crate::lake::Writer,
    view: &dyn ProjectionView,
    ty: &TypeName,
    record: Option<&lakelands_api::record::StoredRecord>,
) -> Result<Vec<(FieldName, Digest)>> {
    let (Some(r), Ok(decl)) = (record, w.ontology.ty(ty)) else {
        return Ok(Vec::new());
    };
    if r.retired || r.is_ended() {
        return Ok(Vec::new());
    }
    let draft = Draft::system(Timestamp(0), EventBody::LakeCreated { format: 0 });
    let event = crate::writes::provisional(lake, r.position, &draft);
    let (mut fields, mut metas, mut due) = (r.fields.clone(), r.meta.clone(), Vec::new());
    // Its computed fields settled from nothing, so a call whose output the
    // record holds already is listed as one that waits.
    for (f, _) in backed_fields(&w.ontology, decl) {
        fields.remove(&f.name);
        metas.remove(&f.name);
    }
    let mut outputs = Outputs {
        view,
        staged: None,
        before: None,
        due: Some(&mut due),
        waiting: true,
        plans: &w.plans,
        keys: Some(&lake.keys),
    };
    let (o, lake_id) = (&w.ontology, &lake.id);
    let (key, before) = ((&r.key, r.made), Some(&r.fields));
    settle_fields(
        o,
        lake_id,
        decl,
        key,
        before,
        &mut fields,
        &mut metas,
        &event,
        true,
        &mut outputs,
    )?;
    Ok(due.into_iter().map(|d| (d.field, d.input)).collect())
}

/// The seam and digest an output is recorded under.
type Key = (String, Digest);

fn key(d: &Due) -> Key {
    (d.seam.clone(), d.input.clone())
}

/// The embedding backend bound here, when it makes the call's model and
/// dimensions.
fn embedder(lake: &Lake, call: &Call) -> Option<Arc<dyn EmbedBackend>> {
    let e = lake.embed()?;
    (model(call) == Some((e.model(), e.dimensions()))).then_some(e)
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
                plans: &w.plans,
                keys: Some(&lake.keys),
            };
            let sweep = matches!(
                o.of,
                Of::Sweep {
                    retried: Some(_),
                    ..
                }
            );
            settle_fields(
                &w.ontology,
                &lake.id,
                decl,
                (&r.key, r.made),
                Some(&r.fields),
                &mut fields,
                &mut metas,
                &event,
                sweep,
                &mut outputs,
            )?;
        }
        let mut owes = Vec::new();
        for mut d in due {
            let recorded = view.output(&d.seam, &d.input)?.map(|(_, v)| v);
            let accepted = o.decides(&d).filter(|x| x.rejected.is_none());
            let accepted = accepted.map(|x| Accepted {
                position: x.position,
                reviewer: x.reviewer.clone(),
            });
            if let Call::Function { call, .. } = &mut d.call {
                call.cause = Some(CallCause {
                    position: o.by.at,
                    writer: o.by.writer.clone(),
                    ty: d.ty.clone(),
                    key: d.key.clone(),
                    source: o.by.source.clone(),
                    context: o.context,
                    accepted,
                });
            }
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
    /// The calls of a function above `low` it proposes, one record's.
    propose: Vec<Due>,
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
    let risky: BTreeSet<String> = (lake.writer().ontology.functions.iter())
        .filter(|(_, f)| !f.decl.risk.is_low())
        .map(|(name, _)| name.clone())
        .collect();
    let mut batch = Batch::default();
    for (at, read) in entries {
        let (l, o) = (lake.clone(), read.clone());
        let (rows, end) = exec::run(move || page(&l, &o)).await?;
        let (t, mut left, mut whole) = (batch.took.len(), read.clone(), true);
        let mut propose = Vec::new();
        for (key, calls) in rows {
            for (d, recorded) in calls {
                let function = match &d.call {
                    Call::Function { call, .. } => Some(&call.function),
                    Call::Embed { .. } => None,
                };
                // A decision is carried out on the calls it decides alone.
                if read.decided.is_some() && read.decides(&d).is_none() {
                    continue;
                }
                // A call of a function above `low` that is not made yet is
                // proposed, unless a decision on it is carried out (064).
                let asks = recorded.is_none() || d.retry;
                if asks && function.is_some_and(|f| risky.contains(f)) && read.decides(&d).is_none()
                {
                    let f = function.cloned().unwrap_or_default();
                    match &read.of {
                        Of::Record(_) => propose.push(d),
                        Of::Sweep { .. } => {
                            let (count, first) = left.proposing.entry(f).or_default();
                            *count += 1;
                            if first.len() < FIRST_KEYS && !first.contains(&d.key) {
                                first.push(d.key.clone());
                            }
                        }
                    }
                    continue;
                }
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
            propose,
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
        // A rejected call is answered so, and no backing is asked (064).
        if let Some(why) = took[*t].read.decides(d).and_then(|x| x.rejected.as_ref()) {
            answers.insert(k, rejected(why));
            continue;
        }
        let retried = match &mut took[*t].left.of {
            Of::Sweep { retried, .. } => retried.as_mut(),
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
                        (b.call(call).await).unwrap_or_else(|e| json!({ "error": e.said() }));
                    vec![(key(ask[0]), output)]
                }
                None => Vec::new(),
            }
        }
    };
    let asked: Vec<(Key, Value)> = asked.into_iter().map(|(k, v)| (k, written(v))).collect();
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
    propose(lake, &took).await?;
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

/// The output a rejected call is recorded with (064).
pub(crate) fn rejected(why: &str) -> Value {
    json!({ "error": format!("rejected: {why}") })
}

/// The `write_rejected` of each pending proposal of the calls `inputs`
/// under `seam`, by the system, caused by `at`: an output recorded for a
/// call decides it, made or rejected, so none of them waits on. A call that
/// waits for a decision is digested on its record ([`on_record`]), so
/// only that record's proposal is closed (064).
pub(crate) fn closed(
    view: &dyn ProjectionView,
    (seam, inputs): (&str, &[Digest]),
    (now, at): (Timestamp, Position),
) -> Result<Vec<Draft>> {
    if !seam.starts_with("function:") {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for p in view.proposals(&pending_computes())? {
        let call =
            matches!(&p.write, Some(ProposedWrite::Compute { input: i, .. }) if inputs.contains(i));
        if call {
            out.push(closing(p.id, "its call's output is recorded", (now, at)));
        }
    }
    Ok(out)
}

/// The pending proposals of records' calls, by record: what a record's
/// retirement, or its move by a re-key, closes ([`closing`]).
pub(crate) fn pending_calls(
    view: &dyn ProjectionView,
) -> Result<BTreeMap<(TypeName, RecordKey), Vec<ProposalId>>> {
    let mut out: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for p in view.proposals(&pending_computes())? {
        if let Some(ProposedWrite::Compute { ty, key, .. }) = p.write {
            out.entry((ty, key)).or_default().push(p.id);
        }
    }
    Ok(out)
}

/// What [`closed`] and [`pending_calls`] read: the pending `compute`s.
fn pending_computes() -> ProposalFilter {
    ProposalFilter {
        state: Some(ProposalState::Pending),
        kind: Some(lakelands_api::write::ProposalKind::Compute),
        ..Default::default()
    }
}

/// The system's `write_rejected` of `proposal`, caused by `at`. A
/// retired record's pending calls, and a moved record's, are closed by it
/// in the append of the event that ends them: a record made again at the
/// key is another, whose calls are its own ([`on_record`]), and a moved
/// record's calls are digested on its new key (064).
pub(crate) fn closing(
    proposal: ProposalId,
    reason: &str,
    (now, at): (Timestamp, Position),
) -> Draft {
    let reason = reason.to_owned();
    let body = EventBody::WriteRejected { proposal, reason };
    Draft {
        cause: Some(at),
        ..Draft::system(now, body)
    }
}

/// Propose what a batch found of functions above `low`, in one append:
/// each record's call that no pending proposal holds already, and a
/// sweep's calls as one once it ends (064). The proposer is the writer
/// that set them off, and the proposal is caused by its event.
async fn propose(lake: &Arc<Lake>, took: &[Took]) -> Result<()> {
    let ends = |t: &&Took| t.done && !t.left.proposing.is_empty();
    if took.iter().all(|t| t.propose.is_empty()) && !took.iter().any(|t| ends(&t)) {
        return Ok(());
    }
    let (l, took) = (lake.clone(), took.iter().map(Took::proposals).collect());
    exec::run(move || proposed(&l, took)).await
}

impl Took {
    /// What it proposes: the writes, each with the entry it came from, and
    /// a record's call with the write that made the record.
    fn proposals(&self) -> (Owed, Vec<Proposing>) {
        let mut out = Vec::new();
        for d in &self.propose {
            if let Call::Function { call, .. } = &d.call {
                let write = ProposedWrite::Compute {
                    ty: d.ty.clone(),
                    key: d.key.clone(),
                    field: d.field.clone(),
                    function: call.function.clone(),
                    inputs: None,
                    input: d.input.clone(),
                };
                out.push((write, Some(d.made)));
            }
        }
        for (function, (count, first)) in self.left.proposing.iter().filter(|_| self.done) {
            let write = ProposedWrite::ComputeSweep {
                ty: self.left.ty.clone(),
                function: function.clone(),
                count: *count,
                first: first.clone(),
            };
            out.push((write, None));
        }
        (self.read.clone(), out)
    }
}

/// A write to propose, and for a record's call the write that made the
/// record.
type Proposing = (ProposedWrite, Option<Position>);

fn proposed(lake: &Lake, took: Vec<(Owed, Vec<Proposing>)>) -> Result<()> {
    let mut w = lake.for_write()?;
    let view = lake.engine.view(None)?;
    // A record's call a pending proposal holds is not proposed again; the
    // same call on another record is (064).
    let pending = ProposalFilter {
        state: Some(ProposalState::Pending),
        kind: Some(lakelands_api::write::ProposalKind::Compute),
        ..Default::default()
    };
    let call = |w: &ProposedWrite| match w {
        ProposedWrite::Compute {
            ty,
            key,
            field,
            input,
            ..
        } => Some((ty.clone(), key.clone(), field.clone(), input.clone())),
        _ => None,
    };
    let mut held = BTreeSet::new();
    for p in view.proposals(&pending)? {
        held.extend(p.write.as_ref().and_then(call));
    }
    let now = lake.clock(&mut w);
    let first = Position(w.tip.as_ref().map_or(0, |t| t.0.0 + 1));
    let mut drafts = Vec::new();
    for (o, writes) in took {
        for (write, made) in writes {
            if call(&write).is_some_and(|c| !held.insert(c)) {
                continue;
            }
            // A record retired since its call was read, or made again, is
            // proposed nothing: its retirement closed what it had (064).
            // Defensive: no conformance check reaches it; it holds for a
            // record retired or made again between the drain's page and
            // its append.
            if let (ProposedWrite::Compute { ty, key, .. }, Some(made)) = (&write, made) {
                let there = view.get(ty, key)?;
                if there.is_none_or(|r| r.retired || r.made != made) {
                    continue;
                }
            }
            let function = match &write {
                ProposedWrite::Compute { function, .. }
                | ProposedWrite::ComputeSweep { function, .. } => function.clone(),
                _ => continue,
            };
            let risk = (w.ontology.functions.get(&function)).map(|f| f.decl.risk);
            let why = format!(
                "`{function}` is declared {} risk; its call is made when accepted",
                crate::writes::risk_word(risk.unwrap_or_default())
            );
            let at = Position(first.0 + drafts.len() as u64);
            let proposal = lakelands_core::log::positional_id(&lake.id, at);
            drafts.push(Draft {
                writer: o.by.writer.clone(),
                origin: lakelands_api::record::Origin::Authored,
                context: o.context,
                cause: Some(o.by.at),
                ..Draft::system(
                    now,
                    EventBody::WriteProposed {
                        proposal: lakelands_api::ProposalId::from(proposal.0.as_str()),
                        write,
                        why,
                        pinned: false,
                    },
                )
            });
        }
    }
    drop(view);
    if !drafts.is_empty() {
        lake.append(&mut w, drafts)?;
    }
    Ok(())
}

/// An output as it is recorded. It is written as the seam gave it, so one
/// that holds an integer beyond ±(2^53 − 1), a function's value or an
/// embedding's vector, is recorded as its call's failure
/// (`docs/log-format.md` §1.2): no event holds a number a reader would
/// read as another.
fn written(output: Value) -> Value {
    match lakelands_core::canonical::integer_too_large(&output) {
        Some(n) => json!({ "error": Case::IntegerTooLarge { n: n.clone() }.to_string() }),
        None => output,
    }
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
            let remote = Remote::Embedding {
                model: backend.model().to_owned(),
            };
            let (answered, asked) = (v.len() as u64, (to - from) as u64);
            let exchange = Exchange::Vectors { answered, asked };
            Err(Error::unavailable(Situation::Remote { remote, exchange }))
        });
        match answer {
            Ok(vectors) => {
                for (slot, v) in out[span].iter_mut().zip(vectors) {
                    *slot = Some(serde_json::to_value(v).expect("a vector serialises"));
                }
            }
            Err(
                e @ Error::Refused {
                    reason: Refusal::Invalid(_),
                    ..
                },
            ) => {
                let half = from + (to - from) / 2;
                if half == from {
                    out[from] = Some(json!({ "error": e.said() }));
                } else {
                    spans.extend([half..to, from..half]);
                }
            }
            Err(e) => {
                weather = json!({ "error": e.said() });
                break;
            }
        }
    }
    out.into_iter()
        .map(|o| o.unwrap_or_else(|| weather.clone()))
        .collect()
}

/// Record one output for the field it fills, with what the fill causes:
/// the triggers it fires and the object sets its record joins or leaves,
/// judged on the record as the fold will leave it, as for any write.
///
/// An output the lake already holds (`known`) is recorded again only when
/// it fills something: a due issued twice appends nothing the second time.
/// Where it is the call's latest output, and no failure, the fill names it
/// by `output_at` instead of carrying it again
/// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`).
fn record(lake: &Lake, d: Due, output: Value, known: bool) -> Result<Option<Position>> {
    let mut w = lake.for_write()?;
    let now = lake.clock(&mut w);
    let view = lake.engine.view(None)?;
    let at = Position(w.tip.as_ref().map_or(0, |t| t.0.0 + 1));
    let latest = view.output(&d.seam, &d.input)?;
    let output_at = latest
        .filter(|(_, v)| *v == output && !is_failure(v))
        .map(|(p, _)| p);
    let context = d.context;
    // An output computed from a source's erasable values is sealed under
    // that source's key, as they are (058).
    let sealed = d.sealed.clone().filter(|_| output_at.is_none());
    let draft = Draft {
        cause: Some(d.cause),
        procedure: Some(d.procedure.clone()),
        context,
        sealed,
        ..Draft::system(
            now,
            EventBody::OutputRecorded {
                seam: d.seam.clone(),
                input: d.input.clone(),
                output: output_at.is_none().then(|| output.clone()),
                output_at,
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
                plans: &w.plans,
                keys: Some(&lake.keys),
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
                shown: None,
            };
            let mut d = call.decide(&w, view.as_ref(), now, 0);
            let filled = lakelands_api::event::ChangeKind::Recomputed;
            drafts = d.reactions(filled, Some(&before), &after)?;
        }
    }
    if known && !fills {
        return Ok(None);
    }
    // A call made without a decision, the function `low` now, decides the
    // record's proposal of it from when it was above `low`.
    let low = (w.ontology.functions.get(&d.procedure.name)).is_some_and(|f| f.decl.risk.is_low());
    let decided = (low && matches!(d.call, Call::Function { .. }))
        .then(|| on_record(&d.input, &d.ty, (&d.key, d.made)));
    let inputs: Vec<Digest> = [d.input.clone()].into_iter().chain(decided).collect();
    drafts.extend(closed(view.as_ref(), (&d.seam, &inputs), (now, at))?);
    drop(view);
    drafts.insert(0, draft);
    let events = lake.append(&mut w, drafts)?;
    Ok(events.last().map(|e| e.position))
}

#[cfg(test)]
mod tests;
