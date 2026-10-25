use std::path::PathBuf;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::seam::DataKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use lakelands_api::id::KeyId;
use lakelands_api::seam::KeyStore;
use lakelands_api::{LakeId, Result, SourceId, Timestamp};

use super::{Keys, List, digest, open, rotates, seal, turns};

/// The lines of `vectors/20/further.json` that show `shows`.
fn line(shows: &str) -> Event {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors/20/further.json");
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

/// The vectors' key of `hr`'s delivery sealed apart, as their README
/// names it.
fn apart() -> DataKey {
    DataKey(Sha256::digest(b"lakelands:vectors:apart").into())
}

/// The vectors' ingest sealed apart opens under its version composed
/// with the delivery's key, to its plaintext, and under neither alone;
/// its output's input is the HMAC of the call under the composed key
/// (065, lakelands#1214).
#[test]
fn the_vectors_ingest_sealed_apart_opens_only_under_both_keys() {
    let both = super::compose(&key(), &[apart()]);
    let stored = line("sealed_apart");
    for alone in [key(), apart()] {
        assert!(open(&mut stored.clone(), &alone).is_err());
    }
    let mut plain = stored.clone();
    open(&mut plain, &both).unwrap();
    let EventBody::RecordIngested { patch, .. } = &plain.body else {
        panic!("an ingest")
    };
    assert_eq!(patch["email"], json!("bea@example.org"));
    let (sealed, source) = (plain.sealed.clone().unwrap(), plain.source.clone().unwrap());
    seal(&mut plain.body, &sealed, &source, &both);
    assert_eq!(plain.body, stored.body);
    let mut e = line("sealed_apart_output");
    let call = json!({"function": "shout", "inputs": {"email": "bea@example.org"}, "config": {}});
    let EventBody::OutputRecorded { input, .. } = &e.body else {
        panic!("an output")
    };
    assert_eq!(*input, digest(&both, &call));
    assert_ne!(*input, digest(&key(), &call));
    open(&mut e, &both).unwrap();
    let EventBody::OutputRecorded { output, .. } = &e.body else {
        panic!("an output")
    };
    assert_eq!(output.as_ref(), Some(&json!("BEA@EXAMPLE.ORG")));
}

/// A version composed with no key apart is the version itself, so a
/// value sealed before format 18 opens as it was sealed; composed with
/// keys apart it depends on each of them and on their order.
#[test]
fn a_version_composed_with_no_key_apart_is_itself() {
    let other = DataKey(Sha256::digest(b"another").into());
    assert_eq!(super::compose(&key(), &[]).0, key().0);
    let one = super::compose(&key(), &[apart()]).0;
    assert_ne!(one, key().0);
    assert_ne!(one, super::compose(&key(), std::slice::from_ref(&other)).0);
    assert_ne!(
        super::compose(&key(), &[apart(), other.clone()]).0,
        super::compose(&key(), &[other, apart()]).0
    );
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

/// A key store of counted versions, each with the vectors' key.
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

/// A version the log has moved past since a writer chose its own, as
/// another writer sealed with a newer one, is chosen again.
#[test]
fn a_version_the_log_moved_past_is_chosen_again() {
    let store = std::sync::Arc::new(Two::default());
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let keys = Keys::new(lake.clone(), Some(store.clone()));
    let (first, _) = keys.sealing(&crm, None, false).unwrap();
    let second = store.create(&lake, &crm).unwrap();
    let (kept, _) = keys.sealing(&crm, Some(&first), false).unwrap();
    assert_eq!(
        kept, first,
        "a writer keeps its version while the log used no newer"
    );
    let (now, _) = keys.sealing(&crm, Some(&second), false).unwrap();
    assert_eq!(
        now, second,
        "once the log used a newer version, the writer seals with it"
    );
    // A choice that never reached the log gives way to a newer version
    // another writer used meanwhile.
    let web = SourceId::from("web");
    let (unused, _) = keys.sealing(&web, None, false).unwrap();
    let newest = store.create(&lake, &web).unwrap();
    let (now, _) = keys.sealing(&web, Some(&newest), false).unwrap();
    assert_ne!(unused, newest);
    assert_eq!(
        now, newest,
        "a choice the log never used gives way to the newer one it did"
    );
}

/// A key period turns at each period's start in UTC: a day at midnight,
/// a week on Monday, a month on its first, a leap day and a year's end
/// included; within a period, and with no period, nothing turns.
#[test]
fn a_key_period_turns_at_each_boundary() {
    let at = |y, m, d, us: i64| Timestamp(Timestamp::from_date(y, m, d).unwrap().0 + us);
    let last = 86_400_000_000 - 1;
    let turns = |first, now, p| rotates(Some(first), now, Some(p));
    // 2026-10-17 is a Saturday; its week began on Monday the 12th.
    let week = at(2026, 10, 17, 5).period_start("week");
    assert_eq!(week, Some(at(2026, 10, 12, 0)));
    for (first, now, period, turned) in [
        (at(2026, 10, 17, last), at(2026, 10, 18, 0), "day", true),
        (at(2026, 10, 17, 0), at(2026, 10, 17, last), "day", false),
        (at(2026, 10, 18, last), at(2026, 10, 19, 0), "week", true),
        (at(2026, 10, 12, 0), at(2026, 10, 18, last), "week", false),
        (at(2026, 10, 31, last), at(2026, 11, 1, 0), "month", true),
        (at(2028, 2, 28, last), at(2028, 2, 29, 0), "month", false),
        (at(2028, 2, 29, last), at(2028, 3, 1, 0), "month", true),
        (at(2026, 12, 31, last), at(2027, 1, 1, 0), "month", true),
        (at(2026, 12, 31, last), at(2027, 1, 1, 0), "week", false),
        (at(2026, 12, 31, last), at(2027, 1, 1, 0), "day", true),
    ] {
        assert_eq!(
            turns(first, now, period),
            turned,
            "{first} → {now} by {period}"
        );
    }
    let (first, now) = (at(2026, 1, 1, 0), at(2027, 1, 1, 0));
    assert!(!rotates(Some(first), now, None), "no period, no turn");
    assert!(
        !rotates(Some(first), now, Some("year")),
        "a period of no name turns nothing"
    );
    assert!(
        rotates(None, now, Some("day")),
        "a first use of no time is a past period's"
    );
}

/// A writer told to turn makes a new version, keeps it for the rest of
/// its window, and keeps it once the log used it.
#[test]
fn a_writer_turned_seals_its_window_under_one_new_version() {
    let store = std::sync::Arc::new(Two::default());
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let keys = Keys::new(lake.clone(), Some(store.clone()));
    let (first, _) = keys.sealing(&crm, None, false).unwrap();
    let used = Some(&first);
    let (turned, _) = keys.sealing(&crm, used, true).unwrap();
    assert_ne!(turned, first, "the period turned");
    let (again, _) = keys.sealing(&crm, used, true).unwrap();
    assert_eq!(again, turned, "the window's next value seals as its first");
    let (kept, _) = keys.sealing(&crm, Some(&turned), false).unwrap();
    assert_eq!(kept, turned);
    assert_eq!(store.ids(&lake, &crm).unwrap().len(), 2, "one new version");
}

/// A version the key store made and the log has not used yet is a new
/// period's already: a writer told to turn seals with it and makes none.
#[test]
fn a_version_made_and_not_used_is_the_turned_period_s() {
    let store = std::sync::Arc::new(Two::default());
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let k1 = store.create(&lake, &crm).unwrap();
    let k2 = store.create(&lake, &crm).unwrap();
    let keys = Keys::new(lake.clone(), Some(store.clone()));
    let (turned, _) = keys.sealing(&crm, Some(&k1), true).unwrap();
    assert_eq!(turned, k2, "the version made and not used");
    assert_eq!(store.ids(&lake, &crm).unwrap().len(), 2, "no new version");
}

/// A source's key period is its own where the lake holds one for it,
/// else the lake's, else none, and is reckoned from its newest version's
/// first use as the list keeps it.
#[test]
fn a_source_turns_by_its_own_period_else_the_lake_s() {
    let at =
        |d, h: i64| Timestamp(Timestamp::from_date(2026, 10, d).unwrap().0 + h * 3_600_000_000);
    let list = List::of(json!([["k1", 3, at(12, 9).0], ["k2", 9, at(16, 12).0]]));
    let now = at(17, 10);
    let mut settings = std::collections::BTreeMap::new();
    let (crm, erp) = (SourceId::from("crm"), SourceId::from("erp"));
    assert!(
        !turns(&settings, Some(&list), &crm, now),
        "no period, no turn"
    );
    settings.insert("key_period".to_owned(), json!("week"));
    settings.insert("key_period.crm".to_owned(), json!("day"));
    assert!(
        turns(&settings, Some(&list), &crm, now),
        "crm's own day turned"
    );
    assert!(
        !turns(&settings, Some(&list), &erp, now),
        "the lake's week has not"
    );
    let today = List::of(json!([["k1", 3, at(12, 9).0], ["k2", 9, at(17, 8).0]]));
    assert!(
        !turns(&settings, Some(&today), &crm, now),
        "crm's newest version is today's, whatever its oldest"
    );
    let untimed = List::of(json!([["k1", 3]]));
    assert!(
        turns(&settings, Some(&untimed), &erp, now),
        "a first use of no time turns"
    );
    assert!(
        turns(&settings, None, &erp, now),
        "and so does a source that sealed nothing"
    );
}

/// A key store that destroys only whole sources refuses to destroy one
/// version, as unbound, by the seam's default.
#[test]
fn a_key_store_without_versions_refuses_to_destroy_one() {
    let store = Two::default();
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let k1 = store.create(&lake, &crm).unwrap();
    let refused = store.destroy_version(&lake, &crm, &k1);
    let seam = refused.as_ref().err().and_then(|e| e.seam());
    assert_eq!(
        seam,
        Some(&lakelands_api::error::Seam::KeyVersions),
        "{refused:?}"
    );
}

/// An ingest turns by its source's key period from the newest version's
/// first use as the list keeps it: one first used on an earlier day, as
/// an imported line from that day leaves it, has the source's next value
/// under `day` sealed under a new version, which the list then names.
#[tokio::test]
async fn an_ingest_past_its_newest_version_s_day_seals_under_a_new_one() {
    use crate::doors::Store;
    use crate::lake::Lake;
    use lakelands_api::engine::OpenMode;
    use lakelands_api::seam::Seams;
    use lakelands_api::write::Declaration;
    use lakelands_api::{Grants, LakeStore, Location, Principal, PrincipalKind, WriteContext};
    let store = std::sync::Arc::new(Two::default());
    let seams = Seams {
        engines: vec![std::sync::Arc::new(
            lakelands_engine_mem::MemoryOpener::new(),
        )],
        keys: Some(store.clone()),
        ..Seams::default()
    };
    let lake = Lake::open(&Location::memory("turn"), seams, OpenMode::Create);
    let lake = std::sync::Arc::new(lake.expect("the lake opens"));
    let cat = Store { lake: lake.clone() }.catalog(WriteContext::default());
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "turn".into(),
        grants: Grants::owner(),
    };
    let person: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "person", "key": ["id"], "fields": [
            {"name": "id", "type": {"kind": "text"}, "authority": "source"},
            {"name": "name", "type": {"kind": "text"}, "authority": "source", "erasable": true}]
    }))
    .unwrap();
    cat.declare(&owner, person).await.unwrap();
    let period = serde_json::from_value(json!({"key": "key_period.crm", "value": "day"}));
    let (ty, key) = ("sys.setting".into(), "key_period.crm".into());
    (cat.upsert(&owner, &ty, &key, period.unwrap(), Default::default())
        .await)
        .unwrap();
    let crm = SourceId::from("crm");
    let ingest = async |id: &str| {
        let record = lakelands_api::SourceRecord {
            source: None,
            upstream: None,
            ty: "person".into(),
            key: id.into(),
            fields: serde_json::from_value(json!({"id": id, "name": id})).unwrap(),
            seen_at: None,
        };
        let fed = lakelands_api::ingest_all(cat.as_ref(), &owner, &crm, vec![record]);
        fed.await.unwrap();
    };
    ingest("p1").await;
    let place = (super::USED, super::used_by(&crm));
    let listed = || {
        let view = lake.engine.view(None).unwrap();
        view.output(place.0, &place.1).unwrap().unwrap().1
    };
    // The list as an import of p1's line from the day before leaves it.
    let list = listed();
    let (k1, at) = (list[0][0].clone(), list[0][1].clone());
    let yesterday = list[0][2].as_i64().unwrap() - 86_400_000_000;
    let (p, head) = (lake.engine.projection(), lake.head());
    let earlier = json!([[k1, at, yesterday]]);
    (p.begin()
        .and_then(|()| p.put_output(place.0, &place.1, &earlier, head)))
    .and_then(|()| p.commit(head))
    .unwrap();
    ingest("p2").await;
    assert_eq!(store.ids(&LakeId::from("turn"), &crm).unwrap().len(), 2);
    let list = listed();
    let named: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e[0].clone())
        .collect();
    assert_eq!(
        named,
        [json!("k1"), json!("k2")],
        "p2 sealed under k2: {list}"
    );
}

