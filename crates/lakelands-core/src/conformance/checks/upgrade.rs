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
