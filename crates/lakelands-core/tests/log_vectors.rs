//! The log format's test vectors (`docs/log-format/vectors/`), recomputed
//! from their bytes by this crate's canonical module and schema hash.
//!
//! The vectors are the document's worked example: a small lake's log in
//! its stored form, with every hash, both kinds of content-derived event
//! id, the segment digests and a view's schema hash. This test reads them
//! and recomputes every one, so `docs/log-format.md` and the code cannot
//! drift apart without it going red. `verify.py` beside the vectors checks
//! the same bytes with an implementation that shares no code with this
//! one.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::time::Timestamp;
use lakelands_api::write::Declaration;
use lakelands_core::{canonical, log, shape};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors")
}

fn read_json(name: &str) -> Value {
    serde_json::from_slice(&fs::read(vectors().join(name)).unwrap()).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    canonical::hex(&Sha256::digest(bytes))
}

fn str_of<'a>(v: &'a Value, member: &str) -> &'a str {
    v[member]
        .as_str()
        .unwrap_or_else(|| panic!("`{member}` is a string"))
}

/// The date-time a log holds is exactly the one form of §1.3.
fn assert_one_form(t: &str, at: u64) {
    let parsed = Timestamp::parse(t).unwrap_or_else(|e| panic!("position {at}: {e}"));
    assert_eq!(
        parsed.to_string(),
        t,
        "position {at}: `{t}` is not the one form"
    );
}

#[test]
fn every_hash_id_digest_and_schema_hash_recomputes() {
    let expected = read_json("expected.json");
    let segments = expected["segments"].as_array().unwrap();

    // Read every segment: its digest, and each line as canonical bytes.
    let mut events: Vec<(Value, Vec<u8>)> = Vec::new();
    for seg in segments {
        let raw = fs::read(vectors().join(str_of(seg, "file"))).unwrap();
        assert_eq!(sha(&raw), str_of(seg, "digest"), "{}", seg["file"]);
        assert_eq!(
            raw.last(),
            Some(&b'\n'),
            "every event ends with a line feed"
        );
        for line in raw[..raw.len() - 1].split(|b| *b == b'\n') {
            let value: Value = serde_json::from_slice(line).unwrap();
            assert_eq!(
                canonical::to_vec(&value),
                line,
                "position {}: the stored line is its canonical bytes",
                value["position"]
            );
            events.push((value, line.to_vec()));
        }
    }

    let want = expected["events"].as_array().unwrap();
    assert_eq!(events.len(), want.len());
    let mut prev = "0".repeat(64);
    let mut key_fields: HashMap<String, Vec<String>> = HashMap::new();
    for (i, ((value, _), want)) in events.iter().zip(want).enumerate() {
        let at = i as u64;
        let event: Event = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("position {at} reads as an Event: {e}"));
        assert_eq!(event.position.0, at);
        assert_eq!(event.prev.0, prev, "position {at}: prev");

        let mut unhashed = value.clone();
        unhashed.as_object_mut().unwrap().remove("hash");
        let hash = canonical::sha256_hex(&unhashed);
        assert_eq!(hash, event.hash.0, "position {at}: hash");
        assert_eq!(hash, str_of(want, "hash"), "position {at}: expected hash");
        assert_eq!(event.id.0, str_of(want, "id"), "position {at}: id");

        assert_one_form(str_of(value, "known_at"), at);
        if let Some(seen_at) = value["seen_at"].as_str() {
            assert_one_form(seen_at, at);
        }

        match (str_of(want, "id_from"), &event.body) {
            (
                "ingest",
                EventBody::RecordIngested {
                    ty,
                    key,
                    version,
                    patch,
                    ..
                },
            ) => {
                let source = event
                    .source
                    .as_ref()
                    .expect("an ingest event names its source");
                let id = log::ingest_id(source, ty, key, version.0 - 1, patch);
                assert_eq!(id, event.id, "position {at}: ingest id");
            }
            ("authored", _) => {
                let key = event.idempotency_key.as_ref().expect("an idempotency key");
                let id = log::authored_id(&event.writer, key);
                assert_eq!(id, event.id, "position {at}: authored id");
            }
            ("random", _) => {}
            (from, body) => panic!("position {at}: `{from}` id on a {}", body.kind()),
        }

        match &event.body {
            EventBody::RecordUpserted { ty, key, patch, .. }
            | EventBody::RecordIngested { ty, key, patch, .. } => {
                let fields = &key_fields[&ty.0];
                if !fields.is_empty() && fields.iter().all(|f| patch.contains_key(f.as_str())) {
                    let derived: Vec<String> = fields
                        .iter()
                        .map(|f| {
                            log::key_component(&patch[f.as_str()]).expect("a key field's value")
                        })
                        .collect();
                    assert_eq!(
                        derived.join(":"),
                        key.0,
                        "position {at}: key from its key fields"
                    );
                }
            }
            EventBody::Declared {
                declaration: Declaration::Type(ty),
                schema_hash: Some(stored),
                ..
            } => {
                key_fields.insert(
                    ty.name.0.clone(),
                    ty.key.iter().map(|f| f.0.clone()).collect(),
                );
                let computed = shape::schema_hash(ty).unwrap();
                assert_eq!(&computed, stored, "position {at}: schema hash");
                let listed = expected["schema_hashes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["position"] == at)
                    .expect("the schema hash is listed");
                assert_eq!(computed, str_of(listed, "schema_hash"));
            }
            EventBody::Checkpoint {
                through,
                hash,
                segments: listed,
            } => {
                assert_eq!(
                    hash.0, prev,
                    "the checkpoint repeats the sealed segment's last hash"
                );
                assert_eq!(through.0 + 1, at, "the checkpoint opens the next segment");
                let sealed: Vec<Value> = segments
                    .iter()
                    .filter(|s| s["last"].as_u64().unwrap() <= through.0)
                    .map(|s| {
                        serde_json::json!({ "first": s["first"], "last": s["last"], "digest": s["digest"] })
                    })
                    .collect();
                assert_eq!(serde_json::to_value(listed).unwrap(), Value::Array(sealed));
            }
            _ => {}
        }
        prev = event.hash.0;
    }
}

