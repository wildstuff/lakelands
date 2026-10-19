use super::*;
use serde_json::json;

#[test]
fn ids_depend_on_what_they_name() {
    let p: Patch = serde_json::from_value(json!({"a": 1})).unwrap();
    let id = |v: u64| ingest_id(&"bank".into(), &"t".into(), &"k".into(), v, &p);
    assert_eq!(id(0), id(0));
    assert_ne!(id(0), id(2), "a revert is a new event");
    assert_ne!(
        authored_id("agent:a", &"k".into()),
        authored_id("agent:b", &"k".into())
    );
    assert_eq!(positional_id(&"l".into(), Position(3)).0.len(), 32);
}

#[test]
fn upstream_belongs_to_record_ingested_alone() {
    let line = |body: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": lakelands_api::event::LOG_FORMAT_VERSION, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "upstream": {"lake": "other", "position": 9},
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    let retired = line(json!({"kind": "record_retired", "type": "t", "key": "k", "version": 2}));
    let why = misplaced(&retired).expect("refused");
    let kind = "record_retired".to_owned();
    assert_eq!(why, lakelands_api::error::Flaw::UpstreamOn { kind });
    let ingested = line(json!({"kind": "record_ingested", "type": "t", "key": "k",
                                   "version": 1, "patch": {"f": 1}}));
    assert_eq!(misplaced(&ingested), None);
}

#[test]
fn an_output_is_carried_or_named_and_never_both() {
    let event = |members: serde_json::Value| -> Event {
        let mut body = json!({"kind": "output_recorded", "seam": "embed:m", "input": "0".repeat(64),
                              "field": ["t", "k", "v"]});
        body.as_object_mut()
            .unwrap()
            .extend(members.as_object().unwrap().clone());
        serde_json::from_value(json!({
            "v": lakelands_api::event::LOG_FORMAT_VERSION, "lake": "l", "position": 1,
            "id": "0".repeat(32), "prev": "0".repeat(64),
            "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&event(json!({"output_at": 7}))), None);
    let carried = event(json!({"output": null}));
    assert!(
        matches!(&carried.body, EventBody::OutputRecorded { output: Some(v), .. } if v.is_null())
    );
    assert_eq!(misplaced(&carried), None);
    for members in [json!({"output": [1, 0], "output_at": 7}), json!({})] {
        assert_eq!(misplaced(&event(members)), Some(Flaw::OutputOnce));
    }
}

/// An event in format `v` with `body`, its envelope otherwise fixed.
fn in_format(v: u32, body: serde_json::Value) -> Event {
    serde_json::from_value(json!({
        "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
        "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
        "writer": "system:lakelands", "origin": "system",
        "context": {"origin": "direct", "depth": 0}, "fence": 1,
        "body": body, "hash": "0".repeat(64)
    }))
    .unwrap()
}

/// The formats this release reads run from the oldest to the one it
/// writes, and nothing outside them.
#[test]
fn the_formats_read_run_from_the_oldest_to_the_one_written() {
    use lakelands_api::event::OLDEST_LOG_FORMAT;
    let (oldest, written) = (u64::from(OLDEST_LOG_FORMAT), u64::from(LOG_FORMAT_VERSION));
    assert!((oldest..=written).all(reads_format));
    assert!(![0, oldest - 1, written + 1].into_iter().any(reads_format));
}

/// A lake's format stays from one event to the next; it rises only at a
/// new writer's `lease_taken`, whose next event is the `lake_upgraded`
/// from the one format to the other, both in the new; it never falls; and
/// a step begun and not marked is a flaw where the log ends.
#[test]
fn a_lake_s_format_rises_only_where_a_new_writer_upgrades_it() {
    let (to, from) = (LOG_FORMAT_VERSION, LOG_FORMAT_VERSION - 1);
    let lease = || json!({"kind": "lease_taken", "fence": 2, "holder": "h"});
    let marker = |from: u32, to: u32| json!({"kind": "lake_upgraded", "from": from, "to": to});
    let declared =
        || json!({"kind": "checkpoint", "through": 1, "hash": "0".repeat(64), "segments": []});
    // The flaws of a walk after `before`, event by event.
    let walk = |before: u32, events: &[(u32, serde_json::Value)]| {
        let mut steps = FormatSteps::after(Some(&in_format(
            before,
            json!({"kind": "checkpoint", "through": 0, "hash": "0".repeat(64), "segments": []}),
        )));
        let mut flaws: Vec<_> = events
            .iter()
            .map(|(v, b)| steps.next(&in_format(*v, b.clone())))
            .collect();
        flaws.push(steps.unmarked());
        flaws
    };
    let step = |v: u32, before: u32| {
        Some(lakelands_api::error::Flaw::FormatStep {
            v: v.into(),
            before: before.into(),
        })
    };
    // Nothing read before: nothing to judge.
    assert_eq!(FormatSteps::default().next(&in_format(from, lease())), None);
    // The format stays.
    assert_eq!(
        walk(to, &[(to, lease()), (to, declared())]),
        [None, None, None]
    );
    // It rises at a new writer's lease, marked next.
    assert_eq!(
        walk(
            from,
            &[(to, lease()), (to, marker(from, to)), (to, declared())]
        ),
        [None, None, None, None]
    );
    // A lease that raises it and no marker after it, or another event.
    assert_eq!(walk(from, &[(to, lease())]), [None, step(to, from)]);
    assert_eq!(
        walk(from, &[(to, lease()), (to, declared())]),
        [None, step(to, from), None]
    );
    // A marker that names other formats than the step.
    for (f, t) in [(from - 1, to), (from, to + 1), (to, from)] {
        assert_eq!(
            walk(from, &[(to, lease()), (to, marker(f, t))]),
            [None, step(to, from), None],
            "{f} -> {t}"
        );
    }
    // A rise at anything but a lease, the marker itself among them.
    assert_eq!(walk(from, &[(to, declared())]), [step(to, from), None]);
    assert_eq!(
        walk(from, &[(to, marker(from, to))]),
        [step(to, from), None]
    );
    // It never falls, at a lease or not.
    assert_eq!(walk(to, &[(from, lease())]), [step(from, to), None]);
    assert_eq!(walk(to, &[(from, declared())]), [step(from, to), None]);
    // A walk that begins right after a lease it did not see rise takes the
    // marker that follows it; after any other event, it does not.
    let leased = in_format(to, lease());
    let mut steps = FormatSteps::after(Some(&leased));
    assert_eq!(steps.next(&in_format(to, marker(from, to))), None);
    let mut steps = FormatSteps::after(Some(&in_format(to, declared())));
    let marked = in_format(to, marker(from, to));
    assert_eq!(steps.next(&marked), step(to, to));
    // A marker where the format stays marks nothing.
    assert_eq!(walk(to, &[(to, marker(to, to))]), [step(to, to), None]);
}

/// A `lake_created` says the format of its own line, one this release
/// reads.
#[test]
fn a_lake_created_says_its_own_line_s_format() {
    let made =
        |v: u32, format: u32| in_format(v, json!({"kind": "lake_created", "format": format}));
    let (written, oldest) = (LOG_FORMAT_VERSION, lakelands_api::event::OLDEST_LOG_FORMAT);
    assert_eq!(created_in_another(&made(written, written)), None);
    assert_eq!(created_in_another(&made(oldest, oldest)), None);
    for (v, format) in [
        (written, oldest),
        (oldest, written),
        (oldest - 1, oldest - 1),
    ] {
        assert!(
            created_in_another(&made(v, format)).is_some(),
            "{v} says {format}"
        );
    }
}

#[test]
fn sealed_belongs_to_format_16_on() {
    let line = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "service:crm", "origin": "source", "source": "crm",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "sealed": {"key": "1", "fields": ["f"]},
            "body": {"kind": "record_ingested", "type": "t", "key": "k", "version": 1,
                     "patch": {"f": "AAAA"}},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&line(16)), None);
    let why = misplaced(&line(15)).expect("refused");
    assert_eq!(why, lakelands_api::error::Flaw::SealedBefore { v: 15 });
}

#[test]
fn source_forgotten_belongs_to_format_16_on() {
    let line = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": {"kind": "source_forgotten", "source": "crm", "keys": ["1"]},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&line(16)), None);
    let why = misplaced(&line(15)).expect("refused");
    assert_eq!(why, lakelands_api::error::Flaw::ForgottenBefore { v: 15 });
}

#[test]
fn values_removed_belongs_to_format_17_on() {
    let line = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": {"kind": "values_removed", "source": "crm",
                     "selection": {"written_before": "2026-01-01T00:00:00.000000Z"}, "before": 1},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&line(17)), None);
    let why = misplaced(&line(16)).expect("refused");
    assert_eq!(why, lakelands_api::error::Flaw::RemovedBefore { v: 16 });
}

