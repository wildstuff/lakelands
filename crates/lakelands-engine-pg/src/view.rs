//! Reads of the projection at one position.
//!
//! A read of more than one statement, or one that asks whether the view
//! is at the head, runs in one `REPEATABLE READ READ ONLY` transaction on
//! the handle's reader connection, so all of it sees one committed state
//! of the projection; a read of one statement runs alone, since one
//! statement already sees one. A read reads the history rows that hold at
//! the view's position. When that position is the one the projection
//! reflects (the head), a read may find its candidates through the head
//! indexes; otherwise it reads the states at the position directly. Either way
//! `lakelands_core::engines` decides what matches, where it sorts and what
//! it scores, exactly as on every other engine.

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

use crate::connect::Db;
use crate::handle::PgLake;
use crate::projection::{self, Names, PROPOSALS, decode, decode_record, holds_at};

/// A read view of one lake's projection at one position.
pub(crate) struct PgView<'a> {
    pub(crate) lake: &'a PgLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, and whether the view's
/// position is the head in the state it sees.
struct Read<'c> {
    db: &'c mut Db,
    names: &'c Names,
    at: Position,
    head: bool,
    /// The view is at the head and the order index describes it.
    sorted: bool,
}

impl PgView<'_> {
    /// Run `f` in one read transaction at the view's position; an empty
    /// projection answers `empty`.
    fn read<R>(&self, empty: R, f: impl FnOnce(&mut Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        let mut db = self.lake.reader();
        let names = self.lake.names();
        // The transaction opens and reads the folded and sorted markers in
        // one round trip, so the flags are of the state every query in it
        // sees.
        let markers = db.simple(&format!(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; \
             SELECT concat((SELECT n FROM {m} WHERE k = 'folded'), ' ', \
             (SELECT n FROM {m} WHERE k = 'sorted'))",
            m = names.t("meta")
        ));
        let answer = markers.and_then(|markers| {
            let markers = markers.into_iter().flatten().next().unwrap_or_default();
            let (folded, sorted) = markers.split_once(' ').unwrap_or((&markers, ""));
            let head = folded == at.0.to_string();
            f(&mut Read {
                db: &mut db,
                names,
                at,
                head,
                sorted: head && sorted == folded,
            })
        });
        match answer {
            Ok(r) => {
                db.batch("COMMIT")?;
                Ok(r)
            }
            Err(e) => {
                let _ = db.batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

impl PgView<'_> {
    /// Run `f`, which runs exactly one statement and never asks whether
    /// the view is at the head, outside a transaction: one statement reads
    /// one committed state, so it needs none, and costs one round trip.
    fn single<R>(&self, empty: R, f: impl FnOnce(&mut Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        f(&mut Read {
            db: &mut self.lake.reader(),
            names: self.lake.names(),
            at,
            head: false,
            sorted: false,
        })
    }
}

impl Read<'_> {
    fn at(&self) -> i64 {
        self.at.0 as i64
    }

    fn type_decl(&mut self, ty: &TypeName) -> Result<Option<TypeDecl>> {
        projection::type_at(self.db, self.names, ty, Some(self.at))
    }

    fn record(&mut self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        let at = self.at();
        self.db
            .query_opt(
                &format!(
                    "SELECT state FROM {} WHERE ty = $1 AND key = $2 AND {}",
                    self.names.t("records"),
                    holds_at(3)
                ),
                &[&ty.as_str(), &key.as_str(), &at],
            )?
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&mut self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        let at = self.at();
        self.db
            .query(
                &format!(
                    "SELECT state FROM {} WHERE ty = $1 AND {} ORDER BY key COLLATE \"C\"",
                    self.names.t("records"),
                    holds_at(2)
                ),
                &[&ty.as_str(), &at],
            )?
            .iter()
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .collect()
    }

    /// At most `n` records of a type at the view's position after the key
    /// `after` (from the first when `None`), in key order, bytewise: a walk
    /// of the index `records_key_c`. The first page and a later one are two
    /// statements, so that neither hides the index condition behind an
    /// `OR`.
    fn records_after(
        &mut self,
        ty: &TypeName,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let at = self.at();
        let limit = i64::from(n);
        let from = if after.is_some() {
            "AND key COLLATE \"C\" > $4"
        } else {
            ""
        };
        let query = format!(
            "SELECT state FROM {} WHERE ty = $1 AND {} {from} ORDER BY key COLLATE \"C\" LIMIT $3",
            self.names.t("records"),
            holds_at(2)
        );
        let rows = match after {
            Some(after) => self
                .db
                .query(&query, &[&ty.as_str(), &at, &limit, &after])?,
            None => self.db.query(&query, &[&ty.as_str(), &at, &limit])?,
        };
        rows.iter()
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .collect()
    }

    /// At most `n` head records of a type with a row in the order index for
    /// `order`'s field, with their sort keys, in the index's order for its
    /// direction, strictly after `from`: a seek into `sort_index_asc` or
    /// `sort_index_desc`.
    fn sorted_after(
        &mut self,
        ty: &TypeName,
        order: &Order,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        let limit = i64::from(n);
        let (seek, by) = match order.dir {
            Direction::Asc => (
                "AND (s.sort_key, s.key COLLATE \"C\") > ($4, $5 COLLATE \"C\")",
                "s.sort_key, s.key COLLATE \"C\"",
            ),
            Direction::Desc => (
                "AND (s.sort_key < $4 OR (s.sort_key = $4 AND s.key COLLATE \"C\" > $5))",
                "s.sort_key DESC, s.key COLLATE \"C\"",
            ),
        };
        let seek = if from.is_some() { seek } else { "" };
        // The index's rows first, in its order and cut at the limit, then
        // their records: every row is a head record's, so the join keeps
        // them all, and the page follows the index whatever the planner
        // knows of the tables.
        let query = format!(
            "SELECT s.sort_key, r.state FROM \
             (SELECT s.sort_key, s.key FROM {sorted} s \
              WHERE s.ty = $1 AND s.field = $2 {seek} ORDER BY {by} LIMIT $3) s \
             JOIN {records} r ON r.ty = $1 AND r.key = s.key AND r.until IS NULL \
             ORDER BY {by}",
            sorted = self.names.t("sort_index"),
            records = self.names.t("records"),
        );
        let rows = match from {
            Some((k, key)) => self.db.query(
                &query,
                &[&ty.as_str(), &order.field.as_str(), &limit, &k, &key],
            )?,
            None => self
                .db
                .query(&query, &[&ty.as_str(), &order.field.as_str(), &limit])?,
        };
        rows.iter()
            .map(|r| Ok((r.get(0), decode_record(&r.get::<_, String>(1))?)))
            .collect()
    }

    /// At most `n` head records of a type without a row in the order index
    /// for `field`, in key order after `after`.
    fn unsorted_after(
        &mut self,
        ty: &TypeName,
        field: &FieldName,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let limit = i64::from(n);
        let from = if after.is_some() {
            "AND r.key COLLATE \"C\" > $4"
        } else {
            ""
        };
        let query = format!(
            "SELECT r.state FROM {records} r WHERE r.ty = $1 AND r.until IS NULL {from} \
             AND NOT EXISTS (SELECT 1 FROM {sorted} s \
             WHERE s.ty = r.ty AND s.field = $2 AND s.key = r.key) \
             ORDER BY r.key COLLATE \"C\" LIMIT $3",
            sorted = self.names.t("sort_index"),
            records = self.names.t("records"),
        );
        let rows = match after {
            Some(after) => self
                .db
                .query(&query, &[&ty.as_str(), &field.as_str(), &limit, &after])?,
            None => self
                .db
                .query(&query, &[&ty.as_str(), &field.as_str(), &limit])?,
        };
        rows.iter()
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .collect()
    }

    /// Records of a type by their keys, at the view's position.
    fn records_by_key(
        &mut self,
        ty: &TypeName,
        keys: BTreeSet<String>,
    ) -> Result<Vec<StoredRecord>> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(r) = self.record(ty, &RecordKey::from(key))? {
                out.push(r);
            }
        }
        Ok(out)
    }

    fn keys(
        &mut self,
        query: &str,
        args: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> Result<BTreeSet<String>> {
        Ok(self
            .db
            .query(query, args)?
            .iter()
            .map(|r| r.get::<_, String>(0))
            .collect())
    }

    /// The type each verb declared at the view's position is on.
    fn verbs(&mut self) -> Result<std::collections::BTreeMap<VerbName, TypeName>> {
        let at = self.at();
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {} WHERE ident LIKE 'verb %' AND {}",
                self.names.t("declarations"),
                holds_at(1)
            ),
            &[&at],
        )?;
        let mut out = std::collections::BTreeMap::new();
        for r in rows {
            if let Declaration::Verb(v) =
                decode::<Declared>(&r.get::<_, String>(0), "a declaration")?.declaration
            {
                out.insert(v.name, v.on);
            }
        }
        Ok(out)
    }

    fn edge_rows(&mut self, query: &str, a: &str, b: &str) -> Result<Vec<Edge>> {
        Ok(self
            .db
            .query(query, &[&a, &b])?
            .iter()
            .map(|r| Edge {
                from: (r.get::<_, String>(0).into(), r.get::<_, String>(1).into()),
                relation: r.get::<_, String>(2).into(),
                to: (r.get::<_, String>(3).into(), r.get::<_, String>(4).into()),
                fields: Default::default(),
            })
            .collect())
    }

    /// Every type declared at the view's position.
    fn types(&mut self) -> Result<Vec<TypeDecl>> {
        let at = self.at();
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {} WHERE ident LIKE 'type %' AND {} \
                 ORDER BY ident COLLATE \"C\"",
                self.names.t("declarations"),
                holds_at(1)
            ),
            &[&at],
        )?;
        let mut out = Vec::new();
        for r in rows {
            if let Declaration::Type(t) =
                decode::<Declared>(&r.get::<_, String>(0), "a declaration")?.declaration
            {
                out.push(t);
            }
        }
        Ok(out)
    }
}

