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
