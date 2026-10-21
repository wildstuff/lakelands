//! A type, a trigger, a verb, an object set or a function withdrawn: out
//! of force from the withdrawal on, still there to a read as of before it
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended). A withdrawal is one `declared` event; what something in
//! force reads is not withdrawn, and a name not in force is not either.

use lakelands_api::event::EventBody;
use lakelands_api::ontology::{FunctionInput, RelationDecl, StorageKind, TriggerDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::seam::Seams;
use lakelands_api::write::{
    Declaration, DeclarationKind, FunctionDecl, InForce, ProposalFilter, ProposalState,
    ProposedWrite, Withdrawal, WriteOpts,
};
use lakelands_api::{
    Access, Catalog, Error, Grants, LakeStore, Missing, Outcome, Position, PrincipalKind,
    ProposalId, Refusal, TypeName,
};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, computed_by, events_after, field, find_at, invalid_like, limits, object_set_on,
    patch, principal, reactive, read_at, reason, reference, refused_alike, refused_unmoved,
    reviewer, trigger_on, try_upsert, type_decl, write_verb,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    WITHDRAWN_TRIGGER = a_withdrawn_trigger_stops_firing_and_is_seen_as_of_before,
    "027, extended (lakelands#1219): a trigger withdrawn by `declare` is one `declared` event, answered in force as its kind and name at the next version, appended; with the lake opened again, `describe` no longer lists it and a describe as of before still does; the next write does not fire it, and a reaction whose `cause` names an earlier firing of it is judged by the lake's `max_reactive_depth` alone, past the trigger's own `max_depth`; withdrawn again it is refused `withdrawn_undeclared`, appending nothing; declared again its version continues and it fires, its own `max_depth` binding a reaction to its own firing, and the reaction to the earlier firing is still judged by the lake's ceiling alone",
    given
);

check!(
    WITHDRAWN_FUNCTION = a_function_is_withdrawn_once_no_field_names_it,
    "027, extended (lakelands#1219): a function a field computes is refused `withdrawn_in_use`, naming the function and that field, appending nothing; so is one a relation's link field computes, named at the relation, and, the relation gone and its link type left standing, named at the link type; one no field names is withdrawn by `apply`, its plan moving no record, gone from `describe` and there as of before, and declared again its version continues; withdrawn by `propose` and its accept; a setting is refused `withdrawn_kind`, naming the kind"
);

check!(
    WITHDRAWN_TYPE = a_type_is_withdrawn_once_it_holds_no_live_record_and_nothing_names_it,
    "027, extended (lakelands#1292): a type is refused `withdrawn_in_use`, appending nothing, naming in turn what holds it, each withdrawn at its next version before the next refusal: a field of another type pointing at its link type, a view computed from it, a verb on it, a verb whose payload points at it, an object set of it, a trigger on it; then its live record, which is retired; a link type its relation carries is refused naming the relation, and a type the lake reserves naming the lake; then a relation of another type pointing at it is named, and the two withdrawn by one `apply`, it first and its link type by name, leave with the other's link type, one `declared` each, all gone from `describe` and there as of before; declared again it is refused `withdrawn_name_held` while the lake keeps its retired record, and the type that pointed, which held none, declared again continues its version"
);

check!(
    WITHDRAWAL_TWIN = a_withdrawal_tells_a_reader_not_shown_its_type_nothing,
    "027 and 024, extended (lakelands#1219): to a holder of `schema` not shown a trigger's type, withdrawing the trigger is refused as withdrawing a name never declared, word for word, appending nothing, alone and by `apply` beside a declaration refused of its own; withdrawing a function that computes a field of that type is not granted by `declare`, `apply` and `propose`, naming neither; the owner's proposal to withdraw the trigger is not listed to that reader, pending or accepted, and is to the owner, under a filter by the trigger's type too, and so once a trigger of that name stands on a type the reader is shown, pending or accepted; that reader's accept of a function's withdrawal is not granted, naming nothing, where only that type reads the function, and refused naming the field it is shown, of a type sorting after the hidden one, where another reads it too; a trigger whose `when` reads a field a reviewer is kept from is not shown to it, and neither is its withdrawal, pending or accepted, under a filter by its type too"
);

check!(
    WITHDRAWN_VERB = a_withdrawn_verb_takes_no_call_and_frees_its_tool_name,
    "027, extended (lakelands#1219): a verb that alone of its type's verbs sets a field only verbs write is refused `withdrawn_in_use`, naming that field, appending nothing; one beside another that sets it is withdrawn by `declare`, one `declared` at the next version, gone from `describe` and there as of before; a direct call of it and the accept of a call of it proposed before, by the owner and by a reviewer shown it then, are refused as a verb not declared, judged by the verb as it stood at the proposal, while a reviewer not shown its type is answered as for a proposal id that does not exist, as it was before the withdrawal, and is listed none of its calls, as before it, whole and by its type; the call stays pending until it is rejected, and an earlier call of it, once it is declared again otherwise, is refused `verb_redeclared`; from there the other verb is the field's last and is refused; a verb's tool name is free once it is withdrawn, a type making that name taken; declared again its version continues"
);

check!(
    WITHDRAWN_SET = a_withdrawn_set_holds_no_member_from_there_on,
    "027 and 004, extended (lakelands#1219): an object set another set names, or a trigger's `when` names, is refused `withdrawn_in_use`, naming that set or trigger, appending nothing; the two sets withdrawn by one `apply` are taken; a set withdrawn by `apply` appends one `declared` and no `object_set_changed`, is gone from `describe` and there as of before, a read naming it refused `set_undeclared` at the head and answering its members as of before; declared again with another filter its version continues, it joins only the records that filter holds for, and a later write that takes in a record it held before joins it again"
);

check!(
    WITHDRAWN_UNSEEN = a_verb_or_set_withdrawal_tells_a_reader_not_shown_it_nothing,
    "027 and 024, extended (lakelands#1219): to a holder of `schema` not shown a type, withdrawing a verb or an object set of it is refused as withdrawing one never declared, word for word, appending nothing; withdrawing a set it is shown that a trigger it is not shown watches, one reading across an edge a field it is kept from, is not granted, by `declare` and by the accept of a proposal made before the trigger, naming neither the trigger nor that field, where the owner is told the trigger"
);

