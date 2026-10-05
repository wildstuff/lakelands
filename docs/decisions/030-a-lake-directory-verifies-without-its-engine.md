# 030 — A lake directory verifies without its engine

## What is decided

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
- `lease` holds the last fencing token handed out and its holder, under
  an operating-system lock. The lock makes a second writer fail at once
  and dies with its process; the token, read from the file on every
  append and every seal, is what refuses a writer that lost the lease.
- A handle opened read-only creates nothing and writes nothing: it opens
  the projection read-only, and the store folds only on the writer's
  handle.
- `projection.db` holds the projection with its kept history. It is
  derived from the log and can always be folded again.

Every lake, whatever its engine, exports these files through its store:
`lakelands_core::export::write(store, dir)` (`lakelands export --to
<dir>`) reads the log through the change feed and writes
`log/<first position>.jsonl` exactly as above, one file per sealed
segment and one for the active run after the last checkpoint (a lake
never checkpointed exports the one file `00000000000000000000.jsonl`).
Each event keeps the format version it was written in; nothing is
rewritten.

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
  position and hash it read through, the lowest and highest format
  version, and each file with its range and digest. It is a convenience
  for a reader and is never trusted: a verifier recomputes everything
  from `log/`.
- An export verifies without Lakelands' engines:
  `lakelands_core::export::verify(dir)` and `verify.py <dir>`, which
  shares no code with Lakelands, both read only `log/` and answer the
  last position and hash, or the position where the files stop being
  the log. The same holds for a SQLite lake directory.

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
  format.
- It is not a replication scheme. Copying a directory while a writer
  appends can catch an uncommitted tail, which the next writer cuts off.
- It is not a requirement on other engines. An engine that keeps events
  elsewhere keeps their bytes unchanged and can export these files.
- An export is not a backup of the projection, which folds again from
  the log; and not a replication scheme: it is a prefix read once, and
  nothing follows the lake afterwards (the change feed does that).

## Extended: an export opens as the lake it came from

`lakelands_store::import` opens a new lake from an export, on any engine
(lakelands#215). It verifies the whole export first, then checks what
only an import needs: every event belongs to the lake it makes, and
every fence is the one its writer held. Nothing is written before both
pass. It then appends every event unchanged, position for position,
through the engine's `import`, which keeps each event's own fence. It
seals a segment before each checkpoint and compares the digest with the
file's. Last it folds the projection as a rebuild does, and appends
nothing. The lake's head is the export's, and an export of it writes the
same files. The host then opens it as any other lake, and that writer's
lease is greater than every fence in the log. An import goes only into
a lake that holds nothing, so it is a lake's birth and never a merge.
One that fails after it began writing names the position it reached and
leaves a true prefix of the export, which is not the source and refuses
another import. The host removes it and imports again.
