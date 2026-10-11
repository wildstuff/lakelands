//! A record that is judged is judged with its provenance, wherever it
//! stands (`docs/decisions/010-provenance-per-field.md`): an
//! object set's filter, a derived field, a verb's `requires` and
//! assignments and an aging rule's guard read `seen_at` and `known_at` as
//! a read's `where` does.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::LakeStore;
use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Boolean, DateTime, Number, Text};
use lakelands_api::ontology::{FieldDecl, OnInputChange, TypeDecl};
use lakelands_api::seam::AGING_RULE_TYPE;
use lakelands_api::write::{Declaration, ProposedWrite, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::backing::KIND;
use crate::conformance::fixture::{
    Lake, Must, computed_by, derived, field, find_where, invalid_case, log_events, object_set_on,
    source_record, sourced, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    NO_RECORD_NO_PROVENANCE = provenance_is_refused_where_no_record_stands,
    "028, extended (lakelands#852): where no record stands there is no provenance to read: a value type's constraint, a function's expression, and a view's `where` and measures over a source that is a view, its own or a merged one, are refused as `no_records_provenance` when they call `seen_at` or `known_at`, by `declare`, `propose` and `apply` alike; the same view over a stored type is declared; a read's `where` over a view's records is refused alike: a `find`, a pattern's start and a step that reaches the view"
);

check!(
    JUDGED_WITH_PROVENANCE = a_record_is_judged_with_its_provenance_wherever_it_stands,
    "010 and 004, extended (lakelands#852): an object set's filter, by the membership the log holds, at a write and where the set is declared over records, a derived field, a verb's `requires`, its assignments, an aging rule's guard and a `find`'s group key read `seen_at(f)` of an ingested field and `known_at(f)` of an authored one as a `find` with the same `where` does, and so after a rebuild and for a derived field declared over the records; a view over the type holds the group its `where` finds and the instant its measure aggregates, computed, kept, after its member is written again and after a rebuild; a derived field that reads the provenance of a field the fold changes in the same event (a derived field, declared before or after its reader or two steps from the written one, a cleared field, a backed one) reads that event's instant, directly after the write, after a declaration computes it again and after a rebuild; a type that keeps no per-field provenance reads none, at a write as at a read"
);

/// The two expressions, each true of `j1` alone.
const SEEN: &str = "seen_at(status) >= '2026-03-01'";
const KNOWN: &str = "known_at(note) >= '2020-01-01'";

/// The fields that read the provenance of a field the fold itself
/// changes, each with the filter true of a job where it holds that
/// field's instant: a derived field declared after its reader, one two
/// steps from the written field, a cleared field and, last, a backed
/// one.
const EDGES: [&str; 4] = [
    "since = known_at(twice)",
    "since_four = known_at(four)",
    "label_at = known_at(label)",
    "fee_at = known_at(fee)",
];

/// `job` with a source's `status`, and for each of the two expressions a
/// derived field (`fresh`, `noted`) and the fields its verbs and its
/// aging rule write; the readers of [`EDGES`], each declared before the
/// field whose provenance it reads; `more` are further fields.
fn job(more: Vec<FieldDecl>) -> TypeDecl {
    let label = FieldDecl {
        depends_on: vec!["n".into()],
        on_input_change: OnInputChange::Clear,
        ..field("label", Text)
    };
    let mut fields = vec![
        sourced("status", Text),
        field("note", Text),
        field("n", Number),
        field("m", Number),
        derived("fresh", Boolean, SEEN, &["status"]),
        derived("noted", Boolean, KNOWN, &["note"]),
        derived("since_four", DateTime, "known_at(four)", &["four"]),
        derived("since", DateTime, "known_at(twice)", &["twice"]),
        derived("label_at", DateTime, "known_at(label)", &["label"]),
        derived("fee_at", DateTime, "known_at(fee)", &["fee"]),
        derived("twice", Number, "n * 2", &["n"]),
        derived("four", Number, "twice * 2", &["twice"]),
        label,
        computed_by("fee", Number, "fee", &["m"]),
    ];
    for name in ["seen_stage", "known_stage", "seen_met", "known_met"] {
        fields.push(field(name, Text));
    }
    for name in ["seen_stamp", "known_stamp"] {
        fields.push(field(name, DateTime));
    }
    fields.extend(more);
    type_decl("job", fields)
}

/// The keys of the jobs `filter` finds.
async fn found(l: &Lake, filter: &str) -> Result<BTreeSet<String>, Failure> {
    found_of(l, "job", filter).await
}

async fn found_of(l: &Lake, ty: &str, filter: &str) -> Result<BTreeSet<String>, Failure> {
    let found = l.all_found(ty, find_where(filter)).await?;
    Ok(found.into_iter().map(|r| r.key.to_string()).collect())
}

/// Calls `verb` on the job `key`. It is refused where its `requires` does
/// not hold: the rows say which.
async fn call(l: &Lake, verb: &str, key: &str) {
    let (verb, key, opts) = (verb.into(), Some(key.into()), WriteOpts::default());
    let _ = l
        .cat
        .call(&l.own, &verb, key.as_ref(), Default::default(), opts)
        .await;
}

/// The lake: for each expression a set, a verb that requires it, a verb
/// that assigns what it reads and an aging rule over `job`; then `j0`,
/// seen in February and without a note, and `j1`, seen in March and
/// noted; each verb called on both and the rules run.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare_json(
        json!({"declare": "function", "name": "fee", "backing": KIND,
                          "inputs": [{"name": "m", "type": {"kind": "number"}}],
                          "config": {"token": "judged", "echo": "m"}}),
    )
    .await?;
    l.declare(job(vec![])).await?;
    for (name, expr, reads) in [
        ("seen", SEEN, "seen_at(status)"),
        ("known", KNOWN, "known_at(note)"),
    ] {
        l.declare_as(object_set_on(name, "job", expr), "a set")
            .await?;
        let met = json!([{"field": format!("{name}_met"), "value": "'yes'"}]);
        let stamp = json!([{"field": format!("{name}_stamp"), "value": reads}]);
        for (verb, requires, set) in [
            (name.to_owned(), Some(expr), met),
            (format!("{name}_stamp"), None, stamp),
        ] {
            l.declare_json(
                json!({"declare": "verb", "name": verb, "on": "job", "risk": "low",
                                  "requires": requires, "effect": "write", "set": set}),
            )
            .await?;
        }
        let rule = json!({"on": "job", "field": format!("{name}_stage"), "from": "new",
                          "to": "old", "when": expr});
        l.upsert(AGING_RULE_TYPE, name, rule).await?;
    }
    for (key, seen) in [("j0", "2026-02-01"), ("j1", "2026-03-02")] {
        let mut r = source_record("job", key, json!({"status": "open"}));
        r.seen_at = Some(format!("{seen}T00:00:00Z").parse().expect("an instant"));
        l.ingest("feed", vec![r]).await.must("ingesting a job")?;
        let mut authored = json!({"n": 1, "label": "a", "seen_stage": "new", "known_stage": "new"});
        if key == "j1" {
            authored["note"] = json!("x");
        }
        l.upsert("job", key, authored).await?;
        for verb in ["seen", "known", "seen_stamp", "known_stamp"] {
            call(&l, verb, key).await;
        }
    }
    let today = "2026-06-01T00:00:00Z".parse().expect("an instant");
    store.age(today).await.must("aging")?;
    Ok(l)
}

