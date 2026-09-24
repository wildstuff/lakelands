# 030 — A lake directory verifies without its engine

## What is decided

The SQLite engine keeps a lake as a directory whose log anyone can read:

- `log/<first position>.jsonl` holds the segments exactly as the log
  format names them, each event's canonical bytes and a line feed,
  nothing else. A verifier needs the format and these files, not the
  engine.
- `index/<first position>.idx` holds, per event, the members the engine
  was handed beside the bytes (position, id, `prev`, `hash`, fence) and
  where the bytes lie, and one commit line closing each append. An
  append writes and syncs its segment lines, then its index lines and
  the commit line. Only events before the last commit line exist, so an
  append is whole after a crash or not there at all. The next writer
  cuts off what an interrupted append left.
- `lease` holds the last fencing token handed out and its holder, under
  an operating-system lock. The lock makes a second writer fail at once
  and dies with its process; the token, read from the file on every
  append and every seal, is what refuses a writer that lost the lease.
- A handle opened read-only creates nothing and writes nothing: it opens
  the projection read-only, and the store folds only on the writer's
  handle.
- `projection.db` holds the projection with its kept history. It is
  derived from the log and can always be folded again.

## Why

The log is the truth (001), so the truth should be readable without the
program that wrote it. With the segments as plain files, an auditor can
check a lake's hash chain and its checkpoints with a script of a few
dozen lines, and a lake can be moved or archived by copying files.
Keeping the members in a separate index keeps the segment files pure
while still letting the engine return the members without parsing the
bytes, and the commit line gives multi-event appends the all-or-nothing
property a file system does not.

A lock alone does not make a lease safe: a process can hold a lock it
believes it lost, or lose one it believes it holds. A token that only
grows, checked on every append, stops a stale writer whatever its lock
says. A lock alone would also outlive a crashed writer if it lived in a
file's contents; the operating system's lock does not.

## What it is not

- It is not a promise that the index or the projection can be read by
  another program. They are the engine's; only the segments are the
  format.
- It is not a replication scheme. Copying a directory while a writer
  appends can catch an uncommitted tail, which the next writer cuts off.
- It is not a requirement on other engines. An engine that keeps events
  elsewhere keeps their bytes unchanged and can export these files.
