//! The writer lease: a row holding the fencing token, under a session-level
//! advisory lock.
//!
//! The `lease` row holds the last fencing token handed out and who holds
//! it now. Taking the lease takes an advisory lock on the lease
//! connection without waiting, keyed by a 64-bit hash of the schema's
//! name, and writes a token one greater than the row's. When the lock is
//! held elsewhere the take fails at once, naming the holder the row
//! records. The server releases the lock when the session that holds it
//! ends, however it ends, so a crashed writer never keeps the lake from the
//! next one; the next one finds the row still naming the dead holder, takes
//! the lock and advances the token, and the dead holder's token is refused
//! from then on.
//!
//! The lock keeps two writers from holding the lease at once; the token is
//! what refuses a stale writer. Every append and every seal is checked
//! against the row (`docs/decisions/008-one-writer-per-lake.md`,
//! `docs/decisions/038-a-lake-in-postgresql.md`).
//!
//! The take reads the log's tip and its active segment in the transaction
//! that writes the token, after it: a writer under an older token that
//! was appending has committed by then or is refused, so what the new
//! writer carries into its appends is the log's end (`crate::log`).

use lakelands_api::engine::Lease;
use lakelands_api::error::{Error, Result, Situation};

use crate::connect::Db;
use crate::log::{self, Carried};
use crate::projection::Names;

/// The advisory lock's key for a schema, bound as `$1`: a 64-bit hash of
/// its name, computed by the server.
const KEY: &str = "hashtextextended('lakelands lease ' || $1, 0)";

/// Take the lease for `holder` on the lease connection, with what its
/// appends carry, or say who holds it.
pub(crate) fn take(
    db: &mut Db,
    names: &Names,
    schema: &str,
    holder: &str,
) -> Result<(Lease, Carried)> {
    let locked: bool = db
        .query_one(&format!("SELECT pg_try_advisory_lock({KEY})"), &[&schema])?
        .get(0);
    if !locked {
        let (fence, held_by) = db
            .query_opt(
                &format!(
                    "SELECT fence, holder FROM {} WHERE one = 1",
                    names.t("lease")
                ),
                &[],
            )?
            .map_or((0, String::new()), |r| {
                (r.get::<_, i64>(0) as u64, r.get(1))
            });
        let holder = (!held_by.is_empty()).then_some(held_by);
        return Err(Error::unavailable(Situation::LeaseLocked { holder, fence }));
    }
    let taken = db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        let row = db.query_one(
            &format!(
                "INSERT INTO {t} (one, fence, holder) VALUES (1, 1, $1) \
                 ON CONFLICT (one) DO UPDATE SET fence = greatest({t}.fence, \
                 (SELECT n FROM {m} WHERE k = 'import_fence')) + 1, holder = $1 \
                 RETURNING fence",
                t = names.t("lease"),
                m = names.t("meta")
            ),
            &[&holder],
        )?;
        let fence = row.get::<_, i64>(0) as u64;
        let carried = log::carried(db, names, fence)?;
        let holder = holder.to_owned();
        Ok((Lease { fence, holder }, carried))
    });
    if taken.is_err() {
        let _ = release_lock(db, schema);
    }
    taken
}

fn release_lock(db: &mut Db, schema: &str) -> Result<()> {
    db.query_one(&format!("SELECT pg_advisory_unlock({KEY})"), &[&schema])?;
    Ok(())
}

/// Give a held lease up: the row keeps its token and names no holder, and
/// the lock is released. The lock is released whatever happens; a failure
/// to clear the holder is reported after it.
pub(crate) fn release(db: &mut Db, names: &Names, schema: &str, lease: &Lease) -> Result<()> {
    let cleared = db.execute(
        &format!(
            "UPDATE {} SET holder = '' WHERE one = 1 AND fence = $1",
            names.t("lease")
        ),
        &[&(lease.fence as i64)],
    );
    let unlocked = release_lock(db, schema);
    cleared.and(unlocked)
}
