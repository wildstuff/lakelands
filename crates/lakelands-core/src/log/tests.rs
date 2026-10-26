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
fn a_box_belongs_to_a_record_ingested_of_format_20_on() {
    let line = |v: u32, body: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "source:crm", "origin": "source", "source": "crm",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "sealed": {"key": "1", "fields": ["email"], "box": BOX},
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    let ingested = json!({"kind": "record_ingested", "type": "contact", "key": "c1",
                          "version": 1, "patch": {"id": "c1"}});
    let output = json!({"kind": "output_recorded", "seam": "embed:m", "input": "0".repeat(64),
                        "field": ["t", "k", "v"], "output": "AAAA"});
    assert_eq!(misplaced(&line(20, ingested.clone())), None);
    let boxed_on = |kind: &str, v| lakelands_api::error::Flaw::BoxedOn {
        kind: kind.into(),
        v,
    };
    assert_eq!(
        misplaced(&line(19, ingested)),
        Some(boxed_on("record_ingested", 19))
    );
    assert_eq!(
        misplaced(&line(20, output)),
        Some(boxed_on("output_recorded", 20))
    );
    let form = |sealed: serde_json::Value, patch: serde_json::Value| {
        let body = json!({"kind": "record_ingested", "type": "contact", "key": "c1",
                          "version": 1, "patch": patch});
        let mut e = serde_json::to_value(line(20, body)).unwrap();
        e["sealed"] = sealed;
        misplaced(&serde_json::from_value(e).unwrap())
    };
    let (id, both) = (json!({"id": "c1"}), json!({"id": "c1", "email": "a@b"}));
    for (sealed, patch) in [
        (json!({"key": "1", "box": BOX}), &id),
        (json!({"key": "1", "fields": ["email"], "box": BOX}), &both),
        (json!({"key": "1", "fields": ["email"], "box": "AAAA"}), &id),
        (
            json!({"key": "1", "fields": ["email"], "box": &BOX[1..]}),
            &id,
        ),
        (
            json!({"key": "1", "fields": ["email"], "box": BOX.replace('A', "-")}),
            &id,
        ),
        (
            json!({"key": "1", "fields": ["email"], "box": format!("{}=A=", &BOX[..21])}),
            &id,
        ),
    ] {
        let why = form(sealed.clone(), patch.clone());
        assert_eq!(
            why,
            Some(lakelands_api::error::Flaw::BoxForm),
            "{sealed} over {patch}"
        );
    }
    let tail = format!("{}==", &BOX[..22]);
    assert_eq!(
        form(json!({"key": "1", "fields": ["email"], "box": tail}), id),
        None
    );
}

/// A box of the tag's 16 bytes and one more, as padded base64.
const BOX: &str = "AAAAAAAAAAAAAAAAAAAAAAA=";

#[test]
fn seen_before_and_deliveries_belong_to_format_18_on() {
    let line = |v: u32, selection: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": {"kind": "values_removed", "source": "crm", "selection": selection},
            "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    for (by, selection) in [
        (
            "seen_before",
            json!({"seen_before": "2026-01-01T00:00:00.000000Z"}),
        ),
        ("deliveries", json!({"deliveries": ["a", "b"]})),
    ] {
        assert_eq!(misplaced(&line(18, selection.clone())), None);
        let why = misplaced(&line(17, selection)).expect("refused");
        let by = by.to_owned();
        assert_eq!(why, lakelands_api::error::Flaw::SelectsLater { v: 17, by });
    }
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

/// Events of one append in format `v`, from `first`, after `prev`: their
/// canonical values and the events the store hands an engine.
fn append(v: u64, first: u64, prev: &str, n: u64) -> (Vec<Value>, Vec<CanonicalEvent>) {
    let body = |p: u64| {
        json!({"kind": "record_ingested", "type": "t", "key": format!("k{p}"),
               "version": 1, "patch": {"f": 1}})
    };
    append_of(v, first, prev, (first..first + n).map(body).collect())
}

