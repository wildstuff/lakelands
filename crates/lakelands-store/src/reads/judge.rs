//! `judge`: what a type's rules say of records, answered with the refusal
//! a write gives and nothing appended
//! (`docs/decisions/113-a-judge-answers-what-rules-say-of-records.md`).
//! One evaluator serves both subjects: a rule is split into its
//! conjuncts, and each is judged over the scope the subject gives — a
//! stored record's read, a candidate's write.

use lakelands_api::error::Place;
use lakelands_api::judge::{JudgeReport, JudgedRecord, Rules, Unjudged, UnjudgedWhy};
use lakelands_api::ontology::{ConstraintDecl, TypeDecl};
use lakelands_core::expr::Scope;

use super::*;

/// What over a stored record only the write that wrote it decided.
const THE_WRITE_ALONE: [Func; 5] = [
    Func::Preceding,
    Func::Previous,
    Func::Changed,
    Func::Retired,
    Func::Created,
];

/// A rule judged: declared, or proposed as if it were, and typed.
pub(crate) struct Rule {
    pub decl: ConstraintDecl,
    pub holds: Expr,
}

/// The rules `rules` names over `decl`, as `sight`'s holder may judge
/// them: the constraints `describe_type` shows it, or one it proposes,
/// checked where a constraint is and probed as a read's filter is.
pub(crate) fn rules(sight: &Sight, decl: &TypeDecl, rules: &Rules) -> Result<Vec<Rule>> {
    match rules {
        Rules::Declared { names } => {
            let shown = (sight.describe_type(&decl.name)).map(|t| t.constraints);
            let shown = shown.unwrap_or_default();
            let chosen: Vec<_> = match names {
                None => shown.iter().collect(),
                Some(names) => (names.iter())
                    .map(|name| {
                        let found = shown.iter().find(|c| &c.name == name);
                        found.ok_or_else(|| Error::NotFound {
                            missing: Missing::Constraint { name: name.clone() },
                        })
                    })
                    .collect::<Result<_>>()?,
            };
            (chosen.into_iter())
                .map(|c| {
                    let holds = crate::declare::typed(&c.holds, Eval::Constraint);
                    let holds = holds.map_err(Error::invalid)?;
                    Ok(Rule {
                        decl: c.clone(),
                        holds,
                    })
                })
                .collect()
        }
        Rules::Proposed { name, holds: asked } => {
            let holds = condition(asked, Eval::Constraint, &[Place::Constraint])?;
            // A relation `exists()` names is no field: what it reaches is
            // the evaluator's to answer, named with each record.
            let link =
                |f: &&FieldName| decl.relations.iter().any(|r| r.name.as_str() == f.as_str());
            let read = holds.fields_read().into_iter().filter(|f| !link(f));
            sight.check_probe(decl, read)?;
            let decl = ConstraintDecl {
                name: name.clone(),
                holds: asked.clone(),
                description: None,
                at_ingest: Default::default(),
            };
            Ok(vec![Rule { decl, holds }])
        }
    }
}

/// A judge of `n` records, refused whole past the lake's `max_limit`.
pub(crate) fn bounded(lake: &Lake, n: usize) -> Result<()> {
    let max = lake.limits.max_limit;
    match n > max as usize {
        true => Err(Error::refused(
            Some("records".into()),
            Refusal::TooLarge { max },
        )),
        false => Ok(()),
    }
}

impl Rule {
    /// The refusal a write gives `ty` `key` for breaking this rule.
    fn broken(&self, ty: &TypeName, key: &RecordKey) -> Refusal {
        let case = Case::ConstraintBroken {
            constraint: self.decl.name.clone(),
            ty: ty.clone(),
            key: key.clone(),
        };
        Refusal::Constraint(Box::new(case.into()))
    }
}

impl Judging {
    /// The record at `index`, `ty` `key`, judged over `scope` by each
    /// rule `chosen` keeps, conjunct by conjunct: over a stored record
    /// (`stored`) a
    /// conjunct only its write decided is named, not judged; one the
    /// evaluator does not answer is named with the record, never read as
    /// a pass. A rule is named once per reason.
    pub(crate) fn judge(
        &mut self,
        (index, ty, key): (u32, &TypeName, &RecordKey),
        scope: &dyn Scope,
        stored: bool,
        chosen: impl Fn(&Rule) -> bool,
    ) -> bool {
        let mut refusals = Vec::new();
        for rule in self.rules.iter().filter(|r| chosen(r)) {
            let mut broke = false;
            for part in rule.holds.conjuncts() {
                let why = match stored && part.calls(&THE_WRITE_ALONE) {
                    true => UnjudgedWhy::TheWriteAlone,
                    false => match expr::holds(part, scope) {
                        Ok(holds) => {
                            broke |= !holds;
                            continue;
                        }
                        Err(case) => UnjudgedWhy::Unevaluable { index, case },
                    },
                };
                let unjudged = &mut self.report.unjudged;
                let named = |u: &Unjudged| u.constraint == rule.decl.name && u.why == why;
                if !unjudged.iter().any(named) {
                    let constraint = rule.decl.name.clone();
                    unjudged.push(Unjudged { constraint, why });
                }
            }
            if broke {
                refusals.push(rule.broken(ty, key));
            }
        }
        let broke = !refusals.is_empty();
        let key = key.clone();
        self.report.records.push(JudgedRecord {
            index,
            key,
            refusals,
        });
        broke
    }

    /// The record at `index` answered without its rules: by the refusal
    /// that stops its write before them, or, by none, as a write that
    /// would leave it as it stands.
    pub(crate) fn stopped(&mut self, index: u32, key: &RecordKey, reason: Option<Refusal>) {
        let key = key.clone();
        let refusals = reason.into_iter().collect();
        self.report.records.push(JudgedRecord {
            index,
            key,
            refusals,
        });
    }

    /// The last record judged, refused past its rules by `reason`.
    pub(crate) fn after(&mut self, reason: Refusal) {
        if let Some(last) = self.report.records.last_mut() {
            last.refusals.push(reason);
        }
    }
}

/// A judge under way: its rules and what they said so far.
pub(crate) struct Judging {
    pub rules: Vec<Rule>,
    pub report: JudgeReport,
}

impl Judging {
    /// A judge of `rules`, nothing said yet.
    pub(crate) fn of(rules: Vec<Rule>) -> Judging {
        Judging {
            rules,
            report: JudgeReport {
                records: Vec::new(),
                unjudged: Vec::new(),
            },
        }
    }
}

impl Reader<'_> {
    /// `judge` of stored `keys` of `ty`, read and judged at this read's
    /// clock; a key the type does not hold is refused as `get` refuses it.
    pub fn judge(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        asked: &Rules,
        at: &ReadAt,
    ) -> Result<JudgeReport> {
        bounded(self.lake, keys.len())?;
        let decl = self.named_type(ty)?.clone();
        let mut judging = Judging::of(rules(self.sight, &decl, asked)?);
        let records = (keys.iter())
            .map(|key| {
                let get = Get {
                    key: key.clone(),
                    with_meta: true,
                    params: Default::default(),
                    at: *at,
                };
                self.get(ty, &get)
            })
            .collect::<Result<Vec<_>>>()?;
        for (i, r) in records.iter().enumerate() {
            let scope = RecordScope {
                fields: &r.fields,
                meta: Some(&r.meta),
                settings: Some(self.told()),
                now: self.now,
            };
            judging.judge((i as u32, ty, &r.key), &scope, true, |_| true);
        }
        Ok(judging.report)
    }
}
