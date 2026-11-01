//! A trigger on a view the lake keeps: it fires at every write that puts a
//! group's row, in that write's own append, where its `when` holds over the
//! row after (`docs/decisions/066-a-set-holds-a-kept-view.md`).

use lakelands_api::event::ChangeKind;

use crate::conformance::prelude::*;

check!(
    KEPT_TRIGGER = a_trigger_on_a_kept_view_fires_where_a_groups_row_is_put,
    "049 and 004, extended (lakelands#1185): a trigger on a kept view fires at a write that puts a group's row, the group arriving or its fields moving, where its `when` holds over the row after, in that write's own append and caused by its event, naming the group's key, the write's kind and whether the group arrived; `previous` reads the row before, `created()` holds where the group arrives, again after it ended; a write that moves no field of the row fires nothing, nor does a setting a set of the view reads, which decides every group again; a trigger whose `on_changes` names `upserted` alone stays silent at a retirement; and neither does a write that ends the group"
);

check!(
    KEPT_TRIGGER_DEEP = a_trigger_on_a_view_over_a_kept_view_fires_and_drops_as_at_a_record,
    "049 and 021, extended (lakelands#1185): a trigger on a kept view over a kept view fires where the outer group's row is put, at the write of the inner view's source; a strict guard on a view fails where a field of the row has no value while its lax twin fires; a trigger past its `max_depth` drops at a reaction's write; a setting a measure names puts every group's row again, and fires where a row moved"
);

check!(
    KEPT_TRIGGER_REFUSED = what_would_unkeep_a_trigger_on_a_view_is_refused,
    "049 and 055, extended (lakelands#1185): a trigger on a view the lake does not keep is refused `watches_view`, saying to set `view_kept.<view>` first; while a trigger watches a kept view, the setting written `false` is refused `view_kept_for_set` naming the trigger, and a migration of a source that moves a field the view reads `view_source_stepped`; each appends nothing; such a trigger is a reader's only where nothing of a source is held from it: `describe` names it to the owner, and not to a reader without `restricted` nor to one with an `only` on the source, and a writer with that `only` is told no firing at its own write, with a held invoice in the group and without"
);

check!(
    KEPT_TRIGGER_BOUND = what_a_trigger_on_a_kept_view_fires_is_held_to_the_ceiling,
    "004 and 055, extended (lakelands#1185): the firings a write sets off on a kept view count toward the lake's `max_set_changes`; past it the write is refused `too_large` naming the trigger to a writer shown it and nothing to another, appending nothing",
    given
);

check!(
    KEPT_TRIGGER_INEXACT = a_trigger_on_an_inexact_group_fires_as_on_a_group_that_arrived,
    "049, extended (lakelands#1185): where the fold has flagged a group as not exact, or a view it reads through holds such a group, a trigger on the view reads no row before: `created()` holds and `previous` has no value, and the firing does not say `created`"
);

check!(
    KEPT_TRIGGER_TERM = a_trigger_on_a_view_grouped_by_a_term_fires_as_on_its_twin,
    "049 and 066, extended (lakelands#1666): a trigger on a kept view grouped by a meter and a `date_bucket` term over the hour of a source that holds no hour field answers at every point as its twin on a view grouped by an equal derived field: declared before the view is kept it is refused `watches_view`; the setting written `false` is refused `view_kept_for_set` and a migration of a field the view reads `view_source_stepped`, each naming the trigger and appending nothing; ten readings of one meter's hour in one ingest are judged record by record, and a guard reading the write's clock, the bound of the hour's day, `created()` and `previous` fires once, at the reading whose sum crosses the bound, naming the meter and the hour, under its lax and its strict form; a new hour whose first reading is past the bound fires under the lax guard and not the strict one, which reads no `previous`; readings of a year before fire nothing, their hour outside the guard's window; a group whose sum the fold cannot keep exactly fires as one that arrived; and past a ceiling of one a delivery that would fire twice is refused `too_large` naming the trigger and appends nothing",
    given
);

