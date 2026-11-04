# 099 — Work beside the writer runs in parts

Decided on lakelands#1649, lakelands#1651 and lakelands#1652, for 0.23.0; the operator's
choices are on lakelands#1578.

## What is decided

**Decided** on lakelands#1652, for 0.23.0, one design with lakelands#1649 and
lakelands#1651, lines 11, 12 and 13 of epic lakelands#1578. It answers part 3 of the
brief and stands on `docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`. It
amends 039, 041, 049, 057, 058, 065, 066, 076 and 081. No code lands with it.

**A breaking change, inside 098's: no data migration, and no lake to carry.**
**Every change of format, stored table and public type this record makes:**

- **Log format 23 gains three kinds:** `act_whole`, which ends an act,
  `source_purged`, which begins a purge, so a purge can be owed, and
  `values_thinned`, which begins a thinning (102).
- **An act is several appends.** `values_removed`, `source_forgotten`,
  `source_purged` or `values_thinned` first, retirements and closes in appends of at most
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
  A thinning act is not one of these: a write at a key it selected is refused,
  since it takes the value a refold would read (102).
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

**The id the acts write, which purge takes first.** The report id stays an opaque
string. Purge's is `<through>-<digest>`: `through` is the view's position when the
report was read, and the digest is over the `made` positions of the records it selected
there, with the source and type named. At the act the purge reads the selection out of
the log as of `through` — the records as they stood when the report read them — takes
in whatever the source made after it, and refuses `purge_report_stale` when a record of
that selection is no longer the source's alone, or has gone. A write elsewhere enters neither the selection nor
the digest, so it does not stale the report. The chain digest matched by prefix was the
other way offered; the position and a digest of positions was taken as the simpler to
say and to check.

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
- **Declaration.** Every write `feed::about` names as about a type the plan's sections
  name — a record's, that type's `declared` or `migrated`, a value type's migration —
  reaches it; found by type in the log index. A write of a type no section names does
  not, and neither does a declaration of a function or a setting: reach is over types,
  not over the data a function or a check reads, so a write to a type no section names
  is not followed, whatever a function reads there.
- **Migrate.** Any write to the type, since it rewrites the type: `routes_ty` in the
  tail. `report_id` and `value_report_id` leave the head.
- **Proposals'** `id_head` take the same id function.

They hashed the same head before this step, and that is all they share. Plan,
declaration and migrate stay in one step and can be cut.

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
    writer, on the view, and its result is merged into `w.removals`; the plan placed it
    in the walk, beside the first turn, where it costs a write nothing, and the tree runs
    it in the restate's turn, where a write waits for it until pages are turns of their
    own. The fold adds each act after it. An act `taken`
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
Verbs, `requires`, a computed field's inputs, ingest's old state and the resolvers of
`preceding` and `in_force` all come through that one seam. So no erased value goes back into the log, no backing is called with one, and no output
is sealed under a destroyed key.

The cost is one test per record while nothing is owed by its source, O(fields), and on a
record that is reached, its versions at up to 2 ms each on 23 (reckoned, from 098's block
read). It needs only the acts in the lists, so it holds from the writer's open, before the
runner has walked again. A read does not do this: it would pay the test on every row of every
page, so it answers old values, as 065 says. A trigger's firing over an old value is held by
the poll rule above, but for a conjunct of `preceding` or `in_force`: `stands` keeps that as its
write decided it, so those two read through, and `preceding`'s half lands in R2 whatever
becomes of 100. The row is `WRITE_READS_THROUGH_AN_OWED_ACT`.

A thinning act is not on this seam. It takes the record's states out of the
projection without erasing a value from the log, so its refold would answer the
very value the act took out; a write at a key its selection holds is refused
`Thinned`, naming the horizon (102), rather than reading through, since the fold
a stranded group holds must not be contradicted. A write at any key it did not
select is ordinary, and reads as it reads when nothing is owed.

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
- **Taken back:** the setting appends and `DropChunk` deletes in chunks, each a turn, marked
  `dropping`, which no read looks at.
- **`choose_order`'s refusal goes.** `eligible` refuses a type with `sorted_fields` (read),
  because a setting's write and a chosen build could not share the rows. They share one
  road now, field by field: a field the host has set is the host's setting, and a chosen
  one goes when the setting comes.

**As built (2026-11-02).** The `sort_index` and `lookup_index` builds take the road. A
setting's event written at a door is folded with `whole` false, and the projection records
the fields it asks for in its mark — `setting_building`, and `looked_building` and
`looked_going` for the exact-text index. The door that appended it sends one build for
each name the type's fields are kept under, and answers when they are whole; a build a
runner left owed is found by the next writer's open from the same mark and sent again, and
the open does not wait for it. Reads keep the old road: a field in `setting_building` is
withheld from `order_fields` until its `Keep`, a field in `looked_building` is no lookup's
until its `Keep` writes `lookup_fields`, and a field that goes keeps its rows and answers
until that `Keep`. A write beside the build keeps the rows of the fields being built, so a
key at or before the cursor is reached twice at most, and once the build is whole. A
host's setting that names a field takes the lake's chosen field away, as it does when the
log is folded whole.

