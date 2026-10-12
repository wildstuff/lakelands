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

**Checked on 2026-10-03, for the `0.8.1` patch**, against the source's
mainline at `60eb52f851`. That covers the 26 commits after `318aae2bdc`,
1029 since the freeze in all, checked the same way.
- **One commit touches a root, and it is the last row.** `580a645406`
  reached the mainline through `60eb52f851`. It belongs to the source's
  own adapter onto a lake and changes no storage behaviour.
- **Three merges inside that branch** each bring it up to the mainline.
  Compared with their side parent, their change under the roots is
  exactly that commit's, by patch id.
- **Two mainline merges** show a change under the roots only when
  compared with their side parent. That parent branched before
  `318aae2bdc`, and the change is exactly `d741fe684e`'s, by patch id,
  which is already a row. Compared with their first parent, they change
  nothing under the roots.

Every root holds the files it held at `318aae2bdc`. **The list is
closed for `0.8.1` as of `60eb52f851`.**

**Checked on 2026-10-03, before the `0.9.0` tag**, against the source's
mainline at `ccbe5d73a3`. That covers the 62 commits after `60eb52f851`,
1091 since the freeze in all, checked the same way.
- **No commit touches a root.** The roots are byte for byte what they
  were at `60eb52f851`.
- **Four merges** show a change under the roots only when compared with
  their side parent: three on the mainline and one inside a branch. Each
  side parent branched before `580a645406` reached the mainline, and the
  change is exactly that commit's, by patch id, which is already a row.
  Compared with their first parent, they change nothing under the roots.

Every root holds the files it held at `60eb52f851`. No row is added.
**The list is closed for `0.9.0` as of `ccbe5d73a3`.** If the mainline
moves before the tag, only the commits after it are checked.

**Checked again on 2026-10-03, before the `0.9.0` tag**, against the
source's mainline at `c8979fff7a`: the mainline had moved, and the 4
commits after `ccbe5d73a3` (1095 after the freeze in all) were checked
the same way. None of them touches a root, neither of the two merges
among them shows a change under one against either parent, and every
root holds the files it held at `ccbe5d73a3`. No row is added. **The
list is closed for `0.9.0` as of `c8979fff7a`.**

**Checked a third time on 2026-10-03, before the `0.9.0` tag**, against
the source's mainline at `d628b79a19`: the 19 commits after
`c8979fff7a` (1114 after the freeze in all), checked the same way.
- **One commit touches a root, and it is the last row.** `3aa65077c6`
  reached the mainline through `31f2e71423`. It makes one function of
  `{store}` public for the source's own adapter onto a lake and changes
  no storage behaviour.
- **Two further merges**, one on the mainline and one inside a branch,
  show a change under the roots only against their side parent, which
  branched before that commit reached the mainline. By patch id the
  change is exactly that commit's.

Every root holds the files it held at `c8979fff7a`. **The list is
closed for `0.9.0` as of `d628b79a19`.**

**Checked on 2026-10-03, minutes before the `0.9.0` tag**, against the
source's mainline at `3918187ec1`: the 2 commits after `d628b79a19`
(1116 after the freeze in all), checked the same way. Neither touches a
root. One of them is a merge that shows a change under the roots only
against its side parent, and by patch id it is exactly `3aa65077c6`'s,
which is already a row. Every root holds the files it held at
`d628b79a19`. No row is added. **The list is closed for `0.9.0` as of
`3918187ec1`**, the commit the tag's message names.

**Checked on 2026-10-03, before the `0.9.1` tag**, against the source's
mainline at `f1747cedea`: the 22 commits after `3918187ec1` (1138 after
the freeze in all), checked the same way.
- **One commit touches a root, and it is the last row.** `e766e2b5ca`
  reached the mainline through `478943fa9c`. It adds one variant to the
  error type among the moved `{types}` files in their new crate, six
  lines, so that the source's adapter gives a store limit its own kind
  of refusal. It changes no storage behaviour.
- **None of the 11 merges** among them adds anything of its own under
  the roots: what each shows against one parent is what the other
  parent brought since their merge base, and by patch id that is this
  commit's change or `3aa65077c6`'s, which is already a row.

Every root holds the files it held at `3918187ec1`. **The list is
closed for `0.9.1` as of `f1747cedea`**, the commit the tag's message
names.

