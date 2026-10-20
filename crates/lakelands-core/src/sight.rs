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
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::Declared;
use lakelands_api::ontology::{RelationDecl, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::principal::Renamed;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Error, FieldName, Missing, Position, Principal, RecordKey, TypeName, Value, ValueTypeName,
};

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
    /// The value types a use of which it is shown, found in one pass.
    pub(crate) used: OnceCell<BTreeSet<ValueTypeName>>,
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
            used: OnceCell::new(),
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
            used: OnceCell::new(),
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

    /// Whether its principal would be shown `t` once it stood in force
    /// under its name: a proposed shape is judged before it is.
    pub fn would_show(&self, t: &TypeDecl) -> bool {
        let find = |n: &TypeName| match n == &t.name {
            true => Some(t),
            false => self.force.ty(n),
        };
        describe::type_shown(t, &find, &self.who)
    }

    /// Whether its principal reads `field` of `ty`: a field the type
    /// declares, of a type it is shown.
    pub fn reads(&self, ty: &TypeName, field: &FieldName) -> bool {
        let t = self.ty(ty).ok();
        let f = t.and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == field)?)));
        f.is_some_and(|(t, f)| rights::field_access(&self.who, t, f).reads())
    }

    /// What its principal's grants admit of the records of `ty` (051).
    pub fn admit(&self, ty: &TypeName) -> rights::Admit {
        rights::admit(&self.who, ty, &|n| self.force.ty(n))
    }

    /// Whether `r` is held from its principal: a restricted record, to
    /// one without the grant, is not there at any door (006).
    pub fn withheld(&self, r: &StoredRecord) -> bool {
        r.restricted && !self.who.grants.restricted
    }

    /// Whether its principal is answered the record `r` of `ty`, live or
    /// retired: not one held from it, and only one its grant admits
    /// (051). A link's ends are judged beside it ([`Self::ends`]).
    pub fn answers(&self, ty: &TypeName, r: &StoredRecord) -> bool {
        !self.withheld(r) && self.admit(ty).holds(&r.fields)
    }

    /// Whether its principal may not be told some records of `ty`, as
    /// [`Self::setting`] tells them: `ty` is `sys.setting`
    /// and something is held from the principal.
    pub fn narrows(&self, ty: &TypeName) -> bool {
        ty.as_str() == lakelands_api::seam::SETTING_TYPE && !self.holds_all()
    }

    /// Whether nothing is held from its principal: it holds `restricted`
    /// and reads every field of every type.
    fn holds_all(&self) -> bool {
        self.who.grants.reads_everything()
    }

    /// The value of the setting `key` as its principal is told it: one of
    /// a type it is not shown is not told, and one that names fields is
    /// told whole or not at all, as [`Self::setting_told`] says
    /// (`docs/decisions/063-every-setting-is-in-one-list.md`). One for a
    /// source is told only where nothing is held from it, since a source
    /// may feed any type and the lake keeps no list of what it feeds
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    pub fn setting(&self, key: &str, value: Value) -> Option<Value> {
        if !self.setting_told(key) {
            return None;
        }
        let shown = |ty: &str| self.ty(&ty.into()).is_ok();
        lakelands_api::settings::narrowed(key, value, &shown, &|_, _| true)
    }

    /// `r` as its principal is told it: a record of `sys.setting` with its
    /// value as [`Self::setting`] tells it, none where it is not told;
    /// any other record as it is.
    pub fn told<'r>(&self, r: &'r StoredRecord) -> Option<Cow<'r, StoredRecord>> {
        if !self.narrows(&r.ty) {
            return Some(Cow::Borrowed(r));
        }
        let value = r.fields.get("value").cloned().unwrap_or(Value::Null);
        let told = self.setting(r.key.as_str(), value.clone())?;
        if told == value {
            return Some(Cow::Borrowed(r));
        }
        let mut r = r.clone();
        r.fields.insert("value".into(), told);
        Some(Cow::Owned(r))
    }

    /// The settings `all` as its principal is told them, each as
    /// [`Self::setting`] tells it: what an expression evaluated at a read
    /// for it reads by `setting('…')`.
    pub fn settings<'s>(
        &self,
        all: &'s BTreeMap<String, Value>,
    ) -> Cow<'s, BTreeMap<String, Value>> {
        let whole = rights::reads_all(&self.who) && all.keys().all(|k| self.setting_told(k));
        if whole || !self.narrows(&lakelands_api::seam::SETTING_TYPE.into()) {
            return Cow::Borrowed(all);
        }
        let told = all
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), self.setting(k, v.clone())?)));
        Cow::Owned(told.collect())
    }

    /// Whether its principal is told the setting `key` at all, before its
    /// value is read: one for a source where nothing is held from it
    /// ([`Self::setting`]); one naming fields of a type it is shown where
    /// it reads that type whole ([`Self::reads_whole`]), so its version
    /// counts no entry it is not told; any other.
    pub fn setting_told(&self, key: &str) -> bool {
        use lakelands_api::settings::{PerType, lake_setting};
        match lake_setting(key) {
            Some((s, Some(_))) if s.per_type == PerType::Source => self.holds_all(),
            Some((_, Some(ty))) if names_fields(key) => {
                let ty = TypeName::from(ty);
                self.ty(&ty).is_err() || self.reads_whole(&ty)
            }
            _ => true,
        }
    }

    /// Whether its principal reads `ty` whole: its grants keep no field of
    /// the type back, by the type's default or by name, whatever the type
    /// declares, and it reads every field the type declares.
    fn reads_whole(&self, ty: &TypeName) -> bool {
        let grants = self.who.grants.types.get(ty);
        let default = grants
            .and_then(|t| t.default)
            .unwrap_or(self.who.grants.default);
        let named = grants.is_none_or(|t| t.fields.values().all(|a| a.reads()));
        let declared = self.force.ty(ty).map(|t| &t.fields);
        let fields = declared.into_iter().flatten();
        default.reads() && named && fields.into_iter().all(|f| self.reads(ty, &f.name))
    }

    /// Whether a link of `decl` is judged by its ends for its principal:
    /// one that holds `only` on an end's type, or on an end's of an end
    /// that is a link (051), or is without `restricted` (006).
    pub fn judges_ends(&self, decl: &TypeDecl) -> bool {
        !rights::held_ends(&self.who, decl, &|n| self.force.ty(n)).is_empty()
            || (!self.who.grants.restricted && crate::links::is_link_type(&decl.name))
    }

    /// Of links of `decl` holding `rows`, which are there with their ends:
    /// every end whose type its principal holds `only` on admitted, that
    /// end retired or not (051), no end it is not answered (006), and an
    /// end that is a link there with its own ends, as deep as they go. A
    /// read, a write of the link and a reference to it ask this alone.
    /// `get` reads the records of an end's type by key, once for the
    /// batch. The records of any other type all pass.
    pub fn ends<E>(
        &self,
        decl: &TypeDecl,
        rows: &[&BTreeMap<FieldName, Value>],
        get: &mut impl FnMut(&TypeName, &[RecordKey]) -> Result<Vec<StoredRecord>, E>,
    ) -> Result<Vec<bool>, E> {
        let mut out = vec![true; rows.len()];
        if rows.is_empty() || !self.judges_ends(decl) {
            return Ok(out);
        }
        let held = rights::held_ends(&self.who, decl, &|n| self.force.ty(n));
        let named = [crate::links::FROM, crate::links::TO];
        let ends = (decl.fields.iter()).filter(|f| named.contains(&f.name.as_str()));
        for (field, end) in ends.filter_map(|f| Some((&f.name, f.ty.target.as_ref()?))) {
            let key = |r: &&BTreeMap<FieldName, Value>| r.get(field)?.as_str().map(RecordKey::from);
            let keys: Vec<RecordKey> = rows.iter().filter_map(key).collect();
            // `None` where its principal holds no `only` on the end's type.
            let only = held
                .iter()
                .any(|(f, _)| *f == field)
                .then(|| self.admit(end));
            let found = get(end, &keys)?;
            let deep: Vec<_> = found.iter().map(|e| &e.fields).collect();
            let deep = (self.force.ty(end)).map_or(Ok(Vec::new()), |d| self.ends(d, &deep, get))?;
            let admitted = (found.iter().enumerate())
                .filter(|(i, e)| deep.get(*i) != Some(&false) && !self.withheld(e))
                .filter(|(_, e)| only.as_ref().is_none_or(|a| a.holds(&e.fields)));
            let admitted: BTreeSet<&RecordKey> = admitted.map(|(_, e)| &e.key).collect();
            let present: BTreeSet<&RecordKey> = found.iter().map(|e| &e.key).collect();
            for (ok, r) in out.iter_mut().zip(rows) {
                // An end no record holds is judged by `only` alone.
                *ok &= match key(r) {
                    Some(k) => admitted.contains(&k) || (only.is_none() && !present.contains(&k)),
                    None => only.is_none(),
                };
            }
        }
        Ok(out)
    }

    /// Whether its principal is shown relation `r` of `owner`: a relation
    /// it is not shown is one the type does not declare, at every door.
    pub fn relation(&self, owner: &TypeDecl, r: &RelationDecl) -> bool {
        describe::relation_shown(owner, r, &|n| self.force.ty(n), &self.who)
    }

    /// Whether its principal may call `verb` on `on`, its type: its
    /// effect writes a field, and every one it writes is granted to it,
    /// or, for a verb that reaches outside the lake, it may write the type
    /// as a whole, or, for one that retires, it may retire the type's
    /// records; and it reads every field the verb reads (024, extended);
    /// and it holds the verb's own grant, where it is declared to need one
    /// (070).
    pub fn callable(&self, on: &TypeDecl, verb: &VerbDecl) -> bool {
        let writes = |f: &FieldName| self.who.grants.access(&on.name, f).writes();
        rights::verb_granted(&self.who, verb)
            && rights::reads_what_verb_reads(&self.who, on, verb, &|n| self.force.ty(n))
            && match &verb.effect {
                VerbEffect::Write { set } => {
                    !set.is_empty() && set.iter().all(|a| writes(&a.field))
                }
                VerbEffect::External { .. } => rights::type_access(&self.who, &on.name).writes(),
                // A retirement is called as `retire` is used: by one who
                // may write every field of the type (lakelands#1148).
                VerbEffect::Retire => rights::may_retire(&self.who, on),
                _ => false,
            }
    }
}

/// Whether the setting `key` names fields of its type in its value: the
/// index settings and `search_weights` (063).
fn names_fields(key: &str) -> bool {
    use lakelands_api::seam::{
        LOOKUP_INDEX_SETTING, SEARCH_WEIGHTS_SETTING, SORT_INDEX_SETTING, VECTOR_INDEX_SETTING,
    };
    let naming = [
        SORT_INDEX_SETTING,
        VECTOR_INDEX_SETTING,
        LOOKUP_INDEX_SETTING,
        SEARCH_WEIGHTS_SETTING,
    ];
    naming.iter().any(|p| key.starts_with(p))
}

#[cfg(test)]
mod tests;
