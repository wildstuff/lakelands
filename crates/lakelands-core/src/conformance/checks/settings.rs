//! A lake's settings are records of `sys.setting`, read in any expression
//! by `setting('<key>')` as they stood at the position being read
//! (`docs/seams.md`, `SystemVocabulary`). A setting is data with a
//! history, so a read pinned to an earlier position answers with the
//! setting of that position, not today's.

use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::Seams;
use lakelands_api::{Access, Find, Grants, LakeStore, Position, PrincipalKind};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, all_found, derived, field, find_where, principal, read_at, restrict, type_decl,
};
use crate::conformance::{Failure, Held, Reopen, Store};

check!(
    SETTING_AS_OF = a_setting_is_read_as_it_stood_at_the_position_read,
    "seams, SystemVocabulary: setting('key') in an expression reads the sys.setting record with that key as of the position being read"
);

async fn a_setting_is_read_as_it_stood_at_the_position_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("band", vec![field("level", StorageKind::Number)]))
        .await?;
    l.upsert("band", "low", json!({"level": 1})).await?;
    l.upsert("band", "high", json!({"level": 2})).await?;
    let first = l.set("threshold", json!(1)).await?.position();
    let second = l.set("threshold", json!(2)).await?.position();

    let at = |as_of: Option<Position>| Find {
        filter: Some("level = setting('threshold')".into()),
        at: read_at(as_of),
        ..Find::default()
    };
    let keys = |records: Vec<lakelands_api::Record>| -> Vec<String> {
        records.into_iter().map(|r| r.key.0).collect()
    };
    for (as_of, want, when) in [
        (Some(first), "low", "as of the first setting"),
        (Some(second), "high", "as of the second setting"),
        (None, "high", "at the head"),
    ] {
        let got = keys(l.all_found("band", at(as_of)).await?);
        ensure!(
            got == [want],
            "`threshold` was set to 1 at {first} and to 2 at {second}; `find` with \
             `level = setting('threshold')` {when} answered {got:?}, where only `{want}` \
             matches the setting as it stood there"
        );
    }
    Ok(())
}

check!(
    SETTING_RESTRICTED = a_restricted_setting_stays_what_the_lake_computes_with,
    "seams, SystemVocabulary (lakelands#583): restricting a setting hides its record and never its value from the lake's computations or from an expression: a derived field written while the writer stays open, after the lake is opened again and after a rebuild reads it, a read's `where` and a view's `where` and measure read it, and a reader without the `restricted` grant is not listed the record and is answered by `setting('key')` in its own `where` what the lake computes with",
    reopens
);

async fn a_restricted_setting_stays_what_the_lake_computes_with(
    store: Store,
    reopen: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    let gross = derived(
        "gross",
        StorageKind::Number,
        "net * setting('rate')",
        &["net"],
    );
    let fields = vec![
        field("kind", StorageKind::Text),
        field("net", StorageKind::Number),
        gross,
    ];
    l.declare(type_decl("line", fields)).await?;
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    l.declare_json(json!({"declare": "type", "name": "book", "key": ["kind"],
        "fields": [{"name": "kind", "type": {"kind": "text"}}, number("total"), number("n")],
        "view": {"source": "line", "group_by": ["kind"], "where": "net * setting('rate') = 6",
            "measures": [{"name": "total", "expr": "sum(net * setting('rate'))"},
                {"name": "n", "expr": "count()"}]}}))
        .await?;
    l.set("rate", json!(2)).await?;
    l.upsert("line", "a", json!({"kind": "k", "net": 3}))
        .await?;
    restrict(store.as_ref(), "sys.setting", "rate", "kept").await?;
    l.upsert("line", "b", json!({"kind": "k", "net": 3}))
        .await?;
    let open = readings(store.as_ref()).await?;
    let again = reopen(store, Seams::default()).await;
    let store = again.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    Lake::of(store.as_ref())
        .upsert("line", "c", json!({"kind": "k", "net": 3}))
        .await?;
    let reopened = readings(store.as_ref()).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = readings(store.as_ref()).await?;
    let two = "gross [6.0, 6.0]; where 2 and 2; book 12 of 2; listed 0";
    let three = "gross [6.0, 6.0, 6.0]; where 3 and 3; book 18 of 3; listed 0";
    ensure!(
        open == two && reopened == three && rebuilt == three,
        "`rate` is 2 and restricted: with the writer open the lake reads `{open}`, opened \
         again and a third record written `{reopened}`, rebuilt `{rebuilt}`; it is `{two}` \
         and then `{three}`"
    );
    Ok(())
}

