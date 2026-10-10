//! A field may declare its initial value
//! (`docs/decisions/050-a-field-declares-its-initial-value.md`,
//! lakelands#551).

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl, ValueTypeDecl};
use lakelands_api::record::{FieldMeta, Origin, SourceRecord, Upstream};
use lakelands_api::write::{Declaration, Outcome, ProposedWrite};
use lakelands_api::{Access, Position, PrincipalKind, Record, Refusal, Timestamp};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, committed, field, field_access, invalid_case, log_events, migration, principal,
    refused_on, reviewer, source_record, trigger_on, value_type, write_verb,
};
use crate::conformance::{Failure, Held, Store};

check!(
    FILLED_AT_CREATION = a_record_takes_its_initial_values_when_it_comes_to_exist,
    "050 (lakelands#551): a declared `initial` is kept in the one form a write of its field stores, so `\"1.50\"` after `\"1.5\"` on a decimal of scale 2 is the declaration in force; the event that makes a record fills each field that declares `initial` and holds no value after its patch, in the field's one form, at the record's first version, with the origin `initial`, the event's position, time and writer and nothing of its source, and a derived field reads it there; so a source's delivery makes a record whose required authored field it does not carry, the same content again is a duplicate, an author moves the value and the next delivery leaves it; an authored create is filled too, one that carries the field keeps what it carries, and a null there is filled; a later declaration with another value changes no record and is what the next one takes, a field added with one fills no record that exists, a record retired and made again starts again as a create to a trigger, an author's clear clears, and a re-key fills nothing"
);

check!(
    DECLARED = an_initial_value_is_declared_on_an_authored_field_of_a_types_own,
    "050 (lakelands#551): `initial` is refused as invalid, naming the field, on a field a source writes, a shared one, a derived one, one with an expression, a key's field, a reference, a relation's `via` field, a field its inputs clear, a view's field, a relation's field, a verb's payload and a function's input; a value its field would not take is refused as a constraint naming the field, by its kind or its value type, a number written as an integer beyond ±(2^53 − 1) before that at its member, as anywhere in a declaration (the log format §1.2); the schema hash leaves it out, so a changed `initial` is a new version under the same hash, and a migration proposed before it changed is refused at its accept by what it restates; a value type's tightening that a declared `initial` no longer fits counts it a misfit of its field, shown to the owner, and to a proposer who may not see the type or reads the field masked is refused by its count alone"
);

/// One field of a record in a line: the record's version, the value, its
/// origin and, where its provenance names one, its source.
fn told(r: &Record, f: &str) -> String {
    let m = r.meta.get(f);
    let origin = m.map_or(Value::Null, |m| json!(m.origin));
    let from = m.and_then(|m| m.source.as_ref());
    format!(
        "v{} {}, {}{}",
        r.version.0,
        r.fields.get(f).unwrap_or(&Value::Null),
        origin.as_str().unwrap_or("no origin"),
        from.map_or(String::new(), |s| format!(" from {s}"))
    )
}

fn note(key: &str) -> TypeDecl {
    let text = json!({"kind": "text"});
    serde_json::from_value(json!({"name": "note", "key": [key], "fields": [
        {"name": "code", "type": text}, {"name": "tag", "type": text, "required": true},
        {"name": "state", "type": text, "initial": "draft"}]}))
    .expect("the check's type reads")
}

