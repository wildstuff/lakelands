//! A schema hash is answered over the shape its reader is shown
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! extended): by `describe`, in a preview and in a listed proposal; and a
//! report's id is made over what the report shows, so that neither says
//! that a type has a field its reader may not know of.

use crate::conformance::prelude::*;

check!(
    HASH_OF_THE_SHOWN_SHAPE = a_schema_hash_is_that_of_the_shape_its_reader_is_shown,
    "024, extended (lakelands#644): a schema hash answered to a reader is that of the declaration it is shown, without the fields and link fields it may not know of, the relations it is not shown (lakelands#662) and a view it is not described, in `describe`, a preview and a listed proposal; `more` is decided before the hashes, a report's id is made over what it shows and the head, the `to` hash is judged through the migration's renames, a stale proposal's `from` is that of the type it was made from; the accept, the log and the owner keep the whole hash"
);

check!(
    STALE_BY_ITS_NAMES = a_stale_proposal_is_judged_by_the_names_it_was_made_with,
    "024 and 006, extended (lakelands#644): a proposed migration made while a field bore its old name, listed after the field was renamed and is hidden under its new name, names it in neither its `to`, its report nor its hashes, and says `more`: the reader's grants are followed to the proposal's position"
);

/// `account` with `score` of that kind, `iban` required or not, `note`
/// when given, and the relation `kept_by`, whose links hold `since`.
fn account(score: &str, iban: bool, note: bool) -> Value {
    let field = |n: &str, kind: &str, required: bool| json!({"name": n, "type": {"kind": kind}, "required": required});
    let mut fields = vec![
        field("number", "text", true),
        field("iban", "text", iban),
        field("score", score, true),
    ];
    fields.extend(note.then(|| field("note", "text", false)));
    let since = json!([field("since", "text", false)]);
    let kept_by = json!({"name": "kept_by", "target": "person", "fields": since});
    json!({"name": "account", "key": ["number"], "fields": fields, "relations": [kept_by]})
}

fn to(ty: Value) -> Migration {
    let to = serde_json::from_value(ty).expect("the check's type reads");
    crate::conformance::fixture::migration(to)
}

/// The hash `describe_type` answers `who` for `ty`, the hash of the
/// declaration it describes, and the version.
async fn described(l: &Lake, who: &Principal, ty: &str) -> Result<(String, String, u32), Failure> {
    let d = describe_type(l.cat.as_ref(), who, ty).await?;
    let field = |f: &lakelands_api::describe::FieldDescription| json!({"name": f.name, "type": f.ty, "required": f.required});
    let fields: Vec<Value> = d.fields.iter().map(field).collect();
    let (key, relations, view) = (&d.access.key, &d.relations, &d.view);
    let t =
        json!({"name": d.name, "key": key, "fields": fields, "relations": relations, "view": view});
    let t: TypeDecl = serde_json::from_value(t).expect("a description reads as a declaration");
    let rebuilt = crate::shape::schema_hash(&t).expect("a described view parses");
    Ok((d.lifecycle.schema_hash, rebuilt, d.lifecycle.version))
}

/// The migration reports `who` is listed, newest first.
async fn listed(l: &Lake, who: &Principal) -> Result<Vec<(Migration, MigrationReport)>, Failure> {
    let all = l.cat.proposals(who, ProposalFilter::default()).await;
    let all = all.must("listing")?;
    let reports = all.data.into_iter().filter_map(|p| match p.write {
        Some(ProposedWrite::Migrate { migration, report }) => Some((*migration, *report)),
        _ => None,
    });
    Ok(reports.collect())
}

async fn proposed(l: &Lake, who: &Principal, m: Migration) -> Result<ProposalId, Failure> {
    let write = ProposedWrite::Migrate {
        migration: Box::new(m),
        report: Default::default(),
    };
    match l.cat.propose(who, write, "the check".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!("a proposal is due: {other:?}"))),
    }
}

async fn applied(l: &Lake, m: Migration) -> Held {
    let report = l.cat.migrate(&l.own, m.clone()).await.must("previewing")?;
    let done = l.cat.migrate_apply(&l.own, m, &report.data.report_id).await;
    done.must("applying").map(drop)
}

