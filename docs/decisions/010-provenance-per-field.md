# 010 — Provenance is per field, and the fold materialises it

## What is decided

### Every field carries its own provenance, and the fold writes it

Every field of every record carries its own provenance:

- `origin`: which authority wrote the value (a source feed, an author,
  a computation);
- `known_at`: when the lake learned the value, the wall time of the
  event that wrote it;
- `seen_at`: when the source observed it, for values that came from a
  source;
- `position`: the exact log coordinate of that event;
- `writer`: the principal that wrote it;
- `source`: where it came from;
- `procedure_version`: for a value a procedure produced, which version
  of the procedure produced it.

The values come from the event, so they are part of the log format. The
fold writes them beside the value every time it changes a field, and a
replay reproduces them byte-identically. A reader gets them with the
record in one call, without a second query: on every door the switch
that returns each field's provenance with its value is spelled
`with_meta` (a boolean on `find`, `get` and `search`).

A field computed at the read carries none: no event holds its value,
and no write wrote it
(`docs/decisions/052-a-field-may-be-computed-at-the-read.md`).

### A field whose backing failed says why in its provenance

A field computed by a function whose backing failed holds no value.
Its provenance still says why: the metadata of the event that cleared
it, with `error` set to the reason. A read with `with_meta` returns
that metadata for the field even though the field has no value, so a
caller can tell "not computed yet" (no metadata) from "computed and
failed" (`error`). A field that holds a value never carries `error`.

### A value read from another lake carries `upstream`

A value a source read from another lake's log carries `upstream`
(`docs/log-format.md` §1.1): that lake and the position of the event
there whose state it is, beside `seen_at`. A mirror states it for every
field it delivers, so a reader can follow a value back to the log it
came from. It is provenance the writer states, and it has the same
standing as `seen_at`: nothing checks it against the other lake. A
later write of the field without it leaves the field without it
(`docs/log-format.md` §2.3).

### A value no writer chose has the origin `initial`

A field that declares `initial` is filled by the fold when its record
is made without a value for it
(`docs/decisions/050-a-field-declares-its-initial-value.md`). Its
`origin` is `initial`, a fifth member of the closed set: not `system`,
which an aging move writes on the same lifecycle fields, so a reader
tells the value a record started with from one the lake moved since.
It takes the creating event's `known_at`, `position` and `writer`, and
nothing of a source: no `source`, `seen_at`, `upstream` or
`procedure_version`, since the value came from the declaration.

### A record that is judged is judged with its provenance

`seen_at(f)` and `known_at(f)` read a field's provenance wherever a
record is judged, and they read the same there as in a read's `where`:
in an object set's filter, at a write and where the set is declared over
records that stand; in a derived field; in a trigger's guard; in a
verb's `requires` and in the values it assigns; in an aging rule's
guard; in a view's `where` and in what its measures aggregate, a source
record at a time; in a `find`'s group key. One expression has one
answer
(`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`):
a set whose filter is `seen_at(status) >= '2026-03-01'` takes the
records a `find` under that filter finds.

So the scope an expression is evaluated over carries the record's
provenance, and a read and a write build the one scope. What each reads
is what a read after the write is answered:

- **A derived field** reads the provenance its event leaves: of a field
  the event writes, the event's; of a field it leaves alone, what the
  field had. The fold computes it, so a rebuild and a derivation
  declared over records that stand compute the same.
- **A field the fold changes takes the event's provenance as it
  changes:** a derived field it computes, a field it clears, a backed
  field it settles. A derived field that reads the provenance of such a
  field is computed after it and reads that event's instant, whichever
  of the two was declared first: a read of a field's provenance orders
  the fold's pass as a read of its value does. Stamped after the pass,
  the reader would hold the instant before the event's until the next
  event on the record, and a `find` under the same expression would
  answer otherwise.
- **A type that keeps no per-field provenance** (`field_meta: false`)
  has none to read, at a write as at a read.

Where no record stands there is no provenance to read, and the two
functions are refused
(`docs/decisions/028-authoring-is-strict-and-whole.md`).

## Why

A record is rarely written by one hand. An invoice's amount comes from
an import, its dispute flag from a person, its risk score from a
computation. Provenance per record would say only who touched the
record last, which is exactly the wrong answer for every other field.
An agent that acts on a value needs to know where that value came from
and how old it is.

Materialising it in the fold, rather than reconstructing it at read
time, keeps reads cheap and keeps the answer identical on every engine:
the fold is pure, so the metadata is as reproducible as the value.

## What it is not

- It is not record-level provenance with field detail looked up
  elsewhere. A reader never has to join a side table to learn where a
  field's value came from.
- It is not business time. A record's `valid_from` and `valid_to` say
  when a fact held in the world; `known_at` and `seen_at` say when the
  lake and its source learned it.
- It is not free for every shape. Its cost follows the fields a write
  changes, not the width of the record, and a high-volume type may opt
  out explicitly; the default is on.

## How it came to be

- The first version: seven members of provenance on every field,
  written by the fold and read with the record through a switch it
  called `with: meta`.
- The switch is spelled `with_meta` on every door.
- The provenance of a computed field whose backing failed carries
  `error`.
- A value read from another lake carries `upstream`; the member came
  with a step of the log format (lakelands#174).
- A value no writer chose has the origin `initial` (lakelands#551).
- Corrected at the rewrite (lakelands#817): a field computed at the
  read carries no provenance, which 052 added and "every field of every
  record" left out; the suite holds it.
- `seen_at` and `known_at`, read by a read's `where`, a view's `where`
  over a stored type and a trigger's guard alone and as nothing
  everywhere else, are read wherever a record is judged, and a field
  the fold changes is stamped as it changes, inside the pass
  (lakelands#852).