/// The invoices, the view `spent` of the open ones by customer, kept, and
/// its triggers: `any`, `authored` by an upsert alone, `big` past 100,
/// `rose` where the total grew, and `fresh` where the group arrives.
async fn lake(l: &Lake) -> Held {
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": [
        field("customer", "text"), field("amount", "decimal"), field("status", "text"),
        field("note", "text")]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "spent", "key": ["customer"],
        "fields": [field("customer", "text"), field("n", "number"), field("total", "decimal")],
        "view": {"source": "invoice", "where": "status = 'open'", "group_by": ["customer"],
                 "measures": [{"name": "n", "expr": "count()"}, {"name": "total", "expr": "sum(amount)"}]}}))
        .await?;
    l.set("view_kept.spent", json!(true)).await?;
    for (name, when, on) in [
        ("any", Value::Null, Value::Null),
        ("authored", Value::Null, json!(["upserted"])),
        ("big", json!("total > 100"), Value::Null),
        ("rose", json!("total > previous(total)"), Value::Null),
        ("fresh", json!("created()"), Value::Null),
    ] {
        l.declare_json(
            json!({"declare": "trigger", "name": name, "on": "spent", "when": when,
            "on_changes": on}),
        )
        .await?;
    }
    Ok(())
}

fn invoice(customer: &str, amount: &str, status: &str) -> Value {
    json!({"customer": customer, "amount": amount, "status": status})
}

/// Every firing appended from `at`, `(trigger, key, change, created)`, in
/// the log's order, each caused by the event at `at`.
async fn fired(
    s: &dyn LakeStore,
    at: Position,
) -> Result<Vec<(String, String, ChangeKind, bool)>, Failure> {
    let mut out = Vec::new();
    for e in events_from(s, at).await? {
        if let EventBody::TriggerFired {
            trigger,
            key,
            change,
            created,
            ..
        } = e.body
        {
            ensure!(
                e.cause == Some(at),
                "`{trigger}` fired at {} is caused by {:?}, not the write at {at}",
                e.position,
                e.cause
            );
            let (change, created) = change.zip(created).ok_or_else(|| {
                Failure::new(format!("`{trigger}` fired without its kind of change"))
            })?;
            out.push((trigger.to_string(), key.to_string(), change, created));
        }
    }
    Ok(out)
}

async fn a_trigger_on_a_kept_view_fires_where_a_groups_row_is_put(store: Store) -> Held {
    use ChangeKind::{Retired, Upserted};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    let row = |t: &str, change, created| (t.to_owned(), "c1".to_owned(), change, created);
    let step = async |what: &str, at: Position, want: Vec<(String, String, ChangeKind, bool)>| {
        let got = fired(s, at).await?;
        ensure!(got == want, "{what} fires {want:?}; it fired {got:?}");
        Ok::<_, Failure>(())
    };
    let at = l
        .upsert("invoice", "i1", invoice("c1", "50", "open"))
        .await?;
    let want = vec![
        row("any", Upserted, true),
        row("authored", Upserted, true),
        row("fresh", Upserted, true),
    ];
    step("`i1` of 50 making `c1`", at.position(), want).await?;
    let at = l
        .upsert("invoice", "i2", invoice("c1", "70", "open"))
        .await?;
    let want = vec![
        row("any", Upserted, false),
        row("authored", Upserted, false),
        row("big", Upserted, false),
        row("rose", Upserted, false),
    ];
    step("`i2` of 70 taking `c1` to 120", at.position(), want).await?;
    let at = l.upsert("invoice", "i2", json!({"note": "late"})).await?;
    step(
        "`i2` noted, a field the view reads not",
        at.position(),
        vec![],
    )
    .await?;
    // A setting a set of the view reads decides every group again, and
    // puts no row.
    l.declare_json(
        json!({"declare": "object_set", "name": "over", "of": "spent",
        "where": "total > setting('limit')"}),
    )
    .await?;
    let at = l.set("limit", json!(10)).await?;
    step("`limit` written", at.position(), vec![]).await?;
    let at = l
        .upsert("invoice", "i3", invoice("c1", "10", "closed"))
        .await?;
    step("a closed `i3`, outside the view", at.position(), vec![]).await?;
    let at = l.retire("invoice", "i2").await?;
    let want = vec![row("any", Retired, false)];
    step("`i2` retired, `c1` back to 50", at.position(), want).await?;
    let at = l.retire("invoice", "i1").await?;
    step("`i1` retired, ending `c1`", at.position(), vec![]).await?;
    let at = l
        .upsert("invoice", "i1", invoice("c1", "50", "open"))
        .await?;
    let want = vec![
        row("any", Upserted, true),
        row("authored", Upserted, true),
        row("fresh", Upserted, true),
    ];
    step("`i1` revived, making `c1` again", at.position(), want).await
}

