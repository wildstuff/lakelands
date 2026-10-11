//! Declarations: the schema hash, what `declare` refuses, and the `sys.`
//! reservation (`docs/decisions/003-reservation-by-namespace.md`,
//! `docs/decisions/007-schema-hash-covers-shape.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`,
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! `docs/decisions/028-authoring-is-strict-and-whole.md`).

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::tools::{Door, GENERIC_TOOLS, MAX_TOOL_NAME, ToolSpec, tools};
use lakelands_api::write::{Declaration, Outcome, ProposedWrite, Recompute};
use lakelands_api::{Error, Grants, LakeStore, PrincipalKind, ReadAt, Refusal, TypeName};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, computed_by, dec, decimal, events_after, field, head_unmoved, invalid_case,
    is_invalid, migration, object_set_on, principal, reason, refused_unmoved, reviewer, trigger_on,
    type_decl, value, value_type, view_of, write_verb,
};
use crate::conformance::{Failure, Held, Store};

check!(
    ADD_FIELD_GRANT_FIRST =
        add_field_without_the_schema_grant_is_refused_before_it_looks_at_the_type,
    "006 and 024, extended: add_field without the schema grant is refused not_granted whether the type is undeclared or already has the field, so the refusal tells nothing about the ontology"
);

check!(
    SCHEMA_HASH_VECTOR = the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins,
    "007 and the log format's vectors: a type's schema hash is SHA-256 over its canonical shape, the same on every engine"
);

check!(
    SCHEMA_HASH_IGNORES_MEANING = describing_and_relabelling_a_type_leaves_its_schema_hash,
    "007: descriptions, labels and field order in a revision are meaning, not shape, and leave the schema hash as it was"
);

check!(
    RESHAPING_DECLARE_REFUSED = declare_refuses_a_revision_that_reshapes_stored_records,
    "027: retyping, removing or requiring a field goes through `migrate`; `declare` refuses it and appends nothing"
);

check!(
    DECLARE_REFUSALS = declare_refuses_bad_names_unknown_references_and_tool_name_collisions,
    "028 and 014: a name that is not an ASCII identifier, a reference to an undeclared type, or a name that collides with a tool is refused whole"
);

check!(
    SYS_RESERVED = an_ordinary_principal_cannot_declare_a_sys_type,
    "003: every type name starting `sys.` is reserved; an ordinary principal, even the owner, is refused declaring one and nothing is appended"
);

const VECTOR: &str = include_str!("../../../../../docs/log-format/vectors/schema-hash.json");

async fn the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins(store: Store) -> Held {
    let vector: Value = serde_json::from_str(VECTOR)
        .map_err(|e| Failure::new(format!("the schema-hash vector does not parse: {e}")))?;
    let decl: TypeDecl = serde_json::from_value(vector["declaration"].clone())
        .map_err(|e| Failure::new(format!("the vector's declaration does not read: {e}")))?;
    let want = vector["schema_hash"].as_str().unwrap_or_default();

    let l = Lake::of(store.as_ref());

    // The view reads an invoice with the fields its measures name.
    let invoice = type_decl(
        "invoice",
        vec![
            field("customer", StorageKind::Text),
            field("amount", StorageKind::Decimal),
            field("status", StorageKind::Text),
            field("due", StorageKind::DateTime),
        ],
    );
    l.declare(invoice).await?;
    let name = decl.name.clone();
    l.declare(decl).await?;
    let d = l.describe_type(name.as_str()).await?;
    ensure!(
        d.lifecycle.schema_hash == want,
        "`{name}` declared from the log format's vector describes schema hash {}, but \
         the vector pins {want}",
        d.lifecycle.schema_hash
    );
    Ok(())
}

async fn hash_of(store: &dyn LakeStore, ty: &str) -> Result<(String, u32), Failure> {
    let d = Lake::of(store).describe_type(ty).await?;
    Ok((d.lifecycle.schema_hash, d.lifecycle.version))
}

