//! A field only its type's declared verbs write (lakelands#1130): its
//! authority is `verb`, so `upsert`, a proposed `upsert` and `ingest` are
//! refused it, and a verb's `set`, its `initial` and an aging rule write
//! it. A refusal names the verbs that write it which its writer is shown
//! and may call, and none it is not.

use lakelands_api::error::Case;
use lakelands_api::ontology::Authority;
use lakelands_api::write::{ProposedWrite, WriteOpts};
use lakelands_api::{Error, Refusal};
use serde_json::{Value, json};

use super::only::holder;
use super::paths::{accept, call, proposal_of};
use crate::conformance::fixture::{Lake, Must, head_unmoved, ingest, patch, try_upsert, upsert};
use crate::conformance::{Failure, Held, Store};

check!(
    VERB_ONLY = a_field_only_its_verbs_write_is_refused_at_every_other_door,
    "016 and 027, extended (lakelands#1130): a field declared `authority: verb` takes its `initial` at a create by `upsert` of the other fields; `upsert`, a proposed `upsert` and `ingest` that name it are refused `verbs_only`, naming the verbs that set it in name order, appending nothing; a low verb's call sets it, a medium one's proposal sets it at its accept, and an aging rule moves it; `declare` refuses it on a key field (`verb_key`), on a required field without `initial` (`verb_required`) and on a derived one (`verb_computed`), and refuses an `initial` of it that a constraint reading only such fields breaks (`initial_breaks`), or no `initial` where such a constraint refuses null (`no_initial_breaks`), where of a field authors write such an `initial` is taken and a create holding it refused `constraint_broken`, and a constraint an ingest admits a breach of or one reading a field authors write too is taken; a field of a relation's link only the link type's verbs write is refused at `link` and set by such a verb; a field written by authors declared again as `verb` is taken by `declare` with no migration, the type's version moving and its schema hash not, and from there refused at `upsert`, as `wrong_door` where no verb sets it; a migration's report restates the authority, and one that retypes a `verb` field converts its values"
);

check!(
    VERB_ONLY_TOLD = a_refusal_names_only_the_verbs_its_writer_is_shown,
    "006 and 016, extended (lakelands#1130): a writer kept from a `verb` field is answered `unknown_field` at `upsert`, as for a field the type does not have; a verb that reads a field kept from its writer is not named to it, so where it is the only one that sets the field the writer is answered `wrong_door` as where no verb sets it, and beside verbs it is shown, `verbs_only` naming those alone; a holder whose `only` names such a field with its `initial`'s value creates by `upsert` and is told `upsert`, and one whose `only` names another value is told no `upsert` and its create refused, `describe` naming the field's authority `verb` and the upsert tool leaving it out"
);

/// `order`: `no`, `status` (`verb`, initial `open`), `rank` (`verb`, a
/// number, initial 1) and `secret`; `ship` (low) and `hold` (medium) set
/// `status`; `bump` sets `rank`.
async fn orders(store: &Store) -> Result<Lake, Failure> {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "order", "fields": [
        {"name": "no", "type": {"kind": "text"}, "authority": "shared"},
        {"name": "status", "type": {"kind": "text"}, "authority": "verb", "initial": "open"},
        {"name": "rank", "type": {"kind": "number"}, "authority": "verb", "initial": 1},
        {"name": "secret", "type": {"kind": "text"}}]}))
        .await?;
    for (name, risk, field, value) in [
        ("ship", "low", "status", "'shipped'"),
        ("hold", "medium", "status", "'held'"),
        ("bump", "low", "rank", "rank + 1"),
    ] {
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "order", "risk": risk,
            "effect": "write", "set": [{"field": field, "value": value}]}),
        )
        .await?;
    }
    Ok(l)
}

fn verbs_only(field: &str, verbs: &[&str]) -> Error {
    Error::Refused {
        field: Some(field.into()),
        reason: Refusal::VerbsOnly {
            verbs: verbs.iter().map(|v| (*v).into()).collect(),
        },
    }
}

fn field_of(r: &lakelands_api::Record, f: &str) -> Value {
    r.fields.get(f).cloned().unwrap_or(Value::Null)
}

