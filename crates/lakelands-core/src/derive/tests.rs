use super::*;
use serde_json::json;

fn ty(v: Value) -> TypeDecl {
    serde_json::from_value(v).unwrap()
}

fn fields(v: Value) -> BTreeMap<FieldName, Value> {
    serde_json::from_value(v).unwrap()
}

fn text(name: &str) -> Value {
    json!({"name": name, "type": {"kind": "text"}})
}

fn decimal(name: &str, derived: Option<&str>) -> Value {
    let mut f = json!({"name": name, "type": {"kind": "decimal"}});
    if let Some(d) = derived {
        f["derived"] = json!(d);
        f["authority"] = json!("derived");
    }
    f
}

/// A first write of `written`, with no settings, at time zero.
fn write(written: &BTreeSet<FieldName>) -> Write<'_> {
    Write {
        before: None,
        written,
        settings: None,
        now: Timestamp(0),
    }
}

#[test]
fn derived_fields_are_ordered_by_what_they_read() {
    let t = ty(json!({"name": "invoice", "fields": [
        decimal("net", Some("gross - vat")),
        decimal("vat", Some("gross * 0.19")),
        decimal("gross", None),
    ]}));
    let names: Vec<String> = order(&t)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n.to_string())
        .collect();
    assert_eq!(names, ["vat", "net"]);
}

#[test]
fn cycles_and_self_reads_are_refused() {
    let cyc = ty(
        json!({"name": "t", "fields": [decimal("a", Some("b + 1")), decimal("b", Some("a + 1"))]}),
    );
    assert!(order(&cyc).unwrap_err().0.contains("cycle"));
    let own = ty(json!({"name": "t", "fields": [decimal("a", Some("a + 1"))]}));
    assert!(order(&own).unwrap_err().0.contains("itself"));
    let bad = ty(json!({"name": "t", "fields": [decimal("a", Some("1 +"))]}));
    assert!(order(&bad).is_err());
}

#[test]
fn derived_values_recompute_in_order_and_clear_when_they_cannot() {
    let t = ty(json!({"name": "invoice", "fields": [
        decimal("gross", None),
        decimal("vat", Some("gross * 0.19")),
        decimal("net", Some("gross - vat")),
    ]}));
    let mut f = fields(json!({"gross": "100.00"}));
    let written: BTreeSet<FieldName> = ["gross".into()].into();
    let w = write(&written);
    let touched = apply(&t, &[], &mut f, &w).unwrap();
    assert_eq!(f[&FieldName::from("vat")], json!("19.0000"));
    assert_eq!(f[&FieldName::from("net")], json!("81.0000"));
    assert_eq!(touched.len(), 2);

    f.insert("gross".into(), Value::Null);
    apply(&t, &[], &mut f, &w).unwrap();
    assert_eq!(
        f[&FieldName::from("vat")],
        Value::Null,
        "absent input clears"
    );
}

#[test]
fn a_derived_decimal_is_rounded_to_its_scale_before_the_next_one_reads_it() {
    let money: ValueTypeDecl =
        serde_json::from_value(json!({"name": "money", "kind": "decimal", "scale": 2})).unwrap();
    let m = |name: &str, derived: Option<&str>| {
        let mut f = decimal(name, derived);
        f["type"]["value_type"] = json!("money");
        f
    };
    let t = ty(json!({"name": "invoice", "fields": [
        m("gross", None),
        m("vat", Some("gross * 19 / 119")),
        m("net", Some("gross - vat")),
    ]}));
    let mut f = fields(json!({"gross": "100.00"}));
    let written: BTreeSet<FieldName> = ["gross".into()].into();
    let w = write(&written);
    apply(&t, &[money], &mut f, &w).unwrap();
    assert_eq!(f[&FieldName::from("vat")], json!("15.97"));
    assert_eq!(
        f[&FieldName::from("net")],
        json!("84.03"),
        "net + vat reconciles to gross"
    );
}

#[test]
fn a_derived_decimal_is_written_at_its_scale_even_when_computed_at_a_smaller_one() {
    let money: ValueTypeDecl =
        serde_json::from_value(json!({"name": "money", "kind": "decimal", "scale": 2})).unwrap();
    let mut total = decimal("total", Some("quantity * 3"));
    total["type"]["value_type"] = json!("money");
    let t = ty(json!({"name": "line", "fields": [decimal("quantity", None), total]}));
    let mut f = fields(json!({"quantity": "2"}));
    let written: BTreeSet<FieldName> = ["quantity".into()].into();
    let w = write(&written);
    apply(&t, &[money], &mut f, &w).unwrap();
    assert_eq!(f[&FieldName::from("total")], json!("6.00"));
}

#[test]
fn a_dependent_is_cleared_when_its_input_changes_unless_the_write_sets_it() {
    let t = ty(json!({"name": "decl", "fields": [
        text("body"),
        {"name": "verdict", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "clear"},
        {"name": "since", "type": {"kind": "text"}, "depends_on": ["verdict"], "on_input_change": "clear"},
        {"name": "note", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "keep"},
    ]}));
    let mut f = fields(json!({"body": "v2", "verdict": "ok", "since": "2026", "note": "n"}));
    let body: BTreeSet<FieldName> = ["body".into()].into();
    let w = write(&body);
    let touched = apply(&t, &[], &mut f, &w).unwrap();
    assert_eq!(f[&FieldName::from("verdict")], Value::Null);
    assert_eq!(
        f[&FieldName::from("since")],
        Value::Null,
        "clearing is itself a change"
    );
    assert_eq!(f[&FieldName::from("note")], json!("n"), "keep keeps");
    assert_eq!(touched, ["since".into(), "verdict".into()].into());

    let mut g = fields(json!({"body": "v3", "verdict": "fresh"}));
    let both: BTreeSet<FieldName> = ["body".into(), "verdict".into()].into();
    let w = write(&both);
    apply(&t, &[], &mut g, &w).unwrap();
    assert_eq!(g[&FieldName::from("verdict")], json!("fresh"));
}

#[test]
fn a_write_that_touches_no_input_clears_nothing() {
    let t = ty(json!({"name": "decl", "fields": [
        text("body"), text("other"),
        {"name": "verdict", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "clear"},
    ]}));
    let mut f = fields(json!({"body": "v", "other": "x", "verdict": "ok"}));
    let other: BTreeSet<FieldName> = ["other".into()].into();
    let w = write(&other);
    assert!(apply(&t, &[], &mut f, &w).unwrap().is_empty());
    assert_eq!(f[&FieldName::from("verdict")], json!("ok"));
}
