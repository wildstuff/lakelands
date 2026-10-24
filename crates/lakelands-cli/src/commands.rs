//! The commands that open a lake: `apply`, `ingest`, `feed`, `age`,
//! `purge`, `forget`, `values remove`, `rebuild`, `compact`, `export` and
//! `serve`. Each calls the library in-process and prints the position of
//! every answer that has one.
//! `import` makes a lake instead of opening one.
//!
//! What a command says of its operator's own files and terminal (a file it
//! cannot read, a CSV that does not parse, a map naming a column the file
//! lacks) is no refusal of the catalog: it is said to the operator at the
//! terminal, never answered by a lake. What the lake answers, and what the
//! lake shows a map to name, is said by its case.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use lakelands_api::error::Case;
use lakelands_api::id::{FieldName, Position, RecordKey, SourceId, TypeName};
use lakelands_api::ontology::StorageKind;
use lakelands_api::read::ReadAt;
use lakelands_api::record::{SourceRecord, Value};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{Catalog, LakeStore, Principal};
use lakelands_core::{bundle, canonical, export};
use serde::Deserialize;

use crate::say::{self, Failure};

type Out<'a> = &'a mut dyn Write;

fn read(file: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(file).map_err(|e| {
        Failure::new(
            format!("could not read {}: {e}", file.display()),
            "check the path and that you may read the file",
        )
    })
}

/// The words a run line adds for moves a constraint or the set ceiling
/// refused: none where none were.
pub(crate) fn refused_words(count: u32) -> String {
    match count {
        0 => String::new(),
        n => format!(", {n} refused"),
    }
}

pub(crate) fn put(out: Out<'_>, line: impl std::fmt::Display) -> Result<(), Failure> {
    writeln!(out, "{line}").map_err(|e| {
        Failure::new(
            format!("could not write output: {e}"),
            "check where the output goes",
        )
    })
}

/// A declaration's kind and name, for a person.
fn label(d: &Declaration) -> String {
    let v = serde_json::to_value(d).unwrap_or_default();
    let kind = v["declare"]
        .as_str()
        .unwrap_or("declaration")
        .replace('_', " ");
    match v["name"].as_str() {
        Some(name) => format!("{kind} `{name}`"),
        None => kind,
    }
}

/// `apply`: load an ontology bundle and apply what it holds as one set
/// (`Catalog::apply`). A bundle the loader refuses, or a set the lake
/// refuses, is refused whole, every problem named, and nothing is
/// declared.
pub async fn apply(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    (dir, plan_id): (&Path, Option<&str>),
) -> Result<(), Failure> {
    is_bundle(dir)?;
    apply_bundle(out, catalog, who, (bundle::load_dir(dir), plan_id)).await
}

/// Apply a bundle as `apply` does, from what its loader answered: a
/// directory's, or the files of an example this binary carries.
pub async fn apply_bundle(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    (loaded, plan_id): (Loaded, Option<&str>),
) -> Result<(), Failure> {
    let bundle = loaded.map_err(refused)?;
    // The bundle is applied as one set: validated together, appended in
    // one append at one position, or refused whole. A bundle of aging
    // rules alone declares nothing.
    let what: Vec<String> = bundle.declarations.iter().map(label).collect();
    if what.is_empty() && !bundle.aging_rules.is_empty() {
        return aging_rules(out, catalog, who, bundle.aging_rules).await;
    }
    let proposed;
    // Against its plan, where one is named (062).
    let applied = match plan_id {
        Some(id) => {
            let change = lakelands_api::write::Change::Declare {
                declarations: bundle.declarations,
            };
            catalog.enact(who, change, id).await
        }
        None => catalog.apply(who, bundle.declarations).await,
    };
    match applied {
        Ok(outcome) => {
            proposed = matches!(outcome, lakelands_api::Outcome::Proposed { .. });
            // Each declaration as it stands: one already in force says
            // since when, and a bundle wholly in force says it appended
            // nothing (lakelands#1106).
            let (declared, plan) = match &outcome {
                lakelands_api::Outcome::Applied { declared, plan, .. } => {
                    (declared.clone(), plan.clone())
                }
                lakelands_api::Outcome::Proposed { .. } => (Vec::new(), None),
            };
            let held = |i: usize| declared.get(i).filter(|d| !d.appended).map(|d| d.since);
            let header = match (0..what.len()).all(|i| held(i).is_some()) {
                true => format!(
                    "{} declaration(s) in force already: nothing appended",
                    what.len()
                ),
                false => format!(
                    "applied {} declaration(s), in force from position {}:",
                    what.len(),
                    outcome.position()
                ),
            };
            put(out, header)?;
            for (i, w) in what.into_iter().enumerate() {
                match held(i) {
                    Some(since) => put(out, format!("  {w}, in force since position {since}"))?,
                    None => put(out, format!("  {w}"))?,
                }
            }
            // What it did to stored records, where it did anything.
            for line in plan
                .iter()
                .filter(|p| !p.types.is_empty())
                .flat_map(|p| say::plan(p))
            {
                put(out, line)?;
            }
        }
        Err(e) => {
            let mut f = Failure::from(e);
            f.todo = "correct the bundle and apply it again; nothing was applied".to_owned();
            return Err(f);
        }
    }
    aging_rules(out, catalog, who, bundle.aging_rules).await?;
    if proposed {
        // A value is judged by its declaration, which is only proposed.
        for value in &bundle.settings {
            let name = value.get("key").and_then(Value::as_str).unwrap_or_default();
            put(
                out,
                format!(
                    "  setting `{name}`: its first value waits until the declarations are \
                     accepted; apply the bundle again then"
                ),
            )?;
        }
        return Ok(());
    }
    first_values(out, catalog, who, bundle.settings).await
}

