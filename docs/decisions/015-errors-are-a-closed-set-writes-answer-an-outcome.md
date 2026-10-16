# 015 — Errors are a closed set, and every write answers an outcome

## What is decided

### Every write answers an outcome

A write never makes its caller guess what happened. `upsert`, `retire`,
every verb call and every schema change answer an outcome: `applied`,
with the position it is visible from, or `proposed`, with the proposal
and the reason it waits. A writer does not need to know beforehand
whether its write will run or wait for review.

### An applied outcome says what the write set off

A write that fires a trigger or moves its record into or out of an object
set appends those reactions after its own event, in the same append. An
answer that names only the write's own event leaves a caller that reads
next answered at a later position, unable to tell its own doing from
someone else's. So an applied outcome says what the write set off, in
members left out when it set off nothing:

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
write itself, so its answer names none of them as set off.

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
An ingest window's answer names the window's last event, its reactions
included, and carries none of these members: a host follows reactions in
the feed.

### Every failure is one of five errors

Every failure is one of five errors, and the set is closed:

- `refused`: the store would not do it, with the field it concerns and a
  reason from a closed list (no grant, another authority writes the
  field, only the type's verbs write the field (naming those its writer
  may call), a reserved name, a request that does not fit the lake's map, a
  broken constraint or precondition, a reaction chain too deep, a
  request that would reach, skip or change more than the lake allows, a
  migration that would lose information, a proposal already decided, a
  retry of a call of an external verb whose executor's answer is not
  recorded, a position outside the log);
- `not_found`: a type, record, verb, function or proposal does not exist
  at the position read;
- `conflict`: the record is not at the version the write expected;
- `unbound`: the call needs a seam the host has not bound;
- `unavailable`: the lake cannot answer now (another writer holds it, it
  is read-only here, its storage failed), with a message for the
  operator.

