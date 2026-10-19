//! The ontology in force at one position: every declaration, indexed for
//! the questions the store asks on every write and read.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lakelands_api::engine::{Declared, ProjectionView};
use lakelands_api::ontology::{FieldDecl, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};
use lakelands_api::tools::{self, MAX_TOOL_NAME, NameSource};
use lakelands_api::write::{
    Declaration, DeclarationKind, FunctionDecl, ObjectSetDecl, SettingDecl,
};
use lakelands_api::{Error, FieldName, Missing, Position, TypeName, VerbName};
use lakelands_core::engines::declaration_key;

/// A type's fields and its relations' link fields: all that may be computed.
fn computed(t: &mut TypeDecl) -> impl Iterator<Item = &mut FieldDecl> {
    let links = t.relations.iter_mut().flat_map(|r| r.fields.iter_mut());
    t.fields.iter_mut().chain(links)
}

/// One declaration with its version and, for a type, its schema hash.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry<T> {
    /// The declaration.
    pub decl: T,
    /// Its version.
    pub version: u32,
    /// For a type: its schema hash.
    pub schema_hash: Option<String>,
    /// The position it has been in force since.
    pub since: Position,
}

/// The declarations in force, by kind and name. What a record's derived
/// fields are computed from (a type, a function, a value type) is held
/// behind a pointer: a recomputation kept as a step shares the entries it
/// reads ([`Ontology::computing`]), and one replaced here stays its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ontology {
    /// Types.
    pub types: BTreeMap<TypeName, Arc<Entry<TypeDecl>>>,
    /// Value types.
    pub value_types: BTreeMap<String, Arc<Entry<ValueTypeDecl>>>,
    /// Verbs.
    pub verbs: BTreeMap<VerbName, Entry<VerbDecl>>,
    /// Triggers, by name.
    pub triggers: BTreeMap<String, Entry<TriggerDecl>>,
    /// Object sets, by name.
    pub object_sets: BTreeMap<String, Entry<ObjectSetDecl>>,
    /// Functions, by name.
    pub functions: BTreeMap<String, Arc<Entry<FunctionDecl>>>,
    /// A host's own settings, by key (063, extended).
    pub settings: BTreeMap<String, Entry<SettingDecl>>,
    /// Each declaration withdrawn and not declared again, by its identity
    /// ([`declaration_key`]): the version and the position of its
    /// withdrawal, which a later declaration of the name continues.
    withdrawn: BTreeMap<String, (u32, Position)>,
    /// Each type's `depends_on` lists as they were declared, field by
    /// field: the completed lists are made again from them.
    listed: BTreeMap<TypeName, Vec<Vec<FieldName>>>,
    /// The writer's tool names, as these declarations make them; none in
    /// a reader's ontology.
    names: Option<Arc<ToolNames>>,
}

/// The tool names a lake's declarations make, each with what makes it,
/// the names each type makes, and those that are a problem: what the
/// writer's ontology keeps, so that a declaration's names are checked
/// against the lake's alone ([`ToolNames::clear`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolNames {
    names: BTreeMap<String, Vec<NameSource>>,
    by: BTreeMap<TypeName, Vec<String>>,
    problems: BTreeSet<String>,
}

impl ToolNames {
    fn add(&mut self, name: &str, from: NameSource) {
        let held = self.names.entry(name.to_owned()).or_default();
        if !held.contains(&from) {
            held.push(from);
        }
        self.judge(name);
    }

    /// Whether `name` is a problem: empty, too long, or made twice.
    fn judge(&mut self, name: &str) {
        let made = self.names.get(name).map_or(0, Vec::len);
        let bad = made > 1 || made == 1 && (name.is_empty() || name.len() > MAX_TOOL_NAME);
        match bad {
            true => self.problems.insert(name.to_owned()),
            false => self.problems.remove(name),
        };
    }

