//! A quarantined record: a source's record its ingest was refused, kept
//! with its cells and its refusal until it is run again, dismissed, or
//! its key lands
//! (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{DeliveryId, FieldName, Position, RecordKey, SourceId, TypeName};
use crate::record::{Patch, Upstream};
use crate::time::Timestamp;

/// A quarantined record's id: its source, the host's run or else the
/// delivery that first quarantined it, and the host's line or else the
/// record's place among the call's records of that delivery. It is never
/// re-keyed; ids order by source, delivery, then index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineId {
    /// The record's source.
    pub source: SourceId,
    /// The run that quarantined it, else the delivery that first did.
    pub delivery: DeliveryId,
    /// Its line, else its place in that delivery, from 0.
    pub index: u64,
}

impl fmt::Display for QuarantineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.source, self.delivery, self.index)
    }
}

/// What a source record resubmits: the delivery and place of a
/// quarantined record of the record's own source.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resubmits {
    /// The delivery that first quarantined it.
    pub delivery: DeliveryId,
    /// Its place in that delivery's stream.
    pub index: u64,
}

/// Whether a quarantined record is open or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineState {
    /// Waiting for a resubmission, a dismiss, or its key to land.
    Open,
    /// Closed, with why.
    Closed,
}

/// Why a quarantined record closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedWhy {
    /// Its resubmission was written, or counted as a duplicate.
    Resubmitted,
    /// A caller's write door wrote a record at its type and key.
    Landed,
    /// A holder of `ingest` dismissed it.
    Dismissed,
    /// Its source was forgotten.
    Forgotten,
    /// The values of its last version were taken back.
    Removed,
    /// Its source, or its type of that source, was purged.
    Purged,
}

/// A quarantined record's close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineClose {
    /// Why it closed.
    pub why: ClosedWhy,
    /// The position of its `quarantine_closed`.
    pub position: Position,
}

/// A quarantined record, as the projection keeps it and as
/// `Catalog::quarantined` lists it: its last version, and its close.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quarantined {
    /// Its id.
    pub id: QuarantineId,
    /// The record's type: listed under its name at the head.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The key as sent.
    pub key: RecordKey,
    /// Its last version, from 1.
    pub version: u32,
    /// The position of its last version.
    pub position: Position,
    /// The lake's time of its last version.
    pub quarantined_at: Timestamp,
    /// The cells as sent, each in its canonical form, under the names of
    /// their fields at the head: those its reader may read. A closed one
    /// holds none, nor does an erased cell.
    #[serde(default, skip_serializing_if = "Patch::is_empty")]
    pub cells: Patch,
    /// The cells its reader is shown masked, their values withheld.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked: Vec<FieldName>,
    /// The cells whose field was `sensitive` when it came, which keep the
    /// raise: the projection's, never listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensitive: Vec<FieldName>,
    /// When the source observed the values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
    /// Where the values were read from, for a mirror's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<Upstream>,
    /// The refusal its writer was answered; to a reader not shown every
    /// name it holds, its kind alone, and `more`.
    pub refusal: crate::Error,
    /// Whether the refusal holds more than its reader is shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
    /// Its close, for a closed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<QuarantineClose>,
}

/// Which quarantined records `Catalog::quarantined` lists, in id order.
/// A source, a delivery and an index together get one by its id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineFilter {
    /// Only this source's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// Only those of this type, by its name at the head.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// Only those first quarantined by this delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryId>,
    /// Only the one at this place of its delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    /// Only open ones, or only closed ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<QuarantineState>,
    /// At most this many; the lake's limits bound it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Those after this id: the last of an earlier page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<QuarantineId>,
    /// The store's own: only those at this key as sent.
    #[serde(skip)]
    pub key: Option<RecordKey>,
}

impl QuarantineFilter {
    /// Whether `q` is one the filter asks for, by its type as stored.
    pub fn holds(&self, q: &Quarantined) -> bool {
        fn is<T: PartialEq>(want: &Option<T>, got: &T) -> bool {
            want.as_ref().is_none_or(|w| w == got)
        }
        let state = match q.closed {
            None => QuarantineState::Open,
            Some(_) => QuarantineState::Closed,
        };
        is(&self.source, &q.id.source)
            && is(&self.ty, &q.ty)
            && is(&self.delivery, &q.id.delivery)
            && self.index.is_none_or(|i| i == q.id.index)
            && self.state.is_none_or(|s| s == state)
            && is(&self.key, &q.key)
            && self.after.as_ref().is_none_or(|a| &q.id > a)
    }

    /// The rows `all` holds that the filter asks for, in id order, at
    /// most its limit: every engine's answer.
    pub fn answer(&self, all: impl IntoIterator<Item = Quarantined>) -> Vec<Quarantined> {
        let mut rows: Vec<_> = all.into_iter().filter(|q| self.holds(q)).collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows.truncate(self.limit.map_or(usize::MAX, |l| l as usize));
        rows
    }
}

/// A record the host refused before any ingest, to be quarantined as one
/// the lake refused (085, extended): its row as the host would have
/// ingested it, its cells in `fields`, and the host's refusal.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRefused {
    /// The row, its key empty where the host could not form one.
    pub record: crate::SourceRecord,
    /// The stable cause code: 1 to 64 bytes of `a`–`z`, `0`–`9` and `_`.
    pub cause: String,
    /// The sentence for the operator, which names no cell's value.
    pub reason: String,
}

impl fmt::Debug for HostRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = &self.record;
        f.debug_struct("HostRefused")
            .field("type", &r.ty)
            .field("key", &r.key)
            .field("cells", &r.fields.keys().collect::<Vec<_>>())
            .field("line", &r.line)
            .field("cause", &self.cause)
            .finish_non_exhaustive()
    }
}

/// What `Catalog::quarantine` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quarantining {
    /// The id the record is quarantined under.
    pub id: QuarantineId,
    /// The position of its version; the head, for a duplicate.
    pub position: Position,
    /// Whether it was the id's last version refused the same way, and
    /// appended nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub duplicate: bool,
}

/// The bound that was full where a refusal past it was not quarantined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineFull {
    /// The lake's `limits.quarantined_max`.
    pub max: u32,
}

/// The quarantined records of one type an act on a source closes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeQuarantined {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// How many of its quarantined records.
    pub quarantined: u64,
}

#[cfg(test)]
mod tests;