/// Write each setting's first value the bundle gives, only where the lake
/// never held one: a value an operator wrote since stands, and one an
/// operator retired stays retired
/// (`docs/decisions/063-every-setting-is-in-one-list.md`).
async fn first_values(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    values: Vec<lakelands_api::Patch>,
) -> Result<(), Failure> {
    let ty = TypeName::from(lakelands_api::seam::SETTING_TYPE);
    for value in values {
        let name = value
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let key = RecordKey::from(name.as_str());
        let get = lakelands_api::Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
            params: Default::default(),
        };
        match catalog.get(who, &ty, get).await {
            Ok(_) => {
                put(
                    out,
                    format!("  setting `{name}` holds a value already, kept"),
                )?;
                continue;
            }
            Err(lakelands_api::Error::NotFound { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        let retired = lakelands_api::Find {
            // A quote in the key is doubled, as a text literal holds one.
            filter: Some(
                format!("key = '{}'", name.replace('\'', "''"))
                    .as_str()
                    .into(),
            ),
            retired: true,
            ..Default::default()
        };
        let found = catalog.find(who, &ty, retired).await?.data;
        if matches!(found, lakelands_api::read::Found::Records { retired: Some(n), .. } if n > 0) {
            put(
                out,
                format!("  setting `{name}` was unset since, kept unset"),
            )?;
            continue;
        }
        let outcome = (catalog
            .upsert(who, &ty, &key, value, Default::default())
            .await)
            .map_err(|e| {
                let mut f = Failure::from(e);
                f.what = format!("setting `{name}`: {}", f.what);
                f.todo =
                    "correct its value and apply the bundle again; the declarations were applied"
                        .to_owned();
                f
            })?;
        let at = outcome.position();
        put(
            out,
            format!("  setting `{name}` given its first value at position {at}"),
        )?;
    }
    Ok(())
}

/// `plan`: what applying a bundle would do, changing nothing, and the
/// plan's id to apply exactly that (`Catalog::plan`, 062).
pub async fn plan(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    dir: &Path,
) -> Result<(), Failure> {
    let bundle = bundle_at(dir)?;
    let change = lakelands_api::write::Change::Declare {
        declarations: bundle.declarations,
    };
    let plan = catalog.plan(who, change).await.map_err(|e| {
        let mut f = Failure::from(e);
        f.todo = "correct the bundle and plan it again; nothing was applied".to_owned();
        f
    })?;
    for line in say::plan(&plan.data) {
        put(out, line)?;
    }
    put(
        out,
        format!(
            "plan {}: apply it with `lakelands apply {} --plan {}`",
            plan.data.report_id,
            dir.display(),
            plan.data.report_id
        ),
    )
}

/// What a bundle's loader answers.
type Loaded = Result<bundle::Bundle, Vec<bundle::BundleError>>;

/// The bundle in `dir`, read whole.
fn bundle_at(dir: &Path) -> Result<bundle::Bundle, Failure> {
    is_bundle(dir)?;
    bundle::load_dir(dir).map_err(refused)
}

/// Refused unless `dir` is a directory a bundle can be read from.
fn is_bundle(dir: &Path) -> Result<(), Failure> {
    match dir.is_dir() {
        true => Ok(()),
        false => Err(Failure::new(
            format!("{} is not a bundle directory", dir.display()),
            "name the directory that holds the bundle's types/, verbs/ and other lanes",
        )),
    }
}

/// A bundle its loader refused, every problem named.
fn refused(problems: Vec<bundle::BundleError>) -> Failure {
    // One `error:` line per problem; Failure prints the first prefix.
    let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
    Failure::new(
        lines.join("\nerror: "),
        "correct the bundle and apply it again; nothing was applied",
    )
}

/// `recompute`: compute again what the named function or type in force
/// computes (`Catalog::recompute`), and say from which position it holds.
pub async fn recompute(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    function: Option<String>,
    ty: Option<String>,
) -> Result<(), Failure> {
    use lakelands_api::write::Recompute;
    let what = match (function, ty) {
        (Some(name), None) => Recompute::Function(name),
        (None, Some(name)) => Recompute::Type(name.as_str().into()),
        _ => {
            return Err(Failure::new(
                "recompute needs one of --function and --type",
                "name the function, or the type with a field computed outside the lake",
            ));
        }
    };
    let named = match &what {
        Recompute::Function(name) => format!("function `{name}`"),
        Recompute::Type(name) => format!("type `{name}`"),
    };
    let outcome = catalog.recompute(who, what).await?;
    put(
        out,
        format!(
            "computed {named} again, from position {}",
            outcome.position()
        ),
    )
}

/// Write a bundle's aging rules after its declarations, each as its
/// `sys.aging_rule` record, skipping a rule whose stored fields already
/// equal the file's. A field the stored rule holds and the file does not
/// is cleared.
async fn aging_rules(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    rules: Vec<lakelands_api::Patch>,
) -> Result<(), Failure> {
    let ty = TypeName::from(lakelands_api::seam::AGING_RULE_TYPE);
    for mut rule in rules {
        let name = rule
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let key = RecordKey::from(name.as_str());
        let get = lakelands_api::Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
            params: Default::default(),
        };
        let stored = match catalog.get(who, &ty, get).await {
            Ok(r) => Some(r.data.fields),
            Err(lakelands_api::Error::NotFound { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        normalise(catalog, who, &mut rule).await;
        if stored.as_ref() == Some(&rule) {
            put(out, format!("  aging rule `{name}` unchanged"))?;
            continue;
        }
        for f in stored.iter().flat_map(|s| s.keys()) {
            rule.entry(f.clone()).or_insert(Value::Null);
        }
        let outcome = catalog
            .upsert(who, &ty, &key, rule, Default::default())
            .await
            .map_err(|e| {
                let mut f = Failure::from(e);
                f.what = format!("aging rule `{name}`: {}", f.what);
                f.todo = "correct the rule and apply the bundle again; the declarations and \
                          the rules before it were applied"
                    .to_owned();
                f
            })?;
        put(
            out,
            format!(
                "  aging rule `{name}` written at position {}",
                outcome.position()
            ),
        )?;
    }
    Ok(())
}

/// A rule's `from` and `to` in the one form the lake stores them in, read
/// in the kind of the field they are values of, so that a file's `1.0` or
/// `2026-01-01` equals the stored `1` or date-time. Left as written when
/// the type or field cannot be read; the lake then checks them.
async fn normalise(catalog: &dyn Catalog, who: &Principal, rule: &mut lakelands_api::Patch) {
    let text = |k: &str| rule.get(k).and_then(Value::as_str).map(str::to_owned);
    let (Some(on), Some(field)) = (text("on"), text("field")) else {
        return;
    };
    let Ok(t) = catalog
        .describe_type(who, &TypeName::from(on.as_str()), ReadAt::default())
        .await
    else {
        return;
    };
    let Some(f) = t.data.fields.iter().find(|f| f.name.as_str() == field) else {
        return;
    };
    let vt = match &f.ty.value_type {
        None => None,
        Some(name) => match catalog.describe(who, ReadAt::default()).await {
            Ok(d) => d.data.value_types.into_iter().find(|v| &v.name == name),
            Err(_) => return,
        },
    };
    for k in ["from", "to"] {
        if let Some(v) = rule.get_mut(k)
            && let Ok(n) = lakelands_core::value::check(v, &f.ty, vt.as_ref())
        {
            *v = n;
        }
    }
}

/// `age`: run every aging rule once for a date (`LakeStore::age`), and
/// print what each rule did and the position the moves are visible from.
pub async fn age(out: Out<'_>, store: &Arc<dyn LakeStore>, today: &str) -> Result<(), Failure> {
    let bad = || {
        Failure::new(
            format!("`{today}` is not a date"),
            "give --today as a date, YYYY-MM-DD",
        )
    };
    let parts: Vec<&str> = today.split('-').collect();
    let date = match parts.as_slice() {
        [y, m, d]
            if [(y, 4), (m, 2), (d, 2)]
                .iter()
                .all(|(p, n)| p.len() == *n && p.bytes().all(|b| b.is_ascii_digit())) =>
        {
            let (y, m, d) = (
                y.parse().map_err(|_| bad())?,
                m.parse().map_err(|_| bad())?,
                d.parse().map_err(|_| bad())?,
            );
            lakelands_api::Timestamp::from_date(y, m, d).ok_or_else(bad)?
        }
        _ => return Err(bad()),
    };
    let report = store.age(date).await?;
    for r in &report.rules {
        match &r.skipped {
            Some(why) => put(out, format!("{}: skipped, {why}", r.rule))?,
            None => put(
                out,
                format!(
                    "{}: moved {}, {} guard(s) failed{}",
                    r.rule,
                    r.moved,
                    r.failed_guards,
                    refused_words(r.refused)
                ),
            )?,
        }
    }
    put(
        out,
        format!("aged {today} through position {}", report.position),
    )
}

/// How the columns of a CSV file become a source record.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CsvMap {
    /// The type every row is a record of.
    #[serde(rename = "type")]
    ty: TypeName,
    /// The column holding the record key.
    key: String,
    /// Column to field.
    fields: BTreeMap<String, FieldName>,
    /// The column holding when the source observed the row, if any.
    #[serde(default)]
    seen_at: Option<String>,
}

/// The records of an NDJSON file, one per non-blank line.
fn ndjson(file: &str, text: &str) -> Result<Vec<SourceRecord>, Failure> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(n, l)| {
            serde_json::from_str(l).map_err(|e| {
                Failure::new(
                    format!("line {} of {file} is not a source record: {e}", n + 1),
                    "each line must be one JSON object with `type`, `key` and `fields`; nothing was ingested",
                )
            })
        })
        .collect()
}

/// The rows of a CSV file (RFC 4180: quoted fields may hold commas,
/// quotes doubled, and line breaks), header first.
fn csv_rows(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let (mut row, mut cell) = (Vec::new(), String::new());
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => cell.push(c),
            (false, '"') if cell.is_empty() => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') if chars.peek() == Some(&'\n') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => cell.push(c),
        }
    }
    if quoted {
        return Err("a quoted field is never closed".into());
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    Ok(rows)
}

