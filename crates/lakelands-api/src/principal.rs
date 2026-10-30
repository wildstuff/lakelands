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
    /// Only the records whose field holds one of its text values, for each
    /// field named here, are this principal's to read, beside every pair of
    /// `only`, as `only` admits them: a value not listed is not admitted,
    /// and an empty set admits none. A field named here and in `only` is a
    /// grant that cannot be applied (`006 § A grant may admit a set of
    /// values, and refuse a part read as the whole`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub only_any: BTreeMap<FieldName, BTreeSet<String>>,
    /// Only the records whose fields hold all these text values are this
    /// principal's to write, each pair valid as one of `only` is; it reads
    /// what its other grants give; empty, every record it may read
    /// (`006 § A grant may admit every record to read and only some to write`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub writes_only: BTreeMap<FieldName, crate::Value>,
    /// The verbs of this type declared `granted` this principal may call,
    /// its field rights permitting. A name no such verb bears grants
    /// nothing. It stays with the type's name it is given under: it
    /// permits, so a rename does not carry it
    /// (`docs/decisions/070-a-verb-may-be-kept-to-the-callers-granted-it.md`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub verbs: BTreeSet<VerbName>,
    /// Fields lowered on only the records a condition selects; on every
    /// other record the access above stands
    /// (`006 § A field may be hidden on only the records a condition selects`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hide_where: Vec<HiddenWhere>,
    /// A read at a valid time dates this type's records by their stored
    /// `valid_from` and `valid_to` also where this principal holds them at
    /// `none` or `masked`; the fields stay withheld from every answer as
    /// their access says. Answers across dates narrow a hidden date down,
    /// and this grant consents to that. Without it, such a read probes the
    /// bounds as a filter would. Held through a type's renames, it holds
    /// only where the grant under every name the type has borne says it
    /// (`docs/decisions/092-reading-is-judged-apart-from-writing.md`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dates_by_hidden_valid_time: bool,
    /// Where `only` or `only_any` narrows this type, a read that computes
    /// over a set of its records (a view over it, an aggregate, a count)
    /// is refused rather than answered from the admitted part; a read of
    /// the admitted records themselves is answered. Nothing where the type
    /// is not narrowed (`006 § A grant may admit a set of values, and refuse
    /// a part read as the whole`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole_sets_only: bool,
}

/// A hiding: on a record whose `field` holds one of `values`, compared as
/// a pair of `only` is, each of `fields` has the lower of its access and
/// the one given here, `none` or `masked`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenWhere {
    /// The field that selects: of kind `text` or `reference`, read by the
    /// holder.
    pub field: FieldName,
    /// The values that select a record; not empty.
    pub values: BTreeSet<String>,
    /// Each field lowered on a selected record, and to what.
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
    /// every field grant reads, no type admits only some records or hides
    /// a field on some, and
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
                    && t.only_any.is_empty()
                    && t.hide_where.is_empty()
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
    /// borne it; and so is each field a hiding names.
    pub fn follow(&mut self, list: &[Renamed], at: Option<Position>) {
        self.follow_names(list, at);
        let lake = self.default;
        let named = |t: &(&TypeName, &mut TypeGrants)| {
            !t.1.fields.is_empty()
                || !t.1.only.is_empty()
                || !t.1.only_any.is_empty()
                || !t.1.writes_only.is_empty()
                || !t.1.hide_where.is_empty()
        };
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
            let borne = |name: &FieldName| {
                let borne = chains.iter().filter(|c| c.iter().any(|n| n.0 == name));
                let borne: Vec<FieldName> = borne.map(|c| borne_of(c)).collect();
                if borne.is_empty() {
                    vec![name.clone()]
                } else {
                    borne
                }
            };
            for pairs in [&mut held.only, &mut held.writes_only] {
                for (name, value) in std::mem::take(pairs) {
                    for to in borne(&name) {
                        unite(pairs, to, &value);
                    }
                }
            }
            for (name, values) in std::mem::take(&mut held.only_any) {
                for to in borne(&name) {
                    unite_any(&mut held.only_any, to, &values);
                }
            }
            for h in std::mem::take(&mut held.hide_where) {
                let lowered = h
                    .fields
                    .iter()
                    .flat_map(|(f, a)| borne(f).into_iter().map(|f| (f, *a)));
                // Two names borne as one keep the lower access.
                let mut fields: BTreeMap<FieldName, Access> = BTreeMap::new();
                for (f, a) in lowered {
                    let was = fields.entry(f).or_insert(a);
                    *was = was.lowest(a);
                }
                let values = h.values;
                let each = borne(&h.field).into_iter();
                (held.hide_where).extend(each.map(|field| HiddenWhere {
                    field,
                    values: values.clone(),
                    fields: fields.clone(),
                }));
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
            for (f, v) in others.iter().flat_map(|t| &t.writes_only) {
                unite(&mut own.writes_only, f.clone(), v);
            }
            for (f, v) in others.iter().flat_map(|t| &t.only_any) {
                unite_any(&mut own.only_any, f.clone(), v);
            }
            own.whole_sets_only |= others.iter().any(|t| t.whole_sets_only);
            (own.hide_where).extend(others.iter().flat_map(|t| t.hide_where.iter().cloned()));
            own.dates_by_hidden_valid_time &= others.iter().all(|t| t.dates_by_hidden_valid_time);
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

/// Adds a pair of `only` or `writes_only`: two names that become one
/// with differing values hold null, which no grant may hold, so the type is refused to the
/// holder until the host's grants are fixed: never the wider of the two.
fn unite(only: &mut BTreeMap<FieldName, crate::Value>, f: FieldName, v: &crate::Value) {
    let held = only.entry(f).or_insert_with(|| v.clone());
    if held != v {
        *held = crate::Value::Null;
    }
}

/// Adds a set of `only_any`: two names that become one admit the values
/// both list, never the wider of the two.
fn unite_any(any: &mut BTreeMap<FieldName, BTreeSet<String>>, f: FieldName, v: &BTreeSet<String>) {
    let held = any.entry(f).or_insert_with(|| v.clone());
    held.retain(|s| v.contains(s));
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

/// The host's context for a write: its origin, how deep in a chain of
/// reactions it sits, and whom it is made for. The store refuses a
/// reactive write deeper than the lake's configured ceiling, so a trigger
/// cannot feed itself forever.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// The principal the write is made for, `<kind>:<id>` with `<kind>`
    /// one of `human`, `agent` or `service`, where the writer acts for
    /// another: a component filing what a person asked for. It lands in
    /// the envelope's `on_behalf_of` of each event the write appends under
    /// its writer, never in `context`, and in each field's provenance. It
    /// is recorded, not trusted: rights are judged on the writer alone
    /// (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
}

#[cfg(test)]
mod tests;