check!(
    WITHDRAWN_RETIRE = the_last_verb_that_retires_a_type_is_not_withdrawn,
    "027 and 016, extended (lakelands#1239): of a type only its verbs retire, a retire verb beside another is withdrawn; the last is refused `withdrawn_in_use`, naming the type, appending nothing; the type marked `retired_by: authors` again, it is withdrawn"
);

fn withdraw(kind: DeclarationKind, name: &str) -> Declaration {
    Declaration::Withdraw(Withdrawal {
        kind,
        name: name.into(),
    })
}

fn function(name: &str) -> FunctionDecl {
    FunctionDecl {
        name: name.into(),
        backing: "expr".into(),
        inputs: vec![FunctionInput::typed(field("n", StorageKind::Number))],
        expr: Some("n * 2".into()),
        config: serde_json::Value::Null,
        description: None,
        risk: Default::default(),
    }
}

/// The one `declared` a withdrawal appended after the head `before`, its
/// answer in force, and whether its plan moves no record.
async fn withdrawn(
    store: &dyn LakeStore,
    before: Position,
    answer: lakelands_api::Result<Outcome>,
    what: &str,
) -> Result<(Position, InForce, bool), Failure> {
    let Outcome::Applied { declared, plan, .. } = answer.must(what)? else {
        return Err(Failure::new(format!("{what} must be applied")));
    };
    let still = plan.is_some_and(|p| p.types.is_empty() && p.value_types.is_empty() && p.lossless);
    let events = events_after(store, before).await?;
    let one = matches!(events.as_slice(), [e] if matches!(
        &e.body,
        EventBody::Declared { declaration: Declaration::Withdraw(_), .. }
    ));
    ensure!(
        one,
        "{what} must append one `declared` of the withdrawal: {events:?}"
    );
    let [d] = declared.as_slice() else {
        return Err(Failure::new(format!(
            "{what} must answer one declaration: {declared:?}"
        )));
    };
    Ok((events[0].position, d.clone(), still))
}

/// The names `who` is described of `kind`, as of `at`.
async fn described(
    cat: &dyn Catalog,
    who: &lakelands_api::Principal,
    at: Option<Position>,
    kind: DeclarationKind,
) -> Result<Vec<String>, Failure> {
    let d = cat
        .describe(who, read_at(at))
        .await
        .must("describing")?
        .data;
    Ok(match kind {
        DeclarationKind::Type => d.types.iter().map(|t| t.name.to_string()).collect(),
        DeclarationKind::Trigger => d.triggers.iter().map(|t| t.name.to_string()).collect(),
        DeclarationKind::Verb => d.verbs.iter().map(|v| v.name.to_string()).collect(),
        DeclarationKind::ObjectSet => d.object_sets.iter().map(|s| s.name.clone()).collect(),
        _ => d.functions.iter().map(|f| f.name.clone()).collect(),
    })
}

/// Fails unless `name` of `kind` is described at the head and not as of
/// the position before `at`, its withdrawal's.
async fn gone_since(l: &Lake, kind: DeclarationKind, name: &str, at: Position) -> Held {
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let (now, then) = (
        described(cat, own, None, kind).await?,
        described(cat, own, Some(Position(at.0 - 1)), kind).await?,
    );
    ensure!(
        !now.iter().any(|n| n == name) && then.iter().any(|n| n == name),
        "`describe` must not list `{name}` at the head and must as of before it: {now:?}, {then:?}"
    );
    Ok(())
}

async fn fired(
    store: &dyn LakeStore,
    after: Position,
    name: &str,
) -> Result<Vec<Position>, Failure> {
    let events = events_after(store, after).await?;
    let fired = events.into_iter().filter(
        |e| matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == name),
    );
    Ok(fired.map(|e| e.position).collect())
}

async fn a_withdrawn_trigger_stops_firing_and_is_seen_as_of_before(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    let capped = Declaration::Trigger(TriggerDecl {
        name: "capped".into(),
        on: "job".into(),
        when: None,
        description: None,
        max_depth: Some(1),
        when_strict: false,
        on_changes: None,
    });
    cat.declare(own, capped.clone())
        .await
        .must("declaring `capped`")?;
    ensure!(
        limits().max_reactive_depth > 2,
        "the lake's ceiling must be above 2 for the check to mean anything"
    );
    let head = store.head().await.must("reading the head")?;
    l.upsert("job", "j0", json!({"n": 0})).await?;
    let firing = *fired(store.as_ref(), head, "capped")
        .await?
        .last()
        .ok_or_else(|| Failure::new("the first write must fire `capped`"))?;
    let before = store.head().await.must("reading the head")?;
    let gone = cat
        .declare(own, withdraw(DeclarationKind::Trigger, "capped"))
        .await;
    let (at, d, _) = withdrawn(store.as_ref(), before, gone, "withdrawing `capped`").await?;
    let want = (DeclarationKind::Trigger, "capped", 2, at, true);
    ensure!(
        (d.kind, d.name.as_str(), d.version, d.since, d.appended) == want,
        "the withdrawal must answer {want:?}; it answers {d:?}"
    );
    // Opened again, the as-of read reaches the engine's kept states.
    let store = (given.reopen)(store, Seams::default()).await;
    let store = store.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    gone_since(&l, DeclarationKind::Trigger, "capped", at).await?;
    let head = store.head().await.must("reading the head")?;
    l.upsert("job", "j1", json!({"n": 1})).await?;
    ensure!(
        fired(store.as_ref(), head, "capped").await?.is_empty(),
        "a withdrawn trigger must not fire"
    );
    let deep = store.catalog(reactive(2, Some(firing)));
    try_upsert(deep.as_ref(), own, "job", "j2", json!({"n": 2}))
        .await
        .must("a depth-2 reaction naming a firing of the withdrawn `capped`")?;
    let before = store.head().await.must("reading the head")?;
    let again = cat
        .declare(own, withdraw(DeclarationKind::Trigger, "capped"))
        .await;
    let e = refused_unmoved(store.as_ref(), before, "withdrawing `capped` again", again).await?;
    let case = json!({"code": "withdrawn_undeclared", "kind": "trigger", "name": "capped"});
    ensure!(
        invalid_like(&e, &case),
        "withdrawing `capped` again must be refused as {case}: {e:?}"
    );
    let Outcome::Applied { declared, .. } = cat
        .declare(own, capped)
        .await
        .must("declaring `capped` again")?
    else {
        return Err(Failure::new("declaring `capped` again must be applied"));
    };
    ensure!(
        declared.first().is_some_and(|d| d.version == 3),
        "`capped` declared again must take version 3: {declared:?}"
    );
    let head = store.head().await.must("reading the head")?;
    l.upsert("job", "j3", json!({"n": 3})).await?;
    let Some(&own_firing) = fired(store.as_ref(), head, "capped").await?.last() else {
        return Err(Failure::new("`capped` declared again must fire"));
    };
    // It binds its own firings.
    let deep = store.catalog(reactive(2, Some(own_firing)));
    let bound = try_upsert(deep.as_ref(), own, "job", "j5", json!({"n": 5})).await;
    ensure!(
        matches!(&bound, Err(e) if reason(e) == Some(&Refusal::TooDeep { max: 1 })),
        "a depth-2 reaction to a firing of `capped` declared again must be refused `too_deep`: {bound:?}"
    );
    let deep = store.catalog(reactive(2, Some(firing)));
    try_upsert(deep.as_ref(), own, "job", "j4", json!({"n": 4}))
        .await
        .must("a depth-2 reaction to the withdrawn `capped`, the name declared again")?;
    Ok(())
}

