//! A source's values of an erasable field, sealed under that source's key
//! in the log and opened by the fold
//! (`docs/decisions/058-a-source-may-be-forgotten.md`).
//!
//! What is sealed is AES-256-GCM-SIV over canonical bytes, written as the
//! base64 of the cipher's output. A record event's members are sealed
//! together, the object of them in one box the patch leaves out, from
//! format 20 (`docs/decisions/083-a-log-stores-what-an-append-shares-once-and-seals-a-records-values-in-one-box.md`):
//! one tag a record, not one a field, and erasing loses nothing by it, as
//! the members were under one key already. Up to format 19 each member
//! was sealed alone, and still opens so. The nonce is made from where the
//! values stand: the source, the type, the record key and the version
//! before the event, and, for a member alone, the field; or, for a
//! recorded output, the seam and the input. So the same values against
//! the same state seal to the same bytes, and a redelivery keeps its
//! ingest id, while two records or two versions never share one. The fold
//! makes the nonce again from the event, so a sealed value moved to
//! another place does not open.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use lakelands_api::engine::{LakeEngine, ProjectionView};
use lakelands_api::error::{Seam, Situation};
use lakelands_api::event::{Digest, Event, EventBody, Sealed};
use lakelands_api::id::KeyId;
use lakelands_api::seam::{DataKey, KeyStore};
use lakelands_api::{Error, FieldName, LakeId, Position, Result, SourceId, Value};
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
    /// The lake's log, which names the keys a delivery sealed apart sealed
    /// each value under ([`Self::apart_named`]).
    log: Option<Arc<dyn LakeEngine>>,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// Each key, by source and version.
    by: BTreeMap<(SourceId, KeyId), DataKey>,
    /// The version of each source's key its new values are sealed with,
    /// and the newest the log had used when it was chosen.
    sealing: BTreeMap<SourceId, (KeyId, Option<KeyId>)>,
    /// Each source's list [`USED`] as the projection held it at a
    /// position ([`Keys::listed`]).
    lists: BTreeMap<SourceId, (Position, Arc<List>)>,
    /// Raised each time lists are forgotten: a list read before is not
    /// kept after, as a reader outside the writer's lock may have read it
    /// from a view a rebuild was clearing.
    generation: u64,
    /// The versions an act of removal destroys (065), from the moment it
    /// names them: none seals again, though the key store has not yet
    /// destroyed it.
    doomed: BTreeSet<(SourceId, KeyId)>,
}

/// The list [`USED`] keeps for a source, read once: as kept, and each
/// version with the position of its first use, in that order.
pub struct List {
    /// The list as kept.
    pub value: Value,
    /// Each version with the position of its first use.
    pub firsts: Vec<(KeyId, Position)>,
    /// When the newest version was first used, where the list says.
    pub newest: Option<lakelands_api::Timestamp>,
    /// The versions a delivery sealed apart sealed under, which the list
    /// marks: only a value of one of these is read for its apart keys
    /// ([`Keys::apart_named`]).
    pub apart: Vec<KeyId>,
}

impl List {
    /// The list kept as `value`.
    pub fn of(value: Value) -> List {
        let firsts = firsts(Some(&value));
        let last = value.as_array().and_then(|l| l.last());
        let newest = last.and_then(|l| l.get(2)?.as_i64());
        let newest = newest.map(lakelands_api::Timestamp);
        let entries = value.as_array().into_iter().flatten();
        let marked = entries.filter(|l| l.get(3) == Some(&Value::Bool(true)));
        let apart = marked.filter_map(|l| Some(KeyId::from(l.get(0)?.as_str()?)));
        let apart = apart.collect();
        List {
            value,
            firsts,
            newest,
            apart,
        }
    }
}

impl Keys {
    /// The keys of `lake`, from `store`, whose log `log` holds.
    pub fn new(
        lake: LakeId,
        store: Option<Arc<dyn KeyStore>>,
        log: Option<Arc<dyn LakeEngine>>,
    ) -> Keys {
        let held = Mutex::new(Held::default());
        Keys {
            lake,
            store,
            log,
            held,
        }
    }

