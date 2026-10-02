# Port log

The core is imported from its source at one frozen commit,
`3d2cfc7b4dfb5348086cf1342aa1751846af4793`. The source keeps moving
after the freeze. Every change made to the storage core there after that
commit is listed here, one row each, and is either ported into Lakelands
or marked not applicable with the reason. The list is closed, and every
row settled, before the source platform switches to Lakelands.

**Checked through 2026-09-24, for `0.1.0`**, against the source's mainline
at `a789d52b23`: every commit after the freeze (123 of them) that touches
a path an organ was imported from, which is the storage core (contract
and types, the store, the engines), the expression grammar and
value-format crates folded into `lakelands-core`, the template loader's
apply lane the bundle format came from, and the tool layer the catalog
was cut from (`docs/import-ledger.md` lists every one). One commit touches
any of them; it is the row below, settled. **The list is closed for
`0.1.0`.** It is checked again before the platform switches.

**The `{pg}` root, checked on 2026-09-24 for lakelands#65**, when the
PostgreSQL engine was taken from it, against the source's mainline at
`87e79584fc`: no commit after the freeze touches it, so it adds no row.

**Checked again on 2026-09-24, for `0.2.0`**, against the source's
mainline at `3cea3c6a4f`: every commit after the freeze (157 of them)
against all nine roots of `docs/import-ledger.md`, `{pg}` included, and
for `{tools}` the ten files the catalog was cut from. Each root holds
the same files it held at the freeze, so no change was hidden by a
move. One commit touches any of them, and it is still the row below.
**The list is closed for `0.2.0` as of that commit**; if the mainline
moves before the tag, the check is run again first.

**Checked again on 2026-09-25, before the `0.2.0` tag**, against the
source's mainline at `c0b30a3f2e`: the mainline had moved, and the 14
commits after `3cea3c6a4f` (171 after the freeze in all) were checked
the same way. None of them touches a root, each root still holds the
files it held at the freeze, and Lakelands has taken nothing new from
the source since. The row below is still the only one. **The list is
closed for `0.2.0` as of `c0b30a3f2e`.**

**Checked again on 2026-09-25, a second time before the `0.2.0` tag**,
against the source's mainline at `db4b2949e7`: the 113 commits after
`c0b30a3f2e` (284 after the freeze in all) were checked against all nine
roots, `{tools}` by its ten files. None of them touches a root, each
root still holds the files it held at the freeze, and Lakelands has
taken nothing new from the source since. The row below is still the only
one. **The list is closed for `0.2.0` as of `db4b2949e7`**; on the day of
the tag only the commits after it are checked, if the mainline has moved.

**Checked on 2026-09-26, the day of the `0.2.0` tag**, against the
source's mainline at `ca9364c420`: the 47 commits after `db4b2949e7`
(331 after the freeze in all), checked the same way. None of them
touches a root, and each root still holds the files it held at the
freeze. The row below is still the only one. **The list is closed for
`0.2.0` as of `ca9364c420`.**

**Checked on 2026-09-28, after the `0.3.0` tag**, against the source's
mainline at `035c01a349`: the 209 commits after `ca9364c420` (540 after
the freeze in all), checked the same way. Four of them touch a root, all
four in `{engine}`, and only its test fixture: the helper that places a
test's store in a temporary directory (`src/data_engine/tests/helpers.rs`,
never imported) and one unix-only development dependency in the crate's
manifest. They are the four rows after the first. None of them is a bug
fix in storage behaviour, and none applies: Lakelands took neither that
fixture nor the manifest. Every root, `{engine}` included, still holds
the files it held at the freeze. **The list is closed for `0.3.0` as of `035c01a349`.**

**Checked on 2026-09-29, before the `0.4.0` tag**, against the source's
mainline at `21384bf69f`: the 152 commits after `035c01a349` (692 after
the freeze in all), checked the same way. Two of them touch a root, and
they are the last two rows. The first moves 17 files of `{types}` (the
type definitions, the migration compatibility classes, the schema hash,
the verb store and its handlers), and three pieces cut from larger
files, into a new crate of the source's own for its shared type layer.
It changes only their import paths. `{types}` no longer holds those
files, so later checks follow them to their new crate. The second is a
fix in `{store}` and `{engine}`'s tests. Lakelands already holds its
behaviour and needs no port. Every other root still holds the files it
held at the freeze, apart from the one test file the second commit adds
to `{engine}`. **The list is closed for `0.4.0` as of `21384bf69f`.**

**Checked on 2026-09-30, before the `0.5.0` tag**, against the source's
mainline at `1c58d0fcbc`: the 39 commits after `21384bf69f` (731 after
the freeze in all). The check is the same as before, with the files the
first of the last two rows moved out of `{types}` followed into their
new crate, all of which was checked. No mainline commit touches a root.
Seven merges do show changes under the roots when compared with their
side parent. Each brings a branch up to a mainline that already held the
last two rows' commits. The roots are byte for byte what they were at
`21384bf69f`, and every root, the new crate included, holds the same
files. No row is added. **The list is closed for `0.5.0` as of
`1c58d0fcbc`.** If the mainline moves before the tag, only the commits
after it are checked.

