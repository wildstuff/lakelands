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
        (json!({"key": "1", "fields": ["email"]}), &id),
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

/// The block a frame at the seam holds, its stored bytes inflated.
fn opened(frame: &Frame) -> (Block, Vec<Vec<u8>>) {
    let at = frame.bytes.iter().position(|b| *b == b'\n').unwrap();
    let (block, v) = Block::of(&serde_json::from_slice(&frame.bytes[..at]).unwrap()).unwrap();
    assert_eq!(v, FORMAT);
    let raw = inflate(&block, &frame.bytes[at + 1..]).unwrap();
    let lines = raw[..raw.len() - 1]
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec);
    (block, lines.collect())
}

#[test]
fn an_append_of_two_or_more_is_a_block_that_restores_every_event() {
    let (values, mut events) = append(FORMAT, 0, &"0".repeat(64), 3);
    let made: Vec<Vec<u8>> = events.iter().map(|e| e.bytes.clone()).collect();
    frame_append(&mut events);
    let frame = events[0].frame.clone().expect("a block");
    assert_eq!((frame.first, frame.events), (Position(0), 3));
    let (block, lines) = opened(&frame);
    assert_eq!(
        (
            block.end,
            block.events,
            block.first,
            &block.hash,
            &block.prev
        ),
        (
            Position(2),
            3,
            Position(0),
            &events[2].hash,
            &Digest::zero()
        )
    );
    let raw: usize = lines.iter().map(|l| l.len() + 1).sum();
    assert_eq!(block.raw, raw as u64);
    assert_eq!(
        block.stored as usize,
        frame.bytes.len() - frame.bytes.iter().position(|b| *b == b'\n').unwrap() - 1
    );
    // Its frame line, then each event's line, which is the event's bytes
    // at the seam and leaves out what the block decides.
    let shared = json!({"body": {"kind": "record_ingested", "patch": {"f": 1}, "type": "t", "version": 1},
                        "fence": 1, "known_at": "2026-01-01T00:00:00.000000Z", "lake": "l",
                        "origin": "direct", "writer": "agent:a"});
    let line: Value = serde_json::from_slice(&lines[0]).unwrap();
    assert_eq!(line, json!({"events": 3, "frame": shared, "v": FORMAT}));
    assert_eq!(
        &lines[1..],
        events.iter().map(|e| e.bytes.clone()).collect::<Vec<_>>()
    );
    for e in &events {
        let line: Value = serde_json::from_slice(&e.bytes).unwrap();
        for name in ["hash", "position", "prev", "v"] {
            assert!(line.get(name).is_none(), "{name}");
        }
        assert!(line.get("id").is_some(), "an id no member decides is kept");
    }
    assert_eq!(frame_fault(&events), Ok(()));
    assert_eq!(append_end(&events[0]), Position(2));
    for (i, e) in events.iter().enumerate() {
        let (restored, whole) = restore(e).unwrap();
        assert!(!whole);
        assert_eq!(canonical::to_vec(&restored), made[i], "event {i}");
        assert_eq!(restored, values[i]);
        assert_eq!(crate::engines::hash_of(restored), e.hash.0);
    }
    // The digest is over the restored events, as the plain lines are.
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
fn a_block_leaves_out_an_id_its_event_decides() {
    let bodies = (0..2).map(|k| {
        json!({"kind": "record_ingested", "type": "t", "key": format!("k{k}"), "version": 3,
               "patch": {"f": 1}})
    });
    let (_, mut events) = append_of(FORMAT, 0, &"0".repeat(64), bodies.collect());
    for e in &mut events {
        let mut value: Value = serde_json::from_slice(&e.bytes).unwrap();
        value["source"] = "bank".into();
        let key = value["body"]["key"].as_str().unwrap().to_owned();
        let patch = serde_json::from_value(json!({"f": 1})).unwrap();
        e.id = ingest_id(&"bank".into(), &"t".into(), &key.as_str().into(), 2, &patch);
        assert_eq!(derived_id(&value), Some(e.id.clone()));
        value["id"] = e.id.0.clone().into();
        value.as_object_mut().unwrap().remove("hash");
        e.hash = Digest(crate::engines::hash_of(value.clone()));
        value["hash"] = e.hash.0.clone().into();
        e.bytes = canonical::to_vec(&value);
    }
    events[1].prev = events[0].hash.clone();
    let mut value: Value = serde_json::from_slice(&events[1].bytes).unwrap();
    value["prev"] = events[0].hash.0.clone().into();
    value.as_object_mut().unwrap().remove("hash");
    events[1].hash = Digest(crate::engines::hash_of(value.clone()));
    value["hash"] = events[1].hash.0.clone().into();
    events[1].bytes = canonical::to_vec(&value);
    let made: Vec<Vec<u8>> = events.iter().map(|e| e.bytes.clone()).collect();
    frame_append(&mut events);
    for (e, made) in events.iter().zip(made) {
        let line: Value = serde_json::from_slice(&e.bytes).unwrap();
        assert!(line.get("id").is_none(), "the id the event decides");
        let (restored, _) = restore(e).unwrap();
        assert_eq!(canonical::to_vec(&restored), made);
    }
    // A sealed ingest's id is over its box too, and a quarantined
    // record's over its own members.
    let mut sealed = json!({"body": {"kind": "record_ingested", "type": "t", "key": "k", "version": 1,
                                     "patch": {}}, "source": "bank", "sealed": {"key": "k1", "box": BOX}});
    let patch = serde_json::from_value(json!({})).unwrap();
    let boxed = boxed_ingest_id(&"bank".into(), &"t".into(), &"k".into(), 0, &patch, BOX);
    assert_eq!(derived_id(&sealed), Some(boxed));
    sealed["body"]["kind"] = "record_upserted".into();
    assert_eq!(
        derived_id(&sealed),
        None,
        "an authored write's id is no member's"
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
    let (newer, _) = append(FORMAT, 2, &last.0, 3);
    let (frame, framed) = stored_form(&newer, |_, _| (false, false));
    let mut file = vec![canonical::to_vec(&whole[0]), canonical::to_vec(&whole[1])];
    file.extend(frame);
    file.extend(framed);
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
    let (values, _) = append(FORMAT, 0, &"0".repeat(64), 3);
    let (frame, lines) = stored_form(&values, |i, _| (i == 0, i == 0));
    let frame = frame.unwrap();
    let file: Vec<Vec<u8>> = std::iter::once(frame.clone()).chain(lines).collect();
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
fn an_append_holds_each_block_it_shares_whole() {
    let (_, mut events) = append(FORMAT, 0, &"0".repeat(64), 3);
    frame_append(&mut events);
    let refused = |events: &[CanonicalEvent]| frame_fault(events).err().map(|e| format!("{e:?}"));
    assert_eq!(refused(&events), None);
    assert!(
        refused(&events[..2]).is_some(),
        "a block past the append's last event"
    );
    assert!(
        refused(&events[1..]).is_some(),
        "a block begun before the append"
    );
    let mut unframed = events.clone();
    unframed[1].frame = None;
    assert!(
        refused(&unframed).is_some(),
        "an event the block holds without it"
    );
    // A frame that is no block, and a block whose `end` is not the
    // append's last position.
    let (values, _) = append(FORMAT, 0, &"0".repeat(64), 3);
    let line = stored_form(&values, |_, _| (false, false)).0.unwrap();
    let mut plain = events.clone();
    let frame = Arc::new(Frame {
        bytes: line,
        ..events[0].frame.as_deref().unwrap().clone()
    });
    plain.iter_mut().for_each(|e| e.frame = Some(frame.clone()));
    assert!(refused(&plain).is_some(), "a frame line, no block");
    let (_, mut longer) = append(FORMAT, 0, &"0".repeat(64), 4);
    frame_append(&mut longer);
    let mut short = events.clone();
    let frame = Arc::new(Frame {
        events: 3,
        ..longer[0].frame.as_deref().unwrap().clone()
    });
    short.iter_mut().for_each(|e| e.frame = Some(frame.clone()));
    assert!(
        refused(&short).is_some(),
        "a block that ends past the append"
    );
}

/// The bytes of a segment file of `units`, each followed by a line feed.
fn file_of(units: &[Vec<u8>]) -> Vec<u8> {
    units
        .iter()
        .flat_map(|u| u.iter().copied().chain(*b"\n"))
        .collect()
}

/// What a strict reader of a segment file says of it: the events it
/// restores, or the first refusal, `None` for a tear.
fn read_file(bytes: &[u8]) -> std::result::Result<Vec<Value>, Option<Flaw>> {
    let mut reader = Restoring::new(bytes, true);
    let mut out = Vec::new();
    loop {
        match reader.next_event() {
            Ok(Some(r)) => out.push(r.event),
            Ok(None) => break,
            Err(Fault::Flaw(flaw)) => return Err(Some(flaw)),
            Err(_) => return Err(None),
        }
    }
    reader.end().map_err(Some)?;
    Ok(out)
}

/// A block's bytes as `frame` holds them, its first line and its lines
/// changed by `head` and `lines`, and its `raw` and `stored` made theirs.
fn reblocked(
    frame: &Frame,
    head: &dyn Fn(&mut Value),
    lines: &dyn Fn(&mut Vec<Vec<u8>>),
) -> Vec<u8> {
    let (block, mut inner) = opened(frame);
    lines(&mut inner);
    let raw = file_of(&inner);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    std::io::Write::write_all(&mut z, &raw).unwrap();
    let z = z.finish().unwrap();
    let mut first = json!({"block": block, "v": FORMAT});
    first["block"]["raw"] = raw.len().into();
    first["block"]["stored"] = z.len().into();
    head(&mut first);
    let mut bytes = canonical::to_vec(&first);
    bytes.push(b'\n');
    bytes.extend(z);
    bytes
}

#[test]
fn a_file_of_lines_and_blocks_restores_every_event_and_every_break_is_refused() {
    let n = BLOCK_EVENTS as u64 + 2;
    let (values, mut events) = append(FORMAT, 0, &"0".repeat(64), n);
    let first = events[0].bytes.clone();
    frame_append(&mut events[1..]);
    let blocks: Vec<Vec<u8>> = events[1..]
        .iter()
        .filter_map(|e| e.frame.as_ref().filter(|f| f.first == e.position))
        .map(|f| f.bytes.clone())
        .collect();
    assert_eq!(blocks.len(), 2, "an append of more than a block's events");
    let file = |b: &[Vec<u8>]| file_of(&[vec![first.clone()], b.to_vec()].concat());
    assert_eq!(read_file(&file(&blocks)), Ok(values));
    let frame = |i: usize| events[1 + i * BLOCK_EVENTS].frame.clone().unwrap();
    let with = |i: usize, bytes: Vec<u8>| {
        let mut b = blocks.clone();
        b[i] = bytes;
        read_file(&file(&b)).err().flatten()
    };
    let head = |i: usize, f: &dyn Fn(&mut Value)| with(i, reblocked(&frame(i), f, &|_| ()));
    let inner = |i: usize, f: &dyn Fn(&mut Vec<Vec<u8>>)| with(i, reblocked(&frame(i), &|_| (), f));
    // An append torn between its blocks, and a block torn.
    assert_eq!(
        read_file(&file(&blocks[..1])),
        Err(Some(Flaw::AppendShort {
            end: Position(n - 1)
        }))
    );
    let mut torn = file(&blocks);
    torn.truncate(torn.len() - 9);
    assert_eq!(read_file(&torn), Err(None));
    // A changed byte of its stored bytes, its sizes, and its first line.
    let mut flipped = blocks[1].clone();
    *flipped.last_mut().unwrap() ^= 1;
    assert_eq!(with(1, flipped), Some(Flaw::BlockBytes));
    assert_eq!(
        head(0, &|h| h["block"]["raw"] = 7.into()),
        Some(Flaw::BlockBytes)
    );
    assert_eq!(
        head(0, &|h| h["block"]["hash"] = "0".repeat(64).into()),
        Some(Flaw::HashMismatch)
    );
    assert_eq!(
        head(0, &|h| h["block"]["prev"] = "1".repeat(64).into()),
        Some(Flaw::PrevHash)
    );
    assert!(matches!(
        head(1, &|h| h["block"]["first"] = 5.into()),
        Some(Flaw::Position { .. })
    ));
    assert_eq!(
        head(1, &|h| h["block"]["end"] = (n + 4).into()),
        Some(Flaw::AppendShort {
            end: Position(n - 1)
        })
    );
    assert_eq!(head(0, &|h| h["extra"] = 1.into()), Some(Flaw::NotABlock));
    assert_eq!(
        head(0, &|h| h["block"]["events"] = 0.into()),
        Some(Flaw::NotABlock)
    );
    // A line that holds what the block decides, a line too many or too
    // few, and a line changed.
    let position = inner(0, &|l| {
        let mut line: Value = serde_json::from_slice(&l[1]).unwrap();
        line["position"] = 1.into();
        l[1] = canonical::to_vec(&line);
    });
    assert_eq!(
        position,
        Some(Flaw::FramedTwice {
            member: "position".into()
        })
    );
    assert_eq!(inner(1, &|l| l.push(l[0].clone())), Some(Flaw::BlockBytes));
    assert_eq!(inner(1, &|l| drop(l.pop())), Some(Flaw::BlockBytes));
    let changed = inner(0, &|l| {
        let mut line: Value = serde_json::from_slice(&l[3]).unwrap();
        line["body"]["key"] = "other".into();
        l[3] = canonical::to_vec(&line);
    });
    assert_eq!(changed, Some(Flaw::HashMismatch));
    // A plain line among an append's blocks.
    let mut b = blocks.clone();
    b.insert(1, first.clone());
    assert_eq!(
        read_file(&file(&b)).err().flatten(),
        Some(Flaw::AppendShort {
            end: Position(n - 1)
        })
    );
}
