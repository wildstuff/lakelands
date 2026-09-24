# 010 — Provenance is per field, and the fold materialises it

## What is decided

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
record in one call (`with: meta`), without a second query.

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

## Extended: the wire name

On every door the switch that returns each field's provenance with its
value is spelled `with_meta` (a boolean on `find`, `get` and `search`).
"`with: meta`" above names the same switch.
