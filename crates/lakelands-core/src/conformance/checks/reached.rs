//! An act reaches every place a value it takes went
//! (`docs/decisions/058-a-source-may-be-forgotten.md`,
//! `docs/decisions/065-part-of-a-source-may-be-taken-back.md`): a record a
//! re-keying migration moved is found at the key it bears now, and a value
//! that identifies records at the act or where it was written is never
//! taken (lakelands#1228).

use std::sync::Arc;

use lakelands_api::ontology::TypeDecl;
use lakelands_api::write::{ForgetReport, RemoveReport, Selection};
use lakelands_api::{LakeStore, Position, SourceId, Timestamp};
use serde_json::{Value, json};

use super::erasable::{Keys, again, person, with};
use super::removed::{at, erased};
use crate::conformance::fixture::{Lake, Must, source_record};
use crate::conformance::{Failure, Held, Reopen, Store};

check!(
    REMOVED_REKEYED = a_value_taken_back_is_followed_through_a_rekeying,
    "065 (lakelands#1228): `visit` keyed by `id` and `zone` holds `tracker`'s `v1`, kept by an author's note, and `v2`, `tracker`'s alone; a migration re-keys it by `id` and `place`; taking back all `tracker` wrote counts both records at the keys they bear now, `duration` and neither `place`, a key field at the act, nor `zone`, one where it was written, retires `v2` at its new key and keeps `v1` there; `v1` holds no `duration`, its provenance saying it was erased, and keeps `place` and `zone`, at its new key at the head and before the act, at its old key before the migration, and after a rebuild"
);

check!(
    FORGOTTEN_REKEYED = a_forgotten_source_is_followed_through_a_rekeying,
    "058 (lakelands#1228): `crm`'s `p1`, kept by an author's note, and `p2`, `crm`'s alone, are re-keyed by `id` and `city`; forgetting `crm` counts both at the keys they bear now, retires `p2` at its new key and keeps `p1` there, whose `name` reads as erased and whose `city` stays, at its new key at the head and before the act, at its old key before the migration, and after a rebuild",
    reopens
);

/// The head of `store`.
async fn head(store: &dyn LakeStore) -> Result<Position, Failure> {
    store.head().await.must("reading the head")
}

/// What a report of either act names of the records it reaches.
fn reached(types: Value, fields: Value, retired: Value, kept: Value) -> Value {
    json!({"types": types, "fields": fields, "retired": retired, "kept": kept})
}

