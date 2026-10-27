use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use lakelands_api::event::{Event, EventBody, Sealed};
use lakelands_api::seam::DataKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use lakelands_api::id::KeyId;
use lakelands_api::seam::KeyStore;
use lakelands_api::{LakeId, Result, SourceId, Timestamp};

use super::{Apart, Ciphers, Keys, List, cipher, digest, open, rotates, seal, turns};

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

/// The vectors' ingest of each member sealed alone opens under their key
/// to its plaintext, and sealed again its member goes into a box the
/// patch leaves out, which opens to the same plaintext (083).
#[test]
fn the_vectors_sealed_ingest_opens_and_seals_again_in_a_box() {
    let stored = line("sealed_ingest");
    let mut plain = stored.clone();
    open(&mut plain, &cipher(&key())).unwrap();
    let EventBody::RecordIngested { patch, .. } = &plain.body else {
        panic!("an ingest")
    };
    assert_eq!(patch["email"], json!("ada@example.org"));
    let opened = plain.clone();
    let (mut sealed, source) = (plain.sealed.clone().unwrap(), plain.source.clone().unwrap());
    seal(&mut plain.body, &mut sealed, &source, &cipher(&key()));
    assert_eq!(patch_of(&plain).get("email"), None);
    assert!(sealed.boxed.is_some());
    plain.sealed = Some(sealed);
    open(&mut plain, &cipher(&key())).unwrap();
    assert_eq!(plain.body, opened.body);
}

/// The patch of an ingest.
fn patch_of(e: &Event) -> &lakelands_api::Patch {
    match &e.body {
        EventBody::RecordIngested { patch, .. } => patch,
        _ => panic!("an ingest"),
    }
}

/// `boxed.json`'s ingest, its members sealed in one box.
fn boxed() -> Event {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors/20/boxed.json");
    let boxed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    serde_json::from_str(boxed["line"].as_str().unwrap()).unwrap()
}

/// The vectors' box opens under their key to the members `sealed` names,
/// and sealing them again gives the box the log holds (083).
#[test]
fn the_vectors_box_opens_and_seals_to_the_same_bytes() {
    let stored = boxed();
    let mut plain = stored.clone();
    open(&mut plain, &cipher(&key())).unwrap();
    assert_eq!(patch_of(&plain)["email"], json!("cleo@example.org"));
    let (mut sealed, source) = (plain.sealed.clone().unwrap(), plain.source.clone().unwrap());
    seal(&mut plain.body, &mut sealed, &source, &cipher(&key()));
    assert_eq!((plain.body, Some(sealed)), (stored.body, stored.sealed));
}

/// An ingest of `name` and `city`, both sealed, and `id` plain, at
/// `version`.
fn two_sealed(version: u64) -> (Event, Sealed) {
    let mut e = boxed();
    if let EventBody::RecordIngested {
        patch, version: v, ..
    } = &mut e.body
    {
        *patch =
            serde_json::from_value(json!({"id": "c6", "name": "Cleo", "city": "Oslo"})).unwrap();
        *v = lakelands_api::Version(version);
    }
    let mut sealed = e.sealed.clone().unwrap();
    sealed.fields = vec!["city".into(), "name".into()];
    sealed.boxed = None;
    (e, sealed)
}

