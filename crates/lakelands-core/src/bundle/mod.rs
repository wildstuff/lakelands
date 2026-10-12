//! The ontology bundle: the files an author writes a lake's ontology in,
//! and the loader that turns them into declarations.
//!
//! A bundle is a directory of YAML files. [`load_dir`] reads one,
//! [`parse`] turns the files into the [`Declaration`]s a lake takes, and
//! whatever declares them (`lakelands apply`) sends those on. Loading is
//! all or nothing: a bundle with a problem is refused with every problem
//! found, each against its file, and yields no declarations at all, so a
//! lake never receives half an ontology.
//!
//! # Layout
//!
//! ```text
//! value_types/<name>.yaml     one value type per file
//! types/<name>.yaml           one type per file, with its relations
//! functions/<name>.yaml       one function per file
//! verbs/<name>.yaml           one verb per file
//! triggers/<name>.yaml        one trigger per file
//! object_sets/<name>.yaml     one object set per file
//! aging_rules/<name>.yaml     one aging rule per file
//! ```
//!
//! Files end in `.yaml` or `.yml`. The file name is the author's to
//! choose; the declaration's name is its `name` key. Anything whose path
//! has a segment starting with `.` is ignored, and so is a file outside
//! the lanes that is not YAML (a `README.md`). Refused: a YAML file
//! outside the seven lanes, a file in a lane that is not YAML, and a
//! directory inside a lane.
//!
//! # Rules every file follows
//!
//! - Keys are strict. A key the lane does not know is refused by name,
//!   never ignored, so a misspelling cannot quietly leave a setting at
//!   its default.
//! - A name is declared once in the whole bundle, across the lanes of
//!   declarations, and is neither empty nor contains the NUL character
//!   (the log separates names by it). An aging rule is a record, not a
//!   declaration: its name is its own, once among the bundle's rules.
//!   Type names starting `sys.` are reserved for system types and
//!   refused.
//! - Every type and value type a file refers to is declared in the
//!   bundle. A `sys.` type may be referred to; the lake checks it.
//! - The tool names a lake derives from type and verb names
//!   ([`lakelands_api::tools`]) must come out distinct and short enough;
//!   a bundle that would break that is refused here, before it reaches a
//!   lake.
//! - Expressions are written in the one grammar ([`crate::expr`]) as YAML
//!   strings, and each must parse. A string literal inside an expression
//!   is single-quoted, so the YAML around it is double-quoted:
//!   `"status = 'open'"`. The typed form (a mapping) is accepted too.
//! - Named parts of a declaration (fields, relations, measures, payload
//!   fields, inputs, assignments) are written as a mapping from name to
//!   entry, and keep the order they are written in.
//!
//! # Field types
//!
//! A field is its type alone (`amount: decimal`) or a mapping whose
//! `type` is one. A type is written short:
//!
//! - a storage kind: `text`, `number`, `decimal`, `boolean`, `date_time`,
//!   `date`, `bytes`, `json`, `geo`;
//! - `reference(<type>)`, `vector(<length>)`;
//! - a value type declared in the bundle (`iban`), which brings its
//!   storage kind; one declared over `reference` or `vector` takes the
//!   argument the same way (`partner_ref(partner)`).
//!
//! or as the contract's object, `{ kind, value_type, target, dimensions }`.
//! Either way a reference names its target, a vector its length, and a
//! value type matches the kind. A value type may not take a storage
//! kind's name.
//!
//! The full form of a field carries `type` and, all optional, `required`,
//! `authority` (`authored`, `source`, `shared`, `derived`), `derived`,
//! `computed_at` (`write`, `read`), `depends_on`, `on_input_change` (`keep`, `clear`), `initial`,
//! `sensitive`, `searchable`, `description` and `label`.
//!
//! # The lanes
//!
//! A value type, `value_types/iban.yaml`:
//!
//! ```yaml
//! name: iban
//! kind: text
//! constraint: "matches(value, '[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}')"
//! format: iban
//! description: An international bank account number.
//! ```
//!
//! `one_of` lists the allowed values of a text value type.
//!
//! A type, `types/invoice.yaml`:
//!
//! ```yaml
//! name: invoice
//! label: Invoice
//! description: A bill sent to a partner.
//! key: number                   # or a list: [year, number]
//! valid_from: issued_on
//! fields:
//!   number: text
//!   issued_on: date_time
//!   partner: reference(partner)
//!   account: iban
//!   net: { type: decimal, required: true }
//!   gross:
//!     type: decimal
//!     authority: derived
//!     derived: "net * 1.19"
//!     depends_on: net
//!     on_input_change: clear
//!   status: { type: text, searchable: true }
//!   summary_embedding: vector(1536)
//! relations:
//!   billed_to:
//!     target: partner
//!     cardinality: many_to_one
//!     via: partner
//!     inverse: invoices
//!     fields:
//!       since: date_time
//! ```
//!
//! `valid_to` and `field_meta` (on unless `false`) are also accepted. A
//! view type has a `view`, and declares the fields it computes: one for
//! each `group_by` field and each measure, of the kind its value holds. A
//! read answers declared fields only, so a measure without a field, or a
//! field nothing computes, is refused when the view is declared:
//!
//! ```yaml
//! name: open_totals
//! fields:
//!   partner: reference(partner)
//!   total: decimal
//!   invoices: number
//! view:
//!   source: invoice
//!   where: "status = 'open'"
//!   group_by: partner
//!   measures:
//!     total: "sum(net)"
//!     invoices: "count()"
//! ```
//!
//! A view's `params` are the values a read binds, each a name and a
//! storage kind, in the order written; its `where` and measures read them
//! with `param('name')`:
//!
//! ```yaml
//! name: due_up_to
//! fields:
//!   partner: reference(partner)
//!   invoices: number
//! view:
//!   source: invoice
//!   params:
//!     until: date_time
//!   where: "issued_on <= param('until')"
//!   group_by: partner
//!   measures:
//!     invoices: "count()"
//! ```
//!
//! A view may `merge` further sources on its key: each groups its own
//! records by fields of its own that line up with the view's `group_by`
//! in order, and has measures of its own, which the view's measures read
//! by name. A key one source lacks is measured over its empty group, so a
//! sum there is 0:
//!
//! ```yaml
//! name: payments_against_invoices
//! fields:
//!   partner: reference(partner)
//!   invoiced: decimal
//!   paid: decimal
//!   open: decimal
//! view:
//!   source: invoice
//!   group_by: partner
//!   measures:
//!     invoiced: "sum(net)"
//!     open: "invoiced - paid"
//!   merge:
//!     - source: payment
//!       group_by: payer
//!       measures:
//!         paid: "sum(amount)"
//! ```
//!
//! A function, `functions/gross.yaml`. The backing is `expr` unless
//! named; an `expr` function carries `expr`, any other backing carries
//! `config` (any YAML) and no `expr`:
//!
//! ```yaml
//! name: gross
//! description: The amount with tax.
//! inputs:
//!   net: decimal
//!   rate: { type: decimal, required: true }
//! expr: "net * (1 + rate)"
//! ```
//!
//! A verb, `verbs/pay_invoice.yaml`. `risk` is `low`, `medium` or
//! `high`. A verb has either `set`, the fields of its record it writes,
//! each from an expression over the record and `payload.<name>`, or
//! `executor`, the executor the host binds for a verb that reaches
//! outside the lake:
//!
//! ```yaml
//! name: pay_invoice
//! on: invoice
//! risk: low
//! description: Record that an invoice was paid.
//! requires: "status = 'open'"
//! payload:
//!   paid_on: { type: date_time, required: true }
//! set:
//!   status: "'paid'"
//!   paid_on: payload.paid_on
//! ```
//!
//! ```yaml
//! name: send_reminder
//! on: invoice
//! risk: medium
//! executor: mailer
//! ```
//!
//! A trigger, `triggers/invoice_paid.yaml`:
//!
//! ```yaml
//! name: invoice_paid
//! on: invoice
//! when: "changed(status) and status = 'paid'"
//! description: An invoice has just been paid.
//! ```
//!
//! An object set, `object_sets/overdue_invoices.yaml`:
//!
//! ```yaml
//! name: overdue_invoices
//! of: invoice
//! where: "status = 'open' and issued_on < today() - 30d"
//! description: Open invoices issued more than thirty days ago.
//! ```
//!
//! An aging rule, `aging_rules/invoice_overdue.yaml`, holds the fields of
//! one `sys.aging_rule` record (`docs/seams.md` §`SystemVocabulary`). Its
//! `when` must parse, and when `on` is a type of the bundle, `field` is
//! one of its fields; `on` may also be a type the lake already holds, so
//! a bundle of rules alone retunes them. The lake checks the rest when the
//! rule is written:
//!
//! ```yaml
//! name: invoice_overdue
//! on: invoice
//! field: status
//! from: open
//! to: overdue
//! when: "issued_on < today() - 30d"
//! ```
//!
//! # Output
//!
//! The declarations come out in one order whatever order the files were
//! given in: value types, types, functions, verbs, triggers, object
//! sets, each lane sorted by name. Value types come before the types
//! that use them, and types before the verbs, triggers and sets that
//! name them. The aging rules come out beside them, sorted by name, as
//! the records `lakelands apply` writes after the declarations.

