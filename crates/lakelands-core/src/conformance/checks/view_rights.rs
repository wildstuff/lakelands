//! A read of a view probes what the view's definition reads, and a reader
//! who may not read all of it is refused without being told what
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! extended).

use std::fmt::Debug;
use std::sync::Arc;

use lakelands_api::read::{Find, Get};
use lakelands_api::{Access, Error, LakeStore, Refusal};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, Must, field_access, pattern, reads_all};

check!(
    /// A view over a field its reader may not read names nothing.
    VIEW_OVER_A_HIDDEN_FIELD = a_view_over_a_field_its_reader_may_not_read_names_nothing,
    "024, extended: `find`, `get` and a pattern started on a view whose `where` or a measure reads a field its reader may not know of, or may not read, are refused as not granted, on no field and with neither the field nor its source in the text; a reader who may read every field the definition reads is answered, and one with the `schema` grant, whom `describe` shows the definition, is told the field as before"
);

/// Fails unless `answer` is a refusal as not granted that names no field.
fn names_nothing<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Result<(), Failure> {
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

async fn a_view_over_a_field_its_reader_may_not_read_names_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    let fields = [text("note"), number("salary"), text("body"), text("team")];
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": fields}))
        .await?;
    for (name, filter, measure) in [
        ("by_where", Some("note = 'x' and salary > 0"), "count()"),
        ("by_measure", None, "sum(salary)"),
        ("by_masked", Some("body = 'x'"), "count()"),
    ] {
        let mut view = json!({"source": "invoice", "group_by": ["team"],
                              "measures": [{"name": "n", "expr": measure}]});
        view["where"] = json!(filter);
        l.declare_json(json!({"declare": "type", "name": name, "key": ["team"],
                              "fields": [text("team"), number("n")], "view": view}))
            .await?;
    }
    let record = json!({"note": "x", "salary": 5, "body": "x", "team": "a"});
    l.upsert("invoice", "i1", record).await?;
    let partial = [("salary", Access::None), ("body", Access::Masked)];
    let reader = field_access(store.as_ref(), "reader", "invoice", &partial);
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
    let mut author = field_access(store.as_ref(), "author", "invoice", &partial);
    author.grants.schema = true;
    let told = l.cat.find(&author, &by_where, Find::default()).await;
    ensure!(
        matches!(&told, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "salary"),
        "one with the `schema` grant is refused on `salary`, as before; it answered {told:?}"
    );
    Ok(())
}
