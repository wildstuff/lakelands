//! A delivery sealed apart is shredded alone
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`, lakelands#1214):
//! its values seal under the source's version composed with a key of its
//! own, an output over them under the oldest version it reads composed
//! with every such key, and taking the delivery back destroys its key and
//! no version.

use std::sync::Arc;

use lakelands_api::error::Seam;
use lakelands_api::event::{Event, EventBody, Sealed};
use lakelands_api::id::KeyId;
use lakelands_api::seam::{KeyStore, Seams};
use lakelands_api::write::Selection;
use lakelands_api::{Error, IngestOpts, LakeStore, Position, SourceId, SourceRecord};
use serde_json::json;

use super::erasable::{Keys, again, first, person, with};
use super::removed::{Whole, acts, at, erased};
use crate::conformance::fixture::{Lake, Must, events_after, events_from, source_record};
use crate::conformance::{Failure, Held, Reopen, Store, backing};

check!(
    APART_SHREDDED = a_delivery_sealed_apart_is_shredded_alone,
    "065 (lakelands#1214): `crm` writes `p1` under its first version, then the delivery `late`, sealed apart, writes `p2` and `p3`: each of its events names that version and one key of its own in `sealed.apart`, and `p1` none; after a second version `p4` is sealed under it alone; the preflight of taking `late` back names no version and that key, and counts `name` 2 redacted, 2 shredded and none redacted only; with `require_shredded` the act appends a `values_removed` naming the key in `apart` and no version, and the key store holds both versions and no key apart; `p2` reads its name erased and keeps its note, `p3`, which only `late` wrote, is retired, at the head and before the act, while `p1` and `p4`, under the older and the newer version, read Ada and Dee; a writer opened again and a rebuild each answer the same, the values erased, not lost; and forgetting `crm` destroys the key of a later delivery sealed apart with its versions",
    reopens
);

check!(
    APART_OUTPUT = an_output_over_an_apart_value_dies_with_either_key,
    "065 (lakelands#1214): `both` is computed from `name` and `nick` by the suite's backing, answering the nick; `crm` writes `c1`'s and `c2`'s names in ordinary deliveries, `c1`'s under its first version and `c2`'s under its second, and their nicks each in a delivery sealed apart under the second: the output over `c1`'s values is sealed under the first version and `nicks`'s key, `c2`'s under the second alone before `nicks2` and with `nicks2`'s key after it, and `both` reads each nick; taking `nicks2` back destroys its key alone, and `c2` holds no `both` while its name reads Bea; taking back what `crm` wrote before `nicks` destroys the first version alone, and `c1` holds no `both` while its nick reads Countess under the second version and `nicks`'s key, which the key store still holds; no call is made again, after either act, after a reopen and after a rebuild",
    reopens
);

check!(
    APART_CUT = a_cut_by_time_takes_a_delivery_sealed_apart_with_its_version,
    "065 (lakelands#1214): `crm` writes `p1`, then the delivery `late`, sealed apart, writes `p2`, both under its first version; after a second version `p3` is sealed under it alone; the preflight of taking back what `crm` wrote before `p3` names the first version and `late`'s key, and counts `name` 2 redacted, 2 shredded and none redacted only; with `require_shredded` the act appends a `values_removed` naming both, and the key store holds the second version alone and no key apart; `p1` and `p2` read their names erased and keep their notes, at the head and before the act, while `p3` reads Cy; a writer opened again and a rebuild each answer the same",
    reopens
);

check!(
    APART_OFF = an_ingest_not_sealed_apart_seals_as_before,
    "065 (lakelands#1214): an ingest that does not ask `sealed_apart` seals under the source's version alone, `sealed` naming no `apart` and the key store making no key apart; one that asks it and writes no erasable value seals nothing and asks for no key; and with a key store that makes no key apart, one that asks it and writes an erasable value is refused as unbound (`key_apart`) and appends nothing, while the same records without it are written",
    reopens
);

check!(
    APART_KEPT = a_delivery_sealed_apart_is_shredded_after_a_crash_and_an_import,
    "065 (lakelands#1214): an act taking back the delivery `late`, sealed apart, whose key store fails to destroy its key appends its event and answers the failure, the key held; the writer's next open finishes it, destroying the key, `p2` reads its name erased before the act and at the head, and a repeat appends nothing; an export made after it imports with the key store as it is, `p2` erased and `p1` read; imported beside a key store that makes no key apart, the import opens, nothing asked of it to destroy; imported with a backup of the key store from before the act, the import's finish destroys the backup's key; and imported with no key store it is refused as unbound, `p1`'s version still needed",
    given
);

/// The suite's key store, whose `destroy_apart` fails while `fails` is set.
struct FailingApart(Arc<Keys>, std::sync::atomic::AtomicBool);

impl KeyStore for FailingApart {
    fn create(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<KeyId> {
        self.0.create(lake, source)
    }
    fn key(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<Option<lakelands_api::seam::DataKey>> {
        self.0.key(lake, source, id)
    }
    fn destroy(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<()> {
        self.0.destroy(lake, source)
    }
    fn destroy_version(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        self.0.destroy_version(lake, source, id)
    }
    fn destroys_versions(&self) -> bool {
        true
    }
    fn ids(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<Vec<KeyId>> {
        self.0.ids(lake, source)
    }
    fn create_apart(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<KeyId> {
        self.0.create_apart(lake, source)
    }
    fn destroy_apart(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        if self.1.load(std::sync::atomic::Ordering::SeqCst) {
            let lake = "the key store".to_owned();
            let situation = lakelands_api::error::Situation::Busy { lake };
            return Err(Error::unavailable(situation));
        }
        self.0.destroy_apart(lake, source, id)
    }
}

/// Ingest `records` from `crm` in the delivery `delivery`, sealed apart.
async fn apart(l: &Lake, delivery: &str, records: Vec<SourceRecord>) -> Held {
    let opts = IngestOpts {
        delivery: Some(delivery.into()),
        sealed_apart: true,
    };
    let crm = SourceId::from("crm");
    let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &crm, opts, records);
    said.await
        .must(&format!("ingesting the delivery `{delivery}` sealed apart"))?;
    Ok(())
}

/// The `sealed` of each event after `after` of the kind `kind`, with its
/// record or output key.
async fn seals(store: &dyn LakeStore, after: Position) -> Result<Vec<(String, Sealed)>, Failure> {
    let events = events_after(store, after).await?;
    let sealed = |e: Event| {
        let what = match &e.body {
            EventBody::RecordIngested { key, .. } => format!("ingest {key}"),
            EventBody::OutputRecorded { seam, .. } => format!("output {seam}"),
            _ => return None,
        };
        Some((what, e.sealed?))
    };
    Ok(events.into_iter().filter_map(sealed).collect())
}

/// A seal of `crm`'s values under `key` and `apart`.
fn under(key: &KeyId, apart: &[KeyId], source: bool) -> Sealed {
    Sealed {
        key: key.clone(),
        source: source.then(|| "crm".into()),
        fields: if source { vec![] } else { vec!["name".into()] },
        apart: apart.to_vec(),
    }
}

async fn a_delivery_sealed_apart_is_shredded_alone(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    let mark = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    apart(&l, "late", vec![p("p2", "Bea"), p("p3", "Cy")]).await?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    let made = keys.apart(store.lake(), &crm);
    let [a] = made.as_slice() else {
        return Err(Failure::new(format!(
            "the delivery makes one key apart; the key store holds {made:?}"
        )));
    };
    let sealed = seals(store.as_ref(), mark).await?;
    let want = |k: &str| {
        (
            format!("ingest {k}"),
            under(&v1, std::slice::from_ref(a), false),
        )
    };
    ensure!(
        sealed
            == [
                ("ingest p1".to_owned(), under(&v1, &[], false)),
                want("p2"),
                want("p3")
            ],
        "`p1` is sealed under {v1} alone, and each event of `late` under {v1} and {a}: \
         {sealed:?}"
    );
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    let mark = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p4", "Dee")])
        .await
        .must("ingesting p4")?;
    let sealed = seals(store.as_ref(), mark).await?;
    ensure!(
        sealed == [("ingest p4".to_owned(), under(&v2, &[], false))],
        "p4 is sealed under {v2} alone: {sealed:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let late = Selection::Delivery("late".into());
    let report = store.remove_values_preflight(&crm, &late).await;
    let report = report.must("the preflight")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        report.keys.is_empty()
            && report.apart == [a.clone()]
            && fields == [("name", 2, 2)]
            && report.unshredded == 0,
        "the preflight destroys {a} and no version, and counts `name` 2 redacted, 2 shredded: \
         {report:?}"
    );
    store
        .remove_values(&crm, &late, true, None)
        .await
        .must("taking `late` back, shredded")?;
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
        named == [(vec![], vec![a.clone()])] && ids == [v1.clone(), v2.clone()] && left.is_empty(),
        "`values_removed` names {a} and no version, and the key store holds {v1} and {v2} and \
         no key apart: {named:?}, {ids:?}, {left:?}"
    );
    let read = async |l: &Lake, by: &str| {
        for when in [None, Some(head)] {
            let p2 = at(l, "person", "p2", when).await?;
            ensure!(
                p2.as_ref()
                    .is_some_and(|r| erased(r, "name", "crm")
                        && r.fields.get("note") == Some(&json!("kept"))),
                "{by}, as of {when:?}, `p2` reads its name erased and keeps its note: {p2:?}"
            );
        }
        let p3 = (
            at(l, "person", "p3", None).await?,
            at(l, "person", "p3", Some(head)).await?,
        );
        ensure!(
            p3.0.is_none() && p3.1.as_ref().is_some_and(|r| erased(r, "name", "crm")),
            "{by}, `p3` is retired, and reads its name erased before the act: {p3:?}"
        );
        for (k, name) in [("p1", "Ada"), ("p4", "Dee")] {
            let r = at(l, "person", k, None).await?;
            ensure!(
                r.as_ref()
                    .is_some_and(|r| r.fields.get("name") == Some(&json!(name))),
                "{by}, `{k}` reads {name}: {r:?}"
            );
        }
        Ok::<_, Failure>(())
    };
    read(&l, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await?;
    // Forgetting the source destroys its keys apart with its versions.
    let l = Lake::of(store.as_ref());
    apart(&l, "later", vec![p("p5", "Eve")]).await?;
    let made = keys.apart(store.lake(), &crm);
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    store
        .forget_source(&crm, Some(&report.report_id))
        .await
        .must("forgetting crm")?;
    let left = keys.apart(store.lake(), &crm);
    ensure!(
        made.len() == 1 && left.is_empty(),
        "forgetting crm destroys its key apart {made:?}: {left:?}"
    );
    Ok(())
}

async fn an_output_over_an_apart_value_dies_with_either_key(store: Store, reopen: Reopen) -> Held {
    use crate::conformance::fixture::computed_by;
    use lakelands_api::ontology::{FieldDecl, StorageKind};
    const TOKEN: &str = "apart_output";
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(
        json!({"declare": "function", "name": "pair", "backing": backing::KIND,
        "inputs": [{"name": "name", "type": text}, {"name": "nick", "type": text}],
        "config": {"token": TOKEN, "echo": "nick"}}),
    )
    .await?;
    let mut t = person();
    t.fields.push(FieldDecl {
        erasable: true,
        ..crate::conformance::fixture::sourced("nick", StorageKind::Text)
    });
    t.fields.push(computed_by(
        "both",
        StorageKind::Text,
        "pair",
        &["name", "nick"],
    ));
    l.declare(t).await?;
    let crm: SourceId = "crm".into();
    let named = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    let nick = |k: &str, nick: &str| source_record("person", k, json!({"id": k, "nick": nick}));
    l.ingest("crm", vec![named("c1", "Ada")])
        .await
        .must("ingesting c1")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mark = store.head().await.must("reading the head")?;
    apart(&l, "nicks", vec![nick("c1", "Countess")]).await?;
    let a1 = keys.apart(store.lake(), &crm);
    let cut = (events_from(store.as_ref(), mark.next()).await?.first())
        .ok_or_else(|| Failure::new("`nicks` was logged"))?
        .known_at;
    l.ingest("crm", vec![named("c2", "Bea")])
        .await
        .must("ingesting c2")?;
    let mid = store.head().await.must("reading the head")?;
    apart(&l, "nicks2", vec![nick("c2", "Bee")]).await?;
    for k in ["c1", "c2"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    let all = keys.apart(store.lake(), &crm);
    let (a1, a2) = match (a1.as_slice(), all.as_slice()) {
        ([a1], [b1, a2]) if a1 == b1 => (a1.clone(), a2.clone()),
        _ => {
            return Err(Failure::new(format!(
                "each delivery makes one key: {all:?}"
            )));
        }
    };
    let outputs = |sealed: Vec<(String, Sealed)>| -> Vec<Sealed> {
        let output = |(w, s): (String, Sealed)| w.ends_with(":pair").then_some(s);
        sealed.into_iter().filter_map(output).collect()
    };
    let (one, two) = (
        outputs(seals(store.as_ref(), mark).await?),
        outputs(seals(store.as_ref(), mid).await?),
    );
    ensure!(
        one == [
            under(&v1, std::slice::from_ref(&a1), true),
            under(&v2, &[], true),
            under(&v2, std::slice::from_ref(&a2), true)
        ] && two == one[2..],
        "`c1`'s output is sealed under {v1} and {a1}, `c2`'s under {v2} alone before `nicks2` \
         and under {v2} and {a2} after it: {one:?}"
    );
    let both = |r: Option<lakelands_api::Record>| r.and_then(|r| r.fields.get("both").cloned());
    let filled = (
        both(at(&l, "person", "c1", None).await?),
        both(at(&l, "person", "c2", None).await?),
    );
    ensure!(
        filled == (Some(json!("Countess")), Some(json!("Bee"))),
        "before either act, `both` reads each nick: {filled:?}"
    );
    let calls = backing::made(TOKEN);
    let done = store
        .remove_values(&crm, &Selection::Delivery("nicks2".into()), true, None)
        .await
        .must("taking `nicks2` back")?;
    ensure!(
        done.keys.is_empty() && done.apart == [a2.clone()] && backing::made(TOKEN) == calls,
        "taking `nicks2` back destroys {a2} alone, and makes no call: {done:?}"
    );
    let done = store
        .remove_values(&crm, &Selection::WrittenBefore(cut), false, None)
        .await
        .must("taking back what crm wrote before `nicks`")?;
    ensure!(
        done.keys == [v1.clone()] && done.apart.is_empty() && backing::made(TOKEN) == calls,
        "taking back what crm wrote before `nicks` destroys {v1} alone, and makes no call: \
         {done:?}"
    );
    let held = (
        keys.key(store.lake(), &crm, &v1).must("asking for v1")?,
        keys.key(store.lake(), &crm, &a1).must("asking for a1")?,
        keys.key(store.lake(), &crm, &v2).must("asking for v2")?,
        keys.key(store.lake(), &crm, &a2).must("asking for a2")?,
    );
    ensure!(
        held.0.is_none() && held.1.is_some() && held.2.is_some() && held.3.is_none(),
        "the key store holds {a1} and {v2}, and neither {v1} nor {a2}"
    );
    let gone = async |store: &Store, by: &str| {
        let l = Lake::of(store.as_ref());
        let (c1, c2) = (
            at(&l, "person", "c1", None).await?,
            at(&l, "person", "c2", None).await?,
        );
        let field = |r: &Option<lakelands_api::Record>, f: &str| {
            r.as_ref().and_then(|r| r.fields.get(f).cloned())
        };
        ensure!(
            field(&c1, "both").is_none()
                && field(&c1, "nick") == Some(json!("Countess"))
                && field(&c2, "both").is_none()
                && field(&c2, "name") == Some(json!("Bea"))
                && backing::made(TOKEN) == calls,
            "{by}, neither holds `both`, `c1` reads its nick and `c2` its name, and no call is \
             made again: {c1:?}, {c2:?}, {} calls after {calls}",
            backing::made(TOKEN)
        );
        Ok::<_, Failure>(())
    };
    gone(&store, "after the acts").await?;
    let store = again(&reopen, store, with(&keys), "after the acts").await?;
    gone(&store, "after a reopen").await?;
    store.rebuild().await.must("rebuilding")?;
    gone(&store, "after a rebuild").await?;
    Ok(())
}

async fn a_cut_by_time_takes_a_delivery_sealed_apart_with_its_version(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    apart(&l, "late", vec![p("p2", "Bea")]).await?;
    for k in ["p1", "p2"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    let made = keys.apart(store.lake(), &crm);
    let [a] = made.as_slice() else {
        return Err(Failure::new(format!(
            "the delivery makes one key apart; the key store holds {made:?}"
        )));
    };
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mark = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p3", "Cy")])
        .await
        .must("ingesting p3")?;
    let cut = (events_from(store.as_ref(), mark.next()).await?.first())
        .ok_or_else(|| Failure::new("`p3` was logged"))?
        .known_at;
    let head = store.head().await.must("reading the head")?;
    let before = Selection::WrittenBefore(cut);
    let report = store.remove_values_preflight(&crm, &before).await;
    let report = report.must("the preflight")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        report.keys == [v1.clone()]
            && report.apart == [a.clone()]
            && fields == [("name", 2, 2)]
            && report.unshredded == 0,
        "the preflight destroys {v1} and {a}, and counts `name` 2 redacted, 2 shredded: \
         {report:?}"
    );
    store
        .remove_values(&crm, &before, true, None)
        .await
        .must("taking back what crm wrote before `p3`, shredded")?;
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
        named == [(vec![v1.clone()], vec![a.clone()])] && ids == [v2.clone()] && left.is_empty(),
        "`values_removed` names {v1} and {a}, and the key store holds {v2} alone and no key \
         apart: {named:?}, {ids:?}, {left:?}"
    );
    let read = async |l: &Lake, by: &str| {
        for k in ["p1", "p2"] {
            for when in [None, Some(head)] {
                let r = at(l, "person", k, when).await?;
                ensure!(
                    r.as_ref().is_some_and(|r| erased(r, "name", "crm")
                        && r.fields.get("note") == Some(&json!("kept"))),
                    "{by}, as of {when:?}, `{k}` reads its name erased and keeps its note: {r:?}"
                );
            }
        }
        let p3 = at(l, "person", "p3", None).await?;
        ensure!(
            p3.as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Cy"))),
            "{by}, `p3` reads Cy: {p3:?}"
        );
        Ok::<_, Failure>(())
    };
    read(&l, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await?;
    Ok(())
}

async fn an_ingest_not_sealed_apart_seals_as_before(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let mark = store.head().await.must("reading the head")?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let city = source_record("person", "p2", json!({"id": "p2", "city": "Rome"}));
    apart(&l, "cities", vec![city]).await?;
    let sealed = seals(store.as_ref(), mark).await?;
    ensure!(
        sealed == [("ingest p1".to_owned(), under(&v1, &[], false))]
            && keys.apart(store.lake(), &crm).is_empty(),
        "`p1` is sealed under {v1} alone, `p2` is sealed not at all, and no key apart is made: \
         {sealed:?}"
    );
    let whole = || Seams {
        keys: Some(Arc::new(Whole(keys.clone()))),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = again(
        &reopen,
        store,
        whole(),
        "with a key store that makes no key apart",
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let head = store.head().await.must("reading the head")?;
    let bea = || source_record("person", "p3", json!({"id": "p3", "name": "Bea"}));
    let opts = IngestOpts {
        sealed_apart: true,
        ..IngestOpts::default()
    };
    let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &crm, opts, vec![bea()]);
    let refused = said.await.map_err(|e| e.unwrapped().clone());
    let unmoved = store.head().await.must("reading the head")?;
    ensure!(
        matches!(&refused, Err(Error::Unbound { seam }) if **seam == Seam::KeyApart)
            && unmoved == head,
        "asking `sealed_apart` of a key store that makes no key apart is refused as unbound, \
         appending nothing: {refused:?}, the head at {unmoved} after {head}"
    );
    l.ingest("crm", vec![bea()])
        .await
        .must("ingesting p3 not sealed apart")?;
    let r = at(&l, "person", "p3", None).await?;
    ensure!(
        r.as_ref()
            .is_some_and(|r| r.fields.get("name") == Some(&json!("Bea"))),
        "`p3` reads Bea: {r:?}"
    );
    Ok(())
}

async fn a_delivery_sealed_apart_is_shredded_after_a_crash_and_an_import(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use std::sync::atomic::{AtomicBool, Ordering};
    let keys = Arc::new(Keys::default());
    let failing = Arc::new(FailingApart(keys.clone(), AtomicBool::new(false)));
    let seams = || Seams {
        keys: Some(failing.clone()),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = again(&given.reopen, store, seams(), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    apart(&l, "late", vec![p("p2", "Bea")]).await?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    let made = keys.apart(store.lake(), &crm);
    let backup = Arc::new(keys.backup());
    let before = store.head().await.must("reading the head")?;
    let late = Selection::Delivery("late".into());
    failing.1.store(true, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &late, false, None).await;
    let held = keys.apart(store.lake(), &crm);
    ensure!(
        stopped.is_err() && made.len() == 1 && held == made,
        "the act whose destroy fails answers {stopped:?}, and the key stays: {made:?}, {held:?}"
    );
    failing.1.store(false, Ordering::SeqCst);
    let store = again(&given.reopen, store, seams(), "after the stopped act").await?;
    let l = Lake::of(store.as_ref());
    let read = async |l: &Lake, by: &str| {
        let p2 = (
            at(l, "person", "p2", Some(before)).await?,
            at(l, "person", "p2", None).await?,
        );
        let p1 = at(l, "person", "p1", None).await?;
        ensure!(
            [&p2.0, &p2.1]
                .iter()
                .all(|r| r.as_ref().is_some_and(|r| erased(r, "name", "crm")))
                && p1
                    .as_ref()
                    .is_some_and(|r| r.fields.get("name") == Some(&json!("Ada"))),
            "{by}, `p2` reads its name erased before the act and at the head, and `p1` reads \
             Ada: {p2:?}, {p1:?}"
        );
        Ok::<_, Failure>(())
    };
    read(&l, "after the writer's next open").await?;
    let left = keys.apart(store.lake(), &crm);
    let head = store.head().await.must("reading the head")?;
    store
        .remove_values(&crm, &late, false, None)
        .await
        .must("repeating the act")?;
    let n = acts(store.as_ref(), before).await?.len();
    let unmoved = store.head().await.must("reading the head")?;
    ensure!(
        left.is_empty() && n == 1 && unmoved == head,
        "the open destroys the key, and a repeat appends nothing: {left:?}, {n} acts, the head \
         at {unmoved} after {head}"
    );
    let base = given.scratch.clone();
    let _ = std::fs::remove_dir_all(&base);
    crate::export::write(store.as_ref(), &base)
        .await
        .must("exporting")?;
    let lake = store.lake().clone();
    let import = |seams| (given.import)(base.clone(), lake.clone(), None, seams);
    let opened = async |seams: Seams, what: &str| {
        let imported = import(seams.clone()).await;
        let imported = imported.map_err(|e| Failure::new(format!("importing {what}: {e}")))?;
        let open = (imported.writer)(imported.store, seams).await;
        open.map_err(|e| Failure::new(format!("opening the import {what}: {e}")))
    };
    let now = opened(with(&keys), "with the key store").await?;
    read(&Lake::of(now.as_ref()), "imported with the key store").await?;
    let whole = Seams {
        keys: Some(Arc::new(Whole(keys.clone()))),
        backings: backing::backings(),
        ..Seams::default()
    };
    let beside = opened(whole, "beside a key store that makes no key apart").await?;
    read(&Lake::of(beside.as_ref()), "imported beside it").await?;
    let restored = opened(with(&backup), "with a backup").await?;
    read(&Lake::of(restored.as_ref()), "imported with a backup").await?;
    let kept = backup.apart(store.lake(), &crm);
    ensure!(
        kept.is_empty(),
        "the import's finish destroys the backup's key apart: {kept:?}"
    );
    let keyless = import(Seams::default()).await.map(drop);
    ensure!(
        matches!(&keyless, Err(Error::Unbound { seam }) if **seam == Seam::KeyStore),
        "imported with no key store, `p1`'s version is still needed: {keyless:?}"
    );
    Ok(())
}
