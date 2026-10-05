//! A type made of another, a link type or a view, is shown only with it
//! (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#595).

use std::sync::Arc;

use lakelands_api::key::link_key;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::read::Get;
use lakelands_api::write::{ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{Access, Find, Grants, LakeStore, Principal, PrincipalKind};
use serde_json::json;

use super::links::{SEPT, lake, link_ok};
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, field, object_set_on, pattern, principal, step, trigger_on, view_of,
};

const LINKS: &str = "sys.link.invoice.approved_by";

check!(
    /// A type made of another is shown only with it.
    SHOWN_WITH_WHAT_IT_IS_MADE_OF = a_type_made_of_another_is_shown_only_with_it,
    "006, extended (lakelands#595): a link type is shown to a principal only where the type that declares its relation is, and a view only where its source is, whatever a grant on its own name says: to one kept from a type, `describe` names neither a link type of it, nor a set or a trigger on that, nor a view over either, a proposed trigger on the link type is not listed, and `describe_type`, `find`, `get` and `search` of the link type, a read of the view, a walk over the relation from the type, `link` and `unlink`, over a relation the type has or one it has not, answer as for a type that is not there; a step from the target by the inverse of a relation of that type, carried by links or by a reference field, answers as a relation that is not there; a principal who reads one field of the owner is shown them all, and one kept from the link type by its own grant is kept from the view over it and walks the edge"
);

/// A principal who writes the lake but for these grants by type.
fn writer(store: &dyn LakeStore, id: &str, types: &[(&str, Option<Access>, &[&str])]) -> Principal {
    let mut grants = Grants {
        default: Access::Write,
        review: true,
        ..Grants::default()
    };
    for (ty, default, read) in types {
        let fields = read.iter().map(|f| ((*f).into(), Access::Read)).collect();
        let held = TypeGrants {
            default: *default,
            fields,
        };
        grants.types.insert((*ty).into(), held);
    }
    principal(store, PrincipalKind::Agent, id, grants)
}

/// An answer in a word.
fn short<T>(answer: lakelands_api::Result<T>) -> &'static str {
    match answer {
        Ok(_) => "answers",
        Err(lakelands_api::Error::NotFound { .. }) => "not found",
        Err(lakelands_api::Error::Refused { .. }) => "refused",
        Err(_) => "another error",
    }
}

