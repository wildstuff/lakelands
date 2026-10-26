//! When a computed field is computed (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`):
//! at the write, by the fold, its value stored; or at every read, from
//! the record as stored and the read's clock, never stored.

use lakelands_api::ontology::ComputedAt;
use lakelands_api::principal::PrincipalKind;
use lakelands_api::read::Found;

use crate::conformance::backing::KIND;
use crate::conformance::fixture::{get_at, order};
use crate::conformance::prelude::*;

check!(
    AT_THE_READ = a_field_computed_at_the_read_means_its_expression_in_every_read,
    "052 (lakelands#550): a derived field declared `computed_at: read` is computed at every read from the record as stored and the read's clock, the clock a `where` reads: `get`, a `where` naming it or its expression, an aggregate grouped by it, a view's measure over it, a pattern's start and rows, and a search's hits agree, at the head and as of a position, whose clock is its event's `known_at` (lakelands#571); its value is of the instant it is read, not of the last write; it carries no provenance and no event of the log holds it; a reader is answered it as what it is computed from, masked or hidden; `describe` shows the member and the completed `depends_on`; `order_by` it is refused, in `find` and in a pattern, once the reader is known to read it, and one kept from it or from its type is told nothing; a grant's `only` on it cannot be applied; and a migration that renames its input follows"
);

check!(
    SAID_WHEN = when_a_field_is_computed_is_said_and_kept_from_stored_reads,
    "052 (lakelands#550): a derived expression or an `expr` function that reads the clock says `computed_at`, or is refused naming both; `computed_at: write` stamps the write's instant; a field computed at the read has the authority `derived`, no backing, no `previous`, and is not required, sensitive by itself, cleared or searchable, no key, validity bound or derived relation's match on either side, no field of a view or of a link, and read by no field computed at the write; a trigger, a verb and an object set that read it are refused, and so is a type declared again so that a field one of them or another type's derived relation reads is computed at the read; a verb's payload and a function's input say no `computed_at`; a view that reads it is not kept, no order index names it, and no door writes it"
);

check!(
    MOVED = a_field_declared_again_moves_between_the_write_and_the_read,
    "052 (lakelands#550): saying `computed_at: write` where nothing was said computes nothing again; a field declared again from the write to the read holds no stored value from then on, and one declared back to the write is computed again for every live record"
);

/// `invoice`, with `overdue` and `read_at` computed at the read and
/// `stamped` at the write.
fn invoice(due: &str) -> Value {
    let at = |name: &str, kind: &str, expr: &str, when: &str| {
        json!({"name": name, "type": {"kind": kind}, "authority": "derived",
               "derived": expr, "computed_at": when})
    };
    json!({"declare": "type", "name": "invoice", "fields": [
        {"name": due, "type": {"kind": "date_time"}},
        {"name": "amount", "type": {"kind": "number"}},
        {"name": "note", "type": {"kind": "text"}, "searchable": true},
        at("overdue", "boolean", &format!("{due} < now()"), "read"),
        at("read_at", "date_time", "now()", "read"),
        at("band", "text", &format!("if({due} < now(), 'late', 'early')"), "read"),
        at("stamped", "date_time", "now()", "write"),
    ]})
}

fn instant(r: &Record, f: &str) -> Option<Timestamp> {
    r.fields.get(f)?.as_str()?.parse().ok()
}

fn keys(rows: &[Record]) -> BTreeSet<String> {
    rows.iter().map(|r| r.key.to_string()).collect()
}

