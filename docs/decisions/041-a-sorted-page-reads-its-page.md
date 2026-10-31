# 041 — A sorted page reads its page

## What is decided

### A page ordered by a field the lake asks an index for reads its page

A `find` page ordered by a field the lake asks an index for reads about
`limit` rows at the head, as a page ordered by key does, on every
engine. Each engine keeps an **order index** of the head for the fields
asked for, and the page is read in the index's order and stops when it
is full. A field not asked for is ordered by sorting the type.

- **The key.** Every value in the index is its `sort_key`
  (`lakelands_core::engines`): bytes whose order is the one order across
  kinds (025), and whose equality is that order's equality, so `1` and
  `"1.00"`, or one instant written in two offsets, share a key. The order
  is still defined by `cmp_values`. A property test holds the key to it,
  and the key never decides an order the rulebook does not.
- **Asked for per field.** The `sys.setting` record `sort_index.<type>`
  holds a JSON array of field names and asks for the index on them.
  The store takes only fields the type declares with a scalar kind
  (text, number, decimal, boolean, date-time, date or reference) and
  refuses a write of the setting naming any other, or a type it does
  not declare, as invalid, naming the field. Retiring the setting, or
  an empty array, asks for no index.
- **The rows.** One row per field asked for that the record's latest
  state holds with a value, retired and restricted states included. A
  reader decides from the setting and the declaration at the head which
  orders the index answers, never from which rows exist. A
  redeclaration rewrites the rows of the type's records under it, as it
  does the other head indexes, and a field it no longer declares with a
  scalar kind loses its rows.
- **The setting's write rebuilds.** The fold that puts the setting's
  head state rewrites that type's rows in the same transaction: rows for
  the fields now named, none for the fields no longer named, and the
  type's mark. So the index describes the head from the next read on,
  with no restart. One pass over the type per change of the setting,
  paid by that write.
- **The cut.** A row keeps at most the first 32 bytes of its key. A kept
  key that long may have been cut, so rows sharing it are a run that
  `cmp_places` orders from the values, as below; a page that continues
  inside such a run reads it from its start and keeps what lies strictly
  after `after`, because record keys inside a run are not in the values'
  order. Instants and numbers written in JSON fit in 32 bytes; a long
  decimal, a reference and text may not.
- **Two directions.** The index is ordered `(sort key, key)` and `(sort
  key descending, key)`. Either way the key ascends inside a tie, as the
  rulebook breaks ties, so a page ordered by one field is the index's
  order and nothing is sorted afterwards.
- **Several fields.** The first field's index orders runs of equal kept keys.
  Inside a run, `cmp_places` orders by the other fields and the key, so
  the index never decides a tie. The records missing the first field
  come last, in key order, or as one run ordered by `cmp_places` when
  there are further fields.
- **Only while it describes the head.** The fold marks each type's
  index as kept through the position it commits (`sorted.<type>` beside
  `folded`). A view uses a type's index only when the two are equal.
  Otherwise it sorts the type, and the answer is the same. A
  writer's open rebuilds, once, every index that does not describe the
  head: one whose mark a writer without the index left behind, and one
  whose setting such a writer wrote.
- **The ceiling.** An order by `known_at` or `seen_at`, an order by a
  field the lake did not ask an index for, and a view type that
  computes are sorted whole by `sort_key`, and so is a view
  at an earlier position whose type changed too much since (below). So
  is a multi-field page whose first field has few values: its runs are
  long.
  `describe_type` names a type's indexed fields in its lifecycle
  (`sorted`), so an agent can see which orders are cheap. The seams page
  gives the numbers, and what an index costs a write.

### The lake chooses an order index from what is ordered by, beside its log, and the host's setting wins

**What changes no answer, the lake may keep beside the log; what it keeps
there is learned from use and never logged.** An order index changes only
what a page costs. A host's `sort_index.<type>` setting is the logged
choice, with a history an auditor can read, and it wins; the lake's own
choice is a cache decision of the kind every engine makes for itself, and
a choice written to the log would be a write decided by how it is read,
which a host's feed, its mirrors and its exports would carry as no
caller's (lakelands#1029, lakelands#1131).

- **What is counted.** Each `find` page ordered first, by value, by a
  scalar field of the type that `sort_index.<type>` does not name, is
  counted for that field with the time the read took, since the lake was
  opened in the process that answers, and within a window. An order by
  key, by `known_at` or `seen_at`, a page of a view and an aggregate are
  not counted, and neither is a page the index answered. Only the
  process that holds the writer acts on the count: a reader-only process
  has no writer to build with, and its pages are not seen there.
- **When a field is chosen.** Once `limits.order_after_pages` (3) pages
  ordered by it came within `limits.order_window_secs` (10 minutes), on a
  type holding at least `limits.order_min_records` (2 000) live records
  and fewer than `limits.order_max_fields` (2) chosen fields, and with no
  `sort_index.<type>` setting naming a field. At 2 000 records an
  unindexed page costs about 15 ms on SQLite, so a build below that size
  is noise either way. The switch `limits.choose_order_index` is on; off,
  only what the host sets is built, as before. All of them are members of
  the lake's `Location`, never of the environment.
