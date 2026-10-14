//! A source's values of an erasable field are sealed under its key
//! (`docs/decisions/058-a-source-may-be-forgotten.md`).
//!
//! The log holds them as ciphertext the source's key opens, and the
//! projection holds them in plaintext: reads, a rebuild and a redelivery
//! answer as for any other value. The checks bring their own key store,
//! [`Keys`], since a key store is the host's.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lakelands_api::error::{Seam, Situation};
use lakelands_api::event::{Event, EventBody};
use lakelands_api::id::KeyId;
use lakelands_api::ontology::{Authority, FieldDecl, FieldType, StorageKind};
use lakelands_api::seam::{DataKey, KeyStore, Seams};
use lakelands_api::{Error, LakeId, LakeStore, Position, SourceId};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, events_after, field, refused_like, source_record, sourced, type_decl,
};
use crate::conformance::{Failure, Held, Reopen, Store};

check!(
    SEALED_IN_THE_LOG = a_sources_value_of_an_erasable_field_is_sealed_in_the_log_and_read_plain,
    "058: a source's value of an erasable field enters the log as `record_ingested` with `sealed` naming the key's version and the field, and the value base64 that is not the plaintext, while a field that is not erasable stays plaintext; the record reads the plaintext, at the head and after a rebuild; the same value sent again against the same state appends nothing, the same value on another record seals to other bytes, and a value that returns after another (A, B, A) appends again; with no key store bound such a value is refused as unbound and nothing is appended",
    reopens
);

check!(
    ERASABLE_DECLARED = erasable_is_declared_on_a_sources_own_field_alone_and_stays,
    "058: `erasable` is refused on a key field, on a field that names a record, on a field only authors or the fold write, and on a relation's field; a field that was erasable is not declared without it; `describe` shows it",
    reopens
);

check!(
    KEY_MISSING = a_key_the_key_store_does_not_give_fails_closed,
    "058: a lake whose log holds a source's sealed value, opened with a key store that does not give its key, takes no new key for that source: its next ingest is refused unavailable naming the source and the key, appending nothing, and the key store is asked for none; a rebuild answers the same, and a lost key is never read as an erasure; with the key store that holds it the rebuild answers the plaintext again",
    reopens
);

check!(
    SEALED_OUTPUT = an_output_computed_from_a_sources_erasable_values_is_sealed_under_its_key,
    "058: a function computed outside the lake that reads a source's erasable value records its output sealed under that source's key, `sealed` naming the key and the source, with an input digest that is not the SHA-256 of the call, and the field reads the plaintext answer; a call that reads erasable values of two sources is not made, and its field holds no value and says why; a write that moves a call back to an input already recorded plans the field, and the sets that read it, as the fold computes them; and after a newer version of the key is made, a rebuild answers the same and calls nothing again: an output is digested under the version of the key the log first sealed with",
    reopens
);

check!(
    FIRST_SEALED = a_sources_first_sealed_write_is_planned_as_the_fold_computes_it,
    "058 (lakelands#1042): a source's first sealed write is planned under the version of its key the write seals with, as the fold computes it: `c1`'s `name`, written before `name` was erasable, has its output recorded under a plain digest; `c2`, the source's first sealed write with the same `name`, finds no output under its key in its plan or its fold, so `shout` is called for it once, and `c2` joins the set over `loud` only where its output fills it, never before",
    reopens
);

/// A key store in memory, for the checks: one key a version, made from a
/// counter, so nothing of it is random, and every id once in the process,
/// so a store made anew never gives an id it gave before.
#[derive(Default)]
pub struct Keys(Mutex<BTreeMap<(LakeId, SourceId), Versions>>);

/// A source's keys by version, none where destroyed.
type Versions = Vec<(KeyId, Option<[u8; 32]>)>;

impl KeyStore for Keys {
    fn create(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        let mut held = crate::lock(&self.0);
        let versions = held.entry((lake.clone(), source.clone())).or_default();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = KeyId::from(format!("k{n}"));
        let bytes: [u8; 32] =
            <sha2::Sha256 as sha2::Digest>::digest(format!("{lake}/{source}/{id}")).into();
        versions.push((id.clone(), Some(bytes)));
        Ok(id)
    }

