//! A retention hold withholds a record without erasing it
//! (`docs/log-format.md` §2.3, the store facade's `restrict`).
//!
//! A restricted record is absent for a principal without the
//! `restricted` grant and present, unchanged, for one with it; its history
//! stays readable at earlier positions; lifting the hold brings it back.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::write::{Outcome, ProposedWrite, WriteOpts};
use lakelands_api::{Access, Catalog, Error, LakeStore, Principal, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, alike_renamed, field, get_at, head_unmoved, ingest, patch, principal, restrict,
    source_record, type_decl,
};

check!(
    /// A restricted record is withheld from principals without the grant.
    HOLD_WITHHOLDS_AND_KEEPS = a_restricted_record_is_withheld_but_kept,
    "the facade: restrict hides a record from principals without the restricted grant, keeps it for those with it, keeps its history, and unrestrict lifts it"
);

check!(
    /// A restricted record is not there to a writer without the grant.
    HOLD_NOT_THERE_TO_A_WRITE = a_restricted_record_is_not_there_to_a_writer_without_the_grant,
    "006, extended (lakelands#772): to a writer without the `restricted` grant a restricted record is not there at any write door, its refusal compared whole with that of a key no record holds: a reference to it and either end of a `link` are `not_live`, `retire`, `unlink` of a link to it and a verb called on it are not found, and so is the proposal of each; a link to it ingested again is `not_live`; an upsert onto its key, made or proposed, and onto a held key retired since, made, proposed or ingested, is not granted and names no field; nothing is written; a reviewer without the grant is refused the accept of a write onto it, as not granted; the accept of a proposal is judged as its write as the records stand then, so one naming a record restricted since is refused, and one that reached a record restricted when proposed, as only a holder of the grant could, is applied"
);

async fn a_restricted_record_is_withheld_but_kept(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("file", vec![field("body", StorageKind::Text)]))
        .await?;
    let written = l.upsert("file", "f1", json!({"body": "kept"})).await?;
    let reader = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "reader",
        Grants {
            default: lakelands_api::Access::Read,
            ..Grants::default()
        },
    );
    let ty = TypeName::from("file");

    let held = store
        .restrict(&ty, &"f1".into(), "statutory retention".into())
        .await
        .must("restricting `file` `f1`")?;
    let hidden = cat.get(&reader, &ty, get_at("f1", None)).await;
    ensure!(
        matches!(hidden, Err(Error::NotFound { .. })),
        "a principal without the restricted grant must not find `f1` after the hold; it answered {hidden:?}"
    );
    let seen = cat
        .get(own, &ty, get_at("f1", None))
        .await
        .must("reading `f1` with the restricted grant")?;
    ensure!(
        seen.data.fields.get("body") == Some(&json!("kept")) && seen.position == held,
        "the owner must read `f1` unchanged at the hold's position {held}; it read {:?} at {}",
        seen.data.fields,
        seen.position
    );
    let then = cat
        .get(&reader, &ty, get_at("f1", Some(written.position())))
        .await
        .must("reading `f1` before the hold")?;
    ensure!(
        then.data.fields.get("body") == Some(&json!("kept")),
        "history before the hold stays readable: `f1` at {} must hold its body",
        written.position()
    );
    store
        .unrestrict(&ty, &"f1".into())
        .await
        .must("lifting the hold on `f1`")?;
    cat.get(&reader, &ty, get_at("f1", None))
        .await
        .must("reading `f1` after the hold is lifted")?;
    Ok(())
}

