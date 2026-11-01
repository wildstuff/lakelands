# 100 — A guard may read the record in force

Decided on lakelands#1640, for 0.23.0.

## What is decided

**Proposed** on lakelands#1640, for 0.23.0. It stands on the tree records 098 and 099
leave: format 23, the road of parts, candles and selected series as records. It amends
004, 052, 058, 065 and 099. No code lands with it.

**Every change of format, stored table, WIT and public type this record makes:**

- **Log format:** the number stays 23 and no event changes, but what a declaration may
  hold widens. Appendix B of `docs/log-format.md` says `in_force` stands in a field
  computed at the read only, "so no event holds its value". It now stands in a trigger's
  `when` too, so a declaration in the log holds the call, though never its value. The
  step line 22 → 23 lists it, and format 23's kept lake holds such a trigger. A firing is
  the `trigger_fired` it was, and the log does not say what was paired.
- **Stored tables, `LAYOUT`, WIT, MCP tools, `Limits`, settings:** none.
- **`Func::InForce`** is admitted in `Place::Guard`, so in a trigger's `when`. The words
  `in_force` take are the words 052 gives them.
- **`Case::InForceForm`** loses its member `field`, and **`Case::InForcePair`** keeps
  `left` and `right` and loses `field`. Both are said at a place, as every refusal of
  `preceding` is: the type and field, or the trigger. Their text opens "it pairs", not
  "the field `{field}` pairs". The kinds keep their names. `docs/refusals.md` and
  `samples.json` follow.
- **`Case::InForceHere`'s text** says a trigger's `when`, too. The kind stays, with no
  member.
- **`Scope` in `lakelands-core`** gains `paired`, beside `preceding`, and `WriteScope`
  a member for it. A guard holding both keeps them apart: a `preceding` of six
  arguments and an `in_force` of one match spell the same series today.
- **`looked_up` and `looked_up_by`** read a trigger's `when` for the type it pairs,
  **`probed_types` and `declaration_probes`** read `T` and its fields for it, and
  **`named_reactions`** judges `T`'s fields beside `on`'s. Their signatures stay.
- **`type_reader`** reads pairings, so withdrawing a type a trigger or a field pairs is
  refused `withdrawn_in_use`, a kind that exists. A field's pairing is withdrawn from
  under it today, so this refuses a withdrawal that now passes.

Claims are marked **measured**, **read** (in the code at `16e687eda`, and in 098 and
099) or **reckoned**, as in 098. Measured means a scratch build of today's layout on
SQLite: release, a lake on disk, a shared machine at load 5 to 13, `tick_bench` of
lakelands#1657 with a copy of `preceding`'s resolver for `in_force` and a timer around
it. Two rounds, each cell once, 500 records a delivery and 16 callers, with no
roll-up. PostgreSQL and the memory engine were not measured; G2's cell measures them.

### A trigger's `when` may call `in_force`

`in_force(T.f, T.by <= by, T.m = m, …)` in a `when` has the form, the pairs and the
meaning 052 gives it, over the record as the write leaves it:

`price > in_force(box.hi, box.from <= at, box.symbol = symbol)`

It may stand anywhere in a `when`, more than once, beside `preceding`, `changed` and
`created()`. A box with both bounds reads one partition, for two calls of one series
are one read. In the crossing
`price > in_force(box.hi, box.from <= at, box.symbol = symbol) and preceding(price, at, symbol) <= in_force(box.hi, box.from <= at, box.symbol = symbol)`
both sides read the bound in force at this tick's `at`, not one at the tick before.

It reads nowhere else new. A constraint, a field computed at the write, a set's `where`
and a `find` stay refused.

### What it reads

The write resolves it as it resolves `preceding`: in the decision that judges the
write, under the writer's lock, at the head with what the same append wrote laid over
it. A new `Decide::in_force` stands beside `Decide::preceding`, and both read by
`at_or_below`. It differs in five things.

- **It reads `T`,** the type the call names, not the record's own.
- **At or below.** A `from` equal to `at` is in force, as 052 says, and `preceding`'s
  bar on the record's own key does not apply, for `T` is another type.
- **One partition.** The records of `T` that hold the record's values in every matched
  pair, never the type. It is keyed by the `equal` of `Scan` and walked from the record's
  `by` down.
- **A value of `T`.** `f` of the record chosen, with the record's other fields left out.