async fn a_trigger_on_a_view_over_a_kept_view_fires_and_drops_as_at_a_record(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": [
        field("customer", "text"), field("amount", "decimal"), field("score", "number")]}))
        .await?;
    l.set("rate", json!(1)).await?;
    l.declare_json(
        json!({"declare": "type", "name": "spent", "key": ["customer"],
        "fields": [field("customer", "text"), field("total", "decimal"), field("top", "number")],
        "view": {"source": "invoice", "group_by": ["customer"],
                 "measures": [{"name": "total", "expr": "sum(amount) * setting('rate')"},
                              {"name": "top", "expr": "max(score)"}]}}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "book", "fields": [field("owed", "decimal")],
        "view": {"source": "spent", "group_by": [], "measures": [{"name": "owed", "expr": "sum(total)"}]}}))
        .await?;
    for view in ["spent", "book"] {
        l.set(&format!("view_kept.{view}"), json!(true)).await?;
    }
    for (name, on, when, strict, depth) in [
        ("deep", "book", "owed > 100", false, json!(1)),
        ("lax", "spent", "not (top = 1)", false, Value::Null),
        ("strict", "spent", "not (top = 1)", true, Value::Null),
    ] {
        l.declare_json(
            json!({"declare": "trigger", "name": name, "on": on, "when": when,
            "when_strict": strict, "max_depth": depth}),
        )
        .await?;
    }
    // What a write set off: each trigger with the key, and whether it dropped.
    let set_off = async |at: Position| -> Result<Vec<(String, String, bool)>, Failure> {
        let mut out = Vec::new();
        for e in events_from(s, at)
            .await?
            .into_iter()
            .filter(|e| e.cause == Some(at))
        {
            match e.body {
                EventBody::TriggerFired { trigger, key, .. } => {
                    out.push((trigger.to_string(), key.to_string(), false))
                }
                EventBody::TriggerDropped { trigger, key, .. } => {
                    out.push((trigger.to_string(), key.to_string(), true))
                }
                _ => {}
            }
        }
        out.sort();
        Ok(out)
    };
    let row = |t: &str, k: &str, dropped| (t.to_owned(), k.to_owned(), dropped);
    let fresh = json!({"customer": "c1", "amount": "150"});
    let at = l.upsert("invoice", "i1", fresh).await?.position();
    let want = vec![row("deep", "all", false), row("lax", "c1", false)];
    let got = set_off(at).await?;
    ensure!(
        got == want,
        "`i1` of 150 fires {want:?}, `strict` not over a row without `top`; it set off {got:?}"
    );
    let cat = s.catalog(crate::conformance::fixture::reactive(1, None));
    let more = json!({"customer": "c1", "amount": "10"});
    let wrote =
        crate::conformance::fixture::try_upsert(cat.as_ref(), &l.own, "invoice", "i2", more);
    let at = wrote.await.must("a reaction at depth 1")?.position();
    let want = vec![row("deep", "all", true), row("lax", "c1", false)];
    let got = set_off(at).await?;
    ensure!(
        got == want,
        "`i2` written at depth 1 drops `deep` (max_depth 1) and fires `lax`; it set off {got:?}"
    );
    let at = l.set("rate", json!(2)).await?.position();
    let want = vec![row("deep", "all", false), row("lax", "c1", false)];
    let got = set_off(at).await?;
    ensure!(
        got == want,
        "`rate` written 2 puts `c1` and `all` again and fires {want:?}; it set off {got:?}"
    );
    Ok(())
}

