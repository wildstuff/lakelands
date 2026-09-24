# 032 — A reference names a live record

## What is decided

A `reference` value a write carries must name a live record of the
field's target type: one at the head, one written earlier in the same
append or ingest window, or the record being written itself. A write that
names any other record is refused whole, on that field, and appends
nothing. The rule holds for every write door: `upsert`, a verb's write,
an accepted proposal and `ingest`.

Retiring a record that others refer to stays allowed. The references
then name a retired record, and a walk hides that end as it hides any
retired record (`docs/decisions/026-the-store-walks-the-graph.md`).

## Why

A relation carried by a reference field is only as good as the key it
holds. A dangling reference written today is a walk that silently finds
nothing tomorrow, and an agent cannot tell a missing partner from a
misspelt one. Checking at the door costs one lookup per reference, and
it catches the mistake where it is made.

Resolving within an ingest window lets a source deliver a partner and
the invoices that name it in one export, in order, without first waiting
for the partner to be committed.

## What it is not

- It is not referential integrity over time. A record may be retired
  while others still name it; the log keeps both, and history reads show
  what was true then.
- It does not order an ingest for the source. A reference to a record
  later in the same window is still refused.
