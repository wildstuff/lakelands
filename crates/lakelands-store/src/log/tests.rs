use super::*;
use serde_json::json;

fn created() -> (Event, CanonicalEvent) {
    let d = Draft::system(Timestamp(0), EventBody::LakeCreated { format: 1 });
    seal(&"finance".into(), Position(0), &Digest::zero(), 1, d)
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

/// Every read of the log (opening a lake, the feed, `verify`) opens
/// its events here, so each refuses a version it does not know, naming
/// it and the newest it knows.
#[test]
fn a_newer_format_is_refused_by_name() {
    let (_, mut stored) = created();
    let mut v: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
    v["v"] = json!(7);
    stored.bytes = canonical::to_vec(&v);
    let e = open(&stored).unwrap_err().to_string();
    assert!(e.contains("format 7") && e.contains("format 6"), "{e}");
}

/// A version-2 event carrying what version 3 added is refused, naming
/// it; the same event written at 3 opens.
#[test]
fn a_version_2_event_that_carries_upstream_or_declares_date_bucket_is_refused() {
    let mut ingested = Draft::system(
        Timestamp(0),
        EventBody::RecordIngested {
            ty: "t".into(),
            key: "k".into(),
            version: lakelands_api::Version(1),
            patch: [("f".into(), json!(1))].into(),
        },
    );
    ingested.upstream = Some(lakelands_api::Upstream {
        lake: "other".into(),
        position: Position(9),
    });
    let declared = Draft::system(
        Timestamp(0),
        EventBody::Declared {
            declaration: serde_json::from_value(json!({
                "declare": "type", "name": "t",
                "fields": [
                    {"name": "at", "type": {"kind": "date_time"}},
                    {"name": "m", "type": {"kind": "date_time"}, "authority": "derived",
                     "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
                ]
            }))
            .unwrap(),
            schema_hash: None,
            version: 1,
        },
    );
    for (d, what) in [
        (ingested, "carries `upstream`"),
        (declared, "declares a call of `date_bucket`"),
    ] {
        let (_, stored) = seal_at(
            2,
            &"finance".into(),
            Position(1),
            &Digest::zero(),
            1,
            d.clone(),
        );
        let e = open(&stored).unwrap_err().to_string();
        assert!(e.contains(what) && e.contains("format 3 added"), "{e}");
        let (_, stored) = seal_at(3, &"finance".into(), Position(1), &Digest::zero(), 1, d);
        assert!(open(&stored).is_ok());
    }
}

#[test]
fn a_version_1_event_that_carries_rekeyed_is_refused() {
    let d = Draft::system(
        Timestamp(0),
        EventBody::Migrated {
            migration: lakelands_api::write::Migration {
                to: serde_json::from_value(json!({"name": "t"})).unwrap(),
                renames: vec![],
            },
            schema_hash: "h".into(),
            version: 2,
            report_id: "r".into(),
            rekeyed: vec![lakelands_api::write::Rekey {
                ty: "t".into(),
                from: "a".into(),
                to: "b".into(),
            }],
        },
    );
    let (_, stored) = seal(
        &"finance".into(),
        Position(0),
        &Digest::zero(),
        0,
        d.clone(),
    );
    let e = open(&stored).unwrap_err().to_string();
    assert!(e.contains("format 2 added"), "{e}");
    let (_, stored) = seal_at(2, &"finance".into(), Position(0), &Digest::zero(), 0, d);
    assert!(open(&stored).is_ok());
}

/// A version-3 event proposing a declaration is refused, naming what
/// version 4 added; the same event written at 4 opens.
#[test]
fn a_version_3_event_proposing_a_declaration_is_refused() {
    let d = Draft::system(
        Timestamp(0),
        EventBody::WriteProposed {
            proposal: "p".into(),
            write: lakelands_api::write::ProposedWrite::Declare {
                declarations: vec![
                    serde_json::from_value(json!({"declare": "type", "name": "t"})).unwrap(),
                ],
            },
            why: "w".into(),
        },
    );
    let (_, stored) = seal_at(
        3,
        &"finance".into(),
        Position(1),
        &Digest::zero(),
        1,
        d.clone(),
    );
    let e = open(&stored).unwrap_err().to_string();
    assert!(
        e.contains("proposes a declaration or a migration") && e.contains("format 4 added"),
        "{e}"
    );
    let (_, stored) = seal_at(4, &"finance".into(), Position(1), &Digest::zero(), 1, d);
    assert!(open(&stored).is_ok());
}

/// `upstream` on any kind but `record_ingested` is refused, naming the
/// kind, whatever the version.
#[test]
fn upstream_on_another_kind_is_refused() {
    let mut retired = Draft::system(
        Timestamp(0),
        EventBody::RecordRetired {
            ty: "t".into(),
            key: "k".into(),
            version: lakelands_api::Version(2),
        },
    );
    retired.upstream = Some(lakelands_api::Upstream {
        lake: "other".into(),
        position: Position(9),
    });
    let (_, stored) = seal_at(
        3,
        &"finance".into(),
        Position(1),
        &Digest::zero(),
        1,
        retired,
    );
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
    let d = Draft::system(
        Timestamp(0),
        EventBody::LeaseTaken {
            fence: 1,
            holder: "t".into(),
        },
    );
    let (second, _) = seal(&"finance".into(), Position(1), &first.hash, 1, d.clone());
    assert!(check_link(Some((&first.position, &first.hash)), &second).is_ok());
    let (bad, _) = seal(&"finance".into(), Position(1), &Digest::zero(), 1, d);
    assert!(check_link(Some((&first.position, &first.hash)), &bad).is_err());
}
