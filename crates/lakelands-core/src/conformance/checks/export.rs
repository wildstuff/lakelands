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

use lakelands_api::error::{Case, Flaw, Situation};
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::principal::PrincipalKind;
use lakelands_api::{Error, LakeStore, Position, Refusal};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::rekey;
use crate::canonical;
use crate::conformance::fixture::{
    Lake, Must, computed_by, count_view, doors, find_at, invalid_case, log_events, owner, reason,
    type_of,
};
use crate::conformance::{Failure, Held, Store};
use crate::export::{self, Manifest};

check!(
    ONE_FILE_PER_SEGMENT = an_export_writes_one_file_per_segment_of_canonical_lines,
    "030 and log-format §4.2: an export writes one file per segment, named by its first position in 20 digits, holding each event's canonical bytes and a line feed, and the sealed file's digest is the one the checkpoint after it lists",
    scratch
);

check!(
    VERIFIES = an_export_verifies_to_the_through_and_hash_the_lake_answers,
    "030: an export verifies from its files alone to the position and hash the lake's own verify answers; its manifest names the same pair and every file with its digest, and is never trusted: with a wrong manifest or none the export verifies the same",
    scratch
);

check!(
    PREFIX = a_later_export_repeats_the_sealed_files_and_extends_the_active_one,
    "030: an export reads through the head as it stood when it began; a lake never checkpointed exports one file 00000000000000000000.jsonl, and a later export has the earlier sealed files byte for byte and an active file the earlier one begins",
    scratch
);

check!(
    NOT_EMPTY = an_export_into_a_directory_that_holds_anything_is_refused,
    "030: an export writes only into an empty or absent directory; into any other it is refused as a failed precondition, the export's directory not empty (no path on the host is named), and writes nothing",
    scratch
);

check!(
    DAMAGE = a_damaged_export_is_refused_naming_the_position,
    "030 and log-format §2.6 and §4.2: verifying an export refuses a changed byte, a space inserted, a missing last line feed, a known_at going back, a checkpoint naming the wrong position before it, a missing file, an empty log/, a file too many, a file not named in 20 digits and two lines out of order, and names the position where the files stop being the log",
    scratch
);

check!(
    IMPORT_IS_THE_LAKE = an_export_imported_into_a_fresh_lake_answers_as_the_lake_it_came_from,
    "030 and log-format §4: an export imported into a fresh lake of the engine holds every event unchanged at its position (annotations included), a value type's renames among them, so the lake answers as the one exported (the head, verify, the feed from position 0, every type's records with provenance at the head and at an earlier position, describe) and exports the same files byte for byte; its next writer takes a lease greater than every fence in the log and continues it",
    given
);

check!(
    IMPORT_REFUSED =
        an_import_is_refused_for_a_damaged_export_another_lake_or_a_lake_holding_events,
    "030: an import verifies the whole export before it writes: a changed byte in a segment is refused naming its position, and an export of another lake (naming both lakes) and a lake that holds events (naming its head) are refused as failed preconditions; nothing is written into the lake it refuses",
    given
);

check!(
    ANOTHER_RELEASES =
        a_lake_another_release_wrote_is_not_opened_and_its_event_is_refused_where_it_is_read,
    "054: a lake whose newest event another release wrote, in the format before the release's or the one after, is refused as `log_format` at an open to read, to write and to rebuild, naming the lake and both formats, and is left as it was: its tip, the position its projection is folded through and its free lease; once a writer of the release's format follows, the lake opens to read, and the event of another format is refused as `format_other` at its position by `verify`, by a writer's open and by a rebuild, with the tip where it was and the lease free",
    given
);

check!(
    PROJECTION_AHEAD = a_projection_folded_past_its_log_opens_only_for_a_rebuild,
    "054 and lakelands#865: a projection folded past its log's tip stands for events the log does not hold. Committed two positions ahead through the engine's seam, the lake is refused as `damaged`, the part the log and the flaw `ends_before` naming the position the projection is folded through, at an open to read, to write and to create, and is left as it was: its tip, that position and its free lease, so the writers appended nothing. An open to rebuild makes the projection from the log as it stands, and the lake answers the record it held",
    given
);

check!(
    ONE_FORMAT = a_lake_is_written_in_one_format_and_another_is_refused_whole,
    "054 and log-format §6: a lake that declares, writes, migrates, re-keys, proposes, ingests and seals a segment holds only events in the release's one format version, from position 0, with `lake_created.format` the same, and its export says so and verifies; an export with one event in another format, older or newer, at position 0 or at its head, its hash made right, is refused by verify naming that position and both formats, and an import of it is refused in the same words; so is one whose head carries no `v`, which is format 0, and one whose `lake_created` says another format than its line, by a flaw of its own",
    given
);

