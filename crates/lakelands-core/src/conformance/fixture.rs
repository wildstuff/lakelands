//! What every check needs: principals, a small ontology, and reading
//! back.
//!
//! Stated in contract types only, so that a check reads as the rule it
//! checks and an engine is never measured by anything but its doors.
//! The helpers that can fail answer a [`Failure`] naming what was being
//! done; [`Must`] does the same for any call a check makes itself.

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use lakelands_api::error::{Case, Said};
use lakelands_api::ontology::{
    Authority, EmbedDecl, FieldDecl, Measure, StorageKind, TypeDecl, ViewDecl, ViewParam,
};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Depth, Pattern, Projection, Start, Step, Walk};
use lakelands_api::read::{AggFunc, AggMeasure, Direction, Order, OrderBy};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Access, Catalog, Committed, Error, FieldName, Find, Found, Get, Grants, LakeStore, Outcome,
    Params, Patch, Position, Principal, PrincipalKind, ReadAt, Record, RecordKey, Refusal,
    SourceId, SourceRecord, SourceStream, TypeName, Value, WriteContext, WriteOpts,
};
use rust_decimal::Decimal;

use super::{Failure, Held};

/// An answer a refusal may be in: an error, or a result that may hold one.
pub trait Answer {
    /// The error the answer holds, if any.
    fn error(&self) -> Option<&Error>;
}

impl Answer for Error {
    fn error(&self) -> Option<&Error> {
        Some(self)
    }
}

impl<T> Answer for lakelands_api::Result<T> {
    fn error(&self) -> Option<&Error> {
        self.as_ref().err()
    }
}

impl<A: Answer + ?Sized> Answer for &A {
    fn error(&self) -> Option<&Error> {
        (**self).error()
    }
}

/// Why an answer is a refusal, if it is one.
pub fn reason(a: &impl Answer) -> Option<&Refusal> {
    match a.error() {
        Some(Error::Refused { reason, .. }) => Some(reason),
        _ => None,
    }
}

/// The field an answer's refusal names, and why, if it is a refusal that
/// names one.
pub fn refused_on(a: &impl Answer) -> Option<(&str, &Refusal)> {
    match a.error() {
        Some(Error::Refused {
            field: Some(f),
            reason,
        }) => Some((f.as_str(), reason)),
        _ => None,
    }
}

/// Whether an answer is the one for a field the type does not declare,
/// naming `name`: also the answer for a field the principal may not know
/// of (`docs/decisions/006-grants-not-roles.md`).
pub fn no_such_field(a: &impl Answer, name: &str) -> bool {
    matches!(refused_on(a), Some((f, Refusal::Invalid(said)))
        if f == name && matches!(&said.case, Case::UnknownField { field, .. } if field == name))
}

/// Whether an answer is a refusal as invalid.
pub fn is_invalid(a: &impl Answer) -> bool {
    matches!(reason(a), Some(Refusal::Invalid(..)))
}

/// What an answer's refusal as invalid says, if it is one.
pub fn invalid_said(a: &impl Answer) -> Option<&Said> {
    match reason(a) {
        Some(Refusal::Invalid(said)) => Some(said),
        _ => None,
    }
}

/// The case of an answer's refusal as invalid, if it is one.
pub fn invalid_case(a: &impl Answer) -> Option<&Case> {
    invalid_said(a).map(|said| &said.case)
}

/// `answer`, to `what`, must be a refusal as invalid that `fits` holds for,
/// `expected` saying which: what the suite's `refused_as!` writes.
#[track_caller]
pub fn refused_as<T: Debug>(
    what: &str,
    answer: lakelands_api::Result<T>,
    expected: &str,
    fits: impl FnOnce(&Said) -> bool,
) -> Held {
    super::tally();
    match invalid_said(&answer) {
        Some(said) if fits(said) => Ok(()),
        Some(said) => Err(Failure::new(format!(
            "{what} must be refused as {expected}; it said {said:?} (\"{said}\")"
        ))),
        None => Err(Failure::new(format!(
            "{what} must be refused as invalid ({expected}); it answered {answer:?}"
        ))),
    }
}

/// `answer`, to `what`, must be a refusal as invalid whose case and places,
/// on the wire, hold every member `expected` holds: `{"code": …}` and the
/// parameters a row names, `"at": [{"code": "field", …}]` for places it
/// is said at, in their order (the others are not compared).
#[track_caller]
pub fn refused_like<T: Debug>(
    what: &str,
    answer: lakelands_api::Result<T>,
    expected: &Value,
) -> Held {
    let shown = expected.to_string();
    refused_as(what, answer, &shown, |said| said_like(said, expected))
}

/// Whether `said`, on the wire, holds `expected` as [`refused_like`] reads it.
pub fn said_like(said: &Said, expected: &Value) -> bool {
    serde_json::to_value(said).is_ok_and(|wire| holds(&wire, expected))
}

/// Whether an answer is refused as invalid as [`refused_like`] reads `expected`.
pub fn invalid_like(a: &impl Answer, expected: &Value) -> bool {
    invalid_said(a).is_some_and(|said| said_like(said, expected))
}

