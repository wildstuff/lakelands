//! A value type's migration may rename values: every stored record that
//! holds an old value takes the new one, in one `value_type_migrated`
//! event the fold applies as a step
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, lakelands#553).
//!
//! The preflight counts through the renames, and refuses what a rename
//! cannot do: a pair that names a value twice, or a value the declaration
//! does not accept; a renamed value in a key field; a value the lake
//! computes from an expression that holds an old one. The step leaves a
//! record's version, its flags and each field's provenance as they were,
//! retired and restricted records included; what reads a renamed value is
//! computed again, the sets decide again, and a field's `initial` that
//! names an old value is declared again with the new one.

use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::{StorageKind, ValueTypeDecl};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{
    Declaration, Migration, Outcome, ProposedWrite, ValueTypeMigration, ValueTypeReport,
};
use lakelands_api::{Access, LakeStore, Position, Refusal, TypeName};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, all_found, computed_by, derived, events_after, field, field_access, find_at,
    followed, get_at, head_unmoved, migration, object_set_on, outputs_from, reason, refused_like,
    restrict, reviewer, trigger_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    RENAMES_IN_ONE_STEP = a_value_type_migration_renames_a_value_in_every_record_in_one_event,
    "027, extended (lakelands#553): a value type's migration with `renames` is preflighted through them, counting per field the values each pair rewrites, and applied as one `value_type_migrated` at the value type's next version, naming the types that use it; a record that held the old value holds the new one from that position, with its version and its field's provenance as they were, and as of the position before it holds the old; a record that held none is unchanged; a later write of the old value is refused"
);

check!(
    RENAMES_EVERY_RECORD = a_value_rename_reaches_retired_and_restricted_records_and_leaves_them_so,
    "027, extended (lakelands#553): a value type's renames reach every stored record, retired and restricted ones included, in every field of every type that uses the value type, and leave a retired record retired and a restricted one restricted"
);

check!(
    RENAMES_REFUSED = renames_that_name_a_value_twice_or_one_not_accepted_are_refused,
    "027, extended (lakelands#553): a rename of a value to itself, of a value twice, of a value the declaration in force does not accept, to one the new declaration does not accept, and any rename of a value type that is not text, are each refused as invalid, naming the pair, and nothing is appended; a stored value neither renamed nor accepted is a misfit, and the apply is refused"
);

check!(
    RENAMES_AT_ONCE = renames_apply_at_once_so_a_chain_and_a_swap_need_no_order,
    "027, extended (lakelands#553): every pair maps a value as it stood before the migration: `a → b, b → c` leaves no `a` at `c`, and `a → b, b → a` swaps them; two old values may take one new value"
);

check!(
    RENAMES_NO_KEY = a_rename_of_a_value_a_key_field_holds_is_refused,
    "027, extended (lakelands#553): a rename of a value a key field holds is refused as invalid, naming the type and the field and pointing to a re-key through the type's migration, and nothing is appended; a key field that holds no old value does not refuse it"
);

check!(
    RENAMES_COMPUTE_AGAIN = what_reads_a_renamed_value_is_computed_again_and_an_old_literal_refuses,
    "027, extended (lakelands#553): a derived field of the value type is renamed and computed again, a derived field that reads a renamed value is computed again as of the event, and a derived field of the value type whose expression holds an old value refuses the rename as invalid, naming the field"
);

check!(
    RENAMES_DECIDE_SETS = a_value_rename_decides_set_membership_in_its_append,
    "004 and 027, extended (lakelands#553): a value type's renames decide set membership again in their append, caused by the `value_type_migrated`, over the records as the step and the computation leave them: a set whose `where` names the new value, and one whose `where` reads a derived field over the renamed one; past the lake's `max_set_changes` the migration is refused `too_large`, naming the set, and appends nothing, at its apply and at the accept of its proposal (013, lakelands#935)",
    given
);

check!(
    RENAMES_INITIAL = a_rename_declares_again_a_field_initial_that_names_an_old_value,
    "027 and 050, extended (lakelands#553): a field's `initial` that names an old value takes the new one: the system declares its type again after the `value_type_migrated`, in its append, and a record made later holds the new value"
);

check!(
    RENAMES_FIRE_NOTHING = a_value_rename_fires_no_trigger,
    "021 and 027, extended (lakelands#553): a value type's renames change records by the fold, and no trigger fires at them"
);

check!(
    RENAMES_PROPOSED = a_proposed_value_rename_is_accepted_only_over_the_fields_it_reported,
    "013 and 027, extended (lakelands#553): a proposed value type's migration carries its renames and its preflight's report; accepted, the `value_type_migrated` follows the `write_accepted`, caused by it; one whose report no longer names the fields that use the value type is refused as changed, and nothing is appended"
);

check!(
    RENAMES_SHOWN = a_value_rename_is_not_granted_to_one_not_shown_every_field_it_writes,
    "024 and 027, extended (lakelands#553): a value type's migration with renames writes every field that uses it, so its preflight, its apply and its proposal are not granted, naming nothing, to a caller not shown one of them, its proposal before a stored value that would not fit is counted (013, lakelands#935)"
);

check!(
    RENAMES_IN_THE_FEED = the_feed_shows_a_value_rename_to_a_consumer_of_a_type_it_rewrites,
    "004 and 027, extended (lakelands#553): the change feed filtered by type shows a `value_type_migrated` to a consumer of a type it rewrites, and not to one of another type; and every filtered poll answers what the log does (lakelands#678)"
);

check!(
    RENAMES_REBUILT = a_rebuild_leaves_what_a_value_rename_left,
    "001 and 027, extended (lakelands#553): a projection rebuilt from a log that renames values, a page of the rebuild ending at the `value_type_migrated` over records earlier pages wrote, answers as the live one did, at the head and as of each position"
);

check!(
    RENAMES_LIST_CONDITIONS =
        a_rename_report_lists_the_declarations_and_rules_that_may_name_an_old_value,
    "024 and 027, extended (lakelands#553): the preflight of a rename lists, by kind and name, each declaration in force whose expressions hold an old value as a literal (a set, a trigger, a verb, a view, a function; a view that names it only in a merged source's `where` and a type that names it only in a relation's field's `derived` too) and each aging rule on a type that uses the value type that moves from or to one, and changes none of them; a declaration the reader is not shown (a function too, judged as `describe` judges it, lakelands#770), and a rule whose `from` it may not read, is told only as one there may be, by its preflight and by the proposal listed to it, there with its grants followed to the proposal through the renames since"
);

check!(
    RENAMES_NAME_THE_DESCRIBED = a_rename_report_names_a_type_by_what_its_reader_is_described_of_it,
    "024 and 027, extended (lakelands#861): a value type's report names a type's declaration only where the description its reader is given of the type holds the old value; held in a relation it is not shown, in a relation's field or a derived field it may not read, or in a view's definition held from it, the old value only sets `conditions_unshown`, and the report is else the one the reader was given before the member was declared; the listing of the proposed rename answers each reader that report, and an apply takes the id of its caller's own"
);

check!(
    RENAMES_KEPT_VIEW = a_kept_view_over_a_renamed_value_counts_by_the_new_one,
    "027 and the `view_kept.` setting, extended (lakelands#553): a kept view grouped by a renamed field is ended and made again at the `value_type_migrated`, and counts by the new value, as its twin computes, live and after a rebuild"
);

