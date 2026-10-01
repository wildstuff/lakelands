//! `describe` answers the declaration whole
//! (`docs/decisions/014-four-doors-per-type-one-name-generator.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
//!
//! A host redeclares a type, or lists what is in force, by reading
//! `describe`, not by replaying the feed. So for a principal who reads a
//! field, its description carries every member of its declaration, and
//! a type's description carries every member of the type's: projected
//! back, it is the declaration, byte for byte. A masked field shows its
//! shape only. Functions, triggers, object sets and views are answered as
//! declared, each to a principal who reads every field it names or holds
//! the `schema` grant, as they stand at the read position.

use std::collections::BTreeMap;

use lakelands_api::describe::{FieldDescription, LakeDescription, TypeDescription};
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{
    Authority, EmbedDecl, FieldDecl, FieldType, Measure, StorageKind, TriggerDecl, TypeDecl,
    ViewDecl,
};
use lakelands_api::write::{Declaration, FunctionDecl, ObjectSetDecl};
use std::sync::Arc;

use lakelands_api::principal::TypeGrants;
use lakelands_api::{
    Access, Catalog, FieldName, Grants, LakeStore, Principal, PrincipalKind, ReadAt, TypeName,
};

use crate::conformance::Failure;
use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{Must, doors, field, owner, principal, type_decl};

check!(
    /// `describe` answers the declaration whole.
    DESCRIBE_IS_THE_DECLARATION = describe_answers_the_declaration_whole,
    "014 and 024: for the owner, a described type projects back onto its declaration byte for byte — every field's function, embedding, searchable, sensitive, label and description, and the type's label, language and view; a masked field shows its shape only; functions, triggers, object sets and views are answered as declared, to a principal who reads every field they name or holds the schema grant, as in force at the read position",
    embedding
);

fn described(name: &str, label: &str, kind: StorageKind) -> FieldDecl {
    FieldDecl {
        label: Some(label.into()),
        description: Some(format!("the ledger's {name}")),
        ..field(name, kind)
    }
}

/// The ledger: a field of every kind of member.
fn ledger() -> TypeDecl {
    let mut title = described("title", "Title", StorageKind::Text);
    title.searchable = true;
    title.required = true;
    let mut body = described("body", "Body", StorageKind::Text);
    body.searchable = true;
    let net = described("net", "Net", StorageKind::Number);
    let mut gross = described("gross", "Gross", StorageKind::Number);
    gross.authority = Authority::Derived;
    gross.function = Some("scaled".into());
    let mut doubled = described("doubled", "Doubled", StorageKind::Number);
    doubled.authority = Authority::Derived;
    doubled.derived = Some(Condition::from("net + net"));
    let mut vec = described("vec", "Vector", StorageKind::Vector);
    vec.ty = FieldType {
        dimensions: Some(2),
        ..FieldType::kind(StorageKind::Vector)
    };
    vec.authority = Authority::Derived;
    vec.embed = Some(EmbedDecl {
        from: vec![title.name.clone(), body.name.clone()],
        model: EMBED_MODEL.into(),
    });
    let mut salary = described("salary", "Salary", StorageKind::Number);
    salary.sensitive = true;
    let mut bonus = described("bonus", "Bonus", StorageKind::Number);
    bonus.authority = Authority::Derived;
    bonus.function = Some("tenth".into());
    TypeDecl {
        language: Some("en".into()),
        label: Some("Ledger".into()),
        description: Some("One line of the books".into()),
        ..type_decl(
            "ledger",
            vec![title, body, net, gross, doubled, vec, salary, bonus],
        )
    }
}

/// A view over the ledger, grouped by title, of one measure.
fn by_title(name: &str, measure: &str, over: &str) -> TypeDecl {
    let mut ty = type_decl(
        name,
        vec![
            field("title", StorageKind::Text),
            field(measure, StorageKind::Number),
        ],
    );
    ty.key = vec![FieldName::from("title")];
    ty.view = Some(ViewDecl {
        source: TypeName::from("ledger"),
        r#where: None,
        group_by: vec![FieldName::from("title")],
        measures: vec![Measure {
            name: FieldName::from(measure),
            expr: Condition::from(format!("sum({over})").as_str()),
        }],
    });
    ty.description = Some(format!("the ledger's {over} by title"));
    ty
}

