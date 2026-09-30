//! The projection is a product of the log
//! (`docs/decisions/001-the-log-is-the-truth.md`, `docs/seams.md`).
//!
//! Rebuilding a lake's projection from its log gives the state it
//! replaces, byte for byte: every record, every version, every piece of
//! provenance, every derived field and view record, at the head and at
//! every earlier position. Anything the projection holds that the log
//! cannot reproduce would be lost on the first rebuild, which is why the
//! check reads everything before and after and compares.

use std::sync::Arc;

use lakelands_api::ontology::{Authority, Measure, StorageKind, ViewDecl};
use lakelands_api::{FieldName, Find, LakeStore, Position, Principal, ReadAt, Record, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all_found, declare, doors, field, get_at, owner, type_decl, upsert,
};

check!(
    /// A rebuilt projection answers every read as before.
    REBUILD_ANSWERS_AS_BEFORE = a_rebuilt_projection_answers_every_read_as_the_live_one_did,
    "001, 040 and the engine seam: a projection rebuilt from the log is the one it replaces, byte for byte, at the head and as of every earlier position, a type migration that converts records and a re-key that rewrites references to moved keys included"
);

check!(
    /// A rebuild leaves the chain as it was.
    REBUILD_REPRODUCES_THE_CHAIN = a_rebuild_reproduces_the_chain,
    "001 and 020: the log is the truth and a rebuild only re-derives the projection, so verifying the chain answers the same range and hash before and after"
);

/// Everything the check reads: every record of every type with
/// provenance at the head, and `invoice` `i1` as of every write.
async fn snapshot(
    store: &dyn LakeStore,
    who: &Principal,
    positions: &[Position],
) -> Result<Vec<(String, Vec<Record>)>, Failure> {
    let cat = doors(store);
    let mut out = Vec::new();
    for ty in ["invoice", "partner_totals"] {
        let find = Find {
            with_meta: true,
            ..Find::default()
        };
        out.push((ty.to_owned(), all_found(cat.as_ref(), who, ty, find).await?));
    }
    for at in positions {
        let find = Find {
            with_meta: true,
            at: ReadAt {
                as_of: Some(*at),
                valid_at: None,
            },
            ..Find::default()
        };
        let label = format!("invoice as of {at}");
        out.push((label, all_found(cat.as_ref(), who, "invoice", find).await?));
        let got = cat
            .get(who, &TypeName::from("invoice"), get_at("i1", Some(*at)))
            .await
            .must(&format!("reading `i1` as of {at}"))?;
        out.push((format!("i1 as of {at}"), vec![got.data]));
    }
    Ok(out)
}

async fn a_rebuilt_projection_answers_every_read_as_the_live_one_did(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());

    let mut gross = field("gross", StorageKind::Decimal);
    gross.authority = Authority::Derived;
    gross.derived = Some("amount * 1.19".into());
    gross.depends_on = vec![FieldName::from("amount")];
    let invoice = type_decl(
        "invoice",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
            gross,
        ],
    );
    declare(cat.as_ref(), &own, invoice).await?;
    let mut view = type_decl(
        "partner_totals",
        vec![
            field("partner", StorageKind::Text),
            field("total", StorageKind::Decimal),
        ],
    );
    view.key = vec![FieldName::from("partner")];
    view.view = Some(ViewDecl {
        source: TypeName::from("invoice"),
        r#where: None,
        group_by: vec![FieldName::from("partner")],
        measures: vec![Measure {
            name: FieldName::from("total"),
            expr: "sum(amount)".into(),
        }],
    });

    let mut positions = Vec::new();
    for (nr, partner, amount) in [("i1", "p1", "10.00"), ("i2", "p1", "5.50")] {
        let written = upsert(
            cat.as_ref(),
            &own,
            "invoice",
            nr,
            json!({"partner": partner, "amount": amount}),
        )
        .await?;
        positions.push(written.position());
    }
    // Declared after records exist, then kept by later writes: a rebuild
    // has to redo both.
    declare(cat.as_ref(), &own, view).await?;
    for (nr, fields) in [
        ("i1", json!({"amount": "20.00"})),
        ("i3", json!({"partner": "p2", "amount": "3.00"})),
        ("i1", json!({"partner": null})),
    ] {
        positions.push(
            upsert(cat.as_ref(), &own, "invoice", nr, fields)
                .await?
                .position(),
        );
    }
    let retired = cat
        .retire(
            &own,
            &TypeName::from("invoice"),
            &"i2".into(),
            Default::default(),
        )
        .await
        .must("retiring `i2`")?;
    positions.push(retired.position());

    // A type migration that converts every record, and a re-key that moves
    // records and rewrites the references to them: a rebuild folds them in
    // the same page as the records they change.
    let migrated = migrations(store.as_ref()).await?;
    let head = store.head().await.must("reading the head")?;
    let mut live = snapshot(store.as_ref(), &own, &positions).await?;
    live.extend(migrated_snapshot(store.as_ref(), &own, &migrated).await?);
    let through = store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        through == head,
        "the lake's head is {head}, but the projection was rebuilt through {through}; a \
         rebuild folds the whole log"
    );
    let mut rebuilt = snapshot(store.as_ref(), &own, &positions).await?;
    rebuilt.extend(migrated_snapshot(store.as_ref(), &own, &migrated).await?);

    for ((label, before), (_, after)) in live.iter().zip(&rebuilt) {
        ensure!(
            before == after,
            "after the rebuild, `{label}` reads differently.\n  before: {before:?}\n  \
             after:  {after:?}\nA projection rebuilt from the log must match the one it \
             replaces in every value, version and piece of provenance"
        );
    }
    Ok(())
}