- **The grants first.** It asks them before it reads, as `preceding`, on its own type, does
  not.

The rest is `preceding`'s.

- **Whose sight.** Every sight the write is judged by: the writer's, at an accept the
  proposer's and the reviewer's, at a source's delivery the caller's.
- **The grants, before any read.** For each sight, `field_access(T, …).reads()` on
  `T.by`, each `T.m` and `T.f`, and `admit(T)` not `Nothing`. If one fails, the guard reads
  null and reads no row, and the write stands, for a guard refuses no write. `fits` cannot
  do this: `answers` is not withheld and admitted, `admit` is `All` without an `only`, and
  `lowered` sees hidings alone. A read has `hold_paired`, which lowers the field that
  pairs. A guard has no such field, so this is its step. `Masked` does not read, so a
  masked field counts as unread.
- **The hidings, per candidate.** A candidate one sight is not answered is passed over,
  and so is one whose `by` or a matched field a hiding lowers, to `none` or `masked`.
- **`f` lowered or masked reads null,** never the mask and never the older record's `f`.
  The record in force is chosen by what the writer may be shown of its place, and the
  value is then lowered or not, as in 052. A guard compares values, and a mask is none.
- **Null.** Where the record's `by` or a matched field is null, where no record is in
  force, or where the one in force holds no `f`. So `price > in_force(…)` does not fire
  there. `when_strict` holds as for any field without a value: a guard that read null
  does not hold. Without it, a guard that negates the comparison does hold over null,
  and that is how a host asks for a tick of no box: `in_force(…) is null`.
- **No clock.** `by` is the record's own field, so a backdated tick pairs the bound in
  force at its own `at`. A write has no `valid_at`, so `T`'s valid time narrows nothing,
  as for `preceding`, and what it reads is not 090's clock of a setting.

### Refusals

Each is `invalid`, said at `Place::Trigger`, so naming the trigger. 052's judgement of
the form moves into one function that serves a field and a trigger.

| Kind | Members | When |
|---|---|---|
| `in_force_here` | none | `on` is a view: its rows are made by no write. Also a constraint, a field computed at the write, a set's `where`, a `find`. |
| `in_force_form` | none | Any other form; `T` the trigger's own type (`preceding` reads that) or a view. A link type stays admitted, as for a field, for one function judges both. |
| `in_force_pair` | `left`, `right` | A matched pair not of one kind, or not text or a reference; an order pair not of one kind, or of a kind with no order. |
| `undeclared` | `ty` | `T` is no type. |
| `unknown_field` | `ty`, `field` | `T` has no such field. |
| `reads_at_read` | `field`, `ty` | A field of `T` the call names is computed at the read. A bare name so computed is refused `in_force_pair`, as in 052. |

`T` declared again or migrated so that a field a guard pairs by goes, changes its kind
or is computed at the read is refused naming the field alone, as for a field computed
at the read. `paired_by` and `reads_of` read triggers as well as types, and
`migrate.rs`'s check of a trigger after a migration runs where `T` or `on` migrated.

`T` withdrawn while a guard reads it is refused `withdrawn_in_use`, naming the trigger.
`withdrawal` asks `type_reader`, which today names a trigger on the type and reads no
pairing, so a field that pairs `T` has the same gap. It closes for both, and the refusal
names the field.

**Not refused: kinds that do not compare.** `price > in_force(box.label, …)` declares, for
`kind.rs` does not know what another type's field holds, and `holds` ends
`.unwrap_or(false)`. The guard never fires and nothing says so. A field in 052 has the
same limit, at the read. `describe` shows the call, and a host's test finds it.

**Rights.** A trigger is a declaration, so its author reads what it reads (024):
`T.f`, `T.by` and each `T.m`, by `declaration_probes`. `describe` shows it to one who
may read them, and a write's answer does not name it to a writer a hiding lowers one of
them for (006, 024). Today `named_reactions` judges a `when` by `on`'s fields alone
(read), so it learns `T`'s. A firing tells a feed reader that the condition held, as a path
across an edge does (006, 058).

### A delivery judges each record in its order

A delivery is judged in the order it holds (read: `ingest.rs` walks `records` in order
and `reactions` puts each record into `pending` before it judges a trigger). Each tick
reads the head with what the delivery has already judged laid over it: `pending`'s
`in_series`, by `T`, `T.by` and the matched fields.

