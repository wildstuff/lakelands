//! A host's own setting, declared (063, extended, lakelands#996): every
//! write of its key is judged as a write of a field of its type, the
//! store stamps its group, which a grant's `only` names, and `describe`
//! tells its declaration to a holder of `schema` and to one who may write
//! it.

use lakelands_api::error::{Case, Place, Said};
use lakelands_api::write::{ProposalFilter, ProposedWrite};
use lakelands_api::{Error, Find, Refusal};
use serde_json::{Value, json};

use super::only::holder;
use crate::conformance::fixture::{
    self, Lake, Must, head_unmoved, invalid_said, read_at, try_upsert,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    HOST_SETTING_KIND = a_declared_setting_holds_its_kind,
    "063, extended (lakelands#996): a setting a host declares with a type is judged at every write as a field of that type: `\"19%\"` for a `decimal` is refused, naming `value` within the setting, appending nothing, and a decimal is taken; `null` is refused as the absence of the required `value`, to the owner too; `setting('<key>')` reads it as a decimal, in a derived field's arithmetic and comparison and in a `find`'s filter alike; a declaration of a key the store reads, or under its prefix, or into the group `store`, of a reference, and one of a key whose held value its type refuses, are refused, naming the setting; a value is held in the form its kind keeps (a `date_time` in UTC)"
);

check!(
    HOST_SETTING_GROUP = a_declared_setting_is_written_by_its_group,
    "063, extended (lakelands#996): the store stamps a setting's `group` from its declaration, `store` on its own settings (`sort_index.<type>`), and a write naming another is refused, naming `group`; a reader of `sys.setting` reads the group as a field and finds by it; a holder of `only {group: operator}` on `sys.setting` writes a setting of that group and is answered on one of another group as in a twin lake where nobody declared it: a value, a value the other setting's kind refuses, and a group guessed right or wrong; a declaration that moves a setting to another group stamps its record again in its own append, after which the holder is refused it as in that twin and is told its declaration no more; a proposal to create a key accepted after its declaration moved it is stamped with the group then, and is listed with no group; `describe` tells a setting's declaration to a holder of `schema` and to one who may write it, and to no other reader",
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
    let operator = holder(
        s,
        "operator",
        json!({"default": "read", "types": {
        ty: {"default": "write", "only": {"group": "operator"}}}}),
    );
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
        owner == both && theirs == both[1..] && none.is_empty(),
        "`describe` tells the owner, who holds `schema`, both declarations ({owner:?}), the \
         holder of `operator` `vat_rate`'s alone ({theirs:?}), and a reader none ({none:?})"
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
