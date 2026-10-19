//! A source's contribution is retracted as one
//! (`docs/decisions/039-a-source-is-retracted-as-one.md`).
//!
//! `purge_source` retires every live record a source brought in, across
//! every type it reached, in one append, as `retire` would retire each; a
//! record another hand made or also wrote refuses the whole purge.

use lakelands_api::error::{Case, RecordName};
use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{Authority, Cardinality, RelationDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult, Walk};
use lakelands_api::seam::AGING_RULE_TYPE;
use lakelands_api::write::{Declaration, ObjectSetDecl, PurgeReference, PurgeReport};
use lakelands_api::{
    Error, LakeStore, Origin, Position, Refusal, SourceId, Timestamp, TypeName, WriteOrigin,
};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, computed_by, count_view, derived, events_after, field, get_at, head_unmoved,
    ingest, invalid_case, pattern, refused_like, refused_on, source_record, sourced, step,
    trigger_on, type_decl,
};
use crate::conformance::{Failure, Held, Store};
use crate::links::{link_key, link_type_name};

check!(
    RETIRES_WHAT_IT_BROUGHT = a_purge_retires_every_record_the_source_brought_in_and_no_other,
    "039: a purge retires, in one append, every live record the source brought in across every type it reached, and leaves other sources' and authored records; the report counts per type"
);

check!(
    KEEPS_HISTORY = a_purged_record_is_still_read_as_of_before_the_purge,
    "039 and 007: a purge retires, it does not erase: a read as of before it sees the records, and each retirement continues the record's version"
);

check!(
    CO_OWNED_REFUSES = a_record_another_hand_also_wrote_refuses_the_whole_purge,
    "039 and 058: a record the source brought in that another hand made, a principal or another source, though the source wrote over every value of it, or that one also wrote, refuses the whole purge and its preflight as invalid, naming it, and nothing is appended; forgetting the source keeps it, its preflight naming it co-owned, and retires the records the source made and alone brought"
);

check!(
    LAKES_OWN_VALUES = derived_and_system_values_make_no_owner,
    "039: a derived value and a value the lake itself wrote (an aging move) make no owner; a cleared authored field co-owns nothing"
);

check!(
    RESTRICTED_PURGED = a_restricted_record_is_purged_too,
    "039: a purge is the host's act over its lake, so a record under a retention hold is retired with the rest"
);

check!(
    UNSEARCHED = a_type_that_keeps_no_provenance_is_named_and_not_searched,
    "039 and 010: a type declared without per-field provenance cannot say who brought its records in; the purge names it as unsearched and leaves its records"
);

check!(
    SECOND_PURGE = a_second_purge_appends_nothing_and_the_feed_can_come_back,
    "039 and log-format §3: a second purge of the same source retires nothing and appends nothing; the same content ingested after a purge is a new ingest"
);

check!(
    REACTS_AS_RETIRE = each_purged_record_reacts_as_a_retire_does_in_the_same_append,
    "039 and 004: each retirement of a purge is written by `system:lakelands`, origin `system`, context `direct`, and fires the triggers and leaves the object sets a `retire` would, in the same append, caused by that retirement"
);

check!(
    LINKS_STAY = another_sources_links_to_a_purged_record_stay_and_the_walk_skips_it,
    "039, 035 and 032: the purged source's own link records are retired with it; link records another source brought in, pointing at purged records, stay, and a walk skips the retired end"
);

check!(
    CLEARED_STAYS = a_record_whose_values_from_the_source_were_all_cleared_stays,
    "039: whose a record is, is read over the fields that hold a value: a record whose values from the source were all cleared holds nothing of it and stays, whether the source brought it in or set and cleared a value on another source's record, while a record still holding one is purged"
);

/// The key a record named by a refusal has.
fn key_of(r: &RecordName) -> &str {
    match r {
        RecordName::Of { key, .. } | RecordName::FieldOf { key, .. } => key.as_str(),
        _ => "",
    }
}

/// What a purge refused whole as shared with another hand names: how
/// many records, and the keys of the first.
fn shared(e: &Error) -> Option<(usize, Vec<&str>)> {
    match invalid_case(e) {
        Some(Case::PurgeShared { count, first, .. }) => {
            Some((*count, first.iter().map(key_of).collect()))
        }
        _ => None,
    }
}

/// `partner` (a sourced `name`, an authored `note`) and `invoice` (a
/// sourced `amount`, a shared `status`, a derived `gross`, and the source
/// relation `approved_by` to `partner`); `crm` brings in partners p1 and p2
/// and the invoice c1, `erp` the invoice i1.
async fn lake(store: &dyn LakeStore) -> Held {
    let l = Lake::of(store);
    l.declare(type_decl(
        "partner",
        vec![
            sourced("name", StorageKind::Text),
            field("note", StorageKind::Text),
        ],
    ))
    .await?;
    let mut status = field("status", StorageKind::Text);
    status.authority = Authority::Shared;
    let gross = derived("gross", StorageKind::Number, "amount * 2", &["amount"]);
    let mut invoice = type_decl(
        "invoice",
        vec![sourced("amount", StorageKind::Number), status, gross],
    );
    invoice.relations = vec![RelationDecl {
        name: "approved_by".into(),
        target: "partner".into(),
        cardinality: Cardinality::ManyToOne,
        authority: Authority::Source,
        ..Default::default()
    }];
    l.declare(invoice).await?;
    feed(
        &l,
        "crm",
        vec![
            source_record("partner", "p1", json!({"name": "Harbor"})),
            source_record("partner", "p2", json!({"name": "Cedar"})),
            source_record("invoice", "c1", json!({"amount": 1, "status": "open"})),
        ],
    )
    .await?;
    feed_one(
        &l,
        "erp",
        "invoice",
        "i1",
        json!({"amount": 5, "status": "open"}),
    )
    .await
}

