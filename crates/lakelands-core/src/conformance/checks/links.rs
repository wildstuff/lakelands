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

use std::sync::Arc;

use lakelands_api::ontology::{
    Authority, Cardinality, FieldDecl, FieldType, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::query::{Depth, Edge, Pattern, Projection, QueryResult, Step, Walk};
use lakelands_api::write::{Declaration, Migration, Outcome, WriteOpts};
use lakelands_api::{
    Catalog, Error, LakeStore, Position, Principal, ReadAt, RecordKey, Refusal, TypeName,
};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all, declare, doors, field, field_writer, head_unmoved, ingest, owner, patch,
    source_record, type_decl, upsert,
};
use crate::links::{link_key, link_type_name};

check!(
    /// A link is written and walked, its fields on the edge.
    WRITTEN_AND_WALKED = a_link_is_written_and_walked_with_its_fields,
    "035 and §2.3: a relation without via is carried by link records; the walk follows it out and in, and the edge carries the link's own fields"
);

check!(
    /// One link per pair.
    ONE_PER_PAIR = writing_a_link_again_changes_its_fields_and_keeps_one_link,
    "035: a link is keyed by its pair, so writing it again changes its fields and advances its version, never adds a second link"
);

check!(
    /// Unlink keeps history.
    UNLINK_KEEPS_HISTORY = an_unlinked_link_leaves_the_walk_and_stays_in_its_history,
    "035 and §2.3: unlink retires the link record; the walk no longer follows it, and a read as of before the unlink still does"
);

check!(
    /// Cardinality holds over live links.
    CARDINALITY = a_link_that_would_break_the_relations_cardinality_is_refused,
    "§2.3: many_to_one allows one live link per `from`; a second is refused as a constraint and nothing is appended"
);

check!(
    /// Both ends are live records.
    LIVE_ENDS = a_link_to_a_key_that_is_no_live_record_is_refused,
    "032 and 035: a link's ends are references, so a link to a key that is not a live record of the target is refused on `to`"
);

check!(
    /// Links go through their own doors.
    OWN_DOORS = link_records_go_through_the_link_doors_and_ingest_only,
    "035 and §2.3: upsert and retire refuse a link type's records, a caller cannot declare a link type, the link door refuses a relation whose links arrive by ingest, and ingest writes them"
);

check!(
    /// A relation keeps its carrier.
    CARRIER_KEPT = a_relation_cannot_move_between_via_and_link_records,
    "035 and §2.3: a declaration that gives a relation via it did not have, or takes it away, is refused"
);

check!(
    /// A link's fields are granted like any field.
    LINK_GRANTS = a_link_field_the_principal_may_not_write_refuses_the_link,
    "035: the link doors check the principal's grants on the link type's fields as upsert does"
);

check!(
    /// A relation's fields follow 027.
    LINK_SHAPE = a_relations_fields_are_declared_when_added_and_migrated_when_retyped,
    "007, 027 and 035: adding an optional link field is declared; retyping one reshapes the link type, is refused at declare and goes through migrate on the owner type"
);

check!(
    /// Retiring an end keeps the link.
    RETIRED_END = retiring_an_end_keeps_the_link_and_the_walk_skips_the_retired_end,
    "032 and 035: retiring a record others link to stays allowed and keeps the links' history; the walk does not reach the retired record"
);

check!(
    /// One link per `to`.
    ONE_PER_TO = a_relation_that_allows_one_link_per_target_refuses_a_second,
    "§2.3: one_to_many and one_to_one allow one live link per `to`; a second link into it is refused as a constraint"
);

check!(
    /// An authority flip swaps the door, keeps the links.
    AUTHORITY_FLIP = a_relations_authority_changes_by_declaration_and_keeps_its_links,
    "035, extended: a relation's authority changes by an ordinary declaration; the door its links arrive by swaps, and the links already written stay"
);

check!(
    /// Ingest holds the cardinality.
    INGEST_CARDINALITY = ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window,
    "§2.3: a link that arrives by ingest holds the relation's cardinality over the lake's live links and the window's, its ends read from the key when the record leaves them out"
);

