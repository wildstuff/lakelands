//! A field may be hidden on only the records a condition selects
//! (`docs/decisions/006-grants-not-roles.md`).
//!
//! A grant's `hide_where` lowers fields on the records whose field `G`
//! holds one of its values. Every read is evaluated over the record as
//! its reader is told it: on a selected record a lowered field, and what
//! is built from it, is absent or masked, at every door.

use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::write::{Change, ChangeTo, Outcome, ProposedWrite};

use super::only::{de, finds, get, hit_keys, holder};
use crate::conformance::fixture::{derived, reference, step};
use crate::conformance::prelude::*;

check!(
    HIDDEN_NOT_ANSWERED = a_field_hidden_where_a_condition_holds_is_not_answered_on_those_records,
    "006: a field a grant's `hide_where` lowers to `none` is not answered on a record whose condition field holds one of its values, nor is a derived field built from it; one lowered to `masked` is named masked there; on any other record the type's access stands"
);

check!(
    HIDDEN_NOT_TOLD =
        a_field_hidden_where_a_condition_holds_tells_nothing_through_a_filter_order_or_aggregate,
    "006: a `where`, an order, a total and a measure over a field hidden on the selected records read it there as null, and a filter over a field no hiding lowers still answers those records"
);

check!(
    HIDDEN_NO_LEXICAL = a_field_hidden_where_a_condition_holds_brings_no_lexical_match_or_rank,
    "006 and 025: a lexical search, over every searchable field or over the hidden one named, matches a selected record through no hidden field, and the records it is told rank as they would without it: a selected record matched through another field scores as a twin that holds no hidden field"
);

check!(
    HIDDEN_NO_VECTOR = a_vector_built_from_a_field_hidden_where_a_condition_holds_brings_no_hit,
    "006 and 078: a vector hidden on the selected records brings none of them as a candidate of a vector search, and a hybrid search fuses the axes as told, so a record ranks among the vectors its reader is told"
);

check!(
    HIDDEN_VIEW =
        a_view_reading_a_field_hidden_where_a_condition_holds_is_computed_from_what_is_told,
    "006 and 049: a view whose measure reads a field hidden on the selected records answers their holder, kept or not, the measure over the records as told, and the owner as kept"
);

check!(
    HIDDEN_NOT_WRITTEN = a_field_hidden_where_a_condition_holds_is_not_written_on_those_records,
    "006 and 012: a write of a field hidden on the record it writes, selected before the write or after it, is refused whole as not granted, naming the field, to a writer that writes it on every other record, appending nothing; a write of other fields of the record is made"
);

check!(
    HIDING_UNAPPLIED = a_hiding_on_a_field_its_holder_may_not_read_is_not_applied,
    "006: a hiding whose condition field its holder may not read, or that lowers its own condition field, a key field or one its `only` names, or that is held on a view, refuses `get`, `find` and `search` of the type as invalid, naming the type and never the field; a view that reads such a type is computed with every field the hiding names hidden on every record"
);

check!(
    HIDDEN_DESCRIBED = describe_tells_the_holder_where_a_field_is_hidden,
    "006: `describe_type` tells the holder of a hiding each field it lowers, and what is built from it, with the condition field and its values, the field's access kept as the type gives it, and a schema hash of its own; a reader without the hiding is told none"
);

check!(
    HIDDEN_AS_OF = a_read_as_of_a_position_judges_a_hiding_by_the_condition_then,
    "006: a read as of a position selects a record by its condition field then, so a record selected since is answered its field as of before and not at the head; the hiding follows a rename of its condition field"
);

check!(
    HIDDEN_SEARCH_AS_OF =
        a_search_as_of_a_position_matches_no_field_hidden_on_a_record_selected_then,
    "006 and 025: a search as of a position, lexical or by vector, reaches no field on a record its condition field selects there or at the head: it is neither matched nor ranked by one, named in its matches or answered it, before the record was selected and after, its fields and its link type renamed since; every other record is found and answered as at any position"
);

check!(
    HIDDEN_RERANKED =
        a_reranked_window_neither_ranks_nor_reranks_by_a_field_hidden_where_a_condition_holds,
    "006 and 067: a hybrid search with `rerank` ranks a selected record in its first stage and its second by its fields as told: it scores as a twin that holds no hidden field, is answered and named in its matches without it, and is handed the reranker without its text; its other fields still rank it"
);

check!(
    HIDDEN_NO_EDGE = an_edge_built_from_a_field_hidden_where_a_condition_holds_is_not_walked_there,
    "006 and 044: a walk over a relation derived from a field hidden on the selected records, or carried by a reference field hidden there, crosses no edge of a selected record, out or in; on any other record the edge stands"
);

check!(
    HIDDEN_PROPOSED = a_proposal_over_a_field_hidden_where_a_condition_holds_is_decided_as_told,
    "006 and 013: a reviewer holding a hiding is listed a proposed write of a selected record without the hidden fields in its patch, and its accept is refused not granted, appending nothing, the proposal pending; one on any other record is listed whole and accepted"
);

check!(
    HIDDEN_CHANGED =
        a_change_to_records_over_a_field_hidden_where_a_condition_holds_tells_nothing_of_it,
    "006 and 081: a change to records whose condition reads a field hidden on the selected records reads it there as null, so their value of it neither selects them nor moves the plan's id; one that sets a field hidden on a record it selects is told refused not granted for that record and refuses its `enact` whole, appending nothing"
);

