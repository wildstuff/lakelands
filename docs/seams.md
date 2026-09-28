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

Seven seams, this roster and no other. Adding one is a change to the
contract and to this page.

| Seam | Trait(s) | Default in Lakelands | A host plugs in |
|---|---|---|---|
| Engine | `LakeEngine`, `ProjectionStore`, `ProjectionView`; `VectorIndex` beside them | SQLite and in-memory (`0.1.0`); PostgreSQL (`0.2.0`) | Rarely anything: a new engine, proven by the conformance suite |
| Principal resolution | `PrincipalResolver` | API key or static entry → one principal | Its identities, roles and policies, compiled to grants |
| Function backings | `FunctionBacking` | `expr`, evaluated in the store | Its own backing kinds; `http` and `exec` ship in `lakelands-backings` |
| Embeddings | `EmbedBackend` | None: lexical search and caller vectors | An embedding client; `lakelands-backings` ships one for `/v1/embeddings` endpoints |
| Verb execution | `VerbExecutor` | None: external verbs are refused as `Unbound` | What its external verbs do |
| Change feed | `ChangeFeed` | The log, read by kind from a position | A relay of the feed onto its own bus, as its own code |
| System vocabulary | `SystemVocabulary` | Lakelands' four `sys.` types, allowed in every lake | Its own `sys.` types and which lakes may hold them |

`Location` is not a seam. It is the configuration every seam and every
engine is configured through, passed at the call site; Lakelands reads no
environment (`docs/decisions/005-no-environment-reads.md`).

The seams do not all point the same way:

- **A host implements and passes in** the engines (`Seams::engines`),
  function backings, an embedding backend, verb executors and a system
  vocabulary. Every slot but `engines` may stay empty and has a default;
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
  §1.2) and hands the engine a `CanonicalEvent`: the bytes, with the
  position, id, `prev`, `hash` and `fence` beside them so the engine can
  check an append without parsing. The engine stores the bytes and gives
  back the same `CanonicalEvent`. The store parses and verifies what it
  reads. Byte identity is checked through this seam by the conformance
  suite.
- **The projection keeps its history.** Every `put_*` adds a state valid
  from a position and keeps the state it supersedes. `view(as_of)` is a
  filter over kept states, never a replay: "what did the agent see at
  position N" is a cheap read on every engine. The conformance suite
  reads each record at every position it changed and compares with a
  fresh replay to that position.

