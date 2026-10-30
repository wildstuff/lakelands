//! The log format's test vectors (`docs/log-format/vectors/`), recomputed
//! from their bytes by this crate's canonical module and schema hash.
//!
//! The vectors are the document's worked example: a small lake's log in
//! its stored form, with every hash, both kinds of content-derived event
//! id, the segment digests and a view's schema hash. This test reads them
//! and recomputes every one, so `docs/log-format.md` and the code cannot
//! drift apart without it going red. `verify.py` beside the vectors checks
//! the same bytes with an implementation that shares no code with this
//! one.
//!
//! The vectors are one lake's log in the format version this release
//! writes, `LOG_FORMAT_VERSION`, kept in `vectors/<N>/`
//! (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`):
//! the two segments, then the events of `further.json`, each following the
//! one before it and each named for what it shows. A format step copies
//! the directory to the next number and stamps the copy
//! (`verify.py --stamp N`); the directory of every format a release
//! shipped stays as its tag holds it, and its own `verify.py` is run here.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::{Flaw, Situation};
use lakelands_api::event::{
    ChangeKind, DepthBound, Digest, Event, EventBody, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT,
};
use lakelands_api::expr::Condition;
use lakelands_api::id::{KeyId, Position};
use lakelands_api::ontology::{AtIngest, TypeDecl, VerbScope};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, ProposedWrite};
use lakelands_core::{canonical, log, shape};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// The vector directories, one a format, by number.
fn every_format() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors")
}

fn vectors() -> PathBuf {
    every_format().join(LOG_FORMAT_VERSION.to_string())
}

fn read_json(name: &str) -> Value {
    serde_json::from_slice(&fs::read(vectors().join(name)).unwrap()).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    canonical::hex(&Sha256::digest(bytes))
}

fn str_of<'a>(v: &'a Value, member: &str) -> &'a str {
    v[member]
        .as_str()
        .unwrap_or_else(|| panic!("`{member}` is a string"))
}

/// The date-time a log holds is exactly the one form of §1.3.
fn assert_one_form(t: &str, at: u64) {
    let parsed = Timestamp::parse(t).unwrap_or_else(|e| panic!("position {at}: {e}"));
    assert_eq!(
        parsed.to_string(),
        t,
        "position {at}: `{t}` is not the one form"
    );
}

/// One entry of `further.json`: what its line shows, the line, and, of
/// an event of the lake, the hash the file repeats.
struct Further {
    shows: String,
    line: String,
    hash: Option<String>,
}

/// The entries `further.json` holds under `member`: its `events`, the
/// lake's lines after the segments, or its `misplaced` lines.
fn further(member: &str) -> Vec<Further> {
    let entries = read_json("further.json")[member]
        .as_array()
        .unwrap()
        .clone();
    let entry = |e: &Value| Further {
        shows: str_of(e, "shows").to_owned(),
        line: str_of(e, "line").to_owned(),
        hash: e["hash"].as_str().map(str::to_owned),
    };
    entries.iter().map(entry).collect()
}

/// The hash of the line with its `hash` member left out.
fn hash_of(line: &str) -> String {
    let mut body: Value = serde_json::from_str(line).unwrap();
    body.as_object_mut().unwrap().remove("hash");
    sha(&canonical::to_vec(&body))
}

/// Every line of the vectors' lake in the order its positions run: the
/// segments, then the events of `further.json`.
fn chain() -> Vec<Event> {
    let mut lines: Vec<String> = Vec::new();
    for seg in read_json("expected.json")["segments"].as_array().unwrap() {
        let text = fs::read_to_string(vectors().join(str_of(seg, "file"))).unwrap();
        lines.extend(text.lines().map(str::to_owned));
    }
    lines.extend(further("events").into_iter().map(|e| e.line));
    lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A release writes one format: every line of the vectors' lake carries
/// `LOG_FORMAT_VERSION` as `v`, from position 0, whose `lake_created`
/// says the same number. The lines are one chain without a gap, and none
/// carries a member where it never sits.
#[test]
fn every_line_of_the_lake_is_in_the_one_format_this_release_writes() {
    let chain = chain();
    let mut prev = "0".repeat(64);
    for (at, event) in chain.iter().enumerate() {
        assert_eq!(event.position.0, at as u64, "the positions run on");
        assert_eq!(event.prev.0, prev, "position {at}: prev");
        assert_eq!(event.v, LOG_FORMAT_VERSION, "position {at}: its format");
        assert_eq!(log::misplaced(event), None, "position {at}");
        prev = event.hash.0.clone();
    }
    assert!(
        matches!(&chain[0].body, EventBody::LakeCreated { format } if *format == LOG_FORMAT_VERSION),
        "position 0 says the lake's format: {:?}",
        chain[0].body
    );
}

/// The page and the release name one format: `docs/log-format.md` opens
/// with `**Format version N.**`, and N is `LOG_FORMAT_VERSION`.
#[test]
fn the_page_names_the_format_this_release_writes() {
    let page = fs::read_to_string(vectors().join("../../../log-format.md")).unwrap();
    let named = page
        .lines()
        .find_map(|l| l.strip_prefix("**Format version "))
        .and_then(|rest| rest.split_once(".**"))
        .expect("the page opens with `**Format version N.**`")
        .0;
    assert_eq!(
        named.parse(),
        Ok(LOG_FORMAT_VERSION),
        "the page names format {named}"
    );
}

/// The independent reader reads the format this release writes:
/// `verify.py` holds its one format as `FORMAT = N`, the line its
/// `--stamp N` rewrites, and N is `LOG_FORMAT_VERSION`.
#[test]
fn the_independent_verifier_reads_the_format_this_release_writes() {
    let script = fs::read_to_string(vectors().join("verify.py")).unwrap();
    let named = script
        .lines()
        .find_map(|l| l.strip_prefix("FORMAT = "))
        .expect("verify.py holds `FORMAT = N`");
    assert_eq!(
        named.parse(),
        Ok(LOG_FORMAT_VERSION),
        "verify.py reads format {named}"
    );
}

/// The independent reader's own run over the vectors, as their README
/// says to run it: `python3 verify.py` in their directory. It prints one
/// line a check and exits with success, and it made every check it
/// makes: a check dropped from it, or a line of `further.json` it no
/// longer reads, moves the count. Fails when `python3` is missing, as
/// the tests that run it over a lake directory do.
#[test]
fn the_independent_verifier_passes_every_check_of_the_vectors() {
    let run = std::process::Command::new("python3")
        .arg("verify.py")
        .current_dir(vectors())
        .output()
        .expect("this test runs verify.py over the vectors and needs python3");
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "verify.py refused the vectors:\n{said}"
    );
    let checks = said.lines().filter(|l| l.starts_with("ok   ")).count();
    assert_eq!(
        checks,
        said.lines().count(),
        "every line is a check that passed:\n{said}"
    );
    assert_eq!(checks, 778, "the checks verify.py makes:\n{said}");
}

