# 015 — Errors are a closed set, and every write answers an outcome

## What is decided

A write never makes its caller guess what happened. `upsert`, `retire`,
every verb call and every schema change answer an outcome: `applied`,
with the position it is visible from, or `proposed`, with the proposal
and the reason it waits. A writer does not need to know beforehand
whether its write will run or wait for review.

Every failure is one of five errors, and the set is closed:

- `refused`: the store would not do it, with the field it concerns and a
  reason from a closed list (no grant, another authority writes the
  field, a reserved name, a request that does not fit the lake's map, a
  broken constraint or precondition, a reaction chain too deep, a
  migration that would lose information, a proposal already decided, a
  retry of a call of an external verb whose executor's answer is not
  recorded, a position outside the log);
- `not_found`: a type, record, verb or proposal does not exist at the
  position read;
- `conflict`: the record is not at the version the write expected;
- `unbound`: the call needs a seam the host has not bound;
- `unavailable`: the lake cannot answer now (another writer holds it, it
  is read-only here, its storage failed), with a message for the
  operator.

Each error has a sentence for a reader who is not an engineer. No error
carries an engine's own code or a free-form error string as its type.

## Why

An agent acts on what a call answers. If a write can silently become a
proposal, or fail with a string that only a developer can interpret, the
agent either stops or guesses. A closed set of outcomes is one that every
door, every transport and every client can handle completely, and the
compiler checks that they do.

Keeping engine errors out of the set keeps engines interchangeable: a
caller cannot come to depend on a SQLite error code that the in-memory
engine will never produce.

## What it is not

- A proposal is not an error. It is a normal answer to a write.
- `unavailable` is not a place for errors that belong elsewhere. A
  request that is wrong is `refused`; `unavailable` is for a lake that
  cannot answer a correct request right now.
- Adding a variant is a change to the contract, not an extension point.
  Because the set is closed, a host's exhaustive `match` breaks when a
  release adds one, which is what makes it handle the new case. That is
  why every release names such an addition in its notes, under "Changes
  a host must make" (lakelands#260).

## Extended: a refusal's sentence is not part of the contract

A refusal's kind and the field it names are the contract; its sentence
is for people and belongs to the implementation. The mock and the store
word their refusals each their own way, and no door test pins a
refusal's sentence: a test checks the kind, the field and, for a door,
its own framing lines. What would otherwise tie a second implementation
to the first one's wording is left free.

## Extended: an outcome says what the write set off (lakelands#431)

A write that fires a trigger or moves its record into or out of an object
set appends those reactions after its own event, in the same append. The
answer named only the write's own event. A caller that read next was then
answered at a later position and could not tell its own doing from someone
else's. An applied outcome now says what the write set off, in members left
out when it set off nothing, so such an answer reads exactly as before:

- `through`: the last position of what the write set off, when that lies
  after `position`. Within the append, everything between the two is the
  write's own doing. For an external verb, `through` is the position of
  its executor's recorded output. That output is recorded in a later
  append, so other writers' events may lie before it: a read at or after
  `through` sees all the write set off, and nothing more is promised.
- `fired`: the triggers fired, in the order fired.
- `joined` and `left`: the object sets the record joined and left.

What the write set off is its reactions, and an external verb's output;
nothing else in the append counts. A declaration that brings link types
with it, or a migration that migrates them, appends those as part of the
write itself, so their answers stay as they were.

The answer names a trigger or an object set only if `describe` lists it to
the caller. One whose condition reads a field the caller may not read is
not named, since naming it would tell the caller that the condition held
(024). Using `describe`'s own rule means the two answers cannot drift
apart. `through` still covers what is not named: positions are no secret,
and every reply carries its own.

A backed function's fill is not named. It is recorded in a later append,
after the drain, and what it fires is that append's. The answer's
`position` already moves to the drain's head, so a read there sees it.

A retry with the write's idempotency key answers the same members, read
back from the log, and judged by the caller's grants at the retry, as any
read is. Accepting a proposal answers what the write it applied set off.
An ingest window's answer already names the window's last event, its
reactions included, and gains nothing: a host follows reactions in the
feed.

## Extended: why a lake cannot answer, and the seam a call needs, are cases (lakelands#650)

A host renders a failure in its users' language, and its assistant acts
on what failed. Neither can do that from an English sentence. So
`unavailable` carries a `Situation` and `unbound` a `Seam`. Each is a
closed set of cases with their parameters as data: which lease and who
holds it, which part of the log is damaged and how (a closed `Flaw`),
which remote end failed and in what exchange, which seam and which name.

- **One sentence source.** A case's sentence is its `Display`, and no
  sentence is built anywhere else. The store, the engines, the mock, the
  backings and the client build the case. A door that advises what to do
  decides by the case, never by the words.
- **On the wire** a case is an object: its `code`, its parameters, and
  the `message` its sentence makes. An error carries its own `message`
  beside its data too. A code a reader does not know, from a newer lake,
  is read as `Unknown`, keeping the code, the parameters and the
  message, and written back as it came. Every closed set has that
  fallback, the error's kinds, a remote end and a tool name's problem
  included, and whether a code is known is decided by the codes the set
  declares, never by the words of a parse failure. What a reader does
  not know stays where it is: a known case keeps its code and the
  parameters it knows around it.
- **A plain name it does not know** (a part, a doing, an act) is kept as
  it came and written back so. It is said as itself in backticks, which
  no known name's sentence is. A known case around it is said again from
  its parts, so a client older than the lake shows the name in backticks
  where the lake's own sentence has words.
- **A sentence names one case.** No two cases, or one case with other
  parameters, render the same sentence. A case is split later only where
  its sentence splits too.
- **Foreign words are scrubbed at their source.** The only free text a
  failure carries is a `Detail`: what an operating system, a database, a
  TLS library, an HTTP client or a parser said. It is made by
  `Detail::io` (the error's kind and the system's own text), by
  `Detail::json` (what kind of parse failure and where, never the text
  the parser quotes, which may be a record's value), or by
  `Detail::scrubbed`. The last is called only in the files the
  `detail-sources` gate lists, each with why its words are safe: a
  PostgreSQL error as its severity, SQLSTATE and primary message, never
  its `DETAIL`; SQLite's messages, which name columns, never a row's
  values.
- **A `Detail` is read from JSON only by the client,** which reads a
  lake's answer. The `detail-sources` gate refuses a read of an `Error`,
  a `Situation` or a `Detail` by name anywhere else. A read whose type
  is only inferred is not seen by a gate and is the reviewer's to catch.
- **No path on the host reaches a caller from an outage.** A situation
  names a part of a lake (the log, its index, a segment, the lease file,
  an export), never the file. The operator, who reads the location,
  knows where it is. A remote end is named by its model, its function or
  its lake, never by an address or a path, which may carry a secret. Two
  refusals still quote a path (the SQLite engine's lake directory and an
  import's source); they become cases with the refusals.

This is a change for a host: an `unavailable` or `unbound` answer is an
object with a code, not a string, every error carries a `message`, and
some sentences changed their words. A client and a lake on either side
of this change cannot read each other's errors.
