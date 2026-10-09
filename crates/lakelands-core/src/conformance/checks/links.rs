//! Relations carried by link records
//! (`docs/decisions/035-a-link-is-a-record.md`, `docs/log-format.md` §2.3).
//!
//! A relation declared without `via` is carried by one record per linked
//! pair, of the link type the store derives from it. A link is written
//! and removed through the relation's link doors, holds the relation's own
//! fields, and is walked like a relation carried by a reference field, its
//! fields on the edge. These checks write links and walk them, and push on
//! the rules that keep a relation a set: one link per pair, the relation's
//! cardinality over live links, live ends, and the doors each link goes
//! through.

use lakelands_api::error::Case;
use lakelands_api::ontology::{
    Authority, Cardinality, FieldDecl, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::principal::PrincipalKind;
use lakelands_api::query::{Edge, Projection, QueryResult, Walk};
use lakelands_api::write::{Outcome, WriteOpts};
use lakelands_api::{Catalog, Error, LakeStore, Position, Principal, Refusal};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, all, doors, field, field_writer, head_unmoved, invalid_case, migration,
    no_such_field, patch, pattern, principal, reads_all, reason, reference, refused_on, restrict,
    source_record, step, type_decl,
};
use crate::conformance::{Failure, Held, Store};
use crate::links::{link_key, link_type_name};

check!(
    WRITTEN_AND_WALKED = a_link_is_written_and_walked_with_its_fields,
    "035 and §2.3: a relation without via is carried by link records; the walk follows it out and in, and the edge carries the link's own fields"
);

check!(
    ONE_PER_PAIR = writing_a_link_again_changes_its_fields_and_keeps_one_link,
    "035: a link is keyed by its pair, so writing it again changes its fields and advances its version, never adds a second link"
);

check!(
    UNLINK_KEEPS_HISTORY = an_unlinked_link_leaves_the_walk_and_stays_in_its_history,
    "035 and §2.3: unlink retires the link record; the walk no longer follows it, and a read as of before the unlink still does"
);

check!(
    CARDINALITY = a_link_that_would_break_the_relations_cardinality_is_refused,
    "§2.3 and 024, extended: many_to_one allows one live link per `from`; a second is refused as a constraint and nothing is appended, and a principal without write on the link type is refused not_granted before the links are read; the record already linked is named only to a caller shown it, so a writer without `restricted`, or a holder of `only` not admitted to it, finds no key of it in the whole error (051)"
);

check!(
    LIVE_ENDS = a_link_to_a_key_that_is_no_live_record_is_refused,
    "032 and 035: a link's ends are references, so a link to a key that is not a live record of the target is refused on `to`"
);

check!(
    OWN_DOORS = link_records_go_through_the_link_doors_and_ingest_only,
    "035 and §2.3: upsert and retire refuse a link type's records, before a retry under the key of a link; a caller cannot declare a link type; the link door refuses a relation whose links arrive by ingest, before `from` among the fields, `from` among the fields before a retry, and a retry before the record checks (035, extended); and ingest writes them"
);

check!(
    CARRIER_KEPT = a_relation_cannot_move_between_via_and_link_records,
    "035 and §2.3: a declaration that gives a relation via it did not have, or takes it away, is refused"
);

check!(
    LINK_GRANTS = a_link_field_the_principal_may_not_write_refuses_the_link,
    "035: the link doors check the principal's grants on the link type's fields as upsert does"
);

check!(
    LINK_SHAPE = a_relations_fields_are_declared_when_added_and_migrated_when_retyped,
    "007, 027 and 035: adding an optional link field is declared; retyping one reshapes the link type, is refused at declare and goes through migrate on the owner type"
);

check!(
    RETIRED_END = retiring_an_end_keeps_the_link_and_the_walk_skips_the_retired_end,
    "032 and 035: retiring a record others link to stays allowed and keeps the links' history; the walk does not reach the retired record"
);

check!(
    ONE_PER_TO = a_relation_that_allows_one_link_per_target_refuses_a_second,
    "§2.3: one_to_many and one_to_one allow one live link per `to`; a second link into it is refused as a constraint"
);

check!(
    AUTHORITY_FLIP = a_relations_authority_changes_by_declaration_and_keeps_its_links,
    "035, extended: a relation's authority changes by an ordinary declaration; the door its links arrive by swaps, and the links already written stay"
);

