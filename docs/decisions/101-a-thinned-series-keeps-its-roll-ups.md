# 101 — A thinned series keeps its roll-ups

**Proposed** on lakelands#1639, for 0.23.0, line 5 of epic lakelands#1578. It
amends 049 and extends the log format's page; it stands on 074, 083, 098 and
099. No code lands with it. The operator reads it and its budget raise
before anything is built.

**A type is thinned past a horizon when the host runs one act.** The act's
records leave the projection; the log keeps every event and every byte; a
kept view whose group contained one of them keeps that group, answers it
as stranded, and never computes it. The series form is a no-go (098), so
this is designed on records.

Claims are marked **measured**, **read** (in the code at `3783131f`, and at
`b8fe5258` for the road of 099) or **reckoned**, as 098 does. No sentence
mixes them. The act's timings and bytes are S7's to measure; nothing here
is a promise of speed.

## What is decided

**Every change of format, stored table and public type this record makes:**

- **Log format 23 gains one kind:** `values_thinned`, one part of a
  thinning act, which begins the act. `act_whole` (099) ends it. The
  format's number does not move; 23 is the green field's number (the
  operator, 2026-10-31), and this change lands before the tag.
- **Stored tables on SQLite and PostgreSQL; `LAYOUT` rises by one.** Two
  new tables: `thinned_types (ty, by, before, at, gone)`, one row for each
  act on a type, `at` its first position; and `thinned (ty, key, before,
  at)`, one row for each record the act took. The memory engine keeps two
  maps.
- **On every engine:** a kept view's state gains `stranded: [u64; 2]`, the
  groups stranded on the open face and on the whole one; a stranded record
  bears the mark `sys:stranded`; the `sys:order.<view>` entries of a thinned
  member stay.
- **Public types.** `EventBody::ValuesThinned { ty, by, before, keys }` and
  its kind; the selection `Thin { by, before }`; `ThinReport`;
  `LakeStore::thin_values` and `thin_values_preflight`; the marker
  `ThinnedAct { by, before, through, gone }`; `Found::Records` gains
  `thinned: Vec<ThinnedAct>` and `stranded: Option<u64>`, the count of
  groups it could not answer; `Record` gains `stranded: bool`;
  `TypeDescription` gains `thinned: Vec<ThinnedAct>` and `stranded: u64`.
  The store facade and the engine traits hold 93 methods at
  `3783131f` by the seam gate's rule (read); the two new methods make 95
  of 110.
- **Refusals.** `ThinBy { ty, field }`; `Thinned { ty, key, before }`;
  `StrandedAtHeadOnly { ty, at }`; `KeptOverThinned { view, ty }`;
  `KeptNotExact { view, ty, group }`.
- **No new limit.** The act reads `part_events`, `part_tail`,
  `page_events` and `order_chunk` as 099 sets them.

### A thinned member leaves the projection and stays in its group

`thin_values` takes a type, one of its fields of instants or dates, and an
instant: every record whose field is certainly before that instant, and
whose key is not already thinned, is taken. A record whose field is absent
or null is not taken. A field of another kind is refused `ThinBy`, naming
the field.

- **The act deletes the record's states, at every position.** One call to
  `ProjectionStore::drop_states(StatesOf::Record(ty, key))` deletes the
  states the record holds, "kept history included", and takes with them
  its memberships in every set and the entries the engine derived from
  them (its indexes, full-text entries and edges) (read, `engine.rs:326`).
  The read that follows is exact: the record is gone at every position,
  not only at the head.
- **Vectors are the act's to take out.** `drop_states` leaves a record's
  vectors to the lake (read, `engine.rs:331`), so a part calls
  `VectorIndex::remove` for each vector field the type declares (read,
  `engine.rs:770`).
