# 022 — Functions are declarations

## What is decided

A function is a declaration in a lake's ontology like a type or a verb:
a name, the backing kind that computes it, typed inputs, and either an
expression (for the built-in `expr` backing) or the backing's own
configuration (for any other kind). It enters the log as a `declared`
event, and a bundle declares it in its `functions` lane.

The backing kind is an open token. `expr` is evaluated inside the store
from its expression, in the one grammar. Any other kind is called
through the `FunctionBacking` seam, and its output is recorded in the log
and read back on replay.

## Why

A function a lake depends on has to be part of what the lake knows at a
position. If functions lived beside the log, in files or in a host's
configuration, a replay could meet a different function under the same
name, and the lake could not say which version produced a value.

Keeping the backing an open token lets a host add its own kind of
computation without a change to Lakelands, while the store keeps the
two guarantees that matter: the declaration is in the log, and every
output that is not deterministic is recorded.

## What it is not

- It is not a way to call arbitrary code from an expression. The
  expression grammar has a closed set of functions (017); a declared
  function is computed by the store where the ontology names it.
- An `expr` function is not recorded output: it is deterministic and is
  recomputed on replay.

## Extended: where a function is invoked

In this step a function is declared and can be loaded from a bundle, but
nothing in a declaration yet names one to be computed: no field says
"this value comes from that function". The invocation belongs to the
fold. A derived field will declare the function that computes it, as a
member of the field's declaration, when the fold that calls it arrives;
until then a declared function is part of the ontology and computes
nothing.

## Extended: a field computed by a function

