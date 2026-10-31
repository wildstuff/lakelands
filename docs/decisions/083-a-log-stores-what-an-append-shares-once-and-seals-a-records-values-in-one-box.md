# 083 — A log stores what an append shares once, and seals a record's values in one box

## What is decided

**Accepted** on lakelands#1413 (2026-10-26, by the operator), for
0.20.0, in format 20. It amends
`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`,
whose log now stores the form an event's canonical bytes are restored
from and digests a segment over those bytes;
`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md` and
`docs/decisions/038-a-lake-in-postgresql.md`, whose engines keep that
form; `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`,
whose step to 20 reads a lake of 19 without a rewrite;
`docs/decisions/065-part-of-a-source-may-be-taken-back.md`, whose fold
no longer keeps the keys a delivery was sealed apart with; and
`docs/decisions/077-a-backup-is-an-export-that-grows.md`, whose backup
writes a form of its own and continues after its last whole frame. Each
amended rule is marked where it stands as accepted for 0.20.0.
`docs/log-format.md` §1.2, §1.4, §3, §4 and §6 specify it.

### A segment's lines are a stored form, and each restores to exactly one canonical event

The log stores a **stored form**, not each event's canonical bytes. A
reader restores every event from it, and the event it restores is the
one the store made: the same members, the same canonical bytes, the same
`hash`, and the same `prev`. The hash and the chain are over that event
as 020 says; nothing of them changes.

- **A frame** is a line holding members that the n events after it all
  hold with equal values: top-level ones but `v`, `id`, `hash`,
  `position`, `prev`, `body` and `sealed`, and ones one level into
  `body` and into `sealed`. Each event's line leaves out what its frame
  holds.
- **Derived members.** An event line may leave out its `position`, the
  one before it plus one, and its `prev`, the hash of the event before
  it; a segment's first event line holds both, so a segment verifies
  from its own first line.
- **`v` on every line.** Every line holds its format, a frame line too,
  so a release that does not read the format finds its number on the
  first line it reads and refuses the lake by it, as 054 says, and
  never takes a line without `v` for one of format 0.
- **A break** is a line that is not canonical, a frame whose `v` is not
  its events' or whose lines do not all follow it in its segment, a line
  that holds a member its frame holds, a `position` or `prev` that is
  not the derived one, or a segment's first event line without both. A
  line of a format before 20 is a full event, taken as it stands.

### A segment's digest is over its restored events, so every copy writes a form of its own

A segment's digest is SHA-256 over the canonical bytes of its events as
restored, each followed by a line feed. Where every line is a full
event, as up to format 19, that is the file's bytes, so no digest a
checkpoint already lists moves. The digest is then the same whatever
stored form holds the events, and a form is judged only by whether it
restores them: a reader cannot see where an append ended, and need not.

- **The store's writer** frames each append of two or more events, with
  exactly the members they all share, and writes `position` and `prev`
  only on a segment's first event line. An append of one event has no
  frame.
- **An export and a backup** read the log through the feed, on any
  engine and through a feed in front of the store. They frame each append
  of two or more events whole, where the feed says it ends (below), and
  every event line they write holds its `position`, so a backup run
  restores its last event from that line, its frame and the lines back
  to that frame alone. Until lakelands#1540 they framed each run of two
  or more, and at most 1 000, consecutive events of one segment that
  shared `known_at`; such a copy is still read alike.
- **An import** restores the events and appends each frame of the copy
  as one append, and each event no frame covers as one of its own, as
  any writer appends them.
- **The store frames, the engine keeps.** The store canonicalises and
  frames; the engine keeps the stored form unchanged and hands it back
  unchanged, and the store restores and verifies it. The suite checks
  through the seam, on every engine, that the stored form comes back as
  it went in and that a digest is over the restored bytes.

### The feed says where each append ends, read from the frames the log holds

Extended on lakelands#1540, for 0.22.0, proposed. A consumer that acts on
a whole write, the firings of one ingest or the events of one call, has
to know where that write ends; `known_at` and a delivery's name do not
tell it, since the clock may read the same for two appends and a caller
may name two ingests alike.

- **`FeedBatch.appends`** lists the last position of each append that
  wrote one of the batch's events, in order, and `append_of` answers an
  event's. It is read from the stored form: where a frame covers an
  event, its append ends at the frame's last event; where none does, at
  the event itself. The writer frames every append of two or more
  events, so that is the append. The format does not change and nothing
  is stored twice.