/// What the lake computes with a restricted `rate`: each line's `gross`,
/// how many lines a `where` on the setting answers the owner and a reader
/// without the `restricted` grant, the view's record, and how many
/// settings that reader is listed.
async fn readings(store: &dyn LakeStore) -> Result<String, Failure> {
    let l = Lake::of(store);
    let grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let plain = principal(store, PrincipalKind::Agent, "plain", grants);
    let number = |r: &lakelands_api::Record, f: &str| r.fields.get(f).and_then(|v| v.as_f64());
    let lines = l.all_found("line", Find::default()).await?;
    let gross: Vec<f64> = lines.iter().filter_map(|r| number(r, "gross")).collect();
    let by = find_where("net * setting('rate') = 6");
    let owner = l.all_found("line", by.clone()).await?.len();
    let reader = all_found(l.cat.as_ref(), &plain, "line", by).await?.len();
    let book = l.all_found("book", Find::default()).await?;
    let book = book.first().map(|r| (number(r, "total"), number(r, "n")));
    let (total, n) = book.unwrap_or_default();
    let listed = all_found(l.cat.as_ref(), &plain, "sys.setting", Find::default()).await?;
    Ok(format!(
        "gross {gross:?}; where {owner} and {reader}; book {} of {}; listed {}",
        total.unwrap_or(0.0),
        n.unwrap_or(0.0),
        listed.len()
    ))
}

check!(
    SETTING_AS_OF_THE_WRITE = a_field_that_reads_a_setting_is_as_of_its_records_last_write,
    "seams, SystemVocabulary and 048 (lakelands#523): a derived field whose expression reads a setting is computed with the setting of its record's last write; the setting changed and the type declared again with one more field that is not computed leave it as it was, live and after a rebuild, and a write of the record brings it up to date"
);

async fn a_field_that_reads_a_setting_is_as_of_its_records_last_write(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let fields = || {
        let d = derived("d", StorageKind::Number, "x * setting('rate')", &["x"]);
        vec![field("x", StorageKind::Number), d]
    };
    l.declare(type_decl("priced", fields())).await?;
    l.set("rate", json!(2)).await?;
    l.upsert("priced", "a", json!({"x": 1})).await?;
    l.set("rate", json!(3)).await?;
    let mut wider = fields();
    wider.push(field("note", StorageKind::Text));
    l.declare(type_decl("priced", wider)).await?;
    let d = || async {
        let a = l.read("priced", "a").await?;
        Ok::<_, Failure>(a.fields.get("d").and_then(|v| v.as_f64()))
    };
    let live = d().await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = d().await?;
    l.upsert("priced", "a", json!({"x": 1})).await?;
    let written = d().await?;
    ensure!(
        (live, rebuilt, written) == (Some(2.0), Some(2.0), Some(3.0)),
        "`a` was written while `rate` was 2; `rate` is 3 and the type was declared again \
         with a plain field more: `d` reads {live:?} live, {rebuilt:?} after a rebuild and \
         {written:?} after a write of the record. It is 2 until the record is written, \
         then 3"
    );
    Ok(())
}

check!(
    VIEW_KEPT_HOLDS_A_BOOLEAN = a_view_kept_setting_holds_true_or_false,
    "049: a `view_kept` setting whose value is no boolean (a text, a number, a list, an object) is refused as invalid, naming `value`, appending nothing; `false`, written where no setting stood, asks for nothing, as no setting does"
);

async fn a_view_kept_setting_holds_true_or_false(store: Store) -> Held {
    use crate::conformance::fixture::{
        count_view, head_unmoved, invalid_case, refused_on, try_upsert,
    };
    use lakelands_api::describe::{Kept, NotKept};
    use lakelands_api::error::Case;
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("part", vec![field("g", StorageKind::Text)]))
        .await?;
    l.declare(count_view("tally", "part", "g")).await?;
    l.upsert("part", "p1", json!({"g": "a"})).await?;
    let tally = lakelands_api::TypeName::from("tally");
    let asked = || async {
        let told = (l.cat).describe_type(&l.own, &tally, Default::default());
        Ok::<_, Failure>(told.await.must("describing `tally`")?.data.lifecycle.kept)
    };
    let unset = asked().await?;
    let head = store.head().await.must("reading the head")?;
    let (ty, key, mut wrong) = (
        lakelands_api::seam::SETTING_TYPE,
        "view_kept.tally",
        Vec::new(),
    );
    for value in [json!("yes"), json!(1), json!([true]), json!({"kept": true})] {
        let setting = json!({"key": key, "value": value});
        let said = try_upsert(l.cat.as_ref(), &l.own, ty, key, setting).await;
        let on_value = matches!(refused_on(&said), Some(("value", _)));
        if !on_value || invalid_case(&said) != Some(&Case::SettingKeptHolds) {
            wrong.push(format!("{value} answered {said:?}"));
        }
    }
    head_unmoved(store.as_ref(), head, "a `view_kept` that is no boolean").await?;
    l.set(key, json!(false)).await?;
    let off = asked().await?;
    let not_asked = Some(Kept::Computed {
        why: NotKept::NotAsked,
    });
    ensure!(
        wrong.is_empty() && unset == not_asked && off == not_asked,
        "a `view_kept` setting that is no boolean is refused as invalid naming `value`: \
         {wrong:?}; with no setting the view is told as {unset:?}, and under `false` as {off:?}"
    );
    Ok(())
}

