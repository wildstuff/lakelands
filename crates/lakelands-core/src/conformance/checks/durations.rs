//! Arithmetic on durations, and what an expression can never compute
//! (`docs/decisions/017-one-expression-grammar.md` and
//! `028-authoring-is-strict-and-whole.md`, both extended).
//!
//! The lake: `invoice` with `due_on` and `paid_on`. `i1` is paid 64 days
//! and 12 hours after it was due, `open` is not paid. Every number below
//! is counted by hand from that.

use std::sync::Arc;

use lakelands_api::expr::Condition;
use lakelands_api::ontology::StorageKind::{self, *};
use lakelands_api::principal::Access;
use lakelands_api::query::Projection;
use lakelands_api::read::{AggFunc, Aggregate, GroupKey};
use lakelands_api::{Find, Found, LakeStore, TypeName, Value};
use rust_decimal::Decimal;
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, dec, decimal, derived, field, field_access, find_where, invalid_naming,
    log_events, migration, no_such_field, object_set_on, pattern, refused_on, trigger_on,
    type_decl, value, write_verb,
};

check!(
    /// The days between two instants.
    DAYS = a_duration_divided_by_a_unit_is_a_number,
    "017, extended: a duration divided by a duration is an exact number, a duration times or divided by a number is a duration, and floor(n) is the whole number below; so (paid_on - due_on) / 1d answers the days between two instants as a group key, in a where and in a derived number field, the same on every engine; log format §6: the declaration that calls floor is written in format version 11, as is every event after it"
);

check!(
    /// A derived field that is never filled.
    LANDS = a_derived_field_its_expression_can_never_fill_is_refused,
    "028, extended: a derived field whose expression is certainly of a kind its field never holds is refused as invalid on that field by declare, add_field and a migration, naming both kinds, and nothing is appended; a text into a number field and an expression whose kind depends on the record are taken"
);

check!(
    /// What can never be evaluated.
    NEVER = what_can_never_be_evaluated_is_refused_and_a_records_own_failure_is_not,
    "028, extended: an operation between two kinds that has no meaning is refused as invalid in a find's where and group key and a pattern's where, and in a trigger's when, an object set's where and a verb's requires and value, naming the operation; a reader kept from a field it names is answered as for a field the type lacks; an evaluation that fails for one record's value refuses nothing: that record does not match, and its key is null"
);

const TYPE: &str = "invoice";

async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let whole = "floor((paid_on - due_on) / 1d)";
    let mut fields = vec![field("due_on", DateTime), field("paid_on", DateTime)];
    fields.extend([field("n", Number), field("code", Text)]);
    fields.push(derived("late", Number, whole, &["due_on", "paid_on"]));
    l.declare(type_decl(TYPE, fields)).await?;
    let paid = json!({"due_on": "2026-08-01T00:00:00Z", "paid_on": "2026-10-04T12:00:00Z",
                      "n": 2, "code": "abc"});
    l.upsert(TYPE, "i1", paid).await?;
    l.upsert(TYPE, "open", json!({"due_on": "2026-08-01T00:00:00Z"}))
        .await?;
    Ok(l)
}

fn keyed(expr: &str) -> Find {
    Find {
        aggregate: Some(Aggregate {
            measures: vec![agg("records", AggFunc::Count, None)],
            group_by: vec![GroupKey::Named {
                name: "k".into(),
                expr: Condition::Text(expr.to_owned()),
            }],
        }),
        ..Find::default()
    }
}

/// The keys `expr` groups the lake's records under, a null key as `None`.
async fn keys(l: &Lake, expr: &str) -> Result<Vec<Option<Decimal>>, Failure> {
    let reply = l.cat.find(&l.own, &TypeName::from(TYPE), keyed(expr)).await;
    let Found::Groups { rows } = reply.must(&format!("grouping by `{expr}`"))?.data else {
        return Err(Failure::new(format!("`{expr}` answered records")));
    };
    // A number is answered as a decimal's text, a duration as a JSON number.
    let key = |v: &Value| decimal(Some(v)).or_else(|| v.as_i64().map(Decimal::from));
    Ok(rows.iter().map(|g| g.by.get("k").and_then(key)).collect())
}

