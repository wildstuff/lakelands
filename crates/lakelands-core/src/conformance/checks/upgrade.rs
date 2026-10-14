//! A lake written in an older log format this release reads is upgraded,
//! never rewritten (`docs/log-format.md` §6,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//! The vectors of format 14 (`docs/log-format/vectors/14/`) are a small
//! lake's log as the release that wrote that format left it, so they are
//! imported here on every engine, as an operator imports an export.

use std::path::PathBuf;

use lakelands_api::event::{EventBody, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT};
use lakelands_api::read::ReadAt;
use lakelands_api::seam::Seams;

use crate::conformance::fixture::{Must, all, log_events, owner};
use crate::conformance::{Failure, Given, Held, ImportedLake, Store};

check!(
    FORMAT_14_UPGRADED = a_lake_of_format_14_is_upgraded_and_read,
    "054 and log-format §6 (lakelands#962): the vectors of format 14, a lake as the release that wrote that format left it, imported on this engine, keep every event as it was written, `v` 14 included, and are followed by a writer's `lease_taken` and one `lake_upgraded` from 14 to 15, and every later event in format 15; the lake then opens read-only, verifies whole, `describe` shows its two types and `find` answers its records; and a writer's open appends its own `lease_taken` and no second `lake_upgraded`",
    given
);

/// The vectors of a kept format.
fn vectors(format: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors")
        .join(format.to_string())
}

async fn a_lake_of_format_14_is_upgraded_and_read(store: Store, given: Given) -> Held {
    // The check's own lake answers before the import is asked of its
    // engine.
    store.head().await.must("reading the head")?;
    let lake = lakelands_api::LakeId::from("vectors");
    let imported = (given.import)(vectors(14), lake, None, Seams::default()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing format 14: {e}")));
    let ImportedLake {
        store: copy,
        writer,
    } = imported?;
    let log = log_events(copy.as_ref()).await?;
    let said: Vec<_> = (log.iter())
        .map(|e| format!("{} {} {}", e.position.0, e.v, e.body.kind()))
        .collect();
    let (old, new) = (OLDEST_LOG_FORMAT, LOG_FORMAT_VERSION);
    let as_written = log.iter().take(8).all(|e| e.v == old);
    let marked = matches!(log.get(9).map(|e| (&e.body, e.v)),
        Some((EventBody::LakeUpgraded { from, to }, v)) if *from == old && *to == new && v == new);
    let leased = matches!(log.get(8).map(|e| (&e.body, e.v)),
        Some((EventBody::LeaseTaken { .. }, v)) if v == new);
    let after = log.iter().skip(8).all(|e| e.v == new);
    ensure!(
        as_written && leased && marked && after,
        "the imported log holds the eight events of format 14 as written, then a writer's \
         `lease_taken` and a `lake_upgraded` from {old} to {new}, and every later event in \
         {new}; it holds {said:?}"
    );
    copy.verify(None, None)
        .await
        .must("verifying the upgraded lake")?;
    let own = owner(copy.as_ref());
    let cat = crate::conformance::fixture::doors(copy.as_ref());
    let described = cat.describe(&own, ReadAt::default()).await;
    let described = described.must("describing the upgraded lake")?.data;
    let typed = |n: &str| described.types.iter().any(|t| t.name.as_str() == n);
    let invoices = all(cat.as_ref(), &own, "invoice").await?.len();
    let entries = all(cat.as_ref(), &own, "ledger_entry").await?.len();
    ensure!(
        typed("invoice") && typed("ledger_entry") && invoices >= 1 && entries >= 1,
        "the upgraded lake shows `invoice` and `ledger_entry` and answers their records; \
         it answers {invoices} and {entries}"
    );
    let reopened = writer(copy, Seams::default()).await;
    let reopened = reopened.map_err(|e| Failure::new(format!("opening it for writing: {e}")))?;
    let after = log_events(reopened.as_ref()).await?;
    let markers = (after.iter())
        .filter(|e| matches!(e.body, EventBody::LakeUpgraded { .. }))
        .count();
    let leases = (after.iter().skip(log.len()))
        .filter(|e| matches!(e.body, EventBody::LeaseTaken { .. }))
        .count();
    ensure!(
        markers == 1 && leases == 1 && after.iter().skip(8).all(|e| e.v == new),
        "a writer's open of the upgraded lake appends its `lease_taken` and no second \
         `lake_upgraded`, in format {new}; there are {markers} marker(s) and {leases} new \
         lease(s)"
    );
    Ok(())
}

check!(
    LAKE_14_MIGRATED_READ = a_migration_of_format_14_folds_as_it_was_written,
    "054 and log-format §6 (lakelands#1070): the lake v0.13.1 made through its doors with a migration that changes only a derived expression (`docs/log-format/lakes/14-migrated/`), imported on this engine and upgraded, answers as that release answered it, each record's computed value and the set's members as v0.13.1 left them, and the same after a rebuild: a `migrated` of format 14 is folded as that format wrote it",
    given
);

check!(
    LAKE_14_READ = a_lake_the_release_before_made_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.13.1 made through its doors (`docs/log-format/lakes/14/`), in format 14, imported on this engine, is upgraded after its events by a writer's open and read as that release answered it, as `answers.json` keeps the answers: every live record of each of its types with its fields, the link records of its relation among them and the counts of its view; the members of its object set; and the walk over its relation with the link's fields; the upgraded lake verifies",
    given
);

check!(
    LAKE_15_READ = the_lake_of_format_15_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.14.0 made through its doors (`docs/log-format/lakes/15/`), in format 15, imported on this engine, keeps its events as written and is read as that release answered it, as `answers.json` keeps the answers: every live record of each of its types with its fields, the link records of its relation among them and the counts of its view; the members of its object set; and the walk over its relation with the link's fields; the lake verifies",
    given
);

async fn a_lake_the_release_before_made_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 14).await
}

