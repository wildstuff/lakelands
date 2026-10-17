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

use lakelands_api::describe::{FieldDescription, LakeDescription, TypeDescription};
use lakelands_api::error::Case;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{
    Authority, Derived, EmbedDecl, FieldDecl, FieldType, MatchPair, RelationDecl, StorageKind,
    TriggerDecl, TypeDecl,
};
use lakelands_api::write::{
    Declaration, FunctionDecl, Migration, ObjectSetDecl, ProposalFilter, ProposedWrite,
};

use lakelands_api::principal::TypeGrants;
use lakelands_api::{Access, Catalog, Grants, LakeStore, Principal, PrincipalKind, ReadAt};
use serde_json::json;

use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, all, describe_type, doors, field, field_access, invalid_case, owner, principal,
    type_decl, upsert, view_of,
};
use crate::conformance::{Failure, Held, Store};

check!(
    DESCRIBE_IS_THE_DECLARATION = describe_answers_the_declaration_whole,
    "014 and 024: for the owner, a described type projects back onto its declaration byte for byte — every field's function, embedding, initial value, searchable, sensitive, label and description, and the type's label, language and view; a masked field shows its shape only, without its initial value (050); functions, triggers, object sets and views are answered as declared, to a principal who reads every field they name or holds the schema grant, as in force at the read position; a type that is shown is one its principal may find and get, also where every field is masked",
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
    body.initial = Some(serde_json::json!("none"));
    let net = described("net", "Net", StorageKind::Number);
    let mut gross = described("gross", "Gross", StorageKind::Number);
    gross.authority = Authority::Derived;
    gross.function = Some("scaled".into());
    gross.depends_on = vec![net.name.clone()];
    let mut doubled = described("doubled", "Doubled", StorageKind::Number);
    doubled.authority = Authority::Derived;
    doubled.derived = Some(Condition::from("net + net"));
    doubled.depends_on = vec![net.name.clone()];
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
    salary.initial = Some(serde_json::json!(0));
    let mut bonus = described("bonus", "Bonus", StorageKind::Number);
    bonus.authority = Authority::Derived;
    bonus.function = Some("tenth".into());
    bonus.depends_on = vec![salary.name.clone()];
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
    let fields = vec![
        field("title", StorageKind::Text),
        field(measure, StorageKind::Number),
    ];
    let sum = format!("sum({over})");
    let mut ty = view_of(name, "ledger", &["title"], fields, &[(measure, &sum)]);
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
        risk: Default::default(),
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
        constraints: Default::default(),
        annotations: t.annotations.clone(),
        insert_only: false,
        retired_by: t.lineage.retired_by,
        name: t.name.clone(),
        key: t.access.key.clone(),
        fields: t.fields.iter().map(field_projected).collect(),
        relations: t.relations.clone(),
        view: t.view.clone().map(Box::new),
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
        computed_at: f.computed_at,
        depends_on: f.depends_on.clone(),
        on_input_change: f.on_input_change,
        initial: f.initial.clone(),
        sensitive: f.sensitive,
        searchable: f.searchable,
        erasable: f.erasable,
        description: f.description.clone(),
        label: f.label.clone(),
        embed: f.embed.clone(),
    }
}

fn bytes<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("<unserialisable: {e}>"))
}

