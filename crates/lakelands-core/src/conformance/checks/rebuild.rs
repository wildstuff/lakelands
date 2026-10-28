//! The projection is a product of the log
//! (`docs/decisions/001-the-log-is-the-truth.md`, `docs/seams.md`).
//!
//! Rebuilding a lake's projection from its log gives the state it
//! replaces, byte for byte: every record, every version, every piece of
//! provenance, every derived field and view record, at the head and at
//! every earlier position. Anything the projection holds that the log
//! cannot reproduce would be lost on the first rebuild, which is why the
//! check reads everything before and after and compares.

use crate::conformance::fixture::{
    derived, doors, get_at, owner, reference, source_record, step, type_of,
};
use crate::conformance::prelude::*;

check!(
    REBUILD_ANSWERS_AS_BEFORE = a_rebuilt_projection_answers_every_read_as_the_live_one_did,
    "001, 040 and the engine seam: a projection rebuilt from the log is the one it replaces, byte for byte, at the head and as of every earlier position, the `initial` values the fold filled (050), a type migration that converts records and a re-key that rewrites references to moved keys included"
);

check!(
    REBUILD_REPRODUCES_THE_CHAIN = a_rebuild_reproduces_the_chain,
    "001 and 020: the log is the truth and a rebuild only re-derives the projection, so verifying the chain answers the same range and hash before and after"
);

check!(
    REBUILD_FOLDS_OVER_RECORDS = a_rebuild_folds_a_page_over_the_records_the_pages_before_wrote,
    "001, 027 and the engine seam: a rebuild folds the log in pages of 1 024 events, each over the projection the pages before left; a page that holds a write, a re-keying migration, a write at a new key and one at an old, and a second migration, over records an earlier page wrote, leaves what the live lake's appends left, at the head and as of each of them"
);

/// Everything the check reads: every record of every type with
/// provenance at the head, and `invoice` `i1` as of every write.
async fn snapshot(
    store: &dyn LakeStore,
    who: &Principal,
    positions: &[Position],
) -> Result<Vec<(String, Vec<Record>)>, Failure> {
    let cat = doors(store);
    let mut out = Vec::new();
    for ty in ["invoice", "partner_totals"] {
        out.push((
            ty.to_owned(),
            all_found(cat.as_ref(), who, ty, find_at(None)).await?,
        ));
    }
    for at in positions {
        let label = format!("invoice as of {at}");
        let find = find_at(Some(*at));
        out.push((label, all_found(cat.as_ref(), who, "invoice", find).await?));
        let got = cat
            .get(who, &TypeName::from("invoice"), get_at("i1", Some(*at)))
            .await
            .must(&format!("reading `i1` as of {at}"))?;
        out.push((format!("i1 as of {at}"), vec![got.data]));
    }
    Ok(out)
}

async fn a_rebuilt_projection_answers_every_read_as_the_live_one_did(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { own, .. } = &l;

    let gross = derived("gross", StorageKind::Decimal, "amount * 1.19", &["amount"]);
    // A field the fold fills when a record comes to exist (050).
    let mut state = field("state", StorageKind::Text);
    state.initial = Some(json!("open"));
    let invoice = type_decl(
        "invoice",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
            gross,
            state,
        ],
    );
    l.declare(invoice).await?;
    let fields = vec![
        field("partner", StorageKind::Text),
        field("total", StorageKind::Decimal),
    ];
    let measures = [("total", "sum(amount)")];
    let view = view_of("partner_totals", "invoice", &["partner"], fields, &measures);

    let mut positions = Vec::new();
    for (nr, partner, amount) in [("i1", "p1", "10.00"), ("i2", "p1", "5.50")] {
        let written = l.upsert("invoice", nr, json!({"partner": partner, "amount": amount}));
        let written = written.await?;
        positions.push(written.position());
    }
    // Declared after records exist, then kept by later writes: a rebuild
    // has to redo both.
    l.declare(view).await?;
    for (nr, fields) in [
        ("i1", json!({"amount": "20.00"})),
        ("i3", json!({"partner": "p2", "amount": "3.00"})),
        ("i1", json!({"partner": null})),
    ] {
        positions.push(l.upsert("invoice", nr, fields).await?.position());
    }
    let retired = l.retire("invoice", "i2").await?;
    positions.push(retired.position());

    // A type migration that converts every record, and a re-key that moves
    // records and rewrites the references to them: a rebuild folds them in
    // the same page as the records they change.
    let migrated = migrations(store.as_ref()).await?;
    let head = store.head().await.must("reading the head")?;
    let mut live = snapshot(store.as_ref(), own, &positions).await?;
    live.extend(migrated_snapshot(store.as_ref(), own, &migrated).await?);
    let through = store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        through == head,
        "the lake's head is {head}, but the projection was rebuilt through {through}; a \
         rebuild folds the whole log"
    );
    let mut rebuilt = snapshot(store.as_ref(), own, &positions).await?;
    rebuilt.extend(migrated_snapshot(store.as_ref(), own, &migrated).await?);

    for ((label, before), (_, after)) in live.iter().zip(&rebuilt) {
        ensure!(
            before == after,
            "after the rebuild, `{label}` reads differently.\n  before: {before:?}\n  \
             after:  {after:?}\nA projection rebuilt from the log must match the one it \
             replaces in every value, version and piece of provenance"
        );
    }
    Ok(())
}

