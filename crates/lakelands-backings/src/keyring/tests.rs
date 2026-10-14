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
