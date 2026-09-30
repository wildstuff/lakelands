//! The graph walks a pattern query runs: one step's multi-hop walk,
//! a chain of steps, a bounded neighbourhood, a shortest path, and the
//! whole-graph measures (degree centrality, connected components).
//!
//! Every engine keeps a lake's links and answers one narrow question
//! about them, [`ProjectionView::edges`]: the links of one record, over
//! one relation or all, in one direction. The walks are written once,
//! here, over that question and one more (may this record appear?), so
//! every engine walks the same way and returns the same answer. An
//! engine never implements a walk of its own.
//!
//! ## The rules every walk keeps
//!
//! - **Order is part of the answer.** [`EdgeSource`] promises no order;
//!   every walk sorts what it is given. Records are ordered by
//!   `(type, key)`, reached records by `(depth, type, key)`, edges by
//!   `(from, relation, to)`. Two engines holding the same links return
//!   the same answer, byte for byte, and a caller paging or diffing
//!   results never sees a reordering as a change.
//! - **Visibility is asked of the far endpoint.** A walk follows a link
//!   only if the record at its far end may appear
//!   ([`EdgeSource::is_visible`]). Through a projection view that means
//!   the record exists at the view's position and is not retired. A
//!   restricted record is the caller's concern: the store wraps the view
//!   in an [`EdgeSource`] that also hides restricted records from a
//!   principal without the grant. The start of a walk is not checked,
//!   because the caller has already read it; only a link that leads back
//!   to it asks about it, like any other far endpoint.
//! - **Cycles terminate by construction.** Every walk keeps the set of
//!   records it has reached and never expands one twice, and every walk
//!   is bounded by a depth the caller names.
//! - **Depth is the fewest hops.** A record reachable along several
//!   routes is reported once, at the shortest distance any route
//!   reaches it.
//! - **A depth over the lake's ceiling is refused, not clamped.** Each
//!   walk is handed the lake's `max_depth`; a step that asks for more,
//!   or whose range is empty, is refused as invalid rather than quietly
//!   answered with less than it asked for.

mod measures;
mod path;
mod step;
mod subgraph;

#[cfg(test)]
mod fixture;

pub use measures::{Component, Components, degree};
pub use path::{Path, shortest_path};
pub use step::{Reached, Traversal, neighbors, step, traverse};
pub use subgraph::{Subgraph, subgraph};

use std::collections::BTreeMap;

use lakelands_api::engine::ProjectionView;
use lakelands_api::query::{Depth, Edge, Walk};
use lakelands_api::{Error, RecordKey, Refusal, RelationName, Result, TypeName};

/// A record's identity: its type and its key. Ordered by type, then key.
pub type Node = (TypeName, RecordKey);

/// The two questions the walks ask of a lake's projection.
///
/// Every [`ProjectionView`] is one: the view's `edges` answers the first
/// question and its `get` the second. The store wraps a view in its own
/// implementation when visibility depends on who is asking.
pub trait EdgeSource {
    /// The links of one record, over one relation or (with `None`) all,
    /// in one direction. No order is promised; the walks sort.
    fn edges(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Edge>>;

    /// Whether a record reached over a link may appear in an answer.
    fn is_visible(&self, ty: &TypeName, key: &RecordKey) -> Result<bool>;
}

/// Through a projection view, a record is visible when it exists at the
/// view's position and is not retired. A link to a record that does not
/// exist is not followed: an answer is made of records, and there is no
/// record to show. Restricted records stay visible here; hiding them is
/// the store's decision, made per principal.
impl<V: ProjectionView + ?Sized> EdgeSource for V {
    fn edges(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Edge>> {
        ProjectionView::edges(self, ty, key, relation, walk)
    }

    fn is_visible(&self, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        Ok(self.get(ty, key)?.is_some_and(|record| !record.retired))
    }
}

/// Refuse a depth range the lake does not allow: one whose `max` is
/// over the lake's `max_depth`, or one that is empty (`min > max`).
pub fn check_depth(depth: Depth, max_depth: u32) -> Result<()> {
    if depth.max > max_depth {
        return Err(invalid(format!(
            "a step may walk at most {max_depth} hops; this one asks for {}",
            depth.max
        )));
    }
    if depth.min > depth.max {
        return Err(invalid(format!(
            "a step's depth range {}..={} is empty",
            depth.min, depth.max
        )));
    }
    Ok(())
}

fn invalid(message: String) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid { message },
    }
}

/// The identity of a link, for deduplication and ordering:
/// `(from, relation, to)`.
type EdgeId = (Node, RelationName, Node);

fn edge_id(edge: &Edge) -> EdgeId {
    (edge.from.clone(), edge.relation.clone(), edge.to.clone())
}

/// The record at the other end of a link from `near`. A link from a
/// record to itself leads back to it.
fn far(edge: &Edge, near: &Node) -> Node {
    if edge.from == *near {
        edge.to.clone()
    } else {
        edge.from.clone()
    }
}

/// The walks' view of an [`EdgeSource`]: links sorted and deduplicated,
/// filtered to visible far endpoints, with each record's visibility asked
/// once per walk.
struct Links<'a, S: EdgeSource + ?Sized> {
    source: &'a S,
    visible: BTreeMap<Node, bool>,
}

impl<'a, S: EdgeSource + ?Sized> Links<'a, S> {
    fn new(source: &'a S) -> Self {
        Links {
            source,
            visible: BTreeMap::new(),
        }
    }

    fn visible(&mut self, node: &Node) -> Result<bool> {
        if let Some(&known) = self.visible.get(node) {
            return Ok(known);
        }
        let answer = self.source.is_visible(&node.0, &node.1)?;
        self.visible.insert(node.clone(), answer);
        Ok(answer)
    }

