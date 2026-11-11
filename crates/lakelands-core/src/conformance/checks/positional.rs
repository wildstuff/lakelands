//! A struct is read from an object, never from a sequence
//! (`docs/decisions/110-a-struct-is-read-from-an-object.md`).
//!
//! Every door reads its contract types through `lakelands_api::strict`, so
//! a positional form is refused by one sentence, made in one place: it
//! names the members the type takes, and the object form reads.

use lakelands_api::strict;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::conformance::prelude::*;

/// The sentence a struct asked for as a sequence is refused with.
const REFUSED: &str =
    "its members are named, not given by position: write it as an object that takes";

check!(
    REQUEST_SEQUENCE = a_request_struct_written_as_a_sequence_is_refused,
    "110: a request struct is read from an object: a sequence is refused, naming the members it takes, and an object naming them reads",
    given
);

check!(
    DECLARATION_SEQUENCE = a_declaration_written_as_a_sequence_is_refused,
    "110: a declaration is read from an object tagged by `declare`: a sequence is refused, naming `declare`, and the tagged object reads",
    given
);

check!(
    GRANT_SEQUENCE = a_grant_written_as_a_sequence_is_refused,
    "110: a grant is read from an object: a sequence is refused, naming each member a grant takes, and the object form reads",
    given
);

async fn a_request_struct_written_as_a_sequence_is_refused(store: Store, _given: Given) -> Held {
    store.head().await.must("the head")?;
    refused::<Find>(
        json!([{"limit": 1}]),
        "a find",
        &["`where`", "`order_by`", "`limit`", "`as_of`"],
    )?;
    reads::<Find>(
        json!({"order_by": [{"field": "amount", "dir": "asc"}], "limit": 1}),
        "a find",
    )
}

async fn a_declaration_written_as_a_sequence_is_refused(store: Store, _given: Given) -> Held {
    store.head().await.must("the head")?;
    refused::<Declaration>(json!(["type", "memo"]), "a declaration", &["`declare`"])?;
    reads::<Declaration>(json!({"declare": "type", "name": "memo"}), "a declaration")
}

async fn a_grant_written_as_a_sequence_is_refused(store: Store, _given: Given) -> Held {
    store.head().await.must("the head")?;
    refused::<Grants>(
        json!([]),
        "a grant",
        &[
            "`default`",
            "`types`",
            "`review`",
            "`schema`",
            "`ingest`",
            "`restricted`",
        ],
    )?;
    reads::<Grants>(
        json!({"default": "read", "types": {"memo": {"default": "read"}}}),
        "a grant",
    )
}

/// Refuses `value` read as `T`, in `REFUSED`'s words, naming `names`.
fn refused<T: DeserializeOwned>(value: Value, what: &str, names: &[&str]) -> Held {
    let said = match strict::from_value::<T>(value) {
        Ok(_) => return Err(Failure::new(format!("{what} as a sequence read"))),
        Err(e) => e.to_string(),
    };
    ensure!(
        said.contains(REFUSED),
        "{what} as a sequence is refused without the form: {said}"
    );
    for name in names {
        ensure!(
            said.contains(name),
            "{what} as a sequence is refused without {name}: {said}"
        );
    }
    Ok(())
}

/// Reads `value` as `T`, the object form every door takes.
fn reads<T: DeserializeOwned>(value: Value, what: &str) -> Held {
    match strict::from_value::<T>(value) {
        Ok(_) => Ok(()),
        Err(e) => Err(Failure::new(format!(
            "{what} written as an object is refused: {e}"
        ))),
    }
}
