//! A create that must send a value, and what `describe` tells of it
//! (`docs/decisions/059-a-type-declares-its-constraints.md`, extended): a
//! constraint is judged over the record a create leaves, so a field whose
//! `initial` or null it refuses is one the create must send, and a create
//! no sender can make hold is told closed.

use lakelands_api::error::Case;
use lakelands_api::{Access, Error, Refusal};
use serde_json::{Value, json};

use crate::conformance::fixture::{Lake, Must, field_access, head_unmoved, reason, try_upsert};
use crate::conformance::{Held, Store};

check!(
    REQUIRED_AT_CREATE = a_constraint_over_what_no_create_sends_is_judged_at_the_declaration_and_one_over_what_it_sends_is_told,
    "059 and 050, extended (lakelands#1169): a constraint that reads only fields no create can send, those only verbs write and those computed from them, is judged over the record a create leaves: `positive: score > 0` over a `score` derived from a `rank` only verbs write, whose `initial` is 0, is refused `initial_breaks` naming `rank`, and over a `rank` with no `initial` `no_initial_breaks`, appending nothing, and taken once `rank` holds 1; one that reads an authored field is taken as it was: `size > 0` over a `size` with `initial` 0 is declared, `describe_type` tells `size` `required_at_create` and no field beside it, and over a `total` derived from `a + b` tells `a` and `b` and not `total`; a key's field, which a create fills from its key, is asked for by no constraint on it, `code > 0` nor `code > 0 or size > 0`, and a create of key `5` sending only `note` is taken; the generated `upsert` tool says a create sends it, and a create without it is refused `constraint_broken` naming the constraint where one that sends it is taken"
);

check!(
    REQUIRED_TWIN = required_at_create_is_told_only_to_a_reader_shown_the_constraint,
    "059 and 024, extended (lakelands#1169): `required_at_create` is told to a reader shown both the field and the constraint: a reader who reads `size` but not `other`, which `both: size > 0 and other > 0` reads too, is told neither field, and one shown both is told both"
);

check!(
    REQUIRED_OUTSIDE = a_constraint_a_setting_or_the_clock_decides_is_judged_at_the_write,
    "059, extended (lakelands#1169): a constraint over a field derived from a setting, or from the clock and a field only verbs write whose `initial` is long past, reads no field a create sends and is taken at the declaration, its value the write's to judge; a create where it holds is taken, and no field is told `required_at_create`; joined by `and` to `rank > 0` over a `rank` only verbs write with `initial` 0, it is refused `initial_breaks` naming `rank`, also where the setting's part reads a field only verbs write with no `initial`, and to `rank >= 0` it is taken, and so is a create"
);

/// The setting `open`, declared and set to `true`.
async fn opened(l: &Lake) -> Held {
    l.declare_json(json!({"declare": "setting", "key": "open", "type": {"kind": "boolean"}}))
        .await?;
    l.set("open", json!(true)).await?;
    Ok(())
}

fn typed(name: &str, fields: Value, constraints: Value) -> Value {
    let mut fields = fields.as_array().cloned().unwrap_or_default();
    fields.insert(0, json!({"name": "no", "type": {"kind": "text"}}));
    json!({"declare": "type", "name": name, "fields": fields, "constraints": constraints})
}

fn broken(e: &Error) -> Option<&str> {
    match reason(e) {
        Some(Refusal::Constraint(said)) => match &said.case {
            Case::ConstraintBroken { constraint, .. } => Some(constraint.as_str()),
            _ => None,
        },
        _ => None,
    }
}

