//! An authored field that admits a named source
//! (`docs/decisions/069-an-authored-field-may-admit-a-named-source.md`):
//! `ingest` writes it under that source alone, and only where no other
//! hand's value stands; the values carry the source's provenance, so its
//! purge and its removal take them; closing the admission is a
//! declaration, and the rows written before keep what they hold.

use lakelands_api::error::{Case, Place, RecordName};
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::write::Selection;
use lakelands_api::{Access, Error, Origin, Position, Principal, Refusal, SourceId, Timestamp};
use serde_json::{Value, json};

use super::removed::{at, erased};
use crate::conformance::fixture::{
    Lake, Must, head_unmoved, ingest, invalid_case, invalid_said, principal, read_at,
    source_record, upsert,
};
use crate::conformance::{Failure, Held, Store};

check!(
    ADMITTED = an_admitted_source_writes_an_authored_field_and_no_other_source_does,
    "069: an authored field that admits `samples` takes `ingest` under `samples`, its value carrying `origin: source` and the source, and under another source's name the record is refused `wrong_door` on the field and its window appends nothing, a source field it also carries included; a record that names `samples` as its own source in another source's delivery is written; any principal holding `ingest` may deliver under the name; an author's `upsert` still writes the field; `describe` shows each field's `admits` in the order declared, and none where it admits none"
);

check!(
    ADMITS_DECLARED = admits_is_declared_on_an_authored_field_of_a_type_alone,
    "069: `declare` refuses `admits` on a field that is `shared`, `source`, `derived` or `verb` (`admits_not_authored`), on a relation's field, a verb's payload, a view's field and a function's input (`admits_not_of_type`), on a type that keeps no per-field provenance (`admits_no_provenance`), with a source id that is not formed as one (`source_id_form`) and with one named twice (`admits_twice`), each at the field's `admits` and appending nothing; admitting a source and closing it again each move the type's version and leave its schema hash as it was"
);

check!(
    OWN_VALUE = an_admitted_source_writes_only_where_no_other_hands_value_stands,
    "069: on an authored field an admitted source writes only where no hand wrote it, where that source wrote it last, or where it holds its `initial` on a record the source made, though it wrote that record's other values again since: onto the `initial` of a record an author made it is refused `wrong_door`; its redelivery onto a field an author wrote since, or cleared, is refused `wrong_door` on the field, appending nothing, and the author's value stands; so is a second admitted source onto the first's value, and the first onto the second's; the source's own value is written again, cleared, and written once more"
);

check!(
    ADMITTED_TAKEN = an_admitted_sources_rows_are_taken_back_as_any_of_its_values,
    "069 with 039 and 065: the purge of an admitted source retires the rows it brought in and passes over one an author wrote over whole; a row an author edited in part, and a row an author made holding a value of a field with an `initial` alone, one of whose empty fields the source filled, refuse the purge and its preflight whole, naming those two, appending nothing; `remove_values` of the source takes its values from that row and keeps the author's, retires the row it alone wrote, and passes over the row the author wrote over whole and a row an author made, whose `initial` the source was refused on; from a row an author made holding a value of a field with an `initial` alone, it takes the source's value and keeps the row"
);

check!(
    CLOSED = closing_an_admission_refuses_the_source_and_keeps_its_rows,
    "069: declaring the field again without the source refuses the source on it from that position on, a redelivery of a row it wrote included, appending nothing; the rows written before keep their values and their provenance, at the head and after a rebuild; `describe` as of before the close shows the admission and at the head shows none; the source's purge still takes the rows; admitting it again lets it write once more"
);

check!(
    ADMITTED_KEY = an_authored_key_field_admits_the_source_or_refuses_its_creates,
    "069 with 065: on a type keyed by an authored field, a create by a source the key field does not admit is refused `wrong_door` on the key field, appending nothing, and once it admits the source is written; on a record an author made holding its key alone, the source writes an empty field it is admitted to; its purge and the preflight are refused naming that record; `remove_values` of the source erases its value there and keeps the record, and retires the record the source made"
);

