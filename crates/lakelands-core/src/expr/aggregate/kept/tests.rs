use super::*;
use crate::expr::aggregate::Grouping;
use crate::expr::parse;
use serde_json::json;

type Answer = std::result::Result<Value, String>;

fn row(v: &Value) -> Row {
    serde_json::from_value(v.clone()).unwrap()
}

fn x(v: &str) -> Value {
    json!({ "x": v })
}

/// One group of a view whose one measure is `m`, kept.
struct Kept {
    named: [(FieldName, Expr); 1],
    keeping: Keeping,
    folds: KeptFolds,
}

impl Kept {
    fn of(expr: &str) -> Kept {
        let named = [(FieldName::from("m"), parse(expr).unwrap())];
        let keeping = Keeping::of(&named);
        let folds = keeping.none();
        Kept {
            named,
            keeping,
            folds,
        }
    }

    fn put(&mut self, key: &str, fields: &Value) -> &mut Kept {
        let (key, fields) = (RecordKey::from(key), row(fields));
        self.keeping.put(&mut self.folds, &key, &fields, None);
        self
    }

    fn take(&mut self, key: &str, fields: &Value) -> bool {
        let (key, fields) = (RecordKey::from(key), row(fields));
        self.keeping.take(&mut self.folds, &key, &fields, None)
    }

    fn change(&mut self, was: (&str, &Value), is: (&str, &Value)) -> bool {
        let was = (RecordKey::from(was.0), row(was.1));
        let is = (RecordKey::from(is.0), row(is.1));
        let (was, is) = ((&was.0, &was.1), (&is.0, &is.1));
        self.keeping.change(&mut self.folds, was, is, None)
    }

    /// The measure over the kept folds.
    fn m(&self) -> Answer {
        let (by, given) = (Row::new(), Row::new());
        let answer = (self.keeping).measures(&self.folds, &by, &self.named, given, None);
        answer.map(|mut r| r.remove("m").unwrap()).map_err(|e| e.0)
    }

    fn text(&self) -> String {
        self.m().unwrap().as_str().unwrap().to_owned()
    }

    fn written(&self) -> String {
        serde_json::to_value(&self.folds).unwrap().to_string()
    }
}

/// The measure `expr` as the computation answers it over `members`.
fn computed(members: &[(&str, Value)], expr: &str) -> Answer {
    let named = [(FieldName::from("m"), parse(expr).unwrap())];
    let mut group = Grouping::new(Vec::new(), &named, Timestamp(0));
    members.iter().for_each(|(_, v)| group.add(&row(v), None));
    let (by, folds) = group.groups().pop().unwrap_or_default();
    let answer = super::super::measures(&folds, &by, &named, Row::new(), None, Timestamp(0));
    answer.map(|mut r| r.remove("m").unwrap()).map_err(|e| e.0)
}

#[test]
fn a_kept_state_answers_what_the_computation_answers_whatever_order_its_members_came_in() {
    let members = [
        (
            "k1",
            json!({"x": "1.10", "t": "2026-03-01T00:00:00Z", "d": 1}),
        ),
        (
            "k2",
            json!({"x": "-1.10", "t": "2026-01-01T00:00:00Z", "d": 0}),
        ),
        ("k3", json!({"x": 6, "d": 2})),
        (
            "k4",
            json!({"x": "0.000", "t": "2026-01-01T00:00:00.000Z", "d": 1}),
        ),
        ("k5", json!({"d": 3})),
    ];
    // A member that holds no extreme, put first and taken last.
    let more = (
        "k0",
        json!({"x": "3.5", "t": "2026-02-01T00:00:00Z", "d": 1}),
    );
    let exprs = [
        "count()",
        "count(x)",
        "sum(x)",
        "avg(x)",
        "min(x)",
        "max(x)",
        "min(t)",
        "max(t)",
        "sum(if(d > 0, x, null))",
        "sum(x) / count() + max(d) - min(d)",
        "max(x) - min(x)",
        "sum(d) / min(d)",
    ];
    for expr in exprs {
        let want = computed(&members, expr);
        let mut written = Vec::new();
        for (against, with_more) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut kept = Kept::of(expr);
            let mut order = members.to_vec();
            if against {
                order.reverse();
            }
            if with_more {
                kept.put(more.0, &more.1);
            }
            for (key, fields) in &order {
                kept.put(key, fields);
            }
            assert!(!with_more || !kept.take(more.0, &more.1), "{expr}");
            assert!(kept.folds.exact() && !kept.folds.is_empty(), "{expr}");
            assert_eq!(kept.m(), want, "{expr}");
            written.push(kept.written());
        }
        assert!(
            written.windows(2).all(|w| w[0] == w[1]),
            "{expr}: {written:?}"
        );
    }
    assert_eq!(computed(&members, "sum(x)"), Ok(json!("6.000")));
    assert_eq!(
        computed(&members, "min(t)"),
        Ok(json!("2026-01-01T00:00:00Z"))
    );
    assert_eq!(
        computed(&members, "sum(d) / min(d)"),
        Err("division by zero".into())
    );
}

