//! A reference a record computes is judged as one its writer sends
//! (`docs/decisions/032-a-reference-names-a-live-record.md`).

use std::fmt::Write as _;

use lakelands_api::error::Missing;
use lakelands_api::write::{Declaration, Outcome, ProposedWrite, WriteOpts};
use lakelands_api::{Error, LakeStore, Principal, ProposalId, Refusal, Version};
use serde_json::{Value, json};

use super::only::holder;
use crate::conformance::backing::{EMBED_MODEL, KIND};
use crate::conformance::fixture::{
    self, Lake, Must, alike_renamed, patch, refused_like, restrict, source_record, try_upsert,
};
use crate::conformance::{Failure, Held, Store};

check!(
    COMPUTED_REFERENCE = a_reference_a_record_computes_is_judged_as_one_its_writer_sends,
    "032 (lakelands#795): a reference field computed at the write, by an expression or an `expr` function, names a live record its writer is answered, as a written one does: for a record that is absent, retired, held or not admitted by the writer's `only`, a writer of everything, one without `restricted` and a holder of `only` are each answered what the same key written into a reference field answers, applied or refused as `not_live`, by an upsert; a verb's call, an ingest and a link to an absent key are refused as their written twins are; a writer who may not read the reference, masked or not shown it, is refused as not granted, a held key as one never there, and the key the record holds, guessed, as any other; to a writer kept from the target type no record of it is there (lakelands#1003), so a key it computes, live or gone, is refused `not_granted` naming nothing, alike to the byte at an upsert and an ingest, and a verb whose path reads into that type is refused so for a live far record and a gone one; a field a computed reference lists counts as sent, whether its expression reads it or not (lakelands#936); a reference its write neither moves nor sends a field of is not judged again, and one whose field is sent again with the value it had, read directly or through another computed field, is judged as a key sent again, as is one of a record retired and written again, which has held nothing; a written reference is computed from nothing, whatever it lists; a proposal is judged at its accept by the function as declared then, while a function declared again computes every record's reference again unjudged, and a write that sends none of its fields is applied; to a reviewer without `restricted` a proposed write or call whose computed reference, from an initial value too or from a field sent again, reaches a held record is not there, its accept answered as an id no proposal holds, and so its accept or rejection again once the write has landed, the written twin alike, while one that reaches a live record is `decided` again (lakelands#923); a reference computed at the read, by a function's backing or from a field a backing computes or, through another computed field, one an embedding computes, is refused by `declare`, `apply` and `propose`, and a function declared again with a backing is refused while a reference calls it or reads a field that does, or a field computed at the read calls it, whatever else its type's fields say"
);

check!(
    COMPUTED_REFERENCE_CELLS = a_computed_reference_answers_alike_in_every_cell,
    "032 (lakelands#923): a reference computed at the write is answered at `ingest`, at a link's field, under `only` on the type written, and under a stale `expected_version`, by one who reads it and one who may not (masked or not shown it), alike on every engine: an ingest window whose record names a record not live to its writer commits nothing, its neighbours included, and `Ingested` names the record; a link's computed field is answered as its written twin; the reference is judged before `only` on the type written: outside it, a key that is not live is `not_live`, and a live one `not_granted` naming nothing; a stale version is told first, `conflict`, where the written twin is told its reference first, and a writer who may not read the reference learns nothing of the key from either"
);

/// A record of `p` in each state a reference may find it in, and whether
/// a write naming it is applied for the owner, for a writer without
/// `restricted` and for a holder of `only` on `p`.
const STATES: [(&str, [bool; 3]); 5] = [
    ("live", [true, true, true]),
    ("gone", [false, false, false]),
    ("old", [false, false, false]),
    ("held", [true, false, false]),
    ("theirs", [true, true, false]),
];

fn wire(e: &Error) -> String {
    serde_json::to_string(e).unwrap_or_default()
}

fn not_live<T>(answer: &lakelands_api::Result<T>) -> bool {
    answer.as_ref().is_err_and(|e| wire(e).contains("not_live"))
}

fn text(n: &str) -> Value {
    json!({"name": n, "type": {"kind": "text"}})
}

fn ty(name: &str, fields: Value) -> Value {
    json!({"declare": "type", "name": name, "fields": fields})
}

/// A reference to `p` computed `how` (`derived`, `function`) from `what`.
fn by(how: &str, what: &str) -> Value {
    let mut r = json!({"name": "r", "type": {"kind": "reference", "target": "p"},
        "authority": "derived"});
    r[how] = json!(what);
    r
}

