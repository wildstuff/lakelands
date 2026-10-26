//! A lexical score is BM25F over the reader's class: a token weighs by
//! how few of the records the reader sees hold it, a field's frequency by
//! its length against the field's mean, and the statistics follow every
//! write (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).

use super::only::{de, holder};
use super::search::doc_search;
use crate::conformance::fixture::{doors, owner, source_record};
use crate::conformance::prelude::*;

check!(
    LEXICAL_BITS = a_lexical_score_is_the_one_functions_bits_on_every_engine,
    "025: a lexical hit's score is the one BM25F function's, to the bit, over the reader's class, whatever the engine and the target; so with the fields named"
);

check!(
    LEXICAL_RARITY = a_rarer_token_outranks_a_common_one,
    "025: of two records holding one query token each, in fields of one length, the one whose token fewer records hold ranks first"
);

check!(
    LEXICAL_LENGTH = a_shorter_field_outranks_a_longer_one_at_equal_frequency,
    "025: of two records holding a token as often, the one whose field is shorter than the field's mean ranks first"
);

check!(
    LEXICAL_HELD = a_held_record_moves_no_score_of_a_reader_without_restricted,
    "025 and 006: a record held from a reader without `restricted` is not counted in its statistics, so holding one moves none of its scores to the bit, while the owner's move; lifted, the record counts to both"
);

check!(
    LEXICAL_ONLY = an_only_readers_score_is_moved_by_nothing_outside_its_pairs,
    "025 and 051: a holder of `only` is scored over the records it is admitted, so a record outside its pairs, written, changed or retired, moves none of its scores to the bit"
);

check!(
    LEXICAL_HIDDEN_AS_OF = a_search_as_of_a_position_counts_no_field_hidden_on_a_record_there,
    "025 and 006: a reader with a field hidden where a condition holds is scored, as of a position, over the records as it is told them there and at the head: the field on a record selected at the position moves none of its statistics, and at the head the record counts whole"
);

check!(
    LEXICAL_RECOUNT = the_text_statistics_equal_a_recount_after_every_write,
    "025: after a write, a change, a retirement, a hold and its release, a re-keying migration, a migration that renames a field and makes one unsearchable, an ingest, a forgetting and a rebuild, the owner's and a reader's without `restricted` scores are the one function's over the heads each sees, counted again",
    reopens
);

/// `memo`: an `owner`, a searchable `title` and `body`, in English.
fn memo() -> TypeDecl {
    let fields = vec![
        field("owner", StorageKind::Text),
        searchable("title"),
        searchable("body"),
    ];
    TypeDecl {
        language: Some("en".into()),
        ..type_decl("memo", fields)
    }
}

/// The hits of `mode` over `ty` as `who` is answered them, best first:
/// each key and its score.
async fn scored(
    store: &dyn LakeStore,
    who: &Principal,
    ty: &str,
    mode: SearchMode,
) -> Result<Vec<(String, f64)>, Failure> {
    let asked = Search {
        ty: ty.into(),
        k: 50,
        ..doc_search(mode)
    };
    let what = format!("`{}` searching `{ty}`", who.id);
    let hits = doors(store).search(who, asked).await.must(&what)?.data;
    Ok(hits
        .hits
        .iter()
        .map(|h| (h.record.key.0.clone(), h.score))
        .collect())
}

/// Writes each of `heads` into `memo` as the owner.
async fn written(l: &Lake, heads: &[(&str, Value)]) -> Held {
    for (key, fields) in heads {
        l.upsert("memo", key, fields.clone()).await?;
    }
    Ok(())
}

/// The keys of `scores`, in their order.
fn order(scores: &[(String, f64)]) -> Vec<&str> {
    scores.iter().map(|(k, _)| k.as_str()).collect()
}

