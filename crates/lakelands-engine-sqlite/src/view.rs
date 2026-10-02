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
use lakelands_api::id::{FieldName, Position, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::read::{Direction, Order};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_core::engines;
use rusqlite::{Connection, ToSql, params};

use crate::handle::SqliteLake;
use crate::projection::{self, column, decode_record, holds_at, one};
use crate::{sql, unavailable};

/// A read view of one lake's projection at one position.
pub(crate) struct SqliteView<'a> {
    pub(crate) lake: &'a SqliteLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, the head in the state it
/// sees, and whether the view's position is that head.
struct Read<'c> {
    conn: &'c Connection,
    at: Position,
    folded: Option<Position>,
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
                folded,
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
        let state: Option<String> = one(
            self.conn,
            &format!(
                "SELECT state FROM records WHERE ty = ?1 AND key = ?2 AND {}",
                holds_at(3)
            ),
            params![ty.as_str(), key.as_str(), self.at()],
        )?;
        state.as_deref().map(decode_record).transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        column(
            self.conn,
            &format!(
                "SELECT state FROM records WHERE ty = ?1 AND {} ORDER BY key",
                holds_at(2)
            ),
            params![ty.as_str(), self.at()],
            |s: String| decode_record(&s),
        )
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
        let from = after.map_or("", |_| "AND key > ?4");
        self.page(
            &format!(
                "SELECT state FROM records WHERE ty = ?1 AND {} {from} ORDER BY key LIMIT ?3",
                holds_at(2)
            ),
            &[&ty.as_str(), &self.at(), &n],
            after,
        )
    }

    /// How many records of a type are in force at the view's position, the
    /// retired and the restricted ones only when asked, their flags read
    /// from the stored state.
    fn count_in_force(&self, ty: &TypeName, retired: bool, restricted: bool) -> Result<u64> {
        let counted: Vec<i64> = column(
            self.conn,
            &format!(
                "SELECT COUNT(*) FROM records WHERE ty = ?1 AND {} \
                 AND (?3 OR json_extract(state, '$.retired') = 0) \
                 AND (?4 OR json_extract(state, '$.restricted') = 0)",
                holds_at(2)
            ),
            params![ty.as_str(), self.at(), retired, restricted],
            Ok,
        )?;
        Ok(counted.first().map_or(0, |n| n.unsigned_abs()))
    }

    /// The fields of `ty` whose order index answers this read: none unless
    /// the index is kept through the head. At an earlier position it is
    /// read with the keys changed since ([`Read::changed`]).
    fn sorted_fields(&self, ty: &TypeName, decl: Option<&TypeDecl>) -> Result<Vec<FieldName>> {
        if self.folded.is_none() || projection::sorted(self.conn, ty)? != self.folded {
            return Ok(Vec::new());
        }
        projection::sorted_fields(self.conn, ty, decl)
    }

    /// The keys of `ty` with a state put or ended after the view's
    /// position, and their states there; `None` past `ceiling` states put
    /// or keys ended. Two range reads, one into `records_since` for the
    /// states put, one into `records_at` for the states that held at the
    /// position and ended: a `UNION` of the two is planned as a merge in
    /// key order, which reads the type.
    fn changed(&self, ty: &TypeName, ceiling: usize) -> Result<Option<engines::Changed>> {
        let args = params![ty.as_str(), self.at(), ceiling as i64 + 1];
        let put: Vec<String> = column(
            self.conn,
            "SELECT key FROM records WHERE ty = ?1 AND since > ?2 ORDER BY since LIMIT ?3",
            args,
            Ok,
        )?;
        let ended: Vec<String> = column(
            self.conn,
            "SELECT key FROM records WHERE ty = ?1 AND until > ?2 AND since <= ?2 ORDER BY until LIMIT ?3",
            args,
            Ok,
        )?;
        let keys: BTreeSet<String> = put.iter().chain(&ended).cloned().collect();
        if put.len().max(ended.len()).max(keys.len()) > ceiling {
            return Ok(None);
        }
        Ok(Some(engines::Changed {
            states: self.records_by_key(ty, keys.clone())?,
            keys: keys.into_iter().map(RecordKey::from).collect(),
        }))
    }

    /// At most `n` head records of a type with a row in the order index for
    /// `order`'s field, with their sort keys, in the index's order for its
    /// direction, strictly after `from`: a seek into `sort_index_asc` or
    /// `sort_index_desc`.
    fn sorted_after(
        &self,
        ty: &TypeName,
        order: &Order,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        let (seek, by) = match order.dir {
            Direction::Asc => ("AND (s.sort_key, s.key) > (?4, ?5)", "s.sort_key, s.key"),
            Direction::Desc => (
                "AND (s.sort_key < ?4 OR (s.sort_key = ?4 AND s.key > ?5))",
                "s.sort_key DESC, s.key",
            ),
        };
        let seek = if from.is_some() { seek } else { "" };
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT s.sort_key, r.state FROM \
                 (SELECT s.sort_key, s.key FROM sort_index s \
                  WHERE s.ty = ?1 AND s.field = ?2 {seek} ORDER BY {by} LIMIT ?3) s \
                 JOIN records r ON r.ty = ?1 AND r.key = s.key AND r.until IS NULL \
                 ORDER BY {by}"
            ))
            .map_err(sql)?;
        let row = |r: &rusqlite::Row<'_>| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?));
        let rows: Vec<(Vec<u8>, String)> = match from {
            Some((k, key)) => stmt
                .query_map(params![ty.as_str(), order.field.as_str(), n, k, key], row)
                .and_then(Iterator::collect),
            None => stmt
                .query_map(params![ty.as_str(), order.field.as_str(), n], row)
                .and_then(Iterator::collect),
        }
        .map_err(sql)?;
        rows.into_iter()
            .map(|(k, state)| Ok((k, decode_record(&state)?)))
            .collect()
    }

    /// At most `n` head records of a type without a row in the order index
    /// for `field`, in key order after `after`.
    fn unsorted_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let from = after.map_or("", |_| "AND r.key > ?4");
        self.page(
            &format!(
                "SELECT r.state FROM records r WHERE r.ty = ?1 AND r.until IS NULL {from} \
                 AND NOT EXISTS (SELECT 1 FROM sort_index s \
                 WHERE s.ty = r.ty AND s.field = ?2 AND s.key = r.key) \
                 ORDER BY r.key LIMIT ?3"
            ),
            &[&ty.as_str(), &field.as_str(), &n],
            after,
        )
    }

    /// The records a page's statement answers, `after` bound as its fourth
    /// parameter when there is one; decoded once all are read.
    fn page(
        &self,
        statement: &str,
        args: &[&dyn ToSql],
        after: Option<&str>,
    ) -> Result<Vec<StoredRecord>> {
        let mut args = args.to_vec();
        args.extend(after.as_ref().map(|a| a as &dyn ToSql));
        let rows: Vec<String> = column(self.conn, statement, args.as_slice(), Ok)?;
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

    fn keys(&self, query: &str, args: &[&dyn ToSql]) -> Result<BTreeSet<String>> {
        column(self.conn, query, args, Ok)
    }

    fn verb_on(&self, verb: &VerbName) -> Option<TypeName> {
        let state: String = one(
            self.conn,
            &format!(
                "SELECT state FROM declarations WHERE ident = ?1 AND {}",
                holds_at(2)
            ),
            params![format!("verb {verb}"), self.at()],
        )
        .ok()??;
        match decode::<Declared>(&state).ok()?.declaration {
            Declaration::Verb(v) => Some(v.on),
            _ => None,
        }
    }

    /// The head's edges whose `end` (`src` or `dst`) is the record.
    fn edge_rows(&self, end: &str, ty: &TypeName, key: &RecordKey) -> Result<Vec<Edge>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM edges \
                 WHERE {end}_ty = ?1 AND {end}_key = ?2"
            ))
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![ty.as_str(), key.as_str()], |r| {
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
        let types: Vec<Option<TypeDecl>> = column(
            self.conn,
            &format!(
                "SELECT state FROM declarations WHERE ident LIKE 'type %' AND {} ORDER BY ident",
                holds_at(1)
            ),
            params![self.at()],
            |s: String| match decode::<Declared>(&s)?.declaration {
                Declaration::Type(t) => Ok(Some(t)),
                _ => Ok(None),
            },
        )?;
        Ok(types.into_iter().flatten().collect())
    }
}

