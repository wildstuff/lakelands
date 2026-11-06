# 012 — A write touches only the fields it carries

## What is decided

### An `upsert` is a sparse patch, merged per field

An `upsert` is a sparse patch, merged per field. A field absent from the
patch is unchanged; a field set to `null` is cleared. A writer that
sends only its own fields cannot erase anyone else's.

Together with per-field grants (006), this makes two writers on one
record safe by construction: each principal writes only the fields it is
granted, and a patch that carries a field its principal may not write is
refused whole. So is one that carries a field its principal's grant
hides on the records a condition selects, where the record is selected
before the write or after it, naming the field
(`006 § A field may be hidden on only the records a condition selects`).

### A field that declares `clear` is cleared in the event that changes its input, transitively

A field may declare that it depends on other fields (`depends_on`) and
what happens when one of those inputs changes (`on_input_change: keep |
clear`). With `clear`, the fold clears the dependent field in the same
event that changes the input. With `keep`, the value stays, and its
`known_at` shows it is older than its inputs.

When the fold clears a field because an input changed, the clearing is
itself a change: a field that depends on the cleared field, with
`on_input_change: clear`, is cleared in the same event, and so on until
nothing more changes. A field the write sets itself is never cleared by
this rule.

### A derived field computed at the write is recomputed in the same event, or cleared

A derived field computed at the write is recomputed in the same event,
in dependency order. If its expression cannot be evaluated for the
record — an input is absent, a division by zero, a value that does not
fit the field — the field is cleared rather than left holding a value
computed from inputs that no longer hold (023). A derived field computed
at the read is never stored (052).

## Why

A store with more than one writer per record is the normal case, not an
edge: an import writes the facts, a person or an agent writes a
verdict, a computation writes a score. If a write replaces the whole
record, the last writer silently erases the others, and the only
defence is every caller reading and re-sending fields it does not own,
which races.

Invalidation belongs in the fold for the same reason. If the caller had
to clear dependent fields, a caller that forgot would leave a verdict
standing on inputs that no longer hold. In the fold it happens on every
write, deterministically, and a replay reproduces it.

## What it is not

- It is not a merge by the door. No transport, CLI or tool carries a
  list of "fields I do not own"; the rule is the fold's, so a caller
  reaching the store directly gets it too.
- It is not last-writer-wins at record level, and it is not a
  conflict-free merge of concurrent edits to the same field. For that,
  a writer passes the record's expected version and the write is
  refused if it moved.
- `keep` does not hide staleness; it leaves the decision to the reader,
  with the evidence in the field's metadata.

## How it came to be

- The first version: the sparse patch, and `depends_on` with
  `on_input_change`.
- Clearing is transitive, and a derived value that cannot be computed
  is cleared.
- Corrected at the rewrite (lakelands#817): "A derived field is
  recomputed in the same event" was said of every derived field; 052
  added fields computed at every read, which the fold computes in no
  event and no record stores.
- A patch that carries a field its writer's grant hides on a selected
  record is refused whole (lakelands#1412), as a field hidden by
  condition came to 006.
