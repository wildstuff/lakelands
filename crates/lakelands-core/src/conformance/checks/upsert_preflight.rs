//! A record write is judged without being written (lakelands#1329).
//!
//! `upsert_preflight` is `upsert`'s own decision, at the head, appending
//! nothing: it answers `Ok` for a write that would land and the refusal
//! the write would give for one that would not, a broken constraint among
//! them, judged on the record the patch makes of the live one with what
//! the write fills in: initial values, derived fields, a setting's group.

use lakelands_api::principal::WriteContext;

use crate::conformance::prelude::*;

check!(
    UPSERT_JUDGED_AS_WRITTEN = an_upsert_is_judged_as_it_would_be_written,
    "lakelands#1329: `upsert_preflight` answers what `upsert` of the same patch by the same principal then answers: `Ok` for a write that lands, and the same refusal for one that breaks a constraint, one sent by a writer kept from what the constraint reads, one the writer may not write, one that names a field the type does not have, and one at an expected version: its own lands, a stale one and `0` on a record that exists conflict, and `0` on a create lands; it appends nothing and changes no record"
);

check!(
    UPSERT_JUDGED_ON_WHAT_IT_MAKES = an_upsert_is_judged_on_the_record_it_would_make,
    "lakelands#1329: `upsert_preflight` judges a constraint on the record the write would make, as `upsert` does: a create with the initial values it is filled with, a constraint that reads a field derived or computed by a function from what the write sends, and a write of `sys.setting` with the setting's declaration; for each it answers what the write then answers, appending nothing and changing no record"
);

/// A type of invoices whose `paid` never goes beyond `amount`, and
/// whose draft is never above 100: `state` is `draft` where a create
/// leaves it out.
fn invoice() -> Value {
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    json!({"declare": "type", "name": "invoice", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"), number("paid"),
                   {"name": "state", "type": {"kind": "text"}, "initial": "draft"},
                   {"name": "note", "type": {"kind": "text"}}],
        "constraints": [
            {"name": "never_overpaid", "holds": "paid <= amount"},
            {"name": "small_draft", "holds": "not (state = 'draft') or amount <= 100"}]})
}

/// [`judged_at`] with no expected version.
async fn judged(
    store: &dyn LakeStore,
    l: &Lake,
    at: (&Principal, &str, &str),
    fields: Value,
) -> Result<lakelands_api::Result<()>, Failure> {
    judged_at(store, l, at, fields, None).await
}

/// Judges `fields` at `key` of `ty` as `who` through `upsert_preflight`,
/// at the version `expected`; fails unless that appended nothing and left
/// every record of `ty` as it was, then writes them through `upsert` at
/// the same version and fails unless the two answers agree. Answers the
/// judgement.
async fn judged_at(
    store: &dyn LakeStore,
    l: &Lake,
    (who, ty, key): (&Principal, &str, &str),
    fields: Value,
    expected: Option<Version>,
) -> Result<lakelands_api::Result<()>, Failure> {
    let head = store.head().await.must("reading the head")?;
    let held = all(l.cat.as_ref(), &l.own, ty).await?;
    let (t, k) = (TypeName::from(ty), RecordKey::from(key));
    let context = WriteContext::default();
    let sent = patch(fields.clone());
    let judged = (store.upsert_preflight(who, context, &t, &k, sent.clone(), expected)).await;
    let moved = store.head().await.must("reading the head")?;
    ensure!(
        moved == head,
        "judging {fields} at `{ty}` `{key}` moved the head from {head} to {moved}"
    );
    let still = all(l.cat.as_ref(), &l.own, ty).await?;
    ensure!(
        still == held,
        "judging {fields} at `{ty}` `{key}` changed its records: {held:?}, then {still:?}"
    );
    let opts = WriteOpts {
        expected_version: expected,
        ..WriteOpts::default()
    };
    let written = l.cat.upsert(who, &t, &k, sent, opts).await.map(drop);
    ensure!(
        judged == written,
        "{fields} at `{ty}` `{key}` is judged {judged:?} and written {written:?}"
    );
    Ok(judged)
}

/// The constraint a refusal says is broken, if it says one is.
fn broken(a: &lakelands_api::Result<()>) -> Option<&str> {
    match a.as_ref().err().and_then(Error::case).map(|s| &s.case) {
        Some(Case::ConstraintBroken { constraint, .. }) => Some(constraint.as_str()),
        _ => None,
    }
}

