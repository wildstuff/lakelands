//! A migration renames a relation and keeps its edges
//! (`docs/decisions/035-a-link-is-a-record.md`, extended, lakelands#429).

use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::{
    Authority, Cardinality, Derived, MatchPair, RelationDecl, StorageKind,
};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Edge, Projection, QueryResult, Search, SearchMode, Walk};
use lakelands_api::read::Found;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{
    Carried, Carrier, Migration, Outcome, ProposedWrite, RelationRename, WriteOpts,
};
use lakelands_api::{Access, Error, Find, Grants, LakeStore, Position, Principal, PrincipalKind};
use serde_json::json;

use super::links::{SEPT, dated, invoice, lake, link_ok, pairs};
use crate::conformance::fixture::{
    Lake, Must, derived, events_after, field, find_at, followed, get_at, head_unmoved,
    invalid_case, invalid_like, object_set_on, patch, pattern, principal, reference, source_record,
    sourced, step, trigger_on, type_decl, vector,
};
use crate::conformance::{Failure, Held, Store};

const OLD: &str = "sys.link.invoice.approved_by";
const NEW: &str = "sys.link.invoice.signed_by";

/// The migration that renames `approved_by` to `signed_by`.
fn renaming() -> Migration {
    let mut signed = dated();
    signed.name = "signed_by".into();
    Migration {
        relation_renames: vec![("approved_by".into(), "signed_by".into())],
        ..Migration::to(invoice(vec![signed]))
    }
}

/// The owner applies `m` after its preflight.
async fn apply(l: &Lake, m: Migration) -> Held {
    let report = l.cat.migrate(&l.own, m.clone()).await;
    let id = report.must("the preflight")?.data.report_id;
    let applied = l.cat.migrate_apply(&l.own, m, &id).await;
    applied.must("applying the migration").map(drop)
}

check!(
    RENAMED_KEEPS_EDGES = a_migration_renames_a_relation_and_keeps_its_edges,
    "035, extended (lakelands#429, lakelands#678): a migration that names a relation's rename keeps its edges: the report lists the relation with its carrier and the link records that move, and no drop; the apply appends the `migrated` and the new link type's declaration and no event for a link; a walk by the new name answers the edges with their fields, a walk as of before answers them by the old name, and the links are records of the new link type; as of the `migrated` itself the walk answers them already, and the new link type is found from its declaration, one position later, on; a consumer of the old link type and one of the new each hear of the `migrated`, and every filtered poll answers what the log does"
);

/// The edges a walk over `relation` from invoice `key` answers.
async fn walked(
    l: &Lake,
    relation: &str,
    key: &str,
    as_of: Option<Position>,
) -> lakelands_api::Result<Vec<Edge>> {
    let steps = vec![step(relation, Walk::Out)];
    let mut p = pattern("invoice", Some(key), steps, Projection::Graph);
    p.at.as_of = as_of;
    match l.cat.query(&l.own, p).await?.data {
        QueryResult::Graph { edges, .. } => Ok(edges),
        _ => Ok(Vec::new()),
    }
}

async fn a_migration_renames_a_relation_and_keeps_its_edges(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    link_ok(&l, "i2", "p2", json!({})).await?;
    let before = store.head().await.must("reading the head")?;
    let m = renaming();
    let report = l.cat.migrate(&l.own, m.clone()).await;
    let report = report.must("the preflight of the rename")?.data;
    let listed = RelationRename {
        from: "approved_by".into(),
        to: "signed_by".into(),
        carrier: Carrier::Links,
        links: 2,
    };
    ensure!(
        report.relations == [listed] && report.dropped.is_empty() && report.lossless,
        "the report lists the renamed relation with its two links and no drop; it is {report:?}"
    );
    let applied = l.cat.migrate_apply(&l.own, m, &report.report_id).await;
    applied.must("applying the rename")?;
    let written = events_after(store.as_ref(), before).await?;
    let kinds: Vec<&str> = written.iter().map(|e| e.body.kind()).collect();
    // As of the `migrated` itself the links have moved and are walked; the
    // new link type is declared one position later, and not found till then.
    let at = Some(written[0].position);
    let moved = walked(&l, "signed_by", "i1", at).await.must("walking")?;
    let undeclared = seen(&l, &l.own, NEW, at).await;
    ensure!(
        pairs(&moved) == ["i1>p1"] && undeclared == "not found",
        "as of the `migrated` a walk by the new name answers {:?} and a `find` of the new \
         link type {undeclared}",
        pairs(&moved)
    );
    let now = walked(&l, "signed_by", "i1", None).await.must("walking")?;
    let then = walked(&l, "approved_by", "i1", Some(before)).await;
    let then = then.must("walking as of before")?;
    let since = now.first().and_then(|e| e.fields.get("since").cloned());
    let (links, old) = (
        l.all_found(NEW, Find::default()).await?.len(),
        l.all_found(OLD, Find::default()).await?.len(),
    );
    ensure!(
        kinds == ["migrated", "declared"]
            && pairs(&now) == ["i1>p1"]
            && pairs(&then) == ["i1>p1"]
            && since == Some(json!(SEPT))
            && (links, old) == (2, 0),
        "the apply appends {kinds:?}; `signed_by` walks {:?} with `since` {since:?}, \
         `approved_by` as of before walks {:?}; the new link type holds {links} records \
         and the old {old}",
        pairs(&now),
        pairs(&then)
    );
    // A consumer of either link type hears of the move (lakelands#678).
    for ty in [OLD, NEW] {
        let only = FeedFilter {
            types: vec![ty.into()],
            ..FeedFilter::default()
        };
        let heard = followed(store.as_ref(), Position::GENESIS, &only).await?;
        let migrated = |e: &&Event| matches!(e.body, EventBody::Migrated { .. });
        ensure!(
            heard.iter().filter(migrated).count() == 1,
            "a consumer of `{ty}` hears of the rename that moves its links: it read the kinds \
             {:?}",
            heard.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
        );
    }
    super::feed::the_index_answers_the_log(store.as_ref(), "a relation renamed").await
}