async fn feed(l: &Lake, source: &str, records: Vec<lakelands_api::SourceRecord>) -> Held {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    let fut = ingest(cat, who, source, records);
    fut.await.must(&format!("ingesting from `{source}`"))?;
    Ok(())
}

/// One record of `ty` at `key` brought in from `source`.
async fn feed_one(l: &Lake, source: &str, ty: &str, key: &str, fields: Value) -> Held {
    feed(l, source, vec![source_record(ty, key, fields)]).await
}

/// A purge of `source` from `ty` alone.
async fn purge_from(
    store: &dyn LakeStore,
    source: &str,
    ty: &str,
) -> lakelands_api::Result<PurgeReport> {
    let (source, ty) = (SourceId::from(source), TypeName::from(ty));
    store.purge_source(&source, Some(&ty), None).await
}

/// A report's `references` as plain values: the type, the relation, how
/// many records, and the first of them as `<type> <key>`.
fn referring(report: &PurgeReport) -> Vec<(&str, &str, u64, Vec<String>)> {
    let of = |r: &PurgeReference| (r.first.iter().map(|f| format!("{} {}", f.ty, f.key))).collect();
    let all = report.references.iter();
    all.map(|r| (r.ty.as_str(), r.relation.as_str(), r.records, of(r)))
        .collect()
}

/// A report's `types` as plain values, in the report's order.
fn per(report: &PurgeReport) -> Vec<(&str, u32)> {
    let all = report.types.iter();
    all.map(|t| (t.ty.as_str(), t.retired)).collect()
}

async fn purge(store: &dyn LakeStore, source: &str) -> Result<PurgeReport, Failure> {
    store
        .purge_source(&SourceId::from(source), None, None)
        .await
        .must(&format!("purging `{source}`"))
}

/// Whether `ty` `key` is live at the head.
async fn live(l: &Lake, ty: &str, key: &str) -> Result<bool, Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    match cat.get(who, &TypeName::from(ty), get_at(key, None)).await {
        Ok(_) => Ok(true),
        Err(Error::NotFound { .. }) => Ok(false),
        Err(e) => Err(Failure::new(format!("reading `{ty}` `{key}`: {e}"))),
    }
}

async fn a_purge_retires_every_record_the_source_brought_in_and_no_other(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("partner", "p3", json!({"note": "ours"})).await?;
    let before = store.head().await.must("reading the head")?;
    let report = purge(store.as_ref(), "crm").await?;
    for (ty, key, want) in [
        ("partner", "p1", false),
        ("partner", "p2", false),
        ("invoice", "c1", false),
        ("invoice", "i1", true),
        ("partner", "p3", true),
    ] {
        let is = live(&l, ty, key).await?;
        ensure!(
            is == want,
            "after purging `crm`, `{ty}` `{key}` is {}, and {} was expected",
            if is { "live" } else { "retired" },
            if want { "live" } else { "retired" }
        );
    }
    let per: Vec<(String, u32)> = report
        .types
        .iter()
        .map(|t| (t.ty.to_string(), t.retired))
        .collect();
    ensure!(
        report.retired == 3 && per == [("invoice".to_owned(), 1), ("partner".to_owned(), 2)],
        "the report says {report:?}; three retired, one invoice and two partners, was expected"
    );
    let head = store.head().await.must("reading the head")?;
    let appended = events_after(store.as_ref(), before).await?;
    let retirements: Vec<Position> = appended
        .iter()
        .filter(|e| matches!(e.body, EventBody::RecordRetired { .. }))
        .map(|e| e.position)
        .collect();
    ensure!(
        retirements.len() == 3 && report.position == head,
        "the purge appended {} retirement(s), and its report names position {} with the head \
         at {head}",
        retirements.len(),
        report.position
    );
    // One append: everything after the purge began is a retirement or
    // caused by one.
    let stray: Vec<&str> = appended
        .iter()
        .filter(|e| e.position <= report.position)
        .filter(|e| {
            !matches!(e.body, EventBody::RecordRetired { .. })
                && !e.cause.is_some_and(|c| retirements.contains(&c))
        })
        .map(|e| e.body.kind())
        .collect();
    ensure!(
        stray.is_empty(),
        "the purge's append holds events that are neither a retirement nor caused by one: \
         {stray:?}"
    );
    Ok(())
}

async fn a_purged_record_is_still_read_as_of_before_the_purge(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let then = cat
        .get(own, &"partner".into(), get_at("p1", Some(before)))
        .await
        .must("reading `p1` before the purge")?
        .data;
    purge(store.as_ref(), "crm").await?;
    let still = cat
        .get(own, &"partner".into(), get_at("p1", Some(before)))
        .await
        .must("reading `p1` as of before the purge, after it")?
        .data;
    ensure!(
        still.fields == then.fields,
        "as of before the purge, `p1` reads {:?}; it read {:?}",
        still.fields,
        then.fields
    );
    let retired = events_after(store.as_ref(), before)
        .await?
        .into_iter()
        .find_map(|e| match e.body {
            EventBody::RecordRetired { key, version, .. } if key.as_str() == "p1" => Some(version),
            _ => None,
        })
        .ok_or_else(|| Failure::new("the purge wrote no retirement of `p1`"))?;
    ensure!(
        retired.0 == then.version.0 + 1,
        "`p1` was at version {}, and its retirement is version {}",
        then.version.0,
        retired.0
    );
    Ok(())
}

