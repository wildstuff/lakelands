//! Part of a source is taken back in one act
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): what it
//! wrote before a time, or in one delivery, reads as erased at every
//! position, a record the source made left holding nothing of another
//! hand is retired, and a record another hand also wrote keeps that
//! hand's values.

use lakelands_api::write::RemoveReport;
use lakelands_api::{DeliveryId, IngestOpts};

use crate::conformance::fixture::{get_at, source_record};
use crate::conformance::prelude::*;

check!(
    REMOVED_BEFORE = a_sources_values_before_a_time_read_as_erased_at_every_position,
    "065: `remove_values_preflight` of what `tracker` wrote before the first event of the second day names the events selected and the first position it does not reach, the records holding a value it takes, the values per field, the two records `tracker` alone wrote as retired, the one with an author's `note` as kept, and not one an author retired and another source made again with nothing but its key, the derived field and the kept view computed again, the record left breaking an admitting constraint over the taken field and one over the field derived from it, and the `tour` whose reference names a record it retires, said as left naming a retired one; `remove_values` with that report appends one `values_removed`, the two retirements and one `act_whole`, and names that `tour` as its preflight did; after it the record made again stays, a record `web` also wrote a value on before the cut is kept, not retired, and keeps `web`'s value, which is not `tracker`'s and so not taken, a reference `tracker` wrote stays, as a field that identifies a record, the kept view counts what is left, and the report says what is computed again; the kept record holds its `note` and no `duration` nor what is derived from it, its provenance saying `erased` and naming `tracker`, at the head and `as_of` before the act, the second day's records keep theirs, and a rebuild answers the same; a field that is not erasable is taken all the same; a report under an id whose head a write moved past is taken: a write elsewhere since leaves its id good, and a time past the year 9999 is refused; a repeat appends nothing, and a selection of nothing appends nothing; a field declared `required` that the act leaves empty is listed among the breaches, naming the field, and said, and the act is not refused for it, though the author's next write of that record is"
);

check!(
    REMOVED_DELIVERY = a_delivery_taken_back_reads_as_erased_and_a_later_one_stays,
    "065: `remove_values` of one delivery, by the caller's id or by the store's own, takes what its events wrote and nothing else: a record the same source wrote again in a later delivery keeps those values from that event on and reads as erased before it, a record only that delivery wrote is retired, a redelivery of the same values under another id appended nothing and so selects nothing, and a later delivery under the same id stays, until the act is asked again, which takes that delivery and only it; a set of deliveries is one act that takes each as its single act would, an id the log never held selecting nothing and the empty set nothing at all, appending nothing"
);

check!(
    REMOVED_SET = a_set_of_deliveries_counts_as_its_single_acts,
    "065 (twins): `tracker` delivers `mon`, writing `v1` and `v2`, and `tue`, writing `v1` again with a place and `v3`, which an author's note keeps; taking back the set in one lake and `mon` then `tue` in its twin leaves each record read alike; the set counts the events and values the single acts sum to, 4 events, `duration` 4 and `place` 1, a record reached by both once, 3 against 4, retires what the last single act leaves retired, `v1` and `v2`, and keeps `v3` alone, which the single acts count with `v1`, 1 against 2",
    given
);

check!(
    REMOVE_CRASH = a_removal_a_crash_stopped_is_finished_by_the_next_writers_open,
    "065, extended (lakelands#1485, 099): a removal stopped after its event, before its rebuild, is finished by the road beside the writer's next open, which rebuilds knowing the act and reclaims after the open has answered, and by the act's repeat on the same handle, each appending no second event; the values read as erased at every position after either; the act stopped so answers `act_unfinished` at the head it appended to",
    given
);

check!(
    REMOVED_TWINS = a_value_taken_from_a_hidden_field_is_told_as_one_never_written,
    "065 (reader's twins): in a lake where `tracker` also wrote `secret`, a field kept from a reader, and in its twin where it never did, the act takes the same records back; that reader is answered alike by `get` of each record at every position of the log, by `find` with its total at every position, filtered on `duration is null`, and by `describe`, while the owner reads `secret`'s provenance as erased in the one lake alone",
    given
);

check!(
    REMOVED_SHREDDED = a_version_whose_values_are_all_taken_is_destroyed_and_the_rest_redacted,
    "065: `crm` seals `p1` under its first version and `p2` and `p3` under its second; taking back what it wrote before `p3` destroys the first version, whose values it all selects, and not the second, which also seals `p3`: the preflight names the first version and counts `name` 2 redacted and 1 shredded, `city`, not erasable, 1 redacted, and 1 value of an erasable field redacted only; with `require_shredded` the act is refused naming that count and appends nothing; without it the act names the version in `values_removed`, the key store holds the second alone, the values read as erased at the head and before the act, `p3` keeps its own, `crm` seals on under the second, and a writer opened again, rebuilding with the first version gone, reads them erased, not lost; taking back all it wrote, `require_shredded` too, destroys the second, and `crm` writes on under a new version, before a reopen and after; once `crm` is forgotten, taking back all it wrote with `require_shredded` counts every value shredded",
    reopens
);

check!(
    REMOVED_IMPORT = a_shredding_act_imports_erased_and_destroys_a_backups_versions,
    "065: an export made after an act destroyed `crm`'s only version imports with the key store as it is now, the fold meeting the values that version sealed before it meets the act: `p1` reads its name erased and keeps its note, before the act and after; imported with a backup of the key store from before the act, the import finishes it: the backup holds the version no more, and the first read answers the name erased; imported with no key store it needs none for that version, while a value `crm` sealed after the act, under a version it left, is refused as unbound",
    given
);

check!(
    REMOVED_WHOLE_KEYS = a_key_store_that_destroys_only_whole_sources_shreds_nothing,
    "065: with a key store that destroys a source's key only whole, the preflight of taking back all `crm` wrote names no version and counts its erasable value redacted only, `require_shredded` is refused naming that count, and the act without it appends, destroys nothing and is not owed: the version stays, the lake opens again, and the value reads as erased",
    reopens
);

check!(
    REMOVED_COMPUTED = what_is_computed_from_a_taken_value_goes_with_it,
    "065: `tracker` writes `place` on the stops `s1` and `s2`, each kept by an author's note; `loud` is computed from `place` by the suite's backing, `vec` embedded from it, and the set `rome` holds the stops in Rome; taking back what `tracker` wrote before `s2` leaves `s1` with no `place`, `loud` nor `vec`, at the head and before the act, with no call made again, and `s1` out of `rome`, while `s2` keeps its own and stays in it; a rebuild answers the same",
    reopens
);

check!(
    REMOVED_REPLAYED = an_output_of_a_later_version_replays_after_an_earlier_one_is_destroyed,
    "065: `crm` seals `p0` and `p1`, both Ada, under its first version, `loud` declared over them so that one fill names the other's output by `output_at`, and `p2` under its second, `loud` computed from `name` by the suite's backing for each, `p0` and `p1` kept by an author's note; taking back what it wrote before `p2` destroys the first version: `p0` and `p1` hold no `loud`, the output named by `output_at` reads as erased, not lost, and `p2`'s output, sealed under the second version, is found again with no call made, after the act, after a reopen and after a rebuild. The versions are made by hand: no check sets the writer's clock, so `key_period`'s rotation is held by its unit tests",
    reopens
);

check!(
    REMOVE_CRASH_SHREDDED = a_shredding_act_a_crash_stopped_is_finished_at_each_step,
    "065, 099: an act that would destroy `crm`'s version, stopped after its event because the key store failed to destroy it, leaves the version, which seals nothing more: `crm`'s next sealed value on that handle is refused `key_missing`, appending nothing; a rebuild does not count the act done while the version is held, and finishes it, destroying the version; one stopped after the destroy, before its scoped restate's first page commits, took the version from its first turn, so `crm`'s next sealed value is taken under a new version while the act is owed, and the road beside the writer's next open runs its restate; each appends no second event, and the values read as erased before the act",
    given
);

check!(
    ACT_ERASES_IN_ITS_FIRST_TURN = an_acts_versions_seal_nothing_and_the_source_has_a_new_one,
    "099: a removal's erasure, and the destruction of the versions it takes whole, happen in its first turn: its appends whole in the log and only its restate owed, the log holds its `values_removed` and the key store no longer holds the version, and a write of the source while the act is owed seals under a version the act left, which is the source's new one, and reads its own values, and the act's repeat finishes its restate and answers its report; a forgetting's erasure is its first turn alone too: with its restate owed its keys are gone and its ingest is refused, so nothing is sealed under a destroyed key, and its repeat answers its report",
    given
);

check!(
    REMOVE_TAKES_A_DELIVERY_THAT_GREW =
        a_removal_takes_an_event_of_its_delivery_written_since_its_report,
    "099: a report names its selection through its position; an event of the delivery written after the report and before the act is taken in, and the act under the report's id answers two events, the record the later event wrote retired beside the first, both read as erased at every position"
);

check!(
    REMOVE_LEAVES_A_LATER_DELIVERY = a_removal_leaves_an_event_of_its_delivery_written_after_it,
    "099: with `part_events` 1, an event of the same delivery written after the act's first turn is not taken: the act answers the six events its selection held, retires the six records, and the seventh record keeps its duration and is not retired, at the head and after a rebuild",
    given
);

check!(
    REMOVE_WRITTEN_BEFORE_JUDGED_AT_ITS_FIRST_TURN =
        a_tail_event_before_the_time_is_taken_and_one_after_is_not,
    "099: a `WrittenBefore` act judges its tail at its first turn by the time: an event of the source written while the act walked, before the time, is taken and its record retired, while one written after the time stays with its value, whichever of the two the walk's view held, and the act answers one event"
);

check!(
    REMOVE_TAIL_WRITE_KEEPS_ITS_VERSION =
        an_unselected_tail_write_keeps_the_version_it_was_sealed_under,
    "099: an unselected write of the source, written while the act walked and sealed under the version the walk saw as wholly selected, keeps that version: the act names no version in `values_removed`, the key store holds the version, and the write's value reads, while the values it selected read as erased",
    given
);

check!(
    REMOVE_TAKEN_IN_UNDER_A_KEPT_VERSION_IS_NOT_SHREDDED =
        a_taken_value_under_a_kept_version_refuses_shredded_and_counts_unshredded,
    "099: a value the act takes that is sealed under a version another delivery keeps is redacted only: with `require_shredded` the act is refused `remove_not_shredded`, counting one, appending nothing and leaving the head where it was, and without it the act appends, names one `unshredded`, keeps the version, and the value reads as erased",
    given
);

check!(
    REMOVE_STALE_ON_ANOTHER_SOURCES_WRITE =
        a_report_whose_record_another_source_wrote_since_is_stale,
    "099: another source's write on a record the report named as retirable, since the report, makes the report stale: the act under its id is refused `remove_report_stale`, appending nothing and leaving the record live with the other source's value, and the preflight that follows names the record kept"
);

check!(
    ACTS_OF_ONE_SOURCE_OVERLAP = a_report_before_the_first_act_is_stale_only_on_the_overlap,
    "099: two acts of one source over deliveries under one version, the second overlapping the first: a report read before the first act is stale for the overlapping delivery, refused `remove_report_stale` and appending nothing, and taken for a disjoint one, whose act appends and retires what it alone emptied; the overlapping act run again folds the earlier selections, counts its own two events once alone, retires the record the two emptied, and destroys the version the acts together take whole, leaving every record with no value; a forgetting's report, read before those acts, selects every event of the source there, and the acts take what it selected, so the act under its id is refused `forget_report_stale`",
    given
);

