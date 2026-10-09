//! The projection a read as of an earlier position reads through for a
//! reader without the `restricted` grant: each record with its hold as it
//! stands at the head, not as it stood then. A hold is about the record,
//! not the moment, so a record held now is not there at any position, and
//! one held then but released since is answered as any record is
//! (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#790).
//! The head is asked once per page of records, by key, as 051 asks it of a
//! search's candidates for a holder of `only`, each record found there by
//! the name and the key it bears there ([`Since`]).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{After, Declared, Matched, ProjectionView, Scan};
use lakelands_api::event::{Digest, EventBody, EventId};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_api::{FieldName, Position, RecordKey, RelationName, Result, TypeName, Value};
use lakelands_core::engines as core_engines;

use crate::lake::Lake;

/// The records a count reads at once.
const PAGE: u32 = 1024;

/// Where a record of an earlier position stands at the head: under the
/// name its type bears there, after each rename since, and under the key
/// it bears there, after each re-key since.
pub(crate) struct Since {
    /// The types renamed since, in the log's order, each with its position.
    renamed: Vec<(Position, TypeName, TypeName)>,
    /// By a type's name at the head, each key a re-key since moved, and
    /// the key it bears at the head.
    moved: BTreeMap<TypeName, BTreeMap<RecordKey, RecordKey>>,
}

impl Since {
    /// Since `at`, by the head `head` of `lake`. A re-key comes with its
    /// type's migration, which the type's declaration has been in force
    /// since: each declaration of a type made since is read, and so is the
    /// one before it, until one older than `at`.
    pub(crate) fn new(lake: &Lake, head: &dyn ProjectionView, at: Position) -> Result<Since> {
        let renamed = (lake.renames(Some(head))?.into_iter())
            .filter(|r| r.at > at)
            .filter_map(|r| Some((r.at, r.ty, r.to?)))
            .collect();
        let types = |declared: Vec<Declared>| {
            let types = declared.into_iter().filter(|d| d.since > at);
            types.filter_map(|d| match d.declaration {
                Declaration::Type(t) => Some((d.since, t.name)),
                _ => None,
            })
        };
        let (mut due, mut rekeys) = (
            types(head.declarations()?).collect::<Vec<_>>(),
            BTreeMap::new(),
        );
        while let Some((since, ty)) = due.pop() {
            if rekeys.contains_key(&since) {
                continue;
            }
            let event = lake.read_events(since, 1)?.into_iter().next();
            let rekeyed = match event.map(|e| e.body) {
                Some(EventBody::Migrated { rekeyed, .. }) => rekeyed,
                _ => Vec::new(),
            };
            rekeys.insert(since, rekeyed);
            let before = lake
                .engine
                .view(Some(Position(since.0 - 1)))?
                .declarations()?;
            due.extend(types(before).filter(|(_, t)| *t == ty));
        }
        let mut since = Since {
            renamed,
            moved: BTreeMap::new(),
        };
        // From each key a record of `at` bears now to its key then. One
        // migration moves its records at once, never one through another,
        // and a key whose record of `at` has moved away names another
        // record since, never that one.
        let mut first: BTreeMap<(TypeName, RecordKey), RecordKey> = BTreeMap::new();
        for (at, rekeyed) in rekeys {
            let mut moves = Vec::new();
            for r in rekeyed {
                let ty = since.name_after(&r.ty, at);
                let gone = (since.moved.get(&ty)).is_some_and(|m| m.contains_key(&r.from));
                let was = first.get(&(ty.clone(), r.from.clone())).cloned();
                if let Some(was) = was.or_else(|| (!gone).then(|| r.from.clone())) {
                    moves.push((ty, r.from, was, r.to));
                }
            }
            for (ty, from, _, _) in &moves {
                first.remove(&(ty.clone(), from.clone()));
            }
            for (ty, _, was, to) in moves {
                first.insert((ty.clone(), to.clone()), was.clone());
                since.moved.entry(ty).or_default().insert(was, to);
            }
        }
        Ok(since)
    }

    /// The name at the head of a type named `ty` at `at`.
    fn name_after(&self, ty: &TypeName, at: Position) -> TypeName {
        let mut name = ty.clone();
        for (_, from, to) in self.renamed.iter().filter(|r| r.0 > at) {
            if *from == name {
                name = to.clone();
            }
        }
        name
    }

    /// The name at the head of a type of the earlier position.
    pub(crate) fn name(&self, ty: &TypeName) -> TypeName {
        self.name_after(ty, Position::GENESIS)
    }