fn function(name: &str, input: &str, expr: &str) -> FunctionDecl {
    FunctionDecl {
        name: name.into(),
        backing: "expr".into(),
        inputs: vec![field(input, StorageKind::Number)],
        expr: Some(Condition::from(expr)),
        config: serde_json::Value::Null,
        description: Some(format!("{name} of {input}")),
    }
}

fn trigger(name: &str, when: &str) -> TriggerDecl {
    TriggerDecl {
        max_depth: None,
        name: name.into(),
        on: "ledger".into(),
        when: Some(Condition::from(when)),
        description: Some(format!("fires on {when}")),
    }
}

fn object_set(name: &str, filter: &str) -> ObjectSetDecl {
    ObjectSetDecl {
        name: name.into(),
        of: "ledger".into(),
        filter: Condition::from(filter),
        description: Some(format!("the lines where {filter}")),
    }
}

/// A described type projected back onto the declaration it describes.
fn projected(t: &TypeDescription) -> TypeDecl {
    TypeDecl {
        annotations: t.annotations.clone(),
        name: t.name.clone(),
        key: t.access.key.clone(),
        fields: t.fields.iter().map(field_projected).collect(),
        relations: t.relations.clone(),
        view: t.view.clone(),
        valid_from: t.access.valid_from.clone(),
        valid_to: t.access.valid_to.clone(),
        language: t.language.clone(),
        field_meta: t.access.field_meta,
        description: t.description.clone(),
        label: t.label.clone(),
    }
}

fn field_projected(f: &FieldDescription) -> FieldDecl {
    FieldDecl {
        annotations: f.annotations.clone(),
        name: f.name.clone(),
        ty: f.ty.clone(),
        required: f.required,
        authority: f.authority,
        derived: f.derived.clone(),
        function: f.function.clone(),
        depends_on: f.depends_on.clone(),
        on_input_change: f.on_input_change,
        sensitive: f.sensitive,
        searchable: f.searchable,
        description: f.description.clone(),
        label: f.label.clone(),
        embed: f.embed.clone(),
    }
}

fn bytes<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("<unserialisable: {e}>"))
}

/// The same declaration, byte for byte, or a failure naming both.
fn same<T: serde::Serialize>(what: &str, got: &T, declared: &T) -> Result<(), Failure> {
    let (got, declared) = (bytes(got), bytes(declared));
    ensure!(
        got == declared,
        "{what}: describe answered\n  {got}\nwhere it should answer\n  {declared}"
    );
    Ok(())
}

fn names<T>(items: &[T], name: impl Fn(&T) -> String) -> Vec<String> {
    items.iter().map(name).collect()
}

fn one_type<'a>(d: &'a LakeDescription, name: &str) -> Result<&'a TypeDescription, Failure> {
    d.types
        .iter()
        .find(|t| t.name.as_str() == name)
        .ok_or_else(|| Failure::new(format!("describe does not list type `{name}`")))
}

/// Reads everything but the ledger's salary, and holds no schema grant.
fn clerk(store: &dyn LakeStore) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    grants.types.insert(
        TypeName::from("ledger"),
        TypeGrants {
            default: None,
            fields: BTreeMap::from([(FieldName::from("salary"), Access::None)]),
        },
    );
    principal(store, PrincipalKind::Agent, "clerk", grants)
}

async fn describe_at(
    cat: &dyn Catalog,
    who: &Principal,
    at: ReadAt,
    what: &str,
) -> Result<LakeDescription, Failure> {
    Ok(cat.describe(who, at).await.must(what)?.data)
}