check!(
    RENAMES_EMBED = an_embedding_over_a_renamed_value_is_embedded_again_caused_by_the_rename,
    "022 and 027, extended (lakelands#553): an embedding made from a renamed field is due on every record the rename changed, its answers recorded with the `value_type_migrated` as their cause, one call per new text; the lake's declaration again of a type whose `initial` a rename changes settles that type's records again, as every declaration of a type with a field computed outside the lake does",
    embedding
);

check!(
    RENAMES_BACKED_MISFIT = a_backed_answer_that_holds_an_old_value_is_a_misfit_of_the_rename,
    "022 and 027, extended (lakelands#553): a field computed by a function with a backing is not renamed, since the lake does not write its answer: one that holds an old value is a misfit, named, and the rename is refused until the backing answers new values"
);

fn vt(value: Value) -> ValueTypeDecl {
    serde_json::from_value(value).expect("a value type")
}

/// The value type `status` over text, accepting `one_of`.
fn status(one_of: &[&str]) -> ValueTypeDecl {
    vt(json!({"name": "status", "kind": "text", "one_of": one_of}))
}

/// A migration of `status` to `one_of` with `renames`.
fn renamed(one_of: &[&str], renames: &[(&str, &str)]) -> ValueTypeMigration {
    ValueTypeMigration {
        to: status(one_of),
        renames: (renames.iter())
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect(),
    }
}

/// A field `name` of the value type `status`.
fn of_status(name: &str) -> lakelands_api::ontology::FieldDecl {
    let mut f = field(name, StorageKind::Text);
    f.ty.value_type = Some("status".into());
    f
}

/// `is_active`, derived from `state`.
fn is_active() -> lakelands_api::ontology::FieldDecl {
    derived(
        "is_active",
        StorageKind::Boolean,
        "state = 'active'",
        &["state"],
    )
}

/// `status` accepting `one_of`, and `task`, whose `state` uses it, with
/// `more` fields.
async fn lake(
    store: &dyn LakeStore,
    one_of: &[&str],
    more: Vec<lakelands_api::ontology::FieldDecl>,
) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare_as(Declaration::ValueType(status(one_of)), "declaring `status`")
        .await?;
    let mut fields = vec![of_status("state")];
    fields.extend(more);
    l.declare(type_decl("task", fields)).await?;
    Ok(l)
}

async fn preflight(l: &Lake, m: ValueTypeMigration) -> Result<ValueTypeReport, Failure> {
    let reply = l.cat.migrate_value_type(&l.own, m).await;
    Ok(reply.must("running a value type's preflight")?.data)
}

/// The preflight and the apply of `m`: the report, and the events the
/// apply appended.
async fn apply(
    store: &dyn LakeStore,
    l: &Lake,
    m: ValueTypeMigration,
) -> Result<(ValueTypeReport, Vec<Event>), Failure> {
    let report = preflight(l, m.clone()).await?;
    let before = store.head().await.must("reading the head")?;
    let applied = l.cat.migrate_value_type_apply(&l.own, m, &report.report_id);
    applied.await.must("applying a value type's migration")?;
    Ok((report, events_after(store, before).await?))
}

/// `m` proposed by the owner: the proposal's id.
async fn propose(l: &Lake, m: &ValueTypeMigration) -> Result<lakelands_api::ProposalId, Failure> {
    let w = ProposedWrite::MigrateValueType {
        to: Box::new(m.to.clone()),
        renames: m.renames.clone(),
        report: Default::default(),
    };
    let said = l.cat.propose(&l.own, w, "a rename".into()).await;
    match said.must("proposing a rename")? {
        Outcome::Proposed { proposal, .. } => Ok(proposal),
        other => Err(Failure::new(format!("a proposal answered {other:?}"))),
    }
}

fn state_of(r: &lakelands_api::Record) -> Option<&str> {
    r.fields.get("state").and_then(Value::as_str)
}

/// The `state` of each `task` of `keys`.
async fn states(l: &Lake, keys: &[&str]) -> Result<Vec<Option<String>>, Failure> {
    let mut out = Vec::new();
    for key in keys {
        out.push(state_of(&l.read("task", key).await?).map(str::to_owned));
    }
    Ok(out)
}

/// Every `task` as of each of `at`, with provenance.
async fn tasks(
    l: &Lake,
    at: &[Option<Position>],
) -> Result<Vec<Vec<lakelands_api::Record>>, Failure> {
    let mut out = Vec::new();
    for at in at {
        let find = find_at(*at);
        out.push(all_found(l.cat.as_ref(), &l.own, "task", find).await?);
    }
    Ok(out)
}

async fn a_value_type_migration_renames_a_value_in_every_record_in_one_event(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    l.upsert("task", "t2", json!({"state": "done"})).await?;
    let (t1, t2) = (l.read("task", "t1").await?, l.read("task", "t2").await?);
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (report, events) = apply(store.as_ref(), &l, m).await?;
    let pairs = report.fields.iter().flat_map(|u| u.renamed.iter());
    let counts: Vec<_> = pairs
        .map(|p| (p.from.as_str(), p.to.as_str(), p.records))
        .collect();
    ensure!(
        report.fits && report.misfits == 0 && counts == [("open", "active", 1)],
        "the preflight renames `open` in one record and fits; it said {report:?}"
    );
    let migrated = events.first().map(|e| (&e.body, e.position));
    let Some((EventBody::ValueTypeMigrated { version, types, .. }, at)) = migrated else {
        return Err(Failure::new(format!(
            "the apply appends a `value_type_migrated` first; it appended {events:?}"
        )));
    };
    ensure!(
        *version == 2 && types == &[TypeName::from("task")] && events.len() == 1,
        "the event puts `status` at version 2, names `task`, and is the append's only one; \
         it is {:?}",
        events
    );
    let (now1, now2) = (l.read("task", "t1").await?, l.read("task", "t2").await?);
    ensure!(
        state_of(&now1) == Some("active")
            && now1.version == t1.version
            && now1.meta.get("state") == t1.meta.get("state")
            && now2 == t2,
        "`t1` holds `active` with its version and provenance as they were, `t2` is as it was; \
         `t1` was {t1:?} and is {now1:?}, `t2` was {t2:?} and is {now2:?}"
    );
    let get = get_at("t1", Some(Position(at.0 - 1)));
    let was = l.cat.get(&l.own, &"task".into(), get).await;
    let was = was.must("reading `t1` before the event")?.data;
    ensure!(
        state_of(&was) == Some("open"),
        "as of the position before the event `t1` holds `open`; it reads {was:?}"
    );
    let open = json!({"state": "open"});
    let old = try_upsert(l.cat.as_ref(), &l.own, "task", "t3", open).await;
    ensure!(
        old.is_err(),
        "a write of the old value is refused once `status` no longer accepts it; it answered {old:?}"
    );
    Ok(())
}

async fn a_value_rename_reaches_retired_and_restricted_records_and_leaves_them_so(
    store: Store,
) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![of_status("next")]).await?;
    for (key, state) in [("t1", "open"), ("t2", "open"), ("t3", "done")] {
        l.upsert("task", key, json!({"state": state, "next": "open"}))
            .await?;
    }
    l.retire("task", "t1").await?;
    restrict(store.as_ref(), "task", "t2", "held").await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (report, _) = apply(store.as_ref(), &l, m).await?;
    let rewritten: Vec<_> = (report.fields.iter())
        .map(|u| (u.field.as_str(), u.renamed.first().map_or(0, |p| p.records)))
        .collect();
    ensure!(
        rewritten == [("state", 2), ("next", 3)],
        "the renames reach both fields of every record, the retired and the restricted one \
         included; the report counts {rewritten:?}"
    );
    // A preflight over a declaration that accepts no `open` finds none.
    let wider = ValueTypeMigration::from(status(&["active", "done", "x"]));
    let after = preflight(&l, wider).await?;
    let scanned: Vec<_> = after.fields.iter().map(|u| u.values_scanned).collect();
    ensure!(
        after.misfits == 0 && scanned == [3, 3],
        "after the rename no stored record holds `open`, of the three in both fields; a \
         preflight says {after:?}"
    );
    let retired = l.cat.get(&l.own, &"task".into(), get_at("t1", None)).await;
    let reader = crate::conformance::fixture::reads_all(store.as_ref(), "plain");
    let held = l.cat.get(&reader, &"task".into(), get_at("t2", None)).await;
    ensure!(
        retired.is_err() && held.is_err(),
        "`t1` stays retired and `t2` restricted: they read {retired:?} and {held:?}"
    );
    Ok(())
}

