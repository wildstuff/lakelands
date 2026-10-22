//! An act reaches every place a value it takes went
//! (`docs/decisions/058-a-source-may-be-forgotten.md`,
//! `docs/decisions/065-part-of-a-source-may-be-taken-back.md`): a record a
//! re-keying migration moved is found at the key it bears now, and a value
//! that identifies records at the act or where it was written is never
//! taken (lakelands#1228); a membership the log decided over an erased
//! value, across an edge or of a kept view, is decided again without it,
//! at every position (lakelands#1232); and a reaction it read is
//! overruled where the history without it does not make it: the feed
//! names it, and a replayed write does not tell it (lakelands#1238).

use lakelands_api::write::{ForgetReport, Outcome, RemoveReport, WriteOpts};

use super::erasable::{Keys, again, person, with};
use super::removed::{at, erased};
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    REMOVED_REKEYED = a_value_taken_back_is_followed_through_a_rekeying,
    "065 (lakelands#1228): `visit` keyed by `id` and `zone` holds `tracker`'s `v1`, kept by an author's note, and `v2`, `tracker`'s alone; a migration re-keys it by `id` and `place`; taking back all `tracker` wrote counts both records at the keys they bear now, `duration` and neither `place`, a key field at the act, nor `zone`, one where it was written, retires `v2` at its new key and keeps `v1` there; `v1` holds no `duration`, its provenance saying it was erased, and keeps `place` and `zone`, at its new key at the head and before the act, at its old key before the migration, and after a rebuild"
);

check!(
    FORGOTTEN_REKEYED = a_forgotten_source_is_followed_through_a_rekeying,
    "058 (lakelands#1228): `crm`'s `p1`, kept by an author's note, and `p2`, `crm`'s alone, are re-keyed by `id` and `city`; forgetting `crm` counts both at the keys they bear now, retires `p2` at its new key and keeps `p1` there, whose `name` reads as erased and whose `city` stays, at its new key at the head and before the act, at its old key before the migration, and after a rebuild",
    reopens
);

check!(
    REMOVED_ACROSS = a_membership_decided_over_a_value_taken_back_is_decided_again,
    "065 (lakelands#1232): `call`s name the stop `s1` by a reference; the sets `at_rome` (`stop.place = 'Rome'`), `unplaced` (`stop.place is null`) and `rome_or_open` (`stop.place = 'Rome' or stop.open`) hold calls, `unplaced` declared after `tracker`'s first write, and `busy` the one group of the kept view `placeless` while it counts more than one stop of no place; `c1` names `s1` before `tracker` writes the `place` of `s1` and `s3` Rome, `s3` kept by an author's note, `c2` names `s1` after, `s1`'s author then closes it, and `tracker` writes `s2` in Rome after the cut; once what `tracker` wrote before the cut is taken back, no call is in `at_rome` nor `rome_or_open`, both are in `unplaced`, and `busy` holds the group, which the log never put in it; at the head after each step, read `as_of` after the act and after a rebuild, each set answers what it answers in a twin lake where `tracker` never wrote a `place` before the cut, and a stop of no place written after the act tells the memberships it moved as in the twin",
    given
);

check!(
    FORGOTTEN_ACROSS = a_membership_decided_over_a_forgotten_value_is_decided_again,
    "058 (lakelands#1232): the lake of `a_membership_decided_over_a_value_taken_back_is_decided_again`, `place` erasable and `tracker` forgotten in place of the values taken back, `s2` retired with it: no call is in `at_rome` nor `rome_or_open`, both are in `unplaced`, `busy` holds the group, and at the head after each step, read `as_of` after the act and after a rebuild, each set answers what it answers in the twin lake, and the stop written after the act tells the memberships it moved as in the twin",
    given
);