mod doc;
mod lower;
mod yaml;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::de::DeserializeOwned;

use lakelands_api::error::Naming;
use lakelands_api::tools::{NameProblem, name_problems};
use lakelands_api::write::Declaration;
use lakelands_api::{Patch, TypeName, VerbName};

use doc::{AgingRuleDoc, FunctionDoc, ObjectSetDoc, TriggerDoc, TypeDoc, ValueTypeDoc, VerbDoc};
use lower::{Known, Lower};

/// A bundle, loaded: its declarations in the order a lake takes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    /// Value types, types, functions, verbs, triggers, object sets, each
    /// lane sorted by name.
    pub declarations: Vec<Declaration>,
    /// Aging rules, as the fields of their `sys.aging_rule` records,
    /// sorted by name.
    pub aging_rules: Vec<Patch>,
}

/// One problem with a bundle, against the file it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleError {
    /// The file, relative to the bundle's directory, `/`-separated.
    pub file: String,
    /// What kind of problem it is.
    pub problem: Problem,
    /// What is wrong, for a person.
    pub message: String,
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for BundleError {}

impl BundleError {
    fn new(file: &str, problem: Problem, message: String) -> BundleError {
        BundleError {
            file: file.to_owned(),
            problem,
            message,
        }
    }
}

/// The kinds of problem a bundle can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Problem {
    /// The file or directory could not be read, or is not UTF-8.
    Unreadable,
    /// A YAML file outside the seven lanes, or anything in a directory
    /// inside a lane.
    UnknownLane,
    /// A file in a lane that is not YAML.
    NotYaml,
    /// The YAML does not parse, has a key the lane does not know, lacks
    /// one it needs, or has a value of the wrong shape (a risk other
    /// than `low`, `medium` or `high`).
    Malformed,
    /// A name that is empty or contains the NUL character.
    BadName,
    /// A name that is reserved: a `sys.` type, or a value type named like
    /// a storage kind.
    ReservedName,
    /// A name declared twice in the bundle.
    DuplicateName,
    /// Type or verb names that would make colliding, empty or overlong
    /// tool names.
    ToolName,
    /// A reference to a type or value type the bundle does not declare.
    UnknownReference,
    /// A field type that does not say what it must, or says what it may
    /// not.
    BadFieldType,
    /// A text expression that does not parse.
    BadExpression,
    /// Keys of one declaration that contradict each other, or one that is
    /// missing because of another.
    Contradiction,
}