The traits are synchronous and `dyn`-compatible: storage work blocks, and
the store runs an engine off its async callers. An engine may build on
`lakelands-core`, and is expected to where this page names one of its
functions as the definition of a behaviour. Every promise below is
checked on every engine on the roster, either by an engine check through
the seam or through the doors' suite; "Where each promise is checked"
at the end of this section says which, and names the promises that have
no check yet.

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
| `lake` | The lake this handle keeps. |
| `take_lease(holder)` | Take the writer lease with a fencing token greater than every token this lake ever handed out. **Fails at once** while the lease is held — by another process, another handle, or this one — with `Unavailable` whose message names the holder; never waits. `Unavailable` on a handle opened read-only. |
| `release_lease(lease)` | Give the lease up; the next taker gets a greater token. The lease is always released; an engine that also records the holder in storage reports a failure to clear that record as `Unavailable`, after releasing. Releasing a lease that is no longer the current one does nothing, so a writer that lost the lease cannot release its successor's. A handle that is dropped releases the lease it took and still holds, so a writer that ends without closing does not keep the lake from every later writer. |
| `tip` | The last event's position and hash, or none for an empty lake. |
| `append(lease, events)` | Append a group of `CanonicalEvent`s, all or none, **durably before returning**: once it returns, the events survive a crash of the process and of the machine. `Unavailable` when `lease` is not the current lease or the handle is read-only. `Invalid` when an event's fence differs from the lease's — except the event at position 0, which carries fence 0 — when the positions do not continue the tip without a gap, or when a `prev` is not the hash before it; nothing of a refused append is kept. An empty group appends nothing. Stores each event's bytes unchanged and never parses them. |
| `read(from, limit)` | At most `limit` `CanonicalEvent`s from `from` on, in position order, exactly as appended: the same members, the same bytes. Empty when `from` is past the tip or `limit` is 0. |
| `import(lease, events)` | Append events carried over from another lake's log, as an import does (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`): as `append`, all or none and durably, except that each event keeps the fence it was written under, 0 at position 0 and never below the fence of the event before it, instead of carrying the lease's. `Invalid` unless the log holds nothing but imported events (it is empty, or every event in it came through `import`), so a lake that was ever appended to is never imported into. Afterwards the next `take_lease` answers a token greater than every fence imported. The store checks, before it imports, that every fence is its writer's (log format §4.1). |
| `seal_segment(lease)` | Close the active segment at its last event and start a new one at the next position appended; none, and nothing changes, if the active segment is empty. Refused as `append` is for a stale lease or a read-only handle. The store then appends the `checkpoint` as the new segment's first event. |
| `segments` | Every sealed segment's range and digest (`docs/log-format.md` §4.2), oldest first; the ranges start at 0 and follow each other with no gap. |
| `projection` | The projection's write side. |
| `view(as_of)` | A read view at `folded_through` (`None`), or as the projection was right after folding `as_of`, served from kept states. `OutOfRange`, with `folded_through` (0 when nothing is folded) as the head, for a position past it. A view keeps answering its own position while the fold moves on. |
| `vectors` | The vector index, if the engine keeps one; a search that needs vectors is refused as `Unbound` on an engine that keeps none. |

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
committed, where their bytes end, its last event's hash, and every
256th event's place in the index file (`MARK_EVERY`): its memory grows
by a few bytes per 256 events, not per event, and a handle opened
read-only in another process holds the same. **The ceiling:** a read by
position costs one seek to the mark at or before it, then reading fewer
than 256 event lines, and the commit lines between them, before the
first event it answers, then one event line per event, in order, with
the commit lines between. A page of the store's sequential reads costs
what it did when the index was held, plus that seek and that skip. The log's files stay as `docs/log-format.md` §4
describes them; an index line that is short, unreadable or not the
position expected is refused as a damaged index, never read as another
event.

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
- **The log.** It is kept as rows holding each event's canonical bytes
  unchanged, and an append is one transaction that checks the fence
  against the lease row. `lakelands_engine_pg::export` writes the
  segments as the §4.2 files, which verify without the engine, for a
  lake whose store cannot be opened; they are the bytes the store's
  export writes.
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
| `open(location, mode)` | Open the lake `Read`, `Write` or `Create` (write, creating it if absent, opening it if present). `Read` or `Write` of a lake that does not exist is `Unavailable`; a location configured for another engine is `Invalid`. Opening never takes the lease; a writer takes it afterwards with `take_lease`. A handle opened `Read` refuses `take_lease`, `append` and `seal_segment` as `Unavailable`, and writes nothing to the projection (`begin`, the puts, `clear` and `compact` are `Unavailable`), so any number of readers can hold the lake while one process writes (`docs/decisions/008-one-writer-per-lake.md`). A read-only open of a lake that does not exist creates nothing. The store folds only on the writer's handle; a reader reads what the writer folded. |

### `ProjectionStore`: what the fold writes

The fold calls the projection inside one transaction per chunk: an
append for a write, a page of the log (1 024 events) for a rebuild or an
open's catch-up. `commit(through)` records the position folded through
in the same transaction, so a crash leaves the projection at a whole
chunk, and opening a lake folds the log from `folded_through` on. A
page's boundary may fall inside an append; a view inside an append
already answers each event's states. What a fold that stops leaves, and
what reads answer meanwhile, is under `LakeStore::head` below.

- The store never nests transactions and never puts outside one.
- Nothing put in a transaction is visible to any view until it commits;
  a commit makes all of it visible at once, with its `through`.
- Before the commit, the store reads a state it put in the transaction
  from what it staged, never through a view. An engine may hold the
  puts until the commit; whatever it answers about a put itself (the
  state it supersedes, where it ends, whether it is the head) is
  answered over what the transaction put and what was committed
  together, so the commit leaves what committing after each put would.
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
| `put_declaration(declaration, version, schema_hash, at)` | A declaration in force from `at`, replacing, from there on, the earlier declaration with the same `declare` and name; the earlier one is kept. |
| `put_record(record)` | A record's whole state as of `record.position`: values, per-field meta, flags. The state it supersedes is kept. The engine derives its own indexes, full-text entries and edges from it and the declarations in force at that position. |
| `put_proposal(proposal, at)` | A proposal's state from `at`, the position of its latest event; earlier states kept. A view at a position inside a multi-event append sees each state from its own event, not from the commit. |
| `put_membership(…, at)` | Whether a record is in an object set from a position on; earlier membership kept. |
| `put_output(…, at)` | A seam's recorded output, by seam and input digest, valid from `at`, the position of the event that recorded it. A later output for the same seam and input replaces the earlier one from its position on (022, extended: a redeclaration retries a failure). |
| `mark_applied(id, at)` | Remember an applied event id, so a redelivery is recognised. The first position marked for an id is kept. |
| `clear` | Delete everything, kept history and the vector index included, for a rebuild; `folded_through` is none afterwards. |
| `compact` | Give back the storage the projection no longer uses, outside a transaction (`Invalid` inside one), and answer its size in bytes before and after as the engine measures it, or none. No view answers otherwise afterwards: no state, kept history included, no applied id and no recorded output goes. A compact that fails leaves every answer as it was (`docs/decisions/040-maintenance-never-changes-an-answer.md`). |

