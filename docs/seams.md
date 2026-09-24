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
| Engine | `LakeEngine`, `ProjectionStore`, `ProjectionView`; `VectorIndex` beside them | SQLite and in-memory (`0.1.0`); PostgreSQL after | Rarely anything: a new engine, proven by the conformance suite |
| Principal resolution | `PrincipalResolver` | API key or static entry → one principal | Its identities, roles and policies, compiled to grants |
| Function backings | `FunctionBacking` | `expr`, evaluated in the store | Its own backing kinds; `http` and `exec` ship after `0.1.0` |
| Embeddings | `EmbedBackend` | None: lexical search and caller vectors | An embedding client; an OpenAI-compatible one ships after `0.1.0` |
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

An engine is found by name and opened through `EngineOpener`:

| Method | Contract |
|---|---|
| `name` | The name the engine answers to in `EngineConfig`. |
| `open(location, mode)` | Open the lake `Read`, `Write` or `Create` (write, creating it if absent, opening it if present). `Read` or `Write` of a lake that does not exist is `Unavailable`; a location configured for another engine is `Invalid`. Opening never takes the lease; a writer takes it afterwards with `take_lease`. A handle opened `Read` refuses `take_lease`, `append` and `seal_segment` as `Unavailable`, and writes nothing to the projection (`begin`, the puts and `clear` are `Unavailable`), so any number of readers can hold the lake while one process writes (`docs/decisions/008-one-writer-per-lake.md`). A read-only open of a lake that does not exist creates nothing. The store folds only on the writer's handle; a reader reads what the writer folded. |

### `ProjectionStore`: what the fold writes

The fold calls the projection inside one transaction per append;
`commit(through)` records the position folded through in the same
transaction, so a crash leaves the projection at a whole append, and
opening a lake folds the log from `folded_through` on.

- The store never nests transactions and never puts outside one.
- Nothing put in a transaction is visible to any view until it commits;
  a commit makes all of it visible at once, with its `through`.
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
| `put_output(…, at)` | A seam's recorded output, by seam and input digest, valid from `at`, the position of the event that recorded it. |
| `mark_applied(id, at)` | Remember an applied event id, so a redelivery is recognised. The first position marked for an id is kept. |
| `clear` | Delete everything, kept history and the vector index included, for a rebuild; `folded_through` is none afterwards. |

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
| `edges(type, key, relation, walk)` | One record's links over one relation or all: `Out` those it declares, `In` those that point at it, `Both` their union with each link once (a link from a record to itself included). Links to a retired or missing record are returned as they are; hiding that end is the store's (`docs/decisions/026-the-store-walks-the-graph.md`). No order is promised; the store's walks order their own answers. In this release a link is carried by the reference field its relation names in `via`: a relation without `via`, or with fields of its own, is refused when declared, and link records come later. |
| `lexical(type, text, k)` | Full-text hits at the position, as below. |
| `proposals(filter)` | Every proposal whose latest state at the position matches `filter.state` and `filter.type` (a `call`'s type is its verb's `on` type in force at the position), newest first — by the position of that latest state, descending, then by id — and at most `filter.limit`. `order_by`, `cursor` and the read position in the filter are the store's and the engine ignores them: the store applies the caller's `order_by` to the engine's answer, and refuses as `Invalid` an `order_by` naming a member a proposal does not have. |
| `is_member(set, type, key)` | Whether the record is in the object set at the position; false if never put. |
| `applied(id)` | The position `id` was applied at, if that is at or before the view's position. |
| `output(seam, input)` | The recorded output valid at the position, if any. |

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
field's value, or its `known_at` or `seen_at`; a record without a value
for a key after every record with one, in either direction; the record
key ascending to break ties; values ranked and compared by
`lakelands_core::expr::order::cmp_values`, the one sort order
(`docs/decisions/029-every-answer-has-one-order.md`) — then only those
strictly after `scan.after` in that order, then at most `scan.limit`
(literal: 0 answers nothing). The store sorts and pages itself in this
release, and asks for unordered, unlimited scans; an engine implements
`order_by`, `after` and `limit` anyway, so that it can take over paging
later without a contract change. Moving paging to the engine is an
optimisation that may never change an answer
(`docs/decisions/029-every-answer-has-one-order.md`).

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

### `VectorIndex`

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

**The ceiling.** Both engines of `0.1.0` search by brute force with the
one cosine (`lakelands_core::engines::cosine`): `nearest` reads every
vector of the field, so its cost grows linearly with the vectors a field
holds, and with their length. At 1,536 dimensions that is about 6 KiB
read and one dot product per vector per query; a field of a hundred
thousand vectors is a scan of some 600 MiB. An index that answers faster
than a scan, sqlite-vec among them, is an implementation of this seam
after `0.1.0`: it picks candidates, and the one cosine ranks them, so an
answer never depends on its arithmetic.

