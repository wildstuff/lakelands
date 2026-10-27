//! A field holds the writer that made its record
//! (`docs/decisions/084-a-field-holds-the-writer-that-made-its-record.md`,
//! lakelands#1488): the store fills it at a create, no door writes it,
//! and a migration that brings it fills each live record with its maker.

use lakelands_api::write::{Change, ChangeTo};

use super::only::{de, holder};
use super::paths::accept;
use crate::conformance::fixture::{ingest, source_record, upsert};
use crate::conformance::prelude::*;

check!(
    WRITER_FILLED = a_record_holds_the_writer_that_made_it,
    "084 (lakelands#1488): a field of `authority: writer` takes, at the event that makes its record, that event's writer as `<kind>:<id>`, by `upsert` and by `ingest`, a required one passing the door; its provenance is `origin: system` at that event, naming that writer, with no source; a later write by another writer keeps it, a record retired and made again by another takes the new writer, an accepted proposal's create its proposer, and a re-key moves it with its record; a rebuild and a read as of a position answer every record byte for byte as before"
);

check!(
    WRITER_REFUSED = no_door_writes_the_writer_of_a_record,
    "084 (lakelands#1488): a write of such a field is refused `wrong_door` naming it, appending nothing, at `upsert`, `ingest`, a verb's `set`, a proposed `upsert` and the `plan` of a change to records; an aging rule naming it is refused `aging_authority`; `declare` refuses it on a view, a relation's field and a verb's payload (`writer_not_of_type`), on a field not plain `text` (`writer_not_text`), on a key field (`writer_key`), on a derived one (`writer_computed`) and with an `initial` (`initial_not_authored`); `describe` names its authority `writer` and asks no create for it, and a holder whose `only` names it with its own writer is told `upsert` and creates, one naming another writer is told none"
);

check!(
    WRITER_MIGRATED = a_migration_fills_the_writer_of_each_live_record,
    "084 (lakelands#1488): declaring a field `authority: writer`, added so or changed to it, is refused `reshapes` naming `field_writer`, and the schema hash moves; a migration fills each live record with the writer its provenance names for the event that made it, dropping what the field held, a record that tells none (every field written again since) with `system:lakelands`, never the migration's author, and leaves a retired record as it was; a rebuild answers the records byte for byte as before"
);

/// `note`: `code`, `body`, and `by`, the writer that made it (required).
fn note() -> Value {
    json!({"declare": "type", "name": "note", "key": ["code"], "fields": [
        {"name": "code", "type": {"kind": "text"}, "required": true, "authority": "shared"},
        {"name": "body", "type": {"kind": "text"}, "authority": "shared"},
        {"name": "by", "type": {"kind": "text"}, "required": true, "authority": "writer"}]})
}

/// `decl`, a type's declaration as JSON, as the type it declares.
fn ty(decl: &Value) -> TypeDecl {
    let mut decl = decl.clone();
    decl.as_object_mut().map(|d| d.remove("declare"));
    de(decl)
}

fn by(r: &Record) -> Value {
    r.fields.get("by").cloned().unwrap_or(Value::Null)
}

/// Every record of `ty`, with its provenance, at `at` (the head when
/// `None`), as one JSON text.
async fn every(l: &Lake, ty: &str, at: Option<Position>) -> Result<String, Failure> {
    let mut all = l.all_found(ty, find_at(at)).await?;
    all.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(serde_json::to_string(&all).expect("records serialise"))
}

