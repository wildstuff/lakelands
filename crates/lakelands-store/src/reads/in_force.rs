//! The record of another type in force at a record a read answers
//! (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`, extended;
//! lakelands#1606): `in_force(T.f, T.by <= by, T.m = m, …)` reads `f` of
//! the record of `T` whose `by` is the greatest at or below the record's
//! among those holding its matched values, the first by key among ties, of
//! those the reader's `find` on `T` answers at the read's position and
//! valid time. One whose `by` or a matched field a hiding lowers is passed
//! over; one whose `f` a hiding lowers is paired, and `f` reads as none.
//!
//! One read pairs a page at a time: each partition of `T` (one set of
//! matched values) is read once, in `by` order, and each record found by a
//! binary search; a partition of more than [`KEPT`] records is not kept,
//! and each record is paired by a scan of its own ([`at_or_below`]).

use std::collections::btree_map::Entry;

use lakelands_api::read::{Direction, Order};
use lakelands_core::expr::{Preceding, order::cmp_values};

use super::*;

/// The most records of one partition a read keeps.
const KEPT: usize = 1024;

/// What `r` holds in `by`; null where none.
fn at<'r>(r: &'r StoredRecord, by: &FieldName) -> &'r Value {
    r.fields.get(by).unwrap_or(&Value::Null)
}

/// The first record of `ty` that `fits`, from `here` down by `by`, ties in
/// key order: `scan` (its `equal`, valid time and flags the caller's) read
/// a page of `scan.limit` at a time, past what does not fit. A record at
/// `here` is read: `fits` bounds it strictly or not. The write's
/// `preceding` and the read's `in_force` both read their series by it.
pub(crate) fn at_or_below(
    view: &dyn ProjectionView,
    ty: &TypeName,
    (by, here): (&FieldName, &Value),
    mut scan: Scan,
    fits: impl Fn(&StoredRecord) -> bool,
) -> Result<Option<StoredRecord>> {
    scan.order_by = vec![Order {
        field: by.clone(),
        by: Default::default(),
        dir: Direction::Desc,
    }];
    scan.after = Some(After {
        values: vec![here.clone()],
        key: RecordKey::from(""),
    });
    loop {
        let page = view.scan(ty, &scan)?;
        if let Some(r) = page.iter().find(|r| fits(r)) {
            return Ok(Some(r.clone()));
        }
        match page.last() {
            Some(last) if page.len() as u64 >= u64::from(scan.limit) => {
                let values = vec![last.fields.get(by).cloned().unwrap_or(Value::Null)];
                let key = last.key.clone();
                scan.after = Some(After { values, key });
            }
            _ => return Ok(None),
        }
    }
}

/// A partition read for one call: its records in `by` and key order, as
/// the reader is told them; `None` where it holds more than [`KEPT`].
type Partition = Option<Vec<StoredRecord>>;

impl Reader<'_> {
    /// For each of `rows` of `decl`, the record in force in each series
    /// its fields computed at the read pair by, as [`Preceding`] holds
    /// them, empty where it pairs by none.
    pub(super) fn in_force(
        &self,
        decl: &TypeDecl,
        rows: &[StoredRecord],
    ) -> Result<Vec<Preceding>> {
        let mut series: BTreeMap<Vec<FieldName>, BTreeSet<FieldName>> = BTreeMap::new();
        for (_, e) in decl.in_force() {
            for p in e.pairings().unwrap_or_default() {
                series.entry(p.series()).or_default().insert(p.read.clone());
            }
        }
        let mut out = vec![Preceding::new(); rows.len()];
        let mut kept: BTreeMap<(&[FieldName], Vec<String>), Partition> = BTreeMap::new();
        for (s, read) in &series {
            let ty = TypeName::from(s[0].as_str());
            let Ok(t) = self.shape.ontology.ty(&ty) else {
                continue;
            };
            let pairs: Vec<(&FieldName, &FieldName)> =
                s[1..].chunks(2).map(|c| (&c[0], &c[1])).collect();
            let Some(((tby, by), matched)) = pairs.split_first() else {
                continue;
            };
            let low = |r: &StoredRecord| self.sight.lowered(&ty, &r.fields);
            let fits = |r: &StoredRecord| {
                let low = low(r);
                let told = |f: &FieldName| !low.contains_key(f);
                !r.retired
                    && r.fields.get(*tby).is_some_and(|v| !v.is_null())
                    && self.sight.answers(&ty, r)
                    && told(tby)
                    && matched.iter().all(|(m, _)| told(m))
            };
            let (valid_at, bound) = self.valid_for(t);
            for (i, r) in rows.iter().enumerate() {
                let value = |f: &FieldName| r.fields.get(f).filter(|v| !v.is_null()).cloned();
                let equal = matched.iter().map(|(m, f)| Some(((*m).clone(), value(f)?)));
                let (Some(here), Some(equal)) = (value(by), equal.collect::<Option<Vec<_>>>())
                else {
                    continue;
                };
                let scan = |limit| Scan {
                    equal: equal.clone(),
                    limit,
                    valid_at,
                    filter: bound.clone(),
                    ..crate::lake::every(false)
                };
                let values = equal.iter().map(|(_, v)| v.to_string()).collect();
                let part = match kept.entry((s.as_slice(), values)) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => {
                        let page = self.view.scan(&ty, &scan(KEPT as u32 + 1))?;
                        v.insert((page.len() <= KEPT).then(|| {
                            let mut all: Vec<_> = page.into_iter().filter(|r| fits(r)).collect();
                            all.sort_by(|a, b| {
                                cmp_values(at(a, tby), at(b, tby)).then(a.key.cmp(&b.key))
                            });
                            all
                        }))
                    }
                };
                let found = match part {
                    Some(all) => {
                        let n = all.partition_point(|r| cmp_values(at(r, tby), &here).is_le());
                        let first = |v: &Value| {
                            all[..n].partition_point(|r| cmp_values(at(r, tby), v).is_lt())
                        };
                        n.checked_sub(1)
                            .map(|last| all[first(at(&all[last], tby))].clone())
                    }
                    None => {
                        let le =
                            |r: &StoredRecord| fits(r) && cmp_values(at(r, tby), &here).is_le();
                        at_or_below(self.view.as_ref(), &ty, (tby, &here), scan(1), le)?
                    }
                };
                let Some(told) = found.as_ref().and_then(|f| self.sight.told(f)) else {
                    continue;
                };
                let held = |f: &FieldName| told.fields.get(f).cloned().unwrap_or(Value::Null);
                out[i].insert(
                    s.clone(),
                    read.iter().map(|f| (f.clone(), held(f))).collect(),
                );
            }
        }
        Ok(out)
    }
}
