//! An entry is decided as its reviewer is listed it
//! (`docs/decisions/006-grants-not-roles.md` and
//! `013-verbs-run-by-declared-risk.md`, each extended).

use super::only::{de, holder};
use crate::conformance::prelude::*;

check!(
    DECIDED_AS_LISTED = an_entry_is_decided_as_its_reviewer_is_listed_it,
    "006 and 013, extended (lakelands#791): the listing, `accept` and `reject` ask one judgement of an entry, so over a grid of proposed writes (an upsert, a retirement, a call, a migration, a bundle, one decided, and an id that was never proposed) and reviewers (kept from a type, holding `only`, kept from a field, masked on one, shown three types, shown all) an entry that is not listed is to its accept and its reject an id that does not exist, word for word, appending nothing: a write to a type its reader is not shown, a call whose verb reads a field it may not read, as it is declared or as it was where the call was proposed, a migration to a holder of `only`, one that keys its type by a field its reader may not know of, one of a type declared again since that its reader was not shown where it was proposed, and a bundle that brings a type its reader would not be shown, which its proposer of that kind is not listed either; a write onto another's record is not listed to a holder of `only` (lakelands#1016), and a declaration listed whole to it is not granted to accept or to reject; one listed with `more` to a reviewer kept from a field or masked on one is not granted to accept (lakelands#867); a migration of a type a field of which was renamed since is judged, for a reader kept from that field, on the type as it was proposed against; a call's payload is listed only to one who reads each field its verb sets, as it is declared and as it was where the call was proposed; and a call whose verb would take the record out of its reviewer's `only` is listed to it without its write, its accept and its reject not granted, the listing judging the record as the call would leave it (lakelands#914)"
);

