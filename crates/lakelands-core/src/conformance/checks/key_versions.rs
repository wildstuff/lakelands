//! A version of a source's key destroyed with every older one, and a key
//! period that seals each period's values under a version of their own
//! (`docs/decisions/058-a-source-may-be-forgotten.md`, lakelands#1153).

use super::erasable::{Keys, again, person, read, with};
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    VERSION_DESTROYED = a_destroyed_version_takes_the_outputs_of_its_values,
    "058 (lakelands#1153): a version of a source's key destroyed in the key store with the lake closed, outside an act that forgets it, is a lost key: once `crm`'s first version is destroyed while a newer one stands, `crm` writes on, sealed under the newer one; an output over values the newer one sealed is found again at a re-plan and called no more; an output over a value of the first and a value of the newer one, sealed under the first, is gone with it: a write that re-plans it is refused unavailable naming the source and the first version and appends nothing, and so is a declaration that re-plans it, before it appends (lakelands#1218), and neither calls anything or answers the recorded output",
    reopens
);

check!(
    KEY_PERIOD = a_key_period_seals_a_periods_values_under_one_version,
    "058 (lakelands#1153): `key_period` for the lake and `key_period.<source>` for a source take `day`, `week` or `month`, or null; another value is refused on `value`, and a key naming no source on `key`, appending nothing, while a host's own key that only begins so is the host's; a source with a key period seals every value written within one period under one version, across windows and a writer's reopen, and a rebuild makes no version; which period a source has, its own over the lake's, and where a period turns are held by the store's unit tests, since no check sets the writer's clock",
    reopens
);

check!(
    KEY_PERIOD_TOLD = a_source_s_key_period_is_told_where_nothing_is_held,
    "058 and 063 (lakelands#1153): `key_period.<source>` is told only to a reader from whom nothing is held, one holding `restricted` that reads every field of every type, since a source may feed any type: to a holder of `schema` without `restricted`, and to one with it masked from a field, `describe`, `find` of the settings with its total, `get` of the key, at the head and as of where it was set, `setting('…')` in a `where` and the proposals listed answer alike before `key_period.crm` is set and proposed and after, as for a lake that never set it, while a write of it, with an expected version of 0 and with none, a retire, a proposal and an ingest of it with no field, by a writer without `restricted` and by one masked from a field, are refused as not granted, alike before it is set and after, and the first's proposal of the lake's `key_period` is not pinned, accepted after the owner changed it; the lake's own `key_period` is told to both, and the owner is told the source's"
);

/// `person` with `nick` erasable too, and `both` computed by `pair` from
/// `name` and `nick`, counting its calls under `token`.
async fn people(l: &Lake, token: &str) -> Result<lakelands_api::ontology::TypeDecl, Failure> {
    let (kind, text) = (crate::conformance::backing::KIND, json!({"kind": "text"}));
    let inputs = [
        json!({"name": "name", "type": text}),
        json!({"name": "nick", "type": text}),
    ];
    l.declare_json(
        json!({"declare": "function", "name": "pair", "backing": kind,
        "inputs": inputs, "config": {"token": token, "echo": "nick"}}),
    )
    .await?;
    let mut t = person();
    t.fields.push(FieldDecl {
        erasable: true,
        ..sourced("nick", StorageKind::Text)
    });
    let both = crate::conformance::fixture::computed_by(
        "both",
        StorageKind::Text,
        "pair",
        &["name", "nick"],
    );
    t.fields.push(both);
    l.declare(t.clone()).await?;
    Ok(t)
}

/// The versions of `crm`'s key `keys` holds for the lake.
fn crm(keys: &Keys, store: &Store) -> Result<Vec<lakelands_api::id::KeyId>, Failure> {
    keys.ids(store.lake(), &"crm".into())
        .must("listing crm's keys")
}