async fn a_record_holds_the_writer_that_made_it(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let ann = holder(s, "ann", json!({"default": "write"}));
    let bob = holder(s, "bob", json!({"default": "write"}));
    l.declare_json(note()).await?;
    upsert(l.cat.as_ref(), &ann, "note", "n1", json!({"code": "n1"})).await?;
    let fed = source_record("note", "n2", json!({"code": "n2"}));
    l.ingest("erp", vec![fed]).await.must("ingesting n2")?;
    let (n1, n2) = (l.read("note", "n1").await?, l.read("note", "n2").await?);
    let made = n1.meta.get("code").map(|m| m.position);
    let meta = n1.meta.get("by");
    ensure!(
        (by(&n1), by(&n2)) == (json!("agent:ann"), json!(l.own.writer()))
            && meta.is_some_and(|m| m.origin == Origin::System
                && m.writer == "agent:ann"
                && Some(m.position) == made
                && m.source.is_none()),
        "a create by `ann`'s `upsert` and by the owner's `ingest` holds its writer, its \
         provenance the store's at the make naming it: {n1:?}, {n2:?}"
    );
    // A later write keeps it; a re-made record takes its new maker.
    upsert(l.cat.as_ref(), &bob, "note", "n1", json!({"body": "x"})).await?;
    upsert(l.cat.as_ref(), &ann, "note", "n3", json!({"code": "n3"})).await?;
    l.retire("note", "n3").await?;
    upsert(l.cat.as_ref(), &bob, "note", "n3", json!({"code": "n3"})).await?;
    // An accepted proposal's create is its proposer's.
    let write = ProposedWrite::Upsert {
        ty: "note".into(),
        key: "n4".into(),
        patch: patch(json!({"code": "n4"})),
        opts: WriteOpts::default(),
    };
    let proposed = l.cat.propose(&bob, write, "a note".into()).await;
    let Ok(Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("`bob` proposes `n4`: {proposed:?}")));
    };
    accept(&l, &l.own, &proposal).await.must("accepting n4")?;
    let [n1, n3, n4] = [
        l.read("note", "n1").await?,
        l.read("note", "n3").await?,
        l.read("note", "n4").await?,
    ];
    ensure!(
        (by(&n1), by(&n3), by(&n4)) == (json!("agent:ann"), json!("agent:bob"), json!("agent:bob")),
        "`bob`'s write keeps `ann` on n1, `bob` re-making n3 holds `bob`, and the owner's \
         accept of `bob`'s proposal holds `bob`: {n1:?}, {n3:?}, {n4:?}"
    );
    // A re-key moves it with its record.
    let mut rekeyed = ty(&note());
    rekeyed.key = vec!["body".into()];
    rekeyed.fields[1].required = true;
    for (k, body) in [("n2", "b2"), ("n3", "b3"), ("n4", "b4")] {
        l.upsert("note", k, json!({"body": body})).await?;
    }
    l.migrate(rekeyed).await?;
    let moved = l.read("note", "x").await?;
    ensure!(
        by(&moved) == json!("agent:ann"),
        "n1, re-keyed to `x`, still holds `ann`: {moved:?}"
    );
    // A rebuild and a read as of a position answer the same.
    let at = s.head().await.must("the head")?;
    let before = every(&l, "note", None).await?;
    store.rebuild().await.must("rebuilding the lake")?;
    let (after, then) = (
        every(&l, "note", None).await?,
        every(&l, "note", Some(at)).await?,
    );
    ensure!(
        before == after && before == then,
        "a rebuild and a read as of the head answer the records as before: {before} / {after} / \
         {then}"
    );
    Ok(())
}

