//! A host's own setting, declared (063, extended, lakelands#996): every
//! write of its key is judged as a write of a field of its type, the
//! store stamps its group, which a grant's `only` names, and `describe`
//! tells its declaration to a holder of `schema` and to one who may write
//! it.

use super::only::holder;
use crate::conformance::fixture;
use crate::conformance::prelude::*;

check!(
    HOST_SETTING_KIND = a_declared_setting_holds_its_kind,
    "063, extended (lakelands#996): a setting a host declares with a type is judged at every write as a field of that type: `\"19%\"` for a `decimal` is refused, naming `value` within the setting, appending nothing, and a decimal is taken; `null` is refused as the absence of the required `value`, to the owner too; `setting('<key>')` reads it as a decimal, in a derived field's arithmetic and comparison and in a `find`'s filter alike; a declaration of a key the store reads, or under its prefix, or into the group `store`, of a reference, and one of a key whose held value its type refuses, are refused, naming the setting; a value is held in the form its kind keeps (a `date_time` in UTC)"
);

check!(
    HOST_SETTING_GROUP = a_declared_setting_is_written_by_its_group,
    "063, extended (lakelands#996): the store stamps a setting's `group` from its declaration, `store` on its own settings (`sort_index.<type>`), and a write naming another is refused, naming `group`; a reader of `sys.setting` reads the group as a field and finds by it; a holder of `only {group: operator}` on `sys.setting` writes a setting of that group and is answered on one of another group as in a twin lake where nobody declared it: a value, a value the other setting's kind refuses, and a group guessed right or wrong; a declaration that moves a setting to another group stamps its record again in its own append, after which the holder is refused it as in that twin and is told its declaration no more; a proposal to create a key accepted after its declaration moved it is stamped with the group then, and is listed with no group; `describe` tells a setting's declaration to a holder of `schema` and to one who may write it, a holder of an `only_any` set of groups too (lakelands#1588), and to no other reader",
    given
);

/// `vat_rate`, a decimal of group `operator`, with its unit and meaning.
fn vat(group: &str) -> Value {
    json!({"declare": "setting", "key": "vat_rate", "type": {"kind": "decimal"},
        "unit": "fraction", "description": "The VAT rate on net amounts.", "group": group})
}

/// What a refusal says, invalid or a value its kind refuses.
fn said(r: &Result<impl std::fmt::Debug, Error>) -> Option<Said> {
    match r {
        Err(Error::Refused {
            reason: Refusal::Constraint(said),
            ..
        }) => Some((**said).clone()),
        _ => invalid_said(r).cloned(),
    }
}

