use serde_json::{Value, json};

use super::*;
use crate::error::Error;

/// A case is written as its code, its parameters and its sentence, and
/// read back as it was; a nested case too.
#[test]
fn a_case_is_written_with_its_sentence_and_read_back() {
    let flaw = Flaw::Position {
        expected: Position(2),
        found: Some(Position(3)),
    };
    let e = Error::unavailable(Situation::Damaged {
        part: Part::Log,
        position: Some(Position(2)),
        flaw,
    });
    let wire = serde_json::to_value(&e).unwrap();
    assert_eq!(wire["situation"]["code"], "damaged");
    assert_eq!(wire["situation"]["flaw"]["code"], "position");
    assert_eq!(
        wire["situation"]["message"],
        "the log is damaged at position 2: position 3 stands where position 2 belongs"
    );
    assert_eq!(serde_json::from_value::<Error>(wire).unwrap(), e);
}

/// A code this build does not know is read as `Unknown`, keeping its
/// code, parameters and sentence, and written back as it came.
#[test]
fn a_code_from_a_newer_lake_is_kept_and_rendered_by_its_message() {
    let newer = json!({"error": "unavailable", "situation":
        {"code": "tide_out", "depth": 3, "message": "the tide is out"},
        "message": "the lake is unavailable: the tide is out"});
    let e: Error = serde_json::from_value(newer.clone()).unwrap();
    let Some(Situation::Unknown(u)) = e.situation() else {
        panic!("{e:?}");
    };
    assert_eq!(
        (u.code(), u.params()["depth"].as_u64()),
        ("tide_out", Some(3))
    );
    assert_eq!(e.to_string(), "the lake is unavailable: the tide is out");
    assert_eq!(serde_json::to_value(&e).unwrap(), newer);
}

/// Inside a known case, what this build does not know stays where it is
/// and nothing around it is lost: a plain name, a remote end, a tool
/// name's problem, and an error of a newer kind in a cause. A plain name
/// is said as itself in backticks and written back as it came.
#[test]
fn a_newer_part_inside_a_known_case_stays_inside_it() {
    let storage = json!({"error": "unavailable", "situation": {"code": "storage",
        "doing": "dredging", "part": "lake_bed", "detail": "d", "message": "m"}});
    let e: Error = serde_json::from_value(storage).unwrap();
    let Some(Situation::Storage { doing, part, .. }) = e.situation() else {
        panic!("{e:?}");
    };
    assert_eq!((doing.name(), part.name()), ("dredging", "lake_bed"));
    assert_eq!(
        e.to_string(),
        "the lake is unavailable: the storage failed while `dredging` `lake_bed`: d"
    );
    let back = serde_json::to_value(&e).unwrap();
    assert_eq!(
        (&back["situation"]["doing"], &back["situation"]["part"]),
        (&json!("dredging"), &json!("lake_bed"))
    );
    let remote = json!({"error": "unavailable", "situation": {"code": "remote",
        "remote": {"code": "ferry", "pier": 4, "message": "the ferry"},
        "exchange": {"code": "timed_out", "secs": 2}}});
    let e: Error = serde_json::from_value(remote).unwrap();
    let Some(Situation::Remote { remote, exchange }) = e.situation() else {
        panic!("{e:?}");
    };
    assert!(matches!(remote, Remote::Unknown(u) if u.params()["pier"] == 4));
    assert!(matches!(exchange, Exchange::TimedOut { secs: 2 }));
    let names = json!({"error": "unavailable", "situation": {"code": "tool_names",
        "problems": [{"problem": "too_wide", "message": "too wide"},
                     {"problem": "too_long", "name": "x"}]}});
    let e: Error = serde_json::from_value(names).unwrap();
    let Some(Situation::ToolNames { problems }) = e.situation() else {
        panic!("{e:?}");
    };
    assert!(matches!(&problems[0], NameProblem::Unknown(u) if u.message() == "too wide"));
    assert!(matches!(&problems[1], NameProblem::TooLong { .. }));
    let cause = json!({"error": "unavailable", "situation": {"code": "behind",
        "cause": {"error": "flooded", "depth": 3, "message": "the lake is flooded"}}});
    let e: Error = serde_json::from_value(cause.clone()).unwrap();
    let Some(Situation::Behind { cause: Some(c) }) = e.situation() else {
        panic!("{e:?}");
    };
    assert!(matches!(&**c, Error::Unknown(u) if u.code() == "flooded"));
    let back = serde_json::to_value(&e).unwrap();
    assert_eq!(back["situation"]["cause"], cause["situation"]["cause"]);
}

