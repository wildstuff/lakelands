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
use postgres::types::ToSql;

use crate::connect::Db;
use crate::handle::PgLake;
use crate::projection::{self, Names, PROPOSALS, decode, decode_record, holds_at};

/// A read view of one lake's projection at one position.
pub(crate) struct PgView<'a> {
    pub(crate) lake: &'a PgLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, the head in the state it
/// sees, and whether the view's position is that head.
struct Read<'c> {
    db: &'c mut Db,
    names: &'c Names,
    at: i64,
    folded: Option<i64>,
    head: bool,
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
        // The transaction opens and reads the folded marker in one round
        // trip, so the head flag is of the state every query in it sees.
        let folded = db.simple(&format!(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; \
             SELECT n FROM {} WHERE k = 'folded'",
            names.t("meta")
        ));
        let answer = folded.and_then(|folded| {
            let folded = folded.into_iter().flatten().next();
            f(&mut Read {
                db: &mut db,
                names,
                at: at.0 as i64,
                folded: folded.as_deref().and_then(|f| f.parse().ok()),
                head: folded == Some(at.0.to_string()),
            })
        });
        db.end(answer)
    }

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
            at: at.0 as i64,
            folded: None,
            head: false,
        })
    }
}

impl Read<'_> {
    fn type_decl(&mut self, ty: &TypeName) -> Result<Option<TypeDecl>> {
        projection::type_at(self.db, self.names, ty, Some(Position(self.at as u64)))
    }

    fn record(&mut self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.db
            .query_opt(
                &format!(
                    "SELECT state FROM {} WHERE ty = $1 AND key = $2 AND {}",
                    self.names.t("records"),
                    holds_at(3)
                ),
                &[&ty.as_str(), &key.as_str(), &self.at],
            )?
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&mut self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        self.db.records(
            &format!(
                "SELECT state FROM {} WHERE ty = $1 AND {} ORDER BY key COLLATE \"C\"",
                self.names.t("records"),
                holds_at(2)
            ),
            &[&ty.as_str(), &self.at],
        )
    }

    /// How many records of a type are in force at the view's position, the
    /// retired and the restricted ones only when asked, their flags read
    /// from the stored state.
    fn count_in_force(&mut self, ty: &TypeName, retired: bool, restricted: bool) -> Result<u64> {
        let query = format!(
            "SELECT COUNT(*) FROM {} WHERE ty = $1 AND {} \
             AND ($3 OR NOT (state::jsonb ->> 'retired')::boolean) \
             AND ($4 OR NOT (state::jsonb ->> 'restricted')::boolean)",
            self.names.t("records"),
            holds_at(2)
        );
        let counted: Vec<i64> = self
            .db
            .column(&query, &[&ty.as_str(), &self.at, &retired, &restricted])?;
        Ok(counted.first().map_or(0, |n| n.unsigned_abs()))
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
        let limit = i64::from(n);
        let from = after.map_or("", |_| "AND key COLLATE \"C\" > $4");
        let query = format!(
            "SELECT state FROM {} WHERE ty = $1 AND {} {from} ORDER BY key COLLATE \"C\" LIMIT $3",
            self.names.t("records"),
            holds_at(2)
        );
        // `$4` is bound only when the statement names it.
        let args: [&(dyn ToSql + Sync); 4] = [&ty.as_str(), &self.at, &limit, &after];
        self.db
            .records(&query, &args[..3 + usize::from(after.is_some())])
    }

    /// The fields of `ty` whose order index answers this read, in one round
    /// trip: none unless the index is kept through the head. At an earlier
    /// position it is read with the keys changed since ([`Read::changed`]).
    fn sorted_fields(&mut self, ty: &TypeName, decl: Option<&TypeDecl>) -> Result<Vec<FieldName>> {
        let Some(decl) = decl.filter(|_| self.folded.is_some()) else {
            return Ok(Vec::new());
        };
        let row = self.db.query_one(
            &format!(
                "SELECT (SELECT n FROM {m} WHERE k = $1), \
                 (SELECT state FROM {r} WHERE ty = $2 AND key = $3 AND until IS NULL)",
                m = self.names.t("meta"),
                r = self.names.t("records"),
            ),
            &[
                &crate::projection::sorted_mark(ty),
                &lakelands_api::seam::SETTING_TYPE,
                &engines::sort_index_key(ty),
            ],
        )?;
        if row.get::<_, Option<i64>>(0) != self.folded {
            return Ok(Vec::new());
        }
        let setting = row
            .get::<_, Option<String>>(1)
            .map(|state| decode_record(&state))
            .transpose()?;
        Ok(engines::sorted_fields(
            decl,
            setting.as_ref().and_then(engines::setting_value),
        ))
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
        let (k, key) = from.unzip();
        let args: [&(dyn ToSql + Sync); 5] =
            [&ty.as_str(), &order.field.as_str(), &limit, &k, &key];
        let rows = self
            .db
            .query(&query, &args[..if from.is_some() { 5 } else { 3 }])?;
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
        let from = after.map_or("", |_| "AND r.key COLLATE \"C\" > $4");
        let query = format!(
            "SELECT r.state FROM {records} r WHERE r.ty = $1 AND r.until IS NULL {from} \
             AND NOT EXISTS (SELECT 1 FROM {sorted} s \
             WHERE s.ty = r.ty AND s.field = $2 AND s.key = r.key) \
             ORDER BY r.key COLLATE \"C\" LIMIT $3",
            sorted = self.names.t("sort_index"),
            records = self.names.t("records"),
        );
        let args: [&(dyn ToSql + Sync); 4] = [&ty.as_str(), &field.as_str(), &limit, &after];
        self.db
            .records(&query, &args[..3 + usize::from(after.is_some())])
    }

    /// The keys of `ty` with a state put or ended after the view's
    /// position, and their states there; `None` past `ceiling` states put
    /// or keys ended. Two range reads, one into `records_since` for the
    /// states put, one into `records_at` for the states that held at the
    /// position and ended.
    fn changed(&mut self, ty: &TypeName, ceiling: usize) -> Result<Option<engines::Changed>> {
        let records = self.names.t("records");
        let args: [&(dyn ToSql + Sync); 3] = [&ty.as_str(), &self.at, &(ceiling as i64 + 1)];
        let put: Vec<String> = self.db.column(
            &format!(
                "SELECT key FROM {records} WHERE ty = $1 AND since > $2 ORDER BY since LIMIT $3"
            ),
            &args,
        )?;
        let ended: Vec<String> = self.db.column(
            &format!(
                "SELECT key FROM {records} WHERE ty = $1 AND until > $2 AND since <= $2 \
                 ORDER BY until LIMIT $3"
            ),
            &args,
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

    /// Records of a type by their keys, at the view's position, in key
    /// order: one statement, however many keys.
    fn records_by_key(
        &mut self,
        ty: &TypeName,
        keys: BTreeSet<String>,
    ) -> Result<Vec<StoredRecord>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = keys.into_iter().collect();
        self.db.records(
            &format!(
                "SELECT state FROM {} WHERE ty = $1 AND key = ANY($2::text[]) AND {} \
                 ORDER BY key COLLATE \"C\"",
                self.names.t("records"),
                holds_at(3)
            ),
            &[&ty.as_str(), &keys, &self.at],
        )
    }

    /// The type each verb declared at the view's position is on.
    fn verbs(&mut self) -> Result<std::collections::BTreeMap<VerbName, TypeName>> {
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {} WHERE ident LIKE 'verb %' AND {}",
                self.names.t("declarations"),
                holds_at(1)
            ),
            &[&self.at],
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

    /// The rows of the head's `edges` table whose `end` (`src` or `dst`)
    /// is a record of `ty` among `keys`: one statement for all of them.
    fn edge_rows(&mut self, end: &str, ty: &TypeName, keys: &[String]) -> Result<Vec<Edge>> {
        let query = format!(
            "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM {} \
             WHERE {end}_ty = $1 AND {end}_key = ANY($2::text[])",
            self.names.t("edges")
        );
        Ok(self
            .db
            .query(&query, &[&ty.as_str(), &keys])?
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
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {} WHERE ident LIKE 'type %' AND {} \
                 ORDER BY ident COLLATE \"C\"",
                self.names.t("declarations"),
                holds_at(1)
            ),
            &[&self.at],
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.extend(projection::declared_type(Some(r.get(0)))?);
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
            r.db.query(
                &format!(
                    "SELECT state FROM {} WHERE {} ORDER BY ident COLLATE \"C\"",
                    r.names.t("declarations"),
                    holds_at(1)
                ),
                &[&r.at],
            )?
            .iter()
            .map(|row| decode(&row.get::<_, String>(0), "a declaration"))
            .collect()
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.single(None, |r| r.record(ty, key))
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.single(Vec::new(), |r| {
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
                (true, Some((field, text))) => {
                    let query = format!(
                        "SELECT key FROM {} WHERE ty = $1 AND field = $2 AND value = $3",
                        r.names.t("text_index")
                    );
                    Some(r.db.column(&query, &[&ty.as_str(), &field.as_str(), &text])?)
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
                // Ordered by a field the lake indexes, at the head, the page
                // is read in the order index's order and stops when it is
                // full.
                None => {
                    let sorted = r.sorted_fields(ty, decl.as_ref())?;
                    match engines::by_order_index(decl.as_ref(), &sorted, scan) {
                        Some(first) => {
                            // At an earlier position, the head's index with
                            // the keys changed since; past the ceiling, the
                            // sort.
                            let changed = if r.head {
                                None
                            } else {
                                let ceiling = engines::changed_ceiling(scan.limit);
                                let Some(changed) = r.changed(ty, ceiling)? else {
                                    let records = r.records(ty)?;
                                    return Ok(engines::scan(decl.as_ref(), &records, scan));
                                };
                                Some(changed)
                            };
                            let r = std::cell::RefCell::new(r);
                            let sorted = |from: Option<(&[u8], &str)>, n| {
                                r.borrow_mut().sorted_after(ty, first, from, n)
                            };
                            let missing = |after: Option<&str>, n| {
                                r.borrow_mut().unsorted_after(ty, &first.field, after, n)
                            };
                            return match &changed {
                                None => engines::scan_in_field_order(
                                    decl.as_ref(),
                                    scan,
                                    sorted,
                                    missing,
                                ),
                                Some(c) => engines::scan_in_field_order_at(
                                    decl.as_ref(),
                                    scan,
                                    c,
                                    sorted,
                                    missing,
                                ),
                            };
                        }
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
        // One read transaction for all the keys, each of its reads one
        // statement for all of them (lakelands#227).
        self.read(vec![Vec::new(); keys.len()], |r| {
            let out = matches!(walk, Walk::Out | Walk::Both);
            let inn = matches!(walk, Walk::In | Walk::Both);
            let mut index: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
            for (i, k) in keys.iter().enumerate() {
                index.entry(k.as_str()).or_default().push(i);
            }
            let names: Vec<String> = index.keys().map(|k| (*k).to_owned()).collect();
            let mut all = vec![Vec::new(); keys.len()];
            let give = |all: &mut Vec<Vec<Edge>>, at: &str, e: &Edge| {
                for &i in index.get(at).into_iter().flatten() {
                    all[i].push(e.clone());
                }
            };
            let types = r.types()?;
            if r.head {
                let wanted = |e: &Edge| relation.is_none_or(|want| *want == e.relation);
                if out {
                    for e in r.edge_rows("src", ty, &names)?.iter().filter(|e| wanted(e)) {
                        give(&mut all, e.from.1.as_str(), e);
                    }
                }
                if inn {
                    for e in r.edge_rows("dst", ty, &names)?.iter().filter(|e| wanted(e)) {
                        give(&mut all, e.to.1.as_str(), e);
                    }
                }
            } else {
                if out && let Some(decl) = r.type_decl(ty)? {
                    for record in r.records_by_key(ty, names.iter().cloned().collect())? {
                        for e in engines::links_of(&decl, &record, relation) {
                            give(&mut all, record.key.as_str(), &e);
                        }
                    }
                }
                if inn {
                    // The declaring types' records at the position, read
                    // once for the page, not once per key.
                    let mut read: std::collections::BTreeMap<TypeName, Vec<StoredRecord>> =
                        Default::default();
                    for key in index.keys() {
                        let key = RecordKey::from(*key);
                        let into = engines::in_links(ty, &key, relation, &types, |name| {
                            if !read.contains_key(name) {
                                read.insert(name.clone(), r.records(name)?);
                            }
                            Ok::<_, lakelands_api::Error>(read[name].clone())
                        })?;
                        for e in into {
                            give(&mut all, key.as_str(), &e);
                        }
                    }
                }
            }
            // Relations carried by link records: the link records whose end
            // is one of the records. At the head the exact-text index finds
            // them; at an earlier position the link type's records there are
            // read. The rulebook decides which count.
            let head = r.head;
            let text_index = r.names.t("text_index");
            let linked = lakelands_core::links::link_edges(
                ty,
                keys,
                relation,
                walk,
                &types,
                |lt, field, ks| {
                    if head {
                        let values: Vec<String> = ks.iter().map(|k| k.to_string()).collect();
                        let query = format!(
                            "SELECT key FROM {text_index} WHERE ty = $1 AND field = $2 \
                             AND value = ANY($3::text[])"
                        );
                        let found = r.db.column(&query, &[&lt.as_str(), &field, &values])?;
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
            // set the shared scoring counts. The rows do not say which
            // field a token came from: limited to named fields, these are
            // a superset, and the shared scoring keeps the records whose
            // named fields match before it cuts at `k`.
            let sql = format!(
                "SELECT DISTINCT key FROM {} WHERE ty = $1 AND token = ANY($2::text[])",
                r.names.t("full_text")
            );
            let keys: BTreeSet<String> = r.db.column(&sql, &[&ty.as_str(), &query])?;
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
                fields,
                k,
            ))
        })
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.read(Vec::new(), |r| {
            let all: Vec<Proposal> =
                r.db.query(
                    &format!(
                        "SELECT state FROM {} WHERE {}",
                        r.names.t(PROPOSALS.table),
                        holds_at(1)
                    ),
                    &[&r.at],
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
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT state FROM {} WHERE object_set = $1 AND ty = $2 AND key = $3 \
                         AND {}",
                        r.names.t("members"),
                        holds_at(4)
                    ),
                    &[&set, &ty.as_str(), &key.as_str(), &r.at],
                )?
                .is_some_and(|row| row.get::<_, bool>(0)))
        })
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.single(BTreeSet::new(), |r| {
            let rows = r.db.query(
                &format!(
                    "SELECT key FROM {} WHERE object_set = $1 AND ty = $2 AND state AND {}",
                    r.names.t("members"),
                    holds_at(3)
                ),
                &[&set, &ty.as_str(), &r.at],
            )?;
            Ok(rows
                .iter()
                .map(|row| RecordKey::from(row.get::<_, String>(0)))
                .collect())
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.single(None, |r| {
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT at FROM {} WHERE id = $1 AND at <= $2",
                        r.names.t("applied")
                    ),
                    &[&id.0, &r.at],
                )?
                .map(|row| Position(row.get::<_, i64>(0) as u64)))
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.single(None, |r| {
            r.db.query_opt(
                &format!(
                    "SELECT state FROM {} WHERE seam = $1 AND input = $2 AND {}",
                    r.names.t("outputs"),
                    holds_at(3)
                ),
                &[&seam, &input.0, &r.at],
            )?
            .map(|row| decode(&row.get::<_, String>(0), "an output"))
            .transpose()
        })
    }
}
