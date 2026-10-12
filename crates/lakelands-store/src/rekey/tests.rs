use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, Location, Refusal};
use serde_json::json;

use super::*;

/// A host's own `sys.` type keyed by a reference to a re-keyed type
/// would be re-keyed too; no principal can declare one through the
/// doors, so the refusal is checked here.
#[test]
fn a_system_type_in_the_closure_is_refused() {
    let engine = lakelands_engine_mem::MemoryOpener::new()
        .open(&Location::memory("rekey-sys"), OpenMode::Create)
        .unwrap();
    let view = engine.view(None).unwrap();
    let decl = |v: serde_json::Value| -> Declaration { serde_json::from_value(v).unwrap() };
    let mut after = Ontology::default();
    after.put(
        decl(
            json!({"declare": "type", "name": "account", "key": ["iban"],
                        "fields": [{"name": "iban", "type": {"kind": "text"}, "required": true}]}),
        ),
        2,
        None,
    );
    after.put(
        decl(
            json!({"declare": "type", "name": "sys.hold", "key": ["account"],
                        "fields": [{"name": "account",
                                    "type": {"kind": "reference", "target": "account"},
                                    "required": true}]}),
        ),
        1,
        None,
    );
    let keyed = vec![(RecordKey::from("1"), Some(RecordKey::from("DE1")))];
    let refused = plan(&after, view.as_ref(), &"account".into(), keyed, &[]);
    assert!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
                if message.contains("sys.hold")),
        "{:?}",
        refused.err()
    );
}
