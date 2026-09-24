# 033 — A process holds a lake once

## What is decided

The doors open a lake through `lakelands_store::open`, once per opener:
the CLI's `open_store` is the process's one opener, keeping one store per
lake — keyed by the location's engine configuration and the lake's name —
for the process's life, and every command and server session of the
process shares it. No engine keeps state beyond its opener: a lake held
in memory belongs to the `MemoryOpener` that made it and its clones, so
the CLI's one opener holds the process's memory lakes, and a test with an
opener of its own is isolated from every other test in the process. A lake
has one writer (`docs/decisions/008-one-writer-per-lake.md`), so the store
that holds the lease is the one every caller of the process writes
through, and a second session never contends for it.

- A command that writes opens the lake for writing, creating it at its
  location the first time. Each process that takes the lease appends its
  `lease_taken`, so the log records every writer that held the lake.
- A command that only reads (`feed`) opens the lake read-only: it takes
  no lease and appends nothing.
- `lakelands serve --mcp` serves the lake on standard input and output
  as the principal the command acts as (the owner, or `--as`). It is a
  writing process: it takes the lease, the log records its
  `lease_taken`, and each session it starts therefore moves the head by
  one before the agent's first call. When the
  root's configuration has a server section, the process also opens
  every lake of the root and serves them over HTTP, each caller resolved
  from the root's principals. Standard output carries the protocol only.

## Why

An MCP client may start and restart sessions freely; if each session
opened the lake, each would fight for the lease, and every open would
append another `lease_taken`. One store per lake per process makes a
session a cheap view onto a lake that is already open, and keeps the
log's record of writers to the processes that wrote.

A read that took the lease would move the head and write to a log it
only meant to look at. The walk found this: `lakelands feed` appended a
`lease_taken`, so every read shifted the positions a person writes down.

## What it is not

- It is not a pool of writers. One process writes a lake; another process
  that tries is refused at once, naming the holder.
- It does not keep a lake in memory across processes or openers. A lake
  held in memory lives as long as its opener, so a sequence of CLI
  commands on a memory lake sees a fresh lake each time; a host embeds
  the library to keep one.