    /// The keys apart that `source`'s ingest at `at` names in `sealed`
    /// (065): as `known` holds them, else read from the log, one read a
    /// position, and kept in `known`. None where the event there is not
    /// an ingest of `source`'s, or the keys were given no log.
    pub fn apart_named(
        &self,
        known: &Apart,
        source: &SourceId,
        at: Position,
    ) -> Result<Vec<KeyId>> {
        let cached = lock(&known.0).get(&at).cloned();
        let named = match cached {
            Some(named) => named,
            None => {
                let Some(log) = &self.log else {
                    return Ok(Vec::new());
                };
                let read = log.read(at, 1)?;
                let stored = read.first().filter(|s| s.position == at);
                let e = stored.map(crate::log::open).transpose()?;
                let ingest = e.filter(|e| matches!(e.body, EventBody::RecordIngested { .. }));
                let named = ingest.and_then(|e| Some((e.source?, e.sealed?.apart)));
                lock(&known.0).insert(at, named.clone());
                named
            }
        };
        let of = named.filter(|(s, _)| s == source);
        Ok(of.map(|(_, apart)| apart).unwrap_or_default())
    }

    fn store(&self) -> Result<&dyn KeyStore> {
        self.store
            .as_deref()
            .ok_or_else(|| Error::unbound(Seam::KeyStore))
    }

    /// The version of `source`'s key its new values are sealed with, and
    /// the key: the newest the key store holds, or a new one where it
    /// holds none and the log holds nothing `source` sealed (`newest`, the
    /// newest version the log used, [`USED`]), or where `rotate` says it
    /// belongs to a period past ([`rotates`]). A key store that lost the
    /// last of them answers `key_missing` here, at the write, and is never
    /// given a new key the old values would read as erased. So does a
    /// version an act of removal named, which the key store may not have
    /// destroyed yet: a value sealed under it would be erased with it.
    /// Without a key store, refused as `Unbound`.
    pub fn sealing(
        &self,
        source: &SourceId,
        newest: Option<&KeyId>,
        rotate: bool,
    ) -> Result<(KeyId, DataKey)> {
        // A choice stands while the log's newest version is the one it was
        // made over, or the one chosen: where another writer sealed with a
        // newer one while this one held no lease, it is made again, so the
        // versions a source seals with only rise.
        let held = lock(&self.held).sealing.get(source).cloned();
        let last = newest;
        let held = held.filter(|(id, _)| !(rotate && last == Some(id)));
        let stands =
            |(id, over): &(KeyId, Option<KeyId>)| last == Some(id) || last == over.as_ref();
        let id = held.filter(stands).map(|(id, _)| id);
        let id = match id {
            Some(id) => id,
            None => {
                let store = self.store()?;
                let mut ids = store.ids(&self.lake, source)?;
                {
                    let held = lock(&self.held);
                    ids.retain(|k| !held.doomed.contains(&(source.clone(), k.clone())));
                }
                if let Some(last) = newest.filter(|u| !ids.contains(u)) {
                    let (source, key) = (source.clone(), last.clone());
                    return Err(Error::unavailable(Situation::KeyMissing { source, key }));
                }
                // A version made and not yet used is a new period's
                // already.
                let id = match ids.last() {
                    Some(id) if !(rotate && newest == Some(id)) => id.clone(),
                    _ => store.create(&self.lake, source)?,
                };
                let chosen = (id.clone(), last.cloned());
                lock(&self.held).sealing.insert(source.clone(), chosen);
                id
            }
        };
        let key = self.key(source, &id)?;
        Ok((id, key))
    }

    /// A new key of `source` outside its versions, for the delivery an
    /// ingest seals apart (065): refused as unbound where the key store
    /// makes none.
    pub fn apart(&self, source: &SourceId) -> Result<(KeyId, DataKey)> {
        let id = self.store()?.create_apart(&self.lake, source)?;
        let key = self.key(source, &id)?;
        Ok((id, key))
    }