/// The records of a CSV file, through a column map. A cell becomes a
/// value of its field's storage kind; an empty cell is left out.
async fn csv(
    catalog: &dyn Catalog,
    who: &Principal,
    (file, text): (&str, &str),
    (map_file, map): (&str, &str),
) -> Result<Vec<SourceRecord>, Failure> {
    let map: CsvMap = serde_json::from_str(map).map_err(|e| {
        Failure::new(
            format!("{map_file} is not a column map: {e}"),
            "a map names `type`, the `key` column, and `fields` from column to field",
        )
    })?;
    let kinds: BTreeMap<FieldName, StorageKind> = catalog
        .describe_type(who, &map.ty, ReadAt::default())
        .await?
        .data
        .fields
        .into_iter()
        .map(|f| (f.name, f.ty.kind))
        .collect();
    let bad = |what: String| {
        Failure::new(
            what,
            "correct the file and run the command again; nothing was ingested",
        )
    };
    let mut rows = csv_rows(text).map_err(|e| bad(format!("{file} is not CSV: {e}")))?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let header = rows.remove(0);
    let column = |name: &str| {
        header.iter().position(|h| h == name).ok_or_else(|| {
            bad(format!(
                "{file} has no column `{name}`, which the map names"
            ))
        })
    };
    let key_col = column(&map.key)?;
    let seen_col = map.seen_at.as_deref().map(column).transpose()?;
    let mut cols = Vec::new();
    for (col, f) in &map.fields {
        // A field hidden from the principal is said as one the type lacks.
        let kind = *kinds.get(f).ok_or_else(|| {
            let ty = map.ty.clone();
            let field = f.clone();
            Failure::new(
                Case::UnknownField { ty, field }.to_string(),
                "correct the map, or ask for a grant on the field",
            )
        })?;
        cols.push((column(col)?, f.clone(), kind, col.as_str()));
    }
    let mut out = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let line = n + 2;
        let cell = |i: usize| row.get(i).map(String::as_str).unwrap_or("");
        let mut fields = BTreeMap::new();
        for (i, f, kind, col) in &cols {
            let raw = cell(*i);
            if raw.is_empty() {
                continue;
            }
            let v = match kind {
                StorageKind::Number => raw
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number),
                StorageKind::Boolean => match raw {
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    _ => None,
                },
                StorageKind::Json | StorageKind::Vector | StorageKind::Geo => {
                    serde_json::from_str(raw).ok()
                }
                _ => Some(Value::String(raw.to_owned())),
            };
            let v = v.ok_or_else(|| {
                bad(format!(
                    "line {line}, column `{col}`: `{raw}` is not a valid {}",
                    serde_json::to_value(kind)
                        .unwrap_or_default()
                        .as_str()
                        .unwrap_or("value")
                ))
            })?;
            fields.insert(f.clone(), v);
        }
        let seen_at = match seen_col {
            Some(i) if !cell(i).is_empty() => Some(
                lakelands_api::Timestamp::parse(cell(i))
                    .map_err(|e| bad(format!("line {line}: {e}")))?,
            ),
            _ => None,
        };
        out.push(SourceRecord {
            source: None,
            upstream: None,
            ty: map.ty.clone(),
            key: RecordKey::from(cell(key_col)),
            fields,
            seen_at,
        });
    }
    Ok(out)
}