async fn describing_and_relabelling_a_type_leaves_its_schema_hash(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let first = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
        ],
    );
    l.declare(first.clone()).await?;
    let (hash, version) = hash_of(store.as_ref(), "memo").await?;

    let mut revised = first;
    revised.description = Some("A note kept for later.".into());
    revised.label = Some("Memo".into());
    revised.fields.reverse();
    revised.fields[0].description = Some("What the memo says.".into());
    revised.fields[0].label = Some("Body".into());
    l.declare(revised).await?;
    let (after, next) = hash_of(store.as_ref(), "memo").await?;
    ensure!(
        after == hash,
        "redeclaring `memo` with new descriptions, labels and field order moved its \
         schema hash from {hash} to {after}; those are meaning, not shape"
    );
    ensure!(
        next == version + 1,
        "the revision must still count as one: version {version} became {next}"
    );
    Ok(())
}

async fn declare_refuses_a_revision_that_reshapes_stored_records(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let first = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    l.declare(first.clone()).await?;

    let mut retyped = first.clone();
    retyped.fields[1].ty = FieldType::kind(StorageKind::Text);
    let mut removed = first.clone();
    removed.fields.pop();
    let mut required = first.clone();
    required.fields[0].required = true;
    for (what, decl) in [
        ("retyping `pages`", retyped),
        ("removing `pages`", removed),
        ("requiring `title`", required),
    ] {
        let before = store.head().await.must("reading the head")?;
        let answer = cat.declare(own, Declaration::Type(decl)).await;
        let what = format!("redeclaring `memo` {what}");
        refused_unmoved(store.as_ref(), before, &what, answer).await?;
    }

    let mut added = first;
    added.fields.push(field("tags", StorageKind::Json));
    l.declare(added).await?;
    Ok(())
}

async fn declare_refuses_bad_names_unknown_references_and_tool_name_collisions(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let reference = FieldDecl {
        ty: FieldType {
            target: Some("nowhere".into()),
            ..FieldType::kind(StorageKind::Reference)
        },
        ..field("link", StorageKind::Text)
    };
    let cases = [
        ("a type named `Bad-Name`", type_decl("Bad-Name", vec![])),
        (
            "a field named `two words`",
            type_decl("note", vec![field("two words", StorageKind::Text)]),
        ),
        (
            "a reference to the undeclared type `nowhere`",
            type_decl("link", vec![reference]),
        ),
    ];
    let cases: Vec<(&str, Declaration)> = cases
        .into_iter()
        .map(|(what, decl)| (what, Declaration::Type(decl)))
        .collect();
    l.declare(type_decl("memo", vec![field("title", StorageKind::Text)]))
        .await?;
    let verb = |name: &str| -> Result<Declaration, Failure> {
        serde_json::from_value(json!({
            "declare": "verb", "name": name, "on": "memo", "risk": "low",
            "effect": "write", "set": [{"field": "title", "value": "'x'"}]
        }))
        .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))
    };
    // The colliding names come from the one generator, never spelled here.
    let generic = GENERIC_TOOLS[0];
    let described = cat.describe(own, ReadAt::default());
    let described = described.await.must("describing the lake")?.data;
    let find_tool = tools(&described)
        .map_err(|p| Failure::new(format!("the lake's names cannot become tools: {p:?}")))?
        .into_iter()
        .find(|t| t.door == Door::Find { ty: "memo".into() })
        .map(|t| t.name)
        .ok_or_else(|| Failure::new("the owner has no find tool for `memo`"))?;
    let generic_what = format!("a verb named `{generic}`, which is a generic tool");
    let find_what = format!("a verb named `{find_tool}`, which is the find tool of `memo`");
    let mut cases: Vec<(String, Declaration)> = cases
        .into_iter()
        .map(|(what, decl)| (what.to_owned(), decl))
        .collect();
    cases.push((generic_what, verb(generic)?));
    cases.push((find_what, verb(&find_tool)?));
    for (what, decl) in cases {
        let before = store.head().await.must("reading the head")?;
        let answer = cat.declare(own, decl).await;
        let err =
            refused_unmoved(store.as_ref(), before, &format!("declaring {what}"), answer).await?;
        ensure!(
            matches!(err, Error::Refused { .. } | Error::NotFound { .. }),
            "declaring {what} must be refused; it failed with {err:?}"
        );
    }
    Ok(())
}

