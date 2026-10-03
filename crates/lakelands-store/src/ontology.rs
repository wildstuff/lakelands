//! The ontology in force at one position: every declaration, indexed for
//! the questions the store asks on every write and read.

use std::collections::BTreeMap;

use lakelands_api::engine::Declared;
use lakelands_api::ontology::{TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};
use lakelands_api::write::{Declaration, FunctionDecl, ObjectSetDecl};
use lakelands_api::{Error, Missing, TypeName, VerbName};

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
}

impl Ontology {
    /// The ontology a set of declarations in force makes.
    pub fn from_declared(declared: &[Declared]) -> Ontology {
        let mut o = Ontology::default();
        for d in declared {
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
            Declaration::Type(t) => put(&mut self.types, t.name.clone(), t, v, h),
            Declaration::ValueType(t) => put(&mut self.value_types, t.name.to_string(), t, v, h),
            Declaration::Verb(t) => put(&mut self.verbs, t.name.clone(), t, v, h),
            Declaration::Trigger(t) => put(&mut self.triggers, t.name.to_string(), t, v, h),
            Declaration::ObjectSet(t) => put(&mut self.object_sets, t.name.clone(), t, v, h),
            Declaration::Function(t) => put(&mut self.functions, t.name.clone(), t, v, h),
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
