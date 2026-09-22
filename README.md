# Lakelands

**The ground that AI agents work on.** A landscape of typed, auditable
lakes — not a data lake. Every lake has a map (its ontology), keeps its
whole history in layers (its event log), is fed and drained by streams
(its changes), and can be searched from any point on the map. Agents get
typed tools to read and write it; every value they see says where it came
from and since when.

```
Lakelands
├── Lakes     — domain data
├── Streams   — changes and events
├── Ontology  — world model
├── History   — bitemporal state
└── Search    — retrieval
```

Lakelands is the open ground. The Wild is one world that lives and works on it:
the agents, the tools they use, the connectors that bring water in, and
the runtime that keeps everything moving. The ground is Apache-2.0 and
belongs to whoever stands on it; the life on top is Wildstuff's product.

## The story, in five parts

- **Lakes.** A lake is one world with one truth. Your invoices, your
  partners, your transactions live in one lake; another tenant's world
  is another lake. There is no query across lakes, and water only
  crosses between them as a mirror that carries its provenance. Many
  lakes live in one process; each has one writer, one log, one map.
- **Ontology.** The map of a lake. Types, fields, relations that can
  carry their own fields, value types with constraints, derived fields
  the lake computes itself. Every field on the map says who may write
  it — the source feed, an author, a computation — and rights are held
  per field and per principal. A re-import never overwrites a verdict a
  human or an agent wrote; a second writer never erases a first.
- **History.** A lake keeps everything that ever happened to it, in
  order, in layers: the append-only event log is the truth, and the
  current state is only a projection of it. Delete the projection,
  replay the log, get the same bytes back. Every reply carries the
  position it was read at, so "what did the agent see when it decided"
  is a question the lake answers. Records carry valid time as well as
  transaction time, and every field remembers when the lake learned it,
  when its source saw it, and who wrote it.
- **Streams.** Water moves. Every change is an event in the log — a
  write, a fired trigger, an object-set membership change — and the
  change feed is that log filtered by kind, read from any position.
  Consumers keep their own cursor; the lake keeps no consumer state.
  Ingest arrives as a stream too, content-addressed, so a message
  delivered twice is a no-op.
- **Search.** Finding your way. Full-text, vector kNN with rank
  fusion, and typed filters over one catalog, in the same file; a
  pattern query walks relations from a typed start. Embeddings come
  from the caller or from a pluggable, OpenAI-compatible client. The
  map is what makes search precise: a search never returns a shape the
  ontology did not declare.

**For agents.** Pin a type and the lake exposes four doors — `find`,
`get`, `upsert`, `retire` — each with a JSON schema derived from the
type and filtered by the caller's rights, plus one tool per declared
verb, `describe`, `query` and `search`. Served over MCP (stdio and HTTP)
from one catalog. Agents never write a query language.

> **Status: pre-release.** Lakelands is being extracted from a
> commercial platform built by Wildstuff, where it has run as the data
> core since 2026. The repository currently holds the project's
> commitments, not its code. The code arrives once the extraction
> reaches a measurable state: the core builds with no dependency on the
> platform it came from, and its conformance suite passes on its own.
> Until then, nothing here is installable.

## What it is not

- **Not a data lake.** A data lake stores raw files and lets meaning
  arrive later. Lakelands is the opposite: schema-first, every write
  governed by declared authority, every value traceable to its event.
- **Not autonomous.** Lakelands detects when something is due — a
  trigger matched, a schedule fired, a verb was called — and records it
  durably. Executing it is behind a seam: an HTTP endpoint, a local
  process, a webhook, or whatever you plug in. Nothing in the store
  calls a model, runs a sandbox, or reaches the network on its own.
- **Not a new query language.** Lakelands sits on SQLite and PostgreSQL
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
2. The core arrives, one organ at a time, transformed on arrival.
3. The standalone door: the `lakelands` CLI and the MCP server.
4. The repository goes public.

## Licence

[Apache License 2.0](LICENSE). Copyright 2026 Wildstuff.