check!(
    RENAMED_GRANT_FOLLOWS = a_grant_on_a_link_type_follows_its_relations_rename,
    "006, extended (lakelands#429): a grant on a link type holds its records through the relation's rename, in both directions of time and only to take away: a principal kept from the old link type by a default alone is kept from the new one, by `find` and in a walk's link fields; one kept from the new name is kept from the old one as of before; a permission on the old name gives nothing under the new; and a field granted `write` under the name borne with a default of none under the earlier name is none"
);

/// An agent `id` with `lake` for every type and `default` and `fields` on
/// the link type `ty`.
fn granted(
    store: &dyn LakeStore,
    id: &str,
    lake: Access,
    (ty, default, fields): (&str, Option<Access>, &[(&str, Access)]),
) -> Principal {
    let fields = fields.iter().map(|(f, a)| ((*f).into(), *a)).collect();
    let mut grants = Grants {
        default: lake,
        ..Grants::default()
    };
    grants.types.insert(
        ty.into(),
        TypeGrants {
            default,
            fields,
            ..Default::default()
        },
    );
    principal(store, PrincipalKind::Agent, id, grants)
}

/// What `who` is answered of the link type `ty` as of `as_of`: how many
/// links, and whether the first shows `since`; or that there is no such type.
async fn seen(l: &Lake, who: &Principal, ty: &str, as_of: Option<Position>) -> String {
    match l.cat.find(who, &ty.into(), find_at(as_of)).await {
        Ok(reply) => match reply.data {
            Found::Records { rows, .. } => {
                let since = rows.first().is_some_and(|r| r.fields.contains_key("since"));
                format!("{} with since {since}", rows.len())
            }
            _ => "something else".into(),
        },
        Err(Error::NotFound { .. }) => "not found".into(),
        Err(e) => format!("{e:?}"),
    }
}

async fn a_grant_on_a_link_type_follows_its_relations_rename(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    let before = store.head().await.must("reading the head")?;
    apply(&l, renaming()).await?;
    let s = store.as_ref();
    let none = Some(Access::None);
    let (kept, later, permitted, both) = (
        granted(s, "kept", Access::Read, (OLD, none, &[])),
        granted(s, "later", Access::Read, (NEW, none, &[])),
        granted(s, "permitted", Access::None, (OLD, Some(Access::Read), &[])),
        {
            let mut both = granted(
                s,
                "both",
                Access::Read,
                (NEW, None, &[("since", Access::Write)]),
            );
            let old = TypeGrants {
                default: none,
                ..TypeGrants::default()
            };
            both.grants.types.insert(OLD.into(), old);
            both
        },
    );
    // A link type is shown with its owner and its target (006, extended).
    let mut permitted = permitted;
    let amount = [("amount".into(), Access::Read)].into();
    let of_owner = TypeGrants {
        default: None,
        fields: amount,
        ..Default::default()
    };
    permitted.grants.types.insert("invoice".into(), of_owner);
    let partner = TypeGrants {
        default: Some(Access::Read),
        ..Default::default()
    };
    permitted.grants.types.insert("partner".into(), partner);
    let mut red = Vec::new();
    for (what, who, ty, as_of, want) in [
        (
            "a default of none on the old name, at the new",
            &kept,
            NEW,
            None,
            "not found",
        ),
        (
            "the same, as of before, at the old",
            &kept,
            OLD,
            Some(before),
            "not found",
        ),
        (
            "a default of none on the new name, as of before",
            &later,
            OLD,
            Some(before),
            "not found",
        ),
        (
            "a permission on the old name, at the new",
            &permitted,
            NEW,
            None,
            "not found",
        ),
        (
            "the same, as of before, at the old",
            &permitted,
            OLD,
            Some(before),
            "1 with since true",
        ),
        (
            "`write` on a field with none under the earlier name",
            &both,
            NEW,
            None,
            "not found",
        ),
        ("the owner", &l.own, NEW, None, "1 with since true"),
    ] {
        let got = seen(&l, who, ty, as_of).await;
        if got != want {
            red.push(format!("{what}: {got}, not {want}"));
        }
    }
    // The other doors of one kept by a default alone: `get`, and the link
    // doors of one who writes everything else.
    let got = l.cat.get(&kept, &NEW.into(), get_at("i1:p1", None)).await;
    let writer = granted(s, "writer", Access::Write, (OLD, none, &[]));
    let (ty, relation) = ("invoice".into(), "signed_by".into());
    let (i1, i2, p1, p2) = ("i1".into(), "i2".into(), "p1".into(), "p2".into());
    let (fields, opts) = (patch(json!({})), WriteOpts::default);
    let linked = (l.cat).link(&writer, &ty, &i2, &relation, &p2, fields, opts());
    let linked = linked.await;
    let unlinked = (l.cat).unlink(&writer, &ty, &i1, &relation, &p1, opts());
    let unlinked = unlinked.await;
    if !matches!(got, Err(Error::NotFound { .. })) || linked.is_ok() || unlinked.is_ok() {
        red.push(format!("get {got:?}, link {linked:?}, unlink {unlinked:?}"));
    }
    // A walk reaches the edge by its ends; its link fields are the link type's.
    let steps = vec![step("signed_by", Walk::Out)];
    let p = pattern("invoice", Some("i1"), steps, Projection::Graph);
    let walked = l.cat.query(&kept, p).await.must("the walk")?.data;
    let QueryResult::Graph { edges, .. } = walked else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let shown = edges.first().map(|e| e.fields.contains_key("since"));
    ensure!(
        red.is_empty() && shown == Some(false),
        "a grant on a link type follows its rename: {}; the walk of one kept from the link \
         type shows `since`: {shown:?}",
        red.join("; ")
    );
    Ok(())
}

