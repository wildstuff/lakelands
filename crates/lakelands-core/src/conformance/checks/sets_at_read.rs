//! A set decided at the read (lakelands#1412,
//! `docs/decisions/066-a-set-holds-a-kept-view.md` § "A set may be
//! decided at the read"): it keeps no membership, a read's `in_set` of it
//! is its `where` at the read's clock, and nothing a write decides names
//! it.

use lakelands_api::write::{DecidedAt, ObjectSetDecl};

use crate::conformance::prelude::*;

check!(
    SET_AT_READ = a_set_decided_at_the_read_judges_at_the_reads_clock,
    "066, lakelands#1412: a set declared `decided_at: read` whose `where` calls `today()`, and one whose `where` names a field computed at the read from `now()`, declare, and neither its declaration nor a write of a record it holds appends `object_set_changed`; a read's `in_set` of each answers what a `find` with its `where` answers at the read's clock: at the head the records due before today, `valid_at` an instant in 2500 also those due before then, so two reads at two clocks answer differently; `describe` shows `decided_at: read`, and a set that says nothing is `write`"
);

check!(
    SET_AT_READ_STEPS = a_set_decided_at_the_read_steps_across_no_reference,
    "066 and 055, lakelands#1412: a set decided at the read whose `where` steps across a reference or calls `exists` is refused `set_at_read_steps` naming the set and the path, one of a view is refused `set_at_read_of_view` naming the set and the view, and one reading a `json` field's member declares"
);

check!(
    SET_AT_READ_NAMED = what_a_write_decides_names_no_set_decided_at_the_read,
    "066, lakelands#1412: a kept set's `where`, a view's `where`, a trigger's `when` and a verb's `requires` that name a set decided at the read by `in_set` are refused `set_at_read_named`, naming the set and what names it, and so is a kept set declared again as decided at the read while a kept set names it; a set decided at the read may name one, and nothing refused is appended"
);

check!(
    SET_AT_READ_AGAIN = a_set_declared_again_between_kept_and_decided_at_the_read,
    "066, lakelands#1412: a kept set declared again as decided at the read ends each of its members in that append, caused by its `declared`, and a later write moves none; declared again as kept it decides the records already there; a rebuild keeps what each left, so a later write decides against it; and one reading `today()` declared again as kept is refused `set_reads_clock`"
);

/// The instant a read names `valid_at`: after `b2` falls due.
const VALID: &str = "2500-01-01T00:00:00Z";

/// The memberships the events after `after` move: (set, key, joined),
/// each caused by the event at `cause` where it is given.
async fn moved(
    s: &dyn LakeStore,
    after: Position,
    cause: Option<Position>,
) -> Result<Vec<(String, String, bool)>, Failure> {
    let mut out = Vec::new();
    for e in events_after(s, after).await? {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = e.body
        {
            ensure!(
                cause.is_none_or(|c| e.cause == Some(c)),
                "`{set}` moving `{key}` is caused by the event at {cause:?}; it names {:?}",
                e.cause
            );
            out.push((set, key.to_string(), joined));
        }
    }
    out.sort();
    Ok(out)
}

fn moves(set: &str, keys: &[&str], joined: bool) -> Vec<(String, String, bool)> {
    (keys.iter())
        .map(|k| (set.to_owned(), (*k).to_owned(), joined))
        .collect()
}

async fn keys_at(l: &Lake, filter: &str, valid_at: Option<Timestamp>) -> Result<String, Failure> {
    let find = Find {
        filter: Some(filter.into()),
        at: ReadAt {
            valid_at,
            ..ReadAt::default()
        },
        ..Find::default()
    };
    let rows = l.all_found("bill", find).await?;
    let mut keys: Vec<String> = rows.iter().map(|r| r.key.to_string()).collect();
    keys.sort();
    Ok(keys.join(" "))
}

fn at_read(name: &str, of: &str, filter: &str) -> Declaration {
    Declaration::ObjectSet(ObjectSetDecl {
        name: name.into(),
        of: of.into(),
        filter: filter.into(),
        description: None,
        decided_at: DecidedAt::Read,
    })
}

