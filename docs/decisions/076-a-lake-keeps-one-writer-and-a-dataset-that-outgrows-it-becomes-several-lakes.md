# 076 — A lake keeps one writer, and a dataset that outgrows it becomes several lakes

## What is decided

**Proposed** on lakelands#1377 (2026-10-24). Nothing of it is built in
0.19.0, and nothing it decides needs code:
`docs/decisions/008-one-writer-per-lake.md` and
`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`
stand as written. Once the operator accepts it, the README's line on
many writers is rewritten as below.

### A lake keeps one writing process; neither partitions nor a leader of several writers is added

A lake has one writer, holding the lease and its fencing token (008),
and one hash-chained log in one order (020). This record weighed two
ways to give one lake several writing processes and takes neither:

- **partitions of one lake**, each with its own writer, lease and log;
- **several writers ordered into one log by a leader**.

A dataset that needs more writes than one writer takes becomes several
lakes, split the way the host already thinks of it (by company, by
client, by region), each with its own writer, and is read together by a
read across lakes
(`docs/decisions/075-a-read-may-cross-lakes-and-each-lake-answers-its-part.md`).

### What one writer takes, measured

Measured on 2026-10-24 at develop `401b952ad`, release build, on one
machine of 16 cores that other work kept at a load average of 11 to 15,
SQLite lakes on its local disk. Each caller writes one record of a
plain type at a time, waiting for its answer before the next, with group
commit
(`docs/decisions/073-record-writes-share-one-append-in-a-batch.md`) at
its default bound of 32 events. SQLite is the store's own harness
(`throughput` in `crates/lakelands-store/tests/batched/mod.rs`); the
memory engine ran the same harness; several lakes at once ran in a test
made for this record, with the same writes. Each is the range over three
rounds, the memory engine's over four. PostgreSQL is not measured again
here: its row is the mean of three rounds from the run of lakelands#1358
on the same machine, which landed group commit.

| callers | SQLite, writes/s | memory, writes/s | PostgreSQL, writes/s |
|---:|---:|---:|---:|
| 1 | 251–277 | 6 163–9 203 | 119 |
| 10 | 1 069–1 102 | 16 380–34 482 | 424 |
| 64 | 4 662–5 058 | 15 769–28 998 | 985 |
| 100 | 4 483–5 309 | 16 624–28 853 | 700 |
| 500 | 4 548–4 954 | 13 807–27 186 | 774 |

Several SQLite lakes in one process, 64 callers on each:

| lakes | writes/s in all | writes/s a lake |
|---:|---:|---:|
| 1 | 4 517–5 057 | 4 517–5 057 |
| 2 | 8 709–9 147 | 4 354–4 573 |
| 4 | 15 741–16 823 | 3 935–4 206 |
| 8 | 24 432–24 906 | 3 054–3 113 |

What the numbers say:

- **One SQLite lake takes about 5 000 writes a second from many
  callers**, about 200 µs of the writer's time a write, and PostgreSQL
  700 to 1 000. A single caller is bound by its own sync, about 4 ms on
  SQLite, which no batch shortens.
- **The fold is not what bounds a disk engine.** The memory engine runs
  the same deciding and the same fold, without a sync or a database to
  write, at three to six times SQLite's rate: of SQLite's 200 µs, about
  150 are the append and the projection's writes. A faster fold in the
  store's core would buy SQLite at most the other 50 µs or so of a
  write, a quarter; append and fold
  side by side, a later round, overlaps the two and is bounded by the
  larger.
- **Lakes add up.** Eight lakes take five times one lake's rate, each
  at about two thirds of a lone lake's on a machine that other work kept
  busy, each lake with its own writer and its own sync.

No host has named a design point for one lake on the record. A host
whose busiest dataset stays below these rates needs nothing of this
record; one above them splits that dataset into lakes. The question of
several writers for one lake comes back when a host names a sustained
rate for one lake that its engine's writer does not meet, measured again
after append and fold side by side.

### Partitions of one lake are not taken

Each partition would have its own writer, lease and chain, so
verification from the segment files alone
(`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`)
would hold for each partition, but no longer for the lake: nothing in a
partition's files says which partitions make the lake and at which head
of each, so a lake that lost a partition would verify, where a log that
lost a segment is refused at the first checkpoint that lists it (020).
Saying so would take a map of the partitions logged somewhere and read
by every verifier, a new part of the format.

What breaks beside it is every rule that reads the whole lake at one
write or one position:

