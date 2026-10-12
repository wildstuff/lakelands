//! A compact gives storage back and changes nothing a caller can observe
//! (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
//!
//! The suite holds the answers, the log and the export; what a compact
//! reclaims is the engine's own measure, and only each engine's tests
//! hold anything about it.
//!
//! **The churned lake.** A compact has most to give back after churn, so
//! every check runs on a lake that made some. Its first part is the
//! re-key fixture (`rekey::lake`): the value type `account_kind`; the
//! types `person`, `account`, `payment` (referring to an account) and
//! `entry` (keyed by its account); account 3 retired, account 4
//! restricted, account 1 linked to its owner. On top of it:
//! - `invoice`, with a searchable `memo` and a derived `gross`, each of
//!   four invoices written three times, one retired, one restricted, one
//!   written with an idempotency key, and one proposal left open;
//! - the view `totals` over `invoice`, per partner;
//! - `place`, whose `name` the suite's backing computes from its `code`,
//!   so the lake holds recorded outputs, one place written twice;
//! - for the checks that bind the suite's embedding backend, `note`,
//!   whose `vec` is embedded from its `title`, so vector search has
//!   something to rank;
//! - a value-type migration tightening `account_kind`;
//! - the re-key of `account` from `number` to `iban` (`rekey::apply`);
//! - and a rebuild, which frees the most.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind, ValueTypeDecl};
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::write::{CompactReport, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Catalog, Find, Found, IdempotencyKey, LakeStore, Outcome, Position, Principal, RecordKey,
    TypeName, WriteOpts,
};
use serde::Serialize;
use serde_json::json;

use super::rekey;
use crate::conformance::Failure;
use crate::conformance::backing::{EMBED_MODEL, made, plug, unplug};
use crate::conformance::fixture::{
    Caller, Lake, Must, embedding, field, get_at, join_all, log_events, partner_totals, patch,
    pattern, read_at, step, type_decl, upsert,
};
use crate::export;

check!(
    /// Every read answers as before.
    ANSWERS_AS_BEFORE = a_compacted_lake_answers_every_read_as_before,
    "040: after a compact of a churned lake (overwrites, a retire, a restrict, a derived field, a view, recorded outputs, embedded vectors, a proposal, a value-type migration, a re-key and a rebuild), every read answers byte for byte as before: every record with provenance at the head and as of every position, a lexical and a vector search, a walk and the proposals; a retry of a keyed write is still recognised and appends nothing; an input the lake recorded is not called again after the compact, and a rebuild after it answers as before",
    embedding
);

check!(
    /// The log and the export are untouched.
    APPENDS_NOTHING = compact_appends_nothing_and_exports_the_same_bytes,
    "040: a compact appends nothing: the head, the whole feed and verify's answer are as before, the report names the head unmoved, and an export before and after is the same, file for file and byte for byte, manifest included",
    scratch
);

check!(
    /// A second compact, then writes as ever.
    AGAIN_AND_ON = a_lake_compacts_again_and_writes_on,
    "040: a lake compacts again and nothing changes either; a write after it continues the record's version and the log's position",
    embedding
);

check!(
    /// Writes during a compact wait, and each lands once.
    WRITES_DURING = writes_made_while_a_compact_runs_each_land_once_before_or_after_it,
    "040 and 008: writers writing while compacts run each land once, before or after a compact; every write answers a position its value is visible from, the log grows by the writes alone, and every read of the churned lake answers as before",
    embedding
);

const INVOICES: usize = 4;

fn invoice_type() -> lakelands_api::ontology::TypeDecl {
    let mut gross = field("gross", StorageKind::Number);
    gross.derived = Some("amount * 2".into());
    type_decl(
        "invoice",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Number),
            FieldDecl {
                searchable: true,
                ..field("memo", StorageKind::Text)
            },
            gross,
        ],
    )
}

/// The write the churned lake made with an idempotency key, and makes
/// again to see it recognised.
async fn keyed(cat: &dyn Catalog, who: &Principal) -> Result<Outcome, Failure> {
    cat.upsert(
        who,
        &TypeName::from("invoice"),
        &RecordKey::from("i9"),
        patch(json!({"partner": "p0", "amount": 9, "memo": "a lake note kept once"})),
        WriteOpts {
            idempotency_key: Some(IdempotencyKey::from("compact-once")),
            ..WriteOpts::default()
        },
    )
    .await
    .must("upserting `invoice` `i9` with an idempotency key")
}

