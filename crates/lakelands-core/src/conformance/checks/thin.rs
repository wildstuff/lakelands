//! The six refusals of 102's thinning act, as the catalog states them:
//! each has its own id and its own sentence, whether a build performs the
//! act or not. The act's reads and writes on every engine are S5's and
//! S6's rows; this one holds the answers' form for every engine alike.

use crate::conformance::prelude::*;

check!(
    THIN_REFUSALS = a_thinning_refuses_in_six_named_cases,
    "102: a thinning act refuses in six named cases, each with its own id and sentence: `thin_by`, a field of neither instants nor dates, naming the type and the field; `thinned`, a write, an ingest, a retirement or a read at a key the act took, naming the type, the key and the horizon; `stranded_at_head_only`, an earlier-position read of a stranded group, naming the type and the position; `kept_over_thinned`, a view made kept over a type whose records the act took, naming the view and the type; `kept_not_exact`, a kept view of the type holding a group that is not exact, naming the view and the type; and `stranded_make`, a view made again by an event that changes what its stranded groups fold, naming the view and the type"
);

async fn a_thinning_refuses_in_six_named_cases(store: Store) -> Held {
    // Every check opens a lake, so an engine the suite cannot reach fails
    // this row with the store's own refusal rather than passing it.
    let _ = super::links::lake(store.as_ref()).await?;
    let before = Timestamp::parse("2026-06-01T00:00:00Z").expect("an instant");
    let said: [(Case, &str, &[&str]); 6] = [
        (
            Case::ThinBy {
                ty: "tick".into(),
                field: "note".into(),
            },
            "thin_by",
            &["`tick`", "`note`"],
        ),
        (
            Case::Thinned {
                ty: "tick".into(),
                key: "t1".into(),
                before,
            },
            "thinned",
            &["`tick`", "`t1`", "2026-06-01"],
        ),
        (
            Case::StrandedAtHeadOnly {
                ty: "daily".into(),
                position: Position(12),
            },
            "stranded_at_head_only",
            &["`daily`", "12"],
        ),
        (
            Case::KeptOverThinned {
                view: "daily".into(),
                ty: "tick".into(),
            },
            "kept_over_thinned",
            &["`daily`", "`tick`"],
        ),
        (
            Case::KeptNotExact {
                view: "daily".into(),
                ty: "tick".into(),
            },
            "kept_not_exact",
            &["`tick`", "`daily`"],
        ),
        (
            Case::StrandedMake {
                view: "daily".into(),
                ty: "tick".into(),
            },
            "stranded_make",
            &["`daily`", "`tick`"],
        ),
    ];
    for (case, id, names) in said {
        let wire = serde_json::to_value(&case).expect("a case serialises");
        ensure!(
            wire["code"] == id,
            "the case's id is `{id}`, not the wire's {}",
            wire["code"]
        );
        let sentence = case.to_string();
        for name in names {
            ensure!(
                sentence.contains(name),
                "`{id}` does not name {name}: {sentence}"
            );
        }
        let read: Case = serde_json::from_value(wire).expect("a case reads back");
        ensure!(
            read == case,
            "`{id}` does not read back as itself: {read:?}"
        );
    }
    Ok(())
}
