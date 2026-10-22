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
use lakelands_api::error::Beyond;
use lakelands_api::ontology::AtIngest;
use lakelands_api::write::Outcome;

use crate::conformance::fixture::{Answer, ingest, reviewer, source_record};
use crate::conformance::prelude::*;

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
    CONSTRAINT_HELD_ON_EVERY_WALK = a_walk_of_the_lake_leaves_no_record_breaking_a_constraint,
    "lakelands#1119: a declaration that computes a type's records again, a value type's migration that renames what they hold, and a re-key that rewrites what other types' records hold of a moved key each judge the records as they would leave them, against each constraint that refuses, where a field it reads moved; one that would leave a record breaking it is refused `constraint_left_broken`, naming the constraint, the type and the count, appending nothing, and to an author who does not read every field it reads `not_granted`, naming nothing, or `field_read_elsewhere` on a field it computes again that the constraint reads (lakelands#1124); a migration's preflight lists in `breaches` the records as it computes them again and as its re-key rewrites their references"
);

check!(
    CONSTRAINT_WALK_GRANTED_BY_FIELDS = a_walk_is_granted_by_the_fields_it_may_move,
    "lakelands#1119: an author who does not read every field a constraint that refuses reads is not granted a walk that may move one of them, a function declared again that computes it or a value type's renames in a field that feeds it, decided before any record is read: the answer is the same whether a record would break it or not"
);