check!(
    ADMITTED_FORGOTTEN = forgetting_an_admitted_source_retires_the_rows_it_alone_brought,
    "069 with 058: forgetting an admitted source retires the rows it alone brought, keeps a row an author edited in part, the source's value there included, since an authored field is never erasable, and a row an author made holding a value of a field with an `initial` alone, one of whose empty fields the source filled, with that value; the preflight names both co-owned; it leaves a row an author made, whose `initial` the source was refused on; after it an ingest under the source is refused"
);

/// `item`, keyless: `title` admits `samples`, `tag` admits `samples` and
/// `demo`, `status` admits `samples` and starts `draft`, `note` admits
/// none, `size` is a source's.
fn item(title: &[&str]) -> Value {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    json!({"declare": "type", "name": "item", "field_meta": true, "fields": [
        {"name": "title", "type": {"kind": "text"}, "admits": title},
        {"name": "tag", "type": {"kind": "text"}, "admits": ["samples", "demo"]},
        {"name": "status", "type": {"kind": "text"}, "admits": ["samples"], "initial": "draft"},
        text("note"),
        {"name": "size", "type": {"kind": "number"}, "authority": "source"}]})
}

async fn lake(store: &Store) -> Result<Lake, Failure> {
    let l = Lake::of(store.as_ref());
    l.declare_json(item(&["samples"])).await?;
    Ok(l)
}

/// An agent that writes every field and holds `ingest`.
fn loader(store: &Store, id: &str) -> Principal {
    let grants = Grants {
        default: Access::Write,
        ingest: true,
        ..Grants::default()
    };
    principal(store.as_ref(), PrincipalKind::Agent, id, grants)
}

/// Ingests one `item` record at `key` as `who` under `source`.
async fn feed(
    l: &Lake,
    who: &Principal,
    source: &str,
    key: &str,
    fields: Value,
) -> lakelands_api::Result<()> {
    let records = vec![source_record("item", key, fields)];
    ingest(l.cat.as_ref(), who, source, records).await.map(drop)
}

fn wrong_door(field: &str) -> Error {
    Error::Refused {
        field: Some(field.into()),
        reason: Refusal::WrongDoor,
    }
}

/// `field` of `item` `key` at the head: its value, and the source its
/// provenance names, if any.
async fn held(l: &Lake, key: &str, field: &str) -> Result<(Value, Option<String>), Failure> {
    let r = at(l, "item", key, None).await?;
    let r = r.ok_or_else(|| Failure::new(format!("`item` `{key}` is not live")))?;
    let value = r.fields.get(field).cloned().unwrap_or(Value::Null);
    let from = (r.meta.get(field))
        .filter(|m| m.origin == Origin::Source)
        .and_then(|m| m.source.as_ref().map(|s| s.as_str().to_owned()));
    Ok((value, from))
}

/// What `describe` shows of each field's `admits`, at `when`.
async fn admits_at(
    l: &Lake,
    when: Option<Position>,
) -> Result<Vec<(String, Vec<String>)>, Failure> {
    let ty = "item".into();
    let d = l.cat.describe_type(&l.own, &ty, read_at(when));
    let d = d.await.must("describing `item`")?.data;
    let of = |f: &lakelands_api::describe::FieldDescription| {
        let admits = f.admits.iter().map(|s| s.as_str().to_owned()).collect();
        (f.name.as_str().to_owned(), admits)
    };
    Ok(d.fields.iter().map(of).collect())
}