async fn a_field_computed_at_the_read_means_its_expression_in_every_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (c, own) = (&l.cat, &l.own);
    l.declare_json(invoice("due_on")).await?;
    let mut at = Position::GENESIS;
    for (key, due, amount, note) in [
        ("i1", Some("2000-01-01T00:00:00Z"), 10, "late"),
        ("i2", Some("2999-01-01T00:00:00Z"), 20, "early"),
        ("i3", None, 30, "soon"),
    ] {
        // `i3` falls due three seconds after it is written, and is read after.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let soon = Timestamp(now.map_or(0, |d| d.as_micros() as i64) + 3_000_000);
        let due = due.map_or(soon.to_string(), str::to_owned);
        let fields = json!({"due_on": due, "amount": amount, "note": note});
        at = l.upsert("invoice", key, fields).await?.position();
    }
    pause(Duration::from_millis(3200)).await;
    // As of `at`, the clock is that event's `known_at`, before `i3` fell due.
    let then = log_events(store.as_ref()).await?;
    let then = then.iter().find(|e| e.position == at).map(|e| e.known_at);
    for as_of in [None, Some(at)] {
        let when = format!("as of {as_of:?}");
        let late: BTreeSet<String> = match as_of {
            None => ["i1", "i3"].map(String::from).into(),
            Some(_) => ["i1"].map(String::from).into(),
        };
        for key in ["i1", "i2", "i3"] {
            let r = c.get(own, &"invoice".into(), get_at(key, as_of)).await;
            let r = r.must(&format!("reading `{key}` {when}"))?.data;
            let (due, read) = (instant(&r, "due_on"), instant(&r, "read_at"));
            let said = r.fields.get("overdue").and_then(Value::as_bool);
            ensure!(
                read.is_some() && said == Some(due < read),
                "`overdue` is `due_on < now()` computed at the read, with the clock `read_at` \
                 reads; `{key}` {when} answers {r:?}"
            );
            ensure!(
                as_of.is_none() || read == then,
                "as of a position, the read's clock is the `known_at` of its event, {then:?}; \
                 `{key}` {when} reads `read_at` {read:?}"
            );
            ensure!(
                !r.meta.contains_key("overdue") && !r.meta.contains_key("read_at"),
                "a field computed at the read was written by no event and carries no \
                 provenance; `{key}` {when} answers {:?}",
                r.meta
            );
        }
        let i3 = c.get(own, &"invoice".into(), get_at("i3", as_of)).await;
        let i3 = i3.must("reading `i3`")?.data;
        ensure!(
            instant(&i3, "stamped") < instant(&i3, "due_on"),
            "`i3` was written before it fell due: its value is of the read, not of the write; \
             {when} it answers {i3:?}"
        );
        for filter in ["overdue", "due_on < now()", "overdue and due_on < now()"] {
            let mut find = find_where(filter);
            find.at = read_at(as_of);
            let found = keys(&all_found(c.as_ref(), own, "invoice", find).await?);
            ensure!(
                found == late,
                "a `where` that names `overdue`, or its expression, reads the clock `overdue` \
                 is computed with: `{filter}` {when} must find {late:?}, and found {found:?}"
            );
        }
        let mut by = aggregate(vec![agg("n", AggFunc::Count, None)]);
        (by.aggregate.as_mut().expect("an aggregate").group_by) =
            vec![GroupKey::Field("overdue".into())];
        by.at = read_at(as_of);
        let groups = c.find(own, &"invoice".into(), by).await.map(|g| g.data);
        let Ok(Found::Groups { rows }) = groups else {
            return Err(Failure::new(format!(
                "grouping by `overdue` answered {groups:?}"
            )));
        };
        let counted: Vec<(Value, Value)> = (rows.iter())
            .map(|g| (g.by["overdue"].clone(), g.values["n"].clone()))
            .collect();
        let n = |k: usize| json!(k.to_string());
        ensure!(
            counted.contains(&(json!(true), n(late.len())))
                && counted.contains(&(json!(false), n(3 - late.len()))),
            "grouped by `overdue` {when}, {late:?} are late and the rest is not; it answers \
             {counted:?}"
        );
        let fields = vec!["invoice.overdue".into()];
        let mut rows = pattern("invoice", None, vec![], Projection::Rows { fields });
        (rows.start.filter, rows.at) = (Some("overdue".into()), read_at(as_of));
        let rows = c.query(own, rows);
        let rows = rows.await.must("a pattern from what is overdue")?;
        let shown = format!("{:?}", rows.data);
        ensure!(
            shown.matches("Bool(true)").count() == late.len() && !shown.contains("Bool(false)"),
            "a pattern that starts where `overdue` holds starts at the late records, {late:?}, \
             and answers `invoice.overdue` true for each; {when} it answers {shown}"
        );
        let search = Search {
            ty: "invoice".into(),
            mode: SearchMode::lexical("late"),
            k: 5,
            cursor: None,
            filter: Some("overdue".into()),
            with_meta: false,
            matches: false,
            at: read_at(as_of),
            rerank: None,
        };
        let hits = c.search(own, search).await.must("searching what is late")?;
        let hit = hits.data.hits.first().map(|h| &h.record);
        ensure!(
            hits.data.hits.len() == 1
                && hit.is_some_and(|r| r.fields.get("overdue") == Some(&json!(true))),
            "a search for `late` among what is overdue hits `i1`, `overdue` true; {when} it \
             answers {:?}",
            hits.data
        );
    }
    let fields = vec![field("late", StorageKind::Number)];
    let sum = [("late", "sum(if(overdue, amount, 0))")];
    l.declare(view_of("lateness", "invoice", &[], fields, &sum))
        .await?;
    let late = l.all_found("lateness", Find::default()).await?;
    let sum = late.first().and_then(|r| r.fields.get("late"));
    ensure!(
        sum.and_then(Value::as_f64) == Some(40.0),
        "a view's measure that sums the amount of what is `overdue` adds 10 and 30; it \
         answers {late:?}"
    );
    let held = |v: &Value| v.get("overdue").is_some() || v.get("read_at").is_some();
    for e in log_events(store.as_ref()).await? {
        let body = serde_json::to_value(&e.body).unwrap_or_default();
        ensure!(
            !body.get("patch").is_some_and(held),
            "no event of the log holds a field computed at the read, nor does the feed or an \
             export of it; {} does: {body}",
            e.position
        );
    }
    let shape = l.describe_type("invoice").await?;
    let overdue = shape.fields.iter().find(|f| f.name.as_str() == "overdue");
    ensure!(
        overdue.is_some_and(|f| f.computed_at == Some(ComputedAt::Read)
            && f.depends_on.iter().any(|d| d.as_str() == "due_on")),
        "`describe` shows `overdue` computed at the read, depending on `due_on`; it shows \
         {overdue:?}"
    );
    let by = || Find {
        order_by: vec![order("overdue", OrderBy::Value, Direction::Asc)],
        ..Find::default()
    };
    let untold = |e: &lakelands_api::Error| {
        !matches!(
            e.case(),
            Some(Said {
                case: Case::OrderAtRead { .. },
                ..
            })
        )
    };
    for (access, masked) in [(Access::Masked, true), (Access::None, false)] {
        let clerk = field_access(store.as_ref(), "clerk", "invoice", &[("due_on", access)]);
        let r = c.get(&clerk, &"invoice".into(), get_at("i1", None)).await;
        let r = r.must("reading `i1` as the clerk")?.data;
        let marked = r.masked.iter().any(|f| f.as_str() == "overdue");
        ensure!(
            !r.fields.contains_key("overdue") && marked == masked,
            "a field computed at the read is read as what it is computed from: with `due_on` \
             {access:?} to the clerk, `overdue` is masked exactly where it is; it reads {r:?}"
        );
        let said = c.find(&clerk, &"invoice".into(), by()).await;
        ensure!(
            said.as_ref().is_err_and(untold),
            "an order by `overdue`, which the clerk may not read, is refused as a probe, \
             telling nothing of how it is computed; it answered {said:?}"
        );
    }
    let grants = |g: Value| serde_json::from_value(g).map_err(|e| Failure::new(e.to_string()));
    let hidden = json!({"default": "read", "types": {"invoice": {"default": "none"}}});
    let stranger = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "stranger",
        grants(hidden)?,
    );
    l.declare(type_decl("memo", vec![field("text", StorageKind::Text)]))
        .await?;
    l.upsert("memo", "m1", json!({"text": "x"})).await?;
    let mut rows = pattern("memo", None, vec![], Projection::Rows { fields: vec![] });
    rows.order_by = vec![order("invoice.overdue", OrderBy::Value, Direction::Asc)];
    let said = c.query(&stranger, rows).await;
    ensure!(
        said.as_ref().map_or_else(untold, |_| true),
        "a pattern ordered by a field of `invoice`, a type its reader is not shown, tells \
         nothing of the field; it answered {said:?}"
    );
    let only = json!({"default": "read", "types": {"invoice": {"only": {"band": "late"}}}});
    let eve = principal(store.as_ref(), PrincipalKind::Agent, "eve", grants(only)?);
    refused_as!(
        "a grant's `only` on `band`, computed at the read",
        c.find(&eve, &"invoice".into(), Find::default()).await,
        Case::OnlyUnappliable { ty } if ty == "invoice"
    )?;
    refused_as!(
        "`find` ordered by `overdue`",
        c.find(own, &"invoice".into(), by()).await,
        Case::OrderAtRead { field, ty } if field == "overdue" && ty == "invoice"
    )?;
    let mut rows = pattern("invoice", None, vec![], Projection::Rows { fields: vec![] });
    rows.order_by = vec![order("invoice.overdue", OrderBy::Value, Direction::Asc)];
    let said = c.query(own, rows).await;
    refused_as!(
        "a pattern ordered by `invoice.overdue`",
        said,
        Case::OrderAtRead { field, ty } if field == "overdue" && ty == "invoice"
    )?;
    let Ok(Declaration::Type(to)) = serde_json::from_value(invoice("due")) else {
        return Err(Failure::new("the check's type does not read"));
    };
    let renamed = Migration {
        renames: vec![("due_on".into(), "due".into())],
        ..migration(to)
    };
    let report = c.migrate(own, renamed.clone());
    let report = report.await.must("renaming `due_on`")?;
    let id = report.data.report_id;
    let fut = c.migrate_apply(own, renamed, &id);
    fut.await.must("renaming `due_on`")?;
    let i1 = l.read("invoice", "i1").await?;
    ensure!(
        i1.fields.get("overdue") == Some(&json!(true)),
        "renamed to `due`, the input is read by `overdue` under its new name; `i1` reads {i1:?}"
    );
    Ok(())
}