- **What stays is the roll-up.** A kept view's record is under
  `sys:kept.<view>`, keyed by the group, and its extreme's entries are
  under `sys:order.<view>`, keyed by the extreme, the group, the face, the
  place and the member (049, 098). Neither is a state of the source
  record, so `drop_states` deletes neither: the group's folds stay, the
  kept record holding each extreme's value, and an extreme whose holder
  was thinned keeps that holder, whose key and sort key its `sys:order`
  entry holds (`098 § A kept view keeps its head, and its order apart`).
- **A tombstone stays.** `thinned` holds the key and the horizon, one row
  a record, so a correction is refused by one seek and a `get` of the key
  is refused naming the horizon that took it. It is the act's price in the
  projection; S7 measures it against what goes.

### A group whose members have gone is stranded

049's rule is "a kept record is exactly what the read would compute, or
the view says at that position that it is not, and the read computes".
**A third case joins it: a group that held a thinned member is stranded —
the view says so at that position, and the read answers the kept record,
never the computation.** The record is what the read would have computed
at the act's position, with every member that arrived since folded in.
Stranded names the record's provenance, not the group's membership: a late
member may join it, and the group is stranded still, since the members it
lost are not there to compute it.

- **The fold notes no leave.** Where 049 has "the fold changes a group by
  the member that arrives, leaves or changes", a thinning is not a leave:
  the fold of `values_thinned` deletes the record's states and marks its
  group stranded, and never calls `kept::note` with the member gone (read,
  `lake/fold.rs:951` for the one path a write takes).
- **A stranded group is exact.** It was exact when it was stranded — the
  act refuses `KeptNotExact` when a kept view has a group that is not
  exact and would lose a member — and every change it can take since is
  the fold's: a new member arrives, a member still present leaves or
  changes. A correction to a thinned member is refused (`Thinned`), so no
  fold it holds is ever contradicted. `State::not_exact` never counts a
  stranded group; `State::stranded` does (read, `kept.rs:84`).
- **Two faces.** The mark is one for both faces, as `sys:differs` is, and
  `stranded: [u64; 2]` counts the groups stranded on each. A thinned
  member is gone for both faces, so where it was a member of both, the
  group is stranded on both; 049 lets a group be on the open face alone,
  and it is counted there alone.
- **A view made again keeps its stranded groups.** A declaration or a
  migration that makes a kept view again (049) would note the live records
  alone, since the thinned ones are no longer rows. So the make of a view
  with stranded groups carries each stranded group's record into the new
  view, with its folds, and the group stays stranded; the live records are
  noted on top, as 049's make does. The alternative — refuse the
  declaration — is not taken.
- **A view kept after the act is refused.** A `view_kept.` setting for a
  view whose source records are thinned is refused `KeptOverThinned`: its
  make would note the live rows alone and be exact in name and short in
  fact. A view over a view with stranded groups may be kept, since its
  members are the inner view's records, which are present.

### A late tick folds in; a correction is refused

- **Late.** A write under a new key whose field is before the horizon is a
  new record: the act's cut is one time, and nothing refuses it. It folds
  into its group as any member arrives. A group it joins is stranded and
  stays exact; a group it makes alone is not stranded.
- **Corrected.** A write, an ingest or a retirement under a key the act
  thinned is refused `Thinned`, naming the type, the key and the horizon;
  nothing is appended. A write that would make the key again (`made`) is
  refused the same, so a thinned record cannot be resurrected in the
  projection while its values are out of it.
- **From the first turn.** 099's first turn judges the tail again. A
  record of the tail whose field is before the horizon is taken in; a
  correction the tail carries to a record the walk selected lands, and the
  first turn judges that record again by the value it then holds, taking
  it in only if that value is still before the horizon. From the first
  turn the thinned keys are refused; before it they are ordinary writes.
  The horizon is the host's one-time cut, never a rule of the type: a
  write after the act is not refused for its time.
- **What a stranded group keeps.** The folds of the members it held, and
  those members' order entries, which stay: an extreme whose holder was
  thinned keeps its holder, and the holder can leave only by a correction,
  which is refused. A late member that sorts first takes the extreme, and
  the thinned holder stands next (049).

### A read says what it can no longer answer

