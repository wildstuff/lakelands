//! A retention hold withholds a record without erasing it
//! (`docs/log-format.md` §2.3, the store facade's `restrict`).
//!
//! A restricted record is absent for a principal without the
//! `restricted` grant and present, unchanged, for one with it, its history
//! included; lifting the hold brings it back, at every position.

use lakelands_api::error::Missing;
use lakelands_api::key::UNGROUPED;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::read::{AggFunc, Found};
use lakelands_api::write::{Outcome, ProposedWrite, WriteOpts};
use lakelands_api::{
    Access, Catalog, Error, Find, Position, Principal, ProposalId, Refusal, Timestamp, TypeName,
};
use serde_json::{Value, json};

use super::only::{claim_with, claims, de, finds, get, hit_keys, holder, node_keys};
use super::rekey;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, alike_renamed, field, get_at, head_unmoved, ingest, migration,
    patch, pause, principal, restrict, source_record, type_decl, view_of,
};
use crate::conformance::{Failure, Held, Store};
use lakelands_api::describe::{Kept, NotKept};

check!(
    HOLD_WITHHOLDS_AND_KEEPS = a_restricted_record_is_withheld_but_kept,
    "the facade: restrict hides a record from principals without the restricted grant, keeps it and its history for those with it, and unrestrict lifts it, its history readable again"
);

check!(
    HOLD_ACROSS_A_REKEY = a_record_held_across_a_rekey_is_not_there_as_of_before,
    "006, extended (lakelands#790): a record is found at the head by the name and the key it bears there, so a record held before a re-key and still held, and one held under its new key since, are not there as of before it to a reader without `restricted`, each compared whole with a key no record holds, nor counted in a `total`; one migration's re-keys move its records at once, never one through another, and a key taken by another record since names that one, never the record it held then; a holder of `only` searching as of before a re-key is judged at the head by each candidate's key there"
);

check!(
    HOLD_AT_EVERY_POSITION = a_record_held_now_is_not_there_at_any_position,
    "006, extended (lakelands#790): the hold as it stands now governs every position: to a reader without the `restricted` grant a record held at the head is not there as of an earlier position at `get` (compared whole with a key no record holds), `find` (rows and `total`), aggregates, a computed and a kept view, a walk, a pattern from it (compared whole with one from an absent record) and a link with an end held now (`get` compared whole, and the listing), a search, lexical and by vector, whose answers, scores and pages are those of a type that never held it; a link held under the name its relation was renamed to since is not there either; `describe` says a view so read is computed; a record held then and released since is answered there as any record is"
);

check!(
    HOLD_NOT_THERE_TO_A_WRITE = a_restricted_record_is_not_there_to_a_writer_without_the_grant,
    "006, extended (lakelands#772): to a writer without the `restricted` grant a restricted record is not there at any write door, its refusal compared whole with that of a key no record holds: a reference to it and either end of a `link` are `not_live`, `retire`, `unlink` of a link to it and a verb called on it, of low risk or high, are not found, and so is the proposal of each; a link to it ingested again is `not_live`; an upsert onto its key, made or proposed, and onto a held key retired since, made, proposed or ingested, an ingest onto a live held key and a link onto a link whose own record is held are not granted and name no field, as is a well-formed setting onto a held one's key; that refusal comes last of the record checks (016), so under a version, without a required field, with a key field that disagrees and with a `view_kept` setting that is no boolean the upsert onto a held key answers as onto a free one, a conflict, a constraint and twice invalid; nothing is written; a reviewer without the grant is refused the accept of a write onto it, as not granted; the accept of a proposal is judged as its write as the records stand then, so one naming a record restricted since is refused, and one that reached a record restricted when proposed, as only a holder of the grant could, is applied"
);

async fn a_restricted_record_is_withheld_but_kept(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("file", vec![field("body", StorageKind::Text)]))
        .await?;
    let written = l.upsert("file", "f1", json!({"body": "kept"})).await?;
    let reader = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "reader",
        Grants {
            default: lakelands_api::Access::Read,
            ..Grants::default()
        },
    );
    let ty = TypeName::from("file");

    let held = store
        .restrict(&ty, &"f1".into(), "statutory retention".into())
        .await
        .must("restricting `file` `f1`")?;
    let hidden = cat.get(&reader, &ty, get_at("f1", None)).await;
    ensure!(
        matches!(hidden, Err(Error::NotFound { .. })),
        "a principal without the restricted grant must not find `f1` after the hold; it answered {hidden:?}"
    );
    let seen = cat.get(own, &ty, get_at("f1", None));
    let seen = seen.await.must("reading `f1` with the restricted grant")?;
    ensure!(
        seen.data.fields.get("body") == Some(&json!("kept")) && seen.position == held,
        "the owner must read `f1` unchanged at the hold's position {held}; it read {:?} at {}",
        seen.data.fields,
        seen.position
    );
    let before = Some(written.position());
    let kept = cat.get(own, &ty, get_at("f1", before)).await;
    let kept = kept.must("reading `f1` before the hold, with the restricted grant")?;
    ensure!(
        kept.data.fields.get("body") == Some(&json!("kept")),
        "the owner must read `f1`'s history while it is held; it read {:?}",
        kept.data.fields
    );
    let then = cat.get(&reader, &ty, get_at("f1", before)).await;
    ensure!(
        matches!(then, Err(Error::NotFound { .. })),
        "a record held now is not there at an earlier position either; it answered {then:?}"
    );
    store
        .unrestrict(&ty, &"f1".into())
        .await
        .must("lifting the hold on `f1`")?;
    let fut = cat.get(&reader, &ty, get_at("f1", None));
    fut.await.must("reading `f1` after the hold is lifted")?;
    cat.get(&reader, &ty, get_at("f1", before))
        .await
        .must("reading `f1` before the hold, once it is lifted")?;
    Ok(())
}

const HELD_NOW: &str = r#"
    claim | rows and total | {"total": true, "as_of": BEFORE} | c2 c3 total=2
    claim | a page and total | {"total": true, "limit": 1, "as_of": BEFORE} | c2 total=2
    claim | a filtered total | {"where": "amount > 1", "total": true, "as_of": BEFORE} | c2 c3 total=2
    claim | aggregates | {"aggregate": {"measures": [{"name": "n", "func": "count"}, {"name": "s", "func": "sum", "field": "amount"}]}, "as_of": BEFORE} | n=2 s=25
    claim | a group | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "amount"}], "group_by": ["owner"]}, "as_of": BEFORE} | owner=alice s=5; owner=bob s=20
    spend | a kept view | {"total": true, "as_of": BEFORE} | alice bob total=2
    spent | a computed view | {"total": true, "as_of": BEFORE} | alice bob total=2
    sys.link.claim.on_trip | the links, an end held now | {"total": true, "as_of": BEFORE} | c2:t1 c3:t1 total=2
    trips | a kept view no hold reached then | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "n"}]}, "as_of": BEFORE} | s=1
