//! Values against their storage kinds and value types
//! (`docs/decisions/011-storage-kind-and-value-type.md`, `docs/log-format.md`
//! §5).

use std::cmp::Ordering;
use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, FieldName, Find, LakeStore, Record, Refusal, TypeName, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all_found, declare, doors, field, head_unmoved, owner, patch, read, type_decl, upsert,
    value,
};
use crate::expr::order::cmp_values;

check!(
    /// An instant reads back in its one written form.
    DATE_TIME_CANONICAL = date_time_values_read_back_in_their_one_canonical_form,
    "the log format §5: a date-time is kept in UTC as YYYY-MM-DDTHH:MM:SS.ffffffZ, whatever offset it was written with"
);

check!(
    /// Bytes have one spelling.
    BYTES_ONE_SPELLING = bytes_are_padded_standard_base64_and_nothing_else,
    "the log format §5: bytes are canonical standard base64, padded and with zero pad bits; unpadded, url-safe or non-canonical spellings are refused"
);

check!(
    /// A vector has its declared length.
    VECTOR_LENGTH = a_vector_of_the_wrong_length_is_refused,
    "011: a vector field declares its dimensions, and a value of another length does not fit it"
);

check!(
    /// A value type's rules hold on every write.
    VALUE_TYPE_RULES = a_value_types_one_of_format_and_scale_hold_on_every_write,
    "011: a value's `one_of`, named `format` and decimal `scale` are the value type's, checked on every write; scale rounds half away from zero"
);

check!(
    /// Null clears a field whatever its value type says.
    NULL_CLEARS_ANY_TYPE = null_clears_a_field_whatever_its_value_type_allows,
    "012 and 011: `null` in a patch clears the field; it is not a value the value type must allow"
);

async fn one_field(store: &dyn LakeStore, name: &str, ty: FieldType) -> Result<(), Failure> {
    let cat = doors(store);
    let decl = type_decl(
        name,
        vec![FieldDecl {
            ty,
            ..field("v", StorageKind::Text)
        }],
    );
    declare(cat.as_ref(), &owner(store), decl).await?;
    Ok(())
}

/// Write `v` to `thing` and expect a refusal on field `v`, with nothing
/// appended.
async fn refused(store: &dyn LakeStore, v: Value, why: &str) -> Result<(), Failure> {
    refused_in(store, "thing", v, why).await
}

async fn refused_in(store: &dyn LakeStore, ty: &str, v: Value, why: &str) -> Result<(), Failure> {
    let cat = doors(store);
    let before = store.head().await.must("reading the head")?;
    let what = format!("writing {v} ({why})");
    let err = cat
        .upsert(
            &owner(store),
            &TypeName::from(ty),
            &"k".into(),
            patch(json!({ "v": v })),
            WriteOpts::default(),
        )
        .await
        .must_fail(&what)?;
    head_unmoved(store, before, &what).await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { field: Some(f), reason: Refusal::Constraint { .. } | Refusal::Invalid { .. } }
                if f.as_str() == "v"
        ),
        "{what} must be refused on field `v`; it failed with {err:?}"
    );
    Ok(())
}

async fn date_time_values_read_back_in_their_one_canonical_form(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::DateTime),
    )
    .await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (written, canonical) in [
        ("2026-09-24T07:57:11+02:00", "2026-09-24T05:57:11.000000Z"),
        ("2026-09-24", "2026-09-24T00:00:00.000000Z"),
        ("2026-09-24T05:57:11.5Z", "2026-09-24T05:57:11.500000Z"),
    ] {
        upsert(cat.as_ref(), &own, "thing", "k", json!({ "v": written })).await?;
        let r = read(cat.as_ref(), &own, "thing", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(canonical)),
            "`{written}` must read back as `{canonical}`; it reads {:?}",
            value(&r, "v")
        );
    }
    refused(store.as_ref(), json!("2026-09-24T05:57:11"), "no offset").await?;
    refused(store.as_ref(), json!("2026-02-30"), "no such day").await
}