async fn a_declared_setting_holds_its_kind(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(vat("operator")).await?;
    let head = store.head().await.must("reading the head")?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let bad = try_upsert(
        l.cat.as_ref(),
        &l.own,
        ty,
        "vat_rate",
        json!({"key": "vat_rate", "value": "19%"}),
    )
    .await;
    let at = said(&bad).map(|s| s.at);
    ensure!(
        matches!(&bad, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "value")
            && at.as_deref().is_some_and(|at| at.contains(&Place::Setting {
                key: "vat_rate".into()
            })),
        "`\"19%\"` for the decimal `vat_rate` is refused, naming `value` within the setting: {bad:?}"
    );
    head_unmoved(store.as_ref(), head, "a setting of the wrong kind").await?;
    // `value` is required: `null` is its absence, to the owner too.
    let null = json!({"key": "vat_rate", "value": null});
    let null = try_upsert(l.cat.as_ref(), &l.own, ty, "vat_rate", null).await;
    let required = Case::Required {
        ty: ty.into(),
        field: "value".into(),
    };
    ensure!(
        said(&null).map(|s| s.case) == Some(required),
        "a `null` value is refused as the absence of the required `value`: {null:?}"
    );
    head_unmoved(store.as_ref(), head, "a null setting").await?;
    l.set("vat_rate", json!("0.19")).await?;
    // Read as a decimal: computed, compared and filtered by. A constraint
    // cannot read a setting (`constraint_reads_beyond`), so none is held to
    // one.
    l.declare_json(json!({"declare": "type", "name": "line", "fields": [
        {"name": "net", "type": {"kind": "decimal"}},
        {"name": "gross", "type": {"kind": "decimal"}, "authority": "derived",
         "derived": "net * (1 + setting('vat_rate'))", "depends_on": ["net"]},
        {"name": "low", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "setting('vat_rate') < 1 and net > 0", "depends_on": ["net"]}]}))
        .await?;
    l.upsert("line", "l1", json!({"net": "100"})).await?;
    let line = l.read("line", "l1").await?.fields;
    let (gross, low) = (line.get("gross").cloned(), line.get("low").cloned());
    let find = Find {
        filter: Some("setting('vat_rate') > 0.1".into()),
        at: read_at(None),
        ..Find::default()
    };
    let found = l.all_found("line", find).await?.len();
    ensure!(
        gross
            .as_ref()
            .and_then(Value::as_str)
            .and_then(|g| g.parse::<f64>().ok())
            == Some(119.0)
            && low == Some(json!(true))
            && found == 1,
        "`setting('vat_rate')` reads `0.19` as a decimal: `gross` of 100 is 119 ({gross:?}), \
         `setting('vat_rate') < 1` is true ({low:?}), and the filter \
         `setting('vat_rate') > 0.1` finds `l1` ({found})"
    );
    // Kept in the form its kind keeps: a time with an offset, in UTC.
    l.declare_json(json!({"declare": "setting", "key": "cutoff",
        "type": {"kind": "date_time"}}))
        .await?;
    let written = "2026-10-16T09:00:00+02:00";
    l.set("cutoff", json!(written)).await?;
    let held = l.read(ty, "cutoff").await?.fields.get("value").cloned();
    let canonical = lakelands_api::Timestamp::parse(written).map(|t| t.to_string());
    ensure!(
        held.as_ref().and_then(Value::as_str) == canonical.as_deref().ok()
            && held != Some(json!(written)),
        "the `date_time` setting `cutoff` written `{written}` is held in the form its kind \
         keeps ({canonical:?}): {held:?}"
    );
    // What the declaration refuses.
    l.set("floor", json!("low")).await?;
    let refused = |key: &str, ty: &str, group: Option<&str>| {
        let mut d = json!({"declare": "setting", "key": key, "type": {"kind": ty}});
        if let Some(g) = group {
            d["group"] = json!(g);
        }
        let d = serde_json::from_value(d).expect("a declaration");
        l.cat.declare(&l.own, d)
    };
    let store_key = refused("sort_index.line", "json", None).await;
    let prefixed = refused("view_kept", "json", None).await;
    let store_group = refused("rounding", "text", Some("store")).await;
    let reference = refused("home", "reference", None).await;
    let misfit = refused("floor", "number", None).await;
    let case = |r: &Result<_, Error>| said(r).map(|s| s.case);
    ensure!(
        case(&store_key)
            == Some(Case::SettingStoreKey {
                prefix: "sort_index".into()
            })
            && case(&prefixed)
                == Some(Case::SettingStoreKey {
                    prefix: "view_kept".into()
                })
            && case(&store_group) == Some(Case::SettingStoreGroup)
            && case(&reference) == Some(Case::SettingReference)
            && said(&misfit).is_some_and(|s| s.at.contains(&Place::Setting {
                key: "floor".into()
            })),
        "a host may not declare a key the store reads ({store_key:?}), one under its prefix \
         ({prefixed:?}), the group `store` ({store_group:?}), a reference ({reference:?}), nor a kind \
         the held value does not fit ({misfit:?})"
    );
    Ok(())
}

