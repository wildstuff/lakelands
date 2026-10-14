# 004 — The change feed is the log

## What is decided

### The feed is the log filtered by kind, read from a position

The change feed is the event log filtered by kind. There is no separate
feed table, queue or outbox. What a consumer needs to react to is
appended to the log as events in the same append as the write that
caused it: a record changed, a trigger fired (`TriggerFired`), an object
set gained or lost a member (`ObjectSetChanged`).

A consumer reads the feed from a log position. The consumer keeps its
position and acknowledges on its own side; the store keeps no consumer
state. The dedup key of a delivered change is its lake and its position.

The contract names a feed reader's place a **position**, never a cursor:
the feed is polled `from` a position and answers the position to poll
from next, and a consumer acknowledges `through` a position. In
Lakelands `cursor` means only the page token of a list-shaped read
(`docs/glossary.md`), and a source's own place in its data is its
**offset**.

### A poll reads and opens a bounded part of the log

A poll stops at `limit` matches, at the head, or where the index stops
deciding (below).

- **A poll also stops after `FEED_EXAMINED` events read and opened,**
  matched or not (4 096, about 0.1 s on SQLite), and answers `next`
  there. A `limit` above it answers at most that many events. The
  positions the index skips are not counted.
- **Caught up is `next` past `head`.** An empty batch may stop short of
  the head, so a consumer reads on while `next` is at or below `head`.
  The store's own consumers (the MCP pump, the deliverer, the mirror)
  follow that rule.
- **It is a constant, not a lake's setting:** a remote consumer must be
  able to know it, and no door reads it from the caller.

**Why.** Before, a poll stopped there alone. A filter that matched
nothing therefore read and verified the whole log in one call: 2.4 s at
100 000 events on SQLite, where a poll that matched answered in 5 ms.
A consumer that starts at position 0 with a narrow filter paid that in
one call, and a `limit` without a filter held every event it asked for.

### The projection keeps an index of the log, and the log answers

- **The fold keeps an index.** It remembers every event it applies, to
  recognise a redelivery. It also keeps the event's kind and what it is
  about: one entry a type it concerns, with the record's key where it
  names one. A `migrated` is about the link types it moves records
  between, and a `value_type_migrated` about every type it rewrites, as
  their filters always admitted them. One rule in `lakelands-core`
  (`feed::about`, `feed::admits`) says what the fold keeps and what a
  poll checks.
- **The index narrows, the log answers.** Up to the view's position, a
  poll asks the index for the positions its filter admits and reads only
  those from the log, opening, verifying and checking each again. The
  events a run reads through between two of them are dropped unopened,
  since nothing is answered from them. Past the view's position, while
  the projection is rebuilt, it reads and verifies every event, whatever
  matches.
- **The index decides only what is folded.** A rebuild's `clear` and its
  refold commit apart from the view a poll took, so an engine bounds what
  it decides by the position the projection has folded, read in the same
  snapshot as the index: a view taken before the `clear` decides only
  what the refold has reached, and nothing where it has not reached the
  poll's position, which then reads the log. Deciding by the view's
  position alone would pass the events of the rest by, for good.
- **What a consumer sees.** The same events in the same order. `next`
  moves past what the index decided, so a filter the index decides
  whole and that matches nothing more is caught up in one poll, and one
  record's history from 0 reads that record's events alone. A filter
  that names types and kinds and no key is decided at most 65 536
  positions a poll (`feed::WINDOW`), so it is caught up that many
  positions at a time.
- **Not the log.** The log format, the events and the engine's log seam
  are unchanged. The index is the projection's, so `rebuild` and an
  import fold it from the log, and `compact` keeps it. A projection
  made before it holds no index; such a lake is rebuilt
  (`lakelands rebuild`), with no path that repairs it in place.