#[test]
fn a_number_taken_out_leaves_the_sum_of_the_rest() {
    let mut kept = Kept::of("sum(x)");
    kept.put("a", &x("5"))
        .put("b", &x("0.00"))
        .put("c", &x("1.5"));
    assert_eq!(kept.text(), "6.50");
    // The zero's places leave with the zero.
    assert!(!kept.take("b", &x("0.00")));
    assert_eq!(kept.text(), "6.5");
    let mut rest = Kept::of("sum(x)");
    rest.put("c", &x("1.5")).put("a", &x("5"));
    assert_eq!(kept.folds, rest.folds);
    kept.take("c", &x("1.5"));
    kept.take("a", &x("5"));
    assert!(kept.folds.is_empty() && kept.folds.exact());
    assert_eq!(kept.folds, kept.keeping.none());
    assert_eq!(kept.text(), "0");
    // A member whose argument is null is a member all the same.
    kept.put("n", &json!({}));
    assert!(!kept.folds.is_empty());
    assert_eq!(kept.text(), "0");
}

#[test]
fn what_does_not_fold_is_counted_and_the_rest_goes_on_being_folded() {
    let (big, small) = ("10000000000000000000", "0.0000000000000000001");
    let mut kept = Kept::of("sum(x)");
    // Past exact arithmetic: the two need 38 digits between them.
    kept.put("a", &x(big)).put("b", &x(small));
    assert!(!kept.folds.exact() && !kept.folds.waits());
    kept.take("b", &x(small));
    assert!(kept.folds.exact());
    assert_eq!(kept.text(), big);
    // A text that reads as no number, and a number no decimal holds.
    let long = "12345678901234567890123456789012345";
    kept.put("c", &x("abc"))
        .put("d", &x(long))
        .put("e", &x("2"));
    assert!(!kept.folds.exact());
    kept.take("c", &x("abc"));
    assert!(!kept.folds.exact());
    kept.take("d", &x(long));
    assert!(kept.folds.exact());
    assert_eq!(kept.text(), "10000000000000000002");
    // An argument that fails, under any call.
    for expr in ["sum(x / d)", "count(x / d)", "min(x / d)"] {
        let mut kept = Kept::of(expr);
        let (fails, folds) = (json!({"x": 1, "d": 0}), json!({"x": 4, "d": 2}));
        kept.put("a", &fails).put("b", &folds);
        assert!(!kept.folds.exact(), "{expr}");
        kept.take("a", &fails);
        assert!(kept.folds.exact(), "{expr}");
        let rest = [("b", folds)];
        assert_eq!(kept.m(), computed(&rest, expr), "{expr}");
    }
    // An extreme folds what reads as a number or an instant.
    let mut kept = Kept::of("max(x)");
    kept.put("a", &x("abc")).put("b", &x("7"));
    assert!(!kept.folds.exact());
    kept.take("a", &x("abc"));
    assert!(kept.folds.exact());
    assert_eq!(kept.text(), "7");
}

