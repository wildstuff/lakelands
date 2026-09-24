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