- **A projection is read only by its layout.** An empty `routes` would
  not fail: a filtered poll over it would find nothing, and a consumer
  would skip events without being told. So the SQLite and PostgreSQL
  engines mark a projection with their layout number in `meta` when they
  make it, and a writer's, a creator's or a reader's open of one marked
  otherwise, or not at all, is refused before any of its tables is read
  or made (`projection_layout`, naming the number found, the number
  wanted and `lakelands rebuild`). An engine raises its number whenever
  a table changes so that an older projection would answer wrongly. A
  lake is opened `OpenMode::Rebuild` to rebuild it: the store takes the
  lease, `clear` drops the projection and makes it at the engine's
  layout, and the whole log is folded. Nothing repairs a projection at an
  open the host did not ask to rebuild.

**Why.** Before, a poll with a filter read and verified every event from
its position to the head, whatever matched: about 24 µs an event, 2.4 s
in 25 polls to catch up 100 000 events, on SQLite and PostgreSQL alike.
Nine tenths of it was opening events the filter then dropped.

**Why in the projection.** This decision keeps a feed out of the
projection because a feed kept there would be lost on a rebuild. An
index is not a feed: it answers nothing a poll returns, it is folded
again with everything else, and a poll that found it wrong would only
read less. Kept beside the log in each engine's log store, it would have
to be written in every engine's append and made whole after every crash
the log survives; in the projection, the fold's transaction already
makes it whole with the rest.

### A consumer's receipts are the host's

What a consumer has done with the feed is consumer state, and it stays
on the consumer's side, never in the lake. That covers what it
delivered, what it tried again and what a receiver refused. Lakelands'
own consumer, the webhook deliverer that `lakelands serve` runs, keeps
it on the host: one append-only journal per endpoint and lake, beside
the file holding the position it reads from (`docs/seams.md`
§`ChangeFeed`).

**Why.** A receipt written into the lake would be a write, and the feed
would carry every write back to the consumer that made it. Each
delivery would then make more of the log that consumers read. It would
also make one consumer's progress part of every other reader's lake,
which is the coupling this record keeps out.

### A trigger fires on every write of its type whose guard holds

A trigger fires on every write of its type — `upsert`, `ingest`, a verb's
write, an accepted proposal's write, `retire` — whose guard holds over the
record after the write. The guard sees the record before the write through
`changed` and `previous`, so a transition is the guard's to state; a guard
that cannot be evaluated does not fire. The `trigger_fired` event is
appended in the same append as the write, naming it as its cause, and an
object set's `object_set_changed` is appended the same way, only when a
record's membership actually changes.

