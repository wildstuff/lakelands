//! What a principal may not read, on every read and in the tools
//! (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult, Search, SearchMode};
use lakelands_api::read::{AggFunc, Aggregate, Direction, OrderBy};
use lakelands_api::tools::{Door, tools};
use lakelands_api::{
    Access, FieldName, Find, Grants, LakeStore, PrincipalKind, ReadAt, Refusal, TypeName, WriteOpts,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, agg, all_found, derived, embedding, field, field_access, field_writer, find_where,
    ingest, no_such_field, order, patch, pattern, principal, read, refused_on, source_record,
    sourced, type_decl,
};

check!(
    /// A masked field is named; an ungranted one is not there at all.
    MASKED_AND_ABSENT = a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent,
    "006 and 014: a field the principal may not read is masked (named, no value) or absent from get, find, describe and the tools' schemas"
);

check!(
    /// A field built from a hidden one is hidden as it is.
    BUILT_FROM_HIDDEN = a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field,
    "024 and 006: a derived field, one derived from it in turn, and a vector embedded from a field are read as the field they are built from: absent where it is absent and masked where it is masked, on get, find, a pattern's rows and describe, which shows a definition only where every field it names is read; a filter, an order or an aggregate over them is refused naming them, as not granted where they are masked and as for an undeclared field where they are hidden",
    embedding
);

async fn a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
        let rows = cat
            .query(
                &who,
                pattern(
                    "emp",
                    Some("e1"),
                    Vec::new(),
                    Projection::Rows {
                        fields: vec!["emp.g".into(), "emp.h".into(), "emp.k".into()],
                    },
                ),
            )
            .await
            .must("the clerk's pattern rows over `emp`")?
            .data;
        let QueryResult::Rows { rows, .. } = rows else {
            return Err(Failure::new("a pattern's rows answered something else"));
        };
        let row: Vec<&String> = rows.iter().flat_map(|r| r.keys()).collect();
        ensure!(
            row == ["emp.k"],
            "the clerk's pattern rows hold `k` only, not `g` or `h`; they held {row:?}"
        );
        let d = cat
            .describe_type(&who, &"emp".into(), ReadAt::default())
            .await
            .must("the clerk describing `emp`")?
            .data;
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
        let probes = [
            ("a filter on `g`", "g", find_where("g > 0")),
            (
                "an order by `h`",
                "h",
                Find {
                    order_by: vec![order("h", OrderBy::Value, Direction::Asc)],
                    ..Find::default()
                },
            ),
            (
                "a sum of `g`",
                "g",
                Find {
                    aggregate: Some(Aggregate {
                        measures: vec![agg("s", AggFunc::Sum, Some("g"))],
                        group_by: Vec::new(),
                    }),
                    ..Find::default()
                },
            ),
        ];
        for (what, name, find) in probes {
            let r = cat.find(&who, &"emp".into(), find).await;
            let answered = match access {
                Access::None => no_such_field(&r, name),
                _ => matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == name),
            };
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
    /// A field the principal may not know of is answered as one never declared.
    NONE_HIDES = a_field_the_principal_may_not_know_of_is_answered_as_one_never_declared,
    "006 and 024: a field whose grant is none by name, sensitive or not, does not exist for that principal: it is absent from get, find, describe (a relation's link field too) and the tools, a search that names no fields never searches it, and a filter, an order, a pattern's `where`, a search's `fields`, an upsert and an ingest that name it are answered as for a field the type does not declare; a sensitive field whose none is inherited is listed as masked, and a filter on it is refused as not granted"
);

