//! A value type is loosened by declaration and tightened by a migration
//! whose preflight counts what would no longer fit
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
//!
//! `declare` admits a change no stored value can stop fitting. A
//! tightening goes through `migrate_value_type`: every value stored under
//! the value type, over every field of every type that uses it, retired
//! and restricted records included, is checked against the new
//! declaration, and the change is put in force only when all of them fit,
//! as one `declared` that rewrites no record. A change of `kind` or
//! `scale` is neither. A migration that renames values is in
//! `value_renames.rs`.

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{Authority, Cardinality, RelationDecl, StorageKind, ValueTypeDecl};
use lakelands_api::write::{Declaration, ValueTypeReport};
use lakelands_api::{
    Access, Error, Grants, LakeStore, Principal, PrincipalKind, Refusal, WriteOpts,
};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, describe_type, events_after, field, field_access, head_unmoved, invalid_case,
    invalid_like, log_events, object_set_on, principal, reason, refused_on, reviewer, trigger_on,
    try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    VERSIONS_KEY_A_TYPE = a_value_types_version_is_answered_with_the_types_that_use_it,
    "lakelands#1058: `describe` answers each value type's version, and a type's lifecycle the version of each value type a field its reader is shown uses; a loosening and a migration of the value type move it, while the type's own `version` and `schema_hash` stay; a field its reader may not know of names no value type"
);

check!(
    TYPE_TELLS_ITS_VALUE_TYPES = a_type_is_described_with_the_value_types_it_uses,
    "lakelands#1060: `describe_type` answers the declaration of each value type its lifecycle's `value_types` names, once, in name order: those of the fields its reader is shown, a masked field's among them, a hidden field's not; a loosening shows the new declaration; `describe` names them once, in the lake's `value_types`, and none on a type"
);

check!(
    LOOSEN_BY_DECLARE = a_value_type_that_accepts_more_is_declared_and_no_record_changes,
    "027: a value type that accepts everything it did (a value added to `one_of`, a `constraint` removed) is declared at its next version, and no stored record changes"
);

check!(
    DECLARE_REFUSES_TIGHTENING = declare_refuses_a_value_type_that_accepts_less_or_reshapes,
    "027: `declare` refuses a value type that would accept less, naming `migrate`, and one whose `kind` or `scale` changes; `migrate_value_type` refuses that change too, since no value-type migration makes it; nothing is appended"
);

check!(
    PREFLIGHT_COUNTS = the_preflight_counts_what_would_not_fit_over_every_use_and_history,
    "027: the preflight of a value type checks every value stored under it, over every field of every type that uses it, retired and restricted records included, counts the records and the misfits per use, names the first of them of records its reader is answered, in a preview and in the refused apply, so that one held from the reader is counted and not named, and one in a field its reader does not read takes none of the ten places (lakelands#875), and appends nothing"
);

check!(
    MISFITS_REFUSE = applying_a_tightening_some_value_does_not_fit_is_refused,
    "027: a value-type migration some stored value would not fit is refused as invalid, naming how many, and appends nothing"
);

check!(
    FITS_DECLARES = a_tightening_every_value_fits_is_one_declared_version,
    "027 and log-format: a value-type migration every stored value fits appends one `declared` at the next version and changes no record; later writes are checked against it"
);

check!(
    STALE_REPORT = a_value_type_report_from_before_the_lake_moved_is_refused,
    "027 and 006: a value-type migration is applied only against the report of the lake as it stands; after the lake moved it is refused as invalid; and only by the id of the report its caller was shown: two readers kept from different uses of one tightening, each applying by the other's id, are refused as `migration_stale`, appending nothing, where its own applies (lakelands#935)"
);

check!(
    SCHEMA_GRANT = a_value_type_migration_needs_the_schema_grant,
    "027 and 006: both the preflight and the apply of a value-type migration refuse a principal without the `schema` grant; migrate_apply, migrate_value_type_apply, declare and apply ask for it before they judge what they are given (028 and 027, extended)"
);

