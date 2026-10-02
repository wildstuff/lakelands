# 020 — Events are hash-chained over canonical bytes

## What is decided

Every event is stored as its canonical bytes: its JSON under the JSON
Canonicalization Scheme (RFC 8785). Its hash is SHA-256 over the
canonical bytes of the event without its `hash` member, and every event
names its predecessor's hash. The first event names 64 zeros.

The log is cut into segments. Sealing a segment starts the next one with
a checkpoint event that repeats the sealed segment's last hash and lists
every sealed segment with its digest. A reader can verify any later
segment from a checkpoint without the earlier ones, and a missing or
altered earlier segment is reported at the first checkpoint that lists
it.

The store produces the canonical bytes and the hash; the engine stores
the bytes and returns them unchanged, and the store verifies what it
reads back. Byte identity is thus a property of the engine seam, checked
by the conformance suite on every engine.

Every event also carries the fencing token of the lease it was written
under, and the provenance the fold materialises per field. The
specification is `docs/log-format.md`.

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
  events where it likes, but keeps their canonical bytes unchanged and
  can export them as the specified segment files.
- It does not trust an engine to canonicalise. An engine that
  re-serialised events would break the chain for every reader; the
  engine only ever sees bytes the store produced.

## Extended: one byte form for every value

Canonical bytes only make a hash reproducible if every value has exactly
one form before it is canonicalised. The review of `docs/log-format.md`
made four rules explicit, each because two careful implementations would
otherwise have written different bytes for the same event:

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
  accepted in format version 1, and a denser form would be a new
  version.
- **A stored line is canonical or it is a break.** A reader re-serialises
  every event it parses and compares; a line that is valid JSON in
  another spelling is refused like a wrong hash.

The same review changed one preimage. An ingest event's id now includes
the record's version before the event. Without it, the id says only
"this source sent these values for this record", and a source whose
value really went back (A, then B, then A) would have its third delivery
taken for a redelivery of the first and dropped, leaving the lake on B.
With the version in it, the id says "these values against this state":
a redelivery still meets the same state and appends nothing, and a
genuine return is a new event. A re-import of unchanged data still
appends nothing, because an ingest patch that changes no field is a
duplicate before its id is looked at.

`docs/log-format/vectors/` carries a worked log that a test in
`lakelands-core` and an independent script both recompute, so the rules
are checked against bytes rather than read.
