# 065 — Part of a source may be taken back

## What is decided

Decided by the operator on lakelands#1153; it is built in 0.16.0, and a
delivery sealed apart, a selection by the time the source observed and a
set of deliveries in one act in 0.17.0 (lakelands#1214, lakelands#1166,
lakelands#1290). It
stands beside 039, which retracts a source as one, and 058, which
forgets a source whole.

### A host takes back part of a source: what it wrote before a time, in one delivery or a set of them, or what it saw before a time

`remove_values(source, selection)` removes the values the source wrote
in the selection from the records they live in. There are four
selections:
- **`written_before: T`:** the source's `record_ingested` events whose
  `known_at` is before `T`. The log's clock never falls, so this is every
  such event below one position, which the act names. What the source
  says it observed (`seen_at`) plays no part in it. A time outside the
  years 0000 to 9999 is refused, by this selection and by `seen_before`.
- **`delivery: D`:** the source's events of one delivery. Each
  `record_ingested` carries the delivery it came in: the id its caller
  gave, or the store's own (`d` and the position of its first event),
  answered by `ingest`. An event from before format 17 carries none, so
  no delivery selects it, and the report counts such events.
- **`deliveries: [D, …]`:** the source's events of any of a set of
  deliveries, in one act (lakelands#1290). A host taking back many
  deliveries would otherwise pay the walk of the log and the rebuild once
  for each; the set pays them once. It takes what the acts taking each
  back in turn would take, and appends one event, which names the set's
  ids in order, each once. An id the log never held selects nothing, and
  an empty set nothing at all. The report adds
  up as theirs would: the events and the values selected are the sum of
  each delivery's, while a record two of them reach is counted once, and
  retired or kept as the last of those acts would leave it. What it
  shreds is decided over the set as one selection, by the rules below.
- **`seen_before: T`:** the source's events whose `seen_at`, the time
  the source says it observed the values, is before `T`
  (lakelands#1166). That time is the source's word, not the log's: it
  need not rise along the log, so the selection is a set of events
  spread across it, not one cut, and the act names no position. A late
  delivery of old observations is selected wherever it lands, and an
  early event that says it observed later is not. An event that states
  no `seen_at` is never selected, and the report counts such events
  apart. The selection is only as true as the source: a false `seen_at`
  keeps a value or takes it by that falsehood.

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
  in it: a field that is one at the event's position or at the act's. A
  value written while its field was part of a key is in the record's key
  there, and one whose field became part of a key since is in the key a
  re-keying moved it to (lakelands#1228).

The records reached are found by the key each selected event wrote,
followed through every rename and re-keying migration since, to the key
the record bears now.

### The act is in the log, and the record keeps everyone else's values

The act appends one `values_removed` event: the source, the selection as
asked, and for a time of the log's clock the first position it does not
reach. A record
the source made, left holding nothing of another hand, nor of the
source outside the selection, is retired in the same append, as 039
retires it. A record another hand made is never retired: the act erases
the source's values there and keeps it (069). Who made a record is read
at its `made` position, as the purge and forgetting read it (039).

A record another hand also wrote stays, with that hand's values.
Co-ownership is 039's: an authored value or another source's co-owns; a
derived value, the lake's own, and a field declaring `initial` on a
record the source made do not.

### Every removed value is redacted; where the keys allow, it is shredded

**Redacted.** The fold folds each selected value as erased, at its own
position: cleared, its provenance kept and marked `erased`, as 058 folds
a forgotten value.
- So does everything computed from it: derived fields, outputs,
  embeddings, kept views, sets and memberships. A recorded output whose
  input held a removed value is not found for the inputs left, so the
  field waits, as a write with no backend waits.
- **A membership the log holds** — of a set whose `where` reads across
  an edge (055), or of a set of a kept view (066) — was decided at a
  write, over the values then. From the first value an act erased that
  such a set reads, the fold decides the set itself, at every write that
  could move it, as a write decides it, and the logged
  `object_set_changed` no longer stands for it. No membership is one an
  erased value made, at the head or `as_of` (lakelands#1232).
- **A reaction the log holds** — a `trigger_fired`, a `trigger_dropped`,
  an `object_set_changed` — is a fact of the write that made it, and
  stays in the log, its exports and the raw feed. Where an act erased a
  value its condition read, and the history without the value does not
  make it, it is *overruled*: the feed names it in `overruled` (004), no
  door tells it (the MCP pump, a replayed write's outcome), and the
  deliverer does not post it (lakelands#1238). One that history makes
  too stands. One that history would make, but the log does not hold, is
  not made: an act runs no trigger over what it erases. The act's own
  retirements fire in its append, decided over the values it takes, and
  are decided again like any other.
  - **How.** As the write decided it, over the projection as of its
    cause, which the rebuild left without the value: a trigger's `when`
    over the record then and just before, a path across an edge read
    then, with the settings then; a set's change where the membership
    before the cause is not the one it moved to and the membership after
    it is. For a set across an edge or of a kept view that membership is
    the kept one. For any other it is the `where` over the record, and
    before the cause the membership the write read, kept by an earlier
    decision, unless the record then holds an erased value the `where`
    reads: a `where` that reads the clock moves without a write. A
    trigger with no `when`, or withdrawn at its cause, stands.
  - **What it costs.** Only a reaction before the lake's last act, or
    sharing its time as every event of its append does, is decided: each
    by a few point reads, and by the shape at its cause, which an `as_of`
    read pays too, once per cause. No type is scanned. A lake that never
    erased anything pays one atomic load a poll. While a rebuild refolds
    the log, a poll stops before the first reaction the projection cannot
    decide yet, and the next poll reads on from it.
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
- **One delivery is shredded on its own where it was sealed apart.** An
  ordinary delivery's values seal under the source's version of their
  period, beside the values of every other delivery in it. Destroying
  that version takes them too, and destroying it alone, with older
  versions left, would break the prefix rule. So such a delivery taken
  back is redacted, and is shredded only once a retention cut's prefix
  covers it. A delivery the source's ingest sealed apart (below) is
  shredded when it is taken back.
- **A time seen shreds only where a key seals nothing else.** Versions
  follow `known_at`, not `seen_at`, so a version is destroyed by
  `seen_before` only where every value it seals states a `seen_at`
  before `T` or was taken by an earlier act, and so does every older
  version: in practice a source that delivers in the order it observes
  and states the time on every record. A key a delivery sealed apart is
  in no chain, so a delivery each of whose values states a `seen_at`
  before `T`, or was taken before, is shredded on its own,
  wherever in the log it stands: a source that seals each delivery apart
  has a late delivery of old observations shredded, not only redacted. A
  version or a key apart that also seals a value seen at `T` or after, or
  one that states no time, stays, and the values it seals in the
  selection are redacted only, counted in `unshredded`; `require:
  shredded` then refuses the act.

The act names the versions it destroys in its event, and drops them
from the open lake's memory, so the lake seals and opens with none of
them again. A source whose newest version an act destroyed is given a
new one at its next sealed value, not refused as having lost it; a key
store never gives a new version the id of one it destroyed.

**A version gone with no act in the log is lost, not destroyed.** The
fold reads a sealed value with no key as erased only where an act in the
log names that version, a `source_forgotten` of the source or a
`values_removed` that destroyed it; any other is a key the store lost,
and the fold fails closed, as 058 says.

The report says, per type and field, how many values were redacted and
how many shredded. A host whose rule is "destroyed or nothing" asks
`require: shredded`, and the act is then refused while any selected
value of an erasable field would only be redacted. A value of a field
that is not erasable was never sealed, so it is redacted and counted,
and never holds the act back.

A key store that destroys a source's key only whole destroys no
version: the act names none, and every value it takes is redacted only,
as the report says. The act does not refuse for it, since redaction is
always possible; `require: shredded` then refuses it.

### A delivery may be sealed apart

An ingest that asks `sealed_apart` (`IngestOpts`) gives its delivery a
key of its own, outside the source's versions, so that the delivery can
be shredded alone (lakelands#1214).

- **What it seals under.** Each value of an erasable field is sealed
  under the source's version of its period, as every ingest seals it,
  composed with the delivery's own key: one key derived from both, the
  version first. `sealed` names the version and, in `apart`, the
  delivery's key. Each ingest call makes one such key per source it
  writes for, at its first sealed value, so a later delivery under the
  same id has a key of its own. An ingest that writes no value of an
  erasable field seals nothing and asks for no key. A call still
  writing when an act took its delivery back takes a new key for what
  it writes after the act, which the act did not select. A key made for
  a call that then appended nothing under it, all duplicates or refused,
  seals nothing and is destroyed with the source.
- **Destroying the apart key takes that delivery alone.** The versions
  before it and after it, and every other delivery in its period, stay
  readable. The key is in no chain, so the prefix rule is untouched, and
  destroying its version still takes it, as a retention cut takes every
  value of its period.
- **An output dies with either key.** An output is sealed under the
  oldest version that sealed a value its call reads, composed with every
  apart key that sealed a value it reads, in the order of their ids. Its
  input is digested under that composed key. No digest of it can be made
  again, and it cannot be opened, once any one of those keys is gone. An
  output that reads no value sealed apart is sealed under the version
  alone, as before. An output reads which apart keys sealed a value
  from the `sealed.apart` of the value's own event in the log, so a
  write's plan and a rebuild compose the same keys, and the projection
  keeps no copy of them per event
  (`docs/decisions/083-a-log-stores-what-an-append-shares-once-and-seals-a-records-values-in-one-box.md`).
  The fold's list of a source's versions marks each version a delivery
  sealed apart sealed under, and an output looks up the apart keys only
  at a value whose version is marked: a source that never sealed apart
  costs no read.
- **The act destroys every apart key whose values it all selects.** For
  a delivery, that is the keys of the ingests of that delivery before
  the act. For a time of the log's clock, it is the keys of the ingests
  below the position the act names. For a time seen, it is the keys
  every one of whose ingests states a `seen_at` before it. It names them
  in its event, in `apart`, beside the prefix of versions, and the
  report names them too. A value under an
  apart key the act destroyed is shredded, so `require: shredded` is met
  by a delivery sealed apart.
- **Forgetting the source destroys its apart keys** with its versions
  (058).
- **A key store that makes no such key refuses.** An ingest that asks
  for `sealed_apart` is refused as unbound at its first value of an
  erasable field, and that value's window appends nothing; the windows
  before it, which sealed nothing, stand, as any window does. The
  default key store makes no apart key, and an act's finish asks a key
  store to destroy only an apart key it holds, so a lake imported beside
  such a store opens.
- **A redelivery is its own.** An ingest's id is made from the patch as
  stored and, from format 20, from the box that seals its values (§3 of
  the log format), so a delivery sealed apart is never a
  duplicate of another delivery's event by its id. Taking back the first
  then leaves the second's values standing, where the second came under
  another delivery id; one id selects every delivery under it before
  the act. One that changes nothing still appends nothing, as any
  ingest that changes nothing does.

**Why it is composed, not apart alone.** A delivery sealed under its
apart key alone would name, in `sealed.key`, a key that is in no chain:
the list of versions a source sealed with, and the refusal of a key store
that lost the newest of them, would each need an exception for it, and a
retention cut that covers its period would leave it readable. Composed,
the version is still the one its position says, and only the apart key
is new.

**Why an output is composed too.** An output over an apart value and an
ordinary value of the same source must die with whichever key goes
first. Sealed under the version alone, its digest would outlive the
delivery taken back; sealed under the apart key alone, it would outlive a
retention cut. Refusing such an output, as a call over two sources'
values is refused, would leave empty every computed field of a record
the source wrote in an ordinary delivery and corrected in one sealed
apart, which is the case the option serves.

**Rejected: an apart key derived from the version.** Destroying it alone
would still need a secret of its own, so it is the composition with one
more step. **Rejected: sealing twice**, under the version and then under
the apart key: the same lifetime at twice the work per value and per
digest. **Rejected: the apart key in each field's provenance.** That is
what a reader is shown (`with: meta`); the event's `sealed` keeps it
from readers.

### The act may be read before it is made

`remove_values_preflight(source, selection)` answers the act's report
and appends nothing. It reads on any handle and takes no lease. The
report gives, in counts and the first keys:
- the selection, and for a time of the log's clock the first position
  it does not reach;
- the events selected, and for a delivery or a set of them the source's
  events from before format 17, which carry no delivery and so are never selected;
  for a time seen, the source's events that state no `seen_at`, which it
  never selects;
- per type, the records reached; per field, the values taken, and of
  them those shredded;
- the versions of the source's key it destroys, and the values of
  erasable fields it redacts only;
- the records retired, and those kept;
- what is computed again;
- the constraints the records it keeps would be left breaking, and the
  fields declared `required` it leaves empty, each as a breach of
  `required` naming the field;
- the references its retirements strand;
- the log's length, and the time it is expected to hold the writer's
  lock at the engine's measured rate.

`RemoveReport::said` puts it in sentences a host can tell its operator.

Its id is of the source, the selection and the head. The act refuses a
report of another head.

The act never refuses for a constraint, nor for a `required` field it
leaves empty. A record it leaves breaking one is listed, and the host
decides by the ordinary doors; until it does, the next write of that
record is refused as any write that leaves a required field empty is.

### It runs as 058's act runs

Under the writer's lock, in steps, each idempotent:
1. the append;
2. the destroy of the versions it shreds;
3. the rebuild, in which the fold knows every act before it meets the
   values they select;
4. the reclaim.

The projection keeps, under a name of the store's own, which acts are
owed. A rebuild reads that list before it clears the projection, so it
knows each act from its first event. A value sealed under a version an
act destroyed, which the fold meets before it knows that act, as after
an import or a crash midway through a rebuild, sends it ahead in the log
for the act, as 058's fold looks ahead for a forgetting: such a value is
erased, never a lost key. One it meets unknown it leaves owed, and the
rebuild after it
knows it. A crash between steps is finished by the next writer's open.
A repeat of the act finishes it and appends nothing.

What an earlier act took is not taken again. The act asked again, once
the earlier one is done, takes only what the source wrote since in the
selection, a later delivery under its id or under one of the set's,
and appends nothing where that is nothing.

From its append on, a version the act names seals nothing, whether the
key store has destroyed it yet or not: a value sealed under it would be
erased with it, though the act never selected it. So while the act is
owed, a source whose newest version it names is refused its next sealed
value, `key_missing`, as a lost key is. A rebuild counts the act rebuilt
only once the key store holds none of its versions, so one whose destroy
failed stays owed. The writer's next open, a repeat or the rebuild's
finish destroys them, and the source is then given a new version.

### It is the host's act

Like `purge_source` and `forget_source`, it is no door: an agent cannot
take a source's values back, and no principal's sight trims the report.
A reader after it is answered as after 058: an erased value is absent,
and its mark is shown only on a field the reader may read.
The CLI's `lakelands values remove` is the host's too: it runs the act in
the host's process, on the host's lake.

### The doors around it

- **The CLI.** `lakelands values remove --source <id>` with
  `--before <instant>` (RFC 3339, or a date at midnight UTC),
  `--delivery <id>`, given once or more, the set as one act, or
  `--seen-before <instant>`, one of the three, and
  `--require-shredded`, as `forget` is shaped: without
  `--report`, as with `--preflight`, it prints the report's sentences and
  the id, appending nothing, and only `--report <id>` acts. The server
  has no door for the act and nothing to configure for it: the doors are
  the CLI and the host's own code.
- **A mirror stops.** A mirror's target holds plaintext copies the act
  cannot reach, redacted values among them, so a mirror that reads
  `values_removed` of any source stops each running mapped type with a
  field its reader reads that a removal may take: one a source writes,
  or derived from one, that is neither a key field nor a reference,
  erasable or not. The event names no type, and the copies hold only
  what the reader saw, so the type as the reader sees it decides, never
  the values the act reached: a field kept from the reader changes
  nothing. The stop names the source and the way out: copy the type
  again, or run `lakelands values remove` on `mirror:<lake>:<type>` in
  the target, then restart. A type of key and reference fields goes on.
- **An export carries the act.** An import folds it again, so the
  values arrive redacted, and needs no key for a version or an apart key
  the act destroyed, nor for the other keys of a value sealed under one;
  a value whose keys it left still needs them, and is refused as unbound
  before anything is written where none is bound. A key store restored
  from a backup that still holds a destroyed key has it destroyed by the
  import's finish.
- **The ingest doors carry the seal.** `IngestOpts::sealed_apart`, and
  `?sealed_apart` on the REST ingest; a catalog that does not carry it
  to the store refuses it, `sealed_apart_not_carried`, rather than seal
  the delivery under its version alone.
- **A reclaim that fails is owed**, for this act as for 058's. The act
  and `rebuild()` answer all the same, the act naming the cause in
  `reclaim_owed`, as a preflight does; an import answers, and the
  writer's next open tells the cause. The next open, a repeat or a
  compact that runs clears it.

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

**A time seen is the source's word, and the report says where it is
silent.** A retention rule is often stated in the source's time: what it
observed before a date. The lake cannot check that time, so the act
takes it as said, and the report counts the events that state none
rather than guessing for them. **Rejected: falling back to `known_at`**
for an event that states no `seen_at`. One selection would then read two
clocks, and which values it took would turn on which records the source
happened to stamp; a host that wants those events gone too takes them by
`written_before` or by their delivery, in an act of its own.

**Rejected: removing only from the act on.** `as_of` would still answer
what the lake says it no longer holds, and an author's clear already
does that.

**The fold decides a membership again from the first erased value to
the end of its run,** not only where a logged change read that value.
Some transitions of the history without it were never logged, since at
the write the membership did not change: a set the erasure makes (`where
stop.place is null`), one another value kept (`stop.place = 'Rome' or
stop.open`, `open` falling while Rome still held), and a referrer moving
from the erased record to another in Rome. The run's end, not the act,
bounds it, since an act a crash left owed lets writes land before its
rebuild. A fold that begins midway in a lake with an act, as one
continuing a stopped rebuild, cannot tell what was erased before it
began, so it decides every such set from its first event.

**The act rebuilds.** A rebuild is the projection's fold of the log, so
no second path has to come out equal to it. It holds the writer for the
whole log, which the preflight says; a scoped refold is lakelands#1120.

## What it is not

- It does not remove bytes from the log. The chain, its hashes, its
  checkpoints, the exports and the mirrors' positions stay valid (001,
  054). Only shredded values are beyond reading.
- It does not reach copies outside the lake: backups, archives, an
  export made before, a mirror's target. A mirror stops at the act, and
  its target keeps what it copied until the host takes it back there.
- It selects by nothing but the source and the selection: no type, no
  field, no value.
- It is not a door, and not a purge: it never refuses a co-owned record,
  and it retires only a record the source made that holds nothing
  else.

## How it came to be

- Built in 0.16.0: a source's values taken back by a time or by one
  delivery, redacted always and shredded by key periods (lakelands#1153).
- A delivery sealed apart, under its version composed with a key of its
  own, is shredded alone, and an output over it dies with either key
  (lakelands#1214).
- A record another hand made, its key the author's only value, is kept,
  not retired: the source's values there are erased (069,
  lakelands#1264).
- A selection by the time the source observed, `seen_before`, whose
  events stand anywhere in the log, shredded only where a version or a
  delivery's key apart seals nothing outside it (lakelands#1166).
- A set of deliveries taken back in one act, with one walk of the log
  and one rebuild (lakelands#1290).
- An output reads the apart keys from the value's own event, where the
  fold kept a copy of them for each event; proposed with 083
  (lakelands#1413).
