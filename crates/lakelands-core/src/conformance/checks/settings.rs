//! A lake's settings are records of `sys.setting`, read in any expression
//! by `setting('<key>')` as they stood at the position being read
//! (`docs/seams.md`, `SystemVocabulary`). A setting is data with a
//! history, so a read pinned to an earlier position answers with the
//! setting of that position, not today's.

use crate::conformance::fixture::derived;
use crate::conformance::prelude::*;

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
    "063 (lakelands#1172): `describe` tells a holder of `schema` each listed lake setting for the lake, with what the lake holds and its default, and each one the lake holds for a type, with the lake's value as its default, and none of a host's own keys; it tells no other reader any; to a holder of `schema` kept from a type, a setting is told as were the type absent, and one naming the fields of a type it is shown is told whole to one that reads the type whole, else not at all; a key under a listed setting's prefix that names none of them is refused, naming the key and the prefix, appending nothing"
);

async fn describe_tells_the_lake_s_settings_to_a_holder_of_schema(store: Store) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{field_access, head_unmoved, invalid_said, try_upsert};
    use lakelands_api::error::{Case, Place, Said};
    use lakelands_api::settings::{Layer, PerType, SETTINGS};
    use serde_json::Value;
    let l = Lake::of(store.as_ref());
    let number = |n: &str| field(n, StorageKind::Number);
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
    // nothing of either, and `bare` whole.
    let arch = holder(
        store.as_ref(),
        "arch",
        json!({"default": "write", "schema": true, "types": {
            "secret": {"default": "none"}, "row": {"fields": {"m": "none"}}}}),
    );
    let arch = l.cat.describe(&arch, Default::default()).await;
    let arch = told(arch.must("describing the lake to arch")?.data);
    let mut as_absent = wide;
    as_absent.push(typed("bare", json!(["n"])));
    ensure!(
        arch == as_absent,
        "a holder of `schema` kept from `secret` and `row.m` is told {arch:?}, where \
         {as_absent:?}: nothing of `row` or of `secret`"
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
    "063 (lakelands#1198, lakelands#1250): a record of `sys.setting` for a type is answered to a reader kept from the type as were the type absent, and one that names the type's fields is told whole or not at all: to a reader kept from any field of the type it is no record, at every read door: `find` with its total, `get`, `setting('…')` in a read's `where`, and the proposals listed to it; that reader's write, retire or proposal of such a setting is refused as not granted, naming no field, whatever value or version it gives, before the record is read; a write of a type's setting by a writer not shown the type is judged as written before its version, so an expected version tells nothing of it"
);