async fn an_admitted_source_writes_an_authored_field_and_no_other_source_does(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let (a, b) = (loader(&store, "a"), loader(&store, "b"));
    let applied = feed(&l, &a, "samples", "s1", json!({"title": "Lamp"})).await;
    ensure!(
        applied.is_ok()
            && held(&l, "s1", "title").await? == (json!("Lamp"), Some("samples".into())),
        "`a` under `samples` writes `title`, its provenance naming `samples`: {applied:?}"
    );
    // Another source's name is refused on the field, the record whole and
    // its window with it.
    let before = s.head().await.must("reading the head")?;
    let records = vec![
        source_record("item", "f1", json!({"size": 1})),
        source_record("item", "f2", json!({"title": "Desk", "size": 2})),
    ];
    let refused = ingest(l.cat.as_ref(), &b, "feed", records).await;
    ensure!(
        refused.as_ref().err() == Some(&wrong_door("title")),
        "`b` under `feed` is refused `wrong_door` on `title`: {refused:?}"
    );
    head_unmoved(s, before, "an ingest of `title` under `feed`").await?;
    ensure!(
        at(&l, "item", "f1", None).await?.is_none(),
        "the refused window wrote `f1`"
    );
    // A record naming `samples` as its own source is written, and so is
    // the name given by any holder of `ingest`.
    let mut own = source_record("item", "s2", json!({"title": "Desk"}));
    own.source = Some(SourceId::from("samples"));
    let named = ingest(l.cat.as_ref(), &b, "feed", vec![own]).await;
    let claimed = feed(&l, &b, "samples", "s3", json!({"title": "Shelf"})).await;
    ensure!(
        named.is_ok()
            && claimed.is_ok()
            && held(&l, "s2", "title").await?.1.as_deref() == Some("samples")
            && held(&l, "s3", "title").await?.1.as_deref() == Some("samples"),
        "a record naming `samples` in `feed`'s delivery, and `b` under `samples`, write \
         `title`: {named:?}, {claimed:?}"
    );
    // An author writes it as before.
    upsert(
        l.cat.as_ref(),
        &l.own,
        "item",
        "a1",
        json!({"title": "Chair"}),
    )
    .await?;
    ensure!(
        held(&l, "a1", "title").await? == (json!("Chair"), None),
        "an author's `upsert` writes `title`"
    );
    let shown = admits_at(&l, None).await?;
    let want = |f: &str, a: &[&str]| (f.to_owned(), a.iter().map(|s| s.to_string()).collect());
    let expected = vec![
        want("title", &["samples"]),
        want("tag", &["samples", "demo"]),
        want("status", &["samples"]),
        want("note", &[]),
        want("size", &[]),
    ];
    ensure!(
        shown == expected,
        "`describe` shows each field's `admits` as declared: {shown:?}"
    );
    Ok(())
}

