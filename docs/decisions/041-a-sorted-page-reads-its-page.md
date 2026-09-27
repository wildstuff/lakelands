# 041 — A sorted page reads its page

## What is decided

A `find` page ordered by a field reads about `limit` rows at the head,
as a page ordered by key does, on every engine. Each engine keeps an
**order index** of the head for it, and the page is read in the index's
order and stops when it is full.

- **The key.** Every value in the index is its `sort_key`
  (`lakelands_core::engines`): bytes whose order is the one order across
  kinds (029), and whose equality is that order's equality, so `1` and
  `"1.00"`, or one instant written in two offsets, share a key. The order
  is still defined by `cmp_values`. A property test holds the key to it,
  and the key never decides an order the rulebook does not.
- **The rows.** One row per field the type's declaration at the head
  declares with a scalar kind (text, number, decimal, boolean, date-time
  or reference) and the record's latest state holds with a value,
  retired and restricted states included. JSON values, vectors,
  geometries and bytes get no rows, and an order by one of them sorts.
  A reader decides from the declaration at the head which orders the
  index answers, never from which rows exist. A redeclaration rewrites
  the rows of the type's records under it, as it does the other head
  indexes.
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
- **Only while it describes the head.** The fold marks the index as
  kept through the position it commits (`sorted` beside `folded`). A
  view uses the index only when the two are equal. Otherwise it sorts
  the type as before, and the answer is the same. A writer's open
  rebuilds an index that does not describe the head, once.
- **The ceiling.** A view at an earlier position, an order by `known_at`
  or `seen_at`, an order by a field the declaration does not name, and a
  view type are sorted whole by `sort_key` as before. So is a multi-field
  page whose first field has few values: its runs are long. The seams
  page gives the numbers.

## Why

A walk ordered by a field is how an agent reads a type in the order it
cares about. Reading and sorting the whole type for every page made each
page cost the type: at 100 000 records, 0.2 to 0.6 seconds after the
sort was made cheap, and seconds before. The order is the rulebook's
mixed-kind order, which no SQL `ORDER BY` reproduces. A byte string that
compares as the rulebook does can be ordered by any storage, bytewise,
so every engine can keep it in an ordinary index.

The index covers declared fields because the store only orders `find`
by declared fields. A flag naming the sortable fields would change what
a logged declaration holds, and would leave the other orders as slow as
before. The index costs every write a row per indexed field, so it keeps
only the kinds an agent sorts by, and only the start of long values:
two texts that differ only after 32 bytes are rare enough that reading
their run whole costs less than writing every long text into two
ordered indexes.

The marker makes a stale index unused rather than wrong. A lake made
before the index, or written in between by a binary that does not keep
it, answers correctly at once, more slowly, until a writer rebuilds.

## What it is not

- **Not a second order.** `cmp_values` defines the order; `sort_key` is
  checked against it, never the other way round.
- **Not history.** The index describes the head. Earlier positions sort,
  as the other head indexes leave earlier positions to the kept states.
- **Not a declaration.** Nothing in a type says which fields are sorted;
  the log format does not change.
- **Not a seam method.** `Scan` already carries the order, `after` and
  `limit`, and the engine decides how to read them.