/// Fails unless each withdrawal is refused as its case, appending nothing.
async fn refused_each(
    l: &Lake,
    store: &dyn LakeStore,
    refusals: Vec<(Declaration, serde_json::Value)>,
) -> Held {
    for (d, case) in refusals {
        let before = store.head().await.must("reading the head")?;
        let what = format!("withdrawing {}", d.what());
        let e = refused_unmoved(store, before, &what, l.cat.declare(&l.own, d).await).await?;
        ensure!(
            invalid_like(&e, &case),
            "{what} must be refused as {case}: {e:?}"
        );
    }
    Ok(())
}

async fn a_function_is_withdrawn_once_no_field_names_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for f in ["double", "fare", "spare", "third"] {
        cat.declare(own, Declaration::Function(function(f)))
            .await
            .must("declaring the functions")?;
    }
    let total = computed_by("total", StorageKind::Number, "double", &["n"]);
    l.declare(type_decl(
        "sheet",
        vec![field("n", StorageKind::Number), total],
    ))
    .await?;
    let leg = RelationDecl {
        name: "leg".into(),
        target: "trip".into(),
        fields: vec![
            field("n", StorageKind::Number),
            computed_by("cost", StorageKind::Number, "fare", &["n"]),
        ],
        ..RelationDecl::default()
    };
    let mut trip = type_decl("trip", vec![field("n", StorageKind::Number)]);
    trip.relations.push(leg);
    l.declare(trip).await?;
    let refusals = [
        (
            withdraw(DeclarationKind::Function, "double"),
            json!({"code": "withdrawn_in_use", "kind": "function", "name": "double",
                   "by": "the field `total` of `sheet`"}),
        ),
        (
            withdraw(DeclarationKind::Function, "fare"),
            json!({"code": "withdrawn_in_use", "kind": "function", "name": "fare",
                   "by": "the field `cost` of `trip`'s relation `leg`"}),
        ),
        (
            withdraw(DeclarationKind::Setting, "app.rate"),
            json!({"code": "withdrawn_kind", "kind": "setting"}),
        ),
    ];
    refused_each(&l, store.as_ref(), refusals.into()).await?;
    // `trip` migrated without `leg` leaves its link type standing, whose
    // `cost` `fare` computes still: named at the link type.
    l.migrate(type_decl("trip", vec![field("n", StorageKind::Number)]))
        .await?;
    let alone = (
        withdraw(DeclarationKind::Function, "fare"),
        json!({"code": "withdrawn_in_use", "kind": "function", "name": "fare",
               "by": "the field `cost` of `sys.link.trip.leg`"}),
    );
    refused_each(&l, store.as_ref(), vec![alone]).await?;
    let before = store.head().await.must("reading the head")?;
    let gone = cat
        .apply(own, vec![withdraw(DeclarationKind::Function, "spare")])
        .await;
    let what = "applying the withdrawal of `spare`";
    let (at, d, still) = withdrawn(store.as_ref(), before, gone, what).await?;
    ensure!(
        d.version == 2 && still,
        "{what} must take version 2 and plan to move no record: {d:?}, plan moves none: {still}"
    );
    gone_since(&l, DeclarationKind::Function, "spare", at).await?;
    let again = cat
        .declare(own, Declaration::Function(function("spare")))
        .await;
    let again = again.must("declaring `spare` again")?;
    ensure!(
        matches!(&again, Outcome::Applied { declared, .. } if declared.first().is_some_and(|d| d.version == 3)),
        "`spare` declared again must take version 3: {again:?}"
    );
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::Function, "third")],
        plan: None,
    };
    let proposed = cat.propose(own, write, "unused".into()).await;
    let Outcome::Proposed { proposal, .. } =
        proposed.must("proposing the withdrawal of `third`")?
    else {
        return Err(Failure::new("the withdrawal of `third` must be proposed"));
    };
    let accepted = cat.accept(&reviewer(store.as_ref(), true), &proposal).await;
    accepted.must("accepting the withdrawal of `third`")?;
    let now = described(cat.as_ref(), own, None, DeclarationKind::Function).await?;
    ensure!(
        now == ["double", "fare", "spare"],
        "after the accept `third` must not be described: {now:?}"
    );
    Ok(())
}

