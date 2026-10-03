//! A view's parameters: values a read binds, so that one view answers
//! the window around a date the reader names (`lakelands_api::ontology::ViewParam`).
//!
//! The fixture is one source (`payment`: an account, a date, an exact
//! amount) and one view over it, `account_window`, which sums each
//! account's payments in the seven days up to its `anchor` and counts
//! those on the anchor's last day.

use std::sync::Arc;

use lakelands_api::ontology::{Derived, MatchPair, RelationDecl, StorageKind, TypeDecl, ViewDecl};
use lakelands_api::query::{Pattern, Projection, QueryResult, Walk};
use lakelands_api::read::{Find, Found, Get};
use lakelands_api::{Error, LakeStore, Params, Refusal, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, bound, field, find_where, log_events, param, pattern, reason, step, type_decl,
    view_of,
};

check!(
    /// Two anchors, two windows.
    TWO_ANCHORS = the_same_view_read_at_two_anchors_answers_two_windows,
    "018: a view's parameters are bound by the read, so the same view read with two anchors answers two windows, by `find` and by `get`, each byte for byte the same on every engine; a date alone is midnight UTC"
);

check!(
    /// A parameter missing, unknown or of the wrong kind is refused.
    BINDING_REFUSED = a_missing_unknown_or_wrongly_kinded_parameter_is_refused_naming_it,
    "018 and 015: a read of a view that leaves a parameter out, names one the view does not declare, or gives a value not of its kind is refused as invalid, naming the parameter; a type that is not a view takes none"
);

check!(
    /// `param()` lives only in a view.
    PARAM_ONLY_IN_A_VIEW = param_outside_a_views_where_and_measures_is_refused,
    "017 and 018: `param()` is read only in a view's `where` and `measures`; in a read's `where`, a pattern's `where` or a derived field it is refused as invalid, naming the function"
);

check!(
    /// A view's parameters are declared strictly.
    PARAMS_DECLARED_STRICTLY = a_view_declaring_its_parameters_badly_is_refused,
    "028 and 018: `declare` refuses a view whose `param()` names no declared parameter, which declares a parameter nothing reads, one twice, or one of a kind other than text, number, decimal, boolean or date_time, naming it"
);

check!(
    /// A pattern binds the view it starts on.
    PATTERN_START_BINDS = a_pattern_started_on_a_view_binds_its_parameters,
    "018 and 026: a pattern that starts on a view binds its parameters with the start, by key and by `where`, and its nodes are the window `find` answers; a `degree` started on it by key answers that key's row of the whole view's; a start that leaves one out is refused, naming it"
);

check!(
    /// A view's parameters are written in format 8.
    FORMAT_8 = a_view_with_parameters_is_written_in_log_format_8,
    "log format §6 and 018: the event that declares a view with parameters is written in format 8, as is every event after it; a view without them leaves the lake's format where it was"
);

check!(
    /// A cursor keeps its parameters.
    CURSOR_KEEPS_PARAMS = a_cursor_continues_only_with_the_parameters_it_was_handed_out_for,
    "018 and 029: a page of a view continues only with the parameters its first page was read with; with others it is refused, since it would continue a different answer"
);

check!(
    /// A step reaching a view binds nothing.
    STEP_TO_UNBOUND_VIEW = a_step_arriving_at_a_view_with_parameters_is_refused_before_reading,
    "018 and 029: a view's parameters are bound only by a read that names it, so a pattern (rows or a measure) whose step arrives at a view with parameters other than its start is refused as invalid before any record is read, naming the view and the relation, the same with no records and with some; a pattern started on that view with its parameters, stepping out, is answered"
);

fn payment() -> TypeDecl {
    type_decl(
        "payment",
        vec![
            field("account", StorageKind::Text),
            field("date", StorageKind::DateTime),
            field("amount", StorageKind::Decimal),
        ],
    )
}

fn account_window() -> TypeDecl {
    let fields = vec![
        field("account", StorageKind::Text),
        field("total", StorageKind::Decimal),
        field("last_day", StorageKind::Number),
    ];
    let last_day = "count(if(date > param('anchor') - 1d, amount, null))";
    let measures = [("total", "sum(amount)"), ("last_day", last_day)];
    let mut t = view_of("account_window", "payment", &["account"], fields, &measures);
    if let Some(v) = t.view.as_mut() {
        v.params = vec![param("anchor", StorageKind::DateTime)];
        v.r#where = Some("date > param('anchor') - 7d and date <= param('anchor')".into());
    }
    t
}