async fn bytes_are_padded_standard_base64_and_nothing_else(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(store.as_ref(), "thing", FieldType::kind(StorageKind::Bytes)).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for ok in ["aGVsbG8=", "aGk=", "+/+/", ""] {
        upsert(cat.as_ref(), &own, "thing", "k", json!({ "v": ok })).await?;
        let r = read(cat.as_ref(), &own, "thing", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(ok)),
            "the bytes `{ok}` must read back as written; they read {:?}",
            value(&r, "v")
        );
    }
    for bad in ["aGVsbG8", "-_-_", "aGl=", "a=bc"] {
        refused(store.as_ref(), json!(bad), "not padded standard base64").await?;
    }
    Ok(())
}

async fn a_vector_of_the_wrong_length_is_refused(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let ty = FieldType {
        dimensions: Some(3),
        ..FieldType::kind(StorageKind::Vector)
    };
    one_field(store.as_ref(), "thing", ty).await?;
    let cat = doors(store.as_ref());
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "thing",
        "k",
        json!({"v": [0.1, 0.2, 0.3]}),
    )
    .await?;
    refused(
        store.as_ref(),
        json!([0.1, 0.2]),
        "two numbers for three dimensions",
    )
    .await?;
    refused(
        store.as_ref(),
        json!([0.1, 0.2, 0.3, 0.4]),
        "four numbers for three",
    )
    .await?;
    refused(store.as_ref(), json!([0.1, "x", 0.3]), "not all numbers").await
}

fn value_type(name: &str, kind: StorageKind) -> ValueTypeDecl {
    ValueTypeDecl {
        name: name.into(),
        kind,
        constraint: None,
        one_of: Vec::new(),
        format: None,
        scale: None,
        description: None,
    }
}

async fn typed(store: &dyn LakeStore, ty: &str, vt: ValueTypeDecl) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    let kind = vt.kind;
    let name = vt.name.clone();
    cat.declare(&own, Declaration::ValueType(vt))
        .await
        .must(&format!("declaring value type `{name}`"))?;
    one_field(
        store,
        ty,
        FieldType {
            value_type: Some(name),
            ..FieldType::kind(kind)
        },
    )
    .await
}

async fn a_value_types_one_of_format_and_scale_hold_on_every_write(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());

    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": "paid"})).await?;
    refused(store.as_ref(), json!("lost"), "not one of open, paid").await?;

    let email = ValueTypeDecl {
        format: Some("email".into()),
        ..value_type("email", StorageKind::Text)
    };
    typed(store.as_ref(), "contact", email).await?;
    upsert(
        cat.as_ref(),
        &own,
        "contact",
        "k",
        json!({"v": "ada@example.org"}),
    )
    .await?;
    refused_in(
        store.as_ref(),
        "contact",
        json!("ada at example"),
        "not an email",
    )
    .await?;

    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    typed(store.as_ref(), "payment", money).await?;
    for (written, kept) in [
        ("1.005", "1.01"),
        ("-1.005", "-1.01"),
        ("2.004", "2.00"),
        ("7", "7.00"),
    ] {
        upsert(cat.as_ref(), &own, "payment", "k", json!({ "v": written })).await?;
        let r = read(cat.as_ref(), &own, "payment", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(kept)),
            "a `money` value (scale 2) written as {written} must be kept as {kept}, \
             rounded half away from zero; it reads {:?}",
            value(&r, "v")
        );
    }

    let unknown = ValueTypeDecl {
        format: Some("shoe_size".into()),
        ..value_type("odd", StorageKind::Text)
    };
    let before = store.head().await.must("reading the head")?;
    cat.declare(&own, Declaration::ValueType(unknown))
        .await
        .must_fail("declaring a value type with the unknown format `shoe_size`")?;
    head_unmoved(store.as_ref(), before, "declaring an unknown format").await?;
    Ok(())
}