#[test]
fn a_view_hashes_its_conditions_in_typed_form() {
    let vector = read_json("schema-hash.json");
    let ty: TypeDecl = serde_json::from_value(vector["declaration"].clone()).unwrap();
    let shape_bytes = canonical::to_vec(&vector["shape"]);
    assert_eq!(
        String::from_utf8(shape_bytes.clone()).unwrap(),
        str_of(&vector, "shape_canonical")
    );
    assert_eq!(sha(&shape_bytes), str_of(&vector, "schema_hash"));
    assert_eq!(
        shape::schema_hash(&ty).unwrap(),
        str_of(&vector, "schema_hash"),
        "the declaration, conditions written as text, hashes to the vector's shape"
    );
}

/// Format version 2's one addition, `migrated.rekeyed`, in a worked event
/// (`docs/log-format.md` §6): its line is canonical, it reads into the
/// contract's event with the moved key, and its hash recomputes.
#[test]
fn the_version_2_rekey_event_recomputes() {
    let v2 = read_json("rekey.json");
    let line = str_of(&v2, "line");
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes the event exactly as the line"
    );
    assert_eq!(event.v, 2);
    let EventBody::Migrated { rekeyed, .. } = &event.body else {
        panic!("a migrated event")
    };
    assert_eq!(rekeyed.len(), 1);
    assert_eq!(rekeyed[0].to.as_str(), "DE%3A01");
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, str_of(&v2, "hash"));
    let expected = read_json("expected.json");
    let last = expected["events"].as_array().unwrap().last().unwrap();
    assert_eq!(event.prev.0, str_of(last, "hash"), "it follows position 7");
}

