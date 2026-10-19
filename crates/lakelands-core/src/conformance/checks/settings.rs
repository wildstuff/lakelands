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

check!(
    SETTING_RECORDS_TOLD = a_setting_record_is_told_as_were_the_hidden_absent,
    "063 (lakelands#1198): a record of `sys.setting` for a type is answered to a reader kept from the type, or from a field it names, as were the type or the field absent, at every read door: `find` with its total, `get`, `setting('…')` in a read's `where`, and a proposal's write; nothing of the type, and of a type it is shown only the fields it reads, none left being no record; a `where` over the records reads each value as told; a write by that reader keeps the fields it is not told after those it writes, and a null it writes is no value; its retire clears only what it is told, accepted from its proposal too, which is listed to it as a retire and refused at the accept once the record changed; a write without a value of a record it is not told is refused whatever version it expects; its write is judged as written before its version, so an expected version tells nothing of a type it is not shown"
);

async fn a_setting_record_is_told_as_were_the_hidden_absent(store: Store) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{get_at, upsert};
    use lakelands_api::write::{ProposalFilter, ProposedWrite};
    use lakelands_api::{Error, RecordKey, TypeName};
    let l = Lake::of(store.as_ref());
    let number = |n: &str| field(n, StorageKind::Number);
    // `bare` is `row` without `m`: the twin of `row` to one kept from `m`.
    l.declare(type_decl(
        "row",
        vec![number("k"), number("n"), field("m", StorageKind::Text)],
    ))
    .await?;
    l.declare(type_decl("bare", vec![number("k"), number("n")]))
        .await?;
    l.declare(type_decl("secret", vec![number("x")])).await?;
    l.upsert("bare", "b1", json!({"n": 1})).await?;
    for (key, value) in [
        ("sort_index.row", json!(["n", "m"])),
        ("lookup_index.row", json!(["m"])),
        ("sort_index.bare", json!(["n"])),
        ("sort_index.secret", json!(["x"])),
        ("threshold", json!(1)),
    ] {
        l.set(key, value).await?;
    }
    let arch = holder(
        store.as_ref(),
        "arch",
        json!({"default": "write", "review": true, "types": {
            "secret": {"default": "none"}, "row": {"fields": {"m": "none"}}}}),
    );
    let ty = "sys.setting";
    let listed = |rows: Vec<lakelands_api::Record>| -> Vec<(String, Option<serde_json::Value>)> {
        let value = |r: &lakelands_api::Record| r.fields.get("value").cloned();
        rows.iter().map(|r| (r.key.0.clone(), value(r))).collect()
    };
    let counted = Find {
        total: true,
        ..Find::default()
    };
    let total = |who| {
        let (cat, counted) = (l.cat.clone(), counted.clone());
        async move {
            let found = cat.find(who, &TypeName::from(ty), counted).await;
            match found.must("finding the settings with their total")?.data {
                lakelands_api::Found::Records { total, .. } => Ok::<_, Failure>(total),
                _ => Ok(None),
            }
        }
    };
    // As were `secret` and `row.m` absent: `row` told as its twin `bare`.
    let theirs = listed(all_found(l.cat.as_ref(), &arch, ty, Find::default()).await?);
    let want = vec![
        ("sort_index.bare".to_owned(), Some(json!(["n"]))),
        ("sort_index.row".to_owned(), Some(json!(["n"]))),
        ("threshold".to_owned(), Some(json!(1))),
    ];
    let counts = (total(&arch).await?, total(&l.own).await?);
    ensure!(
        theirs == want && counts == (Some(3), Some(5)),
        "a reader kept from `secret` and `row.m` finds the settings {theirs:?}, where {want:?}, \
         and is counted {counts:?} of them beside the owner, where (Some(3), Some(5))"
    );
    let get = |key: &str| get_at(key, None);
    for key in ["sort_index.secret", "lookup_index.row"] {
        let got = l.cat.get(&arch, &TypeName::from(ty), get(key)).await;
        let never = l
            .cat
            .get(&arch, &TypeName::from(ty), get("sort_index.never"))
            .await;
        ensure!(
            matches!(
                (&got, &never),
                (Err(Error::NotFound { .. }), Err(Error::NotFound { .. }))
            ),
            "`get` of `{key}` answers the reader kept from what it names {got:?}, where as \
             for a key never set ({never:?})"
        );
    }
    for (filter, owner, reader) in [
        ("setting('sort_index.secret') is not null", 1, 0),
        ("setting('lookup_index.row') is not null", 1, 0),
        (
            "setting('sort_index.row') = setting('sort_index.bare')",
            0,
            1,
        ),
    ] {
        let by = find_where(filter);
        let got = (
            l.all_found("bare", by.clone()).await?.len(),
            all_found(l.cat.as_ref(), &arch, "bare", by).await?.len(),
        );
        ensure!(
            got == (owner, reader),
            "`where {filter}` over `bare` answers the owner and the reader kept from `secret` \
             and `row.m` {got:?} records, where ({owner}, {reader})"
        );
    }
    // A `where` over the records reads each value as it is told.
    let by = find_where("value = setting('sort_index.bare')");
    let keys = |rows: Vec<lakelands_api::Record>| -> Vec<String> {
        rows.into_iter().map(|r| r.key.0).collect()
    };
    let got = (
        keys(l.all_found(ty, by.clone()).await?),
        keys(all_found(l.cat.as_ref(), &arch, ty, by).await?),
    );
    let want = (
        vec!["sort_index.bare".to_owned()],
        vec!["sort_index.bare".to_owned(), "sort_index.row".to_owned()],
    );
    ensure!(
        got == want,
        "`where value = setting('sort_index.bare')` over the settings finds the owner and the \
         reader kept from `row.m` {got:?}, where {want:?}: each value as it is told"
    );
    // A proposal is told as its write would be: nothing of `secret`, and
    // of `row` only what the reader reads, none left being nothing.
    for (key, value) in [
        ("sort_index.secret", json!([])),
        ("lookup_index.row", json!(["m"])),
        ("sort_index.row", json!(["k", "m"])),
    ] {
        let write = ProposedWrite::Upsert {
            ty: TypeName::from(ty),
            key: RecordKey::from(key),
            patch: crate::conformance::fixture::patch(json!({"key": key, "value": value})),
            opts: Default::default(),
        };
        l.cat
            .propose(&l.own, write, "an index".into())
            .await
            .must("proposing a setting")?;
    }
    let told = l.cat.proposals(&arch, ProposalFilter::default()).await;
    let told: Vec<_> = (told.must("listing the proposals")?.data.into_iter())
        .map(|p| match p.write {
            Some(ProposedWrite::Upsert { key, patch, .. }) => {
                Some((key.0, patch.get("value").cloned()))
            }
            _ => None,
        })
        .collect();
    let want = [Some(("sort_index.row".to_owned(), Some(json!(["k"]))))];
    ensure!(
        told == want,
        "the owner proposed `sort_index.secret`, `lookup_index.row` naming `m` and \
         `sort_index.row` naming `k` and `m`; the reader kept from `secret` and `m` is told \
         {told:?}, where {want:?}"
    );
    // A write keeps what its writer is not told, after what it writes; a
    // retire clears only what it is told.
    upsert(
        l.cat.as_ref(),
        &arch,
        ty,
        "sort_index.row",
        json!({"key": "sort_index.row", "value": ["k", "n"]}),
    )
    .await?;
    let owner = l
        .read(ty, "sort_index.row")
        .await?
        .fields
        .get("value")
        .cloned();
    let read =
        crate::conformance::fixture::read(l.cat.as_ref(), &arch, ty, "sort_index.row").await?;
    let theirs = read.fields.get("value").cloned();
    let want = (Some(json!(["k", "n", "m"])), Some(json!(["k", "n"])));
    ensure!(
        (owner.clone(), theirs.clone()) == want,
        "the owner's `sort_index.row` named `n` and `m`; the reader kept from `m` wrote `k` and \
         `n`: the owner reads {owner:?} and the reader {theirs:?}, where {want:?}"
    );
    // A null it writes is no value, as over its twin, whatever it is not told.
    let null = |key: &str| {
        let fields = crate::conformance::fixture::patch(json!({"key": key, "value": null}));
        let (cat, arch, t, k) = (l.cat.clone(), arch.clone(), TypeName::from(ty), key.into());
        async move { cat.upsert(&arch, &t, &k, fields, Default::default()).await }
    };
    let (said, twin) = (null("sort_index.row").await, null("sort_index.bare").await);
    let alike = |e: &lakelands_api::Result<lakelands_api::Outcome>| {
        format!("{e:?}").replace("sort_index.bare", "sort_index.row")
    };
    ensure!(
        said.is_err() && alike(&said) == alike(&twin),
        "a null `value` written by the reader kept from `m` over `sort_index.row` answers \
         {said:?}, where as over its twin `sort_index.bare`: {twin:?}"
    );
    let retired = l
        .cat
        .retire(
            &arch,
            &TypeName::from(ty),
            &RecordKey::from("sort_index.row"),
            Default::default(),
        )
        .await;
    retired.must("retiring `sort_index.row` as the reader kept from `m`")?;
    let owner = l
        .read(ty, "sort_index.row")
        .await?
        .fields
        .get("value")
        .cloned();
    let theirs = l
        .cat
        .get(&arch, &TypeName::from(ty), get("sort_index.row"))
        .await;
    ensure!(
        owner == Some(json!(["m"])) && matches!(theirs, Err(Error::NotFound { .. })),
        "the reader kept from `m` retired `sort_index.row`: the owner reads {owner:?}, where \
         [\"m\"], and the reader {theirs:?}, where no record"
    );
    // A retire it proposes is listed to it as a retire, is refused at the
    // accept once the record changed, and, accepted, clears only what it
    // is told too.
    l.set("sort_index.row", json!(["n", "m"])).await?;
    let propose = || {
        let retire = ProposedWrite::Retire {
            ty: ty.into(),
            key: "sort_index.row".into(),
            opts: Default::default(),
        };
        let (cat, arch) = (l.cat.clone(), arch.clone());
        async move {
            let proposed = cat.propose(&arch, retire, "no index".into()).await;
            match proposed.must("proposing the retire of `sort_index.row`")? {
                lakelands_api::Outcome::Proposed { proposal, .. } => Ok(proposal),
                other => Err(Failure::new(format!("proposing answered {other:?}"))),
            }
        }
    };
    let id = propose().await?;
    let mine = l.cat.proposals(&arch, ProposalFilter::default()).await;
    let mine: Vec<_> = (mine.must("listing the proposals")?.data.into_iter())
        .filter(|p| p.id == id)
        .map(|p| matches!(p.write, Some(ProposedWrite::Retire { .. })))
        .collect();
    l.set("sort_index.row", json!(["m", "n"])).await?;
    let stale = l.cat.accept(&l.own, &id).await;
    ensure!(
        mine == [true] && format!("{stale:?}").contains("WriteChangedSinceProposed"),
        "the reader kept from `m` proposed the retire of `sort_index.row`: it is listed as \
         a retire ({mine:?}, where [true]), and its accept after the owner wrote the record \
         answers {stale:?}, where refused as changed since proposed"
    );
    let id = propose().await?;
    let accepted = l.cat.accept(&l.own, &id).await;
    let owner = l
        .read(ty, "sort_index.row")
        .await?
        .fields
        .get("value")
        .cloned();
    ensure!(
        accepted.is_ok() && owner == Some(json!(["m"])),
        "the reader kept from `m` proposed the retire of `sort_index.row` naming `n` and `m`, \
         and the owner accepted it ({accepted:?}): the owner reads {owner:?}, where [\"m\"]"
    );
    // A record it is not told is none to its write: one without a value
    // is refused, whatever version it expects, as at a key never set.
    for expected in [None, Some(0), Some(1), Some(5)] {
        let write = |key: &str| {
            let opts = lakelands_api::WriteOpts {
                expected_version: expected.map(lakelands_api::Version),
                ..Default::default()
            };
            let fields = crate::conformance::fixture::patch(json!({"key": key}));
            let (cat, arch) = (l.cat.clone(), arch.clone());
            let (t, k) = (TypeName::from(ty), RecordKey::from(key));
            async move { cat.upsert(&arch, &t, &k, fields, opts).await }
        };
        let (said, never) = (
            write("lookup_index.row").await,
            write("lookup_index.bare").await,
        );
        let alike = |e: &lakelands_api::Result<lakelands_api::Outcome>| {
            format!("{e:?}").replace("lookup_index.bare", "lookup_index.row")
        };
        ensure!(
            said.is_err() && alike(&said) == alike(&never),
            "a write without a value of `lookup_index.row`, naming only `m`, expecting \
             {expected:?} by the reader kept from `m` answers {said:?}, where as at a key never \
             set: {never:?}"
        );
    }
    // Written, it expects the version of a key never set, and keeps `m`.
    let valued = |key: &str, expected: u64| {
        let opts = lakelands_api::WriteOpts {
            expected_version: Some(lakelands_api::Version(expected)),
            ..Default::default()
        };
        let fields = crate::conformance::fixture::patch(json!({"key": key, "value": []}));
        let (cat, arch) = (l.cat.clone(), arch.clone());
        let (t, k) = (TypeName::from(ty), RecordKey::from(key));
        async move { cat.upsert(&arch, &t, &k, fields, opts).await }
    };
    let (said, never) = (
        valued("lookup_index.row", 1).await,
        valued("lookup_index.bare", 1).await,
    );
    let alike = |e: &lakelands_api::Result<lakelands_api::Outcome>| {
        format!("{e:?}").replace("lookup_index.bare", "lookup_index.row")
    };
    let written = valued("lookup_index.row", 0).await;
    let owner = l
        .read(ty, "lookup_index.row")
        .await?
        .fields
        .get("value")
        .cloned();
    ensure!(
        said.is_err()
            && alike(&said) == alike(&never)
            && written.is_ok()
            && owner == Some(json!(["m"])),
        "the reader kept from `m` writing `[]` to `lookup_index.row`, naming only `m`, \
         expecting its version 1 answers {said:?}, where as at a key never set: {never:?}; \
         expecting 0 it answers {written:?} and the owner reads {owner:?}, where [\"m\"]"
    );
    // Judged as written before its version: an expected version tells
    // nothing of a type it is not shown.
    for expected in [None, Some(0), Some(1)] {
        let write = |key: &str| {
            let opts = lakelands_api::WriteOpts {
                expected_version: expected.map(lakelands_api::Version),
                ..Default::default()
            };
            let fields = crate::conformance::fixture::patch(json!({"key": key, "value": ["x"]}));
            let (cat, arch) = (l.cat.clone(), arch.clone());
            let (t, k) = (TypeName::from(ty), RecordKey::from(key));
            async move { cat.upsert(&arch, &t, &k, fields, opts).await }
        };
        let (said, never) = (
            write("sort_index.secret").await,
            write("sort_index.never").await,
        );
        let alike = |e: &lakelands_api::Result<lakelands_api::Outcome>| {
            format!("{e:?}").replace("never", "secret")
        };
        ensure!(
            alike(&said) == alike(&never),
            "a write of `sort_index.secret` expecting {expected:?} by the reader kept from \
             `secret` answers {said:?}, where as for a type never declared: {never:?}"
        );
    }
    Ok(())
}