#[test]
fn an_extreme_is_held_by_its_value_and_then_by_the_least_key() {
    let mut kept = Kept::of("min(x)");
    kept.put("k2", &x("1.00")).put("k1", &x("1.0"));
    assert_eq!(kept.text(), "1.0");
    // The holder written to an equal value under its own key stays.
    assert!(!kept.change(("k1", &x("1.0")), ("k1", &x("1.00"))));
    assert!(kept.folds.exact());
    kept.change(("k1", &x("1.00")), ("k1", &x("1.0")));
    assert_eq!(kept.text(), "1.0");
    // Under a greater key it has left, whatever its value.
    assert!(kept.change(("k1", &x("1.0")), ("k9", &x("1.0"))));
    assert!(kept.folds.waits() && !kept.folds.exact());
    // Read again, the group is held by the least key of the equal values.
    let mut again = Kept::of("min(x)");
    again.put("k9", &x("1.0")).put("k2", &x("1.00"));
    assert_eq!(again.text(), "1.00");

    // Each row: a member of `k1 = 5`, `k2 = 3`, `k3 = 7` changes to a key
    // and a value, or leaves (`-`); then the minimum, or `waits`.
    let rows = [
        ("k2", "3", "k2", "2", "2"),
        ("k2", "3", "k2", "3.0", "3.0"),
        ("k2", "3", "k0", "2", "2"),
        ("k2", "3", "k0", "3", "3"),
        ("k2", "3", "k2", "4", "waits"),
        ("k2", "3", "k8", "1", "waits"),
        ("k2", "3", "-", "-", "waits"),
        ("k3", "7", "k3", "1", "1"),
        ("k1", "5", "k1", "3.0", "3.0"),
        ("k3", "7", "k3", "3.0", "3"),
        ("k3", "7", "k3", "9", "3"),
        ("k1", "5", "-", "-", "3"),
    ];
    for (key, was, to, value, want) in rows {
        let mut kept = Kept::of("min(x)");
        kept.put("k1", &x("5"))
            .put("k2", &x("3"))
            .put("k3", &x("7"));
        let waits = match to {
            "-" => kept.take(key, &x(was)),
            to => kept.change((key, &x(was)), (to, &x(value))),
        };
        let got = if waits { "waits".into() } else { kept.text() };
        assert_eq!(got, want, "{key} = {was} to {to} = {value}");
        assert_eq!(kept.folds.waits(), waits);
    }

    // A group of one member has nothing to read again.
    let mut kept = Kept::of("max(x)");
    kept.put("k1", &x("5"));
    assert!(!kept.change(("k1", &x("5")), ("k4", &x("2"))));
    assert_eq!(kept.text(), "2");
    assert!(!kept.take("k4", &x("2")));
    assert_eq!(kept.folds, kept.keeping.none());
    // Nor has one whose other members hold no value.
    kept.put("k1", &x("5")).put("k2", &json!({}));
    assert!(!kept.take("k1", &x("5")));
    assert!(kept.folds.exact() && !kept.folds.is_empty());
    assert_eq!(kept.m(), Ok(Value::Null));
}

#[test]
fn a_state_that_cannot_be_trusted_is_not_exact_and_nothing_panics() {
    for expr in ["sum(x)", "count()", "count(x)", "max(x)"] {
        let mut kept = Kept::of(expr);
        kept.put("a", &x("1"));
        // A member taken that was never put.
        assert!(!kept.take("b", &x("0.50")), "{expr}");
        kept.take("a", &x("1"));
        kept.take("a", &x("1"));
        assert!(!kept.folds.exact() && !kept.folds.waits(), "{expr}");
        kept.put("c", &x("2"));
        assert!(!kept.folds.exact(), "{expr}");
    }
    // One whose argument is null, never put, leaves no fold wrong but the
    // count of members.
    let mut kept = Kept::of("sum(x)");
    kept.take("z", &json!({}));
    assert!(!kept.folds.exact());
    // One with a value, never put, while another member holds the count.
    let mut kept = Kept::of("count(x)");
    kept.put("a", &json!({}));
    kept.take("b", &x("1"));
    assert!(!kept.folds.exact());
    // A state made under another declaration's measures.
    let mut kept = Kept::of("sum(x) + count()");
    kept.folds = Kept::of("count()").folds;
    kept.put("a", &x("1"));
    assert!(!kept.folds.exact());
}