async fn a_lexical_score_is_the_one_functions_bits_on_every_engine(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(memo()).await?;
    let heads = [
        (
            "m1",
            json!({"title": "Harbour report", "body": "the harbour master reports a storm at the harbour"}),
        ),
        ("m2", json!({"body": "storm warning"})),
        (
            "m3",
            json!({"title": "Quay", "body": "boats at the quay wait out the storm"}),
        ),
        ("m4", json!({"body": "nothing here"})),
    ];
    written(&l, &heads).await?;
    let own = owner(store.as_ref());
    // The bits on every target: libm's `ln` and one order of operations.
    let pinned = [
        ("m1", f64::from_bits(0x4005_8d19_7cc2_757f)),
        ("m2", f64::from_bits(0x3fed_b3ee_dd3d_46e9)),
        ("m3", f64::from_bits(0x3fe2_4437_2c55_9920)),
    ]
    .map(|(k, s)| (k.to_owned(), s));
    let mode = SearchMode::lexical("harbour storm");
    let got = scored(store.as_ref(), &own, "memo", mode).await?;
    ensure!(
        got == pinned,
        "searching \"harbour storm\" scored {got:?}; its bits are {pinned:?}"
    );
    for (text, fields) in [
        ("harbour storm", None),
        ("storm quay boats reported", None),
        ("harbour quay", Some(&["body"][..])),
    ] {
        let mode = SearchMode::Lexical {
            text: text.into(),
            fields: fields.map(|f| f.iter().map(|&f| f.into()).collect()),
            text_fields: None,
        };
        let got = scored(store.as_ref(), &own, "memo", mode).await?;
        let one = bm25f(&memo(), &heads, (text, fields), &[]);
        ensure!(
            got == one,
            "searching {text:?} in {fields:?} scored {got:?}; the one function scores {one:?}"
        );
    }
    Ok(())
}

async fn a_rarer_token_outranks_a_common_one(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(memo()).await?;
    let heads = [
        ("common", json!({"body": "apple"})),
        ("rare", json!({"body": "kiwi"})),
        ("x1", json!({"body": "apple pie"})),
        ("x2", json!({"body": "apple tart"})),
        ("x3", json!({"body": "apple cake"})),
    ];
    written(&l, &heads).await?;
    let got = scored(
        store.as_ref(),
        &owner(store.as_ref()),
        "memo",
        SearchMode::lexical("apple kiwi"),
    )
    .await?;
    ensure!(
        order(&got).first() == Some(&"rare") && order(&got).get(1) == Some(&"common"),
        "searching \"apple kiwi\" answered {got:?}; `rare` holds `kiwi`, which one record \
         holds, and `common` `apple`, which four hold, each in a one-word body, so `rare` \
         ranks first and `common` before the longer bodies"
    );
    Ok(())
}

async fn a_shorter_field_outranks_a_longer_one_at_equal_frequency(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(memo()).await?;
    let heads = [
        ("long", json!({"body": "a river flows past the old mill"})),
        ("short", json!({"body": "river"})),
        ("other", json!({"body": "stones on a quiet path"})),
    ];
    written(&l, &heads).await?;
    let own = owner(store.as_ref());
    let got = scored(store.as_ref(), &own, "memo", SearchMode::lexical("river")).await?;
    ensure!(
        order(&got) == ["short", "long"] && got[0].1 > got[1].1,
        "searching \"river\" answered {got:?}; `short` and `long` each hold it once, \
         `short` in a body of one word, `long` in one of seven, so `short` scores higher"
    );
    Ok(())
}

async fn a_held_record_moves_no_score_of_a_reader_without_restricted(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    l.declare(memo()).await?;
    let heads = [
        ("a", json!({"title": "River", "body": "the river runs"})),
        ("b", json!({"body": "running water"})),
        ("c", json!({"body": "a dry bed"})),
    ];
    written(&l, &heads).await?;
    let (own, reader) = (owner(s), field_writer(s, "reader", "memo", &[]));
    let text = || SearchMode::lexical("river run");
    let before = scored(s, &reader, "memo", text()).await?;
    let held = (
        "held",
        json!({"body": "river river run long winding river bank"}),
    );
    written(&l, std::slice::from_ref(&held)).await?;
    restrict(s, "memo", "held", "legal hold").await?;
    let after = scored(s, &reader, "memo", text()).await?;
    ensure!(
        after == before,
        "a reader without `restricted` scored {before:?}, and once `held` was written and \
         held, {after:?}; a record it does not see moves none of its scores"
    );
    let every = [heads.to_vec(), vec![held]].concat();
    let owned = scored(s, &own, "memo", text()).await?;
    let one = bm25f(&memo(), &every, ("river run", None), &[]);
    ensure!(
        owned == one,
        "the owner, holding `restricted`, scored {owned:?}; over every head, `held` among \
         them, the one function scores {one:?}"
    );
    let (ty, key) = (TypeName::from("memo"), RecordKey::from("held"));
    s.unrestrict(&ty, &key).await.must("lifting the hold")?;
    let lifted = scored(s, &reader, "memo", text()).await?;
    ensure!(
        lifted == one,
        "the hold lifted, the reader scored {lifted:?}; over every head the one function \
         scores {one:?}"
    );
    Ok(())
}

