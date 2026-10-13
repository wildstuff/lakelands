//! A migration's report says every difference between the declaration in
//! force and its `to`
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended): in the shape's own lists, or in `restated`.

use std::path::PathBuf;

use lakelands_api::error::Case;
use lakelands_api::id::ProposalId;
use lakelands_api::ontology::{FieldDecl, RelationDecl, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Migration, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Outcome, Principal};
use serde_json::{Value, json};

use super::export::Scratch;
use super::report_names::{preview, refused};
use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, describe_type, field_access, invalid_case, is_invalid, log_events,
};
use crate::conformance::{Failure, Held, Store};
use crate::links::link_type_name;

check!(
    EVERY_MEMBER = a_migration_reports_every_member_its_to_states_otherwise,
    "027, extended (lakelands#554): every member of a type's, a field's, a relation's and a link field's declaration, as it serialises, changed alone in a migration's `to`, is in its preflight's report: a type that changes and a `required` that tightens in `fields`, every other member in `restated` with what was declared and what `to` states, a member left out as null, and nothing for a `depends_on` left to the store; an added field and relation are restated member by member; `describe` shows a field its reader does not read by its shape alone (024)",
    embedding
);

check!(
    AS_ITS_READERS = what_a_migration_restates_is_shown_as_its_fields_are_and_holds_at_the_accept,
    "027, 024 and 013, extended (lakelands#554): a `to` that leaves out `sensitive`, `searchable` and the labels is lossless and its report restates each; an entry is answered where its reader may know of the field, a renamed field's where it may know of the name it had, in a proposal's listing, with `more`, so that a reviewer listed it so is refused the accept as not granted (lakelands#867), and whole in the refusal of an accept that is not lossless to one listed it whole; an author kept from a field restates nothing of it; one who knows of a field or a link field and does not read it, in `to` or as the type stands, is answered its shape alone, `sensitive` and no label, description, annotation, `searchable`, initial value or expression, in its own preflight and in a listing, and in the `to` and the types a listing hands it; the `write_proposed` event carries the report whole in the feed and in an export, and without the member where nothing is restated; an accept is refused where the type gained a mark the proposal's report does not restate",
    scratch
);

/// The members of a declaration as they serialise, taken from a pattern
/// that names each: a member added to the declaration does not compile
/// here until it is named, and is then red in the walk until a row of
/// [`ROWS`] says where the report holds it.
macro_rules! members {
    ($t:ident { $($m:ident),* }) => {{
        #[allow(dead_code)]
        fn named(d: &$t) {
            let $t { $($m: _),* } = d;
        }
        vec![$(stringify!($m)),*]
    }};
}

/// A kind of declaration: its members, and whether a thing of [`ROWS`]
/// is of the kind.
type Kind = (&'static str, Vec<&'static str>, fn(&str) -> bool);

/// Each kind of declaration. A link field's members are a field's.
#[rustfmt::skip]
fn kinds() -> [Kind; 3] {
    [
        ("the type", members!(TypeDecl { name, key, fields, relations, view, valid_from, valid_to, language, field_meta, insert_only, description, label, annotations }), |t| t == "-"),
        ("a field", members!(FieldDecl { name, ty, required, authority, derived, function, computed_at, depends_on, on_input_change, initial, sensitive, searchable, description, label, embed, annotations }), |t| t != "-" && !RELATIONS.contains(&t)),
        ("a relation", members!(RelationDecl { name, target, cardinality, inverse, via, fields, authority, description, annotations, derived }), |t| RELATIONS.contains(&t)),
    ]
}