/// A function of `s`, its `expr` when it has one.
fn function(name: &str, backing: &str, expr: Value) -> Value {
    let mut f = json!({"declare": "function", "name": name, "backing": backing,
        "inputs": [text("s")]});
    if !expr.is_null() {
        f["expr"] = expr;
    }
    f
}

/// The lake both rows write: `p`'s records `live`, `old` (retired), `held`
/// (restricted), `theirs` (another team's) and `p2`, and the types that
/// write, compute or move a reference to them.
async fn fixture(l: &Lake, s: &dyn LakeStore) -> Held {
    let to_p = json!({"kind": "reference", "target": "p"});
    let (s_, note) = (text("s"), text("note"));
    let mut initial = text("s");
    initial["initial"] = json!("held");
    for d in [
        ty("p", json!([text("team")])),
        function("g", "expr", json!("s")),
        function("h", "expr", json!("s")),
        function("remote", KIND, Value::Null),
        ty("dw", json!([s_, note, {"name": "r", "type": to_p}])),
        ty("dr", json!([s_, note, by("derived", "s")])),
        ty("fr", json!([s_, note, by("function", "g")])),
        ty("ir", json!([initial, note, by("derived", "s")])),
        // `kr` writes `r` and lists `x` beside it: nothing computes it.
        ty(
            "kr",
            json!([text("x"), {"name": "r", "type": to_p, "depends_on": ["x"]}]),
        ),
        json!({"declare": "verb", "name": "keep", "on": "dr", "risk": "low", "effect": "write",
            "set": [{"field": "s", "value": "'live'"}]}),
        // `r` reads `s` through `t`, another computed field.
        ty(
            "tr",
            json!([s_, {"name": "t", "type": {"kind": "text"}, "authority": "derived", "derived": "s"}, by("derived", "t")]),
        ),
        ty(
            "ah",
            json!([s_, {"name": "t", "type": {"kind": "text"}, "authority": "derived",
            "function": "h", "computed_at": "read"}]),
        ),
        json!({"declare": "verb", "name": "point", "on": "dr", "risk": "low", "effect": "write",
            "set": [{"field": "s", "value": "'gone'"}]}),
        // Its written twin.
        json!({"declare": "verb", "name": "point_w", "on": "dw", "risk": "low", "effect": "write",
            "set": [{"field": "r", "value": "'gone'"}]}),
        // `r` computed from `s` alone, listing `note` beside it.
        ty(
            "nr",
            json!([s_, note, {"name": "r", "type": to_p, "authority": "derived",
                "derived": "s", "depends_on": ["s", "note"]}]),
        ),
        json!({"declare": "verb", "name": "hold", "on": "dr", "risk": "low", "effect": "write",
            "set": [{"field": "s", "value": "'held'"}]}),
        // A source's twin, and a link's: `r` written and `r` computed.
        ty(
            "sw",
            json!([{"name": "r", "type": to_p, "authority": "source"}]),
        ),
        ty(
            "sr",
            json!([{"name": "s", "type": {"kind": "text"}, "authority": "source"}, by("derived", "s")]),
        ),
        json!({"declare": "type", "name": "lt", "fields": [s_], "relations": [
            {"name": "lw", "target": "p", "fields": [{"name": "r", "type": to_p}]},
            {"name": "lr", "target": "p", "fields": [s_, by("derived", "s")]}]}),
    ] {
        l.declare_json(d).await?;
    }
    for (k, team) in [
        ("live", "a"),
        ("old", "a"),
        ("held", "a"),
        ("theirs", "b"),
        ("p2", "a"),
    ] {
        l.upsert("p", k, json!({ "team": team })).await?;
    }
    let (p, old) = ("p".into(), "old".into());
    let retired = l.cat.retire(&l.own, &p, &old, WriteOpts::default()).await;
    retired.must("retiring `p` `old`")?;
    restrict(s, "p", "held", "a hold").await.map(drop)
}

