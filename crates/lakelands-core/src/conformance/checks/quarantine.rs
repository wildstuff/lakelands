//! A refused record quarantined
//! (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`):
//! a delivery that asks it keeps a record refused by a rule of the
//! record, with its cells and refusal, and goes on; the record is listed
//! under its source, first delivery and index until a resubmission lands,
//! a dismiss, a write at its key or an act on its source closes it.

use lakelands_api::IngestOpts;
use lakelands_api::quarantine::{
    ClosedWhy, QuarantineFilter, QuarantineId, Quarantined, Resubmits,
};
use lakelands_api::write::Committed;

use super::only::holder;
use crate::conformance::fixture::{get_at, restrict, source_record};
use crate::conformance::prelude::*;

check!(
    QUARANTINED_GOES_ON = a_refused_record_is_quarantined_and_the_stream_goes_on,
    "085: a delivery of three records, the second refused, with `quarantine_refused`, lands two and quarantines the second with its refusal, listed by source and type, and its window names index 1 in `quarantined`; without the option the stream ends at index 1, as before; the same at a `commit_window` of 1 and of 256"
);

/// `invoice`: a source's `amount` and `customer`, an author's `note`.
fn invoice() -> TypeDecl {
    let fields = vec![
        sourced("amount", StorageKind::Number),
        sourced("customer", StorageKind::Text),
        field("note", StorageKind::Text),
    ];
    type_decl("invoice", fields)
}

/// The options of a delivery `delivery` that quarantines its refusals.
fn kept(delivery: &str) -> IngestOpts {
    IngestOpts {
        delivery: Some(delivery.into()),
        quarantine_refused: true,
        ..IngestOpts::default()
    }
}

/// `records` fed from `source` by `who` under `opts`.
async fn fed(
    l: &Lake,
    who: &Principal,
    source: &str,
    opts: IngestOpts,
    records: Vec<SourceRecord>,
) -> Result<Vec<Committed>, Error> {
    lakelands_api::ingest_all_with(l.cat.as_ref(), who, &source.into(), opts, records).await
}

/// The stream indexes the windows quarantined.
fn indexes(windows: &[Committed]) -> Vec<u64> {
    windows.iter().flat_map(|w| w.quarantined.clone()).collect()
}

/// What `who` is listed under `filter`.
async fn listed(
    l: &Lake,
    who: &Principal,
    filter: QuarantineFilter,
) -> Result<Vec<Quarantined>, Failure> {
    let reply = l
        .cat
        .quarantined(who, filter)
        .await
        .must("listing quarantined records")?;
    Ok(reply.data)
}

/// The owner's listing of `source`'s records of `ty`.
async fn of(l: &Lake, source: &str, ty: &str) -> Result<Vec<Quarantined>, Failure> {
    let filter = QuarantineFilter {
        source: Some(source.into()),
        ty: Some(ty.into()),
        ..QuarantineFilter::default()
    };
    listed(l, &l.own, filter).await
}

/// The id `(source, delivery, index)`.
fn qid(source: &str, delivery: &str, index: u64) -> QuarantineId {
    QuarantineId {
        source: source.into(),
        delivery: delivery.into(),
        index,
    }
}

/// The invoice `key` with `amount`, and the field `extra` where given,
/// which `invoice` does not declare.
fn inv(key: &str, amount: Value, extra: Option<&str>) -> SourceRecord {
    let mut fields = json!({"amount": amount, "customer": "acme"});
    if let Some(extra) = extra {
        fields[extra] = json!("x");
    }
    source_record("invoice", key, fields)
}

async fn a_refused_record_is_quarantined_and_the_stream_goes_on(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let three = |p: &str| {
        vec![
            inv(&format!("{p}1"), json!(1), None),
            inv(&format!("{p}2"), json!(2), Some("nope")),
            inv(&format!("{p}3"), json!(3), None),
        ]
    };
    for (window, p) in [(1, "a"), (256, "b")] {
        let what = format!("a delivery at a window of {window} quarantining its refusals");
        let opts = IngestOpts {
            commit_window: Some(window),
            ..kept(p)
        };
        let windows = fed(&l, &l.own, "erp", opts, three(p)).await.must(&what)?;
        windows_reach_head(store.as_ref(), &windows, &what).await?;
        let landed = crate::conformance::fixture::committed(&windows);
        ensure!(
            landed == (2, 0) && indexes(&windows) == [1],
            "{what} answered {windows:?}"
        );
        for (k, there) in [(1, true), (2, false), (3, true)] {
            let key = format!("{p}{k}");
            let got = l
                .cat
                .get(&l.own, &"invoice".into(), get_at(&key, None))
                .await;
            ensure!(got.is_ok() == there, "after {what}, `{key}` read {got:?}");
        }
        let rows = listed(
            &l,
            &l.own,
            QuarantineFilter {
                delivery: Some(p.into()),
                ..QuarantineFilter::default()
            },
        )
        .await?;
        let [q] = rows.as_slice() else {
            return Err(Failure::new(format!(
                "after {what}, the delivery lists {rows:?}"
            )));
        };
        let refused = matches!(
            &q.refusal,
            Error::Refused {
                reason: Refusal::Invalid(..),
                ..
            }
        );
        ensure!(
            q.id == qid("erp", p, 1)
                && q.ty.as_str() == "invoice"
                && q.key.0 == format!("{p}2")
                && q.version == 1
                && q.cells.get("nope") == Some(&json!("x"))
                && q.closed.is_none()
                && refused,
            "after {what}, the quarantined record is {q:?}"
        );
    }
    let both = of(&l, "erp", "invoice").await?;
    let ids: Vec<_> = both.iter().map(|q| q.id.clone()).collect();
    ensure!(
        ids == [qid("erp", "a", 1), qid("erp", "b", 1)],
        "`erp`'s invoices list {ids:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let plain = IngestOpts {
        delivery: Some("c".into()),
        ..IngestOpts::default()
    };
    let ended = fed(&l, &l.own, "erp", plain, three("c")).await;
    let at_one = matches!(
        &ended,
        Err(Error::Ingested {
            index: 1,
            quarantine_full: None,
            ..
        })
    );
    ensure!(at_one, "without the option the delivery answered {ended:?}");
    let after = events_after(store.as_ref(), head).await?;
    let none = !after
        .iter()
        .any(|e| matches!(e.body, EventBody::RecordQuarantined { .. }));
    ensure!(none, "without the option the delivery appended {after:?}");
    Ok(())
}

check!(
    QUARANTINED_WHICH = a_refusal_of_the_record_is_quarantined_and_one_of_the_call_ends_the_stream,
    "085: an empty key, a field not declared, an author's field, a field not granted, a value that does not fit and an empty record at a key not live are each quarantined, the empty key listed as the empty string; a type not declared, a key holding NUL and a writer without `ingest` end the stream as without the option; past `limits.quarantined_max` a `not_granted` and a `constraint` each end it, the last item the record's own refusal with `quarantine_full` and the bound beside it, appending nothing, a resubmission refused again under a full bound is its version 2, and a dismiss frees a place under the bound; under a bound of one, a quarantined record whose key lands earlier in the same window frees its place for a later refusal of that window, at a `commit_window` of 1 and of 256",
    given
);

check!(
    QUARANTINED_SIGHT = a_quarantined_record_is_shown_by_the_sight_of_its_type,
    "085 and 006: a sensitive cell is masked to a reader without the field's right and shown with it, a `none` cell is not there; a quarantined record of a type not shown, or at a restricted key, is not listed without `restricted`; without `ingest` the listing is `not_granted`; a refusal naming a hidden field is shown as its kind alone, with `more`; a hiding lowers a cell where the `G` cell selects, where the live record at the key selects though the `G` cell does not, and where the `G` cell is a number, and lowers nothing where the `G` cell is absent and no record is live at the key"
);

