pub(super) mod fixture;

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