struct Items(std::vec::IntoIter<SourceRecord>);

impl Stream for Items {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        Poll::Ready(self.0.next())
    }
}

/// `ingest`: feed the lake from a file, one line per commit window.
pub async fn ingest(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    file: &Path,
    source: &str,
    map: Option<&Path>,
) -> Result<(), Failure> {
    let text = read(file)?;
    let map = match map {
        Some(m) => Some((m.display().to_string(), read(m)?)),
        None => None,
    };
    let map = map
        .as_ref()
        .map(|(name, text)| (name.as_str(), text.as_str()));
    let file = (file.display().to_string(), text);
    ingest_text(out, catalog, who, (&file.0, &file.1), source, map).await?;
    Ok(())
}

/// Feed the lake as `ingest` does, from a file's name and text and, for
/// CSV, its map's; answer the position the last window committed through.
pub async fn ingest_text(
    out: Out<'_>,
    catalog: &dyn Catalog,
    who: &Principal,
    file: (&str, &str),
    source: &str,
    map: Option<(&str, &str)>,
) -> Result<Option<Position>, Failure> {
    let records = match map {
        Some(m) => csv(catalog, who, file, m).await?,
        None => ndjson(file.0, file.1)?,
    };
    let mut acks = catalog
        .ingest(
            who,
            &SourceId::from(source),
            Box::pin(Items(records.into_iter())),
        )
        .await?;
    let mut through = None;
    while let Some(ack) = std::future::poll_fn(|cx| acks.as_mut().poll_next(cx)).await {
        let c = ack?;
        through = Some(c.position);
        put(
            out,
            format!(
                "committed {} record(s), {} already in the lake, through position {}",
                c.records, c.duplicates, c.position
            ),
        )?;
    }
    Ok(through)
}

/// `feed`: the change feed from a position, one event per line in the
/// log format's canonical form, until the head.
pub async fn feed(
    out: Out<'_>,
    store: &Arc<dyn LakeStore>,
    from: Position,
    kinds: Vec<String>,
) -> Result<(), Failure> {
    let feed = store.feed();
    let filter = FeedFilter {
        kinds,
        types: Vec::new(),
        keys: Vec::new(),
    };
    let mut at = from;
    loop {
        let batch = feed.poll(at, &filter, 256).await?;
        for e in &batch.events {
            let bytes = canonical::of(e);
            put(out, String::from_utf8_lossy(&bytes))?;
        }
        if batch.next > batch.head || batch.next == at {
            return Ok(());
        }
        at = batch.next;
    }
}

