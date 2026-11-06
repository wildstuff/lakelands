//! What a walk, a view and a link type answer at a `valid_at` before the
//! lake declared anything (lakelands#1405): the valid time of a record is
//! the only thing a read at `valid_at` asks of it, so an instant before
//! the lake existed is read as any other instant is.

use super::links::{lake, link_ok};
use crate::conformance::fixture::step;
use crate::conformance::prelude::*;

check!(
    BEFORE_DECLARED = a_read_at_a_valid_time_before_the_declaration_is_read_as_any_instant_is,
    "035 and 026, extended (lakelands#1405): a read at a `valid_at` before the lake declared anything is read as one at any other instant, by a table, a walk, a view and a link type alike: a type with no valid time holds at that instant and a valid-timed one holds the records valid then, none before its first; a walk, in a graph and in rows, from either end of a relation, reaches the records valid at the instant and no edge to one that is not, so a start not valid then is a walk of nothing and an end not valid then is a start alone; a view of a type with no valid time counts every record and one of a valid-timed type counts those valid at the instant, none before the first; a link type has no valid time of its own, so its links are all answered at the instant, those to an end that is not valid then among them, which the walk over the relation leaves out"
);

/// A read at `day`.
fn on(day: &str) -> ReadAt {
    let valid_at = Timestamp::parse(&format!("{day}T00:00:00Z")).expect("an instant");
    ReadAt {
        valid_at: Some(valid_at),
        ..ReadAt::default()
    }
}

/// The keys `find` answers of `ty` at `at`, in order.
async fn found(l: &Lake, ty: &str, at: ReadAt) -> Result<Vec<String>, Failure> {
    let find = Find {
        at,
        ..Find::default()
    };
    let rows = l.all_found(ty, find).await?;
    Ok(rows.iter().map(|r| r.key.to_string()).collect())
}

/// What a walk of `relation` from `ty` (`key`, or every record) answers at
/// `at`: the nodes of its graph and its edges, and the rows' keys.
async fn walked(
    l: &Lake,
    (ty, key, relation): (&str, Option<&str>, &str),
    at: ReadAt,
) -> Result<String, Failure> {
    let mut said = Vec::new();
    for project in [Projection::Graph, Projection::Rows { fields: vec![] }] {
        let mut walk = pattern(ty, key, vec![step(relation, Walk::Out)], project);
        walk.at = at;
        let answer = l.cat.query(&l.own, walk).await;
        match answer
            .must(&format!("walking `{relation}` from `{ty}`"))?
            .data
        {
            QueryResult::Graph { nodes, edges, .. } => {
                let mut nodes: Vec<_> = nodes.iter().map(|n| n.key.to_string()).collect();
                nodes.sort();
                let mut edges: Vec<_> = edges
                    .iter()
                    .map(|e| format!("{}>{}", e.from.1, e.to.1))
                    .collect();
                edges.sort();
                said.push(format!("nodes {nodes:?} edges {edges:?}"));
            }
            QueryResult::Rows { rows, .. } => said.push(format!("{} rows", rows.len())),
            other => return Err(Failure::new(format!("a walk answered {other:?}"))),
        }
    }
    Ok(said.join(", "))
}