/// `chunk` with the condition field `doc`, the text `body`, the derived
/// `echo` of it, a number `n` and a vector `v`: k1 (open, taxi rome, 1),
/// k2 (secret, taxi paris, 2), k3 (open, hotel, 3). Answers the head.
async fn chunks(l: &Lake) -> Result<Position, Failure> {
    l.declare(chunk("doc")).await?;
    let mut head = Position::GENESIS;
    for (key, doc, body, n, v) in [
        ("k1", "open", "taxi rome", 1, [1.0, 0.0]),
        ("k2", "secret", "taxi paris", 2, [1.0, 0.1]),
        ("k3", "open", "hotel", 3, [0.0, 1.0]),
    ] {
        let fields = json!({"doc": doc, "body": body, "n": n, "v": v});
        head = l.upsert("chunk", key, fields).await?.position();
    }
    Ok(head)
}

/// `chunk` as [`chunks`] declares it, its condition field named `doc`.
fn chunk(doc: &str) -> lakelands_api::ontology::TypeDecl {
    let fields = vec![
        field(doc, Text),
        searchable("body"),
        derived("echo", Text, "body", &["body"]),
        field("n", Number),
        vector("v", 2),
    ];
    type_decl("chunk", fields)
}

/// `ann` reads every record, with `access` on `chunk`, and on a `secret`
/// chunk reads neither `body` nor `v`, and `n` masked; `hiding` adds to
/// the hiding.
fn ann(store: &dyn LakeStore, access: &str, hiding: Value) -> Principal {
    let mut hide = json!({"field": "doc", "values": ["secret"],
                          "fields": {"body": "none", "v": "none", "n": "masked"}});
    if let (Some(h), Value::Object(more)) = (hide.as_object_mut(), hiding) {
        h.extend(more);
    }
    let chunk = json!({"default": access, "hide_where": [hide]});
    let grants = json!({"default": "read", "restricted": true, "types": {"chunk": chunk}});
    holder(store, "ann", grants)
}

/// The fields a record is answered, and those named masked.
fn told(r: &lakelands_api::Record) -> String {
    let fields: Vec<&str> = r.fields.keys().map(|f| f.as_str()).collect();
    let masked: Vec<&str> = r.masked.iter().map(|f| f.as_str()).collect();
    format!("{} masked={}", fields.join(" "), masked.join(" "))
}

async fn a_field_hidden_where_a_condition_holds_is_not_answered_on_those_records(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let mut wrong = Vec::new();
    for (key, want) in [("k1", "body doc echo n v masked="), ("k2", "doc masked=n")] {
        let r = get(&l, &ann, "chunk", key, None).await;
        let said = told(&r.must(&format!("ann's `get` of `{key}`"))?.data);
        if said != want {
            wrong.push(format!("`{key}`: `{said}`, not `{want}`"));
        }
    }
    let ty = TypeName::from("chunk");
    let found = l
        .cat
        .find(&ann, &ty, de(json!({})))
        .await
        .must("ann's `find` of `chunk`")?
        .data;
    let lakelands_api::read::Found::Records { rows, .. } = &found else {
        return Err(Failure::new(format!(
            "a find of records answered {found:?}"
        )));
    };
    let rows: Vec<String> = rows
        .iter()
        .map(|r| format!("{}: {}", r.key, told(r)))
        .collect();
    let want = [
        "k1: body doc echo n v masked=",
        "k2: doc masked=n",
        "k3: body doc echo n v masked=",
    ];
    if rows != want {
        wrong.push(format!("the find: {rows:?}, not {want:?}"));
    }
    ensure!(
        wrong.is_empty(),
        "ann must not be answered what is hidden on a secret chunk: {wrong:?}"
    );
    Ok(())
}

const TOLD: &str = r#"
    chunk | a filter on the hidden field | {"where": "body = 'taxi paris'", "aggregate": {"measures": [{"name": "c", "func": "count"}]}} | c=0
    chunk | a filter on what is built from it | {"where": "echo = 'taxi paris'", "aggregate": {"measures": [{"name": "c", "func": "count"}]}} | c=0
    chunk | a filter on a masked field | {"where": "n = 2", "aggregate": {"measures": [{"name": "c", "func": "count"}]}} | c=0
    chunk | a filter on the condition | {"where": "doc = 'secret'"} | k2
    chunk | a filter on another field | {"where": "n > 1 or doc = 'secret'"} | k2 k3
    chunk | an order | {"order_by": [{"field": "body", "dir": "desc"}], "where": "body != ''"} | k1 k3
    chunk | a total | {"where": "body != ''", "total": true} | k1 k3 total=2
    chunk | a sum | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "n"}]}} | s=4
    chunk | a count of the field | {"aggregate": {"measures": [{"name": "c", "func": "count", "field": "body"}]}} | c=2
"#;

async fn a_field_hidden_where_a_condition_holds_tells_nothing_through_a_filter_order_or_aggregate(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    finds(&l, &ann, TOLD, Position::GENESIS).await
}