async fn a_setting_record_is_told_as_were_the_hidden_absent(store: Store) -> Held {
    use super::only::{holder, not_found, not_granted};
    use crate::conformance::fixture::{get_at, head_unmoved, patch};
    use lakelands_api::write::{ProposalFilter, ProposedWrite};
    use lakelands_api::{RecordKey, TypeName, WriteOpts};
    let l = Lake::of(store.as_ref());
    let number = |n: &str| field(n, StorageKind::Number);
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
    let ty = TypeName::from("sys.setting");
    let counted = Find {
        total: true,
        ..Find::default()
    };
    let total = async |who| {
        let found = l.cat.find(who, &ty, counted.clone()).await;
        match found.must("finding the settings with their total")?.data {
            lakelands_api::Found::Records { total, .. } => Ok::<_, Failure>(total),
            _ => Ok(None),
        }
    };
    let theirs = all_found(l.cat.as_ref(), &arch, "sys.setting", Find::default()).await?;
    let theirs: Vec<_> = theirs.into_iter().map(|r| r.key.0).collect();
    let counts = (total(&arch).await?, total(&l.own).await?);
    ensure!(
        theirs == ["sort_index.bare", "threshold"] && counts == (Some(2), Some(5)),
        "a reader kept from `secret` and `row.m` finds the settings {theirs:?}, where \
         `sort_index.bare` and `threshold`, and is counted {counts:?} of them beside the \
         owner, where (Some(2), Some(5))"
    );
    let hidden = ["sort_index.secret", "lookup_index.row", "sort_index.row"];
    for key in hidden {
        let got = l.cat.get(&arch, &ty, get_at(key, None)).await;
        not_found(&format!("`get` of `{key}` by the reader kept from it"), got)?;
        let by = find_where(&format!("setting('{key}') is not null"));
        let got = (
            l.all_found("bare", by.clone()).await?.len(),
            all_found(l.cat.as_ref(), &arch, "bare", by).await?.len(),
        );
        ensure!(
            got == (1, 0),
            "`where setting('{key}') is not null` over `bare` answers the owner and the reader \
             kept from it {got:?} records, where (1, 0)"
        );
    }
    let upsert = |key: &str, value: serde_json::Value| ProposedWrite::Upsert {
        ty: ty.clone(),
        key: RecordKey::from(key),
        patch: patch(json!({"key": key, "value": value})),
        opts: Default::default(),
    };
    for key in hidden {
        let proposed = l
            .cat
            .propose(&l.own, upsert(key, json!([])), "an index".into());
        proposed.await.must("proposing a setting")?;
    }
    let told = l.cat.proposals(&arch, ProposalFilter::default()).await;
    let told = told.must("listing the proposals")?.data;
    ensure!(
        told.is_empty(),
        "the owner proposed {hidden:?}; the reader kept from them is listed {told:?}, where none"
    );
    // Each write of a setting naming `row`'s fields is refused it whole,
    // before the record is read.
    let head = store.head().await.must("reading the head")?;
    let (row, key) = (RecordKey::from("sort_index.row"), "sort_index.row");
    for expected in [None, Some(0), Some(1), Some(5)] {
        let opts = WriteOpts {
            expected_version: expected.map(lakelands_api::Version),
            ..Default::default()
        };
        for value in [json!(["k", "n"]), json!(null)] {
            let fields = patch(json!({"key": key, "value": value}));
            let said = l.cat.upsert(&arch, &ty, &row, fields, opts.clone()).await;
            not_granted(&format!("arch's `{value}` expecting {expected:?}"), said)?;
        }
        let said = l.cat.retire(&arch, &ty, &row, opts).await;
        not_granted(&format!("arch's retire expecting {expected:?}"), said)?;
    }
    let retire = ProposedWrite::Retire {
        ty: ty.clone(),
        key: row.clone(),
        opts: Default::default(),
    };
    for write in [upsert(key, json!(["k"])), retire] {
        not_granted(
            "arch's proposal",
            l.cat.propose(&arch, write, "an index".into()).await,
        )?;
    }
    head_unmoved(store.as_ref(), head, "a setting refused its writer").await?;
    // Judged as written before its version: an expected version tells
    // nothing of a type it is not shown.
    for expected in [None, Some(0), Some(1)] {
        let write = async |key: &str| {
            let opts = WriteOpts {
                expected_version: expected.map(lakelands_api::Version),
                ..Default::default()
            };
            let fields = patch(json!({"key": key, "value": ["x"]}));
            l.cat.upsert(&arch, &ty, &key.into(), fields, opts).await
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
    SETTING_RECORDS_PAGED = settings_are_paged_counted_and_written_as_told,
    "063 (lakelands#1198, lakelands#1250): to a reader kept from a type or a field, the records of `sys.setting` are paged and counted as were the ones not told absent: a graph from `sys.setting` pages through each record it is told, at every limit, cut exactly where another page follows, and a kept view counting them by group counts what a `find` tells it; a setting naming fields of a type its writer does not read whole answers that writer as in a twin lake where the field was never declared, or the setting never written: a write expecting no version, and one expecting version 0, are refused alike in both, as not granted, and a `get` is not found in both, while the type's lifecycle names what it sorts and indexes as the twin's does",
    given
);

async fn settings_are_paged_counted_and_written_as_told(store: Store, given: Given) -> Held {
    use super::only::holder;
    use crate::conformance::fixture::{count_view, get_at, patch};
    use lakelands_api::query::{Pattern, QueryResult};
    use lakelands_api::{TypeName, Version, WriteOpts};
    use std::collections::BTreeMap;
    let number = |n: &str| field(n, StorageKind::Number);
    // `row` keeps `m` from `arch`, `col` masks `n` from `mask`; the twin
    // never declared `row.m` and holds no setting.
    let lakes = async |s: &dyn LakeStore, m: bool| {
        let l = Lake::of(s);
        let mut row = vec![field("k", StorageKind::Text), number("n")];
        if m {
            row.push(field("m", StorageKind::Text));
        }
        l.declare(type_decl("row", row)).await?;
        let col = vec![number("k"), number("n"), number("m")];
        l.declare(type_decl("col", col)).await?;
        Ok::<_, Failure>(l)
    };
    let l = lakes(store.as_ref(), true).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = lakes(twin.as_ref(), false).await?;
    l.declare(type_decl("secret", vec![number("x")])).await?;
    l.declare(type_decl("bare", vec![number("n")])).await?;
    // Records it is not told around each it is told.
    for (key, value) in [
        ("lookup_index.row", json!(["m"])),
        ("sort_index.bare", json!(["n"])),
        ("sort_index.col", json!(["n"])),
        ("sort_index.row", json!(["n", "m"])),
        ("sort_index.secret", json!(["x"])),
        ("threshold", json!(1)),
    ] {
        l.set(key, value).await?;
    }
    let arch = holder(
        store.as_ref(),
        "arch",
        json!({"default": "write", "types": {
            "secret": {"default": "none"}, "row": {"fields": {"m": "none"}}}}),
    );
    let mask = holder(
        store.as_ref(),
        "mask",
        json!({"default": "write", "types": {"col": {"fields": {"n": "masked"}}}}),
    );
    let ty = "sys.setting";
    let keys = |rows: &[lakelands_api::Record]| -> Vec<String> {
        rows.iter().map(|r| r.key.0.clone()).collect()
    };
    let found = all_found(l.cat.as_ref(), &arch, ty, Find::default()).await?;
    let told = keys(&found);
    ensure!(
        told == ["sort_index.bare", "sort_index.col", "threshold"],
        "the reader kept from `secret` and `row.m` finds the settings {told:?}"
    );
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
    // A setting naming a field its writer is not shown, or shown masked,
    // answers it as the twin does, where the field or the setting is not.
    let (ty, mut wrong) = (TypeName::from(ty), Vec::new());
    let refused = lakelands_api::Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::NotGranted,
    };
    for (who, key, value, described) in [
        (&arch, "lookup_index.row", json!(["k"]), "row"),
        (&mask, "sort_index.col", json!(["m"]), "col"),
    ] {
        for expected in [None, Some(Version(0))] {
            let write = async |l: &Lake| {
                let fields = patch(json!({"key": key, "value": value}));
                let opts = WriteOpts {
                    expected_version: expected,
                    ..Default::default()
                };
                l.cat.upsert(who, &ty, &key.into(), fields, opts).await
            };
            let (here, there) = (write(&l).await, write(&t).await);
            if here.as_ref().err() != Some(&refused) || format!("{here:?}") != format!("{there:?}")
            {
                wrong.push(format!(
                    "`{}` expecting {expected:?}: {here:?}; twin: {there:?}",
                    who.id
                ));
            }
        }
        let get = async |l: &Lake| {
            let got = l.cat.get(who, &ty, get_at(key, None)).await;
            matches!(got, Err(lakelands_api::Error::NotFound { .. }))
        };
        if !(get(&l).await && get(&t).await) {
            wrong.push(format!("`{}` reads `{key}`", who.id));
        }
        // Its type's lifecycle names what the setting sorts or indexes as
        // the twin's does.
        let described = TypeName::from(described);
        let named = async |l: &Lake| {
            let d = l.cat.describe_type(who, &described, read_at(None));
            let d = d.await.must("describing the type")?.data.lifecycle;
            Ok::<_, Failure>((d.sorted, d.indexed))
        };
        let (here, there) = (named(&l).await?, named(&t).await?);
        if here != there {
            wrong.push(format!(
                "`{}` is told `{described}` sorts and indexes {here:?}; twin: {there:?}",
                who.id
            ));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a setting naming a field its writer does not read must answer it as its twin, \
         refused as not granted: {wrong:?}"
    );
    Ok(())
}

check!(
    SETTING_IN_A_CONDITION = a_condition_reads_a_setting_as_its_reader_is_told_it,
    "063, extended: a condition judged once for every reader reads `setting('…')` as the lake holds it, and what it decides is told only to a principal told every setting it reads: to a reader kept from `secret`, a lake whose `sort_index.secret` it is not told answers as a twin that never set it a trigger's firing over a type it reads and over a kept view whose `where` reads the setting, a set's membership in a write's answer (fired, joined and left; not its position) and in `in_set`, the view read, a verb's `requires` and a value it sets, the owner's proposed call of that verb listed and accepted by the reader, and an aging rule whose `when` reads it, refused for every author, while the owner's write fires both triggers",
    given
);

async fn a_condition_reads_a_setting_as_its_reader_is_told_it(store: Store, given: Given) -> Held {
    // The lake first: a store that answers nothing says so before a twin
    // is asked for.
    let (here, fired) = conditions_told(store.as_ref(), true).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let (there, _) = conditions_told(twin.as_ref(), false).await?;
    ensure!(
        fired == ["t", "tv"],
        "the owner's write of `bare` fires {fired:?}, where `t` and `tv`"
    );
    ensure!(
        here == there,
        "a reader kept from `secret` is answered {here:?} where the lake holds \
         `sort_index.secret`; twin: {there:?}"
    );
    Ok(())
}

/// What a reader kept from `secret` is answered of conditions that read
/// `sort_index.secret`, which the lake holds where `held`; and what the
/// owner's write of `bare` fires.
async fn conditions_told(
    s: &dyn LakeStore,
    held: bool,
) -> Result<(Vec<String>, Vec<String>), Failure> {
    use super::only::holder;
    use crate::conformance::fixture::{
        object_set_on, patch, trigger_on, try_upsert, view_of, write_verb,
    };
    use lakelands_api::write::{Declaration, Outcome};
    const READ: &str = "setting('sort_index.secret') is not null";
    let l = Lake::of(s);
    let text = |n: &str| field(n, StorageKind::Text);
    l.declare(type_decl("secret", vec![text("x")])).await?;
    l.declare(type_decl("bare", vec![text("m")])).await?;
    let n = field("n", StorageKind::Number);
    let mut vw = view_of(
        "vw",
        "bare",
        &["m"],
        vec![text("m"), n],
        &[("n", "count()")],
    );
    if let Some(v) = vw.view.as_mut() {
        v.r#where = Some(READ.into());
    }
    l.declare(vw).await?;
    l.set("view_kept.vw", json!(true)).await?;
    // `v` requires the setting; `w` sets a value read from it.
    let mut v = write_verb("v", "bare", "m", "'c'");
    if let Declaration::Verb(v) = &mut v {
        v.requires = Some(READ.into());
    }
    let w = write_verb("w", "bare", "m", &format!("if({READ}, 'y', 'n')"));
    let set = object_set_on("s", "bare", READ);
    for d in [
        trigger_on("t", "bare", Some(READ)),
        trigger_on("tv", "vw", None),
        set,
        v,
        w,
    ] {
        l.declare_as(d, "a reaction").await?;
    }
    if held {
        l.set("sort_index.secret", json!(["x"])).await?;
    }
    let fired = match l.upsert("bare", "o", json!({"m": "a"})).await? {
        Outcome::Applied { set_off, .. } => set_off.fired.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    };
    let grants = json!({"default": "write", "restricted": true, "review": true,
        "types": {"secret": {"default": "none"}}});
    let (arch, cat) = (holder(s, "arch", grants), l.cat.as_ref());
    let said = |r: lakelands_api::Result<Outcome>| match r {
        Ok(Outcome::Applied { set_off: s, .. }) => {
            format!("{:?} {:?} {:?}", s.fired, s.joined, s.left)
        }
        r => format!("{:?}", r.err()),
    };
    let found = async |ty: &str, filter: &str| {
        let f = cat.find(&arch, &ty.into(), find_where(filter)).await;
        f.map(|f| format!("{:?}", f.data))
            .unwrap_or_else(|e| format!("{e:?}"))
    };
    let mut told = vec![said(
        try_upsert(cat, &arch, "bare", "a", json!({"m": "a"})).await,
    )];
    told.push(found("bare", "in_set('s')").await);
    told.push(found("vw", "n > 0").await);
    let d = cat
        .describe(&arch, Default::default())
        .await
        .must("describing the lake")?
        .data;
    let names = (d.triggers.iter().map(|t| t.name.to_string()))
        .chain(d.object_sets.iter().map(|s| s.name.clone()))
        .chain(d.verbs.iter().map(|v| v.name.to_string()));
    told.push(format!("{:?}", names.collect::<Vec<_>>()));
    for (verb, key) in [("v", "o"), ("w", "o")] {
        let (verb, key) = (verb.into(), key.into());
        let called = cat.call(
            &arch,
            &verb,
            Some(&key),
            patch(json!({})),
            Default::default(),
        );
        told.push(said(called.await));
    }
    told.push(found("bare", "m = 'y'").await);
    // The owner's proposed call of `v`, which the twin refuses to propose:
    // not listed, and not found to accept.
    let call = lakelands_api::write::ProposedWrite::Call {
        verb: "v".into(),
        key: Some("o".into()),
        payload: patch(json!({})),
        opts: Default::default(),
    };
    let accepted = match cat.propose(&l.own, call, "why".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => cat.accept(&arch, &proposal).await.is_ok(),
        _ => false,
    };
    let listed = cat.proposals(&arch, Default::default()).await;
    told.push(format!("{:?} {accepted}", listed.map(|p| p.data.len())));
    let rule =
        json!({"name": "r", "on": "bare", "field": "m", "from": "a", "to": "b", "when": READ});
    let ruled = try_upsert(cat, &arch, lakelands_api::seam::AGING_RULE_TYPE, "r", rule).await;
    told.push(said(ruled));
    let today = lakelands_api::time::Timestamp::from_date(2026, 3, 1).expect("a date");
    s.age(today).await.must("aging")?;
    told.push(found("bare", "m = 'b'").await);
    Ok((told, fired))
}

/// Declares the setting `key` of `kind` and sets it to `value`.
pub(super) async fn declared(l: &Lake, key: &str, kind: &str, value: Value) -> Held {
    let d = json!({"declare": "setting", "key": key, "type": {"kind": kind}});
    l.declare_json(d).await?;
    l.set(key, value).await.map(drop)
}

check!(
    SETTING_AT_A_DATE = a_setting_is_read_on_the_day_asked,
    "090 (lakelands#1549): `setting('key', at)` answers the value of the period that holds `at`'s day: invoices supplied on either side of a period's `from` and written after it was added, or written again, read its two values, one written before it was added keeps the value it read until written again, a `date_time` on the day before reads the first value, and a rebuild and a reopen answer the same",
    reopens
);

async fn a_setting_is_read_on_the_day_asked(store: Store, reopen: Reopen) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "vat_rate", "decimal", json!("0.16")).await?;
    let vat = "net * setting('vat_rate', supplied_on)";
    let fields = vec![
        field("net", StorageKind::Decimal),
        field("supplied_on", StorageKind::Date),
        field("issued", StorageKind::DateTime),
        derived("vat", StorageKind::Decimal, vat, &["net", "supplied_on"]),
    ];
    l.declare(type_decl("invoice", fields)).await?;
    let issued = "2026-06-30T23:59:59Z";
    let on = |day: &str| json!({"net": "100", "supplied_on": day, "issued": issued});
    l.upsert("invoice", "early", on("2026-07-01")).await?;
    l.upsert("invoice", "again", on("2026-07-01")).await?;
    let period = json!({"key": "vat_rate", "from": "2026-07-01", "value": "0.19"});
    l.upsert(lakelands_api::seam::SETTING_TYPE, "vat_rate", period)
        .await?;
    l.upsert("invoice", "june", on("2026-06-30")).await?;
    l.upsert("invoice", "july", on("2026-07-01")).await?;
    l.upsert("invoice", "again", on("2026-07-01")).await?;
    let open = vats(store.as_ref()).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = vats(store.as_ref()).await?;
    let again = reopen(store, Seams::default()).await;
    let store = again.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let reopened = vats(store.as_ref()).await?;
    let want = "again 19, early 16, july 19, june 16; issued at 0.16: 4";
    ensure!(
        open == want && rebuilt == want && reopened == want,
        "`vat_rate` is 0.16 and 0.19 from 2026-07-01; the invoices read `{open}` live, \
         `{rebuilt}` rebuilt and `{reopened}` reopened, where they read `{want}`"
    );
    Ok(())
}

