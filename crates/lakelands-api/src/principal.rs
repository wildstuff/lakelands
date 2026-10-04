//! Who a call is made as, and — for hosts only — the context a write
//! arrives in.
//!
//! The store knows principals and grants, never roles
//! (`docs/decisions/006-grants-not-roles.md`). A host's
//! [`PrincipalResolver`](crate::seam::PrincipalResolver) turns its own
//! identities into one [`Principal`] per call; the store enforces the
//! grants on every read and write.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{FieldName, LakeId, Position, TypeName};

/// What kind of party a principal is. It is recorded as the `writer` of
/// every value the principal writes; it grants nothing by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// A person.
    Human,
    /// An AI agent acting through a door.
    Agent,
    /// A program: an importer, a worker, a host service.
    Service,
    /// The store itself, for events its own fold decides (a trigger that
    /// fired, a dependent field it cleared).
    System,
}

/// The party a call is made as: one lake, one identity, one set of
/// grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    /// What kind of party this is.
    pub kind: PrincipalKind,
    /// A stable identifier within the host (`alice@example.com`,
    /// `importer:bank-feed`). Recorded as the writer of every value.
    pub id: String,
    /// The one lake this principal may address.
    pub lake: LakeId,
    /// What the principal may read and write.
    pub grants: Grants,
}

impl Principal {
    /// The writer string recorded in the log: `<kind>:<id>`.
    pub fn writer(&self) -> String {
        let kind = match self.kind {
            PrincipalKind::Human => "human",
            PrincipalKind::Agent => "agent",
            PrincipalKind::Service => "service",
            PrincipalKind::System => "system",
        };
        format!("{kind}:{}", self.id)
    }
}

/// What one principal may do with one field.
///
/// The variants are not a ladder: `WriteOnly` writes without reading, so
/// no variant is "more" than every other. Code decides with [`reads`],
/// [`writes`] and [`appears`], never by comparing variants; the derived
/// order exists only so that the type can key a collection.
///
/// [`reads`]: Access::reads
/// [`writes`]: Access::writes
/// [`appears`]: Access::appears
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// The field is absent from answers and from generated schemas.
    #[default]
    None,
    /// The field is present, its value replaced by a mask.
    Masked,
    /// The field is read in full.
    Read,
    /// The field is read in full and may be written.
    Write,
    /// The field may be written but not read: for reading it is as
    /// `None`, absent from every answer, and nothing a write answers
    /// shows its value (`docs/decisions/006-grants-not-roles.md`,
    /// extended).
    WriteOnly,
}

impl Access {
    /// Whether the value itself may be read.
    pub fn reads(self) -> bool {
        matches!(self, Access::Read | Access::Write)
    }

    /// Whether the field may be written.
    pub fn writes(self) -> bool {
        matches!(self, Access::Write | Access::WriteOnly)
    }

    /// Whether the field appears in a read answer: its value, or its name
    /// as masked.
    pub fn appears(self) -> bool {
        self.reads() || self == Access::Masked
    }

    /// The lowest of two accesses, reading and writing judged apart: it reads
    /// the value only if both do, shows the field masked if both at least show
    /// it, and writes only if both write.
    pub fn lowest(self, b: Access) -> Access {
        let (appears, writes) = (self.appears() && b.appears(), self.writes() && b.writes());
        match (self.reads() && b.reads(), appears, writes) {
            (true, _, true) => Access::Write,
            (true, _, false) => Access::Read,
            (false, true, _) => Access::Masked,
            (false, false, true) => Access::WriteOnly,
            (false, false, false) => Access::None,
        }
    }
}

/// The rights of one principal, per type and per field.
///
/// A right is looked up most specific first: the field's own grant, then
/// the type's default, then the lake-wide default. There is no role and
/// no switch that stops the checking; a fully trusted principal is one
/// whose `default` is [`Access::Write`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    /// The access for any field not granted more specifically.
    #[serde(default)]
    pub default: Access,
    /// Per type: a default for the type and grants per field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub types: BTreeMap<TypeName, TypeGrants>,
    /// May the principal call `accept` and `reject` on proposals.
    #[serde(default)]
    pub review: bool,
    /// May the principal declare types, add fields and migrate.
    #[serde(default)]
    pub schema: bool,
    /// May the principal feed the lake through `ingest`.
    #[serde(default)]
    pub ingest: bool,
    /// May the principal see records withheld from ordinary reads.
    #[serde(default)]
    pub restricted: bool,
}

/// One migration's renames of a type's fields, and the position it took
/// effect at: an entry of the lake's list of renames, in the log's order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Renamed {
    /// The position of the `migrated` event.
    pub at: Position,
    /// The type migrated.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// Each field's old and new name.
    pub renames: Vec<(FieldName, FieldName)>,
}