Rebuilding a projection from the log produces the same state, byte for
byte, as the projection it replaces
(`docs/decisions/001-the-log-is-the-truth.md`); the conformance suite
checks it on every engine.

### `ProjectionView`: what reads see

A view reflects one position, and every answer is computed from the
states valid there, except that `lexical` ranks by the index at the head
(below). Records come back whole and unmasked; grants and
masking are the store's.

| Method | Contract |
|---|---|
| `position` | The position the view reflects. |
| `declarations` | Every declaration in force: for each `declare` and name, the latest put at or before the position, with the position it is in force `since`. No order is promised. |
| `get(type, key)` | One record's latest state at or before the position, retired and restricted included; none if it had none by then. |
| `scan(type, scan)` | The records of `type` whose latest state at the position qualifies, as below. |
| `edges(type, key, relation, walk)` | One record's links over one relation or all: `Out` those it declares, `In` those that point at it, `Both` their union with each link once (a link from a record to itself included). Links to a retired or missing record are returned as they are; hiding that end is the store's (`docs/decisions/026-the-store-walks-the-graph.md`). No order is promised; the store's walks order their own answers. A relation with `via` is carried by that reference field: a record links to the target whose key is the field's text value, and `Edge.fields` is empty. A relation without `via` is carried by the live records of its link type `sys.link.<type>.<relation>` at the position (`docs/log-format.md` §2.3): `Out` gives the links whose `from` is the record, `In` those whose `to` is, and `Edge.fields` holds each link's own fields, `from` and `to` left out. A removed (retired) link is no edge. The rule is `lakelands_core::links::link_edges`: at the head an engine may find a link type's candidates through its own index (SQLite and PostgreSQL look `from` and `to` up in their exact-text index), and at an earlier position it reads the link type's records there. |
| `lexical(type, text, k)` | Full-text hits at the position, as below. |
| `proposals(filter)` | Every proposal whose latest state at the position matches `filter.state` and `filter.type` (a `call`'s type is its verb's `on` type in force at the position), newest first — by the position of that latest state, descending, then by id — and at most `filter.limit`. `order_by`, `cursor` and the read position in the filter are the store's and the engine ignores them: the store applies the caller's `order_by` to the engine's answer, and refuses as `Invalid` an `order_by` naming a member a proposal does not have. |
| `is_member(set, type, key)` | Whether the record is in the object set at the position; false if never put. |
| `applied(id)` | The position `id` was applied at, if that is at or before the view's position. |
| `output(seam, input)` | The recorded output valid at the position, if any: the latest recorded for that seam and input at or before it. |

**`scan`.** A record qualifies when all of these hold:

- it is live, or `scan.retired` is set; it is unrestricted, or
  `scan.restricted` is set;
- for a valid-timed type and a `scan.valid_at`, it is valid at that
  instant as `lakelands_core::valid_time::record_holds` decides;
- `scan.filter` holds for it, exactly as the one evaluator
  (`lakelands_core::expr::holds`) decides over the record's own fields,
  with no settings and no clock. A filter the evaluator cannot evaluate
  for a record (an error) does not match that record. An engine that
  translates a filter into its own query language must select exactly
  the same records; where it cannot, it evaluates that part with the
  evaluator. The store hands an engine only filters that need neither
  the clock, the lake's settings, per-field provenance nor a pattern
  match (`now`, `today`, `setting`, `known_at`, `seen_at`, `matches`);
  it evaluates any other filter itself.

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

**The ceiling of `find`.** `find` asks the engine for its page,
never the type: its order, the row it continues after, and `limit + 1`
rows. A filter only the store can evaluate is applied over engine pages
read one after another, so its cost is the rows read to fill the page:
bounded in memory, linear in time. A page ordered by key reads about
`limit` rows, not the type; every engine reads it in key order from
`after` and stops when it is full (`lakelands_core::engines::
scan_in_key_order`). **A page ordered by a field reads its page when the
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
about 0.1 ms in memory, 0.3 ms on SQLite and 2 ms on PostgreSQL.
`describe_type` names a type's indexed fields in its lifecycle
(`sorted`). The index is used only while it describes the head; a lake
whose index a writer did not keep sorts as below until a writer's open
rebuilds it. **Its ceilings:** with several sort fields, the first
field's index orders runs of equal values and each run is read whole and
sorted, so a first field with few values reads most of the type (a
boolean first: about 0.3 s in memory, 0.45 s on SQLite and 1.3 s on
PostgreSQL at 100 000 records). A field the lake did not ask an index
for, an earlier position, an order by `known_at`, `seen_at` or `latest`,
and a view type read the type at the position and sort it by `sort_key`,
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
of each record it reaches (`edges`), so what a walk holds and how long
it takes follow what it reaches, not the lake. At the head every engine
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
reads the declaring types and link types there whole, for every record
the walk reaches, so a walk into the past costs the relation's records
per hop. The heap is held by the suite; the time is reported.

