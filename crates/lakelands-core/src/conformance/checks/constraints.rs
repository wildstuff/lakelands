//! A type's constraints hold at every write door (lakelands#1119).
//!
//! A constraint is a condition over a record's own fields that every write
//! moving a field it reads must leave true: an `upsert`, a verb's write,
//! and `ingest`, which may admit what its source sends instead. One who
//! does not read every field a constraint reads is not granted a write the
//! constraint judges, whatever the values. A constraint is put on a type
//! that holds records by a migration, whose preflight lists the records
//! that would break it.

use lakelands_api::describe::TypeDescription;
use lakelands_api::error::{Beyond, Case};
use lakelands_api::ontology::{AtIngest, TypeDecl};
use lakelands_api::write::{Declaration, Migration, Outcome};
use lakelands_api::{Access, Error, Refusal};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Answer, Lake, Must, describe_type, field_access, head_unmoved, ingest, invalid_case, patch,
    reactive, reason, reviewer, source_record, try_upsert, write_verb,
};
use crate::conformance::{Failure, Held, Store};

check!(
    CONSTRAINT_HELD_AT_EVERY_DOOR = a_constraint_holds_at_every_write_door,
    "lakelands#1119: a type's constraint is judged at `upsert`, at a verb's write, at a reaction's write and at `ingest` on the record as written, with `previous` as it was before; a write that would break it is refused `constraint_broken`, naming the constraint, the type and the key and no value, and appends nothing; a write that leaves the fields it reads as they were is not judged, so a record that breaks a constraint `ingest` admitted takes other writes and only a fix of its own; a constraint that refuses at ingest refuses the record there"
);

check!(
    CONSTRAINT_JUDGED_THROUGH_WHAT_MOVES = a_write_is_judged_by_the_fields_it_moves,
    "lakelands#1119: a write is judged by a constraint when it moves a field the constraint reads through a derived field, a field an `expr` function computes at the write, or a field the fold clears when its inputs change, as when it sends the field itself"
);

check!(
    CONSTRAINT_TOLD_AT_ACCEPT = a_reviewer_who_reads_a_constraint_is_told_it_at_accept,
    "lakelands#1119 and 013: a proposal that held when it was proposed and breaks a constraint when it is accepted is refused `constraint_broken` to a reviewer who reads every field the constraint reads, and `write_changed_since_proposed` to one who does not"
);

check!(
    CONSTRAINT_NOT_GRANTED_UNREAD =
        a_writer_kept_from_what_a_constraint_reads_is_not_granted_what_it_judges,
    "lakelands#1119 and 006: a writer that does not read every field a constraint reads is refused `not_granted` for a write the constraint judges, one that creates the record or sends a field it reads, whether it would hold or not, so the answer tells nothing of the field it may not read; its writes to other fields are judged as before"
);

check!(
    CONSTRAINT_READS_ITS_RECORD_ALONE =
        a_constraint_reads_its_record_alone_and_is_added_by_a_migration,
    "lakelands#1119: a constraint names itself once and reads its record alone, its own fields as stored or computed at the write, `previous` and `changed`; a path, `exists`, a setting, the clock or a field computed at the read is refused at the declaration, naming the constraint; a type that holds a constraint is declared again with it removed, restated in other words, described otherwise or moved to admit at ingest, and one added, changed or moved to refuse is refused naming `migrate`; `describe_type` tells a constraint to one who reads every field it reads"
);

check!(
    CONSTRAINT_BROKEN_IN_THE_PREFLIGHT =
        a_migration_lists_the_records_its_constraints_would_find_broken,
    "lakelands#1119: a migration whose `to` declares a constraint lists in its preflight, per constraint, the live records it would leave breaking it, as converted, with the first keys its reader is answered; one that refuses makes the migration lossy and its apply is refused, one that admits is listed and applies; one who does not read every field a constraint reads, kept from one or masked, is not told its entry nor the constraint in `restated`, and is told `more`, and the constraint, which its declaration or migration leaves out, stands as declared"
);

