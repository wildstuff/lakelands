# 099 — Work beside the writer runs in parts

Decided on lakelands#1649, lakelands#1651 and lakelands#1652, for 0.23.0; the operator's
choices are on lakelands#1578.

## What is decided

**Proposed** on lakelands#1652, for 0.23.0, one design with lakelands#1649 and
lakelands#1651, lines 11, 12 and 13 of epic lakelands#1578. It answers part 3 of the
brief and stands on `docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`. It
amends 039, 041, 049, 057, 058, 065, 066, 076 and 081. No code lands with it.

**A breaking change, inside 098's: no data migration, and no lake to carry.**
**Every change of format, stored table and public type this record makes:**

- **Log format 23 gains two kinds:** `act_whole`, which ends an act, and
  `source_purged`, which begins a purge, so a purge can be owed.
- **An act is several appends.** `values_removed`, `source_forgotten` or
  `source_purged` first, retirements and closes in appends of at most
  `limits.part_events`, `act_whole` last. A feed reader sees an act arrive in parts;
  until now one append held it. `act_whole` names the act's first position.
- **Stored tables.** None is added. The acts' lists hold one more stage, and the
  `chosen.<type>` mark of 041 grows to all three settings: `building`, `dropping`,
  and a kept view's cursor per source.
- **Public types.** `Limits` gains `part_events` (500), `part_tail` (1 000) and
  `page_events` (2 000), and `order_chunk`'s default falls from 10 000 to 1 000. A
  report of remove, forget and purge gains `through`, its position, and its id leaves out the head. An act's report
  gains `taken_since`, what it took that its report did not count. `reclaimed` and
  `reclaim_owed` keep their names; `reclaimed` now answers what the reclaim's turns
  gave back. `RemoveReportStale`, `ForgetReportStale`, `PurgeReportStale`, `PlanStale`
  and `ActUnfinished` stay. A new refusal, `view_building`: a set or a trigger over a
  view that is not whole.
- **`compact` leaves the act.** No act runs a full `VACUUM`. `lakelands compact`
  stays, as maintenance.
- **A setting's write no longer walks its type,** so a setting of `sort_index`,
  `lookup_index` or `view_kept` no longer costs its build in one turn.
- **`choose_order` no longer refuses a type with `sort_index`.**
- **A write reads through an owed act,** and a read does not. A write sees what the
  restate will leave; a read answers the old value until then, as 065 says.
- **lakelands#1649's headline changes** unless the operator chooses otherwise: open
  choice 5. An act's turn holds the writer longer than an ordinary write.

Claims are marked **measured**, **read** (in the code at `48383421`, and at `e943b945`
for the seams of the first turn and the restate) or **reckoned**, as in 098. A number
`way` gives, `scoped_ms` and `rebuild_ms`, is the store's forecast, never a timing, and
is called that here. Acts ran on a file-backed SQLite lake with a key store, each delivery
sealed apart, release build, load 4 to 11 on a shared machine, one delivery of 1 000
records, driven by `crates/lakelands-cli/examples/act_bench.rs`, the split by timers
in a scratch build. Acts were not split on PostgreSQL: its rows below are read, and
R1 to R5 measure them.

### Where a small act's time goes

Milliseconds, one delivery of 1 000 records, measured:

| Records | Lake | Preflight | Act | Walk | Append | Keys | Rebuild | Swap | Compact |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 100k | 107 MB | 2 175 | 7 277 | 2 036 | 177 | 9 | 4 770 | 106 | 153 |
| 500k | 541 MB | 10 836 | 46 690 | 10 914 | 368 | 19 | 33 919 | 221 | 1 217 |
| 1M | 1.1 GB | 31 934 | 140 664 | 29 152 | 431 | 70 | 95 773 | 997 | 14 117 |
| 2.5M | 2.6 GB | 89 489 | 270 927 | 52 942 | 615 | 15 | 207 976 | 561 | 8 743 |

A set of 99 deliveries at 100k: preflight 3 019, act 48 752, of which rebuild 35 235.
This is lakelands#1649's 236 to 320 s, measured again at 271 s.

- **Remove:** the rebuild is about 75% and the walk about 20%. The append and the keys
  are nothing. The rebuild happens because `scrub::plan` refuses a scoped pass to a
  lake that holds any removal (read). That reason can go only with the seam named
  under Restate.
