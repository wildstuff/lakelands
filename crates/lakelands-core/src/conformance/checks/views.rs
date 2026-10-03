//! View types: records computed from another type, read with `find`
//! like any other and written by no one (`lakelands_api::ontology::ViewDecl`).
//!
//! A view is a standing obligation, not a query: the store keeps it
//! current as its source changes, through every door that changes the
//! source. It is a function of the source's state, so the same source
//! state gives the same view record for record, byte for byte, however
//! the source got there. The fixture is one source (`invoice`: a partner
//! and an exact amount) and one view over it (a total and a count per
//! partner).

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::key::UNGROUPED;
use lakelands_api::ontology::{
    Authority, Cardinality, Derived, MatchPair, RelationDecl, StorageKind, TypeDecl, ViewParam,
    ViewSource,
};
use lakelands_api::query::{Pattern, Projection, QueryResult, Step, Walk};
use lakelands_api::read::Find;
use lakelands_api::{Error, FieldName, LakeStore, Record, Refusal, TypeName, Value};
use rust_decimal::Decimal;
use serde_json::json;

use super::search::doc_search;
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, dec, decimal, delivered, embedding, field, field_access, find_where,
    get_at, head_unmoved, measure, object_set_on, partner_totals, pattern, reason, source_record,
    step, trigger_on, type_decl, type_of, value, vector, view_of,
};

check!(
    /// A view record's key is its group.
    VIEW_KEY_IS_ITS_GROUP = a_view_record_is_keyed_by_its_group_by_fields,
    "018 and the log format §2.3: a view record's key is made from its `group_by` fields as any record key is, each value with `%` and `:` escaped, joined by `:` in declared order"
);

check!(
    /// A view without `group_by` holds one record.
    UNGROUPED_HOLDS_ONE = an_ungrouped_view_holds_one_record_keyed_all_even_over_none,
    "018, extended: a view without `group_by` holds exactly one record, keyed `all`, the totals over every source record its `where` keeps, even when it keeps none (a count and a sum of none are 0, a minimum null); `get` answers it by that key and no other, and a read's parameters and a merged source measure into the same record"
);

check!(
    /// A view groups its source and sums it exactly.
    GROUPS_AND_SUMS = a_view_groups_its_source_and_sums_it_exactly,
    "the ontology: a view type is computed from its source's records, one record per group, measures exact for decimals"
);

check!(
    /// A view does not depend on when it was declared.
    SAME_EITHER_WAY = a_view_is_the_same_whether_declared_before_or_after_its_source_was_written,
    "the ontology: a view is a function of its source's state, so how the source got there cannot change a byte of it"
);

check!(
    /// A group with no members left has no record.
    EMPTIED_GROUP_GOES = a_view_drops_a_group_whose_last_member_is_retired,
    "the ontology: a view holds a record per group that has members; retiring a group's last member removes the group"
);

check!(
    /// A pattern may start on a view.
    PATTERN_STARTS_ON_A_VIEW = a_pattern_started_on_a_view_holds_the_records_find_returns,
    "018 and 026: a view is read like any other type, so a pattern started on it by key, by `where` or with neither has as its graph's nodes exactly the records `find` returns for that selection; a key with no group has none"
);

check!(
    /// A walk reaches a view over a derived relation.
    WALK_REACHES_A_VIEW = a_walk_over_a_derived_relation_reaches_a_views_records_both_ways,
    "018, 026 and 044: a derived relation between a stored type and a view connects their records by matching values like any other, so a walk out from the stored type reaches the view's records, a step's `where` keeps those of them it holds for, and a walk in from the view reaches the stored ones"
);

check!(
    /// A view's field asks for no search.
    VIEW_FIELD_NOT_SEARCHED = a_view_field_that_asks_to_be_searched_or_embedded_is_refused,
    "018: a view is not searched, so `declare` refuses a view field that is searchable, carries an embedding or is a vector, naming the view and the field"
);

check!(
    /// Nothing that could never act is declared on a view.
    NO_TRIGGER_OR_SET_ON_A_VIEW = a_trigger_or_object_set_on_a_view_is_refused,
    "018, extended: a view is never written, so `declare` and `apply` refuse a trigger on a view, which no write would fire, and an object set over one, which holds stored records, naming both; nothing is appended"
);

check!(
    /// A relation on a view is derived.
    VIEW_RELATIONS_DERIVED = a_relation_on_a_view_is_derived_or_refused,
    "018, extended, and 035: a view's records are computed and never written, so `declare` and `apply` refuse a relation on a view carried by link records or by a `via` field, which could never hold an edge, naming the view and the relation; a relation derived from matching values is taken"
);

check!(
    /// A view reads only what its sources have.
    VIEW_READS_ITS_SOURCES = a_view_reading_a_field_its_source_lacks_is_refused,
    "028 and 018: `declare` and `apply` refuse a view whose `where` names, or whose measures aggregate, a field its source or a merged source does not have, naming the view, the source and the field; a name outside an aggregate is an earlier measure or a grouping field, and is taken"
);