    /// The verb `verb` out of force: the name it made gives way.
    fn take_verb(&mut self, verb: &VerbName) {
        let name = tools::tool_base(verb.as_str());
        if let Some(held) = self.names.get_mut(&name) {
            held.retain(|s| !matches!(s, NameSource::Verb { verb: v } if v == verb));
            if held.is_empty() {
                self.names.remove(&name);
            }
        }
        self.judge(&name);
    }

    /// `t` in force: the names it made give way to those it makes.
    fn put_type(&mut self, t: &TypeDecl) {
        let of = |s: &NameSource| match s {
            NameSource::Type { ty } | NameSource::Link { ty, .. } => ty == &t.name,
            _ => false,
        };
        for name in self.by.remove(&t.name).unwrap_or_default() {
            if let Some(held) = self.names.get_mut(&name) {
                held.retain(|s| !of(s));
                if held.is_empty() {
                    self.names.remove(&name);
                }
            }
            self.judge(&name);
        }
        let links = crate::declare::link_relations(t);
        for (name, from) in tools::all_names(std::slice::from_ref(&t.name), &[], &links) {
            for s in from.into_iter().filter(of) {
                self.by
                    .entry(t.name.clone())
                    .or_default()
                    .push(name.clone());
                self.add(&name, s);
            }
        }
    }

    /// Whether the lake `o` with `d` declared has, for certain, no problem
    /// among its tool names: none where `o` holds none, as a lake does (no
    /// door leaves one, 027, extended), and none at a name `d` makes. `o`
    /// is the writer's ontology or made from it, so it holds no name this
    /// lacks; a source it does not hold is not counted.
    pub fn clear(&self, o: &Ontology, d: &Declaration) -> bool {
        let (types, verbs, links) = match d {
            Declaration::Type(t) => {
                let brought = lakelands_core::links::link_types_of(t).into_iter();
                let types: Vec<_> = brought.map(|l| l.name).chain([t.name.clone()]).collect();
                (types, Vec::new(), crate::declare::link_relations(t))
            }
            Declaration::Verb(v) => (Vec::new(), vec![v.name.clone()], Vec::new()),
            _ => Default::default(),
        };
        let holds = |s: &&NameSource| match s {
            NameSource::Generic { .. } => true,
            NameSource::Type { ty } => o.types.contains_key(ty),
            NameSource::Verb { verb } => o.verbs.contains_key(verb),
            NameSource::Link { ty, relation } => {
                let held = |e: &Arc<Entry<TypeDecl>>| {
                    let mut rs = e.decl.relations.iter();
                    rs.any(|r| &r.name == relation && r.carried_by_links())
                };
                o.types.get(ty).is_some_and(held)
            }
            NameSource::Unknown(_) => true,
        };
        let made = tools::all_names(&types, &verbs, &links);
        let clear = |(name, mine): (String, Vec<NameSource>)| {
            let mut from: Vec<&NameSource> = self
                .names
                .get(&name)
                .into_iter()
                .flatten()
                .filter(holds)
                .collect();
            for s in &mine {
                if !from.contains(&s) {
                    from.push(s);
                }
            }
            from.len() == 1 && !name.is_empty() && name.len() <= MAX_TOOL_NAME
        };
        self.problems.is_empty() && made.into_iter().all(clear)
    }
}

impl Ontology {
    /// This ontology with its tool names kept ([`ToolNames`]), as the
    /// writer's is: each declaration put changes them.
    pub fn named(mut self) -> Ontology {
        let mut names = ToolNames::default();
        for (name, from) in tools::all_names(&[], &[], &[]) {
            from.into_iter().for_each(|s| names.add(&name, s));
        }
        self.types.values().for_each(|e| names.put_type(&e.decl));
        for v in self.verbs.keys() {
            names.add(
                &tools::tool_base(v.as_str()),
                NameSource::Verb { verb: v.clone() },
            );
        }
        self.names = Some(Arc::new(names));
        self
    }

    /// The tool names it keeps, if it is the writer's or made from it.
    pub fn tool_names(&self) -> Option<&ToolNames> {
        self.names.as_deref()
    }