async fn an_only_readers_score_is_moved_by_nothing_outside_its_pairs(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    l.declare(memo()).await?;
    let own = |key: &str, body: &str| (key.to_owned(), json!({"owner": "alice", "body": body}));
    let mine = [
        own("a1", "the river runs"),
        own("a2", "a river"),
        own("a3", "nothing here"),
    ];
    for (key, fields) in &mine {
        l.upsert("memo", key, fields.clone()).await?;
    }
    let grants = json!({"default": "read", "types": {"memo": {"only": {"owner": "alice"}}}});
    let alice = holder(s, "alice", grants);
    let text = || SearchMode::lexical("river run");
    let before = scored(s, &alice, "memo", text()).await?;
    let heads: Vec<(&str, Value)> = mine.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let one = bm25f(&memo(), &heads, ("river run", None), &[]);
    ensure!(
        before == one,
        "alice scored {before:?}; over her own records the one function scores {one:?}"
    );
    let bob = |body: &str| json!({"owner": "bob", "body": body});
    l.upsert("memo", "b1", bob("river river river run")).await?;
    l.upsert("memo", "b2", bob("a long tale of a river"))
        .await?;
    let written = scored(s, &alice, "memo", text()).await?;
    l.upsert("memo", "b1", bob("run")).await?;
    l.retire("memo", "b2").await?;
    let changed = scored(s, &alice, "memo", text()).await?;
    ensure!(
        written == before && changed == before,
        "alice scored {before:?}; once bob's records were written, {written:?}; changed \
         and retired, {changed:?}; records outside her pairs move none of her scores"
    );
    Ok(())
}

/// `note`, keyed by `key`: `code` or `alt`; `body` its name for the
/// body; `title` searchable or not; each field written by either door.
fn note(key: &str, body: &str, title: bool) -> TypeDecl {
    let [mut code, mut alt] = ["code", "alt"].map(|n| field(n, StorageKind::Text));
    (code.required, alt.required) = (true, true);
    let title = FieldDecl {
        searchable: title,
        ..field("title", StorageKind::Text)
    };
    let mut fields = vec![code, alt, title, searchable(body)];
    for f in &mut fields {
        f.authority = Authority::Shared;
    }
    TypeDecl {
        key: vec![key.into()],
        language: Some("en".into()),
        ..type_decl("note", fields)
    }
}

/// The heads of `note` as the check wrote them: each key, its fields and
/// whether it is held.
type Heads = Vec<(String, Value, bool)>;

