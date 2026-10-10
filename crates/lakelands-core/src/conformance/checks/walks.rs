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
    "018, extended (lakelands#680): a step whose records rows or an aggregate bind reaches one type over its whole range or names its `type`: over a relation two types declare it is refused at the first step and at a later one, in a chain and from depth 0, as is one whose second hop reaches other types and one from depth 0 that reaches another type than it stands on, each at its step under the relation as its caller wrote it, an inverse too; a `where` over such a step is refused alike for rows and a graph, and a chain's graph is judged over its range as its rows are, a second hop's type named and a step from depth 0 with a `where` naming one, and its step that another follows names its `type` while a last one keeps every type it reaches; a `type` the step does not reach is refused naming every type it does, at depth 0 as in a chain, and degree and components take no step `type` at all; with its `type` it binds that type alone, a range that reaches nothing is judged by its first hop, and the step after it is read from that type, by an order, `fields`, a group key, a `where` and a `type` of its own; a reader masked from the field that carries the first hop is answered that before the step is judged, in rows, an aggregate and a graph, and a relation a neighbourhood meets around another type when it is walked and not before; a path asked as rows or an aggregate is refused as that, and rows from depth 0 over a relation their type does not declare as a chain's are; a reader not shown one of the two types is answered as on a lake without it, by a measure too"
);

check!(
    ROW_NAMES = a_row_binds_each_record_under_its_name,
    "018, extended (lakelands#680): a row binds each record a pattern reaches under the name `as` gives, or its type's: a chain that reaches one type three times answers each record under its own name, a step that would bind a name again is refused at its step and `as` on a graph, a path, degree and components as naming nothing, an aggregate groups by a named path, a field its reader may not know of answers under a name as on its type, and a name that is no identifier is refused"
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

/// Bills and quotes, each `of` a customer, whose `papers` both are; a bill
/// is `by` a seller and a quote `by` a ware.
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
                inverse: Some("papers".into()),
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

async fn a_step_whose_records_a_row_binds_reaches_one_type(store: Store) -> Held {
    use crate::conformance::fixture::{
        alike_renamed, field_access, invalid_said, order, pattern, principal, refused_on,
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
    // What `who` is answered: each row's values at `read`, the groups, a
    // graph's size, or the refusal.
    let answer = |who: &Principal, p: Pattern, read: &'static [&'static str]| {
        let (cat, who) = (cat.clone(), who.clone());
        async move {
            let cell = |v: Option<&Value>| v.map_or("-".into(), |v| v.to_string());
            let said = cat.query(&who, p).await;
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
    };
    let said = |s: Said| format!("{:?}", Ok::<_, ()>(&s));
    let several =
        |step, relation: &str| said(Said::from(Case::StepSeveralTypes).within(at(step, relation)));
    let mut wrong = Vec::new();
    // Refused where it names no `type`: a row holds one type under a name.
    for (what, start, steps, step_no, relation) in [
        (
            "a chain's first step",
            "customer",
            vec![of(Walk::In, 1, 1)],
            1,
            "of",
        ),
        (
            "the same from depth 0",
            "customer",
            vec![of(Walk::In, 0, 1)],
            1,
            "of",
        ),
        (
            "a chain's second step",
            "bill",
            vec![of(Walk::Out, 1, 1), of(Walk::In, 1, 1)],
            2,
            "of",
        ),
        (
            "a second step from depth 0",
            "bill",
            vec![typed("customer", of(Walk::Out, 0, 1)), of(Walk::In, 0, 1)],
            2,
            "of",
        ),
        (
            "a second hop that reaches other types",
            "bill",
            vec![of(Walk::Both, 1, 2)],
            1,
            "of",
        ),
        (
            "depth 0 and another type",
            "bill",
            vec![of(Walk::Out, 0, 1)],
            1,
            "of",
        ),
        (
            "a range that reaches nothing, by its first hop",
            "customer",
            vec![of(Walk::In, 2, 2)],
            1,
            "of",
        ),
        (
            "an inverse name",
            "customer",
            vec![step("papers", Walk::Out, 1, 1)],
            1,
            "papers",
        ),
    ] {
        for project in [all(), grouped(&[])] {
            let got = answer(own, pattern(start, None, steps.clone(), project), &[]).await;
            if got != several(step_no, relation) {
                wrong.push(format!("{what}, with no `type`: {got}"));
            }
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
        let (reached, ty, also) = ("bill".into(), "ware".into(), Some(also.iter().collect()));
        let reaches = Said::from(Case::StepReaches { reached, ty, also });
        if got != several(1, "of") || far != said(reaches.within(at(1, "of"))) {
            wrong.push(format!(
                "from depth {min} as {project:?}: a `where` {got}, a `type` not reached {far}"
            ));
        }
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
        if got != want || untyped != several(1, "of") {
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
        let s = typed("ware", of(Walk::In, min, 1));
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
    if untyped != several(2, "by") || got != "4 nodes" {
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
        if got != several(1, "of") {
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
