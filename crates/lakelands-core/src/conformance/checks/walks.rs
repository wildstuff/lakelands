//! The one pattern query: steps over declared relations, bounded depth,
//! one order (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use lakelands_api::ontology::{RelationDecl, StorageKind};
use lakelands_api::query::{Depth, Pattern, QueryResult, Step, Walk};
use lakelands_api::{Error, LakeStore, RecordKey, Refusal, TypeName, Value};
use serde_json::json;

use crate::conformance::fixture::{Lake, Must, field, limits, reason, reference, type_decl};
use crate::conformance::{Failure, Held, Store};

check!(
    UNDECLARED_RELATION = a_step_over_a_relation_the_type_does_not_declare_is_refused,
    "018: the planner knows the ontology; a step over an undeclared relation is refused, not answered empty"
);

check!(
    TOO_DEEP = a_step_deeper_than_the_lakes_limit_is_refused,
    "018 and the location's limits: on a lake opened with the default limits, a step whose depth exceeds `max_depth` is refused rather than walked"
);

check!(
    WALK_ORDER = a_walk_answers_what_it_reaches_in_one_order,
    "026 and 029: a graph includes its start, and a walk orders what it reaches by depth, type and key, so no engine's link order shows through"
);

check!(
    INVERSE_STEP = a_step_naming_an_inverse_walks_the_relation_the_other_way,
    "018 and 044, extended: a step naming a relation's `inverse` walks that relation the other way, its `walk` counting from the name it gives, and the edges keep the declared relation with its declaring side as `from`"
);

check!(
    STEP_BINDS_ONE_TYPE = a_step_whose_records_a_row_binds_reaches_one_type,
    "018, extended (lakelands#680, lakelands#990): a step whose records rows or an aggregate bind reaches one type over its whole range or names its `type`, unless it is the last step and nothing reads its fields: over a relation two types declare it is refused where another step follows it, in a chain and from depth 0, at its step under the relation as its caller wrote it and naming every type it reaches; as the last step, at the first or a later one, in a chain and from depth 0, one whose second hop reaches other types and one from depth 0 that reaches another type than it stands on, it is answered under the relation's name with each record's key, `_type` and `_depth` and no field, counted over every type it reaches and grouped by `_type`, a type its reader is not shown neither counted nor named; a `where` over such a step is refused alike for rows, a graph and a path, and with the `type` of either declarer it is read against that type's records (lakelands#753), and a chain's graph is judged over its range as its rows are, a second hop's type named and a step from depth 0 with a `where` naming one, and its step that another follows names its `type` while a last one keeps every type it reaches; a `type` the step does not reach is refused naming every type it does, at depth 0 as in a chain, and degree and components take no step `type` at all; with its `type` it binds that type alone, as a step by an inverse binds its owner's, a range that reaches nothing is judged by its first hop, and the step after it is read from that type, by an order, `fields`, a group key, a `where` and a `type` of its own; a reader masked from the field that carries the first hop is answered that before the step is judged, in rows, an aggregate and a graph, and a relation a neighbourhood meets around another type when it is walked and not before; a path asked as rows or an aggregate is refused as that, and rows from depth 0 over a relation their type does not declare as a chain's are; a reader not shown one of the two types is answered as on a lake without it, by a measure too"
);

check!(
    OPEN_STEP = a_last_step_over_several_types_reads_the_lakes_columns_alone,
    "018, extended (lakelands#990): under a name a last step over several types binds, `key`, `_type` and `_depth` are the lake's columns, read against no type and never probed, so a field `key` its reader may not know of, on either declarer in either order of declaration, turns neither its rows, nor an order or a group by it, and the owner reads the record's key; `_type` orders rows, is projected and is read by a key's expression; rows across the types page by an order of `_type` and the key; a relation carried by link records is bound alike; a reader kept by `only` counts what it is shown; at a `valid_at` the step probes the valid time of every type it may meet, so a reader masked on one is refused that; a step by an inverse's name binds under it; `as` gives a reader shown both types and one shown one the same columns"
);

check!(
    ROW_NAMES = a_row_binds_each_record_under_its_name,
    "018, extended (lakelands#680): a row binds each record a pattern reaches under the name `as` gives, or its type's: a chain that reaches one type three times answers each record under its own name, a step that would bind a name again is refused at its step and `as` on a graph, a path, degree and components as naming nothing, an aggregate groups by a named path, a field its reader may not know of answers under a name as on its type, and a name that is no identifier is refused"
);

check!(
    ROWS_HOLD_MATCHED_STARTS = a_rows_projection_holds_a_start_only_where_a_match_begins,
    "026: a rows projection holds a start only where its steps reach a match from it, and every row binds its start beside what its steps reach: of the people, one with no boss, whom nobody reports to, is in no row of a step that needs a boss and is a node of the same pattern's graph, and a step from depth 0 binds every start, that one too"
);

/// People who report to people, through `boss`.
async fn org(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let mut ty = type_decl(
        "person",
        vec![
            field("name", StorageKind::Text),
            reference("boss", "person"),
        ],
    );
    ty.relations = vec![RelationDecl {
        name: "reports_to".into(),
        target: "person".into(),
        inverse: Some("manages".into()),
        via: Some("boss".into()),
        ..Default::default()
    }];
    l.declare(ty).await?;
    for (key, boss) in [
        ("ceo", None),
        ("vp", Some("ceo")),
        ("zed", Some("vp")),
        ("amy", Some("vp")),
    ] {
        let mut fields = json!({"name": key});
        if let Some(b) = boss {
            fields["boss"] = json!(b);
        }
        l.upsert("person", key, fields).await?;
    }
    Ok(l)
}

fn step(relation: &str, walk: Walk, min: u32, max: u32) -> Step {
    let depth = Depth { min, max };
    Step {
        depth,
        ..crate::conformance::fixture::step(relation, walk)
    }
}

/// A step out over `reports_to` whose record a row binds as `boss`, apart
/// from the start's (018, extended).
fn up(min: u32, max: u32) -> Step {
    Step {
        named: Some("boss".into()),
        ..step("reports_to", Walk::Out, min, max)
    }
}

fn from(key: &str, steps: Vec<Step>) -> Pattern {
    Pattern::traverse(TypeName::from("person"), RecordKey::from(key), steps)
}

async fn a_step_over_a_relation_the_type_does_not_declare_is_refused(store: Store) -> Held {
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let fut = cat.query(own, from("zed", vec![up(1, 1)]));
    fut.await.must("walking `reports_to` from `zed`")?;
    let err = cat
        .query(own, from("zed", vec![step("audited_by", Walk::Out, 1, 1)]))
        .await
        .must_fail("walking the undeclared relation `audited_by`")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::Invalid(..))),
        "a step over the undeclared relation `audited_by` must be refused as invalid; \
         it failed with {err:?}"
    );
    Ok(())
}

async fn a_step_deeper_than_the_lakes_limit_is_refused(store: Store) -> Held {
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let limit = limits().max_depth;
    let fut = cat.query(own, from("zed", vec![up(1, limit)]));
    fut.await.must(&format!(
        "walking `reports_to` up to the default limit of {limit}"
    ))?;
    let err = cat
        .query(own, from("zed", vec![up(1, limit + 1)]))
        .await
        .must_fail(&format!("walking `reports_to` {} deep", limit + 1))?;
    ensure!(
        matches!(err, Error::Refused { .. }),
        "a step {} deep, past the lake's max_depth of {limit}, must be refused; it \
         failed with {err:?}",
        limit + 1
    );
    Ok(())
}

async fn a_walk_answers_what_it_reaches_in_one_order(store: Store) -> Held {
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let mut p = from("ceo", vec![step("reports_to", Walk::In, 1, 2)]);
    p.project = lakelands_api::query::Projection::Graph;
    let reply = cat
        .query(own, p)
        .await
        .must("walking `reports_to` backwards two deep from `ceo`")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["ceo", "vp", "amy", "zed"],
        "walking down from `ceo` two deep must answer the start, then depth 1 (vp), \
         then depth 2 by key (amy, zed); it answered {keys:?}"
    );
    Ok(())
}