/// [`reached`] of a removal's report.
fn removed(r: &RemoveReport) -> Value {
    reached(
        json!(
            r.types
                .iter()
                .map(|t| json!([t.ty, t.records]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.fields
                .iter()
                .map(|f| json!([f.ty, f.field, f.redacted]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.retired
                .iter()
                .map(|t| json!([t.ty, t.retired]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.kept
                .iter()
                .map(|t| json!([t.ty, t.first]))
                .collect::<Vec<_>>()
        ),
    )
}

/// [`reached`] of a forgetting's report.
fn forgotten(r: &ForgetReport) -> Value {
    reached(
        json!(
            r.types
                .iter()
                .map(|t| json!([t.ty, t.records]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.fields
                .iter()
                .map(|f| json!([f.ty, f.field, f.values]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.retired
                .iter()
                .map(|t| json!([t.ty, t.retired]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.co_owned
                .iter()
                .map(|t| json!([t.ty, t.first]))
                .collect::<Vec<_>>()
        ),
    )
}

/// `visit` keyed by `key`: `id`, `zone`, `place` and `duration` from a
/// source, the first three required, `note` an author's.
fn visit(key: &[&str]) -> Value {
    let (text, number) = (json!({"kind": "text"}), json!({"kind": "number"}));
    let sourced = |name: &str, ty: &Value| json!({"name": name, "type": ty, "authority": "source"});
    let keyable = |name: &str| {
        json!({"name": name, "type": text, "authority": "source",
                                      "required": true})
    };
    json!({"declare": "type", "name": "visit", "key": key, "fields": [
        keyable("id"), keyable("zone"), keyable("place"),
        sourced("duration", &number), {"name": "note", "type": text}]})
}

/// What `when` holds of a record of `ty` at `key`: `erased` erased from
/// `source`, and `kept` with the values given.
async fn holds(
    l: &Lake,
    (ty, key, when): (&str, &str, Option<Position>),
    (erased_field, source): (&str, &str),
    kept: Value,
) -> Held {
    let r = at(l, ty, key, when).await?;
    let held = |r: &lakelands_api::Record| {
        let keeps = |(f, v): (&String, &Value)| r.fields.get(f.as_str()) == Some(v);
        erased(r, erased_field, source) && kept.as_object().is_some_and(|k| k.iter().all(keeps))
    };
    ensure!(
        r.as_ref().is_some_and(held),
        "as of {when:?}, `{ty}` `{key}` holds no `{erased_field}`, erased from {source}, and \
         keeps {kept}: {r:?}"
    );
    Ok(())
}

async fn a_value_taken_back_is_followed_through_a_rekeying(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(visit(&["id", "zone"])).await?;
    let v = |k: &str, zone: &str, place: &str, duration: u32| {
        let fields = json!({"id": k, "zone": zone, "place": place, "duration": duration});
        source_record("visit", &format!("{k}:{zone}"), fields)
    };
    let visits = vec![v("v1", "z1", "Rome", 10), v("v2", "z2", "Oslo", 20)];
    l.ingest("tracker", visits)
        .await
        .must("ingesting from tracker")?;
    l.upsert("visit", "v1:z1", json!({"note": "kept"})).await?;
    let written = head(store.as_ref()).await?;
    let mut to = visit(&["id", "place"]);
    to.as_object_mut().map(|t| t.remove("declare"));
    let to: TypeDecl = serde_json::from_value(to).expect("a type");
    l.migrate(to).await?;
    let before = head(store.as_ref()).await?;
    let tracker: SourceId = "tracker".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let report = store.remove_values_preflight(&tracker, &all).await;
    let report = report.must("the preflight")?;
    let want = reached(
        json!([["visit", 2]]),
        json!([["visit", "duration", 2]]),
        json!([["visit", 1]]),
        json!([["visit", ["v1:Rome"]]]),
    );
    ensure!(
        removed(&report) == want,
        "the preflight counts both visits at the keys they bear now, `duration` and neither \
         `place` nor `zone`, retires `v2` and keeps `v1`: it names {} in {report:?}",
        removed(&report)
    );
    let done = store.remove_values(&tracker, &all, false, None).await;
    ensure!(
        removed(&done.must("taking back what tracker wrote")?) == want,
        "the act answers what its preflight named"
    );
    let kept = json!({"place": "Rome", "zone": "z1", "note": "kept"});
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for when in [None, Some(before)] {
            let of = ("visit", "v1:Rome", when);
            holds(&l, of, ("duration", "tracker"), kept.clone()).await?;
        }
        let then = ("visit", "v1:z1", Some(written));
        holds(&l, then, ("duration", "tracker"), kept.clone()).await?;
        let v2 = at(&l, "visit", "v2:Oslo", None).await?;
        ensure!(
            v2.is_none(),
            "after {by}, `v2`, tracker's alone, is retired at its new key: {v2:?}"
        );
    }
    Ok(())
}

async fn a_forgotten_source_is_followed_through_a_rekeying(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let p = |k: &str, name: &str, city: &str| {
        source_record("person", k, json!({"id": k, "name": name, "city": city}))
    };
    let people = vec![p("p1", "Ada", "Rome"), p("p2", "Bob", "Oslo")];
    l.ingest("crm", people).await.must("ingesting from crm")?;
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let written = head(store.as_ref()).await?;
    let mut to = TypeDecl {
        key: vec!["id".into(), "city".into()],
        ..person()
    };
    to.fields
        .iter_mut()
        .filter(|f| f.name == "city")
        .for_each(|f| f.required = true);
    l.migrate(to).await?;
    let before = head(store.as_ref()).await?;
    let crm: SourceId = "crm".into();
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    let want = reached(
        json!([["person", 2]]),
        json!([["person", "name", 2]]),
        json!([["person", 1]]),
        json!([["person", ["p1:Rome"]]]),
    );
    ensure!(
        forgotten(&report) == want,
        "the preflight counts both people at the keys they bear now, retires `p2` and keeps \
         `p1`: it names {} in {report:?}",
        forgotten(&report)
    );
    store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm")?;
    let kept = json!({"city": "Rome", "note": "kept"});
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for when in [None, Some(before)] {
            holds(
                &l,
                ("person", "p1:Rome", when),
                ("name", "crm"),
                kept.clone(),
            )
            .await?;
        }
        let then = ("person", "p1", Some(written));
        holds(&l, then, ("name", "crm"), kept.clone()).await?;
        let p2 = at(&l, "person", "p2:Oslo", None).await?;
        ensure!(
            p2.is_none(),
            "after {by}, `p2`, crm's alone, is retired at its new key: {p2:?}"
        );
    }
    Ok(())
}