async fn what_would_unkeep_a_trigger_on_a_view_is_refused(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    l.upsert("invoice", "i1", invoice("c1", "50", "open"))
        .await?;
    let own = &l.own;
    let declare = |d: Value| {
        l.cat
            .declare(own, serde_json::from_value(d).expect("a declaration"))
    };
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let twin = json!({"declare": "type", "name": "spent_twin", "key": ["customer"],
        "fields": [field("customer", "text"), field("total", "decimal")],
        "view": {"source": "invoice", "group_by": ["customer"],
                 "measures": [{"name": "total", "expr": "sum(amount)"}]}});
    l.declare_json(twin).await?;
    let before = s.head().await.must("the head")?;
    let r = declare(json!({"declare": "trigger", "name": "loose", "on": "spent_twin"})).await;
    let want = json!({"code": "watches_view", "type": "spent_twin",
        "at": [{"code": "trigger", "name": "loose"}]});
    refused_like("a trigger on a view no setting keeps", r, &want)?;
    let (setting, key) = (
        TypeName::from(lakelands_api::seam::SETTING_TYPE),
        "view_kept.spent".into(),
    );
    let r = l
        .cat
        .upsert(
            own,
            &setting,
            &key,
            patch(json!({"value": false})),
            WriteOpts::default(),
        )
        .await;
    let want = json!({"code": "view_kept_for_set", "type": "spent",
        "at": [{"code": "trigger", "name": "any"}]});
    refused_like("`view_kept.spent` written false", r, &want)?;
    let to: lakelands_api::ontology::TypeDecl = serde_json::from_value(json!({"name": "invoice",
        "fields": [field("customer", "text"), field("amount", "number"), field("status", "text"),
            field("note", "text")]}))
    .expect("a type");
    let r = l.cat.migrate(own, migration(to)).await;
    let want = json!({"code": "view_source_stepped", "type": "spent", "source": "invoice",
        "at": [{"code": "trigger", "name": "any"}]});
    refused_like("`invoice.amount` migrated to a number", r, &want)?;
    head_unmoved(s, before, "the refused writes").await?;
    // Decided for no reader, a firing tells what every source adds.
    let plain = principal(
        s,
        PrincipalKind::Agent,
        "plain",
        Grants {
            default: Access::Read,
            ..Grants::default()
        },
    );
    // One holding `restricted` and an `only` on the source is held the
    // other customers' invoices: the trigger is not its either.
    let mut only = principal(
        s,
        PrincipalKind::Agent,
        "only",
        Grants {
            default: Access::Write,
            restricted: true,
            ..Grants::default()
        },
    );
    let grants = lakelands_api::principal::TypeGrants {
        only: [("customer".into(), json!("c2"))].into(),
        ..Default::default()
    };
    only.grants.types.insert("invoice".into(), grants);
    for (who, shown) in [(own, true), (&plain, false), (&only, false)] {
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describe")?.data;
        let named = d.triggers.iter().any(|t| t.on == "spent");
        ensure!(
            named == shown,
            "`describe` names the triggers on `spent` to `{}`: {shown}; it named them: {named}",
            who.id
        );
    }
    // A write of its own invoice is answered alike whether a held invoice
    // of the same customer is in the group or not: it names no firing.
    let (ty, mut answers) = (TypeName::from("invoice"), Vec::new());
    for (key, held) in [("i5", false), ("i6", true)] {
        if held {
            l.upsert("invoice", "i9", invoice("c2", "40", "open"))
                .await?;
            crate::conformance::fixture::restrict(s, "invoice", "i9", "a hold").await?;
        }
        let (open, key) = (patch(invoice("c2", "5", "open")), key.into());
        let wrote = l.cat.upsert(&only, &ty, &key, open, WriteOpts::default());
        let fired = match wrote.await.must(&format!("`only` writing `{key}`"))? {
            lakelands_api::Outcome::Applied { set_off, .. } => set_off.fired,
            other => return Err(Failure::new(format!("`{key}` answered {other:?}"))),
        };
        answers.push(fired);
    }
    ensure!(
        answers.iter().all(|f| f.is_empty()),
        "`only` is told no firing on `spent`, with a held invoice of `c2` and without; it was told {answers:?}"
    );
    Ok(())
}

