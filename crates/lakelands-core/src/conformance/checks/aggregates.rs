//! Aggregates over a set of records (`lakelands_api::read::Aggregate`,
//! `docs/decisions/011-storage-kind-and-value-type.md`).

use lakelands_api::Value;
use lakelands_api::read::ReadAt;

use crate::conformance::prelude::*;

check!(
    COUNT_IS_THE_WALK = a_count_answers_as_many_records_as_a_full_find_walk,
    "the read contract and 029: `find` with only `count()` answers as many records as a walk of every page of the same `find` returns, with no filter, a filter the engine applies and one only the store evaluates, at `valid_at`, at an earlier position and over a view, for a principal with the `restricted` grant and one without; a superseded state, a retired record and, without the grant, a restricted one are not counted"
);

check!(
    SETTING_IN_A_GROUP_KEY = a_group_key_reads_a_setting_as_a_where_reads_it,
    "the read contract (lakelands#563): an aggregate's named group key that calls `setting` reads the lake's setting, as the same call in `where` does, in `find` and in a pattern: `setting('threshold')` groups by its value, and `amount > setting('threshold')`, in a pattern `bill.amount > setting('threshold')` (lakelands#720), splits the records it holds for from the rest"
);

check!(
    PATTERN_AGGREGATE_READS_PATHS = a_pattern_aggregate_reads_its_rows_by_their_paths,
    "018, extended (lakelands#720): a pattern's aggregate names each value as its rows hold it, `<name>.<field>` or `<name>.key` of a name the pattern binds: a field key and a measure by the path, a named key's paths through a field into members, over a step too; a path no bound name starts is refused alike for a type absent and one not shown, over a step binding it too, a named key reading a field its reader may not know of is refused as a field key on it is, one reading a masked or a write-only field as not granted, as `find` refuses it, and one with no meaning as `find` refuses it; a holder of `only` is grouped, counted and keyed over its admitted records alone, at the start and over a step; a null and a missing value fall into one null group; `<name>.key` is a field `key` its reader may know of, judged by its kind as `find` judges it, null where a record holds none, grouping, summing and ordering as `find` does, and the record's key to one kept from it, as over a type with none, by whose `<name>.key` rows are ordered (lakelands#752); a step walked both ways at depth 0, or over a relation two types declare at the default depth, names its `type` where rows or an aggregate bind it or it reads a `where` (lakelands#680), and then binds that type, for an order, `fields` and a group key alike, and rows are sorted before `fields` leaves out a path an order reads"
);

check!(
    EMPTY_AGGREGATE = an_aggregate_over_no_records_answers_zero,
    "the read contract and 011: an aggregate over an empty set answers one group with a count of 0 and a sum of zero, kept at the field's scale"
);

async fn an_aggregate_over_no_records_answers_zero(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    let fut = cat.declare(own, Declaration::ValueType(money));
    fut.await.must("declaring the value type `money`")?;
    let amount = FieldDecl {
        ty: FieldType {
            value_type: Some("money".into()),
            ..FieldType::kind(StorageKind::Decimal)
        },
        ..field("amount", StorageKind::Decimal)
    };
    l.declare(type_decl("payment", vec![amount])).await?;

    let find = aggregate(vec![
        agg("n", AggFunc::Count, None),
        agg("total", AggFunc::Sum, Some("amount")),
    ]);
    let reply = cat
        .find(own, &TypeName::from("payment"), find)
        .await
        .must("aggregating over no payments")?;
    let Found::Groups { rows } = reply.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    ensure!(
        rows.len() == 1,
        "an aggregate without group_by over no records must answer one group; it \
         answered {} ({rows:?})",
        rows.len()
    );
    let values = &rows[0].values;
    ensure!(
        decimal(values.get("n")) == Some(dec("0")) && values.get("total") == Some(&json!("0.00")),
        "over no payments the count must be 0 and the sum of `amount` (scale 2) \
         \"0.00\", an aggregate's values being decimals; the group reads {values:?}"
    );
    Ok(())
}

