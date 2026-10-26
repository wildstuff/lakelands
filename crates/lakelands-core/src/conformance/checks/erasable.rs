//! A source's values of an erasable field are sealed under its key
//! (`docs/decisions/058-a-source-may-be-forgotten.md`).
//!
//! The log holds them as ciphertext the source's key opens, and the
//! projection holds them in plaintext: reads, a rebuild and a redelivery
//! answer as for any other value. The checks bring their own key store,
//! [`Keys`], since a key store is the host's.

use std::sync::Mutex;

use lakelands_api::LakeId;
use lakelands_api::id::KeyId;
use lakelands_api::seam::DataKey;

use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    SEALED_IN_THE_LOG = a_sources_value_of_an_erasable_field_is_sealed_in_the_log_and_read_plain,
    "058, 083: a source's value of an erasable field enters the log as `record_ingested` with `sealed` naming the key's version and the field, the patch leaving the value out and `sealed.box` holding it as base64 that is not the plaintext, while a field that is not erasable stays plaintext in the patch; the record reads the plaintext, at the head and after a rebuild; the same value sent again against the same state appends nothing, the same value on another record seals to another box, and a value that returns after another (A, B, A) appends again; with no key store bound such a value is refused as unbound and nothing is appended",
    reopens
);

check!(
    ONE_BOX = a_records_sealed_members_are_sealed_in_one_box,
    "083: a source's ingest of a record with two erasable members seals both in one `sealed.box`, the patch leaving both out and `sealed.fields` naming both, and its id is made with the box (`lakelands:ingest:v2`); the record reads back each value, after a rebuild too; the same values against the same state append nothing; the same values written again at a later version seal to another box; and once the source is forgotten, neither value reads",
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
    "058: a function computed outside the lake that reads a source's erasable value records its output sealed under that source's key, `sealed` naming the key and the source, with an input digest that is not the SHA-256 of the call, and the field reads the plaintext answer; a call that reads erasable values of two sources is not made, and its field holds no value and says why; a write that moves a call back to an input already recorded plans the field, and the sets that read it, as the fold computes them; and after a newer version of the key is made, a rebuild answers the same and calls nothing again; a source's write after it seals its values and its output under the newer version, digested under it, and a rebuild answers that output too and calls nothing; a declaration that re-plans the first record after it finds that record's output again; a call over values two versions sealed is sealed under the older; one ingest whose second record re-plans a value its first sealed under a version first used there plans it as the fold computes it: an output is digested and sealed under the oldest version that sealed the values it reads; and a key store behind the lake, without the newest version the log sealed with, is refused at the source's next write, of a type with no computed field too, naming that version, and appends nothing",
    reopens
);

check!(
    FIRST_SEALED = a_sources_first_sealed_write_is_planned_as_the_fold_computes_it,
    "058 (lakelands#1042): a source's first sealed write is planned under the version of its key the write seals with, as the fold computes it: `c1`'s `name`, written before `name` was erasable, has its output recorded under a plain digest; `c2`, the source's first sealed write with the same `name`, finds no output under its key in its plan or its fold, so `shout` is called for it once, and `c2` joins the set over `loud` only where its output fills it, never before",
    reopens
);

/// A key store in memory, for the checks: one key a version, made from a
/// counter, so nothing of it is random, and every id once in the process,
/// so a store made anew never gives an id it gave before; and, beside the
/// versions, each key of a delivery sealed apart (065).
#[derive(Default)]
pub struct Keys(
    Mutex<BTreeMap<(LakeId, SourceId), Versions>>,
    Mutex<BTreeMap<(LakeId, SourceId), Versions>>,
);

/// A source's keys by version, none where destroyed.
type Versions = Vec<(KeyId, Option<[u8; 32]>)>;

