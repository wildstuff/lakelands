# 004 — The change feed is the log

## What is decided

The change feed is the event log filtered by kind. There is no separate
feed table, queue or outbox. What a consumer needs to react to is
appended to the log as events in the same append as the write that
caused it: a record changed, a trigger fired (`TriggerFired`), an object
set gained or lost a member (`ObjectSetChanged`).

A consumer reads the feed from a cursor, and a cursor is a log position.
The consumer keeps its cursor and acknowledges on its own side; the
store keeps no consumer state. The dedup key of a delivered change is
its lake and its position.

## Why

A feed kept beside the log can disagree with it. If it lives in the
projection it is lost on a rebuild; if a replay regenerates it, a
trigger fires twice; if it is an in-memory hand-off, a process that dies
between commit and publish loses a dispatch without anyone noticing.
When the feed is the log, none of these can happen: a dispatch is
durable the moment the write is, a replay reads that a trigger fired
instead of firing it again, and an unconsumed change is visible as a
cursor that lags the log.

Keeping consumer state out of the store keeps the store's side simple
and the contract narrow: any number of consumers can read, each at its
own pace, and adding one changes nothing in the lake.

## What it is not

- It is not a message bus. Lakelands never publishes to a broker; a
  relay onto one is the host's code, reading the feed by cursor.
- It is not delivery. Detecting that something is due is the store's
  job; acting on it (a webhook, a worker, a notification) is behind a
  seam.
- A buffer that groups writes before they are committed is a write
  coalescer, not this feed, and nothing reads it as one.

## Extended: the words for a reader's place

The contract names a feed reader's place a **position**, never a cursor:
the feed is polled `from` a position and answers the position to poll
from next, and a consumer acknowledges `through` a position. In
Lakelands `cursor` means only the page token of a list-shaped read
(`docs/glossary.md`), and a source's own place in its data is its
**offset**. Where this record says "cursor" above, it means that
position; nothing else about the decision changes.

## Extended: when a trigger fires

A trigger fires on every write of its type — `upsert`, `ingest`, a verb's
write, an accepted proposal's write, `retire` — whose guard holds over the
record after the write. The guard sees the record before the write through
`changed` and `previous`, so a transition is the guard's to state; a guard
that cannot be evaluated does not fire. The `trigger_fired` event is
appended in the same append as the write, naming it as its cause, and an
object set's `object_set_changed` is appended the same way, only when a
record's membership actually changes.

## Extended: a consumer's receipts are the host's

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

## Extended: what a trigger's guard sees, and how deep its reactions go

