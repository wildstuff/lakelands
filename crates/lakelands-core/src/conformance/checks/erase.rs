//! A record's, a field's or a quarantined row's values erased from the
//! whole history in one act
//! (`docs/decisions/115-a-record-s-or-a-field-s-values-can-be-erased.md`):
//! every hand's values of the selection read as erased at every
//! position, the record stays live, keys, references and `initial`
//! values stay, and the act is one `values_erased` event.

use lakelands_api::error::Flaw;
use lakelands_api::quarantine::{ClosedWhy, QuarantineId};
use lakelands_api::seam::ERASE_ON_CLOSE_SETTING;
use lakelands_api::write::{EraseReport, Selection};
use lakelands_api::{IngestOpts, Origin};

use super::erasable::{again, person, with};
use super::only::holder;
use super::quarantine::{closed_as, fed, inv, invoice, kept, qid, quarantine_at, resubmitting};
use super::removed::{at, erased, tracked, visits};
use crate::conformance::fixture::{refused_as, source_record};
use crate::conformance::prelude::*;

check!(
    ERASE_RECORD = an_erase_by_record_erases_it_at_every_position,
    "115: an erase by record erases every value of the record, authored and sourced, and no read returns one at the head or at an earlier position"
);
check!(
    ERASE_FIELD = an_erase_by_field_leaves_the_other_values,
    "115: an erase by field erases that field's values and leaves the record's other values standing"
);
check!(
    ERASE_KEEPS = keys_and_references_are_never_taken,
    "115: the key field and a reference field are never taken, still read, and the type is counted in `kept`"
);
check!(
    ERASE_EVERY_HAND = an_erase_takes_every_hands_values,
    "115: an erase takes an authored and a sourced value of the same record; `remove_values` still takes only its source's"
);
check!(
    ERASE_STAYS_LIVE = an_erased_record_stays_live,
    "115: a record left holding no erasable value stays live and reads with its values erased and its provenance kept"
);
check!(
    ERASE_NOTHING = an_erase_of_nothing_appends_nothing,
    "115: an erase that would take nothing appends nothing, leaves the head where it was and says so"
);
check!(
    ERASE_QUARANTINED = an_erase_by_quarantine_id_closes_it_removed,
    "115: an erase by quarantine id appends one `values_erased` and closes the open entry `removed` in the same append"
);
check!(
    ERASE_CLOSED_ENTRY = erasing_a_closed_entry_appends_no_close,
    "115: erasing a dismissed entry appends `values_erased` alone and the entry keeps its first close"
);
check!(
    ERASE_SHREDDED = require_shredded_refuses_a_plaintext_value,
    "115: `require_shredded` refuses an erase while a value is plaintext, as `erase_not_shredded` naming the count the report lists in `unshredded`, and appends nothing"
);
check!(
    ERASE_APART = an_apart_key_is_destroyed,
    "115: erasing a quarantined row its delivery sealed apart destroys that delivery's apart key, the report naming it, and the export still imports",
    given
);
check!(
    ERASE_NOT_REMOVE = remove_values_refuses_the_new_selections,
    "115: `remove_values` refuses a record and a quarantined selection as `remove_erases`, naming `erase_values`, and appends nothing"
);
check!(
    ERASE_ON_DISMISS = a_dismiss_erases_with_the_setting_on,
    "115: with `erase_on_close` set, a dismiss appends `quarantine_closed` and then `values_erased` of the id"
);
check!(
    ERASE_ON_LANDING = a_landing_erases_only_with_the_setting_on,
    "115: with `erase_on_close` set, a close by landing erases the quarantined copy in the same append; with it unset, the close erases nothing"
);
check!(
    ERASE_CARRIED = export_import_and_reopen_answer_the_erased_values,
    "115: an export, its import and the lake opened again answer the same erased values as the lake",
    given
);
check!(
    ERASE_VERIFIES = an_export_with_values_erased_verifies,
    "115: a verifier of an export holding `values_erased` answers it whole",
    given
);
check!(
    ERASE_SOURCES = the_report_names_the_sources,
    "115: the report names each source that wrote a value of the erased record, and no author"
);
check!(
    ERASE_NOT_AS_REMOVED = a_values_removed_of_a_record_is_damage,
    "115: a `values_removed` whose selection is a record, its hash made right, is refused as damage by a verifier and by an import",
    given
);
check!(
    ERASE_FOLLOWS_REKEY = an_erase_follows_the_records_keys_back,
    "115: an erase by record follows the record backwards through a re-keying and erases the writes made at the key it held before"
);
check!(
    ERASE_TAKEN_KEY = a_taken_key_ends_the_chain,
    "115: a key another record took after a re-keying ends the chain: that record's writes are not selected and still read"
);
check!(
    ERASE_NOT_SYS = a_sys_type_is_not_erased,
    "115: a selection of a `sys.` type is refused as `not_found`, as an unknown type is, and the head does not move"
);
check!(
    ERASE_PREFLIGHT = a_preflight_appends_nothing,
    "115: a preflight appends nothing, leaves the head where it was and answers the counts the act answers; the act under its report id runs"
);
check!(
    ERASE_ON_RESUBMIT = a_resubmission_erases_as_the_system,
    "115: with `erase_on_close` set, a resubmission by a holder who may write and ingest and is not the erase's caller closes and erases in its append, the erase written by the system; with it unset, nothing is erased"
);

