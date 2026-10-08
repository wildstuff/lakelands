//! The closed set of ways a call can fail.
//!
//! Every error is an outcome phrased for a reader who is not an engineer,
//! and the set is closed: no string-typed errors, and no engine's own
//! codes reach a caller. A proposal is not an error; it is an
//! [`Outcome`](crate::write::Outcome).
//!
//! On the wire an error carries `message`, its sentence, beside its data;
//! an error of a kind this build does not know is read as
//! [`Error::Unknown`] and said by that sentence.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::{FieldName, Position, ProposalId, RecordKey, TypeName, VerbName, Version};
use crate::write::MigrationReport;

mod outage;

pub use outage::{
    Act, Added, Aimless, Bound, Closed, Detail, Doing, Exchange, Flaw, Left, Part, Remote, Retry,
    Seam, Situation, Tls, Unknown, closed,
};

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", tag = "error", rename_all = "snake_case")]
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
        /// The seam.
        #[serde(with = "closed")]
        seam: Box<Seam>,
    },
    /// The lake cannot answer now: another process holds its writer
    /// lease, it is opened read-only, or its storage failed. Retrying
    /// may help; the situation says what the operator can do.
    Unavailable {
        /// What happened.
        #[serde(with = "closed")]
        situation: Box<Situation>,
    },
    /// An error of a kind this build does not know: a newer lake's.
    #[serde(skip)]
    Unknown(Unknown),
}

/// Its derived serde form.
struct Plain(Error);

impl<'de> Deserialize<'de> for Plain {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Plain, D::Error> {
        Error::deserialize(d).map(Plain)
    }
}

impl Serialize for Plain {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Error::serialize(&self.0, s)
    }
}

/// Its data, and `message`, its sentence.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        closed::serialize(&Plain(self.clone()), s)
    }
}

/// An error of a kind this build does not know is [`Error::Unknown`].
impl<'de> Deserialize<'de> for Error {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Error, D::Error> {
        closed::deserialize(d).map(|Plain(e)| e)
    }
}

impl fmt::Display for Plain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Closed for Plain {
    const TAG: &'static str = "error";

    fn unknown(unknown: Unknown) -> Self {
        Plain(Error::Unknown(unknown))
    }

    fn as_unknown(&self) -> Option<&Unknown> {
        match &self.0 {
            Error::Unknown(u) => Some(u),
            _ => None,
        }
    }
}

impl Error {
    /// The lake cannot answer now, for this reason.
    pub fn unavailable(situation: Situation) -> Error {
        let situation = Box::new(situation);
        Error::Unavailable { situation }
    }

    /// Why the lake cannot answer now, if that is the error.
    pub fn situation(&self) -> Option<&Situation> {
        match self {
            Error::Unavailable { situation } => Some(situation),
            _ => None,
        }
    }

    /// The seam the call needs, if that is the error.
    pub fn seam(&self) -> Option<&Seam> {
        match self {
            Error::Unbound { seam } => Some(seam),
            _ => None,
        }
    }

    /// The call needs this seam, and the host has not bound it.
    pub fn unbound(seam: Seam) -> Error {
        let seam = Box::new(seam);
        Error::Unbound { seam }
    }

    /// The sentence of what went wrong, without the words that say which
    /// error it is: what a failure is said in when it is the cause of
    /// another, or a field's provenance.
    pub fn said(&self) -> String {
        match self {
            Error::Unavailable { situation } => situation.to_string(),
            Error::Refused {
                reason: Refusal::Invalid { message } | Refusal::Constraint { message },
                ..
            } => message.clone(),
            other => other.to_string(),
        }
    }
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
    /// A retry of a call of a verb that reaches outside the lake, or of
    /// its acceptance, found no answer of its executor recorded: the call
    /// is in the log, and a retry runs nothing. The message names its
    /// position and what to do.
    Unanswered {
        /// What is in the log, and what to do, for a person.
        message: String,
    },
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
    /// A function.
    Function {
        /// Its name.
        name: String,
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
                    | Refusal::Precondition { message }
                    | Refusal::Unanswered { message } => write!(f, ": {message}"),
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
                    // The type is not named to one who may not see it.
                    Refusal::Migration { report } if report.ty.as_str().is_empty() => {
                        write!(f, ": the migration would lose information")
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
                Missing::Function { name } => write!(f, "there is no function `{name}`"),
                Missing::Proposal { id } => write!(f, "there is no proposal `{id}`"),
            },
            Error::Conflict { expected, actual } => write!(
                f,
                "the record changed: expected version {expected}, found {actual}"
            ),
            Error::Unbound { seam } => write!(f, "nothing is bound for {seam}"),
            Error::Unavailable { situation } => write!(f, "the lake is unavailable: {situation}"),
            Error::Unknown(u) => f.write_str(u.message()),
        }
    }
}

impl std::error::Error for Error {}

/// The result of every fallible call in the contract.
pub type Result<T, E = Error> = std::result::Result<T, E>;
