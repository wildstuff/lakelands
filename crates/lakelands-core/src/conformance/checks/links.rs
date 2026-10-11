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
use lakelands_api::write::{Outcome, ProposedWrite, WriteOpts};
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
    "035 and §2.3: unlink retires the link record; the walk no longer follows it, and a read as of before the unlink still does; an unlink answers in a retirement's order, the grant before the record"
);

check!(
    CARDINALITY = a_link_that_would_break_the_relations_cardinality_is_refused,
    "§2.3 and 024, extended: many_to_one allows one live link per `from`; a second is refused as a constraint and nothing is appended, and a principal without write on the link type is refused not_granted before the links are read; the record already linked is named only to a caller shown it, so a writer without `restricted`, or a holder of `only` not admitted to it, finds no key of it in the whole error (051)"
);

check!(
    VERB_KEEPS_ENDS = a_verb_on_a_link_type_keeps_what_the_link_door_keeps,
    "035 and 003, extended (lakelands#759): a verb on a relation's link type writes a live link's own fields and never its ends: one whose `set` names `from` or `to` is refused as invalid where it is declared, by `declare`, `propose` and `apply` alike; its call on a link an end of which is retired is refused as `link` with a field is, word for word, on that end, and nothing is appended, and so is the accept of a call proposed while both ends were live (lakelands#847); a caller without `restricted` whose link has a held end, and a holder of `only` on an end's type not admitted to that end, are each answered as for a key no link has; and a verb on the link type a relation's rename left behind is refused as on a type that is not declared; each door keeps its family's order: with an end retired and a stale expected version, `link` is refused on the end as without the version, and the verb's call answers the conflict"
);

check!(
    SET_NAMES_ITS_LINK_TYPE = a_set_declares_a_relation_and_what_stands_on_its_link_type,
    "035 and 028, extended: one set that declares a type with a relation carried by link records and a verb, a trigger, an object set and a view on the relation's link type is applied to a lake that holds none of them, and the same set of another type is proposed, listed to its reviewer with every declaration and nothing more, is not listed to a reviewer kept from the link type its relation brings, whose accept and reject of it answer as an id that does not exist (013, extended), and is accepted, and the owner alone, with nothing on its link type, is listed to that reviewer with `more` and the relation without its link fields, and applied by its accept; on the applied one a link and the verb's call each fire the trigger; a set whose verb names the link type of a relation its type does not declare is refused as on a type that is not declared, at the verb, by `apply` and `propose` alike, and one whose type no longer declares the relation the lake holds is refused with that refusal among its problems; a set that adds a field to a link type the lake holds and reads it is applied; an author not shown a link type the lake holds is refused a trigger on it as on a type that is not declared, alone and word for word in a set that declares its owner, applied or proposed, and one kept from a field of it is refused an object set that reads the field as reading a field it does not know, at the set, alone and word for word with the owner beside it"
);

check!(
    REMOVED_KEEPS_READERS = a_removed_relations_link_type_keeps_its_trigger_its_set_and_its_view,
    "035, extended: a relation with a trigger, an object set and a view on its link type and no verb is removed by its migration; `describe` still lists the three and the link type, the set holds the members and the view answers the count it did, and `link` and `unlink` over the relation are refused; a trigger, a set and a view are still declared on the link type, a `retire` of one of its links is refused at the ordinary door, as for any link type, and a retired end sets nothing off and keeps its link; a purge of the source that fed the links then retires them, fires the trigger for each, caused by its retirement, takes them out of the set and out of the view's count"
);

check!(
    LIVE_ENDS = a_link_to_a_key_that_is_no_live_record_is_refused,
    "032 and 035: a link's ends are references, so a link to a key that is not a live record of the target is refused on `to`"
);

check!(
    OWN_DOORS = link_records_are_made_through_the_link_doors_and_ingest_only,
    "035 and §2.3: upsert and retire refuse a link type's records, before a retry under the key of a link, and `propose` refuses an upsert and a retirement of one as they do (lakelands#847); a caller cannot declare a link type; the link door refuses a relation whose links arrive by ingest, before `from` among the fields, `from` among the fields before a retry, and a retry before the record checks (035, extended); and ingest writes them"
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
    "035, extended: a relation's authority changes by an ordinary declaration; the door its links arrive by swaps, and the links already written stay; `unlink` removes a source relation's link as it does any, whatever the authority"
);

