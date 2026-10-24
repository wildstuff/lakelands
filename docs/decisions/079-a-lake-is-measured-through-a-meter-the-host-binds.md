# 079 — A lake is measured through a meter the host binds

## What is decided

**Proposed** on lakelands#1371 (2026-10-24); the operator decides at
review.

### The store tells numbers and keeps none

`Seams.meter` is the tenth seam, `lakelands_api::seam::Meter`: three
calls, `count` (a number that only goes up, told by how much), `level`
(a number told as it now stands) and `observe` (one value of a spread).
Each is handed a `Metered`, the lake's id and its engine's name, and one
number of a closed set: `Count`, `Level` or `Spread`, each a
`#[non_exhaustive]` enum whose `ALL` lists its numbers and whose
`name()` is the name a host exports it under. The store holds no
counter, no registry and no clock of its own for them; what is kept, and
how, is the meter's. With no meter bound nothing is told and no clock is
read: a span reads the clock only where a meter is there to be told.

The numbers are the lake's own, never a record's: no number is told per
type, per record, per principal or per field, and no value a lake holds
is ever part of one.

### A meter cannot part an append from its fold

A meter is the host's code, called from inside the writer's turn. Every
call is made under `catch_unwind`: a meter that panics loses that one
number, and the turn goes on as if none were bound. The counts of an
append, `lakelands_appends_total` and `lakelands_events_appended_total`,
are told only after its fold has committed, so a number never says an
append happened that the projection does not hold. A store test,
`crates/lakelands-store/tests/meter_panics.rs`, binds a meter that
panics on every call and shows every write answered and folded, the head
at the last append.

Every append of the writer, a checkpoint's included, goes through one
path that times it and counts its failure, so `lakelands_appends_total`
and the count of `lakelands_append_seconds` agree. A level that a
rebuild sets is set back however the rebuild ends, an unwind included.

A meter that panics still goes through the process's panic hook on each
call, which by default prints a message every time. A host's meter is
expected not to panic; the guard keeps the lake's turn whole when it
does, not its standard error quiet.

### Where each number is told

| Number | Kind | Told |
|---|---|---|
| `lakelands_events_appended_total` | count | after an append's fold, by its events |
| `lakelands_appends_total` | count | after an append's fold |
| `lakelands_append_failures_total` | count | when the engine answers an append with an error |
| `lakelands_writes_total` | count | after a batch of record writes is decided, by its writes; a write alone by one. Every decided write counts, whatever it answered: appended, refused, replayed or unchanged |
| `lakelands_seals_total` | count | after a seal's checkpoint is folded |
| `lakelands_queue_depth` | level | when a write joins the queue, and when a turn begins |
| `lakelands_tip` | level | when the writer publishes its tip |
| `lakelands_folded_through` | level | after each commit of the projection |
| `lakelands_rebuilding` | level | 1 when a rebuild begins, 0 when it ends, by an unwind as well |
| `lakelands_active_segment_bytes` | level | when the writer weighs its segment for a seal by size, on an engine that reports it |
| `lakelands_lease_held` | level | when the writer publishes: 1 with its lease, 0 without; 0 whenever the lease goes, given up by `close`, or lost or stale as the engine says |
| `lakelands_lease_fence` | level | with the lease held, its fence; 0 whenever it goes |
| `lakelands_append_seconds` | spread | around the engine's append, a checkpoint's included, where it succeeds |
| `lakelands_fold_seconds` | spread | around an append's fold |
| `lakelands_commit_seconds` | spread | around each commit of the projection |
| `lakelands_read_seconds` | spread | around a read that answers, from its view to its answer; a refused read is not timed |
| `lakelands_batch_writes` | spread | the writes a turn decided together, whatever each answered; a write alone as one |

Times are told in microseconds; a host exports them as it chooses.

### `serve` exports them on a listener of its own

`server.metrics.bind` in `lakelands.json` names an address. Where it is
set, `lakelands serve` binds a meter to every lake it opens and answers
`GET /metrics` (and `HEAD`) there in the Prometheus text format, 0.0.4: counters,
gauges and histograms, each line labelled `lake` and `engine`. A level
never told and a lake told nothing are not printed. The listener is not
the door: it asks no key and serves nothing else, answering any other
path 404 and any other method 405. A connection carries one scrape: it
is not kept alive, and one that has not been answered within ten seconds
is closed, so a slow or idle client holds nothing. Where the setting is
absent, nothing is measured and nothing more is listened for.

A scrape tells which lakes the server holds, on which engine, how large
their logs are, how fast they are written and read and whether each
holds its lease. It tells nothing of what a lake holds. It is bound to
loopback or to an interface only the operator's collector reaches.

## Why

An operator running a lake needs to see its write rate, its queue, how
far the fold is behind and how long an append takes, without asking the
lake through a door. A library that embeds the store must not get a
metrics library, a global registry or a listener with it, and a host
that already reports through its own system must not get a second one.
A seam gives both: the store says what happened, and the host keeps it
where it keeps everything else. The set of numbers is closed so that a
host can export each under a stable name and the store cannot grow a
number a host does not know how to keep.

The guard is there because a number is never worth a write. The writer's
turn appends, folds and answers in one step; a meter that panicked
between the append and its fold would leave the log ahead of the
projection until the next open. Catching each call costs nothing on the
path that does not panic, and telling the counts after the fold keeps
what is counted true.

The cost was measured on the release build with a write of one record
of a plain type at a time, the store with no meter, with one that does
nothing, and with the meter `serve` binds. On the memory engine a write
took 22.27 µs unmeasured, 22.47 µs with the meter that does nothing and
22.61 µs with `serve`'s at one caller, and 12.61, 12.59 and 12.56 µs at
sixteen. A timed span and a count take 35 ns on the meter that does
nothing and 47 ns on `serve`'s, and nothing with none bound. On SQLite
the three are within the noise of a commit's sync: 8.17, 8.12 and
7.91 ms at one caller, 1.10, 1.16 and 1.06 ms at sixteen.

Scrapes are served apart from the door because they answer to someone
else: a collector with no API key, on the operator's network. Putting
them behind the door's keys would make every collector a principal;
putting them on the door without keys would publish a lake's names and
sizes to every caller of the door.

## What it is not

- It is no tracing: a meter is told numbers, never a span's context, a
  record, a type or a caller.
- It is no health check; `/livez` and `/readyz` on the door say whether
  `serve` is up and its lakes open.
- The store neither pushes nor exports anything; only `serve` listens,
  and only where `server.metrics` is set.
- The scrape has no key and no TLS. It is bound where only the operator
  reaches it, or put behind something that asks for both.
- The buckets of `serve`'s histograms are fixed, 100 µs to 10 s for a
  time and 1 to 1 024 for a batch; they are not configured.
- A meter is not asked for anything and cannot refuse or slow a write
  but by the time its own calls take.
