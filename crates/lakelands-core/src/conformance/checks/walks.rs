//! The one pattern query: steps over declared relations, bounded depth,
//! one order (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, RelationDecl, StorageKind};
use lakelands_api::query::{Depth, Pattern, QueryResult, Step, Walk};
use lakelands_api::{Error, LakeStore, RecordKey, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Must, declare, doors, field, limits, owner, type_decl, upsert};

check!(
    /// A step must follow a declared relation.
    UNDECLARED_RELATION = a_step_over_a_relation_the_type_does_not_declare_is_refused,
    "018: the planner knows the ontology; a step over an undeclared relation is refused, not answered empty"
);

check!(
    /// A walk may not go deeper than the lake allows.
    TOO_DEEP = a_step_deeper_than_the_lakes_limit_is_refused,
    "018 and the location's limits: on a lake opened with the default limits, a step whose depth exceeds `max_depth` is refused rather than walked"
);

check!(
    /// A walk orders what it reaches, whatever the engine.
    WALK_ORDER = a_walk_answers_what_it_reaches_in_one_order,
    "026 and 029: a graph includes its start, and a walk orders what it reaches by depth, type and key, so no engine's link order shows through"
);

/// People who report to people, through `boss`.
async fn org(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    let mut ty = type_decl(
        "person",
        vec![
            field("name", StorageKind::Text),
            FieldDecl {
                ty: FieldType {
                    target: Some("person".into()),
                    ..FieldType::kind(StorageKind::Reference)
                },
                ..field("boss", StorageKind::Text)
            },
        ],
    );
    ty.relations = vec![RelationDecl {
        name: "reports_to".into(),
        target: "person".into(),
        cardinality: Default::default(),
        inverse: Some("manages".into()),
        via: Some("boss".into()),
        fields: Vec::new(),
        description: None,
    }];
    declare(cat.as_ref(), &own, ty).await?;
    for (key, boss) in [
        ("ceo", None),
        ("vp", Some("ceo")),
        ("zed", Some("vp")),
        ("amy", Some("vp")),
    ] {
        let mut fields = json!({"name": key});
        if let Some(b) = boss {
            fields["boss"] = json!(b);
        }
        upsert(cat.as_ref(), &own, "person", key, fields).await?;
    }
    Ok(())
}

fn step(relation: &str, walk: Walk, min: u32, max: u32) -> Step {
    Step {
        relation: relation.into(),
        walk,
        ty: None,
        filter: None,
        depth: Depth { min, max },
    }
}

fn from(key: &str, steps: Vec<Step>) -> Pattern {
    Pattern::traverse(TypeName::from("person"), RecordKey::from(key), steps)
}

async fn a_step_over_a_relation_the_type_does_not_declare_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    org(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    cat.query(&own, from("zed", vec![step("reports_to", Walk::Out, 1, 1)]))
        .await
        .must("walking `reports_to` from `zed`")?;
    let err = cat
        .query(&own, from("zed", vec![step("audited_by", Walk::Out, 1, 1)]))
        .await
        .must_fail("walking the undeclared relation `audited_by`")?;
    ensure!(
        matches!(
            err,
            Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            }
        ),
        "a step over the undeclared relation `audited_by` must be refused as invalid; \
         it failed with {err:?}"
    );
    Ok(())
}

async fn a_step_deeper_than_the_lakes_limit_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    org(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let limit = limits().max_depth;
    cat.query(
        &own,
        from("zed", vec![step("reports_to", Walk::Out, 1, limit)]),
    )
    .await
    .must(&format!(
        "walking `reports_to` up to the default limit of {limit}"
    ))?;
    let err = cat
        .query(
            &own,
            from("zed", vec![step("reports_to", Walk::Out, 1, limit + 1)]),
        )
        .await
        .must_fail(&format!("walking `reports_to` {} deep", limit + 1))?;
    ensure!(
        matches!(err, Error::Refused { .. }),
        "a step {} deep, past the lake's max_depth of {limit}, must be refused; it \
         failed with {err:?}",
        limit + 1
    );
    Ok(())
}

async fn a_walk_answers_what_it_reaches_in_one_order(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    org(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let mut p = from("ceo", vec![step("reports_to", Walk::In, 1, 2)]);
    p.project = lakelands_api::query::Projection::Graph;
    let reply = cat
        .query(&own, p)
        .await
        .must("walking `reports_to` backwards two deep from `ceo`")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["ceo", "vp", "amy", "zed"],
        "walking down from `ceo` two deep must answer the start, then depth 1 (vp), \
         then depth 2 by key (amy, zed); it answered {keys:?}"
    );
    Ok(())
}
