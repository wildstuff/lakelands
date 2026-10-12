# 041 — A sorted page reads its page

## What is decided

A `find` page ordered by a field the lake asks an index for reads about
`limit` rows at the head, as a page ordered by key does, on every
engine. Each engine keeps an **order index** of the head for the fields
asked for, and the page is read in the index's order and stops when it
is full. A field not asked for is ordered by sorting the type, as
before.

- **The key.** Every value in the index is its `sort_key`
  (`lakelands_core::engines`): bytes whose order is the one order across
  kinds (029), and whose equality is that order's equality, so `1` and
  `"1.00"`, or one instant written in two offsets, share a key. The order
  is still defined by `cmp_values`. A property test holds the key to it,
  and the key never decides an order the rulebook does not.
- **Asked for per field.** The `sys.setting` record `sort_index.<type>`
  holds a JSON array of field names and asks for the index on them.
  The store takes only fields the type declares with a scalar kind
  (text, number, decimal, boolean, date-time or reference) and refuses
  a write of the setting naming any other, or a type it does not
  declare, as invalid, naming the field. Retiring the setting, or an
  empty array, asks for no index.
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
  Otherwise it sorts the type as before, and the answer is the same. A
  writer's open rebuilds, once, every index that does not describe the
  head: one whose mark a writer without the index left behind, and one
  whose setting such a writer wrote.
- **The ceiling.** An order by `known_at` or `seen_at`, an order by a
  field the lake did not ask an index for, and a view type are sorted
  whole by `sort_key` as before, and so is a view at an earlier position
  whose type changed too much since (below). So is a
  multi-field page whose first field has few values: its runs are long.
  `describe_type` names a type's indexed fields in its lifecycle
  (`sorted`), so an agent can see which orders are cheap. The seams page
  gives the numbers, and what an index costs a write.

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

The index keeps only the kinds an agent sorts by, and only the start of
long values: two texts that differ only after 32 bytes are rare enough
that reading their run whole costs less than writing every long text
into two ordered indexes.

The mark makes a stale index unused rather than wrong. A lake written in
between by a binary that does not keep the index answers correctly at
once, more slowly, until a writer rebuilds.

## What it is not

- **Not a second order.** `cmp_values` defines the order; `sort_key` is
  checked against it, never the other way round.
- **Not history.** The index describes the head. An earlier position
  reads it with the kept states of the keys changed since (below).
- **Not a declaration.** Nothing in a type says which fields are sorted;
  the setting does, and the log format does not change.
- **Not a seam method.** `Scan` already carries the order, `after` and
  `limit`, and the engine decides how to read them.

## Extended: an earlier position reads the head index and the keys changed since

A cursor keeps a walk at its first page's position, so its pages stay
one answer while the lake is written. Each page after a write was a view
at an earlier position, and sorted the type: the write the cursor keeps
out of the walk made every later page cost the type.

**What is decided.** A page at a position `p` before the head, ordered
first by a field the head's index answers (the index kept through the
head, the field named by the head's setting), reads that index too:

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
  and the states. SQLite and PostgreSQL read `C` by two range reads,
  `records_since (ty, since)` for the states put and `records_at (ty,
  until, key)` for the states that ended; a `UNION` of the two is planned
  by SQLite as a merge in key order, which reads the type. The memory
  engine keeps, per type, the position and key of every state put.
- **The ceiling.** More than `min(10 × limit, 4 096)` states put since
  `p`, or keys in `C`, and the page sorts the type as before. Every
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

**Why.** The head index still describes every key that has not changed
since `p`, which in a walk is nearly all of them. Reading it and only the
few that changed keeps a pinned page at about its page, as the head's
is. The streaming check holds a walk with writes between its pages to
its page, the one row more the store asks for, three more and two per
changed key; before, each page after a write read the type.

## Extended: a page under an exact text reads its page

A page at the head ordered first by an indexed field, whose scan must
hold a text exactly (a string in `equal`, or the filter's exact-text
term), read every record holding the text and sorted them, for each
page, whether the lake indexed the field or not. A walk of such pages
cost the holders once per page: a square for a host that pages through
a sorted, filtered type.

**What is decided.** At the head, SQLite and PostgreSQL read such a page
through the order index as a page with no text is read, and hand the
rulebook only the records holding the text. There are two ways to find
them, and each is cheap where the other is not:

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
  index. At most 4 096, and the page reads the text first: at most 4 096
  index rows, about 2 ms. More, and the order index first: about
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
  about `log₂ R + R / 1 024` batches. The text first needs it most:
  each of its batches reads every holder again. Before the doubling, a
  page of one under `v = 'at' and w > 40000`, 4 096 holders of 41 000
  records, asked for a batch for every two rows it passed over. On
  SQLite, the median of five, `w` indexed or not:

  | text (holders) | way | limit | not indexed | one row more a batch | doubling |
  |---|---|---|---|---|---|
  | `at` (4 096) | text first | 1 | 12.6 ms | 9.3 s, 3 597 batches | 50 ms, 14 |
  | `at` (4 096) | text first | 10 | 11.6 ms | 964 ms, 361 | 43 ms, 11 |
  | `past` (4 097) | order first | 1 | 12.1 ms | 137 ms, 3 598 | 34 ms, 14 |
  | `past` (4 097) | order first | 10 | 11.1 ms | 44 ms, 361 | 34 ms, 11 |

  Such a page still costs a few times the sort of every holder: the
  text first reads its holders once a batch, and the order first every
  row before the first it admits. Reading the holders' order rows once a
  page would cut the first to one read; it is not done here.
- **The records without the field** come last, as before. They are read
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
the memory engine read as before. The memory engine keeps no exact-text
index, and reads the order index's rows in its order and admits each.

**The cost.** 100 000 records, a page of 50 in an indexed field's order
under a text, then a walk of every holder by cursor, 1 000 a page. The
first page's median of five, then the walk. Before, the page read every
holder and sorted them, as it still does where the field is not
indexed. The other slots were building on the same machine (load about
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

**Why.** The order index already answers a sorted page in its page. A
text the page must hold either thins its rows, which the order index
walks past cheaply, or picks out a few, which the text index names
directly. Counting the few, up to a ceiling, is a short range read, and
it tells the two apart without reading the type.

## Extended: a kept view's order index

A kept view's records are records of its kept name, `sys:kept.<view>`
(049). A kept name has no declaration, and no setting could name it, so
a page of a kept view in another order than its key read every kept
record and sorted them. At 100 000 groups that took about 400 ms on
SQLite and 500 ms on PostgreSQL, and held some 465 MB, more than
computing the view holds. A host asks for an order index of a kept view
as it does for a stored type's (lakelands#668).

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
  (`restricted`, `only`) as before, and at an earlier position the engine
  reads the head's index and the keys changed since, as above.
  `describe_type` names a view's indexed fields in its lifecycle where
  the view is kept, or asked to be and can be, as its `kept` tells that
  reader (lakelands#748). A view that computes for want of `view_kept`,
  or cannot be kept, has no records to index, and to a holder of `only`
  an index would say whether the view is kept, which 051 keeps from it.

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
asked for reads as the index's end, and a page ended early. It now
hands the head's states, as the disk engines do, for stored types and
kept names alike.

**Not in it.** An aggregate other than a count and a pattern started over
a kept view read every group, a page of kept records at a time
(lakelands#731), and a view that computes is computed whole.