**As built (2026-11-03), the rest of R7.** A setting taken back appends, and the fields it
no longer names are marked `dropping`, which no read looks at; a `DropChunk { field,
after, limit }` deletes their rows a key range of `order_chunk` records a turn, each a
write of its own, and its last chunk, answering none, removes the mark. The lake dropping
a chosen field takes the same road. A drop in parts holds between any two parts of a
build: the build's next `Fill` finds the mark gone and ends, and so does the door that
wrote the setting taken back. `Choose::Drop(FieldName)` stays on the host surface for a
caller built against it, deprecated since 0.23.0: it clears the whole field in one step,
answers none and takes the mark away with it, as it always did. A setting set again between
two chunks of its take-back cancels the drop: its fold takes the field out of `dropping`,
the drop's next chunk names a field a setting names and does nothing, and the setting's own
build fills and keeps the field whole. `eligible` no longer refuses a type whose
`sort_index` names a field: the setting and the lake share the rows field by field, a field
the host has set is the host's setting, a field the lake chose is the lake's until the
setting comes to name it,
and `choose_order` refuses only a step naming the field the setting names. A field a drop
holds is the drop's until its last chunk: the lake does not choose it, and a `Start` of it
does nothing, so no build fills what the drop is clearing. The take-back is the writer's
live fold; a fold from the log makes a retired setting whole at its event, its index
rebuilt whole rather than marked for a `Drop`. `describe` names the fields a build is
filling in `building`, and `told` keeps naming a page sorted whole for a field a build
that is not the lake's own is filling: a `lookup_index` setting's build does not stop a
page ordered by its field sorting the type, so that page is still weighed while it builds.
The suite holds R7's build rows — `BUILD_READ_ANSWERS_AS_BEFORE`,
`BUILD_NEVER_READ_PART_BUILT`, `BUILD_WRITES_GO_ON`, `BUILD_WRITE_REACHES_BOTH_SIDES`,
`BUILD_RESTART_FINISHES`, `BUILD_SETTING_TAKEN_BACK`, `BUILD_SETTING_SET_AGAIN`,
`BUILD_TYPE_DECLARED_AGAIN`, `BUILD_BESIDE_A_REMOVAL`, `BUILD_REBUILD_SAME_LAKE` and
`BUILD_EXPORT_IMPORT_SAME_LAKE` — and `CHOSEN_ORDER_BESIDE_SORT_INDEX`, a chosen field
beside a setting's own, the chosen one going when the setting comes to name it, and
`SORTED_WHOLE_BESIDE_A_LOOKUP_BUILD`, the whole sort a lookup build does not hide. The
store's road holds a build and a drop left owed between turns and finished by the next
writer's open, `a_setting_build_stopped_between_turns_is_finished_by_the_next_open` and
`a_drop_stopped_between_chunks_is_finished_by_the_next_open`.

Measured on macOS, a debug build (not optimised), 20 000 records, `order_chunk` 256:

| The build's `Fill` turn | Engine | Turns | Held, p50 | Held, max | The door answered in |
|---|---|---:|---:|---:|---:|
| `sort_index` | memory | 80 | 6.2 ms | 6.8 ms | 0.49 s |
| `sort_index` | SQLite | 80 | 4.5 ms | 6.4 ms | 0.36 s |
| `lookup_index` | SQLite | 80 | 254 ms | 501 ms | 20.0 s |
| `lookup_index` | memory | 2 | — | — | under 1 ms |

The memory engine keeps no exact-text index, so its `lookup_index` setting is whole at its
own event. The SQLite exact-text turn is the slow one: 256 records a turn, each a delete
and an insert a field. A caller's write sent beside either build waits for one turn, as
`BUILD_WRITES_GO_ON` pins.

Product: 913 lines in, 69 out; specification: 530 in, 2 out. The cut is `Look::Start`, a
step with no caller: the fill writes the rows of a field being built, and a field that goes
is deleted at its `Keep`. The core-size gate moved to product 102 559 of 108 500 and
specification 187 001 of 191 500 (from 101 717 and 186 473); seam-width is 100 of 110, two
methods added (`ProjectionStore::put_record_for`, `ProjectionStore::look_index`), and
`cfg(feature)` is 6 of 20. This is more than R7's 350 product and 350 specification: the
row's estimate did not hold, and the split it names already falls here.

**The rest of R7 (2026-11-03).** Product: 543 lines in, 133 out; specification: 690 in,
15 out. The cut is the whole-type delete a `Drop` did in one step, and the refusal that
kept a setting and a lake's choice from sharing a type; what replaces them is the chunked
`DropChunk` and the field-by-field share. The core-size gate moved to product 106 116 of
115 000 and specification 191 630 of 202 500 (from 105 706 and 190 955); seam-width is
104 of 110, no method added or removed, and `cfg(feature)` stays 6 of 20.

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
  it may be declared only over a whole view: 066's own checks (`set_of_view`,
  `watches_view`, `view_kept_for_set`), and R9's `view_building` for a view whose build
  is in flight. The set's remake stays in its declaration's append, and
  `max_set_changes` holds. Read: a set needs its view kept already
  (`view_kept_for_set`), so a set is never over a view from before its build.

**As built (2026-11-03), R8 part 1.** A `view_kept.<view>` setting set true appends, and
the fold marks the view building in the `sys:kept` output — `Building { cursors }`, one
cursor per source, none reached — rather than making it whole in that turn. The door that
wrote it sends a build of every view the view reads through, the inner ones first, and
answers when the last is whole; a writer's open sends what the projection holds owed and
does not wait. A `Fill` turn folds one `order_chunk` page of the first source whose cursor
has not reached its last key, adds it to the staged groups, and puts the advanced cursor in
the view's state in the same commit; a restart therefore goes on from the cursor, not from
0. Where every source is reached, the turn makes the ungrouped group, sets `whole` and the
zone data, and takes the mark away: that is `Keep`. A record written beside the build is
noted on the side of the cursor it lies on — at or before it, by the write; after it, by
the fill when it gets there — so it is counted once, whichever side it lands on. A read of
the view computes it live until `whole`, and `describe` does not say it kept while it
builds. The suite holds `KEPT_BUILD_READ_ANSWERS_AS_BEFORE`, `KEPT_BUILD_WRITE_BESIDE`,
`KEPT_BUILD_WRITES_GO_ON`, `KEPT_BUILD_RESTART_FINISHES`, `KEPT_BUILD_SETTING_TAKEN_BACK`,
`KEPT_BUILD_SETTING_SET_AGAIN` and `KEPT_BUILD_REBUILD_SAME_LAKE`.