/// The grants of one principal on one type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeGrants {
    /// The access for any field of this type not granted per field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Access>,
    /// Access per field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<FieldName, Access>,
}

impl Grants {
    /// Everything, on every type: the grants of a lake's owner.
    pub fn owner() -> Grants {
        Grants {
            default: Access::Write,
            types: BTreeMap::new(),
            review: true,
            schema: true,
            ingest: true,
            restricted: true,
        }
    }

    /// Whether these grants read every field of every record, withheld
    /// records included: the lake default reads, every type's default and
    /// every field grant reads, and `restricted` is held. Only such a
    /// principal may read the change feed through a door, since the feed
    /// is the log unmasked
    /// (`docs/decisions/037-the-feed-over-a-network-door.md`).
    pub fn reads_everything(&self) -> bool {
        self.restricted
            && self.default.reads()
            && self.types.values().all(|t| {
                t.default.is_none_or(Access::reads) && t.fields.values().all(|a| a.reads())
            })
    }

    /// Holds a principal's grants to a field through its renames (006,
    /// extended): a field's access at `at` (the head when `None`) is the
    /// lowest of what the grants say under the name it bears there and under
    /// every other name it has borne or will bear that they name. A grant
    /// reaches further than its own name only to take away. The result is a
    /// grant by name on the name borne at `at`. A field's names are replayed
    /// from `list`, one migration's renames at once; a rename continues the
    /// chain of names that ends in its old name, the newest where several
    /// do, or starts one.
    pub fn follow(&mut self, list: &[Renamed], at: Option<Position>) {
        let lake = self.default;
        for (ty, held) in self.types.iter_mut().filter(|t| !t.1.fields.is_empty()) {
            let mut chains: Vec<Vec<(&FieldName, Position)>> = Vec::new();
            for r in list.iter().filter(|r| &r.ty == ty) {
                // Of the chains that end in the old name, the one whose last
                // name was taken latest: a name is free when a rename takes
                // it, so every earlier bearer of it is gone.
                let ends = |old| {
                    let named = chains
                        .iter()
                        .enumerate()
                        .filter(|c| c.1.last().is_some_and(|l| l.0 == old));
                    named.max_by_key(|c| c.1.last().map(|l| l.1)).map(|c| c.0)
                };
                let ends: Vec<Option<usize>> = r.renames.iter().map(|(old, _)| ends(old)).collect();
                for ((old, new), end) in r.renames.iter().zip(ends) {
                    match end {
                        Some(i) => chains[i].push((new, r.at)),
                        None => chains.push(vec![(old, Position::GENESIS), (new, r.at)]),
                    }
                }
            }
            let given = held.fields.clone();
            for chain in &chains {
                let borne = chain.iter().rfind(|c| at.is_none_or(|p| c.1 <= p));
                let borne = borne.map_or(chain[0].0, |c| c.0);
                let said = chain.iter().filter(|c| c.0 != borne);
                let mut said = said.filter_map(|c| given.get(c.0)).peekable();
                if said.peek().is_some() {
                    let own = held.fields.get(borne).copied();
                    let own = own.or(held.default).unwrap_or(lake);
                    (held.fields).insert(borne.clone(), said.fold(own, |a, b| a.lowest(*b)));
                }
            }
        }
    }

    /// The access this principal has to one field of one type.
    pub fn access(&self, ty: &TypeName, field: &FieldName) -> Access {
        match self.types.get(ty) {
            Some(t) => t
                .fields
                .get(field)
                .copied()
                .or(t.default)
                .unwrap_or(self.default),
            None => self.default,
        }
    }
}

/// Where a write comes from, as the host sees it. Set by the server or
/// the host, never by a caller: it is not part of any public door.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOrigin {
    /// A caller at a door: the CLI, MCP, REST, or a direct library call.
    #[default]
    Direct,
    /// A reaction to an earlier change: a worker acting on a trigger.
    Reactive,
    /// A procedure the host runs on a schedule.
    Scheduled,
}

/// The host's context for a write: its origin and how deep in a chain of
/// reactions it sits. The store refuses a reactive write deeper than the
/// lake's configured ceiling, so a trigger cannot feed itself forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WriteContext {
    /// Where the write comes from.
    pub origin: WriteOrigin,
    /// 0 for a direct write; one more than its cause for a reaction.
    pub depth: u32,
    /// The position the write follows from, as the host names it: the
    /// `trigger_fired` it reacts to, a record of the host's own run, any
    /// earlier event. It lands in the envelope's `cause` of each event the
    /// write itself appends, never in `context`; events the store derives
    /// from those keep the store's cause, which chains back to it. It must
    /// be at or below the head, and it does not count toward the depth
    /// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<crate::id::Position>,
}

#[cfg(test)]
mod tests;
