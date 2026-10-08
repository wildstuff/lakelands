//! A relation derived from matching values
//! (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
//!
//! A derived relation stores no edge. A walk connects the records whose
//! matched fields hold the same non-null value, as the read's position
//! holds them, both ways and in every walk the store runs.

use std::sync::Arc;

use lakelands_api::ontology::{
    Cardinality, Derived, MatchPair, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::query::{Pattern, Projection, QueryResult, Walk};
use lakelands_api::write::WriteOpts;
use lakelands_api::{Access, Catalog, LakeStore, Position, Principal, Refusal, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, doors, field, field_access, head_unmoved, is_invalid, owner, patch, pattern,
    refused_alike, refused_like, refused_on, step, type_decl, type_of,
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
    "024, 044 and 006: a walk over a derived relation, a step either way, a graph and a measure alike, past its first hop too, is answered as for a relation the type does not declare, naming only the relation, when the principal may not know of a matched field on either side; over a matched field it sees masked it is refused as not granted, naming the field"
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
    let mut t = type_of(
        "customer",
        &[("no", StorageKind::Text), ("region", StorageKind::Text)],
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
    let l = Lake::of(store);
    l.declare(customer()).await?;
    l.declare(invoice()).await?;
    for (no, region) in [("10", "north"), ("010", "north"), ("20", "south")] {
        l.upsert("customer", no, json!({"no": no, "region": region}))
            .await?;
    }
    l.upsert(
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
        l.upsert("invoice", key, fields).await?;
    }
    Ok(before)
}

fn walk(ty: &str, key: &str, relation: &str, dir: Walk, as_of: Option<Position>) -> Pattern {
    let rows = Projection::Rows { fields: vec![] };
    let mut p = pattern(ty, Some(key), vec![step(relation, dir)], rows);
    p.at.as_of = as_of;
    p
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

/// The keys a walk over `relation` from `key` reaches: out from an invoice
/// to customers, in from a customer to invoices.
async fn reach(
    cat: &dyn Catalog,
    who: &Principal,
    key: &str,
    relation: &str,
    dir: Walk,
    as_of: Option<Position>,
) -> Result<Vec<String>, Failure> {
    let (ty, far) = match dir {
        Walk::In => ("customer", "invoice"),
        _ => ("invoice", "customer"),
    };
    reached(cat, who, walk(ty, key, relation, dir, as_of), far).await
}

async fn a_walk_over_a_derived_relation_finds_exactly_the_matching_records_both_ways(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let before = lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // A walk to check: its start key, relation, direction, `as_of`, and the
    // far keys it must reach. Out walks from an invoice to a customer, In
    // the other way.
    for (key, relation, dir, as_of, want) in [
        ("i1", "customer", Walk::Out, None, vec!["10"]),
        ("i3", "customer", Walk::Out, None, vec!["010"]),
        ("10", "customer", Walk::In, None, vec!["i1", "i2"]),
        ("010", "customer", Walk::In, None, vec!["i3"]),
        ("10", "customer", Walk::In, Some(before), vec!["i1"]),
        ("i1", "local_customer", Walk::Out, None, vec!["10"]),
        ("i2", "local_customer", Walk::Out, None, vec![]),
        ("10", "local_customer", Walk::In, None, vec!["i1"]),
    ] {
        let (ty, far) = match dir {
            Walk::In => ("customer", "invoice"),
            _ => ("invoice", "customer"),
        };
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
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    l.upsert("invoice", "i1", json!({"customer_no": "20"}))
        .await?;
    for (as_of, want) in [(Some(before), vec!["10"]), (None, vec!["20"])] {
        let got = reach(cat.as_ref(), own, "i1", "customer", Walk::Out, as_of).await?;
        ensure!(
            got == want,
            "i1's customer as_of {as_of:?} must be {want:?}; the walk reached {got:?}"
        );
    }
    let got = reach(cat.as_ref(), own, "20", "customer", Walk::In, None).await?;
    ensure!(
        got == ["i1", "i5"],
        "customer 20 must be reached from i1 and i5 after the move; it was from {got:?}"
    );
    Ok(())
}

async fn a_null_never_matches(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // A customer without a region, and i4 without either field.
    l.upsert("customer", "30", json!({"no": "30"})).await?;
    for relation in ["customer", "same_region"] {
        let got = reach(cat.as_ref(), own, "i4", relation, Walk::Out, None).await?;
        ensure!(
            got.is_empty(),
            "i4 holds no matched value, so `{relation}` must reach nothing; it reached {got:?}"
        );
    }
    let got = reach(cat.as_ref(), own, "30", "same_region", Walk::In, None).await?;
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
    let blind = |ty: &str, f: &str| field_access(store.as_ref(), "blind", ty, &[(f, Access::None)]);
    let in_local = vec![step("local_customer", Walk::In)];
    let both = |relation| vec![step(relation, Walk::Both)];
    let degree = pattern("customer", None, in_local, Projection::Degree);
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
        (
            blind("invoice", "customer_no"),
            walk("customer", "10", "customer", Walk::In, None),
            "customer_no",
        ),
        (
            blind("invoice", "customer_no"),
            pattern("invoice", Some("i1"), both("customer"), Projection::Graph),
            "customer_no",
        ),
        (
            blind("customer", "region"),
            pattern(
                "customer",
                None,
                both("local_customer"),
                Projection::Components,
            ),
            "region",
        ),
    ];
    for (who, p, field) in cases {
        let (relation, what) = (p.steps[0].relation.to_string(), format!("{:?}", p.project));
        // The same walk over `ghost`, which the type does not declare.
        let mut ghost = p.clone();
        ghost.steps[0].relation = "ghost".into();
        let ghost = cat.query(&who, ghost).await;
        let r = cat.query(&who, p).await;
        ensure!(
            is_invalid(&r),
            "{what} over `{relation}` by a principal who may not know of `{field}` is answered \
             as for a relation the type does not declare, naming only the relation; it \
             answered {r:?}"
        );
        let what = format!("{what} over `{relation}`, held from `{field}`,");
        refused_alike(&what, &r, ghost, ("ghost", &relation))?;
    }
    let masked = [("customer_no", Access::Masked)];
    let masked = field_access(store.as_ref(), "masked", "invoice", &masked);
    let r = (cat.query(&masked, walk("invoice", "i1", "customer", Walk::Out, None))).await;
    ensure!(
        matches!(refused_on(&r), Some(("customer_no", Refusal::NotGranted))),
        "a walk over `customer` by a principal who sees `customer_no` masked is refused as not \
         granted, naming it; it answered {r:?}"
    );
    // From `m1`, whose own `customer` reaches `10`, the invoices'
    // `customer` comes up only at the second hop, around `10`.
    let l = Lake::of(store.as_ref());
    let about = json!({"name": "about", "type": {"kind": "reference", "target": "customer"}});
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [about],
        "relations": [{"name": "customer", "target": "customer", "via": "about"}]}))
        .await?;
    l.upsert("memo", "m1", json!({"about": "10"})).await?;
    let depth = lakelands_api::query::Depth { min: 1, max: 2 };
    let steps = vec![lakelands_api::query::Step {
        depth,
        ..step("customer", Walk::Both)
    }];
    let r = (cat.query(
        &masked,
        pattern("memo", Some("m1"), steps, Projection::Graph),
    ))
    .await;
    ensure!(
        matches!(refused_on(&r), Some(("customer_no", Refusal::NotGranted))),
        "a graph two hops over `customer` from `m1` by a principal who sees `customer_no` \
         masked is refused as not granted, naming it, where the second hop walks the \
         invoices' `customer`; it answered {r:?}"
    );
    Ok(())
}

