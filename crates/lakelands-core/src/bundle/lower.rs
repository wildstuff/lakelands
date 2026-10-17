//! From the documents an author wrote to the declarations a lake takes.
//!
//! Each function here lowers one document of one file, reporting every
//! problem it finds against that file and carrying on, so that a bundle
//! with several mistakes is answered with all of them at once. The value
//! it returns when it found a problem is never used: the bundle as a
//! whole is refused.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::Naming;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{
    Assignment, EmbedDecl, FieldDecl, FieldType, Measure, RelationDecl, StorageKind, TriggerDecl,
    TypeDecl, ValueTypeDecl, VerbDecl, VerbEffect, ViewDecl, ViewParam, ViewSource,
};
use lakelands_api::write::{
    AnnotationFault, Declaration, FunctionDecl, ObjectSetDecl, SettingDecl,
};
use lakelands_api::{
    FieldName, ParamName, Patch, RelationName, TriggerName, TypeName, ValueTypeName, VerbName,
};

use super::doc::{
    AgingRuleDoc, Cond, FieldDoc, FieldTypeDoc, FunctionDoc, Named, ObjectSetDoc, SettingDoc,
    StrOr, TriggerDoc, TypeDoc, TypeSpec, ValueTypeDoc, VerbDoc,
};
use super::{BundleError, Problem};

/// The backing a function has when its file names none.
pub(super) const EXPR_BACKING: &str = "expr";

/// What the whole bundle declares, known before any document is lowered
/// so that references between files resolve in any order. A name whose
/// file did not parse is still known, so that its users are not
/// reported a second time.
#[derive(Debug, Default)]
pub(super) struct Known {
    /// Value types, with their storage kind when their file parsed.
    pub value_types: BTreeMap<String, Option<StorageKind>>,
    pub types: BTreeSet<String>,
}

fn kind_named(name: &str) -> Option<StorageKind> {
    StorageKind::ALL.into_iter().find(|k| k.to_string() == name)
}

/// The lowering of one file.
pub(super) struct Lower<'a> {
    known: &'a Known,
    file: &'a str,
    errors: &'a mut Vec<BundleError>,
}

impl<'a> Lower<'a> {
    /// Lower documents of `file`, reporting into `errors`.
    pub(super) fn new(known: &'a Known, file: &'a str, errors: &'a mut Vec<BundleError>) -> Self {
        Lower {
            known,
            file,
            errors,
        }
    }

    fn fail(&mut self, problem: Problem, message: String) {
        self.errors
            .push(BundleError::new(self.file, problem, message));
    }

    fn name(&mut self, what: Naming, name: &str) {
        if let Some(fault) = crate::names::fault(what, name) {
            self.fail(Problem::BadName, fault.to_string());
        }
    }

    fn type_ref(&mut self, at: &str, name: &str) -> TypeName {
        // `sys.` types belong to the lake, not to a bundle; the store
        // checks them when the declaration arrives.
        if !name.starts_with("sys.") && !self.known.types.contains(name) {
            self.fail(
                Problem::UnknownReference,
                format!("{at}: type `{name}` is not declared in the bundle"),
            );
        }
        TypeName::new(name)
    }

    fn cond(&mut self, at: &str, cond: Cond) -> Condition {
        if let Condition::Text(text) = &cond.0
            && let Err(e) = crate::expr::parse(text)
        {
            self.fail(
                Problem::BadExpression,
                format!("{at}: `{text}` does not parse: {e}"),
            );
        }
        cond.0
    }

    fn opt_cond(&mut self, at: &str, cond: Option<Cond>) -> Option<Condition> {
        cond.map(|c| self.cond(at, c))
    }

    /// A view's or a merged source's measures; `at` leads each one's place.
    fn measures(&mut self, at: &str, measures: Named<Cond>) -> Vec<Measure> {
        measures
            .0
            .into_iter()
            .map(|(name, expr)| {
                self.name(Naming::Measure, &name);
                let expr = self.cond(&format!("{at}measure `{name}`"), expr);
                Measure {
                    name: FieldName::new(name),
                    expr,
                }
            })
            .collect()
    }

    fn names<T: From<String>>(&mut self, what: Naming, names: Vec<String>) -> Vec<T> {
        names
            .into_iter()
            .map(|n| {
                self.name(what.clone(), &n);
                T::from(n)
            })
            .collect()
    }