async fn a_count_answers_as_many_records_as_a_full_find_walk(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut item = type_decl(
        "item",
        vec![
            field("from", StorageKind::DateTime),
            field("tag", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    );
    item.valid_from = Some("from".into());
    l.declare(item).await?;
    l.declare(count_view("by_tag", "item", "tag")).await?;
    for i in 0..30u32 {
        let tag = if i % 2 == 0 { "a" } else { "b" };
        let from = format!("2026-01-{:02}T00:00:00Z", i + 1);
        let key = format!("i{i:02}");
        l.upsert("item", &key, json!({"from": from, "tag": tag, "n": i}))
            .await?;
    }
    l.upsert("item", "i01", json!({"n": 101})).await?;
    for key in ["i03", "i04"] {
        l.retire("item", key).await?;
    }
    restrict(store.as_ref(), "item", "i05", "hold").await?;
    let before = store.head().await.must("reading the head")?;
    let later = json!({"from": "2026-02-01T00:00:00Z", "tag": "a", "n": 30});
    l.upsert("item", "i30", later).await?;
    let reader = reads_all(store.as_ref(), "reader");
    let mid = Timestamp::parse("2026-01-15T00:00:00Z").expect("an instant");
    let at = |as_of, valid_at| ReadAt {
        as_of,
        valid_at,
        clock: None,
    };
    let earlier = at(Some(before), None);
    let cases = [
        ("everything", "item", None, at(None, None)),
        ("n > 10", "item", Some("n > 10"), at(None, None)),
        (
            "a filter only the store evaluates",
            "item",
            Some("n > 10 and now() > now() - 1d"),
            at(None, None),
        ),
        ("valid at 2026-01-15", "item", None, at(None, Some(mid))),
        ("before i30 was written", "item", None, earlier),
        ("the view", "by_tag", None, at(None, None)),
    ];
    let mut everything = Vec::new();
    for (who, name) in [
        (own, "the owner"),
        (&reader, "a reader without `restricted`"),
    ] {
        for (what, ty, filter, at) in &cases {
            let find = || Find {
                filter: filter.map(Into::into),
                at: *at,
                ..Find::default()
            };
            let walked = all_found(cat.as_ref(), who, ty, find()).await?.len() as u64;
            let counting = Find {
                aggregate: aggregate(vec![agg("n", AggFunc::Count, None)]).aggregate,
                ..find()
            };
            let reply = cat
                .find(who, &(*ty).into(), counting)
                .await
                .must(&format!("{name} counting {what}"))?
                .data;
            let counted = match &reply {
                Found::Groups { rows } => rows
                    .first()
                    .and_then(|g| g.values.get("n"))
                    .and_then(Value::as_str)
                    .and_then(|n| n.parse::<u64>().ok()),
                Found::Records { .. } => None,
            };
            ensure!(
                counted == Some(walked),
                "{name} counting {what} must answer the {walked} records a full walk finds; \
                 it answered {reply:?}"
            );
            if *what == "everything" {
                everything.push(walked);
            }
        }
    }
    ensure!(
        everything == [29, 28],
        "31 records, two retired: the owner counts 29 and a reader without `restricted` 28, \
         `i05` being held; they counted {everything:?}"
    );
    Ok(())
}

async fn a_group_key_reads_a_setting_as_a_where_reads_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "bill",
        vec![field("amount", StorageKind::Decimal)],
    ))
    .await?;
    l.set("threshold", json!(10)).await?;
    for (key, amount) in [("b1", "5"), ("b2", "20"), ("b3", "30")] {
        l.upsert("bill", key, json!({"amount": amount})).await?;
    }
    let mut by: Aggregate = serde_json::from_value(json!({
        "measures": [{"name": "n", "func": "count"}],
        "group_by": [{"name": "t", "expr": "setting('threshold')"},
                     {"name": "over", "expr": "amount > setting('threshold')"}]}))
    .map_err(|e| Failure::new(format!("reading the aggregate: {e}")))?;
    let mut find = aggregate(Vec::new());
    find.aggregate = Some(by.clone());
    let reply = (l.cat).find(&l.own, &TypeName::from("bill"), find).await;
    let Found::Groups { rows } = reply.must("aggregating bills")?.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    // A pattern's row holds `amount` as `bill.amount` (lakelands#720).
    by.group_by[1] = serde_json::from_value(json!({"name": "over",
        "expr": "bill.amount > setting('threshold')"}))
    .map_err(|e| Failure::new(format!("reading the group key: {e}")))?;
    let p = pattern("bill", None, Vec::new(), Projection::Aggregate(by));
    let reply = (l.cat).query(&l.own, p).await;
    let QueryResult::Groups { rows: walked } = reply.must("aggregating a pattern")?.data else {
        return Err(Failure::new("a pattern's aggregate answered no groups"));
    };
    let found =
        json!([[{"over": false, "t": "10"}, {"n": "1"}], [{"over": true, "t": "10"}, {"n": "2"}]]);
    for (how, rows, want) in [("find", rows, found.clone()), ("a pattern", walked, found)] {
        let got: Vec<Value> = rows.iter().map(|g| json!([g.by, g.values])).collect();
        ensure!(
            json!(got) == want,
            "{how} grouped by `setting('threshold')`, the setting 10, over 5, 20 and 30 \
             answers {got:?}, not {want}"
        );
    }
    Ok(())
}