async fn a_field_the_principal_may_not_know_of_is_answered_as_one_never_declared(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let searchable = |name: &str| FieldDecl {
        searchable: true,
        ..field(name, StorageKind::Text)
    };
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
    let d = cat
        .describe_type(&by_name, &staff, ReadAt::default())
        .await
        .must("describing `staff`")?
        .data;
    let lake = cat
        .describe(&by_name, ReadAt::default())
        .await
        .must("describing the lake")?
        .data;
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
    for f in ["plain", "secret", "nosuch"] {
        let in_fields = |text: &str| Search {
            ty: staff.clone(),
            mode: SearchMode::Lexical {
                text: text.into(),
                fields: Some(vec![f.into()]),
            },
            k: 5,
            cursor: None,
            filter: None,
            with_meta: false,
            at: ReadAt::default(),
        };
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
                cat.search(&by_name, in_fields("pp")).await.map(drop),
            ),
            (
                "an upsert",
                cat.upsert(
                    &by_name,
                    &staff,
                    &"s1".into(),
                    patch(json!({f: "x"})),
                    WriteOpts::default(),
                )
                .await
                .map(drop),
            ),
        ];
        for (what, r) in answers {
            ensure!(
                no_such_field(&r, f),
                "{what} naming `{f}` is answered as for a field `staff` does not declare; it \
                 answered {r:?}"
            );
        }
    }
    let fed = ingest(
        cat.as_ref(),
        &by_name,
        "crm",
        vec![source_record("staff", "s2", json!({"feed": "x"}))],
    )
    .await;
    ensure!(
        no_such_field(&fed, "feed"),
        "an ingest of `feed` is answered as for a field `staff` does not declare; it answered \
         {fed:?}"
    );
    let unnamed = |who| {
        let s = Search {
            ty: staff.clone(),
            mode: SearchMode::lexical("ss pp ann"),
            k: 5,
            cursor: None,
            filter: None,
            with_meta: false,
            at: ReadAt::default(),
        };
        cat.search(who, s)
    };
    let mine = unnamed(own).await.must("the owner's search")?.data;
    let theirs = unnamed(&by_name)
        .await
        .must("a search naming no fields")?
        .data;
    let score = |h: &lakelands_api::query::Hits| h.hits.iter().map(|h| h.score).collect::<Vec<_>>();
    ensure!(
        score(&mine) == [3.0] && score(&theirs) == [1.0],
        "a search naming no fields reads only the searchable fields the principal knows of: \
         the owner's `ss pp ann` scores 3, theirs only `ann`, 1; they scored {:?} and {:?}",
        score(&mine),
        score(&theirs)
    );
    let mut inherited = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    inherited.types.insert(
        staff.clone(),
        lakelands_api::principal::TypeGrants {
            default: Some(Access::None),
            fields: [("name".into(), Access::Read)].into(),
        },
    );
    let by_default = principal(store.as_ref(), PrincipalKind::Agent, "y", inherited);
    let got = read(cat.as_ref(), &by_default, "staff", "s1").await?;
    let secret = cat
        .find(&by_default, &staff, find_where("secret = 'x'"))
        .await;
    ensure!(
        got.masked.iter().map(FieldName::as_str).eq(["secret"])
            && matches!(refused_on(&secret), Some(("secret", Refusal::NotGranted))),
        "a sensitive field nobody named is listed as masked, and a filter on it is not \
         granted; get listed {:?} masked, the filter answered {secret:?}",
        got.masked
    );
    Ok(())
}

async fn a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    let mut clerk = field_writer(store.as_ref(), "clerk", "patient", &["name"]);
    let grants = clerk
        .grants
        .types
        .get_mut(&TypeName::from("patient"))
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

    let d = cat
        .describe_type(&clerk, &TypeName::from("patient"), ReadAt::default())
        .await
        .must("describing `patient` as the clerk")?
        .data;
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

    let lake = cat
        .describe(&clerk, ReadAt::default())
        .await
        .must("describing the lake as the clerk")?
        .data;
    let generated = tools(&lake)
        .map_err(|p| Failure::new(format!("the clerk's describe cannot become tools: {p:?}")))?;
    let get = generated
        .iter()
        .find(|t| {
            t.door
                == Door::Get {
                    ty: "patient".into(),
                }
        })
        .ok_or_else(|| Failure::new("the clerk may read `patient` but has no get tool"))?;
    let readable = &get.output_schema["properties"]["data"]["properties"]["fields"]["properties"];
    ensure!(
        readable.get("name").is_some()
            && readable.get("diagnosis").is_none()
            && readable.get("salary").is_none(),
        "the clerk's get tool must describe only the fields it reads (`name`); its \
         schema lists {readable}"
    );
    let upsert_tool = generated
        .iter()
        .find(|t| {
            t.door
                == Door::Upsert {
                    ty: "patient".into(),
                }
        })
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
