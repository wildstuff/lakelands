# 077 — A backup is an export that grows

## What is decided

**Accepted** on lakelands#1369 (2026-10-24), by the operator.

### A backup holds the export's segment files and nothing else

`lakelands_core::export::backup` writes into a new or empty directory
the files an export of the same head writes, reading the log through
the feed it is handed, the store's own or one in front of it: `log/<first>.jsonl`, each
event's canonical bytes and a line feed, a file per segment
(`docs/log-format.md` §4.2). It writes no `export.json`, removes one an
export it continues left, and holds no index, projection, lease or key.
A directory holding anything beside `log/` and `export.json` is refused
as `export_not_empty` before anything is written: a lake's own
directory holds its index and lease, and a run into it would append to
the file its writer appends to. `lakelands backup --to <DIR>` refuses a
directory inside a lake's too, and runs it on a handle opened to read,
beside a running `serve`, on every engine; a backup taken on one engine
restores on any other. `log/` is synced after the files a run wrote.

### Its cut is a head

A run reads the lake's head once and copies the feed through it. Every
engine hands the feed out in whole appends, so the cut is the end of
one: a SQLite handle reads only events before its index's last commit
line, PostgreSQL commits an append as one transaction, and memory
answers under its lock. Writes that land during the run are past the
cut and are the next run's. A seal during the run reaches it as a
`checkpoint` past the old tip, and the run starts a file there.

### A run into a backup writes only what follows its last line

Run again into its directory, the run finds the last whole line of its
last file, passing over files that hold none. The line must be the
lake's own event at the position it names, byte for byte as the feed
answers it; otherwise the directory holds another lake's or another
log's backup and the run is refused as a failed precondition,
`backup_other_log`, naming the position, with nothing in the directory
changed. Only then are the bytes after that line's line feed, which a
stopped run left, cut, and the files holding no whole line removed.
Then it reads the head and appends the events
after that line to the last file, and every later file as a
`checkpoint` starts it, synced as an export syncs them. Sealed files are
never rewritten. The lines before the last are not read again: a byte
changed in them is the restore's to find.

### A restore is an import

`lakelands restore <DIR>` is `import` of the backup's directory: it
verifies the whole directory, the hash chain and every checkpoint
against the files before it, refuses `damaged` at the position where it
stops being the log, and makes the lake at the backup's head, which
then answers as the lake did at the cut. `--check` verifies only and
writes nothing; its exit status is `verify`'s. There is no store
function of its own.

### The keys are kept apart

A backup never holds a lake's keys. `--keys-to <DIR>` copies the lake's
file of the root's file keyring into a directory of its own, refused
where it is the backup's directory, inside it, around it, or inside a
lake's directory. `restore --keys-from <DIR>` puts that file back first,
only where the root's keyring holds none for the lake: the file the
keyring holds knows every key destroyed since the copy. Where the
import is then refused, the file it put back is taken out again, so a
second try puts it back too. A lake the keyring holds no file for has
no keys to copy, and `--keys-to` copies none and says so. Both flags
need the root to name its file keyring (`keys.keyring` in
`lakelands.json`), which `lakelands init` leaves out; without one each
is refused. Where the import succeeds and only its line fails to be
written, the keys put back stay.

### A backup older than a forgetting restores refused where its keys are gone

A `forget` destroys a source's keys; its values stay in the log as
ciphertext. A backup taken before the act holds the log without the
act. Restored beside the keyring that destroyed the keys, its import
needs keys the keyring no longer has and is refused `key_missing`: it
fails closed and brings nothing back. The way out is a backup taken
after the act, run again into the same directory or a new one: it holds
the act, and the lake it restores reads the source as forgotten.

### Before an upgrade, back up into a new directory

An upgrade appends `lake_upgraded`, and the events after it are in the
next format. A backup run after it extends its directory past that
event, and the release before the upgrade refuses the whole directory,
as it refuses the upgraded lake (`log_format`). The way back from an
upgrade is therefore a backup taken into a new directory before it, and
kept; a run does not refuse to cross an upgrade, which would stop every
backup that grows at each release.

## Why

The log is the truth, and an export already writes it as files anyone
can verify. A backup that is those files needs no format of its own, no
restore of its own and no method on the engine seam: what verifies an
export verifies a backup, and what imports one restores the other. What
a backup adds is that it grows: a run copies only the events after its
directory's last line, so a lake can be backed up often at the cost of
what it wrote since.

The last line is checked against the lake because it is the one byte
string that ties the directory to the lake: it carries the lake, its
position and the hash of everything before it. A line that matches
continues the lake's chain; one that does not is another log, and
appending to it would make a directory that verifies nowhere. Reading
the whole backup again on every run would cost what the incremental run
saves, and the restore verifies it whole anyway.

No manifest is written because it would have to name the last file's
digest, and on PostgreSQL, which seals only on `checkpoint`, that file
can be most of the log, read again on every run. A manifest is never
trusted, and the run answers what it would say.

Keys stay out because forgetting destroys them: a backup that held both
would hold what the act destroyed, and restoring it would undo the act.
A restore that fails closed where the keys are gone keeps that promise
at the cost of an old backup's use, and a backup taken after the act is
always at hand.

## What it is not

- It is no file copy of a lake directory. A copy of a SQLite directory
  holds the index, can catch an append half written, and restores only
  on SQLite (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`);
  the command copies none of that.
- It is no backup of the projection, which the import folds again, and
  none of the keys.
- It is no replication: nothing follows the lake between runs.
- It reads no line before the directory's last on a run, so it finds no
  damage there; `restore --check` does.
- It writes nothing into a directory that holds anything but a backup
  or an export, nor inside a lake's directory, and does not refuse a
  run across an upgrade.
- It takes no lock: two runs into one directory at once are the
  operator's to keep apart.
