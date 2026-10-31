# 097 — A read in a window walks its window

Proposed on lakelands#1633, lakelands#1635 and lakelands#1638, for
0.23.0. No code yet; the build waits for the word.

**No data migration and no breaking change.** The log format stays,
no stored table changes and `LAYOUT` stays on both engines, so a kept lake
is not rebuilt. A kept lake pays only when its host writes a new setting.
Additive: one setting, one wider setting range, one `ProjectionView` method
with a default, one `describe` member. `Scan` is unchanged.

## What is decided

### What `rows_read` counts, and what is added

`rows_read` counts the rows handed to the shared code, not the rows
walked. Only `engines.rs` adds to it. Checked road by road:

| road | walked | handed |
|---|---|---|
| `Holders::All` | `n` | `n` |
| `Holders::Many` | every order row up to the `n`th holder | `n` |
| `Holders::Few` | every holder, `M`, per statement | `n` |
| memory | what it hands | everything |

So `rows_read` stays green on the walk `Many` hides, and it cannot prove
a done-when line about walking. It keeps its meaning. **`engines::rows_walked()`**
is added beside it, counting index rows:

- A pure range statement walks what it hands: the same number is added.
  The plan tests pin that the statement has no residual term.
- `Few` adds `M` per statement; `Holders::Few` carries the count that
  `Holders::of` already reads.
- `Many` keeps its statement. While a check has asked for walks
  (`engines::counting_walks`, off in a product read) the engine also runs a
  `count(*)` of the same range, from the seek to the last row handed, or to
  the range's end on a short batch. That is the walk: the statement passes
  those rows in order and no others.
- The memory engine adds what it hands.

Handing each walked row back with a held flag was tried and refused. It
needs no switch, but on SQLite it cost 8.1 ms against 0.75 ms for a page of
a text held by 60 000, and 3.4 s against 0.65 s for one held by 149
(600 000 records, batches of 1 024). 041's measured pages would not hold.

### The window

`engines::window(filter, field, kind)` takes from the `and` terms of a
filter the terms `field op literal`, with `>=`, `>`, `<=`, `<` or `=`, and
answers the key interval `[lo, hi]`. There is no other range form: a range
is two terms. `or`, `not` and a term under a function give no bound.

- **Only where one rank holds the field.** Numbers and decimals are rank 0,
  date-times and dates rank 1, and the literal must read in that rank by
  `sort_key`. `compare` is byte-wise between two texts of different ranks
  (`'5' > '10x'`), so a text or reference field takes no window, nor does
  any field under a literal of another rank. That read stays as it is.
- **Keys are cut and the ends inclusive.** `lo` and `hi` are `order_key`s,
  so a long decimal shares its cut key with its run, and a strict term walks
  the rows of its one key and the rulebook rejects them. An open side ends
  at the rank's edge.
- **A day bound** is the midnight `Timestamp::denoted` reads, as `compare`
  reads it.
- **The rulebook still admits each row.** The window only narrows.
  `WINDOW_HOLDS_WHAT_IS_ADMITTED` holds that the interval contains every
  value a comparison admits, on a generated set.
- **A window skips the `missing` walk.** A comparison with no value is
  false, so a read bounded on its first order field has no records without
  the field to add. Today that tail would walk the type after a short
  window, on all three engines and every `Holders`.

### The read road

At the head only. An earlier position takes `scan_in_field_order_at` as
today; a narrowing there is welcome and not built.

1. **A key of one text field** is read by key, as today.
2. **A series index holds the pair:** the seek below, on this index alone.
   `Holders` is not asked.
3. **Otherwise, under an exact text:** `Few` joins holders to order rows
   and bounds them, walking `M`. `Many` walks the order index inside the
   window and probes the text. So without a series index the walk is
   bounded by the holders or by the window's rows of the whole type.
4. **No exact text:** the order index inside the window.

**A page in key order** counts the window's rows up to `FEW_HOLDERS` + 1.
At that or under, it reads their keys, sorts them, cuts the page and seeks
the records. Above, it takes the key-order road as today, because sorting a
large window for each page is a square.

