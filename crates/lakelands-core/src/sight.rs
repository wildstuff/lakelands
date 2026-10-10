//! One judgement of what a principal is shown
//! (`docs/decisions/006-grants-not-roles.md`, extended).
//!
//! Every door asks one [`Sight`] whether a type, a relation or a
//! declaration is there for its principal: a read and a write, `describe`
//! and the tools made from it, a listing, and a refusal that would name
//! one. A door that answered by its own means would sooner or later
//! answer otherwise, and hand out what another keeps.
//!
//! A sight is of one principal over the declarations in force at one
//! position. A door makes it of the principal it is called with: its
//! grants are held through the lake's renames, and each reference to a
//! type it is not shown is held at `none`. The listing of proposals alone
//! follows its reader's grants itself, to the position an entry was
//! proposed at. What is shown depends on nothing else a read asks for: a
//! valid time filters records, and the read that filters by it probes its
//! fields.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;

use lakelands_api::engine::Declared;
use lakelands_api::ontology::{RelationDecl, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::principal::Renamed;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, FieldName, Missing, Position, Principal, TypeName};

use crate::{describe, rights};

/// The declarations in force, as a sight asks for them.
pub trait InForce {
    /// The type declared under `name`.
    fn ty(&self, name: &TypeName) -> Option<&TypeDecl>;
    /// Every type, in one order whenever it is asked: the references a
    /// sight hides are listed in it.
    fn types(&self) -> Vec<&TypeDecl>;
    /// Every declaration, in declaration order: asked where a declaration
    /// beside the types is judged, since one that holds them otherwise
    /// lists them anew.
    fn declared(&self) -> Cow<'_, [Declared]>;
}

fn as_type(d: &Declared) -> Option<&TypeDecl> {
    match &d.declaration {
        Declaration::Type(t) => Some(t),
        _ => None,
    }
}

impl InForce for Vec<Declared> {
    fn ty(&self, name: &TypeName) -> Option<&TypeDecl> {
        self.iter().filter_map(as_type).find(|t| &t.name == name)
    }

    fn types(&self) -> Vec<&TypeDecl> {
        self.iter().filter_map(as_type).collect()
    }

    fn declared(&self) -> Cow<'_, [Declared]> {
        self.as_slice().into()
    }
}

/// What one principal is shown of the declarations in force.
pub struct Sight<'o> {
    who: Cow<'o, Principal>,
    pub(crate) force: &'o dyn InForce,
    hidden: Vec<(TypeName, FieldName)>,
    /// Each type asked for, judged once.
    shown: RefCell<BTreeMap<TypeName, bool>>,
}

impl<'o> Sight<'o> {
    /// The sight of `who` over `force`: its grants held through `renames`
    /// (`Grants::follow`), for a read at `at` or at the head, and each
    /// reference to a type it is not shown held at `none` (024 and 051,
    /// extended).
    pub fn new(
        mut who: Principal,
        renames: &[Renamed],
        at: Option<Position>,
        force: &'o dyn InForce,
    ) -> Sight<'o> {
        who.grants.follow(renames, at);
        let hidden = rights::hide_unshown_references(&mut who, force);
        Sight {
            who: Cow::Owned(who),
            force,
            hidden,
            shown: RefCell::default(),
        }
    }

    /// The sight of `who` as it stands, over `force`: of a principal a
    /// sight was made of, as a write judges under its lock (the head
    /// showed it the same references when its call was made, or the call
    /// is refused) and wherever its caller is judged again; and of one
    /// that hides nothing, as the proposer an accept writes as.
    pub fn held(who: &'o Principal, force: &'o dyn InForce) -> Sight<'o> {
        Sight {
            who: Cow::Borrowed(who),
            force,
            hidden: Vec::new(),
            shown: RefCell::default(),
        }
    }

    /// The principal, as it is judged.
    pub fn who(&self) -> &Principal {
        &self.who
    }

    /// The references held at `none` to make it, in order.
    pub fn hidden(&self) -> &[(TypeName, FieldName)] {
        &self.hidden
    }

    /// The type `name`, where its principal is shown it. One it is not
    /// shown is answered as a name that is not declared, at every door,
    /// before anything else is judged: a refusal would tell that the
    /// type is there.
    pub fn ty(&self, name: &TypeName) -> Result<&'o TypeDecl, Error> {
        let known = self.shown.borrow().get(name).copied();
        let t = self.force.ty(name).filter(|t| {
            known.unwrap_or_else(|| {
                let shown = describe::type_shown(t, &|n| self.force.ty(n), &self.who);
                self.shown.borrow_mut().insert(name.clone(), shown);
                shown
            })
        });
        let missing = Missing::Type { name: name.clone() };
        t.ok_or(Error::NotFound { missing })
    }

    /// Whether its principal is shown relation `r` of `owner`: a relation
    /// it is not shown is one the type does not declare, at every door.
    pub fn relation(&self, owner: &TypeDecl, r: &RelationDecl) -> bool {
        describe::relation_shown(owner, r, &|n| self.force.ty(n), &self.who)
    }

    /// Whether its principal may call `verb` on `on`, its type: its
    /// effect writes a field, and every one it writes is granted to it,
    /// or, for a verb that reaches outside the lake, it may write the type
    /// as a whole; and it reads every field the verb reads (024, extended).
    pub fn callable(&self, on: &TypeDecl, verb: &VerbDecl) -> bool {
        let writes = |f: &FieldName| self.who.grants.access(&on.name, f).writes();
        rights::reads_what_verb_reads(&self.who, on, verb)
            && match &verb.effect {
                VerbEffect::Write { set } => {
                    !set.is_empty() && set.iter().all(|a| writes(&a.field))
                }
                VerbEffect::External { .. } => rights::type_access(&self.who, &on.name).writes(),
            }
    }
}

#[cfg(test)]
mod tests;