check!(
    BUNDLE_REFUSED_WHOLE =
        an_apply_that_loosens_one_value_type_and_tightens_another_is_refused_whole,
    "028 and 027: `apply` of a set that loosens one value type and tightens another is refused whole, naming `migrate`, and appends nothing"
);

async fn a_type_is_described_with_the_value_types_it_uses(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let state = |one_of: Value| vt(json!({"name": "state", "kind": "text", "one_of": one_of}));
    let eur = vt(json!({"name": "eur", "kind": "decimal", "scale": 2, "constraint": "value >= 0"}));
    for d in [state(json!(["open", "closed"])), eur.clone()] {
        l.declare_as(Declaration::ValueType(d), "declaring a value type")
            .await?;
    }
    l.declare_json(json!({"declare": "type", "name": "task", "fields": [
        {"name": "state", "type": {"kind": "text", "value_type": "state"}},
        {"name": "amount", "type": {"kind": "decimal", "value_type": "eur"}},
        {"name": "title", "type": {"kind": "text"}}]}))
        .await?;
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let told = |who: &Principal| {
        let (cat, who) = (l.cat.clone(), who.clone());
        async move {
            let t = describe_type(cat.as_ref(), &who, "task").await?;
            let keyed: Vec<_> = t.lifecycle.value_types.into_keys().collect();
            let named: Vec<_> = t.value_types.iter().map(|v| v.name.clone()).collect();
            ensure!(
                keyed == named,
                "`task`'s key names {keyed:?} and it tells {named:?}"
            );
            Ok::<_, Failure>(t.value_types)
        }
    };
    let both = told(&l.own).await?;
    let note = l.describe_type("note").await?.value_types;
    let lake = l.describe().await?;
    let on_types = lake.types.iter().find(|t| !t.value_types.is_empty());
    ensure!(
        both == [eur.clone(), state(json!(["open", "closed"]))]
            && note.is_empty()
            && on_types.is_none()
            && lake.value_types.len() == 2,
        "`task` tells `eur` and `state` as declared, `note` none, and `describe` names them \
         once: {both:?}, {note:?}, a type naming some: {on_types:?}"
    );
    // A masked field's value type is told, a hidden field's is not.
    let masked = field_access(
        store.as_ref(),
        "masked",
        "task",
        &[("state", Access::Masked)],
    );
    let hid = field_access(store.as_ref(), "hid", "task", &[("state", Access::None)]);
    let (masked, hid) = (told(&masked).await?, told(&hid).await?);
    ensure!(
        masked == both && hid == [eur.clone()],
        "a reader of `task.state` masked is told {masked:?}, and one kept from it {hid:?}"
    );
    // A loosening is told as it stands.
    let loosened = state(json!(["open", "closed", "overdue"]));
    l.declare_as(
        Declaration::ValueType(loosened.clone()),
        "loosening `state`",
    )
    .await?;
    let after = told(&l.own).await?;
    ensure!(
        after == [eur, loosened],
        "after `state` is loosened, `task` tells {after:?}"
    );
    Ok(())
}

fn vt(value: Value) -> ValueTypeDecl {
    serde_json::from_value(value).expect("a value type")
}

fn status(one_of: &[&str]) -> ValueTypeDecl {
    vt(json!({"name": "status_code", "kind": "text", "one_of": one_of}))
}

fn score(constraint: Option<&str>) -> ValueTypeDecl {
    let mut v = json!({"name": "score", "kind": "number"});
    if let Some(c) = constraint {
        v["constraint"] = json!(c);
    }
    vt(v)
}

