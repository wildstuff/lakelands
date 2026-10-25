# 076 — A lake takes writes from many processes through its one writer

## What is decided

**Accepted** on lakelands#1377, by the operator on 2026-10-24. Nothing of
it is built in 0.19.0: it comes in force with the issues that build it
(below), and until then a process that does not hold a lake's lease
writes it only through a network door of the process that does.

### Many callers already write one lake; many processes write it through its writer

A lake has one writing process, holding the lease and its fencing token
(`docs/decisions/008-one-writer-per-lake.md`), and one hash-chained log
in one order
(`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
Two things are told apart here:

- **Many concurrent callers through the one writing process.** This
  works today: any number of callers, in the writer's process or over
  its network doors, write at once, and group commit
  (`docs/decisions/073-record-writes-share-one-append-in-a-batch.md`)
  shares one append among the writes queued together.
- **Several processes that each write the lake.** Parallel intake, users
  and agents writing at once from processes of their own: this is what
  a host means by several writers to one lake. Each such process is
  given the ability to write, and its writes go through the lake's one
  writing process, which decides and appends them. No second process
  writes the log or the projection.

What one writing process takes, measured on 2026-10-24 at develop
`401b952ad`, release build, on one machine of 16 cores that other work
kept at a load average of 11 to 15, SQLite lakes on its local disk. Each
caller writes one record of a plain type at a time, waiting for its
answer before the next, with group commit at its default bound of 32
events. SQLite is the store's own harness (`throughput` in
`crates/lakelands-store/tests/batched/mod.rs`, run from the SQLite
engine's `tests/batch.rs`). The memory engine ran the same harness from
a call made for this record, and several lakes at once ran in a test
made for this record, with the same writes; neither is in the tree, so
those two columns cannot be run again from it. Each is the
range over three rounds, the memory engine's over four. PostgreSQL's row
is the mean of three rounds from the run of lakelands#1358 on the same
machine, which landed group commit.

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
  SQLite, which no batch shortens; many callers are what fill a batch,
  wherever they run.
- **The fold is not what bounds a disk engine.** The memory engine runs
  the same deciding and the same fold, without a sync or a database to
  write, at three to six times SQLite's rate: of SQLite's 200 µs, about
  150 are the append and the projection's writes.
- **Lakes add up.** Eight lakes take five times one lake's rate, each
  with its own writer and its own sync.

What forwarding costs a write is not measured here: it adds a hop to the
writer and back, and the writes it carries batch at the writer as any
others do. The issue that builds it measures it.

### Several processes writing one lake directly are not taken

Each of these holds because one process writes:

- **One chain and one order** (020). Every event names the hash of the
  one before it; two appenders would fork the log or need an order
  agreed between them before either appends.
- **Deciding against the current state** (073). A write is decided over
  every write before it: a reference names a live record, a link's
  cardinality counts the other links, a set of a kept view moves with
  its group, a grant with `only` reads the record. A second decider sees
  a state that may already be stale.
- **The fold in log order.** One projection is folded in the log's
  order, and every answer names one position of it
  (`docs/decisions/009-every-response-carries-its-position.md`).
- **The lease and its fence** (008). One holder at a time, and a stale
  holder refused at its next append; several holders would need a
  fence per batch, not per process.
- **SQLite is one file on one machine.** Its lock and its directory live
  where the lake's files are; a writer on another machine has nothing to
  write to.

Forwarding gives every process the ability to write and touches none of
them, which is why it is the way taken.

### A handle without the lease forwards each write to the writer

A handle opened for reading, in a process beside the writer's, or a
remote client, forwards each write to the lake's writing process, which
decides and appends it; the caller gets the writer's answer and its
position.

**What exists.** `lakelands-client`'s `RemoteLake` is already the remote
half: every write door of the contract as its REST route, the principal
resolved by the server from the key the client holds for it, the
server's grants judging the write, a refusal answered as the contract's
own error, ingest as a stream, and the host operations `Unbound`. A
program anywhere writes a lake so today. What it does not carry is a
write context: REST sends none, and the serving host's context is the
one that holds (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).

**What the forwarding handle adds** is one handle for a host process
that reads the lake from its own engine and writes it through the
writer:

- **How it finds the writer.** By configuration: the lake's location
  names the writing process's door, and for failover its standbys', in
  an order. Nothing discovers a writer; the handle connects only where
  it was told to. A process that answers a forwarded write saying it
  does not hold the lease is passed over for the next one named.
- **Who acts.** The caller's principal, named by its id. The writer
  resolves it from its own principals and judges the write by its own
  grants; nothing the forwarder holds of the principal is trusted. A
  forwarded write carries its write context (origin, depth, cause) only
  over a link the writer's configuration names as a host of the lake,
  with a credential of its own; a principal's key over REST still
  carries none.
- **What it answers.** A refusal by the writer is answered as it is. A
  writer that cannot be reached before the write is sent is
  `unavailable`, and nothing was written. A writer lost after the write
  was sent is `unavailable` with the outcome not known: the handle never
  sends that write again by itself, and a caller that retries with the
  write's idempotency key is answered the first write's outcome if it
  landed (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`),
  whichever writer holds the lake by then.
