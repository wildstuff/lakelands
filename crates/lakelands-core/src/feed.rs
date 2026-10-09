//! What the change feed's filter reads an event by: its kind and what it
//! is about. The fold keeps this of every event it applies, as the
//! projection's index of the log, and a poll checks every event it answers
//! by the same rule, so the index can only narrow what a poll reads: the
//! answer always comes from the log (`docs/seams.md` §`ChangeFeed`).

use lakelands_api::engine::About;
use lakelands_api::event::EventBody;
use lakelands_api::id::{RecordKey, TypeName};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;

/// The most positions an engine's index decides in one
/// [`matching`](lakelands_api::engine::ProjectionView::matching) whose
/// filter names types and kinds and no key: such a plan reads a type's
/// entries and checks each one's kind, and a type of many events and a
/// kind it seldom has would otherwise read all of them in one poll.
pub const WINDOW: u64 = 65_536;

/// What `body` is about, one entry a type it concerns, never empty:
/// - a `record_*`, `trigger_fired` or `object_set_changed` event: its type
///   and its record's key;
/// - a type's `declared`: the type;
/// - a `migrated`: its type, and each link type its renamed relations move
///   records between, old and new, so a consumer that follows either hears
///   of the move (035, extended);
/// - a `value_type_migrated`: every type whose records it rewrites;
/// - a `verb_called`: its record's key, of no type;
/// - any other event: an entry with neither.
pub fn about(body: &EventBody) -> Vec<About> {
    let of = |ty: &TypeName| About {
        ty: Some(ty.clone()),
        key: None,
    };
    let about = match body {
        EventBody::RecordUpserted { ty, key, .. }
        | EventBody::RecordIngested { ty, key, .. }
        | EventBody::RecordRetired { ty, key, .. }
        | EventBody::RecordRestricted { ty, key, .. }
        | EventBody::RecordUnrestricted { ty, key, .. }
        | EventBody::TriggerFired { ty, key, .. }
        | EventBody::ObjectSetChanged { ty, key, .. } => vec![About {
            ty: Some(ty.clone()),
            key: Some(key.clone()),
        }],
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => vec![of(&t.name)],
        EventBody::Migrated { migration, .. } => {
            let moved = crate::links::moved(migration);
            let mut types: Vec<&TypeName> = vec![&migration.to.name];
            types.extend(moved.iter().flat_map(|(old, new)| [old, new]));
            types.sort();
            types.dedup();
            types.into_iter().map(of).collect()
        }
        EventBody::ValueTypeMigrated { types, .. } => types.iter().map(of).collect(),
        EventBody::VerbCalled { key, .. } => vec![About {
            ty: None,
            key: key.clone(),
        }],
        _ => Vec::new(),
    };
    if about.is_empty() {
        return vec![About {
            ty: None,
            key: None,
        }];
    }
    about
}

/// Whether `filter` admits an event of `kind` about `about`: its kind is
/// one of `kinds`, and one entry of `about` has a type of `types` and a
/// key of `keys`, an empty list admitting any, so that with both named an
/// event's type and key must match together. A filter that names nothing
/// admits every event.
pub fn admits(filter: &FeedFilter, kind: &str, about: &[About]) -> bool {
    let kind_ok = filter.kinds.is_empty() || filter.kinds.iter().any(|k| k == kind);
    kind_ok
        && (filter.types.is_empty() && filter.keys.is_empty()
            || about.iter().any(|a| {
                let ty_ok = filter.types.is_empty()
                    || a.ty.as_ref().is_some_and(|t| filter.types.contains(t));
                let key_ok = filter.keys.is_empty()
                    || a.key.as_ref().is_some_and(|k| filter.keys.contains(k));
                ty_ok && key_ok
            }))
}

/// Which entries of the index an engine seeks a filter's positions by:
/// those of each of its keys if it names any, else of each of its types,
/// else of each of its kinds, each in position order, checking the rest of
/// the filter on every entry it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seek<'a> {
    /// The entries of these keys.
    Keys(&'a [RecordKey]),
    /// The entries of these types.
    Types(&'a [TypeName]),
    /// The entries of these kinds.
    Kinds(&'a [String]),
}

/// The seek of `filter`; none for a filter that names nothing.
pub fn seek(filter: &FeedFilter) -> Option<Seek<'_>> {
    if !filter.keys.is_empty() {
        Some(Seek::Keys(&filter.keys))
    } else if !filter.types.is_empty() {
        Some(Seek::Types(&filter.types))
    } else if !filter.kinds.is_empty() {
        Some(Seek::Kinds(&filter.kinds))
    } else {
        None
    }
}

/// The last position one `matching` from `from` decides, at most
/// `through`: [`WINDOW`] positions on when the filter's seek checks a kind
/// on every entry of a type, `through` otherwise.
pub fn decides_through(
    filter: &FeedFilter,
    from: lakelands_api::Position,
    through: lakelands_api::Position,
) -> lakelands_api::Position {
    let windowed = filter.keys.is_empty() && !filter.types.is_empty() && !filter.kinds.is_empty();
    if windowed {
        lakelands_api::Position(from.0.saturating_add(WINDOW - 1)).min(through)
    } else {
        through
    }
}

/// The last position a `matching` from `from` decides, at most `to`: no
/// further than the projection had folded where its index is read, in the
/// same snapshot, since a view taken before a rebuild's `clear` would
/// otherwise decide positions the refold has not indexed yet, and a poll
/// would pass their events by. `None` where that is before `from`.
pub fn last_decided(
    from: lakelands_api::Position,
    to: lakelands_api::Position,
    folded: Option<lakelands_api::Position>,
) -> Option<lakelands_api::Position> {
    folded.map(|f| to.min(f)).filter(|last| *last >= from)
}

/// The answer of a `matching` that decides nothing, so the store reads
/// the log.
pub fn undecided() -> lakelands_api::engine::Matched {
    lakelands_api::engine::Matched {
        positions: Vec::new(),
        through: None,
    }
}

/// The answer of a `matching` through `through` from the positions each
/// value of its seek admits, each list in order and of at most `limit`:
/// their first `limit` together, and the last of them as `through` when
/// there are that many. A position among the first `limit` of all is among
/// the first `limit` of its own value, so no list needs more.
pub fn merged(
    lists: impl IntoIterator<Item = Vec<lakelands_api::Position>>,
    limit: u32,
    through: lakelands_api::Position,
) -> lakelands_api::engine::Matched {
    let all: std::collections::BTreeSet<_> = lists.into_iter().flatten().collect();
    let positions: Vec<_> = all.into_iter().take(limit.max(1) as usize).collect();
    let through = match positions.last() {
        Some(last) if positions.len() == limit.max(1) as usize => *last,
        _ => through,
    };
    lakelands_api::engine::Matched {
        positions,
        through: Some(through),
    }
}

#[cfg(test)]
mod tests;