/// `status_code` (open, paid, late, void) and `score` (no constraint);
/// `invoice` uses both, `ticket` uses `status_code`.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    for d in [status(&["open", "paid", "late", "void"]), score(None)] {
        l.declare_as(Declaration::ValueType(d), "declaring a value type")
            .await?;
    }
    let typed = |name: &str, kind, vt: &str| {
        let mut f = field(name, kind);
        f.ty.value_type = Some(vt.into());
        f
    };
    l.declare(type_decl(
        "invoice",
        vec![
            typed("status", StorageKind::Text, "status_code"),
            typed("score", StorageKind::Number, "score"),
        ],
    ))
    .await?;
    // A ticket is raised by an invoice, carried by link records whose
    // `status` uses the value type too.
    let mut ticket = type_decl(
        "ticket",
        vec![typed("state", StorageKind::Text, "status_code")],
    );
    ticket.relations = vec![RelationDecl {
        name: "raised_by".into(),
        target: "invoice".into(),
        cardinality: Cardinality::ManyToOne,
        fields: vec![typed("status", StorageKind::Text, "status_code")],
        authority: Authority::Authored,
        ..Default::default()
    }];
    l.declare(ticket).await?;
    Ok(l)
}

/// The versions `declared` gave `name`, in log order.
async fn versions(store: &dyn LakeStore, name: &str) -> Result<Vec<u32>, Failure> {
    Ok(log_events(store)
        .await?
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::Declared {
                declaration: Declaration::ValueType(v),
                version,
                ..
            } if v.name.as_str() == name => Some(version),
            _ => None,
        })
        .collect())
}

async fn preflight(l: &Lake, to: ValueTypeDecl) -> Result<ValueTypeReport, Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    Ok(cat
        .migrate_value_type(who, to.into())
        .await
        .must("running a value type's preflight")?
        .data)
}

/// Whether `e` is a refusal as invalid of the case `code`: a value type
/// that accepts less is `value_type_tightens`, whose sentence says to
/// tighten it through `migrate`; one that changes its kind or scale
/// `value_type_reshapes`.
fn is_refused_as(e: &Error, code: &str) -> bool {
    invalid_like(e, &json!({"code": code}))
}

async fn a_value_type_that_accepts_more_is_declared_and_no_record_changes(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    l.upsert("invoice", "i1", json!({"status": "late", "score": 3}))
        .await?;
    let before = l.read("invoice", "i1").await?;
    for loosened in [
        status(&["open", "paid", "late", "void", "held"]),
        status(&[]),
    ] {
        let fut = cat.declare(own, Declaration::ValueType(loosened));
        fut.await.must("declaring a loosened `status_code`")?;
    }
    let fut = cat.declare(own, Declaration::ValueType(score(None)));
    fut.await.must("declaring `score` again as it is")?;
    let v = versions(store.as_ref(), "status_code").await?;
    ensure!(
        v == [1, 2, 3],
        "`status_code` was declared, then loosened twice; its versions are {v:?}"
    );
    let after = l.read("invoice", "i1").await?;
    ensure!(
        after.version == before.version && after.fields == before.fields,
        "a loosening changed `i1`: {:?} became {:?}",
        before,
        after
    );
    Ok(())
}

async fn declare_refuses_a_value_type_that_accepts_less_or_reshapes(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    for d in [
        json!({"name": "money", "kind": "decimal", "scale": 2}),
        json!({"name": "contact", "kind": "text"}),
        json!({"name": "site", "kind": "text", "format": "domain"}),
    ] {
        l.declare_as(Declaration::ValueType(vt(d)), "declaring a value type")
            .await?;
    }
    let before = store.head().await.must("reading the head")?;
    let (less, reshapes) = ("value_type_tightens", "value_type_reshapes");
    for (what, d, code) in [
        ("narrows `one_of`", status(&["open", "paid"]), less),
        ("adds a constraint", score(Some("value >= 0")), less),
        (
            "adds a format",
            vt(json!({"name": "contact", "kind": "text", "format": "email"})),
            less,
        ),
        (
            "changes its format",
            vt(json!({"name": "site", "kind": "text", "format": "email"})),
            less,
        ),
        (
            "changes its kind",
            vt(json!({"name": "score", "kind": "text"})),
            reshapes,
        ),
        (
            "changes its scale",
            vt(json!({"name": "money", "kind": "decimal", "scale": 3})),
            reshapes,
        ),
    ] {
        let e = cat
            .declare(own, Declaration::ValueType(d))
            .await
            .must_fail(&format!("declaring a value type that {what}"))?;
        ensure!(
            is_refused_as(&e, code),
            "a value type that {what} was refused as {e:?}; invalid, as `{code}`, was \
             expected"
        );
    }
    // Nor does a value-type migration change `kind` or `scale`.
    for (what, d) in [
        (
            "changes its kind",
            vt(json!({"name": "score", "kind": "text"})),
        ),
        (
            "changes its scale",
            vt(json!({"name": "money", "kind": "decimal", "scale": 3})),
        ),
    ] {
        let e = cat
            .migrate_value_type(own, d.into())
            .await
            .must_fail(&format!("migrating a value type that {what}"))?;
        ensure!(
            is_refused_as(&e, reshapes),
            "a value-type migration that {what} was refused as {e:?}; invalid, as \
             `{reshapes}`, was expected"
        );
    }
    head_unmoved(
        store.as_ref(),
        before,
        "declaring value types that accept less",
    )
    .await
}