async fn a_field_only_its_verbs_write_is_refused_at_every_other_door(store: Store) -> Held {
    let (s, l) = (store.as_ref(), orders(&store).await?);
    for k in ["o1", "o2", "o3"] {
        upsert(l.cat.as_ref(), &l.own, "order", k, json!({"no": k})).await?;
    }
    let r = l.read("order", "o1").await?;
    ensure!(
        (field_of(&r, "status"), field_of(&r, "rank")) == (json!("open"), json!(1)),
        "a create by `upsert` of `no` alone takes `status` and `rank` from their `initial`: {r:?}"
    );
    // Every other door is refused it, naming the verbs that set it.
    let before = s.head().await.must("reading the head")?;
    let up = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "order",
        "o1",
        json!({"status": "shipped"}),
    )
    .await;
    let write = ProposedWrite::Upsert {
        ty: "order".into(),
        key: "o1".into(),
        patch: patch(json!({"status": "shipped"})),
        opts: WriteOpts::default(),
    };
    let proposed = l.cat.propose(&l.own, write, "by hand".into()).await;
    let record = crate::conformance::fixture::source_record("order", "o1", json!({"status": "x"}));
    let fed = ingest(l.cat.as_ref(), &l.own, "erp", vec![record]).await;
    let want = verbs_only("status", &["hold", "ship"]);
    ensure!(
        up.as_ref().err() == Some(&want)
            && proposed.as_ref().err() == Some(&want)
            && fed.as_ref().err() == Some(&want),
        "`upsert`, a proposed `upsert` and `ingest` of `status` are refused `verbs_only` naming \
         `hold` and `ship`: {up:?}, {proposed:?}, {fed:?}"
    );
    head_unmoved(s, before, "a write of a field only its verbs write").await?;
    // Its verbs write it: a low one called, a medium one at its accept.
    let shipped = call(&l, &l.own, "ship", "o1").await;
    let p = proposal_of(&l, &l.own, "hold", "o2").await?;
    let held = accept(&l, &l.own, &p).await;
    let (o1, o2) = (l.read("order", "o1").await?, l.read("order", "o2").await?);
    ensure!(
        (shipped, held) == (Ok("applied"), Ok("applied"))
            && (field_of(&o1, "status"), field_of(&o2, "status"))
                == (json!("shipped"), json!("held")),
        "`ship` sets `status` on `o1`, and `hold` on `o2` at its accept: {o1:?}, {o2:?}"
    );
    // An aging rule moves it.
    let rule =
        json!({"on": "order", "field": "status", "from": "open", "to": "stale", "when": "true"});
    upsert(l.cat.as_ref(), &l.own, "sys.aging_rule", "stale", rule).await?;
    let today = "2030-01-01T00:00:00Z".parse().expect("an instant");
    s.age(today).await.must("aging")?;
    let o3 = l.read("order", "o3").await?;
    ensure!(
        field_of(&o3, "status") == json!("stale"),
        "an aging rule moves `status` of `o3`, still open: {o3:?}"
    );
    // What the declaration refuses.
    for (field, case) in [
        (
            json!({"name": "no", "type": {"kind": "text"}, "authority": "verb"}),
            Case::VerbKey,
        ),
        (
            json!({"name": "x", "type": {"kind": "text"}, "authority": "verb", "required": true}),
            Case::VerbRequired,
        ),
        (
            json!({"name": "x", "type": {"kind": "text"}, "authority": "verb", "derived": "no"}),
            Case::VerbComputed,
        ),
    ] {
        let mut fields = vec![json!({"name": "no", "type": {"kind": "text"}})];
        fields.retain(|f| f["name"] != field["name"]);
        fields.push(field);
        let decl = json!({"declare": "type", "name": "bad", "key": ["no"], "fields": fields});
        let said = l
            .cat
            .declare(&l.own, serde_json::from_value(decl).expect("a declaration"))
            .await;
        let got = said
            .as_ref()
            .err()
            .and_then(Error::case)
            .map(|s| s.case.clone());
        ensure!(
            got == Some(case.clone()),
            "`declare` refuses it as {case:?}: {said:?}"
        );
    }
    // An `initial` a constraint refuses: of a field authors write, the
    // declaration takes it and a create that would hold it is refused, the
    // author free to send another value; of a field only verbs write,
    // which no create can send, the declaration refuses it.
    let boxed = |ty: &str, authority: &str| {
        json!({"declare": "type", "name": ty, "fields": [
            {"name": "no", "type": {"kind": "text"}},
            {"name": "size", "type": {"kind": "number"}, "authority": authority, "initial": 0}],
            "constraints": [{"name": "positive", "holds": "size > 0"}]})
    };
    l.declare_json(boxed("box", "authored")).await?;
    let made = try_upsert(l.cat.as_ref(), &l.own, "box", "b1", json!({"no": "b1"})).await;
    let broken = made
        .as_ref()
        .err()
        .and_then(Error::case)
        .map(|s| s.case.clone());
    let decl = serde_json::from_value(boxed("vbox", "verb")).expect("a declaration");
    let said = l.cat.declare(&l.own, decl).await;
    let refused = said
        .as_ref()
        .err()
        .and_then(Error::case)
        .map(|s| s.case.clone());
    ensure!(
        matches!(&broken, Some(Case::ConstraintBroken { constraint, .. }) if constraint.as_str() == "positive")
            && refused
                == Some(Case::InitialBreaks {
                    constraint: "positive".into(),
                    field: "size".into(),
                }),
        "an `initial` the constraint `positive` refuses is refused at a create where authors \
         write the field ({made:?}), and at the declaration where only verbs do ({said:?})"
    );
    // A constraint an ingest admits a breach of, or one that reads a field
    // authors write too, is taken; a field with no `initial` is null.
    let judged = |ty: &str, initial: Option<i64>, holds: &str, at_ingest: &str| {
        let mut size = json!({"name": "size", "type": {"kind": "number"}, "authority": "verb"});
        if let Some(i) = initial {
            size["initial"] = json!(i);
        }
        json!({"declare": "type", "name": ty,
            "fields": [{"name": "no", "type": {"kind": "text"}}, size],
            "constraints": [{"name": "positive", "holds": holds, "at_ingest": at_ingest}]})
    };
    let declared = async |decl: Value| {
        let decl = serde_json::from_value(decl).expect("a declaration");
        let said = l.cat.declare(&l.own, decl).await;
        let case = said.as_ref().err().and_then(Error::case);
        (said.is_ok(), case.map(|s| s.case.clone()))
    };
    let admitted = declared(judged("abox", Some(0), "size > 0", "admit")).await;
    let mixed = declared(judged("mbox", Some(0), "size > 0 or no = 'x'", "refuse")).await;
    let bare = declared(judged("nbox", None, "size > 0", "refuse")).await;
    let null = Case::NoInitialBreaks {
        constraint: "positive".into(),
        field: "size".into(),
    };
    ensure!(
        admitted == (true, None) && mixed == (true, None) && bare == (false, Some(null)),
        "a constraint an ingest admits a breach of ({admitted:?}) and one reading `no` too, which \
         authors write ({mixed:?}), are taken; `size > 0` over a `size` with no `initial` is \
         refused as a create leaving it null ({bare:?})"
    );
    // A link's field only its link type's verbs write: `link` is refused
    // it, a verb on the link type sets it.
    l.declare_json(json!({"declare": "type", "name": "courier", "fields": [{"name": "no", "type": {"kind": "text"}}]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "crate", "fields": [{"name": "no", "type": {"kind": "text"}}],
        "relations": [{"name": "sent_by", "target": "courier", "cardinality": "many_to_many",
            "fields": [{"name": "label", "type": {"kind": "text"}, "authority": "verb"}]}]}))
        .await?;
    l.declare_json(
        json!({"declare": "verb", "name": "relabel", "on": "sys.link.crate.sent_by", "risk": "low",
        "effect": "write", "set": [{"field": "label", "value": "'checked'"}]}),
    )
    .await?;
    upsert(l.cat.as_ref(), &l.own, "crate", "c1", json!({"no": "c1"})).await?;
    upsert(l.cat.as_ref(), &l.own, "courier", "k1", json!({"no": "k1"})).await?;
    let (ty, on, from, to) = ("crate".into(), "sent_by".into(), "c1".into(), "k1".into());
    let link = |fields: Value| {
        l.cat.link(
            &l.own,
            &ty,
            &from,
            &on,
            &to,
            patch(fields),
            WriteOpts::default(),
        )
    };
    let labelled = link(json!({"label": "x"})).await;
    link(json!({})).await.must("linking c1 to k1")?;
    let relabelled = call(&l, &l.own, "relabel", "c1:k1").await;
    let linked = l.read("sys.link.crate.sent_by", "c1:k1").await?;
    ensure!(
        labelled.as_ref().err() == Some(&verbs_only("label", &["relabel"]))
            && relabelled == Ok("applied")
            && field_of(&linked, "label") == json!("checked"),
        "`link` naming `label` is refused `verbs_only` naming `relabel`, and `relabel` on the \
         link type sets it: {labelled:?}, {relabelled:?}, {linked:?}"
    );
    // Marking a field authors write: a change of meaning, then refused.
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [
        {"name": "note", "type": {"kind": "text"}}]}))
        .await?;
    upsert(l.cat.as_ref(), &l.own, "memo", "m1", json!({"note": "a"})).await?;
    let was = l.describe_type("memo").await?.lifecycle;
    let marked = json!({"name": "memo", "fields": [
        {"name": "note", "type": {"kind": "text"}, "authority": "verb"}]});
    let report = l.cat.migrate(
        &l.own,
        serde_json::from_value(json!({"to": marked.clone()})).expect("a migration"),
    );
    let report = report.await.must("the preflight of marking `note`")?.data;
    let restated = serde_json::to_value(&report.restated).unwrap_or_default();
    let mut declared = marked;
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    let is = l.describe_type("memo").await?.lifecycle;
    let refused = try_upsert(l.cat.as_ref(), &l.own, "memo", "m1", json!({"note": "b"})).await;
    let wrong = Error::Refused {
        field: Some("note".into()),
        reason: Refusal::WrongDoor,
    };
    ensure!(
        (is.version, &is.schema_hash) == (was.version + 1, &was.schema_hash)
            && refused.as_ref().err() == Some(&wrong)
            && restated.to_string().contains("\"authority\""),
        "marking `note` as `verb` is a change of meaning, the version moving and not the hash \
         ({was:?} → {is:?}), restated in a migration's report ({restated}), and from there \
         `upsert` of it is `wrong_door`, no verb setting it: {refused:?}"
    );
    // A migration that retypes a `verb` field converts its values.
    let retyped = json!({"name": "order", "fields": [
        {"name": "no", "type": {"kind": "text"}, "authority": "shared"},
        {"name": "status", "type": {"kind": "text"}, "authority": "verb", "initial": "open"},
        {"name": "rank", "type": {"kind": "text"}, "authority": "verb", "initial": "1"},
        {"name": "secret", "type": {"kind": "text"}}]});
    l.migrate(serde_json::from_value(retyped).expect("a type"))
        .await?;
    let o1 = l.read("order", "o1").await?;
    ensure!(
        field_of(&o1, "rank") == json!("1"),
        "a migration of `rank` to text converts its value: {o1:?}"
    );
    Ok(())
}