/// A `memo` type whose `pages` a migration turns from number to text,
/// then the re-key fixture (`account` re-keyed by its IBAN, `payment`
/// referencing it). Answers the positions of both migrations and the head.
async fn migrations(store: &dyn LakeStore) -> Result<Vec<Position>, Failure> {
    use lakelands_api::ontology::FieldType;
    use lakelands_api::write::Migration;
    let cat = doors(store);
    let own = owner(store);
    let memo = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    declare(cat.as_ref(), &own, memo.clone()).await?;
    for (k, pages) in [("a", 3), ("b", 12)] {
        upsert(cat.as_ref(), &own, "memo", k, json!({"pages": pages})).await?;
    }
    let mut to = memo;
    for f in &mut to.fields {
        if f.name.as_str() == "pages" {
            f.ty = FieldType::kind(StorageKind::Text);
        }
    }
    let m = Migration {
        to,
        renames: Vec::new(),
    };
    let report = cat
        .migrate(&own, m.clone())
        .await
        .must("the preflight of retyping `pages`")?
        .data;
    let retyped = cat
        .migrate_apply(&own, m, &report.report_id)
        .await
        .must("retyping `pages` from number to text")?
        .position();
    super::rekey::lake(store, super::rekey::IBANS).await?;
    let rekeyed = super::rekey::apply(store).await?;
    let head = store.head().await.must("reading the head")?;
    Ok(vec![retyped, rekeyed, head])
}

/// Every record of the migrated types, with provenance, as of each of
/// `positions` (those [`migrations`] answers).
async fn migrated_snapshot(
    store: &dyn LakeStore,
    who: &Principal,
    positions: &[Position],
) -> Result<Vec<(String, Vec<Record>)>, Failure> {
    let cat = doors(store);
    let mut out = Vec::new();
    for (i, at) in positions.iter().enumerate() {
        // `account` and `payment` exist from the re-key's fixture on.
        let types: &[&str] = if i == 0 {
            &["memo"]
        } else {
            &["memo", "account", "payment"]
        };
        for ty in types {
            let find = Find {
                with_meta: true,
                at: ReadAt {
                    as_of: Some(*at),
                    valid_at: None,
                },
                ..Find::default()
            };
            let label = format!("{ty} as of {at}");
            out.push((label, all_found(cat.as_ref(), who, ty, find).await?));
        }
    }
    Ok(out)
}

async fn a_rebuild_reproduces_the_chain(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("memo", vec![field("text", StorageKind::Text)]),
    )
    .await?;
    for (k, t) in [("a", "one"), ("b", "two"), ("a", "three")] {
        upsert(cat.as_ref(), &own, "memo", k, json!({ "text": t })).await?;
    }
    let before = store.verify(None, None).await.must("verifying the chain")?;
    store.rebuild().await.must("rebuilding the projection")?;
    let after = store
        .verify(None, None)
        .await
        .must("verifying the chain after the rebuild")?;
    ensure!(
        before == after,
        "verifying the whole chain answered {before:?} before the rebuild and {after:?} \
         after it; a rebuild must leave the log, and so its chain, as it was"
    );
    Ok(())
}

check!(
    /// Every index answers as before after a rebuild.
    INDEXES_AFTER_REBUILD = after_a_rebuild_every_index_answers_as_before,
    "001 and the engine seam: the projection and its indexes are derived from the log, so after a rebuild a find by a field, a lexical and a vector search and a walk over a relation answer as they did"
);

