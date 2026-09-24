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

use lakelands_api::query::{Edge, Walk};
use lakelands_api::{RecordKey, RelationName, Result, TypeName};

use super::{EdgeSource, Node};

pub(super) struct Graph {
    edges: Vec<Edge>,
    present: BTreeSet<Node>,
    retired: BTreeSet<Node>,
    pub(super) visibility_asked: Cell<usize>,
}

/// A record of type `n`.
pub(super) fn n(key: &str) -> Node {
    (TypeName::new("n"), RecordKey::new(key))
}

/// A link of type `n` to type `n`.
pub(super) fn e(from: &str, relation: &str, to: &str) -> Edge {
    link(n(from), relation, n(to))
}

/// A link between any two records.
pub(super) fn link(from: Node, relation: &str, to: Node) -> Edge {
    Edge {
        from,
        relation: RelationName::new(relation),
        to,
        fields: BTreeMap::new(),
    }
}

/// The keys of a list of records.
pub(super) fn keys<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<&'a str> {
    nodes.into_iter().map(|n| n.1.as_str()).collect()
}

impl Graph {
    pub(super) fn new(edges: Vec<Edge>) -> Self {
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
        }
    }

    /// A chain `a → b → c → d` over `next`.
    pub(super) fn chain() -> Self {
        Graph::new(vec![
            e("a", "next", "b"),
            e("b", "next", "c"),
            e("c", "next", "d"),
        ])
    }

    pub(super) fn retire(mut self, key: &str) -> Self {
        self.retired.insert(n(key));
        self
    }

    pub(super) fn missing(mut self, key: &str) -> Self {
        self.present.remove(&n(key));
        self
    }
}

impl EdgeSource for Graph {
    fn edges(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Edge>> {
        let at = |side: &Node| side.0 == *ty && side.1 == *key;
        Ok(self
            .edges
            .iter()
            .rev()
            .filter(|edge| relation.is_none_or(|r| edge.relation == *r))
            .filter(|edge| match walk {
                Walk::Out => at(&edge.from),
                Walk::In => at(&edge.to),
                Walk::Both => at(&edge.from) || at(&edge.to),
            })
            .cloned()
            .collect())
    }

    fn is_visible(&self, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.visibility_asked.set(self.visibility_asked.get() + 1);
        let node = (ty.clone(), key.clone());
        Ok(self.present.contains(&node) && !self.retired.contains(&node))
    }
}