async fn admits_is_declared_on_an_authored_field_of_a_type_alone(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_json(item(&[])).await?;
    let ty = |field: Value| json!({"declare": "type", "name": "odd", "fields": [field]});
    let admitting = |authority: &str| {
        json!({"name": "x", "type": {"kind": "text"}, "authority": authority,
               "admits": ["samples"]})
    };
    let link = json!({"declare": "type", "name": "odd", "fields": [], "relations": [
        {"name": "of", "target": "item", "cardinality": "many_to_one",
         "fields": [{"name": "x", "type": {"kind": "text"}, "admits": ["samples"]}]}]});
    let payload = json!({"declare": "verb", "name": "mark", "on": "item", "risk": "low",
        "effect": "write", "set": [{"field": "note", "value": "'x'"}],
        "payload": [{"name": "x", "type": {"kind": "text"}, "admits": ["samples"]}]});
    let x = json!({"name": "x", "type": {"kind": "text"}, "admits": ["samples"]});
    let view = json!({"declare": "type", "name": "odd", "key": ["x"], "fields": [x],
        "view": {"source": "item", "group_by": ["x"], "measures": []}});
    let input = json!({"declare": "function", "name": "odd", "backing": "expr",
        "inputs": [x], "expr": "upper(x)"});
    let authored =
        |admits: Value| ty(json!({"name": "x", "type": {"kind": "text"}, "admits": admits}));
    let cases = [
        (ty(admitting("shared")), Case::AdmitsNotAuthored),
        (ty(admitting("source")), Case::AdmitsNotAuthored),
        (ty(admitting("verb")), Case::AdmitsNotAuthored),
        (
            ty(
                json!({"name": "x", "type": {"kind": "number"}, "authority": "derived",
                      "derived": "1", "admits": ["samples"]}),
            ),
            Case::AdmitsNotAuthored,
        ),
        (link, Case::AdmitsNotOfType),
        (
            json!({"declare": "type", "name": "odd", "field_meta": false, "fields": [
                {"name": "x", "type": {"kind": "text"}, "admits": ["samples"]}]}),
            Case::AdmitsNoProvenance,
        ),
        (payload, Case::AdmitsNotOfType),
        (view, Case::AdmitsNotOfType),
        (input, Case::AdmitsNotOfType),
        (authored(json!(["samples", ""])), Case::SourceIdForm),
        (
            authored(json!(["samples", "demo", "samples"])),
            Case::AdmitsTwice {
                source: "samples".into(),
            },
        ),
    ];
    let before = s.head().await.must("reading the head")?;
    for (decl, case) in cases {
        let d = serde_json::from_value(decl.clone()).map_err(|e| Failure::new(e.to_string()))?;
        let e = l.cat.declare(&l.own, d).await;
        let said = invalid_said(&e);
        ensure!(
            said.is_some_and(|s| s.case == case && s.at.last() == Some(&Place::Admits)),
            "{decl} is refused `{case:?}` at its `admits`: {e:?}"
        );
    }
    head_unmoved(s, before, "a declaration of `admits` it refuses").await?;
    // Admitting and closing move the version, not the hash.
    let lifecycle = |d: lakelands_api::describe::TypeDescription| d.lifecycle;
    let at_head = || l.describe_type("item");
    let closed = lifecycle(at_head().await?);
    l.declare_json(item(&["samples"])).await?;
    let open = lifecycle(at_head().await?);
    l.declare_json(item(&[])).await?;
    let again = lifecycle(at_head().await?);
    ensure!(
        (open.version, again.version) == (closed.version + 1, closed.version + 2)
            && open.schema_hash == closed.schema_hash
            && again.schema_hash == closed.schema_hash,
        "admitting and closing move `item`'s version and keep its hash: {closed:?}, {open:?}, \
         {again:?}"
    );
    Ok(())
}

