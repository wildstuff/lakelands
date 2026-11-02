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

/// A read the operating system refused because another handle holds the
/// file's lock names no holder; a failure that is not the lock's is still
/// told.
#[test]
fn a_read_refused_by_another_handles_lock_names_no_holder() {
    let refused = std::io::Error::from_raw_os_error(super::LOCK_VIOLATION);
    assert_eq!(
        super::answer(Err(refused)).unwrap(),
        (0, String::new()),
        "the lock's refusal is not a storage failure"
    );
    let failed = std::io::Error::from_raw_os_error(5);
    assert!(
        super::answer(Err(failed)).is_err(),
        "a failure that is not the lock's is still told"
    );
}

/// A holder checks the fence in the file it holds locked, never in a
/// second handle: the path can stop naming that file and the holder still
/// reads its own (lakelands#1688).
#[test]
fn a_held_lease_is_read_through_the_file_it_holds() {
    let dir = scratch("read-held");
    let lease = super::LeaseFile::new(&dir);
    let mut held = lease.take("holder").unwrap();
    let fence = held.lease.fence;
    std::fs::rename(dir.join("lease"), dir.join("lease.moved")).unwrap();
    assert_eq!(
        lease.recorded(&mut held).unwrap(),
        (fence, "holder".to_owned())
    );
    assert_eq!(
        lease.current().unwrap(),
        (0, String::new()),
        "a second handle reads the path, not the file held"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}
