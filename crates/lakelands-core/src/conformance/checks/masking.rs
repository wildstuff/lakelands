//! What a principal may not read, on every read and in the tools
//! (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult};
use lakelands_api::read::{AggFunc, Aggregate, Direction, OrderBy};
use lakelands_api::tools::{Door, tools};
use lakelands_api::{Access, FieldName, Find, LakeStore, ReadAt, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, agg, all_found, derived, embedding, field, field_access, field_writer, find_where,
    order, pattern, read, refused_on, type_decl,
};

check!(
    /// A masked field is named; an ungranted one is not there at all.
    MASKED_AND_ABSENT = a_masked_field_is_listed_masked_and_an_ungranted_one_is_absent,
    "006 and 014: a field the principal may not read is masked (named, no value) or absent from get, find, describe and the tools' schemas"
);

check!(
    /// A field built from a hidden one is hidden as it is.
    BUILT_FROM_HIDDEN = a_field_built_from_one_the_principal_may_not_read_is_read_as_that_field,
    "024 and 006: a derived field, one derived from it in turn, and a vector embedded from a field are read as the field they are built from: absent where it is absent and masked where it is masked, on get, find and a pattern's rows, and a filter, an order or an aggregate over them is refused not_granted naming them",
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
            ensure!(
                matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == name),
                "{what} by a clerk with `salary` {access:?} is refused not_granted naming \
                 `{name}`; it answered {r:?}"
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