async fn a_declared_setting_is_written_by_its_group(store: Store, given: Given) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    l.declare_json(vat("operator")).await?;
    l.declare_json(json!({"declare": "setting", "key": "embed_model",
        "type": {"kind": "text"}, "group": "system"}))
        .await?;
    l.set("vat_rate", json!("0.19")).await?;
    l.set("embed_model", json!("small")).await?;
    let group = async |key: &str| {
        let r = l.read(lakelands_api::seam::SETTING_TYPE, key).await;
        r.map(|r| r.fields.get("group").cloned().unwrap_or(Value::Null))
    };
    l.declare_json(json!({"declare": "type", "name": "line", "fields": [
        {"name": "due", "type": {"kind": "date"}}]}))
        .await?;
    l.set("sort_index.line", json!(["due"])).await?;
    let store_group = group("sort_index.line").await?;
    ensure!(
        store_group == json!("store"),
        "the store's own setting `sort_index.line` is stamped `store`: {store_group}"
    );
    let (vat_group, model_group) = (group("vat_rate").await?, group("embed_model").await?);
    // A write naming another group.
    let ty = lakelands_api::seam::SETTING_TYPE;
    let renamed = try_upsert(
        l.cat.as_ref(),
        &l.own,
        ty,
        "vat_rate",
        json!({"key": "vat_rate", "value": "0.2", "group": "system"}),
    )
    .await;
    let stamped = Said {
        at: vec![Place::Setting {
            key: "vat_rate".into(),
        }],
        case: Case::SettingGroupStamped,
    };
    ensure!(
        vat_group == json!("operator")
            && model_group == json!("system")
            && said(&renamed).as_ref() == Some(&stamped),
        "the store stamps `operator` on `vat_rate` ({vat_group}) and `system` on \
         `embed_model` ({model_group}); a write naming another group is refused naming \
         `group`: {renamed:?}"
    );
    // A plain reader reads the group as it reads the value: a field of
    // the record, under `sys.setting`'s grants.
    let reader = holder(s, "reader", json!({"default": "read"}));
    let read = fixture::read(l.cat.as_ref(), &reader, ty, "embed_model").await?;
    let by = fixture::find_where("group = 'system'");
    let found = fixture::all_found(l.cat.as_ref(), &reader, ty, by).await?;
    let found: Vec<String> = found.into_iter().map(|r| r.key.0).collect();
    ensure!(
        read.fields.get("group") == Some(&json!("system")) && found == ["embed_model"],
        "a reader of `sys.setting` reads `embed_model`'s group as a field ({:?}) and finds it \
         by `group = 'system'` ({found:?})",
        read.fields.get("group")
    );
    // The holder of a group.
    let operator = group_holder(s, "operator");
    let fields = |key: &str, value: Value| json!({"key": key, "value": value});
    let own = try_upsert(
        l.cat.as_ref(),
        &operator,
        ty,
        "vat_rate",
        fields("vat_rate", json!("0.2")),
    )
    .await;
    // A write of another group's setting, and a probe of it, are answered
    // as in its twin, a lake where nobody declared `embed_model`: a value
    // its kind refuses, a group guessed right or wrong.
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = Lake::of(twin.as_ref());
    t.set("embed_model", json!("small")).await?;
    let mut apart = Vec::new();
    for asked in [
        json!({"key": "embed_model", "value": "large"}),
        json!({"key": "embed_model", "value": 5}),
        json!({"key": "embed_model", "value": "x", "group": "system"}),
        json!({"key": "embed_model", "value": "x", "group": "finance"}),
    ] {
        let probe = async |l: &Lake| {
            let r = try_upsert(l.cat.as_ref(), &operator, ty, "embed_model", asked.clone()).await;
            format!("{:?}", r.err())
        };
        let (here, there) = (probe(&l).await, probe(&t).await);
        if here != there || here == "None" {
            apart.push(format!("{asked}: {here} / {there}"));
        }
    }
    ensure!(
        own.is_ok() && apart.is_empty(),
        "a holder of `only {{group: operator}}` writes `vat_rate` ({own:?}), and is refused \
         `embed_model` of group `system` as in a lake that never declared it; apart: {apart:?}"
    );
    // What `describe` tells.
    let told = |d: lakelands_api::describe::LakeDescription| {
        let s = d
            .settings
            .into_iter()
            .filter_map(|s| Some((s.key, s.declared?.group)));
        s.collect::<Vec<_>>()
    };
    let owner = told(l.describe().await?);
    let told_to = async |who: &lakelands_api::Principal| {
        let d = l.cat.describe(who, Default::default()).await;
        d.must("describing to operator").map(|d| told(d.data))
    };
    let theirs = told_to(&operator).await?;
    // A holder of a set of groups is told the declarations of each
    // (lakelands#1588).
    let groups = json!({"default": "write", "only_any": {"group": ["operator", "system"]}});
    let types = json!({ lakelands_api::seam::SETTING_TYPE: groups });
    let groups = holder(s, "groups", json!({"default": "read", "types": types}));
    let sets = told_to(&groups).await?;
    let none = told(
        l.cat
            .describe(&reader, Default::default())
            .await
            .must("describing to reader")?
            .data,
    );
    let both = vec![
        ("embed_model".to_owned(), Some("system".to_owned())),
        ("vat_rate".to_owned(), Some("operator".to_owned())),
    ];
    ensure!(
        owner == both && theirs == both[1..] && sets == both && none.is_empty(),
        "`describe` tells the owner, who holds `schema`, both declarations ({owner:?}), the \
         holder of `operator` `vat_rate`'s alone ({theirs:?}), the holder of both groups both \
         ({sets:?}), and a reader none ({none:?})"
    );
    // Moved to another group: stamped again in the declaration's append.
    let head = store.head().await.must("reading the head")?;
    l.declare_json(vat("finance")).await?;
    let after = store.head().await.must("reading the head")?;
    let moved = group("vat_rate").await?;
    let now = fields("vat_rate", json!("0.21"));
    let now = try_upsert(l.cat.as_ref(), &operator, ty, "vat_rate", now).await;
    t.set("vat_rate", json!("0.19")).await?;
    let there = fields("vat_rate", json!("0.21"));
    let there = try_upsert(t.cat.as_ref(), &operator, ty, "vat_rate", there).await;
    twin.close().await.must("closing the twin")?;
    let gone = told_to(&operator).await?;
    ensure!(
        gone.is_empty(),
        "moved to `finance`, `vat_rate` is told to the holder of `operator` no more: {gone:?}"
    );
    ensure!(
        moved == json!("finance")
            && after.0 == head.0 + 2
            && now.is_err()
            && format!("{now:?}") == format!("{there:?}"),
        "declared into `finance`, `vat_rate` is stamped again in the declaration's append \
         (head {head} → {after}, group {moved}), and the holder of `operator` is refused it \
         as in a lake that never declared it: {now:?} / {there:?}"
    );
    // A proposal to create a key is stamped at its accept, with the group
    // declared then; it is listed without one.
    let rounding = |group: &str| json!({"declare": "setting", "key": "rounding", "type": {"kind": "text"}, "group": group});
    l.declare_json(rounding("operator")).await?;
    let proposed = ProposedWrite::Upsert {
        ty: ty.into(),
        key: "rounding".into(),
        patch: fixture::patch(fields("rounding", json!("half_up"))),
        opts: Default::default(),
    };
    let proposed = l.cat.propose(&l.own, proposed, "rounding".into()).await;
    let id = match proposed.must("proposing `rounding`")? {
        lakelands_api::Outcome::Proposed { proposal, .. } => proposal,
        other => return Err(Failure::new(format!("proposing answered {other:?}"))),
    };
    l.declare_json(rounding("finance")).await?;
    let accepted = l.cat.accept(&l.own, &id).await;
    let listed = l.cat.proposals(&l.own, ProposalFilter::default()).await;
    let listed = listed.must("listing the proposals")?.data;
    let patch = (listed.into_iter().find(|p| p.id == id)).and_then(|p| match p.write {
        Some(ProposedWrite::Upsert { patch, .. }) => Some(patch),
        _ => None,
    });
    let stamped = group("rounding").await.ok();
    ensure!(
        accepted.is_ok()
            && stamped == Some(json!("finance"))
            && patch.as_ref().is_some_and(|p| !p.contains_key("group")),
        "proposed while `rounding` was of `operator` and accepted after it moved to `finance`: \
         the accept answers {accepted:?}, the record's group is {stamped:?}, and the listed \
         patch {patch:?} holds no group"
    );
    Ok(())
}

