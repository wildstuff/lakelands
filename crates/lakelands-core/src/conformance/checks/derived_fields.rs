//! Derived fields: computed by the fold, written by no door
//! (`lakelands_api::ontology::Authority::Derived`, `docs/log-format.md`
//! §2.3).
//!
//! An author declares `gross = amount * 1.19` once and writes only
//! `amount`; the store fills `gross` in the same event, with `derived` as
//! its origin, and a replay reproduces it. A store that accepted the
//! declaration and then answered records without the field would leave
//! every total built on it quietly short, which is why the check reads
//! the field back rather than trusting the declaration.

use std::sync::Arc;

use lakelands_api::ontology::{Authority, FieldDecl, StorageKind};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, LakeStore, Origin, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, computed_by, dec, decimal, derived, field, get_at, head_unmoved, patch, refused_on,
    refused_unmoved, source_record, try_upsert, type_decl, value,
};

check!(
    /// The fold computes a derived field.
    COMPUTED_BY_THE_FOLD = a_derived_field_is_computed_by_the_fold_on_every_write,
    "the ontology and the log format: the fold computes a derived field from its expression in the event that changes its inputs"
);

check!(
    /// No door writes a derived field.
    NO_DOOR_WRITES_IT = a_derived_field_written_through_a_door_is_refused_as_the_wrong_door,
    "015 and the ontology: no door writes a derived field; a patch carrying one is refused whole as the wrong door"
);

check!(
    /// A derived field added later is computed from then on.
    ADDED_LATER = a_derived_field_added_to_a_live_type_is_computed_on_later_writes,
    "the ontology: a derived field added with add_field is kept by the fold for every write after it, like one declared with the type"
);

fn gross() -> FieldDecl {
    derived("gross", StorageKind::Decimal, "amount * 1.19", &["amount"])
}

async fn a_derived_field_is_computed_by_the_fold_on_every_write(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    l.declare(ty).await?;

    let first = l
        .upsert("invoice", "i1", json!({"amount": "100.00"}))
        .await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` is declared as amount * 1.19; after writing amount 100.00 it must read \
         exactly 119 as a decimal string, but reads {got:?}. A declaration the store \
         accepted is a field it owes"
    );
    let origin = rec.meta.get("gross").map(|m| m.origin);
    ensure!(
        origin == Some(Origin::Derived),
        "a field the fold writes carries origin `derived` in its provenance; `gross` \
         carries {origin:?}"
    );

    l.upsert("invoice", "i1", json!({"amount": "200.00"}))
        .await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("238")),
        "after amount changed to 200.00, `gross` must follow to 238; it reads {got:?}"
    );

    let then = cat
        .get(
            own,
            &TypeName::from("invoice"),
            get_at("i1", Some(first.position())),
        )
        .await
        .must("reading `i1` as of its first write")?;
    let got = then.data.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "as of the first write, `gross` must read 119 as it did then; it reads {got:?}"
    );
    Ok(())
}

async fn a_derived_field_written_through_a_door_is_refused_as_the_wrong_door(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    l.declare(ty).await?;

    let before = store.head().await.must("reading the head")?;
    let fields = json!({"amount": "100.00", "gross": "1.00"});
    let answer = try_upsert(cat.as_ref(), own, "invoice", "i1", fields).await;
    let what = "the owner writing the derived field `gross`";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(refused_on(&err), Some(("gross", Refusal::WrongDoor))),
        "a patch carrying the derived field `gross` must be refused as the wrong door on \
         `gross`, even for the owner; it failed with {err:?}"
    );
    let missing = cat
        .get(own, &TypeName::from("invoice"), get_at("i1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused patch created `i1` anyway: {missing:?}"
    );
    Ok(())
}

async fn a_derived_field_added_to_a_live_type_is_computed_on_later_writes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    l.declare(ty).await?;
    l.upsert("invoice", "i1", json!({"amount": "100.00"}))
        .await?;

    cat.add_field(own, &TypeName::from("invoice"), gross())
        .await
        .must("adding the derived field `gross` to the live type `invoice`")?;

    l.upsert("invoice", "i2", json!({"amount": "100.00"}))
        .await?;
    let rec = l.read("invoice", "i2").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` was added by add_field; a record written afterwards with amount 100.00 \
         must read gross 119, but reads {got:?}"
    );

    l.upsert("invoice", "i1", json!({"amount": "10.00"}))
        .await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("11.9")),
        "`i1` was written before `gross` existed; once its amount changes to 10.00, \
         the fold must compute gross 11.9, but it reads {got:?}"
    );
    Ok(())
}

