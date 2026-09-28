# 016 — Two write doors, by origin

## What is decided

A lake is written through two doors, and they stay two.

`ingest` is how a source feeds the lake. It takes a stream of records for
one source and chunks it into group-commit windows, answering one
acknowledgement per window with the position it is durable at. It writes
only fields whose authority is `source` or `shared`. It is content-addressed: an
event's id is a hash of the source, the type, the key and the values, so
a record delivered twice appends nothing the second time.

`upsert` (and every verb) is how an author writes. It takes one record's
sparse patch under a principal's grants, writes only authored fields,
and accepts an optional idempotency key: a retry with the same key from
the same principal answers the first outcome and appends nothing. The
key is recorded in the log.

The source's own offset belongs to the caller; the lake's position
belongs to the store. The two are never mixed.

## Why

The two doors are where the field-authority line runs. A re-import must
never overwrite a verdict a person wrote, and an author must never
overwrite a figure the source is the authority on. Keeping the doors
separate makes that line visible in every call: the name of the door
says which side of it the write is on, and the store checks the fields
against it.

Idempotency follows from origin too. A source redelivers the same
content, so content is the right key. An author may write the same
content twice on purpose, so an author's retry needs a key of its own,
chosen by the caller.

Taking a stream from the first signature means a real streaming source
is just another caller later, not a change to the contract.

## What it is not

- It is not two stores. Both doors append to the same log and are folded
  by the same rules.
- `ingest` offers no exactly-once promise and needs none: at-least-once
  delivery of the same content is a no-op.
- A `shared` field may be written through either door; the later
  position wins.

## Extended: windows, duplicates and refusals

`ingest` takes its stream in group-commit windows: a window closes when it
holds the lake's `commit_window_events` records or 2 ms after its first,
and is decided whole. A record whose content id is already in the log, or
earlier in its window, appends nothing; so does a record that would change
no field — a re-import of unchanged data appends nothing — and both count
as duplicates. The content id includes the record's version before the
event, so a value that returns after a change is written again. A window
with a refused record appends nothing; the refusal is the stream's next
item and the stream ends there, and the caller's own offset says where to
resume. An authored `upsert` is different on purpose: a write is a write,
even with the same values.

## Extended: where a source read its values

A source record may name `upstream`, the other lake and position its
values were read from (`docs/decisions/010-provenance-per-field.md`,
extended). The ingest door takes it from the ingesting principal as it
takes `seen_at`, and no grant decides it. It is not in the event's id,
so a window delivered again stays one event whatever it names, and no
duplicate rule changes (lakelands#174).