check!(
    INGEST_CARDINALITY = ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window,
    "§2.3: a link that arrives by ingest holds the relation's cardinality over the lake's live links and the window's, its ends read from the key when the record leaves them out"
);

check!(
    REDECLARED_TARGET = a_relation_declared_again_with_another_target_is_a_migration,
    "027 and 035, extended: a relation removed by migrate keeps its links, and the preflight lists it under `dropped` with the number of its live links, none when none is live (lakelands#554); declaring it again with another target would reshape them, and is refused, naming the link type"
);

check!(
    RESTRICTED_LINK = a_restricted_link_is_walked_only_with_the_restricted_grant,
    "035 and the retention hold: a link's edge comes from its own record, so a restricted link is walked only by a principal with the `restricted` grant"
);

check!(
    NARROWING = narrowing_a_relations_cardinality_is_refused_while_links_would_break,
    "027 and §2.3: narrowing a relation's cardinality is a migration, and its preflight refuses it while live links would break the narrower rule"
);

const LT: &str = "sys.link.invoice.approved_by";
pub(super) const SEPT: &str = "2026-09-01T00:00:00.000000Z";
const OCT: &str = "2026-10-01T00:00:00.000000Z";

fn approved_by(fields: Vec<FieldDecl>, authority: Authority) -> RelationDecl {
    RelationDecl {
        name: "approved_by".into(),
        target: "partner".into(),
        cardinality: Cardinality::ManyToOne,
        inverse: Some("approved".into()),
        fields,
        authority,
        ..Default::default()
    }
}

/// `approved_by` as the lake declares it: authored, with a `since`.
pub(super) fn dated() -> RelationDecl {
    approved_by(
        vec![field("since", StorageKind::DateTime)],
        Authority::Authored,
    )
}

pub(super) fn invoice(relations: Vec<RelationDecl>) -> TypeDecl {
    let mut t = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    t.relations = relations;
    t
}

/// Partners p1 and p2, invoices i1 and i2, and `approved_by` from an
/// invoice to a partner, carried by link records with a `since`.
pub(super) async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    l.declare(invoice(vec![dated()])).await?;
    for p in ["p1", "p2"] {
        l.upsert("partner", p, json!({"name": p})).await?;
    }
    for i in ["i1", "i2"] {
        l.upsert("invoice", i, json!({"amount": "10"})).await?;
    }
    Ok(l)
}

async fn link(
    cat: &dyn Catalog,
    who: &Principal,
    from: &str,
    to: &str,
    fields: Value,
) -> lakelands_api::Result<Outcome> {
    cat.link(
        who,
        &"invoice".into(),
        &from.into(),
        &"approved_by".into(),
        &to.into(),
        patch(fields),
        WriteOpts::default(),
    )
    .await
}

/// Links `from` to `to` over `approved_by` as the owner.
pub(super) async fn link_ok(
    l: &Lake,
    from: &str,
    to: &str,
    fields: Value,
) -> Result<Outcome, Failure> {
    let linked = link(l.cat.as_ref(), &l.own, from, to, fields).await;
    linked.must(&format!("linking {from} to {to}"))
}

/// Unlinks `from` from `to` over `approved_by` as the owner.
async fn unlink(l: &Lake, from: &str, to: &str) -> lakelands_api::Result<Outcome> {
    let (ty, rel) = ("invoice".into(), "approved_by".into());
    let opts = WriteOpts::default();
    l.cat
        .unlink(&l.own, &ty, &from.into(), &rel, &to.into(), opts)
        .await
}

/// The edges a one-hop walk over `approved_by` answers from `key` of `ty`.
async fn walked(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
    walk: Walk,
    as_of: Option<Position>,
) -> Result<Vec<Edge>, Failure> {
    let mut p = pattern(
        ty,
        Some(key),
        vec![step("approved_by", walk)],
        Projection::Graph,
    );
    p.at.as_of = as_of;
    let reply = cat
        .query(who, p)
        .await
        .must(&format!("walking `approved_by` from `{ty}` `{key}`"))?;
    match reply.data {
        QueryResult::Graph { edges, .. } => Ok(edges),
        _ => Err(Failure::new("a graph projection answered something else")),
    }
}