check!(
    INGEST_CARDINALITY = ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window,
    "§2.3: a link that arrives by ingest holds the relation's cardinality over the lake's live links and the window's, an end read from the key when the record leaves it out"
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
    owner("invoice", relations)
}

/// A type `name` with an `amount` and `relations`.
fn owner(name: &str, relations: Vec<RelationDecl>) -> TypeDecl {
    let mut t = type_decl(name, vec![field("amount", StorageKind::Decimal)]);
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
    let linked = link_ok(&l, "i1", "p1", json!({"since": SEPT}));
    let linked = linked.await?.position();
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
    let reader = reads_all(store.as_ref(), "reader");
    let (ty, rel, opts) = ("invoice".into(), "approved_by".into(), WriteOpts::default());
    let (i1, p1) = ("i1".into(), "p1".into());
    let ungranted = cat.unlink(&reader, &ty, &i1, &rel, &p1, opts).await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. }))
            && matches!(reason(&ungranted), Some(Refusal::NotGranted)),
        "unlinking a link that is not live is not found, and in a retirement's order not \
         granted first to one who writes nothing; it answered {missing:?} and {ungranted:?}"
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

async fn a_verb_on_a_link_type_keeps_what_the_link_door_keeps(store: Store) -> Held {
    use crate::conformance::fixture::alike_renamed;
    use lakelands_api::write::Declaration;
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let verb = |name: &str, field: &str| -> Declaration {
        let set = json!([{"field": field, "value": format!("'{SEPT}'")}]);
        let v = json!({"declare": "verb", "name": name, "on": LT, "risk": "low",
            "effect": "write", "set": set});
        serde_json::from_value(v).expect("a verb reads")
    };
    // A `set` that names an end is refused where the verb is declared.
    for end in ["from", "to"] {
        let moved = verb("move", end);
        let declared = cat.declare(own, moved.clone()).await;
        let declarations = vec![moved.clone()];
        let proposed = cat.propose(own, ProposedWrite::Declare { declarations }, "why".into());
        let (proposed, applied) = (proposed.await, cat.apply(own, vec![moved]).await);
        ensure!(
            matches!(invalid_case(&declared), Some(Case::LinkEndAsField { end: e }) if e == end)
                && declared == proposed
                && declared == applied,
            "a verb that sets a link's `{end}` is refused as `link_end_as_field` by `declare`, \
             `propose` and `apply` alike; they answered {declared:?}, {proposed:?}, {applied:?}"
        );
    }
    let date = cat.declare(own, verb("date", "since")).await;
    date.must("declaring `date` on the link type")?;
    for (p, i) in [("p3", "i3"), ("p4", "i4")] {
        l.upsert("partner", p, json!({"name": p})).await?;
        l.upsert("invoice", i, json!({"amount": "10"})).await?;
    }
    for n in ["1", "2", "3", "4"] {
        link_ok(&l, &format!("i{n}"), &format!("p{n}"), json!({})).await?;
    }
    let call = |who: &Principal, from: &str, to: &str| {
        let (who, key) = (who.clone(), link_key(&from.into(), &to.into()));
        let (verb, opts) = ("date".into(), WriteOpts::default());
        async move {
            cat.call(&who, &verb, Some(&key), Default::default(), opts)
                .await
        }
    };
    // Proposed while both ends live, accepted below once one is retired.
    let mut held = Vec::new();
    for (from, to) in [("i1", "p1"), ("i2", "p2")] {
        let key = Some(link_key(&from.into(), &to.into()));
        let (verb, payload, opts) = ("date".into(), Default::default(), WriteOpts::default());
        let write = ProposedWrite::Call {
            verb,
            key,
            payload,
            opts,
        };
        let proposed = cat.propose(own, write, "why".into()).await;
        let Outcome::Proposed { proposal, .. } = proposed.must("proposing `date`")? else {
            return Err(Failure::new("a proposed call answers its proposal"));
        };
        held.push(proposal);
    }
    let applied = call(own, "i1", "p1").await;
    let links = all(cat.as_ref(), own, LT).await?;
    let since = |r: &lakelands_api::record::Record| r.fields.get("since") == Some(&json!(SEPT));
    ensure!(
        matches!(applied, Ok(Outcome::Applied { .. }))
            && links.iter().filter(|r| since(r)).count() == 1,
        "`date` on a link with both ends live is applied and sets its `since`; it answered \
         {applied:?}, and the links read {links:?}"
    );
    // An end retired: the verb is refused as `link` with a field is.
    l.retire("partner", "p1").await?;
    l.retire("invoice", "i2").await?;
    let before = store.head().await.must("reading the head")?;
    for ((from, to, end), id) in [("i1", "p1", "to"), ("i2", "p2", "from")]
        .into_iter()
        .zip(&held)
    {
        let by_link = link(cat.as_ref(), own, from, to, json!({"since": SEPT})).await;
        let by_verb = call(own, from, to).await;
        let accepted = cat.accept(own, id).await;
        let said = by_link.as_ref().err().and_then(Error::case);
        ensure!(
            matches!(refused_on(&by_link), Some((f, _)) if f == end)
                && matches!(said.map(|s| &s.case), Some(Case::NotLive { .. }))
                && by_verb == by_link
                && accepted == by_link,
            "with its `{end}` retired, `link` with a field is refused as `not_live` on `{end}`, \
             and a verb on the link, and the accept of its call proposed before, word for word \
             as it; they answered {by_link:?}, {by_verb:?} and {accepted:?}"
        );
    }
    // Each door keeps its family's order (035): beside a retired end, a
    // stale version is a verb's first answer and `link`'s second.
    let stale = WriteOpts {
        expected_version: Some(lakelands_api::Version(9)),
        ..Default::default()
    };
    let (ty, rel, date) = ("invoice".into(), "approved_by".into(), "date".into());
    let (from, to, since) = ("i1".into(), "p1".into(), patch(json!({"since": SEPT})));
    let by_link = cat.link(own, &ty, &from, &rel, &to, since, stale.clone());
    let (by_link, key) = (by_link.await, link_key(&from, &to));
    let by_verb = cat.call(own, &date, Some(&key), Default::default(), stale);
    let by_verb = by_verb.await;
    let fresh = link(cat.as_ref(), own, "i1", "p1", json!({"since": SEPT})).await;
    ensure!(
        by_link == fresh && matches!(by_verb, Err(Error::Conflict { .. })),
        "with `to` retired and a stale expected version, `link` is refused on `to` as without \
         the version ({fresh:?}), as any \
         record write judges its references first, and the verb's call is a conflict, as any \
         verb compares the version where it reads its record; they answered {by_link:?} and \
         {by_verb:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "a verb on a link with a retired end",
    )
    .await?;
    // An end held, or one its caller's `only` does not admit: no such link.
    restrict(store.as_ref(), "partner", "p3", "a hold").await?;
    let grants = |types: Value| {
        let grants = json!({"default": "write", "types": types});
        serde_json::from_value(grants).expect("grants read")
    };
    let writer = principal(store.as_ref(), PrincipalKind::Agent, "w", grants(json!({})));
    let holder = grants(json!({"partner": {"only": {"name": "another's"}}}));
    let holder = principal(store.as_ref(), PrincipalKind::Agent, "h", holder);
    for (who, from, to, what) in [
        (&writer, "i3", "p3", "a held end"),
        (&holder, "i4", "p4", "an end its `only` does not admit"),
    ] {
        let hidden = call(who, from, to).await.must_fail(what)?;
        let absent = call(who, "i9", "p9").await.must_fail("a key no link has")?;
        let keys = ("i9:p9", link_key(&from.into(), &to.into()));
        alike_renamed(what, &hidden, &absent, (keys.0, keys.1.as_str()))?;
        ensure!(
            matches!(hidden, Error::NotFound { .. }),
            "{what}: the link is not found; it answered {hidden:?}"
        );
    }
    // The writer may call the verb: on a link it is answered, it applies.
    let own_link = call(&writer, "i4", "p4").await;
    ensure!(
        matches!(own_link, Ok(Outcome::Applied { .. })),
        "a writer answered both ends of `i4:p4` has its `date` applied: {own_link:?}"
    );
    // Once the relation is renamed, its old link type is a name no type has.
    let signed_by = RelationDecl {
        name: "signed_by".into(),
        ..dated()
    };
    let rename = lakelands_api::write::Migration {
        relation_renames: vec![("approved_by".into(), "signed_by".into())],
        ..migration(invoice(vec![signed_by]))
    };
    let report = cat.migrate(own, rename.clone()).await;
    let report = report.must("the preflight of the rename")?.data.report_id;
    let renamed = cat.migrate_apply(own, rename, &report).await;
    renamed.must("renaming `approved_by`")?;
    let stale = cat.declare(own, verb("late", "since")).await;
    ensure!(
        matches!(invalid_case(&stale), Some(Case::Undeclared { ty }) if ty == LT),
        "a verb on the link type a renamed relation left behind is refused as on a type that \
         is not declared; it answered {stale:?}"
    );
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

async fn link_records_are_made_through_the_link_doors_and_ingest_only(store: Store) -> Held {
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
    // Proposed, either is refused as it is at its own door.
    let opts = WriteOpts::default;
    let (ty, since) = (|| LT.into(), patch(json!({"since": SEPT})));
    for write in [
        ProposedWrite::Upsert {
            ty: ty(),
            key: key.clone(),
            patch: since,
            opts: opts(),
        },
        ProposedWrite::Retire {
            ty: ty(),
            key: key.clone(),
            opts: opts(),
        },
    ] {
        let proposed = cat.propose(own, write, "why".into()).await;
        ensure!(
            matches!(reason(&proposed), Some(Refusal::WrongDoor)),
            "a proposed upsert or retirement of a link type's record is the wrong door; it \
             answered {proposed:?}"
        );
    }
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
    let first = link_once(own.clone(), json!({}));
    let first = first.await.must("linking i2 to p1 once")?;
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
    let fut = handle("i1");
    fut.await.must("linking i1 to p1 over `handled_by`")?;
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
    let unlinked = unlink(&l, "i2", "p2").await;
    unlinked.must("unlinking the link a source wrote")?;
    let left = walked(cat.as_ref(), own, "invoice", "i2", Walk::Out, None).await?;
    ensure!(
        left.is_empty(),
        "`unlink` asks the link type's grants and not the relation's authority, so the \
         source's link is gone; the walk answered {:?}",
        pairs(&left)
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
    let window = l.ingest(
        "registry",
        vec![record("i1", "p1", true), record("i1", "p2", true)],
    );
    let window = window.await;
    ensure!(
        matches!(reason(&window), Some(Refusal::Constraint(..))),
        "two links from i1 in one window break `vouched_by`'s many_to_one; it answered \
         {window:?}"
    );
    let fut = l.ingest("registry", vec![record("i1", "p1", true)]);
    fut.await.must("ingesting the link i1 → p1")?;
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
    let preflight = cat.migrate(own, migration(with(Cardinality::ManyToOne)));
    let preflight = preflight.await;
    ensure!(
        matches!(reason(&preflight), Some(Refusal::Constraint(..))),
        "i1 links to two partners, so narrowing `approved_by` to many_to_one would break \
         them; the preflight refuses it; it answered {preflight:?}"
    );
    Ok(())
}

async fn a_set_declares_a_relation_and_what_stands_on_its_link_type(store: Store) -> Held {
    use crate::conformance::fixture::{count_view, invalid_like, object_set_on, trigger_on};
    use lakelands_api::write::{Declaration, ProposedWrite};
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    l.upsert("partner", "p1", json!({"name": "p1"})).await?;
    let stamp = |name: &str, on: &str| -> Declaration {
        let set = json!([{"field": "since", "value": format!("'{SEPT}'")}]);
        let v = json!({"declare": "verb", "name": name, "on": on, "risk": "low",
            "effect": "write", "set": set});
        serde_json::from_value(v).expect("a verb reads")
    };
    // The relation's owner comes last: the four before it name a link type
    // only the set brings.
    let set = |ty: &str| {
        let (lt, with) = (format!("sys.link.{ty}.with"), "with".into());
        // No inverse: `partner` takes one name from one relation (lakelands#829).
        let with = RelationDecl {
            name: with,
            inverse: None,
            ..dated()
        };
        let linked = trigger_on(&format!("linked_{ty}"), &lt, None);
        let dated = object_set_on(&format!("dated_{ty}"), &lt, "since is not null");
        let counted = Declaration::Type(count_view(&format!("counted_{ty}"), &lt, "to"));
        let stamped = stamp(&format!("stamp_{ty}"), &lt);
        let owner = Declaration::Type(owner(ty, vec![with]));
        vec![stamped, linked, dated, counted, owner]
    };
    let applied = cat.apply(own, set("deal")).await;
    applied.must("applying a relation with a verb and a trigger on its link type")?;
    // Proposed, the set is listed whole to its reviewer and accepted; to a
    // reviewer kept from the link type its relation brings it is not there,
    // in the listing and at the accept and the reject (013, extended).
    let named = |set: &[Declaration]| set.iter().map(Declaration::what).collect::<Vec<_>>();
    let declarations = set("pact");
    let write = ProposedWrite::Declare { declarations };
    let proposed = cat.propose(own, write, "one set".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing the set for `pact`")? else {
        return Err(Failure::new("a proposal answers `proposed`"));
    };
    let blind = json!({"default": "read", "review": true, "schema": true, "types": {
        "sys.link.pact.with": {"default": "none"}, "sys.link.bare.with": {"default": "none"}}});
    let blind = serde_json::from_value(blind).expect("grants read");
    let blind = principal(store.as_ref(), PrincipalKind::Agent, "blind", blind);
    let mut listed = Vec::new();
    for who in [own, &blind] {
        let all = cat.proposals(who, Default::default()).await;
        let entry = all.must("a reviewer's listing")?.data;
        let entry = entry.into_iter().find(|p| p.id == proposal);
        listed.push(entry.map(|p| match p.write {
            Some(ProposedWrite::Declare { declarations }) => (named(&declarations), p.more),
            _ => (Vec::new(), p.more),
        }));
    }
    let missing = lakelands_api::Missing::Proposal {
        id: proposal.clone(),
    };
    let absent = Some(Error::NotFound { missing });
    let decided = [
        cat.accept(&blind, &proposal).await.err(),
        cat.reject(&blind, &proposal, "no".into()).await.err(),
    ];
    let whole =
        listed == [Some((named(&set("pact")), false)), None] && decided == [absent.clone(), absent];
    let accepted = cat.accept(own, &proposal).await;
    l.upsert("deal", "a1", json!({"amount": "1"})).await?;
    let (ty, from, rel, to) = ("deal".into(), "a1".into(), "with".into(), "p1".into());
    let linked = cat.link(
        own,
        &ty,
        &from,
        &rel,
        &to,
        patch(json!({})),
        WriteOpts::default(),
    );
    let (linked, key, verb) = (linked.await, link_key(&from, &to), "stamp_deal".into());
    let called = cat.call(
        own,
        &verb,
        Some(&key),
        Default::default(),
        WriteOpts::default(),
    );
    let called = called.await;
    let fired = |a: &lakelands_api::Result<Outcome>| {
        matches!(a, Ok(Outcome::Applied { set_off, .. })
            if set_off.fired.iter().any(|t| t.as_str() == "linked_deal"))
    };
    ensure!(
        whole
            && matches!(accepted, Ok(Outcome::Applied { .. }))
            && fired(&linked)
            && fired(&called),
        "a set of a relation with a verb, a trigger, a set and a view on its link type is \
         applied; proposed for another type it is listed to its reviewer with every \
         declaration and nothing more, and is not there to one kept from the link type \
         ({listed:?}; its accept and reject: {decided:?}), and accepted ({accepted:?}); on the \
         applied one a link and the verb's call each fire the trigger: {linked:?}, {called:?}"
    );
    // The owner alone, nothing on its link type, is listed to `blind` as
    // `describe` would show it the owner once in force: with `more`, the
    // relation without its link fields; and its accept applies it.
    let declarations = set("bare").split_off(4);
    let write = ProposedWrite::Declare { declarations };
    let bare = cat.propose(own, write, "the owner alone".into()).await;
    let Outcome::Proposed { proposal: bare, .. } = bare.must("proposing `bare` alone")? else {
        return Err(Failure::new("a proposal answers `proposed`"));
    };
    let all = cat.proposals(&blind, Default::default()).await;
    let entry = all.must("a reviewer's listing")?.data;
    let entry = entry.into_iter().find(|p| p.id == bare);
    let link_fields = match entry.as_ref().map(|p| (&p.write, p.more)) {
        Some((Some(ProposedWrite::Declare { declarations }), true)) => match &declarations[..] {
            [Declaration::Type(t)] => t.relations.iter().map(|r| r.fields.len()).collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let applied = cat.accept(&blind, &bare).await;
    ensure!(
        link_fields == [0] && matches!(applied, Ok(Outcome::Applied { .. })),
        "the owner alone, proposed, is listed to one kept from its link type with `more` and \
         the relation without its link fields, and its accept applies it; it was listed \
         {entry:?} and its accept answered {applied:?}"
    );
    // A set may also name a field it adds to a link type the lake holds.
    let mut noted = RelationDecl {
        name: "with".into(),
        inverse: None,
        ..dated()
    };
    noted.fields.push(field("note", StorageKind::Text));
    let reads = object_set_on("noted_pact", "sys.link.pact.with", "note is not null");
    let grown = vec![reads, Declaration::Type(owner("pact", vec![noted]))];
    let grown = cat.apply(own, grown).await;
    grown.must("a set that adds a field to a held link type and reads it")?;
    // A link type no relation of the set's type carries: of a type new with
    // the set, and of one the lake holds with the relation the set drops.
    let before = store.head().await.must("reading the head")?;
    let dropped = "a type the set leaves without the relation";
    for (ty, what, alone) in [
        ("bond", "a type new with the set", true),
        ("deal", dropped, false),
    ] {
        let lt = format!("sys.link.{ty}.with");
        let set = vec![stamp("late", &lt), Declaration::Type(owner(ty, vec![]))];
        let applied = cat.apply(own, set.clone()).await;
        let write = ProposedWrite::Declare { declarations: set };
        let proposed = cat.propose(own, write, "one set".into()).await;
        let at_verb =
            json!({"code": "undeclared", "type": lt, "at": [{"code": "verb", "name": "late"}]});
        // The type that drops its relation is a problem of its own beside it.
        let whole = json!({"code": "set_refused", "count": 2, "problems": [{"reason": at_verb}]});
        ensure!(
            invalid_like(&applied, if alone { &at_verb } else { &whole }) && applied == proposed,
            "a set with a verb on `{lt}`, the link type of a relation of {what}, is refused as \
             `undeclared` at the verb, by `apply` and `propose` alike; they answered \
             {applied:?} and {proposed:?}"
        );
    }
    // An author kept from a link type the lake holds names it in no set,
    // with its owner beside it or without (006).
    let lt = "sys.link.deal.with";
    let grants = json!({"default": "write", "schema": true, "types": {lt: {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let kept = principal(store.as_ref(), PrincipalKind::Agent, "kept", grants);
    // It sends `deal` as it is described it: the relation without the
    // link's field, which is the link type's.
    let (peek, with) = (trigger_on("peek", lt, None), "with".into());
    let with = RelationDecl {
        name: with,
        fields: Vec::new(),
        inverse: None,
        ..dated()
    };
    let beside = vec![peek.clone(), Declaration::Type(owner("deal", vec![with]))];
    let write = ProposedWrite::Declare {
        declarations: beside.clone(),
    };
    let proposed = cat.propose(&kept, write, "one set".into()).await;
    let (beside, alone) = (
        cat.apply(&kept, beside).await,
        cat.declare(&kept, peek).await,
    );
    let at_peek =
        json!({"code": "undeclared", "type": lt, "at": [{"code": "trigger", "name": "peek"}]});
    ensure!(
        invalid_like(&alone, &at_peek) && beside == alone && proposed == alone,
        "an author not shown `{lt}` is refused a trigger on it as on a type that is not \
         declared, at the trigger: alone, and word for word in a set that declares its \
         owner, applied or proposed; they answered {alone:?}, {beside:?} and {proposed:?}"
    );
    // An author kept from a field of a link type the lake holds reads it in
    // no set: a set over it is refused at the set, alone and, word for
    // word, with the owner that names the field beside it.
    let grants = json!({"default": "write", "schema": true,
        "types": {lt: {"default": "write", "fields": {"since": "none"}}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let blind = principal(store.as_ref(), PrincipalKind::Agent, "fieldless", grants);
    let whole = RelationDecl {
        name: "with".into(),
        inverse: None,
        ..dated()
    };
    let reads = object_set_on("since_known", lt, "since is not null");
    let beside = vec![reads.clone(), Declaration::Type(owner("deal", vec![whole]))];
    let (beside, alone) = (
        cat.apply(&blind, beside).await,
        cat.declare(&blind, reads).await,
    );
    let at_set = json!({"code": "unknown_field", "field": "since",
        "at": [{"code": "object_set", "name": "since_known"}]});
    ensure!(
        invalid_like(&alone, &at_set) && beside == alone,
        "an author kept from `since` of `{lt}` is refused an object set that reads it as \
         reading a field it does not know, at the set: alone, and word for word with the \
         owner in the same set; it answered {alone:?} and {beside:?}"
    );
    let what = "a set that names a link type no relation carries, or one its author is not shown";
    head_unmoved(store.as_ref(), before, what).await
}

async fn a_removed_relations_link_type_keeps_its_trigger_its_set_and_its_view(
    store: Store,
) -> Held {
    use crate::conformance::fixture::{
        all_found, count_view, events_from, object_set_on, trigger_on,
    };
    use lakelands_api::event::EventBody;
    use lakelands_api::write::Declaration;
    const FED: &str = "sys.link.deal.fed";
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    l.upsert("partner", "p1", json!({"name": "p1"})).await?;
    let sourced = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let fed = RelationDecl {
        fields: vec![sourced(field("state", StorageKind::Text))],
        authority: Authority::Source,
        ..RelationDecl::to("fed", "partner")
    };
    l.declare(owner("deal", vec![fed])).await?;
    for d in ["d1", "d2"] {
        l.upsert("deal", d, json!({"amount": "1"})).await?;
    }
    let readers = [
        trigger_on("fed_went", FED, None),
        object_set_on("fed_open", FED, "state = 'a'"),
        Declaration::Type(count_view("fed_by_state", FED, "state")),
    ];
    for d in readers {
        cat.declare(own, d)
            .await
            .must("declaring a reader of the link type")?;
    }
    let link = |d: &str| {
        let ends = json!({"from": d, "to": "p1", "state": "a"});
        source_record(FED, link_key(&d.into(), &"p1".into()).as_str(), ends)
    };
    l.ingest("erp", vec![link("d1"), link("d2")])
        .await
        .must("ingesting two `fed` links")?;
    // What the set holds and the view answers, as text.
    let read = || async {
        let members = lakelands_api::read::Find {
            filter: Some("in_set('fed_open')".into()),
            ..Default::default()
        };
        let members = all_found(cat.as_ref(), own, FED, members).await?;
        let members: Vec<_> = members.iter().map(|r| r.key.to_string()).collect();
        let counted = all(cat.as_ref(), own, "fed_by_state").await?;
        let counted: Vec<_> = (counted.iter())
            .map(|r| format!("{}={}", r.key, r.fields.get("n").unwrap_or(&Value::Null)))
            .collect();
        Ok::<_, Failure>(format!("{members:?} {counted:?}"))
    };
    let held = read().await?;
    l.migrate(owner("deal", vec![])).await?;
    let described = cat.describe(own, Default::default()).await;
    let d = described.must("describing the lake")?.data;
    let typed = |n: &&str| d.types.iter().any(|t| t.name.as_str() == *n);
    let listed = d.triggers.iter().any(|t| t.name.as_str() == "fed_went")
        && d.object_sets.iter().any(|s| s.name.as_str() == "fed_open")
        && ["fed_by_state", FED].iter().all(typed);
    let kept = read().await?;
    ensure!(
        listed && kept == held && held == r#"["d1:p1", "d2:p1"] ["a=2"]"#,
        "with `fed` removed, `describe` lists the trigger, the set, the view and `{FED}` \
         ({listed}), and the set and the view read as before it, {held}; they read {kept}"
    );
    let (ty, rel, from, to) = ("deal".into(), "fed".into(), "d1".into(), "p1".into());
    let linked = cat.link(
        own,
        &ty,
        &from,
        &rel,
        &to,
        patch(json!({})),
        WriteOpts::default(),
    );
    let linked = linked.await;
    let unlinked = cat
        .unlink(own, &ty, &from, &rel, &to, WriteOpts::default())
        .await;
    let no_relation = |a: &lakelands_api::Result<Outcome>| {
        matches!(invalid_case(a), Some(Case::NoLinkRelation { .. }))
    };
    ensure!(
        no_relation(&linked) && no_relation(&unlinked),
        "`link` and `unlink` over the removed `fed` find no relation carried by links; they \
         answered {linked:?} and {unlinked:?}"
    );
    // What stands on the link type may be declared there anew; a `retire`
    // of a link is not the ordinary door's, and an end retired sets nothing off.
    for d in [
        trigger_on("fed_seen", FED, None),
        object_set_on("fed_shut", FED, "state = 'b'"),
        Declaration::Type(count_view("fed_by_end", FED, "to")),
    ] {
        let declared = cat.declare(own, d).await;
        declared.must("declaring a reader on the link type its relation left")?;
    }
    let (fed, key, other) = (FED.into(), link_key(&from, &to), "d2".into());
    let retired = cat.retire(own, &fed, &key, WriteOpts::default()).await;
    let end = cat.retire(own, &ty, &other, WriteOpts::default()).await;
    ensure!(
        matches!(reason(&retired), Some(Refusal::WrongDoor))
            && matches!(&end, Ok(Outcome::Applied { set_off, .. }) if set_off.fired.is_empty()),
        "a `retire` of a link of the removed `fed` is not the ordinary door's to make, and \
         `d2`, an end of one, is retired and sets nothing off; they answered {retired:?} and \
         {end:?}"
    );
    // A purge still takes the source's links away, and the readers follow.
    let before = store.head().await.must("reading the head")?;
    let purged = store.purge_source(&"erp".into(), None, None).await;
    purged.must("purging the source that fed the links")?;
    let feed = events_from(store.as_ref(), before.next()).await?;
    let retired = |p: Position| {
        let cause = feed.iter().find(|e| e.position == p);
        cause.is_some_and(
            |e| matches!(&e.body, EventBody::RecordRetired { ty, .. } if ty.as_str() == FED),
        )
    };
    let mut fired: Vec<_> = (feed.iter())
        .filter_map(|e| match &e.body {
            EventBody::TriggerFired { trigger, key, .. } if trigger.as_str() == "fed_went" => {
                Some((key.to_string(), e.cause.is_some_and(retired)))
            }
            _ => None,
        })
        .collect();
    fired.sort();
    let after = read().await?;
    ensure!(
        fired == [("d1:p1".to_owned(), true), ("d2:p1".to_owned(), true)] && after == "[] []",
        "a purge of `erp` retires both links of `{FED}`: `fed_went` fires for each, caused by \
         its retirement, and the set and the view hold none; it fired {fired:?}, and they read \
         {after}"
    );
    Ok(())
}
