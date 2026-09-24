# 008 — One writer per lake, enforced by a lease with a fencing token

## What is decided

A lake has exactly one writing process at a time. The writer holds a
lease in the lake's location, and the lease carries a fencing token
that increases each time a new writer takes it over. Every append is
checked against the current token; an append made under an older token
is refused.

A second process that tries to open the lake for writing fails at once,
with an error that says the lake is held by another writer. Any number
of processes may open the lake read-only.

## Why

A hash-chained log has a single order, and two writers appending to it
would fork it. A lock with a bounded wait does not prevent that: a
writer that stalls past its lock, then wakes up, can still append after
someone else took over. The fencing token closes that gap, because the
late writer's token is stale by the time it tries.

Failing fast and legibly is the other half. A process that waits
silently for a lock looks like a hang; one that says "this lake has a
writer already" tells the operator what to do.

## What it is not

- It is not one writer per process. One process may hold the leases of
  many lakes and write to all of them.
- It is not a limit on concurrent callers. Many principals write to one
  lake through its one writer; the writer orders their appends.
- It is not replication. Standby writers and failover are outside this
  decision; the lease is what makes them safe to add later.
