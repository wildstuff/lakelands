//! Who a call is made as, and — for hosts only — the context a write
//! arrives in.
//!
//! The store knows principals and grants, never roles
//! (`docs/decisions/006-grants-not-roles.md`). A host's
//! [`PrincipalResolver`](crate::seam::PrincipalResolver) turns its own
//! identities into one [`Principal`] per call; the store enforces the
//! grants on every read and write.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::id::{FieldName, LakeId, Position, TypeName, VerbName};

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
    /// May the principal declare types, add fields and migrate; making
    /// such a change, or accepting one, needs `restricted` too.
    #[serde(default)]
    pub schema: bool,
    /// May the principal feed the lake through `ingest`.
    #[serde(default)]
    pub ingest: bool,
    /// May the principal see records withheld from ordinary reads, and,
    /// with `schema`, make or accept a change of the lake's types.
    #[serde(default)]
    pub restricted: bool,
}

/// An entry of the lake's list of renames, in the log's order, of one of
/// two kinds: one migration's renames of a type's fields, or a type's own
/// new name (a link type's, when its relation is renamed), with the
/// position it took effect at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Renamed {
    /// The position of the `migrated` event.
    pub at: Position,
    /// The type migrated; for a type's new name, the name it had.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// Each field's old and new name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<(FieldName, FieldName)>,
    /// The type's new name, in an entry of the second kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TypeName>,
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
    /// Only the records whose fields hold all these text values are this
    /// principal's to read, and it writes nothing that reaches the type or
    /// a shape; empty, every record. A function of [`Principal::id`]
    /// within a lake (`docs/decisions/051-a-grant-may-admit-only-some-records.md`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub only: BTreeMap<FieldName, crate::Value>,
    /// The verbs of this type declared `granted` this principal may call,
    /// its field rights permitting. A name no such verb bears grants
    /// nothing. It stays with the type's name it is given under: it
    /// permits, so a rename does not carry it
    /// (`docs/decisions/070-a-verb-may-be-kept-to-the-callers-granted-it.md`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub verbs: BTreeSet<VerbName>,
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
    /// every field grant reads, no type admits only some records, and
    /// `restricted` is held. Only such a
    /// principal may read the change feed through a door, since the feed
    /// is the log unmasked
    /// (`docs/decisions/037-the-feed-over-a-network-door.md`).
    pub fn reads_everything(&self) -> bool {
        self.restricted
            && self.default.reads()
            && self.types.values().all(|t| {
                t.default.is_none_or(Access::reads)
                    && t.fields.values().all(|a| a.reads())
                    && t.only.is_empty()
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
    /// do, or starts one. A field `only` names is followed the same way:
    /// its pair holds on the name borne at `at` by every chain that has
    /// borne it.
    pub fn follow(&mut self, list: &[Renamed], at: Option<Position>) {
        self.follow_names(list, at);
        let lake = self.default;
        let named =
            |t: &(&TypeName, &mut TypeGrants)| !t.1.fields.is_empty() || !t.1.only.is_empty();
        for (ty, held) in self.types.iter_mut().filter(named) {
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
            let borne_of = |c: &[(&FieldName, Position)]| {
                c.iter()
                    .rfind(|c| at.is_none_or(|p| c.1 <= p))
                    .map_or(c[0].0, |c| c.0)
                    .clone()
            };
            for (name, value) in std::mem::take(&mut held.only) {
                let borne = chains.iter().filter(|c| c.iter().any(|n| n.0 == &name));
                let borne: Vec<FieldName> = borne.map(|c| borne_of(c)).collect();
                for to in if borne.is_empty() { vec![name] } else { borne } {
                    unite(&mut held.only, to, &value);
                }
            }
            for chain in &chains {
                let borne = &borne_of(chain);
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

    /// The step over a type's names, before the one over its fields: the
    /// grants on every name a type has borne or will bear become one entry
    /// under the name it bears at `at`. A field's access there is the
    /// lowest of what that name's entry says (the field's grant, else its
    /// default, else the lake's) and of what each other name's entry says
    /// for it (its grant, else its default; an entry with neither says
    /// nothing). So a restriction follows the type in both directions of
    /// time and a permission stays with its name. A name's chain continues
    /// as a field's does: the newest of those that end in the old name.
    /// A name two chains bear at `at` is written once, by the chain that
    /// took it last; where both took it at one position, the lower of the
    /// two. Each chain reads the grants as they were given.
    fn follow_names(&mut self, list: &[Renamed], at: Option<Position>) {
        let lake = self.default;
        // The chains, and for each name the chain that took it last: a
        // name is free when a rename takes it.
        let mut chains: Vec<Vec<(&TypeName, Position)>> = Vec::new();
        let mut ends: BTreeMap<&TypeName, usize> = BTreeMap::new();
        for (r, to) in list.iter().filter_map(|r| Some((r, r.to.as_ref()?))) {
            let i = ends.remove(&r.ty).unwrap_or_else(|| {
                chains.push(vec![(&r.ty, Position::GENESIS)]);
                chains.len() - 1
            });
            chains[i].push((to, r.at));
            ends.insert(to, i);
        }
        let names = |c: &Vec<(&TypeName, Position)>| c.iter().any(|n| self.types.contains_key(n.0));
        if !chains.iter().any(names) {
            return;
        }
        let given = self.types.clone();
        let said = |t: &TypeGrants, f: &FieldName| t.fields.get(f).copied().or(t.default);
        // What `own` and `others` say together, the lower for each field.
        let lowest = |mut own: TypeGrants, others: &[&TypeGrants]| {
            let named = others.iter().copied().chain([&own]);
            let named: std::collections::BTreeSet<FieldName> =
                named.flat_map(|t| t.fields.keys()).cloned().collect();
            let field = |f: FieldName| {
                let mine = said(&own, &f).unwrap_or(lake);
                let low = (others.iter().filter_map(|t| said(t, &f))).fold(mine, Access::lowest);
                (f, low)
            };
            let fields = named.into_iter().map(field).collect();
            let mut defaults = others.iter().filter_map(|t| t.default).peekable();
            if defaults.peek().is_some() {
                let mine = own.default.unwrap_or(lake);
                own.default = Some(defaults.fold(mine, Access::lowest));
            }
            own.fields = fields;
            for (f, v) in others.iter().flat_map(|t| &t.only) {
                unite(&mut own.only, f.clone(), v);
            }
            own
        };
        // For each name borne at `at`: when it was taken, and the entry.
        let mut borne: BTreeMap<&TypeName, (Position, TypeGrants)> = BTreeMap::new();
        for chain in &chains {
            let held = chain.iter().rfind(|c| at.is_none_or(|p| c.1 <= p));
            let (name, since) = *held.unwrap_or(&chain[0]);
            let others = chain.iter().filter(|c| c.0 != name);
            let others: Vec<&TypeGrants> = others.filter_map(|c| given.get(c.0)).collect();
            let entry = lowest(given.get(name).cloned().unwrap_or_default(), &others);
            let entry = match borne.remove(name) {
                Some((taken, by)) if taken > since => (taken, by),
                Some((taken, by)) if taken == since => (since, lowest(entry, &[&by])),
                _ => (since, entry),
            };
            borne.insert(name, entry);
        }
        for (name, (_, entry)) in borne {
            if entry != TypeGrants::default() || given.contains_key(name) {
                self.types.insert(name.clone(), entry);
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

/// Adds a pair of `only`: two names that become one with differing values
/// hold null, which no grant may hold, so the type is refused to the
/// holder until the host's grants are fixed: never the wider of the two.
fn unite(only: &mut BTreeMap<FieldName, crate::Value>, f: FieldName, v: &crate::Value) {
    let held = only.entry(f).or_insert_with(|| v.clone());
    if held != v {
        *held = crate::Value::Null;
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