pub(super) fn pairs(edges: &[Edge]) -> Vec<String> {
    edges
        .iter()
        .map(|e| format!("{}>{}", e.from.1, e.to.1))
        .collect()
}

async fn a_link_is_written_and_walked_with_its_fields(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    let out = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        pairs(&out) == ["i1>p1"],
        "walking `approved_by` out of i1 must reach p1 over the link; it answered {:?}",
        pairs(&out)
    );
    ensure!(
        out[0].fields.get("since") == Some(&json!(SEPT)),
        "the edge carries the link's own fields: `since` is {SEPT}, the edge holds {:?}",
        out[0].fields
    );
    let inn = walked(cat.as_ref(), own, "partner", "p1", Walk::In, None).await?;
    ensure!(
        pairs(&inn) == ["i1>p1"],
        "walking `approved_by` into p1 must come from i1; it answered {:?}",
        pairs(&inn)
    );
    Ok(())
}

async fn writing_a_link_again_changes_its_fields_and_keeps_one_link(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    let again = link_ok(&l, "i1", "p1", json!({"since": OCT})).await?;
    ensure!(
        matches!(again, Outcome::Applied { version: Some(v), .. } if v.0 == 2),
        "writing the same link again changes it: its version is 2; the answer was {again:?}"
    );
    let links = all(cat.as_ref(), own, LT).await?;
    ensure!(
        links.len() == 1,
        "one relation holds one link per pair; the link type holds {} records",
        links.len()
    );
    let out = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        out.len() == 1 && out[0].fields.get("since") == Some(&json!(OCT)),
        "the walk answers the one link with its new `since` ({OCT}); it answered {out:?}"
    );
    Ok(())
}

async fn an_unlinked_link_leaves_the_walk_and_stays_in_its_history(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let linked = link_ok(&l, "i1", "p1", json!({"since": SEPT}))
        .await?
        .position();
    unlink(&l, "i1", "p1").await.must("unlinking i1 from p1")?;
    let now = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        now.is_empty(),
        "after unlink the walk out of i1 reaches nothing; it answered {:?}",
        pairs(&now)
    );
    let then = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, Some(linked)).await?;
    ensure!(
        pairs(&then) == ["i1>p1"],
        "as of the link's position ({linked}) the walk still follows it; it answered {:?}",
        pairs(&then)
    );
    let missing = unlink(&l, "i1", "p1").await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "unlinking a link that is not live is not found; it answered {missing:?}"
    );
    Ok(())
}

async fn a_link_that_would_break_the_relations_cardinality_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({})).await?;
    // Many invoices to one partner is fine.
    link_ok(&l, "i2", "p1", json!({})).await?;
    let before = store.head().await.must("reading the head")?;
    let second = link(cat.as_ref(), own, "i1", "p2", json!({})).await;
    ensure!(
        matches!(refused_on(&second), Some(("to", Refusal::Constraint(..)))),
        "`approved_by` is many_to_one: i1 already links to p1, so linking it to p2 is \
         refused as a constraint on `to`; it answered {second:?}"
    );
    let reader = reads_all(store.as_ref(), "reader");
    let ungranted = link(cat.as_ref(), &reader, "i1", "p2", json!({})).await;
    ensure!(
        matches!(reason(&ungranted), Some(Refusal::NotGranted))
            || no_such_field(&ungranted, "from"),
        "a principal that writes nothing links i1 to p2: it is refused before the cardinality reads \
         i1's link to p1, which it must not learn of, as not granted or as for a field it may \
         not know of; it answered {ungranted:?}"
    );
    head_unmoved(store.as_ref(), before, "a link against the cardinality").await?;
    // p1 withheld: a writer not shown it, or not admitted to it, is not
    // told which record i1 links to.
    restrict(store.as_ref(), "partner", "p1", "withheld").await?;
    let before = store.head().await.must("reading the head")?;
    let writes = json!({"default": "write"});
    let only = json!({"default": "write", "types": {"partner": {"only": {"name": "p2"}}}});
    for (who, grants, constrained) in [("writer", writes, true), ("holder", only, false)] {
        let grants = serde_json::from_value(grants).expect("grants read");
        let who = principal(store.as_ref(), PrincipalKind::Agent, who, grants);
        let second = link(cat.as_ref(), &who, "i1", "p2", json!({})).await;
        let said = second
            .as_ref()
            .err()
            .map(|e| serde_json::to_string(e).unwrap_or_default());
        let named = said.as_deref().is_none_or(|s| s.contains("p1"));
        let held =
            !constrained || matches!(refused_on(&second), Some(("to", Refusal::Constraint(..))));
        ensure!(
            !named && held,
            "`{}` links i1 to p2, and i1 links to p1 (restricted, not `name` p2) already: it is \
             refused{}, and the whole error names no key of p1; it answered {second:?}",
            who.id,
            if constrained {
                " as a constraint on `to`"
            } else {
                ""
            }
        );
    }
    head_unmoved(store.as_ref(), before, "a link against a withheld link").await?;
    Ok(())
}

