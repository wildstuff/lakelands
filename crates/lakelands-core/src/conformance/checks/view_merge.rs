//! A view that merges sources on its key (`lakelands_api::ontology::ViewSource`).
//!
//! The fixture is an energy balance: `food_log` and `workout_log`, two
//! types that know nothing of each other, each holding a day and the
//! calories of an entry. The view `energy_balance` sums the food by day,
//! merges the workouts summed by their date, and nets the two.

use lakelands_api::error::Case;
use lakelands_api::ontology::{StorageKind, TypeDecl, ViewParam, ViewSource};
use lakelands_api::read::Find;
use lakelands_api::{Access, Error, FieldName, LakeStore, Params, Refusal, Value};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, bound, field, field_access, invalid_case, invalid_like, measure, param, type_decl,
    view_of,
};
use crate::conformance::{Failure, Held, Store};

check!(
    MERGED_ON_KEY = a_view_merges_its_sources_on_their_key_and_keeps_a_key_only_one_has,
    "018, extended: a view's merged sources meet its own where their keys are identical; every key any source has is a record, a source that lacks one measures its empty group (a sum of none is 0, a minimum null), and the view's measures read the merged ones by name, byte for byte the same on every engine"
);

check!(
    KEYS_MEET_BY_IDENTITY = keys_from_two_sources_meet_only_when_their_values_are_identical,
    "018 and 044: merged keys meet by the identity of their values, as a derived relation's do, so the text '010' and the text '10' are two keys, where 017's `=` would call them equal"
);

check!(
    PARAMS_BIND_EVERY_SOURCE = a_views_parameters_bind_every_source_it_merges,
    "018: a view's parameters are bound by the read and read by every source's `where` and measures, the merged ones too"
);

check!(
    MERGE_DECLARED_STRICTLY = a_merge_declared_badly_is_refused_naming_it,
    "028 and 018: `declare` refuses a merged source that is not declared, read twice, grouped by another number of fields or a field of another kind than the one it lines up with, a measure named twice, a merged measure with no declared field or one that calls no aggregate, a view with parameters as any source, and a merge that closes a cycle of views, naming it"
);

check!(
    MERGE_PROBES_EACH_SOURCE = a_merged_sources_fields_are_probed_as_a_read_of_it,
    "024 and 018: a view reads each of its sources as a read of that source, so a principal who may not read a field a merged source's measures aggregate is not answered the view's definition by describe, and a read of the view is refused as not granted, naming neither that field nor its source (024, extended)"
);

fn log(name: &str, day: &str) -> TypeDecl {
    type_decl(
        name,
        vec![
            field(day, StorageKind::DateTime),
            field("kcal", StorageKind::Decimal),
        ],
    )
}

fn workouts(filter: Option<&str>) -> ViewSource {
    ViewSource {
        source: "workout_log".into(),
        r#where: filter.map(Into::into),
        group_by: vec![FieldName::from("date")],
        measures: vec![
            measure("calories_out", "sum(kcal)"),
            measure("hardest", "max(kcal)"),
        ],
    }
}

fn energy_balance(
    name: &str,
    params: Vec<ViewParam>,
    food: Option<&str>,
    work: Option<&str>,
) -> TypeDecl {
    let fields = vec![
        field("day", StorageKind::DateTime),
        field("calories_in", StorageKind::Decimal),
        field("calories_out", StorageKind::Decimal),
        field("hardest", StorageKind::Decimal),
        field("net_balance", StorageKind::Decimal),
    ];
    let measures = [
        ("calories_in", "sum(kcal)"),
        ("net_balance", "calories_in - calories_out"),
    ];
    let mut t = view_of(name, "food_log", &["day"], fields, &measures);
    if let Some(v) = t.view.as_mut() {
        v.params = params;
        v.r#where = food.map(Into::into);
        v.merge = vec![workouts(work)];
    }
    t
}

/// The two logs, three meals and two workouts over three days; the
/// workouts' dates are written with a time, which is the same instant.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(log("food_log", "day")).await?;
    l.declare(log("workout_log", "date")).await?;
    for (key, day, kcal) in [
        ("f1", "2026-06-01", "2000.00"),
        ("f2", "2026-06-01", "500.00"),
        ("f3", "2026-06-02", "1800.00"),
    ] {
        l.upsert("food_log", key, json!({"day": day, "kcal": kcal}))
            .await?;
    }
    for (key, date, kcal) in [
        ("w1", "2026-06-01T00:00:00Z", "300.00"),
        ("w2", "2026-06-03T00:00:00Z", "400.00"),
    ] {
        l.upsert("workout_log", key, json!({"date": date, "kcal": kcal}))
            .await?;
    }
    Ok(l)
}