/// `i1` late, `i2` open with a negative score, `i3` late and retired, `i4`
/// late and restricted; `t1` late, raised by `i1` with a link whose status
/// is late.
async fn stored(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    for (k, s, n) in [
        ("i1", "late", 1),
        ("i2", "open", -1),
        ("i3", "late", 2),
        ("i4", "late", 3),
    ] {
        l.upsert("invoice", k, json!({"status": s, "score": n}))
            .await?;
    }
    l.retire("invoice", "i3").await?;
    store
        .restrict(&"invoice".into(), &"i4".into(), "a retention hold".into())
        .await
        .must("restricting `i4`")?;
    l.upsert("ticket", "t1", json!({"state": "late"})).await?;
    cat.link(
        own,
        &"ticket".into(),
        &"t1".into(),
        &"raised_by".into(),
        &"i1".into(),
        crate::conformance::fixture::patch(json!({"status": "late"})),
        WriteOpts::default(),
    )
    .await
    .must("linking `t1` to `i1`")?;
    Ok(l)
}

async fn the_preflight_counts_what_would_not_fit_over_every_use_and_history(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = stored(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    let r = preflight(&l, status(&["open", "paid", "void"])).await?;
    let uses: Vec<(String, String, u64, u64)> = r
        .fields
        .iter()
        .map(|u| {
            (
                u.ty.to_string(),
                u.field.to_string(),
                u.values_scanned,
                u.misfits,
            )
        })
        .collect();
    ensure!(
        r.records_scanned == 6
            && r.misfits == 5
            && !r.fits
            && uses
                == [
                    ("invoice".into(), "status".into(), 4, 3),
                    ("sys.link.ticket.raised_by".into(), "status".into(), 1, 1),
                    ("ticket".into(), "state".into(), 1, 1)
                ],
        "without `late`, `i1`, the retired `i3`, the restricted `i4`, `t1` and the link from \
         `t1` would not fit, over six records; the report says {r:?}"
    );
    let named: Vec<&str> = r.first.iter().map(|m| m.key.as_str()).collect();
    ensure!(
        named.contains(&"i3") && named.contains(&"i4") && named.contains(&"t1:i1"),
        "the report names {named:?} among the first misfits"
    );
    // One without `restricted` is answered no read of `i4`: it is counted,
    // and the misfits named are of the records that reader is answered.
    let (architect, to) = (
        reviewer(store.as_ref(), true),
        status(&["open", "paid", "void"]),
    );
    let seen = l
        .cat
        .migrate_value_type(&architect, to.clone().into())
        .await;
    let seen = seen.must("the preview by one without `restricted`")?.data;
    let applied = (l.cat).migrate_value_type_apply(&architect, to.into(), &seen.report_id);
    let refused = format!("{:?}", applied.await);
    let named: Vec<&str> = seen.first.iter().map(|m| m.key.as_str()).collect();
    ensure!(
        seen.misfits == 5 && named == ["i1", "i3", "t1:i1", "t1"] && !refused.contains("i4"),
        "a misfit is named only of a record its reader is answered, the count whole: one \
         without `restricted` is named {named:?} of {}, and its apply is refused as {refused}",
        seen.misfits
    );
    // A constraint the stored values are checked against, as a write is.
    let s = preflight(&l, score(Some("value >= 0"))).await?;
    ensure!(
        s.misfits == 1 && s.first.first().is_some_and(|m| m.key.as_str() == "i2"),
        "`i2`'s score of -1 fails `value >= 0`, and the report says {s:?}"
    );
    head_unmoved(store.as_ref(), before, "a value type's preflight").await?;
    // A misfit in a field its reader does not read takes no place: with
    // nine tickets more, one kept from `invoice.status` is named ten.
    for n in 2..=10 {
        let late = json!({"state": "late"});
        l.upsert("ticket", &format!("t{n}"), late).await?;
    }
    let mut kept = field_access(
        store.as_ref(),
        "kept",
        "invoice",
        &[("status", Access::None)],
    );
    kept.grants.schema = true;
    let to = status(&["open", "paid", "void"]).into();
    let seen = l.cat.migrate_value_type(&kept, to).await;
    let seen = seen
        .must("the preview by one kept from `invoice.status`")?
        .data;
    ensure!(
        seen.misfits == 14
            && seen.first.len() == 10
            && seen.first.iter().all(|m| m.ty != "invoice"),
        "of fourteen misfits, three in a field kept from their reader, it is named ten, none \
         of them `invoice`'s; it is named {:?} of {}",
        seen.first,
        seen.misfits
    );
    Ok(())
}

async fn applying_a_tightening_some_value_does_not_fit_is_refused(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = stored(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let to = status(&["open", "paid", "void"]);
    let r = preflight(&l, to.clone()).await?;
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate_value_type_apply(own, to.into(), &r.report_id)
        .await
        .must_fail("applying a tightening five stored values would not fit")?;
    ensure!(
        invalid_like(
            &e,
            &json!({"code": "value_type_not_tightened", "misfits": 5})
        ),
        "the apply was refused as {e:?}; invalid, naming the five misfits, was expected"
    );
    head_unmoved(store.as_ref(), before, "a tightening with misfits").await
}

async fn a_tightening_every_value_fits_is_one_declared_version(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = stored(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let i1 = l.read("invoice", "i1").await?;
    // No record holds `void`.
    let to = status(&["open", "paid", "late"]);
    let r = preflight(&l, to.clone()).await?;
    ensure!(
        r.fits && r.misfits == 0,
        "every stored value fits, and the report says {r:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let applied = cat
        .migrate_value_type_apply(own, to.into(), &r.report_id)
        .await
        .must("applying a tightening every value fits")?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        head == before.next() && applied.position() == head,
        "the tightening appended through {head} from {before}; one event was expected"
    );
    let v = versions(store.as_ref(), "status_code").await?;
    ensure!(
        v == [1, 2],
        "`status_code`'s versions are {v:?}; 1 then 2 was expected"
    );
    let after = l.read("invoice", "i1").await?;
    ensure!(
        after.version == i1.version,
        "the tightening changed `i1`'s version from {} to {}",
        i1.version.0,
        after.version.0
    );
    let e = try_upsert(
        cat.as_ref(),
        own,
        "invoice",
        "i5",
        json!({"status": "void"}),
    );
    let e = e.await.must_fail("writing `void` after it was taken out")?;
    ensure!(
        matches!(refused_on(&e), Some(("status", Refusal::Constraint(..)))),
        "`void` was written after the tightening, or refused as {e:?}"
    );
    Ok(())
}

async fn a_value_type_report_from_before_the_lake_moved_is_refused(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let to = status(&["open", "paid", "late"]);
    let r = preflight(&l, to.clone()).await?;
    l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate_value_type_apply(own, to.into(), &r.report_id)
        .await
        .must_fail("applying a report from before the lake moved")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Invalid(..))),
        "a stale report was refused as {e:?}; invalid was expected"
    );
    head_unmoved(store.as_ref(), before, "a stale value-type report").await?;
    // An id names the report its reader was shown: two readers kept from
    // different uses are given two, and each other's is stale.
    let kept = |id, ty, field| {
        let mut who = field_access(store.as_ref(), id, ty, &[(field, Access::None)]);
        who.grants.schema = true;
        who
    };
    let (ka, kb) = (
        kept("ka", "invoice", "status"),
        kept("kb", "ticket", "state"),
    );
    let to = status(&["open", "paid", "late"]);
    let mut ids = Vec::new();
    for who in [&ka, &kb] {
        let r = cat.migrate_value_type(who, to.clone().into()).await;
        ids.push(r.must("a kept reader's preflight")?.data.report_id);
    }
    let before = store.head().await.must("reading the head")?;
    for (who, other) in [(&ka, &ids[1]), (&kb, &ids[0])] {
        let said = cat.migrate_value_type_apply(who, to.clone().into(), other);
        let said = said.await;
        ensure!(
            matches!(invalid_case(&said), Some(Case::MigrationStale)),
            "`{}` applying by the other reader's id must be `migration_stale`; it answered {said:?}",
            who.id
        );
    }
    head_unmoved(store.as_ref(), before, "an apply by another's id").await?;
    let own = cat.migrate_value_type_apply(&ka, to.into(), &ids[0]).await;
    own.must("applying by the id of the report `ka` was shown")?;
    Ok(())
}