**An aggregate and a count fold in key order.** lakelands#1633 says they do
not depend on order; they do. `first` and `last` keep the least key among
equals, and a sum that is not exact is what adding in order gave. So the new
`ProjectionView::window_keys(ty, scan)` answers the window's keys, or `None`
(the default, no index, past a ceiling of 1 048 576 keys). The store sorts
them and reads records by `get_many` a `COUNT_PAGE` at a time, and every
answer stays. On SQLite this cost 1.6 ms against 1.0 ms for reading the
1 000 records of a window in index order. Past the ceiling the read walks as
today, having lost at most the keys' read.

A filter the store keeps whole (`store_only`) pushes nothing today. It
stays so: a window with a store-only term reads as it does now.

**`in`.** A series or number field under `in` reads each value. A page in
field order seeks every series from the same position, one batch each, and
`engines` merges them by (sort key, key): one statement a series on SQLite
and memory, one `unnest … LATERAL` statement on PostgreSQL. A batch of `n`
walks at most `series × n` index rows and reads `n` records.

### The memory engine

It keeps an exact lookup. Its done-when lines say every engine, and the
conformance suite cannot hold a claim one engine does not meet. It keeps
`BTreeMap<(type, field, value), BTreeSet<key>>` for the `looked_up` set and
takes the same `Holders` roads. Its series rows go in `sorted`, whose
`(sort key, key)` set already fits. A memory lake is never kept, so nothing
moves. Lookup rows follow the set as 057 says.

### A number's exact lookup

`lookup_index.<type>` also takes number and decimal fields. A number field
only gets rows by that setting: `looked_up` admits a number kind only there,
never from a declaration, so no type pays that did not ask.

**One token.** The row in `text_index` holds the value's token: for a text,
the text; for a number, `as_number(value).normalize()` written out. `= 7`,
`= 7.00` and `= '7'` on a decimal field all reach `7`, the set a comparison
finds. A stored value that reads as no number has no row, and no number
equals it. `exact_terms` takes the field's kind, so `in` and a number
literal count on a number field; on a text field it reads as today, so a
text that reads as a number (`'7'`) stays a scan.

Every `Holders` statement stands as it is. 057's rule stands too: a reader
decides from the set, so the rows follow the set in the same transaction,
and a field's fill is never seen half done. The fill cannot be
`json_extract`, because the token is made in Rust: `relook` reads the type's
head states for a number field, holding the writer for that pass.

### The series index

**A new setting, `series_index.<type>`:** a list of pairs,
`[["symbol", "at"]]`, series field first. A derived one would charge every
type that names a field in both settings, and lakelands#1638 asks that a type
not asking pays nothing. The series field is text, reference, number or
decimal; the order field is any `sortable` kind; neither is computed. A
write is refused as invalid, naming the field. One text field only.

**The rows live in `sort_index`,** so there is no new table:

- `field` is `<order>\x1f<series>`; a field name cannot hold U+001F.
- `sort_key` is eight bytes of the SHA-256 of the token, then the `order_key`.
- A record with a series and no order value has the key's eight bytes and
  then `0xFF`, above every rank. Its row is what makes the index enough
  alone: a series' records with no value come last, from that one key, in
  key order, and a window never reaches them.

So a series read is the `All` statement with a derived field and prefixed
bounds, and adds no statement, no asc or desc index and no delete: the rows
go by `(ty, key)` like any order row. A hash collision adds rows to walk, and
the rulebook rejects them. On SQLite the window of 1 000 in a series took
1.0 ms against 16 ms by text and order index together.

**What it costs.** One row a record in three b-trees: +189 bytes a record on
SQLite in the scratch (keys about 18 bytes), and on a write what one order
field costs, 1–5 % on SQLite and 11 % on PostgreSQL. That write cost is
reckoned from 041 and is measured in the build, on both engines.

### How a kept lake gets it

Nothing happens at the upgrade. Writing the setting starts the build, in the
steps a lake's own order index takes (041): `Choose::Start`, `Fill` in chunks
of `limits.order_chunk`, `Keep`, with the pair's derived name as the field.
The fold keeps the rows from `Start`, so there is no catch-up; a reader takes
the pair only after `Keep`, and until then reads by the old road and gets the
same answer. The writer is held for a chunk at a time, not the type.
`choose_order`'s refusal for a type with a `sort_index` setting does not apply
to a pair. Taking the setting back is `Drop`: one delete, in the writer's turn.

