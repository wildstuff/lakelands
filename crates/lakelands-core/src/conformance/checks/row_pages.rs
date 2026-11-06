//! A pattern's rows are walked a page of starts at a time and paged by
//! where the last page ended (lakelands#940,
//! `docs/decisions/026-the-store-walks-the-graph.md`): the pages through
//! their cursors are the rows read in one page, for every order, a reader
//! not shown some starts or ends counts nothing from where a page ends,
//! and rows read whole for an order over a record a step reaches, or for
//! an aggregate, are bounded by the lake's `max_graph_nodes`.

use lakelands_api::Value;
use lakelands_api::id::Cursor;

use crate::conformance::fixture::{order, reference, step};
use crate::conformance::prelude::*;

check!(
    ROW_PAGES = a_patterns_pages_are_its_rows_read_in_one_page,
    "026 and 029, extended (lakelands#940): a pattern's rows are walked a page of starts at a time and a page's cursor names where it ended, so its pages through their cursors are the rows the same pattern answers in one page, in one order: from invoices out to their customer and from customers in to their invoices (several rows a start, so a page ends inside one start's rows), with a step `where` and without, with no order, an order by the start's own field either way and one by the far record's, at limits of 1, 2, 3, 7 and 50, each page holding its limit but the last, which holds a row, and a cursor exactly where a row follows; a reader without `restricted` is answered every page of one family of records with held invoices and held customers as of a twin family where those records are not there: the rows, where each page ends, and the start and offset its cursor names; and a cursor whose start is no position in the order its rows are read in (sort values of another count, or any start where every row is read) is refused as cursor_foreign",
    given
);

check!(
    ROWS_READ_WHOLE_BOUNDED = rows_read_whole_past_the_lakes_max_graph_nodes_are_refused,
    "026, extended (lakelands#940), and the location's limits: rows ordered by a record a step reaches, and an aggregate over rows, are read whole and so at most `max_graph_nodes` of them: past it the read is refused as too_large naming the ceiling and the member, `order_by` or `project`, with a sentence that names the way out, and nothing partial is answered; a reader not shown some starts and ends is bounded by the rows it is answered; rows in the starts' order, and ordered by the start's own field, are walked a page of starts at a time and answered past it",
    given
);

/// Customers `c00` to `c11` of family `p`, every fourth named `x`, and
/// `n` invoices, each billed to customer `i % 12`, both under the names
/// `cust` and `inv` in a pattern. Where `held`, customers 1 and 4 and
/// every seventh invoice are restricted; where `absent`, they are not
/// there, and the invoices of those customers are billed to nobody.
async fn family(l: &Lake, store: &Store, p: &str, n: usize, held: bool, absent: bool) -> Held {
    let (cust, inv) = (format!("{p}cust"), format!("{p}inv"));
    l.declare(type_decl(&cust, vec![field("name", StorageKind::Text)]))
        .await?;
    let mut ty = type_decl(
        &inv,
        vec![
            field("amount", StorageKind::Number),
            reference("customer", &cust),
        ],
    );
    ty.relations = vec![RelationDecl {
        name: "billed_to".into(),
        target: cust.as_str().into(),
        via: Some("customer".into()),
        ..Default::default()
    }];
    l.declare(ty).await?;
    let gone = |c: usize| c == 1 || c == 4;
    for c in (0..12).filter(|c| !(absent && gone(*c))) {
        let name = if c % 4 == 0 {
            "x".to_owned()
        } else {
            format!("n{c:02}")
        };
        l.upsert(&cust, &format!("c{c:02}"), json!({"name": name}))
            .await?;
    }
    for i in (0..n).filter(|i| !(absent && i % 7 == 3)) {
        let to = i % 12;
        let customer = match absent && gone(to) {
            true => Value::Null,
            false => json!(format!("c{to:02}")),
        };
        let fields = json!({"amount": (i * 37) % 10, "customer": customer});
        l.upsert(&inv, &format!("i{i:04}"), fields).await?;
    }
    if held {
        for c in [1, 4] {
            restrict(store.as_ref(), &cust, format!("c{c:02}"), "held").await?;
        }
        for i in (0..n).filter(|i| i % 7 == 3) {
            restrict(store.as_ref(), &inv, format!("i{i:04}"), "held").await?;
        }
    }
    Ok(())
}

/// Rows from family `p`'s invoices out to their customer, or from its
/// customers in to their invoices, each bound as `inv` and `cust`.
fn rows(p: &str, out: bool, filter: Option<&str>, by: Option<(&str, Direction)>) -> Pattern {
    let (from, walk) = if out {
        ("inv", Walk::Out)
    } else {
        ("cust", Walk::In)
    };
    let mut s = step("billed_to", walk);
    s.filter = filter.map(Into::into);
    s.named = Some(if out { "cust" } else { "inv" }.into());
    let fields = Vec::new();
    let mut q = pattern(
        &format!("{p}{from}"),
        None,
        vec![s],
        Projection::Rows { fields },
    );
    q.start.named = Some(from.into());
    q.order_by = by
        .map(|(f, dir)| order(f, OrderBy::Value, dir))
        .into_iter()
        .collect();
    q
}