"#;

async fn a_record_held_now_is_not_there_at_any_position(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    claims(&l, true).await?;
    let (by, sum) = (
        vec![field("owner", Text), field("total", Number)],
        [("total", "sum(amount)")],
    );
    l.declare(view_of("spent", "claim", &["owner"], by, &sum))
        .await?;
    // A kept view whose face without the grant was whole then: `t2` is held
    // only since.
    let n = vec![field("n", Number)];
    l.declare(view_of("trips", "trip", &[], n, &[("n", "count()")]))
        .await?;
    l.set("view_kept.trips", json!(true)).await?;
    l.upsert("trip", "t2", json!({"name": "oslo"})).await?;
    // `memo` as `twin` is, and `m1` and `m4`, which outrank `m2`, besides.
    for ty in ["memo", "twin"] {
        let note = json!({"name": "note", "type": {"kind": "text"}, "searchable": true});
        let v = json!({"name": "v", "type": {"kind": "vector", "dimensions": 2}});
        l.declare(de(json!({"name": ty, "fields": [note, v]})))
            .await?;
        for (key, note, v) in [
            ("m1", "taxi taxi taxi", [1.0, 0.0]),
            ("m2", "taxi ride", [0.9, 0.3]),
            ("m4", "taxi taxi", [0.95, 0.05]),
            ("m5", "bus", [0.0, 1.0]),
        ] {
            if ty == "memo" || !["m1", "m4"].contains(&key) {
                l.upsert(ty, key, json!({"note": note, "v": v})).await?;
            }
        }
    }
    // `c2` and `m2` are held then and released since; the rest held now.
    let released = [("claim", "c2"), ("memo", "m2"), ("twin", "m2")];
    for (ty, key) in released {
        restrict(s, ty, key, "then").await?;
    }
    let then = s.head().await.must("reading the head")?;
    let now = [
        ("claim", "c1"),
        ("memo", "m1"),
        ("memo", "m4"),
        ("trip", "t2"),
    ];
    for (ty, key) in now {
        restrict(s, ty, key, "now").await?;
    }
    for (ty, key) in released {
        s.unrestrict(&ty.into(), &key.into())
            .await
            .must("releasing")?;
    }
    let w = holder(s, "w", json!({"default": "read"}));
    let at = Some(then);
    let got = |key: &'static str| get(&l, &w, "claim", key, at);
    let (held, absent) = (got("c1").await, got("c9").await);
    let link = |key| get(&l, &w, "sys.link.claim.on_trip", key, at);
    let (to_held, to_none) = (link("c1:t1").await, link("c9:t1").await);
    let said = |a: &lakelands_api::Result<_>| json!(a.as_ref().err()).to_string();
    ensure!(
        held.is_err()
            && said(&held) == said(&absent).replace("c9", "c1")
            && to_held.is_err()
            && said(&to_held) == said(&to_none).replace("c9", "c1"),
        "`c1`, held now, and its link must be as of before what a key no record holds is: \
         {held:?}, not {absent:?}; {to_held:?}, not {to_none:?}"
    );
    finds(&l, &w, HELD_NOW, then).await?;
    let walk = |(ty, key): (&str, &str), steps: Value, project: &str| {
        let start = json!({"type": ty, "key": key});
        de(json!({"start": start, "steps": steps, "project": {"as": project}, "as_of": then.0}))
    };
    let into = json!([{"relation": "on_trip", "walk": "in"}]);
    let from_t1 = || walk(("trip", "t1"), into.clone(), "graph");
    let walked = l.cat.query(&w, from_t1()).await;
    let walked = node_keys(&walked.must("a walk from `t1`")?.data, "claim");
    let out = json!([{"relation": "on_trip", "walk": "out"}]);
    let from = |key| l.cat.query(&w, walk(("claim", key), out.clone(), "rows"));
    let (from_held, from_absent) = (from("c1").await, from("c9").await);
    ensure!(
        walked == "c2 c3"
            && format!("{from_held:?}") == format!("{from_absent:?}").replace("c9", "c1"),
        "the walk as of before reached `{walked}`, not `c2 c3`; a pattern from `c1`, held \
         now, answered {from_held:?}, not {from_absent:?}"
    );
    // `memo` as of before answers whole what `twin`, which never held `m1`
    // or `m4`, answers: each search, a pattern started by one, and each
    // page of one with its total.
    let reads = |ty: &str| {
        let search = |mode: Value| {
            let mut search = mode;
            (search["type"], search["as_of"], search["k"]) = (json!(ty), json!(then.0), json!(10));
            search
        };
        let lexical = json!({"mode": "lexical", "text": "taxi"});
        let vector = json!({"mode": "vector", "field": "v", "vector": [1.0, 0.0]});
        let hybrid = json!({"mode": "hybrid", "text": "taxi", "field": "v", "vector": [1.0, 0.0]});
        let started = json!({"start": {"type": ty, "search": vector}, "as_of": then.0});
        [search(lexical), search(vector), search(hybrid), started]
    };
    let mut wrong = Vec::new();
    for (memo, twin) in reads("memo").into_iter().zip(reads("twin")) {
        let answer = async |asked: Value| {
            let said = match asked.get("start") {
                Some(_) => json!(l.cat.query(&w, de(asked)).await.map(|r| r.data)),
                None => json!(l.cat.search(&w, de(asked)).await.map(|r| r.data)),
            };
            said.to_string().replace("twin", "memo")
        };
        let (held, never) = (answer(memo.clone()).await, answer(twin).await);
        if held != never {
            wrong.push(format!("{memo}: {held}, not {never}"));
        }
    }
    let pages = async |ty: &str| {
        let (mut pages, mut cursor) = (Vec::new(), None);
        loop {
            let find = json!({"total": true, "limit": 1, "as_of": then.0, "cursor": cursor});
            let found = l.cat.find(&w, &ty.into(), de(find)).await.must("a page")?;
            let mut page = json!(found.data);
            cursor = page.as_object_mut().and_then(|p| p.remove("next_cursor"));
            pages.push(page.to_string().replace("twin", "memo"));
            if cursor.as_ref().is_none_or(Value::is_null) || pages.len() > 9 {
                return Ok::<_, Failure>(pages);
            }
        }
    };
    let (held, never) = (pages("memo").await?, pages("twin").await?);
    ensure!(
        wrong.is_empty() && held == never,
        "`memo` as of before must be answered as `twin`, which never held `m1` or `m4`: \
         {wrong:#?}; its pages of one {held:#?}, not {never:#?}"
    );
    // `describe` says what the read does: a view it computes is not kept.
    let trips = l
        .cat
        .describe_type(&w, &"trips".into(), de(json!({"as_of": then.0})))
        .await;
    let kept = trips.must("`describe_type` of `trips` as of before")?.data;
    let computed = Some(Kept::Computed {
        why: NotKept::NotKeptHere,
    });
    ensure!(
        kept.lifecycle.kept == computed,
        "`trips` as of before is computed for `w`; `describe` said {:?}",
        kept.lifecycle.kept
    );
    // A relation renamed since: the link `c3:t1` held under its new name.
    let mut to = claim_with("owner");
    to.relations[0].name = "trip_of".into();
    let mut m = migration(to);
    m.relation_renames = vec![("on_trip".into(), "trip_of".into())];
    let report = l.cat.migrate(&l.own, m.clone()).await.must("previewing")?;
    let renamed = l.cat.migrate_apply(&l.own, m, &report.data.report_id).await;
    renamed.must("renaming `on_trip`")?;
    restrict(s, "sys.link.claim.trip_of", "c3:t1", "now").await?;
    let walked = l.cat.query(&w, from_t1()).await;
    let walked = node_keys(&walked.must("a walk from `t1` again")?.data, "claim");
    let find = de(json!({"where": "from = 'c3'", "total": true, "as_of": then.0}));
    let link = l.cat.find(&w, &"sys.link.claim.on_trip".into(), find).await;
    let link = link.must("the links from `c3` as of before")?.data;
    ensure!(
        walked == "c2"
            && matches!(&link, Found::Records { rows, total: Some(0), .. } if rows.is_empty()),
        "a link held under the name its relation bears since is not there as of before: \
         the walk reached `{walked}`, the links from `c3` are {link:?}"
    );
    Ok(())
}

