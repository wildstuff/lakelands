# 084 — A field holds the writer that made its record

## What is decided

Decided on lakelands#1488, for 0.20.0, in format 20. It extends
`docs/decisions/050-a-field-declares-its-initial-value.md`, whose fill
at a create fills this field beside the `initial` values, and
`docs/decisions/010-provenance-per-field.md`, whose provenance of a
filled field names the writer it holds.

### A field of `authority: writer` holds its record's maker

A field of a type's own stored records may declare `authority: writer`
(lakelands#1488): the store fills it with the writer of the event that
made the record, as `<kind>:<id>`, the form an event records its writer
in. It is a plain `text` field, no key field, declares no `initial`, and
is neither derived, computed by a function or an embedding, nor
dependent on another field or cleared by one. A field of a view, of a
relation's links, of a verb's payload or of a function's input is
refused (`writer_not_of_type`), as is one of another kind or with a
value type (`writer_not_text`), a key field (`writer_key`) and a
computed or cleared one (`writer_computed`); an `initial` on it is
refused as on any field that is not authored (`initial_not_authored`).

### The fold fills it at a create, through the function the door shares

Where a record's write is folded and no live record stood under the key
before the event (never written, or retired), each such field takes the
event's writer, beside the `initial` values a create fills (050). The
door judges the record the same function leaves, so a required field of
this authority passes `required` at every door, and a rebuild, a
catch-up, an import and a read as of a position answer the same. Its
provenance (010) is the store's own hand, `origin: system`, at the
creating event's position, naming the writer it holds; no source,
upstream or procedure. A later write keeps the value, a re-key moves it
with its record, and a record retired and written again takes the
writer of the write that makes it again.

The writer is the event's, so it is who the log says wrote the record:
an accepted proposal's writer is its proposer, a reaction's or a
trigger's write is filled with the writer its event records, and a
mirror's copy in its target lake with the principal the mirror writes
as. A mirror lets such a field stand in a target type: nothing crosses
into it.

### No door writes it

A write that carries the field is refused at every door, naming the
field, with `wrong_door`: an upsert, an ingest, a verb's `set`, a
proposed write when it is made and when it is accepted, a change to the
records a condition selects (081), and so a query language's write,
which reaches the store through those doors only (082). An aging rule
may not move it (`aging_authority`). Its value is never a writer's
claim: what the field holds is what the log says.

### A type that comes to have one is migrated

Adding such a field to a stored type, or changing a field to this
authority, needs a migration (`field_writer`), and the schema hash names
the authority of such a field, of no other, so every other type keeps
the hash it had. Each live record takes, at the `migrated` event, the
writer its own provenance names for a field written at the position
that made it; what the field held before is dropped, as the preflight's
`dropped` counts it. A record that tells no such writer — every field
written again since, or a type that keeps no per-field provenance —
takes the store's own writer, `system:lakelands`. A retired record is
left as it is and takes a writer when it is written again. The filled
value's provenance is the store's at the `migrated` event, naming the
writer it holds. A field changed from this authority to another keeps
its values and is declared.

### A create holds an `only` pair on it when the value is the creator's

Whether a create by a principal can hold a field at a value is asked of
one function: an authored or shared field it writes, a field only verbs
write whose `initial` is the value, or a field of this authority whose
value is the principal's own writer. `describe` asks it for each pair a
holder's `only` names, and shows the field's authority, offering no
write of it.

## Why

An agent that shares a lake needs to know whose record it reads, and a
policy that keeps an agent to its own records needs a field it can
trust to say so. An authored field holding the writer would hold what
any writer sends; provenance holds the truth but per field and only
until a field is written again, and it cannot be the subject of a
grant's `only`, a constraint or a view's grouping. A field the store
fills from the log is both: a value of the record, and one no writer can
forge.

Filling it at the fold, in the function the door shares with it, keeps
the one rule `initial` already follows: the event stays as its writer
sent it, and the record every reader sees is computed from the log
alone, on every engine and at every replay.

A migration fills what the record tells because the fold applies a
`migrated` event to a record without reading the log again: the
position that made the record and the provenance of a field written
there are on the record, so the writer of that write is the best answer
the record holds. Where it holds none, the store's own writer says
plainly that the maker is not known; the migration's author did not make
the record, and its id there would be a claim the log does not support.

## What it is not

- Not the last writer. A record keeps the writer that made it; who wrote
  a field last is its provenance.
- Not a writer's choice. No door writes the field, an accepted proposal's
  writer is not its acceptor, and no option names another writer.
- Not a hidden field. It is read and granted as any other field; a
  grant decides who reads it.
- Not a change of the event. The writer is filled, never logged: a
  source's content id and the duplicate test read the patch alone.

