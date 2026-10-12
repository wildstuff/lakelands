//! A reviewer accepts only what it is listed whole, and rejects what it is
//! listed in part (`docs/decisions/006-grants-not-roles.md`,
//! `013-verbs-run-by-declared-risk.md` and
//! `027-shape-changes-are-declared-or-migrated.md`, each extended).

use lakelands_api::write::{ProposalFilter, ProposedWrite};
use lakelands_api::{Error, Missing, Outcome, ProposalId, Refusal};
use serde_json::{Value, json};

use super::only::{de, holder};
use crate::conformance::backing;
use crate::conformance::fixture::{Lake, Must, migration};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    ACCEPTED_WHOLE = a_reviewer_accepts_only_what_it_is_listed_whole,
    "006, 013 and 027, extended (lakelands#867): a reviewer accepts a proposal only where its listing shows it whole, and one listed it with `more` is refused the accept as not granted, naming nothing, and rejects it: over an upsert and a call that set a field, a migration of its type, a proposed set of a verb that sets it, an object set that reads it and a type, a tightening of a value type proposed by one kept from a field that uses it, an upsert of another type, and the renaming of a value a set names, proposed by one kept from the set's type and by the owner, a reviewer kept from the field is listed the upsert, the call and the migration in part, masked on it the upsert and the call, and both are listed the set whole, as `describe` shows a holder of `schema` each definition (lakelands#798); without `schema` one kept from the field is listed the set without what it may not be shown; one not shown the other type is not listed its upsert, and accepts and rejects it as an id that does not exist; a reviewer shown everything is listed each whole, the tightening and both renamings too, as a reader shown everything is shown the report whatever its proposer was, and accepts each, while one kept from the set's type is listed both renamings in part",
    given
);

/// The entries, in the grid's order.
const ENTRIES: [&str; 8] = [
    "an upsert of `salary`",
    "a call of `set_salary`",
    "a proposed set of `zero`, `rich` and `memo`",
    "a migration of `invoice`",
    "a tightening of `grade`",
    "an upsert of `vault`",
    "a renaming of `tier`'s `mid`, by one kept from `secret`",
    "a renaming of `rank`'s `mid`, by the owner",
];

/// Each reviewer, its grants beside `review`, and for each entry how it is
/// listed it (`H` not at all, `T` with `more`, `W` whole), what its accept
/// answers (`a` applied, `g` not granted, `-` as an id that does not
/// exist) and what its reject of another proposal of it answers (`r`
/// taken, `-` as an id that does not exist).
const GRID: [(&str, &str, &str); 7] = [
    (
        "whole",
        r#"{"schema": true}"#,
        "War War War War War War War War",
    ),
    (
        "kept",
        r#"{"schema": true, "types": {"invoice": {"fields": {"salary": "none"}}}}"#,
        "Tgr Tgr War Tgr War War War War",
    ),
    (
        "masked",
        r#"{"schema": true, "types": {"invoice": {"fields": {"salary": "masked"}}}}"#,
        "Tgr Tgr War War War War War War",
    ),
    (
        "unseeing",
        r#"{"schema": true, "types": {"vault": {"default": "none"}}}"#,
        "War War War War Tgr H-- War War",
    ),
    (
        "secretless",
        r#"{"schema": true, "types": {"secret": {"default": "none"}}}"#,
        "War War War War War War Tgr Tgr",
    ),
    ("plain", "{}", "War War Wgr Wgr Wgr War Wgr Wgr"),
    (
        "plain_kept",
        r#"{"types": {"invoice": {"fields": {"salary": "none"}}}}"#,
        "Tgr Tgr Tgr Tgr Wgr War Wgr Wgr",
    ),
];

