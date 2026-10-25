//! What a principal may not read, on every read and in the tools
//! (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`).

use lakelands_api::error::Naming;
use lakelands_api::tools::Door;
use lakelands_api::write::Outcome;

use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    alike_each, derived, get_at, ingest, nobody, order, read, source_record, step,
};
use crate::conformance::prelude::*;

check!(
    MASKED_AND_ABSENT = a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent,
    "006 and 014: a field the principal may not read is masked (named, no value) or absent from get, find, describe and the tools' schemas"
);

check!(
    BUILT_FROM_HIDDEN = a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field,
    "024 and 006: a derived field, one derived from it in turn, and a vector embedded from a field are read as the field they are built from: absent where it is absent and masked where it is masked, on get, find, a pattern's rows (where a projected hidden one is refused as for an undeclared field, lakelands#752) and describe, which shows a definition only where every field it names is read; a filter, an order or an aggregate over them is refused naming them, as not granted where they are masked and as for an undeclared field where they are hidden",
    embedding
);

check!(
    VIA_HIDDEN = a_relation_carried_by_a_hidden_field_is_walked_by_no_one_held_from_it,
    "024, extended (lakelands#727): a relation carried by a reference field is not shown to a reader who may not know of the field: `describe_type` does not list it, a walk out over it is answered as over a relation the type does not declare, and so is a graph from its type, while a walk in and a graph from its target reach only another type's relation of the same name; to a reader who sees the field masked it is shown, and a walk over it, out, in or as a graph, is refused as not granted, naming the field, and so is a walk of two hops, a graph or `components` that meets it past its first hop; an author held from the field who sends a relation of its name is refused, word for word, as the owner is for a type that declares a relation twice, and the type stays as it is"
);

/// What a pattern answered, short: the keys it reached, or the refusal.
fn walked(answer: lakelands_api::Result<lakelands_api::Reply<QueryResult>>) -> String {
    if let Some((f, Refusal::NotGranted)) = refused_on(&answer) {
        return format!("not granted {f}");
    }
    let mut keys: Vec<String> = match answer.map(|r| r.data) {
        Ok(QueryResult::Rows { rows, .. }) => (rows.iter().flatten())
            .filter(|(k, _)| k.ends_with(".key") || *k == "key")
            .map(|(_, v)| v.as_str().unwrap_or_default().to_owned())
            .collect(),
        Ok(QueryResult::Graph { nodes, .. }) => nodes.iter().map(|n| n.key.to_string()).collect(),
        Ok(other) => return format!("{other:?}"),
        Err(e) if matches!(e.case(), Some(Said { case: Case::NoRelationToWalk { relation, .. }, .. }) if relation == "r") =>
        {
            return "no relation".into();
        }
        Err(e) => return e.to_string(),
    };
    keys.sort();
    keys.dedup();
    keys.join(" ")
}

async fn a_relation_carried_by_a_hidden_field_is_walked_by_no_one_held_from_it(
    store: Store,
) -> Held {
    use lakelands_api::query::Walk;
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    // `wk` and `side` each carry a relation `r` to `other` by a field.
    l.declare(type_decl("other", vec![field("n", StorageKind::Text)]))
        .await?;
    let to_other = json!({"kind": "reference", "target": "other"});
    let rows = || Projection::Rows { fields: vec![] };
    let walk = |ty: &str, key: &str, w| pattern(ty, Some(key), vec![step("r", w)], rows());
    let both = || vec![step("r", Walk::Both)];
    let graph = |ty: &str, key: &str| pattern(ty, Some(key), both(), Projection::Graph);
    // Before `wk` declares `r`: the answer for a relation not declared.
    l.declare_json(json!({"declare": "type", "name": "wk", "fields": [
        {"name": "no", "type": {"kind": "text"}}, {"name": "tgt", "type": to_other}]}))
        .await?;
    l.upsert("other", "o1", json!({"n": "o"})).await?;
    l.upsert("wk", "w1", json!({"no": "1", "tgt": "o1"}))
        .await?;
    let own = &l.own;
    let absent = |p| async move { cat.query(own, p).await.must_fail("a walk over no `r`") };
    let (absent_out, absent_graph) = (
        absent(walk("wk", "w1", Walk::Out)).await?,
        absent(graph("wk", "w1")).await?,
    );
    for (name, via) in [("wk", "tgt"), ("side", "pt")] {
        l.declare_json(json!({"declare": "type", "name": name, "fields": [
            {"name": "no", "type": {"kind": "text"}}, {"name": via, "type": to_other}],
            "relations": [{"name": "r", "target": "other", "via": via}]}))
            .await?;
    }
    l.upsert("side", "s1", json!({"no": "1", "pt": "o1"}))
        .await?;
    // From `s1`, `wk`'s `r` comes up only at the second hop, around `o1`.
    let far = |project| {
        let depth = lakelands_api::query::Depth { min: 1, max: 2 };
        let steps = vec![lakelands_api::query::Step {
            depth,
            ..step("r", Walk::Both)
        }];
        pattern("side", Some("s1"), steps, project)
    };
    let components = || pattern("side", Some("s1"), both(), Projection::Components);
    let (mut wrong, ng) = (Vec::new(), "not granted tgt");
    for (access, out, inn, from_wk, from_other) in [
        (Access::None, "no relation", "o1 s1", "no relation", "o1 s1"),
        (Access::Masked, ng, ng, ng, ng),
    ] {
        let who = field_access(store.as_ref(), "held", "wk", &[("tgt", access)]);
        let (a_out, a_graph) = (Some(&absent_out), Some(&absent_graph));
        for (what, p, want, absent) in [
            (
                "a walk out of `w1`",
                walk("wk", "w1", Walk::Out),
                out,
                a_out,
            ),
            (
                "a walk in to `o1`",
                walk("other", "o1", Walk::In),
                inn,
                None,
            ),
            ("a graph from `w1`", graph("wk", "w1"), from_wk, a_graph),
            ("a graph from `o1`", graph("other", "o1"), from_other, None),
        ] {
            let answer = cat.query(&who, p).await;
            // Held from `tgt`, the very error of a relation not declared.
            if let (Access::None, Some(absent), Err(e)) = (access, absent, &answer)
                && let Err(f) = alike_renamed(what, e, absent, ("", ""))
            {
                wrong.push(f.message);
            }
            let said = walked(answer);
            if said != want {
                wrong.push(format!("{what}, `tgt` {access:?}: `{said}`, not `{want}`"));
            }
        }
        let want = if access == Access::None { "o1 s1" } else { ng };
        // The second hop comes back to `side`, and rows bind one type under
        // a name (lakelands#680): theirs names `other`, and still walks on.
        let mut far_rows = far(rows());
        far_rows.steps[0].ty = Some("other".into());
        for (what, p) in [
            ("a graph from `s1`, two hops", far(Projection::Graph)),
            ("rows from `s1`, two hops", far_rows),
            ("the components from `s1`", components()),
        ] {
            let said = walked(cat.query(&who, p).await);
            if said != want {
                wrong.push(format!("{what}, `tgt` {access:?}: `{said}`, not `{want}`"));
            }
        }
        let told = describe_type(cat, &who, "wk").await?;
        let shown = told.relations.iter().any(|r| r.name.as_str() == "r");
        if shown != (access == Access::Masked) {
            wrong.push(format!("`describe_type`, `tgt` {access:?}: `r` {shown}"));
        }
    }
    // An author held from `tgt` sends `r` again: it is refused as the
    // owner is for a type that declares `r` twice, and `wk` stays as it is.
    let mut author = field_access(store.as_ref(), "author", "wk", &[("tgt", Access::None)]);
    (author.grants.schema, author.grants.restricted) = (true, true);
    let kind = |f: &str| match f {
        "tgt" => to_other.clone(),
        _ => json!({"kind": "text"}),
    };
    let wk = |fields: &[&str], rs: serde_json::Value| {
        let fields: Vec<serde_json::Value> =
            (fields.iter().map(|f| json!({"name": f, "type": kind(f)}))).collect();
        let d = json!({"declare": "type", "name": "wk", "fields": fields, "relations": rs});
        serde_json::from_value(d).expect("a declaration reads")
    };
    let via = json!({"name": "r", "target": "other", "via": "tgt"});
    let linked = json!({"name": "r", "target": "other"});
    let owners = cat.declare(&l.own, wk(&["no", "tgt"], json!([via, linked])));
    let owners = owners.await;
    let authors = cat.declare(&author, wk(&["no"], json!([linked]))).await;
    let relations = l.describe_type("wk").await?.relations.len();
    let twice = |e: &lakelands_api::Error| matches!(e.case(), Some(Said { case: Case::DeclaredTwice { what: Naming::Relation, name }, .. }) if name == "r");
    match (&authors, &owners) {
        (Err(a), Err(o)) if twice(o) && relations == 1 => {
            if let Err(f) = alike_renamed(
                "`r` sent again by an author held from `tgt`",
                a,
                o,
                ("", ""),
            ) {
                wrong.push(f.message);
            }
        }
        _ => wrong.push(format!(
            "`r` sent again: {authors:?}, the owner's: {owners:?}; {relations}"
        )),
    }
    ensure!(
        wrong.is_empty(),
        "a relation carried by `tgt` answers nothing of it to a reader held from it: {wrong:?}"
    );
    Ok(())
}