async fn a_refusal_names_only_the_verbs_its_writer_is_shown(store: Store) -> Held {
    let (s, l) = (store.as_ref(), orders(&store).await?);
    // `peek` sets `status` and reads `secret`: one kept from `secret` is
    // not shown it. `diary`'s `mood` is set by `brood` alone, which reads
    // its `secret`; `quiet`'s `level` by no verb.
    l.declare_json(json!({"declare": "verb", "name": "peek", "on": "order", "risk": "low",
        "requires": "secret = 'x'", "effect": "write", "set": [{"field": "status", "value": "'peeked'"}]}))
        .await?;
    for (ty, field) in [("diary", "mood"), ("quiet", "level")] {
        l.declare_json(json!({"declare": "type", "name": ty, "fields": [
            {"name": "no", "type": {"kind": "text"}},
            {"name": "secret", "type": {"kind": "text"}},
            {"name": field, "type": {"kind": "text"}, "authority": "verb", "initial": "low"}]}))
            .await?;
        upsert(l.cat.as_ref(), &l.own, ty, "k1", json!({"no": "k1"})).await?;
    }
    l.declare_json(json!({"declare": "verb", "name": "brood", "on": "diary", "risk": "low",
        "requires": "secret = 'x'", "effect": "write", "set": [{"field": "mood", "value": "'dark'"}]}))
        .await?;
    upsert(l.cat.as_ref(), &l.own, "order", "o1", json!({"no": "o1"})).await?;
    let kept = holder(
        s,
        "kept",
        json!({"default": "write", "types": {"order": {"fields": {"status": "none"}}}}),
    );
    let unshown = holder(
        s,
        "unshown",
        json!({"default": "write", "types": {
            "order": {"fields": {"secret": "none"}}, "diary": {"fields": {"secret": "none"}}}}),
    );
    let write = async |who, ty: &str, field: &str| {
        let fields = json!({ field: "x" });
        let key = if ty == "order" { "o1" } else { "k1" };
        try_upsert(l.cat.as_ref(), who, ty, key, fields).await
    };
    let before = s.head().await.must("reading the head")?;
    let hidden = write(&kept, "order", "status").await;
    let absent = write(&kept, "order", "nowhere").await;
    let hidden_case = hidden
        .as_ref()
        .err()
        .and_then(Error::case)
        .map(|s| s.case.to_string());
    let absent_case = absent
        .as_ref()
        .err()
        .and_then(Error::case)
        .map(|s| s.case.to_string());
    ensure!(
        hidden_case
            .as_deref()
            .map(|c| c.replace("status", "nowhere"))
            == absent_case,
        "a writer kept from `status` is answered at `upsert` as for a field `order` does not \
         have: {hidden:?}, {absent:?}"
    );
    let not_shown = write(&unshown, "order", "status").await;
    let only_hidden = write(&unshown, "diary", "mood").await;
    let none_sets = write(&unshown, "quiet", "level").await;
    let shown = write(&l.own, "order", "status").await;
    let brooding = write(&l.own, "diary", "mood").await;
    let wrong = |f: &str| Error::Refused {
        field: Some(f.into()),
        reason: Refusal::WrongDoor,
    };
    ensure!(
        not_shown.as_ref().err() == Some(&verbs_only("status", &["hold", "ship"]))
            && shown.as_ref().err() == Some(&verbs_only("status", &["hold", "peek", "ship"]))
            && only_hidden.as_ref().err() == Some(&wrong("mood"))
            && none_sets.as_ref().err() == Some(&wrong("level"))
            && brooding.as_ref().err() == Some(&verbs_only("mood", &["brood"])),
        "a verb that reads `secret` is named only to a writer shown it: one kept from `secret` \
         is named `hold` and `ship` for `status`, and for `mood`, which `brood` alone sets, is \
         answered `wrong_door` as for `level`, which no verb sets: {not_shown:?}, {shown:?}, \
         {only_hidden:?}, {none_sets:?}, {brooding:?}"
    );
    head_unmoved(s, before, "a write of a field only its verbs write").await?;
    // One whose `only` names a field only verbs write creates where the
    // field's `initial` is the value named, and is told it may; `describe`
    // names the field's authority, and the upsert tool leaves it out.
    let opener = holder(
        s,
        "opener",
        json!({"default": "write", "types": {"order": {"only": {"status": "open"}}}}),
    );
    let made = try_upsert(l.cat.as_ref(), &opener, "order", "o9", json!({"no": "o9"})).await;
    let order = "order".into();
    let told = (l.cat).describe_type(&opener, &order, Default::default());
    let told = told.await.must("describing `order` to opener")?.data;
    let status = told.fields.iter().find(|f| f.name.as_str() == "status");
    let verb = status.is_some_and(|f| f.authority == Authority::Verb && !f.authored_writable());
    let lake = l.cat.describe(&opener, Default::default()).await;
    let tools = lakelands_api::tools::tools(&lake.must("describing the lake to opener")?.data);
    let upsert_tool = tools.unwrap_or_default().into_iter().find(
        |t| matches!(&t.door, lakelands_api::tools::Door::Upsert { ty } if ty.as_str() == "order"),
    );
    let fields = upsert_tool.map(|t| t.input_schema["properties"]["fields"]["properties"].clone());
    let fields: Vec<String> = (fields.as_ref().and_then(Value::as_object))
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    // One whose `only` names another value is told no `upsert`, and its
    // create, which would hold the `initial`, is refused.
    let shipped = holder(
        s,
        "shipper",
        json!({"default": "write", "types": {"order": {"only": {"status": "shipped"}}}}),
    );
    let refused = try_upsert(l.cat.as_ref(), &shipped, "order", "o8", json!({"no": "o8"})).await;
    let theirs = (l.cat).describe_type(&shipped, &order, Default::default());
    let theirs = theirs.await.must("describing `order` to shipper")?.data;
    ensure!(
        refused.is_err() && !theirs.may.upsert,
        "a holder of `only {{status: shipped}}`, not `status`'s `initial`, is told no `upsert` \
         ({:?}) and its create of `o8` is refused ({refused:?})",
        theirs.may
    );
    ensure!(
        made.is_ok() && told.may.upsert && verb && fields == ["no", "secret"],
        "a holder of `only {{status: open}}`, `open` being `status`'s `initial`, creates `o9` \
         ({made:?}) and is told `upsert` ({:?}); `status` is described as `verb` and not \
         writable at `upsert` ({status:?}), and the upsert tool's fields are `no` and `secret` \
         ({fields:?})",
        told.may
    );
    Ok(())
}