async fn an_admitted_source_writes_only_where_no_other_hands_value_stands(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let own = l.own.clone();
    feed(
        &l,
        &own,
        "samples",
        "s1",
        json!({"title": "Lamp", "tag": "x"}),
    )
    .await
    .must("`samples` bringing `s1`")?;
    for (key, title) in [("s2", "Desk"), ("s3", "Bin")] {
        let fed = feed(&l, &own, "samples", key, json!({"title": title})).await;
        fed.must(&format!("`samples` bringing `{key}`"))?;
    }
    // An author writes over `s1`'s `title`, clears `s3`'s and makes `a1`.
    let cat = l.cat.as_ref();
    for (key, title) in [
        ("s1", json!("Lamp II")),
        ("s3", Value::Null),
        ("a1", json!("Chair")),
    ] {
        upsert(cat, &own, "item", key, json!({"title": title})).await?;
    }
    // Each write onto another hand's value is refused on its field.
    let before = s.head().await.must("reading the head")?;
    let onto = [
        ("samples", "s1", json!({"title": "Lamp"}), "title"),
        ("samples", "s3", json!({"title": "Bin"}), "title"),
        ("demo", "s1", json!({"tag": "y"}), "tag"),
        ("samples", "a1", json!({"title": "Stool"}), "title"),
        ("samples", "a1", json!({"status": "listed"}), "status"),
    ];
    for (source, key, fields, field) in onto {
        let e = feed(&l, &own, source, key, fields.clone()).await;
        ensure!(
            e.as_ref().err() == Some(&wrong_door(field)),
            "`{source}` writing {fields} onto `{key}` is refused `wrong_door` on `{field}`: {e:?}"
        );
    }
    head_unmoved(s, before, "a source's write onto another hand's value").await?;
    ensure!(
        held(&l, "s1", "title").await? == (json!("Lamp II"), None)
            && held(&l, "a1", "status").await? == (json!("draft"), None),
        "the author's `title` of `s1` and the `initial` of `a1` stand"
    );
    // A field with no value, or its `initial` on a record the source made,
    // is written; then it is that source's, and the other admitted source
    // is refused on it.
    feed(&l, &own, "demo", "s2", json!({"tag": "d"}))
        .await
        .must("`demo` writing the empty `tag` of `s2`")?;
    // The source's own record stays its own when it writes again.
    feed(&l, &own, "samples", "s2", json!({"title": "Desk II"}))
        .await
        .must("`samples` writing `s2`'s `title` again")?;
    feed(&l, &own, "samples", "s2", json!({"status": "listed"}))
        .await
        .must("`samples` writing `s2`'s `status`, its `initial`")?;
    let before = s.head().await.must("reading the head")?;
    let e = feed(&l, &own, "samples", "s2", json!({"tag": "s"})).await;
    ensure!(
        e.as_ref().err() == Some(&wrong_door("tag"))
            && held(&l, "s2", "tag").await? == (json!("d"), Some("demo".into()))
            && held(&l, "s2", "status").await? == (json!("listed"), Some("samples".into())),
        "`samples` onto `demo`'s `tag` of `s2` is refused `wrong_door`: {e:?}"
    );
    head_unmoved(s, before, "`samples` onto `demo`'s value").await?;
    // Its own value: written again, cleared, written once more.
    for (value, what) in [
        (json!("y"), "again"),
        (Value::Null, "cleared"),
        (json!("z"), "anew"),
    ] {
        feed(&l, &own, "samples", "s1", json!({"tag": value}))
            .await
            .must(&format!("`samples` writing its own `tag` of `s1` {what}"))?;
        let (got, from) = held(&l, "s1", "tag").await?;
        ensure!(
            got == value && (value.is_null() || from.as_deref() == Some("samples")),
            "`samples`' own `tag` of `s1` {what}: {got:?} from {from:?}"
        );
    }
    Ok(())
}