/// Each invoice's `vat`, and how many a `where` reading the setting on
/// their `issued` instant finds at 0.16.
async fn vats(store: &dyn LakeStore) -> Result<String, Failure> {
    let l = Lake::of(store);
    let shown = |r: &lakelands_api::Record| {
        let vat = crate::conformance::fixture::decimal(r.fields.get("vat"));
        let vat = vat.map(|d| d.normalize().to_string()).unwrap_or_default();
        format!("{} {vat}", r.key.0)
    };
    let all = l.all_found("invoice", Find::default()).await?;
    let mut all: Vec<String> = all.iter().map(shown).collect();
    all.sort();
    let by = find_where("setting('vat_rate', issued) = 0.16");
    let found = l.all_found("invoice", by).await?.len();
    Ok(format!("{}; issued at 0.16: {found}", all.join(", ")))
}

check!(
    SETTING_ON_THE_CLOCK = a_setting_read_without_a_date_is_read_on_the_clock,
    "090 (lakelands#1549): `setting('key')` answers the value of the period that holds the clock's day: a derived field written now reads the period in force, and a `where` reading it, as a field computed at the read, answers by each period at a `valid_at` inside it"
);

async fn a_setting_read_without_a_date_is_read_on_the_clock(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "rate", "number", json!(1)).await?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    for (from, value) in [("2000-01-01", 2), ("9000-01-01", 3)] {
        let period = json!({"key": "rate", "from": from, "value": value});
        l.upsert(ty, "rate", period).await?;
    }
    let mut read = derived("read", StorageKind::Number, "x + setting('rate')", &["x"]);
    read.computed_at = Some(lakelands_api::ontology::ComputedAt::Read);
    let d = derived("d", StorageKind::Number, "x + setting('rate')", &["x"]);
    let fields = vec![field("x", StorageKind::Number), d, read];
    l.declare(type_decl("t", fields)).await?;
    l.upsert("t", "a", json!({"x": 0})).await?;
    let r = l.read("t", "a").await?;
    let d = r.fields.get("d").and_then(Value::as_f64);
    let mut wrong = Vec::new();
    for (day, want) in [(Some("1999-12-31"), 1), (None, 2), (Some("9000-01-01"), 3)] {
        let valid_at = day.map(|d| Timestamp::denoted(d).expect("an instant"));
        for filter in [
            format!("setting('rate') = {want}"),
            format!("read = {want}"),
        ] {
            let find = Find {
                filter: Some(filter.as_str().into()),
                at: ReadAt {
                    valid_at,
                    ..ReadAt::default()
                },
                ..Find::default()
            };
            let n = l.all_found("t", find).await?.len();
            if n != 1 {
                wrong.push(format!("`{filter}` at {day:?} found {n}"));
            }
        }
    }
    ensure!(
        d == Some(2.0) && wrong.is_empty(),
        "`rate` is 1, 2 from 2000-01-01 and 3 from 9000-01-01: `d` written now reads {d:?}, \
         where it reads 2, and a `where` on the setting finds the record by each period's \
         value at a `valid_at` inside it: {wrong:?}"
    );
    Ok(())
}

