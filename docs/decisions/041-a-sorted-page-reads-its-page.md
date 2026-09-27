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
- **The ceiling.** A view at an earlier position, an order by `known_at`
  or `seen_at`, an order by a field the lake did not ask an index for,
  and a view type are sorted whole by `sort_key` as before. So is a
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
- **Not history.** The index describes the head. Earlier positions sort,
  as the other head indexes leave earlier positions to the kept states.
- **Not a declaration.** Nothing in a type says which fields are sorted;
  the setting does, and the log format does not change.
- **Not a seam method.** `Scan` already carries the order, `after` and
  `limit`, and the engine decides how to read them.