async fn a_constraint_over_what_no_create_sends_is_judged_at_the_declaration_and_one_over_what_it_sends_is_told(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let rank = |initial: Option<i64>| {
        let mut f = json!({"name": "rank", "type": {"kind": "number"}, "authority": "verb"});
        if let Some(i) = initial {
            f["initial"] = json!(i);
        }
        f
    };
    let score = json!({"name": "score", "type": {"kind": "number"}, "authority": "derived",
        "derived": "rank * 2", "depends_on": ["rank"]});
    let positive = json!([{"name": "positive", "holds": "score > 0"}]);
    for (ty, initial, want) in [
        (
            "tally",
            Some(0),
            Case::InitialBreaks {
                constraint: "positive".into(),
                field: "rank".into(),
            },
        ),
        (
            "bare",
            None,
            Case::NoInitialBreaks {
                constraint: "positive".into(),
                field: "rank".into(),
            },
        ),
    ] {
        let decl = typed(ty, json!([rank(initial), score.clone()]), positive.clone());
        let decl = serde_json::from_value(decl).expect("a declaration");
        let said = l.cat.declare(&l.own, decl).await;
        let got = said.as_ref().err().and_then(Error::case);
        ensure!(
            got.map(|s| &s.case) == Some(&want),
            "`positive` over a `score` derived from a `rank` with initial {initial:?} must be \
             refused as {want:?}: {said:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "the refused types").await?;
    l.declare_json(typed("tally", json!([rank(Some(1)), score]), positive))
        .await?;
    // What an author sends is its own: the declaration is taken, and told.
    let size = json!([{"name": "size", "type": {"kind": "number"}, "initial": 0},
        {"name": "memo", "type": {"kind": "text"}}]);
    l.declare_json(typed(
        "box",
        size,
        json!([{"name": "positive", "holds": "size > 0"}]),
    ))
    .await?;
    let told = l.describe_type("box").await?;
    let required: Vec<&str> = (told.fields.iter())
        .filter(|f| f.required_at_create)
        .map(|f| f.name.as_str())
        .collect();
    ensure!(
        required == ["size"],
        "`size`, whose `initial` `positive` refuses, is the one field a create must send: {required:?}"
    );
    // A key's field is filled from the key: not asked for, whatever the
    // constraint on it, and a create that sends nothing else is taken.
    let item = json!({"declare": "type", "name": "item", "key": ["code"], "fields": [
        {"name": "code", "type": {"kind": "number"}}, {"name": "note", "type": {"kind": "text"}},
        {"name": "size", "type": {"kind": "number"}}],
        "constraints": [{"name": "coded", "holds": "code > 0"},
                        {"name": "sized", "holds": "code > 0 or size > 0"}]});
    l.declare_json(item).await?;
    let told = l.describe_type("item").await?;
    let asked: Vec<&str> = (told.fields.iter())
        .filter(|f| f.required_at_create)
        .map(|f| f.name.as_str())
        .collect();
    let tools = lakelands_api::tools::tools(&l.describe().await?).unwrap_or_default();
    let door = |t: &&lakelands_api::tools::ToolSpec| matches!(&t.door, lakelands_api::tools::Door::Upsert { ty } if ty.as_str() == "item");
    let said = (tools.iter().find(door)).map(|t| t.input_schema.to_string());
    let said = said.unwrap_or_default();
    let made = try_upsert(l.cat.as_ref(), &l.own, "item", "5", json!({"note": "x"})).await;
    ensure!(
        asked.is_empty() && !said.contains("send also") && made.is_ok(),
        "`item`'s key is filled at a create: no field is asked for ({asked:?}), the tool \
         asks for nothing ({said}), and the create of `5` with only `note` is taken ({made:?})"
    );
    // Through a derived field, the fields it is computed from are asked for.
    let sum = json!([{"name": "a", "type": {"kind": "number"}, "initial": 0},
        {"name": "b", "type": {"kind": "number"}, "initial": 0},
        {"name": "total", "type": {"kind": "number"}, "authority": "derived",
         "derived": "a + b", "depends_on": ["a", "b"]}]);
    l.declare_json(typed(
        "sum",
        sum,
        json!([{"name": "positive", "holds": "total > 0"}]),
    ))
    .await?;
    let told = l.describe_type("sum").await?;
    let required: Vec<&str> = (told.fields.iter())
        .filter(|f| f.required_at_create)
        .map(|f| f.name.as_str())
        .collect();
    ensure!(
        required == ["a", "b"],
        "`total > 0` over `total = a + b` asks a create for `a` and `b`, not `total`: {required:?}"
    );
    let tools = lakelands_api::tools::tools(&l.describe().await?).unwrap_or_default();
    let door = |t: &&lakelands_api::tools::ToolSpec| matches!(&t.door, lakelands_api::tools::Door::Upsert { ty } if ty.as_str() == "box");
    let upsert = tools.iter().find(door);
    let said = upsert
        .map(|t| t.input_schema.to_string())
        .unwrap_or_default();
    ensure!(
        said.contains("To create the record, send also: size"),
        "the generated `upsert` tool of `box` must say a create sends `size`: {said}"
    );
    let made = try_upsert(l.cat.as_ref(), &l.own, "box", "b1", json!({"memo": "m"})).await;
    let e = made.as_ref().err();
    ensure!(
        e.and_then(broken) == Some("positive"),
        "a create that sends no `size` must be refused `constraint_broken` naming `positive`: {made:?}"
    );
    crate::conformance::fixture::upsert(l.cat.as_ref(), &l.own, "box", "b2", json!({"size": 3}))
        .await?;
    Ok(())
}

async fn required_at_create_is_told_only_to_a_reader_shown_the_constraint(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let fields = json!([{"name": "size", "type": {"kind": "number"}, "initial": 0},
        {"name": "other", "type": {"kind": "number"}}]);
    let both = json!([{"name": "both", "holds": "size > 0 and other > 0"}]);
    l.declare_json(typed("box", fields, both)).await?;
    let names = |t: &lakelands_api::describe::TypeDescription| -> Vec<String> {
        let told = t.fields.iter().filter(|f| f.required_at_create);
        told.map(|f| f.name.to_string()).collect()
    };
    let owner = l.describe_type("box").await?;
    ensure!(
        names(&owner) == ["size", "other"],
        "a reader shown both fields is told both: {:?}",
        names(&owner)
    );
    let blind = field_access(store.as_ref(), "blind", "box", &[("other", Access::None)]);
    let seen = crate::conformance::fixture::describe_type(l.cat.as_ref(), &blind, "box").await?;
    ensure!(
        seen.fields.iter().any(|f| f.name == "size") && names(&seen).is_empty(),
        "a reader who reads `size` and not `other` is told no field required: {:?}",
        names(&seen)
    );
    Ok(())
}

async fn a_constraint_a_setting_or_the_clock_decides_is_judged_at_the_write(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    opened(&l).await?;
    // `gate` breaks at the record a create leaves read without the setting
    // and the clock, and holds at the write: `open` is set, and `since`, an
    // `initial` only verbs write, is long past.
    let since = json!({"name": "since", "type": {"kind": "date_time"}, "authority": "verb",
        "initial": "2001-01-01T00:00:00Z"});
    for (ty, derived, at) in [
        ("gated", "setting('open') = true", None),
        ("late", "now() > since", Some("write")),
    ] {
        let fields = json!([{"name": "memo", "type": {"kind": "text"}}, since.clone(),
            {"name": "gate", "type": {"kind": "boolean"}, "authority": "derived",
             "derived": derived, "computed_at": at}]);
        let decl = typed(ty, fields, json!([{"name": "c", "holds": "gate"}]));
        let decl = serde_json::from_value(decl).expect("a declaration");
        let said = l.cat.declare(&l.own, decl).await;
        ensure!(
            said.is_ok(),
            "`c: gate`, `gate` derived as `{derived}`, must be taken: {said:?}"
        );
        let told = l.describe_type(ty).await?;
        let asked: Vec<&str> = (told.fields.iter())
            .filter(|f| f.required_at_create)
            .map(|f| f.name.as_str())
            .collect();
        let made = try_upsert(l.cat.as_ref(), &l.own, ty, "k1", json!({"memo": "x"})).await;
        ensure!(
            asked.is_empty() && made.is_ok(),
            "`{ty}` asks a create for no field ({asked:?}) and the create is taken: {made:?}"
        );
    }
    // Beside a part the record decides: one that breaks for every create is
    // refused, naming what it reads, and one that holds is taken.
    let rank =
        json!({"name": "rank", "type": {"kind": "number"}, "authority": "verb", "initial": 0});
    let score = json!({"name": "score", "type": {"kind": "number"}, "authority": "verb"});
    let gate = |derived: &str| {
        json!({"name": "gate", "type": {"kind": "boolean"}, "authority": "derived",
            "derived": derived})
    };
    for (ty, holds, derived, refused) in [
        (
            "ranked",
            "rank > 0 and gate",
            "setting('open') = true",
            true,
        ),
        (
            "scored",
            "rank > 0 and gate",
            "setting('open') = true and score >= 0",
            true,
        ),
        (
            "level",
            "rank >= 0 and gate",
            "setting('open') = true",
            false,
        ),
    ] {
        let fields = json!([rank.clone(), score.clone(), gate(derived)]);
        let decl = typed(ty, fields, json!([{"name": "c", "holds": holds}]));
        let decl = serde_json::from_value(decl).expect("a declaration");
        let said = l.cat.declare(&l.own, decl).await;
        let want = Case::InitialBreaks {
            constraint: "c".into(),
            field: "rank".into(),
        };
        let got = said.as_ref().err().and_then(Error::case);
        match refused {
            true => ensure!(
                got.map(|s| &s.case) == Some(&want),
                "`{holds}`, `gate` derived as `{derived}`, must be refused as {want:?}: {said:?}"
            ),
            false => ensure!(said.is_ok(), "`{holds}` must be taken: {said:?}"),
        }
    }
    let made = try_upsert(l.cat.as_ref(), &l.own, "level", "v1", json!({"no": "x"})).await;
    ensure!(made.is_ok(), "a create of `level` is taken: {made:?}");
    Ok(())
}