async fn a_record_takes_its_initial_values_when_it_comes_to_exist(store: Store) -> Held {
    let l = &Lake::of(store.as_ref());
    let order = |initial: &str| {
        json!({"declare": "type", "name": "order", "key": ["id"], "fields": [
            {"name": "id", "type": {"kind": "text"}, "required": true, "authority": "source"},
            {"name": "total", "type": {"kind": "number"}, "authority": "source"},
            {"name": "state", "type": {"kind": "text"}, "required": true, "initial": initial},
            {"name": "fee", "type": {"kind": "decimal", "value_type": "money"}, "initial": "1.5"},
            {"name": "open", "type": {"kind": "boolean"}, "authority": "derived",
             "derived": "state = 'new'", "depends_on": ["state"]}]})
    };
    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    l.declare_as(Declaration::ValueType(money), "the value type")
        .await?;
    l.declare_json(order("new")).await?;
    // The value is kept in its one form: `"1.50"` is the declaration in force.
    let fee = |d: &lakelands_api::describe::TypeDescription| {
        let f = d.fields.iter().find(|f| f.name.as_str() == "fee");
        (d.lifecycle.version, f.and_then(|f| f.initial.clone()))
    };
    let before = fee(&l.describe_type("order").await?);
    let mut again = order("new");
    again["fields"][3]["initial"] = json!("1.50");
    l.declare_json(again).await?;
    let after = fee(&l.describe_type("order").await?);
    ensure!(
        before == after && before.1 == Some(json!("1.50")),
        "`initial` is kept as a write of its field stores it; \"1.5\" then \"1.50\" are {before:?} \
         and {after:?}"
    );
    let made = trigger_on("made", "order", Some("previous(state) is null"));
    l.declare_as(made, "the trigger").await?;
    let deliver = |key: &'static str, total: u32| async move {
        let record = SourceRecord {
            seen_at: Some(Timestamp(1)),
            upstream: Some(Upstream {
                lake: "elsewhere".into(),
                position: Position(7),
            }),
            ..source_record("order", key, json!({"id": key, "total": total}))
        };
        let windows = l.ingest("erp", vec![record]).await;
        Ok::<_, Failure>(committed(&windows.must(&format!("delivering `{key}`"))?))
    };
    let state = |ty: &'static str, key: &'static str| async move {
        Ok::<_, Failure>(told(&l.read(ty, key).await?, "state"))
    };
    let mut said = Vec::new();

    deliver("o1", 5).await?;
    deliver("o2", 5).await?;
    let o1 = l.read("order", "o1").await?;
    let total = o1.meta.get("total").cloned();
    let total = total.ok_or_else(|| Failure::new("`total` keeps no provenance"))?;
    let filled = FieldMeta {
        origin: Origin::Initial,
        seen_at: None,
        source: None,
        upstream: None,
        ..total.clone()
    };
    said.push(format!(
        "made: {}; fee {}; open {}; of its event alone {}; total {}",
        told(&o1, "state"),
        told(&o1, "fee"),
        o1.fields["open"],
        o1.meta.get("state") == Some(&filled)
            && total.seen_at.is_some()
            && total.upstream.is_some(),
        told(&o1, "total"),
    ));
    said.push(format!("again: {:?}", deliver("o1", 5).await?));
    l.upsert("order", "o1", json!({"state": "paid"})).await?;
    deliver("o1", 6).await?;
    let o1 = l.read("order", "o1").await?;
    said.push(format!(
        "moved: {}; open {}",
        told(&o1, "state"),
        o1.fields["open"]
    ));

    l.declare_json(order("fresh")).await?;
    deliver("o3", 1).await?;
    said.push(format!(
        "declared again: {}; {}",
        state("order", "o2").await?,
        state("order", "o3").await?
    ));
    l.retire("order", "o2").await?;
    deliver("o2", 5).await?;
    let log = log_events(store.as_ref()).await?;
    let fired = |e: &&lakelands_api::event::Event| matches!(e.body, EventBody::TriggerFired { .. });
    let fired = log.iter().filter(fired).count();
    said.push(format!(
        "made again: {}; creates {fired}",
        state("order", "o2").await?
    ));
    let rush = FieldDecl {
        initial: Some(json!(false)),
        ..field("rush", StorageKind::Boolean)
    };
    let added = l.cat.add_field(&l.own, &"order".into(), rush).await;
    added.must("adding a field with an initial value")?;
    deliver("o4", 1).await?;
    let rush = |r: Record| r.fields.get("rush").cloned().unwrap_or(Value::Null);
    said.push(format!(
        "added: {}; {}",
        rush(l.read("order", "o1").await?),
        rush(l.read("order", "o4").await?)
    ));

    l.declare(note("code")).await?;
    for (key, fields) in [
        ("n1", json!({"tag": "t1"})),
        ("n2", json!({"tag": "t2", "state": "final"})),
        ("n3", json!({"tag": "t3", "state": null})),
    ] {
        l.upsert("note", key, fields).await?;
        said.push(format!("{key}: {}", state("note", key).await?));
    }
    l.upsert("note", "n1", json!({"state": null})).await?;
    said.push(format!("cleared: {}", state("note", "n1").await?));
    l.migrate(note("tag")).await?;
    said.push(format!("re-keyed: {}", state("note", "t1").await?));

    let want = [
        "made: v1 \"new\", initial; fee v1 \"1.50\", initial; open true; of its event alone true; \
         total v1 5, source from erp",
        "again: (0, 1)",
        "moved: v3 \"paid\", authored; open false",
        "declared again: v1 \"new\", initial; v1 \"fresh\", initial",
        "made again: v3 \"fresh\", initial; creates 4",
        "added: null; false",
        "n1: v1 \"draft\", initial",
        "n2: v1 \"final\", authored",
        "n3: v1 \"draft\", initial",
        "cleared: v2 null, no origin",
        "re-keyed: v2 null, no origin",
    ];
    ensure!(
        said == want,
        "a record takes its initial values when it comes to exist, and never again; the lake \
         says:\n{}",
        said.join("\n")
    );
    Ok(())
}

