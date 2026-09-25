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

use lakelands_api::event::{Event, EventBody};
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Error, LakeStore, Position, Refusal};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::rekey;
use crate::canonical;
use crate::conformance::Failure;
use crate::conformance::fixture::{Must, doors, field, owner, type_decl, upsert};
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
    "030 and log-format §2.6 and §4.2: verifying an export refuses a changed byte, a space inserted, a missing last line feed, a format version below the first, past the newest or going down, a checkpoint naming the wrong position before it, a missing file, an empty log/, a file too many, a file not named in 20 digits and two lines out of order, and names the position where the files stop being the log",
    scratch
);

/// The check's scratch directory, cleared of anything an earlier run
/// left, and removed again when the check ends, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(base: PathBuf) -> Scratch {
        let _ = fs::remove_dir_all(&base);
        Scratch(base)
    }

    /// A directory for one export, not yet created.
    fn dir(&self, what: &str) -> PathBuf {
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

async fn events(store: &dyn LakeStore) -> Result<Vec<Event>, Failure> {
    Ok(store
        .feed()
        .poll(Position::GENESIS, &FeedFilter::default(), 10_000)
        .await
        .must("reading the log")?
        .events)
}

/// A lake with an `invoice` type and `n` invoices written.
async fn invoices(store: &dyn LakeStore, from: u32, n: u32) -> Result<Position, Failure> {
    let cat = doors(store);
    let own = owner(store);
    if from == 0 {
        crate::conformance::fixture::declare(
            cat.as_ref(),
            &own,
            type_decl(
                "invoice",
                vec![field(
                    "amount",
                    lakelands_api::ontology::StorageKind::Number,
                )],
            ),
        )
        .await?;
    }
    let mut at = Position::GENESIS;
    for i in from..from + n {
        at = upsert(
            cat.as_ref(),
            &own,
            "invoice",
            &format!("i{i}"),
            json!({"amount": i}),
        )
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
    let all = events(store.as_ref()).await?;
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
    for e in events(store.as_ref()).await? {
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
        matches!(&said, Err(Error::Refused { reason: Refusal::Precondition { message }, .. })
            if message.contains(&named)),
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
    rekey::lake(store.as_ref(), rekey::IBANS).await?;
    rekey::apply(store.as_ref()).await?;
    let sealed_at = store.checkpoint().await.must("checkpointing")?;
    let cat = doors(store.as_ref());
    for holder in ["after", "later"] {
        upsert(
            cat.as_ref(),
            &owner(store.as_ref()),
            "account",
            "DE2",
            json!({"holder": holder}),
        )
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
    for (what, v) in [("v3", 3), ("v1", 1)] {
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
