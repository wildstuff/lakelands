//! `lakelands settings`: every listed setting with the value it has for
//! one lake, and a lake setting written or retired through the store, so
//! the store's own judge refuses a value and names why
//! (`docs/decisions/063-every-setting-is-in-one-list.md`). A setting or a
//! binding of the location or the root is kept in its file, and the
//! command names the file instead of writing it.

use std::io::Write;

use clap::ArgMatches;
use lakelands_api::describe::LakeDescription;
use lakelands_api::location::{EngineConfig, Location};
use lakelands_api::seam::SETTING_TYPE;
use lakelands_api::settings::{BINDINGS, Layer, PerType, SETTINGS, Setting};
use lakelands_api::{Catalog, Principal, Value};

use crate::{Config, Failure};

/// For `settings set` or `unset` of a key the location or the root keeps,
/// the failure naming the file it is kept in: its full key, the last parts
/// of one, or a section's name, which written to the lake would be a
/// host's own key that nothing reads. `lake` is the lake's name where it
/// is known.
pub fn elsewhere(m: &ArgMatches, lake: Option<String>) -> Option<Failure> {
    let ("settings", sub) = m.subcommand()? else {
        return None;
    };
    let (_, change) = sub.subcommand()?;
    let key = change.get_one::<String>("key")?;
    let kept = (SETTINGS.iter().map(|s| (s.key, s.layer)))
        .chain(BINDINGS.iter().map(|b| (b.key, b.layer)))
        .filter(|(_, layer)| *layer != Layer::Lake);
    let named = |full: &str| full == key || full.ends_with(&format!(".{key}"));
    let file = |layer: Layer| match layer {
        Layer::Location => format!("lakes/{}.json", lake.as_deref().unwrap_or("<id>")),
        _ => "lakelands.json".to_owned(),
    };
    let found: Vec<(&str, String)> = (kept.clone())
        .filter(|(full, _)| named(full))
        .map(|(full, layer)| (full, file(layer)))
        .collect();
    let again = |file: &str| match file.starts_with("lakes/") {
        true => "open the lake again",
        false => "open the root again",
    };
    let section = (kept.into_iter())
        .find(|(full, _)| format!(".{full}").contains(&format!(".{key}.")))
        .map(|(_, layer)| file(layer));
    let (what, todo) = match found.as_slice() {
        [] => {
            let file = section?;
            let todo = format!("edit its members in {file} and {}", again(&file));
            (
                format!("`{key}` is not a lake setting: it is a section of {file}"),
                todo,
            )
        }
        [(full, file)] => {
            let todo = format!("edit `{full}` in {file} and {}", again(file));
            match *full == key {
                true => (
                    format!("`{key}` is not a lake setting: it is kept in {file}"),
                    todo,
                ),
                false => (
                    format!("`{key}` is not a lake setting: `{full}` is kept in {file}"),
                    todo,
                ),
            }
        }
        many => {
            let each: Vec<String> = many
                .iter()
                .map(|(f, file)| format!("`{f}` in {file}"))
                .collect();
            let what = format!(
                "`{key}` is not a lake setting: {} are kept so",
                each.join(" and ")
            );
            (
                what,
                "edit the one you mean in its file and open it again".to_owned(),
            )
        }
    };
    Some(Failure::new(what, todo))
}

/// The key `settings set` and `unset` write: `<key>.<type>` with a type.
pub fn key_of(change: &ArgMatches) -> String {
    let key = change.get_one::<String>("key").cloned().unwrap_or_default();
    match change.get_one::<String>("type") {
        Some(ty) => format!("{key}.{ty}"),
        None => key,
    }
}

/// `settings set`: writes `raw`, read as JSON or else as a text, to the
/// lake setting `key`; `settings unset` (`raw` absent) retires it.
pub async fn change(
    out: &mut dyn Write,
    catalog: &dyn Catalog,
    who: &Principal,
    key: &str,
    raw: Option<&str>,
) -> Result<(), Failure> {
    let (ty, record) = (SETTING_TYPE.into(), key.into());
    let Some(raw) = raw else {
        let outcome = catalog
            .retire(who, &ty, &record, Default::default())
            .await?;
        let at = outcome.position();
        return line(out, format!("retired `{key}` at position {at}"));
    };
    let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    let patch = [("key".into(), key.into()), ("value".into(), value.clone())].into();
    let outcome = (catalog.upsert(who, &ty, &record, patch, Default::default())).await?;
    let at = outcome.position();
    line(out, format!("set `{key}` to {value} at position {at}"))
}

