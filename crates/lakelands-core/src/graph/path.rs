//! The shortest walk from one record to another, and the cheapest.
//!
//! Breadth-first, one level at a time, so the first time the target is
//! reached is by the fewest hops. Among equally short walks the choice
//! is fixed, not left to the order an engine answers in: each record's
//! predecessor is the first record, by `(type, key)`, of the level before
//! it that links to it, over the first such link by relation.
//!
//! The cheapest walk holds the neighbourhood within the steps' hops,
//! read level by level as the shortest is, and relaxes it in memory one
//! hop at a time. A search that settled the nearest record first would
//! find the walk that costs least with no bound on hops, which may take
//! more hops than the steps allow and so hide the cheapest walk within
//! them (`docs/decisions/026-the-store-walks-the-graph.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::slice;

use lakelands_api::error::Case;
use lakelands_api::query::{Edge, Step};
use lakelands_api::{Error, FieldName, RelationName, Result, Value};

use super::{EdgeSource, Links, Node, check_depth, hop, too_large};

/// A walk from one record to another.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    /// The records along it, the start first and the target last.
    pub nodes: Vec<Node>,
    /// The links between them: `edges[i]` joins `nodes[i]` and
    /// `nodes[i + 1]`, in whichever direction it was declared.
    pub edges: Vec<Edge>,
}

