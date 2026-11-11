//! `judge`: what a type's rules say of records, stored or not yet sent,
//! answered with the refusal a write gives and nothing appended
//! (`docs/decisions/113-a-judge-answers-what-rules-say-of-records.md`).
//!
//! A stored record is judged at the read's clock; the parts of a rule
//! only a write decides are named, not judged. A candidate is judged as
//! the ingest window judges it. A part the evaluator cannot answer is
//! named with its record, never read as a pass.

use serde::{Deserialize, Serialize};

use crate::error::{Case, Refusal};
use crate::expr::Condition;
use crate::id::{ConstraintName, RecordKey};
use crate::read::ReadAt;
use crate::record::SourceRecord;

/// A judgement asked of one type's rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Judge {
    /// The records judged: stored, or not yet sent.
    pub subject: Subject,
    /// The rules judged.
    pub rules: Rules,
    /// The read's axes and clock; a stored record is read and judged at
    /// them, a candidate at the store's clock.
    #[serde(default)]
    pub at: ReadAt,
}

/// The records a judge judges, of one kind per call; keys and candidates
/// together are at most the lake's `max_limit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// Stored records of the type, by key; a key it does not hold is
    /// refused `not_found`.
    Keys(Vec<RecordKey>),
    /// Records not stored, judged as the ingest window judges them; each
    /// is of the call's type.
    Candidates(Vec<SourceRecord>),
}

/// The rules a judge judges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rules {
    /// The type's constraints the caller is shown, or those it names; a
    /// name not among them is refused `not_found`.
    Declared {
        /// The constraints, by name; absent, every one shown.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        names: Option<Vec<ConstraintName>>,
    },
    /// One condition the caller is considering, checked where a
    /// constraint is and answered under the name it gives; nothing is
    /// declared.
    Proposed {
        /// The name the answer gives it.
        name: ConstraintName,
        /// The condition.
        holds: Condition,
    },
}

/// What the rules say: every record in the subject's order, and every
/// rule not judged whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeReport {
    /// One per record judged, in the order sent.
    pub records: Vec<JudgedRecord>,
    /// The rules not judged whole, once per rule and reason. A rule may
    /// be here and break a record at once: read the two together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unjudged: Vec<Unjudged>,
}

/// What the rules say of one record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgedRecord {
    /// Its place in the subject, from 0.
    pub index: u32,
    /// Its key.
    pub key: RecordKey,
    /// Every refusal a write of it would give in the part judged: a
    /// broken rule's, or for a candidate the refusal that stops a write
    /// before its rules. Empty, it breaks nothing judged.
    #[serde(with = "crate::error::closed::list")]
    pub refusals: Vec<Refusal>,
}

/// A rule not judged whole, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unjudged {
    /// The rule.
    pub constraint: ConstraintName,
    /// Why.
    pub why: UnjudgedWhy,
}

/// Why a rule was not judged whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnjudgedWhy {
    /// Over stored records it reads what only the write that wrote them
    /// decided — `preceding()`, `previous()`, `changed()`, `retired()` or
    /// `created()` — and no read judges that part; the same for every
    /// record of the call.
    TheWriteAlone,
    /// The evaluator answered a case over the record at `index`.
    Unevaluable {
        /// The record's place in the subject.
        index: u32,
        /// What the evaluator answered.
        #[serde(with = "crate::error::closed")]
        case: Case,
    },
}
