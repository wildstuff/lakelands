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
  model and dimensions. Change the model or the fields it is made from by
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
  position to poll from next. It acknowledges on its own side. The host keeps the
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
  `LakeStore::purge_source(source)` (`lakelands purge --source <id>`):
  every record it brought in is retired in one append, and a record
  another hand also wrote refuses the purge
  (`docs/decisions/039-a-source-is-retracted-as-one.md`). The report
  names, per relation, the live records outside the purge that still
  refer to a record it took. A host that must know that before it acts
  asks first: `LakeStore::purge_preflight(source)`
  (`lakelands purge --source <id> --preflight`) appends nothing and
  answers the same report under an id, and `purge_source(source,
  Some(id))` (`--report <id>`) purges only if the lake has not moved
  since.
  A lake's log is exported with `lakelands_store::export::write(store,
  dir)` (`lakelands export --to <dir>`) as files that verify without
  Lakelands (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`);
  an export is the whole log, unmasked, so it is handed over as the
  lake itself would be. An export only reads: it needs no writer, appends
  nothing, and runs beside a `lakelands serve` that holds the lake.
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
- **`recompute` is how a host computes again.** It names a function, or
  a type with an embedding or a field of a function with a backing, and
  does three things a declaration in force no longer does: it calls
  again for a recorded failure, makes a field that waited for its
  backend, and computes a function's fields with the settings and the
  time of the call. It appends one `declared` and holds the writer for a
  walk of the type, as a recomputation does (below). A derived expression
  of a type that reads a setting or the clock is not its business: such
  a field is as of its record's last write, and a write of the record
  brings it up to date. From a shell: `lakelands recompute --function
  <name>` or `--type <name>`. See
  `docs/decisions/022-functions-are-declarations.md`.
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
- **A declaration that recomputes a type is a step for a quiet moment.**
  A function declared again with a change, or a type declared again with
  another derived expression, computes the derived fields of every live record
  of the type again in that one append, under the writer, and holds
  them in memory meanwhile; other writes wait. At 100 000 records of
  four small fields, one of them derived (`tests/recomputed.rs` in the
  SQLite and PostgreSQL engines, by hand), the writer is held 4.3 to 5 s
  on SQLite and 12 to 15.5 s on PostgreSQL, with a peak of about 330 MB
  on SQLite and 340 MB on PostgreSQL, whichever of the two is declared
  again. Both grow with the type's records (1.3 GB at 400 000). A migration's fold holds a page of records at a time;
  a recomputation's does not yet, and lakelands#523 holds what is done
  about it. A type declared again that computes nothing (its
  description changed, say) costs one pass of the engine over its
  records at the commit, a page at a time: 1.3 to 2 s at 100 000
  records and 3 MB whatever the type holds.
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

