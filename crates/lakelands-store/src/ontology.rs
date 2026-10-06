//! The ontology in force at one position: every declaration, indexed for
//! the questions the store asks on every write and read.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::engine::{Declared, ProjectionView};
use lakelands_api::ontology::{FieldDecl, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};
use lakelands_api::write::{Declaration, FunctionDecl, ObjectSetDecl};
use lakelands_api::{Error, FieldName, Missing, TypeName, VerbName};

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
    /// Each type's `depends_on` lists as they were declared, field by
    /// field: the completed lists are made again from them.
    listed: BTreeMap<TypeName, Vec<Vec<FieldName>>>,
}

impl Ontology {
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
            o.put(d.declaration.clone(), d.version, d.schema_hash.clone());
        }
        o
    }

    /// Put one declaration in force, replacing an earlier version.
    pub fn put(&mut self, declaration: Declaration, version: u32, schema_hash: Option<String>) {
        fn put<K: Ord, T, E: From<Entry<T>>>(
            m: &mut BTreeMap<K, E>,
            name: K,
            decl: T,
            v: u32,
            h: Option<String>,
        ) {
            let entry = Entry {
                decl,
                version: v,
                schema_hash: h,
            };
            m.insert(name, entry.into());
        }
        let (v, h) = (version, schema_hash);
        match declaration {
            Declaration::Type(mut t) => {
                let listed = computed(&mut t).map(|f| f.depends_on.clone()).collect();
                self.listed.insert(t.name.clone(), listed);
                self.complete(&mut t);
                put(&mut self.types, t.name.clone(), t, v, h);
            }
            Declaration::ValueType(t) => put(&mut self.value_types, t.name.to_string(), t, v, h),
            Declaration::Verb(t) => put(&mut self.verbs, t.name.clone(), t, v, h),
            Declaration::Trigger(t) => put(&mut self.triggers, t.name.to_string(), t, v, h),
            Declaration::ObjectSet(t) => put(&mut self.object_sets, t.name.clone(), t, v, h),
            // A function's inputs are what its fields are computed from. Each
            // type is completed again from its lists as declared: a function
            // declared again with fewer inputs holds no field by one it lost.
            // An entry a step shares is completed on a copy, and kept when
            // the copy comes out as it was.
            Declaration::Function(t) => {
                put(&mut self.functions, t.name.clone(), t, v, h);
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

    /// Whether `declaration` is the one in force, in the form the lake
    /// keeps: a type with its computed fields completed. Such a declaration
    /// appends nothing and its version stays (027, extended). The one entry
    /// of its kind and name is looked up, whatever the lake declares.
    pub fn in_force(&self, declaration: &Declaration) -> bool {
        fn is<T: PartialEq>(held: Option<&impl Borrow<Entry<T>>>, d: &T) -> bool {
            held.is_some_and(|e| &e.borrow().decl == d)
        }
        match declaration {
            Declaration::Type(t) => {
                let mut t = t.clone();
                self.complete(&mut t);
                is(self.types.get(&t.name), &t)
            }
            Declaration::ValueType(v) => is(self.value_types.get(v.name.as_str()), v),
            Declaration::Verb(v) => is(self.verbs.get(&v.name), v),
            Declaration::Trigger(t) => is(self.triggers.get(t.name.as_str()), t),
            Declaration::ObjectSet(s) => is(self.object_sets.get(&s.name), s),
            Declaration::Function(f) => is(self.functions.get(&f.name), f),
        }
    }

    /// The version a declaration of this name and kind would take next.
    pub fn next_version(&self, declaration: &Declaration) -> u32 {
        let current = match declaration {
            Declaration::Type(t) => self.types.get(&t.name).map(|e| e.version),
            Declaration::ValueType(v) => self.value_types.get(v.name.as_str()).map(|e| e.version),
            Declaration::Verb(v) => self.verbs.get(&v.name).map(|e| e.version),
            Declaration::Trigger(t) => self.triggers.get(t.name.as_str()).map(|e| e.version),
            Declaration::ObjectSet(s) => self.object_sets.get(&s.name).map(|e| e.version),
            Declaration::Function(f) => self.functions.get(&f.name).map(|e| e.version),
        };
        current.map_or(1, |v| v + 1)
    }

    /// The object set `name`, if one is declared.
    pub fn set(&self, name: &str) -> Option<&ObjectSetDecl> {
        self.object_sets.get(name).map(|e| &e.decl)
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
                    since: lakelands_api::Position::GENESIS,
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
        out
    }
}
