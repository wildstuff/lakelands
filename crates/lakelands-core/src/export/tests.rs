use super::*;

#[test]
fn a_tail_with_a_corrupt_last_line_reads_no_further_than_its_window() {
    let dir = std::env::temp_dir().join(format!("lakelands-tail-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tail.jsonl");
    // A frame that covers more lines than the window holds, its lines,
    // and a last line that is not JSON.
    let n = WINDOW * 2;
    let mut text = format!(
        "{{\"v\":{},\"events\":{},\"frame\":{{}}}}\n",
        crate::log::FRAMED_FROM,
        n + 1
    );
    (0..n).for_each(|i| text += &format!("{{\"position\":{i}}}\n"));
    text += "{\"body\":{\n";
    fs::write(&path, &text).unwrap();
    let tail = tail(&path).unwrap().unwrap();
    assert!(tail.frame.is_none(), "the frame lies past the window");
    fs::remove_dir_all(&dir).unwrap();
}
