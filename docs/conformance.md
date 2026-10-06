# Conformance

An engine **supports Lakelands** when the conformance suite passes on it,
every check, with nothing skipped. There is no other definition: no
feature matrix, no partial level, no engine-specific allowance. A
behaviour Lakelands promises is claimed once, in the suite, and every
engine is held to the same claim.

## What the suite is

The suite is a library in `lakelands-core` (`lakelands_core::conformance`)
with two halves:

- **Through the doors.** Checks that open a lake as a `LakeStore` and
  talk to it through its `Catalog`, the way every caller does. They check
  meaning: that a write touches only its fields, that a read at an earlier
  position sees the lake as it was there, that a decimal stays exact,
  that a field a principal may not write refuses the whole write. A check
  that could only pass on one engine's storage cannot be written here,
  because a check never reaches past the contract.
- **Through the engine seam.** Checks that call the engine traits
  directly (`lakelands_core::conformance::engine`), for what the store
  relies on and cannot see through its doors: that an event's bytes come
  back unchanged, that a stale or held lease is refused, that an append
  is whole or nothing, that a view at a position reads the states valid
  there. `docs/seams.md` states these promises and says which check holds
  each one.

Each check is one function whose name is a sentence stating what must
hold, one line in a registry, and one sentence naming the decision
record or specification the rule comes from. The list at the end of this
page is generated from the two registries.

## Running it on an engine

An engine crate runs the suite from two test files:

- `tests/conformance.rs` implements `Harness`: `name()`,
  `fresh(seams)`, a brand-new, empty lake opened as a `LakeStore` over
  the engine, one per check, so that no check sees another's writes, and
  `reopener()`, which closes a lake it handed out and opens it again for
  writing with the seams a check names, as a host's next process would.
  A check that meets the lake's next writer (a host vocabulary that
  grew) is given it. And `scratch()`, a directory the engine's test
  chooses: a check that writes files (an export) is given the
  subdirectory named after it and removes what it wrote, so the suite
  itself reads no environment. Two more are optional. `meter()` counts
  the process's heap, usually through the test binary's global
  allocator; the streaming check uses it to show that ingest, the feed
  and an export hold no more memory at four times a lake's size. A
  harness without one runs that check without the memory claim.
  `large_lake()` sets that check's first size, `LARGE_LAKE` (2 000
  records) by default; a long run by hand asks for 25 000.
  The runner hands it the seams the lake binds besides the engine: the
  suite's function backing and its verb executor, and its embedding
  backend for the checks that need one. The harness adds its engine's opener and nothing else. It calls `run_all`
  and fails unless the `Report` is green. Every lake has the default
  limits of `Location`; the checks read limits from the suite's fixture,
  never from the engine.
- `tests/engine_seam.rs` implements `EngineHarness`: `name()`, `fresh()`
  (a brand-new, empty lake opened for writing as a `LakeEngine`), and
  `reopen(lake, mode)` (another handle onto the same lake, as a second
  process would open it). It calls `run_engine_checks`.

`crates/lakelands-engine-mem/tests/` holds both for the in-memory engine,
and is the example to copy. A run never stops at the first failure, and
a check that panics is reported as failed, so one report lists every
check that did not hold, each with a message that says what was done,
what came back and what the rule is.

```sh
scripts/conformance.sh mem      # one engine
scripts/conformance.sh          # every required engine
```

The script runs under bash 3.2 and later, so the system shell of macOS
runs it as well as a current bash. It needs `cargo-nextest`.

`conformance/roster` lists the engines. A `required` engine runs both
halves on every pull request, as its own CI job; a `deferred` engine has
a crate but is not imported yet, and a required engine never becomes
deferred again.

**An engine that needs a server.** The PostgreSQL engine's suite runs
against a live database. In CI the `conformance (pg)` job starts a
throwaway PostgreSQL as a service container and hands its URL to the
tests as `LAKELANDS_TEST_PG_URL`. Only the harness in
`crates/lakelands-engine-pg/tests/` reads it; the engine itself reads no
environment (`docs/decisions/005-no-environment-reads.md`), so the
harness passes it on as the `url` of `EngineConfig::Postgres`, with TLS
disabled because the container serves none. Each check creates its own
schema and the harness drops it afterwards, so one database serves the
whole suite. The container runs PostgreSQL 16, the oldest version the
engine supports (`docs/decisions/038-a-lake-in-postgresql.md`). The
roster names `pg` required, so the job runs on every pull request.
Without the variable, the
pg tests say so and stand down, so a workspace test run on a machine
without PostgreSQL stays green; `scripts/conformance.sh pg` refuses to
run without it, so the suite is never skipped there. The workspace job
starts no server, since only the pg engine needs one and its job already
has it, so `scripts/conformance.sh pg` runs every test file in
`crates/lakelands-engine-pg/tests/`, by that one rule, not only the
suite. To run the same tests locally:

```sh
docker run --rm -d --name lakelands-pg -p 5432:5432 \
  -e POSTGRES_USER=lakelands -e POSTGRES_PASSWORD=lakelands -e POSTGRES_DB=lakelands \
  postgres:16
LAKELANDS_TEST_PG_URL=postgres://lakelands:lakelands@localhost:5432/lakelands \
  scripts/conformance.sh pg
```

## What a red check means

A red check is a defect, in the engine or in the claim, and it is fixed
in one of those two places:

- **The engine is wrong.** It is fixed in the engine. There is no list
  of checks an engine may fail, and no switch that skips one.
- **The claim is wrong.** The check says something Lakelands should not
  promise, or says it ambiguously. It is corrected in the suite, with the
  specification or decision record it cites, in the same pull request,
  and it then holds for every engine.

A check is never made to pass by teaching it about one engine.

## Adding a check

A behaviour claim comes with a check that fails without the change it
describes (`CONTRIBUTING.md`). A check that every engine must honour
belongs in the suite, not beside one engine:

1. Write the check in the file of its subject: in
   `crates/lakelands-core/src/conformance/checks/` for a check through the
   doors, in `…/conformance/engine/` for one through the seam. Its name is
   a sentence in snake_case stating what must hold; its message says what
   was done, what came back and what the rule is.
2. Turn it into a constant with one `check!` (or `engine_check!`) line,
   whose sentence names the rule's source: a decision record's number, a
   section of `docs/log-format.md` or `docs/seams.md`.
3. Add the constant to the registry (`checks::ALL` or `ENGINE_CHECKS`).
4. Show it red: once against the behaviour it forbids, then green.
5. Run `scripts/conformance-page.py --write` and commit this page with
   it. CI runs `scripts/conformance-page.py --check`, which fails while
   the list below differs from the registries.

## The checks

<!-- The list below is generated by scripts/conformance-page.py from the registries; do not edit it by hand. -->

### Through the doors

**fields**

