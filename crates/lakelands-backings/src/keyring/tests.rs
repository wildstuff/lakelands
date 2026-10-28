use lakelands_api::error::Case;
use lakelands_api::seam::KeyStore;
use lakelands_api::{Error, LakeId, SourceId};

use super::FileKeyring;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lakelands-keyring-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A key made is given back under its version, by another keyring over
/// the same directory too, and a second version is given beside it.
#[test]
fn a_key_is_kept_by_version_across_keyrings() {
    let dir = scratch("kept");
    let (lake, crm) = (LakeId::from("orders"), SourceId::from("crm"));
    let ring = FileKeyring::new(&dir);
    assert!(ring.ids(&lake, &crm).unwrap().is_empty());
    let one = ring.create(&lake, &crm).unwrap();
    let key = ring.key(&lake, &crm, &one).unwrap().unwrap();
    let again = FileKeyring::new(&dir);
    assert_eq!(again.key(&lake, &crm, &one).unwrap().unwrap().0, key.0);
    let two = again.create(&lake, &crm).unwrap();
    assert_eq!(again.ids(&lake, &crm).unwrap(), [one.clone(), two.clone()]);
    let random = |id: &lakelands_api::id::KeyId| {
        id.as_str().len() == 32 && id.as_str().bytes().all(|b| b.is_ascii_hexdigit())
    };
    assert!(random(&one) && random(&two) && one != two, "{one} {two}");
    assert_ne!(again.key(&lake, &crm, &two).unwrap().unwrap().0, key.0);
    let other = SourceId::from("erp");
    assert!(again.ids(&lake, &other).unwrap().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.join("orders.keys.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the file is its owner's alone");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "and so is the directory it made");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A destroyed source's keys are gone, from another keyring over the same
/// directory too, and it is never given a key again.
#[test]
fn a_destroyed_source_has_no_key_and_gets_none() {
    let dir = scratch("destroyed");
    let (lake, crm) = (LakeId::from("orders"), SourceId::from("crm"));
    let ring = FileKeyring::new(&dir);
    let one = ring.create(&lake, &crm).unwrap();
    let hex = lakelands_api::canonical::hex(&ring.key(&lake, &crm, &one).unwrap().unwrap().0);
    ring.destroy(&lake, &crm).unwrap();
    let again = FileKeyring::new(&dir);
    assert!(again.key(&lake, &crm, &one).unwrap().is_none());
    assert!(again.ids(&lake, &crm).unwrap().is_empty());
    let refused = again.create(&lake, &crm);
    let destroyed = |e: &Error| {
        matches!(e, Error::Refused { reason: lakelands_api::Refusal::Invalid(said), .. }
            if matches!(&said.case, Case::KeyDestroyed { source } if source == "crm"))
    };
    assert!(refused.as_ref().is_err_and(destroyed), "{refused:?}");
    let text = std::fs::read_to_string(dir.join("orders.keys.json")).unwrap();
    assert!(
        !text.contains(&hex),
        "the destroyed key is gone from the file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each key the log of the kept lake of `format` seals under
/// (`docs/log-format/lakes/<format>/`), as its source and whether the
/// key file the lake ships gives it, in the log's order, with the keyring
/// over that file.
fn kept_lake_sealed(format: u32) -> (FileKeyring, LakeId, Vec<(String, bool)>) {
    let lake = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/lakes")
        .join(format.to_string());
    let log = std::fs::read_to_string(lake.join("log/00000000000000000000.jsonl")).unwrap();
    let ring = FileKeyring::new(lake.join("keys"));
    let id = LakeId::from(format!("kept{format}").as_str());
    let mut sealed = Vec::new();
    // From format 20 a frame holds what the events it covers share.
    let (mut frame, mut covered) = (serde_json::Value::Null, 0);
    for line in log.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        if event.get("frame").is_some() {
            (covered, frame) = (event["events"].as_u64().unwrap(), event["frame"].clone());
            continue;
        }
        let shared = covered > 0;
        covered = covered.saturating_sub(1);
        let held = |path: &dyn Fn(&serde_json::Value) -> &serde_json::Value| {
            let own = path(&event).as_str();
            let own = own.or_else(|| path(&frame).as_str().filter(|_| shared));
            own.map(str::to_owned)
        };
        let (Some(source), Some(key)) = (held(&|e| &e["source"]), held(&|e| &e["sealed"]["key"]))
        else {
            continue;
        };
        let (source, key) = (
            SourceId::from(source.as_str()),
            lakelands_api::id::KeyId::from(key.as_str()),
        );
        let given = ring.key(&id, &source, &key).unwrap();
        sealed.push((source.to_string(), given.is_some()));
    }
    (ring, id, sealed)
}

/// The key file the kept lake of format 16 ships
/// (`docs/log-format/lakes/16/keys/`) is read by this keyring as it is:
/// every key the lake's log seals under is given for a source kept, and
/// none for the source it forgot, which is destroyed. A change to the
/// file's form that would no longer read it turns this red.
#[test]
fn the_kept_lakes_key_file_is_read_as_written() {
    let (ring, kept16, sealed) = kept_lake_sealed(16);
    assert_eq!(
        sealed,
        [("crm".to_owned(), true), ("web".to_owned(), false)]
    );
    assert!(
        ring.ids(&kept16, &SourceId::from("web"))
            .unwrap()
            .is_empty()
    );
}

/// The key file of the kept lake of format 17 is read as written too:
/// `crm`'s key is given, the forgotten `web`'s is not, and of `ledger`'s
/// two versions the first, which taking back what it wrote before `p5`
/// destroyed (065), is not and the second is, the one version `ledger`
/// holds.
#[test]
fn the_kept_lake_of_format_17s_key_file_is_read_as_written() {
    let (ring, kept17, sealed) = kept_lake_sealed(17);
    let given = |s: &str, g: bool| (s.to_owned(), g);
    assert_eq!(
        sealed,
        [
            given("crm", true),
            given("web", false),
            given("ledger", false),
            given("ledger", true)
        ]
    );
    let ledger = ring.ids(&kept17, &SourceId::from("ledger")).unwrap();
    assert_eq!(ledger.len(), 1);
    assert!(
        ring.ids(&kept17, &SourceId::from("web"))
            .unwrap()
            .is_empty()
    );
}

/// The key file of the kept lake of format 18 is read as written too:
/// `ledger`'s second version and `hr`'s key are given, `web`'s and
/// `ledger`'s first are not.
#[test]
fn the_kept_lake_of_format_18s_key_file_is_read_as_written() {
    let (ring, kept18, sealed) = kept_lake_sealed(18);
    let given = |s: &str, g: bool| (s.to_owned(), g);
    assert!(sealed.contains(&given("hr", true)));
    assert!(sealed.contains(&given("ledger", true)));
    assert!(sealed.contains(&given("ledger", false)));
    let ids = |s: &str| ring.ids(&kept18, &SourceId::from(s)).unwrap();
    assert_eq!(ids("ledger").len(), 1);
    assert!(ids("web").is_empty());
}

/// The key file of the kept lake of format 19 is read as written too:
/// `ledger`'s second version and `hr`'s key are given, `web`'s and
/// `ledger`'s first are not.
#[test]
fn the_kept_lake_of_format_19s_key_file_is_read_as_written() {
    let (ring, kept19, sealed) = kept_lake_sealed(19);
    let given = |s: &str, g: bool| (s.to_owned(), g);
    assert!(sealed.contains(&given("hr", true)));
    assert!(sealed.contains(&given("ledger", true)));
    assert!(sealed.contains(&given("ledger", false)));
    let ids = |s: &str| ring.ids(&kept19, &SourceId::from(s)).unwrap();
    assert_eq!(ids("ledger").len(), 1);
    assert!(ids("web").is_empty());
}

/// The key file of the kept lake of format 20 is read as written too:
/// `crm`'s key is given, the forgotten `web`'s is not, though the three
/// records `crm` ingested in one append are sealed in boxes under it.
#[test]
fn the_kept_lake_of_format_20s_key_file_is_read_as_written() {
    let (ring, kept20, sealed) = kept_lake_sealed(20);
    let given = |s: &str, g: bool| (s.to_owned(), g);
    assert_eq!(
        sealed,
        [given("crm", true), given("crm", true), given("web", false)]
    );
    assert_eq!(ring.ids(&kept20, &SourceId::from("crm")).unwrap().len(), 1);
    assert!(
        ring.ids(&kept20, &SourceId::from("web"))
            .unwrap()
            .is_empty()
    );
}

/// A version destroyed goes with every older one, from another keyring
/// over the same directory too; the newer ones still open, a version not
/// held is refused, and the source is given a key again.
#[test]
fn a_destroyed_version_takes_the_older_ones_and_leaves_the_newer() {
    let dir = scratch("version");
    let (lake, crm) = (LakeId::from("orders"), SourceId::from("crm"));
    let ring = FileKeyring::new(&dir);
    let ids: Vec<_> = (0..3).map(|_| ring.create(&lake, &crm).unwrap()).collect();
    let newest = ring.key(&lake, &crm, &ids[2]).unwrap().unwrap().0;
    ring.destroy_version(&lake, &crm, &ids[1]).unwrap();
    let again = FileKeyring::new(&dir);
    assert_eq!(again.ids(&lake, &crm).unwrap(), [ids[2].clone()]);
    assert!(again.key(&lake, &crm, &ids[0]).unwrap().is_none());
    assert!(again.key(&lake, &crm, &ids[1]).unwrap().is_none());
    assert_eq!(again.key(&lake, &crm, &ids[2]).unwrap().unwrap().0, newest);
    for (source, id) in [(&crm, &ids[0]), (&SourceId::from("erp"), &ids[2])] {
        let refused = again.destroy_version(&lake, source, id).unwrap_err();
        let not_held = matches!(
            refused.case().map(|s| &s.case),
            Some(Case::KeyVersionNotHeld { .. })
        );
        assert!(not_held, "{refused}");
    }
    assert_eq!(again.ids(&lake, &crm).unwrap(), [ids[2].clone()]);
    again.destroy_version(&lake, &crm, &ids[2]).unwrap();
    assert!(again.ids(&lake, &crm).unwrap().is_empty());
    let next = again.create(&lake, &crm).unwrap();
    assert_eq!(again.ids(&lake, &crm).unwrap(), [next]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key made apart is answered by `key`, never listed among the versions,
/// destroyed alone, from another keyring over the same directory too, and
/// with the source's versions by `destroy`, after which none is made (065).
#[test]
fn a_key_apart_is_destroyed_alone_and_with_the_source() {
    let dir = scratch("apart");
    let (lake, crm) = (LakeId::from("orders"), SourceId::from("crm"));
    let ring = FileKeyring::new(&dir);
    let v1 = ring.create(&lake, &crm).unwrap();
    let (a1, a2) = (
        ring.create_apart(&lake, &crm).unwrap(),
        ring.create_apart(&lake, &crm).unwrap(),
    );
    assert_eq!(ring.ids(&lake, &crm).unwrap(), std::slice::from_ref(&v1));
    assert!(ring.key(&lake, &crm, &a1).unwrap().is_some());
    ring.destroy_apart(&lake, &crm, &a1).unwrap();
    ring.destroy_apart(&lake, &crm, &a1).unwrap();
    let again = FileKeyring::new(&dir);
    assert!(again.key(&lake, &crm, &a1).unwrap().is_none());
    assert!(again.key(&lake, &crm, &a2).unwrap().is_some());
    assert!(again.key(&lake, &crm, &v1).unwrap().is_some());
    again.destroy(&lake, &crm).unwrap();
    assert!(again.key(&lake, &crm, &a2).unwrap().is_none());
    let refused = again.create_apart(&lake, &crm).unwrap_err();
    let destroyed = matches!(
        refused.case().map(|s| &s.case),
        Some(Case::KeyDestroyed { .. })
    );
    assert!(destroyed, "{refused}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Keys made apart in one request are one for each entry, in order, each
/// of its source and given back by another keyring over the same
/// directory; a request naming a destroyed source makes none (087).
#[test]
fn keys_apart_made_in_one_request_are_kept_whole_or_not_at_all() {
    let dir = scratch("apart-many");
    let (lake, crm, erp) = (
        LakeId::from("orders"),
        SourceId::from("crm"),
        SourceId::from("erp"),
    );
    let ring = FileKeyring::new(&dir);
    let asked = [crm.clone(), erp.clone(), crm.clone()];
    let made = ring.create_apart_many(&lake, &asked).unwrap();
    assert_eq!(made.len(), 3);
    assert!(made[0] != made[2], "{made:?}");
    let again = FileKeyring::new(&dir);
    for (source, id) in asked.iter().zip(&made) {
        assert!(again.key(&lake, source, id).unwrap().is_some(), "{id}");
    }
    assert!(again.key(&lake, &crm, &made[1]).unwrap().is_none());
    again.destroy(&lake, &erp).unwrap();
    let refused = again
        .create_apart_many(&lake, &[crm.clone(), erp])
        .unwrap_err();
    let destroyed = matches!(
        refused.case().map(|s| &s.case),
        Some(Case::KeyDestroyed { .. })
    );
    assert!(destroyed, "{refused}");
    let ring = std::fs::read_to_string(dir.join("orders.keys.json")).unwrap();
    let held: serde_json::Value = serde_json::from_str(&ring).unwrap();
    assert_eq!(held["sources"]["crm"]["apart"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