async fn null_clears_a_field_whatever_its_value_type_allows(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": "open"})).await?;
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": null})).await?;
    let r = read(cat.as_ref(), &own, "thing", "k").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear `v` although null is not one of its allowed values; `v` \
         reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

check!(
    /// A decimal is written as a string of digits, never a number.
    DECIMAL_NOT_A_NUMBER = a_decimal_written_as_a_json_number_is_refused,
    "the log format §5: a decimal's one JSON form is a string of digits, so a JSON number, which may already have lost digits, is refused"
);

async fn a_decimal_written_as_a_json_number_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::Decimal),
    )
    .await?;
    let cat = doors(store.as_ref());
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "thing",
        "k",
        json!({"v": "12.50"}),
    )
    .await?;
    refused(store.as_ref(), json!(12.5), "a decimal as a JSON number").await?;
    refused(store.as_ref(), json!(12), "a decimal as a JSON integer").await?;
    refused(store.as_ref(), json!("12,50"), "a decimal with a comma").await
}

check!(
    /// A geometry reads back in one form; a malformed one is refused.
    GEO_VALUES = a_geo_value_reads_back_in_its_one_form_and_a_malformed_one_is_refused,
    "011, extended, and the log format §5: a geo value is a GeoJSON geometry whose numbers take their one JSON form, the same through get and find, live and after a rebuild; empty `coordinates` are the empty geometry of every type alike, while an empty part inside one is refused; an open ring, a missing `type` or a GeoJSON object that is not a geometry is refused naming the field; null clears it"
);

check!(
    /// A JSON value reads back in one form, and null clears it.
    JSON_VALUES = a_json_value_reads_back_in_its_one_form_and_null_clears_it,
    "011, extended, and the log format §5: a json value is kept as written but for its numbers, which take their one JSON form (`1`, never `1.0`), the same through get and find, live and after a rebuild; null clears it like any field"
);

check!(
    /// A page ordered by a geo or json field takes the fallback order.
    GEO_JSON_ORDER = a_page_ordered_by_a_geo_or_json_field_takes_the_rulebooks_fallback_order,
    "029: a geo or json value has no order of its own, so a find ordered by one ranks it as `cmp_values` does, a JSON value by its canonical bytes, a missing value last either way, ties by key; paging walks the same order"
);

/// Every `thing` record's fields as `get` and as `find` answer them, as
/// JSON text by key.
async fn texts(store: &dyn LakeStore, keys: &[&str]) -> Result<Vec<(String, String)>, Failure> {
    let cat = doors(store);
    let own = owner(store);
    let found = all_found(cat.as_ref(), &own, "thing", Find::default()).await?;
    let mut out = Vec::new();
    for key in keys {
        let got = read(cat.as_ref(), &own, "thing", key).await?;
        let in_find = found
            .iter()
            .find(|r| r.key.as_str() == *key)
            .ok_or_else(|| Failure::new(format!("find did not answer `thing` `{key}`")))?;
        let text = |r: &Record| {
            serde_json::to_string(&r.fields)
                .map_err(|e| Failure::new(format!("`thing` `{key}` does not print: {e}")))
        };
        out.push((text(&got)?, text(in_find)?));
    }
    Ok(out)
}

/// Writes each value to its own `thing` record, then reads every record
/// through `get` and `find`, live and after a rebuild, and holds each to
/// its one form.
async fn one_form(store: Arc<dyn LakeStore>, cases: &[(&str, Value, &str)]) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (key, written, _) in cases {
        upsert(cat.as_ref(), &own, "thing", key, json!({ "v": written })).await?;
    }
    let keys: Vec<&str> = cases.iter().map(|(k, _, _)| *k).collect();
    let live = texts(store.as_ref(), &keys).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = texts(store.as_ref(), &keys).await?;
    for (((key, written, form), live), rebuilt) in cases.iter().zip(live).zip(rebuilt) {
        let want = format!(r#"{{"v":{form}}}"#);
        for (when, (got, found)) in [("live", live), ("after a rebuild", rebuilt)] {
            ensure!(
                got == want && found == want,
                "`{key}`, written as {written}, must read {want} (numbers in their one \
                 form) through get and find {when}; get read {got} and find {found}"
            );
        }
    }
    Ok(())
}