type Row = (String, String);

/// One page: its rows as the keys they bind, and its cursor.
async fn page(
    cat: &dyn Catalog,
    who: &Principal,
    q: Pattern,
) -> Result<(Vec<Row>, Option<Cursor>), Error> {
    let QueryResult::Rows { rows, next_cursor } = cat.query(who, q).await?.data else {
        return Ok((Vec::new(), None));
    };
    let key = |r: &BTreeMap<String, Value>, n: &str| {
        let k = r.get(&format!("{n}.key")).and_then(Value::as_str);
        k.unwrap_or("-").to_owned()
    };
    Ok((
        rows.iter()
            .map(|r| (key(r, "inv"), key(r, "cust")))
            .collect(),
        next_cursor,
    ))
}

/// A cursor as the lake wrote it, and the same with its `after` set.
fn opened(c: &Cursor) -> Value {
    let bytes = (0..c.0.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&c.0[i..i + 2], 16).ok());
    serde_json::from_slice(&bytes.collect::<Vec<_>>()).unwrap_or(Value::Null)
}

pub(super) fn forged(c: &Cursor, after: Value) -> Cursor {
    let mut state = opened(c);
    state["after"] = after;
    Cursor(lakelands_api::canonical::hex(
        &lakelands_api::canonical::of(&state),
    ))
}

/// Every page of `q` at `limit`, in turn, each with where its cursor
/// says the next page goes on: its start and offset.
async fn pages(
    cat: &dyn Catalog,
    who: &Principal,
    q: &Pattern,
    limit: u32,
) -> Result<Vec<(Vec<Row>, Option<Value>)>, Failure> {
    let (mut out, mut cursor) = (Vec::new(), None);
    loop {
        let mut next = Pattern {
            limit: Some(limit),
            cursor: cursor.take(),
            ..q.clone()
        };
        next.limit = Some(limit);
        let (rows, more) = page(cat, who, next).await.must("a page of rows")?;
        let at = more
            .as_ref()
            .map(opened)
            .map(|c| json!([c["after"], c["offset"]]));
        out.push((rows, at));
        if more.is_none() || out.len() > 400 {
            return Ok(out);
        }
        cursor = more;
    }
}

async fn a_patterns_pages_are_its_rows_read_in_one_page(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    family(&l, &store, "", 60, false, false).await?;
    family(&l, &store, "a", 60, true, false).await?;
    family(&l, &store, "b", 60, false, true).await?;
    let reader = reads_all(store.as_ref(), "reader");
    let orders: [Option<(&str, Direction)>; 4] = [
        None,
        Some(("inv.amount", Direction::Asc)),
        Some(("inv.amount", Direction::Desc)),
        Some(("cust.name", Direction::Asc)),
    ];
    for out in [true, false] {
        for filter in [None, Some("name = 'x'")] {
            let filter = if out {
                filter
            } else {
                filter.map(|_| "amount < 5")
            };
            for by in orders {
                // A start's own field is read from where the pattern starts.
                let by = by.map(|(f, d)| match (out, f) {
                    (false, "inv.amount") => ("cust.name", d),
                    (false, "cust.name") => ("inv.amount", d),
                    _ => (f, d),
                });
                let q = rows("", out, filter, by);
                let (whole, more) = page(
                    l.cat.as_ref(),
                    &l.own,
                    Pattern {
                        limit: Some(1000),
                        ..q.clone()
                    },
                )
                .await
                .must("the rows in one page")?;
                ensure!(
                    more.is_none() && !whole.is_empty(),
                    "{q:?} answers its rows in one page"
                );
                for limit in [1, 2, 3, 7, 50] {
                    let paged = pages(l.cat.as_ref(), &l.own, &q, limit).await?;
                    let full = paged
                        .iter()
                        .rev()
                        .skip(1)
                        .all(|(r, more)| r.len() == limit as usize && more.is_some());
                    let joined: Vec<Row> = paged.iter().flat_map(|(r, _)| r.clone()).collect();
                    let last = paged.last().is_some_and(|(r, more)| {
                        more.is_none() && !r.is_empty() && r.len() <= limit as usize
                    });
                    ensure!(
                        joined == whole && full && last,
                        "the pages of {q:?} at a limit of {limit} are its rows read in one page, \
                         each full but the last, a cursor exactly where a row follows: {} rows in \
                         {} pages against {} in one",
                        joined.len(),
                        paged.len(),
                        whole.len()
                    );
                }
                for limit in [1, 3, 7, 50] {
                    let held =
                        pages(l.cat.as_ref(), &reader, &rows("a", out, filter, by), limit).await?;
                    let twin =
                        pages(l.cat.as_ref(), &reader, &rows("b", out, filter, by), limit).await?;
                    ensure!(
                        held == twin,
                        "a reader without `restricted` is answered the pages of family `a`, with \
                         held invoices and customers, as those of family `b`, where they are not \
                         there, each page's cursor naming the same start and offset ({out} \
                         {filter:?} {by:?}, a limit of {limit}): {} pages against {}",
                        held.len(),
                        twin.len()
                    );
                }
            }
        }
    }
    // A cursor whose start is no position in the order its rows are read
    // in is not one the lake handed out.
    for (by, after) in [
        (Some(("inv.amount", Direction::Asc)), json!([[], "i0010"])),
        (
            Some(("inv.amount", Direction::Asc)),
            json!([[1, 2, 3], "i0010"]),
        ),
        (None, json!([[7], "i0010"])),
        (Some(("cust.name", Direction::Asc)), json!([["x"], "i0010"])),
    ] {
        let q = Pattern {
            limit: Some(3),
            ..rows("", true, None, by)
        };
        let (_, more) = page(l.cat.as_ref(), &l.own, q.clone())
            .await
            .must("a first page")?;
        let cursor = more.map(|c| forged(&c, after.clone()));
        let answer = page(l.cat.as_ref(), &l.own, Pattern { cursor, ..q }).await;
        ensure!(
            matches!(&answer, Err(e) if invalid_case(e) == Some(&Case::CursorForeign)),
            "a cursor of rows ordered by {by:?} whose start reads {after} is refused as \
             cursor_foreign; it answered {answer:?}"
        );
    }
    Ok(())
}