- **Forget, scoped, today:** the walk, then `drafts` and `plan`, which take 135 and 151
  s at 2.5M. That time is not the act's: it is a **planner pick**. `SELECT DISTINCT at
  FROM routes WHERE key IN (?) AND ty IN (?) AND at BETWEEN` takes `routes_ty` where it
  should take `routes_key`. With `+ty` it took 41 µs against 0.1 to 8.4 s (measured).
  That is step F1. It makes `docs/hosting.md`'s claim that the time follows the source
  true, which at `48383421` it is not.

Forget with F1 in, measured, at 2.5M, load 4: act 37 618 ms = walk 25 905, drafts 36,
append 93, keys 12, plan 24, restate 2 079, compact 9 463. Its preflight, 44 998. At 1M:
act 19 467, walk 10 626, compact 6 073. The 118 and 147 177 that run printed for its pass
and for a rebuild are forecasts: `way` is computed before the pass (read,
`lake.rs:1295`), and on a fresh handle is events over `REBUILD_RATE`, 17 000 (read,
`engine-sqlite/src/lib.rs:64`), so 2 016 events give 118 at any size. No rebuild was
avoided on a clock. The timers are under Restate.

- **Purge** ends in no `finish_act`, and appends retirements alone. Measured at 2.5M
  with F1: preflight 36 024, act 14 604, of which `selection` 14 391 under the writer
  and append 200.

So parts alone do not cure this: a part that rebuilt would pay the rebuild. Three
things must leave the writer: the **walk**, the **rebuild** and **`compact`**. What is
left under it is bounded already.

### One road

Every act and every build is the same job, with its walk on a view and the writer
taken only for bounded turns:

1. **Walk,** on a view at position P, off the writer: `exec::run`, as a preflight. It
   ends with what the restate needs: the selected keys, the plan (`scrub::plan`) and
   every act told (`remove::told`). None of these is paid under the writer.
2. **Catch up,** off the writer, while the tail past P is longer than a turn may read.
3. **Turns,** each one `exec::write` entry, each held to a limit, each leaving the lake
   whole. What each is bounded by, and where that stops, is under lakelands#1639 below.
4. **Restate,** in pages or beside.
5. **Sweep and reclaim,** in turns.

The sibling is `choose.rs`' `Start`, `Fill` and `Keep`. A runner spawned as
`choose_orders` is drives each job, and takes the next turn only when the last has
ended, so a write sent meanwhile queues in front of it. FIFO does the rest: **a write
waits for at most the turn in front of it**, plus the writes in front of it, as any
write does. The state of a job is in the log and the acts' lists, not in the runner:
a runner that dies loses nothing, and the writer's open starts it again without
holding the open up; today's open finishes an owed act before it answers.

**Acts that overlap.** `found` takes an earlier act's events into `inside` and into what
is left, and leaves them out of its counts (read, `remove.rs:265-270`), but only for an
act it knows. Two walks of one source that did not know each other would each see the
other's events as outside, so a version both selected whole is never destroyed; a record
they emptied together is not retired; and what both selected is counted twice. So:

- **Walks of one source run one at a time, in the order the acts came.** An act's walk
  starts on a view past the first turn of every earlier act of its source, with those
  among `earlier`. It waits for that walk and that first turn, not for parts. An act of
  another source walks beside it, since it touches none of the same keys.
- **Parts run one job at a time, oldest first.** A later act's first turn does not wait
  for them, so an erasure waits for at most one walk of its own source. Its parts do
  wait, for the earlier act's restate, and re-judge on restated state: the later act
  retires a record the two emptied, skips one already retired, and counts what its own
  parts did.

### An act's turns

- **The first turn** reads the tail, judges the keys again over it (below), appends the
  act's event, destroys the keys that remain and gives a source its new version. It
  walks no selection. So **erasure happens
  in the first turn**, at a position, as soon as it can, and the rest is restating. Destroying
  the keys took 9 to 70 ms (measured); a slow key store holds this turn, as it holds the
  act today.
- **Parts.** Retirements and closes follow in appends of at most `part_events`, 500,
  each its own turn. A part re-judges its keys under the writer, at O(part): a head
  written after the act keeps its record. From 177 to 615 ms for 2 016 events, one part
  takes 45 to 150 ms (reckoned, loaded).
- **`act_whole`** ends the appends, emitted always, so a reader of the log alone tells
  an act that is whole. A purge gains `source_purged`, an act event of its own, and
  takes the same road; until now it had none, and a crash between its parts could not
  be told from a smaller purge.
- **The door answers when the act is whole,** with time spent off the writer. A caller
  who drops does not stop the job. An earlier answer was decided against (lakelands#1578, 2026-11-01): the door answers when the act is whole.

### A report is read at its position, and the log does not move behind it

Today remove, forget and purge hash the head, so any write stales them: the second
removal of lakelands#1649. A log is append-only: **what a selection selects through a
position does not change.** So a report states its `through`, and its id hashes the
selection, `through` and the positions it selected there that no earlier act selects, and
never the head. `plan_id` is the
sibling.

Two questions are then separate, and neither walks under the writer:

- **From `through` to the walk's view P, on the view.** The walk recomputes the id over
  the selection, which is its work anyway, and reads the tail below at any length. No
  writer is held.
- **From the walk's view P to the first turn, under the writer.** The check reads at
  most `part_tail` + 1 rows past P from `routes_source (source, at)` for writes by the
  selected sources, and from `routes_ty (ty, at)` for other writes on selected keys,
  tested against a `HashSet` of the keys the walk built. Both indexes exist, on every
  engine, behind the seam's `routes`. Routes hold no `sealed`, so the selected sources'
  tail events are read from the log as well, by `read_these`: 11 ms for 1 000 events at
  2.5M in today's lines (measured, load 3 to 6), and on 23 one inflate a block touched
  (098). It costs O(tail) and nothing of the selection. More than `part_tail` rows: the
  writer is let go and the view moves up. After three tries with nothing appended, the
  act answers unavailable, and no lake that writes slower than a view moves meets that
  (`ACT_TAIL_OVERRUN_MOVES_THE_VIEW`).

**The tail moves the keys.** `found` decides `keys` by `inside` and `outside` over all
the source's events, as the longest prefix of its versions wholly inside (read,
`remove.rs:301-339`). Under the writer that judgement is final. Here it is out of date by
the tail, in both directions:

- A tail write of a delivery the act did not select, sealed under a version the walk saw
  as wholly inside. Destroying that version shreds a value the act did not select. This
  is data loss, and it cannot happen today, because the walk is under the writer.
- A taken-in tail event sealed under a version outside `keys`. It is only redacted, so
  `shredded: true` and `unshredded` would answer wrong.

So the job keeps the walk's `inside`, `outside` and tally by key, and the first turn
folds the tail into them: a selected event's `sealed` goes to `inside` and the tally, any
other event of the source to `outside`. It then cuts the prefix again, judges `apart`
again, and recounts `shredded` and `unshredded`. With `shredded: true` and anything
unshredded, the act answers `RemoveNotShredded` and appends nothing. All of it is
O(tail + versions), under the writer, before anything is destroyed.

**What "reaches" means for each act.** A write the selection's own rule selects is
*taken in*, so it is never left behind; a write that changes what the report said about
a key goes stale; the rest does nothing.

| Act | Selection | Taken in | Stale | Nothing |
|---|---|---|---|---|
| Forget | every event of its sources | any write by a selected source; a source's writes are refused after the first turn (058) | no write | everything else |
| Removal by `deliveries` or `seen_before` | the source's events its rule selects | a tail event of that source for which `Removal.selects(e)`, O(1): a delivery that grew | a write by another source on a selected key | any other write. Another delivery of the source selects nothing, and its `sealed` still goes to `outside`, so it can cut the keys (above) |
| Removal by `written_before` | the source's events before a time. `before` is frozen at the walk today (read, `remove.rs:274-278`) | a tail event of that source whose `known_at` is before the time. The first turn extends `before` over the tail it reads, O(tail), since `known_at` never falls | as above | as above |
| Purge | what the source brought to a type, not shared | a write by the source into the type | a write by another source on a selected key: it is shared now, and a purge refuses shared records (039) | any other write |
| An earlier act of the source, against a report read before it | what the earlier act selected leaves this report's selection | nothing: the walk starts past it, with it among `earlier` | what the two both select: the report counted it, and the id over what is left moves | a disjoint selection: the id does not move, so lakelands#1649's second removal passes, and a version both selected whole is destroyed by the later act |

A call still writing when a removal takes its delivery takes a new key, for writes
after the act's first turn, which the act did not select (065, stands). A later
delivery under the same id has a key of its own. So everything before the first turn
is judged by the rule, with `written_before`'s `before` taken in that turn, and nothing
after it is selected: there is no gap for a write between preflight and act. The act's
event is the line.

A selected write is taken in and does not stale the report (decided, lakelands#1578, 2026-11-01); the
row that it is never left behind holds.

**Plan, declaration and migrate can follow,** and each reaches differently:

- **Plan.** Selected keys and versions (`plan_id`, read) plus, in the tail, records of
  the type whose `where` now matches. It reaches when a selected key moves or a record
  comes to match. A record of another type does not (081). `enact`'s re-selection moves
  to the view, and the check is as above, through `routes_ty`.
- **Declaration.** A declaration or a migration of a named type or a dependent, or a
  setting it names. Record writes do not reach, unless a check it reports reads data;
  then that type's writes do. The check is one read of the ontology's position.
- **Migrate.** Any write to the type, since it rewrites the type: `routes_ty` in the
  tail. `report_id` and `value_report_id` leave the head.
- **Proposals'** `id_head` take the same id function.

They hash the same head today, and that is all they share. Plan, declaration and migrate
stay in one step and can be cut.

### Restate: scoped first, then beside

Timed in a scratch build with the seam below and F1 in, one delivery of 1 000 records on
the 2.5M lake of the first table, three acts in a row, load 3 to 6:

| Step | ms | Where |
|---|---:|---|
| Walk, as its preflight | 49 838 to 53 253 | on a view |
| Plan, its keys from the walk | 3 to 4 | on a view |
| Told, one to three acts; 1 077 on a reopened lake of about 7 000 records | 0 to 2; 45 | on a view |
| One page, 256 records, 512 events | 136 to 155 | a turn |
| All four pages | about 600 | four turns |
| The lists and the sweep of bases | 1 025 to 4 543 | one transaction today (below) |
| Whole act | 56 779 to 61 259, against 270 927 by the rebuild | |

Planned by a second walk of the source and without F1, the plan took 164 275 to 168 055
and the act 221 399 to 229 647: the keys must come from the walk. The forecast for this
pass said 118 where its pages took about 600. A forget's pass, timed in another throwaway
copy, took 107, 151 and 178 ms at 20k, 100k and 300k, its pages 25 to 59 ms, where the
forecast says 118 at all three. So the forecast has the order of magnitude and no more, and
a forget's pass grows with the lake. A removal's page did not, over these runs: 130 to 146 ms
on a lake of about 6 000 records, 136 to 155 at 2.5M.

- **The scoped pass extends to removals, through a seam of its own.** `redacted` folds a
  page only by the acts the fold was told, and a removal plans nothing today. With only
  the reason cut from `scrub::plan`, in a throwaway copy, an erased value sealed under a
  version another delivery keeps still read `SECRET` after the act, and after a reopen
  and a scoped forget of another source on the same record. There are two causes.
  `finish_forgetting` plans only by owed sources (read, `lake.rs:1290-1294`). `w.removals`
  is told only in `rebuild_held` and the beside build (read, `fold.rs:1368`, `open.rs:240`),
  so a reopened writer holds none, and a page refolds what an older act took. That second
  cause is a forget's too. So:
  - **A plan by an act's selected keys.** The walk visits them anyway, so they come from
    it. The first turn adds the keys of what the tail took in, and a page reads its
    positions in its own turn, through the current head.
  - **Every act told before the first page, for a forget as for a removal.** `remove::told`
    is one log read and one `Ontology::at` per act, so it is O(acts), not O(page):
    45 ms at 1 077 acts (measured, on a lake of few declarations), so about 105 ms at
    lakelands#1649's 2 499 (reckoned), more with more declarations. It runs once per
    writer, in the walk, on the view, and its result is merged into `w.removals` in a
    turn; the fold adds each act after it. An act `taken`
    pushed when it met a missing key has no `fixed` (read, `fold.rs`), so the merge
    replaces it. `Ontology::at` can be shared by the acts between two declarations.
  - **`redacted` stays as it is.** It tests every event against every act (read), so a
    page is O(events × acts). One page of 512 events took 130 to 146 ms with one act and 89
    to 99 at 1 077, unpaired, load 3 to 4: nothing to see at that count. A walk's `earlier`
    is the same test over all the source's events: a small act on that lake took 0.28 s
    over the first 250 acts and 0.57 s from the 750th to the 1 000th, off the writer. R4
    holds no index for this. If its numbers ask for one, acts by source and delivery make
    both O(1) an event.

  Measured with this seam, on SQLite, the value under a retained version read erased after
  the act, after a reopen and after a scoped forget of another source on the same record,
  and the delivery kept beside it read whole. That is a scratch, on one engine: R4 holds
  it in the suite. So 208 s of rebuild falls to about 0.6 s of pages at 2.5M.
- **A kept view's reason goes too,** with 098's heads: a restated older version changes
  no head, and a retired head moves the view through the fold. A removal can also restate
  a head's value, and that has to move the view: a page applies `note(old, new)` for each
  head it changes. Reckoned, R4's to prove; if it does not hold, the reason stays and that
  job goes beside.
- **Where a shape still needs a rebuild** (a set, a vector, a link relation, a computed
  field through a reference, a rename, a page over `page_events`, or
  `limits.forget_rebuild`), the job runs it
  beside: `build_ahead`'s scratch, off the writer, then catch-up passes, then the last
  tail and the swap in one turn. Measured at 1M, ingesting throughout, load 4 to 8: first
  pass 710.3 s, second 1.40 s for 1 500 events, last tail and swap 1.25 s under the writer.
- **It never falls back.** `rebuilt_beside` folds in place today when the engine may not
  swap (`Swapped::Busy`, read). On this road a busy swap leaves the act owed, and it is
  tried again. A scan in place is what this record removes.
- **Restates coalesce.** A rebuild takes every act in the log when it starts, so
  lakelands#1649's 2 499 deliveries, 96 minutes, pay one pass, however many acts are owed.
  An act whose first turn falls after a pass began is not in what that pass folded, since
  catch-up folds only events past it. It stays owed past the swap, and restates then.

Write latency around that run, n writes:

| | n | p50 | p99 | max |
|---|---:|---:|---:|---:|
| before it | 67 | 181 ms | 2.84 s | 3.07 s |
| during the passes | 6 214 | 56.5 ms | 817 ms | 7.67 s |
| the swap and after | 84 | 49 ms | 1.23 s | 2.18 s |

No write waited for the rebuild, and the 7.67 s is not explained: the same machine showed
3.07 s with nothing beside. R5 runs this on a quiet machine and states it. Catch-up takes
base / (fold rate − write rate), so a lake written as fast as it folds never catches up
and stays owed, which `describe` says.

### The sweep of bases leaves its transaction

Read: after every scoped pass `put_lists(swept)` runs `drop_states(StatesOf::Bases)` in one
transaction with the lists (`lake.rs:1425`), a `NOT IN` over every record's state
(`projection.rs`, `drop_bases`). Measured at 2.5M under the writer: 1 025 to 4 543 ms, the
longest step this record's first draft left under it, and it grows with the lake. It is
step 5's first part: the unused bases are listed on a view, and deleted in chunks of
`order_chunk`, each a turn that tests its ids against the records written past that view.
A base holds provenance and no value (read, `engines.rs`), so erasure does not wait for
it. Chunks are reckoned at the cost of a build's.

### `compact` leaves the act

Read: `finish_act` ends in `reclaim`, a full `VACUUM` under the writer, `Situation::Busy` if a
reader pins it. On SQLite it cannot be cut. It does not need to be.

| Engine | Between an act and storage given back |
|---|---|
| SQLite, after a swap | nothing: the new file holds no erased byte (read), so a `VACUUM` after it erases nothing |
| SQLite, after a scoped pass | `secure_delete`, set on every connection that deletes (098), zeroed the pages at the delete; then one turn truncates the WAL, 0.07 to 0.13 s measured; then `incremental_vacuum` turns of 10 MiB, 15.6 ms measured |
| PostgreSQL | a plain `VACUUM` off the writer: it takes `ShareUpdateExclusive`, beside reads and writes. Only the `writing()` check at `handle.rs:1272` ties it to the lease, and it goes. pgvector's repair runs with it. |
| Memory | `Reclaimed::nothing()`, as today |

A full `VACUUM` stays as `lakelands compact`, for an operator's window. A reader pinning
the WAL leaves `reclaim_owed`, as 058 says. Measured: compact took 1 to 10% of a removal
and 25 to 30% of a scoped forget.
Every lake of 23 has `auto_vacuum` in its file and `secure_delete` on its connections, so
this holds without a rewrite.

What leaves a host's reach is what left it before: a WAL archive, a disk snapshot, a backup
(058).

### What a reader sees between turns

- **Between the first turn and its restate,** a read answers the old value, as it does
  today for the length of a rebuild (065). A scoped pass makes that seconds. Beside a
  rebuild it is the rebuild's length.
- **Retirements land part by part,** so a count moves while the act runs.
- **A poll stops** before the first reaction the projection cannot decide yet, while any
  act is owed, not only while a rebuild runs (065).
- **`key_missing`.** A version the act names seals nothing from the first turn, and the
  source's new version comes in that turn. Only a crash inside it leaves 065's case, which
  stands: the next open, a repeat or the rebuild's finish destroys the keys.

### A write reads through an owed act

Writes land in that same time, for every act. 065's rule for an act a crash left owed
becomes the rule for every act, and for writes it does not hold as it stands. Read: a verb's
`assigned()` and `requires` read the head (`writes.rs:2303`, `2184-2188`), and `assigned`
puts what it read into a patch, so into the log. A `function` or `embed` field recomputes
from unrestated state, so it can call a backing with an erased value and seal the output
under a destroyed key (`backed.rs`, `input_of`). Today that window opens only after a failed
act. This road opens it on every act, for seconds by pages and minutes beside a rebuild.

**While an act is owed, a write reads what the restate will leave.** Where the record a write
takes from the projection has a field by an owed act's source, and for a removal an owed act
selects the event at that field's `meta.position`, the write takes the record from a refold
of its own events through `redacted`. That is a page of one key, kept in memory, not stored.
Verbs, `requires`, a computed field's inputs and ingest's old state all come through that one
seam. So no erased value goes back into the log, no backing is called with one, and no output
is sealed under a destroyed key.

The cost is one test per record while nothing is owed by its source, O(fields), and on a
record that is reached, its versions at up to 2 ms each on 23 (reckoned, from 098's block
read). It needs only the acts in the lists, so it holds from the writer's open, before the
runner has walked again. A read does not do this: it would pay the test on every row of every
page, so it answers old values, as 065 says. A trigger's firing over an old value is held by
the poll rule above. The row is `WRITE_READS_THROUGH_AN_OWED_ACT`.

100 changes this for a guard. `stands` keeps a conjunct that reads `preceding` or
`in_force`, so the poll rule does not hold it: both resolvers read through this seam, and
`preceding`'s half lands in R2 whatever becomes of 100.

### Builds of `sort_index`, `lookup_index` and `view_kept`

Measured today under the writer, contended: general 300k, `lookup_index` 2.59 s,
`sort_index` 3.92 to 8.03 s; tick 1M, `sort_index.tick = ["at", "seq"]` 211.1 s and
`view_kept.tick_h1` 79.7 s. So a row costs 80 to 210 µs, and 1 000 rows, the new
`order_chunk`, 80 to 210 ms (reckoned). 041's 10 000 gave 160 ms on its smaller rows.

- **All three take `Start`, `Fill`, `Keep`.** The setting's event appends a mark, and the
  fold no longer walks the type. Each `Fill` is a turn over a key range. The mark
  `building` holds a cursor; a write for a key at or before the cursor goes to the rows in
  its transaction, and one after it needs nothing yet. This is 041's rule, and 057's rows
  are no different.
- **A read takes the old road until `Keep`.** 041 and 057 decide from the declared set;
  here from the mark, never from which rows exist. Never a part-built index or view. A
  read answers as before, and in the same words.
- **The door answers when the build is whole,** as an act's. A small type finishes in its
  first turn, so nothing changes there. `describe` shows `building`.
- **Taken back:** the setting appends and `Drop` deletes in chunks, each a turn, marked
  `dropping`, which no read looks at. Today `Drop` deletes in one step.
- **`choose_order`'s refusal goes.** `eligible` refuses a type with `sorted_fields` (read),
  because a setting's write and a chosen build could not share the rows. They share one
  road now, field by field: a field the host has set is the host's setting, and a chosen
  one goes when the setting comes.

**`view_kept` is the hard case.** A group's state folds from its members, and a write can
move a member from a built group to an unbuilt one.

- **A cursor per source,** in key order. Each `Fill` folds a range into staged groups
  (`Work.note`), under the writer.
- **A write beside the build** applies `note(old, new)` to the sides at or before the
  cursor, and skips what is after. A member that moves from the built side to the unbuilt
  leaves on one side, and is reached on the other when the cursor gets there. So each
  member is counted once, at its head, when the build ends. Folds are commutative (049),
  so the order does not matter.
- **Extremes** read order entries, which `view_order` holds for keys up to the cursor. A
  member at or before it that loses an extreme finds its next holder among them. Nothing
  after the cursor has counted yet.
- **A view over a view** starts after its inner view's `Keep`.
- **Reads compute** until `Keep` puts `whole` at a position, as they do for a view with no
  kept state (049).
- **A set or trigger over a view** (066, `watches_view`, lakelands#1185) reads the fold, so
  it may be declared only over a whole view: `view_building`. The set's remake stays in its
  declaration's append, and `max_set_changes` holds. Read: a set needs its view kept
  already (`view_kept_for_set`), so a set is never over a view from before its build.

### What stands between parts

| Between parts of | Act | Build |
|---|---|---|
| A restart | owed in the log; a write reads through it from the open; the runner re-walks off the writer, retires what remains, appends `act_whole` | the mark holds the cursor; the runner goes on, or from 0 on a kept view, whose stage is memory |
| Another act of the source | its walk starts past this act's first turn; its parts wait for this act's restate, then re-judge on what that left | a build's rows follow its retirements, as below |
| A setting taken back | no bearing | `Drop` in parts; a late `Fill` sees the mark gone and ends |
| A declaration that changes the type | parts re-judge by key under the current ontology; a type gone is skipped | back to `Start`, since a row's form may have moved |
| A removal beside a build | a build's rows follow the fold, since retirements are events; after a swap the scratch is whole, and the mark goes | same |
| An open | a read-only open reads old values and reports it owed; a writer's starts the runner and does not wait | same |
| A repeat of an owed act | answers when the job does; appends no second act | not a thing a caller repeats |
| An export | a copy may hold an act without `act_whole`; that is the log | the setting is in the log and the mark in the projection, so a copy carries the setting alone |
| An import | its writer finishes what the log owes | rebuilds its settings whole, by 041 |
| A rebuild from the log | folds an act and its retirements as appended, and makes a setting whole at its event | whole at once, no mark |
| `verify` | passes: it reads the log, in which an act without `act_whole` is well-formed | passes |

### Recommendations with a size, and no design

| Work | Road? | Recommendation | Product / spec. at ×1 |
|---|---|---|---:|
| Aging: one turn across every rule, a whole-type scan per rule (read) | yes: each move is an idempotent write with its `aging_id`, so a range is a part | after 0.23.0 unless a host meets it; the runner fits as it stands | 120 / 120 |
| Preflights that take the writer and append nothing: `migrate`, `migrate_value_type`, `plan` (read, `doors.rs`) | they need only a view, as remove's | in R3, since staleness moves them anyway; the cheapest gain here | 60 / 80 |
| Declaring and changing a type | yes, with a mark as above, where a declaration walks | later, on its own record: it changes what a read can assume | 300 / 300 |
| Recomputing a computed field | yes: a `Fill` | later, with the above | 150 / 150 |
| A `vector_index` build, 7 min at 100k of 1 536 dimensions under the writer (read, `docs/hosting.md`) | yes | later; named so the road's bound is not read as covering it | 200 / 200 |

### What lakelands#1639 can take

**A write waits for at most one turn,** and each turn is held to a limit:

| Turn | ms, this machine, loaded | Held to | Grows with |
|---|---:|---|---|
| A part | 45 to 150, reckoned | `part_events` | its events |
| First turn: tail and keys judged | 11 for 1 000 events, measured, then the key store's 9 to 70, measured | `part_tail` | the tail, the source's versions, the key store |
| A page | a removal's 89 to 155, a forget's 25 to 59 on smaller lakes, measured | 256 records, `page_events` | its events, `wide`, `walks`; a forget's also with the lake |
| A chunk of the sweep | as a build's, reckoned | `order_chunk` | its chunk, the tail |
| A build's chunk | 80 to 210, reckoned | `order_chunk` | its chunk |
| A reclaim's turn | 15.6 to 130, measured | 10 MiB | nothing else |
| A seal | 700, reckoned (098) | `seal_after_bytes` | its segment; any write pays this, act or not |
| Last tail and swap, on a rebuild beside | 1 250 and 1 960, measured | the catch-up | the tail |

Three things hold that table to what it says:

- **A page is not always small.** It refolds `plan.wide`, and a migration walks the records of
  the `near` types once a page (read, `scrub.rs:452-459`, `Plan.walks`), so on a migrated lake
  a page is O(type). `page_events` bounds what `Plan.events` gives a page, and a job over it
  takes the rebuild beside instead, which is a reason in `plan.way`.
- **`scrub::plan` is O(source)** where it walks a forgotten source's events for kinds, `sealed`
  and outputs (read). It is in the walk, on the view, and shares that walk's pass. It is never
  a turn.
- **The sweep of bases** is in chunks, above.

So the bound it can state is **about 300 ms a turn, and about 2 s where a rebuild's swap or a
seal falls**, at the sizes above. The numbers are `limits`, and R1 measures them on a quiet
machine. That is longer than an ordinary write, which the operator accepted (lakelands#1578, 2026-11-01). lakelands#1639's act
takes this road as an act that retires. It gets its parts, its `act_whole` and its restate from
R1 to R5. The operator chose no-go on the series form (098), so it is designed on records.

### The memory engine

It keeps its log, leases and every state, and it has no scratch: `Lake::scratch` answers
`Swapped::Kept` and folds in place, under its lock (read). The road runs on it by the same
store code, so all the rows below run on it. It promises no bound: nothing it holds outlives
the process, and its compact is nothing. Blocks reach it through the store (098).

### Which lines of the three issues stand

lakelands#1649:

- **The headline changes.** "Must not stop the lake's other writes for longer than an ordinary
  write does" is 22 ms median and 85 ms slowest there. A page here takes about 150 ms at 2.5M,
  and the stated bound is about 300 ms a turn and about 2 s at a seal or a swap. Reason: a
  turn does work a write does not, and a seal and a swap are one step each. Smaller limits take
  the ordinary turns towards 85 ms at the price of more turns; they do not take the seal or the
  swap there. Open choice 5 decides.
- **Ask 1, the walk off the writer,** stands.
- **Ask 2, bounded parts,** stands, and it does not cure the measured cost alone: the
  scoped restate and the rebuild beside do. Named here because that is where the 236 to 320
  s goes.
- **Ask 3, stale only when a write reaches the selection,** changes: a write the selection
  selects is taken in; a report goes stale only on a write that changes what it said about a
  key. Reason: a forget of a source still writing would never pass, and "never left behind"
  is what the addendum asks. Decided by the operator (lakelands#1578, 2026-11-01).

lakelands#1651:

- **All four needs** stand, and the done-when lines, with its table at 1M and 10M on each
  engine, which R7 and R8 measure. This record has no 10M number.
- **"Not asked: two builds at once"** stands as a bound: the runner takes one at a time.

lakelands#1652: every line stands. Its "each kept change has a sized issue" is the leader's
when this is accepted, one issue per step of 098 and of this record.

### The order of the work

After 098's F1 to F12. Each row's pull requests are of at most 600 lines at ×1.35.

| Step | What | Product | Spec. | PRs |
|---|---|---:|---:|---:|
| R1 | The runner, the tail read, `limits`, a job's state | 350 | 280 | 2 |
| R2 | Remove, forget, purge in parts: walk off the writer, `act_whole`, `source_purged`; the tail's judgement of the keys; acts that overlap; a write reads through | 640 | 800 | 4 |
| R3 | Staleness on all acts, preflights on a view, plan, declaration, migrate | 200 | 250 | 1 |
| R4 | Scoped restate for removals: keys from the walk, acts told once; the kept reason; `page_events` | 320 | 400 | 2 |
| R5 | The rebuild beside, with no fall back; an act after a pass began | 340 | 340 | 2 |
| R6 | The sweep and the reclaim in turns; PostgreSQL off the lease | 200 | 220 | 1 |
| R7 | `sort_index`, `lookup_index` builds; `Drop`; `choose_order`'s refusal | 350 | 350 | 2 |
| R8 | `view_kept` builds | 450 | 500 | 3 |
| R9 | `view_building` for sets and triggers | 80 | 120 | 1 |
| | **Sum** | **2 930** | **3 260** | **18** |
| | **×1.35** | **3 956** | **4 401** | |

R2's product is 400 for the parts, 80 for the tail's judgement, 40 for overlapping acts and 120
for reading through. R4's is 60 for acts told once and merged, 60 for a plan on the view, 120
for pages as turns, 60 for `note` on a head and 20 for `page_events`; the scratch that proved
the seam was 55 lines without any of the turns. These are reckoned.
R2 cannot land without reading through: its first turn opens the window on every act.

R1 first; R2 stands on it; R4 and R5 may go in either order; R6 follows them. R7 stands on R1,
R8 on R7 and 098's F9 to F11, R9 on R8. Record 097's step 8 stands on R7; its other steps land
beside this list after F12.

### The estimate, and what is cut first

With 098:

| | Product | Specification |
|---|---:|---:|
| 098, ×1.35 | 2 430 | 1 971 |
| 099, ×1.35 | 3 956 | 4 401 |
| Together | 6 386 | 6 372 |
| Removed by 098's F2 | −599 | −2 498 |
| **Net** | **5 787** | **3 874** |

At ×2, which record 097 says this round's pull requests ran: 8 861 product and 6 942
specification net. The net at ×1.35 is what the raise is sized on, and record 097's reserve
does not hold it. A raise lands by `docs/budgets.md` with its row, each step's size measured.

Cut in this order, with what each costs:

1. **098's F12, provenance once:** 162 and 162. Costs 380 B a tick and the 2.3× page, on a type
   with derived fields, until lakelands#1636.
2. **R8 and R9, `view_kept` builds:** 716 and 837. A kept view's setting stays under the writer,
   79.7 s at 1M, and lakelands#1651 is met for indexes alone.
3. **The plan, declaration and migrate half of R3:** about 135 and 170. They stay on the head.
4. **R5, the rebuild beside:** 459 and 459. Costs only shapes the scoped pass cannot do, which
   then hold the writer as today.
5. **098's derived `id`:** about 50 and 60. Costs 23 B a tick of log.

Not to cut: F1, R1, R2, R4, R6, and 098's F9 to F11, which are the gains the table turns on.

### Conformance rows

The act:

- `ACT_WALKS_OFF_THE_WRITER`: a write sent during an act's walk answers before the act's first turn.
- `ACT_APPENDS_IN_PARTS`: no append of a large act holds more than `part_events`.
- `ACT_WHOLE_ENDS_THE_ACT`: every act's last event is its `act_whole`, purge included.
- `ACT_ERASES_IN_ITS_FIRST_TURN`: from it, the act's versions seal nothing and the source has a
  new one.
- `FORGET_LEAVES_NO_WRITE_BEHIND`: a write by the source between preflight and act is never left
  behind by a forget.
- `REMOVE_TAKES_A_DELIVERY_THAT_GREW`: an event of the delivery between preflight and act is taken.
- `REMOVE_LEAVES_A_LATER_DELIVERY`: a later delivery under the same id is not.
- `REMOVE_WRITTEN_BEFORE_JUDGED_AT_ITS_FIRST_TURN`: a tail event before the time is taken, one
  after it is not, whatever the walk saw.
- `REMOVE_TAIL_WRITE_KEEPS_ITS_VERSION`: an unselected tail write sealed under a version the
  walk saw wholly inside keeps that version, and its value reads.
- `REMOVE_TAKEN_IN_UNDER_A_KEPT_VERSION_IS_NOT_SHREDDED`: `shredded: true` refuses with
  `RemoveNotShredded` and appends nothing; without it, `unshredded` counts the event.
- `PURGE_TAKES_A_WRITE_SINCE_ITS_REPORT`, by the source, into the type.
- `PURGE_STALE_ON_A_SHARED_KEY`: another source's write on a selected key stales it, and no record
  is purged.
- `REMOVE_STALE_ON_ANOTHER_SOURCES_WRITE`: likewise.
- `REPORT_NOT_STALE_BY_A_WRITE_ELSEWHERE`: for remove, forget, purge, plan, declaration and migrate,
  each with its own write that does not reach.
- `REPORT_STALE_WHEN_REACHED`: plan, declaration and migrate, each with its own write that does.
- `ACT_RESUMES_AT_OPEN`: killed between any two turns, the writer's open finishes it, and the lake
  equals one whose act was never killed.
- `ACT_REPEAT_FINISHES_AND_APPENDS_NOTHING`.
- `ACT_BETWEEN_PARTS_EXPORTS`: an export between parts imports, and its writer finishes the act.
- `ACT_BETWEEN_PARTS_VERIFIES`: `verify` passes between any two turns.
- `ACT_REBUILD_SAME_LAKE`: a rebuild from the log between any two turns equals its projection.
- `POLL_STOPS_BEFORE_AN_OWED_ACT`.
- `ACT_TAIL_OVERRUN_MOVES_THE_VIEW`: a tail past `part_tail` lets the writer go and moves the
  view up; after its tries the act answers unavailable and has appended nothing.
- `ACTS_OF_ONE_SOURCE_OVERLAP`: two acts sent together, selecting overlapping and then
  disjoint deliveries under one version: the version is destroyed, a record the two emptied is
  retired, each event is counted once, and a report read before the first is stale only on the
  overlap.
- `WRITE_READS_THROUGH_AN_OWED_ACT`: with the restate held back, a verb's `assigned`, a
  `requires`, and a `function` field see what the restate will leave; no erased value reaches
  the log or a backing, and no output is sealed under a destroyed key.
- `TWO_ACTS_RESTATE_ONCE`: two owed acts cost one rebuild.
- `RESTATE_SCOPED_FOR_A_REMOVAL`: a value under a version another delivery keeps reads erased
  after the act, after a reopen, and after a scoped forget of another source on the same record;
  the result equals a rebuild.
- `RESTATE_TELLS_EVERY_ACT`: a scoped pass on a reopened lake leaves every older act's values
  erased.
- `RESTATE_PAGE_OVER_ITS_BOUND_GOES_BESIDE`: a page over `page_events` on a migrated lake.
- `ACT_AFTER_A_PASS_BEGAN_RESTATES_AFTER_THE_SWAP`.
- `SWEEP_IN_TURNS`: no base a record names goes, and a write naming one during the sweep keeps it.
- `ACT_NEVER_FOLDS_IN_PLACE`: a busy swap leaves it owed.
- `RECLAIM_IN_TURNS`: an act runs no `VACUUM`, and `reclaimed` says what came back.

The build:

- `BUILD_READ_ANSWERS_AS_BEFORE`: at every part, each kind of setting.
- `BUILD_NEVER_READ_PART_BUILT`: the rows of a build are not read until its `Keep`.
- `BUILD_WRITES_GO_ON`: a write sent during a build answers within one turn.
- `BUILD_WRITE_REACHES_BOTH_SIDES`: writes before and after the cursor, in the whole build.
- `BUILD_RESTART_FINISHES`.
- `BUILD_SETTING_TAKEN_BACK`: ends clean, between any two parts.
- `BUILD_TYPE_DECLARED_AGAIN`: starts again.
- `BUILD_BESIDE_A_REMOVAL`.
- `BUILD_REBUILD_SAME_LAKE`.
- `BUILD_EXPORT_IMPORT_SAME_LAKE`.
- `VIEW_BUILD_MEMBER_MOVES_ACROSS_THE_CURSOR`: a member that changes group across the cursor is
  counted once.
- `VIEW_BUILD_EXTREME_LOSES_ITS_HOLDER`.
- `VIEW_BUILD_OVER_A_VIEW`: starts after its inner view's `Keep`.
- `SET_OVER_A_BUILDING_VIEW_REFUSED`: and a trigger.
- `CHOSEN_ORDER_BESIDE_SORT_INDEX`.

### Decided by the operator

Decided on 2026-11-01 (lakelands#1578): every recommendation below was taken.

1. **The series form: no-go.** In 098, with its numbers.
2. **A write a selection selects, between report and act.** Taken in, or stale. Both leave nothing
   behind. Stale makes a forget of a source that still writes wait for a quiet moment that may
   not come. **Taken in, decided**, with `taken_since` saying what the report did not count.
3. **When an act answers.** When it is whole, or at the first turn, with erasure done and a
   restate owed. The first gives a caller one answer, after minutes at worst; the second holds no
   request open, and a caller then needs a status. **When it is whole, decided:** a caller
   that drops loses nothing, and a repeat answers where the job stands.
4. **When a setting's write answers.** When the build is whole, or at once with `building`
   in `describe`. **When it is whole, decided,** as with acts; a large type's operator may drop
   the call, and the build goes on.
5. **lakelands#1649's headline against this bound.** No longer than an ordinary write, 85 ms at
   worst, or about 300 ms a turn and about 2 s at a seal or a swap. To hold the headline, a seal
   would hash at the append and keep a running digest, reckoned at 60 and 60 lines; a swap's last
   tail would need more catch-up passes and could shrink but not vanish, since the next write needs
   the new file; and pages and parts would be a quarter of their size, with four times the turns.
   **The stated bound as `limits`, with the headline of lakelands#1649 restated to it, decided:** a seal already
   holds any write for 0.51 s today (098), a swap falls only on a shape the scoped pass cannot do,
   and the turns are `limits` a host can lower. R1 measures them on a quiet machine first.

## Why

- **The log is append-only, so a report has a position to stand on.** Hashing the head treated
  every write in the lake as a change to what a report read. Almost none is.
- **One road, because two would differ at every edge in the table above.** An act and a build
  walk, check, take turns and finish; only what a turn does differs.
- **The writer's cost is what stays bounded when the lake grows.** Every step left under it is
  held to a limit, within the limits of the turn table, and every step that grows with the lake
  or the acts runs on a view, where 076 says reads run. A migrated lake's page goes beside.
- **A swap is the one turn that cannot be cut,** and it is measured at 1.25 s. Everything else
  is a choice of chunk.
- **A planner pick looked like a design problem.** The scoped forget's 287 s was one index
  chosen wrongly; a measured split found it, and only a split could.
- **Erasure stays first.** Keys go in the first turn, and everything after it restates what
  no key can read any more. The price is that the keys are judged twice, at the walk and over
  the tail, and a write has to read through what is still owed.

## What it is not

- **Not two writers.** 076 holds: a job's turns are the writer's turns.
- **Not a query planner of its own.** F1 is one unary plus.
- **Not parallel builds.** One at a time, as lakelands#1651 says.
- **Not a promise on a machine like the one measured.** The bound is `limits` and turns; each step
  measures its own.
- **Not the design of lakelands#1639.**
- **Not a change to what a forget takes,** what the log holds or what `verify` promises.
