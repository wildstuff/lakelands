# 097 — A read in a window walks its window

Decided on lakelands#1633, lakelands#1635 and lakelands#1638, for 0.23.0.
No code yet. The build waits for the joint record of lakelands#1649,
lakelands#1651 and lakelands#1652, proposed in
`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md` and
`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`, and for the new
format's pull requests, which land first; what waits for it is listed below.

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
- **Crossed ends.** Terms whose cut keys put the start above the end
  (`x >= 10 and x <= 5`, or `x = 5 and x = 7`) admit no value. They narrow
  nothing: the rulebook answers no row, and a range with its start above
  its end is no window.
- **A day bound** is the midnight `Timestamp::denoted` reads, as `compare`
  reads it.
- **The rulebook still admits each row.** The window only narrows.
  `WINDOW_HOLDS_WHAT_IS_ADMITTED` holds that the interval contains every
  value a comparison admits, on a generated set.
- **A window skips the `missing` walk.** A comparison with no value is
  false, so a read bounded on its first order field has no records without
  the field to add. Today that tail would walk the type after a short
  window, on all three engines and every `Holders`.
- **One field.** A filter may bound several. The read takes the page's first
  order field, when the filter bounds it; a bound on another field narrows
  nothing.

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
   The window's side is the rows its road walks, every index row in the window
   whether it holds the text or not, since the walk pays for each. Counting
   the text's holders inside instead prices the road by its answer, and takes
   it where it walks the whole type: measured on SQLite at 600 000 records, a
   window of a quarter of the type under a text held by one record in a
   hundred read in 167 ms against today's 20 ms.
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
- `WINDOW_READS_ITS_ROWS`: aggregate, count, key page, field page, both directions, by `rows_read` and `rows_walked`. As built in steps 1 and 2: the field page, both directions, its walk unchanged when records outside the bound are added, and a windowed page under an exact text by the `Few` and `Many` roads. As built in step 3: the aggregate, the count and the key page read the window's keys, each answering what today's road answers and reading only the window's rows.
- `WINDOW_WALKS_THE_SMALLER_SIDE`: a window narrow and wide against its holders and its type takes the road the rule names, reads no more than today, and walks at most about eight times the smaller side. A window of 31 rows in a type of 5 200 takes its own road and reads 31 rows where today's count reads all 5 200; a walk of 31 rows is the whole window and takes the window's road without counting; a window of 1 031 rows under a text four hold takes the holders' side, four times 1 031 not fitting inside the four.
- `WINDOW_ADMITS_LIKE_A_PAGE`: a retired and a restricted record, `valid_at`, `equal`, `equal_any` and a pushed filter inside a window, on the road by keys and the road by holders, by `rows_read`. `admitted` keeps exactly what a page keeps, so the window's road answers the page's records.
- `WINDOW_AGGREGATE_JUDGES_A_LINKS_ENDS`: an aggregate over a link type's window whose ends its reader does not hold, one link among them to a withheld term, answers the count and the sum a walk of every page answers, not the engine's count of the window's keys; a count takes the page's road already.
- `WINDOW_PAST_ITS_BOUNDS`: a count past `limits.window_keys` reads the window; an aggregate past it and a key page past `FEW_HOLDERS` take the other side, one answer. The same answers under `limits.window_keys` of four and under a ceiling wide enough for every window.
- `WINDOW_HOLDS_WHAT_IS_ADMITTED`: the interval holds every admitted value, long decimals among them.
- `WINDOW_TAKES_ONE_RANK`: a text field, a literal of another rank, or a text literal in a range on a decimal field narrows nothing and answers as today.
- `WINDOW_SKIPS_NO_VALUE`: a short window does not walk the records without the field.
- `WINDOW_KEEPS_KEY_ORDER`: a key page, and `first`, `last` and an inexact sum at ties.
- `NUMBER_LOOKUP_READS_ITS_HOLDERS`: `=`, `equal`, `equal_any`, `in`. As built in steps 5 and 6: 60 000 records, 600 of them holding the asked number, read by about 600 rows on every engine; a put moving one holder off it and a drop of another take both out of the set and the put into the other's.
- `NUMBER_LOOKUP_ONE_TOKEN`: `= 7`, `= 7.00`, `= '7'` find one set, `-0` and `0` are one, and a value that reads as no number keeps no row and is found by its own text on the old road. As built: a decimal field, a 40-digit text held in it and found by `=`, and an `in` with such a value taking the old road.
- `NUMBER_OUTSIDE_THE_SETTING`: reads as today. As built: before the setting, during it, and after it is retired, the same answers.
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
| 1.+2. `rows_walked`, `window`, one rank, field-order roads, no `missing` (measured) | all, 1633 | 607 | 881 | measured |
| 3.+4. `window_keys`, `admitted`, aggregate, count, key page, the smaller side (measured) | 1633 | 1315 | 899 | measured |
| 5.+6. memory lookup, number token, `lookup_index` widened, 057 amended in tree (measured) | 1635 | 648 | 289 | measured |
| 7. `series_index`, rows in the fold, `series.<type>`, the rebuild's exemptions | 1638 | 170 | 200 | 230 / 270 |
| 8. the build, its driver, `describe` | 1638 | 170 | 220 | 230 / 297 |
| 9. the series road, one value, the tail | 1638 | 130 | 250 | 176 / 338 |
| 10. `in`, the merge, the matrix row | 1638 | 100 | 170 | 135 / 230 |
| **sum** | | **3 341** | **3 204** | steps 1 to 6 measured, the rest ×1.35 |