async fn no_door_writes_the_writer_of_a_record(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_json(note()).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "sign", "on": "note", "risk": "low",
        "effect": "write", "set": [{"field": "by", "value": "'agent:x'"}]}),
    )
    .await?;
    l.upsert("note", "n1", json!({"code": "n1"})).await?;
    let before = s.head().await.must("the head")?;
    let forged = json!({"by": "agent:x"});
    let up = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "note",
        "n2",
        json!({"code": "n2", "by": "agent:x"}),
    )
    .await
    .map(drop);
    let fed = source_record("note", "n2", json!({"code": "n2", "by": "agent:x"}));
    let fed = ingest(l.cat.as_ref(), &l.own, "erp", vec![fed])
        .await
        .map(drop);
    let (verb, key) = ("sign".into(), "n1".into());
    let signed = l.cat.call(
        &l.own,
        &verb,
        Some(&key),
        patch(json!({})),
        WriteOpts::default(),
    );
    let signed = signed.await.map(drop);
    let write = ProposedWrite::Upsert {
        ty: "note".into(),
        key: "n1".into(),
        patch: patch(forged.clone()),
        opts: WriteOpts::default(),
    };
    let proposed = l
        .cat
        .propose(&l.own, write, "forged".into())
        .await
        .map(drop);
    let change = Change::Records {
        ty: "note".into(),
        filter: Box::new("code = 'n1'".into()),
        to: ChangeTo::Set(de(forged)),
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let planned = l.cat.plan(&l.own, change).await.map(drop);
    let wrong = Err(Error::Refused {
        field: Some("by".into()),
        reason: Refusal::WrongDoor,
    });
    ensure!(
        [&up, &fed, &signed, &proposed, &planned]
            .iter()
            .all(|a| **a == wrong),
        "every door refuses `by` `wrong_door`: {:?}",
        [&up, &fed, &signed, &proposed, &planned]
    );
    head_unmoved(s, before, "a write of the writer of a record").await?;
    let rule = json!({"on": "note", "field": "by", "from": "a", "to": "b", "when": "true"});
    let aging = try_upsert(l.cat.as_ref(), &l.own, "sys.aging_rule", "r", rule).await;
    ensure!(
        matches!(invalid_case(&aging), Some(Case::AgingAuthority { .. })),
        "an aging rule moving `by` is refused `aging_authority`: {aging:?}"
    );
    // What the declaration refuses.
    let writer = |f: Value| {
        let mut f = f;
        f["authority"] = json!("writer");
        f
    };
    let text = json!({"name": "w", "type": {"kind": "text"}});
    let typed = |fields: Vec<Value>, more: Value| {
        let mut t = json!({"declare": "type", "name": "bad", "key": ["k"], "fields": fields});
        t.as_object_mut()
            .expect("an object")
            .extend(more.as_object().cloned().unwrap_or_default());
        t
    };
    let k = json!({"name": "k", "type": {"kind": "text"}});
    let cases = [
        (
            typed(
                vec![k.clone()],
                json!({"relations": [{"name": "r", "target": "note",
                "cardinality": "many_to_many", "fields": [writer(text.clone())]}]}),
            ),
            Case::WriterNotOfType,
        ),
        (
            json!({"declare": "verb", "name": "bad", "on": "note", "risk": "low",
                "payload": [writer(text.clone())], "effect": "write",
                "set": [{"field": "body", "value": "payload.w"}]}),
            Case::WriterNotOfType,
        ),
        (
            json!({"declare": "type", "name": "bad", "fields": [writer(text.clone())],
                "view": {"source": "note", "group_by": ["code"], "measures": []}}),
            Case::WriterNotOfType,
        ),
        (
            typed(
                vec![
                    k.clone(),
                    writer(json!({"name": "w", "type": {"kind": "number"}})),
                ],
                json!({}),
            ),
            Case::WriterNotText,
        ),
        (typed(vec![writer(k.clone())], json!({})), Case::WriterKey),
        (
            typed(
                vec![
                    k.clone(),
                    writer(json!({"name": "w", "type": {"kind": "text"}, "derived": "k"})),
                ],
                json!({}),
            ),
            Case::WriterComputed,
        ),
        (
            typed(
                vec![
                    k.clone(),
                    writer(json!({"name": "w", "type": {"kind": "text"}, "initial": "a"})),
                ],
                json!({}),
            ),
            Case::InitialNotAuthored,
        ),
    ];
    for (decl, case) in cases {
        let said = l.cat.declare(&l.own, de(decl.clone())).await;
        let got = invalid_case(&said).cloned();
        ensure!(
            got == Some(case.clone()),
            "`declare` refuses {decl} as {case:?}: {said:?}"
        );
    }
    // `describe` tells it and asks no create for it; `only` on it.
    let d = l.describe_type("note").await?;
    let f = d.fields.iter().find(|f| f.name.as_str() == "by");
    ensure!(
        f.is_some_and(|f| f.authority == Authority::Writer
            && !f.required_at_create
            && !f.authored_writable())
            && d.may.create,
        "`describe` names `by` `writer`, neither asked nor written at a create, and a create \
         open: {d:?}"
    );
    let only = |who: &str| json!({"default": "write", "types": {"note": {"only": {"by": who}}}});
    let (mine, theirs) = (
        holder(s, "mine", only("agent:mine")),
        holder(s, "ours", only("agent:x")),
    );
    let told = async |who: &Principal| {
        let d = describe_type(l.cat.as_ref(), who, "note").await;
        d.map(|d| d.may.upsert)
    };
    let (yes, no) = (told(&mine).await?, told(&theirs).await?);
    let made = try_upsert(l.cat.as_ref(), &mine, "note", "m1", json!({"code": "m1"})).await;
    ensure!(
        yes && !no && made.is_ok(),
        "a holder of `only` `by` = its own writer is told `upsert` ({yes}) and creates \
         ({made:?}); one of another writer is told none ({no})"
    );
    Ok(())
}

