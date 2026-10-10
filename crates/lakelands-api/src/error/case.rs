//! What a refusal is about, as data: the catalog of cases, the places a
//! case is said at, and the two together (`Said`). A case's sentence is
//! its template, filled with its parameters; it is built nowhere else, and
//! `docs/refusals.md` is generated from this file
//! (`scripts/refusals.py`).
//!
//! The rules the catalog keeps
//! (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`):
//!
//! - **A sentence determines its case.** No two templates can render the
//!   same text, and a place is part of what is said. A code splits later
//!   only where its sentence splits too, never between a hidden thing and
//!   an absent one, which are answered alike.
//! - **A code keeps its meaning** once a release has shipped it; its
//!   parameters may gain optional members only. A new case takes a new
//!   code, and a retired code is not used again.
//! - **A case says what its sentence says, no more.** Every parameter is in
//!   the template; a parameter of text is a name or a value the caller
//!   gave, never a sentence built elsewhere.
//!
//! On the wire a place and a case are objects: `code`, the parameters by
//! their names in the template, and `message`, the sentence. A code this
//! build does not know is read as `Unknown`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};

use super::outage::{Aimless, Exchange, closed, named};
use crate::expr::Condition;
use crate::expr::kind::Kind;
use crate::id::{
    FieldName, LakeId, ParamName, Position, ProposalId, RecordKey, RelationName, SourceId,
    TriggerName, TypeName, ValueTypeName, VerbName,
};
use crate::ontology::{Authority, Cardinality, FieldType, StorageKind};
use crate::query::Walk;
use crate::read::OrderBy;
use crate::time::Timestamp;
use crate::tools::NameProblem;
use crate::write::{Carrier, ProposedWrite};