/// `lookup`, a function of the suite's backing echoing its input `code`
/// under the check's own `token`, and `place`, whose `name` it computes.
async fn places(store: &dyn LakeStore, token: &str) -> Result<(), Failure> {
    let l = super::backings::places(store, json!({"token": token, "echo": "code"})).await?;
    for (key, code) in [("q1", "harbour"), ("q2", "quay"), ("q1", "lock")] {
        l.upsert("place", key, json!({"code": code})).await?;
    }
    Ok(())
}

/// `note`, whose `vec` the suite's embedding backend makes from `title`.
async fn notes(l: &Lake) -> Result<(), Failure> {
    let vec = embedding("vec", 2, &["title"], EMBED_MODEL);
    l.declare(type_decl(
        "note",
        vec![field("title", StorageKind::Text), vec],
    ))
    .await?;
    for (key, title) in [
        ("n1", "compact notes"),
        ("n2", "a lake"),
        ("n1", "compacted"),
    ] {
        l.upsert("note", key, json!({"title": title})).await?;
    }
    Ok(())
}

/// The churned lake the module documentation describes; `embedded` when
/// the check binds the suite's embedding backend. `token` is the check's
/// own, for the backing's calls.
async fn churned(store: &dyn LakeStore, embedded: bool, token: &str) -> Result<Lake, Failure> {
    rekey::lake(store, rekey::IBANS).await?;
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    l.declare(invoice_type()).await?;
    l.declare(partner_totals("totals", StorageKind::Number))
        .await?;
    places(store, token).await?;
    if embedded {
        notes(&l).await?;
    }
    for round in 0..3 {
        for i in 0..INVOICES {
            l.upsert(
                "invoice",
                &format!("i{i}"),
                json!({"partner": format!("p{}", i % 2), "amount": round * 10 + i,
                       "memo": format!("lake note {i}, round {round}")}),
            )
            .await?;
        }
    }
    keyed(cat.as_ref(), own).await?;
    l.retire("invoice", "i1").await?;
    store
        .restrict(&"invoice".into(), &"i2".into(), "a retention hold".into())
        .await
        .must("restricting invoice i2")?;
    cat.propose(
        own,
        ProposedWrite::Upsert {
            ty: "invoice".into(),
            key: "i3".into(),
            patch: patch(json!({"amount": 99})),
            opts: WriteOpts::default(),
        },
        "a correction".into(),
    )
    .await
    .must("proposing an upsert of invoice i3")?;
    let tighter: ValueTypeDecl = serde_json::from_value(
        json!({"name": "account_kind", "kind": "text", "one_of": ["giro", "savings"]}),
    )
    .expect("a value type");
    let r = cat
        .migrate_value_type(own, tighter.clone())
        .await
        .must("running the preflight of tightening `account_kind`")?
        .data;
    cat.migrate_value_type_apply(own, tighter, &r.report_id)
        .await
        .must("tightening `account_kind`")?;
    rekey::apply(store).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    crate::conformance::fixture::two_makes(&l).await?;
    Ok(l)
}

const TYPES: [&str; 9] = [
    "person", "account", "payment", "entry", "invoice", "totals", "place", "note", "mark",
];

/// One answer as it serialises, or the refusal it was.
fn said<T: Serialize>(answer: lakelands_api::Result<T>) -> String {
    match answer {
        Ok(a) => serde_json::to_string(&a).expect("an answer serialises"),
        Err(e) => format!("refused: {e}"),
    }
}

