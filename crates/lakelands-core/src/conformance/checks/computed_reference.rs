//! A reference a record computes is judged as one its writer sends
//! (`docs/decisions/032-a-reference-names-a-live-record.md`).

use lakelands_api::error::Missing;
use lakelands_api::write::{Declaration, Outcome, ProposedWrite, WriteOpts};
use lakelands_api::{Error, Principal, ProposalId, Refusal};
use serde_json::{Value, json};

use super::only::holder;
use crate::conformance::backing::{EMBED_MODEL, KIND};
use crate::conformance::fixture::{
    Lake, Must, alike_renamed, patch, refused_like, restrict, source_record, try_upsert,
};
use crate::conformance::{Failure, Held, Store};

check!(
    COMPUTED_REFERENCE = a_reference_a_record_computes_is_judged_as_one_its_writer_sends,
    "032 (lakelands#795): a reference field computed at the write, by an expression or an `expr` function, names a live record its writer is answered, as a written one does: for a record that is absent, retired, held or not admitted by the writer's `only`, a writer of everything, one without `restricted` and a holder of `only` are each answered what the same key written into a reference field answers, applied or refused as `not_live`, by an upsert; a verb's call, an ingest and a link to an absent key are refused as their written twins are; a writer who may not read the reference, masked or not shown it, is refused as not granted, a held key as one never there, and the key the record holds, guessed, as any other; a reference its write neither moves nor sends a field of is not judged again, and one whose field is sent again with the value it had, read directly or through another computed field, is judged as a key sent again, as is one of a record retired and written again, which has held nothing; a written reference is computed from nothing, whatever it lists; a proposal is judged at its accept by the function as declared then, while a function declared again computes every record's reference again unjudged, and a write that sends none of its fields is applied; to a reviewer without `restricted` a proposed write or call whose computed reference, from an initial value too or from a field sent again, reaches a held record is not there, its accept answered as an id no proposal holds; a reference computed at the read, by a function's backing or from a field a backing computes or, through another computed field, one an embedding computes, is refused by `declare`, `apply` and `propose`, and a function declared again with a backing is refused while a reference calls it or reads a field that does, or a field computed at the read calls it, whatever else its type's fields say"
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

async fn a_reference_a_record_computes_is_judged_as_one_its_writer_sends(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    let to_p = json!({"kind": "reference", "target": "p"});
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let by = |how: &str, what: &str| {
        let mut r = json!({"name": "r", "type": to_p, "authority": "derived"});
        r[how] = json!(what);
        r
    };
    let function = |name: &str, backing: &str, expr: Value| {
        let mut f = json!({"declare": "function", "name": name, "backing": backing,
            "inputs": [text("s")]});
        if !expr.is_null() {
            f["expr"] = expr;
        }
        f
    };
    let ty = |name: &str, fields: Value| json!({"declare": "type", "name": name, "fields": fields});
    let (s_, note) = (text("s"), text("note"));
    let mut initial = text("s");
    initial["initial"] = json!("held");
    for d in [
        ty("p", json!([text("team")])),
        function("g", "expr", json!("s")),
        function("h", "expr", json!("s")),
        function("remote", KIND, Value::Null),
        ty("dw", json!([s_, {"name": "r", "type": to_p}])),
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
    let retire = |ty: &'static str, k: &'static str| {
        let own = &l.own;
        async move {
            let (t, key, what) = (ty.into(), k.into(), format!("retiring `{ty}` `{k}`"));
            let retired = c.retire(own, &t, &key, WriteOpts::default()).await;
            retired.must(&what).map(drop)
        }
    };
    retire("p", "old").await?;
    restrict(s, "p", "held", "a hold").await?;
    let writes = json!({"default": "write"});
    let only = json!({"default": "write", "types": {"p": {"only": {"team": "a"}}}});
    let unread = json!({"default": "write", "types": {"dr": {"fields": {"r": "masked"}},
        "fr": {"fields": {"r": "none"}}}});
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
    ensure!(
        not_live(&called),
        "a verb's call that moves `dr` `w-live` to `gone`: {called:?}"
    );
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
    // A reference its write neither moves nor sends a field of is not
    // judged again; one whose field is sent again is, as a key sent again,
    // and so is one of a record retired and written again.
    for ty in ["dw", "dr", "fr"] {
        let field = if ty == "dw" { "r" } else { "s" };
        l.upsert(ty, "zr", json!({ field: "p2" })).await?;
        retire(ty, "zr").await?;
        l.upsert(ty, "z1", json!({ field: "p2" })).await?;
    }
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