check!(
    HOST_SETTING_PERIOD_WRITES = a_declared_setting_takes_values_from_dates,
    "090 (lakelands#1549): a declared setting takes a value from a date by `from` and a correction by `period`, a date or `start`, kept in `periods` in the order of their dates; `from` on a retired or undeclared key, on a held date or on a store's own key, `period` on no held date, an undated value on a dated record, a patch naming `periods` and one naming both members are each refused by their case, naming the member, appending nothing; a first value and a period written one after the other land; a proposed add is resolved at its accept unless its proposal is pinned to a record that changed since"
);

check!(
    HOST_SETTING_PERIOD_GRANTS = a_period_is_written_as_the_setting_is,
    "090 (lakelands#1549): a period is judged by the grants that judge its setting: a holder of `only {group: finance}` adds and corrects a period of a `finance` setting, and is answered on an `operator` one as in a twin where nobody declared it; a writer granted `value` and not `periods` corrects `start` and is refused an add",
    given
);

check!(
    HOST_SETTING_PERIODS_TOLD = describe_tells_a_setting_s_periods,
    "090 (lakelands#1549): `describe` tells a dated setting's periods in order, and the value in force on its clock, to a holder of `schema` and to one who may write the setting, and to no other reader"
);

/// A write of `key` from `members`.
fn at_key(key: &str, members: Value) -> Value {
    let mut fields = json!({"key": key});
    if let (Some(f), Value::Object(m)) = (fields.as_object_mut(), members) {
        f.extend(m);
    }
    fields
}

