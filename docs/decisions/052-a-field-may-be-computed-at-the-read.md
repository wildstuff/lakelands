# 052 — A field may be computed at the read

## What is decided

A field derived from an expression, or computed by an `expr` function,
says when it is computed with `computed_at`: `write`, by the fold at each
write that changes its inputs, its value stored; or `read`, at every
read, its value never stored. Left out, it is computed at the write.

- **The clock is said.** A derived expression or an `expr` function that
  reads the clock (`now()`, `today()`) and says no `computed_at` is
  refused where it is declared, in one sentence that names both:
  `computed_at: read` for a value of the day it is read, `computed_at:
  write` for the instant of its last write. A stamp such as `stamped:
  now()` is declared by saying `write`. The fold does not judge
  declarations, so a log that holds such a field folds as before.
- **One rule: the field means its expression, written in the read.** A
  field computed at the read is computed for every record a read loads
  from an engine, before the read uses the record: in the order the fold
  computes derived fields, over the record as stored, with the lake's
  settings and the read's clock, the one a `where` reads (the wall clock,
  also in a read as of a position). So `get`, a `where` that names the
  field or its expression, a group and a measure of an aggregate or of a
  view, a pattern's start and rows and a search's hits agree in every
  read. A `where` that names it is evaluated by the store, a page at a
  time, as one that reads the clock is. A value that cannot be computed
  is none.
- **Nothing holds it.** No event, so no feed and no export, holds its
  value, and it carries no provenance: no write wrote it. A reader is
  answered it as what it is computed from
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`),
  and `describe` shows the member and the completed `depends_on`.
- **What reads stored records does not name it.** An engine, an index, a
  walk over links, the fold and a kept view read records as stored, and
  would read it as absent. So it has the authority `derived` (no door
  writes it), an expression computes it (a function's backing is called
  at a write), it reads no `previous` or `changed`, and it is not
  `required`, `sensitive` by itself, cleared by `on_input_change` or
  searchable; it is no key, validity bound, `via` or derived relation's
  match on either side, no field of a view (a view is computed at the
  read already) or of a link; no field computed at the write reads it; a
  trigger, a verb and an object set that read it are refused; a view that
  reads it is not kept; an order or a vector index does not name it; and
  a grant's `only` on it cannot be applied, as on any field an engine
  cannot filter by. A type declared again so that a field a trigger, a
  verb, an object set or another type's derived relation in force reads
  is computed at the read is refused, naming the field alone. Each is
  refused as invalid where it is declared, naming the field; a verb's
  payload field and a function's input, which no record holds, say no
  `computed_at` at all.
- **No order by it.** `order_by` a field computed at the read is
  refused, in `find` and in a pattern: its value moves with the clock,
  so the order one page is read in would not hold to the next. The
  refusal comes after the reader is known to read the field: one kept
  from the field, or from its type, is told nothing of it.
- **Declared again, it moves.** A declaration that changes `computed_at`
  computes the type's records again: one taken to the read holds no
  stored value from then on, one taken back to the write is computed for
  every live record. Saying `write` where nothing was said computes
  nothing again: a stamp keeps the instant it was written.
- **It is not hashed,** as `derived` is not: it says how a value is
  computed, not how a record is stored.

Not checked: an `expr` function declared again to read the clock under a
field that stands and says no `computed_at`; the field keeps the value
of its last write.

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
