//! A small in-memory [`EdgeSource`] for the walks' tests, so they check
//! the walks rather than any engine.
//!
//! It answers `edges` in the reverse of the order the links were given,
//! so a walk that relied on the source's order instead of sorting would
//! show it. A record that was never mentioned by a link, or was removed
//! with [`Graph::missing`], does not exist; one marked with
//! [`Graph::retire`] exists and is hidden.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::query::{Depth, Edge, Step, Walk};
use lakelands_api::{RecordKey, RelationName, Result, TypeName};

use super::{EdgeSource, Node};

pub(in crate::graph) struct Graph {
    edges: Vec<Edge>,
    present: BTreeSet<Node>,
    retired: BTreeSet<Node>,
    /// Records asked about, one by one.
    pub(in crate::graph) visibility_asked: Cell<usize>,
    /// Calls of `edges`, and of `visible`.
    pub(in crate::graph) edges_asked: Cell<usize>,
    pub(in crate::graph) visible_asked: Cell<usize>,
}

/// A record of type `n`.
pub(in crate::graph) fn n(key: &str) -> Node {
    (TypeName::new("n"), RecordKey::new(key))
}

/// A link of type `n` to type `n`.
pub(in crate::graph) fn e(from: &str, relation: &str, to: &str) -> Edge {
    link(n(from), relation, n(to))
}

/// A link between any two records.
pub(in crate::graph) fn link(from: Node, relation: &str, to: Node) -> Edge {
    Edge {
        from,
        relation: RelationName::new(relation),
        to,
        fields: BTreeMap::new(),
    }
}

/// One walk over `relations`, each walked `walk` up to `max` hops.
pub(in crate::graph) fn over(relations: &[&str], walk: Walk, max: u32) -> Vec<Step> {
    let depth = Depth { min: 0, max };
    let step = |r: &&str| Step {
        relation: RelationName::new(*r),
        walk,
        ty: None,
        filter: None,
        depth,
        named: None,
    };
    relations.iter().map(step).collect()
}

/// A `keep` that keeps every record.
pub(in crate::graph) fn every(_: usize, _: &Node) -> Result<bool> {
    Ok(true)
}

/// The keys of a list of records.
pub(in crate::graph) fn keys<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<&'a str> {
    nodes.into_iter().map(|n| n.1.as_str()).collect()
}

impl Graph {
    pub(in crate::graph) fn new(edges: Vec<Edge>) -> Self {
        let mut present = BTreeSet::new();
        for edge in &edges {
            present.insert(edge.from.clone());
            present.insert(edge.to.clone());
        }
        Graph {
            edges,
            present,
            retired: BTreeSet::new(),
            visibility_asked: Cell::new(0),
            edges_asked: Cell::new(0),
            visible_asked: Cell::new(0),
        }
    }

    /// A chain `a → b → c → d` over `next`.
    pub(in crate::graph) fn chain() -> Self {
        Graph::new(vec![
            e("a", "next", "b"),
            e("b", "next", "c"),
            e("c", "next", "d"),
        ])
    }

    pub(in crate::graph) fn retire(mut self, key: &str) -> Self {
        self.retired.insert(n(key));
        self
    }

    pub(in crate::graph) fn missing(mut self, key: &str) -> Self {
        self.present.remove(&n(key));
        self
    }
}

impl EdgeSource for Graph {
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
        _step: Option<usize>,
    ) -> Result<Vec<Vec<Edge>>> {
        self.edges_asked.set(self.edges_asked.get() + 1);
        Ok(keys
            .iter()
            .map(|key| {
                let at = |side: &Node| side.0 == *ty && side.1 == *key;
                self.edges
                    .iter()
                    .rev()
                    .filter(|edge| relation.is_none_or(|r| edge.relation == *r))
                    .filter(|edge| match walk {
                        Walk::Out => at(&edge.from),
                        Walk::In => at(&edge.to),
                        Walk::Both => at(&edge.from) || at(&edge.to),
                    })
                    .cloned()
                    .collect()
            })
            .collect())
    }

    fn visible(&self, nodes: &[Node]) -> Result<Vec<bool>> {
        self.visible_asked.set(self.visible_asked.get() + 1);
        self.visibility_asked
            .set(self.visibility_asked.get() + nodes.len());
        Ok(nodes
            .iter()
            .map(|node| self.present.contains(node) && !self.retired.contains(node))
            .collect())
    }
}
