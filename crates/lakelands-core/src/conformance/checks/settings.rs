//! A lake's settings are records of `sys.setting`, read in any expression
//! by `setting('<key>')` as they stood at the position being read
//! (`docs/seams.md`, `SystemVocabulary`). A setting is data with a
//! history, so a read pinned to an earlier position answers with the
//! setting of that position, not today's.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{Find, LakeStore, Position, ReadAt};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{all_found, declare, doors, field, owner, type_decl, upsert};

check!(
    /// `setting('key')` reads the setting as of the read's position.
    SETTING_AS_OF = a_setting_is_read_as_it_stood_at_the_position_read,
    "seams, SystemVocabulary: setting('key') in an expression reads the sys.setting record with that key as of the position being read"
);

async fn a_setting_is_read_as_it_stood_at_the_position_read(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("band", vec![field("level", StorageKind::Number)]),
    )
    .await?;
    upsert(cat.as_ref(), &own, "band", "low", json!({"level": 1})).await?;
    upsert(cat.as_ref(), &own, "band", "high", json!({"level": 2})).await?;
    let first = upsert(
        cat.as_ref(),
        &own,
        "sys.setting",
        "threshold",
        json!({"key": "threshold", "value": 1}),
    )
    .await?
    .position();
    let second = upsert(
        cat.as_ref(),
        &own,
        "sys.setting",
        "threshold",
        json!({"key": "threshold", "value": 2}),
    )
    .await?
    .position();

    let at = |as_of: Option<Position>| Find {
        filter: Some("level = setting('threshold')".into()),
        at: ReadAt {
            as_of,
            valid_at: None,
        },
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
        let got = keys(all_found(cat.as_ref(), &own, "band", at(as_of)).await?);
        ensure!(
            got == [want],
            "`threshold` was set to 1 at {first} and to 2 at {second}; `find` with \
             `level = setting('threshold')` {when} answered {got:?}, where only `{want}` \
             matches the setting as it stood there"
        );
    }
    Ok(())
}