async fn renames_that_name_a_value_twice_or_one_not_accepted_are_refused(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done", "void"], vec![]).await?;
    let score = Declaration::ValueType(vt(json!({"name": "score", "kind": "number"})));
    l.declare_as(score, "declaring `score`").await?;
    l.upsert("task", "t1", json!({"state": "void"})).await?;
    let before = store.head().await.must("reading the head")?;
    let to = |pairs| renamed(&["active", "done", "void"], pairs);
    // Each said at its rename but `twice`, as `value_rename_<code>`.
    for (what, pairs, code) in [
        ("to itself", &[("open", "open")][..], "same"),
        ("twice", &[("open", "active"), ("open", "done")], "twice"),
        ("not in force", &[("gone", "active")], "old_refused"),
        ("to one not accepted", &[("open", "nope")], "new_refused"),
    ] {
        let (from, to_value) = pairs[0];
        let mut case = json!({"code": format!("value_rename_{code}")});
        if code != "twice" {
            case["at"] = json!([{"code": "value_rename", "from": from, "to": to_value}]);
        }
        let said = l.cat.migrate_value_type(&l.own, to(pairs)).await;
        refused_like(&format!("a rename {what}"), said, &case)?;
    }
    let number = ValueTypeMigration {
        to: vt(json!({"name": "score", "kind": "number", "constraint": "value > 0"})),
        renames: vec![("1".into(), "2".into())],
    };
    let said = l.cat.migrate_value_type(&l.own, number).await;
    let case = json!({"code": "value_rename_not_text", "value_type": "score"});
    refused_like("a rename of a number", said, &case)?;
    // `void` is neither renamed nor accepted: a misfit, and no apply.
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let report = preflight(&l, m.clone()).await?;
    let applied = l.cat.migrate_value_type_apply(&l.own, m, &report.report_id);
    let applied = applied.await;
    ensure!(
        !report.fits && report.misfits == 1 && applied.is_err(),
        "a stored `void` neither renamed nor accepted is a misfit and the apply is refused; \
         the report says {report:?}, the apply {applied:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused rename").await
}

async fn renames_apply_at_once_so_a_chain_and_a_swap_need_no_order(store: Store) -> Held {
    let l = lake(store.as_ref(), &["a", "b", "c"], vec![]).await?;
    for (key, state) in [("x1", "a"), ("x2", "b"), ("x3", "c")] {
        l.upsert("task", key, json!({ "state": state })).await?;
    }
    let chain = renamed(&["b", "c"], &[("a", "b"), ("b", "c")]);
    apply(store.as_ref(), &l, chain).await?;
    let after = states(&l, &["x1", "x2", "x3"]).await?;
    ensure!(
        after == [Some("b".into()), Some("c".into()), Some("c".into())],
        "`a → b, b → c` maps each value as it stood: `a` to `b`, `b` to `c`; the records hold \
         {after:?}"
    );
    let swap = renamed(&["b", "c"], &[("b", "c"), ("c", "b")]);
    apply(store.as_ref(), &l, swap).await?;
    let swapped = states(&l, &["x1", "x2", "x3"]).await?;
    ensure!(
        swapped == [Some("c".into()), Some("b".into()), Some("b".into())],
        "`b → c, c → b` swaps them; the records hold {swapped:?}"
    );
    let merge = renamed(&["c"], &[("b", "c")]);
    apply(store.as_ref(), &l, merge).await?;
    let merged = states(&l, &["x1", "x2", "x3"]).await?;
    ensure!(
        merged.iter().all(|s| s.as_deref() == Some("c")),
        "two values may take one: every record holds `c`; they hold {merged:?}"
    );
    Ok(())
}

async fn a_rename_of_a_value_a_key_field_holds_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    let mut keyed = type_decl("stage", vec![of_status("name")]);
    keyed.key = vec!["name".into()];
    l.declare(keyed).await?;
    l.upsert("stage", "done", json!({"name": "done"})).await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let before = store.head().await.must("reading the head")?;
    let m = renamed(
        &["active", "done"],
        &[("done", "finished"), ("open", "active")],
    );
    let m = ValueTypeMigration {
        to: status(&["active", "finished"]),
        ..m
    };
    let said = l.cat.migrate_value_type(&l.own, m).await;
    refused_as!(
        "a rename of a value a key field holds",
        said,
        Case::KeyHoldsRenamed { ty, field } if ty == "stage" && field == "name"
    )?;
    head_unmoved(store.as_ref(), before, "a refused re-key by value").await?;
    // The key holds no `open`: renaming it is no re-key.
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let report = preflight(&l, m).await?;
    ensure!(
        report.fits,
        "a key field that holds no old value does not refuse the rename; the report says \
         {report:?}"
    );
    Ok(())
}

async fn what_reads_a_renamed_value_is_computed_again_and_an_old_literal_refuses(
    store: Store,
) -> Held {
    let mut copy = derived("shown", StorageKind::Text, "state", &["state"]);
    copy.ty.value_type = Some("status".into());
    let l = lake(store.as_ref(), &["open", "done"], vec![copy, is_active()]).await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    apply(store.as_ref(), &l, m).await?;
    let t1 = l.read("task", "t1").await?;
    let (shown, active) = (t1.fields.get("shown"), t1.fields.get("is_active"));
    ensure!(
        shown == Some(&json!("active")) && active == Some(&json!(true)),
        "the derived copy holds `active` and `is_active` is computed again; `t1` reads {t1:?}"
    );
    // A derived field of the value type that yields an old value.
    let mut fixed = derived("fixed", StorageKind::Text, "'done'", &[]);
    fixed.ty.value_type = Some("status".into());
    let mut task = type_decl("task", vec![of_status("state")]);
    task.name = "chore".into();
    task.fields.push(fixed);
    l.declare(task).await?;
    let before = store.head().await.must("reading the head")?;
    let m = renamed(&["active", "finished"], &[("done", "finished")]);
    let said = l.cat.migrate_value_type(&l.own, m).await;
    refused_as!(
        "a rename of a value a derived field yields",
        said,
        Case::ComputedHoldsRenamed { ty, field } if ty == "chore" && field == "fixed"
    )?;
    head_unmoved(store.as_ref(), before, "a refused rename").await
}