/// A name this build does not know is said as itself in backticks, which
/// no known name's sentence is, so it cannot be read as one.
#[test]
fn no_known_name_is_said_as_a_name_in_backticks() {
    fn backticked(said: String) -> bool {
        said.starts_with('`') && said.ends_with('`') && said.matches('`').count() == 2
    }
    let said = |v: Value| -> Vec<String> {
        let mut out = Vec::new();
        for name in v.as_array().unwrap() {
            let n = name.clone();
            out.push(
                serde_json::from_value::<Part>(n.clone())
                    .unwrap()
                    .to_string(),
            );
            out.push(
                serde_json::from_value::<Doing>(n.clone())
                    .unwrap()
                    .to_string(),
            );
            out.push(serde_json::from_value::<Tls>(n).unwrap().to_string());
        }
        out
    };
    let known = json!([
        "log",
        "log_index",
        "segment",
        "checkpoint",
        "lease_file",
        "projection",
        "name",
        "export",
        "feed",
        "file",
        "query",
        "create",
        "open",
        "read",
        "write",
        "sync",
        "rename",
        "truncate",
        "lock",
        "unlock",
        "measure",
        "compact",
        "ca_unreadable",
        "ca_empty",
        "no_bundle",
        "setup",
        "key_unreadable",
        "cert_unreadable",
        "cert_empty",
        "cert_unusable"
    ]);
    let mut knowns = 0;
    for s in said(known) {
        // Each name is known to one set and unknown to the others.
        if !backticked(s.clone()) {
            knowns += 1;
        }
    }
    assert_eq!(knowns, 30, "every name is known to exactly one set");
}

/// No two of these cases, nor one case with other parameters, say the
/// same sentence.
#[test]
fn every_seam_and_lease_says_its_own_sentence() {
    let ty = TypeName::from("doc");
    let embedding = |bound| Seam::Embedding {
        model: "m".into(),
        dimensions: 2,
        ty: ty.clone(),
        field: "v".into(),
        bound,
    };
    let seams = [
        Seam::Executor { name: "x".into() },
        embedding(None),
        embedding(Some(Bound {
            model: "n".into(),
            dimensions: 3,
        })),
        Seam::FunctionBacking { kind: "x".into() },
        Seam::VectorIndex,
        Seam::Engine { name: "x".into() },
        Seam::Operation { op: "x".into() },
    ];
    let stale = |holder: Option<&str>, current| Situation::LeaseStale {
        fence: 1,
        holder: holder.map(str::to_owned),
        current,
        undone: Some(Act::Appended),
    };
    let leases = [
        stale(Some("w"), Some(2)),
        stale(None, Some(2)),
        stale(None, None),
        Situation::LeaseHeld {
            holder: "w".into(),
            fence: 2,
        },
        Situation::LeaseNotHeld {
            fence: Some(1),
            undone: None,
        },
    ];
    let cause = || Box::new(Error::unavailable(Situation::Moved));
    let append = |left, cut: bool| Situation::AppendFailed {
        detail: Detail::scrubbed("d"),
        left,
        cut: cut.then(cause),
    };
    let answered = Value::Null;
    let calls = [
        append(Left::Nothing, false),
        append(Left::Nothing, true),
        append(Left::Unsure, false),
        append(Left::Unsure, true),
        append(Left::Unknown("unsure".into()), false),
        Situation::AnswerLost {
            answered: answered.clone(),
            cause: cause(),
        },
        Situation::AnswerHeld {
            answered,
            retry: Retry::OtherKey.resent(),
            cause: cause(),
        },
    ];
    // An unknown name with a backtick is not said as two names.
    let names = [Part::Unknown("a` `b".into()), Part::Unknown("a\\".into())];
    let said: Vec<String> = (seams.iter().map(ToString::to_string))
        .chain(leases.iter().map(ToString::to_string))
        .chain(calls.iter().map(ToString::to_string))
        .chain(names.iter().map(ToString::to_string))
        .collect();
    assert_eq!(said[said.len() - 2], "`a\\` \\`b`");
    let distinct: std::collections::BTreeSet<&String> = said.iter().collect();
    assert_eq!(distinct.len(), said.len(), "{said:#?}");
}
