//! A type's shape: what its schema hash covers, whether a revision
//! reshapes the records already stored, and how stored values convert
//! when a migration changes a field's type.
//!
//! Three questions, one module each:
//!
//! - [`schema_hash`] identifies a type's shape: the SHA-256 of the
//!   canonical bytes of the object `docs/log-format.md` §2.2 specifies.
//!   It changes when, and only when, how a record is stored or validated
//!   changes (`docs/decisions/007-schema-hash-covers-shape.md`).
//! - [`classify`] compares two declarations of one type and says whether
//!   the new one may simply be **declared** over the old one (nothing
//!   stored is reshaped) or needs a **migration**, and why. The store
//!   refuses `declare` for anything that needs a migration, so a stored
//!   record never silently stops matching its type.
//! - [`convert()`], [`preflight`] and [`migrate_record`] are the migration
//!   itself: the closed table of lossless conversions between storage
//!   kinds, the report a migration must pass before it is applied, and
//!   the per-record conversion the fold applies once it is.
//!
//! # Determinism
//!
//! Everything here is a pure function of its arguments: no clock, no
//! randomness, no I/O, no iteration over an unordered collection. The
//! schema hash is written into `declared` and `migrated` events, the
//! report id into `migrated`, and the fold converts every stored record
//! of a migrated type by [`migrate_record`]; a replay therefore
//! recomputes the same hashes, the same report and the same converted
//! values byte for byte, on every engine
//! (`docs/decisions/002-determinism-is-the-boundary.md`). A preflight
//! report does not even depend on the order the records are scanned in.

mod compat;
mod convert;

pub use compat::{Change, Reason, classify, reshapes};
pub use convert::{Conversion, convert, fits, migrate_record, preflight, referring};
pub use lakelands_api::shape::schema_hash;