check!(
    /// A re-declared relation that reshapes its links is a migration.
    REDECLARED_TARGET = a_relation_declared_again_with_another_target_is_a_migration,
    "027 and 035, extended: a relation removed by migrate keeps its links; declaring it again with another target would reshape them, and is refused, naming the link type"
);

check!(
    /// A restricted link is not walked without the grant.
    RESTRICTED_LINK = a_restricted_link_is_walked_only_with_the_restricted_grant,
    "035 and the retention hold: a link's edge comes from its own record, so a restricted link is walked only by a principal with the `restricted` grant"
);

check!(
    /// A narrowing that would break links is refused.
    NARROWING = narrowing_a_relations_cardinality_is_refused_while_links_would_break,
    "027 and §2.3: narrowing a relation's cardinality is a migration, and its preflight refuses it while live links would break the narrower rule"
);

const LT: &str = "sys.link.invoice.approved_by";
const SEPT: &str = "2026-09-01T00:00:00.000000Z";
const OCT: &str = "2026-10-01T00:00:00.000000Z";

fn approved_by(fields: Vec<FieldDecl>, authority: Authority) -> RelationDecl {
    RelationDecl {
        name: "approved_by".into(),
        target: "partner".into(),
        cardinality: Cardinality::ManyToOne,
        inverse: Some("approved".into()),
        via: None,
        fields,
        authority,
        description: None,
    }
}

fn invoice(relations: Vec<RelationDecl>) -> TypeDecl {
    let mut t = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    t.relations = relations;
    t
}

/// Partners p1 and p2, invoices i1 and i2, and `approved_by` from an
/// invoice to a partner, carried by link records with a `since`.
async fn lake(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    declare(
        cat.as_ref(),
        &own,
        type_decl("partner", vec![field("name", StorageKind::Text)]),
    )
    .await?;
    declare(
        cat.as_ref(),
        &own,
        invoice(vec![approved_by(
            vec![field("since", StorageKind::DateTime)],
            Authority::Authored,
        )]),
    )
    .await?;
    for p in ["p1", "p2"] {
        upsert(cat.as_ref(), &own, "partner", p, json!({"name": p})).await?;
    }
    for i in ["i1", "i2"] {
        upsert(cat.as_ref(), &own, "invoice", i, json!({"amount": "10"})).await?;
    }
    Ok(())
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

/// The edges a one-hop walk over `approved_by` answers from `key` of `ty`.
async fn walked(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    key: &str,
    walk: Walk,
    as_of: Option<Position>,
) -> Result<Vec<Edge>, Failure> {
    let mut p = Pattern::traverse(
        TypeName::from(ty),
        RecordKey::from(key),
        vec![Step {
            relation: "approved_by".into(),
            walk,
            ty: None,
            filter: None,
            depth: Depth { min: 1, max: 1 },
        }],
    );
    p.project = Projection::Graph;
    p.at = ReadAt {
        as_of,
        valid_at: None,
    };
    let reply = cat
        .query(who, p)
        .await
        .must(&format!("walking `approved_by` from `{ty}` `{key}`"))?;
    match reply.data {
        QueryResult::Graph { edges, .. } => Ok(edges),
        _ => Err(Failure::new("a graph projection answered something else")),
    }
}

fn pairs(edges: &[Edge]) -> Vec<String> {
    edges
        .iter()
        .map(|e| format!("{}>{}", e.from.1, e.to.1))
        .collect()
}

async fn a_link_is_written_and_walked_with_its_fields(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?;
    let out = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, None).await?;
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
    let inn = walked(cat.as_ref(), &own, "partner", "p1", Walk::In, None).await?;
    ensure!(
        pairs(&inn) == ["i1>p1"],
        "walking `approved_by` into p1 must come from i1; it answered {:?}",
        pairs(&inn)
    );
    Ok(())
}

async fn writing_a_link_again_changes_its_fields_and_keeps_one_link(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?;
    let again = link(cat.as_ref(), &own, "i1", "p1", json!({"since": OCT}))
        .await
        .must("linking i1 to p1 again")?;
    ensure!(
        matches!(again, Outcome::Applied { version: Some(v), .. } if v.0 == 2),
        "writing the same link again changes it: its version is 2; the answer was {again:?}"
    );
    let links = all(cat.as_ref(), &own, LT).await?;
    ensure!(
        links.len() == 1,
        "one relation holds one link per pair; the link type holds {} records",
        links.len()
    );
    let out = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        out.len() == 1 && out[0].fields.get("since") == Some(&json!(OCT)),
        "the walk answers the one link with its new `since` ({OCT}); it answered {out:?}"
    );
    Ok(())
}

