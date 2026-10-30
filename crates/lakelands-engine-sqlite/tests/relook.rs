//! A commit decides the exact-text index again only for the types its
//! declarations and `lookup_index.<type>` settings move, from the head's
//! declarations the handle kept at its last commit (lakelands#1098): after
//! every commit `lookup_fields` holds what `engines::looked_up` decides
//! over every declaration at the head, the whole decision, whether the
//! handle kept them, rolled a transaction back, was opened again or
//! another handle committed meanwhile.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::Position;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_core::engines;
use lakelands_engine_sqlite::SqliteOpener;
use rusqlite::Connection;
use serde_json::{Value, json};

fn decl(v: Value) -> Declaration {
    serde_json::from_value(v).unwrap()
}

fn ty(name: &str, fields: &[&str]) -> Declaration {
    let fields: Vec<_> = (fields.iter())
        .map(|f| json!({"name": f, "type": {"kind": "text"}}))
        .collect();
    decl(json!({"declare": "type", "name": name, "fields": fields}))
}

fn set(of: &str, filter: &str) -> Declaration {
    decl(json!({"declare": "object_set", "name": "s", "of": of, "where": filter}))
}

fn record(ty: &str, key: &str, fields: Value, at: u64) -> StoredRecord {
    stored_record(ty, key, serde_json::from_value(fields).unwrap(), at)
}

/// The steps, each one transaction: its declarations, and the
/// `lookup_index.w` setting's value it puts, if any.
fn steps() -> Vec<(&'static str, Vec<Declaration>, Option<Value>)> {
    let derived = decl(json!({
        "declare": "type", "name": "p", "fields": [{"name": "k", "type": {"kind": "text"}}],
        "relations": [{"name": "hits", "target": "u", "cardinality": "many_to_many",
                       "derived": {"match": [{"from": "k", "to": "c"}]}}]}));
    let view = decl(json!({
        "declare": "type", "name": "v", "key": ["d"],
        "fields": [{"name": "d", "type": {"kind": "text"}}],
        "view": {"source": "u", "where": "d = 'y'", "group_by": ["d"], "measures": [],
                 "merge": [{"source": "t", "where": "b = 'z'", "group_by": ["b"],
                            "measures": []}]}}));
    let mut keyed = ty("t", &["a", "b", "g"]);
    if let Declaration::Type(t) = &mut keyed {
        t.key = vec!["g".into()];
    }
    vec![
        (
            "the lake",
            vec![
                ty("t", &["a", "b"]),
                ty("u", &["c", "d", "h"]),
                ty("w", &["e", "f"]),
            ],
            None,
        ),
        ("a set's type", vec![set("t", "a = 'x'")], None),
        ("records only", vec![], None),
        ("a view's source and its merge's", vec![view], None),
        ("a derived relation's target", vec![derived], None),
        (
            "a type another handle's set reads, declared again",
            vec![ty("u", &["c", "d", "h"])],
            None,
        ),
        ("a setting", vec![], Some(json!(["f"]))),
        ("the set over another type", vec![set("w", "e = 'x'")], None),
        ("a type declared again with a key", vec![keyed], None),
        ("the setting again", vec![], Some(json!([]))),
    ]
}

/// What the whole decision keeps over `decls`, the setting's value `asked`.
fn whole(decls: &[Declaration], asked: &Option<Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for d in decls {
        let Declaration::Type(t) = d else { continue };
        let value = asked.as_ref().filter(|_| t.name.as_str() == "w");
        let fields = engines::looked_up(&t.name, decls, value);
        out.extend(
            fields
                .into_iter()
                .flatten()
                .map(|f| (t.name.to_string(), f.to_string())),
        );
    }
    out.sort();
    out
}

fn open(dir: &std::path::Path, mode: OpenMode) -> Box<dyn LakeEngine> {
    SqliteOpener
        .open(&Location::sqlite("relook", dir), mode)
        .unwrap()
}

#[test]
fn a_commit_keeps_the_exact_text_index_the_whole_decision_keeps() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("relook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut engine = open(&dir, OpenMode::Create);
    let (mut decls, mut asked, mut at) = (Vec::<Declaration>::new(), None, 0);
    for (i, (what, put, setting)) in steps().into_iter().enumerate() {
        // A transaction rolled back first: what it put moves nothing.
        let p = engine.projection();
        p.begin().unwrap();
        p.put_declaration(&set("u", "c = 'x'"), 9, None, Position(at + 1))
            .unwrap();
        p.rollback();
        // Another handle commits between two of this one's: what this
        // one kept no longer holds.
        if i == 5 {
            let other = open(&dir, OpenMode::Write);
            let p = other.projection();
            let d =
                decl(json!({"declare": "object_set", "name": "s2", "of": "u", "where": "h = 'q'"}));
            p.begin().unwrap();
            at += 1;
            p.put_declaration(&d, 1, None, Position(at)).unwrap();
            p.commit(Position(at)).unwrap();
            decls.push(d);
        }
        // And the handle opened again before the last.
        if i == 9 {
            drop(engine);
            engine = open(&dir, OpenMode::Write);
        }
        let p = engine.projection();
        p.begin().unwrap();
        for d in &put {
            at += 1;
            p.put_declaration(d, 1, None, Position(at)).unwrap();
            let key = engines::declaration_key(d);
            decls.retain(|x| engines::declaration_key(x) != key);
            decls.push(d.clone());
        }
        at += 1;
        p.put_record(&record("t", "k", json!({"a": "x", "b": "z"}), at))
            .unwrap();
        if let Some(value) = &setting {
            let key = "lookup_index.w";
            let fields = json!({"key": key, "value": value});
            let setting = record(lakelands_api::seam::SETTING_TYPE, key, fields, at);
            p.put_record(&setting).unwrap();
            asked = Some(value.clone());
        }
        p.commit(Position(at)).unwrap();
        let db = Connection::open(dir.join("projection.db")).unwrap();
        let mut rows = db.prepare("SELECT ty, field FROM lookup_fields").unwrap();
        let rows = rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let mut kept: Vec<(String, String)> = rows.map(Result::unwrap).collect();
        kept.sort();
        assert_eq!(kept, whole(&decls, &asked), "after {what}");
    }
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