/// `who`'s search of `chunk`, `asked` its JSON.
async fn search(l: &Lake, who: &Principal, asked: Value) -> Result<Hits, Failure> {
    let mut s = asked;
    s["type"] = json!("chunk");
    let what = format!("{}'s search {s}", who.id);
    Ok(l.cat.search(who, de(s)).await.must(&what)?.data)
}

/// The keys of each search of `rows`, one a line `what | search | keys`,
/// that `who` is not answered as its line says.
async fn searches(l: &Lake, who: &Principal, rows: &str) -> Result<Vec<String>, Failure> {
    let mut wrong = Vec::new();
    for row in rows.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let [what, asked, want] = row.split('|').map(str::trim).collect::<Vec<_>>()[..] else {
            panic!("a row is three cells: {row}");
        };
        let hits = search(l, who, serde_json::from_str(asked).expect("a search reads")).await?;
        if hit_keys(&hits) != want.trim() {
            wrong.push(format!("{what}: `{}`, not `{want}`", hit_keys(&hits)));
        }
    }
    Ok(wrong)
}

const LEXICAL: &str = r#"
    every field | {"mode": "lexical", "text": "taxi"} | k1
    the hidden field | {"mode": "lexical", "text": "taxi", "fields": ["body"]} | k1
    a word only it holds | {"mode": "lexical", "text": "paris"} |
    the condition | {"mode": "lexical", "text": "secret"} |
"#;

async fn a_field_hidden_where_a_condition_holds_brings_no_lexical_match_or_rank(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let mut wrong = searches(&l, &ann, LEXICAL).await?;
    // The owner is told k2 and finds it; k1 scores alike to both.
    let asked = json!({"mode": "lexical", "text": "taxi"});
    let (mine, theirs) = (
        search(&l, &l.own, asked.clone()).await?,
        search(&l, &ann, asked).await?,
    );
    if hit_keys(&mine) != "k1 k2" {
        wrong.push(format!("the owner's: `{}`", hit_keys(&mine)));
    }
    let score = |h: &Hits| {
        h.hits
            .iter()
            .find(|h| h.record.key.as_str() == "k1")
            .map(|h| h.score)
    };
    if score(&mine) != score(&theirs) {
        wrong.push(format!(
            "k1 scores {:?} to ann, {:?} to the owner",
            score(&theirs),
            score(&mine)
        ));
    }
    // A secret note matched through its title scores as its open twin,
    // which holds no body: the hidden body moves no rank.
    let fields = vec![field("doc", Text), searchable("title"), searchable("body")];
    l.declare(type_decl("note", fields)).await?;
    for (key, doc, body) in [("n1", "secret", Some("rome")), ("n2", "open", None)] {
        let fields = json!({"doc": doc, "title": "taxi", "body": body});
        l.upsert("note", key, fields).await?;
    }
    let hide = json!([{"field": "doc", "values": ["secret"], "fields": {"body": "none"}}]);
    let grants = json!({"default": "read", "types": {"note": {"hide_where": hide}}});
    let bea = holder(store.as_ref(), "bea", grants);
    let asked: lakelands_api::query::Search =
        de(json!({"type": "note", "mode": "lexical", "text": "taxi rome"}));
    let scores = async |who: &Principal| -> Result<Vec<f64>, Failure> {
        let hits = l.cat.search(who, asked.clone()).await;
        let hits = hits.must(&format!("{}'s search of notes", who.id))?.data;
        let score = |k: &str| hits.hits.iter().find(|h| h.record.key.as_str() == k);
        Ok(["n1", "n2"]
            .map(|k| score(k).map_or(-1.0, |h| h.score))
            .to_vec())
    };
    let (told, whole) = (scores(&bea).await?, scores(&l.own).await?);
    if told[0] != told[1] || whole[0] == whole[1] {
        wrong.push(format!(
            "n1 and n2 score {told:?} to bea, {whole:?} to the owner"
        ));
    }
    ensure!(
        wrong.is_empty(),
        "a secret chunk's body must bring ann no match: {wrong:?}"
    );
    Ok(())
}

const VECTORS: &str = r#"
    vector | {"mode": "vector", "field": "v", "vector": [1.0, 0.0]} | k1 k3
    hybrid | {"mode": "hybrid", "text": "taxi", "field": "v", "vector": [1.0, 0.0]} | k1 k3
"#;

async fn a_vector_built_from_a_field_hidden_where_a_condition_holds_brings_no_hit(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let mut wrong = searches(&l, &ann, VECTORS).await?;
    // k2's vector moves no rank: k3 is second of ann's vectors.
    let asked = json!({"mode": "hybrid", "text": "taxi", "field": "v", "vector": [1.0, 0.0]});
    let hits = search(&l, &ann, asked).await?;
    let k3 = hits.hits.iter().find(|h| h.record.key.as_str() == "k3");
    if k3.is_none_or(|h| h.score != 1.0 / 61.0) {
        wrong.push(format!("hybrid: `k3` scores {k3:?}, not 1/61"));
    }
    ensure!(
        wrong.is_empty(),
        "a secret chunk's vector must bring ann no hit: {wrong:?}"
    );
    Ok(())
}