/// Declares the lake and proposes each entry twice, as the owner but the
/// tightening, which `clerk`, kept from `vault.level`, proposes, and the
/// renaming of `tier`, which `blind`, kept from `secret`, proposes: the
/// set `omids` of `secret` names the value both renamings rename.
async fn proposed(
    l: &Lake,
    store: &dyn lakelands_api::LakeStore,
) -> Result<Vec<[ProposalId; 2]>, Failure> {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    for name in ["grade", "tier", "rank"] {
        l.declare_json(
            json!({"declare": "value_type", "name": name, "kind": "text",
                              "one_of": ["low", "high", "mid"]}),
        )
        .await?;
    }
    let level = json!({"name": "level", "type": {"kind": "text", "value_type": "grade"}});
    for d in [
        json!({"declare": "type", "name": "invoice", "fields": [text("no"), text("note"), text("salary")]}),
        json!({"declare": "type", "name": "vault", "fields": [text("n"), level]}),
        json!({"declare": "verb", "name": "set_salary", "on": "invoice", "risk": "high",
               "payload": [text("amount")], "effect": "write",
               "set": [{"field": "salary", "value": "payload.amount"}]}),
        json!({"declare": "type", "name": "secret", "fields": [text("s")]}),
        json!({"declare": "object_set", "name": "omids", "of": "secret", "where": "s = 'mid'"}),
    ] {
        l.declare_json(d).await?;
    }
    l.upsert(
        "invoice",
        "i1",
        json!({"no": "1", "note": "n", "salary": "100"}),
    )
    .await?;
    l.upsert("vault", "v1", json!({"n": "x", "level": "low"}))
        .await?;
    let set = json!({"write": "declare", "declarations": [
        {"declare": "verb", "name": "zero", "on": "invoice", "risk": "high", "effect": "write",
         "set": [{"field": "salary", "value": "'0'"}]},
        {"declare": "object_set", "name": "rich", "of": "invoice", "where": "salary > '1'"},
        {"declare": "type", "name": "memo", "fields": [text("t")]}]});
    let to = json!({"name": "invoice", "fields": [text("no"), text("note"), text("salary"), text("extra")]});
    let tightening = ProposedWrite::MigrateValueType {
        to: Box::new(de(
            json!({"name": "grade", "kind": "text", "one_of": ["low", "high"]}),
        )),
        renames: Vec::new(),
        report: Default::default(),
    };
    let clerk = json!({"default": "write", "types": {"vault": {"fields": {"level": "none"}}}});
    let clerk = holder(store, "clerk", clerk);
    let blind = json!({"default": "write", "types": {"secret": {"default": "none"}}});
    let blind = holder(store, "blind", blind);
    let renaming = |name: &str| ProposedWrite::MigrateValueType {
        to: Box::new(de(
            json!({"name": name, "kind": "text", "one_of": ["low", "high", "medium"]}),
        )),
        renames: vec![("mid".into(), "medium".into())],
        report: Default::default(),
    };
    let writes = [
        de(json!({"write": "upsert", "type": "invoice", "key": "i1",
                  "patch": {"note": "raise", "salary": "250"}})),
        de(json!({"write": "call", "verb": "set_salary", "key": "i1",
                  "payload": {"amount": "300"}})),
        de(set),
        ProposedWrite::Migrate {
            migration: Box::new(migration(de(to))),
            report: Default::default(),
        },
        tightening,
        de(json!({"write": "upsert", "type": "vault", "key": "v1", "patch": {"n": "y"}})),
        renaming("tier"),
        renaming("rank"),
    ];
    let mut ids = Vec::new();
    for (i, write) in writes.into_iter().enumerate() {
        let who = match i {
            4 => &clerk,
            6 => &blind,
            _ => &l.own,
        };
        let mut pair = Vec::new();
        for _ in 0..2 {
            match l
                .cat
                .propose(who, write.clone(), "mine".into())
                .await
                .must(ENTRIES[i])?
            {
                Outcome::Proposed { proposal, .. } => pair.push(proposal),
                other => return Err(Failure::new(format!("{}: {other:?}", ENTRIES[i]))),
            }
        }
        ids.push([pair[0].clone(), pair[1].clone()]);
    }
    Ok(ids)
}

/// What an accept or a reject answers, as the grid writes it.
fn said(answer: lakelands_api::Result<Outcome>, id: &ProposalId, taken: char) -> char {
    let missing = Missing::Proposal { id: id.clone() };
    let refused = Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    };
    match answer {
        Ok(_) => taken,
        Err(e) if e == (Error::NotFound { missing }) => '-',
        Err(e) if e == refused => 'g',
        Err(_) => '?',
    }
}

async fn a_reviewer_accepts_only_what_it_is_listed_whole(store: Store, given: Given) -> Held {
    let lake = store.lake().clone();
    let mut own = Some(store);
    let (mut apart, mut got) = (Vec::new(), Vec::new());
    for (name, grants, want) in GRID {
        let store = match own.take() {
            Some(store) => store,
            None => {
                let seams = lakelands_api::seam::Seams {
                    backings: backing::backings(),
                    executors: backing::executors(),
                    ..Default::default()
                };
                let twin = (given.twin)(lake.clone(), seams).await;
                twin.map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?
            }
        };
        let l = Lake::of(store.as_ref());
        let ids = proposed(&l, store.as_ref()).await?;
        let mut g = json!({"default": "read", "review": true});
        let more: Value = serde_json::from_str(grants).expect("the grid's grants read");
        if let (Some(g), Value::Object(more)) = (g.as_object_mut(), more) {
            g.extend(more);
        }
        let who = holder(store.as_ref(), name, g);
        let listed = l.cat.proposals(&who, ProposalFilter::default()).await;
        let listed = listed.must("listing proposals")?.data;
        let mut row = Vec::new();
        for [first, second] in &ids {
            let cell = match listed.iter().find(|p| &p.id == first) {
                None => 'H',
                Some(p) if p.more => 'T',
                Some(_) => 'W',
            };
            let accept = said(l.cat.accept(&who, first).await, first, 'a');
            let reject = l.cat.reject(&who, second, "no".into()).await;
            row.push(format!("{cell}{accept}{}", said(reject, second, 'r')));
        }
        let row = row.join(" ");
        if row != want {
            apart.push(format!("`{name}`: {row}, where {want} is due"));
        }
        got.push(format!("{name:10} {row}"));
        store.close().await.must("closing a lake")?;
    }
    ensure!(
        apart.is_empty(),
        "a reviewer accepts only what it is listed whole and rejects what it is listed in part; \
         by entry ({}):\n{}\n{}",
        ENTRIES.join(", "),
        got.join("\n"),
        apart.join("\n")
    );
    Ok(())
}