Part 1 leaves three boundaries, and part 2 or 3 takes them: only a live setting write
starts a build (a declaration that changes the type, a lease and the recompute paths still
make a view whole in one fold, and a set that computes a build's source again takes the
mark away at its first declaration and the view is made whole at its last); a set or
trigger declared while its view builds is refused by the declaration's own whole check
(`set_of_view`, `watches_view`), and the refusal that names a build in flight is R9's;
and the mark is not yet told in `describe`'s `building`, nor is a take-back cut into
chunks.
No row holds a kept view over a view either: with the outer's setting set before the
inner's, the outer is made whole in the fold with no groups, and it answers right only
because the inner's `Keep` notes fill it and a read computes while the inner is not whole;
`VIEW_BUILD_OVER_A_VIEW` is part 3's. Parts 2 and 3 take these, below.

The tree departs from the design in one place: the cursors live in the view's state, which
is the projection's, not in the runner's memory as this record's table first said. The
group records are in the projection already, so a cursor held only in memory would restart
the fold from 0 on the next open and count every member twice. The table's restart row is
corrected below.

**The ledger (2026-11-03).** Product: 454 lines in, 29 out; specification: 666 in, 7 out.
The cut is the fold that made a kept view whole at its setting's append; the mark, and
the `Fill` and `Keep` turns, take its place. The core-size gate moved to product 107 856 of
115 000 and specification 193 188 of 202 500 (from 107 431 and 192 529 on develop);
seam-width is 107 of 110, no method added by this part, and `cfg(feature)` stays 6 of 20.

**As built (2026-11-04), R8 parts 2 and 3.** A setting taken back marks its view
`dropping` in the same output and leaves its records for the road: a `DropChunk` turn
ends one `order_chunk` page of the view's kept records, and the last, answering none,
ends the order and the mark, while a read computes beside it. A setting set again
between two chunks keeps the mark and starts a build that begins where the drop ends,
never a make, so no group is made over a member the drop has not ended. The door that
retires a setting runs the drops before
the builds, and an open sends the drops it finds before the builds. `describe` says a
view a build has not kept `building`.

A missing view whose inner view is not whole is started, not made, and the door that
builds a view also builds the views that read it, after it, however many views away: a
round of its build takes only the views whose every source that is a view is already
built, so a view is never sent before one it reads, whatever the declarations' order and
whatever the distance. `VIEW_BUILD_OVER_A_VIEW` holds, since the outer waits for the
inner's `Keep` and the door that builds the inner finishes the outer. A build's extremes are read from `view_order` up to the cursor, so a
record that takes a group's smallest away leaves the next smallest, whichever side of
the cursor it was written on. A zoned build records the copy of the time-zone database
that cuts it, and a writer that opens under another copy ends the build and folds the
view again under the copy in force. A set or a trigger is declared only over a view
that is whole, so a declaration made while its view builds, or while its view is taken
back, is refused by that check (`set_of_view`, `watches_view`), naming the view. A set or
trigger in force is not ended by another view's build or take-back; the one road that
unmakes its view without a refusal is a fold under another copy of the time-zone
database, which starts the view again. The refusal that names the build itself is R9's
`view_building`, which this part does not carry.

The suite holds `VIEW_BUILD_EXTREME_LOSES_ITS_HOLDER`, `VIEW_BUILD_OVER_A_VIEW`,
`KEPT_BUILD_TYPE_DECLARED_AGAIN` and `KEPT_BUILD_EXPORT_IMPORT_SAME_LAKE`. The store's
tests pin the cursor a part leaves
across a restart (`a_kept_build_goes_on_from_the_cursor_a_restart_finds`), a take-back
in chunks (`a_kept_view_is_taken_back_in_chunks`), the outer view starting where its
inner builds (`a_view_set_before_its_inner_view_starts_where_the_inner_builds`), a view
sent after the view it merges (`a_merge_view_is_built_after_the_view_it_merges_by_the_door`)
and one two views away (`a_view_two_views_away_is_built_by_the_same_door`), and a build
cut under another copy (`a_zoned_view_building_under_another_copy_is_folded_again`).

Measured on Linux, a debug build (not optimised), 20 000 kept records, one group a
record, `order_chunk` 256:

| The take-back's `DropChunk` turn | Engine | Turns | Held, p50 | Held, max | The door answered in |
|---|---|---:|---:|---:|---:|
| `view_kept` | memory | 80 | 6.3 ms | 9.5 ms | 0.51 s |
| `view_kept` | SQLite | 80 | 20.2 ms | 213 ms | 1.72 s |

Eighty turns: 79 pages of 256 or fewer, then the ending turn that answers none. The
door that takes the view back waits for them all; the same view's build answered in
1.9 s on memory and 3.4 s on SQLite.

The ledger (parts 2 and 3). Product: 345 lines in, 53 out; specification: 917 in, 45 out.
The lines out are rewrites, not cuts: `want`'s missing branch, `start`, the door's
`finish` and the kept group's helpers now carry the drops, the `building` answer and
the order the outer view waits in, each in place of the lines it
replaces. The core-size gate moved to
product 110 741 of 115 000 and specification 198 092 of 202 500 (from 110 449 and 197 220
on develop); seam-width stays 108 of 110, and `cfg(feature)` 6 of 20.

### What stands between parts

