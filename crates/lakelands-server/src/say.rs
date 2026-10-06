//! What a door says when something goes wrong: a sentence on what
//! happened, and one on what to do. The CLI prints it; the MCP server
//! returns it as a tool error's text. One mapping, so the two doors
//! never say different things about the same error.

use std::fmt;

use lakelands_api::error::{Error, Refusal, THE_EXECUTOR};

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
            Error::Refused { field, reason } => match reason {
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
                Refusal::TooLarge { .. } if field.as_ref().is_some_and(|f| f == "offset") => {
                    "page on with the cursor from a nearer page, or raise the lake's `max_offset` \
                     in its location"
                        .to_owned()
                }
                Refusal::TooLarge { .. } if field.is_some() => {
                    "declare the set with a narrower filter and widen it in steps, after the \
                     migration if one was refused (`find` with the filter and `total` counts a \
                     step); or raise the lake's `max_set_changes` in its location"
                        .to_owned()
                }
                Refusal::TooLarge { .. } => {
                    "start from fewer records, or raise the lake's `max_graph_nodes` in its location"
                        .to_owned()
                }
                Refusal::Migration { .. } => {
                    "fix the records the report names, then run the migration again".to_owned()
                }
                Refusal::Decided => "list the proposals to see how it was decided".to_owned(),
                // Waiting helps only a call whose executor still runs.
                Refusal::Unanswered { .. } => {
                    "a retry under this key runs nothing: if the call's executor is still \
                     running, retry once it has answered; if the first answer said its executor \
                     was not handed the call, call the verb again under another key or with \
                     none, or propose the call again"
                        .to_owned()
                }
                Refusal::OutOfRange { head } => format!("ask for a position at or before {head}"),
            },
            Error::NotFound { .. } => {
                "check the name, or apply the declaration that introduces it first".to_owned()
            }
            Error::Conflict { .. } => {
                "read the record again and retry against its current version".to_owned()
            }
            // The caller may not be the operator: say that nothing was
            // written, and who can make it run.
            Error::Unbound { .. } => {
                "nothing was written; this cannot run until the lake's operator binds it in the host's configuration"
                    .to_owned()
            }
            // A call of an external verb that failed around its hand-out:
            // sent again as it was, it could run twice, or run nothing.
            Error::Unavailable { message } if message.contains(THE_EXECUTOR) => {
                "the call is in the log: do what the message says, and call the verb again only \
                 where it says the executor was not handed the call"
                    .to_owned()
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A call of an external verb that failed around its hand-out, in the
    /// store's words (`writes::not_handed`, the recording's failure), is
    /// not advised to retry; another `unavailable` is.
    #[test]
    fn a_call_that_failed_around_its_hand_out_is_not_advised_to_retry() {
        let executor = THE_EXECUTOR;
        for said in [
            format!("the commit failed; {executor} was not handed this call: call the verb again"),
            format!("{executor} answered this call with null, and recording its answer failed: x"),
        ] {
            let todo = Failure::from(Error::Unavailable { message: said }).todo;
            assert!(!todo.contains("retry"), "{todo}");
        }
        let other = Failure::from(Error::Unavailable {
            message: "held".into(),
        });
        assert!(other.todo.starts_with("wait and retry"), "{}", other.todo);
    }
}
