//! A derived relation that holds under a condition: its `where` reads the
//! owner's record as `from` and the target's as `to`, with the read's clock
//! (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`,
//! extended).

use lakelands_api::ontology::{Derived, MatchPair, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::query::{Projection, QueryResult, Walk};
use lakelands_api::{Access, Catalog, LakeStore, Position, Principal, Refusal, Timestamp, Value};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, derived, doors, field, field_access, head_unmoved, is_invalid, pattern, pause,
    refused_alike, refused_like, refused_on, step, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    CONDITIONAL_WALK = a_derived_relation_with_a_where_connects_only_the_pairs_it_holds_for,
    "044, extended (lakelands#960): a derived relation with a `where` connects two matching records only where the condition holds over the owner's record (`from`) and the target's (`to`): an open invoice inside its terms' discount period reaches its terms, a closed one, one past its period, one whose terms hold no period (null) and one whose terms no record holds reach none, and a condition that answers null (`from.b` unset, `if(from.b, null, true)` with `b` true) or a value that is no boolean (`from.status`) connects nothing, one that answers true connecting, while the same match without a condition reaches every matching terms; out, in and by the inverse alike, and `degree` and `components` count only the edges that hold; as of an earlier position the values are read as they stood and the clock is the read's, so an edge whose period has run out since is not answered there"
);

check!(
    CONDITION_PROBED = a_walk_over_a_conditional_edge_probes_what_its_where_reads,
    "024 and 044, extended (lakelands#960): a walk over a derived relation probes every field its `where` reads, on both sides, as it probes the matched fields: a principal who may not know of one is answered as for a relation the type does not declare, naming only the relation, and one who sees one masked is refused not_granted naming it"
);

check!(
    CONDITION_DECLARED = a_derived_relations_where_reads_a_stored_field_of_either_record,
    "044 and 052, extended (lakelands#960): a derived relation's `where` reads `from.<field>` and `to.<field>`, stored fields of its two records, one step each: a path of another form is refused `derived_where_path` naming it, a field the record does not have `unknown_field` naming its type, one computed at the read `reads_at_read`, and `in_set`, `retired` and an aggregate as anywhere a record's own condition stands; adding a `where` is a declaration that leaves the schema hash byte-identical, and a type declared again so that a field another relation's `where` reads is computed at the read is refused"
);

/// The terms an invoice is discounted by, keyed by `no`.
pub(super) fn terms() -> TypeDecl {
    let mut t = type_decl(
        "terms",
        vec![
            field("no", StorageKind::Text),
            field("discount_days", StorageKind::Number),
            field("rate", StorageKind::Number),
        ],
    );
    t.key = vec!["no".into()];
    t
}

/// An invoice is discountable towards its terms while it is open and its
/// terms' discount period has not passed.
pub(super) const DISCOUNTABLE: &str =
    "from.status = 'open' and now() <= from.invoiced_on + to.discount_days * 1d";

/// A relation from the invoice to its terms by `terms_no`, under `filter`.
pub(super) fn by_terms(name: &str, inverse: &str, filter: Option<&str>) -> RelationDecl {
    let pairs = vec![MatchPair {
        from: "terms_no".into(),
        to: "no".into(),
    }];
    let filter = filter.map(Into::into);
    RelationDecl {
        inverse: Some(inverse.into()),
        derived: Some(Derived { pairs, filter }),
        ..RelationDecl::to(name, "terms")
    }
}

pub(super) fn invoice(filter: &str) -> TypeDecl {
    let mut t = type_decl(
        "invoice",
        vec![
            field("terms_no", StorageKind::Text),
            field("status", StorageKind::Text),
            field("invoiced_on", StorageKind::DateTime),
            field("discount", StorageKind::Number),
            field("alt_no", StorageKind::Text),
            field("b", StorageKind::Boolean),
        ],
    );
    t.relations = vec![
        by_terms("discountable", "discountable_of", Some(filter)),
        by_terms("terms", "invoices", None),
        by_terms("odd", "odd_of", Some(ODD)),
        by_terms("flag", "flagged", Some("from.b")),
        by_terms("unflag", "unflagged", Some("if(from.b, null, true)")),
        by_terms("texty", "texted", Some("from.status")),
    ];
    t
}

/// A condition no pair can be judged by: text times a number.
const ODD: &str = "from.status * 2 > 1";

pub(super) const LATER: &str = "2099-01-01T00:00:00Z";
const EARLIER: &str = "2000-01-01T00:00:00Z";

