//! `find`'s `after`: a sort position said in the open
//! (`docs/decisions/029-every-answer-has-one-order.md`, `docs/glossary.md`).
//!
//! A cursor carries the last row's sort values and key; `after` names the
//! same position without the token. Resumed from each page's last row, it
//! must walk exactly the pages the cursor walks, in the one order: ties
//! broken by key, missing values last in either direction. The row it
//! names need not still exist.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{Aggregate, Direction, Order, OrderBy};
use lakelands_api::{
    After, Catalog, Error, FieldName, Find, Found, LakeStore, Position, Principal, ReadAt, Record,
    Refusal, TypeName, Value, WriteOpts,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all_found, declare, doors, field, owner, type_decl, upsert, value,
};

check!(
    /// `after` resumes where the cursor would.
    AFTER_RESUMES = after_resumes_where_the_cursor_would,
    "029: `after` names the position a cursor carries — a row's sort values and key — and continues strictly after it in the one order, at the caller's read position; it is refused beside a cursor or an aggregate, with a value count other than `order_by`'s, or with a value its key's field cannot hold"
);

const LIMIT: u32 = 3;

fn keys(rows: &[Record]) -> Vec<String> {
    rows.iter().map(|r| r.key.as_str().to_owned()).collect()
}

/// The position of `r` in `order`: its sort values, `null` where it has
/// none, and its key.
fn position_of(order: &[Order], r: &Record) -> After {
    After {
        values: order
            .iter()
            .map(|o| value(r, o.field.as_str()).cloned().unwrap_or(Value::Null))
            .collect(),
        key: r.key.clone(),
    }
}

/// One page of `find`, and whether another follows.
async fn page(
    cat: &dyn Catalog,
    who: &Principal,
    find: Find,
    what: &str,
) -> Result<(Vec<Record>, Option<lakelands_api::Cursor>), Failure> {
    let reply = cat
        .find(who, &TypeName::from("item"), find)
        .await
        .must(what)?;
    match reply.data {
        Found::Records { rows, next_cursor } => Ok((rows, next_cursor)),
        Found::Groups { .. } => Err(Failure::new(format!(
            "{what}: `find` without an aggregate answered groups"
        ))),
    }
}

/// The pages `find` answers following its cursor.
async fn by_cursor(
    cat: &dyn Catalog,
    who: &Principal,
    mut find: Find,
) -> Result<Vec<Vec<String>>, Failure> {
    let mut out = Vec::new();
    for _ in 0..100 {
        let (rows, next) = page(cat, who, find.clone(), "walking the cursor").await?;
        out.push(keys(&rows));
        match next {
            Some(c) => find.cursor = Some(c),
            None => return Ok(out),
        }
    }
    Err(Failure::new("walking the cursor never reached a last page"))
}

/// The pages `find` answers resuming with `after` from each page's last
/// row, from `start` on: at most `bound`, so a walk that never ends shows
/// where it went round.
async fn by_after(
    cat: &dyn Catalog,
    who: &Principal,
    mut find: Find,
    start: Option<After>,
    bound: usize,
) -> Result<Vec<Vec<String>>, Failure> {
    let mut out = Vec::new();
    find.after = start;
    while out.len() < bound {
        let (rows, next) = page(cat, who, find.clone(), "resuming with `after`").await?;
        out.push(keys(&rows));
        match (next, rows.last()) {
            (Some(_), Some(last)) => find.after = Some(position_of(&find.order_by, last)),
            _ => return Ok(out),
        }
    }
    Ok(out)
}

fn order(dir: Direction, two: bool) -> Vec<Order> {
    let by = |f: &str, dir| Order {
        field: FieldName::from(f),
        by: OrderBy::Value,
        dir,
    };
    let other = match dir {
        Direction::Asc => Direction::Desc,
        Direction::Desc => Direction::Asc,
    };
    let mut o = vec![by("n", dir)];
    if two {
        o.push(by("s", other));
    }
    o
}

async fn item(
    cat: &dyn Catalog,
    own: &Principal,
    key: &str,
    n: Option<i64>,
    s: Option<&str>,
) -> Result<lakelands_api::Outcome, Failure> {
    let mut fields = serde_json::Map::new();
    if let Some(n) = n {
        fields.insert("n".into(), json!(n));
    }
    if let Some(s) = s {
        fields.insert("s".into(), json!(s));
    }
    upsert(cat, own, "item", key, Value::Object(fields)).await
}

