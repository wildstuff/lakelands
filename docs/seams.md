# Seams

Reviewed so that a third engine can be written from this page, the
`lakelands-api` crate and the engine checks (lakelands#15). Where the
page and the code disagree, the page is right and the code is a bug.

A seam is a trait in `lakelands-api` through which Lakelands reaches
anything outside its deterministic core. The rule that decides what is a
seam is stated once
(`docs/decisions/002-determinism-is-the-boundary.md`): **whatever must
reproduce byte-identically from the log lives in the store; whatever
needs the outside world sits behind a seam.** The store calls a seam,
records its output in the log (`output_recorded`, see
`docs/log-format.md`), and on replay reads the recorded output instead of
calling the seam again.

A seam never holds an invariant. Field rights, per-field provenance, the
`sys.` guard, idempotency and the writer lease are enforced by the store
on every call; no seam implementation can relax them, and none needs to
enforce them.

## The roster

Ten seams, this roster and no other. Adding one is a change to the
contract and to this page.

| Seam | Trait(s) | Default in Lakelands | A host plugs in |
|---|---|---|---|
| Engine | `LakeEngine`, `ProjectionStore`, `ProjectionView`; `VectorIndex` beside them | SQLite and in-memory (`0.1.0`); PostgreSQL (`0.2.0`) | Rarely anything: a new engine, proven by the conformance suite |
| Principal resolution | `PrincipalResolver` | API key or static entry → one principal | Its identities, roles and policies, compiled to grants |
| Function backings | `FunctionBacking` | `expr`, evaluated in the store | Its own backing kinds; `http` and `exec` ship in `lakelands-backings` |
| Embeddings | `EmbedBackend` | None: lexical search and caller vectors | An embedding client; `lakelands-backings` ships one for `/v1/embeddings` endpoints |
| Reranking | `Reranker` | None: a search that asks for the second stage is refused as `Unbound` | A cross-encoder or any scorer of (query, text) pairs |
| Verb execution | `VerbExecutor` | None: external verbs are refused as `Unbound` | What its external verbs do |
| Change feed | `ChangeFeed` | The log, read by kind from a position | A relay of the feed onto its own bus, as its own code |
| System vocabulary | `SystemVocabulary` | Lakelands' four `sys.` types, allowed in every lake | Its own `sys.` types and which lakes may hold them |
| Keys | `KeyStore` | None: a source's value of an erasable field is refused as `Unbound` | Its KMS; `lakelands-backings` ships `FileKeyring`, a key file per lake |
| Measurement | `Meter` | None: nothing is measured and no clock is read for it | Its metrics system; `lakelands-server` ships `Metrics`, which `serve` exports at `GET /metrics` |

`Location` is not a seam. It is the configuration every seam and every
engine is configured through, passed at the call site; Lakelands reads no
environment (`docs/decisions/005-no-environment-reads.md`).

The seams do not all point the same way:

- **A host implements and passes in** the engines (`Seams::engines`),
  function backings, an embedding backend, verb executors, a system
  vocabulary, a key store and a meter. Every slot but `engines` may stay empty and has a default;
  `engines` needs at least the opener of the engine a lake's `Location`
  names, and Lakelands' own openers are what a host usually passes.
- **The doors hold** the `PrincipalResolver`: the CLI and the server
  turn a credential into a principal before they call the store.
- **The store implements** the `ChangeFeed`, and a host reads it. A host
  implements `ChangeFeed` itself only to relay the feed elsewhere.

A host that passes Lakelands' engine openers and nothing else has a
working store.

## Engine

An engine keeps one lake: its log, its writer lease, its projection and
its vector index. It stores and returns; the store's core decides.

Two rules shape the engine seam:

- **The store canonicalises; the engine keeps bytes.** The store turns
  every event into its canonical bytes and hash (`docs/log-format.md`
  §1.2), writes them in the stored form, and hands the engine a
  `CanonicalEvent`: the event's line, which leaves out what its frame
  holds and what the event before it gives, and `frame`, the frame its
  append shares, if any (its line, its first position and how many
  events it covers, one value for every event it covers), with the
  position, id, `prev`, `hash` and `fence` beside them so the engine can
  check an append without parsing. The engine stores the line and the
  frame and gives back the same `CanonicalEvent`. The store restores the
  events and verifies what it reads; a segment's digest is over the
  events restored (§4.2). Byte identity is checked through this seam by
  the conformance suite, frames included.
- **The projection keeps its history.** Every `put_*` adds a state valid
  from a position and keeps the state it supersedes. `view(as_of)` is a
  filter over kept states, never a replay: "what did the agent see at
  position N" is a cheap read on every engine. The conformance suite
  reads each record at every position it changed and compares with a
  fresh replay to that position.

The traits are synchronous and `dyn`-compatible: storage work blocks, and
the store runs an engine off its async callers. Where this page names a
function of `lakelands-core` as the definition of a behaviour, the
engines in this repository build on it. Those functions are
`#[doc(hidden)]`, and no release promises them. An engine outside the
repository holds to this seam and the conformance suite. Every promise
below is checked on every engine on the roster, either by an engine
check through the seam or through the doors' suite; "Where each promise
is checked" at the end of this section says which, and names the
promises that have no check yet.

**Errors.** An engine answers with three kinds of error and no other:

- `Unavailable` for anything that is not the caller's mistake and may
  succeed later: the lease is held, a lease is stale, the handle was
  opened read-only, the lake does not exist, the storage failed.
- `Refused` as `Invalid` for a request that can never succeed as it is:
  an append that does not continue the chain or carries a wrong fence, a
  commit that would move the projection backwards, a vector of the
  wrong length, a location configured for another engine.
- `Refused` as `OutOfRange`, naming the head, for a view at a position
  the projection has not folded.

### `LakeEngine`: the log and the lease

| Method | Contract |
|---|---|
| `name` | The engine's name, as `EngineConfig` selects it. |
| `rebuild_rate` | The events a rebuild folds a second on this engine, as measured: the store tells a host from it how long an act that rebuilds holds the writer's lock (`forget_preflight`, `docs/decisions/058-a-source-may-be-forgotten.md`). A constant of the engine, measured on a plain type, never a promise: it stands only until the handle has rebuilt once, after which the store reads the rate that rebuild measured (a `rebuild()`, an act's, or an open's that rebuilds). |
| `lake` | The lake this handle keeps. |
| `take_lease(holder)` | Take the writer lease with a fencing token greater than every token this lake ever handed out. **Fails at once** while the lease is held — by another process, another handle, or this one — with `Unavailable` whose message names the holder; never waits. `Unavailable` on a handle opened read-only. |
| `release_lease(lease)` | Give the lease up; the next taker gets a greater token. The lease is always released; an engine that also records the holder in storage reports a failure to clear that record as `Unavailable`, after releasing. Releasing a lease that is no longer the current one does nothing, so a writer that lost the lease cannot release its successor's. A handle that is dropped releases the lease it took and still holds, so a writer that ends without closing does not keep the lake from every later writer. |
| `lease_free()` | Whether no handle holds the lease now, asked of any handle, one opened read-only included; it takes and changes nothing, and the answer is stale once given: a take made on it may still be refused as held. A standby asks it before it opens the lake for writing (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`). The default answers true: a standby over an engine that cannot tell tries the take itself. |
| `tip` | The last event's position and hash, or none for an empty lake. |
| `append(lease, events)` | Append a group of `CanonicalEvent`s, all or none, **durably before returning**: once it returns, the events survive a crash of the process and of the machine. `Unavailable` when `lease` is not the current lease, when this handle did not take it (another handle's lease, copied, appends nothing), or when the handle is read-only. `Invalid` when an event's fence differs from the lease's — except the event at position 0, which carries fence 0 — when the positions do not continue the tip without a gap, or when a `prev` is not the hash before it; `Invalid` as `append_frame`, naming the position, when the group does not hold each frame its events share whole and in order, from the frame's first position to its last; nothing of a refused append is kept. An append that answers `Unavailable` because a write failed keeps nothing either: the handle goes on under its lease, and its next append is not written behind what the failed one left. An empty group appends nothing. Stores each event's bytes and frame unchanged and never parses them. |
| `read(from, limit)` | At most `limit` `CanonicalEvent`s from `from` on, in position order, exactly as appended: the same members, the same bytes, each with its frame, also where the read begins inside one. Empty when `from` is past the tip or `limit` is 0. |
| `import(lease, events)` | Append events carried over from another lake's log, as an import does (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`): as `append`, all or none and durably, except that each event keeps the fence it was written under, 0 at position 0 and never below the fence of the event before it, instead of carrying the lease's. `Invalid` unless the log holds nothing but imported events (it is empty, or every event in it came through `import`), so a lake that was ever appended to is never imported into. Afterwards the next `take_lease` answers a token greater than every fence imported. The store checks, before it imports, that every fence is its writer's (log format §4.1). |
| `seal_segment(lease, imported, first)` | Close the active segment at its last event and begin the next with the events `first` answers for the closed segment's range and digest, **in one step: both hold, or the log is as it was.** A segment after the first therefore never exists without its first event, after a failure and after a crash (`docs/log-format.md` §4.2; lakelands#623). The events enter as `append`'s do, or as `import`'s when `imported`: events that do not continue the log, imported events behind appended ones, and none are refused as `Invalid`, an error from `first` is answered as it is, and nothing is sealed. `first` is asked once, after the lease is found current, and **runs inside the step: it calls nothing of the engine**, which holds its log while it runs. None, and `first` is not asked, if the active segment is empty. Refused as `append` is for a stale lease, a lease this handle did not take or a read-only handle, before `first` is asked. The store's `checkpoint` hands the `checkpoint` event so; an import hands each later file's first events, after it has compared the digest with that file's. |
| `segments` | Every sealed segment's range and digest (`docs/log-format.md` §4.2), oldest first; the ranges start at 0 and follow each other with no gap. |
| `projection` | The projection's write side. |
| `view(as_of)` | A read view at `folded_through` (`None`), or as the projection was right after folding `as_of`, served from kept states. `OutOfRange`, with `folded_through` (0 when nothing is folded) as the head, for a position past it. A view keeps answering its own position while the fold moves on. |
| `ranked(type, asked, k)` | The rankings of one pull of a search, all read from one state of the head's indexes, with the position the projection is folded through there (§`ranked`, below). |
| `vectors` | The vector index's write side, if the engine keeps one; a search that needs vectors is refused as `Unbound` on an engine that keeps none. |
| `mark_built` | On a handle `open_scratch` opened: mark the scratch built through the position it is folded through, with the log's hash there, so it outlives the handle (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`). An open transaction is abandoned. Answers the position marked; none, and no mark kept, for a scratch that folded nothing. `Unavailable` on any other handle (`no_scratch`) and where the log does not hold the position (`scratch_behind`). A scratch folded on and not marked again is removed when its handle is dropped. The memory engine refuses as `Unbound`. |
| `swap(lease)` | Put the scratch in the projection's place in one step, once it is checked: this handle holds `lease`, no handle has the scratch open (`rebuilding`), the scratch is this lake's, laid out by this build, marked built and still folded through its mark (`no_scratch`), and the mark is the log's tip and its hash (`scratch_behind`). A refused swap leaves the projection and the scratch as they were. Afterwards this handle reads and folds the projection it put in place, and the one replaced and the scratch are gone. An open transaction is abandoned, by a swap refused as well as one done. Answers the position folded through. SQLite renames the scratch over `projection.db` under an exclusive lock on the lake's name file, which every other handle onto the projection holds shared from before it connects: the swap is refused `swap_busy` while one does, and a handle that opens during it waits and connects to the file put in place (a release before this one takes no such lock, and is stopped before a swap). PostgreSQL moves the scratch's tables into the lake's schema in one transaction, dropping the projection's and any a layout retired; a statement reads the old projection or the new one, and a transaction that reads across the swap may read tables of both, as across `clear`. The memory engine refuses as `Unbound`. |

**The lease across processes.** An engine that keeps a lake in files
holds the lease with a lock the operating system keeps, on a file that
records the last fencing token handed out and who holds it. The lock
guarantees two things: a take while it is held fails at once, never
waits; and it is released by the operating system when its process ends,
however it ends, so a crashed writer never keeps the lake from the next
one. The lock alone does not refuse a stale writer: the fencing token
does. Every `append` and `seal_segment` is checked against the lease the
handle holds and against the token the file records now, so a writer
that released its lease, or lost its lock without knowing, is refused. The SQLite engine keeps `lease` in the lake's
directory this way (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).

**What a SQLite handle holds.** A handle does not hold the log's index.
Per segment it keeps the segment's first position and how many events it
committed, where their bytes end, its last event's hash, and where its
last read ended: its memory does not grow with the lake, and a handle
opened read-only in another process holds the same. **An open reads the
end of each segment's index,** the sealed segments' and the active
one's: its last 64 kB, and in them the event line before the last commit
line, which names the last committed position, where its bytes end and
its hash. It reads a block further back only while it has met no commit
line, which only a group a crash left uncommitted makes it do. What an
open costs therefore follows the number of segments (a `checkpoint`
seals one), not the events in them (lakelands#564). Afterwards a handle
looks for new lines in the last segment's index alone: a segment it has
read to its end while a later one existed, in a refresh that did not
fail, is sealed, and its index is not opened again, so a read, a `tip`
and an append list the log's directory and do not open a file for every
segment. **The ceiling of a read:** a read by position
halves the committed index by byte, one read of 8 kB a step (each event
line names its position, so the first one after the middle says which
half holds the line wanted), until the line before the one wanted is
within 8 kB; then it reads one event line per event, in order, with the
commit lines between, and checks that each is the position that follows.
The line a read starts at is always reached so, from a line before it
(or it is the segment's first, named by the file): a line that names
another position is never taken for the one a read asks for. A read that starts
where the segment's last one ended begins at that line with no search,
so a page of the store's sequential reads costs its own lines. The log's
files stay as `docs/log-format.md` §4 describes them; an index line that
is short, unreadable or not the position expected is refused as a
damaged index, never read as another event. **A damaged line is found
by the read that meets it,** and so by `verify`, which reads the log
from the first position it is asked for; an open reads the index's end
and the active segment's last group, and finds there a line that names
another position; a line that is no JSON there, or past what is
committed with no commit line after it, is a crash's tear. **An append that fails after it wrote,**
as on a disk that fills, reads what is committed and cuts the index and
then the segment back to it before it answers its error, and every
append and every seal begins the same way, whoever left bytes there: no
event is written behind bytes that are none, and a sealed file holds
its events' lines and nothing else (lakelands#618). The cut is the
writer's: the lease's check and what follows it are one step under the
handle's own lock, and the lease file's lock lets no other handle take
the lease meanwhile. An append writes both files, then syncs them at
once, so a crash can leave the last commit line ahead of its group's
bytes: every handle checks that group against its hashes as it first
reads the active segment, reads the log as ending before one that is not
whole, and the writer cuts it off
(`docs/decisions/060-an-append-syncs-its-two-files-at-once.md`). Any
other commit line that is whole is never cut, since another handle may
have read its group, so one outcome is not known: where a sync after the
commit line fails, the group is in the log for every handle, the
writer's too, and may not outlast a crash; its append answers
`unavailable` and says so. A segment that ends before an earlier group
its index commits is damage no cut mends: the append, the seal and the
next writer's lease are refused, naming the file, before either file is
cut. **A seal is one rename.**
It writes the next segment's index whole (its event lines and their
commit line) and syncs it and its name, then the segment's lines as
`log/<first>.jsonl.partial`, synced, and renames that to
`log/<first>.jsonl`. A handle, or a verifier of the directory, that
lists `log/` before the rename finds the log as it was; one that lists
it after finds the segment with its events committed in an index that
was whole before the segment's name existed. A seal that fails before
the rename removes its two files and answers its error. A crash there
leaves them: no handle lists either, a seal at that position writes
over both, and the next writer removes them as it takes the lease (a
read-only open changes nothing; a leftover that cannot be removed is
passed over and refuses no lease, and a listing with an entry that
cannot be read fails, since a segment passed over would lose its index).
After the rename the seal stands, and an error says so: where the sync
of the name fails the outcome is not known, as for an append, and where
the handle then cannot read on it says that the segment is sealed. A
process that ends between the rename and that sync leaves a name nothing
syncs again
(lakelands#623; the engine's `tests/sealed_in_one_step.rs`). **The journal:** a writer
that gives its lease up (`close`) truncates the projection's journal,
`projection.db-wal`, which otherwise stays as large as it once grew
until a `compact`. It does not wait for a reader that holds a snapshot
of the journal, and leaves it then for the next release. The engine's `tests/log_index.rs` holds these: what an open
answers after each thing a crash leaves, reads from every position and
across a seal, the bytes an open and a read take from the files, the
read calls a handle makes beside many sealed segments, and the journal.

**What only PostgreSQL decides.** The PostgreSQL engine
(`docs/decisions/038-a-lake-in-postgresql.md`) keeps a lake in one schema
and every statement names it; no connection's `search_path` is set.

- **The lease.** It is a row holding the fencing token and a
  session-level advisory lock, keyed by a 64-bit hash of the schema's
  name and taken without waiting. The server releases the lock when the
  holder's session ends, however it ends. The next writer then advances
  the token, which fences the dead one.
- **The pooler probe.** A handle opened for writing first checks that
  its session keeps what it sets and inherits nothing from another, and
  refuses to open when it detects a transaction-mode pooler. The probe
  cannot see every pooler, so a host connects directly or through a
  session-mode one.
- **Isolation.** The fold writes in one `READ COMMITTED` transaction on
  the only connection that writes the projection. A view reads in one
  `REPEATABLE READ READ ONLY` transaction, so its statements see one
  committed state.
- **Statements.** Each connection prepares a statement the first time it
  runs it and keeps it, by its text, until the connection closes, so a
  statement run again costs one round trip. The fold's puts are held
  until its commit and written then: one statement per history table
  for the whole transaction, one that deletes the head index rows of
  the records it changed and one that inserts their new rows; a
  declaration is put as it comes. An append's events go into the log in
  one statement. A read of one statement runs alone, one round trip; a
  longer read's transaction opens with the folded position in the same
  round trip. The ceiling is latency on the reader: the store reads each
  record an ingest writes once, and its applied id once, so an ingest
  costs about two round trips per record there and about fifteen per
  window on the other two connections.
- **The log.** It is kept as rows holding each event's line of the
  stored form unchanged, and each frame once, in `frames`, keyed by the
  first position it covers (a lake made before frames gains the table at
  its next writer's open), and an append is one transaction that checks the fence
  against the lease row. `lakelands_engine_pg::export` writes the
  segments as the §4.2 files, which verify without the engine, for a
  lake whose store cannot be opened; they are the bytes the store's
  export writes. It is a raw copy of the log's lines and judges none of
  them, not their format either: what it writes is judged by whoever
  verifies or imports it.
- **The connection.** It comes from `EngineConfig::Postgres`: the URL,
  the schema, `tls` (`require` by default, rustls, verified against
  `ca_file` or a system bundle; or `disable`) and the timeouts. A URL
  carrying TLS keys, or naming no user, is refused. The driver reads no
  libpq variables and
  no `.pgpass`.
- **The server** is PostgreSQL 16 or newer.

An engine is found by name and opened through `EngineOpener`:

| Method | Contract |
|---|---|
| `name` | The name the engine answers to in `EngineConfig`. |
| `log_ends(location)` | The first and the last event of the lake's log, read without opening the lake: nothing is made or changed, in the log's files or tables or in the projection, nothing of a lake is judged, and a place with no lake stays with nothing (no directory, file or schema). None where there is no lake or its log is empty. An engine may refuse a place that holds something other than a lake, as its `open` does: PostgreSQL refuses a schema with tables that are not a lake's (`schema_not_a_lake`) or whose `meta` or `lease` table is not the one it wrote (`schema_meta_foreign`). Part of a lake without its events is not a place with no lake: SQLite refuses a segment without its index file (`index_missing`), a projection with no segment file (`no_segment_file`) and segments that do not follow each other (`position`, `prev_hash`) as `damaged`, and PostgreSQL answers `storage` for a schema whose `log` table is gone. The store asks this before every `open` and before an import, and opens only a lake written in the format it reads: it judges the first event's `v`, the format its `lake_created` says and the last event's `v`, and refuses another as `log_format` without calling `open`, in every mode; an import onto a lake of the format it reads is refused as `import_not_empty` from the tip, without calling `open` either (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`). A tip that moves between the two calls is harmless: its writer passed the same judgement, so it wrote the same format. The disk engines read the log alone: SQLite its files, PostgreSQL its table over a connection of its own. |
| `open(location, mode)` | Open the lake `Read`, `Write` or `Create` (write, creating it if absent, opening it if present). `Read` or `Write` of a lake that does not exist is `Unavailable`; a location configured for another engine is `Invalid`. Opening never takes the lease; a writer takes it afterwards with `take_lease`. A handle opened `Read` refuses `take_lease`, `append` and `seal_segment` as `Unavailable`, and writes nothing to the projection (`begin`, the puts, `clear` and `compact` are `Unavailable`), so any number of readers can hold the lake while one process writes (`docs/decisions/008-one-writer-per-lake.md`). A read-only open of a lake that does not exist creates nothing. The store folds only on the writer's handle; a reader reads what the writer folded. A disk engine's `Read`, `Write` and `Create` refuse a projection marked with another layout, or none, as `Unavailable` (`projection_layout`) before any of its tables is read or made; `Rebuild` opens an existing lake as `Write` does whatever layout made its projection, and nothing is read from it before `clear` makes it anew at the engine's layout (`docs/decisions/004-the-change-feed-is-the-log.md`). The memory engine opens `Rebuild` as `Write`. |
| `open_scratch(location)` | Open the lake to build its projection beside the one it answers from, in a scratch (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`): the handle reads the log as a `Read` handle does, refuses the lease and appends, and its projection, view, rankings and vector index are the scratch's. The projection the lake answers from is not opened, whatever layout made it: on PostgreSQL only the log's tables of the lake's schema are judged, and tables this build does not know are left to the layout that made them. A scratch marked built for this lake at this build's layout, still folded through its mark, whose mark the log holds, opens as it is, so the fold continues from the mark; any other is removed and made anew. `Unavailable` (`rebuilding`) while another handle has the scratch open. A scratch left by a builder that ended unmarked is removed by the next `Write`, `Create` or `Rebuild` open; one marked by a build of another layout, whose mark the log holds, is left for it. SQLite keeps the scratch in `projection.db.rebuild`, beside a locked `projection.db.rebuild.mark`, which is emptied and never removed; PostgreSQL in a schema `lakelands_rebuild_<hash of the lake's schema>`, reading the log from the lake's. The memory engine refuses as `Unbound`. |

### `ProjectionStore`: what the fold writes

The fold calls the projection inside one transaction per chunk: an
append for a write, a page of the log (1 024 events) for a rebuild or an
open's catch-up. A page is folded in more than one transaction when it
computes a type's derived fields again: once those records are put, the
fold commits through the last event folded and begins another
transaction for the page's remaining events, which then read the
records from the projection
(`docs/decisions/048-the-fold-keeps-the-event-not-the-records-it-changed.md`).
`commit(through)` records the position folded through
in the same transaction, so a crash leaves the projection at a whole
chunk, and opening a lake folds the log from `folded_through` on. A
chunk's boundary may fall inside an append; a view inside an append
already answers each event's states. What a fold that stops leaves, and
what reads answer meanwhile, is under `LakeStore::head` below.

- The store never nests transactions and never puts outside one.
- Nothing put in a transaction is visible to any view until it commits;
  a commit makes all of it visible at once, with its `through`.
- Before the commit, the store reads a state it put in the transaction
  from what it staged, never through a view: a record, a membership, a
  recorded output. A view it reads between its puts answers the output
  recorded before the transaction, and the store depends on that: a
  declaration that computes fields again reads the outputs recorded
  before its own event, the view's among them (048). An engine may hold the
  puts until the commit, or write them before it: PostgreSQL holds a
  page of 1 024 puts and writes each full page inside its open
  transaction, so it holds a page of a large append, never all of it.
  Whatever an engine answers about a put itself (the state it
  supersedes, where it ends, whether it is the head) is answered over
  what the transaction put and what was committed together, so the
  commit leaves what committing after each put would.
- Within a transaction, a type's declaration is put before any record
  of that type at or after the declaration's position. A record's
  states come in that record's own order; the states of different
  records need not come in position order, and a state below a
  declaration's position may be put after the declaration. An engine
  that writes a put before the commit (SQLite each one, PostgreSQL a
  page of them) indexes the record under the declaration in force by
  then, and depends on that order. A record written before the
  declaration it lies at or after is not indexed under it: its full-text
  entries and edges stay those the earlier declaration gave it, and it
  has no row in the order index, which the declaration empties and the
  commit fills again only from the records below the declaration's
  position, so a page ordered by an indexed field reads it as holding no
  value. It stays so, on SQLite and on PostgreSQL, until it is put again
  or its type is declared again. The fold puts in the order of the
  events, which is that order, but for the records a `declared` computes
  again: those it puts later, before the next event that reads a record
  and before the commit, a record at a time, each at the position of
  every such `declared` that changed it (048). Their type's declaration
  is put by then; what later events that read no record put may be too:
  their applied marks, declarations, memberships, proposals and outputs.
- A put may answer an error the engine met writing a page. The
  transaction is then rolled back as after any failed put: `rollback`
  leaves nothing of it, the pages written before the failing one
  included, and the next `begin` starts clean.
- `commit(through)` is `Invalid` when `through` is before
  `folded_through`, or when a put in the transaction is valid from a
  position after `through`.
- The projection need not be durable before the log is: it can always be
  folded again from the log. What must hold after a crash is that the
  states and `folded_through` agree.

Every put carries its own position — a record's `position`, the `at` of
everything else — and its state is valid from there until the next state
of the same thing, so a view at a position inside a multi-event append
sees each state from the event that made it. A put of the same thing at
the same position replaces the state there (the fold writing the same
state again); it never adds a second one.

| Method | Contract |
|---|---|
| `begin`, `commit(through)`, `rollback` | One transaction per append. |
| `folded_through` | The last position the projection reflects. |
| `put_declaration(declaration, version, schema_hash, at)` | A declaration in force from `at`, replacing, from there on, the earlier declaration with the same `declare` and name; the earlier one is kept. Put before any record of its type at or after `at` in the same transaction (above). |
| `put_record(record)` | A record's whole state as of `record.position`: values, per-field meta, flags. The state it supersedes is kept, but under a head-only name (below). A put ends only the latest earlier state of its key, and a read of one record takes the latest state at or before its position: neither reads the key's other states, so a record written often is put and read as one written once. The engine derives its own indexes, full-text entries and edges from it and the declarations in force at that position. One form of name is the store's own: `sys:kept.<view>` (`lakelands_api::seam::KEPT_PREFIX`), under which the fold keeps a view's records, each put again at every write to its group; no declared type bears it, since no type name holds a `:`. An engine keeps such a record as any other, with the rows every record has, its order rows those the view's `sort_index.<view>` setting asks for of the fields the view declares at the head (lakelands#668), and tells the name by that prefix alone, as it tells a setting by `sys.setting`: SQLite and PostgreSQL read its pages, its counts and its records from the states with no end at the head, and at an earlier position step from key to key and seek each key's state there, where a stored type's statements read every state of the keys they pass. That changes no answer, only what reading keys that hold thousands of states costs. A second form is head-only: `sys:order.<view>` (`lakelands_api::seam::ORDER_PREFIX`, `is_head_only`), under which the fold keeps the order of a kept view's extremes, an entry for each member. An engine keeps one state for each of its keys: a put deletes whatever states the key held and keeps the new one, an ended record only deletes, and nothing else is written for it: no lookup, full-text, order-index, edge or vector row. Such a name is read at the head only, by the key-ordered page and `get` of any type, so its page after a key reads the next key's one row; a view at an earlier position shows no state for a key put after it, and the store never reads it so. A rollback leaves the states the key held. |
| `end_head_only(name, at)` | Every key of a head-only name (`put_record`, above) ends at once, at `at`: whatever the keys held, put before the transaction or earlier in it, is deleted, and a key put after it in the same transaction is kept. A disk engine deletes them in one statement, whatever their number. The fold calls it where a kept view ends, so ending a view writes nothing for each entry of its order. A rollback leaves what the keys held. |
| `put_proposal(proposal, at)` | A proposal's state from `at`, the position of its latest event; earlier states kept. A view at a position inside a multi-event append sees each state from its own event, not from the commit. |
| `put_membership(…, at)` | Whether a record is in an object set from a position on; earlier membership kept. |
| `put_output(…, at)` | A seam's recorded output, by seam and input digest, valid from `at`, the position of the event that recorded it. A later output for the same seam and input replaces the earlier one from its position on (022, extended: a redeclaration retries a failure). Two seam names are the store's own. Under `sys:renames` the fold keeps the lake's list of field renames, as one output, put at the position of each `migrated` event that renames. Under `sys:sealed` it keeps, for each source (its input the SHA-256 of the source id's canonical bytes), the versions of its key the log sealed with, in the order first used, put at the first `record_ingested` sealed under each (`docs/decisions/058-a-source-may-be-forgotten.md`), and put again, the version's entry marked, at the first sealed under it by a delivery sealed apart. The keys apart an event a delivery sealed apart names are not kept: a recorded output reads them from the value's own event in the log, and only at a value whose version `sys:sealed` marks (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`). No seam records under `sys:renames` or `sys:sealed` and no event holds their outputs; an engine keeps them as any other (`docs/decisions/006-grants-not-roles.md`, extended). A read of an output takes the latest state of its seam and input at or before the view's position and reads none of their other states, so an output recorded often is read as one recorded once. |
| `mark_applied(applied)` | Remember an applied event's id at its position, so a redelivery is recognised (the first position marked for an id is kept), and keep the feed's index of it: its kind, what it is about (`lakelands_core::feed::about`), one entry a type it concerns, and the source it is of: a `record_ingested`'s is its envelope's, and any other event's the one its seal names, as a recorded output's does, else its envelope's. `clear` takes the index with the rest; `compact` keeps it. |
| `clear` | Delete everything, kept history and the vector index included, for a rebuild; `folded_through` is none afterwards. |
| `compact` | Give back the storage the projection no longer uses, outside a transaction (`Invalid` inside one), and answer its size in bytes before and after as the engine measures it, or none. No view answers otherwise afterwards: no state, kept history included, no applied id and no recorded output goes. A compact that fails leaves every answer as it was (`docs/decisions/040-maintenance-never-changes-an-answer.md`). |
| `choose_order(type, step)` | One step of keeping an order index the lake chose beside the log (`docs/decisions/041-a-sorted-page-reads-its-page.md`), outside a transaction and under the writer's lock; it moves no position and changes no answer. `Start(field)`: the fold keeps the field's rows for every put from now on (a record put during the build has its row), and no read takes the field yet. `Fill { field, after, limit }`: write the field's rows of up to `limit` head records after the key `after`, in key order, and answer the key the chunk ended at, none once the type is filled. `Keep(field)`: the field's rows are whole and a read may take the index for it, while its mark equals the head. `Drop(field)`: stop keeping it and delete its rows. The choice is a meta row of the projection (`chosen.<type>`) and never in the log, so a projection rebuilt from the log starts with none. A disk engine marks the position the lake's chosen fields were kept through (`chosen_through`), which its fold moves only from the position it folds from (PostgreSQL holds it only beside a twin row in `sort_index`, which a rebuild by such a writer drops): once a writer that keeps no chosen field folded past it or rebuilt the projection, no read takes a chosen field and no step is taken, and a writer's open drops the whole choice and its mark. A type whose `sort_index.<type>` setting names a field is the host's: a step on it does nothing, and the setting's own write takes the choice away. An engine that keeps no order index refuses as unbound. |

Rebuilding a projection from the log produces the same state, byte for
byte, as the projection it replaces
(`docs/decisions/001-the-log-is-the-truth.md`); the conformance suite
checks it on every engine.

### `ProjectionView`: what reads see

A view reflects one position, and every answer is computed from the
states valid there. Records come back whole and unmasked; grants and
masking are the store's. A view ranks nothing: the full-text and vector
indexes describe the head, and `ranked` reads them (below).

| Method | Contract |
|---|---|
| `position` | The position the view reflects. |
| `declarations` | Every declaration in force: for each `declare` and name, the latest put at or before the position, with the position it is in force `since`. No order is promised. |
| `chosen` | The fields of each type's order index the lake chose, at the head: those kept, which a read may take, and those being built, which none may. |
| `replaced(from, to)` | Every state of a declaration that a later one replaced after `from` and by `to` (at most the view's position), as it was put, with its `since`. With those in force at `to`, the latest of each name at or before any position from `from` on is what `declarations` answers there: a read that judges many entries at their own positions asks once a range instead of once a position (lakelands#1064). No order is promised. An engine that keeps no such read answers `None` (the default), and the store reads each position's declarations. |
| `get(type, key)` | One record's latest state at or before the position, retired and restricted included; none if it had none by then. |
| `get_many(type, keys)` | What `get` answers for each of the keys, in one read: each record once, in key order, a key with no record left out. The store's walks read the far ends of a page of links through it (lakelands#227). |
| `scan(type, scan)` | The records of `type` whose latest state at the position qualifies, as below. |
| `count(type, scan)` | How many records `scan(type, scan)` would answer with its order, `after` and `limit` ignored: the flags, `valid_at`, `equal` and `filter` qualify them as below. Its cost is in **The ceiling of a count**, below. |
| `edges(type, keys, relation, walk)` | The links of some records of one type, one list per key in the keys' order, each what the key alone answers; a link between two of the keys is in both lists. An engine answers them in one read, not one per key: the store's walks ask a page of records at a time (`lakelands_core::graph::PAGE`, 256), and PostgreSQL reads each page in a fixed number of statements (lakelands#227). For each record, its links over one relation or all: `Out` those it declares, `In` those that point at it, `Both` their union with each link once (a link from a record to itself included). Links to a retired or missing record are returned as they are; hiding that end is the store's (`docs/decisions/026-the-store-walks-the-graph.md`). No order is promised; the store's walks order their own answers. A relation with `via` is carried by that reference field: a record links to the target whose key is the field's text value, and `Edge.fields` is empty. A relation with neither `via` nor `derived` is carried by the live records of its link type `sys.link.<type>.<relation>` at the position (`docs/log-format.md` §2.3): `Out` gives the links whose `from` is the record, `In` those whose `to` is, and `Edge.fields` holds each link's own fields, `from` and `to` left out. A removed (retired) link is no edge. The rule is `lakelands_core::links::link_edges`: at the head an engine may find a link type's candidates through its own index (SQLite and PostgreSQL look `from` and `to` up in their exact-text index), and at an earlier position it reads the link type's records there. |
| `proposals(filter)` | Every proposal whose latest state at the position matches `filter.state`, `filter.kind` (`upsert`, `retire`, `call`, `declare` or `migrate`) and `filter.type` (a `call`'s type is its verb's `on` type in force at the position; a `declare` touches every type it declares or a declaration of it acts on, a `migrate` its type), newest first — by the position of that latest state, descending, then by id — and at most `filter.limit`. `order_by`, `cursor` and the read position in the filter are the store's and the engine ignores them: the store applies the caller's `order_by` to the engine's answer, and refuses as `Invalid` an `order_by` naming a member a proposal does not have. |
| `proposals_before(filter, before)` | The proposals `proposals` answers, newest first, a page at a time (lakelands#1063): those whose latest state at the position matches `filter.state`, proposed before `before` (every one where none), at most `filter.limit`. A proposal's position is its own event's, so no two share one, and the last of a page continues the next. `filter.kind`, `filter.type`, `order_by` and `cursor` are not asked here: the store judges the kind and the type on the entry as its reader is shown it. SQLite and PostgreSQL read it on an index of the state's `position`, which the schema makes at a writer's open, so no projection is rebuilt for it. An engine that keeps no such read answers `None` (the default), and the store reads `proposals` whole. |
| `is_member(set, type, key)` | Whether the record is in the object set at the position; false if never put. |
| `members(set, type)` | The keys of the type's records that are in the object set at the position; empty for a set never put. One read: the store asks it once for a set before it walks a type (a set declared again, a migration, a re-key), where `is_member` for each record would be a statement each on PostgreSQL (lakelands#477). |
| `applied(id)` | The position `id` was applied at, if that is at or before the view's position. |
| `applied_many(ids)` | `applied` for many ids: one answer an id, in their order, an id asked twice answered twice. Provided, its default asks `applied` once an id; an engine answers them in one read, PostgreSQL in one statement, since an ingest window asks every id it judges in one call (lakelands#1036). |
| `matching(filter, from, through, limit)` | The positions in `[from, through]`, none past the view's position, whose applied events `filter` admits by the index `mark_applied` kept (`lakelands_core::feed::admits`), in order, each once, at most `limit`, and the last position decided: the last answered when `limit` is reached, else `through` or the view's position, or earlier where the engine bounds its work, and never past what the projection had folded where the index is read, in the same snapshot: a view taken before a rebuild's `clear` decides only what the refold has reached, and nothing (`through` none) where it has not reached `from`. An engine seeks a filter by its keys, else its types, else its kinds (`lakelands_core::feed::seek`), one ordered range a value, and checks the rest on each entry; one that checks a kind on every entry of a type decides at most `lakelands_core::feed::WINDOW` (65 536) positions at a time. |
| `of_source(source, from, through, limit)` | The positions in `[from, through]` whose applied events are of `source`, by the same index, decided as `matching` decides them: in order, each once, at most `limit`, never past the view's position nor past what the projection had folded where the index is read. Forgetting a source reads its events by it, and walks the log from where it decides nothing (`docs/decisions/058-a-source-may-be-forgotten.md`). |
| `output(seam, input)` | The recorded output valid at the position, if any, with the position it was recorded at: the latest recorded for that seam and input at or before it. The fold checks a fill that names an output by `output_at` against that position (`docs/decisions/053-a-recorded-output-is-recorded-once.md`). |

**`scan`.** A record qualifies when all of these hold:

- it is live, or `scan.retired` is set; it is unrestricted, or
  `scan.restricted` is set;
- for a valid-timed type and a `scan.valid_at`, it is valid at that
  instant as `lakelands_core::valid_time::record_holds` decides;
- it holds each of `scan.equal`'s values exactly: the field's stored
  JSON value is identical to the one given, and never null
  (`lakelands_core::engines::holds_exactly`). This is identity, not the
  evaluator's `=`: a derived relation's walk asks for its matches this
  way (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
  At the head an engine may look a string value up in its exact-text
  index (`lakelands_core::engines::text_lookup`): the index names
  candidates, and this rule still decides each. The index keeps rows
  only for the fields `lakelands_core::engines::looked_up` names, decided
  from the declarations and the `lookup_index.<type>` setting at the
  head, never from which rows exist; a string of any other field is
  read by narrowing the type's states in storage, not by a lookup;
- `scan.filter` holds for it, exactly as the one evaluator
  (`lakelands_core::expr::holds`) decides over the record's own fields,
  with no settings, no clock and no provenance. A filter the evaluator cannot evaluate
  for a record (an error) does not match that record. An engine that
  translates a filter into its own query language must select exactly
  the same records; where it cannot, it evaluates that part with the
  evaluator. The store hands an engine only filters that need neither
  the clock, the lake's settings, per-field provenance, a pattern
  match nor a set's kept membership (`now`, `today`, `setting`,
  `known_at`, `seen_at`, `matches`, `in_set`); it evaluates any other
  filter itself. When the filter is, or has as
  one of its `and` terms, `field = 'text'` with a text that reads neither
  as a number nor as an instant
  (`lakelands_core::engines::exact_text_terms`), only records holding
  exactly that string can qualify, and at the head an engine may look
  them up in its exact-text index as it does for `equal`, by a field
  the index keeps; it still evaluates the whole filter on each.

The qualifying records come ordered by `scan.order_by` — each key the
field's value, or its `known_at` or `seen_at`, or, with no field,
`latest`: the position of the event that made the record's state at the
view (a link record moves itself, not the records it joins; a
reference field moves its record); a record without a value
for a key after every record with one, in either direction; the record
key ascending to break ties; values ranked and compared by
`lakelands_core::expr::order::cmp_values`, the one sort order
(`docs/decisions/029-every-answer-has-one-order.md`) — then only those
strictly after `scan.after` in that order, then at most `scan.limit`
(literal: 0 answers nothing). `find` pages through this contract: the
store asks for the page it answers, in its order, after the cursor's
row — or after the row a caller's `after` names, the same position said
in the open, which reaches the engine as the same `scan.after` — and
one row more than it answers. An engine's paging is an
optimisation that may never change an answer
(`docs/decisions/029-every-answer-has-one-order.md`).

**The ceiling of a count.** `count` holds one batch of the type at a
time, never the type: every engine reads the type's states in key order,
1 024 at a time, and counts those that qualify by the one rule
(`lakelands_core::engines::count_in_key_order`), so its memory is a batch
and its time is linear in the type. At the head, a count whose scan
must hold a text exactly (a string in `equal`, or the filter's exact-text
term) reads on SQLite and PostgreSQL only the records holding that
text, from the exact-text index in key order, a batch at a time: linear
in those records, not in the type. The memory engine keeps no such index
and reads the type, and so does every engine at an earlier position and
under a filter only the store evaluates. At the head, a count that qualifies
records by the flags alone (no `filter`, no `equal`, no `valid_at`) is
SQLite's and PostgreSQL's `COUNT(*)` over the states in force, which
answers without reading a state. The store counts through it: `find`
with only `count()` measures and no `group_by` asks `count` with the
filter the engine can apply, and a filter only the store can evaluate
is applied over engine pages read one after another and dropped, as for
`find`. A grouped aggregate and a view read their records the same way:
each record is folded into its group's count, sum and best value and
dropped with its batch, so `find`'s aggregate over a stored type, and a
view counted or read, hold a batch and the groups, never the records
(lakelands#388). A view that reads another view computes that one first.

**The ceiling of `find`.** `find` asks the engine for its page,
never the type: its order, the row it continues after, and `limit + 1`
rows. A filter only the store can evaluate is applied over engine pages
read one after another, so its cost is the rows read to fill the page:
bounded in memory, linear in time. An `offset` reads the rows it skips
the same way, a page at a time, and drops them, so a jump holds one
page and costs the rows it skips; `total` is the count above, at the
same position
(`docs/decisions/045-a-table-reads-a-page-and-its-total.md`). A page ordered by key reads about
`limit` rows, not the type; every engine reads it in key order from
`after` and stops when it is full (`lakelands_core::engines::
scan_in_key_order`). **A filtered page by key costs the rows it passes
over to fill itself,** read in batches from `after`: with a filter the
engine applies, the type's rows from `after` until the page is full.
When the scan must hold a text exactly (a string in `equal`, or the
filter's exact-text term), at the head SQLite and PostgreSQL pass over
only the records holding that text, read from the exact-text index in
key order from `after`, so a walk of such pages reads those records a
batch at a time as it goes and not all of them for each page
(lakelands#574); a scan with
`equal` and no limit, a derived relation's matches, reads them the same
way, a batch at a time. A reader whose grant admits only some records of
the type (051) has its pairs added to `equal` on every scan and count
the store makes, so its reads go by the same index. The memory engine keeps no such index and passes
over the type's rows. **Its ceilings:** a page in the order of a field
the lake does not index whose scan must hold a text still reads every
record holding it and sorts them, for each page (in an indexed field's
order it reads its page, below); a filter
only the store evaluates is not handed down, so no term of it reaches
the index and the page passes over the type's rows; and an earlier
position has no index of exact text, so there too the page passes over
the type's rows from `after`. **A page ordered by a field reads its page when the
lake asks for it:** the setting `sort_index.<type>` (below, §`SystemVocabulary`)
names the fields of a type to keep an order index for, and a page at the
head ordered first by one of them reads about `limit` rows too. Every
engine keeps the index of the head, each named field a record holds as
the first 32 bytes of its `sort_key` (`lakelands_core::engines::
sort_key`, bytes whose order is the one order across kinds), and reads
the page in the index's order from `after`; values that share the first
32 bytes are read as a run and ordered from the values
(`scan_in_field_order`; `docs/decisions/041-a-sorted-page-reads-its-page.md`).
A page of 50 ordered by one indexed field over 100 000 records takes
about 0.1 ms in memory, 0.3 ms on SQLite and 2 ms on PostgreSQL. **Under
an exact text,** at the head, SQLite and PostgreSQL hand such a page only
the records holding the text (lakelands#587): they count the holders up
to 4 096 (`FEW_HOLDERS`); more, and the order index is read in its
order, a row kept where the exact-text index holds its key; at most
that, and the text's rows in the exact-text index are ordered by their
order rows and cut at the page, index rows only. The records without the
field are read from the text's rows in key order. The memory engine
keeps no exact-text index and admits the order index's rows as it reads
them (`Holders`; 041, extended).
`describe_type` names a type's indexed fields in its lifecycle
(`sorted`). The index is used only while it describes the head; a lake
whose index a writer did not keep sorts as below until a writer's open
rebuilds it. **At an earlier position** — every page of a walk after a
write, which its cursor keeps at the first page's position — a page reads
the head's index too, passing over the keys of the type changed since the
position and merging in their states there, read in the same snapshot
(`scan_in_field_order_at`): SQLite finds those keys by two range reads,
into `records_since (ty, since)` and `records_at`; PostgreSQL by the one
into `records_since`, since a state there ends only where its key's next
is put (lakelands#537); the memory engine in a per-type log of the states
put. More than `min(10 ×
limit, 4 096)` states put since, or keys changed, and the page sorts the
type as below. At 100 000 records a page of 50 with 500 keys changed
takes 2 ms on SQLite and 8 ms on PostgreSQL, against 0.18 s and 0.24 s for
the sort; `records_since` costs a write about 1 to 2 %
(`docs/decisions/041-a-sorted-page-reads-its-page.md`, extended).
**Its ceilings:** with several sort fields, the first
field's index orders runs of equal values and each run is read whole and
sorted, so a first field with few values reads most of the type (a
boolean first: about 0.3 s in memory, 0.45 s on SQLite and 1.3 s on
PostgreSQL at 100 000 records). A field the lake did not ask an index
for, an earlier position past the ceiling above, an order by `known_at`,
`seen_at` or `latest`, and a view type read the type at the position and
sort it by `sort_key`,
holding it only for that answer: about 0.2 s in memory, 0.5 s on SQLite
and 0.6 s on PostgreSQL for a page of 50 over 100 000 records. Because
each such engine page sorts the type again, a filter only the store
evaluates asks for twice as many rows each time it reads on: a page
costs a few sorts of the type, never one per `limit` rows. `latest` asks
for no grant beyond the type's: that a record changed, and when, a
reader already learns from its `version`, which moves on every change,
including one to a field it cannot see. It is refused on a view type,
whose records are computed and have no event of their own, on a
pattern query's rows and on proposals. **What an
index costs:** a lake that asks for none writes as before; each indexed
field costs every write of the type a row in two ordered indexes, and
the setting's write one pass over the type. On PostgreSQL, the ingest
line of the streaming check at 25 000 records (three interleaved runs,
`synchronous_commit` off, because durable commits on the test machine
spread too widely to compare) read 369 events/s before the index, 349
with no setting (within the spread of the runs) and 327 with the index
on one text field. A view type's page
is computed from its source's records, whole, as before.

**The ceiling of a walk.** A pattern query asks the engine for the edges
of the records it reaches (`edges`), a hop of its frontier and a page
of records at a time, so what a walk holds and how long it takes follow
what it reaches, not the lake, and on an engine whose every read is a
round trip, a hop costs a few round trips per page, not per record. At the head every engine
finds a record's edges from the record itself: the edges out of it from
its own reference fields; the edges into it over a relation carried by a
reference field from an index of the head's edges (SQLite's and
PostgreSQL's `edges` table, the memory engine's `in_edges`); and the
link records whose end it is from an index of their ends (SQLite's and
PostgreSQL's exact-text index, the memory engine's `link_ends`). The
streaming check's `traverse` line walks link records three hops
backwards from one start, 48 rows: it holds 0.1 MiB of heap and takes
0.00 to 0.01 s on the memory engine, 0.01 s on SQLite and 0.10 to 0.11 s
on PostgreSQL, at 25 000 records over 40 621 links and at 100 000 over
162 496 alike (its long run, a machine of 12 cores at a load of 14 to
33; lakelands#190). **Its ceiling:** at an earlier position an engine
reads the declaring types and link types there whole, for every page of
records the walk reaches, so a walk into the past costs the relation's
records per page of each hop. The heap is held by the suite; the time is reported.

**A graph's pages.** A pattern query projected as `graph` and cut by
its `limit` is continued by its `next_cursor`, as rows are
(`docs/decisions/018-one-pattern-query.md`). Every page walks again, at
the cursor's position: a neighbourhood (every step from depth 0) as deep
as the page's last record, a chain whole, as a page of rows walks its
pattern whole. A hub of 10 000 records at a limit of 1 000, release
build, on a machine of 16 cores at a load of 5 to 6: the eleven pages
of its graph took 1.1 to 1.2 s together on SQLite (about 0.1 s each),
and 0.7 to 0.8 s on the memory engine, against 3.1 s and 0.8 s for the
ten pages of its rows (lakelands#556). **Its ceiling:** a page costs
the walk to its last depth, so the pages of a wide graph that lie at
increasing depths each cost the walks before them, as each page of
rows costs its whole walk.

**The whole-graph measures.** A pattern query projected as `degree` or
`components` asks nothing new of an engine: the store asks `edges` of
the records it measures a page at a time, as a walk does
(`docs/decisions/026-the-store-walks-the-graph.md`). A degree reads the
start's records a page at a time, reads their links and their far ends
a page at a time, and keeps the best rows asked for, so the streaming
check's `degree` line, the ten records of highest degree over the whole
type, holds the same heap at every size of the lake: a page's links,
5.4 MiB on PostgreSQL and 4.9 MiB on SQLite. At 100 000 records it took
20 to 21 s on PostgreSQL and 16 s on SQLite, against 356 to 536 s and
31 s while each record was read on its own (the long run, interleaved,
on a machine of 12 cores at a load of 17 to 28; lakelands#227).
Components hold every record they reach, so their `components` line
grows with the graph and is reported; the lake's `max_graph_nodes`, a
limit of its `Location` (100 000 unless it says otherwise), bounds them,
and past it they are refused `too_large` naming the ceiling. The
`components-capped` line holds that bound: with the ceiling at half the
lake's first size, the walk is refused at the ceiling at every size.
Walk 3 (`docs/walks/walk-3.md`) asks both measures of a small network,
and asks components again under a ceiling of five, which refuses them.

A view does not aggregate. Aggregates — over `find`, over a pattern's
rows and in a view type's measures — are the store's: it computes them
over the records it reads, decimal-exact and grouped in one order, so
every engine answers them identically
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
A count alone is asked of the engine's `count`, where the engine can
apply the filter. A sum reads at the
largest scale among its values, a zero's included, whatever order they
are added in (`docs/decisions/011-storage-kind-and-value-type.md`,
extended): `5` and `0.00` are `5.00`. An engine may keep
indexes that make the scans behind them faster; it never computes a
group.

An aggregate's `group_by` keys are field names (`"partner"`) or named
expressions (`{"name": "month", "expr": "date_bucket(due_on, 'month')"}`),
evaluated per record by the one evaluator and answered under their name
in each group's `by`. A named key's name follows the rule for field names
and differs from every other key's, and the principal must be allowed to
read every field its expression reads (024). A named key is an object; a
list is refused as a number is. In a pattern, a row holds each value
under its path, `<name>.<field>` or `<name>.key`, of a record the
pattern binds under its type's name or the one `as` gives: a field key,
a measure's field and a named key's paths name it so
(`bill.amount > setting('threshold')`, `customer.tier` over a step), and
a path no bound name starts is refused
(`docs/decisions/018-one-pattern-query.md`, extended). A value the expression
cannot compute for a record is null, so that record falls in the null
group, which sorts last. `date_bucket(x, unit)` answers the instant in
UTC at which the bucket holding `x` starts, for `day`, `week` (ISO, from
Monday), `month`, `quarter` and `year`. A declaration may call it too,
so a derived field can hold the bucket
(`docs/decisions/017-one-expression-grammar.md`, extended).
`date_bucket(x, 30d, origin)` answers the start of the window of that
width holding `x`, the windows counted from `origin` in both directions:
a width is a duration, written unquoted, and always takes its origin (a
literal instant, a field, `today()`); a declaration may call it too. **The
ceiling:** buckets are UTC only, and there is no number bucket.

### Reading an object set

A set is its filter (`ObjectSetDecl.where`), and a host reads it through
that filter (lakelands#430). A condition may name a set, `in_set('open')`,
and still reads the set's filter, never stored membership: the filter
stands in the name's place when the condition is read, in a read's
`where`, in another set's `where` and in a trigger's `when`
(lakelands#552):
- **Which records, how many, a sum, a group:** `describe` lists the set
  with its filter. `find` under that filter answers the members, with
  `total` their count and `aggregate` their sums and groups. A pattern's
  start and a search take the same filter as their `where`.
- **At an earlier position:** `describe` at the same `as_of` gives the
  filter in force there.
- **At a valid time:** `find` with `valid_at` evaluates the filter over
  the records valid then.
- **Combined, sets of one type:** `in_set('a') and not in_set('b')`, or
  with `or`: union, intersection and difference are `or`, `and` and
  `and not`. A set declared from two others names them, so it follows
  them when one is declared again, in that declaration's append.
- **At an earlier position** a named set is the filter in force there.

`describe` lists a set only to a principal who reads every field its
filter names, or who holds `schema`. That is the same rule by which
`find` probes those fields (024), so a principal who may not see a set
cannot read it through its filter either, nor by its name: to a reader
kept from a field, a set whose filter reads it, itself or through the
sets it names, is a set that is not declared. A host that names the set
holds no copy of its filter that drifts when the set is declared again.

A path of more than one name steps into a `json` or `geo` value's
members. Into anything else it reads nothing, and is refused: a
condition reads the record's own fields, so `partner = 'p1'` compares a
reference's key, and a pattern walks to the record it names.

The `object_set_changed` events describe the same members only where the
filter is evaluated in the same way they were:
- for records written after the set was declared, and for those already
  there when it was declared or declared again, which its declaration
  decides ("Declaring a set over a large type", below);
- for a filter that does not read the clock: `now()` and `today()` are
  the write's time in an event and the read's time in `find`, so a set
  whose members move by date is moved by an aging rule, whose moves are
  writes;
- at system time: an event knows no valid time.

The suite holds this (`SET_READ_BY_ITS_FILTER`): for records written
after the set's declaration, `find` under the filter from `describe`
answers what the events record, and `(f1) and not (f2)` the difference.

### `ranked`: the head's rankings

Search is ranked retrieval: a read's position decides what may be seen,
not how the current text or vectors rank
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
The full-text and vector indexes describe the head only, so they are not
read through a view. `LakeEngine::ranked(type, asked, k)` reads them:

- `asked` is every ranking of one pull of a search: a text, with the
  searchable fields it is matched in if it names any and the weight of
  a match in each field the store weighs, and each vector field with
  its query vector. The answer holds one list for each, the
  text's first, each cut at `k`.
- **Whom the lists are for.** `asked.seen` is the reader's selection
  (`Scan`: retired, `restricted`, its `only` pairs, `valid_at`, a
  `where`), and `asked.exact` asks every vector list ranked exactly. An
  engine that reads `seen` admits each vector candidate by it, on its
  head record, through `engines::admits`, ranks the records holding
  `seen.equal` exactly, and answers `Ranked.judged = true`: its vector
  lists then hold only what the reader sees, and the store reads only
  the page's records. One that does not answers `judged = false`, and
  the store judges every candidate. Without `seen` the lists are of
  every live candidate, ranked as for a reader holding `restricted`
  (`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`).
- **One state.** The engine reads the lists and the position the
  projection is folded through in one transaction (one lock on the
  memory engine): the lists are of the head at exactly that position,
  whatever is folded meanwhile, and an open transaction of the fold is in
  none of them. A rebuild's `clear` is no fold: on PostgreSQL it drops
  the tables and makes them again, which a transaction's snapshot does
  not hold back, so `ranked` takes every table it reads in
  `ACCESS SHARE` mode before its snapshot, in the order `clear` drops
  them. The `clear` waits for the call, or the call for the `clear`, and
  the call then reads the cleared projection: no position, and empty
  lists. An engine never narrows the candidates to an earlier position:
  it answers the records live at the head.
- **The store judges at that position.** It takes its head at exactly
  the position answered (a view of it serves that) and reads each
  candidate's key, hold and grants there. A search that took its head
  before it asked, and finds the rankings were read at a later position,
  is made again with the head at that position and judges the candidates
  it already has; it asks again only if the plan differs there (the
  type or a ranked field was renamed in between). A search at the head
  is then a read at that position, and its answer and cursor name it.
  No search is refused or repeated for the writes beside it.

**A text's list.** The engine answers from its full-text index as it
stands at the head: the records live there, restricted ones included
(the store applies the principal's grants to the hits, as for `find` and
`get`); an engine is not required to keep a full-text index of earlier
states. A record is a hit when one of its searchable fields' text at the
head holds at least one token of the text. Tokens are what the store's one pipeline makes of the text
(`lakelands_core::stem::stem_text`, stemmed by the type's `language`, or only
lower-cased and folded when it has none or the pipeline has no stemmer
for it), applied the same way to the query and to the
fields (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
The set of hits, their order and their scores are the same on every
engine: one function ranks (`lakelands_core::engines::lexical`). Today a
hit scores each distinct query token it holds at the weight of the
heaviest field holding it, summed in the tokens' order: `asked`'s
`text_weights` give a field's weight under its name at the head, and a
field they do not name weighs 1, so with none the score is the number
of distinct query tokens the hit holds. Hits come best first, equal
scores ordered by key ascending; at most `k`, cut by that score. A later
release may score otherwise, on every engine at once, so a caller
compares scores within one answer.

From 0.20.0 (lakelands#1412), in force with the issue that builds it,
the function is BM25F in its combined-field form, which replaces that
score for every lake, with no switch per type: a token's frequency in
each field, multiplied by the field's weight and scaled by the field's
length against its average, summed over the fields and saturated once,
times the token's rarity, in `f64` with `ln` from `libm`, the same bits
on every engine (025 holds the formula and its constants). What it
reads:

- **`df`** is counted by the function over the candidates the engine
  hands it, admitted to the reader's class, since every candidate that
  holds a query token is handed. The engine keeps nothing for it.
- **`n_f` and `L_f`**, the records of the class whose field `f` holds a
  token and the tokens of `f` over them, are kept by the engine per
  type, field and hold, one count for the heads that are not held and
  one for the held: `asked.seen` without `restricted` reads the first,
  with it the sum of both. SQLite and PostgreSQL keep the counts and
  each record's tokens per field in their projection, written in the
  fold's transaction beside the text rows and taken out where a record
  is dropped, with a mark of the position they are folded through; a
  writer that finds the mark absent or behind counts them again at
  open, and a read that finds it behind counts at the search. The
  memory engine counts them in the same pass and keeps none.
- **A reader with no class.** Where `asked.seen` admits by `only`
  pairs, by a link's ends or by what its reader is told of a record
  (078), the store counts `n_f` and `L_f` over the heads it admits and
  hands them in `asked.text_stats`; an engine handed them ranks by them
  and not by its own.

They are taken at the head, as the lists are: a `where`, a `valid_at`
and the position read do not shape them.

Where the text matched (`Hit.matches`) is no engine's to answer: the
store computes it over the page's records after the cut, by the same
pipeline (025).
With fields named, a record's text is only that of the named fields that are
searchable in the type's declaration at the head: a record matching only
in another field is no hit, and a record matching in a named field is a
hit however many others match elsewhere, so an engine never cuts its
candidates at `k` before it narrows them. The store has already refused
a name that is not searchable or not readable
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).

**A vector's list** holds at most `k` records, scored by the **cosine**
of the angle between the query and the stored vector (higher is closer;
a vector of zeros scores 0 against everything; PostgreSQL alone leaves
it out of a field the lake asks an index for, while it keeps that index:
not on a server without pgvector 0.8 or later, nor while the field's
mark is behind the head, 025), best first, equal scores by key ascending. A field holding no vectors answers nothing; a query
whose length differs from a stored vector's is `Invalid`.

### `VectorIndex`

`LakeStore::vector_coverage(type)` answers, for each embedding field of
the type at the head, `embedded` and `due`. `embedded` counts the live
records holding a vector, restricted ones included, read from `count`.
`due` counts the live records holding no vector although their
`embed.from` fields hold text: never embedded, failed, or waiting for a
redeclaration's retry (022). A record with no text to embed is counted
by neither. It is the host's, with no door yet; `lakelands-client`
answers it `unbound`. Counting `due` walks the type's records without a
vector, a page at a time, so its time is linear in them (suite:
`vector_coverage_counts_embedded_and_due_records_at_the_head`).

`put`, `remove` and `count`, over the vectors of one field of one type:

- `put` replaces a record's vector for the field; `remove` forgets it,
  and forgetting one that is absent does nothing. Inside the fold's
  transaction both take effect when it commits, as its puts do. Both
  answer alike over a vector a state of the record holds, which a disk
  engine indexes as it puts the state (072): a put of other elements
  replaces the state's and a remove forgets them until the record's
  next state, and a put of a field its type does not keep apart is
  counted (suite: `put_and_remove_answer_alike_over_a_vector_a_state_holds`).
- `count` is how many vectors the field holds.

The store keeps the index in step while it folds: it puts a live
record's vectors and removes a retired record's, or a cleared field's.
Vectors are indexed at the head only, and read by `ranked` (above).

**The ceiling.** Every engine searches by brute force with the one
cosine (`lakelands_api::query::cosine`) and keeps the best `k` as it
goes (`lakelands_core::engines::TopK`), whose answer is what ranking
every vector and cutting at `k` answers, to the bit. So what a search
holds is `k` entries, whatever the field holds; SQLite steps its
statement, and PostgreSQL streams the field from the server a row at a
time without buffering it. What a search costs is a scan: a vector's
list reads every vector of the field, so its time grows linearly with the
vectors a field holds, and with their length. At 1,536 dimensions that
is about 6 KiB read and one dot product per vector per query; a field
of a hundred thousand vectors is a scan of some 600 MiB. The streaming
check holds its `search` line (one hybrid search, k = 10, over every
record's vector) to the heap bound on every engine (lakelands#197). Its
long run, over 22 500 and 90 000 vectors of two dimensions, held
0.1 MiB of heap at both sizes on the memory engine, SQLite and
PostgreSQL, and took 0.18 → 0.68 s, 0.02 → 0.13 s and 0.09 → 0.26 s, on
a machine of 12 cores at a load of 20 to 32; ranking the whole field
had held 1.6 → 6.4 MiB on the memory engine.
An index that answers faster than a scan picks candidates, and the one
cosine ranks them, so an answer never depends on its arithmetic.

**An index asked for** (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
The `sys.setting` `vector_index.<type>` names vector fields of a type. A
search over one of them answers the index's set, ranked by the one
cosine, ties by key, and says `ranked_by: index` (or `hybrid (index)`);
`describe_type` names them as `indexed`. On PostgreSQL the index is a
graph per class of reader (078): B, of the live heads not held, and
from the type's first hold A, of every live head, each a table of unit
vectors under pgvector's HNSW (`m = 16`, `ef_construction = 64`), read
as one stream at `ef_search` 1 000 in strict order while it is built
and not stale; SQLite and memory keep none and answer the exact
ranking. An engine that keeps an index promises that an indexed list
is a prefix of the field's one sequence for the class `seen` names (the
stream ranked by the one cosine, ties by key, then what the stream did
not hand out, ranked alike), and that the graph it streams from holds
nothing that class may not see; one that cannot keep that ranks the
list exactly. Every other field answers the exact
ranking on every engine. The suite holds the index's recall against the
reference at each engine's floor (suite:
`an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor`):
0.95 at k = 1 and 10 and 0.98 at k = 100 on PostgreSQL, 1 on SQLite and
memory. On its corpus (4 000 vectors of 32 dimensions around 40 centres,
50 queries near a centre or between two) PostgreSQL measured 0.98 at
k = 1, 0.97 to 0.98 at k = 10 (an HNSW build draws its own levels, so
two builds differ a little) and 1.00 at k = 100, at the head and at an earlier position,
and at least 0.996 with a filter. With `ef_search = max(k, 64)` it
measured 0.90 at k = 10 (0.86 at an earlier position): the queries
between two centres are where a narrow search stays in one cluster. Recall
is measured on this corpus; a lake's own vectors may be easier or harder. The
streaming check's long run holds its `search-indexed` line (the same
hybrid search with the index asked for on `vec`): on PostgreSQL with
pgvector it took 0.03 s at 22 500 and at 90 000 vectors, against 0.13
and 0.26 s for the scan, holding 0.1 MiB at both; on SQLite and memory,
which scan, it measured as the scan (12 cores, load 24 to 38).

### Where each promise is checked

An engine check runs through the seam
(`lakelands_core::conformance::engine`); a suite check runs through the
doors, so it checks the engine only as far as the store's use of it
reaches. "None yet" is a promise the engine must keep that no check
holds it to.

| Promise | Checked by |
|---|---|
| bytes, members and `tip` come back as appended; `read` honours `from` and `limit` | engine: `appended_bytes_come_back_unchanged` |
| a stale lease cannot append, and an event carrying another fence is refused | engine: `a_stale_lease_cannot_append` |
| an import keeps each event's fence, refuses a falling one, and the next lease is greater than every fence imported | engine: `an_import_keeps_each_events_fence_and_the_next_lease_is_greater` |
| an import goes only into a log of imported events | engine: `an_import_is_refused_once_the_log_holds_an_appended_event` |
| a held lease is refused at once, naming its holder | engine: `a_held_lease_is_refused_at_once_naming_its_holder` |
| every take of the lease has a greater fence | engine: `every_take_of_the_lease_has_a_greater_fence` |
| an append with a gap or a wrong `prev` is refused whole | engine: `an_append_with_a_gap_or_a_wrong_prev_is_refused_whole` |
| a sealed segment's digest is over its events as restored, and it is what the seal hands `first`; sealing an empty segment answers none and adds nothing | engine: `a_sealed_segment_is_digested_as_its_lines` |
| a framed append comes back unchanged, each event with its frame, read from the frame's first position, from within it and one event at a time | engine: `a_framed_append_comes_back_unchanged_read_from_within_it` |
| an append that holds a frame in part is refused as `append_frame`, whole | engine: `an_append_that_holds_a_frame_in_part_is_refused_whole` |
| a segment of format-19 lines and framed ones reads back as appended and is digested over its events restored | engine: `a_segment_of_older_lines_and_framed_ones_is_digested_over_its_events_restored` |
| a line kept as handed that does not restore comes back unchanged, and sealing its segment is refused as `damaged` at its position | engine: `a_framed_line_that_holds_what_its_frame_holds_is_refused_where_it_is_restored` |
| a dropped writer releases its lease | engine: `a_dropped_writer_releases_its_lease` |
| a read-only handle refuses the lease, appends and sealing, and reads | engine: `a_read_only_handle_refuses_the_lease_and_appends` |
| a read-only handle tells whether the lease is free, and takes nothing | engine: `a_read_only_handle_tells_whether_the_lease_is_free` |
| a handle opened for writing that never took the lease is refused an append, an import and a seal under a copy of the writer's, and writes nothing | engine: `a_handle_that_never_took_the_lease_cannot_append_import_or_seal` |
| a read-only open creates nothing and writes no projection | engine: `a_read_only_open_creates_and_changes_nothing` |
| a view at a position reads the states valid there | engine: `a_view_at_a_position_sees_the_states_then`; suite: `a_read_as_of_a_write_sees_that_write_and_nothing_later`, `a_read_as_of_the_position_before_a_write_does_not_see_it`, `retiring_a_record_does_not_erase_its_history` |
| a view past the fold is `OutOfRange` naming the head | engine: `a_view_past_the_folded_position_is_refused`; suite: `a_read_as_of_a_position_past_the_head_is_refused_as_out_of_range` |
| a rolled-back transaction leaves nothing | engine: `a_rolled_back_transaction_leaves_nothing` |
| edges `Out`, `In`, `Both`, a self-link once, links to a retired record kept | engine: `edges_walk_in_out_and_both`, `edges_to_a_retired_record_are_returned` |
| edges of many keys: one list per key, each what the key alone answers, at the head and earlier; `get_many` what `get` answers key by key, once each, in key order | engine: `edges_over_many_keys_answer_each_key_as_it_answers_alone`, `get_many_answers_what_get_answers_key_by_key` |
| edges from link records: `Out` and `In` by `from` and `to`, the link's fields in `Edge.fields`, a removed link gone, an earlier position's links | engine: `edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position`; doors: the nineteen `links` checks |
| a rebuild answers every read as before | suite: `a_rebuilt_projection_answers_every_read_as_the_live_one_did` |
| a rebuild leaves the projection answering until a checked swap; a second builder, a swap while one builds, of an unmarked scratch or of one behind the log are refused and change nothing; a scratch marked built is kept while the log holds its mark, and built beside a projection of another layout | engine: `a_rebuild_leaves_the_projection_answering_until_the_swap`, `a_scratch_marked_built_is_kept_while_the_log_holds_its_mark`; a builder killed halfway, and a scratch built beside a projection this build cannot read: SQLite and PostgreSQL `tests/rebuild_beside.rs`; a swap refused while another handle holds the file, a handle that opens during one, and a mark past the log's end: SQLite `tests/rebuild_beside.rs` |
| ingest, the feed, an export and its verification, a rebuild (lakelands#130) and a reopen (lakelands#133) hold as much memory at four times a lake's size as at its first; so does each page of a walk of a type through `find` (lakelands#134) | suite: `a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it` |
| a compact answers every read as before, appends nothing, and writes wait for it; the space it gives back | suite: the four `compact` checks; engine: SQLite `tests/compact.rs`, PostgreSQL `a_compact_vacuums_the_projection_beside_an_open_reader_and_reports_its_size` |
| `scan` filters, the retired and restricted flags, `valid_at` | suite, through `find` and `get` (paging, restriction, views, decimals) |
| `mark_applied` / `applied` recognise a redelivery | suite: `the_same_content_ingested_twice_appends_nothing_the_second_time`, `a_retry_with_the_same_idempotency_key_answers_the_first_outcome_and_appends_nothing` |
| `append` is durable before it returns: its events are there for the next handle | engine: `an_append_that_returned_is_read_by_the_next_handle`; surviving a crash of the process or the machine is the engine's own test, not the suite's (SQLite: `tests/lake_directory.rs`) |
| the stale-lease refusal is `Unavailable`; another fence is `Invalid` | engine: `a_stale_lease_cannot_append` |
| releasing a stale lease leaves the current holder's lease | engine: `releasing_a_stale_lease_leaves_the_current_holders_lease` |
| `read` past the tip or with `limit` 0 is empty; `tip` of an empty lake is none | engine: `reading_past_the_tip_or_with_limit_zero_answers_nothing` |
| an event at position 0 with a fence other than 0 is refused | engine: `an_event_at_position_zero_with_a_fence_is_refused` |
| sealing under a stale lease is refused; two seals list contiguous ranges | engine: `sealing_under_a_stale_lease_is_refused_and_two_seals_list_contiguous_ranges` |
| a seal begins the next segment with its first events in one step; one that cannot (events that do not continue the log by hash or by position, none, imported events behind appended ones, an error from `first`, a stale lease, which is refused before `first` is asked) seals nothing and leaves the tip, with no segment sealed and behind a sealed one | engine: `a_seal_that_cannot_begin_the_next_segment_leaves_the_log_as_it_was` |
| `open` of a missing lake is `Unavailable`; `Create` of an existing one opens it | engine: `opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one` |
| a commit backwards, or with a put past `through`, is `Invalid`; a view does not see an open transaction; a view keeps its position after later commits | engine: `a_commit_backwards_or_with_a_put_past_through_is_invalid`, `a_view_does_not_see_an_open_transaction`, `a_view_keeps_its_position_after_later_commits` |
| a put of the same thing at the same position replaces it | engine: `a_put_at_the_same_position_replaces_the_state_there` |
| a key put forty thousand times answers at the head, at an earlier position, in a page and in a count as a key put twice | engine: `a_much_written_key_is_put_and_read_as_one_written_twice`. What a put and a read of such a key cost is a count in each disk engine's own `tests/much_written.rs`, never a time: the steps SQLite's statements take, the tuples PostgreSQL fetches; the memory engine keeps a key's states in order and seeks by construction |
| the same states under a view's kept name, `sys:kept.<view>`, and under an ordinary one answer alike through `get`, `get_many`, a walk by key across batches, a page under a filter, in a field's order and under `equal`, and a count, at the head and at an earlier position, over a key put thousands of times, keys first put after that position, and an ended, a retired and a restricted key; an output recorded as often is read at both | engine: `a_kept_name_answers_as_a_stored_type_holding_the_same_states`. The plan of each statement a kept name is read by, and what a read costs, are in each disk engine's own `tests/kept_name.rs`: the steps SQLite's statements take, the tuples PostgreSQL fetches and that it walks a name's heads, `records_head`, with a seek of each head's state, with statistics and without; SQLite's `src/projection/tests.rs` counts that two types put in turn are each read once |
| a transaction of more than a page of puts leaves what putting them one by one leaves: the head indexes, the earlier states, the memberships, an order index and a vector index asked for in it; a view opened in it after a page of its puts reads what was committed; one refused at its commit leaves nothing; on PostgreSQL a page the server refuses leaves nothing of its transaction and a commit after it leaves the session clean, a vector index asked for a page before the commit is built at the commit, and a search reads the committed vectors until then; the states of two records put record by record, below a declaration's position and at it, after the declaration, leave what position order leaves | engine: `a_transaction_of_many_puts_leaves_what_putting_them_one_by_one_leaves`; PostgreSQL: `tests/pages.rs`, `a_setting_written_a_page_before_its_commit_builds_the_index_at_the_commit`, `a_transaction_written_by_pages_is_counted_whole` |
| `scan`'s order, `after` and `limit`, including a limit of 0 and values of mixed kinds | engine: `mixed_kinds_sort_the_same_on_every_engine`, `a_scan_continues_strictly_after_its_keyset_position` |
| a key-ordered `scan` read in batches answers as one scan over every state | engine: `a_key_ordered_scan_answers_across_its_batches_as_one_scan` |
| a `scan` by key that must hold a text exactly, by `equal` or by a term of its filter, reads from `after` and not every record holding the text for each page, past those that do not qualify, an ended key and keys a collation orders otherwise; a `count` under it reads the records holding the text | engine: `a_page_by_key_under_an_exact_text_reads_from_after_not_every_match` |
| `count` is the number of records its scan selects, across batches, at the head and earlier; `find`'s `count()` is a full walk's length, holding a batch | engine: `a_count_is_the_number_of_records_its_scan_selects`; suite: `a_count_answers_as_many_records_as_a_full_find_walk`, the streaming check's `count` rows |
| a `find` page crosses the seam as about `limit` rows, with or without a filter only the store evaluates | store: `tests/find_pages.rs` |
| `scan`'s filter matches the evaluator exactly, including comparisons across kinds and filters that error; the flags and `valid_at` | engine: `scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors`, `scan_honours_retired_restricted_and_valid_at` |
| `ranked`'s text list: its hit set, its language, searchable fields only, live at the head only (restricted included), ties by key, `k` | engine: `lexical_hits_are_the_pipelines_over_searchable_live_records`, `lexical_finds_every_token_the_pipeline_makes_beyond_ascii`, `lexical_over_named_fields_is_the_pipelines_over_those_fields` |
| `ranked`'s lists are of one state, with its position: an open transaction moves neither, a commit both; a record retired at the head is no candidate | engine: `ranked_answers_every_list_from_one_state_and_says_its_position` |
| a fold that commits between two statements of one `ranked` call is in none of its lists; on PostgreSQL the call holds every table it reads from its first statement, and a `clear` queued ahead of it is in its position | SQLite: `src/tests.rs`, `a_fold_committed_inside_one_ranked_call_is_in_none_of_its_lists` (the reader's statement trace commits the fold); PostgreSQL: `tests/ranked.rs`, `a_fold_committed_inside_one_call_is_in_none_of_its_lists`, `a_call_holds_every_table_it_reads_from_its_first_statement`, `a_clear_queued_ahead_of_a_call_is_in_its_position` |
| A search's candidates are judged at the position they were read at: a re-key, a hold placed or lifted, a write between two pulls, a field or a link type renamed in between, a searchable field the reader may not read declared in between; a hold placed in between keeps its record out of a pattern started by a search and out of a fused search, and a record given to another is no hit of a holder of `only` | store: `tests/search_position.rs` (the memory engine, a write planted before `ranked` answers), the ten rows of the file |
| `ranked`'s vector lists: the exact top `k` by cosine, ties, a zero vector, a wrong length; `put` replaces, also over a state's vector; `clear` empties the index | engine: `nearest_ranks_by_cosine_with_ties_by_key`, `a_zero_vector_scores_zero_and_a_wrong_length_is_invalid`, `put_replaces_a_vector_and_remove_forgets_it`, `put_and_remove_answer_alike_over_a_vector_a_state_holds`, `clear_empties_the_projection_and_the_vector_index` |
| `declarations`, `is_member`, `members`, `output`, `applied` and `applied_many` at an earlier position; `mark_applied` keeps the first position | engine: `declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position`, `the_first_applied_position_is_kept` |
| `replaced` with the head's declarations answers `declarations` at every earlier position, and two ranges that meet answer the one over both | engine: `what_was_replaced_since_a_position_and_what_is_in_force_hold_what_was_then` |
| `of_source` answers a source's positions as `matching` answers a filter's | engine: `the_index_answers_a_sources_positions_in_order_to_its_through`; store: `forget_index`'s `a_preflight_reads_the_sources_events_as_a_walk_does`; suite: every row of 058 |
| `matching` answers a filter's positions in order, each once, to its `through`, and nothing past the view or past what the projection has folded | engine: `the_index_answers_a_filters_positions_in_order_to_its_through`; store: `read_through`'s `past_the_views_position_a_poll_reads_the_log`; suite: `a_filtered_poll_answers_the_log_unmasked` and every row that follows the log through `the_index_answers_the_log` |
| `proposals`' filters, a `call`'s type, and its order | engine: `proposals_filter_by_state_and_type_newest_first` |
| `proposals_before`, at the head and earlier, for every state and pages of one, two and four, answers what `proposals` answers whole | engine: `proposals_come_newest_first_a_page_at_a_time`; store: `tests/find_pages.rs`, `a_listing_of_proposals_judges_its_page_and_no_more` (one page of the engine's for a page of the listing's) |

## `PrincipalResolver`

`resolve(lake, credential) -> Principal`. A credential is an API key, a
bearer token or a local user; the answer is one principal with its
grants, for one lake. Refused when the credential is unknown.

The default resolves an API key or a static configuration entry to a
principal. A host that has identities, roles, teams or policies compiles
them into grants here (`docs/decisions/006-grants-not-roles.md`). The
resolver belongs to the doors (the CLI, the server), not to the store.

## `FunctionBacking`

`kind()` names the backing kind, an open token; `call(function call) ->
value` computes one output from `{ lake, function, inputs, config }`.
The call also carries `cause`, the write that made it due (its position,
writer, record, source and context, and an accept where the call was
proposed), which is no part of the call's digest; a function declared
above `low` risk is called only once its proposed call is accepted
(`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`).

The default is `expr`, evaluated inside the store in the one expression
grammar. Because it is deterministic, its results are recomputed on
replay rather than recorded. `lakelands_store::ExprBacking` is that
backing as the seam presents it, for a host that binds it and calls a
function host-side. No door calls a function ad hoc: a function is
computed by the store where the ontology names it
(`docs/decisions/022-functions-are-declarations.md`). The call's `config` carries the expression as
`{ "expr": … }` (typed or text form), its `inputs` the values by input
name, and the answer is the computed value, a number as an exact decimal
string. The fold computes function fields with the same evaluator in
process, so the two can never disagree.

**Any other kind computes fields through the seam.** A function of
another kind is declared only where the host bound a backing of that
kind in `Seams::backings`; otherwise the declaration is refused as
`Unbound`, naming the kind. What the store guarantees for such a
function (`docs/decisions/022-functions-are-declarations.md`, extended):

- The store calls the backing after the append that made a field due,
  never from the fold, a read, opening a lake or `rebuild`. It makes one
  batch at a time per lake: the embeddings of one model, or one call.
- It records every answer as `output_recorded`. The answer is keyed by
  the seam `function:<kind>:<function>` and the digest of
  `{ "function", "inputs", "config" }` (`docs/log-format.md` §2.5). A
  fill of an answer the lake already holds names it by `output_at`
  instead of carrying it again; a failure is always carried
  (`docs/decisions/053-a-recorded-output-is-recorded-once.md`).
- It never sends one input to a backing twice: a recorded output is
  read back for every later call with the same input, failures
  included, and an answer is held, one per call, from the moment the
  backing gave it until it is recorded, so a recording that fails (for
  one call, or for the first text of a batch of embeddings) is made
  again by the next batch without a second call. The hold is the
  handle's, and is lost with it: by a process that ends between an
  answer and its recording, and by a `rebuild` meanwhile. A door's call
  dropped in the middle of its drain loses nothing: what it owed stays
  owed on the handle, and each later write that drains makes a batch of
  it after its own. A field so left
  waits until its call is due again (a write to its inputs, or a
  `recompute`), and that input may be called for once more. The one exception is deliberate: declaring the function, or
  a type whose fields it computes, again calls once more for an input
  whose recorded output is a failure. The later output holds
  (`docs/decisions/022-functions-are-declarations.md`, extended).
- A failure (an error, a timeout, or an output that is not a value of
  the field's type) clears the field, and the field's metadata carries
  the reason as `error`. An output that holds an integer beyond
  ±(2^53 − 1), a backing's or an embedding's vector, is recorded as a
  failure itself, since the log writes no such integer
  (`docs/log-format.md` §1.2). A cleared field without `error` is one whose
  output is not recorded yet.
- A fill is a change of the field like any write's. Derived fields
  that read it are recomputed, triggers fire and object sets change,
  caused by the output.
- A type's computed fields read one another without a cycle; a
  declaration that closes one is refused.
- A write that made calls due answers with the lake's head once its own
  are made, and one batch of the entries no write waits for any more:
  the position from which its filled fields are visible. It waits for
  the batch in flight, its own and that one, not for a backlog another
  write waits for. A write with nothing due answers at once, with its
  own last event's position.

A host adds a kind by implementing the trait and passing it in
`Seams::backings`, with no change to the store.

**`http` and `exec`** ship in `lakelands-backings`. The CLI and the
server bind them from the `backings` section of the root's
`lakelands.json`. Each is configured only if its section is present, and
reaches only what that section allows; nothing is read from the
environment.

```json
"backings": {
  "http": { "hosts": ["geo.example", "localhost:8080"],
            "timeout_secs": 10, "max_reply_bytes": 1048576,
            "ca_file": "/etc/ssl/certs/ca-certificates.crt" },
  "exec": { "commands": { "geocode": { "program": "/usr/local/bin/geocode",
                                       "args": ["--json"] } },
            "timeout_secs": 10, "max_output_bytes": 1048576 }
}
```

- `http`:
  - A function names `config.url`, an `http` or `https` URL whose host
    the operator lists, either as `host` (any port) or as `host:port`.
    Any other host is refused before a connection is made.
  - The backing `POST`s the call `{ "function", "inputs", "config" }`
    as JSON. A `2xx` reply's JSON body is the output. Any other status
    fails, redirects included: a redirect is never followed, since it
    could lead off the list.
  - No proxy is used. `https` trusts the authorities in `ca_file`, or,
    without one, the first system bundle found at a well-known path. A
    relative `ca_file` is the root's, as the embedding client's is,
    never the working directory's.
- `exec`:
  - A function names `config.command`, the name of a command the
    operator configured, never a program path, so a declaration cannot
    run anything the operator did not name.
  - The program is started directly, with no shell and an empty
    environment. It receives the call as JSON on standard input and
    answers one JSON value on standard output.
  - A failure status, output beyond the limit, or a run past the
    timeout fails the call. The last two also kill the program. A
    failure is recorded as the exit status in a fixed sentence. What the
    program writes on standard error goes to the server's own standard
    error, never into the log.
  - `program` must be an absolute path; a configuration naming a
    relative one is refused when it loads.
  - An `http` section that lists no host is refused when it loads.

**The trust boundary.** Allowing a host trusts all of it:
- Every port, unless the entry names one, and every path on it.
- A principal who may declare functions (the `schema` grant) can have
  records' fields sent to any endpoint on an allowed host, and can have
  any allowed command run on them.
- List hosts and commands for what they will receive, not only for
  where they are. An IPv6 literal is listed in brackets, as the URL
  writes it (`[::1]:8080`).
- A reply that is well-formed JSON but not a value of the field's type
  fails the field. It is still kept in the log as the output, up to
  `max_reply_bytes` (`max_output_bytes` for `exec`).

## `EmbedBackend`

`model()`, `dimensions()`, `embed(texts) -> vectors`, one vector per
text, in order. A backend answers `Invalid` when it refuses the request
for what it carries, so that asking again with the same texts would get
the same answer, and `Unavailable` for anything else: the network, the
endpoint, its limits, a reply it cannot read.

A lake reads the backend `Seams::embed` brings at open, or the one a
host binds later on the open lake with `LakeStore::bind_embed`, which
replaces it whatever its model
(`docs/decisions/022-functions-are-declarations.md`, extended). A bind
appends nothing itself; it, and a writer's open with the backend, makes
what waits on its model in a task of its own, a recorded failure aside.
A batch reads the bound backend
when it asks and holds it until it is answered, so a bind meanwhile
reaches the next batch. `bind_embed` is the host's alone: the
network doors answer it `Unbound` as a host operation, no tool of the
catalog names it (MCP, REST and stdio serve that one catalog), and the
WIT does not carry it, since a seam is bound by the host, as at `open`,
and nothing a guest holds can be one.

The store calls the backend for two things.

**A hybrid `search` without a caller's vector** embeds its query text
through the bound backend and ranks over the result. That is a read,
and nothing is recorded. A backend that answers other than one vector
for the one text makes the search `Unavailable`, and so does a backend
that cannot be reached or fails, whatever error it answers. The lexical
fallback is for an absent backend, never a failing one, so the same
query never ranks differently by the network's weather. A search over a
field declared with `embed` whose model or dimensions are not the bound
backend's is refused as `Unbound`, naming both: that is a configuration to
fix, not weather. The search is judged for its caller first, as any
search is, by the shape of the lake it reads (lakelands#741): only one it
may make has its field's model compared and its text embedded, so a search
refused for its caller calls nothing and names no model. The model is the
head's declaration even for a search as of an earlier position, since the
engines index the head's vectors. A vector search, and a hybrid search
that brings its own vector, never call the backend; a vector the caller
says another model made than the field's is refused as `query_model`
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
A hybrid search without a vector embeds only over a field declared
with `embed`: over a vector field its writers fill, whose space the
lake does not know, it is refused as invalid on the field, with a
backend bound or not, and over a field that holds no vectors as any
vector ranking is
(lakelands#743). Either refusal is made before any call.

**A search that fuses rankings** weighs each as it asks
(`docs/decisions/046-a-hybrid-search-fuses-the-ranks-a-read-may-see.md`):
a hybrid search its text (`text_weight`) and its vector (`weight`), a
vector search its `field` and every further field it names in `also`.
The store asks the engine for every ranking of a pull in one `ranked`
call, each a list of its own, and fuses them itself, so an engine sees
no weight and fuses nothing. A ranking the search gives no
weight takes the one its type's setting `search_weights.<type>` gives it
(below), and a ranking neither weighs weighs 1.

**A field declared with `embed`** is the embedding of text fields of its
record (`docs/decisions/022-functions-are-declarations.md`, extended):

```yaml
- name: summary_vec
  type: { kind: vector, dimensions: 1536 }
  authority: derived
  embed:
    from: [title, summary]
    model: text-embedding-3-small
```

- The field is a `vector` with `dimensions`, of authority `derived`,
  with neither a `derived` expression nor a `function`. `from` names one
  or more `text` fields of the same type, not the field itself, each
  once. `depends_on` is implied by `from` and is not written beside it.
- `from` names a `sensitive` field only if the embedding field is
  `sensitive` too. A vector gives its text away to whoever can read it,
  and the text is sent to the operator's endpoint in any case.
- It is declared only where the bound backend names the model and
  answers the field's `dimensions`; otherwise the declaration is refused
  as `Unbound`, naming what it needs. An embedding the type in force
  holds with the same `from`, model and dimensions needs no backend,
  since declared unchanged it makes nothing due; `recompute` of the type
  always needs one. A due embedding whose backend is
  gone, or names another model or other dimensions, when the call would
  be made is not made; the field waits. It is made, with the backend
  bound, when its record is written again, `recompute` names its type,
  or a writer's open or a bind brings that backend, which make the
  waiting calls and call no failure again. A retried embedding waits the
  same way: it reads as waiting (no value, no metadata) though the
  log's latest output for its text is still the failure.
- The text is the `from` fields' values, in declared order, joined with
  one blank line; a field without a value is left out. With no text,
  the field is cleared and keeps its metadata without an `error`
  ("nothing to embed"), and nothing is called. A field waiting for its
  vector has neither a value nor metadata.
- Each vector is recorded as `output_recorded` under the seam
  `embed:<model>`, keyed by the digest of `{"texts": [text]}`
  (`docs/log-format.md` §2.5), with the model as the procedure's name
  and the type's version that declared the `embed` as its version.
  The field's `procedure` is the model and its `procedure_version` the
  type's version when the vector was produced. A later declaration of the type that leaves the vector's
  text and model as they are rewrites no record, so the metadata keeps
  that version. One
  text is embedded once per model in a lake; a replay, and every later
  record with the same text, read the recorded vector.
- A change to `from` or `model` is a redeclaration of the type. It makes
  the field due on every live record, as adding the field does.
- A failure is recorded as `{"error": …}` and clears the field with its
  reason. A redeclaration retries it.
- **At most 64 texts of one model and dimensions go in one request**
  (the store's `EMBED_BATCH`).
  The store never holds more than one batch of vectors at a time,
  however many records a backfill makes due, and a declaration owes one
  sweep over its type, not a call per record, so its memory per drain
  stays bounded. The client may split a batch further by its own
  `max_batch`.
- **A request refused as `Invalid` is asked again in halves**, the first
  half first, until each refused text is alone, and only a text refused
  alone records the refusal (lakelands#658). The others in its batch are
  embedded. One refused text in 64 costs 13 requests, all 64 refused cost
  127; a request that succeeds costs nothing more. Any other failure, and
  an answer with the wrong number of vectors, ends the asking: it is
  recorded for every text of the batch not answered yet, and the texts
  answered before it keep their vectors.

There is no default backend: without one, `search` ranks lexically and
over vectors callers supply, and says in its answer which ranking it
used (`ranked_by`).

**The embedding client** ships in `lakelands-backings`. It speaks to
any endpoint shaped like `POST /v1/embeddings`. The CLI and the server
bind it from the `embed` section of the root's `lakelands.json`, one
client for all of the root's lakes; without the section, no backend is
bound.

```json
"embed": {
  "endpoint": "https://api.example.com/v1/embeddings",
  "model": "text-embedding-3-small",
  "dimensions": 1536,
  "api_key_file": "secrets/embed.key",
  "send_dimensions": false,
  "max_batch": 64,
  "timeout_secs": 30,
  "max_reply_bytes": 16777216,
  "ca_file": null
}
```

- `endpoint`, `model` and `dimensions` are required. The endpoint's
  host is the only host the client reaches; no declaration can name
  another. `http` is allowed, and meant for a server on loopback.
- `api_key_file` names a file whose trimmed contents are sent as
  `Authorization: Bearer <key>`. It is read when the root is opened, and
  a file that cannot be read, or is empty, stops the command. The key
  never comes from the environment
  (`docs/decisions/005-no-environment-reads.md`). Without it, no header
  is sent.
- `api_key_file` and `ca_file` are relative to the root's directory,
  never to the working directory.
- The root refuses to open, naming the reason, when the endpoint is not
  an `http` or `https` URL or carries user information (a key goes in
  `api_key_file`), when `dimensions`, `max_batch`, `timeout_secs` or
  `max_reply_bytes` is zero, or, for `https`, when the host is not a
  name TLS can verify or a named `ca_file` cannot be read or holds no
  certificate, and when the key file holds characters a header cannot
  carry. The
  system bundle, when no `ca_file` is named, is looked up at the first
  request.
- The client sends `{"model", "input": [texts…]}`, at most `max_batch`
  texts to a request, in order. With `send_dimensions`, it adds
  `"dimensions"`, for endpoints that shorten vectors on request.
- A `2xx` reply is read as `{"data": [{"index", "embedding"}, …]}`.
  Items are placed by `index`, every index must appear exactly once,
  and every vector must have `dimensions` finite values. Other members
  are ignored. `encoding_format` is never sent, so the default float
  form is expected; base64 is not read.
- The transport is the `http` backing's: no proxy, no redirect
  followed, `https` trusting `ca_file` or the system bundle, a reply
  limit, and a timeout covering each whole request, connecting
  included.
- A `400`, `413` or `422` is `Invalid`: the endpoint refuses what the
  request carries. Every other failure is `Unavailable`, a `404` and a
  `429` included. Either names the endpoint's host, port and path and
  what went wrong. It never carries the key, the texts, the request, the
  reply or a path on the server: the texts are a caller's query, and a
  failure reaches every caller. A fault in one item of a `2xx` reply (its
  dimensions, a value that is not finite, the number of items) is
  `Unavailable` and fails the texts beside it. Halving sends again the
  client's requests of a batch that had already succeeded.
- Embedding no text makes no request.

## `Reranker`

`model()`, `rerank(call) -> scores`. A call holds the lake, the searched
type, the query text and the candidates in the first stage's order, each
a key and a text: its searchable fields the reader reads, in declared
order, joined by a blank line. A reranker answers one score per
candidate, in order, and higher ranks first. A failure, a count of scores
other than the candidates' and a score that is no number are all
`Unavailable` to the caller of the search, whatever error the reranker
answered, and the store never falls back to the first stage's order
(`docs/decisions/067-a-search-may-take-a-second-stage.md`).

A lake reads the reranker `Seams::rerank` brings at open, or the one a
host binds later on the open lake with `LakeStore::bind_rerank`, which
replaces it. `bind_rerank` is the host's alone: a remote handle answers
it `Unbound` as a host operation, no tool names it, and the WIT does not
carry it. A call is a read, as the query embedding of a hybrid search
is: nothing is recorded, and a replay of the log never makes one. A
window with no candidate asks nothing.

## `VerbExecutor`

`name()`, `execute(invocation) -> value`. A verb whose declared effect
reaches outside the lake names an executor; when that verb runs (at once
if declared `low`, on acceptance otherwise, as
`docs/decisions/013-verbs-run-by-declared-risk.md` decides),
the store hands the executor a `VerbInvocation`: the lake, the verb, the
record as it stood at the position that made the verb due, where the
call was judged, so that a later write, a retirement included, does not
reach the executor, whole and **unmasked** (none for a verb called on
its type, which has no record), the payload, and
the position of the event that made it due. The lake and that position
are the executor's dedup key. The executor is the host's code, bound by
the host, and decides what it returns; the caller sees only the outcome
and the recorded output, never the record the executor was given. A host
that wants an executor to see only what the caller may see applies the
caller's grants in the executor. Its answer is recorded as an
`output_recorded` event caused by that event, with `input` the digest of
`{ "verb", "record", "payload", "position" }`. No lake in the door
checks binds an executor, so the record an executor is handed is pinned
by the store's own test, on the memory engine.

An executor that fails does not undo the verb: the event that made it
due is in the log, and its position is real. The failure is the recorded
output — the error as the wire carries it (`{"error": "refused",
"reason": {…}, "message": "…"}` for a refusal; lakelands#812; before, `{"error": "<its text>"}`). A
call the host declines by policy is answered `Error::declined(verb)`, or
`Error::declined_because(verb, reason)` to record why
(`docs/hosting.md`). The caller's outcome
is `applied` at the position that made the verb due, and carries the
recorded output in its `output` member, the error form included, so an
agent that called an external verb has the answer in the reply. Its
`through` member is the position the output was recorded at
(`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
extended). The verb
is not run again for that position. A host that wants a retry calls the verb again, which is a new
position.

A call retried with the same idempotency key runs nothing: a position
reaches its executor at most once. The retry answers the first outcome,
the position that made the verb due, with the recorded output and its
position, found by a seek of the id the output is recorded under
(`docs/log-format.md` §3), however far the log has grown since. An
executor that answered `null` has answered, and the network doors carry
it as `"output": null`. Where no answer is recorded,
the retry is refused as `unanswered`, never `applied`: the call is in
the log at the position its sentence names, and the retry runs nothing.
If the call is still running, its answer will be recorded and a retry
then answers it; if its first answer said that its executor was not
handed the call, the host calls the verb again under another key or with
none. A retry through `upsert`, `retire`, `link` or `unlink` under the
key of such a call is answered alike. An accept sent again answers by
the verb as it stood at the acceptance, whatever is declared since: a
call of an external verb is answered as a retry of a call is, with
"propose the call again" in the place of "call the verb again", and any
other is `decided`.

A call whose own fold failed is in the log, and no executor was handed
it: the store's answer to that call says so in its sentence (the verb's
executor was not handed this call, and sending it again under its key
will not run it: call the verb again under another key or with none; a
call without a key is told to call the verb again). An acceptance whose
fold failed is answered the same in its own words (the proposal is
decided, so accepting it again will not run it: propose the call again);
the proposal is spent. The executor is handed nothing either where the
drain of due calls before the hand-out failed: the writer may be behind
then, and the executor's answer could not be recorded. Nor is it where
the read of the record for the executor fails. The call, folded at its position, is answered that failure as
`unavailable` with the same sentence. Where the executor answered and
the recording of its answer failed, the call is answered that failure,
saying the executor answered and with what: an answer that reached the
log is answered to a retry once the next write has folded it, and one
that did not is lost but for that sentence, and its retry is
`unanswered`. The network doors know these answers by their words
(`THE_EXECUTOR` in the contract's errors) and never advise a retry:
sent again without its key, a call whose executor answered would run
twice. An answer that holds an integer beyond ±(2^53 − 1) is lost
that way too: the log writes no such integer (`docs/log-format.md`
§1.2), so the answer is never recorded, and its call is answered with
the answer and the number that kept it out.

The append, the drain, the hand-out and the recording run in a task of
their own, which the door awaits: a caller who goes away, as a request
whose client closed its connection does, stops none of them, and the
answer is recorded for its retry.

There is no default: a verb whose executor is not bound is refused as
`Unbound`, never faked. The store checks it when the verb is called or
a call of it is proposed, and again when a proposal to call it is
accepted, before anything is written.
`describe` names each external verb's executor and says whether one of
that name is bound when it answers, at any position, and the verb's tool
says the same (`docs/decisions/013-verbs-run-by-declared-risk.md`,
extended).

## `ChangeFeed`

`poll(from, filter, limit) -> batch`, `ack(consumer, through)` and
`wait(after, within) -> head`. The feed is the log filtered by kind and
type, read from a position
(`docs/decisions/004-the-change-feed-is-the-log.md`). `poll` answers at
once, possibly empty, never waiting for new events. `wait` is how a
consumer waits for them: it answers the head once the log is past
`after` or once `within` has passed, never past the head a `poll`
reads, so a consumer caught up at `next` waits on `next - 1` and polls
again (lakelands#1061). The store's feed wakes at the append its handle
publishes. A read-only handle follows the writer's fold: its poll reads
up to where the writer has folded, and its wait looks there every
`REACH_POLL` (10 ms) until it is past `after` or `within` has passed
(lakelands#1394). Each waiting consumer reads the fold on its own, so
a read-only handle with many feed streams open costs its engine about a
hundred small queries a second for each of them while the lake is
quiet; a host serving many subscribers serves them from the writer, or
fans one feed out to them. The trait's
own `wait` answers at once, so a feed written elsewhere keeps working. A filter that names
something is answered from the projection's index of the log up to the
view's position (`ProjectionView::matching`): only the events it names
are read from the log, each opened, verified and checked again by the
same rule, so the index can only narrow what a poll reads and an
answered event always comes from the log; the events a run of them
reads through between two are dropped unopened. Past the view's
position, as while the projection is rebuilt, where the index decides
nothing from `from` on, and for a filter that names nothing, the poll
reads every event from the log. It stops at
`limit` matches, at the head, where the index stops deciding, or once it
has read `FEED_EXAMINED` (4 096, `lakelands_api::seam`) events, matched
or not, whichever comes first, and answers `next` there. So a poll may
answer no events with `next` at or below `head`: an empty batch is not
the end of the log. A consumer is caught up when `next` is past `head`,
and reads on at once until then; one whose filter the index decides
whole and that matches nothing more is caught up in one poll, whatever
the length of the log, and one that names types and kinds and no key is
decided at most `lakelands_core::feed::WINDOW` (65 536) positions a
poll, so it is caught up that many positions at a time (suite:
`STREAMS`, which follows the feed of its large lake with a filter that
matches nothing, one record's history and its checkpoints):

- `events`: at most `limit` events (a `limit` of 0 is read as 1, and one
  above `FEED_EXAMINED` as `FEED_EXAMINED`) from `from` up to the head,
  each verified as it is read, in position order, whose kind is one of
  `filter.kinds` and whose type is one of
  `filter.types`; an empty list in the filter admits every kind or every
  type. An event has a type when it is a `record_*`, `trigger_fired` or
  `object_set_changed` event, a `declared` type or a `migrated` one (a
  trigger on a relation carried by link records watches its link type,
  `sys.link.<type>.<relation>`, so its `trigger_fired` events carry that
  type and the link's key; `docs/decisions/035-a-link-is-a-record.md`); an
  event without one passes only a filter with no types. A verb's events
  name no type, even for a verb on its type: `verb_called`,
  `write_proposed`, `write_accepted` and `output_recorded` pass no
  filter that names types, while `proposals` filtered by type lists a
  proposed call under its verb's type. The kinds are
  free strings, so a relay may name one a newer format adds;
  `lakelands_api::event::KINDS` lists the kinds of this format version.
  A non-empty `filter.keys` admits only events about a record with one
  of those keys: the `record_*` events, `trigger_fired`,
  `object_set_changed` and `verb_called`, each naming its record.
  - With `types` named, the event's type and key must both match, so
    `types: [invoice], keys: [INV-1]` is one record's history.
  - With `types` empty, a key matches in any type.
  - `verb_called` names no type, so it passes only when `types` is empty,
    as for the type filter.
  - No other event passes a key filter.
  - A link is a record of its link type, keyed by its own key, so a
    record's links are followed by naming the link type, not the
    record's key.
  - A re-key is not followed: the old key answers the record's events
    up to the `migrated` event, and the new key those after it (suite:
    `a_feed_filtered_by_key_answers_that_records_events_only`,
    `a_key_filter_does_not_follow_a_rekey`).

  A key filter is answered from the index as every filter is, so one
  record's history asked for from position 0 reads that record's events
  alone (lakelands#678).
  `lakelands-client` refuses a filter with keys as invalid, naming
  `keys`, until the REST feed takes them.

  A consumer tells a record made from one changed by the event alone: a
  `record_upserted` or `record_ingested` event carries `made: true`
  where its write found no live record under the key, and leaves the
  member out where it changed one. With `version` 1 the record is made
  for the first time; above 1 it was retired before and is made again,
  which a consumer that filters by kind would otherwise miss, since it
  never sees the `record_retired` between (`docs/log-format.md` §2.3;
  suite: `a_record_event_says_whether_it_made_the_record`).
- `next`: the position after the last position the poll decided, by
  the index or by reading the log, matched or not, which is where the
  next poll starts.
  `lakelands-client`'s `poll` answers at most `FEED_EXAMINED` events as
  well, but the server's stream behind it reads on to a match or to the
  head, one bounded poll at a time, so a remote poll whose filter matches
  little may examine up to the head on the server. The stream's
  `: next` keeps its meaning: caught up.
- `head`: the lake's last position when the poll began.
- `overruled`: the positions of the batch's `trigger_fired`,
  `trigger_dropped` and `object_set_changed` events that an act overruled:
  decided over a value it erased since, which the history without the
  value does not make
  (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`). The
  events stay in `events` as the log holds them. Empty, and left out of
  the wire form, on a lake that never erased anything. The MCP pump
  tells none of them, the deliverer posts none, and the REST feed marks
  each with the comment `: overruled` (`docs/rest.md`)
  (suite: `REMOVED_FIRED`, `FORGOTTEN_FIRED`).

The consumer keeps the position it has read up to. The store's own feed
keeps no consumer state, and its `ack` is accepted and does nothing; `ack` exists for feeds
that relay elsewhere (a remote client, a bridge onto a bus) and need to
know what they may release. So a consumer's state is entirely its own:
on the store's feed, `ack` is neither backpressure nor a release, and a
consumer that never calls it loses nothing. The dedup key of a change is its lake and its
position.

Lakelands has three consumers of its own: the MCP session's pump, which
tells a session of the changes its principal may see
(`lakelands-server`'s `watch`; `docs/hosting.md`), and the webhook
deliverer and the mirror below. How they are built, not a promise a test
holds: they read the feed as the host does, not through a door, and keep
their state on the host's side. The pump then decides what a session may be told
through the contract's reads as that session's principal (`get`,
`describe_type`), so a session learns nothing its principal could not
read.

**The webhook deliverer** is one of them, in
`lakelands-backings`. `lakelands serve --mcp` runs one per endpoint of the
`deliver` section of the root's `lakelands.json` and per lake that
endpoint names, and only for the lakes the process holds as their
writer.

```json
"deliver": {
  "endpoints": [
    { "name": "billing", "url": "https://hooks.example.com/lakelands",
      "lakes": ["finance"], "kinds": ["trigger_fired"], "triggers": ["overdue"],
      "secret_file": "secrets/billing.hmac", "start": "head" }
  ],
  "poll_millis": 1000, "batch": 100,
  "retry_first_secs": 1, "retry_max_secs": 300,
  "timeout_secs": 10, "max_reply_bytes": 65536
}
```

- **What it sends.** Only `trigger_fired` and `object_set_changed`
  events, by default both. `kinds`, `triggers` and `sets` narrow them;
  any other kind is refused when the configuration loads. One the batch
  names `overruled` is not sent: it is journaled, and the deliverer
  moves on. Both carry
  keys only, so no field value leaves the lake this way: a receiver
  reads the record through a door, under its own grants.
- **The request.** One `POST` per event, in log order. The body is the
  event's canonical bytes, as a line of the feed holds them. The
  headers:
  - `Lakelands-Lake` and `Lakelands-Position` (together the dedup key),
    `Lakelands-Event` (the event id) and `Lakelands-Attempt` (from 1);
  - with `secret_file`, `Lakelands-Signature: sha256=<hex>`, the
    HMAC-SHA256 of the body's bytes.

  A receiver verifies the signature over the raw body bytes before it
  parses them. The body carries the lake, the position and the id, so a
  replayed request is a duplicate the receiver already recognises, and
  no timestamp is signed.
- **The connection** is the one the `http` backing and the embedding
  client use: no proxy, no redirect followed, `https` trusting `ca_file`
  or the system bundle, a timeout over each whole request, a reply
  limit.
- **Delivery is at least once and in order.** The position is written
  after each event the deliverer is done with, so a crash between a
  request and that write sends the event again. Each endpoint has its
  own position and pace.
- **Answers.**
  - `2xx` is delivered. The status is the verdict: the reply's body is
    read up to `max_reply_bytes` and dropped, and a body that cannot be
    read changes nothing. Any other reply's body is not read at all.
  - A timeout, a failed connection or TLS, `408`, `429` or `5xx` is
    tried again after a wait that starts at `retry_first_secs` and
    doubles up to `retry_max_secs`, without end. So are `401`, `403` and
    `404`: they say the endpoint is misconfigured (a wrong secret, a
    wrong path), not the event refused, and once the operator fixes it
    every event gets through.
  - Any other status, `3xx` included, is the receiver refusing this
    event. It is recorded, said in one line on standard error, and the
    deliverer moves on.
- **Its files** are the host's (004, extended), under
  `deliveries/<lake>/` in the root:
  - `<endpoint>.json` holds the position to read from next,
    `{"next": <position>}`. Without it, an endpoint begins after the
    lake's head (`start: head`, the default) or at its first event
    (`start: genesis`). The operator rewinds by editing it while the
    server is stopped.
  - `<endpoint>.lock` is held while the deliverer runs, so a second
    process refuses to start rather than send everything twice.
  - `<endpoint>.ndjson` is the journal, one line per outcome:
    `position`, `id`, `attempt`, `at` (the host's clock), and `outcome`,
    which is `delivered`, `retrying` (with `why` and `retry_in_secs`),
    `refused` (with `status`) or `overruled` (at `attempt` 0: nothing was
    sent). It is only ever appended to, and opened
    for appending, so the operator rotates it by renaming.
- **What it says.** A message names the endpoint by its `name` and by
  host, port and path, never the URL as written, the secret or a body.
  A URL carrying user information is refused when the configuration
  loads.
- **Paths.** `secret_file` and `ca_file` are relative to the root. The
  secret is read when `serve` starts; a missing or empty file stops it.
  The whitespace around the file's contents is dropped before signing,
  so a receiver that reads the same file trims it too.
- **Checked when the root opens:**
  - every lake an endpoint names is one the root holds (only `lakelands
    lake new` may run before the lake it creates exists);
  - every endpoint has a name of letters, digits, `-` and `_`, used
    once;
  - every URL is `http` or `https`, without user information;
  - every endpoint names at least one lake, and its `kinds`, when given,
    are not empty and are among the two it sends;
  - `retry_first_secs` is not longer than `retry_max_secs`;
  - no number is zero.

**The mirror** is the third, in `lakelands-cli`: it copies types of one
lake of the root into another, the only way water crosses between lakes
(`docs/decisions/042-water-crosses-only-as-a-mirror.md`). There is no
query across lakes and no reference into another lake. A
root that shares a common lake's types with many lakes mirrors those
types into each of them. `lakelands serve` runs one mirror per
connection of the `mirror` section whose target lake it holds, and
refuses to start when it holds that lake but not as its writer.

```json
"mirror": {
  "connections": [
    { "name": "crm", "source": "crm", "target": "main", "principal": "mirror",
      "types": { "customer": "crm_customer", "partner": "partner" },
      "start": "snapshot" }
  ],
  "poll_millis": 1000, "batch": 100
}
```

- **What crosses.** `types` maps each source type to the target type it
  lands in. The target type's declaration is the list of what crosses:
  every field it declares, and the links of every relation carried by
  link records it declares. Everything else stays home: a source field
  the target does not declare, a field the `principal` may not read in
  the source, and a field the source marks `sensitive`. The feed is the
  log unmasked, so the mirror never takes a value from it. The feed says
  which record changed at which position, and the mirror reads that
  record `as_of` the position through `get`, as the principal. A record
  the principal cannot see there, withheld or not granted, does not
  cross, and its copy keeps its last state.
- **How it lands.** Each source type is one source of the target,
  `mirror:<source lake>:<source type>`, written through `ingest` with
  the source's `known_at` as `seen_at` and, as `upstream`, the source
  lake and the position of the state that was read: a field read with
  `with_meta` in the target says which state of the source it reflects
  (`docs/log-format.md` §1.1). The copy's fields are `source`
  fields that nobody in the target can author, and `purge_source` of
  the connection's source removes the copies whole
  (`docs/decisions/039-a-source-is-retracted-as-one.md`). A window
  carries only the fields that differ from the copy. Consecutive changes
  of one type are read once per key, at the key's last position, and
  ordered so that a record lands after the records it refers to. **The
  target's history therefore steps once per window, not once per source
  event.** In return, a window delivered again reads what the copies
  already hold and appends nothing, and a trigger on a mirrored type
  fires once per copy and window.
- **Retirements and holds.** A retirement crosses through `retire` (a
  link's through `unlink`), with an idempotency key naming the source
  position; a later write to the source key brings the copy back. A
  hold crosses through the host's `restrict` and `unrestrict` only when
  the principal holds the `restricted` grant. Each keeps its place in
  feed order.
- **It fails closed, by type.** At every declaration or migration of a
  mapped source type the mirror asks whether the target's declaration
  still admits the source's: the same key rule, and every field it
  declares present in the source with the same type, readable and not
  `sensitive`. If not, that type stops, and so do the types that refer
  to it. A type also stops when the target refuses its window, and when
  the source re-keys it while the target keeps the old key rule; the
  target's own migration to the same key rule moves the copies and every
  reference to them together. A stopped type keeps its position and the
  reason, said on standard error and in its `sys.mirror` record, and the
  connection's other types go on. A restart asks again from where it
  stopped. A new source field crosses once the target declares it, for
  the records that change after.
- **Where it starts.** A type with no position is copied as the source
  holds it at its head, then followed from there (`start: snapshot`, the
  default), or only followed from the head (`start: head`). History is
  never replayed: that would walk every copy through every past state
  and fire its triggers on the way. Retiring a type's `sys.mirror`
  record while the mirror is stopped copies it again at the next start,
  and every unchanged field appends nothing.
- **The principal** is one of the root's `principals`, resolved in each
  lake: in the source it needs to read what crosses, and `restricted`
  for holds to cross; in the target it needs `ingest`, write access to
  every field of the mirrored types, and the write grant on
  `sys.mirror`, where it keeps its position.
- **Its position** lives in the target lake, one `sys.mirror` record
  per connection and source type, link types included: `connection`,
  `source_type` (together the key), `source_lake`, `target_type`,
  `next` (the next source position to read; 0 while the type is still
  to be copied), and `stopped_at` and `stopped_why` when it stopped.
  The CLI's system vocabulary gives the type to the lakes a connection
  writes into, and to no other. The mirror writes each record after the
  window or action it counts is durable, so a crash in between delivers
  that window again, and it appends nothing. An operator reads the
  state as any record: `find sys.mirror`, with `where: "stopped_at is
  not null"` for the stopped types. Walk 3 (`docs/walks/walk-3.md`)
  mirrors one type and reads both the copy's `upstream` and its
  `sys.mirror` record.
- **Its files**, under `mirrors/<target lake>/` in the root:
  `<connection>.lock`, held while the mirror runs, so a second process
  refuses to start, and `<connection>.ndjson`, the journal, one line per
  window, per retirement or hold, and per stop. A position file an
  earlier version kept there (`<connection>.json`) seeds the records
  once, when there are none, and is renamed `<connection>.json.seeded`;
  it is never read again.
- **The source is a lake of the same root.** A source served elsewhere
  is not offered: the feed at a network door opens only to a principal
  that reads everything (`docs/decisions/037-the-feed-over-a-network-door.md`),
  and the mirror's reads must be the connection's principal.
- **Checked when the root opens:** every lake a connection names is one
  the root holds, and its principal one the root lists, with the write
  grant on `sys.mirror`; every connection
  has a name of letters, digits, `-` and `_`, used once, and maps at
  least one type; no lake is mirrored into itself; no `sys.` type is
  mapped; no target type has two sources; no type is mirrored into a
  lake and back out of it; no number is zero. **Checked when `serve`
  starts a mirror:** the target is held as its writer; every field of a
  target type is written by the `source` authority and writable by the
  principal, and every reference and link leads to a type the connection
  mirrors; and, for a type with no position yet, the target admits the
  source's declaration.

### Declaring a set over a large type

A set's declaration decides the membership of the records already there,
in its own append (`docs/decisions/004-the-change-feed-is-the-log.md`,
extended): one `object_set_changed` per record whose membership it
changes, caused by its `declared`. The writer is held while the store
walks the type's live records, 1 024 at a time, and drafts the changes.

Measured with `--release` at 100 000 live records, under some load from
other processes (`crates/lakelands-engine-sqlite/tests/set_declared.rs`
and `crates/lakelands-engine-pg/tests/set_declared.rs`, by hand):

| | the walk alone | 25 000 changes | 50 000 changes |
|---|---|---|---|
| SQLite: writer held, peak memory, on disk | 0.34 s, 3 MB, 0 | 0.88 s, 72 MB, 16 MB | 1.38 s, 145 MB, 33 MB |
| PostgreSQL: writer held, peak memory, in the schema | 0.57 s, 3 MB, 0 | 1.62 s, 104 MB, 26 MB | 2.62 s, 164 MB, 55 MB |

The changes cost, the walk barely does:
- **per change:** about 21 µs and 3 KB on SQLite, about 41 µs and 4 KB
  on PostgreSQL;
- **the walk:** about 0.35 s (SQLite) and 0.6 s (PostgreSQL) per
  100 000 records.

`Limits::max_set_changes` (20 000 unless the location sets it) bounds the
changes of one declaration. Past it the declaration is refused
`too_large`, naming the set and the ceiling, and nothing is appended. The
walk stops at the first change past the ceiling. A host that needs the
number counts what a new set would take in with `find`, the set's filter
and `total`.

A set over a large type is declared in one of two ways:
- **Raise `max_set_changes` in the lake's location,** at the cost above.
  On PostgreSQL, 100 000 changes hold the writer for about 4.7 s and need
  about 400 MB. Other writes wait meanwhile, so this is a step for a
  quiet moment.
- **Widen the filter in steps under the ceiling,** with no limit raised.
  Declare the set with a narrower filter (`status = 'open' and n <
  20000`), then again with a wider one, and last with the filter meant.
  Each step is a whole declaration whose events agree with the filter in
  force after it. A crash between steps leaves a narrower set, never a
  wrong one.

The walk is not bounded: a type of a million live records holds the
writer for several seconds even for a set nothing joins.

A set declared again is compared with its members so far, which the walk
reads once before it starts (`members`), not once for each record. A
migration and a re-key read each set's members the same way. What a
migration itself costs at size is in `docs/hosting.md`, "Rules the host
inherits".

A migration decides the membership of the records it migrates again,
in the walk its preflight makes over the type's records anyway, and
appends the changes in its own append, directly after its `migrated` and
caused by it. Its changes count against the same `max_set_changes`, over all sets of the
type; past it the migration is refused `too_large`, naming a set. Narrow
the sets first (declare them again with a narrower filter), migrate, then
widen them in steps. A re-key also decides the records of every other
type whose reference to a moved key it rewrites, for the sets that read
that reference, and a migration that converts a relation's fields decides
the sets over the relation's link type; all count against the one
ceiling. A type that holds such a reference and has such a set is read
once more, a page at a time. A migration that drops or renames a field a set's
filter reads never gets that far: its preflight refuses it, naming the
set and the field.

## `SystemVocabulary`

`types()` lists a host's own `sys.` types as type declarations;
`may_hold(lake, type)` says whether a lake may hold one. Lakelands
reserves only the system types its own fold reads: `sys.source`,
`sys.provenance`, `sys.setting`, `sys.aging_rule`
(`docs/decisions/003-reservation-by-namespace.md`). Today the fold reads
`sys.setting` and `age` reads `sys.aging_rule`; the other two are
reserved ahead of their readers. The default adds none and allows those
four everywhere. Lakelands' own first host of the seam is `lakelands
serve`, whose vocabulary gives the target lakes of its mirror
connections the host type `sys.mirror`, the state of each connection and
source type (`docs/decisions/042-water-crosses-only-as-a-mirror.md`).

`types()` carries type declarations only. A host whose vocabulary also
needs value types, verbs, triggers or functions applies them itself,
after open, on the writer, through `declare` or `apply`. A verb names
no `sys.` type but a relation's link type and, when its effect is
`external`, a type of the host's vocabulary (003, extended): the host
hands such a type's effects to its executor through a verb, and writes
the type through `upsert`, under its grant; a `write` verb on it is
refused as `reserved`. Neither is
idempotent: declaring again is a new version and has meaning (a
recorded failure is retried once; 022), so the
host first compares what it would apply with what the lake holds
(`describe` answers each declaration whole, and each type's version
and schema hash) and applies only what differs. The hash does not cover
everything a declaration says (a field's `initial`, its description), so
the host compares the declaration itself. The vocabulary's
types are declared by the open; everything else is the host's apply.

The vocabulary is consulted every time a lake is opened for writing:
the store declares every host type that `may_hold` allows for that lake
and the lake does not hold yet, as system declarations in one append
(at creation, together with `sys.setting`). A type the host adds later
reaches an existing lake the next time a writer opens it.

A type the lake holds is declared again where the host's form only adds
optional fields to it, anywhere among its fields, or rewords it (its
descriptions, labels, annotations, the order of its fields), in the
same append: neither reshapes a record, so the records written before
read as they did. Annotations are judged as a door judges them. A
reader's open of a lake that holds a type without fields its
vocabulary adds is refused as `unavailable`, `vocabulary_upgrade_owed`,
naming the lake and the type, until a writer has opened it; one
reworded only is read as held. A vocabulary that changes a held type
in any other way, alone or beside a field added (a field removed,
retyped or added as required, any other member of a field, the key, a
relation, a constraint, the type's other members), is refused at every
open as `invalid`, `vocabulary_changed`, naming the type and each
difference; nothing is appended
(`docs/decisions/003-reservation-by-namespace.md`).

Two kinds of name are not a host's to take, and a vocabulary that names
one is refused when a writer opens the lake. The name is reserved
(`docs/decisions/003-reservation-by-namespace.md`), and the refusal is
`invalid` with a message naming the type and saying it is reserved,
since the `reserved` refusal carries no message to name it in. The open
appends nothing, and the process does not write until its vocabulary
names the type otherwise. **`sys.link.*`** is where the
store puts the link types it derives from relations, one per relation
carried by link records (`docs/decisions/035-a-link-is-a-record.md`); a
host type there could collide with one of them. **The four reserved
names** belong to the store's own readers, including `sys.source` and
`sys.provenance`, whose readers are not written yet: they have no
declared shape, and the shape comes with the reader, not with a host
type that took the name first.

**A host type whose tools would bear another's names** is refused at
the writer's open too: the types an open is about to declare are judged
beside the lake and beside each other as a declaration is, and where the
lake with them would have a problem among its tools' names (a host's
`sys.audit` beside a declared type `sys_audit`, or beside a verb named
as one of its tools would be) the open is refused as `invalid`,
`tool_names`, naming both sides, and appends nothing
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`). The
same lake opens for writing with a vocabulary that leaves the type out,
or whose `may_hold` withholds it from that lake, until the lake's own
type is out of the way. A type the lake already holds is not judged
again.

A `sys.setting` record is one named setting of a lake. Its record key is
the setting's key, and it has two fields: `key` (text, the same key) and
`value` (json, the setting's value). `setting('vat_rate')` in any
expression reads the `value` of the `sys.setting` record with key
`vat_rate` as of the position being read or folded, so a replay reads the
setting as it then stood. A setting is the lake's configuration: every
live setting, restricted or not, is what the lake computes with and what
an expression reads, in the fold, in a read, by a writer opened again
and in a rebuild. Restricting a setting hides its record, as a grant on
`sys.setting` does, and never its value from an expression; only
retiring it takes it from the lake's computations.

One setting is read by the store itself: **`sort_index.<type>`** holds a
JSON array of field names of `<type>` and asks every engine to keep an
order index on them, so that a `find` ordered first by one of them reads
its page, not the type (§`find`'s ceiling above;
`docs/decisions/041-a-sorted-page-reads-its-page.md`). A write of it is
refused as invalid, naming the field, when it names a field the type
does not declare or declares with a kind other than text, number,
decimal, boolean, date-time, date or reference, or when `<type>` is no
declared stored type or view (the field named is then `key`), or its
value is not an array of names (`value`). The fold that puts it rebuilds
that type's index in the same transaction; retiring it, or an empty
array, drops the index. **Of a view** it asks for the index of the
view's kept records, on fields the view declares: an engine reads a
kept name's (`sys:kept.<view>`) order fields from the view's
declaration at the head and from this setting
(`lakelands_core::engines::sorted_fields`), keeps its rows as the fold
puts each kept record, and rebuilds them when the setting is written
(lakelands#668). A view that is computed has no records to index. It
changes no answer, only the price of an ordered page.

Another is **`lookup_index.<type>`**: a JSON array of text fields of
`<type>` whose exact texts a `where` or a derived relation's match finds
by the engine's exact-text index, besides those the declarations look up
by: the type's key and references, the fields a derived relation matches
at either end, and those a set's condition or a view's `where` over it
compares to a text at the top level (`lakelands_core::engines::looked_up`;
`docs/decisions/057-an-exact-text-index-is-kept-where-it-is-looked-up.md`).
A text of any other field is found by a scan of the type, narrowed in
storage. A write of it is refused as `sort_index.<type>`'s is, naming a
field that is no text (`lookup_index_kind`) or computed at the read. The
fold that puts it, or a declaration that changes the fields looked up,
fills or drops that type's rows in the same transaction. It changes no
answer, only the price of a `where` on that field.

Another is **`view_kept.<type>`**: it holds `true` and asks for the
records of the view `<type>` to be kept as its sources are written, so
that a read of the view reads its records where it would compute them.
A write of it is refused as invalid when `<type>` is no declared view
(the member named is `key`), when its value is neither `true` nor
`false` (`value`), and when its value is `true` and the view cannot be
kept (`key`), saying which measure, which call and why. A view can be
kept where it declares no parameter and reads no clock, and every
aggregate of its measures and of its merged sources' is a function of
its values, whatever order they are folded in: any `count`; a `sum` or
an `avg` of what is certainly a number; a `min` or a `max` of what is
certainly a number or an instant, under group fields that are all text
or references, of which there is one at least. A fold skips nulls, so
an `if` with `null` for one branch stands for its other branch
(`lakelands_api::expr::keepable`). `false` and a retired setting ask for
nothing. A view declared again so that it can no longer be kept is not
refused under a standing setting: the setting stays, and `describe_type`
says why the view computes. It changes no answer, only where a read of
the view gets it. While an object set holds the view, or a view that
reads it, or a trigger watches one, the setting written `false` or
retired and a declaration under which the view cannot be kept are
refused `view_kept_for_set`, naming the set or the trigger
(`docs/decisions/066-a-set-holds-a-kept-view.md`).

Another is **`search_weights.<type>`**: it holds an object of `fields`,
vector fields of `<type>` with their weights, `text_fields`, searchable
fields of `<type>` with theirs, and `text_weight`, each optional and one
at least. `fields` and `text_weight` weigh each ranking a fused search
of `<type>` gives no weight; `text_fields` weighs a match of the text by
the field it is in, in every ranking of text: a lexical search's, a
hybrid search's lexical half before the fusion, and its fallback to the
text (§`ranked`, lakelands#1230). A search's own `text_fields` wins
over the setting's for each field it names (lakelands#1237). Each is
read as the read finds it at its position. A write of it is refused as
invalid when `<type>` is no declared stored type (`key`), when it names
a field the type does not declare, in `fields` one it declares with a
kind other than vector, in `text_fields` one it does not declare
searchable, or a weight a search could not give (above 0, at most
1 000 000; the member named), and when its value is no such object or
names nothing (`value`). A weight it holds that a search could not give,
as a log the store did not write may hold, weighs nothing. A field a
migration removed stays named and weighs nothing; declared again, it
takes its weight again. A rename is not followed: the field is weighed
by its old name at a position before it, and at the head by its new one,
which the setting does not name until a host writes it again. The judge
of all four settings is the contract's
(`lakelands_api::seam::check_setting`). It changes no answer's members,
only the order a search ranks them in.

**A setting the store reads is judged by what its writer is shown,**
after the writer's grant on `sys.setting`
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended). The judge is handed `<type>` as `describe_type` answers it to
the writer. A type the writer is not shown is refused as a name that is
not declared, under every value, `null` and `false` too. A setting that
names the type's fields, an index setting or `search_weights`, is
written, retired or proposed only by a writer that reads the type whole;
any other is refused it as not granted, naming no field, before anything
of it is judged (`docs/decisions/063-every-setting-is-in-one-list.md`).
A view that cannot be kept is refused with its reason to a writer
shown the view as declared, and with no more than that it cannot be to
one shown the view without its definition. The judge reads the value as
its writer writes it. `propose` is judged as `upsert` is, as its
proposer.

`describe_type` says in a view's lifecycle (`kept`), and in no other
type's, whether its records are kept, or computed at each read and why:
`not_asked`; `cannot_be_kept`, with the sentence the setting's refusal
says, to a principal who is shown the view as declared and to no other,
since it names what the view reads; `not_whole`, with the number of
groups whose kept record is not exact for the reader (a measure that
fails over one, which mends when its cause leaves, or an extreme given
up, which only what makes the view again mends); or `not_kept_here`,
which is also
what a lake that keeps no view answers for a view that is
asked for and can be kept. Like `sorted` and `indexed`, `kept` describes
the projection and not the log; a rebuild leaves the state the live
appends left, since the fold decides everything of a kept view in the
fold of one event, but for the make after a declaration that computes a
source's records again, which stands where those records are put and
may come at a later position in a rebuild: as of a position between,
`kept` may then say `not_kept_here` where the live lake said `kept`,
and every read of the view's records answers alike (lakelands#631).

The store keeps such a view in its fold
(`docs/decisions/049-a-view-the-fold-keeps.md`): the view's records are
put in the projection under a name of the store's own,
`sys:kept.<view>`, and its state under the seam name `sys:kept`, neither
of which a type or a seam of a host can bear, and a read of the view
answers them where the state at its position says they are the
computation's own. There `describe_type` answers `kept`, `not_whole`
with its number for the reader's face, or `not_kept_here` as of a
position before the view was made and to a principal the view's read
would refuse, who is told nothing of its groups.

A `sys.aging_rule` record is one **aging rule**: on a date, move a field
of every record of a type from one value to another when a condition
holds (`docs/decisions/036-aging-runs-by-date-once.md`). The store declares
the type when it creates a lake, beside `sys.setting`, and in a lake
created before it the next time a writer opens that lake. Its key is
`name`, and its fields are:

| Field | Kind | Holds |
|---|---|---|
| `name` | text, required | the rule's name, an identifier |
| `on` | text, required | the type it ages |
| `field` | text, required | the field it moves; `authored`, `shared` or `verb`, never derived |
| `from` | json, required | the value that ages |
| `to` | json, required | the value it becomes |
| `when` | text, required | a condition in the one grammar, over the record's fields |
| `description` | text | for people |

A rule is written like any record, through `upsert` under the writer's
grants, and checked when it is written: `on` names a type that is
neither a `sys.` type nor a view, `field` one of its fields of the
authority above and not part of its key, `from` and `to` are values of
that field's type and differ, and `when` parses and uses none of `changed`,
`previous`, `retired` and `created`. A rule that fails a check is refused as invalid, naming the
rule field at fault. A rule's authority is checked when it is written,
and the run carries it: its author must hold write on `field` of `on`
and read on every field `when` reads
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`), or
the rule is refused as not granted, naming that field; `age` then moves
the records as the system. `setting()` in `when` reads the lake's
settings as a filter does; no grant on `sys.setting` is asked, and if
settings ever come under a grant, `find` and `age` change together.
A bundle declares rules in its `aging_rules/` lane,
one per file, with the same keys; `lakelands apply` writes each after
the bundle's declarations, skipping a rule whose stored fields already
equal the file's and clearing a field the file no longer carries. A
rule's `on` may be a type the lake already holds, so a bundle of rules
alone retunes them.

`LakeStore::age(today)` runs every live rule once for `today`, a date
(a rule record under a retention hold is live and runs: a hold withholds
a record from reads, it does not stop it),
in rule-name order, and answers what it did: the date, the rules run,
the transitions written, the guards that failed, and the position after
the last append (the head as it stood when nothing was appended).
`today` is a `Timestamp` at UTC midnight; any other time is refused as
invalid, naming `today`. For each rule it reads the live records of
`on` at the head, restricted ones included (aging is the host's
maintenance, not a principal's read), and moves every record whose
`field` equals `from` (as values, by their canonical JSON) and for which
`when` holds, with `today()` and `now()` both meaning `today` at
midnight. `setting(...)` in `when` reads the setting as of the head at
the time of the run; the move is an event, so a replay reads the move
and never evaluates `when` again.

- A rule's transitions are one append of `record_upserted` events, each
  setting `field` to `to`, with `origin: system`, the writer
  `system:lakelands`, `procedure` `{ "name": "aging:<rule>", "version":
  <the rule record's version> }`, `context.origin` `scheduled` and no
  `cause`. Triggers and object sets react to them in the same append,
  as to any write.
- Each transition has the id of `docs/log-format.md` §3 for the rule,
  the record and the date, so a second `age` for the same date appends
  nothing. The id names the record's key, so after a re-key the same date
  can move a record once more, when after its own transition the rule's
  `from` and `when` hold for it again.
- A `when` that cannot be evaluated for a record, or a move the store
  would refuse for it, does not move it; it is counted as a failed
  guard, and the run goes on.
- A rule the lake's shape no longer fits (its field dropped by a
  migration or no longer `authored` or `shared`, `from` or `to` no
  longer a value of the field's type) is skipped, never run half: the
  report names it with its reason, and the other rules run.
- `age` holds the writer's lock for the whole run and scans every live
  record of each rule's `on` at the head, so writes wait while a large
  type ages.

The suite's `aging` checks hold these sentences on every engine, with
two held elsewhere: that a lake which lacks `sys.aging_rule` gains
it at its next writer's open is a store test
(`lakelands-store/tests/older_lake.rs`), since the contract cannot make
such a lake; and that writes wait for a rule's run is one of the
`concurrency` checks. What `lakelands apply` and `lakelands age` print
is held by the CLI's tests.

**The ticker** is `lakelands serve`'s caller of `age`, configured by
the `aging` section of the root's `lakelands.json`:

```json
"aging": { "utc_offset": "+01:00", "check_secs": 60 }
```

- **The date** is the host's clock at `utc_offset` (`Z`, or `+HH:MM` /
  `-HH:MM` within fourteen hours; a fixed offset, not a zone), handed to
  `age` at UTC midnight. The lake never reads a clock (002).
- **When:** once when `serve` starts, and again whenever that date rolls
  over, looked at every `check_secs` (60 by default). A second run on
  one date writes nothing new.
- **No catch-up:** a date the process was down for is not aged
  afterwards. A rule is a condition on today, so the next date's run
  moves every record that should have moved.
- **A clock set back** a day makes the ticker age the earlier date
  again; by 036 a date aged twice is written once, so that run moves
  nothing new.
- **Failure:** a run that fails is said in one line on standard error
  and tried again at the next check, never sooner.
- **Where:** only in a process that holds the lake as its writer, one
  ticker per lake.

Without an `aging` section `serve` ages nothing: a host, or
`lakelands age --today <YYYY-MM-DD>`, calls `age`; the command prints one
line per rule (`<rule>: moved N, M guard(s) failed`, then `, K refused` where a
constraint or the set ceiling refused K moves, or `<rule>:
skipped, <why>`) and `aged <date> through position P`. No MCP tool calls `age`:
like `rebuild` and `checkpoint`, it is the host's.

## `KeyStore`

A source's values of a field declared `erasable` are kept in the log
sealed under that source's data key, so destroying the key forgets them
without touching a byte of the log
(`docs/decisions/058-a-source-may-be-forgotten.md`). The keys are the
host's, never in the lake: not in its log, its projection or its export.

| Method | Contract |
|---|---|
| `create(lake, source)` | Make a new version of the source's key and answer its id. A source whose keys were destroyed is never given one again (`key_destroyed`). |
| `key(lake, source, id)` | The key of that version, or of a key `create_apart` made: 32 bytes, or none once destroyed. |
| `destroy(lake, source)` | Destroy every version of the source's key, and every key `create_apart` made of it. |
| `destroy_version(lake, source, id)` | Destroy that version and every older one, never one alone; a version it does not hold is refused (`key_version_not_held`). The default refuses as `Unbound` (`key_versions`), for a store that destroys only whole sources. |
| `ids(lake, source)` | The versions the store holds, oldest first; none once destroyed. |
| `create_apart(lake, source)` | Make a key of the source outside its versions, for one delivery sealed apart, and answer its id: `key` answers it, `ids` never lists it, and `destroy` destroys it with the versions. The default refuses as `Unbound` (`key_apart`), and an ingest that asks `sealed_apart` is then refused. |
| `destroy_apart(lake, source, id)` | Destroy that key alone; one it does not hold is no change. The default refuses as `Unbound` (`key_apart`). |

The calls are synchronous: the writer makes them on the store's blocking
threads, at a source's first sealed value of an open lake, and at a read
that computes a call through a kept step. The lake keeps each key in
memory while it is open, wiped when dropped, and seals a source's new
values with the newest version. Under a key period (`key_period`, by
the lake or by the source: `day`, `week` or `month`, in UTC) a source's
first value past the start of a later period than its newest version's
first use asks the store for a new version, so each period's values are
sealed apart and a cut at a period's end can destroy the versions
before it. A version is destroyed only with every older one: an output
is sealed under the oldest version its call reads, so it goes with the
first of them to go, and never outlives a value it was computed from. A
key the store does not give where the log holds a value it sealed fails
the fold closed, `unavailable` (`key_missing`), naming the source and
the version: a lost key is never read as an erasure. Nor is a new key
made for a source whose sealed values the log holds while the store
lacks the version it last sealed with: its next write is refused
`key_missing`, so a wrong path, a fresh machine or an old backup fails
at once, unless an act of removal in the log destroyed that version
(`docs/decisions/065-part-of-a-source-may-be-taken-back.md`), when the
source is given a new one. `KeyStore::destroys_versions` says whether
`destroy_version` destroys; where it does not, as by default, such an act
destroys no version and redacts what it takes. `destroy` accepts a
source whose versions are all destroyed, or that has none, and marks it
so. A version's id is the store's and is never given twice:
`FileKeyring` makes it at random. Without a key store, a value that
would be sealed is refused as `Unbound`, and a lake whose log holds one
is folded only by a writer that holds the key.

`lakelands_backings::FileKeyring::new(dir)` keeps one file per lake,
`<lake>.keys.json`, in a directory the operator keeps outside every lake
directory, readable by its owner alone and written whole. Its backups are
the operator's to keep honest: a key restored from a backup restores what
it sealed. The CLI and `serve` bind it where the root's `lakelands.json`
names its directory, `"keys": {"keyring": "<dir>"}`, relative to the
root, and refuse one inside a lake's directory.

An import (`lakelands_store::import`) opens a sealed value with the key
store bound. With none bound, it reads the export first: a source whose
values are sealed and whose `source_forgotten` the export holds arrives
erased, and any other refuses the import as `Unbound` before anything
is written.

## `Meter`

A lake tells its meter what it does, and keeps no number of its own
(`docs/decisions/079-a-lake-is-measured-through-a-meter-the-host-binds.md`).
Each call is handed a `Metered`, the lake's id and its engine's name, and
one number of a closed set, each `#[non_exhaustive]`, its `ALL` listing
them and its `name()` the name a host exports it under.

| Method | Contract |
|---|---|
| `count(at, Count, by)` | A number that only goes up went up by `by`: events appended, appends, appends that failed, writes, seals. |
| `level(at, Level, value)` | A number stands at `value`: the queue's depth, the tip, the position folded through, a rebuild under way, the active segment's bytes, the lease held and its fence, both 0 whenever the lease goes. |
| `observe(at, Spread, value)` | One value of a spread: an append's, a fold's, a commit's or a read's time in microseconds, or a batch's writes. |

The calls are synchronous and are made inside the writer's turn, so a
meter keeps its numbers in atomics or hands them off; it is never asked
anything back. Each call is made under `catch_unwind`: a meter that
panics loses that number and nothing else, and an append's counts are
told only after its fold has committed. No number names a type, a
record, a principal or a value. With no meter bound, nothing is told and
no clock is read for one.

`lakelands_server::Metrics` is the meter `serve` binds to every lake it
opens where the root's `lakelands.json` sets `server.metrics.bind`, and
answers `GET /metrics` there in the Prometheus text format
(`docs/hosting.md`).

## The facade and the doors

Two more traits sit above the seams. They are what callers and hosts
hold, not what hosts implement:

- `Catalog`: the doors of one lake, for any caller, every call naming its
  principal. A principal names its lake too; a call whose principal
  names another lake than the catalog's is refused as `not_granted`, on
  every door (door check: `a_principal_of_another_lake_is_refused`). Tool names for LLM-facing transports come only from
  `lakelands_api::tools::tools`.
- `LakeStore`: one open lake, as its host holds it; it hands out a
  `Catalog` bound to a `WriteContext`, the change feed, the limits it was
  opened with (`limits`), the position the log had at an instant
  (`position_at`, below), how much of each embedding field is embedded
  (`vector_coverage`, under `VectorIndex`), lake maintenance (`verify`, `rebuild`,
  `checkpoint`, `compact`; the export, below, is a function over it), restriction (`restrict`, `unrestrict`), `age` (above,
  under `SystemVocabulary`), and `purge_source` (below).

`describe` and `describe_type` answer the ontology in force at the read
position, as the principal may see it, each declaration whole (suite:
`describe_answers_the_declaration_whole`; door check of the same name):

- A field the principal reads carries every member of its declaration —
  `type`, `required`, `authority`, `derived` or `function`, `computed_at`,
  `depends_on`, `on_input_change`, `searchable`, `sensitive`, `label`, `description`,
  `embed` — beside its `access` and whether it is `filterable`. A type
  carries its `description`, `label`, `language` and `view`, and on its
  access axis `key`, `valid_from`, `valid_to` and `field_meta`. For a
  principal who reads every field, a type's description projected back
  is its declaration byte for byte, so a host redeclares a type from
  `describe` without replaying the feed.
- A masked field shows its shape only: `name`, `type`, `access`,
  `sensitive`, `required`, `authority` and `on_input_change`, neither
  `filterable` nor `searchable`, and no `derived`, `function`,
  `computed_at`, `depends_on`, `embed`, `label` or `description`
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
  Until 0.4.0 a masked field also showed `derived`, `depends_on`,
  `description` and `embed`.
- A type's `may.find` and `may.get` are true of every type described:
  one who is shown a type is answered its records, each with its key,
  its version and the fields it reads or sees masked
  (`docs/decisions/006-grants-not-roles.md`, extended; door check
  `a_type_its_principal_is_shown_is_one_it_may_find_and_get`). A filter
  or a search is still held to the fields it probes.
- The lake lists its value types and verbs, and its `functions`,
  `triggers` and `object_sets` as declared; `triggers` were names until
  0.4.0. A function, a trigger, an object set and a type's `view` are
  answered to a principal who holds the `schema` grant or reads every
  field the declaration names — a trigger's guard and an object set's
  condition on their type; a view's `where`, `group_by` and measures on
  its source; a function's inputs, and the field it computes, on every
  type that uses it — and are absent otherwise. A function no field uses
  is answered to the `schema` grant only. A field's `function` names its
  function either way, and a view type names `view_of` either way.

A host writes a reaction through a `Catalog` bound to a reactive
`WriteContext` one deeper than the write it reacts to, and the log keeps
each write's depth. The store refuses a write deeper than the lake's
`max_reactive_depth`, a limit of its `Location`, as `too_deep` naming
that ceiling, on every door that writes (ingest, schema writes and
proposal decisions included), and appends nothing (suite:
`a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing`,
`the_reactive_ceiling_holds_on_every_door_that_writes`).

The context may also name `cause`, the position the write follows from.
The store takes a cause at or below the head, and refuses one past it as
`invalid` naming both positions. It puts the cause in the envelope of
each event the write itself appends; events it derives from those keep
their own cause, which chains back
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`, extended;
suite: `a_reactive_write_with_a_cause_carries_it_on_each_of_its_events`,
`a_cause_past_the_head_is_refused_and_appends_nothing`). A cause that names
a firing of a trigger with `max_depth` binds the write by that trigger's
ceiling too: deeper than it, the write is refused `too_deep` naming the
trigger (suite: `a_reaction_past_its_triggers_max_depth_is_refused_naming_it`).

`LakeStore::head` answers the last position the projection has folded:
a position a read can be answered at at once, on every engine. An
append is durable before it is folded; the store publishes its head
only once the fold has committed. When a fold fails after its events are
in the log, a fold whose transaction cannot begin included, the append
is answered `unavailable` in the store's own sentence, with the fold's
after it: what was appended is in the log from position N and is not
folded yet, the next write folds it, and a write is sent again only
under its idempotency key. The head stays at the last folded position,
and the writer is behind: before the next write is decided it reads its
ontology and settings at that position and folds what the log holds past
the projection, so the write is decided over every event in the log (a
retry under its key is answered the first write's outcome, and a
record's next version follows the one not folded before), or it is
answered `unavailable` (the projection is behind its log and could not
catch up; nothing was appended), whatever kind that fold answered. The
drain of due calls reads behind the same catch-up, so an output whose
recording was appended and not folded is found and its call is not made
again; and a drain that fails is not its write's failure: an upsert, a
feed's window, an aging run and a purge are each answered their own
outcome, the computed fields waiting. A fold's failure is not lost:
where a recording's fold failed the writer is behind, and the next write
catches up first or is answered `unavailable` in that fold's words. A
drain's failure that is no fold's (the recording could not be appended,
or what the drain decides on could not be read) leaves the writer as it
was and is answered to no one: what it leaves to see is the field that
waits, cleared and without `error`, and for an embedding
`vector_coverage`'s `due`. Either way what was not recorded stays owed on
the handle, with the answer its call was given: each later write that
drains makes a batch of it after its own, and stops where it fails
again. One write is
answered a drain's failure: the call of an external verb and its
acceptance, whose executor is handed nothing after it (§`VerbExecutor`). Only the lease's holder folds: a
handle that closed while it was behind folds nothing, clears nothing by
`rebuild`, and is refused as any handle without the lease. A handle
opened only to read names the position the writer has folded.

`rebuild`, `verify` and the fold at open read the log in pages of 1 024
events and hold no more than one. A rebuild and an open's catch-up fold
page by page, each page in its own transaction that commits
`folded_through` at its last event, or in several, each committing at
the last event it folded (a page that computes derived fields again:
048). A fold that stops, by an error or a
crash, leaves the projection folded through its last commit:
`head()` names that position, a read at or before it answers the lake as
of there, a read past it is refused as `OutOfRange` naming it, as for a
failed append, and never answers from half a transaction: what a commit
inside a page holds is every event through its position, whole, and a
commit the engine refuses leaves the one before it. The next fold (an
append's, a `rebuild`, or the next open) continues from there. While a
rebuild runs, the writer's lock holds every write back and `head()`
still names the head from before it: a read at that head is out of range
until the rebuild has folded through it, and a read as of a position it
has folded answers. A handle
opened only to read in another process meanwhile reads at the
`folded_through` it finds, which is its head.

`LakeStore::position_at(known_at)` answers the position the log had at
an instant: the last position, no later than the head, whose event's
`known_at` is at or before it, or none for an instant before the lake's
first event, and a read `as_of` the answer sees the lake as it stood
then (suite: `position_at_answers_the_last_position_at_or_before_an_instant`).
It is a binary search over positions, reading one event per probe, about
log₂ N reads; it relies on `known_at` never decreasing along the log,
which the verifiers check (`docs/log-format.md` §1.3). It is no door's
yet, only the host's: `lakelands-client` answers it `unbound`, naming it
(client test: `every_host_operation_is_unbound_naming_it`).

`LakeStore::forget_source(source, report_id)` forgets a source whose
values of erasable fields the log holds sealed
(`docs/decisions/058-a-source-may-be-forgotten.md`): under the writer's
lock it appends `source_forgotten` with the retirements of the records
the source made and alone brought, has the key store destroy the
source's keys, rebuilds the projection and reclaims the storage. `forget_preflight`
answers the same report, appending nothing, with the time the act is
expected to hold the lock: an estimate, `events` at the rate the
handle's last rebuild measured (its open's included), or at the
engine's `rebuild_rate` before any, and the time of the last reclaim
the handle ran, which a handle that has not reclaimed does not count,
and the time of the preflight's own walk of the log, which the act walks
again.
The act is refused while the log holds a value of the source in an
erasable field in plaintext, and a forgotten source's ingest is
refused. It is no door's; `lakelands-client` answers it `unbound`. The
CLI's `lakelands forget --source <id> [--preflight | --report <id>]` runs
it. A mirror that meets `source_forgotten` in its source's feed stops each
mapped type whose source type has an erasable field, past the act. A
reclaim that fails is not a failure of the act: the report answers it in
`reclaim_owed`, the cause alone, as `LakeStore::reclaim_owed` does, and
the writer's next open, a repeat or a `compact` that runs clears it
(lakelands#1179).

`LakeStore::forget_sources(sources, report_id)` and
`forget_sources_preflight(sources)` do the same for a set of sources in
one act (lakelands#1183): one `source_forgotten` for each in one append,
one rebuild and one reclaim under one hold of the lock, one report id and
one expected time, each source's report beside them. A set that is
empty, names a source twice or names one the log holds no event of is
refused whole, appending nothing. A set of one answers as
`forget_preflight` does, its id included. They default to `unbound` for
a facade that does not carry them, as `lakelands-client` does.

`LakeStore::remove_values(source, selection, require_shredded,
report_id)` takes back what a source wrote before a time, in one
delivery or a set of them, or of what it observed before a time
(`docs/decisions/065-part-of-a-source-may-be-taken-back.md`), in the
same steps and with the same answer of an owed reclaim;
`remove_values_preflight` answers its report, appending nothing. It is
no door's either. The CLI's `lakelands values remove --source <id>
(--before <instant> | --delivery <id>… | --seen-before <instant>)
[--require-shredded]
[--preflight | --report <id>]` runs it. A mirror that meets
`values_removed` stops each mapped type with a field its reader reads
that a removal may take, past the act.

`LakeStore::purge_source(source, None, None)` retires every live record
one source brought in, across every type it reached, restricted records and the
source's link records included, in one append
(`docs/decisions/039-a-source-is-retracted-as-one.md`). It answers the
records retired per type, the types it did not search, and the head
after its append (the head as it stood when nothing was appended):

- A record belongs to the purge when a field holding a value came from
  the source by its provenance; one whose values from the source were
  all cleared stays. It is co-owned when a field holding a value came from
  a principal's authored write or another source; a derived value, a
  value the lake wrote itself and a cleared field make no owner. One
  co-owned record refuses the whole purge as invalid, naming the count
  and the first ten, and nothing is appended.
- Each retirement is a `record_retired` written by `system:lakelands`
  with `origin: system` and `context.origin` `direct`, and reacts as a
  `retire` does, in the same append. Links another source brought in,
  pointing at a purged record, stay; a walk skips the retired end.
- A type declared without per-field provenance is not searched, and the
  report names it.
- A second purge of the same source retires nothing and appends nothing.
- How it is built, not a promise a test holds: the append holds one
  `record_retired` per record and its reactions in memory before it
  lands, under the writer's lock, so a purge of a very large source is a
  large append and a long hold of the writer. A host that cannot afford
  it retires by type through the doors instead.

`purge_source` and `purge_preflight` take one optional type. Without it
the purge is the one above. With it, the purge is of what the source
brought into that type, and everything it says is said of that selection
alone (039, extended):

- It reads the named type and the link types of the relations that type
  declares, and no other. Co-ownership is judged there alone: a co-owned
  record in another type of the source does not hold the purge, one in
  the named type refuses it whole.
- The report names its type. What it lists as referring into the purge
  now includes the source's own records in its other types, and a link
  of another type's relation that ends at a record it took: they stay.
- A type it cannot take from is refused, where the purge of the source
  whole passes over it: not found for a name that is no declared type,
  invalid, saying why, for a view and for a type declared without
  per-field provenance. A typed report names no type as not searched.
- A preflight's id is of the source, the type and the head. The id of
  the source whole does not fit a purge from one type, nor the reverse,
  and an id that does not fit is refused as invalid in one sentence,
  whether the lake moved or the id is another purge's.

`lakelands purge --source <id>` prints one line per type
(`<type>: retired N`), one per type not searched, and
`purged <id> through position P`; when the source brought in nothing
still live, it prints `nothing to retire for <id>` and appends nothing.
With `--type <type>` the closing lines name both:
`purged <id> from <type> through position P`,
`nothing to retire for <id> in <type>`, and the preflight's
`<id> in <type>: preflight at position P; purge with --report <report>`.
The suite's `purge` checks hold the sentences about what a purge retires
and refuses on every engine; the lines `lakelands purge` prints are held
by the CLI's tests.

`LakeStore::compact()` gives back the storage the projection no longer
uses, when the host chooses
(`docs/decisions/040-maintenance-never-changes-an-answer.md`). It runs
on the writer's handle alone; a handle opened read-only refuses it as
`Unavailable`, as it refuses `checkpoint`. Unlike `checkpoint`, it
appends nothing and does not seal:

- **No answer changes.** Every read at the head and as of every earlier
  position, the feed, `verify`'s answer and an export are as they were.
  Kept history, applied ids and recorded outputs are never pruned.
- **Writes wait.** It holds the writer's lock throughout, like `age`: a
  write that arrives meanwhile waits, then lands after it. On SQLite a
  large projection takes seconds.
- **SQLite:** the journal is truncated, `projection.db` is rewritten
  without its free pages (`VACUUM`), and the journal is truncated again;
  the log's files are not touched. `VACUUM` is one transaction: a reader
  in another process holding a snapshot past the busy timeout makes the
  compact `Unavailable`, saying so, and the projection is as it was. A
  snapshot taken after the rewrite committed keeps only the last truncate
  from running: the compact succeeds, and `after` counts the journal it
  left.
- **PostgreSQL:** `VACUUM (ANALYZE)` of the projection's tables, outside
  a transaction, never `FULL`, so readers go on reading. It cannot be
  rolled back, but it never changes a row a read sees. The log's tables
  are not touched.
- **Memory:** nothing to reclaim.
- **The report** names the head, unmoved, and the projection's size in
  bytes before and after. The sizes are the engine's own measure, not a
  promise: the file and its journal on SQLite, the tables' total size on
  PostgreSQL, none on memory. Only each engine's tests hold anything
  about them.

`lakelands compact` prints `compacted <lake>: projection <before> →
<after> bytes at position P`, or `nothing to reclaim in <lake>: its
engine keeps no storage to give back`. `serve` does not compact on its
own. The suite's `compact` checks hold the answers, the log and the
export on every engine, writers writing meanwhile included.

`lakelands_core::export::write(store, dir)` writes a lake's log as the
segment files of the log format (§4.2) and a manifest, `export.json`,
into an empty or absent directory
(`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
It is a function over any `LakeStore`, not a method of it: it reads the
head and then the change feed through that head, splitting the files at
each `checkpoint`, so it works on every engine and over a client, and
costs the seams nothing. An export is the log as the host reads it:
every event, retired and restricted records' history and every
principal's writes included, unmasked. `export::verify(dir)` checks the
files alone and answers the last position and hash, the pair
`LakeStore::verify` answers for the lake.
`lakelands export --to <dir>` prints one line per file (`<file>:
positions a..b`) and `exported <lake> through position P to <dir>`,
where P is the head the export read: a write landing while it runs is
not in it. A directory that holds anything is refused as a failed
precondition naming it, and nothing is written. The suite's `export`
checks hold the files, the verifier and the refusal on every engine;
the client's tests hold that an export over the REST door is the
served store's own, byte for byte, and the PostgreSQL engine's tests
that `lakelands_engine_pg::export` writes the same bytes.

`Catalog::apply` declares a set of declarations as one: validated
together, so a reference may point forward within the set, and appended
in one append or refused whole, one refusal naming every problem
(`docs/decisions/028-authoring-is-strict-and-whole.md`). It is how a
bundle reaches a lake; it has no tool in `0.1.0`.

A value type changes by `declare` only when no stored value can stop
fitting it: `one_of` gains values or is removed, a `constraint` or a
`format` is removed. A tightening (`one_of` narrowed, a `constraint` or
`format` added or changed) goes through `Catalog::migrate_value_type`,
whose preflight checks every value stored under the value type, over
every field of every type that uses it, retired and restricted records
included, as a write would check it. It counts the records scanned and
the misfits per field, names the first, and appends nothing.
`migrate_value_type_apply` puts the new declaration in force against
that report, and is refused while any value misfits or if the lake moved
since. What it appends is one `declared` at the next version: no record
is rewritten. A change of `kind` or `scale` rewrites stored values, so it
is not a value-type migration; the fields that use the value type are
migrated instead. The `migrate` tool and REST route take `value_type`
instead of `to`, one of the two, never both. A principal without the
`schema` grant proposes a tightening instead (`propose` with the write
`migrate_value_type`): it is preflighted at the proposal and again at the
acceptance, and a reviewer holding `schema` accepts it
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended). The preflight checks stored
values only: a condition that compares with a value the type no longer
admits (a trigger's `when`, an object set's `where`) is not checked, and
simply never holds afterwards; an aging rule whose `from` or `to` the
type no longer admits is skipped by `age` and named in its report.

A value type's migration may rename values (`ValueTypeMigration`, its
`to` and its `renames`, pairs of an old and a new text value; the
`migrate` tool and route take `renames` beside `value_type`). Its
preflight renames each stored value before it checks it, counts per
field how many values each pair rewrites (`ValueTypeUse.renamed`), and
lists what may name an old value in a condition (`conditions`, and
`conditions_unshown` for one the caller is not shown); it refuses a
rename of a value a key field holds, and one a computed expression holds.
Applied, it appends one `value_type_migrated`, the system's declaration
again of each type whose `initial` it renames, and the set memberships
it changes; the fold renames the value in every record of every type
that uses the value type, retired and restricted ones included, with
version, flags and provenance as they were, and computes again what reads
it. A field computed by a function with a backing keeps its answer: an old
one is a misfit. The renames are not granted to a caller not shown every
field that uses the value type (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, extended).

A migration may re-key a type: name other key fields, or change a key
field's value. Its preflight (`MigrationReport.rekeyed`, `references`,
`sample_collisions`) counts, per type, the records whose key moves,
retired and restricted ones included, with every type keyed by a
reference to a re-keyed type (link types among them) and every reference
value that names a moved key; two records that would take one key, or a
record without a value for a new key field, make it not lossless. Applied,
it is one `migrated` event listing every move (`rekeyed`,
`docs/log-format.md` §2.3): each record keeps its
fields, version and per-field provenance at its new key, the old key is
ended as of that position (kept as a retired state at version 0 with no
field, which every scan of stored history skips), object-set memberships
move with the record, and references follow. A source that writes a
re-keyed type must send the new keys from then on: its own key mapping
(a column map's `key`, the keys its feed carries) has to follow. A write
at an old key whose key fields make another key is refused as invalid; a
write of an old key's bare string with no key field has the key field
filled from it and is a new record at version 1. A migration that takes
every key field away, names a key field that is not required or of a
kind no key is made of, or would re-key a view is refused.

A migration's `to` is the type's whole declaration, and its report says
every difference between the declaration in force and `to`
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended). A field's type that changes and a `required` that tightens
are in `fields`; a field removed, and a relation removed, with the
number of its live links, are in `dropped`; a relation renamed is in
`relations`. Every other member `to` states otherwise is in `restated`,
one entry a member, as the two declarations serialise: `field` (left out
for the type's own members; a relation's name; `relation.field` for a
link field), `member`, `was` and `to`, null where a declaration leaves
the member out. So a `to` that names a field without its `sensitive`,
`searchable`, label or annotations takes them away, and the report reads
`{"field": "secret", "member": "sensitive", "was": true, "to": false}`
before anything is applied; a field or a relation `to` adds is restated
from null in each member it states otherwise than a bare declaration,
its name among them; a `depends_on` the store completes restates nothing.
`lossless` speaks of stored values: a migration that only restates is
lossless, and `restated` is where it says what it changes. It says
nothing of what the migration removes: the apply discards the values
`dropped` counts, and a host that must know sums them
(`MigrationReport::discards()` in Rust). An entry is
answered where its reader may know of the field, as a conversion is, and
member by member as `describe` shows that field: to one who knows of it
and does not read it, in `to` or as the type stands, the members of its
shape alone (name, type, `required`, `authority`, `on_input_change`,
`sensitive`), never an expression, a label, a description, annotations,
`searchable` or an embedding, and `more` says that something was left
out. So an author with `secret` masked, whose `to` drops a label it was
never shown, is answered `sensitive` and `more`, and the report whole is
in the log and before a reviewer who reads the field. A listing hands
out a proposed migration's `to` and a proposed declaration's types the
same way
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended). A proposed migration is logged with its report, and an accept of it is
refused ("changed since this migration was proposed; propose it again")
where the type, as it then stands, would have the migration restate
anything else: a mark set while the proposal waited is not taken away
unseen.

A migration keeps the fields the lake's declarations read
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended). Its preflight refuses, as invalid, a migration that drops or
renames a field read by a view over the type, a derived field of the
type, an object set, a trigger or a verb on it, another type's derived
relation to it, or an object set or a trigger on the link type of a
relation whose fields it changes, naming the reader and the field ("after the
migration of `invoice`, the object_set declaration `open` reads `status`
of `invoice`, which the migration renames to `state`"). Declare the
reader again so that it reads the shape the migration leaves, then
migrate. For a rename, the reader is declared without the field first,
and with its new name once the migration is applied. `declare` holds a
declaration to the same reads: one that reads a field its type does not
have is refused, naming it and the field
(`docs/decisions/028-authoring-is-strict-and-whole.md`, extended).

**Many callers, one writer.** A lake has one writing process
(`docs/decisions/008-one-writer-per-lake.md`), and any number of callers
may use its doors at once; the writer orders their writes. What a caller
sees does not depend on who else is writing:

- Every write lands once. The log's positions follow one another without
  a gap, each write's answer names a position of its own from which the
  write is visible, a record's versions count every write to it, and the
  hash chain verifies.
- A declaration made while others write takes its place in the same
  order: a write after it sees its shape, a write before it does not.
- A read at a position answers the same whenever it is made, during the
  writes or after them, and `find` and `get` at one position agree. A
  position `head()` answers can be read at at once.
- Two feed consumers polling from one position while others write see the
  same events, in the same order, each once.
- A write whose field a backing fills answers a position its own filled
  field is visible from, and one input is called once across all the
  writers.

The suite holds each of these on every engine (the `concurrency` checks),
with its callers run together against one lake.

The `seam-width` gate counts the methods of `LakeStore` and the three
engine traits together, and holds them at 110 or fewer. `Catalog` is not
in that count: its width is governed by the four-doors rule
(`docs/decisions/014-four-doors-per-type-one-name-generator.md`).