/// Whether `got` holds `expected`: every member of an object, each item
/// of a list in its order among others, any other value as it is.
fn holds(got: &Value, expected: &Value) -> bool {
    match (got, expected) {
        (Value::Object(g), Value::Object(e)) => {
            e.iter().all(|(k, v)| g.get(k).is_some_and(|g| holds(g, v)))
        }
        (Value::Array(g), Value::Array(e)) => {
            let mut g = g.iter();
            e.iter().all(|e| g.any(|g| holds(g, e)))
        }
        _ => got == expected,
    }
}

/// `hidden`, to `what`, must be refused as `absent` is once its name
/// `from` is `to`, as [`alike_renamed`] reads them; `absent` must be refused.
pub fn refused_alike<T: Debug, U: Debug>(
    what: &str,
    hidden: &lakelands_api::Result<T>,
    absent: lakelands_api::Result<U>,
    names: (&str, &str),
) -> Held {
    let absent = absent.must_fail(&format!("{what}, as `{}`", names.0))?;
    match hidden {
        Err(e) => alike_renamed(what, e, &absent, names),
        Ok(v) => Err(Failure::new(format!("{what} must be refused: {v:?}"))),
    }
}

/// Whether each of `hidden` is refused as `absent`'s beside it ([`alike_renamed`]).
pub fn alike_each(hidden: &[Option<Error>], absent: &[Option<Error>], names: (&str, &str)) -> bool {
    let alike = |(h, a): (&Option<Error>, &Option<Error>)| {
        (h.iter().zip(a)).any(|(h, a)| alike_renamed("", h, a, names).is_ok())
    };
    hidden.len() == absent.len() && hidden.iter().zip(absent).all(alike)
}

/// `hidden`, to `what`, must be the very error `absent` is (its code, places
/// and parameters, and its wire form, `message` among them) once the name
/// `from` that `absent` was asked by is `to` in each text of it: a hidden
/// thing is answered as an absent one of its name. `from` is used nowhere
/// else; `("", "")` renames nothing.
pub fn alike_renamed(what: &str, hidden: &Error, absent: &Error, (from, to): (&str, &str)) -> Held {
    let wire = |e: &Error| serde_json::to_string(e).map_err(|e| Failure::new(e.to_string()));
    let (h, a) = (wire(hidden)?, wire(absent)?);
    // `from` and `to` are names, so no escape in the wire text holds them.
    let a = if from.is_empty() {
        a
    } else {
        a.replace(from, to)
    };
    let read = serde_json::from_str::<Error>(&a);
    if h == a && read.as_ref().is_ok_and(|a| a == hidden) {
        return Ok(());
    }
    Err(Failure::new(format!(
        "{what} must be answered as the absent one is, alike in every member: it answered \
         {h}, and the absent one {a}"
    )))
}

/// Turns the result of a contract call into the check's own result.
pub trait Must<T> {
    /// The value, or a failure saying that `what` failed and why.
    fn must(self, what: &str) -> Result<T, Failure>;

    /// The error, or a failure saying that `what` should have been
    /// refused and was not.
    fn must_fail(self, what: &str) -> Result<Error, Failure>;
}

impl<T: Debug> Must<T> for lakelands_api::Result<T> {
    #[track_caller]
    fn must(self, what: &str) -> Result<T, Failure> {
        super::tally();
        self.map_err(|e| Failure::new(format!("{what} failed: {e} ({e:?})")))
    }

    #[track_caller]
    fn must_fail(self, what: &str) -> Result<Error, Failure> {
        super::tally();
        match self {
            Ok(v) => Err(Failure::new(format!(
                "{what} should have been refused, but it answered {v:?}"
            ))),
            Err(e) => Ok(e),
        }
    }
}

/// The limits every lake of the suite is opened with: the defaults, as
/// [`Harness`](super::Harness) requires. A check that meets one reads it
/// here; [`default_limits`] holds every lake to it before any check runs.
pub fn limits() -> lakelands_api::location::Limits {
    lakelands_api::location::Limits::default()
}

/// Fail unless `store` was opened with the default limits, so a harness
/// that opens lakes otherwise fails here, naming the limits, and never
/// in a check that happens to meet one.
pub fn default_limits(store: &dyn LakeStore) -> Held {
    let got = store.limits();
    if got == limits() {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "the harness opened the lake with limits {got:?}; the suite runs on the \
             defaults, {:?}",
            limits()
        )))
    }
}

/// The doors of `store` for a direct caller.
pub fn doors(store: &dyn LakeStore) -> Arc<dyn Catalog> {
    store.catalog(WriteContext::default())
}

/// The lake's owner: a person holding every grant.
pub fn owner(store: &dyn LakeStore) -> Principal {
    principal(store, PrincipalKind::Human, "owner", Grants::owner())
}

/// A check's lake: its doors, and its owner, who writes most of what a
/// check sets up. A write as someone else goes through the free helpers
/// with the lake's `cat` and that principal.
pub struct Lake {
    /// The lake's doors.
    pub cat: Arc<dyn Catalog>,
    /// Its owner.
    pub own: Principal,
}

impl Lake {
    /// The doors of `store` and its owner.
    pub fn of(store: &dyn LakeStore) -> Lake {
        Lake {
            cat: doors(store),
            own: owner(store),
        }
    }

    /// Writes the setting `key` as the owner.
    pub async fn set(&self, key: &str, value: Value) -> Result<Outcome, Failure> {
        let setting = serde_json::json!({"key": key, "value": value});
        self.upsert(lakelands_api::seam::SETTING_TYPE, key, setting)
            .await
    }