/// A type of invoices: `paid` never beyond `amount`, and a closed one
/// stays closed. `authority` is its fields' but `note`'s, which is
/// authored.
fn invoice(name: &str, authority: &str, at_ingest: &str) -> Value {
    let field =
        |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": authority});
    json!({"declare": "type", "name": name, "key": ["no"],
        "fields": [field("no", "text"), field("amount", "number"), field("paid", "number"),
                   field("status", "text"), {"name": "note", "type": {"kind": "text"}}],
        "constraints": [
            {"name": "never_overpaid", "holds": "paid <= amount", "at_ingest": at_ingest},
            {"name": "closed_stays",
             "holds": "not (previous(status) = 'closed') or status = 'closed'"}]})
}

/// The type a declaration's JSON describes, its `declare` tag dropped.
fn type_of(mut json: Value) -> Result<TypeDecl, Failure> {
    json.as_object_mut().map(|o| o.remove("declare"));
    serde_json::from_value(json)
        .map_err(|e| Failure::new(format!("the check's type does not read: {e}")))
}

/// The declaration `json` describes, its answer as it came if it is an
/// outcome or a refusal.
async fn try_declare(l: &Lake, json: Value) -> Result<lakelands_api::Result<Outcome>, Failure> {
    let d: Declaration = serde_json::from_value(json)
        .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
    let a = l.cat.declare(&l.own, d).await;
    match &a {
        Err(e) if reason(&a).is_none() => Err(Failure::new(format!("declaring failed: {e}"))),
        _ => Ok(a),
    }
}

/// The constraint a refusal says is broken, if it says one is.
fn broken(a: &impl Answer) -> Option<String> {
    let Some(Refusal::Constraint(said)) = reason(a) else {
        return None;
    };
    match &said.case {
        Case::ConstraintBroken { constraint, .. } => Some(constraint.to_string()),
        _ => None,
    }
}

async fn a_constraint_holds_at_every_write_door(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(invoice("invoice", "authored", "refuse"))
        .await?;
    l.declare_as(
        write_verb("pay", "invoice", "paid", "paid + 100"),
        "declaring `pay`",
    )
    .await?;
    l.upsert(
        "invoice",
        "I-1",
        json!({"no": "I-1", "amount": 100, "paid": 50}),
    )
    .await?;
    let head = store.head().await.must("reading the head")?;
    let cat = l.cat.as_ref();
    let over = try_upsert(
        cat,
        &l.own,
        "invoice",
        "I-2",
        json!({"no": "I-2", "amount": 100, "paid": 150}),
    )
    .await;
    let raised = try_upsert(cat, &l.own, "invoice", "I-1", json!({"paid": 150})).await;
    let key = "I-1".into();
    let paid = cat
        .call(
            &l.own,
            &"pay".into(),
            Some(&key),
            patch(json!({})),
            Default::default(),
        )
        .await;
    let reacting = store.catalog(reactive(1, None));
    let reaction = try_upsert(
        reacting.as_ref(),
        &l.own,
        "invoice",
        "I-1",
        json!({"paid": 150}),
    )
    .await;
    let said: Vec<_> = [&over, &raised, &paid, &reaction]
        .iter()
        .map(|a| broken(*a))
        .collect();
    ensure!(
        said.iter().all(|c| c.as_deref() == Some("never_overpaid")),
        "a create, an upsert, a verb and a reaction that leave `paid` beyond `amount` are \
         refused `never_overpaid`: {over:?}, {raised:?}, {paid:?}, {reaction:?}"
    );
    let told = format!("{:?}", over.as_ref().err());
    ensure!(!told.contains("150"), "the refusal names no value: {told}");
    head_unmoved(store.as_ref(), head, "a write that breaks a constraint").await?;
    // A write that moves nothing a constraint reads is not judged by it,
    // and `previous` reads the record before the write.
    l.upsert("invoice", "I-1", json!({"note": "n", "status": "closed"}))
        .await?;
    let reopened = try_upsert(cat, &l.own, "invoice", "I-1", json!({"status": "open"})).await;
    ensure!(
        broken(&reopened).as_deref() == Some("closed_stays"),
        "a closed invoice reopened is refused `closed_stays`: {reopened:?}"
    );
    // At ingest: one type refuses, its twin admits.
    for at_ingest in ["refuse", "admit"] {
        let ty = format!("fed_{at_ingest}");
        l.declare_json(invoice(&ty, "shared", at_ingest)).await?;
        let r = source_record(&ty, "F-1", json!({"no": "F-1", "amount": 10, "paid": 99}));
        let fed = ingest(cat, &l.own, "feed", vec![r]).await;
        let refused = broken(&fed).as_deref() == Some("never_overpaid");
        ensure!(
            refused == (at_ingest == "refuse") && (refused || fed.is_ok()),
            "`{ty}` at ingest: a record over its amount is answered {fed:?}"
        );
    }
    // An admitted breach takes a write that moves nothing the constraint
    // reads, and of what it reads, only a fix.
    l.upsert("fed_admit", "F-1", json!({"note": "seen", "paid": 99}))
        .await?;
    let still = try_upsert(cat, &l.own, "fed_admit", "F-1", json!({"paid": 50})).await;
    ensure!(
        broken(&still).as_deref() == Some("never_overpaid"),
        "an admitted breach moved and still broken is refused: {still:?}"
    );
    l.upsert("fed_admit", "F-1", json!({"paid": 10})).await?;
    Ok(())
}