async fn what_a_trigger_on_a_kept_view_fires_is_held_to_the_ceiling(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    let store = at_one_set_change(&given, store, "opening at a ceiling of one").await?;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let before = s.head().await.must("the head")?;
    let (ty, key) = (TypeName::from("invoice"), "i1".into());
    let blind = principal(
        s,
        PrincipalKind::Agent,
        "blind",
        Grants {
            default: Access::Write,
            ..Grants::default()
        },
    );
    // A group made fires `any` and `fresh`: two, past a ceiling of one.
    for (who, named) in [(&l.own, true), (&blind, false)] {
        let open = patch(invoice("c1", "50", "open"));
        let r = l
            .cat
            .upsert(who, &ty, &key, open, WriteOpts::default())
            .await;
        let refused = match &r {
            Err(lakelands_api::Error::Refused {
                field,
                reason: Refusal::TooLarge { max: 1 },
            }) => Some(field.is_some()),
            _ => None,
        };
        ensure!(
            refused == Some(named),
            "`i1` making `c1` under a ceiling of one is refused too_large, naming a trigger to `{}`: {named}; it answered {r:?}",
            who.id
        );
    }
    head_unmoved(s, before, "the refused writes").await
}

/// A group whose sum the fold cannot keep exactly has no row before, and
/// neither has the group of a view over it: `created()` holds, `previous`
/// has no value, and the firing does not say `created`.
async fn a_trigger_on_an_inexact_group_fires_as_on_a_group_that_arrived(store: Store) -> Held {
    use ChangeKind::Upserted;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    l.declare_json(json!({"declare": "type", "name": "invoice",
        "fields": [field("customer", "text"), field("amount", "decimal")]}))
        .await?;
    l.declare_json(
        json!({"declare": "type", "name": "spent", "key": ["customer"],
        "fields": [field("customer", "text"), field("total", "decimal")],
        "view": {"source": "invoice", "group_by": ["customer"],
                 "measures": [{"name": "total", "expr": "sum(amount)"}]}}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "book", "fields": [field("owed", "decimal")],
        "view": {"source": "spent", "group_by": [], "measures": [{"name": "owed", "expr": "sum(total)"}]}}))
        .await?;
    for view in ["spent", "book"] {
        l.set(&format!("view_kept.{view}"), json!(true)).await?;
    }
    for (name, on, when) in [
        ("fresh", "spent", "created()"),
        ("rose", "spent", "total > previous(total)"),
        ("book_fresh", "book", "created()"),
        ("book_rose", "book", "owed > previous(owed)"),
    ] {
        l.declare_json(json!({"declare": "trigger", "name": name, "on": on, "when": when}))
            .await?;
    }
    let (big, back) = (
        "70000000000000000000000000000",
        "-69999999999999999999999999890",
    );
    for (key, amount) in [("i1", big), ("i2", back)] {
        let fields = json!({"customer": "c1", "amount": amount});
        l.upsert("invoice", key, fields).await?;
    }
    // `c1` sums to 110 but not exactly: a read computes it from the members.
    let at = l
        .upsert("invoice", "i3", json!({"customer": "c1", "amount": "1"}))
        .await?;
    let got = fired(s, at.position()).await?;
    let row = |t: &str, k: &str| (t.to_owned(), k.to_owned(), Upserted, false);
    let want = vec![row("fresh", "c1")];
    ensure!(
        got == want,
        "`i3` into the inexact `c1` fires {want:?}, with no row before and no `created`; it fired {got:?}"
    );
    // The view over it reads a group that is not exact, so it has no row
    // before either, where an exact group of the view below moves.
    let at = l
        .upsert("invoice", "i4", json!({"customer": "c2", "amount": "5"}))
        .await?;
    let mut got = fired(s, at.position()).await?;
    got.sort();
    let want = vec![
        row("book_fresh", "all"),
        ("fresh".to_owned(), "c2".to_owned(), Upserted, true),
    ];
    ensure!(
        got == want,
        "`i4` making `c2` fires {want:?}, `book` having no row before; it fired {got:?}"
    );
    Ok(())
}