/// [`append`]'s events with the bodies named, one an event.
fn append_of(
    v: u64,
    first: u64,
    prev: &str,
    bodies: Vec<Value>,
) -> (Vec<Value>, Vec<CanonicalEvent>) {
    let mut prev = Digest(prev.to_owned());
    let (mut values, mut events) = (Vec::new(), Vec::new());
    for (p, body) in (first..).zip(bodies) {
        let mut event = json!({
            "v": v, "lake": "l", "position": p, "id": format!("{p:032x}"),
            "prev": prev.0, "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "agent:a", "origin": "direct", "fence": 1,
            "body": body,
        });
        let hash = Digest(crate::engines::hash_of(event.clone()));
        event["hash"] = hash.0.clone().into();
        events.push(CanonicalEvent {
            position: Position(p),
            id: EventId(format!("{p:032x}")),
            prev: prev.clone(),
            hash: hash.clone(),
            fence: 1,
            bytes: canonical::to_vec(&event),
            frame: None,
        });
        values.push(event);
        prev = hash;
    }
    (values, events)
}

#[test]
fn a_framed_append_restores_to_the_events_it_was_made_from() {
    let (values, mut events) = append(FRAMED_FROM, 0, &"0".repeat(64), 3);
    let made: Vec<Vec<u8>> = events.iter().map(|e| e.bytes.clone()).collect();
    frame_append(&mut events);
    let frame = events[0].frame.clone().expect("framed");
    assert_eq!((frame.first, frame.events), (Position(0), 3));
    let line: Value = serde_json::from_slice(&frame.bytes).unwrap();
    let shared = json!({"body": {"kind": "record_ingested", "patch": {"f": 1}, "type": "t", "version": 1},
                        "fence": 1, "known_at": "2026-01-01T00:00:00.000000Z", "lake": "l",
                        "origin": "direct", "writer": "agent:a"});
    assert_eq!(
        line,
        json!({"events": 3, "frame": shared, "v": FRAMED_FROM})
    );
    // The first line, position 0, holds `position` and `prev`; the rest derive them.
    let held = |e: &CanonicalEvent, name| {
        serde_json::from_slice::<Value>(&e.bytes)
            .unwrap()
            .get(name)
            .is_some()
    };
    assert!(held(&events[0], "position") && held(&events[0], "prev"));
    assert!(!held(&events[1], "position") && !held(&events[2], "prev"));
    assert_eq!(frame_fault(&events), Ok(()));
    for (i, e) in events.iter().enumerate() {
        let (restored, _) = restore(e).unwrap();
        assert_eq!(canonical::to_vec(&restored), made[i], "event {i}");
        assert_eq!(restored, values[i]);
        assert_eq!(crate::engines::hash_of(restored), e.hash.0);
    }
    // The digest is over the restored events, as the unframed lines are.
    let mut whole = append(FRAMED_FROM, 0, &"0".repeat(64), 3).1;
    assert_eq!(digest_of(&events), digest_of(&whole));
    whole.truncate(1);
    let mut file = whole[0].bytes.clone();
    file.push(b'\n');
    assert_eq!(
        digest_of(&whole).unwrap().0,
        canonical::hex(&Sha256::digest(&file))
    );
}

#[test]
fn a_single_event_and_an_older_format_are_kept_whole() {
    let (_, mut one) = append(FRAMED_FROM, 0, &"0".repeat(64), 1);
    let line = one[0].bytes.clone();
    frame_append(&mut one);
    assert!(one[0].frame.is_none());
    assert_eq!(one[0].bytes, line);
    let (_, mut older) = append(FRAMED_FROM - 1, 4, &"1".repeat(64), 2);
    let lines: Vec<_> = older.iter().map(|e| e.bytes.clone()).collect();
    frame_append(&mut older);
    assert!(older.iter().all(|e| e.frame.is_none()));
    assert_eq!(
        older.iter().map(|e| e.bytes.clone()).collect::<Vec<_>>(),
        lines
    );
    assert_eq!(restore(&older[1]).map(|(_, whole)| whole), Ok(true));
}

