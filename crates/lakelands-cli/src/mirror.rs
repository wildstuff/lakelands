//! The mirror: `serve` follows one lake's change feed and ingests the
//! types a connection maps into another lake of the same root. Water
//! crosses between lakes this way only; there is no query across lakes
//! and no reference into another one.
//!
//! **What crosses.** A connection maps source types to target types, the
//! same name unless it says otherwise. The target type's declaration is
//! the list of what crosses: every field it declares, and the links of
//! every relation carried by link records it declares. A source field the
//! target does not declare stays home, and so does everything the
//! connection's principal may not read in the source, or that the source
//! marks `sensitive`. The feed is the log unmasked, so values are never
//! taken from it: it says which record changed at which position, and the
//! mirror reads that record `as_of` the position through `get`, as the
//! principal, so the source's grants decide what it sees. A record the
//! principal cannot see, withheld or not granted, does not cross, and its
//! copy keeps its last state.
//!
//! **How it lands.** Each mapped source type is one source of the target,
//! `mirror:<source lake>:<source type>`, written through `ingest` with
//! the source's `known_at` as `seen_at`. A copy's fields are source fields
//! nobody in the target can author, and `purge_source` of that source
//! removes the copies whole. A window carries only the fields that differ
//! from the copy, so a reference the source kept to a record it retired
//! is not carried again. Consecutive changes of one type are read once per
//! key, at the key's last position among them, and ordered so that a
//! record lands after the records it refers to. The target's history
//! therefore steps once per window, not once per source event: in return,
//! a window delivered again reads what the copies already hold, and
//! appends nothing.
//!
//! **What is not ingest.** A retirement crosses through `retire` (a link's
//! through `unlink`), with an idempotency key naming the source position,
//! so a retirement delivered again changes nothing. A later write to the
//! same source key brings the copy back. A retention hold crosses through
//! the host's `restrict` and `unrestrict` only when the principal holds
//! the `restricted` grant; without it the hold is invisible to the mirror,
//! and so is the record. Each of these keeps its place in feed order.
//!
//! **It fails closed, by type.** At every declaration or migration of a
//! mapped source type, the mirror asks whether the target's declaration
//! still admits the source's at that position: the same key rule, and
//! every field the target declares present in the source, of the same
//! type, readable and not `sensitive`. If not, that type stops. So does a
//! type whose window the target refuses, and a type a re-key moved while
//! the target keeps the old key rule. A stopped type keeps its position
//! and the reason, said on standard error and in the position file; the
//! connection's other types go on, and a restart tries it again from
//! where it stopped. A new source field crosses once the target declares
//! it, for the records that change after; removing the type from the
//! position file copies it again (`snapshot`), and every unchanged field
//! appends nothing.
//!
//! **What `serve` refuses to start.** A connection whose target lake this
//! process holds but not as its writer; two sources for one target type;
//! a type mirrored into a lake and back out of it; a `sys.` type; a target
//! type with a field its source does not write (authority other than
//! `source`), a field the principal may not write, a reference to a type
//! the connection does not mirror, or, for a new connection, a field the
//! source does not admit. A second process mirroring the same connection
//! is refused by the position file's lock.
//!
//! **Where it starts and where it is.** A new connection copies what the
//! source holds at its head (`snapshot`, the default) or only what changes
//! after it (`head`); it never replays history, which would walk every
//! copy through every past state and fire its triggers on the way. Its
//! position lives under `<root>/mirrors/<target lake>/`: `<name>.json`,
//! per source type the next position to read and why it stopped, if it
//! did; `<name>.lock`; and `<name>.ndjson`, a journal line per window and
//! per action. The position file is interim: the target lake's own record
//! of a connection replaces it. Delivery is at least once, in order: the
//! position is written after each window or action is durable.
//!
//! The source is a lake of the same root; a source served elsewhere is
//! not offered.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::describe::TypeDescription;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::{Authority, FieldType};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Error, FieldName, Find, Found, Get, IdempotencyKey, LakeStore, Patch, Position, Principal,
    ReadAt, Record, RecordKey, RelationName, SourceId, SourceRecord, Timestamp, TypeName, Value,
    WriteContext, WriteOpts, ingest_all,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The event kinds a mirror reads.
pub const KINDS: [&str; 7] = [
    "record_upserted",
    "record_ingested",
    "record_retired",
    "record_restricted",
    "record_unrestricted",
    "declared",
    "migrated",
];

/// The prefix of a link type's name.
const LINK: &str = "sys.link.";

/// The `mirror` section of a root's `lakelands.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorConfig {
    /// The connections.
    pub connections: Vec<ConnectionConfig>,
    /// How long a mirror that has caught up waits before it polls again,
    /// in milliseconds.
    #[serde(default = "default_poll_millis")]
    pub poll_millis: u64,
    /// The most events read from the feed at once, and the most records
    /// read per page of a snapshot.
    #[serde(default = "default_batch")]
    pub batch: u32,
}

/// One connection: a source lake, a target lake, the types that cross.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Its name, which its files and messages carry.
    pub name: String,
    /// The lake read.
    pub source: String,
    /// The lake written.
    pub target: String,
    /// The root's principal the source is read as and the target written
    /// as.
    pub principal: String,
    /// Source type to target type.
    pub types: BTreeMap<String, String>,
    /// Where a type with no position begins.
    #[serde(default)]
    pub start: Start,
}

/// Where a type with no position begins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Start {
    /// Copy what the source holds at its head, then follow its feed.
    #[default]
    Snapshot,
    /// Follow the feed from the head, copying nothing already there.
    Head,
}