/// `visit` keyed by `id`: `duration`, `place`, `secret` and the reference
/// `site` from a source,
/// `note` an author's, `twice` derived from `duration`, the admitting
/// constraints `timed` and `doubled`, over `duration` and `twice`, and the
/// kept view `by_duration`.
async fn visits(l: &Lake) -> Held {
    let text = json!({"kind": "text"});
    let number = json!({"kind": "number"});
    l.declare_json(
        json!({"declare": "type", "name": "site", "fields": [{"name": "name", "type": text}]}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "visit", "key": ["id"],
        "fields": [{"name": "id", "type": text, "authority": "source"},
                   {"name": "site", "type": {"kind": "reference", "target": "site"},
                    "authority": "source"},
                   {"name": "duration", "type": number, "authority": "source"},
                   {"name": "place", "type": text, "authority": "source"},
                   {"name": "secret", "type": number, "authority": "source"},
                   {"name": "note", "type": text},
                   {"name": "twice", "type": number, "authority": "derived",
                    "derived": "duration * 2"}],
        "constraints": [{"name": "timed", "holds": "duration > 0", "at_ingest": "admit"},
                        {"name": "doubled", "holds": "twice > 0", "at_ingest": "admit"}]}))
        .await?;
    l.declare(count_view("by_duration", "visit", "duration"))
        .await?;
    l.set("view_kept.by_duration", json!(true)).await?;
    Ok(())
}

/// `tracker`'s visits `keys`, each with `duration`.
async fn tracked(l: &Lake, visits: &[(&str, u32)]) -> Held {
    let records = (visits.iter())
        .map(|(k, d)| source_record("visit", k, json!({"id": k, "duration": d})))
        .collect();
    l.ingest("tracker", records)
        .await
        .must("ingesting from tracker")?;
    Ok(())
}

/// What a report names, for comparison.
fn shape(r: &RemoveReport) -> Value {
    json!({
        "events": r.events,
        "types": r.types.iter().map(|t| json!([t.ty, t.records])).collect::<Vec<_>>(),
        "fields": r.fields.iter().map(|f| json!([f.ty, f.field, f.redacted])).collect::<Vec<_>>(),
        "retired": r.retired.iter().map(|t| json!([t.ty, t.retired])).collect::<Vec<_>>(),
        "kept": r.kept.iter().map(|t| json!([t.ty, t.first])).collect::<Vec<_>>(),
        "recomputed": r.recomputed.iter().map(|f| json!(f.field)).collect::<Vec<_>>(),
        "views": r.views,
        "breaches": r.breaches.iter().map(|b| json!([b.constraint, b.sample_keys])).collect::<Vec<_>>(),
    })
}

/// What a report leaves naming a retired record: the type, the relation,
/// how many records, and the first of them as `<type> <key>`.
fn stranded(r: &RemoveReport) -> Vec<(&str, &str, u64, Vec<String>)> {
    let of = |r: &lakelands_api::write::PurgeReference| {
        (r.first.iter().map(|f| format!("{} {}", f.ty, f.key))).collect()
    };
    let all = r.references.iter();
    all.map(|r| (r.ty.as_str(), r.relation.as_str(), r.records, of(r)))
        .collect()
}

/// `key`'s fields of `ty` as of `at` (the head when `None`), with
/// provenance; `None` where it is not found.
pub(super) async fn at(
    l: &Lake,
    ty: &str,
    key: &str,
    at: Option<Position>,
) -> Result<Option<lakelands_api::Record>, Failure> {
    match l.cat.get(&l.own, &ty.into(), get_at(key, at)).await {
        Ok(r) => Ok(Some(r.data)),
        Err(lakelands_api::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(Failure::new(format!("reading `{key}` as of {at:?}: {e}"))),
    }
}

/// Whether `r` holds no value of `field` and its provenance says it was
/// erased, from `source`.
pub(super) fn erased(r: &lakelands_api::Record, field: &str, source: &str) -> bool {
    let m = r.meta.get(field);
    !r.fields.contains_key(field)
        && m.is_some_and(|m| m.erased && m.source.as_ref().is_some_and(|s| s == source))
}

/// The `values_removed` events after `after`.
pub(super) async fn acts(store: &dyn LakeStore, after: Position) -> Result<Vec<Event>, Failure> {
    let events = events_after(store, after).await?;
    let act = |e: &Event| matches!(e.body, EventBody::ValuesRemoved { .. });
    Ok(events.into_iter().filter(act).collect())
}

/// What the owner reads of `visit` `v1`..`v6` at `at`: each record's
/// fields, or none.
async fn read_all(l: &Lake, when: Option<Position>) -> Result<Vec<Value>, Failure> {
    let mut all = Vec::new();
    for k in ["v1", "v2", "v3", "v4", "v5", "v6"] {
        let r = at(l, "visit", k, when).await?;
        all.push(json!(r.map(|r| r.fields)));
    }
    Ok(all)
}

async fn a_sources_values_before_a_time_read_as_erased_at_every_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    l.upsert("site", "s1", json!({"name": "Harbour"})).await?;
    // `v3` names its site by a reference, which identifies a record and so
    // is never taken.
    let day1 = [("v0", 5), ("v1", 10), ("v2", 20), ("v3", 30), ("v7", 70)].map(|(k, d)| {
        let mut fields = json!({"id": k, "duration": d});
        if k == "v3" {
            fields["site"] = json!("s1");
        }
        source_record("visit", k, fields)
    });
    l.ingest("tracker", day1.to_vec())
        .await
        .must("ingesting the first day")?;
    l.upsert("visit", "v3", json!({"note": "late"})).await?;
    // A `tour` names `v1`, which the act retires: the reference is left
    // naming a retired record, and the report says so.
    l.declare_json(json!({"declare": "type", "name": "tour",
        "fields": [{"name": "stop", "type": {"kind": "reference", "target": "visit"}}],
        "relations": [{"name": "stops", "target": "visit", "via": "stop"}]}))
        .await?;
    l.upsert("tour", "t1", json!({"stop": "v1"})).await?;
    // `web` writes a value of its own on `v7`, before the cut: it is not
    // tracker's, so it is not taken, and it keeps the record.
    let v7 = source_record("visit", "v7", json!({"id": "v7", "place": "Oslo"}));
    l.ingest("web", vec![v7]).await.must("web writing v7")?;
    // `v0` retired, then made again by `web` with nothing but its key:
    // nothing of it is the first day's.
    l.retire("visit", "v0").await?;
    let v0 = source_record("visit", "v0", json!({"id": "v0"}));
    l.ingest("web", vec![v0])
        .await
        .must("making v0 again from web")?;
    // The second day begins after the first's last `known_at`.
    std::thread::sleep(std::time::Duration::from_millis(3));
    let first_day = store.head().await.must("reading the head")?;
    tracked(&l, &[("v4", 40), ("v5", 50), ("v6", 60)]).await?;
    let second = events_from(store.as_ref(), first_day.next()).await?;
    let day2 = second
        .first()
        .ok_or_else(|| Failure::new("the second day was logged"))?;
    let (cut, p) = (day2.known_at, day2.position);
    let tracker: SourceId = "tracker".into();
    let selection = Selection::WrittenBefore(cut);
    let report = store.remove_values_preflight(&tracker, &selection).await;
    let report = report.must("the preflight")?;
    let want = json!({
        "events": 5, "types": [["visit", 4]], "fields": [["visit", "duration", 5]],
        "retired": [["visit", 2]], "kept": [["visit", ["v3", "v7"]]], "recomputed": ["twice"],
        "views": ["by_duration"],
        "breaches": [["timed", ["v3", "v7"]], ["doubled", ["v3", "v7"]]],
    });
    ensure!(
        shape(&report) == want && report.before == Some(p),
        "the preflight names {want}, and position {p} as the first it does not reach; it \
         names {} in {report:?}",
        shape(&report)
    );
    let said = report.said();
    let says = |line: &str| said.iter().any(|s| s.starts_with(line));
    ensure!(
        says("5 values of `visit.duration`")
            && says("2 records of `visit` are retired")
            && says("2 records of `visit` stay")
            && says("`visit.twice` is computed again.")
            && says("The view `by_duration` is computed again.")
            && says("1 record of `tour` is left naming a retired one by `stops`."),
        "the report says in sentences what it takes, retires, keeps and computes again, and \
         what it leaves naming a retired record: {said:?}"
    );
    ensure!(
        stranded(&report) == [("tour", "stops", 1, vec!["tour t1".to_string()])],
        "the preflight names `t1`, whose `stops` names `v1`, as left naming a retired record: \
         {:?}",
        report.references
    );
    ensure!(
        report.log_events > 0 && !report.report_id.is_empty() && report.reclaimed.is_none(),
        "the preflight counts the log's events, carries an id and reclaims nothing: {report:?}"
    );
    // A write elsewhere since the report leaves its id good: the id names
    // the position the report was read at and the records it selected
    // (099), and the note is neither.
    l.upsert("visit", "v4", json!({"note": "moved"})).await?;
    let before = store.head().await.must("reading the head")?;
    let done = store
        .remove_values(&tracker, &selection, false, Some(&report.report_id))
        .await;
    let done = done.must("removing tracker's first day under its report's id")?;
    let events = events_after(store.as_ref(), before).await?;
    let kinds: Vec<&str> = events.iter().map(|e| e.body.kind()).collect();
    ensure!(
        kinds
            == [
                "values_removed",
                "record_retired",
                "record_retired",
                "act_whole"
            ]
            && shape(&done) == shape(&report)
            && stranded(&done) == stranded(&report)
            && done.reclaimed.is_some(),
        "the act appends `values_removed`, two retirements and `act_whole`, answers its \
         preflight's report and reclaims; it appended {kinds:?} and answered {done:?}"
    );
    // At the head and before the act, the first day reads as erased.
    kept_others(&l, before).await?;
    for when in [None, Some(before), Some(p)] {
        let v3 = at(&l, "visit", "v3", when)
            .await?
            .ok_or_else(|| Failure::new("v3 stays"))?;
        ensure!(
            erased(&v3, "duration", "tracker")
                && !v3.fields.contains_key("twice")
                && v3.fields.get("note") == Some(&json!("late")),
            "as of {when:?}, `v3` keeps its note and holds no `duration` nor `twice`, \
             `duration` saying it was erased from tracker: {v3:?}"
        );
        let v4 = at(&l, "visit", "v4", when).await?;
        ensure!(
            v4.as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(40))),
            "as of {when:?}, `v4` of the second day keeps its duration: {v4:?}"
        );
    }
    let v1 = at(&l, "visit", "v1", Some(before)).await?;
    ensure!(
        v1.as_ref()
            .is_some_and(|r| erased(r, "duration", "tracker")),
        "as of before the act, `v1` reads its duration as erased: {v1:?}"
    );
    let gone = (
        at(&l, "visit", "v1", None).await?,
        at(&l, "visit", "v2", None).await?,
    );
    let v0 = at(&l, "visit", "v0", None).await?;
    ensure!(
        gone.0.is_none() && gone.1.is_none() && v0.is_some(),
        "`v1` and `v2`, tracker's alone, are retired, and `v0`, made again by an author, \
         stays: {gone:?}, {v0:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let answers = (read_all(&l, None).await?, read_all(&l, Some(before)).await?);
    store.rebuild().await.must("rebuilding")?;
    let again = (read_all(&l, None).await?, read_all(&l, Some(before)).await?);
    ensure!(
        answers == again,
        "a rebuild answers as the act left the lake: {answers:?}, then {again:?}"
    );
    kept_others(&l, before).await?;
    // The act again appends nothing; so does a selection of nothing.
    let repeat = store.remove_values(&tracker, &selection, false, None).await;
    repeat.must("removing tracker's first day again")?;
    let none = Selection::WrittenBefore(Timestamp(0));
    refused_like(
        "removing what tracker wrote before a time past the year 9999",
        store
            .remove_values(
                &tracker,
                &Selection::WrittenBefore(Timestamp(i64::MAX)),
                false,
                None,
            )
            .await,
        &json!({"code": "not_date_time"}),
    )?;
    let empty = store.remove_values(&tracker, &none, false, None).await;
    let empty = empty.must("removing what tracker wrote before 1970")?;
    let after = store.head().await.must("reading the head")?;
    let removed = acts(store.as_ref(), before).await?.len();
    ensure!(
        after == head && empty.events == 0 && removed == 1,
        "a repeat and a selection of nothing append nothing: the head moved from {head} to \
         {after}, {removed} acts, the empty selection answered {empty:?}"
    );
    // A field declared `required` the act leaves empty is listed, not
    // refused; the author's next write is then refused for it.
    l.declare_json(json!({"declare": "type", "name": "badge", "key": ["id"],
        "fields": [{"name": "id", "type": {"kind": "text"}, "authority": "source"},
                   {"name": "level", "type": {"kind": "number"}, "authority": "source",
                    "required": true},
                   {"name": "note", "type": {"kind": "text"}}]}))
        .await?;
    let b1 = source_record("badge", "b1", json!({"id": "b1", "level": 3}));
    let opts = IngestOpts {
        delivery: Some(DeliveryId::from("badges")),
        ..IngestOpts::default()
    };
    let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &tracker, opts, vec![b1]);
    said.await.must("ingesting b1")?;
    l.upsert("badge", "b1", json!({"note": "kept"})).await?;
    let all = Selection::Delivery("badges".into());
    let report = store.remove_values_preflight(&tracker, &all).await;
    let report = report.must("the preflight of b1's level")?;
    let required: Vec<_> = (report.breaches.iter())
        .map(|b| {
            (
                b.ty.as_str(),
                b.constraint.as_str(),
                b.field.as_ref().map(|f| f.as_str()),
            )
        })
        .collect();
    let said = report.said();
    ensure!(
        required == [("badge", "required", Some("level"))]
            && report.breaches[0].sample_keys == [lakelands_api::RecordKey::from("b1")]
            && said
                .iter()
                .any(|s| s.starts_with("1 record of `badge` is left without `level`")),
        "the preflight lists `level`, required, left empty on `b1`, and says so: {:?}, {said:?}",
        report.breaches
    );
    let done = store.remove_values(&tracker, &all, false, None).await;
    let done = done.must("removing b1's level")?;
    let refused = l
        .cat
        .upsert(
            &l.own,
            &"badge".into(),
            &"b1".into(),
            crate::conformance::fixture::patch(json!({"note": "again"})),
            Default::default(),
        )
        .await;
    ensure!(
        done.breaches == report.breaches
            && matches!(refused, Err(lakelands_api::Error::Refused { .. })),
        "the act lists what its preflight listed, and the author's next write of `b1` is refused \
         for the empty `level`: {:?}, {refused:?}",
        done.breaches
    );
    Ok(())
}

