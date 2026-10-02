//! A write touches only the fields it carries
//! (`docs/decisions/012-writes-touch-only-their-fields.md`,
//! `docs/decisions/006-grants-not-roles.md`).
//!
//! Two writers share one record: `amount` is the importer's, `verdict`
//! the reviewer's. Each may write only its own field, a patch is merged
//! per field, and a field that depends on another is cleared or kept by
//! the fold, never by the caller.

use std::sync::Arc;

use lakelands_api::ontology::{OnInputChange, StorageKind};
use lakelands_api::{Catalog, Error, FieldName, LakeStore, Outcome, Principal, Refusal, TypeName};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, declare, doors, field, field_writer, get_at, head_unmoved, owner, read, refused_on,
    try_upsert, type_decl, upsert, value,
};

check!(
    /// The record checks refuse in their order.
    RECORD_CHECKS_IN_ORDER = a_write_breaking_two_record_checks_is_refused_by_the_earlier,
    "016, extended (lakelands#338): a record write that breaks two adjacent record checks is refused by the earlier: the type before an empty patch, the key before the fields, field by field in name order with the door before the grant, the grant before the value, the value before the key fill, the key's agreement before a reference, a reference before the version, the version before a required field; a retry answers its first outcome before any of them"
);

check!(
    /// Writer A's rewrite leaves writer B's field standing.
    REWRITE_LEAVES_OTHER_WRITERS_FIELDS = a_rewrite_by_one_writer_leaves_another_writers_fields_alone,
    "012: a patch is merged per field, so a writer that sends only its own fields cannot erase anyone else's"
);

check!(
    /// A dependent field declared `clear` is cleared by the fold.
    CLEAR_ON_INPUT_CHANGE = changing_an_input_clears_a_dependent_field_declared_clear,
    "012: with on_input_change clear, the fold clears the dependent field in the same event that changes its input"
);

check!(
    /// A dependent field declared `keep` stays, visibly older.
    KEEP_ON_INPUT_CHANGE = changing_an_input_keeps_a_dependent_field_declared_keep_and_shows_it_older,
    "012 and 010: with on_input_change keep, the value stays and its provenance shows it is older than its input"
);

check!(
    /// One forbidden field refuses the whole patch.
    UNGRANTED_FIELD_REFUSES_WHOLE_PATCH = a_patch_with_a_field_the_principal_may_not_write_is_refused_whole,
    "006 and 012: a write carrying a field its principal may not write is refused whole, never applied in part"
);

check!(
    /// Absent is unchanged, `null` clears.
    SPARSE_PATCH = a_field_absent_from_a_patch_is_unchanged_and_null_clears_it,
    "012 and the log format: an upsert is a sparse patch; an absent field is unchanged and null clears the field"
);

check!(
    /// A field the type does not declare refuses the whole patch.
    UNDECLARED_FIELD_REFUSES_WHOLE_PATCH = a_patch_with_an_undeclared_field_is_refused_whole,
    "015: a field the lake's map does not declare is refused as invalid, and nothing of the patch is written"
);

/// Upserts `fields` into `invoice` `i1` as `who`.
async fn i1(cat: &dyn Catalog, who: &Principal, fields: Value) -> Result<Outcome, Failure> {
    upsert(cat, who, "invoice", "i1", fields).await
}

/// Declares `invoice` with the importer's `amount` and the reviewer's
/// `verdict`, the latter depending on `amount` as `on_change` says.
async fn two_writer_invoice(
    store: &dyn LakeStore,
    on_change: OnInputChange,
) -> Result<(), Failure> {
    let mut verdict = field("verdict", StorageKind::Text);
    verdict.depends_on = vec![FieldName::from("amount")];
    verdict.on_input_change = on_change;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), verdict],
    );
    Lake::of(store).declare(ty).await?;
    Ok(())
}

