//! `find` ordered by `latest`: the position of the event that made each
//! record's state at the read's position
//! (`docs/decisions/029-every-answer-has-one-order.md`, `docs/seams.md`
//! §`scan`). Every change a record's own events make moves it; a link
//! carried by a link record moves the link record, not its ends.

use std::sync::Arc;

use lakelands_api::ontology::{
    Authority, Cardinality, FieldDecl, FieldType, RelationDecl, StorageKind,
};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::write::WriteOpts;
use lakelands_api::{Find, LakeStore, Position, ReadAt};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all_found, declare, doors, field, ingest, owner, patch, source_record, type_decl, upsert,
};

check!(
    /// `latest` orders by each record's last event.
    LATEST_ORDER = find_ordered_by_latest_lists_records_by_their_last_event,
    "029 and seams §scan: `find` ordered by `latest` lists records by the position of the event that made each one's state: an upsert, an ingest and a reference field move a record, a link record moves only itself, a retired record leaves; ascending is the reverse, a cursor continues it, a filter the store evaluates keeps it, and a read at an earlier position answers the order as it was then"
);

fn latest(dir: Direction) -> Find {
    Find {
        order_by: vec![Order {
            field: "".into(),
            by: OrderBy::Latest,
            dir,
        }],
        ..Find::default()
    }
}

async fn find_ordered_by_latest_lists_records_by_their_last_event(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("partner", vec![field("name", StorageKind::Text)]),
    )
    .await?;
    let mut invoice = type_decl(
        "invoice",
        vec![
            field("amount", StorageKind::Decimal),
            field("note", StorageKind::Text),
            FieldDecl {
                authority: Authority::Source,
                ..field("seen", StorageKind::Text)
            },
            FieldDecl {
                ty: FieldType {
                    target: Some("partner".into()),
                    ..FieldType::kind(StorageKind::Reference)
                },
                ..field("billed", StorageKind::Text)
            },
        ],
    );
    let relation = |name: &str, via: Option<&str>| RelationDecl {
        name: name.into(),
        target: "partner".into(),
        cardinality: Cardinality::ManyToOne,
        inverse: None,
        via: via.map(Into::into),
        fields: vec![],
        authority: Authority::Authored,
        description: None,
    };
    invoice.relations = vec![
        relation("billed_by", Some("billed")),
        relation("approved_by", None),
    ];
    declare(cat.as_ref(), &own, invoice).await?;
    upsert(cat.as_ref(), &own, "partner", "p1", json!({"name": "one"})).await?;
    for i in ["i1", "i2", "i3", "i4", "i5"] {
        upsert(cat.as_ref(), &own, "invoice", i, json!({"amount": "10"})).await?;
    }
    // i1 changes another field; i2 then leaves; i3 changes by ingest; i4
    // gains a link record; i5 a reference.
    let noted = upsert(cat.as_ref(), &own, "invoice", "i1", json!({"note": "late"}))
        .await?
        .position();
    cat.retire(&own, &"invoice".into(), &"i2".into(), WriteOpts::default())
        .await
        .must("retiring i2")?;
    ingest(
        cat.as_ref(),
        &own,
        "erp",
        vec![source_record("invoice", "i3", json!({"seen": "yes"}))],
    )
    .await
    .must("ingesting a change to i3")?;
    cat.link(
        &own,
        &"invoice".into(),
        &"i4".into(),
        &"approved_by".into(),
        &"p1".into(),
        patch(json!({})),
        WriteOpts::default(),
    )
    .await
    .must("linking i4 to p1 by a link record")?;
    upsert(cat.as_ref(), &own, "invoice", "i5", json!({"billed": "p1"})).await?;

    let keys = |records: Vec<lakelands_api::Record>| -> Vec<String> {
        records.into_iter().map(|r| r.key.0).collect()
    };
    let newest = ["i5", "i3", "i1", "i4"];
    let desc = keys(all_found(cat.as_ref(), &own, "invoice", latest(Direction::Desc)).await?);
    ensure!(
        desc == newest,
        "ordered by `latest` descending, `find` answered {desc:?}; the last events are i5's \
         reference, i3's ingest, i1's note, then i4's own upsert (its link record moves only \
         the link), and i2 is retired, so {newest:?}"
    );
    let asc = keys(all_found(cat.as_ref(), &own, "invoice", latest(Direction::Asc)).await?);
    let reversed: Vec<&str> = newest.iter().rev().copied().collect();
    ensure!(
        asc == reversed,
        "ordered by `latest` ascending, `find` answered {asc:?}, not the reverse {reversed:?}"
    );
    let paged = keys(
        all_found(
            cat.as_ref(),
            &own,
            "invoice",
            Find {
                limit: Some(2),
                ..latest(Direction::Desc)
            },
        )
        .await?,
    );
    ensure!(
        paged == newest,
        "in pages of 2 with the cursor, `latest` descending answered {paged:?}, not {newest:?}"
    );
    let filtered = keys(
        all_found(
            cat.as_ref(),
            &own,
            "invoice",
            Find {
                filter: Some("amount > 0 and now() > now() - 1d".into()),
                limit: Some(2),
                ..latest(Direction::Desc)
            },
        )
        .await?,
    );
    ensure!(
        filtered == newest,
        "with a filter the store evaluates (it reads the clock), `latest` descending answered \
         {filtered:?}, not {newest:?}"
    );
    let then = keys(
        all_found(
            cat.as_ref(),
            &own,
            "invoice",
            Find {
                at: ReadAt {
                    as_of: Some(Position(noted.0)),
                    valid_at: None,
                },
                ..latest(Direction::Desc)
            },
        )
        .await?,
    );
    let as_then = ["i1", "i5", "i4", "i3", "i2"];
    ensure!(
        then == as_then,
        "read as of {noted}, when i1's note was the last change and nothing else had moved, \
         `latest` descending answered {then:?}, not {as_then:?}"
    );
    Ok(())
}