async fn a_delivery_taken_back_reads_as_erased_and_a_later_one_stays(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let tracker: SourceId = "tracker".into();
    let deliver = async |id: Option<&str>, visits: &[(&str, Value)]| {
        let opts = IngestOpts {
            delivery: id.map(DeliveryId::from),
            ..IngestOpts::default()
        };
        let records = (visits.iter())
            .map(|(k, f)| source_record("visit", k, f.clone()))
            .collect();
        let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &tracker, opts, records);
        let windows = said.await.must("ingesting a delivery")?;
        let first = windows.first().ok_or_else(|| Failure::new("a window"))?;
        Ok::<_, Failure>((first.position, first.delivery.clone()))
    };
    let v = |k: &str, d: u32| json!({"id": k, "duration": d});
    let (b1, _) = deliver(Some("b1"), &[("v1", v("v1", 10)), ("v2", v("v2", 20))]).await?;
    // `v1` again, its duration unchanged and a place beside it: the event
    // carries the duration too.
    let v1 = json!({"id": "v1", "duration": 10, "place": "Rome"});
    let (b2, _) = deliver(Some("b2"), &[("v1", v1), ("v3", v("v3", 30))]).await?;
    let (_, own) = deliver(None, &[("v4", v("v4", 40))]).await?;
    // The same values under another id append nothing.
    let head = store.head().await.must("reading the head")?;
    deliver(Some("b3"), &[("v2", v("v2", 20))]).await?;
    let b3 = Selection::Delivery("b3".into());
    let none = store
        .remove_values(&tracker, &b3, false, None)
        .await
        .must("removing b3")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        none.events == 0 && after == head,
        "the redelivery under `b3` appended nothing, so its removal selects nothing and \
         appends nothing: {none:?}, the head moved from {head} to {after}"
    );
    let own = own.ok_or_else(|| Failure::new("the store named its delivery"))?;
    let theirs = Selection::Delivery(own.clone());
    let report = store.remove_values(&tracker, &theirs, false, None).await;
    let report = report.must("removing the store's own delivery")?;
    let v4 = at(&l, "visit", "v4", None).await?;
    ensure!(
        report.events == 1 && v4.is_none(),
        "the delivery the store named `{own}` is taken back by that id, and `v4`, which it \
         alone wrote, is retired: {report:?}, {v4:?}"
    );
    let b1_sel = Selection::Delivery("b1".into());
    let report = store
        .remove_values(&tracker, &b1_sel, false, None)
        .await
        .must("removing b1")?;
    let (v1_then, v1_now) = (
        at(&l, "visit", "v1", Some(b1)).await?,
        at(&l, "visit", "v1", None).await?,
    );
    let v1_b2 = at(&l, "visit", "v1", Some(b2)).await?;
    let v2 = at(&l, "visit", "v2", None).await?;
    ensure!(
        report.events == 2
            && v1_then
                .as_ref()
                .is_some_and(|r| erased(r, "duration", "tracker"))
            && [&v1_b2, &v1_now].iter().all(|r| {
                r.as_ref()
                    .is_some_and(|r| r.fields.get("duration") == Some(&json!(10)))
            })
            && v2.is_none(),
        "taking back `b1` selects its two events: `v1` reads its duration as erased at \
         `b1`'s position and holds `b2`'s from there on, and `v2`, which only `b1` wrote, is \
         retired: {report:?}; `v1` at b1 {v1_then:?}, at b2 {v1_b2:?}, now {v1_now:?}; \
         `v2` {v2:?}"
    );
    // A delivery under the same id after the act is not taken back by it.
    deliver(Some("b1"), &[("v5", v("v5", 50))]).await?;
    store.rebuild().await.must("rebuilding")?;
    let v5 = at(&l, "visit", "v5", None).await?;
    ensure!(
        v5.as_ref()
            .is_some_and(|r| r.fields.get("duration") == Some(&json!(50))),
        "a delivery under `b1` after its removal stays, a rebuild after it too: {v5:?}"
    );
    // Taking `b1` back again takes that later delivery, and only it.
    let again = store.remove_values(&tracker, &b1_sel, false, None).await;
    let again = again.must("removing b1 again")?;
    let v5 = at(&l, "visit", "v5", None).await?;
    ensure!(
        again.events == 1 && v5.is_none(),
        "taking `b1` back again selects the one later event, and retires `v5`: {again:?}, \
         {v5:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let empty = Selection::Deliveries(vec![]);
    let said = store.remove_values_preflight(&tracker, &empty).await;
    let said = said.must("the preflight of the empty set")?;
    let done = store.remove_values(&tracker, &empty, false, None).await;
    let done = done.must("removing the empty set")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        said.events == 0 && done.events == 0 && after == head,
        "the empty set selects nothing and appends nothing: {said:?}, {done:?}, the head moved \
         from {head} to {after}"
    );
    // A set of deliveries is one act: an id the log never held selects
    // nothing, and the rest are taken as their single acts would take them.
    let (b4, _) = deliver(Some("b4"), &[("v6", v("v6", 60))]).await?;
    let set = Selection::Deliveries(["b2", "never", "b4"].map(DeliveryId::from).to_vec());
    let said = store.remove_values_preflight(&tracker, &set).await;
    let said = said.must("the preflight of b2, never and b4")?;
    let report = store.remove_values(&tracker, &set, false, None).await;
    let report = report.must("removing b2, never and b4")?;
    let left = (
        read_all(&l, None).await?,
        acts(store.as_ref(), b4).await?.len(),
    );
    ensure!(
        said.events == 3 && report.events == 3 && left == (vec![json!(null); 6], 1),
        "the set selects `b2`'s two events and `b4`'s one in one act, and retires `v1`, `v3` and \
         `v6`: the preflight {said:?}, the act {report:?}, then every visit and the acts after \
         `b4` {left:?}"
    );
    Ok(())
}

async fn a_set_of_deliveries_counts_as_its_single_acts(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    store.head().await.must("reading the head")?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let tracker: SourceId = "tracker".into();
    let v = |k: &str, f: Value| source_record("visit", k, f);
    for s in [&store, &twin] {
        let l = Lake::of(s.as_ref());
        visits(&l).await?;
        let mon = vec![
            v("v1", json!({"id": "v1", "duration": 10})),
            v("v2", json!({"id": "v2", "duration": 20})),
        ];
        let tue = vec![
            v("v1", json!({"id": "v1", "duration": 10, "place": "Rome"})),
            v("v3", json!({"id": "v3", "duration": 30})),
        ];
        for (id, records) in [("mon", mon), ("tue", tue)] {
            let opts = IngestOpts {
                delivery: Some(id.into()),
                ..IngestOpts::default()
            };
            let said = lakelands_api::ingest_all_with(&*l.cat, &l.own, &tracker, opts, records);
            said.await.must("ingesting a delivery")?;
        }
        l.upsert("visit", "v3", json!({"note": "seen"})).await?;
    }
    let set = Selection::Deliveries(["mon", "tue"].map(DeliveryId::from).to_vec());
    let whole = store.remove_values(&tracker, &set, false, None).await;
    let whole = [whole.must("removing the set")?];
    let mut singles = Vec::new();
    for d in ["mon", "tue"] {
        let one = Selection::Delivery(d.into());
        let done = twin.remove_values(&tracker, &one, false, None).await;
        singles.push(done.must("removing one delivery")?);
    }
    // What the reports count, summed.
    let tally = |rs: &[RemoveReport]| {
        let mut values = std::collections::BTreeMap::<&str, u64>::new();
        for f in rs.iter().flat_map(|r| &r.fields) {
            *values.entry(f.field.as_str()).or_default() += f.redacted;
        }
        let sum = |n: fn(&RemoveReport) -> u64| rs.iter().map(n).sum::<u64>();
        json!({
            "events": sum(|r| r.events),
            "values": values,
            "records": sum(|r| r.types.iter().map(|t| t.records).sum()),
            "retired": sum(|r| r.retired.iter().map(|t| u64::from(t.retired)).sum()),
            "kept": sum(|r| r.kept.iter().map(|t| t.records).sum()),
        })
    };
    let counts = |records: u64, kept: u64| {
        json!({"events": 4, "values": {"duration": 4, "place": 1}, "records": records,
               "retired": 2, "kept": kept})
    };
    let (one, two) = (
        read_all(&Lake::of(store.as_ref()), None).await?,
        read_all(&Lake::of(twin.as_ref()), None).await?,
    );
    let (set, acts) = (tally(&whole), tally(&singles));
    ensure!(
        one == two && set == counts(3, 1) && acts == counts(4, 2),
        "the set leaves each record as its single acts leave it, and counts a record they both \
         reach once: the set {set}, the single acts {acts}; read {one:?} and {two:?}"
    );
    Ok(())
}

