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

/// Breadth-first over one relation, one level at a time. Each record is
/// recorded at the first level that reaches it, so its depth is the
/// fewest hops; each level is a sorted set, so nothing depends on the
/// order the source answers in.
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
        let mut next = BTreeSet::new();
        for near in &frontier {
            for (other, _) in links.around(near, slice::from_ref(relation), walk)? {
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
mod tests {
    use lakelands_api::{Error, RecordKey, Refusal, TypeName};

    use super::super::fixture::{Graph, e, keys, link, n};
    use super::*;

    fn next(walk: Walk, min: u32, max: u32) -> Step {
        Step {
            relation: RelationName::new("next"),
            walk,
            ty: None,
            filter: None,
            depth: Depth { min, max },
        }
    }

    fn nodes(reached: &[Reached]) -> Vec<&str> {
        keys(reached.iter().map(|r| &r.node))
    }

    fn depths(reached: &[Reached]) -> Vec<u32> {
        reached.iter().map(|r| r.depth).collect()
    }

    fn all(_: usize, _: &Node) -> Result<bool> {
        Ok(true)
    }

    #[test]
    fn a_link_to_a_missing_or_retired_record_is_not_followed() {
        let g = Graph::new(vec![
            e("a", "next", "gone"),
            e("a", "next", "hidden"),
            e("a", "next", "ok"),
        ])
        .missing("gone")
        .retire("hidden");
        let got = neighbors(&g, &n("a"), &RelationName::new("next"), Walk::Out).unwrap();
        assert_eq!(nodes(&got), vec!["ok"]);
    }

    #[test]
    fn neighbors_are_ordered_by_type_then_key_whatever_order_the_source_answers_in() {
        let g = Graph::new(vec![
            e("a", "r", "c"),
            link(n("a"), "r", (TypeName::new("m"), RecordKey::new("z"))),
            e("a", "r", "b"),
        ]);
        let got = neighbors(&g, &n("a"), &RelationName::new("r"), Walk::Out).unwrap();
        assert_eq!(nodes(&got), vec!["z", "b", "c"]);
        assert_eq!(depths(&got), vec![1, 1, 1]);
    }

    #[test]
    fn walking_out_follows_links_forward_and_walking_in_follows_them_back() {
        let g = Graph::chain();
        let out = step(&g, &n("a"), &next(Walk::Out, 1, 10), 16).unwrap();
        assert_eq!(nodes(&out), vec!["b", "c", "d"]);
        let back = step(&g, &n("d"), &next(Walk::In, 1, 10), 16).unwrap();
        assert_eq!(nodes(&back), vec!["c", "b", "a"]);
        assert_eq!(depths(&back), vec![1, 2, 3]);
    }

    #[test]
    fn walking_both_ways_reaches_either_side() {
        let g = Graph::chain();
        let got = step(&g, &n("b"), &next(Walk::Both, 1, 1), 16).unwrap();
        assert_eq!(nodes(&got), vec!["a", "c"]);
    }

    #[test]
    fn a_record_reachable_twice_reports_its_shortest_depth() {
        let g = Graph::new(vec![
            e("a", "next", "b"),
            e("b", "next", "c"),
            e("a", "next", "c"),
        ]);
        let got = step(&g, &n("a"), &next(Walk::Out, 1, 5), 16).unwrap();
        assert_eq!(nodes(&got), vec!["b", "c"]);
        assert_eq!(depths(&got), vec![1, 1]);
    }

    #[test]
    fn a_cycle_terminates_and_reports_the_start_where_it_closes() {
        let g = Graph::new(vec![e("a", "next", "b"), e("b", "next", "a")]);
        let got = step(&g, &n("a"), &next(Walk::Out, 1, 3), 16).unwrap();
        assert_eq!(nodes(&got), vec!["b", "a"]);
        assert_eq!(depths(&got), vec![1, 2]);
    }

    #[test]
    fn a_step_deeper_than_the_ceiling_is_refused_not_clamped() {
        let err = step(&Graph::chain(), &n("a"), &next(Walk::Out, 1, u32::MAX), 16).unwrap_err();
        assert!(matches!(
            err,
            Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_zero_depth_step_yields_the_start_alone_never_the_whole_graph() {
        let got = step(&Graph::chain(), &n("a"), &next(Walk::Out, 0, 0), 16).unwrap();
        assert_eq!(nodes(&got), vec!["a"]);
        assert_eq!(depths(&got), vec![0]);
    }

    #[test]
    fn a_range_from_zero_includes_the_start_once_even_on_a_cycle() {
        let g = Graph::new(vec![e("a", "next", "b"), e("b", "next", "a")]);
        let got = step(&g, &n("a"), &next(Walk::Out, 0, 3), 16).unwrap();
        assert_eq!(nodes(&got), vec!["a", "b"]);
        assert_eq!(depths(&got), vec![0, 1]);
    }

    #[test]
    fn the_minimum_depth_drops_records_reached_sooner() {
        let got = step(&Graph::chain(), &n("a"), &next(Walk::Out, 2, 3), 16).unwrap();
        assert_eq!(nodes(&got), vec!["c", "d"]);
    }

    #[test]
    fn a_chain_of_steps_yields_one_binding_per_match() {
        let g = Graph::chain();
        let steps = vec![next(Walk::Out, 1, 1); 3];
        let got = traverse(&g, &n("a"), &steps, 16, 100, all).unwrap();
        assert_eq!(got.bindings.len(), 1);
        assert_eq!(nodes(&got.bindings[0]), vec!["b", "c", "d"]);
        assert!(!got.truncated);

        let got = traverse(&g, &n("a"), &steps[..1], 16, 100, all).unwrap();
        assert_eq!(nodes(&got.bindings[0]), vec!["b"]);
    }

    #[test]
    fn a_chain_past_the_end_of_the_links_yields_nothing() {
        let steps = vec![next(Walk::Out, 1, 1); 4];
        let got = traverse(&Graph::chain(), &n("a"), &steps, 16, 100, all).unwrap();
        assert!(got.bindings.is_empty());
    }

    #[test]
    fn a_chain_along_a_missing_relation_yields_nothing() {
        let mut nope = next(Walk::Out, 1, 1);
        nope.relation = RelationName::new("nope");
        let got = traverse(&Graph::chain(), &n("a"), &[nope], 16, 100, all).unwrap();
        assert!(got.bindings.is_empty());
    }

    #[test]
    fn a_chain_with_one_step_too_deep_is_refused_before_walking() {
        let g = Graph::chain();
        let steps = vec![next(Walk::Out, 1, 1), next(Walk::Out, 1, 17)];
        assert!(traverse(&g, &n("a"), &steps, 16, 100, all).is_err());
        assert_eq!(g.visibility_asked.get(), 0);
    }

    #[test]
    fn a_chain_of_no_steps_is_the_start_alone() {
        let got = traverse(&Graph::chain(), &n("a"), &[], 16, 100, all).unwrap();
        assert_eq!(got.bindings, vec![Vec::<Reached>::new()]);
    }

    #[test]
    fn bindings_come_depth_first_in_record_order() {
        // a has children x and y; each has leaves 1 and 2.
        let g = Graph::new(vec![
            e("a", "c", "y"),
            e("a", "c", "x"),
            e("x", "c", "x2"),
            e("x", "c", "x1"),
            e("y", "c", "y1"),
        ]);
        let mut child = next(Walk::Out, 1, 1);
        child.relation = RelationName::new("c");
        let got = traverse(&g, &n("a"), &[child.clone(), child], 16, 100, all).unwrap();
        let flat: Vec<Vec<&str>> = got.bindings.iter().map(|b| nodes(b)).collect();
        assert_eq!(
            flat,
            vec![vec!["x", "x1"], vec!["x", "x2"], vec!["y", "y1"]]
        );
    }

    #[test]
    fn keep_drops_a_record_and_everything_past_it() {
        let g = Graph::new(vec![
            e("a", "next", "b"),
            e("a", "next", "c"),
            e("b", "next", "d"),
        ]);
        let steps = vec![next(Walk::Out, 1, 1); 2];
        let got = traverse(&g, &n("a"), &steps, 16, 100, |i, node| {
            Ok(!(i == 0 && node.1.as_str() == "b"))
        })
        .unwrap();
        assert!(got.bindings.is_empty(), "c has no next, and b was dropped");
    }

    #[test]
    fn the_cap_on_bindings_says_it_cut_the_answer_short() {
        let g = Graph::new(vec![
            e("a", "next", "b"),
            e("a", "next", "c"),
            e("a", "next", "d"),
        ]);
        let steps = vec![next(Walk::Out, 1, 1)];
        let got = traverse(&g, &n("a"), &steps, 16, 2, all).unwrap();
        assert_eq!(got.bindings.len(), 2);
        assert!(got.truncated);
        let exact = traverse(&g, &n("a"), &steps, 16, 3, all).unwrap();
        assert!(!exact.truncated);
    }

    #[test]
    fn visibility_is_asked_once_per_record_per_walk() {
        let g = Graph::new(vec![
            e("a", "next", "b"),
            e("a", "next", "c"),
            e("b", "next", "d"),
            e("c", "next", "d"),
        ]);
        step(&g, &n("a"), &next(Walk::Both, 1, 3), 16).unwrap();
        // a, b, c, d: each asked about once, the start included when a
        // walk both ways comes back to it.
        assert_eq!(g.visibility_asked.get(), 4);
    }
}