- **The raw type.** `find` omits the thinned rows; the page's `thinned`
  lists each act on the type, oldest first, each
  `{ by, before, through, gone }` — the field, the instant, the last
  position the act had got through, and how many records it had taken.
  `total` counts what the
  lake holds and never adds `gone` as if the answer were whole. `get` of a
  thinned key is refused `Thinned`. The marker is the same at every
  position: the record's states are gone at every position, so an `as_of`
  read cannot answer them either, and says so. **The acts' markers and the
  stranded marks are read at the head in every read,** as 098's head-only
  names are: a read at an earlier position uses them to say what it cannot
  answer, and never as a value.
- **A kept view at the head.** A stranded group answers its record with
  `stranded: true`, and `describe` counts the stranded groups. A group
  with no thinned member answers as today, computed or kept by the view's
  state.
- **A kept view at an earlier position.** `sys:kept` keeps heads only
  (098), and a stranded group cannot be computed, so it answers at the
  head alone. A `find` at an earlier position omits stranded groups and
  answers their count in the page's `stranded` — it knows them from the
  head's marks, above; `get` of one is refused `StrandedAtHeadOnly`,
  naming the view and the position. A group with no thinned member
  computes as today. A stranded group's head record is never answered as
  if it were an earlier position's.
- **`describe`.** `TypeDescription` gains `thinned`, the acts on the type
  or, for a view, those on its sources, and `stranded`, the groups
  stranded. A view's `kept` stays as it is: a view with stranded groups
  is kept, and a view computed is computed for its own reason. The `Kept`
  enum gains no variant; a stranded group is not one of the states 049
  counts.
- **A count is the read's own.** A `count` over the raw type answers the
  rows the lake holds and says `gone`; it never answers the pre-thinning
  count as if whole.
- **A view that is not kept says the same.** A view over the type whose
  records are computed, not kept, answers from the rows that remain and
  carries its source's `thinned`, as the raw type does; its `describe`
  says the same. The act does not refuse it — a view nobody asked to keep
  would otherwise be able to stop the lake's thinning — and no caller
  reads its shorter answer as whole.

### The act runs in parts on 099's road

`thin_values` is an act, not a build. It walks on a view, off the writer,
the records of the type whose field is before the instant, with their keys
and groups; it catches up; it takes turns; it sweeps and reclaims
(`099 § One road`).

- **The first turn** appends `values_thinned` with the horizon and the
  first part's keys, puts the act's marker and the first tombstones, and
  deletes the first part's states. It destroys no key: thinning erases
  nothing.
