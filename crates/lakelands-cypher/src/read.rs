//! The translation of a parsed statement into one [`Pattern`], clause by
//! clause in the order the statement writes them: never reordered, and
//! with no choice of a cheaper start.
//!
//! Each node a part binds is a step, named by its variable; a part
//! begins at a name bound before it, the start's for the first. A
//! condition goes to the step that binds the last name it reads, where
//! that step reads its own record by bare field and every name bound
//! before it as `<name>.<field>`. A step reads its own record's fields
//! only, not its `key`, `_type` or `_depth`, so a condition on those
//! goes to the next step of its clause that may carry it, which reads
//! them of a name bound before it; the start's key equal to a text is
//! the start's `key`.

use std::collections::BTreeMap;

use lakelands_api::error::{Case, Construct};
use lakelands_api::expr::{CmpOp, Condition, Expr, Literal, children, paths};
use lakelands_api::query::{Pattern, Projection, Start, Step};
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Direction, GroupKey, Order, OrderBy};
use lakelands_api::{FieldName, Params, ReadAt, RecordKey, RelationName, TypeName};

use crate::parse::{Absent, Kind, Match, Name, Node, Part, Query, Rel, Result, Return, Where};
use crate::parse::{absent_marker, outside};

/// The columns a row holds of every name, which a step does not read of
/// its own record.
const COLUMNS: [&str; 3] = ["key", "_type", "_depth"];

/// What a slot of the pattern binds: the start at 0, each step after.
struct Slot {
    /// The name the store binds it under; empty for a join or an
    /// absent step, which bind none.
    name: String,
    /// Whether another step may read it by `name`: not where the node
    /// names neither a variable nor a label, which the store names by
    /// the type its relation reaches.
    known: bool,
    label: Option<TypeName>,
    optional: bool,
    clause: usize,
}

/// A condition waiting for its step: what it reads, by slot, and where
/// it may go.
struct Pending {
    expr: Expr,
    reads: Vec<(usize, usize)>,
    /// The slot whose own record it is about, for a node's map.
    own: Option<usize>,
}

struct Build {
    start: Start,
    steps: Vec<Step>,
    slots: Vec<Slot>,
    vars: BTreeMap<String, usize>,
}

/// The pattern a statement translates into.
pub(crate) fn pattern(q: Query) -> Result<Pattern> {
    let mut b = Build {
        start: Start {
            ty: None,
            key: None,
            filter: None,
            search: None,
            rerank: None,
            params: Params::new(),
            named: None,
        },
        steps: Vec::new(),
        slots: Vec::new(),
        vars: BTreeMap::new(),
    };
    let mut project = None;
    for (c, clause) in q.clauses.into_iter().enumerate() {
        match clause.kind {
            Kind::Match(m) => {
                let absent = m.filter.iter().flat_map(|w| &w.absent);
                plain(m.parts.iter().chain(absent.map(|a| &a.part)))?;
                b.clause(c, clause.at, m)?
            }
            Kind::Return(r) => project = Some(b.returning(r)?),
            _ => unreachable!("a statement that writes is the write translation's"),
        }
    }
    let (project, order_by, limit) = project.expect("a statement ends in RETURN");
    Ok(Pattern {
        start: b.start,
        steps: b.steps,
        to: None,
        project,
        order_by,
        limit,
        cursor: None,
        at: ReadAt::default(),
    })
}

/// A read's relationships carry neither a variable nor a map: a link's
/// fields are read through its link type.
fn plain<'a>(parts: impl Iterator<Item = &'a Part>) -> Result<()> {
    for rel in parts.flat_map(|p| &p.rels) {
        if let Some(v) = &rel.var {
            return Err(outside(Construct::RelationshipVariable, v.at));
        }
        if let Some((at, _)) = rel.props {
            return Err(outside(Construct::RelationshipProperties, at));
        }
    }
    Ok(())
}

fn path(segments: [&str; 2]) -> Expr {
    Expr::Path {
        path: segments.map(FieldName::from).to_vec(),
    }
}