- `a_rewrite_by_one_writer_leaves_another_writers_fields_alone` — 012: a patch is merged per field, so a writer that sends only its own fields cannot erase anyone else's
- `changing_an_input_clears_a_dependent_field_declared_clear` — 012: with on_input_change clear, the fold clears the dependent field in the same event that changes its input
- `changing_an_input_keeps_a_dependent_field_declared_keep_and_shows_it_older` — 012 and 010: with on_input_change keep, the value stays and its provenance shows it is older than its input
- `a_patch_with_a_field_the_principal_may_not_write_is_refused_whole` — 006 and 012: a write carrying a field its principal may not write is refused whole, never applied in part
- `a_field_absent_from_a_patch_is_unchanged_and_null_clears_it` — 012 and the log format: an upsert is a sparse patch; an absent field is unchanged and null clears the field
- `a_patch_with_an_undeclared_field_is_refused_whole` — 015: a field the lake's map does not declare is refused as invalid, and nothing of the patch is written
- `a_write_breaking_two_record_checks_is_refused_by_the_earlier` — 016, extended (lakelands#338): a record write that breaks two adjacent record checks is refused by the earlier: the type before an empty patch, the key before the fields, field by field in name order with the door before the grant, the grant before the value, the value before the key fill, the key's agreement before a reference, a reference before the version, the version before a required field; a retry answers its first outcome before any of them

**positions**

- `every_reply_carries_the_position_it_reflects` — 009: every answer to a read or a write carries the log position it reflects, so what a caller saw can be read again
- `a_read_as_of_a_write_sees_that_write_and_nothing_later` — 009: a read pinned with as_of answers the lake as it was at that position, not the head
- `a_read_as_of_the_position_before_a_write_does_not_see_it` — 009: the position a write answers is the first from which it is visible; one earlier, it is not
- `retiring_a_record_does_not_erase_its_history` — the log format: a retired record is invisible at and after its retirement, and its history is kept
- `a_read_as_of_a_position_past_the_head_is_refused_as_out_of_range` — 015: a position that is not in the lake's log is refused as out of range, naming the head
- `position_at_answers_the_last_position_at_or_before_an_instant` — 009 and log-format §1.3: position_at answers, at each event's known_at, the last event at or before it; between two instants the earlier one's last; before the first event none; past the head the head; and a read as of the answer sees the lake as it stood then

**versions**

- `versions_start_at_one_and_grow_by_one_with_every_change` — the glossary and the log format: a version starts at 1 and grows by one per event that changes the record, retirement included
- `a_write_expecting_a_superseded_version_is_refused_as_a_conflict` — 015: a write naming a version the record has moved past fails as a conflict, and nothing of it lands
- `a_verb_called_at_a_stale_version_is_a_conflict_before_its_requires_is_judged` — 015 and 024, extended: a verb's call compares the expected version right after its record is found, so a call at a stale version is a conflict, never a precondition judged on a state the caller has not seen
- `a_retry_with_the_same_idempotency_key_answers_the_first_outcome_and_appends_nothing` — 016 and the log format: one idempotency key from one principal is one write in the whole lake; a proposal made with a key carries it, and its retry answers the proposal and appends nothing
- `a_write_expecting_the_version_before_a_retirement_is_a_conflict` — the glossary, the log format §2.3 and 015: a retired key keeps its count, so `expected_version` is compared with the retired record's version, a match re-creates it, and an older one is a conflict reporting that version

**paging**

- `paging_through_find_visits_every_record_exactly_once` — the read contract: a cursor continues where its page ended, so a page set is a total order with no repeat and no gap
- `a_page_answers_its_total_and_an_offset_jumps_to_the_walks_slice` — 045 and 029: `total` answers as many records as a full walk finds, for a filter the engine applies and one only the store evaluates, a view and a reader without `restricted`, the same on every page of a cursor walk while another writer writes; `offset` answers the full walk's slice from it, by key, by a field and by a field with an order index, a view too, and its cursor continues the walk; `offset` beside `cursor`, `after` or `aggregate`, and `total` beside `aggregate`, are refused as invalid, and an offset past `max_offset` as too_large naming it
- `records_without_the_sort_field_are_listed_last_in_either_direction` — the read contract: records without a value for a sort key sort last in either direction; ordering never drops them

**latest**

- `find_ordered_by_latest_lists_records_by_their_last_event` — 029 and seams §scan: `find` ordered by `latest` lists records by the position of the event that made each one's state: an upsert, an ingest and a reference field move a record, a link record moves only itself, a retired record leaves; ascending is the reverse, a cursor continues it, a filter the store evaluates keeps it, and a read at an earlier position answers the order as it was then

**decimals**

- `a_decimal_reads_back_exactly_as_it_was_written` — the log format: a decimal is a string of digits kept as written, trailing zeros and all, beyond any float's precision
- `a_sum_over_decimals_is_exact` — the engine seam: sum is exact for decimals, so 0.10 + 0.20 + 0.30 is 0.60 and not a float's approximation
- `a_scaled_decimal_keeps_its_scale_when_derived_and_summed` — 011: a value is kept at its value type's scale, so a derived decimal is rounded to it before it is stored, and a sum over a scaled field answers at that scale
- `a_sum_reads_at_the_largest_scale_among_its_values` — 011: a sum over a field with no declared scale reads at the largest scale among its values, a zero's included, whatever order their keys give them: 5 and 0.00 are 5.00, and 1.10, -1.10 and 6 are 6.00 with an average of 2.00, through `find`'s aggregate and through a view

**derived fields**

- `a_derived_field_is_computed_by_the_fold_on_every_write` — the ontology and the log format: the fold computes a derived field from its expression in the event that changes its inputs
- `a_derived_field_written_through_a_door_is_refused_as_the_wrong_door` — 015 and the ontology: no door writes a derived field; a patch carrying one is refused whole as the wrong door
- `a_derived_field_added_to_a_live_type_is_computed_on_later_writes` — the ontology: a derived field added with add_field is kept by the fold for every write after it, like one declared with the type
- `a_derived_field_accepts_no_value_from_a_source_or_a_verb_either` — 015 and 016: a derived field is the fold's alone; a source record or a verb that would set it is refused whole, not only an upsert
- `a_derived_field_reading_a_function_is_recomputed_when_the_function_changes` — 022: a derived field that names a function is computed by that function's declaration in force; a new version of the function recomputes it and names that version as its procedure

**backings**

- `a_function_of_a_backing_kind_nobody_bound_is_refused_at_declare` — 022, extended: a function computed outside the store is declared only where the host bound a backing of its kind; otherwise the declaration is refused as unbound, naming the kind, after its own checks (028, extended)
- `a_backed_field_is_filled_from_the_output_the_store_records` — 022, extended, and the log format: the store calls the backing after the write, records the answer as output_recorded naming the field, and the write answers the position the filled field is visible from
- `the_same_input_is_never_sent_to_a_backing_twice` — 002 and 022, extended: an output recorded for a call is read back for every later call with the same input, across records and after the input changed and came back
- `a_failing_backing_clears_the_field_and_records_the_failure` — 022 and 010, extended: a failure is recorded as {"error": …}, clears the field it fills, puts the reason in the field's metadata, and is not retried for the same input
- `a_rebuild_reads_recorded_outputs_back_without_calling_the_backing` — 001 and 002: a rebuild folds the recorded outputs; with the backing refusing every call, the rebuilt field reads as before and no call is made
- `a_filled_field_recomputes_what_reads_it_and_fires_its_triggers` — 012 and 022, extended: the field an output fills is changed through the fold's ordinary path; a derived field reading it is recomputed and a trigger on it fires, caused by the output
- `computed_fields_that_read_each_other_in_a_cycle_are_refused` — 022, extended: a type's computed fields, derived and function-computed alike, read one another without a cycle, so a chain of fills ends; a cycle is refused at declare, naming its fields
- `a_second_record_with_the_same_input_has_its_derived_fields_right` — 022, extended: a field filled from an output the lake already holds is a change of that field; the derived fields reading it are recomputed in the same event
- `a_backed_field_reading_another_waits_for_its_output` — 022, extended: computed fields settle in the order of what they read; a backed field whose input is not computed yet is not called, so no output is recorded under an input that was never meant
- `a_redeclaration_retries_a_failed_call_once_and_the_later_output_holds` — 022, extended: a settle forced by `recompute` calls again, once, for an input whose recorded output is a failure, for a function backing as for an embedding; the later output holds for every record with that input

**views**

- `a_view_groups_its_source_and_sums_it_exactly` — the ontology: a view type is computed from its source's records, one record per group, measures exact for decimals
- `every_door_that_writes_or_holds_a_record_refuses_a_view_alike` — 018, extended (lakelands#370): upsert, retire, link, unlink, ingest, propose, restrict and unrestrict each refuse a view as invalid naming it, as upsert does, before the grants; declaring a verb on a record of a view is refused naming both, while a verb on the view's type that reaches outside is taken
- `an_ungrouped_view_holds_one_record_keyed_all_even_over_none` — 018, extended: a view without `group_by` holds exactly one record, keyed `all`, the totals over every source record its `where` keeps, even when it keeps none (a count and a sum of none are 0, a minimum null); `get` answers it by that key and no other, and a read's parameters and a merged source measure into the same record
- `a_view_is_the_same_whether_declared_before_or_after_its_source_was_written` — the ontology: a view is a function of its source's state, so how the source got there cannot change a byte of it
- `a_view_drops_a_group_whose_last_member_is_retired` — the ontology: a view holds a record per group that has members; retiring a group's last member removes the group
- `a_view_over_an_ingested_source_is_current_once_the_ingest_is_committed` — 016 and the ontology: both write doors are folded by the same rules, so a view over ingested records is kept like any other
- `a_pattern_started_on_a_view_holds_the_records_find_returns` — 018 and 026: a view is read like any other type, so a pattern started on it by key, by `where` or with neither has as its graph's nodes exactly the records `find` returns for that selection; a key with no group has none
- `a_walk_over_a_derived_relation_reaches_a_views_records_both_ways` — 018, 026 and 044: a derived relation between a stored type and a view connects their records by matching values like any other, so a walk out from the stored type reaches the view's records, a step's `where` keeps those of them it holds for, and a walk in from the view reaches the stored ones
- `a_view_field_that_asks_to_be_searched_or_embedded_is_refused` — 018: a view is not searched, so `declare` refuses a view field that is searchable, carries an embedding or is a vector, naming the view and the field
- `a_trigger_or_object_set_on_a_view_is_refused` — 018, extended: a view is never written, so `declare` and `apply` refuse a trigger on a view, which no write would fire, and an object set over one, which holds stored records, naming both; nothing is appended
- `a_relation_on_a_view_is_derived_or_refused` — 018, extended, and 035: a view's records are computed and never written, so `declare` and `apply` refuse a relation on a view carried by link records or by a `via` field, which could never hold an edge, naming the view and the relation; a relation derived from matching values is taken
- `a_view_reading_a_field_its_source_lacks_is_refused` — 028 and 018: `declare` and `apply` refuse a view whose `where` names, or whose measures aggregate, a field its source or a merged source does not have, naming the view, the source and the field; a name outside an aggregate is an earlier measure or a grouping field, and is taken
- `a_search_on_a_view_is_refused_naming_it` — 018: a view's records are computed on read and no index holds them, so a lexical, vector or hybrid search on a view is refused as invalid, naming the view, never answered empty
- `a_view_closing_a_cycle_over_source_is_refused_naming_the_chain` — 018 and 028: a view may not read itself, directly or through other views, so `declare` refuses a view whose `source` closes a cycle of length 1, 2 or 3, and a set applied whole whose new views close one among themselves, naming the chain; the views keep what they read
- `a_view_whose_measure_reads_another_is_described_to_its_readers` — 024 and 018: `describe` answers a view's definition to a principal who reads every field of its source that it names; a measure that reads an earlier measure by name names no field, so it keeps the definition from no one, while a name inside an aggregate is the source's field, even where a measure has the same name
- `a_view_declares_exactly_the_fields_it_computes` — 018 and 028: a read answers declared fields only, so `declare` refuses a view whose `group_by` field or measure has no declared field, or which declares a field nothing computes, naming it
- `a_view_record_is_keyed_by_its_group_by_fields` — 018 and the log format §2.3: a view record's key is made from its `group_by` fields as any record key is, each value with `%` and `:` escaped, joined by `:` in declared order

**view params**

- `the_same_view_read_at_two_anchors_answers_two_windows` — 018: a view's parameters are bound by the read, so the same view read with two anchors answers two windows, by `find` and by `get`, each byte for byte the same on every engine; a date alone is midnight UTC
- `a_missing_unknown_or_wrongly_kinded_parameter_is_refused_naming_it` — 018 and 015: a read of a view that leaves a parameter out, names one the view does not declare, or gives a value not of its kind is refused as invalid, naming the parameter; a type that is not a view takes none
- `param_outside_a_views_where_and_measures_is_refused` — 017 and 018: `param()` is read only in a view's `where` and `measures`; in a read's `where`, a pattern's `where` or a derived field it is refused as invalid, naming the function
- `a_view_declaring_its_parameters_badly_is_refused` — 028 and 018: `declare` refuses a view whose `param()` names no declared parameter, which declares a parameter nothing reads, one twice, or one of a kind other than text, number, decimal, boolean or date_time, naming it
- `a_pattern_started_on_a_view_binds_its_parameters` — 018 and 026: a pattern that starts on a view binds its parameters with the start, by key and by `where`, and its nodes are the window `find` answers; a `degree` started on it by key answers that key's row of the whole view's; a start that leaves one out is refused, naming it
- `a_cursor_continues_only_with_the_parameters_it_was_handed_out_for` — 018 and 029: a page of a view continues only with the parameters its first page was read with; with others it is refused, since it would continue a different answer
- `a_view_with_parameters_is_written_in_log_format_8` — log format §6 and 018: the event that declares a view with parameters is written in format 8, as is every event after it; a view without them leaves the lake's format where it was
- `a_step_arriving_at_a_view_with_parameters_is_refused_before_reading` — 018 and 029: a view's parameters are bound only by a read that names it, so a pattern (rows or a measure) whose step arrives at a view with parameters other than its start is refused as invalid before any record is read, naming the view and the relation, the same with no records and with some; a pattern started on that view with its parameters, stepping out, is answered

**view merge**

- `a_view_merges_its_sources_on_their_key_and_keeps_a_key_only_one_has` — 018, extended: a view's merged sources meet its own where their keys are identical; every key any source has is a record, a source that lacks one measures its empty group (a sum of none is 0, a minimum null), and the view's measures read the merged ones by name, byte for byte the same on every engine
- `keys_from_two_sources_meet_only_when_their_values_are_identical` — 018 and 044: merged keys meet by the identity of their values, as a derived relation's do, so the text '010' and the text '10' are two keys, where 017's `=` would call them equal
- `a_views_parameters_bind_every_source_it_merges` — 018: a view's parameters are bound by the read and read by every source's `where` and measures, the merged ones too
- `a_merge_declared_badly_is_refused_naming_it` — 028 and 018: `declare` refuses a merged source that is not declared, read twice, grouped by another number of fields or a field of another kind than the one it lines up with, a measure named twice, a merged measure with no declared field or one that calls no aggregate, a view with parameters as any source, and a merge that closes a cycle of views, naming it
- `a_merged_sources_fields_are_probed_as_a_read_of_it` — 024 and 018: a view reads each of its sources as a read of that source, so a principal who may not read a field a merged source's measures aggregate is not answered the view's definition by describe, and a read of the view is refused as not granted, naming neither that field nor its source (024, extended)
- `a_view_that_merges_is_written_in_log_format_9` — log format §6 and 018: the event that declares a view merging sources is written in format 9, as is every event after it; a view without a merge leaves the lake's format where it was

**ingest**

- `the_same_content_ingested_twice_appends_nothing_the_second_time` — 016 and the log format: ingest is content-addressed, so a redelivery of the same values appends nothing, within one delivery or across two; a record with no field counts as a duplicate for a live key, and is invalid for a new one (016, extended)
- `the_same_key_twice_in_one_ingest_with_new_values_advances_the_version` — the log format: every event that changes a record moves its version by one, including two in one delivery
- `an_ingested_record_carrying_a_field_the_source_does_not_write_is_refused_whole` — 016 and the log format: ingest writes only source and shared fields; a record carrying any other is refused whole, never trimmed, and a record whose key is empty is refused as invalid before its fields are judged, as `upsert` orders it, after the ingest grant and the type; a field another authority writes is refused before any value is judged (016, extended)
- `an_ingested_record_carries_its_upstream_per_field` — 010, 016 and log-format §1.1 and §6: a record ingested with `upstream` names that lake and position on every field it sets, the event is written in format version 3, and the member is not in the event's id; a later write of a field without it leaves that field without it
- `an_upsert_carrying_a_field_only_a_source_writes_is_refused_as_the_wrong_door` — 016 and 015: an author never overwrites a figure the source is the authority on; the patch is refused as the wrong door
- `ingest_skips_redeliveries_and_unchanged_records_but_writes_a_revert` — 016 and the log format: an ingest that changes nothing appends nothing, and a record set back to an earlier value is a new change, not a redelivery
- `a_record_delivered_twice_in_two_windows_is_written_once` — 016 and the log format: ingest is content-addressed across windows too, so a record delivered again in a later window of the same ingest appends nothing and the feed holds one event

**rebuild**

- `a_rebuilt_projection_answers_every_read_as_the_live_one_did` — 001, 040 and the engine seam: a projection rebuilt from the log is the one it replaces, byte for byte, at the head and as of every earlier position, the `initial` values the fold filled (050), a type migration that converts records and a re-key that rewrites references to moved keys included
- `a_rebuild_reproduces_the_chain` — 001 and 020: the log is the truth and a rebuild only re-derives the projection, so verifying the chain answers the same range and hash before and after
- `a_rebuild_folds_a_page_over_the_records_the_pages_before_wrote` — 001, 027 and the engine seam: a rebuild folds the log in pages of 1 024 events, each over the projection the pages before left; a page that holds a write, a re-keying migration, a write at a new key and one at an old, and a second migration, over records an earlier page wrote, leaves what the live lake's appends left, at the head and as of each of them
- `a_rebuild_folds_a_page_that_computes_fields_again` — 001, 022 and the engine seam (lakelands#523): a rebuild folds a page of 1 024 events that holds a function declared again with a change, the outputs recorded for it, a write, and a `recompute`, over records an earlier page wrote; it leaves what the live lake's appends left, at the head and as of every event of the page, and calls no backing
- `after_a_rebuild_every_index_answers_as_before` — 001 and the engine seam: the projection and its indexes are derived from the log, so a find by a field, a lexical and a vector search and a walk over a relation answer by the type's declaration in force, for records written before it was declared again and for records a migration writes again, and after a rebuild they answer as they did
- `a_number_and_a_vector_read_back_in_their_one_json_form_before_and_after_a_rebuild` — the log format §1.2 and 001: every value a door answers is in its canonical JSON form (`1`, never `1.0`), so a read answers the same text live and after a rebuild

**filters**

- `absent_fields_never_match_not_even_not_equal` — 023: a comparison with an absent or null operand is false, `!=` included; `is null` is how absence is asked about
- `numbers_and_numeric_text_compare_as_numbers` — 023: when both sides read as numbers, a JSON number or a decimal string, they compare exactly as numbers, never as text
- `values_of_kinds_that_do_not_compare_match_nothing_and_fail_nothing` — 023: text that is not a number against a number, a boolean against text: every operator is false, and the read does not fail
- `probing_an_unreadable_field_is_refused_whether_by_filter_order_or_aggregate` — 024 and 006: filtering, ordering or aggregating over a field the principal sees masked, through `find` or through a pattern's rows and its aggregate, is refused as not granted, naming the field; over one it may not see at all, it is answered as for a field the type does not declare
- `a_filter_and_an_order_can_read_when_a_source_saw_a_value` — 010 and 017: `seen_at(f)` reads a field's provenance in `where`, and `order_by` may order by it
- `a_cleared_field_matches_nothing_but_its_record_is_found_by_another` — 012 and 023: a field set to null is absent, so no comparison on it holds, and the record is found by its other fields as before

**provenance**

- `valid_at_and_as_of_combine_and_neither_stands_in_for_the_other` — 009 and the glossary: `valid_at` asks what was true in the world, `as_of` what the lake knew; a read may name both
- `every_field_carries_its_own_provenance` — 010: with meta, each field answers its origin, known_at, position and writer, and a value from a source also its source and seen_at

**masking**

- `a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent` — 006 and 014: a field the principal may not read is masked (named, no value) or absent from get, find, describe and the tools' schemas
- `a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field` — 024 and 006: a derived field, one derived from it in turn, and a vector embedded from a field are read as the field they are built from: absent where it is absent and masked where it is masked, on get, find, a pattern's rows and describe, which shows a definition only where every field it names is read; a filter, an order or an aggregate over them is refused naming them, as not granted where they are masked and as for an undeclared field where they are hidden
- `a_field_the_principal_may_not_know_of_is_answered_as_one_never_declared` — 006 and 024: a field whose grant is none by name, sensitive or not, does not exist for that principal: it is absent from get, find, describe (a relation's link field too) and the tools, a search that names no fields never searches it, and a filter, an order, a pattern's `where`, a search's `fields`, an upsert and an ingest that name it are answered as for a field the type does not declare; a sensitive field whose none is inherited is listed as masked, and a filter on it is refused as not granted, to one who is shown the type: one whose grants give it nothing of the type is answered by describe, get, find and upsert as on a type that is not declared, a sensitive field or none
- `a_read_at_a_valid_time_probes_the_types_valid_time_fields` — 024 and 006: a read at `valid_at` filters on the type's `valid_from` and `valid_to`, so `find`, `get` and a pattern started on the type probe them as a filter would: refused not_granted naming the field where it is masked, answered as an undeclared field where it is hidden; without `valid_at` the same reads are answered
- `a_write_only_field_is_written_and_never_shown` — 006, extended (lakelands#393): a field granted write_only is written by upsert and retire like a writable one, while get, find, query, search, describe and the tools' read schemas never show it, a filter or a search that probes it is refused not_granted naming it, and the write answers no value; a writable field built from one the principal may not read, write-only or masked, is write-only in turn: written, and probed as not granted, never as undeclared

**computed inputs**

- `a_computed_field_is_read_no_further_than_what_it_is_computed_from` — 024, extended (lakelands#540, lakelands#652): a field computed by an expression (one that uses `previous` or `changed` as well), by a function with an expression or by one with a backing, and a field computed from such a field, is not answered to a reader who may not know of a field it is computed from, whether or not `depends_on` lists it: not by `get`, a `where`, an `order_by`, an aggregate, a view's measure, a pattern's projection or a search; a masked input masks it; `describe` shows the completed list, the log holds the declaration as it was sent, and a lake that holds such a field is closed when it is opened again; a function declared again with fewer inputs restricts by those alone, a reader and an author alike; a proposed shape and a relation's link field are held to the same, and a listed proposal's `more` says what was left out of its shape, not what was completed

**authored reads**

- `a_declaration_reads_only_what_its_author_may_read` — 024 and 028, extended (lakelands#538): a declaration whose author may not know of a field it reads is answered as one that reads a field the type does not have, whatever reads it: a set's `where`, a trigger's `when`, a verb's `requires`, what it sets a field from and the field it sets, a derived field, a field computed by a function, an embedding, a view's `where`, grouping and measure, and a derived relation's pair; by `declare`, `apply`, `add_field`, a migration and a proposal alike; a field the author knows of and may not read is refused as not granted on it, and a verb may set it; a field the type's shape names, as a key, a valid time or a relation's `via`, stands as declared for an author who may not know of it; `add_field` adds to its author's view of the type; a write's answer names a trigger or an object set only to one who reads every field its condition names, shown the definition or not, and still says `through`
- `a_type_its_author_is_not_shown_is_a_type_that_is_not_declared` — 027 and 028, extended (lakelands#538): to a principal that holds `schema` and is not shown a type, the type is one that is not declared: `add_field`, a migration, its apply and its proposal answer that there is no such type, and a trigger, a verb and an object set on it, declared or proposed, that it is not declared, each in the words an undeclared type gets; to declare the type or propose it is not granted and names nothing; none of them writes; a value type's preview and its refused apply show a use and a misfit only in a field their caller is shown and reads, the totals whole

**renamed grants**

- `a_grant_holds_a_field_under_every_name_it_has_borne` — 006 and 024, extended (lakelands#527): a field renamed by a migration is one field: its access is the lowest of what a principal's grants say under the name it bears at the position read and under every other name it has borne or will bear that they name, reading and writing judged apart; so a field held at none or masked stays so under its new name, through a chain of renames, and under its old name in a read as of before for a host that renamed its grant, at every door (`get`, a `where`, an `order_by`, an aggregate, a view's measure, a pattern, a search, `describe_type`, a write, a verb that sets it); a permission stays with its name, and a host widens by naming the new name and dropping the old; two fields that have borne one name are restricted together by it; a proposed migration that renames is listed to a reader kept from the old name with neither the pair nor the new name, and its accept answers it no rename; a migration's answer names its renames; the list is kept through a reopen, made again by a rebuild and an import, and is no event of the log

**authors view**

- `a_migration_keeps_the_fields_its_author_may_not_know_of` — 027 and 024, extended: a migration's `to` is read as its author's view, so a field hidden from the author stays as declared, with its values, whether the migration is applied by a principal with the schema grant or proposed and accepted; `dropped` lists only what the author saw and removed; the proposer reads its own proposal without the field, a reviewer who may see it reads it there
- `a_change_of_shape_never_names_a_field_its_author_may_not_know_of` — 027 and 024, extended: a type declared again from its author's view is taken, keeping the fields and the link fields hidden from the author; a shape that names a hidden field is refused as a field declared twice; one that removes a field a hidden one is built from is refused on that field and names no other; a rename from a hidden field is a rename from no field

**values**

- `date_time_values_read_back_in_their_one_canonical_form` — the log format §5: a date-time is kept in UTC as YYYY-MM-DDTHH:MM:SS.ffffffZ, whatever offset it was written with
- `bytes_are_padded_standard_base64_and_nothing_else` — the log format §5: bytes are canonical standard base64, padded and with zero pad bits; unpadded, url-safe or non-canonical spellings are refused
- `a_vector_of_the_wrong_length_is_refused` — 011: a vector field declares its dimensions, and a value of another length does not fit it
- `a_value_types_one_of_format_and_scale_hold_on_every_write` — 011: a value's `one_of`, named `format` and decimal `scale` are the value type's, checked on every write; scale rounds half away from zero
- `null_clears_a_field_whatever_its_value_type_allows` — 012 and 011: `null` in a patch clears the field; it is not a value the value type must allow
- `a_decimal_written_as_a_json_number_is_refused` — the log format §5: a decimal's one JSON form is a string of digits, so a JSON number, which may already have lost digits, is refused
- `a_geo_value_reads_back_in_its_one_form_and_a_malformed_one_is_refused` — 011, extended, and the log format §5: a geo value is a GeoJSON geometry whose numbers take their one JSON form, the same through get and find, live and after a rebuild; empty `coordinates` are the empty geometry of every type alike, while an empty part inside one is refused; an open ring, a missing `type` or a GeoJSON object that is not a geometry is refused naming the field; null clears it
- `a_json_value_reads_back_in_its_one_form_and_null_clears_it` — 011, extended, and the log format §5: a json value is kept as written but for its numbers, which take their one JSON form (`1`, never `1.0`), the same through get and find, live and after a rebuild; null clears it like any field
- `a_page_ordered_by_a_geo_or_json_field_takes_the_rulebooks_fallback_order` — 029: a geo or json value has no order of its own, so a find ordered by one ranks it as `cmp_values` does, a JSON value by its canonical bytes, a missing value last either way, ties by key; paging walks the same order

**schema**

- `the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins` — 007 and the log format's vectors: a type's schema hash is SHA-256 over its canonical shape, the same on every engine
- `describing_and_relabelling_a_type_leaves_its_schema_hash` — 007: descriptions, labels and field order in a revision are meaning, not shape, and leave the schema hash as it was
- `declare_refuses_a_revision_that_reshapes_stored_records` — 027: retyping, removing or requiring a field goes through `migrate`; `declare` refuses it and appends nothing
- `declare_refuses_bad_names_unknown_references_and_tool_name_collisions` — 028 and 014: a name that is not an ASCII identifier, a reference to an undeclared type, or a name that collides with a tool is refused whole
- `add_field_without_the_schema_grant_is_refused_before_it_looks_at_the_type` — 006 and 024, extended: add_field without the schema grant is refused not_granted whether the type is undeclared or already has the field, so the refusal tells nothing about the ontology
- `an_ordinary_principal_cannot_declare_a_sys_type` — 003: every type name starting `sys.` is reserved; an ordinary principal, even the owner, is refused declaring one and nothing is appended
- `redeclaring_a_type_with_its_fields_reordered_keeps_its_schema_hash` — 007: field order is not shape, so a type declared again with its fields in another order keeps its schema hash; the log keeps the declaration as written, so it is one revision
- `a_declaration_that_is_the_one_in_force_appends_nothing` — 027, extended (lakelands#565): a declaration that is the one in force (a value type, a function, a type, a verb, a trigger, a set, and a type whose computed field is stated with its completed `depends_on` or without) is answered `applied` at the head and appends nothing, by `declare` and by `apply`, and its version stays; a refusal still comes first; of a set what differs is appended alone, and what it sets off happens; an accepted proposal of declarations in force appends its acceptance alone
- `a_migration_to_what_is_in_force_appends_nothing` — 027, extended (lakelands#577): a migration whose `to` is the type in force and that renames nothing is answered `applied` at the head and appends nothing; a stale report is refused first; an accepted proposal of it appends its acceptance alone; a value type's migration to itself is refused as before, there being nothing to migrate
- `the_door_computes_again_what_a_declaration_in_force_computes` — 022, extended (lakelands#565): `recompute` appends the declaration in force again as it was declared, at its next version; a function that reads a setting is computed again with the setting as it stands, where declaring it again changes nothing; called again it changes no record and still appends; without the `schema` grant it is not granted before the name is looked at, a name no function or type has and a type its caller is not shown are not found, a view and a type with no field computed outside the lake are invalid, and nothing is appended

**migrate**

- `migrate_apply_requires_the_report_it_was_shown` — 027: `migrate_apply` names the preflight's report; another report id, or a lake that moved since the report, is refused, after the preflight's own refusals (027, extended)
- `a_migration_that_would_lose_information_is_refused_with_its_report` — 027: a migration applies only when every stored value converts without loss; otherwise it is refused and the report names the records, after its report id is compared (027, extended)
- `migrated_values_follow_the_conversion_table_and_renames_keep_them` — 027: a number becomes text in its canonical form, numeric text becomes the equal number, and a renamed field keeps its values
- `a_migration_removing_the_field_a_relation_walks_is_refused` — 027 and 018: a relation is carried by its `via` field; a migration that removes that field would leave the relation dangling and is refused
- `the_preflight_names_every_dropped_field_and_an_empty_type_migrates_cleanly` — 027 and MigrationReport.dropped: the preflight names every removed field with how many values it discards, and a type with no records reports nothing to lose; the report is lossless and the apply discards them
- `a_view_is_never_migrated` — 027, extended: a migration reshapes stored records, and a view stores none, so a migration that would turn a stored type into a view, or a view into a stored type, is refused as invalid naming the type, at the preflight or when applied, and nothing is appended; a view changes, and becomes a stored type whatever its fields become, by being declared again
- `a_migration_taking_a_field_a_view_reads_is_refused` — 027 and 028, extended: a view over a type, as its source or a merged one, reads the shape a migration leaves, so the preflight refuses a migration that removes or renames a field the view reads, naming the view and the field as `declare` would refuse the view against the new shape; a field no view reads migrates as before

**made of**

- `a_type_made_of_another_is_shown_only_with_it` — 006, extended (lakelands#595): a link type is shown to a principal only where the type that declares its relation is, and a view only where its source is, whatever a grant on its own name says: to one kept from a type, `describe` names neither a link type of it, nor a set or a trigger on that, nor a view over either, a proposed trigger on the link type is not listed, and `describe_type`, `find`, `get` and `search` of the link type, a read of the view, a walk over the relation from the type, `link` and `unlink`, over a relation the type has or one it has not, answer as for a type that is not there; a step from the target by the inverse of a relation of that type, carried by links or by a reference field, answers as a relation that is not there; a principal who reads one field of the owner is shown them all, and one kept from the link type by its own grant is kept from the view over it and walks the edge
- `a_type_kept_from_its_reader_is_kept_at_every_door` — 006, extended (lakelands#636, lakelands#635): to one whose grants give it nothing of a type that declares a sensitive field, the type, the link type of its relation and each view made of it (a count that reads no field, a grouping and a sum that read its fields, a view over that view, and a view that merges it beside a type the reader is shown) answer `describe_type`, `get` of a key that is there and of one that is not, `find`, with a total and with an aggregate, `search` and a pattern, and the type `upsert`, `retire` and `link`, value for value as a name that is not declared, and `describe` and the tools name none of them; so to one with no grant, to one granted to read each view and the link type by name, and to one who reads the whole lake but that type

**initial**

- `a_record_takes_its_initial_values_when_it_comes_to_exist` — 050 (lakelands#551): a declared `initial` is kept in the one form a write of its field stores, so `"1.50"` after `"1.5"` on a decimal of scale 2 is the declaration in force; the event that makes a record fills each field that declares `initial` and holds no value after its patch, in the field's one form, at the record's first version, with the origin `initial`, the event's position, time and writer and nothing of its source, and a derived field reads it there; so a source's delivery makes a record whose required authored field it does not carry, the same content again is a duplicate, an author moves the value and the next delivery leaves it; an authored create is filled too, one that carries the field keeps what it carries, and a null there is filled; a later declaration with another value changes no record and is what the next one takes, a field added with one fills no record that exists, a record retired and made again starts again as a create to a trigger, an author's clear clears, and a re-key fills nothing
- `an_initial_value_is_declared_on_an_authored_field_of_a_types_own` — 050 (lakelands#551): `initial` is refused as invalid, naming the field, on a field a source writes, a shared one, a derived one, one with an expression, a key's field, a reference, a relation's `via` field, a field its inputs clear, a view's field, a relation's field, a verb's payload and a function's input; a value its field would not take is refused as a constraint naming the field, by its kind or its value type; the schema hash leaves it out, so a changed `initial` is a new version under the same hash, and a migration proposed before it changed is refused at its accept by what it restates; a value type's tightening that a declared `initial` no longer fits counts it a misfit of its field, shown to the owner, and to a proposer who may not see the type or reads the field masked is refused by its count alone

**links**

- `a_link_is_written_and_walked_with_its_fields` — 035 and §2.3: a relation without via is carried by link records; the walk follows it out and in, and the edge carries the link's own fields
- `writing_a_link_again_changes_its_fields_and_keeps_one_link` — 035: a link is keyed by its pair, so writing it again changes its fields and advances its version, never adds a second link
- `an_unlinked_link_leaves_the_walk_and_stays_in_its_history` — 035 and §2.3: unlink retires the link record; the walk no longer follows it, and a read as of before the unlink still does
- `a_link_that_would_break_the_relations_cardinality_is_refused` — §2.3 and 024, extended: many_to_one allows one live link per `from`; a second is refused as a constraint and nothing is appended, and a principal without write on the link type is refused not_granted before the links are read
- `a_link_to_a_key_that_is_no_live_record_is_refused` — 032 and 035: a link's ends are references, so a link to a key that is not a live record of the target is refused on `to`
- `link_records_go_through_the_link_doors_and_ingest_only` — 035 and §2.3: upsert and retire refuse a link type's records, before a retry under the key of a link; a caller cannot declare a link type; the link door refuses a relation whose links arrive by ingest, before `from` among the fields, `from` among the fields before a retry, and a retry before the record checks (035, extended); and ingest writes them
- `a_relation_cannot_move_between_via_and_link_records` — 035 and §2.3: a declaration that gives a relation via it did not have, or takes it away, is refused
- `a_link_field_the_principal_may_not_write_refuses_the_link` — 035: the link doors check the principal's grants on the link type's fields as upsert does
- `a_relations_fields_are_declared_when_added_and_migrated_when_retyped` — 007, 027 and 035: adding an optional link field is declared; retyping one reshapes the link type, is refused at declare and goes through migrate on the owner type
- `retiring_an_end_keeps_the_link_and_the_walk_skips_the_retired_end` — 032 and 035: retiring a record others link to stays allowed and keeps the links' history; the walk does not reach the retired record
- `a_relation_that_allows_one_link_per_target_refuses_a_second` — §2.3: one_to_many and one_to_one allow one live link per `to`; a second link into it is refused as a constraint
- `a_relations_authority_changes_by_declaration_and_keeps_its_links` — 035, extended: a relation's authority changes by an ordinary declaration; the door its links arrive by swaps, and the links already written stay
- `ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window` — §2.3: a link that arrives by ingest holds the relation's cardinality over the lake's live links and the window's, its ends read from the key when the record leaves them out
- `a_relation_declared_again_with_another_target_is_a_migration` — 027 and 035, extended: a relation removed by migrate keeps its links, and the preflight lists it under `dropped` with the number of its live links, none when none is live (lakelands#554); declaring it again with another target would reshape them, and is refused, naming the link type
- `a_restricted_link_is_walked_only_with_the_restricted_grant` — 035 and the retention hold: a link's edge comes from its own record, so a restricted link is walked only by a principal with the `restricted` grant
- `narrowing_a_relations_cardinality_is_refused_while_links_would_break` — 027 and §2.3: narrowing a relation's cardinality is a migration, and its preflight refuses it while live links would break the narrower rule

**walks**

- `a_step_over_a_relation_the_type_does_not_declare_is_refused` — 018: the planner knows the ontology; a step over an undeclared relation is refused, not answered empty
- `a_step_deeper_than_the_lakes_limit_is_refused` — 018 and the location's limits: on a lake opened with the default limits, a step whose depth exceeds `max_depth` is refused rather than walked
- `a_walk_answers_what_it_reaches_in_one_order` — 026 and 029: a graph includes its start, and a walk orders what it reaches by depth, type and key, so no engine's link order shows through
- `a_step_naming_an_inverse_walks_the_relation_the_other_way` — 018 and 044, extended: a step naming a relation's `inverse` walks that relation the other way, its `walk` counting from the name it gives, and the edges keep the declared relation with its declaring side as `from`
- `a_walk_over_a_cycle_ends_and_answers_each_record_once` — 026: the store walks the graph, so a walk over a cycle of links ends within its depth and reaches every record once

**search**

- `full_text_matches_words_whatever_their_case_inflection_or_accents` — 025: the store's one token pipeline splits, lower-cases, stems and folds diacritics, so the same query matches the same records on every engine
- `search_is_ordered_by_score_then_key` — 029: ranked search answers best first, and equal scores in key order
- `searching_a_type_whose_searchable_field_is_unreadable_is_refused` — 024: a search over a searchable field the principal may not read is refused, naming the field
- `a_hybrid_search_over_an_unreadable_searchable_field_is_refused_whole` — 024: a hybrid search over a type whose searchable field the principal may not read is refused, naming the field, whether it brings a vector or not; it never answers from its vector half
- `a_vector_search_over_an_unreadable_vector_field_is_refused` — 024: a vector or hybrid search that names a vector field the principal may not read is refused, naming that field
- `a_search_over_a_vector_embedded_from_an_unreadable_field_is_refused` — 024: a vector embedded from a field encodes that field, so a vector or hybrid search over it is refused, naming the field, when the principal may not read one of the fields it is embedded from, even though it may read the vector; a vector field named in `also` is probed like the first, and one the principal may not know of is answered as undeclared
- `a_search_finds_a_record_by_its_current_text_and_nothing_by_none` — 025: the index follows every write, so a record is found by its new words and no longer by its old ones, and a query of no words matches nothing
- `a_retired_or_restricted_record_is_never_a_hit` — the log format §2.3 and 006: a retired record is invisible, and a restricted one invisible to a principal without the `restricted` grant; neither is ever a search hit
- `a_retired_record_or_a_cleared_vector_is_no_longer_a_nearest_neighbour` — the engine seam's vector index: it follows the projection, so a retired record or a cleared vector field is no longer among the nearest
- `a_vector_search_answers_the_brute_force_ranking_to_the_bit` — 025 and 029, the engine seam's vector index: for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added, a vector search answers the keys, order and scores of the brute-force reference: the one cosine over every vector at the head, best first, ties by key, kept where the read sees the record
- `a_hybrid_search_without_a_vector_or_a_backend_ranks_lexically_and_says_so` — the embedding seam (docs/seams.md): with no embedding backend bound, a hybrid search that brings no vector ranks by its text alone, and its answer says it ranked lexically
- `a_bound_backend_embeds_a_hybrid_search_without_a_vector_and_nothing_else` — the embedding seam (docs/seams.md): a hybrid search without a vector ranks through the bound backend and says `hybrid`; a vector search or a hybrid one bringing its vector never calls it; a backend that fails, whatever error it answers, makes the search unavailable, never lexical
- `every_hit_is_the_record_get_answers_in_its_declared_shape` — README, Search: a search never returns a shape the ontology did not declare; every hit, lexical, vector or hybrid, is the record `get` answers the same principal, with only the type's declared fields and only those it may read

**search fields**

- `a_search_limited_to_named_fields_matches_and_ranks_in_those_only` — 024 and 025: a lexical or hybrid search that names `fields` matches, scores and ties over those searchable fields only, the same on every engine; without `fields` it searches every searchable field
- `a_search_limited_to_readable_fields_is_answered_beside_a_masked_one` — 024: a search that names `fields` probes only those, so a searchable field the principal may not read no longer refuses it when it is not named; the field stays masked in every hit
- `naming_an_undeclared_unsearchable_or_unreadable_field_is_refused` — 024: a named field that is not declared or not searchable is invalid, one the principal may not read is not granted, judged before its searchability, and a search naming no field is invalid
- `a_hybrid_search_limited_to_named_fields_still_probes_what_its_vector_is_embedded_from` — 024: `fields` narrows only the text half of a hybrid search; its vector still encodes every field it is embedded from, so a hybrid search over it is refused, naming the field, when the principal may not read one, whatever `fields` names

**proposals**

- `a_low_verb_runs_and_a_higher_one_is_proposed` — 013: a `low` verb that writes authored fields runs at once; any other risk is recorded as a proposal and runs only when accepted; a call is refused for its payload before its record, for its record before its version, and a retry answers before the grants (013, extended)
- `an_external_verb_without_an_executor_is_unbound` — 013 and 015: a verb whose effect reaches outside the lake runs only through a bound executor; `describe` names the executor and says beforehand that it is not bound; without one, calling it at any risk or proposing a call of it answers `unbound` naming the executor, after the grant and before the payload (013, extended), and nothing is appended
- `a_riskier_call_that_could_not_run_now_is_not_proposed` — 013, extended, and 024, extended: a call that would become a proposal is checked as if it ran now: at a stale version it is a conflict, and where its `requires` does not hold it is refused as a precondition, the version first; nothing is appended
- `accepting_a_call_after_its_verb_was_declared_differently_is_refused` — 013, extended (lakelands#371): accepting a call whose verb was declared differently after the call was proposed, its effect or its payload changed, is refused as invalid naming the verb and leaves the proposal pending, as a scope that no longer fits does; a verb declared again with the same content, or changed and changed back, is accepted
- `accepting_a_proposal_applies_its_write_as_the_proposers` — 013 and 004: a proposal is a `write_proposed` event; accepting it appends `write_accepted`, then the verb's run caused by that acceptance and its write caused by that run, written as the proposer
- `a_rejected_proposal_writes_nothing_and_a_decided_one_stays_decided` — 013: rejecting appends `write_rejected` and nothing else; a decided proposal is refused as decided
- `only_a_reviewer_decides_and_a_proposer_sees_its_own_proposals` — 006, 013 and Catalog::proposals: accept and reject need the review grant, checked before the proposal is looked up (013, extended); without it, `proposals` lists only the principal's own
- `a_verbs_write_carries_the_verbs_procedure_version` — 010 and the log format §1.1: a value a verb produced carries, as its procedure_version, the version of the verb's declaration
- `an_external_verbs_output_or_failure_is_recorded_and_it_runs_once_per_position` — seams, VerbExecutor, and 013: `describe` says the bound executor is bound; the executor's answer is recorded as output_recorded caused by the event that made the verb due and returned in the outcome's output; a failure is recorded as {"error": …} at a real position; a retry of the same call answers the first outcome, its position and output, and like a rebuild runs nothing again
- `a_retried_external_verb_answers_its_call_though_a_host_write_names_it` — 013 and 021, extended: a host write naming an external verb's `verb_called` as its cause does not become the call's outcome; a retry with the call's idempotency key answers the verb's position and its recorded output, for a verb on a record and one on its type
- `a_retried_write_verb_answers_its_own_write_though_a_host_write_names_it` — 013 and 021, extended: a retry of a write verb answers the record write the verb itself made and its version, though a later host write names the verb's `verb_called` as its cause
- `a_retried_proposed_call_answers_its_proposal_after_it_is_accepted` — 013 and 015: a retry of a call that was proposed answers the proposal, its first outcome, after the proposal was accepted and a host write named the accepted call as its cause

**readers view**

- `a_proposal_shows_its_reader_the_values_it_may_read_and_is_accepted_whole` — 013 and 024, extended (lakelands#499): `proposals` shows a proposed value where a read of the record would: a field its reader may not know of and a masked one are not in a proposed patch, a proposed call's payload is whole only to a reader who may read every field the verb sets, or see the type of a verb that reaches outside, and a write to a type the reader may not see is listed without its write; every entry that holds more than its reader is shown says `more`, and no other does; the proposer reads its own proposal by the same rule; the accept applies the logged write whole, what its reviewer was not shown too

**proposal names**

- `a_proposed_shape_names_only_what_its_reader_may_know_of` — 013, 024 and 027, extended (lakelands#499): `proposals` shows of a proposed migration the renames, the dropped fields and link fields, the re-keyed types and the rewritten references only where its reader may see the type and know of the field, the totals whole; of a proposed bundle each declaration `describe` would show its reader once in force, a verb by the fields it names; an entry that holds more says `more` and names nothing else, and a bundle with nothing left shows no write
- `a_listing_is_filtered_on_what_it_shows_and_an_accept_names_no_hidden_field` — 013 and 024, extended (lakelands#499): a proposal on a type its reader may not see, or on a restricted record without the grant, is listed without its write and says `more`; `type` and `kind` are answered on the entry as shown, so a hidden type answers what an undeclared one answers and an entry without a write is of no kind, while the unfiltered listing and `state` hold it; an accept refused because a declaration reads a field the migration takes away names neither to an accepter who may not know of the field

**report names**

- `a_migrations_report_is_answered_as_its_reader_may_know_it` — 013, 024 and 027, extended (lakelands#531): a migration's report names a conversion, a dropped field, a re-keyed type, a rewritten reference and a colliding key only where its reader may see the type and know of the field, and a sample key only of a record it is answered, in the refusal of an accept, of `migrate_apply` and of `propose` and in a preview, an author's own too; `more` says that the report holds more, and one who may see everything reads it whole without it; the id stays the whole report's, so a preview read trimmed is applied by it; a relation the migration removes is listed as dropped to one who sees the type

**restated**

- `a_migration_reports_every_member_its_to_states_otherwise` — 027, extended (lakelands#554): every member of a type's, a field's, a relation's and a link field's declaration, as it serialises, changed alone in a migration's `to`, is in its preflight's report: a type that changes and a `required` that tightens in `fields`, every other member in `restated` with what was declared and what `to` states, a member left out as null, and nothing for a `depends_on` left to the store; an added field and relation are restated member by member; `describe` shows a field its reader does not read by its shape alone (024)
- `what_a_migration_restates_is_shown_as_its_fields_are_and_holds_at_the_accept` — 027, 024 and 013, extended (lakelands#554): a `to` that leaves out `sensitive`, `searchable` and the labels is lossless and its report restates each; an entry is answered where its reader may know of the field, a renamed field's where it may know of the name it had, in a proposal's listing and in the refusal of an accept that is not lossless, with `more`, and an author kept from a field restates nothing of it; one who knows of a field or a link field and does not read it, in `to` or as the type stands, is answered its shape alone, `sensitive` and no label, description, annotation, `searchable`, initial value or expression, in its own preflight, in a listing and in a refusal, and in the `to` and the types a listing hands it; the `write_proposed` event carries the report whole in the feed and in an export, and without the member where nothing is restated; an accept is refused where the type gained a mark the proposal's report does not restate

**schema proposals**

- `a_proposed_declaration_takes_effect_only_when_accepted` — 013, extended, and 007: a principal without the schema grant may propose declarations; nothing is declared and no schema hash moves until a reviewer with review and schema accepts, and then the declaration is logged at the acceptance, written as the proposer and caused by the acceptance
- `accepting_a_schema_proposal_needs_the_schema_grant` — 013, extended: a reviewer with review alone may not accept a declaration or a migration; the refusal is not_granted, nothing is appended, and the proposal stays pending
- `a_migration_proposal_carries_its_report_and_is_preflighted_again_at_accept` — 013, extended, and 027: a proposed migration carries the report its preflight answered then; at the acceptance it is preflighted again on the head, and one that would now lose information is refused with the fresh report, the proposal pending and the type unchanged
- `a_proposed_migration_is_accepted_only_on_the_shape_it_was_proposed_on` — 013, extended (lakelands#489): a migration's `to` is the type's whole shape, so a proposed migration is accepted only while the type's schema hash is the one its report was made on; on a type declared or migrated since, the accept is refused as invalid, nothing is appended, the proposal stays pending and a field added since keeps its values; on the shape it was proposed on it applies
- `a_rejected_schema_proposal_leaves_the_ontology_untouched` — 013, extended: a rejected declaration leaves the ontology as it was, and proposals can be listed by kind
- `a_value_types_tightening_is_proposed_without_schema_and_applied_on_accept` — 013 and 027, extended (lakelands#476), and log format §6: a principal without the schema grant may propose a value type that accepts less; the proposal is written in format 10 and holds the preflight's report, whose uses a reader of the listing sees only in fields it may read, the totals whole; accepting it takes schema beside review, and then the value type is in force and a value it no longer accepts is refused
- `a_tightening_is_refused_while_a_value_would_not_fit_or_the_value_type_changed` — 013 and 027, extended (lakelands#476): a value type's tightening is refused at the proposal while a stored value would not fit, as invalid, saying how many and naming none, with nothing appended; at the acceptance it is preflighted again and refused the same way, and it is refused when the value type was declared again since the proposal; each time the proposal stays pending and the value type as it was; a narrowed declaration's refusal points to the proposal

**annotations**

- `annotations_round_trip_through_declare_the_feed_and_describe` — 007, 024 and 027, extended: annotations on a type, a field, a relation and a relation's link field, declared or applied, come back exactly in the feed's `declared` event and in `describe` for the owner, in canonical form
- `an_annotation_change_alone_moves_the_version_and_not_the_schema_hash` — 007 and 027, extended: re-declaring a type with only an annotation changed is taken by `declare`, moves its version by one and leaves its schema hash byte-identical
- `a_masked_field_is_described_without_its_annotations` — 024, extended: a field the principal may not read is described by its shape alone, without its annotations, and so is a relation's link field it may not read (no description, label or annotations); the type's and the relation's own annotations are still shown
- `an_unnamespaced_reserved_null_or_oversized_annotation_is_refused_naming_it` — the contract's annotation rules: a key without a namespace, a `null` value and a declaration holding more than 16 KiB of annotations are refused as invalid, a key in `sys.` or `lakelands.` as reserved, each naming the key or the declaration, with nothing appended; a declaration holding exactly 16 KiB is taken

**cursors**

- `a_cursor_stays_on_the_position_of_its_first_page` — the glossary: a cursor is bound to the position its first page was read at, so a page set is stable while the lake is written
- `a_cursor_continues_only_the_answer_it_was_handed_out_for` — 018 and 029: a page continues only the answer its cursor was handed out for, the same door and the same type: a cursor of `find` on one type continues that `find`, and is refused as invalid by `find` on another type, by `query` and by `search`, naming where it belongs; a cursor of `query` or `search` is refused by `find` alike
- `a_cursor_handed_to_a_find_of_another_order_length_is_refused` — 018 and lakelands#436: a cursor of `find` holds one sort value per key of the `order_by` its first page was read with; handed to a `find` whose `order_by` is longer or shorter, on a stored type or on a view, it is refused as invalid, naming the order, and never answers a page or `unavailable`
- `a_cursor_continues_only_the_read_it_was_handed_out_for` — 018 and lakelands#419: a cursor carries a digest of the read it was handed out for, so `find` under another `where` or an `order_by` of the same length, `search` for other words or in named fields, and `query` with another start `where` or projection refuse it as invalid; another page size, `with_meta` or `total` continue it

**feed**

- `the_feed_answers_the_log_from_a_position_by_kind_and_type` — 004: `poll` answers the events from a position in log order, filtered by kind and type, with the position to poll from next and the head
- `rebuilding_the_projection_changes_nothing_in_the_feed` — 001 and 004: the log is the truth and the projection an index; deleting and rebuilding the projection leaves every event as it was
- `a_trigger_fires_on_every_write_whose_guard_holds` — 004 and 015, extended: when a write makes a trigger's guard hold, `trigger_fired` is appended with it, and the write's answer names the trigger, `through` its position; a write under which the guard does not hold fires nothing and answers nothing more; a retry answers the same, and an acceptance what the write it applied set off
- `an_object_set_change_is_written_only_when_membership_changes` — 004 and 015, extended: a write that moves a record into or out of an object set appends `object_set_changed`, and its answer names the set as joined or left; one that leaves membership as it was appends none and names none
- `a_trigger_whose_guard_the_writer_may_not_read_is_not_named` — 015, extended, and 024: a write's answer names a trigger it fired, or an object set its record joined or left, only if `describe` lists it to the writer; one whose condition reads a field the writer may not read is left out, and `through` still covers it
- `polling_from_next_in_small_pages_sees_every_matching_event_once_in_order` — 004: a consumer that polls from the `next` each answer gives, in pages of one or two events and with writes landing between polls, sees every matching event once, in log order; this is the position a deliverer keeps
- `a_poll_with_a_limit_of_zero_answers_one_event` — seams, ChangeFeed: a limit of 0 is read as 1, so a consumer that passes 0 still moves forward instead of polling the same position for ever
- `a_record_event_says_whether_it_made_the_record` — 004 and lakelands#572: `record_upserted` and `record_ingested` carry `made` where the write found no live record under the key and leave it out otherwise, so a consumer of the feed tells a record made from one changed without a read: a first creation with `version` 1, a record made again after its retirement with the version its history gives it, a change and a verb's write without; it is said where the event is made, of what the writer then holds: of two records of one ingest that name one key the first made it, and a proposal is judged when it is accepted, not when it was proposed
- `a_feed_filtered_by_key_answers_that_records_events_only` — 004 and lakelands#146: a feed filtered by types and keys answers exactly the events about those records, in log order, each once: its upserts, a trigger it fired, its object-set changes, a restrict, an unrestrict and its retire; with types empty a key matches in any type and verb_called passes too; kinds narrow it further; no event that names no record passes
- `a_key_filter_does_not_follow_a_rekey` — 004, 027 and lakelands#146: a feed filtered by a record's old key answers its events up to the migration that re-keyed it and none after; the new key answers what came after

**retire**

- `a_principal_who_may_write_only_some_fields_cannot_retire` — 006 and TypeRights::retire: retiring removes every field of a record, so it needs a write grant on every field of the type, checked before the record is looked up, and the record before the expected version (016, extended)
- `a_record_retired_at_a_position_is_readable_just_before_it` — 009 and 001: a read at the position before a retirement sees the record as it was, to a principal who could read it then; a session told of the retirement reads it there
- `a_type_whose_fields_all_come_from_a_source_has_no_upsert` — 016 and TypeRights::upsert: upsert writes authored fields; a type with none has no upsert for anyone, the owner included: describe says so, and the door refuses it as the wrong door, naming the field

**view rights**

- `a_view_over_a_field_its_reader_may_not_read_names_nothing` — 024, extended: `find`, `get` and a pattern started on a view whose `where` or a measure reads a field its reader may not know of, or may not read, are refused as not granted, on no field and with neither the field nor its source in the text; a reader who may read every field the definition reads is answered, and one with the `schema` grant, whom `describe` shows the definition, is told the field as before

**view kept**

- `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` — the `view_kept.` setting, by a twin: a view named by a `view_kept.` setting and the same view under no setting answer alike, in the records the owner finds after each event and a reader without the `restricted` grant at each row's end, as of the row before, and, where the table reads, through every door of a view (`find`, a count and `find` by a text group field, two pages in an order, an aggregate, `get` of a key and of none, a pattern's start and a walk over a derived relation); at the table's end for a reader kept from a field of the view, for one kept from a field it reads of its source and for one kept from a group field of a merged source, and a reference and a link to its records; then after a rebuild, a compact and an import, and for the events sent after them; so a grouped view, one with a `where`, an ungrouped one, one with a merged source, one over a kept view, asked for before the view it reads, and one grouped by a field a recorded output fills, while members arrive, change, move to another group, to none and leave, a member is restricted under the outer view's `where`, and every member of an ungrouped view is
- `a_kept_measure_answers_as_its_twin_whatever_values_it_folds` — the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: an extreme whose holder leaves, equal values at two keys and an equal value written over the holder, which reads as written; a group's values under a number and a decimal group field, written and fed, where `1.0` and `1.00` are two groups of a decimal and one of a number; the holder of an outer view's extreme leaving a group whose other member is in it on the whole face alone; a sum whose large member leaves, and sums of `5` and `0.00` in both key orders; a setting written over a group whose measure divides by zero, after which the next write is accepted and the view is refused as its twin is until the member changes; and `max(known_at(n))`, null in both
- `a_kept_view_answers_as_its_twin_at_every_position_and_under_every_setting` — the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: the kept view and its twin answer alike while a setting its `where` names is written, restricted, lifted, retired and written again, and one a measure's argument names; while its own setting is retired under a kept view over it, written `false` and written again; while a `view_kept.` setting that the `where` of another view names is written; while it is declared with a parameter, which `describe_type` says cannot be kept, and without one, and again with another `where`; while a function a field of its source calls is declared again, and the source with another derived expression, each computing the source's records again, after which it is kept; and its source is migrated; a read at a valid time answers what the twin computes there; `describe_type` says `not_asked` of a view no setting holds for, never `kept` of a view over one that is not, and `not_asked` of every twin
- `a_kept_extreme_answers_as_its_twin_when_a_run_or_a_re_key_moves_its_holders` — the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: a kept `min` and `max` answer as their twins after one aging run takes the holders of nine groups out of the view's `where`, after a purge retires the holders of nine groups, and after a re-key moves a holder to a greater key and the keys of the records that refer to it in the same event, `describe_type` saying `kept` after each, since every group is read again in the fold of the event that took its holder; and after the appends that follow each

**keys**

- `a_key_that_disagrees_with_its_key_fields_is_refused` — the log format: a record's key is its key fields' values joined by `:`; a write that says otherwise is refused and appends nothing
- `a_record_created_by_its_key_alone_gets_its_key_fields` — the log format: a writer may give only the key; the key fields are read from its components

**restriction**

- `a_restricted_record_is_withheld_but_kept` — the facade: restrict hides a record from principals without the restricted grant, keeps it for those with it, keeps its history, and unrestrict lifts it

**reactions**

- `a_trigger_fires_on_a_verbs_write` — 004 and 013: a verb's write fires the triggers whose guard holds and changes object sets in the same append, caused by that write, as any other write does
- `a_sets_filter_from_describe_finds_the_members_its_events_record` — 004, and seams `Reading an object set`: for records written after a set is declared, `find` under the filter `describe` answers for the set returns exactly the records the latest `object_set_changed` of each says are in it, and `(f1) and not (f2)` returns the members of one set that are not in the other

**set declared**

- `a_sets_declaration_decides_the_membership_of_the_records_there` — 004, extended: a set declared over records already written appends, in its declaration's append and caused by its `declared`, an `object_set_changed` joining each live record its filter holds for and no other; declared again with another filter, it joins and leaves exactly the records the change moves; declared again unchanged, it appends none; and a later write decides against the membership so kept; past the lake's `max_set_changes` a declaration is refused `too_large`, naming the set and the ceiling, and appends nothing

**set names**

- `a_set_names_a_set_and_follows_it` — 004, extended (lakelands#552), and log format §6: in_set('name') holds for a record where the named set's filter holds: union, intersection and difference are or, and, and not, through two levels; a read's where names a set and reads its filter, as of an earlier position the filter in force there; a set declared again decides the sets built on it in the same append; a write joins and leaves them; a trigger whose when names a set fires as the filter holds; the declaration that names a set is written in format version 11
- `a_set_that_cannot_be_named_and_a_path_that_cannot_be_walked_are_refused` — 028, extended (lakelands#552): in_set of a set that is not declared, of a set of another type and of a set that names it in turn is refused as invalid, and in_set outside a set's where, a trigger's when and a read's where; a path that steps into a field that is no json is refused at the declaration of a set, a trigger and a derived field and at a read, saying what to write, while a path into a json field is read; a set declared again for another type is refused on `of`; none appends; past max_set_changes over a declared set and the sets built on it the declaration is refused too_large, naming the set whose change passed it
- `a_named_set_is_read_by_the_right_to_read_what_its_filter_reads` — 024, extended (lakelands#552): a reader kept from a field is answered, for in_set of a set whose filter reads it, directly or through the sets it names, as for a set that is not declared; describe lists it no such set; a write's answer names it no such set joined or left; an author kept from the field declares no set that names one

**set migrated**

- `a_migration_decides_the_membership_of_the_records_it_migrates` — 004, extended: a migration appends, directly after its `migrated` and caused by it, an `object_set_changed` for each record whose membership under a set of the type changes as migrated, and none when it changes nothing a filter reads; its answer names each set joined or left once and `through` the changes, also when it adds a relation carried by links; a re-keyed member stays a member under its new key, after a rebuild too; past the lake's `max_set_changes` a migration is refused `too_large`, naming a set and the ceiling, and appends nothing; one that is also applied with a stale report, or is not lossless, is refused for that first

**set rewritten**

- `a_rekey_decides_the_sets_that_read_a_rewritten_reference` — 004, extended: a re-keying migration appends, caused by its `migrated`, an `object_set_changed` for each record whose membership changes once its references to the moved keys are rewritten: a record of the migrated type under its new key, and a record of another type that holds such a reference, naming that type; a set whose filter reads no rewritten reference gets none
- `a_migration_decides_the_sets_over_its_link_types` — 004, extended: a migration that converts a relation's field appends, directly after its `migrated` and caused by it, an `object_set_changed` for each link whose membership under a set of the link type changes, and its answer names those sets; a re-key of the owner moves a link's membership with it and changes only the set that reads a rewritten end; after either, and after the lake is opened again and its projection rebuilt, `find` under each set's filter returns exactly the links the log says are in it, and a later write of the link leaves a set under its new key

**link triggers**

- `a_trigger_on_a_link_type_fires_on_link_rewrite_unlink_and_ingest` — 035 and 004: a trigger naming a relation's link type fires on link, on the link written again, on unlink and on each ingested link, naming the link type and the link's key, caused by that write; its guard reads the link's fields, and `changed(f)` holds on a write that changes f and not on unlink
- `a_trigger_on_the_link_type_of_a_reference_relation_is_refused` — 035: a relation carried by a reference field has no link type, so a trigger naming one is refused as invalid on `on`, saying to watch the type with `changed(<field>)`

**bundles**

- `a_bundle_with_forward_references_is_applied_whole_or_not_at_all` — 028: apply validates a set of declarations together, so a reference may point forward within it, and appends all of it in one append or none of it

**references**

- `a_write_referencing_a_record_that_does_not_exist_is_refused` — 011 and the log format §5: a reference is the key of a record of its target type; a write naming no such record is refused whole, one naming an existing record is accepted
- `a_reference_to_a_record_earlier_in_the_same_ingest_resolves` — 016 and the log format §5: an ingest's records are written in order, so a record may refer to one delivered before it in the same window

**reads kept**

- `a_migration_keeps_the_fields_a_declaration_reads` — 027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from, a field named `payload` read bare too) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; so is one that drops a relation's field a trigger on the relation's link type reads, naming the link type; a migration that drops a field no declaration reads goes through; an author who is not shown the reader, as `describe` shows it, is told that a declaration reads the field and not its name, nor a field hidden from it, which its migration does not take away
- `a_migration_names_a_view_only_to_an_author_who_is_shown_it` — 027, extended: a migration that takes a field a view reads, or changes a kind the view cannot be declared over, is refused naming the view only to an author `describe` shows the view's definition to, in the sentence it had; an author who is not shown the view's type, or is shown it with its definition withheld, is told that a declaration reads the field, or that a declaration over the type could no longer be declared, without the view's name
- `a_declaration_reads_only_fields_its_type_has` — 028, extended: `declare` and `apply` refuse as invalid, naming the declaration and the field and appending nothing, a derived field, an object set's `where`, a trigger's `when` (`changed` and `previous` included) and a verb's `requires` or the value it sets a field from that read a field the type does not have, and so does a migration's preflight for the type it migrates to; in a verb's value only `payload.<name>` is the call's and reads no field, so a bare `payload`, a longer path and one inside a function read the record's field of that name and are refused on a type without it and taken on one that has it, and a member the verb's payload does not have is refused, naming it (lakelands#470); `setting`, `now`, `today` and a guard's `retired` read no field and are accepted

**relation renamed**

- `a_migration_renames_a_relation_and_keeps_its_edges` — 035, extended (lakelands#429): a migration that names a relation's rename keeps its edges: the report lists the relation with its carrier and the link records that move, and no drop; the apply appends the `migrated` and the new link type's declaration and no event for a link; a walk by the new name answers the edges with their fields, a walk as of before answers them by the old name, and the links are records of the new link type; as of the `migrated` itself the walk answers them already, and the new link type is found from its declaration, one position later, on
- `a_grant_on_a_link_type_follows_its_relations_rename` — 006, extended (lakelands#429): a grant on a link type holds its records through the relation's rename, in both directions of time and only to take away: a principal kept from the old link type by a default alone is kept from the new one, by `find` and in a walk's link fields; one kept from the new name is kept from the old one as of before; a permission on the old name gives nothing under the new; and a field granted `write` under the name borne with a default of none under the earlier name is none
- `a_rename_that_would_change_or_lose_edges_is_refused` — 035, extended (lakelands#429): a rename is refused as invalid, naming the relation, and nothing is appended, when its first name is no relation of the type or its second none of the new shape, when it changes the relation's link fields, when the same migration re-keys records, when the link type of the new name still holds the links of an earlier relation, and when a reference field or a relation of another type names the link type that would move: that type is named to one `describe` shows it to, and to another only that a declaration refers to it
- `a_trigger_and_a_set_on_the_link_type_follow_the_rename` — 035, extended (lakelands#429): a trigger on a renamed relation's link type and an object set of it are declared again by the lake under the new link type, after its declaration, in the migration's append, and the report lists them as carried; the set's members stay its members, and a link written or removed under the new name fires the trigger and leaves the set
- `every_carrier_keeps_its_edges_through_renames_and_a_rebuild` — 035, extended (lakelands#429): a relation carried by a `via` field, by link records or derived from matching values keeps its edges under its new name, and the report says how each is carried and how many links move; the walk backwards by the relation's inverse name follows; the `migrated` event is written in format 11 and a migration that renames no relation leaves the format where it was; renamed a second time the links are records of the third name's link type alone; a read as of a position between answers by the name borne there; and a rebuilt projection answers all of it as the live one did
- `the_newest_chain_of_a_link_types_names_continues` — 006, extended (lakelands#429): a relation renamed onto a name, removed, and another renamed onto that name and renamed on: the grants on the first relation's link type do not follow the second relation's links, and the grants on the second's do, at the head and under every name
- `a_proposed_rename_is_listed_logged_and_accepted` — 013 and 035, extended (lakelands#429): a proposed migration that renames a relation is listed to its reviewer with the pair and the preflight's report of it, and to one kept from the type with neither; the `write_proposed` event holds the report whole; accepted, the relation keeps its edges under the new name
- `a_name_two_chains_bear_is_held_by_the_one_that_took_it_last` — 006, extended (lakelands#429): where two chains of a link type's names bear one name, the grants follow the chain that took it last, whichever was begun first: a principal kept from a relation's links is kept from them under a name another relation bore before and was removed under; and as of a position where two chains begin at one name, a principal kept from either is kept; a relation renamed back to a name it bore is kept from as before
- `a_rename_onto_a_standing_link_type_keeps_the_link_fields` — 035, extended (lakelands#429): a removed relation's link type stays declared; a relation renamed onto that name, or back onto a name it bore before a link field was added, keeps every link field of its links, a required one too: the standing link type is declared again in the renamed relation's shape, and a rebuilt projection answers the same; a rename onto a name whose standing link type a set reads a field of, which the renamed relation lacks, is refused, naming the set
- `the_vectors_of_moved_links_are_found_under_the_new_link_type` — 035, extended (lakelands#429): a link's vector field is searched under the new link type after its relation's rename, as it was under the old, live and after a rebuild
- `a_rebuild_recomputes_the_links_a_rename_moved` — 035, extended (lakelands#429): links written in an earlier page of the log, their relation renamed, and then a derived link field of it declared with another expression: every link is computed again under the new link type, by the live fold and by a rebuild, which folds the rename and the declaration in one page
- `the_old_link_type_takes_no_link_after_the_rename` — 035, extended (lakelands#429): after a relation's rename an ingest into the link type of its old name is refused as invalid, naming it, since no relation carries it; an ingest into the new link type is an edge of the renamed relation

**aggregates**

- `an_aggregate_over_no_records_answers_zero` — the read contract and 011: an aggregate over an empty set answers one group with a count of 0 and a sum of zero, kept at the field's scale
- `a_count_answers_as_many_records_as_a_full_find_walk` — the read contract and 029: `find` with only `count()` answers as many records as a walk of every page of the same `find` returns, with no filter, a filter the engine applies and one only the store evaluates, at `valid_at`, at an earlier position and over a view, for a principal with the `restricted` grant and one without; a superseded state, a retired record and, without the grant, a restricted one are not counted

**date bucket**

- `find_groups_records_by_date_bucket_for_every_unit` — 017, extended, and 029: find groups by a named date_bucket key for day, week (ISO, from Monday), month, quarter and year, across a year's end and a leap day, with the same groups, counts and exact sums on every engine, the bucket answered under the key's name as its starting instant, and the records without a date in the null group, last
- `a_where_selects_the_records_of_one_date_bucket` — 017, extended: a where comparing date_bucket(x, 'month') with an instant selects exactly that month's records; a unit outside the five is refused as invalid, naming it
- `a_declaration_may_call_date_bucket_and_from_it_the_lake_writes_format_3` — 017, extended, and log-format §6: a declaration may call date_bucket (a derived field, a view's where, an object set's filter); the event that first declares such a call is written in format version 3, as is every event after it, and a derived bucket answers finds by it
- `find_groups_records_by_windows_of_a_width_from_an_origin` — 017, extended (lakelands#559), and log format §6: date_bucket(x, width, origin) answers the start of the window of that width holding x, the windows counted from the origin in both directions, the same on every engine; a width without its origin, a width that is not above zero, a quoted width and a calendar unit with an origin are refused as invalid, each naming what to write; a declaration that calls it by a width is written in format version 11, as is every event after it

**durations**

- `a_duration_divided_by_a_unit_is_a_number` — 017, extended: a duration divided by a duration is an exact number, a duration times or divided by a number is a duration, and floor(n) is the whole number below; so (paid_on - due_on) / 1d answers the days between two instants as a group key, in a where and in a derived number field, the same on every engine; log format §6: the declaration that calls floor is written in format version 11, as is every event after it
- `a_derived_field_its_expression_can_never_fill_is_refused` — 028, extended: a derived field whose expression is certainly of a kind its field never holds is refused as invalid on that field by declare, add_field and a migration, naming both kinds, and nothing is appended; a text into a number field and an expression whose kind depends on the record are taken
- `what_can_never_be_evaluated_is_refused_and_a_records_own_failure_is_not` — 028, extended: an operation between two kinds that has no meaning is refused as invalid in a find's where and group key and a pattern's where, and in a trigger's when, an object set's where and a verb's requires and value, naming the operation; a reader kept from a field it names is answered as for a field the type lacks; an evaluation that fails for one record's value refuses nothing: that record does not match, and its key is null

**derived relations**

- `a_walk_over_a_derived_relation_finds_exactly_the_matching_records_both_ways` — 044: a pattern step over a derived relation reaches exactly the records whose matched fields hold the same value, out and in, over one pair and over two, at the head and as_of an earlier position; `'010'` does not match `'10'`
- `changing_a_matched_field_moves_the_edge_at_that_position` — 044 and 009: changing a matched field moves the edge at the position of that write: as_of before it the old record is reached, from it on the new one
- `a_null_never_matches` — 044: a matched field that is null or absent on either side connects nothing, a null to a null included
- `a_principal_without_read_on_a_matched_field_is_refused_naming_it` — 024, 044 and 006: a walk over a derived relation, a step and a measure alike, is answered as for a field the type does not declare, naming the field, when the principal may not know of a matched field on either side
- `degree_and_components_over_a_derived_relation_equal_those_over_the_same_links` — 026 and 044: `degree` and `components` over a derived relation answer what they answer over the same connections written as link records
- `a_derived_relation_is_declared_not_migrated_and_never_moves_carrier` — 007, 027, 035 and 044: changing a derived relation's match is taken by `declare` and leaves the schema hash byte-identical; a relation carried by link records or by `via` is not redeclared as derived, nor a derived one as carried, each refused naming the relation
- `a_match_pair_of_two_kinds_is_refused_naming_both` — 044: `declare` refuses a match pair whose fields are of different storage kinds, naming both fields, since identity would never match; a derived relation with `via` or fields of its own is refused too

**graph measures**

- `degree_counts_each_records_links_to_ends_it_may_see_highest_first` — 026, 029 and 035: `degree` counts each start record's links over the steps' relations in each step's direction, a via reference and a link record alike, but not a link to a retired end or, without the grant, a restricted link; highest first, ties by key, paged by limit; a start by key or by `where` counts those records only, and a key no record has counts none
- `components_partition_what_the_start_reaches_largest_first` — 026 and 029: `components` puts every record reached from the start over the steps' relations, both ways, in exactly one component, across types; components largest first, ties by their least (type, key), members by (type, key); a start by key or by a search reaches from those records only
- `a_measure_reads_the_graph_at_the_position_it_is_asked_at` — 009 and 026: a measure `as_of` an earlier position counts the links live there, not a link written later
- `components_past_the_lakes_max_graph_nodes_are_refused_too_large` — 018 and the location's limits: `components` whose records, over all components, would number more than `max_graph_nodes` are refused as too_large naming the ceiling, and nothing partial is answered
- `a_measure_refuses_depth_where_to_order_by_and_one_way_components` — 018: a measure refuses a step `depth` or `where`, `to`, `order_by`, and a `components` step that does not walk `both`, each as invalid, rather than answer less than it seems to
- `a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes` — 026, extended, and the location's limits: a measure's `limit` may go up to the lake's `max_graph_nodes` where that is above `max_limit`, so one page answers the rows its small pages answer in turn, with no cursor; above that bound, and above `max_limit` for a `rows` projection, a `limit` is refused as invalid naming the bound

**vector index**

- `an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor` — 043, 025 and 029: a search over a vector field the `vector_index.<type>` setting names says `ranked_by: index`, ranks the index's candidates by the one cosine with ties by key, answers the same bytes when asked again, and finds on average at least the engine's floor of the brute-force reference's hits for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added; an engine without such an index answers the reference, a recall of 1; a vector search fused over several fields says `index` when any of them is named

**embeddings**

- `a_write_embeds_its_text_and_records_the_vector` — 022, extended, and the log format: the store embeds the joined `from` text after the write, records the vector as output_recorded under embed:<model> naming the field, and the field holds it
- `vector_coverage_counts_embedded_and_due_records_at_the_head` — 022, extended, and lakelands#147: vector_coverage answers, per embedding field at the head, the live records holding a vector and those whose text is not embedded (never embedded, failed or waiting for a retry), not counting a record without text; a redeclaration's retry and a retire move the counts; an unknown type is not found and a type without embeddings answers none
- `the_same_text_is_embedded_once_whichever_record_asks` — 002 and 022, extended: a vector recorded for a text under a model is read back for every later record with that text, and after a record's text changed and came back; the log holds one output per distinct text
- `a_rebuild_reads_the_vectors_back_without_embedding` — 001 and 002: a rebuild folds the recorded vectors; the fields read as before and nothing is embedded
- `an_embedding_whose_model_the_backend_does_not_make_is_refused_at_declare` — 022, extended: an embedding is declared only where the bound backend names its model and makes vectors of its dimensions; otherwise the declaration is refused as unbound
- `an_embedding_without_a_backend_is_refused_at_declare` — 022, extended: with no embedding backend bound, declaring an embedding is refused as unbound, naming the model
- `a_changed_embedding_is_due_on_every_live_record_and_an_unchanged_one_embeds_nothing_new` — 022, extended: a change of `from` or `model` is a redeclaration that makes the field due on every live record, one call per distinct text; declaring it again unchanged reads the recorded vectors back
- `a_record_with_nothing_to_embed_holds_no_vector_and_embeds_nothing` — 022, extended: with no `from` field holding text, the field holds no value and keeps metadata without an error, and nothing is called; the text written later makes it due
- `a_failed_embedding_clears_the_field_and_a_redeclaration_retries_it` — 022, extended: a failure is recorded, clears the field and puts its reason in the metadata; `recompute` calls again for that text once, the later output holds and the vector fills every record holding it
- `an_embedded_vector_recomputes_the_derived_fields_that_read_it` — 012 and 022, extended: the field a vector fills is changed through the fold's ordinary path, so a derived field reading it is recomputed
- `a_hybrid_search_over_an_embedding_the_bound_backend_does_not_make_is_unbound` — seams, EmbedBackend: a hybrid search over an embed field whose model or dimensions are not the bound backend's is refused as unbound when it is made, not ranked with vectors of another space
- `a_due_embedding_waits_while_no_backend_makes_it` — seams, EmbedBackend: a due embedding whose backend is gone or makes another model or other dimensions is not made; the field waits, with neither a value nor metadata, until its record is written again or `recompute` names its type, with the backend bound
- `an_embedding_that_is_not_a_derived_vector_of_its_own_text_fields_is_refused` — seams, EmbedBackend, and TypeDecl::embedding_problem: an embed field is a derived vector with dimensions, made from distinct text fields of its type other than itself, sensitive where a source is; anything else is refused as invalid, whatever backend is bound
- `no_request_carries_more_than_sixty_four_texts` — seams, EmbedBackend: the store sends the due texts of one model in batches of at most 64, however many records one declaration makes due, so its memory per drain stays bounded

**aging**

- `a_rule_moves_only_the_records_holding_from_whose_condition_holds` — 036: `age` moves every record of the rule's type, restricted ones included, whose field holds `from` and whose `when` holds, and no other
- `aging_a_date_twice_appends_nothing_the_second_time` — 036 and log-format §3: every move has an id from the rule, the record and the date, and a second `age` for the same date appends nothing
- `today_in_a_rule_is_the_date_aged_not_the_clock` — 036: inside a rule's condition `today()` is the date `age` was handed, never the clock of the process running it
- `a_condition_that_cannot_be_evaluated_leaves_the_record` — 036: a `when` that cannot be evaluated for a record leaves it as it is, is counted as a failed guard, and the run goes on
- `a_move_is_written_by_the_system_as_its_rule` — 036 and seams: a move is a `record_upserted` of origin `system` by `system:lakelands`, its procedure `aging:<rule>` at the rule record's version, scheduled, with no cause; a trigger reacts in the same append
- `a_rule_that_does_not_fit_is_refused_naming_its_field` — seams: a rule is refused as invalid, naming the rule field at fault, when `on` names no type, a `sys.` type or a view, `field` is derived or part of the key, `from` is no value of the field, `to` equals `from`, or `when` reads `previous`
- `rules_run_in_name_order_one_append_each` — seams: `age` runs the rules in rule-name order, each rule's moves one append, and reports them in that order
- `age_at_a_time_other_than_midnight_is_refused` — seams: `age` is handed a date, at UTC midnight; any other time is refused as invalid, naming `today`, and appends nothing
- `a_rule_that_no_longer_fits_is_skipped_and_named` — seams: a rule the lake's shape no longer fits is skipped, named in the report with its reason, while the other rules run
- `a_rule_needs_write_on_its_field_and_read_on_what_when_reads` — 036, 024 and 006: a rule is refused, naming the field, unless its author may write the field it moves and read every field its `when` reads: as not granted, or as for an undeclared field where its author may not know of the field; the run carries that authority
- `a_move_the_store_would_refuse_leaves_the_record` — 036: a move the store would refuse for a record (its `to` a reference no longer live) leaves the record, is counted as a failed guard, and the other rules run
- `a_rule_under_a_retention_hold_is_live_and_runs` — 036 and seams, SystemVocabulary: a hold withholds a record from reads, it does not stop it; a restricted rule record runs
- `a_rule_reads_the_setting_at_its_run_and_a_replay_reads_its_moves` — seams, SystemVocabulary: setting() in `when` reads the lake's settings as a filter does, with no grant on sys.setting asked of the rule's author, as of the head at the run; the move is an event, so a replay never evaluates `when` again
- `a_rule_is_skipped_when_its_field_is_dropped_or_its_value_no_longer_admitted` — seams, SystemVocabulary, and 027 extended: a rule whose field a migration dropped, or whose `from` or `to` a tightened value type no longer admits, is skipped and named, never run half

**purge**

- `a_purge_retires_every_record_the_source_brought_in_and_no_other` — 039: a purge retires, in one append, every live record the source brought in across every type it reached, and leaves other sources' and authored records; the report counts per type
- `a_purged_record_is_still_read_as_of_before_the_purge` — 039 and 007: a purge retires, it does not erase: a read as of before it sees the records, and each retirement continues the record's version
- `a_record_another_hand_also_wrote_refuses_the_whole_purge` — 039: a record the source brought in that a principal or another source also wrote refuses the whole purge as invalid, naming it, and nothing is appended
- `derived_and_system_values_make_no_owner` — 039: a derived value and a value the lake itself wrote (an aging move) make no owner; a cleared authored field co-owns nothing
- `a_restricted_record_is_purged_too` — 039: a purge is the host's act over its lake, so a record under a retention hold is retired with the rest
- `a_type_that_keeps_no_provenance_is_named_and_not_searched` — 039 and 010: a type declared without per-field provenance cannot say who brought its records in; the purge names it as unsearched and leaves its records
- `a_second_purge_appends_nothing_and_the_feed_can_come_back` — 039 and log-format §3: a second purge of the same source retires nothing and appends nothing; the same content ingested after a purge is a new ingest
- `each_purged_record_reacts_as_a_retire_does_in_the_same_append` — 039 and 004: each retirement of a purge is written by `system:lakelands`, origin `system`, context `direct`, and fires the triggers and leaves the object sets a `retire` would, in the same append, caused by that retirement
- `another_sources_links_to_a_purged_record_stay_and_the_walk_skips_it` — 039, 035 and 032: the purged source's own link records are retired with it; link records another source brought in, pointing at purged records, stay, and a walk skips the retired end
- `a_record_whose_values_from_the_source_were_all_cleared_stays` — 039: whose a record is, is read over the fields that hold a value: a record whose values from the source were all cleared holds nothing of it and stays, whether the source brought it in or set and cleared a value on another source's record, while a record still holding one is purged
- `a_co_owned_purge_names_the_count_and_the_first_ten` — 039 and seams: a purge refused because records are co-owned names how many there are, and the first ten, never more
- `a_value_a_backing_filled_makes_no_owner` — 039: a backed output is the lake's own computation, like a derived value; it makes no owner, so the source's purge retires the record
- `a_field_that_declares_initial_makes_no_owner` — 039, extended, and 050 (lakelands#551): a field that declares `initial` makes no owner, whoever wrote it last: a record whose such field an author moved goes with its source's purge, whole or of one type, and the preflight and the purge name in `moved`, per type and field, how many such records they take and their first keys; another authored field still refuses the purge whole
- `a_preflight_appends_nothing_and_a_purge_that_repeats_its_id_is_stale_once_the_lake_moved` — 039, extended: `purge_preflight` appends nothing and answers the report the purge then answers, under an id; it is refused as the purge is for a co-owned record; a purge that repeats the id is refused as invalid, appending nothing, once the lake has moved or when the id is another source's, and otherwise purges
- `a_purge_names_the_records_outside_it_that_refer_into_it` — 039, extended, 026 and 035: the preflight and the purge name, per relation, the live records the purge does not take that refer to a record it takes, by a `via` field or as a link record with an end inside, restricted ones included, by type and key only: how many, and the first ten in key order; a retired record, one the purge takes itself and a relation derived from matching values are not named, and the purge is not refused
- `a_purge_that_names_a_type_retires_what_the_source_brought_into_it_alone` — 039, extended: a purge that names a type retires, in one append, the live records the source brought into that type, and leaves the source's records of its other types and other sources' records of that type; the report names the type, counts it alone and names no type unsearched
- `a_co_owned_record_refuses_a_typed_purge_only_in_the_named_type` — 039, extended: a record of another type of the source that another hand also wrote does not refuse a purge that names a type; one in the named type refuses it whole as invalid, naming it and the type, and nothing is appended
- `a_typed_purge_names_the_sources_own_records_that_still_refer_into_it` — 039, extended: a typed purge leaves the source's records of its other types, and its report names those that refer to a record it takes, by a `via` field, as it names any other record outside the purge
- `a_typed_purge_takes_the_links_of_the_named_types_relations_and_no_other` — 039, extended, and 035: a typed purge takes the source's link records of the relations the named type declares with its records; the source's link records of another type's relation that end at a purged record stay and are named in the report
- `a_preflight_of_a_type_and_one_of_the_source_whole_answer_ids_of_their_own` — 039, extended: at one head the preflight of a source whole and the preflight of the source from one type answer different ids; each is refused as invalid by the other's purge, appending nothing, and taken by its own
- `a_purge_from_a_type_it_cannot_take_from_is_refused` — 039, extended, and 010: a purge and a preflight that name a type that is not declared are not found; ones that name a view, or a type that keeps no per-field provenance, are refused as invalid saying why; nothing is appended

**value types**

- `a_value_type_that_accepts_more_is_declared_and_no_record_changes` — 027: a value type that accepts everything it did (a value added to `one_of`, a `constraint` removed) is declared at its next version, and no stored record changes
- `declare_refuses_a_value_type_that_accepts_less_or_reshapes` — 027: `declare` refuses a value type that would accept less, naming `migrate`, and one whose `kind` or `scale` changes; `migrate_value_type` refuses that change too, since no value-type migration makes it; nothing is appended
- `the_preflight_counts_what_would_not_fit_over_every_use_and_history` — 027: the preflight of a value type checks every value stored under it, over every field of every type that uses it, retired and restricted records included, counts the records and the misfits per use, names the first, and appends nothing
- `applying_a_tightening_some_value_does_not_fit_is_refused` — 027: a value-type migration some stored value would not fit is refused as invalid, naming how many, and appends nothing
- `a_tightening_every_value_fits_is_one_declared_version` — 027 and log-format: a value-type migration every stored value fits appends one `declared` at the next version and changes no record; later writes are checked against it
- `a_value_type_report_from_before_the_lake_moved_is_refused` — 027: a value-type migration is applied only against the report of the lake as it stands; after the lake moved it is refused as invalid
- `a_value_type_migration_needs_the_schema_grant` — 027 and 006: both the preflight and the apply of a value-type migration refuse a principal without the `schema` grant; migrate_apply, migrate_value_type_apply, declare and apply ask for it before they judge what they are given (028 and 027, extended)
- `an_apply_that_loosens_one_value_type_and_tightens_another_is_refused_whole` — 028 and 027: `apply` of a set that loosens one value type and tightens another is refused whole, naming `migrate`, and appends nothing
- `removing_a_constraint_or_a_format_is_declared_and_changing_a_constraint_is_refused` — 027 extended and seams: `declare` takes a value type whose `constraint` or `format` is removed, since no stored value can stop fitting it; a `constraint` changed may reject a stored value, so `declare` refuses it, naming `migrate`
- `a_trigger_or_set_on_a_value_the_type_no_longer_admits_never_holds` — 027 extended and seams: the preflight checks stored values, not the conditions that mention them; a trigger's `when` or an object set's `where` comparing with a value a tightening removed stays declared and simply never holds afterwards

**rekey**

- `a_rekeys_preflight_counts_what_moves_with_it_and_appends_nothing` — 027: the preflight of a re-key counts the records it moves, retired and restricted included, the types keyed by a reference to them, their link records, and the reference values it rewrites, and appends nothing
- `two_records_that_would_take_one_new_key_refuse_the_rekey` — 027: a re-key under which two records would take one key is not lossless, names the key, and is refused, appending nothing
- `a_record_without_a_new_key_value_refuses_the_rekey` — 027: a re-key under which a record has no value for a new key field is not lossless
- `a_rekey_moves_every_record_with_its_values_version_and_provenance` — 027 and log-format §2.3: a re-key moves every record to its new key with its fields, version and per-field provenance; at the migration's position the new key is found and the old one not, and a read before it still finds the old key
- `a_reference_to_a_rekeyed_record_names_its_new_key` — 027 and 032: a reference field in another type that named a moved key names the new one, and a walk over its relation reaches the record
- `a_link_to_a_rekeyed_record_is_rekeyed_and_walked` — 027 and 035: a link record keyed by a moved end is moved to its new pair key, and a walk over its relation reaches the re-keyed record
- `a_rekey_that_retypes_a_link_field_moves_and_converts_its_links` — 027 and 035: a migration that re-keys a type and retypes a field of its relation appends the link type's migration after its own; a link is at its new pair key from the first on and holds the converted value from the second on
- `a_type_keyed_by_a_reference_to_a_rekeyed_type_is_rekeyed_with_it` — 027: a type whose key fields reference a re-keyed type is re-keyed in the same migration
- `an_old_key_is_ended_and_a_new_write_follows_the_new_rule` — 027 and log-format §2.3: after a re-key a write at an old key whose key fields make another key is refused as invalid, naming it; a write at a new key continues its version; and a write of the bare old key string, its key field filled from it, is a new record at version 1, by an event that says it made it (lakelands#572)
- `a_rekey_is_written_at_the_lakes_format_and_the_log_verifies` — log-format §6: the re-keying `migrated` event and every event after it carry the lake's `v`, 11 since the records it moves were made; the log verifies, and a rebuild answers every read as before
- `a_lake_writes_format_version_1_until_its_first_record_is_made` — log-format §6 (lakelands#572): a lake carries `v` 1 on every event before the first that says it made a record and 11 on it and every later one, a migration that keeps the key included
- `a_migration_that_takes_every_key_field_away_is_refused` — 027: a migration is refused as invalid when it takes every key field away, names a key field that is not required or of a kind a key cannot be made of, or would re-key a view
- `a_rekey_report_from_before_a_write_is_refused` — 027: a re-key is applied only against the report of the lake as it stands; a write between the preflight and the apply, one that changes a move, makes it refused
- `object_set_membership_moves_with_a_rekeyed_record` — 027 and 004: a moved record keeps its object-set memberships at its new key: a write that keeps it in a set changes nothing, and retiring it leaves the set at the new key
- `a_rekeyed_type_migrates_again` — 027 and log-format §2.3: the ended keys a re-key leaves are no records: a later migration that keeps the key, a value-type tightening, and a second re-key over them all apply
- `a_key_is_made_after_every_key_it_reads_has_moved` — 027: a type keyed by references to two types a re-key moves, a link type among them, takes its new keys after both have theirs, whatever the order of their names: each key is what its fields make, a relation carried by links is walked out and in as before, and the report lists the types in the order their keys are made
- `a_rekey_of_keys_made_of_keys_it_moves_is_refused` — 027: a re-key that would move a type keyed by a reference to itself, or types keyed by each other, is refused as invalid naming them, and appends nothing: no order makes such keys
- `a_rekeys_report_counts_a_reference_field_the_migration_renames` — 027, 013 and 024: the references a re-key rewrites are counted for a field the same migration renames, under its new name, and that entry is answered only to a reader who may know of the field in the shape the migration leaves

**export**

- `an_export_writes_one_file_per_segment_of_canonical_lines` — 030 and log-format §4.2: an export writes one file per segment, named by its first position in 20 digits, holding each event's canonical bytes and a line feed, and the sealed file's digest is the one the checkpoint after it lists
- `an_export_verifies_to_the_through_and_hash_the_lake_answers` — 030: an export verifies from its files alone to the position and hash the lake's own verify answers; its manifest names the same pair and every file with its digest, and is never trusted: with a wrong manifest or none the export verifies the same
- `a_lake_exports_both_format_versions_as_written` — 030 and log-format §6: a lake exports its version-1 events, written before its first record was made, and its version-11 events as they were written, a re-key among them, and the export verifies
- `a_later_export_repeats_the_sealed_files_and_extends_the_active_one` — 030: an export reads through the head as it stood when it began; a lake never checkpointed exports one file 00000000000000000000.jsonl, and a later export has the earlier sealed files byte for byte and an active file the earlier one begins
- `an_export_into_a_directory_that_holds_anything_is_refused` — 030: an export writes only into an empty or absent directory; into any other it is refused as a failed precondition naming the directory, and writes nothing
- `a_damaged_export_is_refused_naming_the_position` — 030 and log-format §2.6 and §4.2: verifying an export refuses a changed byte, a space inserted, a missing last line feed, a format version below the first, past the newest or going down, a known_at going back, a checkpoint naming the wrong position before it, a missing file, an empty log/, a file too many, a file not named in 20 digits and two lines out of order, and names the position where the files stop being the log
- `an_export_imported_into_a_fresh_lake_answers_as_the_lake_it_came_from` — 030 and log-format §4: an export imported into a fresh lake of the engine holds every event unchanged at its position, of every format version the lake wrote (annotations included), so the lake answers as the one exported (the head, verify, the feed from position 0, every type's records with provenance at the head and at an earlier position, describe) and exports the same files byte for byte; its next writer takes a lease greater than every fence in the log and continues it
- `an_import_is_refused_for_a_damaged_export_another_lake_or_a_lake_holding_events` — 030: an import verifies the whole export before it writes: a changed byte in a segment is refused naming its position, and an export of another lake (naming both lakes) and a lake that holds events (naming its head) are refused as failed preconditions; nothing is written into the lake it refuses
- `an_import_refuses_a_declaration_member_it_does_not_know_naming_it` — 028 and log-format §2: an export whose `declared` event carries a member its declaration does not take, its hash made right, is refused on import, naming the member and the declaration
- `a_cycle_of_views_from_an_imported_log_is_refused_on_read` — 018 and 030: a log can bring in a cycle of views over `source` that `declare` would have refused; a read of a view on it is refused as invalid, naming the chain, the process goes on, and a view off the cycle reads as before
- `an_import_refuses_a_context_carrying_cause_naming_it` — 021, extended, and log-format §1.1: a host's cause is the envelope's and never written inside `context`; an export whose line carries `cause` in its `context`, its hash made right, is refused on import naming the member, and a write through the doors carries no cause; so is one whose envelope carries the origin `initial`, a field's provenance alone (050, lakelands#551)

**compact**

- `a_compacted_lake_answers_every_read_as_before` — 040: after a compact of a churned lake (overwrites, a retire, a restrict, a derived field, a view, recorded outputs, embedded vectors, a proposal, a value-type migration, a re-key and a rebuild), every read answers byte for byte as before: every record with provenance at the head and as of every position, a lexical and a vector search, a walk and the proposals; a retry of a keyed write is still recognised and appends nothing; an input the lake recorded is not called again after the compact, and a rebuild after it answers as before
- `compact_appends_nothing_and_exports_the_same_bytes` — 040: a compact appends nothing: the head, the whole feed and verify's answer are as before, the report names the head unmoved, and an export before and after is the same, file for file and byte for byte, manifest included
- `a_lake_compacts_again_and_writes_on` — 040: a lake compacts again and nothing changes either; a write after it continues the record's version and the log's position
- `writes_made_while_a_compact_runs_each_land_once_before_or_after_it` — 040 and 008: writers writing while compacts run each land once, before or after a compact; every write answers a position its value is visible from, the log grows by the writes alone, and every read of the churned lake answers as before

**settings**

- `a_setting_is_read_as_it_stood_at_the_position_read` — seams, SystemVocabulary: setting('key') in an expression reads the sys.setting record with that key as of the position being read
- `a_restricted_setting_stays_what_the_lake_computes_with` — seams, SystemVocabulary (lakelands#583): restricting a setting hides its record and never its value from the lake's computations or from an expression: a derived field written while the writer stays open, after the lake is opened again and after a rebuild reads it, a read's `where` and a view's `where` and measure read it, and a reader without the `restricted` grant is not listed the record and is answered by `setting('key')` in its own `where` what the lake computes with
- `a_field_that_reads_a_setting_is_as_of_its_records_last_write` — seams, SystemVocabulary and 048 (lakelands#523): a derived field whose expression reads a setting is computed with the setting of its record's last write; the setting changed and the type declared again with one more field that is not computed leave it as it was, live and after a rebuild, and a write of the record brings it up to date

**fusion**

- `a_record_the_read_may_not_see_moves_no_hybrid_score` — 046 and 024: a hybrid search ranks each list among the hits the read may see, so a restricted record, or one its `where` leaves out, ranked above a visible hit moves neither that hit's rank nor its score
- `every_page_of_a_hybrid_search_reads_one_ranking` — 046 and 029: a hybrid search fuses each list to the lake's `max_limit` ranks whatever the page, so its pages through a cursor answer the one ranking a single page answers, keys and the scores' bits
- `a_search_that_reads_no_record_to_rank_answers_what_reading_them_answers` — 046: at the head, a hybrid search by a principal who sees every live record, without `where` or `valid_at`, ranks without reading a record, and answers the keys and the scores' bits the same search answers under a `where` that holds for every record, with restricted and retired records in the lake
- `a_search_fuses_its_rankings_each_by_its_weight` — 046, extended, and 024: a vector search that names further fields in `also` ranks each field among the hits the read may see and fuses the rankings by weighted reciprocal rank, each adding `weight / (60 + rank)` in the order named, an absent weight being 1; it answers the keys and the scores' bits of that rule on every engine, a record without a vector in a field gains nothing from it, and a restricted record first in every ranking moves no score; a hybrid search weighs its text (`text_weight`) and its vector (`weight`) by the same rule, and weights of 1 answer what no weights answer
- `a_weight_that_cannot_weigh_or_a_field_named_twice_is_refused_naming_it` — 046, extended: a weight of 0, below 0 or above 1 000 000, a `weight` on a vector search that names no other field, and a vector field named twice are each refused as invalid, naming the field or `text_weight`, before the principal's rights to the field are looked at

**vocabulary**

- `a_host_type_is_declared_when_a_writer_opens_the_lake` — 003 and seams, SystemVocabulary: each writer open declares the host's sys. types the lake may hold and does not hold yet; a type added later reaches the existing lake, and a withheld one never does
- `a_vocabulary_naming_a_reserved_type_refuses_the_writers_open` — 003, 035 and seams, SystemVocabulary: a host vocabulary naming a type under sys.link. or one of the store's own four (sys.source, sys.provenance, sys.setting, sys.aging_rule) is refused at the writer's open as invalid, naming the type as reserved; nothing is appended, and the lake never holds it

**verbs on type**

- `a_verb_on_its_type_runs_under_its_types_grants_and_records_its_output` — 013, extended: a `low` external verb declared with `scope: type` is described with that scope and called with no key by a principal with write on its type; it answers `applied` with its executor's output, its `verb_called` carries no key and is written in log format 8, and the output is recorded as `output_recorded` caused by it
- `a_verb_on_its_type_at_medium_risk_is_proposed_and_runs_when_accepted` — 013, extended: a `medium` verb on its type is recorded as a proposal whose call carries no key and runs nothing; accepting it runs its executor once and records the output, caused by the acceptance
- `a_feed_filtered_by_type_passes_none_of_a_verbs_events` — 004 and seams §ChangeFeed: a verb's events name no type, even for a verb on its type, so a feed filtered by that type passes none of `verb_called`, `write_proposed`, `write_accepted` and `output_recorded`, which a feed with no types passes; `proposals` filtered by the type lists the proposed call
- `a_verb_on_its_type_without_write_on_its_type_is_not_granted` — 006 and 013, extended: a principal that may write another type but only read the verb's type is refused not_granted when it calls the verb on that type, and nothing is appended
- `a_verb_on_its_type_retried_with_its_key_answers_the_first_outcome` — 013, extended, and 015: a verb on its type called again with the same idempotency key answers the first outcome, appends no second `verb_called` and runs its executor once
- `a_key_that_does_not_fit_the_verbs_scope_is_refused_naming_the_verb` — 013, extended: a key passed to a verb on its type, none passed to a verb on a record, and an expected version with no key are each refused as invalid naming the verb, before grants are read, and nothing is appended
- `a_verb_on_its_type_with_a_write_effect_or_requires_is_refused` — 013, extended: a verb declared with `scope: type` and a `write` effect, or with `requires`, is refused as invalid naming the verb, since it has no record to set or to read, and nothing is appended

**concurrency**

- `concurrent_writes_each_land_once_in_one_order_of_the_log` — 008: many callers write through the lake's one writer; every write lands once, the log's positions follow one another without a gap, every answer names a position the write is visible from, and the chain verifies
- `writes_and_a_declaration_made_at_once_are_ordered_as_one_log` — 008 and 027: a declaration made while others write takes its place in the same order; every write after it sees the new field at its position, none before it does
- `a_read_at_a_position_answers_the_same_during_writes_and_after` — 008 and 009: a read at a position sees the lake as it was there, whoever is writing; `find` and `get` at one position agree, and answer the same when asked again later
- `two_feed_readers_polling_during_writes_see_one_log` — 004 and 008: the feed is the log; two consumers polling from one position while others write see the same events, in the same order, each once
- `concurrent_writes_see_their_own_filled_field_and_one_input_is_called_once` — 008 and 022, extended: each write answers a position its own filled field is visible from, whoever else is writing, and one input is called once across all the writers
- `writes_made_while_a_rule_runs_land_before_or_after_its_append` — 036 and seams, SystemVocabulary: `age` holds the writer's lock for a rule's whole run, so a write made meanwhile lands before or after the rule's append, never inside it, and every move is made from the record as it stood before that append

**streaming**

- `a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it` — 016, the change feed and 030: ingest reads its source only as windows commit, and ingest, the feed read from the start, an export with its verification and its import into a new lake, each page of a walk of a type through find, the first pages of a walk ordered by an indexed field, with and without writes between them, each also reading its page and a few rows more, not the type, a rebuild, a reopen, a three-hop walk over link records, a three-hop walk over a derived relation, a hybrid search with k = 10 over every record's vector, without and with a vector index asked for, the ten records of highest degree over the whole type, and components refused at the lake's max_graph_nodes, hold as much memory at four times a lake's size as at its first size, give or take a quarter and 1 MiB, and the search answers what it must

**reactive depth**

- `a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing` — 021: a host writes each reaction one deeper than its cause; the store takes every write up to the lake's max_reactive_depth and refuses the next as too_deep naming the ceiling, an upsert and a retire alike and whatever origin the context names, appending nothing, while the log keeps the chain before it with each write's depth

**host cause**

- `a_reactive_write_with_a_cause_carries_it_on_each_of_its_events` — 021, extended: a write through a catalog whose context names a cause carries it as the envelope's `cause` on each event the write itself appends (an upsert's record, a verb's `verb_called`, an ingest's record), while the events the store derives keep the store's cause (a trigger's firing names its write, a verb's write names its `verb_called`); no event's `context` carries `cause`
- `a_cause_past_the_head_is_refused_and_appends_nothing` — 021, extended: a write whose context names a cause past the lake's head is refused as invalid naming both positions, appending nothing, after the door's own checks (016, extended); a cause at the head, and at position 0, is taken
- `the_reactive_ceiling_holds_on_every_door_that_writes` — 021, as extended on 2026-09-25 (lakelands#257): a context deeper than the lake's max_reactive_depth is refused as too_deep naming that ceiling, appending nothing, through ingest, accept, reject, declare, apply, add_field, migrate_apply and migrate_value_type_apply as through the record doors
- `every_write_door_checks_the_context_before_anything_else` — 021, extended (lakelands#344): a context past the ceiling is refused as too_deep before anything else a door checks, so a request that also breaks the door's own first rule (a principal without grants, a link type at an ordinary door, an undeclared type, relation, verb or proposal) is refused as too_deep on every write door; a principal of another lake is refused not_granted before that
- `a_triggers_ceiling_is_read_with_the_cause_at_the_append` — 021, extended (lakelands#375): a reaction naming a cause past the head is refused as invalid even when the next append puts a firing there; once the firing is in the log, a reaction past its trigger's ceiling is refused too_deep naming the trigger, and that ceiling is read at the append, after the door's own checks, while the lake's ceiling comes first

**triggers**

- `a_transition_guard_fires_on_open_to_closed_only` — 004, extended: a guard `previous(status) = 'open' and status = 'closed'` fires on open → closed and not on draft → closed, open → open or a record created closed (`previous` is null for it); `previous` outside a guard is refused as invalid naming it
- `a_guard_tells_a_retirement_from_an_update` — 004, extended: a guard `status = 'closed' and not retired()` fires on closing a record and on updating a closed one but not on retiring it, and a guard `retired()` fires on the retirement only, caused by it, as it does on a proposed retirement when it is accepted; the declaration calling `retired` is written in log format 7
- `a_revival_is_a_creation_to_a_guard` — 004, extended: an upsert of a retired record is a creation to a guard: `previous(f)` is null, `changed(f)` is true and `retired()` is false
- `retired_outside_a_triggers_guard_is_refused_naming_it` — 004, extended: `retired()` in an object set's filter, a verb's `requires`, a derived field or a find's filter, and `retired` called with an argument in a guard, are refused as invalid naming `retired`, appending nothing
- `a_reaction_past_its_triggers_max_depth_is_refused_naming_it` — 021, extended: a reactive write whose `cause` names a firing of a trigger with `max_depth` is refused too_deep naming the trigger when it is deeper than that ceiling, appending nothing, while the lake's own ceiling is higher; a reaction to another trigger, or naming no cause, at the same depth is taken
- `a_trigger_max_depth_above_the_lakes_ceiling_is_refused` — 021, extended: a trigger whose `max_depth` is above the lake's `max_reactive_depth` is refused as invalid naming the trigger and both numbers, by `declare`, `apply` and `propose`, and by `accept` once the lake is opened again with a lower ceiling; the lake's ceiling itself and 0 are taken

**after**

- `after_resumes_where_the_cursor_would` — 029: `after` names the position a cursor carries — a row's sort values and key — and continues strictly after it in the one order, at the caller's read position; it is refused beside a cursor or an aggregate, with a value count other than `order_by`'s, or with a value its key's field cannot hold

**describe**

- `describe_answers_the_declaration_whole` — 014 and 024: for the owner, a described type projects back onto its declaration byte for byte — every field's function, embedding, initial value, searchable, sensitive, label and description, and the type's label, language and view; a masked field shows its shape only, without its initial value (050); functions, triggers, object sets and views are answered as declared, to a principal who reads every field they name or holds the schema grant, as in force at the read position; a type that is shown is one its principal may find and get, also where every field is masked

### Through the engine seam

**log**

- `appended_bytes_come_back_unchanged` — log format §1.2: the engine keeps each event's canonical bytes and returns them unchanged, so the store can verify the hash chain over what it reads
- `a_stale_lease_cannot_append` — 008: every append is checked against the current fencing token, so a writer that lost the lease cannot fork the chain
- `a_held_lease_is_refused_at_once_naming_its_holder` — 008: a second writer fails at once with an unavailable error that names the lease's holder; it never waits
- `every_take_of_the_lease_has_a_greater_fence` — 008: the fencing token increases with every new writer, which is what makes an older token stale
- `an_append_with_a_gap_or_a_wrong_prev_is_refused_whole` — log format §4.1: an append whose positions do not continue the tip, or whose prev is not the hash before it, is refused, and none of it is appended
- `a_sealed_segment_is_digested_as_its_lines` — log format §4.2: a segment's digest is SHA-256 over its file's bytes, each event's canonical bytes followed by a line feed, whatever the engine keeps them in
- `releasing_a_stale_lease_leaves_the_current_holders_lease` — 008: a writer that lost the lease cannot release its successor's, so the successor goes on appending
- `reading_past_the_tip_or_with_limit_zero_answers_nothing` — seams §LakeEngine: read answers at most `limit` events from `from` on, nothing past the tip or for a limit of 0, and an empty lake has no tip
- `an_event_at_position_zero_with_a_fence_is_refused` — log format §4.1: the event at position 0 is written before any lease exists, under fence 0, and an engine refuses it under any other
- `an_import_keeps_each_events_fence_and_the_next_lease_is_greater` — 030 and log format §4.1: `import` stores events carried over from another lake with the fences they were written under, which never fall, refuses a fence below the one before it, and the next lease taken is greater than every fence imported
- `an_import_is_refused_once_the_log_holds_an_appended_event` — 030: `import` continues only a log that holds nothing but imported events; once anything was appended, an import is refused and nothing of it is written
- `sealing_under_a_stale_lease_is_refused_and_two_seals_list_contiguous_ranges` — log format §4.2: only the writer seals, and the sealed segments' ranges start at 0 and follow each other with no gap
- `a_seal_that_cannot_begin_the_next_segment_leaves_the_log_as_it_was` — log format §4.2: a seal closes the active segment and begins the next with its first event in one step; one whose events do not continue the log by hash or by position, that is handed none, or whose events are imported behind appended ones is refused as invalid, one whose `first` answers an error answers that error, and one under a lease no longer current is unavailable before `first` is asked; each seals nothing and leaves the tip, with no segment sealed and behind a sealed one, and the next append lands in the segment that was active
- `opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one` — seams §EngineOpener: reading or writing a lake that does not exist is unavailable, and create opens a lake that exists, as it is
- `a_read_only_open_creates_and_changes_nothing` — 008 and seams §EngineOpener: a reader opens a lake as it is; opening a missing lake read-only makes none, and a read-only handle writes neither the log nor the projection
- `a_dropped_writer_releases_its_lease` — 008: a lake is closed when its last handle goes, so a writer that ends without closing does not keep every later writer out
- `a_read_only_handle_refuses_the_lease_and_appends` — 008: any number of readers may open a lake, and none of them can take the lease, append or seal, so only the writer extends the chain
- `an_append_that_returned_is_read_by_the_next_handle` — seams, append: an append is durable before it returns, so the events are there, byte for byte, for a handle opened after the writer's handle was dropped without closing

**projection**

- `a_view_at_a_position_sees_the_states_then` — seams §Engine: the projection keeps every superseded state, so a view at an earlier position reads the lake as it was right after folding it
- `a_view_past_the_folded_position_is_refused` — seams §Engine: a view at a position the projection has not folded is refused as out of range, naming the head
- `a_rolled_back_transaction_leaves_nothing` — seams §Engine: the fold writes one transaction per append, so a crash or an abandoned append leaves the projection at a whole append
- `edges_walk_in_out_and_both` — 026: out gives the links a record declares, in the links that point at it, both their union with each link once; the store's walks are built on them
- `edges_over_many_keys_answer_each_key_as_it_answers_alone` — seams §ProjectionView (lakelands#227): edges over many records of one type answer one list per key, in the keys' order, each what the key alone answers, at the head and at an earlier position; a link between two of the keys is in both lists
- `get_many_answers_what_get_answers_key_by_key` — seams §ProjectionView (lakelands#227): get_many answers each record of the keys that exists at the view's position, retired and restricted ones included, once, in key order; a key with no record is left out
- `a_much_written_key_is_put_and_read_as_one_written_twice` — seams §Engine (lakelands#596): a key put forty thousand times answers at the head, at an earlier position, in a page and in a count as a key put twice; what its puts and reads cost is counted in each disk engine's own tests
- `edges_to_a_retired_record_are_returned` — 026: an engine returns a link to a retired or missing record as it is; hiding that end is the store's
- `edges_follow_a_records_latest_state` — 026: a record put again links where its new state says, and no longer where its earlier state did, in both directions
- `a_commit_backwards_or_with_a_put_past_through_is_invalid` — seams §ProjectionStore: a commit never moves the projection backwards, and never commits a state valid from after the position it says it folded through
- `a_view_does_not_see_an_open_transaction` — seams §ProjectionStore: nothing put in a transaction is visible to any view until it commits, and then all of it at once
- `a_view_keeps_its_position_after_later_commits` — seams §LakeEngine: a view keeps answering its own position while the fold moves on
- `a_put_at_the_same_position_replaces_the_state_there` — seams §ProjectionStore: a put of the same thing at the same position replaces the state there; the fold writing a state again never adds a second one
- `a_put_before_a_later_state_ends_where_that_state_begins` — seams §ProjectionStore: a state put at a position before a later state of the same thing is valid until that later state begins, which stays the latest
- `several_puts_of_one_thing_in_one_transaction_leave_what_they_leave_one_by_one` — seams §ProjectionStore: puts of the same things in one transaction, at later positions, at the same one or before a later one, leave the history and the head's indexes a commit after each put would leave
- `clear_empties_the_projection_and_the_vector_index` — seams §ProjectionStore: clear deletes every state, kept history and the vector index included, and leaves nothing folded, for a rebuild from the log

**reads**

- `mixed_kinds_sort_the_same_on_every_engine` — 029: a scan orders numbers, then instants, then text, then booleans, then JSON, a missing value last in either direction, ties by key, so every engine puts the same records in the same places
- `a_scan_continues_strictly_after_its_keyset_position` — seams §scan: a scan answers only what sorts strictly after `after`, and at most `limit` of it, 0 answering nothing, so an engine that pages natively gives the store's pages
- `a_key_ordered_scan_answers_across_its_batches_as_one_scan` — seams §scan and lakelands#134: a scan ordered by the key alone, read in batches from `after`, answers what one scan over every state answers, whatever its limit, its `after`, its flags and its filter, at the head and at an earlier position, however many rows each batch passes over
- `a_kept_name_answers_as_a_stored_type_holding_the_same_states` — seams §ProjectionStore and lakelands#575: the same states put under a view's kept name, `sys:kept.<view>`, and under an ordinary one answer alike through `get`, `get_many`, a walk by key from `after` across batches, a page under a filter, in a field's order and under `equal`, and a count under each flag and under a filter, at the head and at an earlier position, over a key put thousands of times, keys first put after that position between older ones, and an ended, a retired and a restricted key; an output recorded as often is read at both
- `a_page_by_key_under_an_exact_text_reads_from_after_not_every_match` — seams §scan and lakelands#574: a scan ordered by the key alone that must hold a text exactly, by a term of its filter or by `equal`, walked page by page from `after` and read whole, answers what one scan over every state answers, past records that hold the text and do not qualify (another term, retired, restricted) and a key that ended on both sides of a batch's end, over keys a database's collation orders otherwise than their bytes; a walk is handed at most twice the type's rows and a batch, not every record holding the text for each page, and a count no more rows than hold the text and a batch
- `a_field_ordered_scan_walks_page_by_page_as_one_scan` — seams §scan and lakelands#137: a scan ordered by fields, walked page by page from `after`, answers what one scan over every state answers, whatever its directions, its filter and its flags, after later puts, a redeclaration and a change of the `sort_index.<type>` setting, at the head and at an earlier position
- `a_sort_setting_for_the_settings_type_orders_the_settings_put_after_it` — 041 and seams §ProjectionStore: a `sort_index.<type>` setting takes effect for the records put after it in the same transaction, the settings type's own included: a setting put before it and one put after it are both in the order index
- `a_page_ordered_by_an_indexed_field_reads_its_page_and_the_setting_switches_it` — 041 and lakelands#137: a page of 10 ordered by a field the `sort_index.<type>` setting names reads its page and at most three rows more, not the type, the first page and one continued after a value longer than the index keeps alike; the setting's write switches the same page from sorting the type to the index and back, and the answer stays what one scan over every state answers
- `a_page_at_an_earlier_position_reads_the_head_index_and_the_keys_changed_since` — 041, extended: a page ordered by an indexed field at an earlier position reads the head's order index and the keys of the type changed since, with their states at the position, so it answers what one scan over the states there answers, a key changed, retired or written after the position in its place there, and reads its page, at most three rows more and two per changed key, not the type; a commit between two pages of one view changes neither
- `scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors` — seams §scan: an engine selects exactly the records the one evaluator selects, comparisons across kinds included, and a filter that errors for a record does not select it
- `a_scan_with_equal_answers_exactly_the_identical_values` — 044 and seams §scan: a scan's `equal` selects the records whose field holds exactly the JSON value, never the expression's `=` (`'010'` is not `'10'`, text is not a number, an instant in another written form is not the same), never a null, at the head and at an earlier position alike
- `scan_honours_retired_restricted_and_valid_at` — seams §scan: retired and restricted records come only when asked for, and a valid-timed type's records only when valid at `valid_at`
- `a_count_is_the_number_of_records_its_scan_selects` — seams §ProjectionView and 029: `count` answers the number of records `scan` selects, its flags, `valid_at`, `filter` and `equal` applied and its order, keyset position and limit ignored, at the head and at an earlier position, across more than one batch, a superseded state and a retired record not counted
- `lexical_hits_are_the_pipelines_over_searchable_live_records` — 025 and seams §lexical: a hit holds a token the one pipeline makes of the query in a searchable field of a live record, restricted ones included (the store applies grants); the hit set is the same on every engine, ties by key, at most k
- `lexical_finds_every_token_the_pipeline_makes_beyond_ascii` — 025: every token the one pipeline makes is a term an engine finds, in any script, combining marks included, so no engine's own tokenizer narrows the hit set
- `lexical_at_an_earlier_position_ranks_by_the_head_and_keeps_what_was_visible_then` — seams §lexical: a view answers from the full-text index at the head and keeps only the records live at its own position
- `lexical_over_named_fields_is_the_pipelines_over_those_fields` — 025 and seams §lexical: a search limited to named fields matches, scores and ties over the head text of the named searchable fields only, and no engine cuts its candidates at k before it narrows them: a record matching in a named field is a hit however many others match only in fields not named
- `a_changed_record_is_found_by_what_it_holds_now_and_read_no_more` — seams §ProjectionStore, 025 and 041: after a record changes, the head indexes hold the rows of its latest state and no others: an exact text lookup, a search and a page in an indexed field's order answer by what it holds now, a field it cleared included, and an exact lookup hands the rulebook no record that matched only before, unless the engine keeps no index of exact text and reads the type for every lookup
- `a_transaction_of_many_puts_leaves_what_putting_them_one_by_one_leaves` — seams §ProjectionStore: an engine may write a transaction's puts before its commit, so a type's declaration is put before any record of the type at or after its position; a transaction of more than a page of puts (records below a declaration's position, the declaration, records after it, a record put again, a state put again at its position, a membership put twice, a `sort_index.<type>` setting after its records, a `vector_index.<type>` setting in its first page and one in its last, each with vectors put before and after the page between), over keys a database's collation orders otherwise than their bytes, leaves the head indexes, the earlier states, the memberships and the nearest vectors that putting them one by one leaves; a view opened in it after more than a page of its puts answers what was committed before it; one refused at its commit leaves nothing; and a type that holds an ended key is indexed whole when it is declared again; and the states of two records put record by record, below a declaration's position and at it, after the declaration, leave what position order leaves
- `a_record_put_after_a_redeclaration_in_the_same_transaction_is_indexed_under_it` — 025 and seams §ProjectionStore: a put reads the declarations as the transaction holds them, so a type declared again inside one fold's transaction governs every record put after it, and the records it already held
- `nearest_ranks_by_cosine_with_ties_by_key` — seams §VectorIndex: nearest answers the exact top k by the cosine of the angle, best first, equal scores by key
- `a_zero_vector_scores_zero_and_a_wrong_length_is_invalid` — seams §VectorIndex: a vector of zeros scores 0 against everything, and a query whose length differs from a stored vector's is refused as invalid
- `put_replaces_a_vector_and_remove_forgets_it` — seams §VectorIndex: put replaces a record's vector for the field, remove forgets it, forgetting one that is absent does nothing, and count counts
- `declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position` — seams §ProjectionView: declarations, membership (a record's, and a set's members of a type), recorded outputs and applied ids are kept states too, read at a view's own position
- `a_later_output_for_the_same_call_replaces_the_earlier_from_its_position` — 022, extended, and seams §ProjectionView: the log may hold two outputs for one seam and input (a failure, then the answer a redeclaration asked for); the later one holds from its position, the earlier one before it; an output under the store's own seam name, `sys:renames`, is kept and answered as of a position like any other (lakelands#527)
- `the_first_applied_position_is_kept` — seams §ProjectionStore: marking an id applied twice keeps the first position, so a redelivery is recognised where the event first took effect
- `proposals_filter_by_state_and_type_newest_first` — seams §ProjectionView: proposals match the filter's state and type, a call's type being its verb's `on`, newest first by their latest state, then by id
- `edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position` — 035 and seams §edges: an engine answers a relation without via from its live link records at the view's position, out and in, each edge carrying the link's own fields
- `a_records_edges_at_the_head_read_its_links_not_the_type` — seams §edges and lakelands#190: at the head, the edges into or out of a record over a relation carried by link records, and the edges into it over one carried by a reference field, are found without reading the link type or the declaring type: the records an engine hands the rulebook to find them are about the edges it returns

<!-- End of the generated list. -->
