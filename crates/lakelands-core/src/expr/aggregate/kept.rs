//! A group's folds, kept: what a view that is kept holds for a group in
//! place of its members, so that a member arriving, leaving or changing
//! moves the group's measures without the group being read again.
//!
//! A kept state is a function of the group's members as a multiset: the
//! same members, put and taken in any order, leave the same state and the
//! same bytes. It answers what the computation answers over those members
//! while it is exact; where it is not (a member whose argument does not
//! fold, a sum past exact arithmetic, an extreme over a number and an
//! instant, an extreme whose holder left) it says so and decides nothing.
//!
//! **What the caller owes:** it takes a member it put, with the fields and
//! under the settings it put it with, and it uses a state with the calls
//! it was made under. A state cannot tell a member taken that was never
//! put unless a count goes below nothing, nor one made under other calls
//! unless their number differs; where it can tell, it is not exact.
//!
//! - A count is a number.
//! - A sum is its numbers by scale (`Scales`), which a number is taken
//!   out of as exactly as it was put in.
//! - A minimum or maximum is its value and the key of the member that
//!   holds it: the first member in the extreme's order ([`Entry`]), which
//!   ranks every value by its sort key, a number before an instant, and
//!   equal values by the least key. So the holder is a function of the
//!   members whatever the order they arrived in. When the holder leaves,
//!   the next one is the first entry of an order the store keeps.

use std::ops::Not;

use lakelands_api::expr::{Expr, Func};
use lakelands_api::{FieldName, RecordKey, Timestamp};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{Fold, Folds, Member, Row, Scales, Settings};
use crate::expr::eval::{RecordScope, Result, Val, as_number, as_time, eval};
use crate::expr::order::val_key;

/// An extreme's value as the evaluator read it, so that what a measure
/// hands on is the value the computation hands on. A number is held as
/// its text: two that are equal as numbers and written at different
/// scales are different values here, as they are different bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Held {
    Num(String),
    Text(String),
    Time(i64),
}

impl Held {
    /// A value an extreme folds: one that reads as a number or an instant.
    fn of(v: &Val) -> Option<Held> {
        match v {
            Val::Num(d) => Some(Held::Num(d.to_string())),
            Val::Time(t) => Some(Held::Time(t.0)),
            Val::Text(s) if as_number(v).is_some() || as_time(v).is_some() => {
                Some(Held::Text(s.clone()))
            }
            _ => None,
        }
    }

    fn val(&self) -> Val {
        match self {
            Held::Num(s) => {
                // A zero's sign is written and not parsed.
                let mut d: Decimal = s.parse().unwrap_or_default();
                d.set_sign_negative(s.starts_with('-'));
                Val::Num(d)
            }
            Held::Text(s) => Val::Text(s.clone()),
            Held::Time(t) => Val::Time(Timestamp(*t)),
        }
    }
}

/// One member's argument to one call: null, which no fold counts; a value
/// the call folds; or one it does not (a text under `sum`, a number no
/// decimal holds, an argument that fails). The test is of the member
/// alone, so it reads the same when the member leaves.
enum Arg {
    None,
    Value(Val),
    Odd,
}

impl Arg {
    fn of(func: Func, arg: Option<&Expr>, scope: &RecordScope<'_>) -> Arg {
        // `count()` counts every member.
        let Some(arg) = arg else {
            return Arg::Value(Val::Null);
        };
        let folds = |v: &Val| match func {
            Func::Sum | Func::Avg => as_number(v).is_some(),
            Func::Min | Func::Max => Held::of(v).is_some(),
            _ => true,
        };
        match eval(arg, scope) {
            Ok(Val::Null) => Arg::None,
            Ok(v) if folds(&v) => Arg::Value(v),
            _ => Arg::Odd,
        }
    }
}

