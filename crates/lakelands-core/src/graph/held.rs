//! The graph a whole-graph measure holds: every record its starts reach
//! over the steps' relations, each walked in its own step's direction, and
//! the arcs between them.
//!
//! Records are indexed by `(type, key)`, the store's one order, and arcs
//! are pairs of those indices, so a measure over it reads a set of records
//! and a set of arcs: a different order of links from an engine gives it
//! the same input (`docs/decisions/026-the-store-walks-the-graph.md`). It
//! holds every record it reaches and is refused `too_large` past
//! `max_nodes`, never cut.

use std::collections::{BTreeMap, BTreeSet};
use std::slice;

use lakelands_api::query::Walk;
use lakelands_api::{RelationName, Result};

use super::{EdgeSource, Links, Node, PAGE};

/// A held graph: its records by `(type, key)`, and its arcs, `(from, to)`
/// as indices into them, sorted. An arc is a link walked from one end:
/// two relations between one pair are two arcs, a step walked `both`
/// gives a link one each way, and a link from a record to itself is an
/// arc to itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    /// The records, by `(type, key)`.
    pub nodes: Vec<Node>,
    /// The arcs, each `(from, to)` by the records' indices.
    pub arcs: Vec<(u32, u32)>,
}

/// Grows a [`Held`] graph from seeds added a page at a time, closed under
/// the steps' arcs.
pub struct Holding<'a, S: EdgeSource + ?Sized> {
    links: Links<'a, S>,
    steps: Vec<(RelationName, Walk)>,
    max_nodes: u32,
    /// Whether a seed with no link it may see is a record of the graph,
    /// or none at all and never counted.
    alone: bool,
    /// Every record held, by the order it was reached in.
    index: BTreeMap<Node, u32>,
    arcs: Vec<(u32, u32)>,
}

impl<'a, S: EdgeSource + ?Sized> Holding<'a, S> {
    /// An empty graph over `steps`, holding at most `max_nodes` records;
    /// `alone` says whether a seed that reaches nothing is held.
    pub fn new(
        source: &'a S,
        steps: Vec<(RelationName, Walk)>,
        max_nodes: u32,
        alone: bool,
    ) -> Self {
        Holding {
            links: Links::new(source),
            steps,
            max_nodes,
            alone,
            index: BTreeMap::new(),
            arcs: Vec::new(),
        }
    }

    /// The index of `node`, held now where it was not, and then walked
    /// from; refused `too_large` past `max_nodes`.
    fn hold(&mut self, node: Node, frontier: &mut Vec<Node>) -> Result<u32> {
        if let Some(&i) = self.index.get(&node) {
            return Ok(i);
        }
        let i = self.index.len() as u32;
        if i >= self.max_nodes {
            return Err(super::too_large(self.max_nodes));
        }
        self.index.insert(node.clone(), i);
        frontier.push(node);
        Ok(i)
    }

    /// Adds a page of seeds and everything their arcs reach. A seed that
    /// may not stand alone is held only where it holds a link it may see
    /// over a step's relation, either way, and so is that link's far end.
    pub fn add(&mut self, seeds: &[Node]) -> Result<()> {
        let mut frontier = Vec::new();
        if self.alone {
            for seed in seeds {
                self.hold(seed.clone(), &mut frontier)?;
            }
        } else {
            let fresh: Vec<Node> = (seeds.iter())
                .filter(|s| !self.index.contains_key(*s))
                .cloned()
                .collect();
            let mut linked: Vec<Node> = Vec::new();
            for (i, (relation, _)) in self.steps.iter().enumerate() {
                let around =
                    (self.links).around(&fresh, slice::from_ref(relation), Walk::Both, Some(i))?;
                for (seed, around) in fresh.iter().zip(around) {
                    if !around.is_empty() {
                        linked.push(seed.clone());
                        linked.extend(around.into_iter().map(|(far, _)| far));
                    }
                }
            }
            for node in linked {
                self.hold(node, &mut frontier)?;
            }
        }
        while !frontier.is_empty() {
            let nears = frontier.split_off(frontier.len().saturating_sub(PAGE));
            // Each record's links, the same link named by two steps once.
            let mut out: Vec<BTreeSet<(RelationName, Node, Node, Node)>> =
                vec![BTreeSet::new(); nears.len()];
            for (i, (relation, walk)) in self.steps.iter().enumerate() {
                let around =
                    (self.links).around(&nears, slice::from_ref(relation), *walk, Some(i))?;
                for (all, one) in out.iter_mut().zip(around) {
                    all.extend(
                        one.into_iter()
                            .map(|(far, e)| (e.relation, far, e.from, e.to)),
                    );
                }
            }
            for (near, all) in nears.iter().zip(out) {
                let from = self.index[near];
                for (_, far, _, _) in all {
                    let to = self.hold(far, &mut frontier)?;
                    self.arcs.push((from, to));
                }
            }
        }
        Ok(())
    }

    /// The graph, its records by `(type, key)` and its arcs sorted.
    pub fn finish(self) -> Held {
        let mut at = vec![0u32; self.index.len()];
        let mut nodes = Vec::with_capacity(self.index.len());
        // The map is ordered by `(type, key)`: a record's rank is its index.
        for (rank, (node, reached)) in self.index.into_iter().enumerate() {
            at[reached as usize] = rank as u32;
            nodes.push(node);
        }
        let mut arcs: Vec<(u32, u32)> = (self.arcs.into_iter())
            .map(|(u, v)| (at[u as usize], at[v as usize]))
            .collect();
        arcs.sort_unstable();
        Held { nodes, arcs }
    }
}

#[cfg(test)]
mod tests;
