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
        declared: None,
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

/// A write keeps what its writer is not told of the value it replaces,
/// after what it writes; a host's key and a setting naming nothing hidden
/// take what is written.
#[test]
fn a_write_keeps_what_its_writer_is_not_told() {
    let hidden = |_: &str, f: &str| f == "m";
    let kept = |key: &str, stored: Value, written: Value| kept(key, &stored, written, &hidden);
    let row = "sort_index.row";
    assert_eq!(
        kept(row, json!(["n", "m"]), json!(["k", "n"])),
        json!(["k", "n", "m"])
    );
    assert_eq!(kept(row, json!(["m", "n"]), Value::Null), json!(["m"]));
    assert_eq!(kept(row, json!(["n"]), json!(["k"])), json!(["k"]));
    assert_eq!(kept(row, json!(["n"]), Value::Null), Value::Null);
    let weights = "search_weights.row";
    assert_eq!(
        kept(
            weights,
            json!({"fields": {"m": 1.0, "n": 2.0}}),
            json!({"text_weight": 0.5})
        ),
        json!({"text_weight": 0.5, "fields": {"m": 1.0}})
    );
    assert_eq!(
        kept(weights, json!({"fields": {"m": 1.0}}), Value::Null),
        json!({"fields": {"m": 1.0}})
    );
    assert_eq!(kept("threshold", json!(["m"]), json!(1)), json!(1));
    // One it names itself, shown it masked, stands once, as written.
    assert_eq!(
        kept(row, json!(["m", "n"]), json!(["m", "k"])),
        json!(["m", "k"])
    );
    assert_eq!(
        kept(
            weights,
            json!({"fields": {"m": 1.0}}),
            json!({"fields": {"m": 2.0}})
        ),
        json!({"fields": {"m": 2.0}})
    );
}

/// A `search_weights` setting gives only the weights a search could give,
/// whatever a log that came another way holds (lakelands#1230).
#[test]
fn a_search_weight_out_of_range_weighs_nothing() {
    let value = json!({
        "fields": {"v": 0.5, "w": -1},
        "text_fields": {"title": 4, "body": 0, "note": 2e6},
        "text_weight": "high"
    });
    let fields = BTreeMap::from([("v".to_owned(), 0.5)]);
    assert_eq!(crate::seam::search_weights(Some(&value)), (fields, None));
    let text = BTreeMap::from([("title".to_owned(), 4.0)]);
    assert_eq!(crate::seam::text_field_weights(Some(&value)), text);
    let narrowed = json!({"fields": {"m": 1.0}, "text_fields": {"m": 2.0}, "text_weight": 0.5});
    let told = |_: &str| true;
    let reads = |_: &str, f: &str| f != "m";
    assert_eq!(
        super::narrowed("search_weights.row", narrowed, &told, &reads),
        Some(json!({"text_weight": 0.5}))
    );
    let hidden = |_: &str, f: &str| f == "m";
    let stored = json!({"text_fields": {"m": 2.0, "n": 1.0}});
    assert_eq!(
        kept(
            "search_weights.row",
            &stored,
            json!({"fields": {"v": 1.0}}),
            &hidden
        ),
        json!({"fields": {"v": 1.0}, "text_fields": {"m": 2.0}})
    );
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

/// A source's key period is named by the source, overrides the lake's,
/// and is told as the lake's is, whatever types its reader is shown.
#[test]
fn a_source_s_key_period_overrides_the_lake_s() {
    let named = |k: &str| lake_setting(k).map(|(s, of)| (s.key, of.map(str::to_owned)));
    assert_eq!(named("key_period"), Some(("key_period", None)));
    assert_eq!(
        named("key_period.crm.eu"),
        Some(("key_period", Some("crm.eu".into())))
    );
    let (s, _) = lake_setting("key_period").unwrap();
    let mut settings = BTreeMap::new();
    assert_eq!(value_of(&settings, s, Some("crm")), Value::Null);
    settings.insert("key_period".to_owned(), json!("month"));
    settings.insert("key_period.crm".to_owned(), json!("day"));
    assert_eq!(value_of(&settings, s, Some("crm")), json!("day"));
    assert_eq!(value_of(&settings, s, Some("erp")), json!("month"));
    let told = SettingValue {
        key: "key_period.crm".into(),
        value: Some(json!("day")),
        default: json!("month"),
        declared: None,
    };
    let none = |_: &str| false;
    let told = seen(told, &none, &|_: &str, _: &str| false).and_then(|s| s.value);
    assert_eq!(told, Some(json!("day")), "a source names no type");
}