    /// Declares a type as the owner.
    pub async fn declare(&self, ty: TypeDecl) -> Result<Outcome, Failure> {
        declare(self.cat.as_ref(), &self.own, ty).await
    }

    /// Upserts `fields` into one record as the owner, with no options.
    pub async fn upsert(&self, ty: &str, key: &str, fields: Value) -> Result<Outcome, Failure> {
        upsert(self.cat.as_ref(), &self.own, ty, key, fields).await
    }

    /// The lake as `describe` answers it to the owner at the head.
    pub async fn describe(&self) -> Result<lakelands_api::describe::LakeDescription, Failure> {
        let d = self.cat.describe(&self.own, ReadAt::default()).await;
        Ok(d.must("describing the lake as the owner")?.data)
    }

    /// The executor `describe` names for `verb` to the owner, and whether
    /// it says it is bound.
    pub async fn executor_of(&self, verb: &str) -> Result<Option<(String, bool)>, Failure> {
        let d = self.describe().await?;
        let v = d.verbs.into_iter().find(|v| v.name.as_str() == verb);
        Ok(v.and_then(|v| v.executor).map(|e| (e.name, e.bound)))
    }

    /// The type `ty` as `describe` answers it to the owner at the head.
    pub async fn describe_type(
        &self,
        ty: &str,
    ) -> Result<lakelands_api::describe::TypeDescription, Failure> {
        describe_type(self.cat.as_ref(), &self.own, ty).await
    }

    /// Declares `d` as the owner; `what` names it if that fails.
    pub async fn declare_as(&self, d: Declaration, what: &str) -> Result<Outcome, Failure> {
        self.cat.declare(&self.own, d).await.must(what)
    }

    /// Declares, as the owner, the declaration `json` describes.
    pub async fn declare_json(&self, json: Value) -> Result<Outcome, Failure> {
        let what = format!("declaring {json}");
        let d: Declaration = serde_json::from_value(json)
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
        self.cat.declare(&self.own, d).await.must(&what)
    }

    /// Declares a type as the owner, answering the store's raw result.
    pub async fn try_declare(&self, ty: TypeDecl) -> lakelands_api::Result<Outcome> {
        self.cat.declare(&self.own, Declaration::Type(ty)).await
    }

    /// Migrates a type to `to` as the owner: the preflight, then the apply
    /// of the report it answered.
    pub async fn migrate(&self, to: TypeDecl) -> Result<Outcome, Failure> {
        self.migrate_renaming(to, &[]).await
    }

    /// [`Self::migrate`], each field of `renames` renamed to its new name.
    pub async fn migrate_renaming(
        &self,
        to: TypeDecl,
        renames: &[(&str, &str)],
    ) -> Result<Outcome, Failure> {
        let what = format!("migrating `{}`", to.name);
        let m = lakelands_api::write::Migration {
            renames: renames
                .iter()
                .map(|(o, n)| ((*o).into(), (*n).into()))
                .collect(),
            ..migration(to)
        };
        let report = self.cat.migrate(&self.own, m.clone());
        let id = report.await.must(&what)?.data.report_id;
        self.cat.migrate_apply(&self.own, m, &id).await.must(&what)
    }

    /// Retires one record as the owner.
    pub async fn retire(&self, ty: &str, key: &str) -> Result<Outcome, Failure> {
        let (t, k) = (TypeName::from(ty), RecordKey::from(key));
        let retired = self.cat.retire(&self.own, &t, &k, WriteOpts::default());
        retired.await.must(&format!("retiring `{ty}` `{key}`"))
    }

    /// Reads one record at the head as the owner, with provenance.
    pub async fn read(&self, ty: &str, key: &str) -> Result<Record, Failure> {
        read(self.cat.as_ref(), &self.own, ty, key).await
    }

    /// Every record of `ty` the owner finds with this request.
    pub async fn all_found(&self, ty: &str, find: Find) -> Result<Vec<Record>, Failure> {
        all_found(self.cat.as_ref(), &self.own, ty, find).await
    }

    /// Feeds `records` from `source` as the owner, as [`ingest`] does.
    pub async fn ingest(
        &self,
        source: &str,
        records: Vec<SourceRecord>,
    ) -> lakelands_api::Result<Vec<Committed>> {
        ingest(self.cat.as_ref(), &self.own, source, records).await
    }
}

/// Two records of `mark`, each made under its own declaration of `flag`'s
/// `initial`, `false` then `true`: a fold of many events at once, a
/// rebuild's or an import's, answers each as made (050).
pub async fn two_makes(l: &Lake) -> Held {
    for (initial, key) in [(false, "m1"), (true, "m2")] {
        l.declare_json(
            serde_json::json!({"declare": "type", "name": "mark", "fields": [
            {"name": "n", "type": {"kind": "text"}},
            {"name": "flag", "type": {"kind": "boolean"}, "initial": initial}]}),
        )
        .await?;
        l.upsert("mark", key, serde_json::json!({"n": key})).await?;
    }
    Ok(())
}

/// A principal of the lake with the given grants.
pub fn principal(
    store: &dyn LakeStore,
    kind: PrincipalKind,
    id: &str,
    grants: Grants,
) -> Principal {
    Principal {
        kind,
        id: id.to_owned(),
        lake: store.lake().clone(),
        grants,
    }
}