- **One position per answer** (`docs/decisions/009-every-response-carries-its-position.md`)
  becomes one per partition, and so do `as_of`, a cursor, the session
  pin, and the order by `latest`
  (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
- **What a write is decided against.** A reference names a live record
  (`docs/decisions/032-a-reference-names-a-live-record.md`), a link's
  cardinality counts its ends' other links, a set of a kept view is
  decided at the write that moves its group
  (`docs/decisions/066-a-set-holds-a-kept-view.md`), a trigger reads
  across an edge, and a re-key moves a record to another key, so to
  another partition where keys choose it. Each, with its records in two
  partitions, needs both writers to agree at the write: a protocol
  across writers, or a lake whose rules stop at a partition's edge.
- **One shape.** A declaration or a migration changes every partition at
  once, so it is a write to all of them that holds or fails whole.
- **The change feed** (`docs/decisions/004-the-change-feed-is-the-log.md`)
  is the log; partitions give several feeds with no order between them.

A partitioned lake whose rules stop at a partition's edge is several
lakes that share a shape. Several lakes and a read across them give
that for `find`, with every rule as it is; a search, a walk or a pattern
across them is not given (075), and a reference or a link across the
edge partitions could give only with the protocol above. Until 075 is
accepted, a host reads each lake on its own.

### A leader ordering several writers into one log is not taken

A leader that appends what several writers send keeps one chain, so 020
and 030 hold unchanged. But the leader is a writer as today's is, and
today any number of callers, in the writer's process or over a network
door, already send writes to the one writer, which orders them (008).
A second writing process adds something only if it decides writes while
the leader decides others. Every write is decided over the writes before
it (073), so deciding side by side means deciding against a state that
may be stale and having the leader refuse, at the append, a write whose
reads have moved since: a conflict where today there is none, or a retry
by the store, and a set of reads to keep for every write, through every
rule a write is held to (grants with `only`, references, sets,
constraints, triggers). It buys at most the deciding, and the memory
measurement bounds that: with deciding and folding free, SQLite would
still spend about 150 µs a write in its append and its projection, which
stay one, so about 6 700 writes a second rather than 5 000. The projection, one per lake, is still folded in one order.

### Standby and failover rest on the same lease

A standby adds no writer: at every instant one process holds the lease.
It is a handle opened read-only that follows the log, as one beside a
writer does today, and takes the lease when the writer is gone. Its
`lease_taken` carries a token greater than every earlier one, so the old
writer's next append or seal is refused by the engine, which reads the
token on each (008, 030). "The writer is gone" is the lease lock's
answer: the operating system releases a file's lock when its process
ends, and PostgreSQL an advisory lock when the server sees its session
end, which for a lost machine is as soon as its connection's keepalive
says so. A writer that hangs but lives keeps the lease; telling it from
a slow one is the standby record's. The lease alone makes a takeover
safe wherever the lock and the token live beside the log: on
PostgreSQL, for a standby on any machine that reaches the database; on
SQLite, only on the machine that holds the lake's directory, whose
operating system keeps the lock. Sealed segments shipped to another
machine are a copy that verifies there, not a standby: taking over on
it is an import as a new lake, without the writes after the last seal
shipped. How a follower reaches the log, and when it
takes over, is the standby record's, in a later round; it needs nothing
of this one but the single writer.

One writer is what keeps a failover simple: the lease changes hands and
the log goes on in one order. Partitions would fail over one by one,
with the lake half-written between; a leader fails over as today's
writer does.

### What it changes

- **008** and **020**: nothing. **030**: nothing. 008's "standby
  writers" are standbys that may come to write, one at a time, as above.
- **The README's "Choose something else when"** keeps the line on many
  writers, said by the measure: one lake's writer takes about 5 000
  writes a second on SQLite from many callers and 700 to 1 000 on
  PostgreSQL; more comes from more lakes, read together by a read across
  lakes (075).

## Why

One order is what lets a lake be verified from its files alone and
every answer be repeated at one position. The measurements show that
the writer is not the bound one would give that up for: one SQLite lake
takes about 5 000 writes a second, most of it in the sync and the
projection's writes that a second writer would not share, and eight
lakes take five times one lake's rate, each at about two thirds of a
lone lake's on a busy machine. Splitting a dataset into lakes asks the host
to name the split, which a host that keeps one lake per company or
client already does; partitions inside a lake would make the store guess
it, and then pay at every write that crosses it.

A leader adds a protocol for what a writer does already. It would earn
its place only if deciding were the bound, and it is not.

## What it is not

- **Not a ceiling on a host.** A host writes as many lakes at once as it
  holds, each through its own writer.
- **Not a refusal of standby and failover.** They come with a record of
  their own, on the same lease.
- **Not the last word.** A host that names a sustained rate for one lake
  above its engine's writer, measured after append and fold side by
  side, reopens it.
- **Not a change in 0.19.0.** No code, no rule of 008, 020 or 030, and
  no row of the suite.
