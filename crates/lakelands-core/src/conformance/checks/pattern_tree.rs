//! A pattern is a tree of steps: a step walks from a name bound before
//! it, joins one, binds null where it is optional, and holds only where
//! it has no match where it is absent
//! (`docs/decisions/080-a-pattern-may-branch-and-join.md`, lakelands#1408).

use lakelands_api::query::Depth;
use lakelands_api::query::Walk::{Both, Out};
use lakelands_api::write::WriteOpts;

use crate::conformance::fixture::{doors, order, owner, reference};
use crate::conformance::prelude::*;

check!(
    TREE_BRANCH = a_step_walks_from_a_name_bound_before_it_and_a_join_closes_a_cycle,
    "080: a step's `from` names the start or a name a step before it binds, and a start's rows are every combination of its branches' matches, in the starts' order and each name's records by depth, type and key whatever order its links were written in; a step's `where` reads an earlier name, a sibling branch's included, as `<name>.<field>`; a join holds only where its step reaches the record the joined name bound, closing a cycle; a pattern that names no `from` is the chain it was"
);

check!(
    TREE_OPTIONAL = an_optional_branch_matches_whole_or_binds_null_whole,
    "080: an optional step keeps its row where it has no match, its name's `key`, `_type` and `_depth` null and no field of it; a step from an optional name is optional too and its branch matches whole or binds null whole, a join in it to a name outside it a condition of the branch; a `where` of a later step, an absent one's included, reads an optional name as absent where its branch has no match; a null comes after a name's records in the rows' order and sorts as an absent value does, a group key over it is null and a measure reads it as absent"
);

check!(
    TREE_ABSENT = an_absent_step_keeps_its_row_only_where_it_has_no_match,
    "080: an absent step keeps its row only where it has no match from its `from`, its `where` judged as a step's; with `join` only where no walk over its relation reaches the joined record; a record its reader is not shown is no match, so it answers alike whether a hidden record is there or not"
);

check!(
    TREE_RIGHTS = an_optional_or_absent_step_reading_a_hidden_field_is_refused,
    "080 and 024: an optional or an absent step whose `where` reads a field its reader may not know of, and an absent step or a join over a relation it is not shown, are refused as a step of a chain is, never answered as if it had not matched; a field a step's type has that its reader may not know of is as one it does not declare, so a `where` reads the name bound before it by that name as the same pattern naming it otherwise"
);

check!(
    TREE_CURSOR = a_page_of_one_tree_never_continues_another,
    "080 and 018: a tree's rows page on with their cursor as they read in one page, and a cursor handed out for one tree is refused on a pattern whose `from`, `join`, `optional` or `absent` differ, as for another read"
);

check!(
    TREE_BOUND = a_tree_read_whole_past_max_graph_nodes_is_refused_too_large,
    "080 and 026: the rows of a tree read whole, for an order over a step's record or an aggregate, are at most `max_graph_nodes`, refused too_large naming the ceiling past it, nothing partial answered; within it, they answer",
    given
);

check!(
    TREE_REFUSED = a_tree_is_refused_for_what_it_says_before_a_record_is_read,
    "080: a `from` or a `join` naming no name an earlier step binds is refused naming the step and the name; a join that can reach no type its name binds is refused naming the types; a step that walks from or joins an optional name and is not optional is refused naming the name; `as` on a join or an absent step, and `type` or `where` on a join, are refused as binding no name, and either saying `optional` outside an optional branch is refused as binding no name to bind null; a step naming no `from` after a join or an absent step is refused; and `join`, `optional`, `absent` and a `from` naming another name than a walk's own are refused by a graph, a path, a measure and `reach`, naming the member"
);

