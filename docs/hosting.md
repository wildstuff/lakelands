# Hosting Lakelands in a platform

This page is for a platform that embeds Lakelands as its store: it
supplies identities, runs agents and workers, talks to the outside world,
and wants the lakes underneath to be Lakelands. It describes the same
seams that `lakelands-api` declares and `docs/seams.md` specifies, seen
from the host's side. If this page and the contract ever disagree, the
contract is right and this page is wrong.

## What a host implements

Lakelands has seven seams. A host implements the ones it needs and
passes them in `Seams`; every one but the engine has a default, and for
the engine a host passes Lakelands' own openers. A host that passes
those and nothing else has a working store. Two seams point the other
way: the doors hold the `PrincipalResolver`, and the store provides the
`ChangeFeed` for the host to read.

- **Engine** (`LakeEngine`, `ProjectionStore`, `ProjectionView`, with
  `VectorIndex` beside it as the engine's index trait). Usually nothing
  to implement: SQLite and in-memory engines ship with `0.1.0`, and
  PostgreSQL with `0.2.0`, a lake per schema of a database the host
  runs (`docs/decisions/038-a-lake-in-postgresql.md`). A new engine is
  accepted when it
  passes the conformance suite.

  A PostgreSQL lake's location file (`lakes/<id>.json` in a root; `lake
  new` does not write one for PostgreSQL yet):

  ```json
  {
    "lake": "finance",
    "engine": {
      "engine": "postgres",
      "url": "postgres://lakelands:secret@db.example:5432/lakes",
      "schema": "lakelands_finance",
      "tls": { "mode": "require", "ca_file": "/etc/ssl/certs/ca-certificates.crt" },
      "tuning": { "connect_timeout_secs": 10, "statement_timeout_secs": 0 }
    }
  }
  ```

  `schema`, `tls` and `tuning` may be left out: the schema is then
  `lakelands_<lake>` and TLS is `require`. With `require` and no
  `ca_file`, the engine looks for the system's certificate bundle at the
  well-known Unix paths; on Windows there is none, so opening the lake
  fails as `unavailable` until `ca_file` names one.

  A process serving a SQLite lake shows up to the tuning's `mmap_mb`
  (256 MiB unless set; `0` turns it off) more resident memory once it
  has read that much of the projection: file pages mapped for reads,
  which the operating system reclaims under pressure, not heap. A host
  that watches the process's heap reads `Anonymous` in
  `/proc/<pid>/smaps_rollup`, not its resident size.
- **`PrincipalResolver`.** Maps the host's identities, roles and
  policies onto one `Principal { kind, id, lake, grants }` per call.
  Grants are per field and per principal, and the store enforces them
  on every read and write, so the host never gates reads or writes
  itself. See `docs/decisions/006-grants-not-roles.md`.
- **`FunctionBacking`.** A backing kind is an open token. `expr`, the
  built-in evaluator, is always there. A host binds any other kind in
  `Seams::backings`: its own, or `http` and `exec` from
  `lakelands-backings`. The CLI and the server bind those two from the
  `backings` section of the root's `lakelands.json`, which names the
  hosts `http` may call and the commands `exec` may run. A function of a
  kind the host did not bind is refused as `Unbound`, naming the kind,
  when it is declared. The store calls a backing after the write that
  made a field due, and records each output in the log
  (`output_recorded`). A replay reads it back and never calls again. See
  `docs/decisions/022-functions-are-declarations.md` and
  `docs/seams.md` §`FunctionBacking`.
- **`EmbedBackend`.** A host may pass vectors in with its writes or
  plug in an embedding client: its own, or the one in
  `lakelands-backings` for any `/v1/embeddings` endpoint, which the CLI
  and the server bind from the `embed` section of the root's
  `lakelands.json`. A bound backend embeds the query text of a hybrid
  search that brings no vector of its own, which is a read and records
  nothing. Without one, search is lexical and over caller-supplied
  vectors, and says so. A bound backend that fails makes the search
  `Unavailable`; it never falls back to lexical. A vector field declared
  with `embed` is the embedding of text fields of its record: the store
  embeds the text after the write, records each vector in the log
  (`output_recorded`) and reads it back on replay, one call per text and
  model. Such a field is declared only where the bound backend names its
  model and dimensions, unless the type in force holds it unchanged. A
  backend that comes up after the open is bound then with
  `LakeStore::bind_embed`. Change the model or the fields it is made from by
  declaring the type again. Every live record is then embedded again, and
  a declaration also retries recorded failures. See `docs/seams.md`
  §`EmbedBackend`.
- **`VerbExecutor`.** A verb declared `low` runs at once; any other
  lands as `WriteProposed`, and the store records `accept` and `reject`
  as events. A verb that writes authored fields runs in the store; a
  verb that reaches outside the lake runs through the executor the host
  bound for it, with the lake and the position that made it due as the
  dedup key. The executor gets the record unmasked; a host that wants it
  to see only what the caller may see applies the caller's grants there. The executor's answer is recorded in the log; a failure is
  recorded as `{"error": …}`, the write that made the verb due stands,
  and the caller gets the recorded answer either way. Without a bound
  executor such a verb is refused as `Unbound`, not faked. See
  `docs/decisions/013-verbs-run-by-declared-risk.md`.
- **`ChangeFeed`.** The host reads the store's feed: the log, filtered
  by kind and type, from a position; each poll answers at once with the
  position to poll from next. A poll examines at most `FEED_EXAMINED`
  (4 096) events, so it may answer none short of the head: the host is
  caught up when `next` is past `head`, not when a batch is empty. It
  acknowledges on its own side. The host keeps the
  position it has read up to; the store keeps no consumer state. A relay onto a message bus is the
  host's code, never Lakelands'; the dedup key of a change is
  `{lake}:{position}`. For webhooks, `lakelands serve` has a consumer of
  its own: the `deliver` section of the root's `lakelands.json` names
  endpoints, and the process that writes a lake posts its fired triggers
  and object-set changes to them, in order, signed and retried, keeping
  its position and journal under `deliveries/` in the root (`docs/seams.md`
  §`ChangeFeed`). The `mirror` section names connections that copy types
  of one lake of the root into another; the process that writes the
  target lake follows the source's feed and ingests what its principal
  may read, one source per type, each field carrying the source lake and
  position it reflects (`upstream`). It keeps each connection's
  position in the target's `sys.mirror` records, which its principal
  needs the write grant on; `find sys.mirror` shows where every
  connection stands and why a type stopped (`docs/seams.md`, the
  mirror). An agent on MCP is told of changes without polling:
  every session receives `notifications/tools/list_changed` when the
  lake's declarations change. After the `subscribe` tool, it also
  receives `notifications/lakelands/changed` for each change of a record
  its principal may read. Without a list of types, the types are the ones
  it may read when each change happens, so a type declared later is
  included; a list given to `subscribe` is fixed. `keys` narrows it to
  those records, of the types given or of any type, and is fixed the
  same way; subscribing again with other keys tells at most one change
  of the old ones that was already on its way. A notification names
  the position, kind, type, key and the changed fields it is shown, never
  a value. A trigger's firing or an object set's change is told only to
  a principal who reads every field of the type in full, since its
  condition may read any of them. That rule sends less than one that
  asks only for the fields the condition reads, never more; a
  condition-exact rule may follow. On stdio these come on the same
  stream. On streamable HTTP they come on the session's event stream, a
  `GET` under its own key, which resumes after `Last-Event-ID`. A stdio
  session answers one request after another: requests sent while one is
  served wait their turn, so a client that needs requests served side by
  side takes the HTTP door. On a heavily loaded machine a stdio request
  can therefore wait far longer than its own work: behind the requests
  queued before it, each behind the writes of the other doors to the
  same lake (the load run saw more than 60 s at a load of 40 on 12
  cores, and under 11 s at a load under 10). A client that does not
  read makes its session lag; nothing is dropped. Delivery is at least once: when a
  session subscribes again, at most one change already on its way may be
  told twice, and each carries its position to deduplicate by. This is a
  limit, not a promise of exactly once: the change in flight when the
  subscription is replaced was read under the old one, and is not
  withdrawn. See `docs/decisions/004-the-change-feed-is-the-log.md`.
- **`SystemVocabulary`.** The host registers its own `sys.*` types and
  says which lakes may hold them; each time a writer opens a lake, the
  store declares the allowed types it does not hold yet.
  Lakelands reserves only the types its own fold reads; today those are
  `sys.setting` and `sys.aging_rule`, and the other two are reserved
  ahead of their readers. See `docs/decisions/003-reservation-by-namespace.md`.
  A value that must be kept from a reader is not put into a setting:
  any reader's expression reads `setting('<key>')`, whatever its grant
  on `sys.setting` and whether or not the setting is restricted.
  The aging rules run when the host calls `LakeStore::age(today)`, once
  a day, with the date (`docs/decisions/036-aging-runs-by-date-once.md`);
  `lakelands age --today <YYYY-MM-DD>` is the same call from the CLI, and
  `lakelands serve` makes it daily when the root's `lakelands.json` has an
  `aging` section: the date at its `utc_offset`, once at start and at each
  rollover (`docs/seams.md` §aging).
  A source that should be taken back is purged with
  `LakeStore::purge_source(source, None, None)`
  (`lakelands purge --source <id>`):
  every record it brought in is retired in one append, and a record
  another hand also wrote refuses the purge
  (`docs/decisions/039-a-source-is-retracted-as-one.md`). The report
  names, per relation, the live records outside the purge that still
  refer to a record it took. A host that must know that before it acts
  asks first: `LakeStore::purge_preflight(source, None)`
  (`lakelands purge --source <id> --preflight`) appends nothing and
  answers the same report under an id, and `purge_source(source, None,
  Some(id))` (`--report <id>`) purges only if the lake has not moved
  since. Both take one type in place of `None` (`--type <type>`): the
  purge is then of what the source brought into that type and the links
  of its relations, a co-owned record elsewhere does not hold it, the
  report names the source's records in other types that still refer
  into what went, and the id is of that purge alone. A view, and a type
  that keeps no per-field provenance, are refused there.
  Rows that one act should remove, such as samples seeded beside a
  lake's own data, are written through `ingest` under a source id of
  their own, into fields declared `shared` (lakelands#611). An authored
  field refuses an ingest, and no write option, idempotency key or
  principal puts a source on an authored write. An authored field
  becomes `shared` by a declaration, with no migration; the values
  written before stay authored and outside the purge. `shared` means the
  later position wins: a source that delivers a record again after an
  author changed it writes over the change, so a host that lets people
  edit seeded rows stops feeding them. An author's later write makes the
  record co-owned while a value from the source remains in it, and the
  purge is refused whole; once none remains, the record is the author's
  and the purge leaves it.
  A lake's log is exported with `lakelands_store::export::write(store,
  dir)` (`lakelands export --to <dir>`) as files that verify without
  Lakelands (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`);
  an export is the whole log, unmasked, so it is handed over as the
  lake itself would be. An export only reads: it needs no writer, appends
  nothing, and runs beside a `lakelands serve` that holds the lake.
  Opening a lake on SQLite reads the end of each log segment's index,
  not the index: what an open costs follows the number of segments, one
  more for each `checkpoint` that sealed one, not the events the lake
  holds, so a host may open every lake at its start. A damaged index line
  is therefore not found at the open; `verify`, which reads the log,
  finds it, as does the read that meets it. A writer's `close` truncates
  the projection's journal (`projection.db-wal`); a writer that ends
  without it leaves the journal as large as it grew, for the next
  `close` or `compact` (lakelands#564). A write the disk refuses, as when
  it is full, is answered `unavailable` and leaves the log's files as
  they were before it: the host makes room and the lake's next write goes
  on, on the same handle. One such answer says that its outcome is not
  known: the disk took the write and did not confirm it, and its events
  are in the log (lakelands#618). The handle's log then holds them and
  the store's writer does not, so the next write on that handle is
  refused as not continuing the log, and each one after it: the host
  opens the lake again, and the handle opened then reads them and writes
  on. A `checkpoint` closes the segment and begins the next one with its
  checkpoint in one step, on every engine: one refused before that step
  has sealed nothing, the lake directory verifies meanwhile as before it,
  and the host sends it again when the cause is gone. An error can also
  come behind the step, with the seal standing: the SQLite engine's says
  so (the new segment's name could not be synced and the outcome is not
  known, or the handle could not read on), and a fold that fails behind
  the seal answers its own. Then the host opens the lake again and sends
  nothing again: the handle opened then reads the checkpoint where it is
  in the log (lakelands#623).
  In a quiet hour the host can call `LakeStore::compact()` (`lakelands
  compact`) to give back the storage the projection no longer uses; it
  changes no answer and appends nothing, and writes wait while it runs
  (`docs/decisions/040-maintenance-never-changes-an-answer.md`). On
  PostgreSQL, `VACUUM` skips, with a warning, a table the connecting role
  does not own, and the compact reclaims nothing there; connect as the
  role that owns the lake's schema.
  After a bulk load into PostgreSQL the engine analyses its tables
  itself, once every 5 000 records a writer puts, so the planner knows
  their size; a host that loads a lake's schema by another path runs
  `ANALYZE` on it itself.
  A vector field the lake asks an index for (`vector_index.<type>`,
  `docs/decisions/043-an-index-asked-for-answers-its-set.md`) is indexed on
  PostgreSQL by **pgvector 0.8 or later** (the PostgreSQL License), a
  server extension the host installs. A writer's open runs `CREATE
  EXTENSION IF NOT EXISTS vector`, which needs a role allowed to create
  it; where it cannot, the lake opens as always, its searches read every
  vector and answer exactly, and the open says so once on standard
  error. An indexed field costs, beside
  its vectors, a unit vector and an index entry per vector: measured at
  100 000 vectors of 1 536 dimensions, about 8.3 KiB for the unit vector
  (kept out of line) and 8 KiB for the index, one page per vector. The
  index is built in the transaction of the setting's write, which holds
  the lake's writer until it is done: 7 minutes for that field at
  PostgreSQL's default `maintenance_work_mem` of 64 MiB on a loaded
  12-core machine, since the graph stops fitting in memory after about
  9 000 vectors; a host that indexes a large field raises
  `maintenance_work_mem` for the lake's role first. PostgreSQL builds a
  large index with parallel workers in shared memory: a server in a
  container needs a `/dev/shm` larger than Docker's default 64 MiB
  (`--shm-size`), or `max_parallel_maintenance_workers = 0`. Otherwise
  the build fails as unavailable ("could not resize shared memory
  segment") after the setting's event is already in the log: the writer
  is behind and appends nothing, and every writer's open folds that event
  again and fails the same way. Reads answer as before; writes do not,
  until the host raises `/dev/shm` or `maintenance_work_mem`, or sets
  `max_parallel_maintenance_workers = 0`, and opens the writer again. SQLite and in-memory lakes keep no such
  index and answer every search exactly; a SQLite lake that needs one
  moves to PostgreSQL by export and import (lakelands#215).

**Moving a lake to another engine.** An installation runs one engine,
and a lake is not bound to the one it was created on: `lakelands export`
writes its log as files, and `lakelands_store::import` opens those files
as the same lake on any engine
(`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`):
every event unchanged at its position, the projection folded, nothing
appended. A SQLite lake that needs PostgreSQL's multi-row commits or its
vector index moves so. Walk 3 (`docs/walks/walk-3.md`) moves a lake
into a second root and compares the two feeds. From the CLI:

```sh
lakelands --root old export --to /tmp/finance
lakelands init new
# write new/lakes/finance.json: the PostgreSQL location shown above
lakelands --root new import /tmp/finance
```

`lakelands import <dir>` fills the root's lake of the export's own name,
read from the export's first event: a lake's name is inside every hash,
so an import cannot rename it, and `--lake`, when given, must name the
same lake. The lake's location must exist and the lake must hold
nothing: `lakelands lake new <id>` writes a SQLite location and opens
nothing, and a PostgreSQL location is written by hand. A lake kept in
memory is refused, since it would end with the command's process. The
command prints the lake, how many events and files it imported, the
head and the head's hash, which is the last line of the old lake's
`lakelands feed`; a failure exits 1 with the store's reason. The import verifies the whole export before it
writes. An import that fails after it began writing names the position
it reached and the file; the target then holds a true prefix of the
export, which is not the lake that was exported, and it refuses another
import, naming its head. Remove it (its directory, or its schema) and
import again. An import takes about as long as a rebuild of the lake,
since it verifies the files and then folds them: the streaming check's
long run imported 257 606 events (100 000 records) in 40 s on the memory
engine, 136 s on SQLite and 335 s on PostgreSQL, holding at most 23 MiB
of heap, on a 12-core machine at a load of 17 to 31.

**Configuration is not a seam.** Engine configuration, tuning, limits
and an optional key handle reach a lake through `Location`, and a
server through its server configuration; Lakelands never reads the
environment. A host that keeps settings in its environment reads them
and passes them in. See `docs/decisions/005-no-environment-reads.md`.

## How a host switches onto Lakelands

A host that already has a store of its own moves in four steps.

1. **Freeze the old store.** From this point it takes bug fixes only,
   and every fix is logged so it can be ported to Lakelands before the
   switch.
2. **Implement the seams against a tagged `lakelands-api`** while the
   old store still serves. Each seam implementation gets a test that
   fails before it and passes after. The pure helpers a host needs
   beside the seams are there too: `schema_hash` in
   `lakelands_api::shape`, the key rule with `link_key` and
   `key_components` in `lakelands_api::key`, and the canonical bytes in
   `lakelands_api::canonical`; `lakelands-core` is not a surface a host
   builds on.
3. **Cut over in one step.** Consumers re-point to Lakelands, pinned by
   tag, never by bare revision. Any contract the host exposes to its own
   plugins is adapted onto `lakelands-api`. The old store and the
   duplicate doors built around it are deleted, not wrapped in shims.
   The port log is empty before this step starts.
4. **Walk the host's own end-to-end scenario** on the new tag before
   calling the switch done. Behaviour the conformance suite did not
   cover shows up here, and it is fixed in Lakelands, not worked around
   in the host.

## Rules the host inherits

- **One tool surface.** The catalog is the only tool surface. A host
  adds no second query door beside it and no hand-written tool names.
- **Invariants live in the store.** Field rights, per-field provenance
  and the `sys.` guard are enforced on every write by the store, so a
  host-side check is never a substitute and never needed. See
  `docs/decisions/010-provenance-per-field.md` and
  `docs/decisions/012-writes-touch-only-their-fields.md`.
- **`schema` is for a principal that reads everything it may declare
  over.** A principal with `schema` is shown every definition, and a
  write answers `through`, the position its consequences reach. The lake
  refuses a declaration that reads a field its author may not read, and
  names a trigger or an object set in a write's answer only to one who
  reads what its condition reads; it cannot hide from such a principal
  that something reacted to its own write. So a host does not hand
  `schema` to a principal it keeps from a field or a type. See
  `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
  extended.
- **A schema hash is that of the shape its reader is shown.** A principal
  kept from a field is answered, by `describe` and in a migration's
  report, the hash of the type without that field, not the stored one,
  and so is a host that declares as such a principal. Nothing breaks: a
  declaration identical to the one in force appends nothing. A hash
  compares only between readers shown the same shape; the host's own
  comparison before it applies describes as one shown everything, and
  the log and the feed carry the whole hash. A report's id is made over
  the report as its reader is shown it. See
  `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
  extended (lakelands#644).
- **A grant by name holds a field through its renames.** A field's
  access is the lowest of what a principal's grants say under every name
  the field has borne. A restriction follows the field, at the head and
  in a read as of before the rename; a permission stays with its name.
  So a host that does nothing stays closed, and a host that wants a
  renamed field read or written by a principal names the new name and
  drops the restriction on the old. A migration's answer names what it
  renamed (`renamed`), and the feed's `migrated` event carries it. Two
  fields that have borne one name (after a swap, or when a name is taken
  again) are restricted together by it; grant by a name only one of them
  has borne. A lake made before this finds its renames at its next open
  from the declarations it keeps, not from its log; until a writer has
  opened it once, each read-only open finds them again. See
  `docs/decisions/006-grants-not-roles.md`, extended.
- **A grant may admit only some records.** `only` on a type, pairs of a
  text or reference field the principal may read and a text value,
  admits the records that hold every pair at every read door, and
  refuses that principal every write that reaches the type and every
  change of shape: its writes go through the host. `only` is a function
  of `Principal.id` within a lake: give each owner its own id, and never
  give one id `only` on some calls and not on others, since a retry is
  answered its first outcome before the grants are asked. A grant whose
  pairs cannot be applied refuses the reads that name the type until it
  is fixed. See `docs/decisions/051-a-grant-may-admit-only-some-records.md`.
- **What a host reads before it applies a migration.** The preflight's
  report says, each in one member: whether the apply is admitted
  (`lossless`: every value the migration keeps converts, and every
  record a re-key moves takes a key of its own; it says nothing of what
  the migration removes); what goes (`dropped`: each removed field with
  the records that hold a value, each removed relation with its live
  links; the apply discards them, so a host that asks "is anything
  lost?" sums the counts, `MigrationReport::discards()` in Rust); what
  moves (`rekeyed`, `references`, `sample_collisions`); a relation's
  rename (`relations`, `carried`, the entry below); and every other
  member `to` states otherwise (`restated`, the entry after it). A
  removal is the migration's intent and is consented to by repeating the
  report's id, so `lossless: true` with a count above zero in `dropped`
  is an apply that is admitted and discards values. See
  `docs/decisions/027-shape-changes-are-declared-or-migrated.md`.
- **A migration may rename a relation, and the relation keeps its
  edges.** `relation_renames` pairs an old name with a new one. A
  relation carried by a `via` field or derived from matching values is
  only named anew. One carried by link records moves them: its links
  become records of `sys.link.<type>.<new>`, with their versions, fields
  and provenance, and a read as of before answers them under the old
  name. The preflight's report lists each renamed relation with the
  links that move (`relations`), and what the lake declares again
  because it names the old link type (`carried`): a trigger or a verb on
  it, an object set of it, a view that reads it. A rename changes no
  link field and re-keys nothing; those are migrations of their own, and
  a rename is refused while a reference field or a relation of another
  type names the link type, or while a set, a trigger or a verb reads a
  field of the new name's standing link type that the relation lacks. A
  consumer of the feed that follows the old or the new link
  type is handed the `migrated` event. The old link type stays declared,
  for the reads as of before: it holds no record and takes none, so a
  source that still sends links under the old name is refused, naming it.
- **A migration's `to` is the type's whole declaration; read `restated`
  before you apply.** A `to` built from a reduced view (a field's name
  and type, nothing else) takes away every mark it does not restate:
  `sensitive`, `searchable`, a label, annotations, an expression. The
  migration is lossless, since no stored value changes, and its
  preflight lists each such member in `restated` with what was declared
  and what `to` states; `dropped` names every removed relation, with 0
  where it holds no live link. Build `to` from `describe`'s answer for
  the type and change what you mean to change; a report whose `restated`
  is not what you meant is the signal. A field you may not read is
  described and reported by its shape alone: your preview says `more`
  where it leaves a member out, and a reviewer who reads the field sees
  the rest. A proposed migration is refused
  at its accept ("propose it again") where the type gained or lost a
  member since that the proposal's report does not restate
  (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
  extended).
- **A grant on a link type follows the relation's rename, as a grant on
  a field follows the field's.** A field of the link type is as closed
  as the lowest of what a principal's grants say under every name the
  link type has borne: its grant, else its default. So
  `sys.link.invoice.signed_by: {default: none}` holds for
  `sys.link.invoice.approved_by` after the rename, and a grant on the new
  name holds for the old one in a read as of before. A permission stays
  with its name: a host that wants the links read under the new name
  names the new link type. A mirror's mapping that names the old link
  type stops applying at the rename and is stated again.
- **A key is shown to whoever is shown the record.** A principal that
  is shown a type finds and gets its records, also where it reads no
  field of them: each with its key, its version and the names of the
  fields it sees masked. A key made of fields holds their values,
  whatever is granted on those fields, so a host does not key a type by
  a field it hides; and a principal that is not to list a type's records
  is granted nothing of the type. See
  `docs/decisions/006-grants-not-roles.md`, extended.
- **One writer per lake.** A lake has one writing process at a time,
  held by a lease; one process may write many lakes. See
  `docs/decisions/008-one-writer-per-lake.md`.
- **No query across lakes.** A lake is one world with one truth; a host
  that needs data from two lakes reads each of them, or mirrors the types
  one lake needs from the other. A mirror carries its provenance, and a
  lake whose types many lakes share is mirrored into each of them.
- **A set over a large type is declared within a bound.** A set's
  declaration decides the records already there in one append, at most
  `max_set_changes` of them (20 000 unless the location sets it). Past
  it the declaration is refused `too_large`. The host raises the limit
  for a quiet moment, or widens the filter in steps; `docs/seams.md`,
  "Declaring a set over a large type", has the costs.
- **A whole-graph measure is read in one page.** `degree` and
  `components` are computed whole for every page read, so a walk through
  small pages computes the measure once per page. A host that wants the
  whole answer, or its own first *n* rows, asks for it in one page: a
  measure's `limit` may go up to the lake's `max_graph_nodes` (100 000
  unless the location sets it), where every other read stops at
  `max_limit`. See `docs/decisions/026-the-store-walks-the-graph.md`.
- **A migration of a large type is a step for a quiet moment.** Applying
  one reads, converts and writes again every record of the type under
  the writer; other writes wait. The check before the append and the fold
  read the type a page of 1 024 records at a time. What the store still
  holds for the whole type is keys, not records: for a type with key
  fields the old and the new key of every record; for a re-key its moves,
  in the report's id, the event and the fold, and one membership for each
  moved record and set; the ends of the live links of a relation it
  narrows; a set's members and its changes up to `max_set_changes`.
  PostgreSQL writes a transaction's puts a page of 1 024 at a time inside
  its transaction, so its engine holds a page of them too. At 100 000 records of four small fields, measured under some
  load from other processes (`tests/set_migrated.rs` in the SQLite and
  PostgreSQL engines, by hand), the writer is held 4 to 5.5 s on SQLite
  and 10.5 to 14 s on PostgreSQL. On SQLite the peak is a few megabytes
  whatever the type's size, 57 MB with a set that changes 20 000
  memberships, and for a re-key of every record about 160 MB, which grows
  with the records moved (630 MB at 400 000). On PostgreSQL it is the
  same, with 8 MB where SQLite holds a few. On PostgreSQL the statements that write a migration
  sort and hash a row for every record; at the server's default `work_mem`
  of 4 MB they do it on disk, so a host that migrates large types gives
  the lake's connections more. An object set of the type adds little: its members
  are read once, and 20 000 memberships changed cost one to three seconds
  and 40 to 55 MB. Before that single read (lakelands#477) each set added a lookup
  for each record: 41 s with one set and 74 to 79 s with three on
  PostgreSQL, 9 to 15 s on SQLite. Bringing the migration's own cost
  down further is lakelands#480.
- **A host may state its declarations at every start.** A declaration
  that is the one in force is answered `applied` at the head and appends
  nothing, by `declare` and by `apply`: a type, a value type, a function,
  a verb, a trigger, an object set. A computed field may be stated with
  the inputs `describe` shows or without them. A declaration's version
  counts its changes. See
  `docs/decisions/027-shape-changes-are-declared-or-migrated.md`.
- **A model that comes up after the open is bound then (lakelands#641).**
  A host whose embedding model answers later than it opens its lakes (a
  sidecar loading, a model downloading) opens them without one and
  calls `LakeStore::bind_embed` once the model answers; no reopen. Until
  then a due embedding waits, a hybrid search ranks lexically and says
  so, and a declaration that adds an embedding, or changes one's
  `from`, model or dimensions, is refused as `Unbound`; a type stated
  again as in force, or grown beside its embedding, is admitted. The
  bind appends nothing and makes nothing due: after it, call `recompute`
  for each type whose `vector_coverage` shows records due, which also
  calls again for a failure recorded while the model was away. A second
  bind replaces the first, whatever its model; a field of the old model
  then waits, and a search over it is refused as `Unbound`, until its
  type is declared with the new one. Over the network doors `bind_embed`
  answers `Unbound`: binding is the host's.
- **`recompute` is how a host computes again.** It names a function, or
  a type with an embedding or a field of a function with a backing, and
  does three things a declaration in force no longer does: it calls
  again for a recorded failure, makes a field that waited for its
  backend, and computes a function's fields with the settings and the
  time of the call. It appends one `declared` and holds the writer for a
  walk of the type, as a recomputation does (below). A derived expression
  of a type that reads a setting or the clock is not its business: such
  a field is as of its record's last write, and a write of the record
  brings it up to date; the type declared again with a field more that
  is not computed does not, since it computes nothing. From a shell: `lakelands recompute --function
  <name>` or `--type <name>`. See
  `docs/decisions/022-functions-are-declarations.md`.
- **A write waits for its own calls, not for a backlog (lakelands#642).**
  A declaration that makes embeddings or calls due over a type answers
  once its sweep has made them all. A write made meanwhile that makes
  nothing due answers at once; one that makes a call due waits for the
  batch in flight and its own (a batch of up to 64 texts, one request or,
  where the backend refuses texts, up to 127 by its halving; or one
  function call; and their recordings), and one batch more of what no
  write waits for any more, not for the backlog. What a declaration
  whose caller went away (or whose drain failed) still owes advances by
  one batch with each later write that owes a call, or is made by a
  `recompute` of the type. A change of an embedding's
  `model` clears every vector of the type: until the sweep has embedded
  them again, the type's records are out of vector search, and
  `vector_coverage` counts them due.
- **A text the embedding backend refuses fails alone (lakelands#658).**
  The store sends the texts due for one model in batches of up to 64.
  A backend that answers `Invalid` refuses the request for what it
  carries, and the store asks again in halves until the refused text is
  alone: only that text records the refusal, and the texts beside it are
  embedded. Any other error is weather and is recorded for every text
  not answered yet; `recompute` retries them. The shipped client answers
  `400`, `413` and `422` as `Invalid`. A host's own backend that answers
  everything `Unavailable` keeps the old behaviour, a whole batch failed
  for one text. An endpoint that answers `400` to every request (a
  model name it does not know, say) costs 127 requests per batch of 64,
  each text recorded as failed alone; fix the configuration, then
  `recompute`. Two limits of the client remain: it splits a batch by its
  own `max_batch`, so a halving sends again the requests that had
  succeeded; and a fault in one item of a `2xx` reply (wrong dimensions,
  a value that is not finite, a wrong count) is `Unavailable` and still
  fails the texts beside it.
- **A write that changes nothing is still a write.** An upsert of the
  values a record already holds appends an event and raises the version:
  it says who vouches for the values and when, a trigger reacts to it,
  and a caller that writes with `expected_version` counts on it. A host
  that states data again and wants no growth has three ways: an
  idempotency key, under which the same call appends once; `ingest`,
  which appends nothing for a record delivered twice; or a read first.
  Opening a lake for writing appends one `lease_taken`, which carries
  the token every later event of that writer bears; a reader appends
  none.
- **A required field a source does not write takes `initial`.** A type
  a source feeds may hold a field the source knows nothing of, such as a
  state the lake's own users move. Declared `required` alone, no door
  can make its record: the ingest may not carry the field, and an
  author's write comes after the record exists. Declare it with
  `initial`, a literal: the write that makes the record fills it at
  version 1, with `origin: initial`, and the host needs no second
  append. The filled value makes no owner: a purge of the source takes
  the record with the state an author moved, and names it in `moved`. A
  record retired and made again starts again from the initial value, and
  a trigger sees a create. Changing `initial` later changes no stored
  record. See `docs/decisions/050-a-field-declares-its-initial-value.md`.
- **A write answered `unavailable` may be in the log.** The store
  appends a write durably, then folds it. When the fold fails (the
  projection's transaction cannot begin or commit: a busy SQLite file, a
  lost connection, a full disk), the write is answered `unavailable` and
  its sentence says so: what was appended is in the log from the
  position named, it is not folded yet, the next write folds it, and a
  write is sent again only under its idempotency key. Reads answer at
  the last folded position meanwhile. The next write through any door
  folds what was left before it is decided, or is answered `unavailable`
  (the projection could not catch up) and appends nothing. A caller that
  sends the write again under its key is answered the first write's
  outcome; one without a key writes a second time. For a verb that
  reaches outside the lake the answer says one thing more: its executor
  was not handed the call, and sending it again under its key will not
  run it (the retry is refused as `unanswered`: a retry is answered
  `applied` only with an answer of the executor that is recorded). The
  host calls the verb again under another key or with none; a proposal
  accepted so is spent, and the call is proposed again. The kind is the same as for a write that never
  reached the log, and a caller cannot tell the two apart by it, over a
  network door least of all: a host that must not write twice gives its
  writes a key. When the calls a write made due cannot be recorded, an
  upsert, a feed's window, an aging run and a purge whose own fold
  succeeded are each answered their own outcome and the computed fields
  wait. The next write records what was left, from the answers the calls
  were already given, or is answered `unavailable` if the lake cannot
  catch up; `vector_coverage` counts the embeddings that still wait. Two
  writes are answered an error although their own fold succeeded. A call
  of a verb that reaches outside the lake, and its acceptance: where the
  calls due before the hand-out could not be recorded, the executor is
  handed nothing and the answer is `unavailable` with the sentence about
  the executor (an acceptance's says that the proposal is decided, that
  accepting it again will not run the verb, and to propose the call
  again). And a write that set
  something off (a trigger fired, a set joined or left), which reads
  the lake's description to name it to its writer: where that read
  fails, the write is answered the read's error. Each is in the log and
  folded, and a write sent again under its key is answered its position.
  An executor's answer whose append fails `unavailable`, or that a
  writer which cannot catch up cannot append, is held by the handle that
  ran it and recorded by its next write; the call's answer says so, and
  the retry under its key answers it. It is lost when the handle is
  closed or dropped before a write records it (the server ends so), and
  on PostgreSQL when a lost connection loses the lease: a retry is then
  `unanswered`, and the first answer carried the output. A host that
  needs more gives the executor its own record of what it did, keyed by
  the call's position (`VerbInvocation.position`). See
  `docs/decisions/013-verbs-run-by-declared-risk.md`, extended.
- **A declaration that recomputes a type is a step for a quiet moment.**
  A function declared again with a change, or a type declared again with
  another derived expression, computes the derived fields of every live record
  of the type again in that one append, under the writer; other writes
  wait. The store reads the type a page of records at a time, puts each
  record it changes and keeps none: it holds the declaration, not the
  records, so what it holds does not grow with the type
  (`docs/decisions/048-the-fold-keeps-the-event-not-the-records-it-changed.md`).
  A set that declares several functions of one type again, applied or
  accepted as a proposal, reads the type once and computes and puts each
  record once, at the set's last declaration (log format 12): at 2 000
  records of 52 fields, a set of 50 such functions takes 0.56 ms a
  record on SQLite, where computing it again at each declaration took
  24.6 ms (lakelands#605). A read at a position inside the set finds
  the records as before it. At 100 000
  records of four small fields, one of them derived
  (`tests/recomputed.rs` in the SQLite and PostgreSQL engines, by
  hand), the writer is held 4.1 to 4.5 s on SQLite and 13.3 to 14 s on
  PostgreSQL, whichever of the two is declared again, and about 20 s
  and 51 to 56 s at 400 000; the peak of the append is 3 MB on SQLite
  and 9 MB on PostgreSQL at both sizes. Before format 12, a bundle of
  two changed functions held the writer 12 s on SQLite and 21 s on
  PostgreSQL at 100 000 records (50 s and 99 s at 400 000), at 4 MB and
  11 MB. The time grows with the type's records; the peak does not. One
  thing still grows with them: what the engine holds of an open
  transaction, which on the memory engine is every put. The calls an
  embedding or a function computed outside the lake makes due on that
  append do not: the declaration owes one sweep over the type, read
  after the append a page of 64 records at a time, the writer released
  between pages (lakelands#609). A rebuild, or an open that catches up,
  commits inside a page of the log after such a declaration, so there
  too each record is computed once for each declaration. Such a fold of
  the log makes no call due and holds none: a rebuild of 100 000
  embedded writes of a 1 000-byte text peaks at 4 MB on SQLite, where it
  held every write's call and peaked at 163 MB before
  (`tests/recomputed.rs` in the SQLite engine, by hand;
  `docs/decisions/022-functions-are-declarations.md`, extended). A type
  declared again that computes nothing (its description changed, say,
  or a field that is not computed was added) costs one pass of the
  engine over its records at the commit, a page at a time: 1.3 to 2.1 s
  at 100 000 records and 3 MB whatever the type holds. A type with an
  embedding or a field of a function computed outside the lake is never
  such a type: the store walks its records at every declaration of it,
  whatever changed.
- **A view that is read often may be kept.** A view's read walks its
  source: seconds at a hundred thousand records. The setting
  `view_kept.<type>` asks the fold to keep the view's records in the
  projection, and a read of the view then reads those, at the head and
  at every position from the setting on
  (`docs/decisions/049-a-view-the-fold-keeps.md`). What it costs and
  where it does not hold:
  - **Turning it on is a step for a quiet moment:** the fold of the
    setting's write walks the view's sources once under the writer and
    holds the folds of every group until that append ends, so a view
    grouped by a nearly unique field holds about as many as its source
    has records.
  - **Every write to a source puts one state more** for each group it
    touches in each kept view, for good. An append reads the state of
    the kept views that read a type it wrote, one read each, and of no
    other; a lake with no `view_kept.` setting reads none.
  - **The view is made again,** by the same walk, by a declaration of
    the view or of a source, by a declaration that computes a source's
    records again (a function they call, declared again with a change),
    by a migration that reaches a source's records, and by a write of a
    setting its expressions name. Declare and migrate as for any large
    type: in a quiet moment. After a declaration that computes records
    again the view is made once those records are put: a read as of a
    position in between computes.
  - **A read computes, as before,** when it names a valid time, when it
    is as of a position before the view was made, and while a group of
    the view is not exact for its reader: a measure that fails over it,
    or an extreme that was given up (below). `describe_type` then says
    `not_whole` with the number of such groups, to a principal the
    view's read would answer.
  - **A write that takes an extreme's holder away reads its group
    again,** in its own append and under the writer: the live records
    that hold the group's values, 8 192 at most. The bound is one for
    the group over all the view's sources, and a record the view's
    `where` leaves out counts in it. An append whose n events each take
    a holder away reads up to n times that: an aging run that moves the
    oldest record of 1 000 groups of 5 000 under `min(date)` reads 5
    million records in its append. Such a step is one for a quiet
    moment, as a migration is. Three reads cost more than the bound
    lets one expect:
    - **A view over a kept view** reads, beside the inner records of
      the group's values, every inner record whose whole face reads
      otherwise than its own (a restricted member of its group makes it
      so), whatever its group, and each counts in the bound: an outer
      group of three members is given up once the inner view has about
      8 190 such records, and until then every such reread reads them
      all.
    - **A view grouped by more than one field** is looked up by its
      first text field alone: a view grouped by `country, city` reads
      every record of the country for a reread of one city, on the
      memory engine every record of the source. Those are read and not
      handed to the fold, and the bound does not count them.
    - **After a migration's step in the same append** every lookup
      walks the source whole.
  - **A group past that bound is given up:** its extreme is kept no
    more, the view computes for the readers it concerns, and
    `describe_type` says `not_whole` with the group counted. Nothing
    mends it by itself, and the fold goes on keeping the view's other
    records meanwhile, at a state for each write: a reader whose face
    of the group is exact, one without the `restricted` grant where the
    group was given up on the whole face alone, is answered from them.
    What mends it is what makes the view again
    (above), or writing its setting `false` and then `true`; or the
    group's own end. A view with a `min` or `max` over groups larger
    than the bound, or whose holders leave in bulk, is not worth
    keeping today (lakelands#629).
  - **A reader with the `restricted` grant** is answered from every
    kept record of a view at each call, read once and once more for a
    total, as soon as one group of it reads otherwise for that reader
    (a restricted record is a member of it). For every other reader,
    and for that one while no group differs, the page and the count are
    the engine's.
  - **Not every view can be kept:** only counts, sums and averages of
    what is certainly a number, and a `min` or `max` of a number or an
    instant under text or reference group fields. The setting's write is
    refused otherwise and says which measure. `describe_type` says for a
    view whether it is kept, and why not.
- **A kept view is made to be read at the head.** The records of a
  view kept as its sources are written (the setting `view_kept.<view>`)
  are put again at every write to their group, and SQLite and
  PostgreSQL read them by statements of their own. At the head a page
  or a count reads the states in force and none before them, however
  often a group was written: with 10 000 writes to each of 20 groups, a
  page in 0.05 ms on SQLite and about 1 ms on PostgreSQL, where a stored
  type's statements take 120 ms and 45 to 60 ms. As of an earlier
  position such a read steps from group to group and seeks each one's
  state there. That pays where groups are written often (the same view
  in 0.1 ms and 1 to 2 ms, against 80 ms and 33 to 39 ms) and costs on
  PostgreSQL where they are written seldom: at about 20 writes a group a
  page of 1 000 takes 14 to 15 ms where a stored type's statement takes
  8, and a count 67 to 78 ms against 39 to 43; SQLite is faster at both
  positions. A read `as_of` an earlier position is the rare one of a
  kept view.
- **Every answer carries its position.** A host that records what an
  agent saw records the position, and reads `as_of` it later. See
  `docs/decisions/009-every-response-carries-its-position.md`.

## Building declarations in Rust

A host that builds declarations or reads descriptions through
`lakelands-api` keeps compiling when a later release adds a member to
them:

- **Build with the constructors or with `..Default::default()`.** For
  example, `FieldDecl { searchable: true, ..FieldDecl::of("note",
  StorageKind::Text) }`, `TypeDecl { fields, ..TypeDecl::named("invoice") }`,
  `RelationDecl::to("partner", "partner")`, and `SearchMode::lexical(text)`,
  `::vector` and `::hybrid` (0.9.0 added `weight` and `also` to a vector
  search and `weight` and `text_weight` to a hybrid one; a mode built by
  its constructor leaves them out). `TypeDecl`, `FieldDecl`, `RelationDecl`,
  `TypeDescription`, `FieldDescription`, `LakeDescription` and
  `TypeLifecycle` all implement `Default`. A name left at its default is
  empty, and the store refuses it at declare. A field's kind defaults to
  text.
- **Read descriptions with `..`.** A pattern such as
  `TypeDescription { name, fields, .. }` names only what it uses.
- **Expect closed enums to grow.** `Refusal`, `Projection` and
  `ProposedWrite` are closed on purpose
  (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
  A release that adds a case names it in its notes, under "Changes a host
  must make", with every other change a host's code has to follow.
  `crates/lakelands-api/tests/host_builds.rs` is the host's view, kept
  compiling.