/// One change alone, a line each: what it is made to (`-` the type, a
/// field, a relation, a link field as `relation.field`), the member, what
/// `to` states for it (`null`: left out), and where the report says it:
/// in `fields` by the name, in `restated` with what was declared and what
/// `to` states, in `dropped` with nothing counted, or nowhere: the
/// migration is `refused` as invalid, or restates `nothing` of the thing,
/// since the store completes what a computed field reads. A line whose
/// member is `-` takes the thing itself away; `more` and `met` are added.
const ROWS: &str = r#"
- key ["other"] restated
- view {"source":"person","measures":[]} refused
- valid_from null restated
- valid_to null restated
- language "en" restated
- field_meta true restated
- insert_only true restated
- description null restated
- label null restated
- annotations null restated
secret type {"kind":"number"} fields
title required true fields
secret required null restated
secret authority "authored" restated
loud derived "lower(title)" restated
loud depends_on null nothing
loud computed_at "write" restated
made function "hush" restated
verdict depends_on ["body"] restated
verdict on_input_change "keep" restated
title initial null restated
secret sensitive null restated
secret searchable null restated
secret description null restated
secret label null restated
near embed {"from":["body"],"model":"MODEL"} restated
secret annotations null restated
more type {"kind":"number"} restated
body - null dropped
same target "guest" restated
read_by cardinality "many_to_one" restated
owned_by inverse null restated
owned_by via null refused
owned_by via "keeper" restated
read_by authority null restated
owned_by description null restated
owned_by annotations null restated
same derived {"match":[{"from":"other","to":"name"}]} restated
met target "person" restated
owned_by - null dropped
read_by - null dropped
same - null dropped
read_by.seen type {"kind":"number"} fields
read_by.seen sensitive null restated
read_by.seen - null dropped
"#;

/// The relations of `note`.
const RELATIONS: [&str; 4] = ["owned_by", "read_by", "same", "met"];

/// `note`, with every member of each kind of declaration stated by one of
/// its fields and relations, otherwise than its default.
fn note() -> Value {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let at = |n: &str| json!({"name": n, "type": {"kind": "date_time"}});
    let marks = json!({"acme.icon": "card"});
    let secret = json!({
        "name": "secret", "type": {"kind": "text"}, "required": true, "authority": "shared",
        "sensitive": true, "searchable": true, "description": "What is kept.",
        "label": "Secret", "annotations": marks,
    });
    let verdict = json!({
        "name": "verdict", "type": {"kind": "text"}, "depends_on": ["title"],
        "on_input_change": "clear",
    });
    let loud = json!({
        "name": "loud", "type": {"kind": "text"}, "authority": "derived",
        "derived": "upper(title)", "depends_on": ["title"],
    });
    let made = json!({
        "name": "made", "type": {"kind": "text"}, "authority": "derived",
        "function": "shout", "depends_on": ["title"],
    });
    let near = json!({
        "name": "near", "type": {"kind": "vector", "dimensions": 2}, "authority": "derived",
        "embed": {"from": ["title"], "model": EMBED_MODEL},
    });
    let to = |n: &str| json!({"name": n, "type": {"kind": "reference", "target": "person"}});
    let seen = json!({
        "name": "seen", "type": {"kind": "text"}, "authority": "source", "sensitive": true,
    });
    let owned_by = json!({
        "name": "owned_by", "target": "person", "cardinality": "many_to_one", "via": "owner",
        "inverse": "owns", "description": "Whose it is.", "annotations": marks,
    });
    let read_by = json!({
        "name": "read_by", "target": "person", "cardinality": "many_to_many",
        "authority": "source", "fields": [seen],
    });
    let same = json!({
        "name": "same", "target": "person", "cardinality": "many_to_many",
        "derived": {"match": [{"from": "code", "to": "name"}]},
    });
    let required = |n: &str| json!({"name": n, "type": {"kind": "text"}, "required": true});
    let title = json!({"name": "title", "type": {"kind": "text"}, "initial": "untitled"});
    json!({
        "name": "note", "key": ["code"], "valid_from": "since", "valid_to": "until",
        "language": "de", "field_meta": false, "description": "A note.", "label": "Note",
        "annotations": marks,
        "fields": [required("code"), required("other"), title, text("body"),
                   at("since"), at("until"), secret, verdict, loud, made, near, to("owner"),
                   to("keeper")],
        "relations": [owned_by, read_by, same],
    })
}