/// The origin the readings' hours are counted from.
const ORIGIN: &str = "2026-01-01T00:00:00Z";

/// The two lanes: a source without an hour and its view grouped by the
/// term, and the twin source holding the hour derived and its view
/// grouped by that field; the suffix of the twin's names.
const LANES: [(&str, &str, &str); 2] = [
    ("reading", "reading_1h", ""),
    ("reading_d", "reading_1h_twin", "_twin"),
];

/// The source of a lane, its litres a `litres` kind of a decimal or not.
fn reading_type(ty: &str, litres: &str) -> Value {
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}, "authority": "source"});
    let mut fields = vec![
        field("meter", "text"),
        field("at", "date_time"),
        field("litres", litres),
    ];
    if ty == "reading_d" {
        fields.push(json!({"name": "hour", "type": {"kind": "date_time"}, "authority": "derived",
            "derived": format!("date_bucket(at, 1h, '{ORIGIN}')"), "depends_on": ["at"]}));
    }
    json!({"name": ty, "fields": fields})
}

/// What a delivery set off on a lane: each firing's trigger, its twin
/// suffix dropped, the group's key, whether it says `created`, and the
/// key of the reading whose event caused it, sorted.
async fn set_off_by(
    s: &dyn LakeStore,
    from: Position,
) -> Result<Vec<(String, String, bool, String)>, Failure> {
    let (mut by, mut out) = (BTreeMap::new(), Vec::new());
    for e in events_from(s, from).await? {
        match e.body {
            EventBody::RecordIngested { key, .. } => {
                by.insert(e.position, key.to_string());
            }
            EventBody::TriggerFired {
                trigger,
                key,
                created,
                ..
            } => {
                let at = e.cause.and_then(|c| by.get(&c)).cloned();
                let at = at.ok_or_else(|| {
                    Failure::new(format!("`{trigger}` fired, caused by no reading of the delivery"))
                })?;
                let trigger = trigger.to_string().replace("_twin", "");
                out.push((trigger, key.to_string(), created.unwrap_or_default(), at));
            }
            _ => {}
        }
    }
    out.sort();
    Ok(out)
}

