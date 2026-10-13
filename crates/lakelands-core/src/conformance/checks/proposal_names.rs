//! A proposal names only what its reader may know of, and says that there
//! is more (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md` and
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`, each
//! extended): a listing shows a name where `describe` would, leaves the
//! write out where its reader may see nothing of it, and answers its
//! filters on what it shows.

use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, LakeStore, Principal, WriteOpts};
use serde_json::{Value, json};

use super::rekey::{IBANS, lake, to_iban};
use crate::conformance::fixture::{
    Lake, Must, field_access, object_set_on, patch, refused_like, reviewer, trigger_on,
};
use crate::conformance::{Failure, Held, Store};

check!(
    NAMES_AS_THE_READERS_VIEW = a_proposed_shape_names_only_what_its_reader_may_know_of,
    "013, 024 and 027, extended (lakelands#499): `proposals` shows of a proposed migration the renames, the dropped fields and link fields, the re-keyed types and the rewritten references only where its reader may see the type and know of the field, the totals whole; of a proposed bundle each declaration `describe` would show its reader once in force, a verb by the fields it names; an entry that holds more says `more` and names nothing else, and a bundle with nothing left shows no write, as one of a view whose definition its reader may not read, whose type `describe` lists once in force with the definition withheld (lakelands#847); a migration or a bundle that names a type its reader may not see is not listed (lakelands#762); a valid time over a field its reader may not know of is named neither in a listed declaration nor in a migration's `to` and report (lakelands#726)"
);

check!(
    LISTED_AS_SHOWN = a_listing_is_filtered_on_what_it_shows_and_an_accept_names_no_hidden_field,
    "013 and 024, extended (lakelands#499): a proposal on a restricted record is not listed to a reader without the grant (lakelands#1008), nor one on a type its reader may not see (lakelands#762); `type` and `kind` are answered on the entry as shown, a bundle that moves a verb or a trigger to another type found by the type it leaves as by the one it moves to (lakelands#916), so a hidden type answers what an undeclared one answers and an entry without a write is of no kind, while the unfiltered listing and `state` hold it; an accepter who may not know of a field the migration takes away, which a declaration reads, is listed it in part and refused its accept as not granted, naming neither (lakelands#867)"
);

/// The lake of the re-key checks with a type `doc`, and the proposals of
/// both checks, each named by its `why`.
async fn proposed(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = lake(store, IBANS).await?;
    let doc = |fields: &[&str], link: &[&str]| {
        let text = |n: &&str| json!({"name": n, "type": {"kind": "text"}});
        let (fields, link): (Vec<_>, Vec<_>) = (
            fields.iter().map(text).collect(),
            link.iter().map(text).collect(),
        );
        json!({"declare": "type", "name": "doc", "fields": fields,
               "relations": [{"name": "signed_by", "target": "person", "fields": link}]})
    };
    l.declare_json(doc(&["title", "secret"], &["ok"])).await?;
    let migrate = |mut to: Value, renames: &[(&str, &str)]| {
        to.as_object_mut().map(|o| o.remove("declare"));
        let to = serde_json::from_value(to).expect("a type");
        let renames = (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect();
        let migration = Box::new(Migration {
            to,
            renames,
            relation_renames: Vec::new(),
        });
        ProposedWrite::Migrate {
            migration,
            report: Default::default(),
        }
    };
    let verb = |name: &str, requires: Option<&str>, field: &str, value: &str| -> Declaration {
        let set = json!([{"field": field, "value": value}]);
        let payload = json!([{"name": "who", "type": {"kind": "text"}}]);
        let v = json!({"declare": "verb", "name": name, "on": "account", "risk": "high",
                       "payload": payload, "requires": requires, "effect": "write", "set": set});
        serde_json::from_value(v).expect("a verb")
    };
    let sum = json!([{"name": "sum", "type": {"kind": "text"}}]);
    let ledger = json!({"declare": "type", "name": "ledger", "fields": sum});
    let bundle = vec![
        trigger_on("held", "account", Some("holder = 'x'")),
        trigger_on("numbered", "account", Some("number = '1'")),
        object_set_on("rich", "account", "holder = 'x'"),
        verb("rename", None, "holder", "payload.who"),
        verb("close", Some("holder = 'x'"), "kind", "'old'"),
        serde_json::from_value(ledger).expect("a type"),
    ];
    let hidden = vec![trigger_on("held_too", "account", Some("holder = 'y'"))];
    let declare = |declarations| ProposedWrite::Declare { declarations };
    let (ty, opts) = (|| "account".into(), WriteOpts::default);
    let saved = patch(json!({"kind": "savings"}));
    let upsert = |key: &str| ProposedWrite::Upsert {
        ty: ty(),
        key: key.into(),
        patch: saved.clone(),
        opts: opts(),
    };
    let retire = ProposedWrite::Retire {
        ty: ty(),
        key: "4".into(),
        opts: opts(),
    };
    // `secret` is renamed and changes its kind, and a field is added.
    let mut renamed = doc(&["title", "hidden", "added"], &[]);
    renamed["fields"][1]["type"]["kind"] = json!("number");
    let renamed = migrate(renamed, &[("secret", "hidden")]);
    // `doc` valid from `signed_on`, which `hid` may not know of.
    let mut timed = doc(&["title", "secret"], &["ok"]);
    let signed_on = json!({"name": "signed_on", "type": {"kind": "date_time"}});
    timed["fields"]
        .as_array_mut()
        .expect("fields")
        .push(signed_on);
    timed["valid_from"] = json!("signed_on");
    let timed_bundle = vec![serde_json::from_value(timed.clone()).expect("a type")];
    for (why, write) in [
        ("timed", migrate(timed, &[])),
        ("timed bundle", declare(timed_bundle)),
        ("rekey", migrate(json!(to_iban().to), &[])),
        ("rename", renamed),
        ("drop", migrate(doc(&["title"], &["ok"]), &[])),
        ("bundle", declare(bundle)),
        ("hidden", declare(hidden)),
        ("upsert", upsert("1")),
        ("restricted", upsert("4")),
        ("retire restricted", retire),
    ] {
        let o = l.cat.propose(&l.own, write, why.into()).await;
        o.must(&format!("the owner proposing `{why}`"))?;
    }
    Ok(l)
}

/// The reviewers, none of whom may know of `account.holder`: `hid`, with
/// `schema`, may not know of `doc.secret` nor of the link field `ok`, nor
/// see `payment` and `entry`; `plain` holds `review` alone; `unseeing` may
/// not see `account`.
fn readers(store: &dyn LakeStore) -> [Principal; 3] {
    let none = |fields: &[&str]| TypeGrants {
        default: fields.is_empty().then_some(Access::None),
        fields: fields.iter().map(|f| ((*f).into(), Access::None)).collect(),
        ..Default::default()
    };
    let reviewer = |id: &str, types: &[(&str, &[&str])]| {
        let mut who = field_access(store, id, "account", &[("holder", Access::None)]);
        who.grants.review = true;
        let types = types.iter().map(|(t, fields)| ((*t).into(), none(fields)));
        who.grants.types.extend(types);
        who
    };
    let link = ("sys.link.doc.signed_by", &["ok"][..]);
    let mut hid = reviewer(
        "hid",
        &[
            ("payment", &[]),
            ("entry", &[]),
            ("doc", &["secret", "signed_on"]),
            link,
        ],
    );
    hid.grants.schema = true;
    [
        hid,
        reviewer("plain", &[]),
        reviewer("unseeing", &[("account", &[])]),
    ]
}

/// A reader, a proposal's `why`, the names its write must show and those it
/// must not, and whether it says there is more.
type Row<'a> = (&'a Principal, &'a str, &'a [&'a str], &'a [&'a str], bool);

/// What `who` lists under `filter`, newest first.
async fn listed(l: &Lake, who: &Principal, filter: Value) -> Result<Vec<Proposal>, Failure> {
    let filter: ProposalFilter = serde_json::from_value(filter).expect("a filter");
    Ok(l.cat
        .proposals(who, filter)
        .await
        .must("listing proposals")?
        .data)
}

/// What `who` lists of the proposal made for `why`: its write as JSON,
/// `None` where it shows none, and whether it says there is more.
async fn entry(l: &Lake, who: &Principal, why: &str) -> Result<(Option<String>, bool), Failure> {
    let p = listed(l, who, json!({}));
    let p = p.await?.into_iter().find(|p| p.why == why);
    let p = p.ok_or_else(|| Failure::new(format!("`{}` must be listed `{why}`", who.id)))?;
    Ok((p.write.map(|w| json!(w).to_string()), p.more))
}

async fn a_proposed_shape_names_only_what_its_reader_may_know_of(store: Store) -> Held {
    let (s, l) = (store.as_ref(), proposed(store.as_ref()).await?);
    let ([hid, plain, unseeing], reads) = (readers(s), reviewer(s, true));
    // A view whose `where` reads `holder`, which `plain` may not know of.
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let view = json!({"source": "account", "where": "holder = 'x'", "group_by": ["number"],
                      "measures": [{"name": "n", "expr": "count()"}]});
    let n = json!({"name": "n", "type": {"kind": "number"}});
    let tally = json!({"declare": "type", "name": "tally", "key": ["number"],
                       "fields": [text("number"), n], "view": view});
    let tally: Declaration = serde_json::from_value(tally).expect("a view");
    let declarations = vec![tally.clone()];
    let write = ProposedWrite::Declare { declarations };
    let o = l.cat.propose(&l.own, write, "view".into()).await;
    o.must("the owner proposing `view`")?;
    let rekey = ["\"payment\"", "\"entry\"", "\"holder\""];
    let converted = "\"field\":\"hidden\"";
    let rename = ["\"secret\"", "\"signed_by.ok\"", converted];
    // A field kept from its reader is kept under the name a rename gives it.
    let renamed = [rename[0], rename[1], converted, "\"name\":\"hidden\""];
    let all = ["held", "numbered", "rich", "rename", "close", "ledger"];
    let all = all.map(|n| format!("\"name\":\"{n}\""));
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let gone = [all[0], all[2], all[3], all[4], "holder"];
    let rows: [Row; 10] = [
        (&reads, "rekey", &rekey, &[], false),
        (&hid, "rekey", &["\"sys.link.account.owner\""], &rekey, true),
        (&reads, "rename", &rename, &[], false),
        (&hid, "rename", &["\"added\""], &renamed, true),
        (&hid, "drop", &["\"title\""], &["\"secret\""], true),
        (&hid, "bundle", &all, &[], false),
        (&plain, "bundle", &[all[1], all[5]], &gone, true),
        (&hid, "view", &["\"name\":\"tally\""], &[], false),
        // A valid time over a field its reader may not know of is not
        // named (lakelands#726).
        (
            &hid,
            "timed",
            &["\"title\""],
            &["valid_from", "signed_on"],
            true,
        ),
        (
            &hid,
            "timed bundle",
            &["\"title\""],
            &["valid_from", "signed_on"],
            true,
        ),
    ];
    let mut red = Vec::new();
    for (who, why, shows, hides, more) in rows {
        let (write, says) = entry(&l, who, why).await?;
        let write = write.unwrap_or_default();
        let has = |n: &&str| write.contains(n);
        if !shows.iter().all(has) || hides.iter().any(has) || says != more {
            let who = &who.id;
            red.push(format!("`{who}` must read of `{why}` {shows:?} and not {hides:?}, `more` {more}; it reads `more` {says} and {write}"));
        }
    }
    for (who, why) in [(&plain, "hidden"), (&plain, "view")] {
        let got = entry(&l, who, why).await?;
        if got != (None, true) {
            let who = &who.id;
            red.push(format!(
                "`{who}` must read no write of `{why}`, and `more`; it reads {got:?}"
            ));
        }
    }
    // In force, `describe` lists the view's type and withholds the definition.
    l.declare_as(tally, "declaring the view `tally`").await?;
    for (who, withheld) in [(&plain, true), (&hid, false)] {
        let seen = l.cat.describe(who, Default::default()).await;
        let seen = seen.must("describing the lake")?.data.types;
        if !(seen.iter()).any(|t| t.name.as_str() == "tally" && t.view.is_none() == withheld) {
            let who = &who.id;
            red.push(format!(
                "`{who}` must be described `tally`, its definition withheld: {withheld}"
            ));
        }
    }
    // One that names `account` is not there to `unseeing` (lakelands#762).
    let theirs = listed(&l, &unseeing, json!({})).await?;
    for why in ["bundle", "rekey", "hidden"] {
        if theirs.iter().any(|p| p.why == why) {
            red.push(format!(
                "`unseeing` must not be listed `{why}`, which names `account`"
            ));
        }
    }
    let red = red.join("; ");
    ensure!(
        red.is_empty(),
        "a proposal names what its reader may know of: {red}"
    );
    Ok(())
}

async fn a_listing_is_filtered_on_what_it_shows_and_an_accept_names_no_hidden_field(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), proposed(store.as_ref()).await?);
    let [hid, plain, unseeing] = readers(s);
    // A record its reader is not answered: restricted, without the grant,
    // and a proposal to write it is not there (lakelands#1008).
    for (who, why) in [(&plain, "upsert"), (&l.own, "restricted")] {
        let (write, more) = entry(&l, who, why).await?;
        ensure!(
            write.is_some() && !more,
            "`{}` must list `{why}` with its write; it reads {write:?}, `more` {more}",
            who.id
        );
    }
    let theirs = listed(&l, &plain, json!({})).await?;
    let held = ["restricted", "retire restricted"];
    ensure!(
        !theirs.iter().any(|p| held.contains(&p.why.as_str())),
        "`plain` must list no proposal on a restricted record; it lists {:?}",
        theirs.iter().map(|p| &p.why).collect::<Vec<_>>()
    );
    // A proposal on a type its reader may not see is not listed at all
    // (lakelands#762).
    let theirs = listed(&l, &unseeing, json!({})).await?;
    let named = ["upsert", "restricted", "retire restricted"];
    let listed_to = |why: &&str| theirs.iter().any(|p| p.why == *why);
    ensure!(
        !named.iter().any(listed_to),
        "`unseeing` must list no proposal on `account`; it lists {:?}",
        theirs.iter().map(|p| &p.why).collect::<Vec<_>>()
    );
    // The filters are answered on the entry as its reader is shown it.
    let mut counts = Vec::new();
    for (who, filter) in [
        (&unseeing, json!({"type": "account"})),
        (&unseeing, json!({"type": "ghost"})),
        (&unseeing, json!({"kind": "upsert"})),
        (&unseeing, json!({"state": "pending"})),
        (&l.own, json!({"type": "account"})),
        (&l.own, json!({"kind": "upsert"})),
    ] {
        counts.push(listed(&l, who, filter).await?.len());
    }
    ensure!(
        counts == [0, 0, 0, 4, 6, 2],
        "`type` and `kind` are answered on what the reader is shown: one who may not see \
         `account` must list nothing by it, as by an undeclared type, no upsert, and by state \
         the four that name no `account`, and the owner six by the type and two upserts; they \
         list {counts:?}"
    );
    // A bundle that moves a verb or a trigger off `doc` is found by `doc`
    // as by the type it moves to, as its entry is judged (lakelands#916).
    let verb = |on: &str, f: &str| {
        json!({"declare": "verb", "name": "stamp", "on": on,
        "risk": "low", "effect": "write", "set": [{"field": f, "value": "'x'"}]})
    };
    l.declare_json(verb("doc", "title")).await?;
    l.declare_as(trigger_on("tick", "doc", None), "a trigger on `doc`")
        .await?;
    for (why, d) in [
        ("moved verb", verb("account", "holder")),
        ("moved trigger", json!(trigger_on("tick", "account", None))),
    ] {
        let write = ProposedWrite::Declare {
            declarations: vec![serde_json::from_value(d).expect("a declaration")],
        };
        l.cat.propose(&l.own, write, why.into()).await.must(why)?;
    }
    for filter in [
        json!({}),
        json!({"type": "doc"}),
        json!({"type": "account"}),
        json!({"type": "doc", "kind": "declare"}),
    ] {
        let found = listed(&l, &l.own, filter.clone()).await?;
        let moved = found.iter().filter(|p| p.why.starts_with("moved")).count();
        ensure!(
            moved == 2,
            "the owner must list both moved declarations under {filter}; it lists {moved}"
        );
    }
    // The accept: a trigger declared since reads the field the migration drops.
    let watch = trigger_on("watch", "doc", Some("secret = 'x'"));
    l.declare_as(watch, "a trigger that reads `secret`").await?;
    let all = listed(&l, &l.own, json!({})).await?;
    let drop = all.into_iter().find(|p| p.why == "drop").map(|p| p.id);
    let drop = drop.ok_or_else(|| Failure::new("the owner lists `drop`".to_owned()))?;
    let told = l.cat.accept(&hid, &drop).await;
    // Neither name in any member of its wire form, its sentence among them.
    let wire = told.as_ref().err().map(|e| json!(e).to_string());
    let wire = wire.unwrap_or_default();
    let names = wire.contains("secret") || wire.contains("watch");
    let not_granted = lakelands_api::Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::NotGranted,
    };
    ensure!(
        told.as_ref().err() == Some(&not_granted) && !names,
        "an accepter who may not know of `secret`, listed the migration in part, must be \
         refused its accept as not granted, naming neither; it answered {told:?}"
    );
    let told = l.cat.accept(&l.own, &drop).await;
    let case = json!({"code": "migration_takes_read", "unnamed": false, "field": "secret",
                      "at": [{"code": "trigger", "name": "watch"}]});
    refused_like("the owner accepting it", told, &case)?;
    Ok(())
}
