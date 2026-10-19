//! A function's input states a kind or none
//! (`docs/decisions/022-functions-are-declarations.md`,
//! extended): a typed input is bound by a field of that kind alone, one
//! with no `type` by a field of any kind, and a function declared again is
//! held to every type that binds it.

use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Outcome, ProposedWrite};
use lakelands_api::{Access, Grants, PrincipalKind, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, computed_by, dec, decimal, field, invalid_like, principal, refused_unmoved,
    type_decl, value,
};
use crate::conformance::{Failure, Held, Store};
use lakelands_api::ontology::StorageKind::{Date, Decimal, Number, Text};

check!(
    ANY_KIND_INPUT = an_input_with_no_type_is_bound_by_a_field_of_any_kind,
    "022, extended (lakelands#933): an `expr` function whose input states no `type` is bound by a type that holds the field as a number, as a decimal, as a text and as a date, each declare taken, and computes in the kind the record holds, `3` to `6` and `2.5` to `5`; an input that states a `type` keeps the rule, a type holding the field in another kind refused `function_input_kind`, naming the function, the input, both kinds and the type"
);

check!(
    INPUT_KIND_REDECLARED = a_function_declared_again_keeps_the_kinds_its_types_hold,
    "022, extended (lakelands#933): a function declared again with an input of another kind while a type binds it as the old kind is refused `function_input_kind`, naming the type, appending nothing; declared again with no `type` for the input, or with the kind the type holds, it is taken, and so it is after being taken without one; a holder of `schema` not shown the type, or shown it and kept from the input's field, is refused naming nothing of it, and so where another input is broken by a field it is shown, by `declare` and at the accept of a proposal made before the type bound it, whose accept by the owner is refused `function_input_kind`"
);

fn untyped(name: &str, input: &str, expr: &str) -> Declaration {
    let d = json!({"declare": "function", "name": name, "backing": "expr", "inputs": [{"name": input}], "expr": expr});
    serde_json::from_value(d).expect("an untyped function reads")
}

fn typed(name: &str, input: &str, kind: &str) -> Declaration {
    let inputs = json!([{"name": input, "type": {"kind": kind}}]);
    let d = json!({"declare": "function", "name": name, "backing": "expr", "inputs": inputs, "expr": "qty * 2"});
    serde_json::from_value(d).expect("a typed function reads")
}

async fn an_input_with_no_type_is_bound_by_a_field_of_any_kind(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let d = untyped("twice_of", "qty", "qty * 2");
    ensure!(
        matches!(&d, Declaration::Function(f) if f.inputs[0].any_kind),
        "an input with no `type` must read as one of any kind: {d:?}"
    );
    l.declare_as(d, "declaring `twice_of`, its input with no `type`")
        .await?;
    for (name, kind) in [("n", Number), ("d", Decimal), ("t", Text), ("on", Date)] {
        let twice = computed_by("twice", kind, "twice_of", &["qty"]);
        let ty = type_decl(name, vec![field("qty", kind), twice]);
        l.declare(ty).await?;
    }
    l.upsert("n", "a", json!({"qty": 3})).await?;
    let got = value(&l.read("n", "a").await?, "twice").and_then(|v| v.as_f64());
    ensure!(
        got == Some(6.0),
        "`twice_of` over a number `3` must compute `6`: {got:?}"
    );
    l.upsert("d", "a", json!({"qty": "2.5"})).await?;
    let got = decimal(value(&l.read("d", "a").await?, "twice"));
    ensure!(
        got == Some(dec("5")),
        "`twice_of` over a decimal `2.5` must compute `5`: {got:?}"
    );
    // The same function, its input typed, is held to its kind.
    l.declare_as(
        typed("fee", "qty", "text"),
        "declaring `fee`, its input a text",
    )
    .await?;
    let bill = type_decl(
        "bill",
        vec![
            field("qty", Decimal),
            computed_by("fee", Text, "fee", &["qty"]),
        ],
    );
    let refused = l.try_declare(bill).await;
    let case = json!({"code": "function_input_kind", "function": "fee", "input": "qty",
        "kind": "text", "type": "bill", "held": "decimal"});
    let e = refused.must_fail("binding the text input `qty` of `fee` to a decimal")?;
    ensure!(
        invalid_like(&e, &case),
        "binding a text input to a decimal must be refused as {case}: {e:?}"
    );
    Ok(())
}

