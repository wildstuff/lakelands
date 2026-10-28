//! A delivery sealed apart is shredded alone
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`, lakelands#1214):
//! its values seal under the source's version composed with a key of its
//! own, an output over them under the oldest version it reads composed
//! with every such key, and taking the delivery back destroys its key and
//! no version.

use std::sync::atomic::AtomicBool;

use lakelands_api::event::Sealed;
use lakelands_api::id::KeyId;
use lakelands_api::seam::DataKey;
use lakelands_api::{IngestOpts, LakeId};

use super::erasable::{Keys, again, first, person, with};
use super::removed::{Whole, acts, at, erased};
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

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
    APART_READ = an_output_reads_the_keys_apart_of_the_events_that_wrote_its_values,
    "065 (lakelands#1423): `pair` computes `both` from `nick` and `city` by the suite's backing, answering the nick; `crm` writes `c2` with the nick Countess and the city Rome, its output sealed under `crm`'s version alone, then the delivery `nicks`, sealed apart, writes `c1`'s nick and then, in the same call, its city, which is not erasable, and an ordinary delivery writes its city again; only the two nicks' ingests are sealed, so each output over the nick takes `nicks`'s key from the event that wrote the nick: each is sealed under `crm`'s version and that key, the one over Rome not `c2`'s, so a set over `both` being Countess takes `c1` in only after `c1`'s own output is recorded, and `both` reads Countess; a writer opened again and a rebuild read the same and make no call again; taking `nicks` back destroys that key alone, and `c1` holds no `both` and no nick while its city reads Oslo, at the head, after a reopen and after a rebuild, no call made again",
    reopens
);

check!(
    APART_CUT = a_cut_by_time_takes_a_delivery_sealed_apart_with_its_version,
    "065 (lakelands#1214): `crm` writes `p1`, then the delivery `late`, sealed apart, writes `p2`, both under its first version; after a second version `p3` is sealed under it alone; the preflight of taking back what `crm` wrote before `p3` names the first version and `late`'s key, and counts `name` 2 redacted, 2 shredded and none redacted only; with `require_shredded` the act appends a `values_removed` naming both, and the key store holds the second version alone and no key apart; `p1` and `p2` read their names erased and keep their notes, at the head and before the act, while `p3` reads Cy; a writer opened again and a rebuild each answer the same",
    reopens
);

check!(
    APART_SET = a_set_forgotten_destroys_the_keys_apart_of_each_source,
    "065 and 058, extended (lakelands#1214): `crm` writes `p1` under its first version and the delivery `late`, sealed apart, writes `p2`, while `erp` writes `e1`, each record given a note by its owner; forgetting the set of `crm` and `erp` in one act appends a `source_forgotten` for each and destroys `late`'s key with both sources' versions; `p1`, `p2` and `e1` read their names erased under their sources and keep their notes, at the head and before the act; a writer opened again and a rebuild each answer the same",
    reopens
);

check!(
    APART_DELIVERIES = a_set_of_deliveries_shreds_what_it_takes_whole,
    "065 (lakelands#1290): `crm` writes `p1` in the delivery `mon` and `p2` in `tue` under its one version, and `p3` in `late`, sealed apart; the preflight of taking back `mon` alone names no version and counts its name redacted only, while the set of `mon`, `tue` and `late` names the version and `late`'s key and counts `name` 3 redacted, 3 shredded and none redacted only; with `require_shredded` the set's act destroys both, the key store holding no version of `crm` and no key apart",
    reopens
);

check!(
    APART_STREAMING = a_call_writing_past_an_act_takes_a_new_key_apart,
    "065 (lakelands#1214): an ingest of the delivery `late`, sealed apart, writes `p2` in its first window and then waits; an act taking `late` back lands before its next window, its key store failing to destroy `late`'s key, so the act answers the failure with the key held; the call then writes `p3`, whose `sealed.apart` names a new key, not the one the act named, since the act did not select it; after the writer's next open finishes the act, `p2` reads its name erased and `p3` reads Cy, as do a writer opened again and a rebuild",
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

check!(
    APART_PER_DELIVERY = a_delivery_a_record_names_is_sealed_under_a_key_of_its_own,
    "065, extended (lakelands#1528): one call of `crm` in `page`, sealed apart at a `commit_window` of 2, writes `a` and `b` in `item-1`, `c` in `item-2` and `d` in the call's, and quarantines `q` in `item-2`: three keys apart, `item-1`'s shared by `a` and `b` across two windows, and `q`'s cell sealed under `item-2`'s, as `c` is; taking `item-1` back with `require_shredded` destroys its key alone, `a` and `b` retired while `c` and `d` read their names, after a writer opened again and a rebuild too",
    reopens
);

check!(
    APART_ASKED_ONCE = a_window_asks_for_its_keys_apart_in_one_request,
    "065, extended (lakelands#1528): under a key store that counts its requests, six one-record deliveries over two sources sealed apart at a `commit_window` of 6 make one request of six keys and no other, at 2 three requests of two; a window whose keys the call keeps already makes none; where the request answers five keys for six, the sixth record's is asked alone and the six are sealed under six keys apart; where the request fails and `create_apart` refuses `unbound` at the record at index 3, that record is refused with `from` 2 and nothing of its window is appended",
    reopens
);

async fn a_delivery_a_record_names_is_sealed_under_a_key_of_its_own(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str, delivery: Option<&str>| SourceRecord {
        delivery: delivery.map(Into::into),
        ..source_record("person", k, json!({"id": k, "name": name}))
    };
    let records = vec![
        p("a", "Ada", Some("item-1")),
        p("c", "Cy", Some("item-2")),
        p("b", "Bea", Some("item-1")),
        p("d", "Dee", None),
        SourceRecord {
            delivery: Some("item-2".into()),
            ..source_record("person", "q", json!({"id": "q", "name": "Quin", "zz": 1}))
        },
    ];
    let opts = IngestOpts {
        delivery: Some("page".into()),
        sealed_apart: true,
        quarantine_refused: true,
        commit_window: Some(2),
    };
    let mark = store.head().await.must("reading the head")?;
    let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &crm, opts, records);
    said.await.must("ingesting three deliveries sealed apart")?;
    let sealed = seals(store.as_ref(), mark).await?;
    let of = |k: &str| {
        let s = sealed.iter().find(|(w, _)| *w == format!("ingest {k}"));
        s.map(|(_, s)| s.apart.clone()).unwrap_or_default()
    };
    let (a, b, c, d) = (of("a"), of("b"), of("c"), of("d"));
    let q = (sealed.iter().find(|(w, _)| w == "quarantined q")).map(|(_, s)| &s.apart);
    let made = keys.apart(store.lake(), &crm);
    ensure!(
        a.len() == 1
            && a == b
            && c.len() == 1
            && d.len() == 1
            && a != c
            && c != d
            && a != d
            && q == Some(&c)
            && made.len() == 3,
        "`a` and `b` share `item-1`'s key, `c` and `d` each have one of their own, and \
         `q` is sealed under `c`'s: \
         {sealed:?}, the key store holding {made:?}"
    );
    let item = Selection::Delivery("item-1".into());
    store
        .remove_values(&crm, &item, true, None)
        .await
        .must("taking `item-1` back, shredded")?;
    let mut left = keys.apart(store.lake(), &crm);
    left.sort();
    let mut want = [c[0].clone(), d[0].clone()];
    want.sort();
    ensure!(
        left == want,
        "taking `item-1` back destroys {a:?} alone: the key store holds {left:?}"
    );
    let read = async |l: &Lake, by: &str| {
        for k in ["a", "b"] {
            let r = at(l, "person", k, None).await?;
            ensure!(r.is_none(), "{by}, `{k}` is retired: {r:?}");
        }
        for (k, name) in [("c", "Cy"), ("d", "Dee")] {
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
    read(&Lake::of(store.as_ref()), "after a rebuild").await
}

async fn a_window_asks_for_its_keys_apart_in_one_request(store: Store, reopen: Reopen) -> Held {
    let counted = Arc::new(FailingApart::default());
    let seams = Seams {
        keys: Some(counted.clone()),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = again(&reopen, store, seams, "with a counting key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    // The record `k`, in the delivery `delivery`, of `crm` or `erp`.
    let p = |k: String, delivery: String, erp: bool| SourceRecord {
        source: erp.then(|| "erp".into()),
        delivery: Some(delivery.into()),
        ..source_record("person", &k, json!({"id": k, "name": "Ada"}))
    };
    let six = |at: &str| -> Vec<_> {
        let each = |i| p(format!("{at}{i}"), format!("{at}-{i}"), i % 2 == 1);
        (0..6).map(each).collect()
    };
    let call = async |window: u32, records: Vec<SourceRecord>| {
        crate::lock(&counted.asked).clear();
        let opts = IngestOpts {
            sealed_apart: true,
            commit_window: Some(window),
            ..IngestOpts::default()
        };
        let crm = SourceId::from("crm");
        let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &crm, opts, records);
        (said.await, crate::lock(&counted.asked).clone())
    };
    for (window, at, want) in [(6, "a", vec![Some(6)]), (2, "b", vec![Some(2); 3])] {
        let (said, asked) = call(window, six(at)).await;
        said.must(&format!("ingesting six deliveries at a window of {window}"))?;
        ensure!(
            asked == want,
            "at a window of {window} the key store is asked {want:?}: {asked:?}"
        );
    }
    let kept = (0..4).map(|i| p(format!("c{i}"), format!("c-{}", i % 2), i % 2 == 1));
    let (said, asked) = call(2, kept.collect()).await;
    said.must("ingesting two deliveries over two windows")?;
    ensure!(
        asked == [Some(2)],
        "a window whose keys the call keeps makes no request: {asked:?}"
    );
    let before = store.head().await.must("reading the head")?;
    counted.short.store(true, Ordering::SeqCst);
    let (said, asked) = call(6, six("d")).await;
    counted.short.store(false, Ordering::SeqCst);
    said.must("ingesting six deliveries whose request answers five keys")?;
    let sealed = seals(store.as_ref(), before).await?;
    let apart: BTreeSet<_> = (sealed.iter()).flat_map(|(_, s)| &s.apart).collect();
    ensure!(
        asked == [Some(6), None] && sealed.len() == 6 && apart.len() == 6,
        "a request answering five keys for six asks the sixth alone and seals each record \
         under a key apart of its own: asked {asked:?}, sealed {sealed:?}"
    );
    counted.fails.store(true, Ordering::SeqCst);
    *crate::lock(&counted.alone) = Some(3);
    let (said, _) = call(2, six("e")).await;
    let refused = matches!(
        &said,
        Err(Error::Ingested { index: 3, from: 2, error, .. })
            if matches!(**error, Error::Unbound { .. })
    );
    ensure!(
        refused,
        "the record at index 3 is refused unbound from 2: {said:?}"
    );
    for (k, there) in [("e0", true), ("e1", true), ("e2", false), ("e3", false)] {
        let r = at(&l, "person", k, None).await?;
        ensure!(r.is_some() == there, "after the refusal, `{k}` read {r:?}");
    }
    Ok(())
}

/// The suite's key store, whose `destroy_apart` and `create_apart_many`
/// fail while `fails` is set, and whose `create_apart_many` answers one
/// key short while `short` is. It counts its requests for keys apart in
/// `asked`, the size of each `create_apart_many` and `None` for each
/// `create_apart`, which refuses as unbound once `alone` is spent.
#[derive(Default)]
struct FailingApart {
    keys: Arc<Keys>,
    fails: AtomicBool,
    short: AtomicBool,
    asked: std::sync::Mutex<Vec<Option<usize>>>,
    alone: std::sync::Mutex<Option<usize>>,
}

impl FailingApart {
    fn failed(&self) -> lakelands_api::Result<()> {
        if self.fails.load(Ordering::SeqCst) {
            let lake = "the key store".to_owned();
            return Err(Error::unavailable(Situation::Busy { lake }));
        }
        Ok(())
    }
}

impl KeyStore for FailingApart {
    fn create(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        self.keys.create(lake, source)
    }
    fn key(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<Option<DataKey>> {
        self.keys.key(lake, source, id)
    }
    fn destroy(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<()> {
        self.keys.destroy(lake, source)
    }
    fn destroy_version(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        self.keys.destroy_version(lake, source, id)
    }
    fn destroys_versions(&self) -> bool {
        true
    }
    fn ids(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<Vec<KeyId>> {
        self.keys.ids(lake, source)
    }
    fn create_apart(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        crate::lock(&self.asked).push(None);
        match crate::lock(&self.alone).as_mut() {
            Some(0) => return Err(Error::unbound(Seam::KeyApart)),
            Some(n) => *n -= 1,
            None => {}
        }
        self.keys.create_apart(lake, source)
    }
    fn create_apart_many(
        &self,
        lake: &LakeId,
        sources: &[SourceId],
    ) -> lakelands_api::Result<Vec<KeyId>> {
        crate::lock(&self.asked).push(Some(sources.len()));
        self.failed()?;
        let mut ids = self.keys.create_apart_many(lake, sources)?;
        if self.short.load(Ordering::SeqCst) {
            ids.pop();
        }
        Ok(ids)
    }
    fn destroy_apart(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        self.failed()?;
        self.keys.destroy_apart(lake, source, id)
    }
}

/// Ingest `records` from `crm` in the delivery `delivery`, sealed apart.
async fn apart(l: &Lake, delivery: &str, records: Vec<SourceRecord>) -> Held {
    let opts = IngestOpts {
        delivery: Some(delivery.into()),
        sealed_apart: true,
        ..IngestOpts::default()
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
        // A quarantined record holds its seal in its payload.
        let (what, seal) = match e.body {
            EventBody::RecordIngested { key, .. } => (format!("ingest {key}"), e.sealed),
            EventBody::OutputRecorded { seam, .. } => (format!("output {seam}"), e.sealed),
            EventBody::RecordQuarantined { key, sealed, .. } => {
                (format!("quarantined {key}"), sealed)
            }
            _ => return None,
        };
        // The box aside: what is compared here is the keys a seal names.
        Some((
            what,
            Sealed {
                boxed: None,
                ..seal?
            },
        ))
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
        boxed: None,
    }
}

async fn a_delivery_sealed_apart_is_shredded_alone(store: Store, reopen: Reopen) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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

async fn an_output_reads_the_keys_apart_of_the_events_that_wrote_its_values(
    store: Store,
    reopen: Reopen,
) -> Held {
    use crate::conformance::fixture::{computed_by, sourced};
    use lakelands_api::ontology::{FieldDecl, StorageKind};
    const TOKEN: &str = "apart_read";
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(
        json!({"declare": "function", "name": "pair", "backing": backing::KIND,
        "inputs": [{"name": "nick", "type": text}, {"name": "city", "type": text}],
        "config": {"token": TOKEN, "echo": "nick"}}),
    )
    .await?;
    let mut t = person();
    t.fields.push(FieldDecl {
        erasable: true,
        ..sourced("nick", StorageKind::Text)
    });
    t.fields.push(computed_by(
        "both",
        StorageKind::Text,
        "pair",
        &["nick", "city"],
    ));
    l.declare(t).await?;
    let crm: SourceId = "crm".into();
    let c1 = |fields: Value| source_record("person", "c1", fields);
    let mark = store.head().await.must("reading the head")?;
    let c2 = source_record(
        "person",
        "c2",
        json!({"id": "c2", "nick": "Countess", "city": "Rome"}),
    );
    let set = crate::conformance::fixture::object_set_on("countess", "person", "both = 'Countess'");
    l.declare_as(set, "declaring the set `countess`").await?;
    l.ingest("crm", vec![c2]).await.must("ingesting c2")?;
    let before = store.head().await.must("reading the head")?;
    let nicks = vec![
        c1(json!({"id": "c1", "nick": "Countess"})),
        c1(json!({"id": "c1", "city": "Rome"})),
    ];
    apart(&l, "nicks", nicks).await?;
    // The plan of the city's ingest reads the nick under the key the event
    // before it names: it does not take the output recorded for `c2`'s
    // equal inputs, so `c1` joins only once its own is.
    let (mut output, mut joined) = (None, None);
    for e in events_after(store.as_ref(), before).await? {
        match &e.body {
            EventBody::OutputRecorded { seam, .. } if seam.ends_with(":pair") => {
                output = output.or(Some(e.position));
            }
            EventBody::ObjectSetChanged {
                set,
                key,
                joined: true,
                ..
            } if set == "countess" && key == "c1" => {
                joined = joined.or(Some(e.position));
            }
            _ => {}
        }
    }
    ensure!(
        output.is_some() && joined > output,
        "`c1` joins `countess` after its own output is recorded; the output is at {output:?}, \
         the join at {joined:?}"
    );
    l.ingest("crm", vec![c1(json!({"id": "c1", "city": "Oslo"}))])
        .await
        .must("ingesting c1's city")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let made = keys.apart(store.lake(), &crm);
    let [a] = made.as_slice() else {
        return Err(Failure::new(format!(
            "the delivery makes one key apart; the key store holds {made:?}"
        )));
    };
    let sealed = seals(store.as_ref(), mark).await?;
    let nick = |apart: &[KeyId]| Sealed {
        fields: vec!["nick".into()],
        ..under(&v1, apart, false)
    };
    let ingests: Vec<_> = (sealed.iter())
        .filter(|(w, _)| w.starts_with("ingest"))
        .collect();
    let want = under(&v1, std::slice::from_ref(a), true);
    let over = (sealed.iter()).filter(|(w, s)| w.ends_with(":pair") && *s == want);
    ensure!(
        ingests
            == [
                &("ingest c2".into(), nick(&[])),
                &("ingest c1".into(), nick(std::slice::from_ref(a)))
            ]
            && over.count() == 2,
        "the nicks' ingests alone are sealed, and the outputs over Rome and over Oslo are \
         each sealed under {v1} and {a}: {sealed:?}"
    );
    let calls = backing::made(TOKEN);
    let read = async |store: &Store, by: &str, nick: Option<Value>| {
        let c1 = at(&Lake::of(store.as_ref()), "person", "c1", None).await?;
        let field = |f: &str| c1.as_ref().and_then(|r| r.fields.get(f).cloned());
        ensure!(
            (field("both"), field("nick")) == (nick.clone(), nick.clone())
                && field("city") == Some(json!("Oslo"))
                && backing::made(TOKEN) == calls,
            "{by}, `c1` reads `both` and its nick as {nick:?} and its city Oslo, and no call is \
             made again: {c1:?}, {} calls after {calls}",
            backing::made(TOKEN)
        );
        Ok::<_, Failure>(())
    };
    let countess = Some(json!("Countess"));
    read(&store, "before the act", countess.clone()).await?;
    let store = again(&reopen, store, with(&keys), "before the act").await?;
    read(&store, "after a reopen", countess.clone()).await?;
    store.rebuild().await.must("rebuilding")?;
    read(&store, "after a rebuild", countess).await?;
    let done = store
        .remove_values(&crm, &Selection::Delivery("nicks".into()), true, None)
        .await
        .must("taking `nicks` back")?;
    ensure!(
        done.keys.is_empty() && done.apart == [a.clone()],
        "taking `nicks` back destroys {a} alone: {done:?}"
    );
    read(&store, "after the act", None).await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&store, "after the act and a reopen", None).await?;
    store.rebuild().await.must("rebuilding")?;
    read(&store, "after the act and a rebuild", None).await
}

async fn a_cut_by_time_takes_a_delivery_sealed_apart_with_its_version(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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

async fn a_set_forgotten_destroys_the_keys_apart_of_each_source(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let (crm, erp): (SourceId, SourceId) = ("crm".into(), "erp".into());
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    apart(&l, "late", vec![p("p2", "Bea")]).await?;
    l.ingest("erp", vec![p("e1", "Eve")])
        .await
        .must("ingesting e1")?;
    for k in ["p1", "p2", "e1"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    let made = keys.apart(store.lake(), &crm);
    ensure!(
        made.len() == 1,
        "the delivery makes one key apart; the key store holds {made:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let set = [crm.clone(), erp.clone()];
    let pre = (store.forget_sources_preflight(&set).await).must("the preflight of the set")?;
    store
        .forget_sources(&set, Some(&pre.report_id))
        .await
        .must("forgetting crm and erp")?;
    let forgotten: Vec<_> = (events_after(store.as_ref(), head).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::SourceForgotten { source, .. } => Some(source),
            _ => None,
        })
        .collect();
    let live = |s: &SourceId| {
        let held = keys.ids(store.lake(), s).unwrap_or_default();
        let held = held
            .iter()
            .filter(|k| keys.key(store.lake(), s, k).ok().flatten().is_some());
        held.count()
    };
    let left = keys.apart(store.lake(), &crm);
    ensure!(
        forgotten == set && left.is_empty() && live(&crm) == 0 && live(&erp) == 0,
        "the act appends a `source_forgotten` for crm and erp and destroys {made:?} with \
         their versions: {forgotten:?}, {left:?}"
    );
    let read = async |l: &Lake, by: &str| {
        for (k, source) in [("p1", "crm"), ("p2", "crm"), ("e1", "erp")] {
            for when in [None, Some(head)] {
                let r = at(l, "person", k, when).await?;
                ensure!(
                    r.as_ref().is_some_and(|r| erased(r, "name", source)
                        && r.fields.get("note") == Some(&json!("kept"))),
                    "{by}, as of {when:?}, `{k}` reads its name erased under {source} and \
                     keeps its note: {r:?}"
                );
            }
        }
        Ok::<_, Failure>(())
    };
    read(&l, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await?;
    Ok(())
}

/// Records in hand: `first` at once, then `rest` once `released` is set
/// and the waker it holds is woken.
struct Gated {
    first: std::vec::IntoIter<SourceRecord>,
    rest: std::vec::IntoIter<SourceRecord>,
    released: bool,
    waker: Option<std::task::Waker>,
}

/// A source stream over [`Gated`].
struct Gate(Arc<std::sync::Mutex<Gated>>);

impl futures_core::Stream for Gate {
    type Item = SourceRecord;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<SourceRecord>> {
        let held = &mut *crate::lock(&self.0);
        if let Some(r) = held.first.next() {
            return std::task::Poll::Ready(Some(r));
        }
        if !held.released {
            held.waker = Some(cx.waker().clone());
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(held.rest.next())
    }
}

async fn a_set_of_deliveries_shreds_what_it_takes_whole(store: Store, reopen: Reopen) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    for (id, record) in [("mon", p("p1", "Ada")), ("tue", p("p2", "Bea"))] {
        let opts = IngestOpts {
            delivery: Some(id.into()),
            sealed_apart: false,
            ..IngestOpts::default()
        };
        let said = lakelands_api::ingest_all_with(&*l.cat, &l.own, &crm, opts, vec![record]);
        said.await.must("ingesting a delivery")?;
    }
    let v1 = first(&keys, store.as_ref(), "crm")?;
    apart(&l, "late", vec![p("p3", "Cy")]).await?;
    let a = keys.apart(store.lake(), &crm);
    let mon = Selection::Delivery("mon".into());
    let alone = store.remove_values_preflight(&crm, &mon).await;
    let alone = alone.must("the preflight of `mon`")?;
    let set = Selection::Deliveries(["mon", "tue", "late"].map(Into::into).to_vec());
    let report = store.remove_values_preflight(&crm, &set).await;
    let report = report.must("the preflight of the set")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        alone.keys.is_empty()
            && alone.unshredded == 1
            && report.keys == [v1.clone()]
            && report.apart == a
            && a.len() == 1
            && fields == [("name", 3, 3)]
            && report.unshredded == 0,
        "`mon` alone destroys no version and leaves its name redacted only: {alone:?}; the set \
         destroys {v1} and the key apart {a:?}, counting `name` 3 redacted, 3 shredded: {report:?}"
    );
    store
        .remove_values(&crm, &set, true, None)
        .await
        .must("taking the set back, shredded")?;
    let ids = keys
        .ids(store.lake(), &crm)
        .must("listing crm's versions")?;
    let left = keys.apart(store.lake(), &crm);
    ensure!(
        ids.is_empty() && left.is_empty(),
        "the act destroys {v1} and {a:?}: the key store holds {ids:?} and {left:?}"
    );
    Ok(())
}

async fn a_call_writing_past_an_act_takes_a_new_key_apart(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let failing = Arc::new(FailingApart {
        keys: keys.clone(),
        ..FailingApart::default()
    });
    let seams = || Seams {
        keys: Some(failing.clone()),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = again(&reopen, store, seams(), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    let v1 = first(&keys, store.as_ref(), "crm")?;
    let gate = Arc::new(std::sync::Mutex::new(Gated {
        first: vec![p("p2", "Bea")].into_iter(),
        rest: vec![p("p3", "Cy")].into_iter(),
        released: false,
        waker: None,
    }));
    let opts = IngestOpts {
        delivery: Some("late".into()),
        sealed_apart: true,
        ..IngestOpts::default()
    };
    let gated = Box::pin(Gate(gate.clone()));
    let mut commits = match l.cat.ingest_with(&l.own, &crm, opts, gated).await {
        Ok(c) => c,
        Err(e) => return Err(Failure::new(format!("ingesting `late` sealed apart: {e}"))),
    };
    let window = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await;
    window
        .ok_or_else(|| Failure::new("`late`'s first window answers"))?
        .must("`late`'s first window")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    let made = keys.apart(store.lake(), &crm);
    let [a] = made.as_slice() else {
        return Err(Failure::new(format!(
            "the first window makes one key apart; the key store holds {made:?}"
        )));
    };
    let before = store.head().await.must("reading the head")?;
    let late = Selection::Delivery("late".into());
    failing.fails.store(true, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &late, false, None).await;
    let held = keys.apart(store.lake(), &crm);
    ensure!(
        stopped.is_err() && held == made,
        "the act whose destroy fails answers {stopped:?}, and the key stays: {held:?}"
    );
    let mark = store.head().await.must("reading the head")?;
    {
        let held = &mut *crate::lock(&gate);
        held.released = true;
        if let Some(w) = held.waker.take() {
            w.wake();
        }
    }
    while let Some(window) = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await {
        window.must("`late`'s window after the act")?;
    }
    failing.fails.store(false, Ordering::SeqCst);
    let sealed = seals(store.as_ref(), mark).await?;
    let p3 = sealed
        .iter()
        .find(|(w, _)| w == "ingest p3")
        .map(|(_, s)| s);
    let fresh = p3.and_then(|s| s.apart.first()).cloned();
    ensure!(
        fresh
            .as_ref()
            .is_some_and(|b| b != a && p3 == Some(&under(&v1, std::slice::from_ref(b), false))),
        "`p3`, written after the act, is sealed under {v1} and a new key apart, not {a}: \
         {sealed:?}"
    );
    let store = again(&reopen, store, seams(), "after the stopped act").await?;
    let left = keys.apart(store.lake(), &crm);
    ensure!(
        left == fresh.into_iter().collect::<Vec<_>>(),
        "the writer's next open destroys {a} alone: {left:?}"
    );
    let read = async |l: &Lake, by: &str| {
        for when in [Some(before), None] {
            let p2 = at(l, "person", "p2", when).await?;
            ensure!(
                p2.as_ref().is_some_and(|r| erased(r, "name", "crm")),
                "{by}, as of {when:?}, `p2` reads its name erased: {p2:?}"
            );
        }
        let p3 = at(l, "person", "p3", None).await?;
        ensure!(
            p3.as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Cy"))),
            "{by}, `p3` reads Cy: {p3:?}"
        );
        Ok::<_, Failure>(())
    };
    read(&Lake::of(store.as_ref()), "after the writer's next open").await?;
    let store = again(&reopen, store, seams(), "after the finish").await?;
    read(&Lake::of(store.as_ref()), "a writer opened again").await?;
    store.rebuild().await.must("rebuilding")?;
    read(&Lake::of(store.as_ref()), "after a rebuild").await?;
    Ok(())
}

async fn an_ingest_not_sealed_apart_seals_as_before(store: Store, reopen: Reopen) -> Held {
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
    let keys = Arc::new(Keys::default());
    let failing = Arc::new(FailingApart {
        keys: keys.clone(),
        ..FailingApart::default()
    });
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
    failing.fails.store(true, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &late, false, None).await;
    let held = keys.apart(store.lake(), &crm);
    ensure!(
        stopped.is_err() && made.len() == 1 && held == made,
        "the act whose destroy fails answers {stopped:?}, and the key stays: {made:?}, {held:?}"
    );
    failing.fails.store(false, Ordering::SeqCst);
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