/// People `alice`..`dan`, `bob` and `carol` reporting to `alice` over
/// `reports_to`; bills `b1`..`b4` with `entered_by` and `contact` over
/// reference fields and `approved_by` over link records, `b1` approved by
/// `bob` before `alice`:
///
/// | bill | entered by | approved by  | contact |
/// |------|------------|--------------|---------|
/// | b1   | alice      | bob, alice   | carol   |
/// | b2   | bob        | alice        |         |
/// | b3   | carol      |              | dan     |
/// | b4   | dan        | dan          |         |
///
/// A bill's `cname` is its contact's name, absent where it has none.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let rel = |name: &str, target: &str, via: Option<&str>| RelationDecl {
        name: name.into(),
        target: target.into(),
        via: via.map(Into::into),
        ..Default::default()
    };
    let text = |name: &str| field(name, StorageKind::Text);
    let mut person = type_decl("person", vec![text("name"), reference("boss", "person")]);
    person.relations = vec![rel("reports_to", "person", Some("boss"))];
    l.declare(person).await?;
    let mut bill = type_decl("bill", vec![text("name"), text("cname")]);
    bill.fields.push(reference("enterer", "person"));
    bill.fields.push(reference("contact", "person"));
    bill.relations = vec![
        rel("entered_by", "person", Some("enterer")),
        rel("has_contact", "person", Some("contact")),
        RelationDecl {
            cardinality: Cardinality::ManyToMany,
            ..rel("approved_by", "person", None)
        },
    ];
    l.declare(bill).await?;
    for (p, boss) in [("alice", None), ("bob", Some("alice"))] {
        l.upsert("person", p, json!({"name": p, "boss": boss}))
            .await?;
    }
    for (p, boss) in [("carol", Some("alice")), ("dan", None)] {
        l.upsert("person", p, json!({"name": p, "boss": boss}))
            .await?;
    }
    for (b, enterer, contact) in [
        ("b1", "alice", Some("carol")),
        ("b2", "bob", None),
        ("b3", "carol", Some("dan")),
        ("b4", "dan", None),
    ] {
        let fields = json!({"name": b, "enterer": enterer, "contact": contact, "cname": contact});
        l.upsert("bill", b, fields).await?;
    }
    let (ty, approved_by) = ("bill".into(), "approved_by".into());
    for (b, p) in [
        ("b1", "bob"),
        ("b1", "alice"),
        ("b2", "alice"),
        ("b4", "dan"),
    ] {
        let (from, to, what) = (b.into(), p.into(), format!("{p} approving {b}"));
        let linked = (l.cat).link(
            &l.own,
            &ty,
            &from,
            &approved_by,
            &to,
            patch(json!({})),
            WriteOpts::default(),
        );
        linked.await.must(&what)?;
    }
    Ok(l)
}

/// One hop over `relation`, bound to `name` where one is given.
fn st(relation: &str, name: Option<&str>) -> Step {
    Step {
        relation: relation.into(),
        walk: Out,
        ty: None,
        filter: None,
        depth: Depth { min: 1, max: 1 },
        named: name.map(Into::into),
        from: None,
        join: None,
        optional: false,
        absent: false,
    }
}

/// `st`, walked from `from`.
fn from(from: &str, relation: &str, name: Option<&str>) -> Step {
    Step {
        from: Some(from.into()),
        ..st(relation, name)
    }
}

/// `st`, walked from `from`, holding only where it reaches what `join`
/// bound.
fn join(from: &str, relation: &str, join: &str) -> Step {
    Step {
        join: Some(join.into()),
        ..self::from(from, relation, None)
    }
}

fn optional(step: Step) -> Step {
    Step {
        optional: true,
        ..step
    }
}

fn absent(step: Step) -> Step {
    Step {
        absent: true,
        ..step
    }
}

fn bills(steps: Vec<Step>) -> Pattern {
    let mut p = pattern("bill", None, steps, Projection::default());
    p.start.named = Some("bill".into());
    p
}