Steps 1 to 4 stand alone, in order. Steps 5 and 6 need step 1, and steps 7 to 10
need step 2. Step 8 changes what `describe` says, so its pull request walks.
Every step lands after 098's last, and step 8 after 099's R7.

The reserve for lines 2, 3, 4, 6 and 7 of lakelands#1578 is 3 500 product and
6 000 specification lines, and the sum fits it. Shipped pull requests of this
round ran about twice their reckoning, though. Steps 1 to 6 now measure
2 570 / 2 069, and the steps after them stay as reckoned until they are
measured.

### As built: steps 1 and 2 (lakelands#1693)

The two steps landed in one pull request.

- **`rows_walked`.** `engines::rows_walked()` stands beside `rows_read`,
  with `counting_walks()` and `count_walks(on)` the switch a check turns on
  and `count_walk(n)` the one call that adds. `Holders::Few` carries the
  count `Holders::of` already read, and each statement that walks the
  holders adds `M`. `Holders::Many` adds nothing on its own: while the
  switch is on, SQLite and PostgreSQL run a `count(*)` of the same range,
  from the seek to the last row handed, or to the range's end on a short
  batch. The memory engine adds the rows it hands, on the order index and
  on the `missing` walk; a pure range statement adds what it hands on every
  engine, a page in key order among them. `rows_read` is unchanged. A
  memory walk's upper bound is the next byte string after `hi`, so a row
  whose key only begins with `hi` can be walked and rejected by the
  rulebook; the answer is unaffected and the SQL engines' walk can be
  smaller.
- **`window`.** `engines::window(filter, field, kind)` answers
  `Window { lo, hi }`, cut by `order_key` and with the ends inclusive; an
  open side ends at the rank's edge. A number or decimal field takes rank 0,
  a date or date-time field rank 1, and no other kind takes a window; a
  literal of another rank narrows nothing. `engines::field_kind` reads the
  field's kind from the declaration. The field-order page at the head,
  under its first order field when the filter bounds that field, reads the
  order index inside the window (memory's sorted set, the SQL engines'
  `sort_key`), under an exact text through the `Few` and `Many` roads, and
  skips the `missing` walk for a windowed page. A crossed bound narrows
  nothing, and a cursor above the window going up or below it going down
  reads nothing, as a seek past the window does. An earlier position, a
  filter the store keeps whole and every other road read as today.