    fn key(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<Option<DataKey>> {
        let held = crate::lock(&self.0);
        let versions = held.get(&(lake.clone(), source.clone()));
        let found = versions.and_then(|v| v.iter().find(|(i, _)| i == id));
        Ok(found.and_then(|(_, k)| k.map(DataKey)))
    }

    fn destroy(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<()> {
        let mut held = crate::lock(&self.0);
        for (_, k) in held.entry((lake.clone(), source.clone())).or_default() {
            *k = None;
        }
        Ok(())
    }

    fn ids(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<Vec<KeyId>> {
        let held = crate::lock(&self.0);
        let versions = held
            .get(&(lake.clone(), source.clone()))
            .into_iter()
            .flatten();
        Ok(versions
            .filter(|(_, k)| k.is_some())
            .map(|(i, _)| i.clone())
            .collect())
    }
}

/// The first version of `source`'s key `keys` holds for the lake.
pub(super) fn first(keys: &Keys, store: &dyn LakeStore, source: &str) -> Result<KeyId, Failure> {
    let ids = keys
        .ids(store.lake(), &source.into())
        .ok()
        .unwrap_or_default();
    ids.into_iter()
        .next()
        .ok_or_else(|| Failure::new(format!("the key store holds no key of `{source}`")))
}

/// Seams with `keys` as the key store, and the suite's backing.
pub(super) fn with(keys: &Arc<Keys>) -> Seams {
    Seams {
        keys: Some(keys.clone()),
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    }
}

/// `person`, keyed by `id`: `name` erasable and `city` not, each a
/// source's, and `note` an author's.
pub(super) fn person() -> lakelands_api::ontology::TypeDecl {
    let name = FieldDecl {
        erasable: true,
        ..sourced("name", StorageKind::Text)
    };
    let fields = vec![
        FieldDecl {
            required: true,
            ..sourced("id", StorageKind::Text)
        },
        name,
        sourced("city", StorageKind::Text),
        field("note", StorageKind::Text),
    ];
    lakelands_api::ontology::TypeDecl {
        key: vec!["id".into()],
        ..type_decl("person", fields)
    }
}

/// The `record_ingested` events after `after`.
async fn ingested(store: &dyn LakeStore, after: Position) -> Result<Vec<Event>, Failure> {
    let events = events_after(store, after).await?;
    let ingested = |e: &Event| matches!(e.body, EventBody::RecordIngested { .. });
    Ok(events.into_iter().filter(ingested).collect())
}

/// A member of an ingested event's patch.
fn stored(e: &Event, f: &str) -> Value {
    match &e.body {
        EventBody::RecordIngested { patch, .. } => patch.get(f).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// The value of `f` in `person` `key` at the head.
pub(super) async fn read(l: &Lake, key: &str, f: &str) -> Result<Value, Failure> {
    let r = l.read("person", key).await?;
    Ok(r.fields.get(f).cloned().unwrap_or(Value::Null))
}

/// Opens the lake again with `seams`, naming `what` if that fails.
pub(super) async fn again(
    reopen: &Reopen,
    store: Store,
    seams: Seams,
    what: &str,
) -> Result<Store, Failure> {
    (reopen)(store, seams)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again {what} failed: {e}")))
}

async fn a_sources_value_of_an_erasable_field_is_sealed_in_the_log_and_read_plain(
    store: Store,
    reopen: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ada = |k: &str| source_record("person", k, json!({"id": k, "name": "Ada", "city": "Oslo"}));
    let before = store.head().await.must("reading the head")?;
    let unbound = l.ingest("crm", vec![ada("p1")]).await;
    ensure!(
        matches!(&unbound, Err(Error::Unbound { seam }) if **seam == Seam::KeyStore),
        "an erasable value with no key store bound must be refused as unbound, the key store; it answered {unbound:?}"
    );
    let head = store.head().await.must("reading the head")?;
    ensure!(head == before, "the refused ingest appended up to {head}");
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![ada("p1")])
        .await
        .must("ingesting p1")?;
    let events = ingested(store.as_ref(), before).await?;
    let [e] = events.as_slice() else {
        return Err(Failure::new(format!(
            "one `record_ingested` was expected, the log holds {events:?}"
        )));
    };
    let k = first(&keys, store.as_ref(), "crm")?;
    let sealed = e.sealed.as_ref().map(|s| (&s.key, s.fields.clone()));
    ensure!(
        sealed == Some((&k, vec!["name".into()])),
        "the event's `sealed` names key `{k}` and the field `name`; it is {:?}",
        e.sealed
    );
    let name = stored(e, "name");
    let base64 = name
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() % 4 == 0 && !s.contains("Ada"));
    ensure!(
        base64,
        "the log holds `name` as base64 that is not the plaintext; it holds {name}"
    );
    ensure!(
        stored(e, "city") == json!("Oslo"),
        "`city` is not erasable and stays plaintext in the log; it holds {}",
        stored(e, "city")
    );
    ensure!(
        read(&l, "p1", "name").await? == json!("Ada"),
        "`p1` reads the plaintext `name`"
    );
    let head = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![ada("p1")])
        .await
        .must("ingesting p1 again")?;
    let again_head = store.head().await.must("reading the head")?;
    ensure!(
        again_head == head,
        "the same value against the same state appends nothing; the head moved from {head} to {again_head}"
    );
    l.ingest("crm", vec![ada("p2")])
        .await
        .must("ingesting p2")?;
    let bob = source_record("person", "p1", json!({"name": "Bob"}));
    l.ingest("crm", vec![bob])
        .await
        .must("ingesting Bob into p1")?;
    l.ingest("crm", vec![ada("p1")])
        .await
        .must("ingesting Ada into p1 again")?;
    let events = ingested(store.as_ref(), before).await?;
    let names: Vec<Value> = events.iter().map(|e| stored(e, "name")).collect();
    ensure!(
        names.len() == 4 && names[0] != names[1] && names[0] != names[3] && names[2] != names[3],
        "p1, p2, Bob into p1 and Ada into p1 again append four events whose sealed `name`s all differ; the log holds {names:?}"
    );
    store.rebuild().await.must("rebuilding the lake")?;
    for (key, want) in [("p1", "Ada"), ("p2", "Ada")] {
        let v = read(&l, key, "name").await?;
        ensure!(
            v == json!(want),
            "after a rebuild `{key}` reads `name` {want}; it reads {v}"
        );
    }
    Ok(())
}

async fn erasable_is_declared_on_a_sources_own_field_alone_and_stays(
    store: Store,
    _: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    // The store answers before the declarations it refuses are sent.
    l.declare(person()).await?;
    let erasable = |f: FieldDecl| FieldDecl {
        erasable: true,
        ..f
    };
    let reference = FieldDecl {
        ty: FieldType {
            target: Some("person".into()),
            ..FieldType::kind(StorageKind::Reference)
        },
        authority: Authority::Source,
        ..field("friend", StorageKind::Reference)
    };
    let id = || FieldDecl {
        required: true,
        ..field("id", StorageKind::Text)
    };
    for (f, case) in [
        (
            erasable(FieldDecl {
                authority: Authority::Source,
                ..id()
            }),
            "erasable_key",
        ),
        (erasable(reference), "erasable_reference"),
        (
            erasable(field("note", StorageKind::Text)),
            "erasable_authored",
        ),
    ] {
        let name = f.name.clone();
        let mut t = person();
        t.fields.retain(|g| g.name != name);
        t.fields.push(f);
        let what = format!("declaring `{name}` erasable");
        refused_like(
            &what,
            l.try_declare(t).await,
            &json!({"code": case, "at": [{"code": "erasable"}]}),
        )?;
    }
    let mut dropped = person();
    dropped.fields.iter_mut().for_each(|f| f.erasable = false);
    let what = "declaring `name` without `erasable`";
    refused_like(
        what,
        l.try_declare(dropped).await,
        &json!({"code": "erasable_dropped"}),
    )?;
    let d = l.describe_type("person").await?;
    let shown: Vec<_> = d
        .fields
        .iter()
        .filter(|f| f.erasable)
        .map(|f| f.name.as_str())
        .collect();
    ensure!(
        shown == ["name"],
        "`describe` shows `name` alone as erasable; it shows {shown:?}"
    );
    Ok(())
}

async fn a_key_the_key_store_does_not_give_fails_closed(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    let k = first(&keys, store.as_ref(), "crm")?;
    let other = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&other), "with another key store").await?;
    let named = |s: &Situation| matches!(s, Situation::KeyMissing { source, key } if source == "crm" && key == &k);
    // A key store that lost the key is never asked for a new one: the
    // write is refused, and nothing is appended.
    let before = store.head().await.must("reading the head")?;
    let l = Lake::of(store.as_ref());
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    let refused = l.ingest("crm", vec![bob]).await;
    ensure!(
        matches!(&refused, Err(Error::Unavailable { situation }) if named(situation)),
        "an ingest of `crm` into a lake that holds its sealed values, with a key store that lost its key `{k}`, must answer unavailable naming them; it answered {refused:?}"
    );
    let head = store.head().await.must("reading the head")?;
    ensure!(head == before, "the refused ingest appended up to {head}");
    let none = other
        .ids(store.lake(), &"crm".into())
        .ok()
        .unwrap_or_default();
    ensure!(
        none.is_empty(),
        "the key store was given no key of `crm`; it holds {none:?}"
    );
    let rebuilt = store.rebuild().await;
    ensure!(
        matches!(&rebuilt, Err(Error::Unavailable { situation }) if named(situation)),
        "a rebuild whose key store does not give `crm`'s key `{k}` must answer unavailable naming them; it answered {rebuilt:?}"
    );
    let store = again(&reopen, store, with(&keys), "with its key store").await?;
    store
        .rebuild()
        .await
        .must("rebuilding with the key store")?;
    let l = Lake::of(store.as_ref());
    let v = read(&l, "p1", "name").await?;
    ensure!(
        v == json!("Ada"),
        "rebuilt with its key store, `p1` reads `name` Ada; it reads {v}"
    );
    Ok(())
}