    fn fields(&mut self, owner: &str, fields: Named<FieldDoc>) -> Vec<FieldDecl> {
        fields
            .0
            .into_iter()
            .map(|(name, doc)| self.field(owner, name, doc))
            .collect()
    }

    fn field(&mut self, owner: &str, name: String, doc: FieldDoc) -> FieldDecl {
        self.name(Naming::Field, &name);
        let at = format!("{owner} `{name}`");
        let body = match doc {
            StrOr::Str(short) => {
                let ty = self.field_type(&at, StrOr::Str(short));
                return FieldDecl {
                    name: FieldName::new(name),
                    ty,
                    ..FieldDecl::default()
                };
            }
            StrOr::Map(body) => body,
        };
        let ty = self.field_type(&at, body.ty);
        let derived = self.opt_cond(&format!("{at}, `derived`"), body.derived);
        let depends_on = self.names(Naming::Field, body.depends_on.0);
        let embed = body.embed.map(|e| EmbedDecl {
            from: self.names(Naming::Field, e.from.0),
            model: e.model,
        });
        FieldDecl {
            annotations: body.annotations,
            name: FieldName::new(name),
            ty,
            required: body.required,
            authority: body.authority,
            derived,
            function: body.function,
            computed_at: body.computed_at,
            depends_on,
            on_input_change: body.on_input_change,
            initial: body.initial,
            sensitive: body.sensitive,
            searchable: body.searchable,
            erasable: body.erasable,
            description: body.description,
            label: body.label,
            embed,
        }
    }

    /// Report a declaration's annotations that cannot be declared: a key
    /// in a reserved namespace, or one the contract refuses. And a number
    /// in it the log would not write, for a declaration of every kind.
    fn annotated(&mut self, d: &Declaration) {
        match d.annotation_fault() {
            Some(AnnotationFault::Reserved { said, .. }) => {
                self.fail(Problem::ReservedName, said.to_string());
            }
            Some(AnnotationFault::Invalid { said }) => {
                self.fail(Problem::Malformed, said.to_string())
            }
            None => {}
        }
        if let Some(said) = d.integer_fault() {
            self.fail(Problem::Malformed, said.to_string());
        }
    }

    /// A field's type, from the short string or the object form. Both
    /// end in the same checks: a reference names its target, a vector
    /// its length, and a value type exists and is declared over the kind
    /// the field says.
    fn field_type(&mut self, at: &str, spec: TypeSpec) -> FieldType {
        let placeholder = FieldType::kind(StorageKind::Json);
        let doc = match spec {
            StrOr::Map(doc) => doc,
            StrOr::Str(short) => match self.short_type(at, &short) {
                Some(doc) => doc,
                None => return placeholder,
            },
        };
        let mut ty = FieldType::kind(doc.kind);
        if let Some(vt) = doc.value_type {
            match self.known.value_types.get(&vt) {
                None => self.fail(
                    Problem::UnknownReference,
                    format!("{at}: value type `{vt}` is not declared in the bundle"),
                ),
                Some(Some(kind)) if *kind != doc.kind => self.fail(
                    Problem::BadFieldType,
                    format!(
                        "{at}: value type `{vt}` is declared over `{}`, the field says `{}`",
                        kind, doc.kind
                    ),
                ),
                Some(_) => {}
            }
            ty.value_type = Some(ValueTypeName::new(vt));
        }
        match (doc.kind, doc.target) {
            (StorageKind::Reference, Some(t)) => ty.target = Some(self.type_ref(at, &t)),
            (StorageKind::Reference, None) => self.fail(
                Problem::BadFieldType,
                format!("{at}: a reference names the type it points to, as `reference(<type>)`"),
            ),
            (_, Some(_)) => self.fail(
                Problem::BadFieldType,
                format!("{at}: only a reference has a `target`"),
            ),
            (_, None) => {}
        }
        match (doc.kind, doc.dimensions) {
            (StorageKind::Vector, Some(n)) if n > 0 => ty.dimensions = Some(n),
            (StorageKind::Vector, _) => self.fail(
                Problem::BadFieldType,
                format!("{at}: a vector gives its length, as `vector(<n>)` with n above 0"),
            ),
            (_, Some(_)) => self.fail(
                Problem::BadFieldType,
                format!("{at}: only a vector has `dimensions`"),
            ),
            (_, None) => {}
        }
        ty
    }