- **Measured.** Product in 711, out 104, net 607; specification in 949,
  out 68, net 881. The core-size gate: product 100 409 of base 108 500,
  specification 184 983 of base 191 500, neither base nor ceiling raised.
  Steps 1 and 2 measured 607 / 881 against the 284 / 432 reckoned for them
  at ×1.35, about twice, as the round's shipped pull requests have run; the
  estimate table above carries the measured size and leaves the remaining
  steps as reckoned. The batch-growth page in the engines' text-lookup
  tests puts its bound under a function, since a bound on the page's order
  field now narrows its window and would ask fewer batches. A review then
  held the `Many` road to the window's rows, a bound on a field beside the
  one the page orders by to nothing, a literal on the left, records below
  the bound and a filter read downwards.

### As built: steps 3 and 4 (lakelands#1705)

The two steps landed in one pull request.

- **`window_keys`.** `ProjectionView::window_keys(ty, window, scan, after,
  n)`, default `None`, answers `WindowBatch { keys, after, done }`: a walk
  of at most `n` rows of the order index inside the window, from `after`
  (the sort key and record key of the last row passed), the keys among the
  rows passed that hold the scan's exact text, and `done` where the walk
  was short. The batch is the walk, not the admitted set: where it stopped
  is the last row walked whether it holds the text or not, so `done` and
  `after` do not move with the text's density. SQLite and PostgreSQL walk
  the `sort_index` range in `(sort_key, key)` order in a subquery and
  apply the text to the rows walked; memory walks its sorted set. Every
  row walked is added to `rows_walked`.
- **`admitted`.** `engines::admitted(decl, records, scan)` holds
  `get_many`'s records to the scan's flags, `valid_at`, exact pairs and
  filter, as a page does, and adds each record read to `rows_read`. A
  window's road reads its keys by `get_many` and passes them through
  `admitted`, so it answers the page's records, not the walk's.
- **The store's roads.** `Reader::window_choice(decl, scan, ceiling)`
  takes the first window its engine keeps an index for. A short first
  batch of `WINDOW_BATCH` is the whole window and takes the window's road
  when its keys fit the ceiling; otherwise `engines::takes_window` counts
  in rounds from `WINDOW_ROUND`, four times more each round. The count
  (`window_count`), the aggregate (`window_aggregate`, which gathers the
  keys, sorts them and folds a `COUNT_PAGE` at a time in key order) and
  the key page (`window_page`) read the window's keys. The aggregate's
  ceiling is `limits.window_keys` and the key page's `FEW_HOLDERS`; past a
  ceiling the count reads the window whole, a count having none, and the
  aggregate and the key page take the other side. `find`'s aggregate takes
  the road only where the type is kept whole, is no view's, and no link's
  ends are judged for its reader — the gate `count` already had, since a
  key read holds no end.
- **The smaller side.** `window_sides(ty, window, scan, l)` counts the
  window's order rows to `l + 1`, `whole` where the window held no more,
  and the other side — the exact text's holders, or the type's order rows
  — to four times the window's count; both counts are index-only and add
  to `rows_walked`. `holders_inside` answers the text's holders in key
  order and whether each order row lies inside the window; a batch more
  than a third inside ends the probe, and the page reads on by
  `page_plain` from the key before it.
- **Two defects the measurement found.** `window_sides` first counted the
  text's holders inside the window with `EXISTS`, which prices the
  window's road by its answer: a window of a quarter of the type under a
  text one record in a hundred holds took the window's road and read in
  167 ms where today's count read in 20 ms. It now counts the window's
  order rows, every index row inside whether it holds the text or not,
  which is what the walk pays for. `window_keys` first put the batch's
  limit on the admitted keys rather than on the index rows walked, so the
  first batch of a text every record holds walked 60 000 index rows to
  answer 1 024 keys and reported the window short only at the text's end,
  and every batch after the first skipped the rows between the keys it
  answered. It now walks `n` index rows and applies the text to those.
