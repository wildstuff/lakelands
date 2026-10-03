//! Every lake exports its log as the segment files a verifier reads
//! without Lakelands
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`,
//! `docs/log-format.md` §4.2).
//!
//! The export is read through the store's change feed, so each check runs
//! it the way a host does, against the lake the engine holds, and reads the
//! files back byte by byte.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::{Error, LakeStore, Position, Refusal};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::rekey;
use crate::canonical;
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, doors, find_at, log_events, owner, reason, type_of,
};
use crate::export::{self, Manifest};

check!(
    /// One file per segment, the events' canonical lines.
    ONE_FILE_PER_SEGMENT = an_export_writes_one_file_per_segment_of_canonical_lines,
    "030 and log-format §4.2: an export writes one file per segment, named by its first position in 20 digits, holding each event's canonical bytes and a line feed, and the sealed file's digest is the one the checkpoint after it lists",
    scratch
);

check!(
    /// The export verifies to the lake's own answer.
    VERIFIES = an_export_verifies_to_the_through_and_hash_the_lake_answers,
    "030: an export verifies from its files alone to the position and hash the lake's own verify answers; its manifest names the same pair and every file with its digest, and is never trusted: with a wrong manifest or none the export verifies the same",
    scratch
);

check!(
    /// Both format versions, as they were written.
    MIXED_VERSIONS = a_rekeyed_lake_exports_both_format_versions_as_written,
    "030 and log-format §6: a lake that re-keyed exports its version-1 events and its version-2 events as they were written, and the export verifies",
    scratch
);

check!(
    /// A later export repeats the earlier one's sealed files.
    PREFIX = a_later_export_repeats_the_sealed_files_and_extends_the_active_one,
    "030: an export reads through the head as it stood when it began; a lake never checkpointed exports one file 00000000000000000000.jsonl, and a later export has the earlier sealed files byte for byte and an active file the earlier one begins",
    scratch
);

check!(
    /// A directory holding anything is refused.
    NOT_EMPTY = an_export_into_a_directory_that_holds_anything_is_refused,
    "030: an export writes only into an empty or absent directory; into any other it is refused as a failed precondition naming the directory, and writes nothing",
    scratch
);

check!(
    /// A damaged export is refused at its position.
    DAMAGE = a_damaged_export_is_refused_naming_the_position,
    "030 and log-format §2.6 and §4.2: verifying an export refuses a changed byte, a space inserted, a missing last line feed, a format version below the first, past the newest or going down, a known_at going back, a checkpoint naming the wrong position before it, a missing file, an empty log/, a file too many, a file not named in 20 digits and two lines out of order, and names the position where the files stop being the log",
    scratch
);

check!(
    /// An imported export is the lake it came from.
    IMPORT_IS_THE_LAKE = an_export_imported_into_a_fresh_lake_answers_as_the_lake_it_came_from,
    "030 and log-format §4: an export imported into a fresh lake of the engine holds every event unchanged at its position, of every format version the lake wrote (annotations included), so the lake answers as the one exported (the head, verify, the feed from position 0, every type's records with provenance at the head and at an earlier position, describe) and exports the same files byte for byte; its next writer takes a lease greater than every fence in the log and continues it",
    given
);

check!(
    /// An import refuses what is not a birth of the exported lake.
    IMPORT_REFUSED = an_import_is_refused_for_a_damaged_export_another_lake_or_a_lake_holding_events,
    "030: an import verifies the whole export before it writes: a changed byte in a segment is refused naming its position, and an export of another lake (naming both lakes) and a lake that holds events (naming its head) are refused as failed preconditions; nothing is written into the lake it refuses",
    given
);

check!(
    /// An import refuses a declaration member it does not know.
    IMPORT_UNKNOWN_MEMBER = an_import_refuses_a_declaration_member_it_does_not_know_naming_it,
    "028 and log-format §2: an export whose `declared` event carries a member its declaration does not take, its hash made right, is refused on import, naming the member and the declaration",
    given
);

check!(
    /// A cycle of views from a log is refused on read.
    IMPORTED_VIEW_CYCLE = a_cycle_of_views_from_an_imported_log_is_refused_on_read,
    "018 and 030: a log can bring in a cycle of views over `source` that `declare` would have refused; a read of a view on it is refused as invalid, naming the chain, the process goes on, and a view off the cycle reads as before",
    given
);

check!(
    /// A line whose `context` carries `cause` is refused.
    IMPORT_CONTEXT_CAUSE = an_import_refuses_a_context_carrying_cause_naming_it,
    "021, extended, and log-format §1.1: a host's cause is the envelope's and never written inside `context`; an export whose line carries `cause` in its `context`, its hash made right, is refused on import naming the member, and a write through the doors carries no cause",
    given
);

/// The check's scratch directory, cleared of anything an earlier run
/// left, and removed again when the check ends, however it ends.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn new(base: PathBuf) -> Scratch {
        let _ = fs::remove_dir_all(&base);
        Scratch(base)
    }

    /// A directory for one export, not yet created.
    pub(super) fn dir(&self, what: &str) -> PathBuf {
        self.0.join(what)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn write(store: &dyn LakeStore, dir: &Path) -> Result<Manifest, Failure> {
    export::write(store, dir)
        .await
        .must(&format!("exporting into {}", dir.display()))
}

fn read(dir: &Path, file: &str) -> Result<Vec<u8>, Failure> {
    fs::read(dir.join(file)).map_err(|e| Failure::new(format!("reading {file} of the export: {e}")))
}

/// The files under `log/`, sorted.
fn files(dir: &Path) -> Result<Vec<String>, Failure> {
    let mut out: Vec<String> = fs::read_dir(dir.join("log"))
        .map_err(|e| Failure::new(format!("listing the export's log/: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| format!("log/{}", e.file_name().to_string_lossy()))
        .collect();
    out.sort();
    Ok(out)
}

/// A lake with an `invoice` type and `n` invoices written.
async fn invoices(store: &dyn LakeStore, from: u32, n: u32) -> Result<Position, Failure> {
    let l = Lake::of(store);
    if from == 0 {
        l.declare(type_of("invoice", &[("amount", Number)])).await?;
    }
    let mut at = Position::GENESIS;
    for i in from..from + n {
        at = l
            .upsert("invoice", &format!("i{i}"), json!({"amount": i}))
            .await?
            .position();
    }
    Ok(at)
}

async fn an_export_writes_one_file_per_segment_of_canonical_lines(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    invoices(store.as_ref(), 0, 3).await?;
    let sealed_at = store.checkpoint().await.must("checkpointing")?;
    invoices(store.as_ref(), 3, 2).await?;
    let dir = scratch.dir("segments");
    write(store.as_ref(), &dir).await?;

    let want = vec![
        export::file_name(Position::GENESIS),
        export::file_name(sealed_at),
    ];
    let got = files(&dir)?;
    ensure!(
        got == want,
        "a lake checkpointed at {sealed_at} exports the files {want:?}, and the export holds {got:?}"
    );
    let all = log_events(store.as_ref()).await?;
    for (file, run) in [
        (&want[0], &all[..sealed_at.0 as usize]),
        (&want[1], &all[sealed_at.0 as usize..]),
    ] {
        let mut lines = Vec::new();
        for e in run {
            lines.extend(canonical::of(e));
            lines.push(b'\n');
        }
        ensure!(
            read(&dir, file)? == lines,
            "{file} holds the canonical line of each event from position {} and a line feed after \
             each, and it does not",
            run[0].position
        );
    }
    let listed = match &all[sealed_at.0 as usize].body {
        EventBody::Checkpoint { segments, .. } => segments.clone(),
        other => {
            return Err(Failure::new(format!(
                "the event at {sealed_at} is the checkpoint, and it is {}",
                other.kind()
            )));
        }
    };
    let digest = canonical::hex(&Sha256::digest(read(&dir, &want[0])?));
    ensure!(
        listed.len() == 1 && listed[0].digest.0 == digest,
        "the checkpoint lists the sealed file with the digest {digest} of its bytes, and it lists \
         {listed:?}"
    );
    Ok(())
}

async fn an_export_verifies_to_the_through_and_hash_the_lake_answers(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    invoices(store.as_ref(), 0, 2).await?;
    store.checkpoint().await.must("checkpointing")?;
    invoices(store.as_ref(), 2, 2).await?;
    let dir = scratch.dir("verifies");
    let manifest = write(store.as_ref(), &dir).await?;
    let lake = store.verify(None, None).await.must("verifying the lake")?;
    let files_say = export::verify(&dir).must("verifying the export")?;
    ensure!(
        (files_say.through, &files_say.hash) == (lake.through, &lake.hash),
        "the export verifies to {} {}, and the lake answers {} {}",
        files_say.through,
        files_say.hash.0,
        lake.through,
        lake.hash.0
    );
    ensure!(
        (manifest.through, &manifest.hash, &manifest.lake)
            == (lake.through, &lake.hash, store.lake()),
        "the manifest names the lake, {} and {}, and it names {:?}",
        lake.through,
        lake.hash.0,
        (manifest.lake, manifest.through, manifest.hash)
    );
    let on_disk: Manifest = serde_json::from_slice(&read(&dir, "export.json")?)
        .map_err(|e| Failure::new(format!("export.json is not a manifest: {e}")))?;
    ensure!(
        on_disk == manifest,
        "export.json is the manifest the export answered, and it is {on_disk:?}"
    );
    let listed: Vec<&str> = manifest.segments.iter().map(|s| s.file.as_str()).collect();
    ensure!(
        listed == files(&dir)?,
        "the manifest lists every file of the export, and it lists {listed:?}"
    );
    for s in &manifest.segments {
        let digest = canonical::hex(&Sha256::digest(read(&dir, &s.file)?));
        ensure!(
            s.digest.0 == digest,
            "the manifest gives {} the digest of its bytes, {digest}, and it gives {}",
            s.file,
            s.digest.0
        );
    }
    // The manifest is never trusted: a wrong one, and none, change nothing.
    let manifest_path = dir.join("export.json");
    for (what, wrong) in [
        ("a wrong manifest", Some(&b"{\"through\": 0}\n"[..])),
        ("no manifest", None),
    ] {
        match wrong {
            Some(bytes) => fs::write(&manifest_path, bytes),
            None => fs::remove_file(&manifest_path),
        }
        .map_err(|e| Failure::new(format!("changing export.json: {e}")))?;
        let again = export::verify(&dir).must(&format!("verifying the export with {what}"))?;
        ensure!(
            again == files_say,
            "an export verifies from log/ alone, so with {what} it answers {files_say:?} again, \
             and it answered {again:?}"
        );
    }
    Ok(())
}

async fn a_rekeyed_lake_exports_both_format_versions_as_written(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    rekey::lake(store.as_ref(), rekey::IBANS).await?;
    let at = rekey::apply(store.as_ref()).await?;
    let dir = scratch.dir("versions");
    let manifest = write(store.as_ref(), &dir).await?;
    let mut lines = Vec::new();
    for file in files(&dir)? {
        lines.extend(read(&dir, &file)?);
    }
    let mut want = Vec::new();
    for e in log_events(store.as_ref()).await? {
        want.extend(canonical::of(&e));
        want.push(b'\n');
    }
    ensure!(
        lines == want,
        "the export holds every event's line as the lake holds it, version 1 before the re-key at \
         {at} and version 2 from it, and it does not"
    );
    ensure!(
        manifest.versions == [1, 2],
        "the manifest of a lake that re-keyed names versions [1, 2], and it names {:?}",
        manifest.versions
    );
    export::verify(&dir).must("verifying an export of both format versions")?;
    Ok(())
}

async fn a_later_export_repeats_the_sealed_files_and_extends_the_active_one(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    let first = invoices(store.as_ref(), 0, 2).await?;
    let (a, b, c) = (scratch.dir("a"), scratch.dir("b"), scratch.dir("c"));
    let early = write(store.as_ref(), &a).await?;
    let only = export::file_name(Position::GENESIS);
    ensure!(
        files(&a)? == [only.clone()] && early.through == first,
        "a lake never checkpointed exports one file, {only}, through its head {first}; it wrote \
         {:?} through {}",
        files(&a)?,
        early.through
    );
    let sealed_at = store.checkpoint().await.must("checkpointing")?;
    invoices(store.as_ref(), 2, 1).await?;
    write(store.as_ref(), &b).await?;
    invoices(store.as_ref(), 3, 2).await?;
    let late = write(store.as_ref(), &c).await?;
    ensure!(
        read(&a, &only)? == read(&c, &only)?,
        "the segment sealed at {sealed_at} is exported byte for byte as the earlier export wrote \
         it"
    );
    let active = export::file_name(sealed_at);
    let (was, is) = (read(&b, &active)?, read(&c, &active)?);
    ensure!(
        is.len() > was.len() && is.starts_with(&was),
        "a later export's active file begins with the earlier one's and is longer: {} bytes \
         then {}",
        was.len(),
        is.len()
    );
    ensure!(
        late.through == store.head().await.must("reading the head")?,
        "an export reads through the head, {}",
        late.through
    );
    Ok(())
}

async fn an_export_into_a_directory_that_holds_anything_is_refused(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    invoices(store.as_ref(), 0, 1).await?;
    let dir = scratch.dir("full");
    fs::create_dir_all(&dir).map_err(|e| Failure::new(format!("creating {dir:?}: {e}")))?;
    fs::write(dir.join("notes.txt"), b"kept\n")
        .map_err(|e| Failure::new(format!("writing into {dir:?}: {e}")))?;
    let said = export::write(store.as_ref(), &dir).await;
    let named = dir.display().to_string();
    ensure!(
        matches!(reason(&said), Some(Refusal::Precondition { message }) if message.contains(&named)),
        "an export into {named}, which holds a file, is refused as a failed precondition naming \
         it, and it answered {said:?}"
    );
    let left: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| Failure::new(format!("listing {dir:?}: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ensure!(
        left == ["notes.txt"] && read(&dir, "notes.txt")? == b"kept\n",
        "a refused export writes nothing, and the directory holds {left:?}"
    );
    Ok(())
}

/// Fails unless verifying `dir` is refused naming `position`.
fn refused_at(dir: &Path, position: Position, damage: &str) -> Result<(), Failure> {
    let said = export::verify(dir);
    let at = format!("at position {position}:");
    ensure!(
        matches!(&said, Err(Error::Unavailable { message }) if message.contains(&at)),
        "verifying an export with {damage} is refused naming position {position}, and it \
         answered {said:?}"
    );
    Ok(())
}

/// A file's lines, without their line feeds.
fn lines_of(dir: &Path, file: &str) -> Result<Vec<Vec<u8>>, Failure> {
    let bytes = read(dir, file)?;
    Ok(bytes
        .strip_suffix(b"\n")
        .unwrap_or(&bytes)
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect())
}

/// Writes `lines` as `file`, each with its line feed.
fn rewrite(dir: &Path, file: &str, lines: &[Vec<u8>]) -> Result<(), Failure> {
    let mut bytes = lines.join(&b'\n');
    bytes.push(b'\n');
    fs::write(dir.join(file), bytes).map_err(|e| Failure::new(format!("damaging {file}: {e}")))
}

/// `line` with `change` applied to its object and its hash computed
/// again, so only the change is wrong.
fn rehashed(line: &[u8], change: impl FnOnce(&mut Value)) -> Result<Vec<u8>, Failure> {
    let mut event: Value = serde_json::from_slice(line)
        .map_err(|e| Failure::new(format!("an exported line is not JSON: {e}")))?;
    change(&mut event);
    let members = event
        .as_object_mut()
        .ok_or_else(|| Failure::new("an exported line is an object".to_owned()))?;
    members.remove("hash");
    let hash = canonical::sha256_hex(&event);
    event["hash"] = json!(hash);
    Ok(canonical::to_vec(&event))
}

async fn a_damaged_export_is_refused_naming_the_position(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = Scratch::new(scratch);
    // A re-key, so the last events are format 2; a checkpoint, so two files.
    let l = rekey::lake(store.as_ref(), rekey::IBANS).await?;
    rekey::apply(store.as_ref()).await?;
    let sealed_at = store.checkpoint().await.must("checkpointing")?;
    for holder in ["after", "later"] {
        l.upsert("account", "DE2", json!({"holder": holder}))
            .await?;
    }
    let head = store.head().await.must("reading the head")?;
    let (first, active) = (
        export::file_name(Position::GENESIS),
        export::file_name(sealed_at),
    );
    let last = (head.0 - sealed_at.0) as usize;
    let fresh = |what: &str| {
        let dir = scratch.dir(what);
        let store = store.clone();
        async move {
            write(store.as_ref(), &dir).await?;
            Ok::<PathBuf, Failure>(dir)
        }
    };

    // One changed byte: the writer of the event at position 2.
    let dir = fresh("byte").await?;
    let mut lines = lines_of(&dir, &first)?;
    let marker = b"\"writer\":\"";
    let Some(at) = lines[2].windows(marker.len()).position(|w| w == marker) else {
        return Err(Failure::new("an event line names its writer".to_owned()));
    };
    lines[2][at + marker.len()] ^= 0x01;
    rewrite(&dir, &first, &lines)?;
    refused_at(&dir, Position(2), "one changed byte at position 2")?;

    // A space inserted: the value and its hash unchanged, the line not canonical.
    let dir = fresh("space").await?;
    let mut lines = lines_of(&dir, &first)?;
    lines[2].insert(1, b' ');
    rewrite(&dir, &first, &lines)?;
    refused_at(&dir, Position(2), "a space inserted at position 2")?;

    // The last line feed missing.
    let dir = fresh("feed").await?;
    let bytes = read(&dir, &active)?;
    fs::write(dir.join(&active), &bytes[..bytes.len() - 1])
        .map_err(|e| Failure::new(format!("damaging {active}: {e}")))?;
    refused_at(&dir, head, "its last line feed missing")?;

    // The first event's format version below the first format, its hash
    // made right.
    let dir = fresh("v0").await?;
    let mut lines = lines_of(&dir, &first)?;
    lines[0] = rehashed(&lines[0], |e| e["v"] = json!(0))?;
    rewrite(&dir, &first, &lines)?;
    refused_at(&dir, Position::GENESIS, "its first event in format 0")?;

    // The last event's format version past the newest, and down from the
    // one before it; each with its hash made right.
    let past = lakelands_api::event::LOG_FORMAT_VERSION + 1;
    for (what, v) in [("past", past), ("v1", 1)] {
        let dir = fresh(what).await?;
        let mut lines = lines_of(&dir, &active)?;
        lines[last] = rehashed(&lines[last], |e| e["v"] = json!(v))?;
        rewrite(&dir, &active, &lines)?;
        refused_at(
            &dir,
            head,
            &format!("its last event in format {v} after format 2"),
        )?;
    }

    // The last event's known_at before the one before it (log-format
    // §1.3), its hash made right.
    let dir = fresh("known_at").await?;
    let mut lines = lines_of(&dir, &active)?;
    lines[last] = rehashed(&lines[last], |e| {
        e["known_at"] = json!("2000-01-01T00:00:00.000000Z");
    })?;
    rewrite(&dir, &active, &lines)?;
    refused_at(&dir, head, "its last event's known_at going back")?;

    // A checkpoint naming the wrong position before it, its hash made right.
    let dir = fresh("through").await?;
    let mut lines = lines_of(&dir, &active)?;
    lines[0] = rehashed(&lines[0], |e| {
        e["body"]["through"] = json!(sealed_at.0.saturating_sub(2));
    })?;
    rewrite(&dir, &active, &lines)?;
    refused_at(
        &dir,
        sealed_at,
        "a checkpoint naming the wrong position before it",
    )?;

    // A missing file: the sealed segment is gone.
    let dir = fresh("missing").await?;
    fs::remove_file(dir.join(&first))
        .map_err(|e| Failure::new(format!("removing {first}: {e}")))?;
    refused_at(&dir, Position::GENESIS, "its first file missing")?;

    // Every file gone: log/ is empty.
    let dir = fresh("empty").await?;
    for file in [&first, &active] {
        fs::remove_file(dir.join(file))
            .map_err(|e| Failure::new(format!("removing {file}: {e}")))?;
    }
    refused_at(&dir, Position::GENESIS, "an empty log/")?;

    // A file too many, named by a position that does not come next.
    let dir = fresh("extra").await?;
    fs::copy(
        dir.join(&first),
        dir.join(export::file_name(Position(head.0 + 5))),
    )
    .map_err(|e| Failure::new(format!("copying {first}: {e}")))?;
    refused_at(&dir, head.next(), "a file too many after the last")?;

    // A file named by its first position in fewer than 20 digits.
    let dir = fresh("name").await?;
    let short = format!("log/{}.jsonl", sealed_at.0);
    fs::rename(dir.join(&active), dir.join(&short))
        .map_err(|e| Failure::new(format!("renaming {active}: {e}")))?;
    refused_at(&dir, sealed_at, "a file named without its 20 digits")?;

    // Two lines out of order in the active file.
    let dir = fresh("order").await?;
    let mut lines = lines_of(&dir, &active)?;
    lines.swap(1, 2);
    rewrite(&dir, &active, &lines)?;
    refused_at(
        &dir,
        sealed_at.next(),
        "the two lines after the checkpoint swapped",
    )?;
    Ok(())
}

/// The seams an imported lake is opened with: the suite's backings, as
/// the fresh lake of every check has them.
fn import_seams() -> lakelands_api::seam::Seams {
    lakelands_api::seam::Seams {
        backings: crate::conformance::backing::backings(),
        executors: crate::conformance::backing::executors(),
        ..lakelands_api::seam::Seams::default()
    }
}

/// A lake with two writers, two sealed segments and seven format
/// versions: invoices, a checkpoint, the lake opened again by a second
/// writer (a second lease, a greater fence), a re-key (version 2), a
/// restricted and a retired invoice, an annotated type with a derived
/// relation (version 6) and a derived `date_bucket` field, a trigger whose
/// guard calls `retired` (version 7), a view with a parameter and a verb
/// called on its type (version 8), a view that merges a second source
/// (version 9), a record ingested with an upstream,
/// a second checkpoint and more invoices.
/// Answers the
/// second writer's handle and a position before the second checkpoint.
async fn exported_lake(
    store: Arc<dyn LakeStore>,
    reopen: &crate::conformance::Reopen,
) -> Result<(Arc<dyn LakeStore>, Position), Failure> {
    invoices(store.as_ref(), 0, 3).await?;
    store.checkpoint().await.must("checkpointing")?;
    let writer = reopen(store, import_seams())
        .await
        .map_err(|e| Failure::new(format!("opening the lake for a second writer: {e}")))?;
    let store = writer.as_ref();
    rekey::lake(store, rekey::IBANS).await?;
    rekey::apply(store).await?;
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    let invoice = lakelands_api::TypeName::from("invoice");
    store
        .restrict(&invoice, &"i1".into(), "held".into())
        .await
        .must("restricting `i1`")?;
    l.retire(invoice.as_str(), "i2").await?;
    // Format version 3: a derived field calling date_bucket, and a record
    // ingested with the lake and position it was read from. Format version
    // 5: the type, a field and a relation carry annotations. Format version
    // 6: a relation derived from matching values.
    l.declare_json(json!({
        "declare": "type", "name": "stamp",
        "annotations": {"acme.icon": "clock", "acme.package": {"name": "audit", "weight": 1.5}},
        "fields": [
            {"name": "at", "type": {"kind": "date_time"}, "authority": "source",
             "annotations": {"acme.format": "iso"}},
            {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
        ],
        "relations": [
            {"name": "of", "target": "invoice", "annotations": {"acme.plural": "invoices"}},
            {"name": "same_at", "target": "stamp", "derived": {"match": [{"from": "at", "to": "at"}]}}
        ]
    }))
    .await?;
    // Format version 7: a trigger whose guard calls `retired`.
    l.declare_json(json!({
        "declare": "trigger", "name": "stamp_retired", "on": "stamp", "when": "retired()"
    }))
    .await?;
    // Format version 8: a view with a parameter.
    l.declare_json(json!({
        "declare": "type", "name": "stamps_up_to", "key": ["month"],
        "fields": [{"name": "month", "type": {"kind": "date_time"}},
                   {"name": "stamps", "type": {"kind": "number"}}],
        "view": {"source": "stamp", "params": [{"name": "until", "kind": "date_time"}],
                 "where": "at <= param('until')", "group_by": ["month"],
                 "measures": [{"name": "stamps", "expr": "count()"}]}
    }))
    .await?;
    // Format version 9: a view that merges a second source on its key.
    l.declare_json(json!({
        "declare": "type", "name": "tick", "fields": [{"name": "at", "type": {"kind": "date_time"}}]
    }))
    .await?;
    l.declare_json(json!({
        "declare": "type", "name": "stamps_and_ticks", "key": ["month"],
        "fields": [{"name": "month", "type": {"kind": "date_time"}},
                   {"name": "stamps", "type": {"kind": "number"}},
                   {"name": "ticks", "type": {"kind": "number"}}],
        "view": {"source": "stamp", "group_by": ["month"],
                 "measures": [{"name": "stamps", "expr": "count()"}],
                 "merge": [{"source": "tick", "group_by": ["at"],
                            "measures": [{"name": "ticks", "expr": "count()"}]}]}
    }))
    .await?;
    // Format version 8 too: a verb on its type, called with no key.
    l.declare_json(json!({
        "declare": "verb", "name": "print_stamp", "on": "stamp", "scope": "type",
        "risk": "low", "effect": "external", "executor": crate::conformance::backing::EXECUTOR
    }))
    .await?;
    cat.call(
        own,
        &"print_stamp".into(),
        None,
        Default::default(),
        lakelands_api::WriteOpts::default(),
    )
    .await
    .must("calling `print_stamp` on its type")?;
    let mut read_from = crate::conformance::fixture::source_record(
        "stamp",
        "s1",
        json!({"at": "2028-02-10T08:00:00Z"}),
    );
    read_from.upstream = Some(lakelands_api::Upstream {
        lake: "upstream-lake".into(),
        position: Position(42),
    });
    crate::conformance::fixture::ingest(cat.as_ref(), own, "mirror", vec![read_from])
        .await
        .must("ingesting `s1` with an upstream")?;
    let earlier = store.head().await.must("reading the head")?;
    store.checkpoint().await.must("checkpointing")?;
    invoices(store, 3, 2).await?;
    Ok((writer, earlier))
}

/// What a lake answers, for comparing two: its head, `verify`, the feed
/// from position 0, `describe`, and every type's records with provenance
/// at the head and at `earlier`.
async fn answers(store: &dyn LakeStore, earlier: Position) -> Result<Value, Failure> {
    use lakelands_api::{Find, ReadAt};
    let own = owner(store);
    let cat = doors(store);
    let head = store.head().await.must("reading the head")?;
    let verified = store.verify(None, None).await.must("verifying")?;
    let feed: Vec<Value> = log_events(store)
        .await?
        .iter()
        .map(|e| serde_json::to_value(e).expect("an event serialises"))
        .collect();
    let described = cat
        .describe(&own, ReadAt::default())
        .await
        .must("describing the lake")?
        .data;
    let mut records = serde_json::Map::new();
    for t in &described.types {
        for (at, label) in [(None, "head"), (Some(earlier), "earlier")] {
            let found = cat
                .find(
                    &own,
                    &t.name,
                    Find {
                        limit: Some(1_000),
                        ..find_at(at)
                    },
                )
                .await;
            let found = match found {
                Ok(r) => serde_json::to_value(&r.data).expect("an answer serialises"),
                Err(e) => json!(format!("refused: {e}")),
            };
            records.insert(format!("{} at {label}", t.name), found);
        }
    }
    Ok(json!({
        "head": head.0,
        "verify": [verified.through.0, verified.hash.0],
        "feed": feed,
        "describe": serde_json::to_value(&described).expect("a description serialises"),
        "records": records,
    }))
}

async fn an_export_imported_into_a_fresh_lake_answers_as_the_lake_it_came_from(
    store: Arc<dyn LakeStore>,
    given: crate::conformance::Given,
) -> Result<(), Failure> {
    let scratch = Scratch::new(given.scratch.clone());
    let (store, earlier) = exported_lake(store, &given.reopen).await?;
    let newest = log_events(store.as_ref())
        .await?
        .iter()
        .map(|e| e.v)
        .max()
        .unwrap_or(0);
    ensure!(
        newest == lakelands_api::event::LOG_FORMAT_VERSION,
        "the exported lake must hold an event of the newest format version, {}; its newest is \
         {newest}",
        lakelands_api::event::LOG_FORMAT_VERSION
    );
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let lake = store.lake().clone();
    let crate::conformance::ImportedLake {
        store: imported,
        writer,
    } = (given.import)(from.clone(), lake.clone(), None, import_seams())
        .await
        .map_err(|e| Failure::new(format!("importing the export into a fresh lake: {e}")))?;
    let (want, got) = (
        answers(store.as_ref(), earlier).await?,
        answers(imported.as_ref(), earlier).await?,
    );
    if let (Some(w), Some(g)) = (want["records"].as_object(), got["records"].as_object()) {
        for (what, answer) in w {
            ensure!(
                g.get(what) == Some(answer),
                "the imported lake must find `{what}` as the lake it came from; it answered {}, \
                 and the lake {answer}",
                g.get(what).unwrap_or(&Value::Null)
            );
        }
    }
    for part in ["head", "verify", "feed", "describe", "records"] {
        ensure!(
            want[part] == got[part],
            "the imported lake must answer `{part}` as the lake it came from; it answered \
             {}, and the lake {}",
            got[part],
            want[part]
        );
    }
    let again = scratch.dir("again");
    write(imported.as_ref(), &again).await?;
    let (files_from, files_again) = (files(&from)?, files(&again)?);
    ensure!(
        files_from == files_again,
        "the imported lake must export the same files; it exported {files_again:?}, and the \
         lake {files_from:?}"
    );
    for f in &files_from {
        ensure!(
            read(&from, f)? == read(&again, f)?,
            "the imported lake's {f} must hold the same bytes as the lake's"
        );
    }
    // The next writer continues the lake above every fence in it.
    let highest = log_events(imported.as_ref())
        .await?
        .iter()
        .map(|e| e.fence)
        .max()
        .unwrap_or(0);
    let writer = writer(imported, import_seams())
        .await
        .map_err(|e| Failure::new(format!("opening the imported lake for writing: {e}")))?;
    invoices(writer.as_ref(), 5, 1).await?;
    let log = log_events(writer.as_ref()).await?;
    let taken = log.iter().rev().find_map(|e| match &e.body {
        EventBody::LeaseTaken { fence, .. } => Some(*fence),
        _ => None,
    });
    ensure!(
        taken.is_some_and(|f| f > highest),
        "the imported lake's next writer must take a lease greater than every fence in the \
         log ({highest}); it took {taken:?}"
    );
    writer
        .verify(None, None)
        .await
        .must("verifying the imported lake after a write")?;
    Ok(())
}

async fn an_import_is_refused_for_a_damaged_export_another_lake_or_a_lake_holding_events(
    store: Arc<dyn LakeStore>,
    given: crate::conformance::Given,
) -> Result<(), Failure> {
    let scratch = Scratch::new(given.scratch.clone());
    let (store, _) = exported_lake(store, &given.reopen).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let lake = store.lake().clone();
    // Refused as asked: the caller's precondition, not a lake that may
    // answer later.
    let precondition = |e: &Error, what: &str| {
        ensure!(
            matches!(reason(&e), Some(Refusal::Precondition { .. })),
            "{what} must be refused as a failed precondition; it was refused as {e:?}"
        );
        Ok(())
    };
    // One byte changed in the first segment's third line.
    let damaged = scratch.dir("damaged");
    fs::create_dir_all(damaged.join("log"))
        .map_err(|e| Failure::new(format!("making the damaged export: {e}")))?;
    for f in files(&from)? {
        fs::copy(from.join(&f), damaged.join(&f))
            .map_err(|e| Failure::new(format!("copying {f}: {e}")))?;
    }
    let first = files(&damaged)?.remove(0);
    let mut lines = lines_of(&damaged, &first)?;
    let at = lines[2]
        .iter()
        .position(|b| *b == b'1')
        .ok_or_else(|| Failure::new("the third line holds no digit 1 to change"))?;
    lines[2][at] = b'2';
    rewrite(&damaged, &first, &lines)?;
    let why = refused(
        (given.import)(damaged, lake.clone(), None, import_seams()).await,
        "an export with a byte changed at position 2",
    )?
    .to_string();
    ensure!(
        why.contains("position 2"),
        "an export with a byte changed at position 2 must be refused naming that position; it \
         was refused with: {why}"
    );
    let other = lakelands_api::LakeId::from("another-lake");
    let e = refused(
        (given.import)(from.clone(), other, None, import_seams()).await,
        "the export imported as another lake",
    )?;
    precondition(&e, "the export imported as another lake")?;
    let why = e.to_string();
    ensure!(
        why.contains(lake.as_str()) && why.contains("another-lake"),
        "an export imported as another lake must be refused naming both lakes; it was refused \
         with: {why}"
    );
    let head = store.head().await.must("reading the head")?;
    let e = refused(
        (given.import)(from, lake.clone(), Some(store), import_seams()).await,
        "an import into the lake the export came from, which holds events",
    )?;
    precondition(&e, "an import into a lake that holds events")?;
    let why = e.to_string();
    ensure!(
        why.contains(&format!("position {head}")),
        "an import into a lake holding events through {head} must be refused naming its head; \
         it was refused with: {why}"
    );
    Ok(())
}

async fn an_import_refuses_a_declaration_member_it_does_not_know_naming_it(
    store: Arc<dyn LakeStore>,
    given: crate::conformance::Given,
) -> Result<(), Failure> {
    let scratch = Scratch::new(given.scratch.clone());
    invoices(store.as_ref(), 0, 1).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let file = files(&from)?.remove(0);
    let mut lines = lines_of(&from, &file)?;
    let at = lines
        .iter()
        .position(|l| {
            serde_json::from_slice::<Value>(l).is_ok_and(|e| {
                e["body"]["kind"] == "declared" && e["body"]["declaration"]["name"] == "invoice"
            })
        })
        .ok_or_else(|| Failure::new("the export holds no `declared` event of `invoice`"))?;
    lines[at] = rehashed(&lines[at], |e| {
        e["body"]["declaration"]["icon"] = json!("receipt");
    })?;
    rewrite(&from, &file, &lines)?;
    let imported = (given.import)(from, store.lake().clone(), None, import_seams()).await;
    let what = "an export whose declaration of `invoice` carries `icon`";
    let why = refused(imported, what)?.to_string();
    ensure!(
        why.contains("`icon`") && why.contains("`invoice`"),
        "an export whose declaration of `invoice` carries `icon` must be refused naming the \
         member and the declaration; it was refused with: {why}"
    );
    Ok(())
}

async fn an_import_refuses_a_context_carrying_cause_naming_it(
    store: Arc<dyn LakeStore>,
    given: crate::conformance::Given,
) -> Result<(), Failure> {
    let scratch = Scratch::new(given.scratch.clone());
    invoices(store.as_ref(), 0, 1).await?;
    let written = log_events(store.as_ref()).await?;
    ensure!(
        written.iter().all(|e| e.context.cause.is_none()),
        "no event carries `cause` in its `context`"
    );
    let upserted = written
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordUpserted { .. }))
        .ok_or_else(|| Failure::new("the lake holds a `record_upserted` event"))?;
    ensure!(
        upserted.cause.is_none(),
        "a write through the doors carries no cause; `record_upserted` at {} carries {:?}",
        upserted.position,
        upserted.cause
    );
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let file = files(&from)?.remove(0);
    let mut lines = lines_of(&from, &file)?;
    let at = upserted.position.0 as usize;
    lines[at] = rehashed(&lines[at], |e| {
        e["context"]["cause"] = json!(0);
    })?;
    rewrite(&from, &file, &lines)?;
    let imported = (given.import)(from, store.lake().clone(), None, import_seams()).await;
    let why = refused(imported, "an export whose `context` carries `cause`")?.to_string();
    ensure!(
        why.contains("`context` carries `cause`"),
        "an export whose `context` carries `cause` must be refused naming it; it was refused \
         with: {why}"
    );
    Ok(())
}

/// The error an import was refused with, or a failure naming `what` if
/// it was imported.
fn refused(
    e: Result<crate::conformance::ImportedLake, Error>,
    what: &str,
) -> Result<Error, Failure> {
    e.map(|_| ())
        .err()
        .ok_or_else(|| Failure::new(format!("{what} must be refused; it was imported")))
}

/// A view over `source`, counting its records by `k`.
fn tally_view(name: &str, source: &str) -> lakelands_api::ontology::TypeDecl {
    count_view(name, source, "k")
}

async fn a_cycle_of_views_from_an_imported_log_is_refused_on_read(
    store: Arc<dyn LakeStore>,
    given: crate::conformance::Given,
) -> Result<(), Failure> {
    let scratch = Scratch::new(given.scratch.clone());
    let l = Lake::of(store.as_ref());
    let mut tally = type_of("tally", &[("k", Text)]);
    tally.key = vec!["k".into()];
    l.declare(tally).await?;
    l.upsert("tally", "x", json!({"k": "x"})).await?;
    l.declare(tally_view("view_a", "tally")).await?;
    l.declare(tally_view("view_b", "view_a")).await?;
    l.declare(tally_view("view_c", "tally")).await?;
    // The last event declares `view_a` again; the export's copy of it is
    // made to read `view_b`, closing a cycle no door would declare.
    let mut again = tally_view("view_a", "tally");
    again.description = Some("view_a, declared again".into());
    l.declare(again).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let file = files(&from)?
        .pop()
        .ok_or_else(|| Failure::new("the export holds no file"))?;
    let mut lines = lines_of(&from, &file)?;
    let last = lines
        .last_mut()
        .ok_or_else(|| Failure::new("the export's last file holds no line"))?;
    let mut failure = None;
    *last = rehashed(last, |e| {
        if e["body"]["kind"] != "declared" || e["body"]["declaration"]["name"] != "view_a" {
            failure = Some("the export's last event does not declare `view_a`");
            return;
        }
        e["body"]["declaration"]["view"]["source"] = json!("view_b");
        match serde_json::from_value::<lakelands_api::write::Declaration>(
            e["body"]["declaration"].clone(),
        )
        .ok()
        .and_then(|d| match d {
            lakelands_api::write::Declaration::Type(t) => {
                lakelands_api::shape::schema_hash(&t).ok()
            }
            _ => None,
        }) {
            Some(hash) => e["body"]["schema_hash"] = json!(hash),
            None => failure = Some("the rewritten declaration has no schema hash"),
        }
    })?;
    if let Some(f) = failure {
        return Err(Failure::new(f));
    }
    rewrite(&from, &file, &lines)?;
    let crate::conformance::ImportedLake {
        store: imported, ..
    } = (given.import)(from, store.lake().clone(), None, import_seams())
        .await
        .map_err(|e| Failure::new(format!("importing the export with the cycle: {e}")))?;
    let cat = doors(imported.as_ref());
    let own = owner(imported.as_ref());
    for view in ["view_a", "view_b"] {
        let r = cat
            .find(&own, &view.into(), lakelands_api::read::Find::default())
            .await;
        ensure!(
            matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains("`view_a`") && message.contains("`view_b`")),
            "`{view}` reads itself through the other view in the imported log, so a read of it \
             must be refused as invalid, naming both; it answered {r:?}"
        );
    }
    let off = cat
        .find(&own, &"view_c".into(), lakelands_api::read::Find::default())
        .await
        .must("reading `view_c`, which is off the cycle")?;
    ensure!(
        matches!(&off.data, lakelands_api::Found::Records { rows, .. } if rows.len() == 1),
        "`view_c` is off the cycle and counts the one tally; it answered {:?}",
        off.data
    );
    Ok(())
}
