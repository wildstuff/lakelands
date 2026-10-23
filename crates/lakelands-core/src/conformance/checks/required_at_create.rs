//! A create that must send a value, and what `describe` tells of it
//! (`docs/decisions/059-a-type-declares-its-constraints.md`, extended): a
//! constraint is judged over the record a create leaves, so a field whose
//! `initial` or null it refuses is one the create must send, and a create
//! no sender can make hold is told closed.

use crate::conformance::prelude::*;

check!(
    REQUIRED_AT_CREATE = a_constraint_over_what_no_create_sends_is_judged_at_the_declaration_and_one_over_what_it_sends_is_told,
    "059 and 050, extended (lakelands#1169): a constraint that reads only fields no create can send, those only verbs write and those computed from them, is judged over the record a create leaves: `positive: score > 0` over a `score` derived from a `rank` only verbs write, whose `initial` is 0, is refused `initial_breaks` naming `rank`, and over a `rank` with no `initial` `no_initial_breaks`, appending nothing, and taken once `rank` holds 1; one that reads an authored field is taken as it was: `size > 0` over a `size` with `initial` 0 is declared, `describe_type` tells `size` `required_at_create` and no field beside it, and over a `total` derived from `a + b` tells `a` and `b` and not `total`; a key's field, which a create fills from its key, is asked for by no constraint on it, `code > 0` nor `code > 0 or size > 0`, and a create of key `5` sending only `note` is taken; the generated `upsert` tool says a create sends it, and a create without it is refused `constraint_broken` naming the constraint where one that sends it is taken"
);

check!(
    REQUIRED_TWIN = required_at_create_is_told_only_to_a_reader_shown_the_constraint,
    "059 and 024, extended (lakelands#1169): `required_at_create` is told to a reader shown both the field and the constraint: a reader who reads `size` but not `other`, which `both: size > 0 and other > 0` reads too, is told neither field, and one shown both is told both"
);

check!(
    MAY_CREATE = may_create_says_no_more_than_the_door,
    "059 and 006, extended (lakelands#1188): `describe_type` tells `may.create` beside `may.upsert`: false, with one `create_why` that names no field and no constraint, to an author kept from the `seal` that `seal > 0` needs, whether it may not read it or may not write it, whose `upsert` of a new record is refused, the first as `not_granted` and the second as `constraint_broken`, and whose `upsert` of a record that exists is taken; true to one who sends `seal`, to a reader shown everything, and to one kept from a field no constraint reads; true where the declaration cannot rule a create out, as for four conditions joined by `and` each a create may make hold, whose create is taken; false where a required field with no `initial`, or the key's field, is one only a source writes, whose create is refused, and where a constraint the create's record holds reads a field the author may not read, whose create is refused not granted; true where a condition joined by `and` a setting decides, the setting holding, whose create is taken; the same `create_why` and the same refusal for another hidden field of another constraint; a constraint that also reads a field only verbs write, whose `initial` it refuses, closes it to the owner too, and a constraint the author's other fields can satisfy leaves it true"
);

