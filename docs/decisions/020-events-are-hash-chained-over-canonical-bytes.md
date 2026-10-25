# 020 — Events are hash-chained over canonical bytes

## What is decided

### Every event is hashed over its canonical bytes and names its predecessor's hash

Every event has canonical bytes: its JSON under the JSON
Canonicalization Scheme (RFC 8785). Its hash is SHA-256 over the
canonical bytes of the event without its `hash` member, and every event
names its predecessor's hash. The first event names 64 zeros.

From format 20 the log stores each event in a stored form, from which
its canonical bytes are restored exactly: an append of two or more
events shares one frame of the members they all share, and `position`
and `prev` stand only on a segment's first line
(`docs/decisions/083-a-log-stores-what-an-append-shares-once-and-seals-a-records-values-in-one-box.md`).
Up to format 19 the stored form of an event is its canonical bytes.

The log is cut into segments. Sealing a segment starts the next one with
a checkpoint event that repeats the sealed segment's last hash and lists
every sealed segment with its digest. A reader can verify any later
segment from a checkpoint without the earlier ones, and a missing or
altered earlier segment is reported at the first checkpoint that lists
it.

The store produces the canonical bytes, the hash and the stored form;
the engine stores the stored form and returns it unchanged, and the
store restores and verifies what it reads back. Byte identity is thus a
property of the engine seam, checked by the conformance suite on every
engine.

Every event also carries the fencing token of the lease it was written
under, and the provenance the fold materialises per field. The
specification is `docs/log-format.md`.

`docs/log-format/vectors/<N>/` carries, for each format, a worked log that a test in
`lakelands-core` and an independent script both recompute, so the rules
are checked against bytes rather than read.

### Every value has exactly one form before it is canonicalised

Canonical bytes only make a hash reproducible if every value has exactly
one form before it is canonicalised. Four rules make it so, each because
two careful implementations would otherwise have written different bytes
for the same event:

- **Numbers are doubles, and integers stay within 2^53 − 1**, in
  payloads as well as in the envelope. Beyond that range, "the digits as
  written" and RFC 8785's number form disagree, so a larger integer is
  refused at the door and a value that needs the digits is a `decimal`.
- **A date-time has one form in the log.** The doors accept friendlier
  input and normalise it; a reader refuses any other form, because a
  valid instant in a second spelling is a second byte string.
- **A vector is an array of numbers, like every other value in its JSON
  form**, with 32-bit floats widened exactly to doubles, never rounded
  through a shorter decimal. A binary encoding would be smaller, but it
  would give the log a second codec to verify for one kind; the size is
  accepted, and a denser form would be a new format version.
- **A stored line is in the one stored form or it is a break.** A
  reader re-serialises every line it parses and compares, and checks
  each frame against the lines it covers; a line that is valid JSON in
  another spelling, or a frame or line that holds other members than the
  one form gives it, is refused like a wrong hash.

### An ingest event's id includes the record's version before the event

An ingest event's id includes the record's version before the event.
Without it, the id says only "this source sent these values for this
record", and a source whose value really went back (A, then B, then A)
would have its third delivery taken for a redelivery of the first and
dropped, leaving the lake on B. With the version in it, the id says
"these values against this state": a redelivery still meets the same
state and appends nothing, and a genuine return is a new event. A
re-import of unchanged data still appends nothing, because an ingest
patch that changes no field is a duplicate before its id is looked at.
From format 20 the id of an ingest whose values are sealed in one box
covers the box too (083).

## Why

An append-only log is only the truth if a change to it can be seen
(001). A hash chain makes any edit, removal or reordering visible at the
first event after it, to anyone holding the log, without trusting the
engine that stored it.

Canonical bytes are what make the hash reproducible outside Lakelands.
A format that depended on one serialiser's field order or number
printing could be verified only by that serialiser; RFC 8785 can be
implemented in any language, so a lake can be verified and replayed by
tools that are not ours.

Checkpoints make verification practical for a long log and make archive
loss loud rather than silent: a deleted segment breaks the chain at a
recorded point.

## What it is not

- It is not a signature. The chain shows that a log is internally
  consistent, not who wrote it; signing is outside `0.1.0`.
- It is not encryption. Encryption at rest is a separate, later concern
  configured through `Location`.
- It does not fix one storage layout for every engine. An engine keeps
  events where it likes, but keeps their stored form unchanged and can
  export it as the specified segment files.
- It does not trust an engine to canonicalise or to frame. An engine
  that re-serialised events would break the chain for every reader; the
  engine only ever sees bytes the store produced.

## How it came to be

- The first version: canonical bytes, the hash chain, segments with
  checkpoints, and byte identity at the engine seam.
- The review of `docs/log-format.md` made the four rules of one form
  explicit, put the record's version before the event into an ingest
  event's id, and added the worked log.
- At the rewrite (lakelands#817): the sentence that accepts a vector's
  size names no format version by its number; `docs/log-format.md` has
  it.
- The log stores a stored form, from which each event's canonical bytes
  are restored, and the one-form rule covers it; proposed with 083
  (lakelands#1413).
