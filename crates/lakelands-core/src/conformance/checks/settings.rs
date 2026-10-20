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
use crate::conformance::{Failure, Given, Held, Reopen, Store};

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