check!(
    /// A view is not searched.
    VIEW_NOT_SEARCHED = a_search_on_a_view_is_refused_naming_it,
    "018: a view's records are computed on read and no index holds them, so a lexical, vector or hybrid search on a view is refused as invalid, naming the view, never answered empty"
);

check!(
    /// A view may not read itself.
    SOURCE_CYCLE_REFUSED = a_view_closing_a_cycle_over_source_is_refused_naming_the_chain,
    "018 and 028: a view may not read itself, directly or through other views, so `declare` refuses a view whose `source` closes a cycle of length 1, 2 or 3, and a set applied whole whose new views close one among themselves, naming the chain; the views keep what they read"
);

check!(
    /// A view declares the fields it computes.
    VIEW_FIELDS_DECLARED = a_view_declares_exactly_the_fields_it_computes,
    "018 and 028: a read answers declared fields only, so `declare` refuses a view whose `group_by` field or measure has no declared field, or which declares a field nothing computes, naming it"
);

check!(
    /// A measure read by name is no source field.
    MEASURE_BY_NAME_DESCRIBED = a_view_whose_measure_reads_another_is_described_to_its_readers,
    "024 and 018: `describe` answers a view's definition to a principal who reads every field of its source that it names; a measure that reads an earlier measure by name names no field, so it keeps the definition from no one, while a name inside an aggregate is the source's field, even where a measure has the same name"
);

check!(
    /// Ingest keeps a view current.
    CURRENT_AFTER_INGEST = a_view_over_an_ingested_source_is_current_once_the_ingest_is_committed,
    "016 and the ontology: both write doors are folded by the same rules, so a view over ingested records is kept like any other"
);

check!(
    /// Every door refuses a view alike.
    REFUSED_ALIKE = every_door_that_writes_or_holds_a_record_refuses_a_view_alike,
    "018, extended (lakelands#370): upsert, retire, link, unlink, ingest, propose, restrict and unrestrict each refuse a view as invalid naming it, as upsert does, before the grants; declaring a verb on a record of a view is refused naming both, while a verb on the view's type that reaches outside is taken"
);

/// The source type, its `partner` and `amount` written by `authority`.
fn invoices(authority: Authority) -> TypeDecl {
    let mut partner = field("partner", StorageKind::Text);
    let mut amount = field("amount", StorageKind::Decimal);
    partner.authority = authority;
    amount.authority = authority;
    type_decl("invoice", vec![partner, amount])
}

/// A lake with the invoices, written by their author, and the view `totals`
/// over them (`partner_totals`, totals as decimals) when one is named.
async fn invoiced(store: &dyn LakeStore, totals: Option<&str>) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(invoices(Authority::Authored)).await?;
    if let Some(name) = totals {
        l.declare(partner_totals(name, StorageKind::Decimal))
            .await?;
    }
    Ok(l)
}

/// A view's records, by partner: each record's fields as stored.
async fn view_rows(
    l: &Lake,
    view: &str,
) -> Result<BTreeMap<String, BTreeMap<FieldName, Value>>, Failure> {
    let mut out = BTreeMap::new();
    for rec in l.all_found(view, Find::default()).await? {
        let Some(partner) = rec.fields.get("partner").and_then(Value::as_str) else {
            return Err(Failure::new(format!(
                "a record of the view `{view}` (key `{}`) does not carry its group field \
                 `partner`: {:?}",
                rec.key, rec.fields
            )));
        };
        out.insert(partner.to_owned(), rec.fields.clone());
    }
    Ok(out)
}

/// A view's totals and counts, by partner, as exact numbers.
async fn view_totals(
    l: &Lake,
    view: &str,
) -> Result<Vec<(String, Option<Decimal>, Option<f64>)>, Failure> {
    Ok(view_rows(l, view)
        .await?
        .into_iter()
        .map(|(partner, fields)| {
            let total = decimal(fields.get("total"));
            let count = fields.get("invoices").and_then(Value::as_f64);
            (partner, total, count)
        })
        .collect())
}

fn want(rows: &[(&str, &str, f64)]) -> Vec<(String, Option<Decimal>, Option<f64>)> {
    rows.iter()
        .map(|(p, t, n)| ((*p).to_owned(), Some(dec(t)), Some(*n)))
        .collect()
}

async fn write_invoice(l: &Lake, nr: &str, partner: &str, amount: &str) -> Result<(), Failure> {
    let fields = json!({"partner": partner, "amount": amount});
    l.upsert("invoice", nr, fields).await?;
    Ok(())
}

async fn a_view_groups_its_source_and_sums_it_exactly(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("partner_totals")).await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
    ] {
        write_invoice(&l, nr, partner, amount).await?;
    }

    let got = view_totals(&l, "partner_totals").await?;
    let expected = want(&[("p1", "15.50", 2.0), ("p2", "3.00", 1.0)]);
    ensure!(
        got == expected,
        "the view over three invoices must hold p1 = 15.50 from 2 invoices and p2 = 3.00 \
         from 1, exactly; it holds {got:?}. A declared view the store accepted is a view \
         it owes"
    );
    Ok(())
}