/// The owner links invoice `from` to partner `to` over `relation`.
async fn link(l: &Lake, relation: &str, from: &str, to: &str) -> lakelands_api::Result<Outcome> {
    let (ty, opts) = ("invoice".into(), WriteOpts::default());
    let (from, to, relation) = (from.into(), to.into(), relation.into());
    let fields = patch(json!({}));
    (l.cat.link(&l.own, &ty, &from, &relation, &to, fields, opts)).await
}

check!(
    RENAME_REFUSED = a_rename_that_would_change_or_lose_edges_is_refused,
    "035, extended (lakelands#429): a rename is refused as invalid, naming the relation, and nothing is appended, when its first name is no relation of the type or its second none of the new shape, when it changes the relation's link fields, when the same migration re-keys records, when the link type of the new name holds the retired links of an earlier relation, and when a reference field or a relation of another type names the link type that would move: that type is named to one `describe` shows it to, and to another only that a declaration refers to it"
);

async fn a_rename_that_would_change_or_lose_edges_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    // An earlier relation `signed_by`, linked, then dropped: its links are
    // retired and stay as history.
    let mut earlier = dated();
    (earlier.name, earlier.inverse) = ("signed_by".into(), None);
    l.declare(invoice(vec![dated(), earlier])).await?;
    link(&l, "signed_by", "i2", "p2").await.must("linking")?;
    l.migrate(invoice(vec![dated()])).await?;
    let before = store.head().await.must("reading the head")?;
    let pair = |old: &str, new: &str| vec![(old.into(), new.into())];
    let named = |name: &str, fields: bool| {
        let mut r = dated();
        r.name = name.into();
        r.fields.retain(|_| fields);
        r
    };
    let mut keyed = invoice(vec![named("countersigned_by", true)]);
    (keyed.key, keyed.fields[0].required) = (vec!["amount".into()], true);
    let mut red = Vec::new();
    for (what, to, relation_renames, code) in [
        (
            "a first name the type has no relation of",
            renaming().to,
            pair("ghost", "signed_by"),
            "relation_rename_names_none",
        ),
        (
            "a second name the new shape has no relation of",
            renaming().to,
            pair("approved_by", "ghost"),
            "relation_rename_names_none",
        ),
        (
            "a change of the link fields",
            invoice(vec![named("countersigned_by", false)]),
            pair("approved_by", "countersigned_by"),
            "relation_rename_changes",
        ),
        (
            "a re-key in the same migration",
            keyed,
            pair("approved_by", "countersigned_by"),
            "rename_while_rekeying",
        ),
        (
            "a name whose link type holds an earlier relation's retired links",
            renaming().to,
            pair("approved_by", "signed_by"),
            "rename_name_held",
        ),
    ] {
        // Said at the rename, which names the relation.
        let (from, to_name) = &relation_renames[0];
        let place = json!({"code": "relation_rename", "from": from, "to": to_name});
        let case = json!({"code": code, "at": [place]});
        let m = Migration {
            relation_renames,
            ..Migration::to(to)
        };
        let got = l.cat.migrate(&l.own, m).await;
        if !invalid_like(&got, &case) {
            red.push(format!("{what}: {got:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "each must be refused as invalid, naming the relation: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), before, "a refused rename").await?;
    // A relation that targets the link type, or a reference into it, would
    // be left naming keys that ended.
    let m = Migration {
        relation_renames: pair("approved_by", "countersigned_by"),
        ..Migration::to(invoice(vec![named("countersigned_by", true)]))
    };
    let mut minute = type_decl("minute", vec![field("t", StorageKind::Text)]);
    minute.relations.push(RelationDecl {
        target: OLD.into(),
        fields: Vec::new(),
        ..named("of", true)
    });
    let memo = type_decl("memo", vec![reference("about", OLD)]);
    for referring in [minute, memo] {
        let ty = referring.name.clone();
        l.declare(referring).await?;
        let got = l.cat.migrate(&l.own, m.clone()).await;
        ensure!(
            matches!(invalid_case(&got), Some(Case::RenameReferredLinks { ty: Some(t), .. })
                if *t == ty),
            "a rename of a relation whose link type `{ty}` refers to is refused, naming \
             `{ty}`; it answered {got:?}"
        );
    }
    // One kept from them, and so from the link type of `minute`'s relation
    // (006, extended), is told that a declaration refers to it, not which.
    let none = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    let mut apart = granted(store.as_ref(), "apart", Access::Write, ("memo", None, &[]));
    let kept = ["memo", "minute"];
    apart.grants.types = kept.map(|t| (t.into(), none.clone())).into();
    let before = store.head().await.must("reading the head")?;
    let write = ProposedWrite::Migrate {
        migration: Box::new(m),
        report: Default::default(),
    };
    let got = l.cat.propose(&apart, write, "a name".into()).await;
    ensure!(
        matches!(invalid_case(&got), Some(Case::RenameReferredLinks { ty: None, links })
            if links == "sys.link.invoice.approved_by"),
        "one kept from the types that refer to the link type is told neither; it is answered {got:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused rename").await
}

check!(
    RENAME_CARRIES = a_trigger_and_a_set_on_the_link_type_follow_the_rename,
    "035, extended (lakelands#429): a trigger on a renamed relation's link type and an object set of it are declared again by the lake under the new link type, after its declaration, in the migration's append, and the report lists them as carried; the set's members stay its members, and a link written or removed under the new name fires the trigger and leaves the set"
);

async fn a_trigger_and_a_set_on_the_link_type_follow_the_rename(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    l.declare_as(trigger_on("signed", OLD, None), "the trigger")
        .await?;
    let set = object_set_on("dated", OLD, "since = since");
    l.declare_as(set, "the set").await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    let before = store.head().await.must("reading the head")?;
    let report = l.cat.migrate(&l.own, renaming()).await;
    let report = report.must("the preflight")?.data;
    let carried = |kind: &str, name: &str| Carried {
        kind: kind.into(),
        name: name.into(),
    };
    // One who is kept from the link type states the relation without the
    // link fields it is not shown, and is not told what stands on the type.
    let mut author = granted(
        store.as_ref(),
        "author",
        Access::Read,
        (OLD, Some(Access::None), &[]),
    );
    author.grants.schema = true;
    let mut known = renaming();
    known.to.relations[0].fields.clear();
    let trimmed = l.cat.migrate(&author, known).await;
    let trimmed = trimmed
        .must("the preflight of one kept from the link type")?
        .data;
    ensure!(
        trimmed.carried.is_empty() && trimmed.more && trimmed.report_id != report.report_id,
        "one kept from the link type is shown no carried declaration, told there is more, \
         and given the id of what it is shown; it is answered {trimmed:?}"
    );
    // Every producer of a report trims it: a proposal's listing too.
    let (migration, whole) = (Box::new(renaming()), Default::default());
    let write = ProposedWrite::Migrate {
        migration,
        report: whole,
    };
    let proposed = l.cat.propose(&l.own, write, "the name".into()).await;
    proposed.must("proposing the rename")?;
    author.grants.review = true;
    let listed = l.cat.proposals(&author, Default::default()).await;
    let listed: Vec<(usize, bool)> = (listed.must("listing")?.data.iter())
        .filter_map(|p| match &p.write {
            Some(ProposedWrite::Migrate { report, .. }) => {
                Some((report.carried.len(), report.more))
            }
            _ => None,
        })
        .collect();
    ensure!(
        listed == [(0, true)],
        "the listing of one kept from the link type names no carried declaration and says \
         there is more; it holds (carried, more) {listed:?}"
    );
    apply(&l, renaming()).await?;
    let declared: Vec<String> = (events_after(store.as_ref(), before).await?.iter())
        .filter_map(|e| match &e.body {
            EventBody::Declared { declaration, .. } => {
                Some(crate::engines::declaration_key(declaration))
            }
            _ => None,
        })
        .collect();
    let (ty, from, relation, to) = (
        "invoice".into(),
        "i1".into(),
        "signed_by".into(),
        "p1".into(),
    );
    let gone = (l.cat).unlink(&l.own, &ty, &from, &relation, &to, WriteOpts::default());
    let gone = gone.await.must("removing the link under the new name")?;
    let Outcome::Applied { set_off, .. } = &gone else {
        return Err(Failure::new("an unlink is not answered as applied"));
    };
    ensure!(
        report.carried == [carried("trigger", "signed"), carried("object_set", "dated")]
            && declared
                == [
                    format!("type {NEW}"),
                    "trigger signed".into(),
                    "object_set dated".into()
                ]
            && set_off.fired == ["signed"]
            && set_off.left == ["dated"],
        "the report carries {:?}; the append declares {declared:?}; removing the link under \
         the new name set off {set_off:?}",
        report.carried
    );
    Ok(())
}

check!(
    RENAMED_EVERY_CARRIER = every_carrier_keeps_its_edges_through_renames_and_a_rebuild,
    "035, extended (lakelands#429): a relation carried by a `via` field, by link records or derived from matching values keeps its edges under its new name, and the report says how each is carried and how many links move; the walk backwards by the relation's inverse name follows; renamed a second time the links are records of the third name's link type alone; a read as of a position between answers by the name borne there; and a rebuilt projection answers all of it as the live one did"
);

async fn every_carrier_keeps_its_edges_through_renames_and_a_rebuild(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let shaped = |names: [&str; 3], extra: bool| {
        let mut fields = vec![
            field("amount", StorageKind::Decimal),
            reference("payer", "partner"),
            field("region", StorageKind::Text),
        ];
        fields.extend(extra.then(|| field("memo", StorageKind::Text)));
        let mut t = type_decl("invoice", fields);
        let (mut by_links, pairs) = (
            dated(),
            vec![MatchPair {
                from: "region".into(),
                to: "name".into(),
            }],
        );
        by_links.name = names[0].into();
        let by_via = RelationDecl {
            via: Some("payer".into()),
            ..RelationDecl::to(names[1], "partner")
        };
        let by_values = RelationDecl {
            cardinality: Cardinality::ManyToOne,
            derived: Some(Derived {
                pairs,
                filter: None,
            }),
            ..RelationDecl::to(names[2], "partner")
        };
        t.relations = vec![by_links, by_via, by_values];
        t
    };
    let first = ["approved_by", "paid_by", "near"];
    l.declare(shaped(first, false)).await?;
    l.upsert("partner", "p1", json!({"name": "north"})).await?;
    let invoice = json!({"amount": "10", "payer": "p1", "region": "north"});
    l.upsert("invoice", "i1", invoice).await?;
    link(&l, "approved_by", "i1", "p1").await.must("linking")?;
    // A migration that renames nothing leaves the lake's format.
    l.migrate(shaped(first, true)).await?;
    let before = store.head().await.must("reading the head")?;
    let renamed = |to: [&str; 3], from: [&str; 3]| Migration {
        relation_renames: (from.iter().zip(to).filter(|(a, b)| **a != *b))
            .map(|(a, b)| ((*a).into(), b.into()))
            .collect(),
        ..Migration::to(shaped(to, true))
    };
    let second = ["signed_by", "settled_by", "close_to"];
    let m = renamed(second, first);
    let report = l.cat.migrate(&l.own, m.clone()).await;
    let report = report.must("the preflight")?.data;
    let how: Vec<(Carrier, u64)> =
        (report.relations.iter().map(|r| (r.carrier, r.links))).collect();
    let applied = l.cat.migrate_apply(&l.own, m, &report.report_id).await;
    let between = applied.must("the first rename")?.position();
    let third = ["countersigned_by", "settled_by", "close_to"];
    apply(&l, renamed(third, second)).await?;
    let mut answers = Vec::new();
    for rebuilt in [false, true] {
        if rebuilt {
            store.rebuild().await.must("rebuilding the projection")?;
        }
        let mut walks = Vec::new();
        for (relation, as_of) in [
            ("countersigned_by", None),
            ("settled_by", None),
            ("close_to", None),
            ("signed_by", Some(between)),
            ("approved_by", Some(before)),
        ] {
            let edges = walked(&l, relation, "i1", as_of).await;
            walks.push(edges.map(|e| pairs(&e)).map_err(|e| json!(e).to_string()));
        }
        let back = pattern(
            "partner",
            Some("p1"),
            vec![step("approved", Walk::Out)],
            Projection::Graph,
        );
        let back = match l.cat.query(&l.own, back).await.must("walking back")?.data {
            QueryResult::Graph { edges, .. } => edges.len(),
            _ => 0,
        };
        let mut held = Vec::new();
        for ty in [OLD, NEW, "sys.link.invoice.countersigned_by"] {
            held.push(l.all_found(ty, Find::default()).await?.len());
        }
        answers.push(format!("{walks:?}, {back} back, links {held:?}"));
    }
    let one = || Ok(vec!["i1>p1".to_owned()]);
    let want = format!(
        "{:?}, 1 back, links [0, 0, 1]",
        [one(), one(), one(), one(), one()] as [Result<_, String>; 5]
    );
    ensure!(
        how == [
            (Carrier::Links, 1),
            (Carrier::Via, 0),
            (Carrier::Derived, 0)
        ] && answers == [want.clone(), want.clone()],
        "the report says {how:?}; live and rebuilt answer {answers:?}, each must be {want}"
    );
    Ok(())
}

/// `dated` under another name, with no inverse.
fn named(name: &str) -> RelationDecl {
    let mut r = dated();
    (r.name, r.inverse) = (name.into(), None);
    r
}

/// The migration of `invoice` to `to` that renames relation `old` to `new`.
fn rename(to: Vec<RelationDecl>, old: &str, new: &str) -> Migration {
    Migration {
        relation_renames: vec![(old.into(), new.into())],
        ..Migration::to(invoice(to))
    }
}

check!(
    RENAMED_NEWEST_CHAIN = the_newest_chain_of_a_link_types_names_continues,
    "006, extended (lakelands#429): a relation renamed onto a name, removed, and another renamed onto that name and renamed on: the grants on the first relation's link type do not follow the second relation's links, and the grants on the second's do, at the head and under every name"
);

async fn the_newest_chain_of_a_link_types_names_continues(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    // `w` is renamed to `x` and removed, never linked; `y` is linked, renamed
    // to `x`, the name `w` bore last, and on to `z`.
    l.migrate(invoice(vec![named("w"), named("y")])).await?;
    link(&l, "y", "i1", "p1").await.must("linking over `y`")?;
    apply(&l, rename(vec![named("x"), named("y")], "w", "x")).await?;
    l.migrate(invoice(vec![named("y")])).await?;
    apply(&l, rename(vec![named("x")], "y", "x")).await?;
    apply(&l, rename(vec![named("z")], "x", "z")).await?;
    let (s, none) = (store.as_ref(), Some(Access::None));
    let on = |relation: &str| {
        let ty = format!("sys.link.invoice.{relation}");
        granted(s, relation, Access::Read, (&ty, none, &[]))
    };
    let (first, second) = (
        seen(&l, &on("w"), "sys.link.invoice.z", None).await,
        seen(&l, &on("y"), "sys.link.invoice.z", None).await,
    );
    ensure!(
        first.starts_with("1 ") && second == "not found",
        "one kept from `w`'s link type reads the links of `z`, which were never `w`'s, and \
         one kept from `y`'s does not; they are answered {first} and {second}"
    );
    Ok(())
}

check!(
    RENAME_PROPOSED = a_proposed_rename_is_listed_logged_and_accepted,
    "013 and 035, extended (lakelands#429): a proposed migration that renames a relation is listed to its reviewer with the pair and the preflight's report of it, and to one kept from the type with neither; the `write_proposed` event holds the report whole; accepted, the relation keeps its edges under the new name"
);

async fn a_proposed_rename_is_listed_logged_and_accepted(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    let before = store.head().await.must("reading the head")?;
    let (migration, report) = (Box::new(renaming()), Default::default());
    let write = ProposedWrite::Migrate { migration, report };
    let proposed = l.cat.propose(&l.own, write, "the name".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing the rename")? else {
        return Err(Failure::new(
            "a proposed migration is not answered as proposed",
        ));
    };
    let mut kept = granted(
        store.as_ref(),
        "kept",
        Access::Read,
        ("invoice", Some(Access::None), &[]),
    );
    kept.grants.review = true;
    let mut shown = Vec::new();
    for who in [&l.own, &kept] {
        let listed = l.cat.proposals(who, Default::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        let one = listed.into_iter().find(|p| p.id == proposal);
        shown.push(match one.and_then(|p| Some((p.write?, p.more))) {
            Some((ProposedWrite::Migrate { migration, report }, more)) => {
                let pairs = migration.relation_renames.len();
                format!(
                    "{pairs} pair(s), {} in the report, more {more}",
                    report.relations.len()
                )
            }
            _ => "no write".to_owned(),
        });
    }
    // The listed shape holds no link field its reader is kept from, by a
    // grant on the link type's old name or on its new one, or by a default
    // alone.
    let since: &[(&str, Access)] = &[("since", Access::None)];
    let mut told = Vec::new();
    for on in [
        (OLD, None, since),
        (NEW, None, since),
        (OLD, Some(Access::None), &[]),
    ] {
        if lists_since(&l, store.as_ref(), on, &proposal).await? {
            told.push(on.0);
        }
    }
    ensure!(
        told.is_empty(),
        "a reviewer kept from `since` of the link type is listed the proposed shape without \
         it; kept by a grant on {told:?} it is listed `since`"
    );
    let logged = events_after(store.as_ref(), before).await?;
    let whole = logged.iter().any(|e| {
        matches!(&e.body, EventBody::WriteProposed { write: ProposedWrite::Migrate { report, .. }, .. }
            if report.relations.len() == 1)
    });
    l.cat.accept(&l.own, &proposal).await.must("accepting")?;
    let now = walked(&l, "signed_by", "i1", None).await.must("walking")?;
    ensure!(
        shown[0] == "1 pair(s), 1 in the report, more false"
            && !shown[1].starts_with("1 pair")
            && whole
            && pairs(&now) == ["i1>p1"],
        "the reviewer is listed {:?} and one kept from the type {:?}; the event holds the \
         report whole: {whole}; accepted, `signed_by` walks {:?}",
        shown[0],
        shown[1],
        pairs(&now)
    );
    // A proposal gone stale: it renames `signed_by`, which the owner renames
    // to another name while it waits. Its listed shape still hides `since`.
    let migration = Box::new(rename(
        vec![named("initialled_by")],
        "signed_by",
        "initialled_by",
    ));
    let write = ProposedWrite::Migrate {
        migration,
        report: Default::default(),
    };
    let stale = l.cat.propose(&l.own, write, "another name".into()).await;
    let Outcome::Proposed {
        proposal: stale, ..
    } = stale.must("proposing a second rename")?
    else {
        return Err(Failure::new(
            "a proposed migration is not answered as proposed",
        ));
    };
    apply(
        &l,
        rename(
            vec![named("countersigned_by")],
            "signed_by",
            "countersigned_by",
        ),
    )
    .await?;
    // Kept by a grant on the old name, on the name in force by a field, or
    // on it by a default alone (lakelands#594).
    let head = "sys.link.invoice.countersigned_by";
    let mut told = Vec::new();
    for on in [
        (NEW, None, since),
        (head, None, since),
        (head, Some(Access::None), &[]),
    ] {
        if lists_since(&l, store.as_ref(), on, &stale).await? {
            told.push(format!("{on:?}"));
        }
    }
    ensure!(
        told.is_empty(),
        "a proposal that renames a relation renamed since is listed without the link field \
         its reviewer is kept from, under the old name or the name in force; it is listed \
         `since` to one kept by {told:?}"
    );
    // The same for a field: `memo` is renamed to `note` by a proposal that
    // waits, and to `remark` by the owner; a reviewer kept from `remark`
    // is not listed `note`.
    let slip = |name: &str| type_decl("slip", vec![field(name, StorageKind::Text)]);
    l.declare(slip("memo")).await?;
    let renames = |to: &str| Migration {
        renames: vec![("memo".into(), to.into())],
        ..Migration::to(slip(to))
    };
    let write = ProposedWrite::Migrate {
        migration: Box::new(renames("note")),
        report: Default::default(),
    };
    let waits = l.cat.propose(&l.own, write, "a field's name".into()).await;
    let Outcome::Proposed {
        proposal: waits, ..
    } = waits.must("proposing a field's rename")?
    else {
        return Err(Failure::new(
            "a proposed migration is not answered as proposed",
        ));
    };
    apply(&l, renames("remark")).await?;
    let kept: &[(&str, Access)] = &[("remark", Access::None)];
    let mut who = granted(store.as_ref(), "narrow", Access::Read, ("slip", None, kept));
    who.grants.review = true;
    let listed = l.cat.proposals(&who, Default::default()).await;
    let listed = listed.must("listing the proposals")?.data;
    let write = listed.into_iter().find(|p| p.id == waits);
    let write = json!(write.and_then(|p| p.write)).to_string();
    ensure!(
        !write.contains("note"),
        "a proposal that renames a field renamed since is listed without the field its \
         reviewer is kept from under the name in force; it is listed {write}"
    );
    Ok(())
}

/// Whether the listing of `proposal` names the link field `since` to a
/// reviewer who reads the lake but for the grant `on` a link type.
async fn lists_since(
    l: &Lake,
    store: &dyn LakeStore,
    on: (&str, Option<Access>, &[(&str, Access)]),
    proposal: &lakelands_api::ProposalId,
) -> Result<bool, Failure> {
    let mut who = granted(store, "narrow", Access::Read, on);
    who.grants.review = true;
    let listed = l.cat.proposals(&who, Default::default()).await;
    let listed = listed.must("listing the proposals")?.data;
    let write = listed.into_iter().find(|p| &p.id == proposal);
    let write = json!(write.and_then(|p| p.write)).to_string();
    Ok(write.contains("since"))
}

check!(
    RENAMED_NAME_BORNE_TWICE = a_name_two_chains_bear_is_held_by_the_one_that_took_it_last,
    "006, extended (lakelands#429): where two chains of a link type's names bear one name, the grants follow the chain that took it last, whichever was begun first: a principal kept from a relation's links is kept from them under a name another relation bore before and was removed under; and as of a position where two chains begin at one name, a principal kept from either is kept; a relation renamed back to a name it bore is kept from as before"
);

async fn a_name_two_chains_bear_is_held_by_the_one_that_took_it_last(store: Store) -> Held {
    let (s, none) = (store.as_ref(), Some(Access::None));
    let lt = |relation: &str| format!("sys.link.invoice.{relation}");
    // `c` is linked and renamed to `e`; `a`, never linked, is renamed to `b`
    // and removed; then `e` takes the name `b`.
    let l = lake(s).await?;
    l.migrate(invoice(vec![named("c"), named("a")])).await?;
    link(&l, "c", "i1", "p1").await.must("linking over `c`")?;
    apply(&l, rename(vec![named("e"), named("a")], "c", "e")).await?;
    apply(&l, rename(vec![named("e"), named("b")], "a", "b")).await?;
    l.migrate(invoice(vec![named("e")])).await?;
    apply(&l, rename(vec![named("b")], "e", "b")).await?;
    let mut kept = granted(s, "kept", Access::Read, (&lt("c"), none, &[]));
    let on_a = TypeGrants {
        fields: [("since".into(), Access::Write)].into(),
        ..TypeGrants::default()
    };
    kept.grants.types.insert(lt("a").as_str().into(), on_a);
    let taken = seen(&l, &kept, &lt("b"), None).await;
    // `x` is linked and renamed to `y`; a new `x` is declared and renamed
    // to `z`: as of before, both chains begin at `x`.
    let before = store.head().await.must("reading the head")?;
    l.migrate(invoice(vec![named("b"), named("x")])).await?;
    link(&l, "x", "i2", "p2").await.must("linking over `x`")?;
    let linked = store.head().await.must("reading the head")?;
    apply(&l, rename(vec![named("b"), named("y")], "x", "y")).await?;
    l.declare(invoice(vec![named("b"), named("y"), named("x")]))
        .await?;
    apply(
        &l,
        rename(vec![named("b"), named("y"), named("z")], "x", "z"),
    )
    .await?;
    let mut early = granted(s, "early", Access::Read, (&lt("y"), none, &[]));
    let on_z = TypeGrants {
        fields: [("since".into(), Access::Write)].into(),
        ..TypeGrants::default()
    };
    early.grants.types.insert(lt("z").as_str().into(), on_z);
    let begun = seen(&l, &early, &lt("x"), Some(linked)).await;
    ensure!(
        taken == "not found" && begun == "not found" && linked > before,
        "one kept from `c`'s links is kept from them under `b`, which `a` bore first, and \
         one kept from `y`'s is kept from them as of when they were `x`'s; they are answered \
         {taken} and {begun}"
    );
    // `b` goes back to `c`, the name its links were first written under: one
    // kept from them under a name between is kept at the head, and one kept
    // from `c` is kept as of when they were `e`'s.
    let between = store.head().await.must("reading the head")?;
    let back = vec![named("c"), named("y"), named("z")];
    apply(&l, rename(back, "b", "c")).await?;
    let on_e = granted(s, "between", Access::Read, (&lt("e"), none, &[]));
    let (head, then) = (
        seen(&l, &on_e, &lt("c"), None).await,
        seen(&l, &kept, &lt("b"), Some(between)).await,
    );
    ensure!(
        head == "not found" && then == "not found",
        "a relation renamed back to a name it bore keeps what its grants take away: they \
         are answered {head} and {then}"
    );
    Ok(())
}

check!(
    RENAME_ONTO_A_STANDING_TYPE = a_rename_onto_a_standing_link_type_keeps_the_link_fields,
    "035, extended (lakelands#429): a removed relation's link type stays declared, with no live link; a relation renamed onto that name, or back onto a name it bore before a link field was added, keeps every link field of its links, a required one too: the standing link type is declared again in the renamed relation's shape, and a rebuilt projection answers the same; a rename onto a name whose standing link type a set reads a field of, which the renamed relation lacks, is refused, naming the set"
);

async fn a_rename_onto_a_standing_link_type_keeps_the_link_fields(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    // Every link bears `since`: the fold moves a link under a link type
    // that stood without its required field, and does not fail.
    let named = |name: &str| {
        let mut r = named(name);
        r.fields[0].required = true;
        r
    };
    let bare = |name: &str| {
        let mut r = named(name);
        r.fields.clear();
        r
    };
    // `w` has no link field and is removed, never linked; `y`, with
    // `since`, takes its name.
    l.migrate(invoice(vec![bare("w"), named("y")])).await?;
    link_ok_over(&l, "y", "i1", "p1", json!({"since": SEPT})).await?;
    l.migrate(invoice(vec![named("y")])).await?;
    apply(&l, rename(vec![named("w")], "y", "w")).await?;
    // `w` goes to `v`, gains the link field `note`, and goes back to `w`,
    // the name it bore without it.
    let wider = |name: &str| {
        let mut r = named(name);
        r.fields.push(field("note", StorageKind::Text));
        r
    };
    apply(&l, rename(vec![named("v")], "w", "v")).await?;
    l.migrate(invoice(vec![wider("v")])).await?;
    let both = json!({"since": SEPT, "note": "kept"});
    link_ok_over(&l, "v", "i1", "p1", both).await?;
    apply(&l, rename(vec![wider("w")], "v", "w")).await?;
    // A set reads `flag` of `n`'s link type, which stands after `n` is
    // removed: a rename onto `n` would leave the set reading a field its
    // type lacks, and is refused, naming the set.
    let mut flagged = bare("n");
    flagged.fields.push(field("flag", StorageKind::Text));
    l.migrate(invoice(vec![wider("w"), flagged])).await?;
    let set = object_set_on("flagged", "sys.link.invoice.n", "flag = 'x'");
    l.declare_as(set, "the set").await?;
    l.migrate(invoice(vec![wider("w")])).await?;
    let onto = l.cat.migrate(&l.own, rename(vec![wider("n")], "w", "n"));
    let onto = onto.await;
    ensure!(
        invalid_like(
            &onto,
            &json!({"code": "migration_takes_read", "field": "flag",
                    "type": "sys.link.invoice.n",
                    "at": [{"code": "object_set", "name": "flagged"}]})
        ),
        "a rename onto a name whose standing link type a set reads a field of is refused, \
         naming the set; it answered {onto:?}"
    );
    let mut answers = Vec::new();
    for rebuilt in [false, true] {
        if rebuilt {
            store.rebuild().await.must("rebuilding the projection")?;
        }
        let edges = walked(&l, "w", "i1", None).await.must("walking `w`")?;
        let held = |f: &str| edges.first().and_then(|e| e.fields.get(f).cloned());
        answers.push((pairs(&edges), held("since"), held("note")));
    }
    let want = (
        vec!["i1>p1".to_owned()],
        Some(json!(SEPT)),
        Some(json!("kept")),
    );
    ensure!(
        answers == [want.clone(), want],
        "the link keeps `since` and `note` under a name whose link type stood without \
         them, live and rebuilt; `w` walks {answers:?}"
    );
    Ok(())
}

/// The owner links invoice `from` to partner `to` over `relation`, with `fields`.
async fn link_ok_over(
    l: &Lake,
    relation: &str,
    from: &str,
    to: &str,
    fields: serde_json::Value,
) -> Held {
    let (ty, opts) = ("invoice".into(), WriteOpts::default());
    let (from, to, relation) = (from.into(), to.into(), relation.into());
    let linked = (l.cat).link(&l.own, &ty, &from, &relation, &to, patch(fields), opts);
    linked.await.must("linking").map(drop)
}

check!(
    RENAMED_VECTORS_FOLLOW = the_vectors_of_moved_links_are_found_under_the_new_link_type,
    "035, extended (lakelands#429): a link's vector field is searched under the new link type after its relation's rename, as it was under the old, live and after a rebuild"
);

async fn the_vectors_of_moved_links_are_found_under_the_new_link_type(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let pointed = |name: &str| {
        let mut r = named(name);
        r.fields = vec![vector("v", 2)];
        r
    };
    l.migrate(invoice(vec![pointed("lv")])).await?;
    link_ok_over(&l, "lv", "i1", "p1", json!({"v": [1.0, 0.0]})).await?;
    let nearest = |ty: &'static str| {
        let search = Search {
            ty: ty.into(),
            mode: SearchMode::vector("v", vec![1.0, 0.0]),
            k: 3,
            cursor: None,
            filter: None,
            with_meta: false,
            at: Default::default(),
        };
        let (cat, own) = (l.cat.clone(), l.own.clone());
        async move { cat.search(&own, search).await.map(|r| r.data.hits.len()) }
    };
    let before = nearest("sys.link.invoice.lv").await;
    apply(&l, rename(vec![pointed("lw")], "lv", "lw")).await?;
    let live = nearest("sys.link.invoice.lw").await;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = nearest("sys.link.invoice.lw").await;
    ensure!(
        matches!((&before, &live, &rebuilt), (Ok(1), Ok(1), Ok(1))),
        "a link's vector is found under its link type before the rename, under the new one \
         after it and after a rebuild; the three searches answered {before:?}, {live:?} and \
         {rebuilt:?} hits"
    );
    Ok(())
}

check!(
    RENAMED_LINKS_RECOMPUTED = a_rebuild_recomputes_the_links_a_rename_moved,
    "035, extended (lakelands#429): links written in an earlier page of the log, their relation renamed, and then a derived link field of it declared with another expression: every link is computed again under the new link type, by the live fold and by a rebuild, which folds the rename and the declaration in one page"
);

async fn a_rebuild_recomputes_the_links_a_rename_moved(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let noted = |name: &str, expr: &str| {
        let mut r = named(name);
        let loud = derived("loud", StorageKind::Text, expr, &["note"]);
        r.fields = vec![field("note", StorageKind::Text), loud];
        r
    };
    l.migrate(invoice(vec![noted("r1", "upper(note)")])).await?;
    link_ok_over(&l, "r1", "i1", "p1", json!({"note": "Aa"})).await?;
    // More than a page of the log between the link and the rename.
    l.declare(type_decl("filler", vec![sourced("n", StorageKind::Number)]))
        .await?;
    let filler = (0..1100).map(|i| source_record("filler", &format!("f{i:04}"), json!({"n": i})));
    let fut = l.ingest("load", filler.collect());
    fut.await.must("filling a page")?;
    apply(&l, rename(vec![noted("r2", "upper(note)")], "r1", "r2")).await?;
    l.declare(invoice(vec![noted("r2", "lower(note)")])).await?;
    let mut answers = Vec::new();
    for rebuilt in [false, true] {
        if rebuilt {
            store.rebuild().await.must("rebuilding the projection")?;
        }
        let edges = walked(&l, "r2", "i1", None).await.must("walking `r2`")?;
        answers.push(edges.first().and_then(|e| e.fields.get("loud").cloned()));
    }
    ensure!(
        answers == [Some(json!("aa")), Some(json!("aa"))],
        "the moved link's derived field is computed again by the new expression, live and \
         rebuilt; `loud` reads {answers:?}"
    );
    Ok(())
}

check!(
    RENAMED_OLD_TYPE_CLOSED = the_old_link_type_takes_no_link_after_the_rename,
    "035, extended (lakelands#429): after a relation's rename an ingest into the link type of its old name is refused as invalid, naming it, since no relation carries it; an ingest into the new link type is an edge of the renamed relation"
);

async fn the_old_link_type_takes_no_link_after_the_rename(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let fed = |name: &str| {
        let mut r = named(name);
        (r.fields, r.authority) = (Vec::new(), Authority::Source);
        r
    };
    l.migrate(invoice(vec![fed("src")])).await?;
    let one = |lt: &str, from: &str, to: &str| {
        let ends = json!({"from": from, "to": to});
        vec![source_record(lt, &format!("{from}:{to}"), ends)]
    };
    let first = l.ingest("feed", one("sys.link.invoice.src", "i1", "p1"));
    first.await.must("ingesting a link over `src`")?;
    apply(&l, rename(vec![fed("dst")], "src", "dst")).await?;
    let old = l.ingest("feed", one("sys.link.invoice.src", "i2", "p2"));
    let old = old.await;
    let new = l.ingest("feed", one("sys.link.invoice.dst", "i2", "p2"));
    let new = new.await;
    let edges = walked(&l, "dst", "i2", None).await.must("walking `dst`")?;
    ensure!(
        matches!(invalid_case(&old), Some(Case::UnclaimedLinkType { ty })
            if ty == "sys.link.invoice.src")
            && new.is_ok()
            && pairs(&edges) == ["i2>p2"],
        "an ingest into the old link type is refused, naming it, and one into the new is an \
         edge; they answered {old:?} and {new:?}, and `dst` walks {:?}",
        pairs(&edges)
    );
    Ok(())
}