async fn a_removal_a_crash_stopped_is_finished_by_the_next_writers_open(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use std::sync::atomic::Ordering;
    let fails: super::forgotten::Fails = std::sync::Arc::default();
    let wrap = super::forgotten::failing(&fails);
    let fault = async |store: Store| {
        let open = (given.fault)(store, Default::default(), wrap.clone()).await;
        open.map_err(|e| Failure::new(format!("the writer's open was refused: {e}")))
    };
    let mut store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    tracked(&l, &[("v1", 10), ("v2", 20)]).await?;
    l.upsert("visit", "v2", json!({"note": "kept"})).await?;
    let start = store.head().await.must("reading the head")?;
    let tracker: SourceId = "tracker".into();
    // The act's rebuild fails; the writer's next open finishes it.
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&tracker, &all, false, None).await;
    ensure!(
        stopped.is_err(),
        "the removal whose rebuild fails answers {stopped:?}"
    );
    store = fault(store).await?;
    let finished = async |store: &Store, key: &str, gone: &str, n: usize, by: &str| {
        let l = Lake::of(store.as_ref());
        let kept = at(&l, "visit", key, Some(start)).await?;
        let gone = at(&l, "visit", gone, None).await?;
        let removed = acts(store.as_ref(), start).await?.len();
        ensure!(
            kept.as_ref()
                .is_some_and(|r| erased(r, "duration", "tracker"))
                && gone.is_none()
                && removed == n,
            "{by} finishes the removal: `{key}` reads its duration as erased before the act, \
             the record tracker alone wrote is retired, and the log holds {n} acts: {kept:?}, \
             {gone:?}, {removed} acts"
        );
        Ok::<_, Failure>(())
    };
    // The open leaves the act to the road beside it: poll the log,
    // bounded, for its settle before any verb is called.
    settles(6_000, Duration::from_millis(10), || {
        finished(
            &store,
            "v2",
            "v1",
            1,
            "the road beside the writer's next open",
        )
    })
    .await?;
    // Again, finished by the act's repeat on the same handle.
    let l = Lake::of(store.as_ref());
    tracked(&l, &[("v3", 30), ("v4", 40)]).await?;
    l.upsert("visit", "v3", json!({"note": "kept"})).await?;
    let again = Selection::WrittenBefore(Timestamp::from_date(9998, 1, 1).expect("a date"));
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&tracker, &again, false, None).await;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        super::forgotten::unfinished(&stopped, head),
        "the second removal whose rebuild fails answers that it is in the log through the \
         head, {head}, and unfinished: {stopped:?}"
    );
    let repeat = store.remove_values(&tracker, &again, false, None).await;
    repeat.must("repeating the act")?;
    finished(&store, "v2", "v4", 2, "the act's repeat").await?;
    let v3 = at(&l, "visit", "v3", None).await?;
    ensure!(
        v3.as_ref()
            .is_some_and(|r| erased(r, "duration", "tracker")),
        "the repeat erases `v3`'s duration too: {v3:?}"
    );
    Ok(())
}

async fn a_value_taken_from_a_hidden_field_is_told_as_one_never_written(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use lakelands_api::{Find, Found};
    // The lake first: a store that answers nothing says so before a twin
    // is asked for.
    store.head().await.must("reading the head")?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let reader = super::only::holder(
        store.as_ref(),
        "reader",
        json!({"default": "read", "types": {"visit": {"fields": {"secret": "none"}}}}),
    );
    let tracker: SourceId = "tracker".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    // The one lake's `v1` carries `secret`; the twin's never does.
    for (s, secret) in [(&store, true), (&twin, false)] {
        let l = Lake::of(s.as_ref());
        visits(&l).await?;
        let mut v1 = json!({"id": "v1", "duration": 10});
        if secret {
            v1["secret"] = json!(5);
        }
        let records = vec![
            source_record("visit", "v1", v1),
            source_record("visit", "v2", json!({"id": "v2", "duration": 20})),
        ];
        l.ingest("tracker", records)
            .await
            .must("ingesting from tracker")?;
        l.upsert("visit", "v2", json!({"note": "kept"})).await?;
        s.remove_values(&tracker, &all, false, None)
            .await
            .must("removing tracker's values")?;
    }
    let told = async |s: &Store| {
        let l = Lake::of(s.as_ref());
        let head = s.head().await.must("reading the head")?;
        let mut said = Vec::new();
        for p in 0..=head.0 {
            let at_p = Some(Position(p));
            for k in ["v1", "v2"] {
                let r = l.cat.get(&reader, &"visit".into(), get_at(k, at_p)).await;
                said.push(format!("{:?}", r.map(|r| r.data)));
            }
            for filter in [None, Some("duration is null")] {
                let find = Find {
                    total: true,
                    filter: filter.map(Into::into),
                    ..crate::conformance::fixture::find_at(at_p)
                };
                let found = l.cat.find(&reader, &"visit".into(), find).await;
                said.push(format!(
                    "{:?}",
                    found.map(|f| match f.data {
                        Found::Records { rows, total, .. } => format!("{rows:?} {total:?}"),
                        other => format!("{other:?}"),
                    })
                ));
            }
        }
        let d = l.cat.describe(&reader, Default::default()).await;
        said.push(format!("{:?}", d.map(|d| d.data.types)));
        Ok::<_, Failure>(said)
    };
    // Each lake has its own clock: the times are left out.
    let (here, there) = (
        timeless(&told(&store).await?),
        timeless(&told(&twin).await?),
    );
    let apart: Vec<_> = (here.iter().zip(&there))
        .filter(|(a, b)| a != b)
        .take(3)
        .collect();
    ensure!(
        here.len() == there.len() && apart.is_empty(),
        "a reader kept from `secret` is answered alike in the lake where tracker wrote it and in \
         its twin; apart: {apart:?}"
    );
    // Before the act, the act and `v1`'s retirement.
    let owner = async |s: &Store| {
        let l = Lake::of(s.as_ref());
        let head = s.head().await.must("reading the head")?;
        let r = at(&l, "visit", "v1", Some(Position(head.0 - 2))).await?;
        Ok::<_, Failure>(r.is_some_and(|r| r.meta.get("secret").is_some_and(|m| m.erased)))
    };
    let (marked, unmarked) = (owner(&store).await?, owner(&twin).await?);
    ensure!(
        marked && !unmarked,
        "the owner reads `secret` as erased in the lake where tracker wrote it, and not in its \
         twin: {marked}, {unmarked}"
    );
    Ok(())
}

/// What the act of [`REMOVED_BEFORE`] leaves of others' and of what no
/// selection takes, at the head and as of `before` it: `web`'s `place` on
/// `v7`, which keeps the record; `v3`'s reference to its site; and in the
/// kept view, the durations of the second day alone.
async fn kept_others(l: &Lake, before: Position) -> Held {
    for when in [None, Some(before)] {
        let v7 = at(l, "visit", "v7", when).await?;
        let v3 = at(l, "visit", "v3", when).await?;
        ensure!(
            v7.as_ref()
                .is_some_and(|r| r.fields.get("place") == Some(&json!("Oslo"))
                    && erased(r, "duration", "tracker"))
                && v3
                    .as_ref()
                    .is_some_and(|r| r.fields.get("site") == Some(&json!("s1"))),
            "as of {when:?}, `v7` keeps web's place, its own duration erased, and `v3` its \
             reference to `s1`: {v7:?}, {v3:?}"
        );
    }
    let groups = l
        .all_found("by_duration", lakelands_api::Find::default())
        .await?;
    let mut durations: Vec<Value> = (groups.iter())
        .filter_map(|r| r.fields.get("duration").cloned())
        .collect();
    durations.sort_by_key(Value::to_string);
    ensure!(
        durations == [json!(40), json!(50), json!(60)],
        "the kept view counts the second day's durations alone: {groups:?}"
    );
    Ok(())
}

/// `said` with every `known_at: Timestamp(…)` left out.
fn timeless(said: &[String]) -> Vec<String> {
    said.iter()
        .map(|s| {
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find("known_at: Timestamp(") {
                out.push_str(&rest[..i]);
                rest = &rest[i..];
                rest = rest.find(')').map_or("", |j| &rest[j + 1..]);
            }
            out.push_str(rest);
            out
        })
        .collect()
}

async fn a_version_whose_values_are_all_taken_is_destroyed_and_the_rest_redacted(
    store: Store,
    reopen: crate::conformance::Reopen,
) -> Held {
    use super::erasable::{again, person, with};
    use lakelands_api::seam::KeyStore;

    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    let ada = source_record(
        "person",
        "p1",
        json!({"id": "p1", "name": "Ada", "city": "Rome"}),
    );
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    let v1 = super::erasable::first(&keys, store.as_ref(), "crm")?;
    let v2 = keys
        .create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![p("p2", "Bea")])
        .await
        .must("ingesting p2")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mark = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p3", "Cy")])
        .await
        .must("ingesting p3")?;
    let after = events_from(store.as_ref(), mark.next()).await?;
    let cut = after
        .first()
        .ok_or_else(|| Failure::new("p3 was logged"))?
        .known_at;
    let selection = Selection::WrittenBefore(cut);
    let report = store.remove_values_preflight(&crm, &selection).await;
    let report = report.must("the preflight")?;
    let fields: Vec<_> = (report.fields.iter())
        .map(|f| (f.field.as_str(), f.redacted, f.shredded))
        .collect();
    ensure!(
        report.keys == [v1.clone()]
            && fields == [("city", 1, 0), ("name", 2, 1)]
            && report.unshredded == 1,
        "the preflight destroys {v1} alone, counts `city` 1 redacted, `name` 2 redacted and 1 \
         shredded, and 1 value of an erasable field redacted only: {report:?}"
    );
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "removing with `require_shredded` what would be redacted only",
        store.remove_values(&crm, &selection, true, None).await,
        &json!({"code": "remove_not_shredded", "source": "crm", "count": 1}),
    )?;
    let unmoved = store.head().await.must("reading the head")?;
    ensure!(unmoved == head, "the refused act appended up to {unmoved}");
    store
        .remove_values(&crm, &selection, false, None)
        .await
        .must("removing what crm wrote before p3")?;
    let named: Vec<_> = (acts(store.as_ref(), head).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ValuesRemoved { keys, .. } => Some(keys),
            _ => None,
        })
        .collect();
    let left = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        named == [vec![v1.clone()]] && left == [v2.clone()],
        "`values_removed` names {v1}, and the key store holds {v2} alone: {named:?}, {left:?}"
    );
    // Read now, before the act, and by a writer opened again.
    let erased_all = async |l: &Lake, by: &str| {
        for when in [None, Some(head)] {
            let p2 = at(l, "person", "p2", when).await?;
            let p3 = at(l, "person", "p3", when).await?;
            ensure!(
                p2.as_ref()
                    .is_some_and(|r| erased(r, "name", "crm")
                        && r.fields.get("note") == Some(&json!("kept")))
                    && p3
                        .as_ref()
                        .is_some_and(|r| r.fields.get("name") == Some(&json!("Cy"))),
                "{by}, as of {when:?}, `p2` reads its name as erased and keeps its note, and `p3` \
                 reads Cy: {p2:?}, {p3:?}"
            );
        }
        let p1 = at(l, "person", "p1", Some(head)).await?;
        ensure!(
            p1.as_ref()
                .is_some_and(|r| erased(r, "name", "crm") && erased(r, "city", "crm")),
            "{by}, before the act `p1` reads its name and city as erased: {p1:?}"
        );
        Ok::<_, Failure>(())
    };
    erased_all(&l, "after the act").await?;
    let before = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p4", "Dee")])
        .await
        .must("ingesting p4")?;
    let sealed: Vec<_> = (events_after(store.as_ref(), before).await?.into_iter())
        .filter_map(|e| e.sealed.map(|s| s.key))
        .collect();
    ensure!(
        sealed == [v2.clone()],
        "crm seals p4 under {v2}: {sealed:?}"
    );
    let store = again(&reopen, store, with(&keys), "with the first version gone").await?;
    store
        .rebuild()
        .await
        .must("rebuilding with the first version gone")?;
    erased_all(&Lake::of(store.as_ref()), "a writer opened again").await?;
    // All crm wrote taken back: its newest version goes too, and it seals
    // on under a new one, on this handle and on the next.
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let done = store.remove_values(&crm, &all, true, None).await;
    let done = done.must("removing all crm wrote")?;
    let left = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        done.keys == [v2.clone()] && left.is_empty(),
        "taking back all crm wrote destroys {v2}, and the key store holds none: {done:?}, {left:?}"
    );
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![p("p5", "Eve")])
        .await
        .must("ingesting p5")?;
    let store = again(&reopen, store, with(&keys), "after crm's last version went").await?;
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![p("p6", "Fay")])
        .await
        .must("ingesting p6 after a reopen")?;
    store.rebuild().await.must("rebuilding")?;
    let names = (
        at(&l, "person", "p5", None).await?,
        at(&l, "person", "p6", None).await?,
    );
    let named = |r: &Option<lakelands_api::Record>, n: &str| {
        r.as_ref()
            .is_some_and(|r| r.fields.get("name") == Some(&json!(n)))
    };
    ensure!(
        named(&names.0, "Eve") && named(&names.1, "Fay"),
        "crm writes on after its last version went, before a reopen and after, and a rebuild \
         reads `p5` Eve and `p6` Fay: {names:?}"
    );
    // Once crm is forgotten, what it sealed is shredded with its keys.
    store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm")?;
    let done = store.remove_values(&crm, &all, true, None).await;
    let done = done.must("removing all crm wrote, forgotten, with `require_shredded`")?;
    ensure!(
        done.unshredded == 0 && done.keys.is_empty(),
        "a forgotten source's values count as shredded, its keys gone with it: {done:?}"
    );
    Ok(())
}