async fn an_output_computed_from_a_sources_erasable_values_is_sealed_under_its_key(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let kind = crate::conformance::backing::KIND;
    let text = json!({"kind": "text"});
    let config = json!({"token": "sealed_output", "echo": "name"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": kind,
        "inputs": [{"name": "name", "type": text}], "config": config}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "function", "name": "pair", "backing": kind,
        "inputs": [{"name": "name", "type": text}, {"name": "nick", "type": text}],
        "config": json!({"token": "sealed_output_pair", "echo": "name"})}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "function", "name": "tag", "backing": kind,
        "inputs": [{"name": "name", "type": text}, {"name": "note", "type": text}],
        "config": json!({"token": "sealed_output_tag", "echo": "note"})}),
    )
    .await?;
    let mut t = person();
    t.fields.push(FieldDecl {
        erasable: true,
        ..sourced("nick", StorageKind::Text)
    });
    t.fields.push(crate::conformance::fixture::computed_by(
        "tagged",
        StorageKind::Text,
        "tag",
        &["name", "note"],
    ));
    t.fields.push(crate::conformance::fixture::computed_by(
        "loud",
        StorageKind::Text,
        "shout",
        &["name"],
    ));
    t.fields.push(crate::conformance::fixture::computed_by(
        "both",
        StorageKind::Text,
        "pair",
        &["name", "nick"],
    ));
    l.declare(t).await?;
    let set = crate::conformance::fixture::object_set_on("tagged_x", "person", "tagged = 'x'");
    l.declare_as(set, "declaring the set `tagged_x`").await?;
    let before = store.head().await.must("reading the head")?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada])
        .await
        .must("ingesting p1 from crm")?;
    let r = l.read("person", "p1").await?;
    ensure!(
        r.fields.get("loud") == Some(&json!("Ada")),
        "`loud` reads the plaintext answer Ada; it reads {:?}",
        r.fields.get("loud")
    );
    let events = events_after(store.as_ref(), before).await?;
    let outputs: Vec<&Event> = (events.iter())
        .filter(|e| matches!(&e.body, EventBody::OutputRecorded { seam, .. } if seam.ends_with(":shout")))
        .collect();
    let [e] = outputs.as_slice() else {
        return Err(Failure::new(format!(
            "one `output_recorded` of `shout` was expected; the log holds {events:?}"
        )));
    };
    let k = first(&keys, store.as_ref(), "crm")?;
    let sealed = (e.sealed.as_ref()).map(|s| {
        (
            &s.key,
            s.source.as_ref().map(|s| s.as_str()),
            s.fields.len(),
        )
    });
    ensure!(
        sealed == Some((&k, Some("crm"), 0)),
        "the output's `sealed` names key `{k}` and the source `crm`; it is {:?}",
        e.sealed
    );
    let EventBody::OutputRecorded { input, output, .. } = &e.body else {
        unreachable!("filtered to outputs")
    };
    let call = json!({"function": "shout", "inputs": {"name": "Ada"}, "config": config});
    ensure!(
        input.0 != crate::canonical::sha256_hex(&call),
        "the input digest is not the SHA-256 of the plaintext call"
    );
    let base64 = output
        .as_ref()
        .and_then(|o| o.as_str())
        .is_some_and(|s| !s.contains("Ada"));
    ensure!(
        base64,
        "the output is held sealed, as base64 that is not the plaintext; it is {output:?}"
    );
    let nick = source_record("person", "p1", json!({"nick": "A"}));
    l.ingest("erp", vec![nick])
        .await
        .must("ingesting p1's nick from erp")?;
    let r = l.read("person", "p1").await?;
    let why = r
        .meta
        .get("both")
        .and_then(|m| m.error.clone())
        .unwrap_or_default();
    ensure!(
        !r.fields.contains_key("both") && why.contains("two sources"),
        "`both` reads erasable values of crm and erp, so it holds no value and says why; it holds {:?}, its error {why:?}",
        r.fields.get("both")
    );
    // A write plans a field computed from a sealed value as the fold
    // computes it: `note` back at `x`, whose call is recorded, fills
    // `tagged` in the write's own plan, and `p1` joins `tagged_x` there.
    let at = store.head().await.must("reading the head")?;
    for note in ["x", "y", "x"] {
        l.upsert("person", "p1", json!({"note": note})).await?;
    }
    let mut member = None;
    for e in events_after(store.as_ref(), at).await? {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = &e.body
            && set == "tagged_x"
            && key == "p1"
        {
            member = Some(*joined);
        }
    }
    let r = l.read("person", "p1").await?;
    ensure!(
        r.fields.get("tagged") == Some(&json!("x")) && member == Some(true),
        "`note` set to x, y and x again leaves `tagged` x and `p1` in `tagged_x`; `tagged` reads {:?} and its last membership change is {member:?}",
        r.fields.get("tagged")
    );
    // A newer version of the key changes nothing the log holds: a rebuild
    // finds the recorded output again and calls nothing.
    let calls = crate::conformance::backing::made("sealed_output");
    keys.create(store.lake(), &"crm".into())
        .must("making a second version")?;
    let store = again(
        &reopen,
        store,
        with(&keys),
        "with a newer version of the key",
    )
    .await?;
    let l = Lake::of(store.as_ref());
    store.rebuild().await.must("rebuilding the lake")?;
    let r = l.read("person", "p1").await?;
    ensure!(
        r.fields.get("loud") == Some(&json!("Ada")),
        "after a rebuild under a newer version of the key, `loud` reads Ada; it reads {:?}",
        r.fields.get("loud")
    );
    let again_calls = crate::conformance::backing::made("sealed_output");
    ensure!(
        again_calls == calls,
        "the rebuild called `shout` again: {calls} calls before, {again_calls} after"
    );
    Ok(())
}