/// What each place answers for the two expressions, as the keys it holds
/// for, beside what `find` answers with the expression as its `where`:
/// one line for each that differs. A set's members are read from the log,
/// each record's latest `object_set_changed`: a read of the set would
/// judge its filter again.
async fn apart(
    (store, l): (&dyn LakeStore, &Lake),
    sets: [&str; 2],
    fields: [&str; 2],
) -> Result<Vec<String>, Failure> {
    let mut last = BTreeMap::new();
    for e in log_events(store).await? {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = e.body
        {
            last.insert((set.to_string(), key.to_string()), joined);
        }
    }
    let mut apart = Vec::new();
    for (i, (expr, name, reads)) in [
        (SEEN, "seen", "seen_at(status)"),
        (KNOWN, "known", "known_at(note)"),
    ]
    .into_iter()
    .enumerate()
    {
        let read = found(l, expr).await?;
        let members = last
            .iter()
            .filter(|((set, _), joined)| set == sets[i] && **joined);
        let members: BTreeSet<String> = members.map(|((_, key), _)| key.clone()).collect();
        if members != read {
            apart.push(format!(
                "`{expr}`: `find` answers {read:?}, the set holds {members:?}"
            ));
        }
        // The assignment has no `requires`: it holds for every job that
        // has the instant.
        let has = format!("{reads} >= '2000-01-01'");
        for (place, under, filter) in [
            ("the derived field", expr, format!("{} = true", fields[i])),
            ("`requires`", expr, format!("{name}_met = 'yes'")),
            ("the aging guard", expr, format!("{name}_stage = 'old'")),
            (
                "the assignment",
                has.as_str(),
                format!("{name}_stamp = {reads}"),
            ),
        ] {
            let (wanted, judged) = (found(l, under).await?, found(l, &filter).await?);
            if judged != wanted {
                apart.push(format!(
                    "`{under}`: `find` answers {wanted:?}, {place} {judged:?}"
                ));
            }
        }
        // A `find`'s group key is an expression over the record as well.
        let by = json!({"aggregate": {"measures": [{"name": "c", "func": "count"}],
                                      "group_by": [{"name": "k", "expr": expr}]}});
        let by: lakelands_api::Find = serde_json::from_value(by).expect("a find reads");
        let grouped = l.cat.find(&l.own, &"job".into(), by).await;
        let held = match grouped.must("grouping the jobs")?.data {
            lakelands_api::Found::Groups { rows } => (rows.into_iter())
                .find(|g| g.by.get("k") == Some(&json!(true)))
                .and_then(|g| g.values.get("c").cloned()),
            _ => None,
        };
        if held != Some(json!(read.len().to_string())) {
            apart.push(format!(
                "`{expr}`: `find` answers {read:?}, as a group key it holds for {held:?}"
            ));
        }
    }
    Ok(apart)
}

