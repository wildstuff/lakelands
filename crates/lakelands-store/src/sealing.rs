//! A source's values of an erasable field, sealed under that source's key
//! in the log and opened by the fold
//! (`docs/decisions/058-a-source-may-be-forgotten.md`).
//!
//! Each value is AES-256-GCM-SIV over its canonical bytes, written as the
//! base64 of the cipher's output. The nonce is made from where the value
//! stands: the source, the type, the record key, the version before the
//! event and the field; or, for a recorded output, the seam and the input.
//! So the same value against the same state seals to the same bytes, and
//! a redelivery keeps its ingest id, while two records or two versions
//! never share one. The fold makes the nonce again from the event, so a
//! sealed value moved to another place does not open.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use lakelands_api::error::{Seam, Situation};
use lakelands_api::event::{Digest, Event, EventBody, Sealed};
use lakelands_api::id::KeyId;
use lakelands_api::seam::{DataKey, KeyStore};
use lakelands_api::{Error, LakeId, Position, Result, SourceId, Value};
use lakelands_core::canonical;
use lakelands_core::lock;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

/// The sources' keys an open lake was given, asked of the host's key store
/// once and never written anywhere: by the writer, which seals and folds,
/// and by a read that computes through a kept step.
pub struct Keys {
    lake: LakeId,
    store: Option<Arc<dyn KeyStore>>,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// Each key, by source and version.
    by: BTreeMap<(SourceId, KeyId), DataKey>,
    /// The version of each source's key its new values are sealed with,
    /// and the newest the log had used when it was chosen.
    sealing: BTreeMap<SourceId, (KeyId, Option<KeyId>)>,
}

impl Keys {
    /// The keys of `lake`, from `store`.
    pub fn new(lake: LakeId, store: Option<Arc<dyn KeyStore>>) -> Keys {
        let held = Mutex::new(Held::default());
        Keys { lake, store, held }
    }

    fn store(&self) -> Result<&dyn KeyStore> {
        self.store
            .as_deref()
            .ok_or_else(|| Error::unbound(Seam::KeyStore))
    }

    /// The version of `source`'s key its new values are sealed with, and
    /// the key: the newest the key store holds, or a new one where it
    /// holds none and the log holds nothing `source` sealed (`used`, the
    /// versions the log used, [`USED`]). A key store that lost the last of
    /// them answers `key_missing` here, at the write, and is never given a
    /// new key the old values would read as erased. Without a key store,
    /// refused as `Unbound`.
    pub fn sealing(&self, source: &SourceId, used: &[KeyId]) -> Result<(KeyId, DataKey)> {
        // A choice stands while the log's newest version is the one it was
        // made over, or the one chosen: where another writer sealed with a
        // newer one while this one held no lease, it is made again, so the
        // versions a source seals with only rise.
        let held = lock(&self.held).sealing.get(source).cloned();
        let last = used.last();
        let stands =
            |(id, over): &(KeyId, Option<KeyId>)| last == Some(id) || last == over.as_ref();
        let id = held.filter(stands).map(|(id, _)| id);
        let id = match id {
            Some(id) => id,
            None => {
                let store = self.store()?;
                let ids = store.ids(&self.lake, source)?;
                if let Some(last) = used.last().filter(|u| !ids.contains(u)) {
                    let (source, key) = (source.clone(), last.clone());
                    return Err(Error::unavailable(Situation::KeyMissing { source, key }));
                }
                let id = match ids.last() {
                    Some(id) => id.clone(),
                    None => store.create(&self.lake, source)?,
                };
                let chosen = (id.clone(), last.cloned());
                lock(&self.held).sealing.insert(source.clone(), chosen);
                id
            }
        };
        let key = self.key(source, &id)?;
        Ok((id, key))
    }

    /// The key of `source` of version `id`. One the key store does not
    /// give fails closed: a lost key is never read as an erasure.
    pub fn key(&self, source: &SourceId, id: &KeyId) -> Result<DataKey> {
        self.opened(source, id)?.ok_or_else(|| {
            let (source, key) = (source.clone(), id.clone());
            Error::unavailable(Situation::KeyMissing { source, key })
        })
    }

    /// The key of `source` of version `id`, or `None` where the key store
    /// gives none: destroyed, which only a forgetting in the log makes an
    /// erasure ([`FORGOTTEN`]).
    pub fn opened(&self, source: &SourceId, id: &KeyId) -> Result<Option<DataKey>> {
        let at = (source.clone(), id.clone());
        if let Some(k) = lock(&self.held).by.get(&at) {
            return Ok(Some(k.clone()));
        }
        let Some(k) = self.store()?.key(&self.lake, source, id)? else {
            return Ok(None);
        };
        lock(&self.held).by.insert(at, k.clone());
        Ok(Some(k))
    }