/// A write of `vat_rate` from `members`.
fn vat_at(members: Value) -> Value {
    at_key("vat_rate", members)
}

/// A holder of `only {group: <group>}` on `sys.setting`.
fn group_holder(s: &dyn LakeStore, group: &str) -> lakelands_api::Principal {
    let only = json!({"default": "write", "only": {"group": group}});
    let types = json!({ lakelands_api::seam::SETTING_TYPE: only });
    holder(s, group, json!({"default": "read", "types": types}))
}

/// `vat_rate`'s first value and its periods, as its record holds them.
async fn periods_of(l: &Lake) -> Result<String, Failure> {
    let r = l.read(lakelands_api::seam::SETTING_TYPE, "vat_rate");
    let r = r.await?.fields;
    let get = |m: &str| r.get(m).cloned().unwrap_or(Value::Null);
    Ok(format!("{} {}", get("value"), get("periods")))
}

async fn a_declared_setting_takes_values_from_dates(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    let ty = lakelands_api::seam::SETTING_TYPE;
    l.declare_json(vat("operator")).await?;
    l.declare_json(json!({"declare": "setting", "key": "old", "type": {"kind": "number"}}))
        .await?;
    l.declare(type_decl("line", vec![field("due", StorageKind::Date)]))
        .await?;
    l.set("sort_index.line", json!(["due"])).await?;
    l.set("loose", json!(1)).await?;
    l.set("old", json!(1)).await?;
    l.retire(ty, "old").await?;
    // A first value and a period one after the other, then the rest.
    l.set("vat_rate", json!("0.15")).await?;
    l.set("vat_rate", json!("0.16")).await?;
    for members in [
        json!({"from": "2026-07-01", "value": "0.19"}),
        json!({"from": "2026-01-01", "value": "0.18"}),
        json!({"period": "2026-07-01", "value": "0.2"}),
        json!({"period": "start", "value": "0.17"}),
    ] {
        l.upsert(ty, "vat_rate", vat_at(members)).await?;
    }
    let held = periods_of(&l).await?;
    let want =
        r#""0.17" [{"from":"2026-01-01","value":"0.18"},{"from":"2026-07-01","value":"0.2"}]"#;
    ensure!(
        held == want,
        "an add, an add before it and two corrections leave `vat_rate` at {held}, where \
         they leave it at {want}"
    );
    // The key, the write's members, the member named and the case.
    let refusals = json!([
        ["old", {"from": "2026-07-01", "value": 2}, "from", {"code": "setting_period_first"}],
        ["vat_rate", {"from": "2026-07-01", "value": "0.2"}, "from",
            {"code": "setting_period_held", "from": "2026-07-01"}],
        ["vat_rate", {"period": "2026-03-01", "value": "0.2"}, "period",
            {"code": "setting_period_unknown", "period": "2026-03-01"}],
        ["vat_rate", {"value": "0.21"}, "value", {"code": "setting_periods_named"}],
        ["vat_rate", {"periods": [], "value": "0.21"}, "periods", {"code": "setting_periods_stamped"}],
        ["vat_rate", {"from": "2026-09-01", "period": "start", "value": "0.2"}, "period",
            {"code": "setting_from_or_period"}],
        ["sort_index.line", {"from": "2026-07-01", "value": ["due"]}, "from",
            {"code": "setting_period_undeclared"}],
        ["loose", {"from": "2026-07-01", "value": 2}, "from", {"code": "setting_period_undeclared"}]
    ]);
    let head = store.head().await.must("reading the head")?;
    let mut wrong = Vec::new();
    for row in refusals.as_array().into_iter().flatten() {
        let (key, member) = (row[0].as_str().unwrap_or_default(), &row[2]);
        let fields = at_key(key, row[1].clone());
        let said = try_upsert(l.cat.as_ref(), &l.own, ty, key, fields.clone()).await;
        let at = [Place::Setting { key: key.into() }];
        let case = |s: &Said| serde_json::to_value(&s.case).ok().as_ref() == Some(&row[3]);
        let right = matches!(refused_on(&said), Some((m, _)) if m == member)
            && invalid_said(&said).is_some_and(|s| case(s) && s.at == at);
        if !right {
            wrong.push(format!("{fields}: {said:?}"));
        }
    }
    head_unmoved(s, head, "a refused write of a period").await?;
    ensure!(
        wrong.is_empty(),
        "each refusal names its member and case within the setting: {wrong:?}"
    );
    // Proposed: resolved at the accept, unless pinned.
    let operator = group_holder(s, "operator");
    let propose = async |who: &lakelands_api::Principal, members: Value| {
        let write = ProposedWrite::Upsert {
            ty: ty.into(),
            key: "vat_rate".into(),
            patch: fixture::patch(vat_at(members)),
            opts: Default::default(),
        };
        match l.cat.propose(who, write, "a period".into()).await {
            Ok(lakelands_api::Outcome::Proposed { proposal, .. }) => Ok(proposal),
            other => Err(Failure::new(format!("proposing answered {other:?}"))),
        }
    };
    let open = propose(&l.own, json!({"from": "2026-10-01", "value": "0.22"})).await?;
    let pinned = propose(&operator, json!({"from": "2026-11-01", "value": "0.23"})).await?;
    let other = vat_at(json!({"from": "2026-04-01", "value": "0.185"}));
    l.upsert(ty, "vat_rate", other).await?;
    let accepted = l.cat.accept(&l.own, &open).await;
    let refused = l.cat.accept(&l.own, &pinned).await;
    let held = periods_of(&l).await?;
    let froms = ["2026-01-01", "2026-04-01", "2026-07-01", "2026-10-01"];
    ensure!(
        accepted.is_ok()
            && refused.is_err()
            && froms.iter().all(|f| held.contains(f))
            && !held.contains("2026-11-01"),
        "a proposed add is resolved at its accept after another add ({accepted:?}); a pinned \
         one is refused there ({refused:?}): {held}"
    );
    Ok(())
}