async fn an_admitted_sources_rows_are_taken_back_as_any_of_its_values(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let own = l.own.clone();
    for (source, key, fields) in [
        ("samples", "s1", json!({"title": "Lamp"})),
        ("samples", "s2", json!({"title": "Desk"})),
        ("samples", "s3", json!({"title": "Shelf"})),
        ("demo", "d1", json!({"tag": "a"})),
        ("demo", "d2", json!({"tag": "b"})),
    ] {
        let fed = feed(&l, &own, source, key, fields).await;
        fed.must(&format!("`{source}` bringing `{key}`"))?;
    }
    // `s2` edited in part, `s3` and `d2` written over whole.
    upsert(l.cat.as_ref(), &own, "item", "s2", json!({"note": "kept"})).await?;
    upsert(
        l.cat.as_ref(),
        &own,
        "item",
        "s3",
        json!({"title": "My shelf"}),
    )
    .await?;
    upsert(l.cat.as_ref(), &own, "item", "d2", json!({"tag": "mine"})).await?;
    // `a1`, an author's, holds its `initial`, which `samples` may not take;
    // `a2`, an author's holding a value of `status` alone, takes its
    // `title`.
    let cat = l.cat.as_ref();
    upsert(cat, &own, "item", "a1", json!({"title": "Chair"})).await?;
    upsert(cat, &own, "item", "a2", json!({"status": "approved"})).await?;
    feed(&l, &own, "samples", "a2", json!({"title": "Lamp"}))
        .await
        .must("`samples` writing the empty `title` of `a2`")?;
    let e = feed(&l, &own, "samples", "a1", json!({"status": "listed"})).await;
    ensure!(
        e.as_ref().err() == Some(&wrong_door("status")),
        "`samples` writing `a1`'s `status` is refused `wrong_door`: {e:?}"
    );
    // The purge of `demo` takes `d1`, and passes over `d2`.
    let demo = s.purge_source(&SourceId::from("demo"), None, None).await;
    let retired =
        |r: &lakelands_api::write::PurgeReport| r.types.iter().map(|t| t.retired).sum::<u32>();
    ensure!(
        demo.as_ref().map(retired).ok() == Some(1)
            && at(&l, "item", "d1", None).await?.is_none()
            && held(&l, "d2", "tag").await? == (json!("mine"), None),
        "the purge of `demo` retires `d1` alone: {demo:?}"
    );
    // `s2`, edited in part, and the author's `a2` refuse the purge of `samples`.
    let samples = SourceId::from("samples");
    let before = s.head().await.must("reading the head")?;
    let preflight = s.purge_preflight(&samples, None).await;
    let purge = s.purge_source(&samples, None, None).await;
    let named = |e: &lakelands_api::Result<_>| match invalid_case(e) {
        Some(Case::PurgeShared { count, first, .. }) => {
            let keys = first.iter().map(|n| match n {
                RecordName::Of { key, .. } => key.as_str(),
                _ => "",
            });
            *count == 2 && keys.collect::<Vec<_>>() == ["a2", "s2"]
        }
        _ => false,
    };
    ensure!(
        named(&preflight) && named(&purge),
        "the preflight and the purge of `samples` are refused naming `a2` and `s2` alone: \
         {preflight:?}, {purge:?}"
    );
    head_unmoved(s, before, "the purge of `samples`").await?;
    // `remove_values` takes `samples`' values: `s1` retired, `s2`'s `note`
    // kept, `s3` passed over.
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    s.remove_values(&samples, &all, false, None)
        .await
        .must("removing what `samples` wrote")?;
    let s2 = at(&l, "item", "s2", None).await?;
    let s2_kept = s2.as_ref().is_some_and(|r| {
        erased(r, "title", "samples") && r.fields.get("note") == Some(&json!("kept"))
    });
    let a1 = at(&l, "item", "a1", None).await?;
    let a2 = at(&l, "item", "a2", None).await?;
    let a2_kept = a2.as_ref().is_some_and(|r| {
        erased(r, "title", "samples") && r.fields.get("status") == Some(&json!("approved"))
    });
    let a1_kept = a1.as_ref().is_some_and(|r| {
        let initial = r
            .meta
            .get("status")
            .is_some_and(|m| m.origin == Origin::Initial);
        r.fields.get("title") == Some(&json!("Chair"))
            && r.fields.get("status") == Some(&json!("draft"))
            && initial
    });
    ensure!(
        at(&l, "item", "s1", None).await?.is_none()
            && s2_kept
            && a1_kept
            && a2_kept
            && held(&l, "s3", "title").await? == (json!("My shelf"), None),
        "`remove_values` retires `s1`, takes `s2`'s `title` and keeps its `note`, takes `a2`'s \
         `title` and keeps the record, and leaves `s3` and `a1`: {s2:?}, {a1:?}, {a2:?}"
    );
    Ok(())
}

/// Whether `s1` and `s2` hold the `title` `samples` brought, its
/// provenance naming it.
async fn kept(l: &Lake) -> Result<bool, Failure> {
    let s1 = held(l, "s1", "title").await?;
    let s2 = held(l, "s2", "title").await?;
    let samples = Some("samples".into());
    Ok(s1 == (json!("Lamp"), samples.clone()) && s2 == (json!("Desk"), samples))
}