async fn an_upsert_is_judged_as_it_would_be_written(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(invoice()).await?;
    l.upsert(
        "invoice",
        "I-1",
        json!({"no": "I-1", "amount": 100, "paid": 50}),
    )
    .await?;
    let s = store.as_ref();
    let at = (&l.own, "invoice", "I-1");
    let lands = judged(s, &l, at, json!({"paid": 80})).await?;
    ensure!(
        lands.is_ok(),
        "a payment within the amount lands: {lands:?}"
    );
    let over = judged(s, &l, at, json!({"paid": 150})).await?;
    ensure!(
        broken(&over) == Some("never_overpaid"),
        "a payment beyond the amount is refused `never_overpaid`: {over:?}"
    );
    // At an expected version: the record's own lands, and then, stale,
    // is refused; `0` lands on a create and is refused on a record.
    let now = l.read("invoice", "I-1").await?.version;
    let current = judged_at(s, &l, at, json!({"paid": 70}), Some(now)).await?;
    let stale = judged_at(s, &l, at, json!({"paid": 60}), Some(now)).await?;
    let fresh = (&l.own, "invoice", "I-9");
    let made = json!({"no": "I-9", "amount": 10, "paid": 0});
    let created = judged_at(s, &l, fresh, made, Some(Version(0))).await?;
    let taken = judged_at(s, &l, at, json!({"paid": 60}), Some(Version(0))).await?;
    let conflict = |a: &lakelands_api::Result<()>| matches!(a, Err(Error::Conflict { .. }));
    ensure!(
        current.is_ok() && conflict(&stale) && created.is_ok() && conflict(&taken),
        "at its own version a write lands, at a stale one it conflicts, and at `0` it lands on a \
         create and conflicts on a record: {current:?}, {stale:?}, {created:?}, {taken:?}"
    );
    let unknown = judged(s, &l, at, json!({"nope": 1})).await?;
    ensure!(
        unknown.is_err(),
        "a field the type lacks is refused: {unknown:?}"
    );
    // Kept from `amount`, which `never_overpaid` reads: a write of `paid`
    // is not granted, whether it would hold or not.
    let rights = [("paid", Access::Write), ("amount", Access::None)];
    let kept = field_access(s, "kept", "invoice", &rights);
    let unread = judged(s, &l, (&kept, "invoice", "I-1"), json!({"paid": 60})).await?;
    let reader = field_access(s, "reader", "invoice", &[]);
    let unwritten = judged(s, &l, (&reader, "invoice", "I-1"), json!({"note": "n"})).await?;
    for (who, a) in [
        ("kept from `amount`", &unread),
        ("who only reads", &unwritten),
    ] {
        let refused = matches!(a, Err(Error::Refused { .. }));
        ensure!(refused, "a writer {who} is refused: {a:?}");
    }
    Ok(())
}

async fn an_upsert_is_judged_on_the_record_it_would_make(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let s = store.as_ref();
    // A create is judged with its initial values: `state` is `draft`.
    l.declare_json(invoice()).await?;
    let big = json!({"no": "I-2", "amount": 150, "paid": 0});
    let draft = judged(s, &l, (&l.own, "invoice", "I-2"), big).await?;
    let open = json!({"no": "I-3", "amount": 150, "paid": 0, "state": "open"});
    let opened = judged(s, &l, (&l.own, "invoice", "I-3"), open).await?;
    ensure!(
        broken(&draft) == Some("small_draft") && opened.is_ok(),
        "a create above 100 is refused `small_draft` as a draft by its initial value and lands \
         as an open one: {draft:?}, {opened:?}"
    );
    // A constraint that reads a field computed from what the write sends.
    let owe = json!({"declare": "function", "name": "owe", "backing": "expr", "expr": "amount - paid",
                     "inputs": [{"name": "amount", "type": {"kind": "number"}},
                                {"name": "paid", "type": {"kind": "number"}}]});
    l.declare_json(owe).await?;
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    for (ty, how) in [
        ("by_derived", json!({"derived": "amount - paid"})),
        ("by_function", json!({"function": "owe"})),
    ] {
        let mut rest = json!({"name": "rest", "type": {"kind": "number"},
                              "authority": "derived", "depends_on": ["amount", "paid"]});
        if let (Some(f), Some(how)) = (rest.as_object_mut(), how.as_object()) {
            f.extend(how.clone());
        }
        l.declare_json(json!({"declare": "type", "name": ty, "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"), number("paid"), rest],
            "constraints": [{"name": "covered", "holds": "rest >= 0"}]}))
            .await?;
        l.upsert(ty, "L-1", json!({"no": "L-1", "amount": 100, "paid": 50}))
            .await?;
        let over = judged(s, &l, (&l.own, ty, "L-1"), json!({"paid": 150})).await?;
        let within = judged(s, &l, (&l.own, ty, "L-1"), json!({"paid": 90})).await?;
        ensure!(
            broken(&over) == Some("covered") && within.is_ok(),
            "`{ty}`: a payment that leaves `rest` below zero is refused `covered` and one that \
             does not lands: {over:?}, {within:?}"
        );
    }
    // A setting is judged by its declaration.
    let vat = json!({"declare": "setting", "key": "vat_rate", "type": {"kind": "decimal"}});
    l.declare_json(vat).await?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let at = (&l.own, ty, "vat_rate");
    let wrong = judged(s, &l, at, json!({"key": "vat_rate", "value": "19%"})).await?;
    let group = json!({"key": "vat_rate", "value": "0.2", "group": "store"});
    let grouped = judged(s, &l, at, group).await?;
    let set = judged(s, &l, at, json!({"key": "vat_rate", "value": "0.19"})).await?;
    ensure!(
        wrong.is_err() && grouped.is_err() && set.is_ok(),
        "a setting is refused a value of another kind and another group, and takes one of its \
         kind: {wrong:?}, {grouped:?}, {set:?}"
    );
    Ok(())
}
