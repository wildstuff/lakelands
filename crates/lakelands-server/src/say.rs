//! What a door says when something goes wrong: a sentence on what
//! happened, and one on what to do. The CLI prints it; the MCP server
//! returns it as a tool error's text, REST beside the error's JSON. One
//! mapping, so the doors never say different things about the same
//! error: the advice goes by the kind of error, and by its case where the
//! case has advice of its own ([`advice`]).

use std::fmt;

use lakelands_api::error::{Case, Error, Refusal, Said, Situation};
use lakelands_api::id::RecordKey;
use lakelands_api::write::Plan;

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

    /// A refusal's case, said as it is, with its advice.
    pub fn of(said: &Said) -> Failure {
        let todo = advice(&said.case).unwrap_or_else(|| CORRECT.to_owned());
        Failure::new(said.to_string(), todo)
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

/// What to do about an input that does not fit.
const CORRECT: &str = "correct the input and run the command again; nothing was written";

/// What to do about a case that has advice of its own: a door's request
/// that does not fit it, or an export into a directory that holds
/// something.
fn advice(case: &Case) -> Option<String> {
    let preflight =
        "run the preflight at …/schema/migrate, then send its report to …/schema/migrate/apply";
    let line = "correct that line and send the body again; what came before it is committed, and sending it again appends nothing";
    let todo = match case {
        Case::NoArgument { .. } => {
            "use only the arguments the tool's input schema lists, then call it again"
        }
        Case::NoMember { .. } => {
            "remove the member or correct its name, then call again; nothing was declared or written"
        }
        Case::MemberMissing { .. } | Case::ArgumentsUnfit { .. } => {
            "correct them against the tool's input schema (over REST, the route's in docs/rest.md) and call it again"
        }
        Case::NoSessionToPin { .. } => "name `as_of` on each read instead",
        Case::NoSession { .. } => "read the change feed instead",
        Case::QueryNotEncoded => "encode the query's parameters and send the request again",
        Case::NoParameter { .. } => "use only the parameters the route takes",
        Case::ParameterForm { .. } => "give the parameter in the form it takes",
        Case::ParameterTwice { .. } => "give each parameter once",
        Case::RouteMethod { .. } => "send it with a method the route takes",
        Case::NoRoute => "see the routes in docs/rest.md; MCP is served at /lakes/<id>/mcp",
        Case::IfMatchBoth => {
            "give `If-Match: <version>`, or `If-None-Match: *` for a record that must not exist yet"
        }
        Case::IfMatchNoVersion { .. } => {
            "give the record's version as its `ETag` answered it; a weak `W/` one is not taken"
        }
        Case::IfNoneMatchNotPut => "give `If-Match: <version>` instead",
        Case::IfNoneMatchValue { .. } => {
            "give `If-None-Match: *` for a record that must not exist yet"
        }
        Case::VerbTypeHeader { .. } => "send it without the header",
        Case::NoBody { .. } => "send it without one",
        Case::BodyObjectOf { argument, .. } => {
            return Some(format!("send the {argument} as one JSON object"));
        }
        Case::BodyNotObject { .. } => "send the arguments as one JSON object",
        Case::PathNames { argument } => return Some(format!("leave `{argument}` out of the body")),
        Case::PreflightReportId | Case::ApplyNeedsReportId => preflight,
        Case::IngestMediaType => "send one record per line as NDJSON",
        Case::LineTooLong { .. } | Case::LineNotRecord { .. } | Case::BodyCutOff { .. } => line,
        Case::BodyMediaType | Case::BodyNotJson { .. } => "send the body as JSON",
        Case::BodyTooLarge => "send a smaller body; many records go through ingest",
        Case::ExportNotEmpty => "export into an empty or new directory",
        _ => return None,
    };
    Some(todo.to_owned())
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        // A refused record of an ingest: what to do about its refusal, and
        // where to send the stream again from (lakelands#1043).
        if let Error::Ingested { from, error, .. } = &e {
            let inner = Failure::from((**error).clone());
            let todo = format!(
                "{}; then send the stream again from record {from}",
                inner.todo
            );
            return Failure {
                what: e.to_string(),
                todo,
            };
        }
        if let Some(todo) = e.case().and_then(|said| advice(&said.case)) {
            return Failure {
                what: e.to_string(),
                todo,
            };
        }
        let todo = match &e {
            Error::Refused { field, reason } => match reason {
                Refusal::NotGranted => {
                    "act as a principal whose grants allow it, or ask the lake's owner for a grant"
                        .to_owned()
                }
                Refusal::GrantNeeded { grant } => format!(
                    "act as a principal holding `{grant}`, or propose the change for one to accept"
                ),
                Refusal::WrongDoor if field.is_none() => {
                    "another door does it: a relation's authored links through `link` and `unlink`, \
                     a source's links through `lakelands ingest`, a derived relation's by the lake \
                     alone, and a record only its type's verbs retire through one of those verbs"
                        .to_owned()
                }
                Refusal::WrongDoor => {
                    "source fields arrive through `lakelands ingest`; authored fields through writes and verbs, or from a source they admit"
                        .to_owned()
                }
                Refusal::VerbsOnly { .. } => {
                    "call one of the verbs it names, which does it under its own condition"
                        .to_owned()
                }
                Refusal::Reserved => "choose a name that does not begin with `sys.`; a verb names no \
                                      `sys.` type but a relation's link type"
                    .to_owned(),
                Refusal::Invalid(_) | Refusal::Constraint(_) => CORRECT.to_owned(),
                Refusal::Precondition(_) => {
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
                // A retry answers the same: the reason is the host's.
                Refusal::Declined(_) => {
                    "the host declined this call and a retry under the same idempotency key answers the same; ask the host's \
                     operator for its reason, or change the call"
                        .to_owned()
                }
                // Waiting helps only a call whose executor still runs.
                Refusal::Unanswered(_) => {
                    "a retry under this key runs nothing: if the call's executor is still \
                     running, retry once it has answered; if the first answer said its executor \
                     was not handed the call, call the verb again under another key or with \
                     none, or propose the call again"
                        .to_owned()
                }
                Refusal::OutOfRange { head } => format!("ask for a position at or before {head}"),
                Refusal::Unknown(_) => "do what the message says".to_owned(),
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
            Error::Unavailable { situation }
                if matches!(
                    **situation,
                    Situation::Executor { .. }
                        | Situation::AnswerHeld { .. }
                        | Situation::AnswerLost { .. }
                ) =>
            {
                "the call is in the log: do what the message says, and call the verb again only \
                 where it says the executor was not handed the call"
                    .to_owned()
            }
            // Waiting mends no damage. A projection is made anew from
            // the log; what the log lost comes back only from a backup.
            Error::Unavailable { situation }
                if matches!(**situation, Situation::Damaged { .. }) =>
            {
                "restore the lake from a backup; where the projection alone is damaged, \
                 `lakelands rebuild` makes it anew from the log"
                    .to_owned()
            }
            Error::Unavailable { .. } => {
                "wait and retry, or check that no other process holds the lake".to_owned()
            }
            Error::Unknown(_) => "do what the message says".to_owned(),
            Error::Ingested { .. } => unreachable!("a refused record is said above"),
        };
        Failure {
            what: e.to_string(),
            todo,
        }
    }
}

/// What a change's plan says, for a person who decides on it: a sentence
/// per finding, each standing alone, its count first and then the first
/// keys its reader is answered
/// (`docs/decisions/062-a-change-plans-before-it-acts.md`). The CLI prints
/// them, and MCP and REST send them beside the plan as `said`; they are no
/// part of the plan or its id.
pub fn plan(plan: &Plan) -> Vec<String> {
    let mut said = Vec::new();
    for t in &plan.types {
        let ty = &t.ty;
        match t.recomputed {
            Some(0) => {}
            Some(n) => said.push(format!("{} of `{ty}` would be computed again.", records(n))),
            None => said.push(format!("It computes every record of `{ty}` again.")),
        }
        for u in &t.unlive {
            said.push(format!(
                "{} of `{ty}` would be left with `{}` naming no live record{}",
                records(u.records),
                u.field,
                first(&u.sample_keys, u.records)
            ));
        }
        for b in &t.breaches {
            said.push(format!(
                "{} of `{}` would break the constraint `{}`{}",
                records(b.records),
                b.ty,
                b.constraint,
                first(&b.sample_keys, b.records)
            ));
        }
        for c in t.fields.iter().filter(|c| c.not_convertible > 0) {
            said.push(format!(
                "{} of `{ty}`.`{}` would not convert to its new type{}",
                values(c.not_convertible),
                c.field,
                first(&c.sample_keys, c.not_convertible)
            ));
        }
        for (field, n) in t.dropped.iter().filter(|(_, n)| *n > 0) {
            said.push(format!(
                "{} of `{ty}`.`{field}` would be dropped.",
                values(*n)
            ));
        }
        for f in t.filled.iter().filter(|f| f.records > 0) {
            said.push(format!(
                "{} of `{ty}` would take `{}`'s initial value{}",
                records(f.records),
                f.field,
                first(&f.sample_keys, f.records)
            ));
        }
        for r in t.rekeyed.iter().filter(|r| r.records > 0) {
            said.push(format!(
                "{} of `{}` would take a new key.",
                records(r.records),
                r.ty
            ));
            if r.collisions > 0 {
                let n = records(r.collisions);
                said.push(format!("{n} of `{}` would take a key another holds.", r.ty));
            }
        }
    }
    for v in plan.value_types.iter().filter(|v| v.misfits > 0) {
        let keys: Vec<RecordKey> = v.first.iter().map(|m| m.key.clone()).collect();
        said.push(format!(
            "{} of the value type `{}` would no longer fit{}",
            values(v.misfits),
            v.value_type,
            first(&keys, v.misfits)
        ));
    }
    if said.is_empty() {
        said.push("The change would leave every stored record as it is.".to_owned());
    }
    if !plan.lossless {
        said.push(
            "It cannot be made as planned: it would lose values or break a constraint that refuses."
                .to_owned(),
        );
    }
    said
}

/// `n` records, said.
fn records(n: u64) -> String {
    match n {
        1 => "1 record".to_owned(),
        n => format!("{n} records"),
    }
}

/// `n` values, said.
fn values(n: u64) -> String {
    match n {
        1 => "1 value".to_owned(),
        n => format!("{n} values"),
    }
}

/// The end of a sentence that counts `n`: the first keys, and whether
/// there are more.
fn first(keys: &[RecordKey], n: u64) -> String {
    if keys.is_empty() {
        return ".".to_owned();
    }
    let named: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
    let more = if n > keys.len() as u64 {
        " and more"
    } else {
        ""
    };
    format!(": {}{more}.", named.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call of an external verb that failed around its hand-out
    /// (`writes::not_handed`, the recording's failure) is not advised to
    /// retry; another `unavailable` is.
    #[test]
    fn a_call_that_failed_around_its_hand_out_is_not_advised_to_retry() {
        use lakelands_api::error::Retry;
        let cause = || Box::new(Error::unavailable(Situation::Moved));
        let (answered, retry) = (serde_json::Value::Null, Retry::Call);
        for situation in [
            Situation::Executor {
                retry: retry.clone(),
                cause: cause(),
            },
            Situation::AnswerHeld {
                answered: answered.clone(),
                retry: retry.resent(),
                cause: cause(),
            },
            Situation::AnswerLost {
                answered,
                cause: cause(),
            },
        ] {
            let todo = Failure::from(Error::unavailable(situation)).todo;
            assert!(!todo.contains("retry"), "{todo}");
        }
        let other = Failure::from(Error::unavailable(Situation::Moved));
        assert!(other.todo.starts_with("wait and retry"), "{}", other.todo);
    }

    /// A damaged lake is not mended by waiting: the hint names a restore.
    #[test]
    fn a_damaged_lake_is_advised_a_restore_not_a_retry() {
        use lakelands_api::error::{Flaw, Part};
        let (part, position, flaw) = (Part::Log, None, Flaw::NoSegmentFile);
        let damaged = Situation::Damaged {
            part,
            position,
            flaw,
        };
        let todo = Failure::from(Error::unavailable(damaged)).todo;
        assert!(
            todo.starts_with("restore") && !todo.contains("retry"),
            "{todo}"
        );
    }
}
