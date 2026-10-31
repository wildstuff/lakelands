use super::*;
use crate::ontology::ComputedAt;
use serde_json::json;

/// What `keepable` says of a view over `invoice` with these members, as
/// its sentence.
fn said(view: serde_json::Value) -> Result<(), String> {
    case(view).map_err(|said| said.to_string())
}

/// What `keepable` says of a view over `invoice` with these members.
fn case(view: serde_json::Value) -> Result<(), Said> {
    use StorageKind as K;
    let kinds = [
        ("partner", K::Reference),
        ("region", K::Text),
        ("amount", K::Decimal),
        ("qty", K::Number),
        ("due", K::DateTime),
        ("note", K::Text),
        ("data", K::Json),
        ("open", K::Boolean),
    ];
    let mut fields: Vec<_> = kinds.map(|(name, kind)| FieldDecl::of(name, kind)).into();
    let buckets = [
        ("hour", "date_bucket(due, 1h, '2026-01-01T00:00:00Z')"),
        ("day", "date_bucket(due, 'day')"),
        ("lot", "date_bucket(note, 'day')"),
        ("late", "due"),
        ("age", "due"),
    ];
    for (name, expr) in buckets {
        let mut f = FieldDecl::of(name, K::DateTime);
        f.derived = Some(expr.into());
        if name == "age" {
            f.computed_at = Some(ComputedAt::Read);
        }
        fields.push(f);
    }
    let invoice = TypeDecl {
        name: "invoice".into(),
        fields,
        ..TypeDecl::default()
    };
    let mut whole = json!({"source": "invoice", "measures": []});
    whole
        .as_object_mut()
        .unwrap()
        .extend(view.as_object().unwrap().clone());
    let view: ViewDecl = serde_json::from_value(whole).unwrap();
    keepable(&view, |ty| (ty.as_str() == "invoice").then_some(&invoice))
}

/// A view grouped by `by` with the one measure `expr`.
fn measured(by: &[&str], expr: &str) -> Result<(), String> {
    said(json!({"group_by": by, "measures": [{"name": "m", "expr": expr}]}))
}

#[test]
fn a_view_is_kept_where_every_fold_is_a_function_of_its_values() {
    let grouped = ["partner", "region"];
    let kept = [
        "count()",
        "count(note)",
        "sum(amount)",
        "avg(qty)",
        "sum(amount * qty) - sum(amount) / count()",
        // A fold skips nulls: a filtered argument is of its value's kind.
        "sum(if(open, amount, null))",
        "avg(if(open, null, qty))",
        "sum(if(open, amount, 0))",
        // Wherever the filter stands: arithmetic over a null is null.
        "sum(if(open, amount, null) * qty)",
        "sum(-if(open, amount, null))",
        "avg(if(open, amount, if(qty > 0, qty, null)))",
        "sum(coalesce(if(open, amount, null), qty))",
        "sum(coalesce(amount, 0))",
        "min(due)",
        "max(amount) - min(if(open, amount, null))",
        "first(note, due)",
        "last(amount, qty) - first(data, if(open, due, null))",
    ];
    for expr in kept {
        assert_eq!(measured(&grouped, expr), Ok(()), "{expr}");
    }
    assert_eq!(measured(&[], "sum(amount) / count()"), Ok(()));
    assert_eq!(measured(&["qty", "open"], "avg(amount)"), Ok(()));
    // An extreme is kept under any group field the write fixes, or none.
    let m = "min(amount) + last(qty, due)";
    for by in ["partner hour", "day region", "", "due", "hour day"] {
        let by: Vec<_> = by.split_whitespace().collect();
        assert_eq!(measured(&by, m), Ok(()), "{by:?}");
    }
    assert_eq!(measured(&["qty", "late"], m), Ok(()));
    // A group field computed at the read is not fixed by the write.
    let aged = "it groups by what reads `age` of `invoice`, which is computed at the read";
    let refused = measured(&["partner", "age"], "max(amount)");
    assert!(refused.is_err_and(|why| why.starts_with(aged)));

    let adds = "it is kept where its argument is certainly a number";
    let orders = "it is kept where what it orders by is certainly a number or an instant";
    let refused = [
        (
            &grouped[..],
            "sum(note)",
            format!("`sum(note)` adds a text; {adds}"),
        ),
        (
            &grouped,
            "avg(due)",
            format!("`avg(due)` adds an instant; {adds}"),
        ),
        (
            &grouped,
            "sum(data)",
            format!("`sum(data)` adds what is of no certain kind: {adds}"),
        ),
        (
            &grouped,
            "count() + sum(if(open, amount, note))",
            format!("`sum(if(open, amount, note))` adds what is of no certain kind: {adds}"),
        ),
        (
            &grouped,
            "sum(coalesce(if(open, amount, null), note))",
            format!(
                "`sum(coalesce(if(open, amount, null), note))` adds what is of no certain kind: \
                 {adds}"
            ),
        ),
        (
            &grouped,
            "min(note)",
            format!("`min(note)` orders a text; {orders}"),
        ),
        (
            &grouped,
            "first(amount, note)",
            format!("`first(amount, note)` orders a text; {orders}"),
        ),
    ];
    for (by, expr, why) in refused {
        assert_eq!(
            measured(by, expr),
            Err(format!("measure `m`: {why}")),
            "{expr}"
        );
    }
}

#[test]
fn a_view_that_a_read_or_the_clock_decides_is_not_kept() {
    let clock = "reads the clock, and a kept record is computed when its source is written";
    let count = json!([{"name": "n", "expr": "count()"}]);
    let param = json!({"params": [{"name": "since", "kind": "date_time"}], "measures": count});
    let name = "since".into();
    assert_eq!(case(param), Err(Case::KeptParam { name }.into()));
    let late = json!({"where": "due < now()", "measures": count});
    assert_eq!(
        said(late),
        Err(format!("its `where` over `invoice` {clock}"))
    );
    let age = json!({"measures": [{"name": "age", "expr": "count() + len(today())"}]});
    assert_eq!(said(age), Err(format!("measure `age`: it {clock}")));
    // A merged source is held to the same.
    let merge = |member: serde_json::Value| {
        let mut source = json!({"source": "invoice", "group_by": ["partner"], "measures": []});
        source
            .as_object_mut()
            .unwrap()
            .extend(member.as_object().unwrap().clone());
        said(json!({"group_by": ["partner"], "measures": count, "merge": [source]}))
    };
    assert_eq!(
        merge(json!({"measures": [{"name": "s", "expr": "sum(amount)"}]})),
        Ok(())
    );
    let texts = merge(json!({"measures": [{"name": "s", "expr": "max(note)"}]}));
    assert!(texts.is_err_and(|why| why.starts_with("measure `s`: `max(note)` orders")));
    let dated = merge(json!({"where": "due < today()", "measures": []}));
    assert_eq!(dated, Err(format!("its `where` over `invoice` {clock}")));
}
