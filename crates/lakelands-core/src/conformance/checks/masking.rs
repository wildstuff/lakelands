//! What a principal may not read, on every read and in the tools
//! (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Projection, QueryResult, Search, SearchMode};
use lakelands_api::read::{AggFunc, Direction, OrderBy};
use lakelands_api::tools::{Door, tools};
use lakelands_api::{Access, FieldName, Find, LakeStore, ReadAt, Refusal, Timestamp, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, all_found, derived, describe_type, embedding, field, field_access,
    field_writer, find_where, get_at, ingest, no_such_field, order, pattern, read, reads_all,
    refused_on, searchable, source_record, sourced, try_upsert, type_decl,
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
        let fields = vec!["emp.g".into(), "emp.h".into(), "emp.k".into()];
        let rows = pattern("emp", Some("e1"), Vec::new(), Projection::Rows { fields });
        let rows = cat
            .query(&who, rows)
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
                aggregate(vec![agg("s", AggFunc::Sum, Some("g"))]),
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
    let d = describe_type(cat.as_ref(), &by_name, "staff").await?;
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
        for (what, r) in answers {
            ensure!(
                no_such_field(&r, f),
                "{what} naming `{f}` is answered as for a field `staff` does not declare; it \
                 answered {r:?}"
            );
        }
    }
    let feed = vec![source_record("staff", "s2", json!({"feed": "x"}))];
    let fed = ingest(cat.as_ref(), &by_name, "crm", feed).await;
    ensure!(
        no_such_field(&fed, "feed"),
        "an ingest of `feed` is answered as for a field `staff` does not declare; it answered \
         {fed:?}"
    );
    let unnamed = |who| cat.search(who, lexical("staff", "ss pp ann", None, false));
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
    let mut by_default = reads_all(store.as_ref(), "y");
    by_default.grants.types.insert(
        staff.clone(),
        lakelands_api::principal::TypeGrants {
            default: Some(Access::None),
            fields: [("name".into(), Access::Read)].into(),
        },
    );
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

check!(
    /// A write-only field is written and never read.
    WRITE_ONLY = a_write_only_field_is_written_and_never_shown,
    "006, extended (lakelands#393): a field granted write_only is written by upsert and retire like a writable one, while get, find, query, search, describe and the tools' read schemas never show it, a filter or a search that probes it is refused not_granted naming it, and the write answers no value; a writable field built from one the principal may not read, write-only or masked, is write-only in turn: written, and probed as not granted, never as undeclared"
);

check!(
    /// A valid-time read probes the valid-time fields.
    VALID_AT_PROBES = a_read_at_a_valid_time_probes_the_types_valid_time_fields,
    "024 and 006: a read at `valid_at` filters on the type's `valid_from` and `valid_to`, so `find`, `get` and a pattern started on the type probe them as a filter would: refused not_granted naming the field where it is masked, answered as an undeclared field where it is hidden; without `valid_at` the same reads are answered"
);

async fn a_read_at_a_valid_time_probes_the_types_valid_time_fields(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    for (timed, access) in cases.into_iter().flatten() {
        let who = field_access(store.as_ref(), "clerk", "contract", &[(timed, access)]);
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
                let answered = match (at.valid_at, access) {
                    (None, _) => r.is_ok(),
                    (Some(_), Access::None) => no_such_field(&r, timed),
                    (Some(_), _) => {
                        matches!(refused_on(&r), Some((f, Refusal::NotGranted)) if f == timed)
                    }
                };
                ensure!(
                    answered,
                    "`{door}` by a clerk with `{timed}` {access:?} at valid time {:?} is \
                     answered only without a valid time, and refused naming `{timed}` with \
                     one; it answered {r:?}",
                    at.valid_at
                );
            }
        }
    }
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

    let lake = cat
        .describe(&clerk, ReadAt::default())
        .await
        .must("describing the lake as the clerk")?
        .data;
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

async fn a_write_only_field_is_written_and_never_shown(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    let searched = cat.search(&intake, search(None)).await.map(drop);
    let gist = find_where("gist = 'g'");
    let folded = cat.find(&intake, &ty, gist).await.map(drop);
    for (what, field, answer) in [
        ("a filter on `note`", "note", filtered),
        ("a search of every field", "note", searched),
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
    let lake = cat
        .describe(&intake, ReadAt::default())
        .await
        .must("describing the lake")?;
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
        },
        k: 5,
        cursor: None,
        filter: None,
        with_meta,
        at: ReadAt::default(),
    }
}
