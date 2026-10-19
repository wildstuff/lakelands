# 052 — A field may be computed at the read

## What is decided

### A field derived from an expression, or computed by an `expr` function, says when it is computed with `computed_at`

A field derived from an expression, or computed by an `expr` function,
says when it is computed with `computed_at`: `write`, by the fold at each
write that changes its inputs, its value stored; or `read`, at every
read, its value never stored. Left out, it is computed at the write.

### The clock is said

A derived expression or an `expr` function that reads the clock
(`now()`, `today()`) and says no `computed_at` is refused where it is
declared, in one sentence that names both: `computed_at: read` for a
value of the day it is read, `computed_at: write` for the instant of its
last write. A stamp such as `stamped: now()` is declared by saying
`write`. The fold does not judge declarations, so a log that holds such
a field folds as before.

Not checked: an `expr` function declared again to read the clock under a
field that stands and says no `computed_at`; the field keeps the value
of its last write.

### One rule: the field means its expression, written in the read

A field computed at the read is computed for every record a read loads
from an engine, before the read uses the record: in the order the fold
computes derived fields, over the record as stored, with the lake's
settings and the read's clock, the one a `where` reads (below). So
`get`, a `where` that names the field or its expression, a group and a
measure of an aggregate or of a view, a pattern's start and rows and a
search's hits agree in every read. A `where` that names it is
evaluated by the store, a page at a time, as one that reads the clock
is. A value that cannot be computed is none.

### The read's clock is the instant or the position it names

A read has one clock, which `now()` and `today()` read wherever the read
evaluates them: a `where`, a field computed at the read, a group key, a
measure, a view's `where` and measures, a pattern's starts and rows,
and a search's `where`. It is decided once, when the read opens:
- a read `valid_at` an instant reads that instant, at the head and as of
  a position: a read of what was true at a business date counts its
  days to that date;
- else a read `as_of` a position reads the `known_at` of the event
  there, the clock the fold read that event under (017), so a read as of
  a position repeats;
- else it reads the wall clock.

A cursor carries the clock its first page read, and every page it
continues reads it, at the head too. A cursor handed out without one
reads by the rule above. A cursor's clock is the caller's to keep, as
`valid_at` is: a cursor is not sealed, and one that names another clock
reads no more than a read `valid_at` that instant would.

The time a position was written is not withheld, as the position is
not: a read as of a position tells the `known_at` of the event there,
also when that event is a write the reader is not shown.

A host may give a read its clock (`ReadAt::clock`), which no wire
carries, so no caller names it. `valid_at` comes before it. An MCP
session's pin of the head keeps the instant it was set at as the clock
of every read it pins: an agent that pins at the start of its turn is
answered as of that turn, not as of the last write, and its pinned reads
repeat within the session. A pin of a named position reads by the rule
above.

A listing of proposals judges what a proposed write reaches under the
host's clock, else the clock of the position it reads, never under
`valid_at`: the `known_at` of the event there as of a position, else
the wall clock. A pin's clock is the one a read at the head had when
the pin was set, so a listing under it shows no more than that read.
Whether a write reaches a record the reviewer is not shown decides
whether it is listed, so a clock the reviewer may name would let it
move the clock until the listing changes, and learn to the microsecond
when a value it may not read lets a hidden record in.

### Nothing holds the value of a field computed at the read

No event, so no feed and no export, holds its value, and it carries no
provenance: no write wrote it. A reader is answered it as what it is
computed from
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`),
and `describe` shows the member and the completed `depends_on`.

### What reads stored records does not name a field computed at the read

An engine, an index, a walk over links, the fold and a kept view read
records as stored, and would read it as absent. So it has the authority
`derived` (no door writes it), an expression computes it (a function's
backing is called at a write), it reads no `previous` or `changed`, and
it is not `required`, `sensitive` by itself, cleared by
`on_input_change` or searchable; it is no key, validity bound, `via` or
derived relation's match or `where` on either side, no `reference` (nothing would
judge the record it names for a writer, 032), no field of a view (a view
is computed at the read already) or of a link; no field computed at the
write reads it; a trigger, a verb and an object set that read it are
refused; a view that reads it is not kept; an order or a vector index
does not name it; and a grant's `only` on it cannot be applied, as on
any field an engine cannot filter by. A type declared again so that a
field a trigger, a verb, an object set or another type's derived
relation in force reads is computed at the read is refused, naming the
field alone. Each is refused as invalid where it is declared, naming the
field; a function declared again with a backing while such a field
calls it is refused naming the function alone; a verb's payload field
and a function's input, which no record holds, say no `computed_at` at
all.

### `order_by` a field computed at the read is refused

`order_by` a field computed at the read is refused, in `find` and in a
pattern: its value moves with the clock, so the order one page is read
in would not hold to the next. Since a cursor carries its clock, that
holds no longer for a cursor; it still holds for `after`, which carries
no clock, and for a cursor handed out before. Whether the refusal stays
is asked in lakelands#1189. The refusal comes after the reader is known
to read the field: one kept from the field, or from its type, is
told nothing of it.

### A declaration that changes `computed_at` computes the type's records again

A declaration that changes `computed_at` computes the type's records
again: one taken to the read holds no stored value from then on, one
taken back to the write is computed for every live record. Saying
`write` where nothing was said computes nothing again: a stamp keeps the
instant it was written.

### `computed_at` is not hashed

It is not hashed, as `derived` is not: it says how a value is computed,
not how a record is stored.

## Why

A host asked for a field of a record, `overdue`, true of an invoice past
its due date. A view answers another question, about groups. A stored
derived field that reads the clock is frozen at the last write that
touched its inputs: `overdue` stays false for an invoice nobody touches
after it falls due. Documenting that leaves the trap, and the next to
fall in is an agent that declares `overdue`; so a stored field that reads
the clock says so, or is refused.

The field and a `where` read one clock in one read: a reader that sees
`overdue` true from `get` and finds nothing with `where overdue` could
trust neither. The value is computed on the record, not its expression
put in the field's place, because a group's key and an aggregate's
argument are field names: one computation serves them all.

Where something reads the record as stored, the field is not there; the
refusals keep every such reader from meeting a field that is always
absent for it.

## How it came to be

- The first version: a field may be computed at the read, with the
  refusals that keep a reader of stored records from it (lakelands#550).
- A `reference` is no field computed at the read: nothing would judge
  the record it names. A function declared again with a backing is
  refused while a field computed at the read calls it, where only the
  field's own declaration was asked (lakelands#795).
- The read's clock was the wall clock, also as of a position, so a read
  as of a position did not repeat and a read of a business date counted
  days to the day it was made. It is the instant `valid_at` names, else
  the `known_at` of the position read, and a cursor carries it, as a
  pin of the head carries the instant it was set at; a listing of
  proposals judges under the position's clock or the host's, never
  `valid_at` (lakelands#571).