async fn a_sources_first_sealed_write_is_planned_as_the_fold_computes_it(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let token = "first_sealed";
    let kind = crate::conformance::backing::KIND;
    l.declare_json(
        json!({"declare": "function", "name": "yell", "backing": kind,
        "inputs": [{"name": "name", "type": {"kind": "text"}}],
        "config": {"token": token, "echo": "name"}}),
    )
    .await?;
    let card = |erasable: bool| {
        let mut t = person();
        t.name = "card".into();
        t.fields.iter_mut().for_each(|f| f.erasable = false);
        t.fields
            .iter_mut()
            .filter(|f| f.name == "name")
            .for_each(|f| f.erasable = erasable);
        t.fields.push(crate::conformance::fixture::computed_by(
            "loud",
            StorageKind::Text,
            "yell",
            &["name"],
        ));
        t
    };
    l.declare(card(false)).await?;
    let set = crate::conformance::fixture::object_set_on("loud_ada", "card", "loud = 'Ada'");
    l.declare_as(set, "declaring the set `loud_ada`").await?;
    let ada = |k: &str| source_record("card", k, json!({"id": k, "name": "Ada"}));
    l.ingest("crm", vec![ada("c1")])
        .await
        .must("ingesting c1 in plaintext")?;
    l.declare(card(true)).await?;
    let (at, calls) = (
        store.head().await.must("reading the head")?,
        crate::conformance::backing::made(token),
    );
    l.ingest("crm", vec![ada("c2")])
        .await
        .must("ingesting c2, sealed")?;
    let (mut filled, mut joined) = (None, None);
    for e in events_after(store.as_ref(), at).await? {
        match &e.body {
            EventBody::OutputRecorded {
                field: Some((_, key, _)),
                ..
            } if key == "c2" => {
                filled = filled.or(Some(e.position));
            }
            EventBody::ObjectSetChanged {
                set,
                key,
                joined: true,
                ..
            } if set == "loud_ada" && key == "c2" => {
                joined = joined.or(Some(e.position));
            }
            _ => {}
        }
    }
    let r = l.read("card", "c2").await?;
    let called = crate::conformance::backing::made(token) - calls;
    ensure!(
        r.fields.get("loud") == Some(&json!("Ada"))
            && called == 1
            && filled.is_some_and(|f| joined.is_some_and(|j| j > f)),
        "`c2`'s first sealed write calls `yell` once ({called}), fills `loud` ({:?}) by its output (at {filled:?}), and joins `loud_ada` after it (at {joined:?})",
        r.fields.get("loud")
    );
    Ok(())
}