A record an ingest refuses is no kind of its own: its refusal is one of
these, with where the record stood beside it (`index`, `from`,
`record`, `docs/decisions/016-two-write-doors-by-origin.md`). On the
wire it keeps its kind's code and shape, so a reader that keys on the
kind is answered as before; in Rust the refusal is wrapped once, and
`Error::unwrapped` answers it (lakelands#1043).

Each error has a sentence for a reader who is not an engineer. No error
carries an engine's own code or a free-form error string as its type.

### Why a lake cannot answer, and the seam a call needs, are cases

A host renders a failure in its users' language, and its assistant acts
on what failed. Neither can do that from an English sentence. So
`unavailable` carries a `Situation` and `unbound` a `Seam`. Each is a
closed set of cases with their parameters as data: which lease and who
holds it, which part of the log is damaged and how (a closed `Flaw`),
which remote end failed and in what exchange, which seam and which name.

- **One sentence source.** A case's sentence is its `Display`, and no
  sentence is built anywhere else. The store, the engines, the
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
  its lake, never by an address or a path, which may carry a secret. No
  refusal names one either: the SQLite engine's lake directory and an
  import's source are "the lake's directory" and "the export's
  directory".

### A refusal says a case

As `unavailable` and `unbound` do, every refusal of `invalid`,
`constraint`, `precondition` and `unanswered` says a case: it carries a
`Said`, a case and the places it is said at, instead of a sentence. On
the wire a refusal's `reason` carries `code`, `at` when it has places,
its parameters and `message`.

- **One catalog.** Every case is one entry of a closed set in
  `lakelands_api::error`: its code, its parameters and its template. The
  template is the case's only sentence, and nothing else builds one.
  `docs/refusals.md` is generated from the entries, so a host reads the
  catalog it renders from. The outage sets are entries of the same form.
- **A place is data.** "type `T`, field `f`: …" is a case said at
  `[type T, field f]`, outermost first. A check inside a place does not
  say it twice.
- **A sentence determines its case.** No two templates of a set can say
  the same text, which the `refusals` gate proves over every template
  with each parameter standing for what its type can say and each
  optional segment read both ways. A code splits later only where its
  sentence splits too, and never between a hidden thing and an absent
  one: a hidden thing is answered with the absent thing's case and the
  same parameters, so the case says no more than the words did, and the
  suite compares the two whole errors.
- **Codes are by meaning.** Sentences that said one thing in two ways
  are one case; sentences that say different things are different
  cases, even where they differ little. A host renders each code it
  knows; rewording to make fewer codes would only churn what hosts read.
- **A code keeps its meaning.** Once a release ships a code, it is not
  removed or renamed, and no parameter is taken away or changes its type
  or whether it is optional; a case may gain optional parameters only,
  and a reader ignores a member it does not know. A retired code is not
  used again. The gate compares the catalog with the page of the last
  release tag, which lists each parameter's type.
- **A code leaves by a recorded withdrawal.** A code goes only when
  the product can no longer say its sentence: what it told of is gone,
  as a refusal about a range of log formats is once a release reads
  one. A rename is never a withdrawal; a code whose sentence is still
  true keeps its name. The withdrawal is a row in
  `scripts/refusals.withdrawn`: the set, the code, the tag it shipped
  in, the release it leaves in, and why. The gate refuses a shipped code
  that is gone without its row, and a code that has a row and is in the
  catalog, so a row cannot be written ahead of the removal and a
  withdrawn code is not used again. The rows stay for good, and
  `docs/refusals.md` lists them under "Withdrawn": a host may still meet
  such a code from an older server, and reads it as it reads any code it
  does not know. The release's notes name the codes that left.
- **A code is the case's name.** A case carries no serde attribute of
  its own, and no parameter takes a name the wire gives the case itself
  (`code`, `message`, the set's tag, and for a refusal's case `at` and
  `refusal`), so every case reads back as itself; a test reads one of
  each, generated from the catalog, back through an error.
- **A reader keeps what it cannot read.** A known code whose parameters
  a reader cannot read (a newer lake's change) is read as `Unknown` with
  its message, as an unknown code is, and a case nested in another is
  read the same way, so the error around it still reads.
- **A parameter is a value, not a sentence.** A parameter of text is a
  name or a value the caller gave, never words built elsewhere; the gate
  refuses a case built from `format!`. Foreign words are a `Detail`, as
  in an outage.
- **The suite compares cases:** a door check asserts the case a refusal
  says, with its places and parameters. A row keeps words only where
  the words are its subject.
- **What stays out of the catalog, and why.** Each of these is said to
  the operator who runs the tool, never answered by a lake to a caller:
  a bundle's problems, read from the operator's own files by the CLI;
  the CLI's own terminal messages; the host's configuration errors at
  load time (a client's, an embedding backend's, a deliverer's); and the
  JSON-RPC protocol errors of MCP, which are the protocol's, not a
  refusal. A `not_granted` refusal carries no case on purpose: it never
  says which grant is missing, and its door words say only how to
  authenticate. A door's advice (`todo`) is a function of the case.

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
  a host must make".

## How it came to be

- The first version: every write answers `applied` or `proposed`, and
  every failure is one of five errors.
- A refusal's sentence was left out of the contract: its kind and the
  field it names were the contract, the mock and the store worded their
  refusals each their own way, and no door test pinned a sentence.
- A release names an added variant in its notes, under "Changes a host
  must make" (lakelands#260).
- An applied outcome says what the write set off, where the answer had
  named only the write's own event; the answer of a write that set off
  nothing reads as before (lakelands#431).
- The reasons of a refusal gained the retry of a call of an external
  verb whose executor's answer is not recorded (lakelands#616).
- `unavailable` carries a `Situation` and `unbound` a `Seam`
  (lakelands#650). For a host, an `unavailable` or `unbound` answer
  became an object with a code, not a string, every error came to carry
  a `message`, and some sentences changed their words; a client and a
  lake on either side of the change cannot read each other's errors.
- A refusal says a case, which replaced "a refusal's sentence is not
  part of the contract": the case is the contract (lakelands#650). Many
  sentences changed their words, so a host that matched words matches
  codes.
- The mock that worded its own refusals, and then built the store's
  cases, is retired (lakelands#800).
- A code leaves by a recorded withdrawal (lakelands#801).
- Corrected at the rewrite (lakelands#817): a refusal's reasons name
  the request too large, and what is not found names a function; the
  contract's `Refusal` and `Missing` hold both, and the lists left them
  out.
- A refusal's reason gained `verbs_only`, naming the verbs that alone write a field (lakelands#1130).
