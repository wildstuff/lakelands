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
use lakelands_core::{canonical, shape};
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

/// A content-derived event id (`docs/log-format.md` §3): the first 16
/// bytes of SHA-256 over the parts joined by `0x00`.
fn content_id(parts: &[&[u8]]) -> String {
    sha(&parts.join(&0u8))[..32].to_owned()
}

fn str_of<'a>(v: &'a Value, member: &str) -> &'a str {
    v[member]
        .as_str()
        .unwrap_or_else(|| panic!("`{member}` is a string"))
}

/// A key field's value as one component of a record's key string
/// (`docs/log-format.md` §2.3): its JSON string form, with `%` and `:`
/// percent-encoded.
fn key_component(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(_) => String::from_utf8(canonical::to_vec(value)).unwrap(),
        other => panic!("{other} cannot be a key field's value"),
    };
    text.replace('%', "%25").replace(':', "%3A")
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
                let id = content_id(&[
                    b"lakelands:ingest:v1",
                    source.0.as_bytes(),
                    ty.0.as_bytes(),
                    key.0.as_bytes(),
                    (version.0 - 1).to_string().as_bytes(),
                    &canonical::of(patch),
                ]);
                assert_eq!(id, event.id.0, "position {at}: ingest id");
            }
            ("authored", _) => {
                let key = event.idempotency_key.as_ref().expect("an idempotency key");
                let id = content_id(&[
                    b"lakelands:authored:v1",
                    event.writer.as_bytes(),
                    key.0.as_bytes(),
                ]);
                assert_eq!(id, event.id.0, "position {at}: authored id");
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
                        .map(|f| key_component(&patch[f.as_str()]))
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