/// Format version 3's additions (`docs/log-format.md` §6) in worked
/// events: an ingest with `upstream`, whose id leaves the member out, and
/// a declaration calling `date_bucket`. Each line is canonical, reads into
/// the contract's event and back byte for byte, hashes to its hash and
/// follows the one before it, and needs version 3; the negative line, the
/// ingest written as version 2, is refused for carrying `upstream`, and the
/// misplaced line, a `record_retired` carrying it, for its kind.
#[test]
fn the_version_3_events_recompute_and_a_version_2_upstream_is_refused() {
    let v3 = read_json("v3.json");
    let mut prev = str_of(&read_json("rekey.json"), "hash").to_owned();
    let lines = v3["lines"].as_array().unwrap();
    let hashes = v3["hashes"].as_array().unwrap();
    let mut events = Vec::new();
    for (line, want) in lines.iter().zip(hashes) {
        let line = line.as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            canonical::to_vec(&value),
            line.as_bytes(),
            "the line is canonical"
        );
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.hash.0, want.as_str().unwrap());
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        assert_eq!(event.v, 3);
        assert_eq!(
            lakelands_core::versions::needed(&event.body, event.upstream.is_some()),
            3,
            "it needs version 3"
        );
        prev = event.hash.0.clone();
        events.push(event);
    }
    let ingested = &events[0];
    let EventBody::RecordIngested { ty, key, patch, .. } = &ingested.body else {
        panic!("an ingested event")
    };
    assert_eq!(
        ingested.id,
        lakelands_core::log::ingest_id(ingested.source.as_ref().unwrap(), ty, key, 0, patch),
        "the id leaves `upstream` out"
    );
    let negative: Event = serde_json::from_str(v3["negative"].as_str().unwrap()).unwrap();
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("`upstream`") && why.contains("format 3 added"),
        "{why}"
    );
    let misplaced: Event = serde_json::from_str(v3["misplaced"].as_str().unwrap()).unwrap();
    assert_eq!(
        lakelands_core::versions::below(&misplaced),
        None,
        "its version is right"
    );
    let why = lakelands_core::versions::misplaced(&misplaced)
        .expect("refused")
        .to_string();
    assert!(why.contains("`record_retired`"), "{why}");
}

/// Format version 4's addition (`docs/log-format.md` §6): a proposed
/// declaration and a proposed migration after `v3.json`'s position 10.
/// Each line is canonical, reads into the contract's event and back byte
/// for byte, hashes to its hash, follows the one before it and needs
/// version 4; the negative line, the declaration proposed as version 3,
/// is refused.
#[test]
fn the_version_4_proposals_recompute_and_a_version_3_one_is_refused() {
    let v4 = read_json("v4.json");
    let v3 = read_json("v3.json");
    let mut prev = v3["hashes"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    for (line, want) in v4["lines"]
        .as_array()
        .unwrap()
        .iter()
        .zip(v4["hashes"].as_array().unwrap())
    {
        let line = line.as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            canonical::to_vec(&value),
            line.as_bytes(),
            "the line is canonical"
        );
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.hash.0, want.as_str().unwrap());
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        assert_eq!(event.v, 4);
        assert_eq!(lakelands_core::versions::needed(&event.body, false), 4);
        prev = event.hash.0.clone();
    }
    let negative: Event = serde_json::from_str(v4["negative"].as_str().unwrap()).unwrap();
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("proposes a declaration or a migration") && why.contains("format 4 added"),
        "{why}"
    );
}

