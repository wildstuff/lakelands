//! What a crash or a failed sync leaves in the log's files, and how a
//! handle reads it: the files are broken by hand, and a sync fails where
//! a test gives the log [`flaky`] to sync with.

use std::io::{self, Write};
use std::path::Path;

use lakelands_api::engine::Lease;
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use lakelands_core::conformance::engine::chain;

use super::{COMMITTED, Log, index_path, last_of, segment_path};

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
    let e = writer.failed(&events[2..4], &sync, false);
    assert!(e.to_string().contains("the outcome is not known"), "{e}");
    assert_eq!(reader.read(Position(2), 9).unwrap(), events[2..4]);
    assert_eq!(writer.tip().map(|t| t.0), Some(Position(3)));
    // One that found no room and left nothing says so, unless it wrote
    // again after a failed sync (lakelands#1568).
    let full = io::Error::from(io::ErrorKind::StorageFull);
    let mut no_room = |again| writer.failed(&events[4..], &full, again).to_string();
    assert!(no_room(false).contains("there is no room left"));
    assert!(!no_room(true).contains("there is no room left"));

    // Half a line is left, and the segment cannot be opened to cut it.
    let (segment, aside) = (segment_path(&dir, 0), dir.join("aside"));
    let mut file = std::fs::OpenOptions::new().append(true).open(&segment);
    file.as_mut().unwrap().write_all(b"{\"half").unwrap();
    std::fs::rename(&segment, &aside).unwrap();
    std::fs::create_dir(&segment).unwrap();
    let e = writer.failed(&events[4..], &sync, false);
    assert!(e.to_string().contains("or is refused"), "{e}");
    assert!(writer.append(Some(1), &events[4..]).is_err(), "it wrote");
    std::fs::remove_dir(&segment).unwrap();
    std::fs::rename(&aside, &segment).unwrap();
    writer.append(Some(1), &events[4..]).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lake whose log holds `events` in groups of two, as one writer
/// appended them, in a directory of its own.
fn appended(name: &str, events: &[lakelands_api::engine::CanonicalEvent]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lakelands-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut writer = Log::open(&dir, true).unwrap();
    events
        .chunks(2)
        .for_each(|group| writer.append(Some(1), group).unwrap());
    dir
}