async fn a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    l.declare(type_decl(
        "emp",
        vec![
            field("salary", StorageKind::Number),
            field("bonus", StorageKind::Number),
            derived("g", StorageKind::Number, "salary * 2", &["salary"]),
            derived("h", StorageKind::Number, "g + 1", &["g"]),
            derived("k", StorageKind::Number, "bonus + 1", &["bonus"]),
        ],
    ))
    .await?;
    l.declare(type_decl(
        "doc",
        vec![
            field("body", StorageKind::Text),
            embedding("vec", 2, &["body"], EMBED_MODEL),
        ],
    ))
    .await?;
    l.upsert("emp", "e1", json!({"salary": 4200, "bonus": 10}))
        .await?;
    l.upsert("doc", "d1", json!({"body": "the harbour master's salary"}))
        .await?;
    let names = |r: &lakelands_api::Record| {
        let fields: Vec<String> = r.fields.keys().map(ToString::to_string).collect();
        let masked: Vec<String> = r.masked.iter().map(ToString::to_string).collect();
        (fields, masked)
    };
    let owner = names(&l.read("emp", "e1").await?);
    ensure!(
        owner.0 == ["bonus", "g", "h", "k", "salary"],
        "the owner reads every field of `e1`; it read {owner:?}"
    );
    for (access, want) in [
        (Access::None, (vec!["bonus", "k"], Vec::<&str>::new())),
        (
            Access::Masked,
            (vec!["bonus", "k"], vec!["salary", "g", "h"]),
        ),
    ] {
        let who = field_access(store.as_ref(), "clerk", "emp", &[("salary", access)]);
        let got = names(&read(cat.as_ref(), &who, "emp", "e1").await?);
        ensure!(
            got.0 == want.0 && got.1 == want.1,
            "a clerk with `salary` {access:?} reads `g` (from salary) and `h` (from g) as \
             `salary` is read: {want:?}; get answered {got:?}"
        );
        let found = all_found(cat.as_ref(), &who, "emp", Find::default()).await?;
        let found: Vec<_> = found.iter().map(names).collect();
        ensure!(
            found == [got.clone()],
            "find answers what get answers for the clerk ({got:?}); it answered {found:?}"
        );
        // Rows that project them: a hidden one is a field `emp` does not
        // declare (lakelands#752), a masked one is left out of the row.
        let fields = vec!["emp.g".into(), "emp.h".into(), "emp.k".into()];
        let rows = pattern("emp", Some("e1"), Vec::new(), Projection::Rows { fields });
        let rows = cat.query(&who, rows).await;
        if access == Access::None {
            ensure!(
                no_such_field(&rows, "g"),
                "the clerk's pattern rows projecting `g` are refused as for an undeclared \
                 field; they answered {rows:?}"
            );
        } else {
            let rows = rows.must("the clerk's pattern rows over `emp`")?.data;
            let QueryResult::Rows { rows, .. } = rows else {
                return Err(Failure::new("a pattern's rows answered something else"));
            };
            let row: Vec<&String> = rows.iter().flat_map(|r| r.keys()).collect();
            ensure!(
                row == ["emp.k"],
                "the clerk's pattern rows hold `k` only, not `g` or `h`; they held {row:?}"
            );
        }
        let d = describe_type(cat.as_ref(), &who, "emp").await?;
        let defined: Vec<String> = (d.fields.iter())
            .filter(|f| f.derived.is_some() || !f.depends_on.is_empty())
            .map(|f| f.name.to_string())
            .collect();
        let text = serde_json::to_string(&d).unwrap_or_default();
        ensure!(
            defined == ["k"] && (access != Access::None || !text.contains("salary")),
            "describe shows a clerk with `salary` {access:?} the definition of `k` only, and \
             names no field the clerk may not see; it defined {defined:?} in {text}"
        );
        let filter: fn(&str) -> Find = |n| find_where(&format!("{n} > 0"));
        let ordered: fn(&str) -> Find = |n| Find {
            order_by: vec![order(n, OrderBy::Value, Direction::Asc)],
            ..Find::default()
        };
        let sum: fn(&str) -> Find = |n| aggregate(vec![agg("s", AggFunc::Sum, Some(n))]);
        let probes = [
            ("a filter on `g`", "g", filter),
            ("an order by `h`", "h", ordered),
            ("a sum of `g`", "g", sum),
        ];
        for (what, name, find) in probes {
            let r = cat.find(&who, &"emp".into(), find(name)).await;
            let answered = match access {
                Access::None => no_such_field(&r, name),
                _ => matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == name),
            };
            // Hidden, as `nosuch`, which `emp` does not declare, under its name.
            if access == Access::None {
                let absent = cat.find(&who, &"emp".into(), find("nosuch")).await;
                refused_alike(what, &r, absent, ("nosuch", name))?;
            }
            ensure!(
                answered,
                "{what} by a clerk with `salary` {access:?} is refused naming `{name}`, as not \
                 granted where it is masked and as an undeclared field where it is hidden; it \
                 answered {r:?}"
            );
        }
    }
    let reader = field_access(store.as_ref(), "reader", "doc", &[("body", Access::None)]);
    let got = names(&read(cat.as_ref(), &reader, "doc", "d1").await?);
    ensure!(
        got.0.is_empty() && got.1.is_empty(),
        "a reader without `body` reads no `vec`, which is embedded from it; get answered {got:?}"
    );
    Ok(())
}

check!(
    NONE_HIDES = a_field_the_principal_may_not_know_of_is_answered_as_one_never_declared,
    "006 and 024: a field whose grant is none by name, sensitive or not, does not exist for that principal: it is absent from get, find, describe (a relation's link field too) and the tools, a search that names no fields never searches it, and a filter, an order, a pattern's `where`, a search's `fields`, an upsert and an ingest that name it are answered as for a field the type does not declare; a sensitive field whose none is inherited is listed as masked, and a filter on it is refused as not granted, to one who is shown the type: one whose grants give it nothing of the type is answered by describe, get, find and upsert as on a type that is not declared, a sensitive field or none"
);