async fn a_value_rename_decides_set_membership_in_its_append(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![is_active()]).await?;
    for d in [
        object_set_on("active", "task", "state = 'active'"),
        object_set_on("flagged", "task", "is_active"),
    ] {
        l.declare_as(d, "declaring a set").await?;
    }
    for (key, state) in [("t1", "open"), ("t2", "done"), ("t3", "open")] {
        l.upsert("task", key, json!({ "state": state })).await?;
    }
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (_, events) = apply(store.as_ref(), &l, m.clone()).await?;
    let at = events.first().map(|e| e.position);
    let joined: Vec<_> = (events.iter())
        .filter_map(|e| match &e.body {
            EventBody::ObjectSetChanged {
                set, key, joined, ..
            } if e.cause == at => Some((set.to_string(), key.to_string(), *joined)),
            _ => None,
        })
        .collect();
    let want = [
        ("active", "t1"),
        ("active", "t3"),
        ("flagged", "t1"),
        ("flagged", "t3"),
    ];
    let want: Vec<_> = (want.iter())
        .map(|(s, k)| ((*s).to_owned(), (*k).to_owned(), true))
        .collect();
    let mut got = joined.clone();
    got.sort();
    ensure!(
        got == want,
        "`t1` and `t3` join both sets in the append, caused by its `value_type_migrated`; it \
         changed {joined:?}"
    );
    // Past a ceiling of one, a rename that two records join a set by is
    // refused.
    let set = object_set_on("finished", "task", "state = 'finished'");
    l.declare_as(set, "declaring a set").await?;
    l.upsert("task", "t4", json!({"state": "done"})).await?;
    let m = renamed(&["active", "finished"], &[("done", "finished")]);
    let id = propose(&l, &m).await?;
    let lower = lakelands_api::location::Limits {
        max_set_changes: 1,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, lakelands_api::seam::Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a lower ceiling: {e}")))?;
    let l = Lake::of(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let accepted = l.cat.accept(&l.own, &id).await;
    ensure!(
        reason(&accepted) == Some(&Refusal::TooLarge { max: 1 }),
        "the accept of the rename past a ceiling of one is refused `too_large`; it answered \
         {accepted:?}"
    );
    let report = preflight(&l, m.clone()).await?;
    let said = l.cat.migrate_value_type_apply(&l.own, m, &report.report_id);
    let said = said.await;
    ensure!(
        reason(&said) == Some(&Refusal::TooLarge { max: 1 })
            && crate::conformance::fixture::refused_on(&said).is_some_and(|(f, _)| f == "finished"),
        "two memberships past a ceiling of one are refused `too_large`, naming `finished`; it \
         answered {said:?}"
    );
    head_unmoved(store.as_ref(), before, "a rename past the ceiling").await
}

async fn a_rename_declares_again_a_field_initial_that_names_an_old_value(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let decl = Declaration::ValueType(status(&["open", "done"]));
    l.declare_as(decl, "declaring `status`").await?;
    let mut state = of_status("state");
    state.initial = Some(json!("open"));
    let fields = vec![state, field("title", StorageKind::Text)];
    l.declare(type_decl("task", fields)).await?;
    l.upsert("task", "t1", json!({"title": "a"})).await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (report, events) = apply(store.as_ref(), &l, m).await?;
    let initial = |e: &Event| match &e.body {
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => t
            .fields
            .iter()
            .find(|f| f.name.as_str() == "state")
            .and_then(|f| f.initial.clone()),
        _ => None,
    };
    let declared: Vec<_> = events.iter().filter_map(initial).collect();
    ensure!(
        report.fits && declared == [json!("active")],
        "the field's `initial` is mapped before it is counted, and `task` is declared again \
         with `active`; the report says {report:?}, the append declared {declared:?}"
    );
    l.upsert("task", "t2", json!({"title": "b"})).await?;
    let (t1, t2) = (l.read("task", "t1").await?, l.read("task", "t2").await?);
    ensure!(
        state_of(&t1) == Some("active") && state_of(&t2) == Some("active"),
        "`t1`'s filled `open` is renamed, and `t2`, made after, is filled with `active`; they \
         read {t1:?} and {t2:?}"
    );
    Ok(())
}

async fn a_value_rename_fires_no_trigger(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    let woke = trigger_on("woke", "task", Some("state = 'active'"));
    l.declare_as(woke, "declaring a trigger").await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (_, events) = apply(store.as_ref(), &l, m).await?;
    let fired: Vec<_> = (events.iter())
        .filter(|e| matches!(e.body, EventBody::TriggerFired { .. }))
        .collect();
    ensure!(
        fired.is_empty(),
        "no trigger fires at a value type's renames; the append holds {fired:?}"
    );
    Ok(())
}

async fn a_proposed_value_rename_is_accepted_only_over_the_fields_it_reported(
    store: Store,
) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let id = propose(&l, &m).await?;
    let before = store.head().await.must("reading the head")?;
    let accepted = l.cat.accept(&l.own, &id).await;
    accepted.must("accepting the rename")?;
    let events = events_after(store.as_ref(), before).await?;
    let accepted = events.first().map(|e| e.position);
    let migrated = events
        .iter()
        .find(|e| matches!(e.body, EventBody::ValueTypeMigrated { .. }));
    let t1 = l.read("task", "t1").await?;
    ensure!(
        migrated.is_some_and(|e| e.cause == accepted) && state_of(&t1) == Some("active"),
        "accepted, the `value_type_migrated` follows the `write_accepted`, caused by it, and \
         `t1` holds `active`; the append was {events:?}"
    );
    // A type that came to use the value type since the proposal.
    let back = renamed(&["open", "done"], &[("active", "open")]);
    let id = propose(&l, &back).await?;
    l.declare(type_decl("chore", vec![of_status("state")]))
        .await?;
    let before = store.head().await.must("reading the head")?;
    let said = l.cat.accept(&l.own, &id).await;
    refused_as!(
        "an accept over a field the report did not name",
        said,
        Case::ValueTypeChangedSinceProposed { value_type } if value_type == "status"
    )?;
    head_unmoved(store.as_ref(), before, "a refused accept").await
}

async fn a_value_rename_is_not_granted_to_one_not_shown_every_field_it_writes(
    store: Store,
) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let mut blank = reviewer(store.as_ref(), true);
    (blank.grants.default, blank.grants.restricted) = (Access::None, true);
    let before = store.head().await.must("reading the head")?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let id = preflight(&l, m.clone()).await?.report_id;
    let w = ProposedWrite::MigrateValueType {
        to: Box::new(m.to.clone()),
        renames: m.renames.clone(),
        report: Default::default(),
    };
    let answers = [
        reason(&l.cat.migrate_value_type(&blank, m.clone()).await).cloned(),
        reason(&l.cat.migrate_value_type_apply(&blank, m, &id).await).cloned(),
        reason(&l.cat.propose(&blank, w, "why".into()).await).cloned(),
    ];
    ensure!(
        answers.iter().all(|a| a == &Some(Refusal::NotGranted)),
        "a caller not shown `task`.`state` is not granted the rename's preflight, apply or \
         proposal; they answered {answers:?}"
    );
    head_unmoved(store.as_ref(), before, "a rename not granted").await?;
    // Before a stored value that would not fit is counted.
    l.upsert("task", "t2", json!({"state": "done"})).await?;
    let tight = renamed(&["active"], &[("open", "active")]);
    let proposal = || ProposedWrite::MigrateValueType {
        to: Box::new(tight.to.clone()),
        renames: tight.renames.clone(),
        report: Default::default(),
    };
    let blank_said = l.cat.propose(&blank, proposal(), "why".into()).await;
    let own_said = l.cat.propose(&l.own, proposal(), "why".into()).await;
    ensure!(
        reason(&blank_said) == Some(&Refusal::NotGranted)
            && matches!(reason(&own_said), Some(Refusal::Invalid(..))),
        "a rename that would leave `done` unfit is not granted to one not shown `task`.`state`, \
         and invalid to the owner; they answered {blank_said:?} and {own_said:?}"
    );
    Ok(())
}