A derived field may name the declared function that computes it (the
field's `function`) instead of carrying an expression; the function's
inputs are read from the record's fields of the same names, and the fold
computes it like any derived field, recomputing it on replay. In this
release only the `expr` backing computes fields; a field whose function
needs any other backing is refused when declared, until the backings that
reach outside the lake arrive with their recorded outputs.

## Extended: a redeclared function recomputes its fields

When a function is declared again, the fold computes, in that `declared`
event, every live record's fields the function computes, and likewise
when a type's own derived expressions are revised. The values take the
event's position and name the function's new version as their
procedure's; a function-computed value names its function's version
from its first computation on. The record's version stays, since no
write touched it.

## Extended: the `expr` backing is the one `FunctionBacking`

In `0.1.0` the `expr` backing is the only implementation of the
`FunctionBacking` seam: `lakelands_store::ExprBacking`, with the
expression in the call's `config`. The fold computes a function field
with the same evaluator in process and never calls a backing, so a
replay recomputes the field exactly and nothing is recorded.

## Extended: backings that reach outside the lake compute fields

From `0.2.0` a field may be computed by a function of any backing kind.
Declaring a function whose backing is not `expr` is accepted when the
host bound a backing of that kind in `Seams::backings`, and refused as
`Unbound` naming the kind otherwise. A field naming such a function is
accepted like one naming an `expr` function. Replay and rebuild never
ask: a declaration already in the log folds whatever the host binds.

**The call.** A field's call is `{ "function", "inputs", "config" }`:
the function's name; every declared input by name, with the record's
value of the field of that name, `null` where the record holds none;
and the function's declared configuration, `null` when it has none. The
call's digest is the SHA-256 of its canonical bytes, and with the seam
`function:<kind>:<function>` it keys the recorded output
(`docs/log-format.md` §2.5). The lake is not part of the call's digest:
the event already names it. The backing receives exactly this object,
with the lake beside it.

**When a field is due.** The fold decides it, from the log alone:

- a record is written through any door and its function's inputs differ
  from before, a record written for the first time included;
- the function is declared again: every live record whose type names it;
- a type is declared again with other functions on its fields: that
  type's live records;
- a type whose fields are computed outside the lake is declared again,
  whatever of it changed: that type's live records.

"Declared again" is a `declared` event in the log: a declaration that
differs from the one in force, or the one in force appended again by
`recompute` (below). A declaration that is the one in force appends none
(027, extended), so it makes nothing due.

A due field whose call already has an output recorded in the lake is
filled from it in the same event, and nothing is called. Otherwise the
field is cleared, and the call is due. A computed field that holds
neither a value nor an error (its output never arrived) is due again on
the next event that touches its record.

Computed fields settle in the order of what they read (see "No cycle"
below), derived and backed alike. Settling repeats until nothing
changes: recompute the derived fields, settle the backed ones, then
recompute again over what settling changed. A field filled from a
recorded output therefore recomputes what reads it in the same event,
however it was filled. A backed field that reads, directly or through
derived fields, one that is still waiting for its output waits too. It
is cleared and not called, and is settled by the fill that completes
its input. Its call would otherwise carry an input that is not computed
yet, and the output would stay recorded under that input for good.

**Calling.** An append that made calls due leaves what owes them in the
writer's list: the record, or for a declaration that settles a type
again, one sweep over the type's records. The list holds no call. After
the append the store reads the calls from the records as they are then,
and makes them outside the writer lock, a batch at a time per lake: the
embeddings of one model, at most 64 texts, or one function call. Before
each call it reads the record again and looks for an output recorded for
the same call, so one input is never sent twice, not even by two writes
that race. An answer is held, one per call, until it is recorded, so a
recording that fails is made again by the next batch without a second
call; the hold is the handle's and ends with it. An entry leaves the
list only once its batch is recorded: a drain that fails, or a door's
call dropped in the middle of its drain, loses nothing, and where no
fold failed nothing says so but the fields that wait. A sweep reads its
type a page at a time in key order, the writer released between pages,
and moves past a record only once its calls are recorded; a migration
that moves the type's keys starts it again from the first, and a
record's entry follows its record. The store records each answer as
`output_recorded` naming the field, with `cause` the event that made it
due and `procedure` the function's name and version. The output and everything its fill causes carry the context
of the event that made it due, with the depth unchanged. A write's own
`trigger_fired` and `object_set_changed` carry the write's context in
the same way, so a host that acts on one writes one level deeper (021).
A due issued again while its call was under way finds the output
recorded; it appends nothing when the output fills nothing new. The calls
a fill makes due are owed to the write behind the output.

A write waits for what it owes, not for what others owe. One that left
nothing due answers its own last event's position at once, whatever is
being made beside it. One that made calls due takes the lane of calls
for one batch at a time, its own entries first, then the rest in the
order they were handed out, until none of its own is left. It then makes
one batch of the entries no write waits for any more, and answers with
the lake's head at that moment: the position from which its filled
fields are visible, even when a neighbour's batch made its calls. Between the
write's append and the output's, a reader sees the field cleared with no
`error` in its metadata. That is how a pending output reads, as against
a failure (below). When no backing of the kind is bound at that
moment (a lake reopened without it), nothing is called or recorded, and
the field stays cleared until it is due again. Opening a lake, an
import and `rebuild` never call a backing, and make no call due (below,
extended).

**Filling.** `output_recorded` fills its field when the record is live,
the field is still computed by that function, and the record's call has
the recorded digest now. Otherwise the output stays recorded for its
input and fills nothing: a later write moved the inputs on while the
backing was called. A filled value takes the event's provenance, with
`origin: derived` and the function's version as the procedure version.
The record's version stays, since no write touched it.

A fill is a change of that field, and it takes the fold's ordinary path,
as a write of that one field would (012). In the same event, the derived
fields that read it are recomputed and the fields that depend on it are
cleared, and a backed field whose inputs it changed is filled from a
recorded output or made due. In the same append, the triggers its record
now meets fire and the object sets it now joins or leaves change, each
with the output as its `cause`. A value computed outside the lake is a
value like any other to everything that reads it.

**No cycle.** A type's computed fields, derived expressions and fields
computed by a function alike, read one another without a cycle. A
derived field reads what its expression reads, and a function-computed
field reads its function's inputs. A declaration that closes a cycle is
refused, naming its fields, whether it is a type's or a function's
declared again with other inputs. A function's inputs are fields of the
same record, so a chain of fills within one record ends, and a fill needs
no depth limit of its own. The one loop that leaves a record (a fill
fires a trigger, the host writes, the write makes a call due) goes
through the host's write, which `max_reactive_depth` bounds (021).

**Failure.** A backing that fails or times out is recorded as
`{"error": "<why>"}`. An output of that form (an object whose one
member is `error`, a string), or an output that is not a value of the
field's type, fills nothing: the field is cleared, and its metadata
carries the reason as `error`, with the event's provenance. A read with
`with_meta` shows that metadata though the field holds no value. A
failure is an output like any other. The same call is not made again;
the field is computed again when an input changes, or when the function
is declared with a different configuration. `recompute` is the one
deliberate exception (see "`recompute` computes again, and retries a
recorded failure" below).

**Why.** A field whose inputs moved on no longer holds the value those
inputs compute. Clearing it until the output arrives is the only state
the lake can vouch for. Failing into a cleared field with its reason,
never into the old value, keeps a failure from looking like an answer.
Recording the failure, instead of retrying it on the next occasion,
keeps the one-input-one-call promise that makes a backing's cost and
side effects predictable, and makes a replay reproduce the failure too.

**What it is not.** A backing is never called by the fold, by a read,
by opening a lake or by `rebuild`; only by the store, after an append,
for the calls that append made due. It is not a way to retry: a host
that wants a failed call made again changes the function's
configuration or the inputs, and the log shows it did.

## Extended: `recompute` computes again, and retries a recorded failure

This qualifies "The same call is not made again" and "It is not a way to
retry" above. A settle forced by a `declared` event calls again for an
input whose recorded output is a failure, and a recorded value is still
never asked for twice: the forced settle reads it back, as any settle
does.

Until lakelands#565 an operator forced that settle by declaring the
function or the type again, unchanged. A declaration in force now appends
nothing (027, extended), because a host states its declarations at every
start and each of them walked the type. So the act has a door of its
own, `recompute`, under the `schema` grant:

- **It names a function, whatever its backing, or a type with a field
  computed outside the lake** (an embedding, or a field of a function
  with a backing). Anything else is refused and nothing is appended: a
  name no function or type has, and a type its caller is not shown, as
  not found; a view, and a type whose fields are all computed inside the
  lake, as invalid, saying that a derived field is as of its record's
  last write.
- **It appends the declaration in force again,** as it was declared, at
  its next version, written by the caller. The fold does at that event
  what it does at any `declared`: a function's fields are computed with
  the settings and the time of that event, a call whose recorded output
  is a failure is made again, a field that waits for its backend is
  made. No event kind of its own, and the log's format does not move: a
  log written before holds such events and folds the same.
- **It is not proposed and takes no idempotency key.** Called twice it
  appends twice; the second changes no record.
- **Its caller is told nothing by it:** the answer is a position, and of
  a type it is shown without every field it is described what it was
  described before.

The log may then hold two outputs for one `seam` + `input`, the failure
and the later answer. The later one holds: the fold, and an engine's
`ProjectionView::output`, answer the latest output recorded for a call,
and an engine's `put_output` replaces the one before it from its
position on. This is a rule of the one rulebook (031), the same on every
engine, and a replay reads the outputs in log order and ends where the
lake did.

**Why.** A failure is often weather: a timeout, a refused connection, a
limit reached during a backfill. When the digest carries nothing the
operator can change, as with an embedding, whose input is the text
alone, a failure recorded for good would stay for good. The door is a
deliberate, visible act in the log, so a retry made through it keeps the
promise the rule protects: nothing is called twice behind anyone's back,
and the log says why a second call was made.

## Extended: an embedding is a computed field's declaration

A vector field may declare that it is the embedding of text fields of
its record: `embed: { from: [fields…], model }`. It is computed like a
field computed by a backed function, and follows every rule above:
- It is declared only where an embedding backend naming its model and
  its dimensions is bound, and refused as `unbound` otherwise.
- It stands in the one reads-graph, reading its `from` fields.
- The fold makes it due when its text changes, when it is waiting, or
  when what computes it is declared again.
- The store calls the backend after the append and records each vector
  as `output_recorded` under the seam `embed:<model>`, keyed by the
  digest of `{"texts": [text]}`. So one text is embedded once per model
  in a lake, whichever record or field asks.
- A failure is recorded and clears the field with its reason;
  `recompute` retries it.

`embed` is not shape (007). Which fields feed a vector, and which model
makes it, say how the value is computed, not what the field is. So a
change to `from` or `model` is a redeclaration of the type, which makes
the field due on every live record. Only a change of `dimensions` is a
migration, and it already is one.

The store sends at most a fixed number of texts in one request
(`docs/seams.md` §EmbedBackend), and a failure is recorded per text, so
it has to be one the text earned. The backend says which kind it is. A
request refused for what it carries (`Invalid`) is asked again in
halves, and a refusal is recorded only for a text refused alone: one
text an endpoint will not take never fails the others beside it, which
would otherwise hold that failure lake-wide and fail again at every
retry. Any other failure is weather: halving it would cost nearly two
requests per text, each waiting the same timeout, so it ends the asking
and is recorded for every text not answered yet. Telling the two apart
is the backend's, because an endpoint refuses a whole request without
naming the input.

## Extended: a write waits for its own calls, and a declaration owes one sweep

The calls are read from the records when they are made, not handed out
by the fold: an append owes the records it left waiting, and a
declaration that settles a type again owes one sweep over it, whatever
the type holds. A write waits for the batch in flight and its own, as
**Calling** says; the declaration still answers once its sweep is done,
so what its answer promises is unchanged. An entry whose write no longer
waits (its drain failed, or its caller went away) is an orphan: each
later write that owes something makes one batch of orphans after its
own, so an abandoned sweep advances a batch a write, and a `recompute`
of its type owes a sweep of its own. A write that caught up over an
append whose fold failed owes what that append left, as its own.

**Why.** The writer held one call per record and field, each with its
text or inputs, and a write that found that list not empty, or a drain
counted since it began, took the whole list and held the lane until it
was empty. A write made beside a declaration that embeds 2 000 records
waited 16 to 20 s for the backlog, and beside one over 100 000 records
about 20 minutes (lakelands#642). The list held 159 MB at 100 000
records of a 1 000-byte text, and 515 MB where 10 000 records shared 64
texts at 1 536 dimensions, since each call carried the answer for its
text (lakelands#609). Now the list holds one entry per written record
and one per declared type, a batch holds at most 64 calls and one answer
per call, and no write waits for more than one batch of a backlog it did
not make.

## Extended: a fold of the log makes no call due

A call is due to the append that wrote its event, never to a fold of
the log. An open's catch-up, a `rebuild`, an import and a writer that
catches up fold events whose appends are over: they read the recorded
outputs, and a field whose output is not recorded is cleared with no
`error`, waiting, as `describe` counts it. They hand out no call. The
field is due again on the next event that touches its record, or at
`recompute`.

The one exception is an append whose own fold failed. Its events are in
the log, and its caller was told to send it again under its key. The
writer remembers where that append begins; the fold that catches up
hands out the calls of the events from there on, and of nothing before,
and forgets the position once it has folded them. So the write sent
again is answered with its fields made, as if its fold had not failed,
and a catch-up holds at most one append's calls.

**Why.** Every fold used to make the calls due that its events made on
their append, and an open and a rebuild dropped them at the end. Until
that end the writer held one call, with its text or its inputs, for
every embedded or backed write in the log: a rebuild of 100 000
embedded writes of a 1 000-byte text peaked at 162 to 164 MB, growing
with the log and with the text; it peaks at 4 MB now. A rebuild that
stopped was continued by the next write, whose catch-up kept them, so
that write called the backend for the rest of the log; and the drop at
a rebuild's end took the calls of a write appended before it and not
made yet. No caller waited for any call a fold of the log made due.
