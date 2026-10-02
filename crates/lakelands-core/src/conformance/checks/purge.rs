//! A source's contribution is retracted as one
//! (`docs/decisions/039-a-source-is-retracted-as-one.md`).
//!
//! `purge_source` retires every live record a source brought in, across
//! every type it reached, in one append, as `retire` would retire each; a
//! record another hand also wrote refuses the whole purge.

use std::sync::Arc;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{Authority, Cardinality, RelationDecl, StorageKind, TriggerDecl};
use lakelands_api::query::{Projection, QueryResult, Walk};
use lakelands_api::seam::{AGING_RULE_TYPE, FeedFilter};
use lakelands_api::write::{Declaration, ObjectSetDecl, PurgeReport};
use lakelands_api::{
    Error, LakeStore, Origin, Position, Refusal, SourceId, Timestamp, TypeName, WriteOrigin,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field, get_at, head_unmoved, ingest, pattern, reason, source_record, step,
    type_decl,
};
use crate::links::{link_key, link_type_name};

check!(
    /// A purge retires what the source brought in, and nothing else.
    RETIRES_WHAT_IT_BROUGHT = a_purge_retires_every_record_the_source_brought_in_and_no_other,
    "039: a purge retires, in one append, every live record the source brought in across every type it reached, and leaves other sources' and authored records; the report counts per type"
);

check!(
    /// A purge keeps history.
    KEEPS_HISTORY = a_purged_record_is_still_read_as_of_before_the_purge,
    "039 and 007: a purge retires, it does not erase: a read as of before it sees the records, and each retirement continues the record's version"
);

check!(
    /// A co-owned record refuses the whole purge.
    CO_OWNED_REFUSES = a_record_another_hand_also_wrote_refuses_the_whole_purge,
    "039: a record the source brought in that a principal or another source also wrote refuses the whole purge as invalid, naming it, and nothing is appended"
);

check!(
    /// The lake's own values make no owner.
    LAKES_OWN_VALUES = derived_and_system_values_make_no_owner,
    "039: a derived value and a value the lake itself wrote (an aging move) make no owner; a cleared authored field co-owns nothing"
);

check!(
    /// Restricted records are purged too.
    RESTRICTED_PURGED = a_restricted_record_is_purged_too,
    "039: a purge is the host's act over its lake, so a record under a retention hold is retired with the rest"
);

check!(
    /// A type without provenance is not searched.
    UNSEARCHED = a_type_that_keeps_no_provenance_is_named_and_not_searched,
    "039 and 010: a type declared without per-field provenance cannot say who brought its records in; the purge names it as unsearched and leaves its records"
);

check!(
    /// Purging twice is once; the same content ingested again comes back.
    SECOND_PURGE = a_second_purge_appends_nothing_and_the_feed_can_come_back,
    "039 and log-format §3: a second purge of the same source retires nothing and appends nothing; the same content ingested after a purge is a new ingest"
);

check!(
    /// Each retirement reacts as a `retire` does.
    REACTS_AS_RETIRE = each_purged_record_reacts_as_a_retire_does_in_the_same_append,
    "039 and 004: each retirement of a purge is written by `system:lakelands`, origin `system`, context `direct`, and fires the triggers and leaves the object sets a `retire` would, in the same append, caused by that retirement"
);

check!(
    /// Other sources' links at purged records stay.
    LINKS_STAY = another_sources_links_to_a_purged_record_stay_and_the_walk_skips_it,
    "039, 035 and 032: the purged source's own link records are retired with it; link records another source brought in, pointing at purged records, stay, and a walk skips the retired end"
);

check!(
    /// A record the source left no value in stays.
    CLEARED_STAYS = a_record_whose_values_from_the_source_were_all_cleared_stays,
    "039: whose a record is, is read over the fields that hold a value: a record whose values from the source were all cleared holds nothing of it and stays, whether the source brought it in or set and cleared a value on another source's record, while a record still holding one is purged"
);

