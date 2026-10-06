//! A type made of another, a link type or a view, is shown only with it
//! (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#595).

use std::sync::Arc;

use lakelands_api::key::{UNGROUPED, link_key};
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::read::{AggFunc, Get};
use lakelands_api::write::{ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{Access, Catalog, Find, Grants, LakeStore, Principal, PrincipalKind, TypeName};
use serde_json::json;

use super::links::{SEPT, lake, link_ok};
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, field, get_at, object_set_on, patch, pattern, principal, step,
    trigger_on, view_of,
};

const LINKS: &str = "sys.link.invoice.approved_by";

check!(
    /// A type made of another is shown only with it.
    SHOWN_WITH_WHAT_IT_IS_MADE_OF = a_type_made_of_another_is_shown_only_with_it,
    "006, extended (lakelands#595): a link type is shown to a principal only where the type that declares its relation is, and a view only where its source is, whatever a grant on its own name says: to one kept from a type, `describe` names neither a link type of it, nor a set or a trigger on that, nor a view over either, a proposed trigger on the link type is not listed, and `describe_type`, `find`, `get` and `search` of the link type, a read of the view, a walk over the relation from the type, `link` and `unlink`, over a relation the type has or one it has not, answer as for a type that is not there; a step from the target by the inverse of a relation of that type, carried by links or by a reference field, answers as a relation that is not there; a principal who reads one field of the owner is shown them all, and one kept from the link type by its own grant is kept from the view over it and walks the edge"
);