/// A reviewer holding `review`, and `schema` when asked.
pub fn reviewer(store: &dyn LakeStore, schema: bool) -> Principal {
    principal(
        store,
        PrincipalKind::Human,
        if schema { "architect" } else { "reviewer" },
        Grants {
            default: Access::Read,
            review: true,
            schema,
            ..Grants::default()
        },
    )
}

/// An agent `id` that reads everything but holds `fields` of `ty`, each
/// with its own access.
pub fn field_access(
    store: &dyn LakeStore,
    id: &str,
    ty: &str,
    fields: &[(&str, Access)],
) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let fields = fields.iter().map(|(f, a)| (FieldName::from(*f), *a));
    grants.types.insert(
        TypeName::from(ty),
        TypeGrants {
            default: None,
            fields: fields.collect(),
            ..Default::default()
        },
    );
    principal(store, PrincipalKind::Agent, id, grants)
}

/// An agent that reads everything and may write only `fields` of `ty`.
pub fn field_writer(store: &dyn LakeStore, id: &str, ty: &str, fields: &[&str]) -> Principal {
    let fields: Vec<_> = fields.iter().map(|f| (*f, Access::Write)).collect();
    field_access(store, id, ty, &fields)
}

/// The type `ty` as `describe` answers it to `who` at the head.
pub async fn describe_type(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
) -> Result<lakelands_api::describe::TypeDescription, Failure> {
    let (t, what) = (
        TypeName::from(ty),
        format!("describing `{ty}` as `{}`", who.writer()),
    );
    let d = cat.describe_type(who, &t, ReadAt::default());
    Ok(d.await.must(&what)?.data)
}

/// A field of a bare storage kind, authored, optional, with every other
/// member at its default.
pub fn field(name: &str, kind: StorageKind) -> FieldDecl {
    FieldDecl::of(name, kind)
}

/// A field only a source writes.
pub fn sourced(name: &str, kind: StorageKind) -> FieldDecl {
    FieldDecl {
        authority: Authority::Source,
        ..field(name, kind)
    }
}

/// A field derived by `expr` from the fields `deps`.
pub fn derived(name: &str, kind: StorageKind, expr: &str, deps: &[&str]) -> FieldDecl {
    FieldDecl {
        authority: Authority::Derived,
        derived: Some(expr.into()),
        depends_on: deps.iter().map(|d| FieldName::from(*d)).collect(),
        ..field(name, kind)
    }
}

/// A field computed by the function `function` from the fields `deps`.
pub fn computed_by(name: &str, kind: StorageKind, function: &str, deps: &[&str]) -> FieldDecl {
    FieldDecl {
        authority: Authority::Derived,
        function: Some(function.into()),
        depends_on: deps.iter().map(|d| FieldName::from(*d)).collect(),
        ..field(name, kind)
    }
}

/// A vector field of `dims` dimensions.
pub fn vector(name: &str, dims: u32) -> FieldDecl {
    let mut f = field(name, StorageKind::Vector);
    f.ty.dimensions = Some(dims);
    f
}

/// A vector field of `dims` dimensions, the embedding by `model` of the
/// fields `from`.
pub fn embedding(name: &str, dims: u32, from: &[&str], model: &str) -> FieldDecl {
    let from = from.iter().map(|f| FieldName::from(*f)).collect();
    FieldDecl {
        authority: Authority::Derived,
        embed: Some(EmbedDecl {
            from,
            model: model.into(),
        }),
        ..vector(name, dims)
    }
}

/// A type whose records are keyed by the writer, of plain fields, each a
/// name and its kind.
pub fn type_of(name: &str, fields: &[(&str, StorageKind)]) -> TypeDecl {
    type_decl(name, fields.iter().map(|(n, k)| field(n, *k)).collect())
}

/// A field that refers to a record of `target`.
pub fn reference(name: &str, target: &str) -> FieldDecl {
    FieldDecl {
        ty: lakelands_api::ontology::FieldType {
            target: Some(target.into()),
            ..lakelands_api::ontology::FieldType::kind(StorageKind::Reference)
        },
        ..field(name, StorageKind::Text)
    }
}

/// A searchable text field.
pub fn searchable(name: &str) -> FieldDecl {
    FieldDecl {
        searchable: true,
        ..field(name, StorageKind::Text)
    }
}

/// A type whose records are keyed by the writer, with these fields.
pub fn type_decl(name: &str, fields: Vec<FieldDecl>) -> TypeDecl {
    TypeDecl {
        fields,
        field_meta: true,
        ..TypeDecl::named(name)
    }
}

/// A view's measure: `name`, computed as `expr`.
pub fn measure(name: &str, expr: &str) -> Measure {
    Measure {
        name: FieldName::from(name),
        expr: expr.into(),
    }
}

/// A view `name` over `source`, keyed and grouped by `by`, declaring
/// `fields` and computing `measures`, each a name and its expression.
pub fn view_of(
    name: &str,
    source: &str,
    by: &[&str],
    fields: Vec<FieldDecl>,
    measures: &[(&str, &str)],
) -> TypeDecl {
    let by: Vec<FieldName> = by.iter().map(|f| FieldName::from(*f)).collect();
    TypeDecl {
        key: by.clone(),
        view: Some(Box::new(ViewDecl {
            source: TypeName::from(source),
            params: Vec::new(),
            r#where: None,
            group_by: by,
            measures: measures.iter().map(|(n, e)| measure(n, e)).collect(),
            merge: Vec::new(),
        })),
        ..type_decl(name, fields)
    }
}

