//! The door-facing behaviours of the contract, as checks any lake can be
//! put through.
//!
//! Each check is one async function over a fresh, empty
//! `Arc<dyn LakeStore>`: it declares the fixture ontology through the
//! catalog, acts as the fixture's principals and asserts what a door can
//! observe. This crate runs them on the store over memory and SQLite
//! (`tests/checks_on_the_store.rs`), and a door crate runs them again over
//! whatever lake it is built on.
//!
//! [`door_checks!`](crate::door_checks) expands to one `#[test]` per
//! check, given a function that makes a fresh lake and one that blocks
//! on a future. No check assumes an absolute position: a lake logs its
//! own events before its first write.
//!
//! A refusal's sentence is its case's, from the one catalog
//! (`lakelands_api::error::Case`): a check asserts a refusal's kind, its
//! field and the case it says, with its places and parameters, never its
//! words.

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::describe::{ExecutorDescription, Kept, NotKept};
use lakelands_api::error::{Case, Error, Missing, Naming, Place, Refusal, Said};
use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::expr::kind::Kind;
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, ProposalId, RecordKey, SourceId, TypeName, VerbName,
    Version,
};
use lakelands_api::location::Limits;
use lakelands_api::ontology::{
    Assignment, Authority, ComputedAt, FieldDecl, FieldType, Measure, OnInputChange, RelationDecl,
    Risk, StorageKind, TriggerDecl, TypeDecl, VerbDecl, VerbEffect, VerbScope, ViewDecl, ViewParam,
};
use lakelands_api::principal::{Access, Grants, Principal, PrincipalKind, TypeGrants};
use lakelands_api::query::{Pattern, Search, SearchMode, Step, VectorTerm, Walk};
use lakelands_api::read::{After, Direction, Find, Found, Get, Order, OrderBy, ReadAt};
use lakelands_api::record::{Origin, Patch, Record, SourceRecord, Value};
use lakelands_api::seam::{FEED_EXAMINED, FeedFilter};
use lakelands_api::tools::{Door, tools};
use lakelands_api::write::{
    Carrier, Declaration, FunctionDecl, Migration, MigrationReport, ObjectSetDecl, Outcome,
    Proposal, ProposalFilter, ProposalState, ProposedWrite, Recompute, ValueTypeMigration,
    ValueTypeReport, WriteOpts,
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
/// lakelands_door_checks::door_checks!(my_fresh_lake, my_runtime::block_on);
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
#[macro_export]
macro_rules! door_checks {
    ($new:expr, $block_on:path) => {
        $crate::door_checks!(@all local, $new, $block_on);
    };
    ($new:expr, $block_on:path, remote) => {
        $crate::door_checks!(@all remote, $new, $block_on);
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
            when_a_field_is_computed_is_declared_as_the_store_declares_it,
            #[own_principals]
            a_grants_only_on_a_field_computed_at_the_read_is_not_applied,
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
            a_call_is_refused_in_the_order_013_gives,
            a_call_that_could_not_run_now_is_neither_run_nor_proposed,
            a_rejected_proposal_writes_nothing_and_stays_decided,
            only_a_reviewer_decides_proposals,
            #[own_principals]
            a_reviewer_not_shown_a_type_finds_no_proposal_on_it,
            #[own_principals]
            an_accepts_refusal_names_only_what_its_reviewer_is_listed,
            sys_types_cannot_be_declared,
            add_field_without_the_schema_grant_tells_nothing_of_the_type,
            a_type_its_proposer_is_not_shown_is_a_type_that_is_not_declared,
            a_type_its_writer_is_not_shown_is_a_type_that_is_not_declared,
            #[own_principals]
            a_sensitive_field_shows_a_field_and_never_a_type,
            #[own_principals]
            a_type_its_principal_is_shown_is_one_it_may_find_and_get,
            #[own_principals]
            a_type_keyed_by_a_field_its_principal_may_not_know_of_is_not_shown,
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
            every_path_a_pattern_names_is_one_its_rows_hold,
            a_row_binds_each_record_under_a_name_of_its_own,
            #[own_principals]
            a_verb_names_no_system_type_but_a_link_type,
            ingest_judges_a_record_around_its_duplicate_test,
            #[own_principals]
            a_value_types_report_names_its_conditions_as_the_store_does,
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
            #[own_principals]
            a_view_names_no_index_to_a_holder_of_only,
            a_vector_index_setting_names_vector_fields_and_search_says_it_ranked_by_the_index,
            a_view_kept_setting_names_a_view_that_can_be_kept_and_describe_says_whether_it_is,
            a_setting_is_judged_by_what_its_writer_is_shown,
            an_ingested_upstream_is_answered_per_field,
            a_schema_proposal_is_refused_by_kind_at_propose_and_at_accept,
            a_value_types_tightening_is_proposed_and_accepted_while_it_fits,
            a_value_types_renames_are_refused_and_applied_as_the_store_does,
            a_proposal_shows_its_reader_the_values_it_may_read,
            a_proposal_names_what_its_reader_may_know_and_says_there_is_more,
            an_order_by_latest_takes_no_field_and_no_view,
            a_view_reads_the_parameters_each_read_binds,
            a_parameter_its_type_does_not_declare_is_refused,
            a_trigger_watches_a_link_type_and_not_a_reference_relations,
            degree_and_components_answer_the_graph_in_one_order,
            a_search_over_a_field_the_caller_may_not_read_is_refused_in_every_mode,
            #[own_principals]
            a_vector_ranking_reads_a_vector_field,
            a_search_limited_to_named_fields_matches_and_probes_only_those,
            a_hybrid_search_fuses_its_text_and_vector_ranks,
            #[own_principals]
            a_grant_admits_only_the_records_whose_fields_hold_its_values,
            #[own_principals]
            a_grant_that_cannot_be_applied_admits_no_record,
            #[own_principals]
            a_view_is_keyed_by_its_groups_where_their_values_come_from,
            #[own_principals]
            a_hidden_target_is_not_learned_by_declaring_its_field,
            #[own_principals]
            a_relation_carried_by_a_hidden_field_answers_nothing_of_it,
            #[own_principals]
            a_shape_names_no_field_its_author_may_not_know_of,
            #[own_principals]
            a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one,
            a_valid_time_names_a_date_time_field,
            #[own_principals]
            a_reference_to_a_type_its_reader_is_not_shown_is_a_field_it_may_not_know_of,
            a_date_is_kept_as_its_day,
            #[own_principals]
            describe_names_no_verb_or_function_its_reader_is_not_shown,
            #[own_principals]
            a_verb_that_reads_a_field_its_caller_may_not_read_is_not_callable);
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
        computed_at: None,
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
    matches!(refusal(e), (Some(f), Refusal::Invalid(said))
        if f.as_str() == name
            && matches!(&said.case, Case::UnknownField { field, .. } if field.as_str() == name))
}

/// The refusal of what was asked as not well-formed, at `field` where one
/// is named, saying `said`.
fn invalid(field: Option<&str>, said: impl Into<Said>) -> Error {
    let reason = Refusal::Invalid(Box::new(said.into()));
    Error::Refused {
        field: field.map(FieldName::from),
        reason,
    }
}

/// The refusal of what a constraint does not allow, at `field`.
fn constraint(field: &str, said: impl Into<Said>) -> Error {
    let reason = Refusal::Constraint(Box::new(said.into()));
    Error::Refused {
        field: Some(field.into()),
        reason,
    }
}

/// `case`, said at `places`, outermost first.
fn said_at(places: &[Place], case: Case) -> Said {
    let at = places.to_vec();
    Said { at, case }
}

/// The place of a type.
fn of_type(name: &str) -> Place {
    Place::Type { name: ty(name) }
}

/// The place of a field.
fn of_field(name: &str) -> Place {
    Place::Field { name: name.into() }
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
        computed_at: f.computed_at,
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
        let on = if verb == "register" { PARTNER } else { INVOICE };
        let (verb, ty) = (VerbName::from(verb), ty(on));
        // Over REST the version is a header, which the door refuses itself.
        let header = Case::VerbTypeHeader {
            header: "If-Match".into(),
            verb: verb.clone(),
        };
        let want = match (k, verb.as_str()) {
            (Some(_), _) => Case::VerbTypeTakesNoKey { verb, ty },
            (None, "register") if e == invalid(None, header.clone()) => header,
            (None, "register") => Case::VerbTypeNoVersion { verb, ty },
            _ => Case::VerbNeedsKey { verb, ty },
        };
        assert_eq!(e, invalid(None, want), "{what}");
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
        let case = Case::IsAView {
            ty: ty("partner_totals"),
        };
        assert_eq!(e, invalid(None, case), "{what}");
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
    let at = Place::Verb {
        name: "on_record".into(),
    };
    let case = Case::VerbOnView {
        ty: ty("partner_totals"),
    };
    assert_eq!(
        e,
        invalid(None, said_at(&[at], case)),
        "a verb on a record of a view"
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
/// the declaration and the view.
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
    let totals = || ty("partner_totals");
    let on = |name: &str| Place::Trigger { name: name.into() };
    let of = |name: &str| Place::ObjectSet { name: name.into() };
    for (d, at, field, case) in [
        (
            trigger,
            on("grew"),
            "on",
            Case::WatchesView { ty: totals() },
        ),
        (set, of("busy"), "of", Case::SetOfView { ty: totals() }),
    ] {
        let e = f
            .catalog
            .declare(owner, d)
            .await
            .expect_err("nothing that never acts is declared on a view");
        assert_eq!(e, invalid(Some(field), said_at(&[at], case)));
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// An object set may hold the records of a relation's link type, as a
/// trigger may watch them: `declare` takes a set over
/// `sys.link.<type>.<relation>` and `describe` shows it.
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
/// described, offered to `link`, linked or walked (lakelands#662).
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
            named: None,
        };
        let (c, who) = (c.clone(), who.clone());
        async move {
            let p = Pattern::traverse(ty(PARTNER), key("P-1"), vec![step]);
            c.query(&who, p).await.map_err(|e| format!("{e:?}"))
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
    let unknown = link(&reads, "nothing").await;
    assert!(
        matches!(
            &unknown,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "a relation `memo` has not, to one shown `memo`: {unknown:?}"
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
    let as_nothing = |e: Error, by: &str| format!("{e:?}").replace(by, "nothing");
    let nothing = link(&far, "nothing")
        .await
        .expect_err("no relation `nothing`");
    let seen = link(&far, "seen_by")
        .await
        .expect_err("`seen_by` is not shown");
    assert_eq!(as_nothing(seen, "seen_by"), format!("{nothing:?}"));
    let from_memo = |by: &str| {
        let step = Step {
            relation: by.into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Default::default(),
            named: None,
        };
        Pattern::traverse(memo.clone(), key("a"), vec![step])
    };
    let seen = c.query(&far, from_memo("seen_by")).await.map(drop);
    let ghost = c.query(&far, from_memo("ghost")).await.map(drop);
    assert_eq!(
        seen.map_err(|e| format!("{e:?}").replace("seen_by", "ghost")),
        ghost.map_err(|e| format!("{e:?}")),
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
    let said = c.migrate(&blind, m).await.map(drop);
    let case = Case::MigrationTakesRead {
        unnamed: true,
        field: "note".into(),
        ty: ty("pay"),
        renamed: None,
    };
    let at = [Place::AfterMigration { name: ty("pay") }];
    assert_eq!(
        said,
        Err(invalid(None, said_at(&at, case))),
        "dropping `pay.note`, `blind` is told no type"
    );
}

/// A read of a view probes what computing it reads: each source's fields,
/// a merged source's as its own, and those of a source that is itself a
/// view. A reader who may not know of a field one of them reads is refused
/// as not granted, on no field and without that field's name, by `find`
/// and by `get`, and `describe` withholds the definition where the view
/// itself reads that field of its own or a merged source; a holder of the
/// `schema` grant is refused on that field; the owner is answered (024,
/// extended).
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
/// refuses it, naming the view and the relation.
pub async fn a_relation_on_a_view_is_derived(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let head = f.head().await;
    for (relation, name, carrier) in [
        (RelationDecl::to("seen", INVOICE), "seen", Carrier::Links),
        (
            RelationDecl {
                via: Some("partner".into()),
                ..RelationDecl::to("by", INVOICE)
            },
            "by",
            Carrier::Via,
        ),
    ] {
        let mut view = partner_totals_view();
        view.relations = vec![relation];
        let e = f
            .catalog
            .declare(&f.cast.owner, Declaration::Type(view))
            .await
            .expect_err("a relation on a view that is not derived");
        let on = [
            of_type("partner_totals"),
            Place::Relation { name: name.into() },
        ];
        let ty = ty("partner_totals");
        let want = said_at(&on, Case::ViewRelation { ty, carrier });
        assert_eq!(e, invalid(None, want));
    }
    assert_eq!(f.head().await, head, "nothing is appended");
}

/// A migration reshapes stored records, and a view stores none: turning
/// a stored type into a view, or a view into a stored type, is refused at
/// the preflight, naming the type.
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
        let want = match name {
            "tally" => Case::MigrationToView { ty: ty(name) },
            _ => Case::MigrationViewToStored { ty: ty(name) },
        };
        assert_eq!(e, invalid(None, want));
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
/// declaration reads the field, without the view.
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
        // The view is checked against the shape the migration leaves.
        let on = [
            Place::AfterMigration { name: ty(INVOICE) },
            of_type("status_totals"),
        ];
        let (ty, field) = (ty(INVOICE), FieldName::from(read));
        assert_eq!(
            e,
            invalid(None, said_at(&on, Case::UnknownField { ty, field }))
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
    let on = [Place::AfterMigration { name: ty(INVOICE) }];
    let case = Case::MigrationTakesRead {
        unnamed: true,
        field: "amount".into(),
        ty: ty(INVOICE),
        renamed: None,
    };
    assert_eq!(e, invalid(None, said_at(&on, case)));
}

/// A view whose `where` names, or whose measure aggregates, a field its
/// source does not have is refused, naming the view and the field.
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
        let (ty, field) = (ty(INVOICE), FieldName::from("nope"));
        let at = [of_type("status_totals")];
        let want = said_at(&at, Case::UnknownField { ty, field });
        assert_eq!(e, invalid(None, want));
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
/// nothing is appended (018, 028).
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
    let (t, merge) = (of_type, |n: &str| Place::MergedSource { name: ty(n) });
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
            said_at(
                &[t("v1"), merge(INVOICE)],
                Case::SourceTwice {
                    source: ty(INVOICE),
                },
            ),
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
            said_at(
                &[t("v2"), merge("credit")],
                Case::SourceTwice {
                    source: ty("credit"),
                },
            ),
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
            said_at(
                &[t("v3"), merge("ghost")],
                Case::Undeclared { ty: ty("ghost") },
            ),
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
            said_at(
                &[t("v4")],
                Case::ReadsParamView {
                    source: ty("pview"),
                    param: "p".into(),
                },
            ),
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
            said_at(
                &[t("v5"), merge("credit")],
                Case::MergedGroupCount { given: 2, view: 1 },
            ),
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
            said_at(
                &[t("v6"), merge("credit")],
                Case::UnknownField {
                    ty: ty("credit"),
                    field: "nope".into(),
                },
            ),
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
            said_at(
                &[t("v7"), merge("credit")],
                Case::MergedGroupKind {
                    field: "x".into(),
                    view_field: "status".into(),
                },
            ),
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
            said_at(
                &[
                    t("v8"),
                    merge("credit"),
                    Place::Measure { name: "m".into() },
                ],
                Case::MeasuresNothing {
                    source: ty("credit"),
                },
            ),
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
            said_at(&[t("v9")], Case::MeasureTwice { name: "n".into() }),
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
            said_at(
                &[t("v10"), merge("credit"), Place::Where],
                Case::NotParsed {
                    text: "status = ".into(),
                    why: Box::new(Case::ParseEndsEarly { offset: 9 }),
                },
            ),
        ),
        (
            view("v11", "ghost", "status", &[], json!({})),
            None,
            said_at(&[t("v11")], Case::Undeclared { ty: ty("ghost") }),
        ),
        (
            view("v12", "pview", "status", &[], json!({})),
            None,
            said_at(
                &[t("v12")],
                Case::ReadsParamView {
                    source: ty("pview"),
                    param: "p".into(),
                },
            ),
        ),
        (
            view("v13", INVOICE, "nope", &[], json!({})),
            None,
            said_at(
                &[t("v13")],
                Case::UnknownField {
                    ty: ty(INVOICE),
                    field: "nope".into(),
                },
            ),
        ),
        (
            view("v14", "v14", "status", &[], json!({})),
            None,
            said_at(
                &[t("v14")],
                Case::ViewCycle {
                    chain: ["v14", "v14"].into_iter().collect(),
                },
            ),
        ),
        (
            view("v15", INVOICE, "status", &["extra"], json!({})),
            Some("extra"),
            said_at(
                &[t("v15")],
                Case::ViewFieldUnfilled {
                    field: "extra".into(),
                },
            ),
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
            said_at(
                &[t("v16")],
                Case::ViewValueNoField {
                    field: "total".into(),
                },
            ),
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
            said_at(&[t("v17")], Case::ParamUnread { name: "p".into() }),
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
            said_at(
                &[t("v18")],
                Case::DeclaredTwice {
                    what: Naming::Parameter,
                    name: "p".into(),
                },
            ),
        ),
    ];
    let head = f.head().await;
    for (d, field, want) in rows {
        // `declare` says each at the declaration too.
        let d = decl(d);
        let want = want.within(d.place());
        let e = c.declare(owner, d).await.expect_err("refused");
        assert_eq!(e, invalid(field, want));
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
/// appended (028).
pub async fn names_and_expressions_are_declared_only_as_the_store_declares_them(
    store: Arc<dyn LakeStore>,
) {
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
    let tn = |n: &str| TypeName::from(n);
    let field = |n: &str, e: &str| {
        let on = vec![Place::Type { name: tn(n) }, of_field("d")];
        (ty(n, derived(e)), on)
    };
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
            vec![
                Place::Type { name: tn(n) },
                Place::Measure { name: "n".into() },
            ],
        )
    };
    let filtered = |n: &str, w: &str| {
        (
            view(n, "n", json!({"where": w})),
            vec![Place::Type { name: tn(n) }, Place::Where],
        )
    };
    let vtype = |n: &str, e: &str| {
        (
            json!({"declare": "value_type", "name": n, "kind": "text", "constraint": e}),
            vec![Place::ValueType { name: n.into() }, Place::Constraint],
        )
    };
    let verb = |n: &str, requires: Option<&str>, value: &str| {
        let mut v = json!({"declare": "verb", "name": n, "on": INVOICE, "risk": "low", "effect": "write", "set": [{"field": "status", "value": value}]});
        if let Some(r) = requires {
            v["requires"] = json!(r);
        }
        v
    };
    let on_verb = |n: &str, p: Place| vec![Place::Verb { name: n.into() }, p];
    let requires = |n: &str, e: &str| (verb(n, Some(e), "'x'"), on_verb(n, Place::Requires));
    let value = |n: &str, e: &str| (verb(n, None, e), on_verb(n, of_field("status")));
    let trigger = |n: &str, e: &str| {
        (
            json!({"declare": "trigger", "name": n, "on": INVOICE, "when": e}),
            vec![Place::Trigger { name: n.into() }],
        )
    };
    let set = |n: &str, e: &str| {
        (
            json!({"declare": "object_set", "name": n, "of": INVOICE, "where": e}),
            vec![Place::ObjectSet { name: n.into() }],
        )
    };
    let function = |n: &str, backing: &str, e: &str| json!({"declare": "function", "name": n, "backing": backing, "inputs": [text("t")], "expr": e});
    let body = |n: &str, e: &str| {
        let on = vec![Place::Function { name: n.into() }];
        (function(n, "expr", e), on)
    };
    let with_relation = |r: &str| {
        let mut t = ty("nr", text("x"));
        t["relations"] = json!([{"name": r, "target": PARTNER}]);
        t
    };
    let unnamed = |what: Naming, name: &str| Case::NameNotIdentifier {
        what,
        name: name.into(),
    };
    let names = [
        (
            ty("bad-type", text("x")),
            Case::TypeNameForm {
                name: "bad-type".into(),
            },
        ),
        (
            ty("a..b", text("x")),
            Case::TypeNameForm {
                name: "a..b".into(),
            },
        ),
        (
            ty("nf", text("bad-field")),
            unnamed(Naming::Field, "bad-field"),
        ),
        (
            with_relation("bad-rel"),
            unnamed(Naming::Relation, "bad-rel"),
        ),
        (
            verb("bad-verb", None, "'x'"),
            unnamed(Naming::Verb, "bad-verb"),
        ),
        (
            json!({"declare": "verb", "name": "ext", "on": INVOICE, "risk": "low", "effect": "external", "executor": "bad exec"}),
            unnamed(Naming::Executor, "bad exec"),
        ),
        (
            trigger("bad-trigger", "true").0,
            unnamed(Naming::Trigger, "bad-trigger"),
        ),
        (
            set("bad-set", "true").0,
            unnamed(Naming::ObjectSet, "bad-set"),
        ),
        (
            vtype("bad-vt", "true").0,
            unnamed(Naming::ValueType, "bad-vt"),
        ),
        (
            function("bad-fn", "expr", "upper(t)"),
            unnamed(Naming::Function, "bad-fn"),
        ),
        (
            function("fine", "bad-backing", "upper(t)"),
            unnamed(Naming::Backing, "bad-backing"),
        ),
        (
            view("nm", "bad-m", json!({})),
            unnamed(Naming::Measure, "bad-m"),
        ),
        (
            view(
                "np",
                "n",
                json!({"where": "status = param('bad-p')", "params": [{"name": "bad-p", "kind": "text"}]}),
            ),
            unnamed(Naming::Parameter, "bad-p"),
        ),
    ];
    let agg = || Case::AggregateHere {
        name: "count".into(),
    };
    let changed = || Case::OnlyInWrite {
        name: "changed".into(),
    };
    let parsed = |text: &str, why: Case| Case::NotParsed {
        text: text.into(),
        why: Box::new(why),
    };
    let early = |text: &str, offset: usize| parsed(text, Case::ParseEndsEarly { offset });
    let inexact = |number: &str| Case::NotExact {
        number: number.into(),
    };
    let faults = [
        (field("e1", "1 +"), early("1 +", 3)),
        (vtype("e2", "value ="), early("value =", 7)),
        (
            requires("e3", "(status"),
            parsed(
                "(status",
                Case::ParseExpected {
                    token: ")".into(),
                    offset: 7,
                },
            ),
        ),
        (value("e4", "upper("), early("upper(", 6)),
        (
            trigger("e5", "status =="),
            parsed(
                "status ==",
                Case::ParseUnexpected {
                    found: '=',
                    offset: 8,
                },
            ),
        ),
        (set("e6", "status = "), early("status = ", 9)),
        (body("e7", "upper("), early("upper(", 6)),
        (
            set("e8", "status in ()"),
            parsed("status in ()", Case::ParseEmptyIn { offset: 12 }),
        ),
        (
            set("e9", "amount > 1.5e3"),
            parsed("amount > 1.5e3", Case::ParseNumberIntoName { offset: 12 }),
        ),
        (field("p1", "count()"), agg()),
        (field("p2", "retired()"), Case::RetiredOnlyInGuard),
        (field("p3", "param('p')"), Case::ParamOutsideView),
        (
            field("p4", "upper()"),
            parsed(
                "upper()",
                Case::ParseArityExact {
                    name: "upper".into(),
                    takes: 1,
                    given: 0,
                    offset: 0,
                },
            ),
        ),
        (
            field("p5", "setting(k)"),
            parsed("setting(k)", Case::ParseSettingKey { offset: 0 }),
        ),
        (
            field("p6", "date_bucket(k, 'decade')"),
            Case::BucketUnit {
                unit: "decade".into(),
            },
        ),
        (
            field("p7", "changed('k')"),
            parsed(
                "changed('k')",
                Case::ParseTakesField {
                    name: "changed".into(),
                    offset: 0,
                },
            ),
        ),
        (filtered("p8", "count() > 1"), agg()),
        (measure("p9", "sum(retired())"), Case::RetiredOnlyInGuard),
        (measure("p10", "count() + len(changed(status))"), changed()),
        (vtype("p11", "count() > 0"), agg()),
        (vtype("p12", "changed(value)"), changed()),
        (requires("p13", "changed(status)"), changed()),
        (requires("p14", "count() > 0"), agg()),
        (value("p15", "count()"), agg()),
        (value("p16", "param('p')"), Case::ParamOutsideView),
        (trigger("p17", "count() > 1"), agg()),
        (trigger("p18", "param('p') = 'x'"), Case::ParamOutsideView),
        (set("p19", "changed(status)"), changed()),
        (set("p20", "count() > 1"), agg()),
        (set("p21", "retired()"), Case::RetiredOnlyInGuard),
        (body("p22", "count()"), agg()),
        (body("p23", "changed(t)"), changed()),
        (field("x1", &format!("k = {BIG}")), inexact(BIG)),
        (set("x2", &format!("amount > {BIG}")), inexact(BIG)),
        (set("x3", &format!("amount > {PAST}")), inexact(PAST)),
        (set("x4", &format!("amount > -{PAST}")), inexact(PAST)),
        (field("i1", "in_set('open')"), Case::InSetHere),
        (filtered("i2", "in_set('open')"), Case::InSetHere),
        (vtype("i3", "in_set('open')"), Case::InSetHere),
        (requires("i4", "in_set('open')"), Case::InSetHere),
        (value("i5", "in_set('open')"), Case::InSetHere),
        (body("i6", "in_set('open')"), Case::InSetHere),
        (
            set("i7", "in_set(open)"),
            Case::CallTakesQuoted {
                name: "in_set".into(),
            },
        ),
    ];
    let refused = |e: Result<Outcome, Error>, want: Said, at: Option<&str>| {
        assert_eq!(e.expect_err("refused"), invalid(at, want));
    };
    let head = f.head().await;
    // `declare` says each at the declaration.
    for (d, want) in names {
        let d = decl(d);
        let at = [d.place()];
        refused(c.declare(owner, d).await, said_at(&at, want), None);
    }
    for ((d, at), why) in faults {
        let d = decl(d);
        let want = said_at(&at, why).within(d.place());
        refused(c.declare(owner, d).await, want, None);
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
    let on_invoice = |n: &str, why: Case| {
        let at = [of_type(INVOICE), of_field(n)];
        said_at(&at, why)
    };
    refused(
        added("bad-f", None).await,
        said_at(&[of_type(INVOICE)], unnamed(Naming::Field, "bad-f")),
        None,
    );
    refused(
        added("a1", Some("count()")).await,
        on_invoice("a1", agg()),
        None,
    );
    refused(
        added("a2", Some("1 +")).await,
        on_invoice("a2", early("1 +", 3)),
        None,
    );
    refused(
        added("a3", Some("in_set('open')")).await,
        on_invoice("a3", Case::InSetHere),
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
        unnamed(Naming::AgingRule, "bad-rule").into(),
        Some("name"),
    );
    assert_eq!(f.head().await, head, "nothing is appended");
    c.declare(owner, decl(set("fits", &format!("amount > {MAX}")).0))
        .await
        .expect("the largest numeral held exactly is taken");
}

/// A migration that names a relation's rename keeps its edges: the report
/// lists the relation with the links that move, and the links are records
/// of the new name's link type and of the old no more; the feed hands
/// the `migrated` to one who follows either link type, and that of a
/// relation carried by `via` to no follower of a link type; a rename of a
/// relation whose link type another type refers to is refused, naming
/// that type.
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
    let at = Place::RelationRename {
        from: "quotes".into(),
        to: "cites".into(),
    };
    let case = Case::RenameReferredLinks {
        ty: Some("gloss".into()),
        links: "sys.link.memo.quotes".into(),
    };
    assert_eq!(e, invalid(None, said_at(&[at], case)));
}

/// A proposed migration that renames a field and a relation waits while
/// the owner renames both otherwise, and `aside`, which the proposal retypes.
/// Listed to a reviewer kept from a field, or from the relation's link
/// field, by a grant on the name in force, its shape holds none of them
/// (lakelands#594, lakelands#644).
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
    let case = Case::NothingComputedOutside { ty: ty(INVOICE) };
    assert_eq!(e, invalid(None, case));
    assert_eq!(f.head().await, head, "a refused call appended");
}

/// When a field is computed is declared as the store takes it: a derived
/// expression that reads the clock says it; a field computed at the read
/// is derived, required by no write, no key, no link's field, no derived
/// relation's match, read by no field computed at the write and by no
/// trigger, verb or object set, and not taken to the read where one reads
/// it; a verb's payload says no `computed_at`. Each is refused as invalid
/// in the store's words and nothing is appended; an order by it is
/// refused; `describe` shows the member (052).
pub async fn when_a_field_is_computed_is_declared_as_the_store_declares_it(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let decl = |v: Value| serde_json::from_value::<Declaration>(v).expect("a declaration");
    let late = |more: Value| {
        let mut l = json!({"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
                           "derived": "due < now()", "computed_at": "read"});
        l.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        l
    };
    let ty = |fields: Vec<Value>, more: Value| {
        let mut t = json!({"declare": "type", "name": "ca", "key": ["k"], "fields": [
            {"name": "k", "type": {"kind": "text"}}, {"name": "due", "type": {"kind": "date_time"}}]});
        t["fields"].as_array_mut().unwrap().extend(fields);
        t.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        t
    };
    let ca = || of_type("ca");
    let on_ca = |field: &str, case: Case| said_at(&[ca(), of_field(field)], case);
    let next = Place::Relation {
        name: "next".into(),
    };
    let same = Place::Relation {
        name: "same".into(),
    };
    let late_ = || FieldName::from("late");
    let rows = [
        (
            ty(vec![late(json!({"computed_at": null}))], json!({})),
            Some("late"),
            on_ca("late", Case::ClockUnsaid),
        ),
        (
            ty(
                vec![json!({"name": "x", "type": {"kind": "text"}, "computed_at": "write"})],
                json!({}),
            ),
            Some("x"),
            on_ca("x", Case::ComputedAtUncomputed),
        ),
        (
            ty(vec![late(json!({"required": true}))], json!({})),
            Some("late"),
            on_ca("late", Case::AtReadRequired),
        ),
        (
            ty(vec![late(json!({}))], json!({"key": ["late"]})),
            Some("late"),
            on_ca("late", Case::AtReadKey),
        ),
        (
            ty(
                vec![],
                json!({"relations": [{"name": "next", "target": "ca", "fields": [late(json!({}))]}]}),
            ),
            Some("late"),
            said_at(&[ca(), next, of_field("late")], Case::LinkFieldAtRead),
        ),
        (
            ty(
                vec![
                    late(json!({})),
                    json!({"name": "flag", "type": {"kind": "boolean"}, "authority": "derived", "derived": "not late"}),
                ],
                json!({}),
            ),
            Some("flag"),
            on_ca("flag", Case::AtWriteReadsAtRead { field: late_() }),
        ),
        (
            ty(
                vec![late(json!({"type": {"kind": "text"}, "derived": "k"}))],
                json!({"relations": [{"name": "same", "target": "ca", "derived": {"match": [{"from": "k", "to": "late"}]}}]}),
            ),
            None,
            said_at(
                &[ca(), same],
                Case::PairAtRead {
                    from: "k".into(),
                    to: late_(),
                    target: TypeName::from("ca"),
                },
            ),
        ),
    ];
    let head = f.head().await;
    for (d, field, want) in rows {
        let e = c.declare(owner, decl(d)).await.expect_err("refused");
        assert_eq!(e, invalid(field, want.within(ca())));
    }
    assert_eq!(f.head().await, head, "nothing is appended");
    let soon = |when: &str| json!({"name": "soon", "type": {"kind": "date_time"}, "authority": "derived", "derived": "due", "computed_at": when});
    let band = late(json!({"name": "band", "type": {"kind": "text"}, "derived": "k"}));
    c.declare(
        owner,
        decl(ty(
            vec![late(json!({})), soon("write"), band.clone()],
            json!({}),
        )),
    )
    .await
    .expect("a field computed at the read is declared");
    let watch = json!({"declare": "trigger", "name": "watch", "on": "ca", "when": "soon > due"});
    c.declare(owner, decl(watch))
        .await
        .expect("a trigger reads `soon`");
    let head = f.head().await;
    for d in [
        json!({"declare": "trigger", "name": "tr", "on": "ca", "when": "late"}),
        json!({"declare": "object_set", "name": "st", "of": "ca", "where": "late"}),
        json!({"declare": "verb", "name": "vb", "on": "ca", "risk": "low", "effect": "write",
            "set": [{"field": "k", "value": "if(late, 'a', 'b')"}]}),
    ] {
        let d = decl(d);
        let e = c.declare(owner, d.clone()).await.expect_err("refused");
        let (field, ty) = (late_(), TypeName::from("ca"));
        let want = said_at(&[d.place()], Case::ReadsAtRead { field, ty });
        assert_eq!(e, invalid(None, want));
    }
    let p = json!([{"name": "p", "type": {"kind": "text"}, "computed_at": "write"}]);
    let pv = Place::Verb { name: "pv".into() };
    let more = [
        (
            ty(vec![late(json!({})), soon("read"), band], json!({})),
            Some("soon"),
            on_ca("soon", Case::ReadAsStored),
        ),
        (
            json!({"declare": "verb", "name": "pv", "on": "ca", "risk": "low", "effect": "write", "payload": p,
                "set": [{"field": "k", "value": "payload.p"}]}),
            Some("p"),
            said_at(
                &[pv, Place::Payload, of_field("p")],
                Case::ComputedAtNoRecord,
            ),
        ),
    ];
    for (d, field, want) in more {
        let d = decl(d);
        let want = want.within(d.place());
        let e = c.declare(owner, d).await.expect_err("refused");
        assert_eq!(e, invalid(field, want));
    }
    assert_eq!(f.head().await, head, "nothing is appended");
    // An order by it is refused, by `find` and by a pattern.
    let order = |field: &str| {
        vec![Order {
            field: field.into(),
            by: OrderBy::Value,
            dir: Direction::Asc,
        }]
    };
    let want = invalid(
        None,
        Case::OrderAtRead {
            field: late_(),
            ty: TypeName::from("ca"),
        },
    );
    let find = Find {
        order_by: order("late"),
        ..Find::default()
    };
    let e = c
        .find(owner, &"ca".into(), find)
        .await
        .expect_err("refused");
    assert_eq!(e, want, "`find`");
    let mut walk: Pattern = serde_json::from_value(
        json!({"start": {"type": "ca"}, "steps": [], "project": {"as": "rows"}}),
    )
    .expect("a pattern");
    walk.order_by = order("ca.late");
    let e = c.query(owner, walk).await.expect_err("refused");
    assert_eq!(e, want, "a pattern");
    let shape = c
        .describe_type(owner, &"ca".into(), Default::default())
        .await;
    let shape = shape.expect("the owner describes `ca`").data;
    let late = shape.fields.iter().find(|f| f.name.as_str() == "late");
    assert_eq!(
        late.and_then(|f| f.computed_at),
        Some(ComputedAt::Read),
        "{late:?}"
    );
}

/// A grant's `only` on a field computed at the read cannot be applied: no
/// engine holds the field to filter by. A read is refused naming the type
/// alone (052).
pub async fn a_grants_only_on_a_field_computed_at_the_read_is_not_applied(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (c, owner) = (&f.catalog, &f.cast.owner);
    let ca = json!({"declare": "type", "name": "ca", "key": ["k"], "fields": [
        {"name": "k", "type": {"kind": "text"}},
        {"name": "band", "type": {"kind": "text"}, "authority": "derived", "derived": "k",
         "computed_at": "read"}]});
    let ca = serde_json::from_value::<Declaration>(ca).expect("a declaration");
    c.declare(owner, ca).await.expect("`ca` is declared");
    let grants = json!({"default": "read", "types": {"ca": {"only": {"band": "x"}}}});
    let eve = Principal {
        id: "eve".into(),
        grants: serde_json::from_value(grants).expect("grants"),
        ..owner.clone()
    };
    let e = c
        .find(&eve, &"ca".into(), Find::default())
        .await
        .expect_err("refused");
    let want = Case::OnlyUnappliable { ty: ty("ca") };
    assert_eq!(e, invalid(None, want));
}

/// A derived field, an object set's `where`, a trigger's `when` and a
/// verb's `requires` or the value it sets a field from read only fields
/// the type has: one that reads another is refused by `declare` and by
/// `apply`, naming the declaration and the field, and nothing is
/// appended. `payload.<name>` and `setting`
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
    let no_field = |on: &str, field: &str| Case::UnknownField {
        ty: ty(on),
        field: field.into(),
    };
    for (on, d) in [
        (
            "memo",
            json!({"declare": "type", "name": "memo", "fields": [{"name": "loud",
                   "type": {"kind": "text"}, "authority": "derived", "derived": "upper(ghost)"}]}),
        ),
        (INVOICE, verb("close", "ghost = 'x'", "'shut'")),
        (INVOICE, verb("copy", "status = 'x'", "ghost")),
        (
            INVOICE,
            json!({"declare": "trigger", "name": "t", "on": INVOICE, "when": "changed(ghost)"}),
        ),
        (
            INVOICE,
            json!({"declare": "object_set", "name": "s", "of": INVOICE, "where": "ghost = 'x'"}),
        ),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        // Said at the declaration.
        let want = said_at(&[d.place()], no_field(on, "ghost"));
        let e = f.catalog.declare(owner, d.clone()).await;
        let e = e.expect_err("a declaration reads only what its type has");
        assert_eq!(e, invalid(None, want.clone()), "declare");
        // A set of one says its problem as `declare` does.
        let e = f.catalog.apply(owner, vec![d.clone()]).await;
        let e = e.expect_err("a declaration reads only what its type has");
        assert_eq!(e, invalid(None, want), "apply");
    }
    // In a verb's value only `payload.<name>` is the call's: every other
    // path is the record's, and a member the payload lacks is never given.
    let p = || Place::Verb { name: "p".into() };
    let reads = || said_at(&[p()], no_field(INVOICE, "payload"));
    let member = Case::PayloadNoMember {
        member: "ghost".into(),
    };
    for (value, want) in [
        ("payload", reads()),
        ("payload.reference.deep", reads()),
        ("upper(payload)", reads()),
        ("payload.ghost", said_at(&[p(), of_field("status")], member)),
    ] {
        let d = serde_json::from_value(verb("p", "status = 'x'", value)).expect("a verb");
        let e = f.catalog.declare(owner, d).await;
        let e = e.expect_err("only `payload.<name>` is the call's");
        assert_eq!(e, invalid(None, want), "`{value}`");
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
/// text function reads a number as its text. The check reads the write's
/// verdict; what a derived field then holds is the suite's to say.
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
/// as invalid. What the field then holds is the suite's to say.
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
    let case = Case::NeverLands {
        kind: Kind::Duration,
        holds: StorageKind::Number,
        duration: true,
    };
    let on = [of_type("term"), of_field("days")];
    assert_eq!(e, invalid(Some("days"), said_at(&on, case)));
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
    let sum = lakelands_api::expr::parse("(to - from) + 1").expect("an expression");
    let case = Case::KindSides {
        expr: lakelands_api::expr::print(&sum),
        left: Kind::Duration,
        right: Kind::Number,
    };
    assert_eq!(e, invalid(None, said_at(&[Place::Where], case)));
}

/// A set names a set with `in_set('name')`, in a set's `where` and in a
/// read's (`docs/decisions/004-the-change-feed-is-the-log.md`, extended):
/// the named filter stands in the name's place. A set that is not
/// declared, one of another type, one that names it in turn and a path
/// that steps through a reference are refused at the declaration, and a
/// set declared again for another type on `of`; nothing is appended. To a
/// reader kept from a field, a set whose filter reads it, itself or
/// through a set it names, is a set that is not declared.
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
    let steps = || Case::StepsIntoReference {
        expr: "partner.name".into(),
        field: "partner".into(),
    };
    for (d, case) in [
        (
            set("s", INVOICE, "in_set('ghost')"),
            Case::SetUndeclared {
                set: "ghost".into(),
            },
        ),
        (
            set("s", INVOICE, "nope = 1 and in_set('open')"),
            Case::UnknownField {
                ty: ty(INVOICE),
                field: "nope".into(),
            },
        ),
        (
            set("s", PARTNER, "in_set('open')"),
            Case::SetOfOtherType {
                set: "open".into(),
                of: ty(INVOICE),
                ty: ty(PARTNER),
            },
        ),
        (
            set("open", INVOICE, "in_set('both')"),
            Case::SetCycle {
                set: "both".into(),
                through: ["both", "open"].into_iter().collect(),
            },
        ),
        (set("s", INVOICE, "partner.name = 'x'"), steps()),
        (serde_json::from_value(when).expect("a trigger"), steps()),
    ] {
        let e = c.declare(owner, d.clone()).await;
        let e = e.expect_err("the declaration is refused");
        let at = [d.place()];
        assert_eq!(e, invalid(None, said_at(&at, case)), "{d:?}");
    }
    let e = c.declare(owner, set("open", PARTNER, "name = 'x'")).await;
    let e = e.expect_err("a set is of one type");
    let case = Case::SetRetyped {
        was: ty(INVOICE),
        ty: ty(PARTNER),
    };
    let at = Place::ObjectSet {
        name: "open".into(),
    };
    assert_eq!(e, invalid(Some("of"), said_at(&[at], case)));
    assert_eq!(f.head().await, head, "nothing is appended");

    // The clerk reads `status` and is kept from `secret`.
    let said = |name: &'static str| {
        let asked = keys(clerk, &format!("in_set('{name}')"));
        async move {
            asked
                .await
                .map_err(|e| format!("{e:?}").replace(name, "ghost"))
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
/// reads the field, not its name.
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
    // The refusal of a migration of `of` taking away `field` of `on`,
    // which `reader` reads, said after the migration at the reader.
    let taken = |of: &str, reader: Option<Place>, field: &str, on: &str, renamed: Option<&str>| {
        let mut at = vec![Place::AfterMigration { name: ty(of) }];
        at.extend(reader.clone());
        let case = Case::MigrationTakesRead {
            unnamed: reader.is_none(),
            field: field.into(),
            ty: ty(on),
            renamed: renamed.map(FieldName::from),
        };
        invalid(None, said_at(&at, case))
    };
    let (set, trigger) = (
        |n: &str| Place::ObjectSet { name: n.into() },
        |n: &str| Place::Trigger { name: n.into() },
    );
    let verb = |n: &str| Place::Verb { name: n.into() };
    let memo_ = || Some(of_type("memo"));
    for (to, want) in [
        (
            migration(memo(&without("a"), &["ok"]), &[]),
            taken("memo", memo_(), "a", "memo", None),
        ),
        (
            migration(memo(&without("b"), &["ok"]), &[]),
            taken("memo", Some(set("s")), "b", "memo", None),
        ),
        (
            migration(memo(&without("c"), &["ok"]), &[]),
            taken("memo", Some(trigger("t")), "c", "memo", None),
        ),
        (
            migration(memo(&without("d"), &["ok"]), &[]),
            taken("memo", Some(verb("settle")), "d", "memo", None),
        ),
        (
            migration(memo(&without("payload"), &["ok"]), &[]),
            taken("memo", Some(verb("w")), "payload", "memo", None),
        ),
        (
            migration(
                memo(&["a", "b2", "c", "d", "payload", "free"], &["ok"]),
                &[("b", "b2")],
            ),
            taken("memo", Some(set("s")), "b", "memo", Some("b2")),
        ),
        (
            migration(memo(&all, &[]), &[]),
            taken(
                "memo",
                Some(trigger("lt")),
                "ok",
                "sys.link.memo.seen_by",
                None,
            ),
        ),
    ] {
        let e = f.catalog.migrate(owner, to).await;
        let e = e.expect_err("a field a declaration reads stays");
        assert_eq!(e, want);
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
    assert_eq!(e, taken("doc", None, "body", "doc", None));
    let e = f.catalog.migrate(owner, to).await;
    let e = e.expect_err("a field a declaration reads stays");
    assert_eq!(e, taken("doc", Some(trigger("audit")), "body", "doc", None));
    // The type it migrates to is held to its own fields too.
    let mut ghost = memo(&without("free"), &["ok"]);
    ghost.fields.last_mut().expect("`loud`").derived = Some("upper(ghost)".into());
    let e = f.catalog.migrate(owner, migration(ghost, &[])).await;
    let e = e.expect_err("a migrated type reads only fields it has");
    let case = Case::UnknownField {
        ty: ty("memo"),
        field: "ghost".into(),
    };
    assert_eq!(e, invalid(None, said_at(&[of_type("memo")], case)));
    f.catalog
        .migrate(owner, migration(memo(&without("free"), &["ok"]), &[]))
        .await
        .expect("a field no declaration reads migrates away");
}

/// A view with parameters is bound only by a read that names it: a
/// pattern whose step arrives at one other than its start is refused
/// before anything is read, naming the view and the relation; started on
/// the view with its parameters, it walks.
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
        let at = Place::Step {
            step: 1,
            relation: "window".into(),
        };
        let case = Case::ParamMissing {
            ty: ty("by_status"),
            name: "state".into(),
            kind: StorageKind::Text,
            unbound: true,
        };
        assert_eq!(e, invalid(None, said_at(&[at], case)), "{project}");
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
    let case = Case::PayloadUndeclared {
        verb: approve.clone(),
        member: "status".into(),
    };
    assert_eq!(e, invalid(Some("status"), case));
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

/// A call is refused in decision record 013's order, through `call` and
/// through `propose`: an external verb's unbound executor before its
/// payload, and the payload before the record, so a member the verb does
/// not declare is refused as such on a key no record holds.
pub async fn a_call_is_refused_in_the_order_013_gives(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    declare_register(&f).await;
    let (owner, head) = (&f.cast.owner, f.head().await);
    let nope = || Patch::from([("nope".into(), json!(1))]);
    for (verb, k) in [("approve", Some(key("INV-404"))), ("register", None)] {
        for proposed in [false, true] {
            let (name, payload, opts) = (VerbName::from(verb), nope(), WriteOpts::default());
            let e = if proposed {
                let write = ProposedWrite::Call {
                    verb: name,
                    key: k.clone(),
                    payload,
                    opts,
                };
                f.catalog.propose(owner, write, "later".into()).await
            } else {
                f.catalog
                    .call(owner, &name, k.as_ref(), payload, opts)
                    .await
            }
            .expect_err("the call is refused");
            match (verb, &e) {
                ("approve", e) => {
                    let case = Case::PayloadUndeclared {
                        verb: verb.into(),
                        member: "nope".into(),
                    };
                    assert_eq!(e, &invalid(Some("nope"), case), "proposed {proposed}");
                }
                (_, Error::Unbound { seam }) => {
                    let name = "crm".to_owned();
                    assert_eq!(**seam, lakelands_api::error::Seam::Executor { name })
                }
                (_, e) => panic!("`register`, proposed {proposed}: {e:?}"),
            }
        }
    }
    assert_eq!(f.head().await, head, "nothing is appended");
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
            let verb = VerbName::from(name);
            assert_eq!(e, invalid(None, Case::VerbRedeclared { verb }));
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

/// As the store (lakelands#780): a refusal of the proposer's write at its
/// accept is whole to a reviewer shown all it names and to its proposer,
/// and to one kept from a field or a type it names answers a write
/// changed since it was proposed: an upsert's reference retired since and
/// a value its field no longer takes, and a call's payload reference; a
/// call whose `requires` reads a field the reviewer is kept from is not
/// there to it (lakelands#779); a conflict and a retirement of a record
/// retired since name only what is listed.
pub async fn an_accepts_refusal_names_only_what_its_reviewer_is_listed(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, own) = (&f.catalog, &f.cast.owner);
    let de = |v: serde_json::Value| serde_json::from_value::<Declaration>(v).unwrap();
    let inv = |level: serde_json::Value| {
        json!({"declare": "type", "name": "inv", "fields": [
            {"name": "note", "type": {"kind": "text"}}, {"name": "level", "type": level},
            {"name": "name", "type": {"kind": "reference", "target": "vault"}}]})
    };
    let to = json!([{"name": "to", "type": {"kind": "reference", "target": "vault"}}]);
    for d in [
        json!({"declare": "type", "name": "vault", "fields": [{"name": "name", "type": {"kind": "text"}}]}),
        inv(json!({"kind": "text"})),
        json!({"declare": "value_type", "name": "grade", "kind": "text", "one_of": ["low", "high"]}),
        json!({"declare": "verb", "name": "refile", "on": "inv", "risk": "high", "effect": "write",
               "payload": to, "set": [{"field": "name", "value": "payload.to"}]}),
        json!({"declare": "verb", "name": "close", "on": "inv", "risk": "high", "effect": "write",
               "requires": "level = 'low'", "set": [{"field": "note", "value": "'shut'"}]}),
    ] {
        c.declare(own, de(d)).await.unwrap();
    }
    let upsert = async |t: &str, k: &str, pairs: &[(&str, Value)]| {
        let opts = WriteOpts::default();
        c.upsert(own, &ty(t), &key(k), patch(pairs), opts)
            .await
            .unwrap();
    };
    for (t, k, pairs) in [
        ("vault", "v1", [("name", json!("v1"))]),
        ("vault", "v2", [("name", json!("v2"))]),
        ("inv", "i2", [("note", json!("n"))]),
        ("inv", "i3", [("note", json!("n"))]),
        ("inv", "i6", [("level", json!("low"))]),
    ] {
        upsert(t, k, &pairs).await;
    }
    let grants = |default, types: serde_json::Value| {
        let grants =
            json!({"default": default, "review": true, "restricted": true, "types": types});
        serde_json::from_value::<Grants>(grants).unwrap()
    };
    let kept = || json!({"inv": {"fields": {"name": "none", "level": "none"}}});
    let who = |id: &str, g| principal(f.store.lake(), PrincipalKind::Human, id, g);
    let field = who("field", grants("read", kept()));
    let typed = who(
        "typed",
        grants("read", json!({"vault": {"default": "none"}})),
    );
    let clerk = who("clerk", grants("write", kept()));
    let all = who("all", grants("read", json!({})));
    let mut shown = all.clone();
    (shown.id, shown.grants.restricted) = ("shown".into(), false);
    let propose = async |by: &Principal, write: serde_json::Value| {
        let write = serde_json::from_value(write).unwrap();
        match c.propose(by, write, "a write".into()).await.unwrap() {
            Outcome::Proposed { proposal, .. } => proposal,
            other => panic!("the write is proposed: {other:?}"),
        }
    };
    let upserted = |k: &str, p: serde_json::Value| {
        json!({"write": "upsert", "type": "inv", "key": k, "patch": p,
               "opts": {"expected_version": (k == "i2").then_some(1)}})
    };
    let called = |verb: &str, k: &str, p: serde_json::Value| json!({"write": "call", "verb": verb, "key": k, "payload": p});
    let retired = propose(own, upserted("i1", json!({"note": "v1", "name": "v1"}))).await;
    let retyped = propose(own, upserted("i4", json!({"level": "mid"}))).await;
    let moved = propose(own, upserted("i2", json!({"note": "b"}))).await;
    let refiled = propose(own, called("refile", "i2", json!({"to": "v2"}))).await;
    let closed = propose(own, called("close", "i6", json!({}))).await;
    let gone = propose(own, json!({"write": "retire", "type": "inv", "key": "i3"})).await;
    for (t, k) in [("vault", "v1"), ("vault", "v2"), ("inv", "i3")] {
        c.retire(own, &ty(t), &key(k), WriteOpts::default())
            .await
            .unwrap();
    }
    upsert("inv", "i2", &[("note", json!("m"))]).await;
    upsert("inv", "i6", &[("level", json!("high"))]).await;
    let Declaration::Type(to) = de(inv(json!({"kind": "text", "value_type": "grade"}))) else {
        unreachable!("a type");
    };
    let m = Migration {
        to,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m.clone()).await.unwrap().data;
    c.migrate_apply(own, m, &report.report_id).await.unwrap();
    let stale = |k: &str| {
        let case = Case::WriteChangedSinceProposed {
            ty: ty("inv"),
            key: Some(key(k)),
        };
        Err(invalid(None, case))
    };
    let head = f.head().await;
    for (id, k, hidden, whole, said) in [
        (
            &retired,
            "i1",
            vec![&field, &typed],
            vec![&shown],
            "not_live",
        ),
        (
            &retyped,
            "i4",
            vec![&field],
            vec![&typed, &shown],
            "not_one_of",
        ),
        (
            &moved,
            "i2",
            vec![],
            vec![&field, &typed, &shown],
            "conflict",
        ),
        (
            &refiled,
            "i2",
            vec![&field, &typed],
            vec![&shown],
            "not_live",
        ),
        (
            &closed,
            "i6",
            vec![],
            vec![&typed, &shown],
            "requires_unmet",
        ),
        (
            &gone,
            "i3",
            vec![],
            vec![&field, &typed, &shown],
            "not_found",
        ),
    ] {
        let to_all = c.accept(&all, id).await.map(drop);
        let wire = json!(to_all.as_ref().err()).to_string();
        assert!(
            wire.contains(&format!("\"{said}\"")),
            "`{k}` to `all`: {wire}"
        );
        for (who, expected) in
            (hidden.iter().map(|w| (w, stale(k)))).chain(whole.iter().map(|w| (w, to_all.clone())))
        {
            let told = c.accept(who, id).await.map(drop);
            assert_eq!(told, expected, "the accept of `{k}` by `{}`", who.id);
        }
    }
    // `close` reads `level`, so its proposal is not there to a reviewer
    // kept from it (lakelands#779).
    for who in [&field, &clerk] {
        let missing = Missing::Proposal { id: closed.clone() };
        let told = c.accept(who, &closed).await.map(drop);
        assert_eq!(
            told,
            Err(Error::NotFound { missing }),
            "`i6` to `{}`",
            who.id
        );
    }
    assert_eq!(f.head().await, head, "nothing was appended");
    let listed = c
        .proposals(own, ProposalFilter::default())
        .await
        .unwrap()
        .data;
    assert!(listed.iter().all(|p| p.state == ProposalState::Pending));
}

/// As the store (lakelands#762): to a reviewer not shown a proposal's
/// type, the proposal is not listed, and `accept` and `reject` answer as
/// for a proposal that does not exist, pending or decided, and append
/// nothing; one that restates a type related to it as it stands is listed
/// and decided.
pub async fn a_reviewer_not_shown_a_type_finds_no_proposal_on_it(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("approved"))])
        .await;
    let (pay, inv, opts) = (VerbName::from("pay"), key("INV-1"), WriteOpts::default);
    let o = f
        .catalog
        .call(&f.cast.clerk, &pay, Some(&inv), Patch::new(), opts());
    let Outcome::Proposed { proposal, .. } = o.await.unwrap() else {
        panic!("a call of `pay` is proposed");
    };
    let grants = Grants {
        default: Access::Read,
        review: true,
        types: [(
            ty(INVOICE),
            TypeGrants {
                default: Some(Access::None),
                ..Default::default()
            },
        )]
        .into(),
        ..Grants::default()
    };
    let kept = principal(f.store.lake(), PrincipalKind::Agent, "kept", grants);
    let absent = |id: &lakelands_api::ProposalId| {
        let missing = Missing::Proposal { id: id.clone() };
        Err::<(), _>(Error::NotFound { missing })
    };
    let mut wrong = Vec::new();
    let filter = ProposalFilter::default;
    let listed = f.catalog.proposals(&kept, filter()).await.unwrap().data;
    if listed.iter().any(|p| p.id == proposal) {
        wrong.push(format!("the listing holds the call: {listed:?}"));
    }
    // A bundle that refers a new type to `invoice` is not there either.
    let stub = json!({"declare": "type", "name": "stub",
                      "fields": [{"name": "i", "type": {"kind": "reference", "target": INVOICE}}]});
    let declarations = vec![serde_json::from_value(stub).unwrap()];
    let write = ProposedWrite::Declare { declarations };
    let o = f
        .catalog
        .propose(&f.cast.owner, write, "a stub".into())
        .await;
    let Outcome::Proposed {
        proposal: referring,
        ..
    } = o.unwrap()
    else {
        panic!("a bundle is proposed");
    };
    let listed = f.catalog.proposals(&kept, filter()).await.unwrap().data;
    let accept = f.catalog.accept(&kept, &referring).await.map(|_| ());
    if listed.iter().any(|p| p.id == referring) || accept != absent(&referring) {
        wrong.push(format!(
            "the bundle referring to `invoice`: listed {listed:?}, accept {accept:?}"
        ));
    }
    let head = f.head().await;
    for state in ["pending", "decided"] {
        let accept = f.catalog.accept(&kept, &proposal).await.map(|_| ());
        let reject = f.catalog.reject(&kept, &proposal, "no".into()).await;
        for (door, said) in [("accept", accept), ("reject", reject.map(|_| ()))] {
            if said != absent(&proposal) {
                wrong.push(format!("{door} of the {state} call answered {said:?}"));
            }
        }
        if state == "pending" {
            assert_eq!(f.head().await, head, "nothing was appended");
            f.catalog
                .accept(&f.cast.reviewer, &proposal)
                .await
                .expect("a reviewer shown `invoice` accepts the call");
        }
    }
    // A migration proposed while `xx` is keyed by a reference to `hh`, read
    // once `xx` refers to `vv`: its reader was not shown `xx` where it was
    // proposed, so the entry is not there to it, shown `xx` now though it is.
    let text = json!([{"name": "n", "type": {"kind": "text"}}]);
    let keyed = |to: &str| json!([{"name": "f", "type": {"kind": "reference", "target": to}}]);
    for (name, fields) in [("hh", text.clone()), ("vv", text), ("xx", keyed("hh"))] {
        let mut d = json!({"declare": "type", "name": name, "fields": fields});
        if name == "xx" {
            d["key"] = json!(["f"]);
        }
        let declared = f
            .catalog
            .declare(&f.cast.owner, serde_json::from_value(d).unwrap());
        declared.await.expect("the owner declares");
    }
    let xx = |fields: serde_json::Value| Migration {
        to: serde_json::from_value(json!({"name": "xx", "key": ["f"], "fields": fields})).unwrap(),
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let migration = Box::new(xx(keyed("vv")));
    let write = ProposedWrite::Migrate {
        migration,
        report: Default::default(),
    };
    let o = f
        .catalog
        .propose(&f.cast.owner, write, "to vv".into())
        .await;
    let Outcome::Proposed {
        proposal: retargeted,
        ..
    } = o.unwrap()
    else {
        panic!("a migration is proposed");
    };
    let mut wider = keyed("vv");
    wider
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "m", "type": {"kind": "text"}}));
    let m = xx(wider);
    let report = f
        .catalog
        .migrate(&f.cast.owner, m.clone())
        .await
        .unwrap()
        .data;
    let applied = f
        .catalog
        .migrate_apply(&f.cast.owner, m, &report.report_id)
        .await;
    applied.expect("the owner migrates `xx`");
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    let grants = Grants {
        default: Access::Read,
        review: true,
        types: [(ty("hh"), hidden)].into(),
        ..Grants::default()
    };
    let kept_hh = principal(f.store.lake(), PrincipalKind::Agent, "kept_hh", grants);
    let listed = f
        .catalog
        .proposals(&kept_hh, filter())
        .await
        .map(|r| r.data);
    let accepted = f.catalog.accept(&kept_hh, &retargeted).await.map(drop);
    let rejected = f.catalog.reject(&kept_hh, &retargeted, "no".into()).await;
    let rejected = rejected.map(drop);
    let unlisted = listed
        .as_ref()
        .is_ok_and(|l| l.iter().all(|p| p.id != retargeted));
    if !unlisted || [&accepted, &rejected] != [&absent(&retargeted); 2] {
        wrong.push(format!(
            "the migration of `xx` to `vv`, to one kept from `hh`: listed {listed:?}, accepted {accepted:?}, rejected {rejected:?}"
        ));
    }
    // `fare` relates to `invoice` through a link field and one computed
    // from it: restated as it stands with a field added, as a bundle and as
    // a migration, it names no `invoice`, the shapes compared completed.
    let link = json!([{"name": "amt", "type": {"kind": "number"}},
                      {"name": "big", "type": {"kind": "boolean"}, "authority": "derived",
                       "derived": "amt > 10"}]);
    let fare = |more: Option<&str>| {
        let mut fields = vec![json!({"name": "n", "type": {"kind": "text"}})];
        fields.extend(more.map(|m| json!({"name": m, "type": {"kind": "text"}})));
        json!({"name": "fare", "fields": fields,
               "relations": [{"name": "of_invoice", "target": INVOICE, "fields": link}]})
    };
    let mut d = fare(None);
    d["declare"] = json!("type");
    let declared = f
        .catalog
        .declare(&f.cast.owner, serde_json::from_value(d).unwrap());
    declared.await.expect("the owner declares `fare`");
    let mut again = fare(Some("extra"));
    again["declare"] = json!("type");
    let writes = [
        ProposedWrite::Declare {
            declarations: vec![serde_json::from_value(again).unwrap()],
        },
        ProposedWrite::Migrate {
            migration: Box::new(Migration {
                to: serde_json::from_value(fare(Some("more"))).unwrap(),
                renames: Vec::new(),
                relation_renames: Vec::new(),
            }),
            report: Default::default(),
        },
    ];
    for write in writes {
        let o = f
            .catalog
            .propose(&f.cast.owner, write, "`fare` again".into())
            .await;
        let Outcome::Proposed {
            proposal: restated, ..
        } = o.unwrap()
        else {
            panic!("a restatement is proposed");
        };
        let listed = f.catalog.proposals(&kept, filter()).await.map(|r| r.data);
        let rejected = f.catalog.reject(&kept, &restated, "no".into()).await;
        let held = (listed.as_ref()).is_ok_and(|l| l.iter().any(|p| p.id == restated));
        if !held || rejected.is_err() {
            wrong.push(format!(
                "`fare` restated, to one not shown `invoice`: listed {listed:?}, rejected {rejected:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "to a reviewer not shown `invoice`: {wrong:?}"
    );
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
    let case = Case::UnknownField {
        ty: ty(INVOICE),
        field: "nowhere".into(),
    };
    let on = [of_type(INVOICE), of_field("vec")];
    assert_eq!(e, invalid(Some("vec"), said_at(&on, case)));
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
    let case = Case::NotLive {
        field: "to".into(),
        ty: ty(PARTNER),
        key: key("P-404"),
    };
    assert_eq!(e, constraint("to", case));
    for (on, k, relation) in [
        ("invoice", "INV-1", "billed_by"),
        ("contract", "C-1", "nowhere"),
    ] {
        let e = link(on, k, relation).await;
        let case = Case::NoLinkRelation {
            ty: ty(on),
            relation: relation.into(),
        };
        assert_eq!(e, invalid(None, case), "{on}.{relation}");
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

/// A value type's renames are judged by the contract's one rule: a value renamed
/// to itself or twice, one the declaration in force does not accept, one
/// the new declaration does not, any rename of a value type that is not
/// text, a value a key field holds; a caller not shown every field the
/// renames write is not granted them. Applied, a record that held the old
/// value holds the new one, and the old one is refused
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, extended).
pub async fn a_value_types_renames_are_refused_and_applied_as_the_store_does(
    store: Arc<dyn LakeStore>,
) {
    use lakelands_api::write::ValueTypeMigration;
    let f = setup(store).await;
    let (c, own, architect) = (&f.catalog, &f.cast.owner, &f.cast.architect);
    let decl = |v: Value| -> Declaration { serde_json::from_value(v).unwrap() };
    let states = |one_of: &[&str]| -> lakelands_api::ontology::ValueTypeDecl {
        serde_json::from_value(json!({"name": "memo_state", "kind": "text", "one_of": one_of}))
            .unwrap()
    };
    let renamed = |one_of: &[&str], pairs: &[(&str, &str)]| ValueTypeMigration {
        to: states(one_of),
        renames: pairs
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
    };
    c.declare(
        own,
        Declaration::ValueType(states(&["open", "filed", "void"])),
    )
    .await
    .expect("the states are declared");
    let score = json!({"declare": "value_type", "name": "memo_score", "kind": "number"});
    c.declare(own, decl(score))
        .await
        .expect("the score is declared");
    let memo = json!({"declare": "type", "name": "memo", "fields": [
        {"name": "state", "type": {"kind": "text", "value_type": "memo_state"}}]});
    c.declare(own, decl(memo))
        .await
        .expect("`memo` is declared");
    let state = |s: &str| patch(&[("state", json!(s))]);
    c.upsert(
        own,
        &ty("memo"),
        &key("m1"),
        state("open"),
        WriteOpts::default(),
    )
    .await
    .expect("`m1` is open");
    let to = ["active", "filed", "void"];
    let at = |from: &str, to: &str, case: Case| {
        let (from, to) = (from.into(), to.into());
        said_at(&[Place::ValueRename { from, to }], case)
    };
    for (m, want) in [
        (
            renamed(&to, &[("open", "open")]),
            at("open", "open", Case::ValueRenameSame),
        ),
        (
            renamed(&to, &[("open", "active"), ("open", "void")]),
            at("open", "void", Case::ValueRenameTwice),
        ),
        (
            renamed(&to, &[("gone", "active")]),
            at("gone", "active", Case::ValueRenameOldRefused),
        ),
        (
            renamed(&to, &[("open", "nope")]),
            at("open", "nope", Case::ValueRenameNewRefused),
        ),
        (
            ValueTypeMigration {
                to: serde_json::from_value(json!({"name": "memo_score", "kind": "number",
                                                  "constraint": "value > 0"}))
                .unwrap(),
                renames: vec![("1".into(), "2".into())],
            },
            Case::ValueRenameNotText {
                value_type: "memo_score".into(),
            }
            .into(),
        ),
    ] {
        let e = c.migrate_value_type(own, m).await.expect_err("refused");
        assert_eq!(e, invalid(None, want));
    }
    // A key field that holds a renamed value refuses it.
    let shelf = json!({"declare": "type", "name": "shelf", "key": ["state"], "fields": [
        {"name": "state", "type": {"kind": "text", "value_type": "memo_state"}}]});
    c.declare(own, decl(shelf))
        .await
        .expect("`shelf` is declared");
    c.upsert(
        own,
        &ty("shelf"),
        &key("void"),
        state("void"),
        WriteOpts::default(),
    )
    .await
    .expect("a shelf is keyed by its state");
    let rekey = renamed(
        &["active", "filed", "gone"],
        &[("open", "active"), ("void", "gone")],
    );
    let e = c.migrate_value_type(own, rekey).await.expect_err("refused");
    let case = Case::KeyHoldsRenamed {
        ty: ty("shelf"),
        field: "state".into(),
    };
    assert_eq!(e, invalid(None, case));
    // The architect does not read a `doc`'s `body`, which the renames write.
    let doc = json!({"declare": "type", "name": "doc", "fields": [
        {"name": "body", "type": {"kind": "text", "value_type": "memo_state"}}]});
    c.declare(own, decl(doc)).await.expect("`doc` is declared");
    let m = renamed(&to, &[("open", "active")]);
    let blind = c
        .migrate_value_type(architect, m.clone())
        .await
        .expect_err("refused");
    assert_eq!(refusal(&blind), (&None, &Refusal::NotGranted), "{blind:?}");
    let report = c
        .migrate_value_type(own, m.clone())
        .await
        .expect("the preflight")
        .data;
    let counts: Vec<_> = (report.fields.iter())
        .flat_map(|u| u.renamed.iter().map(move |p| (u.ty.to_string(), p.records)))
        .collect();
    assert_eq!(
        counts,
        [
            ("doc".to_owned(), 0),
            ("memo".to_owned(), 1),
            ("shelf".to_owned(), 0)
        ],
        "{report:?}"
    );
    c.migrate_value_type_apply(own, m, &report.report_id)
        .await
        .expect("the renames are applied");
    let m1 = c
        .get(own, &ty("memo"), get("m1"))
        .await
        .expect("`m1` reads")
        .data;
    assert_eq!(m1.fields.get("state"), Some(&json!("active")));
    c.upsert(
        own,
        &ty("memo"),
        &key("m2"),
        state("open"),
        WriteOpts::default(),
    )
    .await
    .expect_err("`open` is no longer a state");
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
        renames: Vec::new(),
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
        let case = Case::ValueTypeNotTightened {
            value_type: "memo_state".into(),
            misfits: 1,
            first: Vec::new(),
        };
        e == &invalid(None, case)
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
                renames: Vec::new(),
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
    let case = Case::ValueTypeChangedSinceProposed {
        value_type: "memo_state".into(),
    };
    assert_eq!(stale, invalid(None, case));
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
/// is not listed to it, its own though it is; a reviewer who reads both
/// lists each whole. And a proposal reads only what its proposer may
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
    // A set of one says its problem at the declaration, inside the set.
    let noted = || Place::Trigger {
        name: "noted".into(),
    };
    let want = |field: &str| {
        let case = Case::UnknownField {
            ty: ty(INVOICE),
            field: field.into(),
        };
        invalid(None, said_at(&[noted()], case))
    };
    assert_eq!(absent, want("nope"));
    assert_eq!(
        hidden,
        want("secret"),
        "a field its proposer may not know of answers as one the type does not have"
    );
    // Per reader: the names its listing shows by kind, and each `more`.
    let declared = || ProposalFilter {
        kind: Some(ProposalKind::Declare),
        ..ProposalFilter::default()
    };
    let rows: [(&Principal, _, &[&str], &[bool]); 5] = [
        (clerk, ProposalFilter::default(), &["stated"], &[false]),
        (keeper, ProposalFilter::default(), &[], &[]),
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
            let want = Err(invalid(None, Case::CursorForeign));
            assert_eq!(e, want, "{door} with a cursor no lake handed out");
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
                let want = Err(invalid(None, Case::LimitRange { max }));
                assert_eq!(answer, want, "{door} with `limit` {limit}");
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
    for (what, case, find) in [
        (
            "`after` beside `cursor`",
            Case::AfterWithCursor,
            Find {
                cursor,
                after: Some(resume.clone()),
                ..first.clone()
            },
        ),
        (
            "`after` beside `aggregate`",
            Case::AfterWithAggregate,
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
            Case::AfterCount { values: 2, keys: 1 },
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
        assert_eq!(e, invalid(None, case), "{what}");
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
    let case = Case::AfterUnfit {
        index: 0,
        field: "amount".into(),
        why: Box::new(Case::NotDecimal {
            value: "twelve".into(),
        }),
    };
    assert_eq!(e, invalid(Some("amount"), case));
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

/// A poll examines at most `FEED_EXAMINED` events (lakelands#558), and a
/// filtered one reads only what its filter admits (lakelands#678): one that
/// matches nothing answers no events and `next` past the head in one poll;
/// one that matches answers its match however far from its position, with
/// `next` past the match when the poll is full and past the head when it is
/// not; and a poll with no filter answers at most the bound, whatever its
/// `limit`, and `next` at it.
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
    let none = feed.poll(from, &filter(&["trigger_fired"], &[]), 100).await;
    let none = none.unwrap();
    assert!(none.events.is_empty());
    assert_eq!(none.next, none.head.next(), "a poll from {from}");
    let last = filter(&[], &["B-4199"]);
    let full = feed.poll(from, &last, 1).await.unwrap();
    let p = full
        .events
        .first()
        .map(|e| e.position)
        .expect("`B-4199`'s event");
    assert_eq!((full.events.len(), full.next), (1, p.next()));
    let rest = feed.poll(from, &last, u32::MAX).await.unwrap();
    let found: Vec<_> = rest.events.iter().map(|e| e.position).collect();
    assert_eq!((found, rest.next), (vec![p], rest.head.next()));
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
        named: None,
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

/// Every path a pattern's rows are ordered or projected by is read as the
/// rows hold it, a bound name and then one of its fields or `key`
/// (lakelands#752): a path no bound name starts, one past a field, and a
/// type declared but not bound are refused as `pattern_no_path`; a field
/// its reader may not know of answers as one its type does not have,
/// ordered or projected; `<type>.key` is answered, and a masked field
/// projected is left out of the row. Of two faults, the path no row holds
/// is answered first, as every path is read before one is probed.
pub async fn every_path_a_pattern_names_is_one_its_rows_hold(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let own = &f.cast.owner;
    let p1 = patch(&[("name", json!("Ada"))]);
    let (pt, opts) = (ty(PARTNER), WriteOpts::default());
    f.catalog
        .upsert(own, &pt, &key("P1"), p1, opts)
        .await
        .unwrap();
    f.write(
        own,
        "INV-1",
        &[("partner", json!("P1")), ("note", json!("n"))],
    )
    .await;
    let ask = |stepped: bool, path: &str, ordered: bool| {
        let (c, who) = (f.catalog.clone(), f.cast.clerk.clone());
        let steps = if stepped {
            json!([{"relation": "billed_by"}])
        } else {
            json!([])
        };
        let mut p = json!({"start": {"type": INVOICE}, "steps": steps, "project": {"as": "rows"}});
        match ordered {
            true => p["order_by"] = json!([{"field": path}]),
            false => p["project"]["fields"] = json!([path]),
        }
        async move {
            let p: Pattern = serde_json::from_value(p).expect("a pattern");
            c.query(&who, p).await
        }
    };
    let no_path = |path: &str| invalid(None, Case::PatternNoPath { path: path.into() });
    for ordered in [true, false] {
        for (stepped, path) in [
            (true, "ghost.x"),
            (true, "zz"),
            (true, "invoice.status.x"),
            (false, "partner.name"),
            (false, "partner.zz"),
        ] {
            let e = ask(stepped, path, ordered).await.unwrap_err();
            assert_eq!(
                e,
                no_path(path),
                "`{path}` (ordered {ordered}) names no path"
            );
        }
        for (path, field) in [("invoice.secret", "secret"), ("partner.zz", "zz")] {
            let e = ask(true, path, ordered).await.unwrap_err();
            assert!(no_field(&e, field), "`{path}` (ordered {ordered}): {e:?}");
        }
        for (stepped, path) in [(true, "partner.key"), (false, "invoice.key")] {
            let got = ask(stepped, path, ordered).await;
            assert!(got.is_ok(), "`{path}` (ordered {ordered}): {got:?}");
        }
    }
    // Two faults: every path is read before one is probed, on every lake.
    for ordered in [true, false] {
        let mut p = json!({"start": {"type": INVOICE}, "project": {"as": "rows"}});
        let two = json!(["invoice.secret", "ghost.x"]);
        match ordered {
            true => p["order_by"] = json!([{"field": "invoice.secret"}, {"field": "ghost.x"}]),
            false => p["project"]["fields"] = two,
        }
        let p: Pattern = serde_json::from_value(p).expect("a pattern");
        let e = f.catalog.query(&f.cast.clerk, p).await.unwrap_err();
        assert_eq!(e, no_path("ghost.x"), "two faults (ordered {ordered})");
    }
    // A masked field projected is left out of the row, as an absent one.
    let got = ask(false, "invoice.note", false).await;
    let rows = match got.map(|r| r.data) {
        Ok(lakelands_api::query::QueryResult::Rows { rows, .. }) => rows,
        other => panic!("a masked field projected: {other:?}"),
    };
    assert!(
        rows.len() == 1 && rows[0].is_empty(),
        "a masked field projected: {rows:?}"
    );
}

/// A value type's rename on a lake, as the owner preflights it and as the
/// owner and a reader who is not shown `credit.hush` or the type `cold`
/// are listed the owner's proposal of it.
async fn a_rename_reported(store: Arc<dyn LakeStore>) -> [ValueTypeReport; 3] {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let st = json!({"kind": "text", "value_type": "st"});
    for d in [
        json!({"declare": "value_type", "name": "st", "kind": "text", "one_of": ["a", "b"]}),
        json!({"declare": "type", "name": "credit",
            "fields": [{"name": "s", "type": st}, {"name": "hush", "type": st}]}),
        json!({"declare": "type", "name": "cold", "fields": [{"name": "s", "type": st}]}),
        json!({"declare": "object_set", "name": "hot", "of": "credit", "where": "s = 'a'"}),
        json!({"declare": "object_set", "name": "chill", "of": "cold", "where": "s = 'a'"}),
        json!({"declare": "function", "name": "fa", "backing": "expr",
            "inputs": [{"name": "x", "type": {"kind": "text"}}], "expr": "x = 'a'"}),
    ] {
        c.declare(own, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let rule = json!({"on": "credit", "field": "s", "from": "a", "to": "b", "when": "true"});
    for (t, k, v) in [
        ("credit", "k1", json!({"s": "a", "hush": "a"})),
        ("cold", "k1", json!({"s": "a"})),
        (lakelands_api::seam::AGING_RULE_TYPE, "r1", rule),
    ] {
        let p = serde_json::from_value(v).unwrap();
        c.upsert(own, &ty(t), &key(k), p, WriteOpts::default())
            .await
            .unwrap();
    }
    let to = serde_json::from_value(json!({"name": "st", "kind": "text", "one_of": ["aa", "b"]}));
    let to: lakelands_api::ontology::ValueTypeDecl = to.unwrap();
    let renames = vec![("a".to_owned(), "aa".to_owned())];
    let m = ValueTypeMigration {
        to: to.clone(),
        renames: renames.clone(),
    };
    let preflight = c
        .migrate_value_type(own, m)
        .await
        .expect("the preflight")
        .data;
    let report = Default::default();
    let w = ProposedWrite::MigrateValueType {
        to: Box::new(to),
        renames,
        report,
    };
    c.propose(own, w, "renamed".into()).await.expect("proposed");
    let grants = json!({"default": "write", "schema": true, "review": true, "types": {
        "credit": {"fields": {"hush": "none"}}, "cold": {"default": "none"}}});
    let reader = principal(
        store.lake(),
        PrincipalKind::Agent,
        "reader",
        serde_json::from_value(grants).unwrap(),
    );
    let listed = |who: Principal| {
        let c = c.clone();
        async move {
            let listed = c
                .proposals(&who, ProposalFilter::default())
                .await
                .unwrap()
                .data;
            match listed.into_iter().next().and_then(|p| p.write) {
                Some(ProposedWrite::MigrateValueType { report, .. }) => *report,
                other => panic!("the rename is listed to `{}`: {other:?}", who.id),
            }
        }
    };
    [preflight, listed(own.clone()).await, listed(reader).await]
}

/// A value type's report says what may name an old value in a condition,
/// as its reader is shown it (lakelands#766):
/// the owner's preflight and listing name both object sets and the aging
/// rule, under one id; a reader not shown `cold` or `credit.hush` is
/// listed the use and the conditions it is shown, the rest as unshown,
/// under an id made again for it.
pub async fn a_value_types_report_names_its_conditions_as_the_store_does(
    store: Arc<dyn LakeStore>,
) {
    let [pre, own, reader] = a_rename_reported(store).await;
    let set = |n: &str| format!("the object_set declaration `{n}`");
    let fa = "the function declaration `fa`".to_owned();
    let all = vec![
        fa.clone(),
        set("chill"),
        set("hot"),
        "the aging rule `r1`".to_owned(),
    ];
    assert_eq!(
        (&pre.conditions, pre.conditions_unshown),
        (&all, false),
        "{pre:?}"
    );
    assert_eq!(own, pre, "the owner is listed its preflight");
    let shown = vec![fa, set("hot"), "the aging rule `r1`".to_owned()];
    assert_eq!(
        (&reader.conditions, reader.conditions_unshown),
        (&shown, true),
        "{reader:?}"
    );
    let uses: Vec<_> = reader
        .fields
        .iter()
        .map(|u| format!("{}.{}", u.ty, u.field))
        .collect();
    assert_eq!(uses, ["credit.s"], "the reader is listed the use it reads");
    assert_ne!(
        reader.report_id, pre.report_id,
        "its id is made again for it"
    );
}

/// Ingest judges a record in 016's order around its duplicate test
/// (lakelands#761): an empty record on the source's own live key changes
/// nothing and counts as a duplicate, and on a free key it is refused as
/// empty; an unchanged record of a link type whose relation was renamed
/// is refused before the test, not taken as a duplicate.
pub async fn ingest_judges_a_record_around_its_duplicate_test(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let (c, own, importer) = (&f.catalog, &f.cast.owner, &f.cast.importer);
    let feed = |who: &Principal, ty: &str, k: &str, fields: Value| {
        let c = c.clone();
        let (who, ty, k) = (who.clone(), ty.to_owned(), k.to_owned());
        async move {
            let r = SourceRecord {
                upstream: None,
                ty: ty.into(),
                key: k.into(),
                fields: serde_json::from_value(fields).expect("a patch"),
                seen_at: None,
            };
            lakelands_api::ingest_all(c.as_ref(), &who, &"feed".into(), vec![r]).await
        }
    };
    feed(importer, INVOICE, "INV-9", json!({"amount": "5"}))
        .await
        .expect("the importer feeds `INV-9`");
    let head = f.head().await;
    let again = feed(importer, INVOICE, "INV-9", json!({})).await;
    let counted = again
        .as_ref()
        .ok()
        .and_then(|w| w.first())
        .map(|w| (w.records, w.duplicates));
    assert_eq!(
        counted,
        Some((0, 1)),
        "an empty record on its own `INV-9`: {again:?}"
    );
    assert_eq!(f.head().await, head, "and nothing is appended");
    let e = feed(importer, INVOICE, "INV-10", json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        invalid(None, Case::PatchEmpty),
        "an empty record on a free key"
    );
    let bag = |relation: &str| -> TypeDecl {
        serde_json::from_value(json!({"name": "bag",
            "fields": [{"name": "n", "type": {"kind": "text"}}],
            "relations": [{"name": relation, "target": "box", "authority": "source"}]}))
        .expect("a type")
    };
    let mut boxes = bag("none");
    (boxes.name, boxes.relations) = ("box".into(), Vec::new());
    for (t, k) in [(boxes, "x1"), (bag("in_box"), "b1")] {
        let name = t.name.clone();
        c.declare(own, Declaration::Type(t)).await.unwrap();
        let p = patch(&[("n", json!(k))]);
        c.upsert(own, &name, &key(k), p, WriteOpts::default())
            .await
            .unwrap();
    }
    let old = "sys.link.bag.in_box";
    let link = || json!({"from": "b1", "to": "x1"});
    feed(own, old, "b1:x1", link())
        .await
        .expect("a link over `in_box`");
    let m = Migration {
        to: bag("inside"),
        renames: Vec::new(),
        relation_renames: vec![("in_box".into(), "inside".into())],
    };
    let report = c.migrate(own, m.clone()).await.expect("the preflight").data;
    c.migrate_apply(own, m, &report.report_id)
        .await
        .expect("`in_box` is renamed `inside`");
    let e = feed(own, old, "b1:x1", link()).await.unwrap_err();
    let ty = TypeName::from(old);
    assert_eq!(
        e,
        invalid(None, Case::UnclaimedLinkType { ty }),
        "the old link again"
    );
}

/// Of the system types, a verb names a relation's link type alone
/// (lakelands#759): a verb on `sys.link.claim.on_trip` is declared and sets
/// a link's field, and a holder of `only` on `claim` has its call pinned,
/// refused at accept once the link's end record moved; a verb on
/// `sys.setting` or `sys.aging_rule` is reserved, while an object set may
/// still hold settings.
pub async fn a_verb_names_no_system_type_but_a_link_type(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let declare = |v: Value| {
        let c = c.clone();
        async move { c.declare(own, serde_json::from_value(v).unwrap()).await }
    };
    declare(json!({"declare": "type", "name": "trip",
        "fields": [{"name": "n", "type": {"kind": "text"}}]}))
    .await
    .unwrap();
    declare(json!({"declare": "type", "name": "claim",
        "fields": [{"name": "holder", "type": {"kind": "text"}}],
        "relations": [{"name": "on_trip", "target": "trip",
            "fields": [{"name": "label", "type": {"kind": "text"}}]}]}))
    .await
    .unwrap();
    let opts = WriteOpts::default;
    let p = |pairs: &[(&str, Value)]| patch(pairs);
    c.upsert(
        own,
        &ty("trip"),
        &key("t1"),
        p(&[("n", json!("t"))]),
        opts(),
    )
    .await
    .unwrap();
    let alice = p(&[("holder", json!("alice"))]);
    c.upsert(own, &ty("claim"), &key("c1"), alice, opts())
        .await
        .unwrap();
    let (claim, on_trip, link) = (ty("claim"), "on_trip".into(), key("c1:t1"));
    c.link(
        own,
        &claim,
        &key("c1"),
        &on_trip,
        &key("t1"),
        Patch::new(),
        opts(),
    )
    .await
    .unwrap();
    let verb = |name: &str, on: &str, risk: &str, field: &str, value: &str| {
        json!({"declare": "verb", "name": name, "on": on, "risk": risk, "effect": "write",
            "set": [{"field": field, "value": value}]})
    };
    let lt = "sys.link.claim.on_trip";
    declare(verb("tag", lt, "low", "label", "'x'"))
        .await
        .unwrap();
    declare(verb("relabel", lt, "high", "label", "'y'"))
        .await
        .unwrap();
    let tagged = c
        .call(own, &"tag".into(), Some(&link), Patch::new(), opts())
        .await;
    assert!(
        matches!(tagged, Ok(Outcome::Applied { .. })),
        "the owner tags `c1:t1`: {tagged:?}"
    );
    let grants = json!({"default": "write", "types": {"claim": {"only": {"holder": "alice"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let alice = &principal(store.lake(), PrincipalKind::Agent, "alice", grants);
    let called = c
        .call(alice, &"relabel".into(), Some(&link), Patch::new(), opts())
        .await;
    let Ok(Outcome::Proposed { proposal, .. }) = called else {
        panic!("her `relabel` of `c1:t1` is proposed: {called:?}")
    };
    let bob = p(&[("holder", json!("bob"))]);
    c.upsert(own, &claim, &key("c1"), bob, opts())
        .await
        .unwrap();
    let e = c.accept(own, &proposal).await.unwrap_err();
    let (ty, key) = (TypeName::from(lt), Some(link));
    let moved = Case::WriteChangedSinceProposed { ty, key };
    assert_eq!(
        e,
        invalid(None, moved),
        "accepted after `c1` was handed to bob"
    );
    for (name, on, field) in [
        ("poke", "sys.setting", "value"),
        ("age", "sys.aging_rule", "on"),
    ] {
        let e = declare(verb(name, on, "low", field, "'x'"))
            .await
            .unwrap_err();
        let reserved = Error::Refused {
            field: None,
            reason: Refusal::Reserved,
        };
        assert_eq!(e, reserved, "a verb on `{on}`");
    }
    let set = json!({"declare": "object_set", "name": "all_settings", "of": "sys.setting",
        "where": "true"});
    declare(set).await.expect("an object set holds settings");
}

/// A row binds each record a pattern reaches under its name, the one `as`
/// gives or its type's (lakelands#680): two records of one type are both
/// in the row under names of their own, a pattern that binds one name
/// twice is refused before a record is read, rather than one record
/// overwriting the other, and `as` where no row binds is refused.
pub async fn a_row_binds_each_record_under_a_name_of_its_own(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    for d in [
        json!({"declare": "type", "name": "person", "fields": [
            {"name": "name", "type": {"kind": "text"}},
            {"name": "boss", "type": {"kind": "reference", "target": "person"}}],
            "relations": [{"name": "reports_to", "target": "person", "via": "boss"}]}),
        json!({"declare": "type", "name": "spoke", "fields": [
            {"name": "owner", "type": {"kind": "reference", "target": "person"}}],
            "relations": [{"name": "owned_by", "target": "person", "via": "owner"}]}),
    ] {
        let d = serde_json::from_value::<Declaration>(d).unwrap();
        c.declare(own, d).await.unwrap();
    }
    for (t, k, fields) in [
        ("person", "p2", json!({"name": "Bob"})),
        ("person", "p1", json!({"name": "Ann", "boss": "p2"})),
        ("spoke", "s1", json!({"owner": "p1"})),
    ] {
        let p = serde_json::from_value(fields).unwrap();
        let (t, k, opts) = (ty(t), key(k), WriteOpts::default());
        c.upsert(own, &t, &k, p, opts).await.unwrap();
    }
    let ask = |p: Value| {
        let c = c.clone();
        async move {
            let p: Pattern = serde_json::from_value(p).expect("a pattern");
            c.query(own, p)
                .await
                .map(|r| serde_json::to_value(r.data).unwrap())
        }
    };
    let from_s1 = |first: Value, project: Value| {
        json!({"start": {"type": "spoke", "key": "s1"}, "project": project,
            "steps": [first, {"relation": "reports_to"}], "order_by": [{"field": "owner.name"}]})
    };
    let named = json!({"relation": "owned_by", "as": "owner"});
    let rows = ask(from_s1(named, json!({"as": "rows"}))).await.unwrap();
    let row = &rows["rows"][0];
    assert_eq!(
        (&row["spoke.key"], &row["owner.name"], &row["person.name"]),
        (&json!("s1"), &json!("Ann"), &json!("Bob")),
        "s1's owner Ann and Ann's boss Bob, each under its name: {rows}"
    );
    let e = ask(from_s1(
        json!({"relation": "owned_by"}),
        json!({"as": "rows"}),
    ))
    .await
    .unwrap_err();
    let (step, relation) = (2, "reports_to".into());
    let said = Said::from(Case::PatternBindsTwice {
        name: "person".into(),
    });
    assert_eq!(
        e,
        invalid(None, said.within(Place::Step { step, relation }))
    );
    let rows = ask(
        json!({"start": {"type": "person", "key": "p2"}, "project": {"as": "rows"},
        "steps": [{"relation": "reports_to", "walk": "in", "as": "report"}]}),
    )
    .await
    .unwrap();
    let row = &rows["rows"][0];
    assert_eq!(
        (&row["person.key"], &row["report.key"]),
        (&json!("p2"), &json!("p1")),
        "the start p2 and p1, who reports to it: {rows}"
    );
    let degree = json!({"start": {"type": "spoke", "as": "s"}, "project": {"as": "degree"},
        "steps": [{"relation": "owned_by"}]});
    let e = ask(degree).await.unwrap_err();
    assert_eq!(e, invalid(None, Case::PatternAsNotRows));
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
            named: None,
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
    // Both problems are named, each at its declaration: the type declared
    // twice, and the verb on an undeclared type.
    let haunt = || Place::Verb {
        name: "haunt".into(),
    };
    let twice = invalid(None, said_at(&[of_type(PARTNER)], Case::SetDeclaresTwice));
    let ghost = Case::Undeclared { ty: ty("ghost") };
    let ghost = invalid(None, said_at(&[haunt()], ghost));
    let problems = vec![twice, ghost];
    assert_eq!(e, invalid(None, Case::SetRefused { count: 2, problems }));
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
/// holds at the door and over an ingest, as one window or as two; its records refuse the
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
                    named: None,
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
    // one window as at the door. Where the window is cut is the store's:
    // over the wire the two links may arrive as two windows, and then the
    // first is committed and the second refused (lakelands#885). Either
    // way no two links of `d1` are live.
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
    let mut answers = vec![crate::stream::next(&mut commits).await];
    if matches!(&answers[0], Some(Ok(c)) if (c.records, c.duplicates) == (1, 0)) {
        answers.push(crate::stream::next(&mut commits).await);
    }
    // One window is refused for the window's links, two for the link the
    // first committed, which it names: nothing else passes.
    let last = answers.last().and_then(|a| a.as_ref()?.as_ref().err());
    let held = match last.and_then(Error::case).map(|s| &s.case) {
        Some(Case::WindowLinksAlready { .. }) => answers.len() == 1,
        Some(Case::LinkedAlreadyFrom { to, .. }) => {
            answers.len() == 2 && to.as_ref().is_some_and(|k| k.as_str() == "p1")
        }
        _ => false,
    };
    assert!(
        held,
        "two witnesses for d1 break many_to_one, in one window or in two: {answers:?}"
    );
    let witnessed = ty("sys.link.doc.witnessed_by");
    let live = c.find(own, &witnessed, Find::default()).await.unwrap();
    let live = serde_json::to_value(&live.data).unwrap();
    let live = live["rows"].as_array().map(Vec::len);
    assert_eq!(live, Some(answers.len() - 1), "{answers:?}");
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
                    named: None,
                }],
            ),
        )
        .await
        .unwrap();
    let json = serde_json::to_value(&after.data).unwrap();
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(0), "{json}");
    // A link that breaks both its reference and the cardinality is
    // refused for the reference: an ingested link's cardinality is judged
    // last.
    let witness = |to: &str| SourceRecord {
        upstream: None,
        ty: ty("sys.link.doc.witnessed_by"),
        key: key(&format!("d1:{to}")),
        fields: patch(&[("from", json!("d1")), ("to", json!(to))]),
        seen_at: None,
    };
    let fed = |to: &str| {
        let window = stream::of(vec![witness(to)]);
        let c = c.clone();
        async move {
            let commits = c.ingest(own, &"registry".into(), window).await;
            crate::stream::next(&mut commits.unwrap()).await
        }
    };
    assert!(matches!(fed("p1").await, Some(Ok(_))), "d1 witnessed by p1");
    let both = fed("p9").await;
    let case = both
        .as_ref()
        .and_then(|r| r.as_ref().err())
        .and_then(Error::case);
    assert!(
        matches!(case.map(|s| &s.case), Some(Case::NotLive { key, .. }) if key == "p9"),
        "a witness `p9` that is not there, d1 witnessed already: {both:?}"
    );
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
    let to = |target: &str| FieldType {
        target: Some(ty(target)),
        ..FieldType::kind(StorageKind::Reference)
    };
    let case = Case::ReshapesLinks {
        ty: ty("doc"),
        links: ty("sys.link.doc.signed_by"),
        reasons: vec![lakelands_api::error::Revision::FieldRetyped {
            field: "to".into(),
            relation: None,
            from: to("person"),
            to: to("team"),
        }],
    };
    let at = [of_type("doc")];
    assert_eq!(refused, Err(invalid(None, said_at(&at, case))));
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
    let case = Case::ChangedSinceProposed { ty: ty("badge") };
    assert_eq!(stale, Err(invalid(None, case)));
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
    assert_eq!(stale, Err(invalid(None, Case::PurgeReportStale)));
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
    let case = Case::PurgeShared {
        source: "crm".into(),
        ty: None,
        count: 1,
        first: vec![lakelands_api::error::RecordName::Of {
            ty: ty("partner"),
            key: key("p4"),
        }],
    };
    assert_eq!(refused, Err(invalid(None, case)));
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
    let case = Case::PurgeShared {
        source: crm.clone(),
        ty: Some(bill.clone()),
        count: 1,
        first: vec![lakelands_api::error::RecordName::Of {
            ty: bill.clone(),
            key: key("b1"),
        }],
    };
    for e in [refused.unwrap_err(), seen.unwrap_err()] {
        assert_eq!(e, invalid(None, case.clone()));
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
        assert_eq!(e, invalid(None, Case::PurgeReportStale));
    }
    for (t, case) in [
        ("log_line", Case::NoProvenance { ty: ty("log_line") }),
        ("partners", Case::IsAView { ty: ty("partners") }),
    ] {
        let refused = store.purge_source(&crm, Some(&ty(t)), None).await;
        let seen = store.purge_preflight(&crm, Some(&ty(t))).await;
        for e in [refused.unwrap_err(), seen.unwrap_err()] {
            assert_eq!(e, invalid(None, case.clone()), "{t}");
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
    let case = Case::ValueTypeTightens {
        value_type: "status_code".into(),
    };
    let at = [Place::ValueType {
        name: "status_code".into(),
    }];
    assert_eq!(refused, Err(invalid(None, said_at(&at, case))));
    assert_eq!(store.head().await.unwrap(), head);
    // Its preflight counts `b1`, and the apply is refused while it misfits.
    let report = c
        .migrate_value_type(own, vt(json!(["open", "void"])).into())
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
        .migrate_value_type_apply(own, vt(json!(["open", "void"])).into(), &report.report_id)
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
    let report = c
        .migrate_value_type(own, to.clone().into())
        .await
        .unwrap()
        .data;
    assert!(report.fits, "{report:?}");
    let applied = c
        .migrate_value_type_apply(own, to.into(), &report.report_id)
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
/// type declares with a scalar kind, a view's of its kept records, and is
/// refused naming anything else; `describe_type` names the fields
/// indexed, of those the principal sees
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
    // A view's order index is kept of its kept records (lakelands#668).
    let totals = by_status("totals", &[("total", StorageKind::Decimal, "sum(amount)")]);
    f.catalog
        .declare(owner, totals)
        .await
        .expect("the owner declares `totals`");
    for (k, value, at_fault) in [
        ("sort_index.totals", json!(["amount"]), "amount"),
        ("vector_index.totals", json!(["total"]), "key"),
    ] {
        let e = set(k, value.clone())
            .await
            .expect_err("a setting naming what the view's index does not keep is refused");
        let (field, reason) = refusal(&e);
        assert!(
            matches!(reason, Refusal::Invalid { .. })
                && field.as_ref().map(FieldName::as_str) == Some(at_fault),
            "`{k}` = {value}: expected invalid naming `{at_fault}`, got {reason:?} naming {field:?}"
        );
    }
    set("sort_index.totals", json!(["total", "status"]))
        .await
        .expect("a setting naming a view's declared scalar fields applies");
    // The index is named where the view is kept, or asked to be and can be,
    // as `kept` tells its reader: not asked, it has no records to index
    // (lakelands#748).
    let lifecycle = || {
        let catalog = f.catalog.clone();
        async move {
            let d = catalog
                .describe_type(owner, &ty("totals"), ReadAt::default())
                .await;
            d.expect("the view is described").data.lifecycle
        }
    };
    let l = lifecycle().await;
    let not_asked = Kept::Computed {
        why: NotKept::NotAsked,
    };
    assert!(
        l.sorted.is_empty() && l.kept == Some(not_asked),
        "a view not asked to be kept names no index: {l:?}"
    );
    set("view_kept.totals", json!(true))
        .await
        .expect("the view is asked to be kept");
    assert_eq!(
        lifecycle().await.sorted,
        vec![FieldName::from("total"), FieldName::from("status")],
        "`describe_type` names a kept view's indexed fields"
    );
}

/// To a holder of `only` on a view's source the view is its own records,
/// and whether it is kept is not told (051), so `describe_type` names no
/// order index of it, though the lake keeps one (lakelands#748).
pub async fn a_view_names_no_index_to_a_holder_of_only(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let owner = &f.cast.owner;
    let totals = by_status("totals", &[("total", StorageKind::Decimal, "sum(amount)")]);
    f.catalog
        .declare(owner, totals)
        .await
        .expect("the owner declares `totals`");
    for (k, value) in [
        ("sort_index.totals", json!(["total", "status"])),
        ("view_kept.totals", json!(true)),
    ] {
        let patch = patch(&[("key", json!(k)), ("value", value)]);
        let (setting, k) = (ty("sys.setting"), key(k));
        let wrote = f
            .catalog
            .upsert(owner, &setting, &k, patch, WriteOpts::default());
        wrote.await.expect("the owner writes the setting");
    }
    let grants = json!({"default": "read", "types": {"invoice": {"only": {"status": "open"}}}});
    let holder = Principal {
        id: "holder".into(),
        grants: serde_json::from_value(grants).expect("grants"),
        ..owner.clone()
    };
    let mut told = Vec::new();
    for who in [owner, &holder] {
        let totals = ty("totals");
        let d = f
            .catalog
            .describe_type(who, &totals, ReadAt::default())
            .await;
        let l = d.expect("the view is described").data.lifecycle;
        told.push((l.sorted, l.kept));
    }
    let only_some = Kept::Computed {
        why: NotKept::OnlySome,
    };
    assert!(
        !told[0].0.is_empty() && told[1] == (Vec::new(), Some(only_some)),
        "the owner is told the view's index and the holder of `only` none: {told:?}"
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
    let why = Case::KeptOrdersKind {
        call: "max(secret)".into(),
        kind: Kind::Text,
    };
    let at = [Place::Measure { name: "n".into() }];
    assert_eq!(*reason, said_at(&at, why), "the measure and the call");
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
    let at = [
        Place::Setting {
            key: "view_kept.secrets".into(),
        },
        Place::Measure { name: "n".into() },
    ];
    let case = Case::KeptOrdersKind {
        call: "max(secret)".into(),
        kind: Kind::Text,
    };
    assert_eq!(e, invalid(Some("key"), said_at(&at, case)));
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
        assert!(none.contains("UnknownField"), "{none}");
        let answer =
            answered(set_as(c, tuner, k, json!(["secret"])).await).replace("secret", "nope");
        assert_eq!(answer, none, "`{k}`: `secret` answers as `nope` does");
    }
    let masked = answered(set_as(c, tuner, "vector_index.invoice", json!(["note"])).await);
    assert!(
        masked.contains("VectorIndexKind"),
        "`note` is shown: {masked}"
    );

    // (iv) A view shown without its definition: that it cannot be kept,
    // and nothing of the measure, the call or what it reads.
    let whole = answered(set_as(c, owner, "view_kept.longest", json!(true)).await);
    assert!(
        whole.contains("max(secret)"),
        "the owner is told why: {whole}"
    );
    let e = set_as(c, tuner, "view_kept.longest", json!(true)).await;
    let e = e.expect_err("it cannot be kept");
    let at = Place::Setting {
        key: "view_kept.longest".into(),
    };
    let case = Case::ViewNotKeepable { ty: ty("longest") };
    assert_eq!(e, invalid(Some("key"), said_at(&[at], case)));
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
    assert!(beside.contains("UnknownField"), "{beside}");
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

/// A type keyed by a field its principal may not know of is not shown
/// (lakelands#715): every record key would hand out the field's value. To
/// one held at `none` on `ledger`'s key field, `describe_type`, `get` and
/// `find` answer as on `ghost`, which is not declared, and `describe`
/// names no `ledger`; to one who reads the key field masked, the type is
/// shown.
pub async fn a_type_keyed_by_a_field_its_principal_may_not_know_of_is_not_shown(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let (owner, c, ledger) = (&f.cast.owner, &f.catalog, ty("ledger"));
    let decl = json!({"declare": "type", "name": "ledger", "key": ["code"],
        "fields": [{"name": "code", "type": {"kind": "text"}}]});
    let decl = serde_json::from_value(decl).expect("a declaration reads");
    c.declare(owner, decl)
        .await
        .expect("the owner declares `ledger`");
    let code = patch(&[("code", json!("l1"))]);
    let l1 = key("l1");
    let written = c.upsert(owner, &ledger, &l1, code, WriteOpts::default());
    written.await.expect("the owner writes `l1`");
    let mut differ = Vec::new();
    for (id, access, shown) in [
        ("hidden", Access::None, false),
        ("masked", Access::Masked, true),
    ] {
        let held = TypeGrants {
            default: None,
            fields: [("code".into(), access)].into(),
            ..Default::default()
        };
        let grants = Grants {
            default: Access::Read,
            types: [(ledger.clone(), held)].into(),
            ..Grants::default()
        };
        let who = &principal(f.store.lake(), PrincipalKind::Agent, id, grants);
        for t in ["ledger", "ghost"].map(ty) {
            let said = [
                format!(
                    "{:?}",
                    c.describe_type(who, &t, ReadAt::default()).await.map(drop)
                ),
                format!("{:?}", c.get(who, &t, get("l1")).await.map(drop)),
                format!("{:?}", c.find(who, &t, Find::default()).await.map(drop)),
            ]
            .map(|a| a.replace("ghost", "ledger"));
            differ.push((id, t, said));
        }
        let lake = c.describe(who, ReadAt::default()).await;
        let named =
            (lake.expect("the lake is described").data.types.iter()).any(|t| t.name == ledger);
        if named != shown {
            differ.push((
                id,
                ty("describe"),
                [format!("{named}"), String::new(), String::new()],
            ));
        }
    }
    let as_ghost = |id: &str| {
        differ
            .iter()
            .filter(|d| d.0 == id)
            .map(|d| &d.2)
            .collect::<Vec<_>>()
    };
    let (hidden, masked) = (as_ghost("hidden"), as_ghost("masked"));
    assert!(
        hidden.len() == 2 && hidden[0] == hidden[1] && masked.len() == 2 && masked[0] != masked[1],
        "`ledger` keyed by `code` answers one held at `none` on `code` as `ghost`, which is not \
         declared, and one who reads `code` masked as declared: {differ:?}"
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
    let report = c.migrate_value_type(&own, narrower(&["paid"]).into()).await;
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
        .migrate_value_type_apply(&own, narrower(&["paid"]).into(), &id)
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
    let on = |e: &Error, t: &str, name: &str| {
        let case = Case::Required {
            ty: ty(t),
            field: name.into(),
        };
        e == &constraint(name, case)
    };
    let e = ingest("order", "o1")
        .await
        .expect_err("no value for `state`");
    assert!(on(&e, "order", "state"), "an ingest without `state`: {e:?}");
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
    assert!(on(&e, "memo", "total"), "an upsert without `total`: {e:?}");
    let e = upsert("tally", "s1", &[("note", json!("hi"))]).await;
    let e = e.expect_err("`twice` reads null");
    assert!(
        on(&e, "tally", "twice"),
        "an upsert whose `twice` reads null: {e:?}"
    );
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
    let case = Case::ParamMissing {
        ty: ty("by_status"),
        name: "state".into(),
        kind: StorageKind::Text,
        unbound: false,
    };
    assert_eq!(e, invalid(None, case));
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
        let case = Case::ParamUnknown {
            ty: ty(INVOICE),
            name: "anchor".into(),
            declared: None,
        };
        assert_eq!(e, invalid(None, case));
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
        named: None,
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

/// As the store: a vector search, an `also` term or a hybrid search over a
/// field that holds no vectors is refused as invalid on that field, naming
/// its kind; a hybrid search without a vector over a vector field no model
/// fills is refused on it, though no backend is bound; the caller's rights
/// come first; a caller's vector over a vector field is answered
/// (lakelands#743).
pub async fn a_vector_ranking_reads_a_vector_field(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let d = json!({"declare": "type", "name": "doc", "fields": [
        {"name": "title", "type": {"kind": "text"}, "searchable": true},
        {"name": "secret", "type": {"kind": "text"}, "searchable": true},
        {"name": "n", "type": {"kind": "number"}},
        {"name": "plain", "type": {"kind": "vector", "dimensions": 2}},
        {"name": "wide", "type": {"kind": "vector", "dimensions": 3}}]});
    let d = serde_json::from_value(d).expect("a declaration reads");
    c.declare(own, d).await.expect("the owner declares `doc`");
    let a = json!({"title": "harbour", "n": 1, "plain": [1.0, 0.0], "wide": [1.0, 0.0, 0.0]});
    let a = serde_json::from_value(a).expect("fields read");
    let (doc, key) = (TypeName::from("doc"), RecordKey::from("a"));
    let wrote = c.upsert(own, &doc, &key, a, WriteOpts::default());
    wrote.await.expect("the owner writes `a`");
    let search = |mode| Search {
        ty: "doc".into(),
        mode,
        k: 10,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let hybrid = |field: &str, vector: Option<Vec<f32>>| SearchMode::Hybrid {
        text: "harbour".into(),
        field: field.into(),
        vector,
        fields: None,
        weight: None,
        text_weight: None,
    };
    let along = |field: &str, also: &[&str]| SearchMode::Vector {
        field: field.into(),
        vector: vec![1.0, 0.0],
        weight: None,
        also: (also.iter())
            .map(|f| VectorTerm {
                field: (*f).into(),
                vector: vec![1.0, 0.0],
                weight: None,
            })
            .collect(),
    };
    let not_vector = |field: &str, kind: &str| json!({"code": "search_not_vector", "field": field, "kind": kind});
    let needs = |field: &str| json!({"code": "search_needs_vector", "field": field});
    for (what, mode, on, says) in [
        (
            "a vector search over `title`",
            along("title", &[]),
            "title",
            not_vector("title", "text"),
        ),
        (
            "a vector search over `n`",
            along("n", &[]),
            "n",
            not_vector("n", "number"),
        ),
        (
            "an `also` term over `title`",
            along("plain", &["title"]),
            "title",
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `title` with a vector",
            hybrid("title", Some(vec![1.0, 0.0])),
            "title",
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `title` without one",
            hybrid("title", None),
            "title",
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `plain` without a vector",
            hybrid("plain", None),
            "plain",
            needs("plain"),
        ),
        (
            "a hybrid search over `wide` without a vector",
            hybrid("wide", None),
            "wide",
            needs("wide"),
        ),
    ] {
        let e = c.search(own, search(mode)).await.map(|r| r.data.hits.len());
        let Err(e) = e else {
            panic!("{what} is refused; it answered {e:?}");
        };
        let (field, reason) = refusal(&e);
        let wire = serde_json::to_value(&e).expect("an error prints");
        assert!(
            field.as_ref().map(FieldName::as_str) == Some(on)
                && matches!(reason, Refusal::Invalid { .. })
                && says
                    .as_object()
                    .expect("an object")
                    .iter()
                    .all(|(k, v)| &wire["reason"][k] == v),
            "{what} is refused as {says} on `{on}`; it answered {wire}"
        );
    }
    // The caller's rights first: the hidden `secret` answers as `nosuch`,
    // and the masked `plain` is not granted.
    let grants = json!({"default": "read", "types": {"doc": {"fields": {
        "secret": "none", "plain": "masked"}}}});
    let blind = Principal {
        id: "blind".into(),
        grants: serde_json::from_value(grants).expect("grants"),
        ..own.clone()
    };
    let wire = |r: lakelands_api::Result<usize>| match r {
        Err(e) => serde_json::to_string(&e).expect("an error prints"),
        Ok(n) => format!("answered {n}"),
    };
    let hits = |r: lakelands_api::Result<_>| {
        r.map(|r: lakelands_api::Reply<lakelands_api::query::Hits>| r.data.hits.len())
    };
    let hidden = wire(hits(c.search(&blind, search(along("secret", &[]))).await));
    let absent = wire(hits(c.search(&blind, search(along("nosuch", &[]))).await));
    assert_eq!(
        hidden,
        absent.replace("nosuch", "secret"),
        "the hidden `secret` answers as `nosuch`"
    );
    let masked = c.search(&blind, search(hybrid("plain", None))).await;
    assert!(
        matches!(&masked, Err(e) if matches!(refusal(e), (Some(f), Refusal::NotGranted) if f.as_str() == "plain")),
        "a hybrid search over the masked `plain` is refused as not granted first: {:?}",
        masked.map(|r| r.data.hits.len())
    );
    // A caller's vector over a vector field is answered.
    for mode in [along("plain", &[]), hybrid("plain", Some(vec![1.0, 0.0]))] {
        let got = c
            .search(own, search(mode))
            .await
            .expect("the search answers");
        assert_eq!(
            got.data.hits.len(),
            1,
            "a caller's vector over `plain` finds `a`"
        );
    }
    // As the store, as of a position the rankings are judged by the head's
    // declaration (lakelands#756): at the head `v` holds text, `w` vectors
    // of 3, and `gone` is removed.
    let late = |v: &str, w: u32, gone: bool| {
        let mut fields = vec![
            json!({"name": "body", "type": {"kind": "text"}, "searchable": true}),
            json!({"name": "v", "type": if v == "text" { json!({"kind": "text"}) } else { json!({"kind": "vector", "dimensions": 2}) }}),
            json!({"name": "w", "type": {"kind": "vector", "dimensions": w}}),
        ];
        if gone {
            fields.push(json!({"name": "gone", "type": {"kind": "vector", "dimensions": 2}}));
        }
        let d = json!({"name": "late", "fields": fields});
        serde_json::from_value::<TypeDecl>(d).expect("a type")
    };
    let declared = c.declare(own, Declaration::Type(late("vector", 2, true)));
    declared.await.expect("the owner declares `late`");
    let (a, late_ty) = (
        json!({"body": "harbour", "gone": [1.0, 0.0]}),
        "late".into(),
    );
    let a = serde_json::from_value(a).expect("fields read");
    let wrote = c.upsert(own, &late_ty, &key, a, WriteOpts::default());
    wrote.await.expect("the owner writes `a`");
    let p = store.head().await.expect("the lake has a head");
    let m = Migration {
        to: late("text", 3, false),
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m.clone()).await.expect("the preflight");
    let applied = c.migrate_apply(own, m, &report.data.report_id).await;
    applied.expect("the migration applies");
    let then = |mode| Search {
        ty: "late".into(),
        at: ReadAt {
            as_of: Some(p),
            ..ReadAt::default()
        },
        ..search(mode)
    };
    let e = c.search(own, then(along("v", &[]))).await.map(drop);
    let wire = serde_json::to_value(e.expect_err("`v` holds text at the head")).expect("prints");
    assert_eq!(
        wire["reason"]["kind"], "text",
        "`v` is text at the head: {wire}"
    );
    let w = |n: usize| SearchMode::vector("w", vec![1.0; n]);
    let e = c.search(own, then(w(2))).await.map(drop);
    assert!(
        matches!(&e, Err(e) if matches!(refusal(e).1, Refusal::Invalid(said)
            if said.case == Case::QueryDimensions { query: 2, held: 3 })),
        "two numbers over `w`, of 3 at the head, are refused: {e:?}"
    );
    let three = c
        .search(own, then(w(3)))
        .await
        .expect("three numbers over `w`");
    let gone = c.search(own, then(along("gone", &[]))).await;
    let gone = gone.expect("a search over the removed `gone`");
    assert!(
        three.data.hits.is_empty() && gone.data.hits.is_empty(),
        "no record holds `w` and none `gone` at the head: {:?}, {:?}",
        three.data.hits,
        gone.data.hits
    );
    let e = c.search(own, then(hybrid("gone", None))).await.map(drop);
    assert!(
        matches!(&e, Err(e) if matches!(refusal(e).1, Refusal::Invalid(said)
            if matches!(said.case, Case::SearchNeedsVector { .. }))),
        "a hybrid search without a vector over the removed `gone` needs one: {e:?}"
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
        // A hybrid search without a vector over `v`, which no model fills,
        // brings its own vector (lakelands#743).
        if matches!(mode, SearchMode::Hybrid { vector: None, .. }) {
            let e = f.catalog.search(owner, search(mode)).await.map(drop);
            assert!(
                matches!(&e, Err(Error::Refused { reason: Refusal::Invalid(said), .. })
                    if matches!(said.case, Case::SearchNeedsVector { .. })),
                "the owner's {what} search over `v` is refused as needing a vector: {e:?}"
            );
            continue;
        }
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
        if matches!(mode, SearchMode::Hybrid { vector: None, .. }) {
            continue;
        }
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
/// second field fuses the same way, and a weight that cannot weigh, a
/// field named twice, or a query vector of zeros, is refused as invalid.
/// A record's vector with a component no `f32` holds is refused too.
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
        (SearchMode::vector("v", vec![0.0, 0.0]), Some("v")),
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
    let huge = patch(&[("v", json!([1e300, 0]))]);
    let e = (f.catalog)
        .upsert(owner, &ty("doc"), &key("a"), huge, WriteOpts::default())
        .await
        .expect_err("a record's `v` of [1e300, 0] is refused");
    assert!(
        matches!(&e, Error::Refused { field: Some(f), reason: Refusal::Constraint { .. } } if f.as_str() == "v"),
        "a record's `v` of [1e300, 0] is refused as a constraint naming `v`; it answered {e:?}"
    );
}

/// A grouped view's key is its `group_by`, judged on the view and where its
/// values come from (006, 024, extended, lakelands#782; a view declares no
/// other `key`, lakelands#788): a view
/// grouped by a reference to a type its reader is not shown, by a
/// field its source or its second source holds at `none`, or by one the
/// view itself holds so, answers `describe_type`, `get` and `find` as a
/// name that is not declared, and `describe` names none of them. A view
/// shown is described, listed and its get tool's key told by its groups,
/// its definition naming no measure of a field its reader may not know of,
/// nor one that reads such a measure, which its records do not hold either.
pub async fn a_view_is_keyed_by_its_groups_where_their_values_come_from(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let zref = json!({"name": "zref", "type": {"kind": "reference", "target": "zsecret"}});
    let count = json!([{"name": "n", "expr": "count()"}]);
    let view = |name: &str, by: &str, field: &Value, key: bool| {
        let mut v = json!({"declare": "type", "name": name, "fields": [field, f("n", "decimal")],
            "view": {"source": "pub", "group_by": [by], "measures": count}});
        if key {
            v["key"] = json!([by]);
        }
        v
    };
    let mut merged = view("m_view", "name", &f("name", "text"), false);
    merged["fields"] = json!([f("name", "text"), f("n", "decimal"), f("n2", "decimal")]);
    let n2 = json!([{"name": "n2", "expr": "count()"}]);
    merged["view"]["merge"] = json!([{"source": "aux", "group_by": ["hid2"], "measures": n2}]);
    // Grouped by `[name, hid]`, over a source whose `hid` it reads.
    let mut pair = view("knr_view", "name", &f("name", "text"), false);
    pair["fields"] = json!([f("name", "text"), f("hid", "text"), f("n", "decimal")]);
    pair["view"]["source"] = json!("pub2");
    pair["view"]["group_by"] = json!(["name", "hid"]);
    // Measured from a measure: `m` reads `n`; `open`, and `due` merged
    // beside it, read the merged `paid`;
    // `m0_view` and `p0_view` are the same views without those measures.
    let measured = |name: &str, fields: &[&str], measures: serde_json::Value| {
        let mut v = view(name, "name", &f("name", "text"), false);
        let decimal = |n: &&str| f(n, "decimal");
        let fields = std::iter::once(f("name", "text")).chain(fields.iter().map(decimal));
        v["fields"] = json!(fields.collect::<Vec<_>>());
        v["view"]["measures"] = measures;
        v
    };
    let (n, paid) = (
        json!({"name": "n", "expr": "count()"}),
        json!({"name": "paid", "expr": "count()"}),
    );
    let mut bare = vec![
        measured(
            "mm_view",
            &["n", "m"],
            json!([n, {"name": "m", "expr": "n + 1"}]),
        ),
        measured("m0_view", &[], json!([])),
        measured(
            "pm_view",
            &["n", "paid", "open", "due"],
            json!([n, {"name": "open", "expr": "paid + 1"}]),
        ),
        measured("p0_view", &["n"], json!([n])),
    ];
    let due = json!({"name": "due", "expr": "count() + paid"});
    for (v, merged) in [(2, json!([paid, due])), (3, json!([]))] {
        bare[v]["view"]["merge"] =
            json!([{"source": "pub2", "group_by": ["name"], "measures": merged}]);
    }
    for d in [
        json!({"declare": "type", "name": "zsecret", "fields": [f("n", "text")]}),
        json!({"declare": "type", "name": "pub", "fields": [f("name", "text"), zref, f("hid", "text")]}),
        json!({"declare": "type", "name": "aux", "fields": [f("hid2", "text")]}),
        json!({"declare": "type", "name": "pub2", "fields": [f("name", "text"), f("hid", "text")]}),
        view("z_view", "zref", &zref, false),
        view("h_view", "hid", &f("hid", "text"), false),
        view("h_viewk", "hid", &f("hid", "text"), true),
        merged,
        view("g_view", "name", &f("name", "text"), false),
        view("kn_view", "name", &f("name", "text"), false),
        pair,
        view("n_view", "name", &f("name", "text"), false),
        view("nh_view", "name", &f("name", "text"), true),
    ]
    .into_iter()
    .chain(bare.iter().cloned())
    {
        let d = serde_json::from_value::<Declaration>(d).expect("a declaration reads");
        c.declare(own, d).await.expect("the owner declares");
    }
    for (t, k, p) in [
        ("zsecret", "z1", patch(&[("n", json!("x"))])),
        (
            "pub",
            "p1",
            patch(&[
                ("name", json!("a")),
                ("zref", json!("z1")),
                ("hid", json!("a")),
            ]),
        ),
        ("aux", "x1", patch(&[("hid2", json!("a"))])),
        (
            "pub2",
            "q1",
            patch(&[("name", json!("a")), ("hid", json!("a"))]),
        ),
    ] {
        c.upsert(own, &ty(t), &key(k), p, WriteOpts::default())
            .await
            .expect("the owner writes");
    }
    let grants = json!({"default": "read", "review": true, "types": {"zsecret": {"default": "none"},
        "pub": {"fields": {"hid": "none"}}, "aux": {"fields": {"hid2": "none"}},
        "g_view": {"fields": {"name": "none"}},
        "kn_view": {"fields": {"n": "none"}},
        "knr_view": {"fields": {"hid": "none"}}, "nh_view": {"fields": {"n": "none"}},
        "mm_view": {"fields": {"n": "none"}}, "pm_view": {"fields": {"paid": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let eve = &principal(store.lake(), PrincipalKind::Agent, "eve", grants);
    let doors = |t: &'static str, k: &'static str| {
        let c = c.clone();
        async move {
            let print = |e: Option<Error>| serde_json::to_string(&e).expect("an error prints");
            [
                print(c.describe_type(eve, &ty(t), Default::default()).await.err()),
                print(c.get(eve, &ty(t), get(k)).await.err()),
                print(c.find(eve, &ty(t), Find::default()).await.err()),
            ]
            .map(|e| e.replace(t, "no_such"))
        }
    };
    let mut wrong = Vec::new();
    for (t, k) in [
        ("z_view", "z1"),
        ("h_view", "a"),
        ("h_viewk", "a"),
        ("m_view", "a"),
        ("g_view", "a"),
        ("knr_view", "a:a"),
    ] {
        let (there, none) = (doors(t, k).await, doors("no_such", k).await);
        if there != none || none.iter().any(|e| e == "null") {
            wrong.push(format!("`{t}`: {there:?}"));
        }
    }
    let d = c
        .describe(eve, Default::default())
        .await
        .expect("a description")
        .data;
    let measured_views = ["mm_view", "m0_view", "pm_view", "p0_view"];
    let names: Vec<&str> = (d.types.iter().map(|t| t.name.as_str()))
        .filter(|n| n.contains("_view") && !measured_views.contains(n))
        .collect();
    if names != ["kn_view", "n_view", "nh_view"] {
        wrong.push(format!("eve is described {names:?}"));
    }
    // Described, and in its tools, by the key its records carry.
    let tools = tools(&d).expect("eve's tools");
    for t in ["kn_view", "n_view"] {
        let door = Door::Get { ty: ty(t) };
        let get = tools.iter().find(|s| s.door == door);
        let key = get.map(|s| &s.input_schema["properties"]["key"]["description"]);
        let access = d.types.iter().find(|d| d.name.as_str() == t);
        let access = access.map(|d| d.access.key.iter().map(|f| f.as_str()).collect::<Vec<_>>());
        let text = format!("The key of the `{t}` record: the value of its `name`.");
        if (&access, key) != (&Some(vec!["name"]), Some(&json!(text))) {
            wrong.push(format!(
                "`{t}` is described keyed {access:?}, its get tool {key:?}"
            ));
        }
    }
    // A shown view's definition names that key and no measure of a field
    // left out, described and listed in a redeclaration of `kn_view`.
    let shape = |key: &[FieldName], v: Option<&ViewDecl>| {
        let measures = v.map(|v| v.measures.iter().map(|m| m.name.to_string()).collect());
        (
            key.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
            measures,
        )
    };
    let mut said: Vec<(Vec<String>, Option<Vec<String>>)> = Vec::new();
    for ty in ["kn_view", "n_view"] {
        let t = d.types.iter().find(|t| t.name.as_str() == ty);
        said.extend(t.map(|t| shape(&t.access.key, t.view.as_ref())));
    }
    let mut again = view("kn_view", "name", &f("name", "text"), false);
    again["description"] = json!("again");
    let declarations = vec![serde_json::from_value(again).expect("a declaration reads")];
    let bundle = ProposedWrite::Declare { declarations };
    (c.propose(own, bundle, "why".into()).await).expect("the owner proposes `kn_view` again");
    let listed = c.proposals(eve, ProposalFilter::default()).await;
    for p in listed.expect("eve lists").data {
        let Some(ProposedWrite::Declare { declarations }) = p.write else {
            continue;
        };
        for d in declarations {
            if let Declaration::Type(t) = d {
                said.push(shape(&t.key, t.view.as_deref()));
            }
        }
    }
    // And it is answered the hash of the definition it is shown.
    let field = |f: &lakelands_api::describe::FieldDescription| json!({"name": f.name, "type": f.ty, "required": f.required});
    for t in d.types.iter().filter(|t| t.name.as_str() == "nh_view") {
        let fields: Vec<_> = t.fields.iter().map(field).collect();
        let shown = json!({"name": t.name, "key": t.access.key, "fields": fields, "view": t.view});
        let shown = serde_json::from_value(shown).expect("a description reads as a declaration");
        let rebuilt = lakelands_api::shape::schema_hash(&shown).expect("a described view parses");
        if rebuilt != t.lifecycle.schema_hash {
            wrong.push(format!(
                "`nh_view` is described {:?}, hashed {}",
                t.view, t.lifecycle.schema_hash
            ));
        }
    }
    let one = |m: &[&str]| {
        (
            vec!["name".to_owned()],
            Some(m.iter().map(|m| m.to_string()).collect()),
        )
    };
    if said != [one(&[]), one(&["n"]), one(&[])] {
        wrong.push(format!(
            "eve is shown `kn_view`, `n_view`, and `kn_view` proposed, keyed and measured {said:?}"
        ));
    }
    // `mm_view` and `pm_view` answer eve, and `wes` who may write, described,
    // at `get` and `find`, filtered on the measure, and listed, as the views
    // declared without the measures they may not know of.
    let grants = json!({"default": "write", "review": true, "types": {
        "mm_view": {"fields": {"n": "none"}}, "pm_view": {"fields": {"paid": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let wes = &principal(store.lake(), PrincipalKind::Agent, "wes", grants);
    let mut whole = std::collections::BTreeMap::new();
    for who in [eve, wes] {
        for t in ["mm_view", "m0_view", "pm_view", "p0_view"] {
            let ty = TypeName::from(t);
            let described = (c.describe_type(who, &ty, Default::default()).await).map(|d| d.data);
            // A view is never written: no reader is offered `upsert` or `retire`.
            if let Ok(d) = &described
                && (d.may.upsert || d.may.retire)
            {
                wrong.push(format!(
                    "`{t}` is offered writes to {}: {:?}",
                    who.id, d.may
                ));
            }
            let got = (c.get(who, &ty, get("a")).await).map(|g| g.data);
            let found = (c.find(who, &ty, Find::default()).await).map(|f| f.data);
            // Filtered on the measure of its pair: `m`, or `open` and `due`.
            let on = if t.starts_with('m') {
                "m > 2"
            } else {
                "open > 2 or due > 2"
            };
            let on: Find = serde_json::from_value(json!({"where": on})).expect("a find");
            let filtered = (c.find(who, &ty, on).await).map(|f| f.data);
            let said = format!("{described:?} {got:?} {found:?} {filtered:?}");
            whole.insert((who.id.to_string(), t.to_owned()), said);
        }
    }
    let again = bare.into_iter().map(|mut d| {
        d["description"] = json!("again");
        serde_json::from_value(d).expect("a declaration reads")
    });
    let bundle = ProposedWrite::Declare {
        declarations: again.collect(),
    };
    (c.propose(own, bundle, "why".into()).await).expect("the owner proposes them again");
    for who in [eve, wes] {
        for p in c
            .proposals(who, ProposalFilter::default())
            .await
            .expect("a reader lists")
            .data
        {
            let Some(ProposedWrite::Declare { declarations }) = p.write else {
                continue;
            };
            for d in declarations {
                if let Declaration::Type(t) = d
                    && let Some(w) = whole.get_mut(&(who.id.to_string(), t.name.to_string()))
                {
                    w.push_str(&serde_json::to_string(&t).expect("a declaration prints"));
                }
            }
        }
    }
    for ((who, ty), there) in &whole {
        let Some(bare) = [("mm_view", "m0_view"), ("pm_view", "p0_view")]
            .into_iter()
            .find_map(|(t, b)| (t == ty).then_some(b))
        else {
            continue;
        };
        let without = &whole[&(who.clone(), bare.to_owned())];
        if *there != without.replace(bare, ty.as_str()) {
            wrong.push(format!(
                "`{ty}` answers {who}\n{there}\nwhere `{bare}` answers\n{without}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a view keyed by a field its reader may not know of is not shown: {wrong:?}"
    );
}

/// A grant's `only` admits its holder the records whose fields hold its
/// values alone (051): another's record is not found, `find`, its total,
/// a walk and a degree over the links answer its own, a proposal on
/// another's record is listed without its write, its own records it
/// writes while another's is to its write a key that is free, it
/// proposes writes to its own records, pinned to them, and decides none
/// of another's, and `describe_type` tells it the pairs and its writes.
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
        json!({"declare": "type", "name": "slip", "fields": [
            {"name": "holder", "type": {"kind": "text"}, "initial": "alice"},
            {"name": "tag", "type": {"kind": "text"}, "authority": "shared"}]}),
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
    let his = match c.propose(bob, retire, "bob's".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => proposal,
        other => panic!("bob's retirement must be proposed; it answered {other:?}"),
    };
    let grants = json!({"default": "write", "review": true,
                        "ingest": true, "types": {"claim": {"only": {"holder": "alice"}},
                                                  "slip": {"only": {"holder": "alice"}}}});
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
        named: None,
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
    // Another's record is, to her write, a key that is free: a write that
    // would pass is not granted, one that reads a record is not found.
    let head = store.head().await.expect("the head");
    let (c1, c2, c9) = (key("c1"), key("c2"), key("c9"));
    let p = || patch(&[("amount", json!(1))]);
    let bobs = || patch(&[("holder", json!("bob"))]);
    let hers = patch(&[("holder", json!("alice"))]);
    for (what, answer, read) in [
        (
            "bob's `c2` taken",
            c.upsert(alice, &claim, &c2, hers, opts()).await,
            false,
        ),
        (
            "`upsert` of bob's `c2`",
            c.upsert(alice, &claim, &c2, p(), opts()).await,
            false,
        ),
        (
            "a claim with bob's holder",
            c.upsert(alice, &claim, &c9, bobs(), opts()).await,
            false,
        ),
        (
            "her `c1` handed to bob",
            c.upsert(alice, &claim, &c1, bobs(), opts()).await,
            false,
        ),
        (
            "`retire` of bob's `c2`",
            c.retire(alice, &claim, &c2, opts()).await,
            true,
        ),
        (
            "`unlink` of bob's `c2`",
            c.unlink(alice, &claim, &c2, &on, &t1, opts()).await,
            true,
        ),
    ] {
        let refused = matches!(
            answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        );
        if refused == read || read != matches!(answer, Err(Error::NotFound { .. })) {
            wrong.push(format!("{what}: {answer:?}"));
        }
    }
    let after = store.head().await.expect("the head");
    if after != head {
        wrong.push(format!(
            "the refused writes moved the head from {head} to {after}"
        ));
    }
    // Bob's slip `s3`, retired, is a key that is free to her: her ingest
    // there makes a slip whose holder its `initial` fills, as an upsert
    // of a new one does.
    let slip = ty("slip");
    let bobs = patch(&[("holder", json!("bob"))]);
    c.upsert(own, &slip, &key("s3"), bobs, opts())
        .await
        .expect("the owner writes bob's `s3`");
    c.retire(own, &slip, &key("s3"), opts())
        .await
        .expect("the owner retires `s3`");
    let fed = SourceRecord {
        upstream: None,
        ty: slip.clone(),
        key: key("s3"),
        fields: patch(&[("tag", json!("x"))]),
        seen_at: None,
    };
    let fed = lakelands_api::ingest_all(c.as_ref(), alice, &"feed".into(), vec![fed]).await;
    let tagged = patch(&[("tag", json!("y"))]);
    // Her own she writes, unlinks, links and retires.
    let fed = fed.map(|_| ());
    let new = c.upsert(alice, &slip, &key("s1"), tagged, opts()).await;
    for (what, answer) in [
        ("a new slip", new.map(|_| ())),
        ("a slip on bob's retired `s3`", fed),
        (
            "her `c1` changed",
            c.upsert(alice, &claim, &c1, p(), opts()).await.map(|_| ()),
        ),
        (
            "her `c1` unlinked",
            c.unlink(alice, &claim, &c1, &on, &t1, opts())
                .await
                .map(|_| ()),
        ),
        (
            "her `c1` linked",
            c.link(alice, &claim, &c1, &on, &t1, none(), opts())
                .await
                .map(|_| ()),
        ),
        (
            "her `c1` retired",
            c.retire(alice, &claim, &c1, opts()).await.map(|_| ()),
        ),
    ] {
        if let Err(e) = answer {
            wrong.push(format!("{what}: {e}"));
        }
    }
    let listed = c.proposals(alice, Default::default()).await;
    let listed = listed.expect("alice's listing").data;
    if !listed.iter().all(|p| p.write.is_none() && p.more) || listed.is_empty() {
        wrong.push(format!(
            "bob's proposal is listed to alice with its write: {listed:?}"
        ));
    }
    // Her own records she proposes to write, pinned to them: a proposal
    // applies while its record stands as proposed, and is refused once
    // it moved. Another's she proposes for, and decides, nothing.
    let amount = |k: &str, n: i64| {
        let write = json!({"write": "upsert", "type": "claim", "key": k, "patch": {"amount": n}});
        serde_json::from_value(write).expect("a write reads")
    };
    let mut decided = Vec::new();
    for n in [2, 3] {
        let proposed = c.propose(alice, amount("c3", n), "mine".into()).await;
        let Ok(Outcome::Proposed { proposal, .. }) = proposed else {
            panic!("alice's proposal on her `c3` must be proposed; it answered {proposed:?}");
        };
        if n == 3 {
            c.upsert(own, &claim, &key("c3"), p(), opts())
                .await
                .expect("the owner writes `c3`");
        }
        decided.push(c.accept(own, &proposal).await.map(|_| ()));
    }
    let moved = decided[1]
        .as_ref()
        .err()
        .and_then(Error::case)
        .map(|s| &s.case);
    let moved =
        matches!(moved, Some(Case::WriteChangedSinceProposed { key: Some(k), .. }) if k == "c3");
    if decided[0].is_err() || !moved {
        wrong.push(format!(
            "the owner's accepts of alice's proposals: {decided:?}"
        ));
    }
    let give = json!({"declare": "verb", "name": "give", "on": "claim", "risk": "high",
                      "effect": "write", "set": [{"field": "holder", "value": "'bob'"}]});
    let give = serde_json::from_value(give).expect("a verb reads");
    c.declare(own, give)
        .await
        .expect("the owner declares `give`");
    let (verb, c3) = ("give".into(), key("c3"));
    let given = c.call(bob, &verb, Some(&c3), none(), opts()).await;
    let Ok(Outcome::Proposed {
        proposal: given, ..
    }) = given
    else {
        panic!("bob's call of `give` must be proposed");
    };
    let head = store.head().await.expect("the head");
    for (what, answer) in [
        (
            "an accept of a call that hands her `c3` to bob",
            c.accept(alice, &given).await.map(|_| ()),
        ),
        (
            "her call of a verb that assigns her holder",
            c.call(alice, &verb, Some(&c3), none(), opts())
                .await
                .map(|_| ()),
        ),
        (
            "a proposal on bob's `c2`",
            c.propose(alice, amount("c2", 2), "mine".into())
                .await
                .map(|_| ()),
        ),
        (
            "an accept of bob's",
            c.accept(alice, &his).await.map(|_| ()),
        ),
        (
            "a reject of bob's",
            c.reject(alice, &his, "no".into()).await.map(|_| ()),
        ),
    ] {
        if !matches!(
            answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        ) {
            wrong.push(format!("{what}: {answer:?}"));
        }
    }
    if store.head().await.expect("the head") != head {
        wrong.push("a refused proposal or decision by alice moved the head".to_owned());
    }
    let told = c.describe_type(alice, &claim, ReadAt::default()).await;
    let told = told.expect("`claim` is described to alice").data;
    if !told.verbs.is_empty() {
        wrong.push(format!("`give` is described to alice: {:?}", told.verbs));
    }
    let may = told.may;
    let only = serde_json::to_value(&may.only).expect("pairs read");
    let link: Vec<&str> = may.link.iter().map(|r| r.as_str()).collect();
    if only != json!({"holder": "alice"}) || !may.upsert || !may.retire || link != ["on_trip"] {
        wrong.push(format!("`describe_type` says {may:?}"));
    }
    assert!(
        wrong.is_empty(),
        "alice is admitted her own claims alone: {}",
        wrong.join("; ")
    );
}

/// A grant whose `only` cannot be applied admits no record, whatever a
/// stored value holds (051, lakelands#771): a reviewer is listed every
/// proposal on the type without its write, a reference into the type is
/// refused whole as one to an absent key, and a write on the record whose
/// value equals the pair answers as on another's.
pub async fn a_grant_that_cannot_be_applied_admits_no_record(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let opts = WriteOpts::default;
    for d in [
        json!({"declare": "type", "name": "trip", "fields": [
            {"name": "title", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "claim", "fields": [
            {"name": "holder", "type": {"kind": "text"}, "authority": "shared"},
            {"name": "amount", "type": {"kind": "number"}, "authority": "shared"},
            {"name": "note", "type": {"kind": "text"}, "authority": "shared"}],
            "relations": [{"name": "on_trip", "target": "trip"}]}),
        json!({"declare": "type", "name": "memo", "fields": [
            {"name": "claim", "type": {"kind": "reference", "target": "claim"}}]}),
        json!({"declare": "verb", "name": "pay", "on": "claim", "risk": "low",
               "effect": "write", "set": [{"field": "note", "value": "'paid'"}]}),
    ] {
        let d = serde_json::from_value::<Declaration>(d).expect("a declaration reads");
        c.declare(own, d).await.expect("the owner declares");
    }
    for (k, holder, amount) in [("c1", "alice", 10), ("c2", "bob", 20)] {
        let p = patch(&[("holder", json!(holder)), ("amount", json!(amount))]);
        c.upsert(own, &ty("claim"), &key(k), p, opts())
            .await
            .expect("the owner writes a claim");
        c.upsert(
            own,
            &ty("trip"),
            &key("t1"),
            patch(&[("title", json!("rome"))]),
            opts(),
        )
        .await
        .expect("the owner writes `t1`");
        c.link(
            own,
            &ty("claim"),
            &key(k),
            &"on_trip".into(),
            &key("t1"),
            Patch::new(),
            opts(),
        )
        .await
        .expect("the owner links a claim");
        let w = json!({"write": "upsert", "type": "claim", "key": k, "patch": {"amount": 1}});
        let w = serde_json::from_value(w).expect("a write reads");
        c.propose(own, w, "the owner's".into())
            .await
            .expect("the owner proposes");
    }
    let mut all = c
        .proposals(own, Default::default())
        .await
        .expect("a listing")
        .data;
    for p in &mut all {
        (p.write, p.more) = (None, true);
    }
    let mut wrong = Vec::new();
    for grant in [
        json!({"only": {"amount": 10}}),
        json!({"only": {"holder": "alice"}, "fields": {"holder": "masked"}}),
        json!({"only": {"holder": "alice"}, "fields": {"holder": "none"}}),
    ] {
        let grants = json!({"default": "write", "review": true, "ingest": true,
                            "types": {"claim": grant}});
        let grants = serde_json::from_value(grants).expect("grants read");
        let eve = &principal(store.lake(), PrincipalKind::Agent, "eve", grants);
        let listed = c.proposals(eve, Default::default()).await.map(|r| r.data);
        if listed.as_ref().ok() != Some(&all) {
            wrong.push(format!("{grant}: the listing {listed:?}"));
        }
        let refused = |to: &'static str| {
            let p = patch(&[("claim", json!(to))]);
            let c = c.clone();
            async move {
                let e = c
                    .upsert(eve, &ty("memo"), &key("m1"), p, opts())
                    .await
                    .err();
                serde_json::to_string(&e)
                    .expect("an error prints")
                    .replace(to, "c?")
            }
        };
        let (held, absent) = (refused("c1").await, refused("c9").await);
        if held != absent || absent == "null" {
            wrong.push(format!("{grant}: `c1` {held}, the absent `c9` {absent}"));
        }
        // Her writes on `c1` answer as on bob's `c2`: she owns neither.
        let writes = |k: &'static str| {
            let c = c.clone();
            async move {
                let print = |r: Result<(), Error>| match r {
                    Ok(()) => "applied".to_owned(),
                    Err(e) => serde_json::to_string(&e).expect("an error prints"),
                };
                let (claim, on, t1) = (ty("claim"), "on_trip".into(), key("t1"));
                let note = || patch(&[("note", json!("x"))]);
                let up =
                    json!({"write": "upsert", "type": "claim", "key": k, "patch": {"note": "x"}});
                let gone = json!({"write": "retire", "type": "claim", "key": k});
                let fed: SourceRecord = serde_json::from_value(
                    json!({"type": "claim", "key": k, "fields": {"note": "x"}}),
                )
                .expect("a record reads");
                let fed = match c.ingest(eve, &"feed".into(), stream::of(vec![fed])).await {
                    Ok(mut s) => format!("{:?}", stream::collect(&mut s).await),
                    Err(e) => print(Err(e)),
                };
                let de = |w: serde_json::Value| serde_json::from_value(w).expect("a write reads");
                [
                    print(
                        c.upsert(eve, &claim, &key(k), note(), opts())
                            .await
                            .map(drop),
                    ),
                    print(
                        c.call(eve, &"pay".into(), Some(&key(k)), Patch::new(), opts())
                            .await
                            .map(drop),
                    ),
                    print(
                        c.unlink(eve, &claim, &key(k), &on, &t1, opts())
                            .await
                            .map(drop),
                    ),
                    print(c.propose(eve, de(up), "eve's".into()).await.map(drop)),
                    print(c.propose(eve, de(gone), "eve's".into()).await.map(drop)),
                    print(c.retire(eve, &claim, &key(k), opts()).await.map(drop)),
                    fed,
                ]
                .map(|a| a.replace(k, "c?"))
            }
        };
        let (held, other) = (writes("c1").await, writes("c2").await);
        if held != other {
            wrong.push(format!(
                "{grant}: writes on `c1` {held:?}, on `c2` {other:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an `only` that cannot be applied admits no record: {wrong:?}"
    );
}

/// An author held at `none` on a reference field learns nothing of its
/// target by declaring the type again with that field: the target in
/// force, another type it is not shown and no type are answered alike,
/// never as a field declared twice (lakelands#716).
pub async fn a_hidden_target_is_not_learned_by_declaring_its_field(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let typed = |name: &str, fields: Value| {
        let d = json!({"declare": "type", "name": name, "fields": fields});
        serde_json::from_value::<Declaration>(d).expect("a declaration reads")
    };
    let code = json!([{"name": "code", "type": {"kind": "text"}}]);
    let loose = |target: &str| {
        let payee = json!({"kind": "reference", "target": target});
        typed(
            "loose",
            json!([{"name": "note", "type": {"kind": "text"}},
                              {"name": "payee", "type": payee}]),
        )
    };
    for d in [
        typed("payroll", code.clone()),
        typed("ledger", code),
        loose("payroll"),
    ] {
        c.declare(own, d).await.expect("the owner declares");
    }
    let grants = json!({"default": "write", "schema": true, "types": {
        "payroll": {"default": "none"}, "ledger": {"default": "none"},
        "loose": {"fields": {"payee": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let author = &principal(store.lake(), PrincipalKind::Agent, "author", grants);
    let mut said = Vec::new();
    for target in ["payroll", "ledger", "ghost"] {
        let answer = c.declare(author, loose(target)).await;
        said.push(
            answer
                .map_or_else(|e| format!("{e:?}"), |_| "taken".into())
                .replace(target, "T"),
        );
    }
    assert!(
        said.iter()
            .all(|s| s == &said[2] && !s.contains("DeclaredTwice")),
        "an author held from `payee` is answered alike for each target: {said:?}"
    );
}

/// A migration's renames are judged against its author's view of the
/// type, at the step that judges the lake's own, as the store does: a
/// hidden field or relation renamed answers word for word as an absent one,
/// alone and beside a fault judged before or after it; a shape naming a
/// hidden field answers as one naming a field twice, and one also removing
/// a field a hidden relation reads is refused on that field first
/// (lakelands#750).
pub async fn a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let typed = |d: Value| serde_json::from_value::<Declaration>(d).expect("a declaration reads");
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let texts = |names: &[&str]| names.iter().map(|n| text(n)).collect::<Vec<_>>();
    // `paid` and `matched`, which reads `ref`, go to the hidden `payroll`.
    let matched = json!({"name": "matched", "target": "payroll",
        "derived": {"match": [{"from": "ref", "to": "code"}]}});
    let invoice = json!({"declare": "type", "name": "invoice",
        "fields": texts(&["no", "gross", "salary", "ref"]),
        "relations": [{"name": "paid", "target": "payroll"}, matched]});
    let payroll = json!({"declare": "type", "name": "payroll", "fields": texts(&["code"])});
    for d in [payroll, invoice] {
        c.declare(own, typed(d)).await.expect("the owner declares");
    }
    let grants = json!({"default": "write", "schema": true, "types": {
        "invoice": {"fields": {"salary": "none"}}, "payroll": {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let author = &principal(store.lake(), PrincipalKind::Agent, "author", grants);
    let m = |fields: &[&str], renames: &[(&str, &str)], relations: &[(&str, &str)]| {
        let fields = texts(fields);
        let news: Vec<Value> = (relations.iter())
            .map(|(_, new)| json!({"name": new, "target": "invoice"}))
            .collect();
        let Declaration::Type(to) = typed(json!({"declare": "type", "name": "invoice",
            "fields": fields, "relations": news}))
        else {
            unreachable!("a type")
        };
        Migration {
            to,
            renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
            relation_renames: (relations.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
        }
    };
    // `numbered`, shown to the author, reads `no`, which nothing hidden
    // reads: taking `no` away is a fault judged after the shape is read
    // and before any rename.
    let numbered = json!({"declare": "object_set", "name": "numbered", "of": "invoice",
        "where": "no = 'x'"});
    c.declare(own, typed(numbered))
        .await
        .expect("the owner declares `numbered`");
    // Each row: the migration naming the hidden name, the one naming an
    // absent name in its place, and the two names; ("", "") where the
    // answer names neither.
    let rows = [
        (
            "renaming the hidden `salary`",
            m(&["no", "gross", "ref", "pay"], &[("salary", "pay")], &[]),
            m(&["no", "gross", "ref", "pay"], &[("ghost", "pay")], &[]),
            ("ghost", "salary"),
        ),
        (
            "renaming the hidden `paid`",
            m(&["no", "gross", "ref"], &[], &[("paid", "settled")]),
            m(&["no", "gross", "ref"], &[], &[("nope", "settled")]),
            ("nope", "paid"),
        ),
        (
            "naming the hidden `salary`, as `memo` named twice",
            m(&["no", "gross", "ref", "salary"], &[], &[]),
            m(&["no", "gross", "ref", "memo", "memo"], &[], &[]),
            ("memo", "salary"),
        ),
        (
            "renaming the hidden `paid` and the absent `ghostf`",
            m(
                &["no", "gross", "ref", "x"],
                &[("ghostf", "x")],
                &[("paid", "settled")],
            ),
            m(
                &["no", "gross", "ref", "x"],
                &[("ghostf", "x")],
                &[("nope", "settled")],
            ),
            ("", ""),
        ),
        (
            "renaming `no` to no field, then the hidden `salary`",
            m(
                &["no", "gross", "ref", "x"],
                &[("no", "nosuchto"), ("salary", "x")],
                &[],
            ),
            m(
                &["no", "gross", "ref", "x"],
                &[("no", "nosuchto"), ("ghost", "x")],
                &[],
            ),
            ("", ""),
        ),
        (
            "renaming the hidden `salary` and taking away `no`, which `numbered` reads",
            m(&["gross", "ref", "pay"], &[("salary", "pay")], &[]),
            m(&["gross", "ref", "pay"], &[("ghost", "pay")], &[]),
            ("", ""),
        ),
    ];
    // Each by `migrate`, by `migrate_apply` and by `propose`, which judge
    // the migration in one path.
    let send = |door: &'static str, m: Migration| {
        let c = c.clone();
        async move {
            match door {
                "migrate" => c.migrate(author, m).await.map(drop),
                "apply" => c.migrate_apply(author, m, "none").await.map(drop),
                _ => {
                    let write = ProposedWrite::Migrate {
                        migration: Box::new(m),
                        report: Default::default(),
                    };
                    c.propose(author, write, "a migration".into())
                        .await
                        .map(drop)
                }
            }
        }
    };
    for (what, hidden, absent, (from, to)) in rows {
        for door in ["migrate", "apply", "propose"] {
            let wire = |r: lakelands_api::Result<()>| match r {
                Err(e) => serde_json::to_string(&e).expect("an error prints"),
                Ok(()) => panic!("a migration {what} is refused by `{door}`; it was taken"),
            };
            let hidden = wire(send(door, hidden.clone()).await);
            let absent = wire(send(door, absent.clone()).await);
            let absent = if from.is_empty() {
                absent
            } else {
                absent.replace(from, to)
            };
            assert_eq!(
                hidden, absent,
                "a migration {what}, by `{door}`, is answered as its absent twin"
            );
        }
    }
    // `salary` named again, and `ref` removed, which the hidden `matched`
    // reads: the read is judged first, on the field.
    let answer = c
        .migrate(author, m(&["no", "gross", "salary"], &[], &[]))
        .await;
    let read = |e: &Error| matches!(e.case(), Some(Said { case: Case::FieldReadElsewhere { field, .. }, .. }) if field.as_str() == "ref");
    assert!(
        answer.as_ref().is_err_and(read),
        "a migration naming the hidden `salary` and removing `ref` is refused as `ref` read \
         elsewhere: {answer:?}"
    );
}

/// An author held at `none` on `veiled` and `cloak` names them in a valid
/// time, a key or a new relation's `via`, by `declare` and by `migrate`, and
/// is answered word for word as for a name no field has, with the type in
/// force unmoved. What stands as declared is taken, a relation it renames
/// keeps its `via`, and one it is not shown renames as no relation
/// (lakelands#726).
pub async fn a_shape_names_no_field_its_author_may_not_know_of(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let typed = |d: Value| serde_json::from_value::<Declaration>(d).expect("a declaration reads");
    let kind = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let to = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    let goes = |name: &str, via: &str| json!({"name": name, "target": "other", "via": via});
    // `goes`, carried by the hidden `cloak`, is not shown to the author,
    // nor `pays`, carried by `payee`, which refers to the hidden `payroll`
    // and so is hidden too (lakelands#718).
    let ev = |more: &[Value], relations: Vec<Value>, shape: Value| {
        let mut fields = vec![kind("no", "text"), kind("from", "date_time")];
        fields.extend(more.iter().cloned());
        let mut d = json!({"declare": "type", "name": "ev", "fields": fields,
            "relations": relations});
        let shape = shape.as_object().expect("an object").clone();
        d.as_object_mut().expect("an object").extend(shape);
        typed(d)
    };
    let stamped = |more: Value| {
        typed(
            json!({"declare": "type", "name": "stamped", "valid_from": "veiled",
            "valid_to": "cloak", "fields": [kind("no", "text"), more]}),
        )
    };
    let other =
        |name: &str| typed(json!({"declare": "type", "name": name, "fields": [kind("x", "text")]}));
    let hidden = [
        kind("veiled", "date_time"),
        to("cloak", "other"),
        to("payee", "payroll"),
    ];
    let mut timed = stamped(kind("veiled", "date_time"));
    if let Declaration::Type(t) = &mut timed {
        t.fields
            .push(serde_json::from_value(kind("cloak", "date_time")).expect("a field reads"));
    }
    let whole = ev(
        &hidden,
        vec![goes("goes", "cloak"), goes("pays", "payee")],
        json!({}),
    );
    // `plain` is `stamped` with no valid time.
    let plain = typed(
        json!({"declare": "type", "name": "plain", "fields": [kind("no", "text"),
        kind("veiled", "date_time"), kind("cloak", "date_time")]}),
    );
    for d in [other("other"), other("payroll"), whole, timed, plain] {
        c.declare(own, d).await.expect("the owner declares");
    }
    let later =
        json!({"no": "1", "veiled": "2030-01-01T00:00:00Z", "cloak": "2031-01-01T00:00:00Z"});
    let later: BTreeMap<FieldName, Value> = serde_json::from_value(later).expect("fields read");
    for ty in ["stamped", "plain"] {
        let (ty, s1) = (TypeName::from(ty), RecordKey::from("s1"));
        let wrote = c.upsert(own, &ty, &s1, later.clone(), WriteOpts::default());
        wrote.await.expect("the owner writes `s1`");
    }
    let held = json!({"fields": {"veiled": "none", "cloak": "none"}});
    let grants = json!({"default": "write", "schema": true, "types": {
        "ev": held, "stamped": held, "plain": held, "payroll": {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let author = &principal(store.lake(), PrincipalKind::Agent, "author", grants);
    let seen = |shape: Value, more: Vec<Value>| ev(&[], more, shape);
    // As the store: a valid time over a field the author may not know of
    // is not named to it, and a read at a valid time answers as for a
    // type with none.
    let at = lakelands_api::Timestamp::parse("2026-06-01T00:00:00Z").expect("an instant");
    let mut read = Vec::new();
    for ty in ["stamped", "plain"] {
        let find = Find {
            at: ReadAt {
                valid_at: Some(at),
                ..Default::default()
            },
            ..Default::default()
        };
        let found = c.find(author, &ty.into(), find).await.map(|r| r.data);
        read.push(format!("{found:?}").replace(ty, "T"));
    }
    let shown = c
        .describe_type(author, &"stamped".into(), Default::default())
        .await;
    let shown = shown.expect("the author describes `stamped`").data.access;
    assert!(
        (shown.valid_from, shown.valid_to) == (None, None) && read[0] == read[1],
        "`stamped` must be described with no valid time and read at one as `plain`: {read:?}"
    );
    // As the store: with one bound over a field the reader may not know
    // of, only that bound is dropped. `half` reads as `halfplain`, valid
    // from `from` alone, and `mhalf`, from the masked `veiled`, as `mplain`.
    let fields = json!([
        kind("no", "text"),
        kind("from", "date_time"),
        kind("veiled", "date_time"),
        kind("cloak", "date_time")
    ]);
    let late = json!({"no": "1", "from": "2030-01-01T00:00:00Z", "veiled": "2030-01-01T00:00:00Z"});
    let late: BTreeMap<FieldName, Value> = serde_json::from_value(late).expect("fields read");
    let halves = [
        ("half", "from", Some("cloak")),
        ("halfplain", "from", None),
        ("mhalf", "veiled", Some("cloak")),
        ("mplain", "veiled", None),
    ];
    for (name, from, to) in halves {
        let d = typed(json!({"declare": "type", "name": name, "fields": fields,
            "valid_from": from, "valid_to": to}));
        c.declare(own, d).await.expect("the owner declares");
        let (ty, h1) = (TypeName::from(name), RecordKey::from("h1"));
        let wrote = c.upsert(own, &ty, &h1, late.clone(), WriteOpts::default());
        wrote.await.expect("the owner writes `h1`");
    }
    let held = json!({"fields": {"veiled": "masked", "cloak": "none"}});
    let grants = json!({"default": "write", "types": {
        "half": held, "halfplain": held, "mhalf": held, "mplain": held}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let reader = &principal(store.lake(), PrincipalKind::Agent, "reader", grants);
    let at = ReadAt {
        valid_at: Some(at),
        ..Default::default()
    };
    let mut apart = Vec::new();
    for (a, b) in [("half", "halfplain"), ("mhalf", "mplain")] {
        let mut said = Vec::new();
        for ty in [a, b] {
            let find = Find {
                at,
                ..Default::default()
            };
            let found = c.find(reader, &ty.into(), find).await.map(|r| r.data);
            let get = Get {
                key: "h1".into(),
                with_meta: false,
                at,
                params: Default::default(),
            };
            let got = c.get(reader, &ty.into(), get).await.map(|r| r.data);
            said.push(format!("{found:?} {got:?}").replace(ty, "T"));
        }
        if said[0] != said[1] {
            apart.push(format!("`{a}` | `{b}`: {said:?}"));
        }
    }
    assert!(
        apart.is_empty(),
        "one bound hidden reads as the other alone: {apart:?}"
    );
    let described = match stamped(kind("memo", "text")) {
        Declaration::Type(t) => Declaration::Type(TypeDecl {
            valid_from: None,
            valid_to: None,
            ..t
        }),
        _ => unreachable!("a type"),
    };
    // As the store: restated, a valid time answers as on `plain` (below).
    for d in [seen(json!({}), Vec::new()), described] {
        c.declare(author, d)
            .await
            .expect("declaring again what the author is shown");
    }
    let migration = |d, relation_renames| match d {
        Declaration::Type(to) => Migration {
            to,
            renames: Vec::new(),
            relation_renames,
        },
        _ => unreachable!("a type"),
    };
    let mut apart = Vec::new();
    for member in ["valid_from", "valid_to", "key", "via"] {
        for door in ["declare", "migrate"] {
            let mut said = Vec::new();
            for name in [if member == "via" { "cloak" } else { "veiled" }, "nowhere"] {
                let d = match member {
                    "valid_from" => seen(json!({"valid_from": name}), Vec::new()),
                    "valid_to" => seen(json!({"valid_from": "from", "valid_to": name}), Vec::new()),
                    "key" => seen(json!({"key": [name]}), Vec::new()),
                    _ => seen(json!({}), vec![goes("runs", name)]),
                };
                let answer = match door {
                    "declare" => c.declare(author, d).await.map(drop),
                    _ => c.migrate(author, migration(d, Vec::new())).await.map(drop),
                };
                let answer = answer.map_or_else(|e| e.to_string(), |()| "taken".into());
                said.push(answer.replace(name, "N"));
            }
            if said[0] != said[1] {
                apart.push(format!("{member} by `{door}`: {} | {}", said[0], said[1]));
            }
        }
    }
    // A rename from `goes` or `pays`, which the author is not shown,
    // answers as one from no relation.
    let mut said = Vec::new();
    for from in ["goes", "pays", "nothing"] {
        let m = migration(
            seen(json!({}), vec![goes("went", "cloak")]),
            vec![(from.into(), "went".into())],
        );
        let answer = c.migrate(author, m).await.map(drop);
        said.push(
            answer
                .map_or_else(|e| e.to_string(), |()| "taken".into())
                .replace(from, "N"),
        );
    }
    if said[0] != said[2] || said[1] != said[2] {
        apart.push(format!("a rename: {said:?}"));
    }
    // As the store: a valid time sent where one over a field the author may
    // not know of stands is judged by its name first, as on `plain`, which
    // has none; only a field the author may name is refused, as the bound
    // taken (lakelands#692).
    let send = |d: Declaration, door: &'static str| {
        let c = c.clone();
        async move {
            match door {
                "declare" => c.declare(author, d).await.map(drop),
                _ => c.migrate(author, migration(d, Vec::new())).await.map(drop),
            }
        }
    };
    let shape = |name: &str, axis: &str, to: Option<&str>| {
        let mut d = json!({"declare": "type", "name": name,
            "fields": [kind("no", "text"), kind("memo", "text"), kind("d2", "date_time")]});
        if let Some(to) = to {
            d[axis] = json!(to);
        }
        typed(d)
    };
    let wire = |r: &lakelands_api::Result<()>| match r {
        Err(e) => serde_json::to_string(e).expect("an error prints"),
        Ok(()) => "taken".into(),
    };
    // Each axis: another hidden field, and the bound's own, restated.
    for (axis, hidden, own) in [
        ("valid_from", "cloak", "veiled"),
        ("valid_to", "veiled", "cloak"),
    ] {
        for door in ["declare", "migrate"] {
            for to in ["zz", hidden, own] {
                let on_stamped = wire(&send(shape("stamped", axis, Some(to)), door).await);
                let on_plain = wire(&send(shape("plain", axis, Some(to)), door).await);
                assert!(
                    on_stamped != "taken" && on_stamped == on_plain.replace("plain", "stamped"),
                    "`{axis}: {to}` sent by `{door}` on `stamped` is refused as on `plain`: \
                     {on_stamped} | {on_plain}"
                );
            }
        }
        let left = wire(&send(shape("stamped", axis, None), "migrate").await);
        let plain = wire(&send(shape("plain", axis, None), "migrate").await);
        assert!(
            left == "taken" && plain == "taken",
            "`{axis}` left out is taken on `stamped` as on `plain`: {left} | {plain}"
        );
        for door in ["declare", "migrate"] {
            let answer = wire(&send(shape("stamped", axis, Some("d2")), door).await);
            assert!(
                answer.contains("\"code\":\"valid_time_stands\""),
                "`{axis}: d2` sent by `{door}` on `stamped` is refused as the bound taken: {answer}"
            );
        }
        let plain = wire(&send(shape("plain", axis, Some("d2")), "migrate").await);
        assert_eq!(plain, "taken", "`{axis}: d2` on `plain` is taken");
    }
    // As the store: a field of its own named like a hidden one, sent as a
    // valid time, is a field declared twice, as on `plain`.
    for axis in ["valid_from", "valid_to"] {
        for door in ["declare", "migrate"] {
            let mut said = Vec::new();
            for name in ["stamped", "plain"] {
                let mut d = json!({"declare": "type", "name": name, "fields": [kind("no", "text"),
                    kind("memo", "text"), kind("d2", "date_time"), kind("veiled", "date_time")]});
                d[axis] = json!("veiled");
                said.push(wire(&send(typed(d), door).await));
            }
            assert!(
                said[0] != "taken" && said[0] == said[1].replace("plain", "stamped"),
                "its own `veiled` as `{axis}` by `{door}` on `stamped` is refused as on `plain`: \
                 {said:?}"
            );
        }
    }
    // As the store: with one bound hidden, its restatement answers as on the
    // type with the other bound alone.
    let held = json!({"fields": {"veiled": "masked", "cloak": "none"}});
    let grants = json!({"default": "write", "schema": true, "types": {
        "half": held, "halfplain": held}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let halfer = &principal(store.lake(), PrincipalKind::Agent, "halfer", grants);
    for door in ["declare", "migrate"] {
        let mut said = Vec::new();
        for name in ["half", "halfplain"] {
            let d = typed(json!({"declare": "type", "name": name, "fields": [
                kind("no", "text"), kind("from", "date_time"), kind("veiled", "date_time"),
                kind("cloak", "date_time")], "valid_from": "from", "valid_to": "cloak"}));
            let answer = match door {
                "declare" => c.declare(halfer, d).await.map(drop),
                _ => c.migrate(halfer, migration(d, Vec::new())).await.map(drop),
            };
            said.push(wire(&answer));
        }
        assert!(
            said[0] != "taken" && said[0] == said[1].replace("halfplain", "half"),
            "`half`'s `valid_to: cloak` restated by `{door}` is refused as on `halfplain`: {said:?}"
        );
    }
    let described = |ty: &'static str| {
        let c = c.clone();
        async move {
            let d = c.describe_type(own, &ty.into(), Default::default()).await;
            d.expect("the owner describes it").data
        }
    };

    let (after, timed) = (described("ev").await, described("stamped").await);
    let times =
        |a: &lakelands_api::describe::TypeAccess| (a.valid_from.clone(), a.valid_to.clone());
    assert!(
        apart.is_empty()
            && after.access.key.is_empty()
            && times(&after.access) == (None, None)
            && after.relations.len() == 2
            && times(&timed.access) == (Some("veiled".into()), Some("cloak".into())),
        "a hidden name must answer as `nowhere`, with `ev` and `stamped` as declared; apart \
         (hidden | none): {apart:?}; `ev` holds {:?} and {:?}, `stamped` {:?}",
        after.access,
        after.relations,
        timed.access
    );
    // A relation named `pays`, which the author is not shown, sent to the
    // hidden `payroll` answers as sent to `zzt`, which is no type.
    let mut said = Vec::new();
    for target in ["payroll", "zzt"] {
        let d = seen(json!({}), vec![json!({"name": "pays", "target": target})]);
        let answer = c.declare(author, d).await.map(drop);
        said.push(
            answer
                .map_or_else(|e| e.to_string(), |()| "taken".into())
                .replace(target, "N"),
        );
    }
    assert!(
        said[0] == said[1],
        "`pays` sent to `payroll` must answer as to `zzt`: {said:?}"
    );
}

/// As the store (lakelands#718): a reference to a type its reader is not
/// shown is a field it may not know of, read or masked alike, and a type
/// keyed by it, or by a reference to such a type, answers as one that is
/// not there; a reference the reader's own `only` names stays, and a
/// reader shown the target by its type-level grant alone keeps it all.
pub async fn a_reference_to_a_type_its_reader_is_not_shown_is_a_field_it_may_not_know_of(
    store: Arc<dyn LakeStore>,
) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let typed = |d: Value| serde_json::from_value::<Declaration>(d).expect("a declaration reads");
    let kind = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let to = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    for d in [
        json!({"declare": "type", "name": "payroll", "fields": [kind("code", "text")]}),
        json!({"declare": "type", "name": "ticket", "fields": [kind("no", "text"), to("payee", "payroll")]}),
        json!({"declare": "type", "name": "slip", "key": ["payee"], "fields": [to("payee", "payroll")]}),
        json!({"declare": "type", "name": "stub", "key": ["of"], "fields": [to("of", "slip")]}),
    ] {
        c.declare(own, typed(d)).await.expect("the owner declares");
    }
    let wrote = |ty: &str, key: &str, fields: Value| {
        let (c, ty, key) = (c.clone(), TypeName::from(ty), RecordKey::from(key));
        let fields: BTreeMap<FieldName, Value> =
            serde_json::from_value(fields).expect("fields read");
        async move {
            c.upsert(own, &ty, &key, fields, WriteOpts::default())
                .await
                .expect("the owner writes")
        }
    };
    wrote("payroll", "p1", json!({"code": "c"})).await;
    wrote("ticket", "t1", json!({"no": "1", "payee": "p1"})).await;
    wrote("slip", "p1", json!({"payee": "p1"})).await;
    let reader = |id: &str, types: Value| {
        let grants = json!({"default": "write", "types": types});
        principal(
            store.lake(),
            PrincipalKind::Agent,
            id,
            serde_json::from_value(grants).expect("grants read"),
        )
    };
    let none = json!({"default": "none"});
    let kept = &reader("kept", json!({"payroll": none}));
    let masked = &reader(
        "masked",
        json!({"payroll": none, "ticket": {"fields": {"payee": "masked"}}}),
    );
    let held = &reader(
        "held",
        json!({"payroll": none, "ticket": {"only": {"payee": "p1"}}}),
    );
    let typed_only = &reader(
        "typed",
        json!({"payroll": {"default": "read", "fields": {"code": "none"}}}),
    );
    let ticket = TypeName::from("ticket");
    let mut apart = Vec::new();
    for (who, payee) in [
        (kept, false),
        (masked, false),
        (held, true),
        (typed_only, true),
    ] {
        let d = c
            .describe_type(who, &ticket, Default::default())
            .await
            .expect("described")
            .data;
        let has = d.fields.iter().any(|f| f.name.as_str() == "payee");
        let found = format!(
            "{:?}",
            c.find(who, &ticket, Find::default()).await.map(|r| r.data)
        );
        if has != payee || found.contains("payee") != payee {
            apart.push(format!(
                "`{}` describes `payee`: {has}, and finds {found}",
                who.id
            ));
        }
    }
    // `payee` answers as `zz`, and `slip` and `stub` as `ghost`.
    let mut said = Vec::new();
    for name in ["payee", "zz"] {
        let find: Find =
            serde_json::from_value(json!({"where": format!("{name} = 'p1'")})).expect("a find");
        let found = c.find(kept, &ticket, find).await.map(drop);
        let patch: BTreeMap<FieldName, Value> =
            serde_json::from_value(json!({name: "p1"})).expect("a patch");
        let up = c
            .upsert(kept, &ticket, &"t2".into(), patch, WriteOpts::default())
            .await
            .map(drop);
        said.push(format!("{found:?} {up:?}").replace(name, "N"));
    }
    if said[0] != said[1] {
        apart.push(format!("`payee` | `zz`: {said:?}"));
    }
    for ty in ["slip", "stub"] {
        let mut answers = Vec::new();
        for name in [ty, "ghost"] {
            let t = TypeName::from(name);
            let d = c
                .describe_type(kept, &t, Default::default())
                .await
                .map(drop);
            let get = Get {
                key: "p1".into(),
                with_meta: false,
                at: Default::default(),
                params: Default::default(),
            };
            let g = c.get(kept, &t, get).await.map(drop);
            answers.push(format!("{d:?} {g:?}").replace(name, "T"));
        }
        if answers[0] != answers[1] {
            apart.push(format!("`{ty}` | `ghost`: {answers:?}"));
        }
    }
    // A bundle declaring a type that refers to `payroll` is not there to a
    // reviewer kept from it: it adds a reference to a hidden type
    // (lakelands#762). A migration of `ticket` restates `payee` and is
    // listed without it.
    let pay = typed(json!({"declare": "type", "name": "pay",
        "fields": [kind("no", "text"), to("payee", "payroll")]}));
    let Declaration::Type(to_ticket) = typed(json!({"declare": "type", "name": "ticket",
        "fields": [kind("no", "text"), to("payee", "payroll"), kind("memo", "text")]}))
    else {
        unreachable!("a type")
    };
    let migration = Migration {
        to: to_ticket,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    for (write, why) in [
        (
            ProposedWrite::Declare {
                declarations: vec![pay],
            },
            "a bundle",
        ),
        (
            ProposedWrite::Migrate {
                migration: Box::new(migration),
                report: Default::default(),
            },
            "a migration",
        ),
    ] {
        c.propose(own, write, why.into())
            .await
            .expect("the owner proposes");
    }
    let mut reviewer = kept.clone();
    reviewer.grants.review = true;
    let listed = c.proposals(&reviewer, ProposalFilter::default()).await;
    let listed = listed.expect("the reviewer lists").data;
    let whys: Vec<&str> = listed.iter().map(|p| p.why.as_str()).collect();
    let shown = format!("{listed:?}");
    if whys.contains(&"a bundle") || !whys.contains(&"a migration") || shown.contains("payee") {
        apart.push(format!("the reviewer lists {shown}"));
    }
    // To retire a record removes every field, `payee` too.
    let retired = c
        .retire(kept, &ticket, &"t1".into(), WriteOpts::default())
        .await;
    let may = c.describe_type(kept, &ticket, Default::default()).await;
    let may = may.expect("described").data.may.retire;
    if !matches!(
        &retired,
        Err(Error::Refused {
            reason: Refusal::NotGranted,
            ..
        })
    ) || may
    {
        apart.push(format!(
            "`kept` retiring `t1`: {retired:?}, `may.retire` {may}"
        ));
    }
    assert!(
        apart.is_empty(),
        "a reference to a type not shown: {apart:?}"
    );
}

/// A valid time names a `date_time` or a `date` field: one of any other
/// kind is refused as invalid on it, by `declare`, a migration, `apply`
/// and a proposal, in the store's words (lakelands#714, lakelands#702).
pub async fn a_valid_time_names_a_date_time_field(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let kind = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let stamped = |at: Value, valid: (&str, &str)| {
        let d = json!({"declare": "type", "name": "stamped", "fields": [kind("from", "date_time"), at],
            "valid_from": valid.0, "valid_to": valid.1});
        serde_json::from_value::<Declaration>(d).expect("a declaration reads")
    };
    let refused = |answer: &Result<(), Error>, at: &str| {
        matches!(answer, Err(Error::Refused { field: Some(f), reason: Refusal::Invalid(said) })
            if f.as_str() == at && matches!(said.case, Case::ValidTimeNotTime { .. }))
    };
    let mut wrong = Vec::new();
    for k in ["text", "number", "decimal", "boolean", "json"] {
        for valid in [("at", "from"), ("from", "at")] {
            let answer = c
                .declare(own, stamped(kind("at", k), valid))
                .await
                .map(drop);
            if !refused(&answer, "at") {
                wrong.push(format!("`at` of kind {k} as {valid:?}: {answer:?}"));
            }
        }
    }
    for k in ["date", "date_time"] {
        let mut ok = stamped(kind("at", k), ("at", "from"));
        if let (Declaration::Type(t), "date") = (&mut ok, k) {
            t.name = ty("dated");
        }
        if let Err(e) = c.declare(own, ok).await {
            wrong.push(format!("a valid time of kind {k}: {e}"));
        }
    }
    let text = stamped(kind("at", "text"), ("at", "from"));
    let Declaration::Type(to) = text.clone() else {
        unreachable!("a type")
    };
    let m = Migration {
        to,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let declarations = vec![text.clone()];
    let proposed = lakelands_api::write::ProposedWrite::Declare { declarations };
    for (door, answer) in [
        ("a migration", c.migrate(own, m).await.map(drop)),
        ("`apply`", c.apply(own, vec![text]).await.map(drop)),
        (
            "a proposal",
            c.propose(own, proposed, "text".into()).await.map(drop),
        ),
    ] {
        if !refused(&answer, "at") {
            wrong.push(format!("{door} of `at` as text: {answer:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a valid time names a date-time field: {wrong:?}"
    );
}

/// A `date` field keeps its day: a date or a UTC midnight is written as
/// `YYYY-MM-DD`, another instant is refused naming the field, and a
/// migration converts `text` and `date_time` to a date, and `text` to a
/// `date_time` in its one form, as the store does (lakelands#702).
pub async fn a_date_is_kept_as_its_day(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let ev = |kinds: [&str; 3]| {
        let fields: Vec<Value> = ["on", "t", "u"]
            .iter()
            .zip(kinds)
            .map(|(name, kind)| json!({"name": name, "type": {"kind": kind}}))
            .collect();
        let d = json!({"declare": "type", "name": "ev", "fields": fields});
        serde_json::from_value::<Declaration>(d).expect("a declaration reads")
    };
    c.declare(own, ev(["date", "text", "text"])).await.unwrap();
    let put = |k: &str, fields: Value| {
        let fields = serde_json::from_value(fields).expect("a patch");
        let (c, k) = (c.clone(), key(k));
        async move {
            (c.upsert(own, &ty("ev"), &k, fields, WriteOpts::default())
                .await)
                .map(drop)
        }
    };
    let mut wrong = Vec::new();
    put(
        "e1",
        json!({"on": "2026-07-05T00:00:00Z", "t": "2026-07-06", "u": "2026-07-07T00:00:00+00:00"}),
    )
    .await
    .unwrap();
    let e1 = c.get(own, &ty("ev"), get("e1")).await.unwrap().data;
    if e1.fields.get("on") != Some(&json!("2026-07-05")) {
        wrong.push(format!("`on` written at a midnight: {:?}", e1.fields));
    }
    let refused = put("e2", json!({"on": "2026-07-05T10:00:00Z"})).await;
    let said = matches!(&refused, Err(Error::Refused { field: Some(f), reason: Refusal::Constraint(said) | Refusal::Invalid(said) })
        if f.as_str() == "on" && matches!(&said.case, Case::NotDate { .. }));
    if !said {
        wrong.push(format!("`on` written at 10:00: {refused:?}"));
    }
    // `date → date_time`, `text → date` and `text → date_time`.
    let Declaration::Type(to) = ev(["date_time", "date", "date_time"]) else {
        unreachable!("a type")
    };
    let m = Migration {
        to,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m.clone()).await.unwrap();
    c.migrate_apply(own, m, &report.data.report_id)
        .await
        .unwrap();
    let e1 = c.get(own, &ty("ev"), get("e1")).await.unwrap().data;
    let want = json!({"on": "2026-07-05T00:00:00.000000Z", "t": "2026-07-06",
                      "u": "2026-07-07T00:00:00.000000Z"});
    if json!(e1.fields) != want {
        wrong.push(format!("e1 migrated: {:?}, not {want}", e1.fields));
    }
    // `date_time → date` over an instant at 10:00 is lossy.
    put("e3", json!({"on": "2026-07-05T10:00:00Z"}))
        .await
        .unwrap();
    let Declaration::Type(to) = ev(["date", "date", "date_time"]) else {
        unreachable!("a type")
    };
    let m = Migration {
        to,
        renames: Vec::new(),
        relation_renames: Vec::new(),
    };
    let report = c.migrate(own, m).await.unwrap().data;
    let on = report.fields.iter().find(|f| f.field.as_str() == "on");
    if report.lossless || on.is_none_or(|f| f.sample_keys != [key("e3")]) {
        wrong.push(format!("`on` to a date over e3's 10:00: {report:?}"));
    }
    assert!(wrong.is_empty(), "a date field: {wrong:?}");
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

/// A relation carried by a reference field answers nothing of the field to
/// a reader held from it (024, extended): one who may not know of `tgt`
/// is not shown `r`, and a walk over it either way answers as over a
/// relation that is not there; one who sees `tgt` masked is shown `r`,
/// and a walk over it is refused as not granted, naming `tgt`. An author
/// held from `tgt` who sends `r` again declares it twice.
pub async fn a_relation_carried_by_a_hidden_field_answers_nothing_of_it(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let tgt = json!({"name": "tgt", "type": {"kind": "reference", "target": "other"}});
    for d in [
        json!({"declare": "type", "name": "other", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "wk", "fields": [{"name": "no", "type": {"kind": "text"}}, tgt],
               "relations": [{"name": "r", "target": "other", "via": "tgt"}]}),
    ] {
        let d = serde_json::from_value::<Declaration>(d).expect("a declaration reads");
        c.declare(own, d).await.expect("the owner declares");
    }
    for (t, k, p) in [("other", "o1", ("n", "o")), ("wk", "w1", ("tgt", "o1"))] {
        let p = patch(&[(p.0, json!(p.1))]);
        c.upsert(own, &ty(t), &key(k), p, WriteOpts::default())
            .await
            .expect("the owner writes");
    }
    let step = |walk| Step {
        relation: "r".into(),
        walk,
        ty: None,
        filter: None,
        depth: Default::default(),
        named: None,
    };
    let mut wrong = Vec::new();
    for access in ["none", "masked"] {
        let grants = json!({"default": "read", "types": {"wk": {"fields": {"tgt": access}}}});
        let grants = serde_json::from_value(grants).expect("grants read");
        let who = &principal(store.lake(), PrincipalKind::Agent, "held", grants);
        for (from, k, walk) in [("wk", "w1", Walk::Out), ("other", "o1", Walk::In)] {
            let walked = Pattern::traverse(ty(from), key(k), vec![step(walk)]);
            let answer = c.query(who, walked).await;
            let absent = Case::NoRelationToWalk {
                ty: ty(from),
                relation: "r".into(),
                walk,
            };
            let right = match (&answer, access) {
                (Err(e), "none") => e == &invalid(None, absent),
                (
                    Err(Error::Refused {
                        field: Some(f),
                        reason: Refusal::NotGranted,
                    }),
                    _,
                ) => access == "masked" && f.as_str() == "tgt",
                _ => false,
            };
            if !right {
                wrong.push(format!("a walk from `{k}`, `tgt` {access}: {answer:?}"));
            }
        }
        let told = c.describe_type(who, &ty("wk"), ReadAt::default()).await;
        let told = told.expect("`wk` is described").data;
        if told.relations.iter().any(|r| r.name.as_str() == "r") != (access == "masked") {
            wrong.push(format!(
                "`describe_type` of `wk`, `tgt` {access}: {:?}",
                told.relations
            ));
        }
    }
    // An author held from `tgt` who sends `r` again declares it twice.
    let grants =
        json!({"default": "read", "schema": true, "types": {"wk": {"fields": {"tgt": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let author = &principal(store.lake(), PrincipalKind::Agent, "author", grants);
    let sent = json!({"declare": "type", "name": "wk", "fields": [{"name": "no", "type": {"kind": "text"}}],
                      "relations": [{"name": "r", "target": "other"}]});
    let sent = serde_json::from_value::<Declaration>(sent).expect("a declaration reads");
    let sent = c.declare(author, sent).await;
    let twice = |at: &[Place]| {
        let case = Case::DeclaredTwice {
            what: Naming::Relation,
            name: "r".into(),
        };
        Err(invalid(None, said_at(at, case)))
    };
    if sent != twice(&[of_type("wk")]) {
        wrong.push(format!("`r` sent again by its author: {sent:?}"));
    }
    // The owner who sends `wk` with two `r` is refused in the same words.
    let r = json!({"name": "r", "target": "other", "via": "tgt"});
    let sent = json!({"declare": "type", "name": "wk", "fields": [{"name": "no", "type": {"kind": "text"}},
                      {"name": "tgt", "type": {"kind": "reference", "target": "other"}}],
                      "relations": [r, {"name": "r", "target": "other"}]});
    let sent = serde_json::from_value::<Declaration>(sent).expect("a declaration reads");
    let sent = c.declare(own, sent).await;
    if sent != twice(&[of_type("wk")]) {
        wrong.push(format!("`r` sent twice by the owner: {sent:?}"));
    }
    // From `s1`, `wk`'s `r` comes up only around `o1`, past the first hop.
    let side = json!({"declare": "type", "name": "side", "fields": [{"name": "pt", "type": {"kind": "reference", "target": "other"}}],
                      "relations": [{"name": "r", "target": "other", "via": "pt"}]});
    let side = serde_json::from_value::<Declaration>(side).expect("a declaration reads");
    c.declare(own, side).await.expect("the owner declares");
    let p = patch(&[("pt", json!("o1"))]);
    c.upsert(own, &ty("side"), &key("s1"), p, WriteOpts::default())
        .await
        .expect("the owner writes");
    for access in ["none", "masked"] {
        let grants = json!({"default": "read", "types": {"wk": {"fields": {"tgt": access}}}});
        let grants = serde_json::from_value(grants).expect("grants read");
        let who = &principal(store.lake(), PrincipalKind::Agent, "held", grants);
        let mut p = Pattern::traverse(ty("side"), key("s1"), vec![step(Walk::Both)]);
        p.project = lakelands_api::query::Projection::Components;
        let answer = c.query(who, p).await;
        let right = match &answer {
            Ok(r) => access == "none" && !format!("{:?}", r.data).contains("w1"),
            Err(Error::Refused {
                field: Some(f),
                reason: Refusal::NotGranted,
            }) => access == "masked" && f.as_str() == "tgt",
            Err(_) => false,
        };
        if !right {
            wrong.push(format!(
                "the components from `s1`, `tgt` {access}: {answer:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "`r` answers nothing of `tgt`: {}",
        wrong.join("; ")
    );
}

/// A verb on a type its reader is not shown is not described, nor made a
/// tool, and is called and proposed as one not declared; a condition its
/// reviewer is listed names no function `describe` hides from it, nor is
/// one described that computes a field of a type not shown
/// (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#770).
pub async fn describe_names_no_verb_or_function_its_reader_is_not_shown(store: Arc<dyn LakeStore>) {
    let c = store.catalog(WriteContext::default());
    let own = &Cast::of(store.lake()).owner;
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let computed = |name: &str, from: &str| {
        json!({"name": name, "type": {"kind": "boolean"},
        "authority": "derived", "function": name, "depends_on": [from]})
    };
    for d in [
        json!({"declare": "function", "name": "shut", "backing": "expr",
            "inputs": [text("cat")], "expr": "cat = 'open'"}),
        json!({"declare": "type", "name": "vault", "key": ["code"],
            "fields": [text("code"), text("cat"), computed("shut", "cat")]}),
        json!({"declare": "verb", "name": "seal", "on": "vault", "risk": "low", "effect": "write",
            "set": [{"field": "cat", "value": "'b'"}]}),
        json!({"declare": "value_type", "name": "status", "kind": "text", "one_of": ["open", "done"]}),
        json!({"declare": "function", "name": "opens", "backing": "expr",
            "inputs": [text("note")], "expr": "note = 'open'"}),
        json!({"declare": "type", "name": "task", "fields": [text("note"),
            {"name": "state", "type": {"kind": "text", "value_type": "status"}},
            computed("opens", "note")]}),
    ] {
        let d = serde_json::from_value(d).expect("a declaration reads");
        c.declare(own, d).await.expect("the owner declares");
    }
    let p = serde_json::from_value(json!({"code": "v1"})).expect("a patch reads");
    (c.upsert(own, &ty("vault"), &key("v1"), p, WriteOpts::default())
        .await)
        .expect("v1");
    let to = json!({"name": "status", "kind": "text", "one_of": ["active", "done"]});
    let write = ProposedWrite::MigrateValueType {
        to: Box::new(serde_json::from_value(to).expect("a value type reads")),
        renames: vec![("open".into(), "active".into())],
        report: Default::default(),
    };
    (c.propose(own, write, "a rename".into()).await).expect("the owner proposes");
    // `code` held at `none` keeps `vault` and `shut`, which only `vault`
    // uses; `note` held at `none` keeps `opens`.
    let grants = json!({"default": "write", "review": true, "types": {
        "vault": {"fields": {"code": "none"}}, "task": {"fields": {"note": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let who = &principal(store.lake(), PrincipalKind::Agent, "kept", grants);
    let mut wrong = Vec::new();
    let d = (c.describe(who, ReadAt::default()).await)
        .expect("describe")
        .data;
    let tools = format!("{:?}", lakelands_api::tools::tools(&d));
    if !d.verbs.is_empty() || !d.functions.is_empty() || tools.contains("vault") {
        let verbs: Vec<_> = d.verbs.iter().map(|v| &v.name).collect();
        let functions: Vec<_> = d.functions.iter().map(|f| &f.name).collect();
        wrong.push(format!("`kept` is described {verbs:?} and {functions:?}"));
    }
    let mut answers = Vec::new();
    for verb in ["seal", "ghost"] {
        let (name, k, opts) = (verb.into(), Some(key("v1")), WriteOpts::default);
        let call = c
            .call(who, &name, k.as_ref(), Patch::default(), opts())
            .await;
        let (key, payload) = (k, Patch::default());
        let write = ProposedWrite::Call {
            verb: name,
            key,
            payload,
            opts: opts(),
        };
        let proposed = c.propose(who, write, "why".into()).await;
        answers.push(format!("{call:?} {proposed:?}").replace(verb, "V"));
    }
    if answers[0] != answers[1] {
        wrong.push(format!("`seal` and an undeclared verb answer {answers:?}"));
    }
    let listed = (c.proposals(who, ProposalFilter::default()).await)
        .expect("listed")
        .data;
    let conditions = format!("{:?}", listed.iter().map(|p| &p.write).collect::<Vec<_>>());
    if conditions.contains("`opens`") || conditions.contains("`shut`") {
        wrong.push(format!("listed {conditions}"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("; "));
}

/// What `who` is listed: every proposal, and those on invoices.
async fn listings(catalog: &dyn Catalog, who: &Principal) -> Vec<Vec<Proposal>> {
    let mut lists = Vec::new();
    for on in [None, Some(ty(INVOICE))] {
        let filter = ProposalFilter {
            ty: on,
            ..ProposalFilter::default()
        };
        let listed = catalog.proposals(who, filter).await;
        lists.push(listed.expect("the reviewer lists").data);
    }
    lists
}

/// A verb whose `requires` or set values read a field its caller may not
/// read, a field derived from one or a reference to a type it is not
/// shown, is not its to call, propose or be shown, and a call proposal of
/// it is not there to such a reviewer (lakelands#779).
pub async fn a_verb_that_reads_a_field_its_caller_may_not_read_is_not_callable(
    store: Arc<dyn LakeStore>,
) {
    let f = setup(store).await;
    let own = &f.cast.owner;
    let shade = json!({"name": "shade", "type": {"kind": "text"}, "authority": "derived",
        "derived": "secret", "depends_on": ["secret"]});
    let shade = serde_json::from_value(shade).expect("a field");
    (f.catalog.add_field(own, &ty(INVOICE), shade).await).expect("the owner adds `shade`");
    f.write(
        own,
        "INV-1",
        &[("status", json!("open")), ("secret", json!("x"))],
    )
    .await;
    f.write(own, "INV-1", &[("note", json!("n"))]).await;
    let set = |value: &str| json!([{"field": "status", "value": value}]);
    let verdict = json!([{"field": "verdict", "value": "'ok'"}]);
    for (name, risk, requires, set) in [
        ("vouch", "low", "secret = 'x'", set("'vouched'")),
        ("echo", "low", "status = 'open'", set("note")),
        ("check", "high", "note = 'n'", set("'checked'")),
        ("stamp", "low", "status = 'open'", verdict.clone()),
        ("shadow", "low", "shade = 'x'", set("'shaded'")),
        ("bill", "low", "partner = 'P-1'", verdict),
    ] {
        let verb = json!({"declare": "verb", "name": name, "on": INVOICE, "risk": risk,
            "requires": requires, "effect": "write", "set": set});
        let d = serde_json::from_value(verb).expect("a declaration reads");
        f.catalog.declare(own, d).await.expect("the owner declares");
    }
    // The answer whole: a refusal with its field and reason.
    let answer = |a: Result<Outcome, Error>| match a {
        Ok(Outcome::Applied { .. }) => "applied".to_owned(),
        Ok(Outcome::Proposed { .. }) => "proposed".to_owned(),
        other => format!("{other:?}"),
    };
    let grants = json!({"default": "read", "review": true, "types": {
        INVOICE: {"fields": {"secret": "none", "note": "masked"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let blind = &principal(f.store.lake(), PrincipalKind::Human, "blind", grants);
    let grants = json!({"default": "read", "types": {
        INVOICE: {"fields": {"status": "write", "secret": "write_only"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let sealed = &principal(f.store.lake(), PrincipalKind::Agent, "sealed", grants);
    // Reads `partner` of an invoice, and is not shown the `partner` type.
    let grants = json!({"default": "read", "types": {
        INVOICE: {"fields": {"verdict": "write"}}, PARTNER: {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let unseen = &principal(f.store.lake(), PrincipalKind::Agent, "unseen", grants);
    let (clerk, opts) = (&f.cast.clerk, WriteOpts::default);
    let mut got = Vec::new();
    for (who, verb) in [
        (clerk, "stamp"),
        (clerk, "echo"),
        (clerk, "vouch"),
        (clerk, "check"),
        (sealed, "vouch"),
        (clerk, "shadow"),
        (unseen, "bill"),
    ] {
        let (k, verb) = (Some(key("INV-1")), VerbName::from(verb));
        let call = f.catalog.call(who, &verb, k.as_ref(), Patch::new(), opts());
        got.push(answer(call.await));
    }
    let (verb, key_, payload) = ("vouch".into(), Some(key("INV-1")), Patch::new());
    let write = ProposedWrite::Call {
        verb,
        key: key_,
        payload,
        opts: opts(),
    };
    got.push(answer(f.catalog.propose(clerk, write, "why".into()).await));
    let checked = || async {
        let (k, verb) = (Some(key("INV-1")), VerbName::from("check"));
        let call = f.catalog.call(own, &verb, k.as_ref(), Patch::new(), opts());
        let Ok(Outcome::Proposed { proposal, .. }) = call.await else {
            panic!("the owner calling `check` proposes it");
        };
        proposal
    };
    let before = listings(f.catalog.as_ref(), blind).await;
    let proposals = [checked().await, checked().await];
    let why = || "why".to_owned();
    got.push(answer(f.catalog.accept(blind, &proposals[0]).await));
    got.push(answer(f.catalog.reject(blind, &proposals[1], why()).await));
    got.push(answer(
        f.catalog.accept(&f.cast.reviewer, &proposals[1]).await,
    ));
    let refused = format!(
        "{:?}",
        Err::<Outcome, _>(Error::Refused {
            field: None,
            reason: Refusal::NotGranted,
        })
    );
    // A proposal that is not there is answered as an id no proposal holds.
    let absent = |id: &ProposalId| {
        let missing = Missing::Proposal { id: id.clone() };
        format!("{:?}", Err::<Outcome, _>(Error::NotFound { missing }))
    };
    let none = ProposalId::from("none");
    assert_eq!(
        answer(f.catalog.accept(blind, &none).await),
        absent(&none),
        "an id no proposal holds"
    );
    let mut want = vec![refused; 11];
    want[8] = absent(&proposals[0]);
    want[9] = absent(&proposals[1]);
    want[10] = "applied".to_owned();
    assert_eq!(
        got, want,
        "a verb the clerk may not write, the clerk's, the write-only and the \
         unseen caller's calls and the clerk's proposal, then blind's accept \
         and reject and the reviewer's accept"
    );
    // `check` was proposed only where `note` held what it requires: to
    // `blind` its proposals are not there, so the listings are those of
    // the lake before them.
    let after = listings(f.catalog.as_ref(), blind).await;
    assert_eq!(after, before, "`blind` lists no `check`");
    let listed = f
        .catalog
        .proposals(&f.cast.reviewer, ProposalFilter::default());
    let listed = listed.await.expect("the reviewer lists").data;
    let entry = listed.iter().find(|p| p.id == proposals[0]);
    let entry = entry.map(|p| (p.write.is_some(), p.more));
    assert_eq!(entry, Some((true, false)), "the reviewer lists `check`");
    // Declared again, `check` is still not there to `blind`, before the
    // accept is told it was redeclared.
    let pending = checked().await;
    let again = json!({"declare": "verb", "name": "check", "on": INVOICE, "risk": "high",
        "requires": "note = 'n'", "effect": "write", "set": set("'rechecked'")});
    let again = serde_json::from_value(again).expect("a declaration reads");
    f.catalog
        .declare(own, again)
        .await
        .expect("the owner declares");
    let got = [
        answer(f.catalog.accept(blind, &pending).await),
        answer(f.catalog.reject(blind, &pending, why()).await),
    ];
    assert_eq!(
        got,
        [absent(&pending), absent(&pending)],
        "a redeclared `check`"
    );
    let redeclared = f.catalog.accept(&f.cast.reviewer, &pending).await;
    let verb = VerbName::from("check");
    let told = |e: &Error| match refusal(e) {
        (None, Refusal::Invalid(said)) => said.case == Case::VerbRedeclared { verb },
        _ => false,
    };
    assert!(
        redeclared.as_ref().is_err_and(told),
        "the reviewer is told `check` was declared again: {redeclared:?}"
    );
    let shown = |d: &lakelands_api::describe::LakeDescription| {
        let on = d.types.iter().find(|t| t.name.as_str() == INVOICE);
        let on: Vec<String> = on
            .iter()
            .flat_map(|t| t.verbs.iter().map(|v| v.to_string()))
            .collect();
        let all: Vec<String> = d.verbs.iter().map(|v| v.name.to_string()).collect();
        (on, all)
    };
    let d = f
        .catalog
        .describe(clerk, ReadAt::default())
        .await
        .expect("describe")
        .data;
    let fixture = vec!["approve".to_owned(), "pay".to_owned()];
    assert_eq!(shown(&d), (fixture.clone(), fixture), "the clerk is shown");
}