/// `visit` `v1` of `site` `s1`: `duration`, `place` and `site` from
/// `tracker`, then an authored `note`. The head after the ingest.
async fn one(store: &dyn LakeStore, l: &Lake) -> Result<Position, Failure> {
    visits(l).await?;
    l.upsert("site", "s1", json!({"name": "s1"})).await?;
    let v1 = json!({"id": "v1", "duration": 10, "place": "Pier", "site": "s1"});
    let records = vec![source_record("visit", "v1", v1)];
    l.ingest("tracker", records).await.must("ingesting v1")?;
    let mid = store.head().await.must("reading the head")?;
    l.upsert("visit", "v1", json!({"note": "hi"})).await?;
    Ok(mid)
}

fn rec(ty: &str, key: &str, field: Option<&str>) -> Selection {
    Selection::Record {
        ty: ty.into(),
        key: key.into(),
        field: field.map(Into::into),
    }
}

/// Whether `r` holds no value of `f` and says it was erased.
fn gone(r: &lakelands_api::Record, f: &str) -> bool {
    !r.fields.contains_key(f) && r.meta.get(f).is_some_and(|m| m.erased)
}

async fn erase(store: &dyn LakeStore, s: &Selection) -> Result<EraseReport, Failure> {
    store
        .erase_values(s, false, None, false)
        .await
        .must("erasing")
}

async fn found(
    l: &Lake,
    ty: &str,
    key: &str,
    when: Option<Position>,
) -> Result<lakelands_api::Record, Failure> {
    at(l, ty, key, when)
        .await?
        .ok_or_else(|| Failure::new(format!("`{ty}` `{key}` is found as of {when:?}")))
}

/// The bodies after `after`, as their kinds.
async fn kinds(store: &dyn LakeStore, after: Position) -> Result<Vec<EventBody>, Failure> {
    Ok(events_after(store, after)
        .await?
        .into_iter()
        .map(|e| e.body)
        .collect())
}

fn is_erase(b: &EventBody) -> bool {
    matches!(b, EventBody::ValuesErased { .. })
}

fn closed(b: &EventBody, why: ClosedWhy) -> bool {
    matches!(b, EventBody::QuarantineClosed { why: w, .. } if *w == why)
}

async fn an_erase_by_record_erases_it_at_every_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let mid = one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    for when in [None, Some(mid)] {
        let r = found(&l, "visit", "v1", when).await?;
        for f in ["duration", "place"] {
            ensure!(
                gone(&r, f),
                "`{f}` of v1 reads erased as of {when:?}; it reads {r:?}"
            );
        }
    }
    let r = found(&l, "visit", "v1", None).await?;
    ensure!(
        gone(&r, "note"),
        "the authored `note` of v1 reads erased; it reads {r:?}"
    );
    Ok(())
}

