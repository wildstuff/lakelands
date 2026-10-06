//! A computed field is read no further than every field it is computed
//! from (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! extended): what its expression reads, or its function's inputs, whether
//! or not its declaration lists them in `depends_on`.

use std::sync::Arc;

use lakelands_api::describe::FieldDescription;
use lakelands_api::event::EventBody;
use lakelands_api::principal::Grants;
use lakelands_api::query::{Projection, Search, SearchMode};
use lakelands_api::read::{AggFunc, Direction, OrderBy};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Migration, ProposedWrite};
use lakelands_api::{Access, Find, LakeStore, Position, Principal};
use serde_json::{Value, json};

use crate::conformance::backing::{self, KIND};
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, events_after, field_access, find_where, get_at, log_events, order,
    pattern, read_at,
};
use crate::conformance::{Failure, Reopen};

check!(
    /// A computed field is read as what it is computed from, listed or not.
    COMPUTED_AS_ITS_INPUTS = a_computed_field_is_read_no_further_than_what_it_is_computed_from,
    "024, extended (lakelands#540, lakelands#652): a field computed by an expression (one that uses `previous` or `changed` as well), by a function with an expression or by one with a backing, and a field computed from such a field, is not answered to a reader who may not know of a field it is computed from, whether or not `depends_on` lists it: not by `get`, a `where`, an `order_by`, an aggregate, a view's measure, a pattern's projection or a search; a masked input masks it; `describe` shows the completed list, the log holds the declaration as it was sent, and a lake that holds such a field is closed when it is opened again; a function declared again with fewer inputs restricts by those alone, a reader and an author alike; a proposed shape and a relation's link field are held to the same, and a listed proposal's `more` says what was left out of its shape, not what was completed",
    reopens
);

fn text(n: &str) -> Value {
    json!({"name": n, "type": {"kind": "text"}})
}

/// A searchable field `n` computed by `how`, listing `deps`.
fn computed(n: &str, how: Value, deps: &[&str]) -> Value {
    let mut f = json!({"name": n, "type": {"kind": "text"}, "authority": "derived",
                       "depends_on": deps, "searchable": true});
    for (k, v) in how.as_object().into_iter().flatten() {
        f[k] = v.clone();
    }
    f
}

/// The doors that answer `clerk` the value of `ty`'s `field`, read as of
/// `as_of`.
pub(super) async fn told(
    l: &Lake,
    clerk: &Principal,
    ty: &str,
    field: &str,
    as_of: Option<Position>,
) -> Vec<&'static str> {
    let (c, view) = (&l.cat, format!("v_{ty}_{field}"));
    let by = vec![order(field, OrderBy::Value, Direction::Asc)];
    let by = Find {
        order_by: by,
        ..Find::default()
    };
    let max = aggregate(vec![agg("m", AggFunc::Max, Some(field))]);
    let filter = find_where(&format!("{field} = 'SECRET' or {field} = 'secret'"));
    let fields = vec![format!("{ty}.{field}")];
    let mut rows = pattern(ty, Some("1"), vec![], Projection::Rows { fields });
    rows.at = read_at(as_of);
    let mode = SearchMode::lexical("secret");
    let (k, cursor, filtered, at) = (5, None, None, read_at(as_of));
    let search = Search {
        ty: ty.into(),
        mode,
        k,
        cursor,
        filter: filtered,
        with_meta: false,
        at,
    };
    let mut told = Vec::new();
    let mut answer = |door, text: lakelands_api::Result<String>| {
        if text.is_ok_and(|s| s.to_lowercase().contains("secret")) {
            told.push(door);
        }
    };
    let one = c.get(clerk, &ty.into(), get_at("1", as_of)).await;
    answer("get", one.map(|r| format!("{:?}", r.data)));
    for (door, on, mut find) in [
        ("where", ty, filter),
        ("order_by", ty, by),
        ("aggregate", ty, max),
        ("view", view.as_str(), Find::default()),
    ] {
        find.at = read_at(as_of);
        let found = c.find(clerk, &on.into(), find).await;
        answer(door, found.map(|r| format!("{:?}", r.data)));
    }
    answer(
        "pattern",
        c.query(clerk, rows).await.map(|r| format!("{:?}", r.data)),
    );
    answer(
        "search",
        c.search(clerk, search)
            .await
            .map(|r| format!("{:?}", r.data)),
    );
    told
}