async fn a_view_is_the_same_whether_declared_before_or_after_its_source_was_written(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("totals_early")).await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
        ("i4", "p3", "100.00"),
    ] {
        write_invoice(&l, nr, partner, amount).await?;
    }
    // An update in place, a move between groups, and a group's only
    // member restated at a shorter scale: every way the early view must
    // follow its source.
    write_invoice(&l, "i1", "p1", "20.00").await?;
    write_invoice(&l, "i2", "p2", "5.50").await?;
    write_invoice(&l, "i4", "p3", "100.5").await?;
    l.declare(partner_totals("totals_late", StorageKind::Decimal))
        .await?;

    let early = view_rows(&l, "totals_early").await?;
    let late = view_rows(&l, "totals_late").await?;
    ensure!(
        early == late,
        "two views over the same source state disagree: declared before the writes it \
         holds {early:?}, declared after them {late:?}. A view is a function of its \
         source's state, byte for byte, trailing zeros included"
    );
    let got = view_totals(&l, "totals_early").await?;
    let expected = want(&[
        ("p1", "20.00", 1.0),
        ("p2", "8.50", 2.0),
        ("p3", "100.5", 1.0),
    ]);
    ensure!(
        got == expected,
        "after the updates the view must hold p1 = 20.00 (1), p2 = 8.50 (2), \
         p3 = 100.5 (1); it holds {got:?}"
    );
    Ok(())
}

async fn a_view_drops_a_group_whose_last_member_is_retired(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("partner_totals")).await?;
    write_invoice(&l, "i1", "p1", "10.00").await?;
    write_invoice(&l, "i2", "p2", "3.00").await?;
    write_invoice(&l, "i3", "p2", "4.00").await?;

    for nr in ["i2", "i1"] {
        l.retire("invoice", nr).await?;
    }

    let got = view_totals(&l, "partner_totals").await?;
    let expected = want(&[("p2", "4.00", 1.0)]);
    ensure!(
        got == expected,
        "p1 lost its only invoice and p2 one of two; the view must hold only \
         p2 = 4.00 (1), with no zeroed record for p1; it holds {got:?}"
    );
    Ok(())
}

async fn a_view_over_an_ingested_source_is_current_once_the_ingest_is_committed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(invoices(Authority::Source)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;

    let delivery = [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
        // The same key twice in one delivery: the view reflects the
        // record that stands, not both.
        ("i1", "p1", "20.00"),
    ]
    .into_iter()
    .map(|(nr, partner, amount)| {
        source_record("invoice", nr, json!({"partner": partner, "amount": amount}))
    })
    .collect();
    let what = "ingesting four invoices from `ledger`";
    delivered(store.as_ref(), &l, "ledger", delivery, what).await?;

    let got = view_totals(&l, "partner_totals").await?;
    let expected = want(&[("p1", "25.50", 2.0), ("p2", "3.00", 1.0)]);
    ensure!(
        got == expected,
        "after the ingest the view must hold p1 = 25.50 (2) and p2 = 3.00 (1); it holds \
         {got:?}. A door that writes the source and skips what derives from it leaves the \
         view stale"
    );
    Ok(())
}

async fn a_view_record_is_keyed_by_its_group_by_fields(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let mut source = invoices(Authority::Authored);
    source.fields.push(field("region", StorageKind::Text));
    l.declare(source).await?;
    let fields = vec![
        field("partner", StorageKind::Text),
        field("region", StorageKind::Text),
        field("invoices", StorageKind::Number),
    ];
    let by = ["partner", "region"];
    let measures = [("invoices", "count()")];
    let view = view_of("by_both", "invoice", &by, fields, &measures);
    l.declare(view).await?;
    for (k, partner, region) in [
        ("1", "acme", "north"),
        ("2", "acme", "south:east"),
        ("3", "acme", "north"),
    ] {
        let row = json!({"partner": partner, "region": region, "amount": "1"});
        l.upsert("invoice", k, row).await?;
    }
    let mut keys: Vec<String> = l
        .all_found("by_both", Find::default())
        .await?
        .into_iter()
        .map(|r| r.key.0)
        .collect();
    keys.sort();
    ensure!(
        keys == ["acme:north", "acme:south%3Aeast"],
        "the view grouped by partner and region must key its records `acme:north` and \
         `acme:south%3Aeast` (the `:` inside a value escaped); they are keyed {keys:?}"
    );
    let north = l.read("by_both", "acme:north").await?;
    ensure!(
        north.fields.get("invoices").and_then(Value::as_f64) == Some(2.0),
        "reading the view record `acme:north` by its key must answer 2 invoices; it \
         answers {:?}",
        north.fields
    );
    Ok(())
}

/// A pattern over `ty` from a start, with its steps: a graph without
/// steps, rows with them.
fn from(ty: &str, key: Option<&str>, filter: Option<&str>, steps: Vec<Step>) -> Pattern {
    let project = if steps.is_empty() {
        Projection::Graph
    } else {
        Projection::Rows { fields: vec![] }
    };
    let mut p = pattern(ty, key, steps, project);
    p.start.filter = filter.map(Into::into);
    p
}