/// Terms `t1` (fourteen days) and `t2` (no period); invoices `i1` (open,
/// inside), `i2` (closed), `i3` (open, past), `i4` (open, on `t2`) and `i5`
/// (open, on terms no record holds). Answers the position after them.
pub(super) async fn lake(store: &dyn LakeStore) -> Result<Position, Failure> {
    let l = Lake::of(store);
    l.declare(terms()).await?;
    l.declare(invoice(DISCOUNTABLE)).await?;
    l.upsert(
        "terms",
        "t1",
        json!({"no": "t1", "discount_days": 14, "rate": 2}),
    )
    .await?;
    l.upsert("terms", "t2", json!({"no": "t2", "rate": 3}))
        .await?;
    for (key, terms, status, on) in [
        ("i1", "t1", "open", LATER),
        ("i2", "t1", "closed", LATER),
        ("i3", "t1", "open", EARLIER),
        ("i4", "t2", "open", LATER),
        ("i5", "t9", "open", LATER),
    ] {
        let fields = json!({"terms_no": terms, "status": status, "invoiced_on": on});
        l.upsert("invoice", key, fields).await?;
    }
    store.head().await.must("reading the head")
}

/// The keys of `far` a walk from `ty` `key` over `relation` reaches, sorted.
async fn reached(
    cat: &dyn Catalog,
    who: &Principal,
    (ty, key): (&str, &str),
    (relation, walk): (&str, Walk),
    as_of: Option<Position>,
) -> Result<Vec<String>, Failure> {
    let far = if ty == "invoice" { "terms" } else { "invoice" };
    let mut p = pattern(ty, Some(key), vec![step(relation, walk)], rows());
    p.at.as_of = as_of;
    let what = format!("walking {relation} from {key}");
    let QueryResult::Rows { rows, .. } = cat.query(who, p).await.must(&what)?.data else {
        return Err(Failure::new(format!("{what} answered no rows")));
    };
    let mut keys: Vec<String> = (rows.iter())
        .filter_map(|r| r.get(&format!("{far}.key")).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Each row's key and its `column`, sorted; none where the read is refused.
fn cells(r: lakelands_api::Result<lakelands_api::Reply<QueryResult>>, column: &str) -> Vec<String> {
    let Ok(QueryResult::Rows { rows, .. }) = r.map(|r| r.data) else {
        return Vec::new();
    };
    let cell = |row: &std::collections::BTreeMap<String, Value>, c: &str| match row.get(c) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    let mut out: Vec<String> = (rows.iter())
        .map(|row| format!("{}={}", cell(row, "key"), cell(row, column)))
        .collect();
    out.sort();
    out
}

fn rows() -> Projection {
    Projection::Rows { fields: vec![] }
}

async fn a_derived_relation_with_a_where_connects_only_the_pairs_it_holds_for(
    store: Store,
) -> Held {
    let at = lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let mut got = Vec::new();
    for key in ["i1", "i2", "i3", "i4", "i5"] {
        let edge = reached(
            cat,
            own,
            ("invoice", key),
            ("discountable", Walk::Out),
            None,
        );
        let plain = reached(cat, own, ("invoice", key), ("terms", Walk::Out), None);
        got.push((key, edge.await?, plain.await?));
    }
    let t1 = || vec!["t1".to_owned()];
    let want = vec![
        ("i1", t1(), t1()),
        ("i2", vec![], t1()),
        ("i3", vec![], t1()),
        ("i4", vec![], vec!["t2".to_owned()]),
        ("i5", vec![], vec![]),
    ];
    ensure!(
        got == want,
        "an invoice reaches its terms over `discountable` only where the condition holds, and \
         over the plain match wherever the values match: {got:?}, expected {want:?}"
    );
    let odd = reached(cat, own, ("invoice", "i1"), ("odd", Walk::Out), None).await?;
    ensure!(
        odd.is_empty(),
        "a condition that cannot be evaluated over a pair connects nothing; `i1` reached {odd:?}"
    );
    let i1 = vec!["i1".to_owned()];
    for (relation, walk) in [("discountable", Walk::In), ("discountable_of", Walk::Out)] {
        let got = reached(cat, own, ("terms", "t1"), (relation, walk), None).await?;
        ensure!(
            got == i1,
            "from `t1` `{relation}` walked {walk:?} reaches `i1` alone; it reached {got:?}"
        );
    }
    // `degree` counts the edges that hold, `components` joins only them.
    let measure = |relation: &str, walk, project| {
        let p = pattern("terms", None, vec![step(relation, walk)], project);
        cat.query(own, p)
    };
    let degree = measure("discountable", Walk::In, Projection::Degree).await;
    let plain = measure("terms", Walk::In, Projection::Degree).await;
    let (d, p) = (cells(degree, "degree"), cells(plain, "degree"));
    let want = (vec!["t1=1", "t2=0"], vec!["t1=3", "t2=1"]);
    ensure!(
        (
            d.iter().map(String::as_str).collect(),
            p.iter().map(String::as_str).collect()
        ) == want,
        "`degree` over `discountable` counts the edges that hold, over the plain match every \
         match: {d:?} and {p:?}, expected {want:?}"
    );
    let joined = measure("discountable", Walk::Both, Projection::Components).await;
    let joined = cells(joined, "size");
    let pairs: Vec<&String> = joined.iter().filter(|c| c.ends_with("=2")).collect();
    ensure!(
        pairs == ["i1=2", "t1=2"],
        "`components` over `discountable` joins `t1` and `i1` alone: {joined:?}"
    );
    // A condition that answers null, or no boolean, connects nothing.
    for (key, b) in [
        ("ib1", json!(true)),
        ("ib0", json!(false)),
        ("ibn", Value::Null),
    ] {
        let fields = json!({"terms_no": "t1", "status": "open", "b": b});
        l.upsert("invoice", key, fields).await?;
    }
    let mut got = Vec::new();
    for key in ["ib1", "ib0", "ibn"] {
        for relation in ["flag", "unflag", "texty"] {
            let to = reached(cat, own, ("invoice", key), (relation, Walk::Out), None).await?;
            got.push(format!("{key} {relation} {}", to.join("")));
        }
    }
    let want = [
        "ib1 flag t1",
        "ib1 unflag ",
        "ib1 texty ",
        "ib0 flag ",
        "ib0 unflag t1",
        "ib0 texty ",
        "ibn flag ",
        "ibn unflag t1",
        "ibn texty ",
    ];
    ensure!(
        got == want,
        "a condition connects where it answers true alone, null and a value that is no boolean \
         connecting nothing: {got:?}, expected {want:?}"
    );
    // As of an earlier position the values are read as they stood.
    l.upsert("invoice", "i3", json!({"invoiced_on": LATER}))
        .await?;
    let then = reached(
        cat,
        own,
        ("invoice", "i3"),
        ("discountable", Walk::Out),
        Some(at),
    );
    let now = reached(
        cat,
        own,
        ("invoice", "i3"),
        ("discountable", Walk::Out),
        None,
    );
    let (then, now) = (then.await?, now.await?);
    ensure!(
        then.is_empty() && now == t1(),
        "`i3` moved into its period reaches `t1` from that write on and not as of before it; \
         as of before {then:?}, at the head {now:?}"
    );
    // And the clock is the read's: a period that runs out three seconds after
    // the write is out in a read as of that write made later.
    l.upsert("terms", "t0", json!({"no": "t0", "discount_days": 0}))
        .await?;
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let wall = wall.map_or(0, |d| d.as_micros() as i64);
    let soon = Timestamp(wall + 3_000_000);
    let fields = json!({"terms_no": "t0", "status": "open", "invoiced_on": soon.to_string()});
    l.upsert("invoice", "i6", fields).await?;
    let written = store.head().await.must("reading the head")?;
    let i6 = ("invoice", "i6");
    let before = reached(cat, own, i6, ("discountable", Walk::Out), Some(written)).await?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let left = soon.0 - now.map_or(0, |d| d.as_micros() as i64);
    pause(std::time::Duration::from_micros(
        left.max(0) as u64 + 100_000,
    ))
    .await;
    let after = reached(cat, own, i6, ("discountable", Walk::Out), Some(written)).await?;
    ensure!(
        before == vec!["t0".to_owned()] && after.is_empty(),
        "a read as of a position reads the condition with its own clock: `i6` reaches `t0` \
         before its period runs out and not after, as of the same position; it reached \
         {before:?}, then {after:?}"
    );
    Ok(())
}

async fn a_walk_over_a_conditional_edge_probes_what_its_where_reads(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let (s, cat) = (store.as_ref(), doors(store.as_ref()));
    let walk = |ty: &str, key: &str, relation: &str, walk| {
        pattern(ty, Some(key), vec![step(relation, walk)], rows())
    };
    for (ty, field, p) in [
        (
            "invoice",
            "status",
            walk("invoice", "i1", "discountable", Walk::Out),
        ),
        (
            "terms",
            "discount_days",
            walk("terms", "t1", "discountable", Walk::In),
        ),
        (
            "invoice",
            "invoiced_on",
            walk("terms", "t1", "discountable_of", Walk::Out),
        ),
    ] {
        let blind = field_access(s, "blind", ty, &[(field, Access::None)]);
        let mut ghost = p.clone();
        ghost.steps[0].relation = "ghost".into();
        let ghost = cat.query(&blind, ghost).await;
        let relation = p.steps[0].relation.to_string();
        let r = cat.query(&blind, p.clone()).await;
        ensure!(
            is_invalid(&r),
            "a walk over `{relation}` by a principal who may not know of `{ty}.{field}` is \
             answered as for a relation the type does not declare; it answered {r:?}"
        );
        let what = format!("a walk over `{relation}`, held from `{ty}.{field}`,");
        refused_alike(&what, &r, ghost, ("ghost", &relation))?;
        let masked = field_access(s, "masked", ty, &[(field, Access::Masked)]);
        let r = cat.query(&masked, p).await;
        ensure!(
            matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == field),
            "a walk over `{relation}` by a principal who sees `{ty}.{field}` masked is refused \
             as not granted, naming it; it answered {r:?}"
        );
    }
    Ok(())
}

async fn a_derived_relations_where_reads_a_stored_field_of_either_record(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(terms()).await?;
    let mut none = invoice(DISCOUNTABLE);
    none.relations.iter_mut().for_each(|r| {
        r.derived.iter_mut().for_each(|d| d.filter = None);
    });
    let mut hashes = Vec::new();
    for t in [none, invoice("from.status = 'open'"), invoice(DISCOUNTABLE)] {
        l.declare(t).await?;
        hashes.push(l.describe_type("invoice").await?.lifecycle.schema_hash);
    }
    ensure!(
        hashes.windows(2).all(|w| w[0] == w[1]),
        "adding a derived relation's `where` and changing it leave the schema hash \
         byte-identical; it went {hashes:?}"
    );
    let mut later = terms();
    later
        .fields
        .push(at_read("late", StorageKind::Number, "rate + 1", "rate"));
    l.declare(later.clone()).await?;
    let path = |p: &str| json!({"code": "derived_where_path", "path": p});
    for (filter, want) in [
        ("status = 'open'", path("status")),
        ("from.terms_no.no = 't1'", path("from.terms_no.no")),
        ("to = 't1'", path("to")),
        (
            "from.nope = 1",
            json!({"code": "unknown_field", "type": "invoice", "field": "nope"}),
        ),
        (
            "to.nope = 1",
            json!({"code": "unknown_field", "type": "terms", "field": "nope"}),
        ),
        (
            "to.late > 1",
            json!({"code": "reads_at_read", "type": "terms", "field": "late"}),
        ),
        ("in_set('open')", json!({"code": "in_set_here"})),
        ("retired()", json!({"code": "retired_only_in_guard"})),
        ("sum(to.rate) > 1", json!({"code": "aggregate_here"})),
    ] {
        let head = store.head().await.must("reading the head")?;
        refused_like(filter, l.try_declare(invoice(filter)).await, &want)?;
        head_unmoved(store.as_ref(), head, filter).await?;
    }
    // A field a `where` reads is not taken to the read, on either side.
    l.declare(invoice("from.status = 'open' and to.rate > 1"))
        .await?;
    let mut rate = later.clone();
    rate.fields[2] = at_read(
        "rate",
        StorageKind::Number,
        "discount_days + 1",
        "discount_days",
    );
    let mut status = invoice("from.status = 'open' and to.rate > 1");
    status.fields[1] = at_read("status", StorageKind::Text, "terms_no", "terms_no");
    for (what, t, want) in [
        (
            "terms.rate taken to the read",
            rate,
            json!({"code": "read_as_stored"}),
        ),
        (
            "invoice.status taken to the read",
            status,
            json!({"code": "at_read_match"}),
        ),
    ] {
        let head = store.head().await.must("reading the head")?;
        refused_like(what, l.try_declare(t).await, &want)?;
        head_unmoved(store.as_ref(), head, what).await?;
    }
    Ok(())
}

/// A field computed at the read by `expr` from `dep`.
fn at_read(
    name: &str,
    kind: StorageKind,
    expr: &str,
    dep: &str,
) -> lakelands_api::ontology::FieldDecl {
    let mut f = derived(name, kind, expr, &[dep]);
    f.computed_at = Some(lakelands_api::ontology::ComputedAt::Read);
    f
}
