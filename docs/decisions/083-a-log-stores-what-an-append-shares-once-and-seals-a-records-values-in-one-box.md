# 083 — A log stores what an append shares once, and seals a record's values in one box

## What is decided

**Proposed** on lakelands#1413 (2026-10-25), for the operator to accept,
built in 0.20.0 in format 20. It amends
`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`,
whose log now stores the form an event's canonical bytes are restored
from, `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`,
whose step to 20 reads a lake of 19 without a rewrite, and
`docs/decisions/065-part-of-a-source-may-be-taken-back.md`, whose fold
no longer keeps the keys a delivery was sealed apart with. Each
amendment comes in force with the issue that builds it; until then the
three stand as written. `docs/log-format.md` §1.2, §1.4, §3, §4 and §6
specify it.

### A segment's lines are the log's stored form, and each restores to exactly one canonical event

The log stores a **stored form**, not each event's canonical bytes. A
reader restores every event from it, and the event it restores is the
one the store made: the same members, the same canonical bytes, the same
`hash`, and the same `prev`. The hash and the chain are over that event
as 020 says; nothing of them changes. A segment's digest is over its
file's lines as they stand, so it covers the stored form, frames
included.

### An append of two or more events shares one frame, and `position` and `prev` stand only on a segment's first line

- **A frame.** An append of two or more events is stored as one frame
  line followed by its events' lines. The frame holds exactly the
  members its events all share with equal values: the top-level ones but
  `v`, `id`, `hash`, `position` and `prev`, and those one level into
  `body` and into `sealed`. Each event's line leaves out what its frame
  holds. An append of one event has no frame.
- **`v` on every line.** Every line holds its format, a frame line too,
  so a release that does not read the format finds its number on the
  first line it reads and refuses the lake by it, as 054 says, and
  never takes a line without `v` for one of format 0.
- **Derived members.** An event's `position` is the one before it plus
  one, and its `prev` is the hash of the event before it. A line holds
  them only where it is its segment's first event, so a segment
  verifies from its own first line.
- **One stored form.** The writer's rule is a function of the append's
  events alone, so two engines given the same appends store the same
  bytes and the same digests. Any other stored form is a break, as a
  line that is not canonical is: a frame that does not hold exactly the
  members its lines share, a line that holds a member its frame holds,
  or `position` or `prev` where they are derived.
- **The store frames.** The store canonicalises and frames; the engine
  keeps the stored form unchanged and hands it back unchanged, and the
  store restores and verifies it. The suite checks it through the seam,
  on every engine.

A reader cannot see where an append ended, so it checks each frame
against the lines it covers. A line of a format before 20 is a full
event, taken as it stands.

### A record event's sealed members are sealed in one box

From format 20, the members a source's key seals in a `record_ingested`
are sealed together: the canonical bytes of the object of those
members, under one nonce, into `sealed.box`. The patch leaves them out,
and `sealed.fields` still names them, so the log tells which members
are sealed without the catalog. The nonce is made from the source, the
type, the record key and the version before the event: there is one box
a (source, type, key, version), so two never share a nonce. The key is
the one each member was sealed with before, composed with any key the
delivery was sealed apart with. The ingest id covers the box (§3 of the
log format, `lakelands:ingest:v2`), and the same values against the same
state seal to the same box, so a redelivery is still a duplicate.

### The projection keeps no copy of what the log holds at the value's own position

Where a member is read only by a rare read, at a value's position, and
the log holds it in the event at that position, the projection keeps no
copy of it per event. The keys a value was sealed apart with are such a
member: an output reads them from the `sealed.apart` of the value's own
event, and only where the fold's list of the source's versions marks the
version as one a delivery sealed apart sealed under.

## Why

A lake of readings is mostly envelope. Measured on 10 000 records of
one source, written as its host writes them (32 records an append, ten
erasable fields, each delivery sealed apart) into a SQLite lake of
format 19, in bytes a record:

| | log | projection | index | total |
|---|---:|---:|---:|---:|
| before (format 19) | 1 429.2 | 1 172.7 | 195.4 | 2 797.3 |
| after | 673.6 | 773.3 | 129.0 | 1 575.9 |

The projection is compared after `VACUUM` in both rows; as written it
held 1 212.0 B a record before. Of the log's 755.6 B, the derived
`position` and `prev` take 89.9, the frame's top-level members 225.4,
`sealed` framed 203.0, `body` framed 63.7, and the box 173.6; keeping
`v` on every line gives 7 B of it back, which the table does not count.
Of the
projection's, the copies of the apart keys take 212.2; the rest, and
the index's 66.4, is the engines' own layout of the routes, of a
record's provenance and of the index, which no format fixes.

- **The truth stays the same truth.** The hash is over the restored
  event, so a frame changes no hash and no id, and an edit of a frame
  breaks the hash of every event it restores. A verifier still needs
  only this format and its document.
- **One form keeps the bytes reproducible.** Had the writer been free
  to frame what it liked, two engines would store the same events in two
  byte strings, and a digest a checkpoint names would hold for one of
  them only. The frame of an append is decided by its events, so it is
  not a choice.
- **A box costs one tag a record, not one a field.** Each sealed member
  carried its own 16-byte tag and its own padding. Erasure loses nothing
  by it: every sealed member of one event was already under one key, and
  erasing destroys keys, so the members `sealed.fields` names are erased
  together as before.
- **A copy is kept only where it pays.** The apart keys were copied
  into a row for every record so that a rare read would not need the
  log; that read is one event a value, made only at a marked version.

## What it is not

- **Not a rewrite of any lake.** A lake of 19 keeps its lines, its
  bytes, its hashes and its digests; a segment may hold lines of both
  forms, and a value of 19 sealed per member opens by its event's `v`.
- **Not `sealed.fields` derived from the catalog.** A reader without the
  catalog could no longer tell ciphertext from text, and erasing a
  patch that carries some erasable fields but not others would need it.
- **Not a frame shared across appends.** An append of one event gets
  only the derived `position` and `prev`. A frame kept lake-wide would
  be a rule of its own, and the host this was measured for appends 32
  records a call.
- **Not a hashed key in the routes.** A route by a hash of the key would
  save 49 B more, and every reader of it would have to check each answer
  again; the route stays exact. Dropping the routes would save 197 B and
  leave the feed without an index.
- **Not a change of what erasure takes.** It is still per key, and the
  members of one event still share one.
- **Not a compression of the log.** Every line is still canonical JSON a
  reader parses without a codec.
