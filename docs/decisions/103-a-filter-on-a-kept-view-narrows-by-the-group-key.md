# 103 — A filter on a kept view narrows by the group key

**Accepted** by the operator on 2026-11-03, for 0.23.0, on epic
lakelands#1578. The build of lakelands#1634 follows.

Decided on lakelands#1634, for 0.23.0. No code yet. Green field: format
23, and no compatibility is kept. Standing on record 097 (the window over
an ordered field and `rows_walked`, being built in lakelands#1693 and its
pull lakelands#1699), record 098's F10 and F11 (`view_order`, merged with
lakelands#1692), record 102 (a stranded group, landing with lakelands#1697
and lakelands#1704) and record 101 (a zoned bucket). `Scan` gains one
member and no seam method is added; `LAYOUT` does not rise.

## What is decided

### The group key is the road

A kept view's record for a group is keyed by that group's values, in the
order the view declares its `group_by`, by the one key rule
(`docs/log-format.md` §2.3; `Kept::key`, `record_key`). The key is
already the whole of what the groups are ordered by: each engine holds
it in the structure it reads a kept name through — SQLite's
`keys (ty, key, id)` table, PostgreSQL's `records` and `records_head`
rows keyed by type then key, the memory engine's map keyed the same —
in that order, and seeks it. A read of a kept view therefore narrows by
turning its filter into an interval on that key and handing the interval
to the engine. It adds no table, no column, no index, no setting and no
`LAYOUT`.

The declared order is the order that narrows. A view grouped by
`[symbol, date_bucket(at, 1m)]` narrows by an exact `symbol` first and a
bound on the bucket second. A view grouped by `[date_bucket(at, 1m),
symbol]` narrows by the bucket first; an exact on `symbol` behind an
unfixed bucket narrows nothing, because the key cannot reach it. The host
chooses the order when it declares the view; a read does not reorder it.

### What narrows, and the interval it makes

The filter is read as the conjunction of its `and` terms, as record 097
reads it. A term is a comparison of one group field with one literal:
`=`, `>=`, `>`, `<=` or `<`. The group fields are taken in the declared
key order, and the walk stops at the first field that does not narrow; a
field behind a bound never narrows alone. The two ends of the interval
are built apart: the lower from the `=` and the `>=` and `>` terms, the
upper from the `=` and the `<=` and `<` terms. An end no term fixes stays
open.

- **A text or reference field narrows an exact only.** The literal must
  read as neither a number nor an instant: `compare` coerces two texts
  that read alike, so `'0123'` and `'123'` are equal, and a literal that
  reads as an instant equals every spelling of that instant. Any other
  literal narrows nothing.
- **A date-time or date field narrows every comparison that record 097
  admits a window for.** The literal must read as an instant and not as a
  number (097's rank rule). A `date` field's component is the literal
  instant's UTC day, floored on both sides. Its key may hold that day's
  text (a direct group field, landed as a day) or an instant of that day:
  a group term's value is never landed, so a `date_bucket` term's zoned
  start (record 101) is what the key holds. An upper end is therefore
  the day's prefix — the day's text and `~`, which sorts above the `T`
  of an instant's text and below the next day — and an exact on a date
  ends the walk, since a later component is not reachable past both
  spellings. The rulebook still decides every row.
- **A boolean field narrows an exact given as a boolean literal**; it
  takes no bound.
- **A number or decimal field narrows nothing.** Its component is the
  value as written — a decimal's `7.00` and `7.0` are two key components
  and one number, and a JSON number canonicalises to one spelling — and
  its key order is not the number order. A text range narrows nothing
  either: the key rule writes `:` as `%3A` and `%` as `%25`, and the
  escape does not keep the order of the text.
- **`or`, `not`, `in`, `!=`, a term under a function, a comparison of two
  paths and any other form narrow nothing.** They stay whole.

An end is built from the components of the fields in their declared
order. A field with an `=` term fixes its component: the component is
appended to both ends, and the walk goes on to the next field; its other
terms narrow nothing further, since every row the filter admits holds
the `=`. A field with no `=` but with a bound appends its literal's
component to that bound's end — a `>=` or `>` to the lower, a `<=` or
`<` to the upper — and ends the walk for both ends. A field with no
term ends both ends where they stand. The interval only narrows; the
rulebook admits every row again.

- The lower end is its components joined by `:`, inclusive whatever the
  operator was. A key whose component equals the literal's sorts after
  the lower key; the rulebook rejects it on a `>`. A key with a smaller
  component is of a value the rulebook rejects too, except where one
  component is a prefix of the literal, which widens the interval.
- The upper end is the last component an `=` fixed, or a `<=` or `<`
  term's component, followed by `;`, exclusive. The `;` sorts after the
  `:` that joins a component to the one after it, so every key whose
  component at that position is the literal or below it sorts under the
  end; a component above it that begins with it may sort under the end
  too, again only widening.
- **A followed exact is exact.** When the last component the lower takes
  is an `=` on a field that another group field follows, the lower adds
  the `:` that joins them: `[x:, x;)` holds exactly the groups whose
  field is `x`. A value extending `x` puts, after `x`, the first byte of
  its escaped suffix: `%` or another byte below `:`, or a byte from `;`
  up, never the `:` that joins components, so its key is below the lower
  or above the upper. Without the `:`, the extending value's key
  (`m-70` for `m-7`) would sort between the ends.
- **A final exact cannot exclude the extensions.** On the last group
  field no `:` follows the value in the key, so `[x, x;)` holds every
  group of `x` and also a group of a value that extends `x` and whose
  next byte sorts under `;` (`m-70` for `m-7`: `0` is under `;`). The
  answer is still the rulebook's and the interval is still a superset;
  the walk is that much longer, and the row counts it.
- A bound leaves the other end open, whatever the terms on the fields
  behind it: `b >= lo` alone is `[lo, open)`, and a view grouped `[b, g]`
  under `b >= lo and g = x` is `[lo, open)` too, because a `b` above `lo`
  admits every `g`. `b < hi` alone is `[open, hi;)`; the fields' terms in
  any order in the filter make the same interval.
- Bounds that cross hand an empty interval, which answers none as the
  filter itself would.
- The interval is a superset of the keys the filter admits, on every
  engine. `KEPT_FILTER_NARROWS` holds it against the computed view, with
  one-sided terms, a reversed term order and an extending value among its
  cases.

The key rule's escapes are why an exact on text is sound and a range is
not: the components `a&b` and `a:b` are written `a&b` and `a%3Ab`, and
`%` sorts before `&`, the reverse of the texts' order.

### What stays a full walk, and why

- **A field behind one that did not narrow**, and every field of a view
  declared in an order the filter does not prefix. The key cannot reach
  a second component before the first is fixed.
- **A number or decimal group field, a text range, an instant- or
  number-reading text literal, `or`, `not`, `in`, `!=` and a function's
  term**, for the reasons above.
- **A view with no `group_by`**, whose one record is keyed `all`
  (`UNGROUPED`): there is one row, and the filter decides it.
- **An earlier position**, where the view is computed (098's
  `KEPT_HEAD_ONLY_EARLIER_READ`): there are no kept group records to
  narrow, and the answer is what the computation already answers.
- **A page ordered by something other than the remaining safe-kind group
  fields**, which reads the order index (098's F10 and F11) as it does
  today. The interval would not cut that walk, so it is not passed there.

### Head, earlier position, and a differing face

At the head the interval is the head's key interval. At an earlier
position the view is computed as today, and the filter narrows nothing of
the kept records; the suite holds the answer equal (`KEPT_FILTER_POSITIONS`).

A kept view whose faces differ — a view read as the whole face where a
group's faces differ, or after record 101's zone or a setting changed the
computation — reads its kept page under the same interval and merges the
differing groups as today. The interval comes from the filter and the
group fields alone, so it holds every group the filter admits; the faces
are a superset concern on the same rows and the answer does not change.

### A page in the group fields' order

A read that asks for the order of the remaining group fields after the
fixed ones, ascending, is served by the same key order only where the
key's bytes order as the comparator does. That holds for a date-time or
date group field, whose key component is its canonical UTC text and
whose comparison is its instant, and for a boolean field, whose
components `false` and `true` sort as they compare. It does not hold for
a number or decimal field, whose components are written text (`9` after
`10` bytewise, before it in number order), nor for a text or reference
field, whose values that read as a number or an instant compare that way
(`9` before `10`, one instant's spellings equal) while their keys are
bytewise. So a page is served from the key only when every remaining
ordered field is a date-time, date or boolean; a text, reference, number
or decimal field among them keeps the page on the order index.

Where it is served, the page reads the interval and needs no order
index, and its cursor continues by key; within the symbol's prefix, the
m1 keys run in bucket order. This is the case record 098's F10 and F11
cannot cut: an order index inside a bucket bound holds one entry for
every series' group in the bound, and a bound on the bucket alone walks
them all. The dense run below measures the shape: one day's bound holds
all 144 000 groups of the view, against 1 440 under one symbol's prefix.

Every other order — a measure, a descending order, or an order by a
group field that is not the next unfixed one — stays on the order index.
`view_order` is not removed and is not made redundant.

### The engine seam

`Scan` gains `range: Option<KeyRange>`, where `KeyRange` holds a key
from, inclusive, and a key until, exclusive, each none for an end the
filter leaves open. No trait method is added; the engine seam stands at
the 104 of 110 methods the tree carries (record 098 left it at 98, and
later records raised it), and `Scan` is not a wire
type. Every engine honours the range on its key-ordered road:

- SQLite writes the predicates into the statement `kept_sql` already
  builds for a kept name (`AND k.key >= ?` and `AND k.key < ?`, beside
  the `after` bound `key_from` writes), each only when its end is set;
  the count's statement takes them too. The plan is a seek of `keys`'
  primary key.
- PostgreSQL reads a kept name through the same `records` and
  `records_head` tables as every name, keyed by type then key, and writes
  the predicates into `records_after_sql` and `count_in_force_sql`
  (`AND h.key >= $` and `AND h.key < $`). Its seek is the head table's
  primary key at `ty` the kept name; it has no `keys` table, which is
  SQLite's alone.
- The memory engine starts its `BTreeMap` range at the from key and stops
  at the until key; its own `records_after` takes the bounds (its
  signature changes, the trait's does not), and a from past the until
  answers none before `BTreeMap::range`, which panics on a reversed
  range, is asked.
- `scan_in_key_order` and `count_in_key_order` (097) stop when a batch is
  short, which the bound makes true at the interval's end; nothing past
  the bound is handed.

`rows_read` does not show the narrowing, because it counts the rows an
engine hands after its own filter, not the keys it walks. The dense run
below shows both: an unordered `find` under the filter hands 1 003 rows
of 144 000 and walks all 144 000 keys, while an aggregate under the same
filter reads all 144 000. So the rows count `rows_walked`, as 097
defines it.

### What it adds to a write

Nothing. The group key is already the record's key, written once per
group when the group is folded, and the record adds no row, column,
index or setting to that write. Measured on SQLite at the workload of
lakelands#1637 (100 000 ticks, three kept roll-ups, the scratch harness
outside the tree):

| what | µs a tick | written, B a tick | after `VACUUM`, B a tick |
|---|---:|---:|---:|
| no kept view | 141.7 | 1 594 | 783 |
| three kept roll-ups | 1 621.3 | 5 994 | 3 556 |

The kept `keys` rows of that run are 93 rows and 3 441 key bytes over the
100 000 ticks — 0.03 B a tick — and a group's key is its own 36 B (for
example `S042:2026-10-01T00%3A00%3A00.000000Z`). The bytes repeat
record 098's 5 994 and 3 556 exactly and the time is in 098's range; the
one-minute load was 7.7 and 6.3, so the time is contention-bound while
the bytes are exact.

What a different road would add is measured too. An order index on the
kept view (the `sort_index.<view>` setting) adds 169 B a tick on the
dense run (3 918 to 4 087 B a tick, one kept m1 view), and a text index
beside it cannot be declared at all: `lookup_index.tick_m1` is refused —
a kept name is no declared stored type. The key road adds neither.

### Conformance rows

Each row is one engine-independent check, run on every engine; the rows
that count count `rows_walked`.

- **`KEPT_FILTER_NARROWS`** (store checks, beside `KEPT_ORDER`): a view
  grouped `[g, b]`, where `g` is text and `b` a bucket of time, a read
  filtered `g = x` answers the records, a count and each aggregate as the
  view computed answers them, and `rows_walked` is the groups of `x`, not
  a value extending it (`m-70` is not walked for `g = 'm-7'`); the reads
  filtered `g = x and b >= lo`, `g = x and b < hi` and
  `g = x and b >= lo and b < hi` answer as the computation, the terms in
  the filter's reverse order answer the same, and the end the terms leave
  open stays open; a window covering every bucket answers as the exact
  read, a crossed bound answers none, and a symbol with no group walks
  none. A view grouped `[b, g]` with `g` last, read `b = lo and g = x`,
  answers as the computation and walks the groups of `x` and of a value
  extending it (`m-70` for `m-7`), which the final component's
  `[x, x;)` cannot exclude.
- **`KEPT_FILTER_WHOLE`** (the same file): every filter the key cannot
  carry answers as the computed view and walks the whole view — `or`,
  `not`, `!=`, `in`, a term under a function, a text range, a text
  literal that reads as a number or an instant, a number group field, a
  boolean bound, and a term on the second group field alone.
- **`KEPT_FILTER_POSITIONS`** (the same file): the same reads at an
  earlier position answer as the computed view and narrow none of the
  kept records; a stranded group of a thinned series (record 102) inside
  the interval is answered with its folds and one outside it is not read;
  after a write moves an extreme, a first or a last out of a group and
  back, the narrowed read and its aggregate answer as the computation.
- **`KEPT_RANGE`** (engine checks, beside `KEPT_NAME`): a scan of a kept
  name with a range hands exactly the states in it, in key order, across
  pages and batches, with `after`, with one end open and with both; a
  from past the until hands none; and `rows_walked` is what it hands.
  Each engine's own test reads the plan: SQLite's a seek of `keys`'
  primary key, PostgreSQL's a seek of the head table's primary key at the
  kept name, the memory engine's a range of its map.
- **`KEPT_FILTER_PAGE`** (store checks, beside `KEPT_ORDER`): a page
  ordered by the remaining group fields answers in the comparator's
  order across pages and batches, from the key where every remaining
  field is a date-time, date or boolean, and from the order index where
  one is text or a number; a text group field holding `9` and `10`
  answers `9` first, as the computation does, and the key order's
  bytewise `10` first is not served.

### The build plan

Three pull requests, each well under 600 changed lines, after
lakelands#1693's counting lands (record 097, pull lakelands#1699), on the
tree record 098's F10 and F11 are in (lakelands#1692), and with record
102's thinning (lakelands#1697 and lakelands#1704), which P2's stranded
read and `KEPT_FILTER_POSITIONS` need.

| Step | What | Files | Product | Spec. |
|---|---|---|---:|---:|
| P1 | `Scan.range` and `KeyRange`, either end open; the three engines' key roads; `KEPT_RANGE` | `api/src/engine.rs`, the three engines' `view.rs`, the memory engine's `projection.rs`, and tests; `conformance/engine/reads.rs` | 120 | 90 |
| P2 | the interval from the filter; the store's kept read, count and aggregate narrowed; the three filter rows | `core/src/engines.rs`, `store/src/kept.rs`, `store/src/reads/describe.rs`, `store/src/reads.rs`, `conformance/checks/view_kept.rs` | 170 | 210 |
| P3 | a page in the remaining safe-kind group fields' order; `KEPT_FILTER_PAGE` | `store/src/reads/describe.rs`, `store/src/reads.rs`, the same check file | 70 | 100 |
| | **Sum** | | **360** | **400** |
| | **×1.35** | | **486** | **540** |

P1 and P2 are one claim split where the seam allows a green test: P1
proves the range on the engines, P2 proves the interval. The round's
0.23.0 reserve already names lakelands#1634, and the ceilings hold
without a raise: the tree stands at 101 108 product of 108 500 and
185 592 specification of 191 500, leaving 5 908 specification lines
against this plan's 540 at ×1.35.

**Cut first, with what each costs:**

1. **P3, the page in the remaining safe-kind group fields' order (70
   and 100).** A page ordered by a measure or a text field stays on the
   order index, as a page ordered by the bucket does when P3 is cut; the
   filter, the count and the aggregate still narrow. The issue's ordered
   read then keeps today's cost inside the bound.
2. **The boolean exact (10 and 15).** A boolean group field stays whole.
3. **The date field, as against a date-time one (15 and 20).** A direct
   `date` group field stays whole; buckets are date-time fields and are
   not cut.

Not to cut: P1's range, P2's interval, the text and reference exact, the
date-time and bucket bounds, and the count and aggregate roads. Without
them the done-when lines are not met.

### Open for the operator

Nothing. The order that narrows is the order the view already declares,
and a host that wants one series' day first declares `[series, bucket]`.
The record does not reorder a view behind the host's back.

## Why

- **A kept view exists to make the read cost follow the groups asked
  for.** If a filter reads the whole view anyway, the view's length is
  the cost the host kept it to avoid, and a year of minutes is 105
  million groups for one series' last day.
- **The key already orders the groups.** The group's values are the
  record's key, written once when the group is folded, in the one key
  order every engine shares. Narrowing there costs nothing to write and
  nothing to keep.
- **A second index would still walk the bound.** An exact text index
  beside an order index finds a series' groups but cannot order the
  window within them without walking every series' groups inside the
  bound; the key's order is the only one that holds both. The scratch
  measures the shape: 144 000 groups inside the day's bound against
  1 440 under one series' prefix.
- **The rulebook still decides.** The interval is a superset, and every
  answer is the computed view's; no comparison changes meaning.
- **`rows_read` cannot prove it.** An engine that pushes the filter hands
  few rows while walking all the keys; a count of index rows walked is
  what the check needs, and 097 gives it.

## What it is not

- **No answer changes**, at the head or at an earlier position, with or
  without differing faces.
- **No new query language, index, setting, table or `LAYOUT`.** A kept
  view's key is where it was.
- **Not a narrowing of stored types.** Record 097's window is that road;
  this record adds none of its own.
- **Not `or`, `in`, `!=`, a function's term, a number or decimal group
  field, a text range, or a second group field behind an unfixed first.**
- **Not a change to an ordered read the key order does not serve.**
  Measure, descending and unreached orders stay on `view_order`.

## How it came to be

- Written for lakelands#1634, from a read of the tree (record 097's rank
  rule and counting, record 098's F10 and F11, record 102's stranded
  groups, record 101's zoned buckets) and two scratch runs on SQLite
  outside the tree: the workload of lakelands#1637 at 100 000 ticks and
  three roll-ups for the write, and 100 series a minute for one day
  (144 000 ticks, 144 000 m1 groups) for the read. PostgreSQL and the
  memory engine were not measured; their rows are the suite's, and their
  cost is the plan's.
- A review of the first draft corrected the interval to two ends built
  apart (a one-sided term had dropped admitted rows), the key-ordered
  page to the kinds whose key bytes order as the comparator does, and
  each engine's road by name: PostgreSQL reads a kept name through
  `records_after_sql` and `count_in_force_sql`, seeking the head table's
  primary key, and has no `keys` table.
- The brief says `cargo run -p xtask -- adr-new`. The tree has no
  `xtask` crate — the workspace's members are the twelve crates under
  `crates/` — so the record was written by hand in this directory's form,
  and the plan is corrected in the pull request's body, as the tree wins
  over the plan.
- The measured read: against 144 000 keys, the prefix range
  `[S042, S042;)` holds 1 440; the day's bound across every symbol holds
  144 000; the plan seeks `keys` by its primary key. An unordered `find`
  under the filter handed 1 003 rows in 22.9 ms while walking every key;
  an aggregate under the same filter read 144 000 rows in 2 050.6 ms; a
  page ordered by the bucket with its order index handed 1 001 rows in
  20.5 ms. That is why the rows count `rows_walked`.

## As built

**P1 and P2 in one pull request (lakelands#1634).** The brief makes
them one claim and one PR, where the plan split them: the engine seam
and its three engines, and the interval and the store's narrowed reads,
with the four conformance rows (`KEPT_RANGE`, `KEPT_FILTER_NARROWS`,
`KEPT_FILTER_WHOLE`, `KEPT_FILTER_POSITIONS`).

- **P1, the seam.** `Scan` gains `range: Option<KeyRange>` and
  `KeyRange` its two ends, each none for an open end. No trait method is
  added: the seam stands at the 104 of 110 methods it stood at before.
  SQLite writes `AND k.key >= ?n` and `AND k.key < ?n` into
  `kept_sql`, beside the `after` bound, for the page and the count;
  PostgreSQL writes `AND h.key >= $n` and `AND h.key < $n` into
  `records_after_sql` and `count_in_force_sql`; the memory engine's
  `records_after` takes the bounds and answers none before
  `BTreeMap::range` when the from is past the until. Twice the tree
  corrected the sketch: SQLite's parameter numbers shift with the
  interval's ends, so the `after` bound is numbered after them, and
  PostgreSQL's count at an earlier position bounded the in-force row's
  key rather than the head's, which the plan test caught (a filter
  after the lateral, not a seek); the interval now bounds `h.key` on both
  roads, as this record names them. The engine rows run on every engine.
  Each engine's own plan test reads the seek: SQLite's
  `a_kept_name_read_inside_an_interval_seeks_the_keys_inside_it`, the
  memory engine's `a_range_bounds_the_key_walk_and_a_crossed_one_answers_none`,
  PostgreSQL's `a_kept_name_read_inside_an_interval_seeks_the_heads_primary_key`.
- **P2, the interval and the store.** `engines::key_range` builds the two
  ends apart, in the declared key order, and stops at the first field
  that does not narrow; `group_component` admits the kinds this record
  admits and no more. A date-time or date literal is read by
  `Timestamp::parse`, not `denoted`: an instant outside the years 0000
  to 9999 writes a component (`10000-…`, `-0001-…`) on the wrong side of
  every stored key, and a bound built from it would exclude rows the
  filter admits; such a literal narrows nothing. A date field's upper
  end and its exact are made on the day's prefix, not the day's text
  alone: a `date_bucket` term declared under a date field stores its
  zoned start, and `day = '2026-01-05'` must reach the key
  `2026-01-05T…` as well as the day's text; the walk ends at a date
  exact, a later component not reaching both spellings. The store derives the
  interval only for a kept name (`seam::is_kept_name`), hands it to
  `find`'s unordered read, to the count (the `total` door) and to the
  aggregate (which reads through `each_member`), and withholds it from a
  page that asks another order (P3). An engine's text lookup a kept name
  would take (an exact on a text group field, or on a text key) is
  withheld too when the interval is set: the interval owns the read, so
  the walk is the interval's on every engine, not the lookup's. Store
  checks run on the blocking
  pool, where the thread-local `rows_walked` is invisible, so
  `engines::rows_walked_in_process` counts the walk process-wide, as
  `rows_read_in_process` does for `rows_read`; the harness runs checks
  in order, so a check's delta is its own. The counter is shared by the
  whole process: a conformance binary that runs several checks in one
  process at once (a plain `cargo test` of a sharded suite) would make a
  delta inexact; CI runs nextest, a process for each test.
- **The count door and the year guard, each planted.** `KEPT_FILTER_NARROWS`
  reads the `total` door too: the page walks the three groups of `x` and
  the count walks them again, six in all, and a count that read the whole
  view would walk ten. `KEPT_FILTER_WHOLE` holds a bound on the bucket
  the filter asks first, in an instant outside the written form's years:
  planting the guard (reading the literal by `denoted`) turns the `flip`
  rows red, because the component `10000-…` sorts under every key.
- **The date's day prefix, the bucket's extremes.** `KEPT_FILTER_NARROWS`
  now reads a view grouped by a date field whose term is a day bucket:
  the key holds the bucket's start instant, not a day's text, and the
  exact answers the group the computation answers while the bounds walk
  the day's prefix. The view's measures include a bucket's `max`,
  `first` and `last`, read under the filter through the records, an
  aggregate and its twin. `KEPT_FILTER_POSITIONS` moves a group's least
  and first, its greatest and its last out of the group and puts each
  back.
- **What the tree did not have.** Record 102's S3 (the thinning act) did
  not land; only its tables did. `KEPT_FILTER_POSITIONS` therefore does
  not hold its "stranded group of a thinned series" clause: there is no
  act to strand one. The check holds the earlier position (the view
  computed, the kept read unanswered there) and the writes that move a
  group's extreme, first and last out and back. The plan is corrected
  here, as the tree wins.
- **The seam figure.** The draft named the 98 of 110 methods record 098
  left and the 102 P1 stood at; the tree stands at 104, as
  `scripts/gates/seam-width.sh` measures. Both are corrected here, as
  the tree wins.
- **Measured at the head after this PR's own `git add`**, with
  `origin/develop` merged: product 104 265 of the 115 000 base,
  specification 188 847 of the 202 500 base; seam 104 of 110; 6 of 20
  `cfg(feature)` sites; 1 024 checks through the doors and 98 through the
  engine seam; 1 018 refusal cases.
  The PR's own lines: product +602 −104, specification +742 −3. The
  plan's 290 product and 300 specification for P1 and P2 were the
  sketch; the extra is the four conformance rows, the three engines'
  plan tests and the date's day prefix. Nothing was cut.