/// The merged source `source`, grouped by `by`, measuring `name` as `expr`.
fn merged(source: &str, by: &str, name: &str, expr: &str) -> ViewSource {
    ViewSource {
        source: source.into(),
        r#where: None,
        group_by: vec![FieldName::from(by)],
        measures: vec![measure(name, expr)],
    }
}

async fn rows(l: &Lake, view: &str, params: Params) -> Result<Value, Failure> {
    let found = l.all_found(view, bound(params)).await?;
    Ok(json!(found.iter().map(|r| &r.fields).collect::<Vec<_>>()))
}

async fn a_view_merges_its_sources_on_their_key_and_keeps_a_key_only_one_has(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    l.declare(energy_balance("energy_balance", Vec::new(), None, None))
        .await?;
    let got = rows(&l, "energy_balance", Params::new()).await?;
    let want = json!([
        {"day": "2026-06-01T00:00:00.000000Z", "calories_in": "2500.00", "calories_out": "300.00",
         "hardest": "300.00", "net_balance": "2200.00"},
        {"day": "2026-06-02T00:00:00.000000Z", "calories_in": "1800.00", "calories_out": "0",
         "net_balance": "1800.00"},
        {"day": "2026-06-03T00:00:00.000000Z", "calories_in": "0", "calories_out": "400.00",
         "hardest": "400.00", "net_balance": "-400.00"},
    ]);
    ensure!(
        got == want,
        "the energy balance must be {want}: the first of June from both logs, the second \
         from the meals alone with no workout (calories out 0, hardest none), the third from \
         the workout alone (calories in 0); it is {got}"
    );
    Ok(())
}

async fn keys_from_two_sources_meet_only_when_their_values_are_identical(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    for ty in ["left_log", "right_log"] {
        l.declare(type_decl(ty, vec![field("code", StorageKind::Text)]))
            .await?;
    }
    l.upsert("left_log", "a", json!({"code": "010"})).await?;
    l.upsert("right_log", "b", json!({"code": "10"})).await?;
    let fields = vec![
        field("code", StorageKind::Text),
        field("left", StorageKind::Number),
        field("right", StorageKind::Number),
    ];
    let left = [("left", "count()")];
    let mut view = view_of("codes", "left_log", &["code"], fields, &left);
    if let Some(v) = view.view.as_mut() {
        v.merge = vec![merged("right_log", "code", "right", "count()")];
    }
    l.declare(view).await?;
    let got = rows(&l, "codes", Params::new()).await?;
    let want = json!([
        {"code": "010", "left": 1, "right": 0},
        {"code": "10", "left": 0, "right": 1},
    ]);
    ensure!(
        got == want,
        "the codes '010' and '10' are two keys, each counted by one source; the view holds \
         {got}, and it must hold {want}"
    );
    Ok(())
}

async fn a_views_parameters_bind_every_source_it_merges(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    l.declare(energy_balance(
        "energy_until",
        vec![param("until", StorageKind::DateTime)],
        Some("day <= param('until')"),
        Some("date <= param('until')"),
    ))
    .await?;
    let bind: Params = [("until".into(), json!("2026-06-02"))].into();
    let got = rows(&l, "energy_until", bind).await?;
    let want = json!([
        {"day": "2026-06-01T00:00:00.000000Z", "calories_in": "2500.00", "calories_out": "300.00",
         "hardest": "300.00", "net_balance": "2200.00"},
        {"day": "2026-06-02T00:00:00.000000Z", "calories_in": "1800.00", "calories_out": "0",
         "net_balance": "1800.00"},
    ]);
    ensure!(
        got == want,
        "up to the second of June the balance must leave out the workout of the third, which \
         the merged source's `where` reads the parameter for; it is {got}, and must be {want}"
    );
    Ok(())
}