    /// The declarations in force at a view. Every shape the store shows or
    /// judges comes from here, so that each computed field lists what it
    /// is computed from (024, extended).
    pub fn at(view: &dyn ProjectionView) -> Result<Ontology, Error> {
        Ok(Ontology::from_declared(&view.declarations()?))
    }

    /// The ontology a set of declarations in force makes.
    pub fn from_declared(declared: &[Declared]) -> Ontology {
        let mut o = Ontology::default();
        // The functions first, so that each type is completed once.
        let function = |d: &&Declared| matches!(d.declaration, Declaration::Function(_));
        let rest = declared.iter().filter(|d| !function(d));
        for d in declared.iter().filter(function).chain(rest) {
            o.put_at(
                d.declaration.clone(),
                d.version,
                d.schema_hash.clone(),
                d.since,
            );
        }
        o
    }

    /// Put one declaration in force, replacing an earlier version.
    pub fn put(&mut self, declaration: Declaration, version: u32, schema_hash: Option<String>) {
        self.put_at(declaration, version, schema_hash, Position::GENESIS)
    }

    /// [`Ontology::put`], in force since `since`.
    pub fn put_at(
        &mut self,
        declaration: Declaration,
        version: u32,
        schema_hash: Option<String>,
        since: Position,
    ) {
        fn put<K: Ord, T, E: From<Entry<T>>>(
            m: &mut BTreeMap<K, E>,
            name: K,
            decl: T,
            v: u32,
            h: Option<String>,
            since: Position,
        ) {
            let entry = Entry {
                decl,
                version: v,
                schema_hash: h,
                since,
            };
            m.insert(name, entry.into());
        }
        let (v, h, s) = (version, schema_hash, since);
        if !matches!(declaration, Declaration::Withdraw(_)) {
            self.withdrawn.remove(&declaration_key(&declaration));
        }
        match declaration {
            Declaration::Type(mut t) => {
                let listed = computed(&mut t).map(|f| f.depends_on.clone()).collect();
                self.listed.insert(t.name.clone(), listed);
                self.complete(&mut t);
                if let Some(names) = &mut self.names {
                    Arc::make_mut(names).put_type(&t);
                }
                put(&mut self.types, t.name.clone(), t, v, h, s);
            }
            Declaration::ValueType(t) => put(&mut self.value_types, t.name.to_string(), t, v, h, s),
            Declaration::Verb(t) => {
                if let Some(names) = &mut self.names {
                    let verb = t.name.clone();
                    Arc::make_mut(names)
                        .add(&tools::tool_base(verb.as_str()), NameSource::Verb { verb });
                }
                put(&mut self.verbs, t.name.clone(), t, v, h, s)
            }
            Declaration::Trigger(t) => put(&mut self.triggers, t.name.to_string(), t, v, h, s),
            Declaration::ObjectSet(t) => put(&mut self.object_sets, t.name.clone(), t, v, h, s),
            // A function's inputs are what its fields are computed from. Each
            // type is completed again from its lists as declared: a function
            // declared again with fewer inputs holds no field by one it lost.
            // An entry a step shares is completed on a copy, and kept when
            // the copy comes out as it was.
            Declaration::Function(t) => {
                put(&mut self.functions, t.name.clone(), t, v, h, s);
                for (name, held) in std::mem::take(&mut self.types) {
                    let (mut e, shared) = match Arc::try_unwrap(held) {
                        Ok(own) => (own, None),
                        Err(held) => (Entry::clone(&held), Some(held)),
                    };
                    let listed = self.listed.get(&name).into_iter().flatten();
                    computed(&mut e.decl)
                        .zip(listed)
                        .for_each(|(f, l)| f.depends_on.clone_from(l));
                    self.complete(&mut e.decl);
                    let kept = shared.filter(|held| **held == e);
                    self.types.insert(name, kept.unwrap_or_else(|| Arc::new(e)));
                }
            }
            Declaration::Setting(t) => put(&mut self.settings, t.key.clone(), t, v, h, s),
            Declaration::Withdraw(t) => {
                self.take_out(t.kind, &t.name);
                let key = declaration_key(&Declaration::Withdraw(t));
                self.withdrawn.insert(key, (v, s));
            }
            _ => {}
        }
    }