check!(
    /// A type kept from its reader is kept at every door, with what is made of it.
    KEPT_AT_EVERY_DOOR = a_type_kept_from_its_reader_is_kept_at_every_door,
    "006, extended (lakelands#636, lakelands#635): to one whose grants give it nothing of a type that declares a sensitive field, the type, the link type of its relation and each view made of it (a count that reads no field, a grouping and a sum that read its fields, a view over that view, and a view that merges it beside a type the reader is shown) answer `describe_type`, `get` of a key that is there and of one that is not, `find`, with a total and with an aggregate, `search` and a pattern, and the type `upsert`, `retire` and `link`, value for value as a name that is not declared, and `describe` and the tools name none of them; so to one with no grant, to one granted to read each view and the link type by name, and to one who reads the whole lake but that type"
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

/// Each door that reads `ty`'s records, at `key` and at a key no record
/// holds, and on a type that is not made of another its writes: each
/// answer.
async fn doors(c: &dyn Catalog, who: &Principal, ty: &str, key: &str) -> Vec<String> {
    fn said<T>(r: lakelands_api::Result<T>) -> String {
        format!("{:?}", r.map(drop))
    }
    let (t, opts) = (TypeName::from(ty), WriteOpts::default);
    let total: Find = serde_json::from_value(json!({"total": true})).expect("a find");
    let search = json!({"type": ty, "mode": "lexical", "text": "a"});
    let search = serde_json::from_value(search).expect("a search");
    let from = pattern(ty, Some(key), Vec::new(), Projection::default());
    let mut said_to = vec![
        said(c.describe_type(who, &t, Default::default()).await),
        said(c.get(who, &t, get_at(key, None)).await),
        said(c.get(who, &t, get_at("none", None)).await),
        said(c.find(who, &t, Find::default()).await),
        said(c.find(who, &t, total).await),
        said((c.find(who, &t, aggregate(vec![agg("n", AggFunc::Count, None)]))).await),
        said(c.search(who, search).await),
        said(c.query(who, from).await),
    ];
    if !ty.contains(['.', '_']) {
        let (k, by, to) = (&key.into(), &"cites".into(), &"l2".into());
        let up = c.upsert(who, &t, k, patch(json!({"cat": "c"})), opts());
        said_to.push(said(up.await));
        said_to.push(said(c.retire(who, &t, k, opts()).await));
        let linked = c.link(who, &t, k, by, to, Default::default(), opts());
        said_to.push(said(linked.await));
    }
    said_to
}

async fn a_type_kept_from_its_reader_is_kept_at_every_door(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let (cat, n, amount, cap) = (
        f("cat", "text"),
        f("n", "decimal"),
        f("amount", "decimal"),
        f("cap", "decimal"),
    );
    let count = json!([{"name": "n", "expr": "count()"}]);
    let sum = |of: &str| json!([{"name": of, "expr": format!("sum({of})")}]);
    let mut secret = f("secret", "text");
    secret["sensitive"] = json!(true);
    // The kept type, a type beside it its readers are shown, a count over
    // the kept type, its sum by `cat`, a count over that sum, and a view
    // that merges the kept type into the shown one.
    for mut d in [
        json!({"name": "ledger", "fields": [cat, amount, secret],
            "relations": [{"name": "cites", "target": "ledger"}]}),
        json!({"name": "budget", "fields": [cat, cap]}),
        json!({"name": "ledger_count", "fields": [n],
            "view": {"source": "ledger", "measures": count}}),
        json!({"name": "ledger_by_cat", "key": ["cat"], "fields": [cat, amount],
            "view": {"source": "ledger", "group_by": ["cat"], "measures": sum("amount")}}),
        json!({"name": "ledger_cats", "fields": [n],
            "view": {"source": "ledger_by_cat", "measures": count}}),
        json!({"name": "ledger_and_budget", "key": ["cat"], "fields": [cat, cap, amount],
            "view": {"source": "budget", "group_by": ["cat"], "measures": sum("cap"),
                "merge": [{"source": "ledger", "group_by": ["cat"], "measures": sum("amount")}]}}),
    ] {
        d["declare"] = json!("type");
        l.declare_json(d).await?;
    }
    for (ty, k, fields) in [
        (
            "ledger",
            "l1",
            json!({"cat": "a", "amount": "1", "secret": "s"}),
        ),
        ("ledger", "l2", json!({"cat": "b", "amount": "2"})),
        ("budget", "b1", json!({"cat": "a", "cap": "9"})),
    ] {
        l.upsert(ty, k, fields).await?;
    }
    let (l1, l2, opts) = (&"l1".into(), &"l2".into(), WriteOpts::default());
    let (ledger, cites) = (&"ledger".into(), &"cites".into());
    let linked = l
        .cat
        .link(&l.own, ledger, l1, cites, l2, Default::default(), opts);
    linked.await.must("linking two ledger records")?;
    let link = link_key(l1, l2);
    let kept = [
        ("ledger", "l1"),
        ("sys.link.ledger.cites", link.as_str()),
        ("ledger_count", UNGROUPED),
        ("ledger_by_cat", "a"),
        ("ledger_cats", UNGROUPED),
        ("ledger_and_budget", "a"),
    ];
    // One given nothing; one given to read each view, the link type and
    // `budget` by name; one who reads the whole lake but `ledger`.
    let who = |id: &str, default, types: &[&str], access| {
        let held = TypeGrants {
            default: Some(access),
            fields: Default::default(),
        };
        let types = types.iter().map(|t| ((*t).into(), held.clone())).collect();
        let grants = Grants {
            default,
            types,
            ..Grants::default()
        };
        principal(store.as_ref(), PrincipalKind::Human, id, grants)
    };
    let mut given: Vec<&str> = kept[1..].iter().map(|k| k.0).collect();
    given.push("budget");
    let cast = [
        who("nothing", Access::None, &[], Access::None),
        who("given", Access::None, &given, Access::Read),
        who("rest", Access::Read, &["ledger"], Access::None),
    ];
    let (head, mut apart) = (store.head().await.must("the head")?, Vec::new());
    for who in &cast {
        for (ty, key) in kept {
            let ghost = ty.replace("ledger", "ghost");
            let there = doors(l.cat.as_ref(), who, ty, key).await;
            let none = doors(l.cat.as_ref(), who, &ghost, key).await;
            for (n, (a, b)) in there.iter().zip(none).enumerate() {
                if *a != b.replace(&ghost, ty) {
                    apart.push(format!(
                        "{} on `{ty}`, door {n}: {a}; on `{ghost}`: {b}",
                        who.id
                    ));
                }
            }
        }
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data;
        let names: Vec<_> = d.types.iter().map(|t| &t.name).collect();
        let said = format!("{names:?} {:?}", lakelands_api::tools::tools(&d));
        if said.contains("ledger") || (who.id != "nothing") != said.contains("budget") {
            apart.push(format!("{} is described {said}", who.id));
        }
    }
    ensure!(
        apart.is_empty() && store.head().await.must("the head")? == head,
        "a type kept from its reader, its link type and each view made of it answer as a name \
         that is not declared, and nothing is written; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}
