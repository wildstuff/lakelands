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
