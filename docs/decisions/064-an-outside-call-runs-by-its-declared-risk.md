# 064 — An outside call runs by its declared risk, and names the write that set it off

## What is decided

### A function declares a risk, and above `low` its call is proposed

A function declares `risk`, `low`, `medium` or `high`, as a verb does
(`docs/decisions/013-verbs-run-by-declared-risk.md`). The default is
`low`, and a `low` function's call is made after the append that made it
due, as before (`docs/decisions/022-functions-are-declarations.md`). A
function backed by `expr` is computed in the fold and reaches nothing
outside the lake, so it is refused any risk above `low` (`expr_risk`).

A call of a function above `low` is not made until it is accepted. Where
the drain would call the backing, it appends a `write_proposed` instead,
as the writer of the event that made the call due, caused by it, with
that event's context. The declared risk alone decides, whoever wrote and
at any depth, the owner included. Nothing is sent to the backing, and
the field waits: cleared, with no `error`.

- **A write** proposes its record's call, a `compute`: the type, the
  key, the field the output fills, the function and the call's digest.
  The log holds no inputs: a value sealed under a source's key would stay
  readable in the proposal once the source is forgotten (058). A listing
  shows them as the record holds them, to a reviewer answered the record.
  A record's call that a pending `compute` holds already is not proposed
  again; the same call on another record is that record's to propose.
- **A declaration or a `recompute`** proposes the calls its sweep owes
  as one, a `compute_sweep`: the type, the function, how many calls, and
  the first ten records' keys. An operator decides one act, not one
  proposal per record.

No caller proposes either kind: `propose` refuses them as not granted.

### A call that waits for a decision is its record's

A decision holds for its record. A call above `low` is proposed, decided
and read for the record that makes it: its digest is that of the call on
the record, `{ "call", "record": [type, key], "made" }`, where `call`
is the call's own digest (022) and `made` the position of the write
that made the record: its first, or its first after a retirement. So a
decision, and the output it records, reach no other record that makes
the same call: if they did, whether
another record's call was made or rejected would tell a reader of a
record held from it, or behind an `only`, or of a type it is not shown,
that such a record made the call first. A call that needs no decision
is made once per digest, and its output fills every record that makes
it, whoever is shown it, since nothing was decided by it. An output
recorded under the call alone before the function's risk rose is read
for a record whose value stands on it, and never for a record that makes
the call anew: that it was recorded would tell of another record that
made it. A record made again at a retired key goes on with that key's
version (051), and not with its decisions: they were taken on the
record retired there, whose writer and reviewers may be others than the
new record's, and reading them would tell its writer of a record held
from it. A retirement closes its record's pending `compute`s in its own
append, and a re-key closes a moved record's with the `migrated`, whose
calls are then digested on the new key, so no proposal is left waiting
that could never be accepted. A moved record's call is proposed again
on its new key by its next write, or by a `recompute`.

### An accept makes the calls, and a rejection is recorded as their output

- **The accept of a `compute`** is refused as a `conflict` when the
  record no longer makes the call it proposed, since the reviewer
  judged those inputs; the proposal stays pending. A record written
  since without changing the call is accepted.
- **Its `write_accepted`** leaves the calls owed to its own append: the
  record's call, or every call of the function the sweep owes at the
  head. The drain makes them, and each `output_recorded` is caused by
  the accept, with the accept's context.
- **Its `write_rejected`** leaves the same calls owed, and the drain
  records `{"error": "rejected: <reason>"}` as each one's output without
  calling the backing. The field is cleared with that reason. The
  rejection holds for its record, and it is a failure like any other
  (022): a declaration or `recompute` may propose it again. A call its
  record no longer makes, because the record moved on, is recorded as
  rejected for that record at once, with the rejection, so it holds when
  the record makes the call again.

An output recorded for a record's call fills the record whenever it
makes the call, so no effect is repeated for it and nothing is proposed.
It also decides the call: the record's pending proposal of it is
rejected by the system, caused by the output, whether a sweep's accept
made it or a rejection recorded it. An accept of it is then answered as
of a decided proposal, never as a conflict that leaves it pending nor as
a success that makes nothing. An accepted call is made once, whether it
answers or fails.

### A proposed call is listed whole only to who may judge it all

A proposed call is not there to a reviewer not shown its type, nor, of
one record's call, to one not answered that record. A sweep is there to a
reviewer who holds `restricted` and no `only` alone, whether or not a
record is held: that a sweep is owed at all tells of a record held, and a
held record and a retired or absent one must be listed alike. To any
other reviewer it is not there. A record's call proposed by a record
other than the one at its key now, retired there since or moved away by
a re-key, is there to that reviewer alone too: that it was proposed and
decided tells of a record the key's present reader may not have been
answered. It is listed with no inputs, which are another record's. A
call is listed whole to a reviewer who reads every input the function
takes and every field it computes on the type; to a reviewer who does
not it is there in part, showing nothing: it may reject it and may not
accept it.

### A backing is told the write that set its call off

A call carries `cause`: the position and writer of the event that made
it due, the record's type and key, the event's source where it came
from one, and its context; and, for a call made on an accept, the
accept's position and reviewer. For a declaration's sweep the event is
the `declared`, for a `recompute` the `declared` it appends, and for a
call a fill made due, that fill.

The cause is no part of the call. The digest stays that of the
function, the inputs and the configuration, and of a call above `low`
its record, so the same inputs are one call, read back from one output,
whoever caused them. It holds no value
of the record, so a sealed input (058) stays sealed. The `http` and
`exec` backings send the call as before; a host's own backing reads the
cause.

## Why

An outside call is an effect, as an external verb's is, and the lake
already governs effects by a declared risk. A function had no such road:
its call was made inside any write that moved its inputs, so every
writer, every declaration and every `recompute` held the power to cause
it, and a backing that gated the effect itself could only record a
failure and wait for a `recompute` of the whole function. A host that
must decide or audit the effect needs to know who caused it, which the
backing was not told.

## What it is not

- **Not a change to what is due.** The fold decides from the log alone
  when a call is due, as before (022); only whether it is made at once,
  and what the backing is told, changed.
- **Not a gate on embeddings.** An embedding is computed by the bound
  embedding backend, which declares no risk.
- **Not durable before its proposal.** A call owed and not yet proposed
  is held by the writer as any owed call is; one lost with a crash is
  made due again by the next write that moves its inputs, or by a
  `recompute`.