041's marks and lakelands#591 stand:

- `put_declaration` deletes a type's order rows and writes or drops
  `sorted.<type>`. A pair's state goes with them: a setting put before the
  declaration, or a declaration put over a built pair, puts the pair back at
  `Start`, so no reader takes rows the declaration removed.
- `rebuild_sorted` and `sorted_fields` skip a `field` holding U+001F.
- A rebuild from the log fills the pair whole and writes `Keep`.

**An older writer.** A 0.22.x binary shares the format and the layout, so it
can fold this lake: it writes neither series rows nor number tokens, its
`rebuild_sorted` drops the first, and its `relook` drops the second. A reader
that trusts them would answer wrongly, not slowly. So a mark of this release,
`exact_through`, is kept as `chosen_through` is: in `meta`, with PostgreSQL's
twin row in `sort_index`, moved by a fold only from the position it folded
from. A reader takes series rows and number tokens only while it equals the
fold; else it takes the old road. A writer's open that finds it behind puts
each pair at `Start`, fills each number field in its own transaction, and
moves the mark only once those fills are done.

### What the settings cost

| act | cost |
|---|---|
| `series_index` written | chunks beside the writer; reckoned as 041's `chosen` build, 2.0 s on SQLite and 3.5 s on PostgreSQL at 100 000 records, plus a wait of up to one chunk per write |
| `series_index` removed | one delete, one row a record |
| a number named in `lookup_index` | one pass in the setting's transaction, holding the writer; reckoned as `sort_index`'s, 0.85 s on SQLite and 2.4 s on PostgreSQL at 100 000 records; rows as a text's |
| neither written | nothing, on any read or write |

## Why

The read that carries a time series is one series in one span. The order
index alone sees every series in that span, and the text index alone sees the
series' whole history. Only their pair bounds the walk by the answer, and
reusing `sort_index` gives the pair every road and rule that table already
has. The issues' numbers lean on a count, so the count comes first.

## What it is not

- **No answer changes.** The window, the keys and the series rows narrow;
  the rulebook still decides.