impl ProjectionView for SqliteView<'_> {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.read(Vec::new(), |r| {
            column(
                r.conn,
                &format!(
                    "SELECT state FROM declarations WHERE {} ORDER BY ident",
                    holds_at(1)
                ),
                params![r.at()],
                |s: String| decode(&s),
            )
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.read(None, |r| r.record(ty, key))
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            r.records_by_key(ty, keys.iter().map(|k| k.to_string()).collect())
        })
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        self.read(0, |r| {
            // At the head, a count that selects by the flags alone is the
            // projection's `COUNT(*)`; any other is counted batch by batch.
            if r.head && scan.filter.is_none() && scan.equal.is_empty() && scan.valid_at.is_none() {
                return r.count_in_force(ty, scan.retired, scan.restricted);
            }
            let decl = r.type_decl(ty)?;
            engines::count_in_key_order(decl.as_ref(), scan, |after, n| {
                r.records_after(ty, after, n)
            })
        })
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            let decl = r.type_decl(ty)?;
            let narrowed = match (r.head, engines::text_lookup(scan)) {
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
                // Ordered by a field the lake indexes, at the head, the page
                // is read in the order index's order and stops when it is
                // full.
                None => {
                    let sorted = r.sorted_fields(ty, decl.as_ref())?;
                    match engines::by_order_index(decl.as_ref(), &sorted, scan) {
                        Some(first) if r.head => {
                            return engines::scan_in_field_order(
                                decl.as_ref(),
                                scan,
                                |from, n| r.sorted_after(ty, first, from, n),
                                |after, n| r.unsorted_after(ty, &first.field, after, n),
                            );
                        }
                        // At an earlier position, the head's index with the
                        // keys changed since; past the ceiling, the sort.
                        Some(first) => match r.changed(ty, engines::changed_ceiling(scan.limit))? {
                            Some(changed) => {
                                return engines::scan_in_field_order_at(
                                    decl.as_ref(),
                                    scan,
                                    &changed,
                                    |from, n| r.sorted_after(ty, first, from, n),
                                    |after, n| r.unsorted_after(ty, &first.field, after, n),
                                );
                            }
                            None => r.records(ty)?,
                        },
                        None => r.records(ty)?,
                    }
                }
            };
            Ok(engines::scan(decl.as_ref(), &records, scan))
        })
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        // One read transaction for all the keys; its statements are
        // in-process, so each key keeps its own.
        self.read(vec![Vec::new(); keys.len()], |r| {
            let out = matches!(walk, Walk::Out | Walk::Both);
            let inn = matches!(walk, Walk::In | Walk::Both);
            let types = r.types()?;
            let mut all = Vec::with_capacity(keys.len());
            for key in keys {
                let mut edges = Vec::new();
                if r.head {
                    if out {
                        edges.extend(r.edge_rows("src", ty, key)?);
                    }
                    if inn {
                        edges.extend(r.edge_rows("dst", ty, key)?);
                    }
                    edges.retain(|e| relation.is_none_or(|want| *want == e.relation));
                } else {
                    if out
                        && let (Some(decl), Some(record)) = (r.type_decl(ty)?, r.record(ty, key)?)
                    {
                        edges.extend(engines::links_of(&decl, &record, relation));
                    }
                    if inn {
                        edges.extend(engines::in_links(ty, key, relation, &types, |name| {
                            r.records(name)
                        })?);
                    }
                }
                all.push(edges);
            }
            // Relations carried by link records: the link records whose end
            // is one of the records. At the head the exact-text index finds
            // them; at an earlier position the link type's records there are
            // read. The rulebook decides which count.
            let linked = lakelands_core::links::link_edges(
                ty,
                keys,
                relation,
                walk,
                &types,
                |lt, field, ks| {
                    if r.head {
                        let mut found = BTreeSet::new();
                        for k in ks {
                            found.extend(r.keys(
                                "SELECT key FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3",
                                &[&lt.as_str(), &field, &k.as_str()],
                            )?);
                        }
                        r.records_by_key(lt, found)
                    } else {
                        // The link type at the position, whole: the rulebook
                        // keeps the links whose end a record is.
                        r.records(lt)
                    }
                },
            )?;
            for (edges, linked) in all.iter_mut().zip(linked) {
                edges.extend(linked);
                engines::order_edges(edges);
            }
            Ok(all)
        })
    }

    fn lexical(
        &self,
        ty: &TypeName,
        text: &str,
        fields: Option<&[FieldName]>,
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
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
            // this finds exactly the records the shared scoring counts. A
            // row holds every searchable field's tokens: limited to named
            // fields, these are a superset, and the shared scoring keeps
            // the records whose named fields match before it cuts at `k`.
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
                fields,
                k,
            ))
        })
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.read(Vec::new(), |r| {
            let all: Vec<Proposal> = column(
                r.conn,
                &format!("SELECT state FROM proposals WHERE {}", holds_at(1)),
                params![r.at()],
                |s: String| decode(&s),
            )?;
            Ok(engines::proposals(&all, filter, |v| r.verb_on(v)))
        })
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.read(false, |r| {
            one(
                r.conn,
                &format!(
                    "SELECT state FROM members WHERE object_set = ?1 AND ty = ?2 AND key = ?3 AND {}",
                    holds_at(4)
                ),
                params![set, ty.as_str(), key.as_str(), r.at()],
            )
            .map(|m| m.unwrap_or(false))
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.read(None, |r| {
            one(
                r.conn,
                "SELECT at FROM applied WHERE id = ?1 AND at <= ?2",
                params![id.0, r.at()],
            )
            .map(|p: Option<i64>| p.map(|p| Position(p as u64)))
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.read(None, |r| {
            let state: Option<String> = one(
                r.conn,
                &format!(
                    "SELECT state FROM outputs WHERE seam = ?1 AND input = ?2 AND {}",
                    holds_at(3)
                ),
                params![seam, input.0, r.at()],
            )?;
            state.as_deref().map(decode).transpose()
        })
    }
}