async fn an_unlinked_link_leaves_the_walk_and_stays_in_its_history(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let linked = link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?
        .position();
    cat.unlink(
        &own,
        &"invoice".into(),
        &"i1".into(),
        &"approved_by".into(),
        &"p1".into(),
        WriteOpts::default(),
    )
    .await
    .must("unlinking i1 from p1")?;
    let now = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        now.is_empty(),
        "after unlink the walk out of i1 reaches nothing; it answered {:?}",
        pairs(&now)
    );
    let then = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, Some(linked)).await?;
    ensure!(
        pairs(&then) == ["i1>p1"],
        "as of the link's position ({linked}) the walk still follows it; it answered {:?}",
        pairs(&then)
    );
    let missing = cat
        .unlink(
            &own,
            &"invoice".into(),
            &"i1".into(),
            &"approved_by".into(),
            &"p1".into(),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "unlinking a link that is not live is not found; it answered {missing:?}"
    );
    Ok(())
}

async fn a_link_that_would_break_the_relations_cardinality_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({}))
        .await
        .must("linking i1 to p1")?;
    // Many invoices to one partner is fine.
    link(cat.as_ref(), &own, "i2", "p1", json!({}))
        .await
        .must("linking i2 to p1 as well")?;
    let before = store.head().await.must("reading the head")?;
    let second = link(cat.as_ref(), &own, "i1", "p2", json!({})).await;
    ensure!(
        matches!(
            &second,
            Err(Error::Refused {
                field: Some(f),
                reason: Refusal::Constraint { .. },
            }) if f.as_str() == "to"
        ),
        "`approved_by` is many_to_one: i1 already links to p1, so linking it to p2 is \
         refused as a constraint on `to`; it answered {second:?}"
    );
    head_unmoved(store.as_ref(), before, "a link against the cardinality").await?;
    Ok(())
}

async fn a_link_to_a_key_that_is_no_live_record_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let dangling = link(cat.as_ref(), &own, "i1", "nobody", json!({})).await;
    ensure!(
        matches!(&dangling, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "to"),
        "no partner `nobody` exists, so the link is refused on `to`; it answered {dangling:?}"
    );
    let orphan = link(cat.as_ref(), &own, "nothing", "p1", json!({})).await;
    ensure!(
        matches!(&orphan, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "from"),
        "no invoice `nothing` exists, so the link is refused on `from`; it answered {orphan:?}"
    );
    head_unmoved(store.as_ref(), before, "a link to a missing record").await?;
    Ok(())
}

