//! A record the host refused, quarantined
//! (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`,
//! extended): the host's door keeps it as an ingest keeps one the lake
//! refused, with no ingest of it, and a caller's `(run, line)` keys
//! either kind, so a re-send is the same quarantined record.

use lakelands_api::IngestOpts;
use lakelands_api::error::Case;
use lakelands_api::quarantine::{
    ClosedWhy, HostRefused, QuarantineFilter, QuarantineId, Quarantined, Resubmits,
};

use super::only::holder;
use super::quarantine::{
    closed_as, contact, fed, indexes, inv, invoice, kept, kind, listed, of, qid, row_of, told,
};
use crate::conformance::fixture::{committed, get_at, source_record};
use crate::conformance::prelude::*;

check!(
    QUARANTINED_BY_HOST = a_record_the_host_refused_is_quarantined,
    "085, extended: `quarantine` keeps a record the host refused, though its cells would land, under `(erp, run-1, 4)` with the host's cause and reason as `refused_by_host`, and lands nothing; without a pair under the store's delivery at index 0; a type not declared and a cause not of its form are refused, appending nothing"
);

check!(
    QUARANTINED_BY_RUN_AND_LINE = a_run_and_line_key_either_kind,
    "085, extended: a record the lake refused under run `run-1` at line 11 is quarantined as `(erp, run-1, 11)`; the call sent again appends nothing; refused otherwise it is version 2; the host's refusal at the same pair is version 3 of the same record, and sent again is a duplicate"
);

check!(
    QUARANTINED_ACROSS_CALLS = one_delivery_in_two_calls_keys_by_line,
    "085, extended: one delivery sent in two calls of two records, each naming its line and the second of each refused, lists two quarantined records, `(erp, file, 1)` and `(erp, file, 3)`, each at version 1; either call sent again appends nothing"
);

check!(
    QUARANTINED_BY_HOST_CLOSES = a_record_the_host_refused_lives_as_one_the_lake_refused,
    "085, extended: of three records the host refused, one dismissed closes `dismissed` and sent again is a duplicate, one whose key a re-run lands closes `landed`, and one a resubmission names closes `resubmitted`"
);

check!(
    QUARANTINED_BY_HOST_PROTECTED = a_record_the_host_refused_keeps_its_cells_protected,
    "085, extended: an erasable cell of a record the host refused is sealed in the log and the export, and read back by the owner; a sensitive cell is masked to a reader without its field's right and shown with it; neither a refusal of the door nor the request's debug form holds a cell's value",
    given
);

check!(
    QUARANTINED_BY_HOST_GRANTED = only_a_holder_of_ingest_quarantines,
    "085, extended: a principal without `ingest` is refused `not_granted` by `quarantine`, one with it is answered `not_found` for a type it is not shown, and neither appends"
);

/// The host's refusal of `record`, at `line` where given.
fn refused(record: SourceRecord, line: Option<u64>) -> HostRefused {
    HostRefused {
        record: SourceRecord { line, ..record },
        cause: "mapping".into(),
        reason: "column 3 maps to no field".into(),
    }
}

/// Options keyed by `run`, quarantining.
fn run(run: &str) -> IngestOpts {
    IngestOpts {
        run: Some(run.into()),
        quarantine_refused: true,
        ..IngestOpts::default()
    }
}

/// `refused` quarantined by `who` from `erp` under `opts`.
async fn host(
    l: &Lake,
    who: &Principal,
    opts: IngestOpts,
    refused: HostRefused,
) -> Result<lakelands_api::quarantine::Quarantining, Error> {
    l.cat.quarantine(who, &"erp".into(), opts, refused).await
}

/// Whether `q` holds the host's refusal of `refused`.
fn by_host(q: &Quarantined) -> bool {
    matches!(&q.refusal, Error::Refused { reason: Refusal::Declined(said), .. }
    if said.case == Case::RefusedByHost {
        cause: "mapping".into(),
        reason: "column 3 maps to no field".into(),
    })
}

