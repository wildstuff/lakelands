//! Every lake exports its log as the segment files a verifier reads
//! without Lakelands
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`,
//! `docs/log-format.md` §4.2).
//!
//! The export is read through the store's change feed, so each check runs
//! it the way a host does, against the lake the engine holds, and reads the
//! files back byte by byte.

use std::fs;
use std::path::Path;

use lakelands_api::error::Flaw;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::principal::PrincipalKind;
use lakelands_api::seam::{ChangeFeed, FeedBatch};
use sha2::{Digest as _, Sha256};

use super::rekey;
use crate::canonical;
use crate::conformance::fixture::{self, doors, owner, type_of};
use crate::conformance::prelude::*;
use crate::export::{self, Manifest};

check!(
    ONE_FILE_PER_SEGMENT = an_export_writes_one_file_per_segment_of_canonical_lines,
    "030 and log-format §4.2: an export writes one file per segment, named by its first position in 20 digits, holding its events in a copy's stored form, whose lines restore to each event's canonical bytes, and the sealed file's digest, over those bytes each followed by a line feed, is the one the checkpoint after it lists",
    scratch
);

check!(
    FRAMED_COPY = an_export_frames_each_append_and_imports_byte_for_byte,
    "log-format §4.2 and 083, extended (lakelands#1540): an export frames each append of two or more events the feed tells, whole, and holds the others whole, every event line holding its `position` and only its file's first its `prev`; imported into a fresh lake, which restores each event and appends each frame as one append, it answers the head and verify as the lake it came from, its feed tells the same appends, and it exports the same files byte for byte",
    given
);

check!(
    FRAMED_DAMAGE = a_framed_export_whose_stored_form_breaks_is_refused_naming_the_flaw,
    "log-format §1.2 and §4.2: verifying an export, and importing it, refuses a frame line that is no frame, a frame line among the lines a frame covers, a line of another format than its frame's, a line holding a member its frame holds, a file that ends among a frame's lines, a file's first event line that holds no `prev`, and a line whose `position` or `prev` is not the one the line before gives, each naming its flaw and the position of the event it stands for",
    given
);

check!(
    BACKUP_FRAME_CUT = a_backup_stopped_among_a_frames_lines_writes_its_run_again_whole,
    "077 and log-format §4.2: a backup whose file ends among the lines a frame covers, after its frame line or after some of its lines, a stopped run's, is cut at the frame line, and the next run writes that run again whole, so the backup holds the files an export of the head writes and restores the lake",
    given
);

check!(
    VERIFIES = an_export_verifies_to_the_through_and_hash_the_lake_answers,
    "030: an export verifies from its files alone to the position and hash the lake's own verify answers; its manifest names the same pair and every file with its digest, and is never trusted: with a wrong manifest or none the export verifies the same",
    scratch
);

check!(
    GROWS = a_backup_writes_an_exports_files_and_grows_by_what_follows,
    "030 and 077: an export, and a backup, read through the head as it stood when they began; a lake never checkpointed writes one file 00000000000000000000.jsonl; a backup writes the files an export of its head writes and no manifest, and run again into its directory writes only the events after its last line, leaving its sealed files byte for byte and extending its last",
    scratch
);

check!(
    BACKUP_CUT = a_backup_taken_while_writes_land_restores_the_lake_at_its_cut,
    "077: a backup run while writes land, each before a page of the feed it reads, holds the log through the head it read before them, its cut, and imported into a fresh lake answers at its head as the lake answers at the cut: the head, its hash, the feed, and every type's records with provenance; run again while writes land, it copies after its last line through its own cut",
    given
);

check!(
    BACKUP_TORN = a_backup_cuts_a_torn_tail_and_refuses_a_log_the_lake_does_not_continue,
    "077: a run cuts the bytes after its directory's last line feed, a stopped run's, removing a file that holds no whole line, and continues; a directory whose last line is not the lake's event at its position, another lake's or another log's or one past the lake's head, is refused as a failed precondition, `backup_other_log` naming the position, and nothing in it is changed, its torn tail included",
    scratch
);

check!(
    BACKUP_DAMAGED = a_damaged_backup_is_refused_at_its_import,
    "077: a backup restored is an import, which verifies it whole first: a byte changed in a sealed file or in its last line, or its first file removed, is refused as `damaged`",
    given
);

check!(
    NOT_EMPTY = an_export_into_a_directory_that_holds_anything_is_refused,
    "030 and 077: an export writes only into an empty or absent directory, a backup only into one holding nothing beside `log/` and `export.json` (no lake's directory), one holding only `export.json` taking the whole log; into any other each is refused as a failed precondition, the export's directory not empty (no path on the host is named), and writes nothing",
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
    VERIFY_THROUGH = a_verify_reads_no_further_than_it_is_asked,
    "`LakeStore::verify` (lakelands#1156): a log whose event after the tip is damaged, written in the format before the release's so that its format falls, with one more event after it in the release's, opens to read; `verify` through the tip holds and names it, through the damaged event, and with no end, is refused as `damaged` at its position: an event past the range asked is not read",
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

check!(
    ON_BEHALF_OF_FROM_21 = an_on_behalf_of_out_of_its_place_is_refused_naming_it,
    "088 (lakelands#1489) and log-format §1.1: `on_behalf_of` sits in the envelope from format 21, never inside `context`; an export whose line carries it in `context`, or whose last line is of format 20 and carries it, its hash made right, is refused by its verification and its import at that position, naming the member",
    given
);

check!(
    OCCASION_FROM_22 = an_occasion_before_its_format_is_refused_naming_it,
    "088, extended (lakelands#1584), and log-format §1.1: `context.occasion` is of format 22; an export whose last line is of format 21 and carries it, its hash made right, is refused by its verification and its import at that position, naming the member",
    given
);

check!(
    SETTING_PERIODS_FROM_21 = a_dated_setting_of_format_20_is_refused_naming_it,
    "090 (lakelands#1549) and log-format §1.3: a setting's values from dates are of format 21; an export whose last line, of format 20, its hash made right, writes `periods` on `sys.setting`, proposes a write of it with `from`, or declares a call of `setting` on a date, is refused by its verification and its import at that position, naming the format",
    given
);

check!(
    IMPORT_ID_FORM = an_import_refuses_an_id_not_of_its_form_naming_its_position,
    "086 and log-format §3: an export whose line carries an `id` that is not 32 lower-case hex digits, its hash made right, is refused on import naming the flaw and the line's position, on every engine, as PostgreSQL keeps an id as its bytes",
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
pub(super) fn files(dir: &Path) -> Result<Vec<String>, Failure> {
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
            restored(&dir, file)? == lines,
            "{file} restores to the canonical bytes of each event from position {} and a line \
             feed after each, and it does not",
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
    let digest = canonical::hex(&Sha256::digest(restored(&dir, &want[0])?));
    ensure!(
        listed.len() == 1 && listed[0].digest.0 == digest,
        "the checkpoint lists the sealed file with the digest {digest} of its events as \
         restored, and it lists {listed:?}"
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
        let digest = canonical::hex(&Sha256::digest(restored(&dir, &s.file)?));
        ensure!(
            s.digest.0 == digest,
            "the manifest gives {} the digest of its events as restored, {digest}, and it gives {}",
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

async fn backup(store: &dyn LakeStore, dir: &Path) -> Result<export::Backup, Failure> {
    export::backup(store, &*store.feed(), dir)
        .await
        .must(&format!("backing up into {}", dir.display()))
}

/// Whether `dir` holds under `log/`, byte for byte, the files an export
/// of the lake's head writes into `to`.
async fn as_exported(store: &dyn LakeStore, dir: &Path, to: &Path) -> Result<bool, Failure> {
    write(store, to).await?;
    let names = files(dir)?;
    for f in &names {
        if read(dir, f)? != read(to, f)? {
            return Ok(false);
        }
    }
    Ok(names == files(to)?)
}

/// `from`'s files under `log/`, copied into `to`.
fn copied(from: &Path, to: &Path) -> Result<(), Failure> {
    fs::create_dir_all(to.join("log")).map_err(|e| Failure::new(format!("making {to:?}: {e}")))?;
    for f in files(from)? {
        let copy = fs::copy(from.join(&f), to.join(&f));
        copy.map_err(|e| Failure::new(format!("copying {f}: {e}")))?;
    }
    Ok(())
}

async fn a_backup_writes_an_exports_files_and_grows_by_what_follows(
    store: Store,
    scratch: PathBuf,
) -> Held {
    let (scratch, s) = (Scratch::new(scratch), store.as_ref());
    let (first, dir) = (invoices(s, 0, 2).await?, scratch.dir("backup"));
    let new = backup(s, &dir).await?;
    let only = export::file_name(Position::GENESIS);
    ensure!(
        files(&dir)? == [only.clone()]
            && as_exported(s, &dir, &scratch.dir("a")).await?
            && (new.since, new.through, new.copied) == (None, first, first.0 + 1)
            && !dir.join("export.json").exists(),
        "a lake never checkpointed exports and backs up one file, {only}, the same, through its \
         head {first}, and the backup no manifest; it answered {new:?}",
    );
    let sealed_at = store.checkpoint().await.must("checkpointing")?;
    let head = invoices(s, 2, 3).await?;
    let grown = backup(s, &dir).await?;
    ensure!(
        (grown.since, grown.copied) == (Some(first), head.0 - first.0)
            && as_exported(s, &dir, &scratch.dir("b")).await?,
        "a backup run again after {first} copies the events through {head}, into the files an \
         export writes; it answered {grown:?}"
    );
    let (sealed, active) = (read(&dir, &only)?, export::file_name(sealed_at));
    let was = read(&dir, &active)?;
    invoices(s, 5, 2).await?;
    let late = backup(s, &dir).await?;
    let is = read(&dir, &active)?;
    ensure!(
        late.copied == 2
            && read(&dir, &only)? == sealed
            && is.len() > was.len()
            && is.starts_with(&was)
            && as_exported(s, &dir, &scratch.dir("c")).await?,
        "a later run copies 2 events, leaves the file sealed at {sealed_at} as it was and \
         extends the active one, as an export writes them: it answered {late:?}, the active \
         file {} bytes then {}",
        was.len(),
        is.len()
    );
    Ok(())
}

/// The store's feed, writing an invoice before it answers each poll: the
/// run reading it runs while every one of those writes lands.
struct Writing(Store);

#[async_trait::async_trait]
impl ChangeFeed for Writing {
    async fn poll(&self, from: Position, filter: &FeedFilter, n: u32) -> Result<FeedBatch, Error> {
        let _ = invoices(self.0.as_ref(), 100 + from.0 as u32, 1).await;
        self.0.feed().poll(from, filter, n).await
    }
    async fn ack(&self, consumer: &str, through: Position) -> Result<(), Error> {
        self.0.feed().ack(consumer, through).await
    }
}

async fn a_backup_taken_while_writes_land_restores_the_lake_at_its_cut(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use lakelands_api::{Find, ReadAt};
    let scratch = Scratch::new(given.scratch.clone());
    let (store, _) = exported_lake(store, &given.reopen).await?;
    let (dir, s) = (scratch.dir("backup"), store.as_ref());
    let before = s.head().await.must("reading the head")?;
    let cut = export::backup(s, &Writing(store.clone()), &dir)
        .await
        .must("backing up")?;
    let after = s.head().await.must("reading the head")?;
    ensure!(
        before == cut.through && cut.through < after,
        "a run cuts at the head as it began, {before}, before the lake's head {after}: {}",
        cut.through
    );
    let grown = scratch.dir("grown");
    copied(&dir, &grown)?;
    let more = export::backup(s, &Writing(store.clone()), &grown).await;
    let (more, head) = (
        more.must("backing up again")?,
        s.head().await.must("reading the head")?,
    );
    let says = export::verify(&grown).must("verifying the grown backup")?;
    ensure!(
        more.since == Some(cut.through)
            && after < more.through
            && more.through < head
            && (says.through, &says.hash) == (more.through, &more.hash),
        "a run again while writes land copies after {} through its own cut, before the head \
         {head}, and verifies to it: it answered {more:?}, the files {says:?}",
        cut.through
    );
    let imported = (given.import)(dir, store.lake().clone(), None, import_seams()).await;
    let imported = imported.map_err(|e| Failure::new(format!("restoring the backup: {e}")))?;
    let i = imported.store.as_ref();
    let mut feed = log_events(s).await?;
    feed.truncate(cut.through.0 as usize + 1);
    ensure!(
        log_events(i).await? == feed && feed.last().map(|e| &e.hash) == Some(&cut.hash),
        "the restored lake's feed is the lake's through the cut {}, hash {}",
        cut.through,
        cut.hash.0
    );
    let (own, cat, theirs, restored) = (owner(s), doors(s), owner(i), doors(i));
    let described = cat.describe(&own, ReadAt::default()).await;
    for t in &described.must("describing")?.data.types {
        let wide = |at| Find {
            limit: Some(1_000),
            ..find_at(at)
        };
        let want = cat.find(&own, &t.name, wide(Some(cut.through))).await;
        let got = restored.find(&theirs, &t.name, wide(None)).await;
        ensure!(
            format!("{:?}", want.map(|r| r.data)) == format!("{:?}", got.map(|r| r.data)),
            "the restored lake finds `{}` at its head as the lake does at the cut {}",
            t.name,
            cut.through
        );
    }
    Ok(())
}

async fn a_backup_cuts_a_torn_tail_and_refuses_a_log_the_lake_does_not_continue(
    store: Store,
    scratch: PathBuf,
) -> Held {
    let (scratch, s) = (Scratch::new(scratch), store.as_ref());
    let (dir, export) = (scratch.dir("backup"), scratch.dir("export"));
    let held = invoices(s, 0, 3).await?;
    backup(s, &dir).await?;
    let only = export::file_name(Position::GENESIS);
    // A stopped run: half a line after the last, and a file of half a line.
    let (next, half) = (export::file_name(Position(99)), b"{\"body\":{\"kind\"");
    let tear = || -> Result<Vec<Vec<u8>>, Failure> {
        let torn = [read(&dir, &only)?, half.to_vec()].concat();
        let w = fs::write(dir.join(&only), torn).and_then(|()| fs::write(dir.join(&next), half));
        w.map_err(|e| Failure::new(format!("tearing the backup: {e}")))?;
        Ok(vec![read(&dir, &only)?, read(&dir, &next)?])
    };
    tear()?;
    let head = invoices(s, 3, 1).await?;
    let run = backup(s, &dir).await?;
    ensure!(
        (run.since, run.copied) == (Some(held), 1) && as_exported(s, &dir, &export).await?,
        "a run over a torn tail cuts it and copies the event after {held} through {head}, as an \
         export writes it; it answered {run:?}",
    );
    let mut lines = lines_of(&dir, &only)?;
    let past = Position(head.0 + 5);
    let others = [
        ("lake", json!("another-lake"), head),
        ("known_at", json!(1), head),
        ("position", json!(past.0), past),
    ];
    for (member, value, held) in others {
        let last = lines.len() - 1;
        lines[last] = rehashed(&lines_of(&export, &only)?[last], |e| e[member] = value)?;
        rewrite(&dir, &only, &lines)?;
        let before = tear()?;
        let said = export::backup(s, &*s.feed(), &dir).await;
        ensure!(
            matches!(&said, Err(e) if e.case().map(|c| &c.case) == Some(&Case::BackupOtherLog { held }))
                && [read(&dir, &only)?, read(&dir, &next)?] == *before,
            "a backup whose last line's `{member}` is another log's is refused as \
             `backup_other_log` at {held}, changing nothing, and it answered {said:?}"
        );
    }
    Ok(())
}

async fn a_damaged_backup_is_refused_at_its_import(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (scratch, s) = (Scratch::new(given.scratch.clone()), store.as_ref());
    invoices(s, 0, 3).await?;
    store.checkpoint().await.must("checkpointing")?;
    invoices(s, 3, 2).await?;
    let from = scratch.dir("from");
    backup(s, &from).await?;
    let names = files(&from)?;
    // A hex digit of a line's hash changed, so its hash no longer holds.
    let flip = |file: &str, at_end: bool, dir: &Path| -> Held {
        let mut lines = lines_of(dir, file)?;
        let n = if at_end { lines.len() - 1 } else { 1 };
        let line = &mut lines[n];
        let at = line
            .windows(8)
            .position(|w| w == b"\"hash\":\"")
            .unwrap_or(0)
            + 8;
        line[at] = if line[at] == b'a' { b'b' } else { b'a' };
        rewrite(dir, file, &lines)
    };
    let damages = [
        "a byte in a sealed file",
        "a byte in its last line",
        "its first file removed",
    ];
    for (n, damage) in damages.into_iter().enumerate() {
        let dir = scratch.dir(&format!("damaged-{n}"));
        copied(&from, &dir)?;
        match n {
            0 => flip(&names[0], false, &dir)?,
            1 => flip(&names[names.len() - 1], true, &dir)?,
            _ => fs::remove_file(dir.join(&names[0])).map_err(|e| Failure::new(format!("{e}")))?,
        }
        let imported = (given.import)(dir, store.lake().clone(), None, import_seams()).await;
        let why = refused(imported, damage)?;
        ensure!(
            damaged(&why).is_some(),
            "a backup with {damage} is refused as damaged at its import; it was refused with: {why}"
        );
    }
    Ok(())
}

async fn an_export_into_a_directory_that_holds_anything_is_refused(
    store: Store,
    scratch: PathBuf,
) -> Held {
    let (scratch, s) = (Scratch::new(scratch), store.as_ref());
    invoices(s, 0, 1).await?;
    let dir = scratch.dir("full");
    backup(s, &dir).await?;
    let only = export::file_name(Position::GENESIS);
    let kept = read(&dir, &only)?;
    fs::write(dir.join("notes.txt"), b"kept\n")
        .map_err(|e| Failure::new(format!("writing into {dir:?}: {e}")))?;
    let said = export::write(s, &dir).await.map(|_| ());
    let again = export::backup(s, &*s.feed(), &dir).await.map(|_| ());
    ensure!(
        [&said, &again].iter().all(|said| matches!(reason(said), Some(Refusal::Precondition(said)) if said.case == Case::ExportNotEmpty)),
        "an export, and a backup, into {dir:?}, which holds a file beside a backup's log/, are \
         refused as a failed precondition, the export's directory not empty, and they answered \
         {said:?} and {again:?}"
    );
    let left = fs::read_dir(&dir).map(Iterator::count);
    let left = left.map_err(|e| Failure::new(format!("listing {dir:?}: {e}")))?;
    ensure!(
        left == 2
            && files(&dir)? == [only.clone()]
            && read(&dir, &only)? == kept
            && read(&dir, "notes.txt")? == b"kept\n",
        "a refused export or backup writes nothing; the directory holds {left} entries"
    );
    let lone = scratch.dir("lone");
    let made = fs::create_dir_all(&lone).and_then(|()| fs::write(lone.join("export.json"), "{}"));
    made.map_err(|e| Failure::new(format!("writing into {lone:?}: {e}")))?;
    let new = backup(s, &lone).await?;
    ensure!(
        new.since.is_none() && files(&lone)? == [only],
        "a backup into a directory holding only export.json takes the whole log: {new:?}"
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

/// A file's events as they are restored from its lines and its frames
/// (log-format §1.2), each the canonical bytes of its whole event, without
/// a line feed.
pub(super) fn lines_of(dir: &Path, file: &str) -> Result<Vec<Vec<u8>>, Failure> {
    let bytes = read(dir, file)?;
    let mut lines = crate::log::Lines::default();
    let mut events = Vec::new();
    for line in bytes
        .strip_suffix(b"\n")
        .unwrap_or(&bytes)
        .split(|b| *b == b'\n')
    {
        let read = lines.read(line).map_err(|flaw| {
            Failure::new(format!("{file} of the export does not restore: {flaw:?}"))
        })?;
        events.extend(read);
    }
    Ok(events)
}

/// The bytes a file's digest is taken over: its events as restored, each
/// followed by a line feed.
fn restored(dir: &Path, file: &str) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    for event in lines_of(dir, file)? {
        bytes.extend(event);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// Writes `lines` as `file`, each with its line feed: each a whole event,
/// a line a reader takes as it stands.
pub(super) fn rewrite(dir: &Path, file: &str, lines: &[Vec<u8>]) -> Held {
    let mut bytes = lines.join(&b'\n');
    bytes.push(b'\n');
    fs::write(dir.join(file), bytes).map_err(|e| Failure::new(format!("damaging {file}: {e}")))
}

/// `line` with `change` applied to its object and its hash computed
/// again, so only the change is wrong.
pub(super) fn rehashed(line: &[u8], change: impl FnOnce(&mut Value)) -> Result<Vec<u8>, Failure> {
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
        ..
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
    copied(&from, &damaged)?;
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

async fn an_on_behalf_of_out_of_its_place_is_refused_naming_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let cases: [Misplaced; 2] = [
        (
            "carries `on_behalf_of` in its `context`",
            Flaw::OnBehalfOfInContext,
            |e| e["context"]["on_behalf_of"] = json!("human:alice"),
        ),
        (
            "is of format 20 and carries `on_behalf_of`",
            Flaw::OnBehalfOfBefore { v: 20 },
            |e| {
                e["v"] = json!(20);
                e["on_behalf_of"] = json!("human:alice");
            },
        ),
    ];
    misplaced_refused(store, given, &cases).await
}

async fn an_occasion_before_its_format_is_refused_naming_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let cases: [Misplaced; 1] = [(
        "is of format 21 and carries `context.occasion`",
        Flaw::OccasionBefore { v: 21 },
        |e| {
            e["v"] = json!(21);
            e["context"]["occasion"] = json!("live");
        },
    )];
    misplaced_refused(store, given, &cases).await
}

/// What an export's last line is made to say, the flaw a reader names
/// for it, and the change that makes it.
type Misplaced = (&'static str, Flaw, fn(&mut Value));

/// An export whose last line, its hash made right, is changed by each of
/// `cases` is refused by its verification and its import at that
/// position, naming the case's flaw.
async fn misplaced_refused(
    store: Store,
    given: crate::conformance::Given,
    cases: &[Misplaced],
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    invoices(store.as_ref(), 0, 1).await?;
    let head = store.head().await.must("reading the head")?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let names = files(&from)?;
    let active = names[names.len() - 1].clone();
    for (n, (said, flaw, made)) in cases.iter().enumerate() {
        let dir = scratch.dir(&format!("misplaced-{n}"));
        copied(&from, &dir)?;
        let mut lines = lines_of(&dir, &active)?;
        let last = lines.len() - 1;
        lines[last] = rehashed(&lines[last], made)?;
        rewrite(&dir, &active, &lines)?;
        let verified = export::verify(&dir);
        let imported = (given.import)(dir, store.lake().clone(), None, import_seams()).await;
        let imported = refused(imported, &format!("an export whose last line {said}"))?;
        for (door, e) in [("verify", verified.err()), ("an import", Some(imported))] {
            ensure!(
                e.as_ref().and_then(damaged) == Some(flaw)
                    && matches!(e.as_ref().and_then(Error::situation),
                        Some(Situation::Damaged { position: Some(p), .. }) if *p == head),
                "{door} of an export whose last line {said} is refused at {head} as {flaw:?}; \
                 it answered {e:?}"
            );
        }
    }
    Ok(())
}

async fn an_import_refuses_an_id_not_of_its_form_naming_its_position(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    invoices(store.as_ref(), 0, 1).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let file = (files(&from)?.pop()).ok_or_else(|| Failure::new("the export holds a file"))?;
    let mut lines = lines_of(&from, &file)?;
    let at = lines.len() - 1;
    let position = serde_json::from_slice::<Value>(&lines[at]).ok();
    let position = position.and_then(|e| e["position"].as_u64());
    lines[at] = rehashed(&lines[at], |e| {
        e["id"] = json!(e["id"].as_str().map(str::to_uppercase));
    })?;
    rewrite(&from, &file, &lines)?;
    let imported = (given.import)(from, store.lake().clone(), None, import_seams()).await;
    let why = refused(
        imported,
        "an export whose last line's `id` is upper-case hex",
    )?;
    ensure!(
        matches!(why.situation(), Some(Situation::Damaged { position: Some(p), flaw: Flaw::IdNotOfForm, .. })
            if Some(p.0) == position),
        "an export whose line at {position:?} carries an upper-case `id` must be refused naming \
         the flaw and that position; it was refused with: {why:?}"
    );
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

async fn a_verify_reads_no_further_than_it_is_asked(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::{Digest, Event, EventId};
    use lakelands_api::seam::Seams;
    let lake = store.lake().clone();
    let events = crate::conformance::fixture::log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let (Some(taken), Some(last)) = (events.iter().find(taken), events.last()) else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    let (template, known_at, tip) = (taken.clone(), last.known_at, last.position);
    // Two events after the tip: the first in the format before the
    // release's, a fall a reader refuses at its position, and the second in
    // the release's, so the log's end reads as whole on every engine.
    let lines: crate::conformance::Lines = Box::new(move |(tip, prev), fence| {
        let event = |position: Position, prev: Digest, v: u32| Event {
            v,
            position,
            id: EventId(format!("{:032x}", position.0)),
            prev,
            known_at,
            fence,
            body: EventBody::LeaseTaken {
                fence,
                holder: "another release".into(),
            },
            ..template.clone()
        };
        let format = lakelands_api::event::LOG_FORMAT_VERSION;
        let fallen = event(tip.next(), prev, format - 1);
        let damaged = crate::conformance::fixture::canonical_line(fallen);
        let after = event(tip.next().next(), damaged.hash.clone(), format);
        let after = crate::conformance::fixture::canonical_line(after);
        (vec![damaged, after], 0)
    });
    let c = (given.continued)(Some(store), lake, lines, OpenMode::Read, Seams::default());
    let c = c
        .await
        .map_err(|e| Failure::new(format!("continuing the lake below the store: {e}")))?;
    let reader = c
        .opened
        .map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let at = tip.next();
    let held = reader.verify(None, Some(tip)).await;
    ensure!(
        held.as_ref().is_ok_and(|v| v.through == tip),
        "`verify` through {tip}, before the event at {at} whose format falls, holds and names {tip}; it \
         answered {held:?}"
    );
    for through in [Some(at), None] {
        let refused = reader.verify(None, through).await;
        ensure!(
            matches!(refused.as_ref().err().and_then(Error::situation),
                Some(Situation::Damaged { position: Some(p), .. }) if *p == at),
            "`verify` through {through:?} reads the event at {at}, whose format falls, and is \
             refused as `damaged` there; it answered {refused:?}"
        );
    }
    reader
        .close()
        .await
        .map_err(|e| Failure::new(format!("closing the lake: {e}")))?;
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

/// One event line of an export's file: the line as it stands, the event
/// it restores to, and the frame, counted from 0 in the file, whose lines
/// it is among.
struct CopyLine {
    line: Value,
    event: Value,
    frame: Option<usize>,
}

/// The event lines of an export's file, read in order, and how many
/// events each frame line says it covers.
fn copy_lines(dir: &Path, file: &str) -> Result<(Vec<CopyLine>, Vec<u64>), Failure> {
    let bytes = read(dir, file)?;
    let (mut lines, mut out, mut frames, mut left) =
        (crate::log::Lines::default(), Vec::new(), Vec::new(), 0);
    let broken = |e: String| Failure::new(format!("{file} of the export does not restore: {e}"));
    for raw in bytes
        .strip_suffix(b"\n")
        .unwrap_or(&bytes)
        .split(|b| *b == b'\n')
    {
        let line: Value = serde_json::from_slice(raw).map_err(|e| broken(e.to_string()))?;
        let event = lines.read(raw).map_err(|f| broken(format!("{f:?}")))?;
        let Some(event) = event else {
            left = line["events"].as_u64().unwrap_or(0);
            frames.push(left);
            continue;
        };
        let event = serde_json::from_slice(&event).map_err(|e| broken(e.to_string()))?;
        let frame = (left > 0).then(|| frames.len() - 1);
        left = left.saturating_sub(1);
        out.push(CopyLine { line, event, frame });
    }
    Ok((out, frames))
}

/// The last position of the append of each event of `store`'s log, by
/// its position, as the feed tells it.
async fn append_ends(store: &dyn LakeStore) -> Result<BTreeMap<Position, Position>, Failure> {
    let (feed, mut from, mut ends) = (store.feed(), Position::GENESIS, BTreeMap::new());
    loop {
        let batch = feed.poll(from, &FeedFilter::default(), 100).await;
        let batch = batch.must(&format!("polling the feed from {from}"))?;
        if batch.events.is_empty() {
            return Ok(ends);
        }
        for e in &batch.events {
            ends.insert(
                e.position,
                batch.append_of(e.position).unwrap_or(e.position),
            );
        }
        from = batch.next;
    }
}

/// Fails unless `dir`'s files hold their events in a copy's stored form
/// (log-format §4.2), each append `ends` tells framed, and some frame
/// among them.
fn copy_form(dir: &Path, ends: &BTreeMap<Position, Position>) -> Held {
    let mut framed = false;
    for file in files(dir)? {
        let (lines, frames) = copy_lines(dir, &file)?;
        framed |= !frames.is_empty();
        ensure!(
            frames.iter().all(|n| *n >= 2),
            "each frame of {file} covers two events or more, and they cover {frames:?}"
        );
        for (i, l) in lines.iter().enumerate() {
            let at = &l.event["position"];
            ensure!(
                l.line.get("position") == Some(at) && (l.line.get("prev").is_some() == (i == 0)),
                "every event line of {file} holds its position, and only the first its `prev`; \
                 the line of {at} is {}",
                l.line
            );
        }
        for pair in lines.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let end = |l: &CopyLine| {
                let at = l.event["position"].as_u64().map(Position);
                at.and_then(|at| ends.get(&at)).copied()
            };
            let shared = end(a).is_some() && end(a) == end(b);
            let together = a.frame.is_some() && a.frame == b.frame;
            ensure!(
                together == shared,
                "in {file} the events at {} and {} are in one frame where the feed tells them of \
                 one append, and only then, and they are framed {:?} and {:?}",
                a.event["position"],
                b.event["position"],
                a.frame,
                b.frame
            );
        }
    }
    ensure!(
        framed,
        "an export of a lake that ingested records in one delivery holds a frame, and it holds none"
    );
    Ok(())
}

/// A lake with a `reading` type, a checkpoint, and readings ingested
/// before it and after it, each delivery one append of events that share
/// `known_at`.
async fn ingested(store: &dyn LakeStore) -> Held {
    let l = Lake::of(store);
    l.declare_json(json!({"declare": "type", "name": "reading", "fields": [
        {"name": "amount", "type": {"kind": "number"}, "authority": "source"}]}))
        .await?;
    let records = |from: u32| -> Vec<_> {
        (from..from + 6)
            .map(|i| fixture::source_record("reading", &format!("r{i}"), json!({"amount": i})))
            .collect()
    };
    fixture::delivered(store, &l, "meter", records(0), "ingesting six readings").await?;
    store.checkpoint().await.must("checkpointing")?;
    fixture::delivered(store, &l, "meter", records(6), "ingesting six more").await?;
    Ok(())
}

async fn an_export_frames_each_append_and_imports_byte_for_byte(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    ingested(store.as_ref()).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let ends = append_ends(store.as_ref()).await?;
    copy_form(&from, &ends)?;
    let imported = (given.import)(from.clone(), store.lake().clone(), None, import_seams());
    let imported = imported
        .await
        .map_err(|e| Failure::new(format!("importing the framed export: {e}")))?
        .store;
    let (want, got) = (
        store.verify(None, None).await.must("verifying the lake")?,
        imported
            .verify(None, None)
            .await
            .must("verifying the imported lake")?,
    );
    ensure!(
        (want.through, &want.hash) == (got.through, &got.hash),
        "the imported lake verifies to {} {}, as the lake it came from, {} {}",
        got.through,
        got.hash.0,
        want.through,
        want.hash.0
    );
    let told = append_ends(imported.as_ref()).await?;
    ensure!(
        told == ends,
        "the imported lake's feed tells the appends the lake's tells, {ends:?}; it tells {told:?}"
    );
    let again = scratch.dir("again");
    write(imported.as_ref(), &again).await?;
    let names = files(&from)?;
    ensure!(
        names == files(&again)?,
        "the imported lake exports the files {names:?} again"
    );
    for f in &names {
        ensure!(
            read(&from, f)? == read(&again, f)?,
            "the imported lake's {f} holds the same bytes as the lake's"
        );
    }
    Ok(())
}

/// A file's lines as they stand, frame lines among them, without their
/// line feeds.
fn raw_lines(dir: &Path, file: &str) -> Result<Vec<Vec<u8>>, Failure> {
    let bytes = read(dir, file)?;
    let body = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    Ok(body.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect())
}

/// `line`, a line of an export's file, with `change` applied to it.
fn changed(line: &[u8], change: impl FnOnce(&mut Value)) -> Result<Vec<u8>, Failure> {
    let mut value: Value = serde_json::from_slice(line)
        .map_err(|e| Failure::new(format!("an exported line is not JSON: {e}")))?;
    change(&mut value);
    Ok(canonical::to_vec(&value))
}

async fn a_framed_export_whose_stored_form_breaks_is_refused_naming_the_flaw(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    ingested(store.as_ref()).await?;
    let from = scratch.dir("from");
    write(store.as_ref(), &from).await?;
    let file = export::file_name(Position::GENESIS);
    let raw = raw_lines(&from, &file)?;
    // The file's first frame line, the position of its first event, how
    // many it covers, and its format.
    let values: Vec<Value> = raw
        .iter()
        .map(|l| serde_json::from_slice(l).unwrap_or(Value::Null))
        .collect();
    let Some(k) = values.iter().position(crate::log::is_frame) else {
        return Err(Failure::new(format!(
            "{file} of an export of a lake that ingested records in one delivery holds a frame, \
             and it holds none"
        )));
    };
    let (n, v) = (values[k]["events"].as_u64(), values[k]["v"].as_u64());
    let p = values.get(k + 1).and_then(|l| l["position"].as_u64());
    let (Some(n), Some(v), Some(p)) = (n, v, p) else {
        return Err(Failure::new(format!(
            "the frame line of {file} says how many events it covers and its format, and the \
             line after it its event's position; they are {} and {:?}",
            values[k],
            values.get(k + 1)
        )));
    };
    let known_at = values[k]["frame"]["known_at"].clone();
    let second = k + 2;
    let mut breaks: Vec<(&str, u64, Flaw, Vec<Vec<u8>>)> = Vec::new();
    let mut with = |what, at, flaw, edit: &dyn Fn(&mut Vec<Vec<u8>>) -> Held| -> Held {
        let mut lines = raw.clone();
        edit(&mut lines)?;
        breaks.push((what, at, flaw, lines));
        Ok(())
    };
    with(
        "a frame line holding a member no frame holds",
        p,
        Flaw::NotAFrame,
        &|l| {
            l[k] = changed(&l[k], |f| f["extra"] = json!(1))?;
            Ok(())
        },
    )?;
    with(
        "a frame line among the lines a frame covers",
        p + 1,
        Flaw::FrameInFrame,
        &|l| {
            l.insert(k + 2, l[k].clone());
            Ok(())
        },
    )?;
    with(
        "a line of the format before its frame's",
        p + 1,
        Flaw::FrameOther { frame: v, v: v - 1 },
        &|l| {
            l[second] = changed(&l[second], |e| e["v"] = json!(v - 1))?;
            Ok(())
        },
    )?;
    with(
        "a line holding the `known_at` its frame holds",
        p + 1,
        Flaw::FramedTwice {
            member: "known_at".into(),
        },
        &|l| {
            l[second] = changed(&l[second], |e| e["known_at"] = known_at.clone())?;
            Ok(())
        },
    )?;
    with(
        "the file ending after a frame's first line",
        p + 1,
        Flaw::FrameShort { events: n, held: 1 },
        &|l| {
            l.truncate(k + 2);
            Ok(())
        },
    )?;
    with(
        "the file's first event line holding no `prev`",
        0,
        Flaw::StartUnheld,
        &|l| {
            let o = values
                .iter()
                .position(|l| !crate::log::is_frame(l))
                .unwrap_or(0);
            l[o] = changed(&l[o], |e| _ = e.as_object_mut().map(|e| e.remove("prev")))?;
            Ok(())
        },
    )?;
    with(
        "a line holding a position the line before does not give",
        p + 1,
        Flaw::Position {
            expected: Position(p + 1),
            found: Some(Position(p + 6)),
        },
        &|l| {
            l[second] = changed(&l[second], |e| e["position"] = json!(p + 6))?;
            Ok(())
        },
    )?;
    with(
        "a line holding a `prev` that is not the hash before it",
        p + 1,
        Flaw::PrevHash,
        &|l| {
            l[second] = changed(&l[second], |e| {
                e["prev"] = json!(lakelands_api::event::Digest::zero().0)
            })?;
            Ok(())
        },
    )?;
    for (i, (what, at, flaw, lines)) in breaks.into_iter().enumerate() {
        let dir = scratch.dir(&format!("broken-{i}"));
        copied(&from, &dir)?;
        rewrite(&dir, &file, &lines)?;
        let said = export::verify(&dir);
        let named = match said.as_ref().err().and_then(Error::situation) {
            Some(Situation::Damaged { position, flaw, .. }) => Some((*position, flaw)),
            _ => None,
        };
        ensure!(
            named == Some((Some(Position(at)), &flaw)),
            "verifying an export with {what} is refused as `damaged` at {at}, naming {flaw:?}, \
             and it answered {said:?}"
        );
        let imported = (given.import)(dir, store.lake().clone(), None, import_seams()).await;
        let e = refused(imported, &format!("an import of an export with {what}"))?;
        ensure!(
            damaged(&e) == Some(&flaw),
            "an import of an export with {what} is refused as `damaged`, naming {flaw:?}, and it \
             answered {e}"
        );
    }
    Ok(())
}

async fn a_backup_stopped_among_a_frames_lines_writes_its_run_again_whole(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (scratch, s) = (Scratch::new(given.scratch.clone()), store.as_ref());
    ingested(s).await?;
    let whole = scratch.dir("whole");
    backup(s, &whole).await?;
    let last = files(&whole)?.pop().unwrap_or_default();
    let (lines, frames) = copy_lines(&whole, &last)?;
    let Some(frame) = lines.last().and_then(|l| l.frame) else {
        return Err(Failure::new(format!(
            "the last line of {last} is among the lines of a frame, the last delivery's, and it \
             is not; {} frames there",
            frames.len()
        )));
    };
    let bytes = read(&whole, &last)?;
    let feeds: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(i, _)| i + 1)
        .collect();
    // The frame line is the one before its first event line.
    let first = lines
        .iter()
        .position(|l| l.frame == Some(frame))
        .unwrap_or(0);
    let frame_line = first + frame;
    for (kept, what) in [(0, "its frame line"), (2, "two of its lines")] {
        let (dir, export) = (
            scratch.dir(&format!("stopped-{kept}")),
            scratch.dir(&format!("export-{kept}")),
        );
        copied(&whole, &dir)?;
        let end = feeds[frame_line + kept];
        let torn = [&bytes[..end], b"{\"body\":{".as_slice()].concat();
        fs::write(dir.join(&last), torn).map_err(|e| Failure::new(format!("tearing: {e}")))?;
        let run = backup(s, &dir).await?;
        ensure!(
            as_exported(s, &dir, &export).await?,
            "a backup stopped after {what} writes the frame's run again whole and holds the \
             files an export of the head writes; it answered {run:?}"
        );
        let restored = (given.import)(dir.clone(), store.lake().clone(), None, import_seams());
        let restored = restored
            .await
            .map_err(|e| Failure::new(format!("restoring the backup stopped after {what}: {e}")))?
            .store;
        let (want, got) = (s.head().await, restored.head().await);
        ensure!(
            want.as_ref().ok() == got.as_ref().ok(),
            "the backup stopped after {what}, run again, restores the lake at its head {want:?}, \
             and it restores {got:?}"
        );
    }
    Ok(())
}

async fn a_dated_setting_of_format_20_is_refused_naming_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let scratch = Scratch::new(given.scratch.clone());
    let l = Lake::of(store.as_ref());
    let ty = lakelands_api::seam::SETTING_TYPE;
    super::settings::declared(&l, "rate", "number", json!(1)).await?;
    let v = 20;
    for n in 0..3 {
        let (said, flaw) = match n {
            0 => {
                let period = json!({"key": "rate", "from": "2026-07-01", "value": 2});
                l.upsert(ty, "rate", period).await?;
                ("a dated setting", Flaw::SettingPeriodsBefore { v })
            }
            1 => {
                let write = ProposedWrite::Upsert {
                    ty: ty.into(),
                    key: "rate".into(),
                    patch: patch(json!({"key": "rate", "from": "2026-08-01", "value": 3})),
                    opts: Default::default(),
                };
                let proposed = l.cat.propose(&l.own, write, "a period".into()).await;
                proposed.must("proposing a period")?;
                ("a proposed period", Flaw::SettingMembersBefore { v })
            }
            _ => {
                let at = fixture::derived("at", Number, "setting('rate', due)", &["due"]);
                let due = fixture::field("due", StorageKind::Date);
                l.declare(type_decl("dated", vec![due, at])).await?;
                ("a setting read on a date", Flaw::SettingAtBefore { v })
            }
        };
        let head = store.head().await.must("reading the head")?;
        let dir = scratch.dir(&format!("dated-{n}"));
        write(store.as_ref(), &dir).await?;
        let active = files(&dir)?.pop().unwrap_or_default();
        let mut lines = lines_of(&dir, &active)?;
        let last = lines.len() - 1;
        lines[last] = rehashed(&lines[last], |e| e["v"] = json!(v))?;
        rewrite(&dir, &active, &lines)?;
        let verified = export::verify(&dir);
        let imported = (given.import)(dir, "dated".into(), None, import_seams()).await;
        let imported = refused(imported, &format!("{said} of format 20"))?;
        for (door, e) in [("verify", verified.err()), ("an import", Some(imported))] {
            ensure!(
                e.as_ref().and_then(damaged) == Some(&flaw)
                    && matches!(e.as_ref().and_then(Error::situation),
                        Some(Situation::Damaged { position: Some(p), .. }) if *p == head),
                "{door} of an export whose last line is {said} of format 20 is refused at \
                 {head} as {flaw:?}; it answered {e:?}"
            );
        }
    }
    Ok(())
}