/// The list within a type that declares `thing`, and its name there.
fn held<'t>(ty: &'t mut Value, thing: &'t str) -> (&'t mut Value, &'t str) {
    match thing.split_once('.') {
        Some((r, f)) => (&mut named(&mut ty["relations"], r)["fields"], f),
        None if RELATIONS.contains(&thing) => (&mut ty["relations"], thing),
        None => (&mut ty["fields"], thing),
    }
}

/// The declaration of `name` in a list, made where it has none; a list
/// left out serialises as none.
fn named<'t>(list: &'t mut Value, name: &str) -> &'t mut Value {
    if list.is_null() {
        *list = json!([]);
    }
    let all = list.as_array_mut().expect("a list of declarations");
    let at = all.iter().position(|d| d["name"] == name);
    let at = at.unwrap_or_else(|| {
        all.push(json!({"name": name}));
        all.len() - 1
    });
    &mut all[at]
}

/// The declaration of `thing` within a type, the type's own for `-`.
fn part<'t>(ty: &'t mut Value, thing: &'t str) -> &'t mut Value {
    match thing {
        "-" => ty,
        _ => {
            let (list, name) = held(ty, thing);
            named(list, name)
        }
    }
}

fn migrate_to(to: Value, renames: &[(&str, &str)]) -> Migration {
    let renames = renames.iter().map(|(a, b)| ((*a).into(), (*b).into()));
    Migration {
        to: serde_json::from_value(to).expect("a type"),
        renames: renames.collect(),
        relation_renames: Vec::new(),
    }
}

