//! The file keyring: the key store Lakelands ships, for one machine and
//! for tests (`docs/decisions/058-a-source-may-be-forgotten.md`). A host's
//! own KMS plugs in through the same trait, [`KeyStore`].
//!
//! One file per lake, `<lake>.keys.json`, in a directory the operator
//! names and keeps outside every lake directory: a key in the lake could
//! not be destroyed. The file holds each source's keys, a version each, as
//! hex, and whether the source's keys were destroyed. It is written whole,
//! to a file beside it that then takes its name, readable by its owner
//! alone. A destroyed source keeps its entry with no key, so it is never
//! given one again; a destroyed version goes with every older one, and
//! the file keeps those after it. A backup of the directory restores
//! what it held: only the operator can keep backups honest.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lakelands_api::error::{Case, Detail, Doing, Part, Situation};
use lakelands_api::id::KeyId;
use lakelands_api::seam::{DataKey, KeyStore};
use lakelands_api::{Error, LakeId, Result, SourceId};
use ring::rand::SecureRandom as _;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize as _, Zeroizing};

/// A key store over files in one directory.
pub struct FileKeyring {
    dir: PathBuf,
    /// Held across each read and write of a file, so two calls of one
    /// process do not interleave.
    lock: Mutex<()>,
}

/// One lake's file.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ring {
    sources: BTreeMap<SourceId, Versions>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    /// Each version's id and key, oldest first; empty once destroyed.
    keys: Vec<(KeyId, String)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    destroyed: bool,
}

/// A file's keys are wiped from memory once read or written.
impl Drop for Versions {
    fn drop(&mut self) {
        for (_, key) in &mut self.keys {
            key.zeroize();
        }
    }
}

impl FileKeyring {
    /// The keyring of the files in `dir`, made at the first key.
    pub fn new(dir: impl Into<PathBuf>) -> FileKeyring {
        let (dir, lock) = (dir.into(), Mutex::new(()));
        FileKeyring { dir, lock }
    }

    fn path(&self, lake: &LakeId) -> PathBuf {
        self.dir.join(format!("{lake}.keys.json"))
    }

    fn read(&self, path: &Path) -> Result<Ring> {
        match fs::read(path) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                let ring = serde_json::from_slice(&bytes);
                ring.map_err(|e| failed(Doing::Read, Detail::json(&e)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ring::default()),
            Err(e) => Err(failed(Doing::Read, Detail::io(&e))),
        }
    }

    fn write(&self, path: &Path, ring: &Ring) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(ring);
        let bytes = Zeroizing::new(bytes.map_err(|e| failed(Doing::Write, Detail::json(&e)))?);
        let next = path.with_extension("json.next");
        let written = (|| {
            // The directory it makes, and every file it writes, are its
            // owner's alone.
            let mut dir = fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(dir.recursive(true), 0o700);
            dir.recursive(true).create(&self.dir)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&next)?;
            #[cfg(unix)]
            file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&next, path)?;
            fs::File::open(&self.dir)?.sync_all()
        })();
        written.map_err(|e| failed(Doing::Write, Detail::io(&e)))
    }

    /// Change one lake's file under the lock and write it back.
    fn change<T>(&self, lake: &LakeId, f: impl FnOnce(&mut Ring) -> Result<T>) -> Result<T> {
        let _held = lakelands_core::lock(&self.lock);
        let path = self.path(lake);
        let mut ring = self.read(&path)?;
        let out = f(&mut ring)?;
        self.write(&path, &ring)?;
        Ok(out)
    }

    fn versions(&self, lake: &LakeId, source: &SourceId) -> Result<Option<Versions>> {
        let _held = lakelands_core::lock(&self.lock);
        Ok(self.read(&self.path(lake))?.sources.remove(source))
    }
}

/// The keyring's file could not be read or written.
fn failed(doing: Doing, detail: Detail) -> Error {
    let part = Part::File;
    Error::unavailable(Situation::Storage {
        doing,
        part,
        detail,
    })
}

impl KeyStore for FileKeyring {
    fn create(&self, lake: &LakeId, source: &SourceId) -> Result<KeyId> {
        // A version's id is random too, so a keyring that lost its file
        // never gives a new key the id of one it lost.
        let (mut key, mut id) = (Zeroizing::new([0u8; 32]), [0u8; 16]);
        let random = ring::rand::SystemRandom::new();
        let filled = random.fill(&mut *key).and_then(|()| random.fill(&mut id));
        filled.map_err(|_| failed(Doing::Create, Detail::io(&std::io::ErrorKind::Other.into())))?;
        let hex = lakelands_api::canonical::hex(&*key);
        let id = KeyId::from(lakelands_api::canonical::hex(&id));
        self.change(lake, |ring| {
            let versions = ring.sources.entry(source.clone()).or_default();
            if versions.destroyed {
                let source = source.clone();
                return Err(lakelands_core::invalid(Case::KeyDestroyed { source }));
            }
            versions.keys.push((id.clone(), hex));
            Ok(id)
        })
    }

    fn key(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<Option<DataKey>> {
        let versions = self.versions(lake, source)?.unwrap_or_default();
        let Some((_, hex)) = versions.keys.iter().find(|(i, _)| i == id) else {
            return Ok(None);
        };
        let bytes: Option<Vec<u8>> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect();
        let bytes = bytes.map(Zeroizing::new);
        match bytes.and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()) {
            Some(key) => Ok(Some(DataKey(key))),
            None => {
                let not_a_key = std::io::ErrorKind::InvalidData.into();
                Err(failed(Doing::Read, Detail::io(&not_a_key)))
            }
        }
    }

    fn destroy(&self, lake: &LakeId, source: &SourceId) -> Result<()> {
        self.change(lake, |ring| {
            let versions = ring.sources.entry(source.clone()).or_default();
            versions.keys.clear();
            versions.destroyed = true;
            Ok(())
        })
    }

    fn destroy_version(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<()> {
        self.change(lake, |ring| {
            let keys = ring.sources.get_mut(source).map(|v| &mut v.keys);
            let at = keys
                .as_ref()
                .and_then(|k| k.iter().position(|(i, _)| i == id));
            let (Some(keys), Some(i)) = (keys, at) else {
                let (source, key) = (source.clone(), id.clone());
                return Err(lakelands_core::invalid(Case::KeyVersionNotHeld {
                    source,
                    key,
                }));
            };
            keys.drain(..=i).for_each(|(_, mut key)| key.zeroize());
            Ok(())
        })
    }

    fn destroys_versions(&self) -> bool {
        true
    }

    fn ids(&self, lake: &LakeId, source: &SourceId) -> Result<Vec<KeyId>> {
        let versions = self.versions(lake, source)?.unwrap_or_default();
        Ok(versions.keys.iter().map(|(id, _)| id.clone()).collect())
    }
}

#[cfg(test)]
mod tests;
