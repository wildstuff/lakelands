use super::lex::{RESERVED, Tok, lex};
use lakelands_api::error::Case;

fn tokens(statement: &str) -> Result<Vec<(Tok, usize, usize)>, Case> {
    let chars: Vec<char> = statement.chars().collect();
    let tokens = lex(&chars)?;
    Ok(tokens.into_iter().map(|t| (t.tok, t.at, t.end)).collect())
}

fn word(s: &str) -> Tok {
    Tok::Word(s.to_owned())
}

#[test]
fn a_token_stands_at_its_character_offsets() {
    let got = tokens("MATCH (é:bill)<-[:x*1..3]-() RETURN é.a<>1").expect("lexes");
    let want = vec![
        (word("MATCH"), 0, 5),
        (Tok::Punct("("), 6, 7),
        (word("é"), 7, 8),
        (Tok::Punct(":"), 8, 9),
        (word("bill"), 9, 13),
        (Tok::Punct(")"), 13, 14),
        (Tok::Punct("<"), 14, 15),
        (Tok::Punct("-"), 15, 16),
        (Tok::Punct("["), 16, 17),
        (Tok::Punct(":"), 17, 18),
        (word("x"), 18, 19),
        (Tok::Punct("*"), 19, 20),
        (Tok::Num("1".to_owned()), 20, 21),
        (Tok::Punct(".."), 21, 23),
        (Tok::Num("3".to_owned()), 23, 24),
        (Tok::Punct("]"), 24, 25),
        (Tok::Punct("-"), 25, 26),
        (Tok::Punct("("), 26, 27),
        (Tok::Punct(")"), 27, 28),
        (word("RETURN"), 29, 35),
        (word("é"), 36, 37),
        (Tok::Punct("."), 37, 38),
        (word("a"), 38, 39),
        (Tok::Punct("<>"), 39, 41),
        (Tok::Num("1".to_owned()), 41, 42),
        (Tok::End, 42, 42),
    ];
    assert_eq!(got, want);
}

#[test]
fn a_number_reads_a_fraction_but_not_a_range() {
    let got = tokens("1.5 1..2").expect("lexes");
    let toks: Vec<Tok> = got.into_iter().map(|t| t.0).collect();
    let want = vec![
        Tok::Num("1.5".to_owned()),
        Tok::Num("1".to_owned()),
        Tok::Punct(".."),
        Tok::Num("2".to_owned()),
        Tok::End,
    ];
    assert_eq!(toks, want);
}

#[test]
fn a_letter_run_into_a_number_is_refused_where_it_starts() {
    assert_eq!(
        tokens("x > 1e3"),
        Err(Case::ParseNumberIntoName { offset: 5 })
    );
    assert_eq!(tokens("0x1f"), Err(Case::ParseNumberIntoName { offset: 1 }));
    assert_eq!(
        tokens("2_000"),
        Err(Case::ParseNumberIntoName { offset: 1 })
    );
}

#[test]
fn a_string_unescapes() {
    let got = tokens(r#"'it\'s' "a \"b\"" 'é\n\t\\' "x'y""#).expect("lexes");
    let toks: Vec<Tok> = got.into_iter().map(|t| t.0).collect();
    let want = vec![
        Tok::Str("it's".to_owned()),
        Tok::Str("a \"b\"".to_owned()),
        Tok::Str("é\n\t\\".to_owned()),
        Tok::Str("x'y".to_owned()),
        Tok::End,
    ];
    assert_eq!(toks, want);
}

#[test]
fn a_bad_escape_or_an_open_string_is_refused() {
    assert_eq!(
        tokens(r"'a\qb'"),
        Err(Case::ParseUnexpected {
            found: 'q',
            offset: 2
        })
    );
    assert_eq!(
        tokens(r"'\u12'"),
        Err(Case::ParseUnexpected {
            found: 'u',
            offset: 1
        })
    );
    assert_eq!(tokens("x = 'abc"), Err(Case::ParseUnclosed { offset: 8 }));
    let token = "\"".to_owned();
    assert_eq!(
        tokens("x = \"abc"),
        Err(Case::ParseExpected { token, offset: 8 })
    );
}

#[test]
fn a_backticked_name_is_a_word_no_keyword_can_be() {
    let got = tokens("`MATCH` `a``b` `two words`").expect("lexes");
    let toks: Vec<Tok> = got.into_iter().map(|t| t.0).collect();
    let want = vec![
        Tok::Quoted("MATCH".to_owned()),
        Tok::Quoted("a`b".to_owned()),
        Tok::Quoted("two words".to_owned()),
        Tok::End,
    ];
    assert_eq!(toks, want);
    let token = "`".to_owned();
    assert_eq!(
        tokens("`open"),
        Err(Case::ParseExpected { token, offset: 5 })
    );
}

#[test]
fn a_parameter_is_named_bare_or_backticked() {
    let got = tokens("$a $`b c` $1").expect("lexes");
    let toks: Vec<(Tok, usize, usize)> = got;
    let want = vec![
        (Tok::Param("a".to_owned()), 0, 2),
        (Tok::Param("b c".to_owned()), 3, 9),
        (Tok::Param("1".to_owned()), 10, 12),
        (Tok::End, 12, 12),
    ];
    assert_eq!(toks, want);
    assert_eq!(
        tokens("x = $ y"),
        Err(Case::ParseUnexpected {
            found: '$',
            offset: 4
        })
    );
}

#[test]
fn comments_are_skipped_and_an_open_one_is_refused() {
    let got = tokens("a // to the end\n/* over\nlines */ b").expect("lexes");
    let want = vec![(word("a"), 0, 1), (word("b"), 33, 34), (Tok::End, 34, 34)];
    assert_eq!(got, want);
    let token = "*/".to_owned();
    assert_eq!(
        tokens("a /* open"),
        Err(Case::ParseExpected { token, offset: 9 })
    );
}

#[test]
fn a_character_no_token_starts_with_is_refused() {
    assert_eq!(
        tokens("a ! b"),
        Err(Case::ParseUnexpected {
            found: '!',
            offset: 2
        })
    );
    assert_eq!(
        tokens("a & b"),
        Err(Case::ParseUnexpected {
            found: '&',
            offset: 2
        })
    );
}

#[test]
fn the_reserved_words_are_sorted_upper_case_and_distinct() {
    let mut sorted = RESERVED.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, RESERVED.to_vec());
    assert!(
        RESERVED
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_uppercase()))
    );
}

#[test]
fn every_operator_word_is_reserved_in_any_case() {
    let words = [
        "and", "Or", "XOR", "xor", "not", "IN", "is", "starts", "ends", "contains",
    ];
    for w in words {
        assert!(super::parse::reserved(w), "{w}");
    }
}