async fn a_rewrite_by_one_writer_leaves_another_writers_fields_alone(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let mut ty = type_decl(
        "invoice",
        vec![
            field("amount", StorageKind::Decimal),
            field("verdict", StorageKind::Text),
        ],
    );
    ty.fields.push(field("note", StorageKind::Text));
    declare(cat.as_ref(), &owner(store.as_ref()), ty).await?;
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount", "note"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);

    i1(
        cat.as_ref(),
        &a,
        json!({"amount": "100.00", "note": "first"}),
    )
    .await?;
    i1(cat.as_ref(), &b, json!({"verdict": "approved"})).await?;
    i1(cat.as_ref(), &a, json!({"amount": "120.00"})).await?;

    let rec = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "the importer rewrote `amount` and the reviewer's `verdict` is now {:?}; \
         a write touches only the fields it carries, so it must still be \"approved\"",
        value(&rec, "verdict")
    );
    ensure!(
        value(&rec, "amount") == Some(&json!("120.00")),
        "the importer's rewrite of `amount` to \"120.00\" reads back as {:?}",
        value(&rec, "amount")
    );
    ensure!(
        value(&rec, "note") == Some(&json!("first")),
        "the importer's second patch did not carry `note`, which is now {:?}; \
         an absent field is unchanged, even for the writer who set it",
        value(&rec, "note")
    );
    Ok(())
}

async fn changing_an_input_clears_a_dependent_field_declared_clear(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Clear).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);
    let own = owner(store.as_ref());

    i1(cat.as_ref(), &a, json!({"amount": "100.00"})).await?;
    i1(cat.as_ref(), &b, json!({"verdict": "approved"})).await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "writing `verdict` itself must not clear it, yet it reads {:?}",
        value(&rec, "verdict")
    );

    let changed = i1(cat.as_ref(), &a, json!({"amount": "120.00"})).await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict").is_none(),
        "`verdict` depends on `amount` with on_input_change clear; `amount` changed \
         and `verdict` still reads {:?}. The fold must clear it; no caller does",
        value(&rec, "verdict")
    );

    let ty = TypeName::from("invoice");
    let at_change = cat
        .get(&own, &ty, get_at("i1", Some(changed.position())))
        .await
        .must("reading `i1` as of the write that changed `amount`")?;
    ensure!(
        value(&at_change.data, "verdict").is_none(),
        "as of position {} (the write that changed `amount`), `verdict` still reads {:?}; \
         the fold clears it in the same event that changes the input",
        changed.position(),
        value(&at_change.data, "verdict")
    );
    Ok(())
}

async fn changing_an_input_keeps_a_dependent_field_declared_keep_and_shows_it_older(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Keep).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);

    i1(cat.as_ref(), &a, json!({"amount": "100.00"})).await?;
    i1(cat.as_ref(), &b, json!({"verdict": "approved"})).await?;
    i1(cat.as_ref(), &a, json!({"amount": "120.00"})).await?;

    let rec = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "`verdict` is declared on_input_change keep; after `amount` changed it reads {:?} \
         instead of staying \"approved\"",
        value(&rec, "verdict")
    );
    let (Some(v), Some(amt)) = (rec.meta.get("verdict"), rec.meta.get("amount")) else {
        return Err(Failure::new(format!(
            "a read with with_meta must carry provenance for every field with a value; \
             it carried it for {:?}",
            rec.meta.keys().collect::<Vec<_>>()
        )));
    };
    ensure!(
        v.position < amt.position && v.known_at <= amt.known_at,
        "the kept `verdict` must show it is older than its input: its provenance says \
         position {} / known_at {}, `amount`'s says position {} / known_at {}",
        v.position,
        v.known_at,
        amt.position,
        amt.known_at
    );
    ensure!(
        v.writer == b.writer(),
        "the kept `verdict` was written by `{}`, but its provenance names `{}`",
        b.writer(),
        v.writer
    );
    Ok(())
}