- **Where it is kept.** In the projection's meta row `chosen.<type>`,
  beside the mark `sorted.<type>`: the field names kept, and those being
  built. It is text in a column that takes text, so there is no layout
  step and no format change. Nothing enters the log: a feed, a mirror,
  an export and an import carry none of it, and a projection rebuilt
  from the log starts with none and learns again from use.
  - **A writer that keeps no chosen field.** A binary before this one
    reads the lake (the format and the layout are the same), and its fold
    leaves a chosen field's rows as they were while it moves every
    `sorted.<type>` mark. So the chosen fields carry a mark of their own,
    one for the lake: the meta row `chosen_through`, the position they
    were kept through. The fold moves it to the position it folds
    through only when it stands at the position folded from, so once
    such a writer folded past it, it stays behind, even when a writer
    that keeps them takes the lease back without opening the lake
    again. Such a writer's rebuild on PostgreSQL drops the order index
    and keeps `meta`, so there the mark holds only beside a twin row in
    the order index, which every rebuild drops. A read takes a chosen
    field, and `chosen` tells it, only while the mark holds; no step of
    choosing is taken while it does not; and a writer's open drops the
    lake's whole choice and its mark, after which the lake learns again
    from use.
- **How it is built: beside the writer, never inside one append.** In
  steps the engine takes outside a transaction under the writer's lock
  (`ProjectionStore::choose_order`): `Start` makes the fold keep the
  field's rows for every record put from then on; `Fill` writes the rows
  of the head records in chunks of `limits.order_chunk` (10 000) in key
  order, each chunk a short write between appends; `Keep` marks the field
  as one a read may take. Because the fold keeps a building field's rows
  from `Start`, a record put during the build has its row and nothing is
  caught up at the end. A reader takes a field only once it is kept and
  the type's mark equals the head and `chosen_through` holds, so an
  answer never depends on the build, and a page ordered by a field being built
  sorts the type, as before.
  - **A write waits for a chunk, not for the build.** The setting's own
    write rebuilds the type's rows inside its append and stops every
    write for the whole pass: about 0.85 s at 100 000 records on SQLite
    and 2.4 s on PostgreSQL. A write sent while 100 000 records were
    built waited at most 160 ms on SQLite and 320 ms on PostgreSQL, with
    each chunk 140 to 190 ms on PostgreSQL. Each chunk is a write of its
    own that waits its turn in the lake's queue of writes, which serves
    them in the order sent, so a write sent during the build goes before
    the next chunk; the builder does not rest between chunks. A lock that
    is not fair, as the writer's was before writes queued, let a write
    sent during the build wait for nearly all of it in the runs measured.
- **The host's setting wins.** A `sort_index.<type>` setting naming a
  field takes the lake's choice away in the same rebuild that writes the
  setting's rows, and the lake chooses nothing for that type. Retiring
  the setting hands the type back; the lake may then choose again.