fn eq(lhs: Expr, rhs: Expr) -> Expr {
    Expr::Cmp {
        cmp: CmpOp::Eq,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn text(s: &str) -> Expr {
    Expr::Lit {
        value: Literal::Text(s.to_owned()),
    }
}

/// A path's segments joined by `.`, as a row names a field.
fn dotted(path: &[FieldName]) -> FieldName {
    let segments: Vec<&str> = path.iter().map(FieldName::as_str).collect();
    FieldName::from(segments.join("."))
}

/// Every path of `e` mapped by `f`.
pub(crate) fn map_paths(e: Expr, f: &impl Fn(Vec<FieldName>) -> Vec<FieldName>) -> Expr {
    let boxed = |e: Box<Expr>| Box::new(map_paths(*e, f));
    let all = |args: Vec<Expr>| args.into_iter().map(|a| map_paths(a, f)).collect();
    match e {
        Expr::Path { path } => Expr::Path { path: f(path) },
        Expr::Lit { value } => Expr::Lit { value },
        Expr::Cmp { cmp, lhs, rhs } => Expr::Cmp {
            cmp,
            lhs: boxed(lhs),
            rhs: boxed(rhs),
        },
        Expr::Arith { arith, lhs, rhs } => Expr::Arith {
            arith,
            lhs: boxed(lhs),
            rhs: boxed(rhs),
        },
        Expr::Neg { arg } => Expr::Neg { arg: boxed(arg) },
        Expr::Not { arg } => Expr::Not { arg: boxed(arg) },
        Expr::IsNull { arg } => Expr::IsNull { arg: boxed(arg) },
        Expr::And { args } => Expr::And { args: all(args) },
        Expr::Or { args } => Expr::Or { args: all(args) },
        Expr::In { arg, list } => Expr::In {
            arg: boxed(arg),
            list: all(list),
        },
        Expr::Call { func, args } => Expr::Call {
            func,
            args: all(args),
        },
    }
}

/// Whether `e` holds a marker of a part that must not match, and which.
fn marker_in(e: &Expr, absent: usize) -> Option<usize> {
    (0..absent)
        .find(|i| *e == absent_marker(*i))
        .or_else(|| children(e).find_map(|c| marker_in(c, absent)))
}

impl Build {
    fn binds(&self, slot: usize) -> bool {
        !self.slots[slot].name.is_empty()
    }

    /// The slot each name reads, in order; one nothing bound is refused.
    fn reads(&self, names: &[Name]) -> Result<Vec<(usize, usize)>> {
        let slot = |n: &Name| {
            let unbound = || Case::CypherUnbound {
                name: n.name.clone(),
                offset: n.at,
            };
            self.vars
                .get(&n.name)
                .map(|s| (*s, n.at))
                .ok_or_else(unbound)
        };
        names.iter().map(slot).collect()
    }

    /// A step from `slot` in clause `c` is past an optional name where
    /// openCypher would not be: from a `MATCH`, or from another
    /// `OPTIONAL MATCH`.
    fn past_optional(&self, slot: usize, c: usize, optional: bool, at: usize) -> Result<()> {
        let s = &self.slots[slot];
        match (s.optional, optional) {
            (true, false) => Err(outside(Construct::MatchFromOptional, at)),
            (true, true) if s.clause != c => Err(outside(Construct::OptionalFromOptional, at)),
            _ => Ok(()),
        }
    }

    /// The `from` a step names: none where it walks from the step
    /// before it, which binds a name.
    fn from(&self, slot: usize) -> Option<String> {
        let default = slot == self.steps.len() && self.binds(slot);
        (!default).then(|| self.slots[slot].name.clone())
    }

    fn clause(&mut self, c: usize, at: usize, m: Match) -> Result<()> {
        let first = self.slots.len();
        let mut pending = Vec::new();
        // Where each part hangs from a name bound before the clause.
        let mut roots = Vec::new();
        for part in m.parts {
            self.part(c, m.optional, part, &mut pending, &mut roots)?;
        }
        if m.optional && roots.len() > 1 {
            return Err(outside(Construct::OptionalParts, roots[1]));
        }
        let mut absent = Vec::new();
        if let Some(Where {
            expr,
            names,
            absent: parts,
        }) = m.filter
        {
            let reads = self.reads(&names)?;
            let mut parts: Vec<Option<Absent>> = parts.into_iter().map(Some).collect();
            for conjunct in expr.conjuncts() {
                if let Some(i) = (0..parts.len()).find(|i| *conjunct == absent_marker(*i)) {
                    absent.push(parts[i].take().expect("a marker stands once"));
                    continue;
                }
                if let Some(i) = marker_in(conjunct, parts.len()) {
                    let a = parts[i].as_ref().expect("a marker stands once");
                    return Err(outside(Construct::Exists, a.at));
                }
                let mut ps = Vec::new();
                paths(conjunct, &mut ps);
                let read: Vec<(usize, usize)> = (ps.iter())
                    .filter_map(|p| {
                        reads
                            .iter()
                            .zip(&names)
                            .find(|(_, n)| n.name == p[0].as_str())
                    })
                    .map(|(r, _)| *r)
                    .collect();
                // A name alone is a record, which no condition compares.
                if let Some(p) = ps.iter().find(|p| p.len() == 1) {
                    let n = names.iter().find(|n| n.name == p[0].as_str());
                    let offset = n.map_or(at, |n| n.at);
                    let token = ".".to_owned();
                    return Err(Case::ParseExpected { token, offset });
                }
                pending.push(Pending {
                    expr: conjunct.clone(),
                    reads: read,
                    own: None,
                });
            }
        }
        // An `OPTIONAL MATCH` that binds no name keeps every row whether
        // it matches or not: it says nothing, and stands as nothing.
        if m.optional && !(first..self.slots.len()).any(|s| self.binds(s)) {
            self.steps.truncate(first.saturating_sub(1));
            self.slots.truncate(first);
            return Ok(());
        }
        for p in pending {
            self.place(c, first, m.optional, p)?;
        }
        for a in absent {
            self.absent(c, m.optional, a)?;
        }
        Ok(())
    }

    /// One part of a clause: its first node a name bound before it, or
    /// the start, then a step for each relationship.
    fn part(
        &mut self,
        c: usize,
        optional: bool,
        part: Part,
        pending: &mut Vec<Pending>,
        roots: &mut Vec<usize>,
    ) -> Result<()> {
        let mut nodes = part.nodes.into_iter();
        let head = nodes.next().expect("a part has a node");
        let mut left = if self.slots.is_empty() {
            self.start(c, head, pending)?
        } else {
            let slot = self.bound(&head)?;
            self.past_optional(slot, c, optional, head.at)?;
            self.again(c, optional, slot, head, pending)?;
            slot
        };
        for (rel, node) in part.rels.into_iter().zip(nodes) {
            self.past_optional(left, c, optional, node.at)?;
            let first = self.slots.iter().position(|s| s.clause == c);
            let inside = |s: usize| first.is_some_and(|f| s >= f);
            let joined = node.var.as_ref().and_then(|v| self.vars.get(v)).copied();
            if let Some(j) = joined {
                self.past_optional(j, c, optional, node.at)?;
                if !inside(left) && !inside(j) {
                    roots.push(node.at);
                }
            } else if !inside(left) {
                roots.push(node.at);
            }
            let from = self.from(left);
            left = self.step(c, optional, rel, from, joined, node, pending)?;
        }
        Ok(())
    }

    /// The start: the first node, which names its type.
    fn start(&mut self, c: usize, head: Node, pending: &mut Vec<Pending>) -> Result<usize> {
        let Some(label) = head.label else {
            return Err(Case::CypherStartType { offset: head.at });
        };
        let ty = TypeName::from(label);
        self.start.ty = Some(ty.clone());
        self.start.named = head.var.clone();
        let name = head.var.clone().unwrap_or_else(|| ty.to_string());
        if let Some(v) = head.var {
            self.vars.insert(v, 0);
        }
        self.slots.push(Slot {
            name: name.clone(),
            known: true,
            label: Some(ty),
            optional: false,
            clause: c,
        });
        for p in head.props {
            if p.key == "key" {
                match (p.value, p.param) {
                    (
                        Expr::Lit {
                            value: Literal::Text(k),
                        },
                        _,
                    ) => {
                        self.start.key = Some(RecordKey::from(k));
                        continue;
                    }
                    (_, Some(param)) => {
                        let name = lakelands_api::ParamName::from(param);
                        return Err(Case::CypherParamValue { name });
                    }
                    (value, None) => {
                        let reads = self.reads(&p.names)?;
                        let expr = eq(path([&name, "key"]), value);
                        pending.push(Pending {
                            expr,
                            reads,
                            own: Some(0),
                        });
                        continue;
                    }
                }
            }
            let reads = self.reads(&p.names)?;
            let expr = eq(path([&name, &p.key]), p.value);
            pending.push(Pending {
                expr,
                reads,
                own: Some(0),
            });
        }
        Ok(0)
    }

    /// The slot a part's first node names, which a name bound before it
    /// must.
    fn bound(&self, head: &Node) -> Result<usize> {
        let name = (head.var.clone())
            .or_else(|| head.label.clone())
            .unwrap_or_else(|| "()".to_owned());
        let slot = head.var.as_ref().and_then(|v| self.vars.get(v));
        slot.copied().ok_or(Case::CypherUnbound {
            name,
            offset: head.at,
        })
    }

    /// A node that names a name bound before it: its label, if the
    /// binder named none, is the binder's type; its map, conditions on
    /// that name.
    fn again(
        &mut self,
        c: usize,
        optional: bool,
        slot: usize,
        node: Node,
        pending: &mut Vec<Pending>,
    ) -> Result<()> {
        let name = self.slots[slot].name.clone();
        if let Some(label) = node.label.map(TypeName::from) {
            let s = &self.slots[slot];
            match &s.label {
                Some(l) if *l == label => {}
                Some(_) => return Err(outside(Construct::SeveralLabels, node.at)),
                None if !optional || s.clause == c => {
                    self.steps[slot - 1].ty = Some(label.clone());
                    self.slots[slot].label = Some(label);
                }
                None => pending.push(Pending {
                    expr: eq(path([&name, "_type"]), text(label.as_str())),
                    reads: vec![(slot, node.at)],
                    own: None,
                }),
            }
        }
        for p in node.props {
            let mut reads = vec![(slot, node.at)];
            reads.extend(self.reads(&p.names)?);
            pending.push(Pending {
                expr: eq(path([&name, &p.key]), p.value),
                reads,
                own: None,
            });
        }
        Ok(())
    }

    /// A step over `rel` to `node`: a join where the node names a name
    /// bound before it, a new name otherwise.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        c: usize,
        optional: bool,
        rel: Rel,
        from: Option<String>,
        joined: Option<usize>,
        node: Node,
        pending: &mut Vec<Pending>,
    ) -> Result<usize> {
        let mut step = Step {
            relation: RelationName::from(rel.relation.clone()),
            walk: rel.walk,
            ty: None,
            filter: None,
            depth: rel.depth,
            named: None,
            from,
            join: None,
            optional,
            absent: false,
        };
        if let Some(j) = joined {
            step.join = Some(self.slots[j].name.clone());
            self.steps.push(step);
            self.slots.push(Slot {
                name: String::new(),
                known: false,
                label: None,
                optional,
                clause: c,
            });
            self.again(c, optional, j, node, pending)?;
            return Ok(j);
        }
        let label = node.label.map(TypeName::from);
        let known = node.var.is_some() || label.is_some();
        step.ty = label.clone();
        step.named = node.var.clone();
        let name = (node.var.clone())
            .or_else(|| label.as_ref().map(TypeName::to_string))
            .unwrap_or(rel.relation);
        self.steps.push(step);
        let slot = self.slots.len();
        self.slots.push(Slot {
            name: name.clone(),
            known,
            label,
            optional,
            clause: c,
        });
        if let Some(v) = node.var {
            self.vars.insert(v, slot);
        }
        for p in node.props {
            let reads = self.reads(&p.names)?;
            pending.push(Pending {
                expr: eq(path([&name, &p.key]), p.value),
                reads,
                own: Some(slot),
            });
        }
        Ok(slot)
    }

    /// A condition on the step its clause's rule names: the step binding
    /// the last name it reads, and in an `OPTIONAL MATCH` none before the
    /// clause's first, so it decides whether the optional part matched.
    fn place(&mut self, c: usize, first: usize, optional: bool, p: Pending) -> Result<()> {
        for &(slot, at) in &p.reads {
            if self.slots[slot].optional && !optional {
                return Err(outside(Construct::MatchFromOptional, at));
            }
        }
        // A step of the clause that may carry it: an absent step's
        // `where` says what must not match, never what must.
        let carries = |b: &Build, s: usize| {
            let step = &b.steps[s - 1];
            !step.absent && step.optional == optional && b.slots[s].clause == c
        };
        let last = p.reads.iter().map(|r| r.0).max().unwrap_or(0);
        let mut target = last.max(p.own.unwrap_or(0));
        if optional && p.own.is_none() {
            // A clause of no step matches whatever it says: the
            // condition decides nothing.
            let Some(f) = (first.max(1)..self.slots.len()).find(|s| carries(self, *s)) else {
                return Ok(());
            };
            target = target.max(f);
        }
        let own = self.slots[target].name.clone();
        let mut ps = Vec::new();
        paths(&p.expr, &mut ps);
        let column = |p: &&[FieldName]| {
            p.len() > 1 && p[0].as_str() == own && COLUMNS.contains(&p[1].as_str())
        };
        if !own.is_empty() && ps.iter().any(column) {
            if target == 0
                && self.start.key.is_none()
                && let Some(key) = start_key(&p.expr, &own)
            {
                self.start.key = Some(key);
                return Ok(());
            }
            // Where no later step reads it, it stays on its own step as
            // written, and the store judges it there.
            let later = (target + 1..self.slots.len()).find(|s| carries(self, *s));
            if let Some(later) = later.filter(|_| self.slots[target].known) {
                target = later;
            }
        }
        let own = self.slots[target].name.clone();
        let strip = |path: Vec<FieldName>| match path.split_first() {
            Some((head, rest))
                if head.as_str() == own
                    && !rest.is_empty()
                    && !COLUMNS.contains(&rest[0].as_str()) =>
            {
                rest.to_vec()
            }
            _ => path,
        };
        let expr = map_paths(p.expr, &strip);
        let filter = match target {
            0 => &mut self.start.filter,
            s => &mut self.steps[s - 1].filter,
        };
        *filter = Some(Condition::Expr(match filter.take() {
            Some(Condition::Expr(Expr::And { mut args })) => {
                args.push(expr);
                Expr::And { args }
            }
            Some(Condition::Expr(e)) => Expr::And {
                args: vec![e, expr],
            },
            _ => expr,
        }));
        Ok(())
    }

    /// A part that must not match: one relationship from a name bound
    /// before it, its far node's label, map and the part's condition as
    /// the step's `where`, or a join where the far node names a bound
    /// name.
    fn absent(&mut self, c: usize, optional: bool, a: Absent) -> Result<()> {
        let Absent { at, part, filter } = a;
        if part.rels.len() != 1 {
            return Err(outside(Construct::Exists, at));
        }
        let mut nodes = part.nodes.into_iter();
        let (head, far) = (
            nodes.next().expect("two nodes"),
            nodes.next().expect("two nodes"),
        );
        let rel = part.rels.into_iter().next().expect("one relationship");
        let slot = self.bound(&head)?;
        self.past_optional(slot, c, optional, head.at)?;
        let joined = far.var.as_ref().and_then(|v| self.vars.get(v)).copied();
        if let Some(j) = joined {
            self.past_optional(j, c, optional, far.at)?;
        }
        let mut conds: Vec<(Expr, Vec<(usize, usize)>)> = Vec::new();
        let head_name = self.slots[slot].name.clone();
        if let Some(label) = head.label.map(TypeName::from)
            && self.slots[slot].label.as_ref() != Some(&label)
        {
            let e = eq(path([&head_name, "_type"]), text(label.as_str()));
            conds.push((e, vec![(slot, head.at)]));
        }
        for p in head.props {
            let mut reads = vec![(slot, head.at)];
            reads.extend(self.reads(&p.names)?);
            conds.push((eq(path([&head_name, &p.key]), p.value), reads));
        }
        // The far node's own name is local to the part: its fields are
        // the step's own, read bare, and its `key`, `_type` and `_depth`
        // stay as written, for the store to judge.
        let local = match joined {
            Some(j) => self.slots[j].name.clone(),
            None => (far.var.clone())
                .or_else(|| far.label.clone())
                .unwrap_or_else(|| rel.relation.clone()),
        };
        let own = |path: Vec<FieldName>| match path.split_first() {
            Some((h, rest))
                if joined.is_none()
                    && h.as_str() == local
                    && !COLUMNS.contains(&rest[0].as_str()) =>
            {
                rest.to_vec()
            }
            _ => path,
        };
        let mut wheres = Vec::new();
        for p in far.props {
            let reads = self.reads(&p.names)?;
            conds.push((eq(path([&local, &p.key]), p.value), reads));
        }
        if let Some(w) = filter {
            let inner = |n: &&Name| joined.is_none() && far.var.as_deref() == Some(n.name.as_str());
            let outer: Vec<Name> = w.names.iter().filter(|n| !inner(n)).cloned().collect();
            let reads = self.reads(&outer)?;
            for conjunct in w.expr.conjuncts() {
                let mut ps = Vec::new();
                paths(conjunct, &mut ps);
                // A name alone is a record, which no condition compares.
                if let Some(p) = ps.iter().find(|p| p.len() == 1) {
                    let n = w.names.iter().find(|n| n.name == p[0].as_str());
                    let offset = n.map_or(at, |n| n.at);
                    let token = ".".to_owned();
                    return Err(Case::ParseExpected { token, offset });
                }
                conds.push((conjunct.clone(), reads.clone()));
            }
        }
        for (e, reads) in conds {
            for (s, at) in reads {
                if self.slots[s].optional && !optional {
                    return Err(outside(Construct::MatchFromOptional, at));
                }
            }
            wheres.push(map_paths(e, &own));
        }
        let ty = far
            .label
            .map(TypeName::from)
            .filter(|l| joined.is_none_or(|j| self.slots[j].label.as_ref() != Some(l)));
        let from = self.from(slot);
        self.steps.push(Step {
            relation: RelationName::from(rel.relation),
            walk: rel.walk,
            ty,
            filter: (!wheres.is_empty()).then(|| {
                Condition::Expr(match wheres.len() {
                    1 => wheres.pop().expect("one condition"),
                    _ => Expr::And { args: wheres },
                })
            }),
            depth: rel.depth,
            named: None,
            from,
            join: joined.map(|j| self.slots[j].name.clone()),
            optional,
            absent: true,
        });
        self.slots.push(Slot {
            name: String::new(),
            known: false,
            label: None,
            optional,
            clause: c,
        });
        Ok(())
    }

    /// The projection, the order and the limit `RETURN` says.
    fn returning(&self, r: Return) -> Result<(Projection, Vec<Order>, Option<u32>)> {
        for item in &r.items {
            self.reads(&item.names)?;
        }
        let mut order_by = Vec::new();
        for s in r.order {
            self.reads(&s.names)?;
            let Expr::Path { path } = s.expr.clone() else {
                return Err(Case::CypherOrderItem { item: s.text });
            };
            if path.len() < 2 {
                return Err(Case::CypherOrderItem { item: s.text });
            }
            let dir = if s.desc {
                Direction::Desc
            } else {
                Direction::Asc
            };
            order_by.push(Order {
                field: dotted(&path),
                by: OrderBy::Value,
                dir,
            });
        }
        let project = if r.star {
            Projection::Rows { fields: Vec::new() }
        } else if r.items.iter().any(|i| !i.aggregates.is_empty()) {
            Projection::Aggregate(self.aggregate(r.items)?)
        } else {
            self.rows(r.items)?
        };
        Ok((project, order_by, r.limit))
    }

    fn rows(&self, items: Vec<crate::parse::Item>) -> Result<Projection> {
        let mut fields = Vec::new();
        let mut whole = Vec::new();
        for item in items {
            match (&item.expr, &item.alias) {
                (Expr::Path { path }, None) if path.len() > 1 => {
                    fields.push(dotted(path).to_string())
                }
                (Expr::Path { path }, None) => whole.push((path[0].to_string(), item.text)),
                _ => return Err(Case::CypherRowItem { item: item.text }),
            }
        }
        if whole.is_empty() {
            return Ok(Projection::Rows { fields });
        }
        // Every bound name, and nothing beside them, is every field.
        let named: std::collections::BTreeSet<&String> = self.vars.keys().collect();
        let given: std::collections::BTreeSet<&String> = whole.iter().map(|w| &w.0).collect();
        if fields.is_empty() && named == given {
            return Ok(Projection::Rows { fields });
        }
        let (_, item) = whole.into_iter().next().expect("one name");
        Err(Case::CypherRowItem { item })
    }

    fn aggregate(&self, items: Vec<crate::parse::Item>) -> Result<Aggregate> {
        let (mut measures, mut group_by) = (Vec::new(), Vec::new());
        for item in items {
            if let Some(&at) = item.aggregates.get(1) {
                return Err(outside(Construct::AggregateInExpression, at));
            }
            let alias = item.alias.map(FieldName::from);
            let Expr::Call { func, args } = &item.expr else {
                if let Some(&at) = item.aggregates.first() {
                    return Err(outside(Construct::AggregateInExpression, at));
                }
                match (&item.expr, alias) {
                    (Expr::Path { path }, None) if path.len() > 1 => {
                        group_by.push(GroupKey::Field(dotted(path)));
                    }
                    (Expr::Path { path }, _) if path.len() == 1 => {
                        return Err(Case::CypherRowItem { item: item.text });
                    }
                    (_, Some(name)) => group_by.push(GroupKey::Named {
                        name,
                        expr: Condition::Expr(item.expr),
                    }),
                    (_, None) => return Err(Case::CypherAlias { item: item.text }),
                }
                continue;
            };
            if !func.is_aggregate() {
                if let Some(&at) = item.aggregates.first() {
                    return Err(outside(Construct::AggregateInExpression, at));
                }
                let Some(name) = alias else {
                    return Err(Case::CypherAlias { item: item.text });
                };
                group_by.push(GroupKey::Named {
                    name,
                    expr: Condition::Expr(item.expr),
                });
                continue;
            }
            let Some(name) = alias else {
                return Err(Case::CypherAlias { item: item.text });
            };
            let one = args.first();
            let measure = match (func, one) {
                (lakelands_api::expr::Func::Count, None) => (AggFunc::Count, None),
                (lakelands_api::expr::Func::Count, Some(Expr::Path { path }))
                    if path.len() == 1 =>
                {
                    let slot = self.vars[path[0].as_str()];
                    if self.slots[slot].optional {
                        return Err(Case::CypherCount { item: item.text });
                    }
                    (AggFunc::Count, None)
                }
                (lakelands_api::expr::Func::Count, _) => {
                    return Err(Case::CypherCount { item: item.text });
                }
                (f, Some(Expr::Path { path })) if path.len() > 1 => {
                    let func = match f {
                        lakelands_api::expr::Func::Sum => AggFunc::Sum,
                        lakelands_api::expr::Func::Min => AggFunc::Min,
                        lakelands_api::expr::Func::Max => AggFunc::Max,
                        _ => AggFunc::Avg,
                    };
                    (func, Some(dotted(path)))
                }
                _ => {
                    let at = item.aggregates[0];
                    return Err(outside(Construct::AggregateInExpression, at));
                }
            };
            measures.push(AggMeasure {
                name,
                func: measure.0,
                field: measure.1,
            });
        }
        Ok(Aggregate { measures, group_by })
    }
}

/// The text a condition `<name>.key = '<key>'` compares the start's key
/// with, where it is that and nothing else.
fn start_key(e: &Expr, name: &str) -> Option<RecordKey> {
    let Expr::Cmp {
        cmp: CmpOp::Eq,
        lhs,
        rhs,
    } = e
    else {
        return None;
    };
    let key = |p: &Expr, v: &Expr| match (p, v) {
        (
            Expr::Path { path },
            Expr::Lit {
                value: Literal::Text(k),
            },
        ) if path.len() == 2 && path[0].as_str() == name && path[1].as_str() == "key" => {
            Some(RecordKey::from(k.clone()))
        }
        _ => None,
    };
    key(lhs, rhs).or_else(|| key(rhs, lhs))
}
