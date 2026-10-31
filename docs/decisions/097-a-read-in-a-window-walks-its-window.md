# 097 — A read in a window walks its window

Decided on lakelands#1633, lakelands#1635 and lakelands#1638, for 0.23.0.
No code yet. The build waits for the joint record of lakelands#1649,
lakelands#1651 and lakelands#1652, which may change the format this record
stands on; what waits for it is listed below.

**No data migration and no breaking change of its own, and no compatibility
is kept.** The operator decided a green field: no older lake and no older writer
meets 0.23.0, so nothing here guards against one
(https://github.com/wildstuff/lakelands/issues/1652#issuecomment-6547704529).
This record moves neither the log format nor a stored table, and `LAYOUT` does
not rise for it. Additive: one setting, one wider setting, one `limits` member,
three `ProjectionView` methods with defaults and a `Window` type, one `describe`
member. `Scan` is unchanged.

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

- **Only where one rank holds the comparison.** `compare` is byte-wise
  between two texts of different ranks (`'5' > '10x'`), so a text or reference
  field takes no window. A date-time or date field holds rank 1 only, and a
  literal narrows it only if it reads as an instant and not as a number.
- **A number or decimal field can hold more than rank 0.** A JSON number no
  `Decimal` reads is rank 4, and no comparison admits it. A decimal can be rank
  2: `is_decimal` has no length limit, so 40 digits pass `check_kind`, read as
  no number and compare with a text by their bytes. `x <= '5'` is true of such
  a value today. So a range term narrows only under a number literal, which
  compares with no unread value at all. A text literal narrows only `=`, and
  only if it reads as a number: a value it equals cannot be of rank 2. Any
  other literal narrows nothing, and that read stays as it is.
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
- **One field.** A filter may bound several. The read takes the first of the
  type's `sorted`, then its `chosen`, that the filter bounds.

### The read road

At the head only. An earlier position takes `scan_in_field_order_at` as
today; a narrowing there is welcome and not built.

1. **A key of one text field** is read by key, as today.
2. **A series index holds the pair:** the seek below, on this index alone.
   `Holders` is not asked.
3. **A page in field order, under an exact text:** `Few` joins holders to
   order rows and bounds them, walking `M`. `Many` walks the order index inside
   the window and probes the text, until the page fills.
4. **A page in field order, no exact text:** the order index inside the window.
5. **An aggregate, a count and a page in key order** walk the smaller side,
   below.

A filter the store keeps whole (`store_only`) pushes nothing today. It stays
so: a window with a store-only term reads as it does now.

**`in`.** A series or number field under `in` reads each value. A page in field
order seeks every series from the same position and `engines` merges them by
(sort key, key). The first batch of a series is the page shared among the
series, plus one; a series refills only when its batch runs out, each refill
twice the last. In the scratch, 50 series and a page of 1 001 walked 1 092 rows
in 51 statements; `series × page` would have been 50 050. An aggregate and a
count read each window whole.

### The smaller side

An aggregate, a count and a key page today walk the holders or the type,
whatever the window. A window's road costs a seek per row inside it, whatever
the holders. Neither road always wins, so the read chooses. Under `Few`, at
most 4 096 holders, it needs no choice: it takes the holders with their probe,
below.

SQLite scratch, warm, 600 000 records, milliseconds, ratios only. The last
column is the window's road taken always, which this record's first version
did.

| read | today | window 1 % | window 25 % | window 100 % | window's road at 100 % |
|---|---|---|---|---|---|
| text held by 6 000 | 17 | 10 | 15 | 19 | 477 |
| text held by all | 948 | 37 | 752 | 1 002 | 1 956 |
| no text | 305 | 21 | 395 | 375 | 1 146 |

1. **Read the window's first batch.** `ProjectionView::window_keys(ty, window,
   scan, after, n)` walks at most `n` index rows, 1 024 here, and answers the
   keys among them that hold the exact text, with where it stopped. A short walk
   was the whole window, so nothing is counted. `None`, the default, is today's
   road.
2. **Count in rounds.** `window_sides(ty, window, scan, l)` counts the window to
   `l`, from 4 096 and four times more each round, and the other side to four
   times what the window counted. A round ends the counting once either falls
   short. Both are index-only. The other side is the holders under an exact
   text, or the type's order rows standing for the type.
3. **Choose.** Window × 4 ≤ the other side takes the window's road; anything
   else takes the other side. The ratio is the scratch's crossover: a window of
   a quarter costs about what the type or its holders cost. Step 4 of the build
   measures it on PostgreSQL and moves the constant if the crossover moves.

**The window's road.** A count reads each batch by `get_many`: `count_in_key_order`
only adds up `admits`, so it needs no key order, no sort and no ceiling. An
aggregate does fold in key order. `first` and `last` keep the least key among
equals, and a sum that is not exact is what adding in order gave, so it gathers
the keys, sorts them and reads a `COUNT_PAGE` at a time. A key page does the
same for a window of up to `FEW_HOLDERS` rows, within `page`.

**Past the bounds.** An aggregate counts the window only to `limits.window_keys`
(1 048 576) and a key page to `FEW_HOLDERS`, since sorting a large window for
each page is a square. Past those, each takes the other side, having lost only
the counts. Under an exact text that is the probe; with no exact text it is the
type, as today.

**The other side, under an exact text.** `holders_inside(ty, window, scan, after,
n)` answers a batch of holders in key order and those whose order row lies
inside the window, and only those are read. A batch with more than a third of
its holders inside ends the probing, and the read goes on by `page` from that
key: where most holders are inside, a probe costs more than it saves (1.4 s
against 0.8 s for all of them).

**Who counts and who admits.** `get_many` reads by key, adds nothing to
`rows_read` and applies nothing of `Scan`. So every chunk the store reads on
these roads passes `engines::admitted(decl, records, scan)`, in `engines.rs`
where the count lives. It adds each record read to `rows_read` and keeps those
`admits` keeps, which holds `retired`, `restricted`, `valid_at`, `equal`,
`equal_any` and the pushed filter. `window_keys`, `window_sides` and
`holders_inside` hand on no record: they add every index row they pass to
`rows_walked`, the counts' included. So a read walks at most about eight times
its smaller side, plus a batch.

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
the text; for a number, `as_number(value).normalize()` written out, so `-0` and
`0` are one. `= 7`, `= 7.00` and `= '7'` on a decimal field all reach `7`, the
set a comparison finds.

**A value that reads as no number has no row.** A literal, or an `equal` or
`equal_any` value, that reads as no number asks the index nothing and that read
takes the old road. So `x = '<40 digits>'` finds the 40-digit value as it does
today, and one such value under `in` sends the whole `in` there.

**Three places take the field's kind.** `exact_terms`, so `in` and a number
literal count on a number field; on a text field it reads as today, so a text
that reads as a number (`'7'`) stays a scan. And `text_lookup` and
`text_lookups`, which read `as_str()` today: an `equal` on a number field would
ask nothing, and one on a decimal field would ask its spelling, not its token.

Every `Holders` statement stands as it is. 057's rule stands too: a reader
decides from the set, so a field's fill is never seen half done. The fill cannot
be `json_extract`, because the token is made in Rust: `relook` reads the type's
head states for a number field, in the setting's transaction. That pass holds
the writer and stands only until the joint record: lakelands#1651 asks
`lookup_index` in bounded parts, reads on the old road until a build is whole,
and a number field goes with it.

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
- A series value that reads as no number has no row, and a read of it takes
  the old road, as under the number lookup.

So a series read is the `All` statement with a derived field and prefixed
bounds, and adds no asc or desc index and no delete: the rows go by `(ty, key)`
like any order row. An ascending walk meets the `0xFF` rows last. A descending
one would meet them first, so with no window its upper bound stops under `0xFF`
and they are read after it, by one more statement over that one key. A hash
collision adds rows to walk, and the rulebook rejects them. On SQLite the window of 1 000 in a series took
1.0 ms against 16 ms by text and order index together.

**What it costs.** One row a record in three b-trees: +189 bytes a record on
SQLite in the scratch (keys about 18 bytes), and on a write what one order
field costs, 1–5 % on SQLite and 11 % on PostgreSQL. That write cost is
reckoned from 041 and is measured in the build, on both engines.

### How a lake with records gets it

Writing the setting starts the build, in the steps a lake's own order index
takes (041): `Choose::Start`, `Fill` in chunks of `limits.order_chunk`, `Keep`,
with the pair's derived name as the field. The fold keeps the rows from `Start`,
so there is no catch-up. A reader takes the pair only after `Keep`, and until
then reads by the old road and gets the same answer. The writer is held for a
chunk at a time, not the type. Taking the setting back is `Drop`, one delete in
the writer's turn, at any step, and it ends a build under way.

**A pair is the host's, so it is not in `Chosen`.** The chooser's rules are for
what the lake picked for itself. A pair has its own row, `series.<type>` in
`meta`, naming each pair as `building` or `kept`; a name holding U+001F goes
there in `choose_order`. Therefore:

- `sweep` never sees a pair, so neither its unused window nor the switch off
  (`choose_order_index` false) takes one while its setting stands.
- `eligible` does not count it against `order_max_fields`, and `choose_order`'s
  refusal for a type with a `sort_index` setting does not apply.
- `rebuild_sorted` deletes only rows of a `field` without U+001F and resets
  `Chosen` alone, so a `sort_index` setting written over a built pair leaves it.
  `sorted_fields` skips those rows too.

**What drives the build.** Not a read: `choose_orders` starts from one, and a
host's index must not wait for a `find`. `Lake::build_series` is a task of the
writer's, started by the commit that puts a `series_index` setting or moves a
pair to `Start`, and at each open or `regain` that makes a writer. It finds each
pair in `building` and fills it from the type's first key: a `Fill` is
idempotent, so a restart mid-build repeats chunks and loses nothing. A failed
step leaves the pair `building` for the next start, and readers stay on the old
road.

041's marks and lakelands#591 stand:

- `put_declaration` deletes a type's order rows and writes or drops
  `sorted.<type>`. A pair's state goes with them: a setting put before the
  declaration, or a declaration put over a built pair, puts the pair back at
  `Start`, so no reader takes rows the declaration removed. A task filling that
  pair starts it over.
- A rebuild from the log fills the pair whole and writes `kept`.

**`describe` tells it.** The write says nothing of when an index is ready, so
`TypeDescription.series` does: a list of `{series, order, state}`, `state` being
`building` or `kept`. A pair is told to a holder of `schema` who reads both its
fields, at the head only, as `chosen` is. A taken-back pair leaves the list.

### What the settings cost

| act | cost |
|---|---|
| `series_index` written | chunks beside the writer; reckoned as 041's `chosen` build, 2.0 s on SQLite and 3.5 s on PostgreSQL at 100 000 records, plus a wait of up to one chunk per write |
| `series_index` removed | one delete, one row a record |
| a number named in `lookup_index` | until the joint record, one pass in the setting's transaction, holding the writer; reckoned as `sort_index`'s, 0.85 s on SQLite and 2.4 s on PostgreSQL at 100 000 records; rows as a text's |
| neither written | nothing, on any read or write |

### The issues' done-when lines

**lakelands#1633**

| line | verdict |
|---|---|
| "every road" (Asked) | **changed.** A key page over a window of more than `FEW_HOLDERS` rows, and an aggregate over more than `limits.window_keys`, take the other side: the holders with their probe, or the type as today. A key page would sort the window per page, and an aggregate would hold the keys. Field-order pages have no such limit |
| 600 000 held by all, about 1 000 by `rows_read` | stands; add `rows_walked` |
| the same at 6 000 (`Few`) and above `FEW_HOLDERS` | **changed.** 6 000 is `Many`: `FEW_HOLDERS` is 4 096. `Few` at 4 000: reads the window, walks 4 000. `Many` at 6 000 of 600 000: an aggregate or a count reads the window and walks the smaller side; a field-order page reads the window and walks the type's order rows inside the bound. The series index is what lowers that |
| **new:** a window wide against its holders or type | walks the smaller side, reads no more than today and takes at most 1.3 × today's time on SQLite and PostgreSQL, measured in step 4 |
| descending stops at `lo`, ascending starts at `lo` | stands |
| answers as today | stands; add `first`, `last` and an inexact sum at ties, and long decimals |
| earlier position answers as today | stands |

**lakelands#1635**: all stand, with `rows_read` as written; add
`rows_walked`, since it holds at `Few` and at 6 000 and more.

**lakelands#1638**

| line | verdict |
|---|---|
| 600 000 over 5 000 series, about 1 000 walked and read | stands |
| the same under and over `FEW_HOLDERS` | stands; this road does not ask `Holders` |
| `in` of 50: walked ≈ read + a seek a series | stands, for a page and for a read of whole windows |
| answers as today | stands |
| written builds, rebuild same, taken back drops | stands; "builds" means by chunks, after `Keep`, told in `describe` |
| write cost measured and stated | stands; the build states it |

### Conformance rows

- `ROWS_WALKED_COUNTED`: a `Many`, a `Few` and a range read each add what they walked.
- `WINDOW_READS_ITS_ROWS`: aggregate, count, key page, field page, both directions, by `rows_read` and `rows_walked`.
- `WINDOW_WALKS_THE_SMALLER_SIDE`: a window narrow and wide against its holders and its type takes the road the rule names, reads no more than today, and walks at most about eight times the smaller side.
- `WINDOW_ADMITS_LIKE_A_PAGE`: a retired and a restricted record, `valid_at`, `equal`, `equal_any` and a pushed filter inside a window, on the road by keys and the road by holders, by `rows_read`.
- `WINDOW_PAST_ITS_BOUNDS`: a count past `limits.window_keys` reads the window; an aggregate past it and a key page past `FEW_HOLDERS` take the other side, one answer.
- `WINDOW_HOLDS_WHAT_IS_ADMITTED`: the interval holds every admitted value, long decimals among them.
- `WINDOW_TAKES_ONE_RANK`: a text field, a literal of another rank, or a text literal in a range on a decimal field narrows nothing and answers as today.
- `WINDOW_SKIPS_NO_VALUE`: a short window does not walk the records without the field.
- `WINDOW_KEEPS_KEY_ORDER`: a key page, and `first`, `last` and an inexact sum at ties.
- `NUMBER_LOOKUP_READS_ITS_HOLDERS`: `=`, `equal`, `equal_any`, `in`.
- `NUMBER_LOOKUP_ONE_TOKEN`: `= 7`, `= 7.00`, `= '7'` find one set, `-0` and `0` are one, and a long decimal is found by its own text on the old road.
- `NUMBER_OUTSIDE_THE_SETTING`: reads as today.
- `SERIES_READS_ITS_WINDOW`: every road, a series under and over `FEW_HOLDERS`, by a text and by a number.
- `SERIES_NO_VALUE_LAST`: records with no value come last in a page, ascending and descending, from the series alone.
- `SERIES_IN_MERGES`: `in` over 50 series walks about the rows read plus a seek a series.
- `SERIES_SETTING_BUILDS`: before `Keep` the old road, after it the new, one answer.
- `SERIES_BUILD_RESUMES`: a restart mid-build finishes with no read and no further write.
- `SERIES_KEPT_BY_THE_HOST`: the switch off, `order_max_fields` reached, a `sweep` and a `sort_index` setting over a built pair leave it kept and read.
- `SERIES_TOLD`: `describe` says `building` then `kept`, to a holder of `schema` who reads both fields and to no one else.
- `SERIES_REBUILD_SAME_ROWS`: a rebuild from the log gives the same rows.
- `SERIES_TAKEN_BACK`: during a build and after it, the rows are gone, and the answer stands.
- `SERIES_DECLARED_OVER`: a setting before a declaration, and a declaration over a built pair (lakelands#591).

### The estimate

Lines as reckoned, then ×1.35. Every pull request stays about 600 or under.

| step | issue | product | spec | ×1.35 |
|---|---|---|---|---|
| 1. `rows_walked`, the companion count | all | 60 | 100 | 81 / 135 |
| 2. `window`, one rank, field-order roads, no `missing` | 1633 | 150 | 220 | 203 / 297 |
| 3. `window_keys`, `admitted`, aggregate, count, key page | 1633 | 170 | 200 | 230 / 270 |
| 4. the smaller side: `window_sides`, `holders_inside` | 1633 | 110 | 140 | 149 / 189 |
| 5. memory lookup, 057 amended in tree | 1635 | 120 | 150 | 162 / 203 |
| 6. number token, `lookup_index` widened | 1635 | 140 | 170 | 189 / 230 |
| 7. `series_index`, rows in the fold, `series.<type>`, the rebuild's exemptions | 1638 | 170 | 200 | 230 / 270 |
| 8. the build, its driver, `describe` | 1638 | 170 | 220 | 230 / 297 |
| 9. the series road, one value, the tail | 1638 | 130 | 250 | 176 / 338 |
| 10. `in`, the merge, the matrix row | 1638 | 100 | 170 | 135 / 230 |
| **sum** | | **1 320** | **1 820** | **1 785 / 2 459** |

Steps 1 to 4 stand alone, in order. Steps 5 and 6 need step 1, and steps 7 to 10
need step 2. Step 8 changes what `describe` says, so its pull request walks.

The reserve for lines 2, 3, 4, 6 and 7 of lakelands#1578 is 3 500 product and
6 000 specification lines, and the sum fits it. Shipped pull requests of this
round ran about twice their reckoning, though. At ×2 this line alone takes
2 640 / 3 640, three quarters of the product reserve, so the first three steps'
measured sizes should reset this table before the rest is cut.

### What the operator decided

Both on 2026-10-31.

1. **A host's `series_index` builds by chunks, beside the writer, not in the
   setting's write.** `sort_index` holds the writer for a full pass: at
   SQLite's 8.5 µs a row, one hundred million ticks would hold it for about
   14 minutes. By chunks, ingest keeps running and the read stays at its old
   price until `Keep`
   (https://github.com/wildstuff/lakelands/issues/1633#issuecomment-6547537700).
2. **No compatibility is kept.** 0.23.0 starts a new format with no past, and an
   older binary already refuses a format it does not read. So this record keeps
   no mark and no guard for an older lake or an older writer
   (https://github.com/wildstuff/lakelands/issues/1652#issuecomment-6547704529).

### What waits for the joint record

The joint record of lakelands#1649, lakelands#1651 and lakelands#1652 decides
these. Each stands here until it does, and it says what it changes:

- `sort_index` and `text_index` as the rows' home. A changed format moves the
  rows' shape, not the roads.
- A number field's fill, a single pass today, into the bounded parts of
  lakelands#1651. The same goes for the series build, if one road for parts
  carries it, and for `choose_order`'s refusal.

### Open for the operator

Nothing.

## Why

The read that carries a time series is one series in one span. The order
index alone sees every series in that span, and the text index alone sees the
series' whole history. Only their pair bounds the walk by the answer, and
reusing `sort_index` gives the pair every road and rule that table already
has. The issues' numbers lean on a count, so the count comes first.

## What it is not

- **No answer changes.** The window, the keys and the series rows narrow;
  the rulebook still decides, on every road.
- **Not a query language**, and no field on a host's type.
- **Not an index on a field with none, nor `or`, more than one series field,
  or kept views** (lakelands#1634).

## How it came to be

- Written for lakelands#1633, lakelands#1635 and lakelands#1638, from a read
  of the engines and a scratch run on SQLite at 600 000 records over 5 000
  series, one holding a tenth, with records of about 330 bytes. PostgreSQL was
  not measured here: its figures are reckoned from 041 and measured in the build.
- The operator settled the build by chunks, and a mark for older writers
  (2026-10-31).
- A review measured the first version slower than today on wide windows and
  wrong on long decimals. So a read walks the smaller side, reads a text literal
  on a decimal as it did, and counts on every road. A pair took its own state
  and driver, and `describe` tells it. The kept-lake premise went with
  lakelands#1652, and the record now says what waits for the joint record. The
  scratch's roads were run again at 600 000 records.
- The operator decided a green field, so no compatibility is kept and that mark
  went (2026-10-31).
