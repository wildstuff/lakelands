# Import ledger

Lakelands' core was imported from the storage core of the platform it
was extracted from, at one frozen commit,
`3d2cfc7b4dfb5348086cf1342aa1751846af4793`, in the steps of the core
ladder (O0 to O6). This page accounts for every source module of that
core: each is either **imported** — carried into Lakelands, or read and
cut, in the step that took it — or **never imported**, with the reason, or
left for **after `0.1.0`**. It is complete for `0.1.0`: every file under
the roots below, as it stood at the freeze, has a row. In `0.2.0` the
PostgreSQL engine took the `{pg}` root, which was left for after
`0.1.0`; nothing is left there now.

Lines are counted in the source at the freeze, one per text line. Lines
out are counted per step, not per module: an organ was transformed on
arrival, so one Lakelands file often holds what several source modules
held, and one source module often landed in several. Where a step's pull
request counted differently from the tree, the tree wins, and **Where the
tree corrects the steps** says where.

## The source roots

The source is named here by role, never by its own paths:

| Root | What it is |
|---|---|
| `{types}` | the contract-and-types module of the platform's providers core crate: types, shape, rights, describe, the evaluator, the conformance suite |
| `{views}` | its view-syntax crate: the view grammar |
| `{formats}` | its value-format crate |
| `{templates}` | its template loader: the apply lane the bundle format came from |
| `{store}` | its data store crate |
| `{tools}` | the paths of its tool layer the catalog was cut from: the read tools named below, `crud` and the write preflight (the tool layer's other modules are the platform's own and not part of the storage core) |
| `{mem}` | its in-memory data engine |
| `{engine}` | its SQLite data engine |
| `{pg}` | its PostgreSQL data engine |

## Totals

| | Files | Lines |
|---|---|---|
| imported, carried | 86 | 43,151 |
| imported, read and cut | 81 | 35,105 |
| never imported | 93 | 56,743 |
| after 0.1.0 | 0 | 0 |
| crate scaffolding | 10 | 644 |
| **all** | **270** | **135,643** |

## By step

