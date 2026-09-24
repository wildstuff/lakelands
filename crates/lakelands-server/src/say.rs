//! What a door says when something goes wrong: a sentence on what
//! happened, and one on what to do. The CLI prints it; the MCP server
//! returns it as a tool error's text. One mapping, so the two doors
//! never say different things about the same error.

use std::fmt;

use lakelands_api::error::{Error, Refusal};

/// A failure, for a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What went wrong.
    pub what: String,
    /// What to do about it.
    pub todo: String,
}

impl Failure {
    /// A failure and the advice that goes with it.
    pub fn new(what: impl Into<String>, todo: impl Into<String>) -> Failure {
        Failure {
            what: what.into(),
            todo: todo.into(),
        }
    }

    /// The same failure, said to be about something in particular.
    pub fn about(mut self, context: impl fmt::Display) -> Failure {
        self.what = format!("{context}: {}", self.what);
        self
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "error: {}", self.what)?;
        writeln!(f, "  what to do: {}", self.todo)
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        let todo = match &e {
            Error::Refused { reason, .. } => match reason {
                Refusal::NotGranted => {
                    "act as a principal whose grants allow it, or ask the lake's owner for a grant"
                        .to_owned()
                }
                Refusal::WrongDoor => {
                    "source fields arrive through `lakelands ingest`; authored fields through writes and verbs"
                        .to_owned()
                }
                Refusal::Reserved => "choose a name that does not begin with `sys.`".to_owned(),
                Refusal::Invalid { .. } | Refusal::Constraint { .. } => {
                    "correct the input and run the command again; nothing was written".to_owned()
                }
                Refusal::Precondition { .. } => {
                    "change the record so the verb applies, then call it again".to_owned()
                }
                Refusal::TooDeep { .. } => {
                    "break the chain of reactions, or raise the lake's limit in its location".to_owned()
                }
                Refusal::Migration { .. } => {
                    "fix the records the report names, then run the migration again".to_owned()
                }
                Refusal::Decided => "list the proposals to see how it was decided".to_owned(),
                Refusal::OutOfRange { head } => format!("ask for a position at or before {head}"),
            },
            Error::NotFound { .. } => {
                "check the name, or apply the declaration that introduces it first".to_owned()
            }
            Error::Conflict { .. } => {
                "read the record again and retry against its current version".to_owned()
            }
            Error::Unbound { .. } => "bind it in the host's configuration".to_owned(),
            Error::Unavailable { .. } => {
                "wait and retry, or check that no other process holds the lake".to_owned()
            }
        };
        Failure {
            what: e.to_string(),
            todo,
        }
    }
}
