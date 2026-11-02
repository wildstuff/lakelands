fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lakelands-lease-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_dropped_lease_is_released_while_a_copy_of_its_descriptor_lives() {
    let dir = scratch("dropped");
    let lease = super::LeaseFile::new(&dir);
    let held = lease.take("first").unwrap();
    let copy = held.file.try_clone().unwrap();
    assert!(lease.take("second").is_err(), "the lease is held once");
    drop(held);
    assert!(
        lease.take("third").is_ok(),
        "the lock outlived the dropped lease"
    );
    drop(copy);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A take refused because another handle holds the lease answers
/// `LeaseHeld`, even where the operating system refuses the read that
/// would name the holder: Windows makes the lock cover the file's bytes,
/// so the second handle's read fails with a lock violation and must not
/// be reported as a storage failure (lakelands#1688).
#[test]
fn a_take_while_another_handle_holds_the_lease_answers_held() {
    let dir = scratch("held");
    let lease = super::LeaseFile::new(&dir);
    let first = lease.take("first").unwrap();
    let told = match lease.take("second") {
        Ok(_) => panic!("the lease was taken twice"),
        Err(e) => e.to_string(),
    };
    assert!(told.contains("writer lease is held"), "{told}");
    drop(first);
    let _ = std::fs::remove_dir_all(&dir);
}
