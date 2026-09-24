//! Declarations: the schema hash, what `declare` refuses, and the `sys.`
//! reservation (`docs/decisions/003-reservation-by-namespace.md`,
//! `docs/decisions/007-schema-hash-covers-shape.md`,
//! `docs/decisions/014-four-doors-per-type-one-name-generator.md`,
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! `docs/decisions/028-authoring-is-strict-and-whole.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::tools::{Door, GENERIC_TOOLS, tools};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, LakeStore, ReadAt, Refusal, TypeName};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{Must, declare, doors, field, head_unmoved, owner, type_decl};

check!(
    /// The schema hash is the log format's, byte for byte.
    SCHEMA_HASH_VECTOR = the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins,
    "007 and the log format's vectors: a type's schema hash is SHA-256 over its canonical shape, the same on every engine"
);

check!(
    /// Meaning does not move the schema hash.
    SCHEMA_HASH_IGNORES_MEANING = describing_and_relabelling_a_type_leaves_its_schema_hash,
    "007: descriptions, labels and field order in a revision are meaning, not shape, and leave the schema hash as it was"
);

check!(
    /// A revision that reshapes records is not a declaration.
    RESHAPING_DECLARE_REFUSED = declare_refuses_a_revision_that_reshapes_stored_records,
    "027: retyping, removing or requiring a field goes through `migrate`; `declare` refuses it and appends nothing"
);

check!(
    /// `declare` refuses what cannot become part of the map.
    DECLARE_REFUSALS = declare_refuses_bad_names_unknown_references_and_tool_name_collisions,
    "028 and 014: a name that is not an ASCII identifier, a reference to an undeclared type, or a name that collides with a tool is refused whole"
);

check!(
    /// `sys.` belongs to the system.
    SYS_RESERVED = an_ordinary_principal_cannot_declare_a_sys_type,
    "003: every type name starting `sys.` is reserved; an ordinary principal, even the owner, is refused declaring one and nothing is appended"
);

const VECTOR: &str = include_str!("../../../../../docs/log-format/vectors/schema-hash.json");

async fn the_schema_hash_of_the_log_formats_vector_is_the_one_it_pins(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let vector: Value = serde_json::from_str(VECTOR)
        .map_err(|e| Failure::new(format!("the schema-hash vector does not parse: {e}")))?;
    let decl: TypeDecl = serde_json::from_value(vector["declaration"].clone())
        .map_err(|e| Failure::new(format!("the vector's declaration does not read: {e}")))?;
    let want = vector["schema_hash"].as_str().unwrap_or_default();

    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
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
    declare(cat.as_ref(), &own, invoice).await?;
    let name = decl.name.clone();
    declare(cat.as_ref(), &own, decl).await?;
    let d = cat
        .describe_type(&own, &name, ReadAt::default())
        .await
        .must(&format!("describing `{name}`"))?
        .data;
    ensure!(
        d.lifecycle.schema_hash == want,
        "`{name}` declared from the log format's vector describes schema hash {}, but \
         the vector pins {want}",
        d.lifecycle.schema_hash
    );
    Ok(())
}

async fn hash_of(store: &dyn LakeStore, ty: &str) -> Result<(String, u32), Failure> {
    let d = doors(store)
        .describe_type(&owner(store), &TypeName::from(ty), ReadAt::default())
        .await
        .must(&format!("describing `{ty}`"))?
        .data;
    Ok((d.lifecycle.schema_hash, d.lifecycle.version))
}

async fn describing_and_relabelling_a_type_leaves_its_schema_hash(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let first = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
        ],
    );
    declare(cat.as_ref(), &own, first.clone()).await?;
    let (hash, version) = hash_of(store.as_ref(), "memo").await?;

    let mut revised = first;
    revised.description = Some("A note kept for later.".into());
    revised.label = Some("Memo".into());
    revised.fields.reverse();
    revised.fields[0].description = Some("What the memo says.".into());
    revised.fields[0].label = Some("Body".into());
    declare(cat.as_ref(), &own, revised).await?;
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

async fn declare_refuses_a_revision_that_reshapes_stored_records(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let first = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    declare(cat.as_ref(), &own, first.clone()).await?;

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
        cat.declare(&own, Declaration::Type(decl))
            .await
            .must_fail(&format!("redeclaring `memo` {what}"))?;
        head_unmoved(
            store.as_ref(),
            before,
            &format!("redeclaring `memo` {what}"),
        )
        .await?;
    }

    let mut added = first;
    added.fields.push(field("tags", StorageKind::Json));
    declare(cat.as_ref(), &own, added).await?;
    Ok(())
}

async fn declare_refuses_bad_names_unknown_references_and_tool_name_collisions(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
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
    declare(
        cat.as_ref(),
        &own,
        type_decl("memo", vec![field("title", StorageKind::Text)]),
    )
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
    let described = cat
        .describe(&own, ReadAt::default())
        .await
        .must("describing the lake")?
        .data;
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
        let err = cat
            .declare(&own, decl)
            .await
            .must_fail(&format!("declaring {what}"))?;
        head_unmoved(store.as_ref(), before, &format!("declaring {what}")).await?;
        ensure!(
            matches!(err, Error::Refused { .. } | Error::NotFound { .. }),
            "declaring {what} must be refused; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn an_ordinary_principal_cannot_declare_a_sys_type(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let err = cat
        .declare(
            &own,
            Declaration::Type(type_decl("sys.mine", vec![field("v", StorageKind::Text)])),
        )
        .await
        .must_fail("the owner declaring `sys.mine`")?;
    head_unmoved(store.as_ref(), before, "the owner declaring `sys.mine`").await?;
    ensure!(
        matches!(
            err,
            Error::Refused {
                reason: Refusal::Reserved,
                ..
            }
        ),
        "declaring `sys.mine` as the owner must be refused as reserved; it failed with {err:?}"
    );

    Ok(())
}
