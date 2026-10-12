//! A read of a view probes what the view's definition reads, and a reader
//! who may not read all of it is refused without being told what
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! extended).

use std::fmt::Debug;

use lakelands_api::error::Case;
use lakelands_api::read::{Find, Get};
use lakelands_api::write::Declaration;
use lakelands_api::{Access, Error, Refusal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, field_access, invalid_case, migration, pattern, reads_all, refused_on,
};
use crate::conformance::{Held, Store};

check!(
    SHOWN_AS_READ = a_views_definition_is_shown_to_those_its_read_answers,
    "024, extended (lakelands#798): `describe_type` shows a view's definition to a reader exactly where `find` on the view answers it, by one rule for the fields a view reads, its grouping, its `where` and what its measures aggregate: a view whose `where` or measure reads a field it may not read, a view over such a view and one that merges such a source are described to it as types with no definition and refused its read; a name a measure bears outside an aggregate is an earlier measure and no field of the source, so a view that names one is shown and answered whatever its source holds under that name, to a reader kept from such a field or masked on it the same bytes as before the source held it, since a refusal there would tell that the field is declared; one who reads every field is shown and answered each; and one with the `schema` grant is shown every definition and refused the read it may not make"
);

check!(
    VIEW_OVER_A_HIDDEN_FIELD = a_view_over_a_field_its_reader_may_not_read_names_nothing,
    "024, extended: `find`, `get` and a pattern started on a view whose `where` or a measure reads a field its reader may not know of, or may not read, are refused as not granted, on no field and with neither the field nor its source in the text; a reader who may read every field the definition reads is answered, and one with the `schema` grant, whom `describe` shows the definition, is told the field as before"
);

/// Fails unless `answer` is a refusal as not granted that names no field.
fn names_nothing<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Held {
    let no_field = Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    };
    ensure!(
        answer.as_ref().err() == Some(&no_field),
        "{what} must be refused as not granted, naming no field; it answered {answer:?}"
    );
    Ok(())
}

/// What a reader of `invoice` may not read: `salary` at all, `body` but masked.
const PARTIAL: [(&str, Access); 2] = [("salary", Access::None), ("body", Access::Masked)];

/// `invoice` and `crew`, each with one record, and the views: over
/// `invoice` by a `where`, by a measure and by a masked field; `by_name`,
/// whose second measure reads its first, named as a field of the source
/// is, and `by_alias`, the same under a name the source does not hold;
/// `pay` and `named` over it, the same again, `pay` holding no `salary`;
/// `over`, a view over `by_measure`; and `merged`, over `crew` with
/// `invoice` merged in by a sum.
async fn lake(store: &dyn lakelands_api::LakeStore) -> Result<Lake, crate::conformance::Failure> {
    let l = Lake::of(store);
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    let fields = [text("note"), number("salary"), text("body"), text("team")];
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": fields}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "crew", "fields": [text("team")]}))
        .await?;
    let n = |measure: &str| json!([{"name": "n", "expr": measure}]);
    let after = |first: &str| json!([{"name": first, "expr": "count()"}, {"name": "n", "expr": format!("{first} + 1")}]);
    let view = |source: &str, filter: Option<&str>, measures: serde_json::Value| {
        let mut view = json!({"source": source, "group_by": ["team"], "measures": measures});
        view["where"] = json!(filter);
        view
    };
    let mut merged = view("crew", None, n("count()"));
    merged["merge"] = json!([{"source": "invoice", "group_by": ["team"],
        "measures": [{"name": "paid", "expr": "sum(salary)"}]}]);
    let filtered = Some("note = 'x' and salary > 0");
    for (name, more, view) in [
        ("by_where", None, view("invoice", filtered, n("count()"))),
        ("by_measure", None, view("invoice", None, n("sum(salary)"))),
        (
            "by_masked",
            None,
            view("invoice", Some("body = 'x'"), n("count()")),
        ),
        (
            "by_name",
            Some("salary"),
            view("invoice", None, after("salary")),
        ),
        ("by_alias", Some("m"), view("invoice", None, after("m"))),
        ("over", None, view("by_measure", None, n("sum(n)"))),
        ("merged", Some("paid"), merged),
    ] {
        let mut fields = vec![text("team"), number("n")];
        fields.extend(more.map(number));
        l.declare_json(json!({"declare": "type", "name": name, "key": ["team"],
                              "fields": fields, "view": view}))
            .await?;
    }
    l.declare_json(json!({"declare": "type", "name": "pay", "fields": [text("team")]}))
        .await?;
    let fields = [text("team"), number("n"), number("salary")];
    l.declare_json(json!({"declare": "type", "name": "named", "key": ["team"],
        "fields": fields, "view": view("pay", None, after("salary"))}))
        .await?;
    l.upsert("pay", "p1", json!({"team": "a"})).await?;
    let record = json!({"note": "x", "salary": 5, "body": "x", "team": "a"});
    l.upsert("invoice", "i1", record).await?;
    l.upsert("crew", "c1", json!({"team": "a"})).await?;
    Ok(l)
}