async fn an_initial_value_is_declared_on_an_authored_field_of_a_types_own(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(
        json!({"declare": "value_type", "name": "status", "kind": "text",
        "one_of": ["new", "paid"]}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "partner",
        "fields": [{"name": "name", "type": text}]}))
        .await?;
    // A type `t` whose field `f` declares `initial` with `members` besides,
    // and the type `more`.
    let merged = |mut into: Value, from: Value| {
        for (k, v) in from.as_object().into_iter().flatten() {
            into[k] = v.clone();
        }
        into
    };
    let f = |members: Value| merged(json!({"name": "f", "type": text, "initial": "x"}), members);
    let ty = |members: Value, more: Value| {
        let fields = json!([{"name": "g", "type": text}, f(members)]);
        merged(
            json!({"declare": "type", "name": "t", "fields": fields}),
            more,
        )
    };
    let none = || json!({});
    let partner = json!({"kind": "reference", "target": "partner"});
    let via = json!({"relations": [{"name": "r", "target": "partner", "via": "f"}]});
    let counted = json!({"declare": "type", "name": "t", "key": ["name"],
        "fields": [{"name": "name", "type": text},
                   {"name": "f", "type": {"kind": "number"}, "initial": 0}],
        "view": {"source": "partner", "group_by": ["name"],
                 "measures": [{"name": "f", "expr": "count()"}]}});
    let linked = json!({"relations": [{"name": "r", "target": "partner", "fields": [f(none())]}]});
    let mut verb = write_verb("mark", "partner", "name", "payload.f");
    if let Declaration::Verb(v) = &mut verb {
        v.payload = vec![serde_json::from_value(f(none())).expect("the check's field reads")];
    }
    let verb = serde_json::to_value(&verb).expect("a declaration serialises");
    let function = json!({"declare": "function", "name": "loud", "backing": "expr",
        "inputs": [f(none())], "expr": "upper(f)"});
    let derived = json!({"authority": "derived", "derived": "upper(g)", "depends_on": ["g"]});
    let cleared = json!({"depends_on": ["g"], "on_input_change": "clear"});
    let status = json!({"kind": "text", "value_type": "status"});
    let of = |members: Value| ty(members, none());
    let reference = of(json!({"type": partner, "initial": "p1"}));
    let relation = merged(ty(none(), linked), json!({"fields": []}));
    let number = of(json!({"type": {"kind": "number"}}));
    // Each case and the refusal it takes on `f`: `i`nvalid, or `c`onstraint.
    let cases = [
        ("a source's", of(json!({"authority": "source"})), 'i'),
        ("a shared one", of(json!({"authority": "shared"})), 'i'),
        ("a derived one", of(derived), 'i'),
        ("an expression's", of(json!({"derived": "upper(g)"})), 'i'),
        ("a key's", ty(none(), json!({"key": ["f"]})), 'i'),
        ("a reference", reference, 'i'),
        ("a via field", ty(none(), via), 'i'),
        ("one its inputs clear", of(cleared), 'i'),
        ("a view's", counted, 'i'),
        ("a relation's", relation, 'i'),
        ("a payload's", verb, 'i'),
        ("an input's", function, 'i'),
        ("not of its kind", number, 'c'),
        ("not of its value type", of(json!({"type": status})), 'c'),
    ];
    let mut said = Vec::new();
    for (what, declaration, want) in cases {
        let d: Declaration = serde_json::from_value(declaration)
            .map_err(|e| Failure::new(format!("the declaration of {what} does not read: {e}")))?;
        let answer = l.cat.declare(&l.own, d).await;
        let got = match refused_on(&answer) {
            Some(("f", Refusal::Invalid(..))) => 'i',
            Some(("f", Refusal::Constraint(..))) => 'c',
            _ => '-',
        };
        if got != want {
            said.push(format!(
                "{what}: {want} on `f` expected, answered {answer:?}"
            ));
        }
    }
    ensure!(
        said.is_empty(),
        "`initial` stands on an authored field of a type's own alone:\n{}",
        said.join("\n")
    );

    // The hash leaves it out: a changed `initial` is a new version alone.
    let mut seen = Vec::new();
    for initial in [json!("new"), json!("paid"), Value::Null] {
        l.declare_json(ty(json!({"type": status, "initial": initial}), none()))
            .await?;
        let life = l.describe_type("t").await?.lifecycle;
        seen.push((life.version, life.schema_hash));
    }
    ensure!(
        seen.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 == w[1].1),
        "the schema hash leaves `initial` out, and a changed one is a new version; `new`, \
         `paid` and none are {seen:?}"
    );
    // A proposed migration whose `to` would bring back an `initial` changed
    // since is refused at its accept, by what it restates.
    let new = ty(json!({"type": status, "initial": "new"}), none());
    l.declare_json(new.clone()).await?;
    let renamed = new.to_string().replace(r#""g""#, r#""h""#);
    let Ok(Declaration::Type(to)) = serde_json::from_str(&renamed) else {
        unreachable!("the check's type reads")
    };
    let mut m = migration(to);
    m.renames = vec![("g".into(), "h".into())];
    let (migration, report) = (Box::new(m), Default::default());
    let write = ProposedWrite::Migrate { migration, report };
    let id = match l.cat.propose(&l.own, write, "g is h".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => proposal,
        other => return Err(Failure::new(format!("proposing the rename: {other:?}"))),
    };
    l.declare_json(ty(json!({"type": status, "initial": "paid"}), none()))
        .await?;
    let accepted = l.cat.accept(&reviewer(store.as_ref(), true), &id).await;
    ensure!(
        matches!(invalid_case(&accepted), Some(Case::ChangedSinceProposed { ty }) if ty == "t"),
        "a migration proposed before `initial` changed is refused at its accept; {accepted:?}"
    );
    // A tightening a declared `initial` no longer fits is a misfit of its
    // field: the owner is shown the field, a proposer who may not see the
    // type or reads the field masked the count alone.
    l.declare_json(new).await?;
    let narrower: ValueTypeDecl = serde_json::from_value(json!({"name": "status", "kind": "text",
        "one_of": ["paid"]}))
    .expect("the check's value type reads");
    let to = || Box::new(narrower.clone());
    let shown = l.cat.migrate_value_type(&l.own, narrower.clone().into());
    let shown = shown.await;
    let shown = shown.must("the owner's preflight of the tightening")?.data;
    let uses = json!(shown.fields);
    ensure!(
        !shown.fits
            && uses == json!([{"type": "t", "field": "f", "values_scanned": 0, "misfits": 1}]),
        "the owner is shown the `initial` as a misfit of `t.f`; it is shown {shown:?}"
    );
    let hidden = json!({"default": "read", "types": {"t": {"default": "none"}}});
    let hidden = serde_json::from_value(hidden).expect("the check's grants read");
    let hidden = principal(store.as_ref(), PrincipalKind::Agent, "stranger", hidden);
    let masked = field_access(store.as_ref(), "glance", "t", &[("f", Access::Masked)]);
    for who in [&hidden, &masked] {
        let write = ProposedWrite::MigrateValueType {
            to: to(),
            renames: Vec::new(),
            report: Default::default(),
        };
        let said = l.cat.propose(who, write, "fewer states".into()).await;
        ensure!(
            matches!(invalid_case(&said), Some(Case::ValueTypeNotTightened { misfits: 1, first, .. })
                if first.is_empty())
                && refused_on(&said).is_none(),
            "a tightening a declared `initial` no longer fits is refused to `{}` by its count \
             alone; it answered {said:?}",
            who.id
        );
    }
    Ok(())
}
