#[test]
fn a_dropped_lease_is_released_while_a_copy_of_its_descriptor_lives() {
    let dir = std::env::temp_dir().join(format!("lakelands-lease-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
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