/// An integer beyond ±(2^53 − 1) is one not every reader reads exactly
/// (log-format §1.2). `verify.py` answers a line that holds one with a
/// verdict at the line's position, and with no traceback: the first
/// segment's number at position 5 written as 2^53 + 1.
#[test]
fn the_independent_verifier_refuses_an_integer_too_large_at_its_position() {
    let name = "log/00000000000000000000.jsonl";
    let segment = fs::read_to_string(vectors().join(name)).unwrap();
    let large = segment.replacen("\"weight\":1e+21", "\"weight\":9007199254740993", 1);
    assert_ne!(large, segment, "position 5 holds the number");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("vectors-integer-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("log")).unwrap();
    fs::write(dir.join(name), large).unwrap();
    let run = std::process::Command::new("python3")
        .arg(vectors().join("verify.py"))
        .arg(&dir)
        .output()
        .expect("this test runs verify.py over a directory and needs python3");
    let said = String::from_utf8_lossy(&run.stdout);
    let trace = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(1), "{said}{trace}");
    assert!(
        said.contains("FAIL position 5: ")
            && said.contains("the integer 9007199254740993 in it is beyond ±(2^53 − 1)"),
        "verify.py refuses the integer at position 5:\n{said}"
    );
    assert_eq!(trace, "", "a verdict, not a traceback");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_hash_id_digest_and_schema_hash_recomputes() {
    let expected = read_json("expected.json");
    let segments = expected["segments"].as_array().unwrap();

    // Read every segment: its digest, and each line as canonical bytes.
    let mut events: Vec<(Value, Vec<u8>)> = Vec::new();
    for seg in segments {
        let raw = fs::read(vectors().join(str_of(seg, "file"))).unwrap();
        assert_eq!(sha(&raw), str_of(seg, "digest"), "{}", seg["file"]);
        assert_eq!(
            raw.last(),
            Some(&b'\n'),
            "every event ends with a line feed"
        );
        for line in raw[..raw.len() - 1].split(|b| *b == b'\n') {
            let value: Value = serde_json::from_slice(line).unwrap();
            assert_eq!(
                canonical::to_vec(&value),
                line,
                "position {}: the stored line is its canonical bytes",
                value["position"]
            );
            events.push((value, line.to_vec()));
        }
    }

    let want = expected["events"].as_array().unwrap();
    assert_eq!(events.len(), want.len());
    let mut prev = "0".repeat(64);
    let mut key_fields: HashMap<String, Vec<String>> = HashMap::new();
    for (i, ((value, _), want)) in events.iter().zip(want).enumerate() {
        let at = i as u64;
        let event: Event = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("position {at} reads as an Event: {e}"));
        assert_eq!(event.position.0, at);
        assert_eq!(event.prev.0, prev, "position {at}: prev");

        let mut unhashed = value.clone();
        unhashed.as_object_mut().unwrap().remove("hash");
        let hash = canonical::sha256_hex(&unhashed);
        assert_eq!(hash, event.hash.0, "position {at}: hash");
        assert_eq!(hash, str_of(want, "hash"), "position {at}: expected hash");
        assert_eq!(event.id.0, str_of(want, "id"), "position {at}: id");

        assert_one_form(str_of(value, "known_at"), at);
        if let Some(seen_at) = value["seen_at"].as_str() {
            assert_one_form(seen_at, at);
        }

        match (str_of(want, "id_from"), &event.body) {
            (
                "ingest",
                EventBody::RecordIngested {
                    ty,
                    key,
                    version,
                    patch,
                    ..
                },
            ) => {
                let source = event
                    .source
                    .as_ref()
                    .expect("an ingest event names its source");
                let id = log::ingest_id(source, ty, key, version.0 - 1, patch);
                assert_eq!(id, event.id, "position {at}: ingest id");
            }
            ("authored", _) => {
                let key = event.idempotency_key.as_ref().expect("an idempotency key");
                let id = log::authored_id(&event.writer, key);
                assert_eq!(id, event.id, "position {at}: authored id");
            }
            ("random", _) => {}
            (from, body) => panic!("position {at}: `{from}` id on a {}", body.kind()),
        }

        match &event.body {
            EventBody::RecordUpserted { ty, key, patch, .. }
            | EventBody::RecordIngested { ty, key, patch, .. } => {
                let fields = &key_fields[&ty.0];
                if !fields.is_empty() && fields.iter().all(|f| patch.contains_key(f.as_str())) {
                    let derived: Vec<String> = fields
                        .iter()
                        .map(|f| {
                            log::key_component(&patch[f.as_str()]).expect("a key field's value")
                        })
                        .collect();
                    assert_eq!(
                        derived.join(":"),
                        key.0,
                        "position {at}: key from its key fields"
                    );
                }
            }
            EventBody::Declared {
                declaration: Declaration::Type(ty),
                schema_hash: Some(stored),
                ..
            } => {
                key_fields.insert(
                    ty.name.0.clone(),
                    ty.key.iter().map(|f| f.0.clone()).collect(),
                );
                let computed = shape::schema_hash(ty).unwrap();
                assert_eq!(&computed, stored, "position {at}: schema hash");
                let listed = expected["schema_hashes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["position"] == at)
                    .expect("the schema hash is listed");
                assert_eq!(computed, str_of(listed, "schema_hash"));
            }
            EventBody::Checkpoint {
                through,
                hash,
                segments: listed,
            } => {
                assert_eq!(
                    hash.0, prev,
                    "the checkpoint repeats the sealed segment's last hash"
                );
                assert_eq!(through.0 + 1, at, "the checkpoint opens the next segment");
                let sealed: Vec<Value> = segments
                    .iter()
                    .filter(|s| s["last"].as_u64().unwrap() <= through.0)
                    .map(|s| {
                        serde_json::json!({ "first": s["first"], "last": s["last"], "digest": s["digest"] })
                    })
                    .collect();
                assert_eq!(serde_json::to_value(listed).unwrap(), Value::Array(sealed));
            }
            _ => {}
        }
        prev = event.hash.0;
    }
}

#[test]
fn a_view_hashes_its_conditions_in_typed_form() {
    let vector = read_json("schema-hash.json");
    let ty: TypeDecl = serde_json::from_value(vector["declaration"].clone()).unwrap();
    let shape_bytes = canonical::to_vec(&vector["shape"]);
    assert_eq!(
        String::from_utf8(shape_bytes.clone()).unwrap(),
        str_of(&vector, "shape_canonical")
    );
    assert_eq!(sha(&shape_bytes), str_of(&vector, "schema_hash"));
    assert_eq!(
        shape::schema_hash(&ty).unwrap(),
        str_of(&vector, "schema_hash"),
        "the declaration, conditions written as text, hashes to the vector's shape"
    );
    let keyless = TypeDecl { key: vec![], ..ty };
    assert_eq!(
        shape::schema_hash(&keyless).unwrap(),
        str_of(&vector, "key_left_out_schema_hash"),
        "a grouped view hashes alike with its `key` left out"
    );
}

/// What the events of `further.json` show, in the order of their
/// positions: every name [`shows`] has a check for, once. The file's
/// lines are held to this list, so a line that is gone from the file, a
/// line added to it and a line that shows another's thing are each
/// refused, as `verify.py` refuses them by its own list.
const SHOWN: [&str; 89] = [
    "rekeyed",
    "upstream",
    "date_bucket",
    "proposed_declare",
    "proposed_migrate",
    "annotations",
    "derived_relation",
    "max_depth",
    "retired_guard",
    "view_params",
    "verb_on_type",
    "verb_called_on_type",
    "view_merge",
    "value_type",
    "proposed_tightening",
    "floor",
    "date_bucket_by_width",
    "in_set",
    "relation_renames",
    "view_kept",
    "made_again",
    "restated",
    "initial",
    "with_the_set",
    "value_type_migrated",
    "proposed_renames",
    "pinned",
    "date_field",
    "date_value_type",
    "date_value",
    "output_carried",
    "output_named",
    "lake_upgraded",
    "erasable",
    "sealed_ingest",
    "sealed_output",
    "set_across",
    "trigger_across",
    "constraints",
    "source_forgotten",
    "verb_authority",
    "setting_declared",
    "setting_group",
    "proposed_plan",
    "delivery",
    "function_risk",
    "computed_by_function",
    "computed_write",
    "proposed_compute",
    "withdrawn",
    "retired_by_verbs",
    "retire_effect",
    "trigger_change_guard",
    "slip_write",
    "fired_change",
    "reaction_write",
    "trigger_dropped",
    "untyped_input",
    "values_removed",
    "removal_retires",
    "delivery_own",
    "removed_before",
    "removal_by_time_retires",
    "proposed_migrate_recomputed",
    "sealed_apart",
    "sealed_apart_output",
    "removed_apart",
    "removal_apart_retires",
    "admits",
    "removed_seen_before",
    "verb_granted",
    "removed_deliveries",
    "verb_preconditions",
    "proposed_records",
    "quarantined",
    "quarantined_again",
    "quarantined_sealed",
    "resubmission_lands",
    "closed_resubmitted",
    "closed_dismissed",
    "quarantined_constraint",
    "lands",
    "closed_landed",
    "quarantined_removed",
    "removes_quarantined",
    "closed_removed",
    "setting_dated",
    "setting_corrected",
    "setting_at",
];

/// The last position of the vectors' lake.
const LAST: u64 = 96;

/// The members `further.json`'s misplaced lines misplace, in the file's
/// order: every name the test of them has a check for, once.
const MISPLACED: [&str; 7] = [
    "upstream_on_another_kind",
    "cause_in_context",
    "output_twice",
    "sealed_on_another_kind",
    "delivery_on_another_kind",
    "sealed_for_another_source",
    "on_behalf_of_in_context",
];

/// Whether the event is what its entry of `further.json` says it shows,
/// read from the contract's event where the contract has a member for it
/// and from the line where it is a call inside an expression. A name
/// this does not know is no vector: it panics.
fn shows(what: &str, event: &Event, line: &str) -> bool {
    use Declaration as D;
    use EventBody as B;
    let (declared, stored) = match &event.body {
        B::Declared {
            declaration,
            schema_hash,
            ..
        } => (Some(declaration), schema_hash.as_deref()),
        _ => (None, None),
    };
    let ty = match declared {
        Some(D::Type(t)) => Some(t),
        _ => None,
    };
    let view = ty.and_then(|t| t.view.as_ref());
    let proposed = match &event.body {
        B::WriteProposed { write, .. } => Some(write),
        _ => None,
    };
    // The schema hash the event stores is the one `changed` hashes to.
    let hashes_as =
        |changed: &TypeDecl| stored == Some(shape::schema_hash(changed).unwrap().as_str());
    match what {
        "rekeyed" => matches!(&event.body, B::Migrated { rekeyed, .. }
            if rekeyed.len() == 1 && rekeyed[0].to.as_str() == "DE%3A01"),
        // The id leaves `upstream` out.
        "upstream" => match (&event.body, &event.source) {
            (
                B::RecordIngested {
                    ty,
                    key,
                    version,
                    patch,
                    ..
                },
                Some(source),
            ) => {
                event.upstream.is_some()
                    && event.id == log::ingest_id(source, ty, key, version.0 - 1, patch)
            }
            _ => false,
        },
        "date_bucket" => ty.is_some() && line.contains("date_bucket("),
        "proposed_declare" => matches!(proposed, Some(ProposedWrite::Declare { declarations, .. })
            if !declarations.is_empty()),
        "proposed_migrate" => {
            matches!(proposed, Some(ProposedWrite::Migrate { report, .. }) if report.lossless)
        }
        // A kept plan carries its own id: the plan's, at its head (062).
        "proposed_plan" => matches!(proposed, Some(ProposedWrite::Declare { plan: Some(p), .. })
        if p.types.iter().any(|t| t.recomputed.is_some_and(|n| n > 0) && !t.unlive.is_empty())
            && p.report_id == lakelands_core::canonical::sha256_hex(&serde_json::json!({
                "plan": lakelands_api::write::Plan { report_id: String::new(), ..(**p).clone() },
                "head": p.at.0,
            }))),
        "proposed_records" => matches!(proposed, Some(ProposedWrite::Records { ty, to, plan, .. })
            if ty.as_str() == "ticket"
                && to == &lakelands_api::write::ChangeTo::Set(
                    [("title".into(), serde_json::json!("closed"))].into_iter().collect())
                && plan.records.as_ref().is_some_and(|r| r.selected == 2 && r.fields.len() == 1)),
        // Annotations are not shape.
        "annotations" => ty.is_some_and(|t| {
            let mut bare = t.clone();
            bare.annotations.clear();
            bare.fields.iter_mut().for_each(|f| f.annotations.clear());
            bare.relations
                .iter_mut()
                .for_each(|r| r.annotations.clear());
            bare != *t && hashes_as(&bare)
        }),
        // `derived` is not shape.
        "derived_relation" => ty.is_some_and(|t| {
            let mut carried = t.clone();
            carried.relations.iter_mut().for_each(|r| r.derived = None);
            carried != *t && hashes_as(&carried)
        }),
        "max_depth" => matches!(declared, Some(D::Trigger(t)) if t.max_depth == Some(1)),
        "retired_guard" => matches!(declared, Some(D::Trigger(_))) && line.contains("retired()"),
        "view_params" => view.is_some_and(|v| !v.params.is_empty()) && line.contains("param("),
        "verb_on_type" => matches!(declared, Some(D::Verb(v)) if v.scope == VerbScope::Type),
        "verb_called_on_type" => matches!(&event.body, B::VerbCalled { key: None, .. }),
        "view_merge" => view.is_some_and(|v| !v.merge.is_empty()),
        "value_type" => matches!(declared, Some(D::ValueType(_))),
        "proposed_tightening" => {
            matches!(proposed, Some(ProposedWrite::MigrateValueType { renames, .. }) if renames.is_empty())
        }
        "floor" => ty.is_some() && line.contains("floor("),
        "date_bucket_by_width" => ty.is_some() && line.contains("date_bucket(on, 7d,"),
        "in_set" => matches!(declared, Some(D::ObjectSet(_))) && line.contains("in_set("),
        "relation_renames" => {
            matches!(&event.body, B::Migrated { .. }) && line.contains("\"relation_renames\":")
        }
        "view_kept" => matches!(&event.body, B::RecordUpserted { ty, key, version, made: true, .. }
            if ty.0 == "sys.setting" && key.0.starts_with("view_kept.") && version.0 == 1),
        "made_again" => {
            matches!(&event.body, B::RecordUpserted { version, made: true, .. } if version.0 > 1)
        }
        "restated" => {
            matches!(proposed, Some(ProposedWrite::Migrate { .. }))
                && line.contains("\"restated\":")
        }
        "initial" => ty.is_some_and(|t| t.fields.iter().any(|f| f.initial.is_some())),
        "with_the_set" => matches!(
            &event.body,
            B::Declared {
                with_the_set: true,
                ..
            }
        ),
        "value_type_migrated" => {
            matches!(&event.body, B::ValueTypeMigrated { renames, .. } if !renames.is_empty())
        }
        "proposed_renames" => {
            matches!(proposed, Some(ProposedWrite::MigrateValueType { renames, .. }) if !renames.is_empty())
        }
        "pinned" => matches!(
            &event.body,
            B::WriteProposed {
                write: ProposedWrite::Upsert { .. },
                pinned: true,
                ..
            }
        ),
        "date_field" => ty.is_some() && line.contains("\"kind\":\"date\""),
        "date_value_type" => {
            matches!(declared, Some(D::ValueType(_))) && line.contains("\"kind\":\"date\"")
        }
        "date_value" => {
            matches!(&event.body, B::RecordUpserted { .. }) && line.contains("\"2026-07-05\"")
        }
        "output_carried" => matches!(
            &event.body,
            B::OutputRecorded {
                output: Some(_),
                output_at: None,
                ..
            }
        ),
        "output_named" => matches!(
            &event.body,
            B::OutputRecorded {
                output: None,
                output_at: Some(_),
                ..
            }
        ),
        "lake_upgraded" => matches!(
            &event.body,
            B::LakeUpgraded { from, to } if *to == event.v && *from + 1 == *to
        ),
        "constraints" => matches!(declared, Some(D::Type(t)) if {
            let at: Vec<_> = t.constraints.iter().map(|c| c.at_ingest).collect();
            at == [AtIngest::Admit, AtIngest::Refuse] && line.contains("previous(status)")
        }),
        "erasable" => {
            matches!(declared, Some(D::Type(t)) if t.fields.iter().any(|f| f.erasable))
        }
        "sealed_ingest" => {
            let sealed = event.sealed.as_ref().map(|s| s.fields.as_slice());
            matches!(&event.body, B::RecordIngested { .. }) && sealed == Some(&["email".into()])
        }
        "sealed_output" => {
            let source = event.sealed.as_ref().and_then(|s| s.source.as_ref());
            matches!(
                &event.body,
                B::OutputRecorded {
                    output: Some(_),
                    ..
                }
            ) && source.is_some()
        }
        "set_across" => {
            matches!(declared, Some(D::ObjectSet(_))) && line.contains("shelf.capacity")
        }
        "trigger_across" => {
            matches!(declared, Some(D::Trigger(_)))
                && line.contains("exists(shelf)")
                && line.contains("shelf.capacity")
        }
        "source_forgotten" => matches!(
            &event.body,
            B::SourceForgotten { source, keys }
                if source == "crm" && keys.len() == 1 && keys[0].as_str() == "1"
        ),
        "verb_authority" => matches!(declared, Some(D::Type(t))
            if t.fields.iter().any(|f| f.authority == lakelands_api::ontology::Authority::Verb)),
        "admits" => matches!(declared, Some(D::Type(t))
            if t.fields.iter().any(|f| f.admits.iter().map(|s| s.as_str()).eq(["samples"]))),
        "retired_by_verbs" => matches!(declared, Some(D::Type(t))
            if t.retired_by == lakelands_api::ontology::RetiredBy::Verbs),
        "verb_preconditions" => matches!(declared, Some(D::Verb(v))
            if v.requires == Some(Condition::Text("payload.amount > 0".into()))
                && v.preconditions.iter().map(|p| p.name.as_str()).eq(["not_overpaid"])),
        "verb_granted" => {
            matches!(declared, Some(D::Verb(v)) if v.granted && v.name.as_str() == "void")
        }
        "retire_effect" => matches!(declared, Some(D::Verb(v))
            if v.effect == lakelands_api::ontology::VerbEffect::Retire),
        "setting_declared" => matches!(declared, Some(D::Setting(s))
            if s.key == "vat_rate" && s.group.as_deref() == Some("operator")),
        "setting_group" => matches!(&event.body, B::RecordUpserted { ty, patch, .. }
            if ty.as_str() == lakelands_api::seam::SETTING_TYPE
                && patch.get("group").and_then(|g| g.as_str()) == Some("operator")),
        "setting_dated" | "setting_corrected" => {
            let value = if what == "setting_dated" {
                "0.21"
            } else {
                "0.2"
            };
            let periods = serde_json::json!([{"from": "2027-01-01", "value": value}]);
            matches!(&event.body, B::RecordUpserted { ty, patch, .. }
                if ty.as_str() == lakelands_api::seam::SETTING_TYPE
                    && patch.keys().eq(["periods"]) && patch["periods"] == periods)
        }
        "setting_at" => ty.is_some() && line.contains("setting('vat_rate', sold_on)"),
        "delivery" => {
            matches!(&event.body, B::RecordIngested { .. })
                && event.delivery.as_ref().is_some_and(|d| d == "batch-7")
        }
        "function_risk" => matches!(declared, Some(D::Function(f))
            if f.backing != "expr" && f.risk != lakelands_api::ontology::Risk::Low),
        "computed_by_function" => matches!(declared, Some(D::Type(t))
            if t.fields.iter().any(|f| f.function.is_some())),
        "computed_write" => {
            matches!(&event.body, B::RecordUpserted { ty, .. } if ty.as_str() == "alert")
        }
        // The call's digest on its record, as the write at 55 made it.
        "proposed_compute" => {
            let sha = |v: &Value| lakelands_core::canonical::sha256_hex(v);
            let call = sha(&serde_json::json!({ "function": "notify",
                "inputs": { "subject": "disk full" },
                "config": { "url": "https://notify.example/send" } }));
            let input =
                sha(&serde_json::json!({ "call": call, "record": ["alert", "a1"], "made": 55 }));
            matches!(proposed, Some(ProposedWrite::Compute { inputs: None, input: i, .. }) if i.0 == input)
                && event.cause == Some(lakelands_api::Position(55))
        }
        "withdrawn" => matches!(declared, Some(D::Withdraw(w))
            if w.kind == lakelands_api::write::DeclarationKind::Trigger && w.name == "shelved"),
        "trigger_change_guard" => matches!(declared, Some(D::Trigger(t))
            if t.when_strict && t.max_depth == Some(1)
                && t.on_changes == Some(vec![ChangeKind::Upserted, ChangeKind::Ingested])
                && t.when.as_ref().is_some_and(|w| matches!(w, lakelands_api::expr::Condition::Text(c) if c == "created()"))),
        "slip_write" => {
            matches!(&event.body, B::RecordUpserted { ty, made: true, .. } if ty.as_str() == "slip")
                && event.on_behalf_of.as_deref() == Some("human:reader")
        }
        "fired_change" => {
            matches!(&event.body, B::TriggerFired { trigger, change: Some(ChangeKind::Upserted), created: Some(true), .. }
                if trigger.as_str() == "slip_made")
                && event.cause == Some(lakelands_api::Position(61))
        }
        "reaction_write" => {
            matches!(&event.body, B::RecordUpserted { made: true, .. })
                && event.context.depth == 1
                && event.cause == Some(lakelands_api::Position(62))
        }
        "trigger_dropped" => {
            matches!(
                &event.body,
                B::TriggerDropped {
                    trigger,
                    depth: 1,
                    max_depth: 1,
                    bound: DepthBound::Trigger,
                    ..
                } if trigger.as_str() == "slip_made"
            ) && event.cause == Some(lakelands_api::Position(63))
        }
        "untyped_input" => matches!(declared, Some(D::Function(f))
            if !f.inputs.is_empty() && f.inputs.iter().all(|i| i.any_kind)),
        "values_removed" => {
            matches!(&event.body, B::ValuesRemoved { source, selection, before, keys, apart }
            if source == "erp" && before.is_none() && keys.is_empty() && apart.is_empty()
                && *selection == lakelands_api::write::Selection::Delivery("batch-7".into()))
        }
        "removal_retires" => matches!(&event.body, B::RecordRetired { key, .. } if key == "INV-2"),
        "delivery_own" => {
            matches!(&event.body, B::RecordIngested { key, .. } if key == "INV-3")
                && event.delivery.as_ref().map(|d| d.as_str()) == Some("d68")
        }
        "removed_before" => {
            matches!(&event.body, B::ValuesRemoved { source, selection, before, keys, apart }
            if source == "erp" && *before == Some(lakelands_api::Position(69)) && keys.is_empty() && apart.is_empty()
                && matches!(selection, lakelands_api::write::Selection::WrittenBefore(_)))
        }
        "removal_by_time_retires" => {
            matches!(&event.body, B::RecordRetired { key, .. } if key == "INV-3")
        }
        // A report of format 17 counts what it computes again, under its
        // own id at the head it was made at (062).
        "proposed_migrate_recomputed" => {
            matches!(proposed, Some(ProposedWrite::Migrate { report, .. })
            if report.recomputed == Some(1)
                && report.report_id == lakelands_core::canonical::sha256_hex(&serde_json::json!({
                    "report": lakelands_api::write::MigrationReport { report_id: String::new(), ..(**report).clone() },
                    "head": 70,
                })))
        }
        // A delivery sealed apart, its output, and the act that shreds it
        // alone (065, lakelands#1214).
        "sealed_apart" => {
            matches!(&event.body, B::RecordIngested { key, .. } if key == "c5")
                && event
                    .sealed
                    .as_ref()
                    .is_some_and(|s| s.key == "1" && s.apart == [KeyId::from("a1")])
                && event.delivery.as_ref().map(|d| d.as_str()) == Some("late")
        }
        "sealed_apart_output" => {
            matches!(
                &event.body,
                B::OutputRecorded {
                    output: Some(_),
                    ..
                }
            ) && event.sealed.as_ref().is_some_and(|s| {
                s.source.as_ref().is_some_and(|s| s == "hr") && s.apart == [KeyId::from("a1")]
            })
        }
        "removed_apart" => {
            matches!(&event.body, B::ValuesRemoved { source, keys, apart, .. }
            if source == "hr" && keys.is_empty() && *apart == [KeyId::from("a1")])
        }
        "removal_apart_retires" => {
            matches!(&event.body, B::RecordRetired { key, .. } if key == "c5")
        }
        // A selection by the source's own time names no position
        // (065, lakelands#1166).
        "removed_seen_before" => {
            matches!(&event.body, B::ValuesRemoved { source, selection, before, keys, apart }
            if source == "erp" && before.is_none() && keys.is_empty() && apart.is_empty()
                && matches!(selection, lakelands_api::write::Selection::SeenBefore(_)))
        }
        // A set of deliveries in one act, its ids in order (065, lakelands#1290).
        "removed_deliveries" => {
            matches!(&event.body, B::ValuesRemoved { source, selection, before, keys, apart }
            if source == "erp" && before.is_none() && keys.is_empty() && apart.is_empty()
                && matches!(selection, lakelands_api::write::Selection::Deliveries(ds)
                    if ds.iter().map(|d| d.as_str()).eq(["batch-7", "d68"])))
        }
        // A quarantined record's id is made over its id, the version
        // before, its cells, its box and its refusal (log-format §3).
        "quarantined"
        | "quarantined_again"
        | "quarantined_sealed"
        | "quarantined_constraint"
        | "quarantined_removed" => {
            let B::RecordQuarantined {
                id,
                version,
                cells,
                sealed,
                refusal,
                ..
            } = &event.body
            else {
                return false;
            };
            let boxed = sealed.as_ref().and_then(|s| s.boxed.as_deref());
            let made = log::quarantined_id(id, version - 1, cells, boxed, refusal) == event.id;
            let delivery = event.delivery.as_ref().map(|d| d.as_str());
            made && match what {
                "quarantined" => delivery == Some("q-1") && *version == 1,
                "quarantined_again" => {
                    delivery == Some("q-2") && id.delivery == "q-1" && *version == 2
                }
                "quarantined_sealed" => {
                    boxed.is_some() && event.sealed.is_none() && !cells.contains_key("email")
                }
                "quarantined_constraint" => {
                    serde_json::to_value(refusal).unwrap()["reason"]["refusal"] == "constraint"
                }
                _ => delivery == Some("q-7"),
            }
        }
        "resubmission_lands" | "lands" => matches!(&event.body, B::RecordIngested { key, .. }
            if key == if what == "lands" { "INV-10" } else { "INV-9" }),
        "removes_quarantined" => matches!(&event.body, B::ValuesRemoved { selection, .. }
            if matches!(selection, lakelands_api::write::Selection::Delivery(d) if d == "q-7")),
        "closed_resubmitted" | "closed_dismissed" | "closed_landed" | "closed_removed" => {
            use lakelands_api::quarantine::ClosedWhy as W;
            let want = match what {
                "closed_resubmitted" => W::Resubmitted,
                "closed_dismissed" => W::Dismissed,
                "closed_landed" => W::Landed,
                _ => W::Removed,
            };
            matches!(&event.body, B::QuarantineClosed { why, .. } if *why == want)
                && event.delivery.is_none()
        }
        other => panic!("`further.json` shows `{other}`, which this test has no check for"),
    }
}

/// `further.json`'s events, the lake's lines after the segments. Each
/// line is canonical, reads into the contract's event and back byte for
/// byte, hashes to the hash the file repeats, follows the line before it
/// from position 8 on, and is what the file says it shows. The lines
/// show what [`SHOWN`] lists, each thing once and none left out, and
/// end at [`LAST`].
#[test]
fn every_further_line_recomputes_and_shows_what_it_names() {
    let expected = read_json("expected.json");
    let last = expected["events"].as_array().unwrap().last().unwrap();
    let mut prev = str_of(last, "hash").to_owned();
    let mut at = last["position"].as_u64().unwrap();
    let mut shown = Vec::new();
    for entry in further("events") {
        at += 1;
        let value: Value = serde_json::from_str(&entry.line).unwrap();
        assert_eq!(
            canonical::to_vec(&value),
            entry.line.as_bytes(),
            "position {at}: the line is canonical"
        );
        let event: Event = serde_json::from_value(value)
            .unwrap_or_else(|e| panic!("position {at} reads as an Event: {e}"));
        assert_eq!(
            canonical::of(&event),
            entry.line.as_bytes(),
            "position {at}: the contract writes the event as the line"
        );
        assert_eq!(event.position.0, at, "the positions run on");
        assert_eq!(hash_of(&entry.line), event.hash.0, "position {at}: hash");
        assert_eq!(
            Some(&event.hash.0),
            entry.hash.as_ref(),
            "position {at}: the hash the file repeats"
        );
        assert_eq!(event.prev.0, prev, "position {at}: prev");
        assert_eq!(event.v, LOG_FORMAT_VERSION, "position {at}: its format");
        assert!(
            shows(&entry.shows, &event, &entry.line),
            "position {at} does not show `{}`: {}",
            entry.shows,
            entry.line
        );
        prev = event.hash.0;
        shown.push(entry.shows);
    }
    assert_eq!(
        shown, SHOWN,
        "the lines show every thing this test checks, each once"
    );
    assert_eq!(at, LAST, "the lake's last position");
}

/// `further.json`'s misplaced lines. Each is in the release's format,
/// its hash its own, after the event before its position: only where a
/// member sits is wrong, and `log::misplaced` refuses it for that member.
#[test]
fn every_misplaced_line_is_refused_for_the_member_it_misplaces() {
    let chain = chain();
    let misplaced = further("misplaced");
    let shown: Vec<&str> = misplaced.iter().map(|e| e.shows.as_str()).collect();
    assert_eq!(
        shown, MISPLACED,
        "one line for each misplaced member this test checks"
    );
    for entry in &misplaced {
        let event: Event = serde_json::from_str(&entry.line).unwrap();
        let at = event.position.0;
        assert_eq!(hash_of(&entry.line), event.hash.0, "at {at}: hash");
        assert_eq!(event.v, LOG_FORMAT_VERSION, "at {at}: its format is right");
        assert_eq!(
            event.prev,
            chain[at as usize - 1].hash,
            "at {at}: it follows the event before it"
        );
        let flaw = log::misplaced(&event).unwrap_or_else(|| panic!("at {at}: refused"));
        let why = flaw.to_string();
        match entry.shows.as_str() {
            "upstream_on_another_kind" => assert!(why.contains("`record_retired`"), "{why}"),
            "cause_in_context" => assert!(why.contains("`context` carries `cause`"), "{why}"),
            "on_behalf_of_in_context" => assert_eq!(flaw, Flaw::OnBehalfOfInContext),
            "output_twice" => assert_eq!(flaw, Flaw::OutputOnce),
            "sealed_on_another_kind" => assert!(why.contains("carries `sealed`"), "{why}"),
            "delivery_on_another_kind" => assert!(why.contains("carries `delivery`"), "{why}"),
            "sealed_for_another_source" => assert_eq!(
                flaw,
                Flaw::SealsAnother {
                    sealed: "web".into()
                }
            ),
            other => panic!("`further.json` misplaces `{other}`, which this test has no check for"),
        }
    }
}

/// The stored lines a segment holds from `stored`, read in order after
/// the event `after` (none: at a segment's start) as a reader reads them:
/// each line as `log::line_of` reads one, then `log::Lines`, and the
/// segment's end. Answers the events restored, or the line refused (none:
/// the end) and its flaw.
fn read_stored(stored: &[Value], after: &Value) -> Result<Vec<Value>, (Option<usize>, Flaw)> {
    let mut lines = log::Lines::default();
    let mut before = after.as_object().map(|a| {
        let at = Position(a["position"].as_u64().unwrap());
        (at, Digest(str_of(after, "hash").to_owned()))
    });
    let mut events = Vec::new();
    for (i, line) in stored.iter().enumerate() {
        let read = log::line_of(line.as_str().unwrap().as_bytes())
            .and_then(|l| lines.line(l, before.as_ref().map(|(p, h)| (*p, h))));
        let Some((event, _)) = read.map_err(|flaw| (Some(i), flaw))? else {
            continue;
        };
        let at = Position(event["position"].as_u64().unwrap());
        before = Some((at, Digest(str_of(&event, "hash").to_owned())));
        events.push(event);
    }
    lines.end().map_err(|flaw| (None, flaw))?;
    Ok(events)
}

/// The append `append` holds after the event `last`: each event canonical,
/// in the chain, its hash and its ingest id (under `v2` with its box, if
/// it has one) recomputed; the writer's lines for them, framed, are its
/// `stored`; and those restore, at the seam and read in order, to the
/// events byte for byte. Answers the framed events.
fn the_writers_lines(append: &Value, last: &Event) -> Vec<CanonicalEvent> {
    assert_eq!(append["after"]["position"], last.position.0, "it follows");
    assert_eq!(str_of(&append["after"], "hash"), last.hash.0, "it follows");
    let (mut prev, mut appended) = (last.hash.clone(), Vec::new());
    let lines: Vec<&str> = (append["events"].as_array().unwrap().iter())
        .map(|e| str_of(e, "line"))
        .collect();
    for (entry, line) in append["events"].as_array().unwrap().iter().zip(&lines) {
        let event: Event = serde_json::from_str(line).unwrap();
        let at = event.position.0;
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(canonical::to_vec(&value), line.as_bytes(), "at {at}");
        assert_eq!(hash_of(line), event.hash.0, "at {at}: hash");
        assert_eq!(str_of(entry, "hash"), event.hash.0, "at {at}: repeated");
        assert_eq!(
            (&event.prev, event.v),
            (&prev, LOG_FORMAT_VERSION),
            "at {at}"
        );
        let EventBody::RecordIngested {
            ty,
            key,
            version,
            patch,
            ..
        } = &event.body
        else {
            panic!("at {at}: an ingest");
        };
        let source = event.source.as_ref().unwrap();
        let before = version.0 - 1;
        let id = match event.sealed.as_ref().and_then(|s| s.boxed.as_deref()) {
            Some(b) => log::boxed_ingest_id(source, ty, key, before, patch, b),
            None => log::ingest_id(source, ty, key, before, patch),
        };
        assert_eq!(id, event.id, "at {at}: its ingest id");
        prev = event.hash.clone();
        appended.push(CanonicalEvent {
            position: event.position,
            id: event.id,
            prev: event.prev,
            hash: event.hash,
            fence: event.fence,
            bytes: line.as_bytes().to_vec(),
            frame: None,
        });
    }
    log::frame_append(&mut appended);
    let frame = appended[0].frame.clone().expect("an append is framed");
    assert_eq!(
        (frame.first, frame.events),
        (appended[0].position, lines.len() as u32)
    );
    let written = std::iter::once(&frame.bytes).chain(appended.iter().map(|e| &e.bytes));
    let written: Vec<&str> = written.map(|b| std::str::from_utf8(b).unwrap()).collect();
    let stored = append["stored"].as_array().unwrap();
    let said: Vec<&str> = stored.iter().map(|l| l.as_str().unwrap()).collect();
    assert_eq!(written, said, "the writer's lines");
    for (event, line) in appended.iter().zip(&lines) {
        let (restored, _) = log::restore(event).unwrap();
        assert_eq!(
            canonical::to_vec(&restored),
            line.as_bytes(),
            "restored at the seam"
        );
    }
    let read = read_stored(stored, &append["after"]).unwrap();
    let read: Vec<Vec<u8>> = read.iter().map(canonical::to_vec).collect();
    let want: Vec<Vec<u8>> = lines.iter().map(|l| l.as_bytes().to_vec()).collect();
    assert_eq!(read, want, "read in order");
    appended
}

/// `framed.json`, one append in the stored form (§1.2) after the last
/// event of `further.json`. Its events recompute; the store's writer frames
/// them as its `stored` lines, which restore to them byte for byte, at the
/// seam and read in order; and each of its breaks is refused at its line
/// for the flaw it names.
#[test]
fn a_framed_append_is_stored_as_the_writer_frames_it_and_every_break_is_refused() {
    let framed = read_json("framed.json");
    let last = chain().pop().unwrap();
    assert_eq!(framed["after"]["position"], last.position.0, "it follows");
    assert_eq!(str_of(&framed["after"], "hash"), last.hash.0, "it follows");
    let appended = the_writers_lines(&framed, &last);
    assert_eq!(appended.len(), 3, "an append of three is framed");

    let breaks = framed["breaks"].as_array().unwrap();
    let shown: Vec<&str> = breaks.iter().map(|b| str_of(b, "shows")).collect();
    assert_eq!(shown, BREAKS, "one break for each flaw this test checks");
    for entry in breaks {
        let shows = str_of(entry, "shows");
        let stored = entry["stored"].as_array().unwrap();
        let (at, flaw) = read_stored(stored, &entry["after"])
            .err()
            .unwrap_or_else(|| panic!("the break `{shows}` is refused"));
        let refused = &entry["refused"];
        assert_eq!(
            at.map(|i| i as u64),
            refused["at"].as_u64(),
            "`{shows}`: the line refused"
        );
        assert_eq!(
            serde_json::to_value(&flaw).unwrap(),
            refused["flaw"],
            "`{shows}`: {flaw}"
        );
    }
}

/// `boxed.json`, an ingest whose sealed members are in one box (§1.4),
/// after the framed append: its hash and place in the chain, its id made
/// under `lakelands:ingest:v2` with the box, a patch that holds none of
/// the members `sealed` names, and no flaw a reader refuses it for.
#[test]
fn an_ingest_whose_members_are_in_one_box_follows_the_framed_append() {
    let boxed = read_json("boxed.json");
    let framed = read_json("framed.json");
    let events = framed["events"].as_array().unwrap();
    let last: Event = serde_json::from_str(str_of(events.last().unwrap(), "line")).unwrap();
    let line = str_of(&boxed, "line");
    let event: Event = serde_json::from_str(line).unwrap();
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(canonical::to_vec(&value), line.as_bytes(), "canonical");
    assert_eq!(boxed["after"]["position"], last.position.0, "it follows");
    assert_eq!(str_of(&boxed["after"], "hash"), last.hash.0, "it follows");
    assert_eq!(
        (event.position, &event.prev),
        (last.position.next(), &last.hash)
    );
    assert_eq!(hash_of(line), event.hash.0, "its hash");
    assert_eq!(str_of(&boxed, "hash"), event.hash.0, "its hash repeated");
    assert_eq!(event.v, LOG_FORMAT_VERSION, "its format");
    assert_eq!(log::misplaced(&event), None);
    let EventBody::RecordIngested {
        ty,
        key,
        version,
        patch,
        ..
    } = &event.body
    else {
        panic!("an ingest");
    };
    let sealed = event.sealed.as_ref().unwrap();
    let boxed = sealed.boxed.as_deref().unwrap();
    assert!(!sealed.fields.is_empty() && sealed.fields.iter().all(|f| !patch.contains_key(f)));
    let source = event.source.as_ref().unwrap();
    let id = log::boxed_ingest_id(source, ty, key, version.0 - 1, patch, boxed);
    assert_eq!(id, event.id, "its ingest id");
    assert_ne!(
        log::ingest_id(source, ty, key, version.0 - 1, patch),
        event.id
    );
    let mut before = event.clone();
    before.v = 19;
    assert_eq!(
        log::misplaced(&before),
        Some(Flaw::BoxedOn {
            kind: "record_ingested".into(),
            v: 19
        }),
        "a box before format 20"
    );
}

/// `apart.json`, one append of two ingests of a source, each in a delivery
/// of its own and sealed apart under its own key (065, 083), after the
/// boxed ingest: its events and stored lines hold as `framed.json`'s do,
/// and the frame leaves each line its `delivery` and its `sealed.apart`
/// and box.
#[test]
fn an_append_of_two_deliveries_sealed_apart_is_stored_as_the_writer_frames_it() {
    let apart = read_json("apart.json");
    let last: Event = serde_json::from_str(str_of(&read_json("boxed.json"), "line")).unwrap();
    assert_eq!(the_writers_lines(&apart, &last).len(), 2);
    let held: Vec<Value> = (apart["stored"].as_array().unwrap()[1..].iter())
        .map(|l| serde_json::from_str(l.as_str().unwrap()).unwrap())
        .collect();
    let (apart_of, delivery) = (
        |v: &Value| v["sealed"]["apart"].clone(),
        |v: &Value| v["delivery"].clone(),
    );
    assert!(
        held.iter()
            .all(|v| !apart_of(v).is_null() && !delivery(v).is_null())
    );
    assert!(apart_of(&held[0]) != apart_of(&held[1]) && delivery(&held[0]) != delivery(&held[1]));
}

/// The breaks of `framed.json`, by the code of the flaw each is refused for.
const BREAKS: [&str; 8] = [
    "frame_in_frame",
    "framed_twice",
    "frame_short",
    "not_a_frame",
    "frame_other",
    "position",
    "prev_hash",
    "start_unheld",
];

/// `delivery` on a `record_ingested` of a format before 17, which has no
/// such member: the vectors' delivery line, its format set back, is
/// refused naming its format.
#[test]
fn a_delivery_before_format_17_is_misplaced() {
    let line = further("events")
        .into_iter()
        .find(|e| e.shows == "delivery");
    let mut event: Event = serde_json::from_str(&line.expect("a delivery line").line).unwrap();
    event.v = 16;
    let flaw = log::misplaced(&event).expect("refused");
    assert!(matches!(flaw, Flaw::DeliveryOn { v: 16, .. }), "{flaw}");
}

/// The negative vector `negative/known-at-goes-back`: the first segment
/// of the vectors with the last event's `known_at` set before the one
/// before it, its hash made right. `export::verify` refuses it at that
/// position (log-format §1.3), as `verify.py` and `Lake::verify` do.
#[test]
fn the_negative_vector_whose_known_at_goes_back_is_refused_at_its_position() {
    let dir = vectors().join("negative/known-at-goes-back");
    // Only the rule is broken: position 6's hash is its line's own, and it
    // names position 5's as the hash before it.
    let text = fs::read_to_string(dir.join("log").join(format!("{:020}.jsonl", 0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let six: Value = serde_json::from_str(lines[6]).unwrap();
    let five: Value = serde_json::from_str(lines[5]).unwrap();
    let mut body = six.clone();
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), str_of(&six, "hash"));
    assert_eq!(str_of(&six, "prev"), str_of(&five, "hash"));
    assert!(str_of(&six, "known_at") < str_of(&five, "known_at"));
    match lakelands_core::export::verify(&dir) {
        Err(lakelands_api::Error::Unavailable { situation }) => assert!(
            situation.to_string().contains("at position 6:")
                && situation.to_string().contains("known_at"),
            "{situation}"
        ),
        other => {
            panic!("a known_at going back is refused at position 6, and it answered {other:?}")
        }
    }
}

/// The negative vector `negative/another-format`: the first segment of
/// the vectors with the last event written in the format before this
/// release's, its hash made right. `export::verify` refuses it at that
/// position by both numbers: a lake's format never falls
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`),
/// and `verify.py`, which reads the one format, refuses it there too.
#[test]
fn the_negative_vector_in_another_format_is_refused_at_its_position() {
    let dir = vectors().join("negative/another-format");
    // Only the format is wrong: position 6's hash is its line's own, it
    // names position 5's as the hash before it, and every line before it
    // is in the release's format.
    let text = fs::read_to_string(dir.join("log").join(format!("{:020}.jsonl", 0))).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let (six, five) = (&lines[6], &lines[5]);
    let mut body = six.clone();
    body.as_object_mut().unwrap().remove("hash");
    assert_eq!(sha(&canonical::to_vec(&body)), str_of(six, "hash"));
    assert_eq!(str_of(six, "prev"), str_of(five, "hash"));
    let formats: Vec<u64> = lines.iter().map(|l| l["v"].as_u64().unwrap()).collect();
    let reads = u64::from(LOG_FORMAT_VERSION);
    assert_eq!(formats[..6], [reads; 6]);
    assert_eq!(formats[6], reads - 1);
    match lakelands_core::export::verify(&dir) {
        Err(lakelands_api::Error::Unavailable { situation }) => assert!(
            matches!(
                &*situation,
                Situation::Damaged { position: Some(at), flaw: Flaw::FormatStep { v, before }, .. }
                    if at.0 == 6 && *v == reads - 1 && *before == reads
            ),
            "{situation}"
        ),
        other => {
            panic!("an event in another format is refused at position 6, and it answered {other:?}")
        }
    }
}

/// The vectors are kept for every format from 14, the first a release
/// with real installations wrote, to the one this release writes, with no
/// gap; and each directory's own `verify.py`, which reads that format
/// alone, passes over its own log. `scripts/gates/vectors-kept.sh` holds
/// that a shipped one is as its tag holds it.
#[test]
fn the_vectors_of_every_format_a_release_reads_are_kept_and_verify() {
    let mut formats: Vec<u32> = fs::read_dir(every_format())
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.parse().ok())
        .collect();
    formats.sort_unstable();
    let want: Vec<u32> = (14..=LOG_FORMAT_VERSION).collect();
    assert_eq!(formats, want, "the vector directories by format");
    for n in formats {
        let dir = every_format().join(n.to_string());
        let run = std::process::Command::new("python3")
            .arg("verify.py")
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|e| {
                panic!("this test runs `python3 verify.py` and needs python3: {e}")
            });
        assert!(
            run.status.success(),
            "format {n}'s verify.py over its own vectors: {}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

/// Every line of a kept format's vectors, its log and `further.json`'s,
/// as JSON.
fn lines_of(n: u32) -> Vec<Value> {
    let dir = every_format().join(n.to_string());
    let mut lines = Vec::new();
    for seg in ["00000000000000000000.jsonl", "00000000000000000007.jsonl"] {
        let text = fs::read_to_string(dir.join("log").join(seg)).unwrap();
        lines.extend(
            text.lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap()),
        );
    }
    let further: Value =
        serde_json::from_slice(&fs::read(dir.join("further.json")).unwrap()).unwrap();
    for e in further["events"].as_array().unwrap() {
        lines.push(serde_json::from_str(str_of(e, "line")).unwrap());
    }
    lines
}

/// The steps from each older format to this release's change no event:
/// 14 → 15 lets a verb's path read across a reference or a relation's
/// link (055), which a log of format 14 never declares, and 15 → 16,
/// 16 → 17, 17 → 18, 18 → 19, 19 → 20 and 20 → 21 add forms a log of the
/// format before never holds.
/// So every line of each older format's vectors, of every kind they hold,
/// is read as it was written, `v` included, and decodes into this
/// release's structs; the kinds each holds are named, so a kind the
/// vectors miss is seen.
#[test]
fn the_steps_from_every_older_format_change_no_event_of_any_kind() {
    for n in OLDEST_LOG_FORMAT..LOG_FORMAT_VERSION {
        the_step_from(n);
    }
}

fn the_step_from(n: u32) {
    let mut kinds = std::collections::BTreeSet::new();
    for line in lines_of(n) {
        let v = line["v"].as_u64().unwrap();
        assert_eq!(v, u64::from(n));
        let read = log::converted(v, line.clone());
        assert_eq!(read, line, "the step from {n} changed {line}");
        let event: Event = serde_json::from_value(read).unwrap();
        assert_eq!(canonical::of(&event), canonical::to_vec(&line));
        kinds.insert(event.body.kind());
    }
    let held: Vec<_> = kinds.iter().map(|k| k.to_string()).collect();
    assert_eq!(held.len(), kinds.len());
    let some = [
        "lake_created",
        "lease_taken",
        "declared",
        "migrated",
        "record_ingested",
        "record_upserted",
        "checkpoint",
        "write_proposed",
    ];
    for kind in some {
        assert!(
            held.iter().any(|k| k == kind),
            "format {n}'s vectors hold no `{kind}`: {held:?}"
        );
    }
}

/// Each older format's vectors verify as an export does, through the
/// converter: the chain, every hash over the bytes as written, and every
/// rule of the format, by the same verifier an import runs.
#[test]
fn the_vectors_of_every_older_format_verify_as_an_export_of_that_format() {
    for n in OLDEST_LOG_FORMAT..LOG_FORMAT_VERSION {
        let dir = every_format().join(n.to_string());
        let verified = lakelands_core::export::verify(&dir).unwrap();
        assert_eq!(
            verified.through.0, 7,
            "format {n}: the two segments run through position 7"
        );
    }
}

/// Fences never fall: the vectors' first segment with the event at
/// position 6 carried under fence 0, below position 1's lease of fence 1,
/// its hash made right, is refused at 6 by `export::verify`.
#[test]
fn a_fence_that_falls_is_refused_where_it_falls() {
    let dir = std::env::temp_dir().join(format!("lakelands-fence-falls-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("log")).unwrap();
    let text = fs::read_to_string(vectors().join("log").join(format!("{:020}.jsonl", 0))).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let six = &mut lines[6];
    six["fence"] = serde_json::json!(0);
    six.as_object_mut().unwrap().remove("hash");
    let hash = sha(&canonical::to_vec(six));
    six["hash"] = serde_json::json!(hash);
    let mut file = Vec::new();
    for l in &lines {
        file.extend(canonical::to_vec(l));
        file.push(b'\n');
    }
    fs::write(dir.join("log").join(format!("{:020}.jsonl", 0)), file).unwrap();
    let verified = lakelands_core::export::verify(&dir);
    let _ = fs::remove_dir_all(&dir);
    assert!(
        matches!(&verified, Err(lakelands_api::Error::Unavailable { situation })
            if matches!(&**situation, Situation::Damaged {
                position: Some(at), flaw: Flaw::FenceBelow { fence: 0, floor: 1 }, ..
            } if at.0 == 6)),
        "{verified:?}"
    );
}
