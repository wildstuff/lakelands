//! A relation derived from matching values
//! (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
//!
//! A derived relation stores no edge. A walk connects the records whose
//! matched fields hold the same non-null value, as the read's position
//! holds them, both ways and in every walk the store runs.

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::ontology::{
    Cardinality, Derived, MatchPair, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Start, Step, Walk};
use lakelands_api::read::ReadAt;
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{
    Access, Catalog, Grants, LakeStore, Position, Principal, PrincipalKind, Refusal, TypeName,
    Value,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, invalid_naming, owner, patch, principal, reason, type_decl, upsert,
};

check!(
    /// A walk finds exactly the matching records, both ways.
    BOTH_WAYS = a_walk_over_a_derived_relation_finds_exactly_the_matching_records_both_ways,
    "044: a pattern step over a derived relation reaches exactly the records whose matched fields hold the same value, out and in, over one pair and over two, at the head and as_of an earlier position; `'010'` does not match `'10'`"
);

check!(
    /// A changed matched field moves the edge where it changed.
    EDGE_MOVES = changing_a_matched_field_moves_the_edge_at_that_position,
    "044 and 009: changing a matched field moves the edge at the position of that write: as_of before it the old record is reached, from it on the new one"
);

check!(
    /// A null never matches.
    NULL_NEVER_MATCHES = a_null_never_matches,
    "044: a matched field that is null or absent on either side connects nothing, a null to a null included"
);

check!(
    /// A walk probes the matched fields on both sides.
    PROBES_BOTH_SIDES = a_principal_without_read_on_a_matched_field_is_refused_naming_it,
    "024 and 044: a walk over a derived relation, a step and a measure alike, is refused not_granted, naming the field, when the principal may not read a matched field on either side"
);

check!(
    /// Measures count derived edges as they count links.
    MEASURES_AS_LINKS = degree_and_components_over_a_derived_relation_equal_those_over_the_same_links,
    "026 and 044: `degree` and `components` over a derived relation answer what they answer over the same connections written as link records"
);

check!(
    /// A derived relation is declared, and never changes carrier.
    DECLARED_NOT_MIGRATED = a_derived_relation_is_declared_not_migrated_and_never_moves_carrier,
    "007, 027, 035 and 044: changing a derived relation's match is taken by `declare` and leaves the schema hash byte-identical; a relation carried by link records or by `via` is not redeclared as derived, nor a derived one as carried, each refused naming the relation"
);

check!(
    /// A pair of two kinds is refused.
    PAIR_OF_TWO_KINDS = a_match_pair_of_two_kinds_is_refused_naming_both,
    "044: `declare` refuses a match pair whose fields are of different storage kinds, naming both fields, since identity would never match; a derived relation with `via` or fields of its own is refused too"
);

fn pair(from: &str, to: &str) -> MatchPair {
    MatchPair {
        from: from.into(),
        to: to.into(),
    }
}

fn derived(name: &str, target: &str, pairs: Vec<MatchPair>) -> RelationDecl {
    RelationDecl {
        cardinality: Cardinality::ManyToOne,
        derived: Some(Derived { pairs }),
        ..RelationDecl::to(name, target)
    }
}

fn customer() -> TypeDecl {
    let mut t = type_decl(
        "customer",
        vec![
            field("no", StorageKind::Text),
            field("region", StorageKind::Text),
        ],
    );
    t.key = vec!["no".into()];
    t
}

fn invoice() -> TypeDecl {
    let mut t = type_decl(
        "invoice",
        vec![
            field("customer_no", StorageKind::Text),
            field("region", StorageKind::Text),
            field("amount", StorageKind::Number),
        ],
    );
    t.relations = vec![
        derived("customer", "customer", vec![pair("customer_no", "no")]),
        derived(
            "local_customer",
            "customer",
            vec![pair("customer_no", "no"), pair("region", "region")],
        ),
        derived("same_region", "customer", vec![pair("region", "region")]),
    ];
    t
}