async fn the_feed_shows_a_value_rename_to_a_consumer_of_a_type_it_rewrites(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let before = store.head().await.must("reading the head")?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    apply(store.as_ref(), &l, m).await?;
    let of = |ty: &str| FeedFilter {
        kinds: vec!["value_type_migrated".into()],
        types: vec![ty.into()],
        ..FeedFilter::default()
    };
    let task = followed(store.as_ref(), before.next(), &of("task")).await?;
    let note = followed(store.as_ref(), before.next(), &of("note")).await?;
    ensure!(
        task.len() == 1 && note.is_empty(),
        "a consumer of `task` hears of the rename and one of `note` does not; they read \
         {task:?} and {note:?}"
    );
    super::feed::the_index_answers_the_log(store.as_ref(), "a value renamed").await
}

async fn a_rebuild_leaves_what_a_value_rename_left(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![is_active()]).await?;
    let set = object_set_on("active", "task", "state = 'active'");
    l.declare_as(set, "declaring a set").await?;
    // The rename is the last event of the rebuild's first page of 1 024
    // (positions 0 to 1 023), over records that page wrote.
    let mut n: usize = 0;
    while store.head().await.must("reading the head")?.0 < 1_022 {
        let state = if n.is_multiple_of(2) { "open" } else { "done" };
        l.upsert("task", &format!("t{n}"), json!({ "state": state }))
            .await?;
        n += 1;
    }
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (_, events) = apply(store.as_ref(), &l, m).await?;
    let first = events.first().map(|e| e.position);
    ensure!(
        first == Some(Position(1_023)),
        "the fixture puts the rename at position 1 023; it is at {first:?}"
    );
    l.upsert("task", "late", json!({"state": "active"})).await?;
    let at = [Some(Position(1_022)), Some(Position(1_023)), None];
    let live = tasks(&l, &at).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = tasks(&l, &at).await?;
    ensure!(
        live == rebuilt,
        "the rebuilt projection answers as the live one did, before the rename, at it and at \
         the head"
    );
    let active = (live[2].iter()).filter(|r| r.fields.get("is_active") == Some(&json!(true)));
    ensure!(
        active.count() == n.div_ceil(2) + 1,
        "every renamed record, and the one written after, is active"
    );
    Ok(())
}

async fn a_rename_report_lists_the_declarations_and_rules_that_may_name_an_old_value(
    store: Store,
) -> Held {
    let more = ["note", "flag", "secret"].map(|f| field(f, StorageKind::Text));
    let l = lake(store.as_ref(), &["open", "done"], more.into()).await?;
    l.declare_json(
        json!({"declare": "function", "name": "opens", "backing": "expr",
        "inputs": [{"name": "note", "type": {"kind": "text"}}], "expr": "note = 'open'"}),
    )
    .await?;
    let opens = computed_by("opens", StorageKind::Boolean, "opens", &["note"]);
    let added = l.cat.add_field(&l.own, &"task".into(), opens.clone()).await;
    added.must("adding `opens`")?;
    let fields = ["code", "note"].map(|f| field(f, StorageKind::Text));
    let mut boxed = type_decl("box", [fields.into(), vec![opens]].concat());
    boxed.key = vec!["code".into()];
    l.declare(boxed).await?;
    for d in [
        object_set_on("opened", "task", "state = 'open'"),
        trigger_on("reopened", "task", Some("state = 'open'")),
        crate::conformance::fixture::write_verb("reopen", "task", "state", "'open'"),
        object_set_on("finished", "task", "state = 'done'"),
    ] {
        l.declare_as(d, "declaring what names a value").await?;
    }
    let rule =
        json!({"on": "task", "field": "state", "from": "open", "to": "done", "when": "true"});
    l.upsert(lakelands_api::seam::AGING_RULE_TYPE, "close", rule)
        .await?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let report = preflight(&l, m.clone()).await?;
    let mut listed = report.conditions.clone();
    listed.sort();
    let want = [
        "the aging rule `close`",
        "the function declaration `opens`",
        "the object_set declaration `opened`",
        "the trigger declaration `reopened`",
        "the verb declaration `reopen`",
    ];
    ensure!(
        listed == want && !report.conditions_unshown,
        "the report lists the function, the set, the trigger, the verb and the aging rule that \
         name `open`, and not the set on `done`; it lists {:?}",
        report.conditions
    );
    // Neither may call a verb, and `unruled` may not read a rule's
    // `from`: each is told that one it is not shown may name a value, and
    // the proposal is listed to it with the report its preflight gave it.
    let access = |name, ty, field| {
        let mut p = field_access(store.as_ref(), name, ty, &[(field, Access::None)]);
        (p.grants.schema, p.grants.review) = (true, true);
        p
    };
    let blind = access("blind", "task", "note");
    let unruled = access("unruled", lakelands_api::seam::AGING_RULE_TYPE, "from");
    // `want` is sorted: the rule first, the verb last.
    let readers = [(&blind, &want[..4]), (&unruled, &want[1..4])];
    let mut told = Vec::new();
    for (who, _) in readers {
        let report = l.cat.migrate_value_type(who, m.clone()).await;
        told.push(report.must("a reader's preflight")?.data);
    }
    let id = propose(&l, &m).await?;
    for ((who, sees), told) in readers.into_iter().zip(told) {
        let mut shown = told.conditions.clone();
        shown.sort();
        let listed = l.cat.proposals(who, Default::default()).await;
        let listed = (listed.must("listing the proposals")?.data.into_iter())
            .filter(|p| p.id == id)
            .filter_map(|p| match p.write {
                Some(ProposedWrite::MigrateValueType { report, .. }) => Some(*report),
                _ => None,
            });
        ensure!(
            shown == sees && told.conditions_unshown && listed.eq([told.clone()]),
            "`{}` is shown {sees:?} and told that one it is not shown may name a value, and \
             the proposal is listed to it with that report, its id included; it is shown \
             {shown:?}, unshown {}",
            who.id,
            told.conditions_unshown
        );
    }
    // A reviewer without `schema` is told of `opens` only where `describe`
    // shows it: where it reads `note`, the field it reads, and is shown
    // `box`, which it computes a field of too.
    let (read, none) = (Access::Read, Access::None);
    for (note, code, named) in [(read, read, true), (none, read, false), (read, none, false)] {
        let mut who = field_access(store.as_ref(), "plain", "task", &[("note", note)]);
        let held = who.grants.types.entry("box".into()).or_default();
        held.fields.insert("code".into(), code);
        who.grants.review = true;
        let listed = l.cat.proposals(&who, Default::default()).await;
        let listed = (listed.must("listing as `plain`")?.data.into_iter()).find(|p| p.id == id);
        let report = listed.and_then(|p| match p.write {
            Some(ProposedWrite::MigrateValueType { report, .. }) => Some(report),
            _ => None,
        });
        let told = report.map(|r| r.conditions.iter().any(|c| c.contains("`opens`")));
        ensure!(
            told == Some(named),
            "a reviewer without `schema` that reads `note` as {note:?} and `box`.`code` as \
             {code:?} is told of the function `opens` only where it is shown it; it is told \
             {told:?}"
        );
    }
    // `hot` read `flag`, renamed `tag` since, which `kept` may not know of:
    // its grants followed to the proposal tell it only that one may be.
    let hot = |w| object_set_on("hot", "task", w);
    l.declare_as(hot("flag = 'x' and state = 'open'"), "declaring `hot`")
        .await?;
    let id = propose(&l, &m).await?;
    l.declare_as(hot("secret = 'x'"), "declaring `hot` again")
        .await?;
    let fields = ["note", "tag", "secret"].map(|f| field(f, StorageKind::Text));
    let to = type_decl("task", [vec![of_status("state")], fields.into()].concat());
    let renamed = Migration {
        renames: vec![("flag".into(), "tag".into())],
        ..migration(to)
    };
    let report = l.cat.migrate(&l.own, renamed.clone()).await;
    let report_id = report.must("renaming `flag`")?.data.report_id;
    let applied = l.cat.migrate_apply(&l.own, renamed, &report_id).await;
    applied.must("renaming `flag`")?;
    let mut kept = field_access(store.as_ref(), "kept", "task", &[("tag", Access::None)]);
    kept.grants.review = true;
    let listed = l.cat.proposals(&kept, Default::default()).await;
    let listed = (listed.must("listing as `kept`")?.data.into_iter()).find(|p| p.id == id);
    let report = listed.and_then(|p| match p.write {
        Some(ProposedWrite::MigrateValueType { report, .. }) => Some(report),
        _ => None,
    });
    ensure!(
        report.as_ref().is_some_and(
            |r| r.conditions_unshown && !r.conditions.iter().any(|c| c.contains("`hot`"))
        ),
        "`kept`, held from `flag` under its new name, is told that a set it is not shown may \
         name a value; it is listed {report:?}"
    );
    // An old value named only in a merged source's `where`, and only in a
    // relation's field's `derived`: each is an expression its declaration
    // holds.
    let (text, count) = (
        json!({"kind": "text"}),
        |n: &str| json!([{"name": n, "expr": "count()"}]),
    );
    l.declare_json(json!({"declare": "type", "name": "piles", "key": ["note"],
        "fields": [{"name": "note", "type": text}, {"name": "boxes", "type": {"kind": "number"}},
                   {"name": "tasks", "type": {"kind": "number"}}],
        "view": {"source": "box", "group_by": ["note"], "measures": count("boxes"),
                 "merge": [{"source": "task", "where": "state = 'open'", "group_by": ["note"],
                            "measures": count("tasks")}]}}))
        .await?;
    let is_open = json!({"name": "is_open", "type": {"kind": "boolean"}, "authority": "derived",
                         "derived": "label = 'open'", "depends_on": ["label"]});
    l.declare_json(
        json!({"declare": "type", "name": "shelf", "fields": [{"name": "n", "type": text}],
        "relations": [{"name": "holds", "target": "task", "cardinality": "many_to_many",
                       "fields": [{"name": "label", "type": text}, is_open]}]}),
    )
    .await?;
    let report = preflight(&l, m).await?;
    let named = |n: &str| report.conditions.iter().any(|c| c.contains(n));
    ensure!(
        named("`piles`") && named("`shelf`"),
        "the report lists the view that names `open` in a merged source's `where` and the type \
         that names it in a relation's field's `derived`; it lists {:?}",
        report.conditions
    );
    Ok(())
}