/// The same declaration, byte for byte, or a failure naming both.
fn same<T: serde::Serialize>(what: &str, got: &T, declared: &T) -> Held {
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

/// Reads everything but the ledger's salary, which it sees masked, and
/// holds no schema grant.
fn clerk(store: &dyn LakeStore) -> Principal {
    field_access(store, "clerk", "ledger", &[("salary", Access::Masked)])
}

async fn describe_at(
    cat: &dyn Catalog,
    who: &Principal,
    at: ReadAt,
    what: &str,
) -> Result<LakeDescription, Failure> {
    Ok(cat.describe(who, at).await.must(what)?.data)
}

async fn describe_answers_the_declaration_whole(store: Store) -> Held {
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
        let fut = cat.declare(&own, d);
        fut.await.must("declaring the ledger's ontology")?;
    }
    let earlier = store.head().await.must("reading the head")?;
    let again = function("scaled", "net", "net * 3");
    let fut = cat.declare(&own, Declaration::Function(again.clone()));
    fut.await.must("declaring `scaled` again")?;

    // The owner, at the head: every declaration whole.
    let d = describe_at(cat, &own, ReadAt::default(), "describing as the owner").await?;
    for decl in [&ledger, &net_view, &pay_view] {
        let t = one_type(&d, decl.name.as_str())?;
        same(&format!("the owner's `{}`", decl.name), &projected(t), decl)?;
        let alone = cat.describe_type(&own, &decl.name, ReadAt::default());
        let alone = alone.await.must("describing one type as the owner")?.data;
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
            clock: None,
        },
        "describing as the owner as_of an earlier position",
    )
    .await?;
    same(
        &format!("the owner's functions as_of {earlier}"),
        &then.functions,
        &vec![first.clone(), tenth.clone()],
    )?;

    // The clerk: the salary masked to its shape, with the bonus computed
    // from it, and nothing that names it.
    let who = clerk(store);
    let c = describe_at(cat, &who, ReadAt::default(), "describing as the clerk").await?;
    let t = one_type(&c, "ledger")?;
    for f in &t.fields {
        let decl = ledger
            .fields
            .iter()
            .find(|d| d.name == f.name)
            .ok_or_else(|| Failure::new(format!("describe invents field `{}`", f.name)))?;
        if ["salary", "bonus"].contains(&f.name.as_str()) {
            let shape = FieldDescription {
                name: decl.name.clone(),
                ty: decl.ty.clone(),
                required: decl.required,
                authority: decl.authority,
                access: Access::Masked,
                on_input_change: decl.on_input_change,
                sensitive: decl.sensitive,
                ..Default::default()
            };
            same(&format!("the clerk's masked `{}`", f.name), f, &shape)?;
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
        &net_view.view.clone().map(|v| *v),
    )?;
    same(
        "the clerk's view over the salary",
        &one_type(&c, "pay_by_title")?.view,
        &None,
    )?;

    // One shown the ledger who reads no field of it may find and get it:
    // a record is its key, its version and what is withheld.
    upsert(cat, &own, "ledger", "a", json!({"title": "rent", "net": 1})).await?;
    let masked = Grants {
        default: Access::Masked,
        ..Grants::default()
    };
    let who = principal(store, PrincipalKind::Agent, "auditor", masked);
    let may = describe_type(cat, &who, "ledger").await?.may;
    let found = all(cat, &who, "ledger").await?;
    let withheld = matches!(&found[..], [r] if r.fields.is_empty() && !r.masked.is_empty());
    ensure!(
        may.find && may.get && withheld,
        "one shown the ledger with every field masked is answered its record with the \
         masked list, so describe says it may find and get it; it says {may:?} and find \
         answers {found:?}"
    );
    Ok(())
}

check!(
    SCHEMA_TOLD_AS_RECORDED = a_holder_of_schema_is_told_every_definition_but_a_verb,
    "006, extended (lakelands#928): a holder of `schema` at `none` on a field, or masked on it, is described the trigger, the object set and the view whose conditions and measures name that field, whole, where a reader without `schema` is described none of them; a verb, described as a call to one that may make it, is described to neither; a field derived from one at `none` is not known to either, and one derived from a masked field is described masked to both"
);

check!(
    DESCRIBED_AS_SHOWN = a_description_says_only_what_its_reader_is_shown,
    "006 and 024, extended (lakelands#662): a type's `searchable` and `sourced` are said of the fields its reader is described, and a field it sees masked is searchable to it no more; a relation is described, listed in a proposal, named by a migration's refusal, offered to `link` and its link type shown only to a reader shown both its ends and, of a derived one, every field its match pairs, which a field it sees masked still is; a reader not shown a relation's target proposes the type as it is shown it, its declaration and its migration, and lists and rejects its own proposal (lakelands#762)"
);

