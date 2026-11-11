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

/// A key file as a lake of format 23 holds one is read by this keyring
/// as it is: `crm`'s version and `hr`'s key apart are given, and `web`,
/// which is destroyed, gives none. A change to the file's form that would
/// no longer read it turns this red.
#[test]
fn a_key_file_is_read_as_written() {
    let dir = scratch("as-written");
    std::fs::create_dir_all(&dir).unwrap();
    let (version, apart) = ("11".repeat(16), "22".repeat(16));
    let (held, held_apart) = ("ab".repeat(32), "cd".repeat(32));
    let file = serde_json::json!({"sources": {
        "crm": {"keys": [[version, held]]},
        "hr": {"keys": [], "apart": [[apart, held_apart]]},
        "web": {"keys": [], "destroyed": true},
    }});
    std::fs::write(dir.join("kept.keys.json"), file.to_string()).unwrap();
    let (ring, lake) = (FileKeyring::new(&dir), LakeId::from("kept"));
    let (crm, hr, web) = (
        SourceId::from("crm"),
        SourceId::from("hr"),
        SourceId::from("web"),
    );
    let key = ring.key(&lake, &crm, &version.as_str().into()).unwrap();
    assert_eq!(key.unwrap().0, [0xab; 32]);
    let key = ring.key(&lake, &hr, &apart.as_str().into()).unwrap();
    assert_eq!(key.unwrap().0, [0xcd; 32]);
    assert!(
        ring.key(&lake, &web, &version.as_str().into())
            .unwrap()
            .is_none()
    );
    assert!(ring.ids(&lake, &web).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key file of the wrong shape is refused in words that name the form and
/// never the value it held: a misfiled key reaches no error, and a version
/// written as a sequence is told its members are named, not given by
/// position.
#[test]
fn a_key_file_of_the_wrong_shape_names_the_form_and_not_the_key() {
    let dir = scratch("wrong-shape");
    std::fs::create_dir_all(&dir).unwrap();
    let held = "ab".repeat(32);
    let lake = LakeId::from("kept");
    let crm = SourceId::from("crm");
    std::fs::write(
        dir.join("kept.keys.json"),
        format!(r#"{{"sources": "{held}"}}"#),
    )
    .unwrap();
    let said = FileKeyring::new(&dir)
        .ids(&lake, &crm)
        .unwrap_err()
        .to_string();
    assert!(said.contains("not of the expected shape"), "{said}");
    assert!(!said.contains(&held), "{said}");
    std::fs::write(
        dir.join("kept.keys.json"),
        format!(r#"{{"sources": {{"crm": [["{held}"]]}}}}"#),
    )
    .unwrap();
    let said = FileKeyring::new(&dir)
        .ids(&lake, &crm)
        .unwrap_err()
        .to_string();
    assert!(said.contains("named, not given by position"), "{said}");
    for member in ["`keys`", "`apart`", "`destroyed`"] {
        assert!(said.contains(member), "{member} in {said}");
    }
    let _ = std::fs::remove_dir_all(&dir);
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