check!(
    QUARANTINED_BY_DELIVERY = a_quarantine_is_keyed_by_its_records_delivery,
    "085, extended (lakelands#1528): in `page`, the record naming `item-4` at stream index 7 is quarantined as `(s, item-4, 0)`, its window naming 7 in `quarantined`; `item-4` run again at index 2 of another call and refused alike is a duplicate of that id, which stays at its first version"
);

async fn a_quarantine_is_keyed_by_its_records_delivery(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let item = || SourceRecord {
        delivery: Some("item-4".into()),
        ..inv("q", json!(4), Some("nope"))
    };
    let mut page: Vec<_> = (0..7)
        .map(|i| inv(&format!("p{i}"), json!(i), None))
        .collect();
    page.push(item());
    let windows = fed(&l, &l.own, "s", kept("page"), page).await;
    let windows = windows.must("a page quarantining a record of `item-4`")?;
    ensure!(
        indexes(&windows) == [7],
        "the page's windows name index 7: {windows:?}"
    );
    let again = vec![inv("r0", json!(0), None), inv("r1", json!(1), None), item()];
    let windows = fed(&l, &l.own, "s", kept("page-2"), again).await;
    let windows = windows.must("`item-4` run again in another call")?;
    let landed = crate::conformance::fixture::committed(&windows);
    ensure!(
        landed == (2, 1) && indexes(&windows).is_empty(),
        "`item-4` refused alike again is a duplicate: {windows:?}"
    );
    let rows = of(&l, "s", "invoice").await?;
    let ids: Vec<_> = rows.iter().map(|q| (q.id.clone(), q.version)).collect();
    ensure!(
        ids == [(qid("s", "item-4", 0), 1)],
        "the record is quarantined as `(s, item-4, 0)` at its first version: {ids:?}"
    );
    Ok(())
}