/// Customers 10, 010 and 20; invoices i1 (10, north), i2 (10, south),
/// i3 (010), i4 (no customer, no region) and i5 (20, north). Answers the
/// position before i2 was written.
async fn lake(store: &dyn LakeStore) -> Result<Position, Failure> {
    let cat = doors(store);
    let own = owner(store);
    declare(cat.as_ref(), &own, customer()).await?;
    declare(cat.as_ref(), &own, invoice()).await?;
    for (no, region) in [("10", "north"), ("010", "north"), ("20", "south")] {
        upsert(
            cat.as_ref(),
            &own,
            "customer",
            no,
            json!({"no": no, "region": region}),
        )
        .await?;
    }
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"customer_no": "10", "region": "north"}),
    )
    .await?;
    let before = store.head().await.must("reading the head")?;
    for (key, fields) in [
        ("i2", json!({"customer_no": "10", "region": "south"})),
        ("i3", json!({"customer_no": "010"})),
        ("i4", json!({"amount": 1})),
        ("i5", json!({"customer_no": "20", "region": "north"})),
    ] {
        upsert(cat.as_ref(), &own, "invoice", key, fields).await?;
    }
    Ok(before)
}

fn step(relation: &str, walk: Walk) -> Step {
    Step {
        relation: relation.into(),
        walk,
        ty: None,
        filter: None,
        depth: Depth::default(),
    }
}

fn walk(ty: &str, key: &str, relation: &str, dir: Walk, as_of: Option<Position>) -> Pattern {
    Pattern {
        start: Start {
            ty: ty.into(),
            key: Some(key.into()),
            filter: None,
            search: None,
            params: Default::default(),
        },
        steps: vec![step(relation, dir)],
        to: None,
        project: Projection::Rows { fields: vec![] },
        order_by: Vec::new(),
        limit: None,
        cursor: None,
        at: ReadAt {
            as_of,
            valid_at: None,
        },
    }
}

/// The keys of `far` a walk reaches, sorted.
async fn reached(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    far: &str,
) -> Result<Vec<String>, Failure> {
    let what = format!("walking {} from {:?}", p.steps[0].relation, p.start.key);
    let reply = cat.query(who, p).await.must(&what)?;
    let QueryResult::Rows { rows, .. } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    let mut keys: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get(&format!("{far}.key")).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// A walk to check: its start type and key, relation, direction, `as_of`,
/// the far type, and the far keys it must reach.
type Case<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Walk,
    Option<Position>,
    &'a str,
    Vec<&'a str>,
);

async fn a_walk_over_a_derived_relation_finds_exactly_the_matching_records_both_ways(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let before = lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let cases: Vec<Case> = vec![
        (
            "invoice",
            "i1",
            "customer",
            Walk::Out,
            None,
            "customer",
            vec!["10"],
        ),
        (
            "invoice",
            "i3",
            "customer",
            Walk::Out,
            None,
            "customer",
            vec!["010"],
        ),
        (
            "customer",
            "10",
            "customer",
            Walk::In,
            None,
            "invoice",
            vec!["i1", "i2"],
        ),
        (
            "customer",
            "010",
            "customer",
            Walk::In,
            None,
            "invoice",
            vec!["i3"],
        ),
        (
            "customer",
            "10",
            "customer",
            Walk::In,
            Some(before),
            "invoice",
            vec!["i1"],
        ),
        (
            "invoice",
            "i1",
            "local_customer",
            Walk::Out,
            None,
            "customer",
            vec!["10"],
        ),
        (
            "invoice",
            "i2",
            "local_customer",
            Walk::Out,
            None,
            "customer",
            vec![],
        ),
        (
            "customer",
            "10",
            "local_customer",
            Walk::In,
            None,
            "invoice",
            vec!["i1"],
        ),
    ];
    for (ty, key, relation, dir, as_of, far, want) in cases {
        let got = reached(cat.as_ref(), &own, walk(ty, key, relation, dir, as_of), far).await?;
        ensure!(
            got == want,
            "walking `{relation}` {dir:?} from {ty} {key} as_of {as_of:?} must reach {want:?}; \
             it reached {got:?}"
        );
    }
    Ok(())
}

async fn changing_a_matched_field_moves_the_edge_at_that_position(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    cat.upsert(
        &own,
        &"invoice".into(),
        &"i1".into(),
        patch(json!({"customer_no": "20"})),
        WriteOpts::default(),
    )
    .await
    .must("moving i1 to customer 20")?;
    for (as_of, want) in [(Some(before), vec!["10"]), (None, vec!["20"])] {
        let got = reached(
            cat.as_ref(),
            &own,
            walk("invoice", "i1", "customer", Walk::Out, as_of),
            "customer",
        )
        .await?;
        ensure!(
            got == want,
            "i1's customer as_of {as_of:?} must be {want:?}; the walk reached {got:?}"
        );
    }
    let got = reached(
        cat.as_ref(),
        &own,
        walk("customer", "20", "customer", Walk::In, None),
        "invoice",
    )
    .await?;
    ensure!(
        got == ["i1", "i5"],
        "customer 20 must be reached from i1 and i5 after the move; it was from {got:?}"
    );
    Ok(())
}