async fn a_period_is_written_as_the_setting_is(store: Store, given: Given) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    let ty = lakelands_api::seam::SETTING_TYPE;
    l.declare_json(vat("finance")).await?;
    let fee = json!({"declare": "setting", "key": "fee", "type": {"kind": "number"}, "group": "operator"});
    l.declare_json(fee).await?;
    l.set("vat_rate", json!("0.16")).await?;
    l.set("fee", json!(1)).await?;
    let finance = group_holder(s, "finance");
    let write = async |l: &Lake, who: &lakelands_api::Principal, key: &str, members: Value| {
        try_upsert(l.cat.as_ref(), who, ty, key, at_key(key, members)).await
    };
    let add = json!({"from": "2026-07-01", "value": "0.19"});
    let fix = json!({"period": "2026-07-01", "value": "0.2"});
    let added = write(&l, &finance, "vat_rate", add).await;
    let corrected = write(&l, &finance, "vat_rate", fix).await;
    ensure!(
        added.is_ok() && corrected.is_ok(),
        "a holder of `only {{group: finance}}` adds a period of `vat_rate` ({added:?}) and \
         corrects it ({corrected:?})"
    );
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = Lake::of(twin.as_ref());
    t.set("fee", json!(1)).await?;
    let mut apart = Vec::new();
    for members in [
        json!({"from": "2026-07-01", "value": 2}),
        json!({"period": "start", "value": 2}),
        json!({"period": "2026-07-01", "value": "x"}),
    ] {
        let here = write(&l, &finance, "fee", members.clone()).await.err();
        let there = write(&t, &finance, "fee", members.clone()).await.err();
        let (here, there) = (format!("{here:?}"), format!("{there:?}"));
        if here != there || !here.contains("SettingPeriodUndeclared") {
            apart.push(format!("{members}: {here} / {there}"));
        }
    }
    twin.close().await.must("closing the twin")?;
    ensure!(
        apart.is_empty(),
        "the holder of `finance` is answered `setting_period_undeclared` on the `operator` \
         setting `fee`, as in a twin where nobody declared it: {apart:?}"
    );
    let valuer = fixture::field_writer(s, "valuer", ty, &["key", "value"]);
    let start = json!({"period": "start", "value": "0.15"});
    let add = json!({"from": "2026-09-01", "value": "0.21"});
    let start = write(&l, &valuer, "vat_rate", start).await;
    let head = store.head().await.must("reading the head")?;
    let add = write(&l, &valuer, "vat_rate", add).await;
    head_unmoved(s, head, "an add by a writer not granted `periods`").await?;
    let not_granted = format!("{add:?}").contains("NotGranted");
    ensure!(
        start.is_ok() && not_granted,
        "a writer granted `value` and not `periods` corrects `start` ({start:?}) and is \
         refused an add ({add:?})"
    );
    Ok(())
}