async fn a_link_to_a_key_that_is_no_live_record_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let dangling = link(cat.as_ref(), own, "i1", "nobody", json!({})).await;
    ensure!(
        matches!(refused_on(&dangling), Some(("to", _))),
        "no partner `nobody` exists, so the link is refused on `to`; it answered {dangling:?}"
    );
    let orphan = link(cat.as_ref(), own, "nothing", "p1", json!({})).await;
    ensure!(
        matches!(refused_on(&orphan), Some(("from", _))),
        "no invoice `nothing` exists, so the link is refused on `from`; it answered {orphan:?}"
    );
    head_unmoved(store.as_ref(), before, "a link to a missing record").await?;
    Ok(())
}

async fn link_records_go_through_the_link_doors_and_ingest_only(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let key = link_key(&"i1".into(), &"p1".into());
    let upserted = cat
        .upsert(
            own,
            &LT.into(),
            &key,
            patch(json!({"from": "i1", "to": "p1"})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(reason(&upserted), Some(Refusal::WrongDoor)),
        "upsert on a link type is the wrong door; it answered {upserted:?}"
    );
    link_ok(&l, "i1", "p1", json!({})).await?;
    let retired = cat
        .retire(own, &LT.into(), &key, WriteOpts::default())
        .await;
    ensure!(
        matches!(reason(&retired), Some(Refusal::WrongDoor)),
        "retire on a link type is the wrong door; it answered {retired:?}"
    );
    // In link's order (035, extended): `from` among the fields before the
    // retry, the retry before the record checks; and an ordinary door
    // refuses a link type before it looks for a retry.
    let once = WriteOpts {
        idempotency_key: Some("once".into()),
        ..WriteOpts::default()
    };
    let link_once = |who: Principal, fields: Value| {
        let (cat, once) = (cat.clone(), once.clone());
        async move {
            let (i2, p1, r) = ("i2".into(), "p1".into(), "approved_by".into());
            cat.link(&who, &"invoice".into(), &i2, &r, &p1, patch(fields), once)
                .await
        }
    };
    let first = link_once(own.clone(), json!({}))
        .await
        .must("linking i2 to p1 once")?;
    let bare = Principal {
        grants: Default::default(),
        ..own.clone()
    };
    let retry = link_once(bare, json!({})).await;
    let named = link_once(own.clone(), json!({"from": "i2"})).await;
    let i2_p1 = link_key(&"i2".into(), &"p1".into());
    let upsert = cat
        .upsert(own, &LT.into(), &i2_p1, patch(json!({"to": "p1"})), once)
        .await;
    ensure!(
        retry.as_ref().ok() == Some(&first)
            && matches!(refused_on(&named), Some(("from", Refusal::Invalid(..))))
            && matches!(reason(&upsert), Some(Refusal::WrongDoor)),
        "a retry of a link without grants answers {retry:?} (the first was {first:?}); one \
         naming `from` answers {named:?}; an upsert of its link record under its key \
         answers {upsert:?}"
    );
    let forged = type_decl(
        "sys.link.invoice.forged",
        vec![field("from", StorageKind::Text)],
    );
    let declared = l.try_declare(forged).await;
    ensure!(
        matches!(reason(&declared), Some(Refusal::Reserved)),
        "a caller cannot declare a link type; it answered {declared:?}"
    );
    // A relation whose links arrive by ingest.
    let mut sourced = approved_by(vec![], Authority::Source);
    sourced.name = "vouched_by".into();
    sourced.inverse = None;
    sourced.cardinality = Cardinality::ManyToMany;
    l.declare(invoice(vec![dated(), sourced])).await?;
    let by_door = cat
        .link(
            own,
            &"invoice".into(),
            &"i1".into(),
            &"vouched_by".into(),
            &"p2".into(),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await;
    // `from` among the fields too: the authority is judged first.
    let (i1, p2, vouched) = ("i1".into(), "p2".into(), "vouched_by".into());
    let named = patch(json!({"from": "i1"}));
    let both = cat
        .link(
            own,
            &"invoice".into(),
            &i1,
            &vouched,
            &p2,
            named,
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(reason(&by_door), Some(Refusal::WrongDoor))
            && matches!(reason(&both), Some(Refusal::WrongDoor)),
        "`vouched_by` takes its links from a source, so the link door refuses it, before \
         `from` among the fields; it answered {by_door:?}, and with `from` {both:?}"
    );
    let vt = link_type_name(&"invoice".into(), &"vouched_by".into());
    l.ingest(
        "registry",
        vec![source_record(
            vt.as_str(),
            link_key(&"i1".into(), &"p2".into()).as_str(),
            json!({"from": "i1", "to": "p2"}),
        )],
    )
    .await
    .must("ingesting the link i1 → p2 of `vouched_by`")?;
    let p = pattern(
        "invoice",
        Some("i1"),
        vec![step("vouched_by", Walk::Out)],
        Projection::Graph,
    );
    let reply = cat.query(own, p).await.must("walking `vouched_by`")?;
    let QueryResult::Graph { edges, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    ensure!(
        pairs(&edges) == ["i1>p2"],
        "the ingested link is walked like any; the walk answered {:?}",
        pairs(&edges)
    );
    Ok(())
}

async fn a_relation_cannot_move_between_via_and_link_records(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    let mut moved = invoice(vec![RelationDecl {
        via: Some("approver".into()),
        fields: vec![],
        ..approved_by(vec![], Authority::Authored)
    }]);
    moved.fields.push(reference("approver", "partner"));
    let answer = l.try_declare(moved).await;
    ensure!(
        matches!(reason(&answer), Some(Refusal::Invalid(..))),
        "`approved_by` is carried by link records; giving it `via` would move its links, and \
         is refused as invalid; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a relation moved to `via`").await?;
    Ok(())
}

async fn a_link_field_the_principal_may_not_write_refuses_the_link(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", LT, &["from", "to"]);
    let refused = link(cat.as_ref(), &clerk, "i1", "p1", json!({"since": SEPT})).await;
    ensure!(
        matches!(refused_on(&refused), Some(("since", Refusal::NotGranted))),
        "the clerk may write a link's ends but not its `since`; a link setting `since` is \
         refused on it; it answered {refused:?}"
    );
    link(cat.as_ref(), &clerk, "i1", "p1", json!({}))
        .await
        .must("the clerk linking i1 to p1 without `since`")?;
    Ok(())
}

async fn a_relations_fields_are_declared_when_added_and_migrated_when_retyped(
    store: Store,
) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    // An optional link field added: declared.
    let with = |weight: StorageKind| {
        invoice(vec![approved_by(
            vec![
                field("since", StorageKind::DateTime),
                field("weight", weight),
            ],
            Authority::Authored,
        )])
    };
    l.declare(with(StorageKind::Number)).await?;
    link_ok(&l, "i2", "p1", json!({"weight": 1.5})).await?;
    // `weight` retyped to text, whose form is a string where a number's
    // was not: the links would reshape, so declaring is refused.
    let refused = l.try_declare(with(StorageKind::Text)).await;
    ensure!(
        matches!(reason(&refused), Some(Refusal::Invalid(..))),
        "retyping a link field reshapes the link records; declaring it is refused as a \
         migration; it answered {refused:?}"
    );
    l.migrate(with(StorageKind::Text)).await?;
    let edges = walked(cat.as_ref(), own, "partner", "p1", Walk::In, None).await?;
    let weight = edges
        .iter()
        .find(|e| e.from.1.as_str() == "i2")
        .and_then(|e| e.fields.get("weight"))
        .cloned();
    ensure!(
        weight == Some(json!("1.5")),
        "after the migration the link's `weight` is text, the string \"1.5\" where the \
         number 1.5 was; the walk answered {weight:?}"
    );
    Ok(())
}

async fn retiring_an_end_keeps_the_link_and_the_walk_skips_the_retired_end(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    l.retire("partner", "p1").await?;
    let p = pattern(
        "invoice",
        Some("i1"),
        vec![step("approved_by", Walk::Out)],
        Projection::Graph,
    );
    let reply = cat.query(own, p).await.must("walking from i1")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["i1"],
        "p1 is retired, so the walk reaches no partner; it reached {keys:?}"
    );
    let links = all(cat.as_ref(), own, LT).await?;
    ensure!(
        links.len() == 1,
        "retiring an end keeps the link: the link type still holds it, but it holds {}",
        links.len()
    );
    Ok(())
}

async fn a_relation_that_allows_one_link_per_target_refuses_a_second(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let mut handled = approved_by(vec![], Authority::Authored);
    handled.name = "handled_by".into();
    handled.inverse = None;
    handled.cardinality = Cardinality::OneToMany;
    l.declare(invoice(vec![dated(), handled])).await?;
    let handle = |from: &str| {
        let (cat, own, from) = (cat.clone(), own.clone(), from.to_owned());
        async move {
            cat.link(
                &own,
                &"invoice".into(),
                &from.as_str().into(),
                &"handled_by".into(),
                &"p1".into(),
                patch(json!({})),
                WriteOpts::default(),
            )
            .await
        }
    };
    handle("i1")
        .await
        .must("linking i1 to p1 over `handled_by`")?;
    let before = store.head().await.must("reading the head")?;
    let second = handle("i2").await;
    ensure!(
        matches!(refused_on(&second), Some(("to", Refusal::Constraint(..)))),
        "`handled_by` is one_to_many: p1 is already handled by i1, so i2 cannot link to it; \
         the refusal is a constraint on `to`; it answered {second:?}"
    );
    head_unmoved(store.as_ref(), before, "a second link into one target").await?;
    Ok(())
}

async fn a_relations_authority_changes_by_declaration_and_keeps_its_links(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({})).await?;
    let sourced = FieldDecl {
        authority: Authority::Source,
        ..field("since", StorageKind::DateTime)
    };
    l.declare(invoice(vec![approved_by(vec![sourced], Authority::Source)]))
        .await?;
    let by_door = link(cat.as_ref(), own, "i2", "p2", json!({})).await;
    ensure!(
        matches!(reason(&by_door), Some(Refusal::WrongDoor)),
        "once `approved_by`'s links come from a source, the link door refuses them; it \
         answered {by_door:?}"
    );
    let kept = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        pairs(&kept) == ["i1>p1"],
        "the link written before the flip stays; the walk answered {:?}",
        pairs(&kept)
    );
    l.ingest(
        "registry",
        vec![source_record(
            LT,
            link_key(&"i2".into(), &"p2".into()).as_str(),
            json!({"since": SEPT}),
        )],
    )
    .await
    .must("ingesting the link i2 → p2, its ends from the key")?;
    let arrived = walked(cat.as_ref(), own, "invoice", "i2", Walk::Out, None).await?;
    ensure!(
        pairs(&arrived) == ["i2>p2"],
        "the ingested link, its ends read from its key, is walked; the walk answered {:?}",
        pairs(&arrived)
    );
    Ok(())
}

