use super::*;

fn text(name: &str) -> FieldDecl {
    serde_json::from_value(json!({"name": name, "type": {"kind": "text"}})).unwrap()
}

/// The vector `docs/log-format.md` §2.5 pins.
#[test]
fn a_calls_digest_is_the_one_the_log_format_pins() {
    let geocode = FunctionDecl {
        name: "geocode".into(),
        backing: "http".into(),
        inputs: vec![text("address"), text("country")],
        expr: None,
        config: json!({"url": "https://geo.example/lookup"}),
        description: None,
    };
    let fields: BTreeMap<FieldName, Value> = [
        ("address".into(), json!("Hauptstraße 1")),
        ("unrelated".into(), json!("not an input")),
    ]
    .into();
    let call = call_of(&geocode, &fields);
    assert_eq!(
        call,
        json!({"function": "geocode",
                   "inputs": {"address": "Hauptstraße 1", "country": null},
                   "config": {"url": "https://geo.example/lookup"}})
    );
    assert_eq!(
        digest(&call).0,
        "a4702edebf8e4f34b65be7b55b963f930ac4e6f13d0b9a8773ca942814bc77aa"
    );
    assert_eq!(seam(&geocode), "function:http:geocode");
}

/// The embedding vector `docs/log-format.md` §2.5 pins: the text the
/// `from` fields join to, empty ones left out, and its call's digest.
#[test]
fn an_embeddings_text_and_digest_are_the_ones_the_log_format_pins() {
    let e = EmbedDecl {
        from: vec!["title".into(), "subtitle".into(), "summary".into()],
        model: "text-embedding-3-small".into(),
    };
    let fields: BTreeMap<FieldName, Value> = [
        ("title".into(), json!("Harbour crane")),
        ("subtitle".into(), json!("")),
        ("summary".into(), json!("A crane on the quay.")),
    ]
    .into();
    let text = embed_text(&e, &fields).unwrap();
    assert_eq!(text, "Harbour crane\n\nA crane on the quay.");
    let call = embed_call(&text);
    assert_eq!(
        String::from_utf8(lakelands_core::canonical::to_vec(&call)).unwrap(),
        r#"{"texts":["Harbour crane\n\nA crane on the quay."]}"#
    );
    assert_eq!(
        digest(&call).0,
        "6ff3bba9eed9f7899ca52fb42794fb32c315850b3bc9470dab44481773826ed0"
    );
    assert_eq!(embed_seam(&e.model), "embed:text-embedding-3-small");
    assert_eq!(embed_text(&e, &BTreeMap::new()), None);
}