check!(
    SETTING_UNDATED_AS_BEFORE = an_undated_setting_reads_as_before,
    "090 (lakelands#1549): a setting with no periods answers `setting('k', d)` as `setting('k')` on any day, `describe` tells it no `periods` and its record holds none; a lake of format 20 imported and opened by a writer declares `sys.setting` once more, one version on, and appends no other event of its own",
    given
);

async fn an_undated_setting_reads_as_before(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("t", vec![field("x", StorageKind::Number)]))
        .await?;
    l.upsert("t", "a", json!({"x": 1})).await?;
    declared(&l, "k", "number", json!(5)).await?;
    let mut wrong = Vec::new();
    for at in ["'1999-12-31'", "'2026-07-01'", "'9000-01-01'", "now()"] {
        let same = format!("setting('k', {at}) = setting('k') and setting('k') = 5");
        if l.all_found("t", find_where(&same)).await?.len() != 1 {
            wrong.push(at);
        }
    }
    let told = l.describe().await?.settings;
    let periods = told
        .into_iter()
        .find(|s| s.key == "k")
        .map(|s| s.periods.len());
    let record = l.read(lakelands_api::seam::SETTING_TYPE, "k").await?;
    ensure!(
        wrong.is_empty() && periods == Some(0) && !record.fields.contains_key("periods"),
        "`k` holds 5 and no periods: `setting('k', d)` differs from `setting('k')` on \
         {wrong:?}, `describe` tells {periods:?} periods, and the record is {:?}",
        record.fields
    );
    // The kept lake of format 20, imported and opened by a writer.
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/20");
    let lake = lakelands_api::LakeId::from("kept20");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept20.keys.json"), &lake)?;
    let seams = super::erasable::with(&Arc::new(keys));
    let imported = (given.import)(dir.clone(), lake, None, seams).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing lake 20: {e}")))?;
    // The import opens it for writing: what its release made ends at
    // the head its answers name.
    let answers = std::fs::read(dir.join("answers.json")).ok();
    let answers = answers.and_then(|a| serde_json::from_slice::<Value>(&a).ok());
    let made = answers.and_then(|a| a["head"].as_u64()).unwrap_or(0) as usize;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let setting = |e: &Event| match &e.body {
        EventBody::Declared {
            declaration: Declaration::Type(t),
            version,
            ..
        } if t.name.as_str() == ty => Some((
            *version,
            t.fields.iter().any(|f| f.name.as_str() == "periods"),
        )),
        _ => None,
    };
    let log = log_events(imported.store.as_ref()).await?;
    let (before, after) = log.split_at((made + 1).min(log.len()));
    let was = before.iter().rev().find_map(setting).map(|(v, _)| v);
    let own = ["lease_taken", "lake_upgraded"];
    let tail: Vec<_> = after
        .iter()
        .filter(|e| !own.contains(&e.body.kind()))
        .collect();
    let kinds: Vec<_> = tail.iter().map(|e| e.body.kind()).collect();
    let declared = (tail.len() == 1).then(|| setting(tail[0])).flatten();
    ensure!(
        declared.is_some() && declared == was.map(|v| (v + 1, true)),
        "a writer's open of lake 20 appends, beside its lease and upgrade, one `declared` of \
         `{ty}` with `periods`, one version on from {was:?}: {kinds:?}, {declared:?}"
    );
    Ok(())
}

