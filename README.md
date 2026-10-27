# Lakelands

**The open ontology store. Every value typed, traced and verifiable.**

An ontology types every write, every field keeps its source and its
time, and the log is hash-chained, so anyone can check what was known,
since when, and who changed it — without us. Files, streams, REST and
agents over MCP all write into the same lake under the same rules. It
runs on your machine; engine and on-disk format are Apache-2.0.

Lakelands is the engine [The Wild](https://github.com/wildstuff/wild)
is built on. Use it on its own as a store, or take The Wild for the
whole thing: connectors, agents that build their own tools, and live
apps on top.

## The problem

Whatever writes into a store — an import, a person, an agent — most
stores keep the value and lose how it got there. Ask the questions
that matter and they cannot answer them:

- **Where did this value come from?** An import, a person, or an
  agent's guess?
- **Since when has it held — and what did we know when we decided?**
  A store that updates in place has already forgotten.
- **Who changed it, and was it allowed to?** A second writer quietly
  overwrites the first; a re-import erases a verdict a human wrote.
- **Has anything been rewritten since?** You are asked to trust the
  system that holds it.

Once agents act on these values — file an invoice, mark a contract as
reviewed, chase a payment — a value nobody can trace is no longer an
inconvenience; it is a decision made on a guess. Lakelands keeps every
value together with how it got there, and lets anyone check both.

## What it is

A landscape of typed, auditable lakes — not a data lake.
Every lake has a map (its ontology), keeps its
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
`describe`, `query`, `cypher` and `search`. Rights can remove a door as well
as narrow it: a type whose fields all come from a source has no
`upsert`. Beside them an agent sees the lake's other doors: `pin` for
the position its turn reads at, `subscribe` and `unsubscribe` to be told
of changes without polling, `propose`, `proposals`, `accept` and
`reject` for writes that wait for a review, and `declare_type`,
`add_field`, `migrate` and `recompute` for the schema. Served over MCP
(stdio and HTTP) and REST from one catalog. Agents write no query
language of Lakelands' own: `cypher` reads a statement of the
openCypher subset as the `query` pattern it translates into.

## Verify it

A value you are asked to trust is a value nobody can check. Every
claim Lakelands makes about its data is one you can recompute:

- **What was known, and when.** Every reply — to a person or an agent —
  carries the position it was read at. Pin an earlier position and the
  lake answers as it did then.
- **Where a value came from.** Every field records who wrote it, when
  the lake learned it and when its source saw it — per field, not per
  record.
- **That nothing was rewritten.** Each event carries the hash of the
  one before it. The format is published byte for byte in
  [`docs/log-format.md`](docs/log-format.md), with test vectors and a
  `verify.py` beside them that shares no code with Lakelands: point it
  at a lake's directory and it checks the chain from the segment files
  alone. An import checks an export's chain before it opens.
- **That an engine keeps its word.** An engine is a port, not a
  re-implementation, and proves itself against the open conformance
  suite ([`docs/conformance.md`](docs/conformance.md)) — SQLite,
  PostgreSQL and in-memory pass all of it.

## Where Lakelands fits

Other open stores for agents are good at other things. This is the
terrain Lakelands is built for, and where it is not the right tool.

| You need … | What usually fills it | What Lakelands adds, or leaves to them |
|---|---|---|
| graph and vector search in one fast engine, at scale | graph-vector databases | Lakelands searches (full-text, vectors, typed filters, walks over relations), but its terrain is what a value *means* and *where it came from*, not traversal speed. It runs no speed contest; its load run and its 24-hour soak ([`docs/soak.md`](docs/soak.md)) test that every acknowledged write survives a killed process. |
| the history of every row, in SQL | bitemporal SQL databases | Lakelands is bitemporal too, and adds the ontology, provenance and write rights per field, a hash-chained log and agent tools generated from the types. |
| memory for an agent, as a service | hosted memory layers | Lakelands is not a service. The parts that make memory trustworthy — history, provenance, the verifier — run on your machine, under Apache-2.0. |
| raw files now, meaning later | data lakes | The opposite: schema first, every write governed by its declared authority. |
| notes your agent reads and greps | a Markdown vault | Typed records with a source and a time on every field, and tools instead of grep. |

**Choose something else when:**

- **many processes must each write one dataset directly.** A lake takes
  any number of concurrent callers, and writes from several processes go
  through its one writing process; past what one writer takes (about
  5 000 writes a second on SQLite), throughput comes from more lakes.
- **you need queries across tenants.** There is no query across lakes;
  data crosses only as a mirror that carries its provenance.
- **the work is heavy graph analytics.** Walks have ceilings, and a walk
  past its ceiling is refused, not run.
- **you want a managed service.** There is none; you run it, on SQLite or
  your own PostgreSQL.
- **you want to store first and model later.** Lakelands asks for the
  map first. An agent can draw it over MCP (`declare_type`, `add_field`),
  but there is always a map.

> **Status: `0.20.0`.** Lakelands `0.20.0` is the `lakelands` CLI, its
> MCP server (stdio and streamable HTTP) and its REST door over the
> SQLite, in-memory and PostgreSQL engines, all three passing the whole
> conformance suite (891 checks through the doors and 84 through the
> engine seam), with 83 decision records and the event log at format
> version 20, installable from the release. Its budgets: the store
> facade and engine traits hold 91 of 110 methods, the core crates'
> product code 90 621 of 92 500 lines and their specification 167 799 of
> 168 000, and 6 of 20 `cfg(feature)` sites. A ceiling rises only by a
> recorded rule (decision 047): the ceilings stand at 92 500 and 168 000,
> and the ratchet bases at 91 000 and 168 000.
> Beside what `0.19.0` held, a lexical score is BM25F on every engine
> (lakelands#1459), the log stores what an append shares once and seals a
> record's erasable values in one box (lakelands#1440, lakelands#1455),
> an object set may be decided at the read (lakelands#1437), a field may
> be hidden by a condition (lakelands#1454), and the openCypher subset is
> a `cypher` tool beside `query` over MCP and a statement over REST, the
> CLI and the client (lakelands#1434, lakelands#1451). A condition nests
> at most 32 levels (lakelands#1439). A writer's open upgrades a lake of
> format 14, 15, 16, 17, 18 or 19 to 20 by appending; the first writer's
> open of 0.20.0 also rebuilds each SQLite and PostgreSQL lake's
> projection (its layout is 9) beside the one it replaces, the log
> untouched, and the lake of format 20 is kept beside those of the
> releases before.

## Try it

With the `lakelands` binary from a release (or `cargo install --path
crates/lakelands-cli`), one command makes the first walk's lake and
connects Claude Code to it:

```sh
lakelands init walk --example liquidity
cd walk && claude    # approve the server `lakelands` when it asks
```

Ask it: *which debtors are over their credit limit and more than 30 days
overdue?* The agent finds the answer with the lake's generated tools
(`invoice_find` with an aggregate, `partner_get` with provenance), and
can pin an earlier position to ask what the lake knew last month. The
command prints the walk's answers and that position.

The example (`examples/liquidity/`) is inside the binary, so nothing is
fetched. `init` writes `walk/.mcp.json`, Claude Code's project
configuration, whose server is the binary that ran it; a `.mcp.json`
that is there already is left as it is, and the command prints the entry
to add. For any other MCP client it prints the server's command line.

What it does, as the seven commands of the first walk
(`docs/walks/walk-1.md`) on the SQLite engine, from a checkout:

```sh
lakelands init walk && cd walk
lakelands lake new liquidity
lakelands apply ../examples/liquidity
d=../examples/liquidity/data
lakelands ingest $d/partners.csv --source crm --map $d/partners.map.json
lakelands ingest $d/invoices-august.csv --source erp --map $d/invoices.map.json
lakelands ingest $d/invoices-september.csv --source erp --map $d/invoices.map.json
lakelands serve --mcp                        # point an MCP client at this command
lakelands verify                             # every event's hash and every checkpoint, beside the serve
```

`lakelands verify` reads the whole log and says whether it holds, or
the first position where it does not, where the log lets it name one.
Its exit status is 0 when the log holds, 1 when it is damaged, 2 for a
command line it does not understand or a range outside the log, and 3
when it could not check (no lake yet, no projection, a memory lake, the
database unreachable). A lake's open does not read the whole chain: a
line of the log changed by hand at its own length, before its last
append, opens and is written on, and only `verify` finds it.

The walk's page lists every call and the answer it must give.

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
- [`docs/walks/walk-6.md`](docs/walks/walk-6.md): the sixth walk, by the
  same blind agent: graph tasks through the typed `query` tool and
  through a statement of the openCypher subset, measured on the same
  tasks and the same lake.

## What it is not

- **Not a data lake.** A data lake stores raw files and lets meaning
  arrive later. Lakelands is the opposite: schema-first, every write
  governed by declared authority, every value traceable to its event.
- **Not memory you rent.** There is no hosted tier holding the parts
  that make it trustworthy: the ontology, the history, the provenance
  and the verifier are all in this repository, under the same licence,
  and a lake lives on your machine or in your own PostgreSQL.
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
contract and the seams. Each later release built on them; what it adds
is in its notes on the releases page, and what comes next is on the
milestones. `0.20.0` built the openCypher subset as an MCP read tool
beside `query`, PageRank and the cheapest walk, patterns that branch
and join, a change planned before it acts, sets decided at the read, a
field hidden on the records a condition selects, where a lexical hit
matched, BM25F as the lexical score on every engine (lakelands#1459),
the compact log format 20, and the sealing of a record's erasable values
in one box.

## Licence

[Apache License 2.0](LICENSE). Copyright 2026 Wildstuff.
