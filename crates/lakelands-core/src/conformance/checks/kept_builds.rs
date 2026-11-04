//! A kept view is built in parts beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`, R8).
//!
//! A `view_kept.<view>` setting no longer folds the view whole in the turn
//! that appends it: the fold marks the view's state building, one cursor
//! per source, and the door that wrote the setting sends the build and
//! answers at its `Keep`. Each turn folds one `order_chunk` page of the
//! first source its cursor has not reached, and a read computes the view
//! at the request until the `Keep`, as it did before the setting. These
//! checks hold what a caller sees: every read answers as the lake did
//! before the setting, during the build and after it; a write sent beside
//! the build goes on, is answered first, and reaches both sides of the
//! cursor; a lake whose writer went between two turns finishes the build
//! at its next open; a setting taken back between two parts ends clean,
//! and one set again builds whole; and a projection rebuilt from the log
//! holds the view whole at its setting's event.

use lakelands_api::describe::{Kept, NotKept};
use lakelands_api::engine::OpenMode;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::SETTING_TYPE;

use super::builds::{First, chunked, rows, until_appended, until_first};
use super::export::Scratch;
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

/// The setting the checks write.
const KEPT: &str = "view_kept.by_s";

check!(
    KEPT_BUILD_READ_ANSWERS_AS_BEFORE =
        a_kept_view_building_in_parts_answers_as_the_lake_did_before,
    "099, R8: a `view_kept.<view>` setting on a view over a type with records builds the view in parts of `limits.order_chunk` records, each a turn beside the writer; `describe_type` does not say the view `kept` while it builds and says `kept` once it is, and every read of the view taken during the build, and after its `Keep`, answers exactly what the lake answered before the setting",
    given
);

check!(
    KEPT_BUILD_WRITE_BESIDE = a_record_written_beside_a_kept_build_is_in_its_group_once_it_is_kept,
    "099, R8: a record written beside a kept view's build is in its group once the view is kept, whichever side of the fill's cursor its key lies on: one keyed below the cursor is counted by the write, one keyed above it by the fill, and neither twice; the view then answers what it computes with the setting taken back",
    given
);

check!(
    KEPT_BUILD_WRITES_GO_ON = a_write_beside_a_kept_build_is_answered_before_the_setting_is,
    "099, R8: a kept view's door answers when its build is whole, and a write sent beside the build waits for at most the turn in hand, so it is answered before the setting is",
    given
);

check!(
    KEPT_BUILD_RESTART_FINISHES = a_kept_build_stopped_between_parts_is_finished_by_the_next_open,
    "099, R8: a kept view's build whose writer goes between two of its parts is owed in the projection's mark, and the next open of the lake sends it again without waiting: the reopened lake computes the view at once as before, a write sent beside it is answered before a setting whose door waits for the build, and once that setting is whole the view is kept and answers what it computed",
    given
);

check!(
    KEPT_BUILD_SETTING_TAKEN_BACK =
        a_kept_setting_taken_back_between_two_parts_of_its_build_ends_clean,
    "099, R8: a `view_kept.<view>` setting taken back between two parts of its build ends clean: every read taken beside the build, and after the take-back landed, answers what the lake answered before the setting, the door that retired it answering only once the groups it had reached are gone, and `describe_type` says the view `not asked`",
    given
);

check!(
    KEPT_BUILD_SETTING_SET_AGAIN = a_kept_setting_set_again_after_a_take_back_is_whole,
    "099, R8: a `view_kept.<view>` setting set again after its take-back builds the view whole from no cursor: once the second door answers, the view is kept and every read answers what the lake answered before the setting",
    given
);

check!(
    KEPT_BUILD_REBUILD_SAME_LAKE = a_rebuild_holds_a_kept_view_whole_at_its_event,
    "099, R8: a `view_kept.<view>` setting is in the log and its build in the projection, so a projection rebuilt from the log holds the view whole at its event, with no mark: `describe_type` says it `kept` and every read answers what the built lake answered",
    given
);

check!(
    VIEW_BUILD_EXTREME_LOSES_ITS_HOLDER =
        a_kept_group_whose_smallest_record_moves_beside_the_build_answers_its_next_smallest,
    "099, R8: a kept view's extremes are read from the order its build walked, so a record written beside the build that takes the smallest of its group out of it leaves the group's `lo` the next smallest once the view is kept, and its `hi` the raised record; every read then answers what it computes with the setting taken back",
    given
);