impl ProjectionView for PgView<'_> {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.read(Vec::new(), |r| {
            let at = r.at();
            r.db.query(
                &format!(
                    "SELECT state FROM {} WHERE {} ORDER BY ident COLLATE \"C\"",
                    r.names.t("declarations"),
                    holds_at(1)
                ),
                &[&at],
            )?
            .iter()
            .map(|row| decode(&row.get::<_, String>(0), "a declaration"))
            .collect()
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.single(None, |r| r.record(ty, key))
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            let decl = r.type_decl(ty)?;
            let narrowed = match (
                r.head,
                scan.filter.as_ref().and_then(engines::exact_text_term),
            ) {
                (true, Some((field, text))) => {
                    let query = format!(
                        "SELECT key FROM {} WHERE ty = $1 AND field = $2 AND value = $3",
                        r.names.t("text_index")
                    );
                    Some(r.keys(&query, &[&ty.as_str(), &field.as_str(), &text])?)
                }
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
                // Ordered by a declared field at the head, the page is read
                // in the order index's order and stops when it is full.
                None if r.sorted && engines::by_order_index(decl.as_ref(), scan).is_some() => {
                    let first = engines::by_order_index(decl.as_ref(), scan)
                        .expect("checked above")
                        .clone();
                    let r = std::cell::RefCell::new(r);
                    return engines::scan_in_field_order(
                        decl.as_ref(),
                        scan,
                        |from, n| r.borrow_mut().sorted_after(ty, &first, from, n),
                        |after, n| r.borrow_mut().unsorted_after(ty, &first.field, after, n),
                    );
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
                let table = r.names.t("edges");
                if out {
                    edges.extend(r.edge_rows(
                        &format!(
                            "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM {table} \
                             WHERE src_ty = $1 AND src_key = $2"
                        ),
                        ty.as_str(),
                        key.as_str(),
                    )?);
                }
                if inn {
                    edges.extend(r.edge_rows(
                        &format!(
                            "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM {table} \
                             WHERE dst_ty = $1 AND dst_key = $2"
                        ),
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
            let head = r.head;
            let text_index = r.names.t("text_index");
            let linked = lakelands_core::links::link_edges(
                ty,
                key,
                relation,
                walk,
                &types,
                |lt, field, k| {
                    if head {
                        let query = format!(
                            "SELECT key FROM {text_index} WHERE ty = $1 AND field = $2 AND value = $3"
                        );
                        let keys = r.keys(&query, &[&lt.as_str(), &field, &k.as_str()])?;
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
            )?;
            edges.extend(linked);
            engines::order_edges(&mut edges);
            Ok(edges)
        })
    }

    fn lexical(&self, ty: &TypeName, text: &str, k: u32) -> Result<Vec<(RecordKey, f64)>> {
        self.read(Vec::new(), |r| {
            // The full-text rows describe the head; a view keeps the hits
            // live at its own position (`docs/seams.md`).
            let Some(decl) = projection::type_at(r.db, r.names, ty, None)? else {
                return Ok(Vec::new());
            };
            let query: Vec<String> = engines::tokens(text, decl.language.as_deref().unwrap_or(""))
                .into_iter()
                .collect();
            if query.is_empty() {
                return Ok(Vec::new());
            }
            // The candidates are the records holding any query token: the
            // set the shared scoring counts.
            let sql = format!(
                "SELECT DISTINCT key FROM {} WHERE ty = $1 AND token = ANY($2::text[])",
                r.names.t("full_text")
            );
            let keys = r.keys(&sql, &[&ty.as_str(), &query])?;
            let mut head = Vec::with_capacity(keys.len());
            let mut visible = BTreeSet::new();
            for key in keys {
                let key = RecordKey::from(key);
                if let Some(record) = projection::head_record(r.db, r.names, ty, &key)? {
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
            let at = r.at();
            let all: Vec<Proposal> =
                r.db.query(
                    &format!(
                        "SELECT state FROM {} WHERE {}",
                        r.names.t(PROPOSALS.table),
                        holds_at(1)
                    ),
                    &[&at],
                )?
                .iter()
                .map(|row| decode(&row.get::<_, String>(0), "a proposal"))
                .collect::<Result<_>>()?;
            let verbs = r.verbs()?;
            Ok(engines::proposals(&all, filter, |v| verbs.get(v).cloned()))
        })
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.single(false, |r| {
            let at = r.at();
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT state FROM {} WHERE object_set = $1 AND ty = $2 AND key = $3 \
                         AND {}",
                        r.names.t("members"),
                        holds_at(4)
                    ),
                    &[&set, &ty.as_str(), &key.as_str(), &at],
                )?
                .is_some_and(|row| row.get::<_, bool>(0)))
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.single(None, |r| {
            let at = r.at();
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT at FROM {} WHERE id = $1 AND at <= $2",
                        r.names.t("applied")
                    ),
                    &[&id.0, &at],
                )?
                .map(|row| Position(row.get::<_, i64>(0) as u64)))
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.single(None, |r| {
            let at = r.at();
            r.db.query_opt(
                &format!(
                    "SELECT state FROM {} WHERE seam = $1 AND input = $2 AND {}",
                    r.names.t("outputs"),
                    holds_at(3)
                ),
                &[&seam, &input.0, &at],
            )?
            .map(|row| decode(&row.get::<_, String>(0), "an output"))
            .transpose()
        })
    }
}