async fn a_function_declared_again_keeps_the_kinds_its_types_hold(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_as(typed("twice_of", "qty", "number"), "declaring `twice_of`")
        .await?;
    let bound = |name: &str| {
        let twice = computed_by("twice", Number, "twice_of", &["qty"]);
        type_decl(name, vec![field("qty", Number), twice])
    };
    // Proposed while no type binds it, and judged at its accept.
    let write = ProposedWrite::Declare {
        declarations: vec![typed("twice_of", "qty", "text")],
        plan: None,
    };
    let proposed = l.cat.propose(&l.own, write, "text".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing `twice_of` as a text")?
    else {
        return Err(Failure::new("`twice_of` as a text must be proposed"));
    };
    l.declare(bound("bill")).await?;
    let before = store.head().await.must("reading the head")?;
    let again = l
        .cat
        .declare(&l.own, typed("twice_of", "qty", "text"))
        .await;
    let e = refused_unmoved(
        store.as_ref(),
        before,
        "declaring `twice_of` again with a text input",
        again,
    )
    .await?;
    let case = json!({"code": "function_input_kind", "function": "twice_of", "input": "qty",
        "kind": "text", "type": "bill", "held": "number"});
    ensure!(
        invalid_like(&e, &case),
        "declaring `twice_of` again with a text input must be refused as {case}: {e:?}"
    );
    // A holder of `schema` not shown `bill`, or shown it and kept from
    // `qty`, is told nothing of the binding: the function is not its to
    // declare again (lakelands#917).
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    let mut kept = TypeGrants::default();
    kept.fields.insert("qty".into(), Access::None);
    let before = store.head().await.must("reading the head")?;
    for (id, grant) in [("blind", hidden), ("kept", kept)] {
        let mut grants = Grants {
            default: Access::Read,
            schema: true,
            review: true,
            ..Grants::default()
        };
        grants.types.insert(TypeName::from("bill"), grant);
        let who = principal(store.as_ref(), PrincipalKind::Human, id, grants);
        let declared = l.cat.declare(&who, typed("twice_of", "qty", "text")).await;
        let accepted = l.cat.accept(&who, &proposal).await.map(|_| ());
        for (door, answer) in [("declare", declared.map(|_| ())), ("accept", accepted)] {
            let what =
                format!("declaring `twice_of` again with a text input by `{door}`, as `{id}`");
            let e = refused_unmoved(store.as_ref(), before, &what, answer).await?;
            let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
            ensure!(
                !wire.contains("bill") && !wire.contains("qty"),
                "{what} must name nothing of `bill`: {wire}"
            );
        }
    }
    // Two inputs, one bound by a field the reader is shown and one by a
    // field it is kept from, both broken: nothing of the hidden one.
    let both = json!({"declare": "function", "name": "pair_of", "backing": "expr",
        "inputs": [{"name": "x", "type": {"kind": "number"}}, {"name": "y", "type": {"kind": "number"}}],
        "expr": "x + y"});
    l.declare_json(both).await?;
    let pair = vec![
        field("x", Number),
        field("y", Number),
        computed_by("sum", Number, "pair_of", &["x", "y"]),
    ];
    l.declare(type_decl("pair", pair)).await?;
    let mut grants = Grants {
        default: Access::Read,
        schema: true,
        ..Grants::default()
    };
    let mut kept = TypeGrants::default();
    kept.fields.insert("x".into(), Access::None);
    grants.types.insert(TypeName::from("pair"), kept);
    let who = principal(store.as_ref(), PrincipalKind::Human, "halfway", grants);
    let texts = json!({"declare": "function", "name": "pair_of", "backing": "expr",
        "inputs": [{"name": "x", "type": {"kind": "text"}}, {"name": "y", "type": {"kind": "text"}}],
        "expr": "x + y"});
    let texts: Declaration = serde_json::from_value(texts).expect("a function reads");
    let before = store.head().await.must("reading the head")?;
    let answer = l.cat.declare(&who, texts).await;
    let what = "declaring `pair_of` again with two text inputs, `x` kept from its author";
    let e = refused_unmoved(store.as_ref(), before, what, answer).await?;
    let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
    ensure!(
        !wire.contains("`x`") && !wire.contains("\"x\""),
        "{what} must name nothing of `x`: {wire}"
    );
    // The owner's accept is refused naming the type.
    let e = l.cat.accept(&l.own, &proposal).await;
    let e = refused_unmoved(store.as_ref(), before, "the owner's accept", e).await?;
    ensure!(
        invalid_like(&e, &case),
        "the owner's accept of `twice_of` as a text must be refused as {case}: {e:?}"
    );
    for (what, d) in [
        ("with no `type`", untyped("twice_of", "qty", "qty * 2")),
        (
            "with the kind `bill` holds",
            typed("twice_of", "qty", "number"),
        ),
        (
            "with no `type` again",
            untyped("twice_of", "qty", "qty * 2"),
        ),
    ] {
        let taken = l.cat.declare(&l.own, d).await;
        let taken = taken.must(&format!("declaring `twice_of` again {what}"))?;
        ensure!(
            matches!(taken, Outcome::Applied { .. }),
            "declaring `twice_of` again {what} must be applied"
        );
    }
    Ok(())
}
