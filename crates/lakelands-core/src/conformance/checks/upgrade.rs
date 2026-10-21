//! A lake written in an older log format this release reads is upgraded,
//! never rewritten (`docs/log-format.md` §6,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//! The vectors of each older format (`docs/log-format/vectors/<N>/`) are a
//! small lake's log as the release that wrote that format left it, so they
//! are imported here on every engine, as an operator imports an export.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Seam, Situation};
use lakelands_api::event::{EventBody, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT};
use lakelands_api::ontology::StorageKind::Number;
use lakelands_api::read::ReadAt;
use lakelands_api::seam::Seams;
use lakelands_api::{Error, Location, RecordKey, TypeName};
use serde_json::json;

use crate::conformance::fixture::{Lake, Must, all, field, log_events, owner, type_decl};
use crate::conformance::{Failure, Given, Held, ImportedLake, Store};

check!(
    FORMAT_14_UPGRADED = a_lake_of_format_14_is_upgraded_and_read,
    "054 and log-format §6 (lakelands#962): the vectors of format 14, a lake as the release that wrote that format left it, imported on this engine, keep every event as it was written, `v` 14 included, and are followed by a writer's `lease_taken` and one `lake_upgraded` from 14 to the format this release writes, and every later event in that format; the lake then opens read-only, verifies whole, `describe` shows its two types and `find` answers its records; and a writer's open appends its own `lease_taken` and no second `lake_upgraded`",
    given
);

check!(
    FORMAT_16_UPGRADED = a_lake_of_format_16_is_upgraded_and_read,
    "054 and log-format §6 (lakelands#1164): the vectors of format 16, as v0.15.0 shipped them, imported on this engine, keep every event as it was written, `v` 16 included, and are followed by a writer's `lease_taken` and one `lake_upgraded` from 16 to the format this release writes, and every later event in that format; the lake then opens read-only, verifies whole, `describe` shows its two types and `find` answers its records; and a writer's open appends its own `lease_taken` and no second `lake_upgraded`",
    given
);

check!(
    FORMAT_17_UPGRADED = a_lake_of_format_17_is_upgraded_and_read,
    "054 and log-format §6 (lakelands#1214): the vectors of format 17, as v0.16.0 shipped them, imported on this engine, keep every event as it was written, `v` 17 included, and are followed by a writer's `lease_taken` and one `lake_upgraded` from 17 to the format this release writes, and every later event in that format; the lake then opens read-only, verifies whole, `describe` shows its two types and `find` answers its records; and a writer's open appends its own `lease_taken` and no second `lake_upgraded`",
    given
);

check!(
    FORMAT_15_UPGRADED = a_lake_of_format_15_is_upgraded_and_read,
    "054 and log-format §6 (lakelands#1096): the vectors of format 15, as v0.14.0 shipped them, imported on this engine, keep every event as it was written, `v` 15 included, and are followed by a writer's `lease_taken` and one `lake_upgraded` from 15 to the format this release writes, and every later event in that format; the lake then opens read-only, verifies whole, `describe` shows its two types and `find` answers its records; and a writer's open appends its own `lease_taken` and no second `lake_upgraded`",
    given
);

/// The vectors of a kept format.
fn vectors(format: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors")
        .join(format.to_string())
}

async fn a_lake_of_format_14_is_upgraded_and_read(store: Store, given: Given) -> Held {
    vectors_upgraded(store, given, OLDEST_LOG_FORMAT).await
}

async fn a_lake_of_format_15_is_upgraded_and_read(store: Store, given: Given) -> Held {
    vectors_upgraded(store, given, 15).await
}

async fn a_lake_of_format_16_is_upgraded_and_read(store: Store, given: Given) -> Held {
    vectors_upgraded(store, given, 16).await
}

async fn a_lake_of_format_17_is_upgraded_and_read(store: Store, given: Given) -> Held {
    vectors_upgraded(store, given, 17).await
}