    /// Read `<name>` or `<name>(<argument>)`, where the name is a storage
    /// kind or a value type and the argument is a reference's target or
    /// a vector's length.
    fn short_type(&mut self, at: &str, short: &str) -> Option<FieldTypeDoc> {
        let (head, arg) = match split_call(short.trim()) {
            Some(parts) => parts,
            None => {
                self.fail(
                    Problem::BadFieldType,
                    format!("{at}: `{short}` is not a field type; write `<kind>`, `<value type>` or `<kind>(<argument>)`"),
                );
                return None;
            }
        };
        let (kind, value_type) = match kind_named(head) {
            Some(kind) => (kind, None),
            None => match self.known.value_types.get(head) {
                Some(Some(kind)) => (*kind, Some(head.to_owned())),
                // Its file did not parse; that file's error says why.
                Some(None) => return None,
                None => {
                    self.fail(
                        Problem::UnknownReference,
                        format!("{at}: `{head}` is neither a storage kind nor a value type declared in the bundle"),
                    );
                    return None;
                }
            },
        };
        let mut doc = FieldTypeDoc {
            kind,
            value_type,
            target: None,
            dimensions: None,
        };
        match (kind, arg) {
            (StorageKind::Reference, arg) => doc.target = arg.map(str::to_owned),
            (StorageKind::Vector, None) => {}
            (StorageKind::Vector, Some(n)) => match n.parse::<u32>() {
                Ok(n) => doc.dimensions = Some(n),
                Err(_) => {
                    self.fail(
                        Problem::BadFieldType,
                        format!("{at}: a vector's length is a whole number, not `{n}`"),
                    );
                    return None;
                }
            },
            (_, Some(_)) => {
                self.fail(
                    Problem::BadFieldType,
                    format!("{at}: `{head}` takes no argument; only a reference or a vector does"),
                );
                return None;
            }
            (_, None) => {}
        }
        Some(doc)
    }
}

/// `name` or `name(arg)`, both trimmed and non-empty.
fn split_call(s: &str) -> Option<(&str, Option<&str>)> {
    match s.find('(') {
        None if !s.is_empty() && !s.contains(')') => Some((s, None)),
        None => None,
        Some(open) => {
            let head = s[..open].trim();
            let arg = s[open + 1..].strip_suffix(')')?.trim();
            (!head.is_empty() && !arg.is_empty() && !arg.contains(['(', ')']))
                .then_some((head, Some(arg)))
        }
    }
}

pub(super) fn value_type(l: &mut Lower<'_>, doc: ValueTypeDoc) -> ValueTypeDecl {
    if kind_named(&doc.name).is_some() {
        l.fail(
            Problem::ReservedName,
            format!(
                "`{}` is a storage kind; a value type needs a name of its own",
                doc.name
            ),
        );
    }
    if !doc.one_of.is_empty() && doc.kind != StorageKind::Text {
        l.fail(
            Problem::Contradiction,
            format!(
                "`one_of` lists text values; value type `{}` is `{}`",
                doc.name, doc.kind
            ),
        );
    }
    if doc.scale.is_some() && doc.kind != StorageKind::Decimal {
        l.fail(
            Problem::Contradiction,
            format!(
                "`scale` keeps decimals at a number of digits; value type `{}` is `{}`",
                doc.name, doc.kind
            ),
        );
    }
    if let Some(format) = &doc.format
        && crate::value::ValueFormat::parse(format).is_none()
    {
        let known: Vec<&str> = crate::value::ValueFormat::ALL
            .iter()
            .map(|f| f.name())
            .collect();
        l.fail(
            Problem::Contradiction,
            format!(
                "value type `{}` names the format `{format}`, which is not one of: {}",
                doc.name,
                known.join(", ")
            ),
        );
    }
    let constraint = l.opt_cond("`constraint`", doc.constraint);
    let v = ValueTypeDecl {
        name: ValueTypeName::new(doc.name),
        kind: doc.kind,
        constraint,
        one_of: doc.one_of,
        format: doc.format,
        scale: doc.scale,
        description: doc.description,
        annotations: doc.annotations,
    };
    l.annotated(&Declaration::ValueType(v.clone()));
    v
}