check!(
    IMPORT_UNKNOWN_MEMBER = an_import_refuses_a_declaration_member_it_does_not_know_naming_it,
    "028 and log-format §2: an export whose `declared` event carries a member its declaration does not take, its hash made right, is refused on import, naming the member and the declaration",
    given
);

check!(
    IMPORTED_VIEW_CYCLE = a_cycle_of_views_from_an_imported_log_is_refused_on_read,
    "018 and 030: a log can bring in a cycle of views over `source` that `declare` would have refused; a read of a view on it is refused as invalid, naming the chain, the process goes on, and a view off the cycle reads as before, while one that reads into the cycle is refused naming it too, alike for the owner, a holder of `only` and a reader without `restricted`, each of whom is described the lake (lakelands#942)",
    given
);

check!(
    IMPORT_CONTEXT_CAUSE = an_import_refuses_a_context_carrying_cause_naming_it,
    "021, extended, and log-format §1.1: a host's cause is the envelope's and never written inside `context`; an export whose line carries `cause` in its `context`, its hash made right, is refused on import naming the member, and a write through the doors carries no cause; so is one whose envelope carries the origin `initial`, a field's provenance alone (050, lakelands#551)",
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
    store: Store,
    scratch: PathBuf,
) -> Held {
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
    store: Store,
    scratch: PathBuf,
) -> Held {
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

async fn a_later_export_repeats_the_sealed_files_and_extends_the_active_one(
    store: Store,
    scratch: PathBuf,
) -> Held {
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
    store: Store,
    scratch: PathBuf,
) -> Held {
    let scratch = Scratch::new(scratch);
    invoices(store.as_ref(), 0, 1).await?;
    let dir = scratch.dir("full");
    fs::create_dir_all(&dir).map_err(|e| Failure::new(format!("creating {dir:?}: {e}")))?;
    fs::write(dir.join("notes.txt"), b"kept\n")
        .map_err(|e| Failure::new(format!("writing into {dir:?}: {e}")))?;
    let said = export::write(store.as_ref(), &dir).await;
    ensure!(
        matches!(reason(&said), Some(Refusal::Precondition(said)) if said.case == Case::ExportNotEmpty),
        "an export into {dir:?}, which holds a file, is refused as a failed precondition, the \
         export's directory not empty, and it answered {said:?}"
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
fn refused_at(dir: &Path, position: Position, damage: &str) -> Held {
    let said = export::verify(dir);
    ensure!(
        matches!(said.as_ref().err().and_then(Error::situation), Some(Situation::Damaged { position: Some(p), .. })
            if *p == position),
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
fn rewrite(dir: &Path, file: &str, lines: &[Vec<u8>]) -> Held {
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

async fn a_damaged_export_is_refused_naming_the_position(store: Store, scratch: PathBuf) -> Held {
    let scratch = Scratch::new(scratch);
    // A re-key among the events; a checkpoint, so two files.
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

/// A lake with two writers and two sealed segments: invoices, a
/// checkpoint, the lake opened again by a second writer (a second lease,
/// a greater fence), a re-key, a restricted and a retired invoice, an
/// annotated type with a derived relation and a derived `date_bucket`
/// field, a trigger whose guard calls `retired`, a view with a parameter
/// and a verb called on its type, a view that merges a second source, a
/// proposed tightening of a value type, a derived field calling `floor`,
/// a record ingested with an upstream, a set of declarations and a value
/// type's renames, applied and proposed, a date written, a field
/// computed over two records sharing its input, whose second fill names
/// the first's output, a second checkpoint and more invoices. Answers
/// the second writer's handle and a position before the second
/// checkpoint.
async fn exported_lake(
    store: Store,
    reopen: &crate::conformance::Reopen,
) -> Result<(Store, Position), Failure> {
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
    // A derived field calling date_bucket, and a record ingested with
    // the lake and position it was read from. The type, a field and a
    // relation carry annotations. A relation derived from matching
    // values. A field that declares `initial`, filled when `s1` comes to
    // exist.
    l.declare_json(json!({
        "declare": "type", "name": "stamp",
        "annotations": {"acme.icon": "clock", "acme.package": {"name": "audit", "weight": 1.5}},
        "fields": [
            {"name": "at", "type": {"kind": "date_time"}, "authority": "source",
             "annotations": {"acme.format": "iso"}},
            {"name": "seen", "type": {"kind": "boolean"}, "initial": false},
            {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
        ],
        "relations": [
            {"name": "of", "target": "invoice", "annotations": {"acme.plural": "invoices"}},
            {"name": "same_at", "target": "stamp", "derived": {"match": [{"from": "at", "to": "at"}]}}
        ]
    }))
    .await?;
    // A trigger whose guard calls `retired`.
    l.declare_json(json!({
        "declare": "trigger", "name": "stamp_retired", "on": "stamp", "when": "retired()"
    }))
    .await?;
    // A view with a parameter.
    l.declare_json(json!({
        "declare": "type", "name": "stamps_up_to", "key": ["month"],
        "fields": [{"name": "month", "type": {"kind": "date_time"}},
                   {"name": "stamps", "type": {"kind": "number"}}],
        "view": {"source": "stamp", "params": [{"name": "until", "kind": "date_time"}],
                 "where": "at <= param('until')", "group_by": ["month"],
                 "measures": [{"name": "stamps", "expr": "count()"}]}
    }))
    .await?;
    // A view that merges a second source on its key.
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
    // A proposed tightening of a value type.
    let state = |one_of: Value| json!({"name": "stamp_state", "kind": "text", "one_of": one_of});
    let wide = serde_json::from_value(state(json!(["new", "seen"]))).expect("a value type");
    let declared = lakelands_api::write::Declaration::ValueType(wide);
    let fut = cat.declare(own, declared);
    fut.await.must("declaring `stamp_state`")?;
    let narrower = lakelands_api::write::ProposedWrite::MigrateValueType {
        to: serde_json::from_value(state(json!(["new"]))).expect("a value type"),
        renames: Vec::new(),
        report: Default::default(),
    };
    let proposed = cat.propose(own, narrower, "one state".into()).await;
    proposed.must("proposing a narrower `stamp_state`")?;
    // A derived field calling `floor`, and one that says when it is
    // computed.
    let days = json!({"name": "days", "type": {"kind": "number"}, "authority": "derived",
                      "derived": "floor((now() - at) / 1d)", "computed_at": "write"});
    let days = serde_json::from_value(days).expect("a field");
    let added = cat.add_field(own, &"stamp".into(), days).await;
    added.must("adding `days` to `stamp`")?;
    // A proposal pinned to its proposer's own record.
    l.declare(type_of("chore", &[("by", Text)])).await?;
    l.upsert("chore", "c1", json!({"by": "kim"})).await?;
    let kim = json!({"default": "write", "types": {"chore": {"only": {"by": "kim"}}}});
    let kim = serde_json::from_value(kim).expect("grants");
    let kim = crate::conformance::fixture::principal(store, PrincipalKind::Agent, "kim", kim);
    let done = json!({"write": "retire", "type": "chore", "key": "c1"});
    let done = serde_json::from_value(done).expect("a write");
    let proposed = cat.propose(&kim, done, "done".into()).await;
    proposed.must("kim's proposal to retire her `c1`")?;
    // A verb on its type, called with no key.
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
    let fut = crate::conformance::fixture::ingest(cat.as_ref(), own, "mirror", vec![read_from]);
    fut.await.must("ingesting `s1` with an upstream")?;
    // A set that computes records again, a function
    // and the type in force whose field it computes, appended as one.
    let set = [
        json!({"declare": "function", "name": "tick_at", "backing": "expr", "expr": "at",
               "inputs": [{"name": "at", "type": {"kind": "date_time"}}]}),
        json!({"declare": "type", "name": "tick", "fields": [
            {"name": "at", "type": {"kind": "date_time"}},
            {"name": "again", "type": {"kind": "date_time"}, "authority": "derived",
             "function": "tick_at", "depends_on": ["at"]}]}),
    ];
    let set = set.map(|d| serde_json::from_value(d).expect("a declaration"));
    let applied = cat.apply(own, set.into()).await;
    applied.must("applying `tick_at` and `tick`")?;
    crate::conformance::fixture::two_makes(&l).await?;
    // A value type's values renamed in a record,
    // and a rename proposed.
    l.declare_json(json!({
        "declare": "type", "name": "stamp_mark",
        "fields": [{"name": "state", "type": {"kind": "text", "value_type": "stamp_state"}}]
    }))
    .await?;
    l.upsert("stamp_mark", "m1", json!({"state": "seen"}))
        .await?;
    let renamed = |one_of: Value, old: &str, new: &str| lakelands_api::write::ValueTypeMigration {
        to: serde_json::from_value(state(one_of)).expect("a value type"),
        renames: vec![(old.into(), new.into())],
    };
    let m = renamed(json!(["new", "checked"]), "seen", "checked");
    let report = cat.migrate_value_type(own, m.clone()).await;
    let id = report.must("the preflight of a rename")?.data.report_id;
    let applied = cat.migrate_value_type_apply(own, m, &id).await;
    applied.must("renaming `seen`")?;
    let back = renamed(json!(["new", "seen"]), "checked", "seen");
    let proposed = lakelands_api::write::ProposedWrite::MigrateValueType {
        to: Box::new(back.to),
        renames: back.renames,
        report: Default::default(),
    };
    let proposed = cat.propose(own, proposed, "back".into()).await;
    proposed.must("proposing a rename")?;
    // A field of the kind `date`, and a date written.
    l.declare(type_of(
        "due",
        &[("on", lakelands_api::ontology::StorageKind::Date)],
    ))
    .await?;
    l.upsert("due", "d1", json!({"on": "2026-07-05"})).await?;
    // A field computed over two records that share its
    // input; the sweep's second fill names the first one's output.
    l.declare_json(json!({
        "declare": "function", "name": "echo", "backing": crate::conformance::backing::KIND,
        "inputs": [{"name": "word", "type": {"kind": "text"}}],
        "config": {"token": "exported_named_output", "echo": "word"}
    }))
    .await?;
    l.declare(type_of("said", &[("word", Text)])).await?;
    for key in ["w1", "w2"] {
        l.upsert("said", key, json!({"word": "same"})).await?;
    }
    let echoed = computed_by("echoed", Text, "echo", &["word"]);
    let added = cat.add_field(own, &"said".into(), echoed).await;
    added.must("adding `echoed` to `said`")?;
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
    let described = cat.describe(&own, ReadAt::default());
    let described = described.await.must("describing the lake")?.data;
    let mut records = serde_json::Map::new();
    for t in &described.types {
        for (at, label) in [(None, "head"), (Some(earlier), "earlier")] {
            let found = cat.find(
                &own,
                &t.name,
                Find {
                    limit: Some(1_000),
                    ..find_at(at)
                },
            );
            let found = found.await;
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
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    let (store, earlier) = exported_lake(store, &given.reopen).await?;
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
    let highest = log_events(imported.as_ref());
    let highest = highest.await?.iter().map(|e| e.fence).max().unwrap_or(0);
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
    let fut = writer.verify(None, None);
    fut.await
        .must("verifying the imported lake after a write")?;
    Ok(())
}

async fn an_import_is_refused_for_a_damaged_export_another_lake_or_a_lake_holding_events(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    let (store, _) = exported_lake(store, &given.reopen).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let lake = store.lake().clone();
    // Refused as asked: the caller's precondition, not a lake that may
    // answer later.
    let precondition = |e: &Error, what: &str| {
        ensure!(
            matches!(reason(&e), Some(Refusal::Precondition(..))),
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
    )?;
    ensure!(
        matches!(why.situation(), Some(Situation::Damaged { position: Some(p), .. }) if p.0 == 2),
        "an export with a byte changed at position 2 must be refused naming that position; it \
         was refused with: {why}"
    );
    let other = lakelands_api::LakeId::from("another-lake");
    let e = refused(
        (given.import)(from.clone(), other, None, import_seams()).await,
        "the export imported as another lake",
    )?;
    precondition(&e, "the export imported as another lake")?;
    ensure!(
        matches!(e.case().map(|s| &s.case), Some(Case::ImportOtherLake { held, lake: to, .. })
            if **held == lake && to.as_str() == "another-lake"),
        "an export imported as another lake must be refused naming both lakes; it was refused \
         with: {e:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let e = refused(
        (given.import)(from, lake.clone(), Some(store), import_seams()).await,
        "an import into the lake the export came from, which holds events",
    )?;
    precondition(&e, "an import into a lake that holds events")?;
    ensure!(
        matches!(e.case().map(|s| &s.case), Some(Case::ImportNotEmpty { head: h, .. }) if *h == head),
        "an import into a lake holding events through {head} must be refused naming its head; \
         it was refused with: {e:?}"
    );
    Ok(())
}

async fn an_import_refuses_a_declaration_member_it_does_not_know_naming_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
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
    let why = refused(imported, what)?;
    // No event: the parser's own words, the flaw's foreign detail, name the
    // member and the declaration; words compared on purpose, no case holds them.
    let named = matches!(damaged(&why), Some(Flaw::NotAnEvent { detail })
        if ["`icon`", "`invoice`"].iter().all(|n| detail.to_string().contains(n)));
    ensure!(
        named,
        "an export whose declaration of `invoice` carries `icon` must be refused naming the \
         member and the declaration; it was refused with: {why:?}"
    );
    Ok(())
}

async fn an_import_refuses_a_context_carrying_cause_naming_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
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
    let cases = [
        ("`context` carries `cause`", Flaw::CauseInContext),
        (
            "the envelope carries the origin `initial`",
            Flaw::InitialOrigin,
        ),
    ];
    for (n, (said, flaw)) in cases.into_iter().enumerate() {
        let from = scratch.dir(&format!("from{n}"));
        write(store.as_ref(), &from).await?;
        let file = files(&from)?.remove(0);
        let mut lines = lines_of(&from, &file)?;
        let at = upserted.position.0 as usize;
        lines[at] = rehashed(&lines[at], |e| match n {
            0 => e["context"]["cause"] = json!(0),
            _ => e["origin"] = json!("initial"),
        })?;
        rewrite(&from, &file, &lines)?;
        let imported = (given.import)(from, store.lake().clone(), None, import_seams()).await;
        let why = refused(imported, &format!("an export whose line {said}"))?;
        ensure!(
            damaged(&why) == Some(&flaw),
            "an export whose line {said} must be refused naming it; it was refused with: {why:?}"
        );
    }
    Ok(())
}

async fn a_lake_is_written_in_one_format_and_another_is_refused_whole(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let reads = lakelands_api::event::LOG_FORMAT_VERSION;
    let scratch = Scratch::new(given.scratch.clone());
    let (store, _) = exported_lake(store, &given.reopen).await?;
    let events = log_events(store.as_ref()).await?;
    let other: Vec<_> = (events.iter().filter(|e| e.v != reads))
        .map(|e| (e.position, e.v))
        .collect();
    ensure!(
        other.is_empty()
            && matches!(events.first().map(|e| &e.body),
                Some(EventBody::LakeCreated { format }) if *format == reads),
        "every event of the lake is in format {reads}, from position 0, and `lake_created` \
         says that format; these are not: {other:?}, and position 0 holds {:?}",
        events.first().map(|e| &e.body)
    );
    let from = scratch.dir("from");
    let manifest = write(store.as_ref(), &from).await?;
    ensure!(
        manifest.format == reads,
        "the export's manifest names format {reads}, and it names {}",
        manifest.format
    );
    export::verify(&from).must("verifying the export")?;
    let lake = store.lake().clone();
    let names = files(&from)?;
    let (first, active) = (names[0].clone(), names[names.len() - 1].clone());
    let head = store.head().await.must("reading the head")?;
    // What is changed in one line, its hash made right, and the flaw it
    // is refused by: an event of another format, older or newer, at
    // position 0 and at the head; a head with no `v`, which is format 0;
    // and a `lake_created` that says another format than its line.
    type Change = Box<dyn Fn(&mut serde_json::Value) + Send + Sync>;
    let other = |v: u32| Flaw::FormatOther {
        v: v.into(),
        reads: reads.into(),
    };
    let mut cases: Vec<(String, bool, Change, Flaw)> = Vec::new();
    // Older than the oldest format this release reads, and newer.
    let older = lakelands_api::event::OLDEST_LOG_FORMAT - 1;
    for v in [older, reads + 1] {
        for last in [false, true] {
            let what = format!("in format {v}");
            cases.push((what, last, Box::new(move |e| e["v"] = json!(v)), other(v)));
        }
        let (format, reads) = (v.into(), reads.into());
        let created = Flaw::CreatedInAnother { format, reads };
        let says: Change = Box::new(move |e| e["body"]["format"] = json!(v));
        cases.push((format!("created in format {v}"), false, says, created));
    }
    let bare: Change = Box::new(|e| {
        e.as_object_mut().map(|m| m.remove("v"));
    });
    cases.push(("with no `v`".into(), true, bare, other(0)));
    for (n, (what, last, change, want)) in cases.into_iter().enumerate() {
        let (file, at) = match last {
            true => (&active, head),
            false => (&first, Position::GENESIS),
        };
        let dir = scratch.dir(&format!("other-{n}"));
        fs::create_dir_all(dir.join("log"))
            .map_err(|e| Failure::new(format!("making the other export: {e}")))?;
        for f in &names {
            fs::copy(from.join(f), dir.join(f))
                .map_err(|e| Failure::new(format!("copying {f}: {e}")))?;
        }
        let mut lines = lines_of(&dir, file)?;
        let line = if last { lines.len() - 1 } else { 0 };
        lines[line] = rehashed(&lines[line], |e| change(e))?;
        rewrite(&dir, file, &lines)?;
        let verified = export::verify(&dir);
        let imported = (given.import)(dir, lake.clone(), None, import_seams()).await;
        let imported = refused(imported, "an export holding an event of another format")?;
        for (door, e) in [("verify", verified.err()), ("an import", Some(imported))] {
            ensure!(
                e.as_ref().and_then(damaged) == Some(&want)
                    && matches!(e.as_ref().and_then(Error::situation),
                        Some(Situation::Damaged { position: Some(p), .. }) if *p == at),
                "{door} of an export whose event at {at} is {what} is refused at that position \
                 as {want:?}, which names the format and the one this release reads; it \
                 answered {e:?}"
            );
        }
    }
    Ok(())
}

async fn a_lake_another_release_wrote_is_not_opened_and_its_event_is_refused_where_it_is_read(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use crate::conformance::Lines;
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::{Event, EventId};
    use lakelands_api::seam::Seams;
    let reads = lakelands_api::event::LOG_FORMAT_VERSION;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("note", &[("body", Text)])).await?;
    l.upsert("note", "n1", json!({"body": "x"})).await?;
    let lake = store.lake().clone();
    let events = log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let (Some(taken), Some(last)) = (events.iter().find(taken), events.last()) else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    // What another release's writer appends first: its `lease_taken`
    // after the tip, in its format `v`, the hash made right.
    let (template, known_at) = (taken.clone(), last.known_at);
    let line = move |v: u32| -> Lines {
        let template = template.clone();
        Box::new(move |(tip, prev), fence| {
            let (position, holder) = (tip.next(), "another release".into());
            let event = Event {
                v,
                position,
                id: EventId(format!("{:032x}", position.0)),
                prev,
                known_at,
                fence,
                body: EventBody::LeaseTaken { fence, holder },
                ..template
            };
            (vec![crate::conformance::fixture::canonical_line(event)], 0)
        })
    };
    let nothing = || -> Lines { Box::new(|_, _| (Vec::new(), 0)) };
    let mut held = Some(store);
    let mut first_other = None;
    let older = lakelands_api::event::OLDEST_LOG_FORMAT - 1;
    for other in [older, reads + 1] {
        let modes = [OpenMode::Read, OpenMode::Write, OpenMode::Rebuild];
        for (n, mode) in modes.into_iter().enumerate() {
            let lines = if n == 0 { line(other) } else { nothing() };
            let c = (given.continued)(held.take(), lake.clone(), lines, mode, Seams::default());
            let c = c
                .await
                .map_err(|e| Failure::new(format!("continuing the lake below the store: {e}")))?;
            let e = c.opened.err();
            ensure!(
                matches!(e.as_ref().and_then(Error::situation),
                    Some(Situation::LogFormat { lake: l, found, reads: r, .. })
                        if l == lake.as_str() && *found == u64::from(other) && *r == u64::from(reads)),
                "a lake whose newest event is in format {other} is refused at an open for \
                 {mode:?} as `log_format`, naming the lake, that format and the one this \
                 release reads, {reads}; it answered {e:?}"
            );
            ensure!(
                c.before.lease_free && c.after.as_ref() == Some(&c.before),
                "an open for {mode:?} refused for the lake's format leaves its tip, the \
                 position its projection is folded through and its free lease as they were, \
                 {:?}; they are {:?}",
                c.before,
                c.after
            );
            first_other = first_other.or(c.before.tip.map(|(at, _)| at));
        }
    }
    let at = first_other.ok_or_else(|| Failure::new("the lake has a tip"))?;
    // A writer of the release's format follows: the lake's ends are in
    // its format, and the events between are read where they are read.
    let c = (given.continued)(
        None,
        lake.clone(),
        line(reads),
        OpenMode::Read,
        Seams::default(),
    );
    let c = c
        .await
        .map_err(|e| Failure::new(format!("continuing the lake in the release's format: {e}")))?;
    let reader = c
        .opened
        .map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let refused = |e: Option<&Error>| {
        matches!(e.and_then(Error::situation), Some(Situation::Damaged {
            position: Some(p),
            flaw: Flaw::FormatOther { v, reads: r },
            ..
        }) if *p == at && *v == u64::from(older) && *r == u64::from(reads))
    };
    let verified = reader.verify(None, None).await;
    ensure!(
        refused(verified.as_ref().err()),
        "`verify` of a log that holds an event in format {} at {at} is refused as \
         `format_other` at that position, naming both formats; it answered {verified:?}",
        older
    );
    let tip = c.before.tip;
    held = Some(reader);
    for mode in [OpenMode::Write, OpenMode::Rebuild] {
        let c = (given.continued)(held.take(), lake.clone(), nothing(), mode, Seams::default());
        let c = c
            .await
            .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
        let (e, after) = (c.opened.err(), c.after);
        ensure!(
            refused(e.as_ref())
                && after.as_ref().map(|a| (&a.tip, a.lease_free)) == Some((&tip, true)),
            "an open for {mode:?}, whose fold reads the event in another format at {at}, is \
             refused as `format_other` at that position, the tip where it was ({tip:?}) and \
             the lease free; it answered {e:?}, and the lake is {after:?}"
        );
    }
    Ok(())
}

check!(
    INTEGER_DAMAGE =
        an_export_holding_an_integer_no_reader_reads_exactly_is_damaged_at_its_position,
    "the log format §1.2 and §4.3: a log holds no number written as an integer beyond ±(2^53 − 1), whatever wrote it, so an export whose event holds one (`9007199254740993` in a patch, the line's hash made right) is refused by its verification as damaged at that position, naming the number, and an import of it is refused the same way; the line holding 2^53 − 1 is not",
    given
);

async fn an_export_holding_an_integer_no_reader_reads_exactly_is_damaged_at_its_position(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    let at = invoices(store.as_ref(), 0, 1).await?;
    let too_large = |e: &Error| matches!(damaged(e), Some(Flaw::IntegerTooLarge { .. }));
    for (n, held) in [
        (9_007_199_254_740_991_u64, false),
        (9_007_199_254_740_993, true),
    ] {
        let from = scratch.dir(&format!("from{held}"));
        write(store.as_ref(), &from).await?;
        let file = files(&from)?.remove(0);
        let mut lines = lines_of(&from, &file)?;
        let line = at.0 as usize;
        lines[line] = rehashed(&lines[line], |e| e["body"]["patch"]["amount"] = json!(n))?;
        rewrite(&from, &file, &lines)?;
        let verified = export::verify(&from);
        let named = matches!(verified.as_ref().err().and_then(Error::situation),
            Some(Situation::Damaged { position: Some(p), flaw: Flaw::IntegerTooLarge { .. }, .. }) if *p == at);
        ensure!(
            named == held,
            "an export whose event at {at} holds the amount {n} must {} refused as damaged \
             there by that integer; its verification answered {verified:?}",
            if held { "be" } else { "not be" }
        );
        let imported = (given.import)(from, store.lake().clone(), None, import_seams()).await;
        let refusal = imported.map(|_| ()).err();
        ensure!(
            refusal.as_ref().is_some_and(too_large) == held,
            "an import of an export whose event at {at} holds the amount {n} must {} refused \
             as damaged by that integer; it answered {refusal:?}",
            if held { "be" } else { "not be" }
        );
    }
    Ok(())
}

async fn a_projection_folded_past_its_log_opens_only_for_a_rebuild(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use crate::conformance::Lines;
    use lakelands_api::engine::OpenMode;
    use lakelands_api::error::Part;
    use lakelands_api::seam::Seams;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("note", &[("body", Text)])).await?;
    l.upsert("note", "n1", json!({"body": "x"})).await?;
    let lake = store.lake().clone();
    let mut held = Some(store);
    for (n, mode) in [OpenMode::Read, OpenMode::Write, OpenMode::Create]
        .into_iter()
        .enumerate()
    {
        let ahead = if n == 0 { 2 } else { 0 };
        let lines: Lines = Box::new(move |_, _| (Vec::new(), ahead));
        let c = (given.continued)(held.take(), lake.clone(), lines, mode, Seams::default());
        let c = c
            .await
            .map_err(|e| Failure::new(format!("committing the projection ahead: {e}")))?;
        let (tip, folded) = (c.before.tip.as_ref().map(|t| t.0), c.before.folded_through);
        ensure!(
            tip.is_some() && folded == tip.map(|t| Position(t.0 + 2)),
            "the projection is committed two positions past the log's tip, {tip:?}; it is \
             folded through {folded:?}"
        );
        let e = c.opened.err();
        ensure!(
            matches!(e.as_ref().and_then(Error::situation),
                Some(Situation::Damaged { part: Part::Log, position: None, flaw: Flaw::EndsBefore { position } })
                    if Some(*position) == folded),
            "a lake whose projection is folded through {folded:?}, past its log's tip \
             {tip:?}, is refused at an open for {mode:?} as `damaged`, the log ending before \
             that position; it answered {e:?}"
        );
        ensure!(
            c.before.lease_free && c.after.as_ref() == Some(&c.before),
            "an open for {mode:?} refused for a projection ahead of its log leaves the \
             log's tip, the position the projection is folded through and the free lease as \
             they were, {:?}; they are {:?}",
            c.before,
            c.after
        );
    }
    let nothing: Lines = Box::new(|_, _| (Vec::new(), 0));
    let c = (given.continued)(None, lake, nothing, OpenMode::Rebuild, Seams::default());
    let c = c
        .await
        .map_err(|e| Failure::new(format!("reading the lake below the store: {e}")))?;
    let rebuilt = c.opened.map_err(|e| {
        Failure::new(format!(
            "an open to rebuild makes the projection from the log; it answered {e}"
        ))
    })?;
    let l = Lake::of(rebuilt.as_ref());
    let body = l.read("note", "n1").await?.fields.get("body").cloned();
    ensure!(
        body == Some(json!("x")),
        "after the rebuild the lake answers the record its log holds; `n1` holds {body:?}"
    );
    Ok(())
}

/// The flaw an error says the log is damaged by, itself or as the cause
/// an import's fold or its stop names.
fn damaged(e: &Error) -> Option<&Flaw> {
    match e.situation()? {
        Situation::Damaged { flaw, .. } => Some(flaw),
        Situation::ImportNotFolded { cause } | Situation::ImportStopped { cause, .. } => {
            damaged(cause)
        }
        _ => None,
    }
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
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    let l = Lake::of(store.as_ref());
    let mut tally = type_of("tally", &[("k", Text)]);
    tally.key = vec!["k".into()];
    l.declare(tally).await?;
    l.upsert("tally", "x", json!({"k": "x"})).await?;
    l.declare(tally_view("view_a", "tally")).await?;
    l.declare(tally_view("view_b", "view_a")).await?;
    l.declare(tally_view("view_c", "tally")).await?;
    // Off the cycle, a view that reads into it, both of its views.
    let mut into = tally_view("view_d", "view_a");
    into.fields
        .push(crate::conformance::fixture::field("m", Number));
    if let Some(v) = into.view.as_mut() {
        let merge = json!({"source": "view_b", "group_by": ["k"],
                           "measures": [{"name": "m", "expr": "count()"}]});
        v.merge
            .push(serde_json::from_value(merge).expect("a merged source"));
    }
    l.declare(into).await?;
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
            matches!(invalid_case(&r), Some(Case::ViewReadsItself { chain, .. })
                if ["view_a", "view_b"].iter().all(|v| chain.0.iter().any(|c| c == v))),
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
    // So for a holder of `only` and a reader without `restricted`, and
    // each is described the lake (lakelands#942).
    let only = json!({"default": "read", "restricted": true,
                      "types": {"tally": {"only": {"k": "x"}}}});
    let readers = [
        own.clone(),
        super::only::holder(imported.as_ref(), "only", only),
        super::only::holder(imported.as_ref(), "plain", json!({"default": "read"})),
    ];
    for who in &readers {
        for view in ["view_a", "view_b", "view_d"] {
            let r = cat
                .find(who, &view.into(), lakelands_api::read::Find::default())
                .await;
            ensure!(
                matches!(invalid_case(&r), Some(Case::ViewReadsItself { chain, .. })
                    if ["view_a", "view_b"].iter().all(|v| chain.0.iter().any(|c| c == v))),
                "`{view}` read by `{}` is refused as reading itself; it answered {r:?}",
                who.id
            );
        }
        let off = cat
            .find(who, &"view_c".into(), lakelands_api::read::Find::default())
            .await;
        let off = off.must("reading `view_c`")?;
        ensure!(
            matches!(&off.data, lakelands_api::Found::Records { rows, .. } if rows.len() == 1),
            "`view_c` read by `{}` counts the one tally; it answered {:?}",
            who.id,
            off.data
        );
        let d = cat.describe(who, Default::default()).await;
        d.must(&format!("describing the lake to `{}`", who.id))?;
    }
    Ok(())
}
