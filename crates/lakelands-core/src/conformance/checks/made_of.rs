//! A type made of another, a link type or a view, is shown only with it
//! (`docs/decisions/006-grants-not-roles.md`, extended, lakelands#595).

use lakelands_api::key::{UNGROUPED, link_key};
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::read::{AggFunc, Aggregate, Get, GroupKey, OrderBy};
use lakelands_api::write::{ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{
    Access, Catalog, Error, FieldName, Find, Grants, LakeStore, Missing, Principal, PrincipalKind,
    TypeName,
};
use serde_json::json;

use super::links::{SEPT, lake, link_ok};
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, alike_renamed, field, field_access, get_at, object_set_on, order,
    patch, pattern, principal, step, trigger_on, view_of,
};
use crate::conformance::{Failure, Held, Store};

const LINKS: &str = "sys.link.invoice.approved_by";

check!(
    REFERENCE_TO_UNSHOWN =
        a_reference_to_a_type_its_reader_is_not_shown_is_a_field_it_may_not_know_of,
    "024 and 051, extended (lakelands#718): a reference to a type its reader is not shown is a field it may not know of, read, masked or by default alike: `describe_type`, `get` and `find` leave it out, a filter, an order, a grouping and an upsert on it answer word for word as on a field the type does not have; a type keyed by it, a view grouped by it and a type keyed by a reference to either answer every door as types that are not there, and a verb on such a type is neither described nor made a tool (lakelands#770); proposals are listed to a reviewer kept from the target without it, an upsert, a bundle and a migration alike, and a source is refused it; a value type's rename writing a use in a type keyed by it is not granted to that reviewer's preflight, naming nothing, and listed without the use or the set over it; a principal that may write every other field is not granted `retire`, and `may.retire` is false; a reference the reader's own `only` names stays, a reader shown the target by its type-level grant alone keeps it all, and a read as of a position judges the types as they stood"
);

check!(
    SHOWN_WITH_WHAT_IT_IS_MADE_OF = a_type_made_of_another_is_shown_only_with_it,
    "006, extended (lakelands#595): a link type is shown to a principal only where the type that declares its relation is, and a view only where its source is, whatever a grant on its own name says: to one kept from a type, `describe` names neither a link type of it, nor a set or a trigger on that, nor a view over either, a proposed trigger on the link type is not listed, and `describe_type`, `find`, `get` and `search` of the link type, a read of the view, a walk over the relation from the type, `link` and `unlink`, over a relation the type has or one it has not, answer as for a type that is not there; a link type is made of its relation's target too (lakelands#662), so to one kept from the target the walk over the relation, `link` and `unlink` answer as for a relation that is not there, and a link sent again answers the first; a step from the target by the inverse of a relation of that type, carried by links or by a reference field, answers as a relation that is not there; a principal who reads one field of the owner is shown them all, and one kept from the link type by its own grant is kept from the view over it and walks the edge"
);

check!(
    KEPT_AT_EVERY_DOOR = a_type_kept_from_its_reader_is_kept_at_every_door,
    "006, extended (lakelands#636, lakelands#635): to one whose grants give it nothing of a type that declares a sensitive field, the type, the link type of its relation and each view made of it (a count that reads no field, a grouping and a sum that read its fields, a view over that view, and a view that merges it beside a type the reader is shown) answer `describe_type`, `get` of a key that is there and of one that is not, `find`, with a total and with an aggregate, `search` and a pattern, and the type `upsert`, `retire` and `link`, and a verb of each risk on the type `call` and `propose`, value for value as a name that is not declared, and `describe` and the tools name none of them; so to one with no grant, to one granted to read each view and the link type by name, and to one who reads the whole lake but that type; and the doors answer by what `describe` shows (lakelands#798): a type it names to a reader, and a verb on it, is at none of them a name that is not declared"
);