**Checked on 2026-10-03, after the `0.9.1` tag**, against the source's
mainline at `c492aa07a6`: the 5 commits after `f1747cedea` (1143 after
the freeze in all), checked the same way. None of them touches a root,
neither of the two merges among them shows a change under one against
either parent, and every root holds the files it held at `f1747cedea`.
No row is added. The next check starts after `c492aa07a6`.

**Checked on 2026-10-06, before the `0.10.0` tag**, against the
source's mainline at `8e069a5fac`. The mainline is the source's
`develop`, the branch every check above followed; its `main` is an older
branch and was not checked. The bare repository was fetched to that
commit for the check. That covers the 556 commits after `c492aa07a6`, 188
of them on the first-parent line, 1699 since the freeze in all.
- **Each root still holds the files it held at `c492aa07a6`.** The moved
  `{types}` files in their new crate are included. Five files are added,
  none is removed or moved: a seed list and a type of the source's own,
  two engine tests, and the source store's read judge.
- **26 commits touch a root, and they are the last 26 rows.** The files
  they change are exactly the 122 files that change under the roots
  across the range.
- **None of the 223 merges** adds a change of its own under the roots
  (a combined diff against both parents is empty).
- **One applies and is not held: lakelands#652.** The source closes a
  computed field whose reads cannot be told. In Lakelands, a derived
  expression that uses `changed()` or `previous()` is completed with no
  `depends_on`, so it is read at its own grant and not at its inputs'.
  The other 25 are held, test-only, or not applicable, each with its
  reason in its row.