async fn a_reference_a_record_computes_is_judged_as_one_its_writer_sends(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    fixture(&l, s).await?;
    let s_ = text("s");
    let retire = |ty: &'static str, k: &'static str| {
        let own = &l.own;
        async move {
            let (t, key, what) = (ty.into(), k.into(), format!("retiring `{ty}` `{k}`"));
            let retired = c.retire(own, &t, &key, WriteOpts::default()).await;
            retired.must(&what).map(drop)
        }
    };
    let writes = json!({"default": "write"});
    let only = json!({"default": "write", "types": {"p": {"only": {"team": "a"}}}});
    let unread = json!({"default": "write", "types": {"dr": {"fields": {"r": "masked"}},
        "fr": {"fields": {"r": "none"}}, "nr": {"fields": {"r": "masked"}}}});
    let writers = [l.own.clone(), holder(s, "w", writes), holder(s, "o", only)];
    // Each state for each writer: the computed reference answers what the
    // written one answers, and that is what the state says.
    for (n, who) in writers.iter().enumerate() {
        for (k, applied) in STATES {
            let (id, key) = (&who.id, format!("{}-{k}", who.id));
            let written = try_upsert(c, who, "dw", &key, json!({ "r": k })).await;
            ensure!(
                written.is_ok() == applied[n],
                "`{id}` writing a reference to `{k}`: {written:?}"
            );
            for ty in ["dr", "fr"] {
                let computed = try_upsert(c, who, ty, &key, json!({ "s": k })).await;
                let what = format!("`{id}` writing `{ty}` whose `r` computes `{k}`");
                match (&computed, &written) {
                    (Ok(_), Ok(_)) => {}
                    (Err(a), Err(b)) if not_live(&computed) => {
                        alike_renamed(&what, a, b, ("", ""))?;
                    }
                    _ => {
                        let said = format!("{what}: {computed:?}; written: {written:?}");
                        return Err(Failure::new(said));
                    }
                }
            }
        }
    }
    let (w, opts) = (&writers[1], WriteOpts::default);
    let (point, key) = ("point".into(), "w-live".into());
    let called = c.call(w, &point, Some(&key), Default::default(), opts());
    let called = called.await;
    let point_w = "point_w".into();
    let twin = c.call(w, &point_w, Some(&key), Default::default(), opts());
    let (twin, what) = (
        twin.await,
        "a verb's call that moves `dr` `w-live` to `gone`",
    );
    ensure!(
        not_live(&called) && twin.is_err(),
        "{what}: {called:?}; written: {twin:?}"
    );
    if let (Err(a), Err(b)) = (&called, &twin) {
        alike_renamed(what, a, b, ("", ""))?;
    }
    // A source's write and a link's are judged as an upsert's: each as
    // its written twin.
    let ingest = |ty: &'static str, fields: Value| {
        let l = &l;
        async move { l.ingest("feed", vec![source_record(ty, "k", fields)]).await }
    };
    let (written, computed) = (
        ingest("sw", json!({"r": "gone"})).await,
        ingest("sr", json!({"s": "gone"})).await,
    );
    match (&computed, &written) {
        (Err(a), Err(b)) if not_live(&computed) => {
            alike_renamed("an ingest of `sr` to `gone`", a, b, ("", ""))?
        }
        _ => {
            return Err(Failure::new(format!(
                "an ingest of `sr` to `gone`: {computed:?}; written: {written:?}"
            )));
        }
    }
    l.upsert("lt", "a", json!({"s": "x"})).await?;
    let link = |relation: &'static str, fields: Value| {
        let own = &l.own;
        async move {
            let (ty, a, live) = ("lt".into(), "a".into(), "live".into());
            c.link(own, &ty, &a, &relation.into(), &live, patch(fields), opts())
                .await
        }
    };
    let (written, computed) = (
        link("lw", json!({"r": "gone"})).await,
        link("lr", json!({"s": "gone"})).await,
    );
    match (&computed, &written) {
        (Err(a), Err(b)) if not_live(&computed) => {
            alike_renamed("a link whose `r` computes `gone`", a, b, ("lw", "lr"))?
        }
        _ => {
            return Err(Failure::new(format!(
                "a link whose `r` computes `gone`: {computed:?}; written: {written:?}"
            )));
        }
    }
    // One who may not read the reference is told nothing of its key: a
    // held record as one never there.
    let m = holder(s, "m", unread);
    let named = |e: &Error| {
        !matches!(
            e,
            Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            }
        )
    };
    for ty in ["dr", "fr"] {
        let what = format!("`{ty}` to a live record, by one who may not read `r`");
        let live = try_upsert(c, &m, ty, "m-live", json!({"s": "live"})).await;
        live.must(&what)?;
        let held = try_upsert(c, &m, ty, "m-held", json!({"s": "held"})).await;
        let gone = try_upsert(c, &m, ty, "m-gone", json!({"s": "gone"})).await;
        let what = format!("`{ty}` to `held`, by one who may not read `r`,");
        match (&held, &gone) {
            (Err(a), Err(b)) if !named(a) => alike_renamed(&what, a, b, ("gone", "held"))?,
            _ => {
                let said = format!("{what} names nothing: {held:?}; {gone:?}");
                return Err(Failure::new(said));
            }
        }
    }
    // To one kept from `p` no record of it is there (lakelands#1003): a key
    // its fields compute names none, live or gone, at an upsert and an
    // ingest alike, and a verb whose path reads into `p` is refused before
    // any record is read.
    let k = holder(
        s,
        "k",
        json!({"default": "write", "types": {"p": {"default": "none"}}}),
    );
    l.declare_json(
        json!({"declare": "verb", "name": "peek", "on": "dw", "risk": "low",
        "effect": "write", "requires": "r.team = 'a'", "set": [{"field": "s", "value": "'x'"}]}),
    )
    .await?;
    l.upsert("p", "p3", json!({"team": "a"})).await?;
    for (key, to) in [("k-live", "live"), ("k-gone", "p3")] {
        l.upsert("dw", key, json!({ "r": to })).await?;
    }
    retire("p", "p3").await?;
    let mut answers = Vec::new();
    for (key, to) in [("k-live", "live"), ("k-gone", "gone")] {
        let upsert = try_upsert(c, &k, "dr", key, json!({ "s": to })).await;
        let sent = vec![source_record("sr", key, json!({ "s": to }))];
        let ingest = fixture::ingest(c, &k, "feed", sent).await;
        let (peek, on) = ("peek".into(), key.into());
        let peek = c
            .call(&k, &peek, Some(&on), Default::default(), opts())
            .await;
        ensure!(
            [&upsert, &peek]
                .iter()
                .all(|a| a.as_ref().is_err_and(|e| !named(e))),
            "one kept from `p`, computing or reading across to `{to}`, is refused \
             `not_granted`, naming nothing: {upsert:?}; {peek:?}"
        );
        answers.push(serde_json::to_value((upsert, ingest, peek)).unwrap_or_default());
    }
    ensure!(
        answers[0] == answers[1],
        "one kept from `p` is answered alike for a live key and a gone one: {answers:?}"
    );
    // A reference its write neither moves nor sends a field of is not
    // judged again; one whose field is sent again is, as a key sent again,
    // and so is one of a record retired and written again.
    for ty in ["dw", "dr", "fr"] {
        let field = if ty == "dw" { "r" } else { "s" };
        l.upsert(ty, "zr", json!({ field: "p2" })).await?;
        retire(ty, "zr").await?;
        l.upsert(ty, "z1", json!({ field: "p2" })).await?;
    }
    l.upsert("nr", "z1", json!({"s": "p2"})).await?;
    l.upsert("tr", "z1", json!({"s": "p2"})).await?;
    l.upsert("kr", "z1", json!({"x": "a", "r": "p2"})).await?;
    // `ir` computes `held` from its initial value, each time it is made.
    l.upsert("ir", "zr", json!({"note": "a"})).await?;
    retire("ir", "zr").await?;
    retire("p", "p2").await?;
    let written = try_upsert(c, w, "dw", "zr", json!({"r": "p2"})).await;
    let resent = try_upsert(c, w, "dw", "z1", json!({"r": "p2"})).await;
    for ty in ["dr", "fr"] {
        let what = format!("`{ty}` `z1`, whose `r` stays `p2`, written with another field");
        let note = try_upsert(c, w, ty, "z1", json!({"note": "n"})).await;
        note.must(&what)?;
        let moved = try_upsert(c, w, ty, "z1", json!({"s": "gone"})).await;
        ensure!(not_live(&moved), "`{ty}` `z1` moved to `gone`: {moved:?}");
        for (key, twin) in [("z1", &resent), ("zr", &written)] {
            let again = try_upsert(c, w, ty, key, json!({"s": "p2"})).await;
            let what = format!("`{ty}` `{key}`, sent `p2` again, which is retired");
            match (&again, twin) {
                (Err(a), Err(b)) if not_live(&again) => alike_renamed(&what, a, b, ("", ""))?,
                _ => {
                    let said = format!("{what}: {again:?}; written: {twin:?}");
                    return Err(Failure::new(said));
                }
            }
        }
        // So one who may not read the reference learns nothing by guessing
        // it: the key the record holds is answered as one never there.
        let note = try_upsert(c, &m, ty, "z1", json!({"note": "m"})).await;
        note.must(&format!("`{ty}` `z1`, written with another field by `m`"))?;
        let held = try_upsert(c, &m, ty, "z1", json!({"s": "p2"})).await;
        let gone = try_upsert(c, &m, ty, "z1", json!({"s": "gone"})).await;
        match (&held, &gone) {
            (Err(a), Err(b)) if !named(a) => {
                alike_renamed("a guess of the key held", a, b, ("gone", "p2"))?
            }
            _ => {
                return Err(Failure::new(format!(
                    "`{ty}` `z1`, guessed by `m`: {held:?}; {gone:?}"
                )));
            }
        }
    }
    // To a reviewer without `restricted` a proposal whose computed
    // reference reaches a held record is not there, as with a written one:
    // its accept is answered as an id no proposal holds.
    let rv = holder(s, "rv", json!({"default": "write", "review": true}));
    let onto = |ty: &str, key: &str, fields: Value| ProposedWrite::Upsert {
        ty: ty.into(),
        key: key.into(),
        patch: patch(fields),
        opts: WriteOpts::default(),
    };
    let proposed = |who: &Principal, write: ProposedWrite| {
        let who = who.clone();
        async move {
            match c.propose(&who, write, "why".into()).await {
                Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
                other => Err(Failure::new(format!("a proposal was {other:?}"))),
            }
        }
    };
    let live = proposed(&l.own, onto("dr", "y0", json!({"s": "live"}))).await?;
    let live = c.accept(&rv, &live).await;
    live.must("the reviewer accepting `dr` to a live record")?;
    let written = proposed(&l.own, onto("dw", "y1", json!({"r": "held"}))).await?;
    let written = (c.accept(&rv, &written).await, written);
    let gone = |a: &lakelands_api::Result<Outcome>, id: &ProposalId| matches!(a, Err(Error::NotFound { missing: Missing::Proposal { id: i } }) if i == id);
    ensure!(
        gone(&written.0, &written.1),
        "a proposal writing a reference to `held` is not there to `rv`; {:?}",
        written.0
    );
    let held = json!({"s": "held"});
    for (ty, fields) in [
        ("dr", held.clone()),
        ("fr", held),
        ("ir", json!({"note": "n"})),
    ] {
        let id = proposed(&l.own, onto(ty, "y1", fields)).await?;
        let a = c.accept(&rv, &id).await;
        ensure!(
            gone(&a, &id),
            "a reviewer without `restricted` accepting `{ty}` to `held`: {a:?}"
        );
    }
    // Deciding again a proposal whose write has landed: to `rv` one that
    // reaches a held record is not there, the written twin alike, and one
    // that reaches a live record is `decided` (lakelands#923).
    for x in ["live", "held"] {
        for (ty, f) in [("dr", "s"), ("dw", "r")] {
            let id = proposed(&l.own, onto(ty, &format!("d-{x}"), json!({ f: x }))).await?;
            c.accept(&l.own, &id).await.must("the owner's accept")?;
            let again = c.accept(&rv, &id).await;
            let rejected = c.reject(&rv, &id, "no".into()).await;
            let told = |a| (x == "held" && gone(a, &id)) || (x == "live" && class(a) == "decided");
            let what = format!("`rv` deciding `{ty}` to `{x}` again: {again:?}, {rejected:?}");
            ensure!(told(&again) && told(&rejected), "{what}");
        }
    }
    // A field a computed reference lists counts as sent, whether its
    // expression reads it or not (lakelands#936).
    let listed = try_upsert(c, w, "nr", "z1", json!({"note": "b"})).await;
    let what = "`nr` `z1`, sent the `note` its `r` lists";
    ensure!(
        not_live(&listed) && resent.is_err(),
        "{what}: {listed:?}; written: {resent:?}"
    );
    if let (Err(a), Err(b)) = (&listed, &resent) {
        alike_renamed(what, a, b, ("", ""))?;
    }
    let masked = try_upsert(c, &m, "nr", "z1", json!({"note": "b"})).await;
    ensure!(
        masked.as_ref().is_err_and(|e| !named(e)),
        "`nr` `z1`, sent `note` by one who may not read `r`, is not granted, naming nothing: {masked:?}"
    );
    // A written reference is computed from nothing, whatever it lists.
    let listed = try_upsert(c, w, "kr", "z1", json!({"x": "b"})).await;
    listed.must("`kr` `z1`, whose written `r` lists `x`, written with `x`")?;
    // A record made again has held nothing: its reference is judged though
    // the retired record computed the same key and no field of it is sent.
    let (twin, again) = (
        try_upsert(c, w, "dw", "ih", json!({"r": "held"})).await,
        try_upsert(c, w, "ir", "zr", json!({"note": "b"})).await,
    );
    match (&again, &twin) {
        (Err(a), Err(b)) if not_live(&again) => {
            alike_renamed("`ir` `zr`, made again", a, b, ("", ""))?
        }
        _ => {
            return Err(Failure::new(format!(
                "`ir` `zr`, made again: {again:?}; written: {twin:?}"
            )));
        }
    }
    // A field is sent for a reference that reads it through another.
    let through = try_upsert(c, w, "tr", "z1", json!({"s": "p2"})).await;
    match (&through, &resent) {
        (Err(a), Err(b)) if not_live(&through) => {
            alike_renamed("`tr` `z1`, sent `p2` again", a, b, ("", ""))?
        }
        _ => {
            return Err(Failure::new(format!(
                "`tr` `z1`, sent `p2` again: {through:?}; written: {resent:?}"
            )));
        }
    }
    // What a proposed call reaches is what its record would compute.
    let call = ProposedWrite::Call {
        verb: "hold".into(),
        key: Some("y0".into()),
        payload: patch(json!({})),
        opts: WriteOpts::default(),
    };
    let id = proposed(&l.own, call).await?;
    let a = c.accept(&rv, &id).await;
    ensure!(
        gone(&a, &id),
        "the reviewer accepting a call to `held`: {a:?}"
    );
    let call = ProposedWrite::Call {
        verb: "keep".into(),
        key: Some("y0".into()),
        payload: patch(json!({})),
        opts: WriteOpts::default(),
    };
    let id = proposed(&l.own, call).await?;
    c.accept(&rv, &id)
        .await
        .must("the reviewer accepting a call to `live`")?;
    // A proposal that sends again the field of a reference to a held record
    // reaches it: not there to the reviewer without `restricted`, and applied for
    // one who holds it.
    l.upsert("dr", "h1", json!({"s": "held"})).await?;
    let id = proposed(&l.own, onto("dr", "h1", json!({"s": "held"}))).await?;
    let a = c.accept(&rv, &id).await;
    ensure!(
        gone(&a, &id),
        "the reviewer accepting `dr` `h1`, sent `held` again: {a:?}"
    );
    let all = json!({"default": "write", "review": true, "restricted": true});
    let accepted = c.accept(&holder(s, "ra", all), &id).await;
    accepted.must("a reviewer with `restricted` accepting `dr` `h1`, sent `held` again")?;
    // An accept judges the record as the function declared then computes it.
    let id = proposed(w, onto("fr", "x2", json!({"s": "live"}))).await?;
    l.declare_json(function("g", "expr", json!("'held'")))
        .await?;
    let accepted = c.accept(&l.own, &id).await;
    ensure!(
        not_live(&accepted),
        "`w`'s `fr` `x2`, accepted once `g` answers `held`: {accepted:?}"
    );
    // A declaration is its declarer's act: the fold computes every
    // reference again, unjudged, and a write that sends no field of it is
    // applied.
    let again = l.read("fr", "w-live").await?;
    ensure!(
        again.fields.get("r") == Some(&json!("held")),
        "`fr` `w-live` once `g` answers `held`: {again:?}"
    );
    let note = try_upsert(c, w, "fr", "w-live", json!({"note": "n"})).await;
    note.must("`w` writing `fr` `w-live`, whose `r` a declaration moved to `held`")?;
    // Nothing judges a reference computed at the read or outside the lake.
    let unjudged = json!({"code": "reference_unjudged", "at": [{"code": "field", "name": "r"}]});
    let read = |d: Value| serde_json::from_value(d).map_err(|e| Failure::new(e.to_string()));
    let mut at_read = by("derived", "s");
    at_read["computed_at"] = json!("read");
    let b = json!({"name": "b", "type": {"kind": "text"}, "authority": "derived",
        "function": "remote"});
    let e = json!({"name": "e", "type": {"kind": "vector", "dimensions": 2},
        "authority": "derived", "embed": {"from": ["s"], "model": EMBED_MODEL}});
    let via = json!({"name": "via", "type": {"kind": "text"}, "authority": "derived",
        "derived": "if(e is null, null, s)"});
    for (what, fields) in [
        ("at the read", json!([s_, at_read])),
        ("by a backing", json!([s_, by("function", "remote")])),
        (
            "from a field a backing computes",
            json!([s_, b, by("derived", "b")]),
        ),
        (
            "from a field an embedding computes, through another",
            json!([s_, e, via, by("derived", "via")]),
        ),
    ] {
        let d: Declaration = read(ty("ar", fields))?;
        let what = format!("a reference computed {what}");
        let said = c.declare(&l.own, d.clone()).await;
        refused_like(&format!("{what}, declared,"), said, &unjudged)?;
        let said = c.apply(&l.own, vec![d.clone()]).await;
        refused_like(&format!("{what}, applied,"), said, &unjudged)?;
        let declare = ProposedWrite::Declare {
            declarations: vec![d],
            plan: None,
        };
        let said = c.propose(&l.own, declare, "why".into()).await;
        refused_like(&format!("{what}, proposed,"), said, &unjudged)?;
    }
    // A function declared again computes its callers as they may be: each
    // field of a calling type is asked, whatever else the type says. In
    // `mr` a field at the write comes to read one at the read, before the
    // reference that reads it.
    let x = json!({"name": "x", "type": {"kind": "text"}, "authority": "derived",
        "derived": "s", "computed_at": "read"});
    let w2 = json!({"name": "w", "type": {"kind": "text"}, "authority": "derived",
        "function": "f"});
    l.declare_json(function("f", "expr", json!("s"))).await?;
    l.declare_json(ty("mr", json!([s_, x, w2, by("derived", "w")])))
        .await?;
    for (f, code) in [
        ("g", "reference_unjudged"),
        ("h", "at_read_backed"),
        ("f", "reference_unjudged"),
    ] {
        let mut again = function(f, KIND, Value::Null);
        if f == "f" {
            again["inputs"] = json!([text("s"), text("x")]);
        }
        let said = c.declare(&l.own, read(again)?).await;
        let case = json!({"code": code, "at": [{"code": "function", "name": f}]});
        refused_like(&format!("`{f}` declared again with a backing"), said, &case)?;
    }
    Ok(())
}

