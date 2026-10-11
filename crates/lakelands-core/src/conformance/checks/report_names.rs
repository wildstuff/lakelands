//! A migration's report is answered as its reader may know it
//! (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md` and
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`, each
//! extended): in a listing, in a preview, and in the refusal of a
//! migration that is not lossless, whoever proposes, applies or accepts it.

use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Migration, MigrationReport, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Error, Missing, Outcome, Principal, Refusal, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::fixture::{Lake, Must, field_access, patch, reason, restrict, reviewer};
use crate::conformance::{Failure, Held, Store};

check!(
    NAMED_IN_KEY_ORDER = the_keys_a_report_names_stand_in_the_order_of_the_new_key,
    "027, extended (lakelands#875): the colliding keys a report names stand in the order of the new key and are its first ten, wherever a record held from their reader sorts: of two types of one shape, each with eleven keys that two records their reader is answered would take, and a restricted record that sorts first and would take the last of the keys in the one and the first in the other, the reader is named the same ten keys beside the same count and `more`"
);

async fn the_keys_a_report_names_stand_in_the_order_of_the_new_key(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let shape = |ty: &str, key: &str| json!({"name": ty, "key": [key], "fields": [text("number"), text("iban")]});
    for (ty, held) in [("ta", "K11"), ("tb", "K01")] {
        let mut declared = shape(ty, "number");
        declared["declare"] = json!("type");
        l.declare_json(declared).await?;
        // The restricted record sorts before every other by its key.
        l.upsert(ty, "0", json!({"number": "0", "iban": held}))
            .await?;
        restrict(s, ty, "0", "a hold").await?;
        for (n, half) in (1..=11).flat_map(|n| [(n, "a"), (n, "b")]) {
            let (number, iban) = (format!("{half}{n:02}"), format!("K{n:02}"));
            let fields = json!({"number": number, "iban": iban});
            l.upsert(ty, &number, fields).await?;
        }
    }
    let reader = reviewer(s, true);
    let mut said = Vec::new();
    for ty in ["ta", "tb"] {
        let r = preview(&l, &reader, migrate_to(shape(ty, "iban"))).await?;
        let keys = r.sample_collisions.iter().map(|c| c.key.to_string());
        let counts: Vec<u64> = r.rekeyed.iter().map(|k| k.collisions).collect();
        said.push((keys.collect::<Vec<_>>(), counts, r.more));
    }
    let ten: Vec<String> = (1..=10).map(|n| format!("K{n:02}")).collect();
    ensure!(
        said[0] == said[1] && said[0] == (ten, vec![23], true),
        "the keys named are the first ten in the order of the new key, with 23 records \
         colliding and `more`, whichever key the restricted record would take; `ta`, where it \
         would take the last, and `tb`, the first, are answered {said:?}"
    );
    Ok(())
}

check!(
    REPORT_AS_ITS_READERS_VIEW = a_migrations_report_is_answered_as_its_reader_may_know_it,
    "013, 024 and 027, extended (lakelands#531): a migration's report names a conversion, a dropped field, a re-keyed type, a rewritten reference and a colliding key only where its reader may see the type and know of the field, and a sample key only of a record it is answered, a link's with its ends and where it is shown the link type (lakelands#871), beside a count that stays whole; a colliding key only where it is answered two or more of the records that would take it, a held one beside them or not, and reads every field of the new key, a link's key with its ends, and the keys a report names are the first among those its reader may be named (lakelands#875), in the refusal of an accept, of `migrate_apply` and of `propose` and in a preview, an author's own too; `more` says that an entry was left out or that a count is larger than the keys named beside it, to one who may see everything too, and nothing else; the id is made over what the reader is shown, so a preview read trimmed is applied by it (lakelands#644); a relation the migration removes is listed as dropped to one who sees the type and is shown the relation (lakelands#662); to an accepter who may not see the type the proposal is one that does not exist (lakelands#762)"
);

/// `account` keyed by `key`, with `holder` and, when given, `score` of
/// those kinds, and its relation `owner` when `linked`.
fn account(key: &str, holder: &str, score: Option<&str>, linked: bool) -> Migration {
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let mut fields = vec![text("number"), text("iban"), field("holder", holder)];
    fields.extend(score.map(|kind| field("score", kind)));
    let owner = json!({"name": "owner", "target": "person"});
    let relations: Vec<Value> = linked.then_some(owner).into_iter().collect();
    migrate_to(json!({"name": "account", "key": [key], "fields": fields, "relations": relations}))
}

/// `box` keyed by `key`, with its relation `kept_by` to any number of
/// persons, whose links hold `since` of that kind.
fn boxed(key: &str, since: &str) -> Value {
    let link = json!([{"name": "since", "type": {"kind": since}}]);
    let many = "many_to_many";
    let kept_by =
        json!({"name": "kept_by", "target": "person", "cardinality": many, "fields": link});
    let fields = json!([text("code"), text("label")]);
    json!({"name": "box", "key": [key], "fields": fields, "relations": [kept_by]})
}

fn migrate_to(to: Value) -> Migration {
    let to = serde_json::from_value(to).expect("a type");
    Migration {
        to,
        renames: vec![],
        relation_renames: Vec::new(),
    }
}

fn text(n: &str) -> Value {
    json!({"name": n, "type": {"kind": "text"}, "required": true})
}

/// The preview of `m` as `who` is answered it.
pub(super) async fn preview(
    l: &Lake,
    who: &Principal,
    m: Migration,
) -> Result<MigrationReport, Failure> {
    Ok(l.cat.migrate(who, m).await.must("previewing")?.data)
}

/// The report a refusal carries, as JSON.
pub(super) fn refused(answer: lakelands_api::Result<Outcome>) -> Result<String, Failure> {
    match answer.map_err(|e| (reason(&e).cloned(), e)) {
        Err((Some(Refusal::Migration { report }), _)) => Ok(json!(report).to_string()),
        other => Err(Failure::new(format!(
            "a refusal as a migration is due; it answered {other:?}"
        ))),
    }
}

async fn a_migrations_report_is_answered_as_its_reader_may_know_it(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let keyed = |name: &str, key: &[&str], fields: Value| json!({"declare": "type", "name": name, "key": key, "fields": fields});
    let to = |ty: &str| json!({"name": ty, "type": {"kind": "reference", "target": ty}, "required": true});
    let declared = |mut ty: Value| {
        ty["declare"] = json!("type");
        ty
    };
    for d in [
        keyed("person", &["name"], json!([text("name"), text("nick")])),
        declared(json!(account("number", "text", Some("text"), true).to)),
        keyed(
            "entry",
            &["account", "n"],
            json!([to("account"), text("n")]),
        ),
        declared(boxed("code", "text")),
        keyed("slot", &["box", "n"], json!([to("box"), text("n")])),
    ] {
        l.declare_json(d).await?;
    }
    for n in ["1", "2", "4"] {
        let (iban, seven) = (format!("DE{n}"), "7");
        let fields = json!({"number": n, "iban": iban, "holder": seven, "score": "10"});
        l.upsert("account", n, fields).await?;
    }
    for (ty, key, fields) in [
        ("person", "x", json!({"name": "x", "nick": "nx"})),
        ("entry", "1:1", json!({"account": "1", "n": "1"})),
        ("box", "b1", json!({"code": "b1", "label": "l1"})),
        ("slot", "b1:1", json!({"box": "b1", "n": "1"})),
    ] {
        l.upsert(ty, key, fields).await?;
    }
    let link = |ty: &str, from: &str, relation: &str, since: Value| {
        let (ty, from, relation, x) = (ty.into(), from.into(), relation.into(), "x".into());
        let (cat, own, opts) = (l.cat.clone(), l.own.clone(), WriteOpts::default());
        async move {
            cat.link(&own, &ty, &from, &relation, &x, patch(since), opts)
                .await
        }
    };
    let fut = link("account", "1", "owner", json!({}));
    fut.await.must("linking account 1 to x")?;
    let kept = link("box", "b1", "kept_by", json!({"since": "1"})).await;
    kept.must("linking box b1 to x")?;
    // The reviewer and author: it may not know of `score`, may not see
    // `entry` and `slot`, and is not answered a restricted record.
    let mut hid = field_access(s, "hid", "account", &[("score", Access::None)]);
    let unseen = || TypeGrants {
        default: Some(Access::None),
        fields: [].into(),
        ..Default::default()
    };
    hid.grants
        .types
        .extend(["entry", "slot"].map(|t| (t.into(), unseen())));
    (hid.grants.review, hid.grants.schema) = (true, true);

    // Proposed while lossless: `score` and `holder` as numbers, a re-key,
    // the relation removed, and the link field `since` as a number.
    let mut ids = Vec::new();
    for m in [
        account("number", "text", Some("number"), true),
        account("number", "number", Some("text"), true),
        account("iban", "text", Some("text"), true),
        account("number", "text", Some("text"), false),
        migrate_to(boxed("code", "number")),
    ] {
        let (migration, report) = (Box::new(m), Default::default());
        let write = ProposedWrite::Migrate { migration, report };
        let o = l.cat.propose(&l.own, write, "why".into()).await;
        match o.must("proposing while lossless")? {
            Outcome::Proposed { proposal, .. } => ids.push(proposal),
            other => return Err(Failure::new(format!("a proposal is due: {other:?}"))),
        }
    }
    // Written since: values that do not convert, one on a record then
    // restricted, and an account whose `iban` is taken, with an entry.
    l.upsert("account", "2", json!({"holder": "x", "score": "abc"}))
        .await?;
    l.upsert("account", "4", json!({"holder": "y", "score": "xyz"}))
        .await?;
    restrict(s, "account", "4", "a hold").await?;
    let soon = link("box", "b1", "kept_by", json!({"since": "soon"})).await;
    soon.must("linking box b1 to x again")?;
    restrict(s, "sys.link.box.kept_by", "b1:x", "a hold").await?;
    // A link that does not convert either, of a box then restricted.
    l.upsert("box", "b2", json!({"code": "b2", "label": "l2"}))
        .await?;
    let late = link("box", "b2", "kept_by", json!({"since": "late"})).await;
    late.must("linking box b2 to x")?;
    restrict(s, "box", "b2", "a hold").await?;
    let fifth = json!({"number": "5", "iban": "DE1", "holder": "8", "score": "50"});
    l.upsert("account", "5", fifth).await?;
    l.upsert("entry", "5:1", json!({"account": "5", "n": "1"}))
        .await?;
    // An account whose `iban` the restricted one holds, a restricted one
    // that holds the `iban` of two its reader is answered, and five links
    // that do not convert either, behind the two that are held.
    let sixth = json!({"number": "6", "iban": "DE4", "holder": "9", "score": "60"});
    l.upsert("account", "6", sixth).await?;
    let seventh = json!({"number": "7", "iban": "DE1", "holder": "9", "score": "70"});
    l.upsert("account", "7", seventh).await?;
    restrict(s, "account", "7", "a hold").await?;
    for n in 3..8 {
        let code = format!("b{n}");
        let fields = json!({"code": code, "label": format!("l{n}")});
        l.upsert("box", &code, fields).await?;
        let linked = link("box", &code, "kept_by", json!({"since": "s"})).await;
        linked.must("linking a box whose link does not convert")?;
    }

    // A box kept by two persons of one `nick`, one of them restricted:
    // re-keyed by it, their links would take one key.
    l.upsert("box", "bx", json!({"code": "bx", "label": "lx"}))
        .await?;
    for name in ["b1", "hp"] {
        let fields = json!({"name": name, "nick": "n"});
        l.upsert("person", name, fields).await?;
        let (of, bx, kept_by) = ("box".into(), "bx".into(), "kept_by".into());
        let (to, since, opts) = (
            name.into(),
            patch(json!({"since": "1"})),
            WriteOpts::default(),
        );
        let linked = (l.cat).link(&l.own, &of, &bx, &kept_by, &to, since, opts);
        linked.await.must("linking box bx to a person")?;
    }
    restrict(s, "person", "hp", "a hold").await?;
    let nick = json!({"name": "person", "key": ["nick"], "fields": [text("name"), text("nick")]});
    let nicked = json!(preview(&l, &hid, migrate_to(nick.clone())).await?).to_string();
    let nicked_whole = json!(preview(&l, &l.own, migrate_to(nick)).await?).to_string();

    // The reports, each as its reader is answered it.
    let view = account("iban", "text", None, true);
    let whole = preview(&l, &l.own, account("iban", "text", Some("text"), true)).await?;
    let read = preview(&l, &hid, view.clone()).await?;
    let boolean = preview(&l, &hid, account("number", "boolean", None, true)).await?;
    // One who reads `iban` masked, and so not the key the re-key makes.
    let mut masked = field_access(s, "masked", "account", &[("iban", Access::Masked)]);
    (masked.grants.review, masked.grants.schema) = (true, true);
    let masked = preview(&l, &masked, account("iban", "text", Some("text"), true)).await?;
    let masked = json!(masked).to_string();
    // The author's id is of what it is shown, never the owner's.
    let (id, owners) = (read.report_id.clone(), whole.report_id);
    let boolean = json!(boolean).to_string();
    let (migration, report) = (Box::new(view.clone()), Default::default());
    let again = ProposedWrite::Migrate { migration, report };
    let scored = refused(l.cat.accept(&hid, &ids[0]).await)?;
    let held = refused(l.cat.accept(&hid, &ids[1]).await)?;
    let held_whole = refused(l.cat.accept(&l.own, &ids[1]).await)?;
    let rekeyed = refused(l.cat.accept(&hid, &ids[2]).await)?;
    let rekeyed_whole = refused(l.cat.accept(&l.own, &ids[2]).await)?;
    let applied = refused(l.cat.migrate_apply(&hid, view, &read.report_id).await)?;
    let proposed = refused(l.cat.propose(&hid, again, "again".into()).await)?;
    let since = refused(l.cat.accept(&hid, &ids[4]).await)?;
    let since_whole = refused(l.cat.accept(&l.own, &ids[4]).await)?;
    // One who is answered every record, and is not shown the link type.
    let mut far = l.own.clone();
    let from = [("from".into(), Access::None)].into();
    let link = TypeGrants {
        fields: from,
        ..Default::default()
    };
    far.grants.types.insert("sys.link.box.kept_by".into(), link);
    let far = preview(&l, &far, migrate_to(boxed("code", "number"))).await?;
    let far = json!(far).to_string();
    let read = json!(read).to_string();
    // Per row: the report, the names it must show and those it must not.
    let (more, id, owners) = ("\"more\":true", id.as_str(), owners.as_str());
    let (one, both) = ("\"sample_keys\":[\"2\"]", "\"sample_keys\":[\"2\",\"4\"]");
    let collision = "{\"key\":\"DE1\",\"type\":\"account\"}";
    // `DE4` is the `iban` of one account its reader is answered and of a
    // restricted one, `DE1` of two it is answered and of a restricted
    // one, which moves the count alone.
    let (held_key, counts) = ("DE4", "\"collisions\":5");
    let five = "\"sample_keys\":[\"b3:x\",\"b4:x\",\"b5:x\",\"b6:x\",\"b7:x\"]";
    // A count stays whole: of what is there, not of what its reader is answered.
    let (kept, two) = ("kept_by.since", "\"not_convertible\":7");
    let what = [
        "the accept of a hidden field's conversion",
        "the accept of `holder` as a number",
        "the same, to one who sees everything",
        "the accept of the re-key",
        "the same, to one who sees everything",
        "an author's preview of the re-key",
        "its apply by the preview's id",
        "its proposal",
        "an author's preview of a boolean",
        "the accept of a link field as a number",
        "the same, to one who sees everything",
        "its preview by one not shown the link type",
        "the preview of the re-key by one masked on the new key",
        "the preview of a re-key whose colliding link has an end held from its reader",
        "the same, to one who sees everything",
    ];
    let visible = "[\"1\",\"2\",\"5\",\"6\"]";
    let rows: [Row; 15] = [
        (&scored, &[more], &["score", "keys"]),
        (&held, &[more, "holder", one], &["\"4\""]),
        (&held_whole, &[both], &["more"]),
        (&rekeyed, &[more, collision, counts], &["entry", held_key]),
        (&rekeyed_whole, &[more, "entry", "DE1:1", held_key], &[]),
        (
            &read,
            &[more, id, collision, counts],
            &["entry", owners, held_key],
        ),
        (
            &applied,
            &[more, id, collision],
            &["entry", owners, held_key],
        ),
        (&proposed, &[more, collision], &["entry", held_key]),
        (&boolean, &[more, visible], &["\"4\""]),
        (&since, &[more, kept, two, five], &["b1:x", "b2:x"]),
        (&since_whole, &[more, kept, two, "b1:x", "b2:x"], &[]),
        (&far, &[more, kept, two], &["b1:x", "b2:x"]),
        (
            &masked,
            &[more, counts, "\"collisions\":2"],
            &["sample_collisions"],
        ),
        (&nicked, &[more, "\"collisions\":2"], &["sample_collisions"]),
        (&nicked_whole, &[more, "bx:n"], &[]),
    ];
    let mut red = Vec::new();
    for (what, (report, shows, hides)) in what.into_iter().zip(rows) {
        let has = |n: &&str| report.contains(n);
        if !shows.iter().all(has) || hides.iter().any(has) {
            red.push(format!(
                "{what}: {shows:?} and not {hides:?} are due in {report}"
            ));
        }
    }
    let red = red.join("; ");
    ensure!(
        red.is_empty(),
        "a report is answered as its reader may know it: {red}"
    );

    // To one who may not see the type, the proposal is not there
    // (lakelands#762): no report, and no name.
    let mut blank = reviewer(s, true);
    blank.grants.default = Access::None;
    let unnamed = l.cat.accept(&blank, &ids[1]).await.map(|_| ());
    let missing = Missing::Proposal { id: ids[1].clone() };
    ensure!(
        unnamed == Err(Error::NotFound { missing }),
        "one who may not see `account` accepting its migration is answered as for a proposal \
         that does not exist; it is told {unnamed:?}"
    );
    // A relation the migration removes is listed as dropped, with its links.
    let listed = l.cat.proposals(&l.own, ProposalFilter::default()).await;
    let mut listed = listed.must("listing")?.data;
    listed.retain(|p| p.id == ids[3]);
    let unlinked = listed.pop().map(|p| (json!(p.write).to_string(), p.more));
    let whole = |(w, more): &(String, bool)| w.contains("[\"owner\",1]") && !more;
    ensure!(
        unlinked.as_ref().is_some_and(whole),
        "a proposed migration that removes a relation lists it as dropped, whole to one who \
         sees the type; the owner lists {unlinked:?}"
    );
    // A preview read trimmed is applied by the id it carried.
    let relabel = migrate_to(boxed("label", "text"));
    let read = preview(&l, &hid, relabel.clone()).await?;
    let said = json!(read).to_string();
    ensure!(
        read.lossless && read.more && !said.contains("slot"),
        "the preview of a re-key that moves a hidden type is lossless, trimmed and says \
         `more`; it answers {said}"
    );
    let applied = l.cat.migrate_apply(&hid, relabel, &read.report_id).await;
    applied.must("applying the re-key by the id of the preview read trimmed")?;
    Ok(())
}

/// A report as JSON, the names it must show and those it must not.
type Row<'a> = (&'a String, &'a [&'a str], &'a [&'a str]);