async fn a_null_never_matches(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // A customer without a region, and i4 without either field.
    upsert(cat.as_ref(), &own, "customer", "30", json!({"no": "30"})).await?;
    for (relation, want) in [("customer", "customer"), ("same_region", "customer")] {
        let got = reached(
            cat.as_ref(),
            &own,
            walk("invoice", "i4", relation, Walk::Out, None),
            want,
        )
        .await?;
        ensure!(
            got.is_empty(),
            "i4 holds no matched value, so `{relation}` must reach nothing; it reached {got:?}"
        );
    }
    let got = reached(
        cat.as_ref(),
        &own,
        walk("customer", "30", "same_region", Walk::In, None),
        "invoice",
    )
    .await?;
    ensure!(
        got.is_empty(),
        "a null region never matches a null region; customer 30 was reached from {got:?}"
    );
    Ok(())
}

async fn a_principal_without_read_on_a_matched_field_is_refused_naming_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let blind = |ty: &str, f: &str| {
        let mut types = BTreeMap::new();
        types.insert(
            TypeName::from(ty),
            TypeGrants {
                default: None,
                fields: [(f.into(), Access::None)].into(),
            },
        );
        principal(
            store.as_ref(),
            PrincipalKind::Agent,
            "blind",
            Grants {
                default: Access::Read,
                types,
                ..Grants::default()
            },
        )
    };
    let degree = Pattern {
        start: Start {
            ty: "customer".into(),
            key: None,
            filter: None,
            search: None,
            params: Default::default(),
        },
        steps: vec![step("local_customer", Walk::In)],
        to: None,
        project: Projection::Degree,
        order_by: Vec::new(),
        limit: None,
        cursor: None,
        at: ReadAt::default(),
    };
    let cases = [
        (
            blind("invoice", "customer_no"),
            walk("invoice", "i1", "customer", Walk::Out, None),
            "customer_no",
        ),
        (
            blind("customer", "region"),
            walk("invoice", "i1", "local_customer", Walk::Out, None),
            "region",
        ),
        (blind("invoice", "region"), degree, "region"),
    ];
    for (who, p, field) in cases {
        let what = format!("{:?} over `{}`", p.project, p.steps[0].relation);
        match cat.query(&who, p).await {
            Err(e) if matches!(reason(&e), Some(Refusal::NotGranted)) => {
                let why = e.to_string();
                ensure!(
                    why.contains(field),
                    "{what} must be refused naming `{field}`; it said: {why}"
                );
            }
            other => {
                return Err(Failure::new(format!(
                    "{what} without read on `{field}` must be refused not_granted; it answered \
                     {other:?}"
                )));
            }
        }
    }
    Ok(())
}