/// `settings`: one line per listed setting: its key, layer and value,
/// and its default, or `(default)` where it has its default. A lake
/// setting is told to a holder of `schema`; a per-type one shows each
/// type the lake holds it for.
pub fn list(
    out: &mut dyn Write,
    lake: &LakeDescription,
    location: &Location,
    root: &Config,
) -> Result<(), Failure> {
    let mut rows: Vec<(String, &str, String)> = Vec::new();
    let shown = |value: &Value, default: &Value, held: bool| match held && value != default {
        true => format!("{value} (default {default})"),
        false => format!("{value} (default)"),
    };
    for s in SETTINGS.iter().filter(|s| s.layer == Layer::Lake) {
        if s.per_type == PerType::Only {
            let default = s.default_value();
            rows.push((
                format!("{}.<type>", s.key),
                "lake",
                shown(&default, &default, false),
            ));
        }
        let of =
            |key: &str| key == s.key || key.strip_prefix(s.key).is_some_and(|t| t.starts_with('.'));
        for t in lake.settings.iter().filter(|t| of(&t.key)) {
            let value = t.value.as_ref().unwrap_or(&t.default);
            rows.push((
                t.key.clone(),
                "lake",
                shown(value, &t.default, t.value.is_some()),
            ));
        }
    }
    if !lake.may.schema {
        rows.retain(|(_, layer, _)| *layer != "lake");
        line(
            out,
            "lake settings are told to a holder of the schema grant",
        )?;
    }
    // A host's own settings, with their group, kind and unit, as the lake
    // tells them to a holder of `schema` and to their writers.
    for t in &lake.settings {
        let Some(d) = &t.declared else {
            continue;
        };
        let kind =
            d.ty.value_type
                .as_ref()
                .map_or_else(|| d.ty.kind.to_string(), ToString::to_string);
        let unit = d
            .unit
            .as_deref()
            .map(|u| format!(", in {u}"))
            .unwrap_or_default();
        let group = d
            .group
            .as_deref()
            .map(|g| format!("group {g}, "))
            .unwrap_or_default();
        let value = t
            .value
            .as_ref()
            .map_or_else(|| "not set".to_owned(), ToString::to_string);
        rows.push((
            t.key.clone(),
            "host",
            format!("{value} ({group}{kind}{unit})"),
        ));
    }
    let tuning = match &location.engine {
        EngineConfig::Sqlite { tuning, .. } => serde_json::to_value(tuning),
        EngineConfig::Postgres { tuning, .. } => serde_json::to_value(tuning),
        _ => Ok(Value::Null),
    };
    let held = [
        ("limits.", serde_json::to_value(location.limits)),
        ("engine.tuning.", tuning),
        ("server.", serde_json::to_value(&root.server)),
        ("aging.", serde_json::to_value(&root.aging)),
        ("deliver.", serde_json::to_value(&root.deliver)),
        ("embed.", serde_json::to_value(&root.embed)),
        ("backings.", serde_json::to_value(&root.backings)),
        ("mirror.", serde_json::to_value(&root.mirror)),
        ("standby.", serde_json::to_value(&root.standby)),
    ];
    let elsewhere = SETTINGS.iter().filter(|s| s.layer != Layer::Lake);
    for s in elsewhere {
        let Some(value) = member(s, &held) else {
            continue;
        };
        let section = s.key.rsplit_once('.').map_or(s.key, |(section, _)| section);
        let value = match value {
            Some(v) => shown(&v, &s.default_value(), true),
            None => format!("not configured: lakelands.json has no `{section}`"),
        };
        let layer = match s.layer {
            Layer::Location => "location",
            _ => "root",
        };
        rows.push((s.key.to_owned(), layer, value));
    }
    let width = rows.iter().map(|(k, ..)| k.len()).max().unwrap_or(0);
    for (key, layer, value) in rows {
        line(out, format!("{key:width$}  {layer:8}  {value}"))?;
    }
    Ok(())
}

/// The value of the location's or root's setting `s` in `held`, by its
/// key's prefix and the path after it: `None` where the engine keeps no
/// such member, and `Some(None)` where the root has no such section.
fn member(s: &Setting, held: &[(&str, serde_json::Result<Value>)]) -> Option<Option<Value>> {
    let (rest, of) = held
        .iter()
        .find_map(|(p, v)| Some((s.key.strip_prefix(p)?, v)))?;
    let path = format!("/{}", rest.replace('.', "/"));
    match (of.as_ref().ok()?.pointer(&path), s.layer) {
        (Some(v), _) => Some(Some(v.clone())),
        (None, Layer::Root) => Some(None),
        (None, _) => None,
    }
}

fn line(out: &mut dyn Write, text: impl std::fmt::Display) -> Result<(), Failure> {
    writeln!(out, "{text}").map_err(|e| {
        Failure::new(
            format!("could not write output: {e}"),
            "check where the output goes",
        )
    })
}