async fn ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window(
    store: Store,
) -> Held {
    let l = lake(store.as_ref()).await?;
    // A source relation of its own, beside `approved_by`.
    let mut vouched = approved_by(vec![], Authority::Source);
    vouched.name = "vouched_by".into();
    vouched.inverse = None;
    l.declare(invoice(vec![dated(), vouched])).await?;
    let vt = link_type_name(&"invoice".into(), &"vouched_by".into());
    let record = |from: &str, to: &str, ends: bool| {
        let fields = if ends {
            json!({"from": from, "to": to})
        } else {
            json!({"from": from})
        };
        source_record(
            vt.as_str(),
            link_key(&from.into(), &to.into()).as_str(),
            fields,
        )
    };
    // Two links from i1 in one window: many_to_one breaks.
    let window = l
        .ingest(
            "registry",
            vec![record("i1", "p1", true), record("i1", "p2", true)],
        )
        .await;
    ensure!(
        matches!(reason(&window), Some(Refusal::Constraint(..))),
        "two links from i1 in one window break `vouched_by`'s many_to_one; it answered \
         {window:?}"
    );
    l.ingest("registry", vec![record("i1", "p1", true)])
        .await
        .must("ingesting the link i1 → p1")?;
    // Against the lake, with `to` only in the key.
    let keyed = l.ingest("registry", vec![record("i1", "p2", false)]).await;
    ensure!(
        matches!(reason(&keyed), Some(Refusal::Constraint(..))),
        "i1 already links to p1; a record keyed i1:p2 that leaves `to` to its key \
         breaks the cardinality too, its end read from the key; it answered {keyed:?}"
    );
    Ok(())
}