/// A value as a row reads it: text bare, `-` where there is none.
fn cell(v: Option<&Value>) -> String {
    match v {
        None => "-".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

/// Each row of `p`'s answer to `who` as the values at `read` joined by
/// a space, or the refusal.
async fn rows(cat: &dyn Catalog, who: &Principal, p: Pattern, read: &[&str]) -> Vec<String> {
    match cat.query(who, p).await.map(|r| r.data) {
        Ok(QueryResult::Rows { rows, .. }) => (rows.iter())
            .map(|r| {
                (read.iter())
                    .map(|k| cell(r.get(*k)))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect(),
        Ok(QueryResult::Groups { rows }) => (rows.iter())
            .map(|g| {
                format!(
                    "{}={}",
                    g.by.values().map(|v| cell(Some(v))).collect::<String>(),
                    cell(g.values.get("n"))
                )
            })
            .collect(),
        other => vec![format!("{other:?}")],
    }
}

async fn a_step_walks_from_a_name_bound_before_it_and_a_join_closes_a_cycle(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    let read = ["bill.key", "e.key", "a.key"];
    let branch = |filter: Option<&str>| {
        let mut approved = from("bill", "approved_by", Some("a"));
        approved.filter = filter.map(Into::into);
        bills(vec![st("entered_by", Some("e")), approved])
    };
    let both = rows(cat, own, branch(None), &read).await;
    ensure!(
        both == [
            "b1 alice alice",
            "b1 alice bob",
            "b2 bob alice",
            "b4 dan dan"
        ],
        "a bill's enterer and, from the bill, each approver are every combination, the approvers \
         of b1 in key order though bob's link was written first, and b3, approved by no one, no \
         row; it answered {both:?}"
    );
    // A `where` reads the sibling branch's name and the start's.
    let other = rows(cat, own, branch(Some("name != e.name")), &read).await;
    let named = rows(cat, own, branch(Some("bill.name = 'b2'")), &read).await;
    ensure!(
        other == ["b1 alice bob", "b2 bob alice"] && named == ["b2 bob alice"],
        "an approver whose name is not the enterer's is bob for b1 and alice for b2, and a step \
         `where` over `bill.name` keeps b2's; it answered {other:?} and {named:?}"
    );
    let joined = bills(vec![
        st("entered_by", Some("p")),
        join("bill", "approved_by", "p"),
    ]);
    let cycle = rows(cat, own, joined, &["bill.key", "p.key"]).await;
    ensure!(
        cycle == ["b1 alice", "b4 dan"],
        "the bills their enterer approved are b1 (alice) and b4 (dan): a join holds where it \
         reaches the joined record; it answered {cycle:?}"
    );
    // A chain with every `from` named is the chain that names none.
    let chain = |named: bool| {
        let mut up = st("reports_to", Some("boss"));
        up.from = named.then(|| "c".into());
        let mut first = st("has_contact", Some("c"));
        first.from = named.then(|| "bill".into());
        bills(vec![first, up])
    };
    let read = ["bill.key", "c.key", "boss.key"];
    let (unnamed, named) = (
        rows(cat, own, chain(false), &read).await,
        rows(cat, own, chain(true), &read).await,
    );
    ensure!(
        unnamed == ["b1 carol alice"] && named == unnamed,
        "a chain whose steps name their `from` answers as the one that names none, b1's contact \
         carol and her boss alice; it answered {named:?}, where the chain answered {unnamed:?}"
    );
    Ok(())
}

async fn an_optional_branch_matches_whole_or_binds_null_whole(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    let contact = || optional(st("has_contact", Some("c")));
    let read = ["bill.key", "c.key", "c._type", "c._depth", "c.name"];
    let got = rows(cat, own, bills(vec![contact()]), &read).await;
    ensure!(
        got == [
            "b1 carol person 1 carol",
            "b2 null null null -",
            "b3 dan person 1 dan",
            "b4 null null null -"
        ],
        "an optional contact keeps every bill, binding `c.key`, `c._type` and `c._depth` to null \
         and no `c.name` where there is none; it answered {got:?}"
    );
    // Dan reports to no one: b3's branch has no whole match and is null.
    let branch = bills(vec![
        contact(),
        optional(from("c", "reports_to", Some("cb"))),
    ]);
    let got = rows(cat, own, branch, &["bill.key", "c.key", "cb.key"]).await;
    ensure!(
        got == [
            "b1 carol alice",
            "b2 null null",
            "b3 null null",
            "b4 null null"
        ],
        "a contact and, from it, the contact's boss are one branch, matched whole for b1 and null \
         whole for b3, whose contact dan has no boss; it answered {got:?}"
    );
    // A join in the branch to the enterer: only b1's contact, carol,
    // reports to its enterer, alice; b3's dan does not, so b3's is null.
    let joined = bills(vec![
        st("entered_by", Some("e")),
        optional(from("bill", "has_contact", Some("c"))),
        optional(join("c", "reports_to", "e")),
    ]);
    let got = rows(cat, own, joined, &["bill.key", "e.key", "c.key"]).await;
    ensure!(
        got == [
            "b1 alice carol",
            "b2 bob null",
            "b3 carol null",
            "b4 dan null"
        ],
        "a join in an optional branch to a name outside it is a condition of the branch: b1's \
         contact reports to its enterer, b3's does not and binds null; it answered {got:?}"
    );
    // A later step's `where` reads a null name as absent: b1's contact is
    // carol and b4's enterer dan; b2's has no contact and bob entered it.
    let read_past = |s: Step| {
        let filter = Some("c.name = 'carol' or name = 'dan'".into());
        bills(vec![contact(), Step { filter, ..s }])
    };
    let (kept, unkept) = (
        rows(
            cat,
            own,
            read_past(from("bill", "entered_by", Some("e"))),
            &["bill.key", "c.key", "e.key"],
        )
        .await,
        rows(
            cat,
            own,
            read_past(absent(from("bill", "entered_by", None))),
            &["bill.key", "c.key"],
        )
        .await,
    );
    ensure!(
        kept == ["b1 carol alice", "b4 null dan"] && unkept == ["b2 null", "b3 dan"],
        "a step whose `where` reads the optional contact as absent where there is none keeps b1, \
         whose contact is carol, and b4, entered by dan, and an absent step so the other two; \
         they answered {kept:?} and {unkept:?}"
    );
    // Within b1, the contact carol, whose boss alice approved it, comes
    // before the null the branch binds where bob approved it.
    let boss = Step {
        filter: Some("name = a.name".into()),
        ..optional(from("c", "reports_to", Some("cb")))
    };
    let tree = bills(vec![
        contact(),
        from("bill", "approved_by", Some("a")),
        boss,
    ]);
    let got = rows(cat, own, tree, &["bill.key", "c.key", "a.key", "cb.key"]).await;
    ensure!(
        got == [
            "b1 carol alice alice",
            "b1 null bob null",
            "b2 null alice null",
            "b4 null dan null"
        ],
        "a null comes after a name's records: b1's branch is carol for alice's approval and null \
         for bob's, in that order; it answered {got:?}"
    );
    // A null sorts as an absent value: as the bills by their `cname`.
    let by = |field: &str, dir| {
        let mut p = bills(vec![contact()]);
        p.order_by = vec![order(field, OrderBy::Value, dir)];
        rows(cat, own, p, &["bill.key"])
    };
    for dir in [Direction::Asc, Direction::Desc] {
        let (named, absent) = (by("c.name", dir).await, by("bill.cname", dir).await);
        ensure!(
            named == absent,
            "bills ordered by an optional contact's name ({dir:?}) come as by their `cname`, \
             absent where they have no contact, {absent:?}; they came {named:?}"
        );
    }
    let a = serde_json::from_value(json!({
        "measures": [{"name": "n", "func": "count"}], "group_by": ["c.name"]}))
    .map_err(|e| Failure::new(e.to_string()))?;
    let mut grouped = bills(vec![contact()]);
    grouped.project = Projection::Aggregate(a);
    let groups = rows(cat, own, grouped, &[]).await;
    ensure!(
        groups.len() == 3
            && ["null=2", "carol=1", "dan=1"]
                .iter()
                .all(|g| groups.iter().any(|x| x == g)),
        "the bills grouped by their optional contact's name are two under null, one under carol \
         and one under dan; they were {groups:?}"
    );
    // Two of four bills have a contact, each at depth 1.
    let a = serde_json::from_value(json!({
        "measures": [{"name": "n", "func": "avg", "field": "c._depth"}]}))
    .map_err(|e| Failure::new(e.to_string()))?;
    let mut mean = bills(vec![contact()]);
    mean.project = Projection::Aggregate(a);
    let got = rows(cat, own, mean, &[]).await;
    ensure!(
        got == ["=1"] || got == ["=1.0"],
        "the mean depth of the bills' optional contacts is 1, a null read as absent; it was \
         {got:?}"
    );
    Ok(())
}

async fn an_absent_step_keeps_its_row_only_where_it_has_no_match(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    let unapproved = bills(vec![
        st("entered_by", Some("p")),
        absent(join("bill", "approved_by", "p")),
    ]);
    let got = rows(cat, own, unapproved, &["bill.key", "p.key"]).await;
    ensure!(
        got == ["b2 bob", "b3 carol"],
        "the bills their own enterer did not approve are b2 (bob) and b3 (carol); it answered \
         {got:?}"
    );
    let none = |filter: Option<&str>| {
        let mut step = absent(st("has_contact", None));
        step.filter = filter.map(Into::into);
        bills(vec![step])
    };
    let (no_contact, no_dan) = (
        rows(cat, own, none(None), &["bill.key"]).await,
        rows(cat, own, none(Some("name = 'dan'")), &["bill.key"]).await,
    );
    ensure!(
        no_contact == ["b2", "b4"] && no_dan == ["b1", "b2", "b4"],
        "the bills with no contact are b2 and b4, and those with no contact named dan all but b3: \
         an absent step's `where` is judged as a step's; it answered {no_contact:?} and {no_dan:?}"
    );
    // Carol held: to a reader not shown her, b1 has no contact.
    crate::conformance::fixture::restrict(store.as_ref(), "person", "carol", "a held person")
        .await?;
    let eve = crate::conformance::fixture::reads_all(store.as_ref(), "eve");
    let (seen, shown) = (
        rows(cat, &eve, none(None), &["bill.key"]).await,
        rows(cat, own, none(None), &["bill.key"]).await,
    );
    ensure!(
        seen == ["b1", "b2", "b4"] && shown == ["b2", "b4"],
        "a contact eve is not shown is no match: she reads b1, b2 and b4 as having none, and the \
         owner b2 and b4; they read {seen:?} and {shown:?}"
    );
    Ok(())
}

async fn an_optional_or_absent_step_reading_a_hidden_field_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let cat = l.cat.as_ref();
    let clerk = field_access(store.as_ref(), "clerk", "person", &[("name", Access::None)]);
    let step = || {
        let mut s = st("has_contact", Some("c"));
        s.filter = Some("name = 'dan'".into());
        s
    };
    for (what, s) in [
        ("a chain's", step()),
        ("an optional", optional(step())),
        (
            "an absent",
            absent(Step {
                named: None,
                ..step()
            }),
        ),
    ] {
        let answer = cat.query(&clerk, bills(vec![s])).await;
        ensure!(
            no_such_field(&answer, "name"),
            "{what} step whose `where` reads a person's `name`, which the clerk may not know of, \
             is refused as for a field `person` does not declare; it answered {answer:?}"
        );
    }
    // A clerk not shown a bill's `contact` is not shown `has_contact`.
    let clerk = field_access(
        store.as_ref(),
        "clerk2",
        "bill",
        &[("contact", Access::None)],
    );
    let e = || st("entered_by", Some("e"));
    let refused = |a: &Result<_, Error>| reason(a).cloned();
    let chain = refused(
        &cat.query(&clerk, bills(vec![st("has_contact", None)]))
            .await,
    );
    for (what, s) in [
        ("an absent step", absent(from("bill", "has_contact", None))),
        ("a join", join("bill", "has_contact", "e")),
    ] {
        let answer = cat.query(&clerk, bills(vec![e(), s])).await;
        ensure!(
            chain.is_some() && refused(&answer) == chain,
            "{what} over `has_contact`, which the clerk is not shown, is refused as a chain's step \
             is, {chain:?}; it answered {answer:?}"
        );
    }
    // A clerk not shown a person's `boss` reads `boss.name` on a contact as
    // the name bound before it, as `e.name` where the enterer is `e`.
    let clerk = field_access(
        store.as_ref(),
        "clerk3",
        "person",
        &[("boss", Access::None)],
    );
    let named = |name: &str| {
        let mut c = from("bill", "has_contact", Some("c"));
        c.filter = Some(format!("{name}.name = 'alice'").as_str().into());
        bills(vec![st("entered_by", Some(name)), c])
    };
    let (shadowed, plain) = (
        rows(cat, &clerk, named("boss"), &["bill.key"]).await,
        rows(cat, &clerk, named("e"), &["bill.key"]).await,
    );
    ensure!(
        plain == ["b1"] && shadowed == plain,
        "the bills whose contact's `where` reads the enterer alice by a name the clerk may not \
         know a person's field of answer as by another name, {plain:?}; they answered {shadowed:?}"
    );
    Ok(())
}

async fn a_page_of_one_tree_never_continues_another(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    // Alice and dan report to no one, and b2 and b4 have no contact.
    let tree = bills(vec![
        st("entered_by", Some("e")),
        optional(from("bill", "has_contact", Some("c"))),
        optional(from("e", "reports_to", Some("x"))),
    ]);
    let read = ["bill.key", "e.key", "c.key", "x.key"];
    let whole = rows(cat, own, tree.clone(), &read).await;
    let (mut paged, mut cursor, mut first) = (Vec::new(), None, None);
    loop {
        let p = Pattern {
            limit: Some(1),
            cursor: cursor.clone(),
            ..tree.clone()
        };
        let QueryResult::Rows { rows, next_cursor } = cat.query(own, p).await.must("a page")?.data
        else {
            return Err(Failure::new("a tree's rows answered another shape"));
        };
        paged.extend(rows.iter().map(|r| read.map(|k| cell(r.get(k))).join(" ")));
        first = first.or_else(|| next_cursor.clone());
        cursor = next_cursor;
        if cursor.is_none() || paged.len() > whole.len() {
            break;
        }
    }
    ensure!(
        whole
            == [
                "b1 alice carol null",
                "b2 bob null alice",
                "b3 carol dan alice",
                "b4 dan null null"
            ]
            && paged == whole,
        "a tree's rows a row a page are its rows in one page; it answered {paged:?}, where one \
         page answered {whole:?}"
    );
    let first = first.ok_or_else(|| Failure::new("a page of one row has a next"))?;
    // Each a tree that differs from it in one member, which it reads.
    let with = |i: usize, step: Step| {
        let mut p = tree.clone();
        p.steps[i] = step;
        p
    };
    let others = [
        (
            "`c` not optional",
            with(1, from("bill", "has_contact", Some("c"))),
        ),
        (
            "`x` from `c`",
            with(2, optional(from("c", "reports_to", Some("x")))),
        ),
    ];
    for (what, p) in others {
        let p = Pattern {
            limit: Some(1),
            cursor: Some(first.clone()),
            ..p
        };
        let answer = cat.query(own, p).await;
        let case = crate::conformance::fixture::invalid_case(&answer);
        ensure!(
            matches!(case, Some(Case::CursorOtherRead { .. })),
            "a cursor of one tree is refused as one of another read on one with {what}; it \
             answered {answer:?}"
        );
    }
    // The bills whose enterer reports to no one, b1 and b4, a page each,
    // against a tree that differs in `join` alone and one in `absent` alone.
    let unbossed = |step: Step| bills(vec![st("entered_by", Some("e")), step]);
    let base = unbossed(absent(from("e", "reports_to", None)));
    let p = Pattern {
        limit: Some(1),
        ..base
    };
    let page = cat.query(own, p).await.must("a page")?.data;
    let QueryResult::Rows {
        next_cursor: Some(next),
        ..
    } = page
    else {
        return Err(Failure::new(format!(
            "two rows a row a page have a next: {page:?}"
        )));
    };
    for (what, p) in [
        (
            "`join` alone",
            unbossed(absent(join("e", "reports_to", "e"))),
        ),
        ("`absent` alone", unbossed(from("e", "reports_to", None))),
    ] {
        let p = Pattern {
            limit: Some(1),
            cursor: Some(next.clone()),
            ..p
        };
        let answer = cat.query(own, p).await;
        let case = crate::conformance::fixture::invalid_case(&answer);
        ensure!(
            matches!(case, Some(Case::CursorOtherRead { .. })),
            "a cursor of one tree is refused as one of another read on one that differs in \
             {what}; it answered {answer:?}"
        );
    }
    Ok(())
}

async fn a_tree_read_whole_past_max_graph_nodes_is_refused_too_large(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref()).await?;
    // Four bills' rows, read whole for an order over a step's record.
    let mut tree = bills(vec![
        st("entered_by", Some("e")),
        optional(from("bill", "has_contact", Some("c"))),
    ]);
    tree.order_by = vec![order("e.name", OrderBy::Value, Direction::Desc)];
    let mut store = store;
    for max in [4, 3] {
        let limits = Limits {
            max_graph_nodes: max,
            ..crate::conformance::fixture::limits()
        };
        let opened = (given.relimit)(store, Seams::default(), limits).await;
        store = opened.map_err(|e| Failure::new(format!("opening at a cap of {max}: {e}")))?;
        let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
        let answer = cat.query(&own, tree.clone()).await;
        let rows = match &answer {
            Ok(r) => match &r.data {
                QueryResult::Rows { rows, .. } => rows.len(),
                _ => 0,
            },
            Err(_) => 0,
        };
        let refused = matches!(&answer, Err(e) if matches!(reason(e), Some(Refusal::TooLarge { max: m }) if *m == max));
        ensure!(
            if max < 4 { refused } else { rows == 4 },
            "a tree's four rows ordered by the enterer's name answer under a cap of 4 and are \
             refused too_large naming a cap of 3; under {max} it answered {answer:?}"
        );
    }
    Ok(())
}

async fn a_tree_is_refused_for_what_it_says_before_a_record_is_read(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    let said = |p: Pattern| async move {
        let answer = cat.query(own, p).await;
        let said = crate::conformance::fixture::invalid_said(&answer).cloned();
        said.ok_or_else(|| format!("{answer:?}"))
    };
    let step_at = |step: usize, relation: &str| Place::Step {
        step,
        relation: relation.into(),
    };
    let e = || st("entered_by", Some("e"));
    let cases: Vec<(&str, Pattern, Said)> = vec![
        (
            "a `from` naming a later step's name",
            bills(vec![
                from("a", "entered_by", Some("e")),
                from("bill", "approved_by", Some("a")),
            ]),
            Said::from(Case::StepNameUnbound {
                member: "from".into(),
                name: "a".into(),
            })
            .within(step_at(1, "entered_by")),
        ),
        (
            "a `from` naming the empty name after a join",
            bills(vec![
                e(),
                join("bill", "approved_by", "e"),
                from("", "reports_to", None),
            ]),
            Said::from(Case::StepNameUnbound {
                member: "from".into(),
                name: String::new(),
            })
            .within(step_at(3, "reports_to")),
        ),
        (
            "a join to no name",
            bills(vec![e(), join("bill", "approved_by", "x")]),
            Said::from(Case::StepNameUnbound {
                member: "join".into(),
                name: "x".into(),
            })
            .within(step_at(2, "approved_by")),
        ),
        (
            "a join that can reach no type its name binds",
            bills(vec![e(), join("e", "reports_to", "bill")]),
            Said::from(Case::JoinNeverHolds {
                name: "bill".into(),
                bound: ["bill"].into_iter().collect(),
                reached: ["person"].into_iter().collect(),
            })
            .within(step_at(2, "reports_to")),
        ),
        (
            "a step past an optional one that is not optional",
            bills(vec![
                optional(st("has_contact", Some("c"))),
                from("c", "reports_to", Some("cb")),
            ]),
            Said::from(Case::StepPastOptional { name: "c".into() })
                .within(step_at(2, "reports_to")),
        ),
        (
            "a join to an optional name that is not optional",
            bills(vec![
                optional(st("has_contact", Some("c"))),
                join("bill", "approved_by", "c"),
            ]),
            Said::from(Case::StepPastOptional { name: "c".into() })
                .within(step_at(2, "approved_by")),
        ),
        (
            "an optional join outside an optional branch",
            bills(vec![e(), optional(join("bill", "approved_by", "e"))]),
            Said::from(Case::StepOptionalNoName { join: true }).within(step_at(2, "approved_by")),
        ),
        (
            "an optional absent step",
            bills(vec![optional(absent(st("has_contact", None)))]),
            Said::from(Case::StepOptionalNoName { join: false }).within(step_at(1, "has_contact")),
        ),
        (
            "a join that binds a name",
            bills(vec![
                e(),
                Step {
                    named: Some("x".into()),
                    ..join("bill", "approved_by", "e")
                },
            ]),
            Said::from(Case::StepBindsNoName {
                member: "as".into(),
                join: true,
            })
            .within(step_at(2, "approved_by")),
        ),
        (
            "a join with a `where`",
            bills(vec![
                e(),
                Step {
                    filter: Some("name = 'x'".into()),
                    ..join("bill", "approved_by", "e")
                },
            ]),
            Said::from(Case::StepBindsNoName {
                member: "where".into(),
                join: true,
            })
            .within(step_at(2, "approved_by")),
        ),
        (
            "an absent step that binds a name",
            bills(vec![absent(st("has_contact", Some("c")))]),
            Said::from(Case::StepBindsNoName {
                member: "as".into(),
                join: false,
            })
            .within(step_at(1, "has_contact")),
        ),
        (
            "a step naming no `from` after an absent one",
            bills(vec![absent(st("has_contact", None)), e()]),
            Said::from(Case::StepAfterNoName).within(step_at(2, "entered_by")),
        ),
    ];
    for (what, p, expected) in cases {
        let got = said(p).await;
        ensure!(
            got.as_ref() == Ok(&expected),
            "{what} is refused as {expected:?}; it answered {got:?}"
        );
    }
    // A projection that answers by a walk's own shape takes no tree.
    let member = |m: &str, s: Step| (m.to_owned(), s);
    let members = [
        member("optional", optional(st("has_contact", None))),
        member("absent", absent(st("has_contact", None))),
        member("join", join("bill", "has_contact", "bill")),
        member("from", from("x", "has_contact", None)),
    ];
    for (project, to) in [
        (Projection::Graph, None),
        (Projection::Graph, Some("dan")),
        (Projection::Degree, None),
        (Projection::Components, None),
        (Projection::Reach, None),
    ] {
        for (m, s) in &members {
            let s = Step {
                walk: Both,
                ..s.clone()
            };
            let mut p = pattern("bill", Some("b1"), vec![s], project.clone());
            p.to = to.map(|k| ("person".into(), k.into()));
            let what = format!("{project:?} (to {to:?}) with `{m}`");
            let got = said(p).await;
            let fits = matches!(&got, Ok(s) if matches!(&s.case,
                Case::PatternTreeMember { member, .. } if member == m));
            ensure!(
                fits,
                "{what} is refused as pattern_tree_member naming `{m}`; it answered {got:?}"
            );
        }
    }
    Ok(())
}