async fn a_field_the_principal_may_not_know_of_is_answered_as_one_never_declared(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut staff_decl = type_decl(
        "staff",
        vec![
            searchable("name"),
            searchable("plain"),
            FieldDecl {
                sensitive: true,
                ..searchable("secret")
            },
            sourced("feed", StorageKind::Text),
            // Sensitive, but hidden with `plain`, which it is built from.
            FieldDecl {
                sensitive: true,
                ..derived("echo", StorageKind::Text, "plain", &["plain"])
            },
        ],
    );
    staff_decl.relations = vec![lakelands_api::ontology::RelationDecl {
        cardinality: lakelands_api::ontology::Cardinality::ManyToMany,
        fields: vec![
            field("memo", StorageKind::Text),
            field("since", StorageKind::Text),
        ],
        ..lakelands_api::ontology::RelationDecl::to("buddy", "staff")
    }];
    l.declare(staff_decl).await?;
    l.upsert(
        "staff",
        "s1",
        json!({"name": "ann", "plain": "pp", "secret": "ss"}),
    )
    .await?;
    let hidden = ["plain", "secret", "feed"].map(|f| (f, Access::None));
    let mut by_name = field_access(store.as_ref(), "x", "staff", &hidden);
    by_name.grants.ingest = true;
    by_name.grants.types.insert(
        "sys.link.staff.buddy".into(),
        lakelands_api::principal::TypeGrants {
            default: None,
            fields: [("memo".into(), Access::None)].into(),
            ..Default::default()
        },
    );
    let staff = TypeName::from("staff");
    let got = read(cat.as_ref(), &by_name, "staff", "s1").await?;
    ensure!(
        got.fields.keys().map(FieldName::as_str).eq(["name"]) && got.masked.is_empty(),
        "fields a principal is denied by name, `secret` sensitive among them, are neither \
         answered nor listed as masked; get answered {:?}, masked {:?}",
        got.fields,
        got.masked
    );
    let d = describe_type(cat.as_ref(), &by_name, "staff").await?;
    let lake = cat.describe(&by_name, ReadAt::default());
    let lake = lake.await.must("describing the lake")?.data;
    let generated = tools(&lake).map_err(|p| Failure::new(format!("no tools: {p:?}")))?;
    let schemas = serde_json::to_string(&generated).unwrap_or_default();
    let link_fields: Vec<&str> = (d.relations.iter())
        .flat_map(|r| r.fields.iter().map(|f| f.name.as_str()))
        .collect();
    ensure!(
        link_fields == ["since"],
        "a link field the principal may not know of is not described; `buddy` listed \
         {link_fields:?}"
    );
    ensure!(
        d.fields.iter().map(|f| f.name.as_str()).eq(["name"])
            && !["plain", "secret", "feed"]
                .iter()
                .any(|f| schemas.contains(&format!("\"{f}\""))),
        "describe and the tools name no field the principal may not know of; describe listed \
         {:?}",
        d.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
    );
    let mut undeclared = Vec::new();
    for f in ["nosuch", "plain", "secret"] {
        let in_fields = lexical("staff", "pp", Some(vec![f.into()]), false);
        let mut start = pattern("staff", None, Vec::new(), Projection::Graph);
        start.start.filter = Some(format!("{f} = 'x'").as_str().into());
        let ordered = Find {
            order_by: vec![order(f, OrderBy::Value, Direction::Asc)],
            ..Find::default()
        };
        let answers = [
            (
                "a filter",
                cat.find(&by_name, &staff, find_where(&format!("{f} = 'x'")))
                    .await
                    .map(drop),
            ),
            (
                "an order",
                cat.find(&by_name, &staff, ordered).await.map(drop),
            ),
            (
                "a pattern's `where`",
                cat.query(&by_name, start).await.map(drop),
            ),
            (
                "a search's `fields`",
                cat.search(&by_name, in_fields).await.map(drop),
            ),
            (
                "an upsert",
                try_upsert(cat.as_ref(), &by_name, "staff", "s1", json!({f: "x"}))
                    .await
                    .map(drop),
            ),
        ];
        for (i, (what, r)) in answers.into_iter().enumerate() {
            ensure!(
                no_such_field(&r, f),
                "{what} naming `{f}` is answered as for a field `staff` does not declare; it \
                 answered {r:?}"
            );
            // Word for word, and member for member, as `nosuch` is.
            let Err(e) = r else { continue };
            match undeclared.get(i) {
                Some(absent) => alike_renamed(what, &e, absent, ("nosuch", f))?,
                None => undeclared.push(e),
            }
        }
    }
    let feed = vec![source_record("staff", "s2", json!({"feed": "x"}))];
    let fed = ingest(cat.as_ref(), &by_name, "crm", feed).await;
    ensure!(
        no_such_field(&fed, "feed"),
        "an ingest of `feed` is answered as for a field `staff` does not declare; it answered \
         {fed:?}"
    );
    let nosuch = vec![source_record("staff", "s2", json!({"nosuch": "x"}))];
    let nosuch = ingest(cat.as_ref(), &by_name, "crm", nosuch).await;
    refused_alike("an ingest of `feed`", &fed, nosuch, ("nosuch", "feed"))?;
    let unnamed = |who| cat.search(who, lexical("staff", "ss pp ann", None, false));
    let mine = unnamed(own).await.must("the owner's search")?.data;
    let theirs = unnamed(&by_name);
    let theirs = theirs.await.must("a search naming no fields")?.data;
    let score = |h: &lakelands_api::query::Hits| h.hits.iter().map(|h| h.score).collect::<Vec<_>>();
    ensure!(
        score(&mine) == [3.0] && score(&theirs) == [1.0],
        "a search naming no fields reads only the searchable fields the principal knows of: \
         the owner's `ss pp ann` scores 3, theirs only `ann`, 1; they scored {:?} and {:?}",
        score(&mine),
        score(&theirs)
    );
    let mut by_default = reads_all(store.as_ref(), "y");
    by_default.grants.types.insert(
        staff.clone(),
        lakelands_api::principal::TypeGrants {
            default: Some(Access::None),
            fields: [("name".into(), Access::Read)].into(),
            ..Default::default()
        },
    );
    let got = read(cat.as_ref(), &by_default, "staff", "s1").await?;
    let secret = cat.find(&by_default, &staff, find_where("secret = 'x'"));
    let secret = secret.await;
    ensure!(
        got.masked.iter().map(FieldName::as_str).eq(["secret"])
            && matches!(refused_on(&secret), Some(("secret", Refusal::NotGranted))),
        "a sensitive field nobody named is listed as masked, and a filter on it is not \
         granted; get listed {:?} masked, the filter answered {secret:?}",
        got.masked
    );
    // `sensitive` shows a field, never a type: one given nothing of
    // `staff`, by `{default: none}` or by no entry, is answered as on
    // `ghost`, which is not declared.
    let mut kept = reads_all(store.as_ref(), "z");
    let none = lakelands_api::principal::TypeGrants {
        default: Some(Access::None),
        fields: Default::default(),
        ..Default::default()
    };
    kept.grants.types.insert(staff.clone(), none);
    for who in [&kept, &nobody(store.as_ref())] {
        let mut said: Vec<[Option<lakelands_api::Error>; 4]> = Vec::new();
        for name in ["staff", "ghost"] {
            let (t, secret) = (TypeName::from(name), json!({"secret": "x"}));
            let answers = [
                (cat.describe_type(who, &t, ReadAt::default()).await).map(drop),
                cat.get(who, &t, get_at("s1", None)).await.map(drop),
                cat.find(who, &t, Find::default()).await.map(drop),
                (try_upsert(cat.as_ref(), who, name, "s1", secret).await).map(drop),
            ];
            said.push(answers.map(Result::err));
        }
        let lake = cat.describe(who, ReadAt::default()).await;
        let lake = lake.must("describing the lake")?.data;
        // Each answered as on `ghost`, under its name, member for member.
        let ghost = alike_each(&said[0], &said[1], ("ghost", "staff"));
        ensure!(
            ghost && !lake.types.iter().any(|t| t.name == staff),
            "one whose grants give it nothing of `staff` is answered as on a type that is \
             not declared, whatever `secret` is declared; `{}` was answered {said:?}",
            who.id
        );
    }
    Ok(())
}