/// The source, the view, and five payments in June 2026.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(payment()).await?;
    l.declare(account_window()).await?;
    for (key, account, date, amount) in [
        ("p1", "a", "2026-06-10", "10.00"),
        ("p2", "a", "2026-06-24", "5.50"),
        ("p3", "b", "2026-06-25", "3.00"),
        ("p4", "a", "2026-06-30", "1.25"),
        ("p5", "b", "2026-06-08", "7.00"),
    ] {
        let fields = json!({"account": account, "date": date, "amount": amount});
        l.upsert("payment", key, fields).await?;
    }
    Ok(l)
}

/// `payment` with `window`, a relation derived from its account to the
/// account's window.
fn paid() -> TypeDecl {
    let mut paid = payment();
    paid.relations = vec![RelationDecl {
        derived: Some(Derived {
            pairs: vec![MatchPair {
                from: "account".into(),
                to: "account".into(),
            }],
        }),
        ..RelationDecl::to("window", "account_window")
    }];
    paid
}

/// A change that spoils a view's declaration.
type Change = fn(&mut ViewDecl);

fn anchor(date: &str) -> Params {
    [("anchor".into(), json!(date))].into()
}

fn invalid_naming(r: &Result<impl std::fmt::Debug, Error>, name: &str) -> bool {
    matches!(reason(&r), Some(Refusal::Invalid { message }) if message.contains(name))
}

async fn the_same_view_read_at_two_anchors_answers_two_windows(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    for (date, want) in [
        (
            "2026-06-30",
            json!([{"account": "a", "total": "6.75", "last_day": 1},
                   {"account": "b", "total": "3.00", "last_day": 0}]),
        ),
        (
            "2026-06-12",
            json!([{"account": "a", "total": "10.00", "last_day": 0},
                   {"account": "b", "total": "7.00", "last_day": 0}]),
        ),
    ] {
        let rows = l.all_found("account_window", bound(anchor(date))).await?;
        let got = json!(rows.iter().map(|r| &r.fields).collect::<Vec<_>>());
        ensure!(
            got == want,
            "the window of the seven days up to {date} must be {want}; it is {got}. A view \
             whose parameter is not bound answers the same window, or none, for every anchor"
        );
        let get = Get {
            key: "a".into(),
            with_meta: false,
            at: Default::default(),
            params: anchor(date),
        };
        let what = format!("getting account a's window up to {date}");
        let a = l.cat.get(&l.own, &"account_window".into(), get).await;
        let a = a.must(&what)?;
        ensure!(
            json!(a.data.fields) == want[0],
            "`get` of account a's window up to {date} must answer {}, as `find` does; it \
             answers {:?}",
            want[0],
            a.data.fields
        );
    }
    Ok(())
}

async fn a_missing_unknown_or_wrongly_kinded_parameter_is_refused_naming_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    for (what, ty, params, name) in [
        (
            "leaving `anchor` out",
            "account_window",
            Params::new(),
            "anchor",
        ),
        (
            "naming `anchr`",
            "account_window",
            [
                ("anchor".into(), json!("2026-06-30")),
                ("anchr".into(), json!("2026-06-30")),
            ]
            .into(),
            "anchr",
        ),
        (
            "binding `anchor` to 2026-13-01",
            "account_window",
            anchor("2026-13-01"),
            "anchor",
        ),
        (
            "binding `anchor` to a number",
            "account_window",
            [("anchor".into(), json!(20260630))].into(),
            "anchor",
        ),
        (
            "binding `anchor` to null",
            "account_window",
            [("anchor".into(), Value::Null)].into(),
            "anchor",
        ),
        (
            "binding `anchor` on `payment`, which is no view",
            "payment",
            anchor("2026-06-30"),
            "anchor",
        ),
    ] {
        let r = l.cat.find(&l.own, &ty.into(), bound(params)).await;
        ensure!(
            invalid_naming(&r, name),
            "a read of `{ty}` {what} must be refused as invalid, naming `{name}`; it answered \
             {r:?}"
        );
    }
    Ok(())
}