check!(
    REMOVED_FIRED = a_reaction_decided_over_a_value_taken_back_is_overruled,
    "065 (lakelands#1238): `stop` holds `place` from `tracker`, and `call` names a stop; the triggers `in_rome` (`place = 'Rome'`), `rome_or_open` (`place = 'Rome' or open`), `wrote` (no `when`) and, on `call`, `call_in_rome` (`stop.place = 'Rome'`), the set `at_rome` (`stop.place = 'Rome'`), on `stop` the set `romans` (`place = 'Rome'`), and the set `late` of stops whose `code` was learned after their note; `s1` is opened by its author and named by `c1` before `tracker` writes the `place` of `s1` and `s2` Rome, `s2` its alone, then `s1` is written again by its author and named by `c2`, those two with an idempotency key; once what `tracker` wrote is taken back, a poll of the feed from 0 names overruled the firings of `in_rome` and `call_in_rome`, those of `rome_or_open` on `s2`, at `tracker`'s write and at the act's retirement of `s2`, the `at_rome` joins, and the `romans` joins and `s2` leaving it at that retirement, and not those of `wrote`, of `rome_or_open` on the open `s1`, nor `s1` joining `late`, which reads when `code` was learned; the reactions it does not overrule are those of a twin lake where `tracker` never wrote a `place`, by kind, name and key, and the two keyed writes sent again answer what they set off as the twin's first writes were told; every reaction names the record event it reacts to as its cause, the act's retirement of `s2` included",
    given
);

check!(
    FORGOTTEN_FIRED = a_reaction_decided_over_a_forgotten_value_is_overruled,
    "058 (lakelands#1238): the lake of `a_reaction_decided_over_a_value_taken_back_is_overruled`, `place` erasable and `tracker` forgotten in place of the values taken back: the feed names the same reactions overruled, those it does not are the twin's, and the keyed writes sent again answer as the twin's first writes",
    given
);

/// The head of `store`.
async fn head(store: &dyn LakeStore) -> Result<Position, Failure> {
    store.head().await.must("reading the head")
}

/// What a report of either act names of the records it reaches.
fn reached(types: Value, fields: Value, retired: Value, kept: Value) -> Value {
    json!({"types": types, "fields": fields, "retired": retired, "kept": kept})
}