/// A `memo` type whose `pages` a migration turns from number to text,
/// then the re-key fixture (`account` re-keyed by its IBAN, `payment`
/// referencing it). Answers the positions of both migrations and the head.
async fn migrations(store: &dyn LakeStore) -> Result<Vec<Position>, Failure> {
    use lakelands_api::ontology::FieldType;
    use lakelands_api::write::Migration;
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    let memo = type_of(
        "memo",
        &[("title", StorageKind::Text), ("pages", StorageKind::Number)],
    );
    l.declare(memo.clone()).await?;
    for (k, pages) in [("a", 3), ("b", 12)] {
        l.upsert("memo", k, json!({"pages": pages})).await?;
    }
    let mut to = memo;
    for f in &mut to.fields {
        if f.name.as_str() == "pages" {
            f.ty = FieldType::kind(StorageKind::Text);
        }
    }
    let m = Migration::to(to);
    let report = cat.migrate(own, m.clone());
    let report = report.await.must("the preflight of retyping `pages`")?.data;
    let retyped = cat
        .migrate_apply(own, m, &report.report_id)
        .await
        .must("retyping `pages` from number to text")?
        .position();
    super::rekey::lake(store, super::rekey::IBANS).await?;
    let rekeyed = super::rekey::apply(store).await?;
    let head = store.head().await.must("reading the head")?;
    Ok(vec![retyped, rekeyed, head])
}

/// Every record of the migrated types, with provenance, as of each of
/// `positions` (those [`migrations`] answers).
async fn migrated_snapshot(
    store: &dyn LakeStore,
    who: &Principal,
    positions: &[Position],
) -> Result<Vec<(String, Vec<Record>)>, Failure> {
    let cat = doors(store);
    let mut out = Vec::new();
    for (i, at) in positions.iter().enumerate() {
        // `account` and `payment` exist from the re-key's fixture on.
        let types: &[&str] = if i == 0 {
            &["memo"]
        } else {
            &["memo", "account", "payment"]
        };
        for ty in types {
            let label = format!("{ty} as of {at}");
            out.push((
                label,
                all_found(cat.as_ref(), who, ty, find_at(Some(*at))).await?,
            ));
        }
    }
    Ok(out)
}

