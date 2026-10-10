# 013 — A verb runs by its declared risk

## What is decided

Every verb declared in the ontology declares a risk. When a verb is
called, the store looks at that declaration and nothing else:

- a verb declared `low` whose effect is writing authored fields runs at
  once, as a governed write under the caller's grants;
- a verb with any other risk does not run. The call is recorded as a
  `WriteProposed` event, and it takes effect only when someone with the
  right to do so accepts it (`WriteAccepted`) or is discarded when they
  reject it (`WriteRejected`).

A verb whose effect reaches outside the lake runs only through a
`VerbExecutor` the host has bound; without one, the store says so
rather than pretending to run it.

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

## Extended: a `low` verb that reaches outside the lake

A verb declared `low` whose effect reaches outside the lake runs at
once, like any `low` verb, through the `VerbExecutor` the host bound for
it; the executor's answer is recorded in the log. Declared risk is the
only thing that decides between running and proposing, whatever the
effect.

A verb that reaches outside the lake, of any risk, is refused as
`unbound` when no executor is bound for it, whether it is called or a
call of it is proposed through `propose` (lakelands#345): never faked,
and never recorded as a proposal, because a proposal that could not run
when accepted would be a promise the store cannot keep.

## Extended: what acceptance checks

Accepting a proposal applies it under the proposer's grants as they were
when it was proposed: the write was checked whole then, and the reviewer
applies exactly what was checked. A call is proposed only if it could
run when it is proposed. A call that becomes a proposal, through `call`
at its declared risk or through `propose`, is checked as if it ran now:
its record's version against the expected one, then its `requires` on
that record. A stale version is a conflict and a `requires` that does
not hold is a precondition, and neither is recorded as a proposal
(lakelands#338). At acceptance the store checks again
only what can have moved since — the type's shape and the record's
version — and records the write with the proposer as its writer, caused
by the acceptance. A verb's write, run at once or on acceptance, fires
triggers and changes object sets exactly as any other write does
(`docs/decisions/004-the-change-feed-is-the-log.md`).

## Extended: when an executor fails

An external verb is due at the position of the event that made it due,
and it stays due: an executor's failure does not take that event back.
The failure is recorded as the executor's output — `{"error": "<its
error text>"}` in the `output_recorded` event that position causes — and
the caller's outcome is applied at that position with the recorded
output in it, the error form included: an error reply would say nothing
was written when the position is real, and a caller should not have to
read the log back for the answer. The verb is not run again for that
position; a retry is a new call and a new position. So the log holds, for
every external verb, either what the outside world answered or that it
did not.

A retry with the call's idempotency key is not a new call. It answers
the first outcome: the verb's position and its recorded output, or the
write the verb made. What a call wrote itself is told by what the store
appended with it, the write right after its `verb_called` carrying the
verb as its procedure. It is never told by a later write that names the
call as its cause, as a host's write of the effect's result does
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`,
lakelands#305).

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

## Extended: a retry never hands a position out again (lakelands#616)

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
another key or with none, or proposed again. A retry may not tell the two apart, and the
caller may: it was given the first answer.

The answer is found by a seek, not by a read of the log after the call.
It is recorded under an id derived from the lake and the position that
made the verb due, and an acceptance under one derived from the lake
and its proposal (`docs/log-format.md` §3), so what a retry reads under
the writer's lock does not grow with the log. Before this, a retry read
up to 4,096 events after the call: 74 to 90 ms on a lake of 5,000 events
against 0.1 to 1 ms for the seek, on the three engines.

An accept sent again answers by the verb as it stood at the acceptance,
told by what the acceptance appended (a write verb's write follows its
call, caused by it), whatever was declared since: an external call is
answered as a retry of a call is, and any other acceptance is
`decided`.

## Extended: the executor is handed the record its call was judged on (lakelands#660)

An executor is handed the record as it stood at the position that made
its verb due, the `verb_called` or the `write_accepted`, where the call
was judged; not as the head holds it when the hand-out comes. A write
between the two, another caller's or a retirement, does not reach the
executor, so it never acts on a record its call was not judged on: a
record retired after the call is handed as it stood, where it was
refused before. A purge appends and removes nothing, and compaction
keeps the projection's history, so that state is always there to read;
there is no fallback to the head.

## Extended: an executor's answer is held until it is recorded (lakelands#646)

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

## Extended: who sees which proposals

A principal with the review grant lists every proposal and may accept
or reject it. Any other principal lists only the proposals it made
itself, so a proposer can follow what became of its write, and decides
none.

## Extended: a proposal is read as its reader's view (lakelands#499)

`proposals` shows a proposed write to its reader as that reader may read
the record the write is for
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended). A reviewer need not know all of what it accepts: the accept
asks for `review`, and for `schema` where the ontology changes, and
applies the write as it was logged and checked under the proposer's
grants, the part its reviewer was not shown too. Trimming what is
applied to what the reviewer read would make the reviewer's grants
decide what the proposer wrote, and a write checked whole would be
applied in part.

**Told that there is more.** A listed proposal says `more` when it holds
more than its reader is shown, and says nothing else of it: no name and
no count, since a count would say how many names are hidden. Where the
reader may see nothing of the write, the entry has no write at all: a
write to a record it is not answered (one restricted, without the
grant), a call of a verb on such a record, a bundle none of whose
declarations it would be shown. The entry keeps its id, its proposer,
its times, its state and the proposer's `why`, so a reviewer without a
read grant on the record still finds what it is asked to decide, and
accepts it by its id. A proposal that names a type its reader is not
shown is not listed at all (below, lakelands#762).

The absent write is a form of the answer and of nothing else. A stored
proposal and a logged `write_proposed` hold the write whole, and
`propose` takes a write, never a proposal; so `Proposal.write` is
optional and `ProposedWrite` gained no variant that could be proposed or
logged.

**The filters follow the entry.** `type` and `kind` are answered on the
entry as its reader is shown it. A type the reader may not see answers
what a type that is not declared answers, an empty listing; an entry
without a write is of no kind. The unfiltered listing and `state` hold
every entry the reader is listed. Ordering and paging were already over the entry as shown.

**A proposed bundle** shows each declaration `describe` would show its
reader once the bundle is in force: a type it could see, with the fields
it may know of; a view with its definition or not at all; a trigger and
an object set whose condition it may read; a function by `describe`'s
rule; a value type always. A verb is judged by the fields it names, the
ones it sets known and the ones it reads readable, not by who may call
it: a reviewer need not be one who may. One who holds `schema`, as an
accepter of a bundle must, is shown every definition, so for it `more`
can only mean a field or a type hidden from it.

**The refusal at the accept** follows the same rule. An accept refused
because a declaration made since reads a field the migration takes away
names the declaration and the field to an accepter who may know of the
field, and to another says only that a declaration reads a field the
migration takes away: the proposer took the field away knowingly, the
accepter may not know that it exists.

An accept refused because the migration is no longer lossless carries
the fresh report as the accepter may know it, by the rule of the
listing, and says `more` where that is less than the whole
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended, lakelands#531).

## Extended: a proposal on a type its reader is not shown is not there (lakelands#762)

A proposal that names a type its reader is not shown is not there to
that reader. `proposals` does not list it, and `accept` and `reject`
answer as for a proposal id that does not exist, word for word. The
types a proposal names are:

- the type an upsert or a retirement writes, and the type of a called
  verb;
- of a migration's `to` and of each type a bundle declares: the type
  itself and, of a view, its sources; and the target of each relation
  and reference field it adds, changes or drops against the type in
  force, both compared completed, as the lake keeps a type (each
  computed field's `depends_on` filled, a link field's among them);
- the type each verb, trigger and object set of a bundle is on.

A call of a verb that reads a field its reader may not read is not there
either. Such a verb is not described to that reader, and the proposal
would tell that its `requires` held
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended, lakelands#779).

A link type is shown only with both its ends
(`docs/decisions/006-grants-not-roles.md`, extended), so a write to a
link with a hidden end is among them, and so is a bundle that adds a
relation or a reference to a hidden type. A relation or a reference
restated as it stands names nothing the type in force does not. "As it
stands" is equal as declared, in order (link fields and `depends_on`
included), as the lake judges a declaration in force: a restatement that
reorders them counts as a change, and errs toward hiding. An
author's declaration carries back the relations it is not shown, so a
reader not shown a relation's target still proposes, lists and decides
its own redeclaration and migration of the type, which it may author
directly. Only a type the lake declares counts: a bundle may declare a
new type its reader would not be shown once in force, and is listed
without that declaration, as above.

A listed migration is judged by its reader as it stands; its report is
then trimmed, as above, by the reader's grants at the proposal, against
the type as it stood. An accept or a reject judges this as soon as the
proposal is found,
before anything else of it is told: whether it is decided, whether a
pinned record moved since it was proposed, whether its write or its
bundle would apply. Nothing is appended. A proposal hidden only by a
record, one restricted or outside a holder's `only`, stays listed
without its write, as above. A reviewer shown every type it names
decides it as before, what its grants hide of a record or a field
included.

So no answer that reports on a type's migration is given to one not
shown the type, and its report needs no form with the type taken out:
`propose`, the preview `migrate` and `migrate_apply` answer that the
type is not declared (027, extended), and the accept and the listing
answer as here.

**Why.** An accept is a write to what the proposal names, and its answer
tells what it judged. If it applied, it wrote to a type its reviewer may
not read, or reshaped one. If it was refused, it named the type, a key
of it, a field and its kind, or that a hidden record moved. A listed
entry whose id the accept then calls absent would tell a proposal hidden
by its type from one hidden by a record. To a principal not shown a
type, the type is one that is not declared
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended), and a proposal on a type that is not there is not there
either: not in the listing, and not at the accept.

## Extended: an accept's refusal names what its reviewer's listing shows (lakelands#780)

An accept applies the proposer's write, planned again as the proposer,
so a refusal of that plan is the proposer's: it may name a field, a
type, a key or a value the reviewer is not shown. What an accept
answers a reviewer is judged by the reviewer's view, as its listing is.
A refusal of the write it applies (an upsert's plan, a verb's run) is
told whole where the reviewer is shown each thing it names:

- each type, one the reviewer is shown;
- each field, one it reads of the type written; a verb named counts the
  fields its `requires` reads;
- each verb, the one called;
- what a field's declaration says (the values it allows, its value
  type), only beside a field named;
- each key, the one written or, as each value, one the listing shows
  under a field named, or in a call's payload, which is listed only
  whole.

A call of a verb whose `requires` or set values read a field the
reviewer is kept from does not come to this. It is not there to that
reviewer (above; `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended, lakelands#779).

A name of another kind, a relation's, a trigger's or a function's, is
not judged. Any refusal is told whole to a reviewer that reads
everything, and to the proposer, whose own write was told the same.
Otherwise the accept answers as a proposal stale since it was proposed:
the write changed since it was proposed, naming only the type and the
key written, which the listing shows. A conflict names versions only,
and is told whole. A retirement's refusal names only the record it
retires, and what a write sets off is judged by declarations the lake
already took, so neither is judged. A refusal is judged by what its
case's members are, as typed (`Named` in the contract). A link's
cardinality is judged only where a link is written, and no proposal
writes one. A reject answers nothing of the write.

**Why.** The listing shows a reviewer the proposal trimmed to what it
may know, and says `more` where that is less than the whole. A refusal
that names a hidden field, its target type and the key it held would
tell the reviewer what the listing withheld, through the door the
reviewer is meant to use on it. A write that no longer applies needs no
more than that answer to be proposed again, by one who may see what
changed.

## Extended: a value type's tightening is proposed like a migration (lakelands#476)

A value type that is to accept less could be changed only by a principal
holding `schema`, through its preflight and apply (027, extended). A
principal without the grant could propose that a set gains a value and
not that it loses one, and the refusal of the narrowed declaration
pointed at a door it may not use.

A tightening is the third change of the ontology that can be proposed,
beside a set of declarations and a type's migration. The proposal
carries the value type's new declaration. It is preflighted when it is
made, without the `schema` grant, and the report replaces whatever the
proposer sent and is logged with the proposal, for the reviewer.

- **A report that does not fit refuses the proposal,** as invalid, and
  nothing is appended. The refusal says how many stored values would not
  fit and names none: a misfit names a record and a field, and the
  proposer may know of neither. A reviewer runs the preflight to see
  them.
- **Accepting it takes `schema` beside `review`,** as for the other two.
  It is preflighted again on the head and refused the same way when a
  value no longer fits. It is also refused when the value type was
  declared again since the proposal: `to` is the value type's whole
  declaration, and a value the set gained in between would leave it
  again, shown to no one. Either refusal leaves the proposal pending.
  Accepted, it writes what the apply door writes: one `declared` at the
  next version, as the proposer, caused by the acceptance.
- **A listing shows the report's uses only in fields its reader may
  read,** of types it may see, as a proposed shape is shown
  (lakelands#475). The totals stay whole: a count names nothing. A
  logged report fits, so it names no misfit to anyone.
- **The log format is 10** from the append that holds the first such
  proposal (`docs/log-format.md` §6).

## Extended: a declaration is proposed like a write

A set of declarations, or a type's migration, can be proposed as a
write is (lakelands#199). The proposal is checked when it is made as its
door would check it: the set against the ontology at the head as `apply`
checks it, the migration by its preflight, and a migration that would
lose information is refused with its report. A proposed migration keeps
that report, so a reviewer reads what it would move and what it would
drop. Nothing about the ontology changes until the proposal is accepted,
and a proposer needs no `schema` grant: proposing is how a principal
without it asks.

Accepting one needs `schema` as well as `review`. For a record write,
the proposer's own grants were the check, and a reviewer only decides
whether it happens. A change of the ontology is one the proposer usually
could not make, so the lake's grant for it decides: otherwise any
proposer and any reviewer together could change the ontology though
neither holds that grant. The accept checks the set again against the
head and runs the migration's preflight again, since the lake may have
moved. A change the head no longer admits refuses the accept, naming
why, and the proposal stays pending. What the accept declares is logged
in its append as the proposer's write, caused by the acceptance, and
takes the next versions and schema hashes there, not at the proposal.

A proposed shape, a migration's or a redeclared type's, is logged as
the store completed it from its proposer's view
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended, lakelands#475). `proposals` shows it to each reader without
the fields that reader may not know of, a relation's link fields too:
the proposer reads its own view again, and a reviewer who may see a
carried field reads it in the shape and not under `dropped`.

A proposed migration is accepted only on the shape it was proposed on
(lakelands#489). Its `to` is the type's whole shape, so on a type
declared or migrated since it would remove whatever was added, and the
report the reviewer read says nothing of that. The accept compares the
type's schema hash at the head with the report's `from_schema_hash` and
refuses a difference as invalid, saying the type changed and the
migration is proposed again; nothing is appended and the proposal stays
pending. Records written since are the second preflight's to judge, as
before.

## Extended: a verb called on its type (lakelands#277)

An effect that creates something outside the lake has no record to be
called on yet: a payment made in another system, a ticket opened, a
customer registered upstream before the lake holds them. Calling such a
verb on some other record would take its grants from the wrong type and
file the effect in the wrong history. Running it outside the lake would
lose the declared risk, the proposal, the idempotency and the recorded
output. So a verb may be declared on its type, `scope: type`, and it is
called with no key.

- **Grants.** They are its type's, as for any external verb: write
  access to `on`.
- **Risk.** It decides as it does for every verb. `low` runs at once;
  anything else becomes a proposal whose call carries no key, and
  accepting it runs the verb as the proposer.
- **The log.** `verb_called` carries no key. The executor is handed no
  record, and its answer is recorded as for any external verb, caused by
  the event that made it due. A retry with the same idempotency key
  answers the first outcome and runs nothing again.
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

## Extended: the order of refusals (lakelands#338)

The verb and proposal doors refuse in this order, in the frame every
write door shares (016, extended): the lake, the context, the door's own
checks, the append. Where a step runs "the record checks", they are
016's, on the record the write makes.

**`call`**: the retry; the verb is declared (`not_found`); the key fits
its scope (`invalid`, naming the verb); the principal may call it
(`not_granted`), which for a call that is proposed includes that a
holder of `only` proposes what can be pinned to its record (051); an
external verb's executor is bound (`unbound`); the
payload: a member the verb does not declare (`invalid`, naming the verb
and the member, as one missing from the payload and never from the type,
even where the type has a field of that name), then field by field in
name order a value that does not fit (`constraint`, naming the field),
then its required fields (`constraint`); on a record: the record
is live (`not_found`), the expected version (`conflict`), `requires`
(`precondition`). A `low` verb then computes its assignments (`invalid`,
naming the verb) and makes its write through the record checks. At any
other risk the call is proposed after `requires`: what could not run now
is not proposed.

**`propose`**: the retry; a link type at an ordinary door
(`wrong_door`); then the write as its own door checks it: an upsert
through the record checks; a retirement as `retire` from the type on; a
call as `call` from the verb on, its executor included, and for a write
effect through its assignments and the record checks; a declaration as
`apply` checks a set; a migration by its preflight, then `migration` if
it is not lossless; a value type's tightening by its preflight, then
`invalid` with the count if a stored value would not fit. No `schema`
grant is needed to propose a change of the ontology.

**`accept`**: the `review` grant (`not_granted`); the proposal exists
(`not_found`); it is pending (`decided`); a change of the ontology also
needs `schema` (`not_granted`); then the write, checked again at the
head: an upsert through the record checks; a retirement for its record
and version; a call for its verb, its scope, the verb declared
differently since (`invalid`), its executor, then as `call` from the
record on; a declaration as `apply` checks a set; a migration for the
shape it was proposed on (`invalid`, lakelands#489), then by its
preflight, then `migration`, then its embedding model (`unbound`); a
value type's tightening for the declaration it was proposed on
(`invalid`), then by its preflight, then `invalid` with the count. Any
refusal leaves the proposal pending.

**`reject`**: the `review` grant, the proposal exists, it is pending.

Two steps that answer the same kind for the same verb or field, such as
the scope and a verb declared differently since, are stated here and not
checked between themselves.


## Extended: `describe` says whether the executor is bound (lakelands#432)

A verb that reaches outside the lake is described with its executor's
name and whether the host binds an executor of that name, and its tool
says which. Without this, a caller learns that a verb cannot run in a
lake only by calling it and being refused, and a host's interface cannot
show the verb as unavailable beforehand. The name is no news to the
principal: the verb is described only to one who may call it, and the
`unbound` refusal names the executor to that principal anyway. A verb
that sets fields of its record has no executor and is described without
one. The tool of an unbound verb stays in the catalog, so a host that
binds the executor later changes the tool's text, not the set of tools.

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