- **Read-your-writes, by the answered position.** A read on the handle
  after its own write answers at a position no earlier than that write's
  answer named. The writer answers once the write is folded, and on
  SQLite and PostgreSQL the handle reads the projection the writer
  folded, so the position is there to be read; the handle waits for it
  within a bound and is `unavailable` past it, never answering from
  before its own write.
- **What stays the writer's.** The host operations that need the lease
  (`rebuild`, `checkpoint`, `age`, `purge_source`, `restrict`,
  `unrestrict`) are answered as on any handle opened read-only. A memory
  lake lives in its opener, so it has no second process to forward from;
  it is written over its doors as today.

### A standby follows the log and takes the lease when the writer is gone

A standby adds no writer: at every instant one process holds the lease.
It is a handle opened read-only, which folds nothing and reads what
the writer folded, as one beside a writer does today, and it tries the
lease. When the writer is gone the
standby takes it: its `lease_taken` carries a token greater than every
earlier one, it folds from the projection's mark as every writer's open
does (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`),
and it serves writes. The old writer's next append or seal is refused,
since the engine checks the token on each (008,
`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
Until it holds the lease, a standby answers a forwarded write as a
read-only handle does, so a forwarding handle passes over it.

**"The writer is gone" is the lease lock's answer, and nothing else.**
The operating system releases a file's lock when its process ends;
PostgreSQL releases the session's advisory lock when that session ends,
at once for a process that ends and, for a machine that is lost, when
the server's TCP keepalive gives up on the connection. A writer that
hangs but lives keeps the lease, and no timeout takes it away: a file
lock cannot be broken from another process, and the one engine-neutral
way out is that the writer's process ends. Its supervisor or its
operator ends it; `/readyz` (`docs/rest.md` § Health and readiness)
tells whether a writer holds its lease, as its handle knows it.

**Where it works.** On SQLite, for a standby on the machine that holds
the lake's directory, whose operating system keeps the lock. On
PostgreSQL, for a standby on any machine that reaches the database.
Sealed segments shipped to another machine are a copy that verifies
there, not a standby: taking over on it is an import as a new lake,
without the writes after the last seal shipped.

### What it changes

- **020 and 030: nothing.** One process appends; the chain and its
  verification from the files alone are as they are.
- **008: its rule stands**, and its "standby writers and failover are
  outside this decision" is made concrete here: a standby is a reader
  that may come to hold the lease, one at a time. The issue that builds
  the standby rewrites that line to name this record.
- **The README's "Choose something else when"** says that a lake takes
  any number of concurrent callers, and that writes from several
  processes go through its one writer.

### B, C and D are deferred, and none is planned

**B — direct writers ordered by a database lock.** Any process would
write a PostgreSQL lake directly, taking a short lock in the database
for each batch and deciding against the shared projection; SQLite would
keep one writer. It is a design for one engine, and PostgreSQL is the
reference engine here, not one to optimise for. Under the lock the
deciding, the append and the fold stay one at a time, so it saves the
hop to the writer and little else. It comes back if a host names a
sustained rate of writes from several processes that forwarding does
not meet, with the hop measured as the bound, and with an order that
holds on every engine.

**C — optimistic deciding.** Writers would decide side by side against
a state that may be stale, and the append would refuse a write whose
reads moved since. It needs the set of reads kept for every write,
through every rule a write is held to, gives a conflict where today
there is none, and buys at most the deciding: with deciding and folding
free, SQLite still spends about 150 µs a write in its append and its
projection, so about 6 700 writes a second rather than 5 000. It comes
back if deciding, measured after append and fold run side by side, is
most of a write's cost.

**D — partitions of one lake.** Each partition would have its own
writer, lease and chain. A lake that lost a partition would still
verify, one position per answer would become one per partition, every
rule that reads across records (a live reference, a link's cardinality,
a set of a kept view, a re-key) would need its writers to agree, and the
change feed (`docs/decisions/004-the-change-feed-is-the-log.md`) would
split into feeds with no order between them. Several lakes, split the
way the host already thinks of its data, and a read across them
(`docs/decisions/075-a-read-may-cross-lakes-and-each-lake-answers-its-part.md`)
give what partitions would with every rule as it is. It comes back if a
host names a dataset it cannot split into lakes whose rate one writer
does not meet.

### Two issues build it

- **Forwarded writes** (lakelands#1394). The forwarding handle: the
  writer named in the location, the link between hosts that carries the
  write context, the answers above, read-your-writes, and the door
  checks run through a forwarding handle on SQLite and PostgreSQL. Most
  of its product is outside the core crates, in the client and the
  server; its checks and the read-your-writes wait are in them. Roughly
  1 000 to 1 500 product lines and 1 200 to 1 800 lines of
  specification: the REST client it builds on took 2 155 and 1 088
  (lakelands#118).
- **Standby and failover** (lakelands#1395). The standby's follow and
  takeover in the store, its configuration and its place in
  `lakelands serve`, takeover checks on SQLite and PostgreSQL, the old
  writer refused, and 008's line. Roughly 400 to 700 product lines and
  600 to 1 000 of specification: readiness, which reads the lease's state, took
  197 and 366 (lakelands#1379).

At develop `7ca9a668c` the core crates stand 16 product and 35
specification lines under their ceilings, so each issue cuts first and
raises by the rule of
`docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`, the
standby most of all.

## Why

One order is what lets a lake be verified from its files alone and every
answer be repeated at one position, and the writer is not the bound one
would give that up for: one SQLite lake takes about 5 000 writes a
second, most of it in the sync and the projection's writes that a second
writer would not share. What a host asking for several writers needs is
that every process may write, and forwarding gives that without a second
appender. A standby takes away the one writer as a single point of
failure, on the same lease and fence that already make a stale writer
harmless.

## What it is not

- **Not a ceiling on a host.** A host writes as many lakes at once as it
  holds, each through its own writer.
- **Not a second appender.** A forwarding handle and a standby append
  nothing; at every instant one process holds the lease.
- **Not discovery.** The writer and its standbys are named in
  configuration; nothing finds them on a network.
- **Not a takeover on a timeout.** A writer that hangs keeps the lease
  until its process ends.
- **Not a change in 0.19.0.** No code, no rule of 008, 020 or 030, and
  no row of the suite.

## How it came to be

- Proposed on lakelands#1377 (2026-10-24) as "A lake keeps one writer,
  and a dataset that outgrows it becomes several lakes".
- Rewritten the same day: the operator asked for several writers to one
  lake and chose forwarding through the one writer, with standby and
  failover; direct writers, optimistic deciding and partitions are
  deferred (lakelands#1377).