async fn a_relation_declared_again_with_another_target_is_a_migration(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    // What the preflight of removing the relation lists as dropped.
    let dropped = || async {
        let report = l.cat.migrate(&l.own, migration(invoice(vec![]))).await;
        let report = report.must("the preflight of removing `approved_by`")?;
        Ok::<_, Failure>(report.data.dropped)
    };
    link_ok(&l, "i2", "p2", json!({})).await?;
    unlink(&l, "i2", "p2").await.must("unlinking i2 from p2")?;
    let unlinked = dropped().await?;
    link_ok(&l, "i1", "p1", json!({})).await?;
    link_ok(&l, "i2", "p2", json!({})).await?;
    let linked = dropped().await?;
    ensure!(
        unlinked == [("approved_by".into(), 0)] && linked == [("approved_by".into(), 2)],
        "removing `approved_by` takes its live links from the head: the preflight must list it \
         under `dropped` with its 2 live links, and with none while its only link is unlinked; \
         it listed {linked:?} and {unlinked:?}"
    );
    // The relation removed: a migration, which keeps its links.
    l.migrate(invoice(vec![])).await?;
    // Declared again, pointing at invoices now: the kept links would
    // reshape.
    let mut retargeted = approved_by(vec![], Authority::Authored);
    retargeted.target = "invoice".into();
    let before = store.head().await.must("reading the head")?;
    let answer = l.try_declare(invoice(vec![retargeted])).await;
    ensure!(
        matches!(invalid_case(&answer), Some(Case::ReshapesLinks { links, .. }) if links == LT),
        "declaring `approved_by` again with another target would reshape the links \
         `{LT}` keeps; it is refused naming it; it answered {answer:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "a re-declared relation that reshapes its links",
    )
    .await?;
    Ok(())
}