async fn a_record_another_hand_also_wrote_refuses_the_whole_purge(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("partner", "p1", json!({"note": "called"})).await?;
    // Another source's shared value on `crm`'s invoice.
    feed_one(&l, "erp", "invoice", "c1", json!({"status": "paid"})).await?;
    // Records another hand made, every value of which `crm` wrote over:
    // the author's `c9` and `erp`'s `i1`.
    l.upsert("invoice", "c9", json!({"status": "draft"}))
        .await?;
    for (key, amount) in [("c9", 9), ("i1", 6)] {
        let fields = json!({"amount": amount, "status": "sent"});
        feed_one(&l, "crm", "invoice", key, fields).await?;
    }
    let crm = SourceId::from("crm");
    let before = store.head().await.must("reading the head")?;
    let preflight = store.purge_preflight(&crm, None).await;
    let e = store
        .purge_source(&crm, None, None)
        .await
        .must_fail("purging `crm` while four of its records are co-owned")?;
    let named = |e: &Error| {
        shared(e).is_some_and(|(count, first)| count == 4 && first == ["c1", "c9", "i1", "p1"])
    };
    ensure!(
        matches!(&e, Error::Refused { field: None, .. })
            && named(&e)
            && preflight.as_ref().err().is_some_and(named),
        "the purge and its preflight were refused as {e:?} and {preflight:?}; invalid, naming \
         `c1`, `c9`, `i1` and `p1` and not `p2`, was expected"
    );
    head_unmoved(store.as_ref(), before, "a purge of a co-owned source").await?;
    ensure!(
        live(&l, "partner", "p2").await?,
        "the refused purge retired `p2`"
    );
    // Forgetting `crm` keeps the same four and retires `p2`.
    let pre = store
        .forget_preflight(&crm)
        .await
        .must("the forget preflight")?;
    let co: Vec<&str> = (pre.co_owned.iter())
        .flat_map(|t| t.first.iter().map(|k| k.as_str()))
        .collect();
    store
        .forget_source(&crm, None)
        .await
        .must("forgetting `crm`")?;
    let mut kept = Vec::new();
    let these = [
        ("invoice", "c1"),
        ("invoice", "c9"),
        ("invoice", "i1"),
        ("partner", "p1"),
    ];
    for (ty, key) in these.into_iter().chain([("partner", "p2")]) {
        kept.push(live(&l, ty, key).await?);
    }
    ensure!(
        co == ["c1", "c9", "i1", "p1"] && kept == [true, true, true, true, false],
        "forgetting `crm` names `c1`, `c9`, `i1` and `p1` co-owned, keeps them and retires \
         `p2`; the preflight named {co:?}, and they and `p2` live: {kept:?}"
    );
    Ok(())
}

async fn derived_and_system_values_make_no_owner(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // The lake moves `i1`'s shared status itself.
    l.upsert(
        AGING_RULE_TYPE,
        "overdue",
        json!({"on": "invoice", "field": "status", "from": "open", "to": "overdue",
               "when": "true"}),
    )
    .await?;
    let fut = store.age(Timestamp::from_date(2026, 3, 1).expect("a date"));
    fut.await.must("aging")?;
    // A person wrote a note on `p2`, then cleared it.
    l.upsert("partner", "p2", json!({"note": "x"})).await?;
    l.upsert("partner", "p2", json!({"note": null})).await?;
    let erp = purge(store.as_ref(), "erp").await?;
    ensure!(
        erp.retired == 1 && !live(&l, "invoice", "i1").await?,
        "`i1` holds a derived `gross` and an aged `status`, which make no owner, and the \
         purge of `erp` answered {erp:?}"
    );
    let crm = purge(store.as_ref(), "crm").await?;
    ensure!(
        crm.retired == 3 && !live(&l, "partner", "p2").await?,
        "`p2`'s authored note was cleared, so it co-owns nothing, and the purge of `crm` \
         answered {crm:?}"
    );
    Ok(())
}

async fn a_restricted_record_is_purged_too(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    store
        .restrict(&"partner".into(), &"p1".into(), "a retention hold".into())
        .await
        .must("restricting `p1`")?;
    let report = purge(store.as_ref(), "crm").await?;
    ensure!(
        report.retired == 3 && !live(&l, "partner", "p1").await?,
        "`p1` is under a hold and was not purged; the report says {report:?}"
    );
    Ok(())
}

async fn a_type_that_keeps_no_provenance_is_named_and_not_searched(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let line = sourced("text", StorageKind::Text);
    let mut lines = type_decl("log_line", vec![line]);
    lines.field_meta = false;
    l.declare(lines).await?;
    feed_one(&l, "crm", "log_line", "l1", json!({"text": "hello"})).await?;
    let report = purge(store.as_ref(), "crm").await?;
    ensure!(
        report.unsearched.iter().any(|t| t.as_str() == "log_line")
            && live(&l, "log_line", "l1").await?,
        "`log_line` keeps no provenance: it is named unsearched and `l1` stays; the report \
         says {report:?}"
    );
    Ok(())
}

async fn a_second_purge_appends_nothing_and_the_feed_can_come_back(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    purge(store.as_ref(), "crm").await?;
    let before = store.head().await.must("reading the head")?;
    let again = purge(store.as_ref(), "crm").await?;
    ensure!(
        again.retired == 0 && again.position == before,
        "a second purge of `crm` answered {again:?}; nothing retired at the head {before} was \
         expected"
    );
    head_unmoved(store.as_ref(), before, "a second purge").await?;
    feed_one(&l, "crm", "partner", "p1", json!({"name": "Harbor"})).await?;
    ensure!(
        live(&l, "partner", "p1").await?,
        "the same content ingested after the purge did not bring `p1` back"
    );
    Ok(())
}

