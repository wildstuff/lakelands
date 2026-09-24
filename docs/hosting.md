# Hosting Lakelands in a platform

This page is for a platform that embeds Lakelands as its store: it
supplies identities, runs agents and workers, talks to the outside world,
and wants the lakes underneath to be Lakelands. It describes the same
seams that `lakelands-api` declares and `docs/seams.md` specifies, seen
from the host's side. If this page and the contract ever disagree, the
contract is right and this page is wrong.

## What a host implements

Lakelands has seven seams. A host implements the ones it needs and
passes them in `Seams`; every one but the engine has a default, and for
the engine a host passes Lakelands' own openers. A host that passes
those and nothing else has a working store. Two seams point the other
way: the doors hold the `PrincipalResolver`, and the store provides the
`ChangeFeed` for the host to read.

- **Engine** (`LakeEngine`, `ProjectionStore`, `ProjectionView`, with
  `VectorIndex` beside it as the engine's index trait). Usually nothing
  to implement: SQLite and in-memory engines ship with `0.1.0`, and
  PostgreSQL follows right after. A new engine is accepted when it
  passes the conformance suite.
- **`PrincipalResolver`.** Maps the host's identities, roles and
  policies onto one `Principal { kind, id, lake, grants }` per call.
  Grants are per field and per principal, and the store enforces them
  on every read and write, so the host never gates reads or writes
  itself. See `docs/decisions/006-grants-not-roles.md`.
- **`FunctionBacking`.** A backing is an open token. `0.1.0` ships
  `expr`, the built-in evaluator, and it is the only backing that
  computes fields in `0.1.0`: a field computed by another backing is
  refused as `Unbound` when it is declared. `http` and `exec` follow,
  and a host may add its own kind then; each output of such a backing is
  recorded in the log (`output_recorded`) and read back on replay, never
  recomputed. See `docs/decisions/002-determinism-is-the-boundary.md`.
- **`EmbedBackend`.** A host may pass vectors in with its writes or
  plug in an embedding client. In `0.1.0` a bound backend embeds the
  query text of a hybrid search that brings no vector of its own, which
  is a read and records nothing; without one, search is lexical and over
  caller-supplied vectors. Embedding fields as they are written, and an
  OpenAI-compatible client, follow right after.
- **`VerbExecutor`.** A verb declared `low` runs at once; any other
  lands as `WriteProposed`, and the store records `accept` and `reject`
  as events. A verb that writes authored fields runs in the store; a
  verb that reaches outside the lake runs through the executor the host
  bound for it, with the lake and the position that made it due as the
  dedup key. The executor gets the record unmasked; a host that wants it
  to see only what the caller may see applies the caller's grants there. The executor's answer is recorded in the log; a failure is
  recorded as `{"error": …}`, the write that made the verb due stands,
  and the caller gets the recorded answer either way. Without a bound
  executor such a verb is refused as `Unbound`, not faked. See
  `docs/decisions/013-verbs-run-by-declared-risk.md`.
- **`ChangeFeed`.** The host reads the store's feed: the log, filtered
  by kind and type, from a position; each poll answers at once with the
  position to poll from next. It acknowledges on its own side. The host keeps the
  position it has read up to; the store keeps no consumer state. A relay onto a message bus is the
  host's code, never Lakelands'; the dedup key of a change is
  `{lake}:{position}`. See `docs/decisions/004-the-change-feed-is-the-log.md`.
- **`SystemVocabulary`.** The host registers its own `sys.*` types and
  says which lakes may hold them; each time a writer opens a lake, the
  store declares the allowed types it does not hold yet.
  Lakelands reserves only the four types its own fold reads. See `docs/decisions/003-reservation-by-namespace.md`.

**Configuration is not a seam.** Engine configuration, tuning, limits
and an optional key handle reach a lake through `Location`, and a
server through its server configuration; Lakelands never reads the
environment. A host that keeps settings in its environment reads them
and passes them in. See `docs/decisions/005-no-environment-reads.md`.

## How a host switches onto Lakelands

A host that already has a store of its own moves in four steps.

1. **Freeze the old store.** From this point it takes bug fixes only,
   and every fix is logged so it can be ported to Lakelands before the
   switch.
2. **Implement the seams against a tagged `lakelands-api`** while the
   old store still serves. Each seam implementation gets a test that
   fails before it and passes after.
3. **Cut over in one step.** Consumers re-point to Lakelands, pinned by
   tag, never by bare revision. Any contract the host exposes to its own
   plugins is adapted onto `lakelands-api`. The old store and the
   duplicate doors built around it are deleted, not wrapped in shims.
   The port log is empty before this step starts.
4. **Walk the host's own end-to-end scenario** on the new tag before
   calling the switch done. Behaviour the conformance suite did not
   cover shows up here, and it is fixed in Lakelands, not worked around
   in the host.

## Rules the host inherits

- **One tool surface.** The catalog is the only tool surface. A host
  adds no second query door beside it and no hand-written tool names.
- **Invariants live in the store.** Field rights, per-field provenance
  and the `sys.` guard are enforced on every write by the store, so a
  host-side check is never a substitute and never needed. See
  `docs/decisions/010-provenance-per-field.md` and
  `docs/decisions/012-writes-touch-only-their-fields.md`.
- **One writer per lake.** A lake has one writing process at a time,
  held by a lease; one process may write many lakes. See
  `docs/decisions/008-one-writer-per-lake.md`.
- **No query across lakes.** A lake is one world with one truth; a host
  that needs data from two lakes reads each of them.
- **Every answer carries its position.** A host that records what an
  agent saw records the position, and reads `as_of` it later. See
  `docs/decisions/009-every-response-carries-its-position.md`.
