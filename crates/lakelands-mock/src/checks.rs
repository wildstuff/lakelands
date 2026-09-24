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

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::error::{Error, Missing, Refusal};
use lakelands_api::expr::Condition;
use lakelands_api::id::{FieldName, LakeId, Position, RecordKey, TypeName, VerbName, Version};
use lakelands_api::ontology::{
    Assignment, Authority, FieldDecl, FieldType, OnInputChange, RelationDecl, Risk, StorageKind,
    TypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::{Access, Grants, Principal, PrincipalKind, TypeGrants};
use lakelands_api::query::{Pattern, Step, Walk};
use lakelands_api::read::{Find, Found, Get, ReadAt};
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
#[macro_export]
macro_rules! door_checks {
    ($new:expr, $block_on:path) => {
        $crate::door_checks!(@each $new, $block_on;
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
            a_retired_record_leaves_reads_but_not_its_past,
            a_narrow_writer_cannot_retire,
            a_proposer_sees_only_its_own_proposals,
            ingest_is_content_addressed,
            a_cursor_keeps_its_page_set_while_writes_land,
            a_principal_of_another_lake_is_refused,
            the_feed_is_the_log_by_position,
            a_step_over_an_undeclared_relation_is_refused,
            a_dependent_field_declared_clear_is_cleared_with_its_input,
            two_writers_keep_each_others_fields,
            per_field_meta_names_the_writer_and_the_position,
            apply_declares_a_set_in_one_append_with_forward_references,
            apply_refuses_the_whole_set_naming_every_problem,
            declare_refuses_a_reference_to_an_undeclared_type);
    };
    (@each $new:expr, $block_on:path; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                $block_on($crate::checks::$name(($new)()));
            }
        )*
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

    /// The log's length, as the feed sees it.
    async fn events(&self) -> usize {
        let b = self
            .store
            .feed()
            .poll(Position::GENESIS, &FeedFilter::default(), u32::MAX)
            .await
            .expect("the feed reads");
        b.events.len()
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
    assert_eq!(f.store.rebuild().await.unwrap(), p2);
    assert_eq!(
        f.events().await,
        b.head.0 as usize + 1,
        "a rebuild changes nothing in the log"
    );
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
    assert!(message.contains("verb haunt"), "{message}");
    assert!(message.contains("type partner"), "{message}");
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
