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

  A PostgreSQL lake's location file (`lakes/<id>.json` in a root;
  `lakelands lake new <id> --engine postgres --url <URL>` writes it,
  with `--schema`, `--no-tls` and `--ca-file` for the members below, and
  holds the URL, a password in it too, as plain text, so keep `lakes/`
  to the operator; the first open tells whether the URL connects):

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

  PostgreSQL in a container wants a `shm_size` of at least 256 MB:
  its parallel workers take shared memory of `maintenance_work_mem`'s
  size and more from `/dev/shm`, which Docker's default 64 MB cannot
  give, and the statement then fails as unavailable ("could not resize
  shared memory segment") (lakelands#1161).

  The engine sets `synchronous_commit` itself: `on` for the log's
  appends and seals, so a server or role set to `off` loses no answered
  append, and `off` for the projection's commits, which a crash can lose
  and a writer's open folds again from the log
  (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).

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
  dedup key. The executor gets the record unmasked, as it stood at that
  position, where the call was judged; a host that wants it
  to see only what the caller may see applies the caller's grants there. The executor's answer is recorded in the log; a failure is
  recorded as `{"error": …}`, the write that made the verb due stands,
  and the caller gets the recorded answer either way. Without a bound
  executor such a verb is refused as `Unbound`, not faked. See
  `docs/decisions/013-verbs-run-by-declared-risk.md`.
- **`ChangeFeed`.** The host reads the store's feed: the log, filtered
  by kind and type, from a position; each poll answers at once with the
  position to poll from next. A poll examines at most `FEED_EXAMINED`
  (4 096) events, so it may answer none short of the head: the host is
  caught up when `next` is past `head`, not when a batch is empty. A
  caught-up host calls `wait(next - 1, within)`, which ends at the
  lake's next append, instead of sleeping and polling (lakelands#1061).
  It acknowledges on its own side. The host keeps the
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
  mirror). The store's own consumers wait for the next append once
  caught up, so a change reaches them at once, and each setting bounds
  only that wait (lakelands#1061):
  - the `mirror` section's `poll_millis` (1 000 ms), for its source's
    next append;
  - the `deliver` section's `poll_millis` (1 000 ms), for the lake's;
  - the `server` section's `feed_poll_millis` (250 ms), for a feed
    stream's or an MCP session's, beside its `feed_floor_millis` (10 ms):
    the most often the log is read for such consumers, counted from the
    end of the last read an append woke. The streams and sessions that
    follow one lake with the same kinds and types share one read of the
    log, and each applies its own keys and its own principal's view to
    it (lakelands#1113), so a thousand of them cost the writer what one
    does: 300 upserts took 1.5–1.7 s on SQLite and 1.6–2.0 s on
    PostgreSQL beside 1 000 streams, as beside none, where a read per
    stream had taken 8 s and 51 s at a floor of 250 ms. A stream whose
    client falls behind reads on alone from its own position until it
    is caught up again; it never holds the others.

  An in-process host that calls `wait` paces its own reads the same way.
  An agent on MCP is told of changes without polling:
  every session receives `notifications/tools/list_changed` when the
  lake's declarations change. Until the session lists its tools again,
  each stream it opens begins with that notification, so one taken by a
  stream that closed before its client read it is not lost
  (lakelands#744). After the `subscribe` tool, it also
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
  store declares the allowed types it does not hold yet, and its own
  `sys.*` types again where the lake holds an older form of them.
  Lakelands reserves only the types its own fold reads; today those are
  `sys.setting` and `sys.aging_rule`, and the other two are reserved
  ahead of their readers. See `docs/decisions/003-reservation-by-namespace.md`.
  A value that must be kept from a reader is not put into a setting:
  any reader's expression reads `setting('<key>')`, whatever its grant
  on `sys.setting` and whether or not the setting is restricted.
  The settings the store itself reads, with those of a lake's location
  and of the root's `lakelands.json`, are listed in `docs/settings.md`
  with what each changes, its default and where it is kept, and the
  bindings apart; `lakelands settings` shows
  their values for a lake. A key under the prefix of one of the store's
  own settings that names none of them is refused, naming the prefix
  (`docs/decisions/063-every-setting-is-in-one-list.md`). A host
  declares its own settings in the lake (`{"declare": "setting", ...}`,
  or a bundle's `settings/` lane): each write of one is judged by its
  declared kind, and the store stamps its `group`, which a grant's
  `only` on `sys.setting` names to say who writes it.
  The aging rules run when the host calls `LakeStore::age(today)`, once
  a day, with the date (`docs/decisions/036-aging-runs-by-date-once.md`);
  `lakelands age --today <YYYY-MM-DD>` is the same call from the CLI, and
  `lakelands serve` makes it daily when the root's `lakelands.json` has an
  `aging` section: the date at its `utc_offset`, once at start and at each
  rollover (`docs/seams.md` §aging). A rule moves the records whose field
  holds its `from` to its `to`, and both are values, written as they are:
  a `to` of `concat('over', 'due')` is that text, never evaluated. A
  record whose field is null holds no `from`, so no rule moves it and the
  report does not count it. A host that wants such records aged gives the
  field a value: a declared `initial` at the write, or a verb that sets
  it.
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
  A purge retires and does not erase: `as_of` still reads what the
  source wrote. **Forgetting a source** erases it
  (`docs/decisions/058-a-source-may-be-forgotten.md`). A source's values
  of a field declared `erasable` enter the log sealed under a key of its
  own, which the host's key store keeps outside the lake; the CLI and
  `serve` bind `lakelands-backings`' file keyring where the root's
  `lakelands.json` names one, `"keys": {"keyring": "<directory>"}`,
  relative to the root, and refuse a keyring inside a lake's directory.
  Without one a value that would be sealed is refused.
  - **The act:** `LakeStore::forget_source(source, None)`
    (`lakelands forget --source <id> --report <id>`) appends `source_forgotten`,
    retires what the source alone brought, has the key store destroy its
    keys, rebuilds the projection and reclaims the storage, all under the
    writer's lock. Its preflight, `forget_preflight` (`--preflight`),
    says what it erases and retires, what is computed again, and how
    long it expects to hold the lock (the log's events at the engine's
    measured rate: about 2 500 a second on PostgreSQL, 17 000 on
    SQLite). The CLI acts only with `--report <id>`, the preflight's,
    and only if the lake has not moved since; without it, it answers the
    preflight.
    Choose a moment when waiting writes can wait.
  - **What it cannot reach** refuses it: a value the log holds in
    plaintext, written before its field was erasable or by a release
    before 0.15.0. Such a lake is exported, the source left out, and
    imported as a new lake.
  - **After a crash** between the act's steps, open a writer: the next
    writer's open, or the act repeated, finishes what is owed. A
    handle that only reads answers what the projection held until then.
  - **A reclaim that fails** is owed, never a lock-out: the values are
    erased already, so the writer opens without it, says why on the
    CLI and in `serve`'s log, and tries it again at each open until it
    runs (lakelands#1161). A rebuild still owed refuses the writer's
    open while it fails, since the projection is not whole without it.
  - **Keep backups honest:** a key restored from a backup restores what
    it sealed. The key store's backups must follow its destroys, and the
    keyring stays out of the lake's backup set: a backup that carries
    both carries what forgetting destroys. A disk snapshot, a PostgreSQL
    WAL archive or a copy outside the lake is out of the act's reach. An
    import finishes a forgetting its export holds, so a key store
    restored beside it has that source's keys destroyed again.
  - **Backups under key periods:** back the key directory up at least
    once a period. A backup older than the newest version the log used
    leaves the lake refusing a source's writes, `key_missing`, since the
    key store is behind the log. A backup taken before a version was
    destroyed brings it back, so destroy the same prefix again after any
    restore.
  - **Key periods:** `lakelands settings set key_period week` (or
    `key_period.<source>`, `day`, `week` or `month`, in UTC) seals each
    period's values of a source under a version of its key of their
    own, made at the period's first value: one key a period, made by the
    key store. A version is destroyed only with every older one
    (`KeyStore::destroy_version`), and a host's KMS that destroys only
    whole sources leaves that call refused. Until the act that takes
    part of a source back comes, a version is destroyed with the lake
    closed, by calling `KeyStore::destroy_version`; one who edits the
    file keyring by hand removes only a source's oldest entries, never one
    with an older still there. An open lake keeps the keys it
    holds in memory and goes on sealing with them. Such a version is a
    lost key, since no act in the log names it: at the next open the
    values and the outputs it sealed fail closed, `key_missing`, at a
    rebuild or a re-plan.
  - **An export** holds sealed values as ciphertext, and no key. It
    imports with the key store bound, or with none where the export
    holds the source's forgetting, whose values then arrive erased; any
    other sealed value refuses an import with no key store, as unbound.
  - **A mirror** reads the source through the doors, which answer
    plaintext. At a `source_forgotten`, each mapped type whose source
    type has an erasable field stops past it, naming the source: its
    copies may hold that source's values under the mirror's own source.
    Copy the type again, or forget `mirror:<lake>:<type>` in the target,
    then restart.
  Source ids and record keys stay plaintext in the log, so a lake is
  ready to forget when:
  - **a source id names no person:** an opaque id for a device or a feed,
    the person kept in the host's own mapping;
  - **a type that holds personal data is keyed by a surrogate:** a minted
    id, never an e-mail address or a name.
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
  Opening a lake on SQLite reads the end of each log segment's index
  and the first line of each after the first, not the index: what an
  open costs follows the number of segments, one
  more for each `checkpoint` that sealed one, not the events the lake
  holds, so a host may open every lake at its start. A damaged index line
  is therefore not found at the open; `verify`, which reads the log,
  finds it, as does the read that meets it. Neither is a line of the
  log changed at its own length before the last append, on any engine:
  the lake opens and is written on (one inside the last append is
  refused at the open). `lakelands verify` runs it from the command
  line, beside a `serve`, read-only; for a scheduled check its exit
  status is 0 when the log holds, 1 when it is damaged (the line names
  the position where the log lets it), 2 for a command line it does not
  understand or a range outside the log, and 3 when it could not check
  (no lake yet, no projection, a memory lake, the database
  unreachable). `index/` is part of the
  log, not derived data: back it up and restore it with `log/`. A
  segment without its index file, a `projection.db` with no segment
  beside it, and a log whose segments do not follow each other (a
  sealed segment gone, or another lake's in its place) are refused as
  `damaged` at every open and at an import, and nothing is cut or made;
  the way out is a restore of the files. On every engine, a projection
  folded past its log's tip is refused at an open to read, to write and
  to create (`ends_before`): the log lost events, or the projection is
  another lake's. A writer is refused before it takes the lease, so
  nothing of the log is cut. An import onto such a lake is refused too:
  as `import_not_empty` where the log still holds an event, as onto any
  lake that is not empty, and as `ends_before` where the log reads
  empty. An open to rebuild is not
  refused for it. A rebuild makes the projection from the log as it
  stands, and its writer cuts what the log's index does not commit, as
  every writer does: it is for an operator who knows the log is whole.
  A writer's `close` truncates
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
  A write answered `unsynced` has met a second failed sync: the handle
  is done, and refuses appends, seals and taking the lease until the
  lake is opened again
  (`docs/decisions/060-an-append-syncs-its-two-files-at-once.md`). The
  host closes the handle (`LakeStore::close()`, or drops it) and then
  opens the lake again. In that order: the writer's lease is an OS lock,
  held by the old handle until it is closed, in this process too, and an
  open made while it is held is refused the lease. The open checks the
  last group and writes it again, that group alone: an `unsure` append
  that came before may or may not be in the log, so the host reads it
  back by its id, or sends it again under its idempotency key, before it
  counts it lost. A pool that keeps one open lake per tenant closes the
  refusing handle before it opens the next.
  In a quiet hour the host can call `LakeStore::compact()` (`lakelands
  compact`) to give back the storage the projection no longer uses; it
  changes no answer and appends nothing, and writes wait while it runs
  (`docs/decisions/040-maintenance-never-changes-an-answer.md`). On
  PostgreSQL, `VACUUM` skips, with a warning, a table the connecting role
  does not own, and the compact reclaims nothing there; connect as the
  role that owns the lake's schema.
  After a bulk load into PostgreSQL the engine analyses its tables
  itself, once every 5 000 records a writer puts or order index rows a
  `sort_index.<type>` setting rebuilds, so the planner knows their size;
  a host that loads a lake's schema by another path runs
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
  `max_parallel_maintenance_workers = 0`, and opens the writer again.
  Under the index a page of a search is the index's answer at the depth
  that page asks for, so the pages of one query may answer a record
  twice or leave one out (043): a host that needs a ranked result whole
  and in order reads it in one page, up to `max_limit`, or leaves the
  field without the setting. A group of equally near vectors is no
  exception: the members the index hands out at a page's depth come
  once and in key order there, and the index may hand out a part of
  the group and go on with farther vectors. A vector of zeros is never
  answered for a field read through its index, whether the index or the
  scan after it answers the search; a field without the setting scores
  it 0, and so does every field on a server without pgvector, and a
  field whose index is behind the head, which a writer's open builds.
  Some searches read every vector of the
  field after the index. One is a search the index hands fewer
  candidates than it asked for: on a field smaller than what the search
  pulls (a field under 65 vectors for a search of one ranking, a field
  no larger than `max_limit` for one that fuses rankings, and a page
  deep into a larger one); past 20 000 candidates or pgvector's memory limit for such a
  scan (`hnsw.scan_mem_multiplier` times `work_mem`, which stays the
  server's), which a search under a filter that keeps few records
  reaches on a large field; and over a group of several hundred
  identical vectors, of which the index reaches a part. The other is a
  search whose pull of 4 096 candidates or more ends inside a group of
  equally near vectors. **The engine sets `hnsw.max_scan_tuples`
  itself,** to pgvector's default of 20 000, for each search and
  whatever the server's value: where the index ends decides which
  searches the scan answers, and an answer must not move with a
  server's setting. It is set with `SET LOCAL`, for the search's own
  transaction only; nothing of the role's or the server's is changed,
  and a value a host set there has no effect on a lake's searches.
  SQLite and in-memory lakes keep no such
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
lakelands --root new lake new finance --engine postgres --url postgres://...
lakelands --root new import /tmp/finance
```

`lakelands import <dir>` fills the root's lake of the export's own name,
read from the export's first event: a lake's name is inside every hash,
so an import cannot rename it, and `--lake`, when given, must name the
same lake. The lake's location must exist and the lake must hold
nothing: `lakelands lake new <id>` writes a SQLite location, and with
`--engine postgres --url <URL>` a PostgreSQL one, and opens nothing. A lake kept in
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

**A projection is read only by the layout that made it.** A SQLite or
PostgreSQL projection is marked with its engine's layout number when it
is made, and a build whose engine lays the projection out otherwise does
not read it. A SQLite `projection.db` that holds no table yet is
none of that: its maker has connected and not made the tables, and a
reader that comes in that moment is answered `no_projection`, as the
moment before, and opens a moment later.
- **An older build's projection is rebuilt by the first writer's open.**
  The open takes the lease, drops the projection, makes it anew and
  folds the whole log, and holds the writer that long; the CLI says so
  (`the projection was rebuilt from the log at this open`), and a host
  reads why from `LakeStore::rebuilt_at_open`. 0.16.0 is such a step
  for both engines: the first open of 0.16.0 rebuilds the projection,
  about 8 s per 100 000 records on SQLite and 47 s on
  PostgreSQL (at 1 000 000 records 80 s and 7.5 min; one event a
  record, a release build). `lakelands upgrade` runs that open at a moment
  the operator picks, also for a lake already in this release's log
  format, and `lakelands upgrade --check` names the layout it found.
- **A reader waits for the writer.** A read-only open of it is refused
  before anything is read, naming the number found, the number wanted
  and `lakelands rebuild` (`projection_layout` in `docs/refusals.md`).
- **A newer build's projection, or one with no mark, is refused** at
  every open, as above. `lakelands rebuild`, or a host's own open with
  `OpenMode::Rebuild`, opens it, drops the projection under the lease,
  makes it anew and folds the whole log; the open is the rebuild.
  `LakeStore::rebuild` on an open lake does the same for a lake that
  opened.

**A lake outlives a release from log format 14 on.** A release writes
one log format and reads every one from 14, the format of the first
release with real installations, to it (`docs/log-format.md` §6;
`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
- **An older lake is upgraded by its first writer's open.** The open
  appends one `lake_upgraded` after its `lease_taken` and writes the new
  format from there; the events before stay as they were written, so
  every export and checkpoint the host holds stays true. Where the step
  changes how events fold, the same open rebuilds the projection, and
  holds the writer that long: at 100 000 events about 7 s on SQLite and
  35 s on PostgreSQL; a step that changes no fold, as 14 to 15, costs
  one append. `lakelands upgrade` runs that open by itself at a moment
  the operator picks, and `lakelands upgrade --check` says what it would
  do and opens nothing.
- **Take a backup before, and stop every older process first.** There is
  no downgrade: a lake a release upgraded is not opened by the release
  before. A writer of the release before that still holds the lake is
  refused at its next append once the upgrade's lease is taken (its
  fence is below it), so stop every older process before the first
  writer of the new release opens the lake.
- **A reader waits for the writer.** A read-only open of a lake not yet
  upgraded is refused before anything is read, naming the way
  (`log_upgrade_owed` in `docs/refusals.md`); so is a mirror of such a
  source, whose open is a reader's.
- **An export of an older release imports**, and the import upgrades
  the new lake the same way, by a writer's open after the events are in.
  Should that open fail, the events stay and a second import is refused
  as `import_not_empty`: the lake is whole, and the next writer's open,
  or `lakelands upgrade`, finishes the upgrade.
- **A format this release does not read**, older than 14 or newer than
  the release, is refused at the open in every mode, naming the lake,
  the formats read and the one written (`log_format`). The store reads
  the two ends of the log before it opens the lake's engine, so such a
  lake is left as it stands: its projection is not judged, and nothing
  of it is made, folded or changed. An import asks the same before it
  opens its target: onto such a lake it is refused the same way, and
  onto a lake it reads as `import_not_empty`, with no lease taken and
  nothing changed. The number steps only in a release that changes the
  format, and the release's notes say so.

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
  admits the records that hold every pair at every read door. That
  principal writes those records alone, another's being to its write a
  key that is free; a proposal, a verb above low risk and every change
  of shape still go through the host. `only` is a function
  of `Principal.id` within a lake: give each owner its own id, and never
  give one id `only` on some calls and not on others, since a retry is
  answered its first outcome before the grants are asked. A grant whose
  pairs cannot be applied refuses the reads that name the type until it
  is fixed. See `docs/decisions/051-a-grant-may-admit-only-some-records.md`.
- **What a host reads before it applies a migration.** The preflight's
  report says, each in one member: whether the apply is admitted
  (`lossless`: every value the migration keeps converts, every record a
  re-key moves takes a key of its own, and no live record breaks a
  constraint that refuses at ingest; it says nothing of what the
  migration removes); what goes (`dropped`: each removed field with
  the records that hold a value, each removed relation with its live
  links; the apply discards the values and retires the links, each by
  an event of its own, so a host that asks "is anything lost?" sums
  the counts, `MigrationReport::discards()` in Rust); what
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
- **A pattern's rows cost the starts they walk, not the type.** Rows in
  the starts' order, or ordered by the start's own stored fields, are
  walked a page of 256 starts at a time and stop at the page and one row
  more, and a cursor goes on from the start the page ended in, so a
  first page, a middle page and an ordered page each cost about a page
  of starts. A step `where` that keeps few records of the far type is
  walked from the starts until the page is full, and its last page walks
  to the end of them to know that nothing follows: a host that filters
  by the far record starts from the far type, filters there, and walks
  back (a relation's inverse). Rows ordered by a record a step reaches,
  and an aggregate over rows, are read whole, at most `max_graph_nodes`
  rows, and past it refused `too_large`. A chain's graph walks its starts
  a page of 256 at a time too, each step's records together, walks the
  whole chain again for each page of its answer, and past
  `max_graph_nodes` records is refused `too_large`; a neighbourhood
  reads only the starts its page reaches (lakelands#1027, lakelands#963).
- **A migration of a large type is a step for a quiet moment.** Applying
  one reads, converts and writes again every record of the type under
  the writer; other writes wait. The check before the append and the fold
  read the type a page of 1 024 records at a time. What the store still
  holds for the whole type is keys, not records: for a type with key
  fields the old and the new key of every record; for a re-key its moves,
  in the report's id, the event and the fold, and one membership for each
  moved record and set; the ends of the live links of a relation it
  narrows; the live links of a relation it removes, whole, as a purge
  holds what it retires; a set's members and its changes up to
  `max_set_changes`.
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
  down further is lakelands#480. A migration that removes a relation
  retires its live links in the same append, one event each, as a purge
  retires what it takes, and no limit bounds either: at 100 000 links,
  measured the same way, the apply holds the writer about 11 s on SQLite
  and 36 s on PostgreSQL, where the same migration with no link takes 3
  s and 5.5 s, and a purge of those links 10 s and 39 s.
- **A value is renamed by its value type's migration (lakelands#553).**
  `migrate_value_type` takes a `ValueTypeMigration`: the new declaration
  and `renames`, old and new text values (a bare declaration converts with
  `.into()` and renames nothing). One `value_type_migrated` renames the
  value in every record of every type that uses the value type, retired
  and restricted ones included, with no record event, no new version and
  no trigger: a host that kept its own copy of the values reads the feed's
  `value_type_migrated` (it is about each type it rewrites), and a mirror
  copies those types again. The preflight lists what may still name an
  old value (`conditions`): the host changes those, and the filters it
  holds itself, which the lake cannot see. A key field that holds the old
  value refuses it; re-key the type through its migration first. A field
  computed by a function with a backing keeps what its backing answered:
  widen the value type, change the function so that it answers the new
  value, and rename then. A rename calls a backing or the embedding
  backend once for each call that carries a renamed value, and for no
  other: a recorded failure is not called again by it, and `recompute`
  is the door for that (lakelands#857). Renaming one value of 50 000 of 100 000 records holds the writer
  2.1 s on SQLite and 8.2 s on PostgreSQL, at a peak of 3 MB and 6 MB
  (`tests/value_renamed.rs` in both engines, by hand); widening, writing
  the 50 000 records again and tightening took 17.1 s and 49.1 s, at
  39 MB.
- **A migration may fill a field's `initial` (lakelands#1023).** Each
  field named in `fill`, declared by `to` with an `initial`, takes that
  value in every live record that holds none, at the `migrated` event,
  of origin `initial`; the preflight counts them (`filled`). A type that
  held records before its field had an `initial` gets the value this way
  alone, and an aging rule reaches those records only after it. Like a
  value's rename the fill writes no record event and fires no trigger: a
  host that kept its own copy reads the feed's `migrated` (its `fill`),
  and a mirror copies the type again.
- **A rule over a record is a constraint the store holds
  (lakelands#1119).** A type's `constraints` are judged at every write
  door, an import, a source and a reactive write included, so a host
  drops its own check of the same rule. A write that would break one is
  refused `constraint_broken`, naming the constraint, the type and the
  key and never a value; the host shows its user the constraint's
  `description`. A constraint reads its record alone, and a comparison
  with an absent value is false, so a rule over an optional field says
  what it holds when the field is absent (`paid is null or paid <=
  amount`). A constraint is added or changed by a migration: its
  preflight lists in `breaches` each constraint live records would
  break, with their count and first keys. One that refuses at ingest
  makes the migration lossy, so the host fixes those records first; one
  declared `at_ingest: admit` applies, and lets a source go on writing
  what it states. A principal kept from a field a constraint reads is
  refused `not_granted` for every write the constraint judges, so a host
  grants a writer what the rule reads or keeps those fields out of its
  writes. A declaration that computes records again, a value type's
  migration and a re-key are refused `constraint_left_broken` where they
  would leave records breaking a constraint that refuses, naming it and
  the count, so the host fixes those records first. A constraint that
  admits is named in the value type's or the migration's report, in
  `breaches`, with its type. See
  `docs/decisions/059-a-type-declares-its-constraints.md`.
- **A host may state its declarations at every start.** A declaration
  that is the one in force is answered `applied` at the head and appends
  nothing, by `declare` and by `apply`: a type, a value type, a function,
  a verb, a trigger, an object set. A computed field may be stated with
  the inputs `describe` shows or without them, and a grouped view with
  its `key` or without it. A declaration's version counts its changes.
  The answer's `declared` names each declaration sent, with its
  `version`, the position it stands from (`since`) and whether this call
  `appended` it, so a host drops what it cached only for what changed
  (lakelands#1059). `lakelands apply` prints it: each declaration already
  in force says since which position, and a bundle wholly in force says
  that nothing was appended (lakelands#1106). See
  `docs/decisions/027-shape-changes-are-declared-or-migrated.md`.
- **A read costs the same however much the lake declares
  (lakelands#557).** A handle builds what the lake declares once for a
  range of positions over which nothing was declared, migrated or set,
  and every read in the range shares it. A declaration, a migration or a
  setting written starts a new range, and the next read builds again,
  once. A read as of an earlier position, and a read-only handle beside
  another process's writer, build once per position. On a lake of 300
  types, release build, one read at the head took on SQLite 0.03 to
  0.09 ms (before: 6.7 to 6.8 ms), on PostgreSQL 0.15 to 0.73 ms
  (before: 11.5 to 12.2 ms), and in memory 0.03 ms (before: 1.1 ms). A
  value that changes at every write belongs in a type of its own, not in
  `sys.setting`: each change of a setting makes the next read build.
- **A model that comes up after the open is bound then (lakelands#641).**
  A host whose embedding model answers later than it opens its lakes (a
  sidecar loading, a model downloading) opens them without one and
  calls `LakeStore::bind_embed` once the model answers; no reopen. Until
  then a due embedding waits, a hybrid search ranks lexically and says
  so, and a declaration that adds an embedding, or changes one's
  `from`, model or dimensions, is refused as `Unbound`; a type stated
  again as in force, or grown beside its embedding, is admitted, and one
  grown turns the embedding's recorded failures into waiting fields,
  their `error` gone, until the bind and `recompute`. The
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
- **A request names only what it takes.** `Find`, `Get`, a pattern, a
  search, the write options, a proposed write, a proposal filter and a
  feed filter refuse a member they do not take, at any depth, naming it,
  as a declaration does (lakelands#563): `find {"set": "open"}` is
  refused, where before it answered every record. A host that reads JSON
  into them, or sends a member a newer store would take, is told so
  instead of being answered a request without it. An answer that carries
  a proposed write or write options (a proposal listing, the feed, an
  export) is read as strictly by a client, so a client older than its
  server refuses a member the server added, as it refuses a new variant.
- **A write one level too deep is refused whole.** A reaction at a depth
  past `max_reactive_depth` appends nothing, the write included
  (`docs/decisions/021-the-write-context-belongs-to-the-host.md`). A host
  that comes from a store which landed such a write and dropped only its
  reaction raises `max_reactive_depth` at the lake's location, or sends
  the write outside the chain, at depth 0 with its `cause`.
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
  closed or dropped before a write records it (the server ends so): a
  retry is then `unanswered`, and the first answer carried the output.
  **A dropped connection on PostgreSQL** (a restart of the server, a
  failover, a pooler's or a firewall's cut, an operator's
  `pg_terminate_backend`) costs a read one reconnect: the read opens its
  connection again, once, and runs again (about 15 ms on a local
  server). The writer's lease lived in the session that ended, so the
  first write that finds it gone is answered `unavailable`, the lease not
  held, nothing appended, and a host sends it again once. A write
  answered `unavailable` without "nothing appended" lost its connection
  after its `COMMIT` was sent: it may have landed, and the host reads it
  back from the log (or sends it again under its idempotency key) before
  writing it anew. The next write takes the lease again as an open does:
  a greater fence, what another writer appended meanwhile folded, then a
  `lease_taken` in the log. An executor's answer the handle held
  survives this and is recorded by that write. While another writer
  holds the lease, every write, rebuild and compaction is refused with
  the lease's situation and appends nothing, and the handle reads on.
  The lake's connections carry `application_name` `lakelands <schema>`
  in `pg_stat_activity`. A host that needs more gives the executor its own record of what it did, keyed by
  the call's position (`VerbInvocation.position`). See
  `docs/decisions/013-verbs-run-by-declared-risk.md`, extended.
- **An outage and a missing seam are cases, not sentences
  (lakelands#650).** `unavailable` carries a `Situation` and `unbound` a
  `Seam`: a code and its parameters (which lease and who holds it, which
  part of the log is damaged and how, which remote end failed and in
  what exchange), with the English sentence beside them for logs. A host
  renders the case in its users' language from its own catalog, and
  acts on it without reading words: a call of a verb that reaches
  outside the lake failed around its hand-out is `Situation::Executor`
  (not handed, with a `retry` that says how to send it again),
  `AnswerHeld` or `AnswerLost`. Over REST and MCP the error's
  JSON carries `code`, the parameters and `message`, and the error its
  own `message`; a code the host does not know yet, from a newer lake, is
  read as `Unknown` and shown by its message, and a plain name it does
  not know is shown as itself in backticks. A client and a lake on either
  side of this change cannot read each other's errors: move both
  together. The only free text is a `Detail`, an operating system's,
  database's or library's own words, scrubbed where they arise: never a
  record's key or values, and never a path on the host. See
  `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
  extended.
- **A refusal is a case too (lakelands#650).** A refusal of `invalid`,
  `constraint`, `precondition` or `unanswered` carries its case as data:
  `code`, its parameters, `at` (the places it is said at, outermost
  first: a type, a field, a verb, …) and `message`. The catalog of every
  code, its parameters and its English sentence is
  [`docs/refusals.md`](refusals.md), generated from the code; a host
  renders from it, matches codes rather than words, and shows a code it
  does not know by its `message`. A code a release has shipped keeps its
  meaning and its parameters. A thing the caller may not see is refused
  exactly as an absent one, case and parameters alike. See
  `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
  extended.
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
  record once, at the set's last declaration: at 2 000
  records of 52 fields, a set of 50 such functions takes 0.56 ms a
  record on SQLite, where computing it again at each declaration took
  24.6 ms (lakelands#605). A read at a position inside the set finds
  the records as before it. At 100 000
  records of four small fields, one of them derived
  (`tests/recomputed.rs` in the SQLite and PostgreSQL engines, by
  hand), the writer is held 4.1 to 4.5 s on SQLite and 13.3 to 14 s on
  PostgreSQL, whichever of the two is declared again, and about 20 s
  and 51 to 56 s at 400 000; the peak of the append is 3 MB on SQLite
  and 9 MB on PostgreSQL at both sizes. Before that, a bundle of
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
- **An order index is the host's to ask for, from what was ordered
  (lakelands#1029).** A page ordered by a field `sort_index.<type>` does
  not name sorts the type, on every page: 0.6 s on SQLite and 1.0 s on
  PostgreSQL at 100 000 records, and far more for a deep page by offset.
  Naming a field costs every write about 4 % and one rebuild, about a
  second at that size. The lake does not choose: `describe_type` tells a
  holder of `schema`, in `sorted_whole`, each field it reads that was
  ordered by a whole sort since the lake was opened, with its pages and
  their time, and `lakelands serve`
  prints a line when one grows. A host's rule, run as often as it likes,
  names a field when it was ordered by on a few pages of a large type,
  and keeps a type's list short:

  ```text
  for each type with at least 2 000 records:
    for each field in sorted_whole with pages >= 3 and millis >= 100,
      while sort_index.<type> names fewer than 4 fields:
        add it to sort_index.<type>
  ```

  Write the setting in a quiet moment: its fold rebuilds the type's
  rows under the writer. See
  `docs/decisions/041-a-sorted-page-reads-its-page.md`.
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
    or a member whose argument does not fold. `describe_type` then says
    `not_whole` with the number of such groups, to a principal the
    view's read would answer.
  - **A write that takes an extreme's holder away reads the next one**
    from the extreme's order, in its own append: one entry, whatever
    the group's size, and a batch more for each run of entries the same
    append ended ahead of it. For a view with a `min` or `max`, the fold
    keeps that order beside the kept records, an entry for each member
    of each extreme, and every write to a member puts its entry again
    where its value or group moves. Making the view writes one entry for
    each member of each extreme and holds about 270 bytes for each until
    it commits; making it again writes as many, and ending it deletes
    its order in one statement. On SQLite a `max` over 10⁶ members was
    made in 28 s at a peak of 549 MB more and ended in 7 s, and a
    declaration of a source, which does both in one event, holds the
    writer about 35 s: declare in a quiet moment.
  - **A reader with the `restricted` grant** reads the groups whose
    faces differ (a restricted record is a member of each) beside the
    engine's page of the others (lakelands#628): a page in key order
    holds its rows and those groups, a count is the engine's and those
    groups, and `get` reads its one record. For every other reader, and
    for that one while no group differs, the page and the count are the
    engine's alone. A page in another order reads the engine's page too
    where `sort_index.<view>` asks for an order index on that field,
    below; without it, it reads the whole view for every reader
    (lakelands#668). An aggregate other than a count and a pattern's
    start read every group a page of kept records at a time, holding a
    page, not the view (lakelands#731).
  - **A kept view is indexed by its setting:** `sort_index.<view>`
    names fields the view declares, and a page ordered by one of them
    reads its page (lakelands#668). At 100 000 groups a page of 10 by a
    measure took 0.2 ms on SQLite and 1 ms on PostgreSQL where it took
    about 400 and 500 ms and held some 465 MB without the index. Each
    write that changes a kept group writes its rows too: some 5 to 8 %
    more on an ingest. The setting's write builds the index in one pass,
    0.8 s and 3.9 s at that size. A view that computes has no records
    to index, so `describe_type` names no indexed field for it
    (lakelands#748).
  - **Not every view can be kept:** only counts, sums and averages of
    what is certainly a number, and a `min` or `max` of a number or an
    instant under text or reference group fields. The setting's write is
    refused otherwise and says which measure. `describe_type` says for a
    view whether it is kept, and why not.
- **A view's measure lands in its field's kind, or the view is not
  read.** A measure whose value its field would not take written, a sum
  past ±(2^53 − 1) into `number` or an instant out of the years 0000 to
  9999, refuses every read of its view, a read of another group too,
  naming the view, the measure and the group. So the day a sum outgrows
  `number`, its view answers nothing until the measure is declared
  `decimal`, which answers its digits: declare a sum that can grow so
  as `decimal` from the start. A kept view with such a group says
  `not_whole` and is computed, and so refused. The refused range ends
  at 10^21: from there a number is written with an exponent and read
  as a double by everyone, so a value of that size lands as its nearest
  double, in a view's measure, a derived field and a verb's set alike,
  and is no longer exact to the unit.
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

