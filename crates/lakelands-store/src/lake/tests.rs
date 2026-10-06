use lakelands_api::Version;
use lakelands_api::engine::EngineOpener;
use lakelands_api::write::Rekey;
use serde_json::json;

use super::*;

fn ty(v: serde_json::Value) -> TypeDecl {
    serde_json::from_value(v).unwrap()
}

/// `doc`, its first field named `first` and its `n` of `kind`.
fn doc(first: &str, kind: &str) -> TypeDecl {
    ty(
        json!({"name": "doc", "fields": [{"name": first, "type": {"kind": "text"}},
                                         {"name": "n", "type": {"kind": kind}}]}),
    )
}

fn record(ty: &str, key: &str, fields: serde_json::Value, at: u64) -> StoredRecord {
    let fields = serde_json::from_value(fields).unwrap();
    StoredRecord {
        fields,
        version: Version(3),
        position: Position(at),
        retired: false,
        ..StoredRecord::ended(ty.into(), key.into(), Position(at))
    }
}

/// A step at `at` that migrates `doc` from `before` to `to`, renaming a
/// `title` to `name`, and moves `moves` of its keys, in a lake that also
/// holds `note`, which refers to a `doc`.
fn step(at: u64, before: TypeDecl, to: TypeDecl, moves: &[(&str, &str)]) -> Migrated {
    let note = ty(json!({"name": "note", "fields": [
        {"name": "about", "type": {"kind": "reference", "target": "doc"}}]}));
    let mut ontology = Ontology::default();
    for t in [to.clone(), note] {
        ontology.put(Declaration::Type(t), 1, None);
    }
    let titled = before.fields.iter().any(|f| f.name.as_str() == "title");
    let migration = Migration {
        to,
        renames: (titled.then(|| ("title".into(), "name".into())).into_iter()).collect(),
        relation_renames: Vec::new(),
    };
    let rekeyed: Vec<Rekey> = (moves.iter())
        .map(|(from, to)| Rekey {
            ty: "doc".into(),
            from: (*from).into(),
            to: (*to).into(),
        })
        .collect();
    Migrated::new(&ontology, Position(at), before, &migration, &rekeyed)
}

#[test]
fn a_step_converts_moves_and_rewrites_and_leaves_the_rest() {
    let s = step(
        7,
        doc("title", "text"),
        doc("name", "number"),
        &[("a", "b")],
    );
    let mut a = record("doc", "a", json!({"title": "T", "n": "12"}), 2);
    a.restricted = true;
    let after = s.apply(a).unwrap();
    assert_eq!(
        (after.key.as_str(), json!(after.fields), after.position),
        ("b", json!({"name": "T", "n": 12}), Position(7)),
        "a record of the migrated type is converted and moved, at the step's position"
    );
    assert_eq!(
        (after.version, after.retired, after.restricted),
        (Version(3), false, true),
        "its version and its flags are not touched"
    );
    let about = |key: &str| record("note", "n", json!({"about": key}), 2);
    assert_eq!(
        s.apply(about("a")).unwrap(),
        StoredRecord {
            position: Position(7),
            ..about("b")
        },
        "a reference to a moved key names the new one, at the step's position"
    );
    assert_eq!(
        s.apply(about("u")).unwrap(),
        about("u"),
        "another is as it was"
    );
    let refused = s.apply(record("doc", "x", json!({"n": "abc"}), 2));
    let why = refused.unwrap_err().to_string();
    assert!(why.contains("`doc` `x`") || why.contains("abc"), "{why}");
}