check!(
    DERIVED_NO_TOOL_NAME = a_derived_relation_takes_no_tool_name,
    "014 and 044 (lakelands#902): a derived relation has no link door, so it takes no tool name: a type whose derived relation would make a link door's name too long is declared where the same relation carried by link records is refused, a verb named as the link door or the unlink door of a derived relation would be is declared, after its type or before it, and the lake's catalog is answered each time, with the verb and no link door"
);

async fn a_derived_relation_takes_no_tool_name(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("payroll", vec![field("code", StorageKind::Text)]))
        .await?;
    let ty_of = |name: &str, relation: &str, derived: bool| -> Result<TypeDecl, Failure> {
        let mut r = json!({"name": relation, "target": "payroll"});
        if derived {
            r["derived"] = json!({"match": [{"from": "ref", "to": "code"}]});
        }
        let fields = json!([{"name": "ref", "type": {"kind": "text"}}]);
        serde_json::from_value(json!({"name": name, "fields": fields, "relations": [r]}))
            .map_err(|e| Failure::new(format!("the check's type does not read: {e}")))
    };
    let link_door = |t: &ToolSpec| matches!(t.door, Door::Link { .. } | Door::Unlink { .. });
    // Three characters, the relation and the unlink door's suffix: one
    // character more than a tool name may have.
    let long = "m".repeat(MAX_TOOL_NAME - 10);
    let before = store.head().await.must("reading the head")?;
    let answer = l.try_declare(ty_of("inl", &long, false)?).await;
    let what = "declaring a relation carried by links whose unlink door's name is too long";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(invalid_case(&err), Some(Case::ToolNames { .. })),
        "{what} must be refused for its tool names; it failed with {err:?}"
    );
    l.declare(ty_of("inw", &long, true)?).await?;
    l.declare(ty_of("inv", "matched", true)?).await?;
    let described = l.describe().await?;
    let offered = tools(&described).map_err(|p| {
        Failure::new(format!(
            "a lake with a derived relation of a long name must have its catalog; it has {p:?}"
        ))
    })?;
    ensure!(
        !offered.iter().any(link_door),
        "a derived relation must have no link door; the catalog offers {:?}",
        offered.iter().filter(|t| link_door(t)).collect::<Vec<_>>()
    );
    // The name a link door of a derived relation would have, from the one
    // generator: `inv` described as if `matched` were carried by links, and
    // the same type under the name `iny`, which is not declared yet.
    let mut linked = described.clone();
    linked.types.retain(|t| t.name.as_str() == "inv");
    linked.types[0].relations[0].derived = None;
    linked.types[0].may.link = vec!["matched".into()];
    linked.types.push(linked.types[0].clone());
    linked.types[1].name = "iny".into();
    let doors = tools(&linked).unwrap_or_default();
    // The link door's name after its type, the unlink door's before it.
    for (ty, first) in [("inv", false), ("iny", true)] {
        let (ty_, relation) = (ty.into(), "matched".into());
        let door = match first {
            true => Door::Unlink { ty: ty_, relation },
            false => Door::Link { ty: ty_, relation },
        };
        let name = doors
            .iter()
            .find(|t| t.door == door)
            .map(|t| t.name.clone());
        let name = name.ok_or_else(|| Failure::new("no link door to name"))?;
        let verb = json!({"declare": "verb", "name": name, "on": "payroll", "risk": "low",
                          "effect": "write", "set": [{"field": "code", "value": "'x'"}]});
        l.declare_json(verb).await?;
        if first {
            l.declare(ty_of(ty, "matched", true)?).await?;
        }
        let offered = tools(&l.describe().await?).map_err(|p| {
            Failure::new(format!(
                "a verb named as a link door of `{ty}.matched`, a derived relation, would be \
                 is declared, so the lake must have its catalog; it has {p:?}"
            ))
        })?;
        let call = Door::Call {
            verb: name.as_str().into(),
        };
        ensure!(
            offered.iter().any(|t| t.name == name && t.door == call)
                && !offered.iter().any(link_door),
            "the catalog must offer the verb `{name}` and no link door; it offers {:?}",
            offered.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }
    Ok(())
}

async fn an_ordinary_principal_cannot_declare_a_sys_type(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let mine = type_decl("sys.mine", vec![field("v", StorageKind::Text)]);
    let answer = cat.declare(own, Declaration::Type(mine)).await;
    let what = "the owner declaring `sys.mine`";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(reason(&err), Some(Refusal::Reserved)),
        "declaring `sys.mine` as the owner must be refused as reserved; it failed with {err:?}"
    );

    Ok(())
}