/// A type, with its fields, relations and view.
pub(super) fn type_decl(l: &mut Lower<'_>, doc: TypeDoc) -> TypeDecl {
    let key = l.names(Naming::Field, doc.key.0);
    let fields = l.fields("field", doc.fields);
    let relations = doc
        .relations
        .0
        .into_iter()
        .map(|(name, r)| {
            l.name(Naming::Relation, &name);
            let at = format!("relation `{name}`");
            let target = l.type_ref(&at, &r.target);
            let fields = l.fields(&format!("{at}, field"), r.fields);
            let derived = r.derived.map(|mut d| {
                let filter = d.filter.take().map(super::doc::Cond);
                d.filter = l.opt_cond(&format!("{at}, where"), filter);
                d
            });
            RelationDecl {
                annotations: r.annotations,
                derived,
                name: RelationName::new(name),
                target,
                cardinality: r.cardinality,
                inverse: r.inverse.map(RelationName::new),
                via: r.via.map(FieldName::new),
                fields,
                authority: r.authority,
                description: r.description,
            }
        })
        .collect();
    let view = doc.view.map(|v| {
        let source = l.type_ref("view", &v.source);
        let params = v
            .params
            .0
            .into_iter()
            .filter_map(|(name, kind)| {
                l.name(Naming::Parameter, &name);
                let Some(kind) = kind_named(&kind) else {
                    l.fail(
                        Problem::BadFieldType,
                        format!("view, parameter `{name}`: `{kind}` is not a storage kind"),
                    );
                    return None;
                };
                Some(ViewParam {
                    name: ParamName::new(name),
                    kind,
                })
            })
            .collect();
        let filter = l.opt_cond("view, `where`", v.filter);
        let group_by = l.names(Naming::Field, v.group_by.0);
        let measures = l.measures("", v.measures);
        let merge = v
            .merge
            .into_iter()
            .map(|m| {
                let at = format!("view, merged source `{}`", m.source);
                ViewSource {
                    source: l.type_ref(&at, &m.source),
                    r#where: l.opt_cond(&format!("{at}, `where`"), m.filter),
                    group_by: l.names(Naming::Field, m.group_by.0),
                    measures: l.measures(&format!("{at}, "), m.measures),
                }
            })
            .collect();
        ViewDecl {
            source,
            r#where: filter,
            group_by,
            measures,
            params,
            merge,
        }
    });
    let t = TypeDecl {
        constraints: Default::default(),
        annotations: doc.annotations,
        name: TypeName::new(doc.name),
        key,
        fields,
        relations,
        view: view.map(Box::new),
        valid_from: doc.valid_from.map(FieldName::new),
        valid_to: doc.valid_to.map(FieldName::new),
        field_meta: doc.field_meta,
        insert_only: doc.insert_only,
        language: doc.language,
        description: doc.description,
        label: doc.label,
    };
    l.annotated(&Declaration::Type(t.clone()));
    t
}

/// A verb: a write verb when it has `set`, an external one when it has
/// `executor`, never both.
pub(super) fn verb(l: &mut Lower<'_>, doc: VerbDoc) -> VerbDecl {
    let on = l.type_ref("`on`", &doc.on);
    let payload = l.fields("payload field", doc.payload);
    let requires = l.opt_cond("`requires`", doc.requires);
    let effect = match (doc.set, doc.executor) {
        (Some(set), None) => VerbEffect::Write {
            set: set
                .0
                .into_iter()
                .map(|(field, value)| {
                    l.name(Naming::Field, &field);
                    // A link is keyed by its ends; a verb sets its own fields.
                    if crate::links::is_link_type(&on)
                        && [crate::links::FROM, crate::links::TO].contains(&field.as_str())
                    {
                        let message = format!(
                            "`set` of `{field}`: a link's `{field}` is one of its ends, which \
                             key it; a verb on a link type sets the link's own fields"
                        );
                        l.fail(Problem::Contradiction, message);
                    }
                    let value = l.cond(&format!("`set` of `{field}`"), value);
                    Assignment {
                        field: FieldName::new(field),
                        value,
                    }
                })
                .collect(),
        },
        (None, Some(executor)) => {
            l.name(Naming::Executor, &executor);
            VerbEffect::External { executor }
        }
        (set, _) => {
            let message = if set.is_some() {
                "a verb either sets fields of its record (`set`) or runs an executor (`executor`), not both"
            } else {
                "a verb says what it does: `set` for the fields it writes, or `executor` for what it runs outside the lake"
            };
            l.fail(Problem::Contradiction, message.to_owned());
            VerbEffect::Write { set: Vec::new() }
        }
    };
    let v = VerbDecl {
        name: VerbName::new(doc.name),
        on,
        scope: doc.scope,
        payload,
        risk: doc.risk,
        requires,
        effect,
        description: doc.description,
    };
    l.annotated(&Declaration::Verb(v.clone()));
    v
}