async fn link_records_go_through_the_link_doors_and_ingest_only(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let key = link_key(&"i1".into(), &"p1".into());
    let upserted = cat
        .upsert(
            &own,
            &LT.into(),
            &key,
            patch(json!({"from": "i1", "to": "p1"})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(
            &upserted,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "upsert on a link type is the wrong door; it answered {upserted:?}"
    );
    link(cat.as_ref(), &own, "i1", "p1", json!({}))
        .await
        .must("linking i1 to p1")?;
    let retired = cat
        .retire(&own, &LT.into(), &key, WriteOpts::default())
        .await;
    ensure!(
        matches!(
            &retired,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "retire on a link type is the wrong door; it answered {retired:?}"
    );
    let declared = cat
        .declare(
            &own,
            Declaration::Type(type_decl(
                "sys.link.invoice.forged",
                vec![field("from", StorageKind::Text)],
            )),
        )
        .await;
    ensure!(
        matches!(
            &declared,
            Err(Error::Refused {
                reason: Refusal::Reserved,
                ..
            })
        ),
        "a caller cannot declare a link type; it answered {declared:?}"
    );
    // A relation whose links arrive by ingest.
    let mut sourced = approved_by(vec![], Authority::Source);
    sourced.name = "vouched_by".into();
    sourced.inverse = None;
    sourced.cardinality = Cardinality::ManyToMany;
    declare(
        cat.as_ref(),
        &own,
        invoice(vec![
            approved_by(
                vec![field("since", StorageKind::DateTime)],
                Authority::Authored,
            ),
            sourced,
        ]),
    )
    .await?;
    let by_door = cat
        .link(
            &own,
            &"invoice".into(),
            &"i1".into(),
            &"vouched_by".into(),
            &"p2".into(),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(
            &by_door,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "`vouched_by` takes its links from a source, so the link door refuses it; it \
         answered {by_door:?}"
    );
    let vt = link_type_name(&"invoice".into(), &"vouched_by".into());
    ingest(
        cat.as_ref(),
        &own,
        "registry",
        vec![source_record(
            vt.as_str(),
            link_key(&"i1".into(), &"p2".into()).as_str(),
            json!({"from": "i1", "to": "p2"}),
        )],
    )
    .await
    .must("ingesting the link i1 → p2 of `vouched_by`")?;
    let mut p = Pattern::traverse(
        TypeName::from("invoice"),
        RecordKey::from("i1"),
        vec![Step {
            relation: "vouched_by".into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Depth { min: 1, max: 1 },
        }],
    );
    p.project = Projection::Graph;
    let reply = cat.query(&own, p).await.must("walking `vouched_by`")?;
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

async fn a_relation_cannot_move_between_via_and_link_records(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let mut moved = invoice(vec![RelationDecl {
        via: Some("approver".into()),
        fields: vec![],
        ..approved_by(vec![], Authority::Authored)
    }]);
    moved.fields.push(FieldDecl {
        ty: FieldType {
            target: Some("partner".into()),
            ..FieldType::kind(StorageKind::Reference)
        },
        ..field("approver", StorageKind::Text)
    });
    let answer = cat.declare(&own, Declaration::Type(moved)).await;
    ensure!(
        matches!(
            &answer,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "`approved_by` is carried by link records; giving it `via` would move its links, and \
         is refused as invalid; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a relation moved to `via`").await?;
    Ok(())
}

async fn a_link_field_the_principal_may_not_write_refuses_the_link(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", LT, &["from", "to"]);
    let refused = link(cat.as_ref(), &clerk, "i1", "p1", json!({"since": SEPT})).await;
    ensure!(
        matches!(&refused, Err(Error::Refused { field: Some(f), reason: Refusal::NotGranted }) if f.as_str() == "since"),
        "the clerk may write a link's ends but not its `since`; a link setting `since` is \
         refused on it; it answered {refused:?}"
    );
    link(cat.as_ref(), &clerk, "i1", "p1", json!({}))
        .await
        .must("the clerk linking i1 to p1 without `since`")?;
    Ok(())
}

async fn a_relations_fields_are_declared_when_added_and_migrated_when_retyped(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?;
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
    declare(cat.as_ref(), &own, with(StorageKind::Number)).await?;
    link(cat.as_ref(), &own, "i2", "p1", json!({"weight": 1.5}))
        .await
        .must("linking i2 to p1 with the new `weight`")?;
    // `weight` retyped to text, whose form is a string where a number's
    // was not: the links would reshape, so declaring is refused.
    let refused = cat
        .declare(&own, Declaration::Type(with(StorageKind::Text)))
        .await;
    ensure!(
        matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "retyping a link field reshapes the link records; declaring it is refused as a \
         migration; it answered {refused:?}"
    );
    let report = cat
        .migrate(
            &own,
            Migration {
                to: with(StorageKind::Text),
                renames: vec![],
            },
        )
        .await
        .must("preflighting the migration of `invoice` with `weight` as text")?;
    cat.migrate_apply(
        &own,
        Migration {
            to: with(StorageKind::Text),
            renames: vec![],
        },
        &report.data.report_id,
    )
    .await
    .must("applying the migration")?;
    let edges = walked(cat.as_ref(), &own, "partner", "p1", Walk::In, None).await?;
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

async fn retiring_an_end_keeps_the_link_and_the_walk_skips_the_retired_end(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?;
    cat.retire(&own, &"partner".into(), &"p1".into(), WriteOpts::default())
        .await
        .must("retiring p1")?;
    let mut p = Pattern::traverse(
        TypeName::from("invoice"),
        RecordKey::from("i1"),
        vec![Step {
            relation: "approved_by".into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Depth { min: 1, max: 1 },
        }],
    );
    p.project = Projection::Graph;
    let reply = cat.query(&own, p).await.must("walking from i1")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["i1"],
        "p1 is retired, so the walk reaches no partner; it reached {keys:?}"
    );
    let links = all(cat.as_ref(), &own, LT).await?;
    ensure!(
        links.len() == 1,
        "retiring an end keeps the link: the link type still holds it, but it holds {}",
        links.len()
    );
    Ok(())
}

async fn a_relation_that_allows_one_link_per_target_refuses_a_second(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let mut handled = approved_by(vec![], Authority::Authored);
    handled.name = "handled_by".into();
    handled.inverse = None;
    handled.cardinality = Cardinality::OneToMany;
    declare(
        cat.as_ref(),
        &own,
        invoice(vec![
            approved_by(
                vec![field("since", StorageKind::DateTime)],
                Authority::Authored,
            ),
            handled,
        ]),
    )
    .await?;
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
        matches!(
            &second,
            Err(Error::Refused {
                field: Some(f),
                reason: Refusal::Constraint { .. },
            }) if f.as_str() == "to"
        ),
        "`handled_by` is one_to_many: p1 is already handled by i1, so i2 cannot link to it; \
         the refusal is a constraint on `to`; it answered {second:?}"
    );
    head_unmoved(store.as_ref(), before, "a second link into one target").await?;
    Ok(())
}

async fn a_relations_authority_changes_by_declaration_and_keeps_its_links(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({}))
        .await
        .must("linking i1 to p1")?;
    let sourced = FieldDecl {
        authority: Authority::Source,
        ..field("since", StorageKind::DateTime)
    };
    declare(
        cat.as_ref(),
        &own,
        invoice(vec![approved_by(vec![sourced], Authority::Source)]),
    )
    .await?;
    let by_door = link(cat.as_ref(), &own, "i2", "p2", json!({})).await;
    ensure!(
        matches!(
            &by_door,
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ),
        "once `approved_by`'s links come from a source, the link door refuses them; it \
         answered {by_door:?}"
    );
    let kept = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        pairs(&kept) == ["i1>p1"],
        "the link written before the flip stays; the walk answered {:?}",
        pairs(&kept)
    );
    ingest(
        cat.as_ref(),
        &own,
        "registry",
        vec![source_record(
            LT,
            link_key(&"i2".into(), &"p2".into()).as_str(),
            json!({"since": SEPT}),
        )],
    )
    .await
    .must("ingesting the link i2 → p2, its ends from the key")?;
    let arrived = walked(cat.as_ref(), &own, "invoice", "i2", Walk::Out, None).await?;
    ensure!(
        pairs(&arrived) == ["i2>p2"],
        "the ingested link, its ends read from its key, is walked; the walk answered {:?}",
        pairs(&arrived)
    );
    Ok(())
}

async fn ingest_holds_a_source_relations_cardinality_over_the_lake_and_the_window(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // A source relation of its own, beside `approved_by`.
    let mut vouched = approved_by(vec![], Authority::Source);
    vouched.name = "vouched_by".into();
    vouched.inverse = None;
    let base = approved_by(
        vec![field("since", StorageKind::DateTime)],
        Authority::Authored,
    );
    declare(cat.as_ref(), &own, invoice(vec![base, vouched])).await?;
    let vt = link_type_name(&"invoice".into(), &"vouched_by".into());
    let record = |from: &str, to: &str, ends: bool| {
        let fields = if ends {
            json!({"from": from, "to": to})
        } else {
            json!({})
        };
        source_record(
            vt.as_str(),
            link_key(&from.into(), &to.into()).as_str(),
            fields,
        )
    };
    // Two links from i1 in one window: many_to_one breaks.
    let window = ingest(
        cat.as_ref(),
        &own,
        "registry",
        vec![record("i1", "p1", true), record("i1", "p2", true)],
    )
    .await;
    ensure!(
        matches!(
            &window,
            Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            })
        ),
        "two links from i1 in one window break `vouched_by`'s many_to_one; it answered \
         {window:?}"
    );
    ingest(
        cat.as_ref(),
        &own,
        "registry",
        vec![record("i1", "p1", true)],
    )
    .await
    .must("ingesting the link i1 → p1")?;
    // Against the lake, with the ends only in the key.
    let keyed = ingest(
        cat.as_ref(),
        &own,
        "registry",
        vec![record("i1", "p2", false)],
    )
    .await;
    ensure!(
        matches!(
            &keyed,
            Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            })
        ),
        "i1 already links to p1; a record keyed i1:p2 with no fields breaks the \
         cardinality too, its ends read from the key; it answered {keyed:?}"
    );
    Ok(())
}

