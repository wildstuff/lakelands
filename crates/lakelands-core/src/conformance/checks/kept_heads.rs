//! What a lake keeps of a state, and what it computes again instead
//! (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`).
//!
//! A kept view keeps each group's latest record alone, so a read of it
//! as of an earlier position computes the view there, as a view no
//! setting keeps is computed, and `describe_type` says so. A kept view
//! whose expressions cut a zone's local calendar records the copy of the
//! time-zone database that cut it, and a writer that opens the lake under
//! another folds it again. A state keeps the provenance its fields share once, a field the fold
//! derived in the event that wrote the others naming it rather than
//! carrying a copy; it reads back each field's as it was written, where
//! a derived field's agrees with the version and where an older one's
//! does not.

use std::sync::atomic::AtomicBool;

use lakelands_api::Location;
use lakelands_api::describe::{Kept, NotKept};
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::event::Digest;
use lakelands_api::read::GroupKey;
use lakelands_api::record::FieldMeta;
use lakelands_api::seam::{VIEW_KEPT_SETTING, kept_name};

use crate::conformance::fixture::{derived, read_at, source_record};
use crate::conformance::prelude::*;

check!(
    KEPT_HEAD_ONLY_EARLIER_READ = a_kept_view_as_of_an_earlier_position_answers_as_computed_there,
    "098 (lakelands#1677): a kept view and its twin, which no setting keeps, are read as of every position their source was written at, while members arrive, change, move to another group and leave: each answers the twin's records, in one find and in pages of one continued by their cursors, and a count, and `describe_type` there says the view is computed, `not_kept_here`, where at the head it says `kept`; again after a rebuild"
);

check!(
    PROVENANCE_ONCE = a_fields_provenance_reads_as_written_where_it_agrees_with_the_version_and_not,
    "098 (lakelands#1677): of a type with two written fields, two derived by an expression and one by a function, a record written whole reads each field an expression derived with the provenance of the field it reads, its origin `derived`, and the function's field its own, naming the function; written again in one field, the derived field that reads it takes the new event's, and the fields that do not keep their own, older than the version; a record fed from a source reads its source and the instant it was seen on each field an expression derived; a rebuild and a reopen read every field's alike",
    reopens
);

async fn a_kept_view_as_of_an_earlier_position_answers_as_computed_there(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "line",
        vec![
            field("g", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    ))
    .await?;
    for name in ["sums", "twin"] {
        let fields = vec![
            field("g", StorageKind::Text),
            field("c", StorageKind::Number),
            field("t", StorageKind::Number),
        ];
        let measures = [("c", "count()"), ("t", "sum(n)")];
        l.declare(view_of(name, "line", &["g"], fields, &measures))
            .await?;
    }
    l.set(&format!("{VIEW_KEPT_SETTING}sums"), json!(true))
        .await?;
    let mut at = Vec::new();
    for (key, fields) in [
        ("a", json!({"g": "x", "n": 1})),
        ("b", json!({"g": "x", "n": 2})),
        ("c", json!({"g": "y", "n": 4})),
        ("a", json!({"n": 5})),
        ("b", json!({"g": "y"})),
        ("c", json!({"g": "z"})),
    ] {
        l.upsert("line", key, fields).await?;
        at.push(store.head().await.must("reading the head")?);
    }
    l.retire("line", "a").await?;
    at.push(store.head().await.must("reading the head")?);
    let mut red = Vec::new();
    alike(&l, &at, "", &mut red).await?;
    store.rebuild().await.must("rebuilding the lake")?;
    alike(&l, &at, " after a rebuild", &mut red).await?;
    ensure!(
        red.is_empty(),
        "a kept view as of an earlier position answers as computed: {}",
        red.join("; ")
    );
    Ok(())
}

