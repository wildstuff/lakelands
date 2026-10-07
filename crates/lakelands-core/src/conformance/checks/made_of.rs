//! A type made of another, a link type or a view, is shown only with it
//! (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#595).

use std::sync::Arc;

use lakelands_api::key::{UNGROUPED, link_key};
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::read::{AggFunc, Aggregate, Get, GroupKey, OrderBy};
use lakelands_api::write::{ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{
    Access, Catalog, FieldName, Find, Grants, LakeStore, Principal, PrincipalKind, TypeName,
};
use serde_json::json;

use super::links::{SEPT, lake, link_ok};
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, alike_renamed, field, field_access, get_at, object_set_on, order,
    patch, pattern, principal, step, trigger_on, view_of,
};

const LINKS: &str = "sys.link.invoice.approved_by";

check!(
    /// A reference to a type not shown is a field not known of.
    REFERENCE_TO_UNSHOWN = a_reference_to_a_type_its_reader_is_not_shown_is_a_field_it_may_not_know_of,
    "024 and 051, extended (lakelands#718): a reference to a type its reader is not shown is a field it may not know of, read, masked or by default alike: `describe_type`, `get` and `find` leave it out, a filter, an order, a grouping and an upsert on it answer word for word as on a field the type does not have; a type keyed by it, a view grouped by it and a type keyed by a reference to either answer every door as types that are not there; proposals are listed to a reviewer kept from the target without it, an upsert, a bundle and a migration alike, and a source is refused it; a value type's rename writing a use in a type keyed by it is not granted to that reviewer's preflight, naming nothing, and listed without the use or the set over it; a principal that may write every other field is not granted `retire`, and `may.retire` is false; a reference the reader's own `only` names stays, a reader shown the target by its type-level grant alone keeps it all, and a read as of a position judges the types as they stood"
);

check!(
    /// A type made of another is shown only with it.
    SHOWN_WITH_WHAT_IT_IS_MADE_OF = a_type_made_of_another_is_shown_only_with_it,
    "006, extended (lakelands#595): a link type is shown to a principal only where the type that declares its relation is, and a view only where its source is, whatever a grant on its own name says: to one kept from a type, `describe` names neither a link type of it, nor a set or a trigger on that, nor a view over either, a proposed trigger on the link type is not listed, and `describe_type`, `find`, `get` and `search` of the link type, a read of the view, a walk over the relation from the type, `link` and `unlink`, over a relation the type has or one it has not, answer as for a type that is not there; a link type is made of its relation's target too (lakelands#662), so to one kept from the target the walk over the relation, `link` and `unlink` answer as for a relation that is not there, and a link sent again answers the first; a step from the target by the inverse of a relation of that type, carried by links or by a reference field, answers as a relation that is not there; a principal who reads one field of the owner is shown them all, and one kept from the link type by its own grant is kept from the view over it and walks the edge"
);

check!(
    /// A type kept from its reader is kept at every door, with what is made of it.
    KEPT_AT_EVERY_DOOR = a_type_kept_from_its_reader_is_kept_at_every_door,
    "006, extended (lakelands#636, lakelands#635): to one whose grants give it nothing of a type that declares a sensitive field, the type, the link type of its relation and each view made of it (a count that reads no field, a grouping and a sum that read its fields, a view over that view, and a view that merges it beside a type the reader is shown) answer `describe_type`, `get` of a key that is there and of one that is not, `find`, with a total and with an aggregate, `search` and a pattern, and the type `upsert`, `retire` and `link`, value for value as a name that is not declared, and `describe` and the tools name none of them; so to one with no grant, to one granted to read each view and the link type by name, and to one who reads the whole lake but that type"
);

check!(
    /// A pattern names no type its reader is not shown.
    NAMES_NO_TYPE_NOT_SHOWN = a_pattern_names_no_type_its_reader_is_not_shown,
    "006, extended, and 024 (lakelands#653, lakelands#654): to one who reads the whole lake but a type, a pattern's order, sum and grouping over a path on that type answer value for value as over a type that is not declared, and a step from a type it is shown, in, either way or measured, over a relation the kept type derives under the name of one the reader is shown answers as before the kept type was declared, as does a step out of a type over its own relation, or either way (lakelands#662), where a type the reader is shown derives one of that name into it from a field the reader may not read"
);