| Between parts of | Act | Build |
|---|---|---|
| A restart | owed in the log; a write reads through it from the open; the runner re-walks off the writer, retires what remains, appends `act_whole` | the mark holds the cursors, one per source, in the view's projection state; the open re-sends the build and it goes on from them |
| Another act of the source | its walk starts past this act's first turn; its parts wait for this act's restate, then re-judge on what that left | a build's rows follow its retirements, as below |
| A setting taken back | no bearing | `DropChunk` in parts; a late `Fill` sees the mark gone and ends, and a setting set again takes the field out of the mark |
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
| A part | 94 to 128 on memory, 412 to 696 on SQLite, measured at 500 events in a debug run | `part_events` | its events |
| First turn: tail and keys judged | 11 for 1 000 events, measured, then the key store's 9 to 70, measured | `part_tail` | the tail, the source's versions, the key store |
| A page | a removal's 89 to 155, a forget's 25 to 59 on smaller lakes, measured | 256 records, `page_events` | its events, `wide`, `walks`; a forget's also with the lake |
| A chunk of the sweep | as a build's, reckoned | `order_chunk` | its chunk, the tail |
| A build's chunk | 6.2 to 6.8 on memory and 4.5 to 6.4 on SQLite for a sort index, 254 to 501 on SQLite for a lookup index, measured at 20 000 records and 256 a turn in a debug run | `order_chunk` | its chunk |
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
| R8 | `view_kept` builds | 450 | 500 | 2 |
| R9 | `view_building` for sets and triggers | 80 | 120 | 1 |
| | **Sum** | **2 930** | **3 260** | **17** |
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
- `RESTATE_PAGE_OVER_ITS_BOUND` (named `RESTATE_PAGE_OVER_ITS_BOUND_GOES_BESIDE` in this
  plan; this pull takes the rebuild half, R5 the "goes beside" half): a page over
  `page_events` on a migrated lake.
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
- `VIEW_BUILD_EXTREME_LOSES_ITS_HOLDER`.
- `VIEW_BUILD_OVER_A_VIEW`: starts after its inner view's `Keep`.
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

## As built