check!(
    REORDERED_FIELDS = redeclaring_a_type_with_its_fields_reordered_keeps_its_schema_hash,
    "007: field order is not shape, so a type declared again with its fields in another order keeps its schema hash; the log keeps the declaration as written, so it is one revision"
);

async fn redeclaring_a_type_with_its_fields_reordered_keeps_its_schema_hash(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let first = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    l.declare(first.clone()).await?;
    let (hash, version) = hash_of(store.as_ref(), "memo").await?;
    let before = store.head().await.must("reading the head")?;
    let mut reordered = first;
    reordered.fields.reverse();
    l.declare(reordered).await?;
    let after = store.head().await.must("reading the head")?;
    let (again, next) = hash_of(store.as_ref(), "memo").await?;
    ensure!(
        again == hash,
        "redeclaring `memo` with its fields reordered moved its schema hash from {hash} \
         to {again}; order is not shape"
    );
    ensure!(
        after == before.next() && next == version + 1,
        "the reordered declaration is a revision as written: one event and version \
         {} expected, but the head moved from {before} to {after} and the version is {next}",
        version + 1
    );
    Ok(())
}

check!(
    IN_FORCE_APPENDS_NOTHING = a_declaration_that_is_the_one_in_force_appends_nothing,
    "027, extended (lakelands#565): a declaration that is the one in force (a value type, a function, a type, a verb, a trigger, a set, and a type whose computed field is stated with its completed `depends_on` or without) is answered `applied` at the head and appends nothing, by `declare` and by `apply`, and its version stays; a refusal still comes first; of a set what differs is appended alone, and what it sets off happens; an accepted proposal of declarations in force appends its acceptance alone"
);

async fn a_declaration_that_is_the_one_in_force_appends_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let scaled = |by: u8| -> Declaration {
        serde_json::from_value(json!({"declare": "function", "name": "scaled", "backing": "expr",
            "inputs": [{"name": "net", "type": {"kind": "decimal"}}], "expr": format!("net * {by}")}))
        .expect("the check's function reads")
    };
    let bill = |deps: &[&str]| {
        let net = field("net", StorageKind::Decimal);
        let gross = computed_by("gross", StorageKind::Decimal, "scaled", deps);
        Declaration::Type(type_decl("bill", vec![net, gross]))
    };
    let all = vec![
        Declaration::ValueType(value_type("state", StorageKind::Text)),
        scaled(2),
        bill(&[]),
        write_verb("zero", "bill", "net", "0"),
        trigger_on("noted", "bill", None),
        object_set_on("big", "bill", "net > 1"),
    ];
    for d in &all {
        l.declare_as(d.clone(), "declaring the first time").await?;
    }
    l.upsert("bill", "b", json!({"net": "3"})).await?;
    let before = store.head().await.must("reading the head")?;
    // Each again, the type also with the list `describe` shows, then all as a set.
    let mut got = Vec::new();
    for d in all.iter().chain(&[bill(&["net"])]) {
        got.push(l.declare_as(d.clone(), "declaring again").await?);
    }
    got.push(l.cat.apply(&l.own, all.clone()).await.must("applying")?);
    got.retain(|o| !matches!(o, Outcome::Applied { position, .. } if *position == before));
    let after = store.head().await.must("reading the head")?;
    let version = l.describe_type("bill").await?.lifecycle.version;
    ensure!(
        got.is_empty() && after == before && version == 1,
        "a declaration in force is answered `applied` at the head {before} and appends \
         nothing; the head is {after}, `bill` stands at version {version}, and {got:?}"
    );
    // A refusal comes first: without the `schema` grant it is not told.
    let unschooled = reviewer(store.as_ref(), false);
    let refused = l.cat.declare(&unschooled, bill(&[])).await;
    ensure!(
        matches!(reason(&refused), Some(Refusal::NotGranted)),
        "a declaration in force by one without the `schema` grant answered {refused:?}"
    );
    // Of a set, what differs is appended alone, and the type that reads the
    // changed function is computed again.
    let set = vec![scaled(3), bill(&[])];
    l.cat.apply(&l.own, set).await.must("applying the set")?;
    let written = events_after(store.as_ref(), before).await?;
    let gross = decimal(value(&l.read("bill", "b").await?, "gross"));
    ensure!(
        written.len() == 1 && gross == Some(dec("9")),
        "a set of a changed function and an unchanged type appends the function alone, \
         and `gross` is computed again as 9; it reads {gross:?} after {written:?}"
    );
    // An accepted proposal of declarations in force: its acceptance alone.
    let declarations = vec![scaled(3), bill(&["net"])];
    let proposed = ProposedWrite::Declare { declarations };
    let proposed = l.cat.propose(&l.own, proposed, "again".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing")? else {
        return Err(Failure::new("a proposed set is not answered as proposed"));
    };
    l.cat.accept(&l.own, &proposal).await.must("accepting")?;
    let written = events_after(store.as_ref(), before).await?;
    ensure!(
        written.len() == 3 && matches!(written[2].body, EventBody::WriteAccepted { .. }),
        "an accepted proposal of declarations in force appends `write_accepted` alone; \
         after the function's, the log holds {:?}",
        &written[1..]
    );
    Ok(())
}

