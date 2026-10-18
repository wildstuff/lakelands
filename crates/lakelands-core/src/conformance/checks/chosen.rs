//! The lake chooses an order index from what is ordered by
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`, lakelands#1131).
//!
//! What the engines promise of the build is the engine-seam check
//! `CHOSEN_ORDER`; these checks hold what a caller sees: when the lake
//! chooses, what changes in its answers (nothing), what it logs (nothing),
//! who is told, and who wins.

use std::time::Duration;

use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{Direction, Find, Found, OrderBy};
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Principal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, limits, order, pause, principal, source_record, sourced, type_decl,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    CHOSEN_ORDER_INDEX =
        a_lake_chooses_an_order_index_from_the_pages_it_had_to_sort_whole_and_logs_nothing,
    "041 and lakelands#1131: a type of 2 000 records whose `find` pages are ordered first by a number field no `sort_index.<type>` names, three times, makes the lake choose an order index for it beside its log: `describe_type` names the field in `chosen` (to a holder of `schema` who reads it) and no longer in `sorted_whole`, never in `sorted`; the pages answer what they answered before, the same on a deep page; and the log does not move, so a feed, a mirror and an export carry nothing of the choice, and a projection rebuilt from the log holds none",
    given
);

check!(
    CHOSEN_SWITCH = a_lake_with_the_switch_off_chooses_nothing_and_a_hosts_setting_wins,
    "041 and lakelands#1131: with `limits.choose_order_index` off, pages sorted whole at any number choose nothing; with it on, a host's `sort_index.<type>` setting naming a field keeps the lake from choosing for the type, takes away what it had chosen when written, and retiring the setting hands the type back so that the lake may choose again, and a lake opened with the switch off drops what an earlier writer chose",
    given
);

check!(
    CHOSEN_UNUSED = a_chosen_field_the_lake_has_not_ordered_by_for_the_window_is_dropped,
    "041 and lakelands#1131: a field the lake chose that no page has been ordered by for `limits.order_unused_secs` of the writer's life is dropped, and a page ordered by it in the window keeps it",
    given
);

check!(
    CHOSEN_BUILD_BESIDE_WRITES =
        a_write_during_the_chunked_build_is_answered_before_it_ends_and_is_in_the_index,
    "041 and lakelands#1131: the lake builds a chosen index in chunks of `limits.order_chunk` records, each a short write between appends, so a write sent while it builds (`sorted_whole` no longer names the field, `chosen` not yet) is answered before the build ends, and the record it wrote is in the index once built: a page ordered by the field, past the end of the type, ends with it",
    given
);

check!(
    CHOSEN_TOLD_TO_SCHEMA = a_chosen_field_is_told_only_to_a_holder_of_schema_who_reads_it,
    "041 and 024, lakelands#1131: `chosen` is told to a holder of `schema` who reads the field, as `sorted_whole` is, and to no other reader: a reader without `schema`, and one kept from the field, are told what a lake that chose nothing tells",
    given
);

/// The records of `row`: `n` a number, `s` text; `size` of them.
async fn rows(store: &dyn LakeStore, size: u64) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    rows_of(&l, "row", size).await?;
    Ok(l)
}

/// `size` records of the type `ty`, which is declared first.
async fn rows_of(l: &Lake, ty: &str, size: u64) -> Result<(), Failure> {
    let fields = vec![
        sourced("n", StorageKind::Number),
        sourced("s", StorageKind::Text),
    ];
    l.declare(type_decl(ty, fields)).await?;
    let records = (0..size)
        .map(|i| {
            let n = (i * 7919) % size;
            let s = format!("s{}", i % 17);
            source_record(ty, &format!("r{i:05}"), json!({"n": n, "s": s}))
        })
        .collect();
    l.ingest(ty, records).await.must("ingesting the rows")?;
    Ok(())
}

/// A page of 50 ordered first by `field`, from `offset`.
fn by(field: &str, offset: u32) -> Find {
    Find {
        order_by: vec![order(field, OrderBy::Value, Direction::Asc)],
        limit: Some(50),
        offset: Some(offset).filter(|o| *o > 0),
        ..Find::default()
    }
}

async fn page(l: &Lake, who: &Principal, offset: u32) -> Result<Vec<String>, Failure> {
    page_of(l, who, "row", "n", offset).await
}

async fn page_of(
    l: &Lake,
    who: &Principal,
    ty: &str,
    field: &str,
    offset: u32,
) -> Result<Vec<String>, Failure> {
    let found = l.cat.find(who, &ty.into(), by(field, offset)).await;
    match found.must(&format!("a page ordered by `{field}`"))?.data {
        Found::Records { rows, .. } => Ok(rows.iter().map(|r| r.key.to_string()).collect()),
        other => Err(Failure::new(format!(
            "a page of records answered {other:?}"
        ))),
    }
}

/// Three pages ordered by `n` (the lake weighs the third), with their
/// answers: the first page and a deep one.
async fn weigh(l: &Lake) -> Result<[Vec<String>; 2], Failure> {
    let first = page(l, &l.own, 0).await?;
    page(l, &l.own, 0).await?;
    page(l, &l.own, 0).await?;
    Ok([first, page(l, &l.own, 1500).await?])
}

/// The fields `describe_type` names for `who`: `(sorted, chosen, sorted_whole)`.
async fn told(
    l: &Lake,
    who: &Principal,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), Failure> {
    told_of(l, who, "row").await
}

async fn told_of(
    l: &Lake,
    who: &Principal,
    ty: &str,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), Failure> {
    let d = crate::conformance::fixture::describe_type(l.cat.as_ref(), who, ty).await?;
    let names = |fs: &[lakelands_api::FieldName]| fs.iter().map(|f| f.to_string()).collect();
    let lc = &d.lifecycle;
    let whole = lc
        .sorted_whole
        .iter()
        .map(|w| w.field.to_string())
        .collect();
    Ok((names(&lc.sorted), names(&lc.chosen), whole))
}

/// Wait for `want` to hold of the owner's `chosen`, up to a while.
async fn until_chosen(l: &Lake, want: bool) -> Result<bool, Failure> {
    for _ in 0..600 {
        if told(l, &l.own).await?.1.is_empty() != want {
            return Ok(true);
        }
        pause(Duration::from_millis(50)).await;
    }
    Ok(false)
}

async fn a_lake_chooses_an_order_index_from_the_pages_it_had_to_sort_whole_and_logs_nothing(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(2_000));
    let l = rows(store.as_ref(), size).await?;
    let head = store.head().await.must("reading the head")?;
    let before = weigh(&l).await?;
    ensure!(
        until_chosen(&l, true).await?,
        "three pages ordered by `n` over {size} records, no setting naming it, must make the \
         lake choose an order index for `n`; `describe_type` named no `chosen` field"
    );
    let (sorted, chosen, whole) = told(&l, &l.own).await?;
    ensure!(
        chosen == ["n"] && sorted.is_empty() && whole.is_empty(),
        "once chosen, `n` must be in `chosen` alone, not in `sorted` nor `sorted_whole`; \
         `describe_type` said sorted {sorted:?}, chosen {chosen:?}, sorted whole {whole:?}"
    );
    let after = [page(&l, &l.own, 0).await?, page(&l, &l.own, 1500).await?];
    ensure!(
        before == after,
        "a chosen index changes no answer: the pages ordered by `n` answered {before:?} before \
         and {after:?} after"
    );
    let now = store.head().await.must("reading the head")?;
    ensure!(
        now == head,
        "the lake's choice is kept beside the log: the head moved from {head} to {now}"
    );
    // A projection rebuilt from the log holds no choice: none is logged.
    store.rebuild().await.must("rebuilding the projection")?;
    let (_, chosen, _) = told(&l, &l.own).await?;
    ensure!(
        chosen.is_empty(),
        "a projection rebuilt from the log must hold no chosen field; it named {chosen:?}"
    );
    Ok(())
}

async fn a_lake_with_the_switch_off_chooses_nothing_and_a_hosts_setting_wins(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(2_000));
    rows(store.as_ref(), size).await?;
    // Off: five pages choose nothing.
    let mut off = limits();
    off.choose_order_index = false;
    let store = (given.relimit)(store, Seams::default(), off)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    for _ in 0..5 {
        page(&l, &l.own, 0).await?;
    }
    pause(Duration::from_millis(500)).await;
    ensure!(
        told(&l, &l.own).await?.1.is_empty(),
        "with `choose_order_index` off, pages sorted whole must choose nothing"
    );
    // On, with the host's setting naming `s`: the lake chooses nothing for the type.
    let store = (given.relimit)(store, Seams::default(), limits())
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    // Two pages are fewer than `order_after_pages` (3): nothing is chosen.
    for _ in 0..2 {
        page(&l, &l.own, 0).await?;
    }
    pause(Duration::from_millis(500)).await;
    ensure!(
        told(&l, &l.own).await?.1.is_empty(),
        "two pages sorted whole, fewer than `order_after_pages` (3), must choose nothing"
    );
    l.set("sort_index.row", json!(["s"])).await?;
    for _ in 0..5 {
        page(&l, &l.own, 0).await?;
    }
    pause(Duration::from_millis(500)).await;
    let (sorted, chosen, _) = told(&l, &l.own).await?;
    ensure!(
        chosen.is_empty() && sorted == ["s"],
        "a type whose `sort_index` names a field is the host's: the lake must choose nothing, \
         and `sorted` must be [\"s\"]; it said sorted {sorted:?}, chosen {chosen:?}"
    );
    // Retired, the type is handed back.
    l.retire("sys.setting", "sort_index.row").await?;
    for _ in 0..3 {
        page(&l, &l.own, 0).await?;
    }
    ensure!(
        until_chosen(&l, true).await?,
        "with the setting retired the lake may choose for the type again; it chose nothing"
    );
    // A type of fewer records than the bound is not weighed, and no more
    // fields than `order_max_fields` are chosen for one.
    rows_of(&l, "few", 100).await?;
    for _ in 0..5 {
        page_of(&l, &l.own, "few", "n", 0).await?;
    }
    pause(Duration::from_millis(500)).await;
    ensure!(
        told_of(&l, &l.own, "few").await?.1.is_empty(),
        "a type of 100 records, fewer than `order_min_records`, must be chosen no index"
    );
    let mut one = limits();
    one.order_max_fields = 1;
    let store = (given.relimit)(store, Seams::default(), one)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    for _ in 0..4 {
        page_of(&l, &l.own, "row", "s", 0).await?;
    }
    pause(Duration::from_millis(500)).await;
    let (_, chosen, _) = told(&l, &l.own).await?;
    ensure!(
        chosen == ["n"],
        "with `order_max_fields` 1 the lake must keep to the field it chose, `n`, and not \
         choose `s`; it said chosen {chosen:?}"
    );
    // Written again: the lake's choice goes, the host's stays.
    l.set("sort_index.row", json!(["s"])).await?;
    let (sorted, chosen, _) = told(&l, &l.own).await?;
    ensure!(
        chosen.is_empty() && sorted == ["s"],
        "a `sort_index` written over the lake's choice must take it away; it said sorted \
         {sorted:?}, chosen {chosen:?}"
    );
    // Chosen again, then the switch turned off: the choice an earlier
    // writer made is dropped, and only what the host sets is indexed.
    l.retire("sys.setting", "sort_index.row").await?;
    for _ in 0..3 {
        page(&l, &l.own, 0).await?;
    }
    ensure!(
        until_chosen(&l, true).await?,
        "with the setting retired again the lake chose nothing"
    );
    let store = (given.relimit)(store, Seams::default(), off)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    page(&l, &l.own, 0).await?;
    ensure!(
        until_chosen(&l, false).await?,
        "a lake opened with `choose_order_index` off must drop the index an earlier writer chose"
    );
    Ok(())
}

async fn a_write_during_the_chunked_build_is_answered_before_it_ends_and_is_in_the_index(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(2_000));
    rows(store.as_ref(), size).await?;
    // One record a chunk: the build is thousands of short writes.
    let mut chunked = limits();
    chunked.order_chunk = 1;
    let store = (given.relimit)(store, Seams::default(), chunked)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    weigh(&l).await?;
    // Building: `sorted_whole` no longer names `n`, and `chosen` not yet.
    let mut building = false;
    for _ in 0..600 {
        let (_, chosen, whole) = told(&l, &l.own).await?;
        ensure!(
            chosen.is_empty(),
            "the build of `n` in chunks of one record over {size} ended before a write was sent"
        );
        building = !whole.iter().any(|f| f == "n");
        if building {
            break;
        }
        pause(Duration::from_millis(20)).await;
    }
    ensure!(building, "three pages sorted whole started no build of `n`");
    // The build takes thousands of chunks: a record that sorts last,
    // written now, is answered while it goes on.
    let last = source_record("row", "r00001", json!({"n": size + 5, "s": "late"}));
    l.ingest("feed", vec![last])
        .await
        .must("a write during the build")?;
    ensure!(
        told(&l, &l.own).await?.1.is_empty(),
        "a write sent while the index is built must be answered before the build ends, not held \
         for it; the field was kept already"
    );
    ensure!(
        until_chosen(&l, true).await?,
        "the lake chose nothing after three pages sorted whole"
    );
    let tail = page(&l, &l.own, u32::try_from(size).unwrap_or(u32::MAX) - 1).await?;
    ensure!(
        tail == ["r00001"],
        "the record written during the build sorts last by `n`, and the index built beside the \
         writes must hold it; the last page was {tail:?}"
    );
    Ok(())
}

async fn a_chosen_field_the_lake_has_not_ordered_by_for_the_window_is_dropped(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(2_000));
    rows(store.as_ref(), size).await?;
    let mut unused = limits();
    unused.order_unused_secs = 2;
    let store = (given.relimit)(store, Seams::default(), unused)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    weigh(&l).await?;
    ensure!(
        until_chosen(&l, true).await?,
        "the lake chose nothing after three pages sorted whole"
    );
    // Pages ordered by `n` keep it in use past the window of unuse.
    for i in 0..16 {
        pause(Duration::from_millis(250)).await;
        page(&l, &l.own, 0).await?;
        ensure!(
            !told(&l, &l.own).await?.1.is_empty(),
            "a chosen field ordered by every quarter of a second is in use, and must be kept \
             past an `order_unused_secs` of 2; it was dropped after {i} pages"
        );
    }
    // Pages by another field leave `n` unused, and the lake drops it.
    for _ in 0..40 {
        pause(Duration::from_millis(250)).await;
        l.cat
            .find(&l.own, &"row".into(), Find::default())
            .await
            .must("a page in key order")?;
        if told(&l, &l.own).await?.1.is_empty() {
            return Ok(());
        }
    }
    Err(Failure::new(
        "a chosen field no page was ordered by for `order_unused_secs` must be dropped; it was kept",
    ))
}

async fn a_chosen_field_is_told_only_to_a_holder_of_schema_who_reads_it(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.max(2_000));
    let l = rows(store.as_ref(), size).await?;
    weigh(&l).await?;
    ensure!(
        until_chosen(&l, true).await?,
        "the lake chose nothing after three pages sorted whole"
    );
    let grants = |schema: bool, kept: &[&str]| {
        let mut g = lakelands_api::Grants {
            default: lakelands_api::Access::Read,
            schema,
            ..Default::default()
        };
        let fields = kept.iter().map(|f| {
            (
                lakelands_api::FieldName::from(*f),
                lakelands_api::Access::None,
            )
        });
        g.types.insert(
            "row".into(),
            lakelands_api::principal::TypeGrants {
                default: None,
                fields: fields.collect(),
                ..Default::default()
            },
        );
        g
    };
    let kind = lakelands_api::principal::PrincipalKind::Agent;
    let who = |id: &str, g| principal(store.as_ref(), kind, id, g);
    let (plain, kept_from) = (
        who("plain", grants(false, &[])),
        who("kept", grants(true, &["n"])),
    );
    let schema = who("schema", grants(true, &[]));
    let got = (
        told(&l, &schema).await?.1,
        told(&l, &plain).await?.1,
        told(&l, &kept_from).await?.1,
    );
    ensure!(
        got.0 == ["n"] && got.1.is_empty() && got.2.is_empty(),
        "`chosen` is told to a holder of `schema` who reads `n` and to no one else; a holder of \
         `schema` was told {:?}, one without {:?}, one kept from `n` {:?}",
        got.0,
        got.1,
        got.2
    );
    Ok(())
}