/// [`reached`] of a removal's report.
fn removed(r: &RemoveReport) -> Value {
    reached(
        json!(
            r.types
                .iter()
                .map(|t| json!([t.ty, t.records]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.fields
                .iter()
                .map(|f| json!([f.ty, f.field, f.redacted]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.retired
                .iter()
                .map(|t| json!([t.ty, t.retired]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.kept
                .iter()
                .map(|t| json!([t.ty, t.first]))
                .collect::<Vec<_>>()
        ),
    )
}

/// [`reached`] of a forgetting's report.
fn forgotten(r: &ForgetReport) -> Value {
    reached(
        json!(
            r.types
                .iter()
                .map(|t| json!([t.ty, t.records]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.fields
                .iter()
                .map(|f| json!([f.ty, f.field, f.values]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.retired
                .iter()
                .map(|t| json!([t.ty, t.retired]))
                .collect::<Vec<_>>()
        ),
        json!(
            r.co_owned
                .iter()
                .map(|t| json!([t.ty, t.first]))
                .collect::<Vec<_>>()
        ),
    )
}

/// `visit` keyed by `key`: `id`, `zone`, `place` and `duration` from a
/// source, the first three required, `note` an author's.
fn visit(key: &[&str]) -> Value {
    let (text, number) = (json!({"kind": "text"}), json!({"kind": "number"}));
    let sourced = |name: &str, ty: &Value| json!({"name": name, "type": ty, "authority": "source"});
    let keyable = |name: &str| {
        json!({"name": name, "type": text, "authority": "source",
                                      "required": true})
    };
    json!({"declare": "type", "name": "visit", "key": key, "fields": [
        keyable("id"), keyable("zone"), keyable("place"),
        sourced("duration", &number), {"name": "note", "type": text}]})
}

/// What `when` holds of a record of `ty` at `key`: `erased` erased from
/// `source`, and `kept` with the values given.
async fn holds(
    l: &Lake,
    (ty, key, when): (&str, &str, Option<Position>),
    (erased_field, source): (&str, &str),
    kept: Value,
) -> Held {
    let r = at(l, ty, key, when).await?;
    let held = |r: &lakelands_api::Record| {
        let keeps = |(f, v): (&String, &Value)| r.fields.get(f.as_str()) == Some(v);
        erased(r, erased_field, source) && kept.as_object().is_some_and(|k| k.iter().all(keeps))
    };
    ensure!(
        r.as_ref().is_some_and(held),
        "as of {when:?}, `{ty}` `{key}` holds no `{erased_field}`, erased from {source}, and \
         keeps {kept}: {r:?}"
    );
    Ok(())
}

async fn a_value_taken_back_is_followed_through_a_rekeying(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(visit(&["id", "zone"])).await?;
    let v = |k: &str, zone: &str, place: &str, duration: u32| {
        let fields = json!({"id": k, "zone": zone, "place": place, "duration": duration});
        source_record("visit", &format!("{k}:{zone}"), fields)
    };
    let visits = vec![v("v1", "z1", "Rome", 10), v("v2", "z2", "Oslo", 20)];
    l.ingest("tracker", visits)
        .await
        .must("ingesting from tracker")?;
    l.upsert("visit", "v1:z1", json!({"note": "kept"})).await?;
    let written = head(store.as_ref()).await?;
    let mut to = visit(&["id", "place"]);
    to.as_object_mut().map(|t| t.remove("declare"));
    let to: TypeDecl = serde_json::from_value(to).expect("a type");
    l.migrate(to).await?;
    let before = head(store.as_ref()).await?;
    let tracker: SourceId = "tracker".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let report = store.remove_values_preflight(&tracker, &all).await;
    let report = report.must("the preflight")?;
    let want = reached(
        json!([["visit", 2]]),
        json!([["visit", "duration", 2]]),
        json!([["visit", 1]]),
        json!([["visit", ["v1:Rome"]]]),
    );
    ensure!(
        removed(&report) == want,
        "the preflight counts both visits at the keys they bear now, `duration` and neither \
         `place` nor `zone`, retires `v2` and keeps `v1`: it names {} in {report:?}",
        removed(&report)
    );
    let done = store.remove_values(&tracker, &all, false, None).await;
    ensure!(
        removed(&done.must("taking back what tracker wrote")?) == want,
        "the act answers what its preflight named"
    );
    let kept = json!({"place": "Rome", "zone": "z1", "note": "kept"});
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for when in [None, Some(before)] {
            let of = ("visit", "v1:Rome", when);
            holds(&l, of, ("duration", "tracker"), kept.clone()).await?;
        }
        let then = ("visit", "v1:z1", Some(written));
        holds(&l, then, ("duration", "tracker"), kept.clone()).await?;
        let v2 = at(&l, "visit", "v2:Oslo", None).await?;
        ensure!(
            v2.is_none(),
            "after {by}, `v2`, tracker's alone, is retired at its new key: {v2:?}"
        );
    }
    Ok(())
}

async fn a_forgotten_source_is_followed_through_a_rekeying(store: Store, reopen: Reopen) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let p = |k: &str, name: &str, city: &str| {
        source_record("person", k, json!({"id": k, "name": name, "city": city}))
    };
    let people = vec![p("p1", "Ada", "Rome"), p("p2", "Bob", "Oslo")];
    l.ingest("crm", people).await.must("ingesting from crm")?;
    l.upsert("person", "p1", json!({"note": "kept"})).await?;
    let written = head(store.as_ref()).await?;
    let mut to = TypeDecl {
        key: vec!["id".into(), "city".into()],
        ..person()
    };
    to.fields
        .iter_mut()
        .filter(|f| f.name == "city")
        .for_each(|f| f.required = true);
    l.migrate(to).await?;
    let before = head(store.as_ref()).await?;
    let crm: SourceId = "crm".into();
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    let want = reached(
        json!([["person", 2]]),
        json!([["person", "name", 2]]),
        json!([["person", 1]]),
        json!([["person", ["p1:Rome"]]]),
    );
    ensure!(
        forgotten(&report) == want,
        "the preflight counts both people at the keys they bear now, retires `p2` and keeps \
         `p1`: it names {} in {report:?}",
        forgotten(&report)
    );
    store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm")?;
    let kept = json!({"city": "Rome", "note": "kept"});
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for when in [None, Some(before)] {
            holds(
                &l,
                ("person", "p1:Rome", when),
                ("name", "crm"),
                kept.clone(),
            )
            .await?;
        }
        let then = ("person", "p1", Some(written));
        holds(&l, then, ("name", "crm"), kept.clone()).await?;
        let p2 = at(&l, "person", "p2:Oslo", None).await?;
        ensure!(
            p2.is_none(),
            "after {by}, `p2`, crm's alone, is retired at its new key: {p2:?}"
        );
    }
    Ok(())
}

/// The sets of calls of the ACROSS rows, each with its `where`.
const SETS: [(&str, &str); 3] = [
    ("at_rome", "stop.place = 'Rome'"),
    ("unplaced", "stop.place is null"),
    ("rome_or_open", "stop.place = 'Rome' or stop.open"),
];

/// The act an ACROSS row takes on `tracker`.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    Remove,
    Forget,
}

/// `stop`: `code` and `place` a source's, `place` erasable for a
/// forgetting, `open` and `note` an author's; `call`, whose `stop` names
/// one; the [`SETS`] of calls, and `busy`, holding the one group of the
/// kept view `placeless` while it counts more than one stop with no
/// place.
async fn stops(l: &Lake, act: Act) -> Held {
    let text = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "stop", "fields": [
        {"name": "code", "type": text, "authority": "source"},
        {"name": "place", "type": text, "authority": "source", "erasable": act == Act::Forget},
        {"name": "open", "type": {"kind": "boolean"}},
        {"name": "note", "type": text}]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "call", "fields": [
        {"name": "stop", "type": {"kind": "reference", "target": "stop"}}]}))
        .await?;
    // `unplaced` is declared after `tracker` writes: only the fold's
    // decision of the whole set puts `c1` in it.
    for (name, filter) in SETS.iter().filter(|(n, _)| *n != "unplaced") {
        l.declare_as(object_set_on(name, "call", filter), name)
            .await?;
    }
    let view = json!({"source": "stop", "where": "place is null",
                      "measures": [{"name": "n", "expr": "count()"}]});
    l.declare_json(json!({"declare": "type", "name": "placeless", "view": view,
        "fields": [{"name": "n", "type": {"kind": "number"}}]}))
        .await?;
    l.set("view_kept.placeless", json!(true)).await?;
    l.declare_as(object_set_on("busy", "placeless", "n > 1"), "busy")
        .await?;
    Ok(())
}

/// The ACROSS rows' steps on `store`, `tracker` writing the `place` of
/// `s1` and `s3` where `placed`, then `act`, then a stop written: the head
/// after each, and the memberships that write moved.
async fn across(store: &Store, act: Act, placed: bool) -> Result<Steps, Failure> {
    let l = Lake::of(store.as_ref());
    stops(&l, act).await?;
    let mut heads = Vec::new();
    l.upsert("stop", "s1", json!({"note": "kept", "open": true}))
        .await?;
    heads.push(head(store.as_ref()).await?);
    l.upsert("call", "c1", json!({"stop": "s1"})).await?;
    heads.push(head(store.as_ref()).await?);
    // `s3` joins `s1` among the stops of no place once their place is
    // erased: `busy` holds a group the log never put in it.
    let written = ["s1", "s3"].map(|k| {
        let mut fields = json!({"code": k});
        if placed {
            fields["place"] = json!("Rome");
        }
        source_record("stop", k, fields)
    });
    let written = written.to_vec();
    l.ingest("tracker", written)
        .await
        .must("tracker writing s1")?;
    heads.push(head(store.as_ref()).await?);
    let (name, filter) = SETS[1];
    l.declare_as(object_set_on(name, "call", filter), name)
        .await?;
    heads.push(head(store.as_ref()).await?);
    l.upsert("stop", "s3", json!({"note": "kept"})).await?;
    l.upsert("call", "c2", json!({"stop": "s1"})).await?;
    heads.push(head(store.as_ref()).await?);
    l.upsert("stop", "s1", json!({"open": false})).await?;
    let mark = head(store.as_ref()).await?;
    heads.push(mark);
    std::thread::sleep(std::time::Duration::from_millis(3));
    let s2 = source_record("stop", "s2", json!({"code": "B", "place": "Rome"}));
    l.ingest("tracker", vec![s2])
        .await
        .must("tracker writing s2")?;
    heads.push(head(store.as_ref()).await?);
    let tracker: SourceId = "tracker".into();
    match act {
        Act::Remove => {
            let after = events_from(store.as_ref(), mark.next()).await?;
            let cut = after.first().ok_or_else(|| Failure::new("s2 was logged"))?;
            let cut = Selection::WrittenBefore(cut.known_at);
            let done = store.remove_values(&tracker, &cut, false, None).await;
            done.must("taking back what tracker wrote before s2")?;
        }
        Act::Forget => drop(
            store
                .forget_source(&tracker, None)
                .await
                .must("forgetting")?,
        ),
    }
    let acted = head(store.as_ref()).await?;
    heads.push(acted);
    // A write after the act decides from the memberships the act left: a
    // set of a view is read by its `where`, and told by such a write.
    l.upsert("stop", "s4", json!({"note": "late"})).await?;
    heads.push(head(store.as_ref()).await?);
    let told = (events_after(store.as_ref(), acted).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ObjectSetChanged {
                set, key, joined, ..
            } => Some(format!("{set} {key} {joined}")),
            _ => None,
        })
        .collect();
    Ok((heads, told))
}

/// The head after each step of an ACROSS row, and what its last write
/// told of the memberships it moved.
type Steps = (Vec<Position>, Vec<String>);

/// The members of each set of the ACROSS rows as of `when`, by key.
async fn members(store: &Store, when: Option<Position>) -> Result<Vec<Vec<String>>, Failure> {
    let l = Lake::of(store.as_ref());
    let of = |ty: &'static str, set: &str| (ty, format!("in_set('{set}')"));
    let asked = SETS.map(|(s, _)| of("call", s)).into_iter();
    let mut all = Vec::new();
    for (ty, filter) in asked.chain([of("placeless", "busy")]) {
        let find = lakelands_api::Find {
            filter: Some(filter.as_str().into()),
            ..find_at(when)
        };
        // Before `unplaced` is declared, a read refuses it, in either lake.
        let found = match l.all_found(ty, find).await {
            Ok(found) => found.into_iter().map(|r| r.key.to_string()).collect(),
            Err(e) if e.to_string().contains("SetUndeclared") => vec!["undeclared".into()],
            Err(e) => return Err(e),
        };
        all.push(found);
    }
    Ok(all)
}

/// An ACROSS row: `act` in a lake where `tracker` wrote `s1`'s `place`
/// and in its twin where it did not, each with a key store.
async fn decided_again(store: Store, given: crate::conformance::Given, act: Act) -> Held {
    // The lake first: a store that answers nothing says so before a twin
    // is asked for.
    head(store.as_ref()).await?;
    let keys = Arc::new(Keys::default());
    let seams = with(&keys);
    let store = match act {
        Act::Forget => again(&given.reopen, store, seams.clone(), "with a key store").await?,
        Act::Remove => store,
    };
    let twin = (given.twin)(store.lake().clone(), seams).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let ((heads, told), (twins, twin_told)) = (
        across(&store, act, true).await?,
        across(&twin, act, false).await?,
    );
    ensure!(
        told == twin_told,
        "a write after the act tells the memberships it moved as in the twin: {told:?}, and \
         {twin_told:?} in the twin"
    );
    let at_head = members(&store, None).await?;
    let both = vec!["c1".to_owned(), "c2".to_owned()];
    let want = [Vec::new(), both, Vec::new(), vec!["all".to_owned()]];
    ensure!(
        at_head == want,
        "after the act no call is in `at_rome` nor `rome_or_open`, both are in `unplaced`, and \
         `busy` holds the stops of no place: {at_head:?}"
    );
    for by in ["the act", "a rebuild"] {
        if by == "a rebuild" {
            store.rebuild().await.must("rebuilding")?;
        }
        for (step, (here, there)) in heads.iter().zip(&twins).enumerate() {
            let (a, b) = (
                members(&store, Some(*here)).await?,
                members(&twin, Some(*there)).await?,
            );
            ensure!(
                a == b,
                "after {by}, as of the head after step {step}, the sets answer {a:?}, and {b:?} \
                 in the twin where tracker never wrote s1's place"
            );
        }
    }
    Ok(())
}

async fn a_membership_decided_over_a_value_taken_back_is_decided_again(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    decided_again(store, given, Act::Remove).await
}

async fn a_membership_decided_over_a_forgotten_value_is_decided_again(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    decided_again(store, given, Act::Forget).await
}

/// The triggers of the FIRED rows: name, type and `when`.
const FIRED: [(&str, &str, Option<&str>); 4] = [
    ("in_rome", "stop", Some("place = 'Rome'")),
    ("rome_or_open", "stop", Some("place = 'Rome' or open")),
    ("wrote", "stop", None),
    ("call_in_rome", "call", Some("stop.place = 'Rome'")),
];

/// The FIRED rows' steps on `store`, `tracker` writing `s1`'s `place`
/// where `placed`, then `act`: what the two keyed writes set off, as
/// first told, and as told when they are sent again after the act.
async fn fired(store: &Store, act: Act, placed: bool) -> Result<[Vec<Vec<String>>; 2], Failure> {
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "stop", "fields": [
        {"name": "code", "type": text, "authority": "source"},
        {"name": "place", "type": text, "authority": "source", "erasable": act == Act::Forget},
        {"name": "open", "type": {"kind": "boolean"}},
        {"name": "note", "type": text}]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "call", "fields": [
        {"name": "stop", "type": {"kind": "reference", "target": "stop"}}]}))
        .await?;
    for (name, on, when) in FIRED {
        l.declare_as(trigger_on(name, on, when), name).await?;
    }
    let (name, filter) = SETS[0];
    l.declare_as(object_set_on(name, "call", filter), name)
        .await?;
    // `late` reads when `code` was learned, and no erased value: `s1`
    // joins it at `tracker`'s write, which decided it was not a member
    // before.
    let late = object_set_on("late", "stop", "known_at(code) > known_at(note)");
    l.declare_as(late, "late").await?;
    // `romans` reads the stop's own erased field, by its `where`.
    let romans = object_set_on("romans", "stop", "place = 'Rome'");
    l.declare_as(romans, "romans").await?;
    l.upsert("stop", "s1", json!({"note": "kept", "open": true}))
        .await?;
    l.upsert("call", "c1", json!({"stop": "s1"})).await?;
    crate::conformance::fixture::pause(std::time::Duration::from_millis(400)).await;
    // `s2` is `tracker`'s alone: the act retires it, and its own append
    // fires on the retirement over the place it takes.
    let written = ["s1", "s2"].map(|k| {
        let mut fields = json!({"code": k});
        if placed {
            fields["place"] = json!("Rome");
        }
        source_record("stop", k, fields)
    });
    l.ingest("tracker", written.to_vec())
        .await
        .must("tracker writing")?;
    let keyed = [
        ("stop", "s1", json!({"note": "again"})),
        ("call", "c2", json!({"stop": "s1"})),
    ];
    let mut told = [Vec::new(), Vec::new()];
    for (round, told) in told.iter_mut().enumerate() {
        let tracker: SourceId = "tracker".into();
        match (round, act) {
            (0, _) => {}
            (_, Act::Remove) => {
                let all =
                    Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
                let done = store.remove_values(&tracker, &all, false, None).await;
                drop(done.must("taking back what tracker wrote")?);
            }
            (_, Act::Forget) => drop(
                store
                    .forget_source(&tracker, None)
                    .await
                    .must("forgetting")?,
            ),
        }
        for (ty, key, fields) in &keyed {
            let opts = WriteOpts {
                idempotency_key: Some(format!("{ty}-{key}").into()),
                ..WriteOpts::default()
            };
            let (t, k) = (&(*ty).into(), &(*key).into());
            let done = l
                .cat
                .upsert(&l.own, t, k, patch(fields.clone()), opts)
                .await;
            let Outcome::Applied { set_off, .. } = done.must("a keyed write")? else {
                return Err(Failure::new("a keyed write was applied"));
            };
            let fired = set_off.fired.iter().map(|t| t.to_string());
            told.push([fired.collect(), set_off.joined, set_off.left].concat());
        }
    }
    Ok(told)
}