/// Each write door that names the record `k`, as `who`: a reference to a
/// `partner`, a link to and from it, its retirement, a verb on it, the
/// unlink of a link to it, the proposals of three of them, and a link to
/// it ingested again.
async fn write_doors(c: &dyn Catalog, who: &Principal, k: &str) -> Vec<lakelands_api::Result<()>> {
    fn said<T>(r: lakelands_api::Result<T>) -> lakelands_api::Result<()> {
        r.map(drop)
    }
    let (partner, deal, with, opts) = (
        &"partner".into(),
        &"deal".into(),
        &"with".into(),
        WriteOpts::default,
    );
    let (key, d1) = (&k.into(), &"d1".into());
    let refers = patch(json!({ "partner": k }));
    let call = |key| ProposedWrite::Call {
        verb: "rename".into(),
        key,
        payload: Default::default(),
        opts: opts(),
    };
    let retire = ProposedWrite::Retire {
        ty: "partner".into(),
        key: k.into(),
        opts: opts(),
    };
    let refer = ProposedWrite::Upsert {
        ty: "deal".into(),
        key: "n1".into(),
        patch: refers.clone(),
        opts: opts(),
    };
    vec![
        said(c.upsert(who, deal, &"n1".into(), refers, opts()).await),
        said(
            c.link(who, deal, d1, with, key, Default::default(), opts())
                .await,
        ),
        said(
            c.link(
                who,
                deal,
                key,
                with,
                &"open".into(),
                Default::default(),
                opts(),
            )
            .await,
        ),
        said(c.retire(who, partner, key, opts()).await),
        said(
            c.call(who, &"rename".into(), Some(key), Default::default(), opts())
                .await,
        ),
        said(c.unlink(who, deal, d1, with, key, opts()).await),
        said(c.propose(who, retire, "why".into()).await),
        said(c.propose(who, call(Some(k.into())), "why".into()).await),
        said(c.propose(who, refer, "why".into()).await),
        said(
            ingest(
                c,
                who,
                "feed",
                vec![source_record(
                    "sys.link.deal.fed",
                    &format!("d1:{k}"),
                    json!({"from": "d1", "to": k}),
                )],
            )
            .await,
        ),
    ]
}