    /// A shape with every computed field's `depends_on` holding each field
    /// it is computed from: what its expression reads, or its function's
    /// inputs. A field is read no further than what it depends on, so a
    /// computed field must list what it hands out, whether or not its
    /// declaration did (024, extended). An expression whose reads cannot
    /// be told reads every other field beside it, of its type or of its
    /// relation: such a field is closed, never open.
    pub fn complete(&self, t: &mut TypeDecl) {
        let links = t.relations.iter_mut().map(|r| &mut r.fields);
        for fields in std::iter::once(&mut t.fields).chain(links) {
            let all: Vec<FieldName> = fields.iter().map(|f| f.name.clone()).collect();
            // An embedding names what it is made from in `embed.from` already.
            for f in fields.iter_mut().filter(|f| f.embed.is_none()) {
                let mut reads = match crate::backed::computed_reads(self, f) {
                    Some(reads) => reads,
                    None if f.derived.is_some() => {
                        all.iter().filter(|n| **n != f.name).cloned().collect()
                    }
                    None => continue,
                };
                reads.retain(|r| !f.depends_on.contains(r));
                f.depends_on.extend(reads);
            }
        }
    }

    /// The entries a recomputation of `types` reads, and nothing else:
    /// theirs, and those of the functions and the value types their fields
    /// name, each shared with this ontology as it stands now. A name this
    /// ontology does not hold is one the answer does not hold: a field
    /// whose function is not declared yet is not computed, here as there.
    pub fn computing(&self, types: &[TypeName]) -> Ontology {
        let mut o = Ontology::default();
        for (name, t) in types.iter().filter_map(|n| self.types.get_key_value(n)) {
            for f in &t.decl.fields {
                let function = f.function.as_ref();
                if let Some((n, e)) = function.and_then(|n| self.functions.get_key_value(n)) {
                    o.functions.insert(n.clone(), e.clone());
                }
                let value_type = f.ty.value_type.as_ref().map(|n| n.as_str());
                if let Some((n, e)) = value_type.and_then(|n| self.value_types.get_key_value(n)) {
                    o.value_types.insert(n.clone(), e.clone());
                }
            }
            o.types.insert(name.clone(), t.clone());
        }
        o
    }

    /// The declaration of a type, or `NotFound`.
    pub fn ty(&self, name: &TypeName) -> Result<&TypeDecl, Error> {
        self.types
            .get(name)
            .map(|e| &e.decl)
            .ok_or_else(|| Error::NotFound {
                missing: Missing::Type { name: name.clone() },
            })
    }

    /// A type in force as it was declared: each computed field with the
    /// `depends_on` its declaration listed, not the completed one.
    pub fn as_declared(&self, name: &TypeName) -> Result<TypeDecl, Error> {
        let mut t = self.ty(name)?.clone();
        let listed = self.listed.get(name).into_iter().flatten();
        computed(&mut t)
            .zip(listed)
            .for_each(|(f, l)| f.depends_on.clone_from(l));
        Ok(t)
    }

    /// The declaration of a verb, or `NotFound`.
    pub fn verb(&self, name: &VerbName) -> Result<&VerbDecl, Error> {
        self.verbs
            .get(name)
            .map(|e| &e.decl)
            .ok_or_else(|| Error::NotFound {
                missing: Missing::Verb { name: name.clone() },
            })
    }

    /// The value types, as a list.
    pub fn value_type_list(&self) -> Vec<ValueTypeDecl> {
        self.value_types.values().map(|e| e.decl.clone()).collect()
    }