/// The `type:key` of every node a graph answers, or every `far` key a
/// walk's rows reach, sorted.
async fn nodes(l: &Lake, p: Pattern, far: &str) -> Result<Vec<String>, Failure> {
    let what = format!(
        "a pattern started on `{}` (key {:?}, where {:?})",
        p.start.ty, p.start.key, p.start.filter
    );
    let reply = l.cat.query(&l.own, p).await.must(&what)?;
    let mut got: Vec<String> = match reply.data {
        QueryResult::Graph { nodes, .. } => nodes
            .into_iter()
            .map(|n| format!("{}:{}", n.ty, n.key))
            .collect(),
        QueryResult::Rows { rows, .. } => rows
            .iter()
            .filter_map(|r| r.get(&format!("{far}.key")).and_then(Value::as_str))
            .map(|k| format!("{far}:{k}"))
            .collect(),
        QueryResult::Groups { .. } => {
            return Err(Failure::new(format!("{what} answered groups")));
        }
    };
    got.sort();
    got.dedup();
    Ok(got)
}

async fn a_pattern_started_on_a_view_holds_the_records_find_returns(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("partner_totals")).await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p2", "3.00"),
        ("i3", "p1", "5.00"),
    ] {
        write_invoice(&l, nr, partner, amount).await?;
    }
    for (key, filter) in [
        (None, None),
        (None, Some("partner = 'p1'")),
        (Some("p2"), None),
    ] {
        let find = filter.map(find_where).unwrap_or_default();
        let found = l.all_found("partner_totals", find).await?;
        let want: Vec<String> = found
            .iter()
            .map(|r| format!("partner_totals:{}", r.key))
            .filter(|n| key.is_none_or(|k| *n == format!("partner_totals:{k}")))
            .collect();
        let p = from("partner_totals", key, filter, vec![]);
        let got = nodes(&l, p, "").await?;
        ensure!(
            got == want,
            "a pattern started on the view `partner_totals` (key {key:?}, where {filter:?}) must \
             hold as its graph's nodes the records `find` returns, {want:?}; it holds {got:?}. \
             A view read through the pattern query that answers nothing looks exactly like an \
             empty view"
        );
    }
    let p = from("partner_totals", Some("p9"), None, vec![]);
    let none = nodes(&l, p, "").await?;
    ensure!(
        none.is_empty(),
        "no invoice has partner p9, so the view has no group `p9` and a pattern started on \
         it by that key has no node; it holds {none:?}"
    );
    Ok(())
}

async fn a_walk_over_a_derived_relation_reaches_a_views_records_both_ways(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("partner_totals")).await?;
    let mut partner = type_decl("partner", vec![field("name", StorageKind::Text)]);
    partner.key = vec![FieldName::from("name")];
    partner.relations = vec![RelationDecl {
        cardinality: Cardinality::OneToOne,
        derived: Some(Derived {
            pairs: vec![MatchPair {
                from: "name".into(),
                to: "partner".into(),
            }],
        }),
        ..RelationDecl::to("totals", "partner_totals")
    }];
    l.declare(partner).await?;
    for name in ["p1", "p2", "p3"] {
        l.upsert("partner", name, json!({"name": name})).await?;
    }
    write_invoice(&l, "i1", "p1", "10.00").await?;
    write_invoice(&l, "i2", "p2", "3.00").await?;
    let totals = |walk, filter: Option<&str>| Step {
        filter: filter.map(Into::into),
        ..step("totals", walk)
    };
    // Out walks from `partner` to `partner_totals`, In the other way.
    for (key, walk, filter, want) in [
        ("p1", Walk::Out, None, vec!["partner_totals:p1"]),
        ("p3", Walk::Out, None, vec![]),
        // A step's `where` reads the view's records it reached.
        (
            "p1",
            Walk::Out,
            Some("total > 5"),
            vec!["partner_totals:p1"],
        ),
        ("p2", Walk::Out, Some("total > 5"), vec![]),
        ("p2", Walk::In, None, vec!["partner:p2"]),
    ] {
        let (start, far) = match walk {
            Walk::In => ("partner_totals", "partner"),
            _ => ("partner", "partner_totals"),
        };
        let p = from(start, Some(key), None, vec![totals(walk, filter)]);
        let got = nodes(&l, p, far).await?;
        ensure!(
            got == want,
            "walking `totals` {walk:?} from {start} `{key}` (step where {filter:?}) must reach \
             {want:?}, the records whose matched value is the same and that the step's where \
             holds for; it reaches {got:?}"
        );
    }
    Ok(())
}

