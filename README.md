# Ontolake

Ontology-driven, event-sourced memory for AI agents. Declare types and
relations, ingest with enforced provenance, query as-of any point in
time, and hand agents typed tools. One file per tenant on SQLite,
PostgreSQL optional.

> **Status: pre-release.** Ontolake is being extracted from a
> commercial platform built by Wildstuff, where it has run as the data
> core since 2026. The repository currently holds the project's
> commitments, not its code. The code arrives once the extraction
> reaches a measurable state: the core builds with no dependency on the
> platform it came from, and its conformance suite passes on its own.
> Until then, nothing here is installable.

## What it is

Ontolake is a store whose schema gives data its meaning and whose
history is the truth.

- **A declared ontology, with authority per field.** Types, fields,
  relations (relations are first-class and can carry their own fields),
  value types with constraints, derived fields computed inside the
  store. Every field declares who may write it: the source feed, an
  author, or a computation. A re-import never overwrites a human's or
  an agent's verdict.
- **An append-only event log as the source of truth.** Current state
  is a pure projection of the log. Delete the projection, replay the
  log, and you get the same bytes back. Every read can be pinned to a
  log position, so "what did the agent see when it decided" is a store
  operation, not a forensic exercise. Records carry valid time as well
  as transaction time.
- **Provenance the writer cannot skip.** Every event records who wrote
  it, at which position, from which source, through which procedure
  version. Ingested records carry a source locator and a lineage edge
  the store derives itself.
- **Typed tools for agents, generated from the schema.** Pin a type and
  the store exposes `list`, `get`, `find`, `range`, `count`, `sum`,
  `group`, `view`, plus governed writes, each with a JSON schema derived
  from the type and filtered by the caller's rights. Served over MCP
  (stdio and HTTP) and REST from one catalog. Agents never write a
  query language.
- **Five models in one file.** Document, scalar index, property graph,
  full-text (BM25) and vector kNN with rank fusion, co-resident in one
  SQLite file per tenant. PostgreSQL is a second engine behind the same
  contract; an in-memory engine is the reference. A conformance suite
  is what "supports Ontolake" means for any engine.

## What it is not

- **Not autonomous.** Ontolake detects when something is due (a
  trigger matched, a schedule fired, a verb was called) and records
  it durably. Executing it is behind a seam: an HTTP endpoint, a local
  process, a webhook, or whatever you plug in. Nothing in the store
  calls a model, runs a sandbox, or reaches the network on its own.
- **Not a vector database.** Embeddings are supplied by the caller or
  fetched through a pluggable, OpenAI-compatible client. The store is
  model-agnostic by construction.
- **Not a new query language.** Ontolake sits on SQLite and PostgreSQL
  and keeps them visible. The projection is a plain database you can
  open with any tool; only the log is authoritative.

## Commitments

- **Apache-2.0, on the engine and on the on-disk format.** The event
  log and projection layout are published as a specification and
  versioned. A store you can read without us is the point.
- **No contributor licence agreement.** Contributions are accepted
  under the Developer Certificate of Origin (see
  [CONTRIBUTING.md](CONTRIBUTING.md)).
- **A conformance suite over a narrow engine contract.** New engines
  are ports, not re-implementations, and the suite is how they prove
  it.
- **Determinism is the boundary.** Whatever must reproduce
  byte-identically from the log lives inside the store. Whatever needs
  the outside world lives behind a documented seam.

## Roadmap, at the altitude this file can promise

1. The log format specification and the seam contracts land here as
   documents.
2. The core crates arrive with their history, once the extraction
   trigger is met.
3. The standalone door: the `ontolake` CLI, the MCP and REST server,
   a WIT package for component hosts.
4. The repository goes public.

## Licence

[Apache License 2.0](LICENSE). Copyright 2026 Wildstuff.