check!(
    VIEW_BUILD_OVER_A_VIEW = a_kept_view_over_a_view_starts_after_its_inner_view_is_kept,
    "099, R8: a `view_kept.<view>` setting on a view over a view, set before the inner view's setting is, leaves the outer view not `kept` while the inner's build runs; the door that builds the inner finishes the outer after it, so once both settings are in force the outer is kept and every read answers what it computes with the settings taken back",
    given
);

check!(
    KEPT_BUILD_TYPE_DECLARED_AGAIN = a_type_declared_again_beside_a_kept_build_is_whole_once_it_is,
    "099, R8: a type declared again beside a kept view's build, with a field added, ends that build and makes the view whole again at the declaration's event: every read taken beside the declaration answers what the lake answered before the setting, and once the declaration answers the view is `kept` and answers what it computes with the setting taken back",
    given
);

check!(
    KEPT_BUILD_EXPORT_IMPORT_SAME_LAKE = a_copy_taken_beside_a_kept_build_imports_kept,
    "099, R8: a lake exported while a kept view builds carries the setting alone, since none of the build is appended, and imported it is kept at its first open: the view answers as the lake it came from, and a record written to the copy after it is in its group",
    given
);

check!(
    VIEW_BUILDING_REFUSES_READERS = a_set_or_trigger_over_a_view_a_build_has_not_kept_is_refused,
    "099, R9: a set of a kept view, and a trigger on it, declared while the view's build is not whole are refused `view_building`, naming the view and the set or the trigger, and append nothing; once the setting's door has answered, both land",
    given
);

/// The view over `row`, counting its records, summing and bounding `n`, by
/// `s`.
async fn by_s(l: &Lake) -> Result<(), Failure> {
    let fields = vec![
        field("s", StorageKind::Text),
        field("c", StorageKind::Number),
        field("total", StorageKind::Number),
        field("lo", StorageKind::Number),
        field("hi", StorageKind::Number),
    ];
    let measures = [
        ("c", "count()"),
        ("total", "sum(n)"),
        ("lo", "min(n)"),
        ("hi", "max(n)"),
    ];
    let view = view_of("by_s", "row", &["s"], fields, &measures);
    l.declare(view).await.map(|_| ())
}

/// The view `above` over the view `by_s`: each group, with the count of its
/// records that `by_s` counts and the sum of the counts it sums.
async fn over_a_view(l: &Lake) -> Result<(), Failure> {
    let fields = vec![
        field("s", StorageKind::Text),
        field("c", StorageKind::Number),
        field("total", StorageKind::Number),
    ];
    let measures = [("c", "sum(c)"), ("total", "sum(total)")];
    let view = view_of("above", "by_s", &["s"], fields, &measures);
    l.declare(view).await.map(|_| ())
}

/// What `view` answers: each group's key and measures, sorted so that two
/// reads are compared as answers, not as an order.
async fn groups(l: &Lake, view: &str) -> Result<Vec<String>, Failure> {
    let find = Find {
        limit: Some(1_000),
        ..Default::default()
    };
    let reply = l
        .cat
        .find(&l.own, &TypeName::from(view), find)
        .await
        .must(&format!("a find of `{view}`"))?;
    let Found::Records { rows, .. } = reply.data else {
        return Err(Failure::new(format!(
            "a find of `{view}` answered no records"
        )));
    };
    let line = |r: &Record| {
        let of = |name: &str| r.fields.get(name).cloned().unwrap_or(Value::Null);
        match view {
            "by_s" => format!(
                "{} c={} total={} lo={} hi={}",
                r.key,
                of("c"),
                of("total"),
                of("lo"),
                of("hi")
            ),
            _ => format!("{} c={} total={}", r.key, of("c"), of("total")),
        }
    };
    let mut answers: Vec<String> = rows.iter().map(line).collect();
    answers.sort();
    Ok(answers)
}

/// What `describe_type` says of `view`'s keeping.
async fn keeping(l: &Lake, view: &str) -> Result<Option<Kept>, Failure> {
    Ok(l.describe_type(view).await?.lifecycle.kept)
}

/// Whether `describe_type` says the view `kept`.
fn is_kept(said: &Option<Kept>) -> bool {
    matches!(said, Some(Kept::Kept))
}