async fn bills(l: &Lake) -> Held {
    l.declare_json(json!({"declare": "type", "name": "bill", "fields": [
        {"name": "due", "type": {"kind": "date"}},
        {"name": "due_at", "type": {"kind": "date_time"}},
        {"name": "status", "type": {"kind": "text"}},
        {"name": "meta", "type": {"kind": "json"}},
        {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "due_at < now()", "computed_at": "read"},
    ]}))
    .await?;
    for (key, due, status) in [
        ("b1", "2000-01-01", "open"),
        ("b2", "2400-01-01", "closed"),
        ("b3", "2999-01-01", "open"),
    ] {
        let at = format!("{due}T00:00:00Z");
        l.upsert(
            "bill",
            key,
            json!({"due": due, "due_at": at, "status": status}),
        )
        .await?;
    }
    Ok(())
}

async fn a_set_decided_at_the_read_judges_at_the_reads_clock(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let start = s.head().await.must("the head")?;
    bills(&l).await?;
    l.declare_as(at_read("overdue", "bill", "due < today()"), "overdue")
        .await?;
    l.declare_as(at_read("lapsed", "bill", "late = true"), "lapsed")
        .await?;
    l.declare_as(object_set_on("open", "bill", "status = 'open'"), "open")
        .await?;
    l.upsert(
        "bill",
        "b4",
        json!({"due": "2000-01-01", "due_at": "2000-01-01T00:00:00Z"}),
    )
    .await?;
    let kept = moved(s, start, None).await?;
    let open = [moves("open", &["b1", "b3"], true)].concat();
    ensure!(
        kept == open,
        "only the kept set `open` moves a membership; the log moves {kept:?}"
    );
    let valid: Timestamp = VALID.parse().map_err(|_| Failure::new("an instant"))?;
    for (when, at, due) in [
        ("at the head", None, "b1 b4"),
        ("valid at 2500", Some(valid), "b1 b2 b4"),
    ] {
        for (set, filter) in [("overdue", "due < today()"), ("lapsed", "late = true")] {
            let named = keys_at(&l, &format!("in_set('{set}')"), at).await?;
            let plain = keys_at(&l, filter, at).await?;
            ensure!(
                named == due && plain == due,
                "{when}, `in_set('{set}')` answers what `{filter}` does at the read's clock, \
                 {due}: it answered {named}, and the `find` {plain}"
            );
        }
    }
    let described = l.describe().await?.object_sets;
    let said = |n: &str| described.iter().find(|d| d.name == n).map(|d| d.decided_at);
    ensure!(
        said("overdue") == Some(DecidedAt::Read) && said("open") == Some(DecidedAt::Write),
        "`describe` shows `overdue` decided at the read and `open` at the write: {described:?}"
    );
    Ok(())
}

async fn a_set_decided_at_the_read_steps_across_no_reference(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let partner = type_decl("partner", vec![field("name", StorageKind::Text)]);
    l.declare(partner).await?;
    let mut bill = type_decl("bill", vec![field("meta", StorageKind::Json)]);
    bill.fields.push(field("owner", StorageKind::Text));
    bill.fields
        .push(crate::conformance::fixture::reference("partner", "partner"));
    l.declare(bill).await?;
    l.declare(count_view("load", "bill", "owner")).await?;
    l.set("view_kept.load", json!(true)).await?;
    let before = s.head().await.must("the head")?;
    for (of, filter, case) in [
        (
            "bill",
            "partner.name = 'x'",
            json!({"code": "set_at_read_steps", "set": "s",
                                             "path": "partner.name"}),
        ),
        (
            "bill",
            "exists(partner)",
            json!({"code": "set_at_read_steps", "set": "s",
                                          "path": "partner"}),
        ),
        (
            "load",
            "n > 1",
            json!({"code": "set_at_read_of_view", "set": "s",
                                    "view": "load"}),
        ),
    ] {
        let declared = l.cat.declare(&l.own, at_read("s", of, filter)).await;
        refused_like(&format!("`{filter}` decided at the read"), declared, &case)?;
    }
    head_unmoved(s, before, "each refused set").await?;
    l.declare_as(at_read("tagged", "bill", "meta.a = 1"), "tagged")
        .await?;
    Ok(())
}

