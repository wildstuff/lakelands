//! A cursor is bound to the position of its first page
//! (`docs/glossary.md`, `docs/decisions/009-every-response-carries-its-position.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult, Search, SearchMode};
use lakelands_api::read::{Direction, OrderBy};
use lakelands_api::{Cursor, Find, Found, LakeStore, ReadAt, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, field, invalid_naming, is_invalid, order, pattern, type_decl,
};

check!(
    /// A cursor continues only the answer it was handed out for.
    CURSOR_ONE_ANSWER = a_cursor_continues_only_the_answer_it_was_handed_out_for,
    "018 and 029: a page continues only the answer its cursor was handed out for, the same door and the same type: a cursor of `find` on one type continues that `find`, and is refused as invalid by `find` on another type, by `query` and by `search`, naming where it belongs; a cursor of `query` or `search` is refused by `find` alike"
);

check!(
    /// A cursor continues only the order it was handed out for.
    CURSOR_ONE_ORDER = a_cursor_handed_to_a_find_of_another_order_length_is_refused,
    "018 and lakelands#436: a cursor of `find` holds one sort value per key of the `order_by` its first page was read with; handed to a `find` whose `order_by` is longer or shorter, on a stored type or on a view, it is refused as invalid, naming the order, and never answers a page or `unavailable`"
);

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
    let Found::Records {
        rows, next_cursor, ..
    } = first.data
    else {
        return Err(Failure::new("`find` without an aggregate answered groups"));
    };
    let mut seen: Vec<String> = rows.iter().map(|r| r.key.0.clone()).collect();
    let mut cursor = next_cursor
        .ok_or_else(|| Failure::new("four records in pages of two answered no next page"))?;

    // The lake moves on: a record that sorts first, a change and a retirement.
    l.upsert("item", "a", json!({"n": 9})).await?;
    l.upsert("item", "d", json!({"n": 99})).await?;
    l.retire(item.as_str(), "e").await?;

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
        let Found::Records {
            rows, next_cursor, ..
        } = next.data
        else {
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

async fn a_cursor_continues_only_the_answer_it_was_handed_out_for(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for ty in ["alpha", "beta"] {
        let name = FieldDecl {
            searchable: true,
            ..field("name", StorageKind::Text)
        };
        l.declare(type_decl(ty, vec![name])).await?;
        for k in ["a", "b", "c"] {
            l.upsert(ty, k, json!({"name": "same words"})).await?;
        }
    }
    let find = |cursor| Find {
        limit: Some(1),
        cursor,
        ..Find::default()
    };
    let query = |cursor| {
        let mut p = pattern(
            "alpha",
            None,
            Vec::new(),
            Projection::Rows { fields: vec![] },
        );
        (p.limit, p.cursor) = (Some(1), cursor);
        p
    };
    let search = |cursor| Search {
        ty: "alpha".into(),
        mode: SearchMode::lexical("same"),
        k: 1,
        cursor,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let (alpha, beta) = (TypeName::from("alpha"), TypeName::from("beta"));
    let next = |c: Option<Cursor>, what: &str| {
        c.ok_or_else(|| Failure::new(format!("{what} hands out a cursor")))
    };
    let of_find = match cat
        .find(own, &alpha, find(None))
        .await
        .must("finding alpha")?
        .data
    {
        Found::Records { next_cursor, .. } => next(next_cursor, "a page of `find`")?,
        Found::Groups { .. } => return Err(Failure::new("`find` answered groups")),
    };
    let of_query = match cat
        .query(own, query(None))
        .await
        .must("querying alpha")?
        .data
    {
        QueryResult::Rows { next_cursor, .. } => next(next_cursor, "a page of `query`")?,
        _ => return Err(Failure::new("a pattern's rows answered something else")),
    };
    let of_search = next(
        cat.search(own, search(None))
            .await
            .must("searching alpha")?
            .data
            .next_cursor,
        "a page of `search`",
    )?;
    cat.find(own, &alpha, find(Some(of_find.clone())))
        .await
        .must("continuing `find` on alpha with its own cursor")?;
    let refused = [
        (
            "`find` on beta, with a cursor of `find` on alpha",
            cat.find(own, &beta, find(Some(of_find.clone())))
                .await
                .map(drop),
        ),
        (
            "`query`, with a cursor of `find`",
            cat.query(own, query(Some(of_find.clone()))).await.map(drop),
        ),
        (
            "`search`, with a cursor of `find`",
            cat.search(own, search(Some(of_find))).await.map(drop),
        ),
        (
            "`find`, with a cursor of `query`",
            cat.find(own, &alpha, find(Some(of_query))).await.map(drop),
        ),
        (
            "`find`, with a cursor of `search`",
            cat.find(own, &alpha, find(Some(of_search))).await.map(drop),
        ),
    ];
    for (what, r) in refused {
        ensure!(
            is_invalid(&r) && format!("{r:?}").contains("handed out for"),
            "{what} continues another answer, so it is refused as invalid, naming where the \
             cursor belongs; it answered {r:?}"
        );
    }
    Ok(())
}

async fn a_cursor_handed_to_a_find_of_another_order_length_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("doc", vec![field("t", StorageKind::Text)]))
        .await?;
    l.declare(count_view("by_t", "doc", "t")).await?;
    for (k, t) in [("a", "z"), ("b", "y"), ("c", "x"), ("d", "w")] {
        l.upsert("doc", k, json!({ "t": t })).await?;
    }
    let by_t = || vec![order("t", OrderBy::Value, Direction::Asc)];
    let page = |order_by, cursor| Find {
        order_by,
        limit: Some(2),
        cursor,
        ..Find::default()
    };
    // Every case is tried and every wrong answer named: on a view, the
    // longer order once panicked the read and answered `unavailable`.
    let mut wrong = Vec::new();
    for ty in ["doc", "by_t"] {
        let ty = TypeName::from(ty);
        // By key, then by `t`: a cursor with no sort value handed to a
        // longer order, and one with a sort value handed to a shorter one.
        for (first, then) in [(Vec::new(), by_t()), (by_t(), Vec::new())] {
            let cursor = match cat
                .find(own, &ty, page(first.clone(), None))
                .await
                .must(&format!("the first page of `{ty}`"))?
                .data
            {
                Found::Records {
                    next_cursor: Some(c),
                    ..
                } => c,
                other => {
                    return Err(Failure::new(format!(
                        "four records of `{ty}` in pages of two answered no next page: \
                         {other:?}"
                    )));
                }
            };
            let what = format!(
                "a cursor of `{ty}` read with {} key(s) of `order_by`, handed to a `find` with {}",
                first.len(),
                then.len()
            );
            let answer = cat.find(own, &ty, page(then, Some(cursor))).await;
            if let Err(f) = invalid_naming(&what, answer, &["another order", "order_by"]) {
                wrong.push(f.to_string());
            }
        }
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}