/// A view `name` counting the records of `source` by the text field `by`,
/// into `n`.
pub fn count_view(name: &str, source: &str, by: &str) -> TypeDecl {
    let fields = vec![
        field(by, StorageKind::Text),
        field("n", StorageKind::Number),
    ];
    view_of(name, source, &[by], fields, &[("n", "count()")])
}

/// A find of every record, binding `params`.
pub fn bound(params: Params) -> Find {
    Find {
        params,
        ..Find::default()
    }
}

/// A view's parameter `name`, of `kind`.
pub fn param(name: &str, kind: StorageKind) -> ViewParam {
    ViewParam {
        name: name.into(),
        kind,
    }
}

/// A find of the records `filter` holds for.
pub fn find_where(filter: &str) -> Find {
    Find {
        filter: Some(filter.into()),
        ..Find::default()
    }
}

/// One sort key: `field`'s `by`, in `dir`.
pub fn order(field: &str, by: OrderBy, dir: Direction) -> Order {
    Order {
        field: FieldName::from(field),
        by,
        dir,
    }
}

/// An aggregate's measure `name`: `func` over `field`, or over the records.
pub fn agg(name: &str, func: AggFunc, field: Option<&str>) -> AggMeasure {
    AggMeasure {
        name: FieldName::from(name),
        func,
        field: field.map(FieldName::from),
    }
}

/// A find of `measures` over every record, with no `group_by`.
pub fn aggregate(measures: Vec<AggMeasure>) -> Find {
    Find {
        aggregate: Some(lakelands_api::read::Aggregate {
            measures,
            group_by: Vec::new(),
        }),
        ..Find::default()
    }
}

/// The trigger `name` on `on`, firing when `when` holds (on every change
/// without one), with no ceiling and no description.
pub fn trigger_on(name: &str, on: &str, when: Option<&str>) -> Declaration {
    Declaration::Trigger(lakelands_api::ontology::TriggerDecl {
        max_depth: None,
        when_strict: false,
        on_changes: None,
        name: name.into(),
        on: on.into(),
        when: when.map(Into::into),
        description: None,
    })
}

/// A low-risk verb `name` on the records of `on` that sets `field` to
/// the expression `value`, taking no payload.
pub fn write_verb(name: &str, on: &str, field: &str, value: &str) -> Declaration {
    use lakelands_api::ontology::{Assignment, Risk, VerbDecl, VerbEffect};
    let set = vec![Assignment {
        field: field.into(),
        value: value.into(),
    }];
    Declaration::Verb(VerbDecl {
        name: name.into(),
        on: on.into(),
        scope: Default::default(),
        payload: vec![],
        risk: Risk::Low,
        requires: None,
        effect: VerbEffect::Write { set },
        description: None,
    })
}

/// The object set `name` of the records of `of` that `filter` holds for.
pub fn object_set_on(name: &str, of: &str, filter: &str) -> Declaration {
    Declaration::ObjectSet(lakelands_api::write::ObjectSetDecl {
        name: name.into(),
        of: of.into(),
        filter: filter.into(),
        description: None,
    })
}

/// One hop over `relation`, in the direction `walk`.
pub fn step(relation: &str, walk: Walk) -> Step {
    Step {
        relation: relation.into(),
        walk,
        ty: None,
        filter: None,
        depth: Depth::default(),
        named: None,
    }
}

/// A pattern from the records of `ty`, or the one at `key`, over `steps`,
/// answering `project`; no filter, order, limit or time axis.
pub fn pattern(ty: &str, key: Option<&str>, steps: Vec<Step>, project: Projection) -> Pattern {
    Pattern {
        start: Start {
            ty: Some(ty.into()),
            key: key.map(Into::into),
            filter: None,
            search: None,
            params: Default::default(),
            named: None,
            rerank: None,
        },
        steps,
        to: None,
        project,
        order_by: Vec::new(),
        limit: None,
        cursor: None,
        at: ReadAt::default(),
    }
}

/// Declares a type as `who`.
pub async fn declare(cat: &dyn Catalog, who: &Principal, ty: TypeDecl) -> Result<Outcome, Failure> {
    let what = format!("declaring type `{}`", ty.name);
    cat.declare(who, Declaration::Type(ty)).await.must(&what)
}

/// Declares `declared` below the store, as a log another writer made
/// holds them, each a `declared` event after the last, and opens the
/// lake again through the store for writing.
pub async fn declared_below(
    store: super::Store,
    given: &super::Given,
    declared: Vec<Declaration>,
) -> Result<super::Store, Failure> {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::{Event, EventBody, EventId};
    let template = log_events(store.as_ref()).await?.pop();
    let template = template.ok_or_else(|| Failure::new("the lake holds a log"))?;
    let lines: super::Lines = Box::new(move |(mut tip, mut prev), fence| {
        let mut lines = Vec::new();
        for declaration in declared {
            let position = tip.next();
            let body = EventBody::Declared {
                declaration,
                schema_hash: None,
                version: 1,
                with_the_set: false,
            };
            let line = canonical_line(Event {
                position,
                id: EventId(format!("{:032x}", position.0)),
                prev: prev.clone(),
                fence,
                body,
                delivery: None,
                ..template.clone()
            });
            (tip, prev) = (position, line.hash.clone());
            lines.push(line);
        }
        (lines, 0)
    });
    let lake = store.lake().clone();
    let seams = lakelands_api::seam::Seams::default();
    let c = (given.continued)(Some(store), lake, lines, OpenMode::Write, seams);
    let c = c
        .await
        .map_err(|e| Failure::new(format!("continuing the lake below the store: {e}")))?;
    c.opened
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))
}