/// The kind of a refusal, as the wire names it.
fn kind(e: &Error) -> String {
    let wire = json!(e);
    wire["reason"]["refusal"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The refusal a quarantined record holds, as `(field, refusal)`.
fn refusal(q: &Quarantined) -> Option<(Option<&str>, &Refusal)> {
    match &q.refusal {
        Error::Refused { field, reason } => Some((field.as_ref().map(|f| f.as_str()), reason)),
        _ => None,
    }
}

async fn a_refusal_of_the_record_is_quarantined_and_one_of_the_call_ends_the_stream(
    store: Store,
    given: Given,
) -> Held {
    let lower = Limits {
        quarantined_max: 7,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake with a bound of 7: {e}")))?;
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let feeder = holder(
        store.as_ref(),
        "feeder",
        json!({"default": "write", "ingest": true,
               "types": {"invoice": {"fields": {"customer": "read"}}}}),
    );
    let yes: [(&str, SourceRecord, &Principal); 6] = [
        ("an empty key", inv("", json!(1), None), &l.own),
        (
            "a field not declared",
            inv("k1", json!(1), Some("nope")),
            &l.own,
        ),
        (
            "an author's field",
            inv("k2", json!(1), Some("note")),
            &l.own,
        ),
        ("a field not granted", inv("k3", json!(1), None), &feeder),
        (
            "a value that does not fit",
            inv("k4", json!("lots"), None),
            &l.own,
        ),
        (
            "an empty record",
            source_record("invoice", "k5", json!({})),
            &l.own,
        ),
    ];
    let want = [
        "invalid",
        "invalid",
        "wrong_door",
        "not_granted",
        "constraint",
        "invalid",
    ];
    for (i, ((what, record, who), want)) in yes.into_iter().zip(want).enumerate() {
        let delivery = format!("y{i}");
        let got = fed(&l, who, "erp", kept(&delivery), vec![record]).await;
        let quarantined = got.as_ref().is_ok_and(|w| indexes(w) == [0]);
        ensure!(
            quarantined,
            "{what} must be quarantined; the delivery answered {got:?}"
        );
        let rows = listed(
            &l,
            &l.own,
            QuarantineFilter {
                delivery: Some(delivery.into()),
                ..QuarantineFilter::default()
            },
        )
        .await?;
        let kind = rows.first().map(|q| kind(&q.refusal));
        ensure!(
            rows.len() == 1 && kind.as_deref() == Some(want),
            "{what} must be quarantined as `{want}`; it lists {rows:?}"
        );
    }
    let empty = listed(
        &l,
        &l.own,
        QuarantineFilter {
            delivery: Some("y0".into()),
            ..QuarantineFilter::default()
        },
    )
    .await?;
    ensure!(
        empty.first().is_some_and(|q| q.key.0.is_empty()),
        "the empty key must list as the empty string; it lists {empty:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let nobody = holder(store.as_ref(), "nobody", json!({"default": "write"}));
    let no: [(&str, SourceRecord, &Principal); 3] = [
        (
            "a type not declared",
            source_record("ghost", "g", json!({"a": 1})),
            &l.own,
        ),
        ("a key holding NUL", inv("k\u{0}", json!(1), None), &l.own),
        (
            "a writer without `ingest`",
            inv("k6", json!(1), None),
            &nobody,
        ),
    ];
    for (i, (what, record, who)) in no.into_iter().enumerate() {
        let opts = kept(&format!("n{i}"));
        let records = vec![inv(&format!("n{i}"), json!(1), None), record];
        let with = fed(&l, who, "erp", opts, records.clone()).await;
        let opts = IngestOpts {
            delivery: Some(format!("m{i}").into()),
            ..IngestOpts::default()
        };
        let without = fed(&l, who, "erp", opts, records).await;
        ensure!(
            with.is_err() && with == without,
            "{what} must end the stream as without the option; with it the delivery \
             answered {with:?}, without it {without:?}"
        );
    }
    let after = events_after(store.as_ref(), head).await?;
    let kept_none = !after
        .iter()
        .any(|e| matches!(e.body, EventBody::RecordQuarantined { .. }));
    ensure!(kept_none, "a refusal of the call quarantined: {after:?}");
    // Six are open; the seventh fills the bound, and past it each
    // refusal ends the stream with its own refusal.
    let seventh = fed(
        &l,
        &l.own,
        "erp",
        kept("full"),
        vec![inv("k7", json!("x"), None)],
    )
    .await;
    ensure!(
        seventh.as_ref().is_ok_and(|w| indexes(w) == [0]),
        "the seventh refusal must be quarantined; it answered {seventh:?}"
    );
    let past = [
        (
            "a `constraint`",
            inv("k8", json!("x"), None),
            &l.own,
            "constraint",
        ),
        (
            "a `not_granted`",
            inv("k9", json!(1), None),
            &feeder,
            "not_granted",
        ),
    ];
    for (what, record, who, want) in past {
        let records = vec![source_record("invoice", "ok", json!({"amount": 1})), record];
        let head = store.head().await.must("reading the head")?;
        let got = fed(&l, who, "erp", kept("past"), records).await;
        head_unmoved(store.as_ref(), head, &format!("{what} past the bound")).await?;
        let ended = match &got {
            Err(Error::Ingested {
                index: 1,
                error,
                quarantine_full: Some(full),
                ..
            }) => full.max == 7 && kind(error) == want,
            _ => false,
        };
        ensure!(
            ended,
            "{what} past the bound must end the stream with its own refusal and \
             `quarantine_full` of 7; it answered {got:?}"
        );
    }
    let dismissed = l
        .cat
        .dismiss_quarantined(&l.own, &qid("erp", "y1", 0))
        .await;
    ensure!(
        dismissed
            .as_ref()
            .is_ok_and(|c| c.why == ClosedWhy::Dismissed),
        "dismissing `erp/y1/0` answered {dismissed:?}"
    );
    let freed = fed(
        &l,
        &l.own,
        "erp",
        kept("freed"),
        vec![inv("k8", json!("x"), None)],
    )
    .await;
    ensure!(
        freed.as_ref().is_ok_and(|w| indexes(w) == [0]),
        "a dismiss must free a place under the bound; the next refusal answered {freed:?}"
    );
    // Full again: a new version of an open row takes no place of its own.
    let y2 = qid("erp", "y2", 0);
    let other = vec![resubmitting(&y2, inv("k2", json!(1), Some("nope")))];
    let got = fed(&l, &l.own, "erp", kept("again"), other).await;
    let q = row_of(&l, &l.own, y2.clone()).await?;
    ensure!(
        got.as_ref().is_ok_and(|w| indexes(w) == [0]) && q.is_some_and(|q| q.version == 2),
        "under a full bound, `y2` refused otherwise must be its version 2; it answered {got:?}"
    );
    // A record landing at an open row's key frees its place for the
    // window's next refusal, and one quarantined in the window and landed
    // in it frees its own: the same at either window.
    for (window, p, y) in [(1, "w", "y2"), (256, "x", "y4")] {
        let what = format!("a delivery at a window of {window} landing where rows are open");
        let at = qid("erp", y, 0);
        let row = row_of(&l, &l.own, at.clone()).await?;
        let key = row.map(|q| q.key.0).unwrap_or_default();
        let records = vec![
            inv(&key, json!(1), None),
            inv(&format!("{p}a"), json!("x"), None),
            inv(&format!("{p}a"), json!(1), None),
            inv(&format!("{p}b"), json!("x"), None),
        ];
        let opts = IngestOpts {
            commit_window: Some(window),
            ..kept(p)
        };
        let got = fed(&l, &l.own, "erp", opts, records).await.must(&what)?;
        let landed = crate::conformance::fixture::committed(&got);
        let closed = [
            closed_as(&l, at).await?,
            closed_as(&l, qid("erp", p, 1)).await?,
        ];
        ensure!(
            landed == (2, 0) && indexes(&got) == [1, 3] && closed == [Some(ClosedWhy::Landed); 2],
            "{what} answered {got:?}, and closed `{y}` and its own row {closed:?}"
        );
    }
    Ok(())
}

check!(
    QUARANTINED_TABLE = each_refusal_of_a_rule_of_the_record_is_quarantined_as_it_is_refused,
    "085: a field only verbs write, a key that does not split into its key fields, key fields the key disagrees with, a reference to no live record, a required field with no value, a record `only` does not admit, one at a key restricted from its writer, a second record of a type written once and a link past its cardinality are each quarantined with the refusal the stream ends with without the option"
);

/// The types the table's refusals are of: `acct`, keyed by `bank` and
/// `no`, `code` required, `owner` naming a `party`, `status` verbs'; a
/// `party`; `entry`, written once; `deal`, linked to one `party` at most
/// over `to`; `line`, admitted by `owner`.
async fn table(l: &Lake) -> Result<(), Failure> {
    let (text, source) = (json!({"kind": "text"}), "source");
    l.declare_json(json!({"declare": "type", "name": "party",
        "fields": [{"name": "name", "type": text, "authority": source}]}))
        .await?;
    l.declare_json(
        json!({"declare": "type", "name": "acct", "key": ["bank", "no"], "fields": [
        {"name": "bank", "type": text, "authority": source},
        {"name": "no", "type": text, "authority": source},
        {"name": "code", "type": text, "authority": source, "required": true},
        {"name": "owner", "type": {"kind": "reference", "target": "party"}, "authority": source},
        {"name": "status", "type": text, "authority": "verb", "initial": "open"}]}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "verb", "name": "shut", "on": "acct", "risk": "low",
        "effect": "write", "set": [{"field": "status", "value": "'shut'"}]}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "type", "name": "entry", "insert_only": true,
        "fields": [{"name": "amount", "type": {"kind": "number"}, "authority": source}]}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "deal",
        "fields": [{"name": "name", "type": text, "authority": source}],
        "relations": [{"name": "to", "target": "party", "cardinality": "many_to_one",
                       "authority": source}]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "line",
        "fields": [{"name": "owner", "type": text, "authority": source}]}))
        .await?;
    Ok(())
}

async fn each_refusal_of_a_rule_of_the_record_is_quarantined_as_it_is_refused(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    table(&l).await?;
    let first = [
        source_record("party", "p1", json!({"name": "a"})),
        source_record("party", "p2", json!({"name": "b"})),
        source_record("party", "r1", json!({"name": "c"})),
        source_record("deal", "d1", json!({"name": "d"})),
        source_record("entry", "e1", json!({"amount": 1})),
        source_record(
            "sys.link.deal.to",
            link_key(&"d1".into(), &"p1".into()).as_str(),
            json!({"from": "d1", "to": "p1"}),
        ),
    ];
    l.ingest("erp", first.to_vec())
        .await
        .must("ingesting the table's records")?;
    restrict(store.as_ref(), "party", "r1", "held").await?;
    let feeder = holder(
        store.as_ref(),
        "feeder",
        json!({"default": "write", "ingest": true,
               "types": {"line": {"only": {"owner": "alice"}}}}),
    );
    let acct = |key: &str, fields: Value| source_record("acct", key, fields);
    // The `sys.` row is not met here: every field of `sys.setting` and
    // `sys.aging_rule` is an author's, so a source's is refused at the door
    // first, as `wrong_door`, which the table holds already.
    let rows: [(&str, SourceRecord, &Principal, &str); 9] = [
        (
            "a field only verbs write",
            acct(
                "b:1",
                json!({"bank": "b", "no": "1", "code": "c", "status": "x"}),
            ),
            &l.own,
            "verbs_only",
        ),
        (
            "a key that does not split",
            acct("b", json!({"bank": "b", "no": "2", "code": "c"})),
            &l.own,
            "invalid",
        ),
        (
            "key fields the key disagrees with",
            acct("b:3", json!({"bank": "c", "no": "3", "code": "c"})),
            &l.own,
            "invalid",
        ),
        (
            "a reference to no live record",
            acct(
                "b:4",
                json!({"bank": "b", "no": "4", "code": "c", "owner": "ghost"}),
            ),
            &l.own,
            "constraint",
        ),
        (
            "a required field with no value",
            acct("b:5", json!({"bank": "b", "no": "5"})),
            &l.own,
            "constraint",
        ),
        (
            "a record `only` does not admit",
            source_record("line", "l1", json!({"owner": "bob"})),
            &feeder,
            "not_granted",
        ),
        (
            "a record at a key restricted from its writer",
            source_record("party", "r1", json!({"name": "e"})),
            &feeder,
            "not_granted",
        ),
        (
            "a second record of a type written once",
            source_record("entry", "e1", json!({"amount": 2})),
            &l.own,
            "constraint",
        ),
        (
            "a link past its cardinality",
            source_record(
                "sys.link.deal.to",
                link_key(&"d1".into(), &"p2".into()).as_str(),
                json!({"from": "d1", "to": "p2"}),
            ),
            &l.own,
            "linked_already_from",
        ),
    ];
    for (i, (what, record, who, want)) in rows.into_iter().enumerate() {
        let plain = IngestOpts {
            delivery: Some(format!("w{i}").into()),
            ..IngestOpts::default()
        };
        let without = fed(&l, who, "erp", plain, vec![record.clone()]).await;
        let Err(Error::Ingested { error, .. }) = &without else {
            return Err(Failure::new(format!(
                "{what} must end the stream without the option; it answered {without:?}"
            )));
        };
        // The refusal's kind, or the case it names.
        let named = json!(error).to_string().contains(&format!("\"{want}\""));
        ensure!(named, "{what} must be refused `{want}`, not {error:?}");
        let delivery = format!("t{i}");
        let with = fed(&l, who, "erp", kept(&delivery), vec![record]).await;
        let q = row_of(&l, &l.own, qid("erp", &delivery, 0)).await?;
        ensure!(
            with.as_ref().is_ok_and(|w| indexes(w) == [0])
                && q.as_ref().is_some_and(|q| q.refusal == **error),
            "{what} must be quarantined with {error:?}; it answered {with:?} and lists {q:?}"
        );
    }
    Ok(())
}

check!(
    QUARANTINED_ENDS = a_refusal_of_the_call_or_its_source_ends_the_stream_as_without_the_option,
    "085: a context too deep, a `commit_window` out of range, a record's source id ill formed, a record's delivery ill formed (`delivery_id_form`), a source forgotten, a `sys.setting` key its writer is not told, a `seen_at` outside the years 0000 to 9999, an upstream position or a cell beyond 2^53 - 1, a key store not bound, a key missing and an outage each end the stream as without the option, quarantining nothing; a refused record with an erasable cell and no key store ends it as `unbound`",
    given
);

/// `records` from `source` by `who` through `cat` with `opts`, with and
/// without `quarantine_refused`: each answer, which must be one refusal.
async fn ends_alike(
    cat: &dyn lakelands_api::Catalog,
    who: &Principal,
    (source, opts): (&str, IngestOpts),
    records: Vec<SourceRecord>,
    what: &str,
) -> Result<Error, Failure> {
    let source = SourceId::from(source);
    let asked = IngestOpts {
        quarantine_refused: true,
        ..opts.clone()
    };
    let feed = |opts| lakelands_api::ingest_all_with(cat, who, &source, opts, records.clone());
    let (with, without) = (feed(asked).await, feed(opts).await);
    match with {
        Err(e) if Err(&e) == without.as_ref().map(|_| ()) => Ok(e),
        _ => Err(Failure::new(format!(
            "{what} must end the stream as without the option; with it the delivery answered \
             {with:?}, without it {without:?}"
        ))),
    }
}

async fn a_refusal_of_the_call_or_its_source_ends_the_stream_as_without_the_option(
    store: Store,
    given: Given,
) -> Held {
    use super::erasable::{person, with};
    use super::forgotten::{Fails, failing};
    use lakelands_api::record::Upstream;
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    l.declare(person()).await?;
    let start = store.head().await.must("reading the head")?;
    let n = |d: &str| IngestOpts {
        delivery: Some(d.into()),
        ..IngestOpts::default()
    };
    let refused = |key: &str| inv(key, json!(1), Some("nope"));
    l.ingest("gone", vec![inv("g", json!(1), None)])
        .await
        .must("ingesting from `gone`")?;
    store
        .forget_source(&"gone".into(), None)
        .await
        .must("forgetting `gone`")?;
    let deep = store.catalog(crate::conformance::fixture::reactive(
        limits().max_reactive_depth + 1,
        None,
    ));
    let mut big = refused("i1");
    big.fields.insert("zz".into(), json!(1u64 << 60));
    let untold = holder(
        store.as_ref(),
        "untold",
        json!({"default": "write", "ingest": true}),
    );
    let setting = source_record(
        lakelands_api::seam::SETTING_TYPE,
        "key_period.erp",
        json!({}),
    );
    let calls: [(
        &str,
        &dyn lakelands_api::Catalog,
        &Principal,
        &str,
        IngestOpts,
        SourceRecord,
    ); 9] = [
        (
            "a context too deep",
            deep.as_ref(),
            &l.own,
            "erp",
            n("deep"),
            refused("d1"),
        ),
        (
            "a `commit_window` out of range",
            l.cat.as_ref(),
            &l.own,
            "erp",
            IngestOpts {
                commit_window: Some(0),
                ..n("window")
            },
            refused("w1"),
        ),
        (
            "a record's source id ill formed",
            l.cat.as_ref(),
            &l.own,
            "erp",
            n("form"),
            SourceRecord {
                source: Some("".into()),
                ..refused("f1")
            },
        ),
        (
            "a record's delivery ill formed",
            l.cat.as_ref(),
            &l.own,
            "erp",
            n("named"),
            SourceRecord {
                delivery: Some("d7".into()),
                ..refused("n1")
            },
        ),
        (
            "a source forgotten",
            l.cat.as_ref(),
            &l.own,
            "gone",
            n("gone"),
            refused("g"),
        ),
        (
            "a setting its writer is not told",
            l.cat.as_ref(),
            &untold,
            "erp",
            n("told"),
            setting,
        ),
        (
            "a `seen_at` outside the years",
            l.cat.as_ref(),
            &l.own,
            "erp",
            n("seen"),
            SourceRecord {
                seen_at: Some(Timestamp(i64::MAX)),
                ..refused("s1")
            },
        ),
        (
            "an upstream position beyond 2^53 - 1",
            l.cat.as_ref(),
            &l.own,
            "erp",
            n("up"),
            SourceRecord {
                upstream: Some(Upstream {
                    lake: store.lake().clone(),
                    position: Position(1 << 60),
                }),
                ..refused("u1")
            },
        ),
        (
            "a cell beyond 2^53 - 1",
            l.cat.as_ref(),
            &l.own,
            "erp",
            n("big"),
            big,
        ),
    ];
    for (what, cat, who, source, opts, record) in calls {
        let records = vec![inv(&format!("{source}0"), json!(1), None), record];
        ends_alike(cat, who, (source, opts), records, what).await?;
    }
    // No key store: a refused record with an erasable cell is not sealed.
    let ada = vec![refused_person("p1", "Ada")];
    let keyless = fed(&l, &l.own, "crm", kept("keyless"), ada.clone()).await;
    ensure!(
        matches!(&keyless, Err(Error::Ingested { error, .. }) if matches!(**error, Error::Unbound { .. })),
        "a refused record with an erasable cell and no key store answered {keyless:?}"
    );
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    let what = "a key store not bound";
    ends_alike(
        l.cat.as_ref(),
        &l.own,
        ("crm", n("unbound")),
        vec![bob.clone()],
        what,
    )
    .await?;
    let (_, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    Lake::of(store.as_ref())
        .ingest("crm", vec![bob])
        .await
        .must("ingesting `p2`")?;
    let other = Arc::new(super::erasable::Keys::default());
    let store =
        super::erasable::again(&given.reopen, store, with(&other), "with another key store")
            .await?;
    let l = Lake::of(store.as_ref());
    let missing = |e: &Error| {
        let e = match e {
            Error::Ingested { error, .. } => error,
            e => e,
        };
        matches!(e.situation(), Some(Situation::KeyMissing { .. }))
    };
    let cy = source_record("person", "p3", json!({"id": "p3", "name": "Cy"}));
    let what = "a key missing";
    let lost = ends_alike(l.cat.as_ref(), &l.own, ("crm", n("lost")), vec![cy], what).await?;
    let sealed = fed(&l, &l.own, "crm", kept("lost"), ada).await;
    ensure!(
        missing(&lost) && sealed.as_ref().err().is_some_and(missing),
        "a key missing must end the stream as `key_missing`, a refused record's with an \
         erasable cell too; it answered {lost:?} and {sealed:?}"
    );
    let fails: Fails = Arc::default();
    let store = (given.fault)(store, with(&other), failing(&fails))
        .await
        .map_err(|e| Failure::new(format!("opening the lake again through a fault: {e}")))?;
    let l = Lake::of(store.as_ref());
    let records = vec![inv("o0", json!(1), None), refused("o1")];
    let mut answers = Vec::new();
    for asked in [true, false] {
        fails[8].store(1, Ordering::SeqCst);
        fails[9].store(1, Ordering::SeqCst);
        let opts = IngestOpts {
            quarantine_refused: asked,
            ..n("out")
        };
        answers.push(fed(&l, &l.own, "erp", opts, records.clone()).await);
    }
    // Each meets the lease another writer holds, at a fence of its own.
    let out = |a: &Result<Vec<Committed>, Error>| match a {
        Err(Error::Unavailable { situation }) => Some(std::mem::discriminant(&**situation)),
        _ => None,
    };
    ensure!(
        out(&answers[0]).is_some() && out(&answers[0]) == out(&answers[1]),
        "an outage must end the stream as without the option; it answered {answers:?}"
    );
    let after = events_after(store.as_ref(), start).await?;
    let none = !after
        .iter()
        .any(|e| matches!(e.body, EventBody::RecordQuarantined { .. }));
    ensure!(
        none,
        "a refusal of the call or its source quarantined: {after:?}"
    );
    Ok(())
}

/// `contact`: a source's `name`, `email`, which is sensitive, `passcode` and
/// `region`, the field a hiding selects on.
fn contact() -> TypeDecl {
    let email = FieldDecl {
        sensitive: true,
        ..sourced("email", StorageKind::Text)
    };
    let fields = vec![
        sourced("name", StorageKind::Text),
        email,
        sourced("passcode", StorageKind::Text),
        sourced("region", StorageKind::Text),
    ];
    type_decl("contact", fields)
}

/// The cells a row is shown, and those named masked.
fn told(q: &Quarantined) -> String {
    let cells: Vec<&str> = q.cells.keys().map(|f| f.as_str()).collect();
    let masked: Vec<&str> = q.masked.iter().map(|f| f.as_str()).collect();
    format!("{} masked={}", cells.join(" "), masked.join(" "))
}

/// The row of `id` as `who` is listed it; none where the listing of the
/// id alone answers `not_found`, as for one not there.
async fn row_of(
    l: &Lake,
    who: &Principal,
    id: QuarantineId,
) -> Result<Option<Quarantined>, Failure> {
    let filter = QuarantineFilter {
        source: Some(id.source.clone()),
        delivery: Some(id.delivery.clone()),
        index: Some(id.index),
        ..QuarantineFilter::default()
    };
    match l.cat.quarantined(who, filter).await {
        Err(Error::NotFound {
            missing: Missing::Quarantined { id: missing },
        }) if missing == id => Ok(None),
        got => Ok(got
            .must(&format!("listing `{id}`"))?
            .data
            .into_iter()
            .next()),
    }
}

async fn a_quarantined_record_is_shown_by_the_sight_of_its_type(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(contact()).await?;
    let cells = |region: Value| json!({"name": "Ada", "email": "a@x", "passcode": "1234", "region": region, "zz": 1});
    let live = source_record("contact", "c2", json!({"name": "Bea", "region": "eu"}));
    fed(&l, &l.own, "crm", kept("live"), vec![live])
        .await
        .must("ingesting `c2`")?;
    let records = vec![
        source_record("contact", "c1", cells(json!("eu"))),
        source_record("contact", "c2", cells(json!("us"))),
        source_record("contact", "c3", json!({"name": "Cy", "region": 7})),
        source_record("contact", "c4", json!({"name": "Di", "zz": 1})),
    ];
    let got = fed(&l, &l.own, "crm", kept("d"), records)
        .await
        .must("quarantining four")?;
    ensure!(
        indexes(&got) == [0, 1, 2, 3],
        "the four quarantined {got:?}"
    );
    let reader = |id: &str, fields: Value, more: Value| {
        let mut grants = json!({"default": "read", "ingest": true,
            "types": {"contact": {"default": "none", "fields": fields}}});
        if let (Some(c), Value::Object(more)) = (grants["types"]["contact"].as_object_mut(), more) {
            c.extend(more);
        }
        holder(store.as_ref(), id, grants)
    };
    let seen = json!({"name": "read", "region": "read", "passcode": "none"});
    let masks = reader("masks", seen.clone(), json!({}));
    let mut all = seen.clone();
    all["email"] = json!("read");
    let shows = reader("shows", all, json!({}));
    let one = qid("crm", "d", 0);
    let (masked, shown) = (
        row_of(&l, &masks, one.clone()).await?,
        row_of(&l, &shows, one).await?,
    );
    let (masked, shown) = (masked.as_ref().map(told), shown.as_ref().map(told));
    ensure!(
        masked.as_deref() == Some("name region masked=email")
            && shown.as_deref() == Some("email name region masked="),
        "`email` must be masked without its right and shown with it, `passcode` and `zz`, \
         which the type's default does not show, not there; the rows read {masked:?} and {shown:?}"
    );
    let hide =
        json!({"hide_where": [{"field": "region", "values": ["eu"], "fields": {"name": "none"}}]});
    let hider = reader("hider", json!({"name": "read", "region": "read"}), hide);
    let mut lowered = Vec::new();
    for i in 0..4 {
        let q = row_of(&l, &hider, qid("crm", "d", i)).await?;
        lowered.push(q.is_some_and(|q| !q.cells.contains_key("name")));
    }
    ensure!(
        lowered == [true, true, true, false],
        "the hiding must lower `name` where `region` selects, where the live record \
         does, and where `region` is a number, and not where it is absent and no record \
         is live; it lowered {lowered:?} of `c1` to `c4`"
    );
    let blind = reader("blind", json!({}), json!({}));
    let none = listed(&l, &blind, QuarantineFilter::default()).await?;
    ensure!(
        none.is_empty(),
        "a reader not shown `contact` is listed {none:?}"
    );
    let nobody = holder(store.as_ref(), "nobody", json!({"default": "read"}));
    let refused = l
        .cat
        .quarantined(&nobody, QuarantineFilter::default())
        .await;
    ensure!(
        matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "without `ingest` the listing answered {refused:?}"
    );
    // `r1` restricted, a writer without `restricted` is refused at it,
    // and a writer of `passcode` is refused naming it.
    let r1 = source_record("contact", "r1", json!({"name": "Ed"}));
    fed(&l, &l.own, "crm", kept("r"), vec![r1])
        .await
        .must("ingesting `r1`")?;
    restrict(store.as_ref(), "contact", "r1", "held").await?;
    let feeder = holder(
        store.as_ref(),
        "feeder",
        json!({"default": "write", "ingest": true,
               "types": {"contact": {"fields": {"passcode": "read"}}}}),
    );
    let two = vec![
        source_record("contact", "r1", json!({"name": "Eve"})),
        source_record("contact", "p1", json!({"passcode": "0000"})),
    ];
    let got = fed(&l, &feeder, "crm", kept("f"), two)
        .await
        .must("feeding `r1` and `p1`")?;
    ensure!(
        indexes(&got) == [0, 1],
        "feeding `r1` and `p1` answered {got:?}"
    );
    let (held, owned) = (
        row_of(&l, &masks, qid("crm", "f", 0)).await?,
        row_of(&l, &l.own, qid("crm", "f", 0)).await?,
    );
    ensure!(
        held.is_none() && owned.is_some(),
        "the record at the restricted key must be listed only with `restricted`; \
         without it it lists {held:?}, with it {owned:?}"
    );
    let (hidden, named) = (
        row_of(&l, &masks, qid("crm", "f", 1)).await?,
        row_of(&l, &l.own, qid("crm", "f", 1)).await?,
    );
    let named_pin = named
        .as_ref()
        .and_then(refusal)
        .is_some_and(|(f, _)| f == Some("passcode"));
    let alone = hidden.as_ref().is_some_and(|q| {
        q.more
            && q.refusal == q.refusal.kind_alone()
            && refusal(q).is_some_and(|(f, r)| f.is_none() && *r == Refusal::NotGranted)
    });
    ensure!(
        named_pin && alone,
        "a refusal naming `passcode` must be shown its kind alone, with `more`, to a reader \
         who may not know `passcode`; the owner reads {named:?}, that reader {hidden:?}"
    );
    Ok(())
}

check!(
    QUARANTINED_ACTS = a_quarantined_record_is_sealed_and_closed_by_the_acts_on_its_source,
    "085, 058 and 065: an erasable cell is sealed in the log, no plaintext outside its box; `forget_source` closes every quarantined record of the source and no cell of it is answered; `remove_values` by delivery, by time and by `seen_at` closes what it selects; a record first quarantined under `mon` sealed apart, resubmitted under `tue` and refused again, stays open with its `tue` version when `mon` is taken back and closes when `tue` is; an export, which a verifier without the key reads, holds no plaintext of either version; the reports of `forget_source`, `remove_values` and a purge, preflight and act, count the quarantined records they close by type; a purge of one type of a source closes that type's alone and a purge of the source the rest; a rebuild and an import answer the same quarantined records as the head",
    given
);

/// A `person` with `name` and the field `zz`, which `person` does not
/// declare, so it is refused.
fn refused_person(key: &str, name: &str) -> SourceRecord {
    source_record("person", key, json!({"id": key, "name": name, "zz": 1}))
}

/// How the row of `id` is closed, as its owner lists it.
async fn closed_as(l: &Lake, id: QuarantineId) -> Result<Option<ClosedWhy>, Failure> {
    let q = row_of(l, &l.own, id.clone()).await?;
    let q = q.ok_or_else(|| Failure::new(format!("`{id}` is not listed")))?;
    ensure!(
        q.closed.is_none() || q.cells.is_empty(),
        "the closed `{id}` lists its cells {:?}",
        q.cells
    );
    Ok(q.closed.map(|c| c.why))
}

async fn a_quarantined_record_is_sealed_and_closed_by_the_acts_on_its_source(
    store: Store,
    given: Given,
) -> Held {
    use super::erasable::{person, with};
    use lakelands_api::write::Selection;
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    l.declare(invoice()).await?;
    let head = store.head().await.must("reading the head")?;
    let apart = IngestOpts {
        sealed_apart: true,
        ..kept("mon")
    };
    fed(&l, &l.own, "crm", apart, vec![refused_person("p1", "Ada")])
        .await
        .must("quarantining `p1` sealed apart")?;
    let events = events_after(store.as_ref(), head).await?;
    let sealed = events.iter().any(|e| match &e.body {
        EventBody::RecordQuarantined { sealed, .. } => {
            sealed.as_ref().is_some_and(|s| s.boxed.is_some())
                && !json!(e).to_string().contains("Ada")
        }
        _ => false,
    });
    ensure!(sealed, "`p1`'s name must be sealed in the log: {events:?}");
    let p1 = qid("crm", "mon", 0);
    let shown = row_of(&l, &l.own, p1.clone()).await?;
    ensure!(
        shown
            .as_ref()
            .is_some_and(|q| q.cells.get("name") == Some(&json!("Ada"))),
        "the owner must be shown `p1`'s name; it lists {shown:?}"
    );
    let again = SourceRecord {
        delivery: None,
        resubmits: Some(Resubmits {
            delivery: "mon".into(),
            index: 0,
        }),
        ..refused_person("p1", "Ann")
    };
    let apart = IngestOpts {
        sealed_apart: true,
        ..kept("tue")
    };
    let got = fed(&l, &l.own, "crm", apart, vec![again]).await;
    ensure!(
        got.as_ref().is_ok_and(|w| indexes(w) == [0]),
        "`p1` resubmitted and refused again answered {got:?}"
    );
    let boxes: Vec<_> = events_after(store.as_ref(), head)
        .await?
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::RecordQuarantined { sealed, .. } => sealed.map(|s| s.apart),
            _ => None,
        })
        .collect();
    ensure!(
        matches!(boxes.as_slice(), [mon, tue] if !mon.is_empty() && !tue.is_empty() && mon != tue),
        "`mon` and `tue` must each seal `p1` apart, under keys of their own: {boxes:?}"
    );
    // What a verifier without the key reads: the export, every file of it.
    let dir = given.scratch.join("sealed");
    let _ = std::fs::remove_dir_all(&dir);
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting")?;
    for entry in
        std::fs::read_dir(&dir).map_err(|e| Failure::new(format!("reading the export: {e}")))?
    {
        let path = entry
            .map_err(|e| Failure::new(format!("reading the export: {e}")))?
            .path();
        let bytes = std::fs::read(&path).unwrap_or_default();
        let plain = [&b"Ada"[..], b"Ann"]
            .iter()
            .any(|n| bytes.windows(n.len()).any(|w| w == *n));
        ensure!(
            !plain,
            "the export's `{}` holds `p1`'s name in plaintext",
            path.display()
        );
    }
    let take = |selection: Selection| {
        let store = store.clone();
        async move {
            let what = format!("taking back {selection:?}");
            store
                .remove_values(&"crm".into(), &selection, false, None)
                .await
                .must(&what)
        }
    };
    take(Selection::Delivery("mon".into())).await?;
    let q = row_of(&l, &l.own, p1.clone()).await?;
    ensure!(
        q.as_ref().is_some_and(|q| q.closed.is_none()
            && q.version == 2
            && q.cells.get("name") == Some(&json!("Ann"))),
        "taking back `mon` must leave `p1` open at its `tue` version, read; it lists {q:?}"
    );
    let tue = Selection::Delivery("tue".into());
    let one = vec![lakelands_api::quarantine::TypeQuarantined {
        ty: "person".into(),
        quarantined: 1,
    }];
    let planned = store
        .remove_values_preflight(&"crm".into(), &tue)
        .await
        .must("the preflight of taking back `tue`")?;
    let taken = take(tue).await?;
    ensure!(
        planned.quarantined == one && taken.quarantined == one,
        "taking back `tue` must count `p1`; its preflight counts {:?}, the act {:?}",
        planned.quarantined,
        taken.quarantined
    );
    let why = closed_as(&l, p1).await?;
    ensure!(
        why == Some(ClosedWhy::Removed),
        "taking back `tue` left `p1` {why:?}"
    );
    let seen = |key: &str, at: Option<i64>| SourceRecord {
        seen_at: at.map(Timestamp),
        ..refused_person(key, "Bo")
    };
    let two = vec![seen("p2", Some(1_000)), seen("p3", None)];
    fed(&l, &l.own, "crm", kept("s"), two)
        .await
        .must("quarantining `p2` and `p3`")?;
    take(Selection::SeenBefore(Timestamp(2_000))).await?;
    let (p2, p3) = (
        closed_as(&l, qid("crm", "s", 0)).await?,
        closed_as(&l, qid("crm", "s", 1)).await?,
    );
    ensure!(
        p2 == Some(ClosedWhy::Removed) && p3.is_none(),
        "taking back what was seen before 2000 must close `p2` alone; `p2` is {p2:?}, `p3` {p3:?}"
    );
    let last = events_after(store.as_ref(), head).await?;
    let now = last.last().map_or(0, |e| e.known_at.0) + 1;
    take(Selection::WrittenBefore(Timestamp(now))).await?;
    let p3 = closed_as(&l, qid("crm", "s", 1)).await?;
    ensure!(
        p3 == Some(ClosedWhy::Removed),
        "taking back what was written before now left `p3` {p3:?}"
    );
    for source in ["web", "erp"] {
        let records = vec![refused_person("x", "Cy"), inv("i", json!(1), Some("nope"))];
        fed(&l, &l.own, source, kept(source), records)
            .await
            .must("quarantining `x` and `i`")?;
    }
    let count = |ty: &str| lakelands_api::quarantine::TypeQuarantined {
        ty: ty.into(),
        quarantined: 1,
    };
    let both = vec![count("invoice"), count("person")];
    let (web, erp) = (SourceId::from("web"), SourceId::from("erp"));
    let planned = store
        .forget_preflight(&web)
        .await
        .must("the preflight of forgetting `web`")?;
    let forgot = store
        .forget_source(&web, None)
        .await
        .must("forgetting `web`")?;
    ensure!(
        planned.quarantined == both && forgot.quarantined == both,
        "forgetting `web` must count its two; its preflight counts {:?}, the act {:?}",
        planned.quarantined,
        forgot.quarantined
    );
    let person = TypeName::from("person");
    let planned = store
        .purge_preflight(&erp, Some(&person))
        .await
        .must("the preflight of purging `erp`'s persons")?;
    let purged = store
        .purge_source(&erp, Some(&person), None)
        .await
        .must("purging `erp`'s persons")?;
    let alone = [
        closed_as(&l, qid("erp", "erp", 0)).await?,
        closed_as(&l, qid("erp", "erp", 1)).await?,
    ];
    ensure!(
        planned.quarantined == [count("person")]
            && purged.quarantined == [count("person")]
            && alone == [Some(ClosedWhy::Purged), None],
        "purging `erp`'s persons must count and close `x` alone; its preflight counts {:?}, \
         the act {:?}, and `x` and `i` are {alone:?}",
        planned.quarantined,
        purged.quarantined
    );
    let planned = store
        .purge_preflight(&erp, None)
        .await
        .must("the preflight of purging `erp`")?;
    let purged = store
        .purge_source(&erp, None, None)
        .await
        .must("purging `erp`")?;
    let (web, erp) = (
        closed_as(&l, qid("web", "web", 0)).await?,
        closed_as(&l, qid("erp", "erp", 1)).await?,
    );
    ensure!(
        planned.quarantined == [count("invoice")]
            && purged.quarantined == [count("invoice")]
            && web == Some(ClosedWhy::Forgotten)
            && erp == Some(ClosedWhy::Purged),
        "forgetting `web` left its record {web:?}, purging `erp` its {erp:?}, counted {:?} \
         and {:?}",
        planned.quarantined,
        purged.quarantined
    );
    fed(
        &l,
        &l.own,
        "crm",
        kept("open"),
        vec![refused_person("p4", "Di")],
    )
    .await
    .must("quarantining `p4`")?;
    let at_head = listed(&l, &l.own, QuarantineFilter::default()).await?;
    store.rebuild().await.must("rebuilding")?;
    let rebuilt = listed(&l, &l.own, QuarantineFilter::default()).await?;
    ensure!(
        rebuilt == at_head,
        "rebuilt, the lake lists {rebuilt:?}, not {at_head:?}"
    );
    let dir = given.scratch.join("quarantined");
    let _ = std::fs::remove_dir_all(&dir);
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting")?;
    let imported = (given.import)(dir, store.lake().clone(), None, with(&keys))
        .await
        .map_err(|e| Failure::new(format!("importing: {e}")))?;
    let cat = imported.store.catalog(Default::default());
    let there = cat.quarantined(&l.own, QuarantineFilter::default()).await;
    let there = there.must("listing the imported lake")?.data;
    ensure!(
        there == at_head,
        "imported, the lake lists {there:?}, not {at_head:?}"
    );
    Ok(())
}