async fn a_value_type_migration_needs_the_schema_grant(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, .. } = &l;
    let to = status(&["open", "paid", "late"]);
    let r = preflight(&l, to.clone()).await?;
    let clerk = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "clerk",
        Grants {
            schema: false,
            ..Grants::owner()
        },
    );
    let pre = cat.migrate_value_type(&clerk, to.clone().into()).await;
    let apply = cat.migrate_value_type_apply(&clerk, to.into(), &r.report_id);
    let apply = apply.await;
    let refused = |r: &Result<_, Error>| matches!(reason(&r), Some(Refusal::NotGranted));
    ensure!(
        refused(&pre.map(|_| ())) && refused(&apply.map(|_| ())),
        "a principal without `schema` was not refused as not granted on both calls"
    );
    // Every schema door asks for the grant before it judges what it is
    // given (028 and 027, extended): an undeclared value type or type, a
    // name no declaration may carry, an empty set.
    let none = ValueTypeDecl {
        name: "nothing".into(),
        ..status(&["a"])
    };
    let bad = Declaration::ValueType(ValueTypeDecl {
        name: "not a name".into(),
        ..status(&["a"])
    });
    let missing = lakelands_api::write::Migration::to(type_decl("nothing", vec![]));
    let answers = [
        cat.migrate_value_type_apply(&clerk, none.into(), "r")
            .await
            .map(drop),
        cat.migrate_apply(&clerk, missing, "r").await.map(drop),
        cat.declare(&clerk, bad).await.map(drop),
        cat.apply(&clerk, vec![]).await.map(drop),
    ];
    ensure!(
        answers.iter().all(refused),
        "the schema grant comes before what a schema door is given; they answered {answers:?}"
    );
    Ok(())
}