async fn a_rename_report_names_a_type_by_what_its_reader_is_described_of_it(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    l.declare_as(
        Declaration::ValueType(status(&["open", "done"])),
        "declaring `status`",
    )
    .await?;
    let texts = |names: &[&str]| names.iter().map(|f| field(f, StorageKind::Text)).collect();
    l.declare(type_decl("task", texts(&["n"]))).await?;
    let card = [vec![of_status("state")], texts(&["note", "kind"])].concat();
    l.declare(type_decl("card", card)).await?;
    // The three members that name `open`, each declared in its turn: a
    // relation's field's `derived`, a derived field, a view's `where`.
    let text = json!({"kind": "text"});
    let is_open = json!({"name": "is_open", "type": {"kind": "boolean"}, "authority": "derived",
                         "derived": "label = 'open'", "depends_on": ["label"]});
    let shelf = json!({"declare": "type", "name": "shelf", "fields": [{"name": "n", "type": text}],
        "relations": [{"name": "holds", "target": "task", "cardinality": "many_to_many",
                       "fields": [{"name": "label", "type": text}, is_open]}]});
    let flag = derived("flag", StorageKind::Boolean, "note = 'open'", &["note"]);
    let pile = json!({"declare": "type", "name": "pile", "key": ["note"],
        "fields": [{"name": "note", "type": text}, {"name": "cards", "type": {"kind": "number"}}],
        "view": {"source": "card", "group_by": ["note"], "where": "kind = 'open'",
                 "measures": [{"name": "cards", "expr": "count()"}]}});
    // Each reader is kept from one member: `far` is not shown the
    // relation's target, `loose` nothing of its link type; `masked` reads
    // `flag` masked; `blind` may not know of `kind`, which the view reads,
    // and holds no `schema`, which shows every definition.
    let reader = |name, ty: &str, fields: &[(&str, Access)], unseen: bool| {
        let mut p = field_access(s, name, ty, fields);
        (p.grants.schema, p.grants.review, p.grants.restricted) = (true, true, true);
        p.grants.types.entry(ty.into()).or_default().default = unseen.then_some(Access::None);
        p
    };
    let far = reader("far", "task", &[], true);
    let loose = reader("loose", "sys.link.shelf.holds", &[], true);
    let masked = reader("masked", "card", &[("flag", Access::Masked)], false);
    let mut blind = reader("blind", "card", &[("kind", Access::None)], false);
    blind.grants.schema = false;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    // The report of the proposal `id` as `who` is listed it.
    let listed = |who: &lakelands_api::Principal, id: lakelands_api::ProposalId| {
        let (cat, who) = (l.cat.clone(), who.clone());
        async move {
            let all = cat.proposals(&who, Default::default()).await;
            let mut all = all.must("listing the proposals")?.data;
            all.retain(|p| p.id == id);
            match all.pop().and_then(|p| p.write) {
                Some(ProposedWrite::MigrateValueType { report, .. }) => Ok(*report),
                other => Err(Failure::new(format!("a rename is listed: {other:?}"))),
            }
        }
    };
    // The report as `who` is told it: by its preflight or, without
    // `schema`, by the listing of the rename proposed now.
    let told = |who: &lakelands_api::Principal| {
        let (l, m, who) = (&l, &m, who.clone());
        async move {
            if !who.grants.schema {
                return listed(&who, propose(l, m).await?).await;
            }
            let report = l.cat.migrate_value_type(&who, m.clone()).await;
            Ok::<_, Failure>(report.must("a reader's preflight")?.data)
        }
    };
    let mut red = Vec::new();
    for (kept, name) in [
        (vec![&far, &loose], "shelf"),
        (vec![&masked], "card"),
        (vec![&blind], "pile"),
    ] {
        let mut before = Vec::new();
        for who in &kept {
            before.push(told(who).await?);
        }
        match name {
            "card" => {
                let added = l.cat.add_field(&l.own, &"card".into(), flag.clone()).await;
                added.must("adding `flag`")?
            }
            "shelf" => l.declare_json(shelf.clone()).await?,
            _ => l.declare_json(pile.clone()).await?,
        };
        let whole = preflight(&l, m.clone()).await?;
        let named = |r: &ValueTypeReport| r.conditions.iter().any(|c| c.contains(name));
        if !named(&whole) || whole.conditions_unshown {
            red.push(format!(
                "the owner is named `{name}`: {:?}",
                whole.conditions
            ));
        }
        // The same report but the flag, and the id made over it and the head.
        for (who, mut was) in kept.into_iter().zip(before) {
            let is = told(who).await?;
            (was.conditions_unshown, was.report_id) = (true, is.report_id.clone());
            if is != was {
                let (id, is) = (&who.id, &is.conditions);
                red.push(format!("`{id}`, kept from the member of `{name}`: {is:?}"));
            }
        }
    }
    let red = red.join("; ");
    ensure!(
        red.is_empty(),
        "a type is named to one whose description of it holds the old value, and another is \
         told only that a declaration it is not shown may name one: {red}"
    );
    // The listing of the proposed rename answers each the report its
    // preflight gave it before the proposal, the id too.
    let mut before = Vec::new();
    for who in [&far, &loose, &masked] {
        before.push((who, told(who).await?));
    }
    let id = propose(&l, &m).await?;
    for (who, told) in before {
        let listed = listed(who, id.clone()).await?;
        ensure!(
            listed == told && told.conditions_unshown,
            "the proposal is listed to `{}` with the report its preflight gave it, {told:?}; \
             it is listed {listed:?}",
            who.id
        );
    }
    // An apply takes the id of its caller's report, which names no more.
    let (owners, own) = (preflight(&l, m.clone()).await?, told(&far).await?);
    let said = (l.cat).migrate_value_type_apply(&far, m.clone(), &owners.report_id);
    refused_as!(
        "an apply by the owner's id",
        said.await,
        Case::MigrationStale
    )?;
    let applied = (l.cat).migrate_value_type_apply(&far, m.clone(), &own.report_id);
    let applied = applied.await;
    applied.must("applying by the id of the report `far` was given")?;
    Ok(())
}