/// A declaration of `t` refused, a line each: what it is, what is
/// changed of the field `late` computed at the read (`-`: no such field;
/// `null` takes a member away), the other fields, the type's other
/// members, and the case it is refused as, on the wire (the members it
/// holds, among others).
const REFUSED: &str = r#"
a clock not said | {"computed_at": null} | [] | {} | {"code": "clock_unsaid", "at": [{"code": "field", "name": "late"}]}
a clock through an `expr` function | {"computed_at": null, "derived": null, "function": "aged"} | [] | {} | {"code": "clock_unsaid", "at": [{"code": "field", "name": "late"}]}
said by a field not computed | {"derived": null, "authority": null} | [] | {} | {"code": "computed_at_uncomputed", "at": [{"code": "field", "name": "late"}]}
authored | {"authority": "authored"} | [] | {} | {"code": "at_read_authority", "at": [{"code": "field", "name": "late"}]}
backed | {"derived": null, "function": "remote"} | [] | {} | {"code": "at_read_backed", "at": [{"code": "field", "name": "late"}]}
reading `previous` | {"derived": "previous(amount) > 0"} | [] | {} | {"code": "at_read_previous", "at": [{"code": "field", "name": "late"}]}
required | {"required": true} | [] | {} | {"code": "at_read_required", "at": [{"code": "field", "name": "late"}]}
sensitive | {"sensitive": true} | [] | {} | {"code": "at_read_sensitive", "at": [{"code": "field", "name": "late"}]}
cleared | {"depends_on": ["due_on"], "on_input_change": "clear"} | [] | {} | {"code": "at_read_cleared", "at": [{"code": "field", "name": "late"}]}
searchable | {"type": {"kind": "text"}, "derived": "note", "searchable": true} | [] | {} | {"code": "at_read_searchable", "at": [{"code": "field", "name": "late"}]}
a key | {} | [] | {"key": ["late"]} | {"code": "at_read_key", "at": [{"code": "field", "name": "late"}]}
a validity bound | {"type": {"kind": "date_time"}, "derived": "due_on"} | [] | {"valid_from": "late"} | {"code": "at_read_valid_from", "at": [{"code": "field", "name": "late"}]}
a derived relation's match | {"type": {"kind": "text"}, "derived": "note"} | [] | {"relations": [{"name": "same", "target": "t", "derived": {"match": [{"from": "late", "to": "note"}]}}]} | {"code": "at_read_match", "at": [{"code": "field", "name": "late"}]}
a view's | {"type": {"kind": "number"}, "derived": "1"} | [] | {"name": "v", "key": ["note"], "view": {"source": "t", "group_by": ["note"], "measures": []}} | {"code": "computed_at_in_view", "at": [{"code": "field", "name": "late"}]}
a link's | - | [] | {"relations": [{"name": "next", "target": "t", "fields": [{"name": "late", "type": {"kind": "boolean"}, "authority": "derived", "derived": "due_on < now()", "computed_at": "read"}]}]} | {"code": "link_field_at_read", "at": [{"code": "relation", "name": "next"}, {"code": "field", "name": "late"}]}
read at the write | {} | [{"name": "flag", "type": {"kind": "boolean"}, "authority": "derived", "derived": "not late"}] | {} | {"code": "at_write_reads_at_read", "field": "late", "at": [{"code": "field", "name": "flag"}]}
a derived relation's target | {"type": {"kind": "text"}, "derived": "note"} | [] | {"relations": [{"name": "same", "target": "t", "derived": {"match": [{"from": "note", "to": "late"}]}}]} | {"code": "pair_at_read", "from": "note", "to": "late", "target": "t"}
"#;