#[test]
fn a_key_is_traced_back_through_the_steps() {
    let same = || doc("name", "text");
    let chain = step(2, same(), same(), &[("a", "b"), ("b", "c")]);
    let (doc, k) = (TypeName::from("doc"), |key: &str| RecordKey::from(key));
    let source = |key: &str| chain.source(&doc, &k(key)).map(|(_, k)| k.to_string());
    assert_eq!(
        [source("c"), source("b"), source("a"), source("u")],
        [Some("b".into()), Some("a".into()), None, Some("u".into())],
        "a new key comes from its old one; an old key no record arrives at is ended"
    );
    assert_eq!(
        (chain.moved(&doc, &k("a")), chain.moved(&doc, &k("u"))),
        (&k("b"), &k("u"))
    );
    assert_eq!(chain.moved(&"note".into(), &k("a")), &k("a"));

    // Two steps over a view that holds `a` and `v`: `a` moves to `b`, then
    // `b` to `c` while `v` arrives at `b`.
    let engine = lakelands_engine_mem::MemoryOpener::new()
        .open(&Location::memory("steps"), OpenMode::Create)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    for key in ["a", "v"] {
        p.put_record(&record("doc", key, json!({"name": key}), 1))
            .unwrap();
    }
    p.commit(Position(1)).unwrap();
    let view = engine.view(None).unwrap();
    let staged = Staged {
        steps: vec![
            Step::Migrated(Box::new(step(2, same(), same(), &[("a", "b")]))),
            Step::Migrated(Box::new(step(3, same(), same(), &[("b", "c"), ("v", "b")]))),
        ],
        ..Staged::default()
    };
    let read = |key: &str| {
        let r = staged.record(view.as_ref(), &doc, &k(key)).unwrap()?;
        Some((r.fields.get("name").cloned(), r.position.0))
    };
    assert_eq!(read("c"), Some((Some(json!("a")), 3)), "from `a`, by both");
    assert_eq!(
        read("b"),
        Some((Some(json!("v")), 3)),
        "from `v`, by the second"
    );
    assert_eq!(read("a"), Some((None, 2)), "ended at the first step");
    assert_eq!(read("v"), Some((None, 3)), "ended at the second");
    assert_eq!(read("u"), None, "never written");
}

#[test]
fn a_walk_reads_page_by_page_and_passes_over_ended_keys() {
    // `n` records, the sixth an ended key and every seventh retired.
    let walked = |n: usize, retired: bool| {
        let engine = lakelands_engine_mem::MemoryOpener::new()
            .open(&Location::memory("walk"), OpenMode::Create)
            .unwrap();
        let p = engine.projection();
        p.begin().unwrap();
        for i in 0..n {
            let mut r = record("t", &format!("k{i:04}"), json!({}), 1);
            r.retired = i % 7 == 0;
            let ended = StoredRecord::ended(r.ty.clone(), r.key.clone(), Position(1));
            p.put_record(if i == 5 { &ended } else { &r }).unwrap();
        }
        p.commit(Position(1)).unwrap();
        let view = engine.view(None).unwrap();
        let ty = TypeName::from("t");
        let mut walk = Walk::new(view.as_ref(), &ty, retired);
        let keys: Vec<RecordKey> = walk.by_ref().map(|r| r.key).collect();
        walk.failed().unwrap();
        assert!(
            keys.is_sorted() && !keys.contains(&"k0005".into()),
            "{keys:?}"
        );
        keys.len()
    };
    let live = |n: usize| n - n.div_ceil(7) - 1;
    for n in [1_024, 1_025, 2_500] {
        assert_eq!(
            walked(n, true),
            n - 1,
            "{n} records, the retired ones included"
        );
        assert_eq!(walked(n, false), live(n), "{n} records, the live ones");
    }
}

fn declaration(d: serde_json::Value) -> Declaration {
    serde_json::from_value(d).unwrap()
}