async fn a_trigger_on_a_view_grouped_by_a_term_fires_as_on_its_twin(
    store: Store,
    given: Given,
) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    super::settings::declared(&l, "litre_bound", "decimal", json!("100"))
        .await?;
    let bucket = format!("date_bucket(at, 1h, '{ORIGIN}')");
    for (source, view, _) in LANES {
        let mut ty = reading_type(source, "decimal");
        ty["declare"] = json!("type");
        l.declare_json(ty).await?;
        let hour = match source {
            "reading" => json!({"name": "hour", "expr": bucket}),
            _ => json!("hour"),
        };
        l.declare_json(json!({"declare": "type", "name": view, "key": ["meter", "hour"],
            "fields": [field("meter", "text"), field("hour", "date_time"), field("litres_sum", "decimal")],
            "view": {"source": source, "group_by": ["meter", hour],
                     "measures": [{"name": "litres_sum", "expr": "sum(litres)"}]}}))
            .await?;
    }
    let below = "setting('litre_bound', hour)";
    let when = format!(
        "hour >= date_bucket(now(), 1h, '{ORIGIN}') - 1h and litres_sum > {below} \
         and (created() or previous(litres_sum) <= {below})"
    );
    let triggers = |view: &str, end: &str| {
        [("crossing", false), ("strict", true)].map(|(name, strict)| {
            json!({"declare": "trigger", "name": format!("{name}{end}"), "on": view,
                "when": when, "when_strict": strict})
        })
    };
    let before = s.head().await.must("the head")?;
    for (_, view, end) in LANES {
        let [t, _] = triggers(view, end);
        let r = l.cat.declare(&l.own, serde_json::from_value(t).expect("a trigger")).await;
        let want = json!({"code": "watches_view", "type": view,
            "at": [{"code": "trigger", "name": format!("crossing{end}")}]});
        refused_like(&format!("a trigger on `{view}`, not kept"), r, &want)?;
    }
    head_unmoved(s, before, "the refused triggers").await?;
    for (_, view, end) in LANES {
        l.set(&format!("view_kept.{view}"), json!(true)).await?;
        for t in triggers(view, end) {
            l.declare_json(t).await?;
        }
    }
    // Each delivery goes to both lanes; what each set off is the same.
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as i64);
    let hour = 3_600_000_000;
    let now = micros - micros.rem_euclid(hour);
    let reading = |meter: &str, at: i64, litres: &str| {
        let at = Timestamp(at).to_string();
        json!({"meter": meter, "at": at, "litres": litres})
    };
    let deliver = async |l: &Lake, what: &str, readings: &[(String, Value)]| {
        let mut lanes = Vec::new();
        for (source, _, _) in LANES {
            let records = (readings.iter())
                .map(|(k, f)| crate::conformance::fixture::source_record(source, k, f.clone()))
                .collect();
            let from = s.head().await.must("the head")?.next();
            l.ingest("meter", records)
                .await
                .must(&format!("{what} into `{source}`"))?;
            lanes.push(set_off_by(s, from).await?);
        }
        let [term, twin] = <[_; 2]>::try_from(lanes).expect("two lanes");
        ensure!(
            term == twin,
            "{what} sets off on the view grouped by the term what it does on its twin, \
             {twin:?}; it set off {term:?}"
        );
        Ok::<_, Failure>(term)
    };
    let named = |key: &str, meter: &str, at: i64| {
        let parts = lakelands_api::key::key_components(&key.into());
        let hour = parts.get(1).and_then(|h| Timestamp::parse(h).ok());
        parts.first().map(String::as_str) == Some(meter) && hour == Some(Timestamp(at))
    };
    let ten: Vec<_> = (1..=10)
        .map(|i| (format!("r{i}"), reading("m1", now + i * 1_000_000, "15")))
        .collect();
    let got = deliver(&l, "ten readings of `m1`'s hour", &ten).await?;
    let fired: Vec<_> = got.iter().map(|(t, _, c, at)| (t.as_str(), *c, at.as_str())).collect();
    ensure!(
        fired == [("crossing", false, "r7"), ("strict", false, "r7")]
            && got.iter().all(|(_, k, ..)| named(k, "m1", now)),
        "ten readings of 15 litres into `m1`'s hour fire `crossing` and `strict` once, at `r7`, \
         whose 105 crosses the bound of 100, naming `m1` and the hour; they fired {got:?}"
    );
    let next = [("n1".to_owned(), reading("m1", now + hour, "150"))];
    let got = deliver(&l, "a new hour's first reading past the bound", &next).await?;
    let fired: Vec<_> = got.iter().map(|(t, _, c, at)| (t.as_str(), *c, at.as_str())).collect();
    ensure!(
        fired == [("crossing", true, "n1")] && named(&got[0].1, "m1", now + hour),
        "a new hour of `m1` whose first reading is past the bound fires `crossing`, the group \
         arriving, and not `strict`, which reads no `previous`; it fired {got:?}"
    );
    let year = now - 366 * 24 * hour;
    let old: Vec<_> = (1..=10)
        .map(|i| (format!("y{i}"), reading("m1", year + i * 1_000_000, "15")))
        .collect();
    let got = deliver(&l, "last year's readings", &old).await?;
    ensure!(
        got.is_empty(),
        "readings of an hour a year before cross the bound outside the window, and fire nothing; they fired {got:?}"
    );
    // A group whose sum the fold cannot keep exactly has no row before.
    let (big, back) = (
        "70000000000000000000000000000",
        "-69999999999999999999999999890",
    );
    let wide = [
        ("w1".to_owned(), reading("m9", now, big)),
        ("w2".to_owned(), reading("m9", now + 1, back)),
    ];
    deliver(&l, "two readings `m9` sums inexactly", &wide).await?;
    let one = [("w3".to_owned(), reading("m9", now + 2, "1"))];
    let got = deliver(&l, "a reading into the inexact `m9`", &one).await?;
    let fired: Vec<_> = got.iter().map(|(t, _, c, at)| (t.as_str(), *c, at.as_str())).collect();
    ensure!(
        fired == [("crossing", false, "w3")],
        "a reading into the inexact `m9` fires `crossing` as at a group that arrived, without \
         saying `created`, and not `strict`, which reads no `previous`; it fired {got:?}"
    );
    // What would unkeep the views is refused, naming the trigger.
    let before = s.head().await.must("the head")?;
    let setting = TypeName::from(lakelands_api::seam::SETTING_TYPE);
    for (source, view, end) in LANES {
        let key = format!("view_kept.{view}").into();
        let off = patch(json!({"value": false}));
        let r = (l.cat).upsert(&l.own, &setting, &key, off, WriteOpts::default()).await;
        let want = json!({"code": "view_kept_for_set", "type": view,
            "at": [{"code": "trigger", "name": format!("crossing{end}")}]});
        refused_like(&format!("`view_kept.{view}` written false"), r, &want)?;
        let to = serde_json::from_value(reading_type(source, "number")).expect("a type");
        let r = l.cat.migrate(&l.own, migration(to)).await;
        let want = json!({"code": "view_source_stepped", "type": view, "source": source,
            "at": [{"code": "trigger", "name": format!("crossing{end}")}]});
        refused_like(&format!("`{source}.litres` migrated to a number"), r, &want)?;
    }
    head_unmoved(s, before, "the refused writes").await?;
    // Two new hours past the bound fire twice, past a ceiling of one.
    let store = at_one_set_change(&given, store, "opening at a ceiling of one").await?;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let before = s.head().await.must("the head")?;
    let two = [("m2", "t1"), ("m3", "t2")].map(|(m, k)| (k.to_owned(), reading(m, now, "150")));
    let mut refused = Vec::new();
    for (source, _, end) in LANES {
        let records = two.iter().map(|(k, f)| crate::conformance::fixture::source_record(source, k, f.clone())).collect();
        let r = l.ingest("meter", records).await;
        let named = match &r {
            Err(lakelands_api::Error::Refused {
                field: Some(field),
                reason: Refusal::TooLarge { max: 1 },
            }) => Some(field.to_string().replace(end, "")),
            _ => None,
        };
        ensure!(
            named.is_some(),
            "two new hours past the bound into `{source}` under a ceiling of one are refused \
             too_large naming the trigger; it answered {r:?}"
        );
        refused.push(named);
    }
    ensure!(
        refused[0] == refused[1],
        "the delivery past the ceiling names the trigger on the term's view as on its twin's; \
         they named {refused:?}"
    );
    head_unmoved(s, before, "the deliveries past the ceiling").await
}