async fn rows_read_whole_past_the_lakes_max_graph_nodes_are_refused(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    family(&l, &store, "", 48, false, false).await?;
    family(&l, &store, "h", 40, true, false).await?;
    let limits = Limits {
        max_graph_nodes: 30,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits).await;
    let store = store.map_err(|e| Failure::new(format!("opening again with a cap of 30: {e}")))?;
    let l = Lake::of(store.as_ref());
    let far = rows("", true, None, Some(("cust.name", Direction::Asc)));
    let mut measured = rows("", true, None, None);
    measured.project = Projection::Aggregate(Aggregate {
        measures: vec![agg("n", AggFunc::Count, None)],
        group_by: Vec::new(),
    });
    for (q, member, way) in [
        (far, "order_by", "order by the start's own fields"),
        (measured, "project", "narrow the start"),
    ] {
        let before = store.head().await.must("reading the head")?;
        let err = l
            .cat
            .query(&l.own, q)
            .await
            .must_fail("rows read whole past the cap")?;
        let told = err.to_string();
        ensure!(
            matches!(reason(&err), Some(Refusal::TooLarge { max: 30 }))
                && matches!(&err, Error::Refused { field: Some(f), .. } if f == member)
                && told.contains(way),
            "48 rows read whole past a cap of 30 are refused as too_large naming 30 and \
             `{member}`, with a sentence that names the way out ({way}); it failed with {told} \
             ({err:?})"
        );
        crate::conformance::fixture::head_unmoved(store.as_ref(), before, "rows past the cap")
            .await?;
    }
    // A reader not shown some starts and ends is bounded by the rows it
    // is answered: 27 of 40 invoices, under the cap; the owner's 40 are
    // over it.
    let reader = reads_all(store.as_ref(), "reader");
    let far = || Pattern {
        limit: Some(100),
        ..rows("h", true, None, Some(("cust.name", Direction::Asc)))
    };
    let (held, _) = page(l.cat.as_ref(), &reader, far())
        .await
        .must("the held reader's rows")?;
    let owner = l.cat.query(&l.own, far()).await.err();
    ensure!(
        held.len() == 27
            && owner
                .as_ref()
                .is_some_and(|e| matches!(reason(e), Some(Refusal::TooLarge { max: 30 }))),
        "of 40 invoices, 13 held from a reader without `restricted`, that reader is answered \
         its 27 rows ordered by the far record under a cap of 30, and the owner is refused its \
         40 ({} rows; the owner {owner:?})",
        held.len()
    );
    for by in [None, Some(("inv.amount", Direction::Asc))] {
        let q = rows("", true, None, by);
        let (rows, _) = page(
            l.cat.as_ref(),
            &l.own,
            Pattern {
                limit: Some(100),
                ..q
            },
        )
        .await
        .must("rows in the starts' order")?;
        ensure!(
            rows.len() == 48,
            "48 rows in the starts' order ({by:?}) are answered past a cap of 30, a page of \
             starts at a time; {} were",
            rows.len()
        );
    }
    Ok(())
}