async fn degree_and_components_over_a_derived_relation_equal_those_over_the_same_links(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // The same connections, written as link records between twin types.
    declare(
        cat.as_ref(),
        &own,
        type_decl("client", vec![field("no", StorageKind::Text)]),
    )
    .await?;
    let mut bill = type_decl("bill", vec![field("note", StorageKind::Text)]);
    bill.relations = vec![RelationDecl {
        cardinality: Cardinality::ManyToOne,
        ..RelationDecl::to("customer", "client")
    }];
    declare(cat.as_ref(), &own, bill).await?;
    for c in ["10", "010", "20"] {
        upsert(cat.as_ref(), &own, "client", c, json!({"no": c})).await?;
    }
    for (b, c) in [("i1", "10"), ("i2", "10"), ("i3", "010"), ("i5", "20")] {
        upsert(cat.as_ref(), &own, "bill", b, json!({"note": b})).await?;
        cat.link(
            &own,
            &"bill".into(),
            &b.into(),
            &"customer".into(),
            &c.into(),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await
        .must(&format!("linking {b} to {c}"))?;
    }
    upsert(cat.as_ref(), &own, "bill", "i4", json!({"note": "i4"})).await?;
    let measure = |ty: &str, project: Projection, walk: Walk| Pattern {
        start: Start {
            ty: ty.into(),
            key: None,
            filter: None,
            search: None,
            params: Default::default(),
        },
        steps: vec![step("customer", walk)],
        to: None,
        project,
        order_by: Vec::new(),
        limit: None,
        cursor: None,
        at: ReadAt::default(),
    };
    let rows = |r: QueryResult| -> Vec<String> {
        match r {
            QueryResult::Rows { rows, .. } => rows
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    row.remove("type");
                    serde_json::to_string(&row)
                        .expect("a row serialises")
                        .replace("\"client\"", "\"customer\"")
                        .replace("\"bill\"", "\"invoice\"")
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    for (project, walk_dir) in [
        (Projection::Degree, Walk::In),
        (Projection::Components, Walk::Both),
    ] {
        let derived = cat
            .query(&own, measure("customer", project.clone(), walk_dir))
            .await
            .must(&format!("{project:?} over the derived relation"))?
            .data;
        let linked = cat
            .query(&own, measure("client", project.clone(), walk_dir))
            .await
            .must(&format!("{project:?} over the links"))?
            .data;
        // Members sort by (type, key), and the twin types sort otherwise:
        // the rows are compared as sets within their components.
        let (mut d, mut l) = (rows(derived), rows(linked));
        d.sort();
        l.sort();
        ensure!(
            d == l,
            "{project:?} over the derived relation must answer what it answers over the same \
             links; derived {d:?}, links {l:?}"
        );
    }
    Ok(())
}

async fn a_derived_relation_is_declared_not_migrated_and_never_moves_carrier(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let hash = |d: lakelands_api::describe::TypeDescription| d.lifecycle.schema_hash;
    let before = hash(
        cat.describe_type(&own, &"invoice".into(), ReadAt::default())
            .await
            .must("describing invoice")?
            .data,
    );
    let mut changed = invoice();
    changed.relations[2] = derived(
        "same_region",
        "customer",
        vec![pair("region", "region"), pair("customer_no", "no")],
    );
    cat.declare(&own, Declaration::Type(changed.clone()))
        .await
        .must("declaring invoice with a changed match")?;
    let after = hash(
        cat.describe_type(&own, &"invoice".into(), ReadAt::default())
            .await
            .must("describing invoice again")?
            .data,
    );
    ensure!(
        before == after,
        "changing a derived relation's match must leave the schema hash byte-identical; it \
         went from {before} to {after}"
    );
    // Carried by links, then declared derived; derived, then carried.
    let mut linked = invoice();
    linked.name = "order".into();
    linked.relations = vec![RelationDecl::to("customer", "customer")];
    cat.declare(&own, Declaration::Type(linked.clone()))
        .await
        .must("declaring order with a linked relation")?;
    let mut to_derived = linked.clone();
    to_derived.relations = vec![derived(
        "customer",
        "customer",
        vec![pair("customer_no", "no")],
    )];
    let mut to_links = changed.clone();
    to_links.relations[0] = RelationDecl::to("customer", "customer");
    for (what, t) in [
        ("a linked relation redeclared as derived", to_derived),
        (
            "a derived relation redeclared as carried by links",
            to_links,
        ),
    ] {
        let head = store.head().await.must("reading the head")?;
        invalid_naming(
            what,
            cat.declare(&own, Declaration::Type(t)).await,
            &["customer", "carrier"],
        )?;
        crate::conformance::fixture::head_unmoved(store.as_ref(), head, what).await?;
    }
    Ok(())
}

async fn a_match_pair_of_two_kinds_is_refused_naming_both(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, customer()).await?;
    let mut numbered = customer();
    numbered.name = "account".into();
    numbered.fields.push(field("number", StorageKind::Number));
    declare(cat.as_ref(), &own, numbered).await?;
    let base = || {
        type_decl(
            "invoice",
            vec![
                field("customer_no", StorageKind::Text),
                field("account_no", StorageKind::Text),
            ],
        )
    };
    let mut two_kinds = base();
    two_kinds.relations = vec![derived(
        "account",
        "account",
        vec![pair("account_no", "number")],
    )];
    let mut with_via = base();
    with_via.relations = vec![RelationDecl {
        via: Some("customer_no".into()),
        ..derived("customer", "customer", vec![pair("customer_no", "no")])
    }];
    for (what, t, named) in [
        (
            "a pair of a text and a number field",
            two_kinds,
            vec!["account_no", "number"],
        ),
        (
            "a derived relation with `via`",
            with_via,
            vec!["customer", "via"],
        ),
    ] {
        let head = store.head().await.must("reading the head")?;
        invalid_naming(what, cat.declare(&own, Declaration::Type(t)).await, &named)?;
        crate::conformance::fixture::head_unmoved(store.as_ref(), head, what).await?;
    }
    Ok(())
}
