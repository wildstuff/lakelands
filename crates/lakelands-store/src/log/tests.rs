use super::*;
use serde_json::json;

fn created() -> (Event, CanonicalEvent) {
    let d = Draft::system(
        Timestamp(0),
        EventBody::LakeCreated {
            format: LOG_FORMAT_VERSION,
        },
    );
    seal(&"finance".into(), Position(0), &Digest::zero(), 1, d)
}

/// A system draft at time 0 carrying `body`.
fn draft(body: EventBody) -> Draft {
    Draft::system(Timestamp(0), body)
}

#[test]
fn a_sealed_event_opens_to_itself() {
    let (event, stored) = created();
    assert_eq!(open(&stored).unwrap(), event);
    assert_eq!(event.hash.0.len(), 64);
    assert_eq!(event.id, positional_id(&"finance".into(), Position(0)));
}

#[test]
fn the_hash_covers_the_event_without_its_top_level_hash() {
    let (event, stored) = created();
    let mut v: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
    v.as_object_mut().unwrap().remove("hash");
    assert_eq!(event.hash.0, canonical::sha256_hex(&v));
}

#[test]
fn a_changed_byte_is_caught() {
    let (_, mut stored) = created();
    let text = String::from_utf8(stored.bytes.clone())
        .unwrap()
        .replace("\"format\":1", "\"format\":2");
    stored.bytes = text.into_bytes();
    assert!(open(&stored).is_err());
}

#[test]
fn non_canonical_bytes_are_refused() {
    let (_, mut stored) = created();
    let mut spaced = b" ".to_vec();
    spaced.extend(&stored.bytes);
    stored.bytes = spaced;
    assert!(open(&stored).is_err());
}

#[test]
fn a_member_in_another_written_form_is_refused_even_with_a_matching_hash() {
    let (_, stored) = created();
    let mut v: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
    v["known_at"] = json!("1970-01-01T00:00:00Z");
    v.as_object_mut().unwrap().remove("hash");
    let hash = canonical::sha256_hex(&v);
    v["hash"] = json!(hash);
    let forged = CanonicalEvent {
        hash: Digest(hash),
        bytes: canonical::to_vec(&v),
        ..stored
    };
    assert!(open(&forged).is_err());
}

/// Every read of the log (the feed, `verify`, a rebuild, an import)
/// opens its events here, so each refuses an event of a format this
/// release does not read, older or newer, naming it and the one this
/// release writes; and a lake whose first or last event is in such a
/// format is refused by the lake, where nothing is damaged, and so is
/// one whose newest event is in a format older than its first's.
#[test]
fn an_event_of_another_format_is_refused_by_both_numbers() {
    use lakelands_api::error::{Flaw, Situation};
    use lakelands_api::event::OLDEST_LOG_FORMAT;
    let (reads, oldest) = (u64::from(LOG_FORMAT_VERSION), u64::from(OLDEST_LOG_FORMAT));
    let (_, good) = created();
    for other in [oldest - 1, reads + 1, 0] {
        let mut stored = good.clone();
        let mut v: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
        v["v"] = json!(other);
        stored.bytes = canonical::to_vec(&v);
        let e = open(&stored).unwrap_err();
        assert!(
            matches!(e.situation(), Some(Situation::Damaged {
                flaw: Flaw::FormatOther { v, reads: r }, ..
            }) if *v == other && *r == reads),
            "{e}"
        );
        // At the tip and at position 0 alike.
        for ends in [(good.clone(), stored.clone()), (stored, good.clone())] {
            let e = one_format(&"finance".into(), Some(&ends)).unwrap_err();
            assert!(
                matches!(e.situation(), Some(Situation::LogFormat { lake, found, reads: r, oldest: o })
                    if lake == "finance" && *found == other && *r == reads && *o == Some(oldest)),
                "{e}"
            );
            let said = format!(
                "lake `finance` is written in log format {other}, and this release reads log \
                 formats {oldest} to {reads} and writes {reads}; nothing was read from it and \
                 nothing appended"
            );
            assert!(e.to_string().contains(&said), "{e}");
        }
    }
    // An older format this release reads opens, at its own number; a tip
    // below the first event's format is not read in any.
    let mut older = good.clone();
    let mut v: serde_json::Value = serde_json::from_slice(&older.bytes).unwrap();
    (v["v"], v["body"]["format"]) = (json!(oldest), json!(oldest));
    v.as_object_mut().unwrap().remove("hash");
    let hash = canonical::sha256_hex(&v);
    v["hash"] = json!(hash);
    (older.hash, older.bytes) = (Digest(hash), canonical::to_vec(&v));
    assert_eq!(open(&older).map(|e| e.v), Ok(OLDEST_LOG_FORMAT));
    let ends = (older.clone(), older.clone());
    assert_eq!(
        one_format(&"finance".into(), Some(&ends)).ok(),
        Some(OLDEST_LOG_FORMAT)
    );
    let fell = one_format(&"finance".into(), Some(&(good.clone(), older))).unwrap_err();
    assert!(
        matches!(fell.situation(), Some(Situation::LogFormat { found, oldest: Some(o), .. })
            if *found == oldest && *o == oldest),
        "{fell}"
    );
    let ends = (good.clone(), good);
    assert_eq!(
        one_format(&"finance".into(), Some(&ends)).ok(),
        Some(LOG_FORMAT_VERSION)
    );
    assert_eq!(
        one_format(&"finance".into(), None).ok(),
        Some(LOG_FORMAT_VERSION)
    );
}