async fn a_views_definition_is_shown_to_those_its_read_answers(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(store.as_ref()).await?);
    let mut author = field_access(s, "author", "invoice", &PARTIAL);
    author.grants.schema = true;
    // Each reader: whether it is shown the definitions, and answered the reads.
    let cast = [
        (field_access(s, "reader", "invoice", &PARTIAL), false, false),
        (reads_all(s, "all"), true, true),
        (author, true, false),
    ];
    let mut apart = Vec::new();
    for (who, shown, answered) in &cast {
        let views = [
            "by_where",
            "by_measure",
            "by_masked",
            "by_name",
            "by_alias",
            "over",
        ];
        for view in views.into_iter().chain(["merged"]) {
            let ty = view.into();
            let described = l.cat.describe_type(who, &ty, Default::default()).await;
            let described = described.must("describing a view")?.data.view.is_some();
            let found = l.cat.find(who, &ty, Find::default()).await;
            // Neither reads a field of its source: they are everyone's.
            let open = view == "by_alias" || view == "by_name";
            if (described, found.is_ok()) != (*shown || open, *answered || open) {
                let id = &who.id;
                apart.push(format!(
                    "`{view}` to `{id}`: shown {described}, read {found:?}"
                ));
            }
        }
    }
    ensure!(
        apart.is_empty(),
        "a view's definition is shown to a reader its read answers and to a holder of \
         `schema`, and to no other; apart:\n{}",
        apart.join("\n")
    );
    // A name outside an aggregate is no field of the source: `named` is
    // described and read the same, byte for byte, before `pay` holds a
    // `salary` and after, by one kept from that field and one masked on it.
    let named = "named".into();
    let mut told = Vec::new();
    for round in 0..2 {
        for access in [Access::None, Access::Masked] {
            let who = field_access(s, "kept", "pay", &[("salary", access)]);
            let described = l.cat.describe_type(&who, &named, Default::default()).await;
            let found = l.cat.find(&who, &named, Find::default()).await;
            told.push(format!(
                "{:?} {:?}",
                described.map(|d| d.data),
                found.map(|f| f.data)
            ));
        }
        if round == 0 {
            let salary = json!({"name": "salary", "type": {"kind": "number"}});
            let salary = serde_json::from_value(salary).expect("a field");
            (l.cat.add_field(&l.own, &"pay".into(), salary).await).must("adding `salary`")?;
        }
    }
    ensure!(
        told[..2] == told[2..]
            && told
                .iter()
                .all(|t| t.contains("view: Some") && t.contains("Ok(")),
        "a view that names an earlier measure is shown and answered the same whether or not its \
         source holds a field of that name; before, by one kept from it and one masked on it, \
         then after:\n{}",
        told.join("\n")
    );
    // `declare` and the migration preflight ask the same of a view's own
    // sources, the preflight before it says that a view is not migrated:
    // an author kept from `salary` and masked on `body` makes a view that
    // names a measure as either is named, and none whose `where` reads
    // `body`.
    let author = &cast[2].0;
    let masked = Some(("body", &Refusal::NotGranted));
    for (first, filter) in [("salary", None), ("body", None), ("m", Some("body = 'x'"))] {
        let fields = [("team", "text"), ("n", "number"), (first, "number")];
        let fields = fields.map(|(n, k)| json!({"name": n, "type": {"kind": k}}));
        let measures = json!([{"name": first, "expr": "count()"},
                              {"name": "n", "expr": format!("{first} + 1")}]);
        let view = json!({"source": "invoice", "group_by": ["team"], "measures": measures,
                          "where": filter});
        let to = json!({"name": "by_alias", "key": ["team"], "fields": fields, "view": view});
        let to: lakelands_api::ontology::TypeDecl = serde_json::from_value(to).expect("a view");
        let mut new = to.clone();
        new.name = format!("mine_{first}").as_str().into();
        let flown = l.cat.migrate(author, migration(to)).await;
        let declared = l.cat.declare(author, Declaration::Type(new)).await;
        let told = (refused_on(&flown), refused_on(&declared));
        let of_view = matches!(invalid_case(&flown), Some(Case::MigrationOfView { .. }));
        ensure!(
            if filter.is_none() {
                of_view && declared.is_ok()
            } else {
                told == (masked, masked)
            },
            "an author kept from `salary` and masked on `body` declares a view whose measure is \
             named `{first}`, its preflight refused only as a view's; one whose `where` is \
             {filter:?} is refused both on `body` as not granted; the preflight answered \
             {flown:?}, the declaration {declared:?}"
        );
    }
    Ok(())
}

async fn a_view_over_a_field_its_reader_may_not_read_names_nothing(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let reader = field_access(store.as_ref(), "reader", "invoice", &PARTIAL);
    for view in ["by_where", "by_measure", "by_masked"] {
        let ty = view.into();
        let (params, at) = Default::default();
        let get = Get {
            key: "a".into(),
            with_meta: false,
            params,
            at,
        };
        let found = l.cat.find(&reader, &ty, Find::default()).await;
        names_nothing(&format!("`find` on `{view}`"), found)?;
        names_nothing(
            &format!("`get` on `{view}`"),
            l.cat.get(&reader, &ty, get).await,
        )?;
        let walked = pattern(view, None, vec![], Default::default());
        names_nothing(
            &format!("a pattern on `{view}`"),
            l.cat.query(&reader, walked).await,
        )?;
    }
    // One who may read all the definition reads is answered; one `describe`
    // shows the definition to is told the field it may not know of.
    let by_where = "by_where".into();
    let all = reads_all(store.as_ref(), "all");
    let found = l.cat.find(&all, &by_where, Find::default()).await;
    found.must("`find` on `by_where` by one who reads every field")?;
    let mut author = field_access(store.as_ref(), "author", "invoice", &PARTIAL);
    author.grants.schema = true;
    let told = l.cat.find(&author, &by_where, Find::default()).await;
    ensure!(
        matches!(&told, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "salary"),
        "one with the `schema` grant is refused on `salary`, as before; it answered {told:?}"
    );
    Ok(())
}
