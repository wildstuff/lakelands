# 014 — Four doors per type, and one generator names every tool

## What is decided

Every type a principal may use offers four doors: `find`, `get`,
`upsert` and `retire`. `find` takes a filter, an order, a limit, a
cursor and an optional aggregate, so it covers listing, counting,
summing, ranges and grouping; a view type is found like any other type.
Every declared verb is one door of its own, with a typed payload. The
generic doors are `describe`, `query`, `search`, `propose`,
`proposals`, `accept`, `reject`, and the schema doors `declare_type`,
`add_field` and `migrate`. One generic tool is a door of the MCP
session rather than of the `Catalog`: `pin`, which pins `as_of` for the
session's later reads (009). Three doors of the `Catalog` have no tool of
their own: `describe_type` is `describe` with a `type`, `migrate_apply`
is `migrate` with the `report_id` of the preflight it applies, and
`ingest` is deliberately not a tool — a source feeds a lake through a
stream, not through a model.

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

One generator is what keeps the doors honest with each other. The mock,
the store's catalog, the MCP server and any other transport take names
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

## Corrected (2026-09-25): the doors added since

The list of generic doors above predates two additions. A relation
carried by link records offers `link` and `unlink`, with a tool of each
per relation from the same generator
(`docs/decisions/035-a-link-is-a-record.md`). And `migrate` carries the
value-type migrations and re-keying as well as a type's reshaping, with
no door of their own
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended).