async fn each_purged_record_reacts_as_a_retire_does_in_the_same_append(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let fut = cat.declare(own, trigger_on("partner_changed", "partner", None));
    fut.await.must("declaring the trigger")?;
    let fut = cat.declare(
        own,
        Declaration::ObjectSet(ObjectSetDecl {
            name: "named".into(),
            of: "partner".into(),
            filter: Condition::from("name is not null"),
            description: None,
        }),
    );
    fut.await.must("declaring the object set")?;
    // Written again after the set exists, so `p1` is in it.
    feed_one(&l, "crm", "partner", "p1", json!({"name": "Harbor Ltd"})).await?;
    let before = store.head().await.must("reading the head")?;
    purge(store.as_ref(), "crm").await?;
    let events = events_after(store.as_ref(), before).await?;
    let retired = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::RecordRetired { key, .. } if key.as_str() == "p1"))
        .ok_or_else(|| Failure::new("the purge wrote no retirement of `p1`"))?;
    ensure!(
        retired.writer == "system:lakelands"
            && retired.origin == Origin::System
            && retired.context.origin == WriteOrigin::Direct,
        "`p1`'s retirement was written by {} with origin {:?} and context {:?}; \
         `system:lakelands`, origin `system`, context `direct` was expected",
        retired.writer,
        retired.origin,
        retired.context
    );
    let caused = |f: &dyn Fn(&EventBody) -> bool| {
        events
            .iter()
            .any(|e| e.cause == Some(retired.position) && f(&e.body))
    };
    ensure!(
        caused(
            &|b| matches!(b, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "partner_changed")
        ) && caused(
            &|b| matches!(b, EventBody::ObjectSetChanged { set, joined: false, .. } if set == "named")
        ),
        "`p1`'s retirement did not fire `partner_changed` and leave `named`, caused by it, in \
         the purge's append: {:?}",
        events.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
    );
    Ok(())
}

async fn another_sources_links_to_a_purged_record_stay_and_the_walk_skips_it(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let lt = link_type_name(&"invoice".into(), &"approved_by".into());
    let key = link_key(&"i1".into(), &"p1".into());
    feed_one(
        &l,
        "registry",
        lt.as_str(),
        key.as_str(),
        json!({"from": "i1", "to": "p1"}),
    )
    .await?;
    // A link `crm` brought in itself goes with its records.
    let own_link = link_key(&"c1".into(), &"p2".into());
    feed_one(
        &l,
        "crm",
        lt.as_str(),
        own_link.as_str(),
        json!({"from": "c1", "to": "p2"}),
    )
    .await?;
    let report = purge(store.as_ref(), "crm").await?;
    ensure!(
        live(&l, lt.as_str(), key.as_str()).await?,
        "the link `registry` brought in from `i1` to `p1` was retired with `crm`'s records: \
         {report:?}"
    );
    ensure!(
        !live(&l, lt.as_str(), own_link.as_str()).await?
            && report.types.iter().any(|t| t.ty == lt && t.retired == 1),
        "the link `crm` brought in from `c1` to `p2` stayed: a purge takes the source's link \
         records with it; {report:?}"
    );
    let p = pattern(
        "invoice",
        Some("i1"),
        vec![step("approved_by", Walk::Out)],
        Projection::Graph,
    );
    let reply = cat.query(own, p).await.must("walking from `i1`")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["i1"],
        "`p1` is purged, so the walk from `i1` reaches no partner; it reached {keys:?}"
    );
    Ok(())
}

async fn a_record_whose_values_from_the_source_were_all_cleared_stays(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // `crm` clears the one value it gave `p1`; `p2` keeps its name. On
    // `erp`'s `i1`, `crm` sets `status` and clears it again: `i1` holds
    // nothing of `crm`'s, only `erp`'s `amount`.
    feed_one(&l, "crm", "partner", "p1", json!({"name": null})).await?;
    for status in [json!("disputed"), json!(null)] {
        feed_one(&l, "crm", "invoice", "i1", json!({"status": status})).await?;
    }
    let report = purge(store.as_ref(), "crm").await?;
    ensure!(
        live(&l, "partner", "p1").await?,
        "`p1` holds no value from `crm` any more, and the purge retired it: {report:?}"
    );
    ensure!(
        !live(&l, "partner", "p2").await?,
        "`p2` still holds `crm`'s name, and the purge left it: {report:?}"
    );
    let i1 = l.read("invoice", "i1").await?;
    ensure!(
        i1.fields.get("amount") == Some(&json!(5)),
        "`i1` is `erp`'s, with nothing of `crm`'s left, and the purge took it or its amount: \
         {:?}",
        i1.fields
    );
    Ok(())
}

check!(
    CO_OWNED_COUNTED = a_co_owned_purge_names_the_count_and_the_first_ten,
    "039 and seams: a purge refused because records are co-owned names how many there are, and the first ten, never more"
);

async fn a_co_owned_purge_names_the_count_and_the_first_ten(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let keys: Vec<String> = (0..12).map(|n| format!("b{n:02}")).collect();
    feed(
        &l,
        "bulk",
        keys.iter()
            .map(|k| source_record("partner", k, json!({"name": k})))
            .collect(),
    )
    .await?;
    for k in &keys {
        l.upsert("partner", k, json!({"note": "kept"})).await?;
    }
    let e = store
        .purge_source(&SourceId::from("bulk"), None, None)
        .await
        .must_fail("purging `bulk`, all twelve of whose records a person also wrote")?;
    let Some((count, named)) = shared(&e) else {
        return Err(Failure::new(format!(
            "the purge was refused as {e:?}; invalid, the purge shared, was expected"
        )));
    };
    // The store finds them in key order, so the first ten are b00 to b09.
    let first_ten: Vec<&str> = keys.iter().take(10).map(String::as_str).collect();
    ensure!(
        count == 12 && named == first_ten,
        "twelve records are co-owned: the refusal names the count, 12, and the first ten \
         in the store's order, b00 to b09; it names {count} and {named:?}: {e:?}"
    );
    Ok(())
}

check!(
    BACKED_NO_OWNER = a_value_a_backing_filled_makes_no_owner,
    "039: a backed output is the lake's own computation, like a derived value; it makes no owner, so the source's purge retires the record"
);

