//! A trigger or a function withdrawn: out of force from the withdrawal on,
//! still there to a read as of before it
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended). A withdrawal is one `declared` event; what something in
//! force reads is not withdrawn, and a name not in force is not either.

use lakelands_api::event::EventBody;
use lakelands_api::ontology::{RelationDecl, StorageKind, TriggerDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::seam::Seams;
use lakelands_api::write::{
    Declaration, DeclarationKind, FunctionDecl, InForce, ProposalFilter, ProposedWrite, Withdrawal,
};
use lakelands_api::{
    Access, Catalog, Grants, LakeStore, Outcome, Position, PrincipalKind, ProposalId, Refusal,
    TypeName,
};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, computed_by, events_after, field, invalid_like, limits, principal, reactive,
    read_at, reason, refused_alike, refused_unmoved, reviewer, trigger_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    WITHDRAWN_TRIGGER = a_withdrawn_trigger_stops_firing_and_is_seen_as_of_before,
    "027, extended (lakelands#1219): a trigger withdrawn by `declare` is one `declared` event, answered in force as its kind and name at the next version, appended; with the lake opened again, `describe` no longer lists it and a describe as of before still does; the next write does not fire it, and a reaction whose `cause` names an earlier firing of it is judged by the lake's `max_reactive_depth` alone, past the trigger's own `max_depth`; withdrawn again it is refused `withdrawn_undeclared`, appending nothing; declared again its version continues and it fires, its own `max_depth` binding a reaction to its own firing, and the reaction to the earlier firing is still judged by the lake's ceiling alone",
    given
);

check!(
    WITHDRAWN_FUNCTION = a_function_is_withdrawn_once_no_field_names_it,
    "027, extended (lakelands#1219): a function a field computes is refused `withdrawn_in_use`, naming the function and that field, appending nothing; so is one a relation's link field computes, named at the relation, and, the relation gone and its link type left standing, named at the link type; one no field names is withdrawn by `apply`, its plan moving no record, gone from `describe` and there as of before, and declared again its version continues; withdrawn by `propose` and its accept; a type and a setting are refused `withdrawn_kind`, naming the kind"
);

check!(
    WITHDRAWAL_TWIN = a_withdrawal_tells_a_reader_not_shown_its_type_nothing,
    "027 and 024, extended (lakelands#1219): to a holder of `schema` not shown a trigger's type, withdrawing the trigger is refused as withdrawing a name never declared, word for word, appending nothing, alone and by `apply` beside a declaration refused of its own; withdrawing a function that computes a field of that type is not granted by `declare`, `apply` and `propose`, naming neither; the owner's proposal to withdraw the trigger is not listed to that reader, pending or accepted, and is to the owner, under a filter by the trigger's type too, and so once a trigger of that name stands on a type the reader is shown, pending or accepted; that reader's accept of a function's withdrawal is not granted, naming nothing, where only that type reads the function, and refused naming the field it is shown, of a type sorting after the hidden one, where another reads it too; a trigger whose `when` reads a field a reviewer is kept from is not shown to it, and neither is its withdrawal, pending or accepted, under a filter by its type too"
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

/// The names `who` is described of `kind` (triggers or functions), as of `at`.
async fn described(
    cat: &dyn Catalog,
    who: &lakelands_api::Principal,
    at: Option<Position>,
    triggers: bool,
) -> Result<Vec<String>, Failure> {
    let d = cat
        .describe(who, read_at(at))
        .await
        .must("describing")?
        .data;
    Ok(match triggers {
        true => d.triggers.iter().map(|t| t.name.to_string()).collect(),
        false => d.functions.iter().map(|f| f.name.clone()).collect(),
    })
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
    let (now, then) = (
        described(cat.as_ref(), own, None, true).await?,
        described(cat.as_ref(), own, Some(Position(at.0 - 1)), true).await?,
    );
    ensure!(
        !now.iter().any(|n| n == "capped") && then.iter().any(|n| n == "capped"),
        "`describe` must not list `capped` at the head and must as of before it: {now:?}, {then:?}"
    );
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
            withdraw(DeclarationKind::Type, "sheet"),
            json!({"code": "withdrawn_kind", "kind": "type"}),
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
    let (now, then) = (
        described(cat.as_ref(), own, None, false).await?,
        described(cat.as_ref(), own, Some(Position(at.0 - 1)), false).await?,
    );
    ensure!(
        !now.iter().any(|n| n == "spare") && then.iter().any(|n| n == "spare"),
        "`describe` must not list `spare` at the head and must as of before it: {now:?}, {then:?}"
    );
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
    let now = described(cat.as_ref(), own, None, false).await?;
    ensure!(
        now == ["double", "fare", "spare"],
        "after the accept `third` must not be described: {now:?}"
    );
    Ok(())
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
        !described(cat.as_ref(), &kept, None, true)
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
        !described(cat.as_ref(), own, None, true)
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