    /// The declaration in force of `d`'s kind and name, as
    /// [`Ontology::declared`] lists it.
    pub fn held(&self, d: &Declaration) -> Option<Declaration> {
        fn of<T: Clone>(
            e: Option<&impl Borrow<Entry<T>>>,
            wrap: fn(T) -> Declaration,
        ) -> Option<Declaration> {
            e.map(|e| wrap(e.borrow().decl.clone()))
        }
        match d {
            Declaration::Type(t) => of(self.types.get(&t.name), Declaration::Type),
            Declaration::ValueType(v) => of(
                self.value_types.get(v.name.as_str()),
                Declaration::ValueType,
            ),
            Declaration::Verb(v) => of(self.verbs.get(&v.name), Declaration::Verb),
            Declaration::Trigger(t) => of(self.triggers.get(t.name.as_str()), Declaration::Trigger),
            Declaration::ObjectSet(s) => of(self.object_sets.get(&s.name), Declaration::ObjectSet),
            Declaration::Function(f) => of(self.functions.get(&f.name), Declaration::Function),
            Declaration::Setting(s) => of(self.settings.get(&s.key), Declaration::Setting),
            Declaration::Withdraw(w) => self.held_named(w.kind, &w.name),
            _ => None,
        }
    }

    /// The trigger, verb, object set or function `name` out of force, a
    /// verb with the tool name it made. What reads it refuses its
    /// withdrawal, so no type is completed again.
    pub fn take_out(&mut self, kind: DeclarationKind, name: &str) {
        match kind {
            DeclarationKind::Trigger => drop(self.triggers.remove(name)),
            DeclarationKind::Verb => {
                let verb = VerbName::from(name);
                if self.verbs.remove(&verb).is_some()
                    && let Some(names) = &mut self.names
                {
                    Arc::make_mut(names).take_verb(&verb);
                }
            }
            DeclarationKind::ObjectSet => drop(self.object_sets.remove(name)),
            DeclarationKind::Function => drop(self.functions.remove(name)),
            _ => {}
        }
    }

    /// The declaration in force of `kind` and `name`, if any.
    pub fn held_named(&self, kind: DeclarationKind, name: &str) -> Option<Declaration> {
        fn of<T: Clone>(e: Option<&impl Borrow<Entry<T>>>) -> Option<T> {
            e.map(|e| e.borrow().decl.clone())
        }
        match kind {
            DeclarationKind::Type => of(self.types.get(name)).map(Declaration::Type),
            DeclarationKind::ValueType => {
                of(self.value_types.get(name)).map(Declaration::ValueType)
            }
            DeclarationKind::Verb => of(self.verbs.get(name)).map(Declaration::Verb),
            DeclarationKind::Trigger => of(self.triggers.get(name)).map(Declaration::Trigger),
            DeclarationKind::ObjectSet => {
                of(self.object_sets.get(name)).map(Declaration::ObjectSet)
            }
            DeclarationKind::Function => of(self.functions.get(name)).map(Declaration::Function),
            DeclarationKind::Setting => of(self.settings.get(name)).map(Declaration::Setting),
            _ => None,
        }
    }

    /// The version and since of the entry of `declaration`'s kind and name;
    /// of a withdrawal, the entry it withdraws or, once withdrawn, its own.
    pub fn entry_of(&self, d: &Declaration) -> Option<(u32, Position)> {
        fn of<T>(e: Option<&impl Borrow<Entry<T>>>) -> Option<(u32, Position)> {
            e.map(|e| (e.borrow().version, e.borrow().since))
        }
        match d {
            Declaration::Type(t) => of(self.types.get(&t.name)),
            Declaration::ValueType(v) => of(self.value_types.get(v.name.as_str())),
            Declaration::Verb(v) => of(self.verbs.get(&v.name)),
            Declaration::Trigger(t) => of(self.triggers.get(t.name.as_str())),
            Declaration::ObjectSet(s) => of(self.object_sets.get(&s.name)),
            Declaration::Function(f) => of(self.functions.get(&f.name)),
            Declaration::Setting(s) => of(self.settings.get(&s.key)),
            Declaration::Withdraw(w) => {
                let held = self.held_named(w.kind, &w.name);
                let held = held.and_then(|h| self.entry_of(&h));
                held.or_else(|| self.withdrawn.get(&declaration_key(d)).copied())
            }
            _ => None,
        }
    }