async fn a_value_a_backing_filled_makes_no_owner(store: Store) -> Held {
    use crate::conformance::backing::KIND;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let function: lakelands_api::write::Declaration = serde_json::from_value(json!({
        "declare": "function", "name": "lookup", "backing": KIND,
        "inputs": [{"name": "code", "type": {"kind": "text"}}],
        "config": {"token": "purge_backed", "echo": "code"}
    }))
    .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))?;
    let fut = cat.declare(own, function);
    fut.await.must("declaring `lookup`")?;
    let code = sourced("code", StorageKind::Text);
    let name = computed_by("name", StorageKind::Text, "lookup", &["code"]);
    l.declare(type_decl("place", vec![code, name])).await?;
    feed_one(&l, "geo", "place", "at", json!({"code": "AT"})).await?;
    let filled = crate::conformance::fixture::read(cat.as_ref(), own, "place", "at").await?;
    ensure!(
        filled.fields.get("name") == Some(&json!("AT")),
        "the backing fills `at`'s `name` before the purge; it reads {filled:?}"
    );
    let report = purge(store.as_ref(), "geo").await?;
    ensure!(
        report.retired == 1 && !live(&l, "place", "at").await?,
        "`at`'s `name` was filled by a backing, which makes no owner: the purge of `geo` \
         retires it, and it answered {report:?}"
    );
    Ok(())
}

check!(
    INITIAL_NO_OWNER = a_field_that_declares_initial_makes_no_owner,
    "039, extended, and 050 (lakelands#551): a field that declares `initial` makes no owner, whoever wrote it last: a record whose such field an author moved goes with its source's purge, whole or of one type, and the preflight and the purge name in `moved`, per type and field, how many such records they take and their first keys; another authored field still refuses the purge whole"
);

async fn a_field_that_declares_initial_makes_no_owner(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "order", "fields": [
        {"name": "total", "type": {"kind": "number"}, "authority": "source"},
        {"name": "state", "type": {"kind": "text"}, "initial": "new"},
        {"name": "note", "type": {"kind": "text"}}]}))
        .await?;
    let orders = ["o1", "o2", "o3"].map(|k| source_record("order", k, json!({"total": 1})));
    feed(&l, "erp", orders.into()).await?;
    let others = ["o4", "o5"].map(|k| source_record("order", k, json!({"total": 2})));
    feed(&l, "crm", others.into()).await?;
    for key in ["o1", "o4"] {
        l.upsert("order", key, json!({"state": "paid"})).await?;
    }
    let kept = json!({"state": "paid", "note": "kept"});
    l.upsert("order", "o3", kept).await?;
    let typed = purge_from(store.as_ref(), "crm", "order").await;
    let typed = typed.must("the purge of `crm` from `order`")?;
    let erp = SourceId::from("erp");
    let refused = store.purge_source(&erp, None, None).await;
    let whole = refused.as_ref().err().and_then(shared) == Some((1, vec!["o3"]));
    l.upsert("order", "o3", json!({"note": null})).await?;
    let before = store.purge_preflight(&erp, None).await;
    let before = before.must("the preflight")?;
    let report = purge(store.as_ref(), "erp").await?;
    let moved =
        |n, first| json!([{"type": "order", "field": "state", "records": n, "first": first}]);
    ensure!(
        typed.retired == 2 && json!(typed.moved) == moved(1, json!(["o4"])),
        "a typed purge asks what a whole one asks: `crm`'s purge from `order` takes `o4`, \
         whose state an author moved, and names it in `moved`; it answered {typed:?}"
    );
    ensure!(
        whole
            && report.retired == 3
            && json!(report.moved) == moved(2, json!(["o1", "o3"]))
            && before.moved == report.moved,
        "a field that declares `initial` makes no owner: `o3`'s note alone refuses the purge \
         ({whole}: {refused:?}), and without it the purge takes all three and names the two an \
         author moved, as its preflight does; it answered {report:?} after {before:?}"
    );
    Ok(())
}

check!(
    SHARED_SINCE = a_field_declared_shared_since_is_ingested_and_purged_with_its_source,
    "039 and 016 (lakelands#611): an authored field declared again as `shared`, with no migration, takes an ingest under a source, and that source's purge takes the record it brought in and leaves the records an author wrote before"
);

async fn a_field_declared_shared_since_is_ingested_and_purged_with_its_source(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("sample", vec![field("title", StorageKind::Text)]))
        .await?;
    l.upsert("sample", "s1", json!({"title": "by hand"}))
        .await?;
    let seeded = || vec![source_record("sample", "s2", json!({"title": "seeded"}))];
    let refused = ingest(l.cat.as_ref(), &l.own, "samples", seeded()).await;
    let authored = matches!(refused_on(&refused), Some(("title", Refusal::WrongDoor)));
    let mut title = field("title", StorageKind::Text);
    title.authority = Authority::Shared;
    l.declare(type_decl("sample", vec![title])).await?;
    feed(&l, "samples", seeded()).await?;
    let report = purge(store.as_ref(), "samples").await?;
    let (by_hand, seeded) = (
        live(&l, "sample", "s1").await?,
        live(&l, "sample", "s2").await?,
    );
    ensure!(
        authored && per(&report) == [("sample", 1)] && by_hand && !seeded,
        "an authored `title` refuses an ingest ({authored}: {refused:?}); declared `shared`, it \
         takes one under `samples`, whose purge retires `s2` alone and leaves `s1`, written by \
         an author before; the purge answered {report:?}, `s1` live {by_hand}, `s2` live {seeded}"
    );
    Ok(())
}

check!(
    PREFLIGHT =
        a_preflight_appends_nothing_and_a_purge_that_repeats_its_id_is_stale_once_the_lake_moved,
    "039, extended: `purge_preflight` appends nothing and answers the report the purge then answers, under an id; it is refused as the purge is for a co-owned record; a purge that repeats the id is refused as invalid, appending nothing, once the lake has moved or when the id is another source's, and otherwise purges"
);

check!(
    REFERENCES_NAMED = a_purge_names_the_records_outside_it_that_refer_into_it,
    "039, extended, 026 and 035: the preflight and the purge name, per relation, the live records the purge does not take that refer to a record it takes, by a `via` field or as a link record with an end inside, restricted ones included, by type and key only: how many, and the first ten in key order; a retired record, one the purge takes itself and a relation derived from matching values are not named, and the purge is not refused"
);