async fn found(l: &Lake, filter: &str) -> Result<Vec<String>, Failure> {
    let rows = l.all_found(TYPE, find_where(filter)).await?;
    Ok(rows.iter().map(|r| r.key.as_str().to_owned()).collect())
}

async fn a_duration_divided_by_a_unit_is_a_number(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    // 64 days and 12 hours are 5 572 800 seconds; a duration is answered in
    // microseconds. The record that is not paid has no key.
    for (expr, want) in [
        ("(paid_on - due_on) / 1d", "64.5"),
        ("floor((paid_on - due_on) / 1d)", "64"),
        ("round((paid_on - due_on) / 1d)", "65"),
        ("(paid_on - due_on) / 12h", "129"),
        ("(paid_on - due_on) * 2 / 1d", "129"),
        ("2 * (paid_on - due_on) / 1d", "129"),
        ("(paid_on - due_on) / 2 / 1d", "32.25"),
        ("(paid_on - due_on) / 2", "2786400000000"),
    ] {
        let got = keys(&l, expr).await?;
        ensure!(
            got == [Some(dec(want)), None],
            "`{expr}` is {want} for `i1` and null for `open`; as a group key it answered {got:?}"
        );
    }
    for (expr, want) in [("floor(2.5)", "2"), ("floor(0 - 2.5)", "-3")] {
        let got = keys(&l, expr).await?;
        let want = [Some(dec(want))];
        ensure!(
            got == want,
            "`{expr}` is {want:?}; as a group key it answered {got:?}"
        );
    }
    for (filter, want) in [
        ("(paid_on - due_on) / 1d > 64", vec!["i1"]),
        ("floor((paid_on - due_on) / 1d) > 64", vec![]),
        ("late = 64", vec!["i1"]),
    ] {
        let got = found(&l, filter).await?;
        ensure!(
            got == want,
            "`{filter}` holds for {want:?}; it answered {got:?}"
        );
    }
    let (paid, open) = (l.read(TYPE, "i1").await?, l.read(TYPE, "open").await?);
    ensure!(
        value(&paid, "late") == Some(&json!(64)) && value(&open, "late").is_none(),
        "the derived `late` is 64 where the invoice is paid and absent where it is not; the \
         records hold {:?} and {:?}",
        value(&paid, "late"),
        value(&open, "late")
    );
    // The declaration that calls `floor` is the lake's first event in
    // format 11, and every event after it carries 11.
    let events = log_events(store.as_ref()).await?;
    let formats: Vec<_> = events.iter().map(|e| (e.v, e.body.kind())).collect();
    let mut from = formats.iter().skip_while(|(v, _)| *v != 11);
    ensure!(
        from.next() == Some(&(11, "declared")) && from.all(|(v, _)| *v == 11) && formats[0].0 < 11,
        "the lake writes format 11 from the declaration that calls `floor` on, and not before; \
         its events carry {formats:?}"
    );
    Ok(())
}