async fn an_apply_that_loosens_one_value_type_and_tightens_another_is_refused_whole(
    store: Store,
) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let e = cat.apply(
        own,
        vec![
            Declaration::ValueType(status(&["open", "paid", "late", "void", "held"])),
            Declaration::ValueType(score(Some("value >= 0"))),
        ],
    );
    let e = e.await.must_fail("applying a set that tightens `score`")?;
    ensure!(
        is_refused_as(&e, "value_type_tightens"),
        "the set was refused as {e:?}; invalid, naming `migrate`, was expected"
    );
    head_unmoved(store.as_ref(), before, "a set that tightens a value type").await?;
    let v = versions(store.as_ref(), "status_code").await?;
    ensure!(
        v == [1],
        "the set was refused whole, yet `status_code` was loosened: its versions are {v:?}"
    );
    Ok(())
}

check!(
    CONSTRAINT_AND_FORMAT =
        removing_a_constraint_or_a_format_is_declared_and_changing_a_constraint_is_refused,
    "027 extended and seams: `declare` takes a value type whose `constraint` or `format` is removed, since no stored value can stop fitting it; a `constraint` changed may reject a stored value, so `declare` refuses it, naming `migrate`"
);

async fn removing_a_constraint_or_a_format_is_declared_and_changing_a_constraint_is_refused(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for d in [
        json!({"name": "rating", "kind": "number", "constraint": "value >= 0"}),
        json!({"name": "grade", "kind": "number", "constraint": "value >= 0"}),
        json!({"name": "site", "kind": "text", "format": "domain"}),
    ] {
        l.declare_as(Declaration::ValueType(vt(d)), "declaring a value type")
            .await?;
    }
    for (what, d) in [
        (
            "removes its constraint",
            vt(json!({"name": "rating", "kind": "number"})),
        ),
        (
            "removes its format",
            vt(json!({"name": "site", "kind": "text"})),
        ),
    ] {
        cat.declare(own, Declaration::ValueType(d))
            .await
            .must(&format!("declaring a value type that {what}"))?;
    }
    for name in ["rating", "site"] {
        let v = versions(store.as_ref(), name).await?;
        ensure!(
            v == [1, 2],
            "`{name}` was declared and then loosened by `declare`; its versions are {v:?}"
        );
    }
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .declare(
            own,
            Declaration::ValueType(vt(
                json!({"name": "grade", "kind": "number", "constraint": "value >= 1"}),
            )),
        )
        .await
        .must_fail("declaring a value type that changes its constraint")?;
    ensure!(
        is_refused_as(&e, "value_type_tightens"),
        "a value type whose constraint changes was refused as {e:?}; invalid, naming \
         `migrate`, was expected: a changed constraint may reject a stored value"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "declaring a value type that changes its constraint",
    )
    .await?;
    Ok(())
}

