# 061 — A projection's commit waits for no flush

## What is decided

### The projection commits without waiting for its flush

On PostgreSQL the projection's transaction runs with `SET LOCAL
synchronous_commit = off`. Its `COMMIT` answers as soon as the commit
record is in the server's WAL buffers, without waiting for the flush to
disk. The SQLite engine already commits its projection this way
(`synchronous = NORMAL` in WAL mode).

A crash of the server can therefore lose a projection commit that was
not yet flushed, and only such a commit. The log and the projection
share one server and one WAL stream. Each append waits for the WAL to be
flushed up to its own commit, and that flush carries every record before
it. So the next window's append makes the previous window's fold
durable. What a crash can take is the fold of the last window, or of
the last few, where no append followed them before the WAL writer
flushed them (by default within 600 ms, three times
`wal_writer_delay`).

The projection's mark, `folded_through`, is in the same transaction as
the fold. After such a crash it stands behind the log's tip by the lost
windows, never ahead of it. A writer's open folds from the mark, as it
does after any fold that did not land, so the lost windows are folded
again from the log before anything is written. A read-only handle
answers as of the mark until a writer has opened: the lake as it stood
a window earlier, never a mixture of two positions. A writer that
outlived the server's crash is refused its next write, and the one after
takes the lease again and folds from the mark; until then its head names
the lost position while its reads answer at the mark. A lost commit of
the mark that a forgotten source's reclaim is done
(`docs/decisions/058-a-source-may-be-forgotten.md`) only repeats that
reclaim at the next writer's open.

At a window of one record the projection's `COMMIT` took 1.00–1.33 ms
synced and takes 0.17–0.18 ms without the flush; at a window of 100 it
took 1.73–1.89 ms and takes 0.20–0.21 ms (lakelands#1129).

### The log's commits wait for their flush, whatever the server sets

An append and a seal run with `SET LOCAL synchronous_commit = on`. A
server, a database or a role that sets `synchronous_commit = off` cannot
weaken them: an answered append is on disk, and its flush carries the
projection commits before it. Nothing is refused and nothing is
reported to the host, because the setting is the store's own for these
transactions.

## Why

The projection is derived from the log, and the store already folds the
log again past the projection's mark. A flush of its own buys only a
refold that a crash would have cost, and it costs one flush every
window. The log is the only record of what was answered, so its commit
is never left to the host's settings.

## What it is not

- **Not a weaker log.** The append's durability is the same as before,
  and now holds on a server set to `off` as well.
- **Not the host's remote level for the log.** `on` waits for the local
  flush and, where `synchronous_standby_names` is set, for the
  synchronous standbys' flush as well. A host's `remote_write` is
  therefore strengthened to the standbys' flush for the log's commits,
  and its `remote_apply` is weakened to it: an answered append is on the
  standbys' disks, but may not yet be visible to a read there.
- **Not every projection transaction.** The order indexes and the vector
  index that a writer's open catches up, and the projection that a
  rebuild clears, keep the server's setting. They run once per open or
  rebuild, not once per window.
- **Not the lease.** Taking the lease keeps the server's setting. The
  `lease_taken` event that follows is an append, and its flush carries
  the lease row.