/// Every read the checks compare, as of every position through `through`:
/// each type's records page by page with provenance, a lexical search, a
/// walk from account 1 under either key, and the proposals. Each entry is
/// the read and its serialised answer.
async fn answers(
    store: &dyn LakeStore,
    through: Position,
) -> Result<Vec<(String, String)>, Failure> {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    let mut out = Vec::new();
    for p in 0..=through.0 {
        let at = read_at(Some(Position(p)));
        for ty in TYPES {
            let mut find = Find {
                with_meta: true,
                limit: Some(3),
                at,
                ..Find::default()
            };
            for page in 0.. {
                let reply = cat.find(own, &TypeName::from(ty), find.clone()).await;
                let next = match &reply {
                    Ok(r) => match &r.data {
                        Found::Records { next_cursor, .. } => next_cursor.clone(),
                        Found::Groups { .. } => None,
                    },
                    Err(_) => None,
                };
                out.push((format!("find `{ty}` at {p}, page {page}"), said(reply)));
                match next {
                    Some(c) if page < 100 => find.cursor = Some(c),
                    _ => break,
                }
            }
        }
        let search = Search {
            ty: TypeName::from("invoice"),
            mode: SearchMode::lexical("lake note"),
            k: 20,
            cursor: None,
            filter: None,
            with_meta: true,
            at,
        };
        out.push((
            format!("a lexical search at {p}"),
            said(cat.search(own, search).await),
        ));
        let vector = Search {
            ty: TypeName::from("note"),
            mode: SearchMode::vector("vec", vec![1.0, 0.0]),
            k: 20,
            cursor: None,
            filter: None,
            with_meta: true,
            at,
        };
        out.push((
            format!("a vector search at {p}"),
            said(cat.search(own, vector).await),
        ));
        for key in ["1", "DE1"] {
            let mut walk = pattern(
                "account",
                Some(key),
                vec![step("owner", Walk::Out)],
                Projection::Graph,
            );
            walk.at = at;
            out.push((
                format!("a walk from account `{key}` at {p}"),
                said(cat.query(own, walk).await),
            ));
        }
        let filter = ProposalFilter {
            at,
            ..ProposalFilter::default()
        };
        out.push((
            format!("the proposals at {p}"),
            said(cat.proposals(own, filter).await),
        ));
    }
    Ok(out)
}

/// Fails naming the first read that answers otherwise than before.
fn same(
    before: &[(String, String)],
    after: &[(String, String)],
    when: &str,
) -> Result<(), Failure> {
    for (b, a) in before.iter().zip(after) {
        ensure!(
            b == a,
            "{when}, {} answers\n  {}\nand before it answered\n  {}",
            a.0,
            a.1,
            b.1
        );
    }
    ensure!(
        before.len() == after.len(),
        "{when}, {} reads are made where there were {} before",
        after.len(),
        before.len()
    );
    Ok(())
}

async fn compact(store: &dyn LakeStore) -> Result<CompactReport, Failure> {
    store.compact().await.must("compacting the lake")
}

fn unmoved(report: &CompactReport, head: Position) -> Result<(), Failure> {
    ensure!(
        report.head == head,
        "a compact at head {head} reports the head {}",
        report.head
    );
    Ok(())
}

async fn a_compacted_lake_answers_every_read_as_before(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = churned(store.as_ref(), true, "compact_answers").await?;
    let head = store.head().await.must("reading the head")?;
    let before = answers(store.as_ref(), head).await?;
    // What is compared holds what the churn made: a backed value and
    // ranked vectors at the head.
    let answer = |read: String| {
        before
            .iter()
            .find(|(r, _)| *r == read)
            .map_or(String::new(), |(_, a)| a.clone())
    };
    let (place, vector) = (
        answer(format!("find `place` at {head}, page 0")),
        answer(format!("a vector search at {head}")),
    );
    ensure!(
        place.contains("\"lock\""),
        "the churned lake's place q1 holds the name its backing computed, `lock`; find answers \
         {place}"
    );
    ensure!(
        vector.contains("\"n2\""),
        "a vector search of the churned lake's notes ranks n2; it answers {vector}"
    );
    let report = compact(store.as_ref()).await?;
    unmoved(&report, head)?;
    let after = answers(store.as_ref(), head).await?;
    same(&before, &after, "after a compact")?;
    // The retry is recognised from what the projection keeps of applied
    // ids: it answers what the write answered, and appends nothing.
    let retried = keyed(l.cat.as_ref(), &l.own).await?;
    let now = store.head().await.must("reading the head")?;
    ensure!(
        now == head,
        "a retry of a keyed write after a compact is recognised and appends nothing; the head \
         moved from {head} to {now}, answering {retried:?}"
    );
    // The recorded outputs are kept. With the backing refusing every call,
    // a new write of an input the lake recorded (`harbour`, place q1's
    // first code) takes its value from the output the projection kept
    // through the compact, and a rebuild after it answers every read as
    // before, folding the outputs again from the log: neither calls the
    // backing.
    let token = "compact_answers";
    let calls = made(token);
    unplug(token);
    let probed = async {
        l.upsert("place", "q3", json!({"code": "harbour"})).await?;
        let q3 = l.read("place", "q3").await?;
        store.rebuild().await.must("rebuilding the projection")?;
        same(
            &before,
            &answers(store.as_ref(), head).await?,
            "after a compact and a rebuild with the backing unplugged",
        )?;
        Ok::<_, Failure>(q3)
    }
    .await;
    plug(token);
    let q3 = probed?;
    ensure!(
        q3.fields.get("name") == Some(&json!("harbour")) && made(token) == calls,
        "after a compact, a rebuild and a write of the recorded input `harbour` read the recorded \
         outputs and call no backing; place q3's name reads {:?}, and {} calls were made where \
         {calls} were before",
        q3.fields.get("name"),
        made(token)
    );
    Ok(())
}