async fn a_shredding_act_imports_erased_and_destroys_a_backups_versions(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use super::erasable::{person, with};
    use lakelands_api::seam::KeyStore;
    use std::sync::Arc;
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let backup = Arc::new(keys.backup());
    let crm: SourceId = "crm".into();
    let before = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let done = store.remove_values(&crm, &all, false, None).await;
    let done = done.must("removing all crm wrote")?;
    ensure!(
        done.keys.len() == 1,
        "the act destroys crm's one version: {done:?}"
    );
    let base = given.scratch.clone();
    let _ = std::fs::remove_dir_all(&base);
    crate::export::write(store.as_ref(), &base)
        .await
        .must("exporting")?;
    let lake = store.lake().clone();
    let import = |seams| (given.import)(base.clone(), lake.clone(), None, seams);
    let read = async |s: &Store| {
        let l = Lake::of(s.as_ref());
        let mut said = Vec::new();
        for when in [Some(before), None] {
            said.push(at(&l, "person", "p1", when).await?);
        }
        Ok::<_, Failure>(said)
    };
    let now = import(with(&keys)).await;
    let now = now.map_err(|e| Failure::new(format!("importing with the key store: {e}")))?;
    let p1 = read(&now.store).await?;
    ensure!(
        p1.iter().all(|r| r.as_ref().is_some_and(
            |r| erased(r, "name", "crm") && r.fields.get("note") == Some(&json!("kept"))
        )),
        "imported with the key store as it is now, `p1` reads its name erased and keeps its \
         note, before the act and after: {p1:?}"
    );
    let restored = import(with(&backup)).await;
    let restored = restored.map_err(|e| Failure::new(format!("importing with the backup: {e}")))?;
    let restored = (restored.writer)(restored.store, with(&backup)).await;
    let restored = restored.map_err(|e| Failure::new(format!("opening the import: {e}")))?;
    let p1 = read(&restored).await?;
    let held = backup.ids(store.lake(), &crm).ok().unwrap_or_default();
    ensure!(
        held.is_empty()
            && p1
                .iter()
                .all(|r| r.as_ref().is_some_and(|r| erased(r, "name", "crm"))),
        "imported with a backup from before the act, the import destroys its version, and \
         `p1` reads its name erased: {held:?}, {p1:?}"
    );
    let keyless = import(Default::default()).await.map(drop);
    ensure!(
        keyless.is_ok(),
        "imported with no key store, the version the act destroyed needs no key: {keyless:?}"
    );
    // A value sealed under a version the act did not destroy needs its key.
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    l.ingest("crm", vec![bob]).await.must("ingesting p2")?;
    let _ = std::fs::remove_dir_all(&base);
    crate::export::write(store.as_ref(), &base)
        .await
        .must("exporting again")?;
    let live = import(Default::default()).await.map(drop);
    ensure!(
        matches!(live, Err(lakelands_api::Error::Unbound { .. })),
        "imported with no key store, a value under a version the act left is refused as \
         unbound: {live:?}"
    );
    Ok(())
}

/// A key store that destroys a source's key only whole: `destroy_version`
/// is the seam's default.
pub(super) struct Whole(pub(super) std::sync::Arc<super::erasable::Keys>);

impl lakelands_api::seam::KeyStore for Whole {
    fn create(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<lakelands_api::id::KeyId> {
        self.0.create(lake, source)
    }
    fn key(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &lakelands_api::id::KeyId,
    ) -> lakelands_api::Result<Option<lakelands_api::seam::DataKey>> {
        self.0.key(lake, source, id)
    }
    fn destroy(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<()> {
        self.0.destroy(lake, source)
    }
    fn ids(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<Vec<lakelands_api::id::KeyId>> {
        self.0.ids(lake, source)
    }
}

async fn a_key_store_that_destroys_only_whole_sources_shreds_nothing(
    store: Store,
    reopen: crate::conformance::Reopen,
) -> Held {
    use super::erasable::{Keys, again, person};
    use lakelands_api::seam::{KeyStore, Seams};
    use std::sync::Arc;
    let keys = Arc::new(Keys::default());
    let seams = || Seams {
        keys: Some(Arc::new(Whole(keys.clone()))),
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    };
    let store = again(&reopen, store, seams(), "with a whole key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let crm: SourceId = "crm".into();
    let before = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let report = store.remove_values_preflight(&crm, &all).await;
    let report = report.must("the preflight")?;
    ensure!(
        report.keys.is_empty() && report.unshredded == 1,
        "the preflight names no version and 1 value redacted only: {report:?}"
    );
    refused_like(
        "removing with `require_shredded` where no version can be destroyed",
        store.remove_values(&crm, &all, true, None).await,
        &json!({"code": "remove_not_shredded", "source": "crm", "count": 1}),
    )?;
    store
        .remove_values(&crm, &all, false, None)
        .await
        .must("removing all crm wrote")?;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    let store = again(&reopen, store, seams(), "after the act").await?;
    let p1 = at(&Lake::of(store.as_ref()), "person", "p1", Some(before)).await?;
    ensure!(
        held.len() == 1 && p1.as_ref().is_some_and(|r| erased(r, "name", "crm")),
        "the act destroys nothing, the lake opens again, and `p1` reads its name erased: \
         {held:?}, {p1:?}"
    );
    Ok(())
}

async fn what_is_computed_from_a_taken_value_goes_with_it(
    store: Store,
    reopen: crate::conformance::Reopen,
) -> Held {
    use crate::conformance::backing::{self, EMBED_MODEL};
    use crate::conformance::fixture::{find_at, object_set_on};
    const TOKEN: &str = "removed_computed";
    let seams = super::embeddings::with_embedder(Some(backing::embedder()));
    let store = super::erasable::again(&reopen, store, seams, "with the suite's embedder").await?;
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": backing::KIND,
        "inputs": [{"name": "place", "type": text}],
        "config": {"token": TOKEN, "echo": "place"}}),
    )
    .await?;
    l.declare_json(json!({"declare": "type", "name": "stop", "key": ["id"],
        "fields": [{"name": "id", "type": text, "authority": "source"},
                   {"name": "place", "type": text, "authority": "source"},
                   {"name": "note", "type": text},
                   {"name": "loud", "type": text, "authority": "derived",
                    "function": "shout", "depends_on": ["place"]},
                   {"name": "vec", "type": {"kind": "vector", "dimensions": 2},
                    "authority": "derived", "embed": {"from": ["place"], "model": EMBED_MODEL}}]}))
        .await?;
    l.declare_as(object_set_on("rome", "stop", "place = 'Rome'"), "rome")
        .await?;
    let stop = |k: &str| source_record("stop", k, json!({"id": k, "place": "Rome"}));
    l.ingest("tracker", vec![stop("s1")])
        .await
        .must("ingesting s1")?;
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mark = store.head().await.must("reading the head")?;
    l.ingest("tracker", vec![stop("s2")])
        .await
        .must("ingesting s2")?;
    let after = events_from(store.as_ref(), mark.next()).await?;
    let cut = after
        .first()
        .ok_or_else(|| Failure::new("s2 was logged"))?
        .known_at;
    for k in ["s1", "s2"] {
        l.upsert("stop", k, json!({"note": "kept"})).await?;
    }
    let before = store.head().await.must("reading the head")?;
    let calls = backing::made(TOKEN);
    let tracker: SourceId = "tracker".into();
    store
        .remove_values(&tracker, &Selection::WrittenBefore(cut), false, None)
        .await
        .must("removing what tracker wrote before s2")?;
    let members = async |when: Option<Position>| {
        let find = lakelands_api::Find {
            filter: Some("in_set('rome')".into()),
            ..find_at(when)
        };
        let found = l.all_found("stop", find).await?;
        Ok::<_, Failure>(
            found
                .into_iter()
                .map(|r| r.key.to_string())
                .collect::<Vec<_>>(),
        )
    };
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for when in [None, Some(before)] {
            let s1 = at(&l, "stop", "s1", when).await?;
            let s2 = at(&l, "stop", "s2", when).await?;
            let taken = |r: &lakelands_api::Record| {
                ["place", "loud", "vec"]
                    .iter()
                    .all(|f| !r.fields.contains_key(*f))
            };
            let kept = |r: &lakelands_api::Record| {
                r.fields.get("loud") == Some(&json!("Rome")) && r.fields.contains_key("vec")
            };
            ensure!(
                s1.as_ref().is_some_and(taken) && s2.as_ref().is_some_and(kept),
                "after {by}, as of {when:?}, `s1` holds no `place`, `loud` nor `vec`, and `s2` \
                 keeps them: {s1:?}, {s2:?}"
            );
        }
        let rome = (members(None).await?, members(Some(before)).await?);
        ensure!(
            rome.0 == ["s2"] && rome.1 == ["s2"] && backing::made(TOKEN) == calls,
            "after {by}, `rome` holds `s2` alone, at the head and before the act, and no call is \
             made again: {rome:?}, {} calls after {calls}",
            backing::made(TOKEN)
        );
    }
    Ok(())
}

