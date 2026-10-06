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

/// A derived field reads what its declaration is checked to read:
/// `previous` and `changed` read their field like any other function
/// (lakelands#652).
#[test]
fn a_derived_field_reads_the_field_previous_or_changed_names() {
    for (expr, reads) in [
        ("previous(b)", &["b"][..]),
        ("if(changed(a), b, c)", &["a", "b", "c"]),
    ] {
        let f: FieldDecl =
            serde_json::from_value(json!({"name": "x", "type": {"kind": "text"}, "derived": expr}))
                .unwrap();
        let reads = reads.iter().map(|r| FieldName::from(*r)).collect();
        let told = computed_reads(&Ontology::default(), &f);
        assert_eq!(told, Some(reads), "{expr}");
    }
}

/// A derived field whose reads cannot be told is completed as reading
/// every other field beside it, of its type or of its relation: closed,
/// never open.
#[test]
fn a_derived_field_whose_reads_cannot_be_told_reads_every_field_beside_it() {
    let x = |n: &str| json!({"name": n, "type": {"kind": "text"}, "derived": "sum(b)"});
    let mut t: TypeDecl = serde_json::from_value(json!({"name": "t",
        "fields": [{"name": "a", "type": {"kind": "text"}}, {"name": "b", "type": {"kind": "text"}}, x("x")],
        "relations": [{"name": "r", "target": "t",
                       "fields": [{"name": "n", "type": {"kind": "text"}}, x("e")]}]}))
    .unwrap();
    Ontology::default().complete(&mut t);
    let fields = t.fields.iter().chain(&t.relations[0].fields);
    let lists: Vec<_> = fields
        .map(|f| format!("{} {}", f.name, json!(f.depends_on)))
        .collect();
    assert_eq!(
        lists,
        ["a []", "b []", r#"x ["a","b"]"#, "n []", r#"e ["n"]"#]
    );
}