/// Format version 5's addition (`docs/log-format.md` §6): a `declared`
/// type carrying annotations, after `v4.json`'s position 12. The line is
/// canonical, reads into the contract's event and back byte for byte,
/// hashes to its hash, follows the one before it and needs version 5; its
/// schema hash is the one the same type without annotations hashes to;
/// the negative line, the same event written as version 4, is refused.
#[test]
fn the_version_5_annotations_recompute_and_a_version_4_one_is_refused() {
    let v5 = read_json("v5.json");
    let v4 = read_json("v4.json");
    let prev = v4["hashes"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap();
    let line = v5["lines"][0].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, v5["hashes"][0].as_str().unwrap());
    assert_eq!(event.prev.0, prev, "it follows the event before it");
    assert_eq!(event.v, 5);
    assert_eq!(lakelands_core::versions::needed(&event.body, false), 5);
    let lakelands_api::event::EventBody::Declared {
        declaration: lakelands_api::write::Declaration::Type(t),
        schema_hash,
        ..
    } = &event.body
    else {
        panic!("position 13 declares a type");
    };
    let mut bare = t.clone();
    bare.annotations.clear();
    bare.fields.iter_mut().for_each(|f| f.annotations.clear());
    bare.relations
        .iter_mut()
        .for_each(|r| r.annotations.clear());
    assert_eq!(
        schema_hash.as_deref(),
        Some(lakelands_api::shape::schema_hash(&bare).unwrap().as_str()),
        "annotations are not shape"
    );
    let misplaced: Event = serde_json::from_str(v5["misplaced"].as_str().unwrap()).unwrap();
    let why = lakelands_core::versions::misplaced(&misplaced)
        .expect("refused")
        .to_string();
    assert!(why.contains("`context` carries `cause`"), "{why}");
    let negative: Event = serde_json::from_str(v5["negative"].as_str().unwrap()).unwrap();
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("`annotations`") && why.contains("format 5 added"),
        "{why}"
    );
}

/// Format version 6's addition (`docs/log-format.md` §6): a `declared`
/// type with a derived relation, after `v5.json`'s position 13. The line
/// recomputes, needs version 6, and its schema hash is the one the same
/// type with the relation carried by links hashes to; the negative line,
/// the same event written as version 5, is refused.
#[test]
fn the_version_6_derived_relation_recomputes_and_a_version_5_one_is_refused() {
    let v6 = read_json("v6.json");
    let v5 = read_json("v5.json");
    let prev = v5["hashes"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap();
    let line = v6["lines"][0].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, v6["hashes"][0].as_str().unwrap());
    assert_eq!(event.prev.0, prev, "it follows the event before it");
    assert_eq!(event.v, 6);
    assert_eq!(lakelands_core::versions::needed(&event.body, false), 6);
    let lakelands_api::event::EventBody::Declared {
        declaration: lakelands_api::write::Declaration::Type(t),
        schema_hash,
        ..
    } = &event.body
    else {
        panic!("position 14 declares a type");
    };
    let mut carried = t.clone();
    carried.relations.iter_mut().for_each(|r| r.derived = None);
    assert_eq!(
        schema_hash.as_deref(),
        Some(
            lakelands_api::shape::schema_hash(&carried)
                .unwrap()
                .as_str()
        ),
        "`derived` is not shape"
    );
    let negative: Event = serde_json::from_str(v6["negative"].as_str().unwrap()).unwrap();
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("derived relation") && why.contains("format 6 added"),
        "{why}"
    );
    // Position 15: a trigger's `max_depth`, which version 6 also added.
    let line = v6["lines"][1].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    let capped: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&capped),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), capped.hash.0);
    assert_eq!(capped.hash.0, v6["hashes"][1].as_str().unwrap());
    assert_eq!(capped.prev, event.hash, "it follows position 14");
    assert_eq!(lakelands_core::versions::needed(&capped.body, false), 6);
    let mut below = capped.clone();
    below.v = 5;
    let why = lakelands_core::versions::below(&below)
        .expect("refused")
        .to_string();
    assert!(why.contains("`max_depth`"), "{why}");
}

#[test]
fn the_version_7_retired_guard_recomputes_and_a_version_6_one_is_refused() {
    let v6 = read_json("v6.json");
    let v7 = read_json("v7.json");
    let line = v7["lines"][0].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, v7["hashes"][0].as_str().unwrap());
    assert_eq!(
        event.prev.0,
        v6["hashes"][1].as_str().unwrap(),
        "it follows position 15"
    );
    assert_eq!(event.v, 7);
    assert_eq!(lakelands_core::versions::needed(&event.body, false), 7);
    let negative: Event = serde_json::from_str(v7["negative"].as_str().unwrap()).unwrap();
    assert_eq!(negative.v, 6);
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("`retired`") && why.contains("format 7 added"),
        "{why}"
    );
}