async fn a_write_is_judged_by_the_fields_it_moves(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let owe = json!({"declare": "function", "name": "owe", "backing": "expr", "expr": "amount - paid",
                     "inputs": [{"name": "amount", "type": {"kind": "number"}},
                                {"name": "paid", "type": {"kind": "number"}}]});
    l.declare_json(owe).await?;
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    let computed = |how: Value| {
        let mut f = json!({"name": "rest", "type": {"kind": "number"}, "authority": "derived",
                           "depends_on": ["amount", "paid"]});
        if let (Some(o), Some(how)) = (f.as_object_mut(), how.as_object()) {
            o.extend(how.clone());
        }
        f
    };
    for (ty, how) in [
        ("by_derived", json!({"derived": "amount - paid"})),
        ("by_function", json!({"function": "owe"})),
    ] {
        l.declare_json(json!({"declare": "type", "name": ty, "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"), number("paid"), computed(how)],
            "constraints": [{"name": "covered", "holds": "rest >= 0"}]}))
            .await?;
        l.upsert(ty, "L-1", json!({"no": "L-1", "amount": 100, "paid": 50}))
            .await?;
        let over = try_upsert(l.cat.as_ref(), &l.own, ty, "L-1", json!({"paid": 150})).await;
        ensure!(
            broken(&over).as_deref() == Some("covered"),
            "`{ty}`: a write of `paid` that leaves `rest` below zero is refused `covered`: {over:?}"
        );
    }
    l.declare_json(json!({"declare": "type", "name": "approval", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"),
                   {"name": "status", "type": {"kind": "text"}},
                   {"name": "approved_by", "type": {"kind": "text"}, "depends_on": ["amount"],
                    "on_input_change": "clear"}],
        "constraints": [{"name": "approved_has_approver",
                         "holds": "not (status = 'approved') or approved_by is not null"}]}))
        .await?;
    let fields = json!({"no": "A-1", "amount": 10, "status": "approved", "approved_by": "ana"});
    l.upsert("approval", "A-1", fields).await?;
    let moved = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "approval",
        "A-1",
        json!({"amount": 20}),
    )
    .await;
    ensure!(
        broken(&moved).as_deref() == Some("approved_has_approver"),
        "a write of `amount` that clears `approved_by` on an approved record is refused: {moved:?}"
    );
    Ok(())
}