    /// Whether `declaration` is the one in force, in the form the lake
    /// keeps: a type with its computed fields completed, and keyed as its
    /// records are, so a grouped view with its `key` written is the view
    /// without it. Such a declaration appends nothing and its version
    /// stays (027, extended). The one entry of its kind and name is looked
    /// up, whatever the lake declares.
    pub fn in_force(&self, declaration: &Declaration) -> bool {
        fn is<T: PartialEq>(held: Option<&impl Borrow<Entry<T>>>, d: &T) -> bool {
            held.is_some_and(|e| &e.borrow().decl == d)
        }
        match declaration {
            Declaration::Type(t) => {
                let mut t = t.clone();
                self.complete(&mut t);
                let keyed = |mut t: TypeDecl| {
                    t.key = t.key_fields().to_vec();
                    t
                };
                (self.types.get(&t.name)).is_some_and(|e| keyed(e.decl.clone()) == keyed(t))
            }
            Declaration::ValueType(v) => is(self.value_types.get(v.name.as_str()), v),
            Declaration::Verb(v) => is(self.verbs.get(&v.name), v),
            Declaration::Trigger(t) => is(self.triggers.get(t.name.as_str()), t),
            Declaration::ObjectSet(s) => is(self.object_sets.get(&s.name), s),
            Declaration::Function(f) => is(self.functions.get(&f.name), f),
            Declaration::Setting(s) => is(self.settings.get(&s.key), s),
            _ => false,
        }
    }

    /// The version a declaration of this name and kind would take next.
    pub fn next_version(&self, declaration: &Declaration) -> u32 {
        let withdrawn = || self.withdrawn.get(&declaration_key(declaration)).copied();
        let held = self.entry_of(declaration).or_else(withdrawn);
        held.map_or(1, |(v, _)| v + 1)
    }

    /// The object set `name`, if one is declared.
    pub fn set(&self, name: &str) -> Option<&ObjectSetDecl> {
        self.object_sets.get(name).map(|e| &e.decl)
    }

    /// Each type's name and version, which moves on every change of it.
    pub fn versions(&self) -> Vec<(TypeName, u32)> {
        self.types
            .iter()
            .map(|(n, e)| (n.clone(), e.version))
            .collect()
    }

    /// Every declaration, as the engine stores them.
    pub fn declared(&self) -> Vec<Declared> {
        fn each<T: Clone>(
            m: &BTreeMap<impl Ord, impl Borrow<Entry<T>>>,
            wrap: fn(T) -> Declaration,
            out: &mut Vec<Declared>,
        ) {
            for e in m.values().map(Borrow::borrow) {
                out.push(Declared {
                    declaration: wrap(e.decl.clone()),
                    version: e.version,
                    schema_hash: e.schema_hash.clone(),
                    since: e.since,
                });
            }
        }
        let mut out = Vec::new();
        each(&self.value_types, Declaration::ValueType, &mut out);
        each(&self.types, Declaration::Type, &mut out);
        each(&self.functions, Declaration::Function, &mut out);
        each(&self.verbs, Declaration::Verb, &mut out);
        each(&self.triggers, Declaration::Trigger, &mut out);
        each(&self.object_sets, Declaration::ObjectSet, &mut out);
        each(&self.settings, Declaration::Setting, &mut out);
        out
    }
}

impl lakelands_core::sight::InForce for Ontology {
    fn ty(&self, name: &TypeName) -> Option<&TypeDecl> {
        self.types.get(name).map(|e| &e.decl)
    }

    fn types(&self) -> Vec<&TypeDecl> {
        self.types.values().map(|e| &e.decl).collect()
    }

    fn declared(&self) -> std::borrow::Cow<'_, [Declared]> {
        self.declared().into()
    }
}