async fn a_record_the_host_refused_is_quarantined(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let got = host(
        &l,
        &l.own,
        run("run-1"),
        refused(inv("h1", json!(5), None), Some(4)),
    )
    .await
    .must("quarantining `h1` the host refused")?;
    ensure!(
        got.id == qid("erp", "run-1", 4) && !got.duplicate,
        "`h1` answered {got:?}"
    );
    let q = row_of(&l, &l.own, got.id).await?;
    ensure!(
        q.as_ref().is_some_and(|q| by_host(q)
            && q.version == 1
            && q.closed.is_none()
            && q.cells.get("amount") == Some(&json!(5))),
        "`h1` lists {q:?}"
    );
    let read = l
        .cat
        .get(&l.own, &"invoice".into(), get_at("h1", None))
        .await;
    ensure!(read.is_err(), "`h1` is not ingested, yet reads {read:?}");
    let plain = host(
        &l,
        &l.own,
        IngestOpts::default(),
        refused(inv("h2", json!(6), None), None),
    )
    .await
    .must("quarantining `h2` with no pair")?;
    let own = plain.id.delivery.as_str();
    ensure!(
        own.starts_with('d') && plain.id.index == 0,
        "`h2` with no pair is keyed {}",
        plain.id
    );
    let head = store.head().await.must("reading the head")?;
    let nobody = source_record("nobody", "n1", json!({"a": 1}));
    let none = host(&l, &l.own, run("run-1"), refused(nobody, Some(5))).await;
    let mut badly = refused(inv("h3", json!(7), None), Some(6));
    badly.cause = "Bad Cause".into();
    let form = host(&l, &l.own, run("run-1"), badly).await;
    let said = |e: &Error| e.case().map(|s| s.case.clone());
    ensure!(
        none.as_ref()
            .is_err_and(|e| matches!(e, Error::Ingested { error, .. }
            if matches!(**error, Error::NotFound { .. })))
            && form
                .as_ref()
                .is_err_and(|e| matches!(said(e), Some(Case::HostRefusalForm { .. }))),
        "a type not declared answered {none:?}, a cause not of its form {form:?}"
    );
    let after = events_after(store.as_ref(), head).await?;
    ensure!(after.is_empty(), "the refused calls appended {after:?}");
    Ok(())
}

/// The version of `id` and whether the host refused it.
async fn version(l: &Lake, id: &QuarantineId) -> Result<Option<(u32, bool)>, Failure> {
    Ok(row_of(l, &l.own, id.clone())
        .await?
        .map(|q| (q.version, by_host(&q))))
}

/// The id, key and version of each invoice `erp` quarantined.
async fn ids(l: &Lake) -> Result<Vec<(QuarantineId, String, u32)>, Failure> {
    let rows = of(l, "erp", "invoice").await?;
    Ok(rows
        .into_iter()
        .map(|q| (q.id, q.key.0, q.version))
        .collect())
}

async fn a_run_and_line_key_either_kind(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let two = |extra: &str| {
        vec![
            SourceRecord {
                line: Some(10),
                ..inv("a0", json!(1), None)
            },
            SourceRecord {
                line: Some(11),
                ..inv("a1", json!(2), Some(extra))
            },
        ]
    };
    let id = qid("erp", "run-1", 11);
    let first = fed(&l, &l.own, "erp", run("run-1"), two("nope"))
        .await
        .must("run-1")?;
    ensure!(indexes(&first) == [1], "run-1 answered {first:?}");
    ensure!(
        version(&l, &id).await? == Some((1, false)),
        "`a1` is not `{id}` at version 1"
    );
    let again = fed(&l, &l.own, "erp", run("run-1"), two("nope"))
        .await
        .must("run-1 again")?;
    ensure!(
        committed(&again) == (0, 2) && indexes(&again).is_empty(),
        "run-1 sent again answered {again:?}"
    );
    let other = fed(&l, &l.own, "erp", run("run-1"), two("zz"))
        .await
        .must("run-1 refused otherwise")?;
    ensure!(
        indexes(&other) == [1],
        "run-1 refused otherwise answered {other:?}"
    );
    ensure!(
        version(&l, &id).await? == Some((2, false)),
        "`{id}` is not at version 2"
    );
    let by = host(
        &l,
        &l.own,
        run("run-1"),
        refused(inv("a1", json!(2), None), Some(11)),
    )
    .await
    .must("the host refusing `a1` at line 11")?;
    ensure!(
        by.id == id && !by.duplicate,
        "the host's refusal answered {by:?}"
    );
    ensure!(
        version(&l, &id).await? == Some((3, true)),
        "`{id}` is not at version 3 by the host"
    );
    let dup = host(
        &l,
        &l.own,
        run("run-1"),
        refused(inv("a1", json!(2), None), Some(11)),
    )
    .await
    .must("the host refusing `a1` again")?;
    ensure!(
        dup.id == id && dup.duplicate,
        "the host's refusal again answered {dup:?}"
    );
    let rows = of(&l, "erp", "invoice").await?;
    ensure!(rows.len() == 1, "one record is quarantined, yet {rows:?}");
    Ok(())
}