/// What an answer is, by its kind: `applied`, `not_live <key>`, `not_granted`
/// naming nothing or `not_granted r`, `unknown_field`, `conflict`,
/// `not_found`, `decided`, another refusal by its wire form; an ingest's
/// with the record it names; and any refusal but `not_live` whose wire
/// names a key of `p` marked so.
fn class<T>(a: &lakelands_api::Result<T>) -> String {
    let Err(e) = a else {
        return "applied".into();
    };
    let w = serde_json::to_value(e).unwrap_or_default();
    let r = &w["reason"];
    let kind = [&r["code"], &r["refusal"], &w["error"]];
    let kind = kind.into_iter().find_map(Value::as_str).unwrap_or("?");
    let more = match kind {
        "not_live" => r["key"].as_str(),
        "not_granted" => w["field"].as_str(),
        _ => None,
    };
    let more = more.map(|m| format!(" {m}")).unwrap_or_default();
    let names = |k: &str| kind != "not_live" && w.to_string().contains(&format!("\"{k}\""));
    let named = ["gone", "held", "theirs"].into_iter().any(names);
    let record = w["record"]["message"].as_str().unwrap_or("?");
    let at = w
        .get("index")
        .map(|i| format!(" at {i} from {} {record}", w["from"]));
    let named = if named { " names the key" } else { "" };
    format!("{kind}{more}{named}{}", at.unwrap_or_default())
}

