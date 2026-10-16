use serde_json::json;

use super::*;
use crate::location::{Limits, PgTuning, SqliteTuning};

/// The listed settings under `prefix`, each with its default.
fn listed(prefix: &str) -> BTreeMap<String, Value> {
    (SETTINGS.iter())
        .filter_map(|s| Some((s.key.strip_prefix(prefix)?.to_owned(), s.default_value())))
        .collect()
}

/// What a struct holds by default, member by member.
fn held(v: impl Serialize) -> BTreeMap<String, Value> {
    match serde_json::to_value(v).expect("it serialises") {
        Value::Object(m) => m.into_iter().collect(),
        other => panic!("not an object: {other}"),
    }
}

/// `Limits` and the engines' tuning are the list's location settings,
/// member for member and default for default, both ways: one added to
/// either and not to the other is seen here.
#[test]
fn the_location_s_settings_are_the_list_s() {
    assert_eq!(held(Limits::default()), listed("limits."));
    let mut tuning = held(SqliteTuning::DEFAULT);
    tuning.extend(held(PgTuning::default()));
    assert_eq!(tuning, listed("engine.tuning."));
    let location = SETTINGS.iter().filter(|s| s.layer == Layer::Location);
    assert_eq!(
        location.count(),
        listed("limits.").len() + listed("engine.tuning.").len()
    );
}

/// Every listed setting says what it changes and what values it takes,
/// and its default reads as JSON.
#[test]
fn every_setting_says_what_it_changes() {
    for s in SETTINGS {
        assert!(!s.says.is_empty() && !s.range.is_empty(), "{}", s.key);
        let _ = s.default_value();
    }
}

/// A key names its setting and type; a host's own key names none.
#[test]
fn a_key_names_its_setting_and_its_type() {
    let named = |k: &str| lake_setting(k).map(|(s, ty)| (s.key, ty.map(str::to_owned)));
    assert_eq!(
        named("sort_index.invoice"),
        Some(("sort_index", Some("invoice".into())))
    );
    assert_eq!(
        named("sort_index"),
        None,
        "a per-type setting has no lake-wide value"
    );
    assert_eq!(named("sort_indexes.invoice"), None);
    assert_eq!(named("discount_days"), None, "a host's own key");
}

/// A per-type value overrides the lake's, which overrides the default.
#[test]
fn a_type_s_value_overrides_the_lake_s() {
    let s = lake("example.choose", PerType::Override, "true");
    let mut settings = BTreeMap::new();
    assert_eq!(value_of(&settings, &s, Some("invoice")), json!(true));
    settings.insert("example.choose".to_owned(), json!(false));
    assert_eq!(value_of(&settings, &s, Some("invoice")), json!(false));
    settings.insert("example.choose.invoice".to_owned(), json!(true));
    assert_eq!(value_of(&settings, &s, Some("invoice")), json!(true));
    assert_eq!(value_of(&settings, &s, Some("partner")), json!(false));
    assert_eq!(value_of(&settings, &s, None), json!(false));
}

/// A key under a listed setting's prefix that names none of them is
/// the store's, unlisted; a host's own key is not.
#[test]
fn a_key_under_the_store_s_prefix_is_listed_or_unlisted() {
    assert_eq!(unlisted("sort_index.invoice"), None);
    assert_eq!(
        unlisted("sort_index"),
        Some("sort_index"),
        "a per-type setting needs its type"
    );
    assert_eq!(unlisted("view_kept"), Some("view_kept"));
    assert_eq!(unlisted("sort_indexes.invoice"), None, "a host's own key");
    assert_eq!(unlisted("discount_days"), None);
}

/// A setting is told only of a type its reader is shown, naming only the
/// fields it reads, and not at all where it would name none.
#[test]
fn a_setting_is_told_as_its_reader_sees_the_type() {
    let at = |key: &str, value: Value| SettingValue {
        key: key.into(),
        value: Some(value),
        default: Value::Null,
    };
    let shown = |ty: &str| ty != "secret";
    let reads = |_: &str, f: &str| f != "m";
    let seen = |s| seen(s, &shown, &reads).and_then(|s| s.value);
    assert_eq!(
        seen(at("sort_index.row", json!(["n", "m"]))),
        Some(json!(["n"]))
    );
    assert_eq!(seen(at("sort_index.row", json!(["m"]))), None);
    assert_eq!(seen(at("sort_index.secret", json!(["x"]))), None);
    let weights = json!({"fields": {"m": 1.0}, "text_weight": 0.5});
    assert_eq!(
        seen(at("search_weights.row", weights)),
        Some(json!({"text_weight": 0.5}))
    );
    assert_eq!(
        seen(at("search_weights.row", json!({"fields": {"m": 1.0}}))),
        None
    );
    assert_eq!(seen(at("view_kept.row", json!(true))), Some(json!(true)));
}

/// What a location holds beside its settings is the list's location
/// bindings, both ways: the engines' members and the key's handle.
#[test]
fn the_location_s_bindings_are_the_list_s() {
    use crate::location::{EngineConfig, KeyHandle, Location};
    let mut pg = Location::postgres("l", "postgres://h/d");
    if let EngineConfig::Postgres { schema, .. } = &mut pg.engine {
        *schema = Some("s".into());
    }
    pg.key = Some(KeyHandle("k".into()));
    let mut members = std::collections::BTreeSet::new();
    for l in [Location::sqlite("l", "d"), pg] {
        let v = serde_json::to_value(l).expect("it serialises");
        for (k, _) in v.as_object().expect("an object") {
            if !matches!(k.as_str(), "lake" | "engine" | "limits") {
                members.insert(k.clone());
            }
        }
        for (k, _) in v["engine"].as_object().expect("an object") {
            if !matches!(k.as_str(), "engine" | "tuning") {
                members.insert(format!("engine.{k}"));
            }
        }
    }
    let bound = BINDINGS.iter().filter(|b| b.layer == Layer::Location);
    let listed: std::collections::BTreeSet<String> = bound.map(|b| b.key.to_owned()).collect();
    assert_eq!(members, listed);
}
