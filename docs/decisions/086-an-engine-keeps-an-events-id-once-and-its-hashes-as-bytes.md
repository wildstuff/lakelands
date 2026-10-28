# 086 — An engine keeps an event's id once, and its hashes as bytes

## What is decided

Decided on lakelands#1517 and lakelands#1518 by the operator
(2026-10-28), for 0.21.0, with format 21 (lakelands#1502). It amends
`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`,
whose index line names `id` empty;
`docs/decisions/038-a-lake-in-postgresql.md`, whose log keeps `id`,
`prev` and `hash` as bytes and whose projection keeps no `applied`
table; and `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`,
whose first writer's open of 0.21.0 converts a PostgreSQL lake's log
columns. `docs/seams.md` restates `applied` and `clear`. Each amendment
comes in force with the issue that builds it; until then the three
stand as written.

### Nothing the log format publishes changes

No line, member, frame, hash, id, digest or checkpoint changes, and
neither does `docs/log-format.md`, its vectors (`verify.py` reads `log/`
alone) or a kept lake (`docs/log-format/lakes/` holds `log/` and
`export.json`, no index and no table). What changes is what each engine
keeps *beside* the stored form: the SQLite index, which 030 already
calls the engine's own and no part of the format, and the PostgreSQL
columns beside `bytes`. An export, a backup and an import write and read
the same bytes as before; an import writes its index, or its rows, in
the new shape.

### A SQLite index line names `id` empty, and the id is read from the event's own line

From 0.21.0 an event's index line is
`["e", position, fence, offset, len, "", prev, hash]`: `id` is named
empty, as `prev` is since format 20 after a segment's first line. The
engine hands the store an event's id from the event's line, which holds
`id` whole on every line (a frame never holds it). A line that names an
id is read as it stands, so an index of 20, or a segment holding lines
of both, reads unchanged. `hash` stays: 060's check of the last group
and the derived `prev` read it. The store's check that the event it
restores names the id the engine handed back (`disagrees`) stays, so an
id taken wrongly from a line is caught where it is read.

The slot is kept, named empty, rather than dropped: 0.20.0 parses an
event line as eight members and calls any other shape damage, and it
must read the index far enough to name format 21 (054, as
lakelands#1468 held for 0.19.0). The slot costs 3 bytes a line.

### PostgreSQL keeps `id`, `prev` and `hash` as their bytes

The `log` table keeps `id`, `prev` and `hash` as `bytea`, the bytes the
hex digits spell (16, 32 and 32), and hands them back as the same
lower-case hex. Every read of the log takes them in the type the
catalogue says, found once at a handle's open, so the ends of a lake not
yet converted, and a build ahead that reads one while the release before
still writes it (`docs/hosting.md`), are read as before.

### PostgreSQL answers an applied id from the log, and keeps no `applied` table

`ProjectionView::applied` and `applied_many` answer, on PostgreSQL, from
an index on `log (id)`: the least position holding the id, at or before
the view's position and no further than the projection had folded in
the same snapshot, as `matching` is bounded. That bound keeps the
answer the one every engine gives, after a `clear` too: none until the
refold reaches it. The index is not unique, so a log that holds an id
twice opens and answers its first position, as `applied` kept the first.
`mark_applied` still keeps the feed's routes; it no longer writes an id
on PostgreSQL. SQLite and the memory engine keep their `applied` map:
SQLite's ids are in its files, not its database, and the memory
engine's map is the cheapest index it has.

The seam's rule is restated so that it holds on every engine: the id
`mark_applied` names is the id of the event the log holds at that
position, and `applied` answers for events the log holds. The store
never marks another (its fold marks each event it folds, with its id).

### Both ride format 21, so the release before never reads them as its own

A SQLite index line with an empty `id`, read by 0.20.0 in a lake it
judged its own, would hand its store an event whose id disagrees with
its line: it would call a sound lake damaged. 0.21.0 writes only events
of 21, the upgrade's append included, so every line it writes is of an
event 0.20.0 refuses by its format; that release reads the ends and the
last group by `v` and by `hash`, not by `id`, and names 21.

On PostgreSQL, 0.20.0 reads `id`, `prev` and `hash` as text and fails on
bytes. It never gets there: the conversion is made only where the
tables of 0.21.0's schema are made already (085's `quarantined`), and
0.20.0 refuses a schema that holds a table it does not know before it
reads the log, as 054 says it refused 0.20.0's for 0.19.0.

### A lake gets there at its first writer's open of 0.21.0, and nothing is rebuilt

- **SQLite: nothing is converted.** Lines already written keep their
  `id`; a sealed segment's index never changes (030), and a lost one is
  not made again. Every line written from the upgrade on names `id`
  empty. The saving grows with what is written after the upgrade.
- **PostgreSQL: the log's columns are converted in place, once.** A
  writer's open, and a rebuild's, under the lake's lease and before any
  append, in one transaction: it makes the tables of 0.21.0's schema
  that are missing, converts each of the three columns still of type
  `text` (`decode(…, 'hex')`), makes the index on `log (id)` if
  missing, and drops `applied`. It is idempotent: it does only what the
  catalogue shows undone, and a lake converted is untouched. A crash or
  an error midway leaves nothing of it, the lake as 0.20.0 left it, and
  the next such open converts it; the error is the open's answer. The
  conversion rewrites the `log` table and its indexes: it needs free
  space about the size of that table until it commits, holds the table
  for its whole run, and takes time with the lake's events (measured at
  the build, and said in `docs/hosting.md` and the release notes).
  `applied` is named among the retired tables, so a lake that still
  holds it opens, and a rebuild's `clear` or swap drops it too.
