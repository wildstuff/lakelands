//! Triggers on a relation carried by link records
//! (`docs/decisions/035-a-link-is-a-record.md`): a trigger watches the
//! relation's link type, `sys.link.<type>.<relation>`, and fires on every
//! write of a link, as a trigger on a type fires on every write of a
//! record: `link`, a link written again, `unlink`, and each link a source
//! ingests. A relation carried by a reference field has no link type; its
//! links change with the field, which `changed(<field>)` watches.

use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::{Authority, Cardinality, FieldDecl, RelationDecl, StorageKind};
use lakelands_api::write::WriteOpts;
use lakelands_api::{Error, Position, Refusal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, field, patch, reference, source_record, trigger_on, type_decl,
};
use crate::conformance::{Failure, Held, Store};
use crate::links::{link_key, link_type_name};

check!(
    LINK_TRIGGER_FIRES = a_trigger_on_a_link_type_fires_on_link_rewrite_unlink_and_ingest,
    "035 and 004: a trigger naming a relation's link type fires on link, on the link written again, on unlink and on each ingested link, naming the link type and the link's key, caused by that write; its guard reads the link's fields, and `changed(f)` holds on a write that changes f and not on unlink"
);

check!(
    VIA_LINK_TYPE_REFUSED = a_trigger_on_the_link_type_of_a_reference_relation_is_refused,
    "035: a relation carried by a reference field has no link type, so a trigger naming one is refused as invalid on `on`, saying to watch the type with `changed(<field>)`"
);

fn relation(
    name: &str,
    via: Option<&str>,
    authority: Authority,
    fields: Vec<FieldDecl>,
) -> RelationDecl {
    RelationDecl {
        name: name.into(),
        target: "partner".into(),
        cardinality: Cardinality::ManyToMany,
        via: via.map(Into::into),
        fields,
        authority,
        ..Default::default()
    }
}

async fn a_trigger_on_a_link_type_fires_on_link_rewrite_unlink_and_ingest(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let mut invoice = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    invoice.relations = vec![
        relation(
            "approved_by",
            None,
            Authority::Authored,
            vec![field("note", StorageKind::Text)],
        ),
        relation("sourced_by", None, Authority::Source, vec![]),
    ];
    l.declare(invoice).await?;
    let approved = link_type_name(&"invoice".into(), &"approved_by".into());
    let sourced = link_type_name(&"invoice".into(), &"sourced_by".into());
    for t in [
        trigger_on("any_approval", approved.as_str(), None),
        trigger_on("noted_x", approved.as_str(), Some("note = 'x'")),
        trigger_on("note_changed", approved.as_str(), Some("changed(note)")),
        trigger_on("any_source", sourced.as_str(), None),
    ] {
        l.declare_as(t, "declaring a trigger on a link type")
            .await?;
    }
    l.upsert("partner", "p1", json!({"name": "one"})).await?;
    for i in ["i1", "i2", "i3"] {
        l.upsert("invoice", i, json!({"amount": "1"})).await?;
    }
    let link = |note: &'static str| {
        let cat = cat.clone();
        let own = own.clone();
        async move {
            cat.link(
                &own,
                &"invoice".into(),
                &"i1".into(),
                &"approved_by".into(),
                &"p1".into(),
                patch(json!({"note": note})),
                WriteOpts::default(),
            )
            .await
            .must("linking i1 to p1")
        }
    };
    let first = link("x").await?.position();
    link("y").await?;
    cat.unlink(
        own,
        &"invoice".into(),
        &"i1".into(),
        &"approved_by".into(),
        &"p1".into(),
        WriteOpts::default(),
    )
    .await
    .must("unlinking i1 from p1")?;
    l.ingest(
        "erp",
        ["i2", "i3"]
            .into_iter()
            .map(|i| {
                source_record(
                    sourced.as_str(),
                    link_key(&i.into(), &"p1".into()).as_str(),
                    json!({"from": i, "to": "p1"}),
                )
            })
            .collect(),
    )
    .await
    .must("ingesting two `sourced_by` links in one window")?;

    let feed = events_from(store.as_ref(), first).await?;
    let by_position = |p: Position| feed.iter().find(|e| e.position == p);
    // Each firing of `name`: the link it names and the kind of the write
    // that caused it.
    let fired = |name: &str| -> Vec<(String, String, &'static str)> {
        feed.iter()
            .filter_map(|e: &Event| match &e.body {
                EventBody::TriggerFired {
                    trigger, ty, key, ..
                } if trigger.as_str() == name => {
                    let cause = e
                        .cause
                        .and_then(by_position)
                        .map_or("none", |c| c.body.kind());
                    Some((ty.to_string(), key.to_string(), cause))
                }
                _ => None,
            })
            .collect()
    };
    let i1 = link_key(&"i1".into(), &"p1".into()).to_string();
    let on = |kind: &'static str| (approved.to_string(), i1.clone(), kind);
    for (name, want) in [
        (
            "any_approval",
            vec![
                on("record_upserted"),
                on("record_upserted"),
                on("record_retired"),
            ],
        ),
        ("noted_x", vec![on("record_upserted")]),
        (
            "note_changed",
            vec![on("record_upserted"), on("record_upserted")],
        ),
    ] {
        let got = fired(name);
        ensure!(
            got == want,
            "the trigger `{name}` on `{approved}`, over link (note x), link again (note y) and \
             unlink: fired {got:?}, not {want:?}"
        );
    }
    let sources: Vec<(String, String, &'static str)> = ["i2", "i3"]
        .iter()
        .map(|i| {
            (
                sourced.to_string(),
                link_key(&(*i).into(), &"p1".into()).to_string(),
                "record_ingested",
            )
        })
        .collect();
    let got = fired("any_source");
    ensure!(
        got == sources,
        "the trigger `any_source` on `{sourced}`, over two links ingested in one window: fired \
         {got:?}, not {sources:?}"
    );
    Ok(())
}

async fn a_trigger_on_the_link_type_of_a_reference_relation_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let mut invoice = type_decl("invoice", vec![reference("billed", "partner")]);
    invoice.relations = vec![relation(
        "billed_by",
        Some("billed"),
        Authority::Authored,
        vec![],
    )];
    l.declare(invoice).await?;
    let on = link_type_name(&"invoice".into(), &"billed_by".into());
    let e = cat
        .declare(own, trigger_on("billing", on.as_str(), None))
        .await
        .expect_err("a trigger on the link type of a reference relation is refused");
    let Error::Refused {
        field,
        reason: Refusal::Invalid(said),
    } = &e
    else {
        return Err(Failure::new(format!(
            "a trigger on `{on}` was refused as {e:?}, not as invalid"
        )));
    };
    // The case whose sentence says to watch `invoice` with `changed(billed)`.
    let watch = matches!(&said.case, Case::WatchesViaLinks { via, owner, .. }
        if via == "billed" && **owner == "invoice");
    ensure!(
        field.as_ref().map(|f| f.as_str()) == Some("on") && watch,
        "a trigger on `{on}` is refused on `on`, saying to watch `invoice` with \
         `changed(billed)`; it was refused on {field:?}: {said:?}"
    );
    Ok(())
}
