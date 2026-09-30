//! One step of a pattern, walked from one record, and a chain of steps.
//!
//! A step walks one relation in one direction for `min..=max` hops. It
//! reports every record it reaches whose depth (the fewest hops that
//! reach it) falls in that range. The start is reported at depth 0 when
//! `min` is 0; otherwise it is reported only if the walk comes back to
//! it through a cycle, at the depth it comes back. `neighbors` is the
//! one-hop step; `traverse` chains steps, each walked from every record
//! the previous one kept.

use std::collections::{BTreeMap, BTreeSet};
use std::slice;

use lakelands_api::query::{Depth, Step, Walk};
use lakelands_api::{RelationName, Result};

use super::{EdgeSource, Links, Node, check_depth};

/// A record a walk reached, and how many hops it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reached {
    /// The record.
    pub node: Node,
    /// The fewest hops that reach it from the walk's start.
    pub depth: u32,
}

/// What a chain of steps found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Traversal {
    /// One binding per match: the record each step reached, in step
    /// order. A chain of no steps has one empty binding, the start alone.
    pub bindings: Vec<Vec<Reached>>,
    /// Whether the cap on bindings cut the answer short.
    pub truncated: bool,
}

/// The records one hop away from `from` over one relation.
///
/// Ordered by `(type, key)`; every record carries depth 1.
pub fn neighbors<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    relation: &RelationName,
    walk: Walk,
) -> Result<Vec<Reached>> {
    let one = Depth { min: 1, max: 1 };
    walk_step(&mut Links::new(source), from, relation, walk, one)
}

/// The records one step reaches from `from`, walking `step.relation` in
/// the direction `step.walk` for `step.depth` hops.
///
/// Ordered by `(depth, type, key)`. Refused as invalid when the depth
/// range is empty or goes past `max_depth`, the lake's ceiling. The
/// step's `type` and `where` are not applied here: the type is a
/// declaration the planner checks, and the filter is the caller's.
pub fn step<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    step: &Step,
    max_depth: u32,
) -> Result<Vec<Reached>> {
    check_depth(step.depth, max_depth)?;
    walk_step(
        &mut Links::new(source),
        from,
        &step.relation,
        step.walk,
        step.depth,
    )
}

/// Walk a chain of steps from `from`: each step is walked from every
/// record the previous one kept.
///
/// `keep(i, node)` decides whether a record the `i`-th step reached is
/// kept; that is where the caller applies the step's `where` and its
/// own visibility rules. A record a step drops is not walked from.
///
/// Bindings come depth-first over the steps, each step's records in
/// `(depth, type, key)` order, so the answer is ordered by its first
/// step's records, then its second's, and so on. At most `max_bindings`
/// are returned; `truncated` says whether more were found. Refused as
/// invalid, before any walking, when any step's depth range is empty or
/// goes past `max_depth`.
pub fn traverse<S, K>(
    source: &S,
    from: &Node,
    steps: &[Step],
    max_depth: u32,
    max_bindings: usize,
    keep: K,
) -> Result<Traversal>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    let mut chain = Chain {
        links: Links::new(source),
        steps,
        keep,
        memo: BTreeMap::new(),
        max_bindings,
        out: Traversal {
            bindings: Vec::new(),
            truncated: false,
        },
    };
    chain.extend(0, from, &mut Vec::new())?;
    Ok(chain.out)
}

/// The state of one [`traverse`].
struct Chain<'a, S: EdgeSource + ?Sized, K> {
    links: Links<'a, S>,
    steps: &'a [Step],
    keep: K,
    /// What step `i` kept from a record, so a record reached along
    /// several bindings is walked from once.
    memo: BTreeMap<(usize, Node), Vec<Reached>>,
    max_bindings: usize,
    out: Traversal,
}

impl<S, K> Chain<'_, S, K>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    /// Extend `prefix` (a binding of steps `0..i`, ending at `at`) with
    /// every record step `i` keeps. Returns false once the cap is hit.
    fn extend(&mut self, i: usize, at: &Node, prefix: &mut Vec<Reached>) -> Result<bool> {
        if i == self.steps.len() {
            if self.out.bindings.len() >= self.max_bindings {
                self.out.truncated = true;
                return Ok(false);
            }
            self.out.bindings.push(prefix.clone());
            return Ok(true);
        }
        for reached in self.kept(i, at)? {
            let next = reached.node.clone();
            prefix.push(reached);
            let more = self.extend(i + 1, &next, prefix)?;
            prefix.pop();
            if !more {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn kept(&mut self, i: usize, at: &Node) -> Result<Vec<Reached>> {
        let memo_key = (i, at.clone());
        if let Some(known) = self.memo.get(&memo_key) {
            return Ok(known.clone());
        }
        let s = &self.steps[i];
        let mut kept = Vec::new();
        for reached in walk_step(&mut self.links, at, &s.relation, s.walk, s.depth)? {
            if (self.keep)(i, &reached.node)? {
                kept.push(reached);
            }
        }
        self.memo.insert(memo_key, kept.clone());
        Ok(kept)
    }
}

/// Breadth-first over one relation, one level at a time, each level's
/// links read together. Each record is recorded at the first level that
/// reaches it, so its depth is the fewest hops; each level is a sorted
/// set, so nothing depends on the order the source answers in.
fn walk_step<S: EdgeSource + ?Sized>(
    links: &mut Links<'_, S>,
    from: &Node,
    relation: &RelationName,
    walk: Walk,
    depth: Depth,
) -> Result<Vec<Reached>> {
    // The start is not marked reached: a cycle back to it is reported at
    // the depth it closes, which is what a range starting at 1 asks for.
    let mut best: BTreeMap<Node, u32> = BTreeMap::new();
    let mut frontier: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    for hop in 1..=depth.max {
        let nears: Vec<Node> = frontier.iter().cloned().collect();
        let mut next = BTreeSet::new();
        for around in links.around(&nears, slice::from_ref(relation), walk)? {
            for (other, _) in around {
                if !best.contains_key(&other) {
                    best.insert(other.clone(), hop);
                    next.insert(other);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    if depth.min == 0 {
        best.insert(from.clone(), 0);
    }
    let mut out: Vec<Reached> = best
        .into_iter()
        .filter(|(_, d)| *d >= depth.min)
        .map(|(node, depth)| Reached { node, depth })
        .collect();
    out.sort_by(|a, b| (a.depth, &a.node).cmp(&(b.depth, &b.node)));
    Ok(out)
}

#[cfg(test)]
mod tests;
