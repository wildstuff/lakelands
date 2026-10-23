# 074 — A segment is sealed by size

## What is decided

**Accepted** on lakelands#1309 (2026-10-22), for 0.18.0.

### The writer seals the active segment past `limits.seal_after_bytes`

A lake's `Location` carries `limits.seal_after_bytes`, 256 MiB by
default. Where the engine reports its active segment's committed bytes
and they reach the bound, the host that holds the writer seals the
segment as `checkpoint` does: the same engine step, the same
`checkpoint` event, read by verify, export, rebuild and catch-up as any
other seal. At 0 nothing is sealed by size; a bound below 1 MiB is read
as 1 MiB, since every checkpoint lists every sealed segment and an open
reads the end of each.

### Only an engine that reports a size is sealed by it

`LakeEngine::active_segment_bytes` answers the bytes the active segment
has committed, as the handle last read them, with no I/O, or none. SQLite
answers from the byte end its handle keeps. PostgreSQL keeps the log as
rows, with no file to back up or move on its own, and memory keeps
nothing past the process: both answer none and are never sealed by
size. An engine that wraps another forwards the answer.

### A seal by size falls between two entries of the write queue

The writer weighs the segment after an entry of its queue has run and
before that entry is answered: after a write that runs alone, and after
a batch's shared append, never inside a batch
(`docs/decisions/073-record-writes-share-one-append-in-a-batch.md`).
Any append counts: a write, a batch, an ingest window, a maintenance
act, the `lease_taken` of a writer that took its lease again. A writer
that is behind, lost or without its lease does not seal, and the size is
weighed again only once the tip has moved.

A seal that fails never fails the write: that write is answered by its
own events. A seal refused before the engine's step has sealed nothing
and is tried after the next append; one that fails behind the step
stands, and the writer answers as after a `checkpoint` that failed there
(`docs/hosting.md`).

### A writer's open seals; a read-only open and an import do not

A segment already past the bound, as a lake never checkpointed has one,
is sealed by the writer's open, and its checkpoint is in the log and the
feed. A read-only open never seals. An import appends the export's
events and nothing else, so its head is the export's: it seals nothing,
the open that upgrades an export of an older format included. The
host's next writer open may.

### Where a seal by size falls is the store's own

The checkpoint takes a position, as one a host asks for does, so the
positions after it depend on where the bound was crossed, and with group
commit on where batches closed. Positions are the store's to hand out,
as an ingest window's are; no record and no read answers otherwise for
it.

## Why

A lake that is never checkpointed keeps one log file that grows without
end and changes with every append. A file backup of it cannot be
incremental, and there is nothing sealed to move to slower storage. A
seal by size saves no bytes; it makes incremental backup and tiering
possible for a host that never thought to call `checkpoint`.

The bound is the host's, in `Location`, because it says only when the
writer takes an act whose result is in the log for every later host to
read the same: how this host spends its disk, its backups and its
pauses. As a lake setting it would travel with an export to an engine
where it means nothing, and with a restored lake to a host whose disk it
does not describe. The seal is the store's act, since the store makes
every event; the engine only reports a fact about its storage.

The seal hashes the whole active segment under the writer, so its pause
grows with the bound: about 2 ms a MiB on SQLite, 0.51 s at the default.
The floor keeps a small bound from sealing on nearly every append.

## What it is not

- It is no bound in time or in events, and no seal by size on
  PostgreSQL or memory.
- It moves no segment to other storage and is no backup command.
- It changes no record and no read, and no answer of a write: the write
  that crosses the bound is answered as it would be without the seal,
  after it.
- It does not seal inside a batch, on a read-only handle, or in an
  import.
- It makes no position equal to a serial run's: the batched rows and
  the soak run with the bound at 0.