async fn log_of(store: &dyn LakeStore) -> Result<String, Failure> {
    let events = log_events(store).await?;
    Ok(serde_json::to_string(&events).expect("events serialise"))
}

/// Every file under `dir`, by its path within it, with its bytes.
fn tree(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, Failure> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let entries =
            fs::read_dir(&d).map_err(|e| Failure::new(format!("listing {}: {e}", d.display())))?;
        for entry in entries {
            let path = entry
                .map_err(|e| Failure::new(format!("listing {}: {e}", d.display())))?
                .path();
            if path.is_dir() {
                todo.push(path);
            } else {
                let bytes = fs::read(&path)
                    .map_err(|e| Failure::new(format!("reading {}: {e}", path.display())))?;
                let name = path.strip_prefix(dir).unwrap_or(&path);
                out.push((name.display().to_string(), bytes));
            }
        }
    }
    out.sort();
    Ok(out)
}

async fn compact_appends_nothing_and_exports_the_same_bytes(
    store: Arc<dyn LakeStore>,
    scratch: PathBuf,
) -> Result<(), Failure> {
    let scratch = super::export::Scratch::new(scratch);
    churned(store.as_ref(), false, "compact_appends").await?;
    let head = store.head().await.must("reading the head")?;
    let log = log_of(store.as_ref()).await?;
    let verified = store.verify(None, None).await.must("verifying the lake")?;
    let (first, second) = (scratch.dir("before"), scratch.dir("after"));
    let manifest = export::write(store.as_ref(), &first)
        .await
        .must("exporting before the compact")?;

    let report = compact(store.as_ref()).await?;
    unmoved(&report, head)?;
    let now = store.head().await.must("reading the head")?;
    ensure!(now == head, "a compact moved the head from {head} to {now}");
    ensure!(
        log_of(store.as_ref()).await? == log,
        "the feed from the start answers other events after a compact than before"
    );
    let again = store.verify(None, None).await.must("verifying the lake")?;
    ensure!(
        again == verified,
        "verify answers {again:?} after a compact and {verified:?} before"
    );
    let later = export::write(store.as_ref(), &second)
        .await
        .must("exporting after the compact")?;
    ensure!(
        later == manifest,
        "the export's manifest after a compact is {later:?}, and before it was {manifest:?}"
    );
    let (b, a) = (tree(&first)?, tree(&second)?);
    let names = |t: &[(String, Vec<u8>)]| t.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
    ensure!(
        names(&b) == names(&a),
        "an export after a compact holds the files {:?}, and before it held {:?}",
        names(&a),
        names(&b)
    );
    for ((name, before), (_, after)) in b.iter().zip(&a) {
        ensure!(
            before == after,
            "{name} of an export after a compact differs from the one before"
        );
    }
    Ok(())
}