**The whole-graph measures.** A pattern query projected as `degree` or
`components` asks nothing new of an engine: the store asks `edges` of
every record it measures, as a walk does
(`docs/decisions/026-the-store-walks-the-graph.md`). A degree reads the
start's records a page at a time and keeps the best rows asked for, so
the streaming check's `degree` line, the ten records of highest degree
over the whole type, holds the same heap at every size of the lake.
Components hold every record they reach, so their `components` line
grows with the graph and is reported; the lake's `max_graph_nodes`, a
limit of its `Location` (100 000 unless it says otherwise), bounds them,
and past it they are refused `too_large` naming the ceiling. The
`components-capped` line holds that bound: with the ceiling at half the
lake's first size, the walk is refused at the ceiling at every size.

**`lexical`.** Search is ranked retrieval: the view's position decides
what may be seen, not how the current text ranks. A view answers from
the engine's full-text index as it stands at the head, and keeps only
records that are live at its own position, restricted ones included (the
store applies the principal's grants to the hits, as for `find` and
`get`); an engine is
not required to keep a full-text index of earlier states. A record is a
hit when one of its searchable fields' text at the head holds at least
one token of `text`. Tokens are what the store's one pipeline makes of the text
(`lakelands_core::stem::stem_text`, stemmed by the type's `language`, or only
lower-cased and folded when it has none or the pipeline has no stemmer
for it), applied the same way to the query and to the
fields (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
The set of hits is the same on every engine. Hits come best first, by a
score that is higher for a better match and may differ between engines;
equal scores are ordered by key ascending; at most `k`. The in-memory
engine scores a hit by the number of distinct query tokens it holds.

A view does not aggregate. Aggregates — over `find`, over a pattern's
rows and in a view type's measures — are the store's: it computes them
over the records it reads, decimal-exact and grouped in the one sort
order, so every engine answers them identically
(`docs/decisions/029-every-answer-has-one-order.md`). An engine may keep
indexes that make the scans behind them faster; it never computes a
group.

An aggregate's `group_by` keys are field names (`"partner"`) or named
expressions (`{"name": "month", "expr": "date_bucket(due_on, 'month')"}`),
evaluated per record by the one evaluator and answered under their name
in each group's `by`. A named key's name follows the rule for field names
and differs from every other key's, and the principal must be allowed to
read every field its expression reads (024). A value the expression
cannot compute for a record is null, so that record falls in the null
group, which sorts last. `date_bucket(x, unit)` answers the instant in
UTC at which the bucket holding `x` starts, for `day`, `week` (ISO, from
Monday), `month`, `quarter` and `year`; it belongs to reads, and a
declaration calling it is refused (`docs/decisions/017-one-expression-grammar.md`,
extended). **The ceiling:** buckets are UTC only, and there is no number
bucket (lakelands#174).

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

`put`, `remove`, `nearest(type, field, query, k)` and `count`, over the
vectors of one field of one type:

- `put` replaces a record's vector for the field; `remove` forgets it,
  and forgetting one that is absent does nothing.
- `nearest` answers at most `k` records, scored by the **cosine** of the
  angle between the query and the stored vector (higher is closer; a
  vector of zeros scores 0 against everything), best first, equal scores
  by key ascending. A field holding no vectors answers nothing; a query
  whose length differs from a stored vector's is `Invalid`.
- `count` is how many vectors the field holds.

The store keeps the index in step while it folds: it puts a live
record's vectors and removes a retired record's, or a cleared field's.
Vectors are indexed at the head only. As for `lexical`, a read at an
earlier position ranks by the index at the head and keeps only the
candidates visible at that position.

**The ceiling.** Every engine searches by brute force with the one
cosine (`lakelands_core::engines::cosine`) and keeps the best `k` as it
goes (`lakelands_core::engines::TopK`), whose answer is what ranking
every vector and cutting at `k` answers, to the bit. So what a search
holds is `k` entries, whatever the field holds; SQLite steps its
statement, and PostgreSQL streams the field from the server a row at a
time without buffering it. What a search costs is a scan: `nearest`
reads every vector of the field, so its time grows linearly with the
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
`describe_type` names them as `indexed`. On PostgreSQL the index is
pgvector's HNSW (`m = 16`, `ef_construction = 64`, `ef_search = max(k,
200)` up to 1 000, iterative scans past it) over each vector's unit
vector, used while its mark equals `folded`; SQLite and memory keep none
and answer the exact ranking. Every other field answers the exact
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
| a sealed segment's digest is over its lines; sealing an empty segment answers none | engine: `a_sealed_segment_is_digested_as_its_lines` |
| a dropped writer releases its lease | engine: `a_dropped_writer_releases_its_lease` |
| a read-only handle refuses the lease, appends and sealing, and reads | engine: `a_read_only_handle_refuses_the_lease_and_appends` |
| a read-only open creates nothing and writes no projection | engine: `a_read_only_open_creates_and_changes_nothing` |
| a view at a position reads the states valid there | engine: `a_view_at_a_position_sees_the_states_then`; suite: `a_read_as_of_a_write_sees_that_write_and_nothing_later`, `a_read_as_of_the_position_before_a_write_does_not_see_it`, `retiring_a_record_does_not_erase_its_history` |
| a view past the fold is `OutOfRange` naming the head | engine: `a_view_past_the_folded_position_is_refused`; suite: `a_read_as_of_a_position_past_the_head_is_refused_as_out_of_range` |
| a rolled-back transaction leaves nothing | engine: `a_rolled_back_transaction_leaves_nothing` |
| edges `Out`, `In`, `Both`, a self-link once, links to a retired record kept | engine: `edges_walk_in_out_and_both`, `edges_to_a_retired_record_are_returned` |
| edges from link records: `Out` and `In` by `from` and `to`, the link's fields in `Edge.fields`, a removed link gone, an earlier position's links | engine: `edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position`; doors: the sixteen `links` checks |
| a rebuild answers every read as before | suite: `a_rebuilt_projection_answers_every_read_as_the_live_one_did` |
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
| `open` of a missing lake is `Unavailable`; `Create` of an existing one opens it | engine: `opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one` |
| a commit backwards, or with a put past `through`, is `Invalid`; a view does not see an open transaction; a view keeps its position after later commits | engine: `a_commit_backwards_or_with_a_put_past_through_is_invalid`, `a_view_does_not_see_an_open_transaction`, `a_view_keeps_its_position_after_later_commits` |
| a put of the same thing at the same position replaces it | engine: `a_put_at_the_same_position_replaces_the_state_there` |
| `scan`'s order, `after` and `limit`, including a limit of 0 and values of mixed kinds | engine: `mixed_kinds_sort_the_same_on_every_engine`, `a_scan_continues_strictly_after_its_keyset_position` |
| a key-ordered `scan` read in batches answers as one scan over every state | engine: `a_key_ordered_scan_answers_across_its_batches_as_one_scan` |
| a `find` page crosses the seam as about `limit` rows, with or without a filter only the store evaluates | store: `tests/find_pages.rs` |
| `scan`'s filter matches the evaluator exactly, including comparisons across kinds and filters that error; the flags and `valid_at` | engine: `scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors`, `scan_honours_retired_restricted_and_valid_at` |
| `lexical`'s hit set, its language, searchable fields only, live only (restricted included), ties by key, `k`; at an earlier position, visibility from the position and text from the head | engine: `lexical_hits_are_the_pipelines_over_searchable_live_records`, `lexical_finds_every_token_the_pipeline_makes_beyond_ascii`, `lexical_at_an_earlier_position_ranks_by_the_head_and_keeps_what_was_visible_then` |
| `nearest`'s exact top `k` by cosine, ties, a zero vector, a wrong length; `put` replaces; `clear` empties the index | engine: `nearest_ranks_by_cosine_with_ties_by_key`, `a_zero_vector_scores_zero_and_a_wrong_length_is_invalid`, `put_replaces_a_vector_and_remove_forgets_it`, `clear_empties_the_projection_and_the_vector_index` |
| `declarations`, `is_member`, `output` and `applied` at an earlier position; `mark_applied` keeps the first position | engine: `declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position`, `the_first_applied_position_is_kept` |
| `proposals`' filters, a `call`'s type, and its order | engine: `proposals_filter_by_state_and_type_newest_first` |

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
  never from the fold, a read, opening a lake or `rebuild`. It calls one
  backing at a time per lake.
- It records every answer as `output_recorded`. The answer is keyed by
  the seam `function:<kind>:<function>` and the digest of
  `{ "function", "inputs", "config" }` (`docs/log-format.md` §2.5).
- It never sends one input to a backing twice: a recorded output is
  read back for every later call with the same input, failures
  included. The one exception is deliberate: declaring the function, or
  a type whose fields it computes, again calls once more for an input
  whose recorded output is a failure. The later output holds
  (`docs/decisions/022-functions-are-declarations.md`, extended).
- A failure (an error, a timeout, or an output that is not a value of
  the field's type) clears the field, and the field's metadata carries
  the reason as `error`. A cleared field without `error` is one whose
  output is not recorded yet.
- A fill is a change of the field like any write's. Derived fields
  that read it are recomputed, triggers fire and object sets change,
  caused by the output.
- A type's computed fields read one another without a cycle; a
  declaration that closes one is refused.
- A write whose calls a drain made answers with the lake's head once
  the calls are made: the position from which its filled fields are
  visible. A write with nothing due answers at once, with its own last
  event's position.

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
text, in order.

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
fix, not weather. A vector search, and a hybrid search that brings its
own vector, never call the backend.

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
  as `Unbound`, naming what it needs. A due embedding whose backend is
  gone, or names another model or other dimensions, when the call would
  be made is not made; the field waits. It is made when its record is
  written again, or its type declared again, with the backend bound;
  opening the lake calls nothing. A retried embedding waits the
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
  The field's `procedure_version` is the type's version when the vector
  was produced. A later declaration of the type that leaves the vector's
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
  however many records a backfill makes due, so its memory per drain
  stays bounded. The client may split a batch further by its own
  `max_batch`. A batch that fails, in any of its requests, records a
  failure for each text it carried.

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
- Every failure is `Unavailable`, naming the endpoint's host, port and
  path and what went wrong. It never carries the key, the texts, the
  request, the reply or a path on the server: the texts are a caller's
  query, and an `Unavailable` reaches every caller.
- Embedding no text makes no request.

## `VerbExecutor`

`name()`, `execute(invocation) -> value`. A verb whose declared effect
reaches outside the lake names an executor; when that verb runs (at once
if declared `low`, on acceptance otherwise, as
`docs/decisions/013-verbs-run-by-declared-risk.md` decides),
the store hands the executor a `VerbInvocation`: the lake, the verb, the
record as it stands at the head when the executor is called, after the
append that made the verb due and the function calls that append made
due, whole and **unmasked**, the payload, and
the position of the event that made it due. The lake and that position
are the executor's dedup key. The executor is the host's code, bound by
the host, and decides what it returns; the caller sees only the outcome
and the recorded output, never the record the executor was given. A host
that wants an executor to see only what the caller may see applies the
caller's grants in the executor. Its answer is recorded as an
`output_recorded` event caused by that event, with `input` the digest of
`{ "verb", "record", "payload", "position" }`.

An executor that fails does not undo the verb: the event that made it
due is in the log, and its position is real. The failure is the recorded
output — `{"error": "<the executor's error text>"}`. The caller's outcome
is `applied` at the position that made the verb due, and carries the
recorded output in its `output` member, the error form included, so an
agent that called an external verb has the answer in the reply. The verb
is not run again for that position. A host that wants a retry calls the verb again, which is a new
position.

A call retried with the same idempotency key runs nothing and answers
the first outcome: the position that made the verb due and the recorded
output. The output's `input` digest holds the record as the executor was
given it, which a retry cannot rebuild, so the retry finds the output as
the `output_recorded` caused by that position, among the 4,096 events
after it. An output not among them (not recorded yet, or recorded after
the lake took that many other events while the executor ran) is answered
as none; the position stands, and nothing runs again.

There is no default: a verb whose executor is not bound is refused as
`Unbound`, never faked. The store checks it when the verb is called and
again when a proposal to call it is accepted, before anything is
written.

## `ChangeFeed`

`poll(from, filter, limit) -> batch` and `ack(consumer, through)`. The
feed is the log filtered by kind and type, read from a position
(`docs/decisions/004-the-change-feed-is-the-log.md`). `poll` answers at
once, possibly empty, never waiting for new events:

- `events`: at most `limit` events (a `limit` of 0 is read as 1) from
  `from` up to the head, each verified as it is read, in position order,
  whose kind is one of `filter.kinds` and whose type is one of
  `filter.types`; an empty list in the filter admits every kind or every
  type. An event has a type when it is a `record_*`, `trigger_fired` or
  `object_set_changed` event, a `declared` type or a `migrated` one (a
  trigger on a relation carried by link records watches its link type,
  `sys.link.<type>.<relation>`, so its `trigger_fired` events carry that
  type and the link's key; `docs/decisions/035-a-link-is-a-record.md`); an
  event without one passes only a filter with no types. The kinds are
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

  A key filter reads the log from `from` as every filter does, so one
  record's history asked for from position 0 reads the whole log.
  `lakelands-client` refuses a filter with keys as invalid, naming
  `keys`, until the REST feed takes them.
- `next`: the position after the last event examined, matched or not,
  which is where the next poll starts.
- `head`: the lake's last position when the poll began.

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
  any other kind is refused when the configuration loads. Both carry
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
    which is `delivered`, `retrying` (with `why` and `retry_in_secs`) or
    `refused` (with `status`). It is only ever appended to, and opened
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
lake of the root into another, the only way water crosses between lakes.
There is no query across lakes and no reference into another lake. A
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
  the source's `known_at` as `seen_at`. The copy's fields are `source`
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
  reason, said on standard error and in the position file, and the
  connection's other types go on. A restart asks again from where it
  stopped. A new source field crosses once the target declares it, for
  the records that change after.
- **Where it starts.** A type with no position is copied as the source
  holds it at its head, then followed from there (`start: snapshot`, the
  default), or only followed from the head (`start: head`). History is
  never replayed: that would walk every copy through every past state
  and fire its triggers on the way. Deleting a type from the position
  file copies it again, and every unchanged field appends nothing.
- **The principal** is one of the root's `principals`, resolved in each
  lake: in the source it needs to read what crosses, and `restricted`
  for holds to cross; in the target it needs `ingest` and write access
  to every field of the mirrored types.
- **Its files**, under `mirrors/<target lake>/` in the root, are interim:
  the target lake's own record of each connection replaces them.
  - `<connection>.json` holds, per source type, the next position to
    read and, when it stopped, where and why:
    `{"types": {"customer": {"next": 120, "stopped": {"at": 118, "why": "…"}}}}`.
    It is written after each window or action is durable, so delivery is
    at least once, in order.
  - `<connection>.lock` is held while the mirror runs, so a second
    process refuses to start.
  - `<connection>.ndjson` is the journal, one line per window, per
    retirement or hold, and per stop.
- **The source is a lake of the same root.** A source served elsewhere
  is not offered: the feed at a network door opens only to a principal
  that reads everything (`docs/decisions/037-the-feed-over-a-network-door.md`),
  and the mirror's reads must be the connection's principal.
- **Checked when the root opens:** every lake a connection names is one
  the root holds, and its principal one the root lists; every connection
  has a name of letters, digits, `-` and `_`, used once, and maps at
  least one type; no lake is mirrored into itself; no `sys.` type is
  mapped; no target type has two sources; no type is mirrored into a
  lake and back out of it; no number is zero. **Checked when `serve`
  starts a mirror:** the target is held as its writer; every field of a
  target type is written by the `source` authority and writable by the
  principal, and every reference and link leads to a type the connection
  mirrors; and, for a type with no position yet, the target admits the
  source's declaration.

## `SystemVocabulary`

`types()` lists a host's own `sys.` types as type declarations;
`may_hold(lake, type)` says whether a lake may hold one. Lakelands
reserves only the system types its own fold reads: `sys.source`,
`sys.provenance`, `sys.setting`, `sys.aging_rule`
(`docs/decisions/003-reservation-by-namespace.md`). Today the fold reads
`sys.setting` and `age` reads `sys.aging_rule`; the other two are
reserved ahead of their readers. The default adds none and allows those
four everywhere.

`types()` carries type declarations only. A host whose vocabulary also
needs value types, verbs, triggers or functions applies them itself,
after open, on the writer, through `declare` or `apply`. Neither is
idempotent: declaring again is a new version and has meaning (a
recorded failure is retried once; 022), so the
host first compares what it would apply with what the lake holds
(`describe` answers each declaration whole, and each type's version
and schema hash) and applies only what differs. The vocabulary's
types are declared by the open; everything else is the host's apply.

The vocabulary is consulted every time a lake is opened for writing:
the store declares every host type that `may_hold` allows for that lake
and the lake does not hold yet, as system declarations in one append
(at creation, together with `sys.setting`). A type the host adds later
reaches an existing lake the next time a writer opens it.

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

A `sys.setting` record is one named setting of a lake. Its record key is
the setting's key, and it has two fields: `key` (text, the same key) and
`value` (json, the setting's value). `setting('vat_rate')` in any
expression reads the `value` of the `sys.setting` record with key
`vat_rate` as of the position being read or folded, so a replay reads the
setting as it then stood.

One setting is read by the store itself: **`sort_index.<type>`** holds a
JSON array of field names of `<type>` and asks every engine to keep an
order index on them, so that a `find` ordered first by one of them reads
its page, not the type (§`find`'s ceiling above;
`docs/decisions/041-a-sorted-page-reads-its-page.md`). A write of it is
refused as invalid, naming the field, when it names a field the type
does not declare or declares with a kind other than text, number,
decimal, boolean, date-time or reference, or when `<type>` is no
declared stored type (the field named is then `key`), or its value is
not an array of names (`value`). The fold that puts it rebuilds that
type's index in the same transaction; retiring it, or an empty array,
drops the index. It changes no answer, only the price of an ordered
page.

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
| `field` | text, required | the field it moves; `authored` or `shared`, never derived |
| `from` | json, required | the value that ages |
| `to` | json, required | the value it becomes |
| `when` | text, required | a condition in the one grammar, over the record's fields |
| `description` | text | for people |

A rule is written like any record, through `upsert` under the writer's
grants, and checked when it is written: `on` names a type that is
neither a `sys.` type nor a view, `field` one of its fields of the
authority above and not part of its key, `from` and `to` are values of
that field's type and differ, and `when` parses and uses neither `changed` nor
`previous`. A rule that fails a check is refused as invalid, naming the
rule field at fault. A rule's authority is checked when it is written,
and the run carries it: its author must hold write on `field` of `on`
and read on every field `when` reads
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`), or
the rule is refused as not granted, naming that field; `age` then moves
the records as the system. `setting()` in `when` reads the lake's
settings as a filter does; no grant on `sys.setting` is asked, and if
settings ever come under a grant, `find` and `age` change together. The
mock (`lakelands-mock`) refuses `setting()` in a rule's `when`; the
store takes it. A bundle declares rules in its `aging_rules/` lane,
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
two held elsewhere: that a lake made before aging gains
`sys.aging_rule` at its next writer's open is a store test
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
line per rule (`<rule>: moved N, M guard(s) failed`, or `<rule>:
skipped, <why>`) and `aged <date> through position P`. No MCP tool calls `age`:
like `rebuild` and `checkpoint`, it is the host's.

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
  `type`, `required`, `authority`, `derived` or `function`, `depends_on`,
  `on_input_change`, `searchable`, `sensitive`, `label`, `description`,
  `embed` — beside its `access` and whether it is `filterable`. A type
  carries its `description`, `label`, `language` and `view`, and on its
  access axis `key`, `valid_from`, `valid_to` and `field_meta`. For a
  principal who reads every field, a type's description projected back
  is its declaration byte for byte, so a host redeclares a type from
  `describe` without replaying the feed.