#[test]
fn a_state_is_written_as_an_object_and_reads_back_equal() {
    let mut kept = Kept::of("sum(x) + max(x) + count()");
    kept.put("k1", &x("1.10")).put("k2", &x("2.5"));
    let written = serde_json::to_value(&kept.folds).unwrap();
    assert!(written.is_object());
    assert_eq!(
        written.to_string(),
        r#"{"folds":[{"n":2,"sum":{"1":[1,"25","25"],"2":[1,"110","110"]}},{"best":[{"text":"2.5"},"k2"],"n":2},{"n":2}],"members":2}"#
    );
    let read: KeptFolds = serde_json::from_value(written).unwrap();
    assert_eq!(read, kept.folds);
    assert_eq!(kept.text(), "8.10");
    // What is not exact says so in what is written, too.
    kept.take("k2", &x("2.5"));
    kept.put("k3", &x("abc"));
    let read: KeptFolds = serde_json::from_str(&kept.written()).unwrap();
    assert!(read.waits() && !read.exact());
    assert_eq!(read, kept.folds);
}

#[test]
fn an_extreme_is_held_as_it_is_written_and_only_among_values_of_one_kind() {
    // The holder restated at another scale is another state, as it is
    // other bytes: a caller that puts on a difference puts it.
    let (was, is) = (json!({"p": "2.5", "q": 4}), json!({"p": "2.50", "q": 4}));
    let mut kept = Kept::of("max(p * q)");
    kept.put("k1", &was);
    let before = kept.folds.clone();
    assert!(!kept.change(("k1", &was), ("k1", &is)));
    assert_ne!(kept.folds, before);
    assert_eq!(kept.text(), "10.00");

    // A zero's sign is read back as it was written.
    let members = [("k1", x("0.00")), ("k2", x("-5"))];
    let mut kept = Kept::of("min(-x)");
    kept.put("k1", &members[0].1);
    kept.folds = serde_json::from_str(&kept.written()).unwrap();
    kept.put("k2", &members[1].1);
    assert_eq!(kept.m(), computed(&members, "min(-x)"));
    assert_eq!(kept.text(), "-0.00");

    // A number and an instant have no order between them: a state that
    // holds both says so by itself, in whichever order they came.
    let (number, instant) = (json!({"x": 5}), x("2026-01-01T00:00:00Z"));
    let mut kept = Kept::of("min(x)");
    kept.put("k2", &number).put("k1", &instant);
    assert!(!kept.folds.exact() && !kept.folds.waits());
    assert!(!kept.take("k1", &instant));
    assert!(kept.folds.exact());
    assert_eq!(kept.text(), "5");
    let mut kept = Kept::of("min(x)");
    kept.put("k1", &instant).put("k2", &number);
    assert!(!kept.folds.exact());
    // The holder of the other kind leaving is a holder leaving.
    assert!(kept.take("k1", &instant));
    let mut kept = Kept::of("min(x)");
    kept.put("k1", &instant).put("k2", &number);
    assert!(!kept.take("k2", &number));
    assert!(kept.folds.exact());
    assert_eq!(kept.text(), "2026-01-01T00:00:00Z");

    // A state read back with a scale no decimal has is not exact.
    let written = r#"{"folds":[{"n":1,"sum":{"29":[1,"5","5"]}}],"members":1}"#;
    let mut kept = Kept::of("sum(x)");
    kept.folds = serde_json::from_str(written).unwrap();
    assert!(!kept.folds.exact());
    kept.put("k1", &x("1"));
    assert!(!kept.folds.exact());
}