async fn an_erase_by_field_leaves_the_other_values(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", Some("duration"))).await?;
    let r = found(&l, "visit", "v1", None).await?;
    ensure!(
        gone(&r, "duration")
            && r.fields.get("place") == Some(&json!("Pier"))
            && r.fields.get("note") == Some(&json!("hi")),
        "an erase of `duration` takes it alone; v1 reads {r:?}"
    );
    Ok(())
}

async fn keys_and_references_are_never_taken(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    let report = erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    ensure!(
        report.kept.iter().any(|k| k.ty.as_str() == "visit"),
        "the report counts `visit` in `kept`; it answered {report:?}"
    );
    let r = found(&l, "visit", "v1", None).await?;
    ensure!(
        r.fields.get("id") == Some(&json!("v1")) && r.fields.get("site") == Some(&json!("s1")),
        "the key `id` and the reference `site` still read; v1 reads {r:?}"
    );
    Ok(())
}

async fn an_erase_takes_every_hands_values(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    tracked(&l, &[("v2", 20)]).await?;
    l.upsert("visit", "v2", json!({"note": "kept"})).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let r = found(&l, "visit", "v1", None).await?;
    ensure!(
        gone(&r, "note") && gone(&r, "duration"),
        "an authored and a sourced value of v1 both go; v1 reads {r:?}"
    );
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let removed = store
        .remove_values(&"tracker".into(), &all, false, None)
        .await;
    removed.must("removing tracker's values")?;
    let r = found(&l, "visit", "v2", None).await?;
    ensure!(
        r.fields.get("note") == Some(&json!("kept")) && erased(&r, "duration", "tracker"),
        "`remove_values` takes tracker's `duration` of v2 and leaves its authored `note`; v2 reads {r:?}"
    );
    Ok(())
}

async fn an_erased_record_stays_live(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let r = at(&l, "visit", "v1", None).await?;
    ensure!(
        r.as_ref().is_some_and(|r| erased(r, "duration", "tracker")),
        "v1 is still found, its `duration` erased with `tracker` its source; it reads {r:?}"
    );
    Ok(())
}

async fn an_erase_of_nothing_appends_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let before = store.head().await.must("reading the head")?;
    let again = erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    head_unmoved(store.as_ref(), before, "an erase of what is already erased").await?;
    ensure!(
        again
            .said()
            .iter()
            .any(|s| s.contains("Nothing is left to erase")),
        "a repeated erase says nothing is left; it said {:?}",
        again.said()
    );
    Ok(())
}

async fn an_erase_by_quarantine_id_closes_it_removed(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let id = quarantine_at(&l, "erp", "r1", "k1").await?;
    let before = store.head().await.must("reading the head")?;
    let report = erase(store.as_ref(), &Selection::Quarantined(id.clone())).await?;
    let got = kinds(store.as_ref(), before).await?;
    ensure!(
        got.len() == 2 && is_erase(&got[0]) && closed(&got[1], ClosedWhy::Removed),
        "the erase appends `values_erased` and a `removed` close; it appended {got:?}"
    );
    ensure!(
        report.quarantined.as_ref() == Some(&id),
        "the report names the id it closed; it answered {report:?}"
    );
    let why = closed_as(&l, id).await?;
    ensure!(
        why == Some(ClosedWhy::Removed),
        "the entry closed `removed`; it reads {why:?}"
    );
    Ok(())
}

async fn erasing_a_closed_entry_appends_no_close(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let id = quarantine_at(&l, "erp", "r1", "k1").await?;
    l.cat
        .dismiss_quarantined(&l.own, &id)
        .await
        .must("dismissing")?;
    let before = store.head().await.must("reading the head")?;
    erase(store.as_ref(), &Selection::Quarantined(id.clone())).await?;
    let got = kinds(store.as_ref(), before).await?;
    ensure!(
        got.len() == 1 && is_erase(&got[0]),
        "erasing a dismissed entry appends `values_erased` alone; it appended {got:?}"
    );
    let why = closed_as(&l, id).await?;
    ensure!(
        why == Some(ClosedWhy::Dismissed),
        "the entry keeps its dismiss; it reads {why:?}"
    );
    Ok(())
}