- **PostgreSQL's JIT, turned off for the read.** The window's statements
  walk and count index rows, and the planner's estimate for them
  overshoots `jit_above_cost` by a wide margin: a first batch of 1 024
  rows under a text six records in a thousand hold was estimated at four
  million, and took 177 ms compiled and 10 ms not, at 600 000 records.
  `Reader::jit_off` sets `LOCAL jit = off` for the read that asks a
  window, as the vector index's statement already sets it for its own.
- **PostgreSQL's collation, which hid a 3 400-fold plan.** The walk's
  text probe compared `t.key = w.key` under the database's default
  collation, and `text_index_value` holds its last column
  `key COLLATE "C"`: the comparison could not be an index condition, so
  the probe read every holder of the text for every row walked. A first
  batch of 1 024 rows under a text all 600 000 records hold took 15.9 s,
  a bitmap of 600 000 index rows 1 024 times; compared
  `t.key COLLATE "C" = w.key` it takes 4.6 ms, one seek a row. Several
  texts had the same shape twice over: `t.value = ANY(...)` is no index
  condition either, and is sought as
  `md5(t.value) IN (SELECT md5(x) FROM unnest($::text[]) x)`, the form
  the index holds, 14 ms a batch of 1 024 under two texts.
- **A check that skipped step 1.** `WINDOW_WALKS_THE_SMALLER_SIDE` asked
  `window_sides` alone, so the 31-row window under the four-holder text —
  a short first batch, and so the whole window, taken without counting —
  was answered by the sides as the other side on SQLite and PostgreSQL,
  where the text's four holders count as the other side. The check now
  asks the window's first batch first and counts only where the walk was
  long, as the store does.
- **What the review found.** Three gaps, each now covered by a check.
  The aggregate took the window's road over a link type and judged no
  end: `window_aggregate` reads the window's keys through `admitted`,
  `at_read` and `live_by`, and a key read holds no end, so a link to a
  withheld or restricted end was counted where a page refuses it. The
  gate `count` already had now holds the aggregate too, and
  `WINDOW_AGGREGATE_JUDGES_A_LINKS_ENDS` holds an aggregate over a link
  type's window to a walk of every page, in count and in sum. PostgreSQL's
  sides' count and probe still sought several texts with
  `value = ANY(...)`, no index condition, where `window_keys` sought the
  `md5` form; both now seek the same form. Memory's `window_keys` added
  none of its rows to `rows_walked`; it now adds each row walked, and the
  window checks hold the walk to the window's rows on every engine, an
  aggregate and a key page included.
- **Measured.** Product in 1 337, out 22, net 1 315; specification in
  902, out 3, net 899. The core-size gate, with steps 5 and 6 and
  lakelands#1701, lakelands#1713 and lakelands#1717 merged: product 107 021 of base
  115 000, specification 191 854 of base 202 500. The base and ceiling
  rose with lakelands#1710's reserve for the round; these two steps
  raise neither. The two steps measured 1 315 / 899 against the 379 / 459
  reckoned for them at ×1.35 — three and a half times the product
  reckoning and about twice the specification's; the estimate table
  above carries the measured size.

#### Measured: the smaller side

A script outside the tree (target directory of its own), the table of
600 000 records and three texts — one six records in a thousand hold,
one every record holds, and no text — read at a window of 1, 10, 25, 50
and 100 % of the type. Each read is the median of three runs after a
warm one; `today` is the read as it stood before this decision (a `scan`
paged at 1 024, or `count` over it), `chosen` the read as this record
builds it — the choice, then the road the rule names — and `forced` the
window's road taken whatever the rule says, which is what the crossover
is read from. Times in ms; the count and the aggregate side by side.

SQLite:

| window | today count / agg | chosen count / agg | window's road |
|---|---|---|---|
| held by 6 000, 1 % | 20.6 / 20.5 | 23.6 / 20.6 | 7.1 |
| held by 6 000, 25 % | 19.2 / 19.8 | 20.5 / 20.5 | 801.5 |
| held by 6 000, 100 % | 21.7 / 22.1 | 23.1 / 23.8 | 11 423.8 |
| held by all, 1 % | 1 363.8 / 1 364.0 | 28.9 / 28.9 | 29.2 |
| held by all, 25 % | 1 308.1 / 1 408.3 | 1 359.1 / 1 320.5 | 1 321.3 |
| held by all, 100 % | 1 455.8 / 1 526.4 | 1 516.1 / 1 556.3 | 13 598.7 |
| no text, 1 % | 913.6 / 927.6 | 22.4 / 23.0 | 22.9 |
| no text, 25 % | 901.8 / 965.0 | 1 196.5 / 1 207.1 | 1 191.9 |
| no text, 100 % | 918.3 / 961.3 | 976.2 / 1 013.1 | 12 578.2 |

PostgreSQL:

| window | today count / agg | chosen count / agg | window's road |
|---|---|---|---|
| held by 6 000, 1 % | 116.7 / 96.6 | 104.9 / 92.5 | 30.2 |
| held by 6 000, 25 % | 89.3 / 98.2 | 97.8 / 102.3 | 2 013.0 |
| held by 6 000, 100 % | 99.1 / 99.8 | 98.7 / 93.8 | 18 353.2 |
| held by all, 1 % | 3 843.6 / 3 961.2 | 85.6 / 82.9 | 97.2 |
| held by all, 25 % | 4 117.2 / 4 076.7 | 2 954.3 / 2 986.7 | 2 920.5 |
| held by all, 100 % | 3 898.3 / 4 079.3 | 4 598.3 / 4 649.3 | 20 544.8 |
| no text, 1 % | 2 222.5 / 2 214.2 | 38.0 / 33.1 | 46.8 |
| no text, 25 % | 2 284.7 / 2 342.2 | 1 807.9 / 1 800.8 | 1 814.4 |
| no text, 100 % | 2 274.0 / 2 411.2 | 2 423.5 / 2 674.5 | 17 615.8 |

The crossover — the window at which its own road costs today's — is
where the forced column meets today's. On SQLite it is a quarter of the
type under a text every record holds (1 321.3 against today's 1 408.3
at 25 %, 364.3 against 1 423.7 at 10 %) and between a fifth and a
quarter with no text (1 191.9 against 965.0 at 25 %, 311.7 against
960.0 at 10 %). On PostgreSQL it lies between a quarter and a half
(2 920.5 against 4 076.7 at 25 %, 6 988.2 against 4 097.8 at 50 %).
The ratio of four stands where it was recorded: a window narrower than
a quarter of the other side takes its own road, and the record's ratio
is that quarter.

The line the issue set — a window wide against holders or its type
reads no more than today and takes at most 1.3 times today's time —
holds on both engines for the count and the aggregate but at one
boundary, and the record states plainly that it is not met there:
SQLite's plain type at exactly a quarter takes the window's road (25 %
is the equality the rule admits) and its count takes 1 196.5 ms where
today's took 901.8 ms, 1.33 times, 2.3 % past the 1.3. Today's own
count swings 901.8 to 919.5 across the five windows, so the ratio is
1.30 against its slowest run and 1.33 against its fastest. The cause is
the equality itself: at a quarter both sides are the same count, the
rule takes the window's road, and that road pays the choice before
walking what today's count walks, so its worst ratio is set at the
crossover rather than by the road's cost elsewhere. Every other chosen
read is within 1.25.

The key page pays the choice before it takes either road: 1.1 to 2.2 ms
on SQLite and 3.4 to 26 ms on PostgreSQL against 0.1 and 0.4 to 0.8 ms
for today's first page, and the choice alone grows with the window it
declines — 61 ms on SQLite and 682 ms on PostgreSQL at half the type
and beyond, inside the 1.3 line only because today's read of that
window is already seconds. The page is the reading this record leaves
dearer than it found it.

### As built: steps 5 and 6 (lakelands#1706)

The two steps landed in one pull request.