async fn one_delivery_in_two_calls_keys_by_line(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let call = |n: u64| {
        let at = |line: u64, extra| SourceRecord {
            line: Some(line),
            ..inv(&format!("c{line}"), json!(line), extra)
        };
        vec![at(2 * n, None), at(2 * n + 1, Some("nope"))]
    };
    for (n, what) in [(0, "the first call"), (1, "the second call")] {
        let got = fed(&l, &l.own, "erp", kept("file"), call(n))
            .await
            .must(what)?;
        ensure!(indexes(&got) == [1], "{what} answered {got:?}");
    }
    let want = vec![
        (qid("erp", "file", 1), "c1".to_owned(), 1),
        (qid("erp", "file", 3), "c3".to_owned(), 1),
    ];
    let rows = ids(&l).await?;
    ensure!(rows == want, "the two calls list {rows:?}");
    let head = store.head().await.must("reading the head")?;
    for (n, what) in [(1, "the second call again"), (0, "the first call again")] {
        let got = fed(&l, &l.own, "erp", kept("file"), call(n))
            .await
            .must(what)?;
        ensure!(committed(&got) == (0, 2), "{what} answered {got:?}");
    }
    let after = events_after(store.as_ref(), head).await?;
    let rows = ids(&l).await?;
    ensure!(
        after.is_empty() && rows == want,
        "sent again, the calls appended {after:?} and list {rows:?}"
    );
    Ok(())
}

async fn a_record_the_host_refused_lives_as_one_the_lake_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    for (key, line) in [("k1", 1), ("k2", 2), ("k3", 3)] {
        host(
            &l,
            &l.own,
            run("r"),
            refused(inv(key, json!(line), None), Some(line)),
        )
        .await
        .must(&format!("quarantining `{key}`"))?;
    }
    let (k1, k2, k3) = (qid("erp", "r", 1), qid("erp", "r", 2), qid("erp", "r", 3));
    l.cat
        .dismiss_quarantined(&l.own, &k1)
        .await
        .must("dismissing `k1`")?;
    let dup = host(
        &l,
        &l.own,
        run("r"),
        refused(inv("k1", json!(1), None), Some(1)),
    )
    .await
    .must("the host refusing `k1` again")?;
    let rerun = vec![inv("k2", json!(2), None)];
    fed(&l, &l.own, "erp", IngestOpts::default(), rerun)
        .await
        .must("a re-run landing `k2`")?;
    let resubmits = Some(Resubmits {
        delivery: "r".into(),
        index: 3,
    });
    let mended = vec![SourceRecord {
        resubmits,
        ..inv("k3", json!(3), None)
    }];
    fed(&l, &l.own, "erp", IngestOpts::default(), mended)
        .await
        .must("resubmitting `k3`")?;
    let closes = (
        closed_as(&l, k1).await?,
        closed_as(&l, k2).await?,
        closed_as(&l, k3).await?,
    );
    ensure!(
        dup.duplicate
            && closes
                == (
                    Some(ClosedWhy::Dismissed),
                    Some(ClosedWhy::Landed),
                    Some(ClosedWhy::Resubmitted)
                ),
        "`k1` again answered {dup:?}; the three closed {closes:?}"
    );
    Ok(())
}