- **No rebuild.** No table that stays changes, and the projection
  without `applied` answers what it answered with it, so the layout
  mark is not raised. A build ahead made by 0.21.0 is swapped in as
  before.
- **A process of 0.20.0 still open on a PostgreSQL lake** fails its next
  read of the log once the conversion commits. `docs/hosting.md`
  already asks every older process, readers too, to be stopped before
  the upgrade; it says now that this one cannot read on.

### A host sees nothing new

No method, type, setting, refusal, WIT or REST door changes, and no
answer: `applied` decides the same redeliveries, retries, agings and
recorded outputs. What a meter or `room` measures shrinks. The first
writer's open of 0.21.0 on PostgreSQL takes as long as the rewrite, which
`lakelands upgrade` runs at a moment the operator picks.

### What it takes to build

- **Product:** the SQLite index line and the id read from the line; the
  PostgreSQL columns, the reads in either type, the conversion, the
  index and the `applied` answer; `applied` among the retired tables. No
  `cfg` site and no seam method moves. The ledger row is measured on the
  PR's head.
- **Docs:** 030, 038 and 054 where they stand; `docs/seams.md`
  (`applied`, `applied_many`, `mark_applied`, `clear`);
  `docs/hosting.md` (the conversion's time and room on PostgreSQL); the
  module docs of both engines' `log.rs`.
- **Checks, on every engine:**
  1. An append's events come back with the id, `prev`, `hash` and fence
     handed in, through a handle opened after it: one event, a framed
     run, and a run across a seal.
  2. The `applied` rows append the events they mark, where they now
     mark ids no log holds
     (`declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position`,
     `the_first_applied_position_is_kept`, the second with two events of
     one id), and a new row: after a `clear`, an id answers none until
     the refold reaches its position.
  3. Unchanged and green: the redelivery and retry rows `docs/seams.md`
     names, and `upgrade::LAKE_20_READ` on every kept lake.
- **Checks, per engine:**
  - SQLite, `read_by_0_20_0.rs` beside `read_by_0_19_0.rs`: 0.20.0's
    `parse_line`, `whole` and ends, copied and fixed, judge a lake
    0.21.0 made, and one it upgraded, by format 21 and call neither
    damaged. Every line this build writes names `id` empty, and a
    segment of lines of 20 and of 21 reads back whole.
  - PostgreSQL, `older_layout.rs`: a lake as 0.20.0 left it (text
    columns, `applied`) has its ends read as 20 and is read by a build
    ahead; the first writer's open converts it, after which every event,
    every digest and the export are byte for byte as before; a
    conversion failed inside its transaction leaves the lake as 0.20.0
    left it and the next open converts it; a second open changes
    nothing.
- **Size:** measured, not tested, as 083 measured: 10 000 records of
  one source, 32 an append, plain, before (0.21.0 without this) and
  after, on SQLite by the `index/` files after `close`, on PostgreSQL by
  `pg_total_relation_size` of `log`, `applied` and the new index after
  `VACUUM ANALYZE`; and on PostgreSQL the conversion's time and peak
  room for 1 000 000 events. The table under Why is rewritten with what
  was measured.

### Decided with it, and left to the build

- **An id that is not of its form** (decided by the operator,
  2026-10-28). The store makes every id, `prev` and `hash` in
  lower-case hex, and checks `prev` and `hash` where it reads them, but
  not `id`: an import of a log another writer made may hold one of
  another form, which PostgreSQL could not keep as bytes. The
  conversion first looks for such an id and refuses, naming its
  position, leaving the lake as it was. An import holds every event's
  `id` to §3's form on every engine and refuses one that is not, naming
  its position: a new flaw in `docs/refusals.md`. A read does not
  check it.
- **How SQLite finds `id` in a line.** The member after the top-level
  `hash`, whose value the index holds, or a parse of the line; the
  store's `disagrees` catches a wrong one either way. The build measures
  a read of the whole log before and after and picks the cheaper.
- **The guard on PostgreSQL leans on 085's new table**, which ships in 0.21.0 (lakelands#1512 merged it): a 0.20.0 process refuses the schema before it reads a log row.

## Why

The review of 0.18.0 (lakelands#1295) found both: the SQLite index
repeated each line's `id`, `prev` and `hash`, and PostgreSQL kept them
as text and kept every id a second time in `applied`. Format 20 took
`prev` out of the index. In bytes a record, plain records; the "before"
figures are measured (SQLite as 083 measured it on 0.20.0, PostgreSQL
by the review on 0.17.0), the rest reckoned (est.):

| | before | after | saved |
|---|---:|---:|---:|
| SQLite index | 131.2 | ~99 est. | ~32 est. |
| PostgreSQL `log`: `id`, `prev`, `hash` | 163 | ~83 est. | ~80 est. |
| PostgreSQL `applied` (heap 77, key 80) | 157 | 0 | 157 |
| PostgreSQL index on `log (id)` | 0 | ~40 est. | −40 est. |

On PostgreSQL that is about 200 B of a plain record's 2 296 (about
9 %), on SQLite about 2 % of 1 837 (the totals the review measured on 0.17.0).

- **Kept once, where it already stands.** An event's line holds its id
  on every line; the index's copy and `applied`'s are copies a reader
  can do without. `hash` stays where it is read without the line (the
  last group's check, the next line's `prev`).
- **Bytes, not their spelling.** A hex digit spells half a byte; the
  column keeps the byte, and the index on the id keys 16 bytes, not 33.
- **An id answered from the log is the truth's answer.** `applied` was
  a projection table filled by the fold from the log; the index answers
  the same from the log itself, and a rebuild need not fill it. The
  random ids that made `applied`'s key costly to insert are now the
  index's, one insert in place of two.
- **Upgraded where the release already pays for its step.** The first
  writer's open of a new format is where 054 puts an upgrade, under the
  lease, with the release before stopped; one transaction leaves either
  the old lake or the new one.

## What it is not

- **Not a change of the log format, and not a rewrite of the log.**
  Every event keeps its bytes, its hash and its id; PostgreSQL rewrites
  how its columns beside them are typed, not what they say.
- **Not a rewrite of a SQLite index.** Lines already written keep their
  ids; a lake gains the saving on what it writes from 0.21.0 on.
- **Not `prev` dropped on PostgreSQL.** Derived from the row before, it
  would cost every read a row outside its range for 33 B.
- **Not `applied` dropped on SQLite** or in memory: neither keeps the ids
  where an index could reach them.
- **Not a unique index.** A log that holds an id twice still opens; the
  format asks ids unique, and no reader enforces it here.
- **Not a rebuild,** and no layout raise.
- **Not a downgrade path.** 0.20.0 refuses a lake 0.21.0 converted, on
  both engines; the way back is a backup taken before the upgrade.