async fn closing_an_admission_refuses_the_source_and_keeps_its_rows(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let own = l.own.clone();
    feed(&l, &own, "samples", "s1", json!({"title": "Lamp"}))
        .await
        .must("`samples` bringing `s1`")?;
    feed(&l, &own, "samples", "s2", json!({"title": "Desk"}))
        .await
        .must("`samples` bringing `s2`")?;
    let opened = s.head().await.must("reading the head")?;
    l.declare_json(item(&[])).await?;
    let before = s.head().await.must("reading the head")?;
    for (key, title) in [("s1", "Lamp"), ("s3", "Shelf")] {
        let e = feed(&l, &own, "samples", key, json!({"title": title})).await;
        ensure!(
            e.as_ref().err() == Some(&wrong_door("title")),
            "`samples` writing `{key}`'s `title` after the close is refused `wrong_door`: {e:?}"
        );
    }
    head_unmoved(s, before, "`samples` after the close").await?;
    ensure!(
        kept(&l).await?,
        "`s1` and `s2` keep `samples`' `title` and its provenance after the close"
    );
    s.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        kept(&l).await?,
        "`s1` and `s2` keep `samples`' `title` and its provenance after a rebuild"
    );
    let title = |shown: Vec<(String, Vec<String>)>| {
        shown
            .into_iter()
            .find(|(f, _)| f == "title")
            .map(|(_, a)| a)
    };
    let (then, now) = (
        admits_at(&l, Some(opened)).await?,
        admits_at(&l, None).await?,
    );
    ensure!(
        title(then.clone()) == Some(vec!["samples".into()]) && title(now.clone()) == Some(vec![]),
        "`describe` shows `title` admitting `samples` before the close and nothing at the head: \
         {then:?}, {now:?}"
    );
    let purge = s.purge_source(&SourceId::from("samples"), None, None).await;
    ensure!(
        at(&l, "item", "s1", None).await?.is_none() && at(&l, "item", "s2", None).await?.is_none(),
        "the purge of `samples` retires `s1` and `s2` after the close: {purge:?}"
    );
    l.declare_json(item(&["samples"])).await?;
    feed(&l, &own, "samples", "s3", json!({"title": "Shelf"}))
        .await
        .must("`samples` writing `s3` once admitted again")?;
    Ok(())
}

/// `product`, keyed by `sku`, an authored field admitting `sku`; `name`
/// admits `samples`.
fn product(sku: &[&str]) -> Value {
    json!({"declare": "type", "name": "product", "key": ["sku"], "field_meta": true, "fields": [
        {"name": "sku", "type": {"kind": "text"}, "admits": sku},
        {"name": "name", "type": {"kind": "text"}, "admits": ["samples"]}]})
}

async fn an_authored_key_field_admits_the_source_or_refuses_its_creates(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_json(product(&[])).await?;
    let own = l.own.clone();
    let made = |key: &str, name: &str| {
        let records = vec![source_record("product", key, json!({"name": name}))];
        ingest(l.cat.as_ref(), &own, "samples", records)
    };
    let before = s.head().await.must("reading the head")?;
    let e = made("p1", "Lamp").await;
    ensure!(
        e.as_ref().err() == Some(&wrong_door("sku")),
        "a create by `samples` is refused `wrong_door` on `sku`, which admits no source: {e:?}"
    );
    head_unmoved(s, before, "a create on a key field that admits no source").await?;
    l.declare_json(product(&["samples"])).await?;
    made("p1", "Lamp")
        .await
        .must("a create by `samples` once `sku` admits it")?;
    // An author's record holding its key alone takes the source's `name`.
    upsert(l.cat.as_ref(), &own, "product", "p2", json!({"sku": "p2"})).await?;
    made("p2", "Desk")
        .await
        .must("`samples` writing the empty `name` of an author's `p2`")?;
    let samples = SourceId::from("samples");
    let before = s.head().await.must("reading the head")?;
    let preflight = s.purge_preflight(&samples, None).await;
    let purge = s.purge_source(&samples, None, None).await;
    let named = |e: &lakelands_api::Result<_>| match invalid_case(e) {
        Some(Case::PurgeShared { count, first, .. }) => {
            *count == 1
                && matches!(&first[..], [RecordName::Of { key, .. }] if key.as_str() == "p2")
        }
        _ => false,
    };
    ensure!(
        named(&preflight) && named(&purge),
        "the preflight and the purge of `samples` are refused naming `p2` alone: \
         {preflight:?}, {purge:?}"
    );
    head_unmoved(s, before, "the purge of `samples`").await?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let report = s.remove_values(&samples, &all, false, None).await;
    let report = report.must("removing what `samples` wrote")?;
    let p2 = at(&l, "product", "p2", None).await?;
    let p2_kept = p2
        .as_ref()
        .is_some_and(|r| erased(r, "name", "samples") && r.fields.get("sku") == Some(&json!("p2")));
    ensure!(
        at(&l, "product", "p1", None).await?.is_none() && p2_kept,
        "`remove_values` retires `p1` and keeps the author's `p2`, its `name` erased: {p2:?}, \
         {report:?}"
    );
    Ok(())
}