async fn an_output_of_a_later_version_replays_after_an_earlier_one_is_destroyed(
    store: Store,
    reopen: crate::conformance::Reopen,
) -> Held {
    use super::erasable::{again, person, with};
    use crate::conformance::backing;
    use lakelands_api::ontology::StorageKind;
    use lakelands_api::seam::KeyStore;

    const TOKEN: &str = "removed_replayed";
    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": backing::KIND,
        "inputs": [{"name": "name", "type": {"kind": "text"}}],
        "config": {"token": TOKEN, "echo": "name"}}),
    )
    .await?;
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p0", "Ada"), p("p1", "Ada")])
        .await
        .must("ingesting p0 and p1")?;
    for k in ["p0", "p1"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    // `loud` declared over the two: one output carried, and the other
    // fill naming it by `output_at`.
    let mut t = person();
    let loud =
        crate::conformance::fixture::computed_by("loud", StorageKind::Text, "shout", &["name"]);
    t.fields.push(loud);
    l.declare(t).await?;
    keys.create(store.lake(), &crm)
        .must("making a second version")?;
    let store = again(&reopen, store, with(&keys), "with a second version").await?;
    let l = Lake::of(store.as_ref());
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mark = store.head().await.must("reading the head")?;
    l.ingest("crm", vec![p("p2", "Bea")])
        .await
        .must("ingesting p2")?;
    let after = events_from(store.as_ref(), mark.next()).await?;
    let cut = after
        .first()
        .ok_or_else(|| Failure::new("p2 was logged"))?
        .known_at;
    let calls = backing::made(TOKEN);
    let done = store
        .remove_values(&crm, &Selection::WrittenBefore(cut), false, None)
        .await
        .must("removing what crm wrote before p2")?;
    ensure!(
        done.keys.len() == 1,
        "the act destroys crm's first version alone: {done:?}"
    );
    let replayed = async |store: &Store, by: &str| {
        let l = Lake::of(store.as_ref());
        let (p0, p1, p2) = (
            at(&l, "person", "p0", None).await?,
            at(&l, "person", "p1", None).await?,
            at(&l, "person", "p2", None).await?,
        );
        let none = |r: &Option<lakelands_api::Record>| {
            r.as_ref().is_some_and(|r| !r.fields.contains_key("loud"))
        };
        ensure!(
            none(&p0)
                && none(&p1)
                && p2
                    .as_ref()
                    .is_some_and(|r| r.fields.get("loud") == Some(&json!("Bea")))
                && backing::made(TOKEN) == calls,
            "{by}, `p0` and `p1` hold no `loud`, and `p2`'s reads Bea with no call made again: \
             {p0:?}, {p1:?}, {p2:?}, {} calls after {calls}",
            backing::made(TOKEN)
        );
        Ok::<_, Failure>(())
    };
    replayed(&store, "after the act").await?;
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    replayed(&store, "after a reopen").await?;
    store.rebuild().await.must("rebuilding")?;
    replayed(&store, "after a rebuild").await?;
    Ok(())
}

/// A key store whose `destroy_version` fails while `fails` is set.
struct Failing {
    keys: std::sync::Arc<super::erasable::Keys>,
    fails: std::sync::atomic::AtomicBool,
}

impl lakelands_api::seam::KeyStore for Failing {
    fn create(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<lakelands_api::id::KeyId> {
        self.keys.create(lake, source)
    }
    fn key(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &lakelands_api::id::KeyId,
    ) -> lakelands_api::Result<Option<lakelands_api::seam::DataKey>> {
        self.keys.key(lake, source, id)
    }
    fn destroy(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<()> {
        self.keys.destroy(lake, source)
    }
    fn destroy_version(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
        id: &lakelands_api::id::KeyId,
    ) -> lakelands_api::Result<()> {
        if self.fails.load(std::sync::atomic::Ordering::SeqCst) {
            let lake = "the key store".to_owned();
            let situation = lakelands_api::error::Situation::Busy { lake };
            return Err(lakelands_api::Error::unavailable(situation));
        }
        self.keys.destroy_version(lake, source, id)
    }
    fn destroys_versions(&self) -> bool {
        true
    }
    fn ids(
        &self,
        lake: &lakelands_api::LakeId,
        source: &SourceId,
    ) -> lakelands_api::Result<Vec<lakelands_api::id::KeyId>> {
        self.keys.ids(lake, source)
    }
}

async fn a_shredding_act_a_crash_stopped_is_finished_at_each_step(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use super::erasable::{Keys, person};
    use lakelands_api::seam::{KeyStore, Seams};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let keys = Arc::new(Keys::default());
    let failing = Arc::new(Failing {
        keys: keys.clone(),
        fails: AtomicBool::new(false),
    });
    let fails: super::forgotten::Fails = Arc::default();
    let wrap = super::forgotten::failing(&fails);
    let fault = async |store: Store| {
        let seams = Seams {
            keys: Some(failing.clone()),
            ..Seams::default()
        };
        let open = (given.fault)(store, seams, wrap.clone()).await;
        open.map_err(|e| Failure::new(format!("the writer's open was refused: {e}")))
    };
    let mut store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let crm: SourceId = "crm".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    let start = store.head().await.must("reading the head")?;
    let erased_before = async |store: &Store, key: &str, before: Position, acts_n: usize| {
        let l = Lake::of(store.as_ref());
        let r = at(&l, "person", key, Some(before)).await?;
        let n = acts(store.as_ref(), start).await?.len();
        ensure!(
            r.as_ref().is_some_and(|r| erased(r, "name", "crm")) && n == acts_n,
            "`{key}` reads its name erased before the act, and the log holds {acts_n} acts: \
             {r:?}, {n}"
        );
        Ok::<_, Failure>(())
    };
    // Stopped after the event, the key store failing to destroy.
    l.ingest("crm", vec![p("p1", "Ada")])
        .await
        .must("ingesting p1")?;
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let v1 = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    let before = store.head().await.must("reading the head")?;
    failing.fails.store(true, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &all, false, None).await;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        stopped.is_err() && held == v1,
        "the act whose destroy fails answers {stopped:?}, and the version stays: {held:?}"
    );
    // The version is named, not yet destroyed: it seals nothing more, or
    // what it sealed would go with it.
    let head = store.head().await.must("reading the head")?;
    let refused = l.ingest("crm", vec![p("p9", "Ivo")]).await;
    let unmoved = store.head().await.must("reading the head")?;
    ensure!(
        matches!(&refused, Err(lakelands_api::Error::Unavailable { situation })
            if matches!(**situation, lakelands_api::error::Situation::KeyMissing { .. }))
            && unmoved == head,
        "while the act's version is not yet destroyed, crm's next sealed value is refused \
         `key_missing`, appending nothing: {refused:?}, the head moved from {head} to {unmoved}"
    );
    // A rebuild does not count the act done while its version is held.
    failing.fails.store(false, Ordering::SeqCst);
    store.rebuild().await.must("rebuilding")?;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        held.is_empty(),
        "a rebuild after the failed destroy finishes the act: the version is gone: {held:?}"
    );
    erased_before(&store, "p1", before, 1).await?;
    // Stopped after the destroy, before its scoped restate: the first
    // page's commit fails, after the page dropped the records' states.
    let l = Lake::of(store.as_ref());
    l.ingest("crm", vec![p("p2", "Bea")])
        .await
        .must("ingesting p2")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    fails[5].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &all, false, None).await;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        stopped.is_err() && held.is_empty(),
        "the act whose scoped restate fails answers {stopped:?}, its version destroyed: {held:?}"
    );
    // The version took: it seals nothing more from the act's first turn,
    // and the source seals under a new one while the act is owed (099).
    let head = store.head().await.must("reading the head")?;
    let sealed = l.ingest("crm", vec![p("p3", "Cy")]).await;
    let moved = store.head().await.must("reading the head")?;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        sealed.is_ok() && moved > head && held.len() == 1,
        "the version the act took seals nothing more: crm's next sealed value is taken under \
         a new one while the act is owed: {sealed:?}, the head moved from {head} to {moved}, \
         {held:?}"
    );
    store = fault(store).await?;
    // The open leaves the act to the road beside it: poll, bounded, for
    // the settle whose restate erases `p2` at the head.
    let erased_at_head = async |store: &Store, key: &str| {
        let l = Lake::of(store.as_ref());
        let r = at(&l, "person", key, None).await?;
        ensure!(
            r.as_ref().is_some_and(|r| erased(r, "name", "crm")),
            "`{key}` does not read its name erased at the head: {r:?}"
        );
        Ok::<_, Failure>(())
    };
    settles(6_000, Duration::from_millis(10), || {
        erased_at_head(&store, "p2")
    })
    .await?;
    let l = Lake::of(store.as_ref());
    let p3 = at(&l, "person", "p3", None).await?;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        p3.as_ref()
            .is_some_and(|r| r.fields.get("name") == Some(&json!("Cy")))
            && held.len() == 1,
        "after the open ran the restate, the new version's value reads: {p3:?}, {held:?}"
    );
    Ok(())
}

check!(
    REOPENED_WRITE_READS_THROUGH_AN_OWED_ACT =
        a_writer_that_opened_while_an_act_is_owed_reads_through_it,
    "099: a writer that opens between an act's parts takes it up from the lists: a verb's `requires` on a record the owed source wrote is judged on the record refolded through the erasure before the road has restated it, and the source's own sealed value after that turn seals under a new version the key store holds, not refused `key_missing`; once the road's restate runs, the record reads as the restate left it",
    given
);

async fn a_writer_that_opened_while_an_act_is_owed_reads_through_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use super::erasable::{Keys, person};
    let keys = Arc::new(Keys::default());
    let fails: super::forgotten::Fails = Arc::default();
    let wrap = super::forgotten::failing(&fails);
    let fault = async |store: Store| {
        let seams = Seams {
            keys: Some(keys.clone()),
            ..Seams::default()
        };
        (given.fault)(store, seams, wrap.clone())
            .await
            .map_err(|e| Failure::new(format!("the writer's open was refused: {e}")))
    };
    let mut store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    l.declare_json(json!({"declare": "verb", "name": "copy", "on": "person",
        "risk": "low", "effect": "write", "requires": "name is null",
        "set": [{"field": "note", "value": "'owed'"}]}))
        .await?;
    let crm: SourceId = "crm".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest("crm", vec![p("p1", "Ada"), p("p2", "Bea")])
        .await
        .must("ingesting from crm")?;
    for k in ["p1", "p2"] {
        l.upsert("person", k, json!({"note": "kept"})).await?;
    }
    // The act stops after its first turn, its version destroyed, its
    // scoped restate failed at its first page's commit.
    fails[5].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&crm, &all, false, None).await;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        stopped.is_err() && held.is_empty(),
        "the act whose scoped restate fails answers {stopped:?}, its version destroyed: {held:?}"
    );
    // Every page's commit fails, so the act stays owed through the open.
    fails[5].store(1_000_000, Ordering::SeqCst);
    store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    let head = store.head().await.must("reading the head")?;
    let called = l
        .cat
        .call(
            &l.own,
            &"copy".into(),
            Some(&RecordKey::from("p2")),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        called.is_ok(),
        "a write in the writer that opened while the act is owed reads `p2` through the \
         erasure, so `name is null` passes: {called:?}"
    );
    // A version the act took seals nothing after an open either.
    let sealed = l.ingest("crm", vec![p("p3", "Cy")]).await;
    let moved = store.head().await.must("reading the head")?;
    let held = keys.ids(store.lake(), &crm).must("listing crm's keys")?;
    ensure!(
        sealed.is_ok() && moved > head && held.len() == 1,
        "crm's write after the open whose act took the old version seals under a new one the \
         key store holds: {sealed:?}, the head moved from {head} to {moved}, {held:?}"
    );
    // The pages commit again, and the road the open started restates.
    fails[5].store(0, Ordering::SeqCst);
    store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    settles(6_000, Duration::from_millis(1), || async {
        let shown = at(&l, "person", "p1", None)
            .await?
            .ok_or_else(|| Failure::new("`p1` after the road's restate"))?;
        ensure!(
            erased(&shown, "name", "crm") && shown.fields.get("note") == Some(&json!("kept")),
            "the road's restate takes `p1`'s name and keeps the author's note: {shown:?}"
        );
        Ok(())
    })
    .await
}