async fn a_type_made_of_another_is_shown_only_with_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    l.declare_as(object_set_on("dated", LINKS, "since = since"), "the set")
        .await?;
    l.declare_as(trigger_on("signed", LINKS, None), "the trigger")
        .await?;
    l.declare_json(
        json!({"declare": "type", "name": "approvals", "key": ["to"],
        "fields": [{"name": "to", "type": {"kind": "reference", "target": "partner"}},
            {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": LINKS, "group_by": ["to"],
            "measures": [{"name": "n", "expr": "count()"}]}}),
    )
    .await?;
    let n = vec![field("n", StorageKind::Number)];
    l.declare(view_of("counted", "invoice", &[], n, &[("n", "count()")]))
        .await?;
    // A relation carried by a reference field, with its inverse.
    l.declare_json(json!({"declare": "type", "name": "note",
        "fields": [{"name": "about", "type": {"kind": "reference", "target": "partner"}}],
        "relations": [{"name": "concerns", "target": "partner", "via": "about",
            "inverse": "noted"}]}))
        .await?;
    l.upsert("note", "n1", json!({"about": "p1"})).await?;
    let declarations = vec![trigger_on("countersigned", LINKS, None)];
    let proposed = ProposedWrite::Declare { declarations };
    let proposed = l.cat.propose(&l.own, proposed, "a trigger".into()).await;
    proposed.must("proposing a trigger on the link type")?;
    let none = Some(Access::None);
    let s = store.as_ref();
    // Kept from `invoice` and `note`; the same with a grant on the link
    // type and on the view over it; reading one field of `invoice`; and
    // kept from the link type by its own grant.
    let apart = writer(s, "apart", &[("invoice", none, &[]), ("note", none, &[])]);
    let read = Some(Access::Read);
    let given = [
        ("invoice", none, &[][..]),
        ("note", none, &[]),
        (LINKS, read, &[]),
        ("approvals", read, &[]),
    ];
    let given = writer(s, "given", &given);
    let some = writer(s, "some", &[("invoice", none, &["amount"])]);
    let closed = writer(s, "closed", &[(LINKS, none, &[])]);
    let (c, ty) = (l.cat.as_ref(), LINKS.into());
    let (invoice, relation) = ("invoice".into(), "approved_by".into());
    let mut told = Vec::new();
    for who in [&apart, &given, &some, &closed] {
        let lake = c.describe(who, Default::default()).await;
        let lake = json!(lake.must("describing the lake")?.data).to_string();
        let listed = c.proposals(who, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        let get = Get {
            key: link_key(&"i1".into(), &"p1".into()),
            with_meta: false,
            params: Default::default(),
            at: Default::default(),
        };
        let search = Search {
            ty: LINKS.into(),
            mode: SearchMode::lexical("x"),
            k: 1,
            cursor: None,
            filter: None,
            with_meta: false,
            at: Default::default(),
        };
        // A walk's answer; "no relation" where it is, to the letter, the
        // answer for a relation the type does not have.
        let walk = |from: &'static str, key: &'static str, by: &'static str| async move {
            let over = |by| {
                pattern(
                    from,
                    Some(key),
                    vec![step(by, Walk::Out)],
                    Projection::Graph,
                )
            };
            let ghost = c
                .query(who, over("ghost"))
                .await
                .err()
                .map(|e| e.to_string());
            match c.query(who, over(by)).await {
                Err(e @ lakelands_api::Error::Refused { .. })
                    if Some(e.to_string().replace(by, "ghost")) == ghost =>
                {
                    "no relation"
                }
                other => short(other),
            }
        };
        let read = |view: &str| {
            let view = view.into();
            async move { c.find(who, &view, Find::default()).await }
        };
        let link = |from: &str, by: &str| {
            let (from, by, patch) = (from.into(), by.into(), Default::default());
            async move {
                let opts = WriteOpts::default();
                c.link(who, &from, &"i2".into(), &by, &"p2".into(), patch, opts)
                    .await
            }
        };
        let (from, to, opts) = ("i2".into(), "p2".into(), WriteOpts::default());
        let doors = [
            short(c.describe_type(who, &ty, Default::default()).await),
            short(c.find(who, &ty, Find::default()).await),
            short(c.get(who, &ty, get).await),
            short(c.search(who, search).await),
            short(read("approvals").await),
            short(read("counted").await),
            walk("invoice", "i1", "approved_by").await,
            walk("partner", "p1", "approved").await,
            walk("partner", "p1", "noted").await,
            short(link("invoice", "approved_by").await),
            short(c.unlink(who, &invoice, &from, &relation, &to, opts).await),
            short(link("invoice", "nothing").await),
            short(link("nowhere", "approved_by").await),
        ];
        told.push(format!(
            "{}: named {}, the set {}, the trigger {}, the proposed one {}; {}",
            who.id,
            lake.contains(LINKS),
            lake.contains("\"dated\""),
            lake.contains("\"signed\""),
            listed.iter().any(|p| p.write.is_some()),
            doors.join(", "),
        ));
    }
    // describe_type, find, get and search of the link type; the view over
    // it and the one over `invoice`; the walk out, and from the target by
    // the inverse of the relation carried by links and of the one carried
    // by a reference field; link, unlink, link over no relation and from
    // no type.
    let gone = |n| ["not found"; 7][..n].join(", ");
    let hidden = format!(
        "named false, the set false, the trigger false, the proposed one false; {}, no relation, \
         no relation, {}",
        gone(7),
        gone(4)
    );
    let want = [
        format!("apart: {hidden}"),
        format!("given: {hidden}"),
        format!(
            "some: named true, the set true, the trigger true, the proposed one true; {}, \
             refused, not found",
            ["answers"; 11].join(", ")
        ),
        "closed: named false, the set false, the trigger false, the proposed one false; not \
         found, not found, not found, not found, not found, answers, answers, answers, answers, \
         refused, refused, refused, not found"
            .to_owned(),
    ];
    ensure!(
        told == want,
        "a type made of another is shown with it alone; the four are told:\n{}",
        told.join("\n")
    );
    Ok(())
}
