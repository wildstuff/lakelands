//! The path an append takes after its write failed, entered as a sync
//! that fails enters it: no test here can make a sync fail after a write
//! that was whole.

use std::io::{self, Write};
use std::path::Path;

use lakelands_api::engine::Lease;
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use lakelands_core::conformance::engine::chain;

use super::{Log, segment_path};

/// A group whose commit line is whole may have been read by another
/// handle: the append whose sync failed after it cuts nothing of it, holds
/// it as every reader does, and says that its outcome is not known. Where
/// the cut fails, the error says what the next append does, and it does.
#[test]
fn an_append_that_failed_never_cuts_a_commit_line_that_is_whole() {
    let dir = std::env::temp_dir().join(format!("lakelands-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 6, &Digest::zero(), &Lease { fence, holder });
    let mut writer = Log::open(&dir, true).unwrap();
    writer.append(Some(1), &events[..2]).unwrap();
    // The group as a write leaves it whose sync alone fails: whole in both
    // files, and not in the writer's memory.
    let mut the_write = Log::open(&dir, false).unwrap();
    the_write.append(Some(1), &events[2..4]).unwrap();
    let mut reader = Log::open(&dir, false).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events[..4]);
    let sync = io::Error::other("the sync failed");
    let e = writer.failed(&events[2..4], Path::new("the index"), &sync);
    assert!(e.to_string().contains("the outcome is not known"), "{e}");
    assert_eq!(reader.read(Position(2), 9).unwrap(), events[2..4]);
    assert_eq!(writer.tip().map(|t| t.0), Some(Position(3)));

    // Half a line is left, and the segment cannot be opened to cut it.
    let (segment, aside) = (segment_path(&dir, 0), dir.join("aside"));
    let mut file = std::fs::OpenOptions::new().append(true).open(&segment);
    file.as_mut().unwrap().write_all(b"{\"half").unwrap();
    std::fs::rename(&segment, &aside).unwrap();
    std::fs::create_dir(&segment).unwrap();
    let e = writer.failed(&events[4..], Path::new("the segment"), &sync);
    assert!(e.to_string().contains("or is refused"), "{e}");
    assert!(writer.append(Some(1), &events[4..]).is_err(), "it wrote");
    std::fs::remove_dir(&segment).unwrap();
    std::fs::rename(&aside, &segment).unwrap();
    writer.append(Some(1), &events[4..]).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events);
    let _ = std::fs::remove_dir_all(&dir);
}