async fn a_geo_value_reads_back_in_its_one_form_and_a_malformed_one_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(store.as_ref(), "thing", FieldType::kind(StorageKind::Geo)).await?;
    one_form(
        store.clone(),
        &[
            (
                "point",
                json!({"type": "Point", "coordinates": [7.0, 50.70]}),
                r#"{"coordinates":[7,50.7],"type":"Point"}"#,
            ),
            (
                "polygon",
                json!({"type": "Polygon", "coordinates": [
                    [[0.0, 0.0], [4.0, 0.0], [4.0, 3.50], [0.0, 0.0]],
                    [[1, 1], [2, 1], [2, 2.0], [1, 1]]
                ]}),
                r#"{"coordinates":[[[0,0],[4,0],[4,3.5],[0,0]],[[1,1],[2,1],[2,2],[1,1]]],"type":"Polygon"}"#,
            ),
            (
                "collection",
                json!({"type": "GeometryCollection", "geometries": [
                    {"type": "Point", "coordinates": [1.0, 2.0, 3.25]},
                    {"type": "LineString", "coordinates": [[0, 0], [1e1, 1.5]]}
                ]}),
                r#"{"geometries":[{"coordinates":[1,2,3.25],"type":"Point"},{"coordinates":[[0,0],[10,1.5]],"type":"LineString"}],"type":"GeometryCollection"}"#,
            ),
            (
                "empty-point",
                json!({"type": "Point", "coordinates": []}),
                r#"{"coordinates":[],"type":"Point"}"#,
            ),
            (
                "empty-line",
                json!({"type": "LineString", "coordinates": []}),
                r#"{"coordinates":[],"type":"LineString"}"#,
            ),
            (
                "empty-polygon",
                json!({"type": "Polygon", "coordinates": []}),
                r#"{"coordinates":[],"type":"Polygon"}"#,
            ),
            (
                "empty-multipolygon",
                json!({"type": "MultiPolygon", "coordinates": []}),
                r#"{"coordinates":[],"type":"MultiPolygon"}"#,
            ),
        ],
    )
    .await?;
    for (bad, why) in [
        (
            json!({"type": "Polygon", "coordinates": [[[0, 0], [4, 0], [4, 3], [0, 1]]]}),
            "an open ring",
        ),
        (
            json!({"type": "Polygon", "coordinates": [[[0, 0], [4, 0], [0, 0]]]}),
            "a ring of three positions",
        ),
        (
            json!({"type": "MultiPolygon", "coordinates": [[]]}),
            "a multi-polygon holding an empty polygon",
        ),
        (
            json!({"type": "Polygon", "coordinates": [[]]}),
            "a polygon holding an empty ring",
        ),
        (json!({"coordinates": [7, 50]}), "no `type`"),
        (
            json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [7, 50]}, "properties": {}}),
            "a Feature, not a geometry",
        ),
        (
            json!({"type": "GeometryCollection", "geometries": [{"type": "Point", "coordinates": [7]}]}),
            "a collection holding a point of one number",
        ),
        (json!("POINT(7 50)"), "well-known text, not GeoJSON"),
    ] {
        refused_in(store.as_ref(), "thing", bad, why).await?;
    }
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "thing", "point", json!({"v": null})).await?;
    let r = read(cat.as_ref(), &own, "thing", "point").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear the geo field `v`; it reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

