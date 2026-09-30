//! A cursor is bound to the position of its first page
//! (`docs/glossary.md`, `docs/decisions/009-every-response-carries-its-position.md`).

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{Find, Found, LakeStore, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, Must, field, type_decl};

check!(
    /// Later pages read where the first page was read.
    CURSOR_STAYS = a_cursor_stays_on_the_position_of_its_first_page,
    "the glossary: a cursor is bound to the position its first page was read at, so a page set is stable while the lake is written"
);

async fn a_cursor_stays_on_the_position_of_its_first_page(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("item", vec![field("n", StorageKind::Number)]))
        .await?;
    for (i, k) in ["b", "c", "d", "e"].iter().enumerate() {
        l.upsert("item", k, json!({ "n": i })).await?;
    }
    let item = TypeName::from("item");
    let page = |cursor| Find {
        limit: Some(2),
        cursor,
        ..Find::default()
    };

    let first = cat
        .find(own, &item, page(None))
        .await
        .must("reading the first page")?;
    let Found::Records { rows, next_cursor } = first.data else {
        return Err(Failure::new("`find` without an aggregate answered groups"));
    };
    let mut seen: Vec<String> = rows.iter().map(|r| r.key.0.clone()).collect();
    let mut cursor = next_cursor
        .ok_or_else(|| Failure::new("four records in pages of two answered no next page"))?;

    // The lake moves on: a record that sorts first, a change and a retirement.
    l.upsert("item", "a", json!({"n": 9})).await?;
    l.upsert("item", "d", json!({"n": 99})).await?;
    cat.retire(own, &item, &"e".into(), WriteOpts::default())
        .await
        .must("retiring `e`")?;

    loop {
        let next = cat
            .find(own, &item, page(Some(cursor)))
            .await
            .must("reading a later page")?;
        ensure!(
            next.position == first.position,
            "a later page answered position {}, but its cursor is bound to the first \
             page's position {}",
            next.position,
            first.position
        );
        let Found::Records { rows, next_cursor } = next.data else {
            return Err(Failure::new("`find` without an aggregate answered groups"));
        };
        for r in &rows {
            if r.key.as_str() == "d" {
                ensure!(
                    r.fields.get("n") == Some(&json!(2)),
                    "`d` on a later page reads n = {:?}; at the first page's position it \
                     was 2",
                    r.fields.get("n")
                );
            }
        }
        seen.extend(rows.into_iter().map(|r| r.key.0));
        match next_cursor {
            Some(c) => cursor = c,
            None => break,
        }
    }
    ensure!(
        seen == ["b", "c", "d", "e"],
        "paging from a cursor must list the records as they were at the first page: \
         b, c, d, e; it listed {seen:?}"
    );
    Ok(())
}