async fn a_schema_hash_is_that_of_the_shape_its_reader_is_shown(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let text = json!({"kind": "text"});
    let person =
        json!({"name": "person", "key": ["name"], "fields": [{"name": "name", "type": text}]});
    let by_iban = json!({"name": "by_iban", "key": ["iban"],
        "fields": [{"name": "iban", "type": text}, {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "account", "group_by": ["iban"],
                 "measures": [{"name": "n", "expr": "count()"}]}});
    for mut ty in [person, account("text", true, false), by_iban] {
        ty["declare"] = json!("type");
        l.declare_json(ty).await?;
    }
    let one = json!({"number": "1", "iban": "DE1", "score": "10"});
    l.upsert("account", "1", one).await?;
    // Kept from `score` and from the link field `since`, with `iban`
    // masked; it reviews and declares. One more, without `schema`, is kept
    // from `person`, the relation's target, and is not described the
    // relation, nor its hash; with `iban` masked, it is not described the
    // view.
    let fields = [("score", Access::None), ("iban", Access::Masked)];
    let mut hid = field_access(s, "hid", "account", &fields);
    let since = TypeGrants {
        fields: [("since".into(), Access::None)].into(),
        ..Default::default()
    };
    hid.grants
        .types
        .insert("sys.link.account.kept_by".into(), since);
    (hid.grants.review, hid.grants.schema) = (true, true);
    let mut far = field_access(s, "far", "account", &fields[1..]);
    let person = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    far.grants.types.insert("person".into(), person);

    // Each reader is answered the hash of the declaration it is described.
    let mut differ = Vec::new();
    let pairs = [&l.own, &hid, &far].map(|w| [(w, "account"), (w, "by_iban")]);
    for (who, ty) in pairs.concat() {
        let (answered, rebuilt, _) = described(&l, who, ty).await?;
        differ.extend((answered != rebuilt).then(|| (who.id.clone(), ty, answered, rebuilt)));
    }
    ensure!(
        differ.is_empty(),
        "hashes differ from the shapes described: {differ:?}"
    );
    let seen = describe_type(l.cat.as_ref(), &far, "account").await?;
    ensure!(
        seen.relations.is_empty(),
        "far, kept from `person`, is not described `kept_by`: {:?}",
        seen.relations
    );
    let (_, _, v0) = described(&l, &l.own, "account").await?;
    let (h0, _, _) = described(&l, &hid, "account").await?;

    // A preview of what touches nothing hidden: its hashes are the shapes
    // the reader sent and was shown, `more` is false, and the reader makes
    // its id from the report and the head.
    let mut sent = account("text", true, true);
    let kept: Vec<Value> = (sent["fields"].as_array().into_iter().flatten())
        .filter(|f| f["name"] != "score")
        .cloned()
        .collect();
    (sent["fields"], sent["relations"][0]["fields"]) = (json!(kept), json!([]));
    let sent = to(sent);
    let preview = l.cat.migrate(&hid, sent.clone()).await;
    let preview = preview.must("hid's preview")?;
    let (head, report) = (preview.position, preview.data);
    let made = json!({"report": MigrationReport { report_id: String::new(), ..report.clone() },
        "head": head.0});
    let sent_hash = crate::shape::schema_hash(&sent.to).expect("no view");
    ensure!(
        (report.from_schema_hash == h0 && report.to_schema_hash == sent_hash && !report.more)
            && report.report_id == lakelands_api::canonical::sha256_hex(&made),
        "hid's preview is answered the hashes {h0} and {sent_hash}, no more, and the id of what it \
         shows at {head:?}; it is answered {report:?}"
    );

    // Proposed and listed so; the owner accepts it on the whole hash, and
    // the log holds the whole `to`'s.
    let first = proposed(&l, &hid, sent).await?;
    let shown = listed(&l, &hid).await?.pop().map(|(_, r)| r);
    ensure!(
        shown.as_ref() == Some(&report),
        "hid lists the report it was shown: {shown:?}"
    );
    let before = s.head().await.must("the head")?;
    let fut = l.cat.accept(&l.own, &first);
    fut.await.must("the owner accepts")?;
    let (whole, _, v1) = described(&l, &l.own, "account").await?;
    let logged = events_after(s, before);
    let logged = logged.await?.into_iter().find_map(|e| match e.body {
        EventBody::Migrated { schema_hash, .. } => Some(schema_hash),
        _ => None,
    });
    let want = crate::shape::schema_hash(&to(account("text", true, true)).to).ok();
    ensure!(
        logged == want && Some(&whole) == want.as_ref() && v1 > v0 && whole != h0,
        "the log and the owner hold the whole hash {want:?}; the log holds {logged:?}, the owner \
         {whole}"
    );

    // The owner proposes `score` renamed: the reader is shown the `to` it is
    // listed, without `points`, which it may not know of either.
    let mut renamed = to(account("text", true, true));
    renamed.to.fields[2].name = "points".into();
    renamed.renames = vec![("score".into(), "points".into())];
    proposed(&l, &l.own, renamed).await?;
    let (h1, _, _) = described(&l, &hid, "account").await?;
    let check_listed = |list: Vec<(Migration, MigrationReport)>, from: &str| {
        let Some((m, r)) = list.into_iter().next() else {
            return Err("hid lists no rename".to_owned());
        };
        let to = crate::shape::schema_hash(&m.to).unwrap_or_default();
        match r.from_schema_hash == from && r.to_schema_hash == to {
            true => Ok(()),
            false => Err(format!("the rename is listed from {from} to {to}: {r:?}")),
        }
    };
    check_listed(listed(&l, &hid).await?, &h1).map_err(Failure::new)?;

    // A hidden field retyped moves the owner's hash and not the reader's;
    // both versions move. `iban` loosened moves both: the rename, stale,
    // is listed from the shape it was made from.
    applied(&l, to(account("number", true, true))).await?;
    let ((h2, _, v2), (whole2, _, _)) = (
        described(&l, &hid, "account").await?,
        described(&l, &l.own, "account").await?,
    );
    ensure!(
        h2 == h1 && whole2 != whole && v2 > v1,
        "a hidden field retyped leaves hid's hash {h1} ({h2}) and moves the owner's {whole} \
         ({whole2}) and the version {v1} ({v2})"
    );
    applied(&l, to(account("number", false, true))).await?;
    let (h3, _, _) = described(&l, &hid, "account").await?;
    ensure!(h3 != h1, "`iban` loosened moves hid's hash {h1}");
    check_listed(listed(&l, &hid).await?, &h1).map_err(Failure::new)
}

async fn a_stale_proposal_is_judged_by_the_names_it_was_made_with(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    // Each field `name` of text, or `name:kind`.
    let ledger = |fields: &[&str]| {
        let field = |f: &&str| {
            let (n, kind) = f.split_once(':').unwrap_or((f, "text"));
            json!({"name": n, "type": {"kind": kind}})
        };
        let fields: Vec<Value> = fields.iter().map(field).collect();
        to(json!({"name": "ledger", "key": ["number"], "fields": fields}))
    };
    let mut declared = serde_json::to_value(ledger(&["number", "score"]).to).expect("a type");
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    // Proposed while the field is `score`, retyping it; renamed to `points`
    // since.
    proposed(&l, &l.own, ledger(&["number", "score:number", "note"])).await?;
    let mut renamed = ledger(&["number", "points"]);
    renamed.renames = vec![("score".into(), "points".into())];
    applied(&l, renamed).await?;
    let mut late = field_access(s, "late", "ledger", &[("points", Access::None)]);
    late.grants.review = true;
    let Some((m, r)) = listed(&l, &late).await?.pop() else {
        return Err(Failure::new("the reviewer lists the proposal"));
    };
    let shown = |m: &Migration| crate::shape::schema_hash(&m.to).unwrap_or_default();
    let (from, to) = (
        shown(&ledger(&["number"])),
        shown(&ledger(&["number", "note"])),
    );
    let names: Vec<&str> = m.to.fields.iter().map(|f| f.name.as_str()).collect();
    let report = serde_json::to_string(&r).unwrap_or_default();
    ensure!(
        names == ["number", "note"]
            && r.from_schema_hash == from
            && r.to_schema_hash == to
            && r.more
            && !report.contains("\"score\""),
        "a field hidden as `points` is named under `score` in neither the listed `to`, the \
         report nor the hashes {from} and {to}, and the report says `more`: listed {names:?}, \
         {report}"
    );
    Ok(())
}
