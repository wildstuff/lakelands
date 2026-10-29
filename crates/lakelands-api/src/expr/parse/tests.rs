use super::*;

fn path(p: &str) -> Expr {
    Expr::Path {
        path: p.split('.').map(FieldName::from).collect(),
    }
}

fn lit(value: Literal) -> Expr {
    Expr::Lit { value }
}

fn num(n: &str) -> Expr {
    lit(Literal::Number(n.into()))
}

#[test]
fn precedence_and_associativity() {
    assert_eq!(
        parse("a - b - c").unwrap(),
        Expr::Arith {
            arith: ArithOp::Sub,
            lhs: Box::new(Expr::Arith {
                arith: ArithOp::Sub,
                lhs: Box::new(path("a")),
                rhs: Box::new(path("b")),
            }),
            rhs: Box::new(path("c")),
        }
    );
    let e = parse("a = 1 or b = 2 and not c = 3").unwrap();
    let Expr::Or { args } = e else {
        panic!("or is loosest")
    };
    assert!(matches!(&args[1], Expr::And { args } if matches!(args[1], Expr::Not { .. })));
}

#[test]
fn chains_flatten() {
    let Expr::And { args } = parse("a and b and c").unwrap() else {
        panic!()
    };
    assert_eq!(args.len(), 3);
}

#[test]
fn literals() {
    assert_eq!(parse("'it''s'").unwrap(), lit(Literal::Text("it's".into())));
    assert_eq!(parse("12.50").unwrap(), num("12.50"));
    assert_eq!(
        parse("30d").unwrap(),
        lit(Literal::Duration(30 * 86_400_000_000))
    );
    assert_eq!(parse("1.5s").unwrap(), lit(Literal::Duration(1_500_000)));
    assert_eq!(parse("null").unwrap(), lit(Literal::Null));
    assert_eq!(parse("true").unwrap(), lit(Literal::Bool(true)));
}

#[test]
fn in_and_null_tests() {
    assert!(
        matches!(parse("status in ('open', 'late')").unwrap(), Expr::In { list, .. } if list.len() == 2)
    );
    assert!(matches!(
        parse("note is null").unwrap(),
        Expr::IsNull { .. }
    ));
    assert!(
        matches!(parse("note is not null").unwrap(), Expr::Not { arg } if matches!(*arg, Expr::IsNull { .. }))
    );
}