check!(
    QUARANTINED_CLOSED = a_quarantined_record_closes_when_it_lands_or_is_dismissed,
    "085: a resubmission lands and closes `resubmitted`, under a mended key too; one that is a duplicate appends the close alone; an author's `upsert`, an `enact` and another source's ingest at the key close `landed`; an aging move and a duplicate at the key close nothing; a dismiss closes and its repeat appends nothing; a closed quarantined record is listed with no cell"
);

/// `key` of `invoice` quarantined from `source` in `delivery`, refused
/// for the field `nope`; its id.
async fn quarantine_at(
    l: &Lake,
    source: &str,
    delivery: &str,
    key: &str,
) -> Result<QuarantineId, Failure> {
    let what = format!("quarantining `{key}` in `{delivery}`");
    let got = fed(
        l,
        &l.own,
        source,
        kept(delivery),
        vec![inv(key, json!(1), Some("nope"))],
    )
    .await;
    ensure!(
        got.as_ref().is_ok_and(|w| indexes(w) == [0]),
        "{what} answered {got:?}"
    );
    Ok(qid(source, delivery, 0))
}

/// `record` resubmitting `id`.
fn resubmitting(id: &QuarantineId, record: SourceRecord) -> SourceRecord {
    SourceRecord {
        delivery: None,
        resubmits: Some(Resubmits {
            delivery: id.delivery.clone(),
            index: id.index,
        }),
        ..record
    }
}