async fn a_computed_reference_answers_alike_in_every_cell(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    fixture(&l, s).await?;
    let link = |r: &str| lakelands_api::key::link_type_name(&"lt".into(), &r.into()).to_string();
    let kept = |how: &str| {
        let (r, lr, lw) = (json!({"fields": {"r": how}}), link("lr"), link("lw"));
        json!({"default": "write", "ingest": true,
            "types": {"dr": r, "dw": r, "sr": r, "sw": r, lr: r, lw: r}})
    };
    let (w, m, n) = (
        holder(s, "w", json!({"default": "write", "ingest": true})),
        holder(s, "m", kept("masked")),
        holder(s, "n", kept("none")),
    );
    let writers = [("w", &w), ("m", &m), ("n", &n)];
    // A line per cell: the computed twin's answer, then the written one's.
    let mut said = String::new();
    for (who, p) in writers {
        for x in ["gone", "held"] {
            // 1. An ingest window whose middle record names `x`: what it
            // answers, and which of the window's records it commits.
            let _ = write!(said, "\n1 {who} {x}:");
            for (t, f) in [("sr", "s"), ("sw", "r")] {
                let key = |i: u8| format!("{who}-{x}-{i}");
                let recs = (1..=3u8).map(|i| {
                    let to = if i == 2 { x } else { "live" };
                    source_record(t, &key(i), json!({ f: to }))
                });
                let src = lakelands_api::SourceId::from("feed");
                let a = lakelands_api::ingest_all(c, p, &src, recs.collect()).await;
                let mut kept = Vec::new();
                for i in 1..=3 {
                    kept.push(l.read(t, &key(i)).await.is_ok());
                }
                let _ = write!(said, " [{} {kept:?}]", class(&a));
            }
            // 2. A link's field, computed and written.
            let _ = write!(said, "\n2 {who} {x}:");
            for (rel, f) in [("lr", "s"), ("lw", "r")] {
                let from = format!("{who}-{x}-{rel}");
                l.upsert("lt", &from, json!({"s": "x"})).await?;
                let (t, k, rel) = ("lt".into(), from.as_str().into(), rel.into());
                let (fields, opts) = (patch(json!({ f: x })), WriteOpts::default());
                let a = c.link(p, &t, &k, &rel, &"live".into(), fields, opts).await;
                let _ = write!(said, " [{}]", class(&a));
            }
            // 4. A stale `expected_version` beside a reference that is not
            // live.
            let _ = write!(said, "\n4 {who} {x}:");
            for (t, f) in [("dr", "s"), ("dw", "r")] {
                let key = format!("v-{who}-{x}");
                l.upsert(t, &key, json!({ f: "live" })).await?;
                let expected_version = Some(Version(9));
                let opts = WriteOpts {
                    expected_version,
                    ..Default::default()
                };
                let (ty, k) = (t.into(), key.as_str().into());
                let a = c.upsert(p, &ty, &k, patch(json!({ f: x })), opts).await;
                let _ = write!(said, " [{}]", class(&a));
            }
        }
    }
    // 3. `only` on the type written, inside it and outside.
    let only = |more: Value| {
        let t = json!({"only": {"note": "x"}, "fields": more});
        json!({"default": "write", "types": {"dr": t, "dw": t}})
    };
    let (o, om) = (
        holder(s, "o", only(json!({}))),
        holder(s, "om", only(json!({"r": "masked"}))),
    );
    for (who, p) in [("o", &o), ("om", &om)] {
        for x in ["gone", "held", "theirs"] {
            for note in ["x", "y"] {
                let _ = write!(said, "\n3 {who} {x} {note}:");
                for (t, f) in [("dr", "s"), ("dw", "r")] {
                    let key = format!("{who}-{x}-{note}");
                    let a = try_upsert(c, p, t, &key, json!({"note": note, f: x})).await;
                    let _ = write!(said, " [{}]", class(&a));
                }
            }
        }
    }
    let trim = |t: &str| t.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    let said = trim(&said);
    ensure!(
        said == trim(EXPECTED),
        "the cells answered otherwise than pinned:{said}"
    );
    Ok(())
}