/// Pushes to `red` each position of `at` where `sums` answers otherwise
/// than `twin` or `describe_type` says it is not computed there, and the
/// head, where it must say `kept`; `when` names the moment.
async fn alike(l: &Lake, at: &[Position], when: &str, red: &mut Vec<String>) -> Held {
    let fields =
        |rs: Vec<Record>| -> Vec<_> { rs.into_iter().map(|r| (r.key, r.fields)).collect() };
    let earlier = &at[..at.len() - 1];
    for p in earlier.iter().map(|p| Some(*p)).chain([None]) {
        let mut answers = Vec::new();
        for ty in ["sums", "twin"] {
            let whole = l.all_found(ty, find_at(p)).await?;
            let paged = Find {
                limit: Some(1),
                at: read_at(p),
                ..Find::default()
            };
            let paged = l.all_found(ty, paged).await?;
            let counted = Find {
                total: true,
                at: read_at(p),
                ..Find::default()
            };
            let counted =
                (l.cat.find(&l.own, &TypeName::from(ty), counted).await).must("counting a view")?;
            let total = match counted.data {
                Found::Records { total, .. } => total,
                Found::Groups { .. } => None,
            };
            answers.push((fields(whole), fields(paged), total));
        }
        if answers[0] != answers[1] {
            red.push(format!(
                "as of {p:?}{when} `sums` answers {:?}, its twin {:?}",
                answers[0], answers[1]
            ));
        }
        let sums = TypeName::from("sums");
        let d = l.cat.describe_type(&l.own, &sums, read_at(p));
        let kept = d.await.must("describing `sums`")?.data.lifecycle.kept;
        let want = match p {
            Some(_) => Kept::Computed {
                why: NotKept::NotKeptHere,
            },
            None => Kept::Kept,
        };
        if kept.as_ref() != Some(&want) {
            red.push(format!(
                "as of {p:?}{when} `describe_type` says {kept:?} of `sums`, not {want:?}"
            ));
        }
    }
    Ok(())
}

check!(
    ZONED_REFOLDED =
        a_kept_zoned_view_cut_under_another_tzdb_copy_is_folded_again_at_a_writers_open,
    "098 (lakelands#1677, lakelands#1667): a kept view grouped by a day of Europe/Berlin, a kept view over it and a kept view grouped by a day in UTC answer as their twins; their kept records are then each given another sum below the store, and the zoned view's state is marked as cut under another copy of the time-zone database; a writer's open folds the zoned view and the view over it again, so they answer as their twins and `describe_type` says `kept`, while the view in UTC, no copy of the database deciding it, is not folded again and answers the sums it was given",
    given
);

/// The views [`ZONED_REFOLDED`] keeps, each with its twin, `<name>_twin`.
const ZONED: [&str; 3] = ["local", "over", "utc"];

