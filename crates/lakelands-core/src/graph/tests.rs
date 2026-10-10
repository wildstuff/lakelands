pub(super) mod fixture;

use std::cell::RefCell;
use std::collections::BTreeSet;

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

/// A projection view holding a few records and no links; only
/// `get_many` and `edges` are ever asked.
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
    fn get(&self, _: &TypeName, _: &RecordKey) -> Result<Option<StoredRecord>> {
        unreachable!()
    }
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        Ok(self
            .records
            .iter()
            .filter(|r| r.ty == *ty && keys.contains(&r.key))
            .cloned()
            .collect())
    }
    fn scan(&self, _: &TypeName, _: &Scan) -> Result<Vec<StoredRecord>> {
        unreachable!()
    }
    fn count(&self, _: &TypeName, _: &Scan) -> Result<u64> {
        unreachable!()
    }
    fn edges(
        &self,
        _: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        _: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        self.asked.borrow_mut().push(relation.cloned());
        Ok(vec![Vec::new(); keys.len()])
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
    fn members(&self, _: &str, _: &TypeName) -> Result<BTreeSet<RecordKey>> {
        unreachable!()
    }
    fn applied(&self, _: &EventId) -> Result<Option<Position>> {
        unreachable!()
    }
    fn output(&self, _: &str, _: &Digest) -> Result<Option<Value>> {
        unreachable!()
    }

    fn matching(
        &self,
        _: &lakelands_api::seam::FeedFilter,
        _: Position,
        _: Position,
        _: u32,
    ) -> Result<lakelands_api::engine::Matched> {
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
    let asked = [
        node("n", "live"),
        node("n", "retired"),
        node("n", "missing"),
    ];
    assert_eq!(
        EdgeSource::visible(&v, &asked).unwrap(),
        [true, false, false]
    );
}

#[test]
fn through_a_view_a_restricted_record_is_left_to_the_caller() {
    let v = view();
    let n = node("n", "restricted");
    assert_eq!(EdgeSource::visible(&v, &[n]).unwrap(), [true]);
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
    let steps = fixture::over(&["a", "b"], Walk::Both, 1);
    subgraph(&v, &[node("n", "live")], &steps, 16, 0, 10, fixture::every).unwrap();
    assert_eq!(
        *v.asked.borrow(),
        vec![Some(RelationName::new("a")), Some(RelationName::new("b"))]
    );
}

#[test]
fn an_empty_depth_range_is_refused() {
    assert!(check_depth(Depth { min: 2, max: 1 }, 16).is_err());
    assert!(check_depth(Depth { min: 0, max: 0 }, 16).is_ok());
}