async fn a_restricted_link_is_walked_only_with_the_restricted_grant(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    link_ok(&l, "i1", "p1", json!({"since": SEPT})).await?;
    restrict(
        store.as_ref(),
        LT,
        link_key(&"i1".into(), &"p1".into()),
        "hold",
    )
    .await?;
    let reader = reads_all(store.as_ref(), "reader");
    let hidden = walked(cat.as_ref(), &reader, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        hidden.is_empty(),
        "the link is restricted and the reader holds no `restricted` grant, so the walk \
         does not follow it; it answered {:?}",
        pairs(&hidden)
    );
    let seen = walked(cat.as_ref(), own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        pairs(&seen) == ["i1>p1"],
        "the owner holds the `restricted` grant and walks the link; it answered {:?}",
        pairs(&seen)
    );
    Ok(())
}

async fn narrowing_a_relations_cardinality_is_refused_while_links_would_break(
    store: Store,
) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let with = |c: Cardinality| {
        let mut r = dated();
        r.cardinality = c;
        invoice(vec![r])
    };
    l.declare(with(Cardinality::ManyToMany)).await?;
    for p in ["p1", "p2"] {
        link_ok(&l, "i1", p, json!({})).await?;
    }
    let preflight = cat
        .migrate(own, migration(with(Cardinality::ManyToOne)))
        .await;
    ensure!(
        matches!(reason(&preflight), Some(Refusal::Constraint(..))),
        "i1 links to two partners, so narrowing `approved_by` to many_to_one would break \
         them; the preflight refuses it; it answered {preflight:?}"
    );
    Ok(())
}
