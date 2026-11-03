//! A setting's index is built in parts beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`, R7).
//!
//! `sort_index.<type>` and `lookup_index.<type>` no longer walk the type
//! in the turn that appends them: the fold marks what the setting asks
//! for, and the build fills it a key range at a time. These checks hold
//! what a caller sees: every read answers as before until the build is
//! whole, a write sent beside it goes on and reaches both sides, a record
//! retired beside it leaves the index, a lake whose writer went between
//! two turns finishes the build at its next open, a type declared again
//! is whole once the build is, a projection rebuilt from the log holds
//! the setting whole at its event, and a copy taken beside the build
//! imports whole.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use lakelands_api::engine::OpenMode;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::SETTING_TYPE;

use super::export::Scratch;
use crate::conformance::fixture::{order, source_record};
use crate::conformance::prelude::*;

check!(
    BUILD_READ_ANSWERS_AS_BEFORE = a_setting_that_builds_in_parts_changes_no_answer_either_kind,
    "099, R7: a `sort_index.<type>` and a `lookup_index.<type>` setting on a type with records build in parts of `limits`, each a turn beside the writer; a page ordered by the field and a `where` on a looked-up text answer during the build, and after it, exactly what they answered before it",
    given
);

check!(
    BUILD_NEVER_READ_PART_BUILT = a_page_beside_the_build_is_whole_and_never_a_part_built_index,
    "099, R7: the rows of a build are not read until its `Keep`: a page ordered by the field taken while it builds is the whole type, every key in its place, never the rows filled so far",
    given
);

check!(
    BUILD_WRITES_GO_ON = a_write_beside_the_build_is_answered_before_the_setting_is,
    "099, R7: the setting's door answers when the build is whole, and a write sent beside the build waits for at most the turn in hand, so it is answered before the setting is",
    given
);

check!(
    BUILD_WRITE_REACHES_BOTH_SIDES =
        a_record_written_beside_the_build_is_in_the_index_once_it_is_whole,
    "099, R7: a record written beside the build, after the setting's event, is in the index once the build is whole, beside the records written before it: a page ordered by the field ends with it",
    given
);

check!(
    BUILD_RESTART_FINISHES = a_build_stopped_between_turns_is_finished_by_the_next_open,
    "099, R7: a setting's build whose writer goes between two of its turns is owed in the projection's mark, and the next open of the lake sends it again without waiting: the reopened lake answers as before at once, a write sent beside it is answered before a setting whose door waits for the build, and once that setting is whole the field's page is the whole type with every key in its place",
    given
);

check!(
    BUILD_SETTING_TAKEN_BACK = a_setting_taken_back_between_two_parts_of_its_build_ends_clean,
    "099, R7: a setting taken back between two parts of its build ends clean: `describe` is seen naming the field `building` before the setting is retired, every read taken beside the build, and after the take-back landed, answers what it answered before the setting, the door that retired it answering only once its rows are gone, and `describe` names the field no longer `sorted`, `chosen` nor `building`; the chunks and the mark `dropping` they run under are pinned by the engine seam's `CHOSEN_ORDER`",
    given
);

check!(
    BUILD_SETTING_SET_AGAIN = a_setting_set_again_between_two_chunks_of_its_take_back_is_whole,
    "099, R7: a setting set again between two chunks of its take-back cancels the drop and builds whole: the setting's fold takes the field out of the mark `dropping`, the next drop step finds a field a setting names and does nothing, the setting's own build fills and keeps the field, and every read answers what it answered before the setting",
    given
);

check!(
    BUILD_TYPE_DECLARED_AGAIN = a_type_declared_again_beside_the_build_is_whole_once_it_is,
    "099, R7: a type declared again beside the build, with a field added, does not leave the index part built: every read answers as before beside the declaration, and once the build is whole the field's page is the whole type, every key in its place",
    given
);

check!(
    BUILD_BESIDE_A_REMOVAL = a_record_retired_beside_the_build_is_not_in_the_index,
    "099, R7: a record retired beside the build is not in the index once it is whole, and every other record is: the build's rows follow the fold, since retirements are events",
    given
);