async fn after_a_rebuild_every_index_answers_as_before(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::{FieldType, RelationDecl};
    use lakelands_api::query::{Projection, Search, SearchMode, Step, Walk};
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let mut text = field("text", StorageKind::Text);
    text.searchable = true;
    let embedding = lakelands_api::ontology::FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("embedding", StorageKind::Text)
    };
    let parent = lakelands_api::ontology::FieldDecl {
        ty: FieldType {
            target: Some("note".into()),
            ..FieldType::kind(StorageKind::Reference)
        },
        ..field("parent", StorageKind::Text)
    };
    let mut ty = type_decl("note", vec![text, embedding, parent]);
    ty.language = Some("en".into());
    ty.relations = vec![RelationDecl {
        annotations: Default::default(),
        name: "under".into(),
        target: "note".into(),
        cardinality: Default::default(),
        inverse: None,
        via: Some("parent".into()),
        fields: Vec::new(),
        authority: Default::default(),
        description: None,
    }];
    declare(cat.as_ref(), &own, ty).await?;
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "root",
        json!({"text": "harbour plans", "embedding": [1.0, 0.0]}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "leaf",
        json!({"text": "crane rental", "embedding": [0.0, 1.0], "parent": "root"}),
    )
    .await?;

    let answers = |store: Arc<dyn LakeStore>| async move {
        let cat = doors(store.as_ref());
        let own = owner(store.as_ref());
        let found = all_found(
            cat.as_ref(),
            &own,
            "note",
            Find {
                filter: Some("text = 'crane rental'".into()),
                ..Find::default()
            },
        )
        .await?;
        let search = |mode: SearchMode| Search {
            ty: TypeName::from("note"),
            mode,
            k: 10,
            cursor: None,
            filter: None,
            with_meta: false,
            at: ReadAt::default(),
        };
        let lexical = cat
            .search(
                &own,
                search(SearchMode::Lexical {
                    text: "harbour".into(),
                }),
            )
            .await
            .must("searching \"harbour\"")?
            .data;
        let vector = cat
            .search(
                &own,
                search(SearchMode::Vector {
                    field: "embedding".into(),
                    vector: vec![0.0, 1.0],
                }),
            )
            .await
            .must("searching near (0, 1)")?
            .data;
        let mut walk = lakelands_api::query::Pattern::traverse(
            TypeName::from("note"),
            "leaf".into(),
            vec![Step {
                relation: "under".into(),
                walk: Walk::Out,
                ty: None,
                filter: None,
                depth: Default::default(),
            }],
        );
        walk.project = Projection::Graph;
        let graph = cat
            .query(&own, walk)
            .await
            .must("walking `under` from `leaf`")?
            .data;
        // What each index answers: which records, in which order, with
        // which scores, and which links.
        let ranked = |h: &lakelands_api::query::Hits| {
            h.hits
                .iter()
                .map(|h| serde_json::json!({"key": h.record.key, "score": h.score}))
                .collect::<Vec<_>>()
        };
        let (nodes, edges) = match graph {
            lakelands_api::query::QueryResult::Graph { nodes, edges, .. } => (
                nodes.into_iter().map(|n| n.key.0).collect::<Vec<_>>(),
                serde_json::to_value(&edges).unwrap_or_default(),
            ),
            other => return Err(Failure::new(format!("a graph walk answered {other:?}"))),
        };
        Ok::<_, Failure>(serde_json::json!({
            "found": found.iter().map(|r| r.key.0.clone()).collect::<Vec<_>>(),
            "lexical": ranked(&lexical),
            "vector": ranked(&vector),
            "nodes": nodes,
            "edges": edges,
        }))
    };
    let before = answers(store.clone()).await?;
    ensure!(
        before["found"] == json!(["leaf"])
            && before["lexical"][0]["key"] == json!("root")
            && before["vector"][0]["key"] == json!("leaf")
            && before["edges"].as_array().is_some_and(|e| e.len() == 1),
        "before the rebuild the indexes must answer: find `leaf`, \"harbour\" → root, \
         near (0, 1) → leaf, one edge leaf → root; they answered {before}"
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let after = answers(store.clone()).await?;
    ensure!(
        before == after,
        "after the rebuild the indexes must answer as before: {before} became {after}"
    );
    Ok(())
}

check!(
    /// A value reads in its one JSON form, live and rebuilt.
    ONE_JSON_FORM = a_number_and_a_vector_read_back_in_their_one_json_form_before_and_after_a_rebuild,
    "the log format §1.2 and 001: every value a door answers is in its canonical JSON form (`1`, never `1.0`), so a read answers the same text live and after a rebuild"
);

async fn a_number_and_a_vector_read_back_in_their_one_json_form_before_and_after_a_rebuild(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::{FieldDecl, FieldType};
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let v = FieldDecl {
        ty: FieldType {
            dimensions: Some(3),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("v", StorageKind::Text)
    };
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "sample",
            vec![
                field("n", StorageKind::Number),
                field("m", StorageKind::Number),
                v,
            ],
        ),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "sample",
        "s",
        json!({"n": 2.0, "m": 1.5, "v": [0.0, 1.0, 0.25]}),
    )
    .await?;
    let text = |store: Arc<dyn LakeStore>| async move {
        let r = doors(store.as_ref())
            .get(
                &owner(store.as_ref()),
                &TypeName::from("sample"),
                get_at("s", None),
            )
            .await
            .must("reading `s`")?
            .data;
        Ok::<_, Failure>(serde_json::to_string(&r.fields).unwrap_or_default())
    };
    let canonical = r#"{"m":1.5,"n":2,"v":[0,1,0.25]}"#;
    let live = text(store.clone()).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = text(store.clone()).await?;
    ensure!(
        live == canonical && rebuilt == canonical,
        "`s` must read {canonical} (numbers in their one form) live and after a \
         rebuild; it read {live} live and {rebuilt} after"
    );
    Ok(())
}