async fn a_type_is_withdrawn_once_it_holds_no_live_record_and_nothing_names_it(
    store: Store,
) -> Held {
    use DeclarationKind::{ObjectSet, Trigger, Type, Verb};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let leg = RelationDecl {
        name: "leg".into(),
        target: "ledger".into(),
        fields: vec![field("n", StorageKind::Number)],
        ..RelationDecl::default()
    };
    let to = RelationDecl {
        name: "to".into(),
        ..leg.clone()
    };
    let mut ledger = type_decl("ledger", vec![field("n", StorageKind::Number)]);
    ledger.relations.push(leg);
    l.declare(ledger.clone()).await?;
    let link = "sys.link.ledger.leg";
    l.declare(type_decl("entry", vec![reference("of", link)]))
        .await?;
    let n = json!({"name": "n", "type": {"kind": "number"}});
    l.declare_json(
        json!({"declare": "type", "name": "gauge", "key": ["n"], "fields": [n],
        "view": {"source": "ledger", "group_by": ["n"], "measures": []}}),
    )
    .await?;
    l.declare(type_decl("hold", vec![field("n", StorageKind::Number)]))
        .await?;
    l.declare_json(
        json!({"declare": "verb", "name": "vg", "on": "hold", "risk": "low",
        "effect": "write", "set": [{"field": "n", "value": "1"}],
        "payload": [{"name": "of", "type": {"kind": "reference", "target": "ledger"}}]}),
    )
    .await?;
    for d in [
        write_verb("ve", "ledger", "n", "1"),
        object_set_on("big", "ledger", "n > 1"),
        trigger_on("tr", "ledger", None),
    ] {
        (l.cat.declare(&l.own, d).await).must("declaring what names `ledger`")?;
    }
    let in_use = |name: &str, by: &str| {
        let case = json!({"code": "withdrawn_in_use", "kind": "type", "name": name, "by": by});
        (withdraw(Type, name), case)
    };
    let refusals = vec![
        in_use(link, "the relation `leg` of `ledger`"),
        in_use("sys.setting", "the lake"),
    ];
    refused_each(&l, s, refusals).await?;
    for (by, kind, name) in [
        ("the field `of` of `entry`", Type, "entry"),
        ("the view `gauge`", Type, "gauge"),
        ("the verb `ve`", Verb, "ve"),
        ("the verb `vg`", Verb, "vg"),
        ("the object set `big`", ObjectSet, "big"),
        ("the trigger `tr`", Trigger, "tr"),
    ] {
        refused_each(&l, s, vec![in_use("ledger", by)]).await?;
        withdrawn_at(&l, s, kind, name, 2).await?;
    }
    l.upsert("ledger", "l1", json!({"n": 2})).await?;
    refused_each(&l, s, vec![in_use("ledger", "its records")]).await?;
    l.retire("ledger", "l1").await?;
    let mut fan = type_decl("fan", vec![]);
    fan.relations.push(to);
    l.declare(fan).await?;
    refused_each(&l, s, vec![in_use("ledger", "the relation `to` of `fan`")]).await?;
    // The two leave by one call, the pointed-at first, `fan`'s link type
    // with it, `ledger`'s by its own withdrawal.
    let before = s.head().await.must("reading the head")?;
    let both = vec![
        withdraw(Type, "ledger"),
        withdraw(Type, link),
        withdraw(Type, "fan"),
    ];
    (l.cat.apply(&l.own, both).await).must("withdrawing `fan` and `ledger`")?;
    let events = events_after(s, before).await?;
    let gone = events.iter().filter_map(|e| match &e.body {
        EventBody::Declared {
            declaration: Declaration::Withdraw(w),
            ..
        } => Some(w.name.as_str()),
        _ => None,
    });
    let gone: Vec<&str> = gone.collect();
    let names = ["ledger", link, "fan", "sys.link.fan.to"];
    ensure!(
        gone == names && events.len() == 4,
        "withdrawing `fan` and `ledger` must append their `declared` and their link types': {events:?}"
    );
    for name in names {
        gone_since(&l, Type, name, events[0].position).await?;
    }
    let held = json!({"code": "withdrawn_name_held", "type": "ledger"});
    refused_each(&l, s, vec![(Declaration::Type(ledger), held)]).await?;
    let entry = type_decl("entry", vec![field("n", StorageKind::Number)]);
    let again = l.declare(entry).await?;
    ensure!(
        matches!(&again, Outcome::Applied { declared, .. } if declared.first().is_some_and(|d| d.version == 3)),
        "`entry` declared again must take version 3: {again:?}"
    );
    Ok(())
}

/// One `declared` withdrawal by `declare` as the owner after the head,
/// answered at `version`: the position it lands at.
async fn withdrawn_at(
    l: &Lake,
    store: &dyn LakeStore,
    kind: DeclarationKind,
    name: &str,
    version: u32,
) -> Result<Position, Failure> {
    let before = store.head().await.must("reading the head")?;
    let gone = l.cat.declare(&l.own, withdraw(kind, name)).await;
    let what = format!("withdrawing `{name}`");
    let (at, d, _) = withdrawn(store, before, gone, &what).await?;
    ensure!(
        d.version == version,
        "{what} must take version {version}: {d:?}"
    );
    Ok(at)
}

