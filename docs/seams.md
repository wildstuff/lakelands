# Seams

**Draft.** The first complete draft; a review pass (lakelands#15)
follows before `0.1.0`.

A seam is a trait in `lakelands-api` through which Lakelands reaches
anything outside its deterministic core. The rule that decides what is a
seam is stated once
(`docs/decisions/002-determinism-is-the-boundary.md`): **whatever must
reproduce byte-identically from the log lives in the store; whatever
needs the outside world sits behind a seam.** The store calls a seam,
records its output in the log (`output_recorded`, see
`docs/log-format.md`), and on replay reads the recorded output instead of
calling the seam again.

A seam never holds an invariant. Field rights, per-field provenance, the
`sys.` guard, idempotency and the writer lease are enforced by the store
on every call; no seam implementation can relax them, and none needs to
enforce them.

## The roster

Seven seams, this roster and no other. Adding one is a change to the
contract and to this page.

| Seam | Trait(s) | Default in Lakelands | A host plugs in |
|---|---|---|---|
| Engine | `LakeEngine`, `ProjectionStore`, `ProjectionView`; `VectorIndex` beside them | SQLite and in-memory (`0.1.0`); PostgreSQL after | Rarely anything: a new engine, proven by the conformance suite |
| Principal resolution | `PrincipalResolver` | API key or static entry → one principal | Its identities, roles and policies, compiled to grants |
| Function backings | `FunctionBacking` | `expr`, evaluated in the store | Its own backing kinds; `http` and `exec` ship after `0.1.0` |
| Embeddings | `EmbedBackend` | None: lexical search and caller vectors | An embedding client; an OpenAI-compatible one ships after `0.1.0` |
| Verb execution | `VerbExecutor` | None: external verbs are refused as `Unbound` | What its external verbs do |
| Change feed | `ChangeFeed` | The log, read by kind from a position | A relay of the feed onto its own bus, as its own code |
| System vocabulary | `SystemVocabulary` | Lakelands' four `sys.` types, allowed in every lake | Its own `sys.` types and which lakes may hold them |

`Location` is not a seam. It is the configuration every seam and every
engine is configured through, passed at the call site; Lakelands reads no
environment (`docs/decisions/005-no-environment-reads.md`).

A host that plugs in nothing has a working store: every slot of `Seams`
has a default.

## Engine

An engine keeps one lake: its log, its writer lease, its projection and
its vector index. It stores and returns; the store's core decides.

Two rules shape the engine seam:

- **The store canonicalises; the engine keeps bytes.** The store turns
  every event into its canonical bytes and hash (`docs/log-format.md`
  §1.2) and hands the engine a `CanonicalEvent`: the bytes, with the
  position, id, `prev`, `hash` and `fence` beside them so the engine can
  check an append without parsing. The engine stores the bytes and gives
  back the same `CanonicalEvent`. The store parses and verifies what it
  reads. Byte identity is checked through this seam by the conformance
  suite.
- **The projection keeps its history.** Every `put_*` adds a state valid
  from a position and keeps the state it supersedes. `view(as_of)` is a
  filter over kept states, never a replay: "what did the agent see at
  position N" is a cheap read on every engine. The conformance suite
  reads each record at every position it changed and compares with a
  fresh replay to that position. The
traits are synchronous and `dyn`-compatible: storage work blocks, and the
store runs an engine off its async callers. Every behaviour below is
checked by the conformance suite against every engine on the roster.

### `LakeEngine`: the log and the lease

| Method | Contract |
|---|---|
| `name` | The engine's name, as `EngineConfig` selects it. |
| `lake` | The lake this handle keeps. |
| `take_lease(holder)` | Take the writer lease with a fencing token greater than every earlier token. **Fails at once** when another process holds it, with an `unavailable` error whose message names the holder; never waits. Refused on a handle opened read-only. |
| `release_lease(lease)` | Give the lease up; the next taker gets a greater token. |
| `tip` | The last event's position and hash, or none for an empty lake. |
| `append(lease, events)` | Append a group of `CanonicalEvent`s, all or none, **durably before returning**. Refused when `lease` is stale or an event's fence differs from it, when the positions do not continue the tip without a gap, or when a `prev` is not the hash before it. Stores each event's bytes unchanged. |
| `read(from, limit)` | `CanonicalEvent`s from a position, exactly as appended: the same members, the same bytes. |
| `seal_segment(lease)` | Close the active segment; none if it is empty. The store then appends the `checkpoint`. |
| `segments` | Every sealed segment's range and digest. |
| `projection` | The projection's write side. |
| `view(as_of)` | A read view at the head, or as the projection was right after folding `as_of`, served from kept states. Refused as out of range for a position not yet folded. |
| `vectors` | The vector index, if the engine keeps one. |

An engine is found by name and opened through `EngineOpener`:

| Method | Contract |
|---|---|
| `name` | The name the engine answers to in `EngineConfig`. |
| `open(location, mode)` | Open the lake `Read`, `Write` or `Create` (write, creating it if absent). Opening never takes the lease; a writer takes it afterwards with `take_lease`. A handle opened `Read` refuses `take_lease` and `append`, so any number of readers can hold the lake while one process writes (`docs/decisions/008-one-writer-per-lake.md`). |

### `ProjectionStore`: what the fold writes

The fold calls the projection inside one transaction per append;
`commit(through)` records the position folded through in the same
transaction, so a crash leaves the projection at a whole append, and
opening a lake folds the log from `folded_through` on.

| Method | Contract |
|---|---|
| `begin`, `commit(through)`, `rollback` | One transaction per append. |
| `folded_through` | The last position the projection reflects. |
| `put_declaration` | A declaration in force from a position. |
| `put_record(record)` | A record's whole state as of `record.position`: values, per-field meta, flags. The state it supersedes is kept. The engine derives its own indexes, full-text entries and edges from it and the declarations in force. |
| `put_proposal` | A proposal's state as of its latest event; earlier states kept. |
| `put_membership(…, at)` | Whether a record is in an object set from a position on; earlier membership kept. |
| `put_output` | A seam's recorded output, by seam and input digest. |
| `mark_applied(id, at)` | Remember an applied event id, so a redelivery is recognised. |
| `clear` | Delete everything, kept history included, for a rebuild. |

Rebuilding a projection from the log produces the same state, byte for
byte, as the projection it replaces
(`docs/decisions/001-the-log-is-the-truth.md`); the conformance suite
checks it on every engine.

### `ProjectionView`: what reads see

A view reflects one position. Records come back whole and unmasked;
grants and masking are the store's.

| Method | Contract |
|---|---|
| `position` | The position the view reflects. |
| `declarations` | Every declaration in force. |
| `get(type, key)` | One record, retired and restricted included. |
| `scan(type, scan)` | Records filtered by a typed expression the engine applies **exactly**, ordered with missing values last and the key as tiebreak, continued after a keyset position. |
| `aggregate` | `count`, `sum` (exact for decimals), `min`, `max`, `avg`, per group. |
| `edges(type, key, relation, walk)` | One record's links over one relation or all: `Out` those it declares, `In` those that point at it, `Both` their union with each link once (a link from a record to itself included). Links to a retired or missing record are returned as they are; hiding that end is the store's (`docs/decisions/026-the-store-walks-the-graph.md`). No order is promised; the store's walks order their own answers. |
| `lexical(type, text, k)` | Full-text hits over searchable fields, best first. |
| `proposals`, `is_member`, `applied`, `output` | The projections of proposals, object sets, applied ids and recorded outputs. |

### `VectorIndex`

`put`, `remove`, `nearest(type, field, query, k)` (higher score is
closer) and `count`. Vectors are indexed at the head; a read at an
earlier position filters the candidates by what was visible there.

## `PrincipalResolver`

`resolve(lake, credential) -> Principal`. A credential is an API key, a
bearer token or a local user; the answer is one principal with its
grants, for one lake. Refused when the credential is unknown.

The default resolves an API key or a static configuration entry to a
principal. A host that has identities, roles, teams or policies compiles
them into grants here (`docs/decisions/006-grants-not-roles.md`). The
resolver belongs to the doors (the CLI, the server), not to the store.

## `FunctionBacking`

`kind()` names the backing kind, an open token; `call(function call) ->
value` computes one output. The store records every output and never
calls a backing twice for one input: a replay reads the recorded output.

The default is `expr`, evaluated inside the store in the one expression
grammar; because it is deterministic, its results are recomputed on
replay rather than recorded. `http` and `exec` follow after `0.1.0`. A
host may add its own kind.

## `EmbedBackend`

`model()`, `dimensions()`, `embed(texts) -> vectors`. The store records
each vector it obtains, with the model as its procedure.

There is no default backend: without one, `search` ranks lexically and
over vectors callers supply, and says in its answer which ranking it
used. An OpenAI-compatible client follows after `0.1.0`.

## `VerbExecutor`

`name()`, `execute(invocation) -> value`. A verb whose declared effect
reaches outside the lake names an executor; when that verb runs (at once
if declared `low`, on acceptance otherwise, as
`docs/decisions/013-verbs-run-by-declared-risk.md` decides),
the store hands the executor the record, the payload and the position of
the event that made it due. The lake and that position are the
executor's dedup key. Its answer is recorded.

There is no default: a verb whose executor is not bound is refused as
`Unbound`, never faked.

## `ChangeFeed`

`poll(from, filter, limit) -> batch` and `ack(consumer, through)`. The
feed is the log filtered by kind and type, read from a position
(`docs/decisions/004-the-change-feed-is-the-log.md`). `poll` answers at
once, possibly empty, with the position to poll from next and the head.

The consumer keeps the position it has read up to. The store's own feed
keeps no consumer state, and its `ack` is accepted and does nothing; `ack` exists for feeds
that relay elsewhere (a remote client, a bridge onto a bus) and need to
know what they may release. The dedup key of a change is its lake and its
position.

## `SystemVocabulary`

`types()` lists a host's own `sys.` types; `may_hold(lake, type)` says
whether a lake may hold one. Lakelands reserves only the four system
types its fold reads: `sys.source`, `sys.provenance`, `sys.setting`,
`sys.aging_rule` (`docs/decisions/003-reservation-by-namespace.md`). The
default adds none and allows those four everywhere.

A `sys.setting` record is one named setting of a lake. Its record key is
the setting's key, and it has two fields: `key` (text, the same key) and
`value` (json, the setting's value). `setting('vat_rate')` in any
expression reads the `value` of the `sys.setting` record with key
`vat_rate` as of the position being read or folded, so a replay reads the
setting as it then stood.

## The facade and the doors

Two more traits sit above the seams. They are what callers and hosts
hold, not what hosts implement:

- `Catalog`: the doors of one lake, for any caller, every call naming its
  principal. Tool names for LLM-facing transports come only from
  `lakelands_api::tools::tools`.
- `LakeStore`: one open lake, as its host holds it; it hands out a
  `Catalog` bound to a `WriteContext`, the change feed, and lake
  maintenance (`verify`, `rebuild`, `checkpoint`).

The `seam-width` gate counts the methods of `LakeStore` and the three
engine traits together, and holds them at 110 or fewer. `Catalog` is not
in that count: its width is governed by the four-doors rule
(`docs/decisions/014-four-doors-per-type-one-name-generator.md`).