**Checked on 2026-10-01, before the `0.6.0` tag**, against the source's
mainline at `ff72cdfed4`: the 105 commits after `1c58d0fcbc` (836 after
the freeze in all), checked the same way, the moved `{types}` files in
their new crate included. Two of them touch a root, and they are the last
two rows. The first fixes the writer of `{engine}` and `{store}` around a
lake that is deleted and created again while it is written; Lakelands has
no such deletion and no shared write queue, so it does not apply. The
second makes the `{pg}` engine's refusal of a transaction-mode pooler see
leaked temporary tables as well as a leaked `search_path`; Lakelands'
engine sets neither and probes a setting of its own, so it is not needed.
Every root holds the same files it held at `1c58d0fcbc`. **The list is
closed for `0.6.0` as of `ff72cdfed4`.**

**Checked on 2026-10-01, before the `0.6.1` tag**, against the source's
mainline at `8be8edf0d1`: the 28 commits after `ff72cdfed4` (864 after
the freeze in all), checked the same way, the moved `{types}` files in
their new crate included. Two of them touch a root, and they are the last
two rows. The first gives the reactive dispatch of `{types}` and
`{store}` a cause, so that a redelivered event keeps its run; Lakelands
carried no such dispatch, and a delivered change is already known by its
lake and position, so it is not needed. The second lets the store of
`{types}` and `{store}` put a failed work-queue item back to pending;
Lakelands has no work-queue types and no guard on their status, so it
does not apply. Every root holds the same files it held at `ff72cdfed4`.
**The list is closed for `0.6.1` as of `8be8edf0d1`.**

**Checked on 2026-10-01, before the `0.7.0` tag**, against the source's
mainline at `2ffca4d46d`: the 12 commits after `8be8edf0d1` (876 after
the freeze in all), checked the same way, the moved `{types}` files in
their new crate included. None of them touches a root. Two merges do
show changes under the roots when compared with their side parent; each
brings a branch up to a mainline that already held `3ab89a5271`, and
their changes under the roots are exactly that commit's. The roots are
byte for byte what they were at `8be8edf0d1`, and every root holds the
same files. No row is added. **The list is closed for `0.7.0` as of
`2ffca4d46d`.** If the mainline moves before the tag, only the commits
after it are checked.

**Checked again on 2026-10-02, the day of the `0.7.0` tag**, against the
source's mainline at `ce764acbd7`: the 15 commits after `2ffca4d46d` (891
after the freeze in all), checked the same way. None of them touches a
root, no merge shows a change under one, and every root holds the files
it held at `2ffca4d46d`. No row is added. **The list is closed for
`0.7.0` as of `ce764acbd7`.**

**Checked on 2026-10-02, for the `0.7.1` patch and before the `0.8.0`
tag**, against the source's mainline at `318aae2bdc`: the 112 commits
after `ce764acbd7` (1003 after the freeze in all), checked the same way.
The first 48, through `afbd5c361`, were checked for `0.7.1` (its release
PR says so), and none of them touches a root. Of the 64 after it, three
touch a root, and they are the last three rows. Each comes from the
source's effect and flow layers, not its storage core, and none applies:
- the first marks an effect whose dispatch was interrupted as an unknown
  delivery, and touches a root only in a test fixture of the tool layer;
- the second labels an effect's command for the operator's inbox;
- the third adds a check to the flow lint, which was never imported.

Twelve other mainline merges show changes under the roots when compared
with their side parent. Each brings a branch up to a mainline that
already held those rows, and by patch id each one's change is exactly
that of one of the three, or, for one merge, of the second and third
together. Every root holds the files
it held at `ce764acbd7`. **The list is closed for `0.8.0` as of
`318aae2bdc`.** If the mainline moves before the tag, only the commits
after it are checked.