/// `purge`: retire every live record one source brought in, into the one
/// type of `--type` where it is given, as one append
/// (`LakeStore::purge_source`), and print what went, per type, and what
/// outside it still refers to it. `report` is the purge's `--report`;
/// without one at all (`None`), this is `--preflight`, which prints the
/// same for a purge not made and the id to make it under.
pub async fn purge(
    out: Out<'_>,
    store: &Arc<dyn LakeStore>,
    (source, ty): (&str, Option<&str>),
    report: Option<Option<&str>>,
) -> Result<(), Failure> {
    let (id, ty) = (SourceId::from(source), ty.map(TypeName::from));
    let (report, did) = match report {
        Some(given) => (
            store.purge_source(&id, ty.as_ref(), given).await?,
            "retired",
        ),
        None => (
            store.purge_preflight(&id, ty.as_ref()).await?,
            "would retire",
        ),
    };
    let named = |word: &str| {
        ty.as_ref()
            .map(|t| format!(" {word} {t}"))
            .unwrap_or_default()
    };
    for t in &report.types {
        put(out, format!("{}: {did} {}", t.ty, t.retired))?;
    }
    for r in &report.references {
        let first: Vec<String> = r
            .first
            .iter()
            .map(|f| format!("{} {}", f.ty, f.key))
            .collect();
        let what = format!(
            "{}.{}: {} record(s) outside the purge refer",
            r.ty, r.relation, r.records
        );
        put(out, format!("{what} into it, first {}", first.join(", ")))?;
    }
    for t in &report.unsearched {
        put(
            out,
            format!("{t}: not searched, it keeps no per-field provenance"),
        )?;
    }
    if !report.report_id.is_empty() {
        let (head, id) = (report.position, &report.report_id);
        return put(
            out,
            format!(
                "{source}{}: preflight at position {head}; purge with --report {id}",
                named("in")
            ),
        );
    }
    if report.retired == 0 {
        return put(
            out,
            format!("nothing to retire for {source}{}", named("in")),
        );
    }
    let (from, head) = (named("from"), report.position);
    put(
        out,
        format!("purged {source}{from} through position {head}"),
    )
}

/// `forget`: forget one source (`LakeStore::forget_source`): append the
/// act, destroy its keys, rebuild and reclaim, and print what it erased
/// and retired. `report` is its `--report`, the only way to act; without
/// one (`None`), as with `--preflight`, it prints the same for an act not
/// made, the
/// values the log holds in plaintext that refuse it, the time it expects
/// to hold the writer's lock, and the id to make it under
/// (`docs/decisions/058-a-source-may-be-forgotten.md`).
pub async fn forget(
    out: Out<'_>,
    store: &Arc<dyn LakeStore>,
    source: &str,
    report: Option<Option<&str>>,
) -> Result<(), Failure> {
    let id = SourceId::from(source);
    let (report, did) = match report {
        Some(given) => (store.forget_source(&id, given).await?, "erased"),
        None => (store.forget_preflight(&id).await?, "would erase"),
    };
    for f in &report.fields {
        put(
            out,
            format!("{}.{}: {did} {} value(s)", f.ty, f.field, f.values),
        )?;
    }
    for t in &report.retired {
        put(
            out,
            format!(
                "{}: retires {} record(s) {source} made and alone brought",
                t.ty, t.retired
            ),
        )?;
    }
    for t in &report.co_owned {
        let first: Vec<String> = t.first.iter().map(|k| k.to_string()).collect();
        let what = format!(
            "{}: keeps {} record(s) another hand made or wrote too",
            t.ty, t.records
        );
        put(out, format!("{what}, first {}", first.join(", ")))?;
    }
    if report.outputs > 0 {
        put(out, format!("{did} {} recorded output(s)", report.outputs))?;
    }
    for f in &report.recomputed {
        put(out, format!("{}.{}: computed again", f.ty, f.field))?;
    }
    for v in &report.views {
        put(out, format!("{v}: computed again"))?;
    }
    if report.plaintext > 0 {
        put(
            out,
            format!(
                "{source}: the log holds {} value(s) of erasable fields in plaintext, which the act cannot reach; it is refused",
                report.plaintext
            ),
        )?;
    }
    if !report.report_id.is_empty() {
        let (head, id) = (report.position, &report.report_id);
        let seconds = report.expected_ms.div_ceil(1000);
        if let Some(why) = &report.reclaim_owed {
            put(out, format!("{source}: {}", owed(why)))?;
        }
        return put(
            out,
            format!(
                "{source}: preflight at position {head}; the act holds the writer about {seconds} s \
                 ({} events); forget with --report {id}",
                report.events
            ),
        );
    }
    let reclaimed = reclaimed(report.reclaimed.as_ref(), report.reclaim_owed.as_deref());
    put(
        out,
        format!(
            "forgot {source} through position {}{reclaimed}",
            report.position
        ),
    )
}