check!(
    CONSTRAINT_NAME_UNTOLD = a_constraint_named_like_one_its_author_is_not_told_is_not_granted,
    "lakelands#1119 and 027: a declaration or a migration whose constraint bears the name of one its author is not told is not granted, naming nothing, as a name a hidden field holds; under a free name it is answered as to any author"
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
    "lakelands#1119: a migration whose `to` declares a constraint lists in its preflight, per constraint, the live records it would leave breaking it, as converted, with the first keys its reader is answered; one that refuses makes the migration lossy and its apply is refused, one that admits is listed and applies; one who does not read every field a constraint reads, kept from one or masked, is not told its entry nor the constraint in `restated`, and no record is counted against it for them; a migration that may move a field such a constraint that refuses reads, converted, filled, renamed, added or computed again, is not granted to them, or refused `field_read_elsewhere` on a field it fills or computes again (lakelands#1124), decided before any record is read, so the answer is the same whatever the records hold; the constraint, which its declaration or migration leaves out, stands as declared"
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

/// The case a refusal is, if it is one with a case.
fn case_of<T>(a: &lakelands_api::Result<T>) -> Option<&Case> {
    a.as_ref().err().and_then(Error::case).map(|s| &s.case)
}

/// Whether `a` is refused `constraint_left_broken` for `constraint` of `ty`
/// with `records`.
fn left_broken<T>(a: &lakelands_api::Result<T>, (constraint, ty, n): (&str, &str, u64)) -> bool {
    matches!(case_of(a), Some(Case::ConstraintLeftBroken { constraint: c, ty: t, records })
        if c.as_str() == constraint && t.as_str() == ty && *records == n)
}

async fn a_walk_of_the_lake_leaves_no_record_breaking_a_constraint(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    // A declaration that computes `rest` again.
    let bill = |rest: &str| {
        json!({"declare": "type", "name": "bill", "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"), number("paid"),
                       {"name": "rest", "type": {"kind": "number"}, "authority": "derived",
                        "derived": rest, "depends_on": ["amount", "paid"]}],
            "constraints": [{"name": "covered", "holds": "rest >= 0 and amount > 0"}]})
    };
    l.declare_json(bill("amount - paid")).await?;
    l.upsert(
        "bill",
        "B-1",
        json!({"no": "B-1", "amount": 100, "paid": 50}),
    )
    .await?;
    let head = store.head().await.must("reading the head")?;
    let under = try_declare(&l, bill("amount - paid - 60")).await?;
    let mut masked = field_access(
        store.as_ref(),
        "masked",
        "bill",
        &[("amount", Access::Masked)],
    );
    (masked.grants.schema, masked.grants.restricted) = (true, true);
    // As it knows the type: without the constraint it is not told.
    let mut known = bill("0 - paid");
    known["constraints"] = json!([]);
    known["fields"][3]["depends_on"] = json!(["paid"]);
    let d: Declaration = serde_json::from_value(known).map_err(|e| Failure::new(e.to_string()))?;
    let untold = l.cat.declare(&masked, d).await;
    let elsewhere = matches!(case_of(&untold),
        Some(Case::FieldReadElsewhere { field, .. }) if field.as_str() == "rest");
    ensure!(
        left_broken(&under, ("covered", "bill", 1)) && elsewhere,
        "a declaration whose computing again leaves `B-1` breaking `covered` is refused \
         `constraint_left_broken` to the owner and `field_read_elsewhere` on `rest` to one \
         masked from `amount`: {under:?}, {untold:?}"
    );
    head_unmoved(store.as_ref(), head, "a walk that breaks a constraint").await?;
    try_declare(&l, bill("amount - paid - 10"))
        .await?
        .must("a declaration that leaves it held")?;
    // A migration's preflight counts the records as it computes them again.
    let mut to = bill("amount - paid - 60");
    to.as_object_mut().map(|o| o.remove("declare"));
    let to = Migration::to(type_of(to)?);
    let preview = l
        .cat
        .migrate(&l.own, to)
        .await
        .must("previewing a migration that computes again")?
        .data;
    ensure!(
        preview
            .breaches
            .iter()
            .any(|b| b.constraint.as_str() == "covered" && b.records == 1)
            && !preview.lossless,
        "a migration's preflight lists `B-1` breaking `covered` as it computes it again: {preview:?}"
    );
    // A value type's migration that renames what a record holds.
    l.declare_json(json!({"declare": "value_type", "name": "state", "kind": "text", "one_of": ["draft", "open"]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "ticket", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"),
                   {"name": "state", "type": {"kind": "text", "value_type": "state"}}],
        "constraints": [{"name": "closed_is_free", "holds": "not (state = 'closed') or amount = 0"}]}))
        .await?;
    l.upsert(
        "ticket",
        "T-1",
        json!({"no": "T-1", "amount": 5, "state": "open"}),
    )
    .await?;
    // Its twin admits at ingest: the report names what it admits.
    l.declare_json(json!({"declare": "type", "name": "ticket_a", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"),
                   {"name": "state", "type": {"kind": "text", "value_type": "state"}}],
        "constraints": [{"name": "closed_is_free", "holds": "not (state = 'closed') or amount = 0",
                         "at_ingest": "admit"}]}))
        .await?;
    l.upsert(
        "ticket_a",
        "A-1",
        json!({"no": "A-1", "amount": 5, "state": "open"}),
    )
    .await?;
    let renamed = || {
        let to = serde_json::from_value(
            json!({"name": "state", "kind": "text", "one_of": ["draft", "closed"]}),
        );
        let to: lakelands_api::ontology::ValueTypeDecl =
            to.map_err(|e| Failure::new(e.to_string()))?;
        Ok::<_, Failure>(lakelands_api::write::ValueTypeMigration {
            to,
            renames: vec![("open".into(), "closed".into())],
        })
    };
    let refused = l.cat.migrate_value_type(&l.own, renamed()?).await;
    let left = left_broken(&refused, ("closed_is_free", "ticket", 1));
    l.upsert("ticket", "T-1", json!({"amount": 0})).await?;
    let fits = l.cat.migrate_value_type(&l.own, renamed()?).await;
    let admitted = fits.as_ref().map(|r| {
        let b = r.data.breaches.iter();
        b.map(|b| (b.ty.to_string(), b.records, b.sample_keys.len()))
            .collect::<Vec<_>>()
    });
    ensure!(
        left && admitted
            .as_ref()
            .is_ok_and(|a| a == &[("ticket_a".to_owned(), 1, 1)]),
        "a value type's renames that leave `T-1` breaking `closed_is_free` are refused, and \
         once it holds them, previewed, naming what `ticket_a` admits: {refused:?}, {fits:?}"
    );
    // A re-key that rewrites another type's reference to a moved key.
    l.declare_json(json!({"declare": "type", "name": "owner", "key": ["code"],
        "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                   {"name": "alias", "type": {"kind": "text"}, "required": true}]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "pet", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}},
                   {"name": "owner", "type": {"kind": "reference", "target": "owner"}}],
        "constraints": [{"name": "not_x", "holds": "not (owner = 'X')"}]}))
        .await?;
    l.upsert("owner", "a", json!({"code": "a", "alias": "X"}))
        .await?;
    l.upsert("pet", "P-1", json!({"no": "P-1", "owner": "a"}))
        .await?;
    let rekeyed = json!({"name": "owner", "key": ["alias"],
        "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                   {"name": "alias", "type": {"kind": "text"}, "required": true}]});
    let rekey = l
        .cat
        .migrate(&l.own, Migration::to(type_of(rekeyed)?))
        .await;
    ensure!(
        left_broken(&rekey, ("not_x", "pet", 1)),
        "a re-key that rewrites `P-1`'s reference to `X` is refused `constraint_left_broken` \
         on `pet`: {rekey:?}"
    );
    // One that admits is named in the migration's report, with its type.
    l.retire("pet", "P-1").await?;
    l.declare_json(json!({"declare": "type", "name": "pet_a", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}},
                   {"name": "owner", "type": {"kind": "reference", "target": "owner"}}],
        "constraints": [{"name": "not_x", "holds": "not (owner = 'X')", "at_ingest": "admit"}]}))
        .await?;
    l.upsert("pet_a", "Q-1", json!({"no": "Q-1", "owner": "a"}))
        .await?;
    let rekeyed = json!({"name": "owner", "key": ["alias"],
        "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                   {"name": "alias", "type": {"kind": "text"}, "required": true}]});
    let admits = l
        .cat
        .migrate(&l.own, Migration::to(type_of(rekeyed)?))
        .await;
    let admits = admits
        .must("previewing the re-key over an admitting constraint")?
        .data;
    let named = admits
        .breaches
        .iter()
        .map(|b| (b.ty.to_string(), b.records))
        .collect::<Vec<_>>();
    ensure!(
        named == [("pet_a".to_owned(), 1)] && admits.lossless,
        "a re-key whose rewrite `pet_a` admits names it in the report, lossless: {admits:?}"
    );
    // And one that rewrites the migrated type's own references: listed.
    let node = |key: &str| {
        json!({"name": "node", "key": [key],
            "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                       {"name": "alias", "type": {"kind": "text"}, "required": true},
                       {"name": "boss", "type": {"kind": "reference", "target": "node"}}],
            "constraints": [{"name": "not_y", "holds": "not (boss = 'Y')"}]})
    };
    let mut declared = node("code");
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    l.upsert("node", "c", json!({"code": "c", "alias": "Y", "boss": "c"}))
        .await?;
    let own = l
        .cat
        .migrate(&l.own, Migration::to(type_of(node("alias"))?))
        .await;
    let own = own.must("previewing a re-key of `node`")?.data;
    ensure!(
        own.breaches
            .iter()
            .any(|b| b.constraint.as_str() == "not_y" && b.records == 1)
            && !own.lossless,
        "a re-key that rewrites `c`'s own reference to `Y` lists it breaking `not_y`: {own:?}"
    );
    Ok(())
}

