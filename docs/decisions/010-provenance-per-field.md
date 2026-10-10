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

## Extended: why a computed field holds no value

A field computed by a function whose backing failed holds no value.
Its provenance still says why: the metadata of the event that cleared
it, with `error` set to the reason. A read with `with_meta` returns
that metadata for the field even though the field has no value, so a
caller can tell "not computed yet" (no metadata) from "computed and
failed" (`error`). A field that holds a value never carries `error`.

## Extended: a value read from another lake

A value a source read from another lake's log carries `upstream`: that
lake and the position of the event there whose state it is, beside
`seen_at`. A mirror states it for every field it delivers, so a reader
can follow a value back to the log it came from. It is provenance the
writer states, and it has the same standing as `seen_at`: nothing checks
it against the other lake. A later write of the field without it leaves
the field without it. The member came with a step of the log format
(`docs/log-format.md` §1.1, §6; lakelands#174).

## Extended: a value no writer chose

A field that declares `initial` is filled by the fold when its record
is made without a value for it
(`docs/decisions/050-a-field-declares-its-initial-value.md`). Its
`origin` is `initial`, a fifth member of the closed set: not `system`,
which an aging move writes on the same lifecycle fields, so a reader
tells the value a record started with from one the lake moved since.
It takes the creating event's `known_at`, `position` and `writer`, and
nothing of a source: no `source`, `seen_at`, `upstream` or
`procedure_version`, since the value came from the declaration.