async fn a_view_field_that_asks_to_be_searched_or_embedded_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), None).await?;
    let mut searchable = partner_totals("searched_totals", StorageKind::Decimal);
    searchable.fields[0].searchable = true;
    let mut vectored = partner_totals("vector_totals", StorageKind::Decimal);
    vectored.fields.push(vector("near", 3));
    let mut embedded = partner_totals("embedded_totals", StorageKind::Decimal);
    embedded
        .fields
        .push(embedding("near", 3, &["partner"], "m"));
    for (what, ty, name) in [
        ("is searchable", searchable, "partner"),
        ("is a vector", vectored, "near"),
        ("carries an embedding", embedded, "near"),
    ] {
        let view = ty.name.clone();
        let r = l.try_declare(ty).await;
        ensure!(
            matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains(view.as_str()) && message.contains(name)),
            "a view whose field `{name}` {what} must be refused as invalid, naming `{view}` \
             and `{name}`; declare answered {r:?}"
        );
    }
    Ok(())
}

async fn a_trigger_or_object_set_on_a_view_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::write::Declaration;
    let l = invoiced(store.as_ref(), Some("pt")).await?;
    let Lake { cat, own } = &l;
    let trigger = |on: &str| trigger_on("grew", on, Some("total > 0"));
    let set = object_set_on("busy", "pt", "total > 0");
    let names_both =
        |r: &lakelands_api::Result<lakelands_api::write::Outcome>, what: &str, on: &str| {
            matches!(r, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
            if message.contains(what) && message.contains(on))
        };
    let before = store.head().await.must("reading the head")?;
    let r = cat.declare(own, trigger("pt")).await;
    ensure!(
        names_both(&r, "grew", "pt"),
        "a trigger on the view `pt` can never fire and must be refused as invalid, naming \
         `grew` and `pt`; declare answered {r:?}"
    );
    let r = cat.declare(own, set.clone()).await;
    ensure!(
        names_both(&r, "busy", "pt"),
        "an object set over the view `pt` must be refused as invalid, naming `busy` and \
         `pt`; declare answered {r:?}"
    );
    // The view and its trigger in one set: refused as a whole.
    let both = vec![
        Declaration::Type(partner_totals("pt2", StorageKind::Decimal)),
        trigger("pt2"),
    ];
    let r = cat.apply(own, both).await;
    ensure!(
        names_both(&r, "grew", "pt2"),
        "a trigger on a view declared in the same set must be refused, naming `grew` and \
         `pt2`; apply answered {r:?}"
    );
    let r = cat.apply(own, vec![set]).await;
    ensure!(
        names_both(&r, "busy", "pt"),
        "an object set over the view `pt` must be refused by apply too; it answered {r:?}"
    );
    head_unmoved(store.as_ref(), before, "every declaration on a view").await
}

async fn a_relation_on_a_view_is_derived_or_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::write::Declaration;
    let l = invoiced(store.as_ref(), None).await?;
    let Lake { cat, own } = &l;
    let with = |relation: RelationDecl| {
        let mut view = partner_totals("pt", StorageKind::Decimal);
        view.relations = vec![relation];
        view
    };
    let linked = RelationDecl {
        cardinality: Cardinality::ManyToMany,
        ..RelationDecl::to("seen", "invoice")
    };
    let carried = RelationDecl {
        via: Some("partner".into()),
        ..RelationDecl::to("by", "invoice")
    };
    let names_both = |r: &lakelands_api::Result<lakelands_api::write::Outcome>, relation: &str| {
        matches!(reason(r), Some(Refusal::Invalid { message })
            if message.contains("pt") && message.contains(relation))
    };
    let before = store.head().await.must("reading the head")?;
    for (relation, name) in [(linked.clone(), "seen"), (carried, "by")] {
        let r = cat.declare(own, Declaration::Type(with(relation))).await;
        ensure!(
            names_both(&r, name),
            "a relation `{name}` on the view `pt` that is not derived could never hold an edge \
             and must be refused as invalid, naming `pt` and `{name}`; declare answered {r:?}"
        );
    }
    let r = cat.apply(own, vec![Declaration::Type(with(linked))]).await;
    ensure!(
        names_both(&r, "seen"),
        "apply must refuse the link relation `seen` on the view `pt` too; it answered {r:?}"
    );
    let what = "every relation on a view not derived";
    head_unmoved(store.as_ref(), before, what).await?;
    // A relation derived from matching values is taken.
    let derived = RelationDecl {
        derived: Some(Derived {
            pairs: vec![MatchPair {
                from: "partner".into(),
                to: "partner".into(),
            }],
        }),
        ..RelationDecl::to("billed", "invoice")
    };
    l.declare(with(derived)).await?;
    Ok(())
}

