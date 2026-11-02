//! A setting's index is built in parts beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`, R7).
//!
//! `sort_index.<type>` and `lookup_index.<type>` no longer walk the type
//! in the turn that appends them: the fold marks what the setting asks
//! for, and the build fills it a key range at a time. These checks hold
//! what a caller sees: every read answers as before until the build is
//! whole, a write sent beside it goes on and reaches both sides, a record
//! retired beside it leaves the index, and a projection rebuilt from the
//! log holds the setting whole at its event.

use lakelands_api::read::{Find, Found};
use lakelands_api::seam::SETTING_TYPE;

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
    BUILD_BESIDE_A_REMOVAL = a_record_retired_beside_the_build_is_not_in_the_index,
    "099, R7: a record retired beside the build is not in the index once it is whole, and every other record is: the build's rows follow the fold, since retirements are events",
    given
);

check!(
    BUILD_REBUILD_SAME_LAKE = a_rebuild_holds_the_setting_whole_at_its_event,
    "099, R7: a setting is in the log and its mark in the projection, so a projection rebuilt from the log holds it whole at its event, with no mark; every read answers what the built lake answered",
    given
);

/// `size` records of `row`: `n` a number and `s` a text.
async fn rows(store: &dyn LakeStore, size: u64) -> Result<Lake, Failure> {
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
async fn chunked(given: &Given, store: Store) -> Result<Store, Failure> {
    let limits = Limits {
        order_chunk: 1,
        ..limits()
    };
    (given.relimit)(store, Seams::default(), limits)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))
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

async fn a_setting_that_builds_in_parts_changes_no_answer_either_kind(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(200).min(400));
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
    let size = u64::from(given.large_lake.max(200).min(400));
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
    let size = u64::from(given.large_lake.max(200).min(400));
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
    let size = u64::from(given.large_lake.max(200).min(400));
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
    let size = u64::from(given.large_lake.max(200).min(400));
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
    let size = u64::from(given.large_lake.max(200).min(400));
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