check!(
    NAMES_NO_TYPE_NOT_SHOWN = a_pattern_names_no_type_its_reader_is_not_shown,
    "006, extended, and 024 (lakelands#653, lakelands#654): to one who reads the whole lake but a type, a pattern's order, sum and grouping over a path on that type answer value for value as over a type that is not declared, and a step from a type it is shown, in, either way or measured, over a relation the kept type derives under the name of one the reader is shown answers as before the kept type was declared, as does a step out of a type over its own relation, or either way (lakelands#662), where a type the reader is shown derives one of that name into it from a field the reader may not read"
);

check!(
    VIEW_KEY_AT_SOURCES =
        a_view_keyed_by_a_field_its_reader_may_not_know_of_at_its_sources_is_not_shown,
    "006 and 024, extended (lakelands#782): a grouped view's key is its `group_by`, an ungrouped one has none, and a `key` that says otherwise is refused at declare (lakelands#788): one naming a measure, part of the groups or the groups in another order, and any `key` of an ungrouped view; each key field is judged on the view and where its values come from, its source's field and each merge source's by position: a view grouped by a reference to a type its reader is not shown, by a field its source holds at `none` for it, with `key` declared or not, one whose second source's group field it holds at `none`, one whose own group field it holds at `none`, and one grouped by two fields of which it holds one at `none`, answer `describe_type`, `get`, `find`, `search` and a pattern value for value as a name that is not declared, and `describe` names none of them; a view grouped by a field it reads is shown, so is one grouped so whose measure it holds at `none`, and an ungrouped view to a reader held from its measure, since no record of it is keyed by a field, and each of them, a grouped view with `key` or without and one shown without its definition too, hashes as its description does (lakelands#788), and such a grouped view is described, listed in a proposal and its get tool's key told by its groups, its definition naming no measure of a field it holds at `none`, and a measure that reads such a measure, its own or a merged one, answers in `describe`, `get`, `find`, a filter on it and the listing as a view declared without both, to a reader and to one who may write, who is offered no write to a view"
);

check!(
    KEYED_BY_HIDDEN = a_type_keyed_by_a_field_its_reader_may_not_know_of_is_not_shown,
    "006 and 024, extended (lakelands#715): a type whose key names a field its principal may not know of is not shown to it, since every record key would hand out that field's value: to one who reads the whole lake but a key field held at `none`, the type, the link type of its relation and a view over it answer `describe_type`, `get`, `find`, `search`, a pattern, `upsert`, `retire` and `link`, and a verb on the type `call` and `propose` (lakelands#770), value for value as a name that is not declared, `describe` names none of them, nor any verb on the type, in its verbs or its tools, though the reader writes every field the verb sets, and nothing is written; one who reads the key field masked is shown them all, the verbs too"
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

async fn a_type_made_of_another_is_shown_only_with_it(store: Store) -> Held {
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
        // A verb on the type, called and proposed: `<type>_ok` is low risk.
        for verb in [format!("{ty}_ok"), format!("{ty}_hold")] {
            let verb = verb.as_str().into();
            let (key, payload) = (Some(k.clone()), Default::default());
            let call = c.call(who, &verb, key.as_ref(), Default::default(), opts());
            said_to.push(said(call.await));
            let write = ProposedWrite::Call {
                verb,
                key,
                payload,
                opts: opts(),
            };
            said_to.push(said(c.propose(who, write, "why".into()).await));
        }
    }
    said_to
}

/// `there`, to `what`, must be answered as `none`, at the same door of
/// the type named `ghost` for `name`: an error alike in every member.
fn answered_alike(
    what: &str,
    there: &lakelands_api::Result<()>,
    none: &lakelands_api::Result<()>,
    name: &str,
) -> Held {
    match (there, none) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(a), Err(b)) => alike_renamed(what, a, b, ("ghost", name)),
        _ => Err(Failure::new(format!(
            "{what}: {there:?}; under the name `ghost`: {none:?}"
        ))),
    }
}