async fn a_view_reading_a_field_its_source_lacks_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::write::Declaration;
    let l = invoiced(store.as_ref(), None).await?;
    let Lake { cat, own } = &l;
    l.declare(type_decl(
        "credit",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
        ],
    ))
    .await?;
    let where_nope = {
        let mut t = partner_totals("pt_where", StorageKind::Decimal);
        t.view.as_mut().expect("a view").r#where = Some("nope > 0".into());
        t
    };
    let measure_nope = {
        let mut t = partner_totals("pt_measure", StorageKind::Decimal);
        t.view.as_mut().expect("a view").measures[0] = measure("total", "sum(nope)");
        t
    };
    let merged_nope = {
        let mut t = partner_totals("pt_merged", StorageKind::Decimal);
        t.fields.push(field("credited", StorageKind::Decimal));
        t.view.as_mut().expect("a view").merge = vec![ViewSource {
            source: "credit".into(),
            r#where: None,
            group_by: vec![FieldName::from("partner")],
            measures: vec![measure("credited", "sum(nope)")],
        }];
        t
    };
    let before = store.head().await.must("reading the head")?;
    for (view, source) in [
        (where_nope, "invoice"),
        (measure_nope, "invoice"),
        (merged_nope, "credit"),
    ] {
        let name = view.name.clone();
        for r in [
            cat.declare(own, Declaration::Type(view.clone())).await,
            cat.apply(own, vec![Declaration::Type(view)]).await,
        ] {
            ensure!(
                matches!(reason(&r), Some(Refusal::Invalid { message })
                    if message.contains(name.as_str()) && message.contains("nope")
                        && message.contains(source)),
                "the view `{name}` reads `nope`, which `{source}` does not have, and must be \
                 refused as invalid, naming `{name}`, `{source}` and `nope`; it answered {r:?}"
            );
        }
    }
    let what = "every view reading a field its source lacks";
    head_unmoved(store.as_ref(), before, what).await?;
    // A measure that reads an earlier one by name reads no source field.
    let mut by_name = partner_totals("pt_by_name", StorageKind::Decimal);
    by_name.fields.push(field("average", StorageKind::Decimal));
    let v = by_name.view.as_mut().expect("a view");
    v.measures.push(measure("average", "total / invoices"));
    l.declare(by_name).await?;
    Ok(())
}

async fn a_search_on_a_view_is_refused_naming_it(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    use lakelands_api::query::{Search, SearchMode};
    let l = invoiced(store.as_ref(), Some("partner_totals")).await?;
    let Lake { cat, own } = &l;
    write_invoice(&l, "i1", "p1", "10.00").await?;
    let near = || (FieldName::from("total"), vec![1.0, 0.0, 0.0]);
    for mode in [
        SearchMode::lexical("p1"),
        SearchMode::vector(near().0, near().1),
        SearchMode::Hybrid {
            text: "p1".into(),
            fields: None,
            field: near().0,
            vector: Some(near().1),
            weight: None,
            text_weight: None,
        },
    ] {
        let what = format!("{mode:?}");
        let s = Search {
            ty: "partner_totals".into(),
            ..doc_search(mode)
        };
        let r = cat.search(own, s).await;
        ensure!(
            matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains("partner_totals") && message.contains("view")),
            "a search on the view `partner_totals` ({what}) must be refused as invalid, \
             naming it as a view; it answered {r:?}. An empty answer looks exactly like no \
             match"
        );
    }
    Ok(())
}

/// A view over `source`, counting its records by partner.
fn counted(name: &str, source: &str) -> TypeDecl {
    count_view(name, source, "partner")
}

async fn a_view_closing_a_cycle_over_source_is_refused_naming_the_chain(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::write::Declaration;
    let l = invoiced(store.as_ref(), None).await?;
    let Lake { cat, own } = &l;
    write_invoice(&l, "i1", "p1", "10.00").await?;
    l.declare(counted("view_a", "invoice")).await?;
    l.declare(counted("view_b", "view_a")).await?;
    l.declare(counted("view_c", "view_b")).await?;
    for (reads, chain) in [
        ("view_a", vec!["view_a"]),
        ("view_b", vec!["view_a", "view_b"]),
        ("view_c", vec!["view_a", "view_b", "view_c"]),
    ] {
        let r = l.try_declare(counted("view_a", reads)).await;
        ensure!(
            matches!(reason(&r), Some(Refusal::Invalid { message }) if chain.iter().all(|v| message.contains(&format!("`{v}`")))),
            "`view_a` declared to read `{reads}` closes a cycle of {} view(s); it must be \
             refused as invalid, naming {chain:?}; declare answered {r:?}",
            chain.len()
        );
    }
    let set = vec![
        Declaration::Type(counted("view_x", "view_y")),
        Declaration::Type(counted("view_y", "view_x")),
    ];
    let r = cat.apply(own, set).await;
    ensure!(
        matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains("`view_x`") && message.contains("`view_y`")),
        "a set whose new views `view_x` and `view_y` read each other must be refused as \
         invalid, naming both; apply answered {r:?}"
    );
    let got = view_rows(&l, "view_c").await?;
    ensure!(
        got.len() == 1,
        "the refused declarations change nothing: `view_c` still reads `view_b`, which reads \
         `view_a`, which reads the invoices, and holds one partner; it holds {got:?}"
    );
    Ok(())
}