async fn a_walk_is_granted_by_the_fields_it_may_move(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let number = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    let due = |by: &str| {
        json!({"declare": "function", "name": "due_of", "backing": "expr", "expr": format!("paid * {by}"),
               "inputs": [{"name": "paid", "type": {"kind": "number"}}]})
    };
    l.declare_json(due("1")).await?;
    l.declare_json(json!({"declare": "value_type", "name": "phase", "kind": "text", "one_of": ["open", "done"]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "loan", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}}, number("amount"), number("paid"),
                   {"name": "due", "type": {"kind": "number"}, "authority": "derived",
                    "function": "due_of", "depends_on": ["paid"]},
                   {"name": "phase", "type": {"kind": "text", "value_type": "phase"}}],
        "constraints": [{"name": "within", "holds": "due <= amount"},
                        {"name": "settled_free", "holds": "not (phase = 'settled') or amount = 0"}]}))
        .await?;
    l.upsert(
        "loan",
        "L-1",
        json!({"no": "L-1", "amount": 100, "paid": 10, "phase": "open"}),
    )
    .await?;
    let mut masked = field_access(
        store.as_ref(),
        "masked",
        "loan",
        &[("amount", Access::Masked)],
    );
    (masked.grants.schema, masked.grants.restricted) = (true, true);
    let not_granted =
        |a: &lakelands_api::Result<Outcome>| matches!(reason(a), Some(Refusal::NotGranted));
    // A function declared again: one that leaves `L-1` within, one that does not.
    let (holds, breaks) = (due("2"), due("50"));
    let mut said = Vec::new();
    for d in [holds.clone(), breaks.clone()] {
        let d: Declaration = serde_json::from_value(d).map_err(|e| Failure::new(e.to_string()))?;
        said.push(l.cat.declare(&masked, d).await);
    }
    let owner = try_declare(&l, breaks).await?;
    ensure!(
        said.iter().all(not_granted) && left_broken(&owner, ("within", "loan", 1)),
        "a function declared again is not granted to one masked from `amount`, whether \
         `L-1` would break `within` or not, and refused `constraint_left_broken` to the owner: \
         {said:?}, {owner:?}"
    );
    // A value type's renames: one that leaves `L-1` free, one that does not.
    let renamed = |to: &str| {
        let decl = json!({"name": "phase", "kind": "text", "one_of": [to, "done"]});
        let to_decl = serde_json::from_value(decl).map_err(|e| Failure::new(e.to_string()))?;
        Ok::<_, Failure>(lakelands_api::write::ValueTypeMigration {
            to: to_decl,
            renames: vec![("open".into(), to.into())],
        })
    };
    let st_1 = l.cat.migrate_value_type(&masked, renamed("pending")?).await;
    let st_2 = l.cat.migrate_value_type(&masked, renamed("settled")?).await;
    let not = |a: &lakelands_api::Result<_>| matches!(reason(a), Some(Refusal::NotGranted));
    ensure!(
        not(&st_1) && not(&st_2),
        "a value type's renames into a field `settled_free` reads are not granted to one masked \
         from `amount`, whether `L-1` would break it or not: {st_1:?}, {st_2:?}"
    );
    Ok(())
}