#[test]
fn calls_check_their_names_and_arity() {
    assert!(matches!(
        parse("due < today() - 30d").unwrap(),
        Expr::Cmp { .. }
    ));
    assert!(parse("known_at(amount) > '2026-01-01'").is_ok());
    assert!(parse("setting('vat_rate') * amount").is_ok());
    assert!(parse("changed(status) and previous(status) = 'open'").is_ok());
    for bad in [
        "bogus(x)",
        "upper(a, b)",
        "if(a, b)",
        "today(1)",
        "known_at(a + 1)",
        "setting(vat_rate)",
        "setting('vat_rate', d, e)",
    ] {
        assert!(parse(bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn a_setting_on_a_date_takes_no_kind_but_an_instant() {
    use crate::expr::kind::{Kind, of};
    use crate::ontology::StorageKind as S;
    let reads = |f: &[FieldName]| match f[0].as_str() {
        "n" => Some((S::Number, 1)),
        "on" => Some((S::Date, 1)),
        _ => Some((S::Text, 1)),
    };
    for at in ["on", "t", "'2026-07-01'", "now() - 1d", "null"] {
        let e = parse(&format!("setting('rate', {at})")).unwrap();
        assert_eq!(of(&e, &reads), Ok(None), "{at}");
    }
    for (text, kind) in [
        ("setting('rate', 5)", Kind::Number),
        ("setting('rate', n)", Kind::Number),
        ("setting('rate', true)", Kind::Boolean),
        ("setting('rate', 1d)", Kind::Duration),
    ] {
        let expr = text.to_owned();
        assert_eq!(
            of(&parse(text).unwrap(), &reads),
            Err(Case::KindTakesDate { expr, kind })
        );
    }
}

#[test]
fn malformed_input_is_refused_where_it_goes_wrong() {
    for (bad, at) in [
        ("1 +", 3),
        ("(1 + 2", 6),
        ("a = ", 4),
        ("'open", 5),
        ("a b", 2),
        ("12x", 2),
        ("and", 0),
    ] {
        let e = parse(bad).unwrap_err();
        assert_eq!(e.at, at, "{bad}: {e}");
    }
    assert!(parse("status in ()").is_err());
}

#[test]
fn keywords_do_not_swallow_longer_names() {
    assert_eq!(parse("android").unwrap(), path("android"));
    assert_eq!(parse("order.notes").unwrap(), path("order.notes"));
    assert!(matches!(parse("island = 1").unwrap(), Expr::Cmp { .. }));
}

#[test]
fn printing_round_trips() {
    for text in [
        "a - (b - c)",
        "(a + b) * c",
        "-(a + b)",
        "not (a = 1 or b = 2)",
        "status = 'open' and amount > 100.00",
        "due_date < today() - 30d",
        "note is not null and tag in ('x', 'y''s')",
        "if(amount > 100, 'large', 'small')",
        "known_at(amount) >= '2026-09-01'",
        "a or b and c",
        "1.5s + 2h",
        "invoice.amount / partner.limit",
    ] {
        let e = parse(text).unwrap();
        assert_eq!(print(&e), text, "printed form of {text}");
        assert_eq!(parse(&print(&e)).unwrap(), e);
    }
}

#[test]
fn printing_adds_only_needed_parentheses() {
    assert_eq!(print(&parse("((a))").unwrap()), "a");
    assert_eq!(print(&parse("(a * b) + c").unwrap()), "a * b + c");
    assert_eq!(
        print(&parse("a and (b and c)").unwrap()),
        "a and b and c".replace("b and c", "(b and c)")
    );
}

/// Every function of the contract. A function added to `Func` is added here.
const ALL_FUNCS: [Func; 29] = [
    Func::Now,
    Func::Today,
    Func::KnownAt,
    Func::SeenAt,
    Func::Previous,
    Func::Setting,
    Func::Param,
    Func::Changed,
    Func::Retired,
    Func::Coalesce,
    Func::If,
    Func::Lower,
    Func::Upper,
    Func::Trim,
    Func::Len,
    Func::Concat,
    Func::Substr,
    Func::Contains,
    Func::Replace,
    Func::StartsWith,
    Func::Matches,
    Func::Abs,
    Func::Round,
    Func::DateBucket,
    Func::Count,
    Func::Sum,
    Func::Min,
    Func::Max,
    Func::Avg,
];

#[test]
fn a_function_is_parsed_by_the_name_it_is_printed_and_serialized_with() {
    for f in ALL_FUNCS {
        let name = func_name(f);
        assert_eq!(serde_json::to_value(f).unwrap(), name, "{f:?}");
        let arg = if takes_field(f) { "a" } else { "'k'" };
        let text = format!("{name}({})", vec![arg; arity(f).0].join(", "));
        match parse(&text) {
            Ok(Expr::Call { func, .. }) => assert_eq!(func, f, "{text}"),
            other => panic!("{text}: {other:?}"),
        }
    }
}

fn too_deep(max: usize, offset: usize) -> ParseError {
    ParseError {
        at: offset,
        case: Case::ExprTooDeep { max, offset },
    }
}

#[test]
fn nesting_at_the_limit_parses_and_one_more_is_refused_where_it_begins() {
    // What opens a level, what closes it, and how far into the text the
    // level begins: a parenthesis, `not` and `-` at their own character, a
    // call argument and a list item after the call or the list opens.
    for (what, open, close, begins) in [
        ("parenthesis", "(", ")", 0),
        ("call argument", "coalesce(", ")", "coalesce(".len()),
        ("list item", "a in (", ")", "a in (".len()),
        ("not", "not ", "", 0),
        ("unary minus", "-", "", 0),
    ] {
        let wrap = |n: usize| format!("{}a{}", open.repeat(n), close.repeat(n));
        assert!(parse(&wrap(32)).is_ok(), "{what} at the limit");
        let offset = 32 * open.len() + begins;
        assert_eq!(
            parse(&wrap(33)),
            Err(too_deep(32, offset)),
            "{what} past the limit"
        );
    }
}

#[test]
fn a_hundred_thousand_open_parentheses_are_refused_without_overflow() {
    let text = "(".repeat(100_000);
    assert_eq!(parse(&text), Err(too_deep(32, 32)));
}
