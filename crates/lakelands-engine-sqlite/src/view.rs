//! Reads of the projection at one position.
//!
//! Every read runs in one read transaction on a reader connection, so it
//! sees one committed state of the projection, and reads the history rows
//! that hold at the view's position. When that position is the one the
//! projection reflects (the head), a read may find its candidates through
//! the head indexes; otherwise it reads the states at the position
//! directly. Either way `lakelands_core::engines` decides what matches,
//! where it sorts and what it scores, exactly as on every other engine.

use std::collections::BTreeSet;

use lakelands_api::engine::{Declared, ProjectionView, Scan};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_core::engines;
use rusqlite::{Connection, OptionalExtension, params};

use crate::handle::SqliteLake;
use crate::projection::{self, decode_record, holds_at};
use crate::{sql, unavailable};

/// A read view of one lake's projection at one position.
pub(crate) struct SqliteView<'a> {
    pub(crate) lake: &'a SqliteLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, and whether the view's
/// position is the head in the state it sees.
struct Read<'c> {
    conn: &'c Connection,
    at: Position,
    head: bool,
}

fn decode<T: serde::de::DeserializeOwned>(state: &str) -> Result<T> {
    serde_json::from_str(state)
        .map_err(|e| unavailable(format!("the projection holds a state it cannot read: {e}")))
}

impl SqliteView<'_> {
    /// Run `f` in one read transaction at the view's position; an empty
    /// projection answers `empty`.
    fn read<R>(&self, empty: R, f: impl FnOnce(&Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        let conn = self.lake.reader()?;
        conn.execute_batch("BEGIN").map_err(sql)?;
        let result = projection::folded(&conn).and_then(|folded| {
            f(&Read {
                conn: &conn,
                at,
                head: folded == Some(at),
            })
        });
        let _ = conn.execute_batch("COMMIT");
        result
    }
}

impl Read<'_> {
    fn at(&self) -> i64 {
        self.at.0 as i64
    }

    fn type_decl(&self, ty: &TypeName) -> Result<Option<TypeDecl>> {
        projection::type_at(self.conn, ty, Some(self.at))
    }

    fn record(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        let state: Option<String> = self
            .conn
            .prepare_cached(&format!(
                "SELECT state FROM records WHERE ty = ?1 AND key = ?2 AND {}",
                holds_at(3)
            ))
            .and_then(|mut s| {
                s.query_row(params![ty.as_str(), key.as_str(), self.at()], |r| r.get(0))
                    .optional()
            })
            .map_err(sql)?;
        state.as_deref().map(decode_record).transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT state FROM records WHERE ty = ?1 AND {} ORDER BY key",
                holds_at(2)
            ))
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![ty.as_str(), self.at()], |r| r.get::<_, String>(0))
            .map_err(sql)?;
        rows.map(|r| decode_record(&r.map_err(sql)?)).collect()
    }

    /// At most `n` records of a type at the view's position after the key
    /// `after` (from the first when `None`), in key order: a seek into the
    /// primary key `(ty, key, since)`, which sorts bytewise as keys do. The
    /// first page and a later one are two statements, so that neither
    /// hides the seek behind an `OR`.
    fn records_after(
        &self,
        ty: &TypeName,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let from = if after.is_some() { "AND key > ?4" } else { "" };
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT state FROM records WHERE ty = ?1 AND {} {from} ORDER BY key LIMIT ?3",
                holds_at(2)
            ))
            .map_err(sql)?;
        let state = |r: &rusqlite::Row<'_>| r.get::<_, String>(0);
        let rows: Vec<String> = match after {
            Some(after) => stmt
                .query_map(params![ty.as_str(), self.at(), n, after], state)
                .and_then(Iterator::collect),
            None => stmt
                .query_map(params![ty.as_str(), self.at(), n], state)
                .and_then(Iterator::collect),
        }
        .map_err(sql)?;
        rows.iter().map(|r| decode_record(r)).collect()
    }

    /// Records of a type by their keys, at the view's position.
    fn records_by_key(&self, ty: &TypeName, keys: BTreeSet<String>) -> Result<Vec<StoredRecord>> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(r) = self.record(ty, &RecordKey::from(key))? {
                out.push(r);
            }
        }
        Ok(out)
    }

    fn keys(&self, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<BTreeSet<String>> {
        let mut stmt = self.conn.prepare_cached(query).map_err(sql)?;
        let rows = stmt
            .query_map(args, |r| r.get::<_, String>(0))
            .map_err(sql)?;
        rows.map(|r| r.map_err(sql)).collect()
    }

    fn verb_on(&self, verb: &VerbName) -> Option<TypeName> {
        let state: String = self
            .conn
            .prepare_cached(&format!(
                "SELECT state FROM declarations WHERE ident = ?1 AND {}",
                holds_at(2)
            ))
            .and_then(|mut s| {
                s.query_row(params![format!("verb {verb}"), self.at()], |r| r.get(0))
                    .optional()
            })
            .ok()
            .flatten()?;
        match decode::<Declared>(&state).ok()?.declaration {
            Declaration::Verb(v) => Some(v.on),
            _ => None,
        }
    }

    fn edge_rows(&self, query: &str, a: &str, b: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare_cached(query).map_err(sql)?;
        let rows = stmt
            .query_map(params![a, b], |r| {
                Ok(Edge {
                    from: (r.get::<_, String>(0)?.into(), r.get::<_, String>(1)?.into()),
                    relation: r.get::<_, String>(2)?.into(),
                    to: (r.get::<_, String>(3)?.into(), r.get::<_, String>(4)?.into()),
                    fields: Default::default(),
                })
            })
            .map_err(sql)?;
        rows.map(|r| r.map_err(sql)).collect()
    }

    /// Every type declared at the view's position.
    fn types(&self) -> Result<Vec<TypeDecl>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT state FROM declarations WHERE ident LIKE 'type %' AND {} ORDER BY ident",
                holds_at(1)
            ))
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![self.at()], |r| r.get::<_, String>(0))
            .map_err(sql)?;
        let mut out = Vec::new();
        for r in rows {
            if let Declaration::Type(t) = decode::<Declared>(&r.map_err(sql)?)?.declaration {
                out.push(t);
            }
        }
        Ok(out)
    }
}

