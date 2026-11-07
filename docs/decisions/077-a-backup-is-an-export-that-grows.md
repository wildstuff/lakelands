# 077 — A backup is an export that grows

## What is decided

**Accepted** on lakelands#1369 (2026-10-24), by the operator.

Amended on lakelands#1515, proposed for 0.24.0: a restore may stop at a
position in its log. No code lands with it.

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

098's blocks are this run's unit: a run continues after its last whole
block. The section on an upgrade goes with the operator's green field.

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

Accepted for 0.20.0 (lakelands#1413)
(`docs/decisions/083-a-log-stores-what-an-append-shares-once-and-seals-a-records-values-in-one-box.md`):
a run writes the form a copy of the log writes (`docs/log-format.md`
§4.2), every line holding its `position`, rather than each event's
canonical bytes. Its last whole line is then the last line that ends a
frame's lines, or stands in none: a frame a stopped run left without
all its lines is cut with them. The event the line restores, from the
line, its frame and the `hash` of the line before it, must be the
lake's own at its position, its canonical bytes as the feed answers
them, and a line that states no position is another log's. A
directory a run of format 19 left ends in a full line, which states
its position.

### A restore is an import

`lakelands restore <DIR>` is `import` of the backup's directory: it
verifies the directory through the position it restores, the hash chain
and every checkpoint below that position against the files before it,
refuses `damaged` at the position where it stops being the log, and
makes the lake at that position, which then answers as the lake did there.
The position is the backup's head unless `--through <POSITION>` names one
(`077 § A restore may stop at a position`). `--check` verifies only and
writes nothing; its exit status is `verify`'s. There is no store
function of its own.

### A restore may stop at a position

`restore --through <POSITION>` restores the lake at that position instead
of the backup's head: the import carries the log's events from its first
through it and no more, so the restored lake's head is the position, and
its events, positions and hashes through it are the lake's own. The
position may be 0, which restores the lake at its `lake_created`, and
otherwise names the last event of an append; a position inside an append
is refused naming that append's end, and one past the directory's head is
refused naming the head. Nothing in the backup's directory is written,
cut or marked, so a run into it afterwards still extends it from its last
line.

`restore --check --through <POSITION>` verifies the files through the one
holding the position and no further: the files before that one whole,
that one through the position's append, and every checkpoint at or below
the position against the files before it, which the prefix holds whole.
Damage past the position is not read, so this run neither finds it nor
refuses the position for it; a check without `--through` still reads the
whole directory, and is what refuses a backup broken after the state a
host would go back to.

The prefix is the log as it stood at the position, so a lake whose last
append started an act of forgetting or removal restores as that lake was:
mid-act, holding what the act had appended, and the road takes the act up
at the next writer's open, as it takes up any writer's crash (099); an
open to read only reports it owed. The head is the position when the
restore returns; the act appends later, as a write does.

The restored lake is the lake's own, its name inside every hash, so a
restore to a position is a fork of a lake that went on: it goes into an
empty directory as every import does, never over the lake it came from,
and the two are two writers of one name, kept apart as two runs into one
directory are. The position is what says they part.

The bound is every read the import and a check make of the directory:
`export::segment_files` lists the prefix's files, the store's `scan` reads
them and its sealed, forgot and shredded sets are the prefix's, and
`export::verify` takes the same bound, so `check`, `check_events` and a
restore read the same prefix and none of them reads a fault past the
position. The store's `import` takes the position and stops carrying
there. No method is added to the engine seam, which imports batches and
knows no position, as this record already held. `lakelands import <DIR>`
stays bare: an import is the lake an export came from, and going back to
a state is the restore's.

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
ciphertext. A backup taken before the act, or a position before it, holds
the log without the act. Restored beside the keyring that destroyed the
keys, its import needs keys the keyring no longer has and is refused
`key_missing`: it fails closed and brings nothing back. The way out is a
backup taken after the act, run again into the same directory or a new
one, or a position at or past the append that holds the act: it holds the
act, and the lake it restores reads the source as forgotten.

### A position before a forgetting restores the log, not the keys

A restore carries the log through its position and nothing past it, so a
position before an act of forgetting restores what the source wrote
before it and no act: the restored log does not say the source was
forgotten, because at that position it was not. What is left of the
source is the keys' question. Where the prefix holds a value sealed under
a key the act destroyed, the import needs a key the keyring no longer has
and is refused `key_missing`, as the section above says, and the way out
is a position at or past the act. Where the prefix holds no such value,
the restore succeeds, and beside the keyring that destroyed the keys the
source is still refused a new one (`KeyStore::destroy`): a write that
would seal a value of it is refused where that value's key is asked for,
so nothing of it is sealed again, and a write that seals nothing of it is
not. A `--keys-from` copy taken before the act still holds its keys, and
is put back only where the root's keyring holds no file for the lake:
beside the keyring's own file the older copy is refused, because that
file knows every destroy an older one does not. Restoring it is the
operator's choice of keys and not the log's, as the keys are kept apart
above.

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

A host that goes back to a known state knows it by a position, and the
log is the truth of that state: the prefix through the position is the
lake as it stood there, so the restore needs no form of its own and no
engine's help, and the bound is the one `verify` already carries. Damage
past the position is not the restored lake's and must not stand between a
host and the state it came from: the prefix verifies whole, and the check
without a bound is what refuses the directory whole.

A restore cannot carry an act that lies past its position: the positions
after it would hold events they never held, and the same bytes would fold
one way in the restored lake and another in the lake they came from, so
neither log would be the truth (001, 054). The keys therefore answer for
a source forgotten after the position, as they already answer for a
backup older than the act: destroyed and refused anew, with the prefix's
sealed values refused `key_missing` before a lake is made.

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
- A position is no rewind of the backup, and it leaves no mark in it:
  nothing is cut or written, and a run still extends from the last line.
- It is no undo of a forgetting: an act past the position is not carried,
  and a prefix holding values the act's keys sealed is refused
  `key_missing`.
- `--through` is the restore's, not the import command's: `lakelands import
  <DIR>` stays bare, because an import is the lake an export came from.
- It is no new numbering: the restored lake writes at the position after
  the one chosen, and its positions and hashes through it are the lake's
  own.
- It asserts nothing of the prefix but that the position ends an append:
  the events before it are what the log holds, and what they mean is the
  fold's.

## How it came to be

- A run writing the form a copy of the log writes, and continuing after
  its last whole frame, accepted with 083 (lakelands#1413).
- Amended by 098 (lakelands#1652), proposed for 0.23.0.
- Amended on lakelands#1515, proposed for 0.24.0: a restore may stop at a
  position, restoring the log through it and not the keys; no code lands
  with it.