async fn a_rebuild_folds_a_page_over_the_records_the_pages_before_wrote(store: Store) -> Held {
    // The re-key's fixture, then enough events to end a rebuild's first
    // page: what follows is folded over a projection that holds the fixture.
    let l = super::rekey::lake(store.as_ref(), super::rekey::IBANS).await?;
    l.declare(type_decl("filler", vec![sourced("n", StorageKind::Number)]))
        .await?;
    let filler = (0..1_100).map(|i| source_record("filler", &format!("f{i:04}"), json!({"n": i})));
    let fut = l.ingest("load", filler.collect());
    fut.await.must("loading the filler")?;
    let payment = |amount: &str| -> lakelands_api::ontology::TypeDecl {
        let fields = json!([{"name": "account", "type": {"kind": "reference", "target": "account"}},
                            {"name": "amount", "type": {"kind": amount}}]);
        let relations = json!([{"name": "pays", "target": "account", "via": "account"}]);
        serde_json::from_value(json!({"name": "payment", "fields": fields, "relations": relations}))
            .expect("a type")
    };
    let moved = json!({"holder": "moved"});
    let fresh = json!({"number": "9", "iban": "1", "kind": "giro", "meta": {}});
    let at = [
        l.upsert("payment", "p1", json!({"amount": 6}))
            .await?
            .position(),
        super::rekey::apply(store.as_ref()).await?,
        l.upsert("account", "DE1", moved).await?.position(),
        l.upsert("account", "1", fresh).await?.position(),
        l.migrate(payment("text")).await?.position(),
    ];
    ensure!(
        at[0].0 >= 1_024 && at[4].0 < 2_048,
        "the check's five appends stand in the second page of 1 024 events: {at:?}"
    );
    let read = || async {
        let mut out = Vec::new();
        for ty in ["account", "payment", "entry", "sys.link.account.owner"] {
            for at in at.iter().map(|p| Some(*p)).chain([None]) {
                out.push((
                    format!("{ty} as of {at:?}"),
                    l.all_found(ty, find_at(at)).await?,
                ));
            }
        }
        Ok::<_, Failure>(out)
    };
    let live = read().await?;
    store.rebuild().await.must("rebuilding the projection")?;
    for ((label, before), (_, after)) in live.iter().zip(&read().await?) {
        ensure!(
            before == after,
            "after the rebuild, `{label}` reads differently.\n  before: {before:?}\n  \
             after:  {after:?}\nA page folded over records an earlier page wrote must \
             leave what the live lake's appends left"
        );
    }
    Ok(())
}

check!(
    REBUILD_RECOMPUTES_IN_A_PAGE = a_rebuild_folds_a_page_that_computes_fields_again,
    "001, 022 and the engine seam (lakelands#523): a rebuild folds a page of 1 024 events that holds a function declared again with a change, the outputs recorded for it, a write, and a `recompute`, over records an earlier page wrote; it leaves what the live lake's appends left, at the head and as of every event of the page, and calls no backing"
);

async fn a_rebuild_folds_a_page_that_computes_fields_again(store: Store) -> Held {
    use crate::conformance::backing::{made, plug, unplug};
    let token = "rebuild_recomputes_in_a_page";
    let config = |round: u32| json!({"token": token, "echo": "code", "round": round});
    let l = super::backings::places(store.as_ref(), config(1)).await?;
    for (key, code) in [("p1", "AT"), ("p2", "DE")] {
        l.upsert("place", key, json!({ "code": code })).await?;
    }
    // Enough events to end a rebuild's first page: what follows is folded
    // over a projection that holds the two places, each with its name.
    l.declare(type_decl("filler", vec![sourced("n", StorageKind::Number)]))
        .await?;
    let filler = (0..1_100).map(|i| source_record("filler", &format!("f{i:04}"), json!({"n": i})));
    let fut = l.ingest("load", filler.collect());
    fut.await.must("loading the filler")?;
    // The function changes the call it makes while its backing refuses:
    // every name is cleared and a failure recorded for each call. A write
    // reads the failure back. Then `recompute` calls again, and the outputs
    // fill all three.
    let from = store.head().await.must("reading the head")?.next();
    unplug(token);
    let lookup = json!({"declare": "function", "name": "lookup", "backing": crate::conformance::backing::KIND,
        "inputs": [{"name": "code", "type": {"kind": "text"}}], "config": config(2)});
    l.declare_json(lookup).await?;
    l.upsert("place", "p3", json!({"code": "AT"})).await?;
    plug(token);
    let again = lakelands_api::write::Recompute::Function("lookup".into());
    let recomputed = l.cat.recompute(&l.own, again).await;
    let head = recomputed.must("the `recompute` door")?.position();
    let p1 = l.read("place", "p1").await?;
    ensure!(
        from.0 >= 1_024
            && head.0 < 2_048
            && crate::conformance::fixture::value(&p1, "name") == Some(&json!("AT")),
        "the check's appends stand in the second page of 1 024 events, from {from} to {head}, \
         and leave `p1` its name again: {p1:?}"
    );
    let read = || async {
        let mut out = Vec::new();
        for at in (from.0..=head.0).map(Position) {
            let places = l.all_found("place", find_at(Some(at))).await?;
            out.push((format!("place as of {at}"), places));
        }
        Ok::<_, Failure>(out)
    };
    let (live, calls) = (read().await?, made(token));
    unplug(token);
    store.rebuild().await.must("rebuilding the projection")?;
    for ((label, before), (_, after)) in live.iter().zip(&read().await?) {
        ensure!(
            before == after,
            "after the rebuild, `{label}` reads differently.\n  before: {before:?}\n  \
             after:  {after:?}\nA page that computes fields again must leave what the live \
             lake's appends left"
        );
    }
    ensure!(
        made(token) == calls,
        "the rebuild reads the recorded outputs back: {calls} calls before it, {} after",
        made(token)
    );
    Ok(())
}