check!(
    WRITE_ONLY = a_write_only_field_is_written_and_never_shown,
    "006, extended (lakelands#393): a field granted write_only is written by upsert and retire like a writable one, while get, find, query, search, describe and the tools' read schemas never show it, a filter or a search that names it is refused not_granted naming it, and the write answers no value; a writable field built from one the principal may not read, write-only or masked, is write-only in turn: written, and probed as not granted, never as undeclared"
);

check!(
    VALID_AT_MEETS = a_walk_at_a_valid_time_probes_every_type_it_can_meet,
    "024 and 006, extended (lakelands#831): a walk at `valid_at` keeps each record it meets by its own type's valid-time fields, so a pattern probes those of every type its steps can meet, judged from the declarations before a record is read, with no record of the type written as with one: where two types own a relation, a reader masked on the valid-time field of either, the one a step is judged to reach or the other, is refused not_granted naming it by a chain's graph, a graph from depth 0, a path, `degree`, `components` and rows two hops away; one masked on that of a type two hops away is refused by a range that reaches it, and by a graph from depth 0, a path and `components`, which are judged wherever the relation is declared, and answered by one hop and `degree` as a reader of every field is; a range that names its `type` is refused for a type it passes through and answered where the masked type is no way to it; one hop that names the other owner, as its `type` or by its inverse, is answered, and so is a step of no hop that names no `type`; a reader of both is answered by their dates, one who holds the field at `none` as on a type with no valid time (lakelands#726), by each of the reads in March as in July, and one not shown the types word for word as on a lake that does not declare them; without `valid_at` the masked readers are answered"
);

check!(
    VALID_AT_VIEW = a_view_read_at_a_valid_time_probes_its_sources_valid_time,
    "024 and 006, extended (lakelands#881): a view read at `valid_at` is computed from its source's records as they are valid then, so its computation probes the source's valid-time fields, as deep as the view goes: to a reader masked on the field, the source's `valid_from` or its `valid_to`, `find`, `get`, a pattern started on the view and a walk that arrives at it are refused not_granted, of the view, of one with a `where` and of a view that reads it, in a month the source's record is valid in as in one it is not, naming the field to a holder of `schema` and nothing of the source to another (024); without `valid_at` both are answered as a reader of every field is; one who holds the field at `none` is answered at every time what a reader of every field is answered where the record is valid, the source reading to it as one with no valid time (lakelands#726); one not shown the source is shown none of the views, at any time, each answered word for word as a type no one declared and the walk as one over a relation no one declared; `describe_type` shows a kept view to each reader alike with a valid time and without (lakelands#798); a reader of every field is answered by the dates, a count of one where the record is valid; and (lakelands#910, lakelands#911) a read at `valid_at` reads no view's kept records where a valid time is declared in its tree, its own or a source's, merged and nested ones included, and reads them where none is, a page answered as without `valid_at` and in the heap of that page, give or take twice and 256 KiB, where its computation would hold each of 2 000 groups, while one whose valid time stands only in a merged source, a kept view over a valid-timed type, and a kept view over that one are computed at a valid time where the dates matter; a computed view's own valid time keeps its records as a type's does, by the bounds its reader is shown, so one valid until a hidden end is answered to its reader as a view with no end: a kept view with a valid time of its own is computed for a reader at `none` on that valid time, answered by `find`, a count, `get` and a pattern as its owner is, the group whose records do not hold left out, and refused naming it to the masked and write-only readers, each answered the kept records as the owner is without `valid_at`; a view over merged sources makes the probes of every source before it reads a record, so one masked or write-only on a source's valid time is refused not_granted, naming nothing, whichever source comes first and whatever measure of another fails, and is told the failing measure without `valid_at`, as a reader at `none` is at any time",
    given
);

check!(
    VALID_AT_PROBES = a_read_at_a_valid_time_probes_the_types_valid_time_fields,
    "024 and 006: a read at `valid_at` filters on the type's `valid_from` and `valid_to`, so `find`, `get` and a pattern started on the type probe them as a filter would: refused not_granted naming the field where it is masked; where it is hidden the type is described with no valid time, and the reads are answered as on a type with none (lakelands#726); without `valid_at` the same reads are answered; what is shown does not turn on a read's valid time (lakelands#798): a reviewer with the field masked or hidden is listed a proposed upsert and a proposed call on the type at `valid_at` as without it, whole, and accepts the upsert; 028 and 006, extended (lakelands#909): a `valid_to` with no `valid_from` is refused naming it by `declare`, `apply` and a migration, and a type in force that holds it, as a log the store did not write holds it, keeps it, declared again as it is and with a field more, and is read at `valid_at` as a type with no valid time, filtered by nothing and probing nothing: the masked, write-only and hidden readers are answered `find`, a count, `get` and a pattern as its owner is, of the type and of a view over it; and (lakelands#912) a read as of a position follows the valid time declared there, its filter and its probe: a type that had none is answered to one masked on the field declared since, and a type whose `valid_from` moved is refused to one masked on the field it named there and answered to one masked on the one it names now",
    given
);