    /// The versions of `source`'s key the key store holds; none without
    /// a key store, which seals nothing.
    pub fn ids(&self, source: &SourceId) -> Result<Vec<KeyId>> {
        match &self.store {
            Some(store) => store.ids(&self.lake, source),
            None => Ok(Vec::new()),
        }
    }

    /// Drop `source`'s keys from memory, then have the key store destroy
    /// every version of them (058). Destroying what is destroyed is no
    /// change.
    pub fn destroy(&self, source: &SourceId) -> Result<()> {
        {
            let mut held = lock(&self.held);
            held.by.retain(|(s, _), _| s != source);
            held.sealing.remove(source);
        }
        match &self.store {
            Some(store) if !store.ids(&self.lake, source)?.is_empty() => {
                store.destroy(&self.lake, source)
            }
            _ => Ok(()),
        }
    }
}

/// The store's own seam under which the fold keeps the sources the log
/// forgot, each with what of its act is still owed: `owed` where the
/// projection may hold its values, `rebuilt` where a rebuild after its
/// keys were destroyed erased them and the reclaim is owed, `done`.
pub const FORGOTTEN: &str = "sys:forgotten";

/// The input [`FORGOTTEN`] keeps its one list under.
pub fn forgotten_at() -> Digest {
    crate::backed::digest(&Value::from("forgotten"))
}

/// What of a forgetting is owed, by source, from the list [`FORGOTTEN`]
/// keeps.
pub fn forgotten(list: Option<&Value>) -> BTreeMap<SourceId, String> {
    let all = list.and_then(Value::as_object).into_iter().flatten();
    all.filter_map(|(s, v)| Some((SourceId::from(s.as_str()), v.as_str()?.to_owned())))
        .collect()
}

/// Erase in place what `event`'s `sealed` names, a forgotten source's
/// values: each patch member is cleared, and a recorded output dropped.
/// The fields erased, for their provenance to say so.
pub fn erase(event: &mut Event) -> Vec<lakelands_api::FieldName> {
    let Some(sealed) = event.sealed.take() else {
        return Vec::new();
    };
    match &mut event.body {
        EventBody::RecordIngested { patch, .. } => {
            for f in &sealed.fields {
                if let Some(v) = patch.get_mut(f) {
                    *v = Value::Null;
                }
            }
            sealed.fields
        }
        EventBody::OutputRecorded { output, .. } => {
            *output = None;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// The store's own seam under which the fold keeps, for each source, the
/// versions of its key the log sealed with, each with the position of its
/// first use, in that order. A source seals with the newest version its
/// key store holds when it is first needed and keeps it, and a key store
/// without the newest the log used is refused ([`Keys::sealing`]), so the
/// version of a value is the last one first used at or before its
/// position: a recorded output is digested under the oldest of its
/// inputs', whatever the key store holds now, so a rebuild finds it again
/// (058).
pub const USED: &str = "sys:sealed";

/// The input [`USED`] keeps `source`'s versions under.
pub fn used_by(source: &SourceId) -> Digest {
    crate::backed::digest(&Value::from(source.as_str()))
}

/// The versions of a source's key the list [`USED`] keeps holds.
pub fn versions(list: Option<&Value>) -> Vec<KeyId> {
    firsts(list).into_iter().map(|(k, _)| k).collect()
}

/// The versions the list [`USED`] keeps, each with the position of its
/// first use. A projection kept before these positions is rebuilt at the
/// writer's open, so no list without them is read.
pub fn firsts(list: Option<&Value>) -> Vec<(KeyId, Position)> {
    let ids = list.and_then(Value::as_array).into_iter().flatten();
    let first = |v: &Value| {
        Some((
            KeyId::from(v.get(0)?.as_str()?),
            Position(v.get(1)?.as_u64()?),
        ))
    };
    ids.filter_map(first).collect()
}

/// The list [`USED`] keeps for `source` once `key` seals a value at `at`,
/// with the place it is kept under: none where the list, as `staged` holds
/// it or else `view`, names `key` already. The fold and a write's plan
/// both keep it so, and read the same versions.
pub fn first_use(
    staged: &crate::backed::Recorded,
    view: &dyn lakelands_api::engine::ProjectionView,
    (source, key, at): (&SourceId, &KeyId, Position),
) -> Result<Option<((String, Digest), Value)>> {
    let place = (USED.to_owned(), used_by(source));
    let list = match staged.get(&place).and_then(|l| l.last()) {
        Some((_, v)) => Some(v.clone()),
        None => view.output(&place.0, &place.1)?.map(|(_, v)| v),
    };
    let mut used = firsts(list.as_ref());
    if used.iter().any(|(k, _)| k == key) {
        return Ok(None);
    }
    used.push((key.clone(), at));
    let list = serde_json::to_value(&used).expect("key ids serialise");
    Ok(Some((place, list)))
}

/// The version that sealed a value written at `at`, by `firsts`.
pub fn sealed_at(firsts: &[(KeyId, Position)], at: Position) -> Option<&KeyId> {
    firsts.iter().rev().find(|(_, p)| *p <= at).map(|(k, _)| k)
}

/// The nonce of a value: the first 12 bytes of SHA-256 over its place.
fn nonce(parts: &[&[u8]]) -> [u8; 12] {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0]);
        }
        h.update(p);
    }
    let mut n = [0; 12];
    n.copy_from_slice(&h.finalize()[..12]);
    n
}