    /// The key `sealed` names of `source`: its version, composed with the
    /// keys of deliveries sealed apart it names ([`compose`]). One the key
    /// store does not give fails closed, naming it.
    pub fn sealed(&self, source: &SourceId, sealed: &Sealed) -> Result<DataKey> {
        present(source, self.opened_sealed(source, sealed)?)
    }

    /// [`Self::sealed`], or those of its keys the key store does not
    /// give, in its order: destroyed, which only an act in the log makes
    /// an erasure.
    pub fn opened_sealed(
        &self,
        source: &SourceId,
        sealed: &Sealed,
    ) -> Result<std::result::Result<DataKey, Vec<KeyId>>> {
        let (mut keys, mut missing) = (Vec::with_capacity(1 + sealed.apart.len()), Vec::new());
        for id in std::iter::once(&sealed.key).chain(&sealed.apart) {
            match self.opened(source, id)? {
                Some(k) => keys.push(k),
                None => missing.push(id.clone()),
            }
        }
        if !missing.is_empty() {
            return Ok(Err(missing));
        }
        let (version, apart) = keys.split_first().expect("a seal names its version");
        Ok(Ok(compose(version, apart)))
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

    /// The list [`USED`] keeps for `source` as `view` holds it, read from
    /// the engine once at each position of the view: it grows by an entry
    /// a key period, and is read at every sealed value, so reading it
    /// whole each time would cost a write in proportion to the source's
    /// periods. It changes only with an event that seals, which moves the
    /// position, and a rebuild, which forgets every list kept
    /// ([`Self::unlist`]), as [`Self::destroy`] forgets a source's; a view
    /// of no position is read each time. A list read across a forgetting
    /// of the lists is not kept, as the shapes are not: a reader outside
    /// the writer's lock may have read it before the rebuild's clear. A
    /// rebuild by the writer of
    /// another process is not seen here: a list read while it refolds is
    /// kept until the head moves past the position it was read at.
    pub fn listed(
        &self,
        view: &dyn ProjectionView,
        source: &SourceId,
    ) -> Result<Option<Arc<List>>> {
        let at = view.position();
        let (kept, generation) = {
            let held = lock(&self.held);
            (held.lists.get(source).cloned(), held.generation)
        };
        if let (Some(at), Some((was, list))) = (at, kept)
            && was == at
        {
            return Ok(Some(list));
        }
        let Some((_, value)) = view.output(USED, &used_by(source))? else {
            return Ok(None);
        };
        let list = Arc::new(List::of(value));
        let mut held = lock(&self.held);
        if let Some(at) = at.filter(|_| held.generation == generation) {
            held.lists.insert(source.clone(), (at, list.clone()));
        }
        Ok(Some(list))
    }

    /// Forget every list [`Self::listed`] kept: a rebuild folds them again.
    pub fn unlist(&self) {
        let mut held = lock(&self.held);
        held.lists.clear();
        held.generation += 1;
    }

    /// Whether the key store holds `source`'s key `id`, asked of it, not
    /// of memory; none is held without a key store.
    pub fn holds(&self, source: &SourceId, id: &KeyId) -> Result<bool> {
        match &self.store {
            Some(store) => Ok(store.key(&self.lake, source, id)?.is_some()),
            None => Ok(false),
        }
    }

    /// The versions of `source`'s key the key store holds; none without
    /// a key store, which seals nothing.
    pub fn ids(&self, source: &SourceId) -> Result<Vec<KeyId>> {
        match &self.store {
            Some(store) => store.ids(&self.lake, source),
            None => Ok(Vec::new()),
        }
    }

    /// Whether the key store destroys a version with those before it: an
    /// act of removal shreds nothing where it does not (065).
    pub fn shreds(&self) -> bool {
        self.store.as_ref().is_some_and(|s| s.destroys_versions())
    }

    /// Drop `source`'s keys from memory, then have the key store destroy
    /// every version of them (058), and mark the source destroyed though
    /// an act of removal took every version already (065). Destroying what
    /// is destroyed is no change.
    pub fn destroy(&self, source: &SourceId) -> Result<()> {
        {
            let mut held = lock(&self.held);
            held.by.retain(|(s, _), _| s != source);
            held.sealing.remove(source);
            held.lists.remove(source);
            held.generation += 1;
        }
        match &self.store {
            Some(store) => store.destroy(&self.lake, source),
            None => Ok(()),
        }
    }

    /// Drop the versions `keys` of `source`'s key, a prefix of them oldest
    /// first, from memory, so the open lake seals and opens with none of
    /// them again, then have the key store destroy the newest it still
    /// holds, and with it every older one (065). A prefix destroyed
    /// already is no change.
    pub fn destroy_versions(&self, source: &SourceId, keys: &[KeyId]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        {
            let mut held = lock(&self.held);
            held.by
                .retain(|(s, id), _| s != source || !keys.contains(id));
            if held
                .sealing
                .get(source)
                .is_some_and(|(id, _)| keys.contains(id))
            {
                held.sealing.remove(source);
            }
            held.lists.remove(source);
            held.generation += 1;
            let doomed = keys.iter().map(|k| (source.clone(), k.clone()));
            held.doomed.extend(doomed);
        }
        // With no key store there is none to destroy, as for a source's
        // whole key: an import with none needs no key for these versions.
        let Some(store) = &self.store else {
            return Ok(());
        };
        let ids = store.ids(&self.lake, source)?;
        match keys.iter().rev().find(|k| ids.contains(k)) {
            Some(newest) => store.destroy_version(&self.lake, source, newest),
            None => Ok(()),
        }
    }

    /// Drop the keys `keys` of `source`'s deliveries sealed apart from
    /// memory, so none seals again, though the key store has not yet
    /// destroyed it, then have the key store destroy each it still holds
    /// (065). A key gone already is no change, so a key store that makes
    /// no such key is never asked.
    pub fn destroy_apart(&self, source: &SourceId, keys: &[KeyId]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        {
            let mut held = lock(&self.held);
            held.by
                .retain(|(s, id), _| s != source || !keys.contains(id));
            let doomed = keys.iter().map(|k| (source.clone(), k.clone()));
            held.doomed.extend(doomed);
        }
        let Some(store) = &self.store else {
            return Ok(());
        };
        for id in keys {
            if self.holds(source, id)? {
                store.destroy_apart(&self.lake, source, id)?;
            }
        }
        Ok(())
    }

    /// Whether an act of removal named `source`'s key `id`: from then on
    /// it seals nothing.
    pub fn doomed(&self, source: &SourceId, id: &KeyId) -> bool {
        lock(&self.held)
            .doomed
            .contains(&(source.clone(), id.clone()))
    }
}

/// The ciphers one pass that seals or opens many values builds (an
/// append, an ingest's window, a fold of a page or of an append): each
/// key a seal names built into its [`cipher`] at its first value, not at
/// each, by source and the versions and keys apart it is composed of.
/// They live only as long as the pass, so none outlives a key an act
/// drops after it, and the lake keeps none.
#[derive(Default)]
pub struct Ciphers(BTreeMap<(SourceId, Vec<KeyId>), Aes256GcmSiv>);

impl Ciphers {
    /// The cipher of the key `sealed` names of `source`, failing closed
    /// as [`Keys::sealed`] does.
    pub fn sealed(
        &mut self,
        keys: &Keys,
        source: &SourceId,
        sealed: &Sealed,
    ) -> Result<&Aes256GcmSiv> {
        present(source, self.opened(keys, source, sealed)?)
    }