/// `event` as a line of its log, its hash made over the rest.
pub fn canonical_line(event: lakelands_api::event::Event) -> lakelands_api::engine::CanonicalEvent {
    let mut value = serde_json::json!(event);
    value.as_object_mut().map(|m| m.remove("hash"));
    let hash = lakelands_api::event::Digest(crate::canonical::sha256_hex(&value));
    value["hash"] = serde_json::json!(hash);
    let bytes = crate::canonical::to_vec(&value);
    let (position, id, prev, fence) = (event.position, event.id, event.prev, event.fence);
    lakelands_api::engine::CanonicalEvent {
        position,
        id,
        prev,
        hash,
        fence,
        bytes,
    }
}

/// The lake's log, oldest first: up to 10 000 events.
pub async fn log_events(
    store: &dyn LakeStore,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    events_from(store, Position::GENESIS).await
}

/// The lake's events from `from` on, oldest first.
pub async fn events_from(
    store: &dyn LakeStore,
    from: Position,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    followed(store, from, &lakelands_api::seam::FeedFilter::default()).await
}

/// The events `filter` admits from `from` to the head, oldest first: the
/// feed followed by `next`, since a poll examines a bounded part of it.
pub async fn followed(
    store: &dyn LakeStore,
    mut from: Position,
    filter: &lakelands_api::seam::FeedFilter,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    let mut events = Vec::new();
    loop {
        let polled = store.feed().poll(from, filter, u32::MAX).await;
        let batch = polled.must(&format!("reading the feed from {from} with {filter:?}"))?;
        events.extend(batch.events);
        if batch.next > batch.head || batch.next == from {
            return Ok(events);
        }
        from = batch.next;
    }
}

/// The lake's events after `after`, oldest first.
pub async fn events_after(
    store: &dyn LakeStore,
    after: Position,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    events_from(store, after.next()).await
}

/// A host's reactive context `depth` deep, naming `cause`.
pub fn reactive(depth: u32, cause: Option<Position>) -> WriteContext {
    WriteContext {
        origin: lakelands_api::principal::WriteOrigin::Reactive,
        depth,
        cause,
    }
}

/// A migration of a type to `to`, with no field renamed.
pub fn migration(to: TypeDecl) -> lakelands_api::write::Migration {
    lakelands_api::write::Migration::to(to)
}

/// A value type `name` of `kind`, with no constraint, choices, format or
/// scale.
pub fn value_type(name: &str, kind: StorageKind) -> lakelands_api::ontology::ValueTypeDecl {
    lakelands_api::ontology::ValueTypeDecl {
        name: name.into(),
        kind,
        ..Default::default()
    }
}

/// An agent of the lake that holds no grant at all.
pub fn nobody(store: &dyn LakeStore) -> Principal {
    principal(store, PrincipalKind::Agent, "nobody", Grants::default())
}

/// An agent `id` that reads every field and writes none.
pub fn reads_all(store: &dyn LakeStore, id: &str) -> Principal {
    let grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    principal(store, PrincipalKind::Agent, id, grants)
}

/// Restricts one record of `ty` for `why`, at the store's own door.
pub async fn restrict(
    store: &dyn LakeStore,
    ty: &str,
    key: impl Into<RecordKey>,
    why: &str,
) -> Result<Position, Failure> {
    let (t, k) = (TypeName::from(ty), key.into());
    let what = format!("restricting `{ty}` `{}`", k.as_str());
    store.restrict(&t, &k, why.into()).await.must(&what)
}

/// The `output_recorded` events of the lake's log, oldest first.
pub async fn recorded_outputs(
    store: &dyn LakeStore,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    outputs_from(store, Position::GENESIS).await
}

/// The `output_recorded` events of the lake's log from `from` on, oldest
/// first.
pub async fn outputs_from(
    store: &dyn LakeStore,
    from: Position,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    let outputs = lakelands_api::seam::FeedFilter {
        kinds: vec!["output_recorded".into()],
        ..Default::default()
    };
    followed(store, from, &outputs).await
}

/// A view `name` over `invoice`, keyed by `partner`: `total` the sum of
/// the partner's `amount`s, as a field of kind `total`, and `invoices`
/// their count.
pub fn partner_totals(name: &str, total: StorageKind) -> TypeDecl {
    let fields = vec![
        field("partner", StorageKind::Text),
        field("total", total),
        field("invoices", StorageKind::Number),
    ];
    let measures = [("total", "sum(amount)"), ("invoices", "count()")];
    view_of(name, "invoice", &["partner"], fields, &measures)
}

/// A patch from a JSON object; anything but an object is an empty patch.
pub fn patch(fields: Value) -> Patch {
    match fields {
        Value::Object(map) => map.into_iter().map(|(k, v)| (FieldName(k), v)).collect(),
        _ => Patch::new(),
    }
}