async fn the_text_statistics_equal_a_recount_after_every_write(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (_, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let s = store.as_ref();
    let l = Lake::of(s);
    let (own, reader) = (owner(s), field_writer(s, "reader", "note", &[]));
    let mut decl = note("code", "body", true);
    l.declare(decl.clone()).await?;
    let text = "river run";
    let recounted = |decl: &TypeDecl, heads: &Heads, after: &str| {
        let (decl, heads, after) = (decl.clone(), heads.clone(), after.to_owned());
        let (own, reader) = (&own, &reader);
        async move {
            let all: Vec<(&str, Value)> = heads
                .iter()
                .map(|(k, v, _)| (k.as_str(), v.clone()))
                .collect();
            let seen: Vec<(&str, Value)> = (heads.iter().filter(|h| !h.2))
                .map(|(k, v, _)| (k.as_str(), v.clone()))
                .collect();
            for (who, heads) in [(own, all), (reader, seen)] {
                let got = scored(s, who, "note", SearchMode::lexical(text)).await?;
                let one = bm25f(&decl, &heads, (text, None), &[]);
                ensure!(
                    got == one,
                    "after {after}, `{}` scored {got:?}; the one function over the heads it \
                     sees, counted again, scores {one:?}",
                    who.id
                );
            }
            Ok::<_, Failure>(())
        }
    };
    let head = |code: &str, alt: &str, title: Option<&str>, body: &str| {
        let mut v = json!({"code": code, "alt": alt, "body": body});
        if let Some(t) = title {
            v["title"] = json!(t);
        }
        (code.to_owned(), v, false)
    };
    let mut heads: Heads = vec![
        head("a", "z", Some("River"), "the river runs"),
        head(
            "b",
            "y",
            None,
            "a long river walk along the banks of the river",
        ),
        head("c", "x", Some("Running"), "water"),
        head("d", "w", None, "stones"),
    ];
    for (key, fields, _) in &heads {
        l.upsert("note", key, fields.clone()).await?;
    }
    recounted(&decl, &heads, "the writes").await?;
    heads[1] = head("b", "y", None, "river");
    l.upsert("note", "b", heads[1].1.clone()).await?;
    recounted(&decl, &heads, "a change").await?;
    l.retire("note", "c").await?;
    heads.remove(2);
    recounted(&decl, &heads, "a retirement").await?;
    restrict(s, "note", "a", "legal hold").await?;
    heads[0].2 = true;
    recounted(&decl, &heads, "a hold").await?;
    let (ty, key) = (TypeName::from("note"), RecordKey::from("a"));
    s.unrestrict(&ty, &key).await.must("lifting the hold")?;
    heads[0].2 = false;
    recounted(&decl, &heads, "a release").await?;
    restrict(s, "note", "b", "legal hold").await?;
    heads[1].2 = true;
    decl = note("alt", "body", true);
    l.migrate(decl.clone()).await?;
    for h in &mut heads {
        h.0 = h.1["alt"].as_str().unwrap_or_default().to_owned();
    }
    recounted(&decl, &heads, "a re-keying migration").await?;
    decl = note("alt", "text", false);
    l.migrate_renaming(decl.clone(), &[("body", "text")])
        .await?;
    for h in &mut heads {
        let fields = h.1.as_object_mut().expect("an object");
        let body = fields.remove("body");
        fields.extend(body.map(|b| ("text".to_owned(), b)));
    }
    recounted(&decl, &heads, "a migration renaming `body`").await?;
    let fed = json!({"code": "f", "alt": "v", "text": "river river run"});
    l.ingest("crm", vec![source_record("note", "v", fed.clone())])
        .await
        .must("ingesting from crm")?;
    heads.push(("v".to_owned(), fed, false));
    recounted(&decl, &heads, "an ingest").await?;
    let crm = SourceId::from("crm");
    let report = s.forget_preflight(&crm).await.must("the preflight")?;
    let forgot = s.forget_source(&crm, Some(&report.report_id)).await;
    forgot.must("forgetting crm")?;
    heads.pop();
    recounted(&decl, &heads, "forgetting the source").await?;
    s.rebuild().await.must("rebuilding the projection")?;
    recounted(&decl, &heads, "a rebuild").await
}

async fn a_search_as_of_a_position_counts_no_field_hidden_on_a_record_there(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    let fields = vec![
        field("doc", StorageKind::Text),
        searchable("title"),
        searchable("body"),
    ];
    let decl = type_decl("note", fields);
    l.declare(decl.clone()).await?;
    let mut heads = [
        (
            "n1",
            json!({"doc": "secret", "title": "taxi", "body": "rome rome"}),
        ),
        ("n2", json!({"doc": "open", "title": "bus", "body": "rome"})),
        ("n3", json!({"doc": "open", "title": "taxi"})),
    ];
    for (key, fields) in &heads {
        l.upsert("note", key, fields.clone()).await?;
    }
    let hide = json!([{"field": "doc", "values": ["secret"], "fields": {"body": "none"}}]);
    let grants = json!({"default": "read", "types": {"note": {"hide_where": hide}}});
    let bea = holder(s, "bea", grants);
    let then = s.head().await.must("reading the head")?;
    // n1 is secret at `then` and open at the head.
    l.upsert("note", "n1", json!({"doc": "open"})).await?;
    heads[0].1["doc"] = json!("open");
    let text = "taxi rome";
    let mut wrong = Vec::new();
    for (at, told) in [
        (Some(then), json!({"title": "taxi"})),
        (None, heads[0].1.clone()),
    ] {
        let mut asked = json!({"type": "note", "mode": "lexical", "text": text});
        if let Some(at) = at {
            asked["as_of"] = json!(at.0);
        }
        let hits = l.cat.search(&bea, de(asked.clone())).await;
        let hits = hits.must(&format!("bea's search {asked}"))?.data.hits;
        let got: Vec<(String, f64)> = (hits.iter())
            .map(|h| (h.record.key.0.clone(), h.score))
            .collect();
        let mut seen = heads.clone();
        seen[0].1 = told;
        let one = bm25f(&decl, &seen, (text, None), &[]);
        if got != one {
            wrong.push(format!("{asked} scored {got:?}, not {one:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "bea, told n1 without its body at `then`, is scored over the notes as told: {wrong:?}"
    );
    Ok(())
}