check!(
    SHOWN_SINCE_A_REKEY = an_entry_proposed_before_its_type_was_shown_is_not_there,
    "006 and 013, extended (lakelands#913): an entry is judged as it was proposed against too where a type its type rests on was re-keyed since: `xx` keyed by a reference to `hh`, keyed by a field its reviewer may not know of, is shown to it once `hh` is re-keyed to a field it knows, and an upsert, a retirement, a call and a migration of `xx` proposed before, whose key is that field's value, are not there to it: not listed, and accepted or rejected answered as an id never proposed is, word for word, appending nothing; an upsert of `yy`, which it was shown, whose reference to `hh` holds the old key, is listed to it without that reference and is not granted to accept; the owner and a reviewer that reads every field are listed all five; only a re-key moves an old key, so only a re-key asks the past"
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
/// without its write, `T` with `more`, so not its to accept, `W` whole)
/// and what its accept answers, the reviewers asked in this order (`-` as
/// an id that does not exist, `g` not granted, `a` applied, `d` decided,
/// `c` changed since it was proposed, `v` its verb declared again).
const GRID: [(&str, &str, &str); 6] = [
    (
        "far",
        r#"{"types": {"doc": {"default": "none"}}}"#,
        "H- H- H- H- H- H- H- Wc Wc H- H- H- H- H- Wc H- H- H-",
    ),
    (
        "only",
        r#"{"types": {"doc": {"only": {"owner": "me"}}}}"#,
        "Wa H- Wa Wa Wa Wv H- H- H- Wg Wg Wd Wv Wv H- H- Wv Ng",
    ),
    (
        "kept",
        r#"{"types": {"doc": {"fields": {"secret": "none"}}, "memo": {"fields": {"a": "none"}},
                      "note": {"fields": {"code": "none"}}, "slip": {"fields": {"code": "none"}}}}"#,
        "Tg Wa Wd Wd H- H- Tg Tg H- Wa Wa Wd H- Tg H- H- Tg Wa",
    ),
    (
        "masked",
        r#"{"types": {"doc": {"fields": {"secret": "masked"}}}}"#,
        "Tg Wd Wd Wd H- H- Wa Wc Wc Wd Wd Wd H- Tg Wc H- Tg Wd",
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
        let mut grants =
            json!({"default": "read", "review": true, "schema": true, "restricted": true});
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
            let accept = said(l.cat.accept(&who, id).await, id);
            // One not listed, or not its reviewer's to decide, is refused
            // its reject in the accept's words, and nothing is appended.
            // One listed with `more` is its reviewer's to reject, which is
            // left to `a_reviewer_accepts_only_what_it_is_listed_whole`, so
            // that the entry waits for the rows below.
            if cell == 'H' || (accept == "g" && cell != 'T') {
                let reject = said(l.cat.reject(&who, id, "no".into()).await, id);
                if reject != accept {
                    apart.push(format!(
                        "`{name}` and {entry}: its reject answered {reject}"
                    ));
                }
                head_unmoved(s, before, "the accept and the reject of such an entry").await?;
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

async fn an_entry_proposed_before_its_type_was_shown_is_not_there(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "required": matches!(n, "secret" | "open")});
    let hh =
        |key: &str| json!({"name": "hh", "key": [key], "fields": [text("secret"), text("open")]});
    let h = json!({"name": "h", "type": {"kind": "reference", "target": "hh"}});
    let xx = |fields: Value| json!({"name": "xx", "key": ["h"], "fields": fields});
    let declared = |mut t: Value| {
        t["declare"] = json!("type");
        t
    };
    l.declare_json(declared(hh("secret"))).await?;
    l.declare_json(declared(xx(json!([h, text("note")]))))
        .await?;
    // `yy` is keyed by its own `k` and refers to `hh` by `h`.
    let yy = json!({"name": "yy", "key": ["k"], "fields": [text("k"), h.clone(), text("note")]});
    l.declare_json(declared(yy)).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "stamp", "on": "xx", "risk": "high",
                          "effect": "write", "set": [{"field": "note", "value": "'x'"}]}),
    )
    .await?;
    for (ty, fields) in [
        ("hh", json!({"secret": "s1", "open": "o1"})),
        ("xx", json!({"h": "s1"})),
    ] {
        l.upsert(ty, "s1", fields).await?;
    }
    // The owner proposes four writes of `xx`, keyed by `hh.secret`'s value.
    let propose = async |w: Value| id(l.cat.propose(&l.own, de(w), "mine".into()).await, "mine");
    let to = xx(json!([h, text("note"), text("more")]));
    let (key, none, opts) = (Some("s1".into()), Default::default(), Default::default());
    let call = l
        .cat
        .call(&l.own, &"stamp".into(), key.as_ref(), none, opts)
        .await;
    let mut entries = vec![id(call, "the call")?];
    for w in [
        json!({"write": "upsert", "type": "xx", "key": "s1", "patch": {"note": "u"}}),
        json!({"write": "retire", "type": "xx", "key": "s1"}),
        json!({"write": "migrate", "migration": {"to": to}}),
        json!({"write": "upsert", "type": "yy", "key": "a", "patch": {"h": "s1"}}),
    ] {
        entries.push(propose(w).await?);
    }
    l.migrate(de(hh("open"))).await?;
    let reviewer = |name: &str, more: Value| {
        let grants = json!({"default": "read", "review": true, "schema": true, "types": more});
        holder(s, name, grants)
    };
    let kept = reviewer("kept", json!({"hh": {"fields": {"secret": "none"}}}));
    let shown = l
        .cat
        .describe_type(&kept, &"xx".into(), Default::default())
        .await;
    shown.must("`kept` describing `xx` once `hh` is keyed by `open`")?;
    for (who, name) in [
        (&l.own, "the owner"),
        (&reviewer("all", json!({})), "`all`"),
    ] {
        let listed = l.cat.proposals(who, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        let all = entries.iter().all(|id| listed.iter().any(|p| &p.id == id));
        ensure!(all, "{name} is listed every entry: {listed:?}");
    }
    // `yy`'s upsert is listed without `h`, whose value is `hh`'s old key,
    // and so is not `kept`'s to accept: the refusal names nothing.
    let listed = l.cat.proposals(&kept, ProposalFilter::default()).await;
    let listed = listed.must("`kept` listing the proposals")?.data;
    let refers = entries[4].clone();
    let without = |p: &&lakelands_api::write::Proposal| {
        let patch = json!({"write": "upsert", "type": "yy", "key": "a", "patch": {"k": "a"}});
        p.id == refers && p.more && p.write == Some(de(patch))
    };
    let accepted = l.cat.accept(&kept, &refers).await;
    let refused = Err(lakelands_api::Error::Refused {
        reason: lakelands_api::Refusal::NotGranted,
        field: None,
    });
    ensure!(
        listed.len() == 1 && listed.iter().all(|p| without(&p)) && accepted == refused,
        "`kept` was not shown `xx` where its entries were proposed, and is listed none of them, \
         and `yy`'s without `h`, not granted to accept ({accepted:?}); it is listed {listed:?}"
    );
    let unknown = ProposalId::from("none");
    let said = |r: lakelands_api::Result<Outcome>, id: &ProposalId| {
        format!("{:?}", r.map(|_| ())).replace(id.as_str(), "none")
    };
    let before = s.head().await.must("the head")?;
    let never = (
        said(l.cat.accept(&kept, &unknown).await, &unknown),
        said(l.cat.reject(&kept, &unknown, "no".into()).await, &unknown),
    );
    for id in &entries[..4] {
        let decided = (
            said(l.cat.accept(&kept, id).await, id),
            said(l.cat.reject(&kept, id, "no".into()).await, id),
        );
        ensure!(
            decided == never,
            "`kept`'s accept and reject of {id} answer as of an id never proposed, {never:?}; \
             they answered {decided:?}"
        );
    }
    head_unmoved(s, before, "`kept`'s accepts and rejects").await
}
