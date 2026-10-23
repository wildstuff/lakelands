# 073 — Record writes share one append in a batch

## What is decided

**Accepted** on lakelands#1313 (2026-10-22), for 0.18.0; built on the
preparation of lakelands#1332.

### A batch is the store's own, and no answer depends on it

Record writes queued one after another are taken from the head of the
lake's write queue together and decided as a batch, which shares one
durable append and one fold. A batch is the store's own, as an ingest
window is
(`016 § A window is decided whole, and a record that changes nothing is a duplicate`):
no caller asks for one, sees one, or can tell which batch its write
landed in. The log a batch writes, and each write's answer as the wire
writes it, equal those of the same writes made one by one in the order
the queue took them, except where a stated exception below says
otherwise. A refusal belongs to its caller alone: a write refused in a
batch is answered as it would be alone, and the writes beside it go on.

Nothing waits for a batch to fill. A batch holds what was queued while
the append before it ran, so a lone caller's write is a batch of one and
pays what it paid alone.

### Only plain record writes batch, and a batch never passes one that runs alone

An `upsert`, `retire`, `link` or `unlink` batches when no type it writes
is `sys.setting` or `sys.aging_rule`, its context names no host cause,
and no type it writes has a field computed outside the lake; for a link
or an unlink, the link type is among the types it writes. Everything
else runs alone and closes the batch before it: verb calls and
proposals, ingest windows, settings, aging rules, declarations,
migrations and accepts, maintenance, the chunks of an order-index build,
writes of a type a backing computes, and the seal. The queue is taken in
order: a batch is the run of batchable writes at its head, and stops at
the first that runs alone.

### A batch closes when its events reach `limits.commit_window_events`

A batch closes after the write whose events bring it to the lake's
`limits.commit_window_events`, the bound of an ingest's window. A write
is never split, so a batch may pass the bound by its last write. There
is no bound in bytes. The two share the setting and count differently:
an ingest window counts the records it holds, a batch the events its
writes stamp. At 1, every record write is its own append again, and
every ingest window one record.

### Each write is decided alone, over the writes before it

The writes of a batch are decided one by one under the writer, each over
what the writes before it staged and nothing after it: its positions,
counters, idempotency map and kept views start where the write before it
left them, its checks of what moved under its caller run, and its
reactive depth and trigger ceiling are its own. A write's events are
stamped as it is decided and lie together in the log, in queue order.
A refused write puts back everything it staged, and the next is decided
as if it had not been made. A retry under an idempotency key the batch
already holds is answered by the first write, told under the retrying
caller's sight.

The writer is readied once, when a batch opens: it catches up, regains
its lease and records held answers then, and only a writer with nothing
to catch up or record opens a batch. Otherwise the write at the head
runs alone, as every write did before batching, and batching resumes
after it.

### No write is answered before the batch is durable

Every write of a batch is answered after the shared append and fold
return, each from a view at its own position: its last for a write that
appended, the batch's tip when it was decided for a replay.

### The exceptions

A batch's answers depend on its batch only here. Each exception has its
oracle in the rows every engine runs.

- **An append that fails.** Where the durable append fails, nothing of
  the batch is in the log, and every write of it is answered that
  failure, a refused write and a replay too. The lake writes again.
- **A fold that fails.** Where the events are durable and their fold
  fails, each write that appended is answered `not_folded` at its own
  first position, and the next write catches the projection up. A
  refused write keeps its refusal, decided over what is now durable; a
  replay, and any write without a position of its own, is answered
  `not_folded` at the batch's first position.
- **A panic in the shared phase.** Where the stamp, the append, the fold
  or the telling panics, every write of the batch is answered `stopped`.
  The writer recovers as after a fold that failed: the projection is
  rolled back, the writer is marked behind and reads the log's tip again
  as it catches up, so events the engine made durable before the panic
  are kept. The lease is left as it is. The writes queued behind still
  run, and the lake writes again. Where that recovery panics too, the
  record writes taken from the queue with the batch and not decided yet
  are answered `stopped` as well; the entries queued after them run.
- **A seal by size.** Where an engine reports its segment's size, the
  writer seals it after the entry that took it past
  `limits.seal_after_bytes`, at a batch's close
  (`docs/decisions/074-a-segment-is-sealed-by-size.md`), so the
  checkpoint's position, and every position after it, follows where
  batches closed. The batched rows and the soak run with the bound at 0;
  the row of the seal says only whether a lake seals.

## Why

A record write's cost is mostly durability. On SQLite most of its wall
time is in the append's syncs, on PostgreSQL in the append and the
projection's commit, so throughput stayed flat from 1 to 500 callers and
latency grew with them, while an ingest window, which already shares one
append and one fold, ran eleven to thirteen times as fast on the same
lakes. Sharing the append among the writes queued while the last one ran
buys that rate for many callers and costs a lone caller nothing.

Deciding each write alone over the ones before it is what makes the
batch invisible: every rule a write is held to reads the state it would
read alone, so the log and the answers are those of a serial run, and
the criterion can be checked as such, over a seeded generator, a soak
of callers racing, and each exception with a failure injected. The
exceptions are where a shared step fails: a failure of a step the
writes share is the failure of every write that shares it.

The bound of an ingest window is reused because it says the same thing:
how much one durable step holds.

## What it is not

- It is no timed window. A batch never waits for writes to come.
- It is no bound in bytes, and the bound does not count the two kinds
  alike: records for a window, events for a batch.
- It does not batch verb calls, ingest windows, declarations,
  migrations, settings or writes a backing computes.
- It changes no log format, no engine and no lease, and no reader sees
  a batch.
- It does not recover from a panic in a write that runs alone: that is
  answered `stopped` as before.

## How it came to be

- The first version: one append and one fold for the record writes at
  the queue's head, each decided alone, and three exceptions.
- A fourth exception: where a seal by size falls follows where batches
  closed (lakelands#1309).
