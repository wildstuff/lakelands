# 037 — The feed over a network door

## What is decided

The change feed served over a network door (REST's `GET …/feed`) is
the log as it is, unmasked, and only a principal that reads every field
of every record — withheld records included — may open it. A principal
whose grants mask or hide any field, on any type, or who lacks the
`restricted` grant, is refused as `not_granted` before the stream starts.

The rule is the contract's, not a transport's: `Grants::reads_everything`
in `lakelands-api` states it, with its test, and every door that serves
the feed asks it.

The feed is not filtered per principal: it carries each event as the log
holds it, so its bytes are the log's bytes and a consumer can verify
them against the hash chain.

## Why

Every other door applies a principal's grants to what it answers: a
masked field is masked in `find` and `get`, a withheld record is absent
from both (`docs/decisions/006-grants-not-roles.md`). The feed is the
log (`docs/decisions/004-the-change-feed-is-the-log.md`), and an event in
the log carries the values it wrote. Serving it to a principal that may
not read a value would open a way around every grant at once.

Filtering events per principal would make the feed something other than
the log: an event with a value removed no longer matches its hash, a
relay could not tell a masked event from a corrupted one, and two
consumers of one lake would see two different logs. The feed's readers
are relays, workers and deliverers acting for the lake as a whole; a
caller that may see only part of the lake reads it through the doors
that apply its grants, or through notifications that do.

## What it is not

- It is not a new grant. It asks nothing a principal does not already
  hold; the owner's grants pass, and so does any principal granted read
  on everything with `restricted`.
- It does not change the host's own feed. A host holding the store reads
  `LakeStore::feed()` in-process, as the CLI's `lakelands feed` does; the
  host is the lake's operator, not a caller at a door.
- It is not the last word on narrower feeds. A per-principal view of the
  changes is an MCP notification (lakelands#70), which applies grants
  the way a read does, not the feed.