/// One aggregate call's kept fold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct KeptFold {
    /// The members folded in.
    n: u64,
    /// The members whose argument does not fold.
    #[serde(default, skip_serializing_if = "is_zero")]
    odd: u64,
    #[serde(default, skip_serializing_if = "Scales::is_empty")]
    sum: Scales,
    /// Of the members folded into an extreme, those that read as an
    /// instant and as no number. A number and an instant have no order
    /// between them, so an extreme that holds both is not exact.
    #[serde(default, skip_serializing_if = "is_zero")]
    times: u64,
    /// The extreme and its holder's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    best: Option<(Held, RecordKey)>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl KeptFold {
    /// One member's argument leaves (`was`) and one arrives (`is`), each
    /// under its key. Answers whether the counts hold (what left had been
    /// put) and whether the extreme's holder left members behind it.
    fn moved(
        &mut self,
        func: Func,
        was: Option<(&RecordKey, Arg)>,
        is: Option<(&RecordKey, Arg)>,
    ) -> (bool, bool) {
        let mut held = true;
        let step = |n: &mut u64, put: bool| {
            let next = if put {
                n.checked_add(1)
            } else {
                n.checked_sub(1)
            };
            *n = next.unwrap_or(*n);
            next.is_some()
        };
        for (side, put) in [(&was, false), (&is, true)] {
            held &= match side {
                Some((_, Arg::Odd)) => step(&mut self.odd, put),
                Some((_, Arg::Value(v))) => {
                    let kind = match (func, as_number(v)) {
                        (Func::Sum | Func::Avg, Some(n)) => {
                            self.sum.fold(n, put);
                            true
                        }
                        (Func::Min | Func::Max, None) => step(&mut self.times, put),
                        _ => true,
                    };
                    step(&mut self.n, put) && kind
                }
                _ => true,
            };
        }
        if !extreme(func) {
            return (held, false);
        }
        let new = is.as_ref().and_then(|(key, arg)| match arg {
            Arg::Value(v) => Held::of(v).map(|h| (place(func, v), h, *key)),
            _ => None,
        });
        let holder = self.best.as_ref().map(|(_, key)| key);
        let leaves = was.as_ref().is_some_and(|(k, _)| holder == Some(*k));
        // The first entry of the order holds: the holder's own change keeps
        // it where it stays first, and another member takes over where it
        // comes first.
        let takes = match (&new, &self.best) {
            (Some((at, _, key)), Some((best, holder))) => {
                (at, *key) <= (&place(func, &best.val()), holder)
            }
            (new, _) => new.is_some(),
        };
        // With no other member folded in there is nothing to read again.
        let alone = self.n.saturating_sub(u64::from(new.is_some())) == 0;
        if takes || (leaves && alone) {
            self.best = new.map(|(_, h, key)| (h, key.clone()));
        }
        (held, leaves && !takes && !alone)
    }
}

/// What a group of a kept view keeps instead of its members, for one of
/// the view's sources: the number of its members and one fold for each
/// aggregate call of the source's measures. Written into a record's field
/// it is a JSON object, and reads back equal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KeptFolds {
    members: u64,
    folds: Vec<KeptFold>,
    #[serde(default, skip_serializing_if = "Not::not")]
    waits: bool,
    #[serde(default, skip_serializing_if = "Not::not")]
    lost: bool,
}

impl KeptFolds {
    /// No member is put: the group has no record.
    pub fn is_empty(&self) -> bool {
        self.members == 0
    }

    /// The folds answer what the computation answers over the same
    /// members: no member whose argument does not fold, every sum inside
    /// exact arithmetic, every extreme over values of one kind and with
    /// its holder, and no count that went below nothing.
    pub fn exact(&self) -> bool {
        let one_kind = |f: &KeptFold| f.times == 0 || f.times == f.n;
        let exact = |f: &KeptFold| f.odd == 0 && one_kind(f) && f.sum.exact().is_some();
        !self.lost && !self.waits && self.folds.iter().all(exact)
    }

    /// An extreme's holder left and the next is to be read from the
    /// extreme's order ([`Keeping::waiting`], [`Keeping::seated`]). It
    /// stays so until then.
    pub fn waits(&self) -> bool {
        self.waits
    }
}

/// Whether `func` is an extreme, `min` or `max`.
fn extreme(func: Func) -> bool {
    matches!(func, Func::Min | Func::Max)
}

/// Where a value stands in the order of the extreme `func`: its sort key
/// for a `min`; for a `max` the same with a zero byte after it, every byte
/// complemented, so that the greatest value comes first and a value whose
/// key begins another's (`1` before `1.5`) comes after it.
fn place(func: Func, v: &Val) -> Vec<u8> {
    let key = val_key(v);
    match func {
        Func::Max => key.into_iter().chain([0]).map(|b| !b).collect(),
        _ => key,
    }
}

/// One member's entry in the order of one extreme: its value as the fold
/// read it, so that a holder read from the order hands on its own bytes,
/// and its key. Written into a record's field it is a JSON object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    v: Held,
    m: RecordKey,
}

/// Where a member stands in an extreme's order: its place ([`place`]) and
/// its key.
pub type Standing = (Vec<u8>, RecordKey);

/// An entry of one extreme for one member: the extreme's index among the
/// source's calls, the member's place in its order ([`place`], then its
/// key), and the entry.
pub type Placed = (usize, Vec<u8>, Entry);

/// A member under its key.
type Keyed<'a> = (&'a RecordKey, Member<'a>);

/// The aggregate calls a source's measures make, each once, in their
/// order: what folds a member into a group's kept state. A state is used
/// with the `Keeping` of the declaration it was made under: one with
/// another number of folds is not exact, and one with as many folds under
/// other calls cannot be told, so a view declared again is made again.
#[derive(Clone, Debug)]
pub struct Keeping(Vec<(Func, Option<Expr>)>);

impl Keeping {
    /// The calls `measures` make.
    pub fn of(measures: &[(FieldName, Expr)]) -> Keeping {
        let calls = Folds::of(measures).0.into_iter();
        Keeping(calls.map(|(func, arg, _)| (func, arg)).collect())
    }

    /// The state of a group with no member.
    pub fn none(&self) -> KeptFolds {
        KeptFolds {
            folds: vec![KeptFold::default(); self.0.len()],
            ..KeptFolds::default()
        }
    }

