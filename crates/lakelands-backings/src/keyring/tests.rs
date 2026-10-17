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

/// The key file the kept lake of format 16 ships
/// (`docs/log-format/lakes/16/keys/`) is read by this keyring as it is:
/// every key the lake's log seals under is given for a source kept, and
/// none for the source it forgot, which is destroyed. A change to the
/// file's form that would no longer read it turns this red.
#[test]
fn the_kept_lakes_key_file_is_read_as_written() {
    let lake =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/16");
    let log = std::fs::read_to_string(lake.join("log/00000000000000000000.jsonl")).unwrap();
    let ring = FileKeyring::new(lake.join("keys"));
    let kept16 = LakeId::from("kept16");
    let mut sealed = Vec::new();
    for line in log.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        let (Some(source), Some(key)) = (event["source"].as_str(), event["sealed"]["key"].as_str())
        else {
            continue;
        };
        let (source, key) = (SourceId::from(source), lakelands_api::id::KeyId::from(key));
        let given = ring.key(&kept16, &source, &key).unwrap();
        sealed.push((source.to_string(), given.is_some()));
    }
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
