//! A source's values taken back by the time the source observed them
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`,
//! lakelands#1166): the events whose `seen_at` is before a time, wherever
//! in the log they stand; an event that states none is never selected,
//! and a value is shredded only where every value its key seals is
//! selected, as is every older version's.

use lakelands_api::IngestOpts;

use super::erasable::{Keys, again, first, person, with};
use super::removed::{acts, at, erased};
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    SEEN_BEFORE = a_late_delivery_of_old_observations_is_taken_back_by_the_time_seen,
    "065 (lakelands#1166): `crm` writes `s1`, seen in January, `s2`, which states no time seen, and `s3`, seen in May; then the delivery `late` writes `s4`, seen in February, and `s5`, seen at the first instant of April; the preflight of taking back what `crm` saw before April selects the events of `s1` and `s4`, names no position, counts the one event that states no time seen, and says both, while the preflight of taking back the delivery `late` counts and says none; the act appends one `values_removed` selecting by `seen_before`, with no `before`; `s1` and `s4` read their place erased and keep their notes, at the head and before the act, while `s2`, `s3` and `s5` read theirs; a rebuild answers the same; and a time past the year 9999 is refused"
);

check!(
    SEEN_SHREDDED = a_time_seen_shreds_only_where_a_key_seals_nothing_else,
    "065 (lakelands#1166): `crm` writes `p1`, seen in January, under its first version; under a second, `p2`, which states no time seen, in an ordinary delivery, then the delivery `old`, sealed apart, `p3`, seen in February, and the delivery `mixed`, sealed apart, `p4`, seen in February, and `p5`, seen in July; the preflight of taking back what `crm` saw before April selects `p1`, `p3` and `p4`, counts one event that states no time seen, names the first version, whose values it all selects, and `old`'s key, and neither the second version nor `mixed`'s key, which also seal values it leaves, and counts `name` 3 redacted, 2 shredded and 1 redacted only; with `require_shredded` the act is refused naming that count; without it the act names the first version and `old`'s key, and the key store holds the second version and `mixed`'s key; `p1`, `p3` and `p4` read their names erased and keep their notes, at the head and before the act, while `p2` and `p5` read theirs; a writer opened again and a rebuild each answer the same",
    reopens
);

check!(
    SEEN_PREFIX = a_time_seen_keeps_a_newer_version_while_an_older_seals_a_value_it_leaves,
    "065 (lakelands#1166): `crm` writes `p1`, seen in July, under its first version, and `p2`, seen in January, under a second; the preflight of taking back what `crm` saw before April selects `p2` alone and names no version: the second's values are all selected, but versions die oldest first and the first seals a value the act leaves; it counts `name` 1 redacted, none shredded and 1 redacted only; with `require_shredded` the act is refused naming that count; without it the act names no version, and the key store holds both; `p2` reads its name erased and keeps its note, at the head and before the act, while `p1` reads its own; a writer opened again and a rebuild each answer the same",
    reopens
);

/// The instant at midnight UTC of `date`.
fn day(date: &str) -> Timestamp {
    Timestamp::parse(date).expect("a date")
}

/// `r`, stating that its source saw it on `seen`, where given.
fn observed(mut r: SourceRecord, seen: Option<&str>) -> SourceRecord {
    r.seen_at = seen.map(day);
    r
}

/// Ingest `records` from `crm` in the delivery `delivery`, sealed apart
/// where `apart` says.
async fn deliver(l: &Lake, delivery: &str, apart: bool, records: Vec<SourceRecord>) -> Held {
    let opts = IngestOpts {
        delivery: Some(delivery.into()),
        sealed_apart: apart,
    };
    let crm = SourceId::from("crm");
    let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &crm, opts, records);
    said.await
        .must(&format!("ingesting the delivery `{delivery}`"))?;
    Ok(())
}

/// Whether `l` reads, as of `when`, each of `ty`'s `records` with its note
/// and `field` erased from `crm`, or holding the value given.
async fn reads(
    l: &Lake,
    (ty, field): (&str, &str),
    records: &[(&str, Option<&str>)],
    when: Option<lakelands_api::Position>,
    by: &str,
) -> Held {
    for (k, value) in records {
        let r = at(l, ty, k, when).await?;
        let holds = r.as_ref().is_some_and(|r| {
            r.fields.get("note") == Some(&json!("kept"))
                && match value {
                    None => erased(r, field, "crm"),
                    Some(v) => r.fields.get(field) == Some(&json!(v)),
                }
        });
        let want = value.map_or("erased".to_owned(), |v| format!("`{v}`"));
        ensure!(
            holds,
            "{by}, as of {when:?}, `{k}` keeps its note and reads its {field} {want}: {r:?}"
        );
    }
    Ok(())
}

async fn a_late_delivery_of_old_observations_is_taken_back_by_the_time_seen(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "stop", "key": ["id"],
        "fields": [{"name": "id", "type": text, "authority": "source"},
                   {"name": "place", "type": text, "authority": "source"},
                   {"name": "note", "type": text}]}))
        .await?;
    let s = |k: &str, place: &str, seen: Option<&str>| {
        let r = source_record("stop", k, json!({"id": k, "place": place}));
        observed(r, seen)
    };
    let first = vec![
        s("s1", "Rome", Some("2026-01-10")),
        s("s2", "Oslo", None),
        s("s3", "Kyiv", Some("2026-05-10")),
    ];
    deliver(&l, "first", false, first).await?;
    // Written after `s3`, observed before it.
    // `s5` is seen at the time itself, which is not before it.
    let late = vec![
        s("s4", "Lima", Some("2026-02-10")),
        s("s5", "Bern", Some("2026-04-01")),
    ];
    deliver(&l, "late", false, late).await?;
    for k in ["s1", "s2", "s3", "s4", "s5"] {
        l.upsert("stop", k, json!({"note": "kept"})).await?;
    }
    let head = store.head().await.must("reading the head")?;
    let crm = SourceId::from("crm");
    let april = Selection::SeenBefore(day("2026-04-01"));
    let report = store.remove_values_preflight(&crm, &april).await;
    let report = report.must("the preflight")?;
    let said = report.said();
    let selected = format!(
        "2 events of `crm` seen before {} are selected.",
        day("2026-04-01")
    );
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted))
        .collect();
    ensure!(
        report.events == 2
            && report.unseen == 1
            && report.before.is_none()
            && fields == [("place", 2)]
            && said.first() == Some(&selected)
            && said.contains(&"1 event states no time seen and stays.".to_owned()),
        "the preflight selects `s1` and `s4`, names no position and counts one event that \
         states no time seen, saying both: {report:?}"
    );
    // Only a time seen counts the events it cannot select by it.
    let late = Selection::Delivery("late".into());
    let other = store.remove_values_preflight(&crm, &late).await;
    let other = other.must("the preflight of `late`")?;
    ensure!(
        other.unseen == 0 && !other.said().iter().any(|s| s.contains("no time seen")),
        "the preflight of a delivery counts and says no event that states no time seen: {other:?}"
    );
    store
        .remove_values(&crm, &april, false, Some(&report.report_id))
        .await
        .must("taking back what crm saw before April")?;
    let named: Vec<_> = (acts(store.as_ref(), head).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ValuesRemoved {
                selection, before, ..
            } => Some((selection, before)),
            _ => None,
        })
        .collect();
    ensure!(
        named == [(april.clone(), None)],
        "one `values_removed` selects by `seen_before`, with no `before`: {named:?}"
    );
    let records = [
        ("s1", None),
        ("s2", Some("Oslo")),
        ("s3", Some("Kyiv")),
        ("s4", None),
        ("s5", Some("Bern")),
    ];
    for when in [None, Some(head)] {
        reads(&l, ("stop", "place"), &records, when, "after the act").await?;
    }
    store.rebuild().await.must("rebuilding")?;
    for when in [None, Some(head)] {
        reads(&l, ("stop", "place"), &records, when, "after a rebuild").await?;
    }
    refused_like(
        "taking back what crm saw before a time past the year 9999",
        store
            .remove_values_preflight(&crm, &Selection::SeenBefore(Timestamp(i64::MAX)))
            .await,
        &json!({"code": "not_date_time"}),
    )
}

async fn a_time_seen_shreds_only_where_a_key_seals_nothing_else(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str, seen: Option<&str>| {
        observed(
            source_record("person", k, json!({"id": k, "name": name})),
            seen,
        )
    };
    deliver(&l, "first", false, vec![p("p1", "Ada", Some("2026-01-10"))]).await?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    deliver(&l, "plain", false, vec![p("p2", "Bea", None)]).await?;
    deliver(&l, "old", true, vec![p("p3", "Cy", Some("2026-02-10"))]).await?;
    let old = keys.apart(store.lake(), &crm);
    let both = vec![
        p("p4", "Dee", Some("2026-02-10")),
        p("p5", "Eve", Some("2026-07-10")),
    ];
    deliver(&l, "mixed", true, both).await?;
    let mixed: Vec<_> = (keys.apart(store.lake(), &crm).into_iter())
        .filter(|k| !old.contains(k))
        .collect();
    let ([old], [mixed]) = (old.as_slice(), mixed.as_slice()) else {
        return Err(Failure::new(format!(
            "each delivery sealed apart makes one key: {old:?}, {mixed:?}"
        )));
    };
    for k in ["p1", "p2", "p3", "p4", "p5"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    let head = store.head().await.must("reading the head")?;
    let april = Selection::SeenBefore(day("2026-04-01"));
    let report = store.remove_values_preflight(&crm, &april).await;
    let report = report.must("the preflight")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        report.events == 3
            && report.unseen == 1
            && report.keys == [v1.clone()]
            && report.apart == [old.clone()]
            && fields == [("name", 3, 2)]
            && report.unshredded == 1,
        "the preflight selects `p1`, `p3` and `p4`, destroys {v1} and {old} alone, and counts \
         `name` 3 redacted, 2 shredded and 1 redacted only: {report:?}"
    );
    refused_like(
        "taking back what crm saw before April, shredded",
        store.remove_values(&crm, &april, true, None).await,
        &json!({"code": "remove_not_shredded", "source": "crm", "count": 1}),
    )?;
    store
        .remove_values(&crm, &april, false, None)
        .await
        .must("taking back what crm saw before April")?;
    let named: Vec<_> = (acts(store.as_ref(), head).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ValuesRemoved { keys, apart, .. } => Some((keys, apart)),
            _ => None,
        })
        .collect();
    let (ids, left) = (
        keys.ids(store.lake(), &crm)
            .must("listing crm's versions")?,
        keys.apart(store.lake(), &crm),
    );
    ensure!(
        named == [(vec![v1.clone()], vec![old.clone()])]
            && ids == [v2.clone()]
            && left == [mixed.clone()],
        "`values_removed` names {v1} and {old}, and the key store holds {v2} and {mixed}: \
         {named:?}, {ids:?}, {left:?}"
    );
    let records = [
        ("p1", None),
        ("p2", Some("Bea")),
        ("p3", None),
        ("p4", None),
        ("p5", Some("Eve")),
    ];
    let read = async |l: &Lake, by: &str| {
        for when in [None, Some(head)] {
            reads(l, ("person", "name"), &records, when, by).await?;
        }
        Ok::<_, Failure>(())
    };
    read(&l, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await
}

async fn a_time_seen_keeps_a_newer_version_while_an_older_seals_a_value_it_leaves(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str, seen: &str| {
        observed(
            source_record("person", k, json!({"id": k, "name": name})),
            Some(seen),
        )
    };
    deliver(&l, "first", false, vec![p("p1", "Ada", "2026-07-10")]).await?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    deliver(&l, "second", false, vec![p("p2", "Bea", "2026-01-10")]).await?;
    for k in ["p1", "p2"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    let head = store.head().await.must("reading the head")?;
    let april = Selection::SeenBefore(day("2026-04-01"));
    let report = store.remove_values_preflight(&crm, &april).await;
    let report = report.must("the preflight")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        report.events == 1
            && report.keys.is_empty()
            && report.apart.is_empty()
            && fields == [("name", 1, 0)]
            && report.unshredded == 1,
        "the preflight selects `p2`, destroys neither {v1} nor {v2}, and counts `name` 1 \
         redacted, none shredded and 1 redacted only: {report:?}"
    );
    refused_like(
        "taking back what crm saw before April, shredded",
        store.remove_values(&crm, &april, true, None).await,
        &json!({"code": "remove_not_shredded", "source": "crm", "count": 1}),
    )?;
    store
        .remove_values(&crm, &april, false, None)
        .await
        .must("taking back what crm saw before April")?;
    let named: Vec<_> = (acts(store.as_ref(), head).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ValuesRemoved { keys, apart, .. } => Some((keys, apart)),
            _ => None,
        })
        .collect();
    let ids = (keys.ids(store.lake(), &crm)).must("listing crm's versions")?;
    ensure!(
        named == [(vec![], vec![])] && ids == [v1.clone(), v2.clone()],
        "`values_removed` names no key, and the key store holds {v1} and {v2}: {named:?}, {ids:?}"
    );
    let records = [("p1", Some("Ada")), ("p2", None)];
    let read = async |l: &Lake, by: &str| {
        for when in [None, Some(head)] {
            reads(l, ("person", "name"), &records, when, by).await?;
        }
        Ok::<_, Failure>(())
    };
    read(&l, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await
}