async fn a_record_held_across_a_rekey_is_not_there_as_of_before(store: Store) -> Held {
    let s = store.as_ref();
    // `4` is held before the re-key and `DE2`, once `2`, only after it.
    let l = rekey::lake(s, rekey::IBANS).await?;
    let then = s.head().await.must("reading the head")?;
    rekey::apply(s).await?;
    restrict(s, "account", "DE2", "now").await?;
    let w = holder(s, "w", json!({"default": "read"}));
    let got = async |ty: &str, key: &str, at: Position| {
        let got = get(&l, &w, ty, key, Some(at)).await;
        json!(got.err()).to_string().replace(key, "9")
    };
    let total = async |who| {
        let find = de(json!({"total": true, "as_of": then.0}));
        match l
            .cat
            .find(who, &"account".into(), find)
            .await
            .map(|f| f.data)
        {
            Ok(Found::Records { total, .. }) => total.unwrap_or_default(),
            _ => 0,
        }
    };
    let (seen, all) = (total(&w).await, total(&l.own).await);
    // One migration moves its records at once: `seat` 1→2, 2→3, 3→4, 4→5,
    // `4` held before and `3`, once `2`, since. `pew`'s `1` moves to `0`,
    // and another record takes the key `1`, which a second migration
    // moves to the held `Q`. `card` swaps its keys, and its `2` then is
    // bob's since.
    let typed = |name: &str, key: &str| {
        let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "required": true});
        let note = json!({"name": "note", "type": {"kind": "text"}, "searchable": true});
        let fields = [text("n"), text("m"), text("o"), text("owner"), note];
        de(json!({"name": name, "key": [key], "fields": fields}))
    };
    let put = async |ty: &str, key: &str, [n, m, o, owner, note]: [&str; 5]| {
        let fields = json!({"n": n, "m": m, "o": o, "owner": owner, "note": note});
        l.upsert(ty, key, fields).await
    };
    for ty in ["seat", "pew", "card"] {
        l.declare(typed(ty, "n")).await?;
    }
    for (n, m) in [("1", "2"), ("2", "3"), ("3", "4"), ("4", "5")] {
        put("seat", n, [n, m, "o", "a", "taxi"]).await?;
    }
    put("pew", "1", ["1", "0", "P", "a", "taxi"]).await?;
    put("card", "1", ["1", "2", "o", "alice", "bus"]).await?;
    put("card", "2", ["2", "1", "o", "alice", "taxi"]).await?;
    restrict(s, "seat", "4", "then").await?;
    let before = s.head().await.must("reading the head")?;
    for ty in ["seat", "pew", "card"] {
        l.migrate(typed(ty, "m")).await?;
    }
    restrict(s, "seat", "3", "now").await?;
    put("pew", "1", ["9", "1", "Q", "a", "taxi"]).await?;
    l.migrate(typed("pew", "o")).await?;
    restrict(s, "pew", "Q", "now").await?;
    put("card", "1", ["2", "1", "o", "bob", "taxi"]).await?;
    put("card", "2", ["1", "2", "o", "alice", "taxi"]).await?;
    // Held then and now, held since under a new key, or free: as a key no
    // record holds, or found.
    let mut wrong = Vec::new();
    for (ty, key, at, found) in [
        ("account", "4", then, false),
        ("account", "2", then, false),
        ("seat", "4", before, false),
        ("seat", "2", before, false),
        ("seat", "1", before, true),
        ("seat", "3", before, true),
        ("pew", "1", before, true),
    ] {
        let want = if found {
            "null".into()
        } else {
            got(ty, "9", at).await
        };
        let said = got(ty, key, at).await;
        if said != want {
            wrong.push(format!("{ty} {key}: {said}, not {want}"));
        }
    }
    let only = json!({"default": "read", "types": {"card": {"only": {"owner": "alice"}}}});
    let search = de(json!({"type": "card", "mode": "lexical", "text": "taxi", "as_of": before.0}));
    let hits = l.cat.search(&holder(s, "alice", only), search).await;
    let hits = hit_keys(&hits.must("alice's search as of before")?.data);
    ensure!(
        wrong.is_empty() && seen + 2 == all && hits == "1",
        "as of before a re-key, a record held then and now or since under its new key is not \
         there, one free is: {wrong:#?}; `w` counts {seen} of the owner's {all}; alice's \
         search, judged by each candidate's key at the head, hits `{hits}`, not `1`"
    );
    Ok(())
}

check!(
    HOLD_REACHES_LINKS_AND_REVIEWS =
        a_link_with_a_held_end_is_not_there_and_an_accept_is_judged_by_what_it_reaches,
    "006 and 051, extended (lakelands#781): to a reader without the `restricted` grant a link with a held end is not there: `get` of it answers as of a link no record holds, compared whole, `find` on the link type does not list it, count it in a total or an aggregate, or find it by its hidden end, a kept view over the link type, and over a view over it, answers as its computed twin, a search as of before the hold judged at the head leaves it out, and a reference to it is `not_live` as one to a link no record holds; a cardinality refusal over such a link still refuses and names no end; to a reviewer without the grant a proposed write that references a held record or a link with a held end is not there, its accept and its reject answered as an id no proposal holds (lakelands#929), a decision judged as the write it decides either way, an upsert's or a call's in whatever form its assignment computes it (a payload member, a literal, `coalesce`, another field), which the owner applies; a high-risk call through the `call` door is judged as its caller's write, refused as not live to one without the grant and logging nothing, and proposed to a holder; and what a write reaches is computed as at its proposal, by the declarations, its record, the settings in force and its time, so one that reaches a held record only through a field retyped, a record, a setting or a time changed since is judged at the accept as its proposer's write; a holder's proposed call on a link with a held end is listed to a reviewer without the grant without its write, is not that reviewer's to accept or to reject, and is accepted by one who holds it; and the host places and lifts a hold on a retired record"
);

