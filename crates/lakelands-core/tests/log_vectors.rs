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
//! The vectors are one lake's log in the one format version this release
//! reads and writes, `LOG_FORMAT_VERSION`
//! (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`):
//! the two segments, then the events of `further.json`, each following the
//! one before it and each named for what it shows. `verify.py --stamp N`
//! writes them in another.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use lakelands_api::error::{Flaw, Situation};
use lakelands_api::event::{Event, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::ontology::{TypeDecl, VerbScope};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, ProposedWrite};
use lakelands_core::{canonical, log, shape};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors")
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
    let page = fs::read_to_string(vectors().join("../../log-format.md")).unwrap();
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
    assert_eq!(checks, 304, "the checks verify.py makes:\n{said}");
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
const SHOWN: [&str; 32] = [
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
];

/// The last position of the vectors' lake.
const LAST: u64 = 39;

/// The members `further.json`'s misplaced lines misplace, in the file's
/// order: every name the test of them has a check for, once.
const MISPLACED: [&str; 3] = [
    "upstream_on_another_kind",
    "cause_in_context",
    "output_twice",
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
        "proposed_declare" => matches!(proposed, Some(ProposedWrite::Declare { declarations })
            if !declarations.is_empty()),
        "proposed_migrate" => {
            matches!(proposed, Some(ProposedWrite::Migrate { report, .. }) if report.lossless)
        }
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
            "output_twice" => assert_eq!(flaw, Flaw::OutputOnce),
            other => panic!("`further.json` misplaces `{other}`, which this test has no check for"),
        }
    }
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
/// position by both numbers
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`),
/// as `verify.py` does.
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
                Situation::Damaged { position: Some(at), flaw: Flaw::FormatOther { v, reads: r }, .. }
                    if at.0 == 6 && *v == reads - 1 && *r == reads
            ),
            "{situation}"
        ),
        other => {
            panic!("an event in another format is refused at position 6, and it answered {other:?}")
        }
    }
}
