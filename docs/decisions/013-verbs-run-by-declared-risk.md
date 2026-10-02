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

The default scope, `record`, is never written, so every declaration made
before keeps its bytes.

