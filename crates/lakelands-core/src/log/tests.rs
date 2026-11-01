use super::*;
use serde_json::json;

/// The format this release writes, as a line holds it.
const FORMAT: u64 = LOG_FORMAT_VERSION as u64;

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
        (written, oldest - 1),
        (oldest - 1, written),
        (oldest - 1, oldest - 1),
    ] {
        assert!(
            created_in_another(&made(v, format)).is_some(),
            "{v} says {format}"
        );
    }
}

#[test]
fn a_box_belongs_to_a_record_ingested() {
    let line = |body: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": FORMAT, "lake": "l", "position": 1, "id": "0".repeat(32),
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
    assert_eq!(misplaced(&line(ingested)), None);
    assert_eq!(
        misplaced(&line(output)),
        Some(lakelands_api::error::Flaw::BoxedOn {
            kind: "output_recorded".into(),
            v: LOG_FORMAT_VERSION,
        })
    );
    let form = |sealed: serde_json::Value, patch: serde_json::Value| {
        let body = json!({"kind": "record_ingested", "type": "contact", "key": "c1",
                          "version": 1, "patch": patch});
        let mut e = serde_json::to_value(line(body)).unwrap();
        e["sealed"] = sealed;
        misplaced(&serde_json::from_value(e).unwrap())
    };
    let (id, both) = (json!({"id": "c1"}), json!({"id": "c1", "email": "a@b"}));
    let alone = json!({"id": "c1", "email": BOX});
    for (sealed, patch) in [
        (json!({"key": "1", "fields": ["email"]}), &alone),
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
    let (values, mut events) = append(FORMAT, 0, &"0".repeat(64), 3);
    let made: Vec<Vec<u8>> = events.iter().map(|e| e.bytes.clone()).collect();
    frame_append(&mut events);
    let frame = events[0].frame.clone().expect("framed");
    assert_eq!((frame.first, frame.events), (Position(0), 3));
    let line: Value = serde_json::from_slice(&frame.bytes).unwrap();
    let shared = json!({"body": {"kind": "record_ingested", "patch": {"f": 1}, "type": "t", "version": 1},
                        "fence": 1, "known_at": "2026-01-01T00:00:00.000000Z", "lake": "l",
                        "origin": "direct", "writer": "agent:a"});
    assert_eq!(line, json!({"events": 3, "frame": shared, "v": FORMAT}));
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
    let mut whole = append(FORMAT, 0, &"0".repeat(64), 3).1;
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
fn a_single_event_is_kept_whole() {
    let (_, mut one) = append(FORMAT, 0, &"0".repeat(64), 1);
    let line = one[0].bytes.clone();
    frame_append(&mut one);
    assert!(one[0].frame.is_none());
    assert_eq!(one[0].bytes, line);
    assert_eq!(restore(&one[0]).map(|(_, whole)| whole), Ok(true));
}

#[test]
fn the_append_that_makes_a_lake_is_kept_whole() {
    let lease = json!({"kind": "lease_taken", "fence": 1, "holder": "h"});
    let created = json!({"kind": "lake_created", "format": FORMAT});
    let (values, mut events) = append_of(FORMAT, 0, &"0".repeat(64), vec![created, lease]);
    frame_append(&mut events);
    let lines: Vec<_> = values.iter().map(canonical::to_vec).collect();
    assert!(events.iter().all(|e| e.frame.is_none()));
    assert_eq!(
        events.iter().map(|e| e.bytes.clone()).collect::<Vec<_>>(),
        lines,
        "every line is its event's canonical bytes"
    );
}

#[test]
fn a_segment_reads_whole_lines_and_framed_ones_in_order() {
    let (whole, _) = append(FORMAT, 0, &"0".repeat(64), 2);
    let last = Digest(whole[1]["hash"].as_str().unwrap().to_owned());
    let (newer, mut framed) = append(FORMAT, 2, &last.0, 3);
    frame_append(&mut framed);
    let mut file = vec![canonical::to_vec(&whole[0]), canonical::to_vec(&whole[1])];
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
    assert_eq!(read, [whole, newer].concat());
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
    let (_, mut events) = append(FORMAT, 0, &"0".repeat(64), 3);
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
        edit(2, &|l| l["v"] = (FORMAT + 1).into())
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
    let (_, mut events) = append(FORMAT, 0, &"0".repeat(64), 3);
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
