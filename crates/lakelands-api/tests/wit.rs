//! The drift check between this crate and the WIT package in `wit/`
//! (`lakelands:data@0.13.0`). The crate is the source; the WIT mirrors it,
//! and this test fails as soon as the two disagree.
//!
//! Three things are compared:
//!
//! - **The doors.** The methods of `Catalog`, `LakeStore` and
//!   `ChangeFeed`, read from this crate's source, against the functions
//!   of the WIT's `catalog`, `lake-store` and `change-feed` resources and
//!   of the `query` and `schema` interfaces.
//! - **The records.** Every mirrored struct is built here as a literal
//!   that names every field, with every optional member present, and its
//!   serde members are compared with the WIT record's fields. A field
//!   added to a struct stops this file compiling until it is mirrored.
//! - **The cases.** Every mirrored enum is listed through an exhaustive
//!   `match`, and the names serde gives its variants are compared with
//!   the WIT enum's or variant's cases.
//!
//! Names compare in snake_case: WIT's kebab-case with `-` read as `_`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use lakelands_api::describe::{
    ExecutorDescription, FieldDescription, Kept, LakeDescription, LakeRights, NotKept, TypeAccess,
    TypeDescription, TypeLifecycle, TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::expr::Condition;
use lakelands_api::location::Limits;
use lakelands_api::ontology::{
    Assignment, Authority, Cardinality, ComputedAt, EmbedDecl, FieldDecl, FieldType, Measure,
    OnInputChange, RelationDecl, Risk, StorageKind, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl,
    VerbEffect, VerbScope, ViewDecl, ViewParam, ViewSource,
};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{
    Axis, Depth, Edge, Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Start,
    Step, VectorTerm, Walk,
};
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Direction, Group, Order, OrderBy};
use lakelands_api::seam::{FeedBatch, FeedFilter};
use lakelands_api::write::{
    AgeReport, Carried, Carrier, CompactReport, Declaration, FieldConversion, FieldFill,
    FunctionDecl, Migration, MigrationReport, Misfit, ObjectSetDecl, Proposal, ProposalFilter,
    ProposalState, ProposedWrite, PurgeReport, RecordRef, ReferenceRewrite, Rekeyed,
    RelationRename, Restated, RuleRun, TypeRetired, ValueRenamed, ValueTypeMigration,
    ValueTypeReport, ValueTypeUse,
};
use lakelands_api::{
    Access, After, Committed, Cursor, Error, FieldMeta, Find, Found, Get, Grants, Missing, Origin,
    Outcome, Position, Principal, PrincipalKind, ReadAt, Record, Refusal, SourceRecord, Timestamp,
    Verification, Version, WriteContext, WriteOpts, WriteOrigin,
};
use serde::Serialize;
use serde_json::{Value, json};
use wit_parser::{FunctionKind, Resolve, Type, TypeDefKind};

// ----------------------------------------------------------------------
// Reading the WIT
// ----------------------------------------------------------------------

/// What the WIT package declares, by snake_case name.
struct Wit {
    /// Record name -> field names.
    records: BTreeMap<String, BTreeSet<String>>,
    /// Enum or variant name -> case names.
    cases: BTreeMap<String, BTreeSet<String>>,
    /// Owner (a resource, or an interface for its free functions) ->
    /// function names.
    functions: BTreeMap<String, BTreeSet<String>>,
}

fn snake(s: &str) -> String {
    s.replace('-', "_")
}

fn wit() -> Wit {
    let mut resolve = Resolve::default();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit");
    resolve
        .push_dir(&dir)
        .unwrap_or_else(|e| panic!("the WIT package does not parse: {e:?}"));
    let mut wit = Wit {
        records: BTreeMap::new(),
        cases: BTreeMap::new(),
        functions: BTreeMap::new(),
    };
    for (_, ty) in resolve.types.iter() {
        let Some(name) = &ty.name else { continue };
        match &ty.kind {
            TypeDefKind::Record(r) => {
                wit.records.insert(
                    snake(name),
                    r.fields.iter().map(|f| snake(&f.name)).collect(),
                );
            }
            TypeDefKind::Variant(v) => {
                wit.cases.insert(
                    snake(name),
                    v.cases.iter().map(|c| snake(&c.name)).collect(),
                );
            }
            TypeDefKind::Enum(e) => {
                wit.cases.insert(
                    snake(name),
                    e.cases.iter().map(|c| snake(&c.name)).collect(),
                );
            }
            _ => {}
        }
    }
    for (_, iface) in resolve.interfaces.iter() {
        let iface_name = iface.name.clone().unwrap_or_default();
        for f in iface.functions.values() {
            let owner = match f.kind {
                FunctionKind::Method(id) | FunctionKind::AsyncMethod(id) => {
                    resolve.types[id].name.clone().unwrap_or_default()
                }
                _ => iface_name.clone(),
            };
            wit.functions
                .entry(snake(&owner))
                .or_default()
                .insert(snake(f.item_name()));
            // A free function of `query` or `schema` is a catalog door: it
            // takes the catalog it acts on first.
            if let (FunctionKind::Freestanding, Some(first)) = (&f.kind, f.params.first())
                && let Type::Id(id) = first.ty
                && let TypeDefKind::Handle(_) = resolve.types[id].kind
            {
                assert_eq!(
                    first.name, "catalog",
                    "`{}` takes a handle first; it must be the catalog",
                    f.name
                );
            }
        }
    }
    wit
}

// ----------------------------------------------------------------------
// The doors
// ----------------------------------------------------------------------