/// The vectors of the older format `old`, imported, upgraded and read.
async fn vectors_upgraded(store: Store, given: Given, old: u32) -> Held {
    // The check's own lake answers before the import is asked of its
    // engine.
    store.head().await.must("reading the head")?;
    let lake = lakelands_api::LakeId::from("vectors");
    let imported = (given.import)(vectors(old), lake, None, Seams::default()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing format {old}: {e}")));
    let ImportedLake {
        store: copy,
        writer,
    } = imported?;
    let log = log_events(copy.as_ref()).await?;
    let said: Vec<_> = (log.iter())
        .map(|e| format!("{} {} {}", e.position.0, e.v, e.body.kind()))
        .collect();
    let new = LOG_FORMAT_VERSION;
    let as_written = log.iter().take(8).all(|e| e.v == old);
    let marked = matches!(log.get(9).map(|e| (&e.body, e.v)),
        Some((EventBody::LakeUpgraded { from, to }, v)) if *from == old && *to == new && v == new);
    let leased = matches!(log.get(8).map(|e| (&e.body, e.v)),
        Some((EventBody::LeaseTaken { .. }, v)) if v == new);
    let after = log.iter().skip(8).all(|e| e.v == new);
    ensure!(
        as_written && leased && marked && after,
        "the imported log holds the eight events of format {old} as written, then a writer's \
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
    "054 (lakelands#962): the lake v0.13.1 made through its doors (`docs/log-format/lakes/14/`), in format 14, imported on this engine, is upgraded after its events by a writer's open and read as that release answered it, as `answers.json` keeps the answers: every live record of each of its types with its fields, the link records of its relation among them and the counts of its view; the members of its object set; and the walk over its relation with the link's fields; the upgraded lake verifies; and its `sys.setting`, declared again at the writer's open, holds `group` (063, extended), a store's setting written there stamped `store`",
    given
);

check!(
    LAKE_15_READ = the_lake_of_format_15_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.14.0 made through its doors (`docs/log-format/lakes/15/`), in format 15, imported on this engine, is upgraded after its events by a writer's open and read as that release answered it, as `answers.json` keeps the answers: every live record of each of its types with its fields, the link records of its relation among them and the counts of its view; the members of its object set; and the walk over its relation with the link's fields; the lake verifies; and its `sys.setting`, declared again at the writer's open, holds `group` (063, extended), a store's setting written there stamped `store`",
    given
);

check!(
    LAKE_16_READ = the_lake_of_format_16_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.15.0 made through its doors (`docs/log-format/lakes/16/`), in format 16, imported on this engine, is upgraded after its events by a writer's open and read as that release answered it, as `answers.json` keeps the answers: every live record of each of its types with its fields, the link records of its relation among them and the counts of its view; the members of its object set; the walk over its relation with the link's fields; and, what format 16 adds (058), its sealed values read whole with the file keyring's file it ships and a forgotten source's values read as erased, each with its provenance; imported with no key store bound, it is refused as unbound (the key store) before anything is written; the lake verifies; and its `sys.setting`, declared again at the writer's open, holds `group` (063, extended), a store's setting written there stamped `store`",
    given
);

check!(
    LAKE_17_READ = the_lake_of_format_17_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.16.0 made through its doors (`docs/log-format/lakes/17/`), in format 17, imported on this engine, is upgraded after its events by a writer's open and read as that release answered it, as `answers.json` keeps the answers: every live record of each type it names with its fields, the link records of its relation, the counts of its view, a field only a verb writes, a host's setting with its group, a record a verb retired and two slips, one of them a reaction past its trigger's depth, among them; the members of its object set; the walk over its relation with the link's fields; and its sealed values read whole with the file keyring's file it ships, and a forgotten source's read as erased, each with its provenance, while the values a delivery taken back and a cut that destroyed a key version took are gone with the records only they wrote (065); imported with no key store bound, it is refused as unbound (the key store) before anything is written; the lake verifies",
    given
);

check!(
    LAKE_18_READ = the_lake_of_format_18_is_read_as_it_answered,
    "054 (lakelands#962): the lake v0.17.0 made through its doors (`docs/log-format/lakes/18/`), in format 18, imported on this engine, is read as that release answered it, as `answers.json` keeps the answers: every live record of each type it names with its fields, an item a source the field admits wrote, and a person of a delivery sealed apart among them; the members of its object set; the walk over its relation; and its sealed values read whole with the file keyring's file it ships, the apart delivery's through its composed key, each with its provenance, while the values a delivery, several deliveries, a delivery sealed apart and a cut by the time a source saw them took are gone with the records only they wrote (065); imported with no key store bound, it is refused as unbound (the key store) before anything is written; the lake verifies",
    given
);

async fn a_lake_the_release_before_made_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 14).await
}

async fn the_lake_of_format_15_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 15).await
}

async fn the_lake_of_format_16_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 16).await
}

async fn the_lake_of_format_17_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 17).await
}