async fn a_kept_zoned_view_cut_under_another_tzdb_copy_is_folded_again_at_a_writers_open(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let (dt, num) = (StorageKind::DateTime, StorageKind::Number);
    l.declare(type_decl("reading", vec![field("at", dt), field("v", num)]))
        .await?;
    let day = |name: &str, zone: &str| {
        let fields = vec![field("day", dt), field("s", num)];
        let mut view = view_of(name, "reading", &["day"], fields, &[("s", "sum(v)")]);
        let v = view.view.as_mut().expect("a view");
        let term = json!({"name": "day", "expr": format!("date_bucket(at, 'day'{zone})")});
        v.group_by = vec![serde_json::from_value::<GroupKey>(term).expect("a term")];
        view
    };
    for twin in ["", "_twin"] {
        l.declare(day(&format!("local{twin}"), ", 'Europe/Berlin'"))
            .await?;
        l.declare(day(&format!("utc{twin}"), "")).await?;
        let fields = vec![field("s", num)];
        let over = view_of(
            &format!("over{twin}"),
            &format!("local{twin}"),
            &[],
            fields,
            &[("s", "sum(s)")],
        );
        l.declare(over).await?;
    }
    for view in ZONED {
        l.set(&format!("{VIEW_KEPT_SETTING}{view}"), json!(true))
            .await?;
    }
    for (key, at, v) in [
        ("r1", "2030-03-30T22:30:00Z", 1),
        ("r2", "2030-03-30T23:30:00Z", 2),
        ("r3", "2030-03-31T12:00:00Z", 4),
    ] {
        l.upsert("reading", key, json!({"at": at, "v": v})).await?;
    }
    let mut red = Vec::new();
    for view in ZONED {
        twinned(&l, view, "as made", &mut red).await?;
    }
    let armed = Arc::new(AtomicBool::new(true));
    let wrap: crate::conformance::Wrap = {
        let armed = armed.clone();
        Arc::new(move |inner| {
            let armed = armed.clone();
            Arc::new(Stale { inner, armed }) as Arc<dyn EngineOpener>
        })
    };
    let store = (given.fault)(store, Seams::default(), wrap).await;
    let store = store.map_err(|e| Failure::new(format!("the writer's open was refused: {e}")))?;
    ensure!(
        !armed.load(Ordering::SeqCst),
        "the writer's open meets the projection the check marks"
    );
    let l = Lake::of(store.as_ref());
    for view in ["local", "over"] {
        twinned(&l, view, "folded again at the open", &mut red).await?;
    }
    let utc = sums(&l, "utc").await?;
    let twin = sums(&l, "utc_twin").await?;
    let given: Vec<_> = twin.iter().map(|(k, s)| (k.clone(), s + 1000.0)).collect();
    if utc != given {
        red.push(format!(
            "`utc`, which no copy of the time-zone database decides, keeps the sums it was given, {given:?}: {utc:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a kept zoned view cut under another copy is folded again: {}",
        red.join("; ")
    );
    Ok(())
}

/// Pushes to `red` where `view` answers otherwise than its twin, or
/// `describe_type` does not say it is kept; `when` names the moment.
async fn twinned(l: &Lake, view: &str, when: &str, red: &mut Vec<String>) -> Held {
    let (got, want) = (
        sums(l, view).await?,
        sums(l, &format!("{view}_twin")).await?,
    );
    if got != want {
        red.push(format!(
            "{when}, `{view}` answers {got:?}, its twin {want:?}"
        ));
    }
    let said = l.describe_type(view).await?.lifecycle.kept;
    if said != Some(Kept::Kept) {
        red.push(format!("{when}, `describe_type` says {said:?} of `{view}`"));
    }
    Ok(())
}

/// The sum `s` of each of `view`'s records by key, as the owner finds it.
async fn sums(l: &Lake, view: &str) -> Result<Vec<(RecordKey, f64)>, Failure> {
    let found = l.all_found(view, Find::default()).await?;
    let sum = |r: &Record| {
        r.fields
            .get("s")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN)
    };
    Ok(found.iter().map(|r| (r.key.clone(), sum(r))).collect())
}

/// The harness's engine opener, whose next writer's open meets a
/// projection [`ZONED_REFOLDED`] changed below the store: each kept record
/// of its views with a sum 1000 greater, and each kept view's state that
/// names a copy of the time-zone database naming `2000a` instead.
struct Stale {
    inner: Arc<dyn EngineOpener>,
    armed: Arc<AtomicBool>,
}

impl EngineOpener for Stale {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(
        &self,
        at: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(at)
    }
    fn open_scratch(&self, at: &Location) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.inner.open_scratch(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        let engine = self.inner.open(at, mode)?;
        if mode != OpenMode::Write || !self.armed.swap(false, Ordering::SeqCst) {
            return Ok(engine);
        }
        let through = engine.projection().folded_through()?;
        let through = through.expect("the check's lake holds its writes");
        let view = engine.view(None)?;
        let scan = Scan {
            limit: 1000,
            restricted: true,
            ..Scan::default()
        };
        let mut puts = Vec::new();
        let mut states = Vec::new();
        for name in ZONED {
            for mut r in view.scan(&kept_name(&TypeName::from(name)), &scan)? {
                let s = r
                    .fields
                    .get("s")
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                r.fields.insert("s".into(), json!(s + 1000.0));
                puts.push(r);
            }
            let input = Digest(lakelands_api::canonical::sha256_hex(&json!(name)));
            if let Some((_, mut state)) = view.output("sys:kept", &input)?
                && state.get("tzdb").is_some()
            {
                state["tzdb"] = json!("2000a");
                states.push((input, state));
            }
        }
        drop(view);
        let p = engine.projection();
        p.begin()?;
        for r in &puts {
            p.put_record(r)?;
        }
        for (input, state) in &states {
            p.put_output("sys:kept", input, state, through)?;
        }
        p.commit(through)?;
        Ok(engine)
    }
}

/// The type [`PROVENANCE_ONCE`] writes: `a` and `g` written, `twice` and
/// `loud` derived from one each, `tag` by the suite's echo function.
const TICK: &str = "tick";
/// [`TICK`] fed from a source.
const FED: &str = "fed";

async fn a_fields_provenance_reads_as_written_where_it_agrees_with_the_version_and_not(
    store: Store,
    reopen: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "function", "name": "echo", "backing": "conformance", "inputs": [{"name": "g", "type": {"kind": "text"}}], "config": {"token": "kept_heads", "echo": "g"}}))
        .await?;
    let tag = json!({"name": "tag", "type": {"kind": "text"}, "authority": "derived", "function": "echo", "depends_on": ["g"]});
    let tag: FieldDecl = serde_json::from_value(tag)
        .map_err(|e| Failure::new(format!("the check's field does not read: {e}")))?;
    // `fed` is `tick` with its written fields a source's.
    for (ty, by) in [
        (TICK, field as fn(&str, StorageKind) -> FieldDecl),
        (FED, sourced),
    ] {
        let fields = vec![
            by("a", StorageKind::Number),
            by("g", StorageKind::Text),
            derived("twice", StorageKind::Number, "a * 2", &["a"]),
            derived("loud", StorageKind::Text, "upper(g)", &["g"]),
            tag.clone(),
        ];
        l.declare(type_decl(ty, fields)).await?;
    }
    l.upsert(TICK, "t1", json!({"a": 1, "g": "p"})).await?;
    let first = l.read(TICK, "t1").await?;
    l.upsert(TICK, "t1", json!({"a": 3})).await?;
    let fed = SourceRecord {
        seen_at: Some("2026-01-01T00:00:00Z".parse().expect("an instant")),
        ..source_record(FED, "s1", json!({"a": 5, "g": "q"}))
    };
    l.ingest("feed", vec![fed]).await.must("feeding `s1`")?;
    let mut red = Vec::new();
    let mut held = |what: String, ok: bool| {
        if !ok {
            red.push(what);
        }
    };
    let whole = [("twice", "a"), ("loud", "g")];
    for (f, from) in whole {
        held(
            format!("`t1` written whole: `{f}` reads `{from}`'s provenance derived"),
            derives(&first, f, from),
        );
    }
    held(
        "`t1` written whole: `tag` reads its own provenance, naming `echo`".into(),
        by_echo(&first),
    );
    let again = l.read(TICK, "t1").await?;
    held(
        "`t1` written again in `a`: `twice` reads `a`'s new provenance derived".into(),
        derives(&again, "twice", "a"),
    );
    for f in ["g", "loud", "tag"] {
        held(
            format!("`t1` written again in `a`: `{f}` keeps its own, the first write's"),
            again.meta.get(f) == first.meta.get(f),
        );
    }
    let newer = |f: &str| again.meta.get(f).map(|m| m.position);
    held(
        "`t1` written again: `loud` is older than `twice`".into(),
        newer("loud") < newer("twice"),
    );
    let s1 = l.read(FED, "s1").await?;
    for (f, from) in whole {
        let seen = s1
            .meta
            .get(f)
            .is_some_and(|m| m.seen_at.is_some() && m.source.is_some());
        held(
            format!("`s1` fed: `{f}` reads `{from}`'s provenance derived, seen and sourced"),
            derives(&s1, f, from) && seen,
        );
    }
    held(
        "`s1` fed: `tag` reads its own provenance, naming `echo`".into(),
        by_echo(&s1),
    );
    let read = [again, s1];
    store.rebuild().await.must("rebuilding the lake")?;
    let rebuilt = both(&l).await?;
    let store = reopen(store, Default::default()).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let reopened = both(&Lake::of(store.as_ref())).await?;
    held(
        format!("a rebuild reads the records as written: {rebuilt:?}, not {read:?}"),
        rebuilt == read,
    );
    held(
        format!("a reopen reads the records as written: {reopened:?}, not {read:?}"),
        reopened == read,
    );
    ensure!(
        red.is_empty(),
        "a field's provenance reads as written: {}",
        red.join("; ")
    );
    Ok(())
}

/// `t1` of [`TICK`] and `s1` of [`FED`] as the owner reads them.
async fn both(l: &Lake) -> Result<[Record; 2], Failure> {
    Ok([l.read(TICK, "t1").await?, l.read(FED, "s1").await?])
}

/// Whether `r`'s `field` reads the provenance of `from` with its origin
/// `derived`, `from`'s own not derived.
fn derives(r: &Record, field: &str, from: &str) -> bool {
    let (Some(got), Some(of)) = (r.meta.get(field), r.meta.get(from)) else {
        return false;
    };
    let want = FieldMeta {
        origin: Origin::Derived,
        ..of.clone()
    };
    *got == want && of.origin != Origin::Derived
}

/// Whether `r`'s `tag` reads a provenance of its own: derived by `echo`.
fn by_echo(r: &Record) -> bool {
    (r.meta.get("tag"))
        .is_some_and(|m| m.origin == Origin::Derived && m.procedure.as_deref() == Some("echo"))
}