async fn a_lake_compacts_again_and_writes_on(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    churned(store.as_ref(), true, "compact_again").await?;
    let head = store.head().await.must("reading the head")?;
    let before = answers(store.as_ref(), head).await?;
    for n in ["a first", "a second"] {
        let report = compact(store.as_ref()).await?;
        unmoved(&report, head)?;
        same(
            &before,
            &answers(store.as_ref(), head).await?,
            &format!("after {n} compact"),
        )?;
    }
    let l = Lake::of(store.as_ref());
    let was = l.read("invoice", "i0").await?.version;
    let o = l.upsert("invoice", "i0", json!({"amount": 1000})).await?;
    let Outcome::Applied {
        version: Some(v), ..
    } = &o
    else {
        return Err(Failure::new(format!(
            "an upsert after a compact answers `applied` with a version; it answered {o:?}"
        )));
    };
    ensure!(
        v.0 == was.0 + 1,
        "a write after a compact makes version {} of invoice i0; version {} was its last",
        v.0,
        was.0
    );
    let next = Position(head.0 + 1);
    ensure!(
        o.position() == next,
        "the first write after a compact at head {head} lands at {}, not {next}",
        o.position()
    );
    Ok(())
}

const WRITERS: usize = 4;
const EACH: usize = 10;

/// What one caller did: a write of `item` (its key, the value written and
/// its position), or a compact (the head it reported).
enum Done {
    Wrote(String, i64, Position),
    Compacted(Position),
}

type Doing = Caller<'static, Result<Vec<Done>, Failure>>;

/// One caller writing its own records of `item`.
fn writer(cat: Arc<dyn Catalog>, who: Principal, w: usize) -> Doing {
    Box::pin(async move {
        let mut out = Vec::new();
        for n in 0..EACH {
            let key = format!("w{w}-{n}");
            let o = upsert(cat.as_ref(), &who, "item", &key, json!({"n": n})).await?;
            out.push(Done::Wrote(key, n as i64, o.position()));
        }
        Ok(out)
    })
}

/// One caller compacting three times over.
fn compactor(store: Arc<dyn LakeStore>) -> Doing {
    Box::pin(async move {
        let mut out = Vec::new();
        for _ in 0..3 {
            let report = store
                .compact()
                .await
                .must("compacting while others write")?;
            out.push(Done::Compacted(report.head));
        }
        Ok(out)
    })
}

async fn writes_made_while_a_compact_runs_each_land_once_before_or_after_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = churned(store.as_ref(), true, "compact_writes").await?;
    let Lake { cat, own } = &l;
    l.declare(type_decl("item", vec![field("n", StorageKind::Number)]))
        .await?;
    let head = store.head().await.must("reading the head")?;
    let before = answers(store.as_ref(), head).await?;

    let mut callers: Vec<Doing> = (0..WRITERS)
        .map(|w| writer(cat.clone(), own.clone(), w))
        .collect();
    callers.push(compactor(store.clone()));
    let mut written = Vec::new();
    let mut compacted = Vec::new();
    for r in join_all(callers).await {
        for done in r? {
            match done {
                Done::Wrote(key, n, at) => written.push((key, n, at)),
                Done::Compacted(head) => compacted.push(head),
            }
        }
    }

    let now = store.head().await.must("reading the head")?;
    let want = Position(head.0 + (WRITERS * EACH) as u64);
    ensure!(
        now == want,
        "{} writes after head {head} end at {want} when each lands once and a compact appends \
         nothing; the head is {now}",
        WRITERS * EACH
    );
    let mut positions: Vec<Position> = written.iter().map(|w| w.2).collect();
    positions.sort();
    positions.dedup();
    ensure!(
        positions.len() == WRITERS * EACH,
        "each write answers a position of its own; {} writes answered {} positions",
        WRITERS * EACH,
        positions.len()
    );
    for h in &compacted {
        ensure!(
            *h >= head && *h <= now,
            "a compact during the writes reports the head {h}, outside {head}..={now}"
        );
    }
    for (key, n, at) in &written {
        let r = cat
            .get(own, &"item".into(), get_at(key, Some(*at)))
            .await
            .must(&format!("reading item `{key}` at {at}"))?;
        ensure!(
            r.data.fields.get("n") == Some(&json!(n)),
            "item `{key}` written with n = {n} at {at} reads {:?} there",
            r.data.fields.get("n")
        );
    }
    same(
        &before,
        &answers(store.as_ref(), head).await?,
        "after compacts among writes",
    )
}