check!(
    SETTING_AT_MISFIT = an_at_that_is_no_instant_reads_no_value,
    "090 (lakelands#1549): an `at` that is null answers null; one that is no instant, a text such as `soon` or a JSON value, is the misfit `setting_takes_date`, on a key with periods and on one with none, so a derived field reading it is cleared and a filter reading it matches nothing; an `at` the checker knows to be a number, a literal or a number field, is refused `kind_takes_date` where it is declared"
);

async fn an_at_that_is_no_instant_reads_no_value(store: Store) -> Held {
    use lakelands_api::expr::kind::Kind;
    let l = Lake::of(store.as_ref());
    for key in ["rate", "plain"] {
        declared(&l, key, "number", json!(1)).await?;
    }
    let period = json!({"key": "rate", "from": "2000-01-01", "value": 2});
    l.upsert(lakelands_api::seam::SETTING_TYPE, "rate", period)
        .await?;
    let reads = [
        ("a", "setting('rate', when)"),
        ("b", "setting('rate', blob)"),
        ("c", "setting('plain', when)"),
        ("d", "setting('plain', blob)"),
    ];
    let mut fields = vec![
        field("when", StorageKind::Text),
        field("blob", StorageKind::Json),
        field("gone", StorageKind::Date),
    ];
    for (name, expr) in reads {
        fields.push(derived(name, StorageKind::Number, expr, &["when", "blob"]));
    }
    l.declare(type_decl("m", fields)).await?;
    l.upsert("m", "r", json!({"when": "soon", "blob": {"day": 1}}))
        .await?;
    let r = l.read("m", "r").await?;
    let held: Vec<_> = reads.iter().filter_map(|(n, _)| r.fields.get(*n)).collect();
    let mut matched = Vec::new();
    for (_, expr) in reads {
        for filter in [format!("{expr} = 2"), format!("{expr} is null")] {
            if !l.all_found("m", find_where(&filter)).await?.is_empty() {
                matched.push(filter);
            }
        }
    }
    let null = find_where("setting('rate', gone) is null");
    let null = l.all_found("m", null).await?.len();
    ensure!(
        held.iter().all(|v| v.is_null()) && matched.is_empty() && null == 1,
        "an `at` of `soon` or of a JSON value clears the fields reading it ({held:?}) and \
         matches no filter ({matched:?}); a null `at` answers null, found {null} times"
    );
    // Known to be a number where it is declared.
    let mut refused = Vec::new();
    for at in ["5", "n"] {
        let expr = format!("setting('rate', {at})");
        let d = derived("d", StorageKind::Number, &expr, &["n"]);
        let decl = type_decl("bad", vec![field("n", StorageKind::Number), d]);
        let said = l.try_declare(decl).await;
        let number = Kind::Number;
        if !matches!(invalid_case(&said), Some(Case::KindTakesDate { kind, .. }) if *kind == number)
        {
            refused.push(format!("{expr}: {said:?}"));
        }
    }
    ensure!(
        refused.is_empty(),
        "an `at` known to be a number is refused `kind_takes_date` at the declaration: \
         {refused:?}"
    );
    Ok(())
}