async fn a_json_value_reads_back_in_its_one_form_and_null_clears_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(store.as_ref(), "thing", FieldType::kind(StorageKind::Json)).await?;
    one_form(
        store.clone(),
        &[
            (
                "object",
                json!({"s": "x", "one": 1, "one_again": 1.0, "list": [1, 1.0, 2.50, [0.0, {"deep": 1e2}]]}),
                r#"{"list":[1,1,2.5,[0,{"deep":100}]],"one":1,"one_again":1,"s":"x"}"#,
            ),
            ("number", json!(1.0), "1"),
            ("text", json!("1.0"), r#""1.0""#),
        ],
    )
    .await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "thing", "object", json!({"v": null})).await?;
    let r = read(cat.as_ref(), &own, "thing", "object").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear the json field `v`, not be kept as a JSON value; it reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

async fn a_page_ordered_by_a_geo_or_json_field_takes_the_rulebooks_fallback_order(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "thing",
            vec![field("g", StorageKind::Geo), field("j", StorageKind::Json)],
        ),
    )
    .await?;
    let point = |x: f64, y: f64| json!({"type": "Point", "coordinates": [x, y]});
    // Keys interleave the values, so no order passes by following the
    // keys; two records share each field's value to show ties go by key.
    let rows = [
        ("r01", point(10.0, 0.0), json!({"b": 1})),
        (
            "r02",
            json!({"type": "LineString", "coordinates": [[0, 0], [1, 1]]}),
            json!([10]),
        ),
        ("r03", point(9.0, 0.0), json!("abc")),
        ("r04", Value::Null, json!(3)),
        ("r05", point(-1.0, 2.5), json!(true)),
        ("r06", point(10.0, 0.0), json!({"a": [1, 2]})),
        (
            "r07",
            json!({"type": "GeometryCollection", "geometries": []}),
            Value::Null,
        ),
        ("r08", point(9.0, 0.0), json!("2026-09-27")),
        (
            "r09",
            json!({"type": "MultiPoint", "coordinates": [[0, 0]]}),
            json!([2]),
        ),
        ("r10", Value::Null, json!({"b": 1})),
        ("r11", point(1.5, 0.0), json!(false)),
        ("r12", point(0.0, 0.0), json!({"a": 1})),
    ];
    for (key, g, j) in &rows {
        upsert(cat.as_ref(), &own, "thing", key, json!({"g": g, "j": j})).await?;
    }
    for (name, at) in [("g", 1), ("j", 2)] {
        for dir in [Direction::Asc, Direction::Desc] {
            // The rulebook's order: by `cmp_values`, a missing value last
            // in either direction, ties by key.
            let mut want: Vec<(&str, &Value)> = rows
                .iter()
                .map(|r| (r.0, if at == 1 { &r.1 } else { &r.2 }))
                .collect();
            want.sort_by(|(ka, a), (kb, b)| {
                let by_value = match (a.is_null(), b.is_null()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) if dir == Direction::Asc => cmp_values(a, b),
                    (false, false) => cmp_values(b, a),
                };
                by_value.then(ka.cmp(kb))
            });
            let want: Vec<&str> = want.into_iter().map(|(k, _)| k).collect();
            for limit in [50, 3, 1] {
                let find = Find {
                    order_by: vec![Order {
                        field: FieldName::from(name),
                        by: OrderBy::Value,
                        dir,
                    }],
                    limit: Some(limit),
                    ..Find::default()
                };
                let walked: Vec<String> = all_found(cat.as_ref(), &own, "thing", find)
                    .await?
                    .iter()
                    .map(|r| r.key.as_str().to_owned())
                    .collect();
                ensure!(
                    walked == want,
                    "a find ordered by the {} field `{name}` {dir:?}, {limit} to a page, must \
                     walk the rulebook's order {want:?} (a JSON value by its canonical bytes, \
                     a missing value last, ties by key); it walked {walked:?}",
                    if at == 1 { "geo" } else { "json" }
                );
            }
        }
    }
    Ok(())
}