fn default_poll_millis() -> u64 {
    1000
}

fn default_batch() -> u32 {
    100
}

fn usable(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl MirrorConfig {
    /// Refuse what the section alone shows to be wrong: a number that is
    /// zero, a name used twice or not usable as a file name, a lake
    /// mirrored into itself, a connection with no type, a `sys.` type, two
    /// sources for one target type, and a type mirrored into a lake and
    /// back out of it. A host checks this when it loads the configuration.
    pub fn check(&self) -> Result<(), String> {
        if self.poll_millis == 0 {
            return Err("the mirror section's `poll_millis` is zero".into());
        }
        if self.batch == 0 {
            return Err("the mirror section's `batch` is zero".into());
        }
        let mut names = BTreeSet::new();
        // Each target type's one source: (target lake, type) → (connection,
        // source lake, source type).
        let mut homes: BTreeMap<(&str, &str), (&str, &str, &str)> = BTreeMap::new();
        for c in &self.connections {
            let name = c.name.as_str();
            if !usable(name) {
                return Err(format!(
                    "the connection name `{name}` is not letters, digits, `-` and `_`"
                ));
            }
            if !names.insert(name) {
                return Err(format!("the connection name `{name}` is used twice"));
            }
            if c.source == c.target {
                return Err(format!(
                    "connection `{name}` mirrors lake `{}` into itself",
                    c.source
                ));
            }
            if c.types.is_empty() {
                return Err(format!("connection `{name}` maps no type"));
            }
            for (s, t) in &c.types {
                if s.starts_with("sys.") || t.starts_with("sys.") {
                    return Err(format!(
                        "connection `{name}` maps `{s}` to `{t}`: `sys.` types belong to their \
                         lake and are never mirrored"
                    ));
                }
                if let Some((other, lake, ty)) = homes.get(&(c.target.as_str(), t.as_str())) {
                    return Err(format!(
                        "type `{t}` of lake `{}` would have two sources: `{lake}/{ty}` by \
                         connection `{other}` and `{}/{s}` by connection `{name}`; a mirrored \
                         type has one source",
                        c.target, c.source
                    ));
                }
                homes.insert((&c.target, t), (name, &c.source, s));
            }
        }
        for a in &self.connections {
            for (s, t) in &a.types {
                let back = self.connections.iter().find(|b| {
                    b.source == a.target && b.target == a.source && b.types.contains_key(t)
                });
                if let Some(b) = back {
                    return Err(format!(
                        "type `{t}` of lake `{}` is mirrored from `{}/{s}` by connection `{}` and \
                         back into `{}` by connection `{}`; a field has one home lake",
                        a.target, a.source, a.name, a.source, b.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Where one source type stands, as the position file keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeState {
    /// The next source position to read; 0 while the type is still to be
    /// copied whole.
    pub next: u64,
    /// Why the type stopped, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Stopped>,
}

/// Why a type stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopped {
    /// The source position it stopped at.
    pub at: u64,
    /// Why, for the operator.
    pub why: String,
}

/// The position file's form.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// Per source type, link types included.
    pub types: BTreeMap<TypeName, TypeState>,
}

/// One mapped source type.
#[derive(Clone, Debug)]
struct Mapped {
    /// The target type.
    target: TypeName,
    /// The target's fields that cross, and for a reference field the
    /// target type it refers to.
    fields: BTreeMap<FieldName, Option<TypeName>>,
    /// For a link type: the source type that owns the relation, the
    /// target's owner type and the relation.
    link: Option<(TypeName, TypeName, RelationName)>,
}

/// One connection's mirror.
pub struct Mirror {
    name: String,
    source: Arc<dyn LakeStore>,
    target: Arc<dyn LakeStore>,
    reader: Principal,
    writer: Principal,
    map: BTreeMap<TypeName, Mapped>,
    kept: Kept,
    start: Start,
    batch: u32,
    poll: Duration,
    position_file: PathBuf,
    journal: File,
    // Held for the mirror's life: a second mirror of the same connection
    // is refused at start.
    _lock: File,
}

impl std::fmt::Debug for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mirror")
            .field("connection", &self.name)
            .field("source", self.source.lake())
            .field("target", self.target.lake())
            .finish()
    }
}

/// A run of consecutive changes to one source type: each change's key,
/// position and `known_at`, in feed order.
#[derive(Default)]
struct Run {
    ty: Option<TypeName>,
    first: Position,
    last: Position,
    changes: Vec<(RecordKey, Position, Timestamp)>,
}

impl Run {
    fn push(&mut self, ty: &TypeName, key: &RecordKey, e: &Event) {
        if self.ty.is_none() {
            self.ty = Some(ty.clone());
            self.first = e.position;
        }
        self.last = e.position;
        self.changes.push((key.clone(), e.position, e.known_at));
    }

    /// One entry per key, at the key's last position, in the order keys
    /// first appeared: what a window delivered again reads is what the
    /// copies already hold.
    fn coalesced(self) -> Vec<(RecordKey, Position, Timestamp)> {
        let mut last: HashMap<RecordKey, (Position, Timestamp)> = HashMap::new();
        for (key, p, t) in &self.changes {
            last.insert(key.clone(), (*p, *t));
        }
        self.changes
            .into_iter()
            .filter_map(|(key, _, _)| {
                let (p, t) = last.remove(&key)?;
                Some((key, p, t))
            })
            .collect()
    }
}

fn type_of(body: &EventBody) -> Option<&TypeName> {
    match body {
        EventBody::RecordUpserted { ty, .. }
        | EventBody::RecordIngested { ty, .. }
        | EventBody::RecordRetired { ty, .. }
        | EventBody::RecordRestricted { ty, .. }
        | EventBody::RecordUnrestricted { ty, .. } => Some(ty),
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => Some(&t.name),
        EventBody::Migrated { migration, .. } => Some(&migration.to.name),
        _ => None,
    }
}

fn link_name(owner: &TypeName, relation: &RelationName) -> TypeName {
    TypeName::from(format!("{LINK}{owner}.{relation}"))
}

fn at(p: Position) -> ReadAt {
    ReadAt {
        as_of: Some(p),
        valid_at: None,
    }
}

fn now() -> Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros());
    Timestamp(i64::try_from(micros).unwrap_or(i64::MAX))
}

/// The field type a source field has once its reference target is named
/// by the target lake's type for it.
fn remapped(ty: &FieldType, map: &BTreeMap<TypeName, TypeName>) -> Option<FieldType> {
    let mut ty = ty.clone();
    if let Some(t) = &ty.target {
        ty.target = Some(map.get(t)?.clone());
    }
    Some(ty)
}

/// What the target type `t` must be for the connection, seen from the
/// target alone: every field written by the source, writable by the
/// principal, referring only to mirrored types; every relation carried by
/// link records a source relation between mirrored types.
fn owned_whole(t: &TypeDescription, mirrored: &BTreeSet<TypeName>) -> Result<(), String> {
    let name = &t.name;
    if t.view.is_some() || t.lineage.view_of.is_some() {
        return Err(format!("`{name}` is a view, which nothing writes"));
    }
    for f in &t.fields {
        if f.authority != Authority::Source {
            return Err(format!(
                "`{name}.{}` is written by the authority `{}`; a mirrored type declares only \
                 fields its source writes (`source`)",
                f.name,
                serde_json::to_value(f.authority)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            ));
        }
        if !f.access.writes() {
            return Err(format!("the principal may not write `{name}.{}`", f.name));
        }
        if let Some(target) = &f.ty.target
            && !mirrored.contains(target)
        {
            return Err(format!(
                "`{name}.{}` refers to `{target}`, which the connection does not mirror",
                f.name
            ));
        }
    }
    for r in t.relations.iter().filter(|r| r.via.is_none()) {
        if r.authority != Authority::Source {
            return Err(format!(
                "the links of `{name}.{}` are authored; a mirrored type's relations are \
                 written by its source",
                r.name
            ));
        }
        if !mirrored.contains(&r.target) {
            return Err(format!(
                "the relation `{name}.{}` leads to `{}`, which the connection does not mirror",
                r.name, r.target
            ));
        }
    }
    Ok(())
}

/// Whether the target type `t` admits the source type `s` as the principal
/// reads it: the same key rule, and every target field (and link field)
/// present in the source with the same type, readable, and not
/// `sensitive`. `map` names each source type's target type.
fn admits(
    s: &TypeDescription,
    t: &TypeDescription,
    map: &BTreeMap<TypeName, TypeName>,
) -> Result<(), String> {
    if s.access.key != t.access.key {
        return Err(format!(
            "`{}` is keyed by {:?} in the source and `{}` by {:?} here",
            s.name, s.access.key, t.name, t.access.key
        ));
    }
    let check = |sname: &str,
                 tname: &str,
                 sf: &[&lakelands_api::describe::FieldDescription],
                 tf: &[&lakelands_api::describe::FieldDescription]|
     -> Result<(), String> {
        for f in tf {
            let Some(g) = sf.iter().find(|g| g.name == f.name) else {
                return Err(format!(
                    "`{sname}` has no field `{}` the principal may read; `{tname}` declares it",
                    f.name
                ));
            };
            if !g.access.reads() {
                return Err(format!("the principal may not read `{sname}.{}`", g.name));
            }
            if g.sensitive {
                return Err(format!(
                    "`{sname}.{}` is sensitive and stays in its lake",
                    g.name
                ));
            }
            if remapped(&g.ty, map).as_ref() != Some(&f.ty) {
                return Err(format!(
                    "`{sname}.{}` has another type in the source than `{tname}.{}` here",
                    g.name, f.name
                ));
            }
            if f.required && !g.required {
                return Err(format!(
                    "`{tname}.{}` is required here and optional in the source",
                    f.name
                ));
            }
        }
        Ok(())
    };
    let sf: Vec<_> = s.fields.iter().collect();
    let tf: Vec<_> = t.fields.iter().collect();
    check(s.name.as_str(), t.name.as_str(), &sf, &tf)?;
    for r in t.relations.iter().filter(|r| r.via.is_none()) {
        let Some(q) = s
            .relations
            .iter()
            .find(|q| q.name == r.name && q.via.is_none())
        else {
            return Err(format!(
                "`{}` has no relation `{}` carried by links; `{}` declares it",
                s.name, r.name, t.name
            ));
        };
        if map.get(&q.target) != Some(&r.target) {
            return Err(format!(
                "the relation `{}.{}` leads to `{}` in the source, which is not mirrored as `{}`",
                s.name, q.name, q.target, r.target
            ));
        }
        for f in &r.fields {
            let Some(g) = q.fields.iter().find(|g| g.name == f.name) else {
                return Err(format!(
                    "the links of `{}.{}` have no field `{}`",
                    s.name, q.name, f.name
                ));
            };
            if g.sensitive {
                return Err(format!(
                    "the link field `{}.{}.{}` is sensitive and stays in its lake",
                    s.name, q.name, g.name
                ));
            }
            if remapped(&g.ty, map).as_ref() != Some(&f.ty) || (f.required && !g.required) {
                return Err(format!(
                    "the link field `{}.{}.{}` differs between the source and here",
                    s.name, q.name, g.name
                ));
            }
        }
    }
    Ok(())
}

/// The patch that makes `copy` hold `source`'s values of `fields`: every
/// field that differs, `null` for one the source has cleared. With no copy,
/// every field that holds a value.
fn diff<'a>(
    fields: impl Iterator<Item = &'a FieldName>,
    source: &BTreeMap<FieldName, Value>,
    copy: Option<&Record>,
) -> Patch {
    let mut patch = Patch::new();
    for f in fields {
        let now = source.get(f).cloned().unwrap_or(Value::Null);
        match copy {
            None if now.is_null() => {}
            None => {
                patch.insert(f.clone(), now);
            }
            Some(c) => {
                let was = c.fields.get(f).cloned().unwrap_or(Value::Null);
                if was != now {
                    patch.insert(f.clone(), now);
                }
            }
        }
    }
    patch
}

/// `records` in an order in which each lands after the records among
/// them it refers to, otherwise in the order given. Records that refer to
/// each other in a circle keep their order, and the target refuses the
/// first reference it cannot resolve.
fn ordered(
    records: Vec<(TypeName, SourceRecord)>,
    refs: impl Fn(&TypeName, &SourceRecord) -> Vec<(TypeName, RecordKey)>,
) -> Vec<(TypeName, SourceRecord)> {
    let n = records.len();
    let index: HashMap<(TypeName, RecordKey), usize> = records
        .iter()
        .enumerate()
        .map(|(i, (_, r))| ((r.ty.clone(), r.key.clone()), i))
        .collect();
    let mut waits = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, (s, r)) in records.iter().enumerate() {
        for d in refs(s, r) {
            if let Some(&j) = index.get(&d)
                && j != i
            {
                waits[i] += 1;
                dependents[j].push(i);
            }
        }
    }
    let mut ready: VecDeque<usize> = (0..n).filter(|&i| waits[i] == 0).collect();
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while let Some(i) = ready.pop_front() {
        placed[i] = true;
        order.push(i);
        for &k in &dependents[i] {
            waits[k] -= 1;
            if waits[k] == 0 {
                ready.push_back(k);
            }
        }
    }
    order.extend((0..n).filter(|&i| !placed[i]));
    let mut slots: Vec<Option<(TypeName, SourceRecord)>> = records.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

impl Mirror {
    /// Set up one connection between two open lakes: `reader` is the
    /// connection's principal in the source, `writer` in the target. Its
    /// files live under `dir/mirrors/<target lake>/`. Refused, with the
    /// reason, when the target is not held as its writer, another process
    /// mirrors the connection, a target type is not the source's whole
    /// (see [`MirrorConfig::check`] for what the section alone shows), or,
    /// for a type with no position yet, the target does not admit the
    /// source's declaration. A type with a position that the target no
    /// longer admits stops, and the rest goes on.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: &MirrorConfig,
        connection: &ConnectionConfig,
        dir: &Path,
        source: Arc<dyn LakeStore>,
        target: Arc<dyn LakeStore>,
        reader: Principal,
        writer: Principal,
    ) -> Result<Mirror, String> {
        let name = &connection.name;
        let (from, to) = (source.lake().clone(), target.lake().clone());
        if !target.is_writer() {
            return Err(format!(
                "connection `{name}`: lake `{to}` is not held by this process as its writer (it \
                 was opened read-only, or another process holds its writer lease); a mirror \
                 writes through the target's one writer"
            ));
        }
        if !writer.grants.ingest {
            return Err(format!(
                "connection `{name}`: the principal may not ingest into lake `{to}`"
            ));
        }
        let home = dir.join("mirrors").join(to.as_str());
        std::fs::create_dir_all(&home)
            .map_err(|err| format!("{} cannot be made: {err}", home.display()))?;
        let lock_file = home.join(format!("{name}.lock"));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_file)
            .map_err(|err| format!("{} cannot be opened: {err}", lock_file.display()))?;
        // Called through the trait: the standard library's own `try_lock`
        // is newer than the workspace's minimum Rust version.
        match fs4::FileExt::try_lock(&lock) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => {
                return Err(format!(
                    "connection `{name}` into lake `{to}` is mirrored by another process ({} is \
                     locked)",
                    lock_file.display()
                ));
            }
            Err(fs4::TryLockError::Error(err)) => {
                return Err(format!("{} cannot be locked: {err}", lock_file.display()));
            }
        }
        let position_file = home.join(format!("{name}.json"));
        let kept = match std::fs::read(&position_file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("{} is not a position file: {e}", position_file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Kept::default(),
            Err(e) => return Err(format!("{} cannot be read: {e}", position_file.display())),
        };
        let journal_file = home.join(format!("{name}.ndjson"));
        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_file)
            .map_err(|err| format!("{} cannot be opened: {err}", journal_file.display()))?;
        let names: BTreeMap<TypeName, TypeName> = connection
            .types
            .iter()
            .map(|(s, t)| (TypeName::from(s.as_str()), TypeName::from(t.as_str())))
            .collect();
        let mirrored: BTreeSet<TypeName> = names.values().cloned().collect();
        let mut mirror = Mirror {
            name: name.clone(),
            source,
            target,
            reader,
            writer,
            map: BTreeMap::new(),
            kept,
            start: connection.start,
            batch: config.batch,
            poll: Duration::from_millis(config.poll_millis),
            position_file,
            journal,
            _lock: lock,
        };
        let refused = |why: String| format!("connection `{name}` from `{from}` into `{to}`: {why}");
        let tcat = mirror.target.catalog(WriteContext::default());
        let mut descriptions = BTreeMap::new();
        for (s, t) in &names {
            let td = tcat
                .describe_type(&mirror.writer, t, ReadAt::default())
                .await
                .map_err(|e| refused(format!("the target type `{t}`: {e}")))?
                .data;
            owned_whole(&td, &mirrored).map_err(refused)?;
            let fields: BTreeMap<FieldName, Option<TypeName>> = td
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.ty.target.clone()))
                .collect();
            mirror.map.insert(
                s.clone(),
                Mapped {
                    target: t.clone(),
                    fields,
                    link: None,
                },
            );
            for r in td.relations.iter().filter(|r| r.via.is_none()) {
                let mut fields: BTreeMap<FieldName, Option<TypeName>> = BTreeMap::new();
                fields.insert(FieldName::from(lakelands_api::key::FROM), Some(t.clone()));
                fields.insert(
                    FieldName::from(lakelands_api::key::TO),
                    Some(r.target.clone()),
                );
                for f in &r.fields {
                    fields.insert(f.name.clone(), f.ty.target.clone());
                }
                mirror.map.insert(
                    link_name(s, &r.name),
                    Mapped {
                        target: link_name(t, &r.name),
                        fields,
                        link: Some((s.clone(), t.clone(), r.name.clone())),
                    },
                );
            }
            descriptions.insert(s.clone(), td);
        }
        // A type the file does not name is new: the target must admit the
        // source at its head now. One it names resumes, stopped or not, and
        // is asked again at its own position.
        for (s, td) in &descriptions {
            match mirror.kept.types.get(s).cloned() {
                None => {
                    let sd = mirror.describe_source(s, None).await.map_err(&refused)?;
                    admits(&sd, td, &names).map_err(&refused)?;
                }
                Some(state) => {
                    let resume = state.stopped.as_ref().map_or(state.next, |s| s.at);
                    if resume == 0 {
                        // Stopped before its copy was made: copied again.
                        for ty in mirror.owned(s) {
                            mirror.kept.types.remove(&ty);
                        }
                        continue;
                    }
                    for ty in mirror.owned(s) {
                        if let Some(st) = mirror.kept.types.get_mut(&ty)
                            && let Some(stopped) = st.stopped.take()
                        {
                            st.next = stopped.at;
                        }
                    }
                    // A stopped type is asked at the change that stopped it,
                    // a running one at the last position it brought across.
                    let asked = match &state.stopped {
                        Some(_) => Position(resume),
                        None => Position(resume.saturating_sub(1)),
                    };
                    let why = match mirror.describe_source(s, Some(asked)).await {
                        Ok(sd) => admits(&sd, td, &names).err(),
                        Err(why) => Some(why),
                    };
                    if let Some(why) = why {
                        mirror.stop(&mirror.owned(s), Position(resume), why)?;
                    }
                }
            }
        }
        mirror.save()?;
        Ok(mirror)
    }

    /// The connection's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where each source type stands, as the position file keeps it.
    pub fn kept(&self) -> &Kept {
        &self.kept
    }

    /// Mirror until the process ends: poll, ingest what came, and wait
    /// `poll_millis` once caught up. A round that fails is tried again
    /// after the same wait, from the position kept.
    pub async fn run(mut self) {
        loop {
            match self.round().await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(self.poll).await,
                Err(why) => {
                    self.say(&why);
                    tokio::time::sleep(self.poll).await;
                }
            }
        }
    }

    /// One round: copy the types still to be copied, or read the feed from
    /// the lowest position of the running types and bring each change
    /// across, in order. Answers whether more may be waiting.
    pub async fn round(&mut self) -> Result<bool, String> {
        let unsettled: Vec<TypeName> = self
            .map
            .keys()
            .filter(|t| !self.kept.types.contains_key(*t))
            .cloned()
            .collect();
        if !unsettled.is_empty() {
            self.snapshot(unsettled).await?;
            return Ok(true);
        }
        let running: Vec<TypeName> = self
            .kept
            .types
            .iter()
            .filter(|(t, s)| s.stopped.is_none() && self.map.contains_key(*t))
            .map(|(t, _)| t.clone())
            .collect();
        let Some(from) = running.iter().map(|t| self.kept.types[t].next).min() else {
            return Ok(false);
        };
        let filter = FeedFilter {
            kinds: KINDS.iter().map(|k| (*k).to_owned()).collect(),
            types: running,
            keys: Vec::new(),
        };
        let feed = self.source.feed();
        let batch = feed
            .poll(Position(from), &filter, self.batch)
            .await
            .map_err(|e| format!("the source's feed failed: {e}"))?;
        let full = batch.events.len() >= self.batch as usize;
        let mut run = Run::default();
        for e in &batch.events {
            let Some(ty) = type_of(&e.body) else {
                continue;
            };
            let due = self
                .kept
                .types
                .get(ty)
                .is_some_and(|s| s.stopped.is_none() && e.position.0 >= s.next);
            if !due {
                continue;
            }
            match &e.body {
                EventBody::RecordUpserted { key, .. } | EventBody::RecordIngested { key, .. } => {
                    if run.ty.as_ref().is_some_and(|t| t != ty) {
                        self.flush(&mut run).await?;
                    }
                    run.push(ty, key, e);
                }
                _ => {
                    self.flush(&mut run).await?;
                    self.act(e).await?;
                    self.advance(e.position.next())?;
                }
            }
        }
        self.flush(&mut run).await?;
        if batch.next.0 > from {
            self.advance(batch.next)?;
        }
        // The store's own feed keeps no consumer state; a relay may.
        let _ = feed.ack(&format!("mirror:{}", self.name), batch.next).await;
        Ok(full)
    }

    /// Copy the named types whole, as the source holds them at its head,
    /// and follow each from the position after.
    async fn snapshot(&mut self, types: Vec<TypeName>) -> Result<(), String> {
        let head = self.head().await?;
        if self.start == Start::Head {
            for t in types {
                self.kept.types.insert(
                    t,
                    TypeState {
                        next: head.next().0,
                        stopped: None,
                    },
                );
            }
            return self.save();
        }
        let scat = self.source.catalog(WriteContext::default());
        let mut records = Vec::new();
        for s in &types {
            let mapped = self.map[s].clone();
            let mut cursor = None;
            loop {
                let reply = scat
                    .find(
                        &self.reader,
                        s,
                        Find {
                            limit: Some(self.batch),
                            cursor: cursor.take(),
                            with_meta: true,
                            at: at(head),
                            ..Find::default()
                        },
                    )
                    .await
                    .map_err(|e| format!("`{s}` cannot be read from the source: {e}"))?;
                let Found::Records { rows, next_cursor } = reply.data else {
                    break;
                };
                for row in rows {
                    let copy = self.copy(&mapped.target, &row.key).await?;
                    let patch = diff(mapped.fields.keys(), &row.fields, copy.as_ref());
                    if patch.is_empty() {
                        continue;
                    }
                    let seen_at = row.meta.values().map(|m| m.known_at).max();
                    records.push((
                        s.clone(),
                        SourceRecord {
                            ty: mapped.target.clone(),
                            key: row.key,
                            fields: patch,
                            seen_at,
                        },
                    ));
                }
                match next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
        }
        let mut failed = BTreeMap::new();
        for (s, window) in self.windows(records) {
            if failed.contains_key(&s) {
                continue;
            }
            if let Some(why) = self.ingest(&s, window).await? {
                failed.insert(s, why);
            }
        }
        for t in types {
            let state = match failed.get(&t) {
                // Copied again at the next start.
                Some(why) => TypeState {
                    next: 0,
                    stopped: Some(Stopped {
                        at: 0,
                        why: why.clone(),
                    }),
                },
                None => TypeState {
                    next: head.next().0,
                    stopped: None,
                },
            };
            if let Some(stopped) = &state.stopped {
                self.say(&format!("`{t}` stopped while copied: {}", stopped.why));
                self.journal(json!({ "stopped": t, "at": 0, "why": stopped.why }));
            }
            self.kept.types.insert(t, state);
        }
        self.journal(json!({ "snapshot": head.0 }));
        self.save()
    }

    /// Records in the order they must land, cut into one window per run
    /// of one source type.
    fn windows(
        &self,
        records: Vec<(TypeName, SourceRecord)>,
    ) -> Vec<(TypeName, Vec<SourceRecord>)> {
        let map = &self.map;
        let ordered = ordered(records, |s, r| {
            let Some(m) = map.get(s) else {
                return Vec::new();
            };
            m.fields
                .iter()
                .filter_map(|(f, target)| {
                    let target = target.as_ref()?;
                    let key = r.fields.get(f)?.as_str()?;
                    Some((target.clone(), RecordKey::from(key)))
                })
                .collect()
        });
        let mut out: Vec<(TypeName, Vec<SourceRecord>)> = Vec::new();
        for (s, r) in ordered {
            match out.last_mut() {
                Some((t, w)) if *t == s => w.push(r),
                _ => out.push((s, vec![r])),
            }
        }
        out
    }

    /// Bring one run across: each key read once, at its last position.
    async fn flush(&mut self, run: &mut Run) -> Result<(), String> {
        let Some(s) = run.ty.take() else {
            return Ok(());
        };
        let run = std::mem::take(run);
        let (first, last) = (run.first, run.last);
        let mapped = self.map[&s].clone();
        let mut records = Vec::new();
        for (key, position, known_at) in run.coalesced() {
            let key = &key;
            let Some(record) = self.read(&s, key, position).await? else {
                continue;
            };
            let copy = self.copy(&mapped.target, key).await?;
            let patch = diff(mapped.fields.keys(), &record.fields, copy.as_ref());
            if patch.is_empty() {
                continue;
            }
            records.push((
                s.clone(),
                SourceRecord {
                    ty: mapped.target.clone(),
                    key: key.clone(),
                    fields: patch,
                    seen_at: Some(known_at),
                },
            ));
        }
        for (s, window) in self.windows(records) {
            if let Some(why) = self.ingest(&s, window).await? {
                return self.stop(&[s], first, why);
            }
        }
        self.advance(last.next())
    }

    /// Ingest one window of one source type. A refusal is answered as the
    /// reason the type stops; any other failure ends the round.
    async fn ingest(
        &mut self,
        s: &TypeName,
        window: Vec<SourceRecord>,
    ) -> Result<Option<String>, String> {
        let source = SourceId::from(format!("mirror:{}:{s}", self.source.lake()));
        let tcat = self.target.catalog(WriteContext::default());
        let keys = window.len();
        match ingest_all(tcat.as_ref(), &self.writer, &source, window).await {
            Ok(commits) => {
                let records: u32 = commits.iter().map(|c| c.records).sum();
                let duplicates: u32 = commits.iter().map(|c| c.duplicates).sum();
                let through = commits.last().map(|c| c.position.0);
                self.journal(json!({
                    "type": s, "keys": keys, "records": records,
                    "duplicates": duplicates, "through": through,
                }));
                Ok(None)
            }
            Err(e @ Error::Refused { .. }) => Ok(Some(format!("the target refused a window: {e}"))),
            Err(e) => Err(format!("the target failed: {e}")),
        }
    }

    /// Everything that is not a record's new state: a retirement, a hold,
    /// a declaration.
    async fn act(&mut self, e: &Event) -> Result<(), String> {
        match &e.body {
            EventBody::RecordRetired { ty, key, .. } => self.retire(ty, key, e.position).await,
            EventBody::RecordRestricted {
                ty, key, reason, ..
            } => self.hold(ty, key, Some(reason), e.position).await,
            EventBody::RecordUnrestricted { ty, key, .. } => {
                self.hold(ty, key, None, e.position).await
            }
            EventBody::Declared { .. } | EventBody::Migrated { .. } => {
                let Some(s) = type_of(&e.body) else {
                    return Ok(());
                };
                self.redeclared(s, e.position).await
            }
            _ => Ok(()),
        }
    }

    async fn retire(&mut self, s: &TypeName, key: &RecordKey, p: Position) -> Result<(), String> {
        let mapped = self.map[s].clone();
        let tcat = self.target.catalog(WriteContext::default());
        let opts = WriteOpts {
            idempotency_key: Some(IdempotencyKey::from(format!(
                "mirror:{}:{}",
                self.source.lake(),
                p
            ))),
            ..WriteOpts::default()
        };
        let done = match &mapped.link {
            None => tcat.retire(&self.writer, &mapped.target, key, opts).await,
            Some((_, owner, relation)) => {
                let parts = lakelands_api::key::key_components(key);
                let [from, to] = parts.as_slice() else {
                    return Ok(());
                };
                tcat.unlink(
                    &self.writer,
                    owner,
                    &RecordKey::from(from.as_str()),
                    relation,
                    &RecordKey::from(to.as_str()),
                    opts,
                )
                .await
            }
        };
        match done {
            Ok(_) => {
                self.journal(json!({ "retired": mapped.target, "key": key, "at": p.0 }));
                Ok(())
            }
            // No copy to retire: the record never crossed.
            Err(Error::NotFound { .. }) => Ok(()),
            Err(e @ Error::Refused { .. }) => self.stop(
                std::slice::from_ref(s),
                p,
                format!(
                    "the target refused to retire `{}` `{key}`: {e}",
                    mapped.target
                ),
            ),
            Err(e) => Err(format!("the target failed: {e}")),
        }
    }

    /// Place (`reason`) or lift (`None`) a hold on the copy, when the
    /// principal may see held records at all.
    async fn hold(
        &mut self,
        s: &TypeName,
        key: &RecordKey,
        reason: Option<&String>,
        p: Position,
    ) -> Result<(), String> {
        if !self.reader.grants.restricted {
            return Ok(());
        }
        let target = self.map[s].target.clone();
        let Some(held) = self.withheld(&target, key).await? else {
            return Ok(());
        };
        let done = match (reason, held) {
            (Some(reason), false) => self.target.restrict(&target, key, reason.clone()).await,
            (None, true) => self.target.unrestrict(&target, key).await,
            // Delivered again: the copy is as the source left it.
            _ => return Ok(()),
        };
        match done {
            Ok(_) => {
                let what = if reason.is_some() {
                    "restricted"
                } else {
                    "unrestricted"
                };
                self.journal(json!({ what: target, "key": key, "at": p.0 }));
                Ok(())
            }
            Err(Error::NotFound { .. }) => Ok(()),
            Err(e @ Error::Refused { .. }) => self.stop(
                std::slice::from_ref(s),
                p,
                format!("the target refused a hold: {e}"),
            ),
            Err(e) => Err(format!("the target failed: {e}")),
        }
    }

    /// A declaration or migration of a mapped source type: go on if the
    /// target still admits it at this position, stop the type (and every
    /// type that refers to it) if not.
    async fn redeclared(&mut self, s: &TypeName, p: Position) -> Result<(), String> {
        let Some(mapped) = self.map.get(s).cloned() else {
            return Ok(());
        };
        if mapped.link.is_some() {
            return Ok(());
        }
        let tcat = self.target.catalog(WriteContext::default());
        let td = tcat
            .describe_type(&self.writer, &mapped.target, ReadAt::default())
            .await
            .map_err(|e| {
                format!(
                    "the target type `{}` cannot be described: {e}",
                    mapped.target
                )
            })?
            .data;
        let names: BTreeMap<TypeName, TypeName> = self
            .map
            .iter()
            .filter(|(_, m)| m.link.is_none())
            .map(|(s, m)| (s.clone(), m.target.clone()))
            .collect();
        let why = match self.describe_source(s, Some(p)).await {
            Ok(sd) => admits(&sd, &td, &names).err(),
            Err(why) => Some(why),
        };
        let Some(why) = why else {
            return Ok(());
        };
        // Types that refer to this one would be refused next; they stop
        // with it, named.
        let mut stopping = self.owned(s);
        for (other, m) in &self.map {
            if m.fields.values().flatten().any(|t| *t == mapped.target) && !stopping.contains(other)
            {
                stopping.push(other.clone());
            }
        }
        self.stop(
            &stopping,
            p,
            format!(
                "the source changed `{s}` at position {p}, which `{}` does not admit: {why}",
                mapped.target
            ),
        )
    }

    /// `s` and its link types.
    fn owned(&self, s: &TypeName) -> Vec<TypeName> {
        let mut out = vec![s.clone()];
        out.extend(
            self.map
                .iter()
                .filter(|(_, m)| m.link.as_ref().is_some_and(|(o, _, _)| o == s))
                .map(|(t, _)| t.clone()),
        );
        out
    }

    async fn describe_source(
        &self,
        s: &TypeName,
        p: Option<Position>,
    ) -> Result<TypeDescription, String> {
        let scat = self.source.catalog(WriteContext::default());
        let read_at = p.map(at).unwrap_or_default();
        scat.describe_type(&self.reader, s, read_at)
            .await
            .map(|r| r.data)
            .map_err(|e| format!("the source type `{s}` cannot be described: {e}"))
    }

    async fn head(&self) -> Result<Position, String> {
        self.source
            .head()
            .await
            .map_err(|e| format!("the source's head cannot be read: {e}"))
    }

    /// The source record as the principal reads it at `p`; `None` when it
    /// cannot see it there.
    async fn read(
        &self,
        s: &TypeName,
        key: &RecordKey,
        p: Position,
    ) -> Result<Option<Record>, String> {
        let scat = self.source.catalog(WriteContext::default());
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: at(p),
        };
        match scat.get(&self.reader, s, get).await {
            Ok(r) => Ok(Some(r.data)),
            Err(Error::NotFound { .. }) => Ok(None),
            Err(Error::Refused {
                reason: lakelands_api::Refusal::NotGranted,
                ..
            }) => Ok(None),
            Err(e) => Err(format!("`{s}` `{key}` cannot be read from the source: {e}")),
        }
    }

    /// The copy as the target holds it now; `None` when there is none, or
    /// it is retired.
    async fn copy(&self, t: &TypeName, key: &RecordKey) -> Result<Option<Record>, String> {
        let tcat = self.target.catalog(WriteContext::default());
        let mut who = self.writer.clone();
        who.grants.restricted = true;
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
        };
        match tcat.get(&who, t, get).await {
            Ok(r) => Ok(Some(r.data)),
            Err(Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(format!("`{t}` `{key}` cannot be read from the target: {e}")),
        }
    }

    /// Whether the copy is held: `None` when there is no live copy.
    async fn withheld(&self, t: &TypeName, key: &RecordKey) -> Result<Option<bool>, String> {
        if self.copy(t, key).await?.is_none() {
            return Ok(None);
        }
        let tcat = self.target.catalog(WriteContext::default());
        let mut who = self.writer.clone();
        who.grants.restricted = false;
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
        };
        match tcat.get(&who, t, get).await {
            Ok(_) => Ok(Some(false)),
            Err(Error::NotFound { .. }) => Ok(Some(true)),
            Err(e) => Err(format!("`{t}` `{key}` cannot be read from the target: {e}")),
        }
    }

    /// Stop `types` at `p` for `why`: each keeps `p` as the position to
    /// read from when it resumes.
    fn stop(&mut self, types: &[TypeName], p: Position, why: String) -> Result<(), String> {
        for t in types {
            self.kept.types.insert(
                t.clone(),
                TypeState {
                    next: p.0,
                    stopped: Some(Stopped {
                        at: p.0,
                        why: why.clone(),
                    }),
                },
            );
            self.say(&format!("`{t}` stopped at position {p}: {why}"));
            self.journal(json!({ "stopped": t, "at": p.0, "why": why }));
        }
        self.save()
    }

    /// Every running type has been brought across up to `next`.
    fn advance(&mut self, next: Position) -> Result<(), String> {
        let mut moved = false;
        for state in self.kept.types.values_mut() {
            if state.stopped.is_none() && state.next < next.0 {
                state.next = next.0;
                moved = true;
            }
        }
        if moved { self.save() } else { Ok(()) }
    }

    /// Keep the positions: written beside the file, synced, and renamed
    /// over it, so a crash leaves the old one or the new one.
    fn save(&self) -> Result<(), String> {
        let tmp = self.position_file.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&self.kept).expect("positions serialise");
        File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(&bytes)?;
                f.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, &self.position_file))
            .map_err(|e| format!("{} cannot be written: {e}", self.position_file.display()))
    }

    /// Append one line to the journal; a journal that cannot be written is
    /// said on standard error, and the mirror goes on.
    fn journal(&mut self, mut line: serde_json::Value) {
        if let Some(o) = line.as_object_mut() {
            o.insert("time".into(), json!(now().to_string()));
        }
        let mut bytes = serde_json::to_vec(&line).expect("a journal line serialises");
        bytes.push(b'\n');
        if let Err(e) = self.journal.write_all(&bytes) {
            self.say(&format!("the journal cannot be written: {e}"));
        }
    }

    fn say(&self, what: &str) {
        eprintln!(
            "lakelands: mirroring `{}` from lake `{}` into lake `{}`: {what}",
            self.name,
            self.source.lake(),
            self.target.lake()
        );
    }
}
