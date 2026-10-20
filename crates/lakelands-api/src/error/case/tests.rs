use serde_json::json;

use super::*;
use crate::error::{Detail, Error, Refusal};

/// A refusal's case is written beside its places, its parameters and its
/// own sentence (the error's says the places too), and read back as it
/// was; a refusal of a kind this build does not know is read as
/// `Unknown`, said by its error's sentence, and written back as it came.
#[test]
fn a_said_case_and_an_unknown_refusal_are_read_back_as_they_were_written() {
    let case = Case::UnknownField {
        ty: "invoice".into(),
        field: "total".into(),
    };
    let measure = Place::Measure { name: "sum".into() };
    let view = Place::View {
        name: "revenue".into(),
    };
    let e = Error::invalid(Said::from(case).within(measure).within(view));
    let wire = serde_json::to_value(&e).unwrap();
    let sentence = "`invoice` has no field `total`";
    assert_eq!(
        wire["reason"],
        json!({
            "refusal": "invalid",
            "at": [
                {"code": "view", "name": "revenue", "message": "view `revenue`"},
                {"code": "measure", "name": "sum", "message": "measure `sum`"},
            ],
            "code": "unknown_field",
            "type": "invoice",
            "field": "total",
            "message": sentence,
        })
    );
    let said = "refused: view `revenue`, measure `sum`: `invoice` has no field `total`";
    assert_eq!(wire["message"], said);
    assert_eq!(serde_json::from_value::<Error>(wire).unwrap(), e);

    let newer = json!({"error": "refused", "reason": {"refusal": "newer"}, "message": "m"});
    let e = serde_json::from_value::<Error>(newer.clone()).unwrap();
    let Error::Refused {
        reason: Refusal::Unknown(u),
        field: None,
    } = &e
    else {
        panic!("{e:?} is no unknown refusal");
    };
    assert_eq!(u.code(), "newer");
    assert_eq!(e.to_string(), "m");
    assert_eq!(serde_json::to_value(&e).unwrap(), newer);
}

/// A template says a segment only when what it names is there, picks the
/// other side of `|` when not, and reads doubled braces as themselves.
#[test]
fn a_template_says_a_segment_only_when_its_parameters_are_there() {
    struct Said<'a>(&'a str, &'a [(&'a str, Option<String>)]);
    impl fmt::Display for Said<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            render(f, self.0, self.1)
        }
    }
    let t = "`{a}`[ at {b}][; {?c}flagged|; plain] {{x}}";
    let some = |v: &str| Some(v.to_owned());
    let all = [("a", some("n")), ("b", some("2")), ("c", some(""))];
    assert_eq!(Said(t, &all).to_string(), "`n` at 2; flagged {x}");
    let none = [("a", some("n")), ("b", None), ("c", None)];
    assert_eq!(Said(t, &none).to_string(), "`n`; plain {x}");
    let nested = "x[ {a}[ and {b}]|none]";
    assert_eq!(Said(nested, &none).to_string(), "x n");
    assert_eq!(Said(nested, &[("a", None)]).to_string(), "xnone");
}

/// Every case and every place of the catalog, as the gate writes one of
/// each (`samples.json`), is read back through a refusal as itself, by
/// the code the gate names, and written back as it was read.
#[test]
fn every_case_and_place_reads_back_as_itself_through_an_error() {
    let samples: serde_json::Value =
        serde_json::from_str(include_str!("samples.json")).expect("the samples are JSON");
    let place = json!({"code": "type", "name": "t"});
    let unknown_field = json!({"code": "unknown_field", "type": "t", "field": "f"});
    let reads = |case: &serde_json::Value, at: &serde_json::Value| {
        let mut reason = case.as_object().expect("a case is an object").clone();
        reason.insert("refusal".into(), json!("invalid"));
        reason.insert("at".into(), json!([at]));
        let wire = json!({"error": "refused", "reason": reason});
        let e: Error = serde_json::from_value(wire.clone()).expect("an error");
        let said = e.case().unwrap_or_else(|| panic!("no case in {wire}"));
        let known = !matches!(said.case, Case::Unknown(_));
        let placed = !said.at.iter().any(|p| matches!(p, Place::Unknown(_)));
        assert!(known && placed, "not read as itself: {wire}");
        let back = serde_json::to_value(&e).expect("written");
        assert_eq!(back["reason"]["code"], case["code"], "{wire}");
        assert_eq!(back["reason"]["at"][0]["code"], at["code"], "{wire}");
        let again: Error = serde_json::from_value(back).expect("read again");
        assert_eq!(again, e, "{wire}");
    };
    let (cases, places) = (&samples["Case"], &samples["Place"]);
    assert!(
        cases.as_array().is_some_and(|c| c.len() > 400),
        "the samples hold every case"
    );
    assert!(
        places.as_array().is_some_and(|p| p.len() > 20),
        "and every place"
    );
    for case in samples["Case"].as_array().expect("cases") {
        reads(case, &place);
    }
    for at in samples["Place"].as_array().expect("places") {
        reads(&unknown_field, at);
    }
}

/// A refusal that says what a declaration allows is shown by a listing
/// only beside the field it is said of (lakelands#791): a call's payload
/// lists each value, and no field whose reader could be asked.
#[test]
fn what_a_declaration_says_is_shown_only_beside_its_field() {
    let listed: crate::write::ProposedWrite = serde_json::from_value(
        json!({"write": "call", "verb": "grade", "key": "k", "payload": {"x": "mid"}}),
    )
    .unwrap();
    let shown = |field: Option<&str>| {
        let (value, allowed) = (json!("mid"), ["low", "high"].into_iter().collect());
        let mut e = Error::invalid(Case::NotOneOf { value, allowed });
        if let Error::Refused { field: f, .. } = &mut e {
            *f = field.map(Into::into);
        }
        let mut named = Named::default();
        e.names(&mut named);
        named.shown_by(&listed, &|_| true, &|_| true, &|_| None)
    };
    assert_eq!((shown(None), shown(Some("level"))), (false, true));
}

/// A declined call's `reason` is optional on the wire: a decline with
/// none writes no member, one with a reason reads back as it was written,
/// and a reader of the older form, which has no `reason`, reads a decline
/// without it.
#[test]
fn a_declined_calls_reason_is_optional_on_the_wire() {
    let verb = "judge".into();
    let plain = serde_json::to_value(Error::declined(&verb)).unwrap();
    assert!(plain["reason"].get("reason").is_none(), "{plain}");
    let said = Error::declined_because(&verb, Detail::scrubbed("über the ceiling – 上限"));
    let wire = serde_json::to_value(&said).unwrap();
    assert_eq!(wire["reason"]["reason"], "über the ceiling – 上限");
    assert_eq!(serde_json::from_value::<Error>(wire).unwrap(), said);
    assert_eq!(
        serde_json::from_value::<Error>(plain).unwrap(),
        Error::declined(&verb)
    );
}