async fn a_migration_reports_every_member_its_to_states_otherwise(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let name = json!([{"name": "name", "type": {"kind": "text"}, "required": true}]);
    let function = |name: &str, expr: &str| {
        json!({"declare": "function", "name": name, "backing": "expr", "expr": expr,
               "inputs": [{"name": "title", "type": {"kind": "text"}}]})
    };
    for ty in ["person", "guest"] {
        let d = json!({"declare": "type", "name": ty, "key": ["name"], "fields": name});
        l.declare_json(d).await?;
    }
    l.declare_json(function("shout", "upper(title)")).await?;
    l.declare_json(function("hush", "lower(title)")).await?;
    let mut declared = note();
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    let table = ROWS.replace("MODEL", EMBED_MODEL);
    let rows = table.lines().filter(|l| !l.is_empty()).map(|line| {
        let (thing, rest) = line.split_once(' ').expect("a row");
        let (member, rest) = rest.split_once(' ').expect("a row");
        let (to, said) = rest.rsplit_once(' ').expect("a row");
        let to: Value = serde_json::from_str(to).expect("a row's JSON");
        (line, thing, member, to, said)
    });
    let rows: Vec<_> = rows.collect();
    let mut red = Vec::new();
    // Every member a declaration has is walked. A name is not changed
    // alone, and a list of declarations is walked by what it holds.
    for (kind, members, of) in kinds() {
        let listed = ["name", "fields", "relations"];
        let walked = |m: &str| rows.iter().any(|(_, t, n, ..)| of(t) && *n == m);
        // A field's type is `ty` in the pattern alone.
        let members = members.iter().map(|m| if *m == "ty" { "type" } else { m });
        for m in members.filter(|m| !listed.contains(m) && !walked(m)) {
            red.push(format!("no row changes `{m}` of {kind}"));
        }
    }
    // As the declarations serialise: what a member left out reads as.
    let stated = |ty: &Value| json!(migrate_to(ty.clone(), &[]).to);
    let before = stated(&note());
    for (line, thing, member, to, said) in rows {
        let mut ty = note();
        match (member, &to) {
            ("-", _) => {
                let (list, name) = held(&mut ty, thing);
                let list = list.as_array_mut().expect("a list of declarations");
                list.retain(|d| d["name"] != name);
            }
            (_, Value::Null) => drop(ty_part(&mut ty, thing).remove(member)),
            _ => part(&mut ty, thing)[member] = to,
        }
        let (mut was, mut is) = (before.clone(), stated(&ty));
        let was = part(&mut was, thing)[member].take();
        let is = part(&mut is, thing)[member].take();
        let answer = l.cat.migrate(&l.own, migrate_to(ty, &[])).await;
        let report = match &answer {
            Ok(r) => json!(r.data),
            Err(_) if said == "refused" && is_invalid(&answer) => continue,
            Err(e) => {
                red.push(format!("{line}: {e}"));
                continue;
            }
        };
        let of = |list: &str| report[list].as_array().cloned().unwrap_or_default();
        let named = |e: &Value| e["field"].as_str().unwrap_or("-") == thing;
        let entry = |e: &Value| e["member"] == member && e["was"] == was && e["to"] == is;
        let holds = match said {
            "fields" => of("fields").iter().any(named),
            "restated" => of("restated").iter().any(|e| named(e) && entry(e)),
            "dropped" => of("dropped").contains(&json!([thing, 0])),
            "nothing" => !of("restated").iter().any(named),
            _ => false,
        };
        if !holds {
            let lists = ["fields", "dropped", "restated"].map(|l| json!(of(l)).to_string());
            red.push(format!("{line}: for {was} to {is} it lists {lists:?}"));
        }
    }
    // `describe` shows a field one does not read by its shape alone.
    let masked = ["secret", "loud", "made", "near", "title"].map(|f| (f, Access::Masked));
    let who = field_access(store.as_ref(), "dim", "note", &masked);
    let shown = describe_type(l.cat.as_ref(), &who, "note").await?;
    let unread = [
        "derived",
        "function",
        "depends_on",
        "initial",
        "description",
        "label",
        "embed",
    ];
    for f in shown
        .fields
        .iter()
        .filter(|f| masked.iter().any(|(m, _)| f.name == *m))
    {
        let f = json!(f);
        let beyond = unread.iter().filter(|m| !f[**m].is_null());
        if beyond.count() > 0 || f["searchable"] == true || f["annotations"] != Value::Null {
            red.push(format!("a masked field is described beyond its shape: {f}"));
        }
    }
    let red = red.join("\n  ");
    ensure!(
        red.is_empty(),
        "a change made alone is in the report where its row says:\n  {red}"
    );
    Ok(())
}

/// The members of `thing`'s declaration within a type.
fn ty_part<'t>(ty: &'t mut Value, thing: &'t str) -> &'t mut serde_json::Map<String, Value> {
    part(ty, thing).as_object_mut().expect("a declaration")
}