/// A key store that counts its destroys and holds no version.
#[derive(Default)]
struct Bare(std::sync::atomic::AtomicU32);
impl KeyStore for Bare {
    fn create(&self, _: &LakeId, _: &SourceId) -> Result<KeyId> {
        Ok(KeyId::from("k1".to_owned()))
    }
    fn key(&self, _: &LakeId, _: &SourceId, _: &KeyId) -> Result<Option<DataKey>> {
        Ok(None)
    }
    fn destroy(&self, _: &LakeId, _: &SourceId) -> Result<()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn ids(&self, _: &LakeId, _: &SourceId) -> Result<Vec<KeyId>> {
        Ok(Vec::new())
    }
}

/// A source whose versions an act of removal took all is still destroyed
/// when it is forgotten: the key store marks it so, and gives it none
/// again (065).
#[test]
fn a_source_with_no_version_left_is_destroyed_all_the_same() {
    let store = std::sync::Arc::new(Bare::default());
    let keys = Keys::new(LakeId::from("l"), Some(store.clone()));
    keys.destroy(&SourceId::from("crm")).unwrap();
    assert_eq!(store.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// The list marks a version a delivery sealed apart sealed under with a
/// fourth member, and only those (065).
#[test]
fn the_list_marks_the_versions_a_delivery_sealed_apart_sealed_under() {
    let list = List::of(json!([["k1", 3, 7], ["k2", 9, 8, true], ["k3", 12]]));
    assert_eq!(list.apart, [KeyId::from("k2")]);
    assert_eq!(list.firsts.len(), 3);
    assert!(List::of(json!([["k1", 3, 7]])).apart.is_empty());
}