async fn param_outside_a_views_where_and_measures_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    let filter = "date <= param('anchor')";
    let find = find_where(filter);
    let r = l.cat.find(&l.own, &"payment".into(), find).await;
    ensure!(
        invalid_naming(&r, "param()"),
        "a `find` whose `where` calls param() must be refused as invalid, naming it; it \
         answered {r:?}"
    );
    let pattern = start_on("payment", None, Some(filter), Params::new());
    let r = l.cat.query(&l.own, pattern).await;
    ensure!(
        invalid_naming(&r, "param()"),
        "a pattern whose `where` calls param() must be refused as invalid, naming it; it \
         answered {r:?}"
    );
    let mut derived = field("late", StorageKind::Boolean);
    derived.derived = Some("date > param('anchor')".into());
    let mut ty = payment();
    ty.name = "payment_late".into();
    ty.fields.push(derived);
    let r = l.try_declare(ty).await;
    ensure!(
        invalid_naming(&r, "param()"),
        "a derived field that calls param() must be refused as invalid, naming it; declare \
         answered {r:?}"
    );
    Ok(())
}

async fn a_view_declaring_its_parameters_badly_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(payment()).await?;

    let cases: [(&str, Change, &str); 4] = [
        (
            "reads param('day'), which it does not declare",
            |v| v.r#where = Some("date <= param('day')".into()),
            "day",
        ),
        (
            "declares `spare`, which nothing reads",
            |v| v.params.push(param("spare", StorageKind::Text)),
            "spare",
        ),
        (
            "declares `anchor` twice",
            |v| v.params.push(param("anchor", StorageKind::DateTime)),
            "anchor",
        ),
        (
            "declares `anchor` as json",
            |v| v.params[0].kind = StorageKind::Json,
            "anchor",
        ),
    ];
    for (what, change, name) in cases {
        let mut ty = account_window();
        change(ty.view.as_mut().expect("a view"));
        let r = l.try_declare(ty).await;
        ensure!(
            invalid_naming(&r, name),
            "a view that {what} must be refused as invalid, naming `{name}`; declare \
             answered {r:?}"
        );
    }
    Ok(())
}

async fn a_cursor_continues_only_with_the_parameters_it_was_handed_out_for(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    let page = |date: &str, cursor| Find {
        limit: Some(1),
        cursor,
        ..bound(anchor(date))
    };
    let first = l
        .cat
        .find(&l.own, &"account_window".into(), page("2026-06-30", None))
        .await
        .must("reading the first page of the window up to 2026-06-30")?;
    let Found::Records {
        next_cursor: Some(cursor),
        ..
    } = first.data
    else {
        return Err(Failure::new(
            "the window up to 2026-06-30 holds two accounts, so a page of one must hand out \
             a cursor",
        ));
    };
    let p = page("2026-06-12", Some(cursor.clone()));
    let other = l.cat.find(&l.own, &"account_window".into(), p).await;
    ensure!(
        invalid_naming(&other, "parameters"),
        "a cursor handed out for the anchor 2026-06-30 must be refused with the anchor \
         2026-06-12, naming the parameters; it answered {other:?}"
    );
    let p = page("2026-06-30", Some(cursor));
    let same = l.cat.find(&l.own, &"account_window".into(), p).await;
    let same = same.must("continuing the window up to 2026-06-30 with its own cursor")?;
    let Found::Records { rows, .. } = same.data else {
        return Err(Failure::new("a page of records answered groups"));
    };
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    ensure!(
        keys == ["b"],
        "the second page of the window up to 2026-06-30 must hold account b; it holds \
         {keys:?}"
    );
    Ok(())
}

/// A graph from the records of `view` (or of any type), or the one at
/// `key`, that `filter` holds for, binding `params`.
fn start_on(view: &str, key: Option<&str>, filter: Option<&str>, params: Params) -> Pattern {
    let mut p = pattern(view, key, Vec::new(), Projection::Graph);
    p.start.filter = filter.map(Into::into);
    p.start.params = params;
    p
}

