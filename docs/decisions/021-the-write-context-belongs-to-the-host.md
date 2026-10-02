# 021 — The write context belongs to the host

## What is decided

`WriteContext` — where a write comes from (a direct call, a reaction to
an earlier change, a scheduled procedure) and how deep in a chain of
reactions it sits — is set by the host, never by a caller. A host obtains
a `Catalog` from its `LakeStore` bound to a context, and every write
through that catalog carries the context into the log. No public door
(MCP, REST, the CLI) accepts one from its caller.

The store refuses a reactive write deeper than the lake's configured
ceiling.

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

## Extended: the depth is the chain's, whatever the origin (2026-09-25)

The ceiling holds for every write whose depth passes it, whatever origin
its context names. A depth above 0 means the write is a reaction, so a
direct or scheduled context deeper than the ceiling contradicts itself.
The store refuses it as `too_deep` rather than believe the origin: the
context is the host's, the ceiling the store's. The conformance check
`a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing`
holds it on every engine.

## Extended: the host names the cause (lakelands#243)

A host's context may name `cause`, the position its write follows from:
the `trigger_fired` it reacts to, a record of its own run, any earlier
event. The store checks only that the position is at or below the head
when the write appends, and refuses one past it as `invalid`, naming
both. It neither reads the event nor checks its kind.

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
scheduled step naming its run at depth 0 is coherent. The ceiling is
checked on every door that writes. That covers the record doors, `call`
and `propose`, and also `ingest`, `accept`, `reject`, `declare`,
`apply`, `add_field`, `migrate_apply` and `migrate_value_type_apply`,
which until 0.6.0 let a context past the ceiling through (lakelands#257).
A host that wrote reactively through those doors past the ceiling is
now refused `too_deep` there too.

## Extended: a trigger caps the reactions to it (lakelands#244)

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
trigger's ceiling names the firing. Suppressing the firing itself past
the ceiling would need no cause, but it would drop the durable fact the
feed promises (004). So the firing is always appended, and the reaction
is what is refused.

## Extended: an effect without a record (lakelands#277)

A verb called on its type
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended) has no
record to anchor its effect. Lineage needs no new mechanism for it. The
host's `cause` lands on the call's first event, its `verb_called`, as on
any other. A record the host writes afterwards, as the result of the
effect (the payment it created, the ticket it opened), names that
`verb_called` as its own `cause`. So the record's history leads back to
the call, and the call to whatever the host said caused it.