/// The seven lanes of a bundle, in the order their declarations come out;
/// the aging rules last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Lane {
    ValueTypes,
    Types,
    Functions,
    Verbs,
    Triggers,
    ObjectSets,
    AgingRules,
}

impl Lane {
    const ALL: [Lane; 7] = [
        Lane::ValueTypes,
        Lane::Types,
        Lane::Functions,
        Lane::Verbs,
        Lane::Triggers,
        Lane::ObjectSets,
        Lane::AgingRules,
    ];

    fn dir(self) -> &'static str {
        match self {
            Lane::ValueTypes => "value_types",
            Lane::Types => "types",
            Lane::Functions => "functions",
            Lane::Verbs => "verbs",
            Lane::Triggers => "triggers",
            Lane::ObjectSets => "object_sets",
            Lane::AgingRules => "aging_rules",
        }
    }
}

fn is_yaml(path: &str) -> bool {
    path.ends_with(".yaml") || path.ends_with(".yml")
}

/// Which lane a path belongs to: `Ok(None)` for a path the bundle
/// ignores.
fn lane_of(path: &str) -> Result<Option<Lane>, (Problem, String)> {
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|s| s.starts_with('.')) {
        return Ok(None);
    }
    let lanes = || {
        Lane::ALL
            .iter()
            .map(|l| format!("`{}/`", l.dir()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match Lane::ALL.into_iter().find(|l| l.dir() == segments[0]) {
        Some(lane) if segments.len() == 2 && is_yaml(path) => Ok(Some(lane)),
        Some(lane) if segments.len() == 2 => Err((
            Problem::NotYaml,
            format!("`{}/` holds YAML files only (`.yaml`, `.yml`)", lane.dir()),
        )),
        Some(lane) => Err((
            Problem::UnknownLane,
            format!(
                "lanes are flat: a file of `{}/` sits directly in it, not in a directory below",
                lane.dir()
            ),
        )),
        None if is_yaml(path) => Err((
            Problem::UnknownLane,
            format!("not in a lane; a bundle's YAML files live in {}", lanes()),
        )),
        None => Ok(None),
    }
}

/// The documents of a bundle, each with its file, as read.
#[derive(Default)]
struct Docs<'a> {
    value_types: Vec<(&'a str, ValueTypeDoc)>,
    types: Vec<(&'a str, TypeDoc)>,
    functions: Vec<(&'a str, FunctionDoc)>,
    verbs: Vec<(&'a str, VerbDoc)>,
    triggers: Vec<(&'a str, TriggerDoc)>,
    object_sets: Vec<(&'a str, ObjectSetDoc)>,
    aging_rules: Vec<(&'a str, AgingRuleDoc)>,
}

/// Everything learnt while reading the files: the documents, the names
/// they declare, and the problems found so far.
#[derive(Default)]
struct Reading<'a> {
    docs: Docs<'a>,
    known: Known,
    /// Every declared name, with the file that declared it first.
    declared: BTreeMap<String, &'a str>,
    /// Every aging rule's name, with the file that wrote it first.
    rules: BTreeMap<String, &'a str>,
    errors: Vec<BundleError>,
}

impl<'a> Reading<'a> {
    fn fail(&mut self, file: &str, problem: Problem, message: String) {
        self.errors.push(BundleError::new(file, problem, message));
    }

    /// Record that `file` declares `name`; false when the bundle already
    /// declares it, and the document is then left out.
    fn declare(&mut self, file: &'a str, what: Naming, name: &str) -> bool {
        if let Some(fault) = crate::names::fault(what, name) {
            self.fail(file, Problem::BadName, fault.to_string());
        }
        if let Some(first) = self.declared.get(name) {
            let first = *first;
            self.fail(
                file,
                Problem::DuplicateName,
                format!(
                    "`{name}` is already declared in {first}; a name is declared once in a bundle"
                ),
            );
            return false;
        }
        self.declared.insert(name.to_owned(), file);
        true
    }

    fn read(&mut self, lane: Lane, file: &'a str, text: &str) {
        match lane {
            Lane::ValueTypes => {
                if let Some(d) = self.doc::<ValueTypeDoc>(file, text, lane) {
                    self.known
                        .value_types
                        .entry(d.name.clone())
                        .or_insert(Some(d.kind));
                    if self.declare(file, Naming::ValueType, &d.name) {
                        self.docs.value_types.push((file, d));
                    }
                }
            }
            Lane::Types => {
                if let Some(d) = self.doc::<TypeDoc>(file, text, lane) {
                    if d.name.starts_with("sys.") {
                        self.fail(
                            file,
                            Problem::ReservedName,
                            format!(
                                "`{}`: type names starting `sys.` are reserved for system types",
                                d.name
                            ),
                        );
                    }
                    self.known.types.insert(d.name.clone());
                    if self.declare(file, Naming::Type, &d.name) {
                        self.docs.types.push((file, d));
                    }
                }
            }
            Lane::Functions => {
                if let Some(d) = self.doc::<FunctionDoc>(file, text, lane)
                    && self.declare(file, Naming::Function, &d.name)
                {
                    self.docs.functions.push((file, d));
                }
            }
            Lane::Verbs => {
                if let Some(d) = self.doc::<VerbDoc>(file, text, lane)
                    && self.declare(file, Naming::Verb, &d.name)
                {
                    self.docs.verbs.push((file, d));
                }
            }
            Lane::Triggers => {
                if let Some(d) = self.doc::<TriggerDoc>(file, text, lane)
                    && self.declare(file, Naming::Trigger, &d.name)
                {
                    self.docs.triggers.push((file, d));
                }
            }
            Lane::ObjectSets => {
                if let Some(d) = self.doc::<ObjectSetDoc>(file, text, lane)
                    && self.declare(file, Naming::ObjectSet, &d.name)
                {
                    self.docs.object_sets.push((file, d));
                }
            }
            Lane::AgingRules => {
                if let Some(d) = self.doc::<AgingRuleDoc>(file, text, lane) {
                    if let Some(first) = self.rules.get(&d.name) {
                        let message = format!(
                            "aging rule `{}` is already written in {first}; a rule is written once in a bundle",
                            d.name
                        );
                        self.fail(file, Problem::DuplicateName, message);
                    } else {
                        self.rules.insert(d.name.clone(), file);
                        self.docs.aging_rules.push((file, d));
                    }
                }
            }
        }
    }

    /// Parse one document strictly. When it does not parse, its `name`
    /// is still recorded if it can be found, so that the files referring
    /// to it are not reported as well.
    fn doc<T: DeserializeOwned>(&mut self, file: &'a str, text: &str, lane: Lane) -> Option<T> {
        let doc = yaml::Document::read(text);
        match doc
            .as_ref()
            .map_err(Clone::clone)
            .and_then(yaml::Document::to::<T>)
        {
            Ok(d) => Some(d),
            Err(e) => {
                self.fail(file, Problem::Malformed, e);
                let name = doc.ok().and_then(|d| d.name());
                if let Some(name) = name {
                    self.declared.entry(name.clone()).or_insert(file);
                    match lane {
                        Lane::ValueTypes => {
                            self.known.value_types.entry(name).or_insert(None);
                        }
                        Lane::Types => {
                            self.known.types.insert(name);
                        }
                        _ => {}
                    }
                }
                None
            }
        }
    }
}

/// Turn a bundle's files into its declarations.
///
/// `files` are `(path, contents)` pairs, paths relative to the bundle's
/// directory and `/`-separated; their order does not matter. The
/// contents of a file the bundle ignores or refuses by its path are
/// never read. Pure: no I/O, no clock. Refused with every problem found,
/// sorted by file.
pub fn parse(files: &[(String, String)]) -> Result<Bundle, Vec<BundleError>> {
    let mut sorted: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.strip_prefix("./").unwrap_or(p), c.as_str()))
        .collect();
    sorted.sort();

    let mut r = Reading::default();
    for (path, text) in sorted {
        match lane_of(path) {
            Ok(Some(lane)) => r.read(lane, path, text),
            Ok(None) => {}
            Err((problem, message)) => r.fail(path, problem, message),
        }
    }

    let Reading {
        docs,
        known,
        mut errors,
        ..
    } = r;
    let (declarations, aging_rules) = lower_all(docs, &known, &mut errors);
    if errors.is_empty() {
        Ok(Bundle {
            declarations,
            aging_rules,
        })
    } else {
        errors.sort_by(|a, b| a.file.cmp(&b.file));
        Err(errors)
    }
}

/// Refuse a type whose declaration names fields or relations that do not
/// exist: its `key`, `valid_from` / `valid_to`, a relation's `via`, a
/// field's `depends_on`, a view's `group_by` (fields of the view's
/// source). A relation's `inverse` must not already name a relation of
/// its target. A `json` field may not carry the name of a type, because
/// `name.member` would then read as a path into the field and as the
/// field of a pattern step at once.
fn check_references(
    types: &[(&str, lakelands_api::ontology::TypeDecl)],
    errors: &mut Vec<BundleError>,
) {
    use lakelands_api::ontology::StorageKind;
    let by_name: BTreeMap<&str, &lakelands_api::ontology::TypeDecl> =
        types.iter().map(|(_, t)| (t.name.as_str(), t)).collect();
    for (file, t) in types {
        let mut fail = |problem: Problem, message: String| {
            errors.push(BundleError::new(file, problem, message));
        };
        let mut want = |at: String, field: &str, of: &lakelands_api::ontology::TypeDecl| {
            if !of.fields.iter().any(|d| d.name == field) {
                fail(
                    Problem::UnknownReference,
                    format!(
                        "{at} names `{field}`, which is not a field of `{}`",
                        of.name
                    ),
                );
            }
        };
        for k in &t.key {
            want("`key`".into(), k.as_str(), t);
        }
        for (at, f) in [("`valid_from`", &t.valid_from), ("`valid_to`", &t.valid_to)] {
            if let Some(f) = f {
                want(at.into(), f.as_str(), t);
            }
        }
        for f in &t.fields {
            for d in &f.depends_on {
                want(format!("field `{}`, `depends_on`", f.name), d.as_str(), t);
            }
        }
        for r in &t.relations {
            if let Some(via) = &r.via {
                want(format!("relation `{}`, `via`", r.name), via.as_str(), t);
            }
        }
        if let Some(v) = &t.view
            && let Some(source) = by_name.get(v.source.as_str())
        {
            for g in &v.group_by {
                want("view, `group_by`".into(), g.as_str(), source);
            }
        }
        for r in &t.relations {
            let (Some(inverse), Some(target)) = (&r.inverse, by_name.get(r.target.as_str())) else {
                continue;
            };
            if target.relations.iter().any(|tr| &tr.name == inverse) {
                fail(
                    Problem::Contradiction,
                    format!(
                        "relation `{}` names its inverse `{inverse}`, which `{}` already declares",
                        r.name, target.name
                    ),
                );
            }
        }
        for f in &t.fields {
            if f.ty.kind == StorageKind::Json && by_name.contains_key(f.name.as_str()) {
                fail(
                    Problem::Contradiction,
                    format!(
                        "`json` field `{}` has the name of a type, so `{}.…` would read two ways",
                        f.name, f.name
                    ),
                );
            }
        }
    }
}

/// Lower every document and check the tool names, in output order.
fn lower_all(
    docs: Docs<'_>,
    known: &Known,
    errors: &mut Vec<BundleError>,
) -> (Vec<Declaration>, Vec<Patch>) {
    fn each<'a, D, T>(
        docs: Vec<(&'a str, D)>,
        known: &Known,
        errors: &mut Vec<BundleError>,
        lower: fn(&mut Lower<'_>, D) -> T,
    ) -> Vec<(&'a str, T)> {
        docs.into_iter()
            .map(|(file, d)| (file, lower(&mut Lower::new(known, file, errors), d)))
            .collect()
    }
    fn sorted<T, K: Ord>(mut v: Vec<(&str, T)>, key: impl Fn(&T) -> K) -> Vec<(&str, T)> {
        v.sort_by_key(|a| key(&a.1));
        v
    }

    let value_types = sorted(
        each(docs.value_types, known, errors, lower::value_type),
        |d| d.name.clone(),
    );
    let types = sorted(each(docs.types, known, errors, lower::type_decl), |d| {
        d.name.clone()
    });
    let functions = sorted(each(docs.functions, known, errors, lower::function), |d| {
        d.name.clone()
    });
    let verbs = sorted(each(docs.verbs, known, errors, lower::verb), |d| {
        d.name.clone()
    });
    let triggers = sorted(each(docs.triggers, known, errors, lower::trigger), |d| {
        d.name.clone()
    });
    let object_sets = sorted(
        each(docs.object_sets, known, errors, lower::object_set),
        |d| d.name.clone(),
    );

    let tool_owners: Vec<(Owner, &str)> = types
        .iter()
        .map(|(f, d)| (Owner::Type(d.name.clone()), *f))
        .chain(verbs.iter().map(|(f, d)| (Owner::Verb(d.name.clone()), *f)))
        .collect();
    check_tool_names(&tool_owners, errors);
    check_references(&types, errors);
    let mut aging_rules: Vec<(&str, AgingRuleDoc)> = docs.aging_rules;
    aging_rules.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for (file, r) in &aging_rules {
        let on = types.iter().find(|(_, t)| t.name.as_str() == r.on);
        if let Some((_, t)) = on
            && !t.fields.iter().any(|f| f.name.as_str() == r.field)
        {
            let message = format!(
                "`field` names `{}`, which is not a field of `{}`",
                r.field, r.on
            );
            errors.push(BundleError::new(file, Problem::UnknownReference, message));
        }
    }
    let aging_rules = each(aging_rules, known, errors, lower::aging_rule)
        .into_iter()
        .map(|(_, p)| p)
        .collect();

    let mut out = Vec::new();
    out.extend(
        value_types
            .into_iter()
            .map(|(_, d)| Declaration::ValueType(d)),
    );
    out.extend(types.into_iter().map(|(_, d)| Declaration::Type(d)));
    out.extend(functions.into_iter().map(|(_, d)| Declaration::Function(d)));
    out.extend(verbs.into_iter().map(|(_, d)| Declaration::Verb(d)));
    out.extend(triggers.into_iter().map(|(_, d)| Declaration::Trigger(d)));
    out.extend(
        object_sets
            .into_iter()
            .map(|(_, d)| Declaration::ObjectSet(d)),
    );
    (out, aging_rules)
}

/// A declaration a lake derives tool names from.
enum Owner {
    Type(TypeName),
    Verb(VerbName),
}

fn problems_of(owners: &[&Owner]) -> Vec<NameProblem> {
    let mut types = Vec::new();
    let mut verbs = Vec::new();
    for o in owners {
        match o {
            Owner::Type(t) => types.push(t.clone()),
            Owner::Verb(v) => verbs.push(v.clone()),
        }
    }
    // The link doors' names are checked by the store when the bundle's
    // types are declared, with their relations.
    name_problems(&types, &verbs, &[])
}

/// Refuse type and verb names whose tool names would be empty, too long,
/// or collide with a generic tool (reported against the declaration's
/// own file), or collide with each other (reported against the later
/// file, naming the earlier one); one error per declaration or pair.
/// Tool names come only from the generator in [`lakelands_api::tools`];
/// this asks it about each declaration alone and about each pair.
fn check_tool_names(owners: &[(Owner, &str)], errors: &mut Vec<BundleError>) {
    let singles: Vec<Vec<NameProblem>> = owners.iter().map(|(o, _)| problems_of(&[o])).collect();
    for ((_, file), problems) in owners.iter().zip(&singles) {
        if !problems.is_empty() {
            errors.push(tool_error(file, problems, None));
        }
    }
    for (i, (a, file_a)) in owners.iter().enumerate() {
        for (j, (b, file_b)) in owners.iter().enumerate().skip(i + 1) {
            let pair: Vec<NameProblem> = problems_of(&[a, b])
                .into_iter()
                .filter(|p| !singles[i].contains(p) && !singles[j].contains(p))
                .collect();
            if !pair.is_empty() {
                errors.push(tool_error(file_b, &pair, Some(file_a)));
            }
        }
    }
}

fn tool_error(file: &str, problems: &[NameProblem], other: Option<&str>) -> BundleError {
    let quoted = |names: &[&String]| {
        let list = names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let s = if names.len() == 1 { "" } else { "s" };
        format!("tool name{s} {list}")
    };
    let mut collisions: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    let mut too_long = Vec::new();
    let mut parts = Vec::new();
    for p in problems {
        match p {
            NameProblem::Collision { name, from } => {
                let from: Vec<String> = from.iter().map(ToString::to_string).collect();
                collisions.entry(from.join(" and ")).or_default().push(name)
            }
            NameProblem::TooLong { name } => too_long.push(name),
            NameProblem::Empty { from } => {
                parts.push(format!("{from} leaves nothing to make a tool name of"))
            }
            NameProblem::Unknown(u) => parts.push(u.message().to_owned()),
        }
    }
    for (from, names) in collisions {
        parts.push(format!("{from} would make the same {}", quoted(&names)));
    }
    if !too_long.is_empty() {
        parts.push(format!(
            "{} would be longer than {} characters",
            quoted(&too_long),
            lakelands_api::tools::MAX_TOOL_NAME
        ));
    }
    let mut message = parts.join("; ");
    if let Some(other) = other {
        message.push_str(&format!(" (see {other})"));
    }
    BundleError::new(file, Problem::ToolName, message)
}

/// Read a bundle's directory and [`parse`] it. The only I/O in this
/// module: every file below `dir` is listed, entries whose name starts
/// with `.` are skipped, and the YAML files are read.
pub fn load_dir(dir: &Path) -> Result<Bundle, Vec<BundleError>> {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    walk(dir, "", &mut files, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    parse(&files)
}

fn walk(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<(String, String)>,
    errors: &mut Vec<BundleError>,
) {
    let unreadable = |file: String, e: &dyn fmt::Display| {
        BundleError::new(&file, Problem::Unreadable, e.to_string())
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push(unreadable(dir.display().to_string(), &e));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(unreadable(dir.display().to_string(), &e));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = format!("{prefix}{name}");
        let path = entry.path();
        if path.is_dir() {
            walk(&path, &format!("{rel}/"), files, errors);
        } else if is_yaml(&rel) {
            match std::fs::read(&path).map(String::from_utf8) {
                Ok(Ok(text)) => files.push((rel, text)),
                Ok(Err(_)) => errors.push(unreadable(rel, &"not UTF-8")),
                Err(e) => errors.push(unreadable(rel, &e)),
            }
        } else {
            // Its path alone decides: ignored, or refused as not YAML.
            files.push((rel, String::new()));
        }
    }
}
