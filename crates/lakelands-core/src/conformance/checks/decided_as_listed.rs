//! An entry is decided as its reviewer is listed it
//! (`docs/decisions/006-grants-not-roles.md` and
//! `013-verbs-run-by-declared-risk.md`, each extended).

use lakelands_api::write::ProposalFilter;
use lakelands_api::{Error, Missing, Outcome, ProposalId};
use serde_json::{Value, json};

use super::only::{de, holder};
use crate::conformance::fixture::{Lake, Must, head_unmoved, trigger_on};
use crate::conformance::{Failure, Held, Store};

check!(
    DECIDED_AS_LISTED = an_entry_is_decided_as_its_reviewer_is_listed_it,
    "006 and 013, extended (lakelands#791): the listing, `accept` and `reject` ask one judgement of an entry, so over a grid of proposed writes (an upsert, a retirement, a call, a migration, a bundle, one decided, and an id that was never proposed) and reviewers (kept from a type, holding `only`, kept from a field, masked on one, shown three types, shown all) an entry that is not listed is to its accept and its reject an id that does not exist, word for word, appending nothing: a write to a type its reader is not shown, a call whose verb reads a field it may not read, as it is declared or as it was where the call was proposed, a migration to a holder of `only`, one that keys its type by a field its reader may not know of, one of a type declared again since that its reader was not shown where it was proposed, and a bundle that brings a type its reader would not be shown, which its proposer of that kind is not listed either; one listed without its write to a holder of `only`, and a declaration listed whole to it, is not granted to accept or to reject; a migration of a type a field of which was renamed since is judged, for a reader kept from that field, on the type as it was proposed against; a call's payload is listed only to one who reads each field its verb sets, as it is declared and as it was where the call was proposed; and, as it stands, a call whose verb would take the record out of its reviewer's `only` is listed whole to it, its accept not granted and its reject taken, the listing not judging the record as the call would leave it"
);

/// The entries, in the grid's order.
const ENTRIES: [&str; 18] = [
    "an upsert of `d1`, its own",
    "an upsert of `d4`, another's",
    "a retirement of `d2`",
    "a call of `stamp`",
    "a call of `seal`, which reads `secret`",
    "a call of `open`, which read `secret` where it was proposed",
    "a migration of `doc` that adds a field",
    "a migration of `memo`, whose `a` was renamed since",
    "a migration that keys `note`, which moved since, by `code`",
    "a bundle that brings a type `ledger`",
    "a bundle of a trigger that reads `secret`",
    "a decided upsert of `d3`",
    "a call of `shut`, which reads `secret` since",
    "a call of `file`, which set `secret` where it was proposed",
    "a migration that keys `slip` by `at`, proposed while `code` keyed it",
    "an id that was never proposed",
    "a call of `lock`, which sets `secret` since",
    "a call of `give`, which sets `owner`",
];

/// Each reviewer with what its grants add to `read`, `review` and
/// `schema`, and, for each entry, how it is listed it (`H` not at all, `N`
/// without its write, `T` with `more`, `W` whole) and what its accept
/// answers, the reviewers asked in this order (`-` as an id that does not
/// exist, `g` not granted, `r` not granted and its reject taken, `a`
/// applied, `d` decided, `c` changed since it was proposed, `v` its verb
/// declared again).
const GRID: [(&str, &str, &str); 6] = [
    (
        "far",
        r#"{"types": {"doc": {"default": "none"}}}"#,
        "H- H- H- H- H- H- H- Wc Wc H- H- H- H- H- Wc H- H- H-",
    ),
    (
        "only",
        r#"{"types": {"doc": {"only": {"owner": "me"}}}}"#,
        "Wa Ng Wa Wa Wa Wv H- H- H- Wg Wg Wd Wv Wv H- H- Wv Wr",
    ),
    (
        "kept",
        r#"{"types": {"doc": {"fields": {"secret": "none"}}, "memo": {"fields": {"a": "none"}},
                      "note": {"fields": {"code": "none"}}, "slip": {"fields": {"code": "none"}}}}"#,
        "Td Wa Wd Wd H- H- Ta Tc H- Wa Wa Wd H- Tv H- H- Tv Wd",
    ),
    (
        "masked",
        r#"{"types": {"doc": {"fields": {"secret": "masked"}}}}"#,
        "Td Wd Wd Wd H- H- Wd Wc Wc Wd Wd Wd H- Tv Wc H- Tv Wd",
    ),
    (
        "typed",
        r#"{"default": "none", "types": {"doc": {"default": "read"}, "memo": {"default": "read"},
                                         "note": {"default": "read"}}}"#,
        "Wd Wd Wd Wd Wd Wv Wd Wc Wc H- Wd Wd Wv Wv H- H- Wv Wd",
    ),
    (
        "all",
        "{}",
        "Wd Wd Wd Wd Wd Wv Wd Wc Wc Wd Wd Wd Wv Wv Wc H- Wv Wd",
    ),
];