async fn the_lake_of_format_18_is_read_as_it_answered(store: Store, given: Given) -> Held {
    kept_lake_answers(store, given, 18).await
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
    // A lake whose values are sealed ships the file keyring's file that
    // sealed them (058), and is imported with it; with no key store bound
    // the import fails closed, before it writes.
    let ring = dir.join("keys").join(format!("kept{format}.keys.json"));
    let seams = match ring.exists() {
        true => {
            let bare = (given.import)(dir.clone(), lake.clone(), None, Seams::default()).await;
            let bare = bare.err();
            ensure!(
                matches!(&bare, Some(Error::Unbound { seam }) if **seam == Seam::KeyStore),
                "lake {format} imported with no key store must be refused as unbound, \
                 the key store; it answered {bare:?}"
            );
            let keys = super::erasable::Keys::from_file(&ring, &lake)?;
            super::erasable::with(&std::sync::Arc::new(keys))
        }
        false => Seams::default(),
    };
    let imported = (given.import)(dir, lake, None, seams.clone()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing the lake: {e}")))?;
    let (copy, writer) = (imported.store, imported.writer);
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
    let verifying = match format < LOG_FORMAT_VERSION {
        true => "verifying the upgraded lake",
        false => "verifying the lake",
    };
    copy.verify(None, None).await.must(verifying)?;
    let (own, cat) = (
        owner(copy.as_ref()),
        crate::conformance::fixture::doors(copy.as_ref()),
    );
    // The types whose records the answers keep: those they name, else
    // the four every kept lake holds.
    let named: Option<Vec<&str>> = answers["types"]
        .as_array()
        .map(|t| t.iter().filter_map(serde_json::Value::as_str).collect());
    let types = named.unwrap_or_else(|| {
        vec![
            "partner",
            "invoice",
            "sys.link.invoice.approved_by",
            "by_state",
        ]
    });
    let mut records = Vec::new();
    for ty in types {
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
    // The records whose provenance the answers keep, an erased value's
    // among them.
    let mut erasable = Vec::new();
    for said in answers["erasable"].as_array().into_iter().flatten() {
        let (ty, key) = (
            said["type"].as_str().unwrap_or(""),
            said["key"].as_str().unwrap_or(""),
        );
        let get = crate::conformance::fixture::get_at(key, None);
        let r = cat
            .get(&own, &ty.into(), get)
            .await
            .must(&format!("reading `{key}`"))?
            .data;
        erasable
            .push(serde_json::json!({"type": ty, "key": key, "fields": r.fields, "meta": r.meta}));
    }
    let now = serde_json::json!({"records": records, "open": members, "walk_from_i1": walked, "erasable": erasable});
    let mut red = Vec::new();
    let parts = ["records", "open", "walk_from_i1", "erasable"];
    for part in parts.into_iter().filter(|p| answers.get(*p).is_some()) {
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
    settings_grouped(copy, writer, seams, format).await
}

/// The lake of `format`, older than this release's, holds `sys.setting`
/// with the `group` its writer's open declared again (063, extended), and
/// a store's setting written to it is stamped `store`.
async fn settings_grouped(
    copy: Store,
    writer: crate::conformance::Reopen,
    seams: Seams,
    format: u32,
) -> Held {
    if format >= LOG_FORMAT_VERSION {
        return Ok(());
    }
    let ty = lakelands_api::seam::SETTING_TYPE;
    let own = owner(copy.as_ref());
    let cat = crate::conformance::fixture::doors(copy.as_ref());
    let described = cat.describe(&own, ReadAt::default()).await;
    let described = described.must("describing the upgraded lake")?.data;
    let fields: Vec<_> = (described.types.iter())
        .filter(|t| t.name.as_str() == ty)
        .flat_map(|t| t.fields.iter().map(|f| f.name.to_string()))
        .collect();
    let reopened = writer(copy, seams).await;
    let reopened = reopened.map_err(|e| Failure::new(format!("opening it for writing: {e}")))?;
    let l = crate::conformance::fixture::Lake::of(reopened.as_ref());
    l.set("view_kept.by_state", serde_json::json!(true)).await?;
    let group = l
        .read(ty, "view_kept.by_state")
        .await?
        .fields
        .get("group")
        .cloned();
    ensure!(
        fields == ["key", "value", "group"] && group == Some(serde_json::json!("store")),
        "a writer's open of the lake of format {format} declares `{ty}` again with `group` \
         ({fields:?}), and a store's setting written to it is stamped `store` ({group:?})"
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

check!(
    LAYOUT_REBUILT = a_writers_open_rebuilds_a_projection_an_older_build_laid_out,
    "004, extended (lakelands#1151): a writer's open of a projection an older build laid out rebuilds it from the log under its lease, saying why, and reads what the log holds and nothing else the projection held; the next writer's open rebuilds nothing; a writer's open of one a newer build laid out, or one with no mark, is refused as `projection_layout` and rebuilds nothing; a reader's open of any of them is refused by the engine before the store reads anything (`OTHER_LAYOUT`, through the engine seam)",
    given
);

/// The layout refusal the next writer's open meets, if any: `Some` of
/// the layout found, beside a layout of 2 the engine wants.
type Laid = Arc<Mutex<Option<Option<u64>>>>;

/// The harness's engine opener, whose next writer's open meets the
/// projection [`Laid`] says, and which counts the opens for a rebuild.
/// Each rebuild's engine is handed out holding a record `ghost` that no
/// event wrote, so a rebuild that refolds from the start drops it and a
/// reopen keeps it.
struct LaidOut {
    inner: Arc<dyn EngineOpener>,
    laid: Laid,
    rebuilds: Arc<AtomicU32>,
}

impl EngineOpener for LaidOut {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(
        &self,
        at: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        let laid = crate::lock(&self.laid).take_if(|_| mode != OpenMode::Rebuild);
        if let Some(found) = laid {
            crate::engines::layout(&at.lake, found, 2)?;
        }
        let engine = self.inner.open(at, mode)?;
        if mode == OpenMode::Rebuild {
            self.rebuilds.fetch_add(1, Ordering::SeqCst);
            let (ty, key) = (TypeName::from("t"), RecordKey::from("a"));
            let held = engine.view(None)?.get(&ty, &key)?;
            let at = engine.projection().folded_through()?;
            let (Some(mut ghost), Some(at)) = (held, at) else {
                panic!(
                    "the harness puts its `ghost` beside `t/a`, which this rebuild's engine holds"
                )
            };
            ghost.key = "ghost".into();
            engine.projection().begin()?;
            engine.projection().put_record(&ghost)?;
            engine.projection().commit(at)?;
        }
        Ok(engine)
    }
}

async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out(
    store: Store,
    given: Given,
) -> Held {
    let (laid, rebuilds): (Laid, _) = (Arc::default(), Arc::new(AtomicU32::new(0)));
    let wrap: crate::conformance::Wrap = {
        let (laid, rebuilds) = (laid.clone(), rebuilds.clone());
        Arc::new(move |inner| {
            let (laid, rebuilds) = (laid.clone(), rebuilds.clone());
            Arc::new(LaidOut {
                inner,
                laid,
                rebuilds,
            }) as Arc<dyn EngineOpener>
        })
    };
    let open = |store: Store, found: Option<Option<u64>>| {
        *crate::lock(&laid) = found;
        (given.fault)(store, Seams::default(), wrap.clone())
    };
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("t", vec![field("n", Number)])).await?;
    l.upsert("t", "a", json!({"n": 1})).await?;
    let store = (open(store, Some(Some(1))).await)
        .map_err(|e| Failure::new(format!("the writer's open of layout 1 was refused: {e}")))?;
    let (why, n) = (store.rebuilt_at_open(), rebuilds.load(Ordering::SeqCst));
    let a = Lake::of(store.as_ref()).read("t", "a").await?;
    ensure!(
        n == 1 && why.as_ref().is_some_and(|w| w.contains("layout 1")) && a.fields["n"] == 1,
        "a writer's open of layout 1 rebuilds once, saying why, and reads `a`: {n} rebuilds, \
         {why:?}, {a:?}"
    );
    let (l, at) = (
        Lake::of(store.as_ref()),
        crate::conformance::fixture::get_at("ghost", None),
    );
    let ghost = (l.cat.get(&l.own, &TypeName::from("t"), at).await).map(|r| r.data.fields);
    ensure!(
        matches!(ghost, Err(Error::NotFound { .. })),
        "the rebuild refolds the projection from the start, so `ghost`, which no event wrote, \
         is gone; it answered {ghost:?}"
    );
    let store = (open(store, None).await)
        .map_err(|e| Failure::new(format!("the next writer's open was refused: {e}")))?;
    let (why, n) = (store.rebuilt_at_open(), rebuilds.load(Ordering::SeqCst));
    ensure!(
        n == 1 && why.is_none(),
        "the next writer's open rebuilds nothing: {n} rebuilds, {why:?}"
    );
    for (found, what) in [(Some(3), "newer"), (None, "unmarked")] {
        let twin = (given.twin)(format!("layout-{what}").into(), Seams::default()).await;
        let twin = twin.map_err(|e| Failure::new(format!("opening the {what} twin: {e}")))?;
        let refused = open(twin, Some(found)).await.err();
        let layout = refused.as_ref().and_then(Error::situation);
        ensure!(
            matches!(layout, Some(Situation::ProjectionLayout { .. }))
                && rebuilds.load(Ordering::SeqCst) == 1,
            "a writer's open of a projection {what} is refused and rebuilds nothing: {refused:?}"
        );
    }
    Ok(())
}