async fn a_pattern_started_on_a_view_binds_its_parameters(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    for (key, filter, date, want) in [
        (
            Some("a"),
            None,
            "2026-06-12",
            json!([{"account": "a", "total": "10.00", "last_day": 0}]),
        ),
        (
            None,
            Some("total > 5"),
            "2026-06-30",
            json!([{"account": "a", "total": "6.75", "last_day": 1}]),
        ),
    ] {
        let what = format!(
            "a pattern started on `account_window` (key {key:?}, where {filter:?}) up to {date}"
        );
        let p = start_on("account_window", key, filter, anchor(date));
        let reply = l.cat.query(&l.own, p).await.must(&what)?;
        let QueryResult::Graph { nodes, .. } = reply.data else {
            return Err(Failure::new(format!(
                "{what} answered something other than a graph"
            )));
        };
        let got = json!(nodes.iter().map(|n| &n.fields).collect::<Vec<_>>());
        ensure!(
            got == want,
            "{what} must hold the window's record {want}; it holds {got}"
        );
    }
    l.declare(paid()).await?;
    let degree = |key: Option<&str>| Pattern {
        steps: vec![step("window", Walk::In)],
        project: Projection::Degree,
        ..start_on("account_window", key, None, anchor("2026-06-30"))
    };
    let mut rows = Vec::new();
    for key in [None, Some("a")] {
        let what = format!("`degree` over `window` started on `account_window` at key {key:?}");
        let reply = l.cat.query(&l.own, degree(key)).await.must(&what)?;
        let QueryResult::Rows { rows: got, .. } = reply.data else {
            return Err(Failure::new(format!(
                "{what} answered something other than rows"
            )));
        };
        rows.push(got);
    }
    let of_a: Vec<_> = rows[0]
        .iter()
        .filter(|r| r.get("key") == Some(&json!("a")))
        .cloned()
        .collect();
    ensure!(
        !of_a.is_empty() && rows[1] == of_a,
        "a `degree` started on `account_window` at `a` with its `anchor` answers the whole \
         view's row for `a`, {of_a:?}; it answered {:?}",
        rows[1]
    );
    let unbound = start_on("account_window", Some("a"), None, Params::new());
    let r = l.cat.query(&l.own, unbound).await;
    ensure!(
        invalid_naming(&r, "anchor"),
        "a pattern started on `account_window` without its `anchor` must be refused as \
         invalid, naming it; it answered {r:?}"
    );
    Ok(())
}

async fn a_step_arriving_at_a_view_with_parameters_is_refused_before_reading(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(payment()).await?;
    l.declare(account_window()).await?;
    l.declare(paid()).await?;
    let hop = || vec![step("window", Walk::Out)];
    let rows = || pattern("payment", None, hop(), Projection::Rows { fields: vec![] });
    let degree = || pattern("payment", None, hop(), Projection::Degree);
    // The answer does not depend on the data: with no payment and with one.
    for (when, write) in [("with no payment", false), ("with one payment", true)] {
        if write {
            let p1 = json!({"account": "a", "date": "2026-06-10", "amount": "10.00"});
            l.upsert("payment", "p1", p1).await?;
        }
        for (what, p) in [("rows", rows()), ("a degree", degree())] {
            let r = l.cat.query(&l.own, p).await;
            ensure!(
                invalid_naming(&r, "account_window") && invalid_naming(&r, "window"),
                "a pattern of {what} from `payment` stepping over `window` arrives at the view \
                 `account_window` with no parameters bound, and must be refused as invalid \
                 {when}, naming the view and the relation; it answered {r:?}"
            );
        }
    }
    // Started on the view with its parameters, a step out is answered.
    let out = Pattern {
        steps: vec![step("window", Walk::In)],
        project: Projection::Rows { fields: Vec::new() },
        ..start_on("account_window", Some("a"), None, anchor("2026-06-12"))
    };
    l.cat
        .query(&l.own, out)
        .await
        .must("a pattern started on `account_window` with its anchor, stepping over `window`")?;
    Ok(())
}

async fn a_view_with_parameters_is_written_in_log_format_8(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(payment()).await?;
    let mut plain = account_window();
    plain.name = "account_totals".into();
    if let Some(v) = plain.view.as_mut() {
        v.params.clear();
        v.r#where = None;
        v.measures.truncate(1);
    }
    plain.fields.truncate(2);
    l.declare(plain).await?;
    let before = log_events(store.as_ref()).await?;
    let highest = before.iter().map(|e| e.v).max().unwrap_or(0);
    ensure!(
        highest < 8,
        "a view without parameters needs no format 8, yet the lake writes {highest}"
    );
    let declared = l.declare(account_window()).await?.position();
    let p1 = json!({"account": "a", "date": "2026-06-30", "amount": "1.00"});
    l.upsert("payment", "p1", p1).await?;
    for e in log_events(store.as_ref()).await? {
        let want = if e.position >= declared {
            8
        } else {
            e.v.min(7)
        };
        ensure!(
            e.v == want,
            "the event at {} is written in format {}; the one declaring a view with \
             parameters and every one after it must be 8, and none before it",
            e.position,
            e.v
        );
    }
    Ok(())
}