/// A crash during an append's syncs can leave its commit line on disk
/// ahead of its bytes (060): short, zeroed, or another event's, or tear an
/// index line before it. Every
/// open checks the active segment's last group: a reader reads the log as
/// ending before it, the writer cuts it off, and the reader then reads
/// what the writer appends in its place. Where the group is the
/// segment's only one, the log reads as empty.
#[test]
fn a_last_group_whose_bytes_are_not_whole_was_never_answered() {
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 8, &Digest::zero(), &Lease { fence, holder });
    let len = |e: &[lakelands_api::engine::CanonicalEvent]| -> u64 {
        e.iter().map(|e| e.bytes.len() as u64 + 1).sum()
    };
    type Break = fn(&mut Vec<u8>, usize);
    let cases: [(&str, usize, Break); 5] = [
        ("short", 6, |bytes, from| bytes.truncate(from + 9)),
        ("zeroed", 6, |bytes, from| bytes[from..].fill(0)),
        ("another's", 6, |bytes, from| {
            let tail = bytes.split_off(from);
            let half = tail.len() / 2;
            bytes.extend_from_slice(&tail[half..]);
            bytes.extend_from_slice(&tail[..half]);
        }),
        ("the only", 2, |bytes, _| bytes.fill(0)),
        // The group's first index line, its line feed too, as zeros.
        ("torn index", 6, |bytes, _| {
            let commits: Vec<usize> = (0..bytes.len())
                .filter(|&i| bytes[i..].starts_with(b"[\"c\"]\n"))
                .collect();
            let from = commits[commits.len() - 2] + 6;
            let to = from + bytes[from..].iter().position(|b| *b == b'\n').unwrap();
            bytes[from..=to].fill(0);
        }),
    ];
    for (what, n, broken) in cases {
        let dir = appended(&what.replace(['\'', ' '], ""), &events[..n]);
        let segment = segment_path(&dir, 0);
        let file = match what {
            "torn index" => index_path(&dir, 0),
            _ => segment.clone(),
        };
        let mut bytes = std::fs::read(&file).unwrap();
        broken(&mut bytes, len(&events[..n - 2]) as usize);
        std::fs::write(&file, bytes).unwrap();
        let mut reader = Log::open(&dir, false).unwrap();
        let before = n as u64 - 2;
        let tip = before.checked_sub(1).map(Position);
        assert_eq!(reader.tip().map(|t| t.0), tip, "{what}");
        assert_eq!(
            reader.read(Position(0), 9).unwrap(),
            events[..n - 2],
            "{what}"
        );
        let again = reader.read(Position(0), 9).unwrap();
        assert_eq!(again, events[..n - 2], "{what}: read again before the cut");
        let mut writer = Log::open(&dir, false).unwrap();
        writer.recover().unwrap();
        let held = std::fs::metadata(&segment).unwrap().len();
        assert_eq!(held, len(&events[..n - 2]), "{what}: the writer cut it");
        writer.append(Some(1), &events[n - 2..n]).unwrap();
        assert_eq!(reader.read(Position(0), 9).unwrap(), events[..n], "{what}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Only the last group can be ahead of its bytes: a segment shorter than
/// an earlier group is damage, refused to the writer's cut and to the
/// read that meets it.
#[test]
fn a_segment_short_of_an_earlier_group_is_damaged() {
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 6, &Digest::zero(), &Lease { fence, holder });
    let dir = appended("earlier", &events);
    let segment = segment_path(&dir, 0);
    let mut bytes = std::fs::read(&segment).unwrap();
    bytes.truncate(events[0].bytes.len() + 1 + 9);
    std::fs::write(&segment, bytes).unwrap();
    let mut reader = Log::open(&dir, false).unwrap();
    let e = reader.read(Position(0), 9).unwrap_err();
    assert!(e.to_string().contains("damaged"), "{e}");
    let index = index_path(&dir, 0);
    let indexed = std::fs::metadata(&index).unwrap().len();
    let e = Log::open(&dir, false).unwrap().recover().unwrap_err();
    assert!(e.to_string().contains("damaged"), "{e}");
    let after = std::fs::metadata(&index).unwrap().len();
    assert_eq!(
        after, indexed,
        "the index was cut before the damage was refused"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// How many syncs [`flaky`] fails from now on, of files with which
/// extension.
static FAILING: std::sync::Mutex<(usize, &str)> = std::sync::Mutex::new((0, ""));

/// How many syncs [`flaky`] was asked for.
static ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A sync that fails while [`FAILING`] counts down, for its files.
fn flaky(path: &Path, file: &std::fs::File) -> io::Result<()> {
    ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut failing = FAILING.lock().unwrap();
    let fails = failing.0 > 0 && path.extension().is_some_and(|x| x == failing.1);
    failing.0 -= usize::from(fails);
    drop(failing);
    match fails {
        true => Err(io::Error::other("the sync failed")),
        false => file.sync_data(),
    }
}

/// A failed sync, of the segment or of the index alone, is written again
/// and synced once more (060). Failing once, the append answers as
/// written. Failing twice, it answers that its outcome is not known, and
/// the handle appends, seals and takes the lease no more; the lake opened
/// again checks the group, keeps it, writes it again and syncs both files
/// as a writer takes the lease, and goes on.
#[test]
fn a_failed_sync_is_written_again_and_a_second_failure_stops_the_handle() {
    use std::sync::atomic::Ordering::SeqCst;
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 8, &Digest::zero(), &Lease { fence, holder });
    for file in ["jsonl", "idx"] {
        let dir = appended(&format!("unsynced-{file}"), &events[..2]);
        let mut writer = Log::open(&dir, false).unwrap();
        writer.sync = flaky;
        *FAILING.lock().unwrap() = (1, file);
        writer.append(Some(1), &events[2..4]).unwrap();
        let read = Log::open(&dir, false).unwrap().read(Position(0), 9);
        assert_eq!(
            read.unwrap(),
            events[..4],
            "{file}: written again, the group stands"
        );
        *FAILING.lock().unwrap() = (2, file);
        let e = writer.append(Some(1), &events[4..6]).unwrap_err();
        assert!(
            e.to_string().contains("the outcome is not known"),
            "{file}: {e}"
        );
        *FAILING.lock().unwrap() = (0, "");
        let refused = [
            writer.append(Some(1), &events[6..]),
            writer
                .seal(Some(1), &mut |_| Ok(events[6..].to_vec()))
                .map(|_| ()),
            writer.recover(),
        ];
        for e in refused.map(Result::unwrap_err) {
            assert!(e.to_string().contains("open the lake again"), "{file}: {e}");
        }
        let mut again = Log::open(&dir, false).unwrap();
        again.recover().unwrap();
        assert_eq!(again.read(Position(0), 9).unwrap(), events[..6], "{file}");
        again.sync = flaky;
        let asked = ASKED.load(SeqCst);
        again.settle().unwrap();
        assert_eq!(
            ASKED.load(SeqCst) - asked,
            2,
            "{file}: the last group was synced again"
        );
        again.append(Some(1), &events[6..]).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What a writer's lease writes again is the last group alone, for a
/// handle that appended many and for one that read them.
#[test]
fn a_lease_writes_again_the_last_group_alone() {
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 10, &Digest::zero(), &Lease { fence, holder });
    let dir = appended("settled", &events[..2]);
    let mut writer = Log::open(&dir, false).unwrap();
    let mut reader = Log::open(&dir, false).unwrap();
    events[2..]
        .chunks(2)
        .for_each(|g| writer.append(Some(1), g).unwrap());
    reader.read(Position(0), 10).unwrap();
    let before: u64 = events[..8].iter().map(|e| e.bytes.len() as u64 + 1).sum();
    for (who, log) in [("the writer", &writer), ("a reader", &reader)] {
        let checked = log.segments.last().unwrap().checked.0;
        assert_eq!(checked, before, "{who} would write again from {checked}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A commit line that lies across two of the steps an index's end is read
/// in is found, so a handle's first read steps back to the commit line
/// before the last group and no further.
#[test]
fn a_commit_line_across_two_steps_is_found() {
    let dir = std::env::temp_dir().join(format!("lakelands-log-across-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("index");
    std::fs::write(&path, [&[b'a'; 20][..], COMMITTED, &[b'b'; 13]].concat()).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(last_of(&file, 40, COMMITTED, 16).unwrap(), Some(20));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A reader whose check refused a group checks the next one it reads, and
/// once one passes reads on: a byte flipped by hand in a later group is
/// the read's to meet, not a stall at the tip.
#[test]
fn a_reader_checks_until_a_group_passes() {
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 8, &Digest::zero(), &Lease { fence, holder });
    let dir = appended("passes", &events[..4]);
    let segment = segment_path(&dir, 0);
    let mut bytes = std::fs::read(&segment).unwrap();
    let last = bytes.len() - 9;
    bytes[last] ^= 1;
    std::fs::write(&segment, &bytes).unwrap();
    let mut reader = Log::open(&dir, false).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events[..2]);
    let mut writer = Log::open(&dir, false).unwrap();
    writer.recover().unwrap();
    events[2..]
        .chunks(2)
        .for_each(|g| writer.append(Some(1), g).unwrap());
    let mut bytes = std::fs::read(&segment).unwrap();
    let last = bytes.len() - 9;
    bytes[last] ^= 1;
    std::fs::write(&segment, &bytes).unwrap();
    assert_eq!(reader.tip().map(|t| t.0), Some(Position(1)));
    reader.refresh().unwrap();
    assert_eq!(
        reader.tip().map(|t| t.0),
        Some(Position(7)),
        "the reader stalled"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A crash that lost the last commit line and tore a line of its group
/// before it leaves a tail past the group the check passes: every read
/// takes it as a crash's, and the writer cuts it. A commit line after the
/// torn line makes it damage to a reader past its check.
#[test]
fn a_torn_line_past_the_group_the_check_passes_is_a_crash() {
    let (fence, holder) = (1, "writer".into());
    let events = chain(0, 6, &Digest::zero(), &Lease { fence, holder });
    let dir = appended("tail", &events);
    let index = index_path(&dir, 0);
    let mut bytes = std::fs::read(&index).unwrap();
    bytes.truncate(bytes.len() - (COMMITTED.len() - 1));
    let commits: Vec<usize> = (0..bytes.len())
        .filter(|&i| bytes[i..].starts_with(COMMITTED))
        .collect();
    let from = commits[commits.len() - 1] + COMMITTED.len();
    let to = from + bytes[from..].iter().position(|b| *b == b'\n').unwrap();
    bytes[from..=to].fill(0);
    std::fs::write(&index, bytes).unwrap();
    let mut reader = Log::open(&dir, false).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events[..4]);
    // A commit line after it makes the torn line damage.
    let torn = std::fs::read(&index).unwrap();
    std::fs::write(&index, [&torn[..], &COMMITTED[1..]].concat()).unwrap();
    let e = reader.read(Position(0), 9).unwrap_err();
    assert!(e.to_string().contains("damaged"), "{e}");
    std::fs::write(&index, torn).unwrap();
    let mut writer = Log::open(&dir, false).unwrap();
    writer.recover().unwrap();
    writer.append(Some(1), &events[4..]).unwrap();
    assert_eq!(reader.read(Position(0), 9).unwrap(), events);
    let _ = std::fs::remove_dir_all(&dir);
}