async fn a_merge_declared_badly_is_refused_naming_it(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let base = || energy_balance("bad_balance", Vec::new(), None, None);
    let mut cases: Vec<(&str, TypeDecl, Value)> = Vec::new();
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0].source = "sleep_log".into();
    let case = json!({"code": "undeclared", "type": "sleep_log"});
    cases.push(("merges an undeclared type", t, case));
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0].source = "food_log".into();
    let case = json!({"code": "source_twice", "source": "food_log"});
    cases.push(("merges its own source", t, case));
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0]
        .group_by
        .push(FieldName::from("kcal"));
    let twice = json!({"code": "merged_group_count", "given": 2, "view": 1,
                       "at": [{"code": "merged_source", "name": "workout_log"}]});
    cases.push(("groups a merge by two fields", t, twice));
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0].group_by = vec![FieldName::from("kcal")];
    let case = json!({"code": "merged_group_kind", "field": "kcal", "view_field": "day"});
    cases.push(("lines up a decimal with a date", t, case));
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0].measures[0].name = "calories_in".into();
    let case = json!({"code": "measure_twice", "name": "calories_in"});
    cases.push(("names a measure twice", t, case));
    let mut t = base();
    t.fields.retain(|f| f.name != "hardest");
    let case = json!({"code": "view_value_no_field", "field": "hardest"});
    cases.push(("has a merged measure without a field", t, case));
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0].measures[1].expr = "1".into();
    cases.push((
        "has a merged measure that aggregates nothing",
        t,
        json!({"code": "measures_nothing", "source": "workout_log",
               "at": [{"code": "measure", "name": "hardest"}]}),
    ));
    for (what, t, case) in cases {
        let r = l.try_declare(t).await;
        ensure!(
            invalid_like(&r, &case),
            "a view that {what} must be refused as invalid, as {case}; declare answered {r:?}"
        );
    }
    // A view with parameters as a source, main or merged.
    l.declare(energy_balance(
        "energy_until",
        vec![param("until", StorageKind::DateTime)],
        Some("day <= param('until')"),
        None,
    ))
    .await?;
    let mut t = base();
    t.view.as_mut().expect("a view").merge[0] = merged("energy_until", "day", "over", "count()");
    let r = l.try_declare(t).await;
    ensure!(
        matches!(invalid_case(&r), Some(Case::ReadsParamView { source, param })
            if source == "energy_until" && param == "until"),
        "a view merging `energy_until`, a view with the parameter `until`, must be refused, \
         naming both; declare answered {r:?}"
    );
    // A merge closing a cycle: `cycle_a` merges `cycle_b`, which reads it.
    l.declare(energy_balance("cycle_a", Vec::new(), None, None))
        .await?;
    let mut b = energy_balance("cycle_b", Vec::new(), None, None);
    if let Some(v) = b.view.as_mut() {
        v.source = "cycle_a".into();
        v.merge.clear();
        v.measures = vec![measure("calories_in", "count()")];
    }
    b.fields.truncate(2);
    l.declare(b).await?;
    let mut a = energy_balance("cycle_a", Vec::new(), None, None);
    a.view.as_mut().expect("a view").merge[0] = merged("cycle_b", "day", "calories_out", "count()");
    let r = l.try_declare(a).await;
    ensure!(
        matches!(invalid_case(&r), Some(Case::ViewCycle { chain })
            if ["cycle_a", "cycle_b"].iter().all(|v| chain.0.iter().any(|c| c == v))),
        "`cycle_a` merging `cycle_b`, which reads `cycle_a`, closes a cycle and must be \
         refused, naming both; declare answered {r:?}"
    );
    Ok(())
}

async fn a_merged_sources_fields_are_probed_as_a_read_of_it(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    l.declare(energy_balance("energy_balance", Vec::new(), None, None))
        .await?;
    let kcal = |id, access| field_access(store.as_ref(), id, "workout_log", &[("kcal", access)]);
    let blind = kcal("blind", Access::None);
    // describe answers the definition only to a principal who reads what
    // every source's measures aggregate.
    let reader = kcal("reader", Access::Read);
    for (who, sees) in [(&reader, true), (&blind, false)] {
        let d = l.cat.describe(who, Default::default());
        let d = d
            .await
            .map_err(|e| Failure::new(format!("describing the lake as `{}`: {e}", who.id)))?
            .data;
        let shown = d
            .types
            .iter()
            .any(|t| t.name == "energy_balance" && t.view.is_some());
        ensure!(
            shown == sees,
            "`{}` {} `workout_log.kcal`, which the merged source sums; describe must {} the \
             definition of `energy_balance`",
            who.id,
            if sees { "reads" } else { "may not read" },
            if sees { "answer" } else { "leave out" }
        );
    }
    let r = l
        .cat
        .find(&blind, &"energy_balance".into(), Find::default())
        .await;
    // Not granted, no field: an error that has no member to name either in.
    let unnamed = matches!(
        &r,
        Err(Error::Refused {
            field: None,
            reason: Refusal::NotGranted
        })
    );
    ensure!(
        unnamed,
        "a read of the balance by a principal who may not know of `workout_log.kcal` is \
         refused as not granted, naming neither the field nor its source; it answered {r:?}"
    );
    Ok(())
}