async fn a_withdrawn_verb_takes_no_call_and_frees_its_tool_name(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_json(json!({"declare": "type", "name": "order", "fields": [
        {"name": "status", "type": {"kind": "text"}, "authority": "verb", "initial": "open"},
        {"name": "rank", "type": {"kind": "number"}, "authority": "verb", "initial": 1},
        {"name": "memo", "type": {"kind": "text"}}]}))
        .await?;
    // The name the generator gives the `find` door of a type `note`, read
    // off the lake's own tools as if `note` stood in it.
    let mut lake = l.describe().await?;
    let mut note = (lake
        .types
        .iter()
        .find(|t| t.name.as_str() == "order")
        .cloned())
    .ok_or_else(|| Failure::new("`order` must be described"))?;
    note.name = "note".into();
    lake.types.push(note);
    let made = lakelands_api::tools::tools(&lake).unwrap_or_default();
    let find = |t: &&lakelands_api::tools::ToolSpec| matches!(&t.door, lakelands_api::tools::Door::Find { ty } if ty.as_str() == "note");
    let clash = made.iter().find(find).map(|t| t.name.clone());
    let clash = clash.ok_or_else(|| Failure::new("a type `note` must have a `find` tool"))?;
    for (name, risk, field, value) in [
        ("ship", "low", "status", "'shipped'"),
        ("hold", "medium", "status", "'held'"),
        ("bump", "low", "rank", "rank + 1"),
        (clash.as_str(), "low", "memo", "'noted'"),
    ] {
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "order", "risk": risk,
            "effect": "write", "set": [{"field": field, "value": value}]}),
        )
        .await?;
    }
    l.upsert("order", "o1", json!({"memo": "a"})).await?;
    let (name, key, opts) = ("hold".into(), "o1".into(), WriteOpts::default());
    let called = l
        .cat
        .call(&l.own, &name, Some(&key), patch(json!({})), opts)
        .await;
    let Outcome::Proposed { proposal, .. } = called.must("calling `hold`")? else {
        return Err(Failure::new("a call of the medium `hold` must be proposed"));
    };
    let again = l.cat.call(
        &l.own,
        &name,
        Some(&key),
        patch(json!({})),
        WriteOpts::default(),
    );
    let Outcome::Proposed {
        proposal: later, ..
    } = again.await.must("calling `hold` again")?
    else {
        return Err(Failure::new("a second call of `hold` must be proposed"));
    };
    // A reviewer not shown `order` is answered for the call as for an id
    // that does not exist, before the withdrawal and after it.
    let mut grants = Grants {
        default: Access::Write,
        review: true,
        ..Grants::default()
    };
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert(TypeName::from("order"), hidden);
    let blind = principal(s, PrincipalKind::Human, "blind", grants);
    let ghost = ProposalId::from("nope");
    let names = (ghost.as_str(), proposal.as_str());
    let what = "accepting the call of `hold`, `order` not shown";
    let before = l.cat.accept(&blind, &proposal).await;
    refused_alike(what, &before, l.cat.accept(&blind, &ghost).await, names)?;
    // Its listings, whole and by the type `order`.
    let listing = async || {
        let mut pages = Vec::new();
        for ty in [None, Some(TypeName::from("order"))] {
            let filter = ProposalFilter {
                ty,
                ..ProposalFilter::default()
            };
            let listed = l.cat.proposals(&blind, filter).await;
            pages.push(
                listed
                    .must("listing the proposals, `order` not shown")?
                    .data,
            );
        }
        Ok::<_, Failure>(format!("{pages:?}"))
    };
    let unlisted = listing().await?;
    let in_use = |verb: &str, field: &str| {
        let by = format!("the field `{field}` of `order`");
        (
            withdraw(DeclarationKind::Verb, verb),
            json!({"code": "withdrawn_in_use", "kind": "verb", "name": verb, "by": by}),
        )
    };
    refused_each(&l, s, vec![in_use("bump", "rank")]).await?;
    let at = withdrawn_at(&l, s, DeclarationKind::Verb, "hold", 2).await?;
    gone_since(&l, DeclarationKind::Verb, "hold", at).await?;
    // A direct call of it is a call of a verb not declared; so is the accept
    // of its call proposed before, to the owner and to a reviewer shown it
    // then, judged by the verb as it stood at the proposal.
    let not_declared = |answer: &lakelands_api::Result<Outcome>| matches!(answer, Err(Error::NotFound { missing: Missing::Verb { name } }) if name.as_str() == "hold");
    let called = (l.cat)
        .call(
            &l.own,
            &name,
            Some(&key),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        not_declared(&called),
        "a call of the withdrawn `hold` must be refused as a verb not declared: {called:?}"
    );
    let keeper = Grants {
        default: Access::Write,
        review: true,
        schema: true,
        ..Grants::default()
    };
    let keeper = principal(s, PrincipalKind::Human, "keeper", keeper);
    for who in [&l.own, &keeper] {
        let accepted = l.cat.accept(who, &proposal).await;
        ensure!(
            not_declared(&accepted),
            "the accept by `{}` of a call of the withdrawn `hold` must be refused as a verb not \
             declared: {accepted:?}",
            who.id
        );
    }
    let after = l.cat.accept(&blind, &proposal).await;
    let what = "accepting the call of the withdrawn `hold`, `order` not shown";
    refused_alike(what, &after, l.cat.accept(&blind, &ghost).await, names)?;
    let wire = |e: &lakelands_api::Result<Outcome>| format!("{:?}", e.as_ref().err());
    ensure!(
        wire(&after) == wire(&before),
        "{what} must be answered as before the withdrawal: {after:?}, {before:?}"
    );
    let listed = listing().await?;
    ensure!(
        listed == unlisted,
        "the calls of the withdrawn `hold` must be listed to a reviewer not shown `order` as \
         before the withdrawal: {listed}, before {unlisted}"
    );
    // The call stays pending until it is rejected.
    let state = async |id: &ProposalId| {
        let listed = l.cat.proposals(&l.own, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        Ok::<_, Failure>(listed.into_iter().find(|p| &p.id == id).map(|p| p.state))
    };
    let pending = state(&proposal).await?;
    let rejected = l.cat.reject(&l.own, &proposal, "gone".into()).await;
    let now = state(&proposal).await?;
    ensure!(
        pending == Some(ProposalState::Pending)
            && rejected.is_ok()
            && now == Some(ProposalState::Rejected),
        "the call of the withdrawn `hold` stays pending ({pending:?}) until it is rejected \
         ({rejected:?}, then {now:?})"
    );
    refused_each(&l, s, vec![in_use("ship", "status")]).await?;
    // That verb makes the tool name a type `note` makes too.
    withdrawn_at(&l, s, DeclarationKind::Verb, &clash, 2).await?;
    l.declare(type_decl("note", vec![field("n", StorageKind::Number)]))
        .await?;
    let again = l.declare_json(json!({"declare": "verb", "name": "hold", "on": "order",
        "risk": "low", "effect": "write", "set": [{"field": "status", "value": "'held'"}]}));
    let again = again.await?;
    ensure!(
        matches!(&again, Outcome::Applied { declared, .. } if declared.first().is_some_and(|d| d.version == 3)),
        "`hold` declared again must take version 3: {again:?}"
    );
    // A call proposed before is judged by the verb as it stood then: `hold`
    // declared again otherwise refuses it.
    let accepted = l.cat.accept(&l.own, &later).await;
    let e = accepted.must_fail("accepting the earlier call of `hold`, declared again")?;
    let case = json!({"code": "verb_redeclared", "verb": "hold"});
    ensure!(
        invalid_like(&e, &case),
        "the accept of the earlier call of `hold`, declared again, must be refused as {case}: {e:?}"
    );
    Ok(())
}

