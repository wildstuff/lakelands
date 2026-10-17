# 021 — The write context belongs to the host

## What is decided

### The host sets the context, and no door takes one from its caller

`WriteContext` — where a write comes from (a direct call, a reaction to
an earlier change, a scheduled procedure) and how deep in a chain of
reactions it sits — is set by the host, never by a caller. A host obtains
a `Catalog` from its `LakeStore` bound to a context, and every write
through that catalog carries the context into the log. No public door
(MCP, REST, the CLI) accepts one from its caller.

### A write deeper than the lake's ceiling is refused, whatever its origin

The store refuses a write deeper than the lake's configured ceiling: the
ceiling holds for every write whose depth passes it, whatever origin
its context names. A depth above 0 means the write is a reaction, so a
direct or scheduled context deeper than the ceiling contradicts itself.
The store refuses it as `too_deep` rather than believe the origin: the
context is the host's, the ceiling the store's. The conformance check
`a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing`
holds it on every engine.

The ceiling is checked on every door that writes. That covers the record
doors, `link` and `unlink`, `call` and `propose`, and also `ingest`,
`accept`, `reject`, `declare`, `recompute`, `apply`, `add_field`,
`migrate_apply` and `migrate_value_type_apply`.

### The host may name the cause, and lineage is one chain

A host's context may name `cause`, the position its write follows from:
the `trigger_fired` it reacts to, a record of its own run, any earlier
event. The store checks that the position is at or below the head
when the write appends, and refuses one past it as `invalid`, naming
both. It reads the one event the cause names for a trigger's ceiling
alone; a cause that names any other event binds nothing more.

The cause lands on the events the host's write itself appends: its
`record_upserted`, `record_retired` or `record_ingested`, and the first
event of a verb call or an acceptance (`verb_called`, `write_accepted`).
Events the store derives from those keep the store's cause, which chains
back to the host's. A verb's writes name their `verb_called`, the
declarations after an acceptance name the `write_accepted`, and a
trigger's firing names the write that fired it. So lineage is one chain
from any event back to the change that started it, never a choice
between two causes. The cause sits in the envelope's `cause`, never
inside `context`.

The cause is lineage, not depth. The host still sets `depth`, and a
scheduled step naming its run at depth 0 is coherent.

### An effect without a record is traced through its `verb_called`

A verb called on its type
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended) has no
record to anchor its effect. Lineage needs no new mechanism for it. The
host's `cause` lands on the call's first event, its `verb_called`, as on
any other. A record the host writes afterwards, as the result of the
effect (the payment it created, the ticket it opened), names that
`verb_called` as its own `cause`. So the record's history leads back to
the call, and the call to whatever the host said caused it.

### A trigger caps the reactions that name its firing

A trigger may declare `max_depth`, at most the lake's
`max_reactive_depth`, `0` included. A reactive write whose `cause` names a
firing of that trigger, and whose depth is above the trigger's ceiling,
is refused `too_deep`, naming the trigger, with nothing appended. The
store reads the one event the cause names and takes the ceiling the head
declares for the trigger. A trigger with `max_depth: 1` thus cascades once,
however high the lake's own ceiling is.

The per-trigger ceiling binds the reactions that name the firing. A
reaction that names no cause is bound by the lake's ceiling alone: what a
write follows from is the host's to say, and a host that wants the
trigger's ceiling names the firing.

### A trigger past its ceiling drops instead of firing

A reaction runs before its write is judged: it calls a model, a tool, a
service outside the lake. So a firing whose reaction could write
nothing is not appended. Where the write that would fire a trigger is
at a depth at or above the trigger's `max_depth`, or at or above the
lake's `max_reactive_depth`, the store appends `trigger_dropped` in
place of `trigger_fired`: the trigger, the record, the write's depth,
the bound that applied, the lower of the two, and whose bound it is.
The durable fact the feed promises (004) stays: what would have fired
is in the log, and told to whoever a firing of the trigger is told to.
A host acts on `trigger_fired` alone and checks no depth itself.
A trigger with `max_depth: 0` thus never fires: every write is at depth
0 or deeper, so each of its firings drops. It stays declarable, as a
trigger kept in the log for what it would fire. A lake kept from before
holds its triggers as declared, and from its upgrade on a firing past
the ceiling drops where it fired before.

A reaction to a firing the store appends is one deeper than the write
that fired it, so it is within the trigger's ceiling. The refusal at the
append stays as the second line: it binds a reaction whose trigger's
ceiling was lowered after the firing, one whose host names a firing at
another depth than it was told, and one whose host reacts to a
`trigger_dropped` all the same, naming it as its cause: that reaction
is past the trigger's ceiling and is refused `too_deep`, naming the
trigger.