So a bound and a tick in one append are judged with the bound laid over where the bound
stands first, and not where it stands after the tick. A record the delivery refuses or
quarantines lays nothing over. A bound the append wrote replaces its head version, also
where it moved out of the partition (`holds` keeps the head's copy out of the walk).

Of three ticks and a new bound between them in one delivery, the first tick reads the
old bound and the last two the new. A host that wants a bound to govern a tick puts it
first, or in an earlier append.

### A later write of a bound does nothing to a firing

**A write of a bound fires nothing and moves no firing.** The firing is a fact of the
tick's write. The path `box.hi` is no path of the record: `reaches` does not see it,
and a write of `box` sets off nothing through this guard (read: `children` gives only
this record's side of an `in_force`, and `paths` and `reaches` read by it). A trigger on
`box` is how a host hears of a bound's move. This is why a guard may pair where a stored
field may not. A stored pairing claims a value now, so a later bound makes it false. A
guard's firing claims only what held at its write, so a later bound leaves it true.

A guard is a level, as in 004: a later write of the tick pairs again, at that write's
head, and fires again if the guard holds. A write across an edge that judges this guard
again pairs at its own head.

### An act over a bound, or over a tick

Per 065 a firing stays in the log, and an act decides whether it is overruled. An
`in_force` is the write's, as a `preceding` is.

- **A bound taken back overrules nothing.** A removal, a forget or a purge of `box`, whole
  or in part, leaves every firing paired with it standing, and the firing is still told.
  Which bound governed is the write's to judge, in the writer's sight then, under its
  grants and hidings at the cause. Judging it again would need all of that, and the log
  does not say which record was paired. So `stands.rs` treats a conjunct that calls
  `Func::InForce` as it treats one that calls `Func::Preceding`: kept, not decided again.
- **The tick's own value taken back overrules.** A conjunct reading `in_force` reads the
  tick's `by`, its matched fields and whatever else it names. If the record as of the
  cause holds an erased value of those, the firing is overruled, by the rule that stands.
  `own_reads` already reads only this record's side (read: it goes by `children`).
- **The rest of the `when` is decided again,** as now.
- **What a log keeps.** A firing over an erased bound tells one bit of it. 058 says so of
  every trigger over an erasable field, and this record repeats it for a bound. The
  other answer, overruling it, was decided against (Decided by the operator, 2).

**A write after an act's first turn does not read the erased value.** 099's seam takes a
record from a refold through `redacted` where an owed act reaches it. Its poll rule, which
waits until the projection can decide, does not hold a conjunct of `preceding` or
`in_force`: `stands` keeps that as its write decided it, so a firing paired with an erased
bound after the act began would stand, and both resolvers read `at_or_below` over the
view directly. 099 now names both resolvers in the seam. The same holds for a constraint's
`preceding`, for it shares the resolver.

So each candidate `at_or_below` hands either resolver goes through that seam before
`fits`: one test, O(fields), while nothing is owed by its source, and up to 2 ms a
version on a record reached (reckoned, 099's figure). Each record and version reads
through once in a turn and is held for the turn, or for the append outside one. A bound
whose `by` or match the act erases is passed over, one whose `f` it erases reads null,
and one it retires is passed over as if its part had landed.

The guarantee is no erased value at any part, not the exact record the restate leaves.
The walk goes by the old index until the restate lands, so the record chosen can differ
(reckoned). One whose restated `by` or match would enter the partition is not found. One
whose restated `by` is lowered still stands where it stood, and is chosen above a better
one. The conjunct then holds what the write decided, as any other.

### Where 098 and 099 meet it

| What moved | How the guard meets it |
|---|---|
| Blocks, 098 | No read of a guard touches the log. A firing is one more event in a block. Only reading through refolds, at 099's figure. |
| `flags`, 098 | Nothing. `every(false)` keeps its meaning, and `fits` reads `retired` on the record in Rust. |
| `keys`, 098 | `text_index` and `sort_index` keep text keys. Each state read goes through `keys` on SQLite: one more b-tree, up to 1.2× on points on a small lake (098, measured), so about 1.2× the unindexed row below (reckoned). |
| `sort_index_desc` gone, 098 | The walk down is a backward walk of `sort_index`. Reckoned the same; F5 to F8 measure the descending bound. |
| `view_order`, `sys:kept` heads, 098 | Not read: `T` is never a view. |
| Parts, 099 | An act's retirements fire in their part (065), so a trigger on the retired type judges up to `part_events` guards in a turn, and each reads through. Below. Each part reads its own head. |
| `sort_index`, `lookup_index`, `series_index` beside the writer, 099 and 097 | The guard takes the old road until `Keep` and the same answer after. The cost falls at `Keep`. |
| Declaring, 099 | Declaring and changing a type stay off the road, and a trigger with them. A trigger that pairs a field `lookup_index` does not hold relooks `T`'s exact-text index once, in its declaration, at about 8.6 µs a record (099, measured at 300k). A host with a large `T` writes `lookup_index.T` first, which builds beside the writer, and the declaration then finds it there (reckoned). |
| Staleness, 099 | A trigger that pairs `T` is a dependent of `T` for a declaration's report, by `reads_of`. A record's report does not change. |
| `view_building`, 099 | Does not arise: a trigger that pairs is on no view. |

### What it costs under the writer

"One keyed read" is one read of a partition, and what that costs depends on the road.
`at_or_below` asks for rows of `T` where `m` is one value, ordered by `by`, from one
place down. K is the records of the partition. On SQLite (read) there are four roads:

- **No order index.** `text_index` names the keys, every record is read and sorted in
  Rust. O(K).
- **`sort_index.box`, K up to `FEW_HOLDERS` (4 096).** The holders' keys are read and
  their sort keys compared. O(K), on a smaller row.
- **`sort_index.box`, above it.** The order index is walked down and each row tested for
  a holder, so the cost is the rows above the first holder. That is about the number of
  series where they write evenly, and more for a stale symbol or a backdated tick.
- **`series_index.box = [["symbol", "from"]]`, 097's.** One seek. It is not built yet,
  and takes one series field, so a pairing of two matches stays on the roads above. The
  figure for it is the K = 1 row, a read of one record (reckoned).

Each write reads one partition per trigger, and two calls of one series are one read.
Two triggers read twice. The read comes before the `when` is judged, so no `and` spares
it, as for `preceding`.

**Passing over.** `at_or_below` reads a page of `limit`, `held + 1`, so 1 for a type the
append did not write, and the next page from where it stopped. On the first two roads
every page reads the partition, so a writer who passes over all K would pay K pages:
about 6.3 s at K = 1 024 and 440 s at K = 8 192, under the lock, at every tick (reckoned,
from the p50s below). That writer is common: a feed that holds no read on `box`. Two
things bound it.

- **A sight that reads none of `T` reads no row.** The grants' step above.
- **The page grows.** Each page is eight times the last, so a walk takes ⌈log₈ K⌉ + 1
  pages. A writer an `only` or a hiding passes over pays 31 ms at K = 1 024 and 330 ms at
  K = 8 192 (reckoned), and on the last two roads about eight times the rows it passes.
  `at_or_below` is shared, so `preceding` and the read gain it and no answer moves.

So "reads one partition, shown by a count" is exact for a writer who takes the first
page. One who passes over reads each page's rows again, and the count holds that to
⌈log₈ K⌉ + 1 times the partition.

| K, road | Read, p50 µs | Read, p99 µs | Singles/s | Ingest rec/s | Call of 500, p99 ms |
|---|---:|---:|---:|---:|---:|
| No trigger | | | 1 702; 1 680 | 5 998; 4 763 | 246; 257 |
| `preceding`, up to about 5 000 | | | 1 371; 1 138 | 1 240; 1 056 | 809; 789 |
| 1 | 45; 27 | 107; 68 | 1 406; 1 875 | 3 971; 5 569 | 282; 211 |
| 16 | 138; 106 | 302; 207 | 1 318; 1 806 | 2 759; 3 783 | 357; 270 |
| 16, `sort_index` | 81; 48 | 196; 122 | 1 315; 1 992 | 3 333; 4 832 | 354; 230 |
| 256 | 1 683; 1 296 | 3 419; 2 341 | 398; 528 | 454; 643 | 1 841; 1 072 |
| 256, `sort_index` | 278; 242 | 484; 513 | 690; 1 177 | 1 747; 2 109 | 1 168; 459 |
| 1 024 | 6 285; 6 151 | 17 085; 11 359 | 120; 98 | 127; 145 | 5 999; 6 753 |
| 1 024, `sort_index` | 793; 660 | 2 132; 1 186 | 426; 768 | 820; 1 077 | 1 353; 872 |
| 8 192 | 55 903; 54 328 | 116 876; 105 662 | 15; 18 | 16; 17 | 40 205; 34 040 |
| 8 192, `sort_index` | 441; 284 | 1 044; 683 | 751; 726 | 1 325; 1 963 | 463; 433 |

Measured, round one; round two. Each row has 100 series and 30 000 ticks, but 8 192 has
10 series and 5 000 ticks. "Read" is the timer around one resolve. The other columns
move by up to 40% between rounds with the machine's load, so the read is the figure to
trust, and a write pays its `when` beside it.

So a read costs 5 to 7 µs a partition record with no order index, and about 1 µs with
`sort_index` up to `FEW_HOLDERS`, so about 3 ms just under it (reckoned). Above it the
index holds it under 0.5 ms here.

| Guards in one turn or call, 500 | K = 1 | 16 | 256 | 1 024 | 8 192 |
|---|---:|---:|---:|---:|---:|
| No order index | 14 to 23 ms | 53 to 69 ms | 0.65 to 0.84 s | 3.1 s | 27 to 28 s |
| `sort_index` | | 24 to 41 ms | 120 to 139 ms | 330 to 400 ms | 140 to 220 ms |

Reckoned, from the p50s above; 098's `keys` makes the first row up to 1.2× larger.

**Against 099's turn.** 099 reckons a part at 45 to 150 ms of its 300 ms. Taking the 150,
150 ms remain for 500 guards: 300 µs each. That holds K of about 45 with no order index,
about 256 with `sort_index`, and any K with `series_index`, reckoned at K = 1. Only an act
over a type with a pairing trigger that fires on the retirement pays this: a trigger with
`on_changes: [upserted, ingested]` costs a part nothing.

**Reading through is not in that 300 µs, and it moves 099's bound.** Take a forget of a
source that wrote the ticks and their bounds, with a guard that fires on retirements.
Each of the part's 500 guards reaches a bound the act owes, at up to 2 ms a version, and
the walk stops at the first that fits. All reckoned, for a part of 150 ms and K = 1:

| Bound versions the part reaches | Read through | The turn |
|---|---:|---:|
| Held per turn, 1 series | 2 ms | about 175 ms |
| Held per turn, 100 series | 200 ms | about 375 ms |
| Held per turn, 500 series | 1 s | about 1.2 s |
| Not held, any series | 1 s | about 1.2 s |
| Held, one partition of 1 024 passed over whole | 2 s | about 2.2 s |
| `on_changes: [upserted, ingested]` | none | about 150 ms |

A bound of many versions costs that many times. The last but one is the worst case and
it grows with K: 2 ms each candidate the act reaches and whose part has not landed, once
a turn. A part that has landed shows `retired` and costs no refold. An ordinary tick
pays the same until then, as a verb does in 099. So holding the read-through keeps the
turn near 300 ms only up to about 75 bound versions in a part. The operator decided the
rest (Decided by the operator, 1): 099's bound is restated for this case, at 2 ms for each
bound version an owed act reaches past about 75.

**Against an ordinary write.** No turn bounds it, and the lock is the same. A delivery of
500 at K = 1 024 with no order index holds the writer for seconds (measured: 6 to 7 s a
call at p99), which is a host's declared choice and the reason for the table. `preceding` has
the same roads today (measured: 800 ms a call on these partitions), and 097's series road
serves both.

### Conformance rows

Each runs on every engine. The first twelve answer lakelands#1640's "Done when".

- `GUARD_IN_FORCE_FIRES_ON_ITS_OWN_BOUND`: the guard declares, and fires on a tick above
  its symbol's bound, not on one below, nor on another symbol's tick at the same price.
- `GUARD_IN_FORCE_CROSSING`: with `preceding`, it fires on the crossing and not on the
  ticks after.
- `GUARD_IN_FORCE_BOX`: a box with both bounds fires on leaving either side, and reads
  its partition once.
- `GUARD_IN_FORCE_LATER_BOUND_AND_BACKDATED`: a bound with a later `from` governs ticks
  at or after it, one equal to `at` included, and a backdated tick pairs the bound at
  its own `at`.
- `GUARD_IN_FORCE_LAID_OVER_IN_AN_APPEND`: in one delivery a bound before the tick
  governs it, one after does not, one that moves out of the partition no longer does,
  and one refused lays nothing over.
- `GUARD_IN_FORCE_NO_BOUND`: a symbol of no bound does not fire, with and without
  `when_strict`, and `in_force(…) is null` does, without it.
- `GUARD_IN_FORCE_UNDER_A_HIDING`: a bound the writer is not answered is passed over and
  the older one read; a `by` lowered by a hiding passes over; an `f` lowered to `none` or
  `masked` reads null to the writer and not to the owner; at an accept, by both sights; a
  write's answer does not name the trigger to that writer.
- `GUARD_IN_FORCE_WITHOUT_A_GRANT`: a writer with no read grant on `T`, on `T.by`, on a
  `T.m` or on `T.f`, or with `T.f` masked by a grant, reads null, fires nothing and
  writes; the owner's write of the same tick fires.
- `GUARD_IN_FORCE_REFUSED`: the table above, each naming the trigger, on a view
  `in_force_here`.
- `GUARD_IN_FORCE_KEEPS_ITS_FIELDS`: a field of `T` the guard reads cannot be removed,
  change kind or be computed at the read, by a declaration or a migration, naming the
  field alone; and a trigger's author not shown `T.f` is not granted it.
- `GUARD_IN_FORCE_KEEPS_ITS_TYPE`: `T` cannot be withdrawn under a guard or a field that
  pairs it, naming that reader, and can be once the reader has gone.
- `GUARD_IN_FORCE_READS_ONE_PARTITION`: by `rows_read`, a write under the guard reads no
  more than its partition holds, with the type a hundred times larger; a matched field
  stays looked up (057); a writer with no grant on `T` reads no row of it.
- `GUARD_IN_FORCE_PASSES_OVER_IN_GROWING_PAGES`: by `rows_read`, a writer an `only`
  passes over all K for reads at most ⌈log₈ K⌉ + 1 times K rows, and `at_or_below`'s
  answer to `preceding` and to a read is unchanged.
- `GUARD_IN_FORCE_READ_APART_FROM_PRECEDING`: a guard holding both, with fields named so
  their series spell alike, reads each its own.
- `GUARD_IN_FORCE_LATER_WRITE_MOVES_NO_FIRING`: a write, a retirement or a new bound
  after the firing sets nothing off through the guard, and the feed tells the firing as
  it was.
- `GUARD_IN_FORCE_BOUND_TAKEN_BACK_STANDS`: a removal, a forget and a purge of the
  bound overrule no firing paired with it, and the firing is told; a removal of the
  tick's `at` overrules it; the rest of a `when` is decided again.
- `GUARD_READS_THROUGH_AN_OWED_ACT`: with the restate held back, `in_force`, and
  `preceding` in a guard and in a constraint, read no erased value after the act's first
  turn, whichever parts have landed, and no firing or refusal rests on one; a record and
  version reads through once in a turn.
- `GUARD_IN_FORCE_TAKES_THE_SERIES_ROAD`: with `series_index.box`, the answer is the
  old road's on every engine and, where the engine keeps the index, by `rows_walked`, a
  guard walks one row.

### The order of the work

After 098's F12, beside record 097's steps. Each pull request stays at or under 600
lines at ×1.35.

| Step | What | Product | Spec. | PRs |
|---|---|---:|---:|---:|
| G1 | The form: `check`, one judgement of a pairing, `Case` members, `looked_up`, `reads_of`, `paired_by`, `migrate`, `type_reader`, the probes, `named_reactions`; Appendix B and the 22 → 23 line | 175 | 240 | 1 |
| G2 | The resolver: `Decide::in_force`, `paired` in the scope, `holds_at_write`, one read a series, the growing page | 120 | 290 | 1 |
| G3 | Sight, and what follows the firing: the grants' step, a masked `f`, `stands.rs` and the rows of a bound taken back | 70 | 270 | 1 |
| G4 | A guard reads through an owed act, both resolvers, held per turn | 60 | 160 | 1 |
| G5 | The count, the series row, the cell in `tick_bench`; in `docs/hosting.md` K's figures and that a writer with no read on `T` fires nothing and is told nothing | 20 | 90 | 1 |
| | **Sum** | **445** | **1 050** | **5** |
| | **×1.35** | **601** | **1 418** | |

G1, G2 and G3 land in order and ship in one release: after G1 alone a guard declares and
reads null, and after G2 alone any act overrules its firings. They land before the tag of
format 23, whose kept lake is written then and gains a trigger that calls `in_force`, with
a firing, as format 22's holds each form it added. If they miss that tag the widening is
format 24's step. G4 stands on R2's seam, and no release holds R2 without it: `preceding` has the gap from R2 on, whatever becomes of this record, so if
this record waits, `preceding`'s half goes in R2. G5's count stands on 097's step 5 (the
memory engine reads the type today, read), and its series row on 097's step 9, whose
road must take `Scan`'s `equal` as it takes a `where`. The cell in `tick_bench` needs
lakelands#1657.

These are reckoned. The ceilings of lakelands#1578 leave 9 653 product and 9 653
specification lines over `16e687eda` (measured, `core-size.sh`). At ×1.35, with 097's
1 785 and 2 459 and 099's net of 5 787 and 3 874, the work takes 8 173 and 7 751, so no
raise lands with this record. At ×2, which 097 says this round's pull requests ran, this
record is 890 and 2 100 and the whole is 12 391 and 12 682, over the reserve. Each step
then measures its size and any raise goes by `docs/budgets.md`. If lines must go, G5's
series row waits. Nothing else can: G1 to G3 are the ask, and G4 is what R2 owes.

### Decided by the operator

Decided on 2026-11-01 (lakelands#1578): every recommendation below was taken.

1. **Reading through against 099's "about 300 ms a turn", which the operator decided on
   lakelands#1578.** Each read-through is held per record and version for the turn, as
   this record does, and 099's bound is restated for this case: a part adds 2 ms for each
   bound version an owed act reaches past about 75, up to about 1.2 s at 500 and about
   2 ms × K for a partition passed over whole. Answers stay what the act leaves. The
   other road, passing over any candidate an owed act reaches with no refold, was not
   taken: the bound would hold with no memo, but a removal of one field of a bound would
   silence the guard on it for the length of the act, and those missed firings would stay
   missed. A turn that runs long is seen and ends, a seal already takes about 2 s, and
   `on_changes` frees a host from it. A firing missed in silence does neither.
2. **A firing paired with an erased bound stands.** Overruling would hide the one bit the
   firing tells, but it needs the partition resolved as of the cause under the grants and
   hidings of then, and the log does not name the bound. The firing stands, on 065's line
   for a predecessor, with 058's warning extended to a bound.
3. **A large partition** is served by `sort_index.box` or `series_index.box`, with K's
   figures in `docs/hosting.md`: the proper road and no new machinery. A partition kept
   per append, as a read keeps 1 024 (about 30 lines, about 620 ms for 3.1 s where 500
   ticks share 100 series at K = 1 024, reckoned, and no help to singles or to 500
   series), was not taken. Nor was refusing a pairing without `series_index.T`: too
   strict, since a pairing of two matches has none, and it orders two settings.

## Why

A host keeps a threshold per series as records: one trigger over all series, and a
bound a person can move. Without this it declares a trigger per bound, each judged on
every write, or it reacts to every write and decides by itself, which takes the deciding
out of the lake.

052 refused a stored pairing because it goes stale. A guard does not go stale, for it
says what held at its write, and the firing is that fact. So `in_force` in a guard needs
no new event, no index and no table: it is `preceding` over another type, and the
writer's lock already gives it the head.

An act keeps to the same line. What was paired was the write's to judge, and 065 already
says so of a predecessor, so a bound taken back overrules nothing. A value erased must
still reach no new firing, so a write reads through.

## What it is not

- **Not a firing on a bound's move.** A trigger on the bound's type does that.
- **Not a window of past records.** A box from history is computed by the host and
  written as a record.
- **Not `in_force` in a constraint, a stored derived field or a set's condition.** Each
  is judged later than its write, or stored, so each would go stale.
- **Not a firing on a clock.** A trigger fires on writes.
- **Not a record of what was paired.** The firing does not carry the bound. A host that
  needs it reads `box` as of the firing's position.
- **Not a new index.** `series_index.box` is 097's, and a host's to ask.
- **Not a promise of these numbers.** They come from a scratch build; G2 and G5 measure
  their own.