check!(
    MIGRATION_IN_FORCE = a_migration_to_what_is_in_force_appends_nothing,
    "027, extended (lakelands#577): a migration whose `to` is the type in force and that renames nothing is answered `applied` at the head and appends nothing; a stale report is refused first; an accepted proposal of it appends its acceptance alone; a value type's migration to itself is refused as before, there being nothing to migrate"
);

async fn a_migration_to_what_is_in_force_appends_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (bill, state) = (
        type_decl("bill", vec![field("net", StorageKind::Decimal)]),
        value_type("state", StorageKind::Text),
    );
    l.declare(bill.clone()).await?;
    l.declare_as(Declaration::ValueType(state.clone()), "declaring `state`")
        .await?;
    l.upsert("bill", "b", json!({"net": "3"})).await?;
    let before = store.head().await.must("reading the head")?;
    let stale = l.cat.migrate_apply(&l.own, migration(bill.clone()), "none");
    let (stale, typed) = (
        stale.await,
        l.cat.migrate_value_type(&l.own, state.into()).await,
    );
    let got = l.migrate(bill.clone()).await?;
    let (after, version) = (
        store.head().await.must("reading the head")?,
        l.describe_type("bill").await?.lifecycle.version,
    );
    ensure!(
        is_invalid(&stale)
            && is_invalid(&typed)
            && matches!(got, Outcome::Applied { position, .. } if position == before)
            && (after, version) == (before, 1),
        "a migration to the type in force is answered `applied` at the head {before} and \
         appends nothing, a stale report and a value type's migration to itself refused; \
         the head is {after}, `bill` at version {version}, and they answered {stale:?}, \
         {typed:?} and {got:?}"
    );
    // Proposed and accepted: the proposal and its acceptance, nothing more.
    let (migration, report) = (Box::new(migration(bill)), Default::default());
    let write = ProposedWrite::Migrate { migration, report };
    let proposed = l.cat.propose(&l.own, write, "again".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing")? else {
        return Err(Failure::new(
            "a proposed migration is not answered as proposed",
        ));
    };
    l.cat.accept(&l.own, &proposal).await.must("accepting")?;
    let written = events_after(store.as_ref(), before).await?;
    ensure!(
        written.len() == 2 && matches!(written[1].body, EventBody::WriteAccepted { .. }),
        "an accepted proposal of a migration to the type in force appends `write_accepted` \
         alone; the log holds {written:?}"
    );
    Ok(())
}

check!(
    RECOMPUTED = the_door_computes_again_what_a_declaration_in_force_computes,
    "022, extended (lakelands#565): `recompute` appends the declaration in force again as it was declared, at its next version; a function that reads a setting is computed again with the setting as it stands, where declaring it again changes nothing; called again it changes no record and still appends; without the `schema` grant it is not granted before the name is looked at, a name no function or type has and a type its caller is not shown are not found, a view and a type with no field computed outside the lake are invalid, and nothing is appended"
);