async fn a_view_declares_exactly_the_fields_it_computes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), None).await?;
    let without = |name: &str| {
        let mut t = partner_totals("totals", StorageKind::Decimal);
        t.fields.retain(|f| f.name != name);
        t
    };
    let mut extra = partner_totals("totals", StorageKind::Decimal);
    extra.fields.push(field("note", StorageKind::Text));
    for (what, t, name) in [
        (
            "the measure `total` without a field",
            without("total"),
            "total",
        ),
        (
            "the group_by field `partner` without a field",
            without("partner"),
            "partner",
        ),
        ("a field `note` that nothing computes", extra, "note"),
    ] {
        let r = l.try_declare(t).await;
        ensure!(
            matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains(&format!("`{name}`")) && message.contains("`totals`")),
            "a view with {what} must be refused as invalid, naming `{name}` and the view; \
             declare answered {r:?}"
        );
    }
    l.declare(partner_totals("totals", StorageKind::Decimal))
        .await?;
    Ok(())
}

async fn a_view_whose_measure_reads_another_is_described_to_its_readers(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::Access;
    let l = invoiced(store.as_ref(), None).await?;
    let Lake { cat, .. } = &l;
    let mut averaged = partner_totals("averaged", StorageKind::Decimal);
    averaged.fields.push(field("mean", StorageKind::Decimal));
    if let Some(v) = averaged.view.as_mut() {
        v.measures.push(measure("mean", "total / invoices"));
    }
    l.declare(averaged).await?;
    // A measure with the name of the source field it sums: the name inside
    // the aggregate is the source's field, and reading it is still asked.
    let fields = vec![
        field("partner", StorageKind::Text),
        field("amount", StorageKind::Decimal),
    ];
    let measures = [("amount", "sum(amount)")];
    let summed = view_of("summed", "invoice", &["partner"], fields, &measures);
    l.declare(summed).await?;
    let reader = |id: &str, amount: Access| {
        field_access(store.as_ref(), id, "invoice", &[("amount", amount)])
    };
    for (who, sees) in [
        (reader("reader", Access::Read), true),
        (reader("blind", Access::None), false),
    ] {
        let d = cat
            .describe(&who, Default::default())
            .await
            .must("describing the lake")?
            .data;
        for (view, how) in [
            (
                "averaged",
                "whose measure `mean` reads the measures `total` and `invoices` by name",
            ),
            (
                "summed",
                "whose measure `amount` sums the source field of that name",
            ),
        ] {
            let shown = d.types.iter().any(|t| t.name == view && t.view.is_some());
            ensure!(
                shown == sees,
                "`{}` reads `invoice.amount`: {}; `describe` must {} the view definition of \
                 `{view}`, {how}, and it {}",
                who.id,
                sees,
                if sees { "answer" } else { "leave out" },
                if shown { "answers it" } else { "leaves it out" }
            );
        }
    }
    Ok(())
}

/// A view over `invoice` without `group_by`, keeping what `filter` holds
/// for; `limit` a parameter, and the `credit`s merged in, when asked for.
fn ungrouped(name: &str, filter: Option<&str>, limit: bool, credits: bool) -> TypeDecl {
    let mut fields = vec![
        field("invoices", StorageKind::Number),
        field("total", StorageKind::Decimal),
        field("least", StorageKind::Decimal),
    ];
    let measures = [
        ("invoices", "count()"),
        ("total", "sum(amount)"),
        ("least", "min(amount)"),
    ];
    if credits {
        fields.push(field("credited", StorageKind::Decimal));
    }
    let mut t = view_of(name, "invoice", &[], fields, &measures);
    if let Some(v) = t.view.as_mut() {
        v.r#where = filter.map(Into::into);
        if limit {
            v.params = vec![ViewParam {
                name: "limit".into(),
                kind: StorageKind::Decimal,
            }];
        }
        if credits {
            v.merge = vec![ViewSource {
                source: "credit".into(),
                r#where: None,
                group_by: Vec::new(),
                measures: vec![measure("credited", "sum(amount)")],
            }];
        }
    }
    t
}