#[test]
fn apart_belongs_to_format_18_on() {
    let removed = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": {"kind": "values_removed", "source": "crm",
                     "selection": {"delivery": "b1"}, "apart": ["a1"]},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    let ingested = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "source:crm", "origin": "source", "source": "crm",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "sealed": {"key": "1", "fields": ["email"], "apart": ["a1"]},
            "body": {"kind": "record_ingested", "type": "contact", "key": "c1",
                     "version": 1, "patch": {"email": "AAAA"}},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    for line in [removed, ingested] {
        assert_eq!(misplaced(&line(18)), None);
        let why = misplaced(&line(17)).expect("refused");
        assert_eq!(why, lakelands_api::error::Flaw::ApartBefore { v: 17 });
    }
}

#[test]
fn seen_before_belongs_to_format_18_on() {
    let line = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": {"kind": "values_removed", "source": "crm",
                     "selection": {"seen_before": "2026-01-01T00:00:00.000000Z"}},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&line(18)), None);
    let why = misplaced(&line(17)).expect("refused");
    assert_eq!(why, lakelands_api::error::Flaw::SelectsSeenBefore { v: 17 });
}

#[test]
fn trigger_dropped_belongs_to_format_17_on() {
    let line = |v: u32| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 2, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system", "cause": 1,
            "context": {"origin": "reactive", "depth": 1}, "fence": 1,
            "body": {"kind": "trigger_dropped", "trigger": "t", "type": "job", "key": "k",
                     "depth": 1, "max_depth": 1, "bound": "trigger"},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&line(17)), None);
    let why = misplaced(&line(16)).expect("refused");
    assert_eq!(why, lakelands_api::error::Flaw::DroppedBefore { v: 16 });
}

#[test]
fn a_firings_change_belongs_to_format_17_on() {
    let line = |v: u32, change: serde_json::Value| -> Event {
        let mut body = json!({"kind": "trigger_fired", "trigger": "t", "type": "job", "key": "k"});
        body.as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 2, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system", "cause": 1,
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    let told = json!({"change": "upserted", "created": true});
    assert_eq!(misplaced(&line(17, told.clone())), None);
    assert_eq!(misplaced(&line(16, json!({}))), None);
    for member in [
        json!({"change": "upserted"}),
        json!({"created": false}),
        told,
    ] {
        let why = misplaced(&line(16, member.clone())).expect("refused");
        assert_eq!(
            why,
            lakelands_api::error::Flaw::ChangeBefore { v: 16 },
            "{member}"
        );
    }
}