async fn require_shredded_refuses_a_plaintext_value(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    let s = rec("visit", "v1", None);
    let pre = store
        .erase_values(&s, false, None, true)
        .await
        .must("a preflight")?;
    let n = pre.unshredded;
    ensure!(
        n > 0,
        "v1's plaintext values are listed in `unshredded`; it answered {pre:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let answer = store.erase_values(&s, true, None, false).await;
    refused_as(
        "an erase requiring its values shredded",
        answer,
        "`erase_not_shredded`",
        |s: &Said| matches!(s.case, Case::EraseNotShredded { count } if count == n),
    )?;
    head_unmoved(store.as_ref(), before, "a refused erase").await
}

async fn an_apart_key_is_destroyed(store: Store, given: Given) -> Held {
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let q = source_record("person", "q", json!({"id": "q", "name": "Quin", "zz": 1}));
    let opts = IngestOpts {
        sealed_apart: true,
        ..kept("mon")
    };
    fed(&l, &l.own, "crm", opts, vec![q])
        .await
        .must("quarantining q apart")?;
    let (lake, crm) = (store.lake().clone(), SourceId::from("crm"));
    let apart = keys.apart(&lake, &crm);
    ensure!(
        !apart.is_empty(),
        "the delivery sealed q apart under its own key"
    );
    let report = erase(
        store.as_ref(),
        &Selection::Quarantined(qid("crm", "mon", 0)),
    )
    .await?;
    ensure!(
        report.apart == apart,
        "the report names {apart:?} in `apart`; it answered {report:?}"
    );
    let left = keys.apart(&lake, &crm);
    ensure!(
        left.is_empty(),
        "the apart key is destroyed; the key store still holds {left:?}"
    );
    let base = given.scratch.clone();
    let _ = std::fs::remove_dir_all(&base);
    crate::export::write(store.as_ref(), &base.join("x"))
        .await
        .must("exporting")?;
    let imported = (given.import)(base.join("x"), lake, None, with(&keys)).await;
    ensure!(
        imported.is_ok(),
        "the export imports, its erased cells not read as lost keys; it answered {:?}",
        imported.err()
    );
    Ok(())
}

async fn remove_values_refuses_the_new_selections(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    let before = store.head().await.must("reading the head")?;
    let q = QuarantineId {
        source: "tracker".into(),
        delivery: "d".into(),
        index: 0,
    };
    for s in [rec("visit", "v1", None), Selection::Quarantined(q)] {
        let answer = store
            .remove_values(&"tracker".into(), &s, false, None)
            .await;
        refused_as(
            "`remove_values` of an erase's selection",
            answer,
            "`remove_erases`",
            |s: &Said| matches!(s.case, Case::RemoveErases),
        )?;
    }
    head_unmoved(store.as_ref(), before, "a refused removal").await
}

async fn a_dismiss_erases_with_the_setting_on(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    l.set(ERASE_ON_CLOSE_SETTING, json!(true)).await?;
    let id = quarantine_at(&l, "erp", "r1", "k1").await?;
    let before = store.head().await.must("reading the head")?;
    l.cat
        .dismiss_quarantined(&l.own, &id)
        .await
        .must("dismissing")?;
    let got = kinds(store.as_ref(), before).await?;
    ensure!(
        got.len() == 2 && closed(&got[0], ClosedWhy::Dismissed) && is_erase(&got[1]),
        "a dismiss appends its close and then `values_erased`; it appended {got:?}"
    );
    Ok(())
}

async fn a_landing_erases_only_with_the_setting_on(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    for (on, delivery, key) in [(true, "r1", "k1"), (false, "r2", "k2")] {
        l.set(ERASE_ON_CLOSE_SETTING, json!(on)).await?;
        quarantine_at(&l, "erp", delivery, key).await?;
        let before = store.head().await.must("reading the head")?;
        l.upsert("invoice", key, json!({"note": "seen"})).await?;
        let got = kinds(store.as_ref(), before).await?;
        let landed = got.iter().any(|b| closed(b, ClosedWhy::Landed));
        let erases = got.last().is_some_and(is_erase);
        ensure!(
            landed && erases == on && got.iter().filter(|b| is_erase(b)).count() == usize::from(on),
            "a landing with `erase_on_close` {on} closes `landed` and erases {}; it appended {got:?}",
            if on { "the copy last" } else { "nothing" }
        );
    }
    Ok(())
}

/// Each fixed field of v1 as `l` reads it at the head.
async fn read_v1(l: &Lake) -> Result<Value, Failure> {
    let r = found(l, "visit", "v1", None).await?;
    let of = |f: &str| json!([r.fields.get(f), r.meta.get(f).is_some_and(|m| m.erased)]);
    Ok(json!([
        of("id"),
        of("duration"),
        of("place"),
        of("note"),
        of("site")
    ]))
}

async fn export_import_and_reopen_answer_the_erased_values(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let here = read_v1(&l).await?;
    let base = given.scratch.clone();
    let _ = std::fs::remove_dir_all(&base);
    crate::export::write(store.as_ref(), &base.join("x"))
        .await
        .must("exporting")?;
    let lake = store.lake().clone();
    let imported = (given.import)(base.join("x"), lake, None, Seams::default()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing the export: {e:?}")))?;
    let there = read_v1(&Lake::of(imported.store.as_ref())).await?;
    ensure!(
        there == here,
        "the import reads v1 as {there}; the lake reads {here}"
    );
    drop(imported);
    let store = again(&given.reopen, store, Seams::default(), "after the erase").await?;
    let reopened = read_v1(&Lake::of(store.as_ref())).await?;
    ensure!(
        reopened == here,
        "the lake opened again reads v1 as {reopened}; it read {here}"
    );
    Ok(())
}

async fn an_export_with_values_erased_verifies(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let dir = given.scratch.join("x");
    let _ = std::fs::remove_dir_all(&given.scratch);
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting")?;
    let verified = crate::export::verify(&dir);
    ensure!(
        verified.is_ok(),
        "the export verifies; it answered {:?}",
        verified.err()
    );
    Ok(())
}

async fn the_report_names_the_sources(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    let report = erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    ensure!(
        report.sources == vec![SourceId::from("tracker")],
        "the report names `tracker` alone; it answered {:?}",
        report.sources
    );
    Ok(())
}

/// Each object of `v` of the kind `values_erased`, made a removal by `tracker`.
fn as_removed(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if m.get("kind") == Some(&json!("values_erased")) {
                m.insert("kind".into(), json!("values_removed"));
                m.insert("source".into(), json!("tracker"));
            }
            m.values_mut().for_each(as_removed);
        }
        Value::Array(a) => a.iter_mut().for_each(as_removed),
        _ => {}
    }
}

