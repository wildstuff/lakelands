//! Whole-graph measures: degree centrality and connected components.
//!
//! Both are walks over the same [`EdgeSource`] every other walk reads, so
//! they count what a walk would reach: a link to a record that is missing,
//! retired or hidden from the caller is not there
//! (`docs/decisions/026-the-store-walks-the-graph.md`).
//!
//! - [`degree`] counts one record's links. It asks about that record
//!   alone, so a caller measuring many records holds one record's links at
//!   a time, not the graph.
//! - [`Components`] grows the weak components of the records it is given,
//!   links taken both ways. It holds every record it reaches, so it is
//!   bounded: past `max_nodes` records it refuses `too_large` rather than
//!   answer a partition of part of the graph, which would look whole.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::query::Walk;
use lakelands_api::{Error, Refusal, RelationName, Result};

use super::{EdgeId, EdgeSource, Links, Node, edge_id};

/// The number of distinct links of `node` over the given relations, each
/// walked in its own direction, whose far end may appear. A link named by
/// two of the pairs counts once.
pub fn degree<S: EdgeSource + ?Sized>(
    source: &S,
    node: &Node,
    steps: &[(RelationName, Walk)],
) -> Result<u64> {
    let mut links = Links::new(source);
    let mut seen: BTreeSet<EdgeId> = BTreeSet::new();
    for (relation, walk) in steps {
        for (_, edge) in links.around(node, std::slice::from_ref(relation), *walk)? {
            seen.insert(edge_id(&edge));
        }
    }
    Ok(seen.len() as u64)
}

/// One weak component: its records, ordered by `(type, key)`.
pub type Component = Vec<Node>;

/// The weak components of the records added to it, grown over the given
/// relations with every link taken both ways.
pub struct Components<'a, S: EdgeSource + ?Sized> {
    links: Links<'a, S>,
    relations: Vec<RelationName>,
    max_nodes: u32,
    /// Every record reached, with the component it belongs to.
    member_of: BTreeMap<Node, usize>,
    found: Vec<BTreeSet<Node>>,
}

impl<'a, S: EdgeSource + ?Sized> Components<'a, S> {
    /// An empty set of components over `relations`, holding at most
    /// `max_nodes` records.
    pub fn new(source: &'a S, relations: Vec<RelationName>, max_nodes: u32) -> Self {
        Components {
            links: Links::new(source),
            relations,
            max_nodes,
            member_of: BTreeMap::new(),
            found: Vec::new(),
        }
    }

    /// Adds a record and everything linked to it. A record already reached
    /// adds nothing. Refused `too_large` as soon as more than `max_nodes`
    /// records are held.
    pub fn add(&mut self, seed: Node) -> Result<()> {
        if self.member_of.contains_key(&seed) {
            return Ok(());
        }
        let index = self.found.len();
        let mut component = BTreeSet::new();
        let mut frontier = vec![seed];
        while let Some(near) = frontier.pop() {
            if self.member_of.contains_key(&near) {
                continue;
            }
            self.member_of.insert(near.clone(), index);
            if self.member_of.len() > self.max_nodes as usize {
                return Err(Error::Refused {
                    field: None,
                    reason: Refusal::TooLarge {
                        max: self.max_nodes,
                    },
                });
            }
            for (other, _) in self.links.around(&near, &self.relations, Walk::Both)? {
                if !self.member_of.contains_key(&other) {
                    frontier.push(other);
                }
            }
            component.insert(near);
        }
        self.found.push(component);
        Ok(())
    }

    /// The components, largest first, then by their least `(type, key)`;
    /// each one's records by `(type, key)`.
    pub fn finish(self) -> Vec<Component> {
        let mut all: Vec<Component> = self
            .found
            .into_iter()
            .map(|c| c.into_iter().collect())
            .collect();
        all.sort_by(|a, b| {
            b.len()
                .cmp(&a.len())
                .then_with(|| a.first().cmp(&b.first()))
        });
        all
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{Graph, e, keys, link, n};
    use super::*;
    use lakelands_api::{RecordKey, TypeName};

    fn rel(r: &str) -> RelationName {
        RelationName::new(r)
    }

    fn components(
        g: &Graph,
        seeds: &[&str],
        relations: &[&str],
        max: u32,
    ) -> Result<Vec<Vec<String>>> {
        let mut c = Components::new(g, relations.iter().map(|r| rel(r)).collect(), max);
        for s in seeds {
            c.add(n(s))?;
        }
        Ok(c.finish()
            .iter()
            .map(|c| keys(c).into_iter().map(str::to_owned).collect())
            .collect())
    }

    #[test]
    fn degree_counts_links_in_the_named_direction_to_visible_ends() {
        let g = Graph::new(vec![
            e("a", "r", "b"),
            e("a", "r", "c"),
            e("d", "r", "a"),
            e("a", "s", "b"),
            e("a", "r", "gone"),
        ])
        .retire("gone");
        let d = |steps: &[(&str, Walk)]| {
            let steps: Vec<_> = steps.iter().map(|(r, w)| (rel(r), *w)).collect();
            degree(&g, &n("a"), &steps).unwrap()
        };
        assert_eq!(d(&[("r", Walk::Out)]), 2, "the retired end is not counted");
        assert_eq!(d(&[("r", Walk::In)]), 1);
        assert_eq!(d(&[("r", Walk::Both)]), 3);
        assert_eq!(d(&[("r", Walk::Both), ("s", Walk::Out)]), 4);
        assert_eq!(
            d(&[("r", Walk::Out), ("r", Walk::Both)]),
            3,
            "a link named twice counts once"
        );
    }

    #[test]
    fn components_partition_what_the_seeds_reach_largest_first() {
        let g = Graph::new(vec![
            e("a", "r", "b"),
            e("c", "r", "b"),
            e("x", "r", "y"),
            e("p", "s", "q"),
        ]);
        assert_eq!(
            components(&g, &["y", "a", "p", "lone"], &["r"], 100).unwrap(),
            vec![vec!["a", "b", "c"], vec!["x", "y"], vec!["lone"], vec!["p"],],
            "over `r` only, `p` stands alone; ties by the least member"
        );
    }

    #[test]
    fn a_component_spans_types() {
        let partner = (TypeName::new("partner"), RecordKey::new("P"));
        let g = Graph::new(vec![
            link(n("i1"), "billed", partner.clone()),
            link(n("i2"), "billed", partner.clone()),
        ]);
        let mut c = Components::new(&g, vec![rel("billed")], 10);
        c.add(n("i1")).unwrap();
        c.add(n("i2")).unwrap();
        let all = c.finish();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], vec![n("i1"), n("i2"), partner]);
    }

    #[test]
    fn past_the_cap_a_measure_is_refused_not_cut() {
        let g = Graph::new(vec![e("a", "r", "b"), e("b", "r", "c")]);
        assert!(components(&g, &["a"], &["r"], 3).is_ok());
        match components(&g, &["a"], &["r"], 2) {
            Err(Error::Refused {
                reason: Refusal::TooLarge { max: 2 },
                ..
            }) => {}
            other => panic!("expected too_large, got {other:?}"),
        }
    }
}