async fn a_patch_with_a_field_the_principal_may_not_write_is_refused_whole(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Keep).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let own = owner(store.as_ref());

    i1(cat.as_ref(), &a, json!({"amount": "100.00"})).await?;
    let head = store.head().await.must("reading the head")?;

    let err = try_upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "999.00", "verdict": "approved"}),
    )
    .await
    .must_fail("the importer writing `amount` and the reviewer's `verdict` together")?;
    ensure!(
        matches!(refused_on(&err), Some(("verdict", Refusal::NotGranted))),
        "a patch carrying `verdict`, which the importer holds no write grant for, must be \
         refused as not granted on `verdict`; it failed with {err:?}"
    );

    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "amount") == Some(&json!("100.00")) && value(&rec, "verdict").is_none(),
        "the refused patch was applied in part: `amount` reads {:?} and `verdict` {:?}; \
         a refused write changes nothing",
        value(&rec, "amount"),
        value(&rec, "verdict")
    );
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "the refused patch moved the head from {head} to {after}; a refused write appends nothing"
    );
    Ok(())
}

async fn a_field_absent_from_a_patch_is_unchanged_and_null_clears_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let ty = type_decl(
        "note",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    l.declare(ty).await?;

    l.upsert(
        "note",
        "n1",
        json!({"title": "draft", "body": "text", "pages": 3}),
    )
    .await?;
    l.upsert("note", "n1", json!({"body": null})).await?;
    let rec = l.read("note", "n1").await?;
    ensure!(
        value(&rec, "body").is_none(),
        "a patch setting `body` to null must clear it; it reads {:?}",
        value(&rec, "body")
    );
    ensure!(
        value(&rec, "title") == Some(&json!("draft")) && value(&rec, "pages") == Some(&json!(3)),
        "a patch carrying only `body` changed the fields it did not carry: \
         `title` reads {:?}, `pages` {:?}",
        value(&rec, "title"),
        value(&rec, "pages")
    );

    l.upsert("note", "n1", json!({"title": "final"})).await?;
    let rec = l.read("note", "n1").await?;
    ensure!(
        value(&rec, "body").is_none() && value(&rec, "title") == Some(&json!("final")),
        "after clearing `body` and then writing only `title`, the record reads {:?}; \
         `body` must stay cleared and `title` must be \"final\"",
        rec.fields
    );
    Ok(())
}