async fn a_view_read_at_a_valid_time_probes_its_sources_valid_time(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use lakelands_api::query::Walk;
    const VIEWS: [&str; 3] = ["per_customer", "per_all", "per_some"];
    // What a door answered, whole.
    fn told<T: std::fmt::Debug>(r: lakelands_api::Result<lakelands_api::Reply<T>>) -> String {
        match r {
            Ok(r) => format!("answered {:?}", r.data),
            Err(lakelands_api::Error::Refused {
                field,
                reason: Refusal::NotGranted,
            }) => format!("not granted {}", field.as_ref().map_or("-", |f| f.as_str())),
            Err(e) => e.to_string(),
        }
    }
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    l.declare(type_decl("customer", vec![field("n", StorageKind::Text)]))
        .await?;
    l.declare_json(
        json!({"declare": "type", "name": "quote", "valid_from": "start", "valid_to": "end", "fields": [
        {"name": "start", "type": {"kind": "date_time"}},
        {"name": "end", "type": {"kind": "date_time"}},
        {"name": "to", "type": {"kind": "reference", "target": "customer"}}]}),
    )
    .await?;
    // `per_customer` counts a customer's quotes, `per_all` reads that view,
    // `per_some` counts under a `where`, and an account's `tally` arrives
    // at the first.
    let view = |name: &str, source: &str, expr: &str| {
        json!({"declare": "type", "name": name, "key": ["to"], "fields": [
            {"name": "to", "type": {"kind": "reference", "target": "customer"}},
            {"name": "n", "type": {"kind": "number"}}],
            "view": {"source": source, "group_by": ["to"], "measures": [{"name": "n", "expr": expr}]}})
    };
    l.declare_json(view("per_customer", "quote", "count()"))
        .await?;
    l.declare_json(view("per_all", "per_customer", "sum(n)"))
        .await?;
    let mut some = view("per_some", "quote", "count()");
    some["view"]["where"] = json!("to = 'c1'");
    l.declare_json(some).await?;
    l.declare_json(json!({"declare": "type", "name": "acct", "fields": [
        {"name": "who", "type": {"kind": "reference", "target": "customer"}}],
        "relations": [{"name": "tally", "target": "per_customer",
                       "derived": {"match": [{"from": "who", "to": "to"}]}}]}))
        .await?;
    l.upsert("customer", "c1", json!({"n": "c"})).await?;
    l.upsert(
        "quote",
        "q1",
        json!({"start": "2026-06-15T00:00:00Z", "to": "c1"}),
    )
    .await?;
    l.upsert("acct", "a1", json!({"who": "c1"})).await?;
    l.set("view_kept.per_customer", json!(true)).await?;
    let at = |day: &str| ReadAt {
        valid_at: Some(Timestamp::parse(&format!("2026-{day}T00:00:00Z")).expect("an instant")),
        ..ReadAt::default()
    };
    let times = [at("03-01"), at("07-01"), ReadAt::default()];
    let masked = |id| field_access(store.as_ref(), id, "quote", &[("start", Access::Masked)]);
    let mut author = masked("author");
    author.grants.schema = true;
    let end = [("end", Access::Masked)];
    let mut late = field_access(store.as_ref(), "late", "quote", &end);
    late.grants.schema = true;
    let mut blind = reads_all(store.as_ref(), "blind");
    let none = serde_json::from_value(json!({"default": "none"})).expect("a grant reads");
    blind.grants.types.insert("quote".into(), none);
    let readers = [
        reads_all(store.as_ref(), "plain"),
        masked("masked"),
        author,
        field_access(
            store.as_ref(),
            "at none",
            "quote",
            &[("start", Access::None)],
        ),
        blind,
        late,
    ];
    // Each reader at each time, at every door that computes a view.
    let mut said: Vec<Vec<Vec<(String, String)>>> = Vec::new();
    for who in &readers {
        let mut by_time = Vec::new();
        for at in times {
            let mut doors = Vec::new();
            for v in VIEWS {
                let ty = TypeName::from(v);
                let find = Find {
                    at,
                    ..Find::default()
                };
                doors.push((format!("find {v}"), told(cat.find(who, &ty, find).await)));
                let mut get = get_at("c1", None);
                get.at = at;
                doors.push((format!("get {v}"), told(cat.get(who, &ty, get).await)));
                let mut start = pattern(v, None, Vec::new(), Projection::Graph);
                start.at = at;
                doors.push((
                    format!("a pattern on {v}"),
                    told(cat.query(who, start).await),
                ));
            }
            let arrive = vec![step("tally", Walk::Out)];
            let mut walk = pattern("acct", Some("a1"), arrive, Projection::Graph);
            walk.at = at;
            doors.push((
                "a walk to the view".into(),
                told(cat.query(who, walk).await),
            ));
            by_time.push(doors);
        }
        said.push(by_time);
    }
    let (march, july, timeless) = (0, 1, 2);
    let answers = |who: usize, when: usize| -> Vec<String> {
        said[who][when].iter().map(|(_, a)| a.clone()).collect()
    };
    let all = |refused: &str| vec![refused.to_owned(); said[0][0].len()];
    // A reader of every field is answered by the dates: a group in July and
    // with no valid time, none in March.
    let mut held = vec![(
        0,
        timeless,
        answers(0, july),
        "with no valid time as in July",
    )];
    let mut wrong = Vec::new();
    for ((name, early), late) in said[0][march].iter().zip(answers(0, july)) {
        if *early == late || !late.starts_with("answered") || !late.contains("Number(1)") {
            wrong.push(format!("`plain`, {name}: {early} in March, {late} in July"));
        }
    }
    // Masked on a valid-time field of the source, its start or its end:
    // refused in both months, the field named to the holder of `schema`
    // alone; answered with no valid time.
    let masked = [
        (1, "not granted -"),
        (2, "not granted start"),
        (5, "not granted end"),
    ];
    for (who, refused) in masked {
        held.push((who, march, all(refused), "in March"));
        held.push((who, july, all(refused), "in July"));
        held.push((who, timeless, answers(0, timeless), "with no valid time"));
    }
    // Held from the field, the source has no valid time, at every door.
    // Not shown the source, there is no such view, and nothing says more.
    for when in [march, july, timeless] {
        held.push((3, when, answers(0, july), "the field at `none`"));
        held.push((4, when, answers(4, march), "as in March"));
    }
    for (who, when, want, why) in held {
        for ((name, answer), want) in said[who][when].iter().zip(want) {
            if *answer != want {
                wrong.push(format!("`{}`, {name}, {why}: {answer}", readers[who].id));
            }
        }
    }
    // To the one not shown the source each view is a type no one declared,
    // and the relation that arrives at one a relation no one declared.
    let ghost = TypeName::from("ghost");
    let over = vec![step("ghost", Walk::Out)];
    let walk = pattern("acct", Some("a1"), over, Projection::Graph);
    let absent = [
        told(cat.find(&readers[4], &ghost, Find::default()).await),
        told(cat.get(&readers[4], &ghost, get_at("c1", None)).await),
        told(
            cat.query(
                &readers[4],
                pattern("ghost", None, Vec::new(), Projection::Graph),
            )
            .await,
        ),
        told(cat.query(&readers[4], walk).await),
    ];
    for (n, (name, answer)) in said[4][march].iter().enumerate() {
        let (v, absent) = match VIEWS.get(n / 3) {
            Some(v) => (*v, &absent[n % 3]),
            None => ("tally", &absent[3]),
        };
        if answer.replace(v, "ghost") != *absent {
            wrong.push(format!("`blind`, {name}: {answer}, not {absent}"));
        }
    }
    // What a reader is shown of a kept view does not turn on a valid time.
    for who in &readers {
        let ty = TypeName::from("per_customer");
        let timed = told(cat.describe_type(who, &ty, times[march]).await);
        if timed != told(cat.describe_type(who, &ty, ReadAt::default()).await) {
            wrong.push(format!("`{}`, describe_type in March: {timed}", who.id));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a view read at a valid time probes its source's valid-time fields: {wrong:#?}"
    );
    Box::pin(super::valid_cells::a_kept_view_and_merged_sources_at_a_valid_time(&store)).await?;
    Box::pin(super::valid_cells::a_kept_view_with_no_valid_time_at_one(
        &store, &given,
    ))
    .await
}

async fn a_read_at_a_valid_time_probes_the_types_valid_time_fields(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    let mut contract = type_decl(
        "contract",
        vec![
            field("name", StorageKind::Text),
            field("start", StorageKind::DateTime),
            field("end", StorageKind::DateTime),
        ],
    );
    contract.valid_from = Some("start".into());
    contract.valid_to = Some("end".into());
    l.declare(contract).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "renew", "on": "contract", "risk": "high",
        "effect": "write", "set": [{"field": "name", "value": "'r'"}]}),
    )
    .await?;
    l.upsert(
        "contract",
        "c1",
        json!({"name": "x", "start": "2026-03-15T00:00:00Z"}),
    )
    .await?;
    let ty = TypeName::from("contract");
    let april = ReadAt {
        valid_at: Some(Timestamp::parse("2026-04-01T00:00:00Z").expect("an instant")),
        ..ReadAt::default()
    };
    let cases = ["start", "end"].map(|f| [(f, Access::None), (f, Access::Masked)]);
    for (i, (timed, access)) in cases.into_iter().flatten().enumerate() {
        let mut who = field_access(store.as_ref(), "clerk", "contract", &[(timed, access)]);
        who.grants.review = true;
        // A proposed write to the type is listed at a valid time as without one.
        let write = ProposedWrite::Upsert {
            ty: ty.clone(),
            key: format!("p{i}").as_str().into(),
            patch: patch(json!({"name": "y"})),
            opts: Default::default(),
        };
        let (renew, c1) = ("renew".into(), "c1".into());
        let called = cat.call(
            &l.own,
            &renew,
            Some(&c1),
            Default::default(),
            Default::default(),
        );
        let mut ids = Vec::new();
        for proposed in [cat.propose(&l.own, write, "why".into()).await, called.await] {
            let Outcome::Proposed { proposal, .. } = proposed.must("proposing a write")? else {
                return Err(Failure::new(
                    "a proposed upsert and a high-risk call must wait",
                ));
            };
            ids.push(proposal);
        }
        let proposal = ids[0].clone();
        let mut listed = Vec::new();
        for at in [ReadAt::default(), april] {
            let filter = ProposalFilter {
                at,
                ..Default::default()
            };
            let all = cat.proposals(&who, filter).await.must("listing")?.data;
            let mine = all.into_iter().filter(|p| ids.contains(&p.id));
            listed.push(mine.collect::<Vec<_>>());
        }
        let whole = |p: &lakelands_api::write::Proposal| p.write.is_some() && !p.more;
        ensure!(
            listed[0].len() == 2 && listed[0].iter().all(whole) && listed[0] == listed[1],
            "a reviewer with `{timed}` {access:?} is listed a proposed upsert and a proposed \
             call whole, at a valid time as without one; it was listed {listed:?}"
        );
        let accepted = cat.accept(&who, &proposal).await;
        accepted.must("accepting the proposal it was listed")?;
        for at in [ReadAt::default(), april] {
            let mut start = pattern("contract", None, Vec::new(), Projection::Graph);
            start.at = at;
            let mut get = get_at("c1", None);
            get.at = at;
            let find = Find {
                at,
                ..Find::default()
            };
            let answers = [
                ("find", cat.find(&who, &ty, find).await.map(drop)),
                ("get", cat.get(&who, &ty, get).await.map(drop)),
                ("a pattern's start", cat.query(&who, start).await.map(drop)),
            ];
            for (door, r) in answers {
                // Hidden, as a field `contract` does not declare, member for member.
                if let (Some(_), Access::None, Err(e)) = (at.valid_at, access, &r) {
                    let case = Case::UnknownField {
                        ty: ty.clone(),
                        field: timed.into(),
                    };
                    let reason = Refusal::Invalid(Box::new(case.into()));
                    let field = Some(timed.into());
                    let absent = lakelands_api::Error::Refused { field, reason };
                    alike_renamed(door, e, &absent, ("", ""))?;
                }
                let answered = match (at.valid_at, access) {
                    (None, _) | (Some(_), Access::None) => r.is_ok(),
                    (Some(_), _) => {
                        matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == timed)
                    }
                };
                ensure!(
                    answered,
                    "`{door}` by a clerk with `{timed}` {access:?} at valid time {:?} is \
                     answered without a valid time and where it is hidden, and refused \
                     naming `{timed}` where it is masked; it answered {r:?}",
                    at.valid_at
                );
            }
        }
    }
    // Boxed: the row's future is large enough already.
    let store = Box::pin(super::valid_cells::a_valid_to_alone(store, &given)).await?;
    Box::pin(super::valid_cells::the_valid_time_as_of_a_position(&store)).await
}