check!(
    /// No door at all writes a derived field.
    NO_DOOR_AT_ALL = a_derived_field_accepts_no_value_from_a_source_or_a_verb_either,
    "015 and 016: a derived field is the fold's alone; a source record or a verb that would set it is refused whole, not only an upsert"
);

check!(
    /// A derived field follows its function's version.
    FUNCTION_VERSION_RECOMPUTES = a_derived_field_reading_a_function_is_recomputed_when_the_function_changes,
    "022: a derived field that names a function is computed by that function's declaration in force; a new version of the function recomputes it and names that version as its procedure"
);

async fn a_derived_field_accepts_no_value_from_a_source_or_a_verb_either(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut net = field("net", StorageKind::Decimal);
    net.authority = Authority::Shared;
    let gross = derived("gross", StorageKind::Decimal, "net * 2", &["net"]);
    l.declare(type_decl("bill", vec![net, gross])).await?;
    let verb: Declaration = serde_json::from_value(json!({
        "declare": "verb", "name": "inflate", "on": "bill", "risk": "low",
        "effect": "write", "set": [{"field": "gross", "value": "'99'"}]
    }))
    .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))?;
    let declared = cat.declare(own, verb).await;
    l.upsert("bill", "b", json!({"net": "1"})).await?;

    let before = store.head().await.must("reading the head")?;
    let fed = l
        .ingest(
            "erp",
            vec![source_record("bill", "b", json!({"gross": "5"}))],
        )
        .await;
    ensure!(
        fed.is_err(),
        "a source record setting the derived `gross` must be refused; the ingest answered {fed:?}"
    );
    head_unmoved(store.as_ref(), before, "ingesting a derived field").await?;

    // A verb that sets a derived field may be refused when declared, or
    // when called; either way no value lands.
    if declared.is_ok() {
        let before = store.head().await.must("reading the head")?;
        let called = cat
            .call(
                own,
                &"inflate".into(),
                Some(&"b".into()),
                patch(json!({})),
                Default::default(),
            )
            .await;
        ensure!(
            called.is_err(),
            "a verb setting the derived `gross` must be refused; the call answered {called:?}"
        );
        head_unmoved(store.as_ref(), before, "a verb setting a derived field").await?;
    }
    let r = l.read("bill", "b").await?;
    ensure!(
        decimal(value(&r, "gross")) == Some(dec("2")),
        "`gross` must still be the fold's net * 2 = 2; it reads {:?}",
        value(&r, "gross")
    );
    Ok(())
}

async fn a_derived_field_reading_a_function_is_recomputed_when_the_function_changes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let function = |expr: &str| -> Result<Declaration, Failure> {
        serde_json::from_value(json!({
            "declare": "function", "name": "scaled", "backing": "expr",
            "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
            "expr": expr
        }))
        .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))
    };
    l.declare_as(function("net * 2")?, "declaring `scaled` as net * 2")
        .await?;
    let scaled = computed_by("scaled", StorageKind::Decimal, "scaled", &["net"]);
    l.declare(type_decl(
        "bill",
        vec![field("net", StorageKind::Decimal), scaled],
    ))
    .await?;
    l.upsert("bill", "b", json!({"net": "3"})).await?;
    let r = l.read("bill", "b").await?;
    ensure!(
        decimal(value(&r, "scaled")) == Some(dec("6")),
        "`scaled` must be computed by `scaled` (net * 2) as 6; it reads {:?}",
        value(&r, "scaled")
    );

    l.declare_as(function("net * 3")?, "declaring `scaled` as net * 3")
        .await?;
    let r = l.read("bill", "b").await?;
    let meta = r
        .meta
        .get("scaled")
        .and_then(|m| m.procedure_version.clone());
    ensure!(
        decimal(value(&r, "scaled")) == Some(dec("9")) && meta.as_deref() == Some("2"),
        "after `scaled` became net * 3 (version 2), the record's `scaled` must be \
         recomputed to 9 and name procedure version 2; it reads {:?} with procedure \
         version {meta:?}",
        value(&r, "scaled")
    );
    Ok(())
}