A trigger's guard reads the write: the record's new values, `changed(f)`,
and `previous(f)`, the value before the write, which is null for a record
the write creates (or revives from retirement). A transition is a guard,
`previous(status) = 'open' and status = 'closed'`, and fires on that
transition only. A guard reads no other record. A trigger may also cap
the depth of the reactions that name its firing as their cause
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`, extended).
The firing is appended whatever the depth, since it is the durable fact a
host reacts to.

### A guard reads the provenance its write leaves

A guard may read a field's provenance, `seen_at(f)` and `known_at(f)`
(010). It is judged on the record its write leaves, and that record
carries the write's own provenance, as the fold will keep it. So the
store puts everything an event says of its origin on the event before it
computes the record the event makes, at every door: the source, the
instant the source states and where the source read the value, for an
ingest; the proposer as the writer and the acceptance as the cause, for
an accepted proposal; the call as the cause and the verb as the
procedure, for a verb's write.

A guard is a level, not an edge: `seen_at(status) >= '2026-03-01'` fires
on every write that leaves a record whose `status` its source saw then.
That is the ingest that brings the value, a record later in the same
window that brings another field of that key, and an authored write of
another field after it, since `status` keeps its `seen_at`.

**Why.** A guard that read less than the fold keeps would give one
expression two answers, one at the write and one at every read after it
(023).

### A guard tells a retirement from an update

A retirement keeps the record's fields as they were and only retires it.
A guard over fields alone therefore holds on a retirement exactly when
it held before it, and `changed` and `previous` cannot tell the two
apart. A guard reads it through `retired()`: true when the write retires
the record (live before it, retired after it), false on any other write.
`status = 'closed' and not retired()` fires on closing a record and not
on retiring a closed one; `retired()` alone fires on retirements only.
It holds on every door that retires: `retire`, an accepted retire and a
source's purge.

A trigger without `retired()` fires on retirements too. Leaving
retirements out by default was the other way to do this. It would have
taken away firings this record promises, from triggers whose
declarations did not change. A firing is a durable fact a host may
already depend on.

`retired()` belongs to a trigger's guard alone, since only a guard reads
a write. In a derived field, a filter, a verb's `requires` or an
object set it is refused as invalid. A revival, an upsert of a retired
record, is a creation to a guard: `previous` is null, `changed` is true
and `retired()` is false. The function came with a step of the log
format (`docs/log-format.md` §6), so a release that does not know it
refuses the lake instead of keeping a trigger it cannot evaluate, which
would never fire.

### A set's declaration decides the records already there

A set's declaration decides the records already there. In the
declaration's own append, after its `declared` (and the link types a
type brings), the store appends an `object_set_changed` for each record
of the set's type whose membership the declaration changes. Each is
caused by the `declared`, written by the system and carries the
declaration's context. The filter is evaluated as a write evaluates it,
with the time of the declaration as `now`, and a retired record is never
a member. A write and a declaration decide by one rule.

- **A set declared again unchanged** appends nothing.
- **A set is of one type.** Declared again for another type it is
  refused: it would let go of every member without a word, and a set of
  another type is another set.
- **The same in every way in:** `declare`, `apply` and an accepted
  declaration proposal.
- **The declaration's answer names each set once,** however many
  records it moves, and its `through` is the last change (015,
  extended).

The event is the one a write appends, and the fold applies it as it
applies that one. Only its cause differs: an `object_set_changed` may
name a `declared`, not only a record's write.

**One append, bounded by the lake's `max_set_changes` (20 000 unless the
location sets it).** A declaration that would change more is refused
`too_large`, naming the set and the ceiling, and appends nothing:
- windows of their own after the `declared` would leave the index and
  the events wrong after a crash;
- a new event carrying the keys would be a second shape every consumer
  must learn.

What it costs, and how a host declares a set over a large type, is in
`docs/seams.md`, "Declaring a set over a large type".

**Why.** Before, a set's membership was decided only when a record was
written. A set declared over records that already hold its filter, or
declared again with another filter, left them as they were. A consumer
of the feed never learned of those memberships, and the lake's
membership disagreed with the filter until each record was written
again.

### A type's declaration decides the sets of what it computes again

A type declared again with other computed fields, a derived expression
or a function they name, or a function declared again, has its records
computed again by the fold. The sets whose filter reads such a field,
and so a set that names one, are decided in the declaration's own
append (lakelands#941): one walk of the type's live records, each
computed as the bundle leaves the lake, appends an `object_set_changed`
for each membership that moves, caused by the last `declared` of the
bundle, within the same `max_set_changes`. Past it the declaration is
refused `too_large`, naming the set. A set the bundle declares, or
declares again, of such a type is decided in that walk too, by its new
filter over the records as the bundle computes them. A migration that
changes what its type computes does the same (lakelands#1070): the fold
computes its records again, so a record reads the new value as soon as
the migration is applied, and the migration's own append decides the
sets over the records as converted and computed again. Before, the leaves landed at each
record's next write, caused by a write that did not move them, and the
feed and the filter disagreed until then.

A field computed outside the lake, by a backing, comes out of that walk
as it was: its value lands with its output, and its sets are decided
where the output lands, as before.

### A set names a set

`in_set('name')` names a set in a condition. It holds for a record where
the named set's filter holds for it: the filter stands in the name's
place when the condition is read. It is not a lookup of stored
membership, so a set stays its filter, and union, intersection and
difference are `or`, `and` and `and not`. A set's `where`, a trigger's
`when` and a read's `where` may name a set; anywhere else the call is
refused.

- **A change reaches the sets built on it.** A set declared again decides
  the sets that name it, and those that name them, in the same append,
  each change caused by that `declared`. The lake's `max_set_changes`
  counts over all of them; past it the declaration is refused
  `too_large`, naming the set whose change passed it, and nothing is
  appended.
- **Refused at the declaration:** a set that is not declared, a set of
  another type than the record's, and a set that names itself through
  others, naming them.
- **A read at an earlier position** takes each named set's filter as it
  was in force there.

**Why.** Before, a set built from two others repeated their filters,
and drifted when one of them was declared again: nothing tied the copy
to what it copied.

### A migration decides again every record whose fields it changes

A migration changes the records of its type: a field converted to
another kind or scale, a key moved. It changes more records than those
of its type. A re-key rewrites every reference to a moved key, in the
migrated type and in any other, and it moves the links of the type's
relations, which are keyed by their ends. A migration that changes a
relation's fields converts the relation's links. The rule is one: **a
migration decides again every record whose fields it changes, as the
fold leaves it**: converted, its references to moved keys rewritten,
under its key after the migration. It decides each by the rule a write
uses.
- **The migrated type's records** are decided after the moves are known,
  so a reference from the type to itself is read as rewritten. The
  decision is made in the walk the preflight makes over the type's
  records anyway, so the migration reads none of them twice.
- **A record of another type that holds a reference to a moved key** is
  decided for the sets whose filter reads that reference, and for no
  other: nothing else of it changes. The type is read again, a page at a
  time, only when it has such a set.
- **A link of a relation whose fields the migration converts** is decided
  for every set of the link type, in the read the preflight already makes
  of the links.

In its own append, directly after its `migrated`, the migration appends
an `object_set_changed` for each record whose membership changes, caused
by the `migrated`, written by the system, and naming the record's own
type and its key after the migration. The changes stand before the
events of the link types the migration changes or adds, as a write's
reactions directly follow its event. The fold of `object_set_changed`
writes one membership and reads neither a record nor a declaration, so
the index is the same wherever the event stands in the append. The
migration's answer reads what directly follows its `migrated`, so it
names each set joined or left once and `through` the last change,
whatever else the append carries.

All of a migration's changes count against the one `max_set_changes`,
the same as a set's declaration; past it the migration is refused
`too_large`, naming a set and the ceiling, and appends nothing. A
migration that changes nothing a filter reads appends no
`object_set_changed`.

**A set whose `where` reads across an edge** is decided again at every
write of the far record that moves what it reads there, in that write's
own append and caused by its event, and so is a trigger whose `when`
reads so (`docs/decisions/055-a-path-steps-through-a-reference.md`):
a write of one record can move many memberships. What far writes set
off across an edge in one append counts against the same
`max_set_changes`, at every door that writes a record; past it the write
is refused `too_large` naming the set or the trigger, to a caller shown
it and naming nothing to another, a purge whole, and an aging move is a
failed guard. The ceiling counts the append, records held from the
writer among them: a refusal for size tells that much and no more. A migration, a re-key, a value type's
rename and a `declared` of a far type decide the referring sets in their
own walk, under the same ceiling.

The ceiling is the last of a migration's refusals. One applied with a
stale report is refused for that, and one that is not lossless is refused
with its report, whether or not it is also past the ceiling: the walk
stops deciding once the changes pass it, and the refusal waits until the
report has been compared and found lossless.

**A move alone owes no event.** A re-keyed record, a link among them, is
in every set under its new key that it was in under its old one, and no
filter's truth has changed. The `migrated` event lists each move in
`rekeyed`, for the link types too, and that is what tells a feed
consumer.

The fold carries that membership to the new key, and it does so the same
whether it folds one append or many: a rebuild, an import and the
catch-up of a lake opened behind its log fold a page of appends in one
transaction, and a membership put earlier in the page is read from the
page, as a record put earlier in it is (lakelands#492).

**A filter's text is not rewritten.** A set declared as `boss = 'X'`
names a key in a literal, and a literal is not known to be a key. After
a re-key of `X` the filter holds for no record, and the members leave,
which the events say. A host that means "under this person" declares the
set again with the new key; that declaration decides the records
(lakelands#453).

**Why.** Before, the membership of a migrated type's records in the
type's object sets was carried as it stood, so a set whose filter reads
a converted field kept members its filter no longer holds for.

### A consumer waits on the feed

A consumer that has caught up waits for the next append with
`wait(after, within)`, instead of sleeping and polling again. The wait
answers the head once the log is past `after`, or once `within` has
passed. It never answers past the head a poll reads, so a poll after it
answers what woke it. The lake knows the moment it appends, and every
consumer that slept found that moment again at its own pace, a quarter
of a second late by default and four times a second while the lake was
idle. Positions and acknowledgements stay the consumer's; only the
waiting moved into the lake (lakelands#1061).

## Why

A feed kept beside the log can disagree with it. If it lives in the
projection it is lost on a rebuild; if a replay regenerates it, a
trigger fires twice; if it is an in-memory hand-off, a process that dies
between commit and publish loses a dispatch without anyone noticing.
When the feed is the log, none of these can happen: a dispatch is
durable the moment the write is, a replay reads that a trigger fired
instead of firing it again, and an unconsumed change is visible as a
position that lags the log.

Keeping consumer state out of the store keeps the store's side simple
and the contract narrow: any number of consumers can read, each at its
own pace, and adding one changes nothing in the lake.

## What it is not

- It is not a message bus. Lakelands never publishes to a broker; a
  relay onto one is the host's code, reading the feed by position.
- It is not delivery. Detecting that something is due is the store's
  job; acting on it (a webhook, a worker, a notification) is behind a
  seam.
- A buffer that groups writes before they are committed is a write
  coalescer, not this feed, and nothing reads it as one.

## How it came to be

- The first version: the feed is the log filtered by kind, read from a
  place the consumer keeps, which it called a cursor.
- The contract named a feed reader's place a position, and left `cursor`
  to the page token of a list-shaped read.
- When a trigger fires was stated: on every write of its type whose
  guard holds, in the write's append.
- A consumer's receipts are the host's, with the webhook deliverer.
- What a guard reads of a write was stated, and a trigger may cap the
  depth of the reactions to it (lakelands#244).
- A guard tells a retirement from an update through `retired()`
  (lakelands#271).
- A set's declaration decides the records already there, where a set's
  membership was decided only when a record was written; the log format
  did not change. A set declared again over another type let go of the
  old type's members (lakelands#453).
- A migration decides the membership of its type's records again, where
  it carried it as it stood, and its ceiling is the last of its refusals
  (lakelands#457).
- A type declared again with other computed fields decides the sets of
  what it computes again in its own append, where the leaves landed at
  each record's next write (lakelands#941).
- A migration that changes what its type computes computes its records
  again, where they kept the old values until each was written again, and
  decides their sets in its own append (lakelands#1070).
- A migration decides every record it changes: also a record of another
  type that holds a reference to a moved key, and a link of a relation
  whose fields it converts. Until then the decision concerned only what
  the converted fields of its type's records hold (lakelands#465,
  lakelands#466).
- The fold reads a membership put earlier in a page of appends from the
  page. Before, it asked the projection as it stood before the page, and
  a member whose join and re-key fell into one page was a member of
  nothing afterwards (lakelands#492).
- A set names a set, and a set declared again for another type is
  refused, where it let go of the old type's members (lakelands#552).
- A poll examines a bounded part of the log, where it read on to `limit`
  matches or to the head. It left a consumer that started at 0 with a
  narrow filter reading the log once, a bounded poll at a time
  (lakelands#558).
- The projection keeps an index of the log, which took that read away,
  and a projection is read only by its layout; `FEED_EXAMINED` bounds
  the events a poll reads and opens, where it bounded those it examined
  (lakelands#678).
- The sentence that said in which log format a declaration naming a
  set is written left, and the one on `retired` names a step of the
  format and no number: a lake is written in one format a release
  (lakelands#801).
- A guard reads the provenance its write leaves, at every door. Before,
  `seen_at(f)` read null on the ingest that brings the value, and a
  second record of one key in one window was judged without the
  provenance the first left (lakelands#850).
- Corrected at the rewrite (lakelands#817): "a filter that matches
  nothing more is caught up in one poll" was said of every filter; the
  engines decide one that names types and kinds and no key 65 536
  positions a poll.
- A consumer may wait on the feed: `wait(after, within)` answers the
  head once the log is past `after`, never past what a poll reads, and
  the store's own consumers wait so instead of sleeping. Positions and
  acknowledgements stay the consumer's (lakelands#1061).