/// A new key of `source` in `held`, its id `prefix` and a counter.
fn made(
    held: &Mutex<BTreeMap<(LakeId, SourceId), Versions>>,
    (lake, source): (&LakeId, &SourceId),
    prefix: &str,
) -> KeyId {
    let mut held = crate::lock(held);
    let versions = held.entry((lake.clone(), source.clone())).or_default();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let id = KeyId::from(format!("{prefix}{n}"));
    let bytes: [u8; 32] =
        <sha2::Sha256 as sha2::Digest>::digest(format!("{lake}/{source}/{id}")).into();
    versions.push((id.clone(), Some(bytes)));
    id
}

impl KeyStore for Keys {
    fn create(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        Ok(made(&self.0, (lake, source), "k"))
    }

    fn key(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<Option<DataKey>> {
        for held in [&self.0, &self.1] {
            let held = crate::lock(held);
            let versions = held.get(&(lake.clone(), source.clone()));
            if let Some((_, k)) = versions.and_then(|v| v.iter().find(|(i, _)| i == id)) {
                return Ok(k.map(DataKey));
            }
        }
        Ok(None)
    }

    fn destroy(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<()> {
        for held in [&self.0, &self.1] {
            let mut held = crate::lock(held);
            for (_, k) in held.entry((lake.clone(), source.clone())).or_default() {
                *k = None;
            }
        }
        Ok(())
    }

    fn create_apart(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        Ok(made(&self.1, (lake, source), "a"))
    }

    fn destroy_apart(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        let mut held = crate::lock(&self.1);
        let apart = held.entry((lake.clone(), source.clone())).or_default();
        for (_, k) in apart.iter_mut().filter(|(i, _)| i == id) {
            *k = None;
        }
        Ok(())
    }

    fn destroy_version(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<()> {
        let mut held = crate::lock(&self.0);
        let versions = held.entry((lake.clone(), source.clone())).or_default();
        let Some(i) = versions.iter().position(|(i, k)| i == id && k.is_some()) else {
            let (source, key) = (source.clone(), id.clone());
            return Err(lakelands_api::Error::invalid(Case::KeyVersionNotHeld {
                source,
                key,
            }));
        };
        versions[..=i].iter_mut().for_each(|(_, k)| *k = None);
        Ok(())
    }

    fn destroys_versions(&self) -> bool {
        true
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

impl Keys {
    /// The keys a file keyring's file holds for `lake` (`<lake>.keys.json`,
    /// 058): each source's versions as hex, a destroyed source's none.
    pub(super) fn from_file(path: &std::path::Path, lake: &LakeId) -> Result<Keys, Failure> {
        let text =
            std::fs::read(path).map_err(|e| Failure::new(format!("reading the key file: {e}")))?;
        let ring: Value = serde_json::from_slice(&text)
            .map_err(|e| Failure::new(format!("the key file is no JSON: {e}")))?;
        let key = |k: &Value| {
            let (id, hex) = (k[0].as_str()?, k[1].as_str()?);
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(byte).collect();
            Some((KeyId::from(id), Some(<[u8; 32]>::try_from(bytes?).ok()?)))
        };
        let (mut held, mut apart) = (BTreeMap::new(), BTreeMap::new());
        for (source, versions) in ring["sources"].as_object().into_iter().flatten() {
            let of = |list: &str| -> Versions {
                let all = versions[list].as_array().into_iter().flatten();
                all.filter_map(key).collect()
            };
            let at = (lake.clone(), SourceId::from(source.as_str()));
            held.insert(at.clone(), of("keys"));
            apart.insert(at, of("apart"));
        }
        Ok(Keys(Mutex::new(held), Mutex::new(apart)))
    }

    /// Whether no key was ever made.
    #[cfg(test)]
    pub(in crate::conformance) fn is_new(&self) -> bool {
        crate::lock(&self.0).is_empty() && crate::lock(&self.1).is_empty()
    }

    /// A copy of every key, as a backup of the key store holds them.
    pub(super) fn backup(&self) -> Keys {
        let apart = Mutex::new(crate::lock(&self.1).clone());
        Keys(Mutex::new(crate::lock(&self.0).clone()), apart)
    }

    /// The keys of deliveries sealed apart of `source` it holds, none
    /// destroyed.
    pub(super) fn apart(&self, lake: &LakeId, source: &SourceId) -> Vec<KeyId> {
        let held = crate::lock(&self.1);
        let apart = held.get(&(lake.clone(), source.clone())).into_iter();
        let live = apart.flatten().filter(|(_, k)| k.is_some());
        live.map(|(i, _)| i.clone()).collect()
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
pub(in crate::conformance) fn with(keys: &Arc<Keys>) -> Seams {
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

/// The box an ingested event seals its members in, if it has one.
fn boxed(e: &Event) -> Option<&str> {
    e.sealed.as_ref()?.boxed.as_deref()
}

/// The value of `f` in `person` `key` at the head.
pub(super) async fn read(l: &Lake, key: &str, f: &str) -> Result<Value, Failure> {
    let r = l.read("person", key).await?;
    Ok(r.fields.get(f).cloned().unwrap_or(Value::Null))
}

/// Opens the lake again with `seams`, naming `what` if that fails.
pub(in crate::conformance) async fn again(
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
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
    let base64 = boxed(e).is_some_and(|s| !s.is_empty() && s.len() % 4 == 0 && !s.contains("Ada"));
    ensure!(
        base64 && stored(e, "name").is_null(),
        "the log holds `name` in `sealed.box` as base64 that is not the plaintext, and not in the patch; it holds {e:?}"
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
    let names: Vec<Option<&str>> = events.iter().map(boxed).collect();
    ensure!(
        names.len() == 4
            && names.iter().all(Option::is_some)
            && names[0] != names[1]
            && names[0] != names[3]
            && names[2] != names[3],
        "p1, p2, Bob into p1 and Ada into p1 again append four events whose boxes all differ; the log holds {names:?}"
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

async fn a_records_sealed_members_are_sealed_in_one_box(store: Store, reopen: Reopen) -> Held {
    let (_, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let mut t = person();
    t.fields.push(FieldDecl {
        erasable: true,
        ..sourced("email", StorageKind::Text)
    });
    l.declare(t).await?;
    let ada = || {
        let ada = json!({"id": "p1", "name": "Ada", "email": "ada@example.org", "city": "Oslo"});
        source_record("person", "p1", ada)
    };
    let before = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![ada()]).await.must("ingesting p1")?;
    let events = ingested(store.as_ref(), before).await?;
    let [e] = events.as_slice() else {
        return Err(Failure::new(format!(
            "one `record_ingested` was expected, the log holds {events:?}"
        )));
    };
    let mut fields = e
        .sealed
        .as_ref()
        .map(|s| s.fields.clone())
        .unwrap_or_default();
    fields.sort();
    ensure!(
        fields == ["email", "name"],
        "`sealed.fields` names `email` and `name`; it is {:?}",
        e.sealed
    );
    let one = boxed(e).is_some_and(|b| !b.contains("Ada") && !b.contains("ada@"));
    let out = stored(e, "name").is_null() && stored(e, "email").is_null();
    ensure!(
        one && out && stored(e, "city") == json!("Oslo"),
        "one `sealed.box` holds `name` and `email`, which the patch leaves out, and `city` stays in the patch; the event is {e:?}"
    );
    if let (EventBody::RecordIngested { patch, .. }, Some(b)) = (&e.body, boxed(e)) {
        let id =
            crate::log::boxed_ingest_id(&"crm".into(), &"person".into(), &"p1".into(), 0, patch, b);
        ensure!(
            e.id == id,
            "the ingest's id is made under `lakelands:ingest:v2` with its box: {}; it is {}",
            id.0,
            e.id.0
        );
    }
    let back = async || -> Held {
        for (f, want) in [("name", "Ada"), ("email", "ada@example.org")] {
            let v = read(&l, "p1", f).await?;
            ensure!(v == json!(want), "`p1` reads `{f}` {want}; it reads {v}");
        }
        Ok(())
    };
    back().await?;
    let head = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![ada()])
        .await
        .must("ingesting p1 again")?;
    let again_head = store.head().await.must("reading the head")?;
    ensure!(
        again_head == head,
        "the same values against the same state append nothing; the head moved from {head} to {again_head}"
    );
    let bob = source_record("person", "p1", json!({"name": "Bob"}));
    l.ingest("crm", vec![bob])
        .await
        .must("ingesting Bob into p1")?;
    l.ingest("crm", vec![ada()])
        .await
        .must("ingesting Ada into p1 again")?;
    let events = ingested(store.as_ref(), before).await?;
    let boxes: Vec<Option<&str>> = events.iter().map(boxed).collect();
    ensure!(
        boxes.len() == 3 && boxes.iter().all(Option::is_some) && boxes[0] != boxes[2],
        "the same values at a later version seal to another box; the log holds {boxes:?}"
    );
    store.rebuild().await.must("rebuilding the lake")?;
    back().await?;
    // An author's note keeps `p1` when crm, which brought it, is forgotten.
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let crm: SourceId = "crm".into();
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    store
        .forget_source(&crm, Some(&report.report_id))
        .await
        .must("forgetting crm")?;
    for f in ["name", "email"] {
        let v = read(&l, "p1", f).await?;
        ensure!(
            v.is_null(),
            "once crm is forgotten `p1` reads no `{f}`; it reads {v}"
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
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
    l.declare(t.clone()).await?;
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
    // A write after the newer version seals under it, its output too, so
    // destroying the older version would leave this output found.
    let newer = keys
        .ids(store.lake(), &"crm".into())
        .must("listing crm's keys")?;
    let newer = newer.last().cloned();
    let at = store.head().await.must("reading the head")?;
    let bea = source_record("person", "p2", json!({"id": "p2", "name": "Bea"}));
    l.ingest("crm", vec![bea]).await.must("ingesting p2")?;
    let events = events_after(store.as_ref(), at).await?;
    let keys_of = |shout: bool| {
        let of = |e: &&Event| match &e.body {
            EventBody::OutputRecorded { seam, .. } => shout && seam.ends_with(":shout"),
            EventBody::RecordIngested { .. } => !shout,
            _ => false,
        };
        let sealed = events
            .iter()
            .filter(of)
            .map(|e| e.sealed.as_ref().map(|s| s.key.clone()));
        sealed.collect::<Vec<_>>()
    };
    let (ingested, shouted) = (keys_of(false), keys_of(true));
    ensure!(
        newer.as_ref() != Some(&k) && ingested == [newer.clone()] && shouted == [newer.clone()],
        "p2's value and its output are sealed under the newer version {newer:?}, not {k}; the ingest names {ingested:?}, the output {shouted:?}"
    );
    let calls = crate::conformance::backing::made("sealed_output");
    store.rebuild().await.must("rebuilding the lake")?;
    let r = l.read("person", "p2").await?;
    let again_calls = crate::conformance::backing::made("sealed_output");
    ensure!(
        r.fields.get("loud") == Some(&json!("Bea")) && again_calls == calls,
        "after a rebuild `loud` of p2 reads Bea and `shout` is not called again; it reads {:?}, {calls} calls before, {again_calls} after",
        r.fields.get("loud")
    );
    // A declaration re-plans p1, whose name the older version sealed: its
    // output is digested under that version still, and found.
    let calls = crate::conformance::backing::made("sealed_output");
    t.fields.push(field("extra", StorageKind::Text));
    l.declare(t).await?;
    let r = l.read("person", "p1").await?;
    let again_calls = crate::conformance::backing::made("sealed_output");
    ensure!(
        r.fields.get("loud") == Some(&json!("Ada")) && again_calls == calls,
        "a declaration after the newer version re-plans `loud` under the version that sealed Ada, and finds its output; it reads {:?}, {calls} calls before, {again_calls} after",
        r.fields.get("loud")
    );
    // A call over values two versions sealed is sealed under the older:
    // p1's `name` under the first, its `nick` now crm's under the newer.
    let at = store.head().await.must("reading the head")?;
    let nick = source_record("person", "p1", json!({"nick": "A1"}));
    l.ingest("crm", vec![nick])
        .await
        .must("ingesting p1's nick from crm")?;
    let events = events_after(store.as_ref(), at).await?;
    let paired: Vec<_> = (events.iter())
        .filter(|e| matches!(&e.body, EventBody::OutputRecorded { seam, .. } if seam.ends_with(":pair")))
        .map(|e| e.sealed.as_ref().map(|s| s.key.clone()))
        .collect();
    ensure!(
        paired == [Some(k.clone())],
        "`both` reads p1's name, sealed under {k}, and its nick, sealed under {newer:?}: its output is sealed under {k}; the log holds {paired:?}"
    );
    // One call whose second record re-plans a value its first sealed under
    // a version first used there: the plan reads the version the fold
    // keeps, so it finds no output another version recorded, and a set
    // over the field moves only once the output is recorded.
    let set = crate::conformance::fixture::object_set_on("both_cy", "person", "both = 'Cy'");
    l.declare_as(set, "declaring the set `both_cy`").await?;
    let cy = |key: &str, fields: Value| source_record("person", key, fields);
    let p7 = cy("p7", json!({"id": "p7", "name": "Cy", "nick": "C"}));
    l.ingest("crm", vec![p7]).await.must("ingesting p7")?;
    keys.create(store.lake(), &"crm".into())
        .must("making a third version")?;
    let store = again(&reopen, store, with(&keys), "with a third version").await?;
    let l = Lake::of(store.as_ref());
    let at = store.head().await.must("reading the head")?;
    let p8 = [
        cy("p8", json!({"id": "p8", "name": "Cy"})),
        cy("p8", json!({"nick": "C"})),
    ];
    l.ingest("crm", p8.into())
        .await
        .must("ingesting p8 in one call")?;
    let (mut output, mut joined) = (None, None);
    for e in events_after(store.as_ref(), at).await? {
        match &e.body {
            EventBody::OutputRecorded { seam, .. } if seam.ends_with(":pair") => {
                output = output.or(Some(e.position));
            }
            EventBody::ObjectSetChanged { set, key, .. } if set == "both_cy" && key == "p8" => {
                joined = joined.or(Some(e.position));
            }
            _ => {}
        }
    }
    ensure!(
        output.is_some() && joined > output,
        "p8's `both` is called under the third version and its set moves after the output is recorded; the output is at {output:?}, the join at {joined:?}"
    );
    let newer = keys
        .ids(store.lake(), &"crm".into())
        .must("listing crm's keys")?;
    let newer = newer.last().cloned();
    // A key store behind the lake, as one restored from before the newest
    // version, is refused at the source's next write, naming that version:
    // a write with no computed field too, which would seal under the
    // older version and leave its values taken as the newer's.
    l.declare(lakelands_api::ontology::TypeDecl {
        name: "card".into(),
        ..person()
    })
    .await?;
    let behind = {
        let mut held = crate::lock(&keys.0).clone();
        held.values_mut().for_each(|v| v.truncate(1));
        Arc::new(Keys(Mutex::new(held), Mutex::default()))
    };
    let store = again(&reopen, store, with(&behind), "with a key store behind it").await?;
    let before = store.head().await.must("reading the head")?;
    let cy = source_record("card", "c1", json!({"id": "c1", "name": "Cy"}));
    let refused = Lake::of(store.as_ref()).ingest("crm", vec![cy]).await;
    let named = |s: &Situation| matches!(s, Situation::KeyMissing { source, key } if source == "crm" && Some(key) == newer.as_ref());
    ensure!(
        matches!(&refused, Err(Error::Unavailable { situation }) if named(situation)),
        "a key store without the newest version the log sealed with is refused, naming {newer:?}; it answered {refused:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "the write a key store behind the lake refused",
    )
    .await?;
    Ok(())
}

async fn a_sources_first_sealed_write_is_planned_as_the_fold_computes_it(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (_, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
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