### Every write door checks the lake's ceiling before anything but the lake

Every write door checks the context first, after it has made sure the
principal belongs to the lake: before its grants, before it looks up a
type, relation, verb or proposal, and before it takes the lake's
writer. So a context past a ceiling is refused as `too_deep` alike on
every door, whatever else the request would break, and a refused
context waits on no other write. This holds for the lake's ceiling; a
trigger's ceiling depends on the cause, and is read where the cause is.
`ingest` checks it when it is called,
before it takes the stream, and each window checks it again before it
takes the lake's writer: the check reads the lake's configured ceiling
and no head. A host's `cause` is the exception: it is judged
against the head where the write appends, after every other check.

### A trigger's ceiling is read at the append, under the writer

The ceiling of a trigger depends on the event a reaction's `cause`
names, so it is read where the cause is judged: at the append, under
the lake's writer, right after the cause is found at or below the head.
Read before the writer was taken, a cause one past the head named
nothing yet, so the trigger's ceiling was skipped. If another write
appended the firing in the meantime, the append then took the cause and
wrote a reaction past the trigger's ceiling. Under the writer the
store reads exactly the event the append follows. A cause past the head
is refused as `invalid`, even when the next append would put a firing
there. A cause that names a firing is refused `too_deep`, naming the
trigger, when the depth is above its ceiling. Both come after the
door's other checks; the lake's own ceiling, which reads nothing, stays
first.

### The depth crosses lakes with a mirror

A mirror's copy is a reaction at the depth of the source state it
copies, with no cause, so a chain of reactions that crosses lakes still
meets a ceiling: see 042, "A copy keeps its chain's depth".

### A write past the ceiling lands nothing

A reaction past the ceiling is refused whole: its own write is not
appended either, and nothing of it is kept for later. Landing the write
and dropping only the reaction it calls for leaves a lake that no rule
describes: the rule says the reaction follows the write, and the log
holds the write without it. So a chain past its ceiling stops before
its last write, and the host decides what to do with it: raise the
ceiling, or write it again outside the chain, at depth 0, naming its
cause.

## Why

The context says how much a write should be trusted to have been
intended: a reaction to a trigger is not the same as a person's command.
If a caller could set it, a caller could make a reaction look like a
direct command, or reset the depth counter and let a trigger feed itself
forever. Only the host knows which code path a write really came
through, so only the host sets it.

Keeping it out of the doors also keeps it out of the concepts a caller
must learn. A caller holds a principal, not a context.

## What it is not

- It is not a right. The principal's grants decide what may be written;
  the context only records how the write arrived.
- It is not a substitute for the change feed. A host learns what to
  react to from the feed (004) and writes its reaction with a reactive
  context.

## How it came to be

- The first version: the host sets the context, no door accepts one,
  and a reactive write deeper than the lake's ceiling is refused.
- On 2026-09-25 the ceiling was said to hold by depth, whatever origin
  the context names.
- A context may name its `cause`, which lands on the events the write
  itself appends (lakelands#243).
- `ingest`, `accept`, `reject`, `declare`, `apply`, `add_field`,
  `migrate_apply` and `migrate_value_type_apply` let a context past the
  ceiling through until 0.6.0; a host that wrote reactively through
  them past the ceiling is refused `too_deep` there too
  (lakelands#257).
- A trigger may declare `max_depth`, which binds the reactions that
  name its firing (lakelands#244).
- A firing past the ceiling was appended, and only its reaction's write
  refused, so the reaction ran first. It drops at the write since
  lakelands#1216, as `trigger_dropped`.
- A verb called on its type anchors its effect by its `verb_called`
  (lakelands#277).
- Every write door checks the context first (lakelands#344).
- A trigger's ceiling, read before the writer was taken, is read at the
  append, under the writer, and comes after the door's other checks
  (lakelands#375).
- A mirror's copy carries its chain's depth across lakes
  (lakelands#420).
- A reaction past the ceiling is refused whole, its own write included
  (lakelands#563).
- Corrected at the rewrite (lakelands#817): the cause's rule said the
  store "neither reads the event nor checks its kind". Since
  lakelands#244 it reads the one event a cause names, for a trigger's
  ceiling.
- Corrected at the rewrite (lakelands#817): the list of the doors that
  check the ceiling left out `link`, `unlink` and `recompute`. Each
  calls the same check first, as the doors the list named do.
- Corrected at the rewrite (lakelands#817): each ingest window was said
  to check the context "against the head it reads". The window's check
  is the one every door makes: it compares the depth with the lake's
  configured ceiling and reads no head. The head enters at the append,
  for the cause and a trigger's ceiling.