async fn a_restricted_record_is_not_there_to_a_writer_without_the_grant(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let to_partner = json!({"kind": "reference", "target": "partner"});
    for d in [
        json!({"declare": "type", "name": "partner", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "deal", "fields": [{"name": "partner", "type": to_partner},
            {"name": "src", "type": {"kind": "text"}, "authority": "source"}],
            "relations": [{"name": "with", "target": "partner", "cardinality": "many_to_many"},
            {"name": "fed", "target": "partner", "cardinality": "many_to_many", "authority": "source"}]}),
        json!({"declare": "verb", "name": "rename", "on": "partner", "risk": "high",
            "effect": "write", "set": [{"field": "n", "value": "'q'"}]}),
    ] {
        l.declare_json(d).await?;
    }
    for k in ["held", "open", "late"] {
        l.upsert("partner", k, json!({ "n": k })).await?;
    }
    for k in ["held", "d1", "rr1", "rr2", "rr3"] {
        l.upsert("deal", k, json!({"partner": "open"})).await?;
    }
    let (deal, d1, with, held) = (&"deal".into(), &"d1".into(), &"with".into(), &"held".into());
    let linked = l.cat.link(
        &l.own,
        deal,
        d1,
        with,
        held,
        Default::default(),
        WriteOpts::default(),
    );
    linked.await.must("linking `d1` to `held`")?;
    let ends = json!({"from": "d1", "to": "held"});
    let fed = l.ingest(
        "feed",
        vec![source_record("sys.link.deal.fed", "d1:held", ends)],
    );
    fed.await.must("ingesting `d1` to `held`")?;
    restrict(s, "partner", "held", "a hold").await?;
    restrict(s, "deal", "held", "a hold").await?;
    // Each `rr` is held, then retired: one for each door, as a write that
    // went through would make it live.
    for k in ["rr1", "rr2", "rr3"] {
        restrict(s, "deal", k, "a hold").await?;
        let retired = l
            .cat
            .retire(&l.own, deal, &k.into(), WriteOpts::default())
            .await;
        retired.must("retiring a held `rr`")?;
    }
    let writer = |id: &str, restricted| {
        let grants = Grants {
            default: Access::Write,
            restricted,
            ingest: true,
            ..Grants::default()
        };
        principal(s, PrincipalKind::Agent, id, grants)
    };
    let (w, holder) = (writer("w", false), writer("holder", true));
    let (c, head) = (l.cat.as_ref(), s.head().await.must("the head")?);
    let mut apart = Vec::new();
    let there = write_doors(c, &w, "held").await;
    let gone = write_doors(c, &w, "gone").await;
    for (n, (a, b)) in there.iter().zip(&gone).enumerate() {
        let alike = match (a, b) {
            (Err(a), Err(b)) => alike_renamed(&format!("door {n}"), a, b, ("gone", "held")),
            _ => Err(Failure::new(format!(
                "door {n}: {a:?}; on a key no record holds: {b:?}"
            ))),
        };
        apart.extend(alike.err().map(|f| f.message));
    }
    let (partner, n2, z) = (&"partner".into(), &"n2".into(), || patch(json!({"n": "z"})));
    let (rr1, opts) = (&"rr1".into(), WriteOpts::default);
    let onto = |ty: &str, key: &str, fields| ProposedWrite::Upsert {
        ty: ty.into(),
        key: key.into(),
        patch: patch(fields),
        opts: opts(),
    };
    let refused = |answer: &lakelands_api::Result<()>| {
        matches!(
            answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        )
    };
    let fed = source_record("deal", "rr3", json!({"src": "x"}));
    for (what, answer) in [
        (
            "`held`, made",
            c.upsert(&w, partner, held, z(), opts()).await.map(drop),
        ),
        (
            "`held`, proposed",
            c.propose(&w, onto("partner", "held", json!({"n": "z"})), "why".into())
                .await
                .map(drop),
        ),
        (
            "`rr1`, made",
            c.upsert(&w, deal, rr1, patch(json!({"partner": "open"})), opts())
                .await
                .map(drop),
        ),
        (
            "`rr2`, proposed",
            c.propose(
                &w,
                onto("deal", "rr2", json!({"partner": "open"})),
                "why".into(),
            )
            .await
            .map(drop),
        ),
        (
            "`rr3`, ingested",
            ingest(c, &w, "feed", vec![fed]).await.map(drop),
        ),
    ] {
        if !refused(&answer) {
            apart.push(format!("an upsert onto {what}: {answer:?}"));
        }
    }
    let moved = head_unmoved(s, head, "a write to a restricted record").await;
    apart.extend(moved.err().map(|f| f.message));
    // At the accept: `late` is restricted after `w` proposed a reference
    // to it, and `held` was when `holder` did.
    let mut ids = Vec::new();
    for (who, key, to) in [(&w, "n2", "late"), (&holder, "n3", "held")] {
        let refers = patch(json!({ "partner": to }));
        let write = ProposedWrite::Upsert {
            ty: "deal".into(),
            key: key.into(),
            patch: refers,
            opts: WriteOpts::default(),
        };
        match c
            .propose(who, write, "why".into())
            .await
            .must("a proposal")?
        {
            Outcome::Proposed { proposal, .. } => ids.push(proposal),
            other => return Err(Failure::new(format!("a proposal answered {other:?}"))),
        }
    }
    restrict(s, "partner", "late", "a hold").await?;
    let direct = c.upsert(
        &w,
        deal,
        n2,
        patch(json!({"partner": "late"})),
        WriteOpts::default(),
    );
    let (direct, late) = (direct.await, c.accept(&l.own, &ids[0]).await);
    let same = |a: &Error, b: &Error| alike_renamed("the accept", a, b, ("", ""));
    if !matches!((&late, &direct), (Err(a), Err(b)) if same(a, b).is_ok()) {
        apart.push(format!(
            "the accept naming `late`: {late:?}; the write: {direct:?}"
        ));
    }
    let accepted = c.accept(&l.own, &ids[1]).await;
    if !matches!(accepted, Ok(Outcome::Applied { .. })) {
        apart.push(format!("the holder's accepted proposal: {accepted:?}"));
    }
    // A reviewer without the grant decides no proposal on a held record.
    let said = c
        .propose(
            &holder,
            onto("partner", "held", json!({"n": "z"})),
            "why".into(),
        )
        .await;
    let Outcome::Proposed { proposal, .. } = said.must("the holder's proposal")? else {
        return Err(Failure::new("the holder's write onto `held` is proposed"));
    };
    let mut reviewer = writer("reviewer", false);
    reviewer.grants.review = true;
    let decided = c.accept(&reviewer, &proposal).await.map(drop);
    if !refused(&decided) {
        apart.push(format!(
            "a reviewer without the grant accepts onto `held`: {decided:?}"
        ));
    }
    ensure!(
        apart.is_empty(),
        "a restricted record is not there to a writer without the grant; apart:\n{}",
        apart.join("\n")
    );
    Ok(())
}
