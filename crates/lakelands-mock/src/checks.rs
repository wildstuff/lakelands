//! The door-facing behaviours of the contract, as checks any lake can be
//! put through.
//!
//! Each check is one async function over a fresh, empty
//! `Arc<dyn LakeStore>`: it declares the fixture ontology through the
//! catalog, acts as the fixture's principals and asserts what a door can
//! observe. The mock runs them in its own tests; a door crate runs them
//! again over whatever lake it is built on, and the real store runs them
//! unchanged, which is how the mock is shown to be honest.
//!
//! [`door_checks!`](crate::door_checks) expands to one `#[test]` per
//! check, given a function that makes a fresh lake and one that blocks
//! on a future. No check assumes an absolute position: a lake may log
//! more than the mock before its first write.
//!
//! A refusal's sentence is not part of the contract; its kind and field
//! are, and the names it must mention. So a check asserts a refusal's
//! kind, its field and the names in it, never the words around them.

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::describe::{ExecutorDescription, Kept, NotKept};
use lakelands_api::error::{Error, Missing, Refusal};
use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, RecordKey, SourceId, TypeName, VerbName, Version,
};
use lakelands_api::location::Limits;
use lakelands_api::ontology::{
    Assignment, Authority, FieldDecl, FieldType, Measure, OnInputChange, RelationDecl, Risk,
    StorageKind, TriggerDecl, TypeDecl, VerbDecl, VerbEffect, VerbScope, ViewDecl, ViewParam,
};
use lakelands_api::principal::{Access, Grants, Principal, PrincipalKind, TypeGrants};
use lakelands_api::query::{Pattern, Search, SearchMode, Step, VectorTerm, Walk};
use lakelands_api::read::{After, Direction, Find, Found, Get, Order, OrderBy, ReadAt};
use lakelands_api::record::{Origin, Patch, Record, SourceRecord, Value};
use lakelands_api::seam::{FEED_EXAMINED, FeedFilter};
use lakelands_api::tools::{Door, tools};
use lakelands_api::write::{
    Declaration, FunctionDecl, Migration, MigrationReport, ObjectSetDecl, Outcome, ProposalFilter,
    ProposalState, ProposedWrite, Recompute, WriteOpts,
};
use lakelands_api::{Catalog, LakeStore, WriteContext};
use serde_json::json;

use crate::stream;

/// Expands to one `#[test]` per door check.
///
/// `$new` is called once per check and gives a fresh, empty lake as an
/// `Arc<dyn LakeStore>`; `$block_on` runs a future to completion.
///
/// ```ignore
/// lakelands_mock::door_checks!(
///     || Arc::new(MockLake::new("finance")) as Arc<dyn LakeStore>,
///     my_runtime::block_on
/// );
/// ```
///
/// A check marked `#[host_ops]` calls a host operation of the
/// [`LakeStore`] (`age`, `purge_source`), which a host holds next to its
/// store and no network door offers. Given `remote` after `$block_on`,
/// the macro expands for a lake reached over a network door: each marked
/// check is ignored, saying why on its own line, and every other check
/// runs unchanged. A check that calls a host operation without the mark
/// fails there as `Unbound`, never silently.
///
/// A check marked `#[own_principals]` makes principals beside the cast,
/// which the resolver behind a network door does not know: it is ignored
/// there too, saying why, and runs on every lake reached in process.
///
/// A check marked `#[in_process_feed]` reads where one feed poll stops,
/// which holds for a lake's own feed. Over a network door the server's
/// stream reads on to a match or to the head, a bounded poll at a time
/// (`docs/seams.md` §`ChangeFeed`), so it is ignored there, saying why.
///
/// A check marked `#[views]` reads a view's computed records, which the
/// mock does not compute. Given `mock` after `$block_on`, as the mock's
/// own tests give it, each marked check is ignored, saying why; every
/// lake that computes views runs it.
#[macro_export]
macro_rules! door_checks {
    ($new:expr, $block_on:path) => {
        $crate::door_checks!(@all local, $new, $block_on);
    };
    ($new:expr, $block_on:path, remote) => {
        $crate::door_checks!(@all remote, $new, $block_on);
    };
    ($new:expr, $block_on:path, mock) => {
        $crate::door_checks!(@all mock, $new, $block_on);
    };
    (@all $at:ident, $new:expr, $block_on:path) => {
        $crate::door_checks!(@each $at, $new, $block_on;
            every_write_advances_the_position,
            an_absent_field_is_unchanged_and_null_clears_it,
            one_forbidden_field_refuses_the_whole_write,
            a_field_of_another_authority_is_the_wrong_door,
            a_stale_expected_version_is_a_conflict,
            a_retried_idempotency_key_answers_the_first_outcome,
            a_read_as_of_an_earlier_position_sees_the_earlier_state,
            a_position_past_the_head_is_out_of_range,
            describe_shows_only_what_the_principal_may_see,
            describe_answers_the_declaration_whole,
            a_masked_field_is_named_but_its_value_is_absent,
            a_field_built_from_a_hidden_one_is_hidden_too,
            a_grant_holds_a_field_through_its_rename,
            a_valid_time_read_probes_the_valid_time_fields,
            a_write_only_field_is_written_and_never_shown,
            the_tools_follow_the_grants,
            a_verb_tool_says_what_its_risk_does,
            a_step_to_an_unbound_view_is_refused,
            a_view_reads_only_what_its_source_has,
            a_view_is_declared_only_as_the_store_declares_it,
            names_and_expressions_are_declared_only_as_the_store_declares_them,
            a_migration_keeps_what_a_view_reads,
            a_declaration_reads_only_what_its_type_has,
            a_function_over_null_is_null_on_every_lake,
            a_duration_by_a_unit_is_a_number_and_what_never_lands_is_refused,
            recompute_appends_the_function_in_force_again,
            a_set_names_a_set_and_tells_no_more_than_its_filter,
            a_renamed_relation_keeps_its_edges,
            #[own_principals]
            a_proposal_that_waited_hides_by_the_names_in_force,
            a_migration_keeps_what_a_declaration_reads,
            a_view_is_never_migrated,
            nothing_that_never_acts_is_declared_on_a_view,
            a_view_over_a_hidden_field_names_nothing,
            a_set_may_hold_a_link_types_records,
            #[own_principals]
            a_type_made_of_another_is_shown_only_with_it,
            a_relation_on_a_view_is_derived,
            a_tools_key_says_what_to_pass,
            propose_shows_each_writes_shape,
            a_verb_on_its_type_has_a_tool_without_a_key,
            a_verb_on_its_type_is_called_without_a_key,
            a_low_risk_verb_runs_at_once,
            a_riskier_verb_is_proposed_and_accept_applies_it,
            an_accept_after_the_verb_was_declared_differently_is_refused,
            a_call_that_could_not_run_now_is_neither_run_nor_proposed,
            a_rejected_proposal_writes_nothing_and_stays_decided,
            only_a_reviewer_decides_proposals,
            sys_types_cannot_be_declared,
            add_field_without_the_schema_grant_tells_nothing_of_the_type,
            a_type_its_proposer_is_not_shown_is_a_type_that_is_not_declared,
            a_type_its_writer_is_not_shown_is_a_type_that_is_not_declared,
            #[own_principals]
            a_sensitive_field_shows_a_field_and_never_a_type,
            #[own_principals]
            a_type_its_principal_is_shown_is_one_it_may_find_and_get,
            #[own_principals]
            a_retry_is_answered_before_its_writer_is_asked_what_it_is_shown,
            a_record_event_says_whether_it_made_the_record,
            a_required_field_is_judged_over_the_record_whole,
            #[host_ops]
            a_field_that_declares_initial_makes_no_owner,
            an_embedding_is_unbound_where_no_backend_is_bound,
            a_link_to_no_live_record_or_over_no_link_relation_is_refused,
            every_door_refuses_a_view_alike,
            a_retired_record_leaves_reads_but_not_its_past,
            a_narrow_writer_cannot_retire,
            a_proposer_sees_only_its_own_proposals,
            ingest_is_content_addressed,
            a_cursor_keeps_its_page_set_while_writes_land,
            a_page_answers_its_total_and_jumps_by_offset,
            a_cursor_continues_only_its_own_answer,
            a_cursor_continues_only_the_read_it_began,
            after_walks_the_pages_the_cursor_walks,
            after_is_refused_naming_the_member,
            a_limit_outside_the_lakes_bounds_is_refused_at_every_door,
            a_principal_of_another_lake_is_refused,
            the_feed_is_the_log_by_position,
            the_feed_filtered_by_key_answers_those_records_events,
            #[in_process_feed]
            a_feed_poll_examines_at_most_the_bound,
            a_step_over_an_undeclared_relation_is_refused,
            a_step_naming_an_inverse_walks_the_relation_the_other_way,
            a_dependent_field_declared_clear_is_cleared_with_its_input,
            two_writers_keep_each_others_fields,
            per_field_meta_names_the_writer_and_the_position,
            apply_declares_a_set_in_one_append_with_forward_references,
            apply_refuses_the_whole_set_naming_every_problem,
            declare_refuses_a_reference_to_an_undeclared_type,
            a_relation_removed_retargeted_or_narrowed_or_a_type_turned_view_is_a_migration,
            a_reference_key_field_filled_from_the_key_must_name_a_live_record,
            key_components_and_source_ids_use_each_values_one_form,
            schema_hashes_and_proposal_ids_have_the_stores_form,
            a_reader_is_answered_the_hash_of_the_shape_it_is_described,
            links_are_written_walked_and_held_to_their_relation,
            a_relation_declared_again_with_another_target_reshapes_its_links,
            a_migration_restates_what_its_to_states_otherwise,
            #[own_principals]
            a_migration_shows_a_field_by_its_shape_to_one_who_does_not_read_it,
            a_migration_keeps_a_field_its_author_may_not_know_of,
            a_migration_migrates_and_declares_its_link_types_in_its_append,
            #[host_ops] an_aging_rule_moves_what_holds_from_once_a_date,
            #[host_ops] a_purge_retires_what_one_source_brought_in_and_refuses_a_shared_record,
            #[host_ops] a_purge_preflight_names_what_the_purge_strands_and_its_id_goes_stale,
            #[host_ops] a_purge_that_names_a_type_takes_what_the_source_brought_into_it_alone,
            #[host_ops] a_typed_purge_has_an_id_of_its_own_and_refuses_a_type_it_cannot_take_from,
            a_value_type_is_loosened_by_declare_and_tightened_by_its_preflight,
            a_sort_index_setting_names_declared_scalar_fields_and_describe_shows_them,
            a_vector_index_setting_names_vector_fields_and_search_says_it_ranked_by_the_index,
            a_view_kept_setting_names_a_view_that_can_be_kept_and_describe_says_whether_it_is,
            a_setting_is_judged_by_what_its_writer_is_shown,
            an_ingested_upstream_is_answered_per_field,
            a_schema_proposal_is_refused_by_kind_at_propose_and_at_accept,
            a_value_types_tightening_is_proposed_and_accepted_while_it_fits,
            a_proposal_shows_its_reader_the_values_it_may_read,
            a_proposal_names_what_its_reader_may_know_and_says_there_is_more,
            an_order_by_latest_takes_no_field_and_no_view,
            #[views] a_view_reads_the_parameters_each_read_binds,
            a_parameter_its_type_does_not_declare_is_refused,
            a_trigger_watches_a_link_type_and_not_a_reference_relations,
            degree_and_components_answer_the_graph_in_one_order,
            a_search_over_a_field_the_caller_may_not_read_is_refused_in_every_mode,
            a_search_limited_to_named_fields_matches_and_probes_only_those,
            a_hybrid_search_fuses_its_text_and_vector_ranks,
            #[own_principals]
            a_grant_admits_only_the_records_whose_fields_hold_its_values);
    };
    (@each $at:ident, $new:expr, $block_on:path;
        $($(#[$mark:ident])? $name:ident),* $(,)?) => {
        $(
            $crate::door_checks!(@one $at, $new, $block_on, [$($mark)?] $name);
        )*
    };
    (@one remote, $new:expr, $block_on:path, [host_ops] $name:ident) => {
        #[test]
        #[ignore = "calls a host operation, which no network door offers"]
        fn $name() {
            $block_on($crate::checks::$name(($new)()));
        }
    };
    (@one remote, $new:expr, $block_on:path, [own_principals] $name:ident) => {
        #[test]
        #[ignore = "makes principals of its own, which a network door's resolver does not know"]
        fn $name() {
            $block_on($crate::checks::$name(($new)()));
        }
    };
    (@one remote, $new:expr, $block_on:path, [in_process_feed] $name:ident) => {
        #[test]
        #[ignore = "reads where one poll stops; the server's stream reads on to a match or the head"]
        fn $name() {
            $block_on($crate::checks::$name(($new)()));
        }
    };
    (@one mock, $new:expr, $block_on:path, [views] $name:ident) => {
        #[test]
        #[ignore = "reads a view's records, which the mock does not compute"]
        fn $name() {
            $block_on($crate::checks::$name(($new)()));
        }
    };
    (@one $at:ident, $new:expr, $block_on:path, [$($mark:ident)?] $name:ident) => {
        #[test]
        fn $name() {
            $block_on($crate::checks::$name(($new)()));
        }
    };
}

// ---- the fixture ----

const INVOICE: &str = "invoice";
const PARTNER: &str = "partner";

fn text(name: &str) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: name.into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: Vec::new(),
        on_input_change: OnInputChange::Keep,
        initial: None,
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

/// The fixture's ontology: invoices from a source, judged by authors,
/// linked to partners, with one verb that runs and one that waits.
pub fn fixture() -> Vec<Declaration> {
    let invoice = TypeDecl {
        annotations: Default::default(),
        name: INVOICE.into(),
        key: Vec::new(),
        fields: vec![
            FieldDecl {
                ty: FieldType::kind(StorageKind::Decimal),
                authority: Authority::Source,
                ..text("amount")
            },
            FieldDecl {
                searchable: true,
                ..text("status")
            },
            text("verdict"),
            FieldDecl {
                depends_on: vec!["verdict".into()],
                on_input_change: OnInputChange::Clear,
                ..text("reason")
            },
            FieldDecl {
                sensitive: true,
                ..text("note")
            },
            text("secret"),
            FieldDecl {
                ty: FieldType {
                    target: Some(PARTNER.into()),
                    ..FieldType::kind(StorageKind::Reference)
                },
                ..text("partner")
            },
        ],
        relations: vec![RelationDecl {
            derived: None,
            annotations: Default::default(),
            name: "billed_by".into(),
            target: PARTNER.into(),
            cardinality: Default::default(),
            inverse: Some("bills".into()),
            via: Some("partner".into()),
            fields: Vec::new(),
            authority: Default::default(),
            description: None,
        }],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: Some("A bill received.".into()),
        label: None,
    };
    let partner = TypeDecl {
        name: PARTNER.into(),
        fields: vec![FieldDecl {
            searchable: true,
            ..text("name")
        }],
        relations: Vec::new(),
        description: None,
        ..invoice.clone()
    };
    let set = |status: &str| VerbEffect::Write {
        set: vec![Assignment {
            field: "status".into(),
            value: Condition::from(format!("'{status}'").as_str()),
        }],
    };
    vec![
        Declaration::Type(partner),
        Declaration::Type(invoice),
        Declaration::Verb(VerbDecl {
            name: "approve".into(),
            on: INVOICE.into(),
            scope: Default::default(),
            payload: Vec::new(),
            risk: Risk::Low,
            requires: None,
            effect: set("approved"),
            description: None,
        }),
        Declaration::Verb(VerbDecl {
            name: "pay".into(),
            on: INVOICE.into(),
            scope: Default::default(),
            payload: vec![text("reference")],
            risk: Risk::High,
            requires: Some(Condition::from("status = 'approved'")),
            effect: set("paid"),
            description: None,
        }),
    ]
}

fn principal(lake: &LakeId, kind: PrincipalKind, id: &str, grants: Grants) -> Principal {
    Principal {
        kind,
        id: id.into(),
        lake: lake.clone(),
        grants,
    }
}

fn invoice_grants(default: Access, fields: &[(&str, Access)]) -> Grants {
    Grants {
        default,
        types: [(
            TypeName::from(INVOICE),
            TypeGrants {
                default: None,
                fields: fields
                    .iter()
                    .map(|(f, a)| (FieldName::from(*f), *a))
                    .collect(),
                ..Default::default()
            },
        )]
        .into(),
        ..Grants::default()
    }
}

/// The fixture's principals.
pub struct Cast {
    /// Everything, on every type.
    pub owner: Principal,
    /// Reads everything but `secret`, sees `note` masked, writes `status`
    /// and `partner`; of a `doc`, which a check may declare, sees `body`
    /// masked; of a `credit`, which a check may declare, reads everything
    /// but `hush`.
    pub clerk: Principal,
    /// The clerk, holding the `schema` grant.
    pub architect: Principal,
    /// Reads everything, writes `verdict` and `reason`.
    pub auditor: Principal,
    /// Reads, and reviews proposals.
    pub reviewer: Principal,
    /// Feeds `amount` from a source.
    pub importer: Principal,
    /// Reads `status` of invoices and nothing else.
    pub stranger: Principal,
    /// Writes `status`, and writes `secret` without reading it.
    pub keeper: Principal,
    /// Writes the lake's settings, reads `status` of invoices, and reads a
    /// `longest`, which a check may declare.
    pub tuner: Principal,
}

impl Cast {
    /// The cast for one lake.
    pub fn of(lake: &LakeId) -> Cast {
        let mut importer = invoice_grants(Access::None, &[("amount", Access::Write)]);
        importer.ingest = true;
        let mut clerk = invoice_grants(
            Access::Read,
            &[
                ("status", Access::Write),
                ("partner", Access::Write),
                ("secret", Access::None),
                ("note", Access::Masked),
            ],
        );
        clerk.types.insert(
            TypeName::from("doc"),
            TypeGrants {
                default: None,
                fields: [(FieldName::from("body"), Access::Masked)].into(),
                ..Default::default()
            },
        );
        clerk.types.insert(
            TypeName::from("credit"),
            TypeGrants {
                default: Some(Access::Read),
                fields: [(FieldName::from("hush"), Access::None)].into(),
                ..Default::default()
            },
        );
        let architect = Grants {
            schema: true,
            ..clerk.clone()
        };
        let mut tuner = invoice_grants(Access::None, &[("status", Access::Read)]);
        for (name, access) in [("sys.setting", Access::Write), ("longest", Access::Read)] {
            let grants = TypeGrants {
                default: Some(access),
                fields: Default::default(),
                ..Default::default()
            };
            tuner.types.insert(TypeName::from(name), grants);
        }
        Cast {
            owner: principal(lake, PrincipalKind::Human, "owner", Grants::owner()),
            clerk: principal(lake, PrincipalKind::Agent, "clerk", clerk),
            architect: principal(lake, PrincipalKind::Agent, "architect", architect),
            auditor: principal(
                lake,
                PrincipalKind::Human,
                "auditor",
                invoice_grants(
                    Access::Read,
                    &[("verdict", Access::Write), ("reason", Access::Write)],
                ),
            ),
            reviewer: principal(
                lake,
                PrincipalKind::Human,
                "reviewer",
                Grants {
                    review: true,
                    ..invoice_grants(Access::Read, &[])
                },
            ),
            importer: principal(lake, PrincipalKind::Service, "importer", importer),
            stranger: principal(
                lake,
                PrincipalKind::Agent,
                "stranger",
                invoice_grants(Access::None, &[("status", Access::Read)]),
            ),
            keeper: principal(
                lake,
                PrincipalKind::Agent,
                "keeper",
                invoice_grants(
                    Access::None,
                    &[("status", Access::Write), ("secret", Access::WriteOnly)],
                ),
            ),
            tuner: principal(lake, PrincipalKind::Agent, "tuner", tuner),
        }
    }
}

/// A lake with the fixture declared, its catalog and its cast.
pub struct Fixture {
    /// The lake.
    pub store: Arc<dyn LakeStore>,
    /// Its doors, for direct callers.
    pub catalog: Arc<dyn Catalog>,
    /// Who acts.
    pub cast: Cast,
}

/// Declare the fixture in a fresh lake.
pub async fn setup(store: Arc<dyn LakeStore>) -> Fixture {
    let catalog = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    for d in fixture() {
        catalog
            .declare(&cast.owner, d)
            .await
            .expect("the owner declares the fixture");
    }
    Fixture {
        store,
        catalog,
        cast,
    }
}

/// A patch from pairs.
pub fn patch(pairs: &[(&str, Value)]) -> Patch {
    pairs
        .iter()
        .map(|(k, v)| (FieldName::from(*k), v.clone()))
        .collect()
}

fn ty(name: &str) -> TypeName {
    TypeName::from(name)
}

fn key(k: &str) -> RecordKey {
    RecordKey::from(k)
}

fn get(k: &str) -> Get {
    Get {
        key: key(k),
        with_meta: false,
        at: ReadAt::default(),
        params: Default::default(),
    }
}

fn at(p: Position) -> ReadAt {
    ReadAt {
        as_of: Some(p),
        valid_at: None,
    }
}

fn applied(o: &Outcome) -> (Position, Option<Version>) {
    match o {
        Outcome::Applied {
            position, version, ..
        } => (*position, *version),
        other => panic!("expected the write to apply, got {other:?}"),
    }
}

/// Whether an error is the answer for a field the type does not declare,
/// naming `name`: also the answer for one the principal may not know of.
fn no_field(e: &Error, name: &str) -> bool {
    matches!(refusal(e), (Some(f), Refusal::Invalid { message })
        if f.as_str() == name && message.contains("has no field"))
}

fn refusal(e: &Error) -> (&Option<FieldName>, &Refusal) {
    match e {
        Error::Refused { field, reason } => (field, reason),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

impl Fixture {
    async fn write(&self, who: &Principal, k: &str, pairs: &[(&str, Value)]) -> Outcome {
        self.catalog
            .upsert(
                who,
                &ty(INVOICE),
                &key(k),
                patch(pairs),
                WriteOpts::default(),
            )
            .await
            .expect("the write applies")
    }

    async fn read(&self, who: &Principal, k: &str) -> Record {
        self.catalog
            .get(who, &ty(INVOICE), get(k))
            .await
            .expect("the record reads")
            .data
    }

    async fn head(&self) -> Position {
        self.store.head().await.expect("the lake has a head")
    }
}

fn field<'a>(r: &'a Record, f: &str) -> Option<&'a Value> {
    r.fields.get(&FieldName::from(f))
}

// ---- the checks ----

/// Every read carries a position; every write answers one past the
/// head it saw, and the head moves to it. A write that sets nothing off
/// answers its outcome as it always has, with no member more.
pub async fn every_write_advances_the_position(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let before = f.head().await;
    let first = f
        .write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    let keys = serde_json::to_value(&first).unwrap();
    let keys: Vec<&String> = keys.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["outcome", "position", "version"], "{first:?}");
    let (p1, v1) = applied(&first);
    assert!(p1 > before, "a write lands after the head it saw");
    assert_eq!(f.head().await, p1);
    assert_eq!(v1, Some(Version(1)));
    let (p2, v2) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("held"))])
            .await,
    );
    assert!(p2 > p1);
    assert_eq!(v2, Some(Version(2)));
    let reply = f
        .catalog
        .get(&f.cast.clerk, &ty(INVOICE), get("INV-1"))
        .await
        .unwrap();
    assert_eq!(reply.position, p2, "a read at the head answers the head");
    let d = f
        .catalog
        .describe(&f.cast.clerk, ReadAt::default())
        .await
        .unwrap();
    assert_eq!(d.position, p2);
}

/// `upsert` is a sparse patch.
pub async fn an_absent_field_is_unchanged_and_null_clears_it(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let o = &f.cast.owner;
    f.write(
        o,
        "INV-1",
        &[("status", json!("open")), ("verdict", json!("ok"))],
    )
    .await;
    f.write(o, "INV-1", &[("status", json!("held"))]).await;
    let r = f.read(o, "INV-1").await;
    assert_eq!(field(&r, "status"), Some(&json!("held")));
    assert_eq!(
        field(&r, "verdict"),
        Some(&json!("ok")),
        "absent means unchanged"
    );
    f.write(o, "INV-1", &[("verdict", Value::Null)]).await;
    let r = f.read(o, "INV-1").await;
    assert_eq!(field(&r, "verdict"), None, "null clears");
    assert_eq!(field(&r, "status"), Some(&json!("held")));
}

/// A patch with one field the principal may not write is refused whole.
pub async fn one_forbidden_field_refuses_the_whole_write(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.owner, "INV-1", &[("status", json!("open"))])
        .await;
    let head = f.head().await;
    let e = f
        .catalog
        .upsert(
            &f.cast.clerk,
            &ty(INVOICE),
            &key("INV-1"),
            patch(&[("status", json!("held")), ("verdict", json!("bad"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    let (fld, why) = refusal(&e);
    assert_eq!(fld.as_ref().map(FieldName::as_str), Some("verdict"));
    assert_eq!(why, &Refusal::NotGranted);
    assert_eq!(f.head().await, head, "nothing was appended");
    let r = f.read(&f.cast.owner, "INV-1").await;
    assert_eq!(
        field(&r, "status"),
        Some(&json!("open")),
        "no field was applied"
    );
}

/// A source field through `upsert` is refused as the wrong door, even
/// for a principal who may write it.
pub async fn a_field_of_another_authority_is_the_wrong_door(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let e = f
        .catalog
        .upsert(
            &f.cast.owner,
            &ty(INVOICE),
            &key("INV-1"),
            patch(&[("amount", json!("10.00"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::WrongDoor);
}

/// A write that expects another version answers `Conflict` and writes
/// nothing; `Version(0)` means the record must not exist yet.
pub async fn a_stale_expected_version_is_a_conflict(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let c = &f.catalog;
    let expect = |v| WriteOpts {
        expected_version: Some(Version(v)),
        idempotency_key: None,
    };
    let p = || patch(&[("status", json!("open"))]);
    let o = c
        .upsert(&f.cast.clerk, &ty(INVOICE), &key("INV-1"), p(), expect(0))
        .await;
    assert_eq!(applied(&o.unwrap()).1, Some(Version(1)));
    let head = f.head().await;
    let e = c
        .upsert(&f.cast.clerk, &ty(INVOICE), &key("INV-1"), p(), expect(0))
        .await;
    assert_eq!(
        e.unwrap_err(),
        Error::Conflict {
            expected: Version(0),
            actual: Version(1)
        }
    );
    assert_eq!(f.head().await, head, "nothing was appended");
    let o = c
        .upsert(&f.cast.clerk, &ty(INVOICE), &key("INV-1"), p(), expect(1))
        .await;
    assert_eq!(applied(&o.unwrap()).1, Some(Version(2)));
}

/// A retry with the same idempotency key from the same principal answers
/// the first outcome and appends nothing.
pub async fn a_retried_idempotency_key_answers_the_first_outcome(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let opts = WriteOpts {
        expected_version: None,
        idempotency_key: Some("retry-1".into()),
    };
    let (invoice, k) = (ty(INVOICE), key("INV-1"));
    let write = || {
        f.catalog.upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts.clone(),
        )
    };
    let first = write().await.unwrap();
    let head = f.head().await;
    let second = write().await.unwrap();
    assert_eq!(first, second);
    assert_eq!(f.head().await, head, "the retry appended nothing");
    assert_eq!(f.read(&f.cast.clerk, "INV-1").await.version, Version(1));
}

/// A read pinned `as_of` an earlier position sees the state the lake
/// held there, and says so.
pub async fn a_read_as_of_an_earlier_position_sees_the_earlier_state(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (p1, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("paid"))])
        .await;
    let then = f
        .catalog
        .get(
            &f.cast.clerk,
            &ty(INVOICE),
            Get {
                at: at(p1),
                ..get("INV-1")
            },
        )
        .await
        .unwrap();
    assert_eq!(then.position, p1);
    assert_eq!(field(&then.data, "status"), Some(&json!("open")));
    assert_eq!(then.data.version, Version(1));
    let before = f
        .catalog
        .get(
            &f.cast.clerk,
            &ty(INVOICE),
            Get {
                at: at(Position(p1.0 - 1)),
                ..get("INV-1")
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        before,
        Error::NotFound {
            missing: Missing::Record { .. }
        }
    ));
}

/// A position the log has not reached is refused, naming the head.
pub async fn a_position_past_the_head_is_out_of_range(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let head = f.head().await;
    let e = f
        .catalog
        .describe(&f.cast.owner, at(head.next()))
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::OutOfRange { head });
}

/// Types, fields and verbs the principal holds no grant for are absent,
/// not listed as forbidden.
pub async fn describe_shows_only_what_the_principal_may_see(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let d = f
        .catalog
        .describe(&f.cast.stranger, ReadAt::default())
        .await
        .unwrap()
        .data;
    let types: Vec<&str> = d.types.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(types, [INVOICE]);
    let fields: Vec<&str> = d.types[0].fields.iter().map(|x| x.name.as_str()).collect();
    assert!(fields.contains(&"status"));
    assert!(!fields.contains(&"secret") && !fields.contains(&"verdict"));
    assert!(d.verbs.is_empty(), "the stranger may call no verb");
    assert!(!d.types[0].may.upsert);
    let e = f
        .catalog
        .describe_type(&f.cast.stranger, &ty(PARTNER), ReadAt::default())
        .await
        .unwrap_err();
    assert!(matches!(
        e,
        Error::NotFound {
            missing: Missing::Type { .. }
        }
    ));

    let d = f
        .catalog
        .describe(&f.cast.clerk, ReadAt::default())
        .await
        .unwrap()
        .data;
    let inv = d.types.iter().find(|t| t.name.as_str() == INVOICE).unwrap();
    let note = inv
        .fields
        .iter()
        .find(|x| x.name.as_str() == "note")
        .unwrap();
    assert_eq!(note.access, Access::Masked);
    assert!(inv.fields.iter().all(|x| x.name.as_str() != "secret"));
    // A match over a field the clerk may not know of is no relation to it;
    // one over a field it sees masked is (lakelands#662).
    let by = |field: &str| {
        json!({"name": format!("by_{field}"), "target": INVOICE,
               "derived": {"match": [{"from": "x", "to": field}]}})
    };
    let ledger = json!({"declare": "type", "name": "ledger",
        "fields": [{"name": "x", "type": {"kind": "text"}}],
        "relations": [by("secret"), by("note")]});
    let ledger: Declaration = serde_json::from_value(ledger).expect("a declaration");
    let declared = f.catalog.declare(&f.cast.owner, ledger).await;
    declared.expect("the owner declares `ledger`");
    let (clerk, ledger) = (&f.cast.clerk, ty("ledger"));
    let seen = f
        .catalog
        .describe_type(clerk, &ledger, ReadAt::default())
        .await;
    let seen = seen.expect("the clerk is described `ledger`").data;
    let names: Vec<&str> = seen.relations.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["by_note"],
        "the clerk is described the match it may know of"
    );
}

/// A described field, read back as the declaration it describes.
fn as_declared(f: &lakelands_api::describe::FieldDescription) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: f.name.clone(),
        ty: f.ty.clone(),
        required: f.required,
        authority: f.authority,
        derived: f.derived.clone(),
        function: f.function.clone(),
        depends_on: f.depends_on.clone(),
        on_input_change: f.on_input_change,
        initial: f.initial.clone(),
        sensitive: f.sensitive,
        searchable: f.searchable,
        description: f.description.clone(),
        label: f.label.clone(),
        embed: f.embed.clone(),
    }
}

/// `describe` answers each field's declaration whole to a principal who
/// reads it and a masked field's shape only; functions, triggers and
/// object sets are answered as declared, to a principal who reads every
/// field they name or holds the `schema` grant.
pub async fn describe_answers_the_declaration_whole(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let function = |name: &str, input: &str, expr: &str| FunctionDecl {
        name: name.into(),
        backing: "expr".into(),
        inputs: vec![text(input)],
        expr: Some(Condition::from(expr)),
        config: Value::Null,
        description: Some(format!("{name} of {input}")),
    };
    let described = |name: &str| FieldDecl {
        label: Some(name.to_uppercase()),
        description: Some(format!("the invoice's {name}")),
        ..text(name)
    };
    let derived = |name: &str, by: &str, from: &str| FieldDecl {
        authority: Authority::Derived,
        function: Some(by.into()),
        depends_on: vec![from.into()],
        ..described(name)
    };
    // The fixture's invoice, labelled and described throughout, with a
    // field computed from the status and one from the masked note.
    let Some(Declaration::Type(mut invoice)) = fixture()
        .into_iter()
        .find(|d| matches!(d, Declaration::Type(t) if t.name.as_str() == INVOICE))
    else {
        unreachable!("the fixture declares the invoice")
    };
    for x in &mut invoice.fields {
        x.label = Some(x.name.as_str().to_uppercase());
        x.description = Some(format!("the invoice's {}", x.name));
    }
    invoice.fields.push(derived("loud", "shout", "status"));
    invoice.fields.push(derived("quiet", "hush", "note"));
    invoice.language = Some("en".into());
    invoice.label = Some("Invoice".into());
    let trigger = |name: &str, when: &str| TriggerDecl {
        max_depth: None,
        name: name.into(),
        on: INVOICE.into(),
        when: Some(Condition::from(when)),
        description: None,
    };
    let set = |name: &str, filter: &str| ObjectSetDecl {
        name: name.into(),
        of: INVOICE.into(),
        filter: Condition::from(filter),
        description: Some(filter.into()),
    };
    let (shout, hush) = (
        function("shout", "status", "upper(status)"),
        function("hush", "note", "lower(note)"),
    );
    let (opened, noted) = (
        trigger("opened", "status = 'open'"),
        trigger("noted", "note = 'b'"),
    );
    let (open_set, noted_set) = (set("open", "status = 'open'"), set("with_b", "note = 'b'"));
    for d in [
        Declaration::Function(shout.clone()),
        Declaration::Function(hush.clone()),
        Declaration::Type(invoice.clone()),
        Declaration::Trigger(opened.clone()),
        Declaration::Trigger(noted.clone()),
        Declaration::ObjectSet(open_set.clone()),
        Declaration::ObjectSet(noted_set.clone()),
    ] {
        f.catalog.declare(owner, d).await.unwrap();
    }

    let t = f
        .catalog
        .describe_type(owner, &ty(INVOICE), ReadAt::default())
        .await
        .unwrap()
        .data;
    let fields: Vec<FieldDecl> = t.fields.iter().map(as_declared).collect();
    assert_eq!(fields, invoice.fields, "the owner reads every field whole");
    assert_eq!(
        (&t.label, &t.language, &t.description, &t.view),
        (
            &invoice.label,
            &invoice.language,
            &invoice.description,
            &invoice.view.clone().map(|v| *v)
        )
    );
    let d = f
        .catalog
        .describe(owner, ReadAt::default())
        .await
        .unwrap()
        .data;
    assert_eq!(d.functions, [hush.clone(), shout.clone()]);
    assert_eq!(d.triggers, [noted.clone(), opened.clone()]);
    assert_eq!(d.object_sets, [open_set.clone(), noted_set.clone()]);

    // The clerk: the note masked, `secret` absent.
    let d = f
        .catalog
        .describe(&f.cast.clerk, ReadAt::default())
        .await
        .unwrap()
        .data;
    let t = d.types.iter().find(|t| t.name.as_str() == INVOICE).unwrap();
    let note = t.fields.iter().find(|x| x.name.as_str() == "note").unwrap();
    assert_eq!(
        as_declared(note),
        FieldDecl {
            sensitive: true,
            ..text("note")
        },
        "a masked field shows its shape only"
    );
    assert_eq!((note.access, note.filterable), (Access::Masked, false));
    let quiet = t
        .fields
        .iter()
        .find(|x| x.name.as_str() == "quiet")
        .unwrap();
    assert_eq!(
        (quiet.function.as_deref(), quiet.access),
        (None, Access::Masked),
        "a field computed from a masked one is masked, and shows its shape only"
    );
    assert_eq!(d.functions, [shout], "`hush` reads the note");
    assert_eq!(d.triggers, [opened], "`noted` names the note");
    assert_eq!(d.object_sets, [open_set], "`with_b` names the note");
}

/// A field granted `write_only` is written like a writable one and never
/// shown: absent from `get` (not masked), refused as not granted when a
/// filter probes it, and described with its access (006, extended).
pub async fn a_write_only_field_is_written_and_never_shown(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let keeper = f.cast.keeper.clone();
    let written = f
        .catalog
        .upsert(
            &keeper,
            &ty(INVOICE),
            &key("INV-1"),
            patch(&[("status", json!("open")), ("secret", json!("s3"))]),
            WriteOpts::default(),
        )
        .await
        .expect("a write-only field is written");
    assert!(
        matches!(written, Outcome::Applied { output: None, .. }),
        "{written:?}"
    );
    assert_eq!(
        f.read(&f.cast.owner, "INV-1").await.fields.get("secret"),
        Some(&json!("s3"))
    );
    let seen = f.read(&keeper, "INV-1").await;
    assert!(
        !seen.fields.contains_key("secret") && !seen.masked.iter().any(|m| m.as_str() == "secret"),
        "{seen:?}"
    );
    let probe = Find {
        filter: Some("secret = 's3'".into()),
        ..Find::default()
    };
    let e = f
        .catalog
        .find(&keeper, &ty(INVOICE), probe)
        .await
        .unwrap_err();
    let (field, reason) = refusal(&e);
    assert!(
        field.as_ref().is_some_and(|f| f.as_str() == "secret") && *reason == Refusal::NotGranted,
        "{e:?}"
    );
    let d = f
        .catalog
        .describe_type(&keeper, &ty(INVOICE), ReadAt::default())
        .await
        .unwrap()
        .data;
    assert!(
        d.fields
            .iter()
            .any(|x| x.name.as_str() == "secret" && x.access == Access::WriteOnly),
        "{:?}",
        d.fields
    );
}

/// A masked field is named in the answer; its value is not in it, and
/// a field the principal may not see at all is not named either.
pub async fn a_masked_field_is_named_but_its_value_is_absent(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(
        &f.cast.owner,
        "INV-1",
        &[
            ("note", json!("call back")),
            ("secret", json!("s3")),
            ("status", json!("open")),
        ],
    )
    .await;
    let r = f.read(&f.cast.clerk, "INV-1").await;
    assert_eq!(field(&r, "note"), None);
    assert_eq!(r.masked, [FieldName::from("note")]);
    assert_eq!(field(&r, "secret"), None);
    assert!(!r.masked.contains(&"secret".into()));
    let e = f
        .catalog
        .find(
            &f.cast.clerk,
            &ty(INVOICE),
            Find {
                filter: Some("secret = 's3'".into()),
                ..Find::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        no_field(&e, "secret"),
        "a filter cannot probe a hidden field, and is answered as for a field the type does \
         not declare: {e:?}"
    );
    let w = f
        .catalog
        .upsert(
            &f.cast.clerk,
            &ty(INVOICE),
            &key("INV-1"),
            patch(&[("secret", json!("x"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(
        no_field(&w, "secret"),
        "nor is a hidden field written: {w:?}"
    );
    // A pattern's rows ordered by a field probe it as a filter does.
    for (path, field, hidden) in [
        ("invoice.secret", "secret", true),
        ("invoice.note", "note", false),
    ] {
        let p: Pattern = serde_json::from_value(json!({
            "start": {"type": INVOICE},
            "project": {"as": "rows"},
            "order_by": [{"field": path}]
        }))
        .expect("a pattern");
        let e = f.catalog.query(&f.cast.clerk, p).await.unwrap_err();
        let answered = if hidden {
            no_field(&e, field)
        } else {
            refusal(&e) == (&Some(FieldName::from(field)), &Refusal::NotGranted)
        };
        assert!(answered, "rows ordered by `{path}`: {e:?}");
    }
    // A sensitive field nobody named for the stranger, whose type default
    // is none, is listed as masked rather than left out.
    let ssn: FieldDecl =
        serde_json::from_value(json!({"name": "ssn", "type": {"kind": "text"}, "sensitive": true}))
            .expect("a field");
    f.catalog
        .add_field(&f.cast.owner, &ty(INVOICE), ssn)
        .await
        .expect("the owner adds `ssn`");
    f.write(&f.cast.owner, "INV-1", &[("ssn", json!("1"))])
        .await;
    let r = f.read(&f.cast.stranger, "INV-1").await;
    assert!(r.masked.contains(&"ssn".into()), "{r:?}");
}

/// A read at a valid time filters on the type's valid-time field, so it
/// probes it: the clerk, who sees a `doc`'s `body` masked, may read a
/// `doc` valid from `body`, but not at a valid time.
pub async fn a_valid_time_read_probes_the_valid_time_fields(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let doc: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "doc", "valid_from": "body",
        "fields": [{"name": "body", "type": {"kind": "date_time"}}]
    }))
    .expect("a declaration");
    f.catalog
        .declare(&f.cast.owner, doc)
        .await
        .expect("the owner declares `doc`");
    let at = |valid_at: Option<&str>| Find {
        at: ReadAt {
            valid_at: valid_at.map(|t| serde_json::from_value(json!(t)).expect("an instant")),
            ..ReadAt::default()
        },
        ..Find::default()
    };
    let doc = ty("doc");
    f.catalog
        .find(&f.cast.clerk, &doc, at(None))
        .await
        .expect("a read without a valid time");
    let e = f
        .catalog
        .find(&f.cast.clerk, &doc, at(Some("2026-04-01T00:00:00Z")))
        .await
        .unwrap_err();
    assert_eq!(
        refusal(&e),
        (&Some(FieldName::from("body")), &Refusal::NotGranted),
        "{e:?}"
    );
}

/// A field derived from one the principal may not read is read as that
/// field: a filter on it is refused naming it, and `describe` leaves it
/// out.
pub async fn a_field_built_from_a_hidden_one_is_hidden_too(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    // `g` lists what it is derived from; `h` does not, and is held to it
    // all the same.
    for (name, listed) in [("g", json!(["secret"])), ("h", json!([]))] {
        let field: FieldDecl = serde_json::from_value(json!({
            "name": name, "type": {"kind": "text"}, "authority": "derived",
            "derived": "secret", "depends_on": listed
        }))
        .expect("a field");
        f.catalog
            .add_field(&f.cast.owner, &ty(INVOICE), field)
            .await
            .expect("the owner adds a field derived from `secret`");
        let find = Find {
            filter: Some(format!("{name} = 's3'").as_str().into()),
            ..Find::default()
        };
        let e = f
            .catalog
            .find(&f.cast.clerk, &ty(INVOICE), find)
            .await
            .unwrap_err();
        assert!(
            no_field(&e, name),
            "a filter on `{name}`, derived from the hidden `secret`, is answered as for a \
             field the type does not declare: {e:?}"
        );
        let d = f
            .catalog
            .describe_type(&f.cast.clerk, &ty(INVOICE), ReadAt::default())
            .await
            .expect("describing `invoice`")
            .data;
        assert!(
            d.fields.iter().all(|x| x.name.as_str() != name),
            "`{name}` hands out `secret`, so it is described as `secret` is: not at all; {d:?}"
        );
    }
    // A proposed shape is completed as one in force is, and `more` says
    // what was left out, not what was completed: the owner lists a proposed
    // type and a proposed migration whole, with the list `x` did not give.
    let memo = |name: &str, x: bool| -> TypeDecl {
        let a = json!({"name": "a", "type": {"kind": "text"}});
        let x = x.then(|| json!({"name": "x", "type": {"kind": "text"}, "authority": "derived", "derived": "a"}));
        let fields: Vec<Value> = [a].into_iter().chain(x).collect();
        serde_json::from_value(json!({"name": name, "fields": fields})).expect("a type")
    };
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let declared = c.declare(owner, Declaration::Type(memo("memo", false)));
    declared.await.expect("the owner declares `memo`");
    let migrate = ProposedWrite::Migrate {
        migration: Box::new(lakelands_api::write::Migration {
            to: memo("memo", true),
            renames: vec![],
            relation_renames: Vec::new(),
        }),
        report: Default::default(),
    };
    let declare = ProposedWrite::Declare {
        declarations: vec![Declaration::Type(memo("minute", true))],
    };
    for write in [migrate, declare] {
        let proposed = c.propose(owner, write, "adding x".into()).await;
        proposed.expect("the owner proposes a shape");
    }
    let listed = c.proposals(owner, ProposalFilter::default()).await;
    let listed = listed.expect("the proposals are listed").data;
    let whole = |p: &&lakelands_api::write::Proposal| {
        !p.more && json!(p.write).to_string().contains(r#""depends_on":["a"]"#)
    };
    assert!(
        listed.len() == 2 && listed.iter().filter(whole).count() == 2,
        "the owner is shown each proposed shape with `x`'s completed list and no `more`: \
         {listed:?}"
    );
}

/// A grant by name holds a field through its renames
/// (`docs/decisions/006-grants-not-roles.md`, extended): the clerk sees
/// `body` of a `doc` masked; renamed to `text`, the field is masked under
/// that name, and under `body` in a read as of before the rename. The
/// migration's answer names the rename.
pub async fn a_grant_holds_a_field_through_its_rename(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, owner, clerk) = (&f.catalog, &f.cast.owner, &f.cast.clerk);
    let doc = |body: &str| -> TypeDecl {
        serde_json::from_value(json!({"name": "doc", "fields": [
            {"name": "title", "type": {"kind": "text"}},
            {"name": body, "type": {"kind": "text"}}]}))
        .expect("a type")
    };
    let declared = c.declare(owner, Declaration::Type(doc("body"))).await;
    declared.expect("the owner declares `doc`");
    let fields = patch(&[("title", json!("t")), ("body", json!("b"))]);
    let (doc_ty, one) = (ty("doc"), key("1"));
    let wrote = c.upsert(owner, &doc_ty, &one, fields, WriteOpts::default());
    let before = applied(&wrote.await.expect("the owner writes a `doc`")).0;
    let m = lakelands_api::write::Migration {
        to: doc("text"),
        renames: vec![("body".into(), "text".into())],
        relation_renames: Vec::new(),
    };
    let report = c.migrate(owner, m.clone()).await.expect("the preflight");
    let answer = c
        .migrate_apply(owner, m.clone(), &report.data.report_id)
        .await;
    assert!(
        matches!(&answer, Ok(Outcome::Applied { renamed, .. }) if renamed == &m.renames),
        "the migration's answer names the rename: {answer:?}"
    );
    for (name, as_of) in [("text", None), ("body", Some(before))] {
        let read = Get {
            at: ReadAt {
                as_of,
                valid_at: None,
            },
            ..get("1")
        };
        let got = c
            .get(clerk, &ty("doc"), read)
            .await
            .expect("the clerk reads")
            .data;
        assert!(
            field(&got, name).is_none() && got.masked.iter().any(|m| m.as_str() == name),
            "the clerk sees `{name}` masked as of {as_of:?}, as it saw `body`: {got:?}"
        );
    }
}

/// A type its proposer is not shown is a type that is not declared
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
/// extended): the stranger, who reads `status` of invoices and nothing
/// else, proposes a trigger on `vault`, a migration of it and the type
/// itself. The first two answer as for `ghost`, which is not declared; the
/// third is not granted.
pub async fn a_type_its_proposer_is_not_shown_is_a_type_that_is_not_declared(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (c, owner, stranger) = (&f.catalog, &f.cast.owner, &f.cast.stranger);
    let shape = |name: &str| -> TypeDecl {
        let x = json!({"name": "x", "type": {"kind": "text"}});
        serde_json::from_value(json!({"name": name, "fields": [x]})).expect("a type")
    };
    let declared = c.declare(owner, Declaration::Type(shape("vault"))).await;
    declared.expect("the owner declares `vault`");
    let said = |name: &'static str| {
        let (c, who) = (c.clone(), stranger.clone());
        let trigger = json!({"declare": "trigger", "name": "t", "on": name});
        let trigger = serde_json::from_value(trigger).expect("a trigger");
        let migration = Box::new(lakelands_api::write::Migration {
            to: shape(name),
            renames: vec![],
            relation_renames: Vec::new(),
        });
        let writes = [
            ProposedWrite::Declare {
                declarations: vec![trigger],
            },
            ProposedWrite::Migrate {
                migration,
                report: Default::default(),
            },
        ];
        async move {
            let mut said = Vec::new();
            for write in writes {
                let e = c.propose(&who, write, "why".into()).await;
                said.push(
                    e.expect_err("a proposal on a type that is not there")
                        .to_string(),
                );
            }
            said
        }
    };
    let (hidden, absent) = (said("vault").await, said("ghost").await);
    let absent: Vec<String> = absent.iter().map(|e| e.replace("ghost", "vault")).collect();
    assert_eq!(
        hidden, absent,
        "`vault` answers the stranger as `ghost` does"
    );
    let bundle = ProposedWrite::Declare {
        declarations: vec![Declaration::Type(shape("vault"))],
    };
    let e = c.propose(stranger, bundle, "why".into()).await.unwrap_err();
    assert!(
        matches!(refusal(&e), (None, Refusal::NotGranted)),
        "to propose a type its proposer is not shown is not granted, and names nothing: {e:?}"
    );
}

/// The tools a principal is offered come from the generator over what
/// `describe` shows that principal: no door it may not use, no field it
/// may not write.
pub async fn the_tools_follow_the_grants(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let doors = |who: Principal| {
        let c = f.catalog.clone();
        async move {
            let d = c.describe(&who, ReadAt::default()).await.unwrap().data;
            tools(&d).expect("the fixture's names are valid")
        }
    };
    let invoice = ty(INVOICE);
    let clerk = doors(f.cast.clerk.clone()).await;
    let upsert = clerk
        .iter()
        .find(|t| {
            t.door
                == Door::Upsert {
                    ty: invoice.clone(),
                }
        })
        .expect("the clerk may upsert invoices");
    let writable = &upsert.input_schema["properties"]["fields"]["properties"];
    assert!(writable.get("status").is_some());
    for hidden in ["amount", "verdict", "secret"] {
        assert!(
            writable.get(hidden).is_none(),
            "{hidden} is not the clerk's to write"
        );
    }
    assert!(clerk.iter().any(|t| t.door
        == Door::Call {
            verb: "approve".into()
        }));
    assert!(clerk.iter().all(|t| t.door != Door::Accept));

    let stranger = doors(f.cast.stranger.clone()).await;
    assert!(stranger.iter().any(|t| t.door
        == Door::Get {
            ty: invoice.clone()
        }));
    assert!(stranger.iter().all(|t| !matches!(
        &t.door,
        Door::Upsert { .. } | Door::Retire { .. } | Door::Call { .. } | Door::DeclareType
    )));

    let reviewer = doors(f.cast.reviewer.clone()).await;
    assert!(reviewer.iter().any(|t| t.door == Door::Accept));
}

/// `register` on the type `partner`: an external verb with no record, whose
/// executor no lake in these checks binds.
async fn declare_register(f: &Fixture) {
    let register = VerbDecl {
        name: "register".into(),
        on: PARTNER.into(),
        scope: VerbScope::Type,
        payload: vec![text("name")],
        risk: Risk::Low,
        requires: None,
        effect: VerbEffect::External {
            executor: "crm".into(),
        },
        description: None,
    };
    f.catalog
        .declare(&f.cast.owner, Declaration::Verb(register))
        .await
        .expect("the owner declares a verb on a type");
}

/// The tool of a verb on its type takes no `key` and no
/// `expected_version`, since it has no record and no record version, and
/// says it is called on the type; a verb on a record keeps both. An
/// external verb is described with its executor and whether it is bound,
/// and its tool says so; a verb that sets fields has no executor.
pub async fn a_verb_on_its_type_has_a_tool_without_a_key(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    declare_register(&f).await;
    let d = f
        .catalog
        .describe(&f.cast.owner, ReadAt::default())
        .await
        .unwrap();
    let tools = tools(&d.data).expect("the fixture's names are valid");
    let tool = |verb: &str| {
        tools
            .iter()
            .find(|t| t.door == Door::Call { verb: verb.into() })
            .unwrap_or_else(|| panic!("the owner has the tool of `{verb}`"))
    };
    let register = tool("register");
    let props = register.input_schema["properties"].as_object().unwrap();
    for absent in ["key", "expected_version"] {
        assert!(
            !props.contains_key(absent),
            "`register` takes no `{absent}`"
        );
    }
    for present in ["payload", "idempotency_key"] {
        assert!(props.contains_key(present), "`register` takes `{present}`");
    }
    assert_eq!(register.input_schema["required"], json!(["payload"]));
    assert!(
        register
            .description
            .starts_with("Call `register` on the type `partner`."),
        "{:?}",
        register.description
    );
    let approve = tool("approve");
    let props = approve.input_schema["properties"].as_object().unwrap();
    for present in ["key", "expected_version"] {
        assert!(props.contains_key(present), "`approve` takes `{present}`");
    }
    // No lake in these checks binds an executor, so every one says `crm`
    // is not bound, and its tool says what a call does here.
    let executor = |verb: &str| {
        let v = d.data.verbs.iter().find(|v| v.name.as_str() == verb);
        v.and_then(|v| v.executor.clone())
    };
    assert_eq!(
        executor("register"),
        Some(ExecutorDescription {
            name: "crm".into(),
            bound: false
        })
    );
    assert_eq!(
        executor("approve"),
        None,
        "a verb that sets fields has none"
    );
    assert!(
        register
            .description
            .contains("`crm`; that executor is not bound in this lake"),
        "{:?}",
        register.description
    );
}

/// A call's key is held to the verb's scope before anything else is read:
/// a key, or a record version to compare, given to a verb on its type is
/// `invalid`, and so is no key to a verb on a record, each naming the
/// verb. Without a key, a principal who is shown the type and may not
/// write it is `not_granted`, and with no executor bound the call is
/// `unbound`, sent again under its key too: no answer is looked for.
/// Nothing is appended.
pub async fn a_verb_on_its_type_is_called_without_a_key(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    declare_register(&f).await;
    let head = f.head().await;
    let owner = &f.cast.owner;
    let payload = || Patch::from([("name".into(), json!("Acme"))]);
    let call = |who: Principal, verb: &'static str, k: Option<&'static str>, opts: WriteOpts| {
        let c = f.catalog.clone();
        async move {
            c.call(&who, &verb.into(), k.map(key).as_ref(), payload(), opts)
                .await
        }
    };
    let pinned = WriteOpts {
        expected_version: Some(Version(0)),
        ..WriteOpts::default()
    };
    for (verb, k, opts, what) in [
        (
            "register",
            Some("P-1"),
            WriteOpts::default(),
            "a key to a verb on its type",
        ),
        ("register", None, pinned, "a version to a verb on its type"),
        (
            "approve",
            None,
            WriteOpts::default(),
            "no key to a verb on a record",
        ),
    ] {
        let e = call(owner.clone(), verb, k, opts).await.expect_err(what);
        let (_, reason) = refusal(&e);
        let Refusal::Invalid { message } = reason else {
            panic!("{what}: expected invalid, got {e:?}");
        };
        assert!(message.contains(verb), "{what}: {message:?} names `{verb}`");
    }
    let e = call(
        f.cast.auditor.clone(),
        "register",
        None,
        WriteOpts::default(),
    )
    .await
    .expect_err("the auditor reads partners and may not write them");
    assert_eq!(refusal(&e).1, &Refusal::NotGranted, "{e:?}");
    let keyed = WriteOpts {
        idempotency_key: Some("register-once".into()),
        ..WriteOpts::default()
    };
    for opts in [WriteOpts::default(), keyed.clone(), keyed] {
        let e = call(owner.clone(), "register", None, opts)
            .await
            .expect_err("no lake here binds the executor `crm`");
        assert!(matches!(e, Error::Unbound { .. }), "{e:?}");
    }
    let proposed = ProposedWrite::Call {
        verb: "register".into(),
        key: None,
        payload: payload(),
        opts: WriteOpts::default(),
    };
    let e = f
        .catalog
        .propose(owner, proposed, "later".into())
        .await
        .expect_err("a call of `register` could not run when accepted");
    assert!(matches!(e, Error::Unbound { .. }), "{e:?}");
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// A verb's tool says what a call does by the verb's risk: a `low` verb
/// runs at once, whether its effect writes fields or reaches outside the
/// lake; a riskier one is recorded as a proposal. A verb that reaches
/// outside the lake also names its executor and says whether it is bound
/// here, and that a call (or a proposal) is refused, writing nothing,
/// where it is not.
pub async fn a_verb_tool_says_what_its_risk_does(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    for (name, risk) in [("notify", Risk::Low), ("escalate", Risk::Medium)] {
        let verb = VerbDecl {
            name: name.into(),
            on: INVOICE.into(),
            scope: Default::default(),
            payload: Vec::new(),
            risk,
            requires: None,
            effect: VerbEffect::External {
                executor: "mailer".into(),
            },
            description: None,
        };
        f.catalog
            .declare(owner, Declaration::Verb(verb))
            .await
            .unwrap();
    }
    let d = f.catalog.describe(owner, ReadAt::default()).await.unwrap();
    let tools = tools(&d.data).expect("the fixture's names are valid");
    let says = |verb: &str| {
        tools
            .iter()
            .find(|t| t.door == Door::Call { verb: verb.into() })
            .unwrap_or_else(|| panic!("the owner has the tool of `{verb}`"))
            .description
            .clone()
    };
    let runs = "It runs at once";
    let proposed = "It is recorded as a proposal and runs when accepted";
    // No lake in these checks binds `mailer`.
    let unbound = ", through its executor `mailer`; that executor is not bound in this lake, so";
    let call = format!("{unbound} a call is refused and writes nothing.");
    let either = format!("{unbound} a call or a proposal is refused and writes nothing.");
    for (verb, how, end) in [
        ("approve", runs, "."),
        ("notify", runs, call.as_str()),
        ("pay", proposed, "."),
        ("escalate", proposed, either.as_str()),
    ] {
        let said = says(verb);
        assert!(
            said.ends_with(&format!("{how}{end}")),
            "`{verb}`'s tool says {said:?}"
        );
    }
}

/// A type's tools describe their `key`: for a keyed type, the field whose
/// value it is; for a type that declares none, that the writer chooses it,
/// and on a read or a retirement, that it is the key written under.
pub async fn a_tools_key_says_what_to_pass(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let d = f
        .catalog
        .describe(&f.cast.owner, ReadAt::default())
        .await
        .unwrap();
    let tools = tools(&d.data).expect("the fixture's names are valid");
    let key = |door: Door| {
        tools
            .iter()
            .find(|t| t.door == door)
            .unwrap_or_else(|| panic!("the owner has the tool of {door:?}"))
            .input_schema["properties"]["key"]["description"]
            .clone()
    };
    let invoice = || INVOICE.into();
    let setting = || lakelands_api::seam::SETTING_TYPE.into();
    assert_eq!(
        key(Door::Upsert { ty: invoice() }),
        json!(
            "`invoice` declares no key, so the key is yours to choose. It names this record \
             from now on, and a later write with the same key changes it."
        )
    );
    for read in [Door::Get { ty: invoice() }, Door::Retire { ty: invoice() }] {
        assert_eq!(
            key(read.clone()),
            json!("The key the `invoice` record was written under."),
            "{read:?}"
        );
    }
    for door in [Door::Upsert { ty: setting() }, Door::Get { ty: setting() }] {
        assert_eq!(
            key(door.clone()),
            json!("The key of the `sys.setting` record: the value of its `key`."),
            "{door:?}"
        );
    }
}

/// `propose` shows the members each kind of write takes, as the store
/// reads them (a record write's fields under `patch`), and says that a
/// proposed record write or call needs the grants it would need applied.
pub async fn propose_shows_each_writes_shape(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let d = f
        .catalog
        .describe(&f.cast.owner, ReadAt::default())
        .await
        .unwrap();
    let tools = tools(&d.data).expect("the fixture's names are valid");
    let propose = tools
        .iter()
        .find(|t| t.door == Door::Propose)
        .expect("the owner may propose");
    assert!(
        propose
            .description
            .contains("a proposal waits for review, it does not lift a grant"),
        "{}",
        propose.description
    );
    let kinds = propose.input_schema["properties"]["write"]["oneOf"]
        .as_array()
        .expect("one schema per kind of write");
    let required = |write: &str| {
        kinds
            .iter()
            .find(|k| k["properties"]["write"]["const"] == json!(write))
            .unwrap_or_else(|| panic!("a schema for `{write}`"))["required"]
            .clone()
    };
    for (write, members) in [
        ("upsert", json!(["write", "type", "key", "patch"])),
        ("retire", json!(["write", "type", "key"])),
        ("call", json!(["write", "verb", "payload"])),
        ("declare", json!(["write", "declarations"])),
        ("migrate", json!(["write", "migration"])),
    ] {
        assert_eq!(required(write), members, "{write}");
    }
    // A proposal in the shape shown is read as the store reads it.
    let o = f
        .catalog
        .propose(
            &f.cast.owner,
            serde_json::from_value(json!({"write": "upsert", "type": INVOICE, "key": "INV-9",
                                          "patch": {"status": "open"}}))
            .expect("the shown shape reads as a proposed write"),
            "the shape the tool shows".into(),
        )
        .await
        .unwrap();
    assert!(matches!(o, Outcome::Proposed { .. }), "{o:?}");
}

/// A view is refused alike at every door that writes or holds a record,
/// as invalid naming it; a verb on a record of a view is refused when it
/// is declared, and one on the view's type that reaches outside is taken
/// (018, extended).
pub async fn every_door_refuses_a_view_alike(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let c = &f.catalog;
    c.declare(owner, Declaration::Type(partner_totals_view()))
        .await
        .unwrap();
    let head = f.head().await;
    let (pt, p1) = (ty("partner_totals"), key("p1"));
    let fields = || patch(&[("count", json!("1"))]);
    let names_it = |what: &str, e: Error| {
        assert!(
            matches!(refusal(&e).1, Refusal::Invalid { .. })
                && e.to_string().contains("`partner_totals`"),
            "{what}: {e:?}"
        );
    };
    let opts = WriteOpts::default;
    names_it(
        "upsert",
        c.upsert(owner, &pt, &p1, fields(), opts())
            .await
            .unwrap_err(),
    );
    names_it(
        "retire",
        c.retire(owner, &pt, &p1, opts()).await.unwrap_err(),
    );
    let r = "r".into();
    names_it(
        "link",
        c.link(owner, &pt, &p1, &r, &p1, fields(), opts())
            .await
            .unwrap_err(),
    );
    names_it(
        "unlink",
        c.unlink(owner, &pt, &p1, &r, &p1, opts())
            .await
            .unwrap_err(),
    );
    let verb = |scope: &str| {
        serde_json::from_value::<Declaration>(
            json!({"declare": "verb", "name": format!("on_{scope}"),
            "on": "partner_totals", "scope": scope, "risk": "low", "effect": "external",
            "executor": "x"}),
        )
        .unwrap()
    };
    let e = c.declare(owner, verb("record")).await.unwrap_err();
    assert!(
        matches!(refusal(&e).1, Refusal::Invalid { .. }) && e.to_string().contains("`on_record`"),
        "a verb on a record of a view: {e:?}"
    );
    assert_eq!(f.head().await, head, "nothing is appended");
    c.declare(owner, verb("type"))
        .await
        .expect("a verb on the view's type that reaches outside is taken");
}

/// `partner_totals`, a view counting each partner's invoices.
fn partner_totals_view() -> TypeDecl {
    TypeDecl {
        annotations: Default::default(),
        name: "partner_totals".into(),
        key: vec!["partner".into()],
        fields: vec![text("partner"), text("count")],
        relations: Vec::new(),
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            r#where: None,
            group_by: vec!["partner".into()],
            measures: vec![Measure {
                name: "count".into(),
                expr: "count()".into(),
            }],
            params: Default::default(),
            merge: Default::default(),
        })),
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    }
}

/// A view is never written: a trigger on one would never fire, and an
/// object set holds stored records, so `declare` refuses both, naming
/// the declaration and the view, on the mock as on the store.
pub async fn nothing_that_never_acts_is_declared_on_a_view(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let totals = partner_totals_view();
    f.catalog
        .declare(owner, Declaration::Type(totals))
        .await
        .expect("the view is declared");
    let head = f.head().await;
    let trigger = Declaration::Trigger(TriggerDecl {
        max_depth: None,
        name: "grew".into(),
        on: "partner_totals".into(),
        when: None,
        description: None,
    });
    let set = Declaration::ObjectSet(ObjectSetDecl {
        name: "busy".into(),
        of: "partner_totals".into(),
        filter: "count > 0".into(),
        description: None,
    });
    for (d, name, field) in [(trigger, "grew", "on"), (set, "busy", "of")] {
        let e = f
            .catalog
            .declare(owner, d)
            .await
            .expect_err("nothing that never acts is declared on a view");
        assert!(
            matches!(&e, Error::Refused { field: Some(at), reason: Refusal::Invalid { message } }
                if at.as_str() == field && message.contains(name) && message.contains("partner_totals")),
            "{e:?}"
        );
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// An object set may hold the records of a relation's link type, as a
/// trigger may watch them: `declare` takes a set over
/// `sys.link.<type>.<relation>` and `describe` shows it, on the mock as on
/// the store.
pub async fn a_set_may_hold_a_link_types_records(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    for d in [
        json!({"declare": "type", "name": "memo", "field_meta": true,
               "fields": [{"name": "n", "type": {"kind": "text"}}],
               "relations": [{"name": "seen_by", "target": PARTNER,
                              "fields": [{"name": "ok", "type": {"kind": "text"}}]}]}),
        json!({"declare": "object_set", "name": "seen_ok", "of": "sys.link.memo.seen_by",
               "where": "ok = 'x'"}),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        let what = d.what();
        let taken = f.catalog.declare(owner, d).await;
        assert!(taken.is_ok(), "{what} is taken: {taken:?}");
    }
    let d = f.catalog.describe(owner, ReadAt::default()).await;
    let sets = d.expect("the owner describes the lake").data.object_sets;
    assert!(
        sets.iter().any(|s| s.name.as_str() == "seen_ok"),
        "{sets:?}"
    );
}

/// A type made of another is shown only with it (006, extended): one kept
/// from `memo` is told of neither the link type of its relation nor a view
/// over that or over `memo`, whatever it is granted on their own names;
/// `describe_type`, `find`, `link` and `unlink` answer it as for a type
/// that is not there, and a step from `partner` by the inverse of a
/// relation of `memo` as a relation that is not there. One who reads a
/// field of `memo` is shown all three, links and walks. One kept from
/// `partner` is not shown `seen_by` nor its link type: it is not
/// described, offered to `link`, linked or walked (lakelands#662). On the
/// mock as on the store.
pub async fn a_type_made_of_another_is_shown_only_with_it(store: Arc<dyn LakeStore>) {
    const LINKS: &str = "sys.link.memo.cites";
    let f = setup(store).await;
    let (owner, c) = (&f.cast.owner, &f.catalog);
    let n = json!({"name": "n", "type": {"kind": "number"}});
    for d in [
        json!({"declare": "type", "name": "memo",
               "fields": [{"name": "t", "type": {"kind": "text"}}],
               "relations": [{"name": "cites", "target": "memo"},
                             {"name": "seen_by", "target": PARTNER, "inverse": "saw"}]}),
        json!({"declare": "type", "name": "cited", "key": ["to"],
               "fields": [{"name": "to", "type": {"kind": "reference", "target": "memo"}}, n],
               "view": {"source": LINKS, "group_by": ["to"],
                        "measures": [{"name": "n", "expr": "count()"}]}}),
        json!({"declare": "type", "name": "memos", "fields": [n],
               "view": {"source": "memo", "measures": [{"name": "n", "expr": "count()"}]}}),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        c.declare(owner, d).await.expect("the owner declares it");
    }
    let (memo, cites) = (ty("memo"), "cites".into());
    for k in ["a", "b"] {
        let (k, t) = (key(k), patch(&[("t", json!(k))]));
        let wrote = c.upsert(owner, &memo, &k, t, WriteOpts::default());
        wrote.await.expect("writing a memo");
    }
    let made = [LINKS, "cited", "memos"];
    let grants = |of_memo: &[(&str, Access)]| {
        let whole = |a| TypeGrants {
            default: Some(a),
            ..TypeGrants::default()
        };
        let mut g = invoice_grants(Access::None, &[]);
        g.types = (made.iter().map(|t| (ty(t), whole(Access::Write)))).collect();
        g.types.insert(ty(PARTNER), whole(Access::Read));
        let fields = of_memo.iter().map(|(f, a)| (FieldName::from(*f), *a));
        let fields = fields.collect();
        (g.types).insert(
            memo.clone(),
            TypeGrants {
                default: None,
                fields,
                ..Default::default()
            },
        );
        g
    };
    let lake = f.store.lake();
    let apart = principal(lake, PrincipalKind::Agent, "apart", grants(&[]));
    let reads = principal(
        lake,
        PrincipalKind::Agent,
        "reads",
        grants(&[("t", Access::Read)]),
    );
    let gone = |e: &Error| {
        matches!(
            e,
            Error::NotFound {
                missing: Missing::Type { .. }
            }
        )
    };
    let link = |who: &Principal, by: &str| {
        let (c, who, memo, by) = (c.clone(), who.clone(), memo.clone(), by.into());
        async move {
            let (a, b, opts) = (key("a"), key("b"), WriteOpts::default());
            c.link(&who, &memo, &a, &by, &b, Patch::new(), opts).await
        }
    };
    let described = c.describe(&apart, ReadAt::default()).await;
    let described = described.expect("describing the lake").data;
    let named = |t: &&str| described.types.iter().any(|d| d.name.as_str() == *t);
    assert!(
        !made.iter().any(named),
        "one kept from `memo` is described {described:?}"
    );
    for t in made {
        let said = c.describe_type(&apart, &ty(t), ReadAt::default()).await;
        assert!(
            said.as_ref().is_err_and(gone),
            "describe_type {t}: {said:?}"
        );
        let found = c.find(&apart, &ty(t), Find::default()).await;
        assert!(found.as_ref().is_err_and(gone), "find {t}: {found:?}");
        let shown = c.describe_type(&reads, &ty(t), ReadAt::default()).await;
        assert!(
            shown.is_ok(),
            "one who reads a field of `memo` is shown {t}: {shown:?}"
        );
    }
    for by in ["cites", "nothing"] {
        let linked = link(&apart, by).await;
        assert!(
            linked.as_ref().is_err_and(gone),
            "link over `{by}`: {linked:?}"
        );
    }
    let (a, b) = (key("a"), key("b"));
    let unlinked = c
        .unlink(&apart, &memo, &a, &cites, &b, WriteOpts::default())
        .await;
    assert!(unlinked.as_ref().is_err_and(gone), "unlink: {unlinked:?}");
    link(&reads, "cites").await.expect("one shown `memo` links");
    let walk = |who: &Principal, by: &str| {
        let step = Step {
            relation: by.into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Default::default(),
        };
        let (c, who) = (c.clone(), who.clone());
        async move {
            let p = Pattern::traverse(ty(PARTNER), key("P-1"), vec![step]);
            c.query(&who, p).await.map_err(|e| e.to_string())
        }
    };
    let (saw, ghost) = (walk(&apart, "saw").await, walk(&apart, "ghost").await);
    let saw = saw.expect_err("the inverse of a relation of `memo`");
    assert_eq!(
        Err(saw.replace("saw", "ghost")),
        ghost.map(|_| ()),
        "`saw` answers one kept from `memo` as `ghost` does"
    );
    let walked = walk(&reads, "saw").await;
    assert!(walked.is_ok(), "one shown `memo` walks `saw`: {walked:?}");
    let invalid = link(&reads, "nothing").await;
    assert!(
        matches!(
            &invalid,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "a relation `memo` has not, to one shown `memo`: {invalid:?}"
    );
    // Kept from `partner`, the target of `seen_by`; writes its links.
    let mut kept = grants(&[("t", Access::Read)]);
    let none = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    let write = TypeGrants {
        default: Some(Access::Write),
        ..TypeGrants::default()
    };
    kept.types.insert(ty(PARTNER), none);
    kept.types.insert(ty("sys.link.memo.seen_by"), write);
    let far = principal(lake, PrincipalKind::Agent, "far", kept);
    let d = c.describe_type(&far, &memo, ReadAt::default()).await;
    let d = d.expect("one who reads `t` is described `memo`").data;
    let seen_links = ty("sys.link.memo.seen_by");
    let link_type = c.describe_type(&far, &seen_links, ReadAt::default()).await;
    assert!(
        !d.relations.iter().any(|r| r.name.as_str() == "seen_by")
            && !d.may.link.iter().any(|r| r.as_str() == "seen_by")
            && link_type.as_ref().is_err_and(gone),
        "one kept from `partner` is not shown `seen_by`: {:?}, link {:?}, {link_type:?}",
        d.relations,
        d.may.link
    );
    let as_nothing = |e: Error, by: &str| e.to_string().replace(by, "nothing");
    let nothing = link(&far, "nothing")
        .await
        .expect_err("no relation `nothing`");
    let seen = link(&far, "seen_by")
        .await
        .expect_err("`seen_by` is not shown");
    assert_eq!(as_nothing(seen, "seen_by"), nothing.to_string());
    let from_memo = |by: &str| {
        let step = Step {
            relation: by.into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Default::default(),
        };
        Pattern::traverse(memo.clone(), key("a"), vec![step])
    };
    let seen = c.query(&far, from_memo("seen_by")).await.map(drop);
    let ghost = c.query(&far, from_memo("ghost")).await.map(drop);
    assert_eq!(
        seen.map_err(|e| e.to_string().replace("seen_by", "ghost")),
        ghost.map_err(|e| e.to_string()),
        "`seen_by` answers one kept from `partner` as `ghost` does"
    );
    // A migration's refusal names no type by a relation its reader is not
    // shown: `bill.paid` matches `pay.note` from `bill.note`, hidden.
    let t = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let paid = json!({"name": "paid", "target": "pay",
                      "derived": {"match": [{"from": "note", "to": "note"}]}});
    for d in [
        json!({"declare": "type", "name": "pay", "fields": [t("note"), t("code")]}),
        json!({"declare": "type", "name": "bill", "fields": [t("note"), t("sum")],
               "relations": [paid]}),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        c.declare(owner, d).await.expect("the owner declares it");
    }
    let mut g = invoice_grants(Access::Read, &[]);
    g.schema = true;
    let blind = TypeGrants {
        fields: [(FieldName::from("note"), Access::None)].into(),
        ..TypeGrants::default()
    };
    g.types.insert(ty("bill"), blind);
    let blind = principal(lake, PrincipalKind::Agent, "blind", g);
    let to: TypeDecl =
        serde_json::from_value(json!({"name": "pay", "fields": [t("code")]})).expect("a type");
    let m = Migration {
        to,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let said = format!("{:?}", c.migrate(&blind, m).await.map(drop));
    assert!(
        said.contains("a declaration reads `note`") && !said.contains("bill"),
        "dropping `pay.note`, `blind` is told no type: {said}"
    );
}

/// A read of a view probes what computing it reads: each source's fields,
/// a merged source's as its own, and those of a source that is itself a
/// view. A reader who may not know of a field one of them reads is refused
/// as not granted, on no field and without that field's name, by `find`
/// and by `get`, and `describe` withholds the definition where the view
/// itself reads that field of its own or a merged source; a holder of the
/// `schema` grant is refused on that field; the owner is answered. On the
/// mock as on the store (024, extended).
pub async fn a_view_over_a_hidden_field_names_nothing(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (owner, c) = (&f.cast.owner, &f.catalog);
    let n = |n: &str| json!([{"name": n, "expr": "count()"}]);
    let fields = |n: &str| {
        json!([{"name": "status", "type": {"kind": "text"}},
        {"name": n, "type": {"kind": "number"}}])
    };
    let declared = [
        json!({"declare": "type", "name": "credit", "key": ["status"],
            "fields": [{"name": "status", "type": {"kind": "text"}},
                       {"name": "hush", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "kept_by_status", "key": ["status"],
            "fields": fields("n"),
            "view": {"source": INVOICE, "group_by": ["status"], "where": "secret = 'x'",
                     "measures": n("n")}}),
        json!({"declare": "type", "name": "merged", "key": ["status"],
            "fields": [{"name": "status", "type": {"kind": "text"}},
                       {"name": "n", "type": {"kind": "number"}},
                       {"name": "m", "type": {"kind": "number"}}],
            "view": {"source": INVOICE, "group_by": ["status"], "measures": n("n"),
                     "merge": [{"source": "credit", "group_by": ["status"],
                                "where": "hush = 'x'", "measures": n("m")}]}}),
        json!({"declare": "type", "name": "over_kept", "key": ["status"],
            "fields": fields("k"),
            "view": {"source": "kept_by_status", "group_by": ["status"], "measures": n("k")}}),
    ];
    for d in declared {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        c.declare(owner, d).await.expect("the owner declares it");
    }
    let unnamed = |what: &str, hidden: &str, e: Error| {
        let nothing = matches!(
            &e,
            Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            }
        );
        assert!(nothing && !e.to_string().contains(hidden), "{what}: {e:?}");
    };
    let get = || Get {
        key: key("open"),
        with_meta: false,
        params: Default::default(),
        at: Default::default(),
    };
    let (clerk, architect) = (&f.cast.clerk, &f.cast.architect);
    for (view, hidden) in [
        ("kept_by_status", "secret"),
        ("merged", "hush"),
        ("over_kept", "secret"),
    ] {
        let v = ty(view);
        let found = c.find(clerk, &v, Find::default()).await;
        unnamed(
            view,
            hidden,
            found.expect_err("the clerk may not know of it"),
        );
        let got = c.get(clerk, &v, get()).await;
        unnamed(view, hidden, got.expect_err("the clerk may not know of it"));
        let found = c.find(architect, &v, Find::default()).await;
        let e = found.expect_err("the architect may not read it");
        assert!(no_field(&e, hidden), "{view}: {e:?}");
        let got = c.get(architect, &v, get()).await;
        let e = got.expect_err("the architect may not read it");
        assert!(no_field(&e, hidden), "{view}: {e:?}");
        c.find(owner, &v, Find::default())
            .await
            .expect("the owner reads the view");
    }
    for view in ["kept_by_status", "merged"] {
        let d = c.describe_type(clerk, &ty(view), ReadAt::default()).await;
        let d = d.expect("the clerk is described the view").data;
        assert!(d.view.is_none(), "{view}: its definition is withheld");
    }
}

/// A view's records are computed, so a relation on a view carried by link
/// records or by a `via` field could never hold an edge: `declare`
/// refuses it, naming the view and the relation, on the mock as on the
/// store.
pub async fn a_relation_on_a_view_is_derived(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let head = f.head().await;
    for (relation, name) in [
        (RelationDecl::to("seen", INVOICE), "seen"),
        (
            RelationDecl {
                via: Some("partner".into()),
                ..RelationDecl::to("by", INVOICE)
            },
            "by",
        ),
    ] {
        let mut view = partner_totals_view();
        view.relations = vec![relation];
        let e = f
            .catalog
            .declare(&f.cast.owner, Declaration::Type(view))
            .await
            .expect_err("a relation on a view that is not derived");
        assert!(
            matches!(&e, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains(name) && message.contains("partner_totals")),
            "{e:?}"
        );
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// A migration reshapes stored records, and a view stores none: turning
/// a stored type into a view, or a view into a stored type, is refused at
/// the preflight, naming the type, on the mock as on the store.
pub async fn a_view_is_never_migrated(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let stored = |name: &str| TypeDecl {
        name: name.into(),
        key: vec!["partner".into()],
        fields: vec![text("partner"), text("count")],
        field_meta: true,
        ..TypeDecl::named(name)
    };
    let view = |name: &str| TypeDecl {
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            r#where: None,
            group_by: vec!["partner".into()],
            measures: vec![Measure {
                name: "count".into(),
                expr: "count()".into(),
            }],
            params: Default::default(),
            merge: Default::default(),
        })),
        ..stored(name)
    };
    for d in [stored("tally"), view("partner_totals")] {
        f.catalog
            .declare(owner, Declaration::Type(d))
            .await
            .expect("declared");
    }
    for (name, to) in [
        ("tally", view("tally")),
        ("partner_totals", stored("partner_totals")),
    ] {
        let m = lakelands_api::write::Migration {
            to,
            renames: vec![],
            relation_renames: Vec::new(),
        };
        let e = f
            .catalog
            .migrate(owner, m)
            .await
            .expect_err("a view is never migrated");
        assert!(
            matches!(&e, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains(name)),
            "{e:?}"
        );
    }
    // A view changes by being declared again.
    let mut narrowed = view("partner_totals");
    narrowed.view.as_mut().expect("a view").r#where = Some("status = 'open'".into());
    f.catalog
        .declare(owner, Declaration::Type(narrowed))
        .await
        .expect("a view changes by being declared again");
    // And becomes a stored type by being declared as one, with a required
    // field it never computed: it stored nothing, so nothing is reshaped.
    let mut stored = stored("partner_totals");
    stored.fields.push(FieldDecl {
        required: true,
        ..text("owner")
    });
    f.catalog
        .declare(owner, Declaration::Type(stored))
        .await
        .expect("a view becomes a stored type by being declared as one");
}

/// A view reads its source's shape after a migration as before: a
/// migration that drops or renames a field the view groups by or
/// aggregates is refused at the preflight, naming the view and the field;
/// an author who is not shown the view's definition is told that a
/// declaration reads the field, without the view. On the mock as on the
/// store.
pub async fn a_migration_keeps_what_a_view_reads(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let by_status = TypeDecl {
        name: "status_totals".into(),
        key: vec!["status".into()],
        fields: vec![
            text("status"),
            FieldDecl {
                ty: FieldType::kind(StorageKind::Decimal),
                ..text("total")
            },
        ],
        field_meta: true,
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            r#where: None,
            group_by: vec!["status".into()],
            measures: vec![Measure {
                name: "total".into(),
                expr: "sum(amount)".into(),
            }],
            params: Default::default(),
            merge: Default::default(),
        })),
        ..TypeDecl::named("status_totals")
    };
    f.catalog
        .declare(owner, Declaration::Type(by_status))
        .await
        .expect("the view is declared");
    let invoice = fixture()
        .into_iter()
        .find_map(|d| match d {
            Declaration::Type(t) if t.name.as_str() == INVOICE => Some(t),
            _ => None,
        })
        .expect("the fixture declares `invoice`");
    let mut dropped = invoice.clone();
    dropped.fields.retain(|x| x.name.as_str() != "amount");
    // The clerk's view of that shape: it may not know of `secret`.
    let mut without_amount = dropped.clone();
    without_amount
        .fields
        .retain(|x| x.name.as_str() != "secret");
    let mut renamed = invoice;
    for x in &mut renamed.fields {
        if x.name.as_str() == "status" {
            x.name = "state".into();
        }
    }
    for (to, renames, read) in [
        (dropped, vec![], "amount"),
        (renamed, vec![("status".into(), "state".into())], "status"),
    ] {
        let e = f
            .catalog
            .migrate(
                owner,
                lakelands_api::write::Migration {
                    to,
                    renames,
                    relation_renames: Vec::new(),
                },
            )
            .await
            .expect_err("a field a view reads stays");
        assert!(
            matches!(&e, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains("status_totals") && message.contains(read)),
            "{e:?}"
        );
    }
    // A view its author is not shown is not named to it: `kept_totals` reads
    // `secret`, so `describe` withholds its definition from the clerk, who
    // is told that a declaration reads the field it took away.
    let kept = json!({"declare": "type", "name": "kept_totals", "key": ["status"],
        "fields": [{"name": "status", "type": {"kind": "text"}},
                   {"name": "total", "type": {"kind": "decimal"}}],
        "view": {"source": INVOICE, "group_by": ["status"], "where": "secret = 'x'",
                 "measures": [{"name": "total", "expr": "sum(amount)"}]}});
    let kept: Declaration = serde_json::from_value(kept).expect("a declaration");
    f.catalog
        .declare(owner, kept)
        .await
        .expect("the second view");
    let proposed = ProposedWrite::Migrate {
        migration: Box::new(lakelands_api::write::Migration {
            to: without_amount,
            renames: vec![],
            relation_renames: Vec::new(),
        }),
        report: Default::default(),
    };
    let e = (f.catalog)
        .propose(&f.cast.clerk, proposed, "no amounts".into())
        .await;
    let e = e.expect_err("a field a view reads stays");
    let (said, names) = (e.to_string(), ["a declaration reads `amount`"]);
    assert!(invalid_naming(&e, &names), "must name {names:?}: {e:?}");
    assert!(!said.contains("kept_totals"), "{said}");
}

/// A view whose `where` names, or whose measure aggregates, a field its
/// source does not have is refused, naming the view and the field, on the
/// mock as on the store.
pub async fn a_view_reads_only_what_its_source_has(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let head = f.head().await;
    for (filter, total) in [(Some("nope > 0"), "sum(amount)"), (None, "sum(nope)")] {
        let view = TypeDecl {
            name: "status_totals".into(),
            key: vec!["status".into()],
            fields: vec![
                text("status"),
                FieldDecl {
                    ty: FieldType::kind(StorageKind::Decimal),
                    ..text("total")
                },
            ],
            field_meta: true,
            view: Some(Box::new(ViewDecl {
                source: INVOICE.into(),
                r#where: filter.map(Into::into),
                group_by: vec!["status".into()],
                measures: vec![Measure {
                    name: "total".into(),
                    expr: total.into(),
                }],
                params: Default::default(),
                merge: Default::default(),
            })),
            ..TypeDecl::named("status_totals")
        };
        let e = f
            .catalog
            .declare(&f.cast.owner, Declaration::Type(view))
            .await
            .expect_err("a view reads only what its source has");
        assert!(
            matches!(&e, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains("status_totals") && message.contains("nope")),
            "{e:?}"
        );
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// A view is declared only where the store would compute it: its sources
/// declared, read once, never itself through other views and never a
/// view with a parameter; its grouping on fields of its sources, of one
/// kind; each merged measure an aggregate, every measure named once; a
/// declared field for each value it computes and none that nothing
/// fills; each parameter declared once and read. Each fault is refused
/// as invalid in the store's words, on the field where it names one, and
/// nothing is appended; on the mock as on the store (018, 028).
pub async fn a_view_is_declared_only_as_the_store_declares_it(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).expect("a declaration");
    let count = |n: &str| json!([{"name": n, "expr": "count()"}]);
    for d in [
        json!({"declare": "type", "name": "credit", "key": ["status"], "fields": [
            {"name": "status", "type": {"kind": "text"}}, {"name": "x", "type": {"kind": "number"}}]}),
        json!({"declare": "type", "name": "pview", "key": ["status"], "fields": [
            {"name": "status", "type": {"kind": "text"}}, {"name": "k", "type": {"kind": "number"}}],
            "view": {"source": "credit", "group_by": ["status"], "params": [{"name": "p", "kind": "text"}],
                     "where": "status = param('p')", "measures": count("k")}}),
    ] {
        c.declare(owner, decl(d))
            .await
            .expect("the owner declares it");
    }
    // A view `v` over `source` by `group_by`, counting into `n`, its
    // fields `status`, `n` and `extra`, and `view` merged into its own.
    let view = |v: &str, source: &str, group_by: &str, extra: &[&str], view: Value| {
        let mut fields = vec![
            json!({"name": group_by, "type": {"kind": "text"}}),
            json!({"name": "n", "type": {"kind": "number"}}),
        ];
        fields.extend(
            extra
                .iter()
                .map(|e| json!({"name": e, "type": {"kind": "number"}})),
        );
        let mut d = json!({"declare": "type", "name": v, "key": [group_by], "fields": fields,
            "view": {"source": source, "group_by": [group_by], "measures": count("n")}});
        d["view"]
            .as_object_mut()
            .unwrap()
            .extend(view.as_object().unwrap().clone());
        d
    };
    let merged = |source: &str, group_by: Value, measures: Value| json!({"merge": [{"source": source, "group_by": group_by, "measures": measures}]});
    let m = || count("m");
    let status = || json!(["status"]);
    let rows = [
        (
            view(
                "v1",
                INVOICE,
                "status",
                &["m"],
                merged(INVOICE, status(), m()),
            ),
            None,
            "type `v1`, merged source `invoice`: a view reads each source once, and `invoice` is read already",
        ),
        (
            view(
                "v2",
                INVOICE,
                "status",
                &["m", "o"],
                json!({"merge": [
            {"source": "credit", "group_by": ["status"], "measures": count("m")},
            {"source": "credit", "group_by": ["status"], "measures": count("o")}]}),
            ),
            None,
            "type `v2`, merged source `credit`: a view reads each source once, and `credit` is read already",
        ),
        (
            view(
                "v3",
                INVOICE,
                "status",
                &["m"],
                merged("ghost", status(), m()),
            ),
            None,
            "type `v3`, merged source `ghost`: `ghost` is not declared",
        ),
        (
            view(
                "v4",
                INVOICE,
                "status",
                &["m"],
                merged("pview", status(), m()),
            ),
            None,
            "type `v4`: its view reads `pview`, a view with the parameter `p`; only a read that names a view binds its parameters, so no view reads one",
        ),
        (
            view(
                "v5",
                INVOICE,
                "status",
                &["m"],
                merged("credit", json!(["status", "x"]), m()),
            ),
            None,
            "type `v5`, merged source `credit`: it groups by 2 field(s), and the view by 1; each lines up with one of the view's, in order",
        ),
        (
            view(
                "v6",
                INVOICE,
                "status",
                &["m"],
                merged("credit", json!(["nope"]), m()),
            ),
            None,
            "type `v6`, merged source `credit` groups by `nope`, which `credit` does not have",
        ),
        (
            view(
                "v7",
                INVOICE,
                "status",
                &["m"],
                merged("credit", json!(["x"]), m()),
            ),
            None,
            "type `v7`, merged source `credit`: its `x` lines up with the view's `status`, of another kind, so no group would meet",
        ),
        (
            view(
                "v8",
                INVOICE,
                "status",
                &["m"],
                merged("credit", status(), json!([{"name": "m", "expr": "1"}])),
            ),
            None,
            "type `v8`, merged source `credit`, measure `m`: it calls no aggregate, so it measures nothing of `credit`",
        ),
        (
            view(
                "v9",
                INVOICE,
                "status",
                &[],
                merged("credit", status(), count("n")),
            ),
            None,
            "type `v9`: its view names the measure `n` twice; a view's measures and its merged sources' share one set of names",
        ),
        (
            view(
                "v10",
                INVOICE,
                "status",
                &["m"],
                json!({"merge": [{"source": "credit",
            "group_by": ["status"], "where": "status = ", "measures": count("m")}]}),
            ),
            None,
            "type `v10`, merged source `credit`, `where`: `status = ` does not parse: the expression ends too early (at character 9)",
        ),
        (
            view("v11", "ghost", "status", &[], json!({})),
            None,
            "type `v11`: its view reads `ghost`, which is not declared",
        ),
        (
            view("v12", "pview", "status", &[], json!({})),
            None,
            "type `v12`: its view reads `pview`, a view with the parameter `p`; only a read that names a view binds its parameters, so no view reads one",
        ),
        (
            view("v13", INVOICE, "nope", &[], json!({})),
            None,
            "type `v13`: its view groups by `nope`, which `invoice` does not have",
        ),
        (
            view("v14", "v14", "status", &[], json!({})),
            None,
            "type `v14`: its view closes a cycle, `v14` reads `v14`; a view may not read itself, through other views or directly",
        ),
        (
            view("v15", INVOICE, "status", &["extra"], json!({})),
            Some("extra"),
            "type `v15`: its field `extra` is neither a `group_by` field nor a measure of its view, so nothing would fill it",
        ),
        (
            view(
                "v16",
                INVOICE,
                "status",
                &[],
                json!({"measures": count("total")}),
            ),
            Some("total"),
            "type `v16`: its view's measure `total` has no declared field to land in; a view declares a field for each value it computes, of the kind it holds",
        ),
        (
            view(
                "v17",
                INVOICE,
                "status",
                &[],
                json!({"params": [{"name": "p", "kind": "text"}]}),
            ),
            None,
            "type `v17`: its view declares the parameter `p`, which neither its `where` nor its measures read, so a reader would bind a value that changes nothing",
        ),
        (
            view(
                "v18",
                INVOICE,
                "status",
                &[],
                json!({"where": "status = param('p')",
            "params": [{"name": "p", "kind": "text"}, {"name": "p", "kind": "text"}]}),
            ),
            None,
            "type `v18`: its view declares the parameter `p` twice",
        ),
    ];
    let head = f.head().await;
    for (d, field, want) in rows {
        let e = c.declare(owner, decl(d)).await.expect_err(want);
        assert!(
            matches!(&e, Error::Refused { field: at, reason: Refusal::Invalid { message } }
                if message == want && at.as_ref().map(FieldName::as_str) == field),
            "want {want:?} on {field:?}, got {e:?}"
        );
    }
    assert_eq!(f.head().await, head, "nothing is appended");
    let valid = view(
        "v19",
        INVOICE,
        "status",
        &["m"],
        merged("credit", status(), m()),
    );
    c.declare(owner, decl(valid))
        .await
        .expect("a view merging another source is declared");
}

/// A declaration is taken only with names and expressions the store
/// takes: every name an ASCII identifier, a type's identifiers joined by
/// `.`, an aging rule's too; every expression parsed, its functions given
/// what they take where they have meaning, its numbers held exactly (the
/// largest numeral that fits in 96 bits is taken, the next is not); and
/// `in_set()` only in a set's `where`, a trigger's `when` and a read's
/// `where`. `add_field` declares the type with the field added. Each
/// fault is refused as invalid in the store's words, and nothing is
/// appended; on the mock as on the store (028).
pub async fn names_and_expressions_are_declared_only_as_the_store_declares_them(
    store: Arc<dyn LakeStore>,
) {
    const IDENT: &str = "is not an ASCII identifier: a letter or `_`, then letters, digits or `_`";
    const AGG: &str = "count() folds a set of records and has no meaning here";
    const RETIRED: &str = "retired() has meaning only in a trigger's guard";
    const CHANGED: &str = "changed() has meaning only in a guard over a write";
    const PARAM: &str = "param() is read only in a view's `where` and `measures`, where the \
                         view declares its parameters";
    const IN_SET: &str = "in_set() has meaning in an object set's `where`, a trigger's `when` \
                          and a read's `where`";
    const EARLY: &str = "does not parse: the expression ends too early";
    const INEXACT: &str = "is not a number this evaluator can hold exactly";
    const MAX: &str = "79228162514264337593543950335";
    const PAST: &str = "79228162514264337593543950336";
    const BIG: &str = "123456789012345678901234567890123";
    let f = setup(store).await;
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).expect("a declaration");
    let open =
        json!({"declare": "object_set", "name": "open", "of": INVOICE, "where": "status = 'open'"});
    c.declare(owner, decl(open))
        .await
        .expect("the owner declares it");
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let derived = |e: &str| json!({"name": "d", "type": {"kind": "text"}, "authority": "derived", "derived": e});
    let ty = |n: &str, more: Value| json!({"declare": "type", "name": n, "key": ["k"], "fields": [text("k"), more]});
    let field = |n: &str, e: &str| (ty(n, derived(e)), format!("type `{n}`, field `d`"));
    let view = |n: &str, m: &str, more: Value| {
        let mut v = json!({"source": INVOICE, "group_by": ["status"], "measures": [{"name": m, "expr": "count()"}]});
        v.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        json!({"declare": "type", "name": n, "key": ["status"], "view": v,
               "fields": [text("status"), {"name": "n", "type": {"kind": "number"}}]})
    };
    let measure = |n: &str, e: &str| {
        (
            view(n, "n", json!({"measures": [{"name": "n", "expr": e}]})),
            format!("type `{n}`, measure `n`"),
        )
    };
    let filtered = |n: &str, w: &str| {
        (
            view(n, "n", json!({"where": w})),
            format!("type `{n}`, view `where`"),
        )
    };
    let vtype = |n: &str, e: &str| {
        (
            json!({"declare": "value_type", "name": n, "kind": "text", "constraint": e}),
            format!("value type `{n}`, constraint"),
        )
    };
    let verb = |n: &str, requires: Option<&str>, value: &str| {
        let mut v = json!({"declare": "verb", "name": n, "on": INVOICE, "risk": "low", "effect": "write", "set": [{"field": "status", "value": value}]});
        if let Some(r) = requires {
            v["requires"] = json!(r);
        }
        v
    };
    let requires = |n: &str, e: &str| (verb(n, Some(e), "'x'"), format!("verb `{n}`, requires"));
    let value = |n: &str, e: &str| (verb(n, None, e), format!("verb `{n}`, `status`"));
    let trigger = |n: &str, e: &str| {
        (
            json!({"declare": "trigger", "name": n, "on": INVOICE, "when": e}),
            format!("trigger `{n}`"),
        )
    };
    let set = |n: &str, e: &str| {
        (
            json!({"declare": "object_set", "name": n, "of": INVOICE, "where": e}),
            format!("object set `{n}`"),
        )
    };
    let function = |n: &str, backing: &str, e: &str| json!({"declare": "function", "name": n, "backing": backing, "inputs": [text("t")], "expr": e});
    let body = |n: &str, e: &str| (function(n, "expr", e), format!("function `{n}`"));
    let with_relation = |r: &str| {
        let mut t = ty("nr", text("x"));
        t["relations"] = json!([{"name": r, "target": PARTNER}]);
        t
    };
    let names = [
        (
            ty("bad-type", text("x")),
            "type name `bad-type` is not ASCII identifiers joined by `.`".to_owned(),
        ),
        (
            ty("a..b", text("x")),
            "type name `a..b` is not ASCII identifiers joined by `.`".to_owned(),
        ),
        (
            ty("nf", text("bad-field")),
            format!("field name `bad-field` {IDENT}"),
        ),
        (
            with_relation("bad-rel"),
            format!("relation name `bad-rel` {IDENT}"),
        ),
        (
            verb("bad-verb", None, "'x'"),
            format!("verb name `bad-verb` {IDENT}"),
        ),
        (
            json!({"declare": "verb", "name": "ext", "on": INVOICE, "risk": "low", "effect": "external", "executor": "bad exec"}),
            format!("executor name `bad exec` {IDENT}"),
        ),
        (
            trigger("bad-trigger", "true").0,
            format!("trigger name `bad-trigger` {IDENT}"),
        ),
        (
            set("bad-set", "true").0,
            format!("object set name `bad-set` {IDENT}"),
        ),
        (
            vtype("bad-vt", "true").0,
            format!("value type name `bad-vt` {IDENT}"),
        ),
        (
            function("bad-fn", "expr", "upper(t)"),
            format!("function name `bad-fn` {IDENT}"),
        ),
        (
            function("fine", "bad-backing", "upper(t)"),
            format!("backing name `bad-backing` {IDENT}"),
        ),
        (
            view("nm", "bad-m", json!({})),
            format!("measure name `bad-m` {IDENT}"),
        ),
        (
            view(
                "np",
                "n",
                json!({"where": "status = param('bad-p')", "params": [{"name": "bad-p", "kind": "text"}]}),
            ),
            format!("parameter name `bad-p` {IDENT}"),
        ),
    ];
    let faults = [
        (field("e1", "1 +"), format!("`1 +` {EARLY} (at character 3)")),
        (vtype("e2", "value ="), format!("`value =` {EARLY} (at character 7)")),
        (requires("e3", "(status"), "`(status` does not parse: expected `)` (at character 7)".to_owned()),
        (value("e4", "upper("), format!("`upper(` {EARLY} (at character 6)")),
        (trigger("e5", "status =="), "`status ==` does not parse: unexpected `=` (at character 8)".to_owned()),
        (set("e6", "status = "), format!("`status = ` {EARLY} (at character 9)")),
        (body("e7", "upper("), format!("`upper(` {EARLY} (at character 6)")),
        (set("e8", "status in ()"), "`status in ()` does not parse: `in` needs at least one value (at character 12)".to_owned()),
        (set("e9", "amount > 1.5e3"), "`amount > 1.5e3` does not parse: a number runs into a name (at character 12)".to_owned()),
        (field("p1", "count()"), AGG.to_owned()),
        (field("p2", "retired()"), RETIRED.to_owned()),
        (field("p3", "param('p')"), PARAM.to_owned()),
        (field("p4", "upper()"), "`upper()` does not parse: `upper` takes 1 argument(s), not 0 (at character 0)".to_owned()),
        (field("p5", "setting(k)"), "`setting(k)` does not parse: `setting` takes a quoted key, as in setting('vat_rate') (at character 0)".to_owned()),
        (field("p6", "date_bucket(k, 'decade')"), "date_bucket() knows no unit 'decade'; the units are 'day', 'week', 'month', 'quarter' and 'year', and a width is written unquoted, as in date_bucket(t, 30d, origin)".to_owned()),
        (field("p7", "changed('k')"), "`changed('k')` does not parse: `changed` takes a field name (at character 0)".to_owned()),
        (filtered("p8", "count() > 1"), AGG.to_owned()),
        (measure("p9", "sum(retired())"), RETIRED.to_owned()),
        (measure("p10", "count() + len(changed(status))"), CHANGED.to_owned()),
        (vtype("p11", "count() > 0"), AGG.to_owned()),
        (vtype("p12", "changed(value)"), CHANGED.to_owned()),
        (requires("p13", "changed(status)"), CHANGED.to_owned()),
        (requires("p14", "count() > 0"), AGG.to_owned()),
        (value("p15", "count()"), AGG.to_owned()),
        (value("p16", "param('p')"), PARAM.to_owned()),
        (trigger("p17", "count() > 1"), AGG.to_owned()),
        (trigger("p18", "param('p') = 'x'"), PARAM.to_owned()),
        (set("p19", "changed(status)"), CHANGED.to_owned()),
        (set("p20", "count() > 1"), AGG.to_owned()),
        (set("p21", "retired()"), RETIRED.to_owned()),
        (body("p22", "count()"), AGG.to_owned()),
        (body("p23", "changed(t)"), CHANGED.to_owned()),
        (field("x1", &format!("k = {BIG}")), format!("`{BIG}` {INEXACT}")),
        (set("x2", &format!("amount > {BIG}")), format!("`{BIG}` {INEXACT}")),
        (set("x3", &format!("amount > {PAST}")), format!("`{PAST}` {INEXACT}")),
        (set("x4", &format!("amount > -{PAST}")), format!("`{PAST}` {INEXACT}")),
        (field("i1", "in_set('open')"), IN_SET.to_owned()),
        (filtered("i2", "in_set('open')"), IN_SET.to_owned()),
        (vtype("i3", "in_set('open')"), IN_SET.to_owned()),
        (requires("i4", "in_set('open')"), IN_SET.to_owned()),
        (value("i5", "in_set('open')"), IN_SET.to_owned()),
        (body("i6", "in_set('open')"), IN_SET.to_owned()),
        (set("i7", "in_set(open)"), "in_set() takes a quoted name".to_owned()),
    ];
    let refused = |e: Result<Outcome, Error>, want: &str, at: Option<&str>| {
        assert!(
            matches!(&e, Err(Error::Refused { field, reason: Refusal::Invalid { message } })
                if message == want && field.as_ref().map(FieldName::as_str) == at),
            "want {want:?} on {at:?}, got {e:?}"
        );
    };
    let head = f.head().await;
    for (d, want) in names {
        refused(c.declare(owner, decl(d)).await, &want, None);
    }
    for ((d, at), why) in faults {
        refused(
            c.declare(owner, decl(d)).await,
            &format!("{at}: {why}"),
            None,
        );
    }
    let invoice = TypeName::from(INVOICE);
    let added = |name: &str, e: Option<&str>| {
        let mut fd = text(name);
        if let Some(e) = e {
            fd = json!({"name": name, "type": {"kind": "text"}, "authority": "derived", "derived": e});
        }
        c.add_field(
            owner,
            &invoice,
            serde_json::from_value(fd).expect("a field"),
        )
    };
    let on_invoice = |n: &str, why: &str| format!("type `invoice`, field `{n}`: {why}");
    refused(
        added("bad-f", None).await,
        &format!("field name `bad-f` {IDENT}"),
        None,
    );
    refused(
        added("a1", Some("count()")).await,
        &on_invoice("a1", AGG),
        None,
    );
    refused(
        added("a2", Some("1 +")).await,
        &on_invoice("a2", &format!("`1 +` {EARLY} (at character 3)")),
        None,
    );
    refused(
        added("a3", Some("in_set('open')")).await,
        &on_invoice("a3", IN_SET),
        None,
    );
    let rule = patch(&[
        ("name", json!("bad-rule")),
        ("on", json!(INVOICE)),
        ("field", json!("status")),
        ("from", json!("open")),
        ("to", json!("late")),
        ("when", json!("true")),
    ]);
    let rules = TypeName::from(lakelands_api::seam::AGING_RULE_TYPE);
    let bad = key("bad-rule");
    let written = c.upsert(owner, &rules, &bad, rule, WriteOpts::default());
    refused(
        written.await,
        &format!("aging rule name `bad-rule` {IDENT}"),
        Some("name"),
    );
    assert_eq!(f.head().await, head, "nothing is appended");
    c.declare(owner, decl(set("fits", &format!("amount > {MAX}")).0))
        .await
        .expect("the largest numeral held exactly is taken");
}

/// An invalid refusal whose text names each of `names`.
fn invalid_naming(e: &Error, names: &[&str]) -> bool {
    matches!(e, Error::Refused { reason: Refusal::Invalid { message }, .. }
        if names.iter().all(|n| message.contains(n)))
}

/// A migration that names a relation's rename keeps its edges: the report
/// lists the relation with the links that move, and the links are records
/// of the new name's link type and of the old no more; the feed hands
/// the `migrated` to one who follows either link type, and that of a
/// relation carried by `via` to no follower of a link type; a rename of a
/// relation whose link type another type refers to is refused, naming
/// that type, on the mock as on the store.
pub async fn a_renamed_relation_keeps_its_edges(store: Arc<dyn LakeStore>) {
    use lakelands_api::write::{Carrier, Migration};
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let memo = |relation: &str| -> lakelands_api::ontology::TypeDecl {
        let text = json!({"name": "t", "type": {"kind": "text"}});
        let cites = json!({"name": relation, "target": "memo", "cardinality": "many_to_many"});
        serde_json::from_value(json!({"name": "memo", "fields": [text], "relations": [cites]}))
            .expect("a type")
    };
    let declared = f.catalog.declare(owner, Declaration::Type(memo("cites")));
    declared.await.expect("declaring `memo`");
    let (ty, a, b) = (
        TypeName::from("memo"),
        RecordKey::from("a"),
        RecordKey::from("b"),
    );
    for key in [&a, &b] {
        let patch = serde_json::from_value(json!({"t": "x"})).expect("a patch");
        let wrote = f
            .catalog
            .upsert(owner, &ty, key, patch, WriteOpts::default());
        wrote.await.expect("writing a memo");
    }
    let (cites, opts) = ("cites".into(), WriteOpts::default());
    let linked = (f.catalog).link(owner, &ty, &a, &cites, &b, Default::default(), opts);
    linked.await.expect("linking over `cites`");
    let m = Migration {
        to: memo("quotes"),
        renames: Vec::new(),
        relation_renames: vec![("cites".into(), "quotes".into())],
    };
    let report = f.catalog.migrate(owner, m.clone()).await;
    let report = report.expect("the preflight").data;
    let moved: Vec<_> = (report.relations.iter().map(|r| (r.carrier, r.links))).collect();
    assert_eq!(moved, [(Carrier::Links, 1)], "{report:?}");
    let before = f.head().await;
    let applied = f.catalog.migrate_apply(owner, m, &report.report_id).await;
    applied.expect("applying the rename");
    // The feed: one who follows the old or the new link type is handed the
    // `migrated`. A relation carried by `via` has no link type, so its
    // rename is about its own type and no other.
    let shelf = |relation: &str| -> lakelands_api::ontology::TypeDecl {
        let owner = json!({"name": "owner", "type": {"kind": "reference", "target": "memo"}});
        let by = json!({"name": relation, "target": "memo", "cardinality": "many_to_one",
            "via": "owner"});
        serde_json::from_value(json!({"name": "shelf", "fields": [owner], "relations": [by]}))
            .expect("a type")
    };
    let declared = f
        .catalog
        .declare(owner, Declaration::Type(shelf("held_by")));
    declared.await.expect("declaring `shelf`");
    let via = Migration {
        to: shelf("kept_by"),
        renames: Vec::new(),
        relation_renames: vec![("held_by".into(), "kept_by".into())],
    };
    let report = f.catalog.migrate(owner, via.clone()).await;
    let id = report.expect("the preflight").data.report_id;
    let applied = f.catalog.migrate_apply(owner, via, &id).await;
    applied.expect("renaming a relation carried by `via`");
    let mut handed = Vec::new();
    for ty in [
        "sys.link.memo.cites",
        "sys.link.memo.quotes",
        "shelf",
        "sys.link.shelf.held_by",
        "sys.link.shelf.kept_by",
    ] {
        let filter = FeedFilter {
            kinds: vec!["migrated".into()],
            types: vec![ty.into()],
            keys: Vec::new(),
        };
        let batch = f.store.feed().poll(before, &filter, 10).await;
        handed.push(batch.expect("polling the feed").events.len());
    }
    assert_eq!(handed, [1, 1, 1, 0, 0], "the `migrated` events handed");
    let mut held = Vec::new();
    for lt in ["sys.link.memo.quotes", "sys.link.memo.cites"] {
        let found = f.catalog.find(owner, &lt.into(), Default::default()).await;
        held.push(match found.expect("finding the links").data {
            lakelands_api::read::Found::Records { rows, .. } => rows.len(),
            _ => usize::MAX,
        });
    }
    assert_eq!(held, [1, 0], "the links are the new link type's");
    // A reference into the link type holds a rename: its keys would end.
    let about =
        json!({"name": "about", "type": {"kind": "reference", "target": "sys.link.memo.quotes"}});
    let gloss = serde_json::from_value(json!({"name": "gloss", "fields": [about]}));
    let declared = f
        .catalog
        .declare(owner, Declaration::Type(gloss.expect("a type")));
    declared.await.expect("declaring `gloss`");
    let back = Migration {
        to: memo("cites"),
        renames: Vec::new(),
        relation_renames: vec![("quotes".into(), "cites".into())],
    };
    let e = f.catalog.migrate(owner, back).await;
    let e = e.expect_err("a rename of a relation whose link type is referred to");
    assert!(invalid_naming(&e, &["quotes", "`gloss` refers"]), "{e:?}");
}

/// A proposed migration that renames a field and a relation waits while
/// the owner renames both otherwise, and `aside`, which the proposal retypes.
/// Listed to a reviewer kept from a field, or from the relation's link
/// field, by a grant on the name in force, its shape holds none of them
/// (lakelands#594, lakelands#644), on the mock as on the store.
pub async fn a_proposal_that_waited_hides_by_the_names_in_force(store: Arc<dyn LakeStore>) {
    use lakelands_api::write::Migration;
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let pad = |field: &str, relation: &str, aside: &str| -> lakelands_api::ontology::TypeDecl {
        let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
        let cites = json!({"name": relation, "target": "pad", "cardinality": "many_to_many",
            "fields": [text("why")]});
        serde_json::from_value(
            json!({"name": "pad", "fields": [text(field), text(aside)], "relations": [cites]}),
        )
        .expect("a type")
    };
    let declared = f
        .catalog
        .declare(owner, Declaration::Type(pad("memo", "cites", "aside")));
    declared.await.expect("declaring `pad`");
    let renames = |field: &str, relation: &str, aside: &str| Migration {
        to: pad(field, relation, aside),
        renames: [("memo", field), ("aside", aside)]
            .into_iter()
            .filter(|(old, new)| old != new)
            .map(|(old, new)| (old.into(), new.into()))
            .collect(),
        relation_renames: vec![("cites".into(), relation.into())],
    };
    // The proposal also retypes `aside`, so its report converts it.
    let mut proposal = renames("note", "quotes", "aside");
    proposal.to.fields[1].ty = FieldType::kind(StorageKind::Number);
    let write = ProposedWrite::Migrate {
        migration: Box::new(proposal),
        report: Default::default(),
    };
    let waits = f.catalog.propose(owner, write, "other names".into()).await;
    let waits = match waits.expect("proposing the renames") {
        Outcome::Proposed { proposal, .. } => proposal,
        other => panic!("a proposed migration is answered {other:?}"),
    };
    let now = renames("remark", "refers", "margin");
    let report = f.catalog.migrate(owner, now.clone()).await;
    let id = report.expect("the preflight").data.report_id;
    let applied = f.catalog.migrate_apply(owner, now, &id).await;
    applied.expect("the owner renames both otherwise");
    let none = |fields: &[&str], default: Option<Access>| TypeGrants {
        default,
        fields: fields.iter().map(|f| ((*f).into(), Access::None)).collect(),
        ..Default::default()
    };
    for (on, kept, hidden) in [
        ("pad", none(&["remark"], None), "note"),
        ("pad", none(&["margin"], None), "aside"),
        ("sys.link.pad.refers", none(&["why"], None), "why"),
        ("sys.link.pad.refers", none(&[], Some(Access::None)), "why"),
    ] {
        let mut grants = Grants {
            default: Access::Read,
            review: true,
            ..Grants::default()
        };
        grants.types.insert(on.into(), kept);
        let who = principal(f.store.lake(), PrincipalKind::Human, "narrow", grants);
        let listed = f.catalog.proposals(&who, ProposalFilter::default()).await;
        let listed = listed.expect("listing the proposals").data;
        let write = listed.into_iter().find(|p| p.id == waits);
        let write = json!(write.and_then(|p| p.write)).to_string();
        let named = format!("\"{hidden}\"");
        assert!(
            !write.contains(&named),
            "kept by `{on}`, it is listed {write}"
        );
    }
}

/// `recompute` appends the function in force again and answers its
/// position; it refuses what it cannot name and appends nothing then:
/// without the `schema` grant not granted, a name no function or type has
/// not found, a type with no field computed outside the lake invalid.
pub async fn recompute_appends_the_function_in_force_again(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (owner, clerk) = (&f.cast.owner, &f.cast.clerk);
    let shout = json!({"declare": "function", "name": "shout", "backing": "expr",
        "inputs": [{"name": "t", "type": {"kind": "text"}}], "expr": "upper(t)"});
    let shout: Declaration = serde_json::from_value(shout).expect("a function");
    f.catalog.declare(owner, shout).await.expect("declaring");
    let function = |n: &str| Recompute::Function(n.into());
    let before = f.head().await;
    let done = f.catalog.recompute(owner, function("shout")).await;
    let done = done.expect("the door on a function in force");
    let head = f.head().await;
    assert!(head > before && done.position() == head, "{done:?}");
    let e = f.catalog.recompute(clerk, function("ghost")).await;
    let e = e.expect_err("the door without the `schema` grant");
    assert_eq!(refusal(&e), (&None, &Refusal::NotGranted));
    for what in [function("ghost"), Recompute::Type("ghost".into())] {
        let e = f.catalog.recompute(owner, what).await;
        assert!(matches!(e, Err(Error::NotFound { .. })), "{e:?}");
    }
    let e = f.catalog.recompute(owner, Recompute::Type(INVOICE.into()));
    let e = e
        .await
        .expect_err("a type with nothing computed outside the lake");
    assert!(invalid_naming(&e, &[INVOICE]), "{e:?}");
    assert_eq!(f.head().await, head, "a refused call appended");
}

/// A derived field, an object set's `where`, a trigger's `when` and a
/// verb's `requires` or the value it sets a field from read only fields
/// the type has: one that reads another is refused by `declare` and by
/// `apply`, naming the declaration and the field, and nothing is
/// appended, on the mock as on the store. `payload.<name>` and `setting`
/// read no field.
pub async fn a_declaration_reads_only_what_its_type_has(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let verb = |name: &str, requires: &str, value: &str| {
        json!({"declare": "verb", "name": name, "on": INVOICE, "risk": "low",
               "payload": [{"name": "reference", "type": {"kind": "text"}}],
               "requires": requires, "effect": "write",
               "set": [{"field": "status", "value": value}]})
    };
    let head = f.head().await;
    for (reader, d) in [
        (
            "type declaration `memo`",
            json!({"declare": "type", "name": "memo", "fields": [{"name": "loud",
                   "type": {"kind": "text"}, "authority": "derived", "derived": "upper(ghost)"}]}),
        ),
        (
            "verb declaration `close`",
            verb("close", "ghost = 'x'", "'shut'"),
        ),
        (
            "verb declaration `copy`",
            verb("copy", "status = 'x'", "ghost"),
        ),
        (
            "trigger declaration `t`",
            json!({"declare": "trigger", "name": "t", "on": INVOICE, "when": "changed(ghost)"}),
        ),
        (
            "object_set declaration `s`",
            json!({"declare": "object_set", "name": "s", "of": INVOICE, "where": "ghost = 'x'"}),
        ),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        let names = [reader, "reads `ghost`"];
        let e = f.catalog.declare(owner, d.clone()).await;
        let e = e.expect_err("a declaration reads only what its type has");
        assert!(invalid_naming(&e, &names), "declare: {e:?}");
        let e = f.catalog.apply(owner, vec![d]).await;
        let e = e.expect_err("a declaration reads only what its type has");
        assert!(invalid_naming(&e, &names), "apply: {e:?}");
    }
    // In a verb's value only `payload.<name>` is the call's: every other
    // path is the record's, and a member the payload lacks is never given.
    let reads = ["verb declaration `p`", "reads `payload`"];
    for (value, names) in [
        ("payload", reads),
        ("payload.reference.deep", reads),
        ("upper(payload)", reads),
        ("payload.ghost", ["verb `p`", "`payload.ghost`"]),
    ] {
        let d = serde_json::from_value(verb("p", "status = 'x'", value)).expect("a verb");
        let e = f.catalog.declare(owner, d).await;
        let e = e.expect_err("only `payload.<name>` is the call's");
        assert!(invalid_naming(&e, &names), "`{value}`: {e:?}");
    }
    assert_eq!(f.head().await, head, "nothing is appended");
    let fine = verb("refer", "status = setting('mode')", "payload.reference");
    f.catalog
        .declare(owner, serde_json::from_value(fine).expect("a declaration"))
        .await
        .expect("the call's payload and a setting are no fields of the type");
}

/// A function over a null input is null, as the store has it (lakelands#640;
/// `docs/decisions/017-one-expression-grammar.md`): every function but
/// `concat`, which skips a null, and `coalesce`, `if`, which choose. A
/// required derived field over a null input is refused, naming the field,
/// and one that comes to a value is written. A verb's `requires` reads the
/// same way: `not` of null holds, a function of null holds nothing, and a
/// text function reads a number as its text. The mock computes no derived
/// field, so its verdict on the write is what can be compared.
pub async fn a_function_over_null_is_null_on_every_lake(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, owner) = (&f.catalog, &f.cast.owner);
    // Each over `t`, `n` or `d` left null, and whether it comes to a value.
    let cases = [
        ("lower(t)", "text", false),
        ("upper(t)", "text", false),
        ("trim(t)", "text", false),
        ("len(t)", "number", false),
        ("concat(t, 'x')", "text", true),
        ("concat(t)", "text", true),
        ("substr(t, 1)", "text", false),
        ("substr('abc', n)", "text", false),
        ("substr('abc', 1, n)", "text", false),
        ("contains(t, 'a')", "boolean", false),
        ("contains('abc', t)", "boolean", false),
        ("starts_with(t, 'a')", "boolean", false),
        ("starts_with('abc', t)", "boolean", false),
        ("replace(t, 'a', 'b')", "text", false),
        ("replace('abc', t, 'b')", "text", false),
        ("replace('abc', 'a', t)", "text", false),
        ("matches(t, 'a')", "boolean", false),
        ("matches('abc', t)", "boolean", false),
        ("abs(n)", "number", false),
        ("round(n)", "number", false),
        ("round(1.5, n)", "number", false),
        ("floor(n)", "number", false),
        ("date_bucket(d, 'day')", "date_time", false),
        ("coalesce(t, 'z')", "text", true),
        ("if(t = 'a', 'y', 'n')", "text", true),
        ("n + 1", "number", false),
        ("-n", "number", false),
        ("not (t = 'a')", "boolean", true),
        ("t = 'a'", "boolean", true),
    ];
    for (i, (expr, kind, valued)) in cases.into_iter().enumerate() {
        let name = format!("over_null_{i}");
        let d = json!({"declare": "type", "name": name, "fields": [
            {"name": "t", "type": {"kind": "text"}},
            {"name": "n", "type": {"kind": "number"}},
            {"name": "d", "type": {"kind": "date_time"}},
            {"name": "out", "type": {"kind": kind}, "authority": "derived", "derived": expr,
             "required": true},
        ]});
        let d = serde_json::from_value::<Declaration>(d).expect("a declaration");
        c.declare(owner, d).await.expect("the owner declares it");
        let wrote = c
            .upsert(
                owner,
                &ty(&name),
                &key("a"),
                patch(&[("t", Value::Null)]),
                WriteOpts::default(),
            )
            .await;
        match (valued, wrote) {
            (true, Ok(_)) => {}
            (false, Err(e)) => assert!(
                matches!(refusal(&e), (Some(n), Refusal::Constraint { .. }) if n.as_str() == "out"),
                "`{expr}` over null is null, so `out` lacks its value: {e:?}"
            ),
            (_, wrote) => panic!("`{expr}` over null: {wrote:?}"),
        }
    }
    let gate = json!({"declare": "type", "name": "gate", "fields": [
        {"name": "t", "type": {"kind": "text"}},
        {"name": "n", "type": {"kind": "number"}},
        {"name": "b", "type": {"kind": "boolean"}},
        {"name": "s", "type": {"kind": "text"}},
    ]});
    let gate = serde_json::from_value::<Declaration>(gate).expect("a declaration");
    c.declare(owner, gate)
        .await
        .expect("the owner declares `gate`");
    c.upsert(
        owner,
        &ty("gate"),
        &key("a"),
        patch(&[("n", json!(12))]),
        WriteOpts::default(),
    )
    .await
    .expect("`n` is 12, `t` and `b` are null");
    for (verb, requires, holds) in [
        ("empty", "len(t) = 0", false),
        ("digits", "len(n) = 2", true),
        ("unset", "not b", true),
    ] {
        let d = json!({"declare": "verb", "name": verb, "on": "gate", "risk": "low",
                       "requires": requires, "effect": "write",
                       "set": [{"field": "s", "value": "'x'"}]});
        let d = serde_json::from_value::<Declaration>(d).expect("a verb");
        c.declare(owner, d)
            .await
            .expect("the owner declares the verb");
        let called = c
            .call(
                owner,
                &verb.into(),
                Some(&key("a")),
                Patch::new(),
                WriteOpts::default(),
            )
            .await;
        match (holds, called) {
            (true, Ok(_)) => {}
            (false, Err(e)) => assert!(
                matches!(refusal(&e).1, Refusal::Precondition { .. }),
                "`{requires}` holds nothing over a null `t`: {e:?}"
            ),
            (_, called) => panic!("`{requires}`: {called:?}"),
        }
    }
}

/// The days between two instants are their difference divided by a unit
/// (`docs/decisions/017-one-expression-grammar.md`, extended): a `where`
/// compares `(to - from) / 1d` with a number, and a derived number field
/// may be `floor` of it. A derived number field that is the duration
/// itself would never be filled, and is refused on the field; a `where`
/// adding a number to a duration can never be evaluated, and is refused
/// as invalid. On the mock as on the store; what the field then holds is
/// the suite's to say, the mock computes no derived field.
pub async fn a_duration_by_a_unit_is_a_number_and_what_never_lands_is_refused(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (c, owner, term) = (&f.catalog, &f.cast.owner, ty("term"));
    let declared = |days: &str| {
        let when = |name: &str| json!({"name": name, "type": {"kind": "date_time"}});
        let days = json!({"name": "days", "type": {"kind": "number"}, "authority": "derived",
                          "derived": days});
        let t =
            json!({"declare": "type", "name": "term", "fields": [when("from"), when("to"), days]});
        serde_json::from_value::<Declaration>(t).expect("a declaration")
    };
    let head = f.head().await;
    let e = c.declare(owner, declared("to - from")).await;
    let e = e.expect_err("a duration never lands in a number field");
    assert!(
        matches!(refusal(&e), (Some(n), Refusal::Invalid { message })
            if n.as_str() == "days" && message.contains("a duration") && message.contains("never holds")),
        "{e:?}"
    );
    assert_eq!(f.head().await, head, "nothing is appended");
    c.declare(owner, declared("floor((to - from) / 1d)"))
        .await
        .expect("a duration divided by a unit is a number");
    let span = [
        ("from", json!("2026-08-01T00:00:00Z")),
        ("to", json!("2026-08-03T12:00:00Z")),
    ];
    c.upsert(owner, &term, &key("t1"), patch(&span), WriteOpts::default())
        .await
        .expect("the write applies");
    let asked = |filter: &str| Find {
        filter: Some(filter.into()),
        ..Find::default()
    };
    for (filter, want) in [("(to - from) / 1d > 2", 1), ("(to - from) / 1d > 3", 0)] {
        let found = c.find(owner, &term, asked(filter)).await;
        let Found::Records { rows, .. } = found.expect("the find answers").data else {
            panic!("`{filter}` answers records");
        };
        assert_eq!(rows.len(), want, "`{filter}`");
    }
    let e = c.find(owner, &term, asked("(to - from) + 1 > 0")).await;
    let e = e.expect_err("a duration plus a number has no meaning");
    assert!(
        invalid_naming(&e, &["has no meaning", "a duration and a number"]),
        "{e:?}"
    );
}

/// A set names a set with `in_set('name')`, in a set's `where` and in a
/// read's (`docs/decisions/004-the-change-feed-is-the-log.md`, extended):
/// the named filter stands in the name's place. A set that is not
/// declared, one of another type, one that names it in turn and a path
/// that steps through a reference are refused at the declaration, and a
/// set declared again for another type on `of`; nothing is appended. To a
/// reader kept from a field, a set whose filter reads it, itself or
/// through a set it names, is a set that is not declared. On the mock as
/// on the store.
pub async fn a_set_names_a_set_and_tells_no_more_than_its_filter(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, owner, clerk) = (&f.catalog, &f.cast.owner, &f.cast.clerk);
    let set = |name: &str, of: &str, filter: &str| {
        let d = json!({"declare": "object_set", "name": name, "of": of, "where": filter});
        serde_json::from_value::<Declaration>(d).expect("a declaration")
    };
    let both = "in_set('open') and in_set('hush')";
    for (name, filter) in [
        ("open", "status = 'open'"),
        ("hush", "secret = 's3'"),
        ("both", both),
    ] {
        let declared = c.declare(owner, set(name, INVOICE, filter)).await;
        declared.expect("the owner declares the set");
    }
    let open = ("status", json!("open"));
    f.write(owner, "INV-1", &[open.clone(), ("secret", json!("s3"))])
        .await;
    f.write(owner, "INV-2", &[open]).await;
    let keys = |who: &Principal, filter: &str| {
        let find = Find {
            filter: Some(filter.into()),
            ..Find::default()
        };
        let (c, who) = (c.clone(), who.clone());
        async move {
            match c.find(&who, &ty(INVOICE), find).await?.data {
                Found::Records { rows, .. } => Ok::<_, Error>(keys_of(&rows)),
                other => panic!("find lists records, got {other:?}"),
            }
        }
    };
    assert_eq!(keys(owner, "in_set('both')").await.unwrap(), ["INV-1"]);
    let rest = keys(owner, "in_set('open') and not in_set('hush')").await;
    assert_eq!(rest.unwrap(), ["INV-2"]);

    let head = f.head().await;
    let when = json!({"declare": "trigger", "name": "t", "on": INVOICE,
                      "when": "partner.name = 'x'"});
    for (d, names) in [
        (set("s", INVOICE, "in_set('ghost')"), "is not declared"),
        (
            set("s", INVOICE, "nope = 1 and in_set('open')"),
            "reads `nope`",
        ),
        (
            set("s", PARTNER, "in_set('open')"),
            "a set of `invoice`, not of `partner`",
        ),
        (set("open", INVOICE, "in_set('both')"), "names it in turn"),
        (
            set("s", INVOICE, "partner.name = 'x'"),
            "steps into `partner`",
        ),
        (
            serde_json::from_value(when).expect("a trigger"),
            "steps into `partner`",
        ),
    ] {
        let e = c.declare(owner, d.clone()).await;
        let e = e.expect_err("the declaration is refused");
        assert!(invalid_naming(&e, &[names]), "{d:?}: {e:?}");
    }
    let e = c.declare(owner, set("open", PARTNER, "name = 'x'")).await;
    let e = e.expect_err("a set is of one type");
    assert!(
        matches!(refusal(&e), (Some(n), Refusal::Invalid { .. }) if n.as_str() == "of"),
        "{e:?}"
    );
    assert_eq!(f.head().await, head, "nothing is appended");

    // The clerk reads `status` and is kept from `secret`.
    let said = |name: &'static str| {
        let asked = keys(clerk, &format!("in_set('{name}')"));
        async move {
            asked
                .await
                .map_err(|e| e.to_string().replace(name, "ghost"))
        }
    };
    let ghost = said("ghost").await.expect_err("no such set");
    for name in ["hush", "both"] {
        let hidden = said(name).await;
        assert_eq!(hidden, Err(ghost.clone()), "`{name}` reads `secret`");
    }
    assert_eq!(said("open").await.unwrap(), ["INV-1", "INV-2"]);
}

/// A declaration in force reads its type's shape after a migration as
/// before: a migration that drops or renames a field read by a derived
/// field of the type, or by an object set, a trigger or a verb on it, is
/// refused at the preflight, naming the reader and the field; so is one
/// that drops a relation's field a trigger on the relation's link type
/// reads, naming the link type; and one to a type whose own derived field
/// reads a field it does not have. A field no declaration reads migrates
/// away. An author who is not shown the reader is told that a declaration
/// reads the field, not its name. On the mock as on the store.
pub async fn a_migration_keeps_what_a_declaration_reads(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let memo = |names: &[&str], link: &[&str]| -> TypeDecl {
        let text = |n: &&str| json!({"name": n, "type": {"kind": "text"}});
        let mut fields: Vec<Value> = names.iter().map(text).collect();
        fields.push(
            json!({"name": "loud", "type": {"kind": "text"}, "authority": "derived",
                           "derived": "upper(a)"}),
        );
        serde_json::from_value(json!({"name": "memo", "field_meta": true, "fields": fields,
            "relations": [{"name": "seen_by", "target": PARTNER,
                           "fields": link.iter().map(text).collect::<Vec<_>>()}]}))
        .expect("a type")
    };
    let all = ["a", "b", "c", "d", "payload", "free"];
    f.catalog
        .declare(owner, Declaration::Type(memo(&all, &["ok"])))
        .await
        .expect("the owner declares `memo`");
    for d in [
        json!({"declare": "object_set", "name": "s", "of": "memo", "where": "b = 'x'"}),
        json!({"declare": "trigger", "name": "t", "on": "memo", "when": "changed(c)"}),
        json!({"declare": "verb", "name": "settle", "on": "memo", "risk": "low",
               "requires": "d = 'x'", "effect": "write",
               "set": [{"field": "d", "value": "'y'"}]}),
        json!({"declare": "verb", "name": "w", "on": "memo", "risk": "low",
               "effect": "write", "set": [{"field": "d", "value": "payload"}]}),
        json!({"declare": "trigger", "name": "lt", "on": "sys.link.memo.seen_by",
               "when": "ok = 'x'"}),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        f.catalog
            .declare(owner, d)
            .await
            .expect("a reader of `memo`");
    }
    let without =
        |gone: &str| -> Vec<&str> { all.iter().copied().filter(|n| *n != gone).collect() };
    let migration = |to: TypeDecl, renames: &[(&str, &str)]| lakelands_api::write::Migration {
        to,
        renames: renames
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
        relation_renames: Vec::new(),
    };
    for (to, names) in [
        (
            migration(memo(&without("a"), &["ok"]), &[]),
            vec!["type declaration `memo`", "reads `a`", "drops"],
        ),
        (
            migration(memo(&without("b"), &["ok"]), &[]),
            vec!["object_set declaration `s`", "reads `b`", "drops"],
        ),
        (
            migration(memo(&without("c"), &["ok"]), &[]),
            vec!["trigger declaration `t`", "reads `c`", "drops"],
        ),
        (
            migration(memo(&without("d"), &["ok"]), &[]),
            vec!["verb declaration `settle`", "reads `d`", "drops"],
        ),
        (
            migration(memo(&without("payload"), &["ok"]), &[]),
            vec!["verb declaration `w`", "reads `payload`", "drops"],
        ),
        (
            migration(
                memo(&["a", "b2", "c", "d", "payload", "free"], &["ok"]),
                &[("b", "b2")],
            ),
            vec!["object_set declaration `s`", "reads `b`", "renames to `b2`"],
        ),
        (
            migration(memo(&all, &[]), &[]),
            vec![
                "trigger declaration `lt`",
                "reads `ok` of `sys.link.memo.seen_by`",
                "drops",
            ],
        ),
    ] {
        let e = f.catalog.migrate(owner, to).await;
        let e = e.expect_err("a field a declaration reads stays");
        assert!(invalid_naming(&e, &names), "must name {names:?}: {e:?}");
    }
    // A reader its author is not shown is not named to it: `audit` reads
    // `body`, which the clerk sees masked, so `describe` leaves the trigger
    // out for it. The clerk is told that a declaration reads the field it
    // took away; the owner is told the trigger's name.
    let doc = |names: &[&str]| -> TypeDecl {
        let text = |n: &&str| json!({"name": n, "type": {"kind": "text"}});
        let fields: Vec<Value> = names.iter().map(text).collect();
        serde_json::from_value(json!({"name": "doc", "field_meta": true, "fields": fields}))
            .expect("a type")
    };
    let audit = json!({"declare": "trigger", "name": "audit", "on": "doc",
                       "when": "note = 'x' and body = 'y'"});
    let audit: Declaration = serde_json::from_value(audit).expect("a declaration");
    for d in [Declaration::Type(doc(&["keep", "body", "note"])), audit] {
        f.catalog
            .declare(owner, d)
            .await
            .expect("`doc` and its trigger");
    }
    let to = migration(doc(&["keep"]), &[]);
    let proposed = ProposedWrite::Migrate {
        migration: Box::new(to.clone()),
        report: Default::default(),
    };
    let e = (f.catalog)
        .propose(&f.cast.clerk, proposed, "no notes".into())
        .await;
    let e = e.expect_err("a field a declaration reads stays");
    let (said, names) = (e.to_string(), ["a declaration reads `body`"]);
    assert!(invalid_naming(&e, &names), "must name {names:?}: {e:?}");
    assert!(!said.contains("audit"), "{said}");
    let e = f.catalog.migrate(owner, to).await;
    let e = e.expect_err("a field a declaration reads stays");
    let names = ["trigger declaration `audit`", "reads `body`"];
    assert!(invalid_naming(&e, &names), "must name {names:?}: {e:?}");
    // The type it migrates to is held to its own fields too.
    let mut ghost = memo(&without("free"), &["ok"]);
    ghost.fields.last_mut().expect("`loud`").derived = Some("upper(ghost)".into());
    let e = f.catalog.migrate(owner, migration(ghost, &[])).await;
    let e = e.expect_err("a migrated type reads only fields it has");
    let names = ["type declaration `memo`", "reads `ghost`", "does not have"];
    assert!(invalid_naming(&e, &names), "must name {names:?}: {e:?}");
    f.catalog
        .migrate(owner, migration(memo(&without("free"), &["ok"]), &[]))
        .await
        .expect("a field no declaration reads migrates away");
}

/// A view with parameters is bound only by a read that names it: a
/// pattern whose step arrives at one other than its start is refused
/// before anything is read, naming the view and the relation, on the mock
/// as on the store; started on the view with its parameters, it walks.
pub async fn a_step_to_an_unbound_view_is_refused(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    for d in [
        json!({"name": "by_status", "key": ["status"], "field_meta": true,
               "fields": [{"name": "status", "type": {"kind": "text"}}, {"name": "count", "type": {"kind": "number"}}],
               "view": {"source": INVOICE, "params": [{"name": "state", "kind": "text"}],
                        "where": "status = param('state')", "group_by": ["status"],
                        "measures": [{"name": "count", "expr": "count()"}]}}),
        json!({"name": "ticket", "field_meta": true,
               "fields": [{"name": "status", "type": {"kind": "text"}}],
               "relations": [{"name": "window", "target": "by_status",
                              "derived": {"match": [{"from": "status", "to": "status"}]}}]}),
    ] {
        let t: TypeDecl = serde_json::from_value(d).expect("a type");
        f.catalog
            .declare(owner, Declaration::Type(t))
            .await
            .expect("declared");
    }
    for project in [json!({"as": "rows"}), json!({"as": "degree"})] {
        let pattern: Pattern = serde_json::from_value(json!({
            "start": {"type": "ticket"},
            "steps": [{"relation": "window", "walk": "out"}],
            "project": project
        }))
        .expect("a pattern");
        let e = f
            .catalog
            .query(owner, pattern)
            .await
            .expect_err("a step arriving at an unbound view is refused");
        assert!(
            matches!(&e, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains("by_status") && message.contains("window")),
            "{project}: {e:?}"
        );
    }
}

/// A `low`-risk verb whose effect writes authored fields runs at once. A
/// payload member the verb does not declare is refused naming the verb,
/// even when its type has a field of that name.
pub async fn a_low_risk_verb_runs_at_once(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    let (c, approve, inv) = (&f.catalog, VerbName::from("approve"), key("INV-1"));
    let sent = Patch::from([("status".into(), json!("paid"))]);
    let opts = WriteOpts::default();
    let e = c.call(&f.cast.clerk, &approve, Some(&inv), sent, opts);
    let e = e.await.expect_err("`approve` declares no payload");
    let said = "verb `approve` has no `status` in its payload".to_string();
    let (on, reason) = refusal(&e);
    assert_eq!(on.as_ref().map(FieldName::as_str), Some("status"), "{e:?}");
    assert_eq!(reason, &Refusal::Invalid { message: said });
    let o = f
        .catalog
        .call(
            &f.cast.clerk,
            &VerbName::from("approve"),
            Some(&key("INV-1")),
            Patch::new(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let (p, v) = applied(&o);
    assert_eq!(v, Some(Version(2)));
    assert_eq!(f.head().await, p);
    let r = f.read(&f.cast.clerk, "INV-1").await;
    assert_eq!(field(&r, "status"), Some(&json!("approved")));
}

/// A call that could not run now is neither run nor proposed, at any risk
/// and through `call` or `propose`: at a stale version it is a conflict,
/// the version first; with a `requires` that does not hold on the record,
/// a precondition (013 and 024, extended).
pub async fn a_call_that_could_not_run_now_is_neither_run_nor_proposed(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    for risk in ["low", "high"] {
        let d = json!({"declare": "verb", "name": format!("settle_{risk}"), "on": INVOICE,
                       "risk": risk, "requires": "status = 'open'", "effect": "write",
                       "set": [{"field": "status", "value": "'settled'"}]});
        let d = serde_json::from_value::<Declaration>(d).unwrap();
        f.catalog.declare(owner, d).await.unwrap();
    }
    f.write(owner, "INV-1", &[("status", json!("open"))]).await;
    f.write(owner, "INV-1", &[("status", json!("closed"))])
        .await;
    let head = f.head().await;
    for verb in ["settle_low", "settle_high"] {
        for (version, proposed) in [
            (None, false),
            (Some(1), false),
            (None, true),
            (Some(1), true),
        ] {
            let opts = WriteOpts {
                expected_version: version.map(Version),
                ..WriteOpts::default()
            };
            let e = if proposed {
                let write = ProposedWrite::Call {
                    verb: verb.into(),
                    key: Some(key("INV-1")),
                    payload: Patch::new(),
                    opts,
                };
                f.catalog.propose(owner, write, "later".into()).await
            } else {
                let k = key("INV-1");
                f.catalog
                    .call(owner, &verb.into(), Some(&k), Patch::new(), opts)
                    .await
            }
            .expect_err("`INV-1` is closed, at version 2");
            match version {
                Some(_) => assert!(
                    matches!(e, Error::Conflict { expected, actual }
                        if expected == Version(1) && actual == Version(2)),
                    "`{verb}` at version 1, proposed {proposed}: {e:?}"
                ),
                None => assert!(
                    matches!(refusal(&e).1, Refusal::Precondition { .. }),
                    "`{verb}`, proposed {proposed}: {e:?}"
                ),
            }
        }
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// Accepting a call whose verb was declared differently after the call was
/// proposed is refused as invalid naming the verb, and the proposal stays
/// pending; declared again with the same content, it is accepted (013).
pub async fn an_accept_after_the_verb_was_declared_differently_is_refused(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let decl = |name: &str, status: &str| {
        serde_json::from_value::<Declaration>(json!({"declare": "verb", "name": name,
            "on": INVOICE, "risk": "medium", "effect": "write",
            "set": [{"field": "status", "value": format!("'{status}'")}]}))
        .unwrap()
    };
    f.write(owner, "INV-1", &[("status", json!("open"))]).await;
    for (name, again, applies) in [("changed", "rejected", false), ("same", "approved", true)] {
        f.catalog
            .declare(owner, decl(name, "approved"))
            .await
            .unwrap();
        let k = key("INV-1");
        let Outcome::Proposed { proposal, .. } = f
            .catalog
            .call(
                owner,
                &name.into(),
                Some(&k),
                Patch::new(),
                WriteOpts::default(),
            )
            .await
            .unwrap()
        else {
            panic!("a medium verb is proposed");
        };
        f.catalog.declare(owner, decl(name, again)).await.unwrap();
        let got = f.catalog.accept(&f.cast.reviewer, &proposal).await;
        if applies {
            got.expect("the same content is accepted");
        } else {
            let e = got.expect_err("a verb declared differently refuses the accept");
            assert!(
                matches!(refusal(&e).1, Refusal::Invalid { .. })
                    && e.to_string().contains("`changed`"),
                "{e:?}"
            );
            let pending = f
                .catalog
                .proposals(&f.cast.reviewer, ProposalFilter::default())
                .await
                .unwrap()
                .data
                .into_iter()
                .any(|p| p.id == proposal && p.state == ProposalState::Pending);
            assert!(pending, "the proposal stays pending");
        }
    }
}

/// A verb above `low` risk is proposed, not run; accepting the proposal
/// applies its write, caused by the acceptance.
pub async fn a_riskier_verb_is_proposed_and_accept_applies_it(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("approved"))])
        .await;
    let o = f
        .catalog
        .call(
            &f.cast.clerk,
            &VerbName::from("pay"),
            Some(&key("INV-1")),
            patch(&[("reference", json!("TX-9"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let Outcome::Proposed {
        position, proposal, ..
    } = o
    else {
        panic!("a high-risk verb is proposed, got {o:?}");
    };
    assert_eq!(f.head().await, position);
    let r = f.read(&f.cast.clerk, "INV-1").await;
    assert_eq!(
        field(&r, "status"),
        Some(&json!("approved")),
        "nothing ran yet"
    );

    let pending = f
        .catalog
        .proposals(
            &f.cast.reviewer,
            ProposalFilter {
                state: Some(ProposalState::Pending),
                ..ProposalFilter::default()
            },
        )
        .await
        .unwrap()
        .data;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, proposal);
    assert_eq!(pending[0].proposer, f.cast.clerk.writer());

    let (p, _) = applied(&f.catalog.accept(&f.cast.reviewer, &proposal).await.unwrap());
    assert!(p > position);
    let r = f.read(&f.cast.clerk, "INV-1").await;
    assert_eq!(field(&r, "status"), Some(&json!("paid")));
    let batch = f
        .store
        .feed()
        .poll(
            position,
            &FeedFilter {
                kinds: vec!["write_proposed".into(), "write_accepted".into()],
                types: Vec::new(),
                keys: Vec::new(),
            },
            10,
        )
        .await
        .unwrap();
    let kinds: Vec<&str> = batch.events.iter().map(|e| e.body.kind()).collect();
    assert_eq!(kinds, ["write_proposed", "write_accepted"]);
}

/// A rejection writes nothing but itself, and a decided proposal cannot
/// be decided again.
pub async fn a_rejected_proposal_writes_nothing_and_stays_decided(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("approved"))])
        .await;
    let o = f
        .catalog
        .call(
            &f.cast.clerk,
            &VerbName::from("pay"),
            Some(&key("INV-1")),
            Patch::new(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let Outcome::Proposed { proposal, .. } = o else {
        panic!("expected a proposal, got {o:?}");
    };
    let r = f
        .catalog
        .reject(&f.cast.reviewer, &proposal, "not this month".into())
        .await
        .unwrap();
    assert_eq!(applied(&r).1, None, "a rejection has no record version");
    assert_eq!(f.read(&f.cast.clerk, "INV-1").await.version, Version(1));
    let e = f
        .catalog
        .accept(&f.cast.reviewer, &proposal)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::Decided);
}

/// Only a principal granted review may decide or list proposals.
pub async fn only_a_reviewer_decides_proposals(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("approved"))])
        .await;
    let o = f
        .catalog
        .call(
            &f.cast.clerk,
            &VerbName::from("pay"),
            Some(&key("INV-1")),
            Patch::new(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let Outcome::Proposed { proposal, .. } = o else {
        panic!("expected a proposal, got {o:?}");
    };
    let e = f
        .catalog
        .accept(&f.cast.clerk, &proposal)
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
    let e = f
        .catalog
        .reject(&f.cast.clerk, &proposal, "mine to drop".into())
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
    let e = f
        .catalog
        .accept(&f.cast.reviewer, &"no-such-proposal".into())
        .await
        .unwrap_err();
    assert!(matches!(
        e,
        Error::NotFound {
            missing: Missing::Proposal { .. }
        }
    ));
}

/// Retiring removes every field of a record, so it needs a write grant
/// on every field of the type; writing some fields is not enough.
pub async fn a_narrow_writer_cannot_retire(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    let head = f.head().await;
    let e = f
        .catalog
        .retire(
            &f.cast.clerk,
            &ty(INVOICE),
            &key("INV-1"),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
    assert_eq!(f.head().await, head, "nothing was appended");
    let d = f
        .catalog
        .describe_type(&f.cast.clerk, &ty(INVOICE), ReadAt::default())
        .await
        .unwrap()
        .data;
    assert!(d.may.upsert && !d.may.retire);
    let tools = tools(
        &f.catalog
            .describe(&f.cast.clerk, ReadAt::default())
            .await
            .unwrap()
            .data,
    )
    .unwrap();
    assert!(tools.iter().all(|t| !matches!(t.door, Door::Retire { .. })));
    let o = f
        .catalog
        .retire(
            &f.cast.owner,
            &ty(INVOICE),
            &key("INV-1"),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    assert!(applied(&o).0 > head);
}

/// Without the review grant, `proposals` lists the principal's own
/// proposals and no one else's; a reviewer sees them all.
pub async fn a_proposer_sees_only_its_own_proposals(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.owner, "INV-1", &[("status", json!("approved"))])
        .await;
    let pay = |who: Principal| {
        let c = f.catalog.clone();
        async move {
            let o = c
                .call(
                    &who,
                    &VerbName::from("pay"),
                    Some(&key("INV-1")),
                    Patch::new(),
                    WriteOpts::default(),
                )
                .await
                .unwrap();
            match o {
                Outcome::Proposed { proposal, .. } => proposal,
                other => panic!("expected a proposal, got {other:?}"),
            }
        }
    };
    let clerks = pay(f.cast.clerk.clone()).await;
    let owners = pay(f.cast.owner.clone()).await;
    let list = |who: Principal| {
        let c = f.catalog.clone();
        async move {
            c.proposals(&who, ProposalFilter::default())
                .await
                .unwrap()
                .data
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(list(f.cast.clerk.clone()).await, vec![clerks.clone()]);
    assert!(list(f.cast.auditor.clone()).await.is_empty());
    assert_eq!(list(f.cast.reviewer.clone()).await, [owners, clerks]);
}

/// `add_field` without the schema grant is refused as not granted before
/// the type is looked up, so the refusal tells nothing of the ontology:
/// not whether the type exists, nor whether it has the field (024,
/// extended).
pub async fn add_field_without_the_schema_grant_tells_nothing_of_the_type(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let head = f.head().await;
    for name in ["nothing", "invoice"] {
        let e = f
            .catalog
            .add_field(&f.cast.clerk, &ty(name), text("status"))
            .await
            .expect_err("the clerk holds no schema grant");
        assert_eq!(refusal(&e).1, &Refusal::NotGranted, "`{name}`: {e:?}");
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// An ordinary principal cannot declare a `sys.` type; records of one are
/// written under grants like any other.
pub async fn sys_types_cannot_be_declared(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let decl = TypeDecl {
        annotations: Default::default(),
        name: "sys.setting".into(),
        key: Vec::new(),
        fields: vec![text("value")],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    let e = f
        .catalog
        .declare(&f.cast.owner, Declaration::Type(decl))
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::Reserved);
}

/// An embedding is declared only where a backend of its model is bound;
/// these doors bind none.
pub async fn an_embedding_is_unbound_where_no_backend_is_bound(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let decl: TypeDecl = serde_json::from_value(json!({
        "name": "note",
        "fields": [
            {"name": "body", "type": {"kind": "text"}},
            {"name": "vec", "type": {"kind": "vector", "dimensions": 2}, "authority": "derived",
             "embed": {"from": ["body"], "model": "m"}}
        ]
    }))
    .unwrap();
    let e = f
        .catalog
        .declare(&f.cast.owner, Declaration::Type(decl))
        .await
        .unwrap_err();
    assert!(matches!(&e, Error::Unbound { .. }), "{e:?}");
    // Added to a type already declared, the same.
    let vec = |from: &str| -> FieldDecl {
        serde_json::from_value(json!({
            "name": "vec", "type": {"kind": "vector", "dimensions": 2}, "authority": "derived",
            "embed": {"from": [from], "model": "m"}
        }))
        .unwrap()
    };
    let e = f
        .catalog
        .add_field(&f.cast.owner, &INVOICE.into(), vec("verdict"))
        .await
        .unwrap_err();
    assert!(matches!(&e, Error::Unbound { .. }), "{e:?}");
    // Declared against the rules, it is invalid before it is unbound.
    let e = f
        .catalog
        .add_field(&f.cast.owner, &INVOICE.into(), vec("nowhere"))
        .await
        .unwrap_err();
    assert!(
        matches!(refusal(&e), (Some(field), Refusal::Invalid { .. }) if field.as_str() == "vec"),
        "{e:?}"
    );
}

/// A link must reach a live record over a relation carried by link
/// records: a missing target is a constraint on `to`, a relation carried
/// by a field or not declared at all is invalid.
pub async fn a_link_to_no_live_record_or_over_no_link_relation_is_refused(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let contract: TypeDecl = serde_json::from_value(json!({
        "name": "contract",
        "fields": [{"name": "title", "type": {"kind": "text"}}],
        "relations": [{"name": "signed_by", "target": "partner", "cardinality": "many_to_many"}]
    }))
    .unwrap();
    let owner = &f.cast.owner;
    f.catalog
        .declare(owner, Declaration::Type(contract))
        .await
        .unwrap();
    f.catalog
        .upsert(
            owner,
            &"contract".into(),
            &"C-1".into(),
            patch(&[("title", json!("Supply"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let link = |ty: &'static str, key: &'static str, relation: &'static str| {
        let c = f.catalog.clone();
        let who = owner.clone();
        async move {
            c.link(
                &who,
                &ty.into(),
                &key.into(),
                &relation.into(),
                &"P-404".into(),
                Patch::new(),
                WriteOpts::default(),
            )
            .await
            .unwrap_err()
        }
    };
    let e = link("contract", "C-1", "signed_by").await;
    assert!(
        matches!(refusal(&e), (Some(field), Refusal::Constraint { .. }) if field.as_str() == "to"),
        "{e:?}"
    );
    for (ty, key, relation) in [
        ("invoice", "INV-1", "billed_by"),
        ("contract", "C-1", "nowhere"),
    ] {
        let e = link(ty, key, relation).await;
        assert!(
            matches!(refusal(&e), (None, Refusal::Invalid { .. })),
            "{ty}.{relation}: {e:?}"
        );
    }
}

/// A retired record is gone from later reads; its history is not.
pub async fn a_retired_record_leaves_reads_but_not_its_past(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (p1, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
    let o = f
        .catalog
        .retire(
            &f.cast.owner,
            &ty(INVOICE),
            &key("INV-1"),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    assert_eq!(applied(&o).1, Some(Version(2)));
    let e = f
        .catalog
        .get(&f.cast.clerk, &ty(INVOICE), get("INV-1"))
        .await
        .unwrap_err();
    assert!(matches!(
        e,
        Error::NotFound {
            missing: Missing::Record { .. }
        }
    ));
    let found = f
        .catalog
        .find(&f.cast.clerk, &ty(INVOICE), Find::default())
        .await
        .unwrap()
        .data;
    assert!(matches!(found, Found::Records { rows, .. } if rows.is_empty()));
    let then = f
        .catalog
        .get(
            &f.cast.clerk,
            &ty(INVOICE),
            Get {
                at: at(p1),
                ..get("INV-1")
            },
        )
        .await
        .unwrap();
    assert_eq!(field(&then.data, "status"), Some(&json!("open")));
}

/// The same record from the same source, delivered twice, appends once.
pub async fn ingest_is_content_addressed(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let rec = || SourceRecord {
        upstream: None,
        ty: ty(INVOICE),
        key: key("INV-1"),
        fields: patch(&[("amount", json!("120.50"))]),
        seen_at: None,
    };
    let feed = || async {
        let mut acks = f
            .catalog
            .ingest(
                &f.cast.importer,
                &"bank".into(),
                stream::of(vec![rec(), rec()]),
            )
            .await
            .unwrap();
        stream::collect(&mut acks)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let first = feed().await;
    let (records, duplicates): (u32, u32) = first
        .iter()
        .fold((0, 0), |(r, d), c| (r + c.records, d + c.duplicates));
    assert_eq!((records, duplicates), (1, 1));
    let head = f.head().await;
    let second = feed().await;
    assert!(second.iter().all(|c| c.records == 0));
    assert_eq!(f.head().await, head, "a redelivery appends nothing");
    let r = f.read(&f.cast.owner, "INV-1").await;
    assert_eq!(field(&r, "amount"), Some(&json!("120.50")));
    let e = f
        .catalog
        .ingest(&f.cast.clerk, &"bank".into(), stream::of(vec![rec()]))
        .await
        .err()
        .expect("the clerk may not ingest");
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
}

/// A record ingested with `upstream`, the lake and position its values
/// were read from, answers it in the provenance of every field it set
/// (`docs/decisions/010-provenance-per-field.md`, extended).
pub async fn an_ingested_upstream_is_answered_per_field(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let from = lakelands_api::Upstream {
        lake: "upstream-lake".into(),
        position: Position(42),
    };
    let rec = SourceRecord {
        upstream: Some(from.clone()),
        ty: ty(INVOICE),
        key: key("INV-9"),
        fields: patch(&[("amount", json!("9.90"))]),
        seen_at: None,
    };
    let mut acks = f
        .catalog
        .ingest(&f.cast.importer, &"bank".into(), stream::of(vec![rec]))
        .await
        .expect("the importer ingests");
    for ack in stream::collect(&mut acks).await {
        ack.expect("the window commits");
    }
    let got = f
        .catalog
        .get(
            &f.cast.owner,
            &ty(INVOICE),
            Get {
                key: key("INV-9"),
                with_meta: true,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .expect("the owner reads `INV-9`")
        .data;
    assert_eq!(
        got.meta.get("amount").and_then(|m| m.upstream.clone()),
        Some(from),
        "`amount` names the lake and position it was read from"
    );
}

/// A value type's tightening can be proposed by a principal without the
/// `schema` grant and holds its preflight's report; while a stored value
/// would not fit, the proposal and the accept are refused with the count
/// alone, and an accept after the value type was declared again is
/// refused; accepted, the value type accepts less
/// (`docs/decisions/013-verbs-run-by-declared-risk.md`, extended).
pub async fn a_value_types_tightening_is_proposed_and_accepted_while_it_fits(
    store: Arc<dyn LakeStore>,
) {
    use lakelands_api::write::{ProposalFilter, ProposalKind, ProposedWrite};
    let f = setup(store).await;
    let (c, own, clerk) = (&f.catalog, &f.cast.owner, &f.cast.clerk);
    let states = |one_of: &[&str]| -> lakelands_api::ontology::ValueTypeDecl {
        serde_json::from_value(json!({"name": "memo_state", "kind": "text", "one_of": one_of}))
            .unwrap()
    };
    let narrower = || ProposedWrite::MigrateValueType {
        to: Box::new(states(&["open", "filed"])),
        report: Default::default(),
    };
    let wide = Declaration::ValueType(states(&["open", "filed", "void"]));
    c.declare(own, wide).await.expect("the states are declared");
    let memo = json!({"declare": "type", "name": "memo", "fields": [
        {"name": "state", "type": {"kind": "text", "value_type": "memo_state"}}]});
    let memo = serde_json::from_value(memo).unwrap();
    c.declare(own, memo).await.expect("`memo` is declared");
    let write = |k: &'static str, state: &'static str| {
        let (c, own) = (c.clone(), own.clone());
        let fields = patch(&[("state", json!(state))]);
        async move {
            c.upsert(&own, &ty("memo"), &key(k), fields, WriteOpts::default())
                .await
        }
    };
    write("m1", "open").await.expect("`m1` is open");
    let proposed = c.propose(clerk, narrower(), "no memo is void".into()).await;
    let Ok(Outcome::Proposed { proposal, .. }) = proposed else {
        panic!("the clerk, without the schema grant, proposes the tightening: {proposed:?}")
    };
    let by_kind = ProposalFilter {
        kind: Some(ProposalKind::MigrateValueType),
        ..ProposalFilter::default()
    };
    let listed = c.proposals(own, by_kind).await.unwrap().data;
    assert!(
        matches!(listed.as_slice(), [p] if matches!(&p.write,
            Some(ProposedWrite::MigrateValueType { report, .. }) if report.fits && report.fields.len() == 1)),
        "the proposal holds a report that fits, with its one use: {listed:?}"
    );
    // One `void` stored: the accept and a new proposal say the count alone.
    write("m2", "void").await.expect("`void` is still a state");
    let counted = |e: &Error| {
        matches!(refusal(e).1, Refusal::Invalid { message }
            if message.contains("1 stored value(s)") && !message.contains("m2"))
    };
    let late = c
        .accept(own, &proposal)
        .await
        .expect_err("it no longer fits");
    let again = c
        .propose(clerk, narrower(), "again".into())
        .await
        .expect_err("it does not fit");
    assert!(counted(&late) && counted(&again), "{late:?} {again:?}");
    // Accepted on the declaration it was proposed on, it is in force.
    write("m2", "open").await.expect("`m2` is open again");
    c.accept(own, &proposal)
        .await
        .expect("the owner accepts the tightening");
    write("m2", "void")
        .await
        .expect_err("`void` is no longer a state");
    // Proposed on one declaration and accepted on another, it is refused.
    let proposed = c
        .propose(
            clerk,
            ProposedWrite::MigrateValueType {
                to: Box::new(states(&["open"])),
                report: Default::default(),
            },
            "open only".into(),
        )
        .await;
    let Ok(Outcome::Proposed { proposal, .. }) = proposed else {
        panic!("a second tightening is proposed: {proposed:?}")
    };
    let wider = Declaration::ValueType(states(&["open", "filed", "held"]));
    c.declare(own, wider)
        .await
        .expect("a state more is declared");
    let stale = c
        .accept(own, &proposal)
        .await
        .expect_err("the value type changed");
    assert!(
        matches!(refusal(&stale).1, Refusal::Invalid { message } if message.contains("changed since")),
        "{stale:?}"
    );
}

/// `proposals` shows a proposed value where a read of the record would:
/// the keeper, who writes `secret` and never reads it, proposes it in a
/// patch and through a verb's payload, and lists its own proposals with
/// neither; a reviewer who reads the field lists both whole, and the
/// accept applies the logged write whole
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended).
pub async fn a_proposal_shows_its_reader_the_values_it_may_read(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, own, keeper) = (&f.catalog, &f.cast.owner, &f.cast.keeper);
    let hide = json!({"declare": "verb", "name": "hide", "on": INVOICE, "risk": "high",
        "payload": [{"name": "word", "type": {"kind": "text"}}],
        "effect": "write", "set": [{"field": "secret", "value": "payload.word"}]});
    let hide = serde_json::from_value(hide).unwrap();
    c.declare(own, hide).await.expect("`hide` is declared");
    let (i1, opts) = (key("i1"), WriteOpts::default);
    let first = patch(&[("status", json!("new"))]);
    let written = c.upsert(own, &ty(INVOICE), &i1, first, opts()).await;
    written.expect("`i1` is written");
    let whole = patch(&[("status", json!("open")), ("secret", json!("s3"))]);
    let (sent, shown) = (
        patch(&[("word", json!("w"))]),
        patch(&[("status", json!("open"))]),
    );
    let raise = ProposedWrite::Upsert {
        ty: ty(INVOICE),
        key: i1.clone(),
        patch: whole.clone(),
        opts: opts(),
    };
    let raise = c.propose(keeper, raise, "a secret".into()).await;
    let Ok(Outcome::Proposed { proposal, .. }) = raise else {
        panic!("the keeper proposes the upsert: {raise:?}")
    };
    let verb = "hide".into();
    let called = c.call(keeper, &verb, Some(&i1), sent.clone(), opts()).await;
    assert!(
        matches!(called, Ok(Outcome::Proposed { .. })),
        "a high verb's call is proposed: {called:?}"
    );
    let rows = [
        (&f.cast.reviewer, &whole, &sent),
        (keeper, &shown, &Patch::new()),
    ];
    for (who, patch, payload) in rows {
        let listed = c.proposals(who, ProposalFilter::default()).await;
        let listed = listed.expect("the proposals are listed").data;
        // Newest first: the call, then the upsert.
        let got: Vec<&Patch> = (listed.iter())
            .filter_map(|p| match p.write.as_ref()? {
                ProposedWrite::Upsert { patch, .. } => Some(patch),
                ProposedWrite::Call { payload, .. } => Some(payload),
                _ => None,
            })
            .collect();
        assert!(got == [payload, patch], "`{}` lists {got:?}", who.id);
    }
    let accepted = c.accept(&f.cast.reviewer, &proposal).await;
    accepted.expect("the reviewer accepts the upsert");
    let after = c.get(own, &ty(INVOICE), get("i1")).await.unwrap().data;
    let kept = field(&after, "secret") == Some(&json!("s3"));
    assert!(kept, "the accept applies the logged write whole: {after:?}");
}

/// `proposals` names what its reader may know of and says where there is
/// more: the keeper's proposed type, which it could not see once declared,
/// is listed without its write and matches no `kind`; a reviewer who reads
/// both lists each whole. And a proposal reads only what its proposer may
/// read: a trigger over `note`, which the clerk sees masked, or over
/// `secret`, which the keeper writes and never reads, is refused as not
/// granted on the field, and one over `secret` by the clerk, who may not
/// know of it, as over a field the type does not have
/// (`docs/decisions/013-verbs-run-by-declared-risk.md` and
/// `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended).
pub async fn a_proposal_names_what_its_reader_may_know_and_says_there_is_more(
    store: Arc<dyn LakeStore>,
) {
    use lakelands_api::write::ProposalKind;
    let f = setup(store).await;
    let (c, clerk, keeper) = (&f.catalog, &f.cast.clerk, &f.cast.keeper);
    let trigger = |name: &str, when: &str| -> Declaration {
        let t = json!({"declare": "trigger", "name": name, "on": INVOICE, "when": when});
        serde_json::from_value(t).expect("a trigger")
    };
    let vault = json!({"declare": "type", "name": "vault",
                       "fields": [{"name": "x", "type": {"kind": "text"}}]});
    let vault = vec![serde_json::from_value(vault).expect("a type")];
    let stated = vec![trigger("stated", "status = 'x'")];
    for (who, declarations) in [(clerk, stated), (keeper, vault)] {
        let bundle = ProposedWrite::Declare { declarations };
        let proposed = c.propose(who, bundle, "a bundle".into()).await;
        proposed.expect("a principal without `schema` proposes a bundle");
    }
    let refused = |who: &Principal, field: &str| {
        let declarations = vec![trigger("noted", &format!("{field} = 'x'"))];
        let (c, who) = (c.clone(), who.clone());
        let proposed = async move {
            let bundle = ProposedWrite::Declare { declarations };
            c.propose(&who, bundle, "a bundle".into()).await
        };
        async move {
            proposed
                .await
                .expect_err("a trigger over a field its proposer may not read")
        }
    };
    for (who, field) in [(clerk, "note"), (keeper, "secret")] {
        let e = refused(who, field).await;
        assert!(
            matches!(refusal(&e), (Some(f), Refusal::NotGranted) if f.as_str() == field),
            "`{}` proposes a trigger over `{field}`, which it knows of and may not read: {e:?}",
            who.id
        );
    }
    let (hidden, absent) = (refused(clerk, "secret").await, refused(clerk, "nope").await);
    assert_eq!(
        hidden.to_string().replace("secret", "nope"),
        absent.to_string(),
        "a field its proposer may not know of answers as one the type does not have"
    );
    // Per reader: the names its listing shows by kind, and each `more`.
    let declared = || ProposalFilter {
        kind: Some(ProposalKind::Declare),
        ..ProposalFilter::default()
    };
    let rows: [(&Principal, _, &[&str], &[bool]); 5] = [
        (clerk, ProposalFilter::default(), &["stated"], &[false]),
        (keeper, ProposalFilter::default(), &[], &[true]),
        (keeper, declared(), &[], &[]),
        (clerk, declared(), &["stated"], &[false]),
        (
            &f.cast.reviewer,
            declared(),
            &["vault", "stated"],
            &[false, false],
        ),
    ];
    for (who, filter, names, more) in rows {
        let listed = c.proposals(who, filter).await;
        let listed = listed.expect("the proposals are listed").data;
        let said: Vec<bool> = listed.iter().map(|p| p.more).collect();
        let shown: Vec<String> = (listed.iter().filter_map(|p| p.write.as_ref()))
            .flat_map(|w| match w {
                ProposedWrite::Declare { declarations } => declarations.clone(),
                _ => Vec::new(),
            })
            .map(|d| json!(d)["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(
            shown == names && said == more,
            "`{}` lists {shown:?}, {said:?}",
            who.id
        );
    }
}

/// A declaration or a migration can be proposed by a principal without
/// the `schema` grant; a lossy migration is refused at the proposal with
/// its report, a reviewer without `schema` is refused at the accept, and
/// proposals list by kind (`docs/decisions/013-verbs-run-by-declared-risk.md`,
/// extended).
pub async fn a_schema_proposal_is_refused_by_kind_at_propose_and_at_accept(
    store: Arc<dyn LakeStore>,
) {
    use lakelands_api::write::{Migration, ProposalFilter, ProposalKind, ProposedWrite};
    let f = setup(store).await;
    let memo: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "memo",
        "fields": [{"name": "pages", "type": {"kind": "text"}}]
    }))
    .unwrap();
    f.catalog
        .declare(&f.cast.owner, memo.clone())
        .await
        .expect("the owner declares `memo`");
    f.catalog
        .upsert(
            &f.cast.owner,
            &ty("memo"),
            &key("m1"),
            patch(&[("pages", json!("many"))]),
            WriteOpts::default(),
        )
        .await
        .expect("the owner writes `m1`");
    let Declaration::Type(mut to) = memo else {
        unreachable!()
    };
    to.fields[0].ty = FieldType::kind(StorageKind::Number);
    let e = f
        .catalog
        .propose(
            &f.cast.clerk,
            ProposedWrite::Migrate {
                migration: Box::new(Migration {
                    to,
                    renames: vec![],
                    relation_renames: Vec::new(),
                }),
                report: Default::default(),
            },
            "pages are numbers".into(),
        )
        .await
        .expect_err("a lossy migration is refused at the proposal");
    assert!(
        matches!(refusal(&e).1, Refusal::Migration { report } if !report.lossless),
        "refused with its report, not lossless: {e:?}"
    );
    let ticket: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "ticket",
        "fields": [{"name": "title", "type": {"kind": "text"}}]
    }))
    .unwrap();
    let proposed = f
        .catalog
        .propose(
            &f.cast.clerk,
            ProposedWrite::Declare {
                declarations: vec![ticket],
            },
            "tickets".into(),
        )
        .await
        .expect("the clerk, without the schema grant, proposes a type");
    let Outcome::Proposed { proposal, .. } = proposed else {
        panic!("a proposal answers proposed: {proposed:?}")
    };
    let listed = f
        .catalog
        .proposals(
            &f.cast.owner,
            ProposalFilter {
                kind: Some(ProposalKind::Declare),
                ..ProposalFilter::default()
            },
        )
        .await
        .expect("the owner lists proposals")
        .data;
    assert_eq!(
        listed.iter().map(|p| &p.id).collect::<Vec<_>>(),
        [&proposal],
        "listing by kind `declare` answers the one proposal"
    );
    let e = f
        .catalog
        .accept(&f.cast.reviewer, &proposal)
        .await
        .expect_err("a reviewer without the schema grant may not accept a declaration");
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
    f.catalog
        .accept(&f.cast.owner, &proposal)
        .await
        .expect("the owner, holding review and schema, accepts");
    f.catalog
        .upsert(
            &f.cast.owner,
            &ty("ticket"),
            &key("t1"),
            patch(&[("title", json!("first"))]),
            WriteOpts::default(),
        )
        .await
        .expect("the accepted type takes records");
}

/// A cursor is bound to the position of its first page: the page set is
/// the same however the lake is written between pages.
pub async fn a_cursor_keeps_its_page_set_while_writes_land(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    for k in ["INV-1", "INV-2", "INV-3"] {
        f.write(&f.cast.clerk, k, &[("status", json!("open"))])
            .await;
    }
    let invoice = ty(INVOICE);
    let page = |cursor| {
        f.catalog.find(
            &f.cast.clerk,
            &invoice,
            Find {
                limit: Some(2),
                cursor,
                ..Find::default()
            },
        )
    };
    let first = page(None).await.unwrap();
    let Found::Records {
        rows, next_cursor, ..
    } = first.data
    else {
        panic!("find lists records");
    };
    assert_eq!(rows.len(), 2);
    let next = next_cursor.expect("a third record is left");
    f.write(&f.cast.clerk, "INV-0", &[("status", json!("open"))])
        .await;
    f.write(&f.cast.clerk, "INV-2", &[("status", json!("paid"))])
        .await;
    let second = page(Some(next)).await.unwrap();
    assert_eq!(second.position, first.position, "the cursor pins the read");
    let Found::Records {
        rows: more,
        next_cursor,
        ..
    } = second.data
    else {
        panic!("find lists records");
    };
    let keys: Vec<&str> = rows.iter().chain(&more).map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["INV-1", "INV-2", "INV-3"]);
    assert!(next_cursor.is_none());
}

/// A page answers its total when asked, and `offset` jumps to the walk's
/// slice; `offset` beside a cursor, and `total` beside an aggregate, are
/// refused as invalid, and an offset past the lake's ceiling as too large
/// (045).
pub async fn a_page_answers_its_total_and_jumps_by_offset(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.sortable().await;
    let invoice = ty(INVOICE);
    let find = |find: Find| f.catalog.find(&f.cast.owner, &invoice, find);
    let keys = |found: Found| match found {
        Found::Records {
            rows,
            next_cursor,
            total,
        } => (
            rows.iter().map(|r| r.key.to_string()).collect::<Vec<_>>(),
            next_cursor,
            total,
        ),
        other => panic!("find lists records, got {other:?}"),
    };
    let (all, _, total) = keys(
        find(Find {
            total: true,
            ..Find::default()
        })
        .await
        .unwrap()
        .data,
    );
    assert_eq!(total, Some(all.len() as u64), "{all:?}");
    let (page, next, _) = keys(
        find(Find {
            offset: Some(2),
            limit: Some(2),
            ..Find::default()
        })
        .await
        .unwrap()
        .data,
    );
    assert_eq!(page, all[2..4]);
    let r = find(Find {
        offset: Some(1),
        cursor: next,
        ..Find::default()
    })
    .await
    .unwrap_err();
    assert!(matches!(refusal(&r).1, Refusal::Invalid { .. }), "{r:?}");
    let r = find(Find {
        offset: Some(Limits::default().max_offset + 1),
        ..Find::default()
    })
    .await
    .unwrap_err();
    assert_eq!(
        refusal(&r),
        (
            &Some(FieldName::from("offset")),
            &Refusal::TooLarge {
                max: Limits::default().max_offset
            }
        )
    );
}

/// A cursor continues only the answer it was handed out for: a cursor of
/// `find` on `invoice` is refused by `find` on another type and by `query`,
/// as invalid (018); one no lake handed out is refused by both as the
/// store says it.
pub async fn a_cursor_continues_only_its_own_answer(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.sortable().await;
    let first = f
        .catalog
        .find(
            &f.cast.owner,
            &ty(INVOICE),
            Find {
                limit: Some(1),
                ..Find::default()
            },
        )
        .await
        .unwrap();
    let Found::Records {
        next_cursor: Some(c),
        ..
    } = first.data
    else {
        panic!("a first page of one invoice hands out a cursor");
    };
    let elsewhere = f
        .catalog
        .find(
            &f.cast.owner,
            &ty(PARTNER),
            Find {
                cursor: Some(c.clone()),
                ..Find::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refusal(&elsewhere).1, Refusal::Invalid { .. }),
        "{elsewhere:?}"
    );
    let mut p: Pattern = serde_json::from_value(json!({
        "start": {"type": INVOICE},
        "project": {"as": "rows"}
    }))
    .expect("a pattern");
    p.cursor = Some(c);
    let queried = f.catalog.query(&f.cast.owner, p.clone()).await.unwrap_err();
    assert!(
        matches!(refusal(&queried).1, Refusal::Invalid { .. }),
        "{queried:?}"
    );
    // A cursor of one part, and one of five parts that do not parse.
    for foreign in ["not-a-cursor", "a.b.c.d.e"] {
        let foreign = Some(Cursor(foreign.into()));
        let find = Find {
            cursor: foreign.clone(),
            ..Find::default()
        };
        let found = f.catalog.find(&f.cast.owner, &ty(INVOICE), find).await;
        p.cursor = foreign;
        let queried = f.catalog.query(&f.cast.owner, p.clone()).await;
        for (door, e) in [("find", found.map(|_| ())), ("query", queried.map(|_| ()))] {
            assert!(
                matches!(&e, Err(Error::Refused { field: None, reason: Refusal::Invalid { message } })
                    if message == "the cursor is not one this lake handed out"),
                "{door} with the cursor {:?}, which no lake handed out: {e:?}",
                p.cursor
            );
        }
    }
}

/// A cursor continues only the read it was handed out for: `find` under
/// another `where` or another order of the same length, and `query`
/// projecting other fields, refuse it as invalid; another page size
/// continues it (018).
pub async fn a_cursor_continues_only_the_read_it_began(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.sortable().await;
    let read = |cursor, filter: &str, by: &str| Find {
        limit: Some(1),
        cursor,
        filter: Some(filter.into()),
        order_by: vec![serde_json::from_value(json!({"field": by})).expect("an order")],
        ..Find::default()
    };
    let (_, Some(c)) = f.page(read(None, "status = 'open'", "note")).await else {
        panic!("a first page of one open invoice hands out a cursor");
    };
    f.page(Find {
        limit: Some(2),
        ..read(Some(c.clone()), "status = 'open'", "note")
    })
    .await;
    for other in [
        read(Some(c.clone()), "status = 'paid'", "note"),
        read(Some(c), "status = 'open'", "verdict"),
    ] {
        let e = (f.catalog.find(&f.cast.owner, &ty(INVOICE), other).await).unwrap_err();
        assert!(matches!(refusal(&e).1, Refusal::Invalid { .. }), "{e:?}");
    }
    let rows = |fields: Value, cursor: Option<Cursor>| {
        let mut p: Pattern = serde_json::from_value(json!({
            "start": {"type": INVOICE},
            "project": {"as": "rows", "fields": fields},
            "limit": 1
        }))
        .expect("a pattern");
        p.cursor = cursor;
        p
    };
    let first = (f.catalog.query(&f.cast.owner, rows(json!([]), None)).await).map(|r| r.data);
    let Ok(lakelands_api::query::QueryResult::Rows {
        next_cursor: Some(c),
        ..
    }) = first
    else {
        panic!("a first page of one row hands out a cursor: {first:?}");
    };
    let e = (f
        .catalog
        .query(&f.cast.owner, rows(json!(["note"]), Some(c)))
        .await)
        .unwrap_err();
    assert!(matches!(refusal(&e).1, Refusal::Invalid { .. }), "{e:?}");
}

impl Fixture {
    /// Invoices with ties on `status` and rows missing `status` or
    /// `verdict`, for the order's edges; each carries a `note`, so no
    /// write is empty.
    async fn sortable(&self) {
        for (k, status, verdict) in [
            ("INV-1", Some("open"), Some("a")),
            ("INV-2", Some("open"), None),
            ("INV-3", None, Some("b")),
            ("INV-4", Some("paid"), Some("a")),
            ("INV-5", Some("open"), Some("b")),
            ("INV-6", None, None),
            ("INV-7", Some("draft"), Some("a")),
        ] {
            let mut pairs = vec![("note", json!(k))];
            if let Some(s) = status {
                pairs.push(("status", json!(s)));
            }
            if let Some(v) = verdict {
                pairs.push(("verdict", json!(v)));
            }
            self.write(&self.cast.owner, k, &pairs).await;
        }
    }

    async fn page(&self, find: Find) -> (Vec<Record>, Option<Cursor>) {
        match self
            .catalog
            .find(&self.cast.owner, &ty(INVOICE), find)
            .await
            .expect("the page reads")
            .data
        {
            Found::Records {
                rows, next_cursor, ..
            } => (rows, next_cursor),
            other => panic!("find lists records, got {other:?}"),
        }
    }
}

fn keys_of(rows: &[Record]) -> Vec<String> {
    rows.iter().map(|r| r.key.as_str().to_owned()).collect()
}

/// A row's position in `order`: its sort values, `null` where it has
/// none, and its key.
fn position_of(order: &[Order], r: &Record) -> After {
    After {
        values: order
            .iter()
            .map(|o| field(r, o.field.as_str()).cloned().unwrap_or(Value::Null))
            .collect(),
        key: r.key.clone(),
    }
}

/// `after`, resumed from each page's last row, walks the pages the
/// cursor walks: over one sort key and two, with ties broken by key and
/// a missing value last in either direction.
pub async fn after_walks_the_pages_the_cursor_walks(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.sortable().await;
    let by = |name: &str, dir| Order {
        field: name.into(),
        by: OrderBy::Value,
        dir,
    };
    let orders = [
        (
            vec![by("status", Direction::Asc)],
            [
                "INV-7", "INV-1", "INV-2", "INV-5", "INV-4", "INV-3", "INV-6",
            ],
        ),
        (
            vec![by("status", Direction::Desc), by("verdict", Direction::Asc)],
            [
                "INV-4", "INV-1", "INV-5", "INV-2", "INV-7", "INV-3", "INV-6",
            ],
        ),
    ];
    for (order, expected) in orders {
        let first = Find {
            order_by: order.clone(),
            limit: Some(2),
            ..Find::default()
        };
        let mut by_cursor = Vec::new();
        let mut find = first.clone();
        loop {
            let (rows, next) = f.page(find.clone()).await;
            by_cursor.push(keys_of(&rows));
            let Some(c) = next else { break };
            find.cursor = Some(c);
        }
        let mut by_after = Vec::new();
        let mut find = first.clone();
        for _ in 0..expected.len() + 1 {
            let (rows, _) = f.page(find.clone()).await;
            let Some(last) = rows.last() else { break };
            find.after = Some(position_of(&order, last));
            by_after.push(keys_of(&rows));
        }
        assert_eq!(
            by_after, by_cursor,
            "under {order:?}, `after` from each page's last row walks the cursor's pages"
        );
        assert_eq!(
            by_cursor.concat(),
            expected,
            "the one order under {order:?}"
        );
    }
}

/// A `limit` of 0, or above what the lake allows, is refused as invalid,
/// naming the bounds, at every door that takes one: `find`, `query` and
/// `proposals` up to `max_limit`, a whole-graph measure up to
/// `max_graph_nodes`. It is never cut to fit.
pub async fn a_limit_outside_the_lakes_bounds_is_refused_at_every_door(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, own) = (&f.catalog, &f.cast.owner);
    let limits = Limits::default();
    let rows = |limit| {
        let mut p = Pattern::traverse(ty(INVOICE), key("unused"), Vec::new());
        (p.start.key, p.limit) = (None, Some(limit));
        p
    };
    let measure = |limit| {
        let mut p = rows(limit);
        p.project = lakelands_api::query::Projection::Components;
        p
    };
    for (max, measured) in [(limits.max_limit, false), (limits.max_graph_nodes, true)] {
        for limit in [0, max + 1] {
            let find = Find {
                limit: Some(limit),
                ..Find::default()
            };
            let filter = ProposalFilter {
                limit: Some(limit),
                ..ProposalFilter::default()
            };
            let answers = match measured {
                true => vec![("a measure", c.query(own, measure(limit)).await.map(|_| ()))],
                false => vec![
                    ("find", c.find(own, &ty(INVOICE), find).await.map(|_| ())),
                    ("query", c.query(own, rows(limit)).await.map(|_| ())),
                    ("proposals", c.proposals(own, filter).await.map(|_| ())),
                ],
            };
            for (door, answer) in answers {
                let want = format!("a limit is between 1 and {max}");
                assert!(
                    matches!(&answer, Err(Error::Refused { field: None, reason: Refusal::Invalid { message } })
                        if *message == want),
                    "{door} with `limit` {limit} must be refused as invalid: {want}; it answered {answer:?}"
                );
            }
        }
    }
}

/// `after` is refused as invalid on no field beside a cursor or an
/// aggregate or with a value count other than `order_by`'s, and on the
/// sort key's field when a value is one that field cannot hold.
pub async fn after_is_refused_naming_the_member(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.sortable().await;
    let order = vec![Order {
        field: "status".into(),
        by: OrderBy::Value,
        dir: Direction::Asc,
    }];
    let first = Find {
        order_by: order.clone(),
        limit: Some(2),
        ..Find::default()
    };
    let (rows, cursor) = f.page(first.clone()).await;
    let resume = position_of(&order, rows.last().expect("a first page"));
    let aggregate = serde_json::from_value(json!({"measures": [{"name": "c", "func": "count"}]}))
        .expect("an aggregate");
    for (what, find) in [
        (
            "`after` beside `cursor`",
            Find {
                cursor,
                after: Some(resume.clone()),
                ..first.clone()
            },
        ),
        (
            "`after` beside `aggregate`",
            Find {
                order_by: Vec::new(),
                limit: None,
                after: Some(After {
                    values: Vec::new(),
                    key: key("INV-1"),
                }),
                aggregate: Some(aggregate),
                ..Find::default()
            },
        ),
        (
            "two `after` values for one `order_by` key",
            Find {
                after: Some(After {
                    values: vec![json!("open"), json!("a")],
                    key: key("INV-1"),
                }),
                ..first.clone()
            },
        ),
    ] {
        let e = f
            .catalog
            .find(&f.cast.owner, &ty(INVOICE), find)
            .await
            .expect_err(what);
        assert!(
            matches!(refusal(&e), (None, Refusal::Invalid { message }) if message.contains("after")),
            "{what} is refused as invalid on no field, naming `after`; got {e:?}"
        );
    }
    let unfit = Find {
        order_by: vec![Order {
            field: "amount".into(),
            by: OrderBy::Value,
            dir: Direction::Asc,
        }],
        after: Some(After {
            values: vec![json!("twelve")],
            key: key("INV-1"),
        }),
        ..first
    };
    let e = f
        .catalog
        .find(&f.cast.owner, &ty(INVOICE), unfit)
        .await
        .expect_err("a value `amount` cannot hold");
    assert!(
        matches!(refusal(&e), (Some(field), Refusal::Invalid { message })
            if field.as_str() == "amount" && message.contains("after")),
        "a value `amount` cannot hold is refused as invalid on `amount`, naming `after`; got {e:?}"
    );
}

/// A principal addresses one lake; any other lake refuses it.
pub async fn a_principal_of_another_lake_is_refused(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let mut other = f.cast.owner.clone();
    other.lake = LakeId::from(format!("{}-elsewhere", f.store.lake()));
    let e = f
        .catalog
        .describe(&other, ReadAt::default())
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
    let e = f
        .catalog
        .upsert(
            &other,
            &ty(INVOICE),
            &key("INV-1"),
            patch(&[("status", json!("open"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(&e).1, &Refusal::NotGranted);
}

/// The change feed is the log read from a position: consecutive
/// positions, the kinds asked for, and `next` to poll from.
pub async fn the_feed_is_the_log_by_position(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let from = f.head().await.next();
    let (p1, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
    let (p2, _) = applied(
        &f.write(&f.cast.clerk, "INV-2", &[("status", json!("open"))])
            .await,
    );
    let feed = f.store.feed();
    let b = feed.poll(from, &FeedFilter::default(), 100).await.unwrap();
    let positions: Vec<Position> = b.events.iter().map(|e| e.position).collect();
    assert_eq!(positions, [p1, p2]);
    assert_eq!(b.next, p2.next());
    assert_eq!(b.head, p2);
    assert!(b.events.iter().all(|e| e.origin == Origin::Authored));
    assert!(b.events.iter().all(|e| e.writer == f.cast.clerk.writer()));
    let only = feed
        .poll(
            Position::GENESIS,
            &FeedFilter {
                kinds: vec!["record_upserted".into()],
                types: vec![ty(INVOICE)],
                keys: Vec::new(),
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(only.events.len(), 1);
    assert_eq!(only.events[0].position, p1);
    assert_eq!(only.next, p1.next());
    let empty = feed
        .poll(b.next, &FeedFilter::default(), 100)
        .await
        .unwrap();
    assert!(empty.events.is_empty());
    // That a rebuild changes nothing in the feed is the conformance
    // suite's to claim, on every engine
    // (`rebuilding_the_projection_changes_nothing_in_the_feed`): a rebuild
    // is the host's, and no door serves it.
}

/// A poll examines at most `FEED_EXAMINED` events (lakelands#558): one
/// that matches nothing answers no events and `next` at the bound, short
/// of the head, and following `next` reaches the head; a match at the last
/// position a poll examines is in it, and not in the poll that stops just
/// before it; and no `limit` answers more than the bound.
pub async fn a_feed_poll_examines_at_most_the_bound(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let from = f.head().await.next();
    let records = (0..4200).map(|i| SourceRecord {
        upstream: None,
        ty: ty(INVOICE),
        key: key(&format!("B-{i}")),
        fields: patch(&[("amount", json!(format!("{i}.00")))]),
        seen_at: None,
    });
    let (bank, records) = ("bank".into(), stream::of(records.collect()));
    let ingested = f.catalog.ingest(&f.cast.importer, &bank, records).await;
    let acks = stream::collect(&mut ingested.unwrap()).await;
    assert!(acks.iter().all(Result::is_ok), "{acks:?}");
    let (feed, bound) = (f.store.feed(), u64::from(FEED_EXAMINED));
    let filter = |kinds: &[&str], keys: &[&str]| FeedFilter {
        kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        keys: keys.iter().map(|k| key(k)).collect(),
        ..FeedFilter::default()
    };
    let none = filter(&["trigger_fired"], &[]);
    let (mut at, mut polls) = (from, 0);
    loop {
        let b = feed.poll(at, &none, 100).await.unwrap();
        assert!(b.events.is_empty());
        assert_eq!(
            b.next.0,
            (at.0 + bound).min(b.head.0 + 1),
            "a poll from {at}"
        );
        (at, polls) = (b.next, polls + 1);
        if at > b.head {
            break;
        }
    }
    assert_eq!(polls, (at.0 - from.0).div_ceil(bound));
    let last = filter(&[], &["B-4199"]);
    let (mut at, mut p) = (from, None);
    while p.is_none() {
        let b = feed.poll(at, &last, 1).await.unwrap();
        p = b.events.first().map(|e| e.position);
        assert!(
            p.is_some() || b.next <= b.head,
            "`B-4199`'s event is in the log"
        );
        at = b.next;
    }
    let p = p.expect("found");
    let mut b = feed.poll(Position(p.0 + 1 - bound), &last, u32::MAX).await;
    assert_eq!(
        b.as_ref().unwrap().events.last().map(|e| e.position),
        Some(p)
    );
    assert_eq!(b.as_ref().unwrap().next, p.next());
    b = feed.poll(Position(p.0 - bound), &last, u32::MAX).await;
    assert!(b.as_ref().unwrap().events.is_empty());
    assert_eq!(b.unwrap().next, p);
    let all = feed
        .poll(from, &FeedFilter::default(), u32::MAX)
        .await
        .unwrap();
    assert_eq!(
        (all.events.len() as u64, all.next.0),
        (bound, from.0 + bound)
    );
}

/// The feed filtered by key answers only the events of those records, in
/// order: with a type, one record's history; with no type, a key in any
/// type. A key may hold a comma, and is still one key (lakelands#146).
pub async fn the_feed_filtered_by_key_answers_those_records_events(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let from = f.head().await.next();
    let (p1, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
    applied(
        &f.write(&f.cast.clerk, "INV-2", &[("status", json!("open"))])
            .await,
    );
    let (p3, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("paid"))])
            .await,
    );
    let (p4, _) = applied(
        &f.write(&f.cast.clerk, "INV,4", &[("status", json!("open"))])
            .await,
    );
    let feed = f.store.feed();
    let positions = |keys: &[&str], types: &[&str]| {
        let filter = FeedFilter {
            kinds: Vec::new(),
            types: types.iter().map(|t| ty(t)).collect(),
            keys: keys.iter().map(|k| key(k)).collect(),
        };
        let feed = feed.clone();
        async move {
            let b = feed.poll(from, &filter, 100).await.unwrap();
            b.events.iter().map(|e| e.position).collect::<Vec<_>>()
        }
    };
    assert_eq!(
        positions(&["INV-1"], &[INVOICE]).await,
        [p1, p3],
        "one record's history"
    );
    assert_eq!(
        positions(&["INV-1", "INV,4"], &[]).await,
        [p1, p3, p4],
        "keys in any type"
    );
    assert_eq!(positions(&["INV"], &[]).await, [], "a key is matched whole");
}

/// A pattern step over a relation the type does not declare is refused,
/// not answered empty.
pub async fn a_step_over_an_undeclared_relation_is_refused(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.catalog
        .upsert(
            &f.cast.owner,
            &ty(PARTNER),
            &key("ACME"),
            patch(&[("name", json!("Acme"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    f.write(&f.cast.clerk, "INV-1", &[("partner", json!("ACME"))])
        .await;
    let step = |relation: &str| Step {
        relation: relation.into(),
        walk: Walk::Out,
        ty: None,
        filter: None,
        depth: Default::default(),
    };
    let walked = f
        .catalog
        .query(
            &f.cast.owner,
            Pattern::traverse(ty(INVOICE), key("INV-1"), vec![step("billed_by")]),
        )
        .await
        .unwrap();
    let json = serde_json::to_value(&walked.data).unwrap();
    assert_eq!(json["rows"][0]["partner.name"], json!("Acme"));
    let e = f
        .catalog
        .query(
            &f.cast.owner,
            Pattern::traverse(ty(INVOICE), key("INV-1"), vec![step("audited_by")]),
        )
        .await
        .unwrap_err();
    assert!(matches!(refusal(&e).1, Refusal::Invalid { .. }));
}

/// A step naming a relation's `inverse` walks that relation the other way,
/// its `walk` counting from the name it gives, over a reference field, link
/// records and matching values alike. A relation declared under the name
/// wins: `owns` is both `person`'s own relation and the inverse of
/// `kept_by`, and a step over it walks `person.owns`.
pub async fn a_step_naming_an_inverse_walks_the_relation_the_other_way(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let declare = |v: Value| {
        let c = c.clone();
        async move {
            let d = serde_json::from_value::<Declaration>(v).unwrap();
            c.declare(own, d).await.unwrap();
        }
    };
    declare(json!({"declare": "type", "name": "person",
        "fields": [{"name": "name", "type": {"kind": "text"}}]}))
    .await;
    declare(json!({"declare": "type", "name": "doc", "fields": [
        {"name": "title", "type": {"kind": "text"}},
        {"name": "owner", "type": {"kind": "text"}},
        {"name": "author", "type": {"kind": "reference", "target": "person"}}],
        "relations": [
            {"name": "written_by", "target": "person", "via": "author", "inverse": "wrote"},
            {"name": "signed_by", "target": "person", "inverse": "signed"},
            {"name": "owned_by", "target": "person", "inverse": "holds",
             "derived": {"match": [{"from": "owner", "to": "name"}]}},
            {"name": "kept_by", "target": "person", "inverse": "owns",
             "derived": {"match": [{"from": "owner", "to": "name"}]}}]}))
    .await;
    declare(json!({"declare": "type", "name": "person",
        "fields": [{"name": "name", "type": {"kind": "text"}}],
        "relations": [{"name": "owns", "target": "doc",
            "derived": {"match": [{"from": "name", "to": "title"}]}}]}))
    .await;
    let up = |t: &str, k: &str, fields: Value| {
        let (c, t, k) = (c.clone(), ty(t), key(k));
        async move {
            let p = serde_json::from_value(fields).unwrap();
            c.upsert(own, &t, &k, p, WriteOpts::default())
                .await
                .unwrap();
        }
    };
    up("person", "p1", json!({"name": "p1"})).await;
    up("person", "p2", json!({"name": "p2"})).await;
    up(
        "doc",
        "d1",
        json!({"title": "p2", "owner": "p1", "author": "p1"}),
    )
    .await;
    let (sb, d1, p1) = ("signed_by".into(), key("d1"), key("p1"));
    c.link(
        own,
        &ty("doc"),
        &d1,
        &sb,
        &p1,
        Patch::new(),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    // The keys a step from one record reaches, of the other type.
    let walk = |from: &str, k: &str, relation: &str, walk: Walk| {
        let c = c.clone();
        let reached = if from == "doc" {
            "person.key"
        } else {
            "doc.key"
        };
        let step = Step {
            relation: relation.into(),
            walk,
            ty: None,
            filter: None,
            depth: Default::default(),
        };
        let p = Pattern::traverse(ty(from), key(k), vec![step]);
        async move {
            let rows = serde_json::to_value(c.query(own, p).await.unwrap().data).unwrap();
            let rows = rows["rows"].as_array().unwrap().iter();
            rows.map(|r| r[reached].clone()).collect::<Vec<_>>()
        }
    };
    for (by, inverse) in [
        ("written_by", "wrote"),
        ("signed_by", "signed"),
        ("owned_by", "holds"),
    ] {
        let (back, named) = (
            walk("person", "p1", by, Walk::In).await,
            walk("person", "p1", inverse, Walk::Out).await,
        );
        assert_eq!(named, back, "`{inverse}` out from `p1` walks `{by}` in");
        assert_eq!(named, [json!("d1")], "`{inverse}` reaches `d1`");
        let (out, named) = (
            walk("doc", "d1", by, Walk::Out).await,
            walk("doc", "d1", inverse, Walk::In).await,
        );
        assert_eq!(named, out, "`{inverse}` in from `d1` walks `{by}` out");
    }
    // `person.owns` (name = title) wins over `kept_by`'s inverse (owner =
    // name), both ways: the inverse would reach `p1`'s doc and `d1`'s `p1`.
    assert_eq!(walk("person", "p2", "owns", Walk::Out).await, [json!("d1")]);
    assert_eq!(
        walk("person", "p1", "owns", Walk::Out).await,
        [] as [Value; 0]
    );
    assert_eq!(walk("doc", "d1", "owns", Walk::In).await, [json!("p2")]);
}

/// A field declared `on_input_change: clear` is cleared in the same
/// event that changes one of its inputs.
pub async fn a_dependent_field_declared_clear_is_cleared_with_its_input(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let a = &f.cast.auditor;
    f.write(
        a,
        "INV-1",
        &[("verdict", json!("ok")), ("reason", json!("matches PO"))],
    )
    .await;
    let before = f.head().await;
    let (p, _) = applied(&f.write(a, "INV-1", &[("verdict", json!("bad"))]).await);
    assert_eq!(p, before.next(), "one event");
    let r = f.read(a, "INV-1").await;
    assert_eq!(field(&r, "verdict"), Some(&json!("bad")));
    assert_eq!(field(&r, "reason"), None, "the dependent field was cleared");
}

/// Two writers on one record: each writes only its own fields, and
/// neither erases the other's.
pub async fn two_writers_keep_each_others_fields(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    f.write(&f.cast.auditor, "INV-1", &[("verdict", json!("ok"))])
        .await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("held"))])
        .await;
    let r = f.read(&f.cast.owner, "INV-1").await;
    assert_eq!(field(&r, "status"), Some(&json!("held")));
    assert_eq!(field(&r, "verdict"), Some(&json!("ok")));
    assert_eq!(r.version, Version(3));
}

/// `with_meta` returns each field's provenance: who wrote it, from
/// which authority, at which position.
pub async fn per_field_meta_names_the_writer_and_the_position(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (p1, _) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
    let (p2, _) = applied(
        &f.write(&f.cast.auditor, "INV-1", &[("verdict", json!("ok"))])
            .await,
    );
    let r = f
        .catalog
        .get(
            &f.cast.owner,
            &ty(INVOICE),
            Get {
                with_meta: true,
                ..get("INV-1")
            },
        )
        .await
        .unwrap()
        .data;
    let meta: BTreeMap<&str, (&str, Position, Origin)> = r
        .meta
        .iter()
        .map(|(k, m)| (k.as_str(), (m.writer.as_str(), m.position, m.origin)))
        .collect();
    assert_eq!(meta["status"], ("agent:clerk", p1, Origin::Authored));
    assert_eq!(meta["verdict"], ("human:auditor", p2, Origin::Authored));
    assert!(
        r.meta
            .values()
            .all(|m| m.seen_at.is_none() && m.source.is_none())
    );
    assert!(
        r.meta
            .values()
            .all(|m| m.known_at <= r.meta[&FieldName::from("verdict")].known_at)
    );
    let plain = f.read(&f.cast.owner, "INV-1").await;
    assert!(plain.meta.is_empty(), "meta only when asked");
}

/// `apply` checks a set as one, so a reference may point forward within
/// it, and appends it in one go: the answer names the last position,
/// every declaration is in force from there.
pub async fn apply_declares_a_set_in_one_append_with_forward_references(store: Arc<dyn LakeStore>) {
    let catalog = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let before = store.head().await.unwrap();
    // Verbs first, then `invoice`, which refers to `partner`, declared last.
    let mut set = fixture();
    set.reverse();
    let n = set.len() as u64;
    let o = catalog.apply(&cast.owner, set).await.unwrap();
    assert_eq!(
        o.position(),
        Position(before.0 + n),
        "one append, answered at its last position"
    );
    assert_eq!(store.head().await.unwrap(), o.position());
    let d = catalog
        .describe(&cast.owner, ReadAt::default())
        .await
        .unwrap()
        .data;
    // A lake may hold system types of its own from the start.
    let mut names: Vec<&str> = d
        .types
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| !n.starts_with("sys."))
        .collect();
    names.sort();
    assert_eq!(names, [INVOICE, PARTNER]);
    assert_eq!(d.verbs.len(), 2);
}

/// A set with any problem is refused whole, one refusal naming every
/// problem, and nothing of it is appended.
pub async fn apply_refuses_the_whole_set_naming_every_problem(store: Arc<dyn LakeStore>) {
    let catalog = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let before = store.head().await.unwrap();
    let mut set = fixture();
    let ghost: Declaration = serde_json::from_value(json!({
        "declare": "verb", "name": "haunt", "on": "ghost", "risk": "low",
        "effect": "write", "set": []
    }))
    .unwrap();
    set.push(ghost);
    set.push(set[0].clone());
    let e = catalog.apply(&cast.owner, set).await.unwrap_err();
    assert_eq!(
        store.head().await.unwrap(),
        before,
        "nothing of a refused set is appended"
    );
    let message = match &e {
        Error::Refused {
            reason: Refusal::Invalid { message },
            ..
        } => message.clone(),
        other => {
            panic!("a set with two problems is refused as invalid, naming both; got {other:?}")
        }
    };
    // Both problems are named: the verb on an undeclared type, and the
    // type declared twice; how the refusal words them is not the contract.
    assert!(message.contains("haunt"), "{message}");
    assert!(message.contains("partner"), "{message}");
    let d = catalog
        .describe(&cast.owner, ReadAt::default())
        .await
        .unwrap()
        .data;
    assert!(
        d.types.iter().all(|t| t.name.as_str().starts_with("sys.")),
        "no type of the refused set is declared"
    );
}

/// `declare` takes one declaration at a time, so a reference must point
/// to a type the lake already holds.
pub async fn declare_refuses_a_reference_to_an_undeclared_type(store: Arc<dyn LakeStore>) {
    let catalog = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let before = store.head().await.unwrap();
    // `invoice` refers to `partner`, which is not declared yet.
    let invoice = fixture()
        .into_iter()
        .find(|d| matches!(d, Declaration::Type(t) if t.name.as_str() == INVOICE))
        .unwrap();
    let e = catalog
        .declare(&cast.owner, invoice.clone())
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            }
        ),
        "a dangling reference is refused as invalid; got {e:?}"
    );
    assert_eq!(store.head().await.unwrap(), before);
    let partner = fixture()
        .into_iter()
        .find(|d| matches!(d, Declaration::Type(t) if t.name.as_str() == PARTNER))
        .unwrap();
    catalog.declare(&cast.owner, partner).await.unwrap();
    catalog.declare(&cast.owner, invoice).await.unwrap();
}

/// A revision that would reshape stored records is a migration, for
/// relations and views as for fields (027): removing or retargeting a
/// relation, narrowing its cardinality, or turning a stored type into a
/// view is refused by `declare`; widening a cardinality is not.
pub async fn a_relation_removed_retargeted_or_narrowed_or_a_type_turned_view_is_a_migration(
    store: Arc<dyn LakeStore>,
) {
    use lakelands_api::ontology::Cardinality;
    let f = setup(store).await;
    let invoice = fixture()
        .into_iter()
        .find_map(|d| match d {
            Declaration::Type(t) if t.name.as_str() == INVOICE => Some(t),
            _ => None,
        })
        .unwrap();
    let refused_whole = |decl: TypeDecl, what: &'static str| {
        let (cat, owner, store) = (f.catalog.clone(), f.cast.owner.clone(), f.store.clone());
        async move {
            let before = store.head().await.unwrap();
            let e = cat.declare(&owner, Declaration::Type(decl)).await;
            assert!(
                matches!(
                    e,
                    Err(Error::Refused {
                        reason: Refusal::Invalid { .. },
                        ..
                    })
                ),
                "{what} must be refused as a migration; got {e:?}"
            );
            assert_eq!(
                store.head().await.unwrap(),
                before,
                "{what} appended something"
            );
        }
    };
    let mut removed = invoice.clone();
    removed.relations.clear();
    refused_whole(removed, "removing the relation `billed_by`").await;
    let mut retargeted = invoice.clone();
    retargeted.relations[0].target = INVOICE.into();
    refused_whole(retargeted, "retargeting `billed_by` to `invoice`").await;
    let mut narrowed = invoice.clone();
    narrowed.relations[0].cardinality = Cardinality::OneToOne;
    refused_whole(narrowed, "narrowing `billed_by` to one-to-one").await;
    let mut widened = invoice.clone();
    widened.relations[0].cardinality = Cardinality::ManyToMany;
    f.catalog
        .declare(&f.cast.owner, Declaration::Type(widened))
        .await
        .expect("widening `billed_by` to many-to-many is a declaration");

    // A stored type becomes a view with the same fields: still a migration.
    let tally = |view: bool| -> TypeDecl {
        serde_json::from_value(json!({
            "name": "tally",
            "fields": [{"name": "n", "type": {"kind": "number"}}],
            "view": if view {
                json!({"source": INVOICE, "measures": [{"name": "n", "expr": "count()"}]})
            } else {
                Value::Null
            },
        }))
        .unwrap()
    };
    f.catalog
        .declare(&f.cast.owner, Declaration::Type(tally(false)))
        .await
        .unwrap();
    refused_whole(tally(true), "turning the stored type `tally` into a view").await;
}

/// A key field is written like any other: when a create reads it from the
/// key and it is a reference, it must name a live record (the reference
/// rule of lakelands#52 and the key rule of the log format §2.3).
pub async fn a_reference_key_field_filled_from_the_key_must_name_a_live_record(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let assignment: TypeDecl = serde_json::from_value(json!({
        "name": "assignment",
        "key": ["partner"],
        "fields": [
            {"name": "partner", "type": {"kind": "reference", "target": PARTNER}},
            {"name": "note", "type": {"kind": "text"}}
        ]
    }))
    .unwrap();
    f.catalog
        .declare(&f.cast.owner, Declaration::Type(assignment))
        .await
        .unwrap();
    let assignment = ty("assignment");
    let before = f.store.head().await.unwrap();
    let e = f
        .catalog
        .upsert(
            &f.cast.owner,
            &assignment,
            &key("NOBODY"),
            patch(&[("note", json!("x"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::Refused { field: Some(f), .. } if f.as_str() == "partner"),
        "a key naming no partner must be refused on `partner`; got {e:?}"
    );
    assert_eq!(f.store.head().await.unwrap(), before);
    f.catalog
        .upsert(
            &f.cast.owner,
            &ty(PARTNER),
            &key("ACME"),
            patch(&[("name", json!("Acme"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    f.catalog
        .upsert(
            &f.cast.owner,
            &assignment,
            &key("ACME"),
            patch(&[("note", json!("x"))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
}

/// Values are kept, keyed and hashed in their one form (log format §1.2,
/// §1.3, §2.3): a number written `2.0` is `2`, a date `2026-09-24` is
/// `2026-09-24T00:00:00.000000Z`. So a key agrees with key fields written
/// in another spelling, and a source record delivered again in another
/// spelling of the same values appends nothing.
pub async fn key_components_and_source_ids_use_each_values_one_form(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let reading: TypeDecl = serde_json::from_value(json!({
        "name": "reading",
        "key": ["n", "d"],
        "fields": [
            {"name": "n", "type": {"kind": "number"}, "authority": "shared"},
            {"name": "d", "type": {"kind": "date_time"}, "authority": "shared"},
            {"name": "level", "type": {"kind": "number"}, "authority": "shared"}
        ]
    }))
    .unwrap();
    f.catalog
        .declare(&f.cast.owner, Declaration::Type(reading))
        .await
        .unwrap();
    let reading = ty("reading");
    let canonical_key = "2:2026-09-24T00%3A00%3A00.000000Z";
    f.catalog
        .upsert(
            &f.cast.owner,
            &reading,
            &key(canonical_key),
            patch(&[
                ("n", json!(2.0)),
                ("d", json!("2026-09-24")),
                ("level", json!(1)),
            ]),
            WriteOpts::default(),
        )
        .await
        .expect("a key made from the key fields' one form agrees with them");
    let r = f
        .catalog
        .get(&f.cast.owner, &reading, get(canonical_key))
        .await
        .unwrap()
        .data;
    assert_eq!(r.fields.get(&FieldName::from("n")), Some(&json!(2)));
    assert_eq!(
        r.fields.get(&FieldName::from("d")),
        Some(&json!("2026-09-24T00:00:00.000000Z"))
    );

    let deliver = |level: Value| {
        let (cat, owner) = (f.catalog.clone(), f.cast.owner.clone());
        async move {
            let record = SourceRecord {
                upstream: None,
                ty: ty("reading"),
                key: key(canonical_key),
                fields: patch(&[("level", level)]),
                seen_at: None,
            };
            let mut acks = cat
                .ingest(&owner, &"gauge".into(), stream::of(vec![record]))
                .await
                .unwrap();
            stream::collect(&mut acks)
                .await
                .into_iter()
                .map(|a| a.unwrap())
                .fold((0, 0), |(r, d), c| (r + c.records, d + c.duplicates))
        }
    };
    assert_eq!(deliver(json!(5)).await, (1, 0), "a new level is written");
    let head = f.store.head().await.unwrap();
    assert_eq!(
        deliver(json!(5.0)).await,
        (0, 1),
        "5.0 is 5 in its one form, so the redelivery changes nothing"
    );
    assert_eq!(f.store.head().await.unwrap(), head);
}

/// A schema hash is 64 lower-case hex digits and a proposal id 32, and
/// both are the values the log format derives (`docs/log-format.md`):
/// the fixture's invoice type hashes to one digest, a view over it
/// written as text to another (its conditions are hashed parsed), and a proposal is
/// named by its lake and position, or by its writer and idempotency key
/// (the first 16 bytes of SHA-256 over the parts, joined by a zero byte).
pub async fn schema_hashes_and_proposal_ids_have_the_stores_form(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let hex = |s: &str, n: usize| {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let described = f
        .catalog
        .describe_type(&f.cast.owner, &ty(INVOICE), ReadAt::default())
        .await
        .unwrap();
    let hash = serde_json::to_value(&described.data).unwrap()["lifecycle"]["schema_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(hex(&hash, 64), "a schema hash is 64 hex digits: {hash}");
    assert_eq!(
        hash, "b63567bce0ec4046d82237cc2fab92986b6a01b1a58afeb0a7b5beb29b0a4693",
        "the fixture's invoice type hashes to the same digest everywhere"
    );
    // A view is hashed over its conditions' typed form, so a view written
    // as text hashes to the one digest too.
    let open_by_partner = TypeDecl {
        annotations: Default::default(),
        name: "open_by_partner".into(),
        key: vec!["partner".into()],
        fields: vec![
            text("partner"),
            FieldDecl {
                ty: FieldType::kind(StorageKind::Decimal),
                ..text("total")
            },
            FieldDecl {
                ty: FieldType::kind(StorageKind::Number),
                ..text("late")
            },
        ],
        relations: Vec::new(),
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            r#where: Some("status != 'paid'  and not (amount is null)".into()),
            group_by: vec!["partner".into()],
            measures: vec![
                Measure {
                    name: "total".into(),
                    expr: "sum(amount)".into(),
                },
                Measure {
                    name: "late".into(),
                    expr: "count() * -1.50".into(),
                },
            ],
            params: Default::default(),
            merge: Default::default(),
        })),
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(&f.cast.owner, Declaration::Type(open_by_partner.clone()))
        .await
        .expect("the owner declares a view");
    let described = f
        .catalog
        .describe_type(&f.cast.owner, &ty("open_by_partner"), ReadAt::default())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&described.data).unwrap()["lifecycle"]["schema_hash"],
        "9d89b4ed75c087e803c3e7e8265443c00af7af43a32ad629b02e0f6da7414032",
        "a view type hashes over its conditions' typed form, to the same digest everywhere"
    );
    // A report's id is the SHA-256 of the report, its id left empty, and
    // the head, in canonical form.
    let tally = |fields: Vec<FieldDecl>| TypeDecl {
        name: "tally".into(),
        key: Vec::new(),
        fields,
        view: None,
        ..open_by_partner.clone()
    };
    let owner = &f.cast.owner;
    let declared = f
        .catalog
        .declare(owner, Declaration::Type(tally(vec![text("n")])));
    declared.await.expect("the owner declares a type");
    let m = Migration {
        to: tally(vec![text("n"), text("note")]),
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let preview = f.catalog.migrate(owner, m).await.expect("a preview");
    let report = MigrationReport {
        report_id: String::new(),
        ..preview.data.clone()
    };
    let made = json!({"report": report, "head": preview.position.0});
    assert_eq!(
        preview.data.report_id,
        lakelands_api::canonical::sha256_hex(&made),
        "a report's id is the SHA-256 of the report and the head"
    );

    f.write(&f.cast.owner, "INV-1", &[("status", json!("approved"))])
        .await;
    let propose = |k: Option<&str>| {
        let (cat, clerk) = (f.catalog.clone(), f.cast.clerk.clone());
        let opts = WriteOpts {
            idempotency_key: k.map(Into::into),
            ..WriteOpts::default()
        };
        async move {
            match cat
                .call(&clerk, &"pay".into(), Some(&key("INV-1")), patch(&[]), opts)
                .await
                .expect("pay is proposed")
            {
                Outcome::Proposed {
                    position, proposal, ..
                } => (position, proposal.0),
                other => panic!("expected a proposal, got {other:?}"),
            }
        }
    };
    // The first 16 bytes of SHA-256 over the parts, joined by a zero byte.
    let id_of = |parts: &[&[u8]]| -> String {
        use sha2::{Digest as _, Sha256};
        let mut h = Sha256::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                h.update([0u8]);
            }
            h.update(p);
        }
        h.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let (at, by_position) = propose(None).await;
    assert!(
        hex(&by_position, 32),
        "a proposal id is 32 hex digits: {by_position}"
    );
    assert_eq!(
        by_position,
        id_of(&[
            b"lakelands:event:v1",
            f.store.lake().as_str().as_bytes(),
            at.0.to_string().as_bytes(),
        ]),
        "a proposal without a key is named by its lake and position"
    );
    let (_, by_key) = propose(Some("pay-once")).await;
    assert_eq!(
        by_key,
        id_of(&[
            b"lakelands:authored:v1",
            f.cast.clerk.writer().as_bytes(),
            b"pay-once",
        ]),
        "a proposal with a key is named by its writer and key"
    );
    assert_eq!(
        propose(Some("pay-once")).await.1,
        by_key,
        "a retry under the same key names the same proposal"
    );
}

/// A reader kept from a field, or from a relation's target, is answered
/// the hash of the declaration it is described, in `describe` and in its
/// preview, and its preview's id is of what it is shown
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended).
pub async fn a_reader_is_answered_the_hash_of_the_shape_it_is_described(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    // The architect is kept from `secret`, the stranger from `partner`, the
    // relation's target: each is answered the hash of what it is described.
    let mut seen = Vec::new();
    for who in [&f.cast.owner, &f.cast.architect, &f.cast.stranger] {
        let d = (f
            .catalog
            .describe_type(who, &ty(INVOICE), ReadAt::default())
            .await)
            .unwrap()
            .data;
        let field = |f: &lakelands_api::describe::FieldDescription| json!({"name": f.name, "type": f.ty, "required": f.required});
        let fields: Vec<_> = d.fields.iter().map(field).collect();
        let t = json!({"name": d.name, "key": d.access.key, "fields": fields,
            "relations": d.relations, "view": d.view});
        let t: TypeDecl = serde_json::from_value(t).unwrap();
        let rebuilt = lakelands_api::shape::schema_hash(&t).unwrap();
        assert_eq!(
            d.lifecycle.schema_hash, rebuilt,
            "{} is answered the hash of what it is described",
            who.id
        );
        seen.push((rebuilt, t));
    }
    let hash = "b63567bce0ec4046d82237cc2fab92986b6a01b1a58afeb0a7b5beb29b0a4693";
    assert_eq!(seen[0].0, hash, "the owner is answered the whole hash");
    assert_ne!(
        seen[1].0, hash,
        "the architect's shape is not the whole one"
    );
    // Its preview of the shape it is described: both hashes are that one's,
    // and its id is not the owner's.
    let to = Migration {
        to: seen[1].1.clone(),
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    // The owner's is the same migration, `secret` as the architect's is put back.
    let mut whole = to.clone();
    let secret = seen[0]
        .1
        .fields
        .iter()
        .find(|x| x.name.as_str() == "secret");
    whole.to.fields.extend(secret.cloned());
    let shown = f.catalog.migrate(&f.cast.architect, to).await.unwrap().data;
    let whole = f.catalog.migrate(&f.cast.owner, whole).await.unwrap().data;
    assert_eq!(
        (&shown.records_scanned, &shown.restated, shown.more),
        (&whole.records_scanned, &whole.restated, false),
        "the architect and the owner preview one migration"
    );
    assert_eq!(
        (&shown.from_schema_hash, &shown.to_schema_hash),
        (&seen[1].0, &seen[1].0),
        "the architect's preview is answered the hashes of its shape"
    );
    assert_ne!(
        shown.report_id, whole.report_id,
        "the architect's id is of what it is shown"
    );
}

/// A relation carried by link records (`docs/decisions/035-a-link-is-a-record.md`):
/// a link is written and walked, one per pair; the relation's cardinality
/// holds at the door and in an ingest window; its records refuse the
/// ordinary doors and `propose`; a principal who may not write the link
/// type is not granted; and unlink removes it.
pub async fn links_are_written_walked_and_held_to_their_relation(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    c.declare(
        own,
        decl(json!({"declare": "type", "name": "person", "fields": [
            {"name": "name", "type": {"kind": "text"}}]})),
    )
    .await
    .unwrap();
    c.declare(
        own,
        decl(json!({"declare": "type", "name": "doc", "fields": [
            {"name": "title", "type": {"kind": "text"}}],
            "relations": [
                {"name": "signed_by", "target": "person", "cardinality": "many_to_one",
                 "fields": [{"name": "at", "type": {"kind": "text"}}]},
                {"name": "witnessed_by", "target": "person", "cardinality": "many_to_one",
                 "authority": "source"}]})),
    )
    .await
    .unwrap();
    for (t, k, f) in [
        ("person", "p1", "name"),
        ("person", "p2", "name"),
        ("doc", "d1", "title"),
    ] {
        c.upsert(
            own,
            &ty(t),
            &key(k),
            patch(&[(f, json!(k))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    let link = |to: &str, at: &str| {
        let (c, own, to, at) = (c.clone(), own.clone(), key(to), at.to_owned());
        async move {
            c.link(
                &own,
                &ty("doc"),
                &key("d1"),
                &"signed_by".into(),
                &to,
                patch(&[("at", json!(at))]),
                WriteOpts::default(),
            )
            .await
        }
    };
    link("p1", "monday").await.unwrap();
    let again = link("p1", "tuesday").await.unwrap();
    assert!(
        matches!(again, Outcome::Applied { version: Some(v), .. } if v == Version(2)),
        "writing a link again changes it: {again:?}"
    );
    let walked = c
        .query(
            own,
            Pattern::traverse(
                ty("doc"),
                key("d1"),
                vec![Step {
                    relation: "signed_by".into(),
                    walk: Walk::Out,
                    ty: None,
                    filter: None,
                    depth: Default::default(),
                }],
            ),
        )
        .await
        .unwrap();
    let json = serde_json::to_value(&walked.data).unwrap();
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(1), "{json}");
    assert_eq!(json["rows"][0]["person.name"], json!("p1"), "{json}");
    // many_to_one: d1 links to one signer.
    let second = link("p2", "later").await;
    assert!(
        matches!(
            &second,
            Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            })
        ),
        "{second:?}"
    );
    // The link type's records refuse the ordinary doors and propose.
    let lt = ty("sys.link.doc.signed_by");
    let upserted = c
        .upsert(
            own,
            &lt,
            &key("d1:p1"),
            patch(&[("at", json!("x"))]),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(
            &upserted,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "{upserted:?}"
    );
    let proposed = c
        .propose(
            own,
            lakelands_api::write::ProposedWrite::Retire {
                ty: lt.clone(),
                key: key("d1:p1"),
                opts: WriteOpts::default(),
            },
            "tidy".into(),
        )
        .await;
    assert!(
        matches!(
            &proposed,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "{proposed:?}"
    );
    // Who is not shown the type that declares the relation is answered as
    // for a type that is not there (006, extended).
    let stranger = c
        .link(
            &cast.stranger,
            &ty("doc"),
            &key("d1"),
            &"signed_by".into(),
            &key("p1"),
            Patch::new(),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(
            &stranger,
            Err(Error::NotFound {
                missing: Missing::Type { .. }
            })
        ),
        "{stranger:?}"
    );
    // Who may not write the link type is refused as not granted, and so is
    // a link that would break the cardinality, by a principal that reads
    // the links but may not write them: the write's grants come before
    // the links are read for the cardinality (024, extended).
    let past = c
        .link(
            &cast.auditor,
            &ty("doc"),
            &key("d1"),
            &"signed_by".into(),
            &key("p2"),
            Patch::new(),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(
            &past,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "{past:?}"
    );
    // A source relation's links arrive by ingest, its cardinality held in
    // one window as at the door.
    let window = vec![
        SourceRecord {
            upstream: None,
            ty: ty("sys.link.doc.witnessed_by"),
            key: key("d1:p1"),
            fields: patch(&[("from", json!("d1")), ("to", json!("p1"))]),
            seen_at: None,
        },
        SourceRecord {
            upstream: None,
            ty: ty("sys.link.doc.witnessed_by"),
            key: key("d1:p2"),
            fields: patch(&[("from", json!("d1")), ("to", json!("p2"))]),
            seen_at: None,
        },
    ];
    let mut commits = c
        .ingest(own, &"registry".into(), stream::of(window))
        .await
        .unwrap();
    let first = crate::stream::next(&mut commits).await;
    assert!(
        matches!(
            &first,
            Some(Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            }))
        ),
        "two witnesses for d1 in one window break many_to_one: {first:?}"
    );
    // Unlink removes the link; the walk reaches no one.
    c.unlink(
        own,
        &ty("doc"),
        &key("d1"),
        &"signed_by".into(),
        &key("p1"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let after = c
        .query(
            own,
            Pattern::traverse(
                ty("doc"),
                key("d1"),
                vec![Step {
                    relation: "signed_by".into(),
                    walk: Walk::Out,
                    ty: None,
                    filter: None,
                    depth: Default::default(),
                }],
            ),
        )
        .await
        .unwrap();
    let json = serde_json::to_value(&after.data).unwrap();
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(0), "{json}");
}

/// A migration's shape is its author's view of the type: a field the
/// author may not know of stays, with its values, when the migration is
/// proposed and accepted; `dropped` lists what the author removed; and the
/// proposer reads its own proposal without the field, which a reviewer who
/// may see it reads there
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
/// extended).
pub async fn a_migration_keeps_a_field_its_author_may_not_know_of(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    // The clerk reads invoices but may not know of `secret`.
    let (own, clerk) = (&cast.owner, &cast.clerk);
    let invoice = |fields: &[&str]| {
        let fields: Vec<Value> = (fields.iter())
            .map(|f| json!({"name": f, "type": {"kind": "text"}}))
            .collect();
        let d = json!({"declare": "type", "name": INVOICE, "fields": fields});
        match serde_json::from_value::<Declaration>(d).unwrap() {
            Declaration::Type(t) => t,
            _ => unreachable!(),
        }
    };
    let whole = invoice(&["status", "memo", "secret"]);
    c.declare(own, Declaration::Type(whole)).await.unwrap();
    let all = [
        ("status", json!("open")),
        ("memo", json!("m")),
        ("secret", json!("9")),
    ];
    c.upsert(
        own,
        &ty(INVOICE),
        &key("i1"),
        patch(&all),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let write = ProposedWrite::Migrate {
        migration: Box::new(lakelands_api::write::Migration {
            to: invoice(&["status"]),
            renames: vec![],
            relation_renames: Vec::new(),
        }),
        report: Default::default(),
    };
    let proposed = c.propose(clerk, write, "no memos".into()).await.unwrap();
    let Outcome::Proposed { proposal, .. } = proposed else {
        panic!("a proposed migration answers `proposed`: {proposed:?}")
    };
    let mut read = Vec::new();
    for who in [clerk, own] {
        let listed = c.proposals(who, ProposalFilter::default()).await.unwrap();
        let mine = listed.data.into_iter().find(|p| p.id == proposal).unwrap();
        let Some(ProposedWrite::Migrate { migration, report }) = mine.write else {
            panic!("the proposal is a migration")
        };
        let names: Vec<String> = (migration.to.fields.iter())
            .map(|f| f.name.to_string())
            .collect();
        read.push((names, report.dropped));
    }
    let memo = vec![(FieldName::from("memo"), 1)];
    assert_eq!(read[0].0, ["status"], "the proposer reads its own view");
    assert_eq!(read[1], (vec!["status".into(), "secret".into()], memo));
    c.accept(own, &proposal).await.unwrap();
    let i1 = c.get(own, &ty(INVOICE), get("i1")).await.unwrap().data;
    assert_eq!(
        i1.fields.get("secret"),
        Some(&json!("9")),
        "{:?}",
        i1.fields
    );
    assert!(!i1.fields.contains_key("memo"), "{:?}", i1.fields);
}

/// A relation carried by link records, dropped by a migration and
/// declared again with another target, would reshape the links it kept:
/// the declaration is refused, naming the link type, as a migration
/// (`docs/decisions/035-a-link-is-a-record.md`, extended).
pub async fn a_relation_declared_again_with_another_target_reshapes_its_links(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    let doc = |relations: Value| {
        decl(json!({"declare": "type", "name": "doc",
                    "fields": [{"name": "title", "type": {"kind": "text"}}],
                    "relations": relations}))
    };
    for d in [
        decl(json!({"declare": "type", "name": "person",
                    "fields": [{"name": "name", "type": {"kind": "text"}}]})),
        decl(json!({"declare": "type", "name": "team",
                    "fields": [{"name": "name", "type": {"kind": "text"}}]})),
        doc(json!([{"name": "signed_by", "target": "person"}])),
    ] {
        c.declare(own, d).await.unwrap();
    }
    for (t, k) in [("person", "p1"), ("doc", "d1")] {
        let f = if t == "doc" { "title" } else { "name" };
        c.upsert(
            own,
            &ty(t),
            &key(k),
            patch(&[(f, json!(k))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    c.link(
        own,
        &ty("doc"),
        &key("d1"),
        &"signed_by".into(),
        &key("p1"),
        Patch::new(),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    // The relation dropped: a migration, which keeps the links.
    let Declaration::Type(without) = doc(json!([])) else {
        unreachable!()
    };
    let m = lakelands_api::write::Migration {
        to: without,
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m.clone()).await.unwrap();
    assert_eq!(
        report.data.dropped,
        [("signed_by".into(), 1)],
        "the preflight lists the relation with its one live link"
    );
    c.migrate_apply(own, m, &report.data.report_id)
        .await
        .unwrap();
    // Declared again, pointing at teams: refused, naming the link type.
    let refused = c
        .declare(own, doc(json!([{"name": "signed_by", "target": "team"}])))
        .await;
    assert!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
            if message.contains("sys.link.doc.signed_by")),
        "{refused:?}"
    );
}

/// A migration's report restates what its `to` states otherwise than the
/// declaration in force and lists a removed relation that holds no link;
/// and an accept does not take away a mark set since the proposal
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
/// extended).
pub async fn a_migration_restates_what_its_to_states_otherwise(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let badge = |fields: Value, relations: Value| json!({"declare": "type", "name": "badge", "fields": fields, "relations": relations});
    let mut secret = text("secret");
    (secret["sensitive"], secret["label"]) = (json!(true), json!("Secret"));
    let pair = json!({"match": [{"from": "nick", "to": "nick"}]});
    let twin = json!([{"name": "twin", "target": "badge", "derived": pair}]);
    let mut declared = badge(json!([secret, text("nick")]), twin);
    let declare = |d: &Value| c.declare(own, serde_json::from_value(d.clone()).unwrap());
    declare(&declared).await.unwrap();
    let mut to = badge(json!([text("secret"), text("alias")]), json!([]));
    to.as_object_mut().unwrap().remove("declare");
    let bare = lakelands_api::write::Migration {
        to: serde_json::from_value(to).unwrap(),
        renames: vec![("nick".into(), "alias".into())],
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, bare.clone()).await.unwrap().data;
    let entry = |field: &str, member: &str, was: Value, to: Value| json!({"field": field, "member": member, "was": was, "to": to});
    let restated = json!([
        entry("secret", "label", json!("Secret"), json!(null)),
        entry("secret", "sensitive", json!(true), json!(false)),
        entry("alias", "name", json!("nick"), json!("alias")),
    ]);
    assert_eq!(json!(report.restated), restated);
    assert_eq!(report.dropped, [("twin".into(), 0)]);
    assert!(report.lossless && report.fields.is_empty(), "{report:?}");
    // Proposed, and `nick` declared sensitive since: the accept is refused.
    let (migration, report) = (Box::new(bare), Default::default());
    let write = ProposedWrite::Migrate { migration, report };
    let proposed = c.propose(own, write, "why".into()).await.unwrap();
    let Outcome::Proposed { proposal, .. } = proposed else {
        panic!("a proposal is due: {proposed:?}");
    };
    declared["fields"][1]["sensitive"] = json!(true);
    declare(&declared).await.unwrap();
    let stale = c.accept(own, &proposal).await;
    assert!(
        matches!(&stale, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
            if message.contains("changed since")),
        "{stale:?}"
    );
}

/// One who knows of a field and does not read it is answered its shape
/// alone of a migration and of a proposed declaration: `sensitive`, and
/// no label, in its own preflight, in a refusal's report and in the `to`
/// and the type a listing hands it
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended).
pub async fn a_migration_shows_a_field_by_its_shape_to_one_who_does_not_read_it(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let field = |n: &str, kind: &str, label: &str, sensitive: bool| json!({"name": n, "type": {"kind": kind}, "label": label, "sensitive": sensitive});
    let badge = |fields: Value| json!({"name": "badge", "fields": fields});
    let secret = field("secret", "text", "Secret", true);
    let mut declared = badge(json!([secret, field("nick", "text", "Nick", false)]));
    declared["declare"] = json!("type");
    let declare = |d: &Value| serde_json::from_value::<Declaration>(d.clone()).unwrap();
    c.declare(own, declare(&declared)).await.unwrap();
    // `secret` loses its mark and takes another label; `nick` would not convert.
    let hidden = field("secret", "text", "Hidden", false);
    let to = badge(json!([hidden, field("nick", "number", "Nick", false)]));
    let m = lakelands_api::write::Migration {
        to: serde_json::from_value(to).unwrap(),
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let more = json!({"name": "more", "type": {"kind": "text"}});
    declared["fields"].as_array_mut().unwrap().push(more);
    let mut ids = Vec::new();
    let (migration, report) = (Box::new(m.clone()), Default::default());
    let migrate = ProposedWrite::Migrate { migration, report };
    let declarations = vec![declare(&declared)];
    for write in [migrate, ProposedWrite::Declare { declarations }] {
        let proposed = c.propose(own, write, "why".into()).await.unwrap();
        let Outcome::Proposed { proposal, .. } = proposed else {
            panic!("a proposal is due: {proposed:?}");
        };
        ids.push(proposal);
    }
    let listed = |who: Principal| {
        let c = store.catalog(WriteContext::default());
        async move {
            let all = c.proposals(&who, ProposalFilter::default()).await;
            json!(all.unwrap().data).to_string()
        }
    };
    let mark = "{\"field\":\"secret\",\"member\":\"sensitive\",\"to\":false,\"was\":true}";
    let whole = listed(own.clone()).await;
    let labels = ["\"Secret\"", "\"Hidden\""];
    assert!(labels.iter().all(|l| whole.contains(l)), "{whole}");
    let readers = [Access::Masked, Access::WriteOnly].map(|access| {
        let fields = [("secret".into(), access)].into();
        let default = None;
        let types = [(
            "badge".into(),
            TypeGrants {
                default,
                fields,
                ..Default::default()
            },
        )]
        .into();
        let (default, review, schema) = (Access::Read, true, true);
        let rest = Grants::default();
        let grants = Grants {
            default,
            types,
            review,
            schema,
            ..rest
        };
        principal(store.lake(), PrincipalKind::Agent, "unread", grants)
    });
    let mut answers = Vec::new();
    for who in &readers {
        let own = c.migrate(who, m.clone()).await.unwrap().data;
        answers.push(("the preview", json!(own).to_string()));
    }
    // Written since: a `nick` that is no number, so an accept is refused.
    let nick = patch(&[("nick", json!("x"))]);
    c.upsert(own, &ty("badge"), &key("b1"), nick, WriteOpts::default())
        .await
        .unwrap();
    for who in readers {
        let refused = c.accept(&who, &ids[0]).await;
        let Err(e) = &refused else {
            panic!("a refusal is due: {refused:?}");
        };
        answers.push(("the refusal", json!(refusal(e).1).to_string()));
        answers.push(("the listing", listed(who).await));
    }
    let shape = |a: &str| a.contains(mark) && a.contains("\"more\":true");
    let whole = |a: &str| labels.iter().any(|l| a.contains(l));
    answers.retain(|(_, a)| !shape(a) || whole(a));
    assert!(answers.is_empty(), "the shape alone is due in {answers:?}");
}

/// A migration that changes a relation's fields migrates that relation's
/// link type, and one that adds a relation carried by link records
/// declares its link type, after the type's own `migrated` and in the
/// same append; a link over the new relation is then written.
pub async fn a_migration_migrates_and_declares_its_link_types_in_its_append(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let doc = |relations: Value| {
        serde_json::from_value::<TypeDecl>(json!({"name": "doc",
            "fields": [{"name": "title", "type": {"kind": "text"}}], "relations": relations}))
        .unwrap()
    };
    let person: Declaration = serde_json::from_value(json!({"declare": "type", "name": "person",
        "fields": [{"name": "name", "type": {"kind": "text"}}]}))
    .unwrap();
    c.declare(own, person).await.unwrap();
    let signed = json!({"name": "signed_by", "target": "person"});
    c.declare(own, Declaration::Type(doc(json!([signed]))))
        .await
        .unwrap();
    for (t, k, f) in [("person", "p1", "name"), ("doc", "d1", "title")] {
        c.upsert(
            own,
            &ty(t),
            &key(k),
            patch(&[(f, json!(k))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    let m = lakelands_api::write::Migration {
        to: doc(json!([
            {"name": "signed_by", "target": "person",
             "fields": [{"name": "at", "type": {"kind": "text"}}]},
            {"name": "witnessed_by", "target": "person"}])),
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m.clone()).await.unwrap();
    let from = store.head().await.unwrap().next();
    c.migrate_apply(own, m, &report.data.report_id)
        .await
        .unwrap();
    let feed = store.feed();
    let b = feed.poll(from, &FeedFilter::default(), 100).await.unwrap();
    let appended: Vec<(&str, String)> = b
        .events
        .iter()
        .map(|e| match &e.body {
            EventBody::Migrated { migration, .. } => ("migrated", migration.to.name.to_string()),
            EventBody::Declared {
                declaration: Declaration::Type(t),
                ..
            } => ("declared", t.name.to_string()),
            other => (other.kind(), String::new()),
        })
        .collect();
    assert_eq!(
        appended,
        [
            ("migrated", "doc".to_owned()),
            ("migrated", "sys.link.doc.signed_by".to_owned()),
            ("declared", "sys.link.doc.witnessed_by".to_owned()),
        ]
    );
    c.link(
        own,
        &ty("doc"),
        &key("d1"),
        &"witnessed_by".into(),
        &key("p1"),
        Patch::new(),
        WriteOpts::default(),
    )
    .await
    .expect("a link over the relation the migration added");
}

/// An aging rule is checked when it is written; `age` is handed a date at
/// midnight, moves what holds `from` where `when` holds, as the system,
/// and a date aged twice is written once
/// (`docs/decisions/036-aging-runs-by-date-once.md`).
pub async fn an_aging_rule_moves_what_holds_from_once_a_date(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let invoice: Declaration = serde_json::from_value(json!({"declare": "type", "name": "invoice",
        "fields": [{"name": "status", "type": {"kind": "text"}},
                   {"name": "amount", "type": {"kind": "number"}},
                   {"name": "due_on", "type": {"kind": "date_time"}}]}))
    .unwrap();
    c.declare(own, invoice).await.unwrap();
    // `today()` is the date aged: i1 fell due sixty days before it, i2
    // fifteen, and i3 is paid.
    for (k, status, due) in [
        ("i1", "open", "2026-01-01T00:00:00Z"),
        ("i2", "open", "2026-02-15T00:00:00Z"),
        ("i3", "paid", "2026-01-01T00:00:00Z"),
    ] {
        c.upsert(
            own,
            &ty("invoice"),
            &key(k),
            patch(&[
                ("status", json!(status)),
                ("amount", json!(200)),
                ("due_on", json!(due)),
            ]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    let rule_type = ty(lakelands_api::seam::AGING_RULE_TYPE);
    // A rule that does not fit is refused, naming the rule field at fault.
    let refused = c
        .upsert(
            own,
            &rule_type,
            &key("bad"),
            patch(&[
                ("on", json!("invoice")),
                ("field", json!("amount")),
                ("from", json!("open")),
                ("to", json!(1)),
                ("when", json!("true")),
            ]),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(&refused, Err(Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } })
            if f.as_str() == "from"),
        "{refused:?}"
    );
    // A rule carries its author's rights: one who may write rules but not
    // `status` is refused, naming `status`.
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    grants.types.insert(
        rule_type.clone(),
        TypeGrants {
            default: Some(Access::Write),
            fields: BTreeMap::new(),
            ..Default::default()
        },
    );
    let rule_writer = Principal {
        kind: PrincipalKind::Agent,
        id: "rule-writer".into(),
        lake: store.lake().clone(),
        grants,
    };
    let refused = c
        .upsert(
            &rule_writer,
            &rule_type,
            &key("overdue"),
            patch(&[
                ("on", json!("invoice")),
                ("field", json!("status")),
                ("from", json!("open")),
                ("to", json!("overdue")),
                ("when", json!("amount > 100")),
            ]),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(&refused, Err(Error::Refused { field: Some(f), reason: Refusal::NotGranted })
            if f.as_str() == "status"),
        "{refused:?}"
    );
    // Proposing it is checked the same way: an accept cannot bring it in.
    let proposed = c
        .propose(
            &rule_writer,
            lakelands_api::write::ProposedWrite::Upsert {
                ty: rule_type.clone(),
                key: key("overdue"),
                patch: patch(&[
                    ("on", json!("invoice")),
                    ("field", json!("status")),
                    ("from", json!("open")),
                    ("to", json!("overdue")),
                    ("when", json!("amount > 100")),
                ]),
                opts: WriteOpts::default(),
            },
            "please".into(),
        )
        .await;
    assert!(
        matches!(&proposed, Err(Error::Refused { field: Some(f), reason: Refusal::NotGranted })
            if f.as_str() == "status"),
        "{proposed:?}"
    );
    c.upsert(
        own,
        &rule_type,
        &key("overdue"),
        patch(&[
            ("on", json!("invoice")),
            ("field", json!("status")),
            ("from", json!("open")),
            ("to", json!("overdue")),
            ("when", json!("due_on < today() - 30d")),
        ]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let day = lakelands_api::Timestamp::from_date(2026, 3, 1).unwrap();
    let noon = lakelands_api::Timestamp(day.0 + 12 * 3_600 * 1_000_000);
    let at_noon = store.age(noon).await;
    assert!(
        matches!(&at_noon, Err(Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } })
            if f.as_str() == "today"),
        "{at_noon:?}"
    );
    let report = store.age(day).await.unwrap();
    assert_eq!((report.transitions, report.failed_guards), (1, 0));
    assert_eq!(report.rules.len(), 1);
    assert_eq!(report.rules[0].rule, "overdue");
    assert_eq!(report.rules[0].version, Version(1));
    assert_eq!(report.position, store.head().await.unwrap());
    let status = |k: &'static str| {
        let c = c.clone();
        let own = own.clone();
        async move {
            let r = c.get(&own, &ty("invoice"), get(k)).await.unwrap().data;
            r.fields.get("status").cloned()
        }
    };
    assert_eq!(status("i1").await, Some(json!("overdue")));
    assert_eq!(status("i2").await, Some(json!("open")));
    assert_eq!(status("i3").await, Some(json!("paid")));
    // Set back by hand: the day's move is in the lake, so the same date
    // does not move it again.
    c.upsert(
        own,
        &ty("invoice"),
        &key("i1"),
        patch(&[("status", json!("open"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let head = store.head().await.unwrap();
    let again = store.age(day).await.unwrap();
    assert_eq!((again.transitions, again.position), (0, head));
    assert_eq!(
        store.head().await.unwrap(),
        head,
        "a date aged twice is written once"
    );
    let moved = store
        .feed()
        .poll(Position::GENESIS, &FeedFilter::default(), 1000)
        .await
        .unwrap()
        .events
        .into_iter()
        .find(|e| {
            e.procedure
                .as_ref()
                .is_some_and(|p| p.name == "aging:overdue")
        })
        .unwrap();
    assert_eq!(
        (
            moved.origin,
            moved.writer.as_str(),
            moved.context.origin,
            moved.cause
        ),
        (
            Origin::System,
            "system:lakelands",
            lakelands_api::WriteOrigin::Scheduled,
            None
        )
    );
}

/// A preflight appends nothing and says what the purge would retire and
/// which live records outside it refer into it; a purge that repeats its
/// id is refused once the lake has moved, as not of this purge at this
/// head, and purges under a fresh one, naming the same records
/// (`docs/decisions/039-a-source-is-retracted-as-one.md`, extended).
pub async fn a_purge_preflight_names_what_the_purge_strands_and_its_id_goes_stale(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    for d in [
        json!({"declare": "type", "name": "partner",
            "fields": [{"name": "name", "type": {"kind": "text"}, "authority": "source"}]}),
        json!({"declare": "type", "name": "bill",
            "fields": [{"name": "partner_id", "type": {"kind": "text"}}],
            "relations": [{"name": "billed_to", "target": "partner", "via": "partner_id"},
                          {"name": "seen_by", "target": "partner", "cardinality": "many_to_many"}]}),
    ] {
        c.declare(own, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let p1 = SourceRecord {
        upstream: None,
        ty: ty("partner"),
        key: key("p1"),
        fields: patch(&[("name", json!("Harbor"))]),
        seen_at: None,
    };
    let mut acks = c
        .ingest(own, &"crm".into(), stream::of(vec![p1]))
        .await
        .unwrap();
    stream::collect(&mut acks).await;
    let bill = |k: &'static str, partner: &'static str| {
        let (c, own) = (c.clone(), own.clone());
        let fields = patch(&[("partner_id", json!(partner))]);
        async move {
            c.upsert(&own, &ty("bill"), &key(k), fields, WriteOpts::default())
                .await
                .unwrap();
        }
    };
    bill("b1", "p1").await;
    bill("b2", "p9").await;
    // An authored link from `b2` to `p1` is stranded as `b1`'s field is.
    c.link(
        own,
        &ty("bill"),
        &key("b2"),
        &"seen_by".into(),
        &key("p1"),
        patch(&[]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let head = store.head().await.unwrap();
    let seen = store.purge_preflight(&"crm".into(), None).await.unwrap();
    let named = |r: &lakelands_api::write::PurgeReport| -> Vec<(String, u64, Vec<String>)> {
        (r.references.iter())
            .map(|f| {
                let first = f.first.iter().map(|k| format!("{} {}", k.ty, k.key));
                (
                    format!("{}.{}", f.ty, f.relation),
                    f.records,
                    first.collect(),
                )
            })
            .collect()
    };
    let want = vec![
        ("bill.billed_to".to_owned(), 1, vec!["bill b1".to_owned()]),
        (
            "bill.seen_by".to_owned(),
            1,
            vec!["sys.link.bill.seen_by b2:p1".to_owned()],
        ),
    ];
    assert_eq!((seen.retired, named(&seen)), (1, want.clone()), "{seen:?}");
    assert_eq!(
        (store.head().await.unwrap(), seen.position),
        (head, head),
        "a preflight appends nothing"
    );
    assert!(!seen.report_id.is_empty());
    bill("b3", "p1").await;
    let head = store.head().await.unwrap();
    let stale = store
        .purge_source(&"crm".into(), None, Some(&seen.report_id))
        .await;
    assert!(
        matches!(&stale, Err(Error::Refused { field: None, reason: Refusal::Invalid { message } })
            if message.contains("not of this purge at this head")),
        "{stale:?}"
    );
    assert_eq!(
        store.head().await.unwrap(),
        head,
        "a stale purge appends nothing"
    );
    let again = store.purge_preflight(&"crm".into(), None).await.unwrap();
    let done = store
        .purge_source(&"crm".into(), None, Some(&again.report_id))
        .await
        .unwrap();
    assert_eq!((done.retired, named(&done)), (1, named(&again)), "{done:?}");
    assert_eq!(
        named(&done)[0].1,
        2,
        "`b3` refers into the purge too: {done:?}"
    );
}

/// A purge retires every live record one source brought in, in one
/// append; a second purge appends nothing; a record a principal also
/// wrote refuses the whole purge, naming it
/// (`docs/decisions/039-a-source-is-retracted-as-one.md`).
pub async fn a_purge_retires_what_one_source_brought_in_and_refuses_a_shared_record(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let partner: Declaration = serde_json::from_value(json!({"declare": "type", "name": "partner",
        "fields": [{"name": "name", "type": {"kind": "text"}, "authority": "source"},
                   {"name": "city", "type": {"kind": "text"}, "authority": "source"},
                   {"name": "note", "type": {"kind": "text"}}]}))
    .unwrap();
    c.declare(own, partner).await.unwrap();
    let feed = |keys: &'static [&'static str]| {
        let c = c.clone();
        let own = own.clone();
        async move {
            let records = keys
                .iter()
                .map(|k| SourceRecord {
                    upstream: None,
                    ty: ty("partner"),
                    key: key(k),
                    fields: patch(&[("name", json!(k))]),
                    seen_at: None,
                })
                .collect();
            let mut acks = c
                .ingest(&own, &"crm".into(), stream::of(records))
                .await
                .unwrap();
            stream::collect(&mut acks).await;
        }
    };
    feed(&["p1", "p2"]).await;
    c.upsert(
        own,
        &ty("partner"),
        &key("p3"),
        patch(&[("note", json!("ours"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let report = store.purge_source(&"crm".into(), None, None).await.unwrap();
    assert_eq!(report.retired, 2, "{report:?}");
    assert_eq!(report.position, store.head().await.unwrap());
    let live = |k: &'static str| {
        let c = c.clone();
        let own = own.clone();
        async move { c.get(&own, &ty("partner"), get(k)).await.is_ok() }
    };
    assert!(!live("p1").await && !live("p2").await && live("p3").await);
    let head = store.head().await.unwrap();
    let again = store.purge_source(&"crm".into(), None, None).await.unwrap();
    assert_eq!((again.retired, again.position), (0, head));
    // A record a principal also wrote refuses the whole purge.
    feed(&["p4", "p5"]).await;
    c.upsert(
        own,
        &ty("partner"),
        &key("p4"),
        patch(&[("note", json!("called"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let head = store.head().await.unwrap();
    let refused = store.purge_source(&"crm".into(), None, None).await;
    assert!(
        matches!(&refused, Err(Error::Refused { field: None, reason: Refusal::Invalid { message } })
            if message.contains("p4") && !message.contains("p5") && message.contains("another hand")),
        "{refused:?}"
    );
    assert_eq!(store.head().await.unwrap(), head);
    assert!(live("p5").await);
    // A record the source left no value in is not its any more.
    let clear = SourceRecord {
        upstream: None,
        ty: ty("partner"),
        key: key("p5"),
        fields: patch(&[("name", Value::Null)]),
        seen_at: None,
    };
    let mut acks = c
        .ingest(own, &"crm".into(), stream::of(vec![clear]))
        .await
        .unwrap();
    stream::collect(&mut acks).await;
    c.upsert(
        own,
        &ty("partner"),
        &key("p4"),
        patch(&[("note", Value::Null)]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    // Another source's record `crm` set a value on and cleared again:
    // nothing of `crm`'s is left in it, so it is neither retired nor refused.
    let source = |src: &'static str, fields: Patch| {
        let c = c.clone();
        let own = own.clone();
        async move {
            let rec = SourceRecord {
                upstream: None,
                ty: ty("partner"),
                key: key("p7"),
                fields,
                seen_at: None,
            };
            let mut acks = c
                .ingest(&own, &src.into(), stream::of(vec![rec]))
                .await
                .unwrap();
            stream::collect(&mut acks).await;
        }
    };
    source("erp", patch(&[("name", json!("p7"))])).await;
    source("crm", patch(&[("city", json!("Oslo"))])).await;
    source("crm", patch(&[("city", Value::Null)])).await;
    let report = store.purge_source(&"crm".into(), None, None).await.unwrap();
    assert_eq!(report.retired, 1, "{report:?}");
    assert!(!live("p4").await && live("p5").await && live("p7").await);
}

/// The lake of the typed purge's checks: `crm` brought in the partners
/// `p1` and `p2`, the bill `b1` that names `p1` in a field, and the link
/// of `bill`'s source relation from `b1` to `p2`; `erp` brought in the
/// partner `p3`.
async fn purge_lake(store: &Arc<dyn LakeStore>) -> (Arc<dyn Catalog>, Principal) {
    let c = store.catalog(WriteContext::default());
    let own = Cast::of(store.lake()).owner;
    for d in [
        json!({"declare": "type", "name": "partner",
            "fields": [{"name": "name", "type": {"kind": "text"}, "authority": "source"}]}),
        json!({"declare": "type", "name": "bill",
            "fields": [{"name": "partner_id", "type": {"kind": "text"}, "authority": "source"},
                       {"name": "note", "type": {"kind": "text"}}],
            "relations": [{"name": "billed_to", "target": "partner", "via": "partner_id"},
                          {"name": "seen_by", "target": "partner", "cardinality": "many_to_many",
                           "authority": "source"}]}),
    ] {
        c.declare(&own, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    for (source, t, k, fields) in [
        ("crm", "partner", "p1", json!({"name": "Harbor"})),
        ("crm", "partner", "p2", json!({"name": "Quay"})),
        ("erp", "partner", "p3", json!({"name": "Pier"})),
        ("crm", "bill", "b1", json!({"partner_id": "p1"})),
        (
            "crm",
            "sys.link.bill.seen_by",
            "b1:p2",
            json!({"from": "b1", "to": "p2"}),
        ),
    ] {
        let record = SourceRecord {
            upstream: None,
            ty: ty(t),
            key: key(k),
            fields: serde_json::from_value(fields).unwrap(),
            seen_at: None,
        };
        let mut acks = c
            .ingest(&own, &source.into(), stream::of(vec![record]))
            .await
            .unwrap();
        let said = stream::collect(&mut acks).await;
        assert!(
            said.iter().all(Result::is_ok),
            "feeding `{t}` `{k}`: {said:?}"
        );
    }
    (c, own)
}

/// A purge that names a type takes what the source brought into that type
/// and the links of the relations it declares, and nothing else: another
/// source's records of the type and the source's records of its other
/// types stay, and those that refer into the purge are named; a record
/// another hand also wrote refuses it only in the named type
/// (`docs/decisions/039-a-source-is-retracted-as-one.md`, extended).
pub async fn a_purge_that_names_a_type_takes_what_the_source_brought_into_it_alone(
    store: Arc<dyn LakeStore>,
) {
    let (c, own) = purge_lake(&store).await;
    let (crm, partner, bill) = (SourceId::from("crm"), ty("partner"), ty("bill"));
    let note = |v: Value| {
        let (c, own) = (c.clone(), own.clone());
        async move {
            let fields = patch(&[("note", v)]);
            c.upsert(&own, &ty("bill"), &key("b1"), fields, WriteOpts::default())
                .await
                .unwrap();
        }
    };
    let live = |t: &'static str, k: &'static str| {
        let (c, own) = (c.clone(), own.clone());
        async move { c.get(&own, &ty(t), get(k)).await.is_ok() }
    };
    // `b1` is also a person's: that refuses a purge from `bill`, and does
    // not hold a purge from `partner`.
    note(json!("called")).await;
    let head = store.head().await.unwrap();
    let refused = store.purge_source(&crm, Some(&bill), None).await;
    let seen = store.purge_preflight(&crm, Some(&bill)).await;
    for e in [refused.unwrap_err(), seen.unwrap_err()] {
        assert!(invalid_naming(&e, &["`b1`", "from `bill`"]), "{e:?}");
    }
    assert_eq!(store.head().await.unwrap(), head);
    let report = store.purge_source(&crm, Some(&partner), None).await;
    let report = report.unwrap();
    let per = |r: &lakelands_api::write::PurgeReport| {
        let mut per: Vec<_> = (r.types.iter())
            .map(|t| (t.ty.to_string(), t.retired))
            .collect();
        per.sort();
        per
    };
    let named: Vec<_> = (report.references.iter())
        .map(|f| {
            let first = f.first.iter().map(|k| format!("{} {}", k.ty, k.key));
            (
                format!("{}.{}", f.ty, f.relation),
                f.records,
                first.collect::<Vec<_>>(),
            )
        })
        .collect();
    let want = vec![
        ("bill.billed_to".to_owned(), 1, vec!["bill b1".to_owned()]),
        (
            "bill.seen_by".to_owned(),
            1,
            vec!["sys.link.bill.seen_by b1:p2".to_owned()],
        ),
    ];
    assert_eq!(
        (report.ty.as_ref(), report.retired, per(&report)),
        (Some(&partner), 2, vec![("partner".to_owned(), 2)]),
        "{report:?}"
    );
    assert_eq!(
        (named, report.unsearched.len()),
        (want, 0),
        "what `crm` keeps in `bill` refers into the purge and is named: {report:?}"
    );
    assert!(!live("partner", "p1").await && !live("partner", "p2").await);
    assert!(
        live("partner", "p3").await
            && live("bill", "b1").await
            && live("sys.link.bill.seen_by", "b1:p2").await,
        "another source's partner, and the bill and the link of `bill`'s relation, stay"
    );
    // Once `b1` is the source's alone, the purge from `bill` takes it and
    // the link its relation carries.
    note(Value::Null).await;
    let report = store.purge_source(&crm, Some(&bill), None).await;
    let report = report.unwrap();
    let both = vec![
        ("bill".to_owned(), 1),
        ("sys.link.bill.seen_by".to_owned(), 1),
    ];
    assert_eq!((report.retired, per(&report)), (2, both), "{report:?}");
    assert!(!live("bill", "b1").await && !live("sys.link.bill.seen_by", "b1:p2").await);
}

/// A preflight's id is of the source, the type and the head: the id of
/// the source whole and the id of the source from one type do not fit
/// each other's purge. A type a purge cannot take from is refused, not
/// passed over: not found when it is not declared, invalid when it is a
/// view or keeps no per-field provenance
/// (`docs/decisions/039-a-source-is-retracted-as-one.md`, extended).
pub async fn a_typed_purge_has_an_id_of_its_own_and_refuses_a_type_it_cannot_take_from(
    store: Arc<dyn LakeStore>,
) {
    let (c, own) = purge_lake(&store).await;
    for d in [
        json!({"declare": "type", "name": "log_line", "field_meta": false,
            "fields": [{"name": "text", "type": {"kind": "text"}, "authority": "source"}]}),
        json!({"declare": "type", "name": "partners",
            "fields": [{"name": "n", "type": {"kind": "number"}}],
            "view": {"source": "partner", "measures": [{"name": "n", "expr": "count()"}]}}),
    ] {
        c.declare(&own, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let (crm, partner) = (SourceId::from("crm"), ty("partner"));
    let head = store.head().await.unwrap();
    let whole = store.purge_preflight(&crm, None).await.unwrap();
    let typed = store.purge_preflight(&crm, Some(&partner)).await.unwrap();
    assert_ne!(whole.report_id, typed.report_id);
    assert_eq!(
        (whole.retired, typed.retired),
        (4, 2),
        "{whole:?} {typed:?}"
    );
    for (t, id) in [(None, &typed.report_id), (Some(&partner), &whole.report_id)] {
        let e = store.purge_source(&crm, t, Some(id)).await.unwrap_err();
        assert!(
            invalid_naming(&e, &["not of this purge at this head"]),
            "{e:?}"
        );
    }
    for (t, why) in [
        ("log_line", "keeps no per-field provenance"),
        ("partners", "is a view"),
    ] {
        let refused = store.purge_source(&crm, Some(&ty(t)), None).await;
        let seen = store.purge_preflight(&crm, Some(&ty(t))).await;
        for e in [refused.unwrap_err(), seen.unwrap_err()] {
            assert!(invalid_naming(&e, &[t, why]), "{e:?}");
        }
    }
    let none = store.purge_source(&crm, Some(&ty("nothing")), None).await;
    assert!(matches!(none, Err(Error::NotFound { .. })), "{none:?}");
    assert_eq!(store.head().await.unwrap(), head, "nothing is appended");
    let done = store
        .purge_source(&crm, Some(&partner), Some(&typed.report_id))
        .await
        .unwrap();
    assert_eq!(
        (done.retired, done.ty.as_ref()),
        (2, Some(&partner)),
        "{done:?}"
    );
}

/// A value type is loosened by `declare`; a tightening is refused there,
/// counted by its preflight over every stored value, refused while any
/// would not fit, and otherwise put in force as its next version
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
pub async fn a_value_type_is_loosened_by_declare_and_tightened_by_its_preflight(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    let vt = |one_of: Value| {
        serde_json::from_value::<lakelands_api::ontology::ValueTypeDecl>(
            json!({"name": "status_code", "kind": "text", "one_of": one_of}),
        )
        .unwrap()
    };
    c.declare(
        own,
        decl(
            json!({"declare": "value_type", "name": "status_code", "kind": "text",
                    "one_of": ["open", "late", "void"]}),
        ),
    )
    .await
    .unwrap();
    c.declare(
        own,
        decl(json!({"declare": "type", "name": "bill",
                    "fields": [{"name": "status", "type": {"kind": "text", "value_type": "status_code"}}]})),
    )
    .await
    .unwrap();
    for (k, s) in [("b1", "late"), ("b2", "open")] {
        c.upsert(
            own,
            &ty("bill"),
            &key(k),
            patch(&[("status", json!(s))]),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    // A loosening is declared.
    c.declare(
        own,
        decl(
            json!({"declare": "value_type", "name": "status_code", "kind": "text",
                    "one_of": ["open", "late", "void", "held"]}),
        ),
    )
    .await
    .unwrap();
    // A tightening is not.
    let head = store.head().await.unwrap();
    let refused = c
        .declare(
            own,
            decl(
                json!({"declare": "value_type", "name": "status_code", "kind": "text",
                        "one_of": ["open", "void"]}),
            ),
        )
        .await;
    assert!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
            if message.contains("migrate")),
        "{refused:?}"
    );
    assert_eq!(store.head().await.unwrap(), head);
    // Its preflight counts `b1`, and the apply is refused while it misfits.
    let report = c
        .migrate_value_type(own, vt(json!(["open", "void"])))
        .await
        .unwrap()
        .data;
    assert_eq!(
        (report.records_scanned, report.misfits, report.fits),
        (2, 1, false),
        "{report:?}"
    );
    assert_eq!(report.first[0].key, key("b1"));
    let refused = c
        .migrate_value_type_apply(own, vt(json!(["open", "void"])), &report.report_id)
        .await;
    assert!(
        matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(store.head().await.unwrap(), head);
    // Without `held`, which no bill holds, every value fits.
    let to = vt(json!(["open", "late", "void"]));
    let report = c.migrate_value_type(own, to.clone()).await.unwrap().data;
    assert!(report.fits, "{report:?}");
    let applied = c
        .migrate_value_type_apply(own, to, &report.report_id)
        .await
        .unwrap();
    assert_eq!(applied.position(), store.head().await.unwrap());
    let written = c
        .upsert(
            own,
            &ty("bill"),
            &key("b3"),
            patch(&[("status", json!("held"))]),
            WriteOpts::default(),
        )
        .await;
    assert!(
        matches!(
            &written,
            Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            })
        ),
        "{written:?}"
    );
}

/// A `sort_index.<type>` setting asks for the order index on fields its
/// type declares with a scalar kind, and is refused naming anything else;
/// `describe_type` names the fields indexed, of those the principal sees
/// (`docs/decisions/041-a-sorted-page-reads-its-page.md`).
pub async fn a_sort_index_setting_names_declared_scalar_fields_and_describe_shows_them(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let reading = TypeDecl {
        annotations: Default::default(),
        name: "reading".into(),
        key: Vec::new(),
        fields: vec![
            text("label"),
            FieldDecl {
                ty: FieldType::kind(StorageKind::Json),
                ..text("payload")
            },
        ],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(reading))
        .await
        .expect("the owner declares `reading`");
    let set = |k: &'static str, value: Value| {
        let catalog = f.catalog.clone();
        async move {
            catalog
                .upsert(
                    owner,
                    &ty("sys.setting"),
                    &key(k),
                    patch(&[("key", json!(k)), ("value", value)]),
                    WriteOpts::default(),
                )
                .await
        }
    };
    for (k, value, at_fault) in [
        ("sort_index.reading", json!(["payload"]), "payload"),
        ("sort_index.reading", json!(["label", "weight"]), "weight"),
        ("sort_index.gauge", json!(["label"]), "key"),
        ("sort_index.reading", json!("label"), "value"),
    ] {
        let e = set(k, value.clone())
            .await
            .expect_err("a setting naming what no order index keeps is refused");
        let (field, reason) = refusal(&e);
        assert!(
            matches!(reason, Refusal::Invalid { .. }),
            "`{k}` = {value}: expected invalid, got {reason:?}"
        );
        assert_eq!(
            field.as_ref().map(FieldName::as_str),
            Some(at_fault),
            "`{k}` = {value}: the refusal names the field at fault"
        );
    }
    let sorted = |who: &Principal| {
        let catalog = f.catalog.clone();
        let who = who.clone();
        async move {
            catalog
                .describe_type(&who, &ty(INVOICE), ReadAt::default())
                .await
                .expect("the type is described")
                .data
                .lifecycle
                .sorted
        }
    };
    assert!(
        sorted(owner).await.is_empty(),
        "no setting, nothing indexed"
    );
    set("sort_index.invoice", json!(["amount", "status"]))
        .await
        .expect("a setting naming declared scalar fields applies");
    assert_eq!(
        sorted(owner).await,
        vec![FieldName::from("amount"), FieldName::from("status")]
    );
    assert_eq!(
        sorted(&f.cast.stranger).await,
        vec![FieldName::from("status")],
        "a principal is told of the indexed fields it sees"
    );
    set("sort_index.invoice", json!(["status"]))
        .await
        .expect("a setting naming fewer fields applies");
    assert_eq!(sorted(owner).await, vec![FieldName::from("status")]);
    f.catalog
        .retire(
            owner,
            &ty("sys.setting"),
            &key("sort_index.invoice"),
            WriteOpts::default(),
        )
        .await
        .expect("the setting retires");
    assert!(
        sorted(owner).await.is_empty(),
        "the setting retired, nothing indexed"
    );
}

/// A view of the invoices by `status`, with these measures: each a name,
/// its kind and its expression.
fn by_status(name: &str, measures: &[(&str, StorageKind, &str)]) -> Declaration {
    let named = |(name, _, expr): &(&str, StorageKind, &str)| Measure {
        name: (*name).into(),
        expr: (*expr).into(),
    };
    let number = |(name, kind, _): &(&str, StorageKind, &str)| FieldDecl {
        ty: FieldType::kind(*kind),
        ..text(name)
    };
    let mut fields = vec![text("status")];
    fields.extend(measures.iter().map(number));
    Declaration::Type(TypeDecl {
        name: name.into(),
        key: vec!["status".into()],
        fields,
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            params: Vec::new(),
            r#where: None,
            group_by: vec!["status".into()],
            measures: measures.iter().map(named).collect(),
            merge: Default::default(),
        })),
        field_meta: true,
        ..Default::default()
    })
}

/// A `view_kept.<type>` setting asks for a view's records to be kept. It
/// names a declared view and holds `true` or `false`; under `true`
/// the view must be one that can be kept, and the refusal names the view,
/// the measure and the call. `describe_type` says of a view, and of no
/// other type, whether it is kept or computed and why. A view declared
/// again so that it can no longer be kept is not refused under a standing
/// setting: `describe_type` then says why it computes, and names the
/// measure and what it reads to a principal who is shown the view as
/// declared, and to no other.
pub async fn a_view_kept_setting_names_a_view_that_can_be_kept_and_describe_says_whether_it_is(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (owner, clerk) = (&f.cast.owner, &f.cast.clerk);
    let view = by_status;
    let (decimal, count) = (StorageKind::Decimal, StorageKind::Number);
    let totals = view(
        "totals",
        &[("total", decimal, "sum(amount)"), ("n", count, "count()")],
    );
    for d in [totals, view("secrets", &[("n", count, "count()")])] {
        f.catalog
            .declare(owner, d)
            .await
            .expect("the owner declares a view");
    }
    let set = |k: &'static str, value: Value| {
        let catalog = f.catalog.clone();
        async move {
            let fields = patch(&[("key", json!(k)), ("value", value)]);
            let (setting, opts) = (ty("sys.setting"), WriteOpts::default());
            catalog.upsert(owner, &setting, &key(k), fields, opts).await
        }
    };
    let kept = |who: &Principal, of: &'static str| {
        let (catalog, who) = (f.catalog.clone(), who.clone());
        async move {
            let described = catalog
                .describe_type(&who, &ty(of), ReadAt::default())
                .await;
            described
                .expect("the type is described")
                .data
                .lifecycle
                .kept
        }
    };
    let not_asked = Some(Kept::Computed {
        why: NotKept::NotAsked,
    });

    // The refusals, by the member at fault.
    for (k, value, at_fault) in [
        ("view_kept.invoice", json!(true), "key"),
        ("view_kept.nowhere", json!(true), "key"),
        ("view_kept.totals", json!("yes"), "value"),
        ("view_kept.totals", json!(1), "value"),
    ] {
        let e = set(k, value.clone())
            .await
            .expect_err("the setting is refused");
        let (field, reason) = refusal(&e);
        assert!(
            matches!(reason, Refusal::Invalid { .. }),
            "`{k}` = {value}: expected invalid, got {reason:?}"
        );
        assert_eq!(
            field.as_ref().map(FieldName::as_str),
            Some(at_fault),
            "`{k}` = {value}: the refusal names the member at fault"
        );
    }

    // A stored type says nothing of it; a view nobody asked for says so.
    assert_eq!(kept(owner, INVOICE).await, None);
    assert_eq!(kept(owner, "totals").await, not_asked);
    // Asked for, a view that can be kept is kept, or says why the lake
    // does not keep it here; it is neither refused nor "not asked".
    set("view_kept.totals", json!(true))
        .await
        .expect("the view can be kept");
    for who in [owner, clerk] {
        let asked = kept(who, "totals").await;
        let here = |why: &NotKept| !matches!(why, NotKept::NotAsked | NotKept::CannotBeKept { .. });
        assert!(
            matches!(&asked, Some(Kept::Kept))
                || matches!(&asked, Some(Kept::Computed { why }) if here(why)),
            "`totals` is asked for and can be kept, and says {asked:?}"
        );
    }
    let off = set("view_kept.totals", json!(false)).await;
    off.expect("asking for nothing applies");
    assert_eq!(kept(owner, "totals").await, not_asked);
    set("view_kept.totals", json!(true))
        .await
        .expect("asked for again");
    let (setting, gone) = (ty("sys.setting"), key("view_kept.totals"));
    let retired = f
        .catalog
        .retire(owner, &setting, &gone, WriteOpts::default());
    retired.await.expect("the setting retires");
    assert_eq!(kept(owner, "totals").await, not_asked);

    // A view declared again so that it cannot be kept keeps its setting.
    set("view_kept.secrets", json!(true))
        .await
        .expect("the view can be kept");
    let unkeepable = view("secrets", &[("n", count, "len(max(secret))")]);
    let again = f.catalog.declare(owner, unkeepable).await;
    again.expect("a declaration under a standing setting is not refused");
    let Some(Kept::Computed {
        why: NotKept::CannotBeKept { reason },
    }) = kept(owner, "secrets").await
    else {
        panic!("`secrets` takes an extreme of a text and cannot be kept");
    };
    let reason = reason.expect("the owner is shown the view, and is told why");
    assert!(
        reason.contains("`n`") && reason.contains("max(secret)"),
        "the reason names the measure and the call: {reason}"
    );
    // The clerk is kept from `secret`: it is told that, and not why.
    assert_eq!(
        kept(clerk, "secrets").await,
        Some(Kept::Computed {
            why: NotKept::CannotBeKept { reason: None },
        }),
    );
    // Asking for it again is refused, naming the view, measure and call.
    set("view_kept.secrets", json!(false))
        .await
        .expect("asking for nothing applies");
    let e = set("view_kept.secrets", json!(true))
        .await
        .expect_err("it cannot be kept");
    let (field, reason) = refusal(&e);
    assert_eq!(field.as_ref().map(FieldName::as_str), Some("key"));
    let Refusal::Invalid { message } = reason else {
        panic!("expected invalid, got {reason:?}");
    };
    for name in ["secrets", "`n`", "max(secret)"] {
        assert!(
            message.contains(name),
            "the refusal names {name}: {message}"
        );
    }
}

/// A write of `sys.setting` as `who`, leaving `value` under the key `k`.
async fn set_as(
    c: &Arc<dyn Catalog>,
    who: &Principal,
    k: &str,
    value: Value,
) -> Result<Outcome, Error> {
    let fields = patch(&[("key", json!(k)), ("value", value)]);
    let (setting, opts) = (ty("sys.setting"), WriteOpts::default());
    c.upsert(who, &setting, &key(k), fields, opts).await
}

/// A refusal as text: two writes that differ in one name are answered
/// alike when these differ in that name alone.
#[track_caller]
fn answered<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    format!("{:?}", r.expect_err("the write is refused"))
}

/// A setting the store reads is judged by what its writer is shown, after
/// the grant on `sys.setting` (024, extended). One who may not write a
/// setting gets the grant's answer under every key. To one who may, a
/// type it is not shown is a name that is not declared, under every
/// value, and a field it is not shown is one the type does not declare; a
/// view it is shown without its definition cannot be kept, and nothing
/// says why. `propose` answers as `upsert` does. A standing setting is
/// written by one shown everything it names, and retired by any who may.
pub async fn a_setting_is_judged_by_what_its_writer_is_shown(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, owner, tuner) = (&f.catalog, &f.cast.owner, &f.cast.tuner);
    let (decimal, count) = (StorageKind::Decimal, StorageKind::Number);
    let vault = Declaration::Type(TypeDecl {
        name: "vault".into(),
        key: vec!["code".into()],
        fields: vec![text("code")],
        field_meta: true,
        ..Default::default()
    });
    for d in [
        vault,
        by_status("totals", &[("total", decimal, "sum(amount)")]),
        by_status("longest", &[("n", count, "len(max(secret))")]),
    ] {
        c.declare(owner, d).await.expect("the owner declares");
    }
    // The tuner is shown `longest` and not its definition; `vault` and
    // `totals` it is not shown at all.
    let longest = ty("longest");
    let shown = c.describe_type(tuner, &longest, ReadAt::default()).await;
    let shown = shown.expect("the tuner is shown the view").data;
    assert!(shown.view.is_none(), "and not its definition: {shown:?}");
    for hidden in ["vault", "totals"] {
        let d = c.describe_type(tuner, &ty(hidden), ReadAt::default()).await;
        assert!(d.is_err(), "the tuner is not shown `{hidden}`: {d:?}");
    }

    // (i) The grant first: who may not write a setting is answered so,
    // whatever the setting names.
    for who in [&f.cast.stranger, &f.cast.clerk] {
        let grant = answered(set_as(c, who, "other.thing", json!(1)).await);
        for (k, value) in [
            ("sort_index.invoice", json!(["status"])),
            ("sort_index.vault", json!(["code"])),
            ("sort_index.nowhere", json!(["code"])),
            ("vector_index.invoice", json!(["status"])),
            ("view_kept.totals", json!("yes")),
            ("view_kept.longest", json!(true)),
        ] {
            let answer = answered(set_as(c, who, k, value).await).replace(k, "other.thing");
            assert_eq!(answer, grant, "`{k}` as {}: the grant answers", who.id);
        }
    }

    // (ii) A type that is not shown answers as a name that is none.
    let values = [
        ("sort_index.", vec![json!(["code"]), json!([]), json!("x")]),
        ("vector_index.", vec![json!(["code"]), json!([])]),
        ("view_kept.", vec![json!(true), json!(false), json!("yes")]),
    ];
    for (prefix, values) in &values {
        for value in values {
            let none = format!("{prefix}nowhere");
            let none = answered(set_as(c, tuner, &none, value.clone()).await);
            assert!(none.contains("nowhere"), "the judge answers: {none}");
            for hidden in ["vault", "totals"] {
                let k = format!("{prefix}{hidden}");
                let answer =
                    answered(set_as(c, tuner, &k, value.clone()).await).replace(hidden, "nowhere");
                assert_eq!(answer, none, "`{k}` = {value} answers as `nowhere` does");
            }
        }
    }

    // (iii) A field that is not shown answers as one the type does not
    // declare; one shown masked is named with its kind, as `describe` has it.
    for k in ["sort_index.invoice", "vector_index.invoice"] {
        let none = answered(set_as(c, tuner, k, json!(["nope"])).await);
        assert!(none.contains("does not declare"), "{none}");
        let answer =
            answered(set_as(c, tuner, k, json!(["secret"])).await).replace("secret", "nope");
        assert_eq!(answer, none, "`{k}`: `secret` answers as `nope` does");
    }
    let masked = answered(set_as(c, tuner, "vector_index.invoice", json!(["note"])).await);
    assert!(masked.contains("of kind"), "`note` is shown: {masked}");

    // (iv) A view shown without its definition: that it cannot be kept,
    // and nothing of the measure, the call or what it reads.
    let whole = answered(set_as(c, owner, "view_kept.longest", json!(true)).await);
    assert!(
        whole.contains("max(secret)"),
        "the owner is told why: {whole}"
    );
    let e = set_as(c, tuner, "view_kept.longest", json!(true)).await;
    let e = e.expect_err("it cannot be kept");
    let (field, reason) = refusal(&e);
    assert_eq!(field.as_ref().map(FieldName::as_str), Some("key"));
    let Refusal::Invalid { message } = reason else {
        panic!("expected invalid, got {reason:?}");
    };
    assert!(message.ends_with("and it cannot be"), "{message}");
    for name in ["`n`", "max", "secret"] {
        assert!(!message.contains(name), "nothing says {name}: {message}");
    }
    let off = set_as(c, tuner, "view_kept.longest", json!(false)).await;
    off.expect("asking for nothing applies");

    // (v) `propose` answers as `upsert` does.
    for (k, value) in [
        ("sort_index.vault", json!(["code"])),
        ("sort_index.invoice", json!(["secret"])),
        ("view_kept.longest", json!(true)),
    ] {
        let write = ProposedWrite::Upsert {
            ty: ty("sys.setting"),
            key: key(k),
            patch: patch(&[("key", json!(k)), ("value", value.clone())]),
            opts: WriteOpts::default(),
        };
        let proposed = answered(c.propose(tuner, write, "why".into()).await);
        let written = answered(set_as(c, tuner, k, value).await);
        assert_eq!(proposed, written, "`{k}` is refused when it is proposed");
    }

    // (vi) A standing setting that names what the tuner is not shown: the
    // judge reads the value a write leaves, so the tuner neither extends
    // it nor patches beside it; it retires it.
    let k = "sort_index.invoice";
    let stands = set_as(c, owner, k, json!(["secret"])).await;
    stands.expect("the owner indexes `secret`");
    let none = answered(set_as(c, tuner, k, json!(["nope", "status"])).await);
    let more = set_as(c, tuner, k, json!(["secret", "status"])).await;
    assert_eq!(answered(more).replace("secret", "nope"), none);
    let beside = patch(&[("key", json!(k))]);
    let (setting, opts) = (ty("sys.setting"), WriteOpts::default);
    let beside = c.upsert(tuner, &setting, &key(k), beside, opts()).await;
    let beside = answered(beside);
    assert!(beside.contains("does not declare"), "{beside}");
    let retired = c.retire(tuner, &setting, &key(k), opts()).await;
    retired.expect("a retirement is not judged");
    let own = set_as(c, tuner, k, json!(["status"])).await;
    own.expect("the tuner indexes what it is shown");
}

/// `vault` with a record `v1` and a retired `v0`, the verbs `seal` on a
/// record of it and `open` on the type, and the view `longest` over it.
async fn declare_vault(f: &Fixture) {
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let number = |name: &str| FieldDecl {
        ty: FieldType::kind(StorageKind::Number),
        ..text(name)
    };
    let vault = TypeDecl {
        name: "vault".into(),
        key: vec!["code".into()],
        fields: vec![
            text("code"),
            text("state"),
            number("depth"),
            FieldDecl {
                sensitive: true,
                ..text("hush")
            },
        ],
        field_meta: true,
        ..Default::default()
    };
    let longest = TypeDecl {
        name: "longest".into(),
        key: vec!["state".into()],
        fields: vec![text("state"), number("n")],
        view: Some(Box::new(ViewDecl {
            source: "vault".into(),
            params: Vec::new(),
            r#where: None,
            group_by: vec!["state".into()],
            measures: vec![Measure {
                name: "n".into(),
                expr: "count()".into(),
            }],
            merge: Default::default(),
        })),
        field_meta: true,
        ..Default::default()
    };
    let sealed = Assignment {
        field: "state".into(),
        value: Condition::from("'sealed'"),
    };
    let seal = VerbDecl {
        name: "seal".into(),
        on: "vault".into(),
        scope: Default::default(),
        payload: Vec::new(),
        risk: Risk::Low,
        requires: None,
        effect: VerbEffect::Write { set: vec![sealed] },
        description: None,
    };
    let open = VerbDecl {
        name: "open".into(),
        scope: VerbScope::Type,
        payload: vec![text("why")],
        effect: VerbEffect::External {
            executor: "crm".into(),
        },
        ..seal.clone()
    };
    for d in [
        Declaration::Type(vault),
        Declaration::Type(longest),
        Declaration::Verb(seal),
        Declaration::Verb(open),
    ] {
        c.declare(owner, d).await.expect("the owner declares");
    }
    let (vault, opts) = (ty("vault"), WriteOpts::default);
    for k in ["v0", "v1"] {
        let p = patch(&[("code", json!(k)), ("depth", json!(1))]);
        let written = c.upsert(owner, &vault, &key(k), p, opts()).await;
        written.expect("the owner writes a vault");
    }
    let retired = c.retire(owner, &vault, &key("v0"), opts()).await;
    retired.expect("the owner retires one");
}

/// Every door that writes a record, called by `who` on the type `t`, on a
/// verb `on_record` of its records and on a verb `on_type` of the type:
/// each row's name and what it was answered.
async fn write_doors(
    c: &Arc<dyn Catalog>,
    who: &Principal,
    t: &str,
    on_record: &str,
    on_type: &str,
) -> Vec<(&'static str, String)> {
    let said = |r: Result<Outcome, Error>| format!("{r:?}");
    let (t, o) = (&ty(t), WriteOpts::default);
    let pinned = WriteOpts {
        expected_version: Some(Version(9)),
        ..o()
    };
    let code = || patch(&[("code", json!("v2"))]);
    let depth = || patch(&[("depth", json!(2))]);
    let why = || patch(&[("why", json!("x"))]);
    let mut out = Vec::new();
    for (row, k, p, opts) in [
        ("upsert a new key", "v2", code(), o()),
        ("upsert a live key", "v1", depth(), o()),
        ("upsert a retired key", "v0", depth(), o()),
        (
            "upsert a field that is none",
            "v2",
            patch(&[("nope", json!(1))]),
            o(),
        ),
        ("upsert an empty patch", "v2", patch(&[]), o()),
        ("upsert at a stale version", "v1", depth(), pinned),
        ("upsert an empty key", "", code(), o()),
    ] {
        out.push((row, said(c.upsert(who, t, &key(k), p, opts).await)));
    }
    for (row, k) in [("retire a live key", "v1"), ("retire no record", "v9")] {
        out.push((row, said(c.retire(who, t, &key(k), o()).await)));
    }
    let calls = [
        ("the record's verb", on_record, Some("v1"), patch(&[])),
        (
            "the record's verb without a key",
            on_record,
            None,
            patch(&[]),
        ),
        ("the type's verb", on_type, None, why()),
        ("the type's verb with a key", on_type, Some("v1"), why()),
    ];
    for (row, verb, k, payload) in calls.clone() {
        let k = k.map(key);
        let called = c.call(who, &verb.into(), k.as_ref(), payload, o()).await;
        out.push((row, said(called)));
    }
    let mut proposed = vec![
        ProposedWrite::Upsert {
            ty: t.clone(),
            key: key("v2"),
            patch: code(),
            opts: o(),
        },
        ProposedWrite::Retire {
            ty: t.clone(),
            key: key("v1"),
            opts: o(),
        },
    ];
    proposed.extend(calls.map(|(_, verb, k, payload)| ProposedWrite::Call {
        verb: verb.into(),
        key: k.map(key),
        payload,
        opts: o(),
    }));
    for write in proposed {
        let p = c.propose(who, write, "probing".into()).await;
        out.push(("the same, proposed", said(p)));
    }
    // The source door is its grant's first: only one who holds it is
    // answered about the type.
    for fields in [code(), patch(&[])] {
        if !who.grants.ingest {
            break;
        }
        let rec = SourceRecord {
            upstream: None,
            ty: t.clone(),
            key: key("v3"),
            fields,
            seen_at: None,
        };
        let acks = c.ingest(who, &"bank".into(), stream::of(vec![rec])).await;
        let mut acks = acks.expect("one with the grant feeds");
        let acks = stream::collect(&mut acks).await;
        out.push(("ingest", format!("{acks:?}")));
    }
    out
}

/// A type its writer is not shown is a type that is not declared
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended): at every door that writes a record, the stranger and the
/// importer, who are shown nothing of `vault`, and the tuner, who holds a
/// grant on the view `longest` and none on its source, are answered as on
/// `ghost`, which is not declared, whatever the patch, the key, the
/// version or the verb's scope would be told otherwise. A verb on such a
/// type is a verb that is not declared. Nothing is written. So at every
/// door that reads one, its sensitive `hush` notwithstanding
/// (lakelands#636, lakelands#635): over a network door too, where the
/// cast is all the resolver knows.
pub async fn a_type_its_writer_is_not_shown_is_a_type_that_is_not_declared(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    declare_vault(&f).await;
    let (c, cast) = (&f.catalog, &f.cast);
    let head = f.head().await;
    for (who, hidden) in [
        (&cast.stranger, "vault"),
        (&cast.importer, "vault"),
        (&cast.tuner, "vault"),
        (&cast.tuner, "longest"),
        (&cast.importer, "longest"),
    ] {
        let plain =
            |a: &str, names: [&str; 3]| names.iter().fold(a.to_string(), |a, n| a.replace(n, "_"));
        let mut there = write_doors(c, who, hidden, "seal", "open").await;
        let mut none = write_doors(c, who, "ghost", "haunt", "howl").await;
        there.extend(read_doors(c, who, hidden).await);
        none.extend(read_doors(c, who, "ghost").await);
        for ((row, a), (_, b)) in there.iter().zip(&none) {
            let (a, b) = (
                plain(a, [hidden, "seal", "open"]),
                plain(b, ["ghost", "haunt", "howl"]),
            );
            // A network door refuses an empty key itself, for any type.
            let found = b.contains("NotFound") || *row == "upsert an empty key";
            assert!(found, "{row} on `ghost`: {b}");
            let who = &who.id;
            assert_eq!(a, b, "{row} as {who} on `{hidden}` answers as on `ghost`");
        }
    }
    assert_eq!(f.head().await, head, "nothing was written");
}

/// Every door that reads a type's records, called by `who` on the type
/// `t`: each row's name and what it was answered.
async fn read_doors(c: &Arc<dyn Catalog>, who: &Principal, t: &str) -> Vec<(&'static str, String)> {
    fn said<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
        format!("{r:?}")
    }
    let (t, at) = (&ty(t), ReadAt::default);
    let noted = Find {
        filter: Some("note = 'x'".into()),
        ..Find::default()
    };
    let words = Search {
        ty: t.clone(),
        mode: SearchMode::Lexical {
            text: "open".into(),
            fields: None,
        },
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: at(),
    };
    let from = Pattern::traverse(t.clone(), key("v1"), Vec::new());
    let total = Find {
        total: true,
        ..Find::default()
    };
    let count = json!({"aggregate": {"measures": [{"name": "n", "func": "count"}]}});
    let count: Find = serde_json::from_value(count).expect("a count");
    vec![
        ("describe_type", said(c.describe_type(who, t, at()).await)),
        ("get", said(c.get(who, t, get("v1")).await)),
        (
            "get a key no record has",
            said(c.get(who, t, get("v9")).await),
        ),
        ("find", said(c.find(who, t, Find::default()).await)),
        ("find with a total", said(c.find(who, t, total).await)),
        ("find a count", said(c.find(who, t, count).await)),
        ("find by a field", said(c.find(who, t, noted).await)),
        ("search", said(c.search(who, words).await)),
        ("a pattern", said(c.query(who, from).await)),
    ]
}

/// A type its principal is shown is one it may find and get
/// (`docs/decisions/006-grants-not-roles.md`, extended): one who reads no
/// field of `invoice`, with every field masked, with one masked by name
/// or with one write-only, is described `may.find` and `may.get`, is
/// answered the record by `get` and by `find` with its key, no value and
/// the masked fields in the declaration's order (the sensitive `note`
/// nobody named among them), and is offered the two reads and the type
/// in the lake's three read tools.
pub async fn a_type_its_principal_is_shown_is_one_it_may_find_and_get(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (owner, c, invoice) = (&f.cast.owner, &f.catalog, ty(INVOICE));
    let p = [
        ("note", json!("x")),
        ("status", json!("open")),
        ("verdict", json!("ok")),
    ];
    let (v1, opts) = (key("v1"), WriteOpts::default());
    let written = c.upsert(owner, &invoice, &v1, patch(&p), opts).await;
    written.expect("the owner writes an invoice");
    let one = |f: &str, a| TypeGrants {
        default: None,
        fields: [(f.into(), a)].into(),
        ..Default::default()
    };
    let whole = TypeGrants {
        default: Some(Access::Masked),
        ..TypeGrants::default()
    };
    let cast = [
        ("all masked", whole, &["status", "verdict", "note"][..]),
        (
            "one masked",
            one("verdict", Access::Masked),
            &["verdict", "note"],
        ),
        ("write-only", one("status", Access::WriteOnly), &["note"]),
    ];
    // Every row that is not as the sentence says: one assertion, so a
    // fault names each row it turns.
    let mut differ = Vec::new();
    for (id, grant, masked) in cast {
        let grants = Grants {
            types: [(invoice.clone(), grant)].into(),
            ..Grants::default()
        };
        let who = &principal(f.store.lake(), PrincipalKind::Agent, id, grants);
        let d = c.describe_type(who, &invoice, ReadAt::default()).await;
        let may = d.expect("one shown the type is described it").data.may;
        let got = c.get(who, &invoice, get("v1")).await.map(|r| r.data);
        let found = c.find(who, &invoice, Find::default()).await.map(|r| r.data);
        let lake = c
            .describe(who, ReadAt::default())
            .await
            .expect("the lake is described");
        let offered = tools(&lake.data).expect("the tools are built");
        let offered = offered.iter().filter(|t| match &t.door {
            Door::Find { ty } | Door::Get { ty } => ty == &invoice,
            Door::Query | Door::Search | Door::Subscribe => {
                t.input_schema.to_string().contains("\"invoice\"")
            }
            _ => false,
        });
        let masked: Vec<FieldName> = masked.iter().map(|m| (*m).into()).collect();
        let as_said = |r: &Record| r.fields.is_empty() && r.masked == masked;
        for (row, holds, said) in [
            ("may", may.find && may.get, format!("{may:?}")),
            ("get", got.as_ref().is_ok_and(as_said), format!("{got:?}")),
            (
                "find",
                matches!(&found, Ok(Found::Records { rows, .. }) if rows.len() == 1 && as_said(&rows[0])),
                format!("{found:?}"),
            ),
            (
                "tools",
                offered.clone().count() == 5,
                format!("{}", offered.count()),
            ),
        ] {
            if !holds {
                differ.push(format!("{id}, {row}: {said}"));
            }
        }
    }
    assert!(
        differ.is_empty(),
        "one who is shown `invoice` and reads no field of it may find and get it, is answered \
         its record with the masked fields in the declaration's order, and is offered the \
         reads; these rows differ:\n{}",
        differ.join("\n")
    );
}

/// `sensitive` shows a field, never a type
/// (`docs/decisions/006-grants-not-roles.md`, extended): one whose grants
/// give it nothing of `invoice`, which declares the sensitive `note`, by
/// no entry or by `{default: none}`, is answered at every door that reads
/// and every door that writes a record as on `ghost`, which is not
/// declared; so it is on the link type of a relation of `invoice` and on
/// a view over it, whatever it is granted on their names, and neither
/// `describe` nor the tools name one of the three. The same of `wage` to
/// one whose only grant is to read `band`, or to see it masked, which is
/// built from `salary`: a field is read as what it is built from (024),
/// and one that is none shows no type. One granted to write `band` is
/// shown `wage` with `band` write-only, and writes it. And on the border:
/// one granted to read `tip` of `hint`, which is built from the sensitive
/// `code` nobody named, is shown `hint` by the grant, with both masked.
pub async fn a_sensitive_field_shows_a_field_and_never_a_type(store: Arc<dyn LakeStore>) {
    const LINKS: &str = "sys.link.invoice.cites";
    let f = setup(store).await;
    let (owner, c, opts) = (&f.cast.owner, &f.catalog, WriteOpts::default);
    let mut declared = fixture();
    declared.retain_mut(|d| match d {
        Declaration::Type(t) if t.name.as_str() == INVOICE => {
            t.relations.push(RelationDecl {
                name: "cites".into(),
                target: INVOICE.into(),
                inverse: None,
                via: None,
                ..t.relations[0].clone()
            });
            true
        }
        _ => false,
    });
    for d in [
        json!({"declare": "type", "name": "invoices",
               "fields": [{"name": "n", "type": {"kind": "number"}}],
               "view": {"source": INVOICE, "measures": [{"name": "n", "expr": "count()"}]}}),
        json!({"declare": "type", "name": "wage",
               "fields": [{"name": "salary", "type": {"kind": "text"}},
                          {"name": "band", "type": {"kind": "text"},
                           "depends_on": ["salary"], "on_input_change": "clear"}]}),
        json!({"declare": "type", "name": "hint",
               "fields": [{"name": "code", "type": {"kind": "text"}, "sensitive": true},
                          {"name": "tip", "type": {"kind": "text"},
                           "depends_on": ["code"], "on_input_change": "clear"}]}),
    ] {
        declared.push(serde_json::from_value(d).expect("a declaration"));
    }
    for d in declared {
        c.declare(owner, d).await.expect("the owner declares it");
    }
    let (invoice, wage) = (ty(INVOICE), ty("wage"));
    for k in ["v0", "v1", "v4"] {
        let p = patch(&[("status", json!("open")), ("note", json!("x"))]);
        let written = c.upsert(owner, &invoice, &key(k), p, opts()).await;
        written.expect("the owner writes an invoice");
    }
    let retired = c.retire(owner, &invoice, &key("v0"), opts()).await;
    retired.expect("the owner retires one");
    let (v1, v4, cites) = (key("v1"), key("v4"), "cites".into());
    let linked = c.link(owner, &invoice, &v1, &cites, &v4, Patch::new(), opts());
    linked.await.expect("the owner links two");
    let p = patch(&[("salary", json!("1")), ("band", json!("2"))]);
    let written = c.upsert(owner, &wage, &key("v1"), p, opts()).await;
    written.expect("the owner writes a wage");

    let lake = f.store.lake();
    let whole = |a| TypeGrants {
        default: Some(a),
        ..TypeGrants::default()
    };
    let one = |f: &str, a| TypeGrants {
        default: None,
        fields: [(f.into(), a)].into(),
        ..Default::default()
    };
    let band = |a| one("band", a);
    let of = |id: &str, default, types: &[(&str, TypeGrants)]| {
        let types = types.iter().map(|(t, g)| (ty(t), g.clone())).collect();
        let grants = Grants {
            default,
            types,
            ..Grants::default()
        };
        principal(lake, PrincipalKind::Agent, id, grants)
    };
    let (read, made) = (whole(Access::Read), [INVOICE, LINKS, "invoices"]);
    let beside = [(LINKS, read.clone()), ("invoices", read)];
    let cast = [
        (of("no entry", Access::None, &[]), &made[..1]),
        (
            of("none", Access::Read, &[(INVOICE, whole(Access::None))]),
            &made[..],
        ),
        (of("beside", Access::None, &beside), &made[..]),
        (
            of("reads", Access::None, &[("wage", band(Access::Read))]),
            &["wage"][..],
        ),
        (
            of("masked", Access::None, &[("wage", band(Access::Masked))]),
            &["wage"][..],
        ),
    ];
    // Every row that is answered otherwise than on `ghost`, with both
    // answers: one assertion, so a fault names each row it turns.
    let (head, mut apart) = (f.head().await, Vec::new());
    for (who, hidden) in &cast {
        let id = &who.id;
        for &hidden in *hidden {
            // A link type's records are written at its owner's link doors,
            // and its name is refused at a record's doors by its form.
            let ghost = match hidden {
                LINKS => "sys.link.ghost.cites",
                _ => "ghost",
            };
            let doors = async |t: &str, on_record: &str, on_type: &str| {
                let mut rows = read_doors(c, who, t).await;
                if hidden != LINKS {
                    rows.extend(write_doors(c, who, t, on_record, on_type).await);
                    let (t, by) = (ty(t), "cites".into());
                    let linked = c.link(who, &t, &v4, &by, &v1, Patch::new(), opts());
                    rows.push(("link", format!("{:?}", linked.await)));
                    let unlinked = c.unlink(who, &t, &v1, &by, &v4, opts()).await;
                    rows.push(("unlink", format!("{unlinked:?}")));
                }
                rows
            };
            let there = doors(hidden, "approve", "pay").await;
            let none = doors(ghost, "haunt", "howl").await;
            for ((row, a), (_, b)) in there.into_iter().zip(none) {
                // A network door refuses an empty key itself, for any type.
                let found = b.contains("NotFound") || row == "upsert an empty key";
                assert!(found, "{row} on `{ghost}`: {b}");
                let b = b.replace(ghost, hidden).replace("haunt", "approve");
                let b = b.replace("howl", "pay");
                if a != b {
                    apart.push((format!("{row} as `{id}` on `{hidden}`"), a, b));
                }
            }
        }
        let described = c.describe(who, ReadAt::default()).await;
        let described = described.expect("describing the lake").data;
        let said = format!("{described:?} {:?}", tools(&described).expect("the tools"));
        if hidden.iter().any(|t| said.contains(&format!("\"{t}"))) {
            apart.push((format!("describe and the tools as `{id}`"), said, "".into()));
        }
    }
    let rows: Vec<&str> = apart.iter().map(|r| r.0.as_str()).collect();
    assert!(
        rows.is_empty(),
        "not as on `ghost`: {rows:#?}, answered {apart:?}"
    );
    assert_eq!(f.head().await, head, "nothing was written");

    // Who is shown a type by a grant, and each field as it is described.
    let writes = of("writes", Access::None, &[("wage", band(Access::Write))]);
    let tip = [("hint", one("tip", Access::Read))];
    let tips = of("tips", Access::None, &tip);
    let masked = [("code", Access::Masked), ("tip", Access::Masked)];
    for (who, t, want) in [
        (&writes, "wage", &[("band", Access::WriteOnly)][..]),
        // The grant names `tip`: that shows `hint`, and `code`, which
        // nobody named and `tip` is built from, is masked, and so `tip`.
        (&tips, "hint", &masked[..]),
    ] {
        let shown = c.describe_type(who, &ty(t), ReadAt::default()).await;
        let shown = shown.unwrap_or_else(|e| panic!("`{}` is shown `{t}`: {e:?}", who.id));
        let fields: Vec<_> = (shown.data.fields.iter())
            .map(|f| (f.name.as_str(), f.access))
            .collect();
        assert_eq!(fields, want, "`{t}` as `{}` is described", who.id);
    }
    let p = patch(&[("band", json!("3"))]);
    let wrote = c.upsert(&writes, &wage, &key("v1"), p, opts()).await;
    assert!(matches!(wrote, Ok(Outcome::Applied { .. })), "{wrote:?}");
}

/// A record event says `made` where its write found no live record under
/// the key and leaves it out otherwise (lakelands#572): made, changed,
/// retired and made again by a proposal made while it was live; a record
/// proposed before it is there and accepted after; one key twice in one
/// ingest. Each row is the key, the version and what the event says.
pub async fn a_record_event_says_whether_it_made_the_record(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, clerk) = (&f.catalog, &f.cast.clerk);
    let from = f.head().await.next();
    let status = |s: &str| [("status", json!(s))];
    let propose = async |k: &str| {
        let (ty, key, opts) = (ty(INVOICE), key(k), WriteOpts::default());
        let patch = patch(&status("approved"));
        let write = ProposedWrite::Upsert {
            ty,
            key,
            patch,
            opts,
        };
        match c.propose(clerk, write, "later".into()).await {
            Ok(Outcome::Proposed { proposal, .. }) => proposal,
            other => panic!("expected a proposal, got {other:?}"),
        }
    };
    f.write(clerk, "INV-1", &status("open")).await;
    f.write(clerk, "INV-1", &status("approved")).await;
    let (live, absent) = (propose("INV-1").await, propose("INV-2").await);
    let (invoice, k1, bank) = (ty(INVOICE), key("INV-1"), "bank".into());
    let retired = c.retire(&f.cast.owner, &invoice, &k1, WriteOpts::default());
    retired.await.expect("the owner retires");
    f.write(clerk, "INV-2", &status("open")).await;
    for proposal in [live, absent] {
        let accepted = c.accept(&f.cast.reviewer, &proposal).await;
        accepted.expect("the reviewer accepts");
    }
    let twice = ["1.00", "2.00"].map(|amount| SourceRecord {
        upstream: None,
        ty: ty(INVOICE),
        key: key("INV-3"),
        fields: patch(&[("amount", json!(amount))]),
        seen_at: None,
    });
    let fed = c.ingest(&f.cast.importer, &bank, stream::of(twice.into()));
    let acks = stream::collect(&mut fed.await.unwrap()).await;
    assert!(acks.iter().all(Result::is_ok), "{acks:?}");
    let feed = f.store.feed();
    let b = feed.poll(from, &FeedFilter::default(), 100).await.unwrap();
    let said: Vec<String> = (b.events.iter())
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted {
                key, version, made, ..
            }
            | EventBody::RecordIngested {
                key, version, made, ..
            } => Some(format!(
                "{key}:{}{}",
                version.0,
                if *made { " made" } else { "" }
            )),
            _ => None,
        })
        .collect();
    let want = [
        "INV-1:1 made",
        "INV-1:2",
        "INV-2:1 made",
        "INV-1:4 made",
        "INV-2:2",
    ];
    assert_eq!(said, [&want[..], &["INV-3:1 made", "INV-3:2"]].concat());
}

/// A required field is judged over the record whole, as the write leaves
/// it, whichever door writes the field: an ingest that leaves a required
/// authored field without a value is refused on it, and so is an upsert
/// that leaves a required `source` field without one, or a required
/// derived field whose expression reads null; a field that declares
/// `initial` is filled where the write makes its record, and the record
/// is made. A tightening of its value type the `initial` no longer fits
/// counts it a misfit and is not applied. A migration that makes a field required over a record without
/// a value counts the record and is not applied (050).
pub async fn a_required_field_is_judged_over_the_record_whole(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = Cast::of(store.lake()).owner;
    let src =
        |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": "source"});
    let required = |mut f: serde_json::Value| {
        f["required"] = json!(true);
        f
    };
    let state = json!({"name": "state", "type": {"kind": "text"}, "required": true});
    let note = json!({"name": "note", "type": {"kind": "text"}});
    let twice = json!({"name": "twice", "type": {"kind": "number"}, "required": true,
        "authority": "derived", "derived": "n * 2", "depends_on": ["n"]});
    let status = json!({"declare": "value_type", "name": "status", "kind": "text",
        "one_of": ["new", "paid"]});
    let status = serde_json::from_value(status).expect("a value type");
    c.declare(&own, status).await.expect("declaring `status`");
    for d in [
        json!({"name": "order", "key": ["id"],
            "fields": [required(src("id", "text")), src("total", "number"), state.clone()]}),
        json!({"name": "ticket", "key": ["id"],
            "fields": [required(src("id", "text")), src("total", "number"),
                       {"name": "state", "type": {"kind": "text", "value_type": "status"},
                        "required": true, "initial": "new"}]}),
        json!({"name": "memo", "fields": [required(src("total", "number")), note.clone()]}),
        json!({"name": "tally", "fields": [{"name": "n", "type": {"kind": "number"}}, twice,
            note.clone()]}),
        json!({"name": "task", "fields": [note.clone()]}),
    ] {
        let d = serde_json::from_value(d).expect("a type");
        c.declare(&own, Declaration::Type(d))
            .await
            .expect("declaring");
    }
    let narrower = |one_of: &[&str]| -> lakelands_api::ontology::ValueTypeDecl {
        let d = json!({"name": "status", "kind": "text", "one_of": one_of});
        serde_json::from_value(d).expect("a value type")
    };
    let report = c.migrate_value_type(&own, narrower(&["paid"])).await;
    let report = report.expect("the preflight").data;
    let id = report.report_id.clone();
    let misfits: Vec<_> = (report.fields.iter())
        .map(|u| (u.field.as_str(), u.misfits))
        .collect();
    assert_eq!(
        (report.fits, misfits),
        (false, vec![("state", 1)]),
        "{report:?}"
    );
    let applied = c
        .migrate_value_type_apply(&own, narrower(&["paid"]), &id)
        .await;
    assert!(applied.is_err(), "`initial` would not fit: {applied:?}");
    let ingest = async |t: &str, k: &str| {
        let record = SourceRecord {
            upstream: None,
            ty: ty(t),
            key: key(k),
            fields: patch(&[("id", json!(k)), ("total", json!(1))]),
            seen_at: None,
        };
        let mut acks = (c
            .ingest(&own, &"erp".into(), stream::of(vec![record]))
            .await)
            .expect("the stream opens");
        stream::collect(&mut acks).await.pop().expect("an answer")
    };
    let upsert = async |t: &str, k: &str, pairs: &[(&str, Value)]| {
        c.upsert(&own, &ty(t), &key(k), patch(pairs), WriteOpts::default())
            .await
    };
    let on = |e: &Error, name: &str| {
        matches!(refusal(e), (Some(f), Refusal::Constraint { message })
            if f.as_str() == name && message.contains(name))
    };
    let e = ingest("order", "o1")
        .await
        .expect_err("no value for `state`");
    assert!(on(&e, "state"), "an ingest without `state`: {e:?}");
    for pairs in [
        &[("state", json!("paid"))][..],
        &[("state", Value::Null)][..],
    ] {
        let e = upsert("order", "o1", pairs).await;
        assert!(
            e.is_err(),
            "no `order` `o1` was made, and {pairs:?} makes none: {e:?}"
        );
    }
    ingest("ticket", "t1").await.expect("`state` is filled");
    let t1 = c
        .get(&own, &ty("ticket"), get("t1"))
        .await
        .expect("`t1` reads");
    assert_eq!(field(&t1.data, "state"), Some(&json!("new")), "{t1:?}");
    let e = upsert("memo", "m1", &[("note", json!("hi"))])
        .await
        .expect_err("no `total`");
    assert!(on(&e, "total"), "an upsert without `total`: {e:?}");
    let e = upsert("tally", "s1", &[("note", json!("hi"))]).await;
    let e = e.expect_err("`twice` reads null");
    assert!(on(&e, "twice"), "an upsert whose `twice` reads null: {e:?}");
    upsert("task", "k1", &[("note", json!("hi"))])
        .await
        .expect("writing `k1`");
    let to = json!({"name": "task", "fields": [note, state]});
    let m = lakelands_api::write::Migration {
        to: serde_json::from_value(to).expect("a type"),
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let report = c
        .migrate(&own, m.clone())
        .await
        .expect("the preflight")
        .data;
    let counted = report
        .fields
        .iter()
        .map(|f| (f.field.as_str(), f.not_convertible));
    assert_eq!(counted.collect::<Vec<_>>(), [("state", 1)], "{report:?}");
    let applied = c.migrate_apply(&own, m, &report.report_id).await;
    assert!(
        applied.is_err(),
        "a lossy migration is not applied: {applied:?}"
    );
}

/// A field that declares `initial` makes no owner, whoever wrote it last
/// (050): a typed purge takes the record an author moved it on, and names
/// it in `moved`.
pub async fn a_field_that_declares_initial_makes_no_owner(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = Cast::of(store.lake()).owner;
    let d = json!({"name": "ticket", "fields": [
        {"name": "total", "type": {"kind": "number"}, "authority": "source"},
        {"name": "state", "type": {"kind": "text"}, "initial": "new"}]});
    let d = Declaration::Type(serde_json::from_value(d).expect("a type"));
    c.declare(&own, d).await.expect("declaring");
    let erp = SourceId::from("erp");
    for k in ["t1", "t2"] {
        let fields = patch(&[("total", json!(1))]);
        let record = SourceRecord {
            upstream: None,
            ty: ty("ticket"),
            key: key(k),
            fields,
            seen_at: None,
        };
        let acks = c.ingest(&own, &erp, stream::of(vec![record])).await;
        stream::collect(&mut acks.expect("the stream opens"))
            .await
            .pop()
            .expect("an answer")
            .expect("ingested");
    }
    let moved = patch(&[("state", json!("paid"))]);
    (c.upsert(&own, &ty("ticket"), &key("t1"), moved, WriteOpts::default())
        .await)
        .expect("an author moves `t1`");
    let purged = store.purge_source(&erp, Some(&ty("ticket")), None).await;
    let purged = purged.expect("`state` makes no owner");
    let moved = json!([{"type": "ticket", "field": "state", "records": 1, "first": ["t1"]}]);
    assert_eq!(
        (purged.retired, json!(purged.moved)),
        (2, moved),
        "{purged:?}"
    );
}

/// A retry of a write is answered that write before its writer is asked
/// what it is shown: a writer upserts, calls, proposes and retires under
/// an idempotency key, loses its grant, and repeats each. Under a key it
/// never used it is answered as on a type that is not declared.
pub async fn a_retry_is_answered_before_its_writer_is_asked_what_it_is_shown(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    declare_vault(&f).await;
    let (c, lake) = (&f.catalog, f.store.lake());
    let write = TypeGrants {
        default: Some(Access::Write),
        fields: Default::default(),
        ..Default::default()
    };
    let granted = Grants {
        types: [(ty("vault"), write)].into(),
        ..Grants::default()
    };
    let granted = principal(lake, PrincipalKind::Agent, "warden", granted);
    let gone = principal(lake, PrincipalKind::Agent, "warden", Grants::default());
    let keyed = |k: &str| WriteOpts {
        expected_version: None,
        idempotency_key: Some(k.into()),
    };
    let (vault, r1, seal) = (ty("vault"), key("r1"), "seal".into());
    let code = || patch(&[("code", json!("r1"))]);
    let mut first = Vec::new();
    for who in [&granted, &gone] {
        let proposed = ProposedWrite::Upsert {
            ty: vault.clone(),
            key: key("r2"),
            patch: patch(&[("code", json!("r2"))]),
            opts: keyed("retry-p"),
        };
        let answers = [
            c.upsert(who, &vault, &r1, code(), keyed("retry-u")).await,
            c.call(who, &seal, Some(&r1), patch(&[]), keyed("retry-c"))
                .await,
            c.propose(who, proposed, "probing".into()).await,
            c.retire(who, &vault, &r1, keyed("retry-r")).await,
        ];
        let answers = answers.map(|a| a.expect("the write, and its retry, are answered"));
        if first.is_empty() {
            first.extend(answers);
        } else {
            assert_eq!(first, answers, "each retry is answered its first write");
        }
    }
    let unused = c.upsert(&gone, &vault, &r1, code(), keyed("never")).await;
    let ghost = c
        .upsert(&gone, &ty("ghost"), &r1, code(), keyed("never"))
        .await;
    let (unused, ghost) = (answered(unused), answered(ghost));
    assert_eq!(unused.replace("vault", "ghost"), ghost);
}

/// A `vector_index.<type>` setting asks for an index on vector fields its
/// type declares, and is refused naming anything else; `describe_type`
/// names the fields asked for, and a search over one says it ranked by the
/// index, `index` or `hybrid (index)`, at the head and at an earlier
/// position alike, since the index is the head's; every other field is
/// ranked by `vector`
/// (`docs/decisions/043-an-index-asked-for-answers-its-set.md`). A hybrid
/// search lists its axes, each counted, a text that matches nothing at 0,
/// and a search of one axis lists its own without a count (lakelands#560);
/// a type's `search_weights` weighs what a search does not (lakelands#561).
pub async fn a_vector_index_setting_names_vector_fields_and_search_says_it_ranked_by_the_index(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let vector = |name: &str| FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..text(name)
    };
    let doc = TypeDecl {
        annotations: Default::default(),
        name: "doc".into(),
        key: Vec::new(),
        fields: vec![
            FieldDecl {
                searchable: true,
                ..text("label")
            },
            vector("v"),
            vector("w"),
        ],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(doc))
        .await
        .expect("the owner declares `doc`");
    f.catalog
        .upsert(
            owner,
            &ty("doc"),
            &key("a"),
            patch(&[
                ("label", json!("quay")),
                ("v", json!([1.0, 0.0])),
                ("w", json!([0.0, 1.0])),
            ]),
            WriteOpts::default(),
        )
        .await
        .expect("a doc is written");
    let before = f.head().await;
    let set = |value: Value| {
        let catalog = f.catalog.clone();
        async move {
            catalog
                .upsert(
                    owner,
                    &ty("sys.setting"),
                    &key("vector_index.doc"),
                    patch(&[("key", json!("vector_index.doc")), ("value", value)]),
                    WriteOpts::default(),
                )
                .await
        }
    };
    for (value, at_fault) in [
        (json!(["label"]), "label"),
        (json!(["v", "nope"]), "nope"),
        (json!("v"), "value"),
    ] {
        let e = set(value.clone())
            .await
            .expect_err("a setting naming what no vector index keeps is refused");
        let (field, reason) = refusal(&e);
        assert!(
            matches!(reason, Refusal::Invalid { .. }),
            "`vector_index.doc` = {value}: expected invalid, got {reason:?}"
        );
        assert_eq!(
            field.as_ref().map(FieldName::as_str),
            Some(at_fault),
            "`vector_index.doc` = {value}: the refusal names the field at fault"
        );
    }
    let e = f
        .catalog
        .upsert(
            owner,
            &ty("sys.setting"),
            &key("vector_index.gauge"),
            patch(&[
                ("key", json!("vector_index.gauge")),
                ("value", json!(["v"])),
            ]),
            WriteOpts::default(),
        )
        .await
        .expect_err("a setting for an undeclared type is refused");
    assert_eq!(
        refusal(&e).0.as_ref().map(FieldName::as_str),
        Some("key"),
        "the refusal names the key"
    );
    let indexed = || {
        let catalog = f.catalog.clone();
        async move {
            catalog
                .describe_type(owner, &ty("doc"), ReadAt::default())
                .await
                .expect("the type is described")
                .data
                .lifecycle
                .indexed
        }
    };
    let ranked_by = |mode: SearchMode, as_of: Option<Position>| {
        let catalog = f.catalog.clone();
        async move {
            catalog
                .search(
                    owner,
                    Search {
                        ty: ty("doc"),
                        mode,
                        k: 5,
                        cursor: None,
                        filter: None,
                        with_meta: false,
                        at: ReadAt {
                            as_of,
                            valid_at: None,
                        },
                    },
                )
                .await
                .expect("the search answers")
                .data
                .ranked_by
        }
    };
    let by_vector = |field: &str| SearchMode::Vector {
        field: field.into(),
        vector: vec![1.0, 0.0],
        weight: None,
        also: Vec::new(),
    };
    let hybrid = || SearchMode::Hybrid {
        text: "quay".into(),
        field: "v".into(),
        vector: Some(vec![1.0, 0.0]),
        fields: None,
        weight: None,
        text_weight: None,
    };
    assert!(indexed().await.is_empty(), "no setting, nothing indexed");
    assert_eq!(ranked_by(by_vector("v"), None).await, "vector");
    let axes = |mode: SearchMode| {
        let (catalog, ty) = (f.catalog.clone(), ty("doc"));
        async move {
            let s = Search {
                k: 5,
                cursor: None,
                filter: None,
                with_meta: false,
                at: ReadAt::default(),
                ty,
                mode,
            };
            let hits = catalog.search(owner, s).await.expect("the search answers");
            let said = hits.data.axes.into_iter();
            said.map(|a| (a.axis, a.field.map(|f| f.to_string()), a.candidates))
                .collect::<Vec<_>>()
        }
    };
    let quiet = SearchMode::Hybrid {
        text: "zzz".into(),
        field: "v".into(),
        vector: Some(vec![1.0, 0.0]),
        fields: None,
        weight: None,
        text_weight: None,
    };
    let (lexical, v) = (String::from("lexical"), Some(String::from("v")));
    let vector = || (String::from("vector"), v.clone());
    let fused = |text| {
        vec![
            (lexical.clone(), None, Some(text)),
            (vector().0, vector().1, Some(1)),
        ]
    };
    assert_eq!(axes(hybrid()).await, fused(1), "the text matches `a`");
    assert_eq!(axes(quiet).await, fused(0), "the text matches nothing");
    assert_eq!(
        axes(by_vector("v")).await,
        vec![(vector().0, vector().1, None)]
    );
    // The type's `search_weights` weighs what a search does not
    // (lakelands#561), and the axes say so.
    let weights = json!({"fields": {"v": 0.5}, "text_weight": 0.25});
    let (k, setting) = (key("search_weights.doc"), ty("sys.setting"));
    let fields = patch(&[("key", json!("search_weights.doc")), ("value", weights)]);
    let written = f
        .catalog
        .upsert(owner, &setting, &k, fields, WriteOpts::default());
    written.await.expect("the weights are a setting");
    let s = Search {
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
        ty: ty("doc"),
        mode: hybrid(),
    };
    let hits = f
        .catalog
        .search(owner, s)
        .await
        .expect("the search answers");
    let weighed: Vec<_> = hits.data.axes.iter().map(|a| a.weight).collect();
    assert_eq!(
        weighed,
        [Some(0.25), Some(0.5)],
        "the setting's weights rank"
    );
    set(json!(["v"]))
        .await
        .expect("a setting naming a declared vector field applies");
    assert_eq!(indexed().await, vec![FieldName::from("v")]);
    assert_eq!(ranked_by(by_vector("v"), None).await, "index");
    assert_eq!(
        ranked_by(by_vector("v"), Some(before)).await,
        "index",
        "an earlier position is ranked by the head's index, and says so"
    );
    assert_eq!(ranked_by(hybrid(), None).await, "hybrid (index)");
    assert_eq!(
        ranked_by(by_vector("w"), None).await,
        "vector",
        "a field the setting does not name is ranked by `vector`"
    );
    f.catalog
        .retire(
            owner,
            &ty("sys.setting"),
            &key("vector_index.doc"),
            WriteOpts::default(),
        )
        .await
        .expect("the setting retires");
    assert!(
        indexed().await.is_empty(),
        "the setting retired, nothing indexed"
    );
    assert_eq!(ranked_by(by_vector("v"), None).await, "vector");
}

/// `latest` orders by each record's latest change and takes no field;
/// every other order names its field; a view's records, computed and
/// without events of their own, are not ordered by `latest`
/// (`docs/decisions/029-every-answer-has-one-order.md`).
pub async fn an_order_by_latest_takes_no_field_and_no_view(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let by = |field: &str, by: OrderBy| Find {
        order_by: vec![Order {
            field: field.into(),
            by,
            dir: Direction::Desc,
        }],
        ..Find::default()
    };
    f.catalog
        .find(owner, &ty(INVOICE), by("", OrderBy::Latest))
        .await
        .expect("`latest` without a field orders a stored type");
    for (find, at_fault, what) in [
        (
            by("amount", OrderBy::Latest),
            Some("amount"),
            "`latest` with a field",
        ),
        (by("", OrderBy::KnownAt), None, "`known_at` without a field"),
    ] {
        let e = f
            .catalog
            .find(owner, &ty(INVOICE), find)
            .await
            .expect_err(what);
        let (field, reason) = refusal(&e);
        assert!(
            matches!(reason, Refusal::Invalid { .. }),
            "{what}: expected invalid, got {reason:?}"
        );
        assert_eq!(
            field.as_ref().map(FieldName::as_str),
            at_fault,
            "{what}: the refusal names the field at fault, if any"
        );
    }
    let totals = TypeDecl {
        annotations: Default::default(),
        name: "partner_totals".into(),
        key: vec!["partner".into()],
        fields: vec![text("partner"), text("count")],
        relations: Vec::new(),
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            r#where: None,
            group_by: vec!["partner".into()],
            measures: vec![Measure {
                name: "count".into(),
                expr: "count()".into(),
            }],
            params: Default::default(),
            merge: Default::default(),
        })),
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(totals))
        .await
        .expect("the owner declares a view");
    let e = f
        .catalog
        .find(owner, &ty("partner_totals"), by("", OrderBy::Latest))
        .await
        .expect_err("a view is not ordered by `latest`");
    assert!(
        matches!(refusal(&e).1, Refusal::Invalid { .. }),
        "a view ordered by `latest`: expected invalid, got {e:?}"
    );
}

/// A view's parameters are bound by each read, through every door: `find`
/// and `get` take `params`, a pattern started on the view takes
/// `start.params`, and the same view read with two values answers two
/// windows. A read that leaves a parameter out is refused `invalid`,
/// naming the view and the parameter.
pub async fn a_view_reads_the_parameters_each_read_binds(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    for (k, status) in [("INV-1", "open"), ("INV-2", "open"), ("INV-3", "approved")] {
        f.write(&f.cast.clerk, k, &[("status", json!(status))])
            .await;
    }
    let by_status = TypeDecl {
        name: "by_status".into(),
        key: vec!["status".into()],
        fields: vec![
            text("status"),
            FieldDecl {
                ty: FieldType::kind(StorageKind::Number),
                ..text("count")
            },
        ],
        view: Some(Box::new(ViewDecl {
            source: INVOICE.into(),
            params: vec![ViewParam {
                name: "state".into(),
                kind: StorageKind::Text,
            }],
            r#where: Some("status = param('state')".into()),
            group_by: vec!["status".into()],
            measures: vec![Measure {
                name: "count".into(),
                expr: "count()".into(),
            }],
            merge: Default::default(),
        })),
        field_meta: true,
        ..Default::default()
    };
    f.catalog
        .declare(owner, Declaration::Type(by_status))
        .await
        .expect("the owner declares a view with a parameter");
    let view = ty("by_status");
    let params = |state: &str| BTreeMap::from([("state".into(), json!(state))]);
    // `find`: one window per value bound.
    for (state, count) in [("open", 2), ("approved", 1)] {
        let found = f
            .catalog
            .find(
                owner,
                &view,
                Find {
                    params: params(state),
                    ..Find::default()
                },
            )
            .await
            .unwrap_or_else(|e| panic!("find with `state` = {state}: {e:?}"));
        let Found::Records { rows, .. } = found.data else {
            panic!("find on a view answers its records");
        };
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.key.as_str().to_owned(), r.fields.get("count").cloned()))
            .collect();
        assert_eq!(
            got,
            vec![(state.to_owned(), Some(json!(count)))],
            "the view's window for `state` = {state}"
        );
    }
    // `get`: the record of the window bound, and none outside it.
    let get = |state: &str, key: &str| Get {
        key: key.into(),
        with_meta: false,
        params: params(state),
        at: ReadAt::default(),
    };
    let open = f
        .catalog
        .get(owner, &view, get("open", "open"))
        .await
        .expect("get binds the parameter");
    assert_eq!(open.data.fields.get("count"), Some(&json!(2)));
    let e = f
        .catalog
        .get(owner, &view, get("approved", "open"))
        .await
        .expect_err("`open` is not in the window of `approved`");
    assert!(
        matches!(e, Error::NotFound { .. }),
        "a key outside the window bound is not found: {e:?}"
    );
    // A pattern started on the view binds them as `start.params`.
    let pattern: Pattern = serde_json::from_value(json!({
        "start": {"type": "by_status", "params": {"state": "approved"}}
    }))
    .expect("a pattern starts on a view with its parameters");
    let walked = f.catalog.query(owner, pattern).await.unwrap();
    let json = serde_json::to_value(&walked.data).unwrap();
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(1), "{json}");
    assert_eq!(json["rows"][0]["by_status.count"], json!(1), "{json}");
    // Left out, the parameter is refused, named with its view.
    let e = f
        .catalog
        .find(owner, &view, Find::default())
        .await
        .expect_err("a read of the view binds every parameter");
    let (_, reason) = refusal(&e);
    let Refusal::Invalid { message } = reason else {
        panic!("a missing parameter is invalid: {reason:?}");
    };
    for name in ["by_status", "state"] {
        assert!(message.contains(name), "{message:?} names `{name}`");
    }
}

/// A read that binds a parameter its type does not declare is refused as
/// invalid, naming it: a type that is not a view declares none.
pub async fn a_parameter_its_type_does_not_declare_is_refused(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let params: lakelands_api::Params = [("anchor".into(), json!("2026-06-30"))].into();
    let found = f
        .catalog
        .find(
            &f.cast.owner,
            &ty(INVOICE),
            Find {
                params: params.clone(),
                ..Find::default()
            },
        )
        .await;
    let got = Get {
        params,
        ..get("INV-1")
    };
    let got = f.catalog.get(&f.cast.owner, &ty(INVOICE), got).await;
    for e in [found.map(|_| ()), got.map(|_| ())] {
        let e = e.expect_err("a parameter the type does not declare is refused");
        let (_, why) = refusal(&e);
        assert!(
            matches!(why, Refusal::Invalid { message } if message.contains("anchor")),
            "refused as invalid, naming `anchor`: {e:?}"
        );
    }
}

/// A trigger may watch the link type of a relation carried by link
/// records; the link type of a relation carried by a reference field does
/// not exist, and a trigger naming it is refused on `on`
/// (`docs/decisions/035-a-link-is-a-record.md`).
pub async fn a_trigger_watches_a_link_type_and_not_a_reference_relations(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let doc = TypeDecl {
        annotations: Default::default(),
        name: "doc".into(),
        key: Vec::new(),
        fields: vec![text("title")],
        relations: vec![RelationDecl {
            derived: None,
            annotations: Default::default(),
            name: "signed_by".into(),
            target: PARTNER.into(),
            cardinality: Default::default(),
            inverse: None,
            via: None,
            fields: Vec::new(),
            authority: Default::default(),
            description: None,
        }],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(doc))
        .await
        .expect("the owner declares a type with a relation carried by link records");
    let trigger = |name: &str, on: &str| {
        Declaration::Trigger(TriggerDecl {
            max_depth: None,
            name: name.into(),
            on: on.into(),
            when: None,
            description: None,
        })
    };
    f.catalog
        .declare(owner, trigger("signed", "sys.link.doc.signed_by"))
        .await
        .expect("a trigger watches the link type of a relation carried by link records");
    let e = f
        .catalog
        .declare(owner, trigger("billed", "sys.link.invoice.billed_by"))
        .await
        .expect_err("a trigger on the link type of a reference relation is refused");
    let (field, reason) = refusal(&e);
    assert!(
        matches!(reason, Refusal::Invalid { .. }),
        "expected invalid, got {reason:?}"
    );
    assert_eq!(field.as_ref().map(FieldName::as_str), Some("on"));
}

/// The whole-graph measures of the pattern query
/// (`docs/decisions/026-the-store-walks-the-graph.md`): `degree` counts a
/// record's links over a via relation and a link relation to ends that
/// are live, highest first and ties by key; `components` partitions what
/// the start reaches, both ways, largest first, across types; and
/// components walking one way are refused.
pub async fn degree_and_components_answer_the_graph_in_one_order(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let cast = Cast::of(store.lake());
    let own = &cast.owner;
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    c.declare(
        own,
        decl(json!({"declare": "type", "name": "person", "fields": [
            {"name": "name", "type": {"kind": "text"}}]})),
    )
    .await
    .unwrap();
    c.declare(
        own,
        decl(json!({"declare": "type", "name": "doc", "fields": [
            {"name": "title", "type": {"kind": "text"}},
            {"name": "author", "type": {"kind": "reference", "target": "person"}}],
            "relations": [
                {"name": "written_by", "target": "person", "cardinality": "many_to_one",
                 "via": "author"},
                {"name": "signed_by", "target": "person", "cardinality": "many_to_many"}]})),
    )
    .await
    .unwrap();
    let put = |t: &str, k: &str, pairs: &[(&str, Value)]| {
        let (c, own, t, k, p) = (c.clone(), own.clone(), ty(t), key(k), patch(pairs));
        async move {
            c.upsert(&own, &t, &k, p, WriteOpts::default())
                .await
                .unwrap()
        }
    };
    for p in ["p1", "p2", "p3"] {
        put("person", p, &[("name", json!(p))]).await;
    }
    put(
        "doc",
        "d1",
        &[("title", json!("d1")), ("author", json!("p1"))],
    )
    .await;
    put(
        "doc",
        "d2",
        &[("title", json!("d2")), ("author", json!("p1"))],
    )
    .await;
    put("doc", "d3", &[("title", json!("d3"))]).await;
    for (d, p) in [("d1", "p2"), ("d2", "p1"), ("d2", "p3"), ("d3", "p2")] {
        c.link(
            own,
            &ty("doc"),
            &key(d),
            &"signed_by".into(),
            &key(p),
            Patch::new(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    c.retire(own, &ty("person"), &key("p3"), WriteOpts::default())
        .await
        .unwrap();
    let measure = |steps: Vec<Step>, project: lakelands_api::query::Projection| {
        let mut p = Pattern::traverse(ty("doc"), key("unused"), steps);
        p.start.key = None;
        p.project = project;
        p
    };
    let step = |r: &str, walk: Walk| Step {
        relation: r.into(),
        walk,
        ty: None,
        filter: None,
        depth: Default::default(),
    };
    let rows = |p: Pattern| {
        let (c, own) = (c.clone(), own.clone());
        async move {
            let reply = c.query(&own, p).await.expect("a measure answers");
            let lakelands_api::query::QueryResult::Rows { rows, .. } = reply.data else {
                panic!("a measure answers rows");
            };
            rows.iter()
                .map(|r| {
                    let s = |k: &str| match r.get(k) {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => "-".into(),
                    };
                    if r.contains_key("degree") {
                        format!("{}:{}={}", s("type"), s("key"), s("degree"))
                    } else {
                        format!(
                            "{}/{} {}:{}",
                            s("component"),
                            s("size"),
                            s("type"),
                            s("key")
                        )
                    }
                })
                .collect::<Vec<_>>()
        }
    };
    use lakelands_api::query::Projection;
    let degree = rows(measure(
        vec![step("written_by", Walk::Out), step("signed_by", Walk::Out)],
        Projection::Degree,
    ))
    .await;
    assert_eq!(
        degree,
        ["doc:d1=2", "doc:d2=2", "doc:d3=1"],
        "d2's link to the retired p3 is not counted; ties by key"
    );
    let components = rows(measure(
        vec![
            step("written_by", Walk::Both),
            step("signed_by", Walk::Both),
        ],
        Projection::Components,
    ))
    .await;
    assert_eq!(
        components,
        [
            "0/5 doc:d1",
            "0/5 doc:d2",
            "0/5 doc:d3",
            "0/5 person:p1",
            "0/5 person:p2",
        ],
        "the docs and the people they reach are one component, across types"
    );
    // A measure's page may be larger than `max_limit`, up to the lake's
    // `max_graph_nodes`: it answers the same rows.
    let mut whole = measure(
        vec![
            step("written_by", Walk::Both),
            step("signed_by", Walk::Both),
        ],
        Projection::Components,
    );
    whole.limit = Some(Limits::default().max_limit + 1);
    assert_eq!(rows(whole).await, components, "one page past `max_limit`");
    let e = c
        .query(
            own,
            measure(vec![step("signed_by", Walk::Out)], Projection::Components),
        )
        .await
        .expect_err("components walking one way are refused");
    assert!(
        matches!(refusal(&e).1, Refusal::Invalid { .. }),
        "expected invalid, got {e:?}"
    );
}

/// A search probes what it searches: the searchable fields it names, or
/// those its caller reads, and the vector field it names. A named field
/// the caller may not read refuses the search whole, naming the field, in
/// every mode; one it does not name is never searched, so a hybrid search
/// is then answered from its vector half alone
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
pub async fn a_search_over_a_field_the_caller_may_not_read_is_refused_in_every_mode(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let doc = TypeDecl {
        annotations: Default::default(),
        name: "doc".into(),
        key: Vec::new(),
        fields: vec![
            FieldDecl {
                searchable: true,
                ..text("body")
            },
            FieldDecl {
                ty: FieldType {
                    dimensions: Some(2),
                    ..FieldType::kind(StorageKind::Vector)
                },
                ..text("v")
            },
        ],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(doc))
        .await
        .expect("the owner declares `doc`");
    f.catalog
        .upsert(
            owner,
            &ty("doc"),
            &key("a"),
            patch(&[("body", json!("quayside")), ("v", json!([0.0, 1.0]))]),
            WriteOpts::default(),
        )
        .await
        .expect("a doc is written");
    let search = |mode: SearchMode| Search {
        ty: ty("doc"),
        mode,
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let modes = |fields: Option<Vec<FieldName>>| {
        [
            (
                "lexical",
                SearchMode::Lexical {
                    text: "quayside".into(),
                    fields: fields.clone(),
                },
            ),
            (
                "hybrid without a vector",
                SearchMode::Hybrid {
                    text: "quayside".into(),
                    field: "v".into(),
                    vector: None,
                    fields: fields.clone(),
                    weight: None,
                    text_weight: None,
                },
            ),
            (
                "hybrid along `a`",
                SearchMode::Hybrid {
                    text: "quayside".into(),
                    field: "v".into(),
                    vector: Some(vec![0.0, 1.0]),
                    fields: fields.clone(),
                    weight: None,
                    text_weight: None,
                },
            ),
        ]
    };
    for (what, mode) in modes(None) {
        let hits = f
            .catalog
            .search(owner, search(mode))
            .await
            .unwrap_or_else(|e| panic!("the owner's {what} search answers: {e:?}"))
            .data
            .hits;
        assert_eq!(
            hits.iter()
                .map(|h| h.record.key.as_str())
                .collect::<Vec<_>>(),
            ["a"],
            "the owner's {what} search finds `a`"
        );
    }
    // The clerk reads `v` but sees the searchable `body` only masked.
    for (what, mode) in modes(Some(vec!["body".into()])) {
        match f.catalog.search(&f.cast.clerk, search(mode)).await {
            Err(e) => {
                let (field, reason) = refusal(&e);
                assert!(
                    matches!(reason, Refusal::NotGranted)
                        && field.as_ref().map(FieldName::as_str) == Some("body"),
                    "the clerk's {what} search is refused as not granted, naming `body`; \
                     got {e:?}"
                );
            }
            Ok(r) => panic!(
                "the clerk's {what} search is refused, naming `body`; it answered {:?}",
                r.data
                    .hits
                    .iter()
                    .map(|h| h.record.key.as_str())
                    .collect::<Vec<_>>()
            ),
        }
    }
    // Naming none, it searches no field: only the vector half answers.
    for ((what, mode), want) in modes(None).into_iter().zip([&[][..], &[], &["a"]]) {
        let found = f.catalog.search(&f.cast.clerk, search(mode)).await;
        let found = found.unwrap_or_else(|e| panic!("the clerk's {what} search answers: {e:?}"));
        let keys: Vec<&str> = found
            .data
            .hits
            .iter()
            .map(|h| h.record.key.as_str())
            .collect();
        assert_eq!(keys, want, "the clerk's {what} search naming no field");
    }
}

/// A lexical or hybrid search that names `fields` matches in those
/// fields only and probes only them (024, 025): the clerk, who sees the
/// searchable `body` only masked, is answered when it names `title`. A
/// named field it may not read is not granted, one that is not
/// searchable or not declared is invalid, and naming none is invalid.
pub async fn a_search_limited_to_named_fields_matches_and_probes_only_those(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let clerk = &f.cast.clerk;
    let doc = TypeDecl {
        name: "doc".into(),
        key: Vec::new(),
        fields: vec![
            FieldDecl {
                searchable: true,
                ..text("title")
            },
            FieldDecl {
                searchable: true,
                ..text("body")
            },
            text("label"),
        ],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
        annotations: Default::default(),
    };
    f.catalog
        .declare(owner, Declaration::Type(doc))
        .await
        .expect("the owner declares `doc`");
    for (k, fields) in [
        ("a", [("title", "quayside"), ("body", "harbour")]),
        ("b", [("body", "quayside"), ("label", "quayside")]),
    ] {
        let fields: Vec<(&str, Value)> = fields.iter().map(|(n, v)| (*n, json!(v))).collect();
        f.catalog
            .upsert(
                owner,
                &ty("doc"),
                &key(k),
                patch(&fields),
                WriteOpts::default(),
            )
            .await
            .expect("a doc is written");
    }
    let named = |names: &[&str]| Some(names.iter().map(|&n| FieldName::from(n)).collect());
    let lexical = |names: Option<Vec<FieldName>>| Search {
        ty: ty("doc"),
        mode: SearchMode::Lexical {
            text: "quayside".into(),
            fields: names,
        },
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let found = |hits: &lakelands_api::query::Hits| {
        hits.hits
            .iter()
            .map(|h| h.record.key.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    let every = f
        .catalog
        .search(owner, lexical(None))
        .await
        .expect("the owner searches every searchable field")
        .data;
    assert_eq!(found(&every), ["a", "b"], "the owner finds both docs");
    let title = f
        .catalog
        .search(owner, lexical(named(&["title"])))
        .await
        .expect("the owner searches `title`")
        .data;
    assert_eq!(found(&title), ["a"], "only `a` holds the word in `title`");
    let theirs = f
        .catalog
        .search(clerk, lexical(named(&["title"])))
        .await
        .expect("the clerk searches `title` beside the masked `body`")
        .data;
    assert_eq!(found(&theirs), ["a"], "the clerk finds `a` by its title");
    assert!(
        theirs.hits[0]
            .record
            .masked
            .iter()
            .any(|f| f.as_str() == "body"),
        "the clerk sees `body` masked in the hit"
    );
    for (who, names, field, granted, what) in [
        (
            clerk,
            named(&["body"]),
            Some("body"),
            false,
            "`body`, which the clerk sees masked",
        ),
        (
            clerk,
            named(&["title", "body"]),
            Some("body"),
            false,
            "`title` and `body`",
        ),
        (
            owner,
            named(&["label"]),
            Some("label"),
            true,
            "`label`, which is not searchable",
        ),
        (
            owner,
            named(&["nope"]),
            Some("nope"),
            true,
            "`nope`, which `doc` does not declare",
        ),
        (owner, named(&[]), None, true, "no field"),
    ] {
        match f.catalog.search(who, lexical(names)).await {
            Err(e) => {
                let (named, reason) = refusal(&e);
                assert!(
                    named.as_ref().map(FieldName::as_str) == field
                        && if granted {
                            matches!(reason, Refusal::Invalid { .. })
                        } else {
                            matches!(reason, Refusal::NotGranted)
                        },
                    "a search in {what} is refused naming {field:?}; got {e:?}"
                );
            }
            Ok(r) => panic!(
                "a search in {what} is refused naming {field:?}; it answered {:?}",
                found(&r.data)
            ),
        }
    }
}

/// A hybrid search with a vector fuses its lexical and its vector ranking
/// by reciprocal rank, the lexical first, ties by key, and says `hybrid`:
/// `b` alone holds the word and is the vector's third, so it ranks first
/// at 1/60 + 1/62, above `a` (the vector's first, 1/60) and `c` (1/61).
/// Each ranking is weighed as the search asks, a vector search over a
/// second field fuses the same way, and a weight that cannot weigh, or a
/// field named twice, is refused as invalid.
pub async fn a_hybrid_search_fuses_its_text_and_vector_ranks(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let doc = TypeDecl {
        annotations: Default::default(),
        name: "doc".into(),
        key: Vec::new(),
        fields: vec![
            FieldDecl {
                searchable: true,
                ..text("title")
            },
            vector("v"),
            vector("w"),
        ],
        relations: Vec::new(),
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    f.catalog
        .declare(owner, Declaration::Type(doc))
        .await
        .expect("the owner declares `doc`");
    // In `w` the order is c, b, and `a` holds no `w`.
    for (k, title, v, w) in [
        ("a", "quay", [1.0, 0.0], None),
        ("b", "harbour", [0.5, 0.5], Some([0.5, 0.5])),
        ("c", "quay", [0.8, 0.2], Some([1.0, 0.0])),
    ] {
        let mut fields = vec![("title", json!(title)), ("v", json!(v))];
        fields.extend(w.map(|w| ("w", json!(w))));
        f.catalog
            .upsert(
                owner,
                &ty("doc"),
                &key(k),
                patch(&fields),
                WriteOpts::default(),
            )
            .await
            .expect("a doc is written");
    }
    let search = |mode| Search {
        ty: ty("doc"),
        mode,
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let hybrid = |text_weight, weight| SearchMode::Hybrid {
        text: "harbour".into(),
        field: "v".into(),
        vector: Some(vec![1.0, 0.0]),
        fields: None,
        weight,
        text_weight,
    };
    let fields = |weight, also: &str, also_weight| SearchMode::Vector {
        field: "v".into(),
        vector: vec![1.0, 0.0],
        weight,
        also: vec![VectorTerm {
            field: also.into(),
            vector: vec![1.0, 0.0],
            weight: also_weight,
        }],
    };
    let (t, v) = (1.0 / 60.0, [1.0 / 60.0, 1.0 / 61.0, 1.0 / 62.0]);
    for (mode, by, want) in [
        (
            hybrid(None, None),
            "hybrid",
            [("b", t + v[2]), ("a", v[0]), ("c", v[1])],
        ),
        (
            hybrid(Some(1.0), Some(1.0)),
            "hybrid",
            [("b", t + v[2]), ("a", v[0]), ("c", v[1])],
        ),
        (
            hybrid(None, Some(100.0)),
            "hybrid",
            [
                ("a", 100.0 / 60.0),
                ("c", 100.0 / 61.0),
                ("b", t + 100.0 / 62.0),
            ],
        ),
        (
            fields(Some(2.0), "w", None),
            "vector",
            [
                ("c", 2.0 / 61.0 + 1.0 / 60.0),
                ("b", 2.0 / 62.0 + 1.0 / 61.0),
                ("a", 2.0 / 60.0),
            ],
        ),
    ] {
        let what = format!("{mode:?}");
        let got = f
            .catalog
            .search(owner, search(mode))
            .await
            .expect(&what)
            .data;
        let scored: Vec<(&str, f64)> = (got.hits.iter())
            .map(|h| (h.record.key.as_str(), h.score))
            .collect();
        assert_eq!(
            (scored.as_slice(), got.ranked_by.as_str()),
            (want.as_slice(), by),
            "{what} fuses its rankings by weighted reciprocal rank"
        );
    }
    let lone = SearchMode::Vector {
        field: "v".into(),
        vector: vec![1.0, 0.0],
        weight: Some(2.0),
        also: Vec::new(),
    };
    for (mode, on) in [
        (lone, Some("v")),
        (fields(Some(0.0), "w", None), Some("v")),
        (fields(None, "w", Some(1_000_001.0)), Some("w")),
        (fields(None, "v", None), Some("v")),
        (hybrid(Some(-1.0), None), None),
    ] {
        let what = format!("{mode:?}");
        match f.catalog.search(owner, search(mode)).await {
            Err(Error::Refused {
                field,
                reason: Refusal::Invalid { .. },
            }) => {
                assert_eq!(
                    field.as_ref().map(|f| f.as_str()),
                    on,
                    "{what} names its field"
                )
            }
            other => panic!("{what} must be refused as invalid; it answered {other:?}"),
        }
    }
}

/// A grant's `only` admits its holder the records whose fields hold its
/// values alone (051): another's record is not found, `find`, its total,
/// a walk and a degree over the links answer its own, a proposal on
/// another's record is listed without its write, every write that reaches
/// the type is refused as not granted naming no field and appends nothing,
/// and `describe_type` tells it the pairs.
pub async fn a_grant_admits_only_the_records_whose_fields_hold_its_values(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let (opts, none) = (WriteOpts::default, Patch::new);
    for d in [
        json!({"declare": "type", "name": "trip", "fields": [
            {"name": "title", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "claim", "fields": [
            {"name": "holder", "type": {"kind": "text"}},
            {"name": "amount", "type": {"kind": "number"}}],
            "relations": [{"name": "on_trip", "target": "trip"}]}),
    ] {
        let d = serde_json::from_value::<Declaration>(d).expect("a declaration reads");
        c.declare(own, d).await.expect("the owner declares");
    }
    let p = patch(&[("title", json!("rome"))]);
    c.upsert(own, &ty("trip"), &key("t1"), p, opts())
        .await
        .expect("the owner writes `t1`");
    let (claim, on, t1) = (ty("claim"), "on_trip".into(), key("t1"));
    for (k, holder) in [("c1", "alice"), ("c2", "bob"), ("c3", "alice")] {
        let p = patch(&[("holder", json!(holder)), ("amount", json!(10))]);
        c.upsert(own, &claim, &key(k), p, opts())
            .await
            .expect("the owner writes a claim");
        c.link(own, &claim, &key(k), &on, &t1, none(), opts())
            .await
            .expect("the owner links a claim to `t1`");
    }
    // Bob proposes to retire his `c2`: alice, a reviewer, is listed it
    // without its write.
    let bob = serde_json::from_value(json!({"default": "write"})).expect("grants read");
    let bob = &principal(store.lake(), PrincipalKind::Agent, "bob", bob);
    let retire = json!({"write": "retire", "type": "claim", "key": "c2"});
    let retire = serde_json::from_value(retire).expect("a write reads");
    c.propose(bob, retire, "bob's".into())
        .await
        .expect("bob proposes");
    let grants = json!({"default": "write", "review": true,
                        "types": {"claim": {"only": {"holder": "alice"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let alice = &principal(store.lake(), PrincipalKind::Agent, "alice", grants);
    let mut wrong = Vec::new();
    let other = c.get(alice, &claim, get("c2")).await;
    if !matches!(other, Err(Error::NotFound { .. })) {
        wrong.push(format!("`get` of bob's `c2`: {other:?}"));
    }
    let find = Find {
        total: true,
        ..Find::default()
    };
    let found = match c.find(alice, &claim, find).await.map(|r| r.data) {
        Ok(Found::Records { rows, total, .. }) => {
            let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
            format!("{} total={total:?}", keys.join(" "))
        }
        other => format!("{other:?}"),
    };
    if found != "c1 c3 total=Some(2)" {
        wrong.push(format!("`find` with its total: {found}"));
    }
    let step = Step {
        relation: on.clone(),
        walk: Walk::In,
        ty: None,
        filter: None,
        depth: Default::default(),
    };
    let walk = Pattern::traverse(ty("trip"), t1.clone(), vec![step]);
    let degree = Pattern {
        project: lakelands_api::query::Projection::Degree,
        ..walk.clone()
    };
    for (what, pattern, member, want) in [
        ("a walk", walk, "claim.key", json!(["c1", "c3"])),
        ("a degree", degree, "degree", json!([2])),
    ] {
        let rows = c.query(alice, pattern).await.map(|r| r.data);
        let rows = serde_json::to_value(rows.expect("a query answers")).expect("rows read");
        let mut got: Vec<Value> = (rows["rows"].as_array().into_iter().flatten())
            .map(|r| r[member].clone())
            .collect();
        got.sort_by_key(Value::to_string);
        if json!(got) != want {
            wrong.push(format!("{what} from `t1`: {got:?}, not {want}"));
        }
    }
    let head = store.head().await.expect("the head");
    let c1 = key("c1");
    let p = || patch(&[("amount", json!(1))]);
    for (what, answer) in [
        ("`upsert`", c.upsert(alice, &claim, &c1, p(), opts()).await),
        ("`retire`", c.retire(alice, &claim, &c1, opts()).await),
        (
            "`link`",
            c.link(alice, &claim, &c1, &on, &t1, none(), opts()).await,
        ),
        (
            "`unlink`",
            c.unlink(alice, &claim, &c1, &on, &t1, opts()).await,
        ),
    ] {
        if !matches!(
            answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        ) {
            wrong.push(format!("{what} of her own `c1`: {answer:?}"));
        }
    }
    let after = store.head().await.expect("the head");
    if after != head {
        wrong.push(format!(
            "the refused writes moved the head from {head} to {after}"
        ));
    }
    let listed = c.proposals(alice, Default::default()).await;
    let listed = listed.expect("alice's listing").data;
    if !listed.iter().all(|p| p.write.is_none() && p.more) || listed.is_empty() {
        wrong.push(format!(
            "bob's proposal is listed to alice with its write: {listed:?}"
        ));
    }
    let told = c.describe_type(alice, &claim, ReadAt::default()).await;
    let may = told.expect("`claim` is described to alice").data.may;
    let only = serde_json::to_value(&may.only).expect("pairs read");
    if only != json!({"holder": "alice"}) || may.upsert || may.retire || !may.link.is_empty() {
        wrong.push(format!("`describe_type` says {may:?}"));
    }
    assert!(
        wrong.is_empty(),
        "alice is admitted her own claims alone: {}",
        wrong.join("; ")
    );
}

/// A vector field of two dimensions.
fn vector(name: &str) -> FieldDecl {
    FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..text(name)
    }
}