- **A part** holds at most `part_events` keys, each its own turn under the
  writer: it deletes their records' states, takes their vectors out, puts
  their tombstones and appends a `values_thinned` part naming the keys.
  From 177 to 615 ms for 2 016 events, one part takes 45 to 150 ms
  (reckoned, loaded; 099's part row).
- **The report** says `ty`, `by`, `before`, `records`, `through` and the
  reclaim's `reclaimed`; it repeats no key, since the log's parts name
  them. The preflight answers the same counts, the views it would strand
  and any `KeptNotExact` it would meet.
- **A write meets the act's own line, not a refold.** 099 makes a write
  read what an owed act will leave, because a removal can put an erased
  value back into the log. Thinning takes no value out of a record a
  write may still reach: the selection is fixed at the first turn — the
  walk's records and the tail's, judged again under the horizon — and
  from it a key the act takes is refused `Thinned` while a key it does
  not take is ordinary and the act never touches it. So the write at a
  taken key answers a refusal, never a value the act has taken or will
  take, and no other write needs the refold.
- **The door is the catalog generator's,** as every door's name is; no
  literal tool name is written.
- **`act_whole` ends it,** always, so a reader of the log alone tells an
  act that is whole (099). The door answers when the act is whole; a
  caller who drops loses nothing.
- **An owed act resumes.** The kind is in `OWED`; a writer's open asks
  what the log owes, off the writer and after the open answers, and sends
  the job (read, `road.rs`; `099 § As built`). A crash between any two parts
  leaves the lake equal to one whose act was never killed (the suite's
  row).
- **The wait bound is 099's:** a write waits for at most the turn in front
  of it, about 300 ms a turn and about 2 s where a rebuild's swap or a seal
  falls, at the sizes 099 measured (read, `b8fe5258`; the numbers are
  `limits`, and R1 measures them on a quiet machine). The act blocks no
  write for the length of the type's history.
- **No trigger fires** for a thinned record: the act appends act events,
  not writes. A set over the thinned type loses the record, because its
  membership is a state of the record and `drop_states` takes it; a set or
  trigger over a view with stranded groups reads the stranded record and
  stands. The type's marker says what a set over it does not see.

### The log keeps every byte, and verify, export, import and rebuild hold

- **The log grows, never shrinks.** The act appends its parts and
  `act_whole`; it rewrites no byte and moves no segment (074). The hash
  chain is the append's, as 083 and 098 have it, and `verify` passes,
  between any two parts as after the last (099's row). The values the act
  took out of the projection are still in the log, and a rebuild reads
  them.
- **Export, import and backup are the log's.** An export is the log (083)
  and carries the act's parts; a copy between parts carries an act without
  `act_whole`, which is a log (099). An import's writer finishes an act
  the log owes. A backup is the log and the files beside it; a rebuilt
  projection makes the same lake, and the suite checks it.
- **A rebuild folds the same act.** It deletes the same records' states,
  puts the same tombstones and strands the same groups, and the order
  entries and kept records it puts on the way are the ones the live
  appends put. The suite's row checks the rebuilt lake equals its
  projection, at the head and at an earlier position.
- **Decided with lakelands#1524.** Moving or compressing a sealed segment
  is that record's decision, out of 0.23.0 (epic lakelands#1578, moved to
  0.24.0 unless the operator's word). Nothing here depends on where a
  segment lives or whether it is compressed: the events restore and hash
  as 083 and 098 say, and this act never removes one. The two records meet
  at one line: the log keeps every event a lake ever appended. This record
  gives back the projection's bytes and promises the log is whole, not
  smaller.
- **Format 23.** `LOG_FORMAT_VERSION` and `OLDEST_LOG_FORMAT` stay 23 (read,
  `event.rs:26`); the kind joins 23's set, `docs/log-format.md` states it,
  each engine's `projection.rs` states the two tables and raises its
  `LAYOUT` once more. A lake or an export of an older format is refused by
  `LogFormat`, as today, and nothing is translated.

### What it gives back, and what it costs

A tick's projection is 698 B after 098's set (measured, 098), of which the
record, its indexes, its routes and its state are the parts a thinning
deletes. A tombstone is the key, the horizon and the act, reckoned at
about 24 B; a log part names each key, reckoned at about 20 B a record. So a
thinned record gives back the larger part of 698 B and keeps about 44 B,
the tombstone and the log's part, which is the price of replaying the act
and refusing a correction.

- **Reckoned for a year of one reading a second:** 31.5M records, about
  22 GB of projection thinned and about 1.4 GB kept (tombstones and log
  parts). The issue's two years are twice that.
- **Measured by S7, not here:** the bytes the file gives back. SQLite's
  `secure_delete` zeroes the deleted pages at the delete, the WAL
  truncates and `incremental_vacuum` returns the file (098, 099's reclaim
  row, both measured); PostgreSQL's `VACUUM` runs off the lease (099). The
  number goes into `docs/hosting.md` with the act's preflight and act
  timings, as line 1 of the epic asks.

### The build plan

Seven pull requests, each of at most 600 lines at ×1.35, as 098's and 099's
are. They land after 098's F12 and 099's R2 to R6, on the road R1 built
(lakelands#1675, `b8fe5258`).

| Step | What | Files | Product | Spec. |
|---|---|---|---:|---:|
| S1 | The kind, the public types, the refusals | `event.rs`, `read.rs`, `record.rs`, `write.rs`, `error/case.rs`, `api/tests/wit.rs` | 160 | 180 |
| S2 | The two tables on every engine, `LAYOUT`, the format's page | the engines' `projection.rs`, `conformance/engine/projection.rs`, `docs/log-format.md` | 100 | 120 |
| S3 | The stranded mark, the counter, the fold's arm | `kept.rs`, `lake/fold.rs` | 120 | 180 |
| S4 | The make of a view with stranded groups, the order entries, the checks | `kept.rs`, `lake/fold.rs`, `conformance/checks/stranded.rs` | 100 | 160 |
| S5 | The act: the walk, the first turn, the parts, the tail's judgement, `OWED` | `thin.rs` (new), `lake.rs`, `lake/open.rs`, `lake/standby.rs`, `conformance/checks/thinned.rs` | 180 | 240 |
| S6 | The refusals at the write doors, the store facade, the catalog and the command | `writes.rs`, `ingest.rs`, `doors.rs`, `catalog.rs`, `api/tests/wit.rs`, `cli/commands.rs` | 120 | 150 |
| S7 | The reads' markers at the head and at an earlier position, the reclaim, the measured bytes | `reads.rs`, `lake/described.rs`, `doors.rs`, the engines' `view.rs`, `docs/hosting.md`, the suite's rows | 120 | 170 |
| | **Sum** | | **900** | **1 200** |
| | **×1.35** | | **1 215** | **1 620** |

**The raise.** By `docs/budgets.md`'s rule, a round's reserve is sized from
the count measured on the pull request that records it and the estimate
of the round's tasks, rounded up to the next 500. At ×1.35 the plan is
1 215 product and 1 620 specification; rounded up, the raise is **+1 500
product and +2 000 specification**, taking the ceilings from **107 000 to
108 500** and from **189 500 to 191 500**. The epic's 102 500 and 187 000
were reckoned before 098's and 099's reserve raised the ceilings on
2026-11-01; the tree's 107 000 and 189 500 are what this raise lands on.
No row is written by this record: it lands with the operator's word,
before S1, as 098's did.

**Cut first, with what each costs:**

1. **The `stranded` count per face** (one count, not two): 20 and 30. A group
   on the open face alone is counted with the whole face.
2. **`describe`'s `thinned`:** 30 and 50. A caller reads the view and the
   type through `find` and `get` alone.
3. **The command:** 40 and 40. The store and the doors stand, and a host
   calls the act through them.
4. **The vectors' removal:** 30 and 40. The rows stay indexed until a
   later act; the bytes they hold are not given back.

Not to cut: S1's kind, S2's tables, S3's mark, S4's make, S5's first turn and
refusal, S7's earlier-position statement. The act without them either
answers less as if whole or cannot be rebuilt.

### Conformance rows

- `THIN_TAKES_THE_RAW_ROWS`: after the act, the type's `find` omits the
  thinned rows and says `gone`, `get` of one is refused, and no state of
  the record is in the projection at any position.
- `THIN_KEPT_VIEW_ANSWERS_AS_BEFORE`: every kept view of the type answers
  each group as it did at the act's position, on every engine, live and
  after a rebuild.
- `THIN_GROUP_STRANDED`: a group with a thinned member answers as stranded, with
  the folds of the act's position, and `describe` counts it.
- `THIN_LATE_TICK_FOLDS`: a new record before the horizon in a stranded group
  folds in, and the group stays exact and stranded.
- `THIN_CORRECTION_REFUSED`: a write, an ingest and a retirement at a
  thinned key are each refused, nothing is appended, and the projection is
  unchanged.
- `THIN_AS_OF_SAYS_UNANSWERABLE`: a read at an earlier position omits a
  stranded group and counts it; `get` of it is refused; a group with no
  thinned member computes.
- `THIN_EXACT_GROUPS_ONLY`: the act refuses a view with a not-exact group
  that would lose a member.
- `THIN_PARTS_AND_WRITES`: a write sent during the act answers within one
  turn; a killed writer's open finishes the act; `verify` passes between
  any two parts.
- `THIN_VERIFY_EXPORT_IMPORT_REBUILD`: the log verifies, an export between
  parts imports, and a rebuild equals the projection.
- `THIN_VIEW_KEPT_AFTER_REFUSED`: a setting over a thinned type is refused,
  and one over a view with stranded groups is not.

One test beside the suite, SQLite, `THIN_BYTES_RETURNED`: the act's reclaim
gives back the thinned rows' bytes, measured on the file, with the
tombstones' bytes named in the same row.

### Open choices for the operator

1. **A correction into a thinned group.** Refused, as above (recommended):
   the record's before-values are out of the projection, so it cannot be
   compared and set, and a fold it holds could be contradicted. Or folded
   in as a new version at the same key, with the thinned version's value
   unknown and the group then counting both.
2. **A stranded group at an earlier position.** Refused and counted, as above
   (recommended). Or answered with the head's record and a marker, which
   answers later folds as if they were earlier.
3. **The log's part.** The keys, as above (recommended): the fold deletes
   exactly what the act took, and a rebuild stays a fold. Or the rule
   alone, with the fold walking the type's events at the act's position,
   which saves about 20 B a record and adds a walk to every rebuild of the
   lake.
4. **A group that is not exact at the act.** The act refuses it, as above
   (recommended), so every stranded group is exact. Or holds it flagged and
   answers less than it answered before, which the issue's done-when rules
   out.
5. **The raise:** +1 500 product and +2 000 specification, to 108 500 and
   191 500, with its row landing on the operator's word.

## Why

- **The roll-up is what a series is kept for.** Two years of a reading a
  second are 63M rows, and a year's roll-up is 8 760 groups. A host that
  can drop the rows and keep the groups has the shape the epic's host
  asked for; a host that cannot must keep every tick or lose its history.
- **A fold over a gone member is not a computation.** 049's kept record is
  the read's own answer, so the one thing the fold cannot do is recompute
  a group whose members it no longer has. Holding the record as it last stood is the honest
  third case: it says so, and it says what it is the fold of.
- **A one-time cut keeps a stranded fold true.** Refusing a correction and
  folding a late arrival is what makes a stranded group exact without the
  members. If a correction were folded instead, every fold the group holds
  from the thinned members would need a before-value the projection no
  longer has.
- **The log's bytes are not this act's to give.** The chain and `verify`
  are over the log, and 074 moves no segment. Deleting a record's
  projection states is the only deletion that gives bytes back without
  breaking either; moving or compressing the log's bytes is
  lakelands#1524's, and it changes nothing this record promises.
- **The act is a write's neighbour, not a rebuild.** 099's road is the one
  the round built for work beside the writer, and thinning is a walk, a
  few hundred turns and a reclaim; it blocks no writer for the length of
  the lake's history.

## What it is not

- **Not erasure.** The log keeps every value the act thinned; a read of
  the raw type cannot answer it, but a rebuild of the projection puts it
  back, and `get` of the key answers `Thinned`, naming the horizon. A host that
  needs the values gone uses `remove_values`, `forget_source` or
  `purge_source`, and this act does not stand in for them.
- **Not thinning to a number of points.** No largest-triangle, no
  downsampling, no bucket that keeps one value of many. The cut is a time
  the host names.
- **Not a move or a compression of the log.** 074 and lakelands#1524 hold
  that; this act appends and never removes.
- **Not a clock.** Nothing thins by itself; the host runs the act, and the
  preflight says what it would take.
- **Not a change to `verify`'s promise.** The log is whole at every turn,
  and `verify` reads it as it reads any append.
- **Not a series form.** The series form is a no-go (098); this is records
  with a horizon.
- **Not a promise of these bytes or times.** They are reckoned; S7
  measures them on every engine, and the suite holds the behaviour.

## How it came to be

- Proposed on lakelands#1639, for 0.23.0, after the series form's no-go
  (098, 2026-11-01) left it designed on records.
