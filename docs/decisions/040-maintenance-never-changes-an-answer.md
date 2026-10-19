# 040 — Maintenance never changes an answer

## What is decided

`LakeStore::compact()` gives back the storage a lake's projection no
longer uses. It is host-triggered and engine-local: the host calls it
when it chooses, and each engine does what its storage needs.

- **SQLite** truncates the projection's write-ahead journal, rewrites
  `projection.db` without its free pages (`VACUUM`), and truncates the
  journal again.
- **PostgreSQL** runs `VACUUM (ANALYZE)` on the projection's tables,
  outside a transaction.
- **Memory** has nothing to give back.

A compact runs on the writer's handle alone, under the writer's lock.
A write that arrives meanwhile waits and lands after it. A handle opened
read-only refuses it as unavailable.

A compact changes nothing a caller can observe:

- **No answer.** Every read at the head and as of every earlier position
  answers as before, byte for byte: records and their provenance,
  lexical and vector search, walks, views, proposals, recorded outputs
  and the ids a retry is recognised by.
- **No event, no hash, no export byte.** A compact appends nothing, so
  the head, the feed, the chain, `verify`'s answer and an export are as
  they were. It leaves no trace in the log: it is maintenance of storage,
  not a fact about the lake.
- **No content.** Kept history, the applied ids and recorded outputs are
  content, and a compact never prunes them.
- **It does not seal.** Sealing appends a `checkpoint`, which is an
  event. A host that wants both calls `checkpoint`, then `compact`.

**A compact that fails leaves every answer as it was**, on every engine.
On SQLite, `VACUUM` is one transaction: a reader in another process that
holds a snapshot past the busy timeout makes it fail as unavailable,
naming the reason, and the projection is as it was. One that holds it
after the rewrite keeps the journal, and the freed pages in it, from
being truncated: the compact answers busy too, the rewrite stands, and
the next compact truncates (lakelands#1042). PostgreSQL's
`VACUUM` cannot be rolled back, but at no point does it change a row a
read sees. So "nothing changes" is a sentence about answers, not about
pages on disk.

The report carries the projection's storage before and after, and the
head, unmoved. The sizes are the engine's own measure: the file and its
journal on SQLite, the tables' total relation size on PostgreSQL, and
none on the memory engine. They are not a promise. Only each engine's
tests hold anything about them. The conformance suite holds the answers,
the log and the export.

## Why

A projection only grows in storage. A rebuild clears it and folds it
again, a migration rewrites records, and every write churns pages. SQLite
keeps its free pages and truncates its journal only when an
autocheckpoint happens to run; PostgreSQL waits for autovacuum's own
schedule. A host knows its quiet hours, and the lake does not, so the
host gets an explicit call.

The projection is a function of the log (001), so tidying its storage
can never be allowed to change what it answers. Keeping compact outside
the log is what makes that checkable: before and after, every read, the
feed and the export are compared byte for byte, on every engine.

## What it is not

- **Not log compaction.** Nothing leaves the log, which is the truth
  (001).
- **Not history pruning.** A read `as_of` any position still answers. A
  lake that wants less history needs a different operation, with a
  record of its own.
- **Not sealing.** That is `checkpoint`.
- **Not scheduled.** `serve` does not compact on its own. A daily
  compaction would be a later option with its own configuration.
- **Not `VACUUM FULL`.** It takes an exclusive lock and would lock every
  reader of the lake out while it rewrote.
- **Not a promise for a pgvector index.** A vector field the lake asks an
  index for answers the index's set, and PostgreSQL's `VACUUM` repairs
  pgvector's graph after deletes, so a compact, and autovacuum, may move
  that set. Its recall against the reference holds through it. This is
  the one exception, and it covers pgvector-indexed fields only
  (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
- **Not a door.** It is the host's act, beside `rebuild`, `checkpoint`,
  `age` and `purge_source`, with no MCP tool and no REST route. A
  `lakelands-client` answers it as unbound, naming it.
- **Not hidden inside another call.** The engine's part is
  `ProjectionStore::compact`, a method of its own; the seam grows by it
  and by `LakeStore::compact`. No existing engine call means "maintain
  storage", and each candidate would carry it badly:
  - in `seal_segment`, every `checkpoint` would pay for a `VACUUM`, and
    the log's policy would govern the projection's storage;
  - in `rebuild`, a host that only migrates would never reclaim;
  - in `close`, shutdown would turn slow, and a process killed while
    closing would leave the work half done.