/// `ty`'s field `x` as `who` is described it.
async fn x_of(l: &Lake, who: &Principal, ty: &str) -> Result<Option<FieldDescription>, Failure> {
    let seen = l
        .cat
        .describe_type(who, &ty.into(), Default::default())
        .await;
    let fields = seen.must("describing the type")?.data.fields.into_iter();
    Ok(fields.into_iter().find(|f| f.name.as_str() == "x"))
}

/// Each type, and the computed fields of it a reader kept from `b` must
/// not be answered.
const WAYS: [(&str, &[&str]); 9] = [
    ("by_expr", &["x"]),
    ("by_previous", &["x"]),
    ("by_changed", &["x"]),
    ("by_function", &["x"]),
    ("by_backing", &["x"]),
    ("chain_in", &["y"]),
    ("chain_out", &["x", "y"]),
    ("by_pair", &["x"]),
    ("mutual", &["x", "y"]),
];

/// The reader: it reads all but `b`, which it holds at `b`.
fn clerk(s: &dyn LakeStore, ty: &str, b: Access) -> Principal {
    field_access(s, "clerk", ty, &[("b", b)])
}

/// Every computed field that reader is answered, with the doors.
async fn leaks(store: &dyn LakeStore) -> String {
    let (l, mut red) = (Lake::of(store), Vec::new());
    for (ty, fields) in WAYS {
        for field in fields {
            let doors = told(&l, &clerk(store, ty, Access::None), ty, field, None).await;
            if !doors.is_empty() {
                red.push(format!("`{ty}.{field}` through {doors:?}"));
            }
        }
    }
    red.join("; ")
}

