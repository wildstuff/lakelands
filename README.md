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

![Lakelands, the mechanism: in one lake, ingest streams and authored writes append to a hash-chained log that is the truth; a per-field fold governed by the ontology rebuilds a disposable projection; a catalog generated from the ontology serves typed tools to agents over MCP; below the seam line The Wild plugs in through named seams.](docs/architecture.svg)

*In one lake: writes append to the log, the fold rebuilds the
projection, the ontology governs the fold and generates the tools. The
change feed is the log itself. Below the line, The Wild attaches through
seams — Lakelands never knows a bus, a sandbox or a model.*

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
  fusion, and typed filters over one catalog, in the same engine; a
  pattern query walks relations from a typed start. Embeddings come
  from the caller or from a pluggable, OpenAI-compatible client. The
  map is what makes search precise: a search never returns a shape the
  ontology did not declare.

**For agents.** Declare a type and the lake exposes four doors — `find`,
`get`, `upsert`, `retire` — each with a JSON schema derived from the
type and filtered by the caller's rights, a `link` and `unlink` pair for
each relation carried by link records, plus one tool per declared verb,
`describe`, `query` and `search`. Rights can remove a door as well
as narrow it: a type whose fields all come from a source has no
`upsert`. Beside them an agent sees the lake's other doors: `pin` for
the position its turn reads at, `subscribe` and `unsubscribe` to be told
of changes without polling, `propose`, `proposals`, `accept` and
`reject` for writes that wait for a review, and `declare_type`,
`add_field`, `migrate` and `recompute` for the schema. Served over MCP
(stdio and HTTP) and REST from one catalog. Agents never write a query
language.

> **Status: `0.11.0`.** Lakelands `0.11.0` is the `lakelands` CLI, its
> MCP server (stdio and streamable HTTP) and its REST door over the
> SQLite, in-memory and PostgreSQL engines, all three passing the whole
> conformance suite (466 checks through the doors and 65 through the
> engine seam), with 52 decision records and the event log at format
> version 12, installable from the release. Its budgets: the store
> facade and engine traits hold 58 of 110 methods, the core crates'
> product code 46 345 of 47 000 lines and their specification 59 654 of
> 60 000, and 2 of 20 `cfg(feature)` sites. A ceiling rises only by a
> recorded rule (decision 047): the product's rose to 45 500 and the
> specification's to 58 000, row by row, each for a remedy or a rule
> this release needed, and the operator set both higher, to 47 000 and
> 59 000, as a reserve for the release's last changes; the
> specification's rose once more, to 59 500, for two rights fixes, and
> to 60 000 for a third. Beside what `0.10.0` held, a grant can admit
> only some records of a type, a field can be computed at the read, and
> a derived field that reads the clock says when it is computed. A
> setting weighs a type's vector fields, and a search says what each
> axis brought and with what weight. A graph answers what its steps say,
> a page at a time. A request that names a member it does not take is
> refused, naming it. A value type's migration renames a value in every
> record that holds it, retired ones included, in one event. A
> description, a schema hash and a report's id each answer only what
> their reader is shown: a relation is shown with both its ends, and a
> type keyed by a field its reader may not know of is not shown, nor a
> relation whose `via` field its reader may not know of; an author's
> valid time, `via` or key that names a field it may not know of is
> answered as a name no field has, and a valid time over one is not
> named. An engine's error quotes no row, statement or value, and a
> hybrid search names nothing of a type's embedding before its caller is
> judged to see it. A text the embedding backend refuses fails alone in
> its batch, a backend can be bound after the open, a write waits only
> for its own calls, and an executor's answer is held until it is
> recorded. A set of declarations computes its records once, at its last
> (log format 12); a read takes the lake's shape by position; a page
> under an exact text, a kept view's whole face and a kept extreme's
> next holder are each read a page or a row at a time; a feed poll
> examines a bounded number of events. The mock answers as the store
> does where the two had parted: over null, a view's probe and
> declaration, names and expressions, a cursor, and a call's order of
> refusals.