async fn a_values_removed_of_a_record_is_damage(store: Store, given: Given) -> Held {
    use super::export::{files, lines_of, rehashed, rewrite};
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    erase(store.as_ref(), &rec("visit", "v1", None)).await?;
    let dir = given.scratch.join("x");
    let _ = std::fs::remove_dir_all(&given.scratch);
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting")?;
    let file = files(&dir)?
        .pop()
        .ok_or_else(|| Failure::new("the export has a log file"))?;
    let mut lines = lines_of(&dir, &file)?;
    let last = lines.len() - 1;
    let text = String::from_utf8_lossy(&lines[last]).into_owned();
    ensure!(
        text.contains("values_erased"),
        "the export ends with the erase; it ends {text}"
    );
    lines[last] = rehashed(&lines[last], as_removed)?;
    rewrite(&dir, &file, &lines)?;
    let damaged = |e: Option<&Error>| {
        matches!(e.and_then(Error::situation),
            Some(Situation::Damaged { flaw: Flaw::RemovedSelection { selection }, .. }) if selection == "record")
    };
    let verified = crate::export::verify(&dir).err();
    ensure!(
        damaged(verified.as_ref()),
        "a verifier refuses it as damage; it answered {verified:?}"
    );
    let imported = (given.import)(dir, store.lake().clone(), None, Seams::default())
        .await
        .err();
    ensure!(
        damaged(imported.as_ref()),
        "an import refuses it as damage; it answered {imported:?}"
    );
    Ok(())
}