async fn a_relation_declared_again_with_another_target_is_a_migration(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({}))
        .await
        .must("linking i1 to p1")?;
    // The relation removed: a migration, which keeps its links.
    let without = invoice(vec![]);
    let report = cat
        .migrate(
            &own,
            Migration {
                to: without.clone(),
                renames: vec![],
            },
        )
        .await
        .must("preflighting `invoice` without `approved_by`")?;
    cat.migrate_apply(
        &own,
        Migration {
            to: without,
            renames: vec![],
        },
        &report.data.report_id,
    )
    .await
    .must("migrating `approved_by` away")?;
    // Declared again, pointing at invoices now: the kept links would
    // reshape.
    let mut retargeted = approved_by(vec![], Authority::Authored);
    retargeted.target = "invoice".into();
    let before = store.head().await.must("reading the head")?;
    let answer = cat
        .declare(&own, Declaration::Type(invoice(vec![retargeted])))
        .await;
    ensure!(
        matches!(&answer, Err(Error::Refused { reason: Refusal::Invalid { message }, .. }) if message.contains(LT)),
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

async fn a_restricted_link_is_walked_only_with_the_restricted_grant(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    link(cat.as_ref(), &own, "i1", "p1", json!({"since": SEPT}))
        .await
        .must("linking i1 to p1")?;
    store
        .restrict(
            &LT.into(),
            &link_key(&"i1".into(), &"p1".into()),
            "hold".into(),
        )
        .await
        .must("restricting the link")?;
    let reader = crate::conformance::fixture::principal(
        store.as_ref(),
        lakelands_api::PrincipalKind::Agent,
        "reader",
        lakelands_api::Grants {
            default: lakelands_api::Access::Read,
            ..lakelands_api::Grants::default()
        },
    );
    let hidden = walked(cat.as_ref(), &reader, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        hidden.is_empty(),
        "the link is restricted and the reader holds no `restricted` grant, so the walk \
         does not follow it; it answered {:?}",
        pairs(&hidden)
    );
    let seen = walked(cat.as_ref(), &own, "invoice", "i1", Walk::Out, None).await?;
    ensure!(
        pairs(&seen) == ["i1>p1"],
        "the owner holds the `restricted` grant and walks the link; it answered {:?}",
        pairs(&seen)
    );
    Ok(())
}

async fn narrowing_a_relations_cardinality_is_refused_while_links_would_break(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let with = |c: Cardinality| {
        let mut r = approved_by(
            vec![field("since", StorageKind::DateTime)],
            Authority::Authored,
        );
        r.cardinality = c;
        invoice(vec![r])
    };
    declare(cat.as_ref(), &own, with(Cardinality::ManyToMany)).await?;
    for p in ["p1", "p2"] {
        link(cat.as_ref(), &own, "i1", p, json!({}))
            .await
            .must(&format!("linking i1 to {p}"))?;
    }
    let preflight = cat
        .migrate(
            &own,
            Migration {
                to: with(Cardinality::ManyToOne),
                renames: vec![],
            },
        )
        .await;
    ensure!(
        matches!(
            &preflight,
            Err(Error::Refused {
                reason: Refusal::Constraint { .. },
                ..
            })
        ),
        "i1 links to two partners, so narrowing `approved_by` to many_to_one would break \
         them; the preflight refuses it; it answered {preflight:?}"
    );
    Ok(())
}
