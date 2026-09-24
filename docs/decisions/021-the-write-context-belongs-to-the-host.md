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