async fn a_destroyed_version_takes_the_outputs_of_its_values(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let mut t = people(&l, "prefix").await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    let [v1] = crm(&keys, &store)?.try_into().map_err(|ids| {
        Failure::new(format!(
            "crm's first write made one version; the key store holds {ids:?}"
        ))
    })?;
    let v2 = keys
        .create(store.lake(), &"crm".into())
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    // `both` of p1 reads Ada, sealed under v1, and A, under v2.
    let nick = source_record("person", "p1", json!({"nick": "A"}));
    l.ingest("crm", vec![nick])
        .await
        .must("ingesting p1's nick")?;
    let bea = source_record(
        "person",
        "p2",
        json!({"id": "p2", "name": "Bea", "nick": "B"}),
    );
    l.ingest("crm", vec![bea]).await.must("ingesting p2")?;
    keys.destroy_version(store.lake(), &"crm".into(), &v1)
        .must("destroying the first version")?;
    let left = crm(&keys, &store)?;
    ensure!(
        left == [v2.clone()],
        "destroying {v1} leaves {v2}; the key store holds {left:?}"
    );
    let store = again(
        &reopen,
        store,
        with(&keys),
        "with the first version destroyed",
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let at = store.head().await.must("reading the head")?;
    let cy = source_record("person", "p3", json!({"id": "p3", "name": "Cy"}));
    l.ingest("crm", vec![cy])
        .await
        .must("ingesting p3 under the newer version")?;
    let name = read(&l, "p3", "name").await?;
    let sealed: Vec<_> = (events_after(store.as_ref(), at).await?.into_iter())
        .filter(|e| matches!(e.body, EventBody::RecordIngested { .. }))
        .map(|e| e.sealed.map(|s| s.key))
        .collect();
    ensure!(
        name == json!("Cy") && sealed == [Some(v2.clone())],
        "p3 is sealed under {v2} and reads `name` Cy; it is sealed under {sealed:?} and reads {name}"
    );
    // p2's `both` reads values of v2 alone: a nick moved back to x finds
    // the output recorded for x.
    let made = || crate::conformance::backing::made("prefix");
    let calls = made();
    for nick in ["x", "y", "x"] {
        let r = source_record("person", "p2", json!({"nick": nick}));
        l.ingest("crm", vec![r]).await.must("ingesting p2's nick")?;
    }
    let (called, both) = (made() - calls, read(&l, "p2", "both").await?);
    ensure!(
        called == 2 && both == json!("x"),
        "p2's nick set to x, y and x again calls `pair` twice and finds the first output again; it called {called} times and `both` reads {both}"
    );
    // p1's `both` reads Ada, of v1, and its nick, of v2: sealed under v1,
    // no re-plan finds it, and none calls. A declaration plans its re-plan
    // before it appends, as a write does.
    let missing = |e: Option<&Error>| match e.map(Error::unwrapped) {
        Some(Error::Unavailable { situation }) => {
            matches!(&**situation, Situation::KeyMissing { source, key } if source == "crm" && *key == v1)
        }
        _ => false,
    };
    let calls = made();
    let before = store.head().await.must("reading the head")?;
    let nick = source_record("person", "p1", json!({"nick": "A2"}));
    let nicked = l.ingest("crm", vec![nick]).await;
    ensure!(
        missing(nicked.as_ref().err()),
        "a write of p1's nick under {v2}, whose `both` also reads Ada of {v1}, is refused unavailable naming crm and {v1}; it answered {nicked:?}"
    );
    head_unmoved(store.as_ref(), before, "the write that re-plans p1").await?;
    t.fields.push(field("extra", StorageKind::Text));
    let declared = l.try_declare(t).await;
    ensure!(
        missing(declared.as_ref().err()),
        "a declaration that re-plans p1's `both`, over Ada of {v1} and A of {v2}, is refused unavailable naming crm and {v1}; it answered {declared:?}"
    );
    head_unmoved(store.as_ref(), before, "the declaration that re-plans p1").await?;
    let after = made();
    ensure!(
        after == calls,
        "the refused re-plans call nothing: {calls} calls before, {after} after"
    );
    Ok(())
}

async fn a_key_period_seals_a_periods_values_under_one_version(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let before = store.head().await.must("reading the head")?;
    for (key, value, member, case) in [
        ("key_period", json!("year"), "value", Case::SettingKeyPeriod),
        ("key_period.crm", json!(7), "value", Case::SettingKeyPeriod),
        ("key_period.", json!("day"), "key", Case::SourceIdForm),
    ] {
        let setting = json!({"key": key, "value": value});
        let said = try_upsert(l.cat.as_ref(), &l.own, ty, key, setting).await;
        let on = refused_on(&said).map(|(m, _)| m);
        ensure!(
            on == Some(member) && invalid_case(&said) == Some(&case),
            "`{key}` set to {value} is refused on `{member}` as {case:?}; it answered {said:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "the refused key periods").await?;
    l.set("key_periodicity", json!("quarterly")).await?;
    l.set("key_period", json!("week")).await?;
    l.set("key_period.crm", json!("day")).await?;
    let at = store.head().await.must("reading the head")?;
    let write = |key: &str| source_record("person", key, json!({"id": key, "name": key}));
    l.ingest("crm", vec![write("p1")])
        .await
        .must("ingesting p1")?;
    l.ingest("crm", vec![write("p2"), write("p3")])
        .await
        .must("ingesting p2 and p3")?;
    let store = again(&reopen, store, with(&keys), "for another writer").await?;
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![write("p4")])
        .await
        .must("ingesting p4 after the reopen")?;
    // The writes are a source's, by the writer's clock: each day of them
    // has a version of its own, made at its first value. A run that
    // crosses a midnight shows two.
    let (mut sealed, mut days) = (Vec::new(), Vec::new());
    for e in events_after(store.as_ref(), at).await? {
        if let (EventBody::RecordIngested { .. }, Some(s)) = (&e.body, &e.sealed) {
            sealed.push(s.key.clone());
            days.push(e.known_at.period_start("day"));
        }
    }
    fn turns<T: PartialEq>(v: &[T]) -> usize {
        v.windows(2).filter(|w| w[0] != w[1]).count()
    }
    let held = crm(&keys, &store)?;
    ensure!(
        sealed.len() == 4 && turns(&sealed) == turns(&days) && held.len() == turns(&days) + 1,
        "four values of crm, by day, are sealed under one version a day ({days:?}); they name {sealed:?} and the key store holds {held:?}"
    );
    store.rebuild().await.must("rebuilding the lake")?;
    let after = crm(&keys, &store)?;
    let name = read(&Lake::of(store.as_ref()), "p4", "name").await?;
    ensure!(
        after == held && name == json!("p4"),
        "a rebuild reads p4 and makes no version; the key store holds {after:?}, p4 reads {name}"
    );
    Ok(())
}

async fn a_source_s_key_period_is_told_where_nothing_is_held(store: Store) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{all_found, find_where, get_at, patch};
    use lakelands_api::write::{ProposalFilter, ProposedWrite};
    use lakelands_api::{Catalog, Find, Found, Position, Principal, TypeName};
    let l = Lake::of(store.as_ref());
    let n = crate::conformance::fixture::type_decl("row", vec![field("n", StorageKind::Number)]);
    l.declare(n).await?;
    l.upsert("row", "r1", json!({"n": 1})).await?;
    l.set("key_period", json!("week")).await?;
    let (ty, key) = (
        TypeName::from(lakelands_api::seam::SETTING_TYPE),
        "key_period.crm",
    );
    let open = holder(
        store.as_ref(),
        "open",
        json!({"default": "write", "schema": true, "review": true}),
    );
    let masked = holder(
        store.as_ref(),
        "masked",
        json!({"default": "read", "schema": true, "review": true, "restricted": true,
            "types": {"row": {"fields": {"n": "none"}}}}),
    );
    // One who writes everything but is masked from `row.n`, and so is held
    // something: its writes reach the guard.
    let kept = holder(
        store.as_ref(),
        "kept",
        json!({"default": "write", "ingest": true, "restricted": true,
            "types": {"row": {"fields": {"n": "none"}}}}),
    );
    // What a reader is told of the settings, by every door that tells it,
    // `get` as of `as_of`.
    let told = async |cat: &dyn Catalog, who: &Principal, as_of: Option<Position>| {
        let d = cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data.settings;
        let d: Vec<_> = d.into_iter().map(|s| (s.key, s.value)).collect();
        let counted = Find {
            total: true,
            ..Find::default()
        };
        let found = cat.find(who, &ty, counted).await;
        let total = match found.must("counting the settings")?.data {
            Found::Records { total, .. } => total,
            _ => None,
        };
        let rows = all_found(cat, who, ty.as_str(), Find::default()).await?;
        let rows: Vec<_> = rows.into_iter().map(|r| (r.key.0, r.fields)).collect();
        let got = cat.get(who, &ty, get_at(key, as_of)).await.is_ok();
        let unset = find_where("setting('key_period.crm') is null");
        let unset = all_found(cat, who, "row", unset).await?.len();
        let proposed = cat.proposals(who, ProposalFilter::default()).await;
        let proposed = proposed.must("listing the proposals")?.data.len();
        Ok::<_, Failure>((d, total, rows, got, unset, proposed))
    };
    // What each of them is answered writing it, with an expected version
    // of 0 and none, retiring it, proposing a write of it, and ingesting it
    // with no field.
    let writes = async |who: &Principal| {
        let mut said = Vec::new();
        for expected_version in [Some(lakelands_api::Version(0)), None] {
            let opts = lakelands_api::WriteOpts {
                expected_version,
                ..Default::default()
            };
            let p = patch(json!({"key": key, "value": "month"}));
            said.push(l.cat.upsert(who, &ty, &key.into(), p, opts).await);
        }
        said.push(
            l.cat
                .retire(who, &ty, &key.into(), Default::default())
                .await,
        );
        let write = ProposedWrite::Upsert {
            ty: ty.clone(),
            key: key.into(),
            patch: patch(json!({"key": key, "value": "month"})),
            opts: Default::default(),
        };
        said.push(l.cat.propose(who, write, "a month".into()).await);
        let empty = crate::conformance::fixture::source_record(ty.as_str(), key, json!({}));
        let fed = crate::conformance::fixture::ingest(l.cat.as_ref(), who, "crm", vec![empty]);
        let fed = fed.await.map(drop);
        let not_granted = |r: Option<&Error>| {
            matches!(
                r,
                Some(Error::Refused {
                    reason: lakelands_api::Refusal::NotGranted,
                    ..
                })
            )
        };
        let all =
            said.iter().all(|r| not_granted(r.as_ref().err())) && not_granted(fed.as_ref().err());
        Ok::<_, Failure>((all, format!("{said:?} {fed:?}")))
    };
    let (before_open, before_masked) = (writes(&open).await?, writes(&kept).await?);
    let before = (
        told(l.cat.as_ref(), &open, None).await?,
        told(l.cat.as_ref(), &masked, None).await?,
    );
    let at = l.set(key, json!("day")).await?.position();
    let (after_open, after_masked) = (writes(&open).await?, writes(&kept).await?);
    ensure!(
        before_open.0
            && before_masked.0
            && before_open == after_open
            && before_masked == after_masked,
        "a write of `key_period.crm`, with an expected version of 0 and none, its retire, its \
         proposal and its ingest with no field, by a writer without `restricted` and by one \
         masked from `row.n`, are each \
         refused as not granted, alike before it is set and after: before {before_open:?} \
         {before_masked:?}, after {after_open:?} {after_masked:?}"
    );
    // The owner proposes another period: a proposal no twin holds.
    let write = ProposedWrite::Upsert {
        ty: ty.clone(),
        key: key.into(),
        patch: patch(json!({"key": key, "value": "week"})),
        opts: Default::default(),
    };
    let proposed = l.cat.propose(&l.own, write, "a week".into()).await;
    proposed.must("proposing crm's period")?;
    for as_of in [None, Some(at)] {
        let after = (
            told(l.cat.as_ref(), &open, as_of).await?,
            told(l.cat.as_ref(), &masked, as_of).await?,
        );
        let lake = |t: &(Vec<(String, Option<serde_json::Value>)>, _, _, _, _, _)| {
            t.0.iter()
                .any(|(k, v)| k == "key_period" && v == &Some(json!("week")))
        };
        ensure!(
            before == after && lake(&after.0) && lake(&after.1) && !after.0.3 && after.0.4 == 1,
            "a reader without `restricted` and one masked from `row.n` are told the settings \
             and the proposals alike before `key_period.crm` is set and proposed and after, \
             `get` as of {as_of:?}: the lake's `key_period` and no `key_period.crm`; before \
             {before:?}, after {after:?}"
        );
    }
    let owner = told(l.cat.as_ref(), &l.own, None).await?;
    // Its proposal of the lake's period, told it whole, is not pinned: it
    // is accepted after the owner changed the period.
    let write = ProposedWrite::Upsert {
        ty: ty.clone(),
        key: "key_period".into(),
        patch: patch(json!({"key": "key_period", "value": "month"})),
        opts: Default::default(),
    };
    let id = match l.cat.propose(&open, write, "a month".into()).await {
        Ok(lakelands_api::Outcome::Proposed { proposal, .. }) => proposal,
        other => {
            return Err(Failure::new(format!(
                "proposing a month answered {other:?}"
            )));
        }
    };
    l.set("key_period", json!("day")).await?;
    let accepted = l.cat.accept(&l.own, &id).await;
    ensure!(
        accepted.is_ok(),
        "the proposal of `key_period` by the reader without `restricted`, accepted after the \
         owner changed it, answers {accepted:?}"
    );
    let source = owner
        .0
        .iter()
        .any(|(k, v)| k == key && v == &Some(json!("day")));
    ensure!(
        source && owner.3 && owner.4 == 0 && owner.5 == 1,
        "the owner is told `key_period.crm` by `describe`, `get`, `setting('…')` and its \
         proposal: {owner:?}"
    );
    Ok(())
}