async fn a_reviewer_who_reads_a_constraint_is_told_it_at_accept(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(invoice("invoice", "authored", "refuse"))
        .await?;
    l.upsert(
        "invoice",
        "I-1",
        json!({"no": "I-1", "amount": 100, "paid": 50}),
    )
    .await?;
    let write = json!({"write": "upsert", "type": "invoice", "key": "I-1", "patch": {"paid": 90}});
    let mut proposals = Vec::new();
    for _ in 0..2 {
        let w = serde_json::from_value(write.clone()).map_err(|e| Failure::new(e.to_string()))?;
        let proposed = l.cat.propose(&l.own, w, "pay most".into()).await;
        let Outcome::Proposed { proposal, .. } = proposed.must("proposing a payment")? else {
            return Err(Failure::new("a proposal must be proposed"));
        };
        proposals.push(proposal);
    }
    // `amount` drops under what the proposals pay.
    l.upsert("invoice", "I-1", json!({"amount": 80})).await?;
    let all = reviewer(store.as_ref(), false);
    let mut kept = field_access(
        store.as_ref(),
        "kept",
        "invoice",
        &[("amount", Access::None)],
    );
    kept.grants.review = true;
    let told = l.cat.accept(&all, &proposals[0]).await;
    let untold = l.cat.accept(&kept, &proposals[1]).await;
    let changed = matches!(
        untold.as_ref().err().and_then(Error::case).map(|s| &s.case),
        Some(Case::WriteChangedSinceProposed { .. })
    );
    ensure!(
        broken(&told).as_deref() == Some("never_overpaid") && changed,
        "at accept, a reviewer who reads `amount` is told `never_overpaid` and one kept from it \
         that the write changed: {told:?}, {untold:?}"
    );
    Ok(())
}

async fn a_writer_kept_from_what_a_constraint_reads_is_not_granted_what_it_judges(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(invoice("invoice", "authored", "refuse"))
        .await?;
    l.upsert(
        "invoice",
        "I-1",
        json!({"no": "I-1", "amount": 100, "paid": 50}),
    )
    .await?;
    let access = [
        ("amount", Access::None),
        ("paid", Access::Write),
        ("note", Access::Write),
    ];
    let kept = field_access(store.as_ref(), "kept", "invoice", &access);
    let cat = l.cat.as_ref();
    // Twins that differ only in what `amount` holds against them.
    let holds = try_upsert(cat, &kept, "invoice", "I-1", json!({"paid": 60})).await;
    let breaks = try_upsert(cat, &kept, "invoice", "I-1", json!({"paid": 500})).await;
    let grant = |a: &lakelands_api::Result<Outcome>| matches!(reason(a), Some(Refusal::NotGranted));
    ensure!(
        grant(&holds) && grant(&breaks),
        "kept from `amount`, a write of `paid` is not granted whether it holds or not: \
         {holds:?}, {breaks:?}"
    );
    let note = try_upsert(cat, &kept, "invoice", "I-1", json!({"note": "n"})).await;
    note.must("kept from `amount`, a write of `note`, which no constraint judges")?;
    Ok(())
}