/// A lake says the format it was created in at position 0. One that
/// says another than its release reads was made by another release,
/// though every line carries the release's `v`: a read of the event
/// refuses it by its own flaw, and the lake is not opened.
#[test]
fn a_lake_created_in_another_format_is_refused_though_its_lines_are_in_this_one() {
    use lakelands_api::error::{Flaw, Situation};
    let reads = u64::from(LOG_FORMAT_VERSION);
    let (_, good) = created();
    for other in [reads - 1, reads + 1] {
        let mut v: serde_json::Value = serde_json::from_slice(&good.bytes).unwrap();
        v["body"]["format"] = json!(other);
        v.as_object_mut().unwrap().remove("hash");
        let hash = canonical::sha256_hex(&v);
        v["hash"] = json!(hash);
        let stored = CanonicalEvent {
            hash: Digest(hash),
            bytes: canonical::to_vec(&v),
            ..good.clone()
        };
        let e = open(&stored).unwrap_err();
        assert!(
            matches!(e.situation(), Some(Situation::Damaged {
                flaw: Flaw::CreatedInAnother { format, reads: r }, ..
            }) if *format == other && *r == reads),
            "{e}"
        );
        let e = one_format(&"finance".into(), Some(&(stored, good.clone()))).unwrap_err();
        assert!(
            matches!(e.situation(), Some(Situation::LogFormat { found, reads: r, .. })
                if *found == other && *r == reads),
            "{e}"
        );
    }
}

/// `upstream` on any kind but `record_ingested` is refused, naming the
/// kind.
#[test]
fn upstream_on_another_kind_is_refused() {
    let mut retired = draft(EventBody::RecordRetired {
        ty: "t".into(),
        key: "k".into(),
        version: lakelands_api::Version(2),
    });
    retired.upstream = Some(lakelands_api::Upstream {
        lake: "other".into(),
        position: Position(9),
    });
    let (_, stored) = seal(&"finance".into(), Position(1), &Digest::zero(), 1, retired);
    let e = open(&stored).unwrap_err().to_string();
    assert!(
        e.contains("`record_retired`") && e.contains("only a `record_ingested`"),
        "{e}"
    );
}

#[test]
fn links_are_checked() {
    let (first, _) = created();
    assert!(check_link(None, &first).is_ok());
    let d = draft(EventBody::LeaseTaken {
        fence: 1,
        holder: "t".into(),
    });
    let (second, _) = seal(&"finance".into(), Position(1), &first.hash, 1, d.clone());
    assert!(check_link(Some((&first.position, &first.hash)), &second).is_ok());
    let (bad, _) = seal(&"finance".into(), Position(1), &Digest::zero(), 1, d);
    assert!(check_link(Some((&first.position, &first.hash)), &bad).is_err());
}

/// A record carrying a 384-element vector of doubles, f32 widened and
/// short decimals among them, seals and opens to itself: the open pass
/// that checks the converted value is skipped where the conversion
/// changed nothing, and the bytes still verify.
#[test]
fn a_vector_record_opens_to_itself() {
    let vector: Vec<serde_json::Value> = (0..384_u32)
        .map(|i| match i % 3 {
            0 => json!(f64::from(i as f32 / 7.0 + 0.5)),
            1 => json!(f64::from(i) / 1000.0 - 0.1003),
            _ => json!(-(f64::from(i)) * 1e-9),
        })
        .collect();
    let mut patch = std::collections::BTreeMap::new();
    patch.insert("embedding".into(), serde_json::Value::Array(vector));
    let d = draft(EventBody::RecordUpserted {
        ty: "note".into(),
        key: "n1".into(),
        version: lakelands_api::Version(1),
        patch,
        made: true,
    });
    let (event, stored) = seal(&"finance".into(), Position(0), &Digest::zero(), 1, d);
    assert_eq!(open(&stored).unwrap(), event);
    // A flipped digit in the vector is still caught.
    let mut bad = stored.clone();
    let at = bad.bytes.iter().position(|&b| b == b'.').unwrap() + 1;
    bad.bytes[at] = if bad.bytes[at] == b'1' { b'2' } else { b'1' };
    assert!(open(&bad).is_err());
}