async fn a_type_kept_from_its_reader_is_kept_at_every_door(store: Store) -> Held {
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
        json!({"name": "budget", "fields": [cat, cap],
            "relations": [{"name": "cites", "target": "budget"}]}),
        json!({"name": "budget_count", "fields": [n],
            "view": {"source": "budget", "measures": count}}),
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
    // A verb of each risk on the kept type and on the shown one.
    for (on, verb, risk) in [("ledger", "ok", "low"), ("ledger", "hold", "high")] {
        for on in [on, "budget"] {
            let name = format!("{on}_{verb}");
            l.declare_json(
                json!({"declare": "verb", "name": name, "on": on, "risk": risk,
                "effect": "write", "set": [{"field": "cat", "value": "'b'"}]}),
            )
            .await?;
        }
    }
    for (ty, k, fields) in [
        (
            "ledger",
            "l1",
            json!({"cat": "a", "amount": "1", "secret": "s"}),
        ),
        ("ledger", "l2", json!({"cat": "b", "amount": "2"})),
        ("budget", "b1", json!({"cat": "a", "cap": "9"})),
        ("budget", "b2", json!({"cat": "b", "cap": "8"})),
    ] {
        l.upsert(ty, k, fields).await?;
    }
    let (l1, l2, opts) = (&"l1".into(), &"l2".into(), WriteOpts::default());
    let (ledger, cites) = (&"ledger".into(), &"cites".into());
    let linked = l
        .cat
        .link(&l.own, ledger, l1, cites, l2, Default::default(), opts);
    linked.await.must("linking two ledger records")?;
    let (budget, b1, b2, opts) = (
        &"budget".into(),
        &"b1".into(),
        &"b2".into(),
        WriteOpts::default(),
    );
    let linked = (l.cat).link(&l.own, budget, b1, cites, b2, Default::default(), opts);
    linked.await.must("linking two budget records")?;
    let (link, budgets) = (link_key(l1, l2), link_key(b1, b2));
    // A type its readers are shown, its link type and a view over it.
    let shown = [
        ("budget", "b1"),
        ("sys.link.budget.cites", budgets.as_str()),
        ("budget_count", UNGROUPED),
    ];
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
    given.extend(shown.iter().map(|k| k.0));
    let cast = [
        who("nothing", Access::None, &[], Access::None),
        who("given", Access::None, &given, Access::Read),
        who("rest", Access::Read, &["ledger"], Access::None),
    ];
    let (head, mut apart) = (store.head().await.must("the head")?, Vec::new());
    for who in &cast {
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data;
        let names: Vec<_> = d.types.iter().map(|t| t.name.as_str()).collect();
        let said = format!("{names:?} {:?}", lakelands_api::tools::tools(&d));
        if said.contains("ledger") || (who.id != "nothing") != said.contains("budget") {
            apart.push(format!("{} is described {said}", who.id));
        }
        // The grid: at every door a type `describe` does not name is a
        // name that is not declared, and one it names is at none.
        for (ty, key) in kept.into_iter().chain(shown) {
            let of = if ty.contains("ledger") {
                "ledger"
            } else {
                "budget"
            };
            let there = doors(l.cat.as_ref(), who, ty, key).await;
            let none = doors(l.cat.as_ref(), who, &ty.replace(of, "ghost"), key).await;
            for (n, (a, b)) in there.iter().zip(&none).enumerate() {
                let what = format!("{} on `{ty}`, door {n}", who.id);
                let absent = matches!(a, Err(Error::NotFound { missing }) if matches!(missing, Missing::Type { .. } | Missing::Verb { .. }));
                match (names.contains(&ty), answered_alike(&what, a, b, of)) {
                    (false, Err(f)) => apart.push(f.message),
                    (true, _) if absent => apart.push(format!("{what}, described: {a:?}")),
                    _ => {}
                }
            }
        }
    }
    ensure!(
        apart.is_empty() && store.head().await.must("the head")? == head,
        "a type kept from its reader, its link type and each view made of it answer as a name \
         that is not declared, one `describe` names answers at no door so, and nothing is \
         written; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}

async fn a_pattern_names_no_type_its_reader_is_not_shown(store: Store) -> Held {
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

async fn a_view_keyed_by_a_field_its_reader_may_not_know_of_at_its_sources_is_not_shown(
    store: Store,
) -> Held {
    use crate::conformance::fixture::invalid_said;
    use lakelands_api::error::{Case, Place, Said};
    let l = Lake::of(store.as_ref());
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let zref = json!({"name": "zref", "type": {"kind": "reference", "target": "zsecret"}});
    let count = json!([{"name": "n", "expr": "count()"}]);
    let view = |name: &str, source: &str, by: &str, field: &serde_json::Value, key: bool| {
        let mut v = json!({"declare": "type", "name": name, "fields": [field, f("n", "decimal")],
            "view": {"source": source, "group_by": [by], "measures": count}});
        if key {
            v["key"] = json!([by]);
        }
        v
    };
    // A view's `key` is its `group_by` or left out (lakelands#788): one
    // keyed by its measure, by part of its groups, by its groups in another
    // order, and an ungrouped one keyed at all, are refused at the type.
    let keyed = |name: &str, key: &[&str], by: &[&str]| {
        let mut v = view(name, "pub", "name", &f("name", "text"), false);
        let fields = by.iter().map(|g| f(g, "text")).chain([f("n", "decimal")]);
        v["fields"] = json!(fields.collect::<Vec<_>>());
        (v["key"], v["view"]["group_by"]) = (json!(key), json!(by));
        v
    };
    let total = keyed("u_view", &[], &[]);
    // Its measure reads `hid`, so eve is shown it without its definition.
    let mut unread = view("dh_view", "pub", "name", &f("name", "text"), false);
    unread["view"]["measures"] = json!([{"name": "n", "expr": "count(hid)"}]);
    let mut keyed_total = total.clone();
    keyed_total["key"] = json!(["n"]);
    let mut merged = view("m_view", "pub", "name", &f("name", "text"), false);
    merged["fields"] = json!([f("name", "text"), f("n", "decimal"), f("n2", "decimal")]);
    let n2 = json!([{"name": "n2", "expr": "count()"}]);
    merged["view"]["merge"] = json!([{"source": "aux", "group_by": ["hid2"], "measures": n2}]);
    // Grouped by `[name, hid]`, over a source whose `hid` it reads.
    let mut pair = view("knr_view", "pub2", "name", &f("name", "text"), false);
    pair["fields"] = json!([f("name", "text"), f("hid", "text"), f("n", "decimal")]);
    pair["view"]["group_by"] = json!(["name", "hid"]);
    // Measured from a measure: `m` reads `n`; `open`, and `due` merged
    // beside it, read the merged `paid`;
    // `m0_view` and `p0_view` are the same views without those measures.
    let measured = |name: &str, fields: &[&str], measures: serde_json::Value| {
        let mut v = view(name, "pub", "name", &f("name", "text"), false);
        let decimal = |n: &&str| f(n, "decimal");
        let fields = std::iter::once(f("name", "text")).chain(fields.iter().map(decimal));
        v["fields"] = json!(fields.collect::<Vec<_>>());
        v["view"]["measures"] = measures;
        v
    };
    let (n, paid) = (
        json!({"name": "n", "expr": "count()"}),
        json!({"name": "paid", "expr": "count()"}),
    );
    let mut bare = vec![
        measured(
            "mm_view",
            &["n", "m"],
            json!([n, {"name": "m", "expr": "n + 1"}]),
        ),
        measured("m0_view", &[], json!([])),
        measured(
            "pm_view",
            &["n", "paid", "open", "due"],
            json!([n, {"name": "open", "expr": "paid + 1"}]),
        ),
        measured("p0_view", &["n"], json!([n])),
    ];
    let due = json!({"name": "due", "expr": "count() + paid"});
    for (v, merged) in [(2, json!([paid, due])), (3, json!([]))] {
        bare[v]["view"]["merge"] =
            json!([{"source": "pub2", "group_by": ["name"], "measures": merged}]);
    }
    for d in [
        json!({"declare": "type", "name": "zsecret", "fields": [f("n", "text")]}),
        json!({"declare": "type", "name": "pub", "fields": [f("name", "text"), zref, f("hid", "text")]}),
        json!({"declare": "type", "name": "aux", "fields": [f("hid2", "text")]}),
        json!({"declare": "type", "name": "pub2", "fields": [f("name", "text"), f("hid", "text")]}),
        view("z_view", "pub", "zref", &zref, false),
        view("h_view", "pub", "hid", &f("hid", "text"), false),
        view("h_viewk", "pub", "hid", &f("hid", "text"), true),
        merged,
        view("g_view", "pub", "name", &f("name", "text"), false),
        view("kn_view", "pub", "name", &f("name", "text"), false),
        pair,
        total,
        unread,
        view("n_view", "pub", "name", &f("name", "text"), false),
        view("nh_view", "pub", "name", &f("name", "text"), true),
    ]
    .into_iter()
    .chain(bare.iter().cloned())
    {
        l.declare_json(d).await?;
    }
    l.upsert("zsecret", "z1", json!({"n": "x"})).await?;
    l.upsert("pub2", "q1", json!({"name": "a", "hid": "a"}))
        .await?;
    l.upsert("pub", "p1", json!({"name": "a", "zref": "z1", "hid": "a"}))
        .await?;
    l.upsert("aux", "x1", json!({"hid2": "a"})).await?;
    let grants = json!({"default": "read", "review": true, "types": {"zsecret": {"default": "none"},
        "pub": {"fields": {"hid": "none"}}, "aux": {"fields": {"hid2": "none"}}, "g_view": {"fields": {"name": "none"}},
        "kn_view": {"fields": {"n": "none"}}, "u_view": {"fields": {"n": "none"}},
        "knr_view": {"fields": {"hid": "none"}}, "nh_view": {"fields": {"n": "none"}},
        "mm_view": {"fields": {"n": "none"}}, "pm_view": {"fields": {"paid": "none"}}}});
    let eve = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "eve",
        serde_json::from_value(grants).expect("grants"),
    );
    let mut apart = Vec::new();
    for (ty, key) in [
        ("z_view", "z1"),
        ("h_view", "a"),
        ("h_viewk", "a"),
        ("m_view", "a"),
        ("g_view", "a"),
        ("knr_view", "a:a"),
    ] {
        let there = doors(l.cat.as_ref(), &eve, ty, key).await;
        let none = doors(l.cat.as_ref(), &eve, "no_such", key).await;
        let said = |a: &[lakelands_api::Result<()>]| {
            serde_json::to_string(&a.iter().map(|r| r.as_ref().err()).collect::<Vec<_>>())
                .expect("errors print")
        };
        if said(&there).replace(ty, "no_such") != said(&none) {
            apart.push(format!("`{ty}`: {there:?}"));
        }
    }
    let d = l.cat.describe(&eve, Default::default());
    let d = d.await.must("describing the lake")?.data;
    let measured_views = ["mm_view", "m0_view", "pm_view", "p0_view"];
    let names: Vec<&str> = d
        .types
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| n.contains("_view") && !measured_views.contains(n))
        .collect();
    if names != ["dh_view", "kn_view", "n_view", "nh_view", "u_view"] {
        apart.push(format!("eve is described {names:?}"));
    }
    // An ungrouped view is shown by what its one record carries, its key
    // under no field: eve, held from the `n` a `key` could once name, reads
    // it, described with no key.
    let total = doors(l.cat.as_ref(), &eve, "u_view", UNGROUPED).await;
    let keyless = d.types.iter().find(|t| t.name.as_str() == "u_view");
    let read = [0, 1, 3, 4, 5, 7].iter().all(|&door| total[door].is_ok());
    if !read || keyless.is_none_or(|t| !t.access.key.is_empty()) {
        apart.push(format!(
            "`u_view` answers eve {total:?}, described {keyless:?}"
        ));
    }
    for (bad, group_by) in [
        (keyed("bad_view", &["n"], &["name"]), Some(vec!["name"])),
        (
            keyed("bad_view", &["name"], &["name", "hid"]),
            Some(vec!["name", "hid"]),
        ),
        (
            keyed("bad_view", &["hid", "name"], &["name", "hid"]),
            Some(vec!["name", "hid"]),
        ),
        (keyed_total, None),
    ] {
        let name = TypeName::from(bad["name"].as_str().unwrap_or_default());
        let d = serde_json::from_value(bad.clone()).expect("a declaration reads");
        let refused = l.cat.declare(&l.own, d).await;
        let group_by = group_by.map(|g| g.into_iter().collect());
        let want = Said::from(Case::ViewKey { group_by }).within(Place::Type { name });
        if invalid_said(&refused) != Some(&want) {
            apart.push(format!("declaring {bad}: {refused:?}"));
        }
    }
    // Described, and in its tools, by the key its records carry.
    let tools = lakelands_api::tools::tools(&d).expect("eve's tools");
    for ty in ["kn_view", "n_view", "nh_view"] {
        let door = lakelands_api::tools::Door::Get { ty: ty.into() };
        let get = tools.iter().find(|t| t.door == door);
        let key = get.map(|t| &t.input_schema["properties"]["key"]["description"]);
        let access = d.types.iter().find(|t| t.name.as_str() == ty);
        let access = access.map(|t| t.access.key.iter().map(|f| f.as_str()).collect::<Vec<_>>());
        let text = format!("The key of the `{ty}` record: the value of its `name`.");
        if (&access, key) != (&Some(vec!["name"]), Some(&json!(text))) {
            apart.push(format!(
                "`{ty}` is described keyed {access:?}, its get tool {key:?}"
            ));
        }
    }
    // A shown view's definition names that key and no measure of a field
    // left out, described and listed in a redeclaration of `kn_view`.
    let shape = |key: &[FieldName], v: Option<&lakelands_api::ontology::ViewDecl>| {
        let measures = v.map(|v| v.measures.iter().map(|m| m.name.to_string()).collect());
        (
            key.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
            measures,
        )
    };
    let mut said: Vec<(Vec<String>, Option<Vec<String>>)> = Vec::new();
    for ty in ["kn_view", "n_view"] {
        let t = d.types.iter().find(|t| t.name.as_str() == ty);
        said.extend(t.map(|t| shape(&t.access.key, t.view.as_ref())));
    }
    let mut again = view("kn_view", "pub", "name", &f("name", "text"), false);
    again["description"] = json!("again");
    let declarations = vec![serde_json::from_value(again).expect("a declaration reads")];
    let bundle = ProposedWrite::Declare { declarations };
    let proposed = l.cat.propose(&l.own, bundle, "why".into()).await;
    proposed.must("proposing `kn_view` again")?;
    let listed = l.cat.proposals(&eve, ProposalFilter::default()).await;
    for p in listed.must("listing")?.data {
        let Some(ProposedWrite::Declare { declarations }) = p.write else {
            continue;
        };
        for d in declarations {
            if let lakelands_api::write::Declaration::Type(t) = d {
                said.push(shape(&t.key, t.view.as_deref()));
            }
        }
    }
    // And it is answered the hash of the definition it is shown.
    let field = |f: &lakelands_api::describe::FieldDescription| json!({"name": f.name, "type": f.ty, "required": f.required});
    // The hash is taken over the key the records carry, so a view with its
    // `key` left out hashes as its description, which names it: shown
    // whole (`n_view`), without a measure (`kn_view`), and without its
    // definition (`dh_view`); so does the ungrouped view, under no key.
    let hashed = ["nh_view", "n_view", "kn_view", "dh_view", "u_view"];
    for t in (d.types.iter()).filter(|t| hashed.contains(&t.name.as_str())) {
        let fields: Vec<_> = t.fields.iter().map(field).collect();
        let shown = json!({"name": t.name, "key": t.access.key, "fields": fields, "view": t.view});
        let shown = serde_json::from_value(shown).expect("a description reads as a declaration");
        let rebuilt = lakelands_api::shape::schema_hash(&shown).expect("a described view parses");
        let bare = t.name.as_str() == "dh_view";
        if rebuilt != t.lifecycle.schema_hash || bare != t.view.is_none() {
            apart.push(format!(
                "`{}` is described {:?}, hashed {}",
                t.name, t.view, t.lifecycle.schema_hash
            ));
        }
    }
    let one = |m: &[&str]| {
        (
            vec!["name".to_owned()],
            Some(m.iter().map(|m| m.to_string()).collect()),
        )
    };
    if said != [one(&[]), one(&["n"]), one(&[])] {
        apart.push(format!(
            "eve is shown `kn_view`, `n_view`, and `kn_view` proposed, keyed and measured {said:?}"
        ));
    }
    // `mm_view` and `pm_view` answer eve, and `wes` who may write, described,
    // at `get` and `find`, filtered on the measure, and listed, as the views
    // declared without the measures they may not know of.
    let grants = json!({"default": "write", "review": true, "types": {
        "mm_view": {"fields": {"n": "none"}}, "pm_view": {"fields": {"paid": "none"}}}});
    let wes = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "wes",
        serde_json::from_value(grants).expect("grants"),
    );
    let mut whole = std::collections::BTreeMap::new();
    for who in [&eve, &wes] {
        for t in ["mm_view", "m0_view", "pm_view", "p0_view"] {
            let ty = TypeName::from(t);
            let described =
                (l.cat.describe_type(who, &ty, Default::default()).await).map(|d| d.data);
            // A view is never written: no reader is offered `upsert` or `retire`.
            if let Ok(d) = &described
                && (d.may.upsert || d.may.retire)
            {
                apart.push(format!(
                    "`{t}` is offered writes to {}: {:?}",
                    who.id, d.may
                ));
            }
            let got = (l.cat.get(who, &ty, get_at("a", None)).await).map(|g| g.data);
            let found = (l.cat.find(who, &ty, Find::default()).await).map(|f| f.data);
            // Filtered on the measure of its pair: `m`, or `open` and `due`.
            let on = if t.starts_with('m') {
                "m > 2"
            } else {
                "open > 2 or due > 2"
            };
            let on: Find = serde_json::from_value(json!({"where": on})).expect("a find");
            let filtered = (l.cat.find(who, &ty, on).await).map(|f| f.data);
            let said = format!("{described:?} {got:?} {found:?} {filtered:?}");
            whole.insert((who.id.to_string(), t.to_owned()), said);
        }
    }
    let again = bare.into_iter().map(|mut d| {
        d["description"] = json!("again");
        serde_json::from_value(d).expect("a declaration reads")
    });
    let bundle = ProposedWrite::Declare {
        declarations: again.collect(),
    };
    let proposed = l.cat.propose(&l.own, bundle, "why".into()).await;
    proposed.must("proposing the measured views again")?;
    for who in [&eve, &wes] {
        for p in l
            .cat
            .proposals(who, ProposalFilter::default())
            .await
            .must("listing")?
            .data
        {
            let Some(ProposedWrite::Declare { declarations }) = p.write else {
                continue;
            };
            for d in declarations {
                if let lakelands_api::write::Declaration::Type(t) = d
                    && let Some(w) = whole.get_mut(&(who.id.to_string(), t.name.to_string()))
                {
                    w.push_str(&serde_json::to_string(&t).expect("a declaration prints"));
                }
            }
        }
    }
    for ((who, ty), there) in &whole {
        let Some(bare) = [("mm_view", "m0_view"), ("pm_view", "p0_view")]
            .into_iter()
            .find_map(|(t, b)| (t == ty).then_some(b))
        else {
            continue;
        };
        let without = &whole[&(who.clone(), bare.to_owned())];
        if *there != without.replace(bare, ty.as_str()) {
            apart.push(format!(
                "`{ty}` answers {who}\n{there}\nwhere `{bare}` answers\n{without}"
            ));
        }
    }
    ensure!(
        apart.is_empty(),
        "a view keyed by a field its reader may not know of, on the view or where its values come from, answers as a name that is not declared:\n{}",
        apart.join("\n")
    );
    Ok(())
}

