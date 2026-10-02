//! A lake's settings are records of `sys.setting`, read in any expression
//! by `setting('<key>')` as they stood at the position being read
//! (`docs/seams.md`, `SystemVocabulary`). A setting is data with a
//! history, so a read pinned to an earlier position answers with the
//! setting of that position, not today's.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{Find, LakeStore, Position};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, field, read_at, type_decl};

check!(
    /// `setting('key')` reads the setting as of the read's position.
    SETTING_AS_OF = a_setting_is_read_as_it_stood_at_the_position_read,
    "seams, SystemVocabulary: setting('key') in an expression reads the sys.setting record with that key as of the position being read"
);

async fn a_setting_is_read_as_it_stood_at_the_position_read(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("band", vec![field("level", StorageKind::Number)]))
        .await?;
    l.upsert("band", "low", json!({"level": 1})).await?;
    l.upsert("band", "high", json!({"level": 2})).await?;
    let first = l.set("threshold", json!(1)).await?.position();
    let second = l.set("threshold", json!(2)).await?.position();

    let at = |as_of: Option<Position>| Find {
        filter: Some("level = setting('threshold')".into()),
        at: read_at(as_of),
        ..Find::default()
    };
    let keys = |records: Vec<lakelands_api::Record>| -> Vec<String> {
        records.into_iter().map(|r| r.key.0).collect()
    };
    for (as_of, want, when) in [
        (Some(first), "low", "as of the first setting"),
        (Some(second), "high", "as of the second setting"),
        (None, "high", "at the head"),
    ] {
        let got = keys(l.all_found("band", at(as_of)).await?);
        ensure!(
            got == [want],
            "`threshold` was set to 1 at {first} and to 2 at {second}; `find` with \
             `level = setting('threshold')` {when} answered {got:?}, where only `{want}` \
             matches the setting as it stood there"
        );
    }
    Ok(())
}
