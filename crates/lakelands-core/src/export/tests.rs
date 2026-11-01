use super::*;

/// `n` events from position 0 on, chained by their hashes, as the
/// store hands them to its writer's form.
fn chained(n: u64) -> Vec<CanonicalEvent> {
    let mut prev = Digest::zero();
    (0..n)
        .map(|i| {
            let mut event = serde_json::json!({
                "body": {"kind": "note", "n": i},
                "id": format!("{i:032x}"),
                "position": i,
                "prev": prev.0,
                "v": LOG_FORMAT_VERSION,
            });
            event["hash"] = crate::engines::hash_of(event.clone()).into();
            let stored = stored_of(&event);
            prev = stored.hash.clone();
            stored
        })
        .collect()
}

#[test]
fn a_tail_torn_between_blocks_reads_back_to_the_start_of_its_append() {
    let dir = std::env::temp_dir().join(format!("lakelands-tail-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tail.jsonl");
    // One event on its line, then an append of two blocks, the second
    // torn.
    let mut events = chained(1 + crate::log::BLOCK_EVENTS as u64 + 1);
    let last = crate::log::BLOCK_EVENTS as u64;
    let held = events[last as usize].bytes.clone();
    crate::log::frame_append(&mut events[1..]);
    let mut text = events[0].bytes.clone();
    text.push(b'\n');
    let cut = text.len() as u64;
    let blocks: Vec<_> = events[1..]
        .iter()
        .filter_map(|e| e.frame.as_ref().filter(|f| f.first == e.position))
        .collect();
    assert_eq!(blocks.len(), 2);
    text.extend_from_slice(&blocks[0].bytes);
    text.push(b'\n');
    text.extend_from_slice(&blocks[1].bytes[..blocks[1].bytes.len() / 2]);
    fs::write(&path, &text).unwrap();
    let tail = tail(&path).unwrap().unwrap();
    assert_eq!((tail.position, tail.run, tail.cut), (Position(last), Position(1), cut));
    assert_eq!(canonical::to_vec(&tail.line), held, "the block's last event, restored");
    fs::remove_dir_all(&dir).unwrap();
}