- **The number's token.** `engines::lookup_token(value, kind)` answers the
  index's entry for a number or decimal field: `as_number(value)`,
  normalised, as its text. So `7`, `7.00` and `'7'` share one token, `-0`
  and `0` share one, and a value that reads as no number has no token and
  keeps no row. `engines::literal_token` reads a literal the same way, and
  `engines::looked_up` admits a number or decimal field the
  `lookup_index.<type>` setting names. A literal of such a field reads as
  today when it does not tokenise, and an `in` list with one value that
  does not is dropped whole.
- **The memory engine's lookup.** The projection keeps a
  `BTreeMap<(TypeName, FieldName, String), BTreeSet<RecordKey>>`, filled in
  `index` beside the order rows and recorded per record so a reindex can
  take its old entries out; a declaration that widens or narrows the set
  reindexes its type by pages. A read in key order pages the union of the
  tokens' sets (`lookup_after`); a read by the order field takes the
  holders' own rows for `Few` (`few_after`) and the order index filtered by
  the holders for `Many`; an exact text outside the head's roads reads the
  tokens' records. An earlier position and a kept view read as today.
- **The SQL engines.** SQLite's `index_record`, `unlook` and `relook`
  moved to tokens as PostgreSQL's did: each writes, drops and fills a
  number or decimal field's entry. PostgreSQL computes the token in Rust
  and sends it in one `unnest`; a moved record's entries in those fields
  go whole, its latest state's written again, so a write reads no state of
  the key it moves. The setting's fill reads the type's head states, a
  page at a time.
- **057.** The record now says the memory engine keeps the index in its
  projection, that a number field named in the setting keeps an entry of
  its value's token, and that a value reading as no number keeps none and
  is scanned for.
- **Conformance.** Three rows, `NUMBER_LOOKUP_READS_ITS_HOLDERS`,
  `NUMBER_LOOKUP_ONE_TOKEN` and `NUMBER_OUTSIDE_THE_SETTING`, run on every
  engine. The first is the claim in a test: 60 000 records, `v = 7` held by
  600, about 600 rows read by `=`, `equal`, `equal_any` and `in`. The
  setting's refusal now names a boolean asked of it, since a number is
  admitted.
- **Measured.** Product in 779, out 131, net 648; specification in 298,
  out 9, net 289. The core-size gate: product 103 767 of base 108 500,
  specification 188 108 of base 191 500, neither base nor ceiling raised.
  Steps 5 and 6 measured 648 / 289 against the 351 / 433 reckoned for them
  at ×1.35; the estimate table above carries the measured size and leaves
  the remaining steps as reckoned.

### As built: step 7 (lakelands#1638)

Step 7 is the pair's state, its rows and the roads that keep them. The
build's driver, `describe`'s `series`, and the reads that take a pair are
step 8.

- **The setting.** `series_index.<type>` is listed in
  `lakelands-api`'s settings page, `seam::SERIES_INDEX_SETTING`, and
  `check_setting` validates a value pair by pair: a stored type the reader
  is shown, an array of two-name arrays, a series field of text, reference,
  number or decimal, an order field of a kind the order index keeps, each
  declared and neither computed at the read. A refusal names the field it
  is about — `SeriesFieldKind`, `SeriesIndexShape`, and `UnknownField`,
  `OrderIndexKind` and `OrderIndexAtRead` as the other indexes use them.
  The value itself is `SeriesPair` / `SeriesState` in `lakelands-api`'s
  contract.
- **The rows.** `engines::order_rows` takes the type's declaration and
  answers a derived field's row for a record: eight bytes of the SHA-256 of
  the series value's token (`engines::lookup_token`, so a number series is
  its normalised number), then the order value's `order_key`, or `0xFF`
  where it holds none. A series value that reads as no number keeps no row,
  as under the number lookup. A record without a series value keeps none.