async fn a_kept_view_building_in_parts_answers_as_the_lake_did_before(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let unwhole = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let seen = seen.clone();
        let unwhole = unwhole.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            for _ in 0..20 {
                let said = keeping(l, "by_s").await?;
                if !is_kept(&said) {
                    // A view a build has not kept is told `building`, and a
                    // read computes it meanwhile.
                    ensure!(
                        matches!(
                            said,
                            Some(Kept::Computed {
                                why: NotKept::Building
                            })
                        ),
                        "a view a build has not kept must be told `building` by \
                         `describe_type`; it says {said:?}"
                    );
                    unwhole.store(true, Ordering::SeqCst);
                }
                let got = groups(l, "by_s").await?;
                seen.lock().expect("the answers").push(got);
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    ensure!(
        unwhole.load(Ordering::SeqCst),
        "the build was whole before a read was taken beside it, so nothing shows the old road"
    );
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the build answered differently from the lake before \
             `{KEPT}`.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "once the view is kept, `describe_type` must say `kept`; it says {:?}",
        keeping(&l, "by_s").await?
    );
    let after = groups(&l, "by_s").await?;
    ensure!(
        after == before,
        "once the view is kept, its reads answer what the lake answered before \
         `{KEPT}`.\n  before: {before:?}\n  after:  {after:?}"
    );
    Ok(())
}

async fn a_record_written_beside_a_kept_build_is_in_its_group_once_it_is_kept(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let writer = {
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            // `a-low` sorts before every `r…` key the fill walks, `z-late`
            // after every one: whichever side of the cursor the write lands
            // on, the group must hold both records once and no record twice.
            let late = vec![
                source_record("row", "a-low", json!({"n": 1, "s": "late"})),
                source_record("row", "z-late", json!({"n": 2, "s": "late"})),
            ];
            l.ingest("late", late)
                .await
                .must("ingesting records beside the kept build")?;
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(writer), Box::pin(setter)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let kept = groups(&l, "by_s").await?;
    let late = l.read("by_s", "late").await?;
    ensure!(
        late.fields.get("c") == Some(&json!(2)),
        "the group `late` must count the two records written beside the build once: it says \
         {:?}",
        late.fields.get("c")
    );
    ensure!(
        late.fields.get("total") == Some(&json!(3)),
        "the group `late` must sum `1` and `2`: it says {:?}",
        late.fields.get("total")
    );
    // The kept view answers what it computes with the setting taken back:
    // every group the lake held before, and the write's beside them.
    l.retire(SETTING_TYPE, KEPT).await?;
    let computed = groups(&l, "by_s").await?;
    ensure!(
        computed == kept,
        "the kept view must answer what it computes with the setting taken back.\n  kept:     \
         {kept:?}\n  computed: {computed:?}"
    );
    ensure!(
        computed.len() == before.len() + 1,
        "beside the records before it, the lake computes the view with the group `late` alone \
         added: {} groups, not {}.\n  before:   {before:?}\n  computed: {computed:?}",
        computed.len(),
        before.len() + 1
    );
    Ok(())
}

async fn a_write_beside_a_kept_build_is_answered_before_the_setting_is(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let done = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = {
        let done = done.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            l.ingest(
                "late",
                vec![source_record(
                    "row",
                    "late",
                    json!({"n": size + 5, "s": "late"}),
                )],
            )
            .await
            .must("ingesting a record beside the kept build")?;
            done.lock().expect("the order").push("write");
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let done = done.clone();
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            done.lock().expect("the order").push("setting");
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(writer), Box::pin(setter)]).await;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the write: {e}")))?;
    let order = done.lock().expect("the order").clone();
    ensure!(
        order == ["write", "setting"],
        "a write sent beside the build must be answered before the setting's door is; they \
         answered {order:?}"
    );
    Ok(())
}

