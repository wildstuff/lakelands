//! The pure helpers a host reaches in `lakelands-api` rather than in the
//! core: the schema hash, the key rule and the canonical bytes under them
//! (`docs/hosting.md`). A host computes these without the store, so they
//! are held here to the log format's own vector and to fixed keys.

use lakelands_api::canonical;
use lakelands_api::key::{key_component, key_components, link_key, record_key};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::shape::{schema_hash, shape};
use serde_json::{Value, json};

const VECTOR: &str = include_str!("../../../docs/log-format/vectors/18/schema-hash.json");

/// The log format's schema-hash vector is a view whose conditions are
/// written as text: the helper hashes it to the digest the vector pins,
/// over exactly the shape object the vector spells out.
#[test]
fn the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins() {
    let vector: Value = serde_json::from_str(VECTOR).unwrap();
    let decl: TypeDecl = serde_json::from_value(vector["declaration"].clone()).unwrap();
    assert_eq!(
        String::from_utf8(canonical::to_vec(&shape(&decl).unwrap())).unwrap(),
        vector["shape_canonical"].as_str().unwrap(),
        "the hashed object is the vector's shape, byte for byte"
    );
    assert_eq!(
        schema_hash(&decl).unwrap(),
        vector["schema_hash"].as_str().unwrap()
    );
}

/// The key rule over fixed values: text, a number in its one form and a
/// boolean, with `:` and `%` escaped in a component and read back.
#[test]
fn a_record_key_is_its_components_escaped_and_joined() {
    let ty: TypeDecl =
        serde_json::from_value(json!({"name": "t", "key": ["a", "b", "c", "d"]})).unwrap();
    let fields = serde_json::from_value(json!({
        "a": "x:y%z", "b": 2.0, "c": true, "d": "12.50"
    }))
    .unwrap();
    let key = record_key(&ty, &fields).unwrap();
    assert_eq!(key.as_str(), "x%3Ay%25z:2:true:12.50");
    assert_eq!(key_components(&key), ["x:y%z", "2", "true", "12.50"]);
    assert_eq!(key_component(&json!(null)), None, "null is not a component");
}

/// A link's key is the key rule over its two ends, each end's key one
/// component; the ends read back whole, and each end's own components
/// from them.
#[test]
fn a_link_key_nests_its_ends_and_reads_back() {
    let from = "a%3Ab:c".into();
    let link = link_key(&from, &"P-1".into());
    assert_eq!(link.as_str(), "a%253Ab%3Ac:P-1");
    let ends = key_components(&link);
    assert_eq!(ends, ["a%3Ab:c", "P-1"]);
    assert_eq!(key_components(&ends[0].as_str().into()), ["a:b", "c"]);
}