async fn a_read_at_a_valid_time_before_the_declaration_is_read_as_any_instant_is(
    store: Store,
) -> Held {
    // Partners p1 and p2, invoices i1 and i2 and the link `i1 → p1`: no
    // valid time among them.
    let l = lake(store.as_ref()).await?;
    link_ok(&l, "i1", "p1", json!({"since": "2026-09-01T00:00:00Z"})).await?;
    let (dt, tx) = (
        |n: &str| json!({"name": n, "type": {"kind": "date_time"}}),
        |n: &str| json!({"name": n, "type": {"kind": "text"}}),
    );
    // Gigs valid from September and November, linked to `p1`.
    l.declare_json(
        json!({"declare": "type", "name": "gig", "valid_from": "start",
        "fields": [dt("start"), tx("g"), tx("kind")],
        "relations": [{"name": "hosted_by", "target": "partner", "cardinality": "many_to_one",
            "inverse": "hosts", "fields": [dt("since")]}]}),
    )
    .await?;
    // Views over one group each: `n` counts the records of the source
    // that hold at the instant.
    let view = |name: &str, source: &str, key: &str, kind: &str| {
        json!({"declare": "type", "name": name, "key": [key],
            "fields": [{"name": key, "type": {"kind": kind}}, {"name": "n", "type": {"kind": "number"}}],
            "view": {"source": source, "group_by": [key],
                     "measures": [{"name": "n", "expr": "count()"}]}})
    };
    l.declare_json(view("gigs", "gig", "kind", "text")).await?;
    l.declare_json(view("invoiced", "invoice", "amount", "decimal"))
        .await?;
    for (key, start) in [("g1", "2026-09-01"), ("g2", "2026-11-01")] {
        let at = format!("{start}T00:00:00Z");
        l.upsert("gig", key, json!({"start": at, "g": key, "kind": "show"}))
            .await?;
        let (ty, rel) = ("gig".into(), "hosted_by".into());
        let (from, to) = (key.into(), "p1".into());
        let fields = patch(json!({"since": at}));
        let linked = l
            .cat
            .link(&l.own, &ty, &from, &rel, &to, fields, WriteOpts::default());
        linked.await.must("linking a gig to p1")?;
    }
    let (past, october, head) = (on("2000-01-01"), on("2026-10-01"), ReadAt::default());
    // A table is the row to follow: what holds at the instant is answered.
    for (ty, want) in [
        ("partner", ["p1 p2", "p1 p2", "p1 p2"]),
        ("gig", ["", "g1", "g1 g2"]),
    ] {
        for (at, want) in [past, october, head].into_iter().zip(want) {
            let got = found(&l, ty, at).await?.join(" ");
            ensure!(
                got == want,
                "`{ty}` read at {:?} answers {want:?}; it answered {got:?}",
                at.valid_at
            );
        }
    }
    // A walk reads the valid time of each record it reaches.
    let walks = [
        (
            ("invoice", Some("i1"), "approved_by"),
            ["nodes [\"i1\", \"p1\"] edges [\"i1>p1\"], 1 rows"; 3],
        ),
        (
            ("gig", Some("g1"), "hosted_by"),
            [
                "nodes [] edges [], 0 rows",
                "nodes [\"g1\", \"p1\"] edges [\"g1>p1\"], 1 rows",
                "nodes [\"g1\", \"p1\"] edges [\"g1>p1\"], 1 rows",
            ],
        ),
        (
            ("partner", Some("p1"), "hosts"),
            [
                "nodes [\"p1\"] edges [], 0 rows",
                "nodes [\"g1\", \"p1\"] edges [\"g1>p1\"], 1 rows",
                "nodes [\"g1\", \"g2\", \"p1\"] edges [\"g1>p1\", \"g2>p1\"], 2 rows",
            ],
        ),
    ];
    for (from, want) in walks {
        for (at, want) in [past, october, head].into_iter().zip(want) {
            let got = walked(&l, from, at).await?;
            ensure!(
                got == want,
                "walking `{}` from `{}` {:?} read at {:?} answers {want:?}; it answered {got:?}",
                from.2,
                from.0,
                from.1,
                at.valid_at
            );
        }
    }
    // A view counts what holds at the instant of its source.
    for (view, want) in [
        ("invoiced", ["10=2", "10=2", "10=2"]),
        ("gigs", ["", "show=1", "show=2"]),
    ] {
        for (at, want) in [past, october, head].into_iter().zip(want) {
            let rows = l
                .all_found(
                    view,
                    Find {
                        at,
                        ..Find::default()
                    },
                )
                .await?;
            let got: Vec<_> = rows
                .iter()
                .map(|r| format!("{}={}", r.key, r.fields["n"]))
                .collect();
            let got = got.join(" ");
            ensure!(
                got == want,
                "the view `{view}` read at {:?} counts {want:?}; it answered {got:?}",
                at.valid_at
            );
        }
    }
    // A link type has no valid time of its own.
    let links = "sys.link.gig.hosted_by";
    for at in [past, october, head] {
        let got = found(&l, links, at).await?.join(" ");
        ensure!(
            got == "g1:p1 g2:p1",
            "`{links}` read at {:?} answers both links, whatever holds of their ends; it answered {got:?}",
            at.valid_at
        );
    }
    Ok(())
}