- **Dropped when unused.** A chosen field no page has been ordered by for
  `limits.order_unused_secs` (7 days of the writer's life) is dropped by
  the same steps, `Drop`, which deletes its rows. The first `find` a
  writer answers in a process starts every field it finds chosen as just
  used, and drops one left half built by a task that died, so a restart
  never drops one early. A lake opened with the switch off drops there
  every field an earlier writer chose. A drop deletes the field's rows in
  one step under the writer's lock, which is not chunked as a build is.
- **Where it is told.** `describe_type`'s lifecycle gains `chosen`, the
  fields the lake chose and keeps, beside `sorted` (the fields the setting
  names) and never in it. Like `sorted_whole` it is told to a holder of
  `schema` who reads the field, and to no other reader: the choice says
  what other readers ordered by. It is told at the head only, where the
  index is kept. `sorted_whole` stops naming a field once the lake keeps
  it or builds it. It reads no value.
- **What it costs.** Each chosen field adds an index entry to every
  state put: measured through the store (release builds, 20 000 and 100 000
  records of 11 fields, two lakes side by side, one with the switch off),
  a single upsert's median moved by about 0 to 2 % on SQLite and by 20
  to 24 % on PostgreSQL, and a 10 000-record ingest by −2 to +22 % on
  SQLite, 0 to +43 % on PostgreSQL and +9 % on the memory engine, the
  runs' spread as wide as the change. `limits.order_max_fields`
  bounds what the lake can add to a type's writes. Its default is 2,
  because the switch is on by default and the cost of a lake that chooses
  is the cost a host did not ask for, agreed with the operator: twice one
  field's cost, not measured at two, a single upsert about 0 to 4 % on
  SQLite and 40 to 50 % on PostgreSQL. A host that raises the bound to 4
  pays four times one field's cost, not measured at four: a single upsert
  about 0 to 8 % on SQLite and 80 to 100 % on PostgreSQL, a bulk ingest
  up to about 90 % on SQLite (within the runs' spread at one field) and
  up to about 170 % on PostgreSQL. A field ordered by on a large type pays its build back
  on its first page or two, as a host's own setting does:
  at 100 000 records a first page ordered by an unindexed field
  cost 0.6 to 1.1 s on SQLite and 1.0 to 1.2 s on PostgreSQL, the whole
  build 2.0 s and 3.5 s, and the page then 1.6 ms and 2.6 to 6.2 ms.

### An earlier position reads the head index and the keys changed since

A page at a position `p` before the head, ordered first by a field the
head's index answers (the index kept through the head, the field named
by the head's setting), reads that index too:

- **The keys changed since.** `C` is every key of the type with a state
  put after `p`, or with a state that held at `p` and ended after it.
  The page passes over the index rows of `C` and merges in the states of
  `C` at `p`, in the index's order: the kept sort key and the key, the
  sort key descending for a descending first field; the records without
  the first field in key order. Every other key's head state is its
  state at `p`, so the page is the one a sort of the type at `p` gives,
  byte for byte. A key written only after `p` is not in it; one retired
  after `p` is, with its state at `p`.
- **One read.** The index, `C` and the states at `p` are read in one
  snapshot: SQLite's read transaction, PostgreSQL's `REPEATABLE READ`
  read, the memory engine's lock. A commit between two pages of a view
  changes neither.
- **The merge is shared.** `lakelands_core::engines::
  scan_in_field_order_at` passes over and merges; an engine finds `C`
  and the states. SQLite reads `C` by two range reads,
  `records_since (ty, since)` for the states put and `records_at (ty,
  until, key)` for the states that ended; a `UNION` of the two is planned
  by SQLite as a merge in key order, which reads the type. PostgreSQL
  reads `records_since` alone, since a state there ends only where its
  key's next state is put (lakelands#537). The memory
  engine keeps, per type, the position and key of every state put.
- **The ceiling.** More than `min(10 × limit, 4 096)` states put since
  `p`, or keys in `C`, and the page sorts the type. Every
  state of `C` is held for the page, so the ceiling keeps what a page
  holds near a page. It sits far below where the merge stops paying.
  At 100 000 records, one text field indexed, a page of 50 or 1 000:

  | keys changed | SQLite | PostgreSQL |
  |---|---|---|
  | 50 | 0.5 / 3.1 ms | 1.4 / 8.5 ms |
  | 500 | 2.1 / 5.2 ms | 8.0 / 21.6 ms |
  | 4 096 | 18.8 / 22.7 ms | 33.7 / 54.0 ms |
  | 30 000 | 130 / 141 ms | 176 / 177 ms |
  | the sort | 177 to 194 ms | 224 to 265 ms |

- **What it costs a write.** One more index entry per state put, on
  SQLite and PostgreSQL. Projection writes of 100 000 records, 32 per
  commit, seven interleaved runs each: SQLite 3.39 s without and 3.42 s
  with (medians, +1 %); PostgreSQL 12.63 s and 12.93 s (fastest runs,
  +2 %), its medians within the runs' spread. The memory engine keeps a
  position and a key per state put.

**Why.** A cursor keeps a walk at its first page's position, so its
pages stay one answer while the lake is written. Each page after a
write was a view at an earlier position, and sorted the type: the write
the cursor keeps out of the walk made every later page cost the type.

The head index still describes every key that has not changed
since `p`, which in a walk is nearly all of them. Reading it and only the
few that changed keeps a pinned page at about its page, as the head's
is. The streaming check holds a walk with writes between its pages to
its page, the one row more the store asks for, three more and two per
changed key; before, each page after a write read the type.

### A page under an exact text reads its page

SQLite and PostgreSQL read a page at the head ordered first by an
indexed field, whose scan must hold a text exactly (a string in `equal`,
or the filter's exact-text term), through the order index as a page
with no text is read, and hand the rulebook only the records holding
the text. There are two ways to find them, and each is cheap where the
other is not:

- **Many holders: the order index first.** The order index is read in
  its order from `after`, and a row is kept where the exact-text index
  holds its key, a seek per row. For N records of which M hold the
  text, spread through the order, a page reads about `limit × N / M`
  index rows, and a walk about N in all. Where the holders sit at the
  far end of the order, a page reads every row before them, up to the
  type, as a page with no text reads every row its filter rejects.
- **Few holders: the text first.** The text's rows in the exact-text
  index are joined to their order rows, ordered and cut at the page.
  These are index rows only; no record is read past the page. A page
  reads M index rows, and a walk about `M² / limit`.
- **The switch.** Once per page the engine counts the holders, up to
  4 096 and one more (`FEW_HOLDERS`), by a range of the exact-text
  index. At most 4 096, and the page reads the text first: a seek of
  each holder's order row, at most 4 096 seeks into an index as large
  as the type, and a sort of what they found. On SQLite that is about
  3 ms at the ceiling in a type of 41 000 records and about 30 ms in
  one of 1 000 000; on PostgreSQL the planner chooses how a batch is
  read, and it costs with the batch and the type as well (measured
  below, lakelands#695). More, and the order index first: about
  `limit × N / 4 096` index rows, about 12 000 for a page of 50 at
  1 000 000 records, where the holders are spread through the order.
  So a text no longer makes a page read its holders whole, whatever the
  type's size. The two ways meet where `M² = limit × N`; a
  switch there would need N, and counting N is the type again.
  The fixed ceiling costs one case: a walk in small pages under a text
  held by up to 4 096 records reads up to 4 096 index rows a page, where
  the order index would read `N / M`. Each page stays bounded, and a host
  chooses its page.
- **A filter that rejects rows.** The rest of the filter, restricted
  and retired records, `only` pairs and `valid_at` reject some of the
  rows a batch answers, and the page asks for another. Each further
  batch is at least twice the one before, up to 1 024
  (`scan_in_field_order`), so a page that passes over R rows asks for
  about `log₂ R + R / 1 024` batches. The text first needed it most:
  each of its batches then read every holder again. Before the doubling, a
  page of one under `v = 'at' and w > 40000`, 4 096 holders of 41 000
  records, asked for a batch for every two rows it passed over. On
  SQLite, the median of five, `w` indexed or not:

  | text (holders) | way | limit | not indexed | one row more a batch | doubling |
  |---|---|---|---|---|---|
  | `at` (4 096) | text first | 1 | 12.6 ms | 9.3 s, 3 597 batches | 50 ms, 14 |
  | `at` (4 096) | text first | 10 | 11.6 ms | 964 ms, 361 | 43 ms, 11 |
  | `past` (4 097) | order first | 1 | 12.1 ms | 137 ms, 3 598 | 34 ms, 14 |
  | `past` (4 097) | order first | 10 | 11.1 ms | 44 ms, 361 | 34 ms, 11 |

  Such a page still cost a few times the sort of every holder: the
  text first read its holders once a batch, and the order first every
  row before the first it admits. A page read text first that asks for
  more than three batches reads the holders left once (below,
  lakelands#695).
- **The records without the field** come last. They are read
  from the text's rows in key order, with those that have an order row
  passed over: at most the holders.
- **The rulebook admits each.** `lakelands_core::engines::Holders` names
  the way. `scan_in_field_order`, its runs and the continuation inside a
  run are unchanged, so no answer changes and no right does.
- **The planner is told.** On PostgreSQL, the setting's write rebuilds
  the type's order index in one transaction, and its rows count toward
  the writer's analysis as records put do. Before, a page planned right
  after the setting saw the index as the last analysis left it, empty,
  and could probe it once per holder: in the probe, one page under a
  common text ran for twenty minutes at 100 000 records.

An earlier position, an order by a field the lake does not index, and
the memory engine take neither way. The memory engine keeps no
exact-text index, and reads the order index's rows in its order and
admits each; 097 gives it the index and these ways.

`rows_read` counts the rows handed on, so it does not see the rows the
order-first way passes over; 097 adds `rows_walked`. A bound on the
order field narrows either way and skips the records without the field,
and a series index reads one text's window by itself (097).

**The cost.** 100 000 records, a page of 50 in an indexed field's order
under a text, then a walk of every holder by cursor, 1 000 a page. The
first page's median of five, then the walk. Before, the page read every
holder and sorted them, as it still does where the field is not
indexed. Other worktrees were building on the same machine (load about
5).

| SQLite, text held by | way | before | after |
|---|---|---|---|
| 72 029 | order first | 346 ms / —¹ | 0.94 ms / 641 ms |
| 14 487 | order first | 82 ms / 1.2 s | 1.1 ms / 168 ms |
| 4 790 | order first | 27 ms / 149 ms | 1.4 ms / 93 ms |
| 3 000 | text first | 18 ms / 61 ms | 3.1 ms / 33 ms |
| 603 | text first | 4.4 ms / 5.2 ms | 1.3 ms / 5.5 ms |
| 97 | text first | 0.91 ms / 1.0 ms | 0.75 ms / 1.2 ms |

| PostgreSQL, text held by | way | before | after |
|---|---|---|---|
| 74 280 | order first | 725 ms / —¹ | 7.8 ms / 4.4 s |
| 19 879 | order first | 207 ms / 4.6 s | 9.4 ms / 982 ms |
| 4 742 | order first | 51 ms / 328 ms | 57 ms² / 191 ms |
| 603 | text first | 10.4 ms / 16 ms | 9.0 ms / 21 ms |
| 100 | text first | 7.4 ms / 6.9 ms | 6.3 ms / 9.5 ms |

¹ Not walked: some 75 pages, each sorting every holder. An earlier
SQLite run with 74 900 holders walked in 29.1 s.
² PostgreSQL plans this page in parallel, and starting two workers takes
about 20 ms of it. It reads about 2 200 index rows.

**At 1 000 000 records** on SQLite, a page of 50 under texts held near
the ceiling:

| text held by | way | before | after |
|---|---|---|---|
| 4 979 | order first | 41.7 ms | 8.1 ms |
| 3 000 | text first | 24.2 ms | 8.1 ms |
| 997 | text first | 14.3 ms | 2.1 ms |
| 601 | text first | 4.0 ms | 1.9 ms |
| 400 | text first | 4.8 ms | 1.1 ms |

No text near the ceiling pages slower than before. A text held by 3 in
4 records pages in 0.62 ms, and a walk of all 748 180 of them takes
10.0 s, where each of its 749 pages sorted every holder before. That
figure was measured with a ceiling of `min(10 × limit, 4 096)`, which
reads a text with that many holders the same way.

**Why.** Before, such a page read every record holding the text and
sorted them, for each page, whether the lake indexed the field or not.
A walk of such pages cost the holders once per page: a square for a
host that pages through a sorted, filtered type.

The order index already answers a sorted page in its page. A
text the page must hold either thins its rows, which the order index
walks past cheaply, or picks out a few, which the text index names
directly. Counting the few, up to a ceiling, is a short range read, and
it tells the two apart without reading the type.

### A page under a text few hold reads them once

At the head, SQLite and PostgreSQL read the first three batches of a
page read text first as above, each by itself, and what is left of its
few holders (`Holders::Few`) once, when the page asks for a fourth
batch. `scan_in_field_order`, its doubling, its runs and its
continuation are unchanged, and no answer changes: the rulebook admits
each record.

- **Three batches each by itself** (`engines::BATCHES_BEFORE_HELD`). The
  store asks its engine for one row more than a page, so a page with one
  retired or restricted record among its rows is a page of two batches,
  and one with two or three such records a page of three. Those are
  ordinary pages, and the commonest read is the page that fills in its
  first batch. Each of them runs the statements it ran before, so it
  costs what it cost. A page that asks for a fourth batch has been
  handed three and is still not full: its filter rejects rows, its
  batches double from here, and those are the batches that would each
  read every holder again.
- **The fourth batch reads the rest once,** after the last row the
  three batches read, in the same read transaction and so of the same
  snapshot. Every later batch takes from that one read.
- **PostgreSQL: one cursor.** The view declares a `NO SCROLL` cursor
  after the last row read and fetches each later batch from it. The
  cursor lives in the page's `REPEATABLE READ` transaction, which every
  scan runs in and which the three batches ran in, and ends with it. A
  fetch asks for a power of two, at most 1 024, so a connection prepares
  a dozen fetches at most; the rows past a batch wait for the next.
- **The cursor's statement has one plan.** The text's rows are
  materialized, each holder's order row is sought by its key (`CROSS
  JOIN LATERAL (… LIMIT 1)`, a seek into the order index's primary key),
  the rows are sorted once, and each record is sought by its key as the
  page fetches it. Its cost is a seek a holder and one sort, at most
  `FEW_HOLDERS` of each, whatever the type's size and whatever the
  statistics say. The engine's own test reads the plan at five holders
  and at the ceiling, either way, and no step of it may read a table
  whole or stay unexecuted. A plain join left the choice to the planner,
  which at the ceiling read every order row of the type and hashed them:
  the cursor then cost with the type, and its first fetch ran the whole
  join (measured and not taken, below).
- **SQLite: the holders' order rows held.** At the fourth batch the
  view reads the sort key and key of every holder after the last row
  read, in the index's order for the direction: index rows only, at most
  `FEW_HOLDERS`. Each later batch takes the next ones and seeks their
  head records.
- **The batches read each by itself keep their statement** and, on
  PostgreSQL, a plan that hangs on the statistics as it did. It is not
  pinned here and not made worse.
- **Counted.** `engines::holder_reads` counts the times a thread read a
  text's holders whole to keep them, beside `batches_read`, so the
  engines' checks pin that a page of up to three batches reads none and
  a longer page one, however many batches it asks for.

**Why.** Before, a page read text first read every holder again for
each batch it asked for: a statement over the text's rows, each one's
order row sought, sorted, and cut at the batch. Under a filter that
rejects rows a page of one asks for 14 batches, so it read and sorted
its holders 14 times where a page without the index sorts them once.

**The cost.** Three lakes of 41 000, 200 000 and 1 000 000 records,
every record with an order row in `w`, and in each a text held by 5, by
100 and by 4 096 records (the ceiling). Pages in `w`'s order under the
text; the rest of the filter rejects the first rows in that order, and
so sets how many batches the page asks for. A page of 10 is the
store's: its engine is asked for 11 rows. Measured on the change as
built against the same tree with the engines' sources before it, in
release builds: both builds' binaries ran in turn against the same
lakes, with no build between. A figure is the median of ten rounds, in
milliseconds, with the fastest and the slowest round; a round is the
median of five pages, a fresh view a page. Other worktrees were
building on the same machine throughout (load 9 to 25), so the rounds
spread widely, and the fastest round is the least disturbed one.

| SQLite, records | page under a text 4 096 hold | batches | before | after |
|---|---|---|---|---|
| 41 000 | a page of 1, full at its first row | 1 | 4.45 [3.11..5.97] | 3.99 [2.98..6.89] |
| 41 000 | a page of 10, no row rejected | 1 | 3.45 [2.96..6.25] | 3.44 [2.91..6.21] |
| 41 000 | a page of 10, 3 rows rejected | 3 | 10.0 [8.69..18.1] | 9.98 [8.35..17.6] |
| 41 000 | a page of 1, 4 rows rejected | 4 | 15.8 [11.8..24.1] | 14.3 [13.2..27.5] |
| 41 000 | a page of 1, 8 rows rejected | 5 | 17.8 [14.3..42.2] | 14.8 [13.2..25.5] |
| 41 000 | a page of 1, 40 rows rejected | 7 | 23.0 [20.4..42.5] | 14.6 [12.5..26.5] |
| 41 000 | a page that admits none | 15 | 67.6 [60.4..125] | 31.6 [26.8..59.8] |
| 200 000 | a page of 1, full at its first row | 1 | 4.76 [4.27..9.10] | 5.12 [4.22..7.94] |
| 200 000 | a page of 10, no row rejected | 1 | 6.32 [4.46..9.61] | 5.40 [4.66..9.45] |
| 200 000 | a page of 10, 3 rows rejected | 3 | 18.9 [12.7..35.1] | 17.3 [13.3..26.6] |
| 200 000 | a page of 1, 4 rows rejected | 4 | 20.1 [16.8..35.3] | 20.7 [18.3..34.1] |
| 200 000 | a page of 1, 8 rows rejected | 5 | 25.0 [21.4..44.7] | 22.9 [19.1..33.3] |
| 200 000 | a page of 1, 40 rows rejected | 7 | 33.5 [30.2..69.9] | 21.8 [19.3..38.6] |
| 200 000 | a page that admits none | 15 | 140 [126..276] | 81.6 [68.1..167] |
| 1 000 000 | a page of 1, full at its first row | 1 | 51.7 [26.4..75.6] | 31.6 [27.3..83.6] |
| 1 000 000 | a page of 10, no row rejected | 1 | 30.4 [26.8..80.9] | 32.7 [27.8..82.1] |
| 1 000 000 | a page of 10, 3 rows rejected | 3 | 95.2 [83.1..242] | 98.0 [80.9..258] |
| 1 000 000 | a page of 1, 4 rows rejected | 4 | 123 [110..310] | 133 [119..333] |
| 1 000 000 | a page of 1, 8 rows rejected | 5 | 156 [135..407] | 130 [115..326] |
| 1 000 000 | a page of 1, 40 rows rejected | 7 | 212 [197..572] | 136 [111..333] |
| 1 000 000 | a page that admits none | 15 | 955 [467..1405] | 213 [176..530] |

| PostgreSQL, records | page under a text 4 096 hold | batches | before | after |
|---|---|---|---|---|
| 41 000 | a page of 1, full at its first row | 1 | 6.61 [5.55..9.16] | 7.03 [5.55..10.3] |
| 41 000 | a page of 10, no row rejected | 1 | 17.8 [15.9..30.7] | 25.9 [16.8..33.0] |
| 41 000 | a page of 10, 3 rows rejected | 3 | 59.4 [44.5..92.8] | 60.0 [51.9..89.2] |
| 41 000 | a page of 1, 4 rows rejected | 4 | 53.4 [40.4..73.3] | 70.9 [51.7..98.4] |
| 41 000 | a page of 1, 8 rows rejected | 5 | 82.7 [56.3..101] | 78.2 [53.0..111] |
| 41 000 | a page of 1, 40 rows rejected | 7 | 98.1 [85.1..160] | 73.9 [53.8..106] |
| 41 000 | a page that admits none | 15 | 281 [220..356] | 142 [99.3..186] |
| 200 000 | a page of 1, full at its first row | 1 | 7.86 [6.61..13.2] | 9.14 [6.61..11.7] |
| 200 000 | a page of 10, no row rejected | 1 | 85.7 [67.8..135] | 105 [73.0..136] |
| 200 000 | a page of 10, 3 rows rejected | 3 | 127 [118..191] | 147 [115..202] |
| 200 000 | a page of 1, 4 rows rejected | 4 | 93.1 [80.5..143] | 105 [89.3..133] |
| 200 000 | a page of 1, 8 rows rejected | 5 | 115 [97.4..158] | 101 [90.6..143] |
| 200 000 | a page of 1, 40 rows rejected | 7 | 172 [137..254] | 118 [94.5..168] |
| 200 000 | a page that admits none | 15 | 694 [542..1000] | 190 [164..254] |
| 1 000 000 | a page of 1, full at its first row | 1 | 8.73 [5.86..12.1] | 8.83 [6.05..12.1] |
| 1 000 000 | a page of 10, no row rejected | 1 | 28.7 [25.0..48.7] | 27.7 [24.0..48.3] |
| 1 000 000 | a page of 10, 3 rows rejected | 3 | 92.2 [70.4..135] | 85.0 [70.3..142] |
| 1 000 000 | a page of 1, 4 rows rejected | 4 | 186 [170..271] | 172 [161..273] |
| 1 000 000 | a page of 1, 8 rows rejected | 5 | 205 [191..311] | 193 [165..276] |
| 1 000 000 | a page of 1, 40 rows rejected | 7 | 270 [239..400] | 173 [165..289] |
| 1 000 000 | a page that admits none | 15 | 819 [631..1155] | 425 [305..557] |

- **A page of up to three batches** runs the statements it ran before.
  The differences between its two columns are the machine's.
- **A page of four batches pays for waiting.** It reads three batches
  and then the rest once, where its fourth batch was a small statement
  before. On PostgreSQL at the ceiling that is about 10 to 17 ms more
  at 41 000 and at 200 000 records (the fastest rounds: 40 to 52 ms,
  and 81 to 89 ms), inside the rounds' spread but there by
  construction; at 1 000 000 the two columns lie within a tenth of each
  other. On SQLite at the ceiling the page pays too: its fastest rounds
  are 12 %, 9 % and 8 % slower at the three sizes (11.8 to 13.2 ms,
  16.8 to 18.3 ms, 110 to 119 ms), 1.4 and 1.5 ms at the two smaller,
  inside the spread and there by construction as well. From the fifth
  batch on the one read is the cheaper, and a page that admits none
  takes between a quarter and three fifths of the time.
- **A text held by 5 or by 100:** every page takes under 20 ms on
  either engine at every size, and in every cell the two columns lie
  inside the rounds' spread. A page that admits none of 100 holders
  takes about three quarters of the time on PostgreSQL; one of 5
  holders asks for four batches and costs what it cost.
- **What a batch read by itself costs on PostgreSQL is the planner's
  choice,** and the table shows it on both sides: a first batch of one
  takes 6 to 9 ms at every size, a batch of eleven at 200 000 records
  about 90 ms (the type's order rows read and hashed), and a second
  batch of one at 1 000 000 over 100 ms. That is the cost of the
  statement before this change and after it, in a page of one to three
  batches; it is not pinned here.
- **The type's size is in every figure.** Each holder's order row is a
  seek into an index as large as the type, in a batch and in the one
  read alike: on SQLite a one-batch page at the ceiling takes about
  3 ms at 41 000 records and about 30 ms at 1 000 000.

**Measured and not taken.**

- **The rest read at the first batch** (a cursor from the first row on
  PostgreSQL, the held rows on SQLite): the first cut of this change.
  With the holders joined, not sought, the planner read every order row
  of the type at the ceiling and hashed them, and the first fetch ran
  that join: a page that filled in its first batch took 13 ms where it
  had taken 9 at 41 000 records, and in the review of this change the
  declaration with its first fetch measured 25, 74 to 78 and 43 to
  61 ms at 41 000, 200 000 and 1 000 000 order rows against 4, 5 to 7
  and 19 to 21 ms for the batch before (in `psql`, PostgreSQL 16). On
  SQLite the held rows cost a one-batch page the transfer of every
  holder's row: at the ceiling its fastest rounds were 4.0 ms against
  2.9 at 41 000 records, 5.6 against 4.2 at 200 000 and 30 against 26
  at 1 000 000. The commonest page is the one that fills in its first
  batch, so that batch is read by itself.
- **These three were measured on the prototype** (on `7c94a43c`, whose
  cursor fetched each batch's own count and measured 84 to 89 ms
  admitting none and 15 ms on an admitting page of one), and their
  figures are the prototype's. Reading the first batch by itself and
  the rest of the holders once at the second: an admitting page asks
  for two batches anyway (restricted rows), so it paid both, and was
  slower than either. On PostgreSQL, the holders' sort keys and keys
  read to the client at the first batch and the records sought by key
  per batch: it sends 4 096 rows a page may not use (118 to 142 ms
  admitting none, 24 ms on an admitting page of one). A cursor over the
  keys alone, the records sought per batch: no better than the cursor
  with the records.

### A host asks for an order index of a kept view as it does for a stored type's

A kept view's records are records of its kept name, `sys:kept.<view>`
(049). A host asks for an order index of a kept view as it does for a
stored type's.

- **The setting names the view.** `sort_index.<view>` is taken for a
  declared view, on fields the view declares, under the same refusals as
  a stored type's. A vector index remains a stored type's alone. It is
  asked for per view, not kept for every kept view: the index costs
  every write that changes a kept group, whether or not anyone orders by
  it.
- **The engine reads the view's declaration.** A kept name's order
  fields are those the view's setting names that the view declares at
  the head (`engines::sorted_fields`, `kept_view`). A kept name still has
  no declaration of its own, so nothing else an engine derives changes.
- **The rows follow the kept records.** The fold puts a kept record again
  at every write to its group, and its rows follow it as a stored
  record's do; an ended group has none. The setting's write rebuilds the
  kept name's rows with the view's own (`ordered_names`). A declaration
  of the view needs nothing further: the store makes the view again,
  putting every group. A writer's open catches up the kept name's index
  as it does a type's: each setting asks for the indexes of
  `ordered_names`, and of those only one that keeps a field has a mark
  to be behind, so the view's own name, which keeps none, costs an open
  nothing.
- **The store does not change.** A kept page is the engine's page,
  merged with the groups whose whole face reads otherwise (lakelands#628);
  the store holds every row it is handed to the reader's grants
  (`restricted`, `only`), and at an earlier position the engine
  reads the head's index and the keys changed since, as above.
  `describe_type` names a view's indexed fields in its lifecycle where
  the view is kept, or asked to be and can be, as its `kept` tells that
  reader (lakelands#748). A view that computes for want of `view_kept`,
  or cannot be kept, has no records to index, and to a holder of `only`
  an index would say whether the view is kept, which 051 keeps from it.

**Why.** A kept name has no declaration, and no setting could name it,
so a page of a kept view in another order than its key read every kept
record and sorted them. At 100 000 groups that took about 400 ms on
SQLite and 500 ms on PostgreSQL, and held some 465 MB, more than
computing the view holds.

**The cost.** 200 000 records in 100 000 groups of two, a kept view of
`count()` and `sum(v)`, a page of 10, medians of five:

| a page of 10 by a measure | SQLite | PostgreSQL |
|---|---|---|
| no index | 357 to 433 ms, +463 MB | 471 to 911 ms, +473 MB |
| `sort_index.<view>` on both measures | 0.16 to 0.24 ms, +0.1 MB | 1.0 to 1.2 ms, +0.1 MB |

The setting's write after the ingest built the index in 0.8 s on SQLite
and 3.9 s on PostgreSQL. With the setting written before it, the ingest
took 59.5 s against 56.5 s on SQLite and 167 s against 156 s on
PostgreSQL, one run each: some 5 to 8 % more for each write that changes
a kept group. A page in an indexed order under a filter that rejects the
rows it meets first reads past them, as a stored type's does: ascending
by the sum where half the groups hold more than the middle value took
220 ms on SQLite and 440 ms on PostgreSQL, holding 5 MB.

**Found at the build.** At an earlier position the memory engine handed
the merge each index row's state at that position, not at the head, so
a key first put after it dropped out of a batch. A batch shorter than it
asked for reads as the index's end, and a page ended early. It hands
the head's states, as the disk engines do, for stored types and kept
names alike.

**Not in it.** An aggregate other than a count and a pattern started over
a kept view read every group, a page of kept records at a time
(lakelands#731), and a view that computes is computed whole.

## Why

A walk ordered by a field is how an agent reads a type in the order it
cares about. Reading and sorting the whole type for every page made each
page cost the type: at 100 000 records, 0.2 to 0.6 seconds after the
sort was made cheap, and seconds before. The order is the rulebook's
mixed-kind order, which no SQL `ORDER BY` reproduces. A byte string that
compares as the rulebook does can be ordered by any storage, bytewise,
so every engine can keep it in an ordinary index.

The index is asked for, not kept for every field, because it is not
free: kept on every declared scalar field, it cost PostgreSQL about half
its durable ingest rate, SQLite about two fifths and memory a sixth,
while the fields an agent pages through in order are few. So the lake
chooses them.

It is asked for in a setting, not in the declaration, because it changes
no answer, only its price (040). What changes no answer is not the type's
shape: it does not belong in a logged declaration or in the schema hash
(007), and changing it needs no new log format. A setting is where a
lake keeps its own choices: it is in the log, per lake, with a history
an auditor can read, and the fold already reads settings. The store
checks the setting's fields when it is written, so a name that could
never be indexed is refused at the door rather than ignored.

The lake's own choice is the other half of that reason. A host's setting
is a decision an auditor should read; the lake's is a cache decision,
learned from reads, and logging it would put writes no caller made into
every feed, mirror and export, decided by read traffic. So it is kept in
the projection, where an engine already keeps what changes no answer, and
a rebuild from the log forgets it. It is built in chunks because a
setting's rebuild holds the writer for the whole type, which a host
chooses to pay in a quiet moment and the lake must not impose at a moment
no caller picked.

The index keeps only the kinds an agent sorts by, and only the start of
long values: two texts that differ only after 32 bytes are rare enough
that reading their run whole costs less than writing every long text
into two ordered indexes.

The mark makes a stale index unused rather than wrong. A lake written in
between by a binary that does not keep the index answers correctly at
once, more slowly, until a writer rebuilds. So does `chosen_through` for
the fields the lake chose: a writer's open drops them, and the lake
chooses again from use.

## What it is not

- **Not a second order.** `cmp_values` defines the order; `sort_key` is
  checked against it, never the other way round.
- **Not history.** The index describes the head. An earlier position
  reads it with the kept states of the keys changed since (above).
- **Not a declaration.** Nothing in a type says which fields are sorted;
  the setting does, and the log format does not change.
- **Not a seam method.** `Scan` already carries the order, `after` and
  `limit`, and the engine decides how to read them.

## How it came to be

- The first version: each engine keeps an order index of the head for
  every field a type declares, and reads it only while its mark says it
  describes the head, where a page in a field's order sorted the type
  (lakelands#137).
- The index keeps the scalar kinds only, and the first 32 bytes of a
  key (lakelands#137).
- The index is asked for per field, by the `sort_index.<type>` setting,
  where it was kept for every declared scalar field (lakelands#137).
- An earlier position reads the head index and the keys changed since,
  where each page after a write sorted the type (lakelands#382).
- A page under an exact text reads its page through the order index,
  where it read every holder and sorted them for each page; a page's
  further batches double, and on PostgreSQL the setting's write tells
  the planner of the index it rebuilt (lakelands#587).
- A kept view's order index is asked for by `sort_index.<view>`, where
  a view type was sorted whole; found at that build, the memory engine
  hands the merge the head's states at an earlier position
  (lakelands#668).
- What reads every group of a kept view reads it a page of kept records
  at a time (lakelands#731).
- `describe_type` names a view's indexed fields only where the view is
  kept, or asked to be and can be (lakelands#748).
- PostgreSQL reads the keys changed since by `records_since` alone,
  where it read two ranges as SQLite does (lakelands#537).
- A page read text first reads its first three batches each by itself
  and the holders left once at the fourth, where every batch read every
  holder again; what reading the text first costs was measured again,
  with the type's size in it (lakelands#695).
- Corrected at the rewrite (lakelands#817): the scalar kinds the store
  takes name `date`, which the setting's check takes and this record's
  list left out.
- The order it cites stands in 025, where the rules of 029 moved
  (lakelands#817).
- The lake chooses an order index itself from the pages it sorted whole,
  keeps the choice beside its log and builds it in chunks beside the
  writer, where only the host's setting asked for one; a host's setting
  wins (lakelands#1131).
