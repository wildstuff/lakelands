//! The ontology in force at one position: every declaration, indexed for
//! the questions the store asks on every write and read.

use std::collections::BTreeMap;

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

/// The declarations in force, by kind and name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ontology {
    /// Types.
    pub types: BTreeMap<TypeName, Entry<TypeDecl>>,
    /// Value types.
    pub value_types: BTreeMap<String, Entry<ValueTypeDecl>>,
    /// Verbs.
    pub verbs: BTreeMap<VerbName, Entry<VerbDecl>>,
    /// Triggers, by name.
    pub triggers: BTreeMap<String, Entry<TriggerDecl>>,
    /// Object sets, by name.
    pub object_sets: BTreeMap<String, Entry<ObjectSetDecl>>,
    /// Functions, by name.
    pub functions: BTreeMap<String, Entry<FunctionDecl>>,
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
        fn put<K: Ord, T>(
            m: &mut BTreeMap<K, Entry<T>>,
            name: K,
            decl: T,
            v: u32,
            h: Option<String>,
        ) {
            m.insert(
                name,
                Entry {
                    decl,
                    version: v,
                    schema_hash: h,
                },
            );
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
            Declaration::Function(t) => {
                put(&mut self.functions, t.name.clone(), t, v, h);
                for (name, mut e) in std::mem::take(&mut self.types) {
                    let listed = self.listed.get(&name).into_iter().flatten();
                    computed(&mut e.decl)
                        .zip(listed)
                        .for_each(|(f, l)| f.depends_on.clone_from(l));
                    self.complete(&mut e.decl);
                    self.types.insert(name, e);
                }
            }
        }
    }

    /// A shape with every computed field's `depends_on` holding each field
    /// it is computed from: what its expression reads, or its function's
    /// inputs. A field is read no further than what it depends on, so a
    /// computed field must list what it hands out, whether or not its
    /// declaration did (024, extended).
    pub fn complete(&self, t: &mut TypeDecl) {
        // An embedding names what it is made from in `embed.from` already.
        for f in computed(t).filter(|f| f.embed.is_none()) {
            let mut reads = crate::backed::computed_reads(self, f).unwrap_or_default();
            reads.retain(|r| !f.depends_on.contains(r));
            f.depends_on.extend(reads);
        }
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

    /// Every declaration, as the engine stores them.
    pub fn declared(&self) -> Vec<Declared> {
        fn each<T: Clone>(
            m: &BTreeMap<impl Ord, Entry<T>>,
            wrap: fn(T) -> Declaration,
            out: &mut Vec<Declared>,
        ) {
            for e in m.values() {
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
