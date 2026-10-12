# 030 — A lake directory verifies without its engine

## What is decided

### The SQLite engine keeps a lake as a directory whose log anyone can read

The SQLite engine keeps a lake as a directory whose log anyone can read:

- `log/<first position>.jsonl` holds the segments exactly as the log
  format names them, each event's canonical bytes and a line feed,
  nothing else. A verifier needs the format and these files, not the
  engine.
- `index/<first position>.idx` holds, per event, the members the engine
  was handed beside the bytes (position, id, `prev`, `hash`, fence) and
  where the bytes lie, and one commit line closing each append. An
  append writes and syncs its segment lines, then its index lines and
  the commit line. Only events before the last commit line exist, so an
  append is whole after a crash or not there at all. The next writer
  cuts off what an interrupted append left. A writer that goes on after
  an append that failed cuts off what that one left before it answers
  the error, and again before every append and every seal: an offset in
  the index is the byte its event begins at, and a sealed segment holds
  its events' lines and nothing else. The cut takes the index before
  the segment and never a commit line that is whole; a segment shorter
  than its index commits is refused as damaged (lakelands#618).
- **The index is part of the log, and a segment without it is damage**
  (lakelands#865). The order of the writes tells a crash from a loss. A
  crash leaves an index shorter than its segment, or a lake's first
  segment with no bytes and no index yet; both are read. No
  crash leaves a segment with lines and no index file, a later segment
  with none (a seal writes the next index before the segment comes to
  exist), or a `projection.db` with no segment file (the projection is
  made after the first segment). Each is a lake that lost its index or
  its log: the engine answers `damaged`, naming the segment
  (`index_missing`) or that there is none (`no_segment_file`), at the
  question for the log's ends and at every open, before anything is
  read, cut or made. A lost index is an operator's restore; nothing
  rebuilds it. One case nothing in the files tells apart: an index file
  that is there and was shortened by hand reads as a crash's tail.
  - Each pair of files is looked for in the reverse of the order a
    making writes it in: the projection before the segments, a
    segment's lines before its index a second time. So beside a lake
    that is being made nobody is told of damage.
  - The segments follow each other: the first begins at position 0,
    each later one at the position after the last of the one before
    it, and its first event names that one's hash. A seal leaves them
    so; a log that is otherwise lost a sealed segment or holds another
    log's, and is refused the same way (`position`, `prev_hash`). An
    open reads one line more of each later segment's index for it.
  - A directory whose making stopped before its first segment holds no
    lake. Only an open to create makes anything in it, and an open that
    is refused makes nothing: no directory, projection or lease.
- **A segment after the first comes to exist with its checkpoint in
  it.** The format says every later segment begins with a `checkpoint`,
  and a verifier of the directory holds it to that at whatever instant
  it reads: an empty later file, or one that begins with another event,
  is a directory it refuses. So closing a segment and writing the first
  event of the next are one step of the engine, which holds whole or
  leaves the log as it was, after a failure and after a crash. Two steps
  (close, then append the checkpoint) left a lake whose next write
  headed the new segment, and after its next checkpoint no export of it
  verified or could be imported, on every engine. Here the next
  segment's index is written whole and synced, then its lines under a
  name nobody lists (`log/<first>.jsonl.partial`), and the rename to
  `log/<first>.jsonl` is the step; what a crash before it left is read
  by nobody, written over by a seal at that position and removed by the
  next writer. PostgreSQL does it in the seal's one transaction. The
  store's `verify` refuses a segment after the first whose first event
  is no checkpoint, so the rule is held against any writer; a lake
  written before this that holds such a segment is named by it and not
  mended (lakelands#623).
- `lease` holds the last fencing token handed out and its holder, under
  an operating-system lock. The lock makes a second writer fail at once
  and dies with its process; the token, read from the file on every
  append and every seal, is what refuses a writer that lost the lease.
- A handle opened read-only creates nothing and writes nothing: it opens
  the projection read-only, and the store folds only on the writer's
  handle.
- `projection.db` holds the projection with its kept history. It is
  derived from the log and can always be folded again.

### Every lake, whatever its engine, exports these files through its store

Every lake, whatever its engine, exports these files through its store:
`lakelands_core::export::write(store, dir)` (`lakelands export --to
<dir>`) reads the log through the change feed and writes
`log/<first position>.jsonl` exactly as above, one file per sealed
segment and one for the active run after the last checkpoint (a lake
never checkpointed exports the one file `00000000000000000000.jsonl`).
Every event is in the release's one format version
(`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`), as
the lake holds it; nothing is rewritten.

- An export is the log as the host reads it: every event, every
  principal's writes, retired records' and restricted records' history
  included, and no field masked. Handing over an export hands over the
  whole lake, what no principal of it may read included.
- It reads through the head as it stood when it began, so a lake being
  written exports a consistent prefix, and a later export has the
  earlier one's sealed files byte for byte.
- It writes only into an empty or absent directory, never over an
  earlier export. An export that fails removes what it wrote, so no
  partial export is left to pass for a shorter log; and a feed that
  answers a position out of turn fails it.
- It reads the feed a page at a time and writes each line as it comes,
  and a verifier reads a line at a time, so neither holds the log in
  memory.
- Beside `log/` it writes `export.json`, a manifest of the lake, the
  position and hash it read through, its one format version, and each
  file with its range and digest. It is a convenience for a reader and
  is never trusted: a verifier recomputes everything from `log/`.
- An export verifies without Lakelands' engines:
  `lakelands_core::export::verify(dir)` and `verify.py <dir>`, which
  shares no code with Lakelands, both read only `log/` and answer the
  last position and hash, or the position where the files stop being
  the log. The same holds for a SQLite lake directory.

### An export opens as the lake it came from

`lakelands_store::import` opens a new lake from an export, on any engine
(lakelands#215). It verifies the whole export first, then checks what
only an import needs: every event belongs to the lake it makes, and
every fence is the one its writer held. Nothing is written before both
pass. It then appends every event unchanged, position for position,
through the engine's `import`, which keeps each event's own fence. It
seals a segment with the first events of each file after the first, in
the engine's one step, and compares the digest the engine takes with
the file's inside that step: one that differs stops the import before
anything is sealed. Last it folds the projection as a rebuild does, and
appends nothing. The lake's head is the export's, and an export of it
writes the same files. The host then opens it as any other lake, and
that writer's lease is greater than every fence in the log. An import
goes only into a lake that holds nothing, so it is a lake's birth and
never a merge. One that fails after it began writing names the position
it reached and leaves a true prefix of the export, which is not the
source and refuses another import. The host removes it and imports
again.

## Why

The log is the truth (001), so the truth should be readable without the
program that wrote it. With the segments as plain files, an auditor can
check a lake's hash chain and its checkpoints with a script of a few
dozen lines, and a lake can be moved or archived by copying files.
Because every event re-serialises to its stored bytes and every
checkpoint names where a sealed segment ends, the change feed carries
everything these files hold. So the export is one function over the
store, working the same on every engine and over a client reaching a
lake across the network, and costs no method on the engine seam.
Keeping the members in a separate index keeps the segment files pure
while still letting the engine return the members without parsing the
bytes, and the commit line gives multi-event appends the all-or-nothing
property a file system does not.

A lock alone does not make a lease safe: a process can hold a lock it
believes it lost, or lose one it believes it holds. A token that only
grows, checked on every append, stops a stale writer whatever its lock
says. A lock alone would also outlive a crashed writer if it lived in a
file's contents; the operating system's lock does not.

## What it is not

- It is not a promise that the index or the projection can be read by
  another program. They are the engine's; only the segments are the
  format. The projection is derived and a rebuild makes it again; the
  index is not, and goes into a backup with the segments.
- It is not a replication scheme. Copying a directory while a writer
  appends can catch an uncommitted tail, which the next writer cuts off.
- It is not a requirement on other engines. An engine that keeps events
  elsewhere keeps their bytes unchanged and can export these files.
- An export is not a backup of the projection, which folds again from
  the log; and not a replication scheme: it is a prefix read once, and
  nothing follows the lake afterwards (the change feed does that).

## How it came to be

- The first version: the SQLite lake directory, with its segment files,
  its index and commit lines, its lease and its projection.
- Every lake exports its log as these files through its store, with a
  manifest beside them, and an export verifies without Lakelands'
  engines.
- An export opens as the lake it came from, on any engine
  (lakelands#215).
- A writer that goes on after a failed append cuts off what it left;
  the cut takes the index first and never a whole commit line
  (lakelands#618).
- A segment after the first comes to exist with its checkpoint in it;
  before, closing a segment and writing the next one's checkpoint were
  two steps, and an import sealed before each checkpoint
  (lakelands#623).
- An export's events are in the release's one format version and the
  manifest names that one; before, each event kept the version it was
  written in and the manifest gave the lowest and the highest
  (lakelands#801).
- Part of a lake without its events is damage: a segment without its
  index file, a projection with no segment file and segments that do
  not follow each other are refused before anything is read, cut or
  made. Before, a directory that had lost its index or its segment
  files was read as holding no log, and where the index was lost an
  import replaced its segment and an open to create cut it; segments
  that did not follow each other were opened as a log, and no open
  asked. A directory whose making stopped before its first segment is
  no lake to any open but one to create; before, an open to read
  answered `no_projection` there and an open to write made a
  projection in it (lakelands#865).