async fn a_patch_with_an_undeclared_field_is_refused_whole(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl("memo", vec![field("text", StorageKind::Text)]);
    l.declare(ty).await?;
    l.upsert("memo", "m1", json!({"text": "hello"})).await?;

    for key in ["m1", "m2"] {
        let before = store.head().await.must("reading the head")?;
        let remark = json!({"text": "changed", "remark": "call before dunning"});
        let err = try_upsert(cat.as_ref(), own, "memo", key, remark)
            .await
            .must_fail(&format!(
                "writing the undeclared field `remark` to `memo` `{key}`"
            ))?;
        head_unmoved(
            store.as_ref(),
            before,
            &format!("writing the undeclared field `remark` to `memo` `{key}`"),
        )
        .await?;
        ensure!(
            matches!(
                &err,
                Error::Refused { reason: Refusal::Invalid { .. }, field }
                    if field.as_ref().is_none_or(|f| f.as_str() == "remark")
            ),
            "a patch carrying `remark`, which `memo` does not declare, must be refused as \
             invalid (naming `remark` if it names a field); it failed with {err:?}"
        );
    }

    let rec = l.read("memo", "m1").await?;
    ensure!(
        value(&rec, "text") == Some(&json!("hello")) && value(&rec, "remark").is_none(),
        "the refused patch was applied in part to `m1`: it reads {:?}",
        rec.fields
    );
    let missing = cat
        .get(own, &TypeName::from("memo"), get_at("m2", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused patch to the new key `m2` created the record anyway: {missing:?}"
    );
    Ok(())
}

async fn a_write_breaking_two_record_checks_is_refused_by_the_earlier(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::write::WriteOpts;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let required = |name: &str| lakelands_api::ontology::FieldDecl {
        required: true,
        ..field(name, StorageKind::Text)
    };
    let mut part = field("part", StorageKind::Reference);
    part.ty.target = Some("o".into());
    let src = crate::conformance::fixture::sourced("src", StorageKind::Number);
    let mut o = type_decl(
        "o",
        vec![
            required("code"),
            required("need"),
            field("qty", StorageKind::Number),
            src,
            part,
        ],
    );
    o.key = vec!["code".into()];
    l.declare(o).await?;
    for k in ["k2", "k3"] {
        l.upsert("o", k, json!({"code": k, "need": "n"})).await?;
    }
    // Writes `code`, `need` and `part`; neither `qty` nor `src`.
    let narrow = field_writer(store.as_ref(), "narrow", "o", &["code", "need", "part"]);
    // type, key, patch, an expected version (0: none), as `narrow`, what
    // is due: the earlier of the two checks each row breaks.
    let rows = [
        ("none", "k", "{}", 0, false, "not_found"),
        ("o", "", r#"{"bogus": 1}"#, 0, false, "invalid"),
        ("o", "k1", r#"{"src": 1}"#, 0, true, "wrong_door src"),
        (
            "o",
            "k1",
            r#"{"qty": 1, "zzz": 1}"#,
            0,
            true,
            "not_granted qty",
        ),
        ("o", "k1", r#"{"qty": "x"}"#, 0, true, "not_granted qty"),
        (
            "o",
            "a:b",
            r#"{"qty": "x", "need": "n"}"#,
            0,
            false,
            "constraint qty",
        ),
        (
            "o",
            "k1",
            r#"{"code": "x", "need": "n", "part": "none"}"#,
            0,
            false,
            "invalid",
        ),
        (
            "o",
            "k2",
            r#"{"part": "none"}"#,
            9,
            false,
            "constraint part",
        ),
        ("o", "k3", r#"{"need": null}"#, 9, false, "conflict"),
    ];
    let head = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for (ty, key, fields, v, as_narrow, due) in rows {
        let who = if as_narrow { &narrow } else { own };
        let expected_version = (v > 0).then_some(lakelands_api::Version(v));
        let opts = WriteOpts {
            expected_version,
            ..WriteOpts::default()
        };
        let patch = serde_json::from_str(fields).expect("a patch");
        let got = cat.upsert(who, &ty.into(), &key.into(), patch, opts).await;
        let e = got
            .as_ref()
            .err()
            .map(|e| serde_json::to_value(e).unwrap_or_default());
        let kind = e
            .as_ref()
            .map(|e| e["reason"]["refusal"].as_str().or(e["error"].as_str()));
        let field = e
            .as_ref()
            .and_then(|e| e["field"].as_str())
            .map(|f| format!(" {f}"));
        let answered = format!(
            "{}{}",
            kind.flatten().unwrap_or("?"),
            field.unwrap_or_default()
        );
        if answered != due {
            red.push(format!(
                "{ty} `{key}` {fields}: `{due}` was due; it answered {got:?}"
            ));
        }
    }
    head_unmoved(
        store.as_ref(),
        head,
        "the writes breaking two record checks",
    )
    .await?;
    // A retry answers before any record check: the same writer, now with
    // no grant, sends an empty patch under the first write's key.
    let once = WriteOpts {
        idempotency_key: Some("once".into()),
        ..WriteOpts::default()
    };
    let first = json!({"code": "k4", "need": "n"});
    let applied = cat
        .upsert(
            own,
            &"o".into(),
            &"k4".into(),
            crate::conformance::fixture::patch(first),
            once.clone(),
        )
        .await;
    let bare = lakelands_api::Principal {
        grants: Default::default(),
        ..own.clone()
    };
    let retried = cat
        .upsert(&bare, &"o".into(), &"k4".into(), Default::default(), once)
        .await;
    if applied.is_err() || retried != applied {
        red.push(format!(
            "a retry under `once` answered {retried:?}; the first write {applied:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a write breaking two record checks is refused by the earlier: {}",
        red.join("; ")
    );
    Ok(())
}