async fn a_constraint_reads_its_record_alone_and_is_added_by_a_migration(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let with = |constraints: Value| {
        json!({"declare": "type", "name": "t", "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}},
                       {"name": "n", "type": {"kind": "number"}},
                       {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
                        "derived": "n > 1", "depends_on": ["n"], "computed_at": "read"}],
            "relations": [{"name": "next", "target": "t"}],
            "constraints": constraints})
    };
    let one = |holds: &str| json!([{"name": "c", "holds": holds}]);
    let mut said = Vec::new();
    for (holds, beyond) in [
        ("setting('cap') = 'x'", Beyond::Setting),
        ("now() > now()", Beyond::Clock),
        ("exists(next)", Beyond::Exists),
        ("next.n > 1", Beyond::Path),
        (
            "late",
            Beyond::ComputedAtRead {
                field: "late".into(),
            },
        ),
    ] {
        let a = try_declare(&l, with(one(holds))).await?;
        let right = matches!(invalid_case(&a),
            Some(Case::ConstraintReadsBeyond { reads, .. }) if **reads == beyond);
        said.push((holds, right, a));
    }
    let twice = json!([{"name": "c", "holds": "n > 0"}, {"name": "c", "holds": "n < 9"}]);
    let twice = try_declare(&l, with(twice)).await?;
    ensure!(
        said.iter().all(|s| s.1)
            && matches!(
                invalid_case(&twice),
                Some(Case::ConstraintNamedTwice { .. })
            ),
        "a constraint reading a setting, the clock or a field computed at the read, and \
         one named twice, are refused at the declaration: {said:?}, {twice:?}"
    );
    // Added by `declare` to a type in force: refused, naming `migrate`;
    // taken away: declared.
    l.declare_json(with(one("n > 0"))).await?;
    let both = json!([{"name": "c", "holds": "n > 0"}, {"name": "d", "holds": "n < 9"}]);
    let added = try_declare(&l, with(both)).await?;
    // Restated in other words, described and admitting: no record breaks
    // it that did not before.
    let looser =
        json!([{"name": "c", "holds": "n>0", "description": "positive", "at_ingest": "admit"}]);
    let loosened = try_declare(&l, with(looser)).await?;
    let tighter = try_declare(&l, with(json!([{"name": "c", "holds": "n>0"}]))).await?;
    let removed = try_declare(&l, with(json!([]))).await?;
    let migrate = |a: &lakelands_api::Result<Outcome>| {
        matches!(invalid_case(a), Some(Case::ConstraintNeedsMigrate { .. }))
    };
    ensure!(
        migrate(&added) && loosened.is_ok() && migrate(&tighter) && removed.is_ok(),
        "by `declare`, a constraint added or moved to refuse at ingest is refused naming \
         `migrate`, one loosened or taken away is declared: {added:?}, {loosened:?}, \
         {tighter:?}, {removed:?}"
    );
    // Told to one who reads what it reads.
    l.declare_json(invoice("invoice", "authored", "refuse"))
        .await?;
    let kept = field_access(
        store.as_ref(),
        "kept",
        "invoice",
        &[("amount", Access::None)],
    );
    let told = |d: TypeDescription| {
        let names = d.constraints.iter().map(|c| c.name.to_string());
        names.collect::<Vec<_>>()
    };
    let owner = told(l.describe_type("invoice").await?);
    let theirs = told(describe_type(l.cat.as_ref(), &kept, "invoice").await?);
    ensure!(
        owner == ["never_overpaid", "closed_stays"] && theirs == ["closed_stays"],
        "`describe_type` tells the owner both constraints and one kept from `amount` the \
         other: {owner:?}, {theirs:?}"
    );
    Ok(())
}

