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

use lakelands_api::error::{Error, Missing, Refusal};
use lakelands_api::expr::Condition;
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, RecordKey, TypeName, VerbName, Version,
};
use lakelands_api::ontology::{
    Assignment, Authority, FieldDecl, FieldType, OnInputChange, RelationDecl, Risk, StorageKind,
    TypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::{Access, Grants, Principal, PrincipalKind, TypeGrants};
use lakelands_api::query::{Pattern, Step, Walk};
use lakelands_api::read::{After, Direction, Find, Found, Get, Order, OrderBy, ReadAt};
use lakelands_api::record::{Origin, Patch, Record, SourceRecord, Value};
use lakelands_api::seam::FeedFilter;
use lakelands_api::tools::{Door, tools};
use lakelands_api::write::{Declaration, Outcome, ProposalFilter, ProposalState, WriteOpts};
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
            a_masked_field_is_named_but_its_value_is_absent,
            the_tools_follow_the_grants,
            a_low_risk_verb_runs_at_once,
            a_riskier_verb_is_proposed_and_accept_applies_it,
            a_rejected_proposal_writes_nothing_and_stays_decided,
            only_a_reviewer_decides_proposals,
            sys_types_cannot_be_declared,
            an_embedding_is_unbound_where_no_backend_is_bound,
            a_link_to_no_live_record_or_over_no_link_relation_is_refused,
            a_retired_record_leaves_reads_but_not_its_past,
            a_narrow_writer_cannot_retire,
            a_proposer_sees_only_its_own_proposals,
            ingest_is_content_addressed,
            a_cursor_keeps_its_page_set_while_writes_land,
            after_walks_the_pages_the_cursor_walks,
            after_is_refused_naming_the_member,
            a_principal_of_another_lake_is_refused,
            the_feed_is_the_log_by_position,
            the_feed_filtered_by_key_answers_those_records_events,
            a_step_over_an_undeclared_relation_is_refused,
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
            links_are_written_walked_and_held_to_their_relation,
            a_relation_declared_again_with_another_target_reshapes_its_links,
            #[host_ops] an_aging_rule_moves_what_holds_from_once_a_date,
            #[host_ops] a_purge_retires_what_one_source_brought_in_and_refuses_a_shared_record,
            a_value_type_is_loosened_by_declare_and_tightened_by_its_preflight);
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
        name: name.into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: Vec::new(),
        on_input_change: OnInputChange::Keep,
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
            payload: Vec::new(),
            risk: Risk::Low,
            requires: None,
            effect: set("approved"),
            description: None,
        }),
        Declaration::Verb(VerbDecl {
            name: "pay".into(),
            on: INVOICE.into(),
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
    /// and `partner`.
    pub clerk: Principal,
    /// Reads everything, writes `verdict` and `reason`.
    pub auditor: Principal,
    /// Reads, and reviews proposals.
    pub reviewer: Principal,
    /// Feeds `amount` from a source.
    pub importer: Principal,
    /// Reads `status` of invoices and nothing else.
    pub stranger: Principal,
}

impl Cast {
    /// The cast for one lake.
    pub fn of(lake: &LakeId) -> Cast {
        let mut importer = invoice_grants(Access::None, &[("amount", Access::Write)]);
        importer.ingest = true;
        Cast {
            owner: principal(lake, PrincipalKind::Human, "owner", Grants::owner()),
            clerk: principal(
                lake,
                PrincipalKind::Agent,
                "clerk",
                invoice_grants(
                    Access::Read,
                    &[
                        ("status", Access::Write),
                        ("partner", Access::Write),
                        ("secret", Access::None),
                        ("note", Access::Masked),
                    ],
                ),
            ),
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
/// head it saw, and the head moves to it.
pub async fn every_write_advances_the_position(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let before = f.head().await;
    let (p1, v1) = applied(
        &f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
            .await,
    );
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
    assert_eq!(
        refusal(&e).1,
        &Refusal::NotGranted,
        "a filter cannot probe a hidden field"
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

/// A `low`-risk verb whose effect writes authored fields runs at once.
pub async fn a_low_risk_verb_runs_at_once(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    f.write(&f.cast.clerk, "INV-1", &[("status", json!("open"))])
        .await;
    let o = f
        .catalog
        .call(
            &f.cast.clerk,
            &VerbName::from("approve"),
            &key("INV-1"),
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
            &key("INV-1"),
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
            &key("INV-1"),
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
            &key("INV-1"),
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
                    &key("INV-1"),
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

/// An ordinary principal cannot declare a `sys.` type; records of one are
/// written under grants like any other.
pub async fn sys_types_cannot_be_declared(store: Arc<dyn LakeStore>) {
    let f = setup(store).await;
    let decl = TypeDecl {
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
    let Found::Records { rows, next_cursor } = first.data else {
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
    } = second.data
    else {
        panic!("find lists records");
    };
    let keys: Vec<&str> = rows.iter().chain(&more).map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["INV-1", "INV-2", "INV-3"]);
    assert!(next_cursor.is_none());
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
            Found::Records { rows, next_cursor } => (rows, next_cursor),
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
/// the fixture's invoice type hashes to one digest, and a proposal is
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
                .call(&clerk, &"pay".into(), &key("INV-1"), patch(&[]), opts)
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
    // Who may not write the link type is not granted.
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
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "{stranger:?}"
    );
    // A source relation's links arrive by ingest, its cardinality held in
    // one window as at the door.
    let window = vec![
        SourceRecord {
            ty: ty("sys.link.doc.witnessed_by"),
            key: key("d1:p1"),
            fields: patch(&[("from", json!("d1")), ("to", json!("p1"))]),
            seen_at: None,
        },
        SourceRecord {
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
    };
    let report = c.migrate(own, m.clone()).await.unwrap();
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
    let report = store.purge_source(&"crm".into()).await.unwrap();
    assert_eq!(report.retired, 2, "{report:?}");
    assert_eq!(report.position, store.head().await.unwrap());
    let live = |k: &'static str| {
        let c = c.clone();
        let own = own.clone();
        async move { c.get(&own, &ty("partner"), get(k)).await.is_ok() }
    };
    assert!(!live("p1").await && !live("p2").await && live("p3").await);
    let head = store.head().await.unwrap();
    let again = store.purge_source(&"crm".into()).await.unwrap();
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
    let refused = store.purge_source(&"crm".into()).await;
    assert!(
        matches!(&refused, Err(Error::Refused { field: None, reason: Refusal::Invalid { message } })
            if message.contains("p4") && !message.contains("p5")),
        "{refused:?}"
    );
    assert_eq!(store.head().await.unwrap(), head);
    assert!(live("p5").await);
    // A record the source left no value in is not its any more.
    let clear = SourceRecord {
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
    let report = store.purge_source(&"crm".into()).await.unwrap();
    assert_eq!(report.retired, 1, "{report:?}");
    assert!(!live("p4").await && live("p5").await && live("p7").await);
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