async fn the_lake_of_format_15_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 15).await
}

/// The kept lake of `format` (`docs/log-format/lakes/<format>/`), imported
/// on this engine: its events stay in their format, a writer's open
/// upgrades an older one, the lake verifies, and it answers as the
/// release that made it did.
async fn kept_lake_answers(store: Store, given: Given, format: u32) -> Held {
    use crate::conformance::fixture::{find_where, pattern, step};
    use lakelands_api::query::{Projection, QueryResult, Walk};
    store.head().await.must("reading the head")?;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/lakes")
        .join(format.to_string());
    let said = std::fs::read(dir.join("answers.json"))
        .map_err(|e| Failure::new(format!("reading the answers of lake {format}: {e}")))?;
    let answers: serde_json::Value = serde_json::from_slice(&said)
        .map_err(|e| Failure::new(format!("the answers are no JSON: {e}")))?;
    let by = answers["made_by"]
        .as_str()
        .unwrap_or("its release")
        .to_owned();
    let lake = lakelands_api::LakeId::from(format!("kept{format}").as_str());
    let imported = (given.import)(dir, lake, None, Seams::default()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing the lake: {e}")))?;
    let copy = imported.store;
    let log = log_events(copy.as_ref()).await?;
    let made = answers["head"].as_u64().unwrap_or(0);
    let tail: Vec<_> = log
        .iter()
        .skip(made as usize + 1)
        .map(|e| e.body.kind())
        .collect();
    // An older lake is upgraded by a writer's open; one of this format is not.
    let upgraded = match format < LOG_FORMAT_VERSION {
        true => tail.starts_with(&["lease_taken", "lake_upgraded"]),
        false => !tail.contains(&"lake_upgraded"),
    };
    ensure!(
        log.iter().take(made as usize + 1).all(|e| e.v == format)
            && upgraded
            && log
                .iter()
                .skip(made as usize + 1)
                .all(|e| e.v == LOG_FORMAT_VERSION),
        "the lake's {} events stay in format {format}, a writer's open upgrades it \
         only from an older format, and every later event is in {LOG_FORMAT_VERSION}; \
         after them come {tail:?}",
        made + 1
    );
    copy.verify(None, None)
        .await
        .must("verifying the upgraded lake")?;
    let (own, cat) = (
        owner(copy.as_ref()),
        crate::conformance::fixture::doors(copy.as_ref()),
    );
    let mut records = Vec::new();
    for ty in [
        "partner",
        "invoice",
        "sys.link.invoice.approved_by",
        "by_state",
    ] {
        for r in all(cat.as_ref(), &own, ty).await? {
            records.push(serde_json::json!({"type": ty, "key": r.key, "fields": r.fields}));
        }
    }
    let members = cat
        .find(&own, &"invoice".into(), find_where("in_set('open')"))
        .await;
    let members = serde_json::to_value(members.must("finding the set's members")?.data)
        .map_err(|e| Failure::new(format!("the members as JSON: {e}")))?;
    let walk = pattern(
        "invoice",
        Some("i1"),
        vec![step("approved_by", Walk::Out)],
        Projection::Graph,
    );
    let walked: Vec<serde_json::Value> =
        match cat.query(&own, walk).await.must("walking from `i1`")?.data {
            QueryResult::Graph { edges, .. } => (edges.iter())
                .map(|e| serde_json::json!({"from": e.from.1, "to": e.to.1, "fields": e.fields}))
                .collect(),
            other => return Err(Failure::new(format!("a graph walk answered {other:?}"))),
        };
    let now = serde_json::json!({"records": records, "open": members, "walk_from_i1": walked});
    let mut red = Vec::new();
    for part in ["records", "open", "walk_from_i1"] {
        if now[part] != answers[part] {
            red.push(format!(
                "{part}: {by} answered {} and this release {}",
                answers[part], now[part]
            ));
        }
    }
    ensure!(
        red.is_empty(),
        "the lake {by} made answers as it did: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_migration_of_format_14_folds_as_it_was_written(store: Store, given: Given) -> Held {
    use crate::conformance::fixture::find_where;
    store.head().await.must("reading the head")?;
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/14-migrated");
    let said = std::fs::read(dir.join("answers.json"))
        .map_err(|e| Failure::new(format!("reading the answers v0.13.1 gave: {e}")))?;
    let answers: serde_json::Value = serde_json::from_slice(&said)
        .map_err(|e| Failure::new(format!("the answers are no JSON: {e}")))?;
    let lake = lakelands_api::LakeId::from("migrated14");
    let imported = (given.import)(dir, lake, None, Seams::default()).await;
    let ImportedLake { store, writer } =
        imported.map_err(|e| Failure::new(format!("importing the lake: {e}")))?;
    let copy = writer(store, Seams::default()).await;
    let copy = copy.map_err(|e| Failure::new(format!("opening it for writing: {e}")))?;
    let read = async || {
        let (own, cat) = (
            owner(copy.as_ref()),
            crate::conformance::fixture::doors(copy.as_ref()),
        );
        let mut bill = serde_json::Map::new();
        for r in all(cat.as_ref(), &own, "bill").await? {
            bill.insert(r.key.to_string(), serde_json::json!(r.fields));
        }
        let big = cat
            .find(&own, &"bill".into(), find_where("in_set('big')"))
            .await;
        let big = big.must("finding `big`'s members")?.data;
        let big: Vec<String> = match big {
            lakelands_api::read::Found::Records { rows, .. } => {
                rows.iter().map(|r| r.key.to_string()).collect()
            }
            other => return Err(Failure::new(format!("`find` answered {other:?}"))),
        };
        Ok::<_, Failure>(serde_json::json!({"bill": bill, "big": big}))
    };
    let live = read().await?;
    copy.rebuild().await.must("rebuilding the upgraded lake")?;
    let rebuilt = read().await?;
    ensure!(
        live == answers && rebuilt == answers,
        "the lake of format 14 with a migration of `twice` answers as v0.13.1 did ({answers}), \
         and again after a rebuild; it answered {live}, and {rebuilt} rebuilt"
    );
    Ok(())
}
