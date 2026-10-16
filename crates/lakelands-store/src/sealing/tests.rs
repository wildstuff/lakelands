use std::path::PathBuf;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::seam::DataKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{digest, open, seal};

/// The lines of `vectors/17/further.json` that show `shows`.
fn line(shows: &str) -> Event {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors/17/further.json");
    let further: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let entries = further["events"].as_array().unwrap();
    let entry = entries.iter().find(|e| e["shows"] == shows).unwrap();
    serde_json::from_str(entry["line"].as_str().unwrap()).unwrap()
}

/// The vectors' key, as their README names it.
fn key() -> DataKey {
    DataKey(Sha256::digest(b"lakelands:vectors:key").into())
}

/// The vectors' sealed ingest opens under their key to its plaintext,
/// and sealing the plaintext again gives the bytes the log holds.
#[test]
fn the_vectors_sealed_ingest_opens_and_seals_to_the_same_bytes() {
    let stored = line("sealed_ingest");
    let mut plain = stored.clone();
    open(&mut plain, &key()).unwrap();
    let EventBody::RecordIngested { patch, .. } = &plain.body else {
        panic!("an ingest")
    };
    assert_eq!(patch["email"], json!("ada@example.org"));
    let (sealed, source) = (plain.sealed.clone().unwrap(), plain.source.clone().unwrap());
    seal(&mut plain.body, &sealed, &source, &key());
    assert_eq!(plain.body, stored.body);
}

/// The vectors' sealed output opens under their key, and its input is
/// the HMAC of the call under the key derived from theirs.
#[test]
fn the_vectors_sealed_output_opens_and_its_input_is_the_hmac_of_the_call() {
    let mut e = line("sealed_output");
    let call = json!({"function": "shout", "inputs": {"email": "ada@example.org"}, "config": {}});
    let EventBody::OutputRecorded { input, .. } = &e.body else {
        panic!("an output")
    };
    assert_eq!(*input, digest(&key(), &call));
    open(&mut e, &key()).unwrap();
    let EventBody::OutputRecorded { output, .. } = &e.body else {
        panic!("an output")
    };
    assert_eq!(output.as_ref(), Some(&json!("ADA@EXAMPLE.ORG")));
}

/// A sealed value moved to another record does not open: its nonce is
/// made from the place the fold reads it at.
#[test]
fn a_sealed_value_moved_to_another_record_does_not_open() {
    let mut e = line("sealed_ingest");
    if let EventBody::RecordIngested { key, .. } = &mut e.body {
        *key = "c2".into();
    }
    assert!(open(&mut e, &key()).is_err());
}
