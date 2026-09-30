# 015 — Errors are a closed set, and every write answers an outcome

## What is decided

A write never makes its caller guess what happened. `upsert`, `retire`,
every verb call and every schema change answer an outcome: `applied`,
with the position it is visible from, or `proposed`, with the proposal
and the reason it waits. A writer does not need to know beforehand
whether its write will run or wait for review.

Every failure is one of five errors, and the set is closed:

- `refused`: the store would not do it, with the field it concerns and a
  reason from a closed list (no grant, another authority writes the
  field, a reserved name, a request that does not fit the lake's map, a
  broken constraint or precondition, a reaction chain too deep, a
  migration that would lose information, a proposal already decided, a
  position outside the log);
- `not_found`: a type, record, verb or proposal does not exist at the
  position read;
- `conflict`: the record is not at the version the write expected;
- `unbound`: the call needs a seam the host has not bound;
- `unavailable`: the lake cannot answer now (another writer holds it, it
  is read-only here, its storage failed), with a message for the
  operator.

Each error has a sentence for a reader who is not an engineer. No error
carries an engine's own code or a free-form error string as its type.

## Why

An agent acts on what a call answers. If a write can silently become a
proposal, or fail with a string that only a developer can interpret, the
agent either stops or guesses. A closed set of outcomes is one that every
door, every transport and every client can handle completely, and the
compiler checks that they do.

Keeping engine errors out of the set keeps engines interchangeable: a
caller cannot come to depend on a SQLite error code that the in-memory
engine will never produce.

## What it is not

- A proposal is not an error. It is a normal answer to a write.
- `unavailable` is not a place for errors that belong elsewhere. A
  request that is wrong is `refused`; `unavailable` is for a lake that
  cannot answer a correct request right now.
- Adding a variant is a change to the contract, not an extension point.
  Because the set is closed, a host's exhaustive `match` breaks when a
  release adds one, which is what makes it handle the new case. That is
  why every release names such an addition in its notes, under "Changes
  a host must make" (lakelands#260).

## Extended: a refusal's sentence is not part of the contract

A refusal's kind and the field it names are the contract; its sentence
is for people and belongs to the implementation. The mock and the store
word their refusals each their own way, and no door test pins a
refusal's sentence: a test checks the kind, the field and, for a door,
its own framing lines. What would otherwise tie a second implementation
to the first one's wording is left free.