check!(
    REQUIRED_OUTSIDE = a_constraint_a_setting_or_the_clock_decides_is_judged_at_the_write,
    "059, extended (lakelands#1169): a constraint over a field derived from a setting, or from the clock and a field only verbs write whose `initial` is long past, reads no field a create sends and is taken at the declaration, its value the write's to judge; a create where it holds is taken, is told open, and no field is told `required_at_create`; joined by `and` to `rank > 0` over a `rank` only verbs write with `initial` 0, it is refused `initial_breaks` naming `rank`, also where the setting's part reads a field only verbs write with no `initial`, and to `rank >= 0` it is taken, and so is a create"
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
        asked.is_empty() && told.may.create && !said.contains("send also") && made.is_ok(),
        "`item`'s key is filled at a create: no field is asked for ({asked:?}), a create is \
         told open ({:?}), the tool asks for nothing ({said}), and the create of `5` with \
         only `note` is taken ({made:?})",
        told.may
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

async fn may_create_says_no_more_than_the_door(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let s = store.as_ref();
    let pair = |ty: &str, hidden: &str, holds: &str| {
        let fields = json!([{"name": hidden, "type": {"kind": "number"}},
            {"name": "memo", "type": {"kind": "text"}}, {"name": "note", "type": {"kind": "text"}}]);
        typed(ty, fields, json!([{"name": "gate", "holds": holds}]))
    };
    l.declare_json(pair("crate", "seal", "seal > 0")).await?;
    l.declare_json(pair("sack", "tag", "tag != 0")).await?;
    l.declare_json(typed(
        "free",
        json!([{"name": "memo", "type": {"kind": "text"}},
               {"name": "size", "type": {"kind": "number"}, "initial": 0}]),
        json!([{"name": "positive", "holds": "size > 0 or memo = 'free'"}]),
    ))
    .await?;
    for ty in ["crate", "sack"] {
        let field = if ty == "crate" { "seal" } else { "tag" };
        crate::conformance::fixture::upsert(
            l.cat.as_ref(),
            &l.own,
            ty,
            "k1",
            json!({field: 1, "memo": "m"}),
        )
        .await?;
    }
    let who = |id: &str, ty: &str, field: &str, access: Access| {
        field_access(s, id, ty, &[(field, access), ("memo", Access::Write)])
    };
    let mut whys = Vec::new();
    let mut refusals = Vec::new();
    for (ty, field, access, code) in [
        ("crate", "seal", Access::None, "not_granted"),
        ("crate", "seal", Access::Read, "constraint_broken"),
        ("sack", "tag", Access::None, "not_granted"),
    ] {
        let author = who("author", ty, field, access);
        let told = crate::conformance::fixture::describe_type(l.cat.as_ref(), &author, ty).await?;
        ensure!(
            told.may.upsert && !told.may.create && told.may.create_why.is_some(),
            "an author kept from `{field}` ({access:?}) may upsert, not create, and is told why: {:?}",
            told.may
        );
        let why = told.may.create_why.clone().unwrap_or_default();
        ensure!(
            !why.contains(field) && !why.contains("gate"),
            "the reason must name no field or constraint: {why}"
        );
        let new = try_upsert(l.cat.as_ref(), &author, ty, "k2", json!({"memo": "n"})).await;
        let e = new.as_ref().err().map(|e| (reason(e).cloned(), broken(e)));
        let got = match &e {
            Some((Some(Refusal::NotGranted), _)) => "not_granted",
            Some((_, Some(_))) => "constraint_broken",
            _ => "taken or another refusal",
        };
        ensure!(
            got == code,
            "a create by an author kept from `{field}` ({access:?}) must be refused {code}: {new:?}"
        );
        let change = try_upsert(
            l.cat.as_ref(),
            &author,
            ty,
            "k1",
            json!({"memo": "changed"}),
        )
        .await;
        ensure!(
            change.is_ok(),
            "a change of a record that exists must be taken: {change:?}"
        );
        whys.push(why);
        refusals.push((ty, new));
    }
    ensure!(
        whys.windows(2).all(|w| w[0] == w[1]),
        "the reason is the same for every field and constraint: {whys:?}"
    );
    let (first, last) = (&refusals[0], &refusals[2]);
    refused_alike(
        "a create by an author kept from a field",
        &first.1,
        last.1.clone(),
        ("crate", "sack"),
    )?;
    // Who sends the field, who reads all, and who is kept from a field no
    // constraint reads, may create.
    for (author, ty) in [
        (who("sender", "crate", "seal", Access::Write), "crate"),
        (l.own.clone(), "crate"),
    ] {
        let told = crate::conformance::fixture::describe_type(l.cat.as_ref(), &author, ty).await?;
        ensure!(
            told.may.create && told.may.create_why.is_none(),
            "{} must be told it may create: {:?}",
            author.id,
            told.may
        );
    }
    let fields = [
        ("memo", Access::Write),
        ("seal", Access::Write),
        ("note", Access::None),
    ];
    let kept = field_access(s, "kept", "crate", &fields);
    let told = crate::conformance::fixture::describe_type(l.cat.as_ref(), &kept, "crate").await?;
    ensure!(
        told.may.create,
        "an author kept from `note`, which no constraint reads, may create: {:?}",
        told.may
    );
    // A constraint that also reads a field only verbs write, whose `initial`
    // it refuses, closes the create to every sender, the owner too.
    l.declare_json(typed(
        "vault",
        json!([{"name": "rank", "type": {"kind": "number"}, "authority": "verb", "initial": 0},
               {"name": "size", "type": {"kind": "number"}}]),
        json!([{"name": "gate", "holds": "rank > 0 and size > 0"}]),
    ))
    .await?;
    let told = l.describe_type("vault").await?;
    ensure!(
        told.may.upsert && !told.may.create && told.may.create_why.is_some(),
        "no create can make `rank > 0` hold, so even the owner is told it may not create: {:?}",
        told.may
    );
    let made = try_upsert(l.cat.as_ref(), &l.own, "vault", "v1", json!({"size": 3})).await;
    ensure!(
        made.as_ref().err().and_then(broken) == Some("gate"),
        "that create is refused `constraint_broken`: {made:?}"
    );
    // A constraint the author's other fields can satisfy leaves it true.
    let author = field_access(s, "memoer", "free", &[("memo", Access::Write)]);
    let told = crate::conformance::fixture::describe_type(l.cat.as_ref(), &author, "free").await?;
    ensure!(
        told.may.create,
        "`size > 0 or memo = 'free'` is satisfied by `memo`, which the author sends: {:?}",
        told.may
    );
    let made = try_upsert(
        l.cat.as_ref(),
        &author,
        "free",
        "f1",
        json!({"memo": "free"}),
    )
    .await;
    ensure!(made.is_ok(), "that create is taken: {made:?}");
    // What the declaration cannot rule out leaves it open: four conditions
    // each a create may make hold.
    let four = json!([{"name": "a", "type": {"kind": "number"}}, {"name": "b", "type": {"kind": "number"}},
        {"name": "c", "type": {"kind": "number"}}, {"name": "d", "type": {"kind": "number"}}]);
    let holds = "a > 0 and b > 0 and c > 0 and d > 0";
    l.declare_json(typed(
        "four",
        four,
        json!([{"name": "all", "holds": holds}]),
    ))
    .await?;
    let told = l.describe_type("four").await?;
    let made = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "four",
        "q1",
        json!({"a": 1, "b": 1, "c": 1, "d": 1}),
    )
    .await;
    ensure!(
        told.may.create && made.is_ok(),
        "`{holds}` is told open and a create that sends all four is taken: {:?}, {made:?}",
        told.may
    );
    // A key a source writes: a create fills it from its key, which no
    // author may.
    let ticket = json!({"declare": "type", "name": "ticket", "key": ["ext_id"], "fields": [
        {"name": "ext_id", "type": {"kind": "text"}, "authority": "source"},
        {"name": "note", "type": {"kind": "text"}}]});
    l.declare_json(ticket).await?;
    let told = l.describe_type("ticket").await?;
    let made = try_upsert(l.cat.as_ref(), &l.own, "ticket", "t1", json!({"note": "n"})).await;
    ensure!(
        told.may.upsert && !told.may.create && made.is_err(),
        "a create of `ticket` fills `ext_id`, which only a source writes: told {:?}, the create {made:?}",
        told.may
    );
    // A constraint the create's record holds, over a field the author may
    // not read: the write refuses it all the same, naming nothing.
    l.declare_json(typed(
        "mark",
        json!([{"name": "flag", "type": {"kind": "number"}, "initial": 1},
               {"name": "memo", "type": {"kind": "text"}}]),
        json!([{"name": "gate", "holds": "flag > 0"}]),
    ))
    .await?;
    let author = field_access(
        s,
        "marker",
        "mark",
        &[("memo", Access::Write), ("flag", Access::None)],
    );
    let told = crate::conformance::fixture::describe_type(l.cat.as_ref(), &author, "mark").await?;
    let made = try_upsert(l.cat.as_ref(), &author, "mark", "m1", json!({"memo": "m"})).await;
    let refused = made.as_ref().err().and_then(|e| reason(e).cloned());
    ensure!(
        !told.may.create && refused == Some(Refusal::NotGranted),
        "a create judged by a constraint over `flag`, which the author may not read, is told \
         closed and refused not granted: {:?}, {made:?}",
        told.may
    );
    // A required field with no `initial` that only a source writes closes
    // the create to an author, as the write refuses it.
    l.declare_json(typed(
        "ledger",
        json!([{"name": "entry", "type": {"kind": "text"}, "authority": "source", "required": true},
               {"name": "memo", "type": {"kind": "text"}}]),
        json!([]),
    ))
    .await?;
    let told = l.describe_type("ledger").await?;
    let made = try_upsert(l.cat.as_ref(), &l.own, "ledger", "e1", json!({"memo": "m"})).await;
    ensure!(
        told.may.upsert && !told.may.create && made.is_err(),
        "a create of `ledger` needs `entry`, which no author sends: told {:?}, the create {made:?}",
        told.may
    );
    // A condition a setting decides is no proof: the write judges it.
    opened(&l).await?;
    l.declare_json(typed(
        "gated",
        json!([{"name": "n", "type": {"kind": "number"}},
               {"name": "open", "type": {"kind": "boolean"}, "authority": "derived",
                "derived": "setting('open') = true"}]),
        json!([{"name": "gate", "holds": "n > 0 and open"}]),
    ))
    .await?;
    let told = l.describe_type("gated").await?;
    let made = try_upsert(l.cat.as_ref(), &l.own, "gated", "g1", json!({"n": 1})).await;
    ensure!(
        told.may.create && made.is_ok(),
        "`n > 0 and open`, `open` derived from a setting that holds, is told open and a create \
         sending `n` is taken: {:?}, {made:?}",
        told.may
    );
    head_unmoved(s, s.head().await.must("reading the head")?, "the check").await
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
            asked.is_empty() && told.may.create && made.is_ok(),
            "`{ty}` asks a create for no field ({asked:?}), is told open ({:?}) and the create \
             is taken: {made:?}",
            told.may
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
