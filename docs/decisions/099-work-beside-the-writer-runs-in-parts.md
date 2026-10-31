# 099 — Work beside the writer runs in parts

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
- **Public types.** `Limits` gains `part_events` (500) and `part_tail` (1 000), and
  `order_chunk`'s default falls from 10 000 to 1 000. A report of remove, forget and
  purge gains `through`, its position, and its id leaves out the head. An act's report
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

Claims are marked **measured**, **read** (in the code at `48383421`) or **reckoned**,
as in 098. Acts ran on a file-backed SQLite lake with a key store, each delivery
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
  lake that holds any removal (read); nothing needs that (below).
- **Forget, scoped, today:** the walk, then `drafts` and `plan`, which take 135 and 151
  s at 2.5M. That time is not the act's: it is a **planner pick**. `SELECT DISTINCT at
  FROM routes WHERE key IN (?) AND ty IN (?) AND at BETWEEN` takes `routes_ty` where it
  should take `routes_key`. With `+ty` it took 41 µs against 0.1 to 8.4 s (measured).
  That is step F1. It makes `docs/hosting.md`'s claim that the time follows the source
  true, which at `48383421` it is not.

Forget with F1 in, measured, at 2.5M, load 4: act 37 618 ms = walk 25 905, drafts 36,
append 93, keys 12, plan 24, restate 2 079 (the scoped pass 118), compact 9 463; the
rebuild it avoided, 147 177. Its preflight, 44 998. At 1M: act 19 467, walk 10 626,
compact 6 073.

- **Purge** ends in no `finish_act`, and appends retirements alone. Measured at 2.5M
  with F1: preflight 36 024, act 14 604, of which `selection` 14 391 under the writer
  and append 200.

So parts alone do not cure this: a part that rebuilt would pay the rebuild. Three
things must leave the writer: the **walk**, the **rebuild** and **`compact`**. What is
left under it is bounded already.

### One road

Every act and every build is the same job, with its walk on a view and the writer
taken only for bounded turns:

1. **Walk,** on a view at position P, off the writer: `exec::run`, as a preflight.
2. **Catch up,** off the writer, while the tail past P is longer than a turn may read.
3. **Turns,** each one `exec::write` entry, each bounded, each leaving the lake whole.
4. **Restate,** in pages or beside.
5. **Reclaim,** in turns.

The sibling is `choose.rs`' `Start`, `Fill` and `Keep`. A runner spawned as
`choose_orders` is drives each job, and takes the next turn only when the last has
ended, so a write sent meanwhile queues in front of it. FIFO does the rest: **a write
waits for at most the turn in front of it**, plus the writes in front of it, as any
write does. The state of a job is in the log and the acts' lists, not in the runner:
a runner that dies loses nothing, and the writer's open starts it again without
holding the open up; today's open finishes an owed act before it answers. A lake runs
one job's parts at a time, oldest first, and a later act's first turn does not wait for
them, so an erasure never waits for another act.

### An act's turns

- **The first turn** takes the tail (below), appends the act's event, destroys the
  keys and gives a source its new version. It reads no selection. So **erasure happens
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
  who drops does not stop the job. An earlier answer is an open choice.

### A report is read at its position, and the log does not move behind it

Today remove, forget and purge hash the head, so any write stales them: the second
removal of lakelands#1649. A log is append-only: **what a selection selects through a
position does not change.** So a report states its `through`, and its id hashes the
selection, `through` and what it selected there, and never the head. `plan_id` is the
sibling.

Two questions are then separate, and neither walks under the writer:

- **From `through` to the walk's view P, on the view.** The walk recomputes the id over
  the selection, which is its work anyway, and reads the tail below at any length. No
  writer is held.
- **From the walk's view P to the first turn, under the writer.** The check reads at
  most `part_tail` + 1 rows past P from `routes_source (source, at)` for writes by the
  selected sources, and from `routes_ty (ty, at)` for other writes on selected keys,
  tested against a `HashSet` of the keys the walk built. Both indexes exist, on every
  engine, behind the seam's `routes`. It costs O(tail), about a millisecond (reckoned)
  and nothing of the selection. More than `part_tail` rows: the writer is let go and the
  view moves up. After a bounded number of tries with nothing appended, the act answers
  unavailable, and no lake that writes slower than a view moves meets that.

**What "reaches" means for each act.** A write the selection's own rule selects is
*taken in*, so it is never left behind; a write that changes what the report said about
a key goes stale; the rest does nothing.

| Act | Selection | Taken in | Stale | Nothing |
|---|---|---|---|---|
| Forget | every event of its sources | any write by a selected source; a source's writes are refused after the first turn (058) | no write | everything else |
| Removal | the source's events its rule selects: `deliveries`, `seen_before` | a tail event of that source for which `Removal.selects(e)`, O(1): a delivery that grew | a write by another source on a selected key | another delivery of the source, any other write |
| Purge | what the source brought to a type, not shared | a write by the source into the type | a write by another source on a selected key: it is shared now, and a purge refuses shared records (039) | any other write |

A call still writing when a removal takes its delivery takes a new key, for writes
after the act's first turn, which the act did not select (065, stands). A later
delivery under the same id has a key of its own. So everything before the first turn
is judged by the rule, and nothing after it is selected: there is no gap for a write
between preflight and act. The first act's event is the line.

Whether a selected write should instead stale the report is an open choice below; the
row that it is never left behind holds either way.

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

- **The scoped pass extends to removals.** `redacted(&w.removals, …)` already folds a
  page by the acts it knows (read), so `scrub::plan`'s reason "the lake holds an act of
  removal" goes. Each page of 256 records is its own turn, head unmoved. Measured,
  forget at 2.5M: 118 ms for 1 000 records, where the rebuild took 147 s. Reckoned for
  a removal, about the same: 208 s falls to under a second of turns.
- **A kept view's reason goes too,** with 098's heads: a restated older version changes
  no head, and a retired head moves the view through the fold. Reckoned, R4's to prove.
- **Where a shape still needs a rebuild** (a set, a vector, a link relation, a computed
  field through a reference, a rename, or `limits.forget_rebuild`), the job runs it
  beside: `build_ahead`'s scratch, off the writer, then catch-up passes, then the last
  tail and the swap in one turn. Measured at 1M, ingesting throughout, load 4 to 8: first
  pass 710.3 s, second 1.40 s for 1 500 events, last tail and swap 1.25 s under the writer.
- **It never falls back.** `rebuilt_beside` folds in place today when the engine may not
  swap (`Swapped::Busy`, read). On this road a busy swap leaves the act owed, and it is
  tried again. A scan in place is what this record removes.
- **Restates coalesce.** A rebuild takes every act in the log, so lakelands#1649's 2 499
  deliveries, 96 minutes, pay one pass, however many acts are owed.

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

### `compact` leaves the act

Read: `finish_act` ends in `reclaim`, a full `VACUUM` under the writer, `Situation::Busy` if a
reader pins it. On SQLite it cannot be cut. It does not need to be.

| Engine | Between an act and storage given back |
|---|---|
| SQLite, after a swap | nothing: the new file holds no erased byte (read), so a `VACUUM` after it erases nothing |
| SQLite, after a scoped pass | `secure_delete` zeroed the pages at the delete; then one turn truncates the WAL, 0.07 to 0.13 s measured; then `incremental_vacuum` turns of 10 MiB, 15.6 ms measured |
| PostgreSQL | a plain `VACUUM` off the writer: it takes `ShareUpdateExclusive`, beside reads and writes. Only the `writing()` check at `handle.rs:1272` ties it to the lease, and it goes. pgvector's repair runs with it. |
| Memory | `Reclaimed::nothing()`, as today |

A full `VACUUM` stays as `lakelands compact`, for an operator's window. A reader pinning
the WAL leaves `reclaim_owed`, as 058 says. Measured: compact took 1 to 10% of a removal
and 25 to 30% of a scoped forget.
Every lake of 23 has 098's creation settings, so this holds without a rewrite.

What leaves a host's reach is what left it before: a WAL archive, a disk snapshot, a backup
(058).

### What a reader sees between turns

- **Between the first turn and its restate,** a read answers the old value, as it does
  today for the length of a rebuild (065). A scoped pass makes that seconds. Beside a
  rebuild it is the rebuild's length. New here: writes land in that time, for every act.
  065's rule for an act a crash left owed becomes the rule for every act.
- **Retirements land part by part,** so a count moves while the act runs.
- **A poll stops** before the first reaction the projection cannot decide yet, while any
  act is owed, not only while a rebuild runs (065).
- **`key_missing`.** A version the act names seals nothing from the first turn, and the
  source's new version comes in that turn. Only a crash inside it leaves 065's case, which
  stands: the next open, a repeat or the rebuild's finish destroys the keys.

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
| A restart | owed in the log; the runner re-walks off the writer, retires what remains, appends `act_whole` | the mark holds the cursor; the runner goes on, or from 0 on a kept view, whose stage is memory |
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

**A write waits for at most one turn.** Each turn, on this loaded machine: a part 45 to 150
ms (reckoned); a page of a scoped pass 30 ms (reckoned from 118 ms for 1 000); a build's chunk
80 to 210 ms (reckoned); a reclaim's turn 15.6 to 130 ms (measured); a tail check under 1 ms
(reckoned); a seal 0.7 s (reckoned, 098); and, on a rebuild beside alone, last tail and swap,
1.25 s and 1.96 s measured. So the bound it can state is **about 300 ms a turn, and about 2 s
where a rebuild's swap or a seal falls**, at the sizes above; the numbers are `limits`, and
R1 measures them on a quiet machine. lakelands#1639's act takes this road as an act that
retires. It gets its parts, its `act_whole` and its restate from R1 to R5. It is designed after
the operator's choice on the series form (098).

### The memory engine

It keeps its log, leases and every state, and it has no scratch: `Lake::scratch` answers
`Swapped::Kept` and folds in place, under its lock (read). The road runs on it by the same
store code, so all the rows below run on it. It promises no bound: nothing it holds outlives
the process, and its compact is nothing. Blocks reach it through the store (098).

### Which lines of the three issues stand

lakelands#1649:

- **Ask 1, the walk off the writer,** stands.
- **Ask 2, bounded parts,** stands, and it does not cure the measured cost alone: the
  scoped restate and the rebuild beside do. Named here because that is where the 236 to 320
  s goes.
- **Ask 3, stale only when a write reaches the selection,** changes: a write the selection
  selects is taken in; a report goes stale only on a write that changes what it said about a
  key. Reason: a forget of a source still writing would never pass, and "never left behind"
  is what the addendum asks. An open choice below.

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
| R1 | The runner, the tail read, `limits`, a job's state | 350 | 250 | 2 |
| R2 | Remove, forget, purge in parts: walk off the writer, `act_whole`, `source_purged` | 400 | 450 | 2 |
| R3 | Staleness on all acts, preflights on a view, plan, declaration, migrate | 200 | 250 | 1 |
| R4 | Scoped restate for removals; the kept reason | 250 | 300 | 2 |
| R5 | The rebuild beside, with no fall back | 300 | 300 | 2 |
| R6 | Reclaim in turns; PostgreSQL off the lease | 160 | 180 | 1 |
| R7 | `sort_index`, `lookup_index` builds; `Drop`; `choose_order`'s refusal | 350 | 350 | 2 |
| R8 | `view_kept` builds | 450 | 500 | 3 |
| R9 | `view_building` for sets and triggers | 80 | 120 | 1 |
| | **Sum** | **2 540** | **2 700** | **16** |
| | **×1.35** | **3 429** | **3 645** | |

R1 first; R2 stands on it; R4 and R5 may go in either order; R6 follows them. R7 stands on R1,
R8 on R7 and 098's F9 to F11, R9 on R8. Record 097's step 8 stands on R7; its other steps land
beside this list after F12.

### The estimate, and what is cut first

With 098:

| | Product | Specification |
|---|---:|---:|
| 098, ×1.35 | 2 430 | 1 971 |
| 099, ×1.35 | 3 429 | 3 645 |
| Together | 5 859 | 5 616 |
| Removed by 098's F2 | −599 | −2 498 |
| **Net** | **5 260** | **3 118** |

At ×2, which record 097 says this round's pull requests ran: 8 081 product and 5 822
specification net. The net at ×1.35 is what the raise is sized on, and record 097's reserve
does not hold it. A raise lands by `docs/budgets.md` with its row, each step's size measured.

Cut in this order, with what each costs:

1. **098's F12, provenance once:** 162 and 162. Costs 380 B a tick and the 2.3× page, on a type
   with derived fields, until lakelands#1636.
2. **R8 and R9, `view_kept` builds:** 716 and 837. A kept view's setting stays under the writer,
   79.7 s at 1M, and lakelands#1651 is met for indexes alone.
3. **The plan, declaration and migrate half of R3:** about 135 and 170. They stay on the head.
4. **R5, the rebuild beside:** 405 and 405. Costs only shapes the scoped pass cannot do, which
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
- `TWO_ACTS_RESTATE_ONCE`: two owed acts cost one rebuild.
- `RESTATE_SCOPED_FOR_A_REMOVAL`: a lake with a removal restates by pages; the result equals a rebuild.
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

### Open for the operator

1. **The series form: no-go recommended.** In 098, with its numbers.
2. **A write a selection selects, between report and act.** Taken in, or stale. Both leave nothing
   behind. Stale makes a forget of a source that still writes wait for a quiet moment that may
   not come. **Taken in recommended**, with `taken_since` saying what the report did not count.
3. **When an act answers.** When it is whole, or at the first turn, with erasure done and a
   restate owed. The first gives a caller one answer, after minutes at worst; the second holds no
   request open, and a caller then needs a status. **When it is whole recommended:** a caller
   that drops loses nothing, and a repeat answers where the job stands.
4. **When a setting's write answers.** When the build is whole, or at once with `building`
   in `describe`. **When it is whole recommended,** as with acts; a large type's operator may drop
   the call, and the build goes on.

## Why

- **The log is append-only, so a report has a position to stand on.** Hashing the head treated
  every write in the lake as a change to what a report read. Almost none is.
- **One road, because two would differ at every edge in the table above.** An act and a build
  walk, check, take turns and finish; only what a turn does differs.
- **The writer's cost is what stays bounded when the lake grows.** Every step left under it is
  sized by a part, not by the lake, and every step that grows runs on a view, where 076 says
  reads run.
- **A swap is the one turn that cannot be cut,** and it is measured at 1.25 s. Everything else
  is a choice of chunk.
- **A planner pick looked like a design problem.** The scoped forget's 287 s was one index
  chosen wrongly; a measured split found it, and only a split could.
- **Erasure stays first.** Keys go in the first turn, and everything after it restates what
  no key can read any more.

## What it is not

- **Not two writers.** 076 holds: a job's turns are the writer's turns.
- **Not a query planner of its own.** F1 is one unary plus.
- **Not parallel builds.** One at a time, as lakelands#1651 says.
- **Not a promise on a machine like the one measured.** The bound is `limits` and turns; each step
  measures its own.
- **Not the design of lakelands#1639.**
- **Not a change to what a forget takes,** what the log holds or what `verify` promises.
