use super::*;
use serde_json::json;

/// What `keepable` says of a view over `invoice` with these members.
fn said(view: serde_json::Value) -> Result<(), String> {
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
    let invoice = TypeDecl {
        name: "invoice".into(),
        fields: kinds.map(|(name, kind)| FieldDecl::of(name, kind)).into(),
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
    ];
    for expr in kept {
        assert_eq!(measured(&grouped, expr), Ok(()), "{expr}");
    }
    assert_eq!(measured(&[], "sum(amount) / count()"), Ok(()));
    assert_eq!(measured(&["qty", "open"], "avg(amount)"), Ok(()));

    let adds = "it is kept where its argument is certainly a number";
    let orders = "it is kept where its argument is certainly a number or an instant";
    let by = "is kept only in a view grouped by text or reference fields";
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
            format!("`sum(data)` adds what is of no certain kind; {adds}"),
        ),
        (
            &grouped,
            "count() + sum(if(open, amount, note))",
            format!("`sum(if(open, amount, note))` adds what is of no certain kind; {adds}"),
        ),
        (
            &grouped,
            "sum(coalesce(if(open, amount, null), note))",
            format!(
                "`sum(coalesce(if(open, amount, null), note))` adds what is of no certain kind; \
                 {adds}"
            ),
        ),
        (
            &grouped,
            "min(note)",
            format!("`min(note)` orders a text; {orders}"),
        ),
        (
            &[],
            "max(amount)",
            format!("`max(amount)` {by}, and this one has no group field"),
        ),
        (
            &["partner", "qty"],
            "min(due)",
            format!("`min(due)` {by}, and `qty` is neither"),
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
    assert_eq!(
        said(param),
        Err(
            "it declares the parameter `since`, and a kept record is the same for every read"
                .into()
        )
    );
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