async fn a_kept_view_over_a_renamed_value_counts_by_the_new_one(store: Store) -> Held {
    let l = lake(store.as_ref(), &["open", "done"], vec![]).await?;
    let mut by = crate::conformance::fixture::count_view("by_state", "task", "state");
    by.fields[0].ty.value_type = Some("status".into());
    let mut twin = crate::conformance::fixture::count_view("by_state_twin", "task", "state");
    twin.fields[0].ty.value_type = Some("status".into());
    l.declare(by).await?;
    l.declare(twin).await?;
    l.set("view_kept.by_state", json!(true)).await?;
    for (key, state) in [("t1", "open"), ("t2", "open"), ("t3", "done")] {
        l.upsert("task", key, json!({ "state": state })).await?;
    }
    let m = renamed(&["active", "done"], &[("open", "active")]);
    apply(store.as_ref(), &l, m).await?;
    let counts = |rows: Vec<lakelands_api::Record>| {
        let pair = |r: &lakelands_api::Record| {
            (state_of(r).map(str::to_owned), r.fields.get("n").cloned())
        };
        let mut out: Vec<_> = rows.iter().map(pair).collect();
        out.sort_by_key(|(s, _)| s.clone());
        out
    };
    let read = |view: &'static str| {
        let l = &l;
        async move { Ok::<_, Failure>(counts(l.all_found(view, Default::default()).await?)) }
    };
    let (kept, computed) = (read("by_state").await?, read("by_state_twin").await?);
    let want = vec![
        (Some("active".to_owned()), Some(json!(2))),
        (Some("done".to_owned()), Some(json!(1))),
    ];
    ensure!(
        kept == want && computed == want,
        "the kept view and its twin count two `active` and one `done`; they count {kept:?} and \
         {computed:?}"
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = read("by_state").await?;
    ensure!(
        rebuilt == want,
        "after a rebuild the kept view counts as before; it counts {rebuilt:?}"
    );
    Ok(())
}

async fn an_embedding_over_a_renamed_value_is_embedded_again_caused_by_the_rename(
    store: Store,
) -> Held {
    use crate::conformance::backing::{EMBED_MODEL, embedded};
    let (old, new) = ("renames-553-open", "renames-553-active");
    let l = Lake::of(store.as_ref());
    let decl = Declaration::ValueType(status(&[old, "renames-553-done"]));
    l.declare_as(decl, "declaring `status`").await?;
    let vec = crate::conformance::fixture::embedding("vec", 2, &["state"], EMBED_MODEL);
    l.declare(type_decl("task", vec![of_status("state"), vec]))
        .await?;
    l.upsert("task", "t1", json!({ "state": old })).await?;
    let before = embedded(new);
    let m = renamed(&[new, "renames-553-done"], &[(old, new)]);
    let (_, events) = apply(store.as_ref(), &l, m).await?;
    let at = events.first().map(|e| e.position);
    let recorded =
        crate::conformance::fixture::outputs_from(store.as_ref(), Position::GENESIS).await?;
    let caused: Vec<_> = (recorded.iter())
        .filter(|e| at.is_some_and(|at| e.position > at) && e.cause == at)
        .collect();
    let t1 = l.read("task", "t1").await?;
    ensure!(
        embedded(new) == before + 1 && caused.len() == 1 && t1.fields.contains_key("vec"),
        "the new text is embedded once and its answer recorded caused by the rename; it was \
         embedded {} times, {} answers name it as their cause, `t1` reads {t1:?}",
        embedded(new) - before,
        caused.len()
    );
    Ok(())
}

