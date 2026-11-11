//! `extract(text, pattern)` answers the leftmost text the pattern
//! matches, or null where it matches none
//! (`docs/decisions/112-an-expression-extracts-the-text-a-pattern-matches.md`).

use crate::conformance::fixture::derived;
use crate::conformance::prelude::*;
use lakelands_api::ontology::StorageKind::{Number, Text};

check!(
    EXTRACT = an_expression_extracts_the_text_a_pattern_matches,
    "112 (lakelands#1696): `extract(s, p)` answers the leftmost text `p` matches in `s` and null where it matches none or `s` is null; among matches at that offset the first alternative wins and a greedy quantifier takes as much as it can; an empty match is the empty text; `s` is read as text, a number as its written form; a pattern that is no quoted literal is refused `extract_takes_literal` and one that is no I-Regexp `not_i_regexp`, both as the declaration is checked, and a read over a type that holds no records is refused `not_i_regexp`"
);

fn text_of<'r>(r: &'r Record, field: &str) -> Option<&'r str> {
    value(r, field).and_then(Value::as_str)
}

async fn an_expression_extracts_the_text_a_pattern_matches(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let page = type_decl(
        "page",
        vec![
            field("s", Text),
            field("n", Number),
            derived("first", Text, "extract(s, 'a|ab')", &["s"]),
            derived("ordered", Text, "extract(s, 'ab|a')", &["s"]),
            derived("greedy", Text, "extract(s, 'a+')", &["s"]),
            derived("none", Text, "extract(s, 'z+')", &["s"]),
            derived("empty", Text, "extract(s, 'a*')", &["s"]),
            derived("digits", Text, "extract(n, '[0-9]+')", &["n"]),
        ],
    );
    l.declare(page).await?;
    l.upsert("page", "r1", json!({"s": "xab", "n": 42})).await?;
    l.upsert("page", "r2", json!({"s": "xxaaab", "n": 7}))
        .await?;
    l.upsert("page", "r3", json!({"n": 0})).await?;

    // `xab`: the leftmost match begins at the `a`, the first alternative
    // wins where two begin there, a greedy quantifier takes all it can,
    // and an empty match at offset 0 is `''`.
    let r = l.read("page", "r1").await?;
    let got = (
        text_of(&r, "first"),
        text_of(&r, "ordered"),
        text_of(&r, "greedy"),
        text_of(&r, "none"),
        text_of(&r, "empty"),
        text_of(&r, "digits"),
    );
    let want = (Some("a"), Some("ab"), Some("a"), None, Some(""), Some("42"));
    ensure!(
        got == want,
        "`extract` on `xab` must answer first `a`, ordered `ab`, greedy `a`, none `null`, \
         empty `''` and a number 42 as `42`; it answered {got:?}"
    );

    // `xxaaab`: the search finds the match at offset 2, and the greedy
    // quantifier takes every `a` there.
    let r = l.read("page", "r2").await?;
    let got = (
        text_of(&r, "first"),
        text_of(&r, "ordered"),
        text_of(&r, "greedy"),
        text_of(&r, "none"),
        text_of(&r, "empty"),
        text_of(&r, "digits"),
    );
    let want = (Some("a"), Some("a"), Some("aaa"), None, Some(""), Some("7"));
    ensure!(
        got == want,
        "`extract` on `xxaaab` must answer first `a`, ordered `a`, greedy `aaa`, none `null`, \
         empty `''` and a number 7 as `7`; it answered {got:?}"
    );

    // A null `s` answers null from every pattern over it, and a number 0
    // is read as `0`.
    let r = l.read("page", "r3").await?;
    let nulls = (
        text_of(&r, "first"),
        text_of(&r, "ordered"),
        text_of(&r, "greedy"),
        text_of(&r, "none"),
        text_of(&r, "empty"),
    );
    ensure!(
        nulls == (None, None, None, None, None),
        "a record whose `s` is null must answer null from every pattern over it: {nulls:?}"
    );
    ensure!(
        text_of(&r, "digits") == Some("0"),
        "a number 0 must be read as `0`: {:?}",
        text_of(&r, "digits")
    );

    // The pattern must be a quoted literal, and the check refuses it
    // before any record runs.
    let unread = type_decl(
        "unread",
        vec![
            field("s", Text),
            derived("got", Text, "extract(s, s)", &["s"]),
        ],
    );
    let e = l
        .try_declare(unread)
        .await
        .must_fail("declaring a field whose extract takes a computed pattern")?;
    ensure!(
        invalid_like(&e, &json!({"code": "extract_takes_literal"})),
        "a pattern that is no quoted literal must be refused `extract_takes_literal`: {e:?}"
    );

    let unusable = type_decl(
        "unusable",
        vec![
            field("s", Text),
            derived("got", Text, "extract(s, '[z-a]')", &["s"]),
        ],
    );
    let e = l
        .try_declare(unusable)
        .await
        .must_fail("declaring a field whose extract pattern is no I-Regexp")?;
    ensure!(
        invalid_like(&e, &json!({"code": "not_i_regexp", "pattern": "[z-a]"})),
        "a pattern outside the grammar must be refused `not_i_regexp`: {e:?}"
    );

    // A read over a type that holds no records is refused when it is
    // checked: the pattern is known before any record runs.
    l.declare(type_decl("blank", vec![field("s", Text)]))
        .await?;
    let read = l
        .cat
        .find(
            &l.own,
            &"blank".into(),
            find_where("extract(s, '[z-a]') = 'x'"),
        )
        .await;
    ensure!(
        matches!(invalid_case(&read), Some(Case::NotIRegexp { .. })),
        "a read whose filter carries an unusable extract pattern must be refused \
         `not_i_regexp` over a type that holds no records: {read:?}"
    );
    Ok(())
}