check!(
    /// A type keyed by a field its reader may not know of is not shown.
    KEYED_BY_HIDDEN = a_type_keyed_by_a_field_its_reader_may_not_know_of_is_not_shown,
    "006 and 024, extended (lakelands#715): a type whose key names a field its principal may not know of is not shown to it, since every record key would hand out that field's value: to one who reads the whole lake but a key field held at `none`, the type, the link type of its relation and a view over it answer `describe_type`, `get`, `find`, `search`, a pattern, `upsert`, `retire` and `link` value for value as a name that is not declared, `describe` names none of them, and nothing is written; one who reads the key field masked is shown them all"
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
            ..Default::default()
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
    let away = writer(s, "away", &[("partner", none, &[])]);
    let (c, ty) = (l.cat.as_ref(), LINKS.into());
    let (invoice, relation) = ("invoice".into(), "approved_by".into());
    let mut told = Vec::new();
    for who in [&apart, &given, &some, &closed, &away] {
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
        // A walk's answer; "no relation" where it is the answer for none, alike.
        let walk = |from: &'static str, key: &'static str, by: &'static str| async move {
            let over = |by| {
                pattern(
                    from,
                    Some(key),
                    vec![step(by, Walk::Out)],
                    Projection::Graph,
                )
            };
            let ghost = c.query(who, over("ghost")).await.err();
            match c.query(who, over(by)).await {
                Err(e @ lakelands_api::Error::Refused { .. })
                    if ghost
                        .as_ref()
                        .is_some_and(|g| alike_renamed("", &e, g, ("ghost", by)).is_ok()) =>
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
        // "no relation" where it is, member for member, the answer for none.
        let ghost = link("invoice", "ghost").await.err();
        let renamed = ("ghost", "approved_by");
        let carried = |answer: lakelands_api::Result<_>| match answer {
            Err(e @ lakelands_api::Error::Refused { .. })
                if ghost
                    .as_ref()
                    .is_some_and(|g| alike_renamed("", &e, g, renamed).is_ok()) =>
            {
                "no relation"
            }
            other => short(other),
        };
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
            carried(link("invoice", "approved_by").await),
            carried(c.unlink(who, &invoice, &from, &relation, &to, opts).await),
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
        // Kept from the relation's target: the link type is made of it too.
        format!(
            "away: named false, the set false, the trigger false, the proposed one false; {}, \
             answers, no relation, not found, not found, no relation, no relation, refused, not \
             found",
            gone(5)
        ),
    ];
    ensure!(
        told == want,
        "a type made of another is shown with it alone; the five are told:\n{}",
        told.join("\n")
    );
    // A link sent again after a grant was taken away answers the first.
    let key = Some(lakelands_api::IdempotencyKey::from("again"));
    let again = |who: Principal| {
        let (opts, key) = (WriteOpts::default(), key.clone());
        let opts = WriteOpts {
            idempotency_key: key,
            ..opts
        };
        let (by, i2, p2) = (relation.clone(), "i2".into(), "p2".into());
        let ty = invoice.clone();
        async move {
            c.link(&who, &ty, &i2, &by, &p2, Default::default(), opts)
                .await
        }
    };
    let first = again(writer(s, "late", &[])).await.must("the first link")?;
    let sent = again(writer(s, "late", &[("partner", none, &[])])).await;
    // Its position and version; what it set off is named as to any reader.
    let at = |o: &lakelands_api::write::Outcome| match o {
        lakelands_api::write::Outcome::Applied {
            position, version, ..
        } => Some((*position, *version)),
        _ => None,
    };
    ensure!(
        at(&first).is_some() && sent.as_ref().ok().and_then(at) == at(&first),
        "a link sent again after its writer was kept from `partner` answers the first, \
         {first:?}; it answered {sent:?}"
    );
    Ok(())
}

/// Each door that reads `ty`'s records, at `key` and at a key no record
/// holds, and on a type that is not made of another its writes: each
/// answer.
async fn doors(
    c: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
) -> Vec<lakelands_api::Result<()>> {
    fn said<T>(r: lakelands_api::Result<T>) -> lakelands_api::Result<()> {
        r.map(drop)
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

/// `there`, to `what`, must be answered as `none`, at the same door of
/// the type named `ghost` for `ledger`: an error alike in every member.
fn answered_alike(
    what: &str,
    there: &lakelands_api::Result<()>,
    none: &lakelands_api::Result<()>,
) -> Result<(), Failure> {
    match (there, none) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => alike_renamed(what, a, b, ("ghost", "ledger")),
        _ => Err(Failure::new(format!(
            "{what}: {there:?}; under the name `ghost`: {none:?}"
        ))),
    }
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
            ..Default::default()
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
            for (n, (a, b)) in there.iter().zip(&none).enumerate() {
                let what = format!("{} on `{ty}`, door {n}", who.id);
                if let Err(f) = answered_alike(&what, a, b) {
                    apart.push(f.message);
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

async fn a_pattern_names_no_type_its_reader_is_not_shown(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let x = json!([{"name": "x", "type": {"kind": "text"}}]);
    let to_box = json!({"name": "to_box", "target": "box"});
    for d in [
        json!({"declare": "type", "name": "box", "key": ["x"], "fields": x}),
        json!({"declare": "type", "name": "item", "fields": x, "relations": [to_box]}),
    ] {
        l.declare_json(d).await?;
    }
    l.upsert("box", "b1", json!({"x": "b1"})).await?;
    l.upsert("item", "i1", json!({"x": "b1"})).await?;
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        fields: Default::default(),
        ..Default::default()
    };
    grants.types.insert("secret".into(), none);
    let fields = [(FieldName::from("x"), Access::None)].into();
    let held = TypeGrants {
        default: Some(Access::Read),
        fields,
        ..Default::default()
    };
    grants.types.insert("crate".into(), held);
    let who = &principal(store.as_ref(), PrincipalKind::Agent, "rest", grants);
    let c = l.cat.as_ref();
    // A step from `box` over `to_box`: in, either way, and measured; and
    // one out of `item` over its own.
    let walks = || async {
        let mut said = Vec::new();
        for (from, key, walk, project) in [
            ("box", "b1", Walk::In, Projection::default()),
            ("box", "b1", Walk::Both, Projection::default()),
            ("box", "b1", Walk::In, Projection::Degree),
            ("item", "i1", Walk::Out, Projection::default()),
            ("item", "i1", Walk::Both, Projection::default()),
        ] {
            let p = pattern(from, Some(key), vec![step("to_box", walk)], project);
            said.push(format!("{:?}", c.query(who, p).await.map(|r| r.data)));
        }
        said
    };
    let before = walks().await;
    let mut to_box = to_box;
    to_box["derived"] = json!({"match": [{"from": "x", "to": "x"}]});
    let secret = json!({"declare": "type", "name": "secret", "fields": x, "relations": [to_box]});
    l.declare_json(secret).await?;
    to_box["target"] = json!("item");
    let crate_ = json!({"declare": "type", "name": "crate", "fields": x, "relations": [to_box]});
    l.declare_json(crate_).await?;
    l.upsert("secret", "s1", json!({"x": "b1"})).await?;
    l.upsert("crate", "c1", json!({"x": "b1"})).await?;
    let mut apart = Vec::new();
    let after = walks().await;
    if after != before {
        apart.push(format!("the steps: {before:?}, then {after:?}"));
    }
    // An order, a sum and a grouping over `<ty>.x`, from `item`.
    let rows = |ty: &'static str| async move {
        let path = format!("{ty}.x");
        let mut p = pattern("item", None, Vec::new(), Projection::default());
        p.order_by = vec![order(&path, OrderBy::Value, Default::default())];
        let summed = Aggregate {
            measures: vec![agg("s", AggFunc::Sum, Some(&path))],
            group_by: Vec::new(),
        };
        let grouped = Aggregate {
            measures: vec![agg("n", AggFunc::Count, None)],
            group_by: vec![GroupKey::Field(path.as_str().into())],
        };
        let mut said = vec![format!("{:?}", c.query(who, p).await.map(|r| r.data))];
        for a in [summed, grouped] {
            let p = pattern("item", None, Vec::new(), Projection::Aggregate(a));
            said.push(format!("{:?}", c.query(who, p).await.map(|r| r.data)));
        }
        said
    };
    for (a, b) in rows("secret").await.into_iter().zip(rows("ghost").await) {
        if a.replace("secret", "ghost") != b {
            apart.push(format!("over `secret.x`: {a}; over `ghost.x`: {b}"));
        }
    }
    // Over `item.x`, which the reader is shown, the order and the grouping
    // are answered, so the two above are not alike for being refused alike
    // (lakelands#720); a sum of text is refused for itself.
    let shown = rows("item").await;
    if [&shown[0], &shown[2]].iter().any(|s| s.starts_with("Err")) {
        apart.push(format!("over `item.x`: {shown:?}"));
    }
    ensure!(
        apart.is_empty(),
        "a pattern names no type its reader is not shown; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}

async fn a_type_keyed_by_a_field_its_reader_may_not_know_of_is_not_shown(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let count = json!([{"name": "n", "expr": "count()"}]);
    for mut d in [
        json!({"name": "ledger", "key": ["code"], "fields": [f("code", "text"), f("cat", "text")],
            "relations": [{"name": "cites", "target": "ledger"}]}),
        json!({"name": "ledger_count", "fields": [f("n", "decimal")],
            "view": {"source": "ledger", "measures": count}}),
    ] {
        d["declare"] = json!("type");
        l.declare_json(d).await?;
    }
    for k in ["l1", "l2"] {
        l.upsert("ledger", k, json!({"code": k, "cat": "a"}))
            .await?;
    }
    let (l1, l2, opts) = (&"l1".into(), &"l2".into(), WriteOpts::default());
    let (ledger, cites) = (&"ledger".into(), &"cites".into());
    let linked = (l.cat).link(&l.own, ledger, l1, cites, l2, Default::default(), opts);
    linked.await.must("linking two ledger records")?;
    let link = link_key(l1, l2);
    let kept = [
        ("ledger", "l1"),
        ("sys.link.ledger.cites", link.as_str()),
        ("ledger_count", UNGROUPED),
    ];
    let keyed = |id: &str, access| field_access(store.as_ref(), id, "ledger", &[("code", access)]);
    let (hidden, masked) = (
        keyed("hidden", Access::None),
        keyed("masked", Access::Masked),
    );
    let (head, mut apart) = (store.head().await.must("the head")?, Vec::new());
    for (ty, key) in kept {
        let ghost = ty.replace("ledger", "ghost");
        let there = doors(l.cat.as_ref(), &hidden, ty, key).await;
        let none = doors(l.cat.as_ref(), &hidden, &ghost, key).await;
        for (n, (a, b)) in there.iter().zip(&none).enumerate() {
            if let Err(f) = answered_alike(&format!("on `{ty}`, door {n}"), a, b) {
                apart.push(f.message);
            }
        }
    }
    for who in [&hidden, &masked] {
        let d = l.cat.describe(who, Default::default()).await;
        let names: Vec<_> = d
            .must("describing the lake")?
            .data
            .types
            .into_iter()
            .map(|t| t.name)
            .collect();
        let shown = kept
            .iter()
            .filter(|k| names.iter().any(|n| n.as_str() == k.0))
            .count();
        if shown != if who.id == hidden.id { 0 } else { kept.len() } {
            apart.push(format!("{} is described {names:?}", who.id));
        }
    }
    ensure!(
        apart.is_empty() && store.head().await.must("the head")? == head,
        "a type keyed by a field its reader may not know of, its link type and a view over it \
         answer as a name that is not declared, and to one who reads the key masked as \
         declared; nothing is written; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}

async fn a_reference_to_a_type_its_reader_is_not_shown_is_a_field_it_may_not_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let to = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    // `slip` is keyed by its reference to `payroll`, `stub` by its reference
    // to `slip`, and `by_payee` groups `ticket` by its reference.
    let count = json!([{"name": "n", "expr": "count()"}]);
    for mut d in [
        json!({"name": "payroll", "fields": [f("code", "text")]}),
        json!({"name": "ticket", "fields": [f("no", "text"), to("payee", "payroll"), f("cat", "text")]}),
        json!({"name": "slip", "key": ["payee"], "fields": [to("payee", "payroll")]}),
        json!({"name": "stub", "key": ["of"], "fields": [to("of", "slip")]}),
        json!({"name": "by_payee", "key": ["payee"], "fields": [to("payee", "payroll"), f("n", "decimal")],
            "view": {"source": "ticket", "group_by": ["payee"], "measures": count}}),
        json!({"name": "fed", "fields": [{"name": "no", "type": {"kind": "text"}, "authority": "source"},
            {"name": "payee", "type": {"kind": "reference", "target": "payroll"}, "authority": "source"}]}),
    ] {
        d["declare"] = json!("type");
        l.declare_json(d).await?;
    }
    l.upsert("payroll", "p1", json!({"code": "c"})).await?;
    l.upsert(
        "ticket",
        "t1",
        json!({"no": "1", "payee": "p1", "cat": "a"}),
    )
    .await?;
    l.upsert("slip", "p1", json!({"payee": "p1"})).await?;
    l.upsert("stub", "p1", json!({"of": "p1"})).await?;
    let reader = |id: &str, payroll: TypeGrants, ticket: TypeGrants| {
        let mut grants = Grants {
            default: Access::Write,
            ..Grants::default()
        };
        grants
            .types
            .extend([("payroll".into(), payroll), ("ticket".into(), ticket)]);
        principal(store.as_ref(), PrincipalKind::Agent, id, grants)
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    let masked = TypeGrants {
        fields: [("payee".into(), Access::Masked)].into(),
        ..Default::default()
    };
    let only = TypeGrants {
        only: [("payee".into(), json!("p1"))].into(),
        ..Default::default()
    };
    let by_type = TypeGrants {
        default: Some(Access::Read),
        fields: [("code".into(), Access::None)].into(),
        ..Default::default()
    };
    let kept = reader("kept", none.clone(), TypeGrants::default());
    let mut apart = Vec::new();
    // `ticket` without `payee`, read or masked alike.
    for who in [&kept, &reader("masked", none.clone(), masked)] {
        let c = l.cat.as_ref();
        let t = TypeName::from("ticket");
        let described = c
            .describe_type(who, &t, Default::default())
            .await
            .must("describing `ticket`")?;
        let fields: Vec<String> = described
            .data
            .fields
            .iter()
            .map(|f| f.name.to_string())
            .collect();
        let got = c
            .get(who, &t, get_at("t1", None))
            .await
            .must("getting `t1`")?
            .data;
        let rows = c
            .find(who, &t, Find::default())
            .await
            .must("finding `ticket`")?;
        let found = format!("{:?}", rows.data);
        if fields != ["no", "cat"] || got.fields.contains_key("payee") || found.contains("payee") {
            apart.push(format!(
                "`{}` reads `ticket` as {fields:?}, `t1` as {:?}, and {found}",
                who.id, got.fields
            ));
        }
    }
    // Every door on `payee` as on `zz`, and nothing appended.
    let head = store.head().await.must("the head")?;
    let probes = |name: &str| {
        let (c, kept, t) = (l.cat.clone(), kept.clone(), TypeName::from("ticket"));
        let name = name.to_owned();
        async move {
            let find = |j: serde_json::Value| serde_json::from_value::<Find>(j).expect("a find");
            let asks = [
                find(json!({"where": format!("{name} = 'p1'")})),
                find(json!({"order_by": [{"field": name, "by": "value", "dir": "asc"}]})),
                find(
                    json!({"aggregate": {"measures": [{"name": "n", "func": "count"}], "group_by": [name]}}),
                ),
            ];
            let mut said = Vec::new();
            for ask in asks {
                said.push(format!("{:?}", c.find(&kept, &t, ask).await.map(drop)));
            }
            let t2 = lakelands_api::RecordKey::from("t2");
            let up = c.upsert(
                &kept,
                &t,
                &t2,
                patch(json!({name.as_str(): "p1"})),
                WriteOpts::default(),
            );
            said.push(format!("{:?}", up.await.map(drop)));
            said.join(" | ").replace(&name, "N")
        }
    };
    let (payee, zz) = (probes("payee").await, probes("zz").await);
    if payee != zz || store.head().await.must("the head")? != head {
        apart.push(format!("`payee` and `zz`: {payee} || {zz}"));
    }
    // A type keyed by it, a view grouped by it, and a type keyed by a
    // reference to the first answer as types that are not there.
    for (ty, key) in [("slip", "p1"), ("stub", "p1"), ("by_payee", "p1")] {
        let ghost = "ghost".to_owned();
        let there = doors(l.cat.as_ref(), &kept, ty, key).await;
        let none = doors(l.cat.as_ref(), &kept, &ghost, key).await;
        for (n, (a, b)) in there.iter().zip(&none).enumerate() {
            let what = format!("on `{ty}`, door {n}");
            let alike = match (a, b) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(a), Err(b)) => alike_renamed(&what, a, b, (&ghost, ty)),
                _ => Err(Failure::new(format!("{what}: {a:?}; on `{ghost}`: {b:?}"))),
            };
            if let Err(f) = alike {
                apart.push(f.message);
            }
        }
    }
    // A proposal is listed without it, and a source is refused it as a
    // field `fed` does not have.
    let upsert = |name: &str| ProposedWrite::Upsert {
        ty: "ticket".into(),
        key: "t3".into(),
        patch: patch(json!({"no": "3", name: "p1"})),
        opts: WriteOpts::default(),
    };
    // A bundle declaring a type that refers to `payroll`, and a migration
    // of `ticket`, listed as the reader is shown them.
    let bundle = json!({"declare": "type", "name": "pay", "fields": [f("no", "text"), to("payee", "payroll")]});
    let bundle: lakelands_api::write::Declaration = serde_json::from_value(bundle).expect("a type");
    let to_ticket = json!({"name": "ticket", "fields": [f("no", "text"), to("payee", "payroll"),
        f("cat", "text"), f("memo", "text")]});
    let to_ticket = serde_json::from_value(to_ticket).expect("a type");
    let migrate = ProposedWrite::Migrate {
        migration: Box::new(lakelands_api::write::Migration {
            to: to_ticket,
            renames: Vec::new(),
            relation_renames: Vec::new(),
        }),
        report: Default::default(),
    };
    for (write, why) in [
        (upsert("payee"), "an upsert"),
        (
            ProposedWrite::Declare {
                declarations: vec![bundle],
            },
            "a bundle",
        ),
        (migrate, "a migration"),
    ] {
        let proposed = l.cat.propose(&l.own, write, why.into()).await;
        proposed.must(&format!("proposing {why} naming `payee`"))?;
    }
    let mut reviewer = kept.clone();
    reviewer.grants.review = true;
    let listed = l.cat.proposals(&reviewer, ProposalFilter::default()).await;
    let listed = format!("{:?}", listed.must("listing the proposals")?.data);
    if listed.contains("payee") {
        apart.push(format!("the reviewer lists {listed}"));
    }
    let mut source = kept.clone();
    source.grants.ingest = true;
    let mut fed = Vec::new();
    for name in ["payee", "zz"] {
        let record =
            crate::conformance::fixture::source_record("fed", "f1", json!({"no": "1", name: "p1"}));
        let answer =
            crate::conformance::fixture::ingest(l.cat.as_ref(), &source, "src", vec![record]).await;
        fed.push(format!("{:?}", answer.map(drop)).replace(name, "N"));
    }
    if fed[0] != fed[1] {
        apart.push(format!("an ingest of `payee` and `zz`: {fed:?}"));
    }
    // A value type's rename writes `chit.note`, in a type keyed by `payee`
    // that the reviewer is not shown: its preflight is not granted, naming
    // nothing, and the proposal is listed without the use or the set over
    // `chit`, only telling it that one it is not shown may name a value.
    let state = json!({"declare": "value_type", "name": "state", "kind": "text", "one_of": ["open", "done"]});
    let note = json!({"name": "note", "type": {"kind": "text", "value_type": "state"}});
    let chit = json!({"declare": "type", "name": "chit", "key": ["payee"], "fields": [to("payee", "payroll"), note]});
    for d in [state, chit] {
        l.declare_json(d).await?;
    }
    let opened = object_set_on("opened", "chit", "note = 'open'");
    l.declare_as(opened, "declaring `opened`").await?;
    let state = json!({"name": "state", "kind": "text", "one_of": ["active", "done"]});
    let m = lakelands_api::write::ValueTypeMigration {
        to: serde_json::from_value(state).expect("a value type"),
        renames: vec![("open".into(), "active".into())],
    };
    reviewer.grants.schema = true;
    let told = l.cat.migrate_value_type(&reviewer, m.clone()).await;
    let refused = matches!(
        &told,
        Err(lakelands_api::Error::Refused {
            reason: lakelands_api::Refusal::NotGranted,
            field: None
        })
    );
    let rename = ProposedWrite::MigrateValueType {
        to: Box::new(m.to.clone()),
        renames: m.renames.clone(),
        report: Default::default(),
    };
    l.cat
        .propose(&l.own, rename, "a rename".into())
        .await
        .must("proposing the rename")?;
    let listed = l.cat.proposals(&reviewer, ProposalFilter::default()).await;
    let listed: Vec<_> = (listed.must("listing the proposals")?.data.into_iter())
        .filter_map(|p| match p.write {
            Some(ProposedWrite::MigrateValueType { report, .. }) => Some(*report),
            _ => None,
        })
        .collect();
    let hidden = |r: &lakelands_api::write::ValueTypeReport| {
        r.fields.is_empty() && r.conditions.is_empty() && r.conditions_unshown
    };
    if !refused || listed.len() != 1 || !listed.iter().all(hidden) {
        apart.push(format!("the rename: preflight {told:?}, listed {listed:?}"));
    }
    // To retire a record removes every field, a reference it may not know
    // of too: `kept` may not, and is told so (024).
    let t1 = lakelands_api::RecordKey::from("t1");
    let retired = l
        .cat
        .retire(&kept, &"ticket".into(), &t1, WriteOpts::default())
        .await;
    let described = l
        .cat
        .describe_type(&kept, &"ticket".into(), Default::default())
        .await;
    let may = described.must("describing `ticket`")?.data.may.retire;
    let refused = matches!(
        &retired,
        Err(lakelands_api::Error::Refused {
            reason: lakelands_api::Refusal::NotGranted,
            ..
        })
    );
    if !refused || may {
        apart.push(format!(
            "`kept` retiring `t1`: {retired:?}, `may.retire` {may}"
        ));
    }
    // Its own `only` keeps the reference, and the type-level grant keeps
    // it all.
    let held = reader("held", none, only);
    let typed = reader("typed", by_type, TypeGrants::default());
    for (who, ty, want) in [
        (&held, "ticket", true),
        (&held, "by_payee", true),
        (&typed, "ticket", true),
        (&typed, "slip", true),
    ] {
        let d = l
            .cat
            .describe_type(who, &ty.into(), Default::default())
            .await;
        let has = d
            .as_ref()
            .is_ok_and(|d| d.data.fields.iter().any(|f| f.name.as_str() == "payee"));
        if has != want {
            apart.push(format!(
                "`{}` describes `{ty}` as {:?}",
                who.id,
                d.map(|d| d.data.fields.len())
            ));
        }
    }
    let rows = l.cat.find(&held, &"ticket".into(), Find::default()).await;
    if !format!("{:?}", rows).contains("payee") {
        apart.push(format!("`held` finds `ticket` as {rows:?}"));
    }
    // A read is judged by the types as they stood: keyed by `code`, which
    // `late` may not know of, `boss` is not shown to it at the head, and
    // `memo.by` is there before.
    for mut d in [
        json!({"name": "boss", "fields": [f("code", "text")]}),
        json!({"name": "memo", "fields": [f("no", "text"), to("by", "boss")]}),
    ] {
        d["declare"] = json!("type");
        l.declare_json(d).await?;
    }
    let held = TypeGrants {
        default: Some(Access::Read),
        fields: [("code".into(), Access::None)].into(),
        ..Default::default()
    };
    let mut late = reader("late", TypeGrants::default(), TypeGrants::default());
    late.grants.types.insert("boss".into(), held);
    let before = store.head().await.must("the head")?;
    let code = json!({"name": "code", "type": {"kind": "text"}, "required": true});
    l.migrate(
        serde_json::from_value(json!({"name": "boss", "key": ["code"], "fields": [code]}))
            .expect("a type"),
    )
    .await?;
    let mut by = Vec::new();
    for at in [None, Some(before)] {
        let at = lakelands_api::ReadAt {
            as_of: at,
            valid_at: None,
        };
        let d = l
            .cat
            .describe_type(&late, &"memo".into(), at)
            .await
            .must("describing `memo`")?;
        by.push(d.data.fields.iter().any(|f| f.name.as_str() == "by"));
    }
    if by != [false, true] {
        apart.push(format!(
            "`late` reads `memo.by` at the head and before the key: {by:?}"
        ));
    }
    ensure!(
        apart.is_empty(),
        "a reference to a type not shown:\n{}",
        apart.join("\n")
    );
    Ok(())
}