/// A record's sealed members go into one box, of one tag: the patch keeps
/// only what is plain, the box opens back to the whole patch, and the same
/// values at the same version seal to the same box, at another version
/// to another; a box moved to another record, or beside a member it holds,
/// does not open; erased, every member it holds is null.
#[test]
fn a_records_sealed_members_are_sealed_in_one_box() {
    let (plain, sealed) = two_sealed(1);
    let source = plain.source.clone().unwrap();
    let sealing = |(mut e, mut s): (Event, Sealed)| {
        seal(&mut e.body, &mut s, &source, &cipher(&key()));
        e.sealed = Some(s);
        e
    };
    let e = sealing((plain.clone(), sealed.clone()));
    assert_eq!(
        serde_json::to_value(patch_of(&e)).unwrap(),
        json!({"id": "c6"})
    );
    let boxed = e.sealed.as_ref().and_then(|s| s.boxed.clone()).unwrap();
    let members = br#"{"city":"Oslo","name":"Cleo"}"#;
    assert_eq!(
        base64::Engine::decode(&STANDARD, &boxed).unwrap().len(),
        members.len() + 16
    );
    let mut opened = e.clone();
    open(&mut opened, &cipher(&key())).unwrap();
    assert_eq!(opened.body, plain.body);
    assert_eq!(sealing((plain.clone(), sealed.clone())), e);
    let later = sealing(two_sealed(2));
    assert_ne!(later.sealed.and_then(|s| s.boxed), Some(boxed));
    let mut moved = e.clone();
    if let EventBody::RecordIngested { key, .. } = &mut moved.body {
        *key = "c7".into();
    }
    assert!(open(&mut moved, &cipher(&key())).is_err());
    let mut beside = e.clone();
    if let EventBody::RecordIngested { patch, .. } = &mut beside.body {
        patch.insert("name".into(), json!("Cleo"));
    }
    assert!(open(&mut beside, &cipher(&key())).is_err());
    let mut erased = e.clone();
    assert_eq!(super::erase(&mut erased), sealed.fields);
    let want = json!({"id": "c6", "city": null, "name": null});
    assert_eq!(serde_json::to_value(patch_of(&erased)).unwrap(), want);
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
    open(&mut e, &cipher(&key())).unwrap();
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
/// with the delivery's key, to its plaintext, and under neither alone,
/// and sealed again in a box opens only under both too;
/// its output's input is the HMAC of the call under the composed key
/// (065, lakelands#1214).
#[test]
fn the_vectors_ingest_sealed_apart_opens_only_under_both_keys() {
    let both = super::compose(&key(), &[apart()]);
    let stored = line("sealed_apart");
    for alone in [key(), apart()] {
        assert!(open(&mut stored.clone(), &cipher(&alone)).is_err());
    }
    let mut plain = stored.clone();
    open(&mut plain, &cipher(&both)).unwrap();
    let EventBody::RecordIngested { patch, .. } = &plain.body else {
        panic!("an ingest")
    };
    assert_eq!(patch["email"], json!("bea@example.org"));
    let opened = plain.clone();
    let (mut sealed, source) = (plain.sealed.clone().unwrap(), plain.source.clone().unwrap());
    seal(&mut plain.body, &mut sealed, &source, &cipher(&both));
    plain.sealed = Some(sealed);
    assert!(open(&mut plain.clone(), &cipher(&key())).is_err());
    open(&mut plain, &cipher(&both)).unwrap();
    assert_eq!(plain.body, opened.body);
    let mut e = line("sealed_apart_output");
    let call = json!({"function": "shout", "inputs": {"email": "bea@example.org"}, "config": {}});
    let EventBody::OutputRecorded { input, .. } = &e.body else {
        panic!("an output")
    };
    assert_eq!(*input, digest(&both, &call));
    assert_ne!(*input, digest(&key(), &call));
    open(&mut e, &cipher(&both)).unwrap();
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
    assert!(open(&mut e, &cipher(&key())).is_err());
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

/// A key's cipher, built once in a pass and kept for its every value,
/// seals each value to the bytes a cipher built for that value alone
/// does.
#[test]
fn a_kept_cipher_seals_to_the_bytes_of_one_built_a_value() {
    use std::sync::Arc;
    let keys = Keys::new(LakeId::from("l"), Some(Arc::new(Two::default())), None);
    let (e, sealed) = two_sealed(1);
    let source = e.source.clone().unwrap();
    let mut ciphers = Ciphers::default();
    let kept: *const _ = ciphers.sealed(&keys, &source, &sealed).unwrap();
    assert!(std::ptr::eq(
        kept,
        ciphers.sealed(&keys, &source, &sealed).unwrap()
    ));
    for version in 1..4 {
        let (e, mut each) = two_sealed(version);
        let (mut alone, mut body, mut once) = (e.body.clone(), e.body, each.clone());
        seal(&mut alone, &mut each, &source, &cipher(&key()));
        let kept = ciphers.sealed(&keys, &source, &once).unwrap();
        seal(&mut body, &mut once, &source, kept);
        assert_eq!((alone, each), (body, once));
    }
}

/// A version the log has moved past since a writer chose its own, as
/// another writer sealed with a newer one, is chosen again.
#[test]
fn a_version_the_log_moved_past_is_chosen_again() {
    let store = std::sync::Arc::new(Two::default());
    let (lake, crm) = (LakeId::from("l"), SourceId::from("crm"));
    let keys = Keys::new(lake.clone(), Some(store.clone()), None);
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
    let keys = Keys::new(lake.clone(), Some(store.clone()), None);
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
    let keys = Keys::new(lake.clone(), Some(store.clone()), None);
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

/// A memory lake on `keys` with a `person` type whose `name` is sealed,
/// its catalog and its owner.
async fn person_lake(
    name: &str,
    keys: std::sync::Arc<dyn KeyStore>,
) -> (
    std::sync::Arc<crate::lake::Lake>,
    std::sync::Arc<dyn lakelands_api::Catalog>,
    lakelands_api::Principal,
) {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::seam::Seams;
    use lakelands_api::{Grants, LakeStore, Location, Principal, PrincipalKind, WriteContext};
    let seams = Seams {
        engines: vec![std::sync::Arc::new(
            lakelands_engine_mem::MemoryOpener::new(),
        )],
        keys: Some(keys),
        backings: lakelands_core::conformance::backing::backings(),
        ..Seams::default()
    };
    let lake = crate::lake::Lake::open(&Location::memory(name), seams, OpenMode::Create);
    let lake = std::sync::Arc::new(lake.expect("the lake opens"));
    let doors = crate::doors::Store { lake: lake.clone() };
    let cat = doors.catalog(WriteContext::default());
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: name.into(),
        grants: Grants::owner(),
    };
    let person = serde_json::from_value(json!({
        "declare": "type", "name": "person", "key": ["id"], "fields": [
            {"name": "id", "type": {"kind": "text"}, "authority": "source"},
            {"name": "name", "type": {"kind": "text"}, "authority": "source", "erasable": true}]
    }));
    cat.declare(&owner, person.unwrap()).await.unwrap();
    (lake, cat, owner)
}

/// An ingest turns by its source's key period from the newest version's
/// first use as the list keeps it: one first used on an earlier day, as
/// an imported line from that day leaves it, has the source's next value
/// under `day` sealed under a new version, which the list then names.
#[tokio::test]
async fn an_ingest_past_its_newest_version_s_day_seals_under_a_new_one() {
    let store = std::sync::Arc::new(Two::default());
    let (lake, cat, owner) = person_lake("turn", store.clone()).await;
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
    let keys = Keys::new(LakeId::from("l"), Some(store.clone()), None);
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

/// A key apart a write's plan staged for its own ingest is answered as
/// staged, for its source alone, with no log to read (065).
#[test]
fn a_key_apart_staged_for_an_ingest_not_in_the_log_is_answered_as_staged() {
    let keys = Keys::new(LakeId::from("l"), None, None);
    let (crm, at) = (SourceId::from("crm"), lakelands_api::Position(7));
    let known = Apart::default();
    known.stage(&crm, at, &[KeyId::from("a1")]);
    assert_eq!(
        keys.apart_named(&known, &crm, at).unwrap(),
        [KeyId::from("a1")]
    );
    let erp = SourceId::from("erp");
    assert!(keys.apart_named(&known, &erp, at).unwrap().is_empty());
    let other = lakelands_api::Position(8);
    assert!(keys.apart_named(&known, &crm, other).unwrap().is_empty());
}

/// A key store that makes keys apart too, each its own bytes, and
/// destroys them.
#[derive(Default)]
struct Apiece(Two, std::sync::Mutex<(u32, Vec<KeyId>)>);
impl KeyStore for Apiece {
    fn create(&self, lake: &LakeId, source: &SourceId) -> Result<KeyId> {
        self.0.create(lake, source)
    }
    fn key(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<Option<DataKey>> {
        if !id.as_str().starts_with('a') {
            return self.0.key(lake, source, id);
        }
        let destroyed = self.1.lock().unwrap().1.contains(id);
        Ok((!destroyed).then(|| DataKey(Sha256::digest(id.as_str()).into())))
    }
    fn destroy(&self, lake: &LakeId, source: &SourceId) -> Result<()> {
        self.0.destroy(lake, source)
    }
    fn ids(&self, lake: &LakeId, source: &SourceId) -> Result<Vec<KeyId>> {
        self.0.ids(lake, source)
    }
    fn create_apart(&self, _: &LakeId, _: &SourceId) -> Result<KeyId> {
        let mut made = self.1.lock().unwrap();
        made.0 += 1;
        Ok(KeyId::from(format!("a{}", made.0)))
    }
    fn destroy_apart(&self, _: &LakeId, _: &SourceId, id: &KeyId) -> Result<()> {
        self.1.lock().unwrap().1.push(id.clone());
        Ok(())
    }
}

/// An ingest sealed apart keeps no `sys:apart` output: an output a later
/// event computes over its value reads its key apart from its event in
/// the log. Over a row a projection built by 0.19 left under that name,
/// naming another key, the output of `p1`'s city written after its name
/// is sealed under the version and the delivery's key, reads the name,
/// and dies when the delivery is taken back, which destroys its key
/// alone (065).
#[tokio::test]
async fn an_output_over_an_ingest_sealed_apart_does_not_read_a_leftover_row() {
    use lakelands_api::write::{IngestOpts, Selection};
    use lakelands_core::conformance::backing;
    let (lake, cat, owner) = person_lake("apart", std::sync::Arc::new(Apiece::default())).await;
    let text = json!({"kind": "text"});
    let pair = json!({"declare": "function", "name": "pair", "backing": backing::KIND,
        "inputs": [{"name": "name", "type": text}, {"name": "city", "type": text}],
        "config": {"token": "leftover", "echo": "name"}});
    let person = json!({"declare": "type", "name": "person", "key": ["id"], "fields": [
        {"name": "id", "type": text, "authority": "source"},
        {"name": "name", "type": text, "authority": "source", "erasable": true},
        {"name": "city", "type": text, "authority": "source"},
        {"name": "loud", "type": text, "authority": "derived", "function": "pair",
         "depends_on": ["name", "city"]}]});
    for d in [pair, person] {
        let declared = cat.declare(&owner, serde_json::from_value(d).unwrap());
        declared.await.unwrap();
    }
    let crm = SourceId::from("crm");
    let ingest = async |id: &str, fields: Value, opts: IngestOpts| {
        let record = lakelands_api::SourceRecord {
            source: None,
            upstream: None,
            ty: "person".into(),
            key: id.into(),
            fields: serde_json::from_value(fields).unwrap(),
            seen_at: None,
        };
        let fed = lakelands_api::ingest_all_with(cat.as_ref(), &owner, &crm, opts, vec![record]);
        fed.await.unwrap();
    };
    // `p0`, sealed under the version alone, so the act destroys no version.
    let p0 = json!({"id": "p0", "name": "Bo"});
    ingest("p0", p0, IngestOpts::default()).await;
    let at = lakelands_api::Position(lake.head().0 + 1);
    let late = IngestOpts {
        delivery: Some("late".into()),
        sealed_apart: true,
        ..IngestOpts::default()
    };
    ingest("p1", json!({"id": "p1", "name": "Ada"}), late).await;
    let row = crate::backed::digest(&json!([crm, at]));
    let held = || {
        let view = lake.engine.view(None).unwrap();
        view.output("sys:apart", &row).unwrap()
    };
    assert!(held().is_none(), "no output names the key apart");
    let a1 = vec![KeyId::from("a1")];
    let named = lake.keys.apart_named(&Apart::default(), &crm, at).unwrap();
    assert_eq!(named, a1, "the ingest of `p1` is at {at:?}");
    // The row 0.19 kept for the ingest, naming a key it did not seal with.
    let (p, head) = (lake.engine.projection(), lake.head());
    (p.begin()
        .and_then(|()| p.put_output("sys:apart", &row, &json!(["a9"]), head)))
    .and_then(|()| p.commit(head))
    .unwrap();
    assert!(held().is_some());
    let city = lakelands_api::Position(lake.head().0 + 1);
    ingest(
        "p1",
        json!({"id": "p1", "city": "Oslo"}),
        IngestOpts::default(),
    )
    .await;
    let through = u32::try_from(lake.head().0 + 1 - city.0).unwrap();
    let read = lake.engine.read(city, through).unwrap();
    let events = read.iter().map(crate::log::open);
    let outputs: Vec<_> = (events.map(Result::unwrap))
        .filter(|e| matches!(e.body, EventBody::OutputRecorded { .. }))
        .map(|e| e.sealed.map(|s| (s.key, s.apart)))
        .collect();
    let k1 = KeyId::from("k1");
    assert_eq!(
        outputs,
        [Some((k1, a1.clone()))],
        "the output over Ada and Oslo"
    );
    let loud = || {
        let view = lake.engine.view(None).unwrap();
        let p1 = view.get(&"person".into(), &"p1".into()).unwrap().unwrap();
        p1.fields.get("loud").cloned()
    };
    assert_eq!(loud(), Some(json!("Ada")));
    let doors = crate::doors::Store { lake: lake.clone() };
    let late = Selection::Delivery("late".into());
    let done = lakelands_api::LakeStore::remove_values(&doors, &crm, &late, true, None);
    let done = done.await.unwrap();
    assert_eq!((done.keys.is_empty(), done.apart), (true, a1));
    assert_eq!(loud(), None, "the output dies with the key apart");
}