check!(
    CONDITION_NEVER_HOLDS = a_trigger_or_set_on_a_value_the_type_no_longer_admits_never_holds,
    "027 extended and seams: the preflight checks stored values, not the conditions that mention them; a trigger's `when` or an object set's `where` comparing with a value a tightening removed stays declared and simply never holds afterwards"
);

async fn a_trigger_or_set_on_a_value_the_type_no_longer_admits_never_holds(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    for d in [
        trigger_on("went_late", "invoice", Some("status = 'late'")),
        object_set_on("late_ones", "invoice", "status = 'late'"),
    ] {
        l.declare_as(d, "declaring a trigger and a set on `late`")
            .await?;
    }
    l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    // `late` leaves `status_code`: no stored value holds it, so it applies.
    let tighter = status(&["open", "paid", "void"]);
    let r = preflight(&l, tighter.clone()).await?;
    cat.migrate_value_type_apply(own, tighter.into(), &r.report_id)
        .await
        .must("tightening `status_code` past the trigger and the set")?;
    // Both conditions stay declared: the preflight never looked at them.
    let described = cat
        .describe(own, lakelands_api::ReadAt::default())
        .await
        .must("describing the lake after the tightening")?
        .data;
    ensure!(
        described
            .triggers
            .iter()
            .any(|t| t.name.as_str() == "went_late"),
        "the tightening left the trigger on `late` declared; describe lists {:?}",
        described.triggers
    );
    let sets: Vec<String> = log_events(store.as_ref())
        .await?
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::Declared {
                declaration: Declaration::ObjectSet(o),
                ..
            } => Some(o.name.to_string()),
            _ => None,
        })
        .collect();
    ensure!(
        sets == ["late_ones"],
        "the set of `late` records was declared once and is still the lake's; the log's \
         set declarations are {sets:?}"
    );
    let from = store.head().await.must("reading the head")?;
    for s in ["paid", "void", "open"] {
        l.upsert("invoice", "i1", json!({"status": s})).await?;
    }
    // A record first written after the tightening: a set that held would
    // take it in.
    l.upsert("invoice", "i2", json!({"status": "open"})).await?;
    let late = try_upsert(
        cat.as_ref(),
        own,
        "invoice",
        "i1",
        json!({"status": "late"}),
    )
    .await;
    ensure!(
        late.is_err(),
        "`late` is no longer a `status_code`, and writing it answered {late:?}"
    );
    let reacted: Vec<String> = events_after(store.as_ref(), from)
        .await?
        .into_iter()
        .filter(|e| matches!(e.body.kind(), "trigger_fired" | "object_set_changed"))
        .map(|e| e.body.kind().to_owned())
        .collect();
    ensure!(
        reacted.is_empty(),
        "the trigger on `late` and the set of `late` records stay declared and never hold: \
         after the tightening, writes of paid, void and open, and a new record, made {reacted:?}"
    );
    Ok(())
}