/// Declares a closed set of cases from their templates: the enum, its
/// sentence, its fallback and its [`Param`]. Each case is its doc, its
/// name, its parameters and its template (see [`render`]). Before the
/// set, `tag = "…";` names the member that holds the code (`code` where
/// it is left out), and `join = "…";` what a list of its cases is joined
/// by (`; ` where it is left out).
macro_rules! catalog {
    (tag = $tag:literal; $($rest:tt)*) => { $crate::error::catalog!(@ $tag; $($rest)*); };
    (
        @ $tag:literal;
        $(join = $join:literal;)?
        $(#[$meta:meta])*
        pub enum $set:ident {
            $(
                $(#[$doc:meta])*
                $case:ident $({ $($(#[$attr:meta])* $param:ident: $ty:ty),* $(,)? })? => $template:literal,
            )*
        }
    ) => {
        $(#[$meta])*
        // A parameter is documented by the template that names it.
        #[allow(missing_docs)]
        #[derive(Clone, Debug, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(tag = $tag, rename_all = "snake_case")]
        pub enum $set {
            $(
                $(#[$doc])*
                $case $({ $($(#[$attr])* $param: $ty),* })?,
            )*
            /// A case this build does not know.
            #[serde(skip)]
            Unknown($crate::error::Unknown),
        }

        impl $crate::error::Closed for $set {
            const TAG: &'static str = $tag;

            fn unknown(unknown: $crate::error::Unknown) -> Self {
                $set::Unknown(unknown)
            }

            fn as_unknown(&self) -> Option<&$crate::error::Unknown> {
                match self {
                    $set::Unknown(u) => Some(u),
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Display for $set {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    $( $set::$case $({ $($param),* })? => $crate::error::render(
                        f,
                        $template,
                        &[$($( (stringify!($param), $crate::error::Param::said($param)) ),*)?],
                    ), )*
                    $set::Unknown(u) => f.write_str(u.message()),
                }
            }
        }

        impl $crate::error::Param for $set {
            $(const JOIN: &'static str = $join;)?

            fn said(&self) -> Option<String> {
                Some(self.to_string())
            }

            fn names(&self, n: &mut $crate::error::Named) {
                match self {
                    $( $set::$case $({ $($param),* })? => {
                        $($( $crate::error::Param::names($param, n); )*)?
                    } )*
                    $set::Unknown(_) => n.unjudged = true,
                }
            }
        }
    };
    ($($rest:tt)*) => { $crate::error::catalog!(@ "code"; $($rest)*); };
}

pub(crate) use catalog;

/// A parameter as a template says it: `None` is a parameter that is not
/// there, and a segment that needs it is not said.
pub trait Param {
    /// What a list of them is joined by.
    const JOIN: &'static str = "; ";

    /// What the template says for it, if it is there.
    fn said(&self) -> Option<String>;

    /// Adds what it names to `n`: a kind [`Named`] does not judge leaves
    /// it unjudged.
    fn names(&self, n: &mut Named) {
        n.unjudged = true;
    }
}

/// What a refusal names, by kind (lakelands#780). The members of a case
/// are typed, so a name is known by its type; text the caller gave is a
/// value.
#[derive(Debug, Default)]
pub struct Named {
    /// Types.
    pub types: Vec<TypeName>,
    /// Fields.
    pub fields: Vec<FieldName>,
    /// Records' keys.
    pub keys: Vec<RecordKey>,
    /// Verbs.
    pub verbs: Vec<VerbName>,
    /// Values.
    pub values: Vec<Value>,
    /// What a field's declaration says: the values it allows, its value
    /// type.
    pub declared: bool,
    /// A name of another kind: a relation's, a trigger's, a function's.
    pub unjudged: bool,
}

impl Named {
    /// Whether a reviewer is shown all of it by `listed`, a proposal as
    /// its listing shows it: each type one it `sees`; each field one it
    /// `reads` of the type written, a verb's counting those its `requires`
    /// reads; each verb the one called; what a declaration
    /// says only beside a field; each key the one written or, as each
    /// value, one listed under a field named, or in a call's payload,
    /// which is listed only whole.
    pub fn shown_by(
        &self,
        listed: &ProposedWrite,
        sees: &dyn Fn(&TypeName) -> bool,
        reads: &dyn Fn(&FieldName) -> bool,
        requires: &dyn Fn(&VerbName) -> Option<Condition>,
    ) -> bool {
        let (key, verb, patch) = match listed {
            ProposedWrite::Upsert { key, patch, .. } => (Some(key), None, Some(patch)),
            ProposedWrite::Retire { key, .. } => (Some(key), None, None),
            ProposedWrite::Call {
                verb, key, payload, ..
            } => (key.as_ref(), Some(verb), Some(payload)),
            _ => return false,
        };
        let called = verb.is_some();
        let listed = |v: &Value| {
            let mut under = (patch.iter()).flat_map(|p| p.iter());
            under.any(|(f, x)| x == v && (called || self.fields.contains(f)))
        };
        let mut read = Vec::new();
        for v in &self.verbs {
            match requires(v).map(|c| c.typed()) {
                Some(Ok(e)) => read.extend(e.fields_read().into_iter().cloned()),
                Some(Err(_)) => return false,
                None => {}
            }
        }
        let key_shown = |k: &RecordKey| Some(k) == key || listed(&Value::from(k.as_str()));
        !self.unjudged
            && self.types.iter().all(sees)
            && self.fields.iter().chain(&read).all(reads)
            && (!self.declared || !self.fields.is_empty())
            && self.verbs.iter().all(|v| Some(v) == verb)
            && self.keys.iter().all(key_shown)
            && self.values.iter().all(listed)
    }
}

impl<T: Param> Param for Option<T> {
    fn said(&self) -> Option<String> {
        self.as_ref().and_then(Param::said)
    }

    fn names(&self, n: &mut Named) {
        self.iter().for_each(|t| t.names(n));
    }
}

impl<T: Param + ?Sized> Param for Box<T> {
    fn said(&self) -> Option<String> {
        T::said(self)
    }

    fn names(&self, n: &mut Named) {
        T::names(self, n);
    }
}

/// A list is there when it holds one at least, and says each, joined.
impl<T: Param> Param for Vec<T> {
    fn said(&self) -> Option<String> {
        let each: Vec<String> = self.iter().filter_map(Param::said).collect();
        (!each.is_empty()).then(|| each.join(T::JOIN))
    }

    fn names(&self, n: &mut Named) {
        self.iter().for_each(|t| t.names(n));
    }
}

/// A flag is there when it is set, and says nothing.
impl Param for bool {
    fn said(&self) -> Option<String> {
        self.then(String::new)
    }

    fn names(&self, _: &mut Named) {}
}

/// An error that is the cause of another is said without the words that
/// say which error it is.
impl Param for super::Error {
    fn said(&self) -> Option<String> {
        Some(super::Error::said(self))
    }

    fn names(&self, n: &mut Named) {
        super::Error::names(self, n);
    }
}

/// Each said by its display; after `=>`, the kind [`Named`] holds it as,
/// or `nothing` for one that names nothing.
macro_rules! said_by_display {
    ($($ty:ty $(=> $kind:ident)?),* $(,)?) => {
        $(impl Param for $ty {
            fn said(&self) -> Option<String> {
                Some(self.to_string())
            }

            $(fn names(&self, n: &mut Named) {
                said_by_display!(@ $kind, self, n);
            })?
        })*
    };
    (@ nothing, $t:expr, $n:expr) => { let _ = $n; };
    (@ declared, $t:expr, $n:expr) => { $n.declared = true };
    (@ values, $t:expr, $n:expr) => { $n.values.push(Value::from($t.clone())) };
    (@ $kind:ident, $t:expr, $n:expr) => { $n.$kind.push($t.clone()) };
}

said_by_display!(
    String => values,
    char => nothing,
    u16 => nothing,
    u32 => nothing,
    u64 => nothing,
    usize => nothing,
    i32 => nothing,
    i64 => nothing,
    Number => values,
    Value => values,
    Names => declared,
    Kind => nothing,
    StorageKind => nothing,
    FieldName => fields,
    LakeId,
    ParamName,
    Position => nothing,
    RecordKey => keys,
    RelationName,
    TriggerName,
    TypeName => types,
    ValueTypeName => declared,
    VerbName => verbs,
    ProposalId,
    SourceId,
    Timestamp => nothing,
    super::Detail,
);

/// Write a template with its parameters.
///
/// - `{name}` is the parameter's words; `{?name}` says nothing and only
///   needs it there.
/// - `[a|b]` says `a` when every parameter `a` names is there, and `b`
///   otherwise; `[a]` says `a` or nothing. Segments nest.
/// - `{{` and `}}` are the braces themselves; no sentence says `[`, `]`
///   or `|`.
pub(crate) fn render(
    f: &mut fmt::Formatter<'_>,
    template: &str,
    params: &[(&str, Option<String>)],
) -> fmt::Result {
    let mut t = template.chars().peekable();
    f.write_str(&segment(&mut t, params).0)
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// A segment up to its `|` or `]`, which are left: what it says, and
/// whether every parameter it names is there.
fn segment(t: &mut Chars<'_>, params: &[(&str, Option<String>)]) -> (String, bool) {
    let (mut out, mut there) = (String::new(), true);
    while let Some(&c) = t.peek() {
        let mut ahead = t.clone();
        ahead.next();
        if matches!(c, '{' | '}') && ahead.peek() == Some(&c) {
            t.next();
            t.next();
            out.push(c);
            continue;
        }
        match c {
            '|' | ']' => break,
            '{' => {
                t.next();
                let name: String = t.by_ref().take_while(|c| *c != '}').collect();
                let (only, name) = match name.strip_prefix('?') {
                    Some(name) => (true, name),
                    None => (false, name.as_str()),
                };
                let said = params.iter().find(|(n, _)| *n == name);
                match said.and_then(|(_, s)| s.as_deref()) {
                    Some(s) if !only => out.push_str(s),
                    Some(_) => {}
                    None => there = false,
                }
            }
            '[' => {
                t.next();
                let (a, a_there) = segment(t, params);
                let b = match t.next() {
                    Some('|') => {
                        let b = segment(t, params).0;
                        t.next();
                        b
                    }
                    _ => String::new(),
                };
                out.push_str(if a_there { &a } else { &b });
            }
            c => {
                t.next();
                out.push(c);
            }
        }
    }
    (out, there)
}

/// Names, said as a list: "`a`, `b`, `c`".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Names(pub Vec<String>);

impl<T: fmt::Display> FromIterator<T> for Names {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Names {
        Names(iter.into_iter().map(|n| n.to_string()).collect())
    }
}

impl fmt::Display for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, n) in self.0.iter().enumerate() {
            let sep = if i == 0 { "" } else { ", " };
            write!(f, "{sep}`{n}`")?;
        }
        Ok(())
    }
}

/// A cardinality by its name in a declaration: `many_to_one`.
impl Param for Cardinality {
    fn said(&self) -> Option<String> {
        serde_json::to_value(self).ok()?.as_str().map(str::to_owned)
    }

    fn names(&self, _: &mut Named) {}
}

/// An authority by its name in a declaration: `derived`.
impl Param for Authority {
    fn said(&self) -> Option<String> {
        serde_json::to_value(self).ok()?.as_str().map(str::to_owned)
    }

    fn names(&self, _: &mut Named) {}
}

/// Which way a step walks: `out`, `in`, `either way`.
impl Param for Walk {
    fn said(&self) -> Option<String> {
        let way = [
            (Walk::Out, "out"),
            (Walk::In, "in"),
            (Walk::Both, "either way"),
        ];
        way.iter()
            .find(|(w, _)| w == self)
            .map(|(_, s)| (*s).into())
    }

    fn names(&self, _: &mut Named) {}
}

/// An order by its name in a read: `known_at`.
impl Param for OrderBy {
    fn said(&self) -> Option<String> {
        serde_json::to_value(self).ok()?.as_str().map(str::to_owned)
    }

    fn names(&self, _: &mut Named) {}
}

/// What carries a relation's edges.
impl Param for Carrier {
    fn said(&self) -> Option<String> {
        Some(match self {
            Carrier::Via => "a `via` field".into(),
            Carrier::Links => "link records".into(),
            Carrier::Derived => "matching values (`derived`)".into(),
        })
    }

    fn names(&self, _: &mut Named) {}
}

/// A field's type by its kind, its value type, its target and its length.
impl Param for FieldType {
    fn said(&self) -> Option<String> {
        struct Shown<'a>(&'a FieldType);
        impl fmt::Display for Shown<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let FieldType {
                    kind,
                    value_type,
                    target,
                    dimensions,
                } = self.0;
                let params = [
                    ("kind", kind.said()),
                    ("value_type", value_type.said()),
                    ("target", target.said()),
                    ("dimensions", dimensions.said()),
                ];
                let t = "{kind}[ ({value_type})][ of `{target}`][ of {dimensions} dimensions]";
                render(f, t, &params)
            }
        }
        Some(Shown(self).to_string())
    }

    fn names(&self, n: &mut Named) {
        self.value_type.names(n);
        self.target.names(n);
    }
}

named! {
    /// What a declared name names.
    pub enum Naming {
        /// A type.
        Type = "type" => "type",
        /// A value type.
        ValueType = "value_type" => "value type",
        /// A field.
        Field = "field" => "field",
        /// A relation.
        Relation = "relation" => "relation",
        /// A view's measure.
        Measure = "measure" => "measure",
        /// A view's parameter.
        Parameter = "parameter" => "parameter",
        /// A verb.
        Verb = "verb" => "verb",
        /// A verb's executor.
        Executor = "executor" => "executor",
        /// A trigger.
        Trigger = "trigger" => "trigger",
        /// An object set.
        ObjectSet = "object_set" => "object set",
        /// A function.
        Function = "function" => "function",
        /// A function's backing.
        Backing = "backing" => "backing",
        /// An aging rule.
        AgingRule = "aging_rule" => "aging rule",
        /// An aggregate's group key.
        Group = "group" => "group",
        /// A name a pattern binds a record under.
        Binding = "binding" => "binding",
    }
}

named! {
    /// The JSON form a value of a storage kind takes.
    pub enum Shape {
        /// A string.
        Text = "text" => "a string",
        /// A number.
        Number = "number" => "a number",
        /// A boolean.
        Boolean = "boolean" => "true or false",
        /// A decimal's digits.
        Digits = "digits" => "a string of digits",
        /// A date-time.
        DateTime = "date_time" => "a date-time string",
        /// A date.
        Date = "date" => "a date string",
        /// Bytes.
        Base64 = "base64" => "a base64 string",
        /// A vector.
        Numbers = "numbers" => "an array of numbers, each finite as a 32-bit float",
    }
}

catalog! {
    /// Where in a declaration or a request a case is said.
    pub enum Place {
        /// A type, by name.
        Type { name: TypeName } => "type `{name}`",
        /// A view, by name.
        View { name: TypeName } => "view `{name}`",
        /// A field.
        Field { name: FieldName } => "field `{name}`",
        /// A relation.
        Relation { name: RelationName } => "relation `{name}`",
        /// A verb.
        Verb { name: VerbName } => "verb `{name}`",
        /// A value type.
        ValueType { name: ValueTypeName } => "value type `{name}`",
        /// A trigger.
        Trigger { name: TriggerName } => "trigger `{name}`",
        /// An object set.
        ObjectSet { name: String } => "object set `{name}`",
        /// A function.
        Function { name: String } => "function `{name}`",
        /// A member of a function's `config`.
        FunctionConfig { key: String } => "`config.{key}`",
        /// A source a view merges.
        MergedSource { name: TypeName } => "merged source `{name}`",
        /// A view's measure.
        Measure { name: FieldName } => "measure `{name}`",
        /// A view's or a source's `where`.
        Where => "`where`",
        /// An annotation, by its key.
        Annotation { key: String } => "annotation `{key}`",
        /// A setting, by its key.
        Setting { key: String } => "setting `{key}`",
        /// One of a migration's relation renames.
        RelationRename { from: RelationName, to: RelationName } => "renaming relation `{from}` to `{to}`",
        /// One of a value type migration's value renames.
        ValueRename { from: String, to: String } => "renaming `{from}` to `{to}`",
        /// An aggregate's group key, or a view's `group_by` field.
        Group { name: FieldName } => "group `{name}`",
        /// A verb's payload.
        Payload => "payload",
        /// A function's inputs.
        Input => "input",
        /// A verb's `requires`.
        Requires => "`requires`",
        /// A value type's or a type's `constraint`.
        Constraint => "`constraint`",
        /// A field's `initial` value.
        Initial => "`initial`",
        /// The link types of a type's relations.
        Links { name: TypeName } => "the links of `{name}`",
        /// A pattern's start.
        Start => "the start",
        /// A pattern's step, by its number from 1 and its relation.
        Step { step: usize, relation: RelationName } => "step {step} over `{relation}`",
        /// A `find`'s aggregate over a type.
        Aggregate { name: TypeName } => "aggregate over `{name}`",
        /// What a type's migration leaves.
        AfterMigration { name: TypeName } => "after the migration of `{name}`",
        /// A door's arguments, by the tool or the route that takes them.
        Arguments { tool: String } => "the arguments of `{tool}`",
        /// A declaration among a door's arguments, by its kind and its name.
        Declaration { what: Naming, name: Option<String> } => "the {what} declaration[ `{name}`]",
        /// A member among a door's arguments or a declaration's, by its path.
        Member { path: String } => "at `{path}`",
    }
}

named! {
    /// What kind of failure a JSON parser met, said without the text it
    /// quotes, which may be a record's value.
    pub enum JsonFault {
        /// Not JSON.
        Syntax = "syntax" => "not JSON",
        /// JSON of another shape than the one read.
        Data = "data" => "not of the expected shape",
        /// JSON cut short.
        Eof = "eof" => "cut short",
        /// A failure to read the bytes.
        Io = "io" => "an I/O failure",
    }
}

impl From<&serde_json::Error> for JsonFault {
    fn from(e: &serde_json::Error) -> JsonFault {
        use serde_json::error::Category;
        match e.classify() {
            Category::Syntax => JsonFault::Syntax,
            Category::Data => JsonFault::Data,
            Category::Eof => JsonFault::Eof,
            Category::Io => JsonFault::Io,
        }
    }
}

named! {
    /// The form a REST query parameter's value takes.
    pub enum Form {
        /// A non-negative integer.
        Whole = "whole" => "a whole number",
        /// A flag.
        Flag = "flag" => "`true` or `false`",
        /// JSON.
        Json = "json" => "JSON",
        /// Sort keys.
        Order = "order" => "fields separated by commas, `-field` for descending",
        /// A value given, not empty.
        Given = "given" => "a value that is not empty",
    }
}

named! {
    /// What a REST path segment a client sends names.
    pub enum Segment {
        /// A type.
        Type = "type" => "the type",
        /// A record's key.
        Key = "key" => "the key",
        /// The key a link goes to.
        To = "to" => "the key linked to",
        /// A relation.
        Relation = "relation" => "the relation",
        /// A verb.
        Verb = "verb" => "the verb",
        /// A proposal.
        Proposal = "proposal" => "the proposal",
        /// A source.
        Source = "source" => "the source",
    }
}

catalog! {
    join = ", ";
    /// A record a case names.
    pub enum RecordName {
        /// A record, by its type and its key.
        Of { #[serde(rename = "type")] ty: TypeName, key: RecordKey } => "`{ty}` `{key}`",
        /// A record's field, by the record's type and key and its name.
        FieldOf { #[serde(rename = "type")] ty: TypeName, key: RecordKey, field: FieldName } => "`{ty}` `{key}`.`{field}`",
    }
}

catalog! {
    /// What a refusal is about.
    pub enum Case {
        /// A field the type does not have, or one the caller may not see.
        UnknownField {
            #[serde(rename = "type")]
            ty: TypeName,
            field: FieldName,
        } => "`{ty}` has no field `{field}`",
        /// A reference to a record that is not live.
        NotLive {
            field: FieldName,
            #[serde(rename = "type")]
            ty: TypeName,
            key: RecordKey,
        } => "`{field}` names `{ty}` `{key}`, which is not a live record",

        // ---- declarations
        /// An annotation key that is not a namespace and a name.
        AnnotationKeyForm => "a key is a namespace and a name, ASCII identifiers joined by `.` (`acme.icon`)",
        /// An annotation key over the length a key may have.
        AnnotationKeyLong { max: usize } => "a key is at most {max} bytes",
        /// An annotation whose value is `null`.
        AnnotationNull => "a value is not `null`; an annotation is taken away by leaving its key out",
        /// An annotation key in a reserved namespace.
        AnnotationReserved { namespace: String } => "the namespace `{namespace}.` is reserved",
        /// A declaration's annotations over the bytes it may hold.
        AnnotationsTooLarge { bytes: usize, max: usize } => "it holds {bytes} bytes of annotations, over the {max} a declaration may hold",
        /// A field computed at the read that a door would write.
        AtReadAuthority => "computed at the read, it has the authority `derived`: no door writes it",
        /// A field computed at the read by a function a backing computes.
        AtReadBacked => "computed at the read, an expression computes it; a function's backing is called at a write",
        /// A field computed at the read that a change of its inputs clears.
        AtReadCleared => "computed at the read, no value of it is kept to clear",
        /// A link's field computed at the read.
        LinkFieldAtRead => "a link's field is computed at the write; a walk reads links as stored",
        /// A key field computed at the read.
        AtReadKey => "computed at the read, it is no key, which is read as stored",
        /// A derived relation's match computed at the read.
        AtReadMatch => "computed at the read, it is no derived relation's match, which is read as stored",
        /// A field computed at the read that reads what only a write has.
        AtReadPrevious => "computed at the read, it reads no `previous` or `changed`, which a write has and a read has not",
        /// A required field computed at the read.
        AtReadRequired => "computed at the read, it is not `required`: no write gives it a value",
        /// A searchable field computed at the read.
        AtReadSearchable => "computed at the read, it is not searchable: an index holds stored values only",
        /// A sensitive field computed at the read.
        AtReadSensitive => "computed at the read, it is not `sensitive` by itself, but as what it is computed from",
        /// A `valid_from` computed at the read.
        AtReadValidFrom => "computed at the read, it is no `valid_from`, which is read as stored",
        /// A `valid_to` computed at the read.
        AtReadValidTo => "computed at the read, it is no `valid_to`, which is read as stored",
        /// A relation's `via` computed at the read.
        AtReadVia => "computed at the read, it is no `via`, which is read as stored",
        /// A field computed at the write that reads one computed at the read.
        AtWriteReadsAtRead { field: FieldName } => "computed at the write, it reads `{field}`, which is computed at the read and never stored",
        /// A relation declared again with another carrier.
        CarrierChanged { from: Carrier, to: Carrier } => "a relation keeps its carrier; it is carried by {from} and cannot be carried by {to}; declare the other kind under another name",
        /// A computed field reading the clock that does not say when it is computed.
        ClockUnsaid => "its expression reads the clock: say `computed_at: read` for a value of the day it is read, or `computed_at: write` for the instant of its last write",
        /// A view's field that says when it is computed.
        ComputedAtInView => "a view's fields say no `computed_at`; its records are computed at every read",
        /// `computed_at` on a field no record holds.
        ComputedAtNoRecord => "only a field of a record says `computed_at`",
        /// A field that says when it is computed and is not computed.
        ComputedAtUncomputed => "only a field derived from an expression or computed by a function says `computed_at`",
        /// Computed fields that read each other.
        ComputedCycle { fields: Names } => "computed fields read each other in a cycle: {fields}",
        /// A computed field that reads itself.
        ComputedReadsItself { field: FieldName } => "the computed field `{field}` reads itself",
        /// A name declared twice in one declaration.
        DeclaredTwice { what: Naming, name: String } => "it declares the {what} `{name}` twice",
        /// A field both derived and computed by a function.
        DerivedAndFunction => "a field is derived from an expression or computed by a function, not both",
        /// Derived fields that read each other.
        DerivedCycle { fields: Names } => "derived fields read each other in a cycle: {fields}",
        /// A derived relation with no pair.
        DerivedNoPair => "a derived relation matches at least one pair of fields",
        /// A derived field that reads itself.
        DerivedReadsItself { field: FieldName } => "derived field `{field}` reads itself",
        /// A derived relation with a carrier of its own.
        DerivedStoresEdge => "a derived relation stores no edge, so it has no `via`, no fields and no link authority",
        /// Dimensions on what is no vector.
        DimensionsNotVector => "only a vector names dimensions",
        /// An embedding that is also derived or computed.
        EmbedAlsoComputed => "a field is an embedding, derived from an expression or computed by a function, only one",
        /// An embedding with a `depends_on`.
        EmbedDependsOn => "an embedding depends on its `from` fields; `depends_on` is not written beside it",
        /// An embedding made from itself.
        EmbedFromItself => "it cannot be made from itself",
        /// An embedding made from a field that is not text.
        EmbedFromNotText { field: FieldName } => "it is made from `{field}`, which is not text",
        /// An embedding, not sensitive, made from a sensitive field.
        EmbedFromSensitive { field: FieldName } => "it is made from the sensitive field `{field}` and is not sensitive itself",
        /// An embedding that names a field twice.
        EmbedFromTwice { field: FieldName } => "it names `{field}` in `from` twice",
        /// An embedding made from nothing.
        EmbedNoFrom => "an embedding names the fields it is made `from`",
        /// An embedding that names no model.
        EmbedNoModel => "an embedding names its model",
        /// An embedding whose authority is not `derived`.
        EmbedNotDerived => "an embedding has the authority `derived`",
        /// An embedding that is not a vector with dimensions.
        EmbedNotVector => "only a vector with dimensions is the embedding of text",
        /// An `expr` function with no expression.
        ExprNoExpression => "it is backed by `expr` and has no expression",
        /// An expression on a function another backing computes.
        ExpressionNotExpr { backing: String } => "it has an expression but is backed by `{backing}`",
        /// A function's input the type holds as another kind.
        FunctionInputKind { function: String, input: FieldName, kind: StorageKind, #[serde(rename = "type")] ty: TypeName, held: StorageKind } => "the function `{function}` takes `{input}` as a {kind}, and `{ty}` holds it as a {held}",
        /// A field computed by a function whose authority is not `derived`.
        FunctionNotDerived => "a field computed by a function has the authority `derived`",
        /// A field naming a function that is not declared.
        FunctionUndeclared { name: String } => "it names the function `{name}`, which is not declared",
        /// A view grouped by a field no key may be.
        GroupUnkeyable { field: FieldName } => "it groups by `{field}`, whose kind cannot be part of a key",
        /// An `initial` value on a field its inputs clear.
        InitialCleared => "its inputs would clear it at their next change",
        /// An `initial` value on a view's field.
        InitialInView => "a view's records are computed, and none is created",
        /// An `initial` value on a key field.
        InitialKey => "a key's field is given by the record's key",
        /// An `initial` value on a field its authors do not write alone.
        InitialNotAuthored => "only a field its authors alone write takes one, not one a source writes or the fold computes",
        /// An `initial` value on a field of no type.
        InitialNotOfType => "only a field of a type declares one",
        /// An `initial` value on a field that names a record.
        InitialReference => "a field that names a record is checked against that record by the door that writes it",
        /// A `json` field named as a type.
        JsonNamedAsType => "a json field may not carry the name of a type",
        /// A type keyed by a field no key may be.
        KeyUnkeyable { field: FieldName } => "it is keyed by `{field}`, whose kind has no string form for a key; a key field is text, a reference, a decimal, a number, a boolean, a date-time or a date",
        /// Links of an authority no door writes them by.
        LinkAuthority { authority: Authority } => "its links are written by `authored` or `source`, not `{authority}`",
        /// A field of a source's link that no door writes.
        LinkFieldAuthority { field: FieldName, authority: Authority } => "its links arrive from a source, so its field `{field}` is `source` (or derived); an `{authority}` field of a link no door could write",
        /// A link field named as a link's end.
        LinkFieldIsEnd { field: FieldName } => "a link's `{field}` is its end; the relation cannot name a field of its own so",
        /// A measure named twice in a view.
        MeasureTwice { name: FieldName } => "its view names the measure `{name}` twice; a view's measures and its merged sources' share one set of names",
        /// A merged measure that calls no aggregate.
        MeasuresNothing { source: TypeName } => "it calls no aggregate, so it measures nothing of `{source}`",
        /// A merged source grouped by another number of fields than its view.
        MergedGroupCount { given: usize, view: usize } => "it groups by {given} field(s), and the view by {view}; each lines up with one of the view's, in order",
        /// A merged source's group field of another kind than the view's.
        MergedGroupKind { field: FieldName, view_field: FieldName } => "its `{field}` lines up with the view's `{view_field}`, of another kind, so no group would meet",
        /// An empty name.
        NameEmpty { what: Naming } => "the {what} name must not be empty",
        /// A name that is not an identifier.
        NameNotIdentifier { what: Naming, name: String } => "{what} name `{name}` is not an ASCII identifier: a letter or `_`, then letters, digits or `_`",
        /// An expression whose kind its field never holds.
        NeverLands { kind: Kind, holds: StorageKind, duration: bool } => "its expression is {kind}, which a {holds} field never holds[{?duration}; divide a duration by a unit, as in `(a - b) / 1d`]",
        /// A value type named that is not declared.
        NoValueType { value_type: ValueTypeName } => "no value type named `{value_type}`",
        /// `one_of` on a value type that is not text.
        OneOfNotText => "only text has `one_of`",
        /// A derived relation's pair reading a field computed at the read.
        PairAtRead { from: FieldName, to: FieldName, target: TypeName } => "the pair `{from}` = `{to}` matches a field of `{target}` that is computed at the read; a match reads values as stored",
        /// A derived relation's pair of two kinds.
        PairKinds { from: FieldName, to: FieldName, left: StorageKind, right: StorageKind } => "the pair `{from}` = `{to}` matches a {left} field with a {right} field; values of two kinds are never identical, so the pair would match nothing",
        /// A derived relation's pair named twice.
        PairTwice { from: FieldName, to: FieldName } => "the pair `{from}` = `{to}` is matched twice",
        /// A derived relation's pair of a kind no key may be.
        PairUnkeyable { from: FieldName, to: FieldName, kind: StorageKind } => "the pair `{from}` = `{to}` matches {kind} fields; a match is between fields of a kind a key may be: text, a reference, a decimal, a number, a boolean, a date-time or a date",
        /// A derived relation's pair naming a field that is not there.
        PairUnknown { from: FieldName, to: FieldName, #[serde(rename = "type")] ty: TypeName, target: Box<TypeName> } => "the pair `{from}` = `{to}` names a field that `{ty}` or `{target}` does not have",
        /// A view's parameter of a kind no parameter is.
        ParamKind { name: ParamName, kind: StorageKind } => "its view's parameter `{name}` is of kind {kind}; a parameter is text, number, decimal, boolean, date_time or date",
        /// A `param` its view does not declare.
        ParamUndeclared { name: String, declared: Option<Names> } => "its view reads param('{name}'), which it does not declare; [its parameters are {declared}|it declares no parameters]",
        /// A view's parameter nothing reads.
        ParamUnread { name: ParamName } => "its view declares the parameter `{name}`, which neither its `where` nor its measures read, so a reader would bind a value that changes nothing",
        /// A verb's assignment reading a payload member the payload does not have.
        PayloadNoMember { member: FieldName } => "it reads `payload.{member}`, which the payload does not have",
        /// A field taken to the read that something reads as stored.
        ReadAsStored => "a trigger, a verb, an object set or another type's derived relation in force reads it as stored, so it is not computed at the read",
        /// A declaration reading as stored a field computed at the read.
        ReadsAtRead { field: FieldName, #[serde(rename = "type")] ty: TypeName } => "it reads `{field}`, which `{ty}` computes at the read; it is evaluated over the record as stored, which never holds it",
        /// A function's expression reading what is not one of its inputs.
        ReadsNoInput { field: FieldName } => "it reads `{field}`, which is not one of its inputs",
        /// A view reading a view with parameters.
        ReadsParamView { source: TypeName, param: ParamName } => "its view reads `{source}`, a view with the parameter `{param}`; only a read that names a view binds its parameters, so no view reads one",
        /// A reference with no target.
        ReferenceNoTarget => "a reference names its target type",
        /// A scale on a value type that is no decimal.
        ScaleNotDecimal => "only a decimal has a scale",
        /// An object set of a view.
        SetOfView { #[serde(rename = "type")] ty: TypeName } => "it holds `{ty}`, a view; a view's records are computed on read, and an object set holds stored records only",
        /// An object set declared again over another type.
        SetRetyped { was: TypeName, #[serde(rename = "type")] ty: TypeName } => "it holds records of `{was}`; a set of `{ty}` takes another name",
        /// A view reading a source twice.
        SourceTwice { source: TypeName } => "a view reads each source once, and `{source}` is read already",
        /// A target on what is no reference.
        TargetNotReference => "only a reference names a target",
        /// The tool names the lake's declarations would give.
        ToolNames { #[serde(with = "closed::list")] problems: Vec<NameProblem> } => "the lake's names would not all become tools: {problems}",
        /// A type name that is not identifiers joined by `.`.
        TypeNameForm { name: String } => "type name `{name}` is not ASCII identifiers joined by `.`",
        /// A verb on its type with a `requires`.
        TypeVerbRequires { #[serde(rename = "type")] ty: TypeName } => "it is declared on the type `{ty}`; `requires` reads a record it does not have",
        /// A verb on its type with a write effect.
        TypeVerbWrites { #[serde(rename = "type")] ty: TypeName } => "it is declared on the type `{ty}`, and a write effect sets fields of a record it does not have",
        /// A type named that is not declared.
        Undeclared { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is not a declared type",
        /// A valid time over a field that is neither a date-time nor a date.
        ValidTimeNotTime { field: FieldName, axis: String, kind: StorageKind } => "it names `{field}` as its `{axis}`, a {kind} field; a valid time is a date_time or a date field",
        /// A field its author may name, sent as a valid time where one over
        /// a field it may not know of stands: the bound is taken.
        ValidTimeStands { axis: String } => "its `{axis}` names a field you may not know of, and stands as declared",
        /// A value type naming a format that does not exist.
        ValueTypeFormat { value_type: ValueTypeName, format: String } => "value type `{value_type}` names the unknown format `{format}`",
        /// A field stored as another kind than its value type.
        ValueTypeKind { field: FieldName, value_type: ValueTypeName, expected: StorageKind, found: StorageKind } => "field `{field}` uses value type `{value_type}`, which is {expected}, but the field is {found}",
        /// A vector with no dimensions.
        VectorDimensions => "a vector names its dimensions, more than zero",
        /// A verb on the records of a view.
        VerbOnView { #[serde(rename = "type")] ty: TypeName } => "it is declared on a record of `{ty}`, a view, whose records are computed and never written",
        /// A relation carried by `via` with fields.
        ViaWithFields => "a relation carried by `via` has no fields of its own; fields belong to a relation carried by link records, declared without `via`",
        /// A view that reads itself.
        ViewCycle { chain: Names } => "its view closes a cycle, reading {chain} in turn; a view may not read itself, through other views or directly",
        /// A view's field nothing fills.
        ViewFieldUnfilled { field: FieldName } => "its field `{field}` is neither a `group_by` field nor a measure of its view, so nothing would fill it",
        /// A view's `key` that names other fields than its `group_by`.
        ViewKey { group_by: Option<Names> } => "a view's records are keyed by its `group_by`[ ({group_by})|, and it has none]; its `key` names the same fields in that order, or is left out",
        /// A stored relation on a view.
        ViewRelation { #[serde(rename = "type")] ty: TypeName, carrier: Carrier } => "`{ty}` is a view, whose records are computed and never written, so a relation carried by {carrier} could never hold an edge; a relation on a view is derived from matching values",
        /// A view's field that a search would read.
        ViewSearched { searchable: bool, embedded: bool } => "it [{?searchable}is searchable|[{?embedded}carries an embedding|is a vector]], and a view is not searched; its records are computed on read and no index holds them",
        /// A value a view computes that no field holds.
        ViewValueNoField { field: FieldName } => "its view computes `{field}`, which has no declared field to land in; a view declares a field for each value it computes, of the kind it holds",
        /// A host vocabulary's type under a name the store keeps.
        VocabularyReserved { #[serde(rename = "type")] ty: TypeName, link: bool } => "the host vocabulary's type `{ty}` is reserved: [{?link}the store derives the link types under `sys.link.` from relations itself|the store keeps it for its own reader]; nothing was appended, and the lake is not opened for writing until the vocabulary names it otherwise",
        /// A trigger on a link type of a relation carried by a reference.
        WatchesViaLinks { #[serde(rename = "type")] ty: TypeName, relation: RelationName, via: FieldName, owner: Box<TypeName> } => "it watches `{ty}`, but `{relation}` is carried by a reference field, not by link records; watch `{owner}` with `changed({via})`",
        /// A trigger on a view.
        WatchesView { #[serde(rename = "type")] ty: TypeName } => "it watches `{ty}`, a view; a view is never written, so no write of it fires a trigger",
        /// A write effect that sets nothing.
        WritesNothing => "it writes nothing",

        // ---- expressions
        /// An aggregate where no set of records is.
        AggregateHere { name: String } => "{name}() folds a set of records and has no meaning here",
        /// An aggregate other than `count` with no argument.
        AggregateNeedsField { name: String } => "{name}() needs a field",
        /// A `date_bucket` given neither a unit nor a width and an origin.
        BucketArguments => "date_bucket() takes a calendar unit quoted ('day', 'week', 'month', 'quarter' or 'year') and no origin, or a width above zero and the origin its windows are counted from, as in date_bucket(t, 30d, origin)",
        /// A `date_bucket` unit it does not know.
        BucketUnit { unit: String } => "date_bucket() knows no unit '{unit}'; the units are 'day', 'week', 'month', 'quarter' and 'year', and a width is written unquoted, as in date_bucket(t, 30d, origin)",
        /// A `date_bucket` unit that is no text.
        BucketUnitText { #[serde(with = "closed")] value: Box<Operand> } => "date_bucket() takes its unit as text, not {value}",
        /// A call with a number of arguments its function does not take.
        CallArity { name: String, given: usize } => "{name}() does not take {given} argument(s)",
        /// A call that takes a field name given something else.
        CallTakesField { name: String } => "{name}() takes one field name",
        /// A call that takes a quoted name given something else.
        CallTakesQuoted { name: String } => "{name}() takes a quoted name",
        /// A division by zero.
        DivisionByZero => "division by zero",
        /// An `in` with an empty list.
        InEmpty => "an `in` lists one value at least",
        /// An `in_set` left where no set's filter stands.
        InSetHere => "in_set() has meaning in an object set's `where`, a trigger's `when` and a read's `where`",
        /// A negation of what has no negative.
        KindNegates { expr: String, kind: Kind } => "`{expr}` has no meaning: it negates {kind}",
        /// Arithmetic over kinds that do not combine.
        KindSides { expr: String, left: Kind, right: Kind } => "`{expr}` has no meaning: its sides are {left} and {right}",
        /// A number's function over what is no number.
        KindTakesNumber { expr: String, kind: Kind } => "`{expr}` has no meaning: it takes a number, not {kind}",
        /// A difference of what are not two instants.
        MinusNoMeaning { #[serde(with = "closed")] left: Box<Operand>, #[serde(with = "closed")] right: Box<Operand> } => "{left} minus {right} has no meaning",
        /// A number that is not a number.
        NotANumber { #[serde(with = "closed")] value: Box<Operand> } => "{value} is not a number",
        /// A function's call that is no aggregate where one is folded.
        NotAggregate { name: String } => "{name}() is not an aggregate",
        /// A number written that the evaluator cannot hold exactly.
        NotExact { number: String } => "`{number}` is not a number this evaluator can hold exactly",
        /// A pattern that is not an I-Regexp.
        NotIRegexp { pattern: String, #[serde(with = "closed")] why: Box<Regexp> } => "`{pattern}` is not an I-Regexp ({why})",
        /// A product or quotient of what does not multiply.
        NotMultiplied { #[serde(with = "closed")] left: Box<Operand>, #[serde(with = "closed")] right: Box<Operand> } => "{left} and {right} cannot be multiplied or divided",
        /// An expression's text that does not parse.
        NotParsed { text: String, #[serde(with = "closed")] why: Box<Case> } => "the expression `{text}` does not parse: {why}",
        /// A position or a length that is no whole number at least 0.
        NotPosition { number: String } => "{number} is not a position or a length",
        /// What is no instant shifted by a duration.
        NotShiftable { #[serde(with = "closed")] value: Box<Operand> } => "{value} cannot be shifted by a duration",
        /// A function that has meaning only in a write, outside one.
        OnlyInWrite { name: String } => "{name}() has meaning only in a guard over a write",
        /// A number beyond exact arithmetic.
        Overflow => "the number is too large for exact arithmetic",
        /// A `param` outside a view.
        ParamOutsideView => "param() is read only in a view's `where` and `measures`, where the view declares its parameters",
        /// A `param` given what is no name.
        ParamTakesName { #[serde(with = "closed")] value: Box<Operand> } => "param() takes a name, not {value}",
        /// A call with fewer arguments than its function takes.
        ParseArityAtLeast { name: String, min: usize, given: usize, offset: usize } => "`{name}` takes at least {min} argument(s), not {given} (at character {offset})",
        /// A call with another number of arguments than its function takes.
        ParseArityExact { name: String, takes: usize, given: usize, offset: usize } => "`{name}` takes {takes} argument(s), not {given} (at character {offset})",
        /// A call with a number of arguments outside its function's range.
        ParseArityRange { name: String, min: usize, max: usize, given: usize, offset: usize } => "`{name}` takes {min} to {max} argument(s), not {given} (at character {offset})",
        /// An `in` with an empty list.
        ParseEmptyIn { offset: usize } => "`in` needs at least one value (at character {offset})",
        /// An expression cut short.
        ParseEndsEarly { offset: usize } => "the expression ends too early (at character {offset})",
        /// A token the grammar expects that is not there.
        ParseExpected { token: String, offset: usize } => "expected `{token}` (at character {offset})",
        /// An `is` not followed by `null`.
        ParseIsNull { offset: usize } => "after `is` comes `null` or `not null` (at character {offset})",
        /// A keyword where a value starts.
        ParseKeyword { keyword: String, offset: usize } => "`{keyword}` cannot start a value (at character {offset})",
        /// A `.` with no name after it.
        ParseNameAfterDot { offset: usize } => "expected a name after `.` (at character {offset})",
        /// A duration the grammar cannot hold.
        ParseNoDuration { raw: String, offset: usize } => "`{raw}` is not a duration this grammar can hold (at character {offset})",
        /// A call of a function the grammar does not have.
        ParseNoFunction { name: String, offset: usize } => "there is no function `{name}` (at character {offset})",
        /// A number written into a name.
        ParseNumberIntoName { offset: usize } => "a number runs into a name (at character {offset})",
        /// A `param` whose argument is not a quoted name.
        ParseParamName { offset: usize } => "`param` takes a quoted name, as in param('anchor') (at character {offset})",
        /// A `setting` whose argument is not a quoted key.
        ParseSettingKey { offset: usize } => "`setting` takes a quoted key, as in setting('vat_rate') (at character {offset})",
        /// A function that takes a field name given something else.
        ParseTakesField { name: String, offset: usize } => "`{name}` takes a field name (at character {offset})",
        /// Input after a whole expression.
        ParseTrailing { offset: usize } => "unexpected input after the end of the expression (at character {offset})",
        /// A string not closed.
        ParseUnclosed { offset: usize } => "a string is not closed with `'` (at character {offset})",
        /// A character no token starts with.
        ParseUnexpected { found: char, offset: usize } => "unexpected `{found}` (at character {offset})",
        /// A path with no field.
        PathEmpty => "a path names at least one field",
        /// A pattern the matcher refuses.
        RegexUnmatchable { pattern: String, limit: Option<usize> } => "`{pattern}` cannot be matched[: its compiled form is over {limit} bytes]",
        /// A `retired` outside a trigger's guard.
        RetiredOnlyInGuard => "retired() has meaning only in a trigger's guard",
        /// A set that names itself through others.
        SetCycle { set: String, through: Names } => "in_set('{set}') names a set that names it in turn, through {through}",
        /// A set of another type named in a filter.
        SetOfOtherType { set: String, of: TypeName, #[serde(rename = "type")] ty: TypeName } => "in_set('{set}') names a set of `{of}`, not of `{ty}`",
        /// A set named in a filter that is not declared.
        SetUndeclared { set: String } => "in_set('{set}') names an object set that is not declared",
        /// A `setting` given what is no key.
        SettingTakesKey { #[serde(with = "closed")] value: Box<Operand> } => "setting() takes a key, not {value}",
        /// A path into a reference's members.
        StepsIntoReference { expr: String, field: FieldName } => "`{expr}` steps into `{field}`, which is a reference: a condition reads the record's own fields. Compare the reference's key, or walk to the record it names in a pattern",
        /// A path into the members of what has none.
        StepsIntoScalar { expr: String, field: FieldName } => "`{expr}` steps into `{field}`, which is no `json` field, and has no members",
        /// A sum of what is no number.
        SumNotNumber { #[serde(with = "closed")] value: Box<Operand> } => "sum() met {value}, which is not a number",

        // ---- views
        /// A sum or an average of a kind that is no number.
        KeptAddsKind { call: String, kind: Kind } => "`{call}` adds {kind}; it is kept where its argument is certainly a number",
        /// A sum or an average of what is of no certain kind.
        KeptAddsUncertain { call: String } => "`{call}` adds what is of no certain kind: it is kept where its argument is certainly a number",
        /// A measure reading a field computed at the read.
        KeptAtRead { field: FieldName, source: TypeName } => "it reads `{field}` of `{source}`, which is computed at the read, and a kept record is computed when its source is written",
        /// A measure reading the clock.
        KeptClock => "it reads the clock, and a kept record is computed when its source is written",
        /// A minimum or maximum under a group field that is no text or reference.
        KeptExtremeGroupedBy { call: String, field: FieldName } => "`{call}` is kept only in a view grouped by text or reference fields, and `{field}` is neither",
        /// A minimum or maximum in a view with no group field.
        KeptExtremeUngrouped { call: String } => "`{call}` is kept only in a view grouped by text or reference fields, and this one has no group field",
        /// A grouping by a field computed at the read.
        KeptGroupAtRead { field: FieldName, source: TypeName } => "it groups by what reads `{field}` of `{source}`, which is computed at the read, and a kept record is computed when its source is written",
        /// An aggregate call with no argument.
        KeptNoArgument { call: String } => "`{call}` has no argument",
        /// A minimum or maximum of a kind that is no number or instant.
        KeptOrdersKind { call: String, kind: Kind } => "`{call}` orders {kind}; it is kept where its argument is certainly a number or an instant",
        /// A minimum or maximum of what is of no certain kind.
        KeptOrdersUncertain { call: String } => "`{call}` orders what is of no certain kind: it is kept where its argument is certainly a number or an instant",
        /// A view with a parameter.
        KeptParam { name: ParamName } => "it declares the parameter `{name}`, and a kept record is the same for every read",
        /// A `where` reading a field computed at the read.
        KeptWhereAtRead { field: FieldName, source: TypeName } => "its `where` reads `{field}` of `{source}`, which is computed at the read, and a kept record is computed when its source is written",
        /// A `where` reading the clock.
        KeptWhereClock { source: TypeName } => "its `where` over `{source}` reads the clock, and a kept record is computed when its source is written",

        // ---- reads
        /// An `after` of another number of values than the order's keys.
        AfterCount { values: usize, keys: usize } => "`after.values` holds {values} value(s) and `order_by` has {keys} key(s); give one value per key",
        /// An `after` value its order's field does not take.
        AfterUnfit { index: usize, field: FieldName, #[serde(with = "closed")] why: Box<Case> } => "`after.values` member {index} does not fit `{field}`: {why}",
        /// An `after` beside an aggregate.
        AfterWithAggregate => "`after` pages records; an `aggregate` has no pages",
        /// An `after` beside a cursor.
        AfterWithCursor => "`after` and `cursor` both name where a page starts; give one",
        /// A `components` step that does not walk both ways.
        ComponentsBothWays { relation: RelationName } => "`components` takes every link both ways; the step over `{relation}` must walk `both`",
        /// A cursor the lake did not hand out.
        CursorForeign => "the cursor is not one this lake handed out",
        /// A cursor handed out for another door or type.
        CursorOtherDoor { was: String, was_ty: Option<Box<TypeName>>, door: String, #[serde(rename = "type")] ty: Option<Box<TypeName>> } => "the cursor was handed out for `{was}`[ on `{was_ty}`], not `{door}`[ on `{ty}`]; continue it there",
        /// A cursor handed out for another order.
        CursorOtherOrder => "the cursor was handed out for another order; continue it with the `order_by` its first page was read with",
        /// A cursor handed out for other view parameters.
        CursorOtherParams => "the cursor was handed out for other parameters; continue with the parameters its first page was read with",
        /// A cursor handed out for another read through the same door.
        CursorOtherRead { door: String } => "the cursor was handed out for another `{door}`; continue it with the read its first page was read with",
        /// A step's depth range that is empty.
        DepthEmpty { min: u32, max: u32 } => "a step's depth range {min}..={max} is empty",
        /// Two group keys of one name.
        GroupKeyTwice { name: FieldName } => "two group keys are named `{name}`; each needs a name of its own",
        /// An instant of an `after` that is no text.
        InstantNotText => "an instant is given as text",
        /// A `latest` order where no record has a change of its own.
        LatestNoChange { #[serde(rename = "type")] ty: Option<TypeName> } => "`order_by` `latest` orders records by their latest change, and [`{ty}` is a view, whose records are computed and have none|a query's rows are projections of a walk, not records]",
        /// A `latest` order given a field.
        LatestTakesNoField { field: FieldName } => "`latest` orders by the position of each record's latest change and takes no field, and `{field}` was given",
        /// A limit out of range.
        LimitRange { max: u32 } => "a limit is between 1 and {max}",
        /// A measure's step with a depth.
        MeasureNoDepth { measure: String } => "`{measure}` is a measure over the steps' relations and takes no step `depth`: it reads each record's links, not hops",
        /// A measure with an order.
        MeasureNoOrder { measure: String } => "`{measure}` is a measure over the steps' relations and takes no `order_by`: its rows come in the measure's own order",
        /// A measure with no step.
        MeasureNoSteps { measure: String } => "`{measure}` walks the relations its steps name; name at least one",
        /// A measure with a `to`.
        MeasureNoTo { measure: String } => "`{measure}` is a measure over the steps' relations and takes no `to`",
        /// A measure's step with a `type`.
        MeasureNoType { measure: String } => "`{measure}` is a measure over the steps' relations and takes no step `type`: it reads every link of a relation, whatever it reaches",
        /// A measure's step with a `where`.
        MeasureNoWhere { measure: String } => "`{measure}` is a measure over the steps' relations and takes no step `where`",
        /// A step over a relation that its type does not walk that way.
        NoRelationToWalk { #[serde(rename = "type")] ty: TypeName, relation: RelationName, walk: Walk } => "`{ty}` has no relation `{relation}` to walk {walk}",
        /// A field a search names that is not searchable.
        NotSearchable {
            field: FieldName,
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{field}` of `{ty}` is not searchable",
        /// An `offset` beside another start of the answer.
        OffsetBeside { other: String } => "`offset` and `{other}` both say where the answer starts; give one",
        /// A grant's `only` that cannot be applied to the read.
        OnlyUnappliable {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "the grant's `only` on `{ty}` cannot be applied here; the host's grants need fixing",
        /// An order by a field computed at the read.
        OrderAtRead { field: FieldName, #[serde(rename = "type")] ty: TypeName } => "`order_by` names `{field}` of `{ty}`, which is computed at the read and moves with the clock; a page's order would not hold to the next",
        /// An order by a value or a time that names no field.
        OrderNamesNoField => "an order by a value, `known_at` or `seen_at` names its field",
        /// A view's parameter not given, or met where no read binds it.
        ParamMissing { #[serde(rename = "type")] ty: TypeName, name: ParamName, kind: StorageKind, unbound: bool } => "view `{ty}` needs parameter `{name}` ({kind})[{?unbound}, and only a read that names the view binds it]",
        /// A parameter given that the type does not declare.
        ParamUnknown { #[serde(rename = "type")] ty: TypeName, name: ParamName, declared: Option<Names> } => "`{ty}` declares no parameter `{name}`, so it binds nothing[; its parameters are {declared}]",
        /// A parameter's value not of its kind.
        ParamValue { name: ParamName, #[serde(rename = "type")] ty: TypeName, kind: StorageKind, value: Box<Value> } => "parameter `{name}` of view `{ty}` is {kind}, and {value} is not one",
        /// A path with several starts.
        PathStarts { count: usize } => "a path has one start, and this pattern's start names {count} records",
        /// A pattern naming a binding where no row binds by name.
        PatternAsNotRows => "`as` names what a row binds, and only rows and an aggregate bind by name",
        /// A pattern that binds two records under one name.
        PatternBindsTwice { name: String } => "the pattern binds `{name}` already; give this step another name with `as`",
        /// A path a pattern orders, projects or aggregates by that no row holds.
        PatternNoPath { path: String } => "`{path}` is no path the pattern's rows hold; a row holds `<name>.<field>` for each name the pattern binds, its type's or the one `as` gives",
        /// A pattern with `to` whose steps are a chain.
        PatternToChain => "a pattern with `to` walks its steps as one walk from its start; every step's depth starts at 0",
        /// A pattern with `to` not projected as a graph.
        PatternToNotGraph => "a pattern with `to` answers the walk to it as a graph; ask `project` `graph`",
        /// A position of an `after` that is no whole number.
        PositionNotInteger => "a position is a non-negative integer",
        /// An order of proposals by what is not one of their members.
        ProposalOrder { members: Names, field: FieldName, by: OrderBy } => "`order_by` orders proposals by their members ({members}) and their values, and `{field}` by `{by}` is not one",
        /// A query vector of another length than the field declares.
        QueryDimensions { query: usize, held: usize } => "the query vector has {query} dimensions, and the field holds vectors of {held}",
        /// A search's `fields` that names none.
        SearchFieldsEmpty => "`fields` names no field; leave it out to search every searchable field",
        /// A hybrid search without a vector over a vector field no model
        /// fills: its writers chose the space, so no bound model may stand
        /// in for one.
        SearchNeedsVector { field: FieldName } => "`{field}` holds vectors its writers make, in a space the lake does not know; a hybrid search over it brings its own query vector",
        /// A vector ranking over a field that holds no vectors.
        SearchNotVector { field: FieldName, kind: StorageKind } => "`{field}` is a {kind} field; a vector ranking reads a vector field",
        /// A step that reaches another type than it names.
        StepReaches { reached: TypeName, #[serde(rename = "type")] ty: TypeName, also: Option<Names> } => "it reaches `{reached}`[, {also}], not `{ty}`",
        /// A step reaching several types where a `where`, a row or the next step reads one.
        StepSeveralTypes => "it reaches several types, and a `where`, a row or the step after it reads one; name the step's `type`",
        /// A step deeper than the lake walks.
        StepTooDeep { max: u32, asked: u32 } => "a step may walk at most {max} hops; this one asks for {asked}",
        /// A `text_weight` out of range.
        TextWeightRange => "`text_weight` must be above 0 and at most 1000000",
        /// A `total` beside an aggregate.
        TotalWithAggregate => "`total` counts the records a page lists; an `aggregate` answers groups, so give one",
        /// A query vector with no direction to rank along.
        VectorAimless { field: FieldName, why: Aimless } => "`{field}`'s vector {why}",
        /// A vector field a search names twice.
        VectorFieldTwice { field: FieldName } => "`{field}` is named twice; a search ranks a vector field once",
        /// A search of a view.
        ViewNotSearched { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is a view, and a view is not searched: its records are computed on read and no index holds them",
        /// A view that reads itself through views the reader did not check.
        ViewReadsItself { #[serde(rename = "type")] ty: TypeName, chain: Names } => "view `{ty}` reads itself through other views, reading {chain} in turn, so it has no records",
        /// A `weight` on a vector search of one field.
        WeightAlone => "`weight` weighs one vector field's ranking against another's, and the search names no other in `also`",
        /// A field's weight out of range.
        WeightRange { field: FieldName } => "the weight of `{field}` must be above 0 and at most 1000000",

        // ---- writes
        /// A host's cause past the head.
        CausePastHead { cause: Position, head: Option<Position> } => "the cause {cause} is past the lake's head [{head}|(none)]",
        /// A type declared again since its migration was proposed.
        ChangedSinceProposed { #[serde(rename = "type")] ty: TypeName } => "`{ty}` changed since this migration was proposed; propose it again",
        /// A field added that the type has.
        FieldExists { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}` already has a field `{field}`",
        /// A field taken away that another part of the type reads.
        FieldReadElsewhere { field: FieldName, #[serde(rename = "type")] ty: TypeName } => "`{field}` is read by another part of `{ty}` and cannot be removed",
        /// A view at a door that writes or holds a record.
        IsAView {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{ty}` is a view; it is computed, never written",
        /// A key component of a kind its field cannot read it as.
        KeyComponentKind { part: String, kind: StorageKind, field: FieldName } => "the key component `{part}` is not a {kind} for `{field}`",
        /// A key with another number of components than the type's key fields.
        KeyComponents { #[serde(rename = "type")] ty: TypeName, fields: usize, key: RecordKey, parts: usize } => "`{ty}` is keyed by {fields} field(s), and the key `{key}` has {parts} component(s)",
        /// A key its key fields would make otherwise.
        KeyDisagrees { key: RecordKey, #[serde(rename = "type")] ty: TypeName, derived: RecordKey } => "the key `{key}` and the key fields of `{ty}` (which make `{derived}`) disagree",
        /// A key that is empty or holds a NUL.
        KeyEmpty => "a record key is not empty and holds no NUL character",
        /// A link's end given among its fields.
        LinkEndAsField { end: FieldName } => "a link's `{end}` is one of its ends, which `link` takes as `key` and `to`; no write sets it among the link's fields",
        /// A link from a record that links already where one is allowed, to a record named when the caller is shown it.
        LinkedAlreadyFrom { from: RecordKey, relation: RelationName, #[serde(default, skip_serializing_if = "Option::is_none")] to: Option<RecordKey>, cardinality: Cardinality } => "`{from}` already links over `{relation}`[ to `{to}`], and it allows one ({cardinality}); unlink it first",
        /// A link to a record that is linked already where one is allowed, from a record named when the caller is shown it.
        LinkedAlreadyTo { to: RecordKey, relation: RelationName, #[serde(default, skip_serializing_if = "Option::is_none")] from: Option<RecordKey>, cardinality: Cardinality } => "`{to}` is already linked over `{relation}`[ from `{from}`], and it allows one ({cardinality}); unlink it first",
        /// A relation a link door names that link records do not carry.
        NoLinkRelation { #[serde(rename = "type")] ty: TypeName, relation: RelationName } => "`{ty}` has no relation `{relation}` carried by link records",
        /// A type that keeps no per-field provenance, purged.
        NoProvenance { #[serde(rename = "type")] ty: TypeName } => "`{ty}` keeps no per-field provenance; a purge cannot read who brought its records in",
        /// A recompute of a type nothing of which is computed outside.
        NothingComputedOutside { #[serde(rename = "type")] ty: TypeName } => "`{ty}` has no field computed outside the lake (an embedding, or a field of a function with a backing), so nothing of it is computed again: a derived field is as of its record's last write, and a function is named itself",
        /// A patch with no field.
        PatchEmpty => "nothing to write: the patch carries no field",
        /// A payload member a verb requires that is not given.
        PayloadRequired { verb: VerbName, member: FieldName } => "verb `{verb}` requires `{member}` in its payload",
        /// A payload member a verb does not declare.
        PayloadUndeclared { verb: VerbName, member: FieldName } => "verb `{verb}` has no `{member}` in its payload",
        /// A purge's report of another purge or head.
        PurgeReportStale => "that report is not of this purge at this head; run the preflight again",
        /// A purge of records another hand wrote too.
        PurgeShared { source: SourceId, #[serde(rename = "type")] ty: Option<TypeName>, count: usize, #[serde(with = "closed::list")] first: Vec<RecordName> } => "the purge of `{source}`[ from `{ty}`] is refused whole: {count} record(s) it brought in were also written by another hand (a principal, or another source), and a purge never half-removes a record; retire them or clear those fields, then purge again. First: {first}",
        /// A record with no value for a required field.
        Required { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}` requires a value for `{field}`",
        /// A verb's `requires` that does not hold.
        RequiresUnmet { verb: VerbName, #[serde(rename = "type")] ty: TypeName, key: RecordKey } => "verb `{verb}` requires what `{ty}` `{key}` does not hold",
        /// A restriction of what is restricted, or a release of what is not.
        Restriction { #[serde(rename = "type")] ty: TypeName, key: RecordKey, already: bool } => "`{ty}` `{key}` is [{?already}already|not] restricted",
        /// A set of declarations that names one twice.
        SetDeclaresTwice => "the set declares it more than once",
        /// A set of no declarations.
        SetEmpty => "nothing to apply: the set holds no declaration",
        /// A set of declarations with several problems.
        SetRefused { count: usize, problems: Vec<super::Error> } => "the set of `declarations` is refused whole, with {count} problems: {problems}",
        /// A trigger's ceiling above the lake's.
        TriggerTooDeep { depth: u32, max: u32 } => "its `max_depth` {depth} is above the lake's `max_reactive_depth` {max}",
        /// A retry that finds no answer of the verb's executor recorded.
        Unanswered { position: Position, accepted: bool } => "[{?accepted}the acceptance|the call] is in the log at position {position}, and no answer of the verb's executor is recorded for it, so this retry runs nothing; if the call is still running, or its answer is held, its answer will be recorded and a retry then answers it, unless the handle that ran it is dropped or loses its lease first; if its first answer said that the verb's executor was not handed the call, [{?accepted}propose the call again|call the verb again under another key or with none]",
        /// A link type no relation carries.
        UnclaimedLinkType { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is the link type of no relation: its relation was renamed or removed",
        /// A value its value type's constraint does not hold for.
        ValueConstraint { value_type: ValueTypeName } => "the value does not satisfy the constraint of `{value_type}`",
        /// A verb's assignment that cannot be computed.
        VerbCannotCompute { verb: VerbName, field: FieldName, #[serde(with = "closed")] why: Box<Case> } => "verb `{verb}` cannot compute `{field}`: {why}",
        /// A verb on a record called without a key.
        VerbNeedsKey { verb: VerbName, #[serde(rename = "type")] ty: TypeName } => "verb `{verb}` is called on a `{ty}` record: it needs that record's key",
        /// A verb's accept after the verb was declared again.
        VerbRedeclared { verb: VerbName } => "verb `{verb}` was declared again, differently, after this call was proposed; propose the call again",
        /// A verb on its type called with a version.
        VerbTypeNoVersion { verb: VerbName, #[serde(rename = "type")] ty: TypeName } => "verb `{verb}` is called on the type `{ty}`: there is no record version to compare",
        /// A verb on its type called with a key.
        VerbTypeTakesNoKey { verb: VerbName, #[serde(rename = "type")] ty: TypeName } => "verb `{verb}` is called on the type `{ty}`, not on a record: it takes no key",
        /// A verb's assignment computing what its field does not take.
        VerbValueUnfit { verb: VerbName, field: FieldName } => "verb `{verb}` computes a value that does not fit `{field}`",
        /// A link one ingest window made already where one is allowed.
        WindowLinksAlready { from: RecordKey, to: RecordKey, relation: RelationName, cardinality: Cardinality } => "the window links `{from}` to `{to}` over `{relation}` already, and it allows one ({cardinality})",
        /// A pinned proposal's accept after its record or type changed.
        WriteChangedSinceProposed { #[serde(rename = "type")] ty: TypeName, #[serde(default, skip_serializing_if = "Option::is_none")] key: Option<RecordKey> } => "`{ty}`[ `{key}`] changed since this write was proposed; propose it again",

        // ---- aging
        /// An aging rule's field that its door does not write.
        AgingAuthority { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}`.`{field}` is not `authored` or `shared`; aging writes it as `upsert` does",
        /// An aging rule on a key field.
        AgingKey { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}`.`{field}` is part of the key; aging never moves a key",
        /// An aging rule's member with no value.
        AgingNoValue { member: FieldName } => "an aging rule's `{member}` holds a value",
        /// An aging day that is no UTC midnight.
        AgingNotADate { today: Timestamp } => "`today` is a date, at UTC midnight; `{today}` is not",
        /// An aging rule's value that its field does not take.
        AgingNotAValue { #[serde(rename = "type")] ty: TypeName, field: FieldName, cause: Box<super::Error> } => "not a value of `{ty}`.`{field}`: {cause}",
        /// An aging rule's member that is no text.
        AgingNotText { member: FieldName } => "an aging rule's `{member}` is text",
        /// An aging rule that moves a value to itself.
        AgingSameValue => "`to` equals `from`; a rule moves a value to another",
        /// An aging rule on a system type.
        AgingSystemType { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is a system type; aging never moves it",

        // ---- settings
        /// A type named as a stored type that is none.
        NoStoredType {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{ty}` is no declared stored type",
        /// A type named for an order index that is neither stored nor a view.
        NoStoredTypeOrView {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{ty}` is no declared stored type or view",
        /// A type named as a view that is none.
        NoView {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{ty}` is no declared view",
        /// A field named as a vector field that is none.
        NotVectorField { field: FieldName } => "`{field}` is no vector field",
        /// An order index of a field computed at the read.
        OrderIndexAtRead { field: FieldName } => "`{field}` is computed at the read; an order index keeps stored values",
        /// An order index of a field of a kind it does not keep.
        OrderIndexKind { field: FieldName, kind: StorageKind } => "`{field}` is of kind {kind}; an order index keeps text, number, decimal, boolean, date-time, date and reference fields",
        /// An index setting holding something other than field names.
        SettingFieldNames { value: Value } => "it holds field names, and {value} is not one",
        /// An index setting that is no list.
        SettingIndexShape {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "it holds an array of field names of `{ty}`, or null for no index",
        /// A `view_kept` setting that holds no boolean.
        SettingKeptHolds => "it holds `true` for a view that is kept, or `false`",
        /// A `search_weights` setting of the wrong shape.
        SettingWeightsShape => "it holds an object of `fields` (vector fields and their weights) and `text_weight`, one at least, or null",
        /// A vector index of a field computed at the read.
        VectorIndexAtRead { field: FieldName } => "`{field}` is computed at the read; a vector index keeps stored values",
        /// A vector index of a field that is no vector.
        VectorIndexKind { field: FieldName, kind: StorageKind } => "`{field}` is of kind {kind}; a vector index keeps vector fields",
        /// A view that cannot be kept, to one not shown why.
        ViewNotKeepable {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "the view `{ty}` cannot be kept",

        // ---- migrations
        /// A value a migration renames that a computed field's expression holds.
        ComputedHoldsRenamed { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}`.`{field}` is computed from an expression that holds a value this migration renames; change the expression first",
        /// A key field that is not required in a migration that changes the key.
        KeyFieldRequired { field: FieldName } => "the key field `{field}` is a required field of the type",
        /// A value a migration renames that a key field holds.
        KeyHoldsRenamed { #[serde(rename = "type")] ty: TypeName, field: FieldName } => "`{ty}`.`{field}` is part of the key and holds a value this migration renames, which would leave records whose key no longer matches their fields; re-key the type through its migration first",
        /// A migration of a view.
        MigrationOfView { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is a view, and a view changes by being declared again, never by a migration",
        /// A migration that declares another type.
        MigrationOtherType { from: TypeName, to: TypeName } => "a migration of `{from}` declares the type `{to}`",
        /// A migration applied against a report of another head.
        MigrationStale => "the lake moved since that preflight report; run the preflight again",
        /// A migration that takes every key field away.
        MigrationTakesKey { #[serde(rename = "type")] ty: TypeName } => "the migration takes every key field of `{ty}` away, and leaves no rule to make its records' keys",
        /// A migration that takes away a field a declaration reads.
        MigrationTakesRead { unnamed: bool, field: FieldName, #[serde(rename = "type")] ty: TypeName, renamed: Option<FieldName> } => "[{?unnamed}a declaration|it] reads `{field}` of `{ty}`, which the migration [renames to `{renamed}`|drops]",
        /// A migration that would make a stored type a view.
        MigrationToView { #[serde(rename = "type")] ty: TypeName } => "the migration would turn the stored type `{ty}` into a view; declare the view under another name, and the type keeps its name and its records",
        /// A migration that would make a view a stored type.
        MigrationViewToStored { #[serde(rename = "type")] ty: TypeName } => "`{ty}` is a view, and it becomes a stored type by being declared as one, never by a migration",
        /// A narrowing of a cardinality that existing links break.
        NarrowingBreaksLinks { relation: RelationName, cardinality: Cardinality, count: usize } => "narrowing `{relation}` to {cardinality} would break the links of {count} record(s) that hold more than one; unlink them first",
        /// A declaration over a migrated type that could not be declared again.
        NoLongerDeclarable { #[serde(rename = "type")] ty: TypeName } => "a declaration over `{ty}` could no longer be declared",
        /// A field taken away that a declaration the author does not know reads.
        ReadsTakenAway => "a declaration reads a field the migration takes away",
        /// Re-keying that moves keys made of each other.
        RekeyRing { #[serde(rename = "type")] ty: TypeName, ring: Names } => "re-keying `{ty}` would move keys that are made of keys it moves, in {ring}; no order makes them, so that is not supported",
        /// Re-keying that would re-key a system type.
        RekeySystem { #[serde(rename = "type")] ty: TypeName, system: TypeName } => "re-keying `{ty}` would re-key the system type `{system}`, which is keyed by it; that is not supported",
        /// Re-keying that would re-key a view.
        RekeyView { #[serde(rename = "type")] ty: TypeName, view: TypeName } => "re-keying `{ty}` would re-key the view `{view}`, which is keyed by it; a view's rows are computed and cannot be moved, so that is not supported",
        /// A relation rename that changes more than the name.
        RelationRenameChanges => "a rename keeps the relation's carrier, target, cardinality, link fields and authority; change them in a migration of their own",
        /// A relation rename naming relations that are not there.
        RelationRenameNamesNone => "the first names a relation the type has, the second one the new shape has",
        /// A relation renamed twice, or to a name in use.
        RelationRenameNotOnce => "a relation is renamed once, to a name no relation of the type bears, and leaves its old name free",
        /// A field renamed to one that keeps its own values.
        RenameCollides { to: FieldName } => "rename to `{to}`: that field already exists and keeps its values",
        /// A field renamed from one the type does not have.
        RenameFromUnknown { from: FieldName } => "rename from `{from}`: no such field before the migration",
        /// A relation rename to a name whose links the lake keeps.
        RenameNameHeld => "the lake keeps the links of an earlier relation of that name; take another name",
        /// A relation rename whose link type another declaration refers to.
        RenameReferredLinks { #[serde(rename = "type")] ty: Option<TypeName>, links: TypeName } => "[`{ty}`|a declaration] refers to its link type `{links}`, whose records would move; retarget it first",
        /// A field renamed from or to twice.
        RenameRepeated { field: FieldName } => "the field `{field}` is renamed more than once",
        /// A field renamed to one the new declaration does not have.
        RenameToUnknown { to: FieldName } => "rename to `{to}`: no such field after the migration",
        /// A relation rename in a migration that re-keys.
        RenameWhileRekeying => "the same migration re-keys records; rename in one migration and re-key in another",
        /// A declaration that reshapes stored records.
        Reshapes { #[serde(rename = "type")] ty: TypeName, #[serde(with = "closed::list")] reasons: Vec<Revision> } => "`{ty}` would reshape stored records ({reasons}); that is a migration",
        /// A declaration that reshapes stored links.
        ReshapesLinks { #[serde(rename = "type")] ty: TypeName, links: TypeName, #[serde(with = "closed::list")] reasons: Vec<Revision> } => "`{ty}` would reshape the links `{links}` holds ({reasons}); that is a migration",
        /// A rename's new value the new declaration does not accept.
        ValueRenameNewRefused => "the new value is one the new declaration accepts",
        /// A value rename of a value type that is not text.
        ValueRenameNotText { value_type: ValueTypeName } => "value type `{value_type}` is not text: only a text value type renames its values",
        /// A rename's old value the value type in force does not accept.
        ValueRenameOldRefused => "the old value is one the value type in force accepts",
        /// A value renamed to itself.
        ValueRenameSame => "a value is renamed to another value",
        /// A value renamed twice.
        ValueRenameTwice => "a value is renamed once",
        /// A value type declared again since its migration was proposed.
        ValueTypeChangedSinceProposed { value_type: ValueTypeName } => "value type `{value_type}` changed since this was proposed; propose it again",
        /// A value type tightened while stored values would not fit it.
        ValueTypeNotTightened { value_type: ValueTypeName, misfits: u64, #[serde(with = "closed::list")] first: Vec<RecordName> } => "value type `{value_type}` is not tightened: {misfits} stored value(s) or a field's declared `initial` would not fit it; change them first.[ First: {first}]",
        /// A value type declared with another kind or scale.
        ValueTypeReshapes { value_type: ValueTypeName } => "value type `{value_type}` would change its `kind` or `scale`, which rewrites stored values; that is not a value-type migration: migrate the fields that use it",
        /// A value type's migration that changes nothing.
        ValueTypeSame { value_type: ValueTypeName } => "value type `{value_type}` would accept exactly what it does; there is nothing to migrate",
        /// A value type declared to accept less.
        ValueTypeTightens { value_type: ValueTypeName } => "value type `{value_type}` would accept less than it does: stored values may stop fitting it; tighten it through `migrate`, whose preflight counts them, or propose it as a value type's migration",

        // ---- values
        /// A geometry's coordinates of another shape than its type's.
        GeoCoordinates { geometry: String } => "not a GeoJSON geometry: the `coordinates` are not those of a {geometry}",
        /// A geometry with no coordinates.
        GeoNoCoordinates { geometry: String } => "not a GeoJSON geometry: a {geometry} needs `coordinates`",
        /// A geometry collection with no geometries.
        GeoNoGeometries => "not a GeoJSON geometry: a GeometryCollection needs a `geometries` array",
        /// A geometry with no type.
        GeoNoType => "not a GeoJSON geometry: no string member `type`",
        /// A geometry that is no object.
        GeoNotObject => "not a GeoJSON geometry: not an object",
        /// A geometry of a type GeoJSON does not have.
        GeoUnknownType { name: String } => "not a GeoJSON geometry: \"{name}\" is not a geometry type",
        /// An integer beyond what every reader reads exactly.
        IntegerTooLarge { n: Number } => "{n} is beyond ±(2^53 − 1), the largest integer every reader reads exactly",
        /// Bytes that are not base64.
        NotBase64 => "not standard base64 with padding",
        /// A date-time that is no UTC instant.
        NotDateTime { value: String } => "not a UTC date-time: \"{value}\"",
        /// A date that is no calendar date.
        NotDate { value: String } => "not a date: \"{value}\" is neither `YYYY-MM-DD` nor an instant at a UTC midnight",
        /// A decimal that is not digits.
        NotDecimal { value: String } => "not a decimal: \"{value}\"",
        /// A value its value type's format refuses.
        NotFormat { format: String, value_type: ValueTypeName } => "the value is not a well-formed {format} (`{value_type}`)",
        /// A value its value type does not list.
        NotOneOf { value: Value, allowed: Names } => "{value} is not one of {allowed}",
        /// A value of another JSON form than its kind's.
        ValueShape { kind: StorageKind, shape: Shape } => "a {kind} value must be {shape}",
        /// A vector of another length than declared.
        VectorLength { found: usize, expected: u32 } => "the vector has {found} numbers, not {expected}",
        /// A vector field with no length.
        VectorNoDimensions => "the vector field declares no dimensions",

        // ---- backings
        /// A backing's call that cannot be made as its function configures it.
        CallNotSent { #[serde(with = "closed")] exchange: Box<Exchange> } => "the `http` call is not sent: {exchange}",
        /// An `exec` function naming a command the operator did not configure.
        CommandNotAllowed { command: String } => "the command `{command}` is not one the operator allows",
        /// Texts an embedding backend refuses for what they are.
        EmbeddingRefused { model: String, status: Option<u16> } => "the embedding backend `{model}` refused the texts[: it answered {status}]",
        /// An `exec` function that names no command.
        ExecNoCommand => "an `exec` function names its command as `config.command`",
        /// An `expr` backing's call with no expression.
        ExprConfig { fault: JsonFault } => "its `expr` backing needs `config.expr`, an expression: {fault}",
        /// An `http` function's URL whose host the operator does not allow.
        HostNotAllowed { host: String, port: u16 } => "the host `{host}` (port {port}) is not one the operator allows",
        /// An `http` function that names no URL.
        HttpNoUrl => "an `http` function names its URL as `config.url`",
        /// A function's call whose inputs are no object.
        InputsNotObject { value: Value } => "its inputs are {value}, not an object of values by name",
        /// A backing that answered no value.
        NoOutputValue { function: String } => "`{function}` answered no value",
        /// A URL that does not parse.
        NotUrl => "not a URL",
        /// A backing's output its field does not take.
        OutputNotValue { function: String, field: FieldName, cause: Box<super::Error> } => "the output of `{function}` is not a value of `{field}`: {cause}",
        /// A URL that names no host.
        UrlNoHost => "a URL that names no host",
        /// A URL of another scheme than `http` or `https`.
        UrlScheme => "neither an `http` nor an `https` URL",

        // ---- engines
        /// A segment to append to, named for another position than the event's.
        ActiveSegmentNamed { named: Position, position: Position } => "the active segment is named for position {named}, and the event to append is at position {position}; nothing was appended",
        /// An event with a line feed in its bytes.
        AppendLineFeed { position: Position } => "the event at position {position} holds a line feed in its bytes, which canonical bytes never do; nothing was appended",
        /// An event not at the log's next position.
        AppendNotNext { position: Position, next: u64 } => "the event at position {position} does not continue the log, whose next position is {next}; nothing was appended",
        /// An event whose `prev` is not the hash before it.
        AppendPrev { position: Position, named: String, hash: String } => "the event at position {position} names `{named}` as the hash before it, but that hash is `{hash}`; nothing was appended",
        /// A PostgreSQL client certificate without its key, or a key without its certificate.
        ClientCertHalf => "a client certificate needs both `tls.client_cert` and `tls.client_key`",
        /// A commit through a position the projection is past.
        CommitBehind { folded: Position, through: Position } => "the projection reflects the log through position {folded}; a transaction cannot commit through the earlier position {through}",
        /// A commit of a state valid from a later position.
        CommitLate { late: Position, through: Position } => "a state valid from position {late} cannot be committed through the earlier position {through}",
        /// A commit with no transaction begun.
        CommitUnbegun => "there is no transaction to commit: call `begin` first",
        /// A compaction while a transaction is open.
        CompactInTransaction => "the projection is compacted between appends: a transaction is open",
        /// A location configured for another engine than the one opening it.
        EngineOtherLocation { engine: String, location: String, lake: Box<LakeId> } => "the {engine} engine opens only {location} locations, and lake `{lake}` is configured for another engine",
        /// An export into a directory that holds something.
        ExportNotEmpty => "the export's directory is not empty; an export writes only into an empty or new directory",
        /// An event whose fence is below the one before it.
        FenceFalls { position: Position, fence: u64, floor: u64 } => "the event at position {position} carries fence {fence}, below the fence {floor} of the event before it; fences never fall, and nothing was appended",
        /// An event whose fence is not the one it must carry.
        FenceWrong { position: Position, fence: u64, wanted: u64 } => "the event at position {position} carries fence {fence}, and it must carry fence {wanted}; nothing was appended",
        /// An import into a lake whose log holds events not imported.
        ImportIntoHeld { tip: Position } => "the lake's log holds events up to position {tip} that were not imported; an import goes only into a lake that holds nothing else, and nothing was appended",
        /// An import into a lake that holds events.
        ImportNotEmpty { lake: Box<LakeId>, head: Position } => "the lake `{lake}` already holds events through position {head}; an import makes a new lake and goes only into one that holds nothing",
        /// An export holding another lake's events.
        ImportOtherLake { position: Position, held: Box<LakeId>, lake: Box<LakeId> } => "the export's directory holds at position {position} an event of the lake `{held}`, and the import makes `{lake}`; a lake's name is inside every hash, so an export opens only as the lake it came from; nothing was imported",
        /// A lake's storage that holds another lake: a PostgreSQL schema, or a directory.
        LakeOther { schema: Option<String>, held: String, lake: Box<LakeId> } => "[the schema `{schema}`|the lake's directory] holds the lake `{held}`, not `{lake}`",
        /// A PostgreSQL location's URL that names no user.
        LocationUrlNoUser => "the location's URL names no user; name the one that connects, as `postgres://<user>@<host>/<database>`",
        /// A PostgreSQL location's URL that carries a TLS key.
        LocationUrlTls { key: String } => "the location's URL carries `{key}`; TLS is configured by the location's `tls` alone, so remove it from the URL",
        /// A PostgreSQL location's URL that does not parse.
        LocationUrlUnparsed { detail: super::Detail } => "the location's `url` is not a PostgreSQL URL: {detail}",
        /// A name ended whole that is not a head-only name.
        NotHeadOnly {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "`{ty}` is not a head-only name, so it is not ended whole",
        /// A put outside a transaction.
        NotStaged => "the projection is written inside a transaction: call `begin` first",
        /// A PostgreSQL schema whose `meta` or `lease` table is not a lake's.
        SchemaMetaForeign { schema: String } => "the schema `{schema}` holds a `meta` or `lease` table that is not a lake's",
        /// A PostgreSQL schema name an identifier cannot hold.
        SchemaName { schema: String } => "the schema `{schema}` cannot be a PostgreSQL identifier: a name of 1 to 63 bytes without NUL is needed; name one in the location's `schema`",
        /// A PostgreSQL schema holding tables that are not a lake's, or no `meta`.
        SchemaNotALake { schema: String, tables: Option<Names> } => "the schema `{schema}` holds tables that are not a lake's ([{tables}|it has no `meta`]); a lake needs a schema of its own",
        /// A seal handed no first event of the next segment.
        SealNoFirst { after: Position } => "a seal begins the next segment with its first event, and none was handed for the one after position {after}; nothing was sealed",

        // ---- doors
        /// A migration applied over REST without its preflight's report.
        ApplyNeedsReportId => "applying a migration needs the `report_id` its preflight answered",
        /// Arguments that do not fit what their door reads.
        ArgumentsUnfit { fault: JsonFault } => "what is given does not fit the tool's `inputSchema`: {fault}",
        /// An ingest body cut off.
        BodyCutOff { line: usize } => "the body was cut off at line {line}",
        /// A body of another media type than JSON.
        BodyMediaType => "send the body as application/json",
        /// A body that is not JSON.
        BodyNotJson { fault: JsonFault, row: usize, column: usize } => "the body does not parse as `application/json`: {fault}, at line {row} column {column}",
        /// A body that is no JSON object of the arguments.
        BodyNotObject { route: String } => "`{route}` takes a JSON object",
        /// A body that is no JSON object of the one argument it is.
        BodyObjectOf { route: String, argument: String } => "`{route}` takes a JSON object of {argument}",
        /// A body over the size a door reads.
        BodyTooLarge => "the body is too large or was cut off",
        /// A change filter naming an empty key.
        FilterKeyEmpty => "a feed filter names an empty key",
        /// A typed condition sent where REST takes text.
        FindWhereText => "over REST, `find` takes `where` in its text form only; give the condition as text",
        /// An idempotency key a header cannot carry.
        IdempotencyKeyHeader => "the idempotency key holds a character a header cannot carry, such as a line break",
        /// Both compare-and-set headers.
        IfMatchBoth => "`If-Match` and `If-None-Match` cannot both be given",
        /// An `If-Match` that names no version.
        IfMatchNoVersion { value: String } => "`If-Match: {value}` names no version",
        /// An `If-None-Match` on a route other than `PUT`.
        IfNoneMatchNotPut => "only `PUT` takes `If-None-Match`",
        /// An `If-None-Match` other than `*`.
        IfNoneMatchValue { value: String } => "`If-None-Match: {value}` is not taken",
        /// An ingest body of another media type than NDJSON.
        IngestMediaType => "ingest takes application/x-ndjson",
        /// An ingest line that is not a record.
        LineNotRecord { line: usize, fault: JsonFault, column: usize } => "line {line} is not a record `ingest` takes: {fault}, at column {column}",
        /// An ingest line over the length one may have.
        LineTooLong { line: usize, max: usize } => "line {line} is longer than {max} bytes",
        /// A member the arguments need and do not give.
        MemberMissing { member: String } => "the member `{member}` is missing",
        /// A migration naming both a type and a value type.
        MigrationBoth => "a migration names a type (`to`) or a value type (`value_type`), not both",
        /// A migration naming neither a type nor a value type.
        MigrationNeither => "a migration names a type (`to`) or a value type (`value_type`)",
        /// An argument a tool's input schema does not list.
        NoArgument { tool: String, argument: String } => "`{tool}` takes no argument `{argument}`; nothing was done",
        /// A body sent to a route that takes none.
        NoBody { route: String } => "`{route}` takes no body",
        /// A member the arguments do not take where it sits.
        NoMember { member: String, takes: Option<Names> } => "there is no member `{member}`[; it takes {takes}]",
        /// A query parameter a route does not take.
        NoParameter { route: String, parameter: String, takes: Option<Names> } => "`{route}` takes no parameter `{parameter}`; nothing was done[; it takes {takes}]",
        /// A path no route has.
        NoRoute => "there is no such route",
        /// A session's door asked where there is no session.
        NoSession { tool: String } => "`{tool}` belongs to an MCP session; there is none here",
        /// A pin asked where there is no session.
        NoSessionToPin { tool: String } => "`{tool}` pins an MCP session; there is none here",
        /// A query parameter's value not of the form it takes.
        ParameterForm { parameter: String, value: String, form: Form } => "`{parameter}` cannot be `{value}`: it takes {form}",
        /// A query parameter given twice.
        ParameterTwice { parameter: String } => "`{parameter}` is given twice",
        /// A body member the path names already.
        PathNames { argument: String } => "the path already names `{argument}`",
        /// A migration's preflight over REST given a `report_id`.
        PreflightReportId => "the preflight takes no `report_id`",
        /// A query that is not percent-encoded UTF-8.
        QueryNotEncoded => "the query is not valid percent-encoded UTF-8",
        /// A route asked with a method it does not take.
        RouteMethod { allow: Names, method: String } => "this route takes {allow}, not `{method}`",
        /// An empty path segment.
        SegmentEmpty { part: Segment } => "{part} is empty; over REST it is a path segment, and an empty one names another route",
        /// A value type's migration naming relations to rename.
        ValueTypeNoRelations => "a value type has no relations to rename; `relation_renames` belongs to `to`",
        /// A compare-and-set header on a verb called on its type.
        VerbTypeHeader { header: String, verb: VerbName } => "`POST …/verbs/<verb>` takes no `{header}`: verb `{verb}` is called on its type, which has no record version",
    }
}

catalog! {
    tag = "kind";
    /// A value an expression computed, as a case names it.
    pub enum Operand {
        /// Null.
        Null => "null",
        /// A boolean.
        Boolean { value: bool } => "the boolean [{?value}true|false]",
        /// A number.
        Number { value: String } => "the number `{value}`",
        /// Text.
        Text { value: String } => "the text '{value}'",
        /// An instant.
        Instant { value: Timestamp } => "the instant `{value}`",
        /// A duration.
        Duration { micros: i64 } => "a duration of {micros} microseconds",
        /// A JSON array or object.
        Json => "a JSON value",
    }
}

catalog! {
    /// Why a pattern is not an I-Regexp.
    pub enum Regexp {
        /// An escape the grammar does not have.
        BadEscape { c: char } => "`\\{c}` is not an I-Regexp escape",
        /// A category escape with no category in braces.
        CategoryBrace { p: char } => "`\\{p}` must be followed by a category in braces",
        /// A class's range that ends in a category.
        CategoryRange => "a range cannot end in a category",
        /// A class not closed.
        ClassUnclosed => "a character class is not closed",
        /// A `-` inside a class, not first or last.
        DashInClass => "`-` inside a class must be first, last or escaped",
        /// A pattern ending in `\`.
        EndsInBackslash => "the pattern ends with `\\`",
        /// A group not closed.
        GroupUnclosed => "a group is not closed with `)`",
        /// A character that must be escaped.
        MustEscape { c: char } => "`{c}` must be escaped",
        /// A bracket inside a class.
        MustEscapeInClass { c: char } => "`{c}` must be escaped inside a class",
        /// A category the grammar does not know.
        NotCategory { name: String } => "`{name}` is not a Unicode general category",
        /// Braces that hold no repetition.
        NotRepetition { body: String } => "`{body}` between braces is not a repetition",
        /// A quantifier after a quantifier.
        QuantifierTwice => "a quantifier cannot follow a quantifier",
        /// A class's range that runs backwards.
        RangeBackwards { lo: char, hi: char } => "the range `{lo}-{hi}` runs backwards",
        /// A repetition whose least is more than its most.
        RepeatsBackwards { body: String } => "`{body}` between braces repeats at least more than at most",
        /// A character left after the whole pattern.
        Unmatched { c: char } => "unexpected `{c}` (an unmatched `)`?)",
    }
}

catalog! {
    tag = "reason";
    /// One difference between two declarations of a type; a field's names the relation whose links carry it.
    pub enum Revision {
        /// A field added; it reshapes when required.
        FieldAdded { field: FieldName, relation: Option<RelationName>, required: bool } => "[{?required}required|optional] field `{field}`[ of relation `{relation}`] added",
        /// A field removed; it reshapes.
        FieldRemoved { field: FieldName, relation: Option<RelationName> } => "field `{field}`[ of relation `{relation}`] removed",
        /// A field's type changed; it reshapes.
        FieldRetyped { field: FieldName, relation: Option<RelationName>, from: FieldType, to: FieldType } => "field `{field}`[ of relation `{relation}`] changes type from {from} to {to}",
        /// An optional field made required; it reshapes.
        FieldRequired { field: FieldName, relation: Option<RelationName> } => "field `{field}`[ of relation `{relation}`] becomes required",
        /// A required field made optional.
        FieldOptional { field: FieldName, relation: Option<RelationName> } => "field `{field}`[ of relation `{relation}`] becomes optional",
        /// The key's fields or their order changed; it reshapes.
        KeyChanged { from: Names, to: Names } => "the key changes from {from} to {to}",
        /// A relation added.
        RelationAdded { relation: RelationName } => "relation `{relation}` added",
        /// A relation removed; it reshapes.
        RelationRemoved { relation: RelationName } => "relation `{relation}` removed",
        /// A relation that points at another type; it reshapes.
        RelationRetargeted { relation: RelationName, from: TypeName, to: TypeName } => "relation `{relation}` points at `{to}` instead of `{from}`",
        /// A relation's cardinality changed; it reshapes unless it widens.
        CardinalityChanged { relation: RelationName, from: Cardinality, to: Cardinality } => "relation `{relation}` changes cardinality from {from} to {to}",
        /// A view's source, filter, grouping or measures changed.
        ViewChanged => "the view changes",
        /// A stored type made a view; it reshapes.
        BecameView => "a stored type becomes a view",
        /// A view made a stored type.
        StoppedBeingView => "a view becomes a stored type",
    }
}

/// A case and the places it is said at, outermost first: "type `T`, field
/// `x`: …". What a refusal of `invalid`, `constraint`, `precondition` or
/// `unanswered` says.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    /// The places, outermost first.
    pub at: Vec<Place>,
    /// The case.
    pub case: Case,
}

impl Said {
    /// The same case, said inside `place`, unless it is said there already.
    pub fn within(mut self, place: Place) -> Said {
        // A place is said once: a check inside one already says it.
        if self.at.first() != Some(&place) {
            self.at.insert(0, place);
        }
        self
    }

    /// Adds what it names, its places' and its case's, to `n`.
    pub fn names(&self, n: &mut Named) {
        self.at.names(n);
        self.case.names(n);
    }
}

impl From<Case> for Said {
    fn from(case: Case) -> Said {
        let at = Vec::new();
        Said { at, case }
    }
}

impl fmt::Display for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, p) in self.at.iter().enumerate() {
            let sep = if i == 0 { "" } else { ", " };
            write!(f, "{sep}{p}")?;
        }
        if !self.at.is_empty() {
            write!(f, ": ")?;
        }
        write!(f, "{}", self.case)
    }
}

/// On the wire: the case's object, with `at` beside its code when it has
/// places.
impl Serialize for Said {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let case = closed::serialize(&self.case, serde_json::value::Serializer)
            .map_err(serde::ser::Error::custom)?;
        let Value::Object(mut map) = case else {
            return Err(serde::ser::Error::custom("a case is an object"));
        };
        if !self.at.is_empty() {
            let at = self
                .at
                .iter()
                .map(|p| closed::serialize(p, serde_json::value::Serializer));
            let at = at
                .collect::<Result<Vec<_>, _>>()
                .map_err(serde::ser::Error::custom)?;
            map.insert("at".into(), Value::Array(at));
        }
        map.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Said {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Said, D::Error> {
        let mut map = Map::deserialize(d)?;
        let at = match map.remove("at") {
            Some(Value::Array(at)) => at.into_iter().map(closed::deserialize).collect(),
            Some(_) => return Err(serde::de::Error::custom("`at` is a list of places")),
            None => Ok(Vec::new()),
        };
        let at = at.map_err(serde::de::Error::custom)?;
        let case = closed::deserialize(Value::Object(map)).map_err(serde::de::Error::custom)?;
        Ok(Said { at, case })
    }
}

#[cfg(test)]
mod tests;