/// What every cell answers, a line each in the order asked: the computed
/// twin's answer, then the written one's.
const EXPECTED: &str = "
    1 w gone: [not_live gone at 1 from 0 `sr` `w-gone-2` [false, false, false]] [not_live gone at 1 from 0 `sw` `w-gone-2` [false, false, false]]
    2 w gone: [not_live gone] [not_live gone]
    4 w gone: [conflict] [not_live gone]
    1 w held: [not_live held at 1 from 0 `sr` `w-held-2` [false, false, false]] [not_live held at 1 from 0 `sw` `w-held-2` [false, false, false]]
    2 w held: [not_live held] [not_live held]
    4 w held: [conflict] [not_live held]
    1 m gone: [not_granted at 1 from 0 `sr` `m-gone-2` [false, false, false]] [not_granted r at 0 from 0 `sw` `m-gone-1` [false, false, false]]
    2 m gone: [not_granted] [not_granted r]
    4 m gone: [conflict] [not_granted r]
    1 m held: [not_granted at 1 from 0 `sr` `m-held-2` [false, false, false]] [not_granted r at 0 from 0 `sw` `m-held-1` [false, false, false]]
    2 m held: [not_granted] [not_granted r]
    4 m held: [conflict] [not_granted r]
    1 n gone: [not_granted at 1 from 0 `sr` `n-gone-2` [false, false, false]] [unknown_field at 0 from 0 `sw` `n-gone-1` [false, false, false]]
    2 n gone: [not_granted] [unknown_field]
    4 n gone: [conflict] [unknown_field]
    1 n held: [not_granted at 1 from 0 `sr` `n-held-2` [false, false, false]] [unknown_field at 0 from 0 `sw` `n-held-1` [false, false, false]]
    2 n held: [not_granted] [unknown_field]
    4 n held: [conflict] [unknown_field]
    3 o gone x: [not_live gone] [not_live gone]
    3 o gone y: [not_live gone] [not_live gone]
    3 o held x: [not_live held] [not_live held]
    3 o held y: [not_live held] [not_live held]
    3 o theirs x: [applied] [applied]
    3 o theirs y: [not_granted] [not_granted]
    3 om gone x: [not_granted] [not_granted r]
    3 om gone y: [not_granted] [not_granted r]
    3 om held x: [not_granted] [not_granted r]
    3 om held y: [not_granted] [not_granted r]
    3 om theirs x: [applied] [not_granted r]
    3 om theirs y: [not_granted] [not_granted r]
";