/// What an act says of its reclaim: the projection's bytes before and
/// after, where the engine measures them, or that the reclaim is owed
/// and why (lakelands#1179).
fn reclaimed(r: Option<&lakelands_api::write::CompactReport>, why: Option<&str>) -> String {
    match (r.map(|r| (r.before, r.after)), why) {
        (_, Some(why)) => format!("; {}", owed(why)),
        (Some((Some(b), Some(a))), None) => format!(", the projection {b} → {a} bytes"),
        _ => String::new(),
    }
}

/// The line that tells a reclaim still owed, with its cause: the act
/// answered, its values erased already (lakelands#1161).
pub fn owed(why: &str) -> String {
    format!("an act's reclaim is still owed, tried again at the next open or compact: {why}")
}

/// `values remove`: take back part of a source
/// (`LakeStore::remove_values`): append the act, destroy the key versions
/// whose values it all selects, rebuild and reclaim, and print the
/// report's own sentences (`RemoveReport::said`). As for [`forget`],
/// `report` is the `--report`, the only way to act; without one it
/// prints the preflight, the time it expects to hold the writer's lock,
/// and the id to make it under
/// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
pub async fn values_remove(
    out: Out<'_>,
    store: &Arc<dyn LakeStore>,
    (source, selection): (&str, &lakelands_api::write::Selection),
    (shredded, report): (bool, Option<Option<&str>>),
) -> Result<(), Failure> {
    let id = SourceId::from(source);
    let report = match report {
        Some(given) => store.remove_values(&id, selection, shredded, given).await?,
        None => store.remove_values_preflight(&id, selection).await?,
    };
    for line in report.said() {
        put(out, line)?;
    }
    if !report.report_id.is_empty() {
        let (head, id) = (report.position, &report.report_id);
        let seconds = report.expected_ms.div_ceil(1000);
        if let Some(why) = &report.reclaim_owed {
            put(out, format!("{source}: {}", owed(why)))?;
        }
        return put(
            out,
            format!(
                "{source}: preflight at position {head}; the act holds the writer about {seconds} s \
                 ({} events); remove with --report {id}",
                report.log_events
            ),
        );
    }
    let reclaimed = reclaimed(report.reclaimed.as_ref(), report.reclaim_owed.as_deref());
    put(
        out,
        format!(
            "took back from {source} through position {}{reclaimed}",
            report.position
        ),
    )
}

/// `rebuild`: rebuild the projection from the log. The lake was opened
/// for a rebuild, and that open folded the whole log again.
pub async fn rebuild(out: Out<'_>, store: &Arc<dyn LakeStore>) -> Result<(), Failure> {
    let p = store.head().await?;
    put(
        out,
        format!("rebuilt the projection from the log through position {p}"),
    )
}

/// `build-ahead`: build the projection this release lays out beside the
/// lake's, while an older release still serves it, saying first how much
/// room it takes; the writer's open that upgrades the lake then folds only
/// what was written since, and swaps it in. `discard` removes a build
/// ahead instead.
pub async fn build_ahead(
    out: Out<'_>,
    location: lakelands_api::Location,
    seams: lakelands_api::seam::Seams,
    discard: bool,
) -> Result<(), Failure> {
    let lake = location.lake.clone();
    if discard {
        let said = match lakelands_store::discard_ahead(location, seams).await? {
            true => format!("removed the projection built ahead beside {lake}"),
            false => format!("{lake} kept no projection built ahead: nothing was removed"),
        };
        return put(out, said);
    }
    put(out, room(&location, &seams).await?)?;
    let started = std::time::Instant::now();
    let built = lakelands_store::build_ahead(location, seams).await?;
    let secs = started.elapsed().as_secs_f64();
    let (events, through) = (built.events, built.through.map_or(0, |p| p.0));
    let from = match built.ahead {
        Some(at) => format!(", on from position {at}"),
        None => String::new(),
    };
    put(
        out,
        format!(
            "built {lake}'s projection ahead through position {through}: {events} events \
             folded in {secs:.1} s{from}; `lakelands upgrade` with this release folds what is \
             written after it and swaps it in"
        ),
    )
}

/// How much room a build ahead takes: about what the projection it is
/// built beside takes, as the engine measures it without opening it.
async fn room(
    location: &lakelands_api::Location,
    seams: &lakelands_api::seam::Seams,
) -> Result<String, Failure> {
    use lakelands_api::location::EngineConfig;
    let bytes = match lakelands_store::room_ahead(location.clone(), seams.clone()).await {
        Ok(bytes) => bytes,
        Err(lakelands_api::Error::Unbound { .. }) => {
            return Ok("building ahead keeps nothing on this engine".to_owned());
        }
        Err(e) => return Err(e.into()),
    };
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let place = match &location.engine {
        EngineConfig::Sqlite { dir, .. } => format!("in {}", dir.display()),
        EngineConfig::Postgres { schema, .. } => {
            let schema = schema
                .clone()
                .unwrap_or_else(|| format!("lakelands_{}", location.lake));
            format!("in the database, in a schema of its own beside `{schema}`")
        }
        _ => "beside the projection".to_owned(),
    };
    Ok(format!(
        "building ahead takes about {mib:.1} MiB {place}, as much as the projection beside it, \
         until the upgrade puts it in that one's place"
    ))
}