check!(
    SETTING_PERIODS_AS_OF_THE_EVENT = a_record_keeps_the_period_of_its_write,
    "090 (lakelands#1549): a field that read a period keeps what it read after the period is corrected, live, after a rebuild and after a reopen, and reads the correction when written again; a read as of before the correction tells the periods as they stood",
    reopens
);

async fn a_record_keeps_the_period_of_its_write(store: Store, reopen: Reopen) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "vat_rate", "decimal", json!("0.16")).await?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let period = json!({"key": "vat_rate", "from": "2026-07-01", "value": "0.19"});
    l.upsert(ty, "vat_rate", period).await?;
    let vat = "net * setting('vat_rate', supplied_on)";
    let fields = vec![
        field("net", StorageKind::Decimal),
        field("supplied_on", StorageKind::Date),
        derived("vat", StorageKind::Decimal, vat, &["net", "supplied_on"]),
    ];
    l.declare(type_decl("invoice", fields)).await?;
    let july = json!({"net": "100", "supplied_on": "2026-07-01"});
    let before = l.upsert("invoice", "july", july.clone()).await?.position();
    let corrected = json!({"key": "vat_rate", "period": "2026-07-01", "value": "0.20"});
    l.upsert(ty, "vat_rate", corrected).await?;
    let vat = async |store: &dyn LakeStore| {
        let r = Lake::of(store).read("invoice", "july").await?;
        let d = crate::conformance::fixture::decimal(r.fields.get("vat"));
        Ok::<_, Failure>(d.map(|d| d.normalize().to_string()))
    };
    let live = vat(store.as_ref()).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = vat(store.as_ref()).await?;
    let again = reopen(store, Seams::default()).await;
    let store = again.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let reopened = vat(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    l.upsert("invoice", "july", july).await?;
    let written = vat(store.as_ref()).await?;
    let kept = [live, rebuilt, reopened].map(|v| v.unwrap_or_default());
    ensure!(
        kept == ["19", "19", "19"] && written.as_deref() == Some("20"),
        "an invoice written while 0.19 held from 2026-07-01 reads {kept:?} after the period \
         is corrected to 0.20, live, rebuilt and reopened, and {written:?} written again"
    );
    let told = l.cat.describe(&l.own, read_at(Some(before))).await;
    let told = told.must("describing as of before the correction")?.data;
    let told = told.settings.into_iter().find(|s| s.key == "vat_rate");
    let told = told.iter().flat_map(|s| &s.periods);
    let periods: Vec<_> = told.map(|p| format!("{:?} {}", p.from, p.value)).collect();
    let want = [r#"None "0.16""#, r#"Some("2026-07-01") "0.19""#];
    ensure!(
        periods == want,
        "as of before the correction, `describe` tells `vat_rate`'s periods {periods:?}, \
         where they were {want:?}"
    );
    Ok(())
}

check!(
    SETTING_DATED_NOT_KEPT = what_the_fold_keeps_reads_no_dated_setting_on_the_clock,
    "090 (lakelands#1549): a key read without a date by a kept view or a set decided at the write takes no first period, refused `setting_dated_kept` naming the view, then the set; a dated key refuses `view_kept` for such a view and the declaration of such a set; a view reading the key on a date is kept and moves at each period added"
);

async fn what_the_fold_keeps_reads_no_dated_setting_on_the_clock(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "rate", "number", json!(1)).await?;
    let fields = vec![
        field("kind", StorageKind::Text),
        field("net", StorageKind::Number),
    ];
    l.declare(type_decl("line", fields)).await?;
    let view = |name: &str, rate: &str| {
        json!({"declare": "type", "name": name, "key": ["kind"],
            "fields": [{"name": "kind", "type": {"kind": "text"}},
                {"name": "total", "type": {"kind": "number"}}],
            "view": {"source": "line", "group_by": ["kind"],
                "measures": [{"name": "total", "expr": format!("sum(round(net * {rate}))")}]}})
    };
    l.declare_json(view("book", "setting('rate')")).await?;
    l.set("view_kept.book", json!(true)).await?;
    let hi = object_set_on("hi", "line", "net * setting('rate') > 5");
    l.declare_as(hi.clone(), "declaring `hi`").await?;
    l.upsert("line", "a", json!({"kind": "k", "net": 3}))
        .await?;
    let ty = lakelands_api::seam::SETTING_TYPE;
    let add = |from: &str, value: i64| json!({"key": "rate", "from": from, "value": value});
    let dated = |name: &str| Case::SettingDatedKept {
        key: "rate".into(),
        name: name.into(),
    };
    let mut wrong = Vec::new();
    for name in ["book", "hi"] {
        let head = store.head().await.must("reading the head")?;
        let said = try_upsert(l.cat.as_ref(), &l.own, ty, "rate", add("2026-07-01", 2)).await;
        let on_from = matches!(refused_on(&said), Some(("from", _)));
        if !on_from || invalid_case(&said) != Some(&dated(name)) {
            wrong.push(format!("naming {name}: {said:?}"));
        }
        head_unmoved(store.as_ref(), head, "a first period read on the clock").await?;
        l.set("view_kept.book", json!(false)).await?;
    }
    let withdraw = json!({"declare": "withdraw", "kind": "object_set", "name": "hi"});
    l.declare_json(withdraw).await?;
    l.upsert(ty, "rate", add("2026-07-01", 2)).await?;
    let book = json!({"key": "view_kept.book", "value": true});
    let kept = try_upsert(l.cat.as_ref(), &l.own, ty, "view_kept.book", book).await;
    let declared = l.cat.declare(&l.own, hi).await;
    for (what, said, name) in [
        ("view_kept", format!("{kept:?}"), "book"),
        ("hi", format!("{declared:?}"), "hi"),
    ] {
        if !said.contains("SettingDatedKept") || !said.contains(&format!("\"{name}\"")) {
            wrong.push(format!("{what}: {said}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a key read on the clock by a kept view or a set decided at the write is refused \
         `setting_dated_kept`, naming each; dated, it refuses `view_kept` and the set's \
         declaration: {wrong:?}"
    );
    // On a date, kept, moving at each period added.
    let on = "setting('rate', '2026-08-01')";
    l.declare_json(view("dated", on)).await?;
    l.set("view_kept.dated", json!(true)).await?;
    let mut totals = Vec::new();
    for (from, value) in [(None, 0), (Some("2026-07-15"), 4), (Some("2026-08-01"), 3)] {
        if let Some(from) = from {
            l.upsert(ty, "rate", add(from, value)).await?;
        }
        let r = l.read("dated", "k").await?;
        totals.push(r.fields.get("total").and_then(Value::as_f64));
    }
    let dated = "dated".into();
    let told = l
        .cat
        .describe_type(&l.own, &dated, Default::default())
        .await;
    let told = told.must("describing `dated`")?.data.lifecycle.kept;
    ensure!(
        totals == [Some(6.0), Some(12.0), Some(9.0)]
            && matches!(told, Some(lakelands_api::describe::Kept::Kept)),
        "a view reading `{on}` is kept ({told:?}) and moves at each period added: {totals:?}"
    );
    Ok(())
}
