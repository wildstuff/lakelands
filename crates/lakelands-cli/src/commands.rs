//! The commands that open a lake: `apply`, `ingest`, `feed`, `age`,
//! `purge`, `rebuild`, `compact`, `export` and `serve`. Each calls the library
//! in-process and prints the position of every answer that has one.
//! `import` makes a lake instead of opening one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use lakelands_api::id::{FieldName, Position, RecordKey, SourceId, TypeName};
use lakelands_api::ontology::StorageKind;
use lakelands_api::read::ReadAt;
use lakelands_api::record::{SourceRecord, Value};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{Catalog, LakeStore, Principal};
use lakelands_core::{bundle, canonical, export};
use serde::Deserialize;

use crate::say::Failure;

type Out<'a> = &'a mut dyn Write;

fn read(file: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(file).map_err(|e| {
        Failure::new(
            format!("could not read {}: {e}", file.display()),
            "check the path and that you may read the file",
        )
    })
}

fn put(out: Out<'_>, line: impl std::fmt::Display) -> Result<(), Failure> {
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
    dir: &Path,
) -> Result<(), Failure> {
    if !dir.is_dir() {
        return Err(Failure::new(
            format!("{} is not a bundle directory", dir.display()),
            "name the directory that holds the bundle's types/, verbs/ and other lanes",
        ));
    }
    let bundle = bundle::load_dir(dir).map_err(|problems| {
        // One `error:` line per problem; Failure prints the first prefix.
        let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
        Failure::new(
            lines.join("\nerror: "),
            "correct the bundle and apply it again; nothing was applied",
        )
    })?;
    // The bundle is applied as one set: validated together, appended in
    // one append at one position, or refused whole. A bundle of aging
    // rules alone declares nothing.
    let what: Vec<String> = bundle.declarations.iter().map(label).collect();
    if what.is_empty() && !bundle.aging_rules.is_empty() {
        return aging_rules(out, catalog, who, bundle.aging_rules).await;
    }
    match catalog.apply(who, bundle.declarations).await {
        Ok(outcome) => {
            put(
                out,
                format!(
                    "applied {} declaration(s), in force from position {}:",
                    what.len(),
                    outcome.position()
                ),
            )?;
            for w in what {
                put(out, format!("  {w}"))?;
            }
        }
        Err(e) => {
            let mut f = Failure::from(e);
            f.todo = "correct the bundle and apply it again; nothing was applied".to_owned();
            return Err(f);
        }
    }
    aging_rules(out, catalog, who, bundle.aging_rules).await
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
                    "{}: moved {}, {} guard(s) failed",
                    r.rule, r.moved, r.failed_guards
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
fn ndjson(file: &Path, text: &str) -> Result<Vec<SourceRecord>, Failure> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(n, l)| {
            serde_json::from_str(l).map_err(|e| {
                Failure::new(
                    format!("line {} of {} is not a source record: {e}", n + 1, file.display()),
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
    file: &Path,
    text: &str,
    map_file: &Path,
) -> Result<Vec<SourceRecord>, Failure> {
    let map: CsvMap = serde_json::from_str(&read(map_file)?).map_err(|e| {
        Failure::new(
            format!("{} is not a column map: {e}", map_file.display()),
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
    let mut rows =
        csv_rows(text).map_err(|e| bad(format!("{} is not CSV: {e}", file.display())))?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let header = rows.remove(0);
    let column = |name: &str| {
        header.iter().position(|h| h == name).ok_or_else(|| {
            bad(format!(
                "{} has no column `{name}`, which the map names",
                file.display()
            ))
        })
    };
    let key_col = column(&map.key)?;
    let seen_col = map.seen_at.as_deref().map(column).transpose()?;
    let mut cols = Vec::new();
    for (col, f) in &map.fields {
        let kind = *kinds.get(f).ok_or_else(|| {
            Failure::new(
                format!("`{}` has no field `{f}` you may see", map.ty),
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
    let records = match map {
        Some(m) => csv(catalog, who, file, &text, m).await?,
        None => ndjson(file, &text)?,
    };
    let mut acks = catalog
        .ingest(
            who,
            &SourceId::from(source),
            Box::pin(Items(records.into_iter())),
        )
        .await?;
    while let Some(ack) = std::future::poll_fn(|cx| acks.as_mut().poll_next(cx)).await {
        let c = ack?;
        put(
            out,
            format!(
                "committed {} record(s), {} already in the lake, through position {}",
                c.records, c.duplicates, c.position
            ),
        )?;
    }
    Ok(())
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

/// `purge`: retire every live record one source brought in, as one append
/// (`LakeStore::purge_source`), and print what went, per type.
pub async fn purge(out: Out<'_>, store: &Arc<dyn LakeStore>, source: &str) -> Result<(), Failure> {
    let report = store.purge_source(&SourceId::from(source)).await?;
    for t in &report.types {
        put(out, format!("{}: retired {}", t.ty, t.retired))?;
    }
    for t in &report.unsearched {
        put(
            out,
            format!("{t}: not searched, it keeps no per-field provenance"),
        )?;
    }
    if report.retired == 0 {
        return put(out, format!("nothing to retire for {source}"));
    }
    put(
        out,
        format!("purged {source} through position {}", report.position),
    )
}

/// `rebuild`: rebuild the projection from the log.
pub async fn rebuild(out: Out<'_>, store: &Arc<dyn LakeStore>) -> Result<(), Failure> {
    let p = store.rebuild().await?;
    put(
        out,
        format!("rebuilt the projection from the log through position {p}"),
    )
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
    let manifest = export::write(store.as_ref(), to)
        .await
        .map_err(|e| match e {
            lakelands_api::Error::Refused {
                reason: lakelands_api::Refusal::Precondition { .. },
                ..
            } => Failure::new(e.to_string(), "export into an empty or new directory"),
            e => e.into(),
        })?;
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
pub async fn import(
    out: Out<'_>,
    location: lakelands_api::Location,
    seams: lakelands_api::seam::Seams,
    dir: &Path,
) -> Result<(), Failure> {
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
                _ => {
                    "a damaged export imports nothing: export the lake again; a failure of the \
                     target's storage leaves what it wrote, so remove that lake (its file or \
                     schema) and import again"
                }
            };
            Failure::new(e.to_string(), todo)
        })?;
    put(
        out,
        format!(
            "imported lake `{}`: {} event(s) from {} file(s), head {}, hash {}",
            imported.lake, imported.events, imported.files, imported.through, imported.hash.0
        ),
    )
}
