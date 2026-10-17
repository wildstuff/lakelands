//! A trigger, a verb, an object set or a function withdrawn: out of force
//! from the withdrawal on, still there to a read as of before it
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended). A withdrawal is one `declared` event; what something in
//! force reads is not withdrawn, and a name not in force is not either.

use lakelands_api::event::EventBody;
use lakelands_api::ontology::{RelationDecl, StorageKind, TriggerDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{
    Declaration, DeclarationKind, FunctionDecl, InForce, ProposalFilter, ProposedWrite, Withdrawal,
    WriteOpts,
};
use lakelands_api::{
    Access, Catalog, Error, Grants, LakeStore, Missing, Outcome, Position, PrincipalKind,
    ProposalId, Refusal, TypeName,
};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, computed_by, events_after, field, find_at, invalid_like, limits, object_set_on,
    patch, principal, reactive, read_at, reason, refused_alike, refused_unmoved, reviewer,
    trigger_on, try_upsert, type_decl, write_verb,
};
use crate::conformance::{Failure, Held, Store};

check!(
    WITHDRAWN_TRIGGER = a_withdrawn_trigger_stops_firing_and_is_seen_as_of_before,
    "027, extended (lakelands#1219): a trigger withdrawn by `declare` is one `declared` event, answered in force as its kind and name at the next version, appended; `describe` no longer lists it and a describe as of before still does; the next write does not fire it, and a reaction whose `cause` names an earlier firing of it is judged by the lake's `max_reactive_depth` alone, past the trigger's own `max_depth`; withdrawn again it is refused `withdrawn_undeclared`, appending nothing; declared again its version continues and it fires"
);

check!(
    WITHDRAWN_FUNCTION = a_function_is_withdrawn_once_no_field_names_it,
    "027, extended (lakelands#1219): a function a field computes is refused `withdrawn_in_use`, naming the function and that field, appending nothing; so is one a relation's link field computes, named at the relation; one no field names is withdrawn by `apply`, its plan moving no record, gone from `describe` and there as of before, and declared again its version continues; withdrawn by `propose` and its accept; a type and a setting are refused `withdrawn_kind`, naming the kind"
);

check!(
    WITHDRAWAL_TWIN = a_withdrawal_tells_a_reader_not_shown_its_type_nothing,
    "027 and 024, extended (lakelands#1219): to a holder of `schema` not shown a trigger's type, withdrawing the trigger is refused as withdrawing a name never declared, word for word, appending nothing, alone and by `apply` beside a declaration refused of its own; withdrawing a function that computes a field of that type is not granted by `declare`, `apply` and `propose`, naming neither; the owner's proposal to withdraw the trigger is not listed to that reader, pending or accepted, and is to the owner, under a filter by the trigger's type too"
);

check!(
    WITHDRAWN_VERB = a_withdrawn_verb_takes_no_call_and_frees_its_tool_name,
    "027, extended (lakelands#1219): a verb that alone of its type's verbs sets a field only verbs write is refused `withdrawn_in_use`, naming that field, appending nothing; one beside another that sets it is withdrawn by `declare`, one `declared` at the next version, gone from `describe` and there as of before; a call of it proposed before is refused at the accept as a verb not declared; from there the other verb is the field's last and is refused; a verb's tool name is free once it is withdrawn, a type making that name taken; declared again its version continues"
);

check!(
    WITHDRAWN_SET = a_withdrawn_set_holds_no_member_from_there_on,
    "027 and 004, extended (lakelands#1219): an object set another set names, or a trigger's `when` names, is refused `withdrawn_in_use`, naming that set or trigger, appending nothing; the two sets withdrawn by one `apply` are taken; a set withdrawn by `apply` appends one `declared` and no `object_set_changed`, is gone from `describe` and there as of before, a read naming it refused `set_undeclared` at the head and answering its members as of before; declared again with another filter its version continues, it joins only the records that filter holds for, and a later write that takes in a record it held before joins it again"
);

check!(
    WITHDRAWN_UNSEEN = a_verb_or_set_withdrawal_tells_a_reader_not_shown_it_nothing,
    "027 and 024, extended (lakelands#1219): to a holder of `schema` not shown a type, withdrawing a verb or an object set of it is refused as withdrawing one never declared, word for word, appending nothing; withdrawing a set it is shown that a trigger it is not shown watches is not granted, naming neither the trigger nor the field that hides it"
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
        inputs: vec![field("n", StorageKind::Number)],
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

async fn a_withdrawn_trigger_stops_firing_and_is_seen_as_of_before(store: Store) -> Held {
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
    ensure!(
        !fired(store.as_ref(), head, "capped").await?.is_empty(),
        "`capped` declared again must fire"
    );
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
            withdraw(DeclarationKind::Type, "sheet"),
            json!({"code": "withdrawn_kind", "kind": "type"}),
        ),
        (
            withdraw(DeclarationKind::Setting, "app.rate"),
            json!({"code": "withdrawn_kind", "kind": "setting"}),
        ),
    ];
    for (d, case) in refusals {
        let before = store.head().await.must("reading the head")?;
        let what = format!("withdrawing {}", d.what());
        let e = refused_unmoved(store.as_ref(), before, &what, cat.declare(own, d).await).await?;
        ensure!(
            invalid_like(&e, &case),
            "{what} must be refused as {case}: {e:?}"
        );
    }
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

