# 038 — A lake in PostgreSQL

## What is decided

The PostgreSQL engine keeps one lake in one schema of one database.
The lake's location names the connection, the schema and the TLS the
connection uses (`EngineConfig::Postgres`). Nothing is read from the
environment. The driver reads no libpq variables (`PGHOST`,
`PGPASSWORD` and the rest) and no `.pgpass` file. The URL names the
server, the database and the credentials, the user included: a URL
naming no user is refused, since the driver would take the operating
system's. It says nothing about TLS: a URL
carrying `sslmode`, `sslrootcert`, `sslcert` or `sslkey` is refused, since
the mode and the certificates come from the configuration alone.

**TLS** is rustls. With `require`, the default, the server's certificate
and name are verified against `ca_file` or the first system bundle found
at a well-known path, and a client certificate is presented when one is
configured. With `disable` the connection is plaintext.

- **One schema per lake, every table qualified.**
  - The schema is `lakelands_<lake>` unless the location names another.
    Its `lake` row names the lake it holds, and opening it under another
    name is refused, as a lake directory refuses it (030).
  - The schema's name is always quoted, so it is taken exactly as
    written. A name longer than PostgreSQL's identifier limit (63 bytes)
    or holding a character an identifier cannot hold is refused, never
    shortened.
  - Every statement names its tables with the schema. No connection's
    `search_path` is set, so no statement depends on which session runs
    it.
- **The log is kept as the format's lines.**
  - An event's canonical bytes are stored unchanged, one row per event,
    with the position, id, `prev`, `hash` and fence beside them, and the
    first position of the segment it belongs to.
  - A segment's digest is the SHA-256 of its events' bytes, each followed
    by one line feed: the digest of the file the format describes
    (`docs/log-format.md` §4.2).
  - `lakelands_engine_pg::export` writes a lake's segments as those files,
    and they verify with nothing but the format.
  - An append is one transaction: it commits every event or none. It
    checks the fence against the lease row in the same transaction.
- **The lease is a row and a lock.**
  - The row holds the last fencing token handed out and its holder.
  - Taking the lease takes a session-level advisory lock without
    waiting, and writes a token one greater than the row's in the same
    transaction. The lock's key is a 64-bit hash of the schema's name.
    The writer's lease connection keeps the lock for as long as it lives.
  - A lock that is held while the lake's row names no holder is either
    the tail of a session that is ending, or another schema whose name
    hashes to the same key. The open is refused and says so, naming both
    causes.
  - The server releases the lock when that session ends, however it
    ends, so a writer that died never keeps the lake from the next one.
    The next writer finds the lock free and the row still naming the dead
    holder. It takes the lock and advances the token. The dead holder, if
    it were ever heard from again, holds a token the row no longer
    records, and every append and seal it tries is refused: both check the
    row's fence inside their own transaction, with the row locked until
    it ends.
  - The lock keeps two writers out; the token refuses a stale one (008).
- **Sessions must keep their state.** The lease is session state.
  - A handle opened for writing first probes: a setting made on its
    lease connection must survive to the next statement, and two fresh
    connections must not inherit it. A transaction-mode pooler fails one
    test or the other, and the open is then refused.
  - The probe cannot see every pooler. One with enough idle server
    connections, and no reset between clients, can pass it.
  - So the host must connect the engine directly or through a
    session-mode pooler. A transaction-mode pooler in front of a lake is
    a misconfiguration the probe catches when it can, not one the engine
    survives.
- **Isolation.**
  - The fold writes the projection in one `READ COMMITTED` transaction on
    the writer's projection connection, the only connection that writes
    it. It commits with the position folded through, as on every engine.
  - Every read of a view runs in one `REPEATABLE READ READ ONLY`
    transaction, so it sees one committed state of the projection,
    whatever commits beside it.
  - They differ for a reason. The fold has no one to race: one writer,
    one connection, and its reads are its own. A read shares the database
    with that writer, and one read may take several statements; under
    `READ COMMITTED` each statement could see a later commit than the one
    before it, and a record could be read at one position and its type at
    another.
- **The rulebook decides.** The projection keeps its history as every
  engine does (states valid from `since` until `until`), recorded
  outputs included. A later output for the same call, a failed call made
  again after its function is declared again (lakelands#76), supersedes
  the earlier from its own position on: the head answers the latest, and
  an earlier view what was recorded by then. The head indexes
  only find candidates:
  - exact text values;
  - links;
  - full-text tokens, one row per pipeline token of a record;
  - vectors, compared by brute-force cosine.

  Selection, order, scoring and ranking are `lakelands_core::engines`'
  (031), so an answer is the same on every engine.
- **The server is PostgreSQL 16 or newer**, the release the conformance
  job runs. The engine uses nothing a newer release adds. The floor is
  checked when a handle opens, before anything is created.

## Why

A host that already runs PostgreSQL should be able to keep its lakes
there without giving up anything the log promises. The log's bytes are
the truth (001, 020), so they are stored as they were sealed, and a lake
can leave PostgreSQL as the files every engine and every verifier reads.
Schema-qualified statements and an explicit lock keep one lake from
reaching another's tables through a shared or reused session. The
advisory lock is the database's own answer to the question the lease
file answers on disk: it dies with its holder.

## What it is not

- It is not a second fold. The engine keeps what the store hands it and
  decides nothing about what an event means.
- It is not pooler-safe in transaction mode, and does not pretend to be:
  a session-mode pooler or a direct connection works; a transaction-mode
  one is refused when the probe detects it, and must not be used when it
  does not.
- It does not use `pgvector`, PostgreSQL's text search or its own
  ranking. Those would decide matches and orders the other engines
  decide differently.
- It does not share a lake across schemas, or a schema across lakes.