check!(
    CYCLE_TERMINATES = a_walk_over_a_cycle_ends_and_answers_each_record_once,
    "026: the store walks the graph, so a walk over a cycle of links ends within its depth and reaches every record once"
);

async fn a_walk_over_a_cycle_ends_and_answers_each_record_once(store: Store) -> Held {
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // The ceo reports to the one who reports to the ceo: a cycle of two.
    l.upsert("person", "ceo", json!({"boss": "vp"})).await?;
    let mut p = from(
        "ceo",
        vec![step("reports_to", Walk::Out, 1, limits().max_depth)],
    );
    p.project = lakelands_api::query::Projection::Graph;
    let reply = cat
        .query(own, p)
        .await
        .must("walking `reports_to` around the cycle ceo → vp → ceo")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    let mut keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    let all = keys.len();
    keys.sort();
    keys.dedup();
    ensure!(
        keys == ["ceo", "vp"] && all == 2,
        "a walk around the cycle ceo → vp → ceo must answer ceo and vp, each once; it \
         answered {all} node(s): {keys:?}"
    );
    Ok(())
}

async fn a_step_naming_an_inverse_walks_the_relation_the_other_way(store: Store) -> Held {
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    for (walk, declared) in [
        (Walk::Out, Walk::In),
        (Walk::In, Walk::Out),
        (Walk::Both, Walk::Both),
    ] {
        let mut answers = Vec::new();
        for (name, w) in [("manages", walk), ("reports_to", declared)] {
            let mut p = from("vp", vec![step(name, w, 1, 1)]);
            p.project = lakelands_api::query::Projection::Graph;
            let what = format!("walking `{name}` {w:?} from `vp`");
            answers.push(cat.query(own, p).await.must(&what)?.data);
        }
        ensure!(
            answers[0] == answers[1],
            "`manages` walked {walk:?} from `vp` answers what `reports_to` walked {declared:?} \
             does, {:?}; it answered {:?}",
            answers[1],
            answers[0]
        );
    }
    Ok(())
}

async fn a_row_binds_each_record_under_its_name(store: Store) -> Held {
    use crate::conformance::fixture::{
        field_access, invalid_case, invalid_said, no_such_field, order,
    };
    use lakelands_api::Access;
    use lakelands_api::error::{Case, Naming, Place, Said};
    use lakelands_api::query::Projection;
    use lakelands_api::read::{Direction, OrderBy};
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let top = Step {
        named: Some("top".into()),
        ..step("reports_to", Walk::Out, 1, 1)
    };
    let p = from("zed", vec![up(1, 1), top]);
    let rows = match cat.query(own, p).await.map(|r| r.data) {
        Ok(QueryResult::Rows { rows, .. }) => rows,
        other => return Err(Failure::new(format!("zed's chain answered {other:?}"))),
    };
    let names: Vec<_> = ["person.name", "boss.name", "top.name"]
        .iter()
        .map(|n| rows.first().and_then(|r| r.get(*n)).and_then(Value::as_str))
        .collect();
    ensure!(
        rows.len() == 1 && names == [Some("zed"), Some("vp"), Some("ceo")],
        "a chain from zed binds zed, vp and ceo under `person`, `boss` and `top`; it answered \
         {rows:?}"
    );
    // One name, one record: the second `person` is refused, not written
    // over the first, and nothing but rows and an aggregate binds by name.
    let unnamed = from("zed", vec![step("reports_to", Walk::Out, 1, 1)]);
    let twice = invalid_said(&cat.query(own, unnamed).await).cloned();
    let (name, relation) = ("person".into(), "reports_to".into());
    let binds = Said::from(Case::PatternBindsTwice { name });
    let mut not_rows = Vec::new();
    for (project, to) in [
        (Projection::Graph, None),
        (Projection::Graph, Some("vp")),
        (Projection::Degree, None),
        (Projection::Components, None),
    ] {
        let mut p = from("zed", vec![up(0, 1)]);
        (p.project, p.to) = (project, to.map(|k| ("person".into(), k.into())));
        not_rows.push(invalid_case(&cat.query(own, p).await).cloned());
    }
    ensure!(
        twice == Some(binds.within(Place::Step { step: 1, relation }))
            && not_rows.iter().all(|c| *c == Some(Case::PatternAsNotRows)),
        "a step binding `person` again is refused at its step, and `as` on a graph, a path, \
         degree and components as naming nothing; they answered {twice:?} and {not_rows:?}"
    );
    let a = serde_json::from_value(json!({
        "measures": [{"name": "n", "func": "count"}], "group_by": ["boss.name"]}))
    .map_err(|e| Failure::new(e.to_string()))?;
    let mut by_boss = from("zed", vec![up(1, 1)]);
    by_boss.start.key = None;
    by_boss.project = Projection::Aggregate(a);
    let groups = match cat.query(own, by_boss.clone()).await.map(|r| r.data) {
        Ok(QueryResult::Groups { rows }) => rows
            .iter()
            .map(|g| format!("{}={}", g.by["boss.name"], g.values["n"]))
            .collect::<Vec<_>>()
            .join("|"),
        other => format!("{other:?}"),
    };
    ensure!(
        groups == r#""ceo"="1"|"vp"="2""#,
        "people grouped by `boss.name`: ceo 1, vp 2; it answered {groups}"
    );
    let clerk = field_access(store.as_ref(), "clerk", "person", &[("name", Access::None)]);
    by_boss.project = Projection::Rows { fields: Vec::new() };
    by_boss.order_by = vec![order("boss.name", OrderBy::Value, Direction::Asc)];
    let hidden = cat.query(&clerk, by_boss.clone()).await;
    ensure!(
        no_such_field(&hidden, "name"),
        "rows ordered by `boss.name` for a clerk who may not know of `name` are refused as for \
         a field `person` does not declare; it answered {hidden:?}"
    );
    by_boss.start.named = Some("a.b".into());
    let said = invalid_case(&cat.query(own, by_boss).await).cloned();
    let (what, name) = (Naming::Binding, "a.b".into());
    ensure!(
        said == Some(Case::NameNotIdentifier { what, name }),
        "a pattern binding a name `a.b` is refused as no identifier; it answered {said:?}"
    );
    Ok(())
}