/// The reactions a poll of `store`'s feed from 0 tells, by kind, name and
/// key: those it does not name overruled, and those it does, in order.
async fn reactions(store: &dyn LakeStore) -> Result<[Vec<String>; 2], Failure> {
    let kinds = ["trigger_fired", "trigger_dropped", "object_set_changed"];
    let filter = FeedFilter {
        kinds: kinds.map(str::to_owned).to_vec(),
        ..FeedFilter::default()
    };
    let (mut from, mut told) = (Position::GENESIS, [Vec::new(), Vec::new()]);
    loop {
        let b = store.feed().poll(from, &filter, 1024).await;
        let b = b.must("polling the feed")?;
        for e in &b.events {
            let what = match &e.body {
                EventBody::TriggerFired { trigger, key, .. }
                | EventBody::TriggerDropped { trigger, key, .. } => format!("{trigger} {key}"),
                EventBody::ObjectSetChanged {
                    set, key, joined, ..
                } => format!("{set} {key} {joined}"),
                _ => continue,
            };
            told[usize::from(b.overruled.contains(&e.position))].push(what);
        }
        if b.next > b.head {
            return Ok(told);
        }
        from = b.next;
    }
}

/// A FIRED row: `act` in a lake where `tracker` wrote `s1`'s `place` and
/// in its twin where it did not.
async fn overruled(store: Store, given: crate::conformance::Given, act: Act) -> Held {
    head(store.as_ref()).await?;
    let keys = Arc::new(Keys::default());
    let seams = with(&keys);
    let store = match act {
        Act::Forget => again(&given.reopen, store, seams.clone(), "with a key store").await?,
        Act::Remove => store,
    };
    let twin = (given.twin)(store.lake().clone(), seams).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let ([_, again], [first, _]) = (
        fired(&store, act, true).await?,
        fired(&twin, act, false).await?,
    );
    let ([told, overruled], [twin_told, _]) = (
        reactions(store.as_ref()).await?,
        reactions(twin.as_ref()).await?,
    );
    let mut overruled = overruled;
    overruled.sort();
    let want = [
        "at_rome c1 true",
        "at_rome c2 true",
        "call_in_rome c1",
        "call_in_rome c2",
        "in_rome s1",
        "in_rome s1",
        "in_rome s2",
        "in_rome s2",
        "romans s1 true",
        "romans s2 false",
        "romans s2 true",
        "rome_or_open s2",
        "rome_or_open s2",
    ];
    ensure!(
        overruled == want,
        "after the act the feed names overruled the firings of `in_rome` and `call_in_rome`, \
         those of `rome_or_open` on `s2`, which is not open, at `tracker`'s write and at the \
         act's retirement of it, the `at_rome` joins, and the `romans` joins and the leave at \
         that retirement, and nothing else: {overruled:?}"
    );
    ensure!(
        told == twin_told,
        "the reactions the feed does not overrule are the twin's: {told:?}, and {twin_told:?} in \
         the twin where tracker never wrote s1's place"
    );
    // The act's retirements are its own events: a reaction to one names it.
    let log = events_from(store.as_ref(), Position::GENESIS).await?;
    let record = |at: Option<Position>| {
        let e = log.iter().find(|e| Some(e.position) == at);
        e.is_some_and(|e| e.body.kind().starts_with("record_"))
    };
    let reactions = log.iter().filter(|e| {
        matches!(
            e.body,
            EventBody::TriggerFired { .. } | EventBody::ObjectSetChanged { .. }
        )
    });
    let stray: Vec<_> = reactions
        .filter(|e| !record(e.cause))
        .map(|e| e.position)
        .collect();
    ensure!(
        stray.is_empty(),
        "every reaction's cause is the record event it reacts to: not at {stray:?}"
    );
    ensure!(
        again == first,
        "the keyed writes sent again after the act answer what they set off as the twin's first \
         writes were told: {again:?}, and {first:?} in the twin"
    );
    Ok(())
}

async fn a_reaction_decided_over_a_value_taken_back_is_overruled(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    overruled(store, given, Act::Remove).await
}

async fn a_reaction_decided_over_a_forgotten_value_is_overruled(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    overruled(store, given, Act::Forget).await
}