async fn a_constraint_named_like_one_its_author_is_not_told_is_not_granted(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(invoice("invoice", "authored", "refuse"))
        .await?;
    let mut masked = field_access(
        store.as_ref(),
        "masked",
        "invoice",
        &[("amount", Access::Masked)],
    );
    (masked.grants.schema, masked.grants.restricted) = (true, true);
    let theirs = |name: &str| {
        let mut t = invoice("invoice", "authored", "refuse");
        if let Some(cs) = t["constraints"].as_array_mut() {
            cs.retain(|c| c["name"] != "never_overpaid");
            cs.push(json!({"name": name, "holds": "paid >= 0"}));
        }
        t
    };
    let mut said = Vec::new();
    for name in ["never_overpaid", "fresh"] {
        let m = Migration::to(type_of(theirs(name))?);
        let migrated = l.cat.migrate(&masked, m).await.map(|r| r.data.lossless);
        let d: Declaration =
            serde_json::from_value(theirs(name)).map_err(|e| Failure::new(e.to_string()))?;
        let declared = l.cat.declare(&masked, d).await;
        said.push((name, format!("{migrated:?}"), declared));
    }
    let [(_, hidden_m, hidden_d), (_, free_m, free_d)] = &said[..] else {
        return Err(Failure::new("two names"));
    };
    let not_granted =
        |a: &lakelands_api::Result<Outcome>| matches!(reason(a), Some(Refusal::NotGranted));
    ensure!(
        hidden_m.contains("NotGranted")
            && not_granted(hidden_d)
            && free_m.starts_with("Ok(")
            && matches!(case_of(free_d), Some(Case::ConstraintNeedsMigrate { .. })),
        "a constraint named like one its author is not told is not granted, by `migrate` and \
         by `declare`; under a free name it is previewed, and `declare` names `migrate`: {said:?}"
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
    (kept.grants.schema, kept.grants.restricted) = (true, true);
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
        shown.breaches.is_empty() && shown.lossless && !shown.more,
        "one kept from `amount` is not told the entry, nor that there is more: {shown:?}"
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
    (masked.grants.schema, masked.grants.restricted) = (true, true);
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
    // Twins that differ only in a hidden value: filling `paid` under a
    // constraint that reads `amount` is refused on `paid` to one kept from
    // it, whether a record would break it or not.
    let hold = |name: &str, with_amount: bool| {
        let mut fields = vec![
            json!({"name": "no", "type": {"kind": "text"}}),
            json!({"name": "paid", "type": {"kind": "number"}, "initial": 60}),
        ];
        if with_amount {
            fields.insert(1, json!({"name": "amount", "type": {"kind": "number"}}));
        }
        let constraints = match with_amount {
            true => json!([{"name": "fits", "holds": "paid is null or paid <= amount"}]),
            false => json!([]),
        };
        json!({"name": name, "key": ["no"], "fields": fields, "constraints": constraints})
    };
    for (name, amount) in [("hold_a", 100), ("hold_b", 50)] {
        let mut d = hold(name, true);
        d["declare"] = json!("type");
        d["fields"][2].as_object_mut().map(|f| f.remove("initial"));
        l.declare_json(d).await?;
        l.upsert(name, "h", json!({"no": "h", "amount": amount}))
            .await?;
    }
    let mut hidden = field_access(
        store.as_ref(),
        "hidden",
        "hold_a",
        &[("amount", Access::None)],
    );
    let held_b = hidden
        .grants
        .types
        .get("hold_a")
        .cloned()
        .unwrap_or_default();
    hidden.grants.types.insert("hold_b".into(), held_b);
    hidden.grants.schema = true;
    let mut answers = Vec::new();
    for name in ["hold_a", "hold_b"] {
        let filling = |who_knows: bool| {
            type_of(hold(name, who_knows)).map(|to| Migration {
                fill: vec!["paid".into()],
                ..Migration::to(to)
            })
        };
        let theirs = l.cat.migrate(&hidden, filling(false)?).await;
        let owner = l
            .cat
            .migrate(&l.own, filling(true)?)
            .await
            .map(|r| r.data.lossless);
        answers.push((
            name,
            matches!(case_of(&theirs),
                Some(Case::FieldReadElsewhere { field, .. }) if field.as_str() == "paid"),
            owner,
        ));
    }
    ensure!(
        answers.iter().all(|a| a.1) && matches!(answers[..], [(_, _, Ok(true)), (_, _, Ok(false))]),
        "filling `paid` under `fits` is refused `field_read_elsewhere` on `paid` to one kept \
         from `amount`, `h` at 100 or at 50, and to the owner lossless at 100, lossy at 50: \
         {answers:?}"
    );
    Ok(())
}
