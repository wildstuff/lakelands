//! The closed set of ways a call can fail.
//!
//! Every error is an outcome phrased for a reader who is not an engineer,
//! and the set is closed: no string-typed errors, and no engine's own
//! codes reach a caller. A proposal is not an error; it is an
//! [`Outcome`](crate::write::Outcome).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{FieldName, Position, ProposalId, RecordKey, TypeName, VerbName, Version};
use crate::write::MigrationReport;

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum Error {
    /// The store would not do it: a right is missing, a declaration
    /// forbids it, or the request does not fit the lake's map. Nothing
    /// was written.
    Refused {
        /// The field it concerns, if one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<FieldName>,
        /// Why.
        reason: Refusal,
    },
    /// Something named does not exist at the position read.
    NotFound {
        /// What was looked for.
        missing: Missing,
    },
    /// The record moved: it is not at the version the write expected.
    Conflict {
        /// The version the write expected.
        expected: Version,
        /// The version found (0: no such record).
        actual: Version,
    },
    /// The call needs a seam the host has not bound (a verb's executor,
    /// an embedding backend, a function backing).
    Unbound {
        /// The seam, and the name it was asked for.
        seam: String,
    },
    /// The lake cannot answer now: another process holds its writer
    /// lease, it is opened read-only, or its storage failed. Retrying
    /// may help; the message says what the operator can do.
    Unavailable {
        /// What happened, for a person.
        message: String,
    },
}

/// Why a call was refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    /// The principal holds no grant for it.
    NotGranted,
    /// The field is written by another authority, through another door
    /// (a source field through `upsert`, a derived field by anyone).
    WrongDoor,
    /// The name is in a reserved namespace: `sys.` for a type,
    /// `sys.` and `lakelands.` for an annotation key.
    Reserved,
    /// The request does not fit the lake's map: an unknown field, a
    /// relation the type does not declare, an expression that does not
    /// parse or does not type.
    Invalid {
        /// What does not fit, for a person.
        message: String,
    },
    /// A value breaks a declared constraint or a required field is
    /// missing.
    Constraint {
        /// What is broken, for a person.
        message: String,
    },
    /// A verb's `requires` does not hold for the record.
    Precondition {
        /// What does not hold, for a person.
        message: String,
    },
    /// A chain of reactive writes went deeper than the lake allows.
    TooDeep {
        /// The lake's ceiling.
        max: u32,
    },
    /// A whole-graph measure would reach more records than the lake
    /// allows, a `find` would skip more rows than it allows (the refusal
    /// then names `offset`), or a set's declaration or a migration would
    /// change more memberships than it allows (the refusal names the set);
    /// nothing partial is answered or appended.
    TooLarge {
        /// The lake's `max_graph_nodes`, `max_offset` or `max_set_changes`.
        max: u32,
    },
    /// A migration would lose information; the report says where.
    Migration {
        /// The preflight report.
        report: Box<MigrationReport>,
    },
    /// The proposal is already decided.
    Decided,
    /// The position asked for is not in the lake's log.
    OutOfRange {
        /// The head of the log.
        head: Position,
    },
}

/// What was not found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Missing {
    /// A type.
    Type {
        /// Its name.
        name: TypeName,
    },
    /// A record.
    Record {
        /// Its type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// Its key.
        key: RecordKey,
    },
    /// A verb.
    Verb {
        /// Its name.
        name: VerbName,
    },
    /// A proposal.
    Proposal {
        /// Its identifier.
        id: ProposalId,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused { field, reason } => {
                write!(f, "refused")?;
                if let Some(field) = field {
                    write!(f, " on `{field}`")?;
                }
                match reason {
                    Refusal::NotGranted => write!(f, ": you hold no grant for this"),
                    Refusal::WrongDoor => write!(f, ": another authority writes this field"),
                    Refusal::Reserved => write!(f, ": the name is in a reserved namespace"),
                    Refusal::Invalid { message }
                    | Refusal::Constraint { message }
                    | Refusal::Precondition { message } => write!(f, ": {message}"),
                    Refusal::TooDeep { max } => {
                        write!(f, ": reactions may go {max} levels deep, no deeper")
                    }
                    Refusal::TooLarge { max } if field.as_ref().is_some_and(|f| f == "offset") => {
                        write!(f, ": a page may skip {max} rows, no more")
                    }
                    Refusal::TooLarge { max } if field.is_some() => {
                        write!(f, ": more than {max} of the set's memberships would change")
                    }
                    Refusal::TooLarge { max } => {
                        write!(f, ": a graph measure may reach {max} records, no more")
                    }
                    Refusal::Migration { report } => write!(
                        f,
                        ": the migration of `{}` would lose information",
                        report.ty
                    ),
                    Refusal::Decided => write!(f, ": the proposal is already decided"),
                    Refusal::OutOfRange { head } => {
                        write!(f, ": the lake's log ends at position {head}")
                    }
                }
            }
            Error::NotFound { missing } => match missing {
                Missing::Type { name } => write!(f, "there is no type `{name}`"),
                Missing::Record { ty, key } => write!(f, "there is no `{ty}` with key `{key}`"),
                Missing::Verb { name } => write!(f, "there is no verb `{name}`"),
                Missing::Proposal { id } => write!(f, "there is no proposal `{id}`"),
            },
            Error::Conflict { expected, actual } => write!(
                f,
                "the record changed: expected version {expected}, found {actual}"
            ),
            Error::Unbound { seam } => write!(f, "nothing is bound for {seam}"),
            Error::Unavailable { message } => write!(f, "the lake is unavailable: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// The result of every fallible call in the contract.
pub type Result<T, E = Error> = std::result::Result<T, E>;