/// Upserts `fields` into one record as `who`, with no options.
pub async fn upsert(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
    fields: Value,
) -> Result<Outcome, Failure> {
    let what = format!("upserting `{ty}` `{key}` as `{}`", who.writer());
    cat.upsert(
        who,
        &TypeName::from(ty),
        &RecordKey::from(key),
        patch(fields),
        WriteOpts::default(),
    )
    .await
    .must(&what)
}

/// An upsert of `fields` into one record with no options, its answer as
/// it came: for a check that expects a refusal.
pub async fn try_upsert(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
    fields: Value,
) -> lakelands_api::Result<Outcome> {
    let (t, k) = (TypeName::from(ty), RecordKey::from(key));
    cat.upsert(who, &t, &k, patch(fields), WriteOpts::default())
        .await
}

/// A `get` of one record, with provenance, at `as_of` (the head when
/// `None`).
pub fn get_at(key: &str, as_of: Option<Position>) -> Get {
    Get {
        key: RecordKey::from(key),
        with_meta: true,
        at: read_at(as_of),
        params: Default::default(),
    }
}

/// A read at `as_of` (the head when `None`), with no valid time.
pub fn read_at(as_of: Option<Position>) -> ReadAt {
    ReadAt {
        as_of,
        valid_at: None,
        clock: None,
    }
}

/// A `find` of every record, with provenance, at `as_of`.
pub fn find_at(as_of: Option<Position>) -> Find {
    Find {
        with_meta: true,
        at: read_at(as_of),
        ..Find::default()
    }
}

/// Reads one record at the head, with provenance.
pub async fn read(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
) -> Result<Record, Failure> {
    let what = format!("reading `{ty}` `{key}`");
    let reply = cat
        .get(who, &TypeName::from(ty), get_at(key, None))
        .await
        .must(&what)?;
    Ok(reply.data)
}

/// Every record `find` answers for `ty` at the head, following cursors
/// until the last page.
pub async fn all(cat: &dyn Catalog, who: &Principal, ty: &str) -> Result<Vec<Record>, Failure> {
    all_found(cat, who, ty, Find::default()).await
}

/// Every record `find` answers for `ty` with this request, following
/// cursors until the last page (at most a thousand pages).
pub async fn all_found(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    mut find: Find,
) -> Result<Vec<Record>, Failure> {
    let mut out = Vec::new();
    for _ in 0..1000 {
        let reply = cat
            .find(who, &TypeName::from(ty), find.clone())
            .await
            .must(&format!("finding `{ty}` records"))?;
        match reply.data {
            Found::Records {
                rows, next_cursor, ..
            } => {
                out.extend(rows);
                match next_cursor {
                    Some(cursor) => find.cursor = Some(cursor),
                    None => return Ok(out),
                }
            }
            Found::Groups { .. } => {
                return Err(Failure::new(format!(
                    "finding `{ty}` records without an aggregate answered groups"
                )));
            }
        }
    }
    Err(Failure::new(format!(
        "finding `{ty}` records never reached a last page"
    )))
}

/// The value of `field` in `record`, if it holds one.
pub fn value<'r>(record: &'r Record, field: &str) -> Option<&'r Value> {
    record.fields.get(field)
}

/// A decimal value (a string of digits) as an exact number; `None` for
/// anything else.
pub fn decimal(v: Option<&Value>) -> Option<Decimal> {
    v.and_then(Value::as_str)
        .and_then(|s| Decimal::from_str(s).ok())
}

/// An exact decimal from a literal the check itself writes.
pub fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap_or_default()
}

/// One record from a source.
pub fn source_record(ty: &str, key: &str, fields: Value) -> SourceRecord {
    SourceRecord {
        source: None,
        upstream: None,
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        fields: patch(fields),
        seen_at: None,
    }
}

/// A source stream over records already in hand.
pub fn stream(records: Vec<SourceRecord>) -> SourceStream {
    Box::pin(Records(records.into_iter()))
}

struct Records(std::vec::IntoIter<SourceRecord>);

impl Stream for Records {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        Poll::Ready(self.0.next())
    }
}

/// Feeds `records` from `source` as `who`: as one ready stream, or with
/// those from `cut` on held back until the first window has answered, so
/// the store cuts there on a signal, not a clock (lakelands#886). Answers
/// the windows committed and the error that ended the stream, as is.
pub async fn fed(
    cat: &dyn Catalog,
    who: &Principal,
    source: &str,
    records: Vec<SourceRecord>,
    cut: Option<usize>,
) -> (Vec<Committed>, Option<lakelands_api::Error>) {
    let held = Arc::new(std::sync::Mutex::new((records.into_iter(), cut, None)));
    let stream = Box::pin(Cut(held.clone()));
    let mut commits = match cat.ingest(who, &SourceId::from(source), stream).await {
        Ok(c) => c,
        Err(e) => return (Vec::new(), Some(e)),
    };
    let mut out = Vec::new();
    while let Some(window) = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await {
        match window {
            Ok(c) => out.push(c),
            Err(e) => return (out, Some(e)),
        }
        let (_, cut, waker) = &mut *held.lock().expect("the cut");
        cut.take();
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }
    (out, None)
}