impl ProjectionView for SqliteView<'_> {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.read(Vec::new(), |r| {
            let mut stmt = r
                .conn
                .prepare_cached(&format!(
                    "SELECT state FROM declarations WHERE {} ORDER BY ident",
                    holds_at(1)
                ))
                .map_err(sql)?;
            let rows = stmt
                .query_map(params![r.at()], |row| row.get::<_, String>(0))
                .map_err(sql)?;
            rows.map(|s| decode(&s.map_err(sql)?)).collect()
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.read(None, |r| r.record(ty, key))
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            let decl = r.type_decl(ty)?;
            let narrowed = match (
                r.head,
                scan.filter.as_ref().and_then(engines::exact_text_term),
            ) {
                (true, Some((field, text))) => Some(r.keys(
                    "SELECT key FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3",
                    &[&ty.as_str(), &field.as_str(), &text],
                )?),
                _ => None,
            };
            let records = match narrowed {
                Some(keys) => r.records_by_key(ty, keys)?,
                // Ordered by the key alone, the page is read from `after`
                // in key order and stops when it is full.
                None if scan.order_by.is_empty() => {
                    return engines::scan_in_key_order(decl.as_ref(), scan, |after, n| {
                        r.records_after(ty, after, n)
                    });
                }
                None => r.records(ty)?,
            };
            Ok(engines::scan(decl.as_ref(), &records, scan))
        })
    }

    fn edges(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Edge>> {
        self.read(Vec::new(), |r| {
            let mut edges = Vec::new();
            let out = matches!(walk, Walk::Out | Walk::Both);
            let inn = matches!(walk, Walk::In | Walk::Both);
            if r.head {
                if out {
                    edges.extend(r.edge_rows(
                        "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM edges \
                         WHERE src_ty = ?1 AND src_key = ?2",
                        ty.as_str(),
                        key.as_str(),
                    )?);
                }
                if inn {
                    edges.extend(r.edge_rows(
                        "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM edges \
                         WHERE dst_ty = ?1 AND dst_key = ?2",
                        ty.as_str(),
                        key.as_str(),
                    )?);
                }
                edges.retain(|e| relation.is_none_or(|want| *want == e.relation));
            } else {
                if out && let (Some(decl), Some(record)) = (r.type_decl(ty)?, r.record(ty, key)?) {
                    edges.extend(engines::links_of(&decl, &record, relation));
                }
                if inn {
                    let types = r.types()?;
                    edges.extend(engines::in_links(ty, key, relation, &types, |name| {
                        r.records(name)
                    })?);
                }
            }
            // Relations carried by link records: the link records whose end
            // is this record. At the head the exact-text index finds them;
            // at an earlier position the link type's records there are
            // read. The rulebook decides which count.
            let types = r.types()?;
            edges.extend(lakelands_core::links::link_edges(
                ty,
                key,
                relation,
                walk,
                &types,
                |lt, field, k| {
                    if r.head {
                        let keys = r.keys(
                            "SELECT key FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3",
                            &[&lt.as_str(), &field, &k.as_str()],
                        )?;
                        r.records_by_key(lt, keys)
                    } else {
                        Ok(r.records(lt)?
                            .into_iter()
                            .filter(|l| {
                                l.fields.get(field).and_then(|v| v.as_str()) == Some(k.as_str())
                            })
                            .collect())
                    }
                },
            )?);
            engines::order_edges(&mut edges);
            Ok(edges)
        })
    }

    fn lexical(&self, ty: &TypeName, text: &str, k: u32) -> Result<Vec<(RecordKey, f64)>> {
        self.read(Vec::new(), |r| {
            // The full-text index describes the head; a view keeps the hits
            // live at its own position (`docs/seams.md`).
            let Some(decl) = projection::type_at(r.conn, ty, None)? else {
                return Ok(Vec::new());
            };
            let query = engines::tokens(text, decl.language.as_deref().unwrap_or(""));
            if query.is_empty() {
                return Ok(Vec::new());
            }
            // Every pipeline token is one term of the table (its hex), so
            // this finds exactly the records the shared scoring counts.
            let matching = query
                .iter()
                .map(|t| projection::fts_term(t))
                .collect::<Vec<_>>()
                .join(" OR ");
            let keys = r.keys(
                "SELECT s.key FROM full_text f JOIN search_rows s ON s.id = f.rowid \
                 WHERE full_text MATCH ?1 AND s.ty = ?2",
                &[&matching, &ty.as_str()],
            )?;
            let mut head = Vec::with_capacity(keys.len());
            let mut visible = BTreeSet::new();
            for key in keys {
                let key = RecordKey::from(key);
                if let Some(record) = projection::head_record(r.conn, ty, &key)? {
                    let then = if r.head {
                        Some(record.clone())
                    } else {
                        r.record(ty, &key)?
                    };
                    if engines::searchable(then.as_ref()) {
                        visible.insert(key);
                    }
                    head.push(record);
                }
            }
            Ok(engines::lexical(
                &decl,
                &head,
                |key| visible.contains(key),
                text,
                k,
            ))
        })
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.read(Vec::new(), |r| {
            let mut stmt = r
                .conn
                .prepare_cached(&format!(
                    "SELECT state FROM proposals WHERE {}",
                    holds_at(1)
                ))
                .map_err(sql)?;
            let rows = stmt
                .query_map(params![r.at()], |row| row.get::<_, String>(0))
                .map_err(sql)?;
            let all: Vec<Proposal> = rows
                .map(|s| decode(&s.map_err(sql)?))
                .collect::<Result<_>>()?;
            Ok(engines::proposals(&all, filter, |v| r.verb_on(v)))
        })
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.read(false, |r| {
            r.conn
                .prepare_cached(&format!(
                    "SELECT state FROM members WHERE object_set = ?1 AND ty = ?2 AND key = ?3 AND {}",
                    holds_at(4)
                ))
                .and_then(|mut s| {
                    s.query_row(params![set, ty.as_str(), key.as_str(), r.at()], |row| {
                        row.get::<_, bool>(0)
                    })
                    .optional()
                })
                .map(|m| m.unwrap_or(false))
                .map_err(sql)
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.read(None, |r| {
            r.conn
                .prepare_cached("SELECT at FROM applied WHERE id = ?1 AND at <= ?2")
                .and_then(|mut s| {
                    s.query_row(params![id.0, r.at()], |row| row.get::<_, i64>(0))
                        .optional()
                })
                .map(|p| p.map(|p| Position(p as u64)))
                .map_err(sql)
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.read(None, |r| {
            let state: Option<String> = r
                .conn
                .prepare_cached(&format!(
                    "SELECT state FROM outputs WHERE seam = ?1 AND input = ?2 AND {}",
                    holds_at(3)
                ))
                .and_then(|mut s| {
                    s.query_row(params![seam, input.0, r.at()], |row| row.get(0))
                        .optional()
                })
                .map_err(sql)?;
            state.as_deref().map(decode).transpose()
        })
    }
}
