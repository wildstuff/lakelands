# 054 — A lake is written in one format a release

## What is decided

### There are two halves, and the operator names the release at which the first gives way to the second

There are two halves, and the operator names the release at which the
first gives way to the second.

This replaces the versioning rule of `docs/log-format.md` §6 as it stood
through format 14's first writing, and the sentences of other records
that restated it.

### Until the first release with real installations, there is one log format a release, and no older lake is read

- **A release reads and writes one format version,**
  `LOG_FORMAT_VERSION`. Every event of a lake carries it as `v`, from
  position 0, and `lake_created.format` says the same number
  (`docs/log-format.md` §6).
- **The number rises by one in a release that changes the format, and
  only there.** One step a release, whatever the release adds. A release
  that changes nothing of the format keeps the number and reads the
  lakes of the release before it. A number that rose at every release
  would be a release number, and would refuse lakes for nothing.
- **Between two releases the format is open.** The first change after a
  release raises the number, and later ones change the format under the
  same number. A lake written by a build between two releases may stop
  opening; that price is paid in development alone.
- **A lake in another format is refused, older or newer, and never read
  in part.**
  - An open, in any mode, judges the lake's format before the lake's
    engine is opened. The store asks the opener for the log's first and
    last event (`EngineOpener::log_ends`, which reads the log and makes
    and changes nothing) and judges three numbers: the first
    event's `v`, the format its `lake_created` says, and the last
    event's `v`. Another format is refused as the situation
    `log_format`, naming the lake, the first of the three that is
    another and the one read. The engine's `open` is not called, so no
    projection is judged or touched, no lease taken, nothing cleared,
    folded or appended: a lake the release before left is told its
    format, and never that its projection's layout is unknown. Such a
    lake is not damaged and restoring it would not help, so it is not
    said to be.
  - An import asks the same before it opens its target. Onto a lake of
    another format it is refused as `log_format`; onto a lake of this
    format as `import_not_empty`, naming the tip. The engine is opened
    for neither, so an import takes no lease on a lake that is there
    and changes nothing of it.
  - A place holds no lake when it holds no committed event and nothing
    that was made from one. Part of a lake without its events is not
    "no log": an engine refuses it as `damaged` where it can tell (a
    SQLite segment without its index, a projection with no segment
    file,
    `docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`),
    and the store refuses, on every engine, a projection folded past
    the log's tip, at a reader's open, a writer's and an import. A
    writer is asked before it takes the lease, since an engine may cut
    its log as it hands the lease out, so a lake that is refused is
    left as it was found. A rebuild is not asked: it makes the
    projection from the log as it stands (lakelands#865).
  - Every other read of the log (the feed, `verify`, a rebuild, an
    import, the verification of an export) opens its events through one
    check, which refuses an event in another format at its position as
    the flaw `format_other`, naming both numbers. A log that mixes
    formats is refused at its first event of another. An open judges
    the two ends alone; an event of another format between them is
    `verify`'s, a writer's fold's and a rebuild's to find.
  - A `lake_created` that says another format than the release's is
    refused where it is read as the flaw `created_in_another`, at
    whatever position it stands (a store writes it at position 0
    alone): its lake was made by another release. A line whose own `v`
    is another is refused for that first, as `format_other`. That
    member is where an upgrade starts, so both verifiers of an export
    hold it.

### From the first release with real installations on, every change of the format carries its upgrade path

How a lake the release before wrote becomes one the new release reads is
then part of the change, and the suite proves it on a lake the release
before wrote. Upgrading is then a commitment of the first rank, not an
option a release may leave out. The operator named 0.14.0 that release,
and its format 15 the first step: 0.14.0 reads 14 and 15 and writes 15.
0.15.0 takes the second, to 16, once, by a step of its own before any
change that writes a form 15 does not hold: it reads 14 to 16 and writes
16, so a 0.14.0 binary refuses its lakes by their number
(lakelands#1096). 0.16.0 takes the third, to 17, the same way: it reads
14 to 17 and writes 17, so a 0.15.0 binary refuses its lakes by their
number (lakelands#1164). 0.17.0 takes the fourth, to 18: it reads 14 to
18 and writes 18, so a 0.16.0 binary refuses its lakes by their number
(lakelands#1214). 0.18.0 takes the fifth, to 19: it reads 14 to 19 and
writes 19, so a 0.17.0 binary refuses its lakes by their number
(lakelands#1330). 0.20.0 takes the sixth, to 20: it reads 14 to 20 and
writes 20, so a 0.19.0 binary refuses its lakes by their number
(lakelands#1409). Its one step is proposed to carry also a new stored
form of the log and a record's values sealed in one box, each in force
with the issue that builds it
(`docs/decisions/083-a-log-stores-what-an-append-shares-once-and-seals-a-records-values-in-one-box.md`,
lakelands#1413), and a lake of 19 is read without a rewrite.

**How a lake is upgraded** (`docs/log-format.md` §6):

- **The log is never rewritten.** Each format step from 14 on ships a
  converter, a function over an event's members as the earlier format
  wrote them. A reader runs the converters from an event's `v` up to the
  release's, and the event keeps its `v`, its bytes and its hash. Every
  hash, checkpoint and export a host already holds stays true; a log
  that replaced the events with new ones would be a second truth (001).
  A step that changes how events are stored, as 083 proposes for 20,
  changes it only for the lines written after it: a line of an earlier
  format is a full event, read as it stands beside the new ones in one
  segment, and a value sealed in an earlier format opens by its event's
  `v`.
- **A writer's open upgrades.** It appends, after its `lease_taken`, one
  `lake_upgraded` from the older format to the release's, and the lake
  is written in the new one from there; a step that changes a fold also
  has the open rebuild the projection. `lakelands upgrade` is that open
  as a command, with `--check` to say what it would do. A read-only open
  of such a lake is refused, naming the way. An import of an older
  export upgrades the new lake the same way.
- **A format changes only where a new writer upgrades it**, at its
  `lease_taken` followed by the `lake_upgraded`, and never falls; both
  verifiers refuse any other change.
- **There is no downgrade.** A backup taken before the upgrade is the
  way back.
- **The proof on a lake the release before wrote**:
  `docs/log-format/lakes/14/`, a small lake v0.13.1 made through its
  doors, with what that release answered of it, is imported, upgraded
  and read the same on every engine. `docs/log-format/lakes/15/`, the
  lake v0.14.0 made, is held the same way from that release on, read in
  its own format until a later one upgrades it, as 0.15.0 does;
  `docs/log-format/lakes/16/`, the lake v0.15.0 made, from that release
  on, `docs/log-format/lakes/17/`, the lake v0.16.0 made, from that one
  on, `docs/log-format/lakes/18/`, the lake v0.17.0 made, from that one on,
  and `docs/log-format/lakes/19/`, the lake v0.18.0 made, from that one
  on. The vectors of each older format are imported and upgraded on
  every engine too.

At 100 000 events a step that changes no fold costs one append, and
about 5 µs an event at each later read of an older event; one that
changes a fold costs a rebuild, about 7 s on SQLite and 35 s on
PostgreSQL. Rewriting the log would cost an export, the conversion and
an import, about 14 s and 44 s, and would move every hash.

**What was kept so that the path is cheap:**

- `v` on every event and `lake_created.format`, so a lake says what
  wrote it without being read further;
- a refusal that names both numbers, at the one place an open decides
  it: that is where the upgrade's instruction will stand;
- one check on every read of the log, so no reader takes an event of
  another format for one of its own;
- the vectors as one lake's log, in a directory a format
  (`docs/log-format/vectors/<N>/`): a format step copies the newest to
  the next number and stamps the copy by one command (`verify.py --stamp
  N`), so a step is that command and the store's agreement with it; the
  directory of every format a release shipped stays as its tag holds it,
  byte for byte (`scripts/gates/vectors-kept.sh`), and every later
  release runs it.

### An import, a mirror and a moved pin stop at a format this release does not read

Each of these is refused in the words above and never done in part,
where the other format is one the release does not read; one from 14 on
that is older is upgraded instead, and a mirror's source is opened as a
reader, so it is upgraded first:

- **An import of an export another release wrote.** The export's lines
  are in that release's format; the import verifies the whole export
  before it writes, and refuses it at its first line.
- **A mirror whose source is a lake of another format.** A connection's
  source is a lake of the same root, opened by the process that mirrors
  it (`docs/decisions/042-water-crosses-only-as-a-mirror.md`): that
  open is refused, and no mirror of it starts.
- **A host that moves its pin across a format step.** Its lakes do not
  open under the new release. Until the second half holds, such a host
  starts the new release on fresh lakes; what it needs from the old ones
  it writes again through the doors.

### The refusals that have nothing left to say are withdrawn from the catalog

The refusals that told of a range of formats, of a format going down
along a log and of a member carried below its version have nothing left
to say. They are withdrawn by the rule of
`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`
("A code leaves by a recorded withdrawal"), and `docs/refusals.md` lists
them.

## Why

- **Nothing reads an older lake today.** No installation holds data of
  its own yet. A lake that wrote "the lowest version each event needs"
  kept every reader able to read every older log: a table of what each
  version added, a check of every event against it, a refusal for each
  member carried too early, and a test for each of those. That was about
  a thousand lines kept true for lakes that do not exist.
- **The rule it replaces was also the harder one to keep.** A lake's
  format depended on its history: which append first held which member.
  Two lakes of one release differed, and a reader's refusal depended on
  where in the log it stood. One number a release is a rule a host can
  hold in its head.

## What it is not

- **The second half was not built early.** An upgrade path written
  before a format it must upgrade from was real would have been written
  against a guess. It was written with the first change that needed it,
  the step from 14 to 15.
- **A refusal, not a repair.** A release that read an older format in
  part would answer from a log it does not fully understand. Refusing
  the lake whole, by name and with both numbers, is the one answer that
  cannot be wrong.

## How it came to be

- The vectors of each format a release shipped are kept in a directory
  of their own, as its tag holds them, and run by every later release;
  before, one directory was stamped again in place at each step, and
  nothing of an older format was kept (lakelands#962).
- The second half is built: a release reads every format from 14 on,
  each older event through its format's converter, and a writer's open
  upgrades a lake with `lake_upgraded`; a reader of such a lake is
  refused with the way, and a lake's format changes only where it is
  marked. Before, a lake of format 14 was refused under 0.14.0's format
  15 (lakelands#962).
- 0.16.0's step, to 17, is taken as 0.15.0's to 16 was: one step of its
  own before any form it adds, the vectors of 16 kept as v0.15.0 holds
  them (lakelands#1164).
- 0.17.0's step, to 18, is taken as 0.16.0's to 17 was: one step of its
  own before any form it adds, the vectors of 17 kept as v0.16.0 holds
  them (lakelands#1214).
- 0.18.0's step, to 19, is taken as 0.17.0's to 18 was: one step of its
  own before any form it adds, the vectors of 18 kept as v0.17.0 holds
  them (lakelands#1330).
- 0.20.0's step, to 20, changing also how the log stores events, a
  lake of 19 keeping its lines as they stand, proposed with 083
  (lakelands#1413).