/// `t` with `due_on`, `amount` and `note`, and `more` beside them, and
/// `rest` over its other members.
fn t(more: &[Value], rest: Value) -> Value {
    let mut fields = vec![
        json!({"name": "due_on", "type": {"kind": "date_time"}}),
        json!({"name": "amount", "type": {"kind": "number"}}),
        json!({"name": "note", "type": {"kind": "text"}}),
    ];
    fields.extend(more.iter().cloned());
    over(
        json!({"declare": "type", "name": "t", "fields": fields}),
        &rest,
    )
}

/// `v` with each member of `with` put over it, a null one taken away.
fn over(mut v: Value, with: &Value) -> Value {
    for (k, x) in with.as_object().into_iter().flatten() {
        match (v.as_object_mut(), x.is_null()) {
            (Some(o), true) => drop(o.remove(k)),
            _ => v[k] = x.clone(),
        }
    }
    v
}

fn late() -> Value {
    json!({"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
           "derived": "due_on < now()", "computed_at": "read"})
}

/// Writes the setting `key` as the owner, answering the store's result.
async fn setting(l: &Lake, key: &str, value: Value) -> lakelands_api::Result<Outcome> {
    let fields = json!({"key": key, "value": value});
    try_upsert(
        l.cat.as_ref(),
        &l.own,
        lakelands_api::seam::SETTING_TYPE,
        key,
        fields,
    )
    .await
}