async fn a_description_says_only_what_its_reader_is_shown(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let note = FieldDecl {
        searchable: true,
        authority: Authority::Source,
        ..field("note", StorageKind::Text)
    };
    let mut invoice = type_decl("invoice", vec![field("amount", StorageKind::Decimal), note]);
    let pairs = vec![MatchPair {
        from: "note".into(),
        to: "note".into(),
    }];
    invoice.relations = vec![
        RelationDecl::to("paid", "salary"),
        RelationDecl {
            derived: Some(Derived {
                pairs,
                filter: None,
            }),
            ..RelationDecl::to("same", "salary")
        },
    ];
    let again = ProposedWrite::Declare {
        declarations: vec![Declaration::Type(invoice.clone())],
        plan: None,
    };
    let invoice_as_declared = invoice.clone();
    for t in [
        type_decl(
            "salary",
            vec![
                field("note", StorageKind::Text),
                field("code", StorageKind::Text),
            ],
        ),
        invoice,
    ] {
        l.declare_as(Declaration::Type(t), "declaring the types")
            .await?;
    }
    let proposed = l.cat.propose(&l.own, again, "again".into()).await;
    proposed.must("proposing `invoice` again")?;
    // `invoice` as `far` is shown it: its author's declaration carries the
    // relations it is not shown back as they stand.
    let mut mine = invoice_as_declared.clone();
    mine.relations.clear();
    mine.description = Some("far's".into());
    let mut wider = mine.clone();
    wider.fields.push(field("extra", StorageKind::Text));
    let theirs = [
        ProposedWrite::Declare {
            declarations: vec![Declaration::Type(mine)],
            plan: None,
        },
        ProposedWrite::Migrate {
            migration: Box::new(Migration::to(wider)),
            report: Default::default(),
        },
    ];
    // `far` writes everything but `salary`, which it is kept from.
    let mut grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert("salary".into(), none);
    grants.review = true;
    let far = principal(s, PrincipalKind::Agent, "far", grants);
    let mut hidden = field_access(s, "hidden", "invoice", &[("note", Access::None)]);
    hidden.grants.schema = true;
    let masked = field_access(s, "masked", "invoice", &[("note", Access::Masked)]);
    let mut red = Vec::new();
    // `far` proposes `invoice` as it is shown it, and finds and rejects its
    // own proposals: the relations restated name nothing it is not shown
    // (lakelands#762).
    for (write, why) in theirs.into_iter().zip(["far's", "far's migration"]) {
        let proposed = l.cat.propose(&far, write, why.into()).await;
        let proposed = proposed.must(&format!("`far` proposing {why}"))?;
        let lakelands_api::Outcome::Proposed { proposal, .. } = proposed else {
            return Err(Failure::new(format!(
                "{why} must be proposed: {proposed:?}"
            )));
        };
        let listed = l.cat.proposals(&far, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals as `far`")?.data;
        let rejected = l.cat.reject(&far, &proposal, "mine".into()).await;
        if !listed.iter().any(|p| p.id == proposal) || rejected.is_err() {
            red.push(format!(
                "`far` must list and reject {why}; it lists {} and rejecting answered {rejected:?}",
                json!(listed)
            ));
        }
    }
    for (who, want) in [
        (&l.own, (true, true, "paid same", true, "paid")),
        (&far, (true, true, "", false, "")),
        (&hidden, (false, false, "paid", true, "")),
        (&masked, (false, true, "paid same", true, "")),
    ] {
        let lake = l.cat.describe(who, ReadAt::default()).await;
        let lake = lake
            .must(&format!("describing the lake as `{}`", who.id))?
            .data;
        let names = |n: Vec<String>| n.join(" ");
        let Some(t) = lake.types.iter().find(|t| t.name.as_str() == "invoice") else {
            return Err(Failure::new(format!(
                "`{}` is not described `invoice`",
                who.id
            )));
        };
        let got = (
            t.access.searchable,
            t.lineage.sourced,
            names(t.relations.iter().map(|r| r.name.to_string()).collect()),
            lake.types
                .iter()
                .any(|t| t.name.as_str() == "sys.link.invoice.paid"),
            names(t.may.link.iter().map(|r| r.to_string()).collect()),
        );
        if (got.0, got.1, got.2.as_str(), got.3, got.4.as_str()) != want {
            red.push(format!("`{}`: {got:?}, not {want:?}", who.id));
        }
    }
    // A proposed declaration is listed as it would be described.
    let listed = l.cat.proposals(&far, ProposalFilter::default()).await;
    let listed = listed.must("listing the proposals as `far`")?.data;
    let again = listed.iter().find(|p| p.why == "again");
    let relations = again.and_then(|p| match p.write.as_ref()? {
        ProposedWrite::Declare { declarations, .. } => match declarations.first() {
            Some(Declaration::Type(t)) => Some(t.relations.len()),
            _ => None,
        },
        _ => None,
    });
    if relations != Some(0) {
        red.push(format!(
            "`far` is listed `invoice` with relations: {relations:?}"
        ));
    }
    // Nor does a migration's refusal name the type of a relation not shown.
    let to = type_decl("salary", vec![field("code", StorageKind::Text)]);
    let m = Migration::to(to);
    let dropped = l.cat.migrate(&hidden, m).await.map(drop);
    // No member of its wire form, its sentence among them, names `invoice`.
    let said = dropped.as_ref().err().map(|e| json!(e).to_string());
    let unnamed = matches!(invalid_case(&dropped),
        Some(Case::MigrationTakesRead { unnamed: true, field, ty, .. })
            if field == "note" && ty == "salary");
    if !unnamed || said.as_deref().unwrap_or_default().contains("invoice") {
        red.push(format!("`hidden` drops `salary.note`: {dropped:?}"));
    }
    ensure!(
        red.is_empty(),
        "searchable, sourced, the relations, the link type and `may.link` as described, and \
         the relations listed and named: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_holder_of_schema_is_told_every_definition_but_a_verb(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let t = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let shout = json!({"name": "shout", "type": {"kind": "text"}, "derived": "upper(sec)"});
    let x = "sec = 'x'";
    for d in [
        json!({"declare": "type", "name": "vault", "fields": [t("name"), t("sec"), shout]}),
        json!({"declare": "trigger", "name": "watch", "on": "vault", "when": x}),
        json!({"declare": "object_set", "name": "secrets", "of": "vault", "where": x}),
        json!({"declare": "verb", "name": "seal", "on": "vault", "risk": "high", "effect": "write",
               "requires": "sec is not null", "set": [{"field": "sec", "value": "'y'"}]}),
        json!({"declare": "type", "name": "byname", "key": ["name"], "fields": [t("name"),
               {"name": "n", "type": {"kind": "number"}}], "view": {"source": "vault", "where": x,
               "group_by": ["name"], "measures": [{"name": "n", "expr": "count(sec)"}]}}),
    ] {
        l.declare_json(d).await?;
    }
    // Per reader: trigger, set, view's definition, verb, and `shout`'s
    // access, `-` where it is not described.
    let mut seen = Vec::new();
    for schema in [true, false] {
        for sec in ["none", "masked"] {
            let grants = json!({"default": "read", "schema": schema,
                                "types": {"vault": {"fields": {"sec": sec}}}});
            let who = super::only::holder(store.as_ref(), &format!("{schema}-{sec}"), grants);
            let d = l.cat.describe(&who, Default::default()).await;
            let d = d.must("describing")?.data;
            let (w, s) = (Some(Condition::from(x)), Condition::from(x));
            let view = |t: &TypeDescription| t.view.as_ref().is_some_and(|v| v.r#where == w);
            let fields = d.types.iter().flat_map(|t| &t.fields);
            let mut shout = fields.filter(|f| f.name.as_str() == "shout");
            let cells = [
                d.triggers.iter().any(|t| t.when == w),
                d.object_sets.iter().any(|o| o.filter == s),
                d.types.iter().any(view),
                d.verbs.iter().any(|v| v.name.as_str() == "seal"),
            ];
            let cell = |(b, c): (bool, char)| if b { c } else { '-' };
            let mut row: String = cells.into_iter().zip("TSVC".chars()).map(cell).collect();
            row.push_str(
                &shout
                    .next()
                    .map_or("-".into(), |f| format!(" {:?}", f.access)),
            );
            seen.push(row);
        }
    }
    let want = ["TSV--", "TSV- Masked", "-----", "---- Masked"];
    ensure!(
        seen == want,
        "to `schema` at `none` and masked on `sec`, then to readers without it, (trigger, set, \
         view, verb, `shout`) are {seen:?}, where {want:?} are due"
    );
    Ok(())
}