async fn a_withdrawn_set_holds_no_member_from_there_on(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    for (k, n) in [("j1", 10), ("j2", 60), ("j3", 1)] {
        l.upsert("job", k, json!({"n": n})).await?;
    }
    for (name, filter) in [
        ("big", "n > 5"),
        ("huge", "in_set('big') and n > 50"),
        ("small", "n < 5"),
        ("wide", "n > 5"),
    ] {
        l.cat
            .declare(&l.own, object_set_on(name, "job", filter))
            .await
            .must("declaring the sets")?;
    }
    l.cat
        .declare(&l.own, trigger_on("watch", "job", Some("in_set('small')")))
        .await
        .must("declaring `watch`")?;
    let in_use = |set: &str, by: &str| {
        (
            withdraw(DeclarationKind::ObjectSet, set),
            json!({"code": "withdrawn_in_use", "kind": "object_set", "name": set, "by": by}),
        )
    };
    refused_each(
        &l,
        s,
        vec![
            in_use("big", "the object set `huge`"),
            in_use("small", "the trigger `watch`"),
        ],
    )
    .await?;
    let both = vec![
        withdraw(DeclarationKind::ObjectSet, "big"),
        withdraw(DeclarationKind::ObjectSet, "huge"),
    ];
    l.cat
        .apply(&l.own, both)
        .await
        .must("withdrawing `big` and `huge` together")?;
    let before = s.head().await.must("reading the head")?;
    let gone = l
        .cat
        .apply(&l.own, vec![withdraw(DeclarationKind::ObjectSet, "wide")])
        .await;
    let (at, d, _) = withdrawn(s, before, gone, "applying the withdrawal of `wide`").await?;
    ensure!(
        d.version == 2,
        "the withdrawal of `wide` must take version 2: {d:?}"
    );
    gone_since(&l, DeclarationKind::ObjectSet, "wide", at).await?;
    let job = TypeName::from("job");
    let read = |at: Option<Position>| {
        let mut find = find_at(at);
        find.filter = Some("in_set('wide')".into());
        l.cat.find(&l.own, &job, find)
    };
    let now = read(None).await;
    let case = json!({"code": "set_undeclared", "set": "wide"});
    ensure!(
        now.as_ref().is_err_and(|e| invalid_like(e, &case)),
        "a read naming the withdrawn `wide` must be refused as {case}: {now:?}"
    );
    let then = read(Some(Position(at.0 - 1)))
        .await
        .must("reading `wide` as of before")?;
    let lakelands_api::Found::Records { rows, .. } = then.data else {
        return Err(Failure::new(
            "a read of `wide` as of before must answer records",
        ));
    };
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort_unstable();
    ensure!(
        keys == ["j1", "j2"],
        "`wide` as of before must hold `j1` and `j2`: {keys:?}"
    );
    // Declared again over fewer records, it joins only those; `j1`, a
    // member before, joins it again at the write that takes it in.
    let before = s.head().await.must("reading the head")?;
    let again = l
        .cat
        .declare(&l.own, object_set_on("wide", "job", "n > 50"))
        .await;
    let again = again.must("declaring `wide` again")?;
    let joined: Vec<String> = events_after(s, before)
        .await?
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::ObjectSetChanged { key, joined, .. } => Some(format!("{joined} {key}")),
            _ => None,
        })
        .collect();
    ensure!(
        matches!(&again, Outcome::Applied { declared, .. } if declared.first().is_some_and(|d| d.version == 3))
            && joined == ["true j2"],
        "`wide` declared again must take version 3 and join `j2` alone: {again:?}, {joined:?}"
    );
    let up = l.upsert("job", "j1", json!({"n": 100})).await?;
    ensure!(
        matches!(&up, Outcome::Applied { set_off, .. } if set_off.joined.iter().any(|n| n == "wide")),
        "`j1` taken into `wide` again must join it: {up:?}"
    );
    Ok(())
}

async fn a_verb_or_set_withdrawal_tells_a_reader_not_shown_it_nothing(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare(type_decl("secret", vec![field("n", StorageKind::Number)]))
        .await?;
    l.declare(type_decl("person", vec![field("grade", StorageKind::Text)]))
        .await?;
    let open = vec![
        field("n", StorageKind::Number),
        reference("owner", "person"),
    ];
    l.declare(type_decl("open", open)).await?;
    for d in [
        write_verb("peek", "secret", "n", "1"),
        object_set_on("hid", "secret", "n > 0"),
        object_set_on("opened", "open", "n > 0"),
    ] {
        l.cat.declare(&l.own, d).await.must("declaring the lake")?;
    }
    // Proposed before the trigger that reads the set is declared.
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::ObjectSet, "opened")],
        plan: None,
    };
    let proposed = l.cat.propose(&l.own, write, "gone".into()).await;
    let Outcome::Proposed { proposal, .. } =
        proposed.must("proposing the withdrawal of `opened`")?
    else {
        return Err(Failure::new("the withdrawal of `opened` must be proposed"));
    };
    // `peeper` reads across `owner` a field of `person`: to one kept from
    // that field it is not there, holder of `schema` or not (055).
    let watch = trigger_on(
        "peeper",
        "open",
        Some("in_set('opened') and owner.grade = 'x'"),
    );
    (l.cat.declare(&l.own, watch).await).must("declaring `peeper`")?;
    let mut grants = Grants {
        default: Access::Read,
        schema: true,
        restricted: true,
        review: true,
        ..Grants::default()
    };
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert(TypeName::from("secret"), hidden);
    let mut far = TypeGrants::default();
    far.fields.insert("grade".into(), Access::None);
    grants.types.insert(TypeName::from("person"), far);
    let blind = principal(s, PrincipalKind::Human, "blind", grants);
    let before = s.head().await.must("reading the head")?;
    for (kind, name) in [
        (DeclarationKind::Verb, "peek"),
        (DeclarationKind::ObjectSet, "hid"),
    ] {
        let hidden = l.cat.declare(&blind, withdraw(kind, name)).await;
        let absent = l.cat.declare(&blind, withdraw(kind, "ghost")).await;
        let what = format!("withdrawing `{name}`, of a type not shown");
        refused_alike(&what, &hidden, absent, ("ghost", name))?;
    }
    // The owner is told the trigger; the reader not shown it is told
    // nothing, by `declare` and by the accept of the proposal alike.
    let by = "the trigger `peeper`";
    let in_use =
        json!({"code": "withdrawn_in_use", "kind": "object_set", "name": "opened", "by": by});
    refused_each(
        &l,
        s,
        vec![(withdraw(DeclarationKind::ObjectSet, "opened"), in_use)],
    )
    .await?;
    for (door, answer) in [
        (
            "declare",
            (l.cat)
                .declare(&blind, withdraw(DeclarationKind::ObjectSet, "opened"))
                .await,
        ),
        ("accept", l.cat.accept(&blind, &proposal).await),
    ] {
        let what = format!("withdrawing `opened` by `{door}`, which a trigger not shown watches");
        let e = answer.must_fail(&what)?;
        let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
        ensure!(
            reason(&e) == Some(&Refusal::NotGranted)
                && !wire.contains("peeper")
                && !wire.contains("grade"),
            "{what} must be not granted, naming nothing: {wire}"
        );
    }
    crate::conformance::fixture::head_unmoved(s, before, "the withdrawals not shown").await
}

