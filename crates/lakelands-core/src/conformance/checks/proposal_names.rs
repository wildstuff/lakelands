//! A proposal names only what its reader may know of, and says that there
//! is more (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md` and
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`, each
//! extended): a listing shows a name where `describe` would, leaves the
//! write out where its reader may see nothing of it, and answers its
//! filters on what it shows.

use std::sync::Arc;

use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, LakeStore, Principal, WriteOpts};
use serde_json::{Value, json};

use super::rekey::{IBANS, lake, to_iban};
use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field_access, invalid_naming, object_set_on, patch, reviewer, trigger_on,
};

check!(
    /// A proposed shape names only what its reader may know of.
    NAMES_AS_THE_READERS_VIEW = a_proposed_shape_names_only_what_its_reader_may_know_of,
    "013, 024 and 027, extended (lakelands#499): `proposals` shows of a proposed migration the renames, the dropped fields and link fields, the re-keyed types and the rewritten references only where its reader may see the type and know of the field, the totals whole; of a proposed bundle each declaration `describe` would show its reader once in force, a verb by the fields it names; an entry that holds more says `more` and names nothing else, and a bundle with nothing left shows no write"
);

check!(
    /// A listing is filtered on what it shows, and an accept names no hidden field.
    LISTED_AS_SHOWN = a_listing_is_filtered_on_what_it_shows_and_an_accept_names_no_hidden_field,
    "013 and 024, extended (lakelands#499): a proposal on a type its reader may not see, or on a restricted record without the grant, is listed without its write and says `more`; `type` and `kind` are answered on the entry as shown, so a hidden type answers what an undeclared one answers and an entry without a write is of no kind, while the unfiltered listing and `state` hold it; an accept refused because a declaration reads a field the migration takes away names neither to an accepter who may not know of the field"
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
    for (why, write) in [
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
        &[("payment", &[]), ("entry", &[]), ("doc", &["secret"]), link],
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
    let p = listed(l, who, json!({}))
        .await?
        .into_iter()
        .find(|p| p.why == why);
    let p = p.ok_or_else(|| Failure::new(format!("`{}` must be listed `{why}`", who.id)))?;
    Ok((p.write.map(|w| json!(w).to_string()), p.more))
}

async fn a_proposed_shape_names_only_what_its_reader_may_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), proposed(store.as_ref()).await?);
    let ([hid, plain, unseeing], reads) = (readers(s), reviewer(s, true));
    let rekey = ["\"payment\"", "\"entry\"", "\"holder\""];
    let converted = "\"field\":\"hidden\"";
    let rename = ["\"secret\"", "\"signed_by.ok\"", converted];
    // A field kept from its reader is kept under the name a rename gives it.
    let renamed = [rename[0], rename[1], converted, "\"name\":\"hidden\""];
    let all = ["held", "numbered", "rich", "rename", "close", "ledger"];
    let all = all.map(|n| format!("\"name\":\"{n}\""));
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let gone = [all[0], all[2], all[3], all[4], "holder"];
    let rows: [Row; 8] = [
        (&reads, "rekey", &rekey, &[], false),
        (&hid, "rekey", &["\"sys.link.account.owner\""], &rekey, true),
        (&reads, "rename", &rename, &[], false),
        (&hid, "rename", &["\"added\""], &renamed, true),
        (&hid, "drop", &["\"title\""], &["\"secret\""], true),
        (&hid, "bundle", &all, &[], false),
        (&plain, "bundle", &[all[1], all[5]], &gone, true),
        (&unseeing, "bundle", &[all[5]], &["account"], true),
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
    for (who, why) in [
        (&plain, "hidden"),
        (&unseeing, "rekey"),
        (&unseeing, "hidden"),
    ] {
        let got = entry(&l, who, why).await?;
        if got != (None, true) {
            let who = &who.id;
            red.push(format!(
                "`{who}` must read no write of `{why}`, and `more`; it reads {got:?}"
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
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), proposed(store.as_ref()).await?);
    let [hid, plain, unseeing] = readers(s);
    // A record its reader is not answered: restricted, without the grant.
    for (who, why, shown) in [
        (&plain, "upsert", true),
        (&plain, "restricted", false),
        (&plain, "retire restricted", false),
        (&l.own, "restricted", true),
        (&unseeing, "upsert", false),
    ] {
        let (write, more) = entry(&l, who, why).await?;
        let (who, with) = (&who.id, if shown { "with" } else { "without" });
        ensure!(
            write.is_some() == shown && more != shown,
            "`{who}` must list `{why}` {with} its write; it reads {write:?}, `more` {more}"
        );
    }
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
        counts == [0, 0, 0, 8, 6, 2],
        "`type` and `kind` are answered on what the reader is shown: one who may not see \
         `account` must list nothing by it, as by an undeclared type, no upsert, and all eight \
         by state, and the owner six by the type and two upserts; they list {counts:?}"
    );
    // The accept: a trigger declared since reads the field the migration drops.
    let watch = trigger_on("watch", "doc", Some("secret = 'x'"));
    l.declare_as(watch, "a trigger that reads `secret`").await?;
    let all = listed(&l, &l.own, json!({})).await?;
    let drop = all.into_iter().find(|p| p.why == "drop").map(|p| p.id);
    let drop = drop.ok_or_else(|| Failure::new("the owner lists `drop`".to_owned()))?;
    let told = l.cat.accept(&hid, &drop).await;
    let said = told.as_ref().err().map(ToString::to_string);
    let said = said.unwrap_or_default();
    let names = said.contains("secret") || said.contains("watch");
    ensure!(
        said.contains("a declaration reads a field") && !names,
        "an accepter who may not know of `secret` is told that a declaration reads a field \
         the migration takes away, and neither name; it answered {told:?}"
    );
    let names = ["trigger declaration `watch`", "reads `secret`"];
    let told = l.cat.accept(&l.own, &drop).await;
    invalid_naming("the owner accepting it", told, &names)?;
    Ok(())
}