/// Each write door that names the record `k`, as `who`: a reference to a
/// `partner`, a link to and from it, its retirement, a verb on it, the
/// unlink of a link to it, the proposals of three of them, and a link to
/// it ingested again.
async fn write_doors(c: &dyn Catalog, who: &Principal, k: &str) -> Vec<lakelands_api::Result<()>> {
    fn said<T>(r: lakelands_api::Result<T>) -> lakelands_api::Result<()> {
        r.map(drop)
    }
    let (partner, deal, with, opts) = (
        &"partner".into(),
        &"deal".into(),
        &"with".into(),
        WriteOpts::default,
    );
    let (key, d1) = (&k.into(), &"d1".into());
    let refers = patch(json!({ "partner": k }));
    let call = |key| ProposedWrite::Call {
        verb: "rename".into(),
        key,
        payload: Default::default(),
        opts: opts(),
    };
    let retire = ProposedWrite::Retire {
        ty: "partner".into(),
        key: k.into(),
        opts: opts(),
    };
    let refer = ProposedWrite::Upsert {
        ty: "deal".into(),
        key: "n1".into(),
        patch: refers.clone(),
        opts: opts(),
    };
    vec![
        said(c.upsert(who, deal, &"n1".into(), refers, opts()).await),
        said(
            c.link(who, deal, d1, with, key, Default::default(), opts())
                .await,
        ),
        said(
            c.link(
                who,
                deal,
                key,
                with,
                &"open".into(),
                Default::default(),
                opts(),
            )
            .await,
        ),
        said(c.retire(who, partner, key, opts()).await),
        said(
            c.call(who, &"rename".into(), Some(key), Default::default(), opts())
                .await,
        ),
        said(
            c.call(who, &"touch".into(), Some(key), Default::default(), opts())
                .await,
        ),
        said(c.unlink(who, deal, d1, with, key, opts()).await),
        said(c.propose(who, retire, "why".into()).await),
        said(c.propose(who, call(Some(k.into())), "why".into()).await),
        said(c.propose(who, refer, "why".into()).await),
        said(
            ingest(
                c,
                who,
                "feed",
                vec![source_record(
                    "sys.link.deal.fed",
                    &format!("d1:{k}"),
                    json!({"from": "d1", "to": k}),
                )],
            )
            .await,
        ),
    ]
}