**Exactness.** Every index in `0.1.0` is exact: `nearest` answers the true
top `k` by cosine, and the checks require it. An approximate index (HNSW
in PostgreSQL, after `0.1.0`) will say so through an additive method
whose default answer is "exact"; it answers its candidate set, the store
re-ranks those candidates by the same cosine, and the search's
`ranked_by` says the ranking was approximate. That door needs no change
to the rest of the contract.

### Where each promise is checked

An engine check runs through the seam
(`lakelands_core::conformance::engine`); a suite check runs through the
doors, so it checks the engine only as far as the store's use of it
reaches. "None yet" is a promise the engine must keep that no check
holds it to; each is proposed on lakelands#15 as an engine check for O4.

| Promise | Checked by |
|---|---|
| bytes, members and `tip` come back as appended; `read` honours `from` and `limit` | engine: `appended_bytes_come_back_unchanged` |
| a stale lease cannot append, and an event carrying another fence is refused | engine: `a_stale_lease_cannot_append` |
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
| a rebuild answers every read as before | suite: `a_rebuilt_projection_answers_every_read_as_the_live_one_did` |
| `scan` filters, the retired and restricted flags, `valid_at` | suite, through `find` and `get` (paging, restriction, views, decimals) |
| `mark_applied` / `applied` recognise a redelivery | suite: `the_same_content_ingested_twice_appends_nothing_the_second_time`, `a_retry_with_the_same_idempotency_key_answers_the_first_outcome_and_appends_nothing` |
| `append` is durable before it returns | none yet (needs a harness that kills the process) |
| the stale-lease refusal is `Unavailable`; another fence is `Invalid` | engine: `a_stale_lease_cannot_append` |
| releasing a stale lease leaves the current holder's lease | engine: `releasing_a_stale_lease_leaves_the_current_holders_lease` |
| `read` past the tip or with `limit` 0 is empty; `tip` of an empty lake is none | engine: `reading_past_the_tip_or_with_limit_zero_answers_nothing` |
| an event at position 0 with a fence other than 0 is refused | engine: `an_event_at_position_zero_with_a_fence_is_refused` |
| sealing under a stale lease is refused; two seals list contiguous ranges | engine: `sealing_under_a_stale_lease_is_refused_and_two_seals_list_contiguous_ranges` |
| `open` of a missing lake is `Unavailable`; `Create` of an existing one opens it | engine: `opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one` |
| a commit backwards, or with a put past `through`, is `Invalid`; a view does not see an open transaction; a view keeps its position after later commits | engine: `a_commit_backwards_or_with_a_put_past_through_is_invalid`, `a_view_does_not_see_an_open_transaction`, `a_view_keeps_its_position_after_later_commits` |
| a put of the same thing at the same position replaces it | engine: `a_put_at_the_same_position_replaces_the_state_there` |
| `scan`'s order, `after` and `limit`, including a limit of 0 and values of mixed kinds | engine: `mixed_kinds_sort_the_same_on_every_engine`, `a_scan_continues_strictly_after_its_keyset_position` |
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
Once a backing other than `expr` computes fields, the store records
every output as `output_recorded`, keyed by the function and the digest
of `{ "function", "inputs", "config" }` (`docs/log-format.md` §2.5), and
never calls a backing twice for one input: a replay reads the recorded
output.

The default is `expr`, evaluated inside the store in the one expression
grammar; because it is deterministic, its results are recomputed on
replay rather than recorded. In `0.1.0` it is the only backing that
computes fields: declaring a field computed by a function of another
backing is refused as `Unbound` naming that backing, whatever the host
passed in `Seams::backings`. `http` and `exec` follow after `0.1.0`,
and a host may add its own kind then.

`lakelands_store::ExprBacking` is that backing as the seam presents it,
for a host or a door that calls a function directly: the call's `config`
carries the expression as `{ "expr": … }` (typed or text form), its
`inputs` the values by input name, and the answer is the computed value,
a number as an exact decimal string. The fold computes function fields
with the same evaluator in process, so the two can never disagree.

## `EmbedBackend`

`model()`, `dimensions()`, `embed(texts) -> vectors`, one vector per
text, in order.

In `0.1.0` the store calls the backend for one thing: a hybrid `search`
without a caller's vector embeds its query text through the bound
backend and ranks over the result. That is a read, and nothing is
recorded; a backend that answers other than one vector for the one text
makes the search `Unavailable`. Vectors stored in records come from
callers. Embedding a field's value when it is written comes with the
embedding client after `0.1.0`; each such vector is then recorded as
`output_recorded`, with the model as its procedure, and read back on
replay.

There is no default backend: without one, `search` ranks lexically and
over vectors callers supply, and says in its answer which ranking it
used. An OpenAI-compatible client follows after `0.1.0`.