check!(
    SETTINGS_TOLD = describe_tells_the_lake_s_settings_to_a_holder_of_schema,
    "063 (lakelands#1172): `describe` tells a holder of `schema` each listed lake setting for the lake, with what the lake holds and its default, and each one the lake holds for a type, with the lake's value as its default, and none of a host's own keys; it tells no other reader any; to a holder of `schema` kept from a type or a field, a setting is told as were the type or the field absent: nothing of the type, and of a type it is shown only the fields it reads, as of the type's twin without the field; a key under a listed setting's prefix that names none of them is refused, naming the key and the prefix, appending nothing"
);

async fn describe_tells_the_lake_s_settings_to_a_holder_of_schema(store: Store) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{field_access, head_unmoved, invalid_said, try_upsert};
    use lakelands_api::error::{Case, Place, Said};
    use lakelands_api::settings::{Layer, PerType, SETTINGS};
    use serde_json::Value;
    let l = Lake::of(store.as_ref());
    let number = |n: &str| field(n, StorageKind::Number);
    // `bare` is `row` without `m`: the twin of `row` to one kept from `m`.
    for (ty, fields) in [
        ("row", vec!["n", "m"]),
        ("bare", vec!["n"]),
        ("secret", vec!["x"]),
    ] {
        l.declare(type_decl(ty, fields.into_iter().map(number).collect()))
            .await?;
    }
    for (ty, fields) in [
        ("row", json!(["n", "m"])),
        ("bare", json!(["n"])),
        ("secret", json!(["x"])),
    ] {
        l.set(&format!("sort_index.{ty}"), fields).await?;
    }
    l.set("threshold", json!(1)).await?;
    let told = |d: lakelands_api::describe::LakeDescription| {
        let s = d.settings.into_iter();
        s.map(|s| (s.key, s.value, s.default)).collect::<Vec<_>>()
    };
    let owner = told(l.describe().await?);
    let wide = SETTINGS
        .iter()
        .filter(|s| s.layer == Layer::Lake && s.per_type != PerType::Only);
    let wide: Vec<_> = wide
        .map(|s| (s.key.to_owned(), None, s.default_value()))
        .collect();
    let typed = |ty: &str, v: Value| (format!("sort_index.{ty}"), Some(v), json!(null));
    let mut want = wide.clone();
    want.push(typed("bare", json!(["n"])));
    want.push(typed("row", json!(["n", "m"])));
    want.push(typed("secret", json!(["x"])));
    let reader = field_access(store.as_ref(), "reader", "row", &[]);
    let theirs = l.cat.describe(&reader, Default::default()).await;
    let theirs = told(theirs.must("describing the lake as a reader")?.data);
    ensure!(
        owner == want && theirs.is_empty(),
        "the owner, who holds `schema`, is told {owner:?}, where {want:?}; a reader without \
         `schema` is told {theirs:?}, where nothing"
    );
    // A holder of `schema` kept from `secret` and from `row.m` is told
    // `row` as the owner is told its twin `bare`, and nothing of `secret`,
    // as were the type absent.
    let arch = holder(
        store.as_ref(),
        "arch",
        json!({"default": "write", "schema": true, "types": {
            "secret": {"default": "none"}, "row": {"fields": {"m": "none"}}}}),
    );
    let arch = l.cat.describe(&arch, Default::default()).await;
    let arch = told(arch.must("describing the lake to arch")?.data);
    let twin = (owner.iter())
        .find(|(k, ..)| k == "sort_index.bare")
        .map(|(_, v, _)| v.clone());
    let mut as_absent = wide;
    as_absent.push(typed("bare", json!(["n"])));
    as_absent.push(typed("row", json!(["n"])));
    let row = (arch.iter())
        .find(|(k, ..)| k == "sort_index.row")
        .map(|(_, v, _)| v.clone());
    ensure!(
        arch == as_absent && row == twin,
        "a holder of `schema` kept from `secret` and `row.m` is told {arch:?}, where \
         {as_absent:?}: `row` as its twin `bare` ({twin:?}), and nothing of `secret`"
    );
    let head = store.head().await.must("reading the head")?;
    let (ty, key) = (lakelands_api::seam::SETTING_TYPE, "sort_index");
    let said = try_upsert(
        l.cat.as_ref(),
        &l.own,
        ty,
        key,
        json!({"key": key, "value": ["n"]}),
    )
    .await;
    let want = Said {
        at: vec![Place::Setting { key: key.into() }],
        case: Case::SettingUnlisted {
            prefix: "sort_index".into(),
        },
    };
    ensure!(
        invalid_said(&said) == Some(&want),
        "`sort_index`, under the prefix of the lake's own `sort_index.<type>` and naming no \
         type, is refused as {want:?}; it answered {said:?}"
    );
    head_unmoved(store.as_ref(), head, "an unlisted setting").await
}
