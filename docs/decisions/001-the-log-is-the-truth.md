# 001 — The append-only log is the truth

## What is decided

Every lake has one append-only event log, and the log is the only
authoritative state. Every change to a lake is an event appended to it;
nothing is ever changed in place or removed from it.

The projection, the tables that answer queries, is an index derived from
the log. It can be deleted at any time and rebuilt by replaying the log,
and the rebuild produces the same bytes as the projection it replaced.
A projection that differs from its replay is a bug in the fold or the
engine, not a second opinion about the data.

## Why

A store for agents has to answer "what did the lake hold when this
decision was made" and "who wrote this, from where". Both questions have
an exact answer only if history is kept as history, not overwritten by
the current state. With the log as the truth, every earlier state is a
replay up to a position, and every value can be traced to the event that
wrote it.

It also keeps the projection cheap to change. A new index, a new engine
or a corrected fold is a rebuild, not a data migration; the only format
a lake is bound to is the log's, and that format is specified and
versioned. What a release owes a lake written in an older format is in
`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`.

## What it is not

- It is not a promise that the projection is a cache that may drift. The
  projection is exact; it is only disposable.
- It is not a free-form journal. Events have a specified envelope and a
  closed set of kinds, and each event is chained to the one before it,
  so a gap or an edit in the log is detectable.
- It is not an audit trail kept beside mutable tables. There is no
  second copy of the state that could disagree with the log: retiring a
  record is an event like any other write, and the projection shows
  what the fold makes of it.