async fn the_door_computes_again_what_a_declaration_in_force_computes(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let rated: Declaration = serde_json::from_value(json!({"declare": "function", "name": "rated",
        "backing": "expr", "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
        "expr": "net * setting('rate')"}))
    .expect("the check's function reads");
    l.set("rate", json!("2")).await?;
    l.declare_as(rated.clone(), "declaring `rated`").await?;
    let gross = computed_by("gross", StorageKind::Decimal, "rated", &[]);
    l.declare(type_decl(
        "bill",
        vec![field("net", StorageKind::Decimal), gross],
    ))
    .await?;
    let n = vec![field("n", StorageKind::Number)];
    l.declare(view_of("bills", "bill", &[], n, &[("n", "count()")]))
        .await?;
    l.upsert("bill", "b", json!({"net": "3"})).await?;
    l.set("rate", json!("5")).await?;
    l.declare_as(rated.clone(), "declaring `rated` again")
        .await?;
    let before = store.head().await.must("reading the head")?;
    let mut read = Vec::new();
    for _ in 0..3 {
        read.push(decimal(value(&l.read("bill", "b").await?, "gross")));
        let what = Recompute::Function("rated".into());
        l.cat.recompute(&l.own, what).await.must("the door")?;
    }
    let written = events_after(store.as_ref(), before).await?;
    let as_declared = |(e, v): (&lakelands_api::event::Event, u32)| {
        matches!(&e.body, EventBody::Declared { declaration, version, .. }
            if *declaration == rated && *version == v)
    };
    ensure!(
        read == [dec("6"), dec("15"), dec("15")].map(Some)
            && written.len() == 3
            && written.iter().zip(2..).all(as_declared),
        "`gross` is 6 while `rated` is only declared again, and 15 once the door computed it \
         with the setting as it stands; each call appends `rated` as it was declared, at \
         versions 2 to 4; it read {read:?} and the log holds {written:?}"
    );
    // The refusals, in their order; nothing is appended.
    let before = store.head().await.must("reading the head")?;
    let (unschooled, blind) = (
        reviewer(store.as_ref(), false),
        principal(
            store.as_ref(),
            PrincipalKind::Agent,
            "blind",
            Grants {
                schema: true,
                ..Grants::default()
            },
        ),
    );
    let (function, ty) = (
        |n: &str| Recompute::Function(n.into()),
        |n: &str| Recompute::Type(n.into()),
    );
    let mut red = Vec::new();
    for (what, who, named, want) in [
        (
            "without the `schema` grant",
            &unschooled,
            function("none"),
            "not granted",
        ),
        (
            "a name no function has",
            &l.own,
            function("none"),
            "not found",
        ),
        ("a name no type has", &l.own, ty("none"), "not found"),
        (
            "a type its caller is not shown",
            &blind,
            ty("bill"),
            "not found",
        ),
        (
            "a type computed inside the lake",
            &l.own,
            ty("bill"),
            "invalid",
        ),
        ("a view", &l.own, ty("bills"), "invalid"),
    ] {
        let got = l.cat.recompute(who, named).await;
        let is = match &got {
            Err(Error::NotFound { .. }) => "not found",
            _ if matches!(reason(&got), Some(Refusal::NotGranted)) => "not granted",
            _ if is_invalid(&got) => "invalid",
            _ => "something else",
        };
        if is != want {
            red.push(format!("{what} must be {want} and answered {got:?}"));
        }
    }
    ensure!(red.is_empty(), "the door's refusals: {}", red.join("; "));
    head_unmoved(store.as_ref(), before, "the door").await
}

async fn add_field_without_the_schema_grant_is_refused_before_it_looks_at_the_type(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let writer = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "writer",
        Grants {
            schema: false,
            ..Grants::owner()
        },
    );
    let before = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for (what, ty) in [
        ("an undeclared type", "nothing"),
        ("a field `note` has", "note"),
    ] {
        let got = l
            .cat
            .add_field(
                &writer,
                &TypeName::from(ty),
                field("text", StorageKind::Text),
            )
            .await;
        if !matches!(reason(&got), Some(Refusal::NotGranted)) {
            red.push(format!("add_field of {what} answered {got:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "add_field by a principal without the schema grant is refused not_granted before \
         the type is looked up: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), before, "add_field without the schema grant").await
}