async fn a_quarantined_record_closes_when_it_lands_or_is_dismissed(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let ingest = |source: &'static str, key: &'static str, amount: i64| {
        let l = &l;
        async move {
            let what = format!("ingesting `{key}` from `{source}`");
            fed(
                l,
                &l.own,
                source,
                IngestOpts::default(),
                vec![inv(key, json!(amount), None)],
            )
            .await
            .must(&what)
        }
    };
    let k1 = quarantine_at(&l, "erp", "r1", "k1").await?;
    let k2 = quarantine_at(&l, "erp", "r2", "bad key").await?;
    let mended = vec![
        resubmitting(&k1, inv("k1", json!(1), None)),
        resubmitting(&k2, inv("k2", json!(2), None)),
    ];
    let landed = fed(&l, &l.own, "erp", kept("fix"), mended)
        .await
        .must("resubmitting `k1` and `k2`")?;
    let (w1, w2) = (closed_as(&l, k1).await?, closed_as(&l, k2).await?);
    ensure!(
        crate::conformance::fixture::committed(&landed) == (2, 0)
            && w1 == Some(ClosedWhy::Resubmitted)
            && w2 == Some(ClosedWhy::Resubmitted),
        "resubmitted, `k1` and `k2` under its mended key answered {landed:?} and closed \
         {w1:?} and {w2:?}"
    );
    ingest("erp", "k3", 3).await?;
    let k3 = quarantine_at(&l, "erp", "r3", "k3").await?;
    let head = store.head().await.must("reading the head")?;
    let same = vec![resubmitting(&k3, inv("k3", json!(3), None))];
    fed(&l, &l.own, "erp", kept("same"), same)
        .await
        .must("resubmitting `k3` unchanged")?;
    let after = events_after(store.as_ref(), head).await?;
    let alone = matches!(
        after.as_slice(),
        [Event {
            body: EventBody::QuarantineClosed {
                why: ClosedWhy::Resubmitted,
                ..
            },
            ..
        }]
    );
    ensure!(
        alone,
        "`k3` resubmitted unchanged must append its close alone: {after:?}"
    );
    for (key, note) in [("k4", "open"), ("k5", "enact"), ("k7", "open")] {
        l.upsert("invoice", key, json!({ "note": note })).await?;
    }
    let k4 = quarantine_at(&l, "erp", "r4", "k4").await?;
    let k5 = quarantine_at(&l, "erp", "r5", "k5").await?;
    let k6 = quarantine_at(&l, "erp", "r6", "k6").await?;
    let k7 = quarantine_at(&l, "erp", "r7", "k7").await?;
    l.upsert("invoice", "k4", json!({"note": "seen"})).await?;
    let change = lakelands_api::write::Change::Records {
        ty: "invoice".into(),
        filter: Box::new("note = 'enact'".into()),
        to: lakelands_api::write::ChangeTo::Set(patch(json!({"note": "done"}))),
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let planned = l
        .cat
        .plan(&l.own, change.clone())
        .await
        .must("planning the change")?;
    let report = planned.data.report_id;
    l.cat
        .enact(&l.own, change, &report)
        .await
        .must("enacting the change")?;
    ingest("crm", "k6", 6).await?;
    let rule =
        json!({"on": "invoice", "field": "note", "from": "open", "to": "late", "when": "true"});
    l.upsert(lakelands_api::seam::AGING_RULE_TYPE, "late", rule)
        .await?;
    let today = Timestamp::from_date(2026, 10, 1).expect("a date");
    store.age(today).await.must("aging")?;
    let moved = l.read("invoice", "k7").await?.fields.get("note").cloned();
    ensure!(
        moved == Some(json!("late")),
        "aging left `k7`'s note {moved:?}"
    );
    ingest("erp", "k8", 8).await?;
    let k8 = quarantine_at(&l, "erp", "r8", "k8").await?;
    ingest("erp", "k8", 8).await?;
    let mut whys = Vec::new();
    for id in [&k4, &k5, &k6, &k7, &k8] {
        whys.push(closed_as(&l, id.clone()).await?);
    }
    let landed = Some(ClosedWhy::Landed);
    ensure!(
        whys == [landed, landed, landed, None, None],
        "an upsert, an enact and another source's ingest must close `landed`, an aging \
         move and a duplicate nothing; `k4` to `k8` read {whys:?}"
    );
    let first = l
        .cat
        .dismiss_quarantined(&l.own, &k8)
        .await
        .must("dismissing `k8`")?;
    let head = store.head().await.must("reading the head")?;
    let again = l
        .cat
        .dismiss_quarantined(&l.own, &k8)
        .await
        .must("dismissing `k8` again")?;
    head_unmoved(store.as_ref(), head, "dismissing `k8` again").await?;
    let why = closed_as(&l, k8).await?;
    ensure!(
        first.why == ClosedWhy::Dismissed && again == first && why == Some(ClosedWhy::Dismissed),
        "a dismiss answered {first:?}, its repeat {again:?}, and `k8` lists {why:?}"
    );
    Ok(())
}

check!(
    QUARANTINED_VERSIONS = a_quarantined_record_is_versioned_and_read_as_its_type_is_redeclared,
    "085: the same delivery sent again appends nothing for a row refused the same way, open or dismissed, and makes version 2 of one refused otherwise, reopening a dismissed one; a renamed field's cell is read under its new name; a renamed relation's link records quarantined are listed under its new link type, and a link sent to the link type it had is quarantined as `unclaimed_link_type`; a removed sensitive field's cell stays masked"
);

async fn a_quarantined_record_is_versioned_and_read_as_its_type_is_redeclared(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let id = qid("erp", "v", 0);
    let send = |amount: Value| {
        let l = &l;
        async move {
            let what = format!("sending `v` with `k1` at {amount}");
            fed(
                l,
                &l.own,
                "erp",
                kept("v"),
                vec![inv("k1", amount, Some("nope"))],
            )
            .await
            .must(&what)
        }
    };
    send(json!(1)).await?;
    for state in ["open", "dismissed"] {
        if state == "dismissed" {
            l.cat
                .dismiss_quarantined(&l.own, &id)
                .await
                .must("dismissing `k1`")?;
        }
        let head = store.head().await.must("reading the head")?;
        let again = send(json!(1)).await?;
        ensure!(
            crate::conformance::fixture::committed(&again) == (0, 1) && indexes(&again).is_empty(),
            "`v` sent again with `k1` {state} answered {again:?}"
        );
        head_unmoved(store.as_ref(), head, "sending `v` again").await?;
    }
    let other = send(json!(2)).await?;
    let q = row_of(&l, &l.own, id.clone()).await?;
    ensure!(
        indexes(&other) == [0]
            && q.as_ref()
                .is_some_and(|q| q.version == 2 && q.closed.is_none()),
        "`k1` refused otherwise must be version 2, open again; it answered {other:?} and \
         lists {q:?}"
    );
    let mut renamed = invoice();
    renamed.fields[1].name = "client".into();
    l.migrate_renaming(renamed, &[("customer", "client")])
        .await?;
    let q = row_of(&l, &l.own, id).await?;
    let cells: Option<Vec<&str>> = q
        .as_ref()
        .map(|q| q.cells.keys().map(|f| f.as_str()).collect());
    ensure!(
        cells.as_deref() == Some(&["amount", "client", "nope"][..]),
        "after `customer` is renamed `client`, `k1`'s cells are {cells:?}"
    );
    // A link record of `fed`, renamed `fed_by`.
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let deal = |relation: &str| TypeDecl {
        relations: vec![RelationDecl {
            authority: Authority::Source,
            ..RelationDecl::to(relation, "partner")
        }],
        ..type_decl("deal", vec![sourced("amount", StorageKind::Number)])
    };
    l.declare(deal("fed")).await?;
    let ends = json!({"from": "d1", "to": "p1", "zz": 1});
    let lt = "sys.link.deal.fed";
    let link = source_record(lt, link_key(&"d1".into(), &"p1".into()).as_str(), ends);
    let got = fed(&l, &l.own, "erp", kept("links"), vec![link]).await;
    ensure!(
        got.as_ref().is_ok_and(|w| indexes(w) == [0]),
        "quarantining a `fed` link answered {got:?}"
    );
    let renaming = Migration {
        relation_renames: vec![("fed".into(), "fed_by".into())],
        ..Migration::to(deal("fed_by"))
    };
    let report = l
        .cat
        .migrate(&l.own, renaming.clone())
        .await
        .must("the preflight")?;
    let report = report.data.report_id;
    l.cat
        .migrate_apply(&l.own, renaming, &report)
        .await
        .must("renaming `fed`")?;
    let (new, old) = (
        of(&l, "erp", "sys.link.deal.fed_by").await?,
        of(&l, "erp", lt).await?,
    );
    ensure!(
        new.len() == 1 && old.is_empty(),
        "after `fed` is renamed, the link lists under `fed_by` {new:?} and under `fed` {old:?}"
    );
    // `fed`'s link type, which no relation carries now.
    let ends = json!({"from": "d2", "to": "p1"});
    let link = source_record(lt, link_key(&"d2".into(), &"p1".into()).as_str(), ends);
    let got = fed(&l, &l.own, "erp", kept("unclaimed"), vec![link]).await;
    let q = row_of(&l, &l.own, qid("erp", "unclaimed", 0)).await?;
    let unclaimed = q.as_ref().and_then(refusal).is_some_and(|(_, r)| {
        matches!(r, Refusal::Invalid(said) if matches!(said.case, Case::UnclaimedLinkType { .. }))
    });
    ensure!(
        got.as_ref().is_ok_and(|w| indexes(w) == [0]) && unclaimed,
        "a link of `fed`, which no relation carries, must be quarantined as such; it answered \
         {got:?} and lists {q:?}"
    );
    // `pan`, sensitive, removed: its cell stays masked.
    let pan = FieldDecl {
        sensitive: true,
        ..sourced("pan", StorageKind::Text)
    };
    let card = |fields: Vec<FieldDecl>| type_decl("card", fields);
    l.declare(card(vec![sourced("holder", StorageKind::Text), pan]))
        .await?;
    let c1 = source_record(
        "card",
        "c1",
        json!({"holder": "Ada", "pan": "4111", "zz": 1}),
    );
    fed(&l, &l.own, "erp", kept("cards"), vec![c1])
        .await
        .must("quarantining `c1`")?;
    l.migrate(card(vec![sourced("holder", StorageKind::Text)]))
        .await?;
    let reader = holder(
        store.as_ref(),
        "reader",
        json!({"default": "read", "ingest": true,
               "types": {"card": {"default": "none", "fields": {"holder": "read"}}}}),
    );
    let q = row_of(&l, &reader, qid("erp", "cards", 0)).await?;
    let told = q.as_ref().map(told);
    ensure!(
        told.as_deref() == Some("holder masked=pan"),
        "after `pan` is removed, `c1` is shown {told:?}"
    );
    Ok(())
}
