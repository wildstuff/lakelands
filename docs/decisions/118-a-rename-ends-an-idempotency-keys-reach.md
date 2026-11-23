# 118 — A rename ends an idempotency key's reach

A host retries an authored write under the same idempotency key and
expects one event. The id of such a write is derived from the writer's
name and the key, so a host that renames the writer — as one did when it
moved its verbs from a shared system writer to one writer per verb —
gets another id for the same key, and a retry in flight across that
upgrade lands as a second write. The alternatives were a scope the write
names, a public read of whether a key was applied, and a writer that
declares an earlier name. This record decides the fourth: nothing is
built, and the reach of a key ends where its writer's name changes.

## What is decided

### A key is the writer's, and a rename ends its reach

- The id of an authored write is derived from the writer's name and the
  idempotency key. One key from one writer is one write in the whole
  lake: a retry under the same key answers the first write's outcome and
  appends nothing.
- A writer's name is its kind and its id (`Principal::writer` answers
  `<kind>:<id>`). A change to either is a rename, and a rename is a
  different writer: the same key under the new name derives another id
  and is a second write, not a recognised retry. The key's reach ends at
  the rename.
- Nothing is built. No idempotency scope is added to a write, no public
  read answers whether an authored write with a key was applied, and a
  writer cannot declare an earlier name. `authored_id`, the derivation of
  ids and the log format are unchanged, so lakes and hosts that never
  rename a writer see no difference.

### The remedy is the host's, and it is a drain

- A host drains the retries it may still send under a writer's name
  before it renames the writer. Once the rename is in place, a key it
  sends under the new name is a new key.
- The lake offers no other way to learn that a try under the old name
  landed: the only "is this applied" read is an engine trait's
  (`ProjectionView::applied`), not a host's door, and no function of the
  caller's contract derives the id an earlier writer made. A host that
  has not drained the retries cannot tell a second write from a retry
  after a rename.

## Why

The writer is what the lake records and what a host holds; a key belongs
to it. Deduplicating across two names would first need the lake to know
that two names are one actor, and it can only learn that from the host —
which is a second identity model inside the store, and a declaration
every host that never renames would still carry. The window is one
retry in flight across one upgrade of a host, and the remedy — drain
the retries before the rename — is an ordering the host already
controls. Nothing changed in the log or in the derivation, so every
lake's existing idempotency history means what it meant.

## What it is not

- It is not a promise that a retry survives a rename. The three
  alternatives were considered and ruled out: a scope the write names,
  a public read of whether a key was applied, and a writer that declares
  an earlier name. Each enlarges the contract to serve a window a drain
  closes.
- It is not a defect in the store that two names make two writes. The
  host asked for a write under each name; the store has no ground to
  call them one.
- It is not a change to the refusal set, the log format or
  `authored_id`. A host meets no new refusal; it meets a new write.

## How it came to be

- Added at lakelands#1832, deciding option 4 on 2026-11-23: a rename
  ends a key's reach, nothing is built, and the host drains its retries
  before it renames a writer.