A trigger's guard reads the write: the record's new values, `changed(f)`,
and `previous(f)`, the value before the write, which is null for a record
the write creates (or revives from retirement). A transition is a guard,
`previous(status) = 'open' and status = 'closed'`, and fires on that
transition only. A guard reads no other record. A trigger may also cap
the depth of the reactions that name its firing as their cause
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`, extended).
The firing is appended whatever the depth, since it is the durable fact a
host reacts to.

## Extended: a guard tells a retirement from an update (lakelands#271)

A retirement keeps the record's fields as they were and only retires it.
A guard over fields alone therefore holds on a retirement exactly when
it held before it, and `changed` and `previous` cannot tell the two
apart. A guard reads it through `retired()`: true when the write retires
the record (live before it, retired after it), false on any other write.
`status = 'closed' and not retired()` fires on closing a record and not
on retiring a closed one; `retired()` alone fires on retirements only.
It holds on every door that retires: `retire`, an accepted retire and a
source's purge.

A trigger without `retired()` fires as before, on retirements too.
Leaving retirements out by default was the other way to do this. It
would have taken away firings this record promises, from triggers whose
declarations did not change. A firing is a durable fact a host may
already depend on.

`retired()` belongs to a trigger's guard alone, since only a guard reads
a write. In a derived field, a filter, a verb's `requires` or an
object set it is refused as invalid. A revival, an upsert of a retired
record, is a creation to a guard: `previous` is null, `changed` is true
and `retired()` is false. A declaration calling `retired` is written in
log format 7 (`docs/log-format.md` §6), so a reader that does not know
the function refuses the log instead of keeping a trigger it cannot
evaluate, which would never fire.

## Extended: a set's declaration decides the records already there (lakelands#453)

A set's membership was decided only when a record was written. A set
declared over records that already hold its filter, or declared again
with another filter, left them as they were. A consumer of the feed never
learned of those memberships, and the lake's membership disagreed with
the filter until each record was written again.

Now a set's declaration decides them. In the declaration's own append,
after its `declared` (and the link types a type brings), the store
appends an `object_set_changed` for each record of the set's type whose
membership the declaration changes. Each is caused by the `declared`,
written by the system and carries the declaration's context. The filter
is evaluated as a write evaluates it, with the time of the declaration
as `now`, and a retired record is never a member. A write and a
declaration decide by one rule.

- **A set declared again unchanged** appends nothing.
- **One declared again over another type** lets go of the old type's
  members.
- **The same in every way in:** `declare`, `apply` and an accepted
  declaration proposal.
- **The declaration's answer names each set once,** however many
  records it moves, and its `through` is the last change (015,
  extended).

The event is the existing one, and the fold already applies it. Only its
cause is new: an `object_set_changed` may now name a `declared`, not
only a record's write. No log format changes.

**One append, bounded by the lake's `max_set_changes` (20 000 unless the
location sets it).** A declaration that would change more is refused
`too_large`, naming the set and the ceiling, and appends nothing:
- windows of their own after the `declared` would leave the index and
  the events wrong after a crash;
- a new event carrying the keys would be a second shape every consumer
  must learn.

What it costs, and how a host declares a set over a large type, is in
`docs/seams.md`, "Declaring a set over a large type".

## Extended: a migration decides the membership again (lakelands#457)

A migration changes the records of its type: a field converted to
another kind or scale, a key moved. Their membership in the type's object
sets was carried as it stood, so a set whose filter reads a converted
field kept members its filter no longer holds for. Now the migration
decides each record again, as migrated, by the rule a write uses. In its
own append, directly after its `migrated`, it appends an
`object_set_changed` for each record whose membership changes, caused by
the `migrated`, written by the system, and naming the record's key after
the migration. A re-keyed record's membership moves with it, as it did
before; the decision concerns only what the converted fields hold.

The changes stand before the events of the link types the migration
changes or adds, as a write's reactions directly follow its event. The
migration's answer reads what directly follows its `migrated`, so it
names each set joined or left once and `through` the last change,
whatever else the append carries.

The decision is made in the walk the preflight makes over the type's
records anyway, so a migration reads no record twice. Its changes count
against the same `max_set_changes` as a set's declaration, over all sets
of the type; past it the migration is refused `too_large`, naming a set
and the ceiling, and appends nothing. A migration that changes nothing a
filter reads appends no `object_set_changed`.

The ceiling is the last of a migration's refusals. One applied with a
stale report is refused for that, and one that is not lossless is refused
with its report, whether or not it is also past the ceiling: the walk
stops deciding once the changes pass it, and the refusal waits until the
report has been compared and found lossless.

## Extended: a migration decides every record it changes (lakelands#465, lakelands#466)

A migration changes more records than those of its type. A re-key
rewrites every reference to a moved key, in the migrated type and in any
other, and it moves the links of the type's relations, which are keyed
by their ends. A migration that changes a relation's fields converts the
relation's links. The rule is one: **a migration decides again every
record whose fields it changes, as the fold leaves it**: converted, its
references to moved keys rewritten, under its key after the migration.
- **The migrated type's records** are decided after the moves are known,
  so a reference from the type to itself is read as rewritten.
- **A record of another type that holds a reference to a moved key** is
  decided for the sets whose filter reads that reference, and for no
  other: nothing else of it changes. The type is read again, a page at a
  time, only when it has such a set.
- **A link of a relation whose fields the migration converts** is decided
  for every set of the link type, in the read the preflight already makes
  of the links.

Each change is an `object_set_changed` naming the record's own type and
its key after the migration, directly after the migration's `migrated`
and caused by it, before the link types' own events. The fold of
`object_set_changed` writes one membership and reads neither a record nor
a declaration, so the index is the same wherever the event stands in the
append. All of a migration's changes count against the one
`max_set_changes`.

**A move alone owes no event.** A re-keyed record, a link among them, is
in every set under its new key that it was in under its old one, and no
filter's truth has changed. The `migrated` event lists each move in
`rekeyed`, for the link types too, and that is what tells a feed
consumer.

The fold carries that membership to the new key, and it does so the same
whether it folds one append or many: a rebuild, an import and the
catch-up of a lake opened behind its log fold a page of appends in one
transaction, and a membership put earlier in the page is read from the
page, as a record put earlier in it is (lakelands#492). Before, the fold
asked the projection as it stood before the page, and a member whose
join and re-key fell into one page was a member of nothing afterwards.

**A filter's text is not rewritten.** A set declared as `boss = 'X'`
names a key in a literal, and a literal is not known to be a key. After
a re-key of `X` the filter holds for no record, and the members leave,
which the events say. A host that means "under this person" declares the
set again with the new key; that declaration decides the records
(lakelands#453).