## Try it

This follows the first walk (`docs/walks/walk-1.md`), whose example
lake ships in `examples/liquidity/`. It runs on the SQLite engine, the
default: a lake kept in memory lives only as long as its process, so a
sequence of commands cannot walk one.

```sh
cargo install --path crates/lakelands-cli    # or the binary from the release
lakelands init walk && cd walk
lakelands lake new liquidity
lakelands apply ../examples/liquidity
d=../examples/liquidity/data
lakelands ingest $d/partners.csv --source crm --map $d/partners.map.json
lakelands ingest $d/invoices-august.csv --source erp --map $d/invoices.map.json
lakelands ingest $d/invoices-september.csv --source erp --map $d/invoices.map.json
lakelands serve --mcp                        # point an MCP client at this command
# Ask it: which debtors are over their credit limit and more than 30 days overdue?
```

The agent finds the answer with the lake's generated tools
(`invoice_find` with an aggregate, `partner_get` with provenance), and
can pin an earlier position to ask what the lake knew last month. The
walk's page lists every call and the answer it must give.

## Read further

- [`docs/log-format.md`](docs/log-format.md): the event log, byte for
  byte, with test vectors a verifier in any language can check.
- [`docs/seams.md`](docs/seams.md): the engine contract and the seams a
  host plugs into; [`docs/hosting.md`](docs/hosting.md): the same seen
  from a platform that embeds Lakelands.
- [`docs/conformance.md`](docs/conformance.md): the suite that defines
  what it means for an engine to support Lakelands, and every check in
  it.
- [`docs/decisions/`](docs/decisions/): why Lakelands is built the way
  it is, one decision per record.
- [`docs/walks/walk-1.md`](docs/walks/walk-1.md): the first walk, end
  to end, with its expected answers.
- [`docs/walks/walk-2.md`](docs/walks/walk-2.md): the second walk, over
  what `0.2.0` adds: a function run by a program, an embedding on
  write, links with fields, aging by date and the webhook deliverer.
- [`docs/walks/walk-3.md`](docs/walks/walk-3.md): the third walk, over
  what `0.4.0` adds: a lake moved by export and import, a mirror between
  two lakes that names the source's position, and degree and components
  over links, refused past the lake's ceiling.
- [`docs/walks/walk-4.md`](docs/walks/walk-4.md): the fourth walk, over
  what `0.7.0` adds: a procedure from an order to its payment, through a
  derived relation, a transition guard, a host's reaction calling a verb
  on its type with its cause named, the lineage read back, a retirement
  that fires nothing, and a view bound to a reader's date.
- [`docs/walks/walk-5.md`](docs/walks/walk-5.md): the fifth walk, by a
  blind LLM agent that sees only the MCP door: a clerk's tasks, and the
  common refusals, each with what the agent changed after it.

## What it is not

- **Not a data lake.** A data lake stores raw files and lets meaning
  arrive later. Lakelands is the opposite: schema-first, every write
  governed by declared authority, every value traceable to its event.
- **Not autonomous.** Lakelands detects when something is due — a
  trigger matched, an object set changed, a verb was called — and records it
  durably. Executing it is behind a seam: an HTTP endpoint, a local
  process, a webhook, or whatever you plug in. Nothing in the store
  calls a model, runs a sandbox, or reaches the network on its own.
- **Not a new query language.** Lakelands sits on SQLite and
  PostgreSQL and keeps them visible. The projection is a plain database you can
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

`0.1.0` holds what is on disk, in every reply, or in a trait: the log
format, per-field provenance, positions on every answer, the engine
contract and the seams. What came right after arrived through those
seams without changing them: the PostgreSQL engine, the REST door, the
OpenAI-compatible embedding client and webhook delivery of fired
triggers and set changes, MCP notifications and a client for a lake
served over REST, all in `0.2.0`. The repository goes public once The
Wild runs on a tagged Lakelands.

## Licence

[Apache License 2.0](LICENSE). Copyright 2026 Wildstuff.