/// What answers, by its name, what it must show and what it must not.
type Answered<'a> = (&'a (&'a str, String), &'a [&'a str], &'a [&'a str]);

/// The owner's proposal of `write`.
async fn proposed(l: &Lake, write: ProposedWrite) -> Result<ProposalId, Failure> {
    let proposed = l.cat.propose(&l.own, write, "why".into()).await;
    match proposed.must("proposing")? {
        Outcome::Proposed { proposal, .. } => Ok(proposal),
        other => Err(Failure::new(format!("a proposal is due: {other:?}"))),
    }
}

/// The proposal `id` as `who` is listed it, as JSON.
async fn listed(l: &Lake, who: &Principal, id: &ProposalId) -> Result<String, Failure> {
    let all = l.cat.proposals(who, ProposalFilter::default()).await;
    let mut all = all.must("listing")?.data;
    all.retain(|p| &p.id == id);
    Ok(json!(all).to_string())
}

async fn what_a_migration_restates_is_shown_as_its_fields_are_and_holds_at_the_accept(
    store: Store,
    scratch: PathBuf,
) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let marked = |n: &str, l: &str| json!({"name": n, "type": {"kind": "text"}, "label": l});
    let knows = json!({"name": "knows", "target": "person", "cardinality": "many_to_many",
                       "fields": [text("seen")]});
    let person = |fields: Value| json!({"name": "person", "key": ["name"], "fields": fields, "relations": [knows]});
    let name = json!({"name": "name", "type": {"kind": "text"}, "required": true});
    let mut secret = marked("secret", "Secret");
    (secret["sensitive"], secret["searchable"]) = (json!(true), json!(true));
    secret["description"] = json!("What is kept.");
    secret["annotations"] = json!({"acme.icon": "card"});
    // Computed from `secret` or from `name`, as declared or in a `to`.
    let computed = |n: &str, of: &str| {
        json!({"name": n, "type": {"kind": "text"}, "authority": "derived",
               "derived": format!("upper({of})"), "depends_on": [of]})
    };
    let (loud, soft) = (computed("loud", "secret"), computed("soft", "name"));
    let declare = |mut ty: Value| {
        ty["declare"] = json!("type");
        l.declare_json(ty)
    };
    let nick = marked("nick", "Nick");
    let mut declared = person(json!([name, secret, nick, loud, soft]));
    declared["label"] = json!("Person");
    declared["relations"][0]["fields"][0] = marked("seen", "Seen");
    declared["relations"][0]["fields"][0]["sensitive"] = json!(true);
    declare(declared.clone()).await?;
    let memo = |title: Value, note: Value| json!({"name": "memo", "fields": [title, note]});
    declare(memo(text("title"), text("note"))).await?;
    let ada = json!({"name": "ada", "secret": "s", "nick": "7"});
    l.upsert("person", "ada", ada).await?;
    // `to` restates nothing it was declared with, and renames `nick`; and
    // the same as one kept from `secret` writes it, the field left out.
    let (renames, alias) = ([("nick", "alias")], text("alias"));
    let stated = json!([name, text("secret"), alias, computed("loud", "name"), soft]);
    let bare = migrate_to(person(stated), &renames);
    let unknowing = migrate_to(person(json!([name, alias, soft])), &renames);
    // One that converts `nick` too, and one that restates nothing.
    let number = json!({"name": "nick", "type": {"kind": "number"}});
    let stated = json!([
        name,
        text("secret"),
        number,
        loud,
        computed("soft", "secret")
    ]);
    let numbered = migrate_to(person(stated), &[]);
    // The type declared again with a field more, and with `secret` required.
    let mut wider = declared.clone();
    wider["declare"] = json!("type");
    (wider["fields"].as_array_mut().expect("fields")).push(text("more"));
    let declarations = vec![serde_json::from_value(wider).expect("a declaration")];
    declared["fields"][1]["required"] = json!(true);
    let tightened = migrate_to(declared, &[]);
    let titled = migrate_to(memo(marked("title", "Title"), text("note")), &[]);
    // The readers: one kept from `secret`, one from the name `nick`.
    let mut hid = field_access(s, "hid", "person", &[("secret", Access::None)]);
    let mut old = field_access(s, "old", "person", &[("nick", Access::None)]);
    (hid.grants.review, hid.grants.schema, old.grants.review) = (true, true, true);
    let mut ids = Vec::new();
    for m in [bare.clone(), tightened, titled, numbered] {
        let (migration, report) = (Box::new(m), Default::default());
        ids.push(proposed(&l, ProposedWrite::Migrate { migration, report }).await?);
    }
    ids.push(proposed(&l, ProposedWrite::Declare { declarations }).await?);
    // Written since: a `nick` that is no number.
    let bob = json!({"name": "bob", "secret": "t", "nick": "x"});
    l.upsert("person", "bob", bob).await?;
    let whole = json!(preview(&l, &l.own, bare.clone()).await?).to_string();
    let own = json!(preview(&l, &hid, unknowing).await?).to_string();
    // Listed it in part, a reviewer is refused its accept (lakelands#867).
    let not_granted = Err(lakelands_api::Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::NotGranted,
    });
    let accepted = l.cat.accept(&hid, &ids[3]).await.map(|_| ());
    ensure!(
        accepted == not_granted,
        "one kept from `secret`, listed the migration in part, must be refused its accept as \
         not granted, naming nothing; it answered {accepted:?}"
    );
    let for_hid = listed(&l, &hid, &ids[0]).await?;
    let for_old = listed(&l, &old, &ids[0]).await?;
    let for_own = listed(&l, &l.own, &ids[0]).await?;
    // Two who know of `secret`, of `nick` and of the link field `seen` and
    // read none: what each is answered of a report, and of a shape listed.
    let link = link_type_name(&"person".into(), &"knows".into());
    let (mut reports, mut shapes, mut wholes) = (Vec::new(), Vec::new(), Vec::new());
    // `loud` is masked by a masked `secret`, and kept from one who only
    // writes it, who therefore leaves it out of its own `to`.
    let stated = json!([name, text("secret"), alias, soft]);
    let quiet = migrate_to(person(stated), &renames);
    for (access, own) in [(Access::Masked, &bare), (Access::WriteOnly, &quiet)] {
        let (id, fields) = (
            format!("{access:?}"),
            [("secret", access), ("nick", access)],
        );
        let mut who = field_access(s, &id, "person", &fields);
        (who.grants.review, who.grants.schema) = (true, true);
        let (default, fields) = (None, [("seen".into(), access)].into());
        let seen = TypeGrants {
            default,
            fields,
            ..Default::default()
        };
        who.grants.types.insert(link.clone(), seen);
        let own = json!(preview(&l, &who, own.clone()).await?).to_string();
        reports.push(("the preview of one who does not read `secret`", own));
        let listing = listed(&l, &who, &ids[0]).await?;
        reports.push(("the listing for one who does not read `secret`", listing));
        let accepted = l.cat.accept(&who, &ids[3]).await.map(|_| ());
        ensure!(
            accepted == not_granted,
            "one who does not read `secret`, listed the migration in part, must be refused its \
             accept as not granted, naming nothing; it answered {accepted:?}"
        );
        for id in [&ids[1], &ids[4]] {
            let listing = listed(&l, &who, id).await?;
            shapes.push(("the shape listed for one who does not read it", listing));
        }
    }
    for id in [&ids[1], &ids[4]] {
        let listing = listed(&l, &l.own, id).await?;
        wholes.push(("the shape the owner is listed", listing));
    }
    // The log as the host reads it, in the feed and in an export.
    let proposed = |e: &lakelands_api::event::Event| e.body.kind() == "write_proposed";
    let events = log_events(s).await?;
    let feed = json!(events.into_iter().filter(proposed).collect::<Vec<_>>()).to_string();
    let dir = Scratch::new(scratch).dir("restated");
    let exported = crate::export::write(s, &dir).await;
    exported.must("exporting")?;
    let unread = |e: std::io::Error| Failure::new(format!("reading the export: {e}"));
    let mut export = String::new();
    for file in std::fs::read_dir(dir.join("log")).map_err(unread)? {
        export += &std::fs::read_to_string(file.map_err(unread)?.path()).map_err(unread)?;
    }
    // Per row: what answers, what it must show and what it must not.
    let mark = "{\"field\":\"secret\",\"member\":\"sensitive\",\"to\":false,\"was\":true}";
    let found = "{\"field\":\"secret\",\"member\":\"searchable\",\"to\":false,\"was\":true}";
    let label = "{\"member\":\"label\",\"to\":null,\"was\":\"Person\"}";
    let alias = "{\"field\":\"alias\",\"member\":\"name\",\"to\":\"alias\",\"was\":\"nick\"}";
    let (more, lossless, lost) = (
        "\"more\":true",
        "\"lossless\":true",
        "\"not_convertible\":1",
    );
    let empty = "\"restated\":[]";
    let whole = ("the owner's preview", whole);
    let own = (
        "the preview of one kept from `secret`, of its own `to`",
        own,
    );
    let accepted = refused(l.cat.accept(&l.own, &ids[3]).await)?;
    let accepted = ("the owner's refused accept, not lossless", accepted);
    let for_hid = ("the listing for one kept from `secret`", for_hid);
    let for_old = ("the listing for one kept from the name `nick`", for_old);
    let for_own = ("the owner's listing", for_own);
    let (feed, export) = (("the feed", feed), ("an export", export));
    let rows: [Answered; 8] = [
        (&whole, &[mark, found, label, alias, lossless], &["more"]),
        (&own, &[label, alias], &["secret", more]),
        (&for_hid, &[label, alias, more], &["secret"]),
        (&for_old, &[label, mark, more], &["alias", "nick"]),
        (&for_own, &[mark, label, alias], &[more]),
        (&accepted, &[label, lost], &[more]),
        (&feed, &[mark, label, alias], &[empty, more]),
        (&export, &[mark, label, alias], &[empty, more]),
    ];
    // The shape alone: `sensitive`, and nothing a reader of the field reads.
    let seen = "{\"field\":\"knows.seen\",\"member\":\"sensitive\",\"to\":false,\"was\":true}";
    let searched = "\"searchable\":true";
    let beyond = [
        "\"Secret\"",
        found,
        searched,
        "What is kept.",
        "acme.icon",
        "\"Seen\"",
        "\"Nick\"",
        "upper(secret)",
    ];
    let (said, shape) = ([mark, seen, label, more], ["\"sensitive\":true", more]);
    let every = ["\"Secret\"", searched, "\"Seen\"", "upper(secret)"];
    let reports = reports.iter().map(|a| (a, &said[..], &beyond[..]));
    let shapes = shapes.iter().map(|a| (a, &shape[..], &beyond[..]));
    let wholes = wholes.iter().map(|a| (a, &every[..], &[][..]));
    let rows = rows.into_iter().chain(reports).chain(shapes).chain(wholes);
    let mut red = Vec::new();
    for ((what, report), shows, hides) in rows {
        let has = |n: &&str| report.contains(n);
        if !shows.iter().all(has) || hides.iter().any(has) {
            red.push(format!(
                "{what}: {shows:?} and not {hides:?} are due in {report}"
            ));
        }
    }
    // Four migrations and a declaration are proposed, and three restate.
    for (what, log) in [&feed, &export] {
        let count = |of: &str| log.matches(of).count();
        let (all, restating) = (count("write_proposed"), count("\"restated\""));
        if (all, restating) != (5, 3) {
            red.push(format!(
                "{what}: {restating} of {all} proposals hold `restated`: {log}"
            ));
        }
    }
    let red = red.join("\n  ");
    ensure!(
        red.is_empty(),
        "what a migration restates is answered as its reader may know it:\n  {red}"
    );

    // A mark set while a proposal waited is not taken away by its accept.
    let mut sensitive = text("note");
    sensitive["sensitive"] = json!(true);
    declare(memo(text("title"), sensitive)).await?;
    let stale = l.cat.accept(&l.own, &ids[2]).await;
    let said =
        matches!(invalid_case(&stale), Some(Case::ChangedSinceProposed { ty }) if ty == "memo");
    let after = json!(describe_type(l.cat.as_ref(), &l.own, "memo").await?).to_string();
    ensure!(
        said && after.contains("\"sensitive\":true"),
        "the accept of a migration proposed before `note` was declared sensitive is refused \
         as one of a type changed since, and the mark stays; it answered {stale:?}, and \
         `memo` is {after}"
    );
    // One whose type restates what its report said is accepted.
    let accepted = l.cat.accept(&l.own, &ids[0]).await;
    accepted.must("accepting the migration that restates the marks")?;
    Ok(())
}