- **Two holes of Lakelands' own were found beside them**, while the
  source's read judge was being judged. Neither is a port of a source
  commit. In a pattern read, a refusal names a type to a reader it is
  not shown: by an order or a measure over a path on that type
  (lakelands#653), and by a step over a relation that the type derives
  under a name the reader is shown (lakelands#654). Both are probed on
  develop, and their probes are in the issues.

**The list is closed for `0.10.0` as of `8e069a5fac`**, once
lakelands#652 is settled. If the mainline moves before the tag, only the
commits after it are checked.

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
| `580a645406` | The delete guard's rule for which relations hold a record alive moves into a public function, beside a public refusal helper, so that an engine counting its own referrers asks the same question and refuses in the same words. Two ownership functions become public. The field-grant set gains an accessor, and the error type gains a refusal for an operation the source's lake adapter does not carry. | Not applicable: the source exposes its own guard and helpers to its adapter. No event, fold rule, signature of ours or stored byte changes, and the guard's rule and wording stay as they were. |
| `3aa65077c6` | The function of `{store}` that describes a type as its declaration states it becomes public, and `{engine}` re-exports it, so that the source's adapter onto a lake can answer a description. Two `use` lines change; the function itself does not. | Not applicable: only a visibility changes, for the source's own adapter. No event, fold rule, signature or stored byte changes. Lakelands answers a type's description through `describe_type`, which is a door of its contract already. |
| `e766e2b5ca` | The error type among the moved `{types}` files gains one variant, for a limit of the store that was reached: a graph measure past the lake's node limit, or a chain of reactive writes past its depth. The source's adapter onto a lake maps those two refusals to it. Six added lines; nothing else under a root changes. | Not applicable: the variant is the source's own name for two refusals at its adapter. No event, fold rule, signature or stored byte changes. Lakelands answers both in its contract already, each with its own kind: `too_large` for a measure past the node limit and `too_deep` for a reactive chain past its depth. |
| `1f7b2d0571` | The source's adapter onto a lake serves a whole source's retraction through the lake's purge preflight. It refuses the purge itself where records outside it still refer in, or where its caller sees only part of what the source brought, and reads the preflight again when the lake moved. It also hands a search over several fields to the lake's fusion, and reads a graph measure in one page of the node limit. | Already held. A preflight's id is stale once the lake moved (`a_preflight_appends_nothing_and_a_purge_that_repeats_its_id_is_stale_once_the_lake_moved`). The records that refer in are named, never a refusal (`a_purge_names_the_records_outside_it_that_refer_into_it`, record 039); the stricter refusal is the host's, which is what the source now does. A purge is the host's act on its store, not a principal's (`a_restricted_record_is_purged_too`). A measure is read whole in one page (`a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes`), and a search fuses its rankings (`a_search_fuses_its_rankings_each_by_its_weight`). The source's owner gate and inbound guard were never imported. |
| `a1c544e6ab` | A row of the source's inbox names whose it is, and the model's door refuses every row addressed to the operator; under a root only a test fixture's constructor is renamed. | Not applicable: Lakelands has no inbox, and the tool layer's channel reads were never imported (only the ten `{tools}` files were). |
| `3dd1d81134` | The source's expression parser takes Lakelands' spelling: `=`, `!=`, `and`, `or`, `not` binding between `and` and the comparisons, single-quoted text with `''`, and a keyword refused as a value. `==`, `&&`, `\|\|`, `!x` and double-quoted text are each refused, naming the spelling to write. | Already held: the grammar is Lakelands' own (`lakelands-api/src/expr/parse.rs`: the precedence, `''` and keywords). Each old spelling is refused there and none is read as something else, though the refusal does not name the spelling to write. |
| `bde4de53aa` | A computed field is closed to every caller kept from a field its expression reads, down the chain. Where the reads cannot be told (an expression that does not parse, a function no one resolved, a name that is no field), the field counts as reading every field of its type; a setting, the read's date and a literal count for nothing. | The rule is held (`rights::built_from`; `Ontology::complete` fills `depends_on`; `a_computed_field_is_read_no_further_than_what_it_is_computed_from`, `a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field`), and the declaration refuses an expression that does not parse, an undeclared function and a name that is no field. **One case is not held: lakelands#652.** A derived expression that uses `changed()` or `previous()` is completed with no `depends_on`, so it is read at its own grant; its probe is in the issue. |
| `a5261b8fbf` | The source's adapter tells the lake each computed field's reads as `depends_on`, holds a computed field sensitive with its inputs, which keeps it out of the text index, and shows the inherited marks in its type read. | Already held: the store fills `depends_on` itself (`Ontology::complete`), a search never searches a field its reader may not (`rights::searched_fields`), a masked input is probed and refused (`rights::check_probe`), and `describe` shows the completed list (`a_computed_field_is_read_no_further_than_what_it_is_computed_from`). The adapter is the source's own. |
| `b3e8186ee6` | Tests only: the computed-field rule is pinned by unit tests: the chain followed to its end, a cycle ending it, the field closed where its reads are unknown, a setting, the date and a literal adding nothing, a type pinned again read as it now stands. | Held, except where the reads are unknown (lakelands#652): a cycle is refused at declare (`computed_fields_that_read_each_other_in_a_cycle_are_refused`), a setting adds nothing (`a_restricted_setting_stays_what_the_lake_computes_with`), and a function declared again restricts by its inputs as they now stand. |
| `39f8de2135` | `sensitive` and the source's `internal` mark are always inherited from what a field reads; its model-scope mark is inherited unless the computed field declares its own, which then stands for the field and what is computed from it. | Not applicable: Lakelands has no model scope and no `internal` mark (`docs/import-ledger.md`: the platform's vocabulary), and no override: a grant that names a derived field is still lowered by its inputs. |
| `aa2feb8e32` | A stored key the type does not name (an old name in a read as of before a rename, or a dropped field's leftover value) is withheld from every caller the source restricts; an old name is judged as the field it became, and the mask strips by the record's keys. | Already held: an answer is built from declared fields only (`rights::visible`), a probe of an undeclared name is refused (`rights::check_probe`), an as-of read uses the type as it stood then, with grants followed through renames (`a_grant_holds_a_field_under_every_name_it_has_borne`), and a migration drops a removed field's values (`the_preflight_names_every_dropped_field_and_an_empty_type_migrates_cleanly`). |
| `4464ad2d64` | Tests only: the rule of the row before is pinned with the source's rights switch off, and its mask hands back the stored bytes untouched where it strips nothing. | The first is held: Lakelands has no switch, so the probe rule always holds. The second is not applicable: Lakelands has no byte-level mask; an answer is built as a record. |
| `1afe0a7ed3` | A refined type's key and projected columns, named in its `normalize` declaration, count as named and stay open, after the row before closed them. | Not applicable: refined types and `normalize` were never imported. |
| `0adfde0316` | Tests only (written red first): a read as of a position, through every door, is judged by marks set after it: a field marked sensitive, a type made owner-scoped and the reverse, the root level, and the refusal's kind. | Held where it maps: grants are the host's as they are now at every door, followed through the renames (`a_grant_holds_a_field_under_every_name_it_has_borne`). `sensitive` read as of then only turns an inherited `none` into `masked`, never shows a value. Owner scope and the root level do not exist. A retention hold is judged as it stood then, by decision (`a_restricted_record_is_withheld_but_kept`). |
| `88256e9366` | Every read door of the source's store asks one judge, which reads the marks (a sensitive field, an owner-scoped type) as they stand today, not as of the read's position; before, aggregates, searches, views and graph doors judged by the marks of then. | Already held: every read door opens one reader with one principal, its grants followed through the head's renames, and asks `lakelands_core::rights` (`a_grant_holds_a_field_under_every_name_it_has_borne`). Owner scope was cut. A retention hold as of before is decided (`a_restricted_record_is_withheld_but_kept`). |
| `da527aeffd` | Tests only: a scan fails where any read door of the source's store calls a guard outside its judge. | Not applicable: it pins the source's own split between guards and a judge. The guards were read and cut; `rights` is Lakelands' one place for the rule. |
| `a4f6c0a7d9` | A lint allowance goes: the judge's neighbour check takes seven arguments. | Not applicable: no behaviour changes. |
| `8ce537ab9a` | A filter or an aggregate over neighbours is refused where the relation reaches a type the caller may not read; the adapter's subgraph gives no fields of such a type, and a field match skips it. | Already held: a pattern step that reaches a type its caller is not shown is refused before a record is read, a step's `where` and row paths are probed on the reached type, and a subgraph's nodes go out through `rights::visible` (`a_type_made_of_another_is_shown_only_with_it`). The adapter and the field match were never imported. Beside it, an order or a measure over a path on a type the caller is not shown names that type (lakelands#653). |
| `f2a18c24b2` | Tests only: the folding doors of the source's root level refuse a closed neighbour type too. | Held for the behaviour: a step over a relation whose owner its caller is not shown is "no relation" (`a_type_made_of_another_is_shown_only_with_it`). The root level does not exist. Beside it, a step over a relation that a hidden type derives under a shown name names that type (lakelands#654). |
| `4e7f2385fb` | A walk (neighbours, transitive walks, a subgraph, a path, a degree, components) never enters a type closed to its caller: no edge leads into it, so an open node reached only through it is not reached either. | Already held: a walk asks whether every far endpoint is visible before it follows a link, and a node of a type not shown is never visible; unit rows in `lakelands-core/src/graph/` (`a_hidden_record_and_its_links_are_left_out` and the path and measure rows). The source's push-down was read and cut. |
| `723a498781` | The walk's edge source of the row before needs no visibility of its own, since no edge leads into a closed type. | Not applicable: a refactor of the source's own; no behaviour changes. |
| `8574017244` | Tests only: a conformance check of the source declares its type before the instant it asks about, where both fell in one millisecond. | Not applicable: Lakelands reads as of a position, and its check of `position_at` takes instants from the log at least two milliseconds apart. |
| `1bd4c2e878` | German operator text changes the source's tenant noun in labels, notices and fault rows. | Not applicable: wording of modules that were cut or never imported; Lakelands is English. |
| `559b3a42e7` | The source's German system card titles itself like the section under it. | Not applicable: the card was never imported. |
| `4960d0fa03` | The source keeps one seed of its learning and capability types and builds every one of its tenants from it; its shared mirror writes a row alone. | Not applicable: the source's own seeds and mirror. A seed declared again unchanged appends nothing in Lakelands anyway (`a_declaration_that_is_the_one_in_force_appends_nothing`). |
| `3ad1139415` | Tests only: the SQLite engine's test of an outside writer stops on writes that waited, not on refusals alone. | Not applicable as code (never imported). The behaviour it pins is held: every SQLite connection waits out a busy timeout and every projection write begins `IMMEDIATE` (`lakelands-engine-sqlite/tests/busy_fold.rs`). |
| `0516dad4da` | A door of the source that cannot express a mark (a component's re-pin, a schema delta, the operator's create-type tool) lays its words over the stored type and no longer strips the owner field, `sensitive` or `internal`. | Not applicable: every declaring door of Lakelands takes the declaration whole and strict (record 028), and WIT and bundles carry `sensitive`. Those narrow doors were never imported. |
| `0e08100bc1` | Tests only: frozen test files of the tool layer are held at their size by a test-local helper. | Not applicable: those files are not among the ten `{tools}` files; no behaviour is pinned. |
| `8d76bd000f` | The source's verb door refuses a call that names an input the verb does not take, naming it and what the verb takes, where it ran the call and dropped the value; a comparison over a value that is no number says so. | Already held: a payload field the verb does not declare is refused as invalid, naming it (`a_low_verb_runs_and_a_higher_one_is_proposed`), and a payload's values are typed, so a value that is no number is refused before anything is judged. |