/// The names of the methods of `trait <name>` in one of this crate's
/// source files.
fn trait_methods(source: &str, name: &str) -> BTreeSet<String> {
    let start = source
        .find(&format!("pub trait {name}"))
        .unwrap_or_else(|| panic!("no `pub trait {name}` in the source"));
    let body = &source[start..];
    let open = body.find('{').expect("a trait body");
    let mut depth = 0;
    let mut end = open;
    for (i, c) in body[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    body[open..end]
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter_map(|l| {
            let rest = l.split_once("fn ")?.1;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

#[test]
fn every_door_of_the_crate_is_a_function_of_the_wit_and_no_other() {
    let wit = wit();
    let catalog_rs = include_str!("../src/catalog.rs");
    let seam_rs = include_str!("../src/seam.rs");
    let fns = |owner: &str| wit.functions.get(owner).cloned().unwrap_or_default();

    let catalog: BTreeSet<String> = fns("catalog")
        .into_iter()
        .chain(fns("query"))
        .chain(fns("schema"))
        .collect();
    assert_eq!(
        catalog,
        trait_methods(catalog_rs, "Catalog"),
        "the catalog's doors: the `catalog` resource with `query` and `schema`, against `Catalog`"
    );
    // A door that binds a seam is the host's: the host binds the seams,
    // as at `open`, and nothing a guest holds can be one.
    let mut store = trait_methods(catalog_rs, "LakeStore");
    store.retain(|door| !["bind_embed"].contains(&door.as_str()));
    assert_eq!(
        fns("lake_store"),
        store,
        "the `lake-store` resource against `LakeStore`, less the doors that bind a seam"
    );
    assert_eq!(
        fns("change_feed"),
        trait_methods(seam_rs, "ChangeFeed"),
        "the `change-feed` resource against `ChangeFeed`"
    );
    // The one function the crate has no trait method for: how a component
    // gets a lake from its host, which the crate leaves to the host.
    assert_eq!(fns("store"), BTreeSet::from(["open".to_owned()]));
}

// ----------------------------------------------------------------------
// The records
// ----------------------------------------------------------------------

fn t() -> Timestamp {
    Timestamp(0)
}

fn cond() -> Condition {
    Condition::Text("a = 1".into())
}

fn field_type() -> FieldType {
    FieldType {
        kind: StorageKind::Reference,
        value_type: Some("money".into()),
        target: Some("partner".into()),
        dimensions: Some(3),
    }
}

fn field_decl() -> FieldDecl {
    FieldDecl {
        annotations: [("acme.icon".to_owned(), serde_json::json!("x"))].into(),
        name: "f".into(),
        ty: field_type(),
        required: true,
        authority: Authority::Shared,
        derived: Some(cond()),
        function: Some("g".into()),
        computed_at: Some(ComputedAt::Read),
        depends_on: vec!["e".into()],
        on_input_change: OnInputChange::Clear,
        initial: Some(serde_json::json!("new")),
        sensitive: true,
        searchable: true,
        description: Some("d".into()),
        label: Some("l".into()),
        embed: Some(embed_decl()),
    }
}

fn embed_decl() -> EmbedDecl {
    EmbedDecl {
        from: vec!["t".into()],
        model: "m".into(),
    }
}

fn relation_decl() -> RelationDecl {
    RelationDecl {
        derived: Some(lakelands_api::ontology::Derived {
            pairs: vec![lakelands_api::ontology::MatchPair {
                from: "f".into(),
                to: "g".into(),
            }],
            filter: Some(cond()),
        }),
        annotations: [("acme.icon".to_owned(), serde_json::json!("x"))].into(),
        name: "r".into(),
        target: "partner".into(),
        cardinality: Cardinality::OneToMany,
        inverse: Some("i".into()),
        via: Some("f".into()),
        fields: vec![field_decl()],
        authority: Authority::Source,
        description: Some("d".into()),
    }
}

fn type_decl() -> TypeDecl {
    TypeDecl {
        annotations: [("acme.icon".to_owned(), serde_json::json!("x"))].into(),
        insert_only: true,
        name: "invoice".into(),
        key: vec!["f".into()],
        fields: vec![field_decl()],
        relations: vec![relation_decl()],
        view: Some(Box::new(view_decl())),
        valid_from: Some("f".into()),
        valid_to: Some("f".into()),
        language: Some("en".into()),
        field_meta: true,
        description: Some("d".into()),
        label: Some("l".into()),
    }
}

fn view_decl() -> ViewDecl {
    ViewDecl {
        source: "invoice".into(),
        r#where: Some(cond()),
        group_by: vec!["f".into()],
        measures: vec![measure()],
        params: vec![view_param()],
        merge: vec![view_source()],
    }
}

fn view_source() -> ViewSource {
    ViewSource {
        source: "payment".into(),
        r#where: Some(cond()),
        group_by: vec!["f".into()],
        measures: vec![measure()],
    }
}

fn view_param() -> ViewParam {
    ViewParam {
        name: "anchor".into(),
        kind: StorageKind::DateTime,
    }
}

fn measure() -> Measure {
    Measure {
        name: "m".into(),
        expr: cond(),
    }
}

fn params() -> lakelands_api::Params {
    [("anchor".into(), json!("2026-06-30"))].into()
}

fn read_at() -> ReadAt {
    ReadAt {
        as_of: Some(Position(1)),
        valid_at: Some(t()),
    }
}

/// A case said at places, as a refusal of `invalid`, `constraint`,
/// `precondition` or `unanswered` carries it.
fn said() -> Box<lakelands_api::error::Said> {
    use lakelands_api::error::{Case, Place, Said};
    let case = Case::UnknownField {
        ty: "invoice".into(),
        field: "f".into(),
    };
    let at = Place::Type {
        name: "invoice".into(),
    };
    Box::new(Said::from(case).within(at))
}

fn order() -> Order {
    Order {
        field: "f".into(),
        by: OrderBy::KnownAt,
        dir: Direction::Desc,
    }
}

fn after_row() -> After {
    After {
        values: vec![json!(1)],
        key: "k".into(),
    }
}

fn aggregate() -> Aggregate {
    Aggregate {
        measures: vec![agg_measure()],
        group_by: vec!["f".into()],
    }
}

fn agg_measure() -> AggMeasure {
    AggMeasure {
        name: "m".into(),
        func: AggFunc::Sum,
        field: Some("f".into()),
    }
}

fn record() -> Record {
    Record {
        ty: "invoice".into(),
        key: "k".into(),
        version: Version(1),
        fields: [("f".into(), json!(1))].into(),
        masked: vec!["g".into()],
        meta: [("f".into(), field_meta())].into(),
    }
}

fn upstream() -> lakelands_api::Upstream {
    lakelands_api::Upstream {
        lake: "other".into(),
        position: Position(7),
    }
}

fn field_meta() -> FieldMeta {
    FieldMeta {
        upstream: Some(upstream()),
        origin: Origin::Source,
        known_at: t(),
        seen_at: Some(t()),
        position: Position(1),
        writer: "human:a".into(),
        source: Some("erp".into()),
        procedure_version: Some("1".into()),
        error: Some("the backing failed".into()),
    }
}

fn write_opts() -> WriteOpts {
    WriteOpts {
        expected_version: Some(Version(1)),
        idempotency_key: Some("i".into()),
    }
}

fn migration_report() -> MigrationReport {
    MigrationReport {
        ty: "invoice".into(),
        from_schema_hash: "a".into(),
        to_schema_hash: "b".into(),
        records_scanned: 1,
        fields: vec![FieldConversion {
            field: "f".into(),
            from: field_type(),
            to: field_type(),
            convertible: 1,
            not_convertible: 0,
            sample_keys: vec!["k".into()],
        }],
        dropped: vec![("f".into(), 1)],
        rekeyed: vec![Rekeyed {
            ty: "invoice".into(),
            records: 1,
            collisions: 0,
            missing: 0,
        }],
        references: vec![ReferenceRewrite {
            ty: "payment".into(),
            field: "invoice".into(),
            values: 1,
        }],
        sample_collisions: vec![RecordRef {
            ty: "invoice".into(),
            key: "k".into(),
        }],
        restated: vec![Restated {
            field: Some("f".into()),
            member: "sensitive".into(),
            was: json!(true),
            to: json!(null),
        }],
        filled: vec![FieldFill {
            field: "f".into(),
            records: 1,
            sample_keys: vec!["k".into()],
        }],
        lossless: true,
        report_id: "r".into(),
        more: true,
        relations: vec![RelationRename {
            from: "r".into(),
            to: "s".into(),
            carrier: Carrier::Links,
            links: 1,
        }],
        carried: vec![Carried {
            kind: "trigger".into(),
            name: "t".into(),
        }],
    }
}

fn trigger_decl() -> TriggerDecl {
    TriggerDecl {
        max_depth: Some(2),
        name: "t".into(),
        on: "invoice".into(),
        when: Some(cond()),
        description: Some("d".into()),
    }
}

fn object_set_decl() -> ObjectSetDecl {
    ObjectSetDecl {
        name: "s".into(),
        of: "invoice".into(),
        filter: cond(),
        description: Some("d".into()),
    }
}

fn function_decl() -> FunctionDecl {
    FunctionDecl {
        name: "g".into(),
        backing: "expr".into(),
        inputs: vec![field_decl()],
        expr: Some(cond()),
        config: json!({"a": 1}),
        description: Some("d".into()),
    }
}

fn field_description() -> FieldDescription {
    FieldDescription {
        annotations: [("acme.icon".to_owned(), serde_json::json!("x"))].into(),
        name: "f".into(),
        ty: field_type(),
        required: true,
        authority: Authority::Source,
        access: Access::Read,
        derived: Some(cond()),
        function: Some("g".into()),
        computed_at: Some(ComputedAt::Read),
        depends_on: vec!["e".into()],
        on_input_change: OnInputChange::Keep,
        initial: Some(serde_json::json!("new")),
        filterable: true,
        searchable: true,
        sensitive: true,
        description: Some("d".into()),
        label: Some("l".into()),
        embed: Some(embed_decl()),
    }
}

fn type_description() -> TypeDescription {
    TypeDescription {
        annotations: [("acme.icon".to_owned(), serde_json::json!("x"))].into(),
        name: "invoice".into(),
        description: Some("d".into()),
        label: Some("l".into()),
        language: Some("de".into()),
        view: Some(view_decl()),
        fields: vec![field_description()],
        relations: vec![relation_decl()],
        access: TypeAccess {
            key: vec!["f".into()],
            valid_from: Some("f".into()),
            valid_to: Some("f".into()),
            searchable: true,
            field_meta: true,
        },
        may: TypeRights {
            link: vec!["r".into()],
            ..TypeRights::default()
        },
        verbs: vec!["pay".into()],
        lineage: TypeLineage {
            sourced: true,
            insert_only: true,
            view_of: Some("t".into()),
        },
        lifecycle: TypeLifecycle {
            version: 1,
            schema_hash: "h".into(),
            sorted: vec!["amount".into()],
            indexed: vec!["embedding".into()],
            kept: Some(Kept::Kept),
        },
    }
}

fn value_renamed() -> ValueRenamed {
    ValueRenamed {
        from: "open".into(),
        to: "active".into(),
        records: 1,
    }
}

fn purge_moved() -> lakelands_api::write::PurgeMoved {
    lakelands_api::write::PurgeMoved {
        ty: "order".into(),
        field: "state".into(),
        records: 1,
        first: vec!["o1".into()],
    }
}

fn purge_reference() -> lakelands_api::write::PurgeReference {
    lakelands_api::write::PurgeReference {
        ty: "bill".into(),
        relation: "billed_to".into(),
        records: 1,
        first: vec![],
    }
}

fn search_mode() -> SearchMode {
    SearchMode::Hybrid {
        text: "t".into(),
        field: "f".into(),
        vector: Some(vec![1.0]),
        fields: Some(vec!["f".into()]),
        weight: None,
        text_weight: None,
    }
}

/// The serde members of a value, as a set of names.
fn members<T: Serialize>(value: &T) -> BTreeSet<String> {
    match serde_json::to_value(value).expect("serialises") {
        Value::Object(m) => m.keys().cloned().collect(),
        other => panic!("{other} is not an object"),
    }
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// What the WIT says a record has, minus `drop` and plus `add`: the
/// members serde flattens or tags, which a WIT record names differently.
fn adjusted(serde: BTreeSet<String>, drop: &[&str], add: &[&str]) -> BTreeSet<String> {
    serde
        .into_iter()
        .filter(|m| !drop.contains(&m.as_str()))
        .chain(add.iter().map(|a| (*a).to_owned()))
        .collect()
}

#[test]
fn every_mirrored_record_has_the_fields_of_its_struct() {
    let wit = wit();
    let at = &["as_of", "valid_at"];
    let checks: Vec<(&str, BTreeSet<String>)> = vec![
        ("read_at", members(&read_at())),
        (
            "principal",
            members(&Principal {
                kind: PrincipalKind::Agent,
                id: "a".into(),
                lake: "l".into(),
                grants: Grants::default(),
            }),
        ),
        (
            "grants",
            members(&Grants {
                default: Access::Read,
                types: [(
                    "invoice".into(),
                    TypeGrants {
                        default: Some(Access::Read),
                        fields: [("f".into(), Access::Write)].into(),
                        only: [("o".into(), json!("a"))].into(),
                    },
                )]
                .into(),
                review: true,
                schema: true,
                ingest: true,
                restricted: true,
            }),
        ),
        (
            "type_grants",
            adjusted(
                members(&TypeGrants {
                    default: Some(Access::Read),
                    fields: [("f".into(), Access::Write)].into(),
                    only: [("o".into(), json!("a"))].into(),
                }),
                &[],
                &["type"],
            ),
        ),
        (
            "write_context",
            members(&WriteContext {
                cause: Some(Position(1)),
                origin: WriteOrigin::Reactive,
                depth: 1,
            }),
        ),
        ("field_type", members(&field_type())),
        ("field_decl", members(&field_decl())),
        ("embed_decl", members(&embed_decl())),
        ("relation_decl", members(&relation_decl())),
        ("measure", members(&measure())),
        ("view_param", members(&view_param())),
        ("view_source", members(&view_source())),
        ("view_decl", members(&view_decl())),
        ("type_decl", members(&type_decl())),
        (
            "value_type_decl",
            members(&ValueTypeDecl {
                name: "money".into(),
                kind: StorageKind::Decimal,
                constraint: Some(cond()),
                one_of: vec!["a".into()],
                format: Some("iban".into()),
                scale: Some(2),
                description: Some("d".into()),
                annotations: [("acme.unit".to_owned(), serde_json::json!("EUR"))].into(),
            }),
        ),
        (
            "assignment",
            members(&Assignment {
                field: "f".into(),
                value: cond(),
            }),
        ),
        (
            "verb_decl",
            adjusted(
                members(&VerbDecl {
                    name: "pay".into(),
                    on: "invoice".into(),
                    scope: VerbScope::Type,
                    payload: vec![field_decl()],
                    risk: Risk::Low,
                    requires: Some(cond()),
                    effect: VerbEffect::External {
                        executor: "x".into(),
                    },
                    description: Some("d".into()),
                }),
                &["executor"],
                &[],
            ),
        ),
        ("trigger_decl", members(&trigger_decl())),
        ("object_set_decl", members(&object_set_decl())),
        ("function_decl", members(&function_decl())),
        (
            "migration",
            members(&Migration {
                renames: vec![("a".into(), "b".into())],
                relation_renames: vec![("r".into(), "s".into())],
                fill: vec!["c".into()],
                ..Migration::to(type_decl())
            }),
        ),
        ("migration_report", members(&migration_report())),
        // A case is flattened into its object: its parameters (here `type`
        // and `field`) are members beside `at`, `code` and `message`, and
        // the WIT carries them as one JSON text, `params`.
        (
            "said",
            adjusted(members(&said()), &["type", "field"], &["params"]),
        ),
        (
            "field_conversion",
            members(&migration_report().fields[0].clone()),
        ),
        (
            "relation_rename",
            members(&migration_report().relations[0].clone()),
        ),
        ("carried", members(&migration_report().carried[0].clone())),
        ("restated", members(&migration_report().restated[0].clone())),
        ("upstream", members(&upstream())),
        ("field_meta", members(&field_meta())),
        ("record", members(&record())),
        (
            "source_record",
            members(&SourceRecord {
                upstream: Some(upstream()),
                ty: "invoice".into(),
                key: "k".into(),
                fields: [("f".into(), json!(1))].into(),
                seen_at: Some(t()),
            }),
        ),
        ("order", members(&order())),
        ("after_row", members(&after_row())),
        ("agg_measure", members(&agg_measure())),
        ("aggregate", members(&aggregate())),
        (
            "find",
            adjusted(
                members(&Find {
                    filter: Some(cond()),
                    order_by: vec![order()],
                    limit: Some(1),
                    cursor: Some(Cursor("c".into())),
                    after: Some(after_row()),
                    offset: Some(7),
                    total: true,
                    retired: true,
                    aggregate: Some(aggregate()),
                    with_meta: true,
                    at: read_at(),
                    params: params(),
                }),
                at,
                &["at"],
            ),
        ),
        (
            "group",
            members(&Group {
                by: [("f".into(), json!(1))].into(),
                values: [("m".into(), json!(1))].into(),
            }),
        ),
        (
            "get",
            adjusted(
                members(&Get {
                    key: "k".into(),
                    with_meta: true,
                    at: read_at(),
                    params: params(),
                }),
                at,
                &["at"],
            ),
        ),
        ("write_opts", members(&write_opts())),
        // The `applied` record is the variant's members beside its tag.
        ("applied", {
            let mut m = members(&Outcome::Applied {
                position: Position(1),
                version: Some(Version(1)),
                output: Some(serde_json::json!({})),
                set_off: lakelands_api::SetOff {
                    through: Some(Position(2)),
                    fired: vec!["t".into()],
                    joined: vec!["s".into()],
                    left: vec!["s".into()],
                },
                renamed: vec![("a".into(), "b".into())],
            });
            m.remove("outcome");
            m
        }),
        (
            "proposal",
            members(&Proposal {
                id: "p".into(),
                position: Position(1),
                proposer: "human:a".into(),
                proposed_at: t(),
                write: Some(ProposedWrite::Retire {
                    ty: "invoice".into(),
                    key: "k".into(),
                    opts: write_opts(),
                }),
                why: "w".into(),
                state: ProposalState::Accepted,
                decided_by: Some("human:b".into()),
                reason: Some("r".into()),
                more: true,
            }),
        ),
        (
            "proposal_filter",
            adjusted(
                members(&ProposalFilter {
                    state: Some(ProposalState::Pending),
                    ty: Some("invoice".into()),
                    kind: Some(lakelands_api::write::ProposalKind::Declare),
                    limit: Some(1),
                    order_by: vec![order()],
                    cursor: Some(Cursor("c".into())),
                    at: read_at(),
                }),
                at,
                &["at"],
            ),
        ),
        (
            "committed",
            members(&Committed {
                position: Position(1),
                records: 1,
                duplicates: 0,
            }),
        ),
        ("depth", members(&Depth { min: 1, max: 2 })),
        (
            "vector_term",
            members(&VectorTerm {
                field: "f".into(),
                vector: vec![1.0],
                weight: Some(2.0),
            }),
        ),
        (
            "start",
            members(&Start {
                ty: Some("invoice".into()),
                key: Some("k".into()),
                filter: Some(cond()),
                search: Some(search_mode()),
                params: params(),
                named: Some("start".into()),
            }),
        ),
        (
            "step",
            members(&Step {
                relation: "r".into(),
                walk: Walk::Both,
                ty: Some("partner".into()),
                filter: Some(cond()),
                depth: Depth { min: 1, max: 2 },
                named: Some("by".into()),
            }),
        ),
        (
            "pattern",
            adjusted(
                members(&Pattern {
                    start: Start {
                        ty: Some("invoice".into()),
                        key: None,
                        filter: None,
                        search: None,
                        params: Default::default(),
                        named: None,
                    },
                    steps: vec![],
                    to: Some(("partner".into(), "k".into())),
                    project: Projection::Graph,
                    order_by: vec![order()],
                    limit: Some(1),
                    cursor: Some(Cursor("c".into())),
                    at: read_at(),
                }),
                at,
                &["at", "steps"],
            ),
        ),
        (
            "edge",
            members(&Edge {
                from: ("invoice".into(), "k".into()),
                relation: "r".into(),
                to: ("partner".into(), "k".into()),
                fields: [("f".into(), json!(1))].into(),
            }),
        ),
        (
            "search",
            adjusted(
                members(&Search {
                    ty: "invoice".into(),
                    mode: search_mode(),
                    k: 5,
                    cursor: Some(Cursor("c".into())),
                    filter: Some(cond()),
                    with_meta: true,
                    at: read_at(),
                }),
                &["as_of", "valid_at", "text", "field", "vector", "fields"],
                &["at"],
            ),
        ),
        (
            "hit",
            members(&Hit {
                record: record(),
                score: 1.0,
            }),
        ),
        (
            "hits",
            members(&Hits {
                hits: vec![],
                next_cursor: Some(Cursor("c".into())),
                ranked_by: "lexical".into(),
                axes: vec![],
            }),
        ),
        (
            "axis",
            members(&Axis {
                axis: "vector".into(),
                field: Some("v".into()),
                candidates: Some(5),
                weight: Some(0.5),
            }),
        ),
        (
            "lake_rights",
            members(&LakeRights {
                review: true,
                schema: true,
                ingest: true,
            }),
        ),
        (
            "type_rights",
            members(&TypeRights {
                link: vec!["r".into()],
                only: [("o".into(), json!("a"))].into(),
                ..TypeRights::default()
            }),
        ),
        ("type_access", members(&type_description().access)),
        ("type_lineage", members(&type_description().lineage)),
        ("type_lifecycle", members(&type_description().lifecycle)),
        ("field_description", members(&field_description())),
        ("type_description", members(&type_description())),
        (
            "verb_description",
            members(&VerbDescription {
                name: "pay".into(),
                on: "invoice".into(),
                scope: VerbScope::Type,
                payload: vec![field_description()],
                risk: Risk::High,
                external: true,
                executor: Some(ExecutorDescription {
                    name: "ledger".into(),
                    bound: true,
                }),
                description: Some("d".into()),
            }),
        ),
        (
            "executor_description",
            members(&ExecutorDescription {
                name: "ledger".into(),
                bound: true,
            }),
        ),
        (
            "lake_description",
            members(&LakeDescription {
                lake: "l".into(),
                types: vec![type_description()],
                verbs: vec![VerbDescription {
                    name: "pay".into(),
                    on: "invoice".into(),
                    scope: VerbScope::Record,
                    payload: vec![],
                    risk: Risk::Low,
                    external: false,
                    executor: None,
                    description: None,
                }],
                value_types: vec![ValueTypeDecl {
                    name: "money".into(),
                    kind: StorageKind::Decimal,
                    scale: Some(2),
                    ..Default::default()
                }],
                functions: vec![function_decl()],
                triggers: vec![trigger_decl()],
                object_sets: vec![object_set_decl()],
                may: LakeRights::default(),
            }),
        ),
        (
            "verification",
            members(&Verification {
                from: Position(0),
                through: Position(1),
                hash: lakelands_api::event::Digest::zero(),
            }),
        ),
        (
            "age_report",
            members(&AgeReport {
                date: t(),
                rules: vec![],
                transitions: 1,
                failed_guards: 2,
                position: Position(3),
            }),
        ),
        (
            "rule_run",
            members(&RuleRun {
                rule: "r".into(),
                version: Version(1),
                moved: 1,
                failed_guards: 2,
                skipped: Some("s".into()),
            }),
        ),
        (
            "rekeyed",
            members(&Rekeyed {
                ty: "invoice".into(),
                records: 1,
                collisions: 0,
                missing: 0,
            }),
        ),
        (
            "reference_rewrite",
            members(&ReferenceRewrite {
                ty: "payment".into(),
                field: "invoice".into(),
                values: 1,
            }),
        ),
        (
            "value_type_report",
            members(&ValueTypeReport {
                value_type: "money".into(),
                from_version: 1,
                records_scanned: 2,
                fields: vec![],
                misfits: 1,
                first: vec![Misfit {
                    ty: "invoice".into(),
                    key: "i1".into(),
                    field: "net".into(),
                }],
                fits: false,
                report_id: "r".into(),
                conditions: vec!["the object_set declaration `open`".into()],
                conditions_unshown: true,
            }),
        ),
        (
            "value_type_use",
            members(&ValueTypeUse {
                ty: "invoice".into(),
                field: "net".into(),
                values_scanned: 2,
                misfits: 1,
                renamed: vec![value_renamed()],
            }),
        ),
        ("value_renamed", members(&value_renamed())),
        (
            "value_type_migration",
            members(&ValueTypeMigration {
                to: serde_json::from_value(json!({"name": "v", "kind": "text"})).unwrap(),
                renames: vec![("open".into(), "active".into())],
            }),
        ),
        (
            "misfit",
            members(&Misfit {
                ty: "invoice".into(),
                key: "i1".into(),
                field: "net".into(),
            }),
        ),
        (
            "purge_report",
            members(&PurgeReport {
                source: "erp".into(),
                ty: Some("ord".into()),
                retired: 1,
                types: vec![],
                unsearched: vec![],
                position: Position(3),
                references: vec![purge_reference()],
                moved: vec![purge_moved()],
                report_id: "r".into(),
            }),
        ),
        ("purge_reference", members(&purge_reference())),
        ("purge_moved", members(&purge_moved())),
        (
            "vector_coverage",
            members(&lakelands_api::read::VectorCoverage {
                field: "vec".into(),
                embedded: 3,
                due: 1,
            }),
        ),
        (
            "compact_report",
            members(&CompactReport {
                before: Some(2),
                after: Some(1),
                head: Position(3),
            }),
        ),
        (
            "type_retired",
            members(&TypeRetired {
                ty: "invoice".into(),
                retired: 1,
            }),
        ),
        (
            "limits",
            members(&Limits {
                default_limit: 1,
                max_limit: 2,
                max_depth: 3,
                max_reactive_depth: 4,
                commit_window_events: 5,
                max_graph_nodes: 6,
                max_offset: 7,
                max_set_changes: 8,
            }),
        ),
        (
            "feed_filter",
            members(&FeedFilter {
                kinds: vec!["k".into()],
                types: vec!["invoice".into()],
                keys: vec!["INV-1".into()],
            }),
        ),
        (
            "feed_batch",
            members(&FeedBatch {
                events: vec![],
                next: Position(1),
                head: Position(1),
            }),
        ),
    ];
    // serde leaves an empty list out; a sample that keeps one empty names
    // it in `adjusted`, so the comparison still covers it.
    let mut wrong = Vec::new();
    for (name, serde) in checks {
        match wit.records.get(name) {
            None => wrong.push(format!("the WIT has no record `{name}`")),
            Some(fields) if *fields != serde => wrong.push(format!(
                "`{name}`: the WIT has {fields:?}, the crate {serde:?}"
            )),
            Some(_) => {}
        }
    }
    assert!(wrong.is_empty(), "records drifted:\n{}", wrong.join("\n"));
}

// ----------------------------------------------------------------------
// The cases
// ----------------------------------------------------------------------

/// The names serde gives values: the string of a unit variant, or the
/// member `tag` of an internally tagged one.
fn case_names<T: Serialize>(values: &[T], tag: Option<&str>) -> BTreeSet<String> {
    values
        .iter()
        .map(
            |v| match (serde_json::to_value(v).expect("serialises"), tag) {
                (Value::String(s), _) => s,
                (Value::Object(m), Some(tag)) => m[tag].as_str().expect("a tag").to_owned(),
                (other, _) => panic!("{other} has no case name"),
            },
        )
        .collect()
}

#[test]
fn every_mirrored_enum_has_the_cases_of_its_type() {
    let wit = wit();
    let mut checks: Vec<(&str, BTreeSet<String>)> = Vec::new();

    checks.push(("storage_kind", case_names(&StorageKind::ALL, None)));
    macro_rules! units {
        ($wit:literal, $ty:ident, [$($v:ident),+ $(,)?]) => {{
            // Exhaustive: a new variant stops this compiling.
            fn guard(x: $ty) { match x { $($ty::$v)|+ => {} } }
            let _ = guard;
            checks.push(($wit, case_names(&[$($ty::$v),+], None)));
        }};
    }
    units!("authority", Authority, [Authored, Source, Shared, Derived]);
    units!("on_input_change", OnInputChange, [Keep, Clear]);
    units!("computed_at", ComputedAt, [Write, Read]);
    units!(
        "cardinality",
        Cardinality,
        [OneToOne, OneToMany, ManyToOne, ManyToMany]
    );
    units!("risk", Risk, [Low, Medium, High]);
    units!("verb_scope", VerbScope, [Record, Type]);
    units!(
        "origin",
        Origin,
        [Source, Authored, Derived, System, Initial]
    );
    units!("direction", Direction, [Asc, Desc]);
    units!("order_by", OrderBy, [Value, KnownAt, SeenAt, Latest]);
    units!("agg_func", AggFunc, [Count, Sum, Min, Max, Avg]);
    units!(
        "proposal_state",
        ProposalState,
        [Pending, Accepted, Rejected]
    );
    units!(
        "principal_kind",
        PrincipalKind,
        [Human, Agent, Service, System]
    );
    units!("access", Access, [None, Masked, Read, Write, WriteOnly]);
    units!("write_origin", WriteOrigin, [Direct, Reactive, Scheduled]);
    units!("walk", Walk, [Out, In, Both]);
    units!("carrier", Carrier, [Via, Links, Derived]);

    fn kept(k: &Kept) -> [NotKept; 5] {
        match k {
            Kept::Kept | Kept::Computed { .. } => {}
        }
        let (reason, not_exact) = (None, 0);
        let all = [
            NotKept::NotAsked,
            NotKept::CannotBeKept { reason },
            NotKept::NotWhole { not_exact },
            NotKept::NotKeptHere,
            NotKept::OnlySome,
        ];
        for why in &all {
            match why {
                NotKept::NotAsked | NotKept::CannotBeKept { .. } => {}
                NotKept::NotWhole { .. } | NotKept::NotKeptHere | NotKept::OnlySome => {}
            }
        }
        all
    }
    let why = kept(&Kept::Kept);
    checks.push(("not_kept", case_names(&why, Some("because"))));
    let [why, ..] = why;
    let states = [Kept::Kept, Kept::Computed { why }];
    checks.push(("kept", case_names(&states, Some("state"))));

    fn outcomes(o: &Outcome) {
        match o {
            Outcome::Applied { .. } | Outcome::Proposed { .. } => {}
        }
    }
    let _ = outcomes;
    checks.push((
        "outcome",
        case_names(
            &[
                Outcome::Applied {
                    position: Position(1),
                    version: None,
                    output: None,
                    set_off: Default::default(),
                    renamed: Vec::new(),
                },
                Outcome::Proposed {
                    position: Position(1),
                    proposal: "p".into(),
                    why: "w".into(),
                },
            ],
            Some("outcome"),
        ),
    ));

    fn errors(e: &Error) {
        match e {
            Error::Refused { .. }
            | Error::NotFound { .. }
            | Error::Conflict { .. }
            | Error::Unbound { .. }
            | Error::Unavailable { .. } => {}
            // A refused record of an ingest: the WIT's `ingest` answers it
            // as an `ingest-error`, its places beside its refusal
            // (lakelands#1043).
            Error::Ingested { .. } => {}
            // A newer lake's kind, which a component built against this
            // WIT cannot be sent.
            Error::Unknown(_) => {}
        }
    }
    let _ = errors;
    let refused = |reason| Error::Refused {
        field: None,
        reason,
    };
    checks.push((
        "error",
        case_names(
            &[
                refused(Refusal::Decided),
                Error::NotFound {
                    missing: Missing::Verb { name: "v".into() },
                },
                Error::Conflict {
                    expected: Version(1),
                    actual: Version(2),
                },
                Error::unbound(lakelands_api::error::Seam::VectorIndex),
                Error::unavailable(lakelands_api::error::Situation::Moved),
            ],
            Some("error"),
        ),
    ));

    fn refusals(r: &Refusal) {
        match r {
            Refusal::NotGranted
            | Refusal::WrongDoor
            | Refusal::Reserved
            | Refusal::Invalid { .. }
            | Refusal::Constraint { .. }
            | Refusal::Precondition { .. }
            | Refusal::TooDeep { .. }
            | Refusal::TooLarge { .. }
            | Refusal::Migration { .. }
            | Refusal::Decided
            | Refusal::Unanswered { .. }
            | Refusal::OutOfRange { .. } => {}
            // A newer lake's refusal, which a component built against this
            // WIT cannot be sent.
            Refusal::Unknown(_) => {}
        }
    }
    let _ = refusals;
    checks.push((
        "refusal",
        case_names(
            &[
                Refusal::NotGranted,
                Refusal::WrongDoor,
                Refusal::Reserved,
                Refusal::Invalid(said()),
                Refusal::Constraint(said()),
                Refusal::Precondition(said()),
                Refusal::TooDeep { max: 1 },
                Refusal::TooLarge { max: 1 },
                Refusal::Migration {
                    report: Box::new(migration_report()),
                },
                Refusal::Decided,
                Refusal::Unanswered(said()),
                Refusal::OutOfRange { head: Position(1) },
            ],
            Some("refusal"),
        ),
    ));

    fn missings(m: &Missing) {
        match m {
            Missing::Type { .. }
            | Missing::Record { .. }
            | Missing::Verb { .. }
            | Missing::Function { .. }
            | Missing::Proposal { .. } => {}
            // A newer lake's kind, said by its error's message.
            Missing::Unknown(_) => {}
        }
    }
    let _ = missings;
    checks.push((
        "missing",
        case_names(
            &[
                Missing::Type { name: "t".into() },
                Missing::Record {
                    ty: "t".into(),
                    key: "k".into(),
                },
                Missing::Verb { name: "v".into() },
                Missing::Function { name: "f".into() },
                Missing::Proposal { id: "p".into() },
            ],
            Some("kind"),
        ),
    ));

    fn writes(w: &ProposedWrite) {
        match w {
            ProposedWrite::Upsert { .. }
            | ProposedWrite::Retire { .. }
            | ProposedWrite::Call { .. }
            | ProposedWrite::Declare { .. }
            | ProposedWrite::Migrate { .. }
            | ProposedWrite::MigrateValueType { .. } => {}
        }
    }
    let _ = writes;
    checks.push((
        "proposed_write",
        case_names(
            &[
                ProposedWrite::Upsert {
                    ty: "t".into(),
                    key: "k".into(),
                    patch: Default::default(),
                    opts: WriteOpts::default(),
                },
                ProposedWrite::Retire {
                    ty: "t".into(),
                    key: "k".into(),
                    opts: WriteOpts::default(),
                },
                ProposedWrite::Call {
                    verb: "v".into(),
                    key: Some("k".into()),
                    payload: Default::default(),
                    opts: WriteOpts::default(),
                },
                ProposedWrite::Declare {
                    declarations: vec![],
                },
                ProposedWrite::Migrate {
                    migration: Box::new(lakelands_api::write::Migration::to(
                        serde_json::from_value(json!({"name": "t"})).unwrap(),
                    )),
                    report: Box::new(migration_report()),
                },
                ProposedWrite::MigrateValueType {
                    to: serde_json::from_value(json!({"name": "v", "kind": "text"})).unwrap(),
                    renames: Vec::new(),
                    report: Default::default(),
                },
            ],
            Some("write"),
        ),
    ));

    fn founds(f: &Found) {
        match f {
            Found::Records { .. } | Found::Groups { .. } => {}
        }
    }
    let _ = founds;
    checks.push((
        "found",
        case_names(
            &[
                Found::Records {
                    rows: vec![],
                    next_cursor: None,
                    total: Some(3),
                    retired: Some(1),
                },
                Found::Groups { rows: vec![] },
            ],
            Some("shape"),
        ),
    ));

    fn results(q: &QueryResult) {
        match q {
            QueryResult::Rows { .. } | QueryResult::Graph { .. } | QueryResult::Groups { .. } => {}
        }
    }
    let _ = results;
    checks.push((
        "query_result",
        case_names(
            &[
                QueryResult::Rows {
                    rows: vec![],
                    next_cursor: None,
                },
                QueryResult::Graph {
                    nodes: vec![],
                    depths: vec![],
                    edges: vec![],
                    truncated: false,
                    next_cursor: None,
                },
                QueryResult::Groups { rows: vec![] },
            ],
            Some("shape"),
        ),
    ));

    fn projections(p: &Projection) {
        match p {
            Projection::Rows { .. }
            | Projection::Graph
            | Projection::Aggregate(_)
            | Projection::Degree
            | Projection::Components
            | Projection::Reach => {}
        }
    }
    let _ = projections;
    checks.push((
        "projection",
        case_names(
            &[
                Projection::Rows { fields: vec![] },
                Projection::Graph,
                Projection::Aggregate(aggregate()),
                Projection::Degree,
                Projection::Components,
                Projection::Reach,
            ],
            Some("as"),
        ),
    ));

    fn modes(m: &SearchMode) {
        match m {
            SearchMode::Lexical { .. } | SearchMode::Vector { .. } | SearchMode::Hybrid { .. } => {}
        }
    }
    let _ = modes;
    checks.push((
        "search_mode",
        case_names(
            &[
                SearchMode::Lexical {
                    text: "t".into(),
                    fields: Some(vec!["f".into()]),
                },
                SearchMode::Vector {
                    field: "f".into(),
                    vector: vec![],
                    weight: None,
                    also: Vec::new(),
                },
                search_mode(),
            ],
            Some("mode"),
        ),
    ));

    fn effects(e: &VerbEffect) {
        match e {
            VerbEffect::Write { .. } | VerbEffect::External { .. } => {}
        }
    }
    let _ = effects;
    checks.push((
        "verb_effect",
        case_names(
            &[
                VerbEffect::Write { set: vec![] },
                VerbEffect::External {
                    executor: "x".into(),
                },
            ],
            Some("effect"),
        ),
    ));

    fn declarations(d: &Declaration) {
        match d {
            Declaration::Type(_)
            | Declaration::ValueType(_)
            | Declaration::Verb(_)
            | Declaration::Trigger(_)
            | Declaration::ObjectSet(_)
            | Declaration::Function(_) => {}
        }
    }
    let _ = declarations;
    let value_type = ValueTypeDecl {
        name: "v".into(),
        ..Default::default()
    };
    checks.push((
        "declaration",
        case_names(
            &[
                Declaration::Type(type_decl()),
                Declaration::ValueType(value_type),
                Declaration::Verb(VerbDecl {
                    name: "v".into(),
                    on: "t".into(),
                    scope: VerbScope::Record,
                    payload: vec![],
                    risk: Risk::Low,
                    requires: None,
                    effect: VerbEffect::Write { set: vec![] },
                    description: None,
                }),
                Declaration::Trigger(TriggerDecl {
                    max_depth: Some(2),
                    name: "t".into(),
                    on: "t".into(),
                    when: None,
                    description: None,
                }),
                Declaration::ObjectSet(ObjectSetDecl {
                    name: "s".into(),
                    of: "t".into(),
                    filter: cond(),
                    description: None,
                }),
                Declaration::Function(FunctionDecl {
                    name: "g".into(),
                    backing: "expr".into(),
                    inputs: vec![],
                    expr: None,
                    config: Value::Null,
                    description: None,
                }),
            ],
            Some("declare"),
        ),
    ));

    // A condition is untagged in the crate: its text form is a string,
    // its typed form an object. The WIT names the two forms.
    fn conditions(c: &Condition) {
        match c {
            Condition::Text(_) | Condition::Expr(_) => {}
        }
    }
    let _ = conditions;
    checks.push(("condition", set(&["text", "typed"])));

    let mut wrong = Vec::new();
    for (name, rust) in checks {
        match wit.cases.get(name) {
            None => wrong.push(format!("the WIT has no enum or variant `{name}`")),
            Some(cases) if *cases != rust => wrong.push(format!(
                "`{name}`: the WIT has {cases:?}, the crate {rust:?}"
            )),
            Some(_) => {}
        }
    }
    assert!(wrong.is_empty(), "cases drifted:\n{}", wrong.join("\n"));
}