async fn forgetting_an_admitted_source_retires_the_rows_it_alone_brought(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let own = l.own.clone();
    for (key, title) in [("s1", "Lamp"), ("s2", "Desk")] {
        let fed = feed(&l, &own, "samples", key, json!({"title": title})).await;
        fed.must(&format!("`samples` bringing `{key}`"))?;
    }
    let cat = l.cat.as_ref();
    upsert(cat, &own, "item", "s2", json!({"note": "kept"})).await?;
    upsert(cat, &own, "item", "a1", json!({"title": "Chair"})).await?;
    upsert(cat, &own, "item", "a2", json!({"status": "approved"})).await?;
    feed(&l, &own, "samples", "a2", json!({"title": "Lamp"}))
        .await
        .must("`samples` writing the empty `title` of `a2`")?;
    let e = feed(&l, &own, "samples", "a1", json!({"status": "listed"})).await;
    ensure!(
        e.as_ref().err() == Some(&wrong_door("status")),
        "`samples` writing `a1`'s `status` is refused `wrong_door`: {e:?}"
    );
    let samples = SourceId::from("samples");
    let pre = s
        .forget_preflight(&samples)
        .await
        .must("the forget preflight")?;
    let co: Vec<&str> = (pre.co_owned.iter())
        .flat_map(|t| t.first.iter().map(|k| k.as_str()))
        .collect();
    ensure!(
        co == ["a2", "s2"],
        "the preflight of `samples` names `a2` and `s2` co-owned: {co:?}"
    );
    let forgot = s.forget_source(&samples, None).await;
    forgot.must("forgetting `samples`")?;
    let s2 = at(&l, "item", "s2", None).await?;
    let a1 = at(&l, "item", "a1", None).await?;
    ensure!(
        at(&l, "item", "s1", None).await?.is_none()
            && s2.is_some_and(|r| r.fields.get("note") == Some(&json!("kept")))
            && held(&l, "s2", "title").await? == (json!("Desk"), Some("samples".into()))
            && held(&l, "a2", "status").await? == (json!("approved"), None)
            && held(&l, "a2", "title").await? == (json!("Lamp"), Some("samples".into()))
            && a1.is_some_and(|r| {
                r.fields.get("title") == Some(&json!("Chair"))
                    && r.fields.get("status") == Some(&json!("draft"))
            }),
        "forgetting `samples` retires `s1`, keeps `s2` with its `note` and `samples`' \
         `title`, which is not erasable, keeps the author's `a2` with it, and leaves `a1`"
    );
    let e = feed(&l, &own, "samples", "s3", json!({"title": "Shelf"})).await;
    ensure!(
        e.is_err(),
        "an ingest under `samples` after it is forgotten is refused: {e:?}"
    );
    Ok(())
}