| Source commit | Behaviour, in one line | Ported in, or not applicable and why |
|---|---|---|
| `5914084de9` | One German user-interface sentence in the capabilities module changes its form of address from formal to informal. | Not applicable: the capabilities module was not carried (cut in O2: there is no capability negotiation; see `docs/import-ledger.md`), and no storage behaviour changes: no event, no fold rule, no signature, no stored byte. |
| `c5fae3e073` | The engine's test fixture puts each test store on `/dev/shm` when the machine has it, so two long property tests stop waiting on a shared disk's fsyncs; the store still calls `sync_all`, and no test's coverage changes. | Not applicable: the fixture (`{engine}/src/data_engine/tests/helpers.rs`) was never imported, and no storage behaviour changes. Lakelands' engine tests make their own temporary directories. |
| `7971832d97` | The same fixture uses `/dev/shm` only when at least 256 MiB are free there, and names each store directory after the test process, so leftovers can be found by their owner; `nix` becomes a unix-only development dependency. | Not applicable: the same fixture, and a development dependency of a manifest that was not carried; no storage behaviour changes. |
| `afdc3fed00` | The fixture's free-room arithmetic widens to `u128`, so it is right where the platform's block counts are 32-bit and no linter reports a no-op conversion. | Not applicable: the same fixture; no storage behaviour changes. |
| `c9f8a0ad6e` | A new store on `/dev/shm` first removes the directories left by test processes that were killed by a signal. | Not applicable: the same fixture; no storage behaviour changes. |
| `5c9c476b11` | Seventeen files of the contract-and-types module move into a new crate of the source's own for its shared type layer. The old module re-exports them for the time being. Three pieces cut from larger files move with them: the verb and CRUD vocabulary with its mapping onto the write grant, the filter predicate, and the page cursor. Only import paths change. | Not applicable: no behaviour changes. No event, fold rule, signature or stored byte changes; the renamed files differ from their old selves in `use` lines alone. Lakelands' contract types are its own, written new in O1 (`lakelands-api`), and the crate the files now live in is outside the roots. Later checks follow the moved files. |
| `8a47557f5e` | A lexical search, which matches every searchable field at once, is refused and names the field when the caller may not read one of those fields: otherwise the question probes the field even though the hit masks it. A hybrid search whose lexical half is refused now answers with that refusal instead of falling back to its vector half. | Not needed: Lakelands already refuses. `rights::check_search` refuses a search in every mode, before either half runs, when the principal may not read one of the type's searchable fields or the searched vector field (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`). So a hybrid search never falls back to its vector half. The suite holds the lexical case (`searching_a_type_whose_searchable_field_is_unreadable_is_refused`). |
| `def9edb870` | A write made around a lake that is deleted and created again lands once or says why: a writer handle that no longer fronts the lake's files hands its queued window back unwritten to the handle that does, and a queued write's acknowledgement answers exactly once, the close's answer when it is dropped. | Not applicable: Lakelands deletes no lake and parks no lake's files, so no handle can outlive the files it writes. A lake has one writer, which holds its lease (`docs/decisions/008-one-writer-per-lake.md`), and an ingest window is taken from one caller's stream and answered through that caller's own channel (`lakelands-store/src/ingest.rs`); there is no queue shared between callers whose answers a close could drop. |
| `d3cb2ccf17` | The PostgreSQL engine's refusal of a transaction-mode pooler also probes a temporary table, since a pooler that learned to re-apply a client's `search_path` still lets one session read another's temporary tables, and the engine's replay of an earlier position builds such tables. | Not needed: Lakelands' PostgreSQL engine sets no `search_path` (every statement names its schema) and builds no temporary tables; an earlier position is read from the kept history rows. The session state a pooler could lose or leak is the writer lease, a session lock, and `connect::session_keeping_state` refuses before taking it, by a setting of its own (`lakelands.probe`) that the server does not report, so no pooler that re-applies reported settings restores it. |
| `14b12d63c7` | The reactive dispatch the store hands a trigger's worker carries a cause, the identity of the event that fired it (its lake and position on the lake plane), so a run id derived from it stays the same when the at-least-once feed delivers that event again. | Not needed: the dispatch structure was not carried; Lakelands' store hands out no dispatches and runs no walks. A trigger's firing is a `trigger_fired` event in the log, and the dedup key of a delivered change is already its lake and its position (`docs/decisions/004-the-change-feed-is-the-log.md`), so a consumer that derives a run from it gets the same one on every delivery. |
| `3ab89a5271` | The store grants one more write authority, used only by its own requeue verb: a work-queue item stored `failed` may be written back to `pending` on the version the caller read, and every other transition under that authority is refused by type. The flag that marked an enrichment write becomes a closed set of authorities. | Not applicable: Lakelands has no work-queue types, no guard that keeps a terminal status, and no enrichment write authority; the `DataBackend` trait the verb is added to was never imported. Which door may write a field is decided by its declared authority and the caller's grants (`docs/decisions/016-two-write-doors-by-origin.md`), and nothing in the store changes a status on its own. |
| `acbe9e4b50` | An effect whose dispatch was cut off when the source's daemon died is no longer freed for a re-run: it is reported as an unknown delivery, and only an effect its plugin declares repeatable runs again, since it may have reached the outside world and lost only its receipt. Under the roots, only a test fixture of the tool layer gains the new flag. | Not applicable: Lakelands has no effect daemon, no dispatch queue and no plugin declarations, and the tool layer's tests were not carried. A verb's external effect runs within its call or its acceptance. A retry with the same idempotency key answers the first outcome and runs nothing again (`docs/decisions/016-two-write-doors-by-origin.md`). |
| `98fb09c1db` | The command an effect is submitted with carries an optional label, the operator's name for it: a domain verb's name and the record it acts on. An inbox row about an unknown delivery then names that, not the handler every data verb shares. | Not applicable: the effect command and the inbox were not carried. Lakelands records a verb's call as `verb_called`, which names the verb and the record it acts on itself. |
| `d741fe684e` | The flow lint gains a check that warns, before a flow's first run, when a step files a record under a key an agent step produced with no confirm step between, since a retry could produce another key and file the record twice. | Not applicable: the flow lint (`{store}/src/flow_lint.rs`) was never imported, being a product feature (`docs/import-ledger.md`), and Lakelands has no flows. An idempotency key is the caller's own, and the same key answers the same outcome (`docs/decisions/016-two-write-doors-by-origin.md`). |