- **The end, not the start, on the batch, not on the event.** An append's
  first position on each event would tell a consumer which events go
  together, but not when it holds all of them: it learns that only from
  an event of the next append, which may come much later. The end says
  it at once, once `next` passes it, and holds alike when a filter
  admits only some of the append's events and when a page cuts through
  it. On the batch it is no member of the hashed event, so no event's
  bytes change.
- **Every door carries it**: the store's `poll`, the REST feed as a
  `: append <position>` comment line before each event, the MCP watch
  notification as `append`, and the deliverer as the `Lakelands-Append`
  header. The feed and the deliverer are read by one who reads the
  whole lake or by the operator's own receiver. A watch session may be
  told only some of an append, so its `append` is the last position of
  the append it is told, and the session is sent the append's changes
  once the pump has read past its end. An event it is not told of, such
  as a firing whose condition reads a field masked from it (024), moves
  no mark: told the end, it would learn that the write it was told of
  set off something it is not told. A receiver that sees no `next`
  holds an append whole once the event at its end arrives, or one of a
  later append; the deliverer names as the end the last event of the
  append it sends, since it does not send them all.
- **Where it is not exact.** An append of a format before 20, or of 20
  before lakelands#1433, has no frame, and each of its events reads as
  an append of its own; so does each event of the append that creates
  or upgrades a lake, which is stored whole. A lake imported before
  lakelands#1540 reads each import batch of up to 512 KiB as one append.
- **A copy keeps the marks.** The export frames by the feed's appends
  and the import appends each frame as one, so the imported lake's feed
  tells the appends the lake it came from told, and exports the same
  files again. A copy made before lakelands#1540 framed runs that share
  `known_at`; imported, each of those runs is one append. An export
  holds one append at a time, and an import hands one append whole to
  the engine, however large: what a copy holds follows the lake's
  largest append, as its writer held it once, and no longer the 1 000
  events a frame was capped at.
- **A mirror does not.** It ingests what it reads into a lake of its
  own, in windows of its own, and each record names the lake and
  position it came from in `upstream`; its appends are its own writes,
  and their ends are its own.

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
| after | 673.6 | 773.3 | 131.2 | 1 578.1 |

The projection is compared after `VACUUM` in both rows; as written it
held 1 212.0 B a record before. The index is as 0.20.0 writes it, each
line one 0.19.0 parses (lakelands#1473): `,""` on every event line after
a segment's first, less the frame's own line, is 2.2 B a record more
than first measured, taken before and after that change on 10 000
records of one source, 32 an append (lakelands#1477). Of the log's
755.6 B, the derived `position` and `prev` take 89.9, the frame's
top-level members 225.4, `sealed` framed 203.0, `body` framed 63.7, and
the box 173.6; keeping `v` on every line costs 7 B of it, which the
table does not count. Of the projection's, the copies of the apart keys
take 212.2; the rest, and the index's 64.2, is the engines' own layout
of the routes, of a record's provenance and of the index, which no
format fixes.

- **The truth stays the same truth.** The hash is over the restored
  event, so a frame changes no hash and no id, and an edit of a frame
  breaks the hash of every event it restores. A verifier still needs
  only this format and its document.
- **A digest over the restored events lets a copy be made from what the
  feed answers.** A digest over the file's bytes would hold only for the
  form the writer chose, so a copy would have to rebuild that form.
  Carrying the frames through the feed would change the seam and every
  wire the feed crosses; reading the engine's lines for a copy would
  give up a backup through a feed in front of the store. The append ends
  the feed now names are one number per append, not the frame's
  members, and a copy frames by them in a form of its own. Over the restored events, the digest is what it
  was for every lake of 19, and any form that restores the events is the
  log.
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
- **Not one stored form.** Two forms of the same events restore the
  same bytes, hashes and digests, and a reader takes either; the
  writer's form is the store's rule for its own size, not a reader's
  check.
- **Not `sealed.fields` derived from the catalog.** A reader without the
  catalog could no longer tell ciphertext from text, and erasing a
  patch that carries some erasable fields but not others would need it.
- **Not a frame kept lake-wide.** The writer's append of one event gets
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