/// Whether `who` is listed proposal `id`, under a filter by `ty` if any.
async fn listed(
    cat: &dyn Catalog,
    who: &lakelands_api::Principal,
    id: &ProposalId,
    ty: Option<&str>,
) -> Result<bool, Failure> {
    let filter = ProposalFilter {
        ty: ty.map(TypeName::from),
        ..ProposalFilter::default()
    };
    let listed = cat
        .proposals(who, filter)
        .await
        .must("listing the proposals")?
        .data;
    Ok(listed.iter().any(|p| &p.id == id))
}

async fn a_withdrawal_tells_a_reader_not_shown_its_type_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    cat.declare(own, Declaration::Function(function("double")))
        .await
        .must("declaring `double`")?;
    let total = computed_by("total", StorageKind::Number, "double", &["n"]);
    l.declare(type_decl(
        "secret",
        vec![field("n", StorageKind::Number), total],
    ))
    .await?;
    l.declare(type_decl("open", vec![field("n", StorageKind::Number)]))
        .await?;
    cat.declare(own, trigger_on("watch", "secret", None))
        .await
        .must("declaring `watch`")?;
    let mut grants = Grants {
        default: Access::Read,
        schema: true,
        restricted: true,
        review: true,
        ..Grants::default()
    };
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert(TypeName::from("secret"), hidden);
    let blind = principal(store.as_ref(), PrincipalKind::Human, "blind", grants);
    let before = store.head().await.must("reading the head")?;
    let hidden = cat
        .declare(&blind, withdraw(DeclarationKind::Trigger, "watch"))
        .await;
    let absent = cat
        .declare(&blind, withdraw(DeclarationKind::Trigger, "ghost"))
        .await;
    refused_alike(
        "withdrawing a trigger on a type not shown",
        &hidden,
        absent,
        ("ghost", "watch"),
    )?;
    // Beside a declaration refused of its own, the set is refused alike.
    let deep = Declaration::Trigger(TriggerDecl {
        name: "deep".into(),
        on: "open".into(),
        when: None,
        description: None,
        max_depth: Some(limits().max_reactive_depth + 1),
        when_strict: false,
        on_changes: None,
    });
    let set = |name: &str| vec![withdraw(DeclarationKind::Trigger, name), deep.clone()];
    let hidden = cat.apply(&blind, set("watch")).await;
    let absent = cat.apply(&blind, set("ghost")).await;
    refused_alike(
        "applying a set that withdraws a trigger not shown",
        &hidden,
        absent,
        ("ghost", "watch"),
    )?;
    // A function that computes a field it is not shown is not its to withdraw.
    let double = || withdraw(DeclarationKind::Function, "double");
    let proposed = ProposedWrite::Declare {
        declarations: vec![double()],
        plan: None,
    };
    for (door, answer) in [
        ("declare", cat.declare(&blind, double()).await),
        ("apply", cat.apply(&blind, vec![double()]).await),
        (
            "propose",
            cat.propose(&blind, proposed, "gone".into()).await,
        ),
    ] {
        let e = answer.must_fail(&format!("withdrawing `double` by `{door}`"))?;
        let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
        ensure!(
            reason(&e) == Some(&Refusal::NotGranted)
                && !wire.contains("total")
                && !wire.contains("secret"),
            "withdrawing `double` by `{door}` must be not granted, naming nothing: {wire}"
        );
    }
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "withdrawing `watch`")
        .await?;
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::Trigger, "watch")],
        plan: None,
    };
    let proposed = cat.propose(own, write, "gone".into()).await;
    let Outcome::Proposed { proposal, .. } =
        proposed.must("proposing the withdrawal of `watch`")?
    else {
        return Err(Failure::new("the withdrawal of `watch` must be proposed"));
    };
    let told = |what: String, proposal: ProposalId| {
        let (cat, blind, own) = (cat.clone(), blind.clone(), own.clone());
        async move {
            let (to_blind, to_owner, by_type) = (
                listed(cat.as_ref(), &blind, &proposal, None).await?,
                listed(cat.as_ref(), &own, &proposal, None).await?,
                listed(cat.as_ref(), &own, &proposal, Some("secret")).await?,
            );
            ensure!(
                !to_blind && to_owner && by_type,
                "{what} must be listed to the owner and not to a reader not shown `secret`: \
                 owner {to_owner} (by `secret` {by_type}), reader {to_blind}"
            );
            Ok::<(), Failure>(())
        }
    };
    told("the pending withdrawal of `watch`".into(), proposal.clone()).await?;
    cat.accept(own, &proposal)
        .await
        .must("accepting the withdrawal")?;
    told(
        "the accepted withdrawal of `watch`".into(),
        proposal.clone(),
    )
    .await?;
    // Judged as proposed whatever stands under the name since: `watch`
    // once its withdrawal is decided, `peep` while its withdrawal waits,
    // each declared again on `open`.
    cat.declare(own, trigger_on("peep", "secret", None))
        .await
        .must("declaring `peep`")?;
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::Trigger, "peep")],
        plan: None,
    };
    let proposed = cat.propose(own, write, "gone".into()).await;
    let Outcome::Proposed { proposal: peep, .. } =
        proposed.must("proposing the withdrawal of `peep`")?
    else {
        return Err(Failure::new("the withdrawal of `peep` must be proposed"));
    };
    for name in ["watch", "peep"] {
        cat.declare(own, trigger_on(name, "open", None))
            .await
            .must("declaring a trigger again on `open`")?;
    }
    told(
        "the accepted withdrawal of `watch`, `watch` on `open`".into(),
        proposal,
    )
    .await?;
    told(
        "the pending withdrawal of `peep`, `peep` on `open`".into(),
        peep,
    )
    .await?;
    // A trigger whose `when` reads a field a reviewer is kept from is not
    // shown to it, so neither is its withdrawal, pending or accepted: the
    // type it withdrew from is shown, and a listing would tell that the
    // trigger existed. With no `peek` at all, nothing is listed either.
    let mut kept = Grants {
        default: Access::Read,
        review: true,
        ..Grants::default()
    };
    let mut half = TypeGrants::default();
    half.fields.insert("hush".into(), Access::None);
    kept.types.insert(TypeName::from("half"), half);
    let kept = principal(store.as_ref(), PrincipalKind::Human, "kept", kept);
    let fields = vec![
        field("n", StorageKind::Number),
        field("hush", StorageKind::Number),
    ];
    l.declare(type_decl("half", fields)).await?;
    cat.declare(own, trigger_on("peek", "half", Some("hush > 0")))
        .await
        .must("declaring `peek`")?;
    ensure!(
        !described(cat.as_ref(), &kept, None, DeclarationKind::Trigger)
            .await?
            .contains(&"peek".to_string()),
        "`peek`, which reads `half.hush`, must not be described to a reader kept from it"
    );
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::Trigger, "peek")],
        plan: None,
    };
    let proposed = cat.propose(own, write, "gone".into()).await;
    let Outcome::Proposed { proposal: peek, .. } =
        proposed.must("proposing the withdrawal of `peek`")?
    else {
        return Err(Failure::new("the withdrawal of `peek` must be proposed"));
    };
    for (what, filter) in [("pending", None), ("pending, by `half`", Some("half"))] {
        ensure!(
            listed(cat.as_ref(), own, &peek, filter).await?
                && !listed(cat.as_ref(), &kept, &peek, filter).await?,
            "the {what} withdrawal of `peek` must be listed to the owner and not to a reader kept from `half.hush`"
        );
    }
    cat.accept(own, &peek)
        .await
        .must("accepting the withdrawal of `peek`")?;
    for (what, filter) in [("accepted", None), ("accepted, by `half`", Some("half"))] {
        ensure!(
            listed(cat.as_ref(), own, &peek, filter).await?
                && !listed(cat.as_ref(), &kept, &peek, filter).await?,
            "the {what} withdrawal of `peek` must be listed to the owner and not to a reader kept from `half.hush`"
        );
    }
    ensure!(
        !described(cat.as_ref(), own, None, DeclarationKind::Trigger)
            .await?
            .contains(&"peek".to_string()),
        "`peek` must be withdrawn"
    );
    let told = cat
        .proposals(
            &kept,
            ProposalFilter {
                ty: Some(TypeName::from("half")),
                ..ProposalFilter::default()
            },
        )
        .await
        .must("listing the proposals on `half`")?
        .data;
    ensure!(
        told.is_empty(),
        "a reader kept from `half.hush` must be listed nothing on `half`, as with no `peek` at all: {told:?}"
    );
    // A withdrawal its reviewer accepts is refused as the lake is shown to
    // it: `spare`, taken into use since by `secret` alone, is not granted,
    // naming nothing; taken into use by `vista` too, it names `vista`'s field.
    cat.declare(own, Declaration::Function(function("spare")))
        .await
        .must("declaring `spare`")?;
    let write = ProposedWrite::Declare {
        declarations: vec![withdraw(DeclarationKind::Function, "spare")],
        plan: None,
    };
    let proposed = cat.propose(own, write, "gone".into()).await;
    let Outcome::Proposed {
        proposal: spare, ..
    } = proposed.must("proposing the withdrawal of `spare`")?
    else {
        return Err(Failure::new("the withdrawal of `spare` must be proposed"));
    };
    let extra = || computed_by("extra", StorageKind::Number, "spare", &["n"]);
    let total = computed_by("total", StorageKind::Number, "double", &["n"]);
    let fields = vec![field("n", StorageKind::Number), total, extra()];
    l.declare(type_decl("secret", fields)).await?;
    let e = cat
        .accept(&blind, &spare)
        .await
        .must_fail("accepting the withdrawal of `spare`, which only `secret` reads")?;
    let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
    ensure!(
        reason(&e) == Some(&Refusal::NotGranted)
            && !wire.contains("secret")
            && !wire.contains("extra"),
        "accepting the withdrawal of `spare`, which only `secret` reads, must be not granted, naming nothing: {wire}"
    );
    // On a type that sorts after `secret`, so the field named is the one
    // its reviewer is shown, not the first of the whole lake.
    l.declare(type_decl(
        "vista",
        vec![field("n", StorageKind::Number), extra()],
    ))
    .await?;
    let e = cat
        .accept(&blind, &spare)
        .await
        .must_fail("accepting the withdrawal of `spare`, which `vista` reads too")?;
    let case = json!({"code": "withdrawn_in_use", "kind": "function", "name": "spare",
        "by": "the field `extra` of `vista`"});
    ensure!(
        invalid_like(&e, &case),
        "accepting the withdrawal of `spare`, which `vista` reads too, must be refused as {case}: {e:?}"
    );
    Ok(())
}

async fn the_last_verb_that_retires_a_type_is_not_withdrawn(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let order = |by: &str| {
        json!({"declare": "type", "name": "order", "retired_by": by, "fields": [
            {"name": "memo", "type": {"kind": "text"}}]})
    };
    l.declare_json(order("verbs")).await?;
    for name in ["cancel", "void"] {
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "order", "risk": "low",
            "effect": "retire"}),
        )
        .await?;
    }
    withdrawn_at(&l, s, DeclarationKind::Verb, "cancel", 2).await?;
    let last = (
        withdraw(DeclarationKind::Verb, "void"),
        json!({"code": "withdrawn_in_use", "kind": "verb", "name": "void",
               "by": "the type `order`"}),
    );
    refused_each(&l, s, vec![last]).await?;
    l.declare_json(order("authors")).await?;
    withdrawn_at(&l, s, DeclarationKind::Verb, "void", 2).await?;
    Ok(())
}