async fn a_computed_field_is_read_no_further_than_what_it_is_computed_from(
    store: Arc<dyn LakeStore>,
    reopen: Reopen,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let function = |name: &str, inputs: &[&str], backing: Value| {
        let inputs: Vec<Value> = inputs.iter().map(|n| text(n)).collect();
        let mut f = json!({"declare": "function", "name": name, "inputs": inputs});
        for (k, v) in backing.as_object().into_iter().flatten() {
            f[k] = v.clone();
        }
        f
    };
    let expr = |e: &str| json!({"backing": "expr", "expr": e});
    let backed = json!({"backing": KIND, "config": {"token": "inputs", "echo": "b"}});
    for f in [
        function("shout", &["b"], expr("upper(b)")),
        function("lookup", &["b"], backed),
        function("pair", &["a", "b"], expr("upper(b)")),
    ] {
        l.declare_json(f).await?;
    }
    // Each way a field is computed from `b`, none listing it; a chain
    // that lists it at one link and not at the other; `x` reads `y`,
    // which lists `x`.
    let upper = || json!({"derived": "upper(b)"});
    let lower = || json!({"derived": "lower(x)"});
    let derived = |e: &str| json!({ "derived": e });
    let by = |function: &str| json!({"function": function});
    let ways = [
        vec![computed("x", upper(), &[])],
        vec![computed("x", derived("previous(b)"), &[])],
        vec![computed("x", derived("if(changed(a), b, b)"), &[])],
        vec![computed("x", by("shout"), &[])],
        vec![computed("x", by("lookup"), &[])],
        vec![computed("x", upper(), &["b"]), computed("y", lower(), &[])],
        vec![computed("x", upper(), &[]), computed("y", lower(), &["x"])],
        vec![computed("x", by("pair"), &[])],
        vec![
            computed("x", json!({"derived": "upper(y)"}), &[]),
            computed("y", upper(), &["x"]),
        ],
    ];
    for ((ty, fields), computed) in WAYS.into_iter().zip(ways) {
        let all: Vec<Value> = [text("a"), text("b")].into_iter().chain(computed).collect();
        l.declare_json(json!({"declare": "type", "name": ty, "fields": all}))
            .await?;
        l.upsert(ty, "1", json!({"a": "1", "b": "Secret"})).await?;
        // `previous(b)` holds a value once `b` is written again.
        if ty == "by_previous" {
            l.upsert(ty, "1", json!({"b": "plain"})).await?;
        }
        for field in fields {
            let measures = json!([{"name": "m", "expr": format!("max({field})")}]);
            let view = json!({"source": ty, "group_by": ["a"], "measures": measures});
            let name = format!("v_{ty}_{field}");
            let fields = json!([text("a"), text("m")]);
            l.declare_json(json!({"declare": "type", "name": name, "key": ["a"],
                                  "fields": fields, "view": view}))
                .await?;
        }
    }
    let red = leaks(store.as_ref()).await;
    ensure!(
        red.is_empty(),
        "a reader who may not know of `b` must be answered no field computed from it; it is \
         answered {red}"
    );

    // A masked input masks; `describe` shows the completed list.
    let masked = clerk(store.as_ref(), "by_function", Access::Masked);
    let x = x_of(&l, &masked, "by_function").await?;
    let listed = x_of(&l, &l.own, "by_function").await?;
    let listed = listed.map(|f| f.depends_on).unwrap_or_default();
    ensure!(
        x.as_ref().is_some_and(|x| x.access == Access::Masked) && listed == ["b"],
        "a field computed from a masked one is masked, and the owner is described \
         `depends_on: [b]`; `x` is {x:?}, the list {listed:?}"
    );
    // A shape that is not in force is completed before it is judged: a
    // proposed migration that adds `z`, computed from `b`.
    let mut reviewer = clerk(store.as_ref(), "by_expr", Access::None);
    reviewer.grants.review = true;
    let fields = [
        text("a"),
        text("b"),
        computed("x", upper(), &[]),
        computed("z", upper(), &[]),
    ];
    let to = serde_json::from_value(json!({"name": "by_expr", "fields": fields}));
    let migration = Box::new(Migration {
        to: to.expect("a type"),
        renames: vec![],
        relation_renames: Vec::new(),
    });
    let write = ProposedWrite::Migrate {
        migration,
        report: Default::default(),
    };
    l.cat
        .propose(&l.own, write, "adding z".into())
        .await
        .must("proposing `z`")?;
    let listed = l
        .cat
        .proposals(&reviewer, Default::default())
        .await
        .must("listing")?
        .data;
    let shown: Vec<_> = listed
        .iter()
        .map(|p| (json!(p.write).to_string(), p.more))
        .collect();
    ensure!(
        matches!(shown.as_slice(), [(to, true)] if !to.contains("\"z\"") && !to.contains("\"x\"")),
        "a proposed shape is shown without a field computed from one its reader may not know \
         of, and says `more`; the reviewer lists {shown:?}"
    );
    // To the owner nothing is left out: a proposed type too is shown with
    // the completed lists, and neither entry says `more`.
    let fields = [text("a"), text("b"), computed("x", upper(), &[])];
    let new = json!({"declare": "type", "name": "new", "fields": fields});
    let declarations = vec![serde_json::from_value(new).expect("a type")];
    let bundle = ProposedWrite::Declare { declarations };
    let proposed = l.cat.propose(&l.own, bundle, "a type".into()).await;
    proposed.must("proposing a type")?;
    let listed = l.cat.proposals(&l.own, Default::default()).await;
    let listed = listed.must("listing")?.data.into_iter();
    let lists = |w| {
        json!(w)
            .to_string()
            .matches(r#""depends_on":["b"]"#)
            .count()
    };
    let mut shown: Vec<_> = listed.map(|p| (lists(p.write), p.more)).collect();
    shown.sort();
    ensure!(
        shown == [(1, false), (2, false)],
        "a proposed type and a proposed migration are shown to the owner with each computed \
         field's completed list, and `more` says only what was left out; the owner lists, by \
         completed lists and `more`, {shown:?}"
    );
    // The log holds a declaration as it was sent.
    let before = store.head().await.must("reading the head")?;
    let mut all = vec![text("a"), text("b")];
    all.push(computed("x", upper(), &[]));
    l.declare_json(json!({"declare": "type", "name": "by_expr", "label": "again", "fields": all}))
        .await?;
    let written = events_after(store.as_ref(), before).await?;
    let sent = written.iter().filter_map(|e| match &e.body {
        EventBody::Declared { declaration, .. } => Some(json!(declaration).to_string()),
        _ => None,
    });
    let sent: Vec<String> = sent.collect();
    ensure!(
        sent.len() == 1 && !sent[0].contains("depends_on"),
        "a declaration sent again with a label is logged once, as it was sent, without a \
         completed list; the log holds {sent:?}"
    );
    // A function declared again with fewer inputs restricts by those alone,
    // for a reader and for an author: its view of `by_pair` takes `x` away
    // only while `x` is its to see, and the preview is refused then, for
    // the view that reads `x`.
    let free = field_access(store.as_ref(), "free", "by_pair", &[("b", Access::None)]);
    let author = Principal {
        grants: Grants {
            schema: true,
            ..free.grants.clone()
        },
        ..free.clone()
    };
    let view = serde_json::from_value(json!({"name": "by_pair", "fields": [text("a")]}));
    let view = Migration {
        to: view.expect("a type"),
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let mut reads = Vec::new();
    for inputs in [&["a", "b"][..], &["a"], &["a", "b"]] {
        let of = if inputs.len() == 2 {
            "upper(b)"
        } else {
            "upper(a)"
        };
        let pair = function("pair", inputs, expr(of));
        l.declare_json(pair).await?;
        let kept = l.cat.migrate(&author, view.clone()).await;
        let kept = kept.is_ok_and(|r| r.data.dropped.is_empty());
        reads.push((x_of(&l, &free, "by_pair").await?.is_some(), !kept));
    }
    ensure!(
        reads == [(false, false), (true, true), (false, false)],
        "`pair` reads `a` and `b`, then `a` alone, then both again: one who may not know of \
         `b` must be shown `x`, and may take it away, only in between; it was {reads:?}"
    );
    // A relation's link field computed from one its reader may not know of.
    let fields = json!([
        text("note"),
        computed("echo", json!({"derived": "upper(note)"}), &[])
    ]);
    let signed = json!([{"name": "signed_by", "target": "by_expr", "fields": fields}]);
    l.declare_json(
        json!({"declare": "type", "name": "doc", "fields": [text("a")], "relations": signed}),
    )
    .await?;
    let linked = field_access(
        store.as_ref(),
        "linked",
        "sys.link.doc.signed_by",
        &[("note", Access::None)],
    );
    let doc = l
        .cat
        .describe_type(&linked, &"doc".into(), Default::default())
        .await;
    let doc = json!(doc.must("describing `doc`")?.data.relations).to_string();
    ensure!(
        !doc.contains("echo") && !doc.contains("note"),
        "a link field computed from one its reader may not know of is not described; `doc` \
         is described the relations {doc}"
    );
    // A lake that holds such fields opens again, closed, its log untouched.
    let log = log_events(store.as_ref()).await?;
    let seams = Seams {
        backings: backing::backings(),
        ..Seams::default()
    };
    let again = reopen(store, seams).await;
    let store = again.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let (l, red) = (Lake::of(store.as_ref()), leaks(store.as_ref()).await);
    l.upsert("mutual", "1", json!({"b": "Secret"})).await?;
    ensure!(
        red.is_empty() && log_events(store.as_ref()).await?.starts_with(&log),
        "opened again, the lake reads and writes as before, its log as it was, and answers \
         no field computed from `b`; it answers {red}"
    );
    Ok(())
}