#[test]
fn an_append_that_begins_the_lakes_format_is_kept_whole() {
    let lease = json!({"kind": "lease_taken", "fence": 1, "holder": "h"});
    let created = json!({"kind": "lake_created", "format": FRAMED_FROM});
    let upgraded = json!({"kind": "lake_upgraded", "from": FRAMED_FROM - 1, "to": FRAMED_FROM});
    let made = append_of(
        FRAMED_FROM,
        0,
        &"0".repeat(64),
        vec![created, lease.clone()],
    );
    let upgrade = append_of(FRAMED_FROM, 7, &"1".repeat(64), vec![lease, upgraded]);
    for (what, (values, mut events)) in [("created", made), ("upgraded", upgrade)] {
        frame_append(&mut events);
        let lines: Vec<_> = values.iter().map(canonical::to_vec).collect();
        assert!(events.iter().all(|e| e.frame.is_none()), "{what}");
        assert_eq!(
            events.iter().map(|e| e.bytes.clone()).collect::<Vec<_>>(),
            lines,
            "{what}: every line is its event's canonical bytes"
        );
    }
}

#[test]
fn a_segment_reads_older_lines_and_framed_ones_in_order() {
    let (older, _) = append(FRAMED_FROM - 1, 0, &"0".repeat(64), 2);
    let last = Digest(older[1]["hash"].as_str().unwrap().to_owned());
    let (newer, mut framed) = append(FRAMED_FROM, 2, &last.0, 3);
    frame_append(&mut framed);
    let mut file = vec![canonical::to_vec(&older[0]), canonical::to_vec(&older[1])];
    file.push(framed[0].frame.as_ref().unwrap().bytes.clone());
    file.extend(framed.iter().map(|e| e.bytes.clone()));
    let (mut lines, mut before) = (Lines::default(), None::<(Position, Digest)>);
    let mut read = Vec::new();
    for bytes in &file {
        let line = line_of(bytes).unwrap();
        if let Some((event, _)) = lines
            .line(line, before.as_ref().map(|(p, h)| (*p, h)))
            .unwrap()
        {
            let hash = Digest(event["hash"].as_str().unwrap().to_owned());
            before = Some((Position(event["position"].as_u64().unwrap()), hash));
            read.push(event);
        }
    }
    assert_eq!(lines.end(), Ok(()));
    assert_eq!(read, [older, newer].concat());
}

/// What a reader of `file`, one segment's lines from its first, says of
/// it: the first refusal, or none.
fn refusal(file: &[Vec<u8>]) -> Option<Flaw> {
    let (mut lines, mut before) = (Lines::default(), None::<(Position, Digest)>);
    for bytes in file {
        let line = match line_of(bytes) {
            Ok(line) => line,
            Err(flaw) => return Some(flaw),
        };
        match lines.line(line, before.as_ref().map(|(p, h)| (*p, h))) {
            Err(flaw) => return Some(flaw),
            Ok(Some((event, _))) => {
                let hash = Digest(event["hash"].as_str().unwrap().to_owned());
                before = Some((Position(event["position"].as_u64().unwrap()), hash));
            }
            Ok(None) => {}
        }
    }
    lines.end().err()
}