/// Append `records` of `ty` from `source` in delivery `id`, answering the
/// first window's position.
async fn delivered(
    l: &Lake,
    source: &str,
    ty: &str,
    id: &str,
    records: &[(&str, Value)],
) -> Result<Position, Failure> {
    let opts = IngestOpts {
        delivery: Some(DeliveryId::from(id)),
        ..IngestOpts::default()
    };
    let source: SourceId = source.into();
    let records = (records.iter())
        .map(|(k, f)| source_record(ty, k, f.clone()))
        .collect();
    let windows = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, records)
        .await
        .must("ingesting a delivery")?;
    windows
        .first()
        .map(|w| w.position)
        .ok_or_else(|| Failure::new("a delivery names no window"))
}

/// The wall clock, in microseconds since the epoch.
fn micros_now() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    i64::try_from(since.as_micros()).expect("the clock fits")
}

async fn an_acts_versions_seal_nothing_and_the_source_has_a_new_one(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let fails: super::forgotten::Fails = Arc::default();
    let wrap = super::forgotten::failing(&fails);
    let store = (given.fault)(store, super::erasable::with(&keys), wrap)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    l.declare(super::erasable::person()).await?;
    let p = |k: &str, name: &str| source_record("person", k, json!({"id": k, "name": name}));
    l.ingest(
        "tracker",
        vec![p("v1", "Ada"), p("v2", "Bea"), p("v3", "Cy")],
    )
    .await
    .must("ingesting tracker's records")?;
    let start = store.head().await.must("reading the head")?;
    let tracker: SourceId = "tracker".into();
    let old = super::erasable::first(&keys, store.as_ref(), "tracker")?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a far date"));
    // The act stops before its restate: its appends are whole in the log,
    // its version destroyed, and the projection still holds what it took.
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&tracker, &all, false, None).await;
    let events = events_after(store.as_ref(), start).await?;
    let removed = (events.iter())
        .filter(|e| matches!(e.body, EventBody::ValuesRemoved { .. }))
        .count();
    let whole = (events.iter()).any(|e| e.body.kind() == "act_whole");
    let held = keys
        .ids(store.lake(), &tracker)
        .must("listing tracker's keys")?;
    ensure!(
        stopped.is_err() && removed == 1 && whole && held.is_empty(),
        "the act's first turn destroys the version it takes whole, and only its restate is \
         owed: the act answered {stopped:?}, the log's `values_removed` {removed}, its whole \
         {whole}, the key store {held:?}"
    );
    // The source's write while the act is owed seals under a version the
    // act left, the source's new one, and reads its own values.
    let l = Lake::of(store.as_ref());
    l.ingest("tracker", vec![p("v9", "Zoe")])
        .await
        .must("ingesting while the act is owed")?;
    let sealed = (events_after(store.as_ref(), start).await?.into_iter())
        .filter(|e| matches!(e.body, EventBody::RecordIngested { .. }))
        .filter_map(|e| e.sealed.map(|s| s.key))
        .next_back();
    let held = keys
        .ids(store.lake(), &tracker)
        .must("listing tracker's keys")?;
    let v9 = at(&l, "person", "v9", None).await?;
    ensure!(
        held.len() == 1
            && held[0] != old
            && sealed.as_ref() == Some(&held[0])
            && v9
                .as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Zoe"))),
        "the version the act took seals nothing more: the source's write while the act is owed \
         seals under a new one the key store holds, and reads its own values: the old key \
         {old:?}, the keys {held:?}, sealing {sealed:?}, v9 {v9:?}"
    );
    // The act's repeat finishes its restate, appending its events no
    // second time, and answers its report.
    fails[1].store(0, Ordering::SeqCst);
    let done = store
        .remove_values(&tracker, &all, false, None)
        .await
        .must("the act's repeat")?;
    ensure!(
        done.events == 3,
        "the act's repeat finishes its restate and answers the report of the three records \
         it took whole: {done:?}"
    );
    // A forgetting's keys go in its first turn too, before its parts: a
    // reader between them finds none, and the source's ingest is refused,
    // so nothing is sealed under a destroyed key (099).
    let giver: SourceId = "giver".into();
    l.ingest(
        "giver",
        vec![p("q1", "Dee"), p("q2", "Eve"), p("q3", "Fay")],
    )
    .await
    .must("ingesting giver's records")?;
    let start = store.head().await.must("reading the head")?;
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.forget_source(&giver, None).await;
    let events = events_after(store.as_ref(), start).await?;
    let forgot = (events.iter()).any(|e| matches!(e.body, EventBody::SourceForgotten { .. }));
    let whole = (events.iter()).any(|e| e.body.kind() == "act_whole");
    let left = keys.ids(store.lake(), &giver).ok().unwrap_or_default();
    let refused = l.ingest("giver", vec![p("late", "Eve")]).await;
    let retired = (events.iter())
        .filter(|e| matches!(e.body, EventBody::RecordRetired { .. }))
        .count();
    ensure!(
        stopped.is_err() && forgot && whole && left.is_empty() && refused.is_err() && retired == 3,
        "the forgetting destroys giver's keys before its restate and its first turn retires \
         the three records giver alone brought, and the source's ingest while the act is owed \
         is refused, so nothing is sealed under a destroyed key: {stopped:?}, forgotten \
         {forgot}, whole {whole}, retired {retired}, keys {left:?}, ingest {refused:?}"
    );
    fails[1].store(0, Ordering::SeqCst);
    let done = store
        .forget_source(&giver, None)
        .await
        .must("the forgetting's repeat")?;
    ensure!(
        done.reclaimed.is_some(),
        "the forgetting's repeat finishes its restate and reclaim, appending nothing again: \
         {done:?}"
    );
    Ok(())
}

async fn a_removal_takes_an_event_of_its_delivery_written_since_its_report(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let v = |k: &str, d: u32| json!({"id": k, "duration": d});
    let b1 = delivered(&l, "tracker", "visit", "b1", &[("v1", v("v1", 10))]).await?;
    let tracker: SourceId = "tracker".into();
    let sel = Selection::Delivery(DeliveryId::from("b1"));
    let report = store
        .remove_values_preflight(&tracker, &sel)
        .await
        .must("the preflight")?;
    delivered(&l, "tracker", "visit", "b1", &[("v2", v("v2", 20))]).await?;
    let done = store
        .remove_values(&tracker, &sel, false, Some(&report.report_id))
        .await
        .must("the act under the report's id")?;
    let v1 = at(&l, "visit", "v1", Some(b1)).await?;
    let v2 = at(&l, "visit", "v2", None).await?;
    ensure!(
        report.events == 1
            && done.events == 2
            && v1
                .as_ref()
                .is_some_and(|r| erased(r, "duration", "tracker"))
            && v2.is_none(),
        "the report names one event, the act under its id takes the event the delivery grew \
         since, and the record it wrote is retired beside the first: the preflight {report:?}, \
         the act {done:?}, v1 {v1:?}, v2 {v2:?}"
    );
    Ok(())
}

async fn a_removal_leaves_an_event_of_its_delivery_written_after_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let mut few = limits();
    few.part_events = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let v = |k: &str, d: u32| json!({"id": k, "duration": d});
    let start = store.head().await.must("reading the head")?;
    delivered(
        &l,
        "tracker",
        "visit",
        "b1",
        &[
            ("v1", v("v1", 10)),
            ("v2", v("v2", 20)),
            ("v3", v("v3", 30)),
            ("v4", v("v4", 40)),
            ("v5", v("v5", 50)),
            ("v6", v("v6", 60)),
        ],
    )
    .await?;
    let tracker: SourceId = "tracker".into();
    let sel = Selection::Delivery(DeliveryId::from("b1"));
    let slot: Arc<std::sync::Mutex<Option<RemoveReport>>> = Arc::default();
    let (s, answer, s2) = (store.clone(), slot.clone(), store.clone());
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &sel, false, None)
                .await
                .must("the act")?;
            *crate::lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            settles(6_000, Duration::from_millis(5), || async {
                ensure!(
                    !acts(s2.as_ref(), start).await?.is_empty(),
                    "the act's `values_removed`"
                );
                Ok(())
            })
            .await?;
            let l = Lake::of(s2.as_ref());
            delivered(&l, "tracker", "visit", "b1", &[("v7", v("v7", 70))]).await?;
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = crate::lock(&slot).take().expect("the act answered");
    let v7 = at(&l, "visit", "v7", None).await?;
    let v1 = at(&l, "visit", "v1", None).await?;
    store.rebuild().await.must("rebuilding")?;
    let rebuilt = at(&l, "visit", "v7", None).await?;
    ensure!(
        done.events == 6
            && v1.is_none()
            && v7
                .as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(70)))
            && rebuilt
                .as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(70))),
        "the act counts the six events its selection held and retires the six records, while \
         the seventh, written after its first turn, keeps its duration at the head and after a \
         rebuild: the act {done:?}, v1 {v1:?}, v7 {v7:?}, after a rebuild {rebuilt:?}"
    );
    Ok(())
}

async fn a_tail_event_before_the_time_is_taken_and_one_after_is_not(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let names: Vec<String> = (0..1_200).map(|i| format!("m{i}")).collect();
    let many: Vec<(&str, u32)> = names.iter().map(|n| (n.as_str(), 10)).collect();
    tracked(&l, &many).await?;
    let tracker: SourceId = "tracker".into();
    let far = Timestamp(micros_now() + 3_000_000);
    let sel = Selection::WrittenBefore(far);
    let pre = store
        .remove_values_preflight(&tracker, &sel)
        .await
        .must("the preflight")?;
    let slot: Arc<std::sync::Mutex<Option<RemoveReport>>> = Arc::default();
    let (s, answer, s2) = (store.clone(), slot.clone(), store.clone());
    let early = source_record("visit", "early", json!({"id": "early", "duration": 5}));
    let late = source_record("visit", "late", json!({"id": "late", "duration": 7}));
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &sel, false, None)
                .await
                .must("the act")?;
            *crate::lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            let l = Lake::of(s2.as_ref());
            l.ingest("tracker", vec![early])
                .await
                .must("writing the event before the time")?;
            pause(Duration::from_millis(4_000)).await;
            l.ingest("tracker", vec![late])
                .await
                .must("writing the event after the time")?;
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = crate::lock(&slot).take().expect("the act answered");
    let early = at(&l, "visit", "early", None).await?;
    let late = at(&l, "visit", "late", None).await?;
    // The report is of the records the act judged again at its first turn,
    // not of the walk's: the one the tail made is retired, and counted.
    let retired: u32 = done.retired.iter().map(|t| t.retired).sum();
    let held: u64 = done.types.iter().map(|t| t.records).sum();
    ensure!(
        pre.events == 1_200
            && done.events == pre.events + 1
            && held == pre.events + 1
            && u64::from(retired) == pre.events + 1
            && done.kept.is_empty()
            && early.is_none()
            && late
                .as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(7))),
        "the act takes the event written before the time while it walked, one more than its \
         report read, retires it and counts it: the preflight {pre:?}, the act {done:?}, early \
         {early:?}, late {late:?}"
    );
    Ok(())
}

