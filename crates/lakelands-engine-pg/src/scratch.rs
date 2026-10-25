//! The projection a rebuild builds beside the lake's own
//! (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
//!
//! The scratch is a schema of its own, named from the lake's
//! ([`Target::scratch_schema`]), holding the projection's tables and a
//! `meta` of its own; the log, its segments and the lease stay in the
//! lake's schema, where the scratch's handle reads them ([`Names::beside`]).
//! A handle building it holds a session lock keyed by the lake's schema, so
//! a second builder and a swap are refused while it is open, and the lock
//! goes with the session however the builder ends.
//!
//! The mark is the `built` row of the scratch's `meta`: the position and
//! the log's hash there. It is written in a transaction of its own, so the
//! scratch it names is whole through it; a scratch folded on since holds a
//! `folded` past it, and is no longer what the mark names.
//!
//! The swap is one transaction on the lake's schema: the projection's
//! tables are dropped and the scratch's moved into their place, the
//! projection's rows of `meta` replaced with the scratch's, and the
//! scratch's schema dropped. The log shares the lake's schema, so the
//! tables move, not the schema: every reader goes on naming the same one.

use lakelands_api::error::Result;
use lakelands_api::id::{LakeId, Position};

use crate::connect::{Db, Target};
use crate::log;
use crate::projection::{self, Names, PROJECTION_TABLES, RETIRED_TABLES};
use crate::vector_index;

/// The rows of `meta` the log keeps: everything else is the projection's.
const LOG_KEYS: &str = "'lake', 'imported', 'import_fence'";

/// Take the builder's lock on the lake whose schema is `live`: in the
/// session, for a builder, or in the open transaction, for a swap or a
/// sweep. `false` while another session holds it.
pub(crate) fn lock(db: &mut Db, live: &str, session: bool) -> Result<bool> {
    let f = if session {
        "pg_try_advisory_lock"
    } else {
        "pg_try_advisory_xact_lock"
    };
    let statement = format!("SELECT {f}(hashtextextended('lakelands rebuild ' || $1, 0))");
    Ok(db.query_one(&statement, &[&live])?.get(0))
}

/// Give the builder's session lock on the lake whose schema is `live` up,
/// as its handle is dropped: the server lets a session's locks go only
/// once it sees the connection close, which may be after a swap that
/// follows the drop has been refused `rebuilding`.
pub(crate) fn unlock(db: &mut Db, live: &str) -> Result<()> {
    let statement = "SELECT pg_advisory_unlock(hashtextextended('lakelands rebuild ' || $1, 0))";
    db.query_one(statement, &[&live]).map(|_| ())
}

/// The statement that removes the scratch `names` keeps.
pub(crate) fn remove_sql(names: &Names) -> String {
    format!("DROP SCHEMA IF EXISTS {} CASCADE", names.schema())
}

/// The scratch's mark, when it is fit to keep: marked for `lake`, the
/// mark's position holding its hash in the log, and, unless it is another
/// build's and `others` keeps those, laid out at this build's layout and
/// folded through the mark. `bare` is the scratch's schema, unquoted.
pub(crate) fn fit(
    db: &mut Db,
    names: &Names,
    bare: &str,
    lake: &LakeId,
    others: bool,
) -> Result<Option<Position>> {
    let meta = "SELECT 1 FROM pg_catalog.pg_tables WHERE schemaname = $1 AND tablename = 'meta'";
    if db.query_opt(meta, &[&bare])?.is_none() {
        return Ok(None);
    }
    let rows = db.query(
        &format!(
            "SELECT k, n, t FROM {} WHERE k IN ('lake', 'layout', 'folded', 'built')",
            names.t("meta")
        ),
        &[],
    )?;
    let row = |k: &str| rows.iter().find(|r| r.get::<_, String>(0) == k);
    let n = |k: &str| row(k).and_then(|r| r.get::<_, Option<i64>>(1));
    let t = |k: &str| row(k).and_then(|r| r.get::<_, Option<String>>(2));
    let (Some(built), Some(hash)) = (n("built"), t("built")) else {
        return Ok(None);
    };
    let at = Position(built as u64);
    let held = log::read(db, names, at, 1)?
        .pop()
        .filter(|e| e.position == at);
    if t("lake").as_deref() != Some(lake.as_str()) || held.is_none_or(|e| e.hash.0 != hash) {
        return Ok(None);
    }
    if n("layout").map(|v| v as u64) != Some(projection::LAYOUT) {
        return Ok(others.then_some(at));
    }
    Ok((n("folded") == Some(built)).then_some(at))
}

/// Remove the scratch an earlier builder left beside the lake `target`
/// keeps, unless it may still be swapped in: one another session builds is
/// left to it, and so is one marked by a build of another layout, whose
/// scratch this one does not read, while the log holds its mark.
pub(crate) fn sweep(db: &mut Db, target: &Target, live: &Names, lake: &LakeId) -> Result<()> {
    let bare = target.scratch_schema();
    let there = "SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = $1";
    if db.query_opt(there, &[&bare])?.is_none() {
        return Ok(());
    }
    let names = Names::beside(Target::quote(&bare), live);
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        if lock(db, &target.schema, false)? && fit(db, &names, &bare, lake, true)?.is_none() {
            db.batch(&remove_sql(&names))?;
        }
        Ok(())
    })
}

/// The statements, run in the swap's transaction, that put the scratch
/// `scratch` keeps in the place of the projection `live` keeps.
pub(crate) fn swap_sql(live: &Names, scratch: &Names) -> String {
    let mut sql = String::new();
    for t in PROJECTION_TABLES.iter().chain(RETIRED_TABLES) {
        sql += &format!("DROP TABLE IF EXISTS {} CASCADE; ", live.t(t));
    }
    sql += &vector_index::tables_sql(live, None);
    for t in PROJECTION_TABLES {
        sql += &format!(
            "ALTER TABLE {} SET SCHEMA {}; ",
            scratch.t(t),
            live.schema()
        );
    }
    sql += &vector_index::tables_sql(scratch, Some(live));
    let (m, sm) = (live.t("meta"), scratch.t("meta"));
    sql += &format!(
        "DELETE FROM {m} WHERE k NOT IN ({LOG_KEYS}); \
         INSERT INTO {m} (k, n, t) SELECT k, n, t FROM {sm} WHERE k NOT IN ({LOG_KEYS}, 'built'); \
         {};",
        remove_sql(scratch)
    );
    sql
}