check!(
    BUILD_REBUILD_SAME_LAKE = a_rebuild_holds_the_setting_whole_at_its_event,
    "099, R7: a setting is in the log and its mark in the projection, so a projection rebuilt from the log holds it whole at its event, with no mark; every read answers what the built lake answered",
    given
);

check!(
    BUILD_EXPORT_IMPORT_SAME_LAKE = a_copy_taken_beside_the_build_imports_whole,
    "099, R7: a lake exported while a setting builds carries the setting alone, since none of the build is appended, and imported it answers as the lake it came from: its writer rebuilds the settings whole, every page and `where` the same, and a record written after it sorts last",
    given
);

/// `size` records of `row`: `n` a number and `s` a text.
pub(super) async fn rows(store: &dyn LakeStore, size: u64) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let fields = vec![
        sourced("n", StorageKind::Number),
        sourced("s", StorageKind::Text),
    ];
    l.declare(type_decl("row", fields)).await?;
    let records = (0..size)
        .map(|i| {
            source_record(
                "row",
                &format!("r{i:05}"),
                json!({"n": i, "s": format!("s{}", i % 17)}),
            )
        })
        .collect();
    l.ingest("load", records).await.must("ingesting the rows")?;
    Ok(l)
}

/// A page of `limit` records ordered first by `field`, from `offset`.
fn by(field: &str, limit: u32, offset: u64) -> Find {
    Find {
        order_by: vec![order(field, OrderBy::Value, Direction::Asc)],
        limit: Some(limit),
        offset: u32::try_from(offset).ok().filter(|o| *o > 0),
        ..Find::default()
    }
}

/// The keys of a page ordered by `field`.
async fn page(l: &Lake, field: &str, limit: u32, offset: u64) -> Result<Vec<String>, Failure> {
    keys(l, by(field, limit, offset)).await
}

/// The keys `find` answers.
async fn keys(l: &Lake, find: Find) -> Result<Vec<String>, Failure> {
    match l
        .cat
        .find(&l.own, &"row".into(), find)
        .await
        .must("a find")?
        .data
    {
        Found::Records { rows, .. } => Ok(rows.iter().map(|r| r.key.to_string()).collect()),
        other => Err(Failure::new(format!(
            "a page of records answered {other:?}"
        ))),
    }
}

/// The answers the checks compare: the first and a deep page ordered by
/// `n`, and every record whose `s` is `value`.
async fn answers(l: &Lake, size: u64) -> Result<Vec<Vec<String>>, Failure> {
    let deep = size.saturating_sub(50);
    Ok(vec![
        page(l, "n", 50, 0).await?,
        page(l, "n", 50, deep).await?,
        keys(l, find_where("s = 's3'")).await?,
    ])
}

/// The lake opened again with one record a chunk, so a build spans many
/// turns on every engine.
pub(super) async fn chunked(given: &Given, store: Store) -> Result<Store, Failure> {
    let limits = Limits {
        order_chunk: 1,
        ..limits()
    };
    (given.relimit)(store, Seams::default(), limits)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))
}

/// The lake opened again as [`chunked`] opens it, and with the lake's own
/// choice of an order index off: the setting the check writes is the only
/// order index the type has, and a page sorted whole is read as it always
/// was.
pub(super) async fn chunked_alone(given: &Given, store: Store) -> Result<Store, Failure> {
    let limits = Limits {
        order_chunk: 1,
        choose_order_index: false,
        ..limits()
    };
    (given.relimit)(store, Seams::default(), limits)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))
}

/// Wait, up to a while, for `describe` to name `field` of `ty` `building`:
/// an index build is filling it in parts.
pub(super) async fn until_building(l: &Lake, ty: &str, field: &str) -> Result<bool, Failure> {
    until_building_within(l, ty, field, 2_000).await
}