async fn an_unselected_tail_write_keeps_the_version_it_was_sealed_under(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(super::erasable::person()).await?;
    let names: Vec<String> = (0..1_500).map(|i| format!("m{i}")).collect();
    let rows: Vec<(String, Value)> = names
        .iter()
        .map(|n| (n.clone(), json!({"id": n, "name": n})))
        .collect();
    let many: Vec<(&str, Value)> = rows.iter().map(|(n, f)| (n.as_str(), f.clone())).collect();
    let b1 = delivered(&l, "tracker", "person", "b1", &many).await?;
    let tracker: SourceId = "tracker".into();
    let version = super::erasable::first(&keys, store.as_ref(), "tracker")?;
    let sel = Selection::Delivery(DeliveryId::from("b1"));
    let slot: Arc<std::sync::Mutex<Option<RemoveReport>>> = Arc::default();
    let (s, answer, s2) = (store.clone(), slot.clone(), store.clone());
    let source = tracker.clone();
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&source, &sel, false, None)
                .await
                .must("the act")?;
            *crate::lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            delivered(
                &Lake::of(s2.as_ref()),
                "tracker",
                "person",
                "b2",
                &[("other", json!({"id": "other", "name": "Zoe"}))],
            )
            .await?;
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = crate::lock(&slot).take().expect("the act answered");
    let held = keys
        .ids(store.lake(), &tracker)
        .must("listing tracker's keys")?;
    let other = at(&l, "person", "other", None).await?;
    let m0 = at(&l, "person", "m0", Some(b1)).await?;
    ensure!(
        done.events == 1_500
            && done.keys.is_empty()
            && held == [version.clone()]
            && other
                .as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Zoe")))
            && m0.as_ref().is_some_and(|r| erased(r, "name", "tracker")),
        "the unselected write, sealed under the version the walk saw wholly selected, keeps it: \
         the act names no version {done:?}, the key store holds {held:?}, the write reads \
         {other:?}, and a selected value reads as erased {m0:?}"
    );
    Ok(())
}

async fn a_taken_value_under_a_kept_version_refuses_shredded_and_counts_unshredded(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(super::erasable::person()).await?;
    let tracker: SourceId = "tracker".into();
    delivered(
        &l,
        "tracker",
        "person",
        "b1",
        &[("v1", json!({"id":"v1","name":"Ada"}))],
    )
    .await?;
    let first = super::erasable::first(&keys, store.as_ref(), "tracker")?;
    let second = keys
        .create(store.lake(), &tracker)
        .must("making a second version")?;
    let store = super::erasable::again(
        &given.reopen,
        store,
        super::erasable::with(&keys),
        "with a second version",
    )
    .await?;
    let l = Lake::of(store.as_ref());
    delivered(
        &l,
        "tracker",
        "person",
        "b2",
        &[("other", json!({"id":"other","name":"Bea"}))],
    )
    .await?;
    delivered(
        &l,
        "tracker",
        "person",
        "b1",
        &[("v3", json!({"id":"v3","name":"Cy"}))],
    )
    .await?;
    let sel = Selection::Delivery(DeliveryId::from("b1"));
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "removing d1 with `require_shredded` while a version it keeps seals one of its values",
        store.remove_values(&tracker, &sel, true, None).await,
        &json!({"code": "remove_not_shredded", "source": "tracker", "count": 1}),
    )?;
    let unmoved = store.head().await.must("reading the head")?;
    ensure!(unmoved == head, "the refused act appended up to {unmoved}");
    let done = store
        .remove_values(&tracker, &sel, false, None)
        .await
        .must("the act")?;
    let held = keys
        .ids(store.lake(), &tracker)
        .must("listing tracker's keys")?;
    let v3 = at(&l, "person", "v3", Some(head)).await?;
    let other = at(&l, "person", "other", None).await?;
    ensure!(
        done.keys == [first.clone()]
            && done.unshredded == 1
            && held == [second.clone()]
            && v3.as_ref().is_some_and(|r| erased(r, "name", "tracker"))
            && other
                .as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Bea"))),
        "the act destroys the version whose values it all selects, names one value redacted \
         only, and keeps the version that seals one outside its selection: the act {done:?}, \
         the keys {held:?}, v3 {v3:?}, other {other:?}"
    );
    Ok(())
}

async fn a_report_whose_record_another_source_wrote_since_is_stale(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    tracked(&l, &[("v1", 10)]).await?;
    let tracker: SourceId = "tracker".into();
    let far = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a far date"));
    let report = store
        .remove_values_preflight(&tracker, &far)
        .await
        .must("the preflight")?;
    ensure!(
        report
            .retired
            .iter()
            .any(|t| t.ty.as_str() == "visit" && t.retired == 1),
        "the preflight names `v1` retirable: {report:?}"
    );
    l.ingest(
        "web",
        vec![source_record(
            "visit",
            "v1",
            json!({"id": "v1", "place": "Oslo"}),
        )],
    )
    .await
    .must("web writing v1")?;
    refused_like(
        "removing under the report's id after another source's write on a record it named",
        store
            .remove_values(&tracker, &far, false, Some(&report.report_id))
            .await,
        &json!({"code": "remove_report_stale"}),
    )?;
    let live = at(&l, "visit", "v1", None).await?;
    let again = store
        .remove_values_preflight(&tracker, &far)
        .await
        .must("the preflight after web's write")?;
    ensure!(
        live.as_ref()
            .is_some_and(|r| r.fields.get("duration") == Some(&json!(10))
                && r.fields.get("place") == Some(&json!("Oslo")))
            && again.retired != report.retired
            && again
                .kept
                .iter()
                .any(|k| k.ty.as_str() == "visit" && k.first.iter().any(|k| k == "v1")),
        "web's write makes the report stale, the lake unmoved, the record live with both \
         values, and the preflight after names it kept: the first {report:?}, the second \
         {again:?}, v1 {live:?}"
    );
    Ok(())
}

async fn a_report_before_the_first_act_is_stale_only_on_the_overlap(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let (keys, store) = reopen_with_keys(&given.reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(super::erasable::person()).await?;
    let tracker: SourceId = "tracker".into();
    delivered(
        &l,
        "tracker",
        "person",
        "mon",
        &[
            ("v1", json!({"id": "v1", "name": "Ada"})),
            ("v2", json!({"id": "v2", "name": "Bea"})),
        ],
    )
    .await?;
    let version = super::erasable::first(&keys, store.as_ref(), "tracker")?;
    delivered(
        &l,
        "tracker",
        "person",
        "tue",
        &[
            ("v2", json!({"id": "v2", "city": "Rome"})),
            ("v3", json!({"id": "v3", "name": "Cy"})),
        ],
    )
    .await?;
    delivered(
        &l,
        "tracker",
        "person",
        "wed",
        &[
            ("v4", json!({"id": "v4", "name": "Dee"})),
            ("v5", json!({"id": "v5", "name": "Eve"})),
        ],
    )
    .await?;
    let mon = Selection::Delivery(DeliveryId::from("mon"));
    let tue = Selection::Delivery(DeliveryId::from("tue"));
    let wed = Selection::Delivery(DeliveryId::from("wed"));
    let pre_tue = store
        .remove_values_preflight(&tracker, &tue)
        .await
        .must("the preflight for tue")?;
    let pre_wed = store
        .remove_values_preflight(&tracker, &wed)
        .await
        .must("the preflight for wed")?;
    let pre_forget = store
        .forget_preflight(&tracker)
        .await
        .must("the forgetting's preflight")?;
    let retired = |r: &RemoveReport| -> Vec<u32> {
        (r.retired.iter())
            .filter(|t| t.ty.as_str() == "person")
            .map(|t| t.retired)
            .collect()
    };
    let kept = |r: &RemoveReport| -> Vec<String> {
        (r.kept.iter())
            .filter(|k| k.ty.as_str() == "person")
            .flat_map(|k| k.first.iter().map(ToString::to_string))
            .collect()
    };
    ensure!(
        pre_tue.events == 2
            && retired(&pre_tue) == [1]
            && kept(&pre_tue) == ["v2"]
            && pre_wed.events == 2
            && retired(&pre_wed) == [2]
            && kept(&pre_wed).is_empty(),
        "tue's preflight, read before either act, takes its two events, retires `v3`, and \
         keeps `v2`, which holds `mon`'s name; wed's, disjoint from mon, retires its two \
         records: tue {pre_tue:?}, wed {pre_wed:?}"
    );
    let mon_done = store
        .remove_values(&tracker, &mon, false, None)
        .await
        .must("the act for mon")?;
    let tue_slot: Arc<std::sync::Mutex<Option<Result<RemoveReport, Error>>>> = Arc::default();
    let wed_slot: Arc<std::sync::Mutex<Option<Result<RemoveReport, Error>>>> = Arc::default();
    let (tue_in, wed_in) = (tue_slot.clone(), wed_slot.clone());
    let (s1, s2) = (store.clone(), store.clone());
    let (source1, source2) = (tracker.clone(), tracker.clone());
    let tue_first_choice = tue.clone();
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s1
                .remove_values(&source1, &tue_first_choice, false, Some(&pre_tue.report_id))
                .await;
            *crate::lock(&tue_in) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            let done = s2
                .remove_values(&source2, &wed, false, Some(&pre_wed.report_id))
                .await;
            *crate::lock(&wed_in) = Some(done);
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let tue_first = crate::lock(&tue_slot).take().expect("tue answered");
    refused_like(
        "tue's act under a report read before mon's",
        tue_first,
        &json!({"code": "remove_report_stale"}),
    )?;
    let wed_done = crate::lock(&wed_slot)
        .take()
        .expect("wed answered")
        .must("the act for wed, under the report read before mon's")?;
    let tue_done = store
        .remove_values(&tracker, &tue, false, None)
        .await
        .must("the act for tue, run again")?;
    let held = keys
        .ids(store.lake(), &tracker)
        .must("listing tracker's keys")?;
    let mut live = Vec::new();
    for k in ["v1", "v2", "v3", "v4", "v5"] {
        live.push(at(&l, "person", k, None).await?);
    }
    ensure!(
        mon_done.events == 2
            && retired(&mon_done) == [1]
            && wed_done.events == 2
            && retired(&wed_done) == [2]
            && tue_done.events == 2
            && retired(&tue_done) == [2]
            && tue_done.keys == [version.clone()]
            && held.is_empty()
            && live.iter().all(Option::is_none),
        "each act counts its own two events; mon retires what it alone emptied, wed its two \
         records, and tue, folding the earlier selections, the record the two emptied; the \
         three together take the version whole and tue destroys it: mon {mon_done:?}, wed \
         {wed_done:?}, tue {tue_done:?}, the keys {held:?}, left {live:?}"
    );
    // A forgetting's report selects every event of its source: the acts
    // above took what it selected, so the id over what is left moves (099).
    refused_like(
        "a forgetting under a report read before the acts of its source",
        store
            .forget_source(&tracker, Some(pre_forget.report_id.as_str()))
            .await,
        &json!({"code": "forget_report_stale"}),
    )?;
    Ok(())
}