async fn a_withdrawn_verb_takes_no_call_and_frees_its_tool_name(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_json(json!({"declare": "type", "name": "order", "fields": [
        {"name": "status", "type": {"kind": "text"}, "authority": "verb", "initial": "open"},
        {"name": "rank", "type": {"kind": "number"}, "authority": "verb", "initial": 1},
        {"name": "memo", "type": {"kind": "text"}}]}))
        .await?;
    for (name, risk, field, value) in [
        ("ship", "low", "status", "'shipped'"),
        ("hold", "medium", "status", "'held'"),
        ("bump", "low", "rank", "rank + 1"),
        ("note_find", "low", "memo", "'noted'"),
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
    let accepted = l.cat.accept(&reviewer(s, true), &proposal).await;
    ensure!(
        matches!(&accepted, Err(Error::NotFound { missing: Missing::Verb { name } }) if name.as_str() == "hold"),
        "the accept of a call of the withdrawn `hold` must be refused as a verb not declared: {accepted:?}"
    );
    refused_each(&l, s, vec![in_use("ship", "status")]).await?;
    // `note_find` makes the tool name a type `note` makes too.
    withdrawn_at(&l, s, DeclarationKind::Verb, "note_find", 2).await?;
    l.declare(type_decl("note", vec![field("n", StorageKind::Number)]))
        .await?;
    let again = l.declare_json(json!({"declare": "verb", "name": "hold", "on": "order",
        "risk": "low", "effect": "write", "set": [{"field": "status", "value": "'held'"}]}));
    let again = again.await?;
    ensure!(
        matches!(&again, Outcome::Applied { declared, .. } if declared.first().is_some_and(|d| d.version == 3)),
        "`hold` declared again must take version 3: {again:?}"
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
    let fields = || {
        vec![
            field("n", StorageKind::Number),
            field("hush", StorageKind::Number),
        ]
    };
    l.declare(type_decl("secret", fields())).await?;
    l.declare(type_decl("open", fields())).await?;
    for d in [
        write_verb("peek", "secret", "n", "1"),
        object_set_on("hid", "secret", "n > 0"),
        object_set_on("opened", "open", "n > 0"),
        trigger_on("peeper", "open", Some("in_set('opened') and hush = 1")),
    ] {
        l.cat.declare(&l.own, d).await.must("declaring the lake")?;
    }
    let mut grants = Grants {
        default: Access::Read,
        schema: true,
        ..Grants::default()
    };
    let hidden = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert(TypeName::from("secret"), hidden);
    let mut hush = TypeGrants::default();
    hush.fields.insert("hush".into(), Access::None);
    grants.types.insert(TypeName::from("open"), hush);
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
    let e = l
        .cat
        .declare(&blind, withdraw(DeclarationKind::ObjectSet, "opened"))
        .await
        .must_fail("withdrawing `opened`, which a trigger not shown watches")?;
    let wire = serde_json::to_string(&e).map_err(|e| Failure::new(e.to_string()))?;
    ensure!(
        reason(&e) == Some(&Refusal::NotGranted)
            && !wire.contains("peeper")
            && !wire.contains("hush"),
        "withdrawing `opened`, which a trigger not shown watches, must be not granted, naming nothing: {wire}"
    );
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
    for state in ["pending", "accepted"] {
        let (to_blind, to_owner, by_type) = (
            listed(cat.as_ref(), &blind, &proposal, None).await?,
            listed(cat.as_ref(), own, &proposal, None).await?,
            listed(cat.as_ref(), own, &proposal, Some("secret")).await?,
        );
        ensure!(
            !to_blind && to_owner && by_type,
            "the {state} withdrawal of `watch` must be listed to the owner and not to a reader \
             not shown `secret`: owner {to_owner} (by `secret` {by_type}), reader {to_blind}"
        );
        if state == "pending" {
            cat.accept(own, &proposal)
                .await
                .must("accepting the withdrawal")?;
        }
    }
    Ok(())
}