#[test]
fn every_break_of_a_framed_segment_is_refused() {
    let (_, mut events) = append(FRAMED_FROM, 0, &"0".repeat(64), 3);
    frame_append(&mut events);
    let frame = events[0].frame.as_ref().unwrap().bytes.clone();
    let file: Vec<Vec<u8>> = std::iter::once(frame.clone())
        .chain(events.iter().map(|e| e.bytes.clone()))
        .collect();
    assert_eq!(refusal(&file), None);
    let edit = |i: usize, f: &dyn Fn(&mut Value)| {
        let mut file = file.clone();
        let mut line: Value = serde_json::from_slice(&file[i]).unwrap();
        f(&mut line);
        file[i] = canonical::to_vec(&line);
        refusal(&file)
    };
    // A frame that ends before its events do, or covers more than the segment holds.
    assert_eq!(
        refusal(&file[..3]),
        Some(Flaw::FrameShort { events: 3, held: 2 })
    );
    // A frame line among the lines a frame covers.
    let mut inner = file.clone();
    inner.insert(2, frame.clone());
    assert_eq!(refusal(&inner), Some(Flaw::FrameInFrame));
    // A frame of another format than its line's.
    assert_eq!(
        edit(2, &|l| l["v"] = (FRAMED_FROM + 1).into())
            .map(|f| matches!(f, Flaw::FormatOther { .. } | Flaw::FrameOther { .. })),
        Some(true)
    );
    // A member a line holds that its frame holds too.
    let twice = edit(2, &|l| l["lake"] = "l".into());
    assert_eq!(
        twice,
        Some(Flaw::FramedTwice {
            member: "lake".into()
        })
    );
    let nested = edit(3, &|l| l["body"]["type"] = "t".into());
    assert_eq!(
        nested,
        Some(Flaw::FramedTwice {
            member: "body.type".into()
        })
    );
    // A frame that holds what no frame holds, or is not one.
    assert_eq!(
        edit(0, &|l| l["frame"]["position"] = 0.into()),
        Some(Flaw::NotAFrame)
    );
    assert_eq!(edit(0, &|l| l["events"] = 0.into()), Some(Flaw::NotAFrame));
    assert_eq!(
        edit(0, &|l| l["frame"]["body"] = json!({})),
        Some(Flaw::NotAFrame)
    );
    assert_eq!(edit(0, &|l| l["extra"] = 1.into()), Some(Flaw::NotAFrame));
    // A segment's first event line that does not hold its position and prev.
    assert_eq!(
        edit(1, &|l| drop(l.as_object_mut().unwrap().remove("prev"))),
        Some(Flaw::StartUnheld)
    );
    // A position or a prev a line holds that is not the one derived.
    let position = edit(2, &|l| l["position"] = 7.into());
    assert!(
        matches!(
            position,
            Some(Flaw::Position {
                found: Some(Position(7)),
                ..
            })
        ),
        "{position:?}"
    );
    assert_eq!(
        edit(3, &|l| l["prev"] = "0".repeat(64).into()),
        Some(Flaw::PrevHash)
    );
    // A line that is not canonical, as before frames.
    let mut loose = file.clone();
    loose[2].insert(1, b' ');
    assert_eq!(refusal(&loose), Some(Flaw::NotCanonical));
}

#[test]
fn an_append_holds_each_frame_it_shares_whole() {
    let (_, mut events) = append(FRAMED_FROM, 0, &"0".repeat(64), 3);
    frame_append(&mut events);
    let refused = |events: &[CanonicalEvent]| frame_fault(events).err().map(|e| format!("{e:?}"));
    assert_eq!(refused(&events), None);
    assert!(
        refused(&events[..2]).is_some(),
        "a frame past the append's last event"
    );
    assert!(
        refused(&events[1..]).is_some(),
        "a frame begun before the append"
    );
    let mut unframed = events.clone();
    unframed[1].frame = None;
    assert!(
        refused(&unframed).is_some(),
        "an event the frame covers without it"
    );
    let mut feed = events.clone();
    let mut bytes = feed[0].frame.as_ref().unwrap().as_ref().clone();
    bytes.bytes.push(b'\n');
    let bytes = Arc::new(bytes);
    feed.iter_mut().for_each(|e| e.frame = Some(bytes.clone()));
    assert!(refused(&feed).is_some(), "a frame with a line feed");
}