#[test]
fn the_version_8_view_with_parameters_recomputes_and_a_version_7_one_is_refused() {
    let v7 = read_json("v7.json");
    let v8 = read_json("v8.json");
    let line = v8["lines"][0].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, v8["hashes"][0].as_str().unwrap());
    assert_eq!(
        event.prev.0,
        v7["hashes"][0].as_str().unwrap(),
        "it follows position 16"
    );
    assert_eq!(event.v, 8);
    assert_eq!(lakelands_core::versions::needed(&event.body, false), 8);
    let negative: Event = serde_json::from_str(v8["negative"].as_str().unwrap()).unwrap();
    assert_eq!(negative.v, 7);
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("parameters") && why.contains("format 8 added"),
        "{why}"
    );
}

#[test]
fn the_version_8_verb_on_its_type_recomputes_and_a_version_7_one_is_refused() {
    let v8 = read_json("v8.json");
    let mut prev = v8["hashes"][0].as_str().unwrap().to_owned();
    for (i, what) in [
        (1, "declares a verb on its type"),
        (2, "calls a verb with no key"),
    ] {
        let line = v8["lines"][i].as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            canonical::to_vec(&value),
            line.as_bytes(),
            "the line is canonical"
        );
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.hash.0, v8["hashes"][i].as_str().unwrap());
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        assert_eq!(event.v, 8);
        assert_eq!(lakelands_core::versions::needed(&event.body, false), 8);
        prev = event.hash.0.clone();
        let name = if i == 1 {
            "negative_verb"
        } else {
            "negative_call"
        };
        let negative: Event = serde_json::from_str(v8[name].as_str().unwrap()).unwrap();
        assert_eq!(negative.v, 7);
        let why = lakelands_core::versions::below(&negative)
            .expect("refused")
            .to_string();
        assert!(
            why.contains(what) && why.contains("format 8 added"),
            "{why}"
        );
    }
    let lakelands_api::event::EventBody::Declared {
        declaration: Declaration::Verb(verb),
        ..
    } = &serde_json::from_str::<Event>(v8["lines"][1].as_str().unwrap())
        .unwrap()
        .body
    else {
        panic!("position 18 declares a verb");
    };
    assert_eq!(verb.scope, lakelands_api::ontology::VerbScope::Type);
}

#[test]
fn the_version_10_proposed_tightening_recomputes_and_a_version_9_one_is_refused() {
    let v9 = read_json("v9.json");
    let v10 = read_json("v10.json");
    let mut prev = v9["hashes"][0].as_str().unwrap().to_owned();
    // The declared value type needs no format of its own; it is written in
    // the version the lake had. The proposal needs 10.
    for (line, want) in v10["lines"]
        .as_array()
        .unwrap()
        .iter()
        .zip([(9, 1), (10, 10)])
    {
        let line = line.as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        let needed = lakelands_core::versions::needed(&event.body, false);
        assert_eq!((event.v, needed), want);
        prev = event.hash.0;
    }
    assert_eq!(prev, v10["hashes"][1].as_str().unwrap());
    let negative: Event = serde_json::from_str(v10["negative"].as_str().unwrap()).unwrap();
    assert_eq!(negative.v, 9);
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("tightening") && why.contains("format 10 added"),
        "{why}"
    );
}

#[test]
fn the_version_11_lines_recompute_and_each_written_as_version_10_is_refused() {
    let v11 = read_json("v11.json");
    let mut prev = read_json("v10.json")["hashes"][1]
        .as_str()
        .unwrap()
        .to_owned();
    for line in v11["lines"].as_array().unwrap() {
        let line = line.as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        let needed = lakelands_core::versions::needed(&event.body, false);
        assert_eq!((event.v, needed), (11, 11));
        prev = event.hash.0;
    }
    let hashes = v11["hashes"].as_array().unwrap();
    assert_eq!(prev, hashes.last().unwrap().as_str().unwrap());
    for negative in v11["negatives"].as_array().unwrap() {
        let negative: Event = serde_json::from_str(negative.as_str().unwrap()).unwrap();
        assert_eq!(negative.v, 10);
        let why = lakelands_core::versions::below(&negative)
            .expect("refused")
            .to_string();
        assert!(why.contains("format 11 added"), "{why}");
    }
}

