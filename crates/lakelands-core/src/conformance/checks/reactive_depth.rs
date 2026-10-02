//! A chain of reactions stops at the lake's ceiling
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).
//!
//! A host reacts to what the change feed tells it and writes its
//! reaction through a catalog bound to a reactive context one deeper than
//! the write it reacts to. The store refuses a write deeper than the
//! lake's `max_reactive_depth`, so a trigger that feeds itself stops
//! there. The check plays that host, over the feed, with the ceiling the
//! lake was opened with.

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Catalog, Error, LakeStore, Position, RecordKey, Refusal, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, head_unmoved, limits, owner, patch, reason, type_decl,
};

check!(
    /// A reaction past the ceiling is refused `TooDeep`.
    TOO_DEEP = a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing,
    "021: a host writes each reaction one deeper than its cause; the store takes every write up to the lake's max_reactive_depth and refuses the next as too_deep naming the ceiling, an upsert and a retire alike and whatever origin the context names, appending nothing, while the log keeps the chain before it with each write's depth"
);

fn at_depth(depth: u32) -> WriteContext {
    WriteContext {
        cause: None,
        origin: if depth == 0 {
            WriteOrigin::Direct
        } else {
            WriteOrigin::Reactive
        },
        depth,
    }
}

fn step_key(n: u32) -> RecordKey {
    RecordKey::from(format!("s{n}").as_str())
}

/// The key of the last `step` the feed tells of after `from`, and where
/// to read from next: what the host reacts to.
async fn last_step(
    store: &dyn LakeStore,
    from: Position,
) -> Result<(Option<(RecordKey, u32)>, Position), Failure> {
    let batch = store
        .feed()
        .poll(from, &FeedFilter::default(), limits().max_limit)
        .await
        .must(&format!("polling the feed from {from}"))?;
    let mut last = None;
    for e in &batch.events {
        if let EventBody::RecordUpserted { ty, key, .. } = &e.body
            && ty.as_str() == "step"
        {
            last = Some((key.clone(), e.context.depth));
        }
    }
    Ok((last, batch.next))
}

async fn react(
    store: &dyn LakeStore,
    depth: u32,
) -> lakelands_api::Result<lakelands_api::write::Outcome> {
    let cat: Arc<dyn Catalog> = store.catalog(at_depth(depth));
    cat.upsert(
        &owner(store),
        &TypeName::from("step"),
        &step_key(depth),
        patch(json!({ "n": depth })),
        WriteOpts::default(),
    )
    .await
}

async fn a_chain_of_reactions_past_the_lakes_ceiling_is_refused_too_deep_and_appends_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let max = limits().max_reactive_depth;
    let own = owner(store.as_ref());
    declare(
        doors(store.as_ref()).as_ref(),
        &own,
        type_decl("step", vec![field("n", StorageKind::Number)]),
    )
    .await?;

    // The direct write that starts the chain, then one reaction per
    // change the feed tells of, each one deeper than its cause.
    react(store.as_ref(), 0)
        .await
        .must("the direct write of `s0`")?;
    let mut from = Position::GENESIS;
    for depth in 1..=max + 1 {
        let (cause, next) = last_step(store.as_ref(), from).await?;
        from = next;
        let expected = (step_key(depth - 1), depth - 1);
        ensure!(
            cause.as_ref() == Some(&expected),
            "the feed told of {cause:?} as the last `step` written; the chain wrote `s{}` at \
             depth {} last",
            depth - 1,
            depth - 1
        );
        let head = store.head().await.must("reading the head")?;
        match react(store.as_ref(), depth).await {
            Ok(_) if depth <= max => {}
            Ok(o) => {
                return Err(Failure::new(format!(
                    "a reaction at depth {depth}, past the lake's ceiling of {max}, was \
                     written at {}; the store refuses it as too_deep",
                    o.position()
                )));
            }
            Err(Error::Refused {
                reason: Refusal::TooDeep { max: named },
                ..
            }) if depth > max => {
                ensure!(
                    named == max,
                    "the reaction at depth {depth} was refused as too_deep naming a ceiling \
                     of {named}; the lake's is {max}"
                );
                head_unmoved(store.as_ref(), head, "the reaction past the ceiling").await?;
            }
            Err(e) => {
                return Err(Failure::new(format!(
                    "a reaction at depth {depth} (the lake's ceiling is {max}) answered {e} \
                     ({e:?}); up to the ceiling it is written, past it refused as too_deep"
                )));
            }
        }
    }

    // Every door that writes holds the ceiling: a retire past it too.
    let head = store.head().await.must("reading the head")?;
    let retired = store
        .catalog(at_depth(max + 1))
        .retire(
            &own,
            &TypeName::from("step"),
            &step_key(0),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        matches!(reason(&retired), Some(Refusal::TooDeep { max: named }) if *named == max),
        "retiring `s0` at depth {} (the lake's ceiling is {max}) answered {retired:?}; it is \
         refused as too_deep naming the ceiling",
        max + 1
    );
    head_unmoved(store.as_ref(), head, "the retire past the ceiling").await?;

    // The depth is the chain's, whatever origin the context names: a
    // direct or scheduled context past the ceiling is refused the same.
    for origin in [WriteOrigin::Direct, WriteOrigin::Scheduled] {
        let head = store.head().await.must("reading the head")?;
        let context = WriteContext {
            cause: None,
            origin,
            depth: max + 1,
        };
        let written = store
            .catalog(context)
            .upsert(
                &own,
                &TypeName::from("step"),
                &step_key(max + 1),
                patch(json!({ "n": max + 1 })),
                WriteOpts::default(),
            )
            .await;
        ensure!(
            matches!(reason(&written), Some(Refusal::TooDeep { max: named }) if *named == max),
            "an upsert with a {origin:?} context at depth {} (the lake's ceiling is {max}) \
             answered {written:?}; the depth is the chain's whatever the origin, and it is \
             refused as too_deep naming the ceiling",
            max + 1
        );
        head_unmoved(store.as_ref(), head, "the write past the ceiling").await?;
    }

    // The log keeps the chain up to the ceiling, each write with its
    // depth, and nothing past it.
    let log = store
        .feed()
        .poll(
            Position::GENESIS,
            &FeedFilter::default(),
            limits().max_limit,
        )
        .await
        .must("reading the log")?;
    let chain: Vec<(String, u32, WriteOrigin)> = log
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted { ty, key, .. } if ty.as_str() == "step" => {
                Some((key.to_string(), e.context.depth, e.context.origin))
            }
            _ => None,
        })
        .collect();
    let expected: Vec<(String, u32, WriteOrigin)> = (0..=max)
        .map(|d| (format!("s{d}"), d, at_depth(d).origin))
        .collect();
    ensure!(
        chain == expected,
        "the log holds the `step` writes {chain:?}; it holds `s0` to `s{max}`, each with its \
         depth and origin, and nothing past the ceiling"
    );
    Ok(())
}