async fn a_rows_projection_holds_a_start_only_where_a_match_begins(store: Store) -> Held {
    use lakelands_api::query::Projection;
    let l = org(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    l.upsert("person", "sol", json!({"name": "sol"})).await?;
    // Every person as a start; each row as `start>boss`, in one order.
    let people = |step: Step, project: Projection| {
        let mut p = from("zed", vec![step]);
        (p.start.key, p.project) = (None, project);
        cat.query(own, p)
    };
    let mut said = Vec::new();
    for min in [1, 0] {
        let rows = Projection::Rows { fields: Vec::new() };
        let mut pairs = match people(up(min, 1), rows).await.map(|r| r.data) {
            Ok(QueryResult::Rows { rows, .. }) => (rows.iter())
                .map(|r| {
                    let cell = |n| r.get(n).and_then(Value::as_str).unwrap_or("-");
                    [cell("person.key"), cell("boss.key")].join(">")
                })
                .collect::<Vec<_>>(),
            other => vec![format!("{other:?}")],
        };
        pairs.sort();
        said.push(pairs.join(" "));
    }
    let drawn = people(step("reports_to", Walk::Out, 1, 1), Projection::Graph);
    let mut nodes = match drawn.await.map(|r| r.data) {
        Ok(QueryResult::Graph { nodes, .. }) => (nodes.iter())
            .map(|n| n.key.as_str().to_owned())
            .collect::<Vec<_>>(),
        other => vec![format!("{other:?}")],
    };
    nodes.sort();
    said.push(nodes.join(" "));
    ensure!(
        said == [
            "amy>vp vp>ceo zed>vp",
            "amy>amy amy>vp ceo>ceo sol>sol vp>ceo vp>vp zed>vp zed>zed",
            "amy ceo sol vp zed"
        ],
        "rows hold a start only where a match begins at it, `sol` and `ceo` in none of a step \
         that needs a boss, every start in those of a step from depth 0, and the graph every \
         start: {said:#?}"
    );
    Ok(())
}

/// Bills and quotes, each `of` a customer, whose `bills` the bills alone
/// are; a bill is `by` a seller and a quote `by` a ware.
async fn trade(store: &dyn LakeStore) -> Result<Lake, Failure> {
    use crate::conformance::fixture::type_of;
    let l = Lake::of(store);
    let paper = |name: &str, own: &str, by: (&str, &str)| {
        let fields = vec![
            field(own, StorageKind::Text),
            reference("cust", "customer"),
            reference(by.0, by.1),
        ];
        let mut ty = type_decl(name, fields);
        ty.relations = vec![
            RelationDecl {
                via: Some("cust".into()),
                inverse: (name == "bill").then(|| "bills".into()),
                ..RelationDecl::to("of", "customer")
            },
            RelationDecl {
                via: Some(by.0.into()),
                ..RelationDecl::to("by", by.1)
            },
        ];
        ty
    };
    for (name, own) in [("customer", "tier"), ("seller", "s"), ("ware", "z")] {
        l.declare(type_of(name, &[(own, StorageKind::Text)]))
            .await?;
    }
    l.declare(paper("bill", "amount", ("from", "seller")))
        .await?;
    l.declare(paper("quote", "x", ("item", "ware"))).await?;
    for (ty, key, fields) in [
        ("customer", "c1", json!({"tier": "gold"})),
        ("customer", "c2", json!({"tier": "tin"})),
        ("seller", "s1", json!({"s": "sam"})),
        ("ware", "w1", json!({"z": "m"})),
        ("ware", "w2", json!({"z": "a"})),
        (
            "bill",
            "b1",
            json!({"amount": "5", "cust": "c1", "from": "s1"}),
        ),
        (
            "quote",
            "q1",
            json!({"x": "qa", "cust": "c1", "item": "w1"}),
        ),
        (
            "quote",
            "q2",
            json!({"x": "qb", "cust": "c2", "item": "w2"}),
        ),
    ] {
        l.upsert(ty, key, fields).await?;
    }
    Ok(l)
}

/// What `who` is answered: each row's values at `read`, the groups, a
/// graph's size, or the refusal.
async fn answered(
    cat: &dyn lakelands_api::Catalog,
    who: &lakelands_api::Principal,
    p: Pattern,
    read: &[&str],
) -> String {
    use crate::conformance::fixture::invalid_said;
    let cell = |v: Option<&Value>| v.map_or("-".into(), |v| v.to_string());
    let said = cat.query(who, p).await;
    match said.as_ref().map(|r| &r.data) {
        Ok(QueryResult::Rows { rows, .. }) => (rows.iter())
            .map(|r| read.iter().map(|k| cell(r.get(*k))).collect::<String>())
            .collect::<Vec<_>>()
            .join(","),
        Ok(QueryResult::Groups { rows }) => (rows.iter())
            .map(|g| format!("{:?}={}", g.by.values().next(), g.values["n"]))
            .collect(),
        Ok(QueryResult::Graph { nodes, .. }) => format!("{} nodes", nodes.len()),
        Err(e) => format!("{:?}", invalid_said(&said).ok_or(e)),
    }
}

async fn a_step_whose_records_a_row_binds_reaches_one_type(store: Store) -> Held {
    use crate::conformance::fixture::{
        alike_renamed, field_access, order, pattern, principal, refused_on,
    };
    use lakelands_api::error::{Case, Place, Said};
    use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
    use lakelands_api::query::Projection::{self, Graph};
    use lakelands_api::read::{Direction, OrderBy};
    use lakelands_api::{Access, Principal};
    let l = trade(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let all = || Projection::Rows { fields: Vec::new() };
    let grouped = |by: &[&str]| {
        let a = json!({"measures": [{"name": "n", "func": "count"}], "group_by": by});
        Projection::Aggregate(serde_json::from_value(a).expect("an aggregate"))
    };
    let of = |walk, min, max| step("of", walk, min, max);
    let typed = |ty: &str, s: Step| Step {
        ty: Some(ty.into()),
        ..s
    };
    let kept = |filter: &str, s: Step| Step {
        filter: Some(filter.into()),
        ..s
    };
    let at = |step, relation: &str| Place::Step {
        step,
        relation: relation.into(),
    };
    let answer = |who: &Principal, p: Pattern, read: &'static [&'static str]| {
        let (cat, who) = (cat.clone(), who.clone());
        async move { answered(cat.as_ref(), &who, p, read).await }
    };
    let said = |s: Said| format!("{:?}", Ok::<_, ()>(&s));
    let several = |step, relation: &str, all: &[&str]| {
        let (reached, also) = (Some(all[0].into()), Some(all[1..].iter().collect()));
        let case = Case::StepSeveralTypes { reached, also };
        said(Said::from(case).within(at(step, relation)))
    };
    let mut wrong = Vec::new();
    // Refused where another step follows it: that step is judged from one
    // type, and a row holds one type under a name.
    for (min, types) in [
        (1, &["bill", "quote"][..]),
        (0, &["bill", "customer", "quote"]),
    ] {
        for project in [all(), grouped(&[])] {
            let steps = vec![of(Walk::In, min, 1), step("by", Walk::Out, 1, 1)];
            let got = answer(own, pattern("customer", None, steps, project), &[]).await;
            if got != several(1, "of", types) {
                wrong.push(format!("from depth {min} then `by`, with no `type`: {got}"));
            }
        }
    }
    // Answered where it is the last step and nothing reads its fields
    // (lakelands#990): under the relation's name, each record's key, type
    // and depth, and counted over every type it reaches.
    const COLS: &[&str] = &["of.key", "of._type", "of._depth", "of.amount"];
    let (first, at_0) = (vec![of(Walk::In, 1, 1)], vec![of(Walk::In, 0, 1)]);
    let second = vec![of(Walk::Out, 1, 1), of(Walk::In, 1, 1)];
    let second_0 = vec![typed("customer", of(Walk::Out, 0, 1)), of(Walk::In, 0, 1)];
    for (what, start, steps, want) in [
        (
            "a chain's first step",
            "customer",
            first,
            [r#""b1""bill"1-,"q1""quote"1-,"q2""quote"1-"#, r#"None="3""#],
        ),
        (
            "the same from depth 0",
            "customer",
            at_0,
            [
                r#""c1""customer"0-,"b1""bill"1-,"q1""quote"1-,"c2""customer"0-,"q2""quote"1-"#,
                r#"None="5""#,
            ],
        ),
        (
            "a chain's second step",
            "bill",
            second,
            [r#""b1""bill"1-,"q1""quote"1-"#, r#"None="2""#],
        ),
        (
            "a second step from depth 0",
            "bill",
            second_0,
            [
                r#""c1""customer"0-,"b1""bill"1-,"q1""quote"1-"#,
                r#"None="3""#,
            ],
        ),
        (
            "a second hop that reaches other types",
            "bill",
            vec![of(Walk::Both, 1, 2)],
            [
                r#""c1""customer"1-,"b1""bill"2-,"q1""quote"2-"#,
                r#"None="3""#,
            ],
        ),
        (
            "depth 0 and another type",
            "bill",
            vec![of(Walk::Out, 0, 1)],
            [r#""b1""bill"0-,"c1""customer"1-"#, r#"None="2""#],
        ),
        (
            "a range that reaches nothing",
            "customer",
            vec![of(Walk::In, 2, 2)],
            ["", r#"None="0""#],
        ),
    ] {
        let rows = answer(own, pattern(start, None, steps.clone(), all()), COLS).await;
        let counted = answer(own, pattern(start, None, steps, grouped(&[])), &[]).await;
        if [rows.as_str(), counted.as_str()] != want {
            wrong.push(format!("{what}, with no `type`: {rows} | {counted}"));
        }
    }
    // A `where` over it is refused alike, for rows and a graph, and a `type`
    // it does not reach names every type it does.
    for (min, project, also) in [
        (1, all(), &["quote"][..]),
        (1, Graph, &["quote"]),
        (0, Graph, &["quote"]),
        (0, all(), &["customer", "quote"]),
    ] {
        let s = kept("x = 'qa'", of(Walk::In, min, 1));
        let got = answer(
            own,
            pattern("customer", None, vec![s], project.clone()),
            &[],
        )
        .await;
        let s = typed("ware", of(Walk::In, min, 1));
        let far = answer(
            own,
            pattern("customer", None, vec![s], project.clone()),
            &[],
        )
        .await;
        let all_types = [&["bill"][..], also].concat();
        let (reached, ty, also) = ("bill".into(), "ware".into(), Some(also.iter().collect()));
        let reaches = Said::from(Case::StepReaches { reached, ty, also });
        if got != several(1, "of", &all_types) || far != said(reaches.within(at(1, "of"))) {
            wrong.push(format!(
                "from depth {min} as {project:?}: a `where` {got}, a `type` not reached {far}"
            ));
        }
    }
    // The second type that declares the relation, named, reads the step's
    // `where` against its own records (lakelands#753); a path whose step
    // reads a `where` names its `type` as a graph's does.
    for (project, want) in [(all(), r#""qa""#), (Graph, "2 nodes")] {
        let s = typed("quote", kept("x = 'qa'", of(Walk::In, 1, 1)));
        let p = pattern("customer", Some("c1"), vec![s], project.clone());
        let got = answer(own, p, &["quote.x"]).await;
        if got != want {
            wrong.push(format!(
                "`type: quote` with a `where` as {project:?}: {got}"
            ));
        }
    }
    let s = kept("x = 'qa'", of(Walk::In, 0, 1));
    let mut path = pattern("customer", Some("c1"), vec![s], Graph);
    path.to = Some(("quote".into(), "q1".into()));
    let untyped = answer(own, path.clone(), &[]).await;
    path.steps[0].ty = Some("quote".into());
    let named = answer(own, path, &[]).await;
    if untyped != several(1, "of", &["bill", "quote"]) || named != "2 nodes" {
        wrong.push(format!(
            "a path to `q1` with a `where`: untyped {untyped}, typed `quote` {named}"
        ));
    }
    // A chain's graph is judged over its range as its rows are: it may name
    // a type its second hop reaches, and a `where` over the range names one.
    // Typed `bill`, the graph is b1 and the customer it came back through.
    for (project, ty, want) in [
        (all(), "quote", r#""qa""#),
        (Graph, "quote", "3 nodes"),
        (Graph, "bill", "2 nodes"),
    ] {
        let far = |s| pattern("bill", Some("b1"), vec![s], project.clone());
        let named = far(typed(ty, of(Walk::Both, 1, 2)));
        let got = answer(own, named, &["quote.x"]).await;
        let read = far(kept("x = 'qa'", of(Walk::Both, 1, 2)));
        let untyped = answer(own, read, &[]).await;
        if got != want || untyped != several(1, "of", &["bill", "customer", "quote"]) {
            wrong.push(format!(
                "two hops both ways as {project:?}: typed `{ty}` {got}, a `where` {untyped}"
            ));
        }
    }
    // With its `type` it binds that type alone, and the step after it is
    // read from that type.
    let by = || step("by", Walk::Out, 1, 1);
    let quotes = || typed("quote", of(Walk::In, 1, 1));
    let ware = &["ware.z", "quote.x"][..];
    let only = Projection::Rows {
        fields: vec!["ware.z".into()],
    };
    for (start, steps, project, by, read, want) in [
        (
            "customer",
            vec![quotes()],
            all(),
            "quote.x",
            &["quote.x"][..],
            r#""qa","qb""#,
        ),
        (
            "customer",
            vec![typed("bill", of(Walk::In, 1, 1))],
            all(),
            "bill.amount",
            &["bill.amount"][..],
            r#""5""#,
        ),
        (
            "bill",
            vec![typed("customer", of(Walk::Out, 0, 1))],
            all(),
            "customer.tier",
            &["customer.tier", "customer.amount"][..],
            r#""gold"-"#,
        ),
        // An inverse is its one relation, whatever else bears its name.
        (
            "customer",
            vec![step("bills", Walk::Out, 1, 1)],
            all(),
            "bill.amount",
            &["bill.amount"][..],
            r#""5""#,
        ),
        // A range that reaches nothing is judged by its first hop.
        (
            "customer",
            vec![typed("quote", of(Walk::In, 2, 2))],
            all(),
            "quote.x",
            &["quote.x"][..],
            "",
        ),
        (
            "customer",
            vec![quotes(), by()],
            all(),
            "ware.z",
            ware,
            r#""a""qb","m""qa""#,
        ),
        (
            "customer",
            vec![quotes(), by()],
            only,
            "ware.z",
            ware,
            r#""a"-,"m"-"#,
        ),
        (
            "customer",
            vec![quotes(), kept("z = 'm'", by())],
            all(),
            "ware.z",
            ware,
            r#""m""qa""#,
        ),
        (
            "customer",
            vec![quotes(), typed("ware", by())],
            all(),
            "ware.z",
            ware,
            r#""a""qb","m""qa""#,
        ),
        (
            "customer",
            vec![quotes(), by()],
            grouped(&["ware.z"]),
            "",
            ware,
            r#"Some(String("a"))="1"Some(String("m"))="1""#,
        ),
    ] {
        let mut p = pattern(start, None, steps.clone(), project.clone());
        if !by.is_empty() {
            p.order_by = vec![order(by, OrderBy::Value, Direction::Asc)];
        }
        let got = answer(own, p, read).await;
        if got != want {
            wrong.push(format!("{start} over {steps:?} as {project:?}: {got}"));
        }
    }
    let p = pattern(
        "customer",
        None,
        vec![quotes(), typed("seller", by())],
        all(),
    );
    let (reached, ty) = ("ware".into(), "seller".into());
    let reaches = Said::from(Case::StepReaches {
        reached,
        ty,
        also: None,
    });
    let got = answer(own, p, &[]).await;
    if got != said(reaches.within(at(2, "by"))) {
        wrong.push(format!("a second step typed `seller` after quotes: {got}"));
    }
    // The first hop's relation is probed before the step is judged: a
    // reader masked from the field that carries it is answered that.
    let held = field_access(store.as_ref(), "held", "bill", &[("cust", Access::Masked)]);
    let count = grouped(&[]);
    for (min, project) in [(1, all()), (0, all()), (1, count), (1, Graph), (0, Graph)] {
        let s = typed("bill", of(Walk::In, min, 1));
        let p = pattern("customer", None, vec![s], project.clone());
        let got = cat.query(&held, p).await;
        if !matches!(refused_on(&got), Some(("cust", Refusal::NotGranted))) {
            wrong.push(format!(
                "masked from `cust`, from depth {min} as {project:?}: {got:?}"
            ));
        }
    }
    // A chain's graph step from depth 0 reaches the type it stands on too:
    // its `where` names a `type`, and then keeps what holds of it.
    let wares = |s| pattern("customer", None, vec![quotes(), s], Graph);
    let m = |s| kept("z = 'm'", s);
    let untyped = answer(own, wares(m(step("by", Walk::Out, 0, 1))), &[]).await;
    let named = wares(m(typed("ware", step("by", Walk::Out, 0, 1))));
    let got = answer(own, named, &[]).await;
    if untyped != several(2, "by", &["quote", "ware"]) || got != "4 nodes" {
        wrong.push(format!(
            "a graph's second step from depth 0 with a `where`: {untyped}, typed `ware` {got}"
        ));
    }
    // A chain's graph binds no name, but its next step is judged from one
    // type: a step another follows names its `type`, whatever the next
    // step reads. A last step keeps every type it reaches, and the graph
    // holds them all.
    let then = |last: Step| pattern("customer", None, vec![of(Walk::In, 1, 1), last], Graph);
    for last in [by(), m(by()), typed("ware", by())] {
        let got = answer(own, then(last.clone()), &[]).await;
        if got != several(1, "of", &["bill", "quote"]) {
            wrong.push(format!(
                "a graph over `of` with no `type`, then {last:?}: {got}"
            ));
        }
    }
    let alone = pattern("customer", None, vec![of(Walk::In, 1, 1)], Graph);
    let back = vec![of(Walk::Out, 1, 1), of(Walk::In, 1, 1)];
    let back = pattern("bill", Some("b1"), back, Graph);
    let (alone, back) = (answer(own, alone, &[]).await, answer(own, back, &[]).await);
    if (alone.as_str(), back.as_str()) != ("5 nodes", "3 nodes") {
        wrong.push(format!(
            "a graph whose last step reaches bills and quotes: {alone}, after a step {back}"
        ));
    }
    // A measure reads every link of a relation and takes no step `type`,
    // whether the type is reached, declared, or neither.
    for (project, walk, measure) in [
        (Projection::Degree, Walk::In, "degree"),
        (Projection::Components, Walk::Both, "components"),
    ] {
        let measure = measure.to_owned();
        let none = said(Case::MeasureNoType { measure }.into());
        for ty in ["quote", "ware", "ghost"] {
            let typed = vec![typed(ty, of(walk, 1, 1))];
            let got = answer(own, pattern("customer", None, typed, project.clone()), &[]).await;
            if got != none {
                wrong.push(format!("{project:?} with a step typed `{ty}`: {got}"));
            }
        }
    }
    // A relation a neighbourhood meets around another type than its step
    // stands on is probed when it is walked.
    let kept_from = field_access(store.as_ref(), "kept", "bill", &[("from", Access::Masked)]);
    let around = |key| {
        let steps = vec![of(Walk::In, 0, 1), step("by", Walk::Out, 0, 2)];
        pattern("customer", Some(key), steps, Graph)
    };
    let got = cat.query(&kept_from, around("c1")).await;
    // From c2 the walk meets no bill, and the same reader is answered.
    let unmet = answer(&kept_from, around("c2"), &[]).await;
    if !matches!(refused_on(&got), Some(("from", Refusal::NotGranted))) || unmet != "3 nodes" {
        wrong.push(format!(
            "a neighbourhood, masked from `from`: {got:?}, and from c2 {unmet}"
        ));
    }
    // A path asked as rows is told so, not that its step from depth 0
    // reaches its own type too.
    for project in [all(), grouped(&[])] {
        let mut p = pattern("bill", Some("b1"), vec![of(Walk::Out, 0, 1)], project);
        p.to = Some(("customer".into(), "c1".into()));
        let got = answer(own, p, &[]).await;
        if got != said(Case::PatternToNotGraph.into()) {
            wrong.push(format!("a path asked as rows or an aggregate: {got}"));
        }
    }
    // From depth 0 a step is judged where it stands, as a chain's is.
    let p = pattern("customer", None, vec![step("by", Walk::Out, 0, 1)], all());
    let (ty, relation, walk) = ("customer".into(), "by".into(), Walk::Out);
    let none = said(Case::NoRelationToWalk { ty, relation, walk }.into());
    let got = answer(own, p, &[]).await;
    if got != none {
        wrong.push(format!(
            "rows from depth 0 over a relation of another type: {got}"
        ));
    }
    // A reader not shown `quote` is answered as on a lake without it: the
    // step reaches bills alone, and `quote` is a type as absent as `ghost`.
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    grants.types.insert("quote".into(), none);
    let clerk = principal(store.as_ref(), PrincipalKind::Agent, "clerk", grants);
    let mut bills = pattern("customer", None, vec![of(Walk::In, 1, 1)], all());
    let got = answer(&clerk, bills.clone(), &["bill.amount"]).await;
    if got != r#""5""# {
        wrong.push(format!("a reader not shown `quote`, over `of`: {got}"));
    }
    // Counted by `_type` over every type the step reaches (lakelands#990):
    // a type the reader is not shown is neither counted nor named.
    let mut hop = of(Walk::In, 1, 1);
    hop.named = Some("hop".into());
    let kinds = pattern("customer", None, vec![hop], grouped(&["hop._type"]));
    for (who, want) in [
        (own, r#"Some(String("bill"))="1"Some(String("quote"))="2""#),
        (&clerk, r#"Some(String("bill"))="1""#),
    ] {
        let got = answer(who, kinds.clone(), &[]).await;
        if got != want {
            wrong.push(format!("counted by `hop._type` for `{}`: {got}", who.id));
        }
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    let mut refused = Vec::new();
    for project in [all(), Projection::Degree] {
        bills.project = project;
        for ty in ["quote", "ghost"] {
            bills.steps[0].ty = Some(ty.into());
            let fut = cat.query(&clerk, bills.clone());
            refused.push(fut.await.must_fail(&format!("a step typed `{ty}`"))?);
        }
    }
    alike_renamed(
        "a measure's step typed `quote` to a reader not shown it",
        &refused[2],
        &refused[3],
        ("ghost", "quote"),
    )?;
    alike_renamed(
        "a step typed `quote` to a reader not shown it",
        &refused[0],
        &refused[1],
        ("ghost", "quote"),
    )
}

check!(
    PATTERN_SAYS = a_pattern_is_refused_for_what_it_says_and_a_step_reads_what_it_keeps,
    "018, 024 and 044, extended (lakelands#829): an `order_by` on a graph or a path is refused before a step is judged, and one on an aggregate, which is answered without it (lakelands#901); a depth no walk takes is refused for rows and a path whose start no record has; a neighbourhood over a relation no shown type declares is refused as a chain's step is, with a `type` too, an inverse that is not around the type it is read from alike, and a relation whose carrying fields its reader may not know of as an absent one; an inverse is one relation onto its target: a second is refused by `declare`, `propose` and `apply` at the type, naming no relation, to a proposer without `schema` who is not shown the other's type word for word as to the owner, whose proposal under another inverse is taken, and to an author with `schema` who is not shown it at `declare` and by a migration that adds the relation, and taken onto another target, where the next is refused alone in a set with the first as it stands, a step by it reaches its owner's records alone in rows, a graph and a degree, in a component and over two hops both ways, on to a second bill, and a `type` it names that is another is one it does not reach; a step with a `type` probes and reads only the relations its walk keeps a record over: a reader masked from `quote.cust` is answered the bills in rows, a graph and a path, to the byte what it is answered once the quotes are gone, one masked from `bill.cust` is refused, and told what the step reaches where its `type` is one it does not reach, naming only the types it is shown, both are without the `type`, a relation a chain's step walks through to its `type` is probed, and each step reads by its own `type` where another walks the same relation the same way"
);

async fn a_pattern_is_refused_for_what_it_says_and_a_step_reads_what_it_keeps(
    store: Store,
) -> Held {
    use crate::conformance::fixture::{
        alike_renamed, field_access, invalid_said, migration, order, pattern, principal, refused_on,
    };
    use lakelands_api::error::{Case, Place, Said};
    use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
    use lakelands_api::query::Projection::{self, Graph};
    use lakelands_api::read::{Direction, OrderBy};
    use lakelands_api::write::{Declaration, ProposedWrite};
    use lakelands_api::{Access, Principal};
    let l = trade(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let all = || Projection::Rows { fields: Vec::new() };
    let of = |walk, min, max| step("of", walk, min, max);
    let typed = |ty: &str, s: Step| Step {
        ty: Some(ty.into()),
        ..s
    };
    let to = |mut p: Pattern, key: &str| {
        p.to = Some(("bill".into(), key.into()));
        p
    };
    let said = |who: &Principal, p: Pattern| {
        let (cat, who) = (cat.clone(), who.clone());
        async move { invalid_said(&cat.query(&who, p).await).cloned() }
    };
    let mut wrong = Vec::new();
    // An order a graph and a path would pass over, before a step is judged.
    let ghost = || step("ghost", Walk::Out, 0, 1);
    let path = to(
        pattern("customer", Some("c1"), vec![of(Walk::In, 0, 1)], Graph),
        "b1",
    );
    // And an aggregate's groups, which come by their key (lakelands#901).
    let counted = json!({"start": {"type": "customer"}, "project": {"as": "aggregate",
                         "measures": [{"name": "c", "func": "count"}]}});
    let counted: Pattern = serde_json::from_value(counted).expect("a pattern reads");
    if cat.query(own, counted.clone()).await.is_err() {
        wrong.push("an aggregate without an `order_by` is not answered".to_owned());
    }
    let mut ghostly = counted.clone();
    ghostly.steps = vec![ghost()];
    for mut p in [
        pattern("customer", None, vec![of(Walk::In, 1, 1)], Graph),
        pattern("customer", None, vec![ghost()], Graph),
        path.clone(),
        counted,
        ghostly,
    ] {
        p.order_by = vec![order("customer.tier", OrderBy::Value, Direction::Asc)];
        let got = said(own, p.clone()).await;
        if got != Some(Case::PatternOrderNotRows.into()) {
            wrong.push(format!(
                "an `order_by` on {:?} to {:?}: {got:?}",
                p.steps, p.to
            ));
        }
    }
    // A depth no walk takes, from a start no record has.
    let nobody = |s, project| pattern("customer", Some("nobody"), vec![typed("bill", s)], project);
    let (max, asked) = (limits().max_depth, limits().max_depth + 1);
    for (p, case) in [
        (
            nobody(of(Walk::In, 2, 1), all()),
            Case::DepthEmpty { min: 2, max: 1 },
        ),
        (
            nobody(of(Walk::In, 1, asked), all()),
            Case::StepTooDeep { max, asked },
        ),
        (
            to(nobody(of(Walk::In, 0, asked), Graph), "b1"),
            Case::StepTooDeep { max, asked },
        ),
    ] {
        let got = said(own, p).await;
        if got != Some(case.clone().into()) {
            wrong.push(format!("from no record, {case:?}: {got:?}"));
        }
    }
    // A neighbourhood over a relation no shown type declares, and over an
    // inverse that is not around the type it is read from.
    let around = |ty: &str, s| pattern(ty, None, vec![s], Graph);
    for (ty, s, relation) in [
        ("customer", ghost(), "ghost"),
        ("customer", typed("bill", ghost()), "ghost"),
        ("seller", step("bills", Walk::Out, 0, 1), "bills"),
    ] {
        let got = said(own, around(ty, s)).await;
        let (ty, walk, over) = (ty.into(), Walk::Out, relation.into());
        if got
            != Some(
                Case::NoRelationToWalk {
                    ty,
                    relation: over,
                    walk,
                }
                .into(),
            )
        {
            wrong.push(format!("a neighbourhood over `{relation}`: {got:?}"));
        }
    }
    // An inverse is one relation: `bills` is `bill.of`, though a quote is
    // `of` a customer too.
    let bills = |ty: Option<&str>, project| {
        let s = step("bills", Walk::Out, 1, 1);
        let s = ty.map_or(s.clone(), |ty| typed(ty, s));
        pattern("customer", Some("c1"), vec![s], project)
    };
    let mut got = Vec::new();
    for (project, read) in [
        (all(), "bill.amount"),
        (Graph, ""),
        (Projection::Degree, "degree"),
        (all(), "quote.x"),
    ] {
        let ty = (read == "quote.x").then_some("quote");
        got.push(answered(cat.as_ref(), own, bills(ty, project), &[read]).await);
    }
    let (reached, ty) = ("bill".into(), "quote".into());
    let reaches = Said::from(Case::StepReaches {
        reached,
        ty,
        also: None,
    })
    .within(Place::Step {
        step: 1,
        relation: "bills".into(),
    });
    let want = [
        r#""5""#.to_owned(),
        "2 nodes".into(),
        "1".into(),
        format!("{:?}", Ok::<_, ()>(&reaches)),
    ];
    if got != want {
        wrong.push(format!("a step by the inverse `bills` from c1: {got:?}"));
    }
    // c1's component over it is c1 and b1; and over two hops both ways
    // from b1 it comes to c1 and on to a second bill, to no quote.
    let both = |max| vec![step("bills", Walk::Both, 1, max)];
    let part = pattern("customer", Some("c1"), both(1), Projection::Components);
    let part = answered(cat.as_ref(), own, part, &["size"]).await;
    l.upsert("bill", "b2", json!({"cust": "c1"})).await?;
    let hops = pattern("bill", Some("b1"), both(2), Graph);
    let got = [answered(cat.as_ref(), own, hops, &[]).await, part];
    l.retire("bill", "b2").await?;
    if got != ["3 nodes", "2,2"] {
        wrong.push(format!(
            "the inverse `bills` over two hops from b1, and c1's component: {got:?}"
        ));
    }
    let under = |inverse: &str, name: &str, target: &str| {
        let mut ty = type_decl(name, vec![field("m", StorageKind::Text)]);
        ty.relations = vec![RelationDecl {
            inverse: Some(inverse.into()),
            ..RelationDecl::to("for", target)
        }];
        Declaration::Type(ty)
    };
    let memo = |name: &str, target: &str| under("bills", name, target);
    // Said at the type alone: the other relation may be one its author is
    // not shown, and no place names either.
    let twice = |name: &str, target: &str| {
        let (inverse, target, name) = ("bills".into(), target.into(), name.into());
        Some(Said::from(Case::InverseTwice { inverse, target }).within(Place::Type { name }))
    };
    let declarations = vec![memo("memo", "customer")];
    let proposed = ProposedWrite::Declare { declarations };
    let doors = [
        cat.declare(own, memo("memo", "customer")).await,
        cat.propose(own, proposed, "a memo".into()).await,
        cat.apply(own, vec![memo("memo", "customer")]).await,
    ];
    let mut refused: Vec<_> = doors.iter().map(|o| invalid_said(o).cloned()).collect();
    let other = cat.declare(own, memo("memo", "seller")).await;
    // A relation a type holds already is not judged again: in a set with
    // the memo as it stands, the second alone is refused.
    let set = vec![memo("memo", "seller"), memo("note", "seller")];
    let second = invalid_said(&cat.apply(own, set).await).cloned();
    refused.dedup();
    if refused != [twice("memo", "customer")] || other.is_err() || second != twice("note", "seller")
    {
        wrong.push(format!(
            "a second relation with the inverse `bills` onto `customer`, by `declare`, `propose` \
             and `apply`: {refused:?}; onto `seller`: {other:?}, and a second there: {second:?}"
        ));
    }
    // Told on purpose (044, extended): a proposer without `schema` who is
    // not shown `bill` is refused the pair it sent, word for word as the
    // owner is, and taken under another inverse; and so is an author with
    // `schema` who is not shown it, at `declare` and by a migration that
    // adds the relation.
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    grants.types.insert("bill".into(), hidden.clone());
    let outsider = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "outsider",
        grants.clone(),
    );
    grants.schema = true;
    let architect = principal(store.as_ref(), PrincipalKind::Human, "architect", grants);
    let mut told = Vec::new();
    for who in [&outsider, own] {
        let declarations = vec![memo("slip", "customer")];
        let slip = ProposedWrite::Declare { declarations };
        told.push(invalid_said(&cat.propose(who, slip, "a slip".into()).await).cloned());
    }
    told.push(invalid_said(&cat.declare(&architect, memo("slip", "customer")).await).cloned());
    let declarations = vec![under("zzz", "slip", "customer")];
    let free = ProposedWrite::Declare { declarations };
    let free = cat.propose(&outsider, free, "a slip".into()).await;
    let mut added = type_decl("memo", vec![field("m", StorageKind::Text)]);
    added.relations = [("for", "seller"), ("also", "customer")]
        .map(|(name, target)| RelationDecl {
            inverse: Some("bills".into()),
            ..RelationDecl::to(name, target)
        })
        .into();
    let moved = invalid_said(&cat.migrate(&architect, migration(added)).await).cloned();
    let pair = told.iter().all(|t| *t == twice("slip", "customer")) && free.is_ok();
    if !pair || moved != twice("memo", "customer") {
        wrong.push(format!(
            "a second inverse proposed by one not shown `bill` and by the owner, and declared \
             by an author not shown it: {told:?}; under `zzz`: {free:?}; added by that author's \
             migration: {moved:?}"
        ));
    }
    // A step with a `type` probes what its walk keeps a record over.
    let masked = |id, ty| field_access(store.as_ref(), id, ty, &[("cust", Access::Masked)]);
    let (no_quote, no_bill) = (masked("no_quote", "quote"), masked("no_bill", "bill"));
    // A `type` the step does not reach walks nothing, so a reader masked
    // from `bill.cust` is told what the step reaches, of the types it is
    // shown: one not shown `quote` beside is named `bill` alone.
    let mut alone = masked("alone", "bill");
    alone.grants.types.insert("quote".into(), hidden);
    for (who, also) in [(&no_bill, &["quote"][..]), (&alone, &[])] {
        let wares = vec![typed("ware", of(Walk::In, 1, 1))];
        let got = said(who, pattern("customer", Some("c1"), wares, all())).await;
        let also = (!also.is_empty()).then(|| also.iter().collect());
        let (reached, ty) = ("bill".into(), "ware".into());
        let want = Said::from(Case::StepReaches { reached, ty, also }).within(Place::Step {
            step: 1,
            relation: "of".into(),
        });
        if got != Some(want) {
            wrong.push(format!("`of` in, typed `ware`, to `{}`: {got:?}", who.id));
        }
    }
    let read = |ty: Option<&str>| {
        let s = |min| ty.map_or(of(Walk::In, min, 1), |ty| typed(ty, of(Walk::In, min, 1)));
        let rows = pattern("customer", Some("c1"), vec![s(1)], Graph);
        let mut all = rows.clone();
        all.project = Projection::Rows { fields: Vec::new() };
        [
            all,
            rows,
            to(pattern("customer", Some("c1"), vec![s(0)], Graph), "b1"),
        ]
    };
    let not_granted = |got: &lakelands_api::Result<_>| {
        matches!(refused_on(got), Some(("cust", Refusal::NotGranted)))
    };
    let mut kept = Vec::new();
    for (i, p) in read(Some("bill")).into_iter().enumerate() {
        let (bills, held) = (
            cat.query(&no_quote, p.clone()).await,
            cat.query(&no_bill, p).await,
        );
        let untyped = [&no_quote, &no_bill].map(|who| cat.query(who, read(None)[i].clone()));
        let [a, b] = untyped;
        if !not_granted(&held) || !not_granted(&a.await) || !not_granted(&b.await) {
            wrong.push(format!(
                "read {i}, masked from `bill.cust` or with no `type`: {held:?}"
            ));
        }
        // Collected with the rest, so one run names every row that fails.
        let wire = bills.as_ref().map(|r| serde_json::to_string(&r.data));
        match wire {
            Ok(Ok(wire)) => kept.push(wire),
            _ => wrong.push(format!(
                "read {i}, typed `bill`, masked from `quote.cust`: {bills:?}"
            )),
        }
    }
    // A chain's step walks its range before it keeps: over two hops both
    // ways from b1 it comes to a quote through `bill.of`, and back to a
    // bill through no quote.
    let far = |ty| {
        pattern(
            "bill",
            Some("b1"),
            vec![typed(ty, of(Walk::Both, 1, 2))],
            Graph,
        )
    };
    let through = cat.query(&no_bill, far("quote")).await;
    let back = answered(cat.as_ref(), &no_quote, far("bill"), &[]).await;
    if !not_granted(&through)
        || back != "2 nodes"
        || !kept.first().is_some_and(|k| k.contains("\"5\""))
    {
        wrong.push(format!(
            "two hops both ways: typed `quote`, masked from `bill.cust`, {through:?}; typed \
             `bill`, masked from `quote.cust`, {back}; the bills of c1 {kept:?}"
        ));
    }
    // Each step reads by its own `type`, though another walks the same
    // relation the same way: the last step walks through quotes, and the
    // first, from the customer, reads none.
    let mut line = type_decl(
        "line",
        vec![reference("q", "quote"), reference("b", "bill")],
    );
    line.relations = [("of", "quote", "q"), ("about", "bill", "b")]
        .map(|(name, target, via)| RelationDecl {
            via: Some(via.into()),
            ..RelationDecl::to(name, target)
        })
        .into();
    l.declare(line).await?;
    l.upsert("line", "l1", json!({"q": "q1", "b": "b1"}))
        .await?;
    let about = Step {
        named: Some("l".into()),
        ..step("about", Walk::In, 1, 1)
    };
    let steps = vec![
        typed("bill", of(Walk::In, 1, 1)),
        about,
        of(Walk::Out, 1, 1),
        typed("line", of(Walk::In, 1, 2)),
    ];
    let lines = pattern("customer", Some("c1"), steps, all());
    let got = answered(cat.as_ref(), &no_quote, lines, &["line.key"]).await;
    if got != r#""l1""# {
        wrong.push(format!(
            "bills of c1, then the lines of their lines' quotes, masked from `quote.cust`: {got}"
        ));
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    // A relation whose carrying fields its reader may not know of is one no
    // type declares.
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    for (ty, carries) in [("bill", "from"), ("quote", "item")] {
        let fields = [(carries.into(), Access::None)].into();
        let held = TypeGrants {
            fields,
            ..Default::default()
        };
        grants.types.insert(ty.into(), held);
    }
    let blind = principal(store.as_ref(), PrincipalKind::Agent, "blind", grants);
    let by = |name: &str| cat.query(&blind, around("customer", step(name, Walk::Out, 0, 1)));
    alike_renamed(
        "a neighbourhood over `by` to a reader who may not know of the fields that carry it",
        &by("by").await.must_fail("a neighbourhood over `by`")?,
        &by("ghost")
            .await
            .must_fail("a neighbourhood over `ghost`")?,
        ("ghost", "by"),
    )?;
    // The same reads once the quotes are gone.
    for key in ["q1", "q2"] {
        l.retire("quote", key).await?;
    }
    for (p, with) in read(Some("bill")).into_iter().zip(kept) {
        let reply = cat.query(&no_quote, p).await.must("a step typed `bill`")?;
        let without =
            serde_json::to_string(&reply.data).map_err(|e| Failure::new(e.to_string()))?;
        ensure!(
            with == without,
            "a reader masked from `quote.cust` is answered the bills as on a lake without the \
             quotes: it answered {with}, and without them {without}"
        );
    }
    Ok(())
}

async fn a_last_step_over_several_types_reads_the_lakes_columns_alone(store: Store) -> Held {
    use crate::conformance::fixture::principal;
    use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
    use lakelands_api::{Access, Principal};
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = |name: &str, own: &[(&str, StorageKind)], relations: Vec<RelationDecl>| {
        let mut t = type_decl(name, own.iter().map(|(f, k)| field(f, *k)).collect());
        t.relations = relations;
        t
    };
    let via = |name: &str, f: &str, target: &str| RelationDecl {
        via: Some(f.into()),
        ..RelationDecl::to(name, target)
    };
    // A hub reached over `at` by `pa`, which declares a field `key`, then
    // `pb`; over `ta` by `pd`, then `pc`, which declares one.
    l.declare(ty("hub", &[("h", StorageKind::Text)], vec![]))
        .await?;
    for (name, keyed, rel) in [
        ("pa", true, "at"),
        ("pb", false, "at"),
        ("pd", false, "ta"),
        ("pc", true, "ta"),
    ] {
        let mut t = ty(
            name,
            &[("n", StorageKind::Text)],
            vec![via(rel, "h", "hub")],
        );
        t.fields.push(reference("h", "hub"));
        if keyed {
            t.fields.push(field("key", StorageKind::Text));
        }
        l.declare(t).await?;
    }
    l.upsert("hub", "h1", json!({"h": "h"})).await?;
    for (t, k, keyed) in [
        ("pa", "a1", true),
        ("pb", "b1", false),
        ("pc", "c1", true),
        ("pd", "d1", false),
    ] {
        let mut f = json!({"n": k, "h": "h1"});
        if keyed {
            f["key"] = json!("zz");
        }
        l.upsert(t, k, f).await?;
    }
    let mut masked = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    for t in ["pa", "pc"] {
        let key = TypeGrants {
            fields: [("key".into(), Access::Masked)].into(),
            ..Default::default()
        };
        masked.types.insert(t.into(), key);
    }
    let masked = principal(store.as_ref(), PrincipalKind::Agent, "masked", masked);
    let ask = |who: &Principal, p: serde_json::Value, read: &'static [&'static str]| {
        let (cat, who) = (cat.clone(), who.clone());
        async move {
            match serde_json::from_value::<Pattern>(p) {
                Ok(p) => answered(cat.as_ref(), &who, p, read).await,
                Err(e) => e.to_string(),
            }
        }
    };
    let mut wrong = Vec::new();
    for rel in ["at", "ta"] {
        let step = json!([{"relation": rel, "walk": "in"}]);
        let start = json!({"type": "hub", "key": "h1"});
        let rows = json!({"start": start, "steps": step, "project": {"as": "rows", "fields": [format!("{rel}.key")]}, "order_by": [{"field": format!("{rel}.key")}]});
        let grouped = json!({"start": start, "steps": step, "project": {"as": "aggregate", "measures": [{"name": "n", "func": "count"}], "group_by": [format!("{rel}.key")]}});
        let read: &'static [&'static str] = if rel == "at" {
            &["at.key"]
        } else {
            &["ta.key"]
        };
        for who in [own, &masked] {
            let said = [
                ask(who, rows.clone(), read).await,
                ask(who, grouped.clone(), &[]).await,
            ];
            let want = if rel == "at" {
                [
                    r#""a1","b1""#,
                    r#"Some(String("a1"))="1"Some(String("b1"))="1""#,
                ]
            } else {
                [
                    r#""c1","d1""#,
                    r#"Some(String("c1"))="1"Some(String("d1"))="1""#,
                ]
            };
            if said != want {
                wrong.push(format!("`{rel}.key` for `{}`: {said:?}", who.id));
            }
        }
    }
    // Bills and quotes of customers, over a reference and over link
    // records; quotes hold a valid time.
    l.declare(ty("customer", &[("tier", StorageKind::Text)], vec![]))
        .await?;
    for (name, own_field) in [("bill", "amount"), ("quote", "x")] {
        let mut t = ty(
            name,
            &[(own_field, StorageKind::Text)],
            vec![
                RelationDecl {
                    inverse: (name == "bill").then(|| "bills".into()),
                    ..via("of", "cust", "customer")
                },
                RelationDecl::to("noted", "customer"),
            ],
        );
        t.fields.push(reference("cust", "customer"));
        if name == "quote" {
            t.fields.push(field("since", StorageKind::DateTime));
            t.valid_from = Some("since".into());
        }
        l.declare(t).await?;
    }
    for (t, k, f) in [
        ("customer", "c1", json!({"tier": "gold"})),
        ("customer", "c2", json!({"tier": "tin"})),
        ("bill", "b1", json!({"amount": "5", "cust": "c1"})),
        (
            "quote",
            "q1",
            json!({"x": "qa", "cust": "c1", "since": "2026-01-01T00:00:00Z"}),
        ),
        (
            "quote",
            "q2",
            json!({"x": "qb", "cust": "c2", "since": "2026-01-01T00:00:00Z"}),
        ),
    ] {
        l.upsert(t, k, f).await?;
    }
    for (t, from) in [("bill", "b1"), ("quote", "q1")] {
        let (t, from, with, to) = (
            TypeName::from(t),
            RecordKey::from(from),
            "noted".into(),
            "c1".into(),
        );
        let linked = cat.link(
            own,
            &t,
            &from,
            &with,
            &to,
            Default::default(),
            Default::default(),
        );
        linked.await.must("noting a record")?;
    }
    let of = |more: serde_json::Value| {
        let mut p =
            json!({"start": {"type": "customer"}, "steps": [{"relation": "of", "walk": "in"}]});
        p.as_object_mut()
            .unwrap()
            .extend(more.as_object().cloned().unwrap_or_default());
        p
    };
    let count = |by: serde_json::Value| json!({"project": {"as": "aggregate", "measures": [{"name": "n", "func": "count"}], "group_by": by}});
    let only = Grants {
        default: Access::Read,
        types: [(
            "quote".into(),
            TypeGrants {
                only: [("x".into(), json!("qa"))].into(),
                ..Default::default()
            },
        )]
        .into(),
        ..Grants::default()
    };
    let kept = principal(store.as_ref(), PrincipalKind::Agent, "kept", only);
    let mut since_masked = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    since_masked.types.insert(
        "quote".into(),
        TypeGrants {
            fields: [("since".into(), Access::Masked)].into(),
            ..Default::default()
        },
    );
    let since_masked = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "untimed",
        since_masked,
    );
    let page = of(
        json!({"order_by": [{"field": "of._type", "dir": "desc"}, {"field": "of.key"}], "limit": 2}),
    );
    let first =
        serde_json::from_value::<Pattern>(page.clone()).map_err(|e| Failure::new(e.to_string()))?;
    let next = match cat.query(own, first).await.must("the first page")?.data {
        QueryResult::Rows { next_cursor, .. } => next_cursor,
        _ => None,
    };
    let mut second = page.clone();
    second["cursor"] = json!(next);
    let at_time = |who: &Principal| {
        let p = of(json!({"valid_at": "2026-06-01T00:00:00Z"}));
        ask(who, p, &["of.key"])
    };
    let checks: Vec<(&str, String, &str)> = vec![
        ("ordered by `_type`", ask(own, of(json!({"order_by": [{"field": "of._type", "dir": "desc"}, {"field": "of.key"}], "project": {"as": "rows", "fields": ["of.key", "of._type"]}})), &["of.key", "of._type"]).await, r#""q1""quote","q2""quote","b1""bill""#),
        ("counted by `of._type = 'bill'`", ask(own, of(count(json!([{"name": "b", "expr": "of._type = 'bill'"}]))), &[]).await, r#"Some(Bool(false))="2"Some(Bool(true))="1""#),
        ("the first page by `_type` and key", ask(own, page, &["of.key"]).await, r#""q1","q2""#),
        ("the page after it", ask(own, second, &["of.key"]).await, r#""b1""#),
        ("over link records", ask(own, json!({"start": {"type": "customer", "key": "c1"}, "steps": [{"relation": "noted", "walk": "in"}]}), &["noted.key", "noted._type"]).await, r#""b1""bill","q1""quote""#),
        ("counted over link records", ask(own, json!({"start": {"type": "customer", "key": "c1"}, "steps": [{"relation": "noted", "walk": "in"}], "project": {"as": "aggregate", "measures": [{"name": "n", "func": "count"}]}}), &[]).await, r#"None="2""#),
        ("counted by a reader kept by `only`", ask(&kept, of(count(json!(["of._type"]))), &[]).await, r#"Some(String("bill"))="1"Some(String("quote"))="1""#),
        ("at a valid time", at_time(own).await, r#""b1","q1","q2""#),
        ("by an inverse's name from depth 0", ask(own, json!({"start": {"type": "customer", "key": "c1"}, "steps": [{"relation": "bills", "walk": "out", "depth": {"min": 0, "max": 1}}]}), &["bills.key", "bills._type"]).await, r#""c1""customer","b1""bill""#),
    ];
    for (what, got, want) in checks {
        if got != want {
            wrong.push(format!("{what}: {got}"));
        }
    }
    let untimed = at_time(&since_masked).await;
    if !untimed.contains("NotGranted") && !untimed.contains("not_granted") {
        wrong.push(format!(
            "at a valid time, masked on `quote.since`: {untimed}"
        ));
    }
    // `as` gives a reader shown both types and one shown one the same
    // columns; without it the second binds under `bill`.
    let mut one = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    one.types.insert(
        "quote".into(),
        TypeGrants {
            default: Some(Access::None),
            ..Default::default()
        },
    );
    let one = principal(store.as_ref(), PrincipalKind::Agent, "one", one);
    let named = json!({"start": {"type": "customer", "key": "c1"}, "steps": [{"relation": "of", "walk": "in", "as": "hop"}]});
    let named: Pattern = serde_json::from_value(named).map_err(|e| Failure::new(e.to_string()))?;
    let mut columns = Vec::new();
    for who in [own, &one] {
        let QueryResult::Rows { rows, .. } = cat
            .query(who, named.clone())
            .await
            .must("named `hop`")?
            .data
        else {
            return Err(Failure::new("rows"));
        };
        let keys: std::collections::BTreeSet<String> = rows
            .iter()
            .flat_map(|r| {
                r.keys()
                    .filter(|k| {
                        k.starts_with("hop.")
                            && ["hop.key", "hop._type", "hop._depth"].contains(&k.as_str())
                    })
                    .cloned()
            })
            .collect();
        columns.push(keys);
    }
    if columns[0] != columns[1] || columns[0].len() != 3 {
        wrong.push(format!("`as` answers both readers {columns:?}"));
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}