    /// A member arrives. An aggregate's argument is evaluated over its
    /// fields and their provenance, without a clock, as the computation
    /// evaluates it; a view that reads the clock is not kept.
    pub fn put(&self, folds: &mut KeptFolds, is: Keyed<'_>, settings: Settings<'_>) {
        self.moved(folds, None, Some(is), settings);
    }

    /// A member leaves, with the key, the fields and the provenance it was
    /// put with, under the settings it was put under. Answers
    /// [`KeptFolds::waits`].
    pub fn take(&self, folds: &mut KeptFolds, was: Keyed<'_>, with: Settings<'_>) -> bool {
        self.moved(folds, Some(was), None, with)
    }

    /// A member stays in its group and changes: its key, its fields, or
    /// both. An extreme's holder that stays first in the extreme's order
    /// stays the holder; else it has left. Answers [`KeptFolds::waits`].
    pub fn change(
        &self,
        folds: &mut KeptFolds,
        was: Keyed<'_>,
        is: Keyed<'_>,
        settings: Settings<'_>,
    ) -> bool {
        self.moved(folds, Some(was), Some(is), settings)
    }

    fn moved(
        &self,
        folds: &mut KeptFolds,
        was: Option<Keyed<'_>>,
        is: Option<Keyed<'_>>,
        settings: Settings<'_>,
    ) -> bool {
        let members = match (was, is) {
            (None, Some(_)) => folds.members.checked_add(1),
            (Some(_), None) => folds.members.checked_sub(1),
            _ => Some(folds.members),
        };
        folds.lost |= members.is_none() || folds.folds.len() != self.0.len();
        folds.members = members.unwrap_or(folds.members);
        for ((func, arg), fold) in self.0.iter().zip(&mut folds.folds) {
            let read = |(fields, meta): Member<'_>| {
                let now = Timestamp(0);
                let scope = RecordScope {
                    fields,
                    meta,
                    settings,
                    now,
                };
                Arg::of(*func, arg.as_ref(), &scope)
            };
            let (was, is) = (was.map(|m| (m.0, read(m.1))), is.map(|m| (m.0, read(m.1))));
            let (held, left) = fold.moved(*func, was, is);
            folds.lost |= !held;
            folds.waits |= left;
        }
        folds.waits
    }

    /// The member's entries, one for each extreme it holds a value for: a
    /// member with a null argument, or one an extreme does not fold, has
    /// none, as it has no part in the extreme.
    pub fn entries(&self, (key, is): Keyed<'_>, settings: Settings<'_>) -> Vec<Placed> {
        let ((fields, meta), now) = (is, Timestamp(0));
        let scope = RecordScope {
            fields,
            meta,
            settings,
            now,
        };
        let mut placed = Vec::new();
        for (i, (func, arg)) in self.0.iter().enumerate().filter(|(_, (f, _))| extreme(*f)) {
            if let Arg::Value(v) = Arg::of(*func, arg.as_ref(), &scope)
                && let Some(v) = Held::of(&v).map(|held| (place(*func, &v), held))
            {
                let m = key.clone();
                placed.push((i, v.0, Entry { v: v.1, m }));
            }
        }
        placed
    }

    /// Where the order of each extreme of `folds` is read from when they
    /// wait: the place of the holder they keep, which is the first entry
    /// unless it left, and then the next one follows it. None when they
    /// do not wait.
    pub fn waiting(&self, folds: &KeptFolds) -> Vec<(usize, Option<Standing>)> {
        let mut from = Vec::new();
        for (i, ((func, _), f)) in self.0.iter().zip(&folds.folds).enumerate() {
            if folds.waits && extreme(*func) {
                from.push((
                    i,
                    f.best
                        .as_ref()
                        .map(|(h, k)| (place(*func, &h.val()), k.clone())),
                ));
            }
        }
        from
    }

    /// Each waiting extreme's holder is the first entry of its order, or
    /// none where the order holds none. An order with no entry for an
    /// extreme that still folds members has lost them, and the folds are
    /// not exact.
    pub fn seated(&self, folds: &mut KeptFolds, first: Vec<(usize, Option<Entry>)>) {
        for (i, entry) in first {
            let fold = &mut folds.folds[i];
            folds.lost |= entry.is_none() && fold.n > 0;
            fold.best = entry.map(|e| (e.v, e.m));
        }
        folds.waits = false;
    }

    /// One group's measures over its kept folds, as [`super::measures`]
    /// answers them over the computation's: the same values, byte for
    /// byte, where the state is exact. An error is a measure that fails
    /// over this group.
    pub fn measures(
        &self,
        folds: &KeptFolds,
        by: &Row,
        measures: &[(FieldName, Expr)],
        given: Row,
        settings: Settings<'_>,
    ) -> Result<Row> {
        let folded = self.0.iter().zip(&folds.folds).map(|((func, arg), kept)| {
            let fold = Fold {
                count: usize::try_from(kept.n).unwrap_or(usize::MAX),
                scales: kept.sum.clone(),
                best: kept.best.as_ref().map(|(held, _)| held.val()),
                ..Fold::default()
            };
            (*func, arg.clone(), fold)
        });
        let folds = Folds(folded.collect());
        super::measures(&folds, by, measures, given, settings, Timestamp(0))
    }
}

#[cfg(test)]
mod tests;
