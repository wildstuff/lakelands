//! A cursor is bound to the position of its first page
//! (`docs/glossary.md`, `docs/decisions/009-every-response-carries-its-position.md`).

use std::sync::Arc;

use lakelands_api::error::Case;
use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult, Search, SearchMode};
use lakelands_api::read::{Direction, OrderBy};
use lakelands_api::{Cursor, Find, Found, LakeStore, ReadAt, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, field, invalid_case, order, pattern, type_decl,
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
    /// A cursor continues only the read it was handed out for.
    CURSOR_ONE_READ = a_cursor_continues_only_the_read_it_was_handed_out_for,
    "018 and lakelands#419: a cursor carries a digest of the read it was handed out for, so `find` under another `where` or an `order_by` of the same length, `search` for other words or in named fields, and `query` with another start `where` or projection refuse it as invalid; another page size, `with_meta` or `total` continue it"
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
            ("find", "find", &beta),
        ),
        (
            "`query`, with a cursor of `find`",
            cat.query(own, query(Some(of_find.clone()))).await.map(drop),
            ("find", "query", &alpha),
        ),
        (
            "`search`, with a cursor of `find`",
            cat.search(own, search(Some(of_find))).await.map(drop),
            ("find", "search", &alpha),
        ),
        (
            "`find`, with a cursor of `query`",
            cat.find(own, &alpha, find(Some(of_query))).await.map(drop),
            ("query", "find", &alpha),
        ),
        (
            "`find`, with a cursor of `search`",
            cat.find(own, &alpha, find(Some(of_search))).await.map(drop),
            ("search", "find", &alpha),
        ),
    ];
    for (what, r, (was, door, ty)) in refused {
        let belongs = Case::CursorOtherDoor {
            was: was.into(),
            was_ty: Some(Box::new(alpha.clone())),
            door: door.into(),
            ty: Some(Box::new(ty.clone())),
        };
        ensure!(
            invalid_case(&r) == Some(&belongs),
            "{what} continues another answer, so it is refused as invalid, naming where the \
             cursor belongs ({belongs}); it answered {r:?}"
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
            if let Err(f) = refused_as!(&what, answer, Case::CursorOtherOrder) {
                wrong.push(f.to_string());
            }
        }
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

async fn a_cursor_continues_only_the_read_it_was_handed_out_for(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let name = FieldDecl {
        searchable: true,
        ..field("name", StorageKind::Text)
    };
    l.declare(type_decl(
        "item",
        vec![name, field("n", StorageKind::Number)],
    ))
    .await?;
    for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
        l.upsert("item", k, json!({"name": "same words", "n": i}))
            .await?;
    }
    let item = TypeName::from("item");
    let by = |f: &str| vec![order(f, OrderBy::Value, Direction::Asc)];
    let find = |cursor, filter: &str, order_by| Find {
        limit: Some(1),
        cursor,
        filter: Some(filter.into()),
        order_by,
        ..Find::default()
    };
    let search = |cursor, mode| Search {
        ty: item.clone(),
        mode,
        k: 1,
        cursor,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let query = |cursor, fields: Vec<String>, filter: &str| {
        let mut p = pattern("item", None, Vec::new(), Projection::Rows { fields });
        (p.limit, p.cursor, p.start.filter) = (Some(1), cursor, Some(filter.into()));
        p
    };
    let next = |c: Option<Cursor>, what: &str| {
        c.ok_or_else(|| Failure::new(format!("a first page of {what} hands out a cursor")))
    };
    let of_find = match cat.find(own, &item, find(None, "n >= 0", by("name"))).await {
        Ok(r) => match r.data {
            Found::Records { next_cursor, .. } => next(next_cursor, "`find`")?,
            Found::Groups { .. } => return Err(Failure::new("`find` answered groups")),
        },
        Err(e) => {
            return Err(Failure::new(format!(
                "the first page of `find` failed: {e}"
            )));
        }
    };
    let of_search = next(
        (cat.search(own, search(None, SearchMode::lexical("same")))
            .await)
            .must("the first page of `search`")?
            .data
            .next_cursor,
        "`search`",
    )?;
    let of_query = match (cat.query(own, query(None, Vec::new(), "n >= 0")).await)
        .must("the first page of `query`")?
        .data
    {
        QueryResult::Rows { next_cursor, .. } => next(next_cursor, "`query`")?,
        _ => return Err(Failure::new("a pattern's rows answered something else")),
    };
    // Another page size, `total` and `with_meta` read the same rows.
    let wider = Find {
        limit: Some(2),
        total: true,
        with_meta: true,
        ..find(Some(of_find.clone()), "n >= 0", by("name"))
    };
    cat.find(own, &item, wider)
        .await
        .must("continuing `find` with another page size, `total` and `with_meta`")?;
    let wider = Search {
        k: 2,
        with_meta: true,
        ..search(Some(of_search.clone()), SearchMode::lexical("same"))
    };
    cat.search(own, wider)
        .await
        .must("continuing `search` with another page size and `with_meta`")?;
    let mut wider = query(Some(of_query.clone()), Vec::new(), "n >= 0");
    wider.limit = Some(2);
    cat.query(own, wider)
        .await
        .must("continuing `query` with another page size")?;
    let named = SearchMode::Lexical {
        text: "same".into(),
        fields: Some(vec!["name".into()]),
    };
    let refused = [
        (
            "`find` under another `where`",
            (cat.find(
                own,
                &item,
                find(Some(of_find.clone()), "n >= 1", by("name")),
            )
            .await)
                .map(drop),
        ),
        (
            "`find` in another order of the same length",
            (cat.find(own, &item, find(Some(of_find), "n >= 0", by("n")))
                .await)
                .map(drop),
        ),
        (
            "`search` for other words",
            (cat.search(
                own,
                search(Some(of_search.clone()), SearchMode::lexical("words")),
            )
            .await)
                .map(drop),
        ),
        (
            "`search` in named fields",
            (cat.search(own, search(Some(of_search), named)).await).map(drop),
        ),
        (
            "`query` under another start `where`",
            (cat.query(own, query(Some(of_query.clone()), Vec::new(), "n >= 1"))
                .await)
                .map(drop),
        ),
        (
            "`query` projecting other fields",
            (cat.query(own, query(Some(of_query), vec!["name".into()], "n >= 0"))
                .await)
                .map(drop),
        ),
    ];
    let wrong: Vec<String> = (refused.into_iter())
        .filter(|(_, r)| !matches!(invalid_case(r), Some(Case::CursorOtherRead { .. })))
        .map(|(what, r)| format!("{what} answered {r:?}"))
        .collect();
    ensure!(
        wrong.is_empty(),
        "a cursor continues only the read it was handed out for, so each of these must be \
         refused as invalid: {}",
        wrong.join("; ")
    );
    Ok(())
}