**R1 (lakelands#1675).** The road is `crates/lakelands-store/src/road.rs`; no act and no build
is on it yet.

- **The seam is a job of five steps:** `reach` (the sources whose writes reach it, and the types
  and keys whose records do), `walk` on a view, `fold` of the tail's events that reach it, `turn`
  under the writer, and `end`, where a caller who waits is answered. A turn says whether another
  follows. What an act or a build keeps is its own: the runner holds nothing of a job but the
  job.
- **The runner** is spawned when a job is sent and none runs, as `choose_orders` is, and drives
  each job whole before the next, oldest first. Between turns it holds its lake weakly, so a lake
  whose last handle goes stops it before its next turn. That a later act's first turn goes ahead
  of an earlier act's parts ("Acts that overlap") is the acts' rule, and lands with R2.
- **The tail read** finds at most `part_tail` events past the view in the log index, by source
  (`of_source`) and by type and key (`matching`), each event once however many rows name it, and
  reads them with `read_these`, through the block seam of format 23. Where the index decides
  nothing, as while a rebuild refolds, it reads the log itself and every event counts against the
  bound. The catch-up reads the tail off the writer while it is longer than that, and folds it in
  pieces of `part_tail` events; it holds the tail's positions, never its events, and stops after
  three rounds that each find the lake moved on, when the first turn finds the tail too long. The
  first turn reads it under the writer and folds it before it turns. Three tries that each find it
  longer end the job unavailable, `TailOverran`, with nothing appended.
- **`limits`** hold `part_events`, `part_tail` and `page_events` with the defaults above, and
  `order_chunk` is 1 000. `page_events` bounds a page of a scoped restate, and a page over it
  takes the rebuild (R4). `part_events` is read from R2, where purge appends in parts.
- **The writer's open** asks every kind of job in `OWED` what the log and the lists owe, off the
  writer and after the open has answered, and sends it; a writer taken over from standby asks
  the same. `OWED` is empty; the open and the standby take the kinds they ask as an argument
  inside the store (`opened_owing`, `standing_by`), so the road's tests open through both with the
  test's kind beside the store's. A failure to find what is owed is not reported: the store keeps
  no log of its own, and the job stays owed for the next open. The open still finishes an owed forget or removal before it answers
  (`finish_forgetting`): moving that beside the writer opens the window that reading through
  closes, so it moves with R2.
- **Measured** on the shared machine of this step, not a quiet one: beside a job of 50 ms turns,
  20 writes each waited for at most the one turn in front of them on memory and on SQLite, and
  PostgreSQL's run is CI's. The machine stretched each 50 ms sleep to as much as 200 ms, so the test
  counts the turns a write waited for, not its milliseconds. The turn table is measured by the
  step whose turns they are, R2 to R7.
- **Conformance rows:** none is taken. Each names an act or a build, and neither is on the road
  before R2 and R7. The runner's half of `ACT_TAIL_OVERRUN_MOVES_THE_VIEW`,
  `ACT_RESUMES_AT_OPEN` and `BUILD_WRITES_GO_ON` is in the store's unit tests under a test job,
  on every engine, the `pg` leg included; the rows go into the suite with the steps that put an
  act and a build on the road.

**R2, first pull (lakelands#1686).** Purge is the first act on the road; remove and forget
keep their one-append path, with `act_whole` set on it, until the second pull, which puts
remove on the frame (lakelands#1691); forget follows in lakelands#1700.

- **The act's frame.** `source_purged` opens a purge in the log and `act_whole` ends every act:
  kind codes 25 and 26, appended last so no code moves, and the log format stays 23. An
  append of nothing at all is still nothing: a purge with nothing to take and no close to
  write appends no frame.
- **The selection the frame keeps.** `sys:purged` is a list keyed by the act's position,
  saying the source, the type if one was named, and whether the act is whole; each
  `source_purged` adds an entry and its `act_whole` marks it. The fold of both kinds writes
  the list, and the road's owed hook finds an unwhole entry and sends the act again with
  the act's position as its window. The list holds every act the log has, closed or not, and
  both frame events write it whole: no act is ever dropped from it.
- **Parts.** The walk judges the selection off the writer, and the report is the walk's.
  A removal's first turn reads its report again from the records it judged again and the
  tail folded in, so `retired`, `types`, `kept`, `breaches`, `references`, `recomputed`
  and `views` name what the act takes; `taken_since` is not built, the act's report being
  its own and not the walk's patched. The first turn folds the tail past the walk's view
  into that selection — it reads again
  only the keys the tail wrote — names the records outside it that refer in, and appends
  `source_purged` beside the quarantine closes, recording the act's position. Each part
  then takes up to `part_events` of the selection's records in type and key order, rereads
  only those, and appends their retirements; one that went since the walk is dropped, and
  one whose live version another hand made again is dropped too. The append that ends the
  act carries `act_whole` where it fits. A record's retirements are never split across
  parts, so one append may hold more than `part_events` for one record, never for more than
  one. The door answers when the act is whole, through a oneshot, and settles the backed
  calls the act's own appends caused.
- **The report id.** `<through>-<digest>`, as restated under "A report is read at its
  position": opaque, no public type changed, and stale when one of its positions has gone or
  a record the report named is no longer the source's alone, or when the position it names
  is past the log's head. A record the source made after that position that another hand now
  holds is not the report's, so it does not stale it: the act answers `PurgeShared`, as a
  purge without an id does.
- **A turn is a write.** The road's turn takes the writer through `for_write`, which
  catches a writer that was behind up before the act decides anything, and drains the list's
  causes after each turn so the door can settle them.
- **Resume at open.** The writer's open asks `OWED` after it answers, and purge's hook sends
  an unwhole act; the same act runs again with `made < at`, where `at` is its
  `source_purged`, and closes what the source still has open, since its first append carried
  only the closes that fit. A resumed act has no caller waiting: the backed calls its
  appends leave owed settle at the next write's orphan pass, not at the open. The look for
  what the open owes holds the lake weakly, as the runner does between turns, so a look
  stops with a lake let go.
- **A lake let go.** The store facade, and the doors and batches taken from it, are the
  caller's handles onto the lake: the lease goes when the last of them is dropped, as the
  last handle's drop always gave it, though a job beside the writer still holds the lake
  while it runs. The job stops at its next step and what it left stays owed in the log, so a
  lake let go on its answer opens again at once. A lake held without a facade, as the road's
  tests hold one, keeps the old rule: its lease goes when its last handle does.
- **Rows taken on every engine:** `PREFLIGHT` and `REPORT_HEAD` restated for the token,
  `RETIRES_WHAT_IT_BROUGHT` and the remove check's kind list for the frame, and
  `ACT_WHOLE_ENDS_THE_ACT`, `PURGE_TAKES_A_WRITE_SINCE_ITS_REPORT`, `PURGE_MADE_SINCE_SHARED`,
  `PURGE_STALE_ON_A_SHARED_KEY`, `ACT_RESUMES_AT_OPEN`, `ACT_APPENDS_IN_PARTS`,
  `ACT_REPEAT_FINISHES_AND_APPENDS_NOTHING`, `ACT_REBUILD_SAME_LAKE`. The rows that need a
  reader held at an owed act — `ACT_WALKS_OFF_THE_WRITER`, `ACT_BETWEEN_PARTS_EXPORTS`,
  `ACT_BETWEEN_PARTS_VERIFIES`, `POLL_STOPS_BEFORE_AN_OWED_ACT`,
  `ACT_TAIL_OVERRUN_MOVES_THE_VIEW` — waited for the second pull, which brought reading
  through an owed act; it took them. The forget rows wait for lakelands#1700.
- **Measured** on the shared machine of this step, in a debug run, not a quiet one: a part of
  500 retirements — the act's turns after its first, a mean over five, on 2 000 records —
  took 94 to 128 ms on memory and 412 to 696 ms on SQLite (two runs each); PostgreSQL's run
  is CI's. Twenty writes sent beside an act of parts of one retirement each waited for at
  most the one part in front of them, every write finding at most one retirement in the log
  before it: p50 1.6 ms, longest 2.0 ms on memory, p50 5.7 to 10.9 ms, longest 9.4 to 65 ms
  on SQLite. Both figures are the store's
  `a_write_waits_for_at_most_a_part_of_a_purge`, which runs on every engine; the `pg` leg
  is CI's.

**R2, second pull, remove on the road (lakelands#1691).** `remove_values` runs as an act
on the first pull's frame. Forget keeps its one-append path until lakelands#1700.

- **The act.** The walk gathers the selection off the writer and judges the records it
  reached. The first turn folds the tail past the walk's view into that gathering — a
  selected event goes inside, any other event of the source outside — cuts the versions
  again, reads again the keys the tail moved, refuses `RemoveNotShredded` before
  appending, and appends `values_removed`, the closes that fit, the first retirements and
  `act_whole` where it fits. It then destroys the versions that remain and gives the
  source its new version, so erasure happens in the first turn. A `written_before`
  selection moves its cut forward over the tail it reads, since `known_at` never falls.
  Later turns append the closes and retirements still owed, each read again as the part
  takes it, and `act_whole` ends them.
- **The restate and the reclaim follow the whole.** The act's last turn finishes what it
  appended through `finish_forgetting`. `reclaim` marks an act done only where its
  `act_whole` is in the log, and `finish_forgetting` restates no act between its parts:
  a rebuild that meets one leaves it owed, and its last turn marks it done.
- **The open no longer finishes an act.** `Turn::leased` appends the lake's own events
  and nothing else. `OWED` gains remove's unwhole acts and a settle job for the restate
  and reclaim a whole act owes or a crash left, and both run beside the writer after the
  open answers. `rebuilt_at_open` therefore no longer says the open folded the log to
  finish an act, and a standby's taking over leaves that work to the road.
- **A write reads through an owed act.** `restate::Viewed` folds again, for one key and
  in memory, the events of a record a source with an owed act wrote, through the
  erasure the act names; a write's `requires`, its functions and its backed fields judge
  on the restated record. A writer's open tells itself the acts the lists hold, which
  its fold from the projection's mark does not meet, and dooms the versions those acts
  took, so both hold from the open, before the road has walked again.
- **Rows taken on every engine.** The five that waited on reading through an owed act —
  `ACT_WALKS_OFF_THE_WRITER`, `ACT_BETWEEN_PARTS_VERIFIES`, `ACT_BETWEEN_PARTS_EXPORTS`,
  `POLL_STOPS_BEFORE_AN_OWED_ACT`, `ACT_TAIL_OVERRUN_MOVES_THE_VIEW` — and remove's:
  `ACT_ERASES_IN_ITS_FIRST_TURN`, `REMOVE_TAKES_A_DELIVERY_THAT_GREW`,
  `REMOVE_LEAVES_A_LATER_DELIVERY`, `REMOVE_WRITTEN_BEFORE_JUDGED_AT_ITS_FIRST_TURN`,
  `REMOVE_TAIL_WRITE_KEEPS_ITS_VERSION`,
  `REMOVE_TAKEN_IN_UNDER_A_KEPT_VERSION_IS_NOT_SHREDDED`,
  `REMOVE_STALE_ON_ANOTHER_SOURCES_WRITE`, `ACTS_OF_ONE_SOURCE_OVERLAP`,
  `WRITE_READS_THROUGH_AN_OWED_ACT`, `REOPENED_WRITE_READS_THROUGH_AN_OWED_ACT`,
  `REPORT_NOT_STALE_BY_A_WRITE_ELSEWHERE`.
  `REMOVE_CRASH_SHREDDED` is restated for the act's first turn: a version the act took
  seals nothing from then, so the source seals under a new one while the act is owed, and
  only a destroy that failed inside that turn is `key_missing` (065). The forgetting's
  rows wait for lakelands#1700.
- **Measured** on the shared machine of this step, in a debug run, not a quiet one:
  twenty writes sent beside a remove of parts of one retirement each waited for exactly
  the one part in front of them — p50 6.3 ms and longest 14.5 ms on memory, p50 14.6 ms
  and longest 17.6 ms on SQLite. A part of 500 retirements, on 2 000 records: 2.76 s in
  five turns after `values_removed`, 553 ms a turn, on memory; 6.93 s, 1 386 ms a turn,
  on SQLite. Both figures are the store's `a_write_waits_for_at_most_a_part_of_a_remove`,
  which runs on every engine; the `pg` leg is CI's. The test holds the turn between
  parts with an entry of its own that reads the log's tip, queues the act's next part
  and then the write behind it, and counts the retirements between the gate's tip and
  the write's own position, so neither the scheduler nor a projection mark behind the
  log can widen the count; fifty runs with every core busy did not.

**R2, third pull, forget on the road (lakelands#1700).** `forget_source` and
`forget_sources` run as acts on the frame, and no act is left on its one-append path.

- **The act.** The walk gathers off the writer what the set's sources brought: the records,
  which of them each source holds a value in, which it alone brought, and the events at the
  report's `through`. The first turn opens a view under the writer, destroys the keys of any
  source whose `source_forgotten` the log already holds (from a crash inside the act, or an
  open between its parts), reads again the keys the tail moved, refuses `ForgetPlaintext` for
  a source not yet in the log, and appends a `source_forgotten` for as many sources as fit,
  destroying each source's key in the same turn, and then the first part where the room
  allows it after them. Its report is read
  again from the records it re-judged, so the door answers what the act did, not what the walk
  saw. A forgotten source's writes are refused from its `source_forgotten` on, so nothing is
  sealed under a destroyed key between the parts. The act judges under the ontology it finds
  there: a declaration or a migration since the walk changes the ontology the set is gathered
  under — what is erasable, what a field's first `initial` makes co-owned, what recomputes —
  or the names the log bears, so the set is gathered again under the writer, and the walk's
  gathering would be the walk's judgement and not the act's (058).
- **Parts.** A later turn reads the tail past the act's last append first, as the first turn
  read the walk's, so a write by a source whose `source_forgotten` is still owed is taken in,
  retired with the rest and counted in the act's report, its own `source_forgotten` landing
  after it. A retirement a part has already made stays counted when another hand makes its
  key again between two turns: the log holds the retirement, and the record made since is
  not the act's to retire. A record a write between turns leaves quarantined for a source
  still owed is closed `forgotten` by the act, with the closes it read, and counted with
  them: 085 makes the close the act's. It then appends the `source_forgotten`s still owed,
  destroying their keys as they land, and, once none is left, one part of the closes and
  retirements, at most `part_events`, each re-judged as the part takes it: a record another
  hand wrote since is co-owned and not retired. The append that ends the act carries `act_whole`. The report's
  quarantined counts are the closes the act has read and taken in, held as the parts append
  them, since the view holds none of a record once a part has closed it.
  The first turn appends two of those: the `source_forgotten`s as many as fit, and then the
  first part where the room allows it after them, so it may hold the writer for two appends
  of `part_events`.
- **The restate and the reclaim follow the whole.** `finish_forgetting` destroys the keys of
  an owed source and restates only an act whose `act_whole` is in the log; `reclaim` marks a
  forgotten source `done` only where its act is whole. `forget::owed` joins `OWED`, so the
  writer's open sends an unwhole forgetting again, and the settle job restates and reclaims
  once it is whole.
- **Its reactions are judged to the act's end.** The feed's `acted` is the act's last frame
  event: the fold records an `act_whole` where it records a `values_removed` or a
  `source_forgotten`, and `last_act` reads it. A reaction a retirement in an earlier part set
  off — its cause inside the act, before its `act_whole` — is judged overruled once the whole
  is in, not by the act's first append alone. Remove and purge move with it in the fold: their
  `act_whole` sets `acted` too, so a reaction their parts set off follows. `last_act`, at an
  open, reads the ends of the forgetting and removal acts the open knows.
- **A write reads through an owed forgetting** as it does a removal: `restate::Viewed` refolds
  the records a source with an owed act wrote through the erasure the act will leave, so a
  verb's `requires`, its assignments and a function field's inputs judge on the record the
  restate will leave.
- **The report id** is a removal's: `<through>-<digest>`, the digest over the positions the
  forgetting selects at `through` that no earlier removal act selects. A write elsewhere,
  and a write of the source after `through`, reach neither and leave it good — the act takes
  the source's write in. An earlier act of the source takes what the report selected there,
  so the id over what is left moves, and the act under it is refused `ForgetReportStale`; a
  removal act of the source written between the walk and the first turn is judged there,
  under the writer, and the act refused where the id moved.
- **Rows taken on every engine.** `FORGET_LEAVES_NO_WRITE_BEHIND` is new, and over a set
  larger than one turn's room takes a write by a source the act has not forgotten yet,
  between two of its turns, into the retirements and the report's count;
  `ACT_ERASES_IN_ITS_FIRST_TURN`, `REPORT_NOT_STALE_BY_A_WRITE_ELSEWHERE`,
  `ACTS_OF_ONE_SOURCE_OVERLAP` and `WRITE_READS_THROUGH_AN_OWED_ACT` gain the forgetting's
  half, so each runs for both kinds of act. `ACT_WHOLE_FOLLOWS_ITS_PARTS_FIRED` is new: at
  `part_events` 1 a retirement in the first part fires `in_rome`, and a poll of the feed from
  0 names that firing overruled after the `act_whole` lands — in the act's own handle, and in
  one opened again after it, which reads the end from the log. Beside them, four store tests in
  `road/tests.rs`: a record a part retired stays counted when another hand makes its key
  again between two turns; a quarantined record a part closed stays counted in the act's
  report after a later turn judges a tail; a record a still-owed source leaves quarantined
  between two turns is closed by the act and counted; and a tail past `part_tail` between
  two turns ends the turn unavailable, appending nothing.
- **Measured** on the shared machine of this step, in a debug run, not a quiet one:
  twenty writes beside parts of one retirement each waited p50 7.8 ms and at most 12.7 ms on
  the memory engine, p50 16.7 ms and at most 24.9 ms on SQLite — never more than the one part
  in front of them; an act of 2 000 retirements in parts of 500 took 613 ms from its
  `source_forgotten` to its `act_whole` on memory (153 ms a part) and 4.0 s on SQLite
  (1 000 ms a part). The figures are the store's `a_write_waits_for_at_most_a_part_of_a_forget`,
  which times the act from the log's own wall time, and runs on every engine; the `pg` leg
  is CI's.

**R3 (lakelands#1718).** A plan, a declaration's plan, and a migration's or a value
type's report name the position they were read at, and a write after them stales them
only when it reaches what they name.

- **The id.** A migration's or a value type's report, a plan of declarations, and a
  plan's section ids are `<through>-<digest>`: `through` is the report head 039 names at
  the position the report was read, and the digest is made over the report as its reader
  is shown it there, with its own id empty — and, in a plan, each section's id empty and
  `at` at `GENESIS`, since a section's id is itself a position and a digest. The one
  function builds all of these (`writes::report_named`). A change to records' plan is
  the exception (081): its id is the digest alone, made over `{"plan", "selection"}`
  with its own id empty and `at` at `GENESIS`, the selection being each selected key
  with the version it read. It names no position, so it is compared whole, and one that
  moves a selected key or the selection moves its digest.
- **The check.** `writes::says` reads the position out of the id the caller repeats and
  compares digests; `writes::reached_since` reads at most one event past that position
  through `road::read_tail` with a `Reach` of types and no sources or keys, and answers
  reached when one is found, when the position is past the head, or when the bound is
  overrun. `stale_plan` names the types every section and every value type's uses name;
  `stale_report` names the type a migration rewrites, or the types that use the value
  type it changes. Each refusal appends nothing.
- **What reads it.** `declaring`, `migrating` and `migrating_value_type` stop comparing
  the head; a proposal's `id_head` is the report head the value type's id is made from
  again, and a listed proposal recomputes the same digest.
- **Restated where they said otherwise:** 006's report id and a declaration's plan id in
  062 now name the position, not the head, and 062 says a change to records' plan carries
  no position (081); `docs/refusals.md` for `plan_stale` and `migration_stale`.
- **Rows taken on every engine:** `REPORT_STALE_WHEN_REACHED`, and
  `REPORT_NOT_STALE_BY_A_WRITE_ELSEWHERE` extended to a change to records, a
  declaration, a migration and a value type. `HASH_OF_THE_SHOWN_SHAPE`, `PLAN_TELLS`,
  `PLAN_REOPENED` and `STALE_REPORT` are restated for the id and the word "moved".

**R4 (lakelands#1728), with lakelands#1725.** A removal's restate and a forgetting's
refold are one scoped pass: the records an act of removal selects join those of the
forgotten sources in one walk of the feed index, and every act the lists hold is told to
the fold before its first page.

- **`source_forgotten` names its act (lakelands#1725).** The event carries `act`, the
  position of its act's first event, and `forget::grouped` groups by it in place of
  adjacency: an act a crash left owed between its turns reads as itself when another act
  begins and ends before it is taken up again. The conformance row `FORGET_GROUPS_BY_ACT`
  holds it: two acts each stopped before their `act_whole`, the writer's open finishing
  both, and each group holding only its own sources. Format 23 is unreleased, so no older
  log is read.
- **Keys from the walk.** `scrub::plan` takes the acts to restate beside the forgotten
  sources, and the one walk adds each act's selected records to the plan's keys, and the
  outputs a destroyed key sealed to those recorded again. An act of removal in the lake
  no longer forces the rebuild by itself; `removed_list` names no reason.
- **Acts told once.** `scrub_held` reads `remove::told` once, before its first page, and
  merges each act into the writer's as `rebuild_held` does, so a page that folds a record
  an older act took leaves its value erased, under a kept version and under one the act
  destroyed. A read the telling fails stops the pass before its first page, so the act is
  still owed and the next open retries: a page never meets a `values_removed`, which a
  rebuild's fold does, and an untold act has no `fixed` to erase by. The acts restated are
  marked `rebuilt` with the forgotten list, and the reclaim finishes them.
- **The way asked.** `forget::beside` reads the removal acts the lists hold owed and whole
  at the view's head, and a forget's preflight and its held report plan with them beside
  the set, as `finish_forgetting` does: a lake owing a removal asks the way the finish
  takes, its keys and its pages counted.
- **`page_events`.** `Plan::page_events` is what the fullest page folds: the events about
  its records, `wide`, `walks`, and the outputs it records again, which only the first
  page folds. The outputs of the restate as a whole are not one page's, so the bound is
  not crossed by a long restate whose pages are each short, and a plan with no page names
  none. A page over `limits.page_events` is a reason for the rebuild, and the settings
  row says so. `RESTATE_PAGE_OVER_ITS_BOUND` holds the reason, and the unit test
  `the_bound_counts_a_page_not_the_whole_restate` the count: two pages each at the bound,
  whose recorded outputs together are over it. The row this record names,
  `RESTATE_PAGE_OVER_ITS_BOUND_GOES_BESIDE`, is R5's: the road beside the writer is what
  "goes beside" states, and R5 renames the row with it.
- **Left as the head holds it.** An act's source keeps its used-versions output
  (`sys:sealed`) as the head has it, where a rebuild drops a version the act destroyed: a
  page folds part of a source's records, so re-deriving the list from that page would lose
  the versions of the records it does not hold. No read was found that resolves a surviving
  value to the entry, and a page that holds the whole source, where the list is rebuilt as
  a rebuild leaves it, is R5's.
- **Pages as turns.** This pull folds every page of a restate in the one turn the road
  hands it, so `page_events` guards a page's size, not a write's wait. The plan's page row
  — a page reads its positions in its own turn — and the turn table's page row are not
  built yet; the next pull on lakelands#1728 takes them, before R5, and the issue stays
  open for them.
- **The kept reason stays.** A page would apply `note(old, new)` for each head it changes,
  and a settings event replayed under a page starts a view's build: opening kept outputs
  and view state to the scope is what 098's heads and R5 need, so a kept view over the
  restated records is still a reason for the rebuild. `SCRUB_FALLBACK` holds it, and the
  `note` job goes beside with R5.
- **Rows taken on every engine:** `FORGET_GROUPS_BY_ACT`, `RESTATE_SCOPED_FOR_A_REMOVAL`,
  `RESTATE_TELLS_EVERY_ACT`, `RESTATE_PAGE_OVER_ITS_BOUND`.
- **The ledger.** Product: 205 lines in, 64 out; specification: 453 in, 14 out. The
  cut is the adjacency grouping in `forget::grouped` and the removal act's fallback in
  `scrub::plan`, each replaced by what the log now names; core-size reads product 110 330
  of 115 000 and specification 196 672 of 202 500, the seam 107 of 110 methods.

## How it came to be

- Amended by 100 (lakelands#1640), proposed for 0.23.0: the resolvers of `preceding` and
  `in_force` read through an owed act, which the poll rule does not hold.
- R1 built by lakelands#1675; what the tree took is under "As built".
- R2's first pull built by lakelands#1686 with purge as the first act on the road, and the
  report id as the position the report was read at beside a digest of the selection it
  read there.
- R2's second pull built by lakelands#1691 with remove on the road and a write reading
  through an owed act; forget follows in lakelands#1700.
- R2's third pull built by lakelands#1700 with forget on the road: its keys destroyed in the
  first turn, its parts after, and its restate and reclaim once it is whole.
- R3 built by lakelands#1718: a plan, a declaration's plan and a migration's or a value
  type's report name the position they were read at, and only a write that reaches what
  they name stales them.
- R4's first pull built by lakelands#1728, with lakelands#1725: a removal's restate is the
  scoped pass, acts told once before the first page, `page_events` read. Three parts are
  left: the kept reason and the row's "goes beside" half wait for R5 behind their own
  pull, and pages as turns waits for the next pull on lakelands#1728, before R5.
- Amended by 102 (lakelands#1639), proposed for 0.23.0: a thinning act begins
  with `values_thinned`, and a write at a key it selected is refused rather
  than reading through the act.
