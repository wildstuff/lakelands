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
//!   holds it: of equal values the least key, as a fold in key order
//!   keeps. No value's leaving is read again but the holder's.

use std::collections::BTreeMap;
use std::ops::Not;

use lakelands_api::expr::{CmpOp, Expr, Func};
use lakelands_api::{FieldName, RecordKey, Timestamp, Value};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{Fold, Folds, Row, Scales};
use crate::expr::eval::{RecordScope, Result, Val, as_number, as_time, compare, eval};

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
        let want = match func {
            Func::Min => CmpOp::Lt,
            Func::Max => CmpOp::Gt,
            _ => return (held, false),
        };
        let new = is.as_ref().and_then(|(key, arg)| match arg {
            Arg::Value(v) => Held::of(v).map(|h| (v, h, *key)),
            _ => None,
        });
        let holder = self.best.as_ref().map(|(_, key)| key);
        let leaves = was.as_ref().is_some_and(|(k, _)| holder == Some(*k));
        // The holder's own change replaces in place under an equal or
        // better value and the same or a lesser key; another member takes
        // over with a better value, or an equal one under a lesser key.
        let takes = match (&new, &self.best) {
            (Some((v, _, key)), Some((best, holder))) => {
                let is = |op| compare(op, v, &best.val()).unwrap_or(false);
                match leaves {
                    true => (is(want) || is(CmpOp::Eq)) && *key <= holder,
                    false => is(want) || (is(CmpOp::Eq) && *key < holder),
                }
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

    /// An extreme's holder left and the group is to be read again: put
    /// into [`Keeping::none`], member by member. It stays so until then.
    pub fn waits(&self) -> bool {
        self.waits
    }
}

/// A member: its key and its fields.
type Member<'a> = (&'a RecordKey, &'a Row);

/// The lake's settings by key, which an argument may read.
type Settings<'a> = Option<&'a BTreeMap<String, Value>>;

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
    /// fields without provenance and without a clock, as the computation
    /// evaluates it; a view that reads the clock is not kept.
    pub fn put(&self, folds: &mut KeptFolds, key: &RecordKey, row: &Row, settings: Settings<'_>) {
        self.moved(folds, None, Some((key, row)), settings);
    }

    /// A member leaves, with the key and the fields it was put with, under
    /// the settings it was put under. Answers [`KeptFolds::waits`].
    pub fn take(
        &self,
        folds: &mut KeptFolds,
        key: &RecordKey,
        row: &Row,
        with: Settings<'_>,
    ) -> bool {
        self.moved(folds, Some((key, row)), None, with)
    }

    /// A member stays in its group and changes: its key, its fields, or
    /// both. An extreme's holder that changes to an equal or better value
    /// under the same or a lesser key stays the holder; under a worse
    /// value or a greater key it has left. Answers [`KeptFolds::waits`].
    pub fn change(
        &self,
        folds: &mut KeptFolds,
        was: Member<'_>,
        is: Member<'_>,
        settings: Settings<'_>,
    ) -> bool {
        self.moved(folds, Some(was), Some(is), settings)
    }

    fn moved(
        &self,
        folds: &mut KeptFolds,
        was: Option<Member<'_>>,
        is: Option<Member<'_>>,
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
            let read = |fields: &Row| {
                let now = Timestamp(0);
                let scope = RecordScope {
                    fields,
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