fn id(o: lakelands_api::Result<Outcome>, what: &str) -> Result<ProposalId, Failure> {
    match o.must(what)? {
        Outcome::Proposed { proposal, .. } => Ok(proposal),
        other => Err(Failure::new(format!("{what} answered {other:?}"))),
    }
}

async fn an_entry_is_decided_as_its_reviewer_is_listed_it(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let ty = |name: &str, fields: &[&str], declare: bool| {
        let text = |n: &&str| json!({"name": n, "type": {"kind": "text"}, "required": matches!(*n, "code" | "at")});
        let fields = fields.iter().map(text).collect::<Vec<_>>();
        let mut ty = json!({"name": name, "fields": fields});
        if declare {
            ty["declare"] = json!("type");
        }
        ty
    };
    for (name, fields) in [
        ("doc", &["title", "secret", "owner"][..]),
        ("memo", &["a", "b"]),
        ("note", &["n", "code"]),
    ] {
        l.declare_json(ty(name, fields, true)).await?;
    }
    let slip = |declare: bool, key: &str| {
        let mut slip = ty("slip", &["at", "code"], declare);
        slip["key"] = json!([key]);
        slip
    };
    l.declare_json(slip(true, "code")).await?;
    let verb = |name: &str, requires: Option<&str>| {
        json!({"declare": "verb", "name": name, "on": "doc", "risk": "high", "effect": "write",
               "requires": requires, "set": [{"field": "title", "value": "'x'"}]})
    };
    let reads = Some("secret is not null");
    for (name, requires) in [
        ("stamp", None),
        ("seal", reads),
        ("open", reads),
        ("shut", None),
    ] {
        l.declare_json(verb(name, requires)).await?;
    }
    let file = |name: &str, field: &str| {
        let mut file = verb(name, None);
        file["payload"] = json!([{"name": "x", "type": {"kind": "text"}}]);
        file["set"] = json!([{"field": field, "value": "payload.x"}]);
        file
    };
    for (name, field) in [("file", "secret"), ("lock", "title"), ("give", "owner")] {
        l.declare_json(file(name, field)).await?;
    }
    for (key, owner) in [("d1", "me"), ("d2", "me"), ("d3", "me"), ("d4", "other")] {
        let fields = json!({"title": "t", "secret": "s", "owner": owner});
        l.upsert("doc", key, fields).await?;
    }
    let propose = async |write: Value| {
        let proposed = l.cat.propose(&l.own, de(write), "the owner's".into()).await;
        id(proposed, "the owner's proposal")
    };
    let call = async |verb: &str| {
        let (verb, key, none, opts) = (
            verb.into(),
            "d3".into(),
            Default::default(),
            Default::default(),
        );
        id(
            l.cat.call(&l.own, &verb, Some(&key), none, opts).await,
            "the owner's call",
        )
    };
    let filed =
        |verb: &str| json!({"write": "call", "verb": verb, "key": "d3", "payload": {"x": "v"}});
    let upsert = |key: &str, patch: Value| json!({"write": "upsert", "type": "doc", "key": key, "patch": patch});
    let migrate = |to: Value| json!({"write": "migrate", "migration": {"to": to}});
    let mut keyed = ty("note", &["n", "code"], false);
    keyed["key"] = json!(["code"]);
    let bundle = |d: Vec<Value>| json!({"write": "declare", "declarations": d});
    let brings = vec![
        ty("ledger", &["n"], true),
        json!(trigger_on("watch", "doc", None)),
    ];
    let guard = vec![json!(trigger_on("guard", "doc", Some("secret = 'x'")))];
    let entries = [
        propose(upsert("d1", json!({"title": "u", "secret": "v"}))).await?,
        propose(upsert("d4", json!({"title": "u"}))).await?,
        propose(json!({"write": "retire", "type": "doc", "key": "d2"})).await?,
        call("stamp").await?,
        call("seal").await?,
        call("open").await?,
        propose(migrate(ty(
            "doc",
            &["title", "secret", "owner", "extra"],
            false,
        )))
        .await?,
        propose(migrate(ty("memo", &["a", "b", "c"], false))).await?,
        propose(migrate(keyed)).await?,
        propose(bundle(brings.clone())).await?,
        propose(bundle(guard)).await?,
        propose(upsert("d3", json!({"title": "w"}))).await?,
        call("shut").await?,
        propose(filed("file")).await?,
        propose(migrate(slip(false, "at"))).await?,
        ProposalId::from("none"),
        propose(filed("lock")).await?,
        propose(filed("give")).await?,
    ];
    // Moved since: `open` without its `requires` and `shut` with it, `file`
    // setting `title` and `lock` setting `secret`, `memo` with `a` renamed,
    // `note` with a field more and `slip` keyed by `at`; and one upsert
    // decided.
    l.declare_json(verb("open", None)).await?;
    l.declare_json(verb("shut", reads)).await?;
    l.declare_json(file("file", "title")).await?;
    l.declare_json(file("lock", "secret")).await?;
    l.migrate(de(ty("note", &["n", "code", "m"], false)))
        .await?;
    l.migrate(de(slip(false, "at"))).await?;
    l.migrate_renaming(de(ty("memo", &["a2", "b"], false)), &[("a", "a2")])
        .await?;
    (l.cat.accept(&l.own, &entries[11]).await).must("the owner accepting an upsert of `d3`")?;
    let absent = |id: &ProposalId| {
        let missing = Missing::Proposal { id: id.clone() };
        Error::NotFound { missing }
    };
    let said = |a: lakelands_api::Result<Outcome>, id: &ProposalId| match a {
        Ok(Outcome::Applied { .. }) => "a".to_owned(),
        Ok(o) => format!("{o:?}"),
        Err(e) if e == absent(id) => "-".to_owned(),
        Err(e) => {
            let e = json!(e);
            let code = e["reason"]["code"]
                .as_str()
                .or(e["reason"]["refusal"].as_str());
            match code.or(e["error"].as_str()).unwrap_or("?") {
                "not_granted" => "g".to_owned(),
                "decided" => "d".to_owned(),
                "changed_since_proposed" => "c".to_owned(),
                "verb_redeclared" => "v".to_owned(),
                other => other.to_owned(),
            }
        }
    };
    let (mut apart, mut got) = (Vec::new(), Vec::new());
    for (name, more, want) in GRID {
        let mut grants = json!({"default": "read", "review": true, "schema": true});
        let more: Value = serde_json::from_str(more).expect("the grid's grants read");
        if let (Some(g), Value::Object(more)) = (grants.as_object_mut(), more) {
            g.extend(more);
        }
        let who = holder(s, name, grants);
        let listed = l.cat.proposals(&who, ProposalFilter::default()).await;
        let listed = listed.must("listing proposals")?.data;
        let mut row = Vec::new();
        for (entry, id) in ENTRIES.iter().zip(&entries) {
            let cell = match listed.iter().find(|p| &p.id == id) {
                None => 'H',
                Some(p) if p.write.is_none() => 'N',
                Some(p) if p.more => 'T',
                Some(_) => 'W',
            };
            let before = s.head().await.must("the head")?;
            let mut accept = said(l.cat.accept(&who, id).await, id);
            // One not listed, or not its reviewer's to decide, is refused
            // its reject in the accept's words, and nothing is appended;
            // a reject taken there is the grid's `r`.
            if cell == 'H' || accept == "g" {
                let reject = l.cat.reject(&who, id, "no".into()).await;
                if accept == "g" && reject.is_ok() {
                    accept = "r".to_owned();
                } else {
                    let reject = said(reject, id);
                    if reject != accept {
                        apart.push(format!(
                            "`{name}` and {entry}: its reject answered {reject}"
                        ));
                    }
                    head_unmoved(s, before, "the accept and the reject of such an entry").await?;
                }
            }
            row.push(format!("{cell}{accept}"));
        }
        let row = row.join(" ");
        if row != want {
            apart.push(format!("`{name}`: {row}, where {want} is due"));
        }
        got.push(format!("{name:7} {row}"));
    }
    ensure!(
        apart.is_empty(),
        "\nGRID\n{}\n{}",
        got.join("\n"),
        apart.join("\n")
    );
    // One who proposes a type it will not be shown is not listed its own.
    let maker = holder(
        s,
        "maker",
        json!({"default": "none", "types": {"doc": {"default": "write"}}}),
    );
    let mut again = brings;
    again[0]["name"] = json!("ledger2");
    let proposed = l
        .cat
        .propose(&maker, de(bundle(again)), "mine".into())
        .await;
    let mine = id(proposed, "`maker` proposing a bundle that brings a type")?;
    let listed = l.cat.proposals(&maker, ProposalFilter::default()).await;
    let listed = listed.must("`maker` listing its proposals")?.data;
    ensure!(
        listed.is_empty(),
        "`maker` proposed {mine}, a bundle that brings a type it would not be shown, and is \
         not listed it; it is listed {listed:?}"
    );
    Ok(())
}
