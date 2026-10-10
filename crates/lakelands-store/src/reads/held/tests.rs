use super::*;

/// A record of `t` at `key`, held or not.
fn record(key: &str, held: bool) -> StoredRecord {
    let mut r = StoredRecord::ended("t".into(), key.into(), Position(1));
    (r.retired, r.restricted) = (false, held);
    r
}

#[test]
fn a_record_the_head_does_not_hold_keeps_its_hold() {
    let rows = vec![record("gone", true), record("freed", true)];
    let keys = ["gone".into(), "freed".into()];
    let held = held_as(rows, &keys, vec![record("freed", false)]);
    let held: Vec<_> = held
        .iter()
        .map(|r| (r.key.as_str(), r.restricted))
        .collect();
    // `gone` has no counterpart at the head: it is never released by that.
    assert_eq!(held, [("gone", true), ("freed", false)]);
}

/// A read's holds are read from the view its search's head was read from,
/// with the moves that head found. Two views of the head could differ by
/// a re-key that landed between them: the moves would not know of it, and
/// a held record would be judged by the hold of the record now at its key.
/// No single process can time that, so this holds the two to one view.
#[test]
fn a_reads_holds_and_its_searchs_head_are_one_view() {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::principal::{Grants, Principal, PrincipalKind};
    use lakelands_api::seam::Seams;
    let seams = Seams {
        engines: vec![std::sync::Arc::new(
            lakelands_engine_mem::MemoryOpener::new(),
        )],
        ..Seams::default()
    };
    let location = lakelands_api::Location::memory("held");
    let lake = Lake::open(&location, seams, OpenMode::Create).expect("the lake opens");
    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "held".into(),
        grants: Grants::owner(),
    };
    let then = || lake.engine.view(None).expect("a view");
    let at = then().position().unwrap_or(Position::GENESIS);
    let head = super::super::Head::new(&lake, &who, at, None).expect("the head is read");
    let with = HeldNow::new(&lake, then(), Some(&head)).expect("the holds are read");
    assert!(
        Rc::ptr_eq(&with.head, &head.view) && Rc::ptr_eq(&with.since, &head.since),
        "a read with a search's head reads its holds from that head's view and moves"
    );
    let alone = HeldNow::new(&lake, then(), None).expect("the holds are read");
    assert!(
        !Rc::ptr_eq(&alone.head, &head.view),
        "a read without one opens its own"
    );
}
