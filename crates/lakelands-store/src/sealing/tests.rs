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

/// A version the log has moved past since a writer chose its own, as
/// another writer sealed with a newer one, is chosen again.
#[test]
fn a_version_the_log_moved_past_is_chosen_again() {
    use super::Keys;
    use lakelands_api::id::KeyId;
    use lakelands_api::seam::KeyStore;
    use lakelands_api::{LakeId, Result, SourceId};
    #[derive(Default)]
    struct Two(std::sync::Mutex<Vec<KeyId>>);
    impl KeyStore for Two {
        fn create(&self, _: &LakeId, _: &SourceId) -> Result<KeyId> {
            let mut held = self.0.lock().unwrap();
            let id = KeyId::from(format!("k{}", held.len() + 1));
            held.push(id.clone());
            Ok(id)
        }
        fn key(&self, _: &LakeId, _: &SourceId, _: &KeyId) -> Result<Option<DataKey>> {
            Ok(Some(key()))
        }
        fn destroy(&self, _: &LakeId, _: &SourceId) -> Result<()> {
            Ok(())
        }
        fn ids(&self, _: &LakeId, _: &SourceId) -> Result<Vec<KeyId>> {
            Ok(self.0.lock().unwrap().clone())
        }
    }
    let store = std::sync::Arc::new(Two::default());
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let keys = Keys::new(lake.clone(), Some(store.clone()));
    let (first, _) = keys.sealing(&crm, &[]).unwrap();
    let second = store.create(&lake, &crm).unwrap();
    let (kept, _) = keys.sealing(&crm, std::slice::from_ref(&first)).unwrap();
    assert_eq!(
        kept, first,
        "a writer keeps its version while the log used no newer"
    );
    let (now, _) = keys.sealing(&crm, &[first, second.clone()]).unwrap();
    assert_eq!(
        now, second,
        "once the log used a newer version, the writer seals with it"
    );
    // A choice that never reached the log gives way to a newer version
    // another writer used meanwhile.
    let web = SourceId::from("web");
    let (unused, _) = keys.sealing(&web, &[]).unwrap();
    let newest = store.create(&lake, &web).unwrap();
    let (now, _) = keys.sealing(&web, std::slice::from_ref(&newest)).unwrap();
    assert_ne!(unused, newest);
    assert_eq!(
        now, newest,
        "a choice the log never used gives way to the newer one it did"
    );
}