async fn when_a_field_is_computed_is_said_and_kept_from_stored_reads(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let input = json!([{"name": "due_on", "type": {"kind": "date_time"}}]);
    for (name, backing, expr) in [
        ("aged", "expr", json!("due_on < now()")),
        ("remote", KIND, json!(null)),
    ] {
        let f = json!({"declare": "function", "name": name, "backing": backing, "inputs": input});
        l.declare_json(over(f, &json!({"expr": expr}))).await?;
    }
    let json =
        |s: &str| serde_json::from_str::<Value>(s).map_err(|e| Failure::new(format!("`{s}`: {e}")));
    for row in REFUSED.trim().lines() {
        let [what, change, more, rest, case] = row.split(" | ").collect::<Vec<_>>()[..] else {
            return Err(Failure::new(format!(
                "the row `{row}` has not five columns"
            )));
        };
        let mut fields: Vec<Value> = serde_json::from_value(json(more)?).unwrap_or_default();
        if change != "-" {
            fields.push(over(late(), &json(change)?));
        }
        let d = serde_json::from_value(t(&fields, json(rest)?))
            .map_err(|e| Failure::new(format!("the row `{what}` does not read: {e}")))?;
        let said = l.cat.declare(&l.own, d).await;
        let what = format!("a field computed at the read, {what},");
        refused_like(&what, said, &json(case)?)?;
    }
    let at = |name: &str, kind: &str, expr: &str, when: &str| {
        json!({"name": name, "type": {"kind": kind}, "authority": "derived", "derived": expr,
               "computed_at": when})
    };
    let stamp = at("stamp", "date_time", "now()", "write");
    // Every field declared each time, one of them taken to the read: a
    // field left out would be refused as a migration's drop instead.
    let fields = |read: &str| {
        let when = |n: &str| if n == read { "read" } else { "write" };
        let (soon, tag) = (when("soon"), when("tag"));
        let (soon, tag) = (
            at("soon", "date_time", "due_on", soon),
            at("tag", "text", "note", tag),
        );
        [late(), stamp.clone(), soon, tag]
    };
    l.declare_json(t(&fields(""), json!({}))).await?;
    // What reads a field as stored keeps it at the write: declared again
    // to the read, it is refused, naming the field alone.
    l.declare_as(
        trigger_on("watch", "t", Some("soon > due_on")),
        "the trigger `watch`",
    )
    .await?;
    let relation = json!({"name": "tagged", "target": "t", "derived": {"match": [{"from": "code", "to": "tag"}]}});
    let u = json!({"declare": "type", "name": "u", "fields": [{"name": "code", "type": {"kind": "text"}}],
                   "relations": [relation]});
    l.declare_json(u).await?;
    for name in ["soon", "tag"] {
        let d = serde_json::from_value(t(&fields(name), json!({})));
        let said = l
            .cat
            .declare(&l.own, d.map_err(|e| Failure::new(e.to_string()))?);
        let what = format!("`{name}` taken to the read");
        let case = json!({"code": "read_as_stored", "at": [{"code": "field", "name": name}]});
        refused_like(&what, said.await, &case)?;
    }
    let p = json!([{"name": "p", "type": {"kind": "text"}, "computed_at": "write"}]);
    for d in [
        json!({"declare": "verb", "name": "pv", "on": "t", "risk": "low", "effect": "write",
               "payload": p, "set": [{"field": "note", "value": "payload.p"}]}),
        json!({"declare": "function", "name": "fi", "backing": "expr", "inputs": p,
               "expr": "upper(p)"}),
    ] {
        let d = serde_json::from_value(d).map_err(|e| Failure::new(e.to_string()))?;
        let said = l.cat.declare(&l.own, d).await;
        let case = json!({"code": "computed_at_no_record", "at": [{"code": "field", "name": "p"}]});
        refused_like("`computed_at` on a field no record holds", said, &case)?;
    }
    let index = format!("{}t", lakelands_api::seam::SORT_INDEX_SETTING);
    let index = setting(&l, &index, json!(["late"])).await;
    refused_as!(
        "an order index of `late`",
        index,
        Case::OrderIndexAtRead { field } if field == "late"
    )?;
    for (what, d) in [
        ("a trigger", trigger_on("tr", "t", Some("late"))),
        (
            "a verb",
            write_verb("vb", "t", "note", "if(late, 'late', 'early')"),
        ),
        ("an object set", object_set_on("st", "t", "late")),
    ] {
        refused_as!(
            &format!("{what} that reads `late`"),
            l.cat.declare(&l.own, d).await,
            Case::ReadsAtRead { field, ty } if field == "late" && ty == "t"
        )?;
    }
    let mut kept = view_of(
        "lates",
        "t",
        &["note"],
        vec![
            field("note", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
        &[("n", "count()")],
    );
    kept.view.as_mut().expect("a view").r#where = Some("late".into());
    l.declare(kept).await?;
    let kept = format!("{}lates", lakelands_api::seam::VIEW_KEPT_SETTING);
    let kept = setting(&l, &kept, json!(true)).await;
    refused_as!(
        "keeping a view whose `where` reads `late`",
        kept,
        Case::KeptWhereAtRead { field, source } if field == "late" && source == "t"
    )?;
    let late = json!({"note": "x", "late": true});
    let written = try_upsert(l.cat.as_ref(), &l.own, "t", "k", late).await;
    ensure!(
        written.is_err(),
        "no door writes a field computed at the read; an upsert that names `late` answered \
         {written:?}"
    );
    Ok(())
}

async fn a_field_declared_again_moves_between_the_write_and_the_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let sheet = |when: Option<&str>| {
        let mut double = json!({"name": "double", "type": {"kind": "number"},
                                "authority": "derived", "derived": "amount * 2"});
        if let Some(when) = when {
            double["computed_at"] = json!(when);
        }
        json!({"declare": "type", "name": "sheet",
               "fields": [{"name": "amount", "type": {"kind": "number"}}, double]})
    };
    // `rated` is as of its last write: the setting changes after it, and
    // saying `write` where nothing was said must not compute it again.
    let rate = |when: Option<&str>| {
        let mut f = json!({"name": "rated", "type": {"kind": "number"}, "authority": "derived",
                           "derived": "amount * setting('rate')"});
        if let Some(when) = when {
            f["computed_at"] = json!(when);
        }
        json!({"declare": "type", "name": "rate", "fields": [{"name": "amount", "type": {"kind": "number"}}, f]})
    };
    l.set("rate", json!(2)).await?;
    l.declare_json(rate(None)).await?;
    l.upsert("rate", "r1", json!({"amount": 2})).await?;
    l.set("rate", json!(3)).await?;
    let before = l.read("rate", "r1").await?;
    l.declare_json(rate(Some("write"))).await?;
    let after = l.read("rate", "r1").await?;
    ensure!(
        before.fields.get("rated").and_then(Value::as_f64) == Some(4.0)
            && (&after.fields, after.meta.get("rated"))
                == (&before.fields, before.meta.get("rated")),
        "saying `computed_at: write` where nothing was said computes nothing again: `rated` \
         keeps 4, of the rate it was written under, and its provenance; it read {before:?}, \
         and reads {after:?}"
    );
    l.declare_json(sheet(None)).await?;
    l.upsert("sheet", "s1", json!({"amount": 2})).await?;
    for (when, amount, stored) in [(Some("read"), 3, false), (Some("write"), 4, true)] {
        let moved = l.declare_json(sheet(when)).await?.position();
        let r = l.read("sheet", "s1").await?;
        let was = (amount - 1) as f64 * 2.0;
        let meta = r.meta.get("double").map(|m| m.position);
        ensure!(
            r.fields.get("double").and_then(Value::as_f64) == Some(was)
                && meta == stored.then_some(moved),
            "declared `computed_at: {when:?}`, `double` reads {was} and is {} at the \
             declaration; it reads {r:?}",
            if stored {
                "computed again"
            } else {
                "stored no more"
            }
        );
        l.upsert("sheet", "s1", json!({"amount": amount})).await?;
        let r = l.read("sheet", "s1").await?;
        ensure!(
            r.fields.get("double").and_then(Value::as_f64) == Some(amount as f64 * 2.0)
                && r.meta.contains_key("double") == stored,
            "declared `computed_at: {when:?}`, `double` is computed from `amount` {amount}, \
             stored {stored}; it reads {r:?}"
        );
    }
    Ok(())
}