async fn a_backed_answer_that_holds_an_old_value_is_a_misfit_of_the_rename(store: Store) -> Held {
    use crate::conformance::backing::KIND;
    let l = Lake::of(store.as_ref());
    let decl = Declaration::ValueType(status(&["open", "done"]));
    l.declare_as(decl, "declaring `status`").await?;
    l.declare_json(json!({
        "declare": "function", "name": "echo_state",
        "inputs": [{"name": "state", "type": {"kind": "text"}}],
        "backing": KIND, "config": {"token": "renames_553", "echo": "state"}
    }))
    .await?;
    let mut echoed = crate::conformance::fixture::computed_by(
        "echoed",
        StorageKind::Text,
        "echo_state",
        &["state"],
    );
    echoed.ty.value_type = Some("status".into());
    l.declare(type_decl("task", vec![of_status("state"), echoed]))
        .await?;
    l.upsert("task", "t1", json!({"state": "open"})).await?;
    let t1 = l.read("task", "t1").await?;
    ensure!(
        t1.fields.get("echoed") == Some(&json!("open")),
        "the backing answers `open` for `t1`; it reads {t1:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let report = preflight(&l, m.clone()).await?;
    let named = report.first.iter().any(|f| f.field.as_str() == "echoed");
    let applied = l.cat.migrate_value_type_apply(&l.own, m, &report.report_id);
    let applied = applied.await;
    ensure!(
        !report.fits && report.misfits == 1 && named && applied.is_err(),
        "the backed answer `open` is a misfit, named, and the rename is refused; the report \
         says {report:?}, the apply {applied:?}"
    );
    head_unmoved(store.as_ref(), before, "a rename over a backed answer").await
}

check!(
    RENAMES_NO_RETRY = a_value_rename_calls_what_reads_the_new_value_and_no_recorded_failure_again,
    "022 and 027, extended (lakelands#857): a value type's rename calls once where a call reads a renamed value, and calls no recorded failure again: not of a type that uses the value type, whether its call reads that field or another and whether or not its record was renamed, for a function as for an embedding, not of another record of the type whose call moved, and not of a type that uses another value type or none; each kept failure still reads with its `error`, and the rename is the cause of the one output; `recompute` retries as before; and a rename folded with no backing bound leaves, in the lake opened again, the failures failures and the one moved call waiting, made when its record is next written",
    reopens
);

/// The types of [`RENAMES_NO_RETRY`]: the value type each uses, what its
/// call reads, and its record's value of that type. Each has a backing
/// token of its own, so its calls are counted alone; the last embeds.
const RETRIED: [(&str, &str, &str, &str); 7] = [
    ("read_kept", "status", "state", "done"),
    ("read_moved", "status", "state", "open"),
    ("beside_moved", "status", "code", "open"),
    ("beside_kept", "status", "code", "done"),
    ("other", "colour", "code", "red"),
    ("plain", "", "code", ""),
    ("embeds", "status", "title", "done"),
];

async fn a_value_rename_calls_what_reads_the_new_value_and_no_recorded_failure_again(
    store: Store,
    reopen: crate::conformance::Reopen,
) -> Held {
    use super::embeddings::{reopen_failed, with_embedder};
    use crate::conformance::backing::{
        EMBED_MODEL, KIND, embedded, embedder, made, plug, plug_text, unplug, unplug_text,
    };
    let bound = || with_embedder(Some(embedder()));
    let again = |e| reopen_failed("with the suite's backings", e);
    let mut store = reopen(store, bound()).await.map_err(again)?;
    let l = Lake::of(store.as_ref());
    let colour = |one_of: &[&str]| vt(json!({"name": "colour", "kind": "text", "one_of": one_of}));
    for decl in [status(&["open", "done"]), colour(&["red", "blue"])] {
        l.declare_as(Declaration::ValueType(decl), "declaring a value type")
            .await?;
    }
    let (text, token) = ("renames-857 fails once", |ty: &str| {
        format!("renames_857_{ty}")
    });
    for (ty, uses, reads, value) in RETRIED {
        let mut state = field("state", StorageKind::Text);
        state.ty.value_type = (!uses.is_empty()).then(|| uses.into());
        let mut fields = vec![state];
        fields.extend(["code", "title", "note"].map(|f| field(f, StorageKind::Text)));
        if ty == "embeds" {
            let vec = crate::conformance::fixture::embedding("out", 2, &[reads], EMBED_MODEL);
            fields.push(vec);
        } else {
            l.declare_json(json!({
                "declare": "function", "name": format!("f_{ty}"), "backing": KIND,
                "inputs": [{"name": reads, "type": {"kind": "text"}}],
                "config": {"token": token(ty), "echo": reads}
            }))
            .await?;
            let f = format!("f_{ty}");
            fields.push(computed_by("out", StorageKind::Text, &f, &[reads]));
        }
        l.declare(type_decl(ty, fields)).await?;
        let state = (!value.is_empty()).then_some(value);
        let record = json!({"state": state, "code": "XX", "title": text});
        // Every call fails once but that of `read_moved`'s `r1`, which
        // will move; its `r2` fails and will not: the sweep the rename
        // leaves owed of the type passes it.
        let (key, record) = match ty {
            "read_moved" => {
                l.upsert(ty, "r1", record).await?;
                ("r2", json!({"state": "done"}))
            }
            _ => ("r1", record),
        };
        unplug(&token(ty));
        unplug_text(text);
        l.upsert(ty, key, record).await?;
        plug(&token(ty));
    }
    plug_text(text);
    // The calls each type's backing received, and the records that read a
    // failure, no value and an `error` in the field's metadata, each with
    // the position its provenance names: each type's `r1`, then
    // `read_moved`'s `r2`.
    let calls =
        || RETRIED.map(|(ty, ..)| made(&token(ty)) + embedded(text) * usize::from(ty == "embeds"));
    let failed = async |store: &dyn LakeStore| {
        let mut out = Vec::new();
        let records = RETRIED.iter().map(|(ty, ..)| (*ty, "r1"));
        for (ty, key) in records.chain([("read_moved", "r2")]) {
            let r = Lake::of(store).read(ty, key).await?;
            let error = r.meta.get("out").filter(|m| m.error.is_some());
            out.push(
                error
                    .filter(|_| !r.fields.contains_key("out"))
                    .map(|m| m.position),
            );
        }
        Ok::<_, Failure>(out)
    };
    let more = |before: [usize; 7]| -> Vec<usize> {
        let now = calls();
        now.iter().zip(before).map(|(now, b)| now - b).collect()
    };
    let only = |i: usize| (0..7).map(|at| usize::from(at == i)).collect::<Vec<_>>();
    let first = failed(store.as_ref()).await?;
    ensure!(
        calls() == [1, 2, 1, 1, 1, 1, 1] && (0..8).all(|at| first[at].is_some() == (at != 1)),
        "every call was made once and all but that of `read_moved`'s `r1` failed; the calls \
         are {:?} and the records that read a failure {first:?}",
        calls()
    );

    let before = calls();
    let m = renamed(&["active", "done"], &[("open", "active")]);
    let (_, events) = apply(store.as_ref(), &l, m).await?;
    let (called, kept) = (more(before), failed(store.as_ref()).await?);
    let at = events.first().map(|e| e.position);
    let recorded = outputs_from(store.as_ref(), Position::GENESIS).await?;
    let caused = recorded.iter().filter(|e| e.cause == at).count();
    ensure!(
        called == only(1) && kept == first && caused == 1,
        "a rename of `status` calls once for `read_moved`, whose `r1` reads the renamed \
         value, and for no recorded failure, which still reads with its `error` and the \
         provenance of its own output; and it is the cause of that one output, since it \
         settles no other field. It called {called:?} (in the order of the table), the \
         failures read {kept:?}, not {first:?}, and {caused} outputs name it as their cause"
    );
    let before = calls();
    let m = ValueTypeMigration {
        to: colour(&["crimson", "blue"]),
        renames: vec![("red".to_owned(), "crimson".to_owned())],
    };
    apply(store.as_ref(), &l, m).await?;
    let (called, kept) = (more(before), failed(store.as_ref()).await?);
    ensure!(
        called == [0; 7] && kept == first,
        "a rename of `colour` calls nothing, since no call reads it; it called {called:?} \
         and the failures read {kept:?}"
    );
    let before = calls();
    let what = lakelands_api::write::Recompute::Type("plain".into());
    l.cat.recompute(&l.own, what).await.must("the door")?;
    let (called, kept) = (more(before), failed(store.as_ref()).await?);
    let mut healed = first;
    healed[5] = None;
    ensure!(
        called == only(5) && kept == healed,
        "`recompute` of `plain` retries its recorded failure, once, and no other; it called \
         {called:?} and the failures read {kept:?}"
    );

    // A rename folded with no backing bound: its calls are not made, and
    // the lake opened again hands none out.
    let unbound = |e| reopen_failed("with no backing", e);
    store = reopen(store, Seams::default()).await.map_err(unbound)?;
    let l = Lake::of(store.as_ref());
    let before = calls();
    let m = renamed(&["live", "done"], &[("active", "live")]);
    apply(store.as_ref(), &l, m).await?;
    store = reopen(store, bound()).await.map_err(again)?;
    let (called, kept) = (more(before), failed(store.as_ref()).await?);
    ensure!(
        called == [0; 7] && kept == healed,
        "a rename folded with no backing bound, and the lake opened again, call nothing, and \
         the failures are failures still, not fields that wait; they called {called:?} and \
         the failures read {kept:?}"
    );
    let l = Lake::of(store.as_ref());
    let records = RETRIED.iter().map(|(ty, ..)| (*ty, "r1"));
    for (ty, key) in records.chain([("read_moved", "r2")]) {
        l.upsert(ty, key, json!({"note": "written again"})).await?;
    }
    let (called, kept) = (more(before), failed(store.as_ref()).await?);
    ensure!(
        called == only(1) && kept == healed,
        "the next write of each record makes the one call the rename moved, and calls no \
         recorded failure again; they called {called:?} and the failures read {kept:?}"
    );
    Ok(())
}
