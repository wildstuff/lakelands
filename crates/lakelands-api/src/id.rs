//! Names and coordinates: what identifies a lake, a type, a record, and
//! a point in a lake's history.
//!
//! Three words of the glossary live here and never vary: a [`Position`]
//! is the log coordinate every reply carries, a [`Version`] is one
//! record's compare-and-set token, and `valid_at` (in [`crate::time`])
//! is business time. See `docs/glossary.md`.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! name {
    ($($(#[$doc:meta])* $name:ident,)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Wraps a string; validation is the store's, on declaration.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// The name as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    )*};
}

name! {
    /// A lake: one world with one truth, one log and one writer. Nothing
    /// is ever read or written across two lakes.
    LakeId,
    /// A type declared in a lake's ontology. Names beginning with `sys.`
    /// are reserved for system types (`docs/decisions/003-reservation-by-namespace.md`).
    TypeName,
    /// A field of a type or of a relation.
    FieldName,
    /// A relation declared on a type.
    RelationName,
    /// A verb declared in a lake's ontology.
    VerbName,
    /// A value type declared in a lake's ontology over a storage kind.
    ValueTypeName,
    /// A record's key within its type.
    RecordKey,
    /// A source that feeds a lake through `ingest`. The source's own
    /// offset is the caller's business; the lake keeps only positions.
    SourceId,
    /// A proposal recorded by `WriteProposed`, accepted or rejected later.
    ProposalId,
    /// A caller-chosen key that makes a retried authored write a no-op.
    IdempotencyKey,
    /// A trigger declared in a lake's ontology.
    TriggerName,
    /// A constraint a type declares over its records.
    ConstraintName,
    /// A precondition a verb declares over its record and its call.
    PreconditionName,
    /// A parameter a view declares and a read binds.
    ParamName,
    /// A version of a source's data key, as the host's key store names it
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    KeyId,
    /// One delivery of a source: the caller's id for an `ingest`, or the
    /// store's own, `d<first position>` (`docs/log-format.md` §1.1).
    DeliveryId,
}

/// The log coordinate: the exact place in a lake's history an answer
/// reflects. Positions start at 0 (the lake's first event) and have no
/// gaps. Every reply carries one; a read may be pinned to one with
/// `as_of`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Position(pub u64);

impl Position {
    /// The position of a lake's first event.
    pub const GENESIS: Position = Position(0);

    /// The position after this one.
    pub fn next(self) -> Position {
        Position(self.0 + 1)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A record's compare-and-set token. It starts at 1 when a record is
/// first written and grows by one with every event that changes the
/// record. It orders one record's changes; it says nothing about the
/// lake as a whole (that is [`Position`]).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Version(pub u64);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An opaque page token. A cursor is bound to the position its first
/// page was read at, so a page set is stable across calls even while
/// the lake is written.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);