/// The places of what `sealed` names in `body`, each with its nonce: the
/// patch members, or the recorded output.
fn places(body: &EventBody, sealed: &Sealed, source: &SourceId) -> Vec<(Option<String>, [u8; 12])> {
    let source = source.as_str().as_bytes();
    match body {
        EventBody::RecordIngested {
            ty, key, version, ..
        } => {
            let before = version.0.saturating_sub(1).to_string();
            let at = |f: &str| {
                let parts = [source, ty.as_str().as_bytes(), key.as_str().as_bytes()];
                nonce(&[&parts[..], &[before.as_bytes(), f.as_bytes()]].concat())
            };
            sealed
                .fields
                .iter()
                .map(|f| (Some(f.to_string()), at(f.as_str())))
                .collect()
        }
        EventBody::OutputRecorded { seam, input, .. } => {
            let parts: [&[u8]; 3] = [source, seam.as_bytes(), input.0.as_bytes()];
            vec![(None, nonce(&parts))]
        }
        _ => Vec::new(),
    }
}

/// The member a place names in `body`.
fn member<'a>(body: &'a mut EventBody, field: Option<&str>) -> Option<&'a mut Value> {
    match (body, field) {
        (EventBody::RecordIngested { patch, .. }, Some(f)) => patch.get_mut(f),
        (EventBody::OutputRecorded { output, .. }, None) => output.as_mut(),
        _ => None,
    }
}

/// Seal in place what `sealed` names of `body`, a source's plaintext.
pub fn seal(body: &mut EventBody, sealed: &Sealed, source: &SourceId, key: &DataKey) {
    let cipher = Aes256GcmSiv::new((&key.0).into());
    for (field, n) in places(body, sealed, source) {
        if let Some(v) = member(body, field.as_deref()) {
            let plain = canonical::to_vec(v);
            let sealed = cipher
                .encrypt(Nonce::from_slice(&n), plain.as_slice())
                .expect("AES-256-GCM-SIV seals any value");
            *v = Value::String(STANDARD.encode(sealed));
        }
    }
}

/// Open in place what `event`'s `sealed` names under `key`. A value that
/// does not open under it is answered as such, at the event's position.
pub fn open(event: &mut Event, key: &DataKey) -> Result<()> {
    let Some(sealed) = event.sealed.clone() else {
        return Ok(());
    };
    let Some(source) = sealed.source.clone().or(event.source.clone()) else {
        return Ok(());
    };
    let cipher = Aes256GcmSiv::new((&key.0).into());
    let position = event.position;
    for (field, n) in places(&event.body, &sealed, &source) {
        let Some(v) = member(&mut event.body, field.as_deref()) else {
            continue;
        };
        let plain = v
            .as_str()
            .and_then(|s| STANDARD.decode(s).ok())
            .and_then(|b| cipher.decrypt(Nonce::from_slice(&n), b.as_slice()).ok())
            .and_then(|b| serde_json::from_slice(&b).ok());
        match plain {
            Some(p) => *v = p,
            None => return Err(Error::unavailable(Situation::Unsealable { position })),
        }
    }
    Ok(())
}

/// The digest of a call whose input holds sealed values: HMAC-SHA-256
/// under a key derived from the source's (HKDF-SHA-256, info
/// `lakelands:output:v1`), so no digest of plaintext enters the log and
/// none links to a value once the key is gone.
pub fn digest(key: &DataKey, call: &Value) -> Digest {
    let mac = |k: &[u8], parts: &[&[u8]]| {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(k).expect("HMAC takes any key");
        parts.iter().for_each(|p| m.update(p));
        Zeroizing::new(<[u8; 32]>::from(m.finalize().into_bytes()))
    };
    let prk = mac(&[0; 32], &[&key.0]);
    let derived = mac(&*prk, &[b"lakelands:output:v1".as_slice(), &[1]]);
    Digest(canonical::hex(&*mac(
        &*derived,
        &[&canonical::to_vec(call)],
    )))
}

#[cfg(test)]
mod tests;
