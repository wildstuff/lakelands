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

## Extended: who sees which proposals

A principal with the review grant lists every proposal and may accept
or reject it. Any other principal lists only the proposals it made
itself, so a proposer can follow what became of its write, and decides
none.

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
its scope (`invalid`, naming the verb); a verb called on a record of a
view (`invalid`, 018, extended); the principal may call it
(`not_granted`); an external verb's executor is bound (`unbound`); the
payload, field by field in name order (a field the verb does not take
is `invalid`, a value that does not fit is `constraint`, each naming the
field), then its required fields (`constraint`); on a record: the record
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
it is not lossless. No `schema` grant is needed to propose a change of
the ontology.

**`accept`**: the `review` grant (`not_granted`); the proposal exists
(`not_found`); it is pending (`decided`); a change of the ontology also
needs `schema` (`not_granted`); then the write, checked again at the
head: an upsert through the record checks; a retirement for its record
and version; a call for its verb, its scope, a view, the verb declared
differently since (`invalid`), its executor, then as `call` from the
record on; a declaration as `apply` checks a set; a migration for the
shape it was proposed on (`invalid`, lakelands#489), then by its
preflight, then `migration`, then its embedding model (`unbound`). Any
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
