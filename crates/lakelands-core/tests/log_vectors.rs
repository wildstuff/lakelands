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
    let why = lakelands_core::versions::below(&negative).expect("refused");
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
    let why = lakelands_core::versions::misplaced(&misplaced).expect("refused");
    assert!(why.contains("`record_retired`"), "{why}");
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
        Err(lakelands_api::Error::Unavailable { message }) => assert!(
            message.contains("at position 6:") && message.contains("known_at"),
            "{message}"
        ),
        other => {
            panic!("a known_at going back is refused at position 6, and it answered {other:?}")
        }
    }
}
