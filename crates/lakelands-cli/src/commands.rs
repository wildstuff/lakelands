//! The commands that open a lake: `apply`, `ingest`, `feed`,
//! `rebuild` and `serve`. Each calls the library in-process and prints
//! the position of every answer that has one.

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
use lakelands_core::{bundle, canonical};
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

/// `apply`: load an ontology bundle and declare what it holds, in the
/// order the loader gives. A bundle with any problem is refused whole,
/// every problem named against its file, and nothing is declared.
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
    let total = bundle.declarations.len();
    for (i, d) in bundle.declarations.into_iter().enumerate() {
        let what = label(&d);
        match catalog.declare(who, d).await {
            Ok(outcome) => put(
                out,
                format!("declared {what} at position {}", outcome.position()),
            )?,
            Err(e) => {
                let mut f =
                    Failure::from(e).about(format!("declaration {} of {total} ({what})", i + 1));
                if i > 0 {
                    f.todo = format!(
                        "{}; the {i} declaration(s) before it stay applied",
                        f.todo.trim_end_matches("; nothing was written")
                    );
                }
                return Err(f);
            }
        }
    }
    Ok(())
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

/// `rebuild`: rebuild the projection from the log.
pub async fn rebuild(out: Out<'_>, store: &Arc<dyn LakeStore>) -> Result<(), Failure> {
    let p = store.rebuild().await?;
    put(
        out,
        format!("rebuilt the projection from the log through position {p}"),
    )
}