/// Wait up to `tries` milliseconds for `describe` to name `field` of `ty`
/// `building`: a build that has parts here is filling it.
pub(super) async fn until_building_within(
    l: &Lake,
    ty: &str,
    field: &str,
    tries: u32,
) -> Result<bool, Failure> {
    for _ in 0..tries {
        let d = l.describe_type(ty).await?;
        if d.lifecycle.building.iter().any(|f| f.as_str() == field) {
            return Ok(true);
        }
        pause(Duration::from_millis(1)).await;
    }
    Ok(false)
}

/// Wait, up to a while, for a setting's record to be in the log: the
/// door has appended it and the build is owed.
async fn until_appended(l: &Lake, key: &str) -> Result<bool, Failure> {
    for _ in 0..2_000 {
        if l.read(SETTING_TYPE, key).await.is_ok() {
            return Ok(true);
        }
        pause(Duration::from_millis(1)).await;
    }
    Ok(false)
}

/// Which of two futures answered first.
enum First<A, B> {
    Made(A),
    Done(B),
}

/// Polls `made` and `done` together until one answers. When `done` does,
/// `made` is polled once more first: a door that has appended its record
/// sends its build in that poll, before it is dropped here, so the lake
/// is left with a build between its turns.
async fn until_first<A, B>(
    made: impl Future<Output = A>,
    done: impl Future<Output = B>,
) -> First<A, B> {
    let (mut made, mut done) = (Box::pin(made), Box::pin(done));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(made) = made.as_mut().poll(cx) {
            return Poll::Ready(First::Made(made));
        }
        if let Poll::Ready(done) = done.as_mut().poll(cx) {
            let _ = made.as_mut().poll(cx);
            return Poll::Ready(First::Done(done));
        }
        Poll::Pending
    })
    .await
}

async fn a_setting_that_builds_in_parts_changes_no_answer_either_kind(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    // Both kinds: the order index, then the exact-text index. Each is
    // marked building by the setting's fold, and filled a record a turn.
    for (key, value) in [
        ("sort_index.row", json!(["n"])),
        ("lookup_index.row", json!(["s"])),
    ] {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reader = {
            let seen = seen.clone();
            let l = &l;
            async move {
                ensure!(
                    until_appended(l, key).await?,
                    "`{key}` was never appended, so no build began"
                );
                for _ in 0..20 {
                    let got = answers(l, size).await?;
                    seen.lock().expect("the answers").push(got);
                    pause(Duration::from_millis(1)).await;
                }
                Ok::<_, Failure>(())
            }
        };
        let setter = {
            let l = &l;
            async move {
                l.set(key, value).await?;
                Ok::<_, Failure>(())
            }
        };
        let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
        out[0]
            .as_ref()
            .map_err(|e| Failure::new(format!("{key}: {e}")))?;
        out[1]
            .as_ref()
            .map_err(|e| Failure::new(format!("{key}: {e}")))?;
        let during = seen.lock().expect("the answers").clone();
        for (i, got) in during.iter().enumerate() {
            ensure!(
                *got == before,
                "read {i} beside the build of `{key}` answered differently from the lake \
                 before it.\n  before: {before:?}\n  beside: {got:?}"
            );
        }
        let after = answers(&l, size).await?;
        ensure!(
            after == before,
            "once `{key}` is whole, its reads answer what the lake answered before it.\n  \
             before: {before:?}\n  after:  {after:?}"
        );
    }
    Ok(())
}

async fn a_page_beside_the_build_is_whole_and_never_a_part_built_index(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let whole = u32::try_from(size).unwrap_or(u32::MAX);
    let ordered = page(&l, "n", whole, 0).await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
            );
            for _ in 0..20 {
                let got = page(l, "n", whole, 0).await?;
                seen.lock().expect("the pages").push(got);
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let during = seen.lock().expect("the pages").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == ordered,
            "page {i} taken beside the build is not the whole type: {} keys, not {}. A read \
             must never answer from a part-built index, and every key is in its place",
            got.len(),
            ordered.len()
        );
    }
    Ok(())
}

