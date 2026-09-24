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
//! - [`convert`], [`preflight`] and [`migrate_record`] are the migration
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

use std::fmt;

use lakelands_api::{FieldName, TypeName};

use crate::expr::ParseError;

mod compat;
mod convert;
mod hash;

pub use compat::{Change, Reason, classify};
pub use convert::{Conversion, convert, fits, migrate_record, preflight};
pub use hash::schema_hash;

/// Why a shape question has no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// A condition inside a view does not parse, so its typed form (the
    /// form the schema hash covers) does not exist.
    Condition {
        /// Where the condition sits: `view.where` or
        /// `view.measures.<name>`.
        at: String,
        /// Why it does not parse.
        error: ParseError,
    },
    /// A migration names a different type than the one it migrates.
    OtherType {
        /// The type being migrated.
        from: TypeName,
        /// The type the migration declares.
        to: TypeName,
    },
    /// A rename's old name is not a field of the type before.
    RenameFromUnknown {
        /// The old name.
        from: FieldName,
    },
    /// A rename's new name is not a field of the type after.
    RenameToUnknown {
        /// The new name.
        to: FieldName,
    },
    /// A field is renamed from, or to, twice.
    RenameRepeated {
        /// The field named twice.
        field: FieldName,
    },
    /// A rename's new name is a field that already exists and keeps its
    /// own values, so two fields' values would land in one.
    RenameCollides {
        /// The new name.
        to: FieldName,
    },
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShapeError::Condition { at, error } => {
                write!(f, "the condition at `{at}` does not parse: {error}")
            }
            ShapeError::OtherType { from, to } => {
                write!(f, "a migration of `{from}` declares the type `{to}`")
            }
            ShapeError::RenameFromUnknown { from } => {
                write!(
                    f,
                    "rename from `{from}`: no such field before the migration"
                )
            }
            ShapeError::RenameToUnknown { to } => {
                write!(f, "rename to `{to}`: no such field after the migration")
            }
            ShapeError::RenameRepeated { field } => {
                write!(f, "the field `{field}` is renamed more than once")
            }
            ShapeError::RenameCollides { to } => write!(
                f,
                "rename to `{to}`: that field already exists and keeps its values"
            ),
        }
    }
}

impl std::error::Error for ShapeError {}