async fn an_erase_follows_the_records_keys_back(store: Store) -> Held {
    let l = super::rekey::lake(store.as_ref(), super::rekey::IBANS).await?;
    let before = store.head().await.must("reading the head")?;
    super::rekey::apply(store.as_ref()).await?;
    erase(store.as_ref(), &rec("account", "DE1", Some("holder"))).await?;
    let old = found(&l, "account", "1", Some(before)).await?;
    let now = found(&l, "account", "DE1", None).await?;
    ensure!(
        gone(&old, "holder") && gone(&now, "holder"),
        "`holder` reads erased at key 1 before the re-keying and at DE1 after; they read {old:?} and {now:?}"
    );
    Ok(())
}

async fn a_taken_key_ends_the_chain(store: Store) -> Held {
    let l = super::rekey::lake(store.as_ref(), super::rekey::IBANS).await?;
    super::rekey::apply(store.as_ref()).await?;
    let taker =
        json!({"iban": "1", "number": "9", "holder": "new", "kind": "giro", "meta": {"n": "9"}});
    l.upsert("account", "1", taker).await?;
    erase(store.as_ref(), &rec("account", "DE1", Some("holder"))).await?;
    let r = found(&l, "account", "1", None).await?;
    ensure!(
        r.fields.get("holder") == Some(&json!("new")),
        "the account that took key 1 still reads its `holder`; it reads {r:?}"
    );
    Ok(())
}

async fn a_sys_type_is_not_erased(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.set(ERASE_ON_CLOSE_SETTING, json!(true)).await?;
    let before = store.head().await.must("reading the head")?;
    for ty in ["sys.setting", "nowhere"] {
        let answer = store
            .erase_values(&rec(ty, ERASE_ON_CLOSE_SETTING, None), false, None, false)
            .await;
        ensure!(
            matches!(&answer, Err(Error::NotFound { .. })),
            "an erase of `{ty}` is refused as not found; it answered {answer:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "a refused erase").await
}

/// What a report counts, for comparison.
fn counts(r: &EraseReport) -> String {
    format!(
        "{:?} {:?} {} {:?} {}",
        r.types, r.fields, r.versions, r.kept, r.unshredded
    )
}

async fn a_preflight_appends_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    one(store.as_ref(), &l).await?;
    let s = rec("visit", "v1", None);
    let before = store.head().await.must("reading the head")?;
    let pre = store
        .erase_values(&s, false, None, true)
        .await
        .must("a preflight")?;
    head_unmoved(store.as_ref(), before, "a preflight").await?;
    let act = store
        .erase_values(&s, false, Some(&pre.report_id), false)
        .await;
    let act = act.must("erasing under the preflight's report id")?;
    ensure!(
        counts(&pre) == counts(&act),
        "the preflight counts {} and the act {}",
        counts(&pre),
        counts(&act)
    );
    Ok(())
}

async fn a_resubmission_erases_as_the_system(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(invoice()).await?;
    let who = holder(
        store.as_ref(),
        "fixer",
        json!({"default": "write", "ingest": true}),
    );
    for (on, delivery, key) in [(true, "r1", "k1"), (false, "r2", "k2")] {
        if on {
            l.set(ERASE_ON_CLOSE_SETTING, json!(true)).await?;
        } else {
            l.set(ERASE_ON_CLOSE_SETTING, json!(false)).await?;
        }
        let id = quarantine_at(&l, "erp", delivery, key).await?;
        let before = store.head().await.must("reading the head")?;
        let fix = vec![resubmitting(&id, inv(key, json!(1), None))];
        fed(&l, &who, "erp", kept(&format!("fix{key}")), fix)
            .await
            .must("resubmitting")?;
        let got = events_after(store.as_ref(), before).await?;
        let erases: Vec<_> = got.iter().filter(|e| is_erase(&e.body)).collect();
        let by_system = erases
            .iter()
            .all(|e| e.writer == "system:lakelands" && e.origin == Origin::System);
        ensure!(
            erases.len() == usize::from(on) && by_system,
            "a resubmission with `erase_on_close` {on} erases {} by the system; it appended {got:?}",
            if on { "once" } else { "nothing" }
        );
    }
    Ok(())
}