check!(
    SETTING_RECORDS_PAGED = settings_are_paged_counted_and_proposed_as_told,
    "063 (lakelands#1198): to a reader kept from a type or a field, the records of `sys.setting` are paged, counted and proposed as were these absent: a graph from `sys.setting` pages through each record it is told, at every limit, cut exactly where another page follows; a kept view counting them by group counts what a `find` tells it; a proposal is listed as the write it is told, with nothing left out of it: one over a record it is not told as over a key never set, its own too, a narrowed one whole, and one that leaves a record it is told none as a retire, also once that record is retired; the version a write answers over a record it is not told counts the writes it is not told (a stated limit, lakelands#1210)"
);

async fn settings_are_paged_counted_and_proposed_as_told(store: Store) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{count_view, patch, upsert};
    use lakelands_api::query::{Pattern, QueryResult};
    use lakelands_api::write::{ProposalFilter, ProposedWrite};
    use lakelands_api::{Outcome, RecordKey, TypeName, Version};
    use std::collections::BTreeMap;
    let l = Lake::of(store.as_ref());
    let number = |n: &str| field(n, StorageKind::Number);
    let text = |n: &str| field(n, StorageKind::Text);
    let row = vec![text("k"), number("n"), text("m")];
    l.declare(type_decl("row", row)).await?;
    l.declare(type_decl("secret", vec![number("x")])).await?;
    // A record it is not told after each it is told: `lookup_index.row`
    // names only `m`, `sort_index.secret` a type it is not shown.
    for (key, value) in [
        ("lookup_index.row", json!(["m"])),
        ("sort_index.row", json!(["n", "m"])),
        ("sort_index.secret", json!(["x"])),
        ("threshold", json!(1)),
    ] {
        l.set(key, value).await?;
    }
    let arch = holder(
        store.as_ref(),
        "arch",
        json!({"default": "write", "review": true, "types": {
            "secret": {"default": "none"}, "row": {"fields": {"m": "none"}}}}),
    );
    let ty = "sys.setting";
    let keys = |rows: &[lakelands_api::Record]| -> Vec<String> {
        rows.iter().map(|r| r.key.0.clone()).collect()
    };
    let found = all_found(l.cat.as_ref(), &arch, ty, Find::default()).await?;
    let told = keys(&found);
    // Paged a record at a time and two at a time, each page cut while
    // another follows.
    for limit in [1, 2] {
        let (mut paged, mut cut, mut cursor) = (Vec::new(), Vec::new(), None);
        while cut.len() <= told.len() {
            let what = format!("a graph of the settings at limit {limit}");
            let asked = json!({"start": {"type": ty}, "project": {"as": "graph"}, "limit": limit});
            let mut p: Pattern = serde_json::from_value(asked)
                .map_err(|e| Failure::new(format!("{what} is no pattern: {e}")))?;
            p.cursor = cursor.take();
            let QueryResult::Graph {
                nodes,
                truncated,
                next_cursor,
                ..
            } = l.cat.query(&arch, p).await.must(&what)?.data
            else {
                return Err(Failure::new(format!("{what} answered another shape")));
            };
            paged.extend(keys(&nodes));
            cut.push(truncated);
            match next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        let mut want = vec![true; told.len().div_ceil(limit)];
        if let Some(last) = want.last_mut() {
            *last = false;
        }
        ensure!(
            paged == told && cut == want,
            "a graph of the settings at limit {limit} pages the reader kept from `secret` and \
             `row.m` through {paged:?}, cut {cut:?}; it finds {told:?}, so the pages are cut \
             {want:?}"
        );
    }
    // A kept view counts the settings as the reader is told them.
    l.declare(count_view("tally", ty, "group")).await?;
    l.set("view_kept.tally", json!(true)).await?;
    let group = |r: &lakelands_api::Record| r.fields.get("group").map(|g| g.to_string());
    let mut want: BTreeMap<_, f64> = BTreeMap::new();
    // A setting without a group is in none.
    for r in all_found(l.cat.as_ref(), &arch, ty, Find::default()).await? {
        if let Some(g) = group(&r) {
            *want.entry(Some(g)).or_default() += 1.0;
        }
    }
    let counted = all_found(l.cat.as_ref(), &arch, "tally", Find::default()).await?;
    let n = |r: &lakelands_api::Record| r.fields.get("n").and_then(|v| v.as_f64());
    let got: BTreeMap<_, _> = counted
        .iter()
        .map(|r| (group(r), n(r).unwrap_or(0.0)))
        .collect();
    ensure!(
        got == want,
        "the kept view `tally` counts the settings by group to the reader kept from `secret` \
         and `row.m` as {got:?}; its `find` tells it {want:?}"
    );
    // Each proposal as the write it is told, nothing more left out.
    let proposal = |key: &str, value: serde_json::Value| ProposedWrite::Upsert {
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        patch: patch(json!({"key": key, "value": value})),
        opts: Default::default(),
    };
    let mut ids = Vec::new();
    for (who, key, value) in [
        (&l.own, "lookup_index.row", json!(["k", "m"])),
        (&l.own, "sort_index.row", json!(["k", "m"])),
        (&l.own, "sort_index.row", json!(["m"])),
        (&arch, "lookup_index.row", json!(["k"])),
    ] {
        let proposed = l.cat.propose(who, proposal(key, value), "an index".into());
        match proposed.await.must("proposing a setting")? {
            Outcome::Proposed { proposal, .. } => ids.push(proposal),
            other => return Err(Failure::new(format!("proposing answered {other:?}"))),
        }
    }
    let listing = || async {
        let listed = l.cat.proposals(&arch, ProposalFilter::default()).await;
        let listed =
            (listed.must("listing the proposals")?.data.into_iter()).map(|p| match p.write {
                Some(ProposedWrite::Upsert { key, patch, .. }) => {
                    (key.0, patch.get("value").cloned(), p.more)
                }
                Some(ProposedWrite::Retire { key, .. }) => (key.0, None, p.more),
                other => (format!("{other:?}"), None, p.more),
            });
        Ok::<_, Failure>(listed.collect::<Vec<_>>())
    };
    let listed = listing().await?;
    // Newest first.
    let want = vec![
        ("lookup_index.row".to_owned(), Some(json!(["k"])), false),
        ("sort_index.row".to_owned(), None, false),
        ("sort_index.row".to_owned(), Some(json!(["k"])), false),
        ("lookup_index.row".to_owned(), Some(json!(["k"])), false),
    ];
    ensure!(
        listed == want,
        "the owner proposed `lookup_index.row` naming `k` and `m` over one naming `m`, \
         `sort_index.row` naming `k` and `m` and then `m` alone, and the reader kept from `m` \
         `lookup_index.row` naming `k`: the reader is listed {listed:?} (key, value, more), \
         where {want:?}"
    );
    // Listed as it was proposed, whatever became of the record since: the
    // owner's `["m"]` accepted, the record is not told to the reader.
    let accepted = l.cat.accept(&l.own, &ids[2]).await;
    accepted.must("accepting the owner's `sort_index.row` naming `m`")?;
    let after = listing().await?;
    ensure!(
        after == want,
        "the owner accepted its `sort_index.row` naming only `m`, listed to the reader kept \
         from `m` as a retire: the reader is listed {after:?}, where as before, {want:?}"
    );
    // And so once the owner retires that record: a retired record is a
    // free key, as one never set (lakelands#1198).
    l.retire(ty, "sort_index.row").await?;
    let retired = listing().await?;
    ensure!(
        retired == want,
        "the owner retired `sort_index.row`, which the reader kept from `m` is not told: the \
         reader is listed {retired:?}, where as before, {want:?}"
    );
    // Its write over a record it is not told answers the record's version,
    // which counts the owner's write: pinned until lakelands#1210.
    let written = upsert(
        l.cat.as_ref(),
        &arch,
        ty,
        "lookup_index.row",
        json!({"key": "lookup_index.row", "value": ["k"]}),
    )
    .await?;
    let version = match written {
        Outcome::Applied { version, .. } => version,
        _ => None,
    };
    ensure!(
        version == Some(Version(2)),
        "the reader kept from `m` wrote `lookup_index.row` over the owner's, naming only `m`: \
         it answers {version:?}, where Some(Version(2)), the record's own (063 states this \
         limit; lakelands#1210 ends it)"
    );
    Ok(())
}