async fn a_migration_fills_the_writer_of_each_live_record(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let ann = holder(s, "ann", json!({"default": "write"}));
    let bob = holder(s, "bob", json!({"default": "write"}));
    let mut memo = json!({"declare": "type", "name": "memo", "fields": [
        {"name": "body", "type": {"kind": "text"}},
        {"name": "by", "type": {"kind": "text"}}]});
    l.declare_json(memo.clone()).await?;
    upsert(
        l.cat.as_ref(),
        &ann,
        "memo",
        "m1",
        json!({"body": "a", "by": "agent:forged"}),
    )
    .await?;
    upsert(l.cat.as_ref(), &bob, "memo", "m2", json!({"body": "b"})).await?;
    upsert(l.cat.as_ref(), &ann, "memo", "m3", json!({"body": "c"})).await?;
    upsert(l.cat.as_ref(), &bob, "memo", "m3", json!({"body": "d"})).await?;
    upsert(
        l.cat.as_ref(),
        &ann,
        "memo",
        "m4",
        json!({"body": "e", "by": "agent:kept"}),
    )
    .await?;
    l.retire("memo", "m4").await?;
    // Declared, it reshapes; migrated, each live record takes its maker.
    let was = l.describe_type("memo").await?.lifecycle.schema_hash;
    memo["fields"][1]["authority"] = json!("writer");
    memo["fields"][1]["required"] = json!(true);
    let declared = l.cat.declare(&l.own, de(memo.clone())).await;
    let reshapes = matches!(invalid_case(&declared), Some(Case::Reshapes { reasons, .. })
        if reasons.contains(&lakelands_api::error::Revision::FieldWriter { field: "by".into() }));
    ensure!(
        reshapes,
        "declaring `by` `writer` is refused `reshapes` naming it: {declared:?}"
    );
    l.migrate(ty(&memo)).await?;
    let is = l.describe_type("memo").await?.lifecycle.schema_hash;
    let live = l.all_found("memo", find_at(None)).await?;
    let at = |k: &str| {
        let r = live.iter().find(|r| r.key.as_str() == k);
        r.map(|r| {
            (
                by(r),
                r.meta.get("by").map(|m| (m.origin, m.writer.clone())),
            )
        })
    };
    let (m1, m2, m3) = (at("m1"), at("m2"), at("m3"));
    let store_writer = "system:lakelands";
    ensure!(
        was != is
            && m1
                == Some((
                    json!("agent:ann"),
                    Some((Origin::System, "agent:ann".into()))
                ))
            && m2
                == Some((
                    json!("agent:bob"),
                    Some((Origin::System, "agent:bob".into()))
                ))
            && m3
                == Some((
                    json!(store_writer),
                    Some((Origin::System, store_writer.into()))
                )),
        "the migration moves the hash ({was} → {is}), fills m1 with `ann`, dropping the forged \
         value, m2 with `bob`, and m3, all rewritten since, with the store's writer: {m1:?}, \
         {m2:?}, {m3:?}"
    );
    upsert(l.cat.as_ref(), &bob, "memo", "m4", json!({"body": "f"})).await?;
    let m4 = l.read("memo", "m4").await?;
    ensure!(
        live.iter().all(|r| r.key.as_str() != "m4") && by(&m4) == json!("agent:bob"),
        "the retired m4 stays retired and, written again by `bob`, holds `bob`: {m4:?}"
    );
    let before = every(&l, "memo", None).await?;
    store.rebuild().await.must("rebuilding the lake")?;
    let after = every(&l, "memo", None).await?;
    ensure!(
        before == after,
        "a rebuild answers the records as before: {before} / {after}"
    );
    Ok(())
}
