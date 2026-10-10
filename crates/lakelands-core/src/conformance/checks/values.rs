//! Values against their storage kinds and value types
//! (`docs/decisions/011-storage-kind-and-value-type.md`, `docs/log-format.md`
//! §5).

use std::cmp::Ordering;

use lakelands_api::error::Case;
use lakelands_api::ontology::{
    Authority, ComputedAt, FieldDecl, FieldType, StorageKind, ValueTypeDecl,
};
use lakelands_api::read::{Direction, OrderBy};
use lakelands_api::write::{Declaration, ProposedWrite, WriteOpts};
use lakelands_api::{Find, LakeStore, Record, Refusal};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, derived, doors, field, head_unmoved, order, owner, patch, refused_on,
    source_record, try_upsert, type_decl, value, value_type,
};
use crate::conformance::{Failure, Held, Store};
use crate::expr::order::cmp_values;

check!(
    DATE_TIME_CANONICAL = date_time_values_read_back_in_their_one_canonical_form,
    "the log format §5: a date-time is kept in UTC as YYYY-MM-DDTHH:MM:SS.ffffffZ, whatever offset it was written with"
);

check!(
    BYTES_ONE_SPELLING = bytes_are_padded_standard_base64_and_nothing_else,
    "the log format §5: bytes are canonical standard base64, padded and with zero pad bits; unpadded, url-safe or non-canonical spellings are refused"
);

check!(
    VECTOR_LENGTH = a_vector_of_the_wrong_length_or_an_infinite_component_is_refused,
    "011 and the log format §5: a vector field declares its dimensions, and a value of another length does not fit it, nor one with a component that is not finite as an `f32` (`1e300`); a vector of zeros fits; a vector derived from a `json` field, written or computed at the read, holding what no vector of the field is (`[1e300, 0, 0]`, `[\"a\", \"b\", \"c\"]`, two numbers) holds no value"
);

check!(
    VALUE_TYPE_RULES = a_value_types_one_of_format_and_scale_hold_on_every_write,
    "011: a value's `one_of`, named `format` and decimal `scale` are the value type's, checked on every write; scale rounds half away from zero"
);

check!(
    NULL_CLEARS_ANY_TYPE = null_clears_a_field_whatever_its_value_type_allows,
    "012 and 011: `null` in a patch clears the field; it is not a value the value type must allow"
);

async fn one_field(store: &dyn LakeStore, name: &str, ty: FieldType) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let v = FieldDecl {
        ty,
        ..field("v", StorageKind::Text)
    };
    l.declare(type_decl(name, vec![v])).await?;
    Ok(l)
}

/// Write `v` to `thing` and expect a refusal on field `v`, with nothing
/// appended.
async fn refused(store: &dyn LakeStore, v: Value, why: &str) -> Held {
    refused_in(store, "thing", v, why).await
}

async fn refused_in(store: &dyn LakeStore, ty: &str, v: Value, why: &str) -> Held {
    let before = store.head().await.must("reading the head")?;
    let what = format!("writing {v} ({why})");
    let err = try_upsert(
        doors(store).as_ref(),
        &owner(store),
        ty,
        "k",
        json!({ "v": v }),
    )
    .await
    .must_fail(&what)?;
    head_unmoved(store, before, &what).await?;
    ensure!(
        matches!(
            refused_on(&err),
            Some(("v", Refusal::Constraint(..) | Refusal::Invalid(..)))
        ),
        "{what} must be refused on field `v`; it failed with {err:?}"
    );
    Ok(())
}

async fn date_time_values_read_back_in_their_one_canonical_form(store: Store) -> Held {
    one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::DateTime),
    )
    .await?;
    let l = Lake::of(store.as_ref());
    for (written, canonical) in [
        ("2026-09-24T07:57:11+02:00", "2026-09-24T05:57:11.000000Z"),
        ("2026-09-24", "2026-09-24T00:00:00.000000Z"),
        ("2026-09-24T05:57:11.5Z", "2026-09-24T05:57:11.500000Z"),
    ] {
        l.upsert("thing", "k", json!({ "v": written })).await?;
        let r = l.read("thing", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(canonical)),
            "`{written}` must read back as `{canonical}`; it reads {:?}",
            value(&r, "v")
        );
    }
    refused(store.as_ref(), json!("2026-09-24T05:57:11"), "no offset").await?;
    refused(store.as_ref(), json!("2026-02-30"), "no such day").await
}