async fn a_migration_lists_the_records_its_constraints_would_find_broken(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let mut bill = invoice("bill", "authored", "refuse");
    bill["constraints"] = json!([]);
    l.declare_json(bill).await?;
    for (k, paid) in [("b1", 50), ("b2", 150), ("b3", 170)] {
        l.upsert("bill", k, json!({"no": k, "amount": 100, "paid": paid}))
            .await?;
    }
    let to = |at_ingest: &str| type_of(invoice("bill", "authored", at_ingest)).map(Migration::to);
    let refusing = l.cat.migrate(&l.own, to("refuse")?).await;
    let report = refusing.must("previewing a refusing constraint")?.data;
    let entry = report
        .breaches
        .iter()
        .find(|b| b.constraint.as_str() == "never_overpaid");
    let entry = entry.map(|b| {
        let keys = b.sample_keys.iter().map(|k| k.to_string());
        (b.records, keys.collect::<Vec<_>>())
    });
    ensure!(
        entry == Some((2, vec!["b2".to_owned(), "b3".to_owned()]))
            && report.breaches.len() == 1
            && !report.lossless,
        "the preflight lists `b2` and `b3` breaking `never_overpaid`, and is not lossless: \
         {report:?}"
    );
    let applied = l
        .cat
        .migrate_apply(&l.own, to("refuse")?, &report.report_id)
        .await;
    applied.must_fail("applying a migration that leaves a refusing constraint broken")?;
    // One that admits at ingest is listed, and applies.
    let admitting = l.cat.migrate(&l.own, to("admit")?).await;
    let admit = admitting.must("previewing an admitting constraint")?.data;
    ensure!(
        admit.lossless && admit.breaches.iter().any(|b| b.records == 2),
        "an admitting constraint is listed, and the migration is lossless: {admit:?}"
    );
    let applied = l
        .cat
        .migrate_apply(&l.own, to("admit")?, &admit.report_id)
        .await;
    applied.must("applying an admitting constraint")?;
    // To one kept from `amount`, its migration leaves the constraint over
    // it as declared, and is not told the entry but `more`.
    let mut kept = field_access(store.as_ref(), "kept", "bill", &[("amount", Access::None)]);
    kept.grants.schema = true;
    let mut theirs = invoice("bill", "authored", "admit");
    let unread = |v: &Value| v.to_string().contains("amount");
    for list in ["fields", "constraints"] {
        if let Some(l) = theirs[list].as_array_mut() {
            l.retain(|v| !unread(v));
        }
    }
    if let Some(f) = theirs["fields"].as_array_mut() {
        f.push(json!({"name": "memo", "type": {"kind": "text"}}));
    }
    let m = Migration::to(type_of(theirs)?);
    let shown = l.cat.migrate(&kept, m.clone()).await;
    let shown = shown.must("previewing as one kept from `amount`")?.data;
    ensure!(
        shown.breaches.is_empty() && shown.more && shown.lossless,
        "one kept from `amount` is not told the entry, and is told more: {shown:?}"
    );
    let applied = l.cat.migrate_apply(&kept, m, &shown.report_id).await;
    applied.must("applying as one kept from `amount`")?;
    // A masked author is not told it either: its migration that describes
    // the other constraint names only that one, and its declaration
    // leaves it standing.
    let mut masked = field_access(
        store.as_ref(),
        "masked",
        "bill",
        &[("amount", Access::Masked)],
    );
    masked.grants.schema = true;
    let mut theirs = invoice("bill", "authored", "admit");
    if let Some(cs) = theirs["constraints"].as_array_mut() {
        cs.retain(|c| c["name"] != "never_overpaid");
        cs[0]["description"] = json!("closed stays closed");
    }
    let m = Migration::to(type_of(theirs.clone())?);
    let shown = l.cat.migrate(&masked, m.clone()).await;
    let shown = shown.must("previewing as one masked from `amount`")?.data;
    let told = format!("{shown:?}");
    ensure!(
        !told.contains("never_overpaid") && told.contains("closed stays closed") && shown.more,
        "one masked from `amount` is told the constraint it reads and not the other: {shown:?}"
    );
    let applied = l.cat.migrate_apply(&masked, m, &shown.report_id).await;
    applied.must("applying as one masked from `amount`")?;
    let d: Declaration = serde_json::from_value(theirs).map_err(|e| Failure::new(e.to_string()))?;
    let declared = l.cat.declare(&masked, d).await;
    declared.must("declaring as one masked from `amount`")?;
    let held = l.describe_type("bill").await?.constraints;
    ensure!(
        held.iter()
            .any(|c| c.name.as_str() == "never_overpaid" && c.at_ingest == AtIngest::Admit),
        "the constraint its author may not read stands: {held:?}"
    );
    Ok(())
}