async fn degree_and_components_over_a_derived_relation_equal_those_over_the_same_links(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // The same connections, written as link records between twin types.
    l.declare(type_decl("client", vec![field("no", StorageKind::Text)]))
        .await?;
    let mut bill = type_decl("bill", vec![field("note", StorageKind::Text)]);
    bill.relations = vec![RelationDecl {
        cardinality: Cardinality::ManyToOne,
        ..RelationDecl::to("customer", "client")
    }];
    l.declare(bill).await?;
    for c in ["10", "010", "20"] {
        l.upsert("client", c, json!({"no": c})).await?;
    }
    for (b, c) in [("i1", "10"), ("i2", "10"), ("i3", "010"), ("i5", "20")] {
        l.upsert("bill", b, json!({"note": b})).await?;
        cat.link(
            own,
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
    l.upsert("bill", "i4", json!({"note": "i4"})).await?;
    let measure = |ty: &str, project: Projection, walk: Walk| {
        pattern(ty, None, vec![step("customer", walk)], project)
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
            .query(own, measure("customer", project.clone(), walk_dir))
            .await
            .must(&format!("{project:?} over the derived relation"))?
            .data;
        let linked = cat
            .query(own, measure("client", project.clone(), walk_dir))
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
    let l = Lake::of(store.as_ref());
    let before = l.describe_type("invoice").await?.lifecycle.schema_hash;
    let mut changed = invoice();
    changed.relations[2] = derived(
        "same_region",
        "customer",
        vec![pair("region", "region"), pair("customer_no", "no")],
    );
    l.declare(changed.clone()).await?;
    let after = l.describe_type("invoice").await?.lifecycle.schema_hash;
    ensure!(
        before == after,
        "changing a derived relation's match must leave the schema hash byte-identical; it \
         went from {before} to {after}"
    );
    // Carried by links, then declared derived; derived, then carried.
    let mut linked = invoice();
    linked.name = "order".into();
    linked.relations = vec![RelationDecl::to("customer", "customer")];
    l.declare(linked.clone()).await?;
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
        let case = json!({"code": "carrier_changed",
                          "at": [{"code": "relation", "name": "customer"}]});
        refused_like(what, l.try_declare(t).await, &case)?;
        head_unmoved(store.as_ref(), head, what).await?;
    }
    Ok(())
}

async fn a_match_pair_of_two_kinds_is_refused_naming_both(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(customer()).await?;
    let mut numbered = customer();
    numbered.name = "account".into();
    numbered.fields.push(field("number", StorageKind::Number));
    l.declare(numbered).await?;
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
    for (what, t, case) in [
        (
            "a pair of a text and a number field",
            two_kinds,
            json!({"code": "pair_kinds", "from": "account_no", "to": "number"}),
        ),
        (
            "a derived relation with `via`",
            with_via,
            json!({"code": "derived_stores_edge",
                   "at": [{"code": "relation", "name": "customer"}]}),
        ),
    ] {
        let head = store.head().await.must("reading the head")?;
        refused_like(what, l.try_declare(t).await, &case)?;
        head_unmoved(store.as_ref(), head, what).await?;
    }
    Ok(())
}