async fn a_preflight_appends_nothing_and_a_purge_that_repeats_its_id_is_stale_once_the_lake_moved(
    store: Store,
) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let (crm, erp) = (SourceId::from("crm"), SourceId::from("erp"));
    let before = store.head().await.must("reading the head")?;
    let seen = store.purge_preflight(&crm, None).await;
    let seen = seen.must("the preflight")?;
    head_unmoved(store.as_ref(), before, "a preflight").await?;
    let other = store.purge_preflight(&erp, None).await;
    let other = other.must("the preflight")?;
    ensure!(
        seen.retired == 3 && seen.position == before && seen.report_id != other.report_id,
        "the preflight of `crm` must say the three records it would retire, at the head it \
         read, under an id of its own; it answered {seen:?}, and `erp`'s {other:?}"
    );
    // Another source's id, then the lake moving, make the id stale.
    l.upsert("partner", "p9", json!({"note": "new"})).await?;
    let moved = store.head().await.must("reading the head")?;
    for (id, why) in [
        (&other.report_id, "another source's"),
        (&seen.report_id, "stale"),
    ] {
        let e = store.purge_source(&crm, None, Some(id.as_str())).await;
        let e = e.must_fail(&format!("a purge under a {why} id"))?;
        ensure!(
            matches!(invalid_case(&e), Some(Case::PurgeReportStale)),
            "a purge under a {why} preflight id must be refused as invalid, saying the report \
             is not of this purge at this head; it was refused as {e:?}"
        );
        head_unmoved(store.as_ref(), moved, "a purge under a stale id").await?;
    }
    let again = store.purge_preflight(&crm, None).await;
    let again = again.must("the preflight")?;
    let done = store.purge_source(&crm, None, Some(again.report_id.as_str()));
    let done = done.await;
    let done = done.must("a purge under its preflight's id")?;
    ensure!(
        (done.retired, &done.types, &done.references) == (3, &again.types, &again.references)
            && done.report_id.is_empty(),
        "the purge must answer what its preflight said, without an id; the preflight said \
         {again:?}, the purge {done:?}"
    );
    // A co-owned record refuses the preflight as it refuses the purge.
    l.upsert("invoice", "i1", json!({"status": "paid"})).await?;
    let e = store.purge_preflight(&erp, None).await;
    let e = e.must_fail("a preflight of `erp`, whose invoice a principal also wrote,")?;
    ensure!(
        shared(&e).is_some_and(|(_, first)| first.contains(&"i1")),
        "the preflight of a co-owned source must be refused as invalid, naming `i1`; it was \
         refused as {e:?}"
    );
    Ok(())
}

async fn a_purge_names_the_records_outside_it_that_refer_into_it(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // A `bill` names its partner in the text field `partner_id`, and is
    // matched to partners by name over a derived relation.
    let mut bill = type_decl(
        "bill",
        vec![
            field("partner_id", StorageKind::Text),
            field("name", StorageKind::Text),
        ],
    );
    let by_name = lakelands_api::ontology::Derived {
        pairs: vec![lakelands_api::ontology::MatchPair {
            from: "name".into(),
            to: "name".into(),
        }],
        filter: None,
    };
    bill.relations = vec![
        RelationDecl {
            via: Some("partner_id".into()),
            ..RelationDecl::to("billed_to", "partner")
        },
        RelationDecl {
            derived: Some(by_name),
            ..RelationDecl::to("same_name", "partner")
        },
    ];
    l.declare(bill).await?;
    // b1 and b3 name `crm`'s `p1`, b3 under a hold; b2 names it and is
    // retired; b4 names no partner and only shares `p2`'s name.
    for (key, fields) in [
        ("b1", json!({"partner_id": "p1"})),
        ("b2", json!({"partner_id": "p1"})),
        ("b3", json!({"partner_id": "p1"})),
        ("b4", json!({"name": "Cedar"})),
    ] {
        l.upsert("bill", key, fields).await?;
    }
    l.retire("bill", "b2").await?;
    crate::conformance::fixture::restrict(store.as_ref(), "bill", "b3", "hold").await?;
    // Another source's link at `p1` stays behind; `crm`'s own goes.
    let lt = link_type_name(&"invoice".into(), &"approved_by".into());
    let (theirs, own) = (
        link_key(&"i1".into(), &"p1".into()),
        link_key(&"c1".into(), &"p2".into()),
    );
    let ends = |from: &str, to: &str| json!({"from": from, "to": to});
    feed_one(
        &l,
        "registry",
        lt.as_str(),
        theirs.as_str(),
        ends("i1", "p1"),
    )
    .await?;
    feed_one(&l, "crm", lt.as_str(), own.as_str(), ends("c1", "p2")).await?;
    let want = vec![
        (
            "bill",
            "billed_to",
            2,
            vec!["bill b1".to_owned(), "bill b3".to_owned()],
        ),
        ("invoice", "approved_by", 1, vec![format!("{lt} {theirs}")]),
    ];
    let crm = SourceId::from("crm");
    let seen = store.purge_preflight(&crm, None);
    let seen = seen.await.must("the preflight of `crm`")?;
    let done = purge(store.as_ref(), "crm").await?;
    for (what, report) in [("preflight", &seen), ("purge", &done)] {
        let got = referring(report);
        ensure!(
            got == want && report.retired == 4,
            "the {what} of `crm` must take its four records and name, per relation, the \
             live records outside it that refer into it: {want:?}; it took {} and named \
             {got:?}",
            report.retired
        );
    }
    Ok(())
}

check!(
    ONE_TYPE = a_purge_that_names_a_type_retires_what_the_source_brought_into_it_alone,
    "039, extended: a purge that names a type retires, in one append, the live records the source brought into that type, and leaves the source's records of its other types and other sources' records of that type; the report names the type, counts it alone and names no type unsearched"
);