async fn a_rebuild_reproduces_the_chain(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("memo", vec![field("text", StorageKind::Text)]))
        .await?;
    for (k, t) in [("a", "one"), ("b", "two"), ("a", "three")] {
        l.upsert("memo", k, json!({ "text": t })).await?;
    }
    let before = store.verify(None, None).await.must("verifying the chain")?;
    store.rebuild().await.must("rebuilding the projection")?;
    let after = store.verify(None, None);
    let after = after.await.must("verifying the chain after the rebuild")?;
    ensure!(
        before == after,
        "verifying the whole chain answered {before:?} before the rebuild and {after:?} \
         after it; a rebuild must leave the log, and so its chain, as it was"
    );
    Ok(())
}

check!(
    INDEXES_AFTER_REBUILD = after_a_rebuild_every_index_answers_as_before,
    "001 and the engine seam: the projection and its indexes are derived from the log, so a find by a field, a lexical and a vector search and a walk over a relation answer by the type's declaration in force, for records written before it was declared again and for records a migration writes again, and after a rebuild they answer as they did"
);

async fn after_a_rebuild_every_index_answers_as_before(store: Store) -> Held {
    use lakelands_api::ontology::RelationDecl;
    use lakelands_api::query::{Projection, Search, SearchMode, Walk};
    let l = Lake::of(store.as_ref());
    // Declared first with nothing to search and no relation, and with a
    // `tag` a migration takes away later.
    let note = |indexed: bool, tag: bool| {
        let mut text = field("text", StorageKind::Text);
        text.searchable = indexed;
        let mut fields = vec![text, vector("embedding", 2), reference("parent", "note")];
        fields.extend(tag.then(|| field("tag", StorageKind::Text)));
        let mut ty = type_decl("note", fields);
        ty.language = Some("en".into());
        ty.relations.extend(indexed.then(|| RelationDecl {
            name: "under".into(),
            target: "note".into(),
            via: Some("parent".into()),
            ..Default::default()
        }));
        ty
    };
    l.declare(note(false, true)).await?;
    l.upsert(
        "note",
        "root",
        json!({"text": "harbour plans", "embedding": [1.0, 0.0], "tag": "a"}),
    )
    .await?;
    l.upsert(
        "note",
        "leaf",
        json!({"text": "crane rental", "embedding": [0.0, 1.0], "parent": "root", "tag": "b"}),
    )
    .await?;
    // Declared again, the type's text is searched and its relation walked:
    // the records written before are indexed under it, none written again.
    l.declare(note(true, true)).await?;

    let answers = |store: Store| async move {
        let cat = doors(store.as_ref());
        let own = owner(store.as_ref());
        let found = all_found(
            cat.as_ref(),
            &own,
            "note",
            find_where("text = 'crane rental'"),
        )
        .await?;
        let search = |mode: SearchMode| Search {
            ty: TypeName::from("note"),
            mode,
            k: 10,
            cursor: None,
            filter: None,
            with_meta: false,
            matches: false,
            at: ReadAt::default(),
            rerank: None,
            text_as_of: false,
        };
        let lexical = cat.search(&own, search(SearchMode::lexical("harbour")));
        let lexical = lexical.await.must("searching \"harbour\"")?.data;
        let vector = cat.search(
            &own,
            search(SearchMode::vector("embedding", vec![0.0, 1.0])),
        );
        let vector = vector.await.must("searching near (0, 1)")?.data;
        let walk = pattern(
            "note",
            Some("leaf"),
            vec![step("under", Walk::Out)],
            Projection::Graph,
        );
        let graph = cat.query(&own, walk);
        let graph = graph.await.must("walking `under` from `leaf`")?.data;
        // What each index answers: which records, in which order, with
        // which scores, and which links.
        let ranked = |h: &lakelands_api::query::Hits| {
            h.hits
                .iter()
                .map(|h| serde_json::json!({"key": h.record.key, "score": h.score}))
                .collect::<Vec<_>>()
        };
        let (nodes, edges) = match graph {
            lakelands_api::query::QueryResult::Graph { nodes, edges, .. } => (
                nodes.into_iter().map(|n| n.key.0).collect::<Vec<_>>(),
                serde_json::to_value(&edges).unwrap_or_default(),
            ),
            other => return Err(Failure::new(format!("a graph walk answered {other:?}"))),
        };
        Ok::<_, Failure>(serde_json::json!({
            "found": found.iter().map(|r| r.key.0.clone()).collect::<Vec<_>>(),
            "lexical": ranked(&lexical),
            "vector": ranked(&vector),
            "nodes": nodes,
            "edges": edges,
        }))
    };
    let declared = answers(store.clone()).await?;
    ensure!(
        declared["found"] == json!(["leaf"])
            && declared["lexical"][0]["key"] == json!("root")
            && declared["vector"][0]["key"] == json!("leaf")
            && declared["edges"].as_array().is_some_and(|e| e.len() == 1),
        "once the type is declared again the indexes must answer: find `leaf`, \"harbour\" → \
         root, near (0, 1) → leaf, one edge leaf → root; they answered {declared}"
    );
    // A migration writes every record again; what it indexes is what the
    // declaration left, and what a rebuild leaves.
    l.migrate(note(true, false)).await?;
    let before = answers(store.clone()).await?;
    ensure!(
        before == declared,
        "a migration that takes `tag` away leaves the indexes answering as they did: \
         {declared} became {before}"
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let after = answers(store.clone()).await?;
    ensure!(
        before == after,
        "after the rebuild the indexes must answer as before: {before} became {after}"
    );
    Ok(())
}

check!(
    ONE_JSON_FORM =
        a_number_and_a_vector_read_back_in_their_one_json_form_before_and_after_a_rebuild,
    "the log format §1.2 and 001: every value a door answers is in its canonical JSON form (`1`, never `1.0`), so a read answers the same text live and after a rebuild"
);

async fn a_number_and_a_vector_read_back_in_their_one_json_form_before_and_after_a_rebuild(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let v = vector("v", 3);
    l.declare(type_decl(
        "sample",
        vec![
            field("n", StorageKind::Number),
            field("m", StorageKind::Number),
            v,
        ],
    ))
    .await?;
    l.upsert(
        "sample",
        "s",
        json!({"n": 2.0, "m": 1.5, "v": [0.0, 1.0, 0.25]}),
    )
    .await?;
    let text = |store: Store| async move {
        let r = Lake::of(store.as_ref()).read("sample", "s").await?;
        Ok::<_, Failure>(serde_json::to_string(&r.fields).unwrap_or_default())
    };
    let canonical = r#"{"m":1.5,"n":2,"v":[0,1,0.25]}"#;
    let live = text(store.clone()).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = text(store.clone()).await?;
    ensure!(
        live == canonical && rebuilt == canonical,
        "`s` must read {canonical} (numbers in their one form) live and after a \
         rebuild; it read {live} live and {rebuilt} after"
    );
    Ok(())
}