/// One line for each of `edges` that does not hold for both jobs.
async fn unsettled(l: &Lake, when: &str, edges: &[&str]) -> Result<Vec<String>, Failure> {
    let mut lines = Vec::new();
    for edge in edges {
        let holds = found(l, edge).await?;
        if holds.len() != 2 {
            lines.push(format!(
                "{when}, `{edge}` must hold for both jobs, and does for {holds:?}"
            ));
        }
    }
    Ok(lines)
}

/// What the readers of [`EDGES`] hold, job by job.
async fn readers(l: &Lake) -> Result<Vec<Vec<Option<Value>>>, Failure> {
    let jobs = l.all_found("job", Default::default()).await?;
    let names = ["since", "since_four", "label_at", "fee_at"];
    let of = |r: &lakelands_api::record::Record| names.map(|n| r.fields.get(n).cloned()).to_vec();
    Ok(jobs.iter().map(of).collect())
}

async fn a_record_is_judged_with_its_provenance_wherever_it_stands(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let at = (store.as_ref(), &l);
    let j1: BTreeSet<String> = ["j1".to_owned()].into();
    let read = (found(&l, SEEN).await?, found(&l, KNOWN).await?);
    ensure!(
        read == (j1.clone(), j1.clone()),
        "`find` must answer `j1` alone under each expression; it answers {read:?}"
    );
    let mut wrong = apart(at, ["seen", "known"], ["fresh", "noted"]).await?;
    // A field the fold changes takes the event's provenance, and a field
    // computed after it reads that: asked directly after each write,
    // before a later event computes the reader again.
    // The backed field's input is written last: a backed field that
    // settles brings a further round, which would compute the others again.
    wrong.extend(unsettled(&l, "after the first writes", &EDGES[..3]).await?);
    for (when, key, patch, asked) in [
        ("after `n` is written again", "j0", json!({"n": 5}), 3),
        (
            "after another field is written",
            "j0",
            json!({"known_stage": "later"}),
            3,
        ),
        ("after a backing is called", "j0", json!({"m": 7}), 3),
        ("after a held output is read", "j1", json!({"m": 7}), 4),
    ] {
        l.upsert("job", key, patch).await?;
        wrong.extend(unsettled(&l, when, &EDGES[..asked]).await?);
    }
    // A set and a derived field declared over the records that stand.
    for (name, expr) in [("seen_late", SEEN), ("known_late", KNOWN)] {
        l.declare_as(object_set_on(name, "job", expr), "a late set")
            .await?;
    }
    let late = vec![
        derived("fresh_late", Boolean, SEEN, &["status"]),
        derived("noted_late", Boolean, KNOWN, &["note"]),
    ];
    l.declare(job(late)).await?;
    let over = ["seen_late", "known_late"];
    wrong.extend(apart(at, over, ["fresh_late", "noted_late"]).await?);
    wrong.extend(unsettled(&l, "computed again by a declaration", &EDGES).await?);
    // A view over the type: its `where` and what a measure aggregates,
    // computed, kept and after a rebuild.
    for (name, filter, measure) in [
        ("by_seen", SEEN, "seen_at(status)"),
        ("by_known", KNOWN, "known_at(note)"),
    ] {
        let view = json!({"source": "job", "where": filter, "group_by": ["status"],
                          "measures": [{"name": "c", "expr": "count()"},
                                       {"name": "at", "expr": format!("max({measure})")}]});
        let fields = json!([{"name": "status", "type": {"kind": "text"}},
                            {"name": "c", "type": {"kind": "number"}},
                            {"name": "at", "type": {"kind": "date_time"}}]);
        l.declare_json(json!({"declare": "type", "name": name, "key": ["status"],
                              "fields": fields, "view": view}))
            .await?;
    }
    let mut views = Vec::new();
    for state in ["computed", "kept", "written again", "rebuilt"] {
        match state {
            "kept" => {
                l.set("view_kept.by_seen", json!(true)).await?;
                l.set("view_kept.by_known", json!(true)).await?;
            }
            // The kept instant follows its member's provenance.
            "written again" => {
                l.upsert("job", "j1", json!({"note": "y"})).await?;
                call(&l, "known_stamp", "j1").await;
            }
            "rebuilt" => {
                let held = readers(&l).await?;
                store.rebuild().await.must("rebuilding the projection")?;
                if readers(&l).await? != held {
                    wrong.push("a rebuild computes other instants for the readers".into());
                }
            }
            _ => {}
        }
        for (view, measure) in [
            ("by_seen", "seen_at(status)"),
            ("by_known", "known_at(note)"),
        ] {
            let rows = l.all_found(view, Default::default()).await?;
            let at = rows.first().and_then(|r| r.fields.get("at")?.as_str());
            let holders = match at {
                Some(at) => found(&l, &format!("{measure} = '{at}'")).await?,
                None => BTreeSet::new(),
            };
            let c = rows.first().and_then(|r| r.fields.get("c").cloned());
            if (rows.len(), c, &holders) != (1, Some(json!(1)), &j1) {
                wrong.push(format!("the view `{view}`, {state}, must hold one group of `j1` alone with the instant `{measure}` reads of it; it holds {rows:?}"));
            }
        }
        views.push(apart(at, over, ["fresh_late", "noted_late"]).await?);
        wrong.extend(unsettled(&l, state, &EDGES).await?);
    }
    wrong.extend(views.concat().into_iter().map(|w| format!("later, {w}")));
    // A type that keeps no per-field provenance reads none, at a write as
    // at a read.
    let mut bare = type_decl(
        "bare",
        vec![
            sourced("status", Text),
            derived("fresh", Boolean, SEEN, &["status"]),
        ],
    );
    bare.field_meta = false;
    l.declare(bare).await?;
    l.declare_as(object_set_on("bare_seen", "bare", SEEN), "a set")
        .await?;
    let mut r = source_record("bare", "b1", json!({"status": "open"}));
    r.seen_at = Some("2026-03-02T00:00:00Z".parse().expect("an instant"));
    l.ingest("feed", vec![r])
        .await
        .must("ingesting a bare record")?;
    for filter in [SEEN, "in_set('bare_seen')", "fresh = true"] {
        let read = found_of(&l, "bare", filter).await?;
        if !read.is_empty() {
            wrong.push(format!(
                "a type without per-field provenance reads none: `{filter}` finds {read:?}"
            ));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a record is judged with its provenance wherever it stands; apart:\n{}",
        wrong.join("\n")
    );
    Ok(())
}

async fn provenance_is_refused_where_no_record_stands(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "job",
        vec![sourced("status", Text), field("n", Number)],
    ))
    .await?;
    let view = |name: &str, source: &str, filter: Option<&str>, measure: &str, merge: Value| {
        let mut fields = json!([{"name": "status", "type": {"kind": "text"}},
                                {"name": "c", "type": {"kind": "date_time"}}]);
        if merge != json!([]) {
            let m = json!({"name": "m", "type": {"kind": "date_time"}});
            fields.as_array_mut().expect("a list").push(m);
        }
        json!({"declare": "type", "name": name, "key": ["status"], "fields": fields,
               "view": {"source": source, "where": filter, "group_by": ["status"],
                        "measures": [{"name": "c", "expr": measure}], "merge": merge}})
    };
    let seen = "max(seen_at(status))";
    // Over the stored type both are read, so the view is declared.
    l.declare_json(view("stored", "job", Some(SEEN), seen, json!([])))
        .await?;
    let merged = |filter: Option<&str>, measure: &str| {
        json!([{"source": "stored", "where": filter, "group_by": ["status"],
                "measures": [{"name": "m", "expr": measure}]}])
    };
    let cases = [
        (
            "a value type's constraint",
            json!({"declare": "value_type", "name": "late", "kind": "text",
                                             "constraint": "known_at(value) >= '2020-01-01'"}),
        ),
        (
            "a function's expression",
            json!({"declare": "function", "name": "is_fresh", "backing": "expr",
                                           "inputs": [{"name": "status", "type": {"kind": "text"}}],
                                           "expr": SEEN}),
        ),
        (
            "a view's `where` over a view",
            view("over", "stored", Some(SEEN), "max(c)", json!([])),
        ),
        (
            "a view's measure over a view",
            view("over", "stored", None, seen, json!([])),
        ),
        (
            "a merged view's `where`",
            view("over", "job", None, seen, merged(Some(SEEN), "max(c)")),
        ),
        (
            "a merged view's measure",
            view("over", "job", None, seen, merged(None, seen)),
        ),
    ];
    let mut wrong = Vec::new();
    for (what, json) in cases {
        let d: Declaration = serde_json::from_value(json).expect("a declaration reads");
        let declared = l.cat.declare(&l.own, d.clone()).await;
        let declarations = vec![d.clone()];
        let proposed = l.cat.propose(
            &l.own,
            ProposedWrite::Declare { declarations },
            "why".into(),
        );
        let (proposed, applied) = (proposed.await, l.cat.apply(&l.own, vec![d]).await);
        let refused = matches!(invalid_case(&declared), Some(Case::NoRecordsProvenance));
        if !refused || declared != proposed || declared != applied {
            wrong.push(format!("{what}: {declared:?}, {proposed:?}, {applied:?}"));
        }
    }
    // A read's `where` over a view's records: a `find`, a pattern's start
    // and a step that reaches the view over a derived relation.
    l.declare_json(json!({"declare": "type", "name": "tag",
        "fields": [{"name": "status", "type": {"kind": "text"}}],
        "relations": [{"name": "tally", "target": "stored", "cardinality": "many_to_one",
                       "derived": {"match": [{"from": "status", "to": "status"}]}}]}))
        .await?;
    let found = l.cat.find(&l.own, &"stored".into(), find_where(SEEN)).await;
    let pattern = |p: Value| serde_json::from_value(p).expect("a pattern reads");
    let start = pattern(json!({"start": {"type": "stored", "where": SEEN}}));
    let step = json!({"start": {"type": "tag"}, "steps": [{"relation": "tally", "where": SEEN}]});
    let (start, step) = (
        l.cat.query(&l.own, start).await,
        l.cat.query(&l.own, pattern(step)).await,
    );
    for (what, case) in [
        ("a `find`", invalid_case(&found)),
        ("a pattern's start", invalid_case(&start)),
        ("a pattern's step", invalid_case(&step)),
    ] {
        if !matches!(case, Some(Case::NoRecordsProvenance)) {
            wrong.push(format!("{what} over a view's records: {case:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "`seen_at` and `known_at` are refused as `no_records_provenance` where no record \
         stands, by `declare`, `propose` and `apply` alike, and by a read; they answered:\n{}",
        wrong.join("\n")
    );
    Ok(())
}
