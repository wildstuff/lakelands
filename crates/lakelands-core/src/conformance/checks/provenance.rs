//! The two time axes of a read, and the provenance every field carries
//! (`docs/decisions/009-every-response-carries-its-position.md`,
//! `docs/decisions/010-provenance-per-field.md`, `docs/glossary.md`).

use std::sync::Arc;

use lakelands_api::ontology::{Authority, FieldDecl, StorageKind};
use lakelands_api::record::Origin;
use lakelands_api::{
    Error, Find, Get, LakeStore, Missing, Position, ReadAt, SourceId, Timestamp, TypeName,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field, field_writer, source_record, type_decl, upsert, windows_reach_head,
};

check!(
    /// Business time and transaction time are two axes.
    TWO_TIME_AXES = valid_at_and_as_of_combine_and_neither_stands_in_for_the_other,
    "009 and the glossary: `valid_at` asks what was true in the world, `as_of` what the lake knew; a read may name both"
);

check!(
    /// Every field says where its value came from.
    FIELD_PROVENANCE = every_field_carries_its_own_provenance,
    "010: with meta, each field answers its origin, known_at, position and writer, and a value from a source also its source and seen_at"
);

fn day(m: u32, d: u32) -> String {
    Timestamp::from_date(2026, m, d)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

async fn valid_at_and_as_of_combine_and_neither_stands_in_for_the_other(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut ty = type_decl(
        "contract",
        vec![
            field("party", StorageKind::Text),
            field("from", StorageKind::DateTime),
            field("to", StorageKind::DateTime),
        ],
    );
    ty.valid_from = Some("from".into());
    ty.valid_to = Some("to".into());
    l.declare(ty).await?;

    // Valid in January only, as first written; then extended to March.
    let first = l
        .upsert(
            "contract",
            "c",
            json!({"party": "acme", "from": day(1, 1), "to": day(2, 1)}),
        )
        .await?
        .position();
    l.upsert("contract", "c", json!({"to": day(4, 1)})).await?;

    let contract = TypeName::from("contract");
    let at = |as_of: Option<Position>, valid: String| ReadAt {
        as_of,
        valid_at: Timestamp::parse(&valid).ok(),
    };
    let found = |as_of, valid: String| {
        let cat = cat.clone();
        let own = own.clone();
        let contract = contract.clone();
        async move {
            let got = cat
                .get(
                    &own,
                    &contract,
                    Get {
                        key: "c".into(),
                        with_meta: false,
                        at: at(as_of, valid),
                        params: Default::default(),
                    },
                )
                .await;
            match got {
                Ok(_) => Ok(true),
                Err(Error::NotFound {
                    missing: Missing::Record { .. },
                }) => Ok(false),
                Err(e) => Err(Failure::new(format!("reading `c` failed: {e}"))),
            }
        }
    };

    let cases = [
        (None, day(1, 15), true, "January, as the lake knows it now"),
        (None, day(3, 15), true, "March, as the lake knows it now"),
        (None, day(5, 1), false, "May, after the contract ends"),
        (
            Some(first),
            day(1, 15),
            true,
            "January, as the lake knew it first",
        ),
        (
            Some(first),
            day(3, 15),
            false,
            "March, as the lake knew it before the extension",
        ),
        (
            None,
            day(2, 1),
            true,
            "the instant the first version ended, now extended",
        ),
    ];
    for (as_of, valid, want, what) in cases {
        let got = found(as_of, valid.clone()).await?;
        ensure!(
            got == want,
            "reading `c` valid at {valid} (as_of {as_of:?}): {what}: expected {}, but \
             it was {}. `valid_at` selects what held in the world and `as_of` what the \
             lake knew; together they must answer the earlier knowledge about that \
             instant",
            if want { "found" } else { "absent" },
            if got { "found" } else { "absent" }
        );
    }

    let find = Find {
        at: at(Some(first), day(3, 15)),
        ..Find::default()
    };
    let rows = l.all_found("contract", find).await?;
    ensure!(
        rows.is_empty(),
        "`find` valid at March as of the first write answered {} record(s); the lake \
         then knew the contract as ending on 1 February",
        rows.len()
    );
    Ok(())
}

async fn every_field_carries_its_own_provenance(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![
            FieldDecl {
                authority: Authority::Source,
                ..field("amount", StorageKind::Decimal)
            },
            field("status", StorageKind::Text),
        ],
    );
    l.declare(ty).await?;

    let seen = Timestamp::from_date(2026, 2, 3);
    let mut rec = source_record("invoice", "i", json!({"amount": "12.50"}));
    rec.seen_at = seen;
    let windows = l
        .ingest("bank", vec![rec])
        .await
        .must("ingesting `i` from `bank`")?;
    windows_reach_head(store.as_ref(), &windows, "ingesting `i` from `bank`").await?;
    let ingested = windows.last().map(|w| w.position);
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status"]);
    let authored = upsert(
        cat.as_ref(),
        &clerk,
        "invoice",
        "i",
        json!({"status": "open"}),
    )
    .await?
    .position();

    let r = l.read("invoice", "i").await?;
    let amount = r
        .meta
        .get("amount")
        .ok_or_else(|| Failure::new("reading `i` with meta answered no provenance for `amount`"))?;
    ensure!(
        amount.origin == Origin::Source
            && amount.source == Some(SourceId::from("bank"))
            && amount.seen_at == seen
            && Some(amount.position) == ingested
            && amount.writer == own.writer(),
        "`amount` came from source `bank`, seen {seen:?}, ingested at {ingested:?} by \
         {}; its provenance reads {amount:?}",
        own.writer()
    );
    let status = r
        .meta
        .get("status")
        .ok_or_else(|| Failure::new("reading `i` with meta answered no provenance for `status`"))?;
    ensure!(
        status.origin == Origin::Authored
            && status.source.is_none()
            && status.seen_at.is_none()
            && status.procedure_version.is_none()
            && status.position == authored
            && status.writer == clerk.writer(),
        "`status` was written by {} at {authored} through `upsert`; its provenance \
         must say authored, name that writer and position, and carry no source, \
         seen_at or procedure_version; it reads {status:?}",
        clerk.writer()
    );
    ensure!(
        status.known_at >= amount.known_at,
        "`status` was learned after `amount`, but its known_at {} is earlier than {}",
        status.known_at,
        amount.known_at
    );

    let plain = cat
        .get(
            own,
            &TypeName::from("invoice"),
            Get {
                key: "i".into(),
                with_meta: false,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .must("reading `i` without meta")?;
    ensure!(
        plain.data.meta.is_empty(),
        "a read that does not ask for meta answered provenance for {:?}",
        plain.data.meta.keys().collect::<Vec<_>>()
    );
    Ok(())
}