    /// The key at the head of the record `key` of the type named `name`
    /// there.
    pub(crate) fn key(&self, name: &TypeName, key: &RecordKey) -> RecordKey {
        let moved = self.moved.get(name).and_then(|m| m.get(key));
        moved.unwrap_or(key).clone()
    }
}

/// The view at a position, with the head beside it.
pub(crate) struct HeldNow<'a> {
    then: Box<dyn ProjectionView + 'a>,
    head: Box<dyn ProjectionView + 'a>,
    since: Since,
}

impl<'a> HeldNow<'a> {
    /// `then`, a view of `lake` at an earlier position, read as held now.
    pub(crate) fn new(lake: &'a Lake, then: Box<dyn ProjectionView + 'a>) -> Result<Self> {
        let head = lake.engine.view(None)?;
        let at = then.position().unwrap_or(Position::GENESIS);
        let since = Since::new(lake, head.as_ref(), at)?;
        Ok(HeldNow { then, head, since })
    }

    /// `rows` of `ty`, each with its hold at the head.
    fn now(&self, ty: &TypeName, rows: Vec<StoredRecord>) -> Result<Vec<StoredRecord>> {
        if rows.is_empty() {
            return Ok(rows);
        }
        let name = self.since.name(ty);
        let keys: Vec<RecordKey> = rows.iter().map(|r| self.since.key(&name, &r.key)).collect();
        Ok(held_as(rows, &keys, self.head.get_many(&name, &keys)?))
    }
}

/// `rows`, each held as the record at its key in `head` is, and one with
/// no record there as it was: a record is never released by not being
/// found.
fn held_as(
    mut rows: Vec<StoredRecord>,
    keys: &[RecordKey],
    head: Vec<StoredRecord>,
) -> Vec<StoredRecord> {
    let held: BTreeMap<RecordKey, bool> = head.into_iter().map(|r| (r.key, r.restricted)).collect();
    for (r, key) in rows.iter_mut().zip(keys) {
        r.restricted = held.get(key).copied().unwrap_or(r.restricted);
    }
    rows
}

impl ProjectionView for HeldNow<'_> {
    fn position(&self) -> Option<Position> {
        self.then.position()
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.then.declarations()
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        let row = self.then.get(ty, key)?.into_iter().collect();
        Ok(self.now(ty, row)?.pop())
    }

    /// The engine selects what the hold then would not, and a page without
    /// the records held now is read on until it is full: a short page is
    /// the last.
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        let mut asked = Scan {
            restricted: true,
            ..scan.clone()
        };
        let (want, mut out) = (scan.limit as usize, Vec::new());
        loop {
            let rows = self.then.scan(ty, &asked)?;
            let full = want > 0 && rows.len() >= asked.limit as usize;
            if let Some(last) = rows.last() {
                let values = (scan.order_by.iter())
                    .map(|o| core_engines::sort_value(last, o).unwrap_or(Value::Null));
                let key = last.key.clone();
                asked.after = Some(After {
                    values: values.collect(),
                    key,
                });
            }
            let rows = self.now(ty, rows)?.into_iter();
            out.extend(rows.filter(|r| scan.restricted || !r.restricted));
            if !full || out.len() >= want {
                out.truncate(want);
                return Ok(out);
            }
        }
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        if scan.restricted {
            return self.then.count(ty, scan);
        }
        let mut page = Scan {
            order_by: Vec::new(),
            after: None,
            limit: PAGE,
            ..scan.clone()
        };
        let mut n = 0;
        loop {
            let rows = self.scan(ty, &page)?;
            n += rows.len() as u64;
            match rows.last() {
                Some(last) if rows.len() == PAGE as usize => {
                    let key = last.key.clone();
                    page.after = Some(After {
                        values: Vec::new(),
                        key,
                    });
                }
                _ => return Ok(n),
            }
        }
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.now(ty, self.then.get_many(ty, keys)?)
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        self.then.edges(ty, keys, relation, walk)
    }

    fn lexical(
        &self,
        ty: &TypeName,
        text: &str,
        fields: Option<&[FieldName]>,
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        self.then.lexical(ty, text, fields, k)
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.then.proposals(filter)
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.then.is_member(set, ty, key)
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.then.members(set, ty)
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.then.applied(id)
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.then.output(seam, input)
    }

    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.then.matching(filter, from, through, limit)
    }
}

#[cfg(test)]
mod tests;
