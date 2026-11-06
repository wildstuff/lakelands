//! A declaration reads only what its author may read, and a write's
//! answer names a reaction only to one who reads what its condition reads
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`
//! and `docs/decisions/028-authoring-is-strict-and-whole.md`, extended).

use lakelands_api::error::RecordName;
use lakelands_api::write::Outcome;

use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::alike_each;
use crate::conformance::prelude::*;

check!(
    AUTHOR_READS = a_declaration_reads_only_what_its_author_may_read,
    "024 and 028, extended (lakelands#538): a declaration whose author may not know of a field it reads is answered as one that reads a field the type does not have, whatever reads it: a set's `where`, a trigger's `when`, a verb's `requires`, what it sets a field from and the field it sets, a derived field, a field computed by a function, an embedding, a view's `where`, grouping and measure, and a derived relation's pair; by `declare`, `apply`, `add_field`, a migration and a proposal alike; a field the author knows of and may not read is refused as not granted on it, and a verb may set it; a field the type's shape names, as a valid time or a relation's `via`, stands as declared for an author who may not know of it, and a key field it may not know of hides the type from it (lakelands#715); `add_field` adds to its author's view of the type; a write's answer names a trigger or an object set only to one who reads every field its condition names, shown the definition or not, and still says `through`",
    embedding
);

check!(
    HIDDEN_TYPE = a_type_its_author_is_not_shown_is_a_type_that_is_not_declared,
    "027 and 028, extended (lakelands#538): to a principal that holds `schema` and is not shown a type, the type is one that is not declared: `add_field`, a migration, its apply and its proposal answer that there is no such type, and a trigger, a verb and an object set on it, declared or proposed, that it is not declared, each in the words an undeclared type gets; to declare the type or propose it is not granted and names nothing; none of them writes; a value type's preview and its refused apply show a use and a misfit only in a field their caller is shown and reads, the totals whole"
);

fn decl(v: Value) -> Declaration {
    serde_json::from_value(v).expect("a declaration")
}

fn text(n: &str) -> Value {
    json!({"name": n, "type": {"kind": "text"}})
}

/// `base` with the members of `with` put over it.
fn over(mut base: Value, with: Value) -> Value {
    for (k, v) in with.as_object().into_iter().flatten() {
        base[k] = v.clone();
    }
    base
}

/// Each way a declaration reads the field `f` of `account`, by name.
fn ways(f: &str) -> Vec<(&'static str, Vec<Declaration>)> {
    let (on, up) = (format!("{f} = '2'"), format!("upper({f})"));
    // `account`, as its author sees it, with a field `x` computed by `how`.
    let account = |how: Value| {
        let x = json!({"name": "x", "type": {"kind": "text"}, "authority": "derived"});
        let fields = [text("a"), text("m"), text("s"), over(x, how)];
        decl(json!({"declare": "type", "name": "account", "fields": fields}))
    };
    let view = |how: Value| {
        let n = json!([{"name": "n", "expr": "count()"}]);
        let view = json!({"source": "account", "group_by": ["a"], "measures": n});
        let view = over(view, how);
        let by = view["group_by"][0].clone();
        let fields = json!([{"name": by, "type": {"kind": "text"}},
                            {"name": "n", "type": {"kind": "number"}}]);
        decl(
            json!({"declare": "type", "name": "seen", "key": [by], "fields": fields,
                    "view": view}),
        )
    };
    let pair = json!({"name": "hits", "target": "account", "cardinality": "many_to_many",
                      "derived": {"match": [{"from": "k", "to": f}]}});
    let guess = json!({"declare": "type", "name": "guess", "fields": [text("k")]});
    let shout = json!({"declare": "function", "name": "shout", "backing": "expr",
                       "inputs": [text(f)], "expr": up});
    let mut requires = write_verb("v", "account", "a", "'x'");
    if let Declaration::Verb(v) = &mut requires {
        v.requires = Some(on.as_str().into());
    }
    let vector = json!({"type": {"kind": "vector", "dimensions": 2},
                        "embed": {"from": [f], "model": EMBED_MODEL}});
    let max = json!({"measures": [{"name": "n", "expr": format!("max({f})")}]});
    vec![
        ("a set's `where`", vec![object_set_on("o", "account", &on)]),
        (
            "a trigger's `when`",
            vec![trigger_on("t", "account", Some(&on))],
        ),
        ("a verb's `requires`", vec![requires]),
        (
            "what a verb sets from",
            vec![write_verb("v", "account", "a", f)],
        ),
        ("a derived field", vec![account(json!({"derived": up}))]),
        (
            "a function's input",
            vec![decl(shout), account(json!({"function": "shout"}))],
        ),
        ("an embedding", vec![account(vector)]),
        ("a view's `where`", vec![view(json!({"where": on}))]),
        ("a view's grouping", vec![view(json!({"group_by": [f]}))]),
        ("a view's measure", vec![view(max)]),
        (
            "a derived relation",
            vec![decl(over(guess, json!({"relations": [pair]})))],
        ),
    ]
}

async fn a_declaration_reads_only_what_its_author_may_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let fields = [text("a"), text("b"), text("m"), text("s")];
    l.declare_json(json!({"declare": "type", "name": "account", "fields": fields}))
        .await?;
    l.upsert("account", "1", json!({"a": "1", "b": "2", "m": "2"}))
        .await?;
    // `partial` holds `schema` and `review`, writes `a`, may not know of
    // `b` and sees `m` masked; `full` reads both.
    let grants = [
        ("a", Access::Write),
        ("b", Access::None),
        ("m", Access::Masked),
    ];
    let mut partial = field_access(store.as_ref(), "partial", "account", &grants);
    let mut full = field_access(store.as_ref(), "full", "account", &grants[..1]);
    for who in [&mut partial, &mut full] {
        (who.grants.schema, who.grants.review, who.grants.restricted) = (true, true, true);
    }

    // R2: a field its author may not know of answers as one the type does
    // not have, for every way of reading it; a masked one as not granted.
    let mut red = Vec::new();
    let all = ways("b").into_iter().zip(ways("nope")).zip(ways("m"));
    for (((way, hidden), (_, absent)), (_, masked)) in all {
        let (hidden, absent) = (
            l.cat.apply(&partial, hidden).await,
            l.cat.apply(&partial, absent).await,
        );
        // Member for member and word for word, as over `nope`.
        match (&hidden, &absent) {
            (Err(h), Err(a)) => {
                if let Err(f) = alike_renamed(&format!("{way} over `b`"), h, a, ("nope", "b")) {
                    red.push(f.message);
                }
            }
            _ => red.push(format!(
                "{way}: over `b` {hidden:?}, over `nope` {absent:?}"
            )),
        }
        let masked = l.cat.apply(&partial, masked).await;
        if !matches!(refused_on(&masked), Some(("m", Refusal::NotGranted))) {
            red.push(format!("{way}: over `m` {masked:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "a declaration over a field its author may not know of is answered as over one the \
         type does not have, and over one it sees masked as not granted on it; {}",
        red.join("; ")
    );

    // The other doors a declaration comes through answer `b` as `nope`
    // too: `declare`, `add_field`, a migration and a proposal of either.
    let (c, account) = (&l.cat, "account".into());
    let mut said = Vec::new();
    for f in ["b", "nope"] {
        let set = object_set_on("o", "account", &format!("{f} = '2'"));
        let x = json!({"name": "x", "type": {"kind": "text"}, "authority": "derived",
                       "derived": format!("upper({f})")});
        let to = json!({"name": "account", "fields": [text("a"), text("m"), text("s"), x]});
        let to = migration(serde_json::from_value(to).expect("a type"));
        let field = serde_json::from_value(x).expect("a field");
        let bundle = ProposedWrite::Declare {
            declarations: vec![set.clone()],
            plan: None,
        };
        let migrate = ProposedWrite::Migrate {
            migration: Box::new(to.clone()),
            report: Default::default(),
        };
        let target = write_verb("w", "account", f, "'x'");
        said.push([
            why(c.declare(&partial, set).await),
            why(c.add_field(&partial, &account, field).await),
            why(c.migrate(&partial, to).await),
            why(c.propose(&partial, bundle, "why".into()).await),
            why(c.propose(&partial, migrate, "why".into()).await),
            why(c.declare(&partial, target).await),
        ]);
    }
    ensure!(
        alike_each(&said[0], &said[1], ("nope", "b")),
        "`declare`, `add_field`, `migrate`, a proposal of a set and of a migration, and a verb \
         that sets the field answer a field their author may not know of as one the type does \
         not have; over `b` {:?}, over `nope` {:?}",
        said[0],
        said[1]
    );
    // What a type's shape names stands as declared: an author kept from
    // its valid time or a relation's `via` field declares the type again
    // from its view, with one more field, and migrates it; the valid time
    // over that field is not in its view (lakelands#726, lakelands#692),
    // nor the relation it is not shown, and both are kept (lakelands#727). One
    // kept from its key field is not shown the type (lakelands#715): its
    // declaration is not granted and its migration finds no type.
    let reference = json!({"kind": "reference", "target": "account"});
    let owner = json!({"name": "owner", "target": "account", "via": "to"});
    let shapes = [
        (
            "k",
            json!({"name": "keyed", "key": ["k"]}),
            json!({"kind": "text"}),
        ),
        (
            "since",
            json!({"name": "timed", "valid_from": "since"}),
            json!({"kind": "date_time"}),
        ),
        (
            "to",
            json!({"name": "doc", "relations": [owner]}),
            reference,
        ),
    ];
    let mut kept = Vec::new();
    for (hidden, shape, kind) in shapes {
        let field = json!({"name": hidden, "type": kind, "required": hidden == "k"});
        let whole = json!({"declare": "type", "fields": [field, text("a")]});
        l.declare_json(over(shape.clone(), whole)).await?;
        let name = shape["name"].as_str().unwrap_or_default().to_owned();
        let mut author = field_access(store.as_ref(), "author", &name, &[(hidden, Access::None)]);
        (author.grants.schema, author.grants.restricted) = (true, true);
        let mut seen = shape.clone();
        if let Some(s) = seen.as_object_mut() {
            s.remove("relations");
            s.remove("valid_from");
        }
        let view = |a: &str| over(seen.clone(), json!({"fields": [text(a), text("more")]}));
        let again = decl(over(view("a"), json!({"declare": "type"})));
        let mut to = migration(serde_json::from_value(view("a2")).expect("a type"));
        to.renames = vec![("a".into(), "a2".into())];
        let again = why(c.declare(&author, again).await);
        kept.push((name, again, why(c.migrate(&author, to).await)));
    }
    let doc = l.describe_type("doc").await?.relations.len();
    ensure!(
        doc == 1,
        "`doc` keeps its relation `owner`; it holds {doc} relations"
    );
    let hidden = (
        Some(lakelands_api::Error::Refused {
            field: None,
            reason: Refusal::NotGranted,
        }),
        Some(lakelands_api::Error::NotFound {
            missing: lakelands_api::Missing::Type {
                name: "keyed".into(),
            },
        }),
    );
    ensure!(
        kept.iter().all(|k| match k.0.as_str() {
            "keyed" => (&k.1, &k.2) == (&hidden.0, &hidden.1),
            _ => k.1.is_none() && k.2.is_none(),
        }),
        "an author kept from a valid time or a `via` field declares the type again from its \
         view and migrates it, and one kept from a key field is answered as for a type it is \
         not shown; by type, at `declare` and at `migrate`: {kept:?}"
    );
    // A verb may set a field its author knows of and may not read, and
    // `add_field` adds to its author's view of the type.
    let sets = c.declare(&partial, write_verb("w", "account", "m", "'x'"));
    let plain = serde_json::from_value(text("extra")).expect("a field");
    let (sets, added) = (sets.await, c.add_field(&partial, &account, plain).await);
    ensure!(
        sets.is_ok() && added.is_ok(),
        "one who sees `m` masked declares a verb that sets it, and one who may not know of \
         `b` adds a field to the type; the verb {sets:?}, the field {added:?}"
    );

    // Another's derived field over `m` does not keep one who sees `m`
    // masked from declaring the type again; its own over `m` does.
    let shown = |n: &str| {
        over(
            text(n),
            json!({"authority": "derived", "derived": "upper(m)"}),
        )
    };
    let field = serde_json::from_value(shown("shown")).expect("a field");
    let by_owner = c.add_field(&l.own, &account, field).await;
    by_owner.must("the owner adds `shown`")?;
    let mut fields = vec![
        text("a"),
        text("m"),
        text("s"),
        text("extra"),
        shown("shown"),
    ];
    let again = |f: &[Value]| decl(json!({"declare": "type", "name": "account", "fields": f}));
    let taken = c.declare(&partial, again(&fields)).await;
    fields.push(shown("mine"));
    let own = c.declare(&partial, again(&fields)).await;
    ensure!(
        taken.is_ok() && matches!(refused_on(&own), Some(("m", Refusal::NotGranted))),
        "one who sees `m` masked declares again a type that holds another's field derived \
         from `m`, and is not granted its own; again {taken:?}, with its own {own:?}"
    );

    // R3: a write's answer names a reaction to one who reads what its
    // condition reads. The owner declares; `partial` is shown both.
    l.upsert("account", "2", json!({"a": "1", "b": "2"}))
        .await?;
    let on = "b = '2' and a = 'go'";
    let reactions = vec![
        trigger_on("t", "account", Some(on)),
        object_set_on("held", "account", on),
    ];
    c.apply(&l.own, reactions).await.must("declaring")?;
    let mut named = Vec::new();
    for (who, key) in [(&partial, "1"), (&full, "2")] {
        let wrote = try_upsert(c.as_ref(), who, "account", key, json!({"a": "go"})).await;
        let Outcome::Applied { set_off, .. } = wrote.must("writing `a`")? else {
            return Err(Failure::new("the write is applied".to_owned()));
        };
        let reached = set_off.through.is_some();
        named.push((set_off.fired, set_off.joined, reached));
    }
    // The same through an accept: the owner proposes a set over `b`.
    for (who, name) in [(&partial, "by_b"), (&full, "by_b_too")] {
        let declarations = vec![object_set_on(name, "account", "b = '2'")];
        let bundle = ProposedWrite::Declare {
            declarations,
            plan: None,
        };
        let proposed = c.propose(&l.own, bundle, "why".into()).await;
        let Outcome::Proposed { proposal, .. } = proposed.must("proposing a set")? else {
            return Err(Failure::new("the set is proposed".to_owned()));
        };
        let Outcome::Applied { set_off, .. } = c.accept(who, &proposal).await.must("accepting")?
        else {
            return Err(Failure::new("the accept applies".to_owned()));
        };
        named.push((set_off.fired, set_off.joined, set_off.through.is_some()));
    }
    let names = |n: &(Vec<_>, Vec<String>, bool)| (n.0.len(), n.1.clone());
    let told: Vec<_> = named.iter().map(names).collect();
    ensure!(
        told == [
            (0, vec![]),
            (1, vec!["held".to_owned()]),
            (0, vec![]),
            (0, vec!["by_b_too".to_owned()])
        ] && named[0].2 == named[1].2,
        "a write's answer names a trigger or an object set only to one who reads every field \
         its condition names, and says `through` to both; `partial`, then `full`, by a write \
         and by an accept, are named (fired, joined, through) {named:?}"
    );
    Ok(())
}

/// Why an answer is refused.
fn why<T>(a: lakelands_api::Result<T>) -> Option<lakelands_api::Error> {
    a.err()
}

async fn a_type_its_author_is_not_shown_is_a_type_that_is_not_declared(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let state = |one_of: &[&str]| json!({"name": "state", "kind": "text", "one_of": one_of});
    let declared = over(state(&["x", "y"]), json!({"declare": "value_type"}));
    l.declare_json(declared).await?;
    let held = json!({"name": "s", "type": {"kind": "text", "value_type": "state"}});
    let account = json!({"declare": "type", "name": "account", "fields": [text("a"), held]});
    l.declare_json(account).await?;
    l.upsert("account", "1", json!({"a": "1", "s": "x"}))
        .await?;
    // `blank` holds `schema` and `review` and reads nothing.
    let mut blank = crate::conformance::fixture::reviewer(s, true);
    blank.grants.default = Access::None;
    let (c, before) = (&l.cat, s.head().await.must("reading the head")?);

    // Each door, on `account` and on `ghost`, which is not declared.
    let mut said = Vec::new();
    for name in ["account", "ghost"] {
        let ty = name.into();
        let shape = |f: &str| json!({"name": name, "fields": [text(f)]});
        let to = || migration(serde_json::from_value(shape("a")).expect("a type"));
        let proposed = ProposedWrite::Migrate {
            migration: Box::new(to()),
            report: Default::default(),
        };
        let on = [
            trigger_on("t", name, Some("a = 'x'")),
            trigger_on("u", name, None),
            write_verb("v", name, "a", "'x'"),
            object_set_on("o", name, "a = 'x'"),
        ];
        let bundle = ProposedWrite::Declare {
            declarations: on.to_vec(),
            plan: None,
        };
        let field = |f: &str| serde_json::from_value(text(f)).expect("a field");
        let mut answers = vec![
            why(c.add_field(&blank, &ty, field("a")).await),
            why(c.add_field(&blank, &ty, field("new")).await),
            why(c.migrate(&blank, to()).await),
            why(c.migrate_apply(&blank, to(), "an id").await),
            why(c.propose(&blank, proposed, "why".into()).await),
            why(c.propose(&blank, bundle, "why".into()).await),
        ];
        for d in on {
            answers.push(why(c.declare(&blank, d).await));
        }
        said.push(answers);
    }
    ensure!(
        alike_each(&said[0], &said[1], ("ghost", "account")),
        "a type its author is not shown answers as one that is not declared, at `add_field`, \
         a migration, its apply and its proposal, a proposed bundle, and a trigger, a verb \
         and an object set on it; on `account` and on `ghost` {said:?}"
    );
    // To declare the type, or to propose it, is not granted and names
    // nothing; and nothing was written.
    let mut refused = Vec::new();
    for fields in [json!([]), json!([text("a")]), json!([text("new")])] {
        let d = decl(json!({"declare": "type", "name": "account", "fields": fields}));
        let bundle = ProposedWrite::Declare {
            declarations: vec![d.clone()],
            plan: None,
        };
        refused.push(c.declare(&blank, d).await);
        refused.push(c.propose(&blank, bundle, "why".into()).await);
    }
    let not_granted = |a: &lakelands_api::Result<Outcome>| {
        reason(a) == Some(&Refusal::NotGranted) && refused_on(a).is_none()
    };
    let head = s.head().await.must("reading the head")?;
    ensure!(
        refused.iter().all(not_granted) && head == before,
        "to declare or propose a type its author is not shown is not granted, names nothing \
         and writes nothing; answered {refused:?}, the head moved from {before:?} to {head:?}"
    );

    // A value type's preview, and its refused apply, show a use and a
    // misfit only where their caller is shown the type and reads the field.
    let to = || -> lakelands_api::ontology::ValueTypeDecl {
        serde_json::from_value(state(&["y"])).expect("a value type")
    };
    let mut shown = Vec::new();
    for who in [&blank, &l.own] {
        let preview = c.migrate_value_type(who, to().into()).await;
        let report = preview.must("the preview")?.data;
        let applied = c.migrate_value_type_apply(who, to().into(), &report.report_id);
        let named = matches!(invalid_case(&applied.await),
            Some(Case::ValueTypeNotTightened { first, .. }) if first.iter().any(|r| matches!(r,
                RecordName::Of { ty, .. } | RecordName::FieldOf { ty, .. } if ty == "account")));
        let (uses, first) = (report.fields.len(), report.first.len());
        shown.push((uses, first, report.misfits, named));
    }
    ensure!(
        shown == [(0, 0, 1, false), (1, 1, 1, true)],
        "a value type's preview and its refused apply name a use and a misfit only to one who \
         reads the field, the totals whole; `blank`, then the owner, are shown (uses, \
         misfits named, misfits counted, the refusal names the type) {shown:?}"
    );
    Ok(())
}
