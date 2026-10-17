# 065 — Part of a source may be taken back

## What is decided

Decided by the operator on lakelands#1153; it is built in 0.16.0. It
stands beside 039, which retracts a source as one, and 058, which
forgets a source whole.

### A host takes back part of a source: what it wrote before a time, or one delivery

`remove_values(source, selection)` removes the values the source wrote
in the selection from the records they live in. There are two
selections:
- **`written_before: T`:** the source's `record_ingested` events whose
  `known_at` is before `T`. The log's clock never falls, so this is every
  such event below one position, which the act names. What the source
  says it observed (`seen_at`) selects nothing: it may be absent, false
  or out of order. A time outside the years 0000 to 9999 is refused.
- **`delivery: D`:** the source's events of one delivery. Each
  `record_ingested` carries the delivery it came in: the id its caller
  gave, or the store's own (`d` and the position of its first event),
  answered by `ingest`. An event from before format 17 carries none, so
  no delivery selects it, and the report counts such events.

Only events before the act are selected: a later delivery under the
same id is not taken.

**A value of the selection** is a field a selected event carries,
non-null, in its patch as the log stores it.
- The same source writing the field again later keeps it from that
  event on: that event carries the value itself.
- A redelivery that changed nothing appended nothing, so it keeps
  nothing, and selects nothing.
- A value another hand wrote is never in the selection.
- A key field and a reference field identify records, so they are never
  in it.

### The act is in the log, and the record keeps everyone else's values

The act appends one `values_removed` event: the source, the selection as
asked, and for a time the first position it does not reach. A record
left holding nothing of another hand, nor of the source outside the
selection, is retired in the same append, as 039 retires it.

A record another hand also wrote stays, with that hand's values.
Co-ownership is 039's: an authored value or another source's co-owns; a
derived value, the lake's own, and a field declaring `initial` do not.

### Every removed value is redacted; where the keys allow, it is shredded

**Redacted.** The fold folds each selected value as erased, at its own
position: cleared, its provenance kept and marked `erased`, as 058 folds
a forgotten value.
- So does everything computed from it: derived fields, outputs,
  embeddings, kept views, sets and memberships. A recorded output whose
  input held a removed value is not found for the inputs left, so the
  field waits, as a write with no backend waits.
- No read answers it again, `as_of` included.
- Its bytes stay in the log, its exports and its backups: in plaintext
  for a field that is not erasable, sealed under a live key otherwise.

**Shredded.** A sealed value is also shredded where its key version is
destroyed. Versions die oldest first (058): a version is destroyed only
with every older one, so the act destroys the longest prefix of the
source's versions whose values are all in the selection.
- **Key periods** make a retention cut shred. A source sealing by period
  (`key_period`) has a version per period of `known_at`, so a cut at `T`
  destroys each version whose period ends at or before `T`. A period the
  cut falls inside is redacted only, and the report counts it apart.
- **One delivery is not shredded on its own.** Its values seal under the
  source's version of their period, beside the values of every other
  delivery in it. Destroying that version takes them too; destroying it
  alone, with older versions left, breaks the prefix rule, which is what
  lets an output sealed under the oldest of its inputs' versions die with
  the first of them. So a delivery taken back is redacted, and is
  shredded only once a retention cut's prefix covers it. A key outside
  the chain for one delivery is lakelands#1214.

**A version gone with no act in the log is lost, not destroyed.** The
fold reads a sealed value with no key as erased only where an act in the
log names that version, a `source_forgotten` of the source or a
`values_removed` that destroyed it; any other is a key the store lost,
and the fold fails closed, as 058 says.

The report says, per type and field, how many values were redacted and
how many shredded. A host whose rule is "destroyed or nothing" asks
`require: shredded`, and the act is then refused while any selected
value would only be redacted.

### The act may be read before it is made

`remove_values_preflight(source, selection)` answers the act's report
and appends nothing. It reads on any handle and takes no lease. The
report gives, in counts and the first keys:
- the selection, and for a time the first position it does not reach;
- the events selected, and for a delivery those that keep none;
- per type, the records reached; per field, the values taken;
- the records retired, and those kept;
- what is computed again;
- the constraints the records it keeps would be left breaking;
- the references its retirements strand;
- the log's length, and the time it is expected to hold the writer's
  lock at the engine's measured rate.

`RemoveReport::said` puts it in sentences a host can tell its operator.

Its id is of the source, the selection and the head. The act refuses a
report of another head.

The act never refuses for a constraint. A record it leaves breaking one
is listed, and the host decides by the ordinary doors.

### It runs as 058's act runs

Under the writer's lock, in steps, each idempotent:
1. the append;
2. the destroy of the versions it shreds;
3. the rebuild, in which the fold knows every act before it meets the
   values they select;
4. the reclaim.

The projection keeps, under a name of the store's own, which acts are
owed. A rebuild reads that list before it clears the projection, so it
knows each act from its first event. One it meets unknown, as after an
import or a lost projection, it leaves owed, and the rebuild after it
knows it. A crash between steps is finished by the next writer's open.
A repeat of the act finishes it and appends nothing.

### It is the host's act

Like `purge_source` and `forget_source`, it is no door: an agent cannot
take a source's values back, and no principal's sight trims the report.
A reader after it is answered as after 058: an erased value is absent,
and its mark is shown only on a field the reader may read.

## Why

A host lets a source keep its data for a period, or lets an operator
take back one delivery. Until now the only safe rule was to keep such a
source out of every type a person also writes: a purge refuses a
co-owned record whole, and forgetting takes the whole source. That
forced a type per source and a reference to join them back. Taking back
values, not records, lets the source and the people write the same
record.

**Redaction is the floor, because it is always possible.** It needs no
key and works for every field, and reads keep the promise a retention
period makes. Shredding is better and costs the host a key version per
period, so it is offered where the host arranged for it, and the report
never claims it where it did not happen.

**Rejected: removing only from the act on.** `as_of` would still answer
what the lake says it no longer holds, and an author's clear already
does that.

**The act rebuilds.** A rebuild is the projection's fold of the log, so
no second path has to come out equal to it. It holds the writer for the
whole log, which the preflight says; a scoped refold is lakelands#1120.

## What it is not

- It does not remove bytes from the log. The chain, its hashes, its
  checkpoints, the exports and the mirrors' positions stay valid (001,
  054). Only shredded values are beyond reading.
- It does not reach copies outside the lake: backups, archives, an
  export made before, a mirror's target past its stop.
- It does not select by the source's own time, or by anything but the
  source and the selection.
- It is not a door, and not a purge: it never refuses a co-owned record,
  and it retires only what holds nothing else.