async fn a_kept_build_stopped_between_parts_is_finished_by_the_next_open(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(400, 800));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let lake = store.lake().clone();
    // The setting's door appends its record and sends the build; the
    // caller then goes, and the lake with it, as a process that ended
    // between two of the build's turns.
    {
        let l = &l;
        let made = l.set(KEPT, json!(true));
        let done = async {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            Ok::<_, Failure>(())
        };
        match until_first(made, done).await {
            First::Done(seen) => seen?,
            First::Made(_) => {
                return Err(Failure::new(
                    "the build was whole before the lake was let go, so nothing was owed",
                ));
            }
        }
    }
    drop(l);
    drop(store);
    // The next open of the same lake, a new writer: it finds the build owed
    // in the mark and sends it, and does not wait for it.
    let none: crate::conformance::Lines = Box::new(|_, _| (Vec::new(), 0));
    let continued = (given.continued)(None, lake, none, OpenMode::Write, Seams::default()).await;
    let continued = continued.map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let store = continued
        .opened
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let l = Lake::of(store.as_ref());
    let during = groups(&l, "by_s").await?;
    ensure!(
        during == before,
        "a lake reopened with a kept build owed computes the view at once as it did before \
         `{KEPT}`: {before:?} became {during:?}"
    );
    let done = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = {
        let done = done.clone();
        let l = &l;
        async move {
            l.ingest(
                "late",
                vec![source_record(
                    "row",
                    "late",
                    json!({"n": size + 5, "s": "late"}),
                )],
            )
            .await
            .must("ingesting a record beside the kept build")?;
            done.lock().expect("the order").push("write");
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let done = done.clone();
        let l = &l;
        async move {
            // This door waits for its own build, which waits for the one
            // the open owes.
            l.set("lookup_index.row", json!(["s"])).await?;
            done.lock().expect("the order").push("setting");
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(writer), Box::pin(setter)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let order = done.lock().expect("the order").clone();
    ensure!(
        order == ["write", "setting"],
        "a write beside the restarted build is answered before a setting whose door waits \
         for it; they answered {order:?}"
    );
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "the build the open took up must finish, so `describe_type` says the view `kept`; it \
         says {:?}",
        keeping(&l, "by_s").await?
    );
    let after = groups(&l, "by_s").await?;
    let late: Vec<&String> = after.iter().filter(|g| g.starts_with("late ")).collect();
    ensure!(
        late.len() == 1,
        "the record written after the restart must be in one group, `late`; the view answers \
         {after:?}"
    );
    let before_again: Vec<String> = after
        .iter()
        .filter(|g| !g.starts_with("late "))
        .cloned()
        .collect();
    ensure!(
        before_again == before,
        "once the owed build and the setting beside it are whole, the groups the lake held \
         before answer as they did.\n  before: {before:?}\n  after:  {before_again:?}"
    );
    let late = l.read("by_s", "late").await?;
    ensure!(
        late.fields.get("c") == Some(&json!(1)),
        "the group `late` must count the record written after the restart once: it says {:?}",
        late.fields.get("c")
    );
    Ok(())
}

async fn a_kept_setting_taken_back_between_two_parts_of_its_build_ends_clean(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(2_000, 4_000));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Set when a read has been taken beside the build: the take-back is
    // sent only then, so it lands between two of the build's parts.
    let beside = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let seen = seen.clone();
        let beside = beside.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            let got = groups(l, "by_s").await?;
            seen.lock().expect("the answers").push(got);
            beside.store(true, Ordering::SeqCst);
            // Reads go on until the take-back has landed and a read is
            // taken after it.
            let mut after_the_take_back = 0;
            for _ in 0..4_000 {
                let got = groups(l, "by_s").await?;
                seen.lock().expect("the answers").push(got);
                if l.read(SETTING_TYPE, KEPT).await.is_err() {
                    after_the_take_back += 1;
                    if after_the_take_back == 3 {
                        break;
                    }
                }
                pause(Duration::from_millis(1)).await;
            }
            ensure!(
                after_the_take_back > 0,
                "no read was taken after the take-back landed: the reads did not span it"
            );
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let taker = {
        let beside = beside.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            for _ in 0..2_000 {
                if beside.load(Ordering::SeqCst) {
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            l.retire(SETTING_TYPE, KEPT).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(taker), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the take-back: {e}")))?;
    out[2]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read: {e}")))?;
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the build taken back answered differently from the lake before \
             `{KEPT}`.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    let after = groups(&l, "by_s").await?;
    ensure!(
        after == before,
        "once the setting is taken back, every read answers what it answered before it: \
         {before:?} became {after:?}"
    );
    ensure!(
        keeping(&l, "by_s").await?
            == Some(Kept::Computed {
                why: NotKept::NotAsked
            }),
        "a setting taken back leaves the view computed, not asked; `describe_type` says {:?}",
        keeping(&l, "by_s").await?
    );
    Ok(())
}

async fn a_kept_setting_set_again_after_a_take_back_is_whole(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(2_000, 4_000));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    // Set once the take-back is sent, with the setting in force: until then
    // its record is absent too, and a set would race the first one.
    let retiring = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let taker = {
        let retiring = retiring.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            retiring.store(true, Ordering::SeqCst);
            l.retire(SETTING_TYPE, KEPT).await?;
            Ok::<_, Failure>(())
        }
    };
    let again = {
        let retiring = retiring.clone();
        let l = &l;
        async move {
            for _ in 0..4_000 {
                if retiring.load(Ordering::SeqCst) {
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            // The take-back has landed once its record is retired, and its
            // door waits for the groups to go: a set now is a write that
            // lands after it.
            let mut gone = false;
            for _ in 0..4_000 {
                if l.read(SETTING_TYPE, KEPT).await.is_err() {
                    gone = true;
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            ensure!(
                gone,
                "the take-back's record was never retired, so the setting was not set again \
                 after it"
            );
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(taker), Box::pin(again)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the take-back: {e}")))?;
    out[2]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting again: {e}")))?;
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "a setting set again is built whole: `describe_type` says `kept`; it says {:?}",
        keeping(&l, "by_s").await?
    );
    let after = groups(&l, "by_s").await?;
    ensure!(
        after == before,
        "a setting set again after a take-back builds the view whole, so every read answers \
         what it answered before the setting: {before:?} became {after:?}"
    );
    Ok(())
}

async fn a_rebuild_holds_a_kept_view_whole_at_its_event(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    l.set(KEPT, json!(true)).await?;
    let built = groups(&l, "by_s").await?;
    store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        built == before,
        "the built lake answers what it answered before `{KEPT}`: {before:?} became {built:?}"
    );
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "a projection rebuilt from the log holds the setting whole at its event, so \
         `describe_type` says `kept`; it says {:?}",
        keeping(&l, "by_s").await?
    );
    let rebuilt = groups(&l, "by_s").await?;
    ensure!(
        rebuilt == before,
        "a projection rebuilt from the log folds the view whole at its setting's event, so \
         every read answers what the built lake answered: {before:?} became {rebuilt:?}"
    );
    Ok(())
}

async fn a_kept_group_whose_smallest_record_moves_beside_the_build_answers_its_next_smallest(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let writer = {
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            // `a-hold` sorts before every `r…` key the fill walks: written
            // now, the fill folds it, or the writer applies it beside the
            // cursor. Either way it is `s0`'s smallest.
            l.ingest(
                "lower",
                vec![source_record("row", "a-hold", json!({"n": -1, "s": "s0"}))],
            )
            .await
            .must("ingesting a record below every other")?;
            // It leaves the group: `s0`'s smallest must be the next record
            // the build walked, read from the order, and the raised record
            // its largest.
            l.ingest(
                "raise",
                vec![source_record(
                    "row",
                    "a-hold",
                    json!({"n": size + 1, "s": "s0"}),
                )],
            )
            .await
            .must("raising the record beside the build")?;
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(writer)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the write: {e}")))?;
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "the build beside the write must be whole, so `describe_type` says `kept`; it says {:?}",
        keeping(&l, "by_s").await?
    );
    let s0 = l.read("by_s", "s0").await?;
    ensure!(
        s0.fields.get("lo") == Some(&json!(0)),
        "the group `s0` keeps its smallest from the order the build walked, `0`, once the \
         record written beside it left; it says {:?}",
        s0.fields.get("lo")
    );
    ensure!(
        s0.fields.get("hi") == Some(&json!(size + 1)),
        "the group `s0` holds the raised record as its largest, `{}`; it says {:?}",
        size + 1,
        s0.fields.get("hi")
    );
    let kept = groups(&l, "by_s").await?;
    l.retire(SETTING_TYPE, KEPT).await?;
    let computed = groups(&l, "by_s").await?;
    ensure!(
        kept == computed,
        "the kept view must answer what it computes with the setting taken back.\n  kept:     \
         {kept:?}\n  computed: {computed:?}"
    );
    ensure!(
        computed != before,
        "the records written beside the build must show: the lake computes {computed:?}, as it \
         did before `{KEPT}`: {before:?}"
    );
    Ok(())
}

async fn a_kept_view_over_a_view_starts_after_its_inner_view_is_kept(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    over_a_view(&l).await?;
    let before = groups(&l, "above").await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            // The setting's fold has landed once the inner view answers
            // `building`: the outer view's setting was appended before it,
            // and by then the outer is either started or still not asked,
            // never made.
            let mut building = false;
            for _ in 0..20_000 {
                if matches!(
                    keeping(l, "by_s").await?,
                    Some(Kept::Computed {
                        why: NotKept::Building
                    })
                ) {
                    building = true;
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            ensure!(
                building,
                "the inner view's build never answered `building`, so nothing shows where the \
                 outer view starts; `describe_type` says {:?}",
                keeping(l, "by_s").await?
            );
            let beside = keeping(l, "above").await?;
            ensure!(
                !is_kept(&beside),
                "the outer view must not be kept while the inner view answers `building`: a \
                 view over a view starts after its inner view's `Keep`; it says {beside:?}"
            );
            // A declaration beside the build reads every asked view in, the
            // inner among them as one a build is making: the outer must
            // start then too, not be made over the part the inner reached.
            l.declare(type_decl("aside", vec![sourced("k", StorageKind::Text)]))
                .await?;
            let beside = keeping(l, "above").await?;
            ensure!(
                !is_kept(&beside),
                "a declaration beside the build must not make the outer view over the part \
                 the inner view's build has reached; it says {beside:?}"
            );
            for _ in 0..20_000 {
                let inner = is_kept(&keeping(l, "by_s").await?);
                let outer = is_kept(&keeping(l, "above").await?);
                seen.lock().expect("the answers").push((inner, outer));
                if inner && outer {
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            // The outer view's setting first: its inner view is not kept, so
            // the outer is not made beside it.
            l.set("view_kept.above", json!(true)).await?;
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the settings: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read: {e}")))?;
    let seen = seen.lock().expect("the answers").clone();
    let beside = seen.iter().filter(|(inner, _)| !*inner).count();
    ensure!(
        beside > 0,
        "no read was taken while the inner view's build ran, so nothing shows where the outer \
         view starts"
    );
    for (i, (inner, outer)) in seen.iter().enumerate() {
        ensure!(
            *inner || !*outer,
            "sample {i} says the outer view `kept` while the inner view's build was not whole: \
             a view over a view starts after its inner view's `Keep`"
        );
    }
    ensure!(
        is_kept(&keeping(&l, "by_s").await?) && is_kept(&keeping(&l, "above").await?),
        "once the inner view's door answers, both views are kept; `describe_type` says by_s {:?} \
         and above {:?}",
        keeping(&l, "by_s").await?,
        keeping(&l, "above").await?
    );
    let kept = groups(&l, "above").await?;
    ensure!(
        kept == before,
        "the outer view answers what it answered before the settings: {before:?} became {kept:?}"
    );
    l.retire(SETTING_TYPE, "view_kept.above").await?;
    l.retire(SETTING_TYPE, KEPT).await?;
    let computed = groups(&l, "above").await?;
    ensure!(
        computed == kept,
        "the outer view must answer what it computes with the settings taken back.\n  kept:     \
         {kept:?}\n  computed: {computed:?}"
    );
    Ok(())
}

async fn a_type_declared_again_beside_a_kept_build_is_whole_once_it_is(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            for _ in 0..20 {
                let got = groups(l, "by_s").await?;
                seen.lock().expect("the answers").push(got);
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let declarer = {
        let l = &l;
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            // The same fields and one more: the type's shape moves while
            // its kept view builds, as a declaration that changes it does.
            let fields = vec![
                sourced("n", StorageKind::Number),
                sourced("s", StorageKind::Text),
                sourced("t", StorageKind::Text),
            ];
            l.declare(type_decl("row", fields)).await?;
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(declarer), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the declaration: {e}")))?;
    out[2]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read: {e}")))?;
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the declaration answered differently from the lake before the \
             setting.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    ensure!(
        is_kept(&keeping(&l, "by_s").await?),
        "a type declared again beside the build does not leave the view part built: \
         `describe_type` says `kept`; it says {:?}",
        keeping(&l, "by_s").await?
    );
    let kept = groups(&l, "by_s").await?;
    l.retire(SETTING_TYPE, KEPT).await?;
    let computed = groups(&l, "by_s").await?;
    ensure!(
        kept == computed,
        "the view declared again must answer what it computes with the setting taken back.\n  \
         kept:     {kept:?}\n  computed: {computed:?}"
    );
    ensure!(
        computed == before,
        "the field added beside the build changes no answer: {before:?} became {computed:?}"
    );
    Ok(())
}

async fn a_copy_taken_beside_a_kept_build_imports_kept(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l, "by_s").await?;
    let scratch = Scratch::new(given.scratch.clone());
    let dir = scratch.dir("beside-a-kept-build");
    let s = store.as_ref();
    // The setting's door appends its record, then builds; the copy is
    // taken while the build runs, so the log holds the setting alone.
    let setter = {
        let l = &l;
        async move {
            l.set(KEPT, json!(true)).await?;
            Ok::<_, Failure>(())
        }
    };
    let exporter = {
        let l = &l;
        let dir = dir.clone();
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            crate::export::write(s, &dir)
                .await
                .must("exporting the lake beside the kept build")?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(exporter)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let seams = Seams {
        backings: backing::backings(),
        executors: backing::executors(),
        ..Seams::default()
    };
    let imported = (given.import)(dir, store.lake().clone(), None, seams).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing the copy: {e}")))?;
    let opened = (imported.writer)(imported.store, Seams::default()).await;
    let opened = opened.map_err(|e| Failure::new(format!("opening the copy to write: {e}")))?;
    let i = Lake::of(opened.as_ref());
    ensure!(
        is_kept(&keeping(&i, "by_s").await?),
        "a copy taken beside the build carries the setting alone, so its lake keeps the view \
         whole at its first open; `describe_type` says {:?}",
        keeping(&i, "by_s").await?
    );
    let after = groups(&i, "by_s").await?;
    ensure!(
        after == before,
        "the imported lake answers as the lake the copy came from: {before:?} became {after:?}"
    );
    i.ingest(
        "late",
        vec![source_record(
            "row",
            "late",
            json!({"n": size + 5, "s": "late"}),
        )],
    )
    .await
    .must("ingesting a record into the copy")?;
    let late = i.read("by_s", "late").await?;
    ensure!(
        late.fields.get("c") == Some(&json!(1)),
        "a record written to the copy after its setting is in its group once: it says {:?}",
        late.fields.get("c")
    );
    Ok(())
}

async fn a_set_or_trigger_over_a_view_a_build_has_not_kept_is_refused(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let readers = [
        (
            "object_set",
            "busy",
            json!({"of": "by_s", "where": "c > 0"}),
        ),
        ("trigger", "grew", json!({"on": "by_s"})),
    ];
    let refused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let declarer = {
        let (l, s, refused, readers) = (&l, store.as_ref(), refused.clone(), &readers);
        async move {
            ensure!(
                until_appended(l, KEPT).await?,
                "`{KEPT}` was never appended, so no build began"
            );
            for (code, name, d) in readers {
                let before = s.head().await.must("the head")?;
                let d = declaration(code, name, d);
                let r = l.cat.declare(&l.own, d).await;
                if r.is_ok() {
                    // It landed: the build was whole by then.
                    ensure!(
                        is_kept(&keeping(l, "by_s").await?),
                        "the {code} over `by_s` landed while its build was not whole"
                    );
                    continue;
                }
                let at = json!([{"code": code, "name": name}]);
                let want = json!({"code": "view_building", "type": "by_s", "at": at});
                refused_like(&format!("the {code} over a view in its build"), r, &want)?;
                head_unmoved(s, before, &format!("the {code}")).await?;
                refused.store(true, Ordering::SeqCst);
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = async {
        l.set(KEPT, json!(true)).await?;
        Ok::<_, Failure>(())
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(declarer)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    ensure!(
        refused.load(Ordering::SeqCst),
        "the build was whole before a declaration was sent beside it, so nothing was refused"
    );
    for (code, name, d) in &readers {
        l.cat
            .declare(&l.own, declaration(code, name, d))
            .await
            .must(name)?;
    }
    Ok(())
}

/// The declaration of the `code` named `name`, with `fields`.
fn declaration(code: &str, name: &str, fields: &Value) -> Declaration {
    let mut d = fields.clone();
    d["declare"] = json!(code);
    d["name"] = json!(name);
    serde_json::from_value(d).expect("a declaration")
}
