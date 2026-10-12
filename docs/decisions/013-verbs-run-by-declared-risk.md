# 013 — A verb runs by its declared risk

## What is decided

### A verb's declared risk, and nothing else, decides whether its call runs or is proposed

Every verb declared in the ontology declares a risk. When a verb is
called, the store looks at that declaration and nothing else:

- a verb declared `low` whose effect is writing authored fields runs at
  once, as a governed write under the caller's grants;
- a verb declared `low` whose effect reaches outside the lake runs at
  once, like any `low` verb, through the `VerbExecutor` the host bound
  for it; the executor's answer is recorded in the log;
- a verb with any other risk does not run. The call is recorded as a
  `WriteProposed` event, and it takes effect only when someone with the
  right to do so accepts it (`WriteAccepted`) or is discarded when they
  reject it (`WriteRejected`).

Declared risk is the only thing that decides between running and
proposing, whatever the effect.

### A verb that reaches outside the lake runs only through an executor the host has bound

A verb whose effect reaches outside the lake runs only through a
`VerbExecutor` the host has bound; without one, the store says so
rather than pretending to run it. A verb that reaches outside the lake,
of any risk, is refused as `unbound` when no executor is bound for it,
whether it is called or a call of it is proposed through `propose`
(lakelands#345): never faked, and never recorded as a proposal, because
a proposal that could not run when accepted would be a promise the store
cannot keep.

### A call is proposed only if it could run now, and an accept checks the write again at the head

A call is proposed only if it could run when it is proposed. A call that
becomes a proposal, through `call` at its declared risk or through
`propose`, is checked as if it ran now: its record's version against the
expected one, then its `requires` on that record. A stale version is a
conflict and a `requires` that does not hold is a precondition, and
neither is recorded as a proposal (lakelands#338).

Accepting a proposal applies its write as it was logged, whole, in the
proposer's name and not under its grant
(`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`).
At acceptance the store checks the write again at the head, as the
order of refusals lists it, and records the write with the proposer as
its writer, caused by the acceptance. A call that reads across a
reference reads the far record as it stands at the accept
(`docs/decisions/055-a-path-steps-through-a-reference.md`): a
`requires` it no longer meets refuses the accept as a precondition, and
a far record no longer reached refuses it `not_live`; either leaves the
proposal pending. A verb's write, run at once or on
acceptance, fires triggers and changes object sets exactly as any other
write does (`docs/decisions/004-the-change-feed-is-the-log.md`).

### A proposal makes or retires no link, and the accept of a call on a link judges the link and both ends again

A proposal makes or retires no link: an upsert or a retirement of a
link type is refused at `propose` as the wrong door
(`link_records_are_made_through_the_link_doors_and_ingest_only`,
lakelands#847). The store judges a proposed call on a link again at the
accept, whatever the verb's effect: the link live and answered to the
proposer, and the reviewer by the records the call reaches, the link
and its two end records among them
(`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`).
A verb on a link, whatever its risk or effect, finds both ends live and
answered to its caller, at the call and again at its accept, as `link`
with a field does: a call, or the accept of one proposed before an end
was retired, is refused as `not_live` on that end
(`a_verb_on_a_link_type_keeps_what_the_link_door_keeps`, lakelands#847,
lakelands#935). A verb that sets fields of a link type sets a live
link's own fields; one that reaches outside the lake writes no record,
and is held to the same ends: an executor is handed no link whose end
is gone. Cardinality is judged only where a link is made, by `link` and
by ingest, since no verb moves an end.

### An executor's failure is recorded as its output, and a call no executor was handed is answered so

An external verb is due at the position of the event that made it due,
and it stays due: an executor's failure does not take that event back.
The failure is recorded as the executor's output — `{"error": "<its
error text>"}` in the `output_recorded` event that position causes — and
the caller's outcome is applied at that position with the recorded
output in it, the error form included: an error reply would say nothing
was written when the position is real, and a caller should not have to
read the log back for the answer. The verb is not run again for that
position; a call sent again without its idempotency key is a new call
and a new position. So the log holds, for every external verb, either
what the outside world answered or that it did not.

A call whose own fold failed is in the log, and no executor was handed
it. The store does not answer that call as if it were: its sentence
says that the verb's executor was not handed the call, that sending it
again under its key will not run it, and to call the verb again under
another key or with none. An acceptance is answered so in its own words
(the proposal is decided, accepting it again will not run it, and the
call is proposed again). The same holds where the call's own fold
succeeded and the drain of due calls before the hand-out failed: an
executor is handed a call only when its answer can be recorded, and
after such a drain the writer may be behind.

### A retry never hands a position out again

A retry with the call's idempotency key is not a new call. It answers
the first outcome: the write the verb made, or an external verb's
position and its recorded output where one is recorded. What a call
wrote itself is told by what the store appended with it, the write
right after its `verb_called` carrying the verb as its procedure. It is
never told by a later write that names the call as its cause, as a
host's write of the effect's result does
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`,
lakelands#305).

A position reaches its executor at most once, and a retry runs nothing:
a call sent again under its key, or an acceptance sent again, answers
by what is recorded for its position. Where an answer of the executor
is recorded, the retry is `applied` with it, `null` included. Where none
is, the retry is refused as `unanswered`, never `applied`: one sentence,
true whichever way the position got there, says that the call is in the
log at that position, that no answer is recorded and that the retry
runs nothing; that a call still running, or whose answer is held, will
have its answer recorded and a retry then answers it, unless the handle
that ran it is dropped or loses its lease first; and that a call whose
first answer said its executor was not handed it is called again under
another key or with none, or proposed again. A retry may not tell the
two apart, and the caller may: it was given the first answer.

The answer is found by a seek, not by a read of the log after the call.
It is recorded under an id derived from the lake and the position that
made the verb due, and an acceptance under one derived from the lake
and its proposal (`docs/log-format.md` §3), so what a retry reads under
the writer's lock does not grow with the log. A retry that read up to
4,096 events after the call took 74 to 90 ms on a lake of 5,000 events
against 0.1 to 1 ms for the seek, on the three engines.

An accept sent again answers by the verb as it stood at the acceptance,
told by what the acceptance appended (a write verb's write follows its
call, caused by it), whatever was declared since: an external call is
answered as a retry of a call is, and any other acceptance is
`decided`.

### An executor is handed the record its call was judged on

An executor is handed the record as it stood at the position that made
its verb due, the `verb_called` or the `write_accepted`, where the call
was judged; not as the head holds it when the hand-out comes. A write
between the two, another caller's or a retirement, does not reach the
executor, so it never acts on a record its call was not judged on: a
record retired after the call is handed as it stood. A purge appends
and removes nothing, and compaction keeps the projection's history, so
that state is always there to read; there is no fallback to the head.

### An executor's answer is held until it is recorded

An executor's answer whose append the engine refuses `unavailable`, or
that a writer which cannot catch up with its log cannot append, is not
lost to the handle that ran it: the writer holds it, and the next
write that takes the writer appends it, the oldest first, before it is
decided. A call sent again under its key therefore records the held
answer and is answered it; a neighbour's write, an ingest, an aging run,
a purge, the drain and the preflights of `migrate` and
`migrate_value_type` record it too, since each takes the writer; a
preflight that does moves the head, so a purge's run is then refused
until its preflight is run again, as after any neighbour's write. The
first answer says so: the answer is held and recorded before the next
write, and sent again under its key, or accepted again, the call answers
it. A held answer leaves the writer once the engine holds it, before it
is folded, so a fold that fails is told as any append's is, and the
write that recorded it catches up before it is decided. One that is
refused again stays held and stops no other write: nothing in the log
depends on it. Any other refusal is answered and the answer is not held.
The answer stays where the handle is: closed or dropped before a write
records it, or its lease lost, the handle loses it, and a retry is then
`unanswered`; the first answer carried the output.

Handing a position out again where no answer is recorded was weighed
and not built. It would change what an executor is promised, once per
position, to once per position unless the store asks again; and under
which declaration, by whose grant, on which handle and while which other
hand-out still runs it would happen are each a rule a host would have to
read. A host that needs it carries its effect under a key of its own and
calls the verb again.

### A declaration is proposed like a write

A set of declarations, or a type's migration, can be proposed as a
write is (lakelands#199). The proposal is checked when it is made as its
door would check it: the set against the ontology at the head as `apply`
checks it, the migration by its preflight, and a migration that would
lose information is refused with its report. A proposed migration keeps
that report, so a reviewer reads what it would move and what it would
drop. Nothing about the ontology changes until the proposal is accepted,
and a proposer needs no `schema` grant: proposing is how a principal
without it asks.

Accepting one needs `schema` as well as `review`
(`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`).
The accept checks the set again against the head and runs the
migration's preflight again, since the lake may have moved. A change the
head no longer admits refuses the accept, naming why, and the proposal
stays pending. What the accept declares is logged in its append as the
proposer's write, caused by the acceptance, and takes the next versions
and schema hashes there, not at the proposal.

A proposed shape, a migration's or a redeclared type's, is logged as
the store completed it from its proposer's view
(`006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there`).

A proposed migration is accepted only on the shape it was proposed on
(lakelands#489). Its `to` is the type's whole shape, so on a type
declared or migrated since it would remove whatever was added, and the
report the reviewer read says nothing of that. The accept compares the
type's schema hash at the head with the report's `from_schema_hash` and
refuses a difference as invalid, saying the type changed and the
migration is proposed again; nothing is appended and the proposal stays
pending. Records written since are the second preflight's to judge.

### A value type's tightening is proposed like a migration

A tightening is the third change of the ontology that can be proposed,
beside a set of declarations and a type's migration. The proposal
carries the value type's new declaration. It is preflighted when it is
made, without the `schema` grant, and the report replaces whatever the
proposer sent and is logged with the proposal, for the reviewer.

- **A report that does not fit refuses the proposal,** as invalid, and
  nothing is appended. The refusal says how many stored values would not
  fit and names none
  (`006 § A report is answered as its reader may know it`).
- **Accepting it takes `schema` beside `review`,** as for the other two.
  It is preflighted again on the head and refused the same way when a
  value no longer fits. It is also refused when the value type was
  declared again since the proposal: `to` is the value type's whole
  declaration, and a value the set gained in between would leave it
  again, shown to no one. Either refusal leaves the proposal pending.
  Accepted, it writes what the apply door writes: one `declared` at the
  next version, as the proposer, caused by the acceptance.
- **A listing shows the report's uses only in fields its reader may
  read,** of types it may see
  (`006 § A report is answered as its reader may know it`).
- **The proposal came with a step of the log format**
  (`docs/log-format.md` §6).

Without the proposal, a value type that is to accept less could be
changed only by a principal holding `schema`, through its preflight and
apply (027). A principal without the grant could propose that a set
gains a value and not that it loses one, and the refusal of the narrowed
declaration pointed at a door it may not use.

### A verb may be declared on its type, and is called with no key

An effect that creates something outside the lake has no record to be
called on yet: a payment made in another system, a ticket opened, a
customer registered upstream before the lake holds them. Calling such a
verb on some other record would take its grants from the wrong type and
file the effect in the wrong history. Running it outside the lake would
lose the declared risk, the proposal, the idempotency and the recorded
output. So a verb may be declared on its type, `scope: type`, and it is
called with no key.

- **Grants.** They are its type's, as for any external verb: write
  access to `on`
  (`006 § A verb is callable and described only to one who may write what it sets and read what it reads`).
- **Risk.** It decides as it does for every verb. `low` runs at once;
  anything else becomes a proposal whose call carries no key, and
  accepting it runs the verb as the proposer.
- **The log.** `verb_called` carries no key. The executor is handed no
  record, and its answer is recorded as for any external verb, caused by
  the event that made it due. A retry with the same idempotency key
  runs nothing again and answers by what is recorded for its position,
  as for any external verb.
- **Only an external effect.** A verb on its type reaches outside the
  lake. A `write` effect sets fields of a record, and `requires` reads
  one, so a declaration with either is refused as invalid, naming the
  verb. Creating a record in the lake needs no anchor: it is `upsert`,
  or a proposal of one.
- **The key must fit the scope.** A key passed to a verb on its type, no
  key passed to a verb on a record, and an expected version with no key
  (there is no record version to compare) are each refused as invalid,
  naming the verb. This is checked before grants are read or anything is
  appended.
- **A redeclaration between a proposal and its acceptance.** A pending
  proposal can outlive a change of the verb's declaration. Accepting
  checks the scope at the head again, and a mismatch refuses the accept
  with the same text; the proposal stays pending.
  Beyond the scope, the reviewer applies exactly what was checked: if
  the verb is declared differently at the head than it was when the
  call was proposed (another effect, another payload), the accept is
  refused as invalid, naming the verb, and the proposal stays pending;
  the caller proposes again (lakelands#371). The store compares the
  declaration's content, as the lake held it at the proposal's position
  and at the head, so a verb declared again unchanged, or changed and
  changed back, as a host that applies its bundle at every start does,
  leaves its pending calls acceptable. No event carries more for it.

The default scope, `record`, is never written, so every declaration made
before keeps its bytes.

### The verb and proposal doors refuse in one order

The verb and proposal doors refuse in this order, in the frame every
write door shares (016): the lake, the context, the door's own checks,
the append. Where a step runs "the record checks", they are 016's, on
the record the write makes.

**`call`**: the retry; the verb is declared (`not_found`); the key fits
its scope (`invalid`, naming the verb); the principal may call it
(`not_granted`), which for a call that is proposed includes that a
holder of `only` proposes what can be pinned to its record
(`006 § A grant may admit only the records that hold its pairs`); an
external verb's executor is bound (`unbound`); the
payload: a member the verb does not declare (`invalid`, naming the verb
and the member, as one missing from the payload and never from the type,
even where the type has a field of that name), then field by field in
name order a value that does not fit (`constraint`, naming the field),
then its required fields (`constraint`); on a record: the record
is live (`not_found`), the expected version (`conflict`), `requires`
(`precondition`). A verb with a write effect then computes its
assignments (`invalid`, naming the verb) and takes its write through
the record checks. A `low` verb's call is appended after that, and at
any other risk the call is proposed after it: what could not run now is
not proposed.

**`propose`**: the retry; a holder of `only` proposes no change of the
ontology (`not_granted`,
`006 § A grant may admit only the records that hold its pairs`); an
upsert or a retirement of a link type (`wrong_door`); for any other
upsert or retirement, the type is shown to its proposer (`not_found`),
then a holder of `only` proposes what can be pinned to its record
(`not_granted`, as for a call); then the write as its own door checks
it: an upsert through the record checks; a retirement as `retire` from
the type on; a call as `call` from the verb on, its executor included,
and for a write effect through its assignments and the record checks; a
declaration as `apply` checks a set; a migration by its preflight, then
`migration` if it is not lossless; a value type's tightening by its
preflight, then, with renames, that its proposer is shown every field
they write (`not_granted`,
`006 § A declaration or a migration is its author's view`), then
`invalid` with the count if a stored value would not fit. No `schema`
grant is needed to propose a change of the ontology.

**`accept`**: the `review` grant (`not_granted`); the proposal exists
(`not_found`); it is there to its reviewer, where one that is not
answers as an id that does not exist (`not_found`,
`006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there`);
it is its reviewer's to decide, where a reviewer without `restricted`
decides no write that reaches a held record, and a holder of `only` no
change of the ontology and no write to a type it holds `only` on that
it is not listed whole (`not_granted`,
`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`);
it is pending (`decided`; an accept sent again of an external verb's
call is answered as a retry of a call is); a change of the ontology
also needs `schema` (`not_granted`); a pinned proposal's record, its
type and, for a link, either end record stand as they were proposed
(`invalid`,
`006 § A grant may admit only the records that hold its pairs`);
then the write, checked again at the head: an upsert through the record
checks; a retirement for its record and version; a call for its verb,
its scope, the verb declared differently since (`invalid`), its
executor, then as `call` from the record on; a declaration as `apply`
checks a set, then its set changes (`too_large`); a migration for the
shape it was proposed on (`invalid`, lakelands#489), then by its
preflight, then for what it restates against what its report restates
(`invalid`, 027), then `migration`, then its set changes (`too_large`),
then its embedding model (`unbound`); a value type's tightening for the
declaration it was proposed on (`invalid`), then by its preflight,
then, with renames, for the fields its report named (`invalid`, 027),
then `invalid` with the count, then, with renames, its set changes
(`too_large`). Any refusal leaves the proposal pending.

A refusal of an upsert's, a retirement's or a call's write at the
accept is told as its reviewer's listing shows the write, and what an
upsert or a verb's write leaves lies within the pairs of a reviewer
holding `only` (`not_granted`), judged once the write is planned
(`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`).
A declaration and a migration are judged first for a tool's name taken
since by what the accepter is not shown (`not_granted`,
`006 § A declaration or a migration is its author's view`).

**`reject`**: the `review` grant, the proposal exists, it is there to
its reviewer and its reviewer's to decide, as at `accept`, it is
pending.

Two steps that answer the same kind for the same verb or field, such as
the scope and a verb declared differently since, are stated here and not
checked between themselves.

### `describe` says whether a verb's executor is bound

A verb that reaches outside the lake is described with its executor's
name and whether the host binds an executor of that name, and its tool
says which. Without this, a caller learns that a verb cannot run in a
lake only by calling it and being refused, and a host's interface cannot
show the verb as unavailable beforehand. The name is no news to the
principal: the verb is described only to one who may call it
(`006 § A verb is callable and described only to one who may write what it sets and read what it reads`),
and the `unbound` refusal names the executor to that principal anyway.
A verb that sets fields of its record has no executor and is described
without one. The tool of an unbound verb stays in the catalog, so a host
that binds the executor later changes the tool's text, not the set of
tools.

`bound` is the binding when the answer is made, at whatever position
`describe` is read. The binding is the host's configuration, not a fact
of the log, so there is no earlier answer to give; and a call always
runs at the head, so whether a verb can run is a question about now.
The reply's position covers the declarations, not the binding: two
answers at one position differ in `bound` when the host rebinds between
them, as `search`'s `ranked_by` already reports the embedding backend
bound when it ranks.

A function's backing and an embedding model are not described the same
way. Both are checked when they are declared, so a lake does not hold one
its host never bound. No caller calls a function: a missing backing
leaves the fields it computes due, and the host that unbound it is the one
who can see that. A search already says how it ranked.

## Why

A verb's declaration is where its author says how dangerous it is. A
default that ran anything the store could technically execute would
ignore the one piece of information meant to stop it. Making the
proposal a state in the log, rather than a queue in some other system,
means a pending action survives restarts, can be listed and audited like
any other write, and is accepted or rejected by the same principals and
grants as everything else.

## What it is not

- It is not an approval workflow. Who reviews a proposal, and how they
  are notified, is the host's business; the store records the proposal,
  the acceptance and the rejection.
- A caller cannot lower a verb's risk at call time. The declaration
  decides.
- `propose`, `accept` and `reject` are also plain doors: any write can
  be proposed rather than applied, not only a verb.

## What moved

What a principal is shown and told of a proposal, and which right its
listing and its accept need, moved to
`docs/decisions/006-grants-not-roles.md` at lakelands#817.

| what this record held | where it is now |
|---|---|
| what acceptance checks, its first sentence: whose grants an accept applies the write under | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| who sees which proposals: a principal with `review` lists the proposals, any other its own | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| who sees which proposals: who may accept or reject, and that any other principal decides none | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a proposal is read as its reader's view (lakelands#499): the write as its reader may read the record, `more`, the entry without its write, the absent write as a form of the answer, the filters, a proposed bundle | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| a proposal is read as its reader's view (lakelands#499): a reviewer need not know all of what it accepts, and the accept applies the write whole | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a proposal is read as its reader's view (lakelands#499): the refusal at the accept for a declaration that reads a field the migration takes away | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a proposal is read as its reader's view (lakelands#531): an accept refused as no longer lossless carries the fresh report as the accepter may know it | `006 § A report is answered as its reader may know it` |
| a proposal on a type its reader is not shown is not there (lakelands#762): not listed, its accept and reject as an id that does not exist, the types a proposal names, the three sights of a migration, why | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| a proposal on a type its reader is not shown is not there (lakelands#762): no answer that reports on a type's migration is given to one not shown the type, and its report needs no form with the type taken out | `006 § A report is answered as its reader may know it` |
| a call of a verb that reads a field its reader may not read is not there (lakelands#779) | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| an entry is decided as it is listed (lakelands#798, lakelands#791, lakelands#893): one answer for the listing, the accept and the reject; three sights; what turned; the rows | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| an entry is decided as it is listed (lakelands#798): one listed without its write, or to a holder of `only` not whole, is not decided; what an accept applies | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| an entry is decided as it is listed (lakelands#798): a migration is not listed to a holder of `only`, and its accept and its reject answer as an id that does not exist | `006 § A grant may admit only the records that hold its pairs` |
| an accept's refusal names what its reviewer's listing shows (lakelands#780, lakelands#791) | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a value type's tightening (lakelands#476): the refusal of a report that does not fit names no misfit, and why | `006 § A report is answered as its reader may know it` |
| a value type's tightening (lakelands#476): what a listing shows of the report, its totals whole | `006 § A report is answered as its reader may know it` |
| a declaration is proposed like a write (lakelands#199): why accepting one needs `schema` as well as `review` | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a declaration is proposed like a write (lakelands#475): `proposals` shows a proposed shape to each reader without the fields that reader may not know of | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| a verb called on its type (lakelands#277): its grants are its type's | `006 § A verb is callable and described only to one who may write what it sets and read what it reads` |

## How it came to be

- The first version: a verb's declared risk decides whether its call
  runs or is proposed; a `low` verb that reaches outside the lake runs
  through its bound executor; an accept writes as the proposer; an
  executor's failure is recorded as its output.
- A set of declarations and a type's migration can be proposed
  (lakelands#199).
- A verb can be declared on its type and called with no key
  (lakelands#277).
- A retry with the call's key answers the call's own outcome, where a
  later write that named the call as its cause was taken for it
  (lakelands#305).
- A call of a verb whose executor is not bound is refused at `propose`
  too, where only a call was (lakelands#345).
- A call that could not run now is not proposed, and the order of
  refusals was stated (lakelands#338).
- An accept refuses a call whose verb was declared differently since
  (lakelands#371).
- `describe` says whether a verb's executor is bound (lakelands#432).
- A proposed shape is logged as completed from its proposer's view
  (lakelands#475).
- A proposed migration is accepted only on the shape it was proposed on
  (lakelands#489).
- A value type's tightening can be proposed, where only a principal
  holding `schema` could make a value type accept less (lakelands#476).
- A call whose own fold failed, or whose hand-out a failed drain
  stopped, is answered that no executor was handed it (lakelands#603).
- A retry never hands a position out again and is refused as
  `unanswered` where no answer is recorded, found by a seek, where a
  retry read up to 4,096 events after the call and answered the
  position with no output (lakelands#616).
- An executor's answer is held until it is recorded (lakelands#646).
- A payload member the verb does not declare is refused naming the verb
  and the member (lakelands#656).
- An executor is handed the record its call was judged on, where a
  record retired after the call was refused (lakelands#660).
- The order of `call` and of `accept` lost the step for a verb called
  on a record of a view (lakelands#823).
- The order of `call` names that a holder of `only` proposes what can
  be pinned (lakelands#799).
- The tightening's bullet on the log format names a step, where it
  named the format's number (lakelands#801).
- Moved at the rewrite (lakelands#817): the sections and sentences on
  what a principal is shown and told of a proposal moved to 006, as
  "What moved" lists them; the rules that stay were put under headings
  of their own.
- Corrected at the rewrite (lakelands#817): "Accepting a proposal
  applies it under the proposer's grants as they were when it was
  proposed". The proposer's grant is not in the log; the accept writes
  in the proposer's name, not under its grant.
- Corrected at the rewrite (lakelands#817): "At acceptance the store
  checks again only what can have moved since — the type's shape and
  the record's version". The accept checks the write again at the head,
  its record judged again, as the order of refusals said.
- Corrected at the rewrite (lakelands#817): "a retry is a new call and
  a new position" holds for a call sent again without its key; and a
  retry "answers the first outcome", here and of a verb on its type,
  only where an answer is recorded, and is else refused as `unanswered`
  (lakelands#616).
- Corrected at the rewrite (lakelands#817): "A link's cardinality is
  judged only where a link is written, and no proposal writes one". An
  accepted call of a verb that sets fields of a link type writes the
  link's own fields (lakelands#759), and its accept judges the link and
  both ends again (lakelands#935 made the ends live for every verb, see
  below). Cardinality is judged where a link is made. The
  order's "a link type at an ordinary door" at `propose` is an upsert
  or a retirement of one.
- Corrected at the rewrite (lakelands#817): "At any other risk the call
  is proposed after `requires`". A call above low risk is judged as its
  caller's write before it is proposed, its assignments and the record
  checks included (lakelands#781).
- Corrected at the rewrite (lakelands#817): the order of `accept` and
  of `reject` left out the steps later changes added: whether the entry
  is there to its reviewer (lakelands#762) and its reviewer's to decide
  (lakelands#660, lakelands#772, lakelands#781), an accept sent again
  (lakelands#616), the pin (lakelands#660), a reviewer's pairs, the
  refusal told as the listing shows (lakelands#780), a tool's name taken
  since (lakelands#896), what a migration restates (lakelands#554), the
  set changes of a declaration (lakelands#453), of a migration
  (lakelands#457) and of a value type's renames, and a value type's
  renamed fields (lakelands#553).
- Corrected at the rewrite (lakelands#817): the order of `propose` left
  out the steps later changes added: that a holder of `only` proposes
  no change of the ontology (lakelands#634), that the type of an upsert
  or a retirement is shown to its proposer, the step that stands before
  the pin, and that a holder of `only` proposes one only where it can be
  pinned (lakelands#660), and that a value type's renames are proposed
  only by one shown every field they write (lakelands#553).
- An accept reads a record its call reads across a reference as it
  stands at the accept (lakelands#567).
- A verb on a link finds both ends live whatever its effect
  (lakelands#935): one that reaches outside the lake ran on a link whose
  end was retired, at the call and at the accept of one proposed
  before, its ends judged only as not held from its caller, since they
  were judged live in the record's write, which such a verb does not
  make.