async fn after_resumes_where_the_cursor_would(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let cat = cat.as_ref();
    let own = owner(store.as_ref());
    declare(
        cat,
        &own,
        type_decl(
            "item",
            vec![
                field("n", StorageKind::Number),
                field("s", StorageKind::Text),
            ],
        ),
    )
    .await?;
    // Ties on `n`, ties on `(n, s)`, and records without either, written
    // out of key order.
    let first = [
        ("k00", Some(3), Some("b")),
        ("k01", Some(1), Some("a")),
        ("k02", Some(2), None),
        ("k03", Some(1), Some("a")),
        ("k04", None, Some("c")),
        ("k05", Some(3), Some("b")),
        ("k06", Some(2), Some("a")),
        ("k07", Some(1), None),
        ("k08", None, Some("b")),
        ("k09", Some(3), Some("b")),
    ];
    let mut earlier = Position(0);
    for (k, n, s) in first {
        earlier = item(cat, &own, k, n, s).await?.position();
    }
    // The lake moves on after `earlier`: new rows among the old ones and
    // one row that sorts elsewhere now.
    for (k, n, s) in [
        ("k10", Some(1), Some("a")),
        ("k11", Some(2), Some("c")),
        ("k12", None, Some("a")),
        ("k13", Some(2), None),
        ("k05", Some(1), Some("b")),
    ] {
        item(cat, &own, k, n, s).await?;
    }

    for dir in [Direction::Asc, Direction::Desc] {
        for two in [false, true] {
            for filter in [None, Some("n > 1 or n is null")] {
                for as_of in [None, Some(earlier)] {
                    let find = Find {
                        filter: filter.map(Into::into),
                        order_by: order(dir, two),
                        limit: Some(LIMIT),
                        at: ReadAt {
                            as_of,
                            valid_at: None,
                        },
                        ..Find::default()
                    };
                    let cursor = by_cursor(cat, &own, find.clone()).await?;
                    let after = by_after(cat, &own, find.clone(), None, cursor.len() + 1).await?;
                    ensure!(
                        cursor == after,
                        "ordering {:?}, where {filter:?}, as_of {as_of:?}, by {LIMIT}: resuming \
                         with `after` from each page's last row must walk the pages the \
                         cursor walks.\n  cursor: {cursor:?}\n  after:  {after:?}",
                        find.order_by
                    );
                }
            }
        }
    }

    // The row `after` names need not exist any more: retired, it still
    // marks a place in the order, and it does not come back.
    let order = order(Direction::Asc, true);
    let whole = Find {
        order_by: order.clone(),
        limit: Some(LIMIT),
        ..Find::default()
    };
    let (rows, _) = page(cat, &own, whole.clone(), "reading the first page").await?;
    let gone = rows
        .last()
        .ok_or_else(|| Failure::new("fourteen records answered an empty first page"))?
        .clone();
    let listed = keys(&all_found(cat, &own, "item", whole.clone()).await?);
    let at = listed
        .iter()
        .position(|k| k.as_str() == gone.key.as_str())
        .ok_or_else(|| Failure::new("the first page's last row is not in the walk"))?;
    let expected = listed[at + 1..].to_vec();
    cat.retire(
        &own,
        &TypeName::from("item"),
        &gone.key,
        WriteOpts::default(),
    )
    .await
    .must("retiring the first page's last row")?;
    let resumed: Vec<String> = by_after(
        cat,
        &own,
        whole.clone(),
        Some(position_of(&order, &gone)),
        100,
    )
    .await?
    .concat();
    ensure!(
        resumed == expected,
        "after retiring `{}`, resuming after its position ({:?}) at the head walked \
         {resumed:?}; the rows strictly after it are {expected:?}",
        gone.key,
        position_of(&order, &gone)
    );

    // Refused, naming the member.
    let (_, cursor) = page(cat, &own, whole.clone(), "reading a first page").await?;
    let beside_cursor = Find {
        cursor,
        after: Some(position_of(&order, &gone)),
        ..whole.clone()
    };
    let one_value = Find {
        after: Some(After {
            values: vec![json!(1)],
            key: "k00".into(),
        }),
        ..whole.clone()
    };
    let aggregated = Find {
        order_by: Vec::new(),
        after: Some(After {
            values: Vec::new(),
            key: "k00".into(),
        }),
        aggregate: Some(
            serde_json::from_value::<Aggregate>(
                json!({"measures": [{"name": "c", "func": "count"}]}),
            )
            .map_err(|e| Failure::new(format!("the aggregate does not read: {e}")))?,
        ),
        ..whole.clone()
    };
    for (what, find) in [
        ("`after` beside `cursor`", beside_cursor),
        ("one `after` value for two `order_by` keys", one_value),
        ("`after` beside `aggregate`", aggregated),
    ] {
        let refused = cat
            .find(&own, &TypeName::from("item"), find)
            .await
            .must_fail(what)?;
        ensure!(
            matches!(&refused, Error::Refused { reason: Refusal::Invalid { message }, .. }
                if message.contains("`after")),
            "{what} must be refused as invalid, naming `after`; it answered {refused:?}"
        );
    }
    let unfit = Find {
        after: Some(After {
            values: vec![json!("three"), json!("b")],
            key: "k00".into(),
        }),
        ..whole
    };
    let refused = cat
        .find(&own, &TypeName::from("item"), unfit)
        .await
        .must_fail("a text `after` value for the number `n`")?;
    ensure!(
        matches!(&refused, Error::Refused { field: Some(f), reason: Refusal::Invalid { message } }
            if f.as_str() == "n" && message.contains("`after")),
        "a text `after` value for the number `n` must be refused as invalid on `n`, \
         naming `after`; it answered {refused:?}"
    );
    Ok(())
}