async fn describe_answers_the_declaration_whole(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let store = store.as_ref();
    let cat = doors(store);
    let cat = cat.as_ref();
    let own = owner(store);
    let first = function("scaled", "net", "net * 2");
    let tenth = function("tenth", "salary", "salary / 10");
    let net_changed = trigger("net_changed", "changed(net)");
    let rich = trigger("rich", "salary > 1000");
    let big = object_set("big", "net > 10");
    let paid_well = object_set("paid_well", "salary > 100");
    let ledger = ledger();
    let net_view = by_title("net_by_title", "net_total", "net");
    let pay_view = by_title("pay_by_title", "pay_total", "salary");
    for d in [
        Declaration::Function(first.clone()),
        Declaration::Function(tenth.clone()),
        Declaration::Type(ledger.clone()),
        Declaration::Type(net_view.clone()),
        Declaration::Type(pay_view.clone()),
        Declaration::Trigger(net_changed.clone()),
        Declaration::Trigger(rich.clone()),
        Declaration::ObjectSet(big.clone()),
        Declaration::ObjectSet(paid_well.clone()),
    ] {
        cat.declare(&own, d)
            .await
            .must("declaring the ledger's ontology")?;
    }
    let earlier = store.head().await.must("reading the head")?;
    let again = function("scaled", "net", "net * 3");
    cat.declare(&own, Declaration::Function(again.clone()))
        .await
        .must("declaring `scaled` again")?;

    // The owner, at the head: every declaration whole.
    let d = describe_at(cat, &own, ReadAt::default(), "describing as the owner").await?;
    for decl in [&ledger, &net_view, &pay_view] {
        let t = one_type(&d, decl.name.as_str())?;
        same(&format!("the owner's `{}`", decl.name), &projected(t), decl)?;
        let alone = cat
            .describe_type(&own, &decl.name, ReadAt::default())
            .await
            .must("describing one type as the owner")?
            .data;
        same(
            &format!("the owner's describe_type `{}`", decl.name),
            &projected(&alone),
            decl,
        )?;
    }
    same(
        "the owner's functions at the head",
        &d.functions,
        &vec![again.clone(), tenth.clone()],
    )?;
    same(
        "the owner's triggers",
        &d.triggers,
        &vec![net_changed.clone(), rich.clone()],
    )?;
    same(
        "the owner's object sets",
        &d.object_sets,
        &vec![big.clone(), paid_well.clone()],
    )?;

    // The owner, before `scaled` was declared again: the first one.
    let then = describe_at(
        cat,
        &own,
        ReadAt {
            as_of: Some(earlier),
            valid_at: None,
        },
        "describing as the owner as_of an earlier position",
    )
    .await?;
    same(
        &format!("the owner's functions as_of {earlier}"),
        &then.functions,
        &vec![first.clone(), tenth.clone()],
    )?;

    // The clerk: the salary masked to its shape, and nothing that names it.
    let who = clerk(store);
    let c = describe_at(cat, &who, ReadAt::default(), "describing as the clerk").await?;
    let t = one_type(&c, "ledger")?;
    for f in &t.fields {
        let decl = ledger
            .fields
            .iter()
            .find(|d| d.name == f.name)
            .ok_or_else(|| Failure::new(format!("describe invents field `{}`", f.name)))?;
        if f.name.as_str() == "salary" {
            let shape = FieldDescription {
                name: decl.name.clone(),
                ty: decl.ty.clone(),
                required: decl.required,
                authority: decl.authority,
                access: Access::Masked,
                on_input_change: decl.on_input_change,
                sensitive: true,
                ..Default::default()
            };
            same("the clerk's masked `salary`", f, &shape)?;
        } else {
            same(
                &format!("the clerk's `{}`", f.name),
                &field_projected(f),
                decl,
            )?;
        }
    }
    ensure!(
        t.fields.len() == ledger.fields.len(),
        "the clerk sees {} of the ledger's {} fields; each is read or masked",
        t.fields.len(),
        ledger.fields.len()
    );
    same(
        "the clerk's functions: `tenth` reads the salary",
        &names(&c.functions, |f| f.name.clone()),
        &vec!["scaled".to_owned()],
    )?;
    same(
        "the clerk's triggers: `rich` names the salary",
        &names(&c.triggers, |t| t.name.to_string()),
        &vec!["net_changed".to_owned()],
    )?;
    same(
        "the clerk's object sets: `paid_well` names the salary",
        &names(&c.object_sets, |s| s.name.clone()),
        &vec!["big".to_owned()],
    )?;
    same(
        "the clerk's view over the net",
        &one_type(&c, "net_by_title")?.view,
        &net_view.view,
    )?;
    same(
        "the clerk's view over the salary",
        &one_type(&c, "pay_by_title")?.view,
        &None,
    )?;
    Ok(())
}
