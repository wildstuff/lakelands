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
    "099, R8: a kept view's build whose writer goes between two of its parts is owed in the projection's mark, cursors and all, and the next open of the lake sends it again without waiting: the reopened lake computes the view at once as before, a write sent beside it is answered before a setting whose door waits for the build, and once that setting is whole the view is kept and answers what it computed",
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

/// The view over `row`, counting its records and summing `n`, by `s`.
async fn by_s(l: &Lake) -> Result<(), Failure> {
    let fields = vec![
        field("s", StorageKind::Text),
        field("c", StorageKind::Number),
        field("total", StorageKind::Number),
    ];
    let measures = [("c", "count()"), ("total", "sum(n)")];
    let view = view_of("by_s", "row", &["s"], fields, &measures);
    l.declare(view).await.map(|_| ())
}

/// What the view `by_s` answers: each group's key, count and total, sorted
/// so that two reads are compared as answers, not as an order.
async fn groups(l: &Lake) -> Result<Vec<String>, Failure> {
    let mut find = Find::default();
    find.limit = Some(1_000);
    let reply = l
        .cat
        .find(&l.own, &TypeName::from("by_s"), find)
        .await
        .must("a find of `by_s`")?;
    let Found::Records { rows, .. } = reply.data else {
        return Err(Failure::new(
            "a find of `by_s` answered no records".to_owned(),
        ));
    };
    let line = |r: &Record| {
        let of = |name: &str| r.fields.get(name).cloned().unwrap_or(Value::Null);
        format!("{} c={} total={}", r.key, of("c"), of("total"))
    };
    let mut answers: Vec<String> = rows.iter().map(line).collect();
    answers.sort();
    Ok(answers)
}

/// What `describe_type` says of the view's keeping.
async fn keeping(l: &Lake) -> Result<Option<Kept>, Failure> {
    Ok(l.describe_type("by_s").await?.lifecycle.kept)
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
    let before = groups(&l).await?;
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
                if !is_kept(&keeping(l).await?) {
                    unwhole.store(true, Ordering::SeqCst);
                }
                let got = groups(l).await?;
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
        is_kept(&keeping(&l).await?),
        "once the view is kept, `describe_type` must say `kept`; it says {:?}",
        keeping(&l).await?
    );
    let after = groups(&l).await?;
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
    let before = groups(&l).await?;
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
    let kept = groups(&l).await?;
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
    let computed = groups(&l).await?;
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
    let before = groups(&l).await?;
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
    let during = groups(&l).await?;
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
        is_kept(&keeping(&l).await?),
        "the build the open took up must finish, so `describe_type` says the view `kept`; it \
         says {:?}",
        keeping(&l).await?
    );
    let after = groups(&l).await?;
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
    let before = groups(&l).await?;
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
            let got = groups(l).await?;
            seen.lock().expect("the answers").push(got);
            beside.store(true, Ordering::SeqCst);
            // Reads go on until the take-back has landed and a read is
            // taken after it.
            let mut after_the_take_back = 0;
            for _ in 0..4_000 {
                let got = groups(l).await?;
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
    let after = groups(&l).await?;
    ensure!(
        after == before,
        "once the setting is taken back, every read answers what it answered before it: \
         {before:?} became {after:?}"
    );
    ensure!(
        keeping(&l).await?
            == Some(Kept::Computed {
                why: NotKept::NotAsked
            }),
        "a setting taken back leaves the view computed, not asked; `describe_type` says {:?}",
        keeping(&l).await?
    );
    Ok(())
}

async fn a_kept_setting_set_again_after_a_take_back_is_whole(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(2_000, 4_000));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    by_s(&l).await?;
    let before = groups(&l).await?;
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
        is_kept(&keeping(&l).await?),
        "a setting set again is built whole: `describe_type` says `kept`; it says {:?}",
        keeping(&l).await?
    );
    let after = groups(&l).await?;
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
    let before = groups(&l).await?;
    l.set(KEPT, json!(true)).await?;
    let built = groups(&l).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        built == before,
        "the built lake answers what it answered before `{KEPT}`: {before:?} became {built:?}"
    );
    ensure!(
        is_kept(&keeping(&l).await?),
        "a projection rebuilt from the log holds the setting whole at its event, so \
         `describe_type` says `kept`; it says {:?}",
        keeping(&l).await?
    );
    let rebuilt = groups(&l).await?;
    ensure!(
        rebuilt == before,
        "a projection rebuilt from the log folds the view whole at its setting's event, so \
         every read answers what the built lake answered: {before:?} became {rebuilt:?}"
    );
    Ok(())
}