async fn a_view_reading_a_field_hidden_where_a_condition_holds_is_computed_from_what_is_told(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let fields = vec![field("total", Number)];
    l.declare(view_of(
        "sums",
        "chunk",
        &[],
        fields,
        &[("total", "sum(n)")],
    ))
    .await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let mut wrong = Vec::new();
    for kept in [false, true] {
        if kept {
            l.set("view_kept.sums", json!(true)).await?;
        }
        for (who, want) in [(&ann, "t=4"), (&l.own, "t=6")] {
            let sum =
                r#"{"aggregate": {"measures": [{"name": "t", "func": "sum", "field": "total"}]}}"#;
            let rows = format!("sums | the sum, kept {kept} | {sum} | {want}");
            if let Err(e) = finds(&l, who, &rows, Position::GENESIS).await {
                wrong.push(e.to_string());
            }
        }
    }
    ensure!(
        wrong.is_empty(),
        "a view must sum for ann what ann is told: {wrong:?}"
    );
    Ok(())
}

async fn a_field_hidden_where_a_condition_holds_is_not_written_on_those_records(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let head = chunks(&l).await?;
    let ann = ann(store.as_ref(), "write", json!({}));
    let write = |key: &str, fields: Value| {
        let (ty, key) = (TypeName::from("chunk"), RecordKey::from(key));
        let cat = Arc::clone(&l.cat);
        let ann = ann.clone();
        async move {
            cat.upsert(&ann, &ty, &key, de(fields), Default::default())
                .await
        }
    };
    let refused = Error::refused(Some("body".into()), Refusal::NotGranted);
    for (what, key, fields) in [
        (
            "a write of `body` on a secret chunk",
            "k2",
            json!({"body": "x"}),
        ),
        (
            "a write making k1 secret with its `body`",
            "k1",
            json!({"doc": "secret", "body": "x"}),
        ),
        (
            "a write making k2 open with its `body`",
            "k2",
            json!({"doc": "open", "body": "x"}),
        ),
    ] {
        let said = write(key, fields).await.must_fail(what)?;
        ensure!(
            said == refused,
            "{what} must be refused not granted naming `body`: {said:?}"
        );
    }
    let n = store.head().await.must("reading the head")?;
    ensure!(
        n == head,
        "a refused write must append nothing: the head moved from {head:?} to {n:?}"
    );
    write("k1", json!({"body": "tram rome"}))
        .await
        .must("ann's write of k1's `body`")?;
    write("k2", json!({"doc": "secret"}))
        .await
        .must("ann's write of k2's `doc`")?;
    Ok(())
}

/// Whether `e` says that the grant's `hide_where` on `on` cannot be applied.
fn unappliable(e: &Error, on: &str) -> bool {
    matches!(e.case(), Some(Said { case: Case::HidingUnappliable { ty }, .. }) if ty == on)
}

async fn a_hiding_on_a_field_its_holder_may_not_read_is_not_applied(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let mut tome = type_decl("tome", vec![field("code", Text), field("doc", Text)]);
    tome.key = vec!["code".into()];
    l.declare(tome).await?;
    let fields = vec![field("doc", Text), field("total", Number)];
    let per_kind = view_of(
        "per_kind",
        "chunk",
        &["doc"],
        fields,
        &[("total", "sum(n)")],
    );
    l.declare(per_kind).await?;
    let on = |ty: &str, grant: Value, hidden: &str| {
        let mut grant = grant;
        let hide = json!([{"field": "doc", "values": ["secret"], "fields": {hidden: "none"}}]);
        grant["hide_where"] = hide;
        let grants = json!({"default": "read", "types": {ty: grant}});
        holder(store.as_ref(), "ann", grants)
    };
    let unread = |fields: Value| on("chunk", json!({"fields": fields}), "body");
    let bad = [
        (
            "its condition field unread",
            "chunk",
            unread(json!({"doc": "none"})),
        ),
        (
            "its condition field masked",
            "chunk",
            unread(json!({"doc": "masked"})),
        ),
        (
            "its own condition field lowered",
            "chunk",
            ann(store.as_ref(), "read", json!({"fields": {"doc": "none"}})),
        ),
        (
            "a value it does not hold",
            "chunk",
            ann(store.as_ref(), "read", json!({"values": []})),
        ),
        (
            "a field the type does not declare",
            "chunk",
            ann(store.as_ref(), "read", json!({"field": "nowhere"})),
        ),
        (
            "a field its `only` names",
            "chunk",
            on("chunk", json!({"only": {"body": "hotel"}}), "body"),
        ),
        ("a key field", "tome", on("tome", json!({}), "code")),
        ("a view", "per_kind", on("per_kind", json!({}), "total")),
    ];
    for (what, on, who) in &bad {
        let (cat, ty) = (&l.cat, TypeName::from(*on));
        let got = get(&l, who, on, "k1", None)
            .await
            .must_fail(&format!("a `get` under {what}"))?;
        let found = cat
            .find(who, &ty, de(json!({})))
            .await
            .must_fail(&format!("a `find` under {what}"))?;
        let asked = de(json!({"type": on, "mode": "lexical", "text": "taxi"}));
        let sought = cat
            .search(who, asked)
            .await
            .must_fail(&format!("a search under {what}"))?;
        for e in [got, found, sought] {
            ensure!(
                unappliable(&e, on),
                "under {what} the type must be refused unappliable: {e:?}"
            );
            ensure!(
                !e.to_string().contains("doc"),
                "the refusal must name no field: {e}"
            );
        }
    }
    // A view over the type reads every `n` as hidden: no total.
    let sum = r#"{"aggregate": {"measures": [{"name": "t", "func": "sum", "field": "total"}]}}"#;
    let (failed, rows) = (&bad[3].2, format!("per_kind | the sum | {sum} | t=0"));
    finds(&l, failed, &rows, Position::GENESIS).await
}