async fn a_restricted_record_is_not_there_to_a_writer_without_the_grant(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let to_partner = json!({"kind": "reference", "target": "partner"});
    for d in [
        json!({"declare": "type", "name": "partner", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "deal", "fields": [{"name": "partner", "type": to_partner},
            {"name": "src", "type": {"kind": "text"}, "authority": "source"}],
            "relations": [{"name": "with", "target": "partner", "cardinality": "many_to_many"},
            {"name": "fed", "target": "partner", "cardinality": "many_to_many", "authority": "source"}]}),
        json!({"declare": "verb", "name": "rename", "on": "partner", "risk": "high",
            "effect": "write", "set": [{"field": "n", "value": "'q'"}]}),
        json!({"declare": "verb", "name": "touch", "on": "partner", "risk": "low",
            "effect": "write", "set": [{"field": "n", "value": "'t'"}]}),
    ] {
        l.declare_json(d).await?;
    }
    for k in ["held", "open", "late"] {
        l.upsert("partner", k, json!({ "n": k })).await?;
    }
    for k in ["held", "d1", "rr1", "rr2", "rr3"] {
        l.upsert("deal", k, json!({"partner": "open"})).await?;
    }
    let (deal, d1, with, held) = (&"deal".into(), &"d1".into(), &"with".into(), &"held".into());
    let linked = l.cat.link(
        &l.own,
        deal,
        d1,
        with,
        held,
        Default::default(),
        WriteOpts::default(),
    );
    linked.await.must("linking `d1` to `held`")?;
    let ends = json!({"from": "d1", "to": "held"});
    let fed = l.ingest(
        "feed",
        vec![source_record("sys.link.deal.fed", "d1:held", ends)],
    );
    fed.await.must("ingesting `d1` to `held`")?;
    let (open, none) = (&"open".into(), Default::default());
    let linked = l
        .cat
        .link(&l.own, deal, d1, with, open, none, WriteOpts::default());
    linked.await.must("linking `d1` to `open`")?;
    restrict(s, "sys.link.deal.with", "d1:open", "a hold").await?;
    restrict(s, "partner", "held", "a hold").await?;
    restrict(s, "deal", "held", "a hold").await?;
    // Each `rr` is held, then retired: one for each door, as a write that
    // went through would make it live.
    for k in ["rr1", "rr2", "rr3"] {
        restrict(s, "deal", k, "a hold").await?;
        let retired = l
            .cat
            .retire(&l.own, deal, &k.into(), WriteOpts::default())
            .await;
        retired.must("retiring a held `rr`")?;
    }
    // A slip under a hold, keyed by its `code` and needing a `sum`, and
    // a `view_kept` setting under one: what the record checks before the
    // last answer there.
    l.declare_json(
        json!({"declare": "type", "name": "slip", "key": ["code"], "fields": [
        {"name": "code", "type": {"kind": "text"}}, {"name": "note", "type": {"kind": "text"}},
        {"name": "sum", "type": {"kind": "number"}, "required": true}]}),
    )
    .await?;
    for view in ["tally", "tally2"] {
        let fields = vec![field("n", Text), field("c", Number)];
        let counted = view_of(view, "partner", &["n"], fields, &[("c", "count()")]);
        l.declare(counted).await?;
    }
    l.upsert("slip", "s2", json!({"sum": 1})).await?;
    l.set("view_kept.tally", json!(false)).await?;
    let setting = lakelands_api::seam::SETTING_TYPE;
    restrict(s, "slip", "s2", "a hold").await?;
    restrict(s, setting, "view_kept.tally", "a hold").await?;
    let writer = |id: &str, restricted| {
        let grants = Grants {
            default: Access::Write,
            restricted,
            ingest: true,
            ..Grants::default()
        };
        principal(s, PrincipalKind::Agent, id, grants)
    };
    let (w, holder) = (writer("w", false), writer("holder", true));
    let (c, head) = (l.cat.as_ref(), s.head().await.must("the head")?);
    let mut apart = Vec::new();
    let there = write_doors(c, &w, "held").await;
    let gone = write_doors(c, &w, "gone").await;
    for (n, (a, b)) in there.iter().zip(&gone).enumerate() {
        let alike = match (a, b) {
            (Err(a), Err(b)) => alike_renamed(&format!("door {n}"), a, b, ("gone", "held")),
            _ => Err(Failure::new(format!(
                "door {n}: {a:?}; on a key no record holds: {b:?}"
            ))),
        };
        apart.extend(alike.err().map(|f| f.message));
    }
    let (partner, n2, z) = (&"partner".into(), &"n2".into(), || patch(json!({"n": "z"})));
    let (rr1, opts) = (&"rr1".into(), WriteOpts::default);
    let onto = |ty: &str, key: &str, fields| ProposedWrite::Upsert {
        ty: ty.into(),
        key: key.into(),
        patch: patch(fields),
        opts: opts(),
    };
    let refused = |answer: &lakelands_api::Result<()>| {
        matches!(
            answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        )
    };
    // The record checks before the last answer a probe as on a free key.
    let v7 = WriteOpts {
        expected_version: Some(lakelands_api::Version(7)),
        ..opts()
    };
    let mut kinds = Vec::new();
    for (ty, there, free, fields, opts) in [
        ("slip", "s2", "s9", json!({"sum": 2}), v7),
        ("slip", "s2", "s9", json!({"note": "x"}), opts()),
        ("slip", "s2", "s9", json!({"sum": 2, "code": "x"}), opts()),
        (
            setting,
            "view_kept.tally",
            "view_kept.tally2",
            json!({"value": "yes"}),
            opts(),
        ),
    ] {
        let (t, held, gone) = (ty.into(), there.into(), free.into());
        let at = c.upsert(&w, &t, &held, patch(fields.clone()), opts.clone());
        let at = at.await;
        let on_free = c.upsert(&w, &t, &gone, patch(fields.clone()), opts).await;
        let what = format!("{fields} onto the held `{there}`");
        match (&at, &on_free) {
            (Err(a), Err(b)) => {
                let alike = alike_renamed(&what, a, b, (free, there));
                apart.extend(alike.err().map(|f| f.message));
                let e = serde_json::to_value(a).unwrap_or_default();
                let kind = e["reason"]["refusal"].as_str().or(e["error"].as_str());
                kinds.push(kind.unwrap_or("?").to_owned());
            }
            _ => apart.push(format!("{what}: {at:?}; onto a free key: {on_free:?}")),
        }
    }
    if kinds != ["conflict", "constraint", "invalid", "invalid"] {
        apart.push(format!("the probes of a held key answered {kinds:?}"));
    }
    let fed = source_record("deal", "rr3", json!({"src": "x"}));
    let live = source_record("deal", "held", json!({"src": "x"}));
    for (what, answer) in [
        (
            "`held`, made",
            c.upsert(&w, partner, held, z(), opts()).await.map(drop),
        ),
        (
            "`held`, proposed",
            c.propose(&w, onto("partner", "held", json!({"n": "z"})), "why".into())
                .await
                .map(drop),
        ),
        (
            "`rr1`, made",
            c.upsert(&w, deal, rr1, patch(json!({"partner": "open"})), opts())
                .await
                .map(drop),
        ),
        (
            "`rr2`, proposed",
            c.propose(
                &w,
                onto("deal", "rr2", json!({"partner": "open"})),
                "why".into(),
            )
            .await
            .map(drop),
        ),
        (
            "`rr3`, ingested",
            ingest(c, &w, "feed", vec![fed]).await.map(drop),
        ),
        (
            "the live `held`, ingested",
            ingest(c, &w, "feed", vec![live]).await.map(drop),
        ),
        (
            "the held link `d1:open`, linked",
            c.link(&w, deal, d1, with, open, Default::default(), opts())
                .await
                .map(drop),
        ),
        (
            "the held setting, made",
            c.upsert(
                &w,
                &setting.into(),
                &"view_kept.tally".into(),
                patch(json!({"value": false})),
                opts(),
            )
            .await
            .map(drop),
        ),
    ] {
        if !refused(&answer) {
            apart.push(format!("an upsert onto {what}: {answer:?}"));
        }
    }
    let moved = head_unmoved(s, head, "a write to a restricted record").await;
    apart.extend(moved.err().map(|f| f.message));
    // At the accept: `late` is restricted after `w` proposed a reference
    // to it, and `held` was when `holder` did.
    let mut ids = Vec::new();
    for (who, key, to) in [(&w, "n2", "late"), (&holder, "n3", "held")] {
        let refers = patch(json!({ "partner": to }));
        let write = ProposedWrite::Upsert {
            ty: "deal".into(),
            key: key.into(),
            patch: refers,
            opts: WriteOpts::default(),
        };
        match c
            .propose(who, write, "why".into())
            .await
            .must("a proposal")?
        {
            Outcome::Proposed { proposal, .. } => ids.push(proposal),
            other => return Err(Failure::new(format!("a proposal answered {other:?}"))),
        }
    }
    restrict(s, "partner", "late", "a hold").await?;
    let direct = c.upsert(
        &w,
        deal,
        n2,
        patch(json!({"partner": "late"})),
        WriteOpts::default(),
    );
    let (direct, late) = (direct.await, c.accept(&l.own, &ids[0]).await);
    let same = |a: &Error, b: &Error| alike_renamed("the accept", a, b, ("", ""));
    if !matches!((&late, &direct), (Err(a), Err(b)) if same(a, b).is_ok()) {
        apart.push(format!(
            "the accept naming `late`: {late:?}; the write: {direct:?}"
        ));
    }
    let accepted = c.accept(&l.own, &ids[1]).await;
    if !matches!(accepted, Ok(Outcome::Applied { .. })) {
        apart.push(format!("the holder's accepted proposal: {accepted:?}"));
    }
    // A reviewer without the grant decides no proposal on a held record.
    let said = c.propose(
        &holder,
        onto("partner", "held", json!({"n": "z"})),
        "why".into(),
    );
    let said = said.await;
    let Outcome::Proposed { proposal, .. } = said.must("the holder's proposal")? else {
        return Err(Failure::new("the holder's write onto `held` is proposed"));
    };
    let mut reviewer = writer("reviewer", false);
    reviewer.grants.review = true;
    let decided = c.accept(&reviewer, &proposal).await.map(drop);
    if !refused(&decided) {
        apart.push(format!(
            "a reviewer without the grant accepts onto `held`: {decided:?}"
        ));
    }
    ensure!(
        apart.is_empty(),
        "a restricted record is not there to a writer without the grant; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}

async fn a_link_with_a_held_end_is_not_there_and_an_accept_is_judged_by_what_it_reaches(
    store: Store,
) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let to = json!({"kind": "reference", "target": "partner"});
    let p = json!([{"name": "p", "type": {"kind": "text"}}]);
    let lt_ref = json!({"kind": "reference", "target": "sys.link.deal.with"});
    let note = json!([{"name": "note", "type": {"kind": "text"}, "searchable": true}]);
    for d in [
        json!({"declare": "type", "name": "partner", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "deal", "fields": [{"name": "partner", "type": to}, {"name": "backup", "type": to},
            {"name": "cutoff", "type": {"kind": "date_time"}}],
            "relations": [{"name": "with", "target": "partner", "cardinality": "many_to_many", "fields": note},
            {"name": "pick", "target": "partner", "cardinality": "many_to_one"}]}),
        json!({"declare": "verb", "name": "tag", "on": "sys.link.deal.with", "risk": "high",
            "effect": "write", "set": [{"field": "note", "value": "'t'"}]}),
        json!({"declare": "verb", "name": "repoint", "on": "deal", "risk": "high", "effect": "write",
            "payload": [{"name": "p", "type": {"kind": "text"}}], "set": [{"field": "partner", "value": "payload.p"}]}),
        json!({"declare": "verb", "name": "co", "on": "deal", "risk": "high", "effect": "write", "payload": p,
            "set": [{"field": "partner", "value": "coalesce(payload.p, 'p1')"}]}),
        json!({"declare": "verb", "name": "swap", "on": "deal", "risk": "high", "effect": "write",
            "set": [{"field": "partner", "value": "backup"}]}),
        json!({"declare": "type", "name": "tick", "fields": [{"name": "t", "type": {"kind": "text"}}, {"name": "u", "type": {"kind": "text"}}]}),
        json!({"declare": "verb", "name": "cp", "on": "tick", "risk": "high", "effect": "write",
            "set": [{"field": "u", "value": "t"}]}),
        json!({"declare": "verb", "name": "fromset", "on": "deal", "risk": "high", "effect": "write",
            "set": [{"field": "partner", "value": "setting('tgt')"}]}),
        json!({"declare": "verb", "name": "bynow", "on": "deal", "risk": "high", "effect": "write",
            "set": [{"field": "partner", "value": "if(cutoff < now(), 'held', 'p1')"}]}),
        json!({"declare": "type", "name": "memo", "fields": [{"name": "l", "type": lt_ref}, {"name": "lb", "type": lt_ref}]}),
        json!({"declare": "verb", "name": "swapl", "on": "memo", "risk": "high", "effect": "write",
            "set": [{"field": "l", "value": "lb"}]}),
        json!({"declare": "verb", "name": "setl", "on": "memo", "risk": "high", "effect": "write",
            "payload": [{"name": "l", "type": {"kind": "text"}}], "set": [{"field": "l", "value": "payload.l"}]}),
        json!({"declare": "verb", "name": "lit", "on": "memo", "risk": "high", "effect": "write",
            "set": [{"field": "l", "value": "'d1:held'"}]}),
        count_of("vk", "sys.link.deal.with"),
        count_of("vc", "sys.link.deal.with"),
        json!({"declare": "type", "name": "byto", "key": ["to"], "fields": [{"name": "to", "type": to},
            {"name": "n", "type": {"kind": "decimal"}}],
            "view": {"source": "sys.link.deal.with", "group_by": ["to"], "measures": [{"name": "n", "expr": "count()"}]}}),
        count_of("vv", "byto"),
        count_of("vv2", "byto"),
    ] {
        l.declare_json(d).await?;
    }
    for kept in ["vk", "byto", "vv"] {
        l.set(&format!("view_kept.{kept}"), json!(true)).await?;
    }
    for (ty, k) in [
        ("partner", "held"),
        ("partner", "p1"),
        ("partner", "r2"),
        ("deal", "d1"),
        ("deal", "d2"),
        ("deal", "d8"),
    ] {
        let fields = if ty == "deal" {
            json!({"partner": "p1", "backup": "held"})
        } else {
            json!({ "n": k })
        };
        l.upsert(ty, k, fields).await?;
    }
    let (deal, opts) = (&"deal".into(), WriteOpts::default);
    for (from, by, to) in [
        ("d1", "with", "held"),
        ("d1", "with", "p1"),
        ("d2", "pick", "held"),
    ] {
        let note = patch(if by == "with" {
            json!({"note": "x"})
        } else {
            json!({})
        });
        let (from, by, to) = (from.into(), by.into(), to.into());
        let linked = l.cat.link(&l.own, deal, &from, &by, &to, note, opts());
        linked.await.must("linking to `held`")?;
    }
    for m in ["m3", "m4"] {
        l.upsert("memo", m, json!({"l": "d1:p1", "lb": "d1:held"}))
            .await?;
    }
    l.upsert("deal", "e3", json!({"partner": "p1", "backup": "p1"}))
        .await?;
    l.set("tgt", json!("p1")).await?;
    let before = s.head().await.must("the head")?;
    restrict(s, "partner", "held", "a hold").await?;
    let who = |id: &str, restricted, review| {
        let grants = Grants {
            default: Access::Write,
            restricted,
            review,
            ..Grants::default()
        };
        principal(s, PrincipalKind::Agent, id, grants)
    };
    let (w, holder, reviewer) = (
        who("w", false, false),
        who("holder", true, false),
        who("reviewer", false, true),
    );
    let (c, lt, mut apart) = (l.cat.as_ref(), "sys.link.deal.with".into(), Vec::new());
    // The link `d1:held` is answered as `d1:gone`, and not listed.
    let (there, gone) = (
        c.get(&w, &lt, get_at("d1:held", None)).await,
        c.get(&w, &lt, get_at("d1:gone", None)).await,
    );
    match (there, gone) {
        (Err(a), Err(b)) => apart.extend(
            alike_renamed("`get`", &a, &b, ("gone", "held"))
                .err()
                .map(|f| f.message),
        ),
        (a, b) => apart.push(format!("`get` of `d1:held`: {a:?}; of `d1:gone`: {b:?}")),
    }
    let listed = c.find(&w, &lt, Default::default());
    let listed = listed.await.must("finding the links")?;
    if format!("{:?}", listed.data).contains("held") {
        apart.push(format!("`find` lists {:?}", listed.data));
    }
    // Nor is it in a total, a filter on its hidden end, an aggregate count,
    // or a kept view's count, which answers as its computed twin.
    let found = |f: Find| async {
        c.find(&w, &lt, f)
            .await
            .must("finding the links")
            .map(|r| r.data)
    };
    let total = found(serde_json::from_value(json!({"total": true})).expect("a find")).await?;
    let ended =
        found(serde_json::from_value(json!({"where": "to = 'held'"})).expect("a find")).await?;
    let counted = found(aggregate(vec![agg("n", AggFunc::Count, None)])).await?;
    let shown = |f: &Found| match f {
        Found::Records { rows, total, .. } => (rows.len(), *total),
        Found::Groups { rows, .. } => (
            0,
            rows.first()
                .and_then(|g| g.values.get("n")?.as_str()?.parse().ok()),
        ),
    };
    let (kept, computed) = (
        c.get(&w, &"vk".into(), get_at(UNGROUPED, None))
            .await
            .map(|r| r.data.fields),
        c.get(&w, &"vc".into(), get_at(UNGROUPED, None))
            .await
            .map(|r| r.data.fields),
    );
    let (over, twin) = (
        c.get(&w, &"vv".into(), get_at(UNGROUPED, None))
            .await
            .map(|r| r.data.fields),
        c.get(&w, &"vv2".into(), get_at(UNGROUPED, None))
            .await
            .map(|r| r.data.fields),
    );
    if over.as_ref().ok() != twin.as_ref().ok()
        || twin.as_ref().ok().and_then(|f| f.get("n")) != Some(&json!("1"))
    {
        apart.push(format!(
            "a kept view over a view over the links: {over:?}; computed: {twin:?}"
        ));
    }
    let n = kept.as_ref().ok().and_then(|f| f.get("n").cloned());
    if [&total, &ended, &counted].map(shown) != [(1, Some(1)), (0, None), (0, Some(1))]
        || n != Some(json!("1"))
        || kept.as_ref().ok() != computed.as_ref().ok()
    {
        apart.push(format!(
            "{total:?}; {ended:?}; {counted:?}; kept {kept:?}, computed {computed:?}"
        ));
    }
    // A search as of before the hold, by one whose `only` elsewhere has it
    // judged at the head too, judges the link's ends there.
    let mut searcher = who("searcher", false, false);
    let only = [("l".into(), json!("x"))].into();
    let held = TypeGrants {
        only,
        ..Default::default()
    };
    searcher.grants.types.insert("memo".into(), held);
    let search =
        json!({"type": "sys.link.deal.with", "mode": "lexical", "text": "x", "as_of": before.0});
    let hits = c.search(&searcher, serde_json::from_value(search).expect("a search"));
    let hits = hits.await;
    if !format!("{hits:?}").contains("d1:p1") || format!("{hits:?}").contains("held") {
        apart.push(format!("a search as of before the hold: {hits:?}"));
    }
    // A reference to the link answers as one to a link that is not there.
    let (memo, m1) = (&"memo".into(), &"m1".into());
    let memo = |l: &str| c.upsert(&w, memo, m1, patch(json!({ "l": l })), opts());
    match (memo("d1:held").await, memo("d1:gone").await) {
        (Err(a), Err(b)) => apart.extend(
            alike_renamed("a reference", &a, &b, ("gone", "held"))
                .err()
                .map(|f| f.message),
        ),
        (a, b) => apart.push(format!(
            "a reference to `d1:held`: {a:?}; to `d1:gone`: {b:?}"
        )),
    }
    // The cardinality still holds over it, naming no end.
    let picked = c
        .link(
            &w,
            deal,
            &"d2".into(),
            &"pick".into(),
            &"p1".into(),
            Default::default(),
            opts(),
        )
        .await;
    let named = format!("{picked:?}");
    if !named.contains("LinkedAlreadyFrom") || named.contains("held") {
        apart.push(format!("a second `pick` from `d2`: {picked:?}"));
    }
    // A reviewer without the grant decides no write that references `held`.
    let refers = ProposedWrite::Upsert {
        ty: "deal".into(),
        key: "n3".into(),
        patch: patch(json!({"partner": "held"})),
        opts: opts(),
    };
    let id = match c
        .propose(&holder, refers, "why".into())
        .await
        .must("the holder's proposal")?
    {
        Outcome::Proposed { proposal, .. } => proposal,
        other => {
            return Err(Failure::new(format!(
                "the holder's proposal answered {other:?}"
            )));
        }
    };
    // A proposal naming a record held from its reviewer is not there to it
    // (lakelands#929): its accept answers as an id no proposal holds.
    let gone = |a: &lakelands_api::Result<Outcome>, id: &ProposalId| matches!(a, Err(Error::NotFound { missing: Missing::Proposal { id: i } }) if i == id);
    let decided = c.accept(&reviewer, &id).await;
    if !gone(&decided, &id) {
        apart.push(format!(
            "a reviewer without the grant accepts a reference to `held`: {decided:?}"
        ));
    }
    // Each of the holder's proposals that references `held` or `d1:held`,
    // whatever form its write takes, is not there to the reviewer and is
    // the owner's to apply.
    let call = |verb: &str, key: &str, payload| ProposedWrite::Call {
        verb: verb.into(),
        key: Some(key.into()),
        payload: patch(payload),
        opts: opts(),
    };
    let memo = ProposedWrite::Upsert {
        ty: "memo".into(),
        key: "m2".into(),
        patch: patch(json!({"l": "d1:held"})),
        opts: opts(),
    };
    for (what, write) in [
        ("repoint", call("repoint", "d1", json!({"p": "held"}))),
        ("co", call("co", "d2", json!({"p": "held"}))),
        ("swap", call("swap", "d2", json!({}))),
        ("a memo", memo),
        ("setl", call("setl", "m3", json!({"l": "d1:held"}))),
        ("lit", call("lit", "m3", json!({}))),
    ] {
        let id = match c.propose(&holder, write, "why".into()).await.must(what)? {
            Outcome::Proposed { proposal, .. } => proposal,
            other => return Err(Failure::new(format!("{what} answered {other:?}"))),
        };
        let refused = c.accept(&reviewer, &id).await;
        let rejected = c.reject(&reviewer, &id, "no".into()).await;
        let applied = c.accept(&l.own, &id).await;
        if !gone(&refused, &id) || !gone(&rejected, &id) || applied.is_err() {
            apart.push(format!(
                "the holder's {what}: {refused:?}, rejected {rejected:?}; by the owner: {applied:?}"
            ));
        }
    }
    // Through the call door, a high-risk call is judged as the caller's
    // write: `w`'s is refused as not live and logs nothing, the holder's
    // is proposed and applied.
    let doors = [
        ("repoint", "d8", json!({"p": "held"})),
        ("swap", "d8", json!({})),
        ("setl", "m4", json!({"l": "d1:held"})),
        ("swapl", "m4", json!({})),
    ];
    for (verb, key, payload) in doors {
        let (v, k) = (verb.into(), key.into());
        let head = s.head().await.must("the head")?;
        let refused = c.call(&w, &v, Some(&k), patch(payload.clone()), opts());
        let refused = refused.await;
        let logged = s.head().await.must("the head")? != head;
        let said = c.call(&holder, &v, Some(&k), patch(payload), opts()).await;
        let applied = match said {
            Ok(Outcome::Proposed { proposal, .. }) => c.accept(&l.own, &proposal).await.map(drop),
            other => other.map(drop),
        };
        let not_live = matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::Constraint(_),
                ..
            })
        );
        if !not_live || logged || applied.is_err() {
            apart.push(format!(
                "`{verb}` on `{key}`: {refused:?} for `w`; the holder's: {applied:?}"
            ));
        }
    }
    // `w` proposed `swap e3` while its `backup` was `p1`: once `backup` is
    // `held`, the accept judges it as `w`'s write.
    let swap = c
        .call(
            &w,
            &"swap".into(),
            Some(&"e3".into()),
            Default::default(),
            opts(),
        )
        .await;
    let Outcome::Proposed { proposal, .. } = swap.must("`w`'s `swap e3`")? else {
        return Err(Failure::new("`w`'s `swap e3` is proposed"));
    };
    l.upsert("deal", "e3", json!({"backup": "held"})).await?;
    let late = c.accept(&l.own, &proposal).await;
    if !matches!(
        late,
        Err(Error::Refused {
            reason: Refusal::Constraint(_),
            ..
        })
    ) {
        apart.push(format!("`w`'s `swap e3`, `backup` held since: {late:?}"));
    }
    // What `w`'s call reaches is computed as at its proposal: a setting
    // turned to `held`, or a time passed, since is judged at the accept as
    // `w`'s write. `bynow` turns to `held` three seconds after it is written.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let cutoff = Timestamp(now.map_or(0, |d| d.as_micros() as i64) + 3_000_000);
    l.upsert(
        "deal",
        "e4",
        json!({"partner": "p1", "backup": "p1", "cutoff": cutoff.to_string()}),
    )
    .await?;
    let mut later = Vec::new();
    for verb in ["fromset", "bynow"] {
        let said = c
            .call(
                &w,
                &verb.into(),
                Some(&"e4".into()),
                Default::default(),
                opts(),
            )
            .await;
        match said.must(verb)? {
            Outcome::Proposed { proposal, .. } => later.push((verb, proposal)),
            other => return Err(Failure::new(format!("`w`'s `{verb}` answered {other:?}"))),
        }
    }
    l.set("tgt", json!("held")).await?;
    pause(std::time::Duration::from_millis(3200)).await;
    for (verb, proposal) in later {
        let late = c.accept(&l.own, &proposal).await;
        if !matches!(
            late,
            Err(Error::Refused {
                reason: Refusal::Constraint(_),
                ..
            })
        ) {
            apart.push(format!(
                "`w`'s `{verb}`, accepted once it reaches `held`: {late:?}"
            ));
        }
    }
    // `w`'s upsert and `cp` call were proposed while `t` and `u` were
    // text: retyped to references to `held` since, they are judged at the
    // accept as `w`'s writes.
    l.upsert("tick", "x1", json!({"t": "held", "u": "p1"}))
        .await?;
    let tick = |key: &str, fields| ProposedWrite::Upsert {
        ty: "tick".into(),
        key: key.into(),
        patch: patch(fields),
        opts: opts(),
    };
    let mut retyped = Vec::new();
    for write in [
        tick("x2", json!({"t": "held"})),
        call("cp", "x1", json!({})),
    ] {
        match c
            .propose(&w, write, "why".into())
            .await
            .must("`w`'s proposal on `tick`")?
        {
            Outcome::Proposed { proposal, .. } => retyped.push(proposal),
            other => {
                return Err(Failure::new(format!(
                    "`w`'s proposal on `tick` answered {other:?}"
                )));
            }
        }
    }
    let to: TypeDecl = serde_json::from_value(json!({"name": "tick", "fields": [
        {"name": "t", "type": to}, {"name": "u", "type": to}]}))
    .expect("a type");
    let m = migration(to);
    let report = l.cat.migrate(&l.own, m.clone());
    let report = report.await.must("retyping `tick`")?.data.report_id;
    let fut = l.cat.migrate_apply(&l.own, m, &report);
    fut.await.must("retyping `tick`")?;
    for proposal in retyped {
        let late = c.accept(&l.own, &proposal).await;
        if !matches!(
            late,
            Err(Error::Refused {
                reason: Refusal::Constraint(_),
                ..
            })
        ) {
            apart.push(format!(
                "`w`'s proposal on `tick`, accepted after the retype: {late:?}"
            ));
        }
    }
    // A holder's call on `d1:held`, proposed, is accepted.
    let called = c
        .call(
            &holder,
            &"tag".into(),
            Some(&"d1:held".into()),
            Default::default(),
            opts(),
        )
        .await;
    let Outcome::Proposed { proposal: id, .. } = called.must("the holder's call")? else {
        return Err(Failure::new("the holder's call is proposed"));
    };
    // The reviewer without the grant is listed it without its write, as
    // it is not answered the link, and decides it not.
    let listed = c.proposals(&reviewer, Default::default()).await;
    let listed = listed.must("the reviewer's listing")?.data;
    let bare = |p: &lakelands_api::write::Proposal| p.id == id && p.write.is_none() && p.more;
    let no = [
        c.accept(&reviewer, &id).await.map(drop),
        c.reject(&reviewer, &id, "no".into()).await.map(drop),
    ];
    let refused = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    if !listed.iter().any(bare) || no != [refused.clone(), refused] {
        apart.push(format!(
            "the holder's call on `d1:held`, to a reviewer without the grant: {no:?}, listed {listed:?}"
        ));
    }
    let accepted = c.accept(&l.own, &id).await.map(drop);
    if accepted.is_err() {
        apart.push(format!(
            "the holder's call on `d1:held`, accepted: {accepted:?}"
        ));
    }
    // The host holds a retired record, and lets it go.
    let retired = c
        .retire(&l.own, &"partner".into(), &"r2".into(), opts())
        .await;
    retired.must("retiring `r2`")?;
    let (r2, partner) = (&"r2".into(), &"partner".into());
    let placed = s.restrict(partner, r2, "a hold".into()).await.map(drop);
    let lifted = s.unrestrict(partner, r2).await.map(drop);
    if placed.is_err() || lifted.is_err() {
        apart.push(format!(
            "a hold on retired `r2`: {placed:?}, lifted: {lifted:?}"
        ));
    }
    ensure!(
        apart.is_empty(),
        "a link with a held end is not there, and an accept is judged by what it reaches; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}

/// A count over `source`.
fn count_of(name: &str, source: &str) -> serde_json::Value {
    let n = json!([{"name": "n", "type": {"kind": "decimal"}}]);
    let count = json!([{"name": "n", "expr": "count()"}]);
    json!({"declare": "type", "name": name, "fields": n,
        "view": {"source": source, "measures": count}})
}