async fn a_pattern_aggregate_reads_its_rows_by_their_paths(store: Store) -> Held {
    use crate::conformance::fixture::{
        field_access, invalid_case, principal, reason, reference, step,
    };
    use lakelands_api::error::Case;
    use lakelands_api::principal::{PrincipalKind, TypeGrants};
    use lakelands_api::{Access, ontology::RelationDecl, query::Walk};
    let l = Lake::of(store.as_ref());
    let tier = vec![field("tier", StorageKind::Text)];
    let fields = [
        ("amount", StorageKind::Decimal),
        ("meta", StorageKind::Json),
    ];
    let mut bill = type_decl("bill", fields.map(|(n, k)| field(n, k)).to_vec());
    bill.fields.extend([
        field("on", StorageKind::DateTime),
        reference("cust", "customer"),
    ]);
    let via = Some("cust".into());
    bill.relations = vec![RelationDecl {
        via,
        ..RelationDecl::to("of", "customer")
    }];
    for t in [type_decl("customer", tier), bill] {
        l.declare(t).await?;
    }
    l.set("threshold", json!(10)).await?;
    for (key, tier) in [("c1", "gold"), ("c2", "tin")] {
        l.upsert("customer", key, json!({"tier": tier})).await?;
    }
    for (key, amount, cust, on) in [
        ("b1", "5", "c1", 1),
        ("b2", "20", "c1", 2),
        ("b3", "30", "c2", 2),
    ] {
        let on = format!("2026-0{on}-05T00:00:00Z");
        let bill = json!({"amount": amount, "meta": {"x": key}, "cust": cust, "on": on});
        l.upsert("bill", key, bill).await?;
    }
    // Each group as `<its first key's value>=<its first measure>`, or the
    // refusal.
    let groups = |who: lakelands_api::Principal, stepped: bool, a: Value| {
        let cat = l.cat.clone();
        async move {
            let a: Aggregate = serde_json::from_value(a).map_err(|e| e.to_string())?;
            let steps = if stepped {
                vec![step("of", Walk::Out)]
            } else {
                Vec::new()
            };
            let p = pattern("bill", None, steps, Projection::Aggregate(a));
            let text = |v: Option<&Value>| {
                v.map_or(String::new(), |v| {
                    v.as_str().map_or(v.to_string(), Into::into)
                })
            };
            match cat.query(&who, p).await.map(|r| r.data) {
                Ok(QueryResult::Groups { rows }) => Ok(rows
                    .iter()
                    .map(|g| {
                        format!(
                            "{}={}",
                            text(g.by.values().next()),
                            text(g.values.values().next())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("|")),
                other => Err(format!("{other:?}")),
            }
        }
    };
    // The case the owner's pattern aggregate is refused as, if it is.
    let refused = |a: Value| {
        let (cat, own) = (l.cat.clone(), l.own.clone());
        async move {
            let a: Aggregate = serde_json::from_value(a).ok()?;
            let p = pattern("bill", None, Vec::new(), Projection::Aggregate(a));
            invalid_case(&cat.query(&own, p).await).cloned()
        }
    };
    let n = json!([{"name": "n", "func": "count"}]);
    let named = |expr: &str| json!({"measures": n, "group_by": [{"name": "k", "expr": expr}]});
    let by = |key: &str| json!({"measures": n, "group_by": [key]});
    let sum = |f: &str| json!({"measures": [{"name": "s", "func": "sum", "field": f}]});
    let (jan, feb) = ("2026-01-01T00:00:00.000000Z", "2026-02-01T00:00:00.000000Z");
    let mut wrong = Vec::new();
    for (stepped, a, want) in [
        (
            false,
            named("bill.amount > setting('threshold')"),
            "false=1|true=2".to_owned(),
        ),
        (false, by("bill.amount"), "20=1|30=1|5=1".into()),
        (true, named("customer.tier"), "gold=2|tin=1".into()),
        (true, by("customer.tier"), "gold=2|tin=1".into()),
        (false, named("bill.meta.x"), "b1=1|b2=1|b3=1".into()),
        (
            false,
            named("date_bucket(bill.on, 'month')"),
            format!("{jan}=1|{feb}=2"),
        ),
        (false, named("bill.key"), "b1=1|b2=1|b3=1".into()),
        (false, sum("bill.amount"), "=55".into()),
    ] {
        let got = groups(l.own.clone(), stepped, a.clone()).await;
        if got.as_ref() != Ok(&want) {
            wrong.push(format!("{a}: {got:?}, not {want}"));
        }
    }
    // Refused: a path no bound name starts, one its type does not have, one
    // with no meaning.
    let no_path = |path: &str| Case::PatternNoPath { path: path.into() };
    let unknown = Case::UnknownField {
        ty: "bill".into(),
        field: "zz".into(),
    };
    for (a, said) in [
        (named("amount > 10"), no_path("amount")),
        (by("zz"), no_path("zz")),
        (sum("zz"), no_path("zz")),
        (named("ghost.x"), no_path("ghost.x")),
        (named("bill.zz > 1"), unknown),
    ] {
        let got = refused(a.clone()).await;
        if got.as_ref() != Some(&said) {
            wrong.push(format!("{a}: {got:?}, not refused as {said:?}"));
        }
    }
    let got = refused(named("bill.amount + true")).await;
    if !matches!(got, Some(Case::KindSides { .. })) {
        wrong.push(format!(
            "`bill.amount + true`: {got:?}, not refused as having no meaning"
        ));
    }
    // A reader kept from `bill.amount` and not shown `customer` is answered
    // as for a field and a type that do not exist, over a step binding
    // `customer` too.
    let mut kept = field_access(store.as_ref(), "kept", "bill", &[("amount", Access::None)]);
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    kept.grants.types.insert("customer".into(), none);
    for (path, hidden, absent, keyed, stepped) in [
        ("bill.{}", "amount", "zz", true, false),
        ("bill.{}", "amount", "zz", false, false),
        ("{}.tier", "customer", "ghost", true, false),
        ("{}.tier", "customer", "ghost", true, true),
        ("{}.tier", "customer", "ghost", false, true),
    ] {
        let mut said = Vec::new();
        for n in [hidden, absent] {
            let a = if keyed {
                named(&path.replace("{}", n))
            } else {
                by(&path.replace("{}", n))
            };
            let got = groups(kept.clone(), stepped, a).await;
            said.push(format!("{got:?}").replace(n, "N"));
        }
        if said[0] != said[1] || said[0].starts_with("Ok") {
            wrong.push(format!("{path} with `{hidden}` and `{absent}`: {said:?}"));
        }
    }
    // A named key reading a masked or a write-only field is not granted, as
    // `find`'s is.
    for access in [Access::Masked, Access::WriteOnly] {
        let who = field_access(store.as_ref(), "half", "bill", &[("amount", access)]);
        let mut find = crate::conformance::fixture::aggregate(Vec::new());
        find.aggregate = serde_json::from_value(named("amount > 10")).ok();
        let found = l.cat.find(&who, &TypeName::from("bill"), find).await;
        let a = serde_json::from_value(named("bill.amount > 10"))
            .map_err(|e| Failure::new(format!("reading the aggregate: {e}")))?;
        let p = pattern("bill", None, Vec::new(), Projection::Aggregate(a));
        let walked = l.cat.query(&who, p).await;
        let (f, w) = (reason(&found), reason(&walked));
        if f != w || !matches!(w, Some(lakelands_api::Refusal::NotGranted)) {
            wrong.push(format!("`amount` {access:?}: find {f:?}, a pattern {w:?}"));
        }
    }
    // A holder of `only` is grouped, counted and keyed over its admitted
    // records alone: the bills of `c1` from the start, the bills that reach
    // a gold customer over a step.
    for (only, stepped, a, want) in [
        (
            json!({"bill": {"only": {"cust": "c1"}}}),
            false,
            by("bill.amount"),
            "20=1|5=1",
        ),
        (
            json!({"bill": {"only": {"cust": "c1"}}}),
            false,
            named("bill.key"),
            "b1=1|b2=1",
        ),
        (
            json!({"bill": {"only": {"cust": "c1"}}}),
            false,
            sum("bill.amount"),
            "=25",
        ),
        (
            json!({"bill": {"only": {"cust": "c1"}}}),
            true,
            by("customer.tier"),
            "gold=2",
        ),
        (
            json!({"customer": {"only": {"tier": "gold"}}}),
            true,
            by("customer.tier"),
            "gold=2",
        ),
        (
            json!({"customer": {"only": {"tier": "gold"}}}),
            true,
            named("bill.key"),
            "b1=1|b2=1",
        ),
    ] {
        let grants = serde_json::from_value(json!({"default": "read", "types": only}))
            .map_err(|e| Failure::new(format!("reading the grants: {e}")))?;
        let who = principal(store.as_ref(), PrincipalKind::Agent, "alice", grants);
        let got = groups(who, stepped, a.clone()).await;
        if got.as_deref() != Ok(want) {
            wrong.push(format!("`only` {only}, {a}: {got:?}, not {want}"));
        }
    }
    // A row's `<name>.key` is a field `key` its reader may know of, judged
    // by its kind as `find` judges it; one kept from it reads the record's
    // key, as over a type with no such field.
    let keyed = [("acct", StorageKind::Text), ("flag", StorageKind::Boolean)];
    for (t, kind) in keyed {
        l.declare(type_decl(t, vec![field("key", kind)])).await?;
    }
    l.declare(type_decl("plain", vec![field("n", StorageKind::Number)]))
        .await?;
    l.upsert("acct", "a1", json!({"key": "secret"})).await?;
    l.upsert("plain", "a1", json!({"n": 1})).await?;
    l.upsert("flag", "f1", json!({"key": true})).await?;
    let kept_key = field_access(store.as_ref(), "keyless", "acct", &[("key", Access::None)]);
    let over = |ty: &str, who: &lakelands_api::Principal, a: Value| {
        let (cat, who, ty) = (l.cat.clone(), who.clone(), ty.to_owned());
        async move {
            let a: Aggregate = serde_json::from_value(a).expect("an aggregate");
            let p = pattern(&ty, None, Vec::new(), Projection::Aggregate(a));
            format!("{:?}", cat.query(&who, p).await.map(|r| r.data)).replace(&ty, "T")
        }
    };
    for a in [named("T.key"), by("T.key")] {
        let (hidden, plain) = (
            a.to_string().replace('T', "acct"),
            a.to_string().replace('T', "plain"),
        );
        let hidden = over(
            "acct",
            &kept_key,
            serde_json::from_str(&hidden).expect("json"),
        )
        .await;
        let plain = over(
            "plain",
            &kept_key,
            serde_json::from_str(&plain).expect("json"),
        )
        .await;
        if hidden != plain || !hidden.contains("a1") {
            wrong.push(format!(
                "{a} over a hidden `key` and over none: {hidden} || {plain}"
            ));
        }
    }
    // A declared `key` with no value is null in the row, as in `find`: it
    // groups, sums and orders as `find` does.
    let mut held = type_decl("held", vec![field("key", StorageKind::Text)]);
    held.fields.push(field("no", StorageKind::Text));
    l.declare(held).await?;
    let tally = type_decl(
        "tally",
        vec![
            field("key", StorageKind::Number),
            field("n", StorageKind::Number),
        ],
    );
    l.declare(tally).await?;
    for (k, fields) in [
        ("h1", json!({"key": "b", "no": "1"})),
        ("h2", json!({"no": "2"})),
        ("h3", json!({"key": "a", "no": "3", })),
        ("h4", json!({"key": null, "no": "4"})),
    ] {
        l.upsert("held", k, fields).await?;
    }
    l.upsert("tally", "t1", json!({"key": 7})).await?;
    l.upsert("tally", "t2", json!({"n": 1})).await?;
    let grouped = |rows: &[lakelands_api::read::Group]| -> Vec<String> {
        let mut g: Vec<String> = rows
            .iter()
            .map(|g| format!("{:?}={:?}", g.by.values().next(), g.values.values().next()))
            .collect();
        g.sort();
        g
    };
    let a: Aggregate = serde_json::from_value(by("held.key")).expect("an aggregate");
    let p = pattern("held", None, Vec::new(), Projection::Aggregate(a));
    let walked = match l.cat.query(&l.own, p).await.map(|r| r.data) {
        Ok(QueryResult::Groups { rows }) => grouped(&rows),
        other => vec![format!("{other:?}")],
    };
    let mut find = crate::conformance::fixture::aggregate(Vec::new());
    find.aggregate = serde_json::from_value(by("key")).ok();
    let found = match l
        .cat
        .find(&l.own, &TypeName::from("held"), find)
        .await
        .map(|r| r.data)
    {
        Ok(Found::Groups { rows }) => grouped(&rows),
        other => vec![format!("{other:?}")],
    };
    if walked != found || walked.len() != 3 {
        wrong.push(format!("`held.key` grouped: {walked:?}, `find` {found:?}"));
    }
    let summed = over("tally", &l.own, sum("tally.key")).await;
    if !summed.contains("String(\"7\")") {
        wrong.push(format!("`sum(tally.key)` over 7 and none: {summed}"));
    }
    let mut ordered = pattern(
        "held",
        None,
        Vec::new(),
        Projection::Rows { fields: Vec::new() },
    );
    let up = |f: &str| {
        crate::conformance::fixture::order(
            f,
            lakelands_api::read::OrderBy::Value,
            lakelands_api::read::Direction::Asc,
        )
    };
    ordered.order_by = vec![up("held.key")];
    let walked: Vec<String> = match l.cat.query(&l.own, ordered).await.map(|r| r.data) {
        Ok(QueryResult::Rows { rows, .. }) => rows
            .iter()
            .map(|r| format!("{:?}", r.get("held.no")))
            .collect(),
        other => vec![format!("{other:?}")],
    };
    let by_key = Find {
        order_by: vec![up("key")],
        ..Find::default()
    };
    let found: Vec<String> = all_found(l.cat.as_ref(), &l.own, "held", by_key)
        .await?
        .iter()
        .map(|r| format!("{:?}", r.fields.get("no")))
        .collect();
    if walked != found {
        wrong.push(format!(
            "rows ordered by `held.key`: {walked:?}, `find` by `key` {found:?}"
        ));
    }
    // Rows from `start` ordered down by `by`, projecting `fields`, as each
    // row's `read` joined, or the answer (lakelands#752).
    type Steps = Vec<lakelands_api::query::Step>;
    let rows = |start: &str, steps: Steps, fields: &[&str], by: &str, read: &str| {
        let fields = fields.iter().map(|f| f.to_string()).collect();
        let mut p = pattern(start, None, steps, Projection::Rows { fields });
        p.order_by = vec![crate::conformance::fixture::order(
            by,
            lakelands_api::read::OrderBy::Value,
            lakelands_api::read::Direction::Desc,
        )];
        let (cat, own, read) = (l.cat.clone(), l.own.clone(), read.to_owned());
        async move {
            match cat.query(&own, p).await.map(|r| r.data) {
                Ok(QueryResult::Rows { rows, .. }) => (rows.iter())
                    .map(|r| r.get(&read).map_or("-".into(), |v| v.to_string()))
                    .collect::<Vec<_>>()
                    .join(","),
                other => format!("{other:?}"),
            }
        }
    };
    // The groups by the path `key` from `start`, each with its count.
    let grouped = |start: &str, steps: Steps, key: &str| {
        let a = serde_json::from_value(by(key)).expect("an aggregate");
        let p = pattern(start, None, steps, Projection::Aggregate(a));
        let (cat, own, key) = (l.cat.clone(), l.own.clone(), key.to_owned());
        async move {
            match cat.query(&own, p).await.map(|r| r.data) {
                Ok(QueryResult::Groups { rows }) => (rows.iter())
                    .map(|g| format!("{}={}", g.by[key.as_str()], g.values["n"]))
                    .collect::<Vec<_>>()
                    .join("|"),
                other => format!("{other:?}"),
            }
        }
    };
    // A depth-0 step walked both ways reaches `bill` and `customer`: rows
    // that bind it name its `type` (lakelands#680), and then hold its
    // paths.
    let both = |ty: Option<&str>| {
        let mut s = step("of", Walk::Both);
        (s.depth.min, s.ty) = (0, ty.map(Into::into));
        vec![s]
    };
    let tiers = grouped("bill", both(Some("customer")), "customer.tier").await;
    if tiers != r#""gold"="2"|"tin"="1""# {
        wrong.push(format!(
            "grouped by `customer.tier` over a step both ways: {tiers}"
        ));
    }
    // A relation two types declare, walked in at the default depth: the
    // step reaches bills and quotes, and names its `type` alike.
    let mut quote = type_decl("quote", vec![field("x", StorageKind::Text)]);
    quote.fields.push(reference("cust", "customer"));
    quote.relations = vec![RelationDecl {
        via: Some("cust".into()),
        ..RelationDecl::to("of", "customer")
    }];
    l.declare(quote).await?;
    l.upsert("quote", "q1", json!({"x": "qx", "cust": "c1"}))
        .await?;
    let into = |ty: Option<&str>| {
        let mut s = step("of", Walk::In);
        s.ty = ty.map(Into::into);
        vec![s]
    };
    // Untyped, it is bound under its relation's name and holds the fields
    // its types declare alike (lakelands#990, lakelands#1186): a path that
    // reads another is refused.
    for (start, steps, path) in [
        ("bill", both(None), "of.tier"),
        ("customer", into(None), "of.x"),
    ] {
        let untyped = rows(start, steps.clone(), &[path], path, path).await;
        let groups = grouped(start, steps, path).await;
        if !untyped.contains("StepSeveralTypes") || !groups.contains("StepSeveralTypes") {
            wrong.push(format!(
                "rows and groups by `{path}` over a step with no `type`: {untyped}, {groups}"
            ));
        }
    }
    let quotes = grouped("customer", into(Some("quote")), "quote.x").await;
    if quotes != r#""qx"="1""# {
        wrong.push(format!(
            "grouped by `quote.x` over a relation two types declare: {quotes}"
        ));
    }
    // Its `where` is read against one type: the step names its `type`, as
    // a neighbourhood's does, where it dropped the other type's records.
    let kept = |ty: Option<&str>| {
        let mut s = step("of", Walk::In);
        (s.ty, s.filter) = (ty.map(Into::into), Some("x = 'qx'".into()));
        rows("customer", vec![s], &[], "quote.x", "quote.x")
    };
    let (untyped, typed) = (kept(None).await, kept(Some("quote")).await);
    if !untyped.contains("StepSeveralTypes") || typed != r#""qx""# {
        wrong.push(format!(
            "a `where` over a relation two types declare: {untyped}, with its `type` {typed}"
        ));
    }
    for (start, steps, fields, by, read, want) in [
        // Over a type with no field `key`, by the record's key.
        ("bill", Vec::new(), &[][..], "bill.key", r#""b3","b2","b1""#),
        // Sorted before `fields`, by a path it leaves out.
        (
            "bill",
            Vec::new(),
            &["bill.key"][..],
            "bill.on",
            r#""b2","b3","b1""#,
        ),
        (
            "bill",
            both(Some("customer")),
            &["customer.tier"][..],
            "customer.tier",
            r#""tin","gold","gold""#,
        ),
        (
            "customer",
            into(Some("quote")),
            &["quote.x"][..],
            "quote.x",
            r#""qx""#,
        ),
    ]
    .map(|(start, steps, fields, by, want)| {
        let read = fields.first().copied().unwrap_or(by);
        (start, steps, fields, by, read, want)
    }) {
        let got = rows(start, steps, fields, by, read).await;
        if got != want {
            wrong.push(format!(
                "rows of `{start}` by `{by}` down, `fields` {fields:?}: {got}"
            ));
        }
    }
    let flag = over("flag", &l.own, named("flag.key + 1")).await;
    let mut find = crate::conformance::fixture::aggregate(Vec::new());
    find.aggregate = serde_json::from_value(named("key + 1")).ok();
    let found = l.cat.find(&l.own, &TypeName::from("flag"), find).await;
    let kind_sides = |c: Option<&Case>| matches!(c, Some(Case::KindSides { .. }));
    if !flag.contains("KindSides") || !kind_sides(invalid_case(&found)) {
        wrong.push(format!(
            "`key + 1` over a boolean `key`: a pattern {flag}, find {found:?}"
        ));
    }
    // A null and a missing value fall into one null group, as in `find`.
    l.upsert("bill", "b4", json!({"meta": {"y": 1}})).await?;
    l.upsert("bill", "b5", json!({"meta": {"x": null}})).await?;
    for (a, want) in [
        (by("bill.amount"), "20=1|30=1|5=1|null=2"),
        (named("bill.meta.x"), "b1=1|b2=1|b3=1|null=2"),
    ] {
        let got = groups(l.own.clone(), false, a.clone()).await;
        if got.as_deref() != Ok(want) {
            wrong.push(format!("{a} with b4 and b5: {got:?}, not {want}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a pattern's aggregate over bills:\n{}",
        wrong.join("\n")
    );
    Ok(())
}