async fn a_record_the_host_refused_keeps_its_cells_protected(store: Store, given: Given) -> Held {
    use super::erasable::person;
    let (_keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    l.declare(contact()).await?;
    let head = store.head().await.must("reading the head")?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    let request = refused(ada, Some(1));
    ensure!(
        !format!("{request:?}").contains("Ada"),
        "the request debugs as {request:?}"
    );
    let apart = IngestOpts {
        sealed_apart: true,
        ..run("r")
    };
    host(&l, &l.own, apart, request)
        .await
        .must("quarantining `p1`")?;
    let events = events_after(store.as_ref(), head).await?;
    let sealed = events.iter().any(|e| {
        matches!(
            &e.body,
            EventBody::RecordQuarantined {
                sealed: Some(_),
                ..
            }
        )
    });
    let plain = events.iter().any(|e| json!(e).to_string().contains("Ada"));
    ensure!(
        sealed && !plain,
        "`p1`'s name must be sealed in the log: {events:?}"
    );
    let dir = given.scratch.join("host-sealed");
    let _ = std::fs::remove_dir_all(&dir);
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting")?;
    let files = std::fs::read_dir(&dir).map_err(|e| Failure::new(format!("the export: {e}")))?;
    for path in files.flatten().map(|e| e.path()) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let held = bytes.windows(3).any(|w| w == b"Ada");
        ensure!(!held, "the export's `{}` holds `p1`'s name", path.display());
    }
    let p1 = row_of(&l, &l.own, qid("erp", "r", 1)).await?;
    ensure!(
        p1.as_ref()
            .is_some_and(|q| q.cells.get("name") == Some(&json!("Ada"))),
        "the owner must read `p1`'s name back for a re-run; it lists {p1:?}"
    );
    let cells = json!({"name": "Bo", "email": "b@x", "region": "eu"});
    let c1 = refused(source_record("contact", "c1", cells), Some(2));
    host(&l, &l.own, run("r"), c1)
        .await
        .must("quarantining `c1`")?;
    let reader = |id: &str, fields: Value| {
        let grants = json!({"default": "read", "ingest": true,
            "types": {"contact": {"default": "none", "fields": fields}}});
        holder(store.as_ref(), id, grants)
    };
    let c1 = qid("erp", "r", 2);
    let masked = row_of(
        &l,
        &reader("masks", json!({"name": "read", "region": "read"})),
        c1.clone(),
    )
    .await?
    .map(|q| told(&q));
    let shown = row_of(
        &l,
        &reader(
            "shows",
            json!({"name": "read", "region": "read", "email": "read"}),
        ),
        c1,
    )
    .await?
    .map(|q| told(&q));
    ensure!(
        masked.as_deref() == Some("name region masked=email")
            && shown.as_deref() == Some("email name region masked="),
        "`c1`'s email is listed {masked:?} without the right, {shown:?} with it"
    );
    let nobody = source_record("nobody", "n1", json!({"name": "Ada"}));
    let refusal = host(&l, &l.own, run("r"), refused(nobody, Some(3))).await;
    let said = format!("{refusal:?} {}", json!(refusal.as_ref().err()));
    ensure!(
        refusal.is_err() && !said.contains("Ada"),
        "a refusal of the door holds a cell's value: {said}"
    );
    Ok(())
}

async fn only_a_holder_of_ingest_quarantines(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let head = store.head().await.must("reading the head")?;
    let reader = holder(store.as_ref(), "reader", json!({"default": "read"}));
    let blind = holder(
        store.as_ref(),
        "blind",
        json!({"default": "read", "ingest": true, "types": {"invoice": {"default": "none"}}}),
    );
    let none = host(
        &l,
        &reader,
        run("r"),
        refused(inv("x1", json!(1), None), Some(1)),
    )
    .await;
    let hidden = host(
        &l,
        &blind,
        run("r"),
        refused(inv("x2", json!(1), None), Some(2)),
    )
    .await;
    let not_found = |e: &Error| match e {
        Error::Ingested { error, .. } => matches!(**error, Error::NotFound { .. }),
        e => matches!(e, Error::NotFound { .. }),
    };
    ensure!(
        none.as_ref().is_err_and(|e| kind(e) == "not_granted")
            && hidden.as_ref().is_err_and(not_found),
        "without `ingest` the door answered {none:?}; for a type not shown {hidden:?}"
    );
    let listing = listed(&l, &reader, QuarantineFilter::default()).await;
    ensure!(
        listing.is_err(),
        "without `ingest` the listing answered {listing:?}"
    );
    let after = events_after(store.as_ref(), head).await?;
    ensure!(after.is_empty(), "the refused calls appended {after:?}");
    Ok(())
}