    /// The cipher of [`Keys::opened_sealed`]'s key, or those of its keys
    /// the key store does not give, as it answers them.
    pub fn opened(
        &mut self,
        keys: &Keys,
        source: &SourceId,
        sealed: &Sealed,
    ) -> Result<std::result::Result<&Aes256GcmSiv, Vec<KeyId>>> {
        let ids = std::iter::once(&sealed.key).chain(&sealed.apart);
        let at = (source.clone(), ids.cloned().collect());
        Ok(Ok(match self.0.entry(at) {
            Entry::Occupied(c) => c.into_mut(),
            Entry::Vacant(v) => match keys.opened_sealed(source, sealed)? {
                Ok(key) => v.insert(cipher(&key)),
                Err(missing) => return Ok(Err(missing)),
            },
        }))
    }
}

/// What `opened` gives of `source`'s keys, or a refusal naming the first
/// of them missing.
fn present<T>(source: &SourceId, opened: std::result::Result<T, Vec<KeyId>>) -> Result<T> {
    opened.map_err(|mut missing| {
        let (source, key) = (source.clone(), missing.remove(0));
        Error::unavailable(Situation::KeyMissing { source, key })
    })
}

/// The cipher of `key`: the AES schedule of `key` as AES-GCM-SIV's
/// key-generating key, wiped when the cipher is dropped. Only that is
/// saved where a cipher is kept: each value sealed or opened still
/// derives its own authentication and encryption keys from its nonce,
/// and expands the encryption key's schedule.
pub fn cipher(key: &DataKey) -> Aes256GcmSiv {
    Aes256GcmSiv::new((&key.0).into())
}

/// One key from a version of a source's key and the keys of deliveries
/// sealed apart that seal with it (065): HKDF-SHA-256 with an empty salt
/// over the version's bytes then each apart key's, info
/// `lakelands:apart:v1`. With none apart it is the version itself, so a
/// value sealed before format 18 opens as it was sealed.
pub fn compose(version: &DataKey, apart: &[DataKey]) -> DataKey {
    if apart.is_empty() {
        return version.clone();
    }
    let mac = |k: &[u8], parts: &[&[u8]]| {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(k).expect("HMAC takes any key");
        parts.iter().for_each(|p| m.update(p));
        Zeroizing::new(<[u8; 32]>::from(m.finalize().into_bytes()))
    };
    let ikm: Vec<&[u8]> = std::iter::once(&version.0[..])
        .chain(apart.iter().map(|k| &k.0[..]))
        .collect();
    let prk = mac(&[0; 32], &ikm);
    DataKey(*mac(&*prk, &[b"lakelands:apart:v1".as_slice(), &[1]]))
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
            // A box's members are not in the patch: each is erased there.
            for f in &sealed.fields {
                patch.insert(f.clone(), Value::Null);
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

/// The keys outside its source's versions that each ingest a delivery
/// sealed apart names in `sealed`, by position (065): a value's version
/// is the one its position says ([`USED`]), and its apart keys are the
/// ones its own event names, so a recorded output composes the same keys
/// at the write and at a rebuild. Each is read from the log once
/// ([`Keys::apart_named`]) and kept while a fold or a write's plan lasts; a
/// plan stages its own events' here, as they are not in the log yet.
#[derive(Default)]
pub struct Apart(Mutex<BTreeMap<Position, Named>>);

/// The source and keys apart of the ingest at a position, `None` where
/// the event there is no ingest.
type Named = Option<(SourceId, Vec<KeyId>)>;

impl Apart {
    /// Stage the keys apart `source`'s ingest at `at` names, before it is
    /// in the log.
    pub fn stage(&self, source: &SourceId, at: Position, apart: &[KeyId]) {
        lock(&self.0).insert(at, Some((source.clone(), apart.to_vec())));
    }
}

impl Clone for Apart {
    fn clone(&self) -> Apart {
        Apart(Mutex::new(lock(&self.0).clone()))
    }
}

/// Whether a source whose newest version was first used at `first`
/// (none where the list does not say) seals a value written at `now`
/// under a new one: where the source has a key period and `now` is past
/// the start of a later period than `first`'s
/// (`lakelands_api::seam::KEY_PERIOD_SETTING`).
pub fn rotates(
    first: Option<lakelands_api::Timestamp>,
    now: lakelands_api::Timestamp,
    period: Option<&str>,
) -> bool {
    let start = |t: lakelands_api::Timestamp| period.and_then(|p| t.period_start(p));
    match (start(now), first) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(now), Some(first)) => start(first).is_some_and(|f| f < now),
    }
}

/// Whether `source`, whose versions the list [`USED`] keeps as `list`,
/// seals a value written at `now` under a new version: by its own key
/// period where the lake's `settings` hold one, else the lake's, from
/// its newest version's first use ([`rotates`]).
pub fn turns(
    settings: &BTreeMap<String, Value>,
    list: Option<&List>,
    source: &SourceId,
    now: lakelands_api::Timestamp,
) -> bool {
    use lakelands_api::settings::{lake_setting, value_of};
    let setting = lake_setting(lakelands_api::seam::KEY_PERIOD_SETTING).map(|(s, _)| s);
    let period = setting.map(|s| value_of(settings, s, Some(source.as_str())));
    let first = list.and_then(|l| l.newest);
    rotates(first, now, period.as_ref().and_then(Value::as_str))
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

/// What the fold keeps once `sealed` seals a value of `source` at `at`,
/// written at `known`, with the place it is kept under: the list [`USED`]
/// where its version is first used there, or first seals a delivery
/// sealed apart ([`first_use`]). Its apart keys stay in the log alone
/// ([`Apart`]). The fold and a write's plan both keep it so, and read the
/// same keys.
pub fn kept(
    read: (&crate::backed::Recorded, &dyn ProjectionView, &Keys),
    (source, sealed, at, known): (&SourceId, &Sealed, Position, lakelands_api::Timestamp),
) -> Result<Option<((String, Digest), Value)>> {
    let apart = !sealed.apart.is_empty();
    first_use(read, (source, &sealed.key, at, known), apart)
}

/// The list [`USED`] keeps for `source` once `key` seals a value at `at`,
/// written at `known`, `apart` where a delivery sealed apart seals it,
/// with the place it is kept under: none where the list, as `staged`
/// holds it or else `view` ([`Keys::listed`]), names `key` already, and
/// marks it where `apart`. A mark is a fourth member, `true`, of the
/// version's entry, so a recorded output reads a value's apart keys only
/// at a value whose version it marks ([`Keys::apart_named`]).
fn first_use(
    (staged, view, keys): (&crate::backed::Recorded, &dyn ProjectionView, &Keys),
    (source, key, at, known): (&SourceId, &KeyId, Position, lakelands_api::Timestamp),
    apart: bool,
) -> Result<Option<((String, Digest), Value)>> {
    let place = (USED.to_owned(), used_by(source));
    let list = match staged.get(&place).and_then(|l| l.last()) {
        Some((_, v)) => Some(Arc::new(List::of(v.clone()))),
        None => keys.listed(view, source)?,
    };
    // A source seals with its newest version, so it is looked for first.
    let named = |l: &Arc<List>| l.firsts.iter().rev().any(|(k, _)| k == key);
    let marked = |l: &Arc<List>| !apart || l.apart.contains(key);
    if list.as_ref().is_some_and(|l| named(l) && marked(l)) {
        return Ok(None);
    }
    let used = list.and_then(|l| l.value.as_array().cloned());
    let mut used = used.unwrap_or_default();
    let entry =
        (used.iter_mut().rev()).find(|l| l.get(0).and_then(Value::as_str) == Some(key.as_str()));
    match entry.and_then(Value::as_array_mut) {
        Some(entry) => {
            entry.resize(3, Value::Null);
            entry.push(Value::Bool(true));
        }
        None if apart => used.push(serde_json::json!([key, at, known.0, true])),
        None => used.push(serde_json::json!([key, at, known.0])),
    }
    Ok(Some((place, Value::Array(used))))
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
    let digest = h.finalize();
    std::array::from_fn(|i| digest[i])
}

/// The nonce of what a source's key seals in `body`, by its place: a
/// record's box, from its source, type, key and version before, or, up to
/// format 19, its member `field` alone; or a recorded output, from its
/// source, seam and input. No other event holds a sealed value.
fn place(source: &SourceId, body: &EventBody, field: Option<&str>) -> Option<[u8; 12]> {
    let source = source.as_str().as_bytes();
    match body {
        EventBody::RecordIngested {
            ty, key, version, ..
        } => {
            let before = version.0.saturating_sub(1).to_string();
            let parts = [source, ty.as_str().as_bytes(), key.as_str().as_bytes()];
            let parts = [&parts[..], &[before.as_bytes()]].concat();
            Some(match field {
                Some(f) => nonce(&[&parts[..], &[f.as_bytes()]].concat()),
                None => nonce(&parts),
            })
        }
        EventBody::OutputRecorded { seam, input, .. } => {
            Some(nonce(&[source, seam.as_bytes(), input.0.as_bytes()]))
        }
        _ => None,
    }
}

/// Seal in place what `sealed` names of `body`, a source's plaintext: a
/// record's members, taken out of its patch, together in `sealed`'s box,
/// the canonical bytes of their object (083); or a recorded output.
pub fn seal(body: &mut EventBody, sealed: &mut Sealed, source: &SourceId, cipher: &Aes256GcmSiv) {
    let close = |v: &Value, n: [u8; 12]| {
        let plain = canonical::to_vec(v);
        let sealed = cipher
            .encrypt(Nonce::from_slice(&n), plain.as_slice())
            .expect("AES-256-GCM-SIV seals any value");
        STANDARD.encode(sealed)
    };
    let Some(n) = place(source, body, None) else {
        return;
    };
    match body {
        EventBody::RecordIngested { patch, .. } => {
            let fields = sealed.fields.iter();
            let taken = fields.filter_map(|f| Some((f.to_string(), patch.remove(f)?)));
            sealed.boxed = Some(close(&Value::Object(taken.collect()), n));
        }
        EventBody::OutputRecorded {
            output: Some(v), ..
        } => *v = Value::String(close(v, n)),
        _ => {}
    }
}

/// Open in place what `event`'s `sealed` names under `cipher`'s key: a
/// record's box back into its patch, each member sealed alone up to
/// format 19, or a recorded output. A value that does not open under it, or a box that
/// does not hold exactly the members `sealed` names, none of them in the
/// patch, is answered as such, at the event's position.
pub fn open(event: &mut Event, cipher: &Aes256GcmSiv) -> Result<()> {
    let Some(sealed) = event.sealed.clone() else {
        return Ok(());
    };
    let Some(source) = sealed.source.clone().or(event.source.clone()) else {
        return Ok(());
    };
    let position = event.position;
    let unsealable = || Error::unavailable(Situation::Unsealable { position });
    let opened = |v: &Value, n: [u8; 12]| -> Option<Value> {
        let b = STANDARD.decode(v.as_str()?).ok()?;
        let plain = cipher.decrypt(Nonce::from_slice(&n), b.as_slice()).ok()?;
        serde_json::from_slice(&plain).ok()
    };
    let at = |f: Option<&str>| place(&source, &event.body, f);
    let each: Option<Vec<_>> = (sealed.fields.iter())
        .map(|f| at(Some(f.as_str())))
        .collect();
    let (Some(whole), Some(each)) = (at(None), each) else {
        return Ok(());
    };
    match (&mut event.body, &sealed.boxed) {
        (EventBody::RecordIngested { patch, .. }, Some(boxed)) => {
            let members = opened(&Value::String(boxed.clone()), whole);
            let Some(Value::Object(members)) = members else {
                return Err(unsealable());
            };
            let named = |f: &FieldName| members.contains_key(f.as_str()) && !patch.contains_key(f);
            if members.len() != sealed.fields.len() || !sealed.fields.iter().all(named) {
                return Err(unsealable());
            }
            patch.extend(members.into_iter().map(|(f, v)| (f.as_str().into(), v)));
        }
        (EventBody::RecordIngested { patch, .. }, None) => {
            for (f, n) in sealed.fields.iter().zip(each) {
                if let Some(v) = patch.get_mut(f) {
                    *v = opened(v, n).ok_or_else(unsealable)?;
                }
            }
        }
        (
            EventBody::OutputRecorded {
                output: Some(v), ..
            },
            _,
        ) => *v = opened(v, whole).ok_or_else(unsealable)?,
        _ => {}
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