    /// The links of `near` over the given relations (all of them when
    /// `relations` is empty), each paired with its far endpoint, keeping
    /// only visible far endpoints. Ordered by `(relation, far, from, to)`,
    /// without duplicates.
    fn around(
        &mut self,
        near: &Node,
        relations: &[RelationName],
        walk: Walk,
    ) -> Result<Vec<(Node, Edge)>> {
        let mut raw = Vec::new();
        if relations.is_empty() {
            raw.extend(self.source.edges(&near.0, &near.1, None, walk)?);
        } else {
            for relation in relations {
                raw.extend(self.source.edges(&near.0, &near.1, Some(relation), walk)?);
            }
        }
        let mut sorted: BTreeMap<(RelationName, Node, Node, Node), Edge> = BTreeMap::new();
        for edge in raw {
            let other = far(&edge, near);
            let key = (
                edge.relation.clone(),
                other,
                edge.from.clone(),
                edge.to.clone(),
            );
            sorted.entry(key).or_insert(edge);
        }
        let mut out = Vec::with_capacity(sorted.len());
        for ((_, other, _, _), edge) in sorted {
            if self.visible(&other)? {
                out.push((other, edge));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use lakelands_api::engine::{Declared, Scan};
    use lakelands_api::event::{Digest, EventId};
    use lakelands_api::id::{Position, Version};
    use lakelands_api::record::{StoredRecord, Value};
    use lakelands_api::write::{Proposal, ProposalFilter};

    use super::*;

    fn node(t: &str, k: &str) -> Node {
        (TypeName::new(t), RecordKey::new(k))
    }

    fn stored(key: &str, retired: bool, restricted: bool) -> StoredRecord {
        StoredRecord {
            ty: TypeName::new("n"),
            key: RecordKey::new(key),
            version: Version(1),
            fields: BTreeMap::new(),
            meta: BTreeMap::new(),
            position: Position(0),
            retired,
            restricted,
        }
    }

    /// A projection view holding a few records and no links; only `get`
    /// and `edges` are ever asked.
    struct View {
        records: Vec<StoredRecord>,
        asked: RefCell<Vec<Option<RelationName>>>,
    }

    impl ProjectionView for View {
        fn position(&self) -> Option<Position> {
            None
        }
        fn declarations(&self) -> Result<Vec<Declared>> {
            unreachable!()
        }
        fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
            Ok(self
                .records
                .iter()
                .find(|r| r.ty == *ty && r.key == *key)
                .cloned())
        }
        fn scan(&self, _: &TypeName, _: &Scan) -> Result<Vec<StoredRecord>> {
            unreachable!()
        }
        fn edges(
            &self,
            _: &TypeName,
            _: &RecordKey,
            relation: Option<&RelationName>,
            _: Walk,
        ) -> Result<Vec<Edge>> {
            self.asked.borrow_mut().push(relation.cloned());
            Ok(Vec::new())
        }
        fn lexical(
            &self,
            _: &TypeName,
            _: &str,
            _: Option<&[lakelands_api::id::FieldName]>,
            _: u32,
        ) -> Result<Vec<(RecordKey, f64)>> {
            unreachable!()
        }
        fn proposals(&self, _: &ProposalFilter) -> Result<Vec<Proposal>> {
            unreachable!()
        }
        fn is_member(&self, _: &str, _: &TypeName, _: &RecordKey) -> Result<bool> {
            unreachable!()
        }
        fn applied(&self, _: &EventId) -> Result<Option<Position>> {
            unreachable!()
        }
        fn output(&self, _: &str, _: &Digest) -> Result<Option<Value>> {
            unreachable!()
        }
    }

    fn view() -> View {
        View {
            records: vec![
                stored("live", false, false),
                stored("retired", true, false),
                stored("restricted", false, true),
            ],
            asked: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn through_a_view_a_live_record_is_visible_and_a_retired_or_missing_one_is_not() {
        let v = view();
        let visible = |k: &str| EdgeSource::is_visible(&v, &TypeName::new("n"), &RecordKey::new(k));
        assert!(visible("live").unwrap());
        assert!(!visible("retired").unwrap());
        assert!(!visible("missing").unwrap());
    }

    #[test]
    fn through_a_view_a_restricted_record_is_left_to_the_caller() {
        let v = view();
        let n = node("n", "restricted");
        assert!(EdgeSource::is_visible(&v, &n.0, &n.1).unwrap());
    }

    #[test]
    fn a_boxed_view_is_an_edge_source() {
        let boxed: Box<dyn ProjectionView> = Box::new(view());
        let got = neighbors(
            &*boxed,
            &node("n", "live"),
            &RelationName::new("next"),
            Walk::Out,
        )
        .unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn a_walk_over_named_relations_asks_the_view_for_each_one() {
        let v = view();
        let relations = [RelationName::new("a"), RelationName::new("b")];
        subgraph(&v, &node("n", "live"), &relations, Walk::Both, 1, 10, 16).unwrap();
        assert_eq!(
            *v.asked.borrow(),
            vec![Some(RelationName::new("a")), Some(RelationName::new("b"))]
        );
    }

    #[test]
    fn a_depth_over_the_ceiling_is_refused() {
        let err = check_depth(Depth { min: 1, max: 17 }, 16).unwrap_err();
        assert!(matches!(
            err,
            Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            }
        ));
        assert!(check_depth(Depth { min: 1, max: 16 }, 16).is_ok());
    }

    #[test]
    fn an_empty_depth_range_is_refused() {
        assert!(check_depth(Depth { min: 2, max: 1 }, 16).is_err());
        assert!(check_depth(Depth { min: 0, max: 0 }, 16).is_ok());
    }
}
