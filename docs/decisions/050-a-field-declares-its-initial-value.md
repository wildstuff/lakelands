# 050 — A field declares its initial value

## What is decided

### An authored field of a type's own may declare `initial`

An authored field of a type's own may declare `initial`: a literal the
field takes when its record comes to exist and the write that made it
gives the field none.

### The fold fills the field

Where a record's write is folded, after its patch is merged and before
what is computed is computed: when no live record stood under the key
before the event (never written, or retired), each field whose
declaration in force declares `initial` and holds no value takes it. It
is the record's from its first version: a derived or computed field
reads it there, since a create computes every such field. The door and
the fold call one function, so a door's `required` check passes on what
the fold will write, and a rebuild, a catch-up, an import and a read as
of a position answer the same. The event stays as the door wrote it: a
source's content id and the duplicate test read the patch and never see
the fill. The filled field is not counted among those the event changed:
at a make every derived field is computed anyway, and no field holds a
value an input change could clear, so nothing would read that count.

### A record is filled once

A record that is re-keyed is the same record and is not filled again. A
later declaration with another `initial`, or none, changes no stored
record; an author's clear clears, refused where the field is required. A
record retired and written again starts again: it holds the initial
value at its next version, and a trigger sees a create.

### The filled value has its own origin

The filled value's provenance says origin `initial`, with the creating
event's position, time and writer, and no source, observation time,
upstream or procedure version: the value came from the declaration, not
from what the event carried. It is a field's provenance alone: an
event's envelope never carries it, and a log whose event does is refused
at that event.

### A notification tells the fill

A change notification tells the event that made a record as a change of
each field the fold filled, beside those its patch names, to a principal
that is shown the field's `initial`. Which fields declare one is read
from the principal's own description, the one the watch reads already
for what a type shows it, so a create costs no read more. One who reads
the field masked is not shown its `initial`, and telling it of the field
by the fill would tell it that the field declares one; it is told of the
field by a patch that names it.

### `initial` is declared only on an authored field of a type's own

A field whose authority is `source`, `shared` or `derived`; one that is
derived, computed or an embedding; a key field; a reference, and a
relation's `via` field; a field cleared when an input changes; a field
of a view: each refuses `initial` as invalid, naming the field. A
relation's link field, a verb's payload field and a function's input
refuse it by name. The value is checked as a write of the field is and
kept in the one form that write would store, so `"1.5"` and `"1.50"` on
a decimal of scale 2 are one declaration. A value type tightened so that
a field's `initial` no longer fits is not applied: the `initial` is a
misfit of its field, told as a stored misfit is, by its count alone to
one who does not read the field.

### `initial` makes no owner

A purge passes over a field that declares `initial`, whoever wrote it
last: the record goes with the source that brought it, with the state an
author moved. The report says in `moved`, per type and field, how many
such records an author had written and the first ten of them.

### `initial` is not hashed

The schema hash leaves it out, so declaring, changing or removing an
`initial` leaves a type's hash as it was. A host that decides whether to
send a declaration again compares the declaration, which `describe`
answers it whole, to learn of a changed `initial`. A proposed migration
whose `to` would bring back an `initial` changed since it was proposed
is refused at its accept by what it restates.

## Why

A type a source feeds often holds a field the source knows nothing of: a
state the lake's own users move, which every record must have. Declared
required, no door can make the record: the source's write may not carry
an authored field, and an author's write comes after the record exists.
Declared optional, every reader has to treat a missing value as the
first state, and every host writes it a second time after each ingest.

The value could be written by the door into the creating event, or by
an event of its own. In the first, the source's write would carry a
field it does not own, and the duplicate test, which reads the patch,
would see a different write for the same content. In the second, the
record would exist for one position without the value, at version 1,
and its state would come at version 2. Filling it where the fold applies
a write keeps the event as the door wrote it and makes the value part
of what the record is at its first version, wherever that version is
read or rebuilt.

The origin is its own because the lake already writes authored fields
itself: aging moves a lifecycle field with origin `system`. With the
same word, a reader could not tell the value a record started with from
a value the lake moved since.

A record's state that an author moved is not an author's claim on the
record. The field is there because the record is, and the record is
there because the source brought it: when the source is taken back, the
state has nothing left to be the state of. Every other authored field
still co-owns, so a purge that would take what a person wrote beside the
state is refused whole, as before.

The schema hash leaves it out because the hash is given to every
principal shown the type, and a principal who reads the field masked is
shown everything else the hash covers. Were `initial` hashed, such a
principal could rebuild the declaration and try values until the hash
matched: over a `one_of`, a short loop to the value it is not shown. The
hash then moves only when how a stored record is stored or validated
changes, as `docs/decisions/007-schema-hash-covers-shape.md` has it, and
`initial` does neither: it acts once, when a record is made.

## What it is not

- It is not a default for a write. A write that names the field with
  `null` on an existing record clears it; the fill is at creation alone.
- It is not a migration. Adding, changing or removing `initial` is a
  declaration and fills no record that exists; a field made required
  with `initial` over records without a value is as lossy as without
  it.
- It is not an expression. The value is a literal; a value that depends
  on the record is a derived field.
- It is not on a source's field. A delivered value equal to the initial
  one would be taken for a duplicate, and the source never seen as its
  owner.

## How it came to be

- The record first had the schema hash cover `initial`, so that a host
  deciding by the hash alone would send a changed `initial`.
- That was withdrawn: the hash leaves `initial` out, and the host
  compares the declaration (lakelands#551).
