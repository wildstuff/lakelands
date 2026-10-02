//! Whole-graph measures: degree centrality and connected components.
//!
//! Both are walks over the same [`EdgeSource`] every other walk reads, so
//! they count what a walk would reach: a link to a record that is missing,
//! retired or hidden from the caller is not there
//! (`docs/decisions/026-the-store-walks-the-graph.md`).
//!
//! - [`degrees`] counts the links of the records it is handed, read
//!   together. A caller measuring a whole type hands it a page at a time,
//!   so it holds a page's links, not the graph.
//! - [`Components`] grows the weak components of the records it is given,
//!   links taken both ways. It holds every record it reaches, so it is
//!   bounded: past `max_nodes` records it refuses `too_large` rather than
//!   answer a partition of part of the graph, which would look whole.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::query::Walk;
use lakelands_api::{Error, Refusal, RelationName, Result};

use super::{EdgeId, EdgeSource, Links, Node, PAGE, edge_id};

/// For each of `nodes`, in their order, the number of its distinct links
/// over the given relations, each walked in its own direction, whose far
/// end may appear. A link named by two of the pairs counts once. The
/// links of all of them are read together, and each far end is asked
/// about once for all of them; a caller hands it a page at a time.
pub fn degrees<S: EdgeSource + ?Sized>(
    source: &S,
    nodes: &[Node],
    steps: &[(RelationName, Walk)],
) -> Result<Vec<u64>> {
    let mut links = Links::new(source);
    let mut seen: Vec<BTreeSet<EdgeId>> = vec![BTreeSet::new(); nodes.len()];
    for (relation, walk) in steps {
        let around = links.around(nodes, std::slice::from_ref(relation), *walk)?;
        for (seen, around) in seen.iter_mut().zip(around) {
            seen.extend(around.iter().map(|(_, edge)| edge_id(edge)));
        }
    }
    Ok(seen.iter().map(|s| s.len() as u64).collect())
}

/// One weak component: its records, ordered by `(type, key)`.
pub type Component = Vec<Node>;

/// The weak components of the records added to it, grown over the given
/// relations with every link taken both ways.
///
/// What it holds is bounded by `max_nodes` for the records reached, plus
/// one entry per hidden far end of those records (retired, missing or not
/// visible to the caller), each asked about once: a record is reached when
/// it is first seen, so the frontier holds each record once and only
/// records already counted against the cap; visibility is asked only of a
/// record not yet reached, and only a hidden one is remembered; and the
/// records reached are the one map, grouped into components when the walk
/// is done.
pub struct Components<'a, S: EdgeSource + ?Sized> {
    source: &'a S,
    relations: Vec<RelationName>,
    max_nodes: u32,
    /// Every record reached, with the component it belongs to.
    member_of: BTreeMap<Node, usize>,
    /// Far ends asked about that may not appear.
    hidden: BTreeSet<Node>,
    /// How many components have been started.
    started: usize,
}

impl<'a, S: EdgeSource + ?Sized> Components<'a, S> {
    /// An empty set of components over `relations`, holding at most
    /// `max_nodes` records.
    pub fn new(source: &'a S, relations: Vec<RelationName>, max_nodes: u32) -> Self {
        Components {
            source,
            relations,
            max_nodes,
            member_of: BTreeMap::new(),
            hidden: BTreeSet::new(),
            started: 0,
        }
    }

    /// Counts `node` as reached in component `index`; refused `too_large`
    /// once more than `max_nodes` records are reached.
    fn reach(&mut self, node: Node, index: usize) -> Result<()> {
        self.member_of.insert(node, index);
        if self.member_of.len() > self.max_nodes as usize {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::TooLarge {
                    max: self.max_nodes,
                },
            });
        }
        Ok(())
    }

    /// Adds a record and everything linked to it, reading the links of a
    /// page of the frontier at a time. A record already reached adds
    /// nothing. Refused `too_large` as soon as more than `max_nodes`
    /// records are reached.
    pub fn add(&mut self, seed: Node) -> Result<()> {
        if self.member_of.contains_key(&seed) {
            return Ok(());
        }
        let index = self.started;
        self.started += 1;
        self.reach(seed.clone(), index)?;
        let mut frontier = vec![seed];
        while !frontier.is_empty() {
            let nears = frontier.split_off(frontier.len().saturating_sub(PAGE));
            let mut found: BTreeSet<Node> = BTreeSet::new();
            for relation in &self.relations {
                let edges = super::edges_of(self.source, &nears, Some(relation), Walk::Both)?;
                for (near, edges) in nears.iter().zip(edges) {
                    for edge in edges {
                        let other = super::far(&edge, near);
                        if !self.member_of.contains_key(&other) && !self.hidden.contains(&other) {
                            found.insert(other);
                        }
                    }
                }
            }
            let found: Vec<Node> = found.into_iter().collect();
            for page in found.chunks(PAGE) {
                for (other, visible) in page.iter().zip(self.source.visible(page)?) {
                    if visible {
                        self.reach(other.clone(), index)?;
                        frontier.push(other.clone());
                    } else {
                        self.hidden.insert(other.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// The components, largest first, then by their least `(type, key)`;
    /// each one's records by `(type, key)`.
    pub fn finish(self) -> Vec<Component> {
        let mut all: Vec<Component> = vec![Vec::new(); self.started];
        // The map is ordered by `(type, key)`, so each component's records
        // come out in that order.
        for (node, index) in self.member_of {
            all[index].push(node);
        }
        all.sort_by(|a, b| (b.len(), a.first()).cmp(&(a.len(), b.first())));
        all
    }
}

#[cfg(test)]
mod tests;