/// An ontology that holds `doc`, whose `twice` is computed by `f` and
/// rounded as `money`, whose `ghost` names a function that is not declared
/// and whose `odd` a value type that is not; and a type, a function and a
/// value type that `doc` does not name.
fn computing_doc() -> Ontology {
    let f = |name: &str, expr: &str| {
        json!({"declare": "function", "name": name, "backing": "expr", "expr": expr,
               "inputs": [{"name": "n", "type": {"kind": "decimal"}}]})
    };
    let by = |name: &str, function: &str, value_type: &str| {
        json!({"name": name, "type": {"kind": "decimal", "value_type": value_type},
               "authority": "derived", "function": function})
    };
    let money =
        |name: &str| json!({"declare": "value_type", "name": name, "kind": "decimal", "scale": 1});
    let doc = json!({"declare": "type", "name": "doc", "field_meta": true, "fields": [
        {"name": "n", "type": {"kind": "decimal"}},
        by("twice", "f", "money"), by("ghost", "g", "money"), by("odd", "f", "absent")]});
    let memo = json!({"declare": "type", "name": "memo", "fields": [by("m", "h", "other")]});
    let mut o = Ontology::default();
    let all = [
        f("f", "n * 2"),
        f("h", "n"),
        money("money"),
        money("other"),
        doc,
        memo,
    ];
    for d in all {
        o.put(declaration(d), 1, None);
    }
    o
}

/// A `declared` event at `at`, as the store seals one.
fn declared_at(at: u64) -> Event {
    let body = EventBody::Declared {
        declaration: declaration(json!({"declare": "function", "name": "f", "backing": "expr"})),
        schema_hash: None,
        version: 2,
    };
    let draft = Draft::system(Timestamp(5), body);
    log::seal_at(1, &"l".into(), Position(at), &Digest::zero(), 1, draft).0
}

#[test]
fn the_entries_of_a_recomputation_mirror_the_writers_ontology() {
    let mut o = computing_doc();
    let entries = o.computing(&["doc".into(), "gone".into()]);
    let names = |e: &Ontology| {
        let types: Vec<_> = e.types.keys().map(TypeName::to_string).collect();
        let functions: Vec<_> = e.functions.keys().cloned().collect();
        (
            types,
            functions,
            e.value_types.keys().cloned().collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        names(&entries),
        (
            vec!["doc".to_owned()],
            vec!["f".to_owned()],
            vec!["money".to_owned()]
        ),
        "what `doc`'s fields name and the ontology holds, and no name it does not hold"
    );
    // A field whose function is not declared is not computed, under the
    // entries as under the ontology they were taken from.
    let engine = lakelands_engine_mem::MemoryOpener::new()
        .open(&Location::memory("entries"), OpenMode::Create)
        .unwrap();
    let view = engine.view(None).unwrap();
    let event = declared_at(7);
    let r = record("doc", "a", json!({"n": "1.26"}), 2);
    let under = |o: &Ontology| {
        let mut outputs = Outputs {
            view: view.as_ref(),
            staged: None,
            before: None,
            due: None,
            waiting: false,
        };
        record::rederive(o, &r, &event, &BTreeMap::new(), &mut outputs).unwrap()
    };
    let computed = under(&entries).expect("the record changes");
    assert_eq!(Some(&computed), under(&o).as_ref());
    assert_eq!(
        (json!(computed.fields), computed.position),
        (
            json!({"n": "1.26", "twice": "2.5", "odd": "2.52"}),
            Position(7)
        ),
        "`twice` at its value type's scale, `odd` at none, `ghost` not computed"
    );
    // Each entry is the ontology's own, not a copy; a function declared
    // again leaves the step its version, and a type it does not change its
    // entry.
    let shared = |o: &Ontology| {
        let f = Arc::ptr_eq(&o.functions["f"], &entries.functions["f"]);
        let doc = Arc::ptr_eq(
            &o.types[&TypeName::from("doc")],
            &entries.types[&TypeName::from("doc")],
        );
        (
            f,
            doc,
            Arc::ptr_eq(&o.value_types["money"], &entries.value_types["money"]),
        )
    };
    assert_eq!(shared(&o), (true, true, true));
    let again = json!({"declare": "function", "name": "f", "backing": "expr", "expr": "n * 3",
                       "inputs": [{"name": "n", "type": {"kind": "decimal"}}]});
    o.put(declaration(again), 2, None);
    assert_eq!(shared(&o), (false, true, true));
    assert_eq!(
        (entries.functions["f"].version, o.functions["f"].version),
        (1, 2)
    );
}
