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

use crate::id::{FieldName, LakeId, TypeName};

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
}

impl Access {
    /// Whether the value itself may be read.
    pub fn reads(self) -> bool {
        matches!(self, Access::Read | Access::Write)
    }

    /// Whether the field may be written.
    pub fn writes(self) -> bool {
        self == Access::Write
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_grant_beats_the_type_default_which_beats_the_lake_default() {
        let invoice = TypeName::from("invoice");
        let mut g = Grants {
            default: Access::Read,
            ..Grants::default()
        };
        g.types.insert(
            invoice.clone(),
            TypeGrants {
                default: Some(Access::Masked),
                fields: [(FieldName::from("verdict"), Access::Write)].into(),
            },
        );
        assert_eq!(g.access(&invoice, &"verdict".into()), Access::Write);
        assert_eq!(g.access(&invoice, &"amount".into()), Access::Masked);
        assert_eq!(g.access(&"partner".into(), &"name".into()), Access::Read);
    }

    #[test]
    fn nothing_is_granted_by_default() {
        let g = Grants::default();
        assert_eq!(g.access(&"invoice".into(), &"amount".into()), Access::None);
        assert!(!g.review && !g.schema && !g.ingest && !g.restricted);
    }
}
