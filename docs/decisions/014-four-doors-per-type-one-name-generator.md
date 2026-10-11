# 014 — Four doors per type, and one generator names every tool

## What is decided

### A type offers four doors

Every type a principal may use offers four doors: `find`, `get`,
`upsert` and `retire`. `find` takes a filter, an order, a limit, a
cursor and an optional aggregate, so it covers listing, counting,
summing, ranges and grouping; a view type is found like any other type.
Every declared verb is one door of its own, with a typed payload. A
relation carried by link records offers `link` and `unlink`, with a
tool of each per relation from the one generator
(`docs/decisions/035-a-link-is-a-record.md`).

The generic doors are `describe`, `query`, `search`, `propose`,
`proposals`, `accept`, `reject`, and the schema doors `declare_type`,
`add_field`, `recompute`
(`docs/decisions/022-functions-are-declarations.md`) and `migrate`.
`migrate` carries the value-type migrations and re-keying as well as a
type's reshaping: the value-type migrations have no tool of their own,
and re-keying no door of its own
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended). Three generic tools are doors of the MCP session rather than
of the `Catalog`: `pin`, which pins `as_of` for the session's later
reads (009), and `subscribe` and `unsubscribe`, which begin and end the
session's notifications of the changes it may read
(`docs/decisions/037-the-feed-over-a-network-door.md`).

Six doors of the `Catalog` have no tool of their own: `describe_type`
is `describe` with a `type`, `migrate_apply` is `migrate` with the
`report_id` of the preflight it applies, `migrate_value_type` and
`migrate_value_type_apply` are `migrate` with a `value_type`, `apply`
has no tool, and `ingest` is deliberately not a tool — a source feeds a
lake through a stream, not through a model. `declare_type` is `declare`
of a type; `declare` of any other declaration has no tool, as `apply`
has none.

### One generator names every tool

The names of the tools an LLM sees are produced in exactly one place:
`lakelands_api::tools::tools`, a pure function of the lake's
description as one principal received it. Per type the tools are
`<type>_find`, `<type>_get`, `<type>_upsert` and `<type>_retire`, each
present only if the principal may use it; per verb, the verb's name. The
schemas in and out come from the same function, so they list only what
that principal may read or write. A declaration whose names would
collide with another tool, run past 64 characters or come out empty is
refused when declared, and the generator itself refuses to produce such
a set rather than emit a duplicate.

## Why

A model chooses reliably among a few dozen tools, not among hundreds.
Eight read verbs per type, plus writes and views, make a twenty-type
lake offer over two hundred tools; four per type make it about eighty.
Nothing is lost: every read the eight verbs did is a `find` with
arguments.

One generator is what keeps the doors honest with each other. The
store's catalog, the MCP server and any other transport take names
from the same function, so a tool cannot be named one way in a test and
another in production, and the `single-catalog` gate can refuse a tool
name written anywhere else. Deriving the tools from `describe` means
rights filter the tool list and the schemas by construction, with no
second rule to keep in step.

## What it is not

- It is not a ban on richer reads. Anything a caller needs to ask goes
  into `find`'s arguments, the pattern query (018) or the one expression
  grammar (017), never into a fifth per-type door.
- It is not a naming convention a transport may reinterpret. A
  transport presents the generator's names as they are.
- A caller that should not see a door does not see it at all; the tool
  list is not a list of everything with some entries forbidden.

## How it came to be

- The first version: four doors per type, one door per verb, the generic
  doors, and one generator for every tool name.
- Corrected on 2026-09-25: the doors added since the first version were
  named: `link` and `unlink` per relation carried by link records, and
  `migrate` carrying the value-type migrations and re-keying.
- Corrected at the rewrite (lakelands#817): the lists were read again
  off the generator and the `Catalog`. `recompute` joined the schema
  doors; `subscribe` and `unsubscribe` joined `pin` as doors of the MCP
  session; the doors of the `Catalog` without a tool, three before, are
  six, with `apply` and the two of a value type's migration, and
  `declare` has a tool for a type only. The value-type migrations have
  doors of their own on the `Catalog` and no tool of their own.