async fn a_derived_field_its_expression_can_never_fill_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let ty = TypeName::from(TYPE);
    let head = store.head().await.must("reading the head")?;
    let duration = "paid_on - due_on";
    // One row a pair: what the expression certainly is, and a field's kind
    // that never holds it.
    // Nothing of a certain kind lands in a `geo` or a `bytes` field.
    let exact = StorageKind::Decimal;
    let pairs: [(&str, &str, &[StorageKind]); 5] = [
        (duration, "a duration", &[Number, exact, Boolean, DateTime]),
        ("paid_on > due_on", "a boolean", &[Number, exact, DateTime]),
        ("paid_on + 1d", "an instant", &[Number, exact, Boolean]),
        ("n * 2", "a number", &[Boolean, DateTime, Geo]),
        ("upper(code)", "a text", &[Boolean, Bytes]),
    ];
    for (expr, is, kinds) in pairs {
        for kind in kinds.iter().copied() {
            let d = derived("d", kind, expr, &[]);
            let what = format!("a derived {kind:?} field that is `{expr}`");
            let refused = cat.add_field(own, &ty, d).await;
            ensure!(
                refused_on(&refused).is_some_and(|(f, _)| f == "d"),
                "{what} is refused on the field; add_field answered {refused:?}"
            );
            invalid_naming(&what, refused, &[is, "never holds"])?;
        }
    }
    // Every door a declaration comes through holds it, and says how a
    // duration becomes a number.
    let dates = vec![field("due_on", DateTime), field("paid_on", DateTime)];
    let mut to = type_decl(TYPE, dates);
    to.fields.push(derived("d", Number, duration, &[]));
    let unit = ["a duration", "/ 1d"];
    invalid_naming("declaring it", l.try_declare(to.clone()).await, &unit)?;
    let migrated = cat.migrate(own, migration(to)).await;
    invalid_naming("migrating to it", migrated, &unit)?;
    let now = store.head().await.must("reading the head")?;
    ensure!(
        now == head,
        "a refused declaration appends nothing; the head is {now}"
    );
    // What may land is taken: a text that reads as a number, a duration as
    // text, and a kind that depends on the record.
    for (name, kind, expr) in [
        ("as_text", Number, "concat(paid_on - due_on) / 86400000000"),
        ("spelled", Text, duration),
        ("either", Boolean, "if(n > 1, paid_on, n)"),
    ] {
        let d = derived(name, kind, expr, &[]);
        let taken = cat.add_field(own, &ty, d).await;
        taken.must(&format!("a derived {kind:?} field that is `{expr}`"))?;
    }
    Ok(())
}

async fn what_can_never_be_evaluated_is_refused_and_a_records_own_failure_is_not(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let ty = TypeName::from(TYPE);
    let kept = field_access(store.as_ref(), "kept", TYPE, &[("due_on", Access::None)]);
    for (expr, names) in [
        ("due_on + paid_on", "an instant and an instant"),
        ("(due_on < paid_on) * 2", "a boolean and a number"),
        ("(paid_on - due_on) + 1", "a duration and a number"),
        ("n / 1d", "a number and a duration"),
        ("abs(paid_on - due_on)", "a number, not a duration"),
        ("-due_on", "negates an instant"),
    ] {
        let names = ["has no meaning", names];
        let filter = format!("{expr} is null");
        let asked = cat.find(own, &ty, find_where(&filter)).await;
        invalid_naming(&format!("a where `{filter}`"), asked, &names)?;
        let asked = cat.find(own, &ty, keyed(expr)).await;
        invalid_naming(&format!("a group key `{expr}`"), asked, &names)?;
        let mut walk = pattern(TYPE, None, Vec::new(), Projection::Graph);
        walk.start.filter = Some(filter.as_str().into());
        let asked = cat.query(own, walk).await;
        invalid_naming(&format!("a pattern's where `{filter}`"), asked, &names)?;
        let mut requires = write_verb("v", TYPE, "code", "'x'");
        if let lakelands_api::write::Declaration::Verb(v) = &mut requires {
            v.requires = Some(filter.as_str().into());
        }
        for (what, d) in [
            ("a trigger's when", trigger_on("t", TYPE, Some(&filter))),
            ("an object set's where", object_set_on("s", TYPE, &filter)),
            ("a verb's requires", requires),
            ("a verb's value", write_verb("v", TYPE, "code", expr)),
        ] {
            let declared = cat.declare(own, d).await;
            invalid_naming(&format!("{what} `{filter}`"), declared, &names)?;
        }
    }
    // The refusal tells nothing of a field its reader is kept from.
    let hidden = find_where("due_on + paid_on is null");
    let asked = cat.find(&kept, &ty, hidden).await;
    ensure!(
        no_such_field(&asked, "due_on"),
        "a reader kept from `due_on` is answered as for a field the type lacks; find answered \
         {asked:?}"
    );
    // A failure of one record's value is that record's: `i1` divides by a
    // duration of zero and multiplies a text that is no number.
    for expr in ["(paid_on - due_on) / (paid_on - paid_on)", "code * 2"] {
        let got = found(&l, &format!("{expr} > 0")).await?;
        let by = keys(&l, expr).await?;
        ensure!(
            got.is_empty() && by == [None],
            "`{expr}` fails for `i1` alone: no record matches and every key is null; the where \
             answered {got:?}, the group keys {by:?}"
        );
    }
    Ok(())
}