pub(super) fn trigger(l: &mut Lower<'_>, doc: TriggerDoc) -> TriggerDecl {
    let on = l.type_ref("`on`", &doc.on);
    let when = l.opt_cond("`when`", doc.when);
    let t = TriggerDecl {
        max_depth: doc.max_depth,
        name: TriggerName::new(doc.name),
        on,
        when,
        description: doc.description,
    };
    l.annotated(&Declaration::Trigger(t.clone()));
    t
}

pub(super) fn object_set(l: &mut Lower<'_>, doc: ObjectSetDoc) -> ObjectSetDecl {
    let of = l.type_ref("`of`", &doc.of);
    let filter = l.cond("`where`", doc.filter);
    let s = ObjectSetDecl {
        name: doc.name,
        of,
        filter,
        description: doc.description,
    };
    l.annotated(&Declaration::ObjectSet(s.clone()));
    s
}

/// An aging rule: the fields of its `sys.aging_rule` record. The type it
/// ages may be the lake's rather than the bundle's, so a bundle of rules
/// alone can retune them; the lake checks `on`, `field`, `from` and `to`
/// when the rule is written.
pub(super) fn aging_rule(l: &mut Lower<'_>, doc: AgingRuleDoc) -> Patch {
    l.name(Naming::AgingRule, &doc.name);
    l.cond("`when`", Cond(Condition::Text(doc.when.clone())));
    let mut p: Patch = [
        ("name", doc.name.into()),
        ("on", doc.on.into()),
        ("field", doc.field.into()),
        ("from", doc.from),
        ("to", doc.to),
        ("when", doc.when.into()),
    ]
    .into_iter()
    .map(|(k, v)| (FieldName::from(k), v))
    .collect();
    if let Some(d) = doc.description {
        p.insert("description".into(), d.into());
    }
    p
}

/// A host's setting, and the fields of the `sys.setting` record that gives
/// it its first value, where the file gives one.
pub(super) fn setting(l: &mut Lower<'_>, doc: SettingDoc) -> (SettingDecl, Option<Patch>) {
    let ty = l.field_type("`type`", doc.ty);
    let value = doc.value.map(|v| {
        let key = serde_json::Value::String(doc.key.clone());
        [("key".into(), key), ("value".into(), v)].into()
    });
    let decl = SettingDecl {
        key: doc.key,
        ty,
        unit: doc.unit,
        description: doc.description,
        group: doc.group,
    };
    (decl, value)
}

/// A function. An `expr` function (the default backing) carries its
/// expression; any other backing is configured through `config` and
/// carries none, so a function never has two definitions.
pub(super) fn function(l: &mut Lower<'_>, doc: FunctionDoc) -> FunctionDecl {
    let backing = doc.backing.unwrap_or_else(|| EXPR_BACKING.to_owned());
    l.name(Naming::Backing, &backing);
    let inputs = l.fields("input", doc.inputs);
    let expr = l.opt_cond("`expr`", doc.expr);
    match (backing == EXPR_BACKING, expr.is_some()) {
        (true, false) => l.fail(
            Problem::Contradiction,
            "an `expr` function needs `expr`, the expression it computes".to_owned(),
        ),
        (false, true) => l.fail(
            Problem::Contradiction,
            format!(
                "`expr` belongs to the `expr` backing; a `{backing}` function is configured through `config`"
            ),
        ),
        _ => {}
    }
    let f = FunctionDecl {
        name: doc.name,
        backing,
        inputs,
        expr,
        config: doc.config.unwrap_or(serde_json::Value::Null),
        description: doc.description,
        risk: doc.risk,
    };
    l.annotated(&Declaration::Function(f.clone()));
    f
}