async fn describe_tells_the_holder_where_a_field_is_hidden(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let bob = holder(
        store.as_ref(),
        "bob",
        json!({"default": "read", "restricted": true}),
    );
    let describe = |who: &Principal| {
        let (cat, who) = (Arc::clone(&l.cat), who.clone());
        async move {
            cat.describe_type(&who, &"chunk".into(), Default::default())
                .await
        }
    };
    let mine = describe(&ann)
        .await
        .must("ann's `describe_type` of `chunk`")?
        .data;
    let theirs = describe(&bob)
        .await
        .must("bob's `describe_type` of `chunk`")?
        .data;
    let hidden = |d: &lakelands_api::describe::TypeDescription| {
        let each = d.fields.iter().filter(|f| !f.hidden_where.is_empty());
        let each = each.map(|f| {
            let at = f.hidden_where.iter().map(|(g, v)| format!("{g}={v:?}"));
            format!(
                "{}:{:?}:{}",
                f.name,
                f.access,
                at.collect::<Vec<_>>().join(",")
            )
        });
        each.collect::<Vec<_>>().join(" ")
    };
    let want = ["body", "echo", "n", "v"].map(|f| format!(r#"{f}:Read:doc={{"secret"}}"#));
    let want = want.join(" ");
    ensure!(
        hidden(&mine) == want,
        "ann must be told `{want}`: `{}`",
        hidden(&mine)
    );
    ensure!(
        hidden(&theirs).is_empty(),
        "bob holds no hiding: `{}`",
        hidden(&theirs)
    );
    let (a, b) = (&mine.lifecycle.schema_hash, &theirs.lifecycle.schema_hash);
    ensure!(
        a != b,
        "ann is described another shape than bob, and a hash of its own: {a}"
    );
    Ok(())
}

async fn a_read_as_of_a_position_judges_a_hiding_by_the_condition_then(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    chunks(&l).await?;
    let ann = ann(store.as_ref(), "read", json!({}));
    let made = l.upsert("chunk", "k1", json!({"doc": "secret"})).await?;
    let before = Position(made.position().0 - 1);
    let body = |at: Option<Position>| {
        let ann = ann.clone();
        let l = &l;
        async move {
            let r = get(l, &ann, "chunk", "k1", at)
                .await
                .must("ann's `get` of `k1`")?;
            Ok::<_, Failure>(r.data.fields.contains_key("body"))
        }
    };
    ensure!(
        body(Some(before)).await?,
        "k1 was open before: ann must be answered its body"
    );
    ensure!(
        !body(None).await?,
        "k1 is secret at the head: ann must not be answered its body"
    );
    let mut m = crate::conformance::fixture::migration(chunk("kind"));
    m.renames = vec![("doc".into(), "kind".into())];
    let report = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .must("previewing the rename")?;
    let id = report.data.report_id;
    l.cat
        .migrate_apply(&l.own, m, &id)
        .await
        .must("renaming `doc` to `kind`")?;
    ensure!(
        body(Some(before)).await?,
        "after the rename ann must still be answered k1's body as of before"
    );
    ensure!(
        !body(None).await?,
        "after the rename the hiding must follow `doc` to `kind`"
    );
    Ok(())
}

/// `note` with the condition field `doc`, the searchable `title` and
/// `body` and a vector `v`, each of `rows` written by its key; and
/// `bea`, who reads no `body` of a secret note.
async fn notes(
    l: &Lake,
    store: &dyn LakeStore,
    rows: &[(&str, Value)],
) -> Result<Principal, Failure> {
    let fields = vec![
        field("doc", Text),
        searchable("title"),
        searchable("body"),
        vector("v", 2),
    ];
    l.declare(type_decl("note", fields)).await?;
    for (key, fields) in rows {
        l.upsert("note", key, fields.clone()).await?;
    }
    let hide = json!([{"field": "doc", "values": ["secret"], "fields": {"body": "none"}}]);
    let grants = json!({"default": "read", "types": {"note": {"hide_where": hide}}});
    Ok(holder(store, "bea", grants))
}

/// `who`'s search, `asked` its JSON, of `note` where it names no type,
/// matches asked for where it has text: each hit by key,
/// `key:fields/matched fields`, and its score.
async fn noted(l: &Lake, who: &Principal, asked: Value) -> Result<Vec<(String, f64)>, Failure> {
    let mut s = asked;
    s["matches"] = json!(s.get("text").is_some());
    if s.get("type").is_none() {
        s["type"] = json!("note");
    }
    let what = format!("{}'s search {s}", who.id);
    let hits = l.cat.search(who, de(s)).await.must(&what)?.data.hits;
    let mut said: Vec<(String, f64)> = (hits.iter())
        .map(|h| {
            let fields: Vec<&str> = h.record.fields.keys().map(|f| f.as_str()).collect();
            let matched: Vec<&str> = h.matches.keys().map(|f| f.as_str()).collect();
            let (key, fields, matched) = (&h.record.key, fields.join(" "), matched.join(" "));
            (format!("{key}:{fields}/{matched}"), h.score)
        })
        .collect();
    said.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(said)
}

/// What `said` tells of its hits, without their scores.
fn told_hits(said: &[(String, f64)]) -> String {
    let told: Vec<&str> = said.iter().map(|h| h.0.as_str()).collect();
    told.join(" ")
}

async fn a_search_as_of_a_position_matches_no_field_hidden_on_a_record_selected_then(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let v = || json!([1.0, 0.0]);
    let rows = [
        (
            "n1",
            json!({"doc": "open", "title": "taxi", "body": "rome", "v": v()}),
        ),
        ("n2", json!({"doc": "open", "title": "taxi", "v": v()})),
        (
            "n3",
            json!({"doc": "secret", "title": "bus", "body": "rome", "v": v()}),
        ),
    ];
    notes(&l, store.as_ref(), &rows).await?;
    // The same notes as links of `seen`, whose link type is renamed.
    l.declare(type_decl("partner", vec![field("name", Text)]))
        .await?;
    let seen = |name: &str| {
        let mut r = super::links::dated();
        (r.name, r.inverse) = (name.into(), None);
        r.fields = vec![field("doc", Text), searchable("body"), vector("v", 2)];
        super::links::invoice(vec![r])
    };
    l.declare(seen("seen")).await?;
    l.upsert("partner", "p1", json!({"name": "p1"})).await?;
    let link = |from: &'static str, fields: Value| {
        let (ty, rel, to) = ("invoice".into(), "seen".into(), "p1".into());
        let (cat, own, from) = (l.cat.clone(), l.own.clone(), from.into());
        async move {
            let opts = Default::default();
            let linked = cat.link(&own, &ty, &from, &rel, &to, patch(fields), opts);
            linked.await.must("linking").map(drop)
        }
    };
    for (key, doc) in [("i1", "open"), ("i2", "open"), ("i3", "secret")] {
        l.upsert("invoice", key, json!({"amount": "1"})).await?;
        link(key, json!({"doc": doc, "body": "rome", "v": v()})).await?;
    }
    let hidden = json!({"body": "none", "v": "none"});
    let hide = json!({"hide_where": [{"field": "doc", "values": ["secret"], "fields": hidden}]});
    let types = json!({"note": hide, "sys.link.invoice.seen": hide});
    let bea = holder(
        store.as_ref(),
        "bea",
        json!({"default": "read", "types": types}),
    );
    let before = store.head().await.must("reading the head")?;
    let after = l.upsert("note", "n1", json!({"doc": "secret"})).await?;
    l.upsert("note", "n3", json!({"doc": "open"})).await?;
    link("i1", json!({"doc": "secret"})).await?;
    link("i3", json!({"doc": "open"})).await?;
    // `body` is `text` at the head, and the hiding follows it there.
    let text = vec![
        field("doc", Text),
        searchable("title"),
        searchable("text"),
        vector("v", 2),
    ];
    let mut m = crate::conformance::fixture::migration(type_decl("note", text));
    m.renames = vec![("body".into(), "text".into())];
    let mut seen_saw = crate::conformance::fixture::migration(seen("saw"));
    seen_saw.relation_renames = vec![("seen".into(), "saw".into())];
    // And `seen` is `saw`, its link type with it.
    for m in [m, seen_saw] {
        let id = l.cat.migrate(&l.own, m.clone()).await.must("previewing")?;
        let id = id.data.report_id;
        let applied = l.cat.migrate_apply(&l.own, m, &id).await;
        applied.must("renaming")?;
    }
    // n3 and i3 are secret before, n1 and i1 at the head, n1 and n3 both
    // at `after`; i2 is never.
    let lexical = json!({"mode": "lexical", "text": "taxi rome"});
    let vector = json!({"mode": "vector", "field": "v", "vector": v()});
    let ty = |asked: &Value, ty: &str| {
        let mut asked = asked.clone();
        asked["type"] = json!(format!("sys.link.invoice.{ty}"));
        asked
    };
    let (at, now) = (Some(before), Some(after.position()));
    let mut wrong = Vec::new();
    for (at, asked, want) in [
        (
            at,
            lexical.clone(),
            "n1:body doc title v/body title n2:doc title v/title",
        ),
        (at, vector.clone(), "n2:doc title v/"),
        (
            now,
            lexical.clone(),
            "n1:doc title/title n2:doc title v/title",
        ),
        (now, vector.clone(), "n2:doc title v/"),
        (
            None,
            lexical.clone(),
            "n1:doc title/title n2:doc title v/title n3:doc text title v/text",
        ),
        (None, vector.clone(), "n2:doc title v/ n3:doc text title v/"),
        (at, ty(&lexical, "seen"), "i2:p1:body doc from to v/body"),
        (at, ty(&vector, "seen"), "i2:p1:body doc from to v/"),
        (
            None,
            ty(&lexical, "saw"),
            "i2:p1:body doc from to v/body i3:p1:body doc from to v/body",
        ),
        (
            None,
            ty(&vector, "saw"),
            "i2:p1:body doc from to v/ i3:p1:body doc from to v/",
        ),
    ] {
        let mut asked = asked;
        if let Some(at) = at {
            asked["as_of"] = json!(at.0);
        }
        let said = told_hits(&noted(&l, &bea, asked.clone()).await?);
        if said != want {
            wrong.push(format!("{asked}: `{said}`, not `{want}`"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a body and a vector must bring bea no hit where they are secret: {wrong:?}"
    );
    Ok(())
}

async fn a_reranked_window_neither_ranks_nor_reranks_by_a_field_hidden_where_a_condition_holds(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    // m1 is m2 but for its secret body, which the reranker scores.
    let rows = [
        (
            "m1",
            json!({"doc": "secret", "title": "reminder", "body": "overdue overdue"}),
        ),
        ("m2", json!({"doc": "open", "title": "reminder"})),
        (
            "m3",
            json!({"doc": "open", "title": "memo", "body": "overdue"}),
        ),
    ];
    let bea = notes(&l, store.as_ref(), &rows).await?;
    for (key, v) in [("m1", [1.0, 0.0]), ("m2", [1.0, 0.0]), ("m3", [0.0, 1.0])] {
        l.upsert("note", key, json!({"v": v})).await?;
    }
    let bound = store.bind_rerank(backing::reranker());
    bound.must("binding the reranker")?;
    // The first stage's text ranks m1 as m2 for bea, as its stage does.
    let text = json!({"mode": "lexical", "text": "overdue reminder"});
    let mut reranked = json!({"mode": "hybrid", "field": "v", "vector": [1.0, 0.0]});
    (reranked["text"], reranked["rerank"]) = (text["text"].clone(), json!({"top": 10}));
    let (mut wrong, since) = (Vec::new(), backing::rerank_calls().len());
    for (who, asked, want) in [
        (&bea, &text, None),
        (&l.own, &text, None),
        (&bea, &reranked, Some([1.0, 1.0, 2.0])),
        (&l.own, &reranked, Some([5.0, 1.0, 2.0])),
    ] {
        let said = noted(&l, who, asked.clone()).await?;
        let s: Vec<f64> = said.iter().map(|h| h.1).collect();
        let (bea, body) = (who.id == bea.id, "m3:body doc title v/body");
        if want.is_some_and(|w| s != w) || (s[0] == s[1]) != bea {
            wrong.push(format!("{asked}: {} scores {s:?}, not {want:?}", who.id));
        }
        let want = match bea {
            true => format!("m1:doc title v/title m2:doc title v/title {body}"),
            false => format!("m1:body doc title v/body title m2:doc title v/title {body}"),
        };
        if told_hits(&said) != want {
            wrong.push(format!("{}: `{}`, not `{want}`", who.id, told_hits(&said)));
        }
    }
    let handed = backing::rerank_calls().split_off(since).into_iter();
    let m1 = handed.map(|c| {
        (c.candidates.into_iter()).find_map(|c| (c.key.as_str() == "m1").then_some(c.text))
    });
    let m1: Vec<Option<String>> = m1.collect();
    if m1
        != [
            Some("reminder".into()),
            Some("reminder\n\noverdue overdue".into()),
        ]
    {
        wrong.push(format!("m1 handed the reranker {m1:?}"));
    }
    ensure!(
        wrong.is_empty(),
        "a secret note's body must neither rank nor rerank it for bea: {wrong:?}"
    );
    Ok(())
}

/// `paper` with the condition field `kind`, citing a `source` by its `no`
/// in the derived `cited` and filed under one by the reference `src` in
/// `filed`: p1 open and p2 secret, both of s1.
async fn papers(l: &Lake) -> Held {
    let mut source = type_decl("source", vec![field("no", Text)]);
    source.key = vec!["no".into()];
    l.declare(source).await?;
    let fields = vec![
        field("kind", Text),
        field("cites", Text),
        reference("src", "source"),
    ];
    let mut paper = type_decl("paper", fields);
    let cites = MatchPair {
        from: "cites".into(),
        to: "no".into(),
    };
    paper.relations = vec![
        RelationDecl {
            derived: Some(Derived {
                pairs: vec![cites],
                filter: None,
            }),
            ..RelationDecl::to("cited", "source")
        },
        RelationDecl {
            via: Some("src".into()),
            ..RelationDecl::to("filed", "source")
        },
    ];
    l.declare(paper).await?;
    l.upsert("source", "s1", json!({"no": "s1"})).await?;
    for (key, kind) in [("p1", "open"), ("p2", "secret")] {
        let fields = json!({"kind": kind, "cites": "s1", "src": "s1"});
        l.upsert("paper", key, fields).await?;
    }
    Ok(())
}

async fn an_edge_built_from_a_field_hidden_where_a_condition_holds_is_not_walked_there(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    papers(&l).await?;
    let hide =
        json!({"field": "kind", "values": ["secret"], "fields": {"cites": "none", "src": "none"}});
    let paper = json!({"hide_where": [hide]});
    let ann = holder(
        store.as_ref(),
        "ann",
        json!({"default": "read", "types": {"paper": paper}}),
    );
    let mut wrong = Vec::new();
    for relation in ["cited", "filed"] {
        for (who, ty, key, walk, want) in [
            (&ann, "paper", "p2", Walk::Out, ""),
            (&ann, "paper", "p1", Walk::Out, "p1>s1"),
            (&ann, "source", "s1", Walk::In, "p1>s1"),
            (&l.own, "source", "s1", Walk::In, "p1>s1 p2>s1"),
        ] {
            let p = pattern(ty, Some(key), vec![step(relation, walk)], Projection::Graph);
            let what = format!("{}'s walk over `{relation}` from {key}", who.id);
            let QueryResult::Graph { edges, .. } = l.cat.query(who, p).await.must(&what)?.data
            else {
                return Err(Failure::new(format!("{what} answered no graph")));
            };
            let mut got: Vec<String> = (edges.iter())
                .map(|e| format!("{}>{}", e.from.1, e.to.1))
                .collect();
            got.sort();
            if got.join(" ") != want {
                wrong.push(format!("{what}: `{}`, not `{want}`", got.join(" ")));
            }
        }
    }
    ensure!(
        wrong.is_empty(),
        "a secret paper's edges must not be walked by ann: {wrong:?}"
    );
    Ok(())
}

async fn a_proposal_over_a_field_hidden_where_a_condition_holds_is_decided_as_told(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    chunks(&l).await?;
    let bob = holder(store.as_ref(), "bob", json!({"default": "write"}));
    let mut ids = Vec::new();
    for (key, patch) in [
        ("k2", json!({"doc": "secret", "body": "x"})),
        ("k1", json!({"body": "y"})),
    ] {
        let write = json!({"write": "upsert", "type": "chunk", "key": key, "patch": patch});
        match cat.propose(&bob, de(write), "mine".into()).await {
            Ok(Outcome::Proposed { proposal, .. }) => ids.push(proposal),
            other => return Err(Failure::new(format!("bob's proposal on {key}: {other:?}"))),
        }
    }
    let hide = json!([{"field": "doc", "values": ["secret"], "fields": {"body": "none"}}]);
    let chunk = json!({"default": "write", "hide_where": hide});
    let grants = json!({"default": "read", "review": true, "types": {"chunk": chunk}});
    let ann = holder(store.as_ref(), "ann", grants);
    let listed = cat.proposals(&ann, Default::default()).await;
    let listed = listed.must("ann's listing")?.data;
    let patch = |id: &ProposalId| {
        let p = listed.iter().find(|p| &p.id == id);
        match p.and_then(|p| p.write.as_ref()) {
            Some(ProposedWrite::Upsert { patch, .. }) => {
                let fields: Vec<&str> = patch.keys().map(|f| f.as_str()).collect();
                fields.join(" ")
            }
            other => format!("{other:?}"),
        }
    };
    let said = [patch(&ids[0]), patch(&ids[1])];
    ensure!(
        said == ["doc", "body"],
        "ann must be listed k2's patch without `body` and k1's with it: {said:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let refused = cat.accept(&ann, &ids[0]).await;
    ensure!(
        matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "ann's accept of a write of k2's `body` must be refused not granted: {refused:?}"
    );
    head_unmoved(store.as_ref(), head, "a refused accept").await?;
    cat.accept(&ann, &ids[1])
        .await
        .must("ann's accept of a write of k1's `body`")?;
    Ok(())
}

async fn a_change_to_records_over_a_field_hidden_where_a_condition_holds_tells_nothing_of_it(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let (c, s) = (l.cat.as_ref(), store.as_ref());
    chunks(&l).await?;
    let ann = ann(s, "write", json!({}));
    let change = |filter: &str, set: Value| Change::Records {
        ty: "chunk".into(),
        filter: Box::new(filter.into()),
        to: ChangeTo::Set(de(set)),
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let plan = async |who: &Principal, change: &Change| {
        let p = c.plan(who, change.clone()).await;
        p.must(&format!("{}'s plan of {change:?}", who.id))
            .map(|p| p.data)
    };
    // `n` is masked on k2: its value selects nothing and moves no id.
    let over = change("n > 0", json!({"doc": "open"}));
    let (mine, before) = (plan(&l.own, &over).await?, plan(&ann, &over).await?);
    l.upsert(
        "chunk",
        "k2",
        json!({"doc": "secret", "body": "taxi paris", "n": 9}),
    )
    .await?;
    let after = plan(&ann, &over).await?;
    let said = [&mine, &before, &after].map(super::change::line);
    ensure!(
        said[1..] == ["2 [k1,k3] doc 0", "2 [k1,k3] doc 0"] && said[0].starts_with("3 "),
        "a condition over k2's masked `n` must not select it for ann: {said:?}"
    );
    ensure!(
        before.report_id == after.report_id,
        "k2's `n` must not move ann's plan id"
    );
    // A change setting `body` on a secret chunk is refused for it, whole.
    let body = change("n > 0 or doc = 'secret'", json!({"body": "x"}));
    let planned = plan(&ann, &body).await?;
    let told = super::change::line(&planned);
    ensure!(
        told.ends_with("refused NotGranted 1 [k2]"),
        "a change of k2's `body` must be told refused not granted for k2: {told}"
    );
    let head = s.head().await.must("reading the head")?;
    let enacted = c.enact(&ann, body, &planned.report_id).await;
    ensure!(
        enacted.is_err(),
        "the change must be refused whole: {enacted:?}"
    );
    head_unmoved(s, head, "a refused change").await
}