async fn describe_tells_a_setting_s_periods(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    let ty = lakelands_api::seam::SETTING_TYPE;
    l.declare_json(vat("finance")).await?;
    l.set("vat_rate", json!("0.16")).await?;
    for (from, value) in [("9000-01-01", "0.25"), ("2000-01-01", "0.18")] {
        let period = vat_at(json!({"from": from, "value": value}));
        l.upsert(ty, "vat_rate", period).await?;
    }
    let finance = group_holder(s, "finance");
    let reader = holder(s, "reader", json!({"default": "read"}));
    let told = async |who: &lakelands_api::Principal| {
        let d = l.cat.describe(who, Default::default()).await;
        let d = d.must("describing the lake")?.data;
        let vat = d.settings.into_iter().find(|s| s.key == "vat_rate");
        Ok::<_, Failure>(vat.map(|s| {
            let p = s.periods.iter();
            let p: Vec<_> = p.map(|p| format!("{:?} {}", p.from, p.value)).collect();
            format!("{:?}: {}", s.value, p.join(", "))
        }))
    };
    let want = r#"Some(String("0.18")): None "0.16", Some("2000-01-01") "0.18", Some("9000-01-01") "0.25""#;
    let (owner, theirs) = (told(&l.own).await?, told(&finance).await?);
    let none = told(&reader).await?;
    ensure!(
        owner.as_deref() == Some(want) && theirs.as_deref() == Some(want) && none.is_none(),
        "`describe` tells `vat_rate` as `{want}` to the owner ({owner:?}) and to a holder of \
         its group ({theirs:?}), and to a plain reader nothing ({none:?})"
    );
    Ok(())
}