#[test]
fn the_version_12_line_recomputes_and_written_as_version_11_is_refused() {
    let v12 = read_json("v12.json");
    let mut prev = read_json("v11.json")["hashes"][7]
        .as_str()
        .unwrap()
        .to_owned();
    for line in v12["lines"].as_array().unwrap() {
        let line = line.as_str().unwrap();
        let value: Value = serde_json::from_str(line).unwrap();
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            canonical::of(&event),
            line.as_bytes(),
            "the contract writes it as the line"
        );
        let mut body = value;
        body.as_object_mut().unwrap().remove("hash");
        assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
        assert_eq!(event.prev.0, prev, "it follows the event before it");
        let needed = lakelands_core::versions::needed(&event.body, false);
        assert_eq!((event.v, needed), (12, 12));
        prev = event.hash.0;
    }
    let hashes = v12["hashes"].as_array().unwrap();
    assert_eq!(prev, hashes.last().unwrap().as_str().unwrap());
    for negative in v12["negatives"].as_array().unwrap() {
        let negative: Event = serde_json::from_str(negative.as_str().unwrap()).unwrap();
        assert_eq!(negative.v, 11);
        let why = lakelands_core::versions::below(&negative)
            .expect("refused")
            .to_string();
        assert!(why.contains("format 12 added"), "{why}");
    }
}

#[test]
fn the_version_9_view_that_merges_recomputes_and_a_version_8_one_is_refused() {
    let v8 = read_json("v8.json");
    let v9 = read_json("v9.json");
    let line = v9["lines"][0].as_str().unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        canonical::to_vec(&value),
        line.as_bytes(),
        "the line is canonical"
    );
    let event: Event = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        canonical::of(&event),
        line.as_bytes(),
        "the contract writes it as the line"
    );
    let mut body = value;
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), event.hash.0);
    assert_eq!(event.hash.0, v9["hashes"][0].as_str().unwrap());
    let last = v8["hashes"].as_array().unwrap().last().unwrap();
    assert_eq!(event.prev.0, last.as_str().unwrap(), "it follows v8.json");
    assert_eq!(event.v, 9);
    assert_eq!(lakelands_core::versions::needed(&event.body, false), 9);
    let negative: Event = serde_json::from_str(v9["negative"].as_str().unwrap()).unwrap();
    assert_eq!(negative.v, 8);
    let why = lakelands_core::versions::below(&negative)
        .expect("refused")
        .to_string();
    assert!(
        why.contains("merges sources") && why.contains("format 9 added"),
        "{why}"
    );
}

/// The negative vector `negative/known-at-goes-back`: the first segment
/// of the vectors with the last event's `known_at` set before the one
/// before it, its hash made right. `export::verify` refuses it at that
/// position (log-format §1.3), as `verify.py` and `Lake::verify` do.
#[test]
fn the_negative_vector_whose_known_at_goes_back_is_refused_at_its_position() {
    let dir = vectors().join("negative/known-at-goes-back");
    // Only the rule is broken: position 6's hash is its line's own, and it
    // names position 5's as the hash before it.
    let text = fs::read_to_string(dir.join("log").join(format!("{:020}.jsonl", 0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let six: Value = serde_json::from_str(lines[6]).unwrap();
    let five: Value = serde_json::from_str(lines[5]).unwrap();
    let mut body = six.clone();
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), str_of(&six, "hash"));
    assert_eq!(str_of(&six, "prev"), str_of(&five, "hash"));
    assert!(str_of(&six, "known_at") < str_of(&five, "known_at"));
    match lakelands_core::export::verify(&dir) {
        Err(lakelands_api::Error::Unavailable { situation }) => assert!(
            situation.to_string().contains("at position 6:")
                && situation.to_string().contains("known_at"),
            "{situation}"
        ),
        other => {
            panic!("a known_at going back is refused at position 6, and it answered {other:?}")
        }
    }
}