async fn bytes_are_padded_standard_base64_and_nothing_else(store: Store) -> Held {
    let l = one_field(store.as_ref(), "thing", FieldType::kind(StorageKind::Bytes)).await?;
    for ok in ["aGVsbG8=", "aGk=", "+/+/", ""] {
        l.upsert("thing", "k", json!({ "v": ok })).await?;
        let r = l.read("thing", "k").await?;
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

async fn a_vector_of_the_wrong_length_or_an_infinite_component_is_refused(store: Store) -> Held {
    let ty = FieldType {
        dimensions: Some(3),
        ..FieldType::kind(StorageKind::Vector)
    };
    let l = one_field(store.as_ref(), "thing", ty).await?;
    l.upsert("thing", "k", json!({"v": [0.1, 0.2, 0.3]}))
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
    refused(store.as_ref(), json!([0.1, "x", 0.3]), "not all numbers").await?;
    refused(store.as_ref(), json!([1e300, 0, 0]), "infinite as an f32").await?;
    for zeros in [json!([0, 0, 0]), json!([1e-200, 0, 0])] {
        l.upsert("thing", "k", json!({ "v": zeros })).await?;
    }
    let mut v = derived("v", StorageKind::Vector, "raw", &["raw"]);
    v.ty.dimensions = Some(3);
    let w = FieldDecl {
        name: "w".into(),
        computed_at: Some(ComputedAt::Read),
        ..v.clone()
    };
    let fields = vec![field("raw", StorageKind::Json), v, w];
    l.declare(type_decl("from_raw", fields)).await?;
    for raw in [json!([1e300, 0, 0]), json!(["a", "b", "c"]), json!([1, 2])] {
        l.upsert("from_raw", "k", json!({ "raw": [1, 2, 3] }))
            .await?;
        l.upsert("from_raw", "k", json!({ "raw": raw })).await?;
        let r = l.read("from_raw", "k").await?;
        ensure!(
            value(&r, "v").is_none() && value(&r, "w").is_none(),
            "`v` and `w` (computed at the read) are vectors of three derived from `raw`, \
             which holds {raw}: no vector of theirs, so they must hold no value; they read \
             {:?} and {:?}",
            value(&r, "v"),
            value(&r, "w")
        );
    }
    Ok(())
}

async fn typed(store: &dyn LakeStore, ty: &str, vt: ValueTypeDecl) -> Held {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    let kind = vt.kind;
    let name = vt.name.clone();
    let fut = cat.declare(own, Declaration::ValueType(vt));
    fut.await.must(&format!("declaring value type `{name}`"))?;
    one_field(
        store,
        ty,
        FieldType {
            value_type: Some(name),
            ..FieldType::kind(kind)
        },
    )
    .await?;
    Ok(())
}

async fn a_value_types_one_of_format_and_scale_hold_on_every_write(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;

    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    l.upsert("thing", "k", json!({"v": "paid"})).await?;
    refused(store.as_ref(), json!("lost"), "not one of open, paid").await?;

    let email = ValueTypeDecl {
        format: Some("email".into()),
        ..value_type("email", StorageKind::Text)
    };
    typed(store.as_ref(), "contact", email).await?;
    l.upsert("contact", "k", json!({"v": "ada@example.org"}))
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
        l.upsert("payment", "k", json!({ "v": written })).await?;
        let r = l.read("payment", "k").await?;
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
    cat.declare(own, Declaration::ValueType(unknown))
        .await
        .must_fail("declaring a value type with the unknown format `shoe_size`")?;
    head_unmoved(store.as_ref(), before, "declaring an unknown format").await?;
    Ok(())
}

async fn null_clears_a_field_whatever_its_value_type_allows(store: Store) -> Held {
    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("thing", "k", json!({"v": "open"})).await?;
    l.upsert("thing", "k", json!({"v": null})).await?;
    let r = l.read("thing", "k").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear `v` although null is not one of its allowed values; `v` \
         reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

check!(
    DECIMAL_NOT_A_NUMBER = a_decimal_written_as_a_json_number_is_refused,
    "the log format §5: a decimal's one JSON form is a string of digits, so a JSON number, which may already have lost digits, is refused"
);

async fn a_decimal_written_as_a_json_number_is_refused(store: Store) -> Held {
    let l = one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::Decimal),
    )
    .await?;
    l.upsert("thing", "k", json!({"v": "12.50"})).await?;
    refused(store.as_ref(), json!(12.5), "a decimal as a JSON number").await?;
    refused(store.as_ref(), json!(12), "a decimal as a JSON integer").await?;
    refused(store.as_ref(), json!("12,50"), "a decimal with a comma").await
}

check!(
    GEO_VALUES = a_geo_value_reads_back_in_its_one_form_and_a_malformed_one_is_refused,
    "011, extended, and the log format §5: a geo value is a GeoJSON geometry whose numbers take their one JSON form, the same through get and find, live and after a rebuild; empty `coordinates` are the empty geometry of every type alike, while an empty part inside one is refused; an open ring, a missing `type` or a GeoJSON object that is not a geometry is refused naming the field; null clears it"
);

check!(
    JSON_VALUES = a_json_value_reads_back_in_its_one_form_and_null_clears_it,
    "011, extended, and the log format §5: a json value is kept as written but for its numbers, which take their one JSON form (`1`, never `1.0`), the same through get and find, live and after a rebuild; null clears it like any field"
);

check!(
    WRITTEN_FORM_AT_EVERY_DOOR = a_value_its_written_form_would_not_pass_is_refused_at_every_door,
    "the log format §1.2 and §1.3: a value is judged in the form the log writes it in, by one check at every door; a number written as an integer beyond ±(2^53 − 1) (`1e16`, `9007199254740993.0`, `1e17` inside a `json` value) and a date-time an offset carries out of the years 0000 to 9999 are refused by `upsert`, `ingest`, a call whose verb sets a field from its payload and `propose`, on the field or the payload member, appending nothing; the largest integer, `1e21` and an offset that stays within the years are written and read back in their one form"
);

check!(
    COMPUTED_IN_ITS_WRITTEN_FORM = a_computed_value_its_field_would_not_take_written_holds_no_value,
    "the log format §1.2 and §1.3: what an expression computes is judged as what is written is; a derived number that would be written as an integer beyond ±(2^53 − 1) (`n + 1` at the largest integer, `n * 2`, `n * 1.5`) and a derived date-time or date carried out of the years 0000 to 9999 hold no value, through get and find, where the largest integer and the last days hold theirs; a verb that sets such a value is refused on the field it sets, appending nothing"
);

check!(
    GEO_JSON_ORDER = a_page_ordered_by_a_geo_or_json_field_takes_the_rulebooks_fallback_order,
    "029: a geo or json value has no order of its own, so a find ordered by one ranks it as `cmp_values` does, a JSON value by its canonical bytes, a missing value last either way, ties by key; paging walks the same order"
);

/// Every `thing` record's fields as `get` and as `find` answer them, as
/// JSON text by key.
async fn texts(store: &dyn LakeStore, keys: &[&str]) -> Result<Vec<(String, String)>, Failure> {
    let l = Lake::of(store);
    let found = l.all_found("thing", Find::default()).await?;
    let mut out = Vec::new();
    for key in keys {
        let got = l.read("thing", key).await?;
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
async fn one_form(store: Store, cases: &[(&str, Value, &str)]) -> Held {
    let l = Lake::of(store.as_ref());
    for (key, written, _) in cases {
        l.upsert("thing", key, json!({ "v": written })).await?;
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
    store: Store,
) -> Held {
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
    let l = Lake::of(store.as_ref());
    l.upsert("thing", "point", json!({"v": null})).await?;
    let r = l.read("thing", "point").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear the geo field `v`; it reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

async fn a_json_value_reads_back_in_its_one_form_and_null_clears_it(store: Store) -> Held {
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
    let l = Lake::of(store.as_ref());
    l.upsert("thing", "object", json!({"v": null})).await?;
    let r = l.read("thing", "object").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear the json field `v`, not be kept as a JSON value; it reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

async fn a_page_ordered_by_a_geo_or_json_field_takes_the_rulebooks_fallback_order(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "thing",
        vec![field("g", StorageKind::Geo), field("j", StorageKind::Json)],
    ))
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
        l.upsert("thing", key, json!({"g": g, "j": j})).await?;
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
                    order_by: vec![order(name, OrderBy::Value, dir)],
                    limit: Some(limit),
                    ..Find::default()
                };
                let walked: Vec<String> = l
                    .all_found("thing", find)
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

async fn a_value_its_written_form_would_not_pass_is_refused_at_every_door(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let kinds = [
        ("n", StorageKind::Number),
        ("j", StorageKind::Json),
        ("at", StorageKind::DateTime),
    ];
    let shared = |(name, kind): &(&str, StorageKind)| FieldDecl {
        authority: Authority::Shared,
        ..field(name, *kind)
    };
    l.declare(type_decl("thing", kinds.iter().map(shared).collect()))
        .await?;
    for (f, kind) in kinds {
        let payload = json!([{"name": "v", "type": {"kind": kind.to_string()}}]);
        l.declare_json(
            json!({"declare": "verb", "name": format!("put_{f}"), "on": "thing",
            "risk": "low", "effect": "write", "payload": payload,
            "set": [{"field": f, "value": "payload.v"}]}),
        )
        .await?;
    }
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let max = 9_007_199_254_740_991_i64;
    for (f, written, one) in [
        ("n", json!(9007199254740991.0), json!(max)),
        ("n", json!(1e21), json!(1e21)),
        (
            "at",
            json!("9999-12-31T23:30:00+01:00"),
            json!("9999-12-31T22:30:00.000000Z"),
        ),
    ] {
        l.upsert("thing", "k", json!({ f: written })).await?;
        let r = l.read("thing", "k").await?;
        ensure!(
            value(&r, f) == Some(&one),
            "`{f}` written {written} must read back as {one}; it reads {:?}",
            value(&r, f)
        );
    }
    // Each door at a record of its own, so that none answers what another wrote.
    for key in ["upserted", "ingested", "called", "proposed"] {
        l.upsert("thing", key, json!({"n": 1})).await?;
    }
    let before = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for (f, v) in [
        ("n", json!(1e16)),
        ("n", json!(9007199254740993.0)),
        ("j", json!({"a": [1e17]})),
        ("at", json!("9999-12-31T23:30:00-01:00")),
        ("at", json!("0000-01-01T00:30:00+01:00")),
    ] {
        let fields = json!({ f: v });
        let upserted = try_upsert(cat, own, "thing", "upserted", fields.clone()).await;
        let record = source_record("thing", "ingested", fields.clone());
        let ingested = l.ingest("feed", vec![record]).await;
        let (verb, key) = (format!("put_{f}").as_str().into(), "called".into());
        let payload = patch(json!({ "v": v }));
        let called = cat.call(own, &verb, Some(&key), payload, WriteOpts::default());
        let called = called.await;
        let write = ProposedWrite::Upsert {
            ty: "thing".into(),
            key: "proposed".into(),
            patch: patch(fields),
            opts: WriteOpts::default(),
        };
        let proposed = cat.propose(own, write, "later".into()).await;
        for (door, on, got) in [
            ("upsert", f, upserted.map(drop)),
            ("ingest", f, ingested.map(drop)),
            ("a call that sets it from `v`", "v", called.map(drop)),
            ("propose", f, proposed.map(drop)),
        ] {
            let fits = match refused_on(&got) {
                Some((named, Refusal::Constraint(said))) if named == on => match &said.case {
                    Case::IntegerTooLarge { .. } => !v.is_string(),
                    Case::NotDateTime { .. } => v.is_string(),
                    _ => false,
                },
                _ => false,
            };
            if !fits {
                red.push(format!("{door} of `{f}` = {v} answered {got:?}"));
            }
        }
    }
    ensure!(
        red.is_empty(),
        "a number the log writes as an integer beyond ±(2^53 − 1), and a date-time whose \
         instant leaves the years 0000 to 9999, is refused at every door on the field it is \
         sent in, a verb's on its payload member `v`: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), before, "the refused values").await
}

async fn a_computed_value_its_field_would_not_take_written_holds_no_value(store: Store) -> Held {
    use StorageKind::{Date, DateTime, Number};
    let l = Lake::of(store.as_ref());
    let mut fields = vec![
        field("at", DateTime),
        field("n", Number),
        field("set_at", DateTime),
        field("set_n", Number),
    ];
    for (name, kind, expr, dep) in [
        ("after", DateTime, "at + 1d", "at"),
        ("before", DateTime, "at - 1d", "at"),
        ("day", Date, "at + 12h", "at"),
        ("next", Number, "n + 1", "n"),
        ("twice", Number, "n * 2", "n"),
        ("half_more", Number, "n * 1.5", "n"),
    ] {
        fields.push(derived(name, kind, expr, &[dep]));
    }
    l.declare(type_decl("thing", fields)).await?;
    for (verb, f, expr) in [
        ("later", "set_at", "at + 1d"),
        ("earlier", "set_at", "at - 1d"),
        ("count", "set_n", "n + 1"),
        ("grow", "set_n", "n * 1.5"),
    ] {
        l.declare_json(
            json!({"declare": "verb", "name": verb, "on": "thing", "risk": "low",
            "effect": "write", "set": [{"field": f, "value": expr}]}),
        )
        .await?;
    }
    let max = 9_007_199_254_740_991_i64;
    // Each record with what its derived fields hold; one not named holds no value.
    let records = [
        (
            "last",
            json!({"at": "9999-12-31T12:00:00Z", "n": max}),
            json!({"before": "9999-12-30T12:00:00.000000Z"}),
        ),
        (
            "first",
            json!({"at": "0000-01-01T12:00:00Z", "n": -max}),
            json!({"after": "0000-01-02T12:00:00.000000Z", "day": "0000-01-02", "next": 1 - max}),
        ),
        (
            "within",
            json!({"at": "9999-12-30T12:00:00Z", "n": max - 1}),
            json!({"after": "9999-12-31T12:00:00.000000Z", "day": "9999-12-31",
                "before": "9999-12-29T12:00:00.000000Z", "next": max}),
        ),
    ];
    for (key, written, _) in &records {
        l.upsert("thing", key, written.clone()).await?;
    }
    let found = l.all_found("thing", Find::default()).await?;
    let mut red = Vec::new();
    for (key, _, derived) in &records {
        let got = l.read("thing", key).await?;
        let listed = found.iter().find(|r| r.key.as_str() == *key);
        for f in ["after", "before", "day", "next", "twice", "half_more"] {
            let (want, read) = (derived.get(f), value(&got, f));
            let at_find = listed.and_then(|r| value(r, f));
            if read != want || at_find != want {
                red.push(format!(
                    "`{f}` of `{key}` reads {read:?} at get and {at_find:?} at find, not {want:?}"
                ));
            }
        }
    }
    ensure!(
        red.is_empty(),
        "a derived number written as an integer beyond ±(2^53 − 1), and a derived date-time \
         or date out of the years 0000 to 9999, holds no value, and one within them holds \
         its own: {}",
        red.join("; ")
    );
    let before = store.head().await.must("reading the head")?;
    for (verb, key, on) in [
        ("later", "last", "set_at"),
        ("earlier", "first", "set_at"),
        ("count", "last", "set_n"),
        ("grow", "last", "set_n"),
        ("grow", "first", "set_n"),
    ] {
        let (name, k) = (verb.into(), key.into());
        let called = (l.cat).call(
            &l.own,
            &name,
            Some(&k),
            patch(json!({})),
            WriteOpts::default(),
        );
        let called = called.await.map(drop);
        let fits = match refused_on(&called) {
            Some((named, Refusal::Constraint(said))) if named == on => match &said.case {
                Case::IntegerTooLarge { .. } => on == "set_n",
                Case::NotDateTime { .. } => on == "set_at",
                _ => false,
            },
            _ => false,
        };
        if !fits {
            red.push(format!("`{verb}` on `{key}` answered {called:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "a verb that sets a number written as an integer beyond ±(2^53 − 1), or a date-time \
         out of the years 0000 to 9999, is refused on the field it sets: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), before, "the refused calls").await?;
    for (verb, f, set) in [
        ("later", "set_at", json!("9999-12-31T12:00:00.000000Z")),
        ("count", "set_n", json!(max)),
    ] {
        let (name, k) = (verb.into(), "within".into());
        let called = (l.cat).call(
            &l.own,
            &name,
            Some(&k),
            patch(json!({})),
            WriteOpts::default(),
        );
        called
            .await
            .must("a call that sets a value within the range")?;
        let r = l.read("thing", "within").await?;
        ensure!(
            value(&r, f) == Some(&set),
            "`{verb}` on `within` must set `{f}` to {set}; it reads {:?}",
            value(&r, f)
        );
    }
    Ok(())
}