async fn what_a_write_decides_names_no_set_decided_at_the_read(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    bills(&l).await?;
    l.declare_as(at_read("overdue", "bill", "due < today()"), "overdue")
        .await?;
    l.declare_as(object_set_on("open", "bill", "status = 'open'"), "open")
        .await?;
    l.declare_as(object_set_on("wide", "bill", "in_set('open')"), "wide")
        .await?;
    let before = s.head().await.must("the head")?;
    let named = |kind: &str, name: &str| json!({"code": "set_at_read_named", "set": "overdue", "kind": kind, "name": name});
    let mut verb = write_verb("close", "bill", "status", "'closed'");
    if let Declaration::Verb(v) = &mut verb {
        v.requires = Some("in_set('overdue')".into());
    }
    let view = json!({"declare": "type", "name": "late_load",
        "fields": [{"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "bill", "where": "in_set('overdue')",
                 "measures": [{"name": "n", "expr": "count()"}]}});
    for (what, d, case) in [
        (
            "a kept set",
            object_set_on("late", "bill", "in_set('overdue')"),
            named("object_set", "late"),
        ),
        (
            "a trigger",
            trigger_on("t", "bill", Some("in_set('overdue')")),
            named("trigger", "t"),
        ),
        ("a verb", verb, named("verb", "close")),
    ] {
        refused_like(what, l.cat.declare(&l.own, d).await, &case)?;
    }
    let view: Declaration = serde_json::from_value(view)
        .map_err(|e| Failure::new(format!("the view does not read: {e}")))?;
    let declared = l.cat.declare(&l.own, view).await;
    refused_like("a view", declared, &named("type", "late_load"))?;
    let again = l
        .cat
        .declare(&l.own, at_read("open", "bill", "status = 'open'"));
    let case = json!({"code": "set_at_read_named", "set": "open", "kind": "object_set",
                      "name": "wide"});
    refused_like("`open` declared again at the read", again.await, &case)?;
    head_unmoved(s, before, "each refused declaration").await?;
    l.declare_as(
        at_read("both", "bill", "in_set('overdue') and in_set('open')"),
        "both",
    )
    .await?;
    Ok(())
}

async fn a_set_declared_again_between_kept_and_decided_at_the_read(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    bills(&l).await?;
    l.declare_as(object_set_on("open", "bill", "status = 'open'"), "open")
        .await?;
    let at = s.head().await.must("the head")?;
    let declared = l
        .declare_as(at_read("open", "bill", "status = 'open'"), "at the read")
        .await?
        .position();
    let ended = moved(s, at, Some(declared)).await?;
    ensure!(
        ended == moves("open", &["b1", "b3"], false),
        "`open` declared again at the read ends `b1` and `b3`, caused by its `declared`; the \
         log moves {ended:?}"
    );
    s.rebuild().await.must("rebuilding the projection")?;
    let at = s.head().await.must("the head")?;
    l.upsert("bill", "b2", json!({"status": "open"})).await?;
    l.upsert("bill", "b3", json!({"status": "closed"})).await?;
    let none = moved(s, at, None).await?;
    ensure!(
        none.is_empty() && keys_at(&l, "in_set('open')", None).await? == "b1 b2",
        "after a rebuild a write moves no membership of `open`, and a read's `in_set` answers \
         `b1 b2`; the log moves {none:?}"
    );
    let declared = l
        .declare_as(object_set_on("open", "bill", "status = 'open'"), "kept")
        .await?
        .position();
    let joined = moved(s, declared, Some(declared)).await?;
    ensure!(
        joined == moves("open", &["b1", "b2"], true),
        "`open` declared again as kept decides `b1` and `b2`, caused by its `declared`; the \
         log moves {joined:?}"
    );
    s.rebuild().await.must("rebuilding the projection")?;
    let at = s.head().await.must("the head")?;
    l.upsert("bill", "b1", json!({"status": "closed"})).await?;
    let left = moved(s, at, None).await?;
    ensure!(
        left == moves("open", &["b1"], false),
        "after a rebuild `b1` closed leaves `open`; the log moves {left:?}"
    );
    l.declare_as(at_read("overdue", "bill", "due < today()"), "overdue")
        .await?;
    let case = json!({"code": "set_reads_clock", "set": "overdue", "call": "today()"});
    let kept = l
        .cat
        .declare(&l.own, object_set_on("overdue", "bill", "due < today()"));
    refused_like("`overdue` declared again as kept", kept.await, &case)
}