/// `upgrade`: say which log format the lake is written in, as a
/// read-only open finds it, and, where it is older than the one this
/// release writes, open it for writing once: that open appends
/// `lake_upgraded` and writes the new format from there; the events
/// before stay as they were written. A lake in this release's format
/// whose projection an older build laid out is opened for writing once
/// too, which rebuilds the projection. `check` stops before that open.
pub async fn upgrade(
    out: Out<'_>,
    location: lakelands_api::Location,
    seams: lakelands_api::seam::Seams,
    check: bool,
    open: &crate::Opener<'_>,
) -> Result<(), Failure> {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::error::Situation;
    use lakelands_api::event::{EventBody, LOG_FORMAT_VERSION as TO};
    let lake = location.lake.clone();
    // A lake already in this release's format opens to read; an older one
    // is refused by its format, and a projection an older build laid out
    // by its layout, which the refusal names.
    let from = match open(location.clone(), OpenMode::Read, seams.clone()).await {
        Ok(_) => {
            return put(
                out,
                format!("{lake} is written in log format {TO}: nothing to upgrade"),
            );
        }
        Err(e) => match e.situation() {
            Some(Situation::LogUpgradeOwed { found, .. }) => *found,
            Some(Situation::ProjectionLayout {
                found: Some(found),
                wanted,
                ..
            }) if found < wanted => {
                let found = *found;
                if check {
                    let said = format!(
                        "{lake} is written in log format {TO}, its projection in layout \
                         {found}: an open for writing rebuilds it; nothing was opened for writing"
                    );
                    return put(out, said);
                }
                let started = std::time::Instant::now();
                let store = open(location, OpenMode::Write, seams).await?;
                let secs = started.elapsed().as_secs_f64();
                // Another writer may have rebuilt it since the read-only open.
                let said = match store.rebuilt_at_open() {
                    Some(why)
                        if why.contains("a build ahead had folded")
                            && why.contains("folded the whole log again") =>
                    {
                        format!(
                            "rebuilt the projection of {lake} from layout {found} in {secs:.1} s, \
                         folding what was written after the build ahead, then the whole log \
                         again: the log held an act erasing values after the build ahead"
                        )
                    }
                    Some(why) if why.contains("removed the build ahead") => format!(
                        "rebuilt the projection of {lake} from layout {found} in {secs:.1} s, \
                         folding the whole log in place: another handle read the projection, \
                         so the build ahead could not be swapped in and was removed"
                    ),
                    Some(why) if why.contains("build ahead") => format!(
                        "rebuilt the projection of {lake} from layout {found} in {secs:.1} s, \
                         folding only what was written after the build ahead"
                    ),
                    Some(_) => {
                        format!(
                            "rebuilt the projection of {lake} from layout {found} in {secs:.1} s"
                        )
                    }
                    None => format!(
                        "{lake}'s projection: another writer rebuilt it from layout {found}"
                    ),
                };
                return put(out, said);
            }
            _ => return Err(e.into()),
        },
    };
    if check {
        return put(
            out,
            format!(
                "{lake} is written in log format {from}: an open for writing appends \
                 `lake_upgraded` from {from} to {TO}, and rebuilds the projection where an \
                 older build laid it out; nothing was opened for writing"
            ),
        );
    }
    let started = std::time::Instant::now();
    let store = open(location, OpenMode::Write, seams).await?;
    let head = store.head().await?;
    let secs = started.elapsed().as_secs_f64();
    // What this open appended: its own `lease_taken` is the last one in
    // the log, and the marker, where it upgraded, is the event after it.
    // Another writer may have upgraded the lake since the read-only open.
    let from_tail = Position(head.0.saturating_sub(64));
    let tail = store
        .feed()
        .poll(from_tail, &FeedFilter::default(), 128)
        .await?;
    let leased = (tail.events.iter()).rposition(|e| matches!(e.body, EventBody::LeaseTaken { .. }));
    let marked = leased
        .and_then(|i| tail.events.get(i + 1))
        .and_then(|e| match e.body {
            EventBody::LakeUpgraded { from, to } => Some((from, to)),
            _ => None,
        });
    match marked {
        Some((from, to)) => put(
            out,
            format!(
                "upgraded {lake} from log format {from} to {to} through position {head} in {secs:.1} s"
            ),
        ),
        None => put(
            out,
            format!("{lake} is written in log format {TO}: another writer upgraded it from {from}"),
        ),
    }
}

/// `compact`: give back the storage the projection no longer uses, and
/// print what the engine measured of it before and after. An engine that
/// measures nothing, the memory engine's, has nothing to reclaim.
pub async fn compact(out: Out<'_>, store: &Arc<dyn LakeStore>) -> Result<(), Failure> {
    let report = store.compact().await?;
    let lake = store.lake();
    match (report.before, report.after) {
        (Some(before), Some(after)) => put(
            out,
            format!(
                "compacted {lake}: projection {before} → {after} bytes at position {}",
                report.head
            ),
        ),
        _ => put(
            out,
            format!("nothing to reclaim in {lake}: its engine keeps no storage to give back"),
        ),
    }
}

/// `export`: write the lake's log as its segment files and a manifest
/// into a directory that is empty or absent, through the head the export
/// read when it began; a write landing during the export is not in it.
pub async fn export(out: Out<'_>, store: &Arc<dyn LakeStore>, to: &Path) -> Result<(), Failure> {
    let manifest = export::write(store.as_ref(), to).await?;
    for s in &manifest.segments {
        put(
            out,
            format!("{}: positions {}..{}", s.file, s.first, s.last),
        )?;
    }
    put(
        out,
        format!(
            "exported {} through position {} to {}",
            manifest.lake,
            manifest.through,
            to.display()
        ),
    )
}