async fn a_walk_at_a_valid_time_probes_every_type_it_can_meet(store: Store) -> Held {
    use crate::conformance::fixture::principal;
    use lakelands_api::PrincipalKind;
    use lakelands_api::query::{Depth, Step, Walk};
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    // `bill` and `quote` are each `of` a customer, its `bills` and `quotes`,
    // and valid from a field of their own; a `note` is `of` a quote, two
    // hops from the customer.
    let owner = |name: &str, target: &str, from: &str| {
        json!({"declare": "type", "name": name, "valid_from": from, "fields": [
            {"name": from, "type": {"kind": "date_time"}},
            {"name": "to", "type": {"kind": "reference", "target": target}}],
            "relations": [{"name": "of", "target": target, "via": "to", "inverse": format!("{name}s")}]})
    };
    let day = |day: &str| format!("2026-{day}T00:00:00Z");
    let at = |d: &str| ReadAt {
        valid_at: Some(Timestamp::parse(&day(d)).expect("an instant")),
        ..ReadAt::default()
    };
    let (march, july) = (at("03-01"), at("07-01"));
    let deep = |min, max| Step {
        depth: Depth { min, max },
        ..step("of", Walk::In)
    };
    let from_c1 = |steps, project, at| {
        let mut p = pattern("customer", Some("c1"), steps, project);
        p.at = at;
        p
    };
    let graph = |step, at| from_c1(vec![step], Projection::Graph, at);
    let rows = || Projection::Rows { fields: vec![] };
    let reads = |at| {
        let mut path = graph(deep(0, 1), at);
        path.to = Some(("bill".into(), "b1".into()));
        let (one, both) = (vec![step("of", Walk::In)], vec![step("of", Walk::Both)]);
        [
            ("a chain's graph", graph(step("of", Walk::In), at)),
            ("a graph from depth 0", graph(deep(0, 1), at)),
            ("a path", path),
            ("degree", from_c1(one, Projection::Degree, at)),
            ("components", from_c1(both, Projection::Components, at)),
            ("rows two hops away", from_c1(vec![deep(2, 2)], rows(), at)),
            ("a graph one or two hops away", graph(deep(1, 2), at)),
        ]
    };
    // What a read answered, whole: its data, or its refusal.
    let said = |r: lakelands_api::Result<lakelands_api::Reply<QueryResult>>| match refused_on(&r) {
        Some((f, Refusal::NotGranted)) => format!("not granted {f}"),
        _ => r.map_or_else(|e| e.to_string(), |r| format!("{:?}", r.data)),
    };
    l.declare(type_decl("customer", vec![field("n", StorageKind::Text)]))
        .await?;
    l.declare_json(owner("bill", "customer", "since")).await?;
    l.upsert("customer", "c1", json!({"n": "c"})).await?;
    l.upsert("bill", "b1", json!({"since": day("01-15"), "to": "c1"}))
        .await?;
    // The answers of a lake that declares neither `quote` nor `note`.
    let plain = reads_all(store.as_ref(), "plain");
    let mut without = Vec::new();
    for (_, p) in reads(march).into_iter().chain(reads(july)) {
        without.push(said(cat.query(&plain, p).await));
    }
    l.declare_json(owner("quote", "customer", "start")).await?;
    l.declare_json(owner("note", "quote", "at")).await?;
    let to_note = |at| {
        let ty = Some("note".into());
        graph(Step { ty, ..deep(1, 2) }, at)
    };
    // Not shown `quote`, nor the `note` that hangs on it.
    let grants = json!({"default": "read", "types": {"quote": {"default": "none"}, "note": {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let blind = principal(store.as_ref(), PrincipalKind::Agent, "blind", grants);
    let (mut wrong, mut pinned) = (Vec::new(), without.clone());
    // Judged from the declarations: with no quote and no note written, and
    // then with one of each.
    for written in [false, true] {
        let seen = wrong.len();
        if written {
            l.upsert("quote", "q1", json!({"start": day("06-15"), "to": "c1"}))
                .await?;
            l.upsert("note", "n1", json!({"at": day("01-01"), "to": "q1"}))
                .await?;
        }
        // What a reader of every field is answered: the range to a note first.
        let mut open = vec![said(cat.query(&plain, to_note(july)).await)];
        for (_, p) in reads(july) {
            open.push(said(cat.query(&plain, p).await));
        }
        // Masked on a valid-time field: refused by each read that can meet
        // the type, the one a step is judged to reach or another. A note is
        // two hops away, or wherever `of` is declared for a neighbourhood
        // and a component; a range to a note passes a quote and no bill.
        for (ty, timed, spared, other) in [
            ("quote", "start", &[][..], Some("bill")),
            ("bill", "since", &[], Some("quote")),
            ("note", "at", &["a chain's graph", "degree"], None),
        ] {
            let who = field_access(store.as_ref(), "held", ty, &[(timed, Access::Masked)]);
            let refused = format!("not granted {timed}");
            for ((what, p), open) in reads(july).into_iter().zip(&open[1..]) {
                let answer = said(cat.query(&who, p).await);
                if answer != *[&refused, open][usize::from(spared.contains(&what))] {
                    wrong.push(format!("{what}, `{ty}.{timed}` masked: {answer}"));
                }
            }
            // No valid time probes nothing, and a step of no hop meets nothing.
            let none = reads(ReadAt::default()).into_iter();
            for (what, p) in none.chain([("a graph of no hop", graph(deep(0, 0), july))]) {
                if let Err(e) = cat.query(&who, p).await {
                    wrong.push(format!(
                        "{what}, at no valid time or of no hop, `{ty}.{timed}`: {e}"
                    ));
                }
            }
            let answer = said(cat.query(&who, to_note(july)).await);
            if answer != *if ty == "bill" { &open[0] } else { &refused } {
                wrong.push(format!(
                    "a range to a note, `{ty}.{timed}` masked: {answer}"
                ));
            }
            // A later step is probed as the first is: from a bill out to its
            // customer and in again, where a quote is met.
            let steps = vec![step("of", Walk::Out), step("of", Walk::In)];
            let mut later = pattern("bill", Some("b1"), steps, Projection::Graph);
            later.at = july;
            let open = cat.query(&plain, later.clone()).await.is_ok();
            let answer = said(cat.query(&who, later).await);
            if ty == "quote" && (answer != refused || !open) {
                wrong.push(format!(
                    "a chain's second step, `{ty}.{timed}` masked: {answer}; open: {open}"
                ));
            }
            // One hop that names the other owner, as its `type` or by its
            // inverse, keeps no record of this one.
            let Some(other) = other else { continue };
            let ty = Some(other.into());
            for (what, one) in [
                (
                    "a chain's graph",
                    Step {
                        ty: ty.clone(),
                        ..deep(1, 1)
                    },
                ),
                ("a graph from depth 0", Step { ty, ..deep(0, 1) }),
                ("an inverse", step(&format!("{other}s"), Walk::Out)),
            ] {
                let want = said(cat.query(&plain, graph(one.clone(), july)).await);
                let answer = said(cat.query(&who, graph(one, july)).await);
                if answer != want {
                    wrong.push(format!("{what}, `{other}` alone, `{timed}`: {answer}"));
                }
                pinned.push(want);
            }
        }
        pinned.extend(open);
        let shown = reads(march).into_iter().chain(reads(july));
        for ((what, p), absent) in shown.zip(&without) {
            let answer = said(cat.query(&blind, p).await);
            if &answer != absent {
                wrong.push(format!("{what}, not shown `quote`: {answer}, not {absent}"));
            }
        }
        let tag = if written { "" } else { ", no quote written" };
        wrong[seen..].iter_mut().for_each(|w| w.push_str(tag));
    }
    // A reader of both is answered by their dates; held from `start`, as
    // on a type with no valid time: by each read in March as in July.
    let hidden = field_access(
        store.as_ref(),
        "at none",
        "quote",
        &[("start", Access::None)],
    );
    for (who, at, want) in [
        (&plain, march, "b1 c1"),
        (&plain, july, "b1 c1 q1"),
        (&hidden, march, "b1 c1 q1"),
    ] {
        let answer = walked(cat.query(who, graph(step("of", Walk::In), at)).await);
        if answer != want {
            wrong.push(format!(
                "`{}` at {:?}: {answer}, not {want}",
                who.id, at.valid_at
            ));
        }
    }
    for ((what, early), (_, late)) in reads(march).into_iter().zip(reads(july)) {
        let early = said(cat.query(&hidden, early).await);
        if early != said(cat.query(&hidden, late).await) {
            wrong.push(format!(
                "{what}, `quote.start` at `none`, in March: {early}"
            ));
        }
        pinned.push(early);
    }
    // Each answer another was measured against is an answer.
    let answer = |a: &&String| !a.starts_with("Graph") && !a.starts_with("Rows");
    if let Some(e) = pinned.iter().find(answer) {
        wrong.push(format!("a read to be answered is refused: {e}"));
    }
    ensure!(
        wrong.is_empty(),
        "a walk at a valid time probes every type it can meet: {wrong:#?}"
    );
    Ok(())
}

async fn a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    let ty = type_decl(
        "patient",
        vec![
            field("name", StorageKind::Text),
            FieldDecl {
                sensitive: true,
                ..field("diagnosis", StorageKind::Text)
            },
            field("salary", StorageKind::Number),
        ],
    );
    l.declare(ty).await?;
    l.upsert(
        "patient",
        "p",
        json!({"name": "Ada", "diagnosis": "flu", "salary": 100}),
    )
    .await?;

    // Reads `name`, may write it; `diagnosis` masked by grant; no grant
    // at all for `salary`.
    let ty = TypeName::from("patient");
    let mut clerk = field_writer(store.as_ref(), "clerk", "patient", &["name"]);
    let grants = clerk
        .grants
        .types
        .get_mut(&ty)
        .expect("field_writer grants the type");
    grants.fields.insert("diagnosis".into(), Access::Masked);
    grants.fields.insert("salary".into(), Access::None);

    let masked = [FieldName::from("diagnosis")];
    let r = read(cat.as_ref(), &clerk, "patient", "p").await?;
    ensure!(
        r.fields.keys().map(FieldName::as_str).collect::<Vec<_>>() == ["name"]
            && r.masked == masked
            && !r.meta.contains_key("diagnosis")
            && !r.meta.contains_key("salary"),
        "`get` as the clerk must answer `name` with its value, `diagnosis` named in \
         `masked` without value or provenance, and nothing of `salary`; it answered \
         fields {:?}, masked {:?}, meta for {:?}",
        r.fields.keys().collect::<Vec<_>>(),
        r.masked,
        r.meta.keys().collect::<Vec<_>>()
    );
    let rows = all_found(cat.as_ref(), &clerk, "patient", Find::default()).await?;
    ensure!(
        rows.len() == 1 && rows[0].fields == r.fields && rows[0].masked == r.masked,
        "`find` as the clerk must mask exactly as `get` does; it answered {rows:?}"
    );

    let d = describe_type(cat.as_ref(), &clerk, "patient").await?;
    let described: Vec<(&str, Access)> = d
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.access))
        .collect();
    ensure!(
        described.contains(&("diagnosis", Access::Masked))
            && described.iter().all(|(f, _)| *f != "salary"),
        "`describe` as the clerk must list `diagnosis` as masked and leave `salary` \
         out; it listed {described:?}"
    );

    let lake = cat.describe(&clerk, ReadAt::default());
    let lake = lake.await.must("describing the lake as the clerk")?.data;
    let generated = tools(&lake)
        .map_err(|p| Failure::new(format!("the clerk's describe cannot become tools: {p:?}")))?;
    let tool = |door: Door| generated.iter().find(|t| t.door == door);
    let get = tool(Door::Get { ty: ty.clone() })
        .ok_or_else(|| Failure::new("the clerk may read `patient` but has no get tool"))?;
    let readable = &get.output_schema["properties"]["data"]["properties"]["fields"]["properties"];
    ensure!(
        readable.get("name").is_some()
            && readable.get("diagnosis").is_none()
            && readable.get("salary").is_none(),
        "the clerk's get tool must describe only the fields it reads (`name`); its \
         schema lists {readable}"
    );
    let upsert_tool = tool(Door::Upsert { ty: ty.clone() })
        .ok_or_else(|| Failure::new("the clerk may write `name` but has no upsert tool"))?;
    let writable = &upsert_tool.input_schema["properties"]["fields"]["properties"];
    ensure!(
        writable.get("name").is_some()
            && writable.get("diagnosis").is_none()
            && writable.get("salary").is_none(),
        "the clerk's upsert tool must offer only `name`; it offers {writable}"
    );
    Ok(())
}

async fn a_write_only_field_is_written_and_never_shown(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // `summary` and `gist` are written from `note` (write-only) and
    // `secret` (masked): writable, so write-only in turn.
    let from = |name: &str, source: &str| FieldDecl {
        depends_on: vec![source.into()],
        ..field(name, StorageKind::Text)
    };
    let fields = vec![
        searchable("name"),
        searchable("note"),
        field("secret", StorageKind::Text),
        from("summary", "note"),
        from("gist", "secret"),
    ];
    l.declare(type_decl("patient", fields)).await?;
    let ty = TypeName::from("patient");
    let intake = field_access(
        store.as_ref(),
        "intake",
        "patient",
        &[
            ("name", Access::Write),
            ("note", Access::WriteOnly),
            ("secret", Access::Masked),
            ("summary", Access::Write),
            ("gist", Access::Write),
        ],
    );
    let opts = lakelands_api::write::WriteOpts::default;
    // `summary` and `gist` fold to write-only: written, not undeclared.
    let fields = json!({"name": "Ada", "note": "kept", "summary": "s", "gist": "g"});
    let written = try_upsert(cat.as_ref(), &intake, "patient", "p", fields).await;
    let mut red = Vec::new();
    if !matches!(
        written,
        Ok(lakelands_api::Outcome::Applied { output: None, .. })
    ) {
        red.push(format!("the write answered {written:?}"));
    }
    let stored = read(cat.as_ref(), own, "patient", "p").await.ok();
    if stored.and_then(|r| r.fields.get("note").cloned()) != Some(json!("kept")) {
        red.push("the owner does not read the `note` written".into());
    }
    let search = |fields| lexical("patient", "Ada", fields, true);
    // What `intake` reads never names `note`.
    let all = Projection::Rows { fields: vec![] };
    let rows = pattern("patient", Some("p"), vec![], all);
    let answers = [
        (
            "get",
            json!(read(cat.as_ref(), &intake, "patient", "p").await.ok()),
        ),
        (
            "find",
            json!(
                all_found(cat.as_ref(), &intake, "patient", Find::default())
                    .await
                    .ok()
            ),
        ),
        (
            "query",
            json!(cat.query(&intake, rows).await.map(|r| r.data).ok()),
        ),
        (
            "search in `name`",
            json!(
                cat.search(&intake, search(Some(vec!["name".into()])))
                    .await
                    .ok()
                    .map(|h| h.data.hits)
            ),
        ),
    ];
    for (what, answer) in answers {
        let shown = answer.to_string();
        if answer.is_null()
            || ["note", "summary", "gist"]
                .iter()
                .any(|f| shown.contains(f))
            || !shown.contains("Ada")
        {
            red.push(format!("{what} as `intake` answered {answer}"));
        }
    }
    // A probe of it is not granted, naming it.
    let probe = find_where("note = 'kept'");
    let filtered = cat.find(&intake, &ty, probe).await.map(drop);
    let searched = cat.search(&intake, search(Some(vec!["note".into()])));
    let searched = searched.await.map(drop);
    let gist = find_where("gist = 'g'");
    let folded = cat.find(&intake, &ty, gist).await.map(drop);
    for (what, field, answer) in [
        ("a filter on `note`", "note", filtered),
        ("a search in `note`", "note", searched),
        ("a filter on `gist`", "gist", folded),
    ] {
        if !matches!(
            refused_on(&answer),
            Some((f, lakelands_api::Refusal::NotGranted)) if f == field
        ) {
            red.push(format!("{what} answered {answer:?}"));
        }
    }
    let d = describe_type(cat.as_ref(), &intake, "patient").await?;
    let write_only = |name: &str| {
        d.fields
            .iter()
            .any(|f| f.name.as_str() == name && f.access == Access::WriteOnly)
    };
    if !["note", "summary", "gist"].iter().all(|f| write_only(f)) {
        red.push(format!("`describe` lists {:?}", d.fields));
    }
    let lake = cat.describe(&intake, ReadAt::default());
    let lake = lake.await.must("describing the lake")?;
    let generated = tools(&lake.data).map_err(|p| Failure::new(format!("no tools: {p:?}")))?;
    let schema = |door: Door| {
        generated
            .iter()
            .find(|t| t.door == door)
            .map(|t| (t.input_schema.to_string(), t.output_schema.to_string()))
    };
    let get = schema(Door::Get { ty: ty.clone() }).map(|(_, out)| out);
    let upsert = schema(Door::Upsert { ty: ty.clone() }).map(|(input, _)| input);
    let offers = |u: &String| {
        ["note", "summary", "gist"]
            .iter()
            .all(|f| u.contains(&format!("\"{f}\"")))
    };
    if get
        .as_ref()
        .is_none_or(|g| g.contains("note") || g.contains("gist"))
        || upsert.as_ref().is_none_or(|u| !offers(u))
    {
        red.push(format!(
            "the get tool answers {get:?}, the upsert tool takes {upsert:?}"
        ));
    }
    // Retiring takes write on every field: write-only counts.
    let mut keeper = intake.clone();
    let grants = keeper
        .grants
        .types
        .get_mut(&ty)
        .expect("granted on `patient`");
    grants.fields.insert("secret".into(), Access::WriteOnly);
    let retired = cat.retire(&keeper, &ty, &"p".into(), opts()).await;
    if retired.is_err() {
        red.push(format!(
            "`retire`, every field writable, answered {retired:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a write-only field is written like a writable one and never shown: {}",
        red.join("; ")
    );
    Ok(())
}

/// A lexical search of `ty` for `text`, five hits a page, in `fields` or
/// in every searchable field.
fn lexical(ty: &str, text: &str, fields: Option<Vec<FieldName>>, with_meta: bool) -> Search {
    Search {
        ty: TypeName::from(ty),
        mode: SearchMode::Lexical {
            text: text.into(),
            fields,
            text_fields: None,
        },
        k: 5,
        cursor: None,
        filter: None,
        with_meta,
        matches: false,
        at: ReadAt::default(),
        rerank: None,
    }
}