- A masked field shows its shape only: `name`, `type`, `access`,
  `sensitive`, `required`, `authority` and `on_input_change`, neither
  `filterable` nor `searchable`, and no `derived`, `function`,
  `depends_on`, `embed`, `label` or `description`
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
  Until 0.4.0 a masked field also showed `derived`, `depends_on`,
  `description` and `embed`.
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
that ceiling, on every door that writes, and appends nothing (suite:
`a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing`).

`LakeStore::head` answers the last position the projection has folded:
a position a read can be answered at at once, on every engine. An
append is durable before it is folded; the store publishes its head
only once the fold has committed. When a fold fails after its events are
in the log, the append answers the fold's error, the head stays at the
last folded position, and nothing more is appended until the projection
has caught up: the next append first folds what the log holds past the
projection, or answers that fold's error and appends nothing. A handle
opened only to read names the position the writer has folded.

`rebuild`, `verify` and the fold at open read the log in pages of 1 024
events and hold no more than one. A rebuild and an open's catch-up fold
page by page, each page in its own transaction that commits
`folded_through` at its last event. A fold that stops, by an error or a
crash, leaves the projection folded through the last page it committed:
`head()` names that position, a read at or before it answers the lake as
of there, a read past it is refused as `OutOfRange` naming it, as for a
failed append, and never answers from half a page. The next fold (an
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

`LakeStore::purge_source(source)` retires every live record one source
brought in, across every type it reached, restricted records and the
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

`lakelands purge --source <id>` prints one line per type
(`<type>: retired N`), one per type not searched, and
`purged <id> through position P`; when the source brought in nothing
still live, it prints `nothing to retire for <id>` and appends nothing.
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
instead of `to`, one of the two, never both. The preflight checks stored
values only: a condition that compares with a value the type no longer
admits (a trigger's `when`, an object set's `where`) is not checked, and
simply never holds afterwards; an aging rule whose `from` or `to` the
type no longer admits is skipped by `age` and named in its report.

A migration may re-key a type: name other key fields, or change a key
field's value. Its preflight (`MigrationReport.rekeyed`, `references`,
`sample_collisions`) counts, per type, the records whose key moves,
retired and restricted ones included, with every type keyed by a
reference to a re-keyed type (link types among them) and every reference
value that names a moved key; two records that would take one key, or a
record without a value for a new key field, make it not lossless. Applied,
it is one `migrated` event listing every move (`rekeyed`, log format
version 2, `docs/log-format.md` §2.3 and §6): each record keeps its
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