check!(
    CO_OWNED_IN_TYPE = a_co_owned_record_refuses_a_typed_purge_only_in_the_named_type,
    "039, extended: a record of another type of the source that another hand also wrote does not refuse a purge that names a type; one in the named type refuses it whole as invalid, naming it and the type, and nothing is appended"
);

check!(
    OWN_REFERENCES = a_typed_purge_names_the_sources_own_records_that_still_refer_into_it,
    "039, extended: a typed purge leaves the source's records of its other types, and its report names those that refer to a record it takes, by a `via` field, as it names any other record outside the purge"
);

check!(
    TYPED_LINKS = a_typed_purge_takes_the_links_of_the_named_types_relations_and_no_other,
    "039, extended, and 035: a typed purge takes the source's link records of the relations the named type declares with its records; the source's link records of another type's relation that end at a purged record stay and are named in the report"
);

check!(
    TYPED_ID = a_preflight_of_a_type_and_one_of_the_source_whole_answer_ids_of_their_own,
    "039, extended: at one head the preflight of a source whole and the preflight of the source from one type answer different ids; each is refused as invalid by the other's purge, appending nothing, and taken by its own"
);

check!(
    TYPE_REFUSED = a_purge_from_a_type_it_cannot_take_from_is_refused,
    "039, extended, and 010: a purge and a preflight that name a type that is not declared are not found; ones that name a view, or a type that keeps no per-field provenance, are refused as invalid saying why; nothing is appended"
);

async fn a_purge_that_names_a_type_retires_what_the_source_brought_into_it_alone(
    store: Store,
) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let report = purge_from(store.as_ref(), "crm", "invoice").await;
    let report = report.must("purging `crm` from `invoice`")?;
    for (ty, key, want) in [
        ("invoice", "c1", false),
        ("invoice", "i1", true),
        ("partner", "p1", true),
        ("partner", "p2", true),
    ] {
        ensure!(
            live(&l, ty, key).await? == want,
            "after purging `crm` from `invoice`, `{ty}` `{key}` is not {}: {report:?}",
            if want { "live" } else { "retired" }
        );
    }
    let retired = events_after(store.as_ref(), before).await?;
    let retired = (retired.iter()).filter(|e| matches!(e.body, EventBody::RecordRetired { .. }));
    ensure!(
        report.ty.as_ref().map(TypeName::as_str) == Some("invoice")
            && (report.retired, per(&report)) == (1, vec![("invoice", 1)])
            && report.unsearched.is_empty()
            && retired.count() == 1,
        "the report of `crm` from `invoice` says {report:?}; it must name the type, one \
         invoice retired by one event, and no type unsearched"
    );
    Ok(())
}

async fn a_co_owned_record_refuses_a_typed_purge_only_in_the_named_type(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("partner", "p1", json!({"note": "kept"})).await?;
    let before = store.head().await.must("reading the head")?;
    refused_as!(
        "a purge of `crm` from `partner`, whose `p1` a person also wrote,",
        purge_from(store.as_ref(), "crm", "partner").await,
        Case::PurgeShared { ty: Some(ty), first, .. }
            if ty == "partner" && first.iter().any(|r| key_of(r) == "p1")
    )?;
    head_unmoved(store.as_ref(), before, "a purge from the co-owned type").await?;
    let report = purge_from(store.as_ref(), "crm", "invoice").await;
    let report = report.must("purging `crm` from `invoice` while its partner `p1` is co-owned")?;
    ensure!(
        report.retired == 1
            && !live(&l, "invoice", "c1").await?
            && live(&l, "partner", "p1").await?,
        "a co-owned partner must not hold a purge of `crm` from `invoice`: {report:?}"
    );
    Ok(())
}

async fn a_typed_purge_names_the_sources_own_records_that_still_refer_into_it(
    store: Store,
) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // `crm` also feeds a `bill` that names its partner `p1` in a field.
    let mut bill = type_decl("bill", vec![sourced("partner_id", StorageKind::Text)]);
    bill.relations = vec![RelationDecl {
        via: Some("partner_id".into()),
        ..RelationDecl::to("billed_to", "partner")
    }];
    l.declare(bill).await?;
    feed_one(&l, "crm", "bill", "b1", json!({"partner_id": "p1"})).await?;
    let report = purge_from(store.as_ref(), "crm", "partner").await;
    let report = report.must("purging `crm` from `partner`")?;
    let want = vec![("bill", "billed_to", 1, vec!["bill b1".to_owned()])];
    ensure!(
        report.retired == 2 && referring(&report) == want && live(&l, "bill", "b1").await?,
        "a purge of `crm` from `partner` takes its two partners, leaves its bill `b1` and \
         names it as referring into the purge: {want:?}; the report says {report:?}"
    );
    Ok(())
}

async fn a_typed_purge_takes_the_links_of_the_named_types_relations_and_no_other(
    store: Store,
) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // `crm` brought in the link from its invoice `c1` to its partner `p2`:
    // a record of the link type of `invoice`'s relation.
    let lt = link_type_name(&"invoice".into(), &"approved_by".into());
    let link = link_key(&"c1".into(), &"p2".into());
    let ends = json!({"from": "c1", "to": "p2"});
    feed_one(&l, "crm", lt.as_str(), link.as_str(), ends).await?;
    let report = purge_from(store.as_ref(), "crm", "partner").await;
    let report = report.must("purging `crm` from `partner`")?;
    let want = vec![("invoice", "approved_by", 1, vec![format!("{lt} {link}")])];
    ensure!(
        live(&l, lt.as_str(), link.as_str()).await? && referring(&report) == want,
        "a purge of `crm` from `partner` leaves the link of `invoice`'s relation that ends \
         at `p2` and names it: {want:?}; the report says {report:?}"
    );
    let report = purge_from(store.as_ref(), "crm", "invoice").await;
    let report = report.must("purging `crm` from `invoice`")?;
    let both = vec![("invoice", 1), (lt.as_str(), 1)];
    ensure!(
        !live(&l, lt.as_str(), link.as_str()).await? && per(&report) == both,
        "a purge of `crm` from `invoice` takes `c1` and the link its relation carries: \
         {both:?}; the report says {report:?}"
    );
    Ok(())
}