async fn a_write_beside_the_build_is_answered_before_the_setting_is(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let done = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = {
        let done = done.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
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
            .must("ingesting a record beside the build")?;
            done.lock().expect("the order").push("write");
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let done = done.clone();
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
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

async fn a_record_written_beside_the_build_is_in_the_index_once_it_is_whole(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let early = page(&l, "n", 50, 0).await?;
    let writer = {
        let l = &l;
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
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
            .must("ingesting a record beside the build")?;
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(writer), Box::pin(setter)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let first = page(&l, "n", 50, 0).await?;
    let last = page(&l, "n", 1, size).await?;
    ensure!(
        first == early,
        "the records written before the setting are in the index, in their order: {early:?} \
         became {first:?}"
    );
    ensure!(
        last == ["late"],
        "a record written beside the build sorts last by `n`, and the index built beside the \
         writes must hold it; the last page was {last:?}"
    );
    Ok(())
}

async fn a_record_retired_beside_the_build_is_not_in_the_index(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let retired = "r00005";
    let writer = {
        let l = &l;
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
            );
            l.retire("row", retired).await?;
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(writer), Box::pin(setter)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let whole = u32::try_from(size).unwrap_or(u32::MAX);
    let got = page(&l, "n", whole, 0).await?;
    ensure!(
        !got.iter().any(|k| k == retired),
        "`{retired}` was retired beside the build and must not be in the index"
    );
    ensure!(
        got.len() == usize::try_from(size - 1).unwrap_or(usize::MAX),
        "every other record is in the index: {} keys, not {}",
        got.len(),
        size - 1
    );
    Ok(())
}

async fn a_rebuild_holds_the_setting_whole_at_its_event(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    l.set("sort_index.row", json!(["n"])).await?;
    l.set("lookup_index.row", json!(["s"])).await?;
    let built = answers(&l, size).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = answers(&l, size).await?;
    ensure!(
        built == before,
        "the built lake answers what it answered before the settings: {before:?} became \
         {built:?}"
    );
    ensure!(
        rebuilt == before,
        "a projection rebuilt from the log holds the settings whole at their events, so every \
         read answers what the built lake answered: {before:?} became {rebuilt:?}"
    );
    Ok(())
}

async fn a_setting_taken_back_between_two_parts_of_its_build_ends_clean(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(2_000, 4_000));
    // The lake's own choice is off: the setting's field is the type's only
    // order index, so what `describe` says is the setting's alone.
    let store = chunked_alone(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    // The first thousand keys, a page `find` answers whole: the type is
    // larger than one page, and the order of the first thousand says
    // whether the index is whole.
    let whole = 1_000;
    let ordered = page(&l, "n", whole, 0).await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Set when a read has been taken beside the build: the take-back is
    // sent only then, so it lands between two of the build's parts every
    // run.
    let beside = Arc::new(AtomicBool::new(false));
    let reader = {
        let seen = seen.clone();
        let beside = beside.clone();
        let l = &l;
        async move {
            // The build is gated on its mark, not on the setting's record:
            // the take-back retires that record between two of the build's
            // parts, and it may be gone before a read waits on it.
            ensure!(
                until_building(l, "row", "n").await?,
                "the build of `n` in parts ended before a read was taken beside it"
            );
            let got = answers(l, size).await?;
            seen.lock().expect("the answers").push(got);
            beside.store(true, Ordering::SeqCst);
            // Reads go on until the take-back has landed and a read is
            // taken after it: the setting's record goes at its fold, and
            // the drop runs its chunks behind it.
            let mut after_the_take_back = 0;
            for _ in 0..4_000 {
                let got = answers(l, size).await?;
                seen.lock().expect("the answers").push(got);
                if l.read(SETTING_TYPE, "sort_index.row").await.is_err() {
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
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let taker = {
        let beside = beside.clone();
        let l = &l;
        async move {
            // The build holds `n` building: retiring the setting is a
            // write that lands between two of its parts, once a read has
            // been taken beside it.
            ensure!(
                until_building(l, "row", "n").await?,
                "the build of `n` in parts ended before the setting was taken back"
            );
            for _ in 0..2_000 {
                if beside.load(Ordering::SeqCst) {
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            l.retire(SETTING_TYPE, "sort_index.row").await?;
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
             the setting.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the setting is taken back, every read answers what it answered before it: \
         {before:?} became {after:?}"
    );
    let got = page(&l, "n", whole, 0).await?;
    ensure!(
        got == ordered,
        "with the setting taken back, the first {whole} keys of a page ordered by `n` are in \
         their order: {} keys, not {}",
        got.len(),
        ordered.len()
    );
    let d = l.describe_type("row").await?;
    let names = |fs: &[lakelands_api::FieldName]| -> Vec<String> {
        fs.iter().map(|f| f.to_string()).collect()
    };
    ensure!(
        d.lifecycle.sorted.is_empty(),
        "a setting taken back names no `sorted` field; `describe` said {:?}",
        names(&d.lifecycle.sorted)
    );
    ensure!(
        d.lifecycle.chosen.is_empty(),
        "a setting taken back names no `chosen` field; `describe` said {:?}",
        names(&d.lifecycle.chosen)
    );
    ensure!(
        d.lifecycle.building.is_empty(),
        "a build taken back ends with nothing `building`; `describe` said {:?}",
        names(&d.lifecycle.building)
    );
    Ok(())
}

async fn a_setting_set_again_between_two_chunks_of_its_take_back_is_whole(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(2_000, 4_000));
    let store = chunked_alone(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let whole = 1_000;
    let ordered = page(&l, "n", whole, 0).await?;
    let setter = {
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    // Set once the take-back is sent, with the setting in force: until
    // then its record is absent too, and a set would race the first one.
    let retiring = Arc::new(AtomicBool::new(false));
    let taker = {
        let retiring = retiring.clone();
        let l = &l;
        async move {
            // The build holds `n` building: retiring the setting is a
            // write that lands between two of its parts.
            ensure!(
                until_building(l, "row", "n").await?,
                "the build of `n` in parts ended before the setting was taken back"
            );
            retiring.store(true, Ordering::SeqCst);
            l.retire(SETTING_TYPE, "sort_index.row").await?;
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
            // The take-back's fold has landed once its record is retired,
            // and the drop runs its chunks behind it: a set now is a write
            // that lands between two of them.
            let mut gone = false;
            for _ in 0..4_000 {
                if l.read(SETTING_TYPE, "sort_index.row").await.is_err() {
                    gone = true;
                    break;
                }
                pause(Duration::from_millis(1)).await;
            }
            ensure!(
                gone,
                "the take-back's record was never retired, so the setting was not set again \
                 between two chunks of its drop"
            );
            l.set("sort_index.row", json!(["n"])).await?;
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
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "a setting set again between two chunks of its take-back builds whole, so every read \
         answers what it answered before the setting: {before:?} became {after:?}"
    );
    let got = page(&l, "n", whole, 0).await?;
    ensure!(
        got == ordered,
        "the field set again is built whole: the first {whole} keys of a page ordered by `n` \
         are in their order, {} keys, not {}",
        got.len(),
        ordered.len()
    );
    let d = l.describe_type("row").await?;
    let names = |fs: &[lakelands_api::FieldName]| -> Vec<String> {
        fs.iter().map(|f| f.to_string()).collect()
    };
    ensure!(
        names(&d.lifecycle.sorted) == ["n"],
        "a setting set again names `n` `sorted`; `describe` said {:?}",
        names(&d.lifecycle.sorted)
    );
    ensure!(
        d.lifecycle.chosen.is_empty(),
        "the lake chooses nothing with its switch off; `describe` said {:?}",
        names(&d.lifecycle.chosen)
    );
    ensure!(
        d.lifecycle.building.is_empty(),
        "the setting set again is built whole: nothing is `building`; `describe` said {:?}",
        names(&d.lifecycle.building)
    );
    Ok(())
}

async fn a_type_declared_again_beside_the_build_is_whole_once_it_is(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let whole = u32::try_from(size).unwrap_or(u32::MAX);
    let ordered = page(&l, "n", whole, 0).await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
            );
            for _ in 0..20 {
                let got = answers(l, size).await?;
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
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
            );
            // The same fields and one more: the type's shape moves while
            // its index builds, as a declaration that changes it does.
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
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(declarer), Box::pin(reader)]).await;
    out[0].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[1].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    out[2].as_ref().map_err(|e| Failure::new(format!("{e}")))?;
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the declaration answered differently from the lake before the \
             setting.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the build beside the declaration is whole, every read answers as before: \
         {before:?} became {after:?}"
    );
    let got = page(&l, "n", whole, 0).await?;
    ensure!(
        got == ordered,
        "a type declared again beside its build must not leave the index part built: the page \
         ordered by `n` is {} keys, not the whole type's {}, or not in its order",
        got.len(),
        ordered.len()
    );
    Ok(())
}

async fn a_build_stopped_between_turns_is_finished_by_the_next_open(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(400, 800));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let whole = u32::try_from(size).unwrap_or(u32::MAX);
    let ordered = page(&l, "n", whole, 0).await?;
    let lake = store.lake().clone();
    // The setting's door appends its record and sends the build; the
    // caller then goes, and the lake with it, as a process that ended
    // between two of the build's turns.
    {
        let l = &l;
        let made = l.set("sort_index.row", json!(["n"]));
        let done = async {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
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
    // The next open of the same lake, a new writer: it finds the build
    // owed in the mark and sends it, and does not wait for it.
    let none: crate::conformance::Lines = Box::new(|_, _| (Vec::new(), 0));
    let continued = (given.continued)(None, lake, none, OpenMode::Write, Seams::default()).await;
    let continued = continued.map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let store = continued
        .opened
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let l = Lake::of(store.as_ref());
    let during = answers(&l, size).await?;
    ensure!(
        during == before,
        "a lake reopened with a build owed answers at once as it answered before the \
         setting: {before:?} became {during:?}"
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
            .must("ingesting a record beside the restarted build")?;
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
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the builds beside the restart are whole, every read answers as before: \
         {before:?} became {after:?}"
    );
    let got = page(&l, "n", whole, 0).await?;
    ensure!(
        got == ordered,
        "the build the open took up finishes the index it stopped in: the page ordered by \
         `n` is {} keys, not the whole type's {}, or not in its order",
        got.len(),
        ordered.len()
    );
    let last = page(&l, "n", 1, size).await?;
    ensure!(
        last == ["late"],
        "a record written after the restart sorts last by `n`, and the index built beside \
         it must hold it; the last page was {last:?}"
    );
    Ok(())
}

async fn a_copy_taken_beside_the_build_imports_whole(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let whole = u32::try_from(size).unwrap_or(u32::MAX);
    let ordered = page(&l, "n", whole, 0).await?;
    let scratch = Scratch::new(given.scratch.clone());
    let dir = scratch.dir("beside-the-build");
    let s = store.as_ref();
    // The setting's door appends its record, then builds; the copy is
    // taken while the build runs, so the log holds the setting alone.
    let setter = {
        let l = &l;
        async move {
            l.set("sort_index.row", json!(["n"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let exporter = {
        let l = &l;
        let dir = dir.clone();
        async move {
            ensure!(
                until_appended(l, "sort_index.row").await?,
                "`sort_index.row` was never appended, so no build began"
            );
            crate::export::write(s, &dir)
                .await
                .must("exporting the lake beside the build")?;
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
    let after = answers(&i, size).await?;
    ensure!(
        after == before,
        "a copy taken beside the build imports to a lake that answers as the lake it came \
         from: {before:?} became {after:?}"
    );
    let got = page(&i, "n", whole, 0).await?;
    ensure!(
        got == ordered,
        "the imported lake's writer rebuilds the setting whole, so the page ordered by `n` \
         is the whole type in its order: {} keys, not {}",
        got.len(),
        ordered.len()
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
    let last = page(&i, "n", 1, size).await?;
    ensure!(
        last == ["late"],
        "a record written to the copy after its setting sorts last by `n`, in the index its \
         open built; the last page was {last:?}"
    );
    Ok(())
}