- **The fold.** A `series_index.<type>` setting folded from the log fills
  its pairs whole at its event and writes them `kept`; put beside the
  writer, through `put_record_for`'s live road, it writes them `building`
  for a build to fill and `Keep`, as a redeclaration's `Start` does. A pair the
  setting no longer names is taken back in that write: its state leaves
  the row and its rows go in the writer's turn, as the record's `Drop`
  says, so no row is left without a pair. The state is the `series.<type>`
  row: SQLite's `meta.v`, PostgreSQL's `meta.t` and the memory
  projection's map. A write keeps the pairs' rows from `Start`: the fold's
  field list is the type's order fields and its pairs' derived fields, and
  a `Start`, `Fill`, `Keep`, `DropChunk` and `Drop` for a derived name
  take the pair's own road in `choose_order`, apart from `Chosen` and its
  setting.
- **The exemptions.** `order_fields` and `sorted_fields` never name a
  derived field, so no read, no rebuild of the lake's own index and no
  chooser takes one. `rebuild_sorted` deletes only a `field` without
  U+001F, `start_sorted` and `mark_sorted` read the distinct fields without
  it, `eligible` refuses one, and `sweep` drops none.
- **`put_declaration`.** A type's declaration deletes its order rows and
  `sorted.<type>`, as 041's rule says; a kept pair in `series.<type>` goes
  back to `building`, so no reader takes rows the declaration may have
  moved, and the rows the head's records are indexed again under it. The
  pairs the setting at the head names are read again there, so a setting
  folded before the declaration that first declares its fields starts its
  pair at that declaration, as §How says.

**Where the tree and this record differ.** §How a lake with records gets
it says taking the setting back is `Drop`, one delete in the writer's
turn. The tree as 099 R7 left it takes a setting back in parts:
`DropChunk`, whose last chunk removes the pair, with `Choose::Drop`
deprecated, clearing the whole field in one step for a caller built against
it. The tree wins, and the pair takes the parts road as a setting's own
fields do. A setting put beside the writer that stops naming a pair is
still one delete in that write, the record's road, beside the parts road a
caller drives. `choose_order`'s refusal for a type with a `sort_index`
setting is likewise gone, as 099 R7 records: `eligible` refuses only a
derived field, and the lake may choose the type's other fields beside a
pair.

**Conformance.** No check row: until step 8 no read takes a pair, so the
suite has no answer to hold. The engines' tests hold the claim for now —
`a_series_pair_is_filled_whole_kept_and_put_back_to_start` and
`a_setting_put_before_the_declaration_starts_its_pair` on SQLite and
PostgreSQL, and `a_series_pair_is_filled_from_the_log_kept_and_put_back_to_start`
and `a_setting_put_before_the_declaration_starts_its_pair` on the memory
projection, which walks the live road too — and `engines`' unit tests hold
the pair's validation, the hash and `0xFF` row, and the fields the lakes
never name. Step 8 adds the suite's rows with the reads.

**Measured.** SQLite, five rounds of a fresh lake, 100 000 writes of a
three-field record in transactions of 1 000, the pair's setting put
before them: a write cost 13.5–30.1 µs without the pair (median 16.2)
and 25.1–38.4 µs with it (median 28.2), slower in every round; the
per-round difference was 5.5–24.7 µs, median 8.9. The one-pass fill of
the 100 000 records at the setting cost 0.46–1.03 s. The write cost is
the series token's hash and one more `sort_index` row a record, not the
row's bytes.

**Size.** The step landed at 1 051 product lines and 821 specification
lines, against the 230 and 270 the estimate above reckoned with ×1.35,
and above the record's 600 a pull request. The rows are written three
times, once in each engine, as the store's other indexes are; nothing in
it was cut for the number, and step 8's driver stands as reckoned.

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
these. Each stands here until 098 and 099 are accepted, and they say what they
change:

- `sort_index` and `text_index` as the rows' home. A changed format moves the
  rows' shape, not the roads. 098 keeps both tables' columns and primary keys; its
  only change here is the bound a window down takes on PostgreSQL.
- A number field's fill, a single pass today, into the bounded parts of
  lakelands#1651. The same goes for the series build, if one road for parts
  carries it, and for `choose_order`'s refusal. 099 carries all three on its road.

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
- Amended by 098 and 099 (lakelands#1652), proposed for 0.23.0.
