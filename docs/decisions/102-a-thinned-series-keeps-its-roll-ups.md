# 102 — A thinned series keeps its roll-ups

Decided on lakelands#1578, 2026-11-02
(https://github.com/wildstuff/lakelands/issues/1578#issuecomment-6572113836):
the operator took every recommendation, with the raise.

**Accepted** on lakelands#1639, for 0.23.0, line 5 of epic lakelands#1578. It
amends 049 and 099 and extends the log format's page; it stands on 074, 083
and 098. No code lands with it. The operator read it and its budget raise;
both are accepted, and the raise's two rows are in `docs/budgets.md`.

**A type is thinned past a horizon when the host runs one act.** The act's
records leave the projection; the log keeps every event and every byte; a
kept view whose group contained one of them keeps that group, answers it
as stranded, and never computes it. The series form is a no-go (098), so
this is designed on records.

Claims are marked **measured**, **read** (in the code at `aac038fc`, and at
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
  new tables: `thinned_types (ty, by, before, at, through, gone)`, one row
  for each act on a type, `at` its first position, `through` the last
  position its parts had got through and `gone` how many records it had
  taken through it, both read at the head and moved by each part; and
  `thinned (ty, key, before, at)`, one row for each record the act took.
  The memory engine keeps two maps.
- **On every engine:** a kept view's state gains `stranded: [u64; 2]`, the
  groups stranded on the open face and on the whole one; a stranded record
  bears the mark `sys:stranded`; the order entries (`view_order`, 098) of a
  thinned member stay.
- **Public types.** `EventBody::ValuesThinned { ty, by, before, keys }` and
  its kind; the selection `Thin { by, before }`; `ThinReport`;
  `LakeStore::thin_values` and `thin_values_preflight`; the marker
  `ThinnedAct { ty, by, before, through, gone }`, which names its type
  so an act a view lists across its sources is attributed; `Found::Records`
  and `Found::Groups` gain `thinned: Vec<ThinnedAct>` and
  `stranded: Option<u64>`, the count of groups it could not answer;
  `Record` and `Group` gain `stranded: bool`;
  `TypeDescription` gains `thinned: Vec<ThinnedAct>` and `stranded: u64`.
  Four engine-trait methods reach the two tables: the fold's
  `ProjectionStore::put_thinned_act` (an act's row) and `put_thinned`
  (a record's tombstone), and the reads' `ProjectionView::thinned_acts`
  (the acts on a type) and `thinned_at` (a seek by key, answering the
  horizon). The store facade and the engine traits hold 94 methods at
  `aac038fc` by the seam gate's rule (read); the two new facade methods
  and these four make 100 of 110.
- **Refusals.** `ThinBy { ty, field }`; `Thinned { ty, key, before }`;
  `StrandedAtHeadOnly { ty, position }`; `KeptOverThinned { view, ty }`;
  `KeptNotExact { view, ty, group }`; `StrandedMake { view, ty }`.
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
  in the view's order (`view_order`, 098), keyed by the source's extreme (its
  call), the group, the face, the place and the member. Neither is a state of the source
  record, so `drop_states` deletes neither: the group's folds stay, the
  kept record holding each extreme's value, and an extreme whose holder
  was thinned keeps that holder, whose key and sort key its order entry
  holds (`098 § A kept view keeps its head, and its order apart`).
- **A tombstone stays.** `thinned` holds the key and the horizon, one row
  a record, so a correction is refused by one seek and a `get` of the key
  is refused naming the horizon that took it. It is the act's price in the
  projection; S7 measures it against what goes.

### A group whose members have gone is stranded

049's rule is "a kept record is exactly what the read would compute, or
the view says at that position that it is not and the read computes, or
the members it was computed from are gone and the group is stranded,
answered at the head alone". **A group that held a thinned member at any
position is stranded: the view says so, and the read answers the kept
record, never the computation.** The record is what the read would have
computed at the act's position, with every member that arrived since
folded in. Stranded names the record's provenance, not the group's
membership: a late member may join it, and the group is stranded still,
since the members it lost are not there to compute it.

- **Every group at every position, not the head's alone.** The act
  deletes a record's states at every position, so an earlier read loses
  the member where it stood then, even where its group field or its
  restriction moved it to another group, or it stood on one face alone,
  before the act. A part reads each selected record's versions as they
  stand before `drop_states` — the record's `made` through the act's
  first position — and marks stranded every group key, on every face, a
  version of it held. The mark is the view's, one per group and face, and
  it is read at the head for every read (below). So a group that was
  whole at an earlier position may be omitted there: the record's history
  is one mark, and the read refuses a group rather than answering it
  short.
- **A retired record is a record.** One retired before the act is taken
  by the value it held when it retired, so its key is in the selection:
  its states — its earlier positions, which the retirement left — are
  dropped, its row with them, its tombstone put, and every group and face
  its versions held is stranded as above. It is a member of no group at
  the act's position, so a group it left is stranded for those earlier
  reads alone, while the mark is one and the head answers its kept record
  too. `get` of its key answers `Thinned`, and a write that would make it
  again is refused, as below.
- **The fold notes no leave.** Where 049 has "the fold changes a group by
  the member that arrives, leaves or changes", a thinning is not a leave:
  the fold of `values_thinned` deletes the record's states and marks every
  group and face the record held at any position stranded, and never calls
  `kept::note` with the member gone (read, `lake/fold.rs:951` for the one
  path a write takes). A rebuild folds the same mark, since the fold meets
  the record's versions in order before it meets the act.
- **A stranded group is exact.** It was exact when it was stranded — the
  act refuses `KeptNotExact` when a kept view has a group it would strand
  that is not exact — and every change it can take since is
  the fold's: a new member arrives, a member still present leaves or
  changes. A correction to a thinned member is refused (`Thinned`), so no
  fold it holds is ever contradicted by a write; an erasure act is not a
  correction, and it is folded from the log again (below).
  `State::not_exact` never counts a stranded group (read, `kept.rs:91`);
  this record adds `State::stranded`, which does.
- **An erasure reaches a stranded group's folds.** `remove_values` may
  select a value only a stranded group's folds now hold: the selection is
  the source's events, which the log keeps, and the act is a rebuild of
  the projection (065), so the fold meets the value as erased at its own
  position, before the act that thinned it. The group's folds are that
  fold, and the group stays stranded and exact; what the restatement
  leaves is the fold of the log with those values erased, as 065 decides
  it — a member erased of everything its source wrote may retire and leave
  the count, a sum drops the erased values, and an extreme whose holder's
  value was erased loses its entry, the next holder standing.
  `forget_source` restates the projection the same way, its keys destroyed
  (058), and leaves the same folds. A correction is refused because its
  fold needs the record's before-value, which the act took; an erasure is
  folded from the log again, and needs none.
- **Two faces.** The mark is one for both faces, as `sys:differs` is, and
  `stranded: [u64; 2]` counts the groups stranded on each. The mark is
  set on each face where a version of the record was a member of the
  group on that face, so a group the record was on both faces of is
  stranded on both, and one it was on the open face alone at some
  position — 049 lets a group be there — is counted there alone.
- **A view made again keeps its stranded groups, where the event leaves
  what they fold.** A declaration or a migration that makes a kept view
  again (049) would note the live records alone, since the thinned ones
  are no longer rows. So the make reads each stranded group's record and
  its order entries off the view it ends, puts them in the view it
  makes, and **notes none of that group's live records on top**: the
  carried folds already hold every member the fold has seen, those that
  arrived since the act included, so noting them again would count them
  twice. Every group with no carried record is noted from the live rows,
  as 049's make does. The carried record is exact only where the event
  leaves every input its folds were computed from as it was — the view's
  own `where`, `group_by` and aggregate calls, every source field those
  read, and the members' values: the view or a source declared with
  nothing the view reads changed. An event that changes one of them — a
  declaration that rewrites the group key, a measure or the `where`; a
  migration whose step moves the sources' records; a setting an
  expression of the view names written — is refused `StrandedMake`,
  naming the view and the type, before it is appended: its carried record
  would answer a rule the new declaration no longer states, and the
  thinned members are gone to compute it. A host that means to change the
  rule ends the kept view first; the view then computes from the rows that
  remain and carries its `thinned`, as any computed view does.
- **A view kept after the act is refused.** Every event that would make a
  view whose source records are thinned a kept view — a `view_kept.`
  setting, a declaration that declares it kept, a migration whose step
  does — is refused `KeptOverThinned`: its make would note the live rows
  alone and be exact in name and short in fact. A view over a view with stranded groups may be kept, since its
  members are the inner view's records, which are present at the head.
  **A group of it that holds a stranded inner group is stranded too:** it
  bears the mark, its folds at the head hold the inner's stranded records,
  and an earlier-position read of the outer omits it and counts it in the
  page's `stranded`, while `get` of it is refused `StrandedAtHeadOnly`,
  naming the inner view and the position. The read the outer would
  compute from cannot answer there, and the outer never answers short.

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
  it in only if that value is still before the horizon. **The selection
  is the record's state at the act's first position:** a key is one the
  act takes when the record of the type stood at that position with its
  field certainly before the horizon, or had retired by it and the value
  it held when it retired was so. The first turn's own position is the
  act's first position, held in the marker, and a write at the type reads
  the record there and is refused `Thinned` when it is in the selection,
  whether or not a part has reached it yet. A key the record had not at
  that position — a late tick — and one whose field was not before the
  horizon are ordinary, and the act never touches them. A resumed act,
  and a rebuild, derive the same selection from the same marker and the
  same position, holding nothing in memory; the log's parts name the keys
  the fold takes, below. Before the first turn the writes are ordinary.
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
  `{ type, by, before, through, gone }` — the type, the field, the instant,
  the last position the act had got through, and how many records it had
  taken. `ty` is carried because a view's page lists the acts on all its
  sources, and an act is attributed to its type.
  `total` counts what the lake holds and never adds `gone` as if the
  answer were whole. An aggregate answers `Found::Groups`, which carries
  the same `thinned` beside its rows — the type's acts, or a view's
  sources'. `get` of a thinned key is refused `Thinned`. The marker is
  the same at every position: the record's states are gone at every
  position, so an `as_of` read cannot answer them either, and says so.
  **The acts' markers and the stranded marks are read at the head in every
  read,** as 098's head-only names are: a read at an earlier position uses
  them to say what it cannot answer, and never as a value.
- **A kept view at the head.** A stranded group answers its record with
  `stranded: true`, and `describe` counts the stranded groups. A group
  with no thinned member answers as today, computed or kept by the view's
  state.
- **A kept view at an earlier position.** `sys:kept` keeps heads only
  (098), and a stranded group cannot be computed, so it answers at the
  head alone. A `find` at an earlier position omits stranded groups and
  answers their count in the page's `stranded` — it knows them from the
  head's marks, above; `get` of one is refused `StrandedAtHeadOnly`,
  naming the view and the position; an aggregate omits them and answers
  their count the same way. A group with no thinned member at any position
  computes as today. A group a record held only at an earlier position —
  where its field or restriction moved it, or it stood on that face alone
  — is stranded there too: the earlier read omits it and counts it in
  `stranded`, though its computation at that position would have been
  whole, because the mark is one for the record's history and a short
  computation is never answered as whole. A stranded group's head record
  is never answered as if it were an earlier position's.
- **`describe`.** `TypeDescription` gains `thinned`, the acts on the type
  or, for a view, those on its sources, and `stranded`, the groups
  stranded. A view's `kept` stays as it is: a view with stranded groups
  is kept, and a view computed is computed for its own reason. The `Kept`
  enum gains no variant; a stranded group is not one of the states 049
  counts.
- **A count is the read's own.** A `count` over the raw type is a
  `Found::Groups` answer with one row and no group key, and its `thinned`
  says what it left out; any group-by aggregate's answer carries the same.
  None answers the pre-thinning count as if whole.
- **A view that is not kept says the same.** A view over the type whose
  records are computed, not kept, answers from the rows that remain and
  carries its source's `thinned` and `stranded`, as the raw type does;
  its `describe` says the same. The act does not refuse it — a view
  nobody asked to keep would otherwise be able to stop the lake's
  thinning — and no caller reads its shorter answer as whole.

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
  write may still reach: the selection is the record's state at the act's
  first position, which the write reads there, and from it a key the act
  takes is refused `Thinned` while a key it does not take is ordinary and
  the act never touches it. So the write at a taken key answers a refusal,
  never a value the act has taken or will take — a key a later part has
  not reached included — and no other write needs the refold.
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
  `event.rs:26`); the kind joins 23's set, and the SQLite and PostgreSQL
  engines state the two tables in their `projection.rs` and raise its
  `LAYOUT` once more (9 and 10 today, read); the memory engine keeps two
  maps and has no layout to raise. `docs/log-format.md` states the kind
  when S2 lands, the step that adds the tables. A lake or an export of an
  older format is refused by `LogFormat`, as today, and nothing is
  translated.

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
| S2 | The two tables on SQLite and PostgreSQL, the memory engine's maps, `LAYOUT`, the format's page | the engines' `projection.rs`, `conformance/engine/projection.rs`, `docs/log-format.md` | 100 | 120 |
| S3 | The stranded mark, the counter, the fold's arm, the read of a key's states | `engine.rs`, the engines' `handle.rs` and `projection.rs`, `kept.rs`, `lake/fold.rs`, `lake/scrub.rs`, `conformance/engine/projection.rs`, `conformance/checks/forgotten.rs`, `store/tests/probed/mod.rs`, `store/tests/fold_gap.rs`, `docs/seams.md`, `scripts/release/surface-accepted` | 260 | 230 |
| S4 | As before: the make of a view with stranded groups, the order entries, the checks | `kept.rs`, `lake/fold.rs`, `conformance/checks/stranded.rs` | 100 | 160 |
| S5 | The act: the walk, the first turn, the parts, the tail's judgement, `OWED` | `thin.rs` (new), `lake.rs`, `lake/open.rs`, `lake/standby.rs`, `conformance/checks/thinned.rs` | 180 | 240 |
| S6 | The refusals at the write doors, the store facade, the catalog and the command | `writes.rs`, `ingest.rs`, `doors.rs`, `api/catalog.rs`, `api/tests/wit.rs`, `cli/commands.rs` | 120 | 150 |
| S7 | The reads' markers at the head and at an earlier position, the reclaim, the measured bytes | `reads.rs`, `lake/described.rs`, `doors.rs`, the engines' `view.rs`, `docs/hosting.md`, the suite's rows | 120 | 170 |
| | **Sum** | | **1 040** | **1 250** |
| | **×1.35** | | **1 404** | **1 688** |

**The raise.** By `docs/budgets.md`'s rule, a round's reserve is sized from
the count measured on the pull request that records it and the estimate
of the round's tasks, rounded up to the next 500. At ×1.35 the plan is
now 1 404 product and 1 688 specification; rounded up to the next 500,
that is the **+1 500 product and +2 000 specification** the operator
took, so the raise stands unopened: the ceilings move from **107 000 to
108 500** and from **189 500 to 191 500**. The epic's 102 500 and 187 000
were reckoned before 098's and 099's reserve raised the ceilings on
2026-11-01; the tree's 107 000 and 189 500 are what this raise lands on.
The operator's word landed on 2026-11-02: `docs/budgets.md` holds the two
rows, and the ceilings and bases rose with them, before S1.

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
  the folds of the act's position, and `describe` counts it; a record whose
  group field moved before the act strands the group it held earlier too,
  and a retired record strands the groups its versions held.
- `THIN_LATE_TICK_FOLDS`: a new record before the horizon in a stranded group
  folds in, and the group stays exact and stranded.
- `THIN_ERASURE_RESTATES_STRANDED_FOLDS`: an erasure selecting a value only
  a stranded group's folds hold restates them — a count loses an erased
  member, a sum drops the erased value, an extreme loses the erased
  holder's entry — and the group stays stranded and exact; `forget_source`
  restates the same.
- `THIN_STRANDED_MAKE_CARRIES`: a declaration that makes a view with
  stranded groups again carries each group's record and order
  entries and notes none of its live records again, so the folds are
  not counted twice; a declaration, migration or setting that changes the
  group key, a measure, the `where` or a member the folds hold is refused
  `StrandedMake`. S4, `conformance/checks/stranded.rs`.
- `THIN_CORRECTION_REFUSED`: a write, an ingest and a retirement at a
  thinned key are each refused, nothing is appended, and the projection is
  unchanged.
- `THIN_AS_OF_SAYS_UNANSWERABLE`: a read at an earlier position omits a
  stranded group and counts it; `get` of it is refused; a group with no
  thinned member at any position computes; a record that moved group and
  one that moved face before the act strand the earlier group and face as
  well, and the earlier read omits them rather than computing short; and a
  view over the view does the same, its refusal naming the inner view.
- `THIN_EXACT_GROUPS_ONLY`: the act refuses a view with a not-exact group
  it would strand, at the act's position or an earlier one.
- `THIN_PARTS_AND_WRITES`: a write sent during the act answers within one
  turn; a write at a key a later part has not yet taken is refused
  `Thinned`; a killed writer's open finishes the act; `verify` passes
  between any two parts.
- `THIN_VERIFY_EXPORT_IMPORT_REBUILD`: the log verifies, an export between
  parts imports, and a rebuild equals the projection.
- `THIN_VIEW_KEPT_AFTER_REFUSED`: every event that would make a view over
  a thinned type kept — a setting, a declaration, a migration's step — is
  refused, and one over a view with stranded groups is not.

One test beside the suite, SQLite, `THIN_BYTES_RETURNED`: the act's reclaim
gives back the thinned rows' bytes, measured on the file, with the
tombstones' bytes named in the same row.

### Decided by the operator

Decided on 2026-11-02 (lakelands#1578): every recommendation below was taken.

1. **A correction into a thinned group is refused**, as above: the record's
   before-values are out of the projection, so it cannot be compared and
   set, and a fold it holds could be contradicted. It is not folded in as a
   new version at the same key.
2. **A stranded group at an earlier position is refused and counted**, as
   above. It is not answered with the head's record and a marker, which
   would answer later folds as if they were earlier.
3. **The log's part is the keys**, as above: the fold deletes exactly what
   the act took, and a rebuild stays a fold. The rule alone — with the fold
   walking the type's events at the act's position, which saves about 20 B a
   record and adds a walk to every rebuild of the lake — was not taken.
4. **A group the act would strand that is not exact is refused**, as above,
   so every stranded group is exact. It is not held flagged and answered
   less than it answered before, which the issue's done-when rules out.
5. **The raise is +1 500 product and +2 000 specification**, to 108 500 and
   191 500, in `docs/budgets.md` by the rule, dated 2026-11-02.

### S3: how the fold finds the groups

The mark S3 owes is every group and face a taken key's versions held. S2
landed none of it, and the operator's five decisions above do not say how
the fold finds them. This section weighs the shapes against the tree and
recommends one **for the operator's word**: it is not yet a rule of this
record, and the plan's rows below are sized for it.

**Measured** at `582e075c` (develop after S2 and the kept view's parts):
the seam stands at 108 of 110 methods — `ProjectionStore` 22,
`ProjectionView` 27 — and the core at 110 882 product of 115 000 and
198 531 specification of 202 500. **Read** there: every engine keeps every
state a key ever held — SQLite's `records` rows under its `keys` id,
PostgreSQL's under `(ty, key, since)`, the memory engine's `History` for
each id — and no seam read answers more than one state.
`ProjectionView::get` takes the state at a position, and `matching`
answers positions of events, not states. The fold's `view` is a committed
snapshot on every engine: the disk engines read it through a separate
reader connection or a read-only snapshot, and the memory engine holds the
transaction's puts until `commit`. `Staged.records` keeps one state for a
key, the latest in the append. A part that folds in the same transaction
as a version's put — a horizon a second old, a log page of 1 024 events
that holds both — cannot answer that version from the view. A correct
shape sees the transaction's states, or the log names the mark.

| Shape | What it reads, and where | Per taken key | Seam | Lines (×1.35) |
|---|---|---|---:|---:|
| 1. A states read | a page of the key's states at or before the part's position; the write side's transaction on SQLite and PostgreSQL, the memory engine's pending puts and history | one seek and one row for each version | 109 | 260 / 230 (351 / 311) |
| 2. A replay | the key's events, from the log, folded again | one index page and one log read for each version, its erasures and sealing applied, the key's migrations found | 108 only with a commit before each part, or 109 | 320 / 260 (432 / 351) |
| 3. The parts name the groups | the same states, read by the act's turn; the fold reads the event | the act's read once; the log carries the groups | 109 | 340 / 280 across S1, S3 and S5 (459 / 378) |

**1 — a states read of a key.** One method answers a page of a key's
states at or before `through`, oldest first, after the last position it
handed back; the fold loops, and for each state asks every kept view
reading the type for its member, by `kept::member`'s own rule, and marks
the group on the face it stood. It is the read this record's first shape
names, with one correction the tree forces: it belongs on
`ProjectionStore`, beside `drop_states`, not on `ProjectionView`. The
view is a committed snapshot, and a part folding in the transaction that
holds a version's put cannot see it there. The write side answers the
transaction: SQLite and PostgreSQL query the open connection, and the
memory engine reads its pending puts over the history. A `ProjectionView`
read would need a commit before each part — `fold_all` commits inside a
page only when a flush kept a step — and a year at a part of 500 keys is
63 000 parts (reckoned), 126 000 for the issue's two years. The read
costs one seek and one row for each version: a tick's state is 619 B of
the 698 B its projection holds (098), so a year of 31.5M taken ticks
reads 31.5M rows, about 19.5 GB (reckoned) — the rows the part's
`drop_states` then deletes and, in a rebuild, the rows the fold itself
has just put. The log is not read again. The mark uses the view's
declaration at each version's position, which the projection keeps, so a
view made again between a version and the act still has the earlier
group marked and an earlier read finds it; that is one declarations read
for each distinct position, not one for each version. S3 owes it, and it
adds no read of a record.

**2 — a replay of the key's events.** The fold asks the feed's index for
the key's positions (`matching`, `About { ty, key }`) and reads those
events from the log, then folds them again. A record's event is a patch
over the state before it, and the state it makes carries the fields the
fold derived at its position, so the replay is a second fold of the key's
history, from its first state, under the declarations and settings of
each position. It is unsound in three places the tree names. `matching`
is a `ProjectionView` read, so the index does not hold an event marked
earlier in the same transaction, and the replay needs the commit shape 1
avoids or a write-side index read — it does not avoid the seam. `about`
names the type alone for `Migrated` and `ValueTypeMigrated`
(`core/src/feed.rs`), so a type's events that moved a record's fields or
its key are not in the key's positions; the replay must walk the type's
events from the key's first position, which is the walk the operator's
decision 3 refused. An erasure restates the stored states (065), and
`forget_source` destroys the keys a seal was made with (`sealing.rs`), so
a replay either re-applies 065's redaction or answers a value the live
fold erased, and it cannot unseal what the forgetting destroyed. Its cost
is one index page and one log read for each version, the event's whole
bytes decoded, and the record's fold run again; a rebuild re-reads events
its pages have already passed. It is the only shape that could leave the
seam at 108, and it is neither sound nor cheap.

**3 — the parts name the groups.** The act's first turn and every part
compute the groups under the writer — the same states read as shape 1,
called by the act rather than the fold — and the part's event carries
each key's groups and faces; the fold reads them off the event and marks.
A rebuild then reads no states. What it costs: the read is still owed
(the act is under the writer and must see the versions), so the seam
still makes 109; S1's kind gains a body field, and its WIT mirror, its
refusals' row and `docs/log-format.md` reopen; S5's turn and parts
compute and carry the groups; and the log grows by them — a part of 500
ticks inside one hour holds one or two groups on two faces, tens of
bytes, while a part whose keys each held a group of their own holds 500,
about 20 KB (reckoned). It also writes kept-view naming — `sys:kept.<view>`
and the group key — into the log, where a rebuild of another binary must
read it, and it gives the mark two callers that must agree. Its reward is
the rebuild's states read, which S7 measures; if that measure is too
high, this is the shape to bring back, with the log's bytes as its price.

**Decided by the operator 2026-11-04: shape 1, the states read on
`ProjectionStore`.** It is the only shape that answers the transaction's
states without a forced commit and without reading the log again; it
computes the mark once, with the code that already notes a member, from
the states the part deletes; it keeps the log as the operator decided it
(the part names the keys, and the fold derives the mark); and it makes
the live fold and a rebuild read the same rows and the same rule. It
spends the plan's only new seam method — S5, S6 and S7's rows name none —
and the seam keeps one spare, 109 of 110. The read is named `states`;
`docs/seams.md` gains its row and the `put_record` row's "neither reads
the key's other states" is narrowed to the reads of a record, not the
act's; `scripts/release/surface-accepted` gains `ProjectionStore::states`
for the next release (056).

**What `verify`, a rebuild, an export and an import must do.** `verify`
reads the log and is unchanged by every shape: the parts are events, the
chain is the appender's, and nothing here reads a state. A rebuild folds
the parts; at each, shape 1 reads the states from the projection the fold
is making — the write side's transaction, or the last commit — marks, and
deletes them, so it leaves the rows and the mark the live append left.
Shape 2's rebuild re-reads events its pages have passed; shape 3's reads
the marks off the parts and no states. An export is the log, unchanged,
and the parts travel as events; the marks are not in it, as the kept
records are not. An import's writer finishes an owed act (099) and reads
the states it has put exactly as a rebuild does; shape 1 answers the
import's own transaction, so a part imported beside its versions is
marked the same. The suite's `THIN_VERIFY_EXPORT_IMPORT_REBUILD` holds it.

**What it means for S4 to S7.** S4 is unchanged: the make carries the
stranded groups' records off the view it ends, which are kept-name
records read through `get` and `scan`, and adds no seam method; its row
stays 100 and 160. S5 is unchanged in shape: the act selects its keys and
appends its parts, and the mark is the fold's; its preflight's counts are
unchanged. S6 is unchanged: the write doors refuse by the tombstone. S7
is unchanged: it reads the markers and the kept state through the reads
that exist, and its act timing and `docs/hosting.md` measure now include
the fold's states read, which is the act's own cost. Shape 3 would move
the group computation into S5 and reopen S1; shape 2 would put the replay
into S3 and leave S4 to S7 as they are.

**The plan's rows.** S3 now carries the read, the mark and the fold's
arm; S4 stays as it was. At `582e075c` the room is 4 118 product and
3 969 specification (measured); the plan's remaining steps, S3 to S7, are
780 product and 950 specification raw, 1 053 and 1 283 at ×1.35, and fit.

| Step | What | Files | Product | Spec. |
|---|---|---|---:|---:|
| S3 | The stranded mark, the counter, the fold's arm, the read of a key's states | `engine.rs`, the engines' `handle.rs` and `projection.rs`, `kept.rs`, `lake/fold.rs`, `lake/scrub.rs`, `conformance/engine/projection.rs`, `conformance/checks/forgotten.rs`, `store/tests/probed/mod.rs`, `store/tests/fold_gap.rs`, `docs/seams.md`, `scripts/release/surface-accepted` | 260 | 230 |
| S4 | As before: the make of a view with stranded groups, the order entries, the checks | `kept.rs`, `lake/fold.rs`, `conformance/checks/stranded.rs` | 100 | 160 |

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

- **Not erasure.** The log keeps every value the act thinned, and a
  rebuild folds the act again: the projection it makes has the same
  records gone, and `get` of the key answers `Thinned`, naming the
  horizon. The value is answered by no read of the projection, only by a
  read of the log itself. A host that needs the value erased uses
  `remove_values`, `forget_source` or `purge_source`, as 065, 058 and 039
  decide them; this act does not stand in for them.
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

## As built

**S1 (lakelands#1695), the kind, the public types and the refusals.**
Nothing performs the act: a `values_thinned` part is declared, folded to
its mark and changed nowhere else.

- **The kind joins format 23's list last**, after `quarantine_closed`, so
  that no kind's stored code moves. 1686's `source_purged` and
  `act_whole` merged first and took 25 and 26; `values_thinned` takes 27
  and sits last of `kinds!`. Its variant sits beside `ValuesRemoved` in
  `EventBody`, where it is read. `LOG_FORMAT_VERSION` and
  `OLDEST_LOG_FORMAT` stay 23.
- **The public types**, as this record names them: the selection `Thin`,
  `ThinReport`, `ThinnedAct`, `stranded` on `Record` and `Group`,
  `thinned` and `stranded` on both `Found` variants, and on
  `TypeDescription`. Each new member is omitted from the wire where it
  is empty or false, so a page or a description with nothing to say
  reads as it did before. `ThinnedAct` puts its type on the wire as
  `type`, as every other public type does; its Rust member stays `ty`.
- **The six refusals** are in the catalog with their ids and sentences,
  and `THIN_REFUSALS` is the suite's row for them: every engine runs it,
  and it fails when an id, a sentence or a name the sentence says
  drifts. The act's own rows are S5's, S6's and S7's.
- **`docs/log-format.md` states the kind now**, though the build plan
  gives the page to S2: `api/tests/contract.rs` holds that every kind in
  `KINDS` is on the page, so S1 cannot be green without it. The page says
  the two tables landed at S2 and the fold's effect lands with the act,
  at S3 to S5.
- **Corrections where the tree holds a rule the sketch did not name:**
  `StrandedAtHeadOnly` takes `position`, not `at`, because the refusals
  catalog reserves `at` for a `Place`'s position and refuses it as a
  case's parameter; `ThinnedAct`'s wire name is `type`; and the six
  refusals share one suite row, S1's, because the issue's "a conformance
  row" is read as one row that holds all six — it checks each id, each
  sentence and every name a sentence says, so a drift in any fails it.
- **Every file outside S1's plan list the tree forced, and why:**
  `api/src/describe.rs` carries `TypeDescription`'s two members, which
  this record's public-types bullet names — S2 is the engines' stored
  tables, not the description's shape; `api/tests/thin.rs` and
  `conformance/checks/thin.rs` are the wire tests and the suite's row;
  `core/src/engines.rs` holds `KIND_CODES`, the format's stored code
  table, where the kind joins last, as 27; `core/src/describe.rs` builds
  the description, and now names the two members field by field rather
  than letting `Default` absorb a member a later step adds;
  `core/src/expr/aggregate.rs` and `core/src/rights.rs` fill `Group` and
  `Record` with their new member; `store/src/reads/find.rs` builds the
  pages, which carry no acts and no stranded count yet; `store/src/lake.rs`
  (`ends_a_range`, `reads_a_record`) and `store/src/lake/fold.rs` (one
  no-op arm) are exhaustive matches over `EventBody`, where the kind
  changes no shape, a fold of it reads the records it takes, and the
  act's fold is S3's; `wit/types.wit` and `api/tests/wit.rs` are the
  mirror, with new checks for `records-page` and `groups-page`; the
  rest patterns in the suite and in the store's and cli's tests follow
  from the new members; `docs/log-format.md` is required by the contract
  test, above.
- **Measured**, at the branch head with 1686 merged: product 100 925 of
  the 108 500 base and specification 185 175 of the 191 500 base; seam
  94 of 110; 6 of 20 `cfg(feature)` sites; 1 011 checks through the
  doors and 87 through the engine seam; 1 018 refusal cases. S1's own
  lines: product +184 −4, specification +376 −25; the plan's 160 and
  180 were the sketch, and the row's extra is the wire tests and the
  suite row. The split is the gate's six-crate one;
  `crates/lakelands-cli/tests/walk_1.rs`'s one line each way is outside
  the specification budget.
- **S2's as built.** The two tables are in SQLite's and PostgreSQL's
  schema and their lists of tables, their `LAYOUT`s at 11 and 13, and the
  memory engine's two maps; the four seam methods are in `engine.rs`, the
  release's accepted-surface list and `docs/seams.md`; the engine seam's
  row is `THINNED_TABLES`, green on the memory and SQLite engines, and
  run on PostgreSQL in CI. Both tables are read at the head in every
  read. The tree forced files the plan's list did not name: every struct
  that wraps an engine behind `ProjectionStore` or `ProjectionView` — the
  store's `Scoped`, `ScopedView` and `HeldNow`, and the suite's doubles —
  had to answer the four methods by forwarding them, since a wrapper that
  stopped forwarding would answer a read short. Nothing performs the act
  yet: the fold meets `values_thinned` and does nothing. One limit the
  tables leave: two acts on one type, field and horizon are one
  `(ty, by, before)` row, so the second's `gone` replaces the first's,
  and `ThinnedAct` exposes no position to tell them apart. The decided
  shape cannot distinguish them either; this is noted, not repaired.
- **Measured at S2's head**: product 101 668 of the 108 500 base and
  specification 186 081 of the 191 500 base; seam 102 of 110; 6 of 20
  `cfg(feature)` sites; 1 012 checks through the doors and 89 through the
  engine seam; 1 018 refusal cases. S2's own lines: product +390 −10,
  specification +128 −1; the plan's 100 and 120 were the sketch, and the
  extra is the wrappers' forwarding and the seam's row. Nothing was cut.
  Merging `develop` after S2 landed raised both `LAYOUT`s by one again,
  to 11 and 13, since the order's tables landed there from their own
  step; the two additions each took the number after the one they found.
- **S3 and S4 (lakelands#1639), shape 1 as decided.** The seam's read is
  `ProjectionStore::states`, method 109 of 110, on all three engines,
  answering the transaction's own puts (PostgreSQL writes its buffer
  first); the store's wrappers forward it, and a scoped refold answers a
  record it does not hide with none, as it drops none. The fold's arm
  reads each taken key's states 256 at a time through the part's
  position, marks every group and face a state was a member of in each
  kept view of its type, then drops the states, removes the key's
  vectors and puts its tombstone. Edges the plan did not name:
  - **A state is judged by the declarations in force at its position,**
    read from the projection's committed ontology there; a state the
    transaction itself put is judged by the current one. The settings a
    view's `where` reads are the current ones.
  - **The tombstone's and the row's position is the part's.** The fold
    holds no marker, so a tombstone carries the position of the part that
    took its key, and the act's row keeps its first part's; `gone` adds
    up across the parts of one `(type, field, horizon)`.
  - **A group left with no member on either face** — its last member
    moved or retired before the act — keeps a record that holds its mark
    and nothing a reader is shown, so the mark survives and counts, and
    no read answers the group.
  - **A view over a view** marks its own group stranded where the inner
    record it folds is, so the outer's count includes it.
  - **The make carries.** A make that ends a view with stranded groups
    takes their records and their order entries into the view it makes,
    and leaves their live members unnoted, so no member is counted twice.
    A declaration, a migration or a setting that would make it under
    another rule is refused `stranded_make` before it is appended. The
    test is conservative: any change to the view's own declaration, to a
    field the view reads as its source declares it, a step that moves
    one, or a setting it or a view it reads names, refuses, whether or
    not it reaches a stranded group.
  - **Which rule marks.** A state is asked by the declarations in force
    at its position, read once for each distinct position a thinning
    meets (S7 measures that cost), and by the ones in force now unless
    it was superseded before the view was last made. A fold of the log
    commits before a thinning, so a rebuild reads those declarations
    committed as the live fold did; a state put inside the thinning's
    own append, where none are committed yet, is asked by the ones now. A make by a setting
    is not told apart from one by a declaration, so a state live at the
    last declaration is marked by the rule now even where that make did
    not fold it: the mark may name a group the state never stood in,
    which is then exact all the same.
  - The suite's rows are `THIN_GROUP_STRANDED` (the fold's side; the
    read's markers are S7's), `THIN_ERASURE_RESTATES_STRANDED_FOLDS` and
    `THIN_STRANDED_MAKE_CARRIES`, and the engine seam's `A_KEYS_STATES`,
    each green on the memory, SQLite and PostgreSQL engines. Each row
    appends the act's part below the store, since the act itself is S5's.
- **Measured at S3 and S4's head**: product 112 299 and specification
  199 533 against 111 694 and 199 058 at its base, so S3 and S4 add
  605 product and 475 specification (raw lines +638 −33 and +475 −0);
  seam 109 of 110; 6 of 20 `cfg(feature)` sites; 1 080 checks through
  the doors and 103 through the engine seam. The plan's 360 and 390 were
  the sketch; the extra is the make's refusal at three doors, the
  bare group's record and the view-over-view mark. Nothing was cut: the
  four items of § Cut first stay, and the ceilings hold.

## How it came to be

- lakelands#1639 — S3's shape weighed and recommended: a read of a key's
  states on the write side, beside `drop_states`; the S3 and S4 rows and
  the plan's totals restated. No code. The operator took shape 1 on
  2026-11-04, and S3 and S4 landed together under the same issue.