/// The shortest walk from `from` to `to` over `steps`, one walk over any
/// of their relations, each walked in its own step's direction to its
/// own step's `max`. A record reached over a step's relation is walked
/// through, or ends the walk, only where `keep(i, record)` holds for that
/// step.
///
/// `None` when the target is not reachable so, or is hidden. A walk from
/// a record to itself is that record alone, with no links, whatever its
/// links are. Refused as invalid when a step's depth goes past
/// `max_depth`, the lake's ceiling.
pub fn shortest_path<S, K>(
    source: &S,
    from: &Node,
    to: &Node,
    steps: &[Step],
    max_depth: u32,
    keep: K,
) -> Result<Option<Path>>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    if from == to {
        return Ok(Some(Path {
            nodes: vec![from.clone()],
            edges: Vec::new(),
        }));
    }
    let (mut links, mut keep) = (Links::new(source), keep);
    let mut prev: BTreeMap<Node, (Node, Edge)> = BTreeMap::new();
    let mut frontier: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    let deepest = steps.iter().map(|s| s.depth.max).max().unwrap_or(0);
    for at in 1..=deepest {
        let nears: Vec<Node> = frontier.iter().cloned().collect();
        let mut next = BTreeSet::new();
        for (near, around) in nears
            .iter()
            .zip(hop(&mut links, &nears, steps, at, &mut keep)?)
        {
            for (other, edge) in around {
                if other == *from || prev.contains_key(&other) {
                    continue;
                }
                prev.insert(other.clone(), (near.clone(), edge));
                if other == *to {
                    return Ok(Some(unwind(prev, from, to)));
                }
                next.insert(other);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// Follow the predecessors back from the target to the start.
fn unwind(mut prev: BTreeMap<Node, (Node, Edge)>, from: &Node, to: &Node) -> Path {
    let mut nodes = vec![to.clone()];
    let mut edges = Vec::new();
    let mut at = to.clone();
    while at != *from {
        let (before, edge) = prev
            .remove(&at)
            .expect("every reached record but the start has a predecessor");
        edges.push(edge);
        nodes.push(before.clone());
        at = before;
    }
    nodes.reverse();
    edges.reverse();
    Path { nodes, edges }
}

/// The cheapest walk from one record to another.
#[derive(Clone, Debug, PartialEq)]
pub struct Cheap {
    /// The records along it, the start first and the target last.
    pub nodes: Vec<Node>,
    /// The links between them: `edges[i]` joins `nodes[i]` and
    /// `nodes[i + 1]`, in whichever direction it was declared.
    pub edges: Vec<Edge>,
    /// What the walk has cost on reaching each of `nodes`, in units of
    /// 10^-9: 0 at the start.
    pub costs: Vec<i128>,
}

/// A weight's units: 10^9 to 1.
pub const WEIGHT_PLACES: u32 = 9;

/// One arc of the held neighbourhood, keyed by its near record, then its
/// link by relation, far record and ends.
type Arc = (Node, RelationName, Node, Node, Node);

/// The walk from `from` to `to` over `steps` whose links' `weight` sum
/// least, then of the fewest hops, then at each hop from the least record
/// by `(type, key)` over the least link by relation and ends. Steps are
/// walked as [`shortest_path`] walks them, `keep` included, and a link
/// is used at a hop only within its step's `max`.
///
/// Every record within the steps' hops is held, and past `max_nodes`
/// the walk is refused `too_large`. Every link held is weighed, in the
/// order of its relation and ends, so which is refused for a missing or
/// a negative weight follows from the graph alone. `None` when the target
/// is not reachable so; a walk from a record to itself is that record
/// alone, at cost 0.
pub fn cheapest<S, K>(
    source: &S,
    (from, to): (&Node, &Node),
    steps: &[Step],
    (max_depth, max_nodes): (u32, u32),
    weight: &FieldName,
    keep: K,
) -> Result<Option<Cheap>>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    if from == to {
        return Ok(Some(Cheap {
            nodes: vec![from.clone()],
            edges: Vec::new(),
            costs: vec![0],
        }));
    }
    let (mut links, mut keep) = (Links::new(source), keep);
    // Each record held, and each arc out of one with the deepest hop a
    // step that names it allows.
    let mut held: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    let mut arcs: BTreeMap<Arc, (u32, Edge)> = BTreeMap::new();
    let mut frontier = vec![from.clone()];
    let deepest = steps.iter().map(|s| s.depth.max).max().unwrap_or(0);
    for at in 1..=deepest {
        let mut next = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            if s.depth.max < at {
                continue;
            }
            let around = links.around(&frontier, slice::from_ref(&s.relation), s.walk, Some(i))?;
            for (near, one) in frontier.iter().zip(around) {
                for (far, edge) in one {
                    if !keep(i, &far)? {
                        continue;
                    }
                    let arc = (
                        near.clone(),
                        edge.relation.clone(),
                        far.clone(),
                        edge.from.clone(),
                        edge.to.clone(),
                    );
                    let upto = &mut arcs.entry(arc).or_insert((0, edge)).0;
                    *upto = (*upto).max(s.depth.max);
                    if held.insert(far.clone()) {
                        if held.len() > max_nodes as usize {
                            return Err(too_large(max_nodes));
                        }
                        next.push(far);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort();
        frontier = next;
    }
    // Each link once, by relation and ends: a link walked both ways is
    // two arcs and one weight.
    let by_link: BTreeMap<(&RelationName, &Node, &Node), &Edge> = (arcs.values())
        .map(|(_, e)| ((&e.relation, &e.from, &e.to), e))
        .collect();
    let mut costs = BTreeMap::new();
    for (link, edge) in by_link {
        costs.insert(link, weight_of(edge, weight)?);
    }
    let nodes: Vec<&Node> = held.iter().collect();
    let id = |n: &Node| nodes.binary_search(&n).expect("every arc's ends are held");
    let Ok(target) = nodes.binary_search(&to) else {
        return Ok(None);
    };
    // Each arc as (near, far, deepest hop, weight), in the arcs' order.
    let plain: Vec<(usize, usize, u32, i128)> = (arcs.iter())
        .map(|((near, _, far, _, _), (upto, e))| {
            (
                id(near),
                id(far),
                *upto,
                costs[&(&e.relation, &e.from, &e.to)],
            )
        })
        .collect();
    // `layers[k][v]`: the least walk of exactly `k` hops to `v`, by its
    // cost and then its last arc, whose order is the record it came from
    // and then the link.
    let mut layers: Vec<Vec<Option<(i128, usize)>>> = vec![vec![None; nodes.len()]];
    layers[0][id(from)] = Some((0, usize::MAX));
    let mut best: Option<(i128, usize)> = None;
    for k in 1..=deepest {
        let mut next: Vec<Option<(i128, usize)>> = vec![None; nodes.len()];
        for (a, &(u, v, upto, w)) in plain.iter().enumerate() {
            let Some((cost, _)) = layers[(k - 1) as usize][u] else {
                continue;
            };
            if upto < k {
                continue;
            }
            let cost = cost.checked_add(w).ok_or(Error::invalid(Case::Overflow))?;
            if next[v].is_none_or(|b| (cost, a) < b) {
                next[v] = Some((cost, a));
            }
        }
        let reached = next.iter().any(Option::is_some);
        if let Some((cost, _)) = next[target]
            && best.is_none_or(|(b, _)| cost < b)
        {
            best = Some((cost, k as usize));
        }
        layers.push(next);
        if !reached {
            break;
        }
    }
    let Some((_, hops)) = best else {
        return Ok(None);
    };
    let arcs: Vec<&Edge> = arcs.values().map(|(_, e)| e).collect();
    let mut walk = Cheap {
        nodes: vec![to.clone()],
        edges: Vec::new(),
        costs: Vec::new(),
    };
    let mut at = target;
    for k in (1..=hops).rev() {
        let (cost, a) = layers[k][at].expect("the walk is unwound along its own arcs");
        walk.costs.push(cost);
        walk.edges.push(arcs[a].clone());
        at = plain[a].0;
        walk.nodes.push(nodes[at].clone());
    }
    walk.costs.push(0);
    walk.nodes.reverse();
    walk.edges.reverse();
    walk.costs.reverse();
    Ok(Some(walk))
}

/// A link's weight in units of 10^-9: refused `weight_missing` where it
/// is null or absent and `weight_negative` below 0.
fn weight_of(edge: &Edge, field: &FieldName) -> Result<i128> {
    let (relation, from, to) = (
        edge.relation.clone(),
        edge.from.1.clone(),
        edge.to.1.clone(),
    );
    let text = match edge.fields.get(field) {
        None | Some(Value::Null) => {
            return Err(Error::invalid(Case::WeightMissing { relation, from, to }));
        }
        Some(Value::Number(n)) if n.is_f64() => n.as_f64().map(crate::canonical::number),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => None,
    };
    let Some(text) = text else {
        let field = field.clone();
        return Err(Error::invalid(Case::WeightNotNumber { relation, field }));
    };
    let units = units(&text).map_err(|why| match why {
        Some(case) => Error::invalid(case),
        None => Error::invalid(Case::WeightNotNumber {
            relation: relation.clone(),
            field: field.clone(),
        }),
    })?;
    if units < 0 {
        return Err(Error::invalid(Case::WeightNegative { relation, from, to }));
    }
    Ok(units)
}

/// A decimal written `-?digits(.digits)?(e[+-]?digits)?` in units of
/// 10^-9, rounded half away from zero; `Err(None)` for text that is no
/// such decimal, `Err(Some(overflow))` past `i128`.
pub fn units(text: &str) -> std::result::Result<i128, Option<Case>> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((m, e)) => (
            m,
            e.strip_prefix('+')
                .unwrap_or(e)
                .parse::<i64>()
                .map_err(|_| None)?,
        ),
        None => (rest, 0),
    };
    let (whole, part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digit = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digit(whole) || !digit(part) || mantissa.ends_with('.') {
        return Err(None);
    }
    let digits: Vec<u8> = whole
        .bytes()
        .chain(part.bytes())
        .map(|b| b - b'0')
        .collect();
    // Digits up to the units' place are kept; the first dropped rounds.
    let kept = (whole.len() as i64)
        .saturating_add(exponent)
        .saturating_add(i64::from(WEIGHT_PLACES));
    let at = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| digits.get(i).copied())
            .unwrap_or(0)
    };
    let mut n: i128 = 0;
    for i in 0..kept.max(0) {
        let d = i128::from(at(i));
        n = (n.checked_mul(10).and_then(|n| n.checked_add(d))).ok_or(Some(Case::Overflow))?;
        if n == 0 && i >= digits.len() as i64 {
            // Only zeros remain, and zero is zero at any place.
            break;
        }
    }
    if at(kept) >= 5 {
        n = n.checked_add(1).ok_or(Some(Case::Overflow))?;
    }
    Ok(if negative { -n } else { n })
}

/// A cost in units of 10^-9 as a decimal written in its fewest digits.
pub fn cost_text(units: i128) -> String {
    let scale = 10i128.pow(WEIGHT_PLACES);
    let sign = if units < 0 { "-" } else { "" };
    let (whole, part) = (
        units.unsigned_abs() / scale as u128,
        units.unsigned_abs() % scale as u128,
    );
    if part == 0 {
        return format!("{sign}{whole}");
    }
    let part = format!("{part:09}");
    format!("{sign}{whole}.{}", part.trim_end_matches('0'))
}

#[cfg(test)]
mod tests;