async fn a_type_keyed_by_a_field_its_reader_may_not_know_of_is_not_shown(store: Store) -> Held {
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
    for (verb, risk) in [("ledger_ok", "low"), ("ledger_hold", "high")] {
        l.declare_json(
            json!({"declare": "verb", "name": verb, "on": "ledger", "risk": risk,
            "effect": "write", "set": [{"field": "cat", "value": "'b'"}]}),
        )
        .await?;
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
    // Each writes `cat`, so each may call the verbs on a type it is shown.
    let keyed = |id: &str, access| {
        let mut who = field_access(store.as_ref(), id, "ledger", &[("code", access)]);
        who.grants.default = Access::Write;
        who
    };
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
            if let Err(f) = answered_alike(&format!("on `{ty}`, door {n}"), a, b, "ledger") {
                apart.push(f.message);
            }
        }
    }
    for who in [&hidden, &masked] {
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data;
        let names: Vec<_> = d.types.iter().map(|t| &t.name).collect();
        let shown = kept
            .iter()
            .filter(|k| names.iter().any(|n| n.as_str() == k.0))
            .count();
        // Its verbs, and the tools made of them, name the type with it.
        let verbs: Vec<_> = d.verbs.iter().map(|v| v.name.as_str()).collect();
        let tools = format!("{:?}", lakelands_api::tools::tools(&d));
        let (all, n) = (who.id != hidden.id, usize::from(who.id != hidden.id));
        if shown != n * kept.len() || verbs.len() != 2 * n || tools.contains("ledger_ok") != all {
            apart.push(format!("{} is described {names:?}, {verbs:?}", who.id));
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
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let f = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}});
    let to = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    // `slip` is keyed by its reference to `payroll`, `stub` by its reference
    // to `slip`, and `by_payee` groups `ticket` by its reference.
    let count = json!([{"name": "n", "expr": "count()"}]);
    for mut d in [
        json!({"name": "payroll", "fields": [f("code", "text")]}),
        json!({"name": "ticket", "fields": [f("no", "text"), to("payee", "payroll"), f("cat", "text")]}),
        json!({"name": "slip", "key": ["payee"], "fields": [to("payee", "payroll"), f("n", "text")]}),
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
    for (verb, risk) in [("slip_ok", "low"), ("slip_hold", "high")] {
        l.declare_json(
            json!({"declare": "verb", "name": verb, "on": "slip", "risk": risk,
            "effect": "write", "set": [{"field": "n", "value": "'b'"}]}),
        )
        .await?;
    }
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
        let described = c.describe_type(who, &t, Default::default());
        let described = described.await.must("describing `ticket`")?;
        let fields: Vec<String> = described
            .data
            .fields
            .iter()
            .map(|f| f.name.to_string())
            .collect();
        let got = c.get(who, &t, get_at("t1", None));
        let got = got.await.must("getting `t1`")?.data;
        let rows = c.find(who, &t, Find::default());
        let rows = rows.await.must("finding `ticket`")?;
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
    let fut = l.cat.propose(&l.own, rename, "a rename".into());
    fut.await.must("proposing the rename")?;
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
    // A verb on `slip` is described, and made a tool, only where `slip` is.
    for (who, want) in [(&kept, 0), (&typed, 2)] {
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data;
        let on = d.verbs.iter().filter(|v| v.on.as_str() == "slip").count();
        let tools = format!("{:?}", lakelands_api::tools::tools(&d));
        if on != want || tools.contains("slip_ok") != (want > 0) {
            apart.push(format!("`{}` is described {on} verbs on `slip`", who.id));
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