type Holding = (
    std::vec::IntoIter<SourceRecord>,
    Option<usize>,
    Option<std::task::Waker>,
);

/// Records in hand, all but the first `cut` held until it is released.
struct Cut(Arc<std::sync::Mutex<Holding>>);

impl Stream for Cut {
    type Item = SourceRecord;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        let (records, cut, waker) = &mut *self.0.lock().expect("the cut");
        if *cut == Some(0) {
            *waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        *cut = cut.map(|n| n - 1);
        Poll::Ready(records.next())
    }
}

/// Feeds `records` from `source` and drains the answer: every window's
/// acknowledgement, or the first error, whether `ingest` refused the
/// call or a window failed; a refused record's refusal, which the checks
/// judge, as [`lakelands_api::Error::unwrapped`] answers it (lakelands#1043).
pub async fn ingest(
    cat: &dyn Catalog,
    who: &Principal,
    source: &str,
    records: Vec<SourceRecord>,
) -> lakelands_api::Result<Vec<Committed>> {
    let fed = lakelands_api::ingest_all(cat, who, &SourceId::from(source), records).await;
    fed.map_err(|e| e.unwrapped().clone())
}

/// Fail unless the lake's head is still `before`: a refused write must
/// append nothing at all.
pub async fn head_unmoved(store: &dyn LakeStore, before: Position, what: &str) -> Held {
    let after = store.head().await.must("reading the head")?;
    if after == before {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "{what} was refused, yet the head moved from {before} to {after}: \
             a refused write must append nothing"
        )))
    }
}

/// Feeds `records` from `source` as `l`'s owner and checks that the
/// windows it answers reach the head; `what` names the delivery.
pub async fn delivered(
    store: &dyn LakeStore,
    l: &Lake,
    source: &str,
    records: Vec<SourceRecord>,
    what: &str,
) -> Result<Vec<Committed>, Failure> {
    let windows = l.ingest(source, records).await.must(what)?;
    windows_reach_head(store, &windows, what).await?;
    Ok(windows)
}

/// The error a refused `answer` holds, once the head is still `before`;
/// `what` names the attempt in either failure.
pub async fn refused_unmoved<T: Debug>(
    store: &dyn LakeStore,
    before: Position,
    what: &str,
    answer: lakelands_api::Result<T>,
) -> Result<Error, Failure> {
    let err = answer.must_fail(what)?;
    head_unmoved(store, before, what).await?;
    Ok(err)
}

/// Fail unless an ingest's windows answer positions in log order and the
/// last of them is the lake's head: everything the delivery committed is
/// visible from the position its last window names.
pub async fn windows_reach_head(store: &dyn LakeStore, windows: &[Committed], what: &str) -> Held {
    let head = store.head().await.must("reading the head")?;
    let Some(last) = windows.last() else {
        return Err(Failure::new(format!(
            "{what} answered no commit window at all"
        )));
    };
    if windows.windows(2).any(|w| w[1].position < w[0].position) {
        return Err(Failure::new(format!(
            "{what} answered windows out of log order: {windows:?}"
        )));
    }
    if last.position != head {
        return Err(Failure::new(format!(
            "{what} answered its last window at position {}, but the head is {head}",
            last.position
        )));
    }
    Ok(())
}

/// The records and duplicates a run of windows committed, summed.
pub fn committed(windows: &[Committed]) -> (u32, u32) {
    windows
        .iter()
        .fold((0, 0), |(r, d), w| (r + w.records, d + w.duplicates))
}

/// The position an outcome was recorded at, minus one: the last position
/// before the write.
pub fn before(outcome: &Outcome) -> Position {
    Position(outcome.position().0.saturating_sub(1))
}

/// One future of a check's concurrent callers.
pub type Caller<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Drive every caller to its end together, in the check's one task, and
/// answer their outputs in the callers' order. Each caller is polled
/// whenever the task wakes, so while one waits on the lake (the store
/// runs each write on its blocking pool, under its writer lock) the
/// others go on: the calls overlap at the store as the calls of many
/// clients do. No runtime is needed beyond the one the engine's test
/// brings.
pub async fn join_all<T>(callers: Vec<Caller<'_, T>>) -> Vec<T> {
    let mut pending: Vec<Option<Caller<'_, T>>> = callers.into_iter().map(Some).collect();
    let mut done: Vec<Option<T>> = pending.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut all = true;
        for (slot, out) in pending.iter_mut().zip(done.iter_mut()) {
            if let Some(f) = slot {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => {
                        *out = Some(v);
                        *slot = None;
                    }
                    Poll::Pending => all = false,
                }
            }
        }
        if all { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
    done.into_iter()
        .map(|v| v.expect("every caller ended"))
        .collect()
}

/// Wait `d` without a runtime: a thread sleeps and wakes the task. For a
/// check that must let the lake run on its own for a while, as a slow
/// consumer does.
pub async fn pause(d: std::time::Duration) {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut started = false;
    std::future::poll_fn(|cx| {
        if done.load(std::sync::atomic::Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !started {
            started = true;
            let (done, waker) = (done.clone(), cx.waker().clone());
            std::thread::spawn(move || {
                std::thread::sleep(d);
                done.store(true, std::sync::atomic::Ordering::Release);
                waker.wake();
            });
        }
        Poll::Pending
    })
    .await
}
