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
    "001 and the engine seam: a projection rebuilt from the log is the one it replaces, byte for byte, at the head and as of every earlier position"
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

    let head = store.head().await.must("reading the head")?;
    let live = snapshot(store.as_ref(), &own, &positions).await?;
    let through = store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        through == head,
        "the lake's head is {head}, but the projection was rebuilt through {through}; a \
         rebuild folds the whole log"
    );
    let rebuilt = snapshot(store.as_ref(), &own, &positions).await?;

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