| Step | Lines in, carried | Lines in, read and cut | Lines out (Lakelands, as the step's pull request measured them) |
|---|---|---|---|
| O0 — the workspace and the nine gates | — | — | 0 imported; scaffolding, scripts and docs |
| O1 — the contract | — | — | `lakelands-api` written new, 5,051 lines of `src/` (no source: the contract is Lakelands' own) |
| O2 — contract and types | 16,038 | 11,566 | 15,261 in `lakelands-core` |
| O3 — the store | 25,521 | 4,729 | 10,419: `lakelands-store` 5,259 + 985 tests; `lakelands-engine-mem` 2,526 + 86 tests; `lakelands-core` +1,563 |
| O4 — the SQLite engine | 794 | 11,566 | +4,428 net: `lakelands-engine-sqlite` 2,076 + 313 tests; `lakelands-core` +1,887 (the shared rulebook, checks); the rest across api, store and mem |
| O5 — the doors meet the core | — | — | nothing imported; +1,381 / −435 across the doors, the mock and the store |
| O6 — `0.1.0` | — | — | nothing imported; this ledger, the port log and the version |
| pg — the PostgreSQL engine (lakelands#65, `0.2.0`) | 798 | 7,244 | `lakelands-engine-pg` 2,396 `src/` + 538 tests; `lakelands-api` +83 (`EngineConfig::Postgres`) |

At the tag the core crates hold **30,999** lines (`lakelands-core`
23,112, `lakelands-store` 5,780, `lakelands-engine-sqlite` 2,107, each
`src/` with its inline tests), against the **70,000** the `core-size`
gate allows. The roots above held 135,643 lines at the freeze.

## Where the tree corrects the steps

- **O2, cut:** the row counted 10,950. The tree adds `{types}/guards.rs`
  (616), read authorization by caller, role and group, which grants
  replaced and the row did not name: **11,566**.
- **O3, the store row:** the row counted 13,422 carried, with the store's
  `backend.rs` (1,287) in it as "facade only". Nothing of it was carried:
  it is the `DataBackend` blanket implementation, which the plan's
  never-imported table names. The store's carried lines are **12,135**,
  and `backend.rs` is never imported.
- **O3, cut:** the row counted 1,875 + 1,943 + 617. The tree adds
  `{types}/write/handle.rs` (294), a per-lake handle trait the engine
  traits superseded: **4,729**.
- **O4:** the row counted 12,561 for the engine; the tree agrees. Of it,
  794 came across (the tuning as code, the schema's and the edges' ideas
  as design), 11,566 were read and cut, and `projection/concept.rs` (201)
  is never imported.

## Never imported, outside the roots

The plan's never-imported table also names what lies outside the roots
above; none of it was imported:

| What | Why not |
|---|---|
| The `DataBackend` trait, its blanket implementation and its stub | the store facade is the API (both `backend.rs` files are in the tables below) |
| The data command enum, the CLI's command tree, and the carrying of derived fields in the CLI | the CLI calls the library; the requirement behind them survives as fold rules (sparse patches, per-field grants, `depends_on` with `clear`) |
| The tool layer's three name registries, their generic record twins and their bijection tests | one catalog: every tool name comes from `lakelands_api::tools` |
| The self-model query operations of the tool layer | per-type reads over `sys.` types are the platform's |
| The platform's flow lint and adaptation modules | product features |
| The platform's advisors (intro, scaffold, concept, resolve, curation) | may return as an advisor crate of their own, never into the core |
| The platform's JSON snapshots under a profile path | replaced by positions and kept history |
| Intake-side transforms and generated samples | the platform's |
| The blocking bridge and its call sites | async only |
| The platform's common vocabulary modules (flows, self-modelling, company settings, settings, intake, the reserved-type roster) | the `sys.` namespace and a host's registered vocabulary |
| The platform core's profile layout, subjects, plugin settings, declared-by, LLM scope and sample recipe | `Location` replaces the profile layout; the rest is the platform's vocabulary |
| The caller shapes (caller, actor, capabilities, field grants and their neighbours) | `Principal` and `WriteContext` |
| The locale enum | BCP-47 tag strings |
| The environment reads | engine and `Location` configuration |
| The platform's WIT package for data | authored fresh when the WIT package is written, after `0.1.0` |
| About 1,300 engine-local test functions | only tests of engine mechanics the suite cannot see were imported; every behaviour claim moved into the conformance suite |

## Every module

### `{types}` — 83 files, 38,851 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{types}/adaptation.rs` | 529 | never imported | — | a product feature |
| `{types}/adaptation/tests.rs` | 590 | never imported | — | a product feature |
| `{types}/backend.rs` | 1,472 | never imported | — | the `DataBackend` trait: the store facade is the API |
| `{types}/capabilities.rs` | 825 | imported, read and cut | O2 | no capability negotiation: every engine implements the whole narrow seam, and the store walks the graph (026) |
| `{types}/capability_record.rs` | 278 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/compat.rs` | 1,041 | imported, carried | O2 | shape classification and conversion |
| `{types}/conformance.rs` | 1,205 | imported, carried | O2 | the conformance suite |
| `{types}/conformance/computed_fields.rs` | 211 | imported, carried | O2 | the conformance suite |
| `{types}/conformance/derived_views.rs` | 596 | imported, carried | O2 | the conformance suite |
| `{types}/conformance/fold_rules.rs` | 145 | imported, carried | O2 | the conformance suite |
| `{types}/conformance/money_scale.rs` | 94 | imported, carried | O2 | the conformance suite |
| `{types}/conformance/undeclared_fields.rs` | 160 | imported, carried | O2 | the conformance suite |
| `{types}/curation.rs` | 552 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/customer_plane.rs` | 540 | imported, read and cut | O2 | product vocabulary |
| `{types}/describe.rs` | 1,441 | imported, carried | O2 | describe |
| `{types}/describe_actions.rs` | 335 | imported, carried | O2 | describe |
| `{types}/describe_axes.rs` | 254 | imported, carried | O2 | describe |
| `{types}/describe_field.rs` | 71 | imported, carried | O2 | describe |
| `{types}/describe_flows.rs` | 37 | imported, carried | O2 | describe |
| `{types}/describe_handlers.rs` | 32 | imported, carried | O2 | describe |
| `{types}/describe_provenance.rs` | 102 | imported, carried | O2 | describe |
| `{types}/event.rs` | 580 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/field_rights.rs` | 314 | imported, carried | O2 | field rights |
| `{types}/fold.rs` | 718 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/guards.rs` | 616 | imported, read and cut | O2 | read authorization by caller, role and group; replaced by grants (`lakelands_core::rights`, 006). Not named in the O2 row: the tree adds it |
| `{types}/interface.rs` | 263 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/intro.rs` | 1,328 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/intro/capabilities.rs` | 195 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/intro/function_eval.rs` | 69 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/intro/functions.rs` | 102 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/intro/grounding.rs` | 347 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/intro/tests.rs` | 555 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/json_schema.rs` | 358 | imported, read and cut | O2 | superseded by `lakelands_api::tools::field_schema` |
| `{types}/lineage_backbone.rs` | 166 | imported, read and cut | O2 | provenance lives in the envelope and per-field meta (010) |
| `{types}/mod.rs` | 251 | crate scaffolding | — | module declarations; each Lakelands crate declares its own |
| `{types}/object_set.rs` | 1,019 | imported, carried | O2 | object-set predicate |
| `{types}/package_provenance.rs` | 135 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/portable.rs` | 1,115 | imported, carried | O2 | the portable expression form |
| `{types}/projection_store.rs` | 779 | imported, read and cut | O2 | superseded by the contract's engine traits |
| `{types}/projection_store/store.rs` | 471 | imported, read and cut | O2 | superseded by the contract's engine traits |
| `{types}/projection_store/view.rs` | 560 | imported, read and cut | O2 | superseded by the contract's engine traits |
| `{types}/pushdown.rs` | 719 | imported, read and cut | O2 | no capability negotiation: every engine implements the whole narrow seam, and the store walks the graph (026) |
| `{types}/resolve.rs` | 450 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/row_provenance.rs` | 150 | imported, read and cut | O2 | provenance lives in the envelope and per-field meta (010) |
| `{types}/sample_eligibility.rs` | 114 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/scaffold.rs` | 289 | never imported | — | an advisor; may return later as its own crate, never into the core |
| `{types}/schema_hash.rs` | 819 | imported, carried | O2 | schema hash, over canonical bytes |
| `{types}/setting_scope.rs` | 543 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/shared_property.rs` | 826 | imported, carried | O2 | shared properties |
| `{types}/snapshot.rs` | 490 | never imported | — | JSON files under a profile path; replaced by positions and kept history |
| `{types}/sqlite_tuning.rs` | 213 | imported, carried | O4 | as `SqliteTuning` in the location |
| `{types}/stem.rs` | 202 | imported, carried | O2 | the token pipeline |
| `{types}/surface.rs` | 158 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/type_doc.rs` | 541 | imported, carried | O2 | type documentation |
| `{types}/types/askable.rs` | 117 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/field.rs` | 1,050 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/function_def.rs` | 174 | imported, carried | O2 | function declarations |
| `{types}/types/mod.rs` | 57 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/record.rs` | 803 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/relation.rs` | 324 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/schema_delta.rs` | 145 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/type_def.rs` | 1,492 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/view.rs` | 289 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/write_authority.rs` | 158 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/types/write_key.rs` | 113 | imported, read and cut | O2 | superseded by the contract's types; the write-authority admission rule survives as `rights::Door::admits` |
| `{types}/valid_time.rs` | 221 | imported, carried | O2 | valid time |
| `{types}/verb_store.rs` | 101 | imported, read and cut | O2 | filesystem reads; verbs are declarations |
| `{types}/write.rs` | 103 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/write/coerce.rs` | 450 | imported, carried | O2 | value coercion |
| `{types}/write/derived.rs` | 1,384 | imported, carried | O2 | derived fields |
| `{types}/write/derived/tests.rs` | 1,310 | never imported | — | engine-local tests: their behaviour claims are in the conformance suite |
| `{types}/write/effect_completion.rs` | 218 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/write/enrichment_specs.rs` | 108 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/write/eval.rs` | 843 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/write/flow_sovereignty.rs` | 125 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/write/guard_eval.rs` | 798 | imported, carried | O2 | guard evaluation |
| `{types}/write/guards.rs` | 373 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/write/handle.rs` | 294 | imported, read and cut | O3 | a per-lake handle trait, superseded by the engine traits `LakeEngine` / `LakeStore` |
| `{types}/write/inbound_guard.rs` | 288 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/write/ownership.rs` | 122 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |
| `{types}/write/validate.rs` | 909 | imported, carried | O3 | store semantics, moved to the store row |
| `{types}/write/view.rs` | 383 | imported, carried | O2 | view evaluation |
| `{types}/write/write_plane.rs` | 134 | imported, read and cut | O3 | platform planes; neither the field-authority rule nor a cause link lives here |

### `{views}` — 2 files, 820 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{views}/Cargo.toml` | 15 | crate scaffolding | — | the crate manifest |
| `{views}/src/lib.rs` | 805 | imported, carried | O2 | the view grammar |

### `{formats}` — 2 files, 320 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{formats}/Cargo.toml` | 40 | crate scaffolding | — | the crate manifest |
| `{formats}/src/lib.rs` | 280 | imported, carried | O2 | value formats |

### `{templates}` — 8 files, 2,740 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{templates}/Cargo.toml` | 24 | crate scaffolding | — | the crate manifest |
| `{templates}/src/apply.rs` | 721 | imported, carried | O2 | the bundle loader (the apply lane) |
| `{templates}/src/intake_config.rs` | 173 | imported, read and cut | O2 | seeds, CSV records and intake: not the bundle's business |
| `{templates}/src/lib.rs` | 49 | imported, carried | O2 | the bundle loader (the apply lane) |
| `{templates}/src/records.rs` | 227 | imported, read and cut | O2 | seeds, CSV records and intake: not the bundle's business |
| `{templates}/src/seeds.rs` | 392 | imported, read and cut | O2 | seeds, CSV records and intake: not the bundle's business |
| `{templates}/src/tests.rs` | 941 | imported, read and cut | O2 | seeds, CSV records and intake: not the bundle's business |
| `{templates}/src/types.rs` | 213 | imported, carried | O2 | the bundle loader (the apply lane) |

### `{store}` — 45 files, 19,188 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{store}/Cargo.toml` | 48 | crate scaffolding | — | the crate manifest |
| `{store}/src/aging.rs` | 302 | imported, read and cut | O3 | aging is after `0.1.0`, with a `LakeStore::age` call and a specified `sys.aging_rule` |
| `{store}/src/append.rs` | 13 | imported, carried | O3 | the store |
| `{store}/src/backend.rs` | 1,287 | never imported | — | the `DataBackend` blanket implementation: the store facade is the API |
| `{store}/src/blocking_bridge.rs` | 30 | never imported | — | async only |
| `{store}/src/compact.rs` | 369 | imported, read and cut | O3 | compaction is after `0.1.0`; sealing and checkpoints come from the format |
| `{store}/src/concept.rs` | 470 | never imported | — | an advisor |
| `{store}/src/ctor.rs` | 46 | imported, carried | O3 | the store |
| `{store}/src/embed.rs` | 668 | imported, read and cut | O3 | declared embeddings are after `0.1.0` |
| `{store}/src/engine.rs` | 244 | imported, carried | O3 | the store |
| `{store}/src/flow_lint.rs` | 1,141 | never imported | — | a product feature |
| `{store}/src/graph.rs` | 826 | imported, carried | O3 | the store |
| `{store}/src/grouped.rs` | 1,005 | imported, carried | O3 | the store |
| `{store}/src/handle.rs` | 2 | imported, carried | O3 | the store |
| `{store}/src/hybrid.rs` | 172 | imported, carried | O3 | the store |
| `{store}/src/ingest_batch.rs` | 717 | imported, carried | O3 | the store |
| `{store}/src/ingest_mode.rs` | 56 | imported, carried | O3 | the store |
| `{store}/src/lib.rs` | 269 | imported, carried | O3 | the store |
| `{store}/src/lifecycle.rs` | 358 | imported, carried | O3 | the store |
| `{store}/src/migration.rs` | 1,095 | imported, carried | O3 | the store |
| `{store}/src/normalize.rs` | 370 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/object_sets.rs` | 1,381 | imported, carried | O3 | the store |
| `{store}/src/ontology.rs` | 1,101 | imported, carried | O3 | the store |
| `{store}/src/outbox.rs` | 100 | imported, carried | O3 | the store |
| `{store}/src/projection/aggregate.rs` | 326 | imported, carried | O3 | the store |
| `{store}/src/projection/decode.rs` | 20 | imported, carried | O3 | the store |
| `{store}/src/projection/mod.rs` | 4 | crate scaffolding | — | module declarations |
| `{store}/src/projection_seam.rs` | 3 | imported, carried | O3 | the store |
| `{store}/src/query.rs` | 1,326 | imported, carried | O3 | the store |
| `{store}/src/query/query_indexed.rs` | 424 | imported, carried | O3 | the store |
| `{store}/src/record_health.rs` | 31 | imported, carried | O3 | the store |
| `{store}/src/records.rs` | 318 | imported, carried | O3 | the store |
| `{store}/src/records_as_of.rs` | 197 | imported, carried | O3 | the store |
| `{store}/src/restriction.rs` | 105 | imported, carried | O3 | the store |
| `{store}/src/sample.rs` | 337 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/sample/scenario.rs` | 177 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/sample/scenario/tests.rs` | 158 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/sample/tests.rs` | 249 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/sample_source.rs` | 657 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/schema.rs` | 1,190 | imported, carried | O3 | the store |
| `{store}/src/schema_projection.rs` | 235 | imported, carried | O3 | the store |
| `{store}/src/silver.rs` | 250 | never imported | — | intake-side transforms and generated samples |
| `{store}/src/snapshot_checkpoint.rs` | 42 | imported, read and cut | O3 | superseded by the projection's kept history |
| `{store}/src/source_contribution.rs` | 494 | imported, read and cut | O3 | purge is after `0.1.0` |
| `{store}/src/write_fold.rs` | 575 | imported, carried | O3 | the store |

### `{tools}` — 10 files, 7,577 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{tools}/crud.rs` | 549 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/catalog.rs` | 1,216 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/dispatch.rs` | 1,168 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/export.rs` | 617 | imported, read and cut | O3 | no export door in the contract |
| `{tools}/read/helpers.rs` | 715 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/query.rs` | 1,132 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/search.rs` | 938 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/traverse.rs` | 723 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/read/types.rs` | 152 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |
| `{tools}/write_preflight.rs` | 367 | imported, carried | O3 | the tool layer, cut down to the catalog and the one name generator |

### `{mem}` — 7 files, 3,036 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{mem}/Cargo.toml` | 56 | crate scaffolding | — | the crate manifest |
| `{mem}/src/handle.rs` | 560 | imported, carried | O3 | the in-memory engine |
| `{mem}/src/lib.rs` | 316 | imported, carried | O3 | the in-memory engine |
| `{mem}/src/state.rs` | 500 | imported, carried | O3 | the in-memory engine |
| `{mem}/src/view.rs` | 981 | imported, carried | O3 | the in-memory engine |
| `{mem}/tests/conformance.rs` | 83 | never imported | — | engine-local tests: behaviour claims are in the suite and the engine-seam harness |
| `{mem}/tests/seam.rs` | 540 | never imported | — | engine-local tests: behaviour claims are in the suite and the engine-seam harness |

### `{engine}` — 76 files, 50,340 lines

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{engine}/Cargo.toml` | 102 | crate scaffolding | — | the crate manifest |
| `{engine}/benches/as_of_restriction.rs` | 603 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/benches/hotpaths.rs` | 329 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/benches/snapshots.rs` | 133 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/graph.rs` | 755 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/grouped.rs` | 139 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/mod.rs` | 155 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/object_set_sql.rs` | 553 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/portable_parity.rs` | 575 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/projection/aggregate.rs` | 631 | imported, read and cut | O4 | aggregation is the store's |
| `{engine}/src/data_engine/projection/apply.rs` | 1,107 | imported, read and cut | O4 | the fold: the engine never folds, the store does |
| `{engine}/src/data_engine/projection/concept.rs` | 201 | never imported | — | an advisor |
| `{engine}/src/data_engine/projection/edges.rs` | 118 | imported, carried | O4 | the edges table, derived from `via` at the head (as design; rewritten for kept history) |
| `{engine}/src/data_engine/projection/fts.rs` | 522 | imported, read and cut | O4 | per-type FTS5 tables with SQL scoring; replaced by one table of pipeline tokens that finds candidates |
| `{engine}/src/data_engine/projection/keyset.rs` | 109 | imported, read and cut | O4 | SQL paging; the shared rulebook orders and pages |
| `{engine}/src/data_engine/projection/mod.rs` | 79 | imported, read and cut | O4 | module declarations |
| `{engine}/src/data_engine/projection/order_tail.rs` | 136 | imported, read and cut | O4 | SQL ordering; the shared rulebook |
| `{engine}/src/data_engine/projection/page_bound.rs` | 262 | imported, read and cut | O4 | SQL paging; the shared rulebook |
| `{engine}/src/data_engine/projection/queries.rs` | 1,498 | imported, read and cut | O4 | SQL reads over one row per record without history; replaced by the history reads and the shared rulebook |
| `{engine}/src/data_engine/projection/schema.rs` | 463 | imported, carried | O4 | the schema's ideas: value index, edges, the search row map, per-field vectors (as design; rewritten for kept history) |
| `{engine}/src/data_engine/projection/tests.rs` | 86 | imported, read and cut | O4 | engine-local tests |
| `{engine}/src/data_engine/projection/vectors.rs` | 557 | imported, read and cut | O4 | `vec0`: no C extension in `0.1.0` |
| `{engine}/src/data_engine/projection_conformance.rs` | 417 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/sqlite_projection.rs` | 1,445 | imported, read and cut | O4 | the old view and store wrapper; replaced by the handle and the view |
| `{engine}/src/data_engine/store/append.rs` | 175 | imported, read and cut | O4 | the old facade; its semantics are the store's |
| `{engine}/src/data_engine/store/compact.rs` | 913 | imported, read and cut | O4 | compaction is after `0.1.0` |
| `{engine}/src/data_engine/store/engine.rs` | 234 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/handle.rs` | 169 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/lifecycle.rs` | 1,179 | imported, read and cut | O4 | log rotation into SQLite and open-time repair: the log is segment files; its connection pragmas are kept, as tuning |
| `{engine}/src/data_engine/store/mod.rs` | 77 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/ontology.rs` | 42 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/outbox.rs` | 482 | imported, read and cut | O4 | group commit is the store's ingest window |
| `{engine}/src/data_engine/store/projection_seam.rs` | 389 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/query.rs` | 70 | imported, read and cut | O4 | the old facade |
| `{engine}/src/data_engine/store/records_tests.rs` | 1,185 | imported, read and cut | O4 | engine-local tests: behaviour claims are in the suite |
| `{engine}/src/data_engine/store/snapshot_checkpoint.rs` | 219 | imported, read and cut | O4 | superseded by kept history |
| `{engine}/src/data_engine/tests.rs` | 62 | never imported | — | in neither of the plan's tables; not imported, as agreed: the store owns this job (walks, groups, object sets, parity, conformance) |
| `{engine}/src/data_engine/tests/aging.rs` | 630 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/auth_rbac.rs` | 1,534 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/basics.rs` | 80 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/bench.rs` | 1,567 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/bench_write_stages.rs` | 198 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/bitemporal.rs` | 1,691 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/bounded_page.rs` | 882 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/concurrent_writers.rs` | 598 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/derived.rs` | 1,556 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/edge_cases.rs` | 1,892 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/enrichment.rs` | 1,329 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/event_envelope.rs` | 103 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/filter_scale.rs` | 1,893 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/group_commit.rs` | 490 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/handle_release.rs` | 161 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/helpers.rs` | 1,712 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/llm_prompt.rs` | 163 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/mirror.rs` | 894 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/ontology.rs` | 2,704 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/out_of_band_delete.rs` | 213 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/procedure_backed.rs` | 174 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/projection.rs` | 2,376 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/reader_pool.rs` | 172 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/relations.rs` | 1,822 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/restriction.rs` | 323 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/sample_handover.rs` | 199 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/sample_recipe.rs` | 535 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/sample_scenario.rs` | 305 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/sample_source.rs` | 312 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/sample_source_fixtures.rs` | 74 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/search_query.rs` | 3,172 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/source_contribution.rs` | 635 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/stichtag_scale.rs` | 330 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/upsert.rs` | 744 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/data_engine/tests/vector_search.rs` | 2,071 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/src/lib.rs` | 30 | crate scaffolding | — | the crate root |
| `{engine}/tests/channel_identity_link.rs` | 200 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/tests/channel_identity_link_grounding.rs` | 250 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |
| `{engine}/tests/conformance.rs` | 155 | never imported | — | engine-local tests and benches: behaviour claims are in the suite |

### `{pg}` — 37 files, 12,771 lines

Taken in `0.2.0` (lakelands#65). As with `{engine}` in O4, the engine's
design is Lakelands' own (`docs/decisions/038-a-lake-in-postgresql.md`):
the connection discipline came across as code, and the rest was read and
cut.

| Module | Lines | Status | Step | Why |
|---|---|---|---|---|
| `{pg}/Cargo.toml` | 74 | crate scaffolding | — | the crate's manifest |
| `{pg}/src/append.rs` | 208 | imported, read and cut | pg | appends by `MAX(position)+1` are replaced by the log as rows checked against the lease row in one transaction (`log`) |
| `{pg}/src/batch_append.rs` | 425 | imported, read and cut | pg | group commit is the store's ingest window |
| `{pg}/src/batch_reads.rs` | 184 | imported, read and cut | pg | SQL reads over one row per record; reads are the kept history and the rulebook (`view`) |
| `{pg}/src/bucketed.rs` | 131 | imported, read and cut | pg | aggregation is the store's |
| `{pg}/src/composites.rs` | 241 | imported, read and cut | pg | grouped views are the store's |
| `{pg}/src/errors.rs` | 63 | imported, carried | pg | the error chain came across: every cause the driver gives, in one sentence (`connect`) |
| `{pg}/src/fold.rs` | 899 | imported, read and cut | pg | the engine never folds; the store does |
| `{pg}/src/handle.rs` | 1,202 | imported, read and cut | pg | replaced by `handle`: the seam's three traits over a reader, a lease and a projection connection |
| `{pg}/src/lib.rs` | 926 | imported, read and cut | pg | the engine and its per-tenant slots are replaced by one handle per lake (`handle`); read and cut, its driver guard reimplemented (`connect::driver`) |
| `{pg}/src/migration_guard.rs` | 72 | imported, read and cut | pg | migrations are the store's; the engine keeps states |
| `{pg}/src/paging.rs` | 420 | imported, read and cut | pg | paging and order are the rulebook's |
| `{pg}/src/schema.rs` | 513 | imported, read and cut | pg | replaced by `projection`: one schema per lake, every statement qualified, the kept history |
| `{pg}/src/search.rs` | 302 | imported, read and cut | pg | `tsvector` ranking decides matches and orders the other engines decide differently; full-text candidates are token rows, the rulebook scores |
| `{pg}/src/session_isolation.rs` | 179 | imported, carried | pg | the session probe came across: a setting must survive, a fresh connection must not inherit it; a transaction-mode pooler is refused (`connect::session_keeping_state`) |
| `{pg}/src/tls.rs` | 478 | imported, carried | pg | the rustls connector came across: roots from a named CA file, a client certificate with its key, both or neither; the URL keys and the machine store were cut for the location's `tls` and well-known bundle paths (`connect::tls_config`) |
| `{pg}/src/type_cache.rs` | 88 | imported, read and cut | pg | nothing is cached that a write invalidates; a view reads its declarations |
| `{pg}/src/vectors.rs` | 192 | imported, read and cut | pg | `pgvector` is not used; vectors are compared by the one cosine, by brute force |
| `{pg}/src/version.rs` | 78 | imported, carried | pg | the server floor came across as a check at open, at PostgreSQL 16, the release the conformance job runs (`connect`) |
| `{pg}/src/view.rs` | 1,441 | imported, read and cut | pg | replaced by `view`: history reads in one repeatable-read transaction, the rulebook deciding |
| `{pg}/tests/batch_cursor.rs` | 256 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/batch_write.rs` | 222 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/bucketed_parity.rs` | 248 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/conformance.rs` | 192 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/connection_discipline.rs` | 473 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/engine.rs` | 350 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/fold_memo.rs` | 188 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/grouped_parity.rs` | 293 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/migration_refusal.rs` | 173 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/mutual_tls.rs` | 101 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/null_axis_parity.rs` | 254 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/order_page_parity.rs` | 254 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/pooler_refusal.rs` | 116 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/rederive_clears.rs` | 252 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/restriction_parity.rs` | 235 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/search_parity.rs` | 673 | never imported | — | engine-local tests: behaviour claims are in the suite |
| `{pg}/tests/stichtag_parity.rs` | 375 | never imported | — | engine-local tests: behaviour claims are in the suite |

## At the tag

| Gate | At `0.1.0` |
|---|---|
| `core-size` | 30,999 of 70,000 (the base is unchanged; it ratchets down from here) |
| `seam-width` | 45 of 110 methods: `LakeStore` 12, `LakeEngine` 12, `ProjectionStore` 11, `ProjectionView` 10 |
| `cfg-budget` | 0 of 20 `cfg(feature)` sites |
| `conformance-required` | green on `sqlite` and `mem`; `pg` deferred |
| Conformance suite | 96 checks through the doors, 37 through the engine seam, all green on both engines |
| `no-external-decisions`, `vocabulary`, `single-catalog`, `no-external-platform-deps`, `dco` | green |