/// `backup`: `to` brought up to the lake's head, and the lake's keyring
/// file copied into `keys` where it is named, apart from the backup.
pub async fn backup(
    out: Out<'_>,
    store: &Arc<dyn LakeStore>,
    root: &crate::root::Root,
    to: &Path,
    keys: Option<&Path>,
) -> Result<(), Failure> {
    // The directories are judged before anything is written.
    root.outside_the_lakes(to, "backup's")?;
    let keyring = match keys {
        Some(keys) => {
            root.keys_apart(keys, to)?;
            let Some(keyring) = root.file_keyring() else {
                let what = "--keys-to was given, and the root names no file keyring";
                let todo = "leave --keys-to out, and back the keys up as the key store's host does";
                return Err(Failure::new(what, todo));
            };
            Some((keyring, keys))
        }
        None => None,
    };
    let b = export::backup(store.as_ref(), &*store.feed(), to).await?;
    let since = b
        .since
        .map_or_else(|| "new".to_owned(), |p| format!("since {p}"));
    let line = format!(
        "backed up {} through position {} ({} new event(s), {since}) to {}",
        b.lake,
        b.through,
        b.copied,
        to.display()
    );
    put(out, line)?;
    if let Some((keyring, keys)) = keyring {
        let line = match keyring.back_up(&b.lake, keys)? {
            Some(file) => format!("copied the lake's keys to {}", file.display()),
            None => "the root's keyring holds no keys for the lake; none were copied".to_owned(),
        };
        put(out, line)?;
    }
    Ok(())
}

/// `restore --keys-from`: the lake's keyring file put back from `keys`,
/// only where the root's keyring has none: the file it holds knows every
/// destroy, and an older copy would bring back a forgotten source's keys.
/// Answers the keyring, for the file to be taken out again where the
/// import is refused.
pub fn restore_keys(
    out: Out<'_>,
    root: &crate::root::Root,
    lake: &lakelands_api::LakeId,
    keys: &Path,
) -> Result<lakelands_backings::keyring::FileKeyring, Failure> {
    let Some(keyring) = root.file_keyring() else {
        let what = "--keys-from was given, and the root names no file keyring";
        return Err(Failure::new(
            what,
            "name the keyring in the keys section of the configuration",
        ));
    };
    if !keyring.restore(lake, keys)? {
        let what = format!("the root's keyring already holds a file for lake `{lake}`");
        let todo = "leave --keys-from out: the keyring's own file knows every key destroyed since";
        return Err(Failure::new(what, todo));
    }
    let line = format!("put the keys of lake `{lake}` back from {}", keys.display());
    put(out, line).inspect_err(|_| drop(keyring.unrestore(lake)))?;
    Ok(keyring)
}

/// The lake an export holds, as its first event names it: the manifest is
/// never trusted. `None` when the first event cannot be read; the import's
/// own verification then says why.
pub fn exported_lake(dir: &Path) -> Option<String> {
    let first = dir.join(export::file_name(lakelands_api::Position::GENESIS));
    let file = std::fs::File::open(first).ok()?;
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut line).ok()?;
    let event: serde_json::Value = serde_json::from_str(&line).ok()?;
    event["lake"].as_str().map(str::to_owned)
}

/// `import`: the export in `dir` becomes the lake at `location`, which
/// must hold nothing. A lake kept in memory is refused before anything is
/// read: this process ends when the command answers, and the lake with it.
/// The outer error is the import refused; the inner one is its line not
/// written, after the lake was made.
pub async fn import(
    out: Out<'_>,
    location: lakelands_api::Location,
    seams: lakelands_api::seam::Seams,
    dir: &Path,
) -> Result<Result<(), Failure>, Failure> {
    if location.engine == lakelands_api::EngineConfig::Memory {
        return Err(Failure::new(
            format!(
                "the lake `{}` is kept in memory, which lives only as long as its process",
                location.lake
            ),
            "import into a lake kept on disk: SQLite (`lakelands lake new <id>`), or PostgreSQL",
        ));
    }
    let imported = lakelands_store::import(location, seams, dir.to_owned())
        .await
        .map_err(|e| {
            let todo = match &e {
                lakelands_api::Error::Refused { .. } => {
                    "an export opens only as the lake it came from, into a lake that holds \
                     nothing: write that lake's location (`lakelands lake new <id>`, or by hand \
                     for PostgreSQL), remove a lake there that holds events (its file or \
                     schema), and import again"
                }
                _ if e.seam().is_some()
                    || matches!(
                        e.situation(),
                        Some(lakelands_api::error::Situation::KeyMissing { .. })
                    ) =>
                {
                    "the lake's sealed values need its keys: name the keyring in the keys \
                     section of the configuration, and restore with --keys-from the keys file \
                     the backup copied"
                }
                _ => {
                    "a damaged export imports nothing: export the lake again; a failure of the \
                     target's storage leaves what it wrote, so remove that lake (its file or \
                     schema) and import again"
                }
            };
            Failure::new(e.to_string(), todo)
        })?;
    Ok(put(
        out,
        format!(
            "imported lake `{}`: {} event(s) from {} file(s), head {}, hash {}",
            imported.lake, imported.events, imported.files, imported.through, imported.hash.0
        ),
    ))
}