/// `partner` (a sourced `name`, an authored `note`) and `invoice` (a
/// sourced `amount`, a shared `status`, a derived `gross`, and the source
/// relation `approved_by` to `partner`); `crm` brings in partners p1 and p2
/// and the invoice c1, `erp` the invoice i1.
async fn lake(store: &dyn LakeStore) -> Result<(), Failure> {
    let l = Lake::of(store);
    let sourced = |name: &str, kind| {
        let mut f = field(name, kind);
        f.authority = Authority::Source;
        f
    };
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
    let mut gross = field("gross", StorageKind::Number);
    gross.authority = Authority::Derived;
    gross.derived = Some(Condition::from("amount * 2"));
    gross.depends_on = vec!["amount".into()];
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
    feed(
        &l,
        "erp",
        vec![source_record(
            "invoice",
            "i1",
            json!({"amount": 5, "status": "open"}),
        )],
    )
    .await
}

async fn feed(
    l: &Lake,
    source: &str,
    records: Vec<lakelands_api::SourceRecord>,
) -> Result<(), Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    ingest(cat, who, source, records)
        .await
        .must(&format!("ingesting from `{source}`"))?;
    Ok(())
}

async fn purge(store: &dyn LakeStore, source: &str) -> Result<PurgeReport, Failure> {
    store
        .purge_source(&SourceId::from(source))
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

async fn events_after(store: &dyn LakeStore, before: Position) -> Result<Vec<Event>, Failure> {
    Ok(store
        .feed()
        .poll(before.next(), &FeedFilter::default(), 1000)
        .await
        .must("reading the feed")?
        .events)
}

async fn a_purge_retires_every_record_the_source_brought_in_and_no_other(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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

async fn a_purged_record_is_still_read_as_of_before_the_purge(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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

async fn a_record_another_hand_also_wrote_refuses_the_whole_purge(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("partner", "p1", json!({"note": "called"})).await?;
    // Another source's shared value on `crm`'s invoice.
    feed(
        &l,
        "erp",
        vec![source_record("invoice", "c1", json!({"status": "paid"}))],
    )
    .await?;
    let before = store.head().await.must("reading the head")?;
    let e = store
        .purge_source(&SourceId::from("crm"))
        .await
        .must_fail("purging `crm` while two of its records are co-owned")?;
    let named = |m: &str| m.contains("p1") && m.contains("c1") && !m.contains("p2");
    ensure!(
        matches!(
            &e,
            Error::Refused { field: None, reason: Refusal::Invalid { message } } if named(message)
        ),
        "the purge was refused as {e:?}; invalid, naming `p1` and `c1` and not `p2`, was \
         expected"
    );
    head_unmoved(store.as_ref(), before, "a purge of a co-owned source").await?;
    ensure!(
        live(&l, "partner", "p2").await?,
        "the refused purge retired `p2`"
    );
    Ok(())
}

async fn derived_and_system_values_make_no_owner(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
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
    store
        .age(Timestamp::from_date(2026, 3, 1).expect("a date"))
        .await
        .must("aging")?;
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

async fn a_restricted_record_is_purged_too(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
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

async fn a_type_that_keeps_no_provenance_is_named_and_not_searched(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let mut line = field("text", StorageKind::Text);
    line.authority = Authority::Source;
    let mut lines = type_decl("log_line", vec![line]);
    lines.field_meta = false;
    l.declare(lines).await?;
    feed(
        &l,
        "crm",
        vec![source_record("log_line", "l1", json!({"text": "hello"}))],
    )
    .await?;
    let report = purge(store.as_ref(), "crm").await?;
    ensure!(
        report.unsearched.iter().any(|t| t.as_str() == "log_line")
            && live(&l, "log_line", "l1").await?,
        "`log_line` keeps no provenance: it is named unsearched and `l1` stays; the report \
         says {report:?}"
    );
    Ok(())
}

async fn a_second_purge_appends_nothing_and_the_feed_can_come_back(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    feed(
        &l,
        "crm",
        vec![source_record("partner", "p1", json!({"name": "Harbor"}))],
    )
    .await?;
    ensure!(
        live(&l, "partner", "p1").await?,
        "the same content ingested after the purge did not bring `p1` back"
    );
    Ok(())
}

async fn each_purged_record_reacts_as_a_retire_does_in_the_same_append(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    cat.declare(
        own,
        Declaration::Trigger(TriggerDecl {
            max_depth: None,
            name: "partner_changed".into(),
            on: "partner".into(),
            when: None,
            description: None,
        }),
    )
    .await
    .must("declaring the trigger")?;
    cat.declare(
        own,
        Declaration::ObjectSet(ObjectSetDecl {
            name: "named".into(),
            of: "partner".into(),
            filter: Condition::from("name is not null"),
            description: None,
        }),
    )
    .await
    .must("declaring the object set")?;
    // Written again after the set exists, so `p1` is in it.
    feed(
        &l,
        "crm",
        vec![source_record(
            "partner",
            "p1",
            json!({"name": "Harbor Ltd"}),
        )],
    )
    .await?;
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

async fn another_sources_links_to_a_purged_record_stay_and_the_walk_skips_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let lt = link_type_name(&"invoice".into(), &"approved_by".into());
    let key = link_key(&"i1".into(), &"p1".into());
    feed(
        &l,
        "registry",
        vec![source_record(
            lt.as_str(),
            key.as_str(),
            json!({"from": "i1", "to": "p1"}),
        )],
    )
    .await?;
    // A link `crm` brought in itself goes with its records.
    let own_link = link_key(&"c1".into(), &"p2".into());
    feed(
        &l,
        "crm",
        vec![source_record(
            lt.as_str(),
            own_link.as_str(),
            json!({"from": "c1", "to": "p2"}),
        )],
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

async fn a_record_whose_values_from_the_source_were_all_cleared_stays(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    // `crm` clears the one value it gave `p1`; `p2` keeps its name. On
    // `erp`'s `i1`, `crm` sets `status` and clears it again: `i1` holds
    // nothing of `crm`'s, only `erp`'s `amount`.
    feed(
        &l,
        "crm",
        vec![source_record("partner", "p1", json!({"name": null}))],
    )
    .await?;
    for status in [json!("disputed"), json!(null)] {
        feed(
            &l,
            "crm",
            vec![source_record("invoice", "i1", json!({"status": status}))],
        )
        .await?;
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
    /// A co-owned purge names how many, and the first ten.
    CO_OWNED_COUNTED = a_co_owned_purge_names_the_count_and_the_first_ten,
    "039 and seams: a purge refused because records are co-owned names how many there are, and the first ten, never more"
);

async fn a_co_owned_purge_names_the_count_and_the_first_ten(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
        .purge_source(&SourceId::from("bulk"))
        .await
        .must_fail("purging `bulk`, all twelve of whose records a person also wrote")?;
    let Some(Refusal::Invalid { message }) = reason(&e) else {
        return Err(Failure::new(format!(
            "the purge was refused as {e:?}; invalid was expected"
        )));
    };
    // The store finds them in key order, so the first ten are b00 to b09.
    let named: Vec<&String> = keys
        .iter()
        .filter(|k| message.contains(k.as_str()))
        .collect();
    let first_ten: Vec<&String> = keys.iter().take(10).collect();
    ensure!(
        message.contains("12 record(s)") && named == first_ten,
        "twelve records are co-owned: the refusal names the count, 12, and the first ten \
         in the store's order, b00 to b09; it names {named:?}: {message}"
    );
    Ok(())
}

check!(
    /// A value a backing filled makes no owner.
    BACKED_NO_OWNER = a_value_a_backing_filled_makes_no_owner,
    "039: a backed output is the lake's own computation, like a derived value; it makes no owner, so the source's purge retires the record"
);

async fn a_value_a_backing_filled_makes_no_owner(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    use crate::conformance::backing::KIND;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let function: lakelands_api::write::Declaration = serde_json::from_value(json!({
        "declare": "function", "name": "lookup", "backing": KIND,
        "inputs": [{"name": "code", "type": {"kind": "text"}}],
        "config": {"token": "purge_backed", "echo": "code"}
    }))
    .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))?;
    cat.declare(own, function)
        .await
        .must("declaring `lookup`")?;
    let mut code = field("code", StorageKind::Text);
    code.authority = Authority::Source;
    let mut name = field("name", StorageKind::Text);
    name.authority = Authority::Derived;
    name.function = Some("lookup".into());
    name.depends_on = vec!["code".into()];
    l.declare(type_decl("place", vec![code, name])).await?;
    feed(
        &l,
        "geo",
        vec![source_record("place", "at", json!({"code": "AT"}))],
    )
    .await?;
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