async fn a_preflight_of_a_type_and_one_of_the_source_whole_answer_ids_of_their_own(
    store: Store,
) -> Held {
    lake(store.as_ref()).await?;
    let (crm, invoice) = (SourceId::from("crm"), TypeName::from("invoice"));
    let before = store.head().await.must("reading the head")?;
    let whole = store.purge_preflight(&crm, None).await;
    let whole = whole.must("the preflight of `crm`")?;
    let typed = store.purge_preflight(&crm, Some(&invoice)).await;
    let typed = typed.must("the preflight of `crm` from `invoice`")?;
    ensure!(
        whole.report_id != typed.report_id && (whole.retired, typed.retired) == (3, 1),
        "at one head the preflight of `crm` and the one from `invoice` must answer ids of \
         their own, for three records and for one: {whole:?} and {typed:?}"
    );
    for (ty, id) in [(None, &typed), (Some(&invoice), &whole)] {
        let what = format!("a purge of `crm` from {ty:?} under the other preflight's id");
        let id = Some(id.report_id.as_str());
        let answer = store.purge_source(&crm, ty, id).await;
        refused_as!(&what, answer, Case::PurgeReportStale)?;
        head_unmoved(store.as_ref(), before, &what).await?;
    }
    let done = store.purge_source(&crm, Some(&invoice), Some(typed.report_id.as_str()));
    let done = done.await;
    let done = done.must("the purge from `invoice` under its own preflight's id")?;
    ensure!(
        done.retired == 1 && done.ty.as_ref() == Some(&invoice),
        "the purge from `invoice` under its preflight's id answered {done:?}"
    );
    Ok(())
}

async fn a_purge_from_a_type_it_cannot_take_from_is_refused(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let mut lines = type_decl("log_line", vec![sourced("text", StorageKind::Text)]);
    lines.field_meta = false;
    l.declare(lines).await?;
    feed_one(&l, "crm", "log_line", "l1", json!({"text": "hello"})).await?;
    l.declare(count_view("by_name", "partner", "name")).await?;
    let before = store.head().await.must("reading the head")?;
    let crm = SourceId::from("crm");
    for (ty, why) in [("log_line", "no_provenance"), ("by_name", "is_a_view")] {
        let name = TypeName::from(ty);
        let what = format!("a purge of `crm` from `{ty}`");
        let case = json!({"code": why, "type": ty});
        let done = purge_from(store.as_ref(), "crm", ty).await;
        refused_like(&what, done, &case)?;
        let seen = store.purge_preflight(&crm, Some(&name)).await;
        refused_like(&format!("the preflight of {what}"), seen, &case)?;
    }
    let none = purge_from(store.as_ref(), "crm", "nothing").await;
    ensure!(
        matches!(none, Err(Error::NotFound { .. })),
        "a purge from a type that is not declared must be not found; it answered {none:?}"
    );
    let what = "a purge from a type it cannot take from";
    head_unmoved(store.as_ref(), before, what).await
}

check!(
    CARRIED_LINKS_NAMED = a_purge_from_a_type_names_the_link_types_its_relations_carry,
    "039, 044 and 035: a purge of a source from one type reads that type and the link types of the relations of it that link records carry; the migration that removes such a relation retires its links itself, so the preflight and the purge of the type take none after it, and none once the relation is declared again as derived, which has no link type"
);

async fn a_purge_from_a_type_names_the_link_types_its_relations_carry(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // `route` r1 and r2, each linked to a partner over `calls_at`; the
    // links alone come from `bridge`.
    let route = |relations| {
        let mut route = type_decl("route", vec![sourced("name", StorageKind::Text)]);
        route.relations = relations;
        route
    };
    let mut carried = RelationDecl::to("calls_at", "partner");
    carried.authority = Authority::Source;
    l.declare(route(vec![carried])).await?;
    let lt = link_type_name(&"route".into(), &"calls_at".into());
    for (from, to) in [("r1", "p1"), ("r2", "p2")] {
        feed_one(&l, "tt", "route", from, json!({"name": "Harbor"})).await?;
        let key = link_key(&from.into(), &to.into());
        let ends = json!({"from": from, "to": to});
        feed_one(&l, "bridge", lt.as_str(), key.as_str(), ends).await?;
    }
    let (bridge, ty) = (SourceId::from("bridge"), TypeName::from("route"));
    let mut taken = Vec::new();
    // The relation carried by links, removed, and declared again as one
    // derived from matching names.
    let by_name = lakelands_api::ontology::Derived {
        pairs: vec![lakelands_api::ontology::MatchPair {
            from: "name".into(),
            to: "name".into(),
        }],
        filter: None,
    };
    let derived = RelationDecl {
        derived: Some(by_name),
        ..RelationDecl::to("calls_at", "partner")
    };
    for relations in [None, Some(vec![]), Some(vec![derived])] {
        if let Some(relations) = relations {
            l.migrate(route(relations)).await?;
        }
        let seen = store.purge_preflight(&bridge, Some(&ty)).await;
        taken.push(seen.must("the preflight of `bridge` from `route`")?.retired);
    }
    let done = purge_from(store.as_ref(), "bridge", "route").await;
    taken.push(done.must("the purge of `bridge` from `route`")?.retired);
    let stand = live(&l, lt.as_str(), "r1:p1").await? || live(&l, lt.as_str(), "r2:p2").await?;
    ensure!(
        taken == [2, 0, 0, 0] && !stand,
        "a purge of `bridge` from `route` must take its two links while `calls_at` is carried \
         by links, and none once the migration that removes the relation has retired them, \
         once it is declared again as derived, and at the purge; it took {taken:?}, and a \
         link is live: {stand}"
    );
    Ok(())
}