async fn an_ungrouped_view_holds_one_record_keyed_all_even_over_none(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), None).await?;
    let Lake { cat, own } = &l;
    l.declare(type_of("credit", &[("amount", StorageKind::Decimal)]))
        .await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "2.50"),
        ("i3", "p2", "7.25"),
    ] {
        write_invoice(&l, nr, partner, amount).await?;
    }
    l.upsert("credit", "c1", json!({"amount": "1.00"})).await?;
    for view in [
        ungrouped("everything", None, false, true),
        ungrouped("nothing", Some("amount > 1000"), false, false),
        ungrouped("up_to", Some("amount <= param('limit')"), true, false),
    ] {
        l.declare(view).await?;
    }
    // A record as (key, invoices, total, least, credited).
    let show = |r: &Record| {
        (
            r.key.to_string(),
            value(r, "invoices").and_then(Value::as_f64),
            decimal(value(r, "total")),
            decimal(value(r, "least")),
            decimal(value(r, "credited")),
        )
    };
    let one = |n: f64, total: &str, least: Option<&str>, credited: Option<&str>| {
        vec![(
            UNGROUPED.to_owned(),
            Some(n),
            Some(dec(total)),
            least.map(dec),
            credited.map(dec),
        )]
    };
    let limit = Find {
        params: [("limit".into(), json!("8.00"))].into(),
        ..Find::default()
    };
    for (ty, find, want, what) in [
        (
            "everything",
            Find::default(),
            one(3.0, "19.75", Some("2.50"), Some("1.00")),
            "over three invoices, the credit merged in,",
        ),
        (
            "nothing",
            Find::default(),
            one(0.0, "0", None, None),
            "whose `where` keeps no invoice (a count and a sum of none are 0, a minimum null)",
        ),
        (
            "up_to",
            limit,
            one(2.0, "9.75", Some("2.50"), None),
            "read with `limit` 8.00 (i2 and i3)",
        ),
    ] {
        let got: Vec<_> = l.all_found(ty, find).await?.iter().map(show).collect();
        ensure!(
            got == want,
            "a view without `group_by` {what} holds one record, {want:?}; it holds {got:?}"
        );
    }
    let got = show(&l.read("everything", UNGROUPED).await?);
    ensure!(
        vec![got.clone()] == one(3.0, "19.75", Some("2.50"), Some("1.00")),
        "`get` of `{UNGROUPED}` answers the record `find` holds; it answered {got:?}"
    );
    let other = cat.get(own, &"everything".into(), get_at("p1", None)).await;
    ensure!(
        matches!(other, Err(Error::NotFound { .. })),
        "a view without `group_by` holds no record but `{UNGROUPED}`, so `get` of `p1` is \
         not_found; it answered {other:?}"
    );
    Ok(())
}

async fn every_door_that_writes_or_holds_a_record_refuses_a_view_alike(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = invoiced(store.as_ref(), Some("pt")).await?;
    let Lake { cat, own } = &l;
    write_invoice(&l, "i1", "p1", "10.00").await?;
    let (pt, p1) = (TypeName::from("pt"), lakelands_api::RecordKey::from("p1"));
    let opts = lakelands_api::write::WriteOpts::default;
    let fields = || crate::conformance::fixture::patch(json!({"total": "1"}));
    let proposed = |write| cat.propose(own, write, "why".into());
    let head = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    let mut must = |what: &str, view: &str, got: lakelands_api::Result<()>| match &got {
        Err(
            e @ Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            },
        ) if e.to_string().contains(&format!("`{view}`")) => {}
        _ => red.push(format!("{what} answered {got:?}")),
    };
    let upsert = cat.upsert(own, &pt, &p1, fields(), opts()).await;
    must("upsert", "pt", upsert.map(drop));
    let retired = cat.retire(own, &pt, &p1, opts()).await;
    must("retire", "pt", retired.map(drop));
    // Before the grants: a principal with none is told it is a view.
    let nobody = lakelands_api::Principal {
        grants: Default::default(),
        ..own.clone()
    };
    let bare = cat.retire(&nobody, &pt, &p1, opts()).await;
    must("retire with no grant", "pt", bare.map(drop));
    let r = "r".into();
    let link = cat.link(own, &pt, &p1, &r, &p1, fields(), opts()).await;
    must("link", "pt", link.map(drop));
    let unlinked = cat.unlink(own, &pt, &p1, &r, &p1, opts()).await;
    must("unlink", "pt", unlinked.map(drop));
    let fed = vec![source_record("pt", "p1", json!({"total": "1"}))];
    must("ingest", "pt", l.ingest("feed", fed).await.map(drop));
    use lakelands_api::write::ProposedWrite::{Retire, Upsert};
    let up = proposed(Upsert {
        ty: pt.clone(),
        key: p1.clone(),
        patch: fields(),
        opts: opts(),
    });
    must("propose upsert", "pt", up.await.map(drop));
    let down = proposed(Retire {
        ty: pt.clone(),
        key: p1.clone(),
        opts: opts(),
    });
    must("propose retire", "pt", down.await.map(drop));
    let restricted = store.restrict(&pt, &p1, "hold".into()).await;
    must("restrict", "pt", restricted.map(drop));
    let unrestricted = store.unrestrict(&pt, &p1).await;
    must("unrestrict", "pt", unrestricted.map(drop));
    let verb = json!({"declare": "verb", "name": "nudge", "on": "pt",
                      "risk": "low", "effect": "external", "executor": "x"});
    let verb = serde_json::from_value(verb).expect("a verb declaration");
    let on_record = cat.declare(own, verb).await;
    match &on_record {
        Err(
            e @ Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            },
        ) if e.to_string().contains("`nudge`") && e.to_string().contains("`pt`") => {}
        _ => red.push(format!(
            "declaring `nudge` on a record of `pt` answered {on_record:?}"
        )),
    }
    ensure!(
        red.is_empty(),
        "every door that writes or holds a record refuses a view as invalid naming it: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), head, "every refusal of a view").await?;
    l.declare_json(
        json!({"declare": "verb", "name": "announce", "on": "pt", "scope": "type",
                          "risk": "low", "effect": "external", "executor": "x"}),
    )
    .await?;
    Ok(())
}