async fn a_value_types_version_is_answered_with_the_types_that_use_it(store: Store) -> Held {
    use lakelands_api::write::ValueTypeMigration;
    let l = Lake::of(store.as_ref());
    let status = |one_of: Value| -> ValueTypeDecl {
        let v = json!({"name": "status", "kind": "text", "one_of": one_of});
        serde_json::from_value(v).expect("the check's value type reads")
    };
    l.declare_as(
        Declaration::ValueType(status(json!(["open", "closed"]))),
        "declaring `status`",
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "task", "fields": [
        {"name": "status", "type": {"kind": "text", "value_type": "status"}},
        {"name": "title", "type": {"kind": "text"}}]}))
        .await?;
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    l.upsert("task", "t1", json!({"status": "closed"})).await?;
    let first = l.describe_type("task").await?.lifecycle;
    let versions = |at: u32| {
        [("status".into(), at)]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let held = |at: u32, what: &str| {
        let (l, first) = (&l, &first);
        let what = what.to_owned();
        async move {
            let lake = l.describe().await?.value_type_versions;
            let task = l.describe_type("task").await?.lifecycle;
            let note = l.describe_type("note").await?.lifecycle.value_types;
            ensure!(
                lake == versions(at)
                    && task.value_types == versions(at)
                    && note.is_empty()
                    && (task.version, &task.schema_hash) == (first.version, &first.schema_hash),
                "{what}: `status` is at version {at} in `describe` ({lake:?}) and in `task`'s \
                 lifecycle ({:?}), `note` names none ({note:?}), and `task` stays at version {} \
                 and hash {}: it is at {} and {}",
                task.value_types,
                first.version,
                first.schema_hash,
                task.version,
                task.schema_hash
            );
            Ok::<_, Failure>(())
        }
    };
    held(1, "declared").await?;
    let loosened = status(json!(["open", "closed", "overdue"]));
    l.declare_as(
        Declaration::ValueType(loosened.clone()),
        "loosening `status`",
    )
    .await?;
    held(2, "loosened by `declare`").await?;
    let m = ValueTypeMigration {
        to: status(json!(["open", "done", "overdue"])),
        renames: vec![("closed".into(), "done".into())],
    };
    let report = l.cat.migrate_value_type(&l.own, m.clone()).await;
    let report = report.must("the preflight of `closed` to `done`")?.data;
    let applied = l
        .cat
        .migrate_value_type_apply(&l.own, m, &report.report_id)
        .await;
    applied.must("migrating `closed` to `done`")?;
    held(3, "migrated").await?;
    // A reader kept from `task.status` is told no value type of it.
    let hid = field_access(store.as_ref(), "hid", "task", &[("status", Access::None)]);
    let shown = describe_type(l.cat.as_ref(), &hid, "task")
        .await?
        .lifecycle
        .value_types;
    ensure!(
        shown.is_empty(),
        "a reader kept from `task.status` is told its value type: {shown:?}"
    );
    Ok(())
}
