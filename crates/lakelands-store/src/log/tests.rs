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
/// opens its events here, so each refuses an event of another format,
/// older or newer, naming it and the one this release reads; and a lake
/// whose first or last event is in another format is refused by the
/// lake, where nothing is damaged.
#[test]
fn an_event_of_another_format_is_refused_by_both_numbers() {
    use lakelands_api::error::{Flaw, Situation};
    let reads = u64::from(LOG_FORMAT_VERSION);
    let (_, good) = created();
    for other in [reads - 1, reads + 1, 0] {
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
                matches!(e.situation(), Some(Situation::LogFormat { lake, found, reads: r })
                    if lake == "finance" && *found == other && *r == reads),
                "{e}"
            );
            let said = format!(
                "lake `finance` is written in log format {other}, and this release reads and \
                 writes format {reads} alone; nothing was read from it and nothing appended"
            );
            assert!(e.to_string().contains(&said), "{e}");
        }
    }
    assert!(one_format(&"finance".into(), Some(&(good.clone(), good))).is_ok());
    assert!(one_format(&"finance".into(), None).is_ok());
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