## `VerbExecutor`

`name()`, `execute(invocation) -> value`. A verb whose declared effect
reaches outside the lake names an executor; when that verb runs (at once
if declared `low`, on acceptance otherwise, as
`docs/decisions/013-verbs-run-by-declared-risk.md` decides),
the store hands the executor a `VerbInvocation`: the lake, the verb, the
record at the head right after the append that made the verb due,
whole and **unmasked**, the payload, and
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
  `object_set_changed` event, a `declared` type or a `migrated` one; an
  event without one passes only a filter with no types.
- `next`: the position after the last event examined, matched or not,
  which is where the next poll starts.
- `head`: the lake's last position when the poll began.

The consumer keeps the position it has read up to. The store's own feed
keeps no consumer state, and its `ack` is accepted and does nothing; `ack` exists for feeds
that relay elsewhere (a remote client, a bridge onto a bus) and need to
know what they may release. The dedup key of a change is its lake and its
position.

## `SystemVocabulary`

`types()` lists a host's own `sys.` types as type declarations;
`may_hold(lake, type)` says whether a lake may hold one. Lakelands
reserves only the four system types its fold reads: `sys.source`,
`sys.provenance`, `sys.setting`, `sys.aging_rule`
(`docs/decisions/003-reservation-by-namespace.md`). The default adds
none and allows those four everywhere.

The vocabulary is consulted every time a lake is opened for writing:
the store declares every host type that `may_hold` allows for that lake
and the lake does not hold yet, as system declarations in one append
(at creation, together with `sys.setting`). A type the host adds later
reaches an existing lake the next time a writer opens it.

A `sys.setting` record is one named setting of a lake. Its record key is
the setting's key, and it has two fields: `key` (text, the same key) and
`value` (json, the setting's value). `setting('vat_rate')` in any
expression reads the `value` of the `sys.setting` record with key
`vat_rate` as of the position being read or folded, so a replay reads the
setting as it then stood.

A `sys.aging_rule` record is one **aging rule**: on a date, move a field
of every record of a type from one value to another when a condition
holds (`docs/decisions/036-aging-runs-by-date-once.md`). The store declares
the type when it creates a lake, beside `sys.setting`. Its key is
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
grants, and checked when it is written: `on` names a type and `field`
one of its fields of the authority above, `from` and `to` are values of
that field's type, and `when` parses and uses neither `changed` nor
`previous`. A rule that fails a check is refused as invalid. A bundle
declares rules in its `aging_rules/` lane, one per file, with the same
keys; `lakelands apply` writes each after the bundle's declarations,
skipping a rule whose stored fields already equal the file's.

`LakeStore::age(today)` runs every live rule once for `today`, a date,
in rule-name order, and answers what it did: the date, the rules run,
the transitions written, the guards that failed, and the position after
the last append. For each rule it reads the live records of `on` at the
head and moves every record whose `field` equals `from` (as values, by
their canonical JSON) and for which `when` holds, with `today()` and
`now()` both meaning `today` at midnight:

- A rule's transitions are one append of `record_upserted` events, each
  setting `field` to `to`, with `origin: system`, the writer
  `system:lakelands`, `procedure` `{ "name": "aging:<rule>", "version":
  <the rule record's version> }`, `context.origin` `scheduled` and no
  `cause`. Triggers and object sets react to them in the same append,
  as to any write.
- Each transition has the id of `docs/log-format.md` §3 for the rule,
  the record and the date, so a second `age` for the same date appends
  nothing.
- A `when` that cannot be evaluated for a record does not move it; it is
  counted as a failed guard, and the run goes on.

The ticker (lakelands#12) calls `age` once a day. Without it, a host, or
`lakelands age --today <date>`, calls it.

## The facade and the doors

Two more traits sit above the seams. They are what callers and hosts
hold, not what hosts implement:

- `Catalog`: the doors of one lake, for any caller, every call naming its
  principal. Tool names for LLM-facing transports come only from
  `lakelands_api::tools::tools`.
- `LakeStore`: one open lake, as its host holds it; it hands out a
  `Catalog` bound to a `WriteContext`, the change feed, the limits it was
  opened with (`limits`), lake maintenance (`verify`, `rebuild`,
  `checkpoint`), and `age` (above, under `SystemVocabulary`).

`Catalog::apply` declares a set of declarations as one: validated
together, so a reference may point forward within the set, and appended
in one append or refused whole, one refusal naming every problem
(`docs/decisions/028-authoring-is-strict-and-whole.md`). It is how a
bundle reaches a lake; it has no tool in `0.1.0`.

The `seam-width` gate counts the methods of `LakeStore` and the three
engine traits together, and holds them at 110 or fewer. `Catalog` is not
in that count: its width is governed by the four-doors rule
(`docs/decisions/014-four-doors-per-type-one-name-generator.md`).