- **Not a query language**, and no field on a host's type.
- **Not an index on a field with none, nor `or`, more than one series field,
  or kept views** (lakelands#1634).

## The issues' done-when lines

**lakelands#1633**

| line | verdict |
|---|---|
| 600 000 held by all, about 1 000 by `rows_read` | stands; add `rows_walked` |
| the same at 6 000 (`Few`) and above `FEW_HOLDERS` | **changed.** 6 000 is `Many`: `FEW_HOLDERS` is 4 096. `Few` at 4 000: reads the window, walks 4 000. `Many` at 6 000 of 600 000: reads the window, walks the order rows of the whole type inside the bound. The series index is what lowers that |
| descending stops at `lo`, ascending starts at `lo` | stands |
| answers as today | stands; add `first`, `last` and an inexact sum at ties |
| earlier position answers as today | stands |

**lakelands#1635**: all stand, with `rows_read` as written; add
`rows_walked`, since it holds at `Few` and at 6 000 and more.

**lakelands#1638**

| line | verdict |
|---|---|
| 600 000 over 5 000 series, about 1 000 walked and read | stands |
| the same under and over `FEW_HOLDERS` | stands; this road does not ask `Holders` |
| `in` of 50: walked ≈ read + a seek a series | **changed.** It stands for an aggregate and a count, which read each window whole. A page in field order walks at most `series × batch` index rows and reads about the page |
| answers as today | stands |
| written builds, rebuild same, taken back drops | stands; "builds" means by chunks, after `Keep` |
| write cost measured and stated | stands; the build states it |

## Conformance rows

- `ROWS_WALKED_COUNTED`: a `Many`, a `Few` and a range read each add what they walked.
- `WINDOW_READS_ITS_ROWS`: aggregate, count, key page, field page, both directions, by `rows_read` and `rows_walked`.
- `WINDOW_HOLDS_WHAT_IS_ADMITTED`: the interval holds every admitted value.
- `WINDOW_TAKES_ONE_RANK`: a text field, or a literal of another rank, narrows nothing and answers as today.
- `WINDOW_SKIPS_NO_VALUE`: a short window does not walk the records without the field.
- `WINDOW_KEEPS_KEY_ORDER`: a key page, and `first`, `last` and an inexact sum at ties.
- `NUMBER_LOOKUP_READS_ITS_HOLDERS`: `=`, `equal`, `equal_any`, `in`.
- `NUMBER_LOOKUP_ONE_TOKEN`: `= 7`, `= 7.00`, `= '7'` find one set.
- `NUMBER_OUTSIDE_THE_SETTING`: reads as today.
- `SERIES_READS_ITS_WINDOW`: every road, a series under and over `FEW_HOLDERS`, by a text and by a number.
- `SERIES_NO_VALUE_LAST`: records with no value come last in a page, from the series alone.
- `SERIES_IN_MERGES`: `in` over 50 series.
- `SERIES_SETTING_BUILDS`: before `Keep` the old road, after it the new, one answer.
- `SERIES_REBUILD_SAME_ROWS`: a rebuild from the log gives the same rows.
- `SERIES_TAKEN_BACK`: the rows are gone, and the answer stands.
- `SERIES_DECLARED_OVER`: a setting before a declaration, and a declaration over a built pair (lakelands#591).
- `EXACT_MARK_BEHIND`: a fold that leaves `exact_through` behind sends readers to the old road. Engine tests hold the older writer on SQLite and PostgreSQL.

## The estimate

Lines as reckoned, then ×1.35. Every pull request stays about 600 or under.

| step | issue | product | spec | ×1.35 |
|---|---|---|---|---|
| 1. `rows_walked`, the companion count | all | 60 | 100 | 81 / 135 |
| 2. `window`, field-order roads, no `missing` | 1633 | 150 | 200 | 203 / 270 |
| 3. `window_keys`, aggregate, count, key page | 1633 | 190 | 220 | 257 / 297 |
| 4. memory lookup, 057 amended in tree | 1635 | 120 | 150 | 162 / 203 |
| 5. number token, `lookup_index` widened | 1635 | 120 | 150 | 162 / 203 |
| 6. `series_index`, rows in the fold | 1638 | 150 | 180 | 203 / 243 |
| 7. the build, `exact_through`, older writer | 1638 | 150 | 200 | 203 / 270 |
| 8. the series road, one value | 1638 | 130 | 250 | 176 / 338 |
| 9. `in`, the merge, the matrix row | 1638 | 80 | 150 | 108 / 203 |
| **sum** | | **1 150** | **1 600** | **1 555 / 2 162** |

Steps 1 to 3 stand alone. Steps 4 and 5 need step 1, 6 to 9 need step 2, and
step 7 needs step 5's fill.

The sums are of the epic's size and within the granted reserve. They are
reckoned against that estimate, not against shipped sizes of each step. Step 7
carries the most risk, since a mark with an older writer has cost rounds before.

## Open for the operator

1. **A host's `series_index` builds by chunks, not in the setting's write.**
   `sort_index` holds the writer for a full pass. By chunks, ingest keeps
   running and the read stays at its old price until `Keep`; a host cannot
   tell from the write that it is ready, only from `describe`. Held, a large
   lake stops writing for the pass: at SQLite's 8.5 µs a row, one hundred
   million ticks hold it for about 14 minutes. **Recommended: chunks.**
2. **`exact_through` against a layout step.** The mark costs step 7's lines
   and leaves a kept lake unrebuilt. Raising `LAYOUT` costs no lines and needs
   no mark, but rebuilds every kept lake at its first 0.23.0 open, and an older
   writer's open then rebuilds it back. **Recommended: the mark.**

## How it came to be

- Written for lakelands#1633, lakelands#1635 and lakelands#1638, from a read
  of the engines and a scratch run on SQLite at 600 000 records over 5 000
  series, one holding a tenth, with records of about 330 bytes. PostgreSQL was
  not measured here: its figures are reckoned from 041 and measured in the build.
