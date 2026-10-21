//! A change to the ontology plans before it acts
//! (`docs/decisions/062-a-change-plans-before-it-acts.md`).

use lakelands_api::event::EventBody;
use lakelands_api::ontology::ValueTypeDecl;
use lakelands_api::write::{
    Change, Declaration, Migration, Plan, ProposedWrite, Recompute, ValueTypeMigration, WriteOpts,
};
use lakelands_api::{Principal, RecordKey};
use serde_json::{Value, json};

use super::only::{de, holder};
use crate::conformance::fixture::{Lake, Must, log_events, patch, restrict};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    PLAN_TELLS = a_change_tells_its_plan_before_it_acts,
    "062 (lakelands#1141, lakelands#1140): every door that changes the ontology is planned on its own path: `plan` of a function declared again, of `recompute`, of an expression a migration changes, of a field added and of a migration that only fills tells each type whose records it would change, the live records computed again, per reference field the live records it would leave naming no live record (one absent or retired; a held one counts as live), the records an admitting constraint would break and those it would fill, each with the first keys its reader is answered, and a value type's migration the values that would no longer fit, not lossless; `enact` with the plan's id makes the change and answers that plan, with an id of another head it is refused `plan_stale`, and `declare`, `apply`, `recompute`, `add_field` and `migrate_apply` each answer the plan that applied; `migrate`'s preview is the plan's section, id included"
);

check!(
    PLAN_REFUSES = a_plan_refuses_what_its_act_refuses,
    "062, 016, 028: `plan` refuses each change its act refuses, alike to the byte: a field of a type not declared, a holder of `only`, a function declared again whose records would break a constraint that refuses, a migration of a type not declared, a recompute of a function not declared, and past the lake's `max_set_changes` a migration of an expression a set reads and a value type's rename that moves a set's members, each `too_large`; and appends nothing",
    given
);

check!(
    PLAN_SHOWN = a_plan_tells_each_reader_only_what_it_may_see,
    "062, 006: a plan tells each reader only what it may see, alike for twins: a held record of the planned type is counted and named only to a holder of `restricted`, so twins that differ by a held record, or by whether a target is held, answer one plan to one without it; a reader not shown the target type is told no count of the reference, nor a section of a type none of whose fields the change computes again it knows of, so twins that differ by a field computed by the function a reader may not know of plan alike, and a function nothing uses plans as one a hidden field uses, also where a field its reader knows of reads the clock, and a constraint over such a field counts nothing it is told; and a proposal's plan is lossy only by a record its proposer is told of, so twins whose record that would break a constraint that refuses is held in the one and retired in the other are answered and listed one plan; a stored plan of declarations is lossy to its proposer by the breaches it is told of, each judged by the bundle's own declaration: twins that differ only in a field a reviewer may not read, the change breaking a constraint that refuses in the one alone, are lossy to their proposer in the one alone, as the log keeps it, and a bundle that loosens a constraint to admit is lossless; a reviewer kept from a field and a holder of `only` are listed no stored plan, alike for twins, and a trimmed plan is no `more`; and a setting declared again with a kind its held value fits in the one twin and not the other answers its proposer one plan"
);

check!(
    PLAN_ACCEPTED = a_proposal_is_accepted_against_its_plan,
    "062, 013: a proposal is answered its plan as its proposer is shown it and refused only by the declaration's own checks, as before: one kept from a field that a constraint that refuses reads, proposing a declaration that would leave that constraint broken, is answered as for the twin without it, and the owner's accept of it is refused; a proposed change to the ontology accepted with `accept_planned` is accepted only where the plan of its change made now, as its reviewer is shown it by `plan`, is the one named: after the lake moved, the plan named before is refused `plan_stale` and the proposal stays pending, and the plan made again applies; a plain `accept` answers the plan that applied; a proposal of a write of records is refused `proposal_plans_nothing`; and a re-key's plan holds the sections of the other types it would leave naming no live record alike in its proposal's answer, its plan and its accept against that plan, and a reader kept from the reference field is told no section of that type"
);

check!(
    PLAN_TOLD = a_proposal_is_lossless_as_its_proposer_is_told,
    "062, 027, 006: a proposed migration is lossless as its proposer is told it, and so is a change's `plan` and a preview to a holder of `schema` without `restricted`, whose `enact`, `migrate_apply` and `migrate_value_type_apply` are refused `grant_needed` whatever the lake holds, as are its `declare`, `apply`, `add_field` and `recompute` and a reviewer's `accept` without `restricted`: twins whose one record a change would leave breaking a constraint that refuses, by a function declared again, by a field a migration computes again, by a re-key that rewrites another type's reference to it or by a value type's rename, or whose one link a narrowing to `many_to_one` would leave second at its end, is restricted in the one and retired or unlinked in the other are planned, previewed, proposed and listed alike to callers without `restricted`, its acts refused alike to that holder of `schema`, and the owner's accept refuses the one and applies the other; with that record live and unrestricted, each proposal is refused and the plan and preview are lossy or refused as the act"
);

/// A plan as one line: per section its type, the records computed again,
/// what its references would name, the constraints broken and the fields
/// filled, each with its first keys; a value type's misfits; then whether
/// it is lossless.
fn line(p: &Plan) -> String {
    let keys = |k: &[RecordKey]| k.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(",");
    let mut out = Vec::new();
    for t in &p.types {
        let n = t
            .recomputed
            .map_or_else(|| "all".to_owned(), |n| n.to_string());
        let mut s = format!("{} {n}", t.ty);
        for u in &t.unlive {
            s += &format!(" unlive {} {} {}", u.field, u.records, keys(&u.sample_keys));
        }
        for b in &t.breaches {
            s += &format!(
                " breaks {} {} {}",
                b.constraint,
                b.records,
                keys(&b.sample_keys)
            );
        }
        for f in &t.filled {
            s += &format!(" fills {} {} {}", f.field, f.records, keys(&f.sample_keys));
        }
        out.push(s);
    }
    out.extend((p.value_types.iter()).map(|v| format!("{} misfits {}", v.value_type, v.misfits)));
    let lossy = if p.lossless { "" } else { " lossy" };
    format!("[{}]{lossy}", out.join("; "))
}

/// What `sc`, holding `schema` without `restricted`, is answered of a
/// migration: its plan and its preview, then `enact` and the apply against
/// them.
async fn as_schema(c: &dyn lakelands_api::Catalog, sc: &Principal, change: Change) -> String {
    let planned = c.plan(sc, change.clone()).await;
    let id = (planned.as_ref()).map_or_else(|_| String::new(), |p| p.data.report_id.clone());
    let (preview, applied) = match change.clone() {
        Change::Migrate { migration } => {
            let r = c.migrate(sc, (*migration).clone()).await;
            let id = r
                .as_ref()
                .map_or_else(|_| String::new(), |r| r.data.report_id.clone());
            let r = r.map(|r| Plan {
                lossless: r.data.lossless,
                types: vec![r.data],
                ..Plan::default()
            });
            (r, c.migrate_apply(sc, *migration, &id).await)
        }
        Change::MigrateValueType { migration } => {
            let r = c.migrate_value_type(sc, (*migration).clone()).await;
            let id = r
                .as_ref()
                .map_or_else(|_| String::new(), |r| r.data.report_id.clone());
            let r = r.map(|r| Plan {
                lossless: r.data.fits,
                value_types: vec![r.data],
                ..Plan::default()
            });
            (r, c.migrate_value_type_apply(sc, *migration, &id).await)
        }
        _ => return String::new(),
    };
    let preview = preview.as_ref().map_or_else(|_| class(&preview), line);
    let enacted = c.enact(sc, change, &id).await;
    let (planned, enacted, applied) = (class(&planned), class(&enacted), class(&applied));
    format!("plans {planned}, previews {preview}, enacts {enacted}, applies {applied}")
}

/// What `who`'s listing shows of the proposal of declarations naming
/// `name`: its plan's line and `more`, `no plan`, or `none` where it is
/// not listed.
async fn listed(
    c: &dyn lakelands_api::Catalog,
    who: &Principal,
    name: &str,
) -> Result<String, Failure> {
    let listed = c.proposals(who, Default::default()).await;
    let listed = listed.must("the listing")?.data;
    let found = listed.into_iter().find_map(|p| match p.write {
        Some(ProposedWrite::Declare { declarations, plan })
            if format!("{declarations:?}").contains(&format!("\"{name}\"")) =>
        {
            Some((plan, p.more))
        }
        _ => None,
    });
    Ok(match found {
        Some((plan, more)) => {
            let plan = plan.map_or_else(|| "no plan".to_owned(), |p| line(&p));
            format!("{plan}, more {more}")
        }
        None => "none".to_owned(),
    })
}

/// What an answer is: a plan's line, an outcome's plan's, or a refusal's
/// code.
fn class<T: serde::Serialize>(a: &lakelands_api::Result<T>) -> String {
    let w = match a {
        Ok(v) => serde_json::to_value(v).unwrap_or_default(),
        Err(e) => serde_json::to_value(e).unwrap_or_default(),
    };
    let plan = w
        .get("data")
        .filter(|d| d.get("at").is_some())
        .or(w.get("plan"));
    if let Some(plan) = plan.and_then(|p| serde_json::from_value::<Plan>(p.clone()).ok()) {
        return line(&plan);
    }
    let r = &w["reason"];
    let codes = [&r["code"], &r["refusal"], &w["error"], &w["outcome"]];
    codes
        .into_iter()
        .find_map(Value::as_str)
        .unwrap_or("?")
        .to_owned()
}

/// `p{n}` (`a0`, `a1`, `a2` retired, `a3`, held where `held.0`), the
/// function `f{n}` of `s`, and `t{n}`, whose `r` names a `p{n}` as `f{n}`
/// computes it, under the admitting constraint `named` (`r` is not
/// `none`): `k1`…`k3` of `s` = `a0`…`a2`, `k5` (held where `held.1`) of
/// `a2` and `k6` of `a3`.
async fn family(l: &Lake, s: &dyn lakelands_api::LakeStore, n: &str, held: (bool, bool)) -> Held {
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let (p, f, t) = (format!("p{n}"), format!("f{n}"), format!("t{n}"));
    l.declare_json(json!({"declare": "type", "name": p, "fields": [text("team")]}))
        .await?;
    l.declare_json(function(&f, "s")).await?;
    l.declare_json(json!({"declare": "type", "name": t, "fields": [text("s"),
        {"name": "r", "type": {"kind": "reference", "target": p}, "authority": "derived",
         "function": f, "depends_on": ["s"]}],
        "constraints": [{"name": "named", "holds": "r != 'none'", "at_ingest": "admit"}]}))
        .await?;
    for k in ["a0", "a1", "a2", "a3"] {
        l.upsert(&p, k, json!({"team": "a"})).await?;
    }
    for (k, v) in [
        ("k1", "a0"),
        ("k2", "a1"),
        ("k3", "a2"),
        ("k5", "a2"),
        ("k6", "a3"),
    ] {
        l.upsert(&t, k, json!({ "s": v })).await?;
    }
    l.retire(&p, "a2").await?;
    if held.0 {
        restrict(s, &p, "a3", "a hold").await?;
    }
    if held.1 {
        restrict(s, &t, "k5", "a hold").await?;
    }
    Ok(())
}

/// The function `name`, of `s`, by `expr`.
fn function(name: &str, expr: &str) -> Value {
    json!({"declare": "function", "name": name, "backing": "expr", "expr": expr,
        "inputs": [{"name": "s", "type": {"kind": "text"}}]})
}

/// `f{n}` declared again so that `a0` computes the absent `none`.
fn again(n: &str) -> Change {
    let declarations = vec![de(function(&format!("f{n}"), "if(s = 'a0', 'none', s)"))];
    Change::Declare { declarations }
}

/// A writer who holds `schema` and not `restricted`.
fn writer(s: &dyn lakelands_api::LakeStore) -> Principal {
    holder(s, "w", json!({"default": "write", "schema": true}))
}

async fn a_change_tells_its_plan_before_it_acts(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    family(&l, s, "", (true, true)).await?;
    let w = writer(s);
    let mut said = Vec::new();
    // A function declared again: planned, made against its plan, and
    // against it again once the lake moved.
    let planned = c.plan(&l.own, again("")).await.must("planning `f`")?.data;
    said.push(format!("declare plan own: {}", line(&planned)));
    said.push(format!(
        "declare plan w: {}",
        class(&c.plan(&w, again("")).await)
    ));
    let made = c.enact(&l.own, again(""), &planned.report_id).await;
    let answered = matches!(&made, Ok(lakelands_api::Outcome::Applied { plan: Some(p), .. }) if **p == planned);
    said.push(format!("declare enact: {} {answered}", class(&made)));
    let stale = c.enact(&l.own, again(""), &planned.report_id).await;
    said.push(format!("declare enact again: {}", class(&stale)));
    // `recompute` computes the type again as it stands.
    let what = || Change::Recompute {
        what: Recompute::Function("f".into()),
    };
    said.push(format!(
        "recompute plan: {}",
        class(&c.plan(&l.own, what()).await)
    ));
    let made = c.recompute(&l.own, Recompute::Function("f".into())).await;
    said.push(format!("recompute: {}", class(&made)));
    // An expression a migration changes: the preview is the plan's
    // section, its id included.
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let u = |expr: &str| -> Migration {
        Migration::to(de(json!({"name": "u", "fields": [text("s"),
            {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
             "derived": expr, "depends_on": ["s"]}]})))
    };
    let mut declared: Value = serde_json::to_value(&u("s").to).unwrap_or_default();
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    for (k, v) in [("u1", "a0"), ("u2", "a1")] {
        l.upsert("u", k, json!({ "s": v })).await?;
    }
    let migration = || Box::new(u("if(s = 'a1', 'a2', s)"));
    let change = || Change::Migrate {
        migration: migration(),
    };
    let planned = c.plan(&l.own, change()).await.must("planning `u`")?.data;
    let preview = c
        .migrate(&l.own, *migration())
        .await
        .must("previewing `u`")?
        .data;
    said.push(format!(
        "migrate plan: {} {}",
        line(&planned),
        planned.types == [preview.clone()]
    ));
    let made = c
        .migrate_apply(&l.own, *migration(), &preview.report_id)
        .await;
    said.push(format!("migrate_apply: {}", class(&made)));
    // A field added, and a migration that only fills it.
    let v = json!({"declare": "type", "name": "v", "fields": [text("n")]});
    l.declare_json(v).await?;
    for k in ["v1", "v2"] {
        l.upsert("v", k, json!({"n": "x"})).await?;
    }
    let g = json!({"name": "g", "type": {"kind": "text"}, "initial": "'open'"});
    let add = || Change::AddField {
        ty: "v".into(),
        field: Box::new(de(g.clone())),
    };
    let planned = c.plan(&l.own, add()).await.must("planning `g`")?.data;
    said.push(format!("add_field plan: {}", line(&planned)));
    let made = c.enact(&l.own, add(), &planned.report_id).await;
    said.push(format!("add_field enact: {}", class(&made)));
    let filled = || {
        let to = de(json!({"name": "v", "fields": [text("n"), g.clone()]}));
        let migration = Box::new(Migration {
            fill: vec!["g".into()],
            ..Migration::to(to)
        });
        Change::Migrate { migration }
    };
    let planned = c
        .plan(&l.own, filled())
        .await
        .must("planning the fill")?
        .data;
    said.push(format!("fill plan: {}", line(&planned)));
    said.push(format!(
        "fill enact: {}",
        class(&c.enact(&l.own, filled(), &planned.report_id).await)
    ));
    // A value type tightened: what would no longer fit, and the act
    // refused as its plan says.
    let tone = |one_of: Value| {
        json!({"declare": "value_type", "name": "tone",
        "kind": "text", "one_of": one_of})
    };
    l.declare_json(tone(json!(["a", "b"]))).await?;
    let z = json!({"declare": "type", "name": "z",
        "fields": [{"name": "c", "type": {"kind": "text", "value_type": "tone"}}]});
    l.declare_json(z).await?;
    for (k, v) in [("z1", "a"), ("z2", "b")] {
        l.upsert("z", k, json!({ "c": v })).await?;
    }
    let tightened = || {
        let to = de(json!({"name": "tone", "kind": "text", "one_of": ["a"]}));
        let m = ValueTypeMigration {
            to,
            renames: Vec::new(),
        };
        Change::MigrateValueType {
            migration: Box::new(m),
        }
    };
    let planned = c
        .plan(&l.own, tightened())
        .await
        .must("planning `tone`")?
        .data;
    said.push(format!("value type plan: {}", line(&planned)));
    said.push(format!(
        "value type enact: {}",
        class(&c.enact(&l.own, tightened(), &planned.report_id).await)
    ));
    // A re-key rewrites the references to the records it moves: those of
    // another type that would name no live record are that type's section.
    let ac = |key: &str| {
        json!({"name": "ac", "key": [key], "fields": [
            {"name": "no", "type": {"kind": "text"}, "required": true},
            {"name": "code", "type": {"kind": "text"}, "required": true}]})
    };
    let mut declared = ac("no");
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    let pay = json!({"declare": "type", "name": "pay",
        "fields": [{"name": "ac", "type": {"kind": "reference", "target": "ac"}}]});
    l.declare_json(pay).await?;
    for (k, code) in [("1", "C1"), ("2", "C2")] {
        l.upsert("ac", k, json!({"no": k, "code": code})).await?;
        l.upsert("pay", &format!("pay{k}"), json!({ "ac": k }))
            .await?;
    }
    l.retire("ac", "2").await?;
    // A type nothing is judged of: every record computed again, uncounted.
    l.declare_json(function("gx", "s")).await?;
    let tx = json!({"declare": "type", "name": "tx", "fields": [
        {"name": "s", "type": {"kind": "text"}},
        {"name": "d", "type": {"kind": "text"}, "authority": "derived", "function": "gx",
         "depends_on": ["s"]}]});
    l.declare_json(tx).await?;
    l.upsert("tx", "x1", json!({"s": "a"})).await?;
    let gx = Change::Declare {
        declarations: vec![de(function("gx", "if(s = 'a', 'b', s)"))],
    };
    said.push(format!(
        "nothing judged: {}",
        class(&c.plan(&l.own, gx).await)
    ));
    let rekey = Change::Migrate {
        migration: Box::new(Migration::to(de(ac("code")))),
    };
    said.push(format!(
        "rekey plan: {}",
        class(&c.plan(&l.own, rekey).await)
    ));
    let expected = [
        "declare plan own: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1]",
        "declare plan w: [t 4 unlive r 2 k1,k3 breaks named 1 k1]",
        "declare enact: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1] true",
        "declare enact again: plan_stale",
        "recompute plan: [t 5 unlive r 3 k1,k3,k5]",
        "recompute: [t 5 unlive r 3 k1,k3,k5]",
        "migrate plan: [u 2 unlive r 1 u2] true",
        "migrate_apply: [u 2 unlive r 1 u2]",
        "add_field plan: []",
        "add_field enact: []",
        "fill plan: [v 0 fills g 2 v1,v2]",
        "fill enact: [v 0 fills g 2 v1,v2]",
        "value type plan: [tone misfits 1] lossy",
        "value type enact: value_type_not_tightened",
        "nothing judged: [tx all]",
        "rekey plan: [ac 0; pay 0 unlive ac 1 pay2]",
    ];
    ensure!(said == expected, "the plans were otherwise: {said:#?}");
    Ok(())
}

async fn a_plan_refuses_what_its_act_refuses(store: Store, given: Given) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    family(&l, s, "", (false, false)).await?;
    // `tc`, whose `r` is computed by `fc` under a constraint that refuses.
    let ty = json!({"declare": "type", "name": "tc", "fields": [
        {"name": "s", "type": {"kind": "text"}},
        {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
         "function": "fc", "depends_on": ["s"]}],
        "constraints": [{"name": "named", "holds": "r != 'none'"}]});
    l.declare_json(function("fc", "s")).await?;
    l.declare_json(ty).await?;
    l.upsert("tc", "c1", json!({"s": "a0"})).await?;
    let o = holder(
        s,
        "o",
        json!({"default": "write", "schema": true,
        "types": {"t": {"only": {"s": "a0"}}}}),
    );
    let unknown = json!({"declare": "type", "name": "x", "fields": [
        {"name": "r", "type": {"kind": "reference", "target": "nope"}}]});
    let declare = |d: Value| Change::Declare {
        declarations: vec![de(d)],
    };
    let undeclared = || Change::Migrate {
        migration: Box::new(Migration::to(de(json!({"name": "nope", "fields": []})))),
    };
    let recompute = || Change::Recompute {
        what: Recompute::Function("nothing".into()),
    };
    let cases: [(&str, &Principal, Change); 5] = [
        (
            "a reference to a type not declared",
            &l.own,
            declare(unknown),
        ),
        ("a holder of `only`", &o, again("")),
        (
            "a constraint left broken",
            &l.own,
            declare(function("fc", "if(s = 'a0', 'none', s)")),
        ),
        ("a migration of a type not declared", &l.own, undeclared()),
        (
            "a recompute of a function not declared",
            &l.own,
            recompute(),
        ),
    ];
    alike(c, s, cases).await?;
    // Past the lake's `max_set_changes`: a set over an expression a
    // migration changes, and one over the values a rename moves.
    let num = json!({"kind": "number"});
    l.declare_json(json!({"declare": "type", "name": "bill", "fields": [
        {"name": "amount", "type": num},
        {"name": "open", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "amount > 0", "depends_on": ["amount"]}]}))
        .await?;
    let set = |name: &str, of: &str, filter: &str| json!({"declare": "object_set", "name": name, "of": of, "where": filter});
    l.declare_json(set("opened", "bill", "open = true")).await?;
    let tone = |one_of: Value| json!({"name": "tone", "kind": "text", "one_of": one_of});
    let mut declared = tone(json!(["a", "b"]));
    declared["declare"] = json!("value_type");
    l.declare_json(declared).await?;
    let z = json!({"declare": "type", "name": "z",
        "fields": [{"name": "c", "type": {"kind": "text", "value_type": "tone"}}]});
    l.declare_json(z).await?;
    l.declare_json(set("ays", "z", "c = 'a'")).await?;
    for (k, n) in [("b1", 1), ("b2", 2), ("b3", 3)] {
        l.upsert("bill", k, json!({ "amount": n })).await?;
    }
    for k in ["z1", "z2"] {
        l.upsert("z", k, json!({"c": "a"})).await?;
    }
    let lower = lakelands_api::location::Limits {
        max_set_changes: 1,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, lakelands_api::seam::Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening at a ceiling of one: {e}")))?;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let to = de(
        json!({"name": "bill", "fields": [{"name": "amount", "type": num},
        {"name": "open", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "amount > 5", "depends_on": ["amount"]}]}),
    );
    let renamed = ValueTypeMigration {
        to: de(tone(json!(["b", "c"]))),
        renames: vec![("a".into(), "c".into())],
    };
    // Their acts are the doors that make them against a fresh preview.
    let c = l.cat.as_ref();
    let head = s.head().await.must("the head")?;
    let wire = |e: &lakelands_api::Error| serde_json::to_string(e).unwrap_or_default();
    let m = Migration::to(to);
    let planned = c
        .plan(
            &l.own,
            Change::Migrate {
                migration: Box::new(m.clone()),
            },
        )
        .await;
    let id = c
        .migrate(&l.own, m.clone())
        .await
        .must("the preview")?
        .data
        .report_id;
    let made = c.migrate_apply(&l.own, m, &id).await;
    let id = c
        .migrate_value_type(&l.own, renamed.clone())
        .await
        .must("the preview")?
        .data
        .report_id;
    let change = Change::MigrateValueType {
        migration: Box::new(renamed.clone()),
    };
    let pairs = [
        (
            "a migration past the ceiling",
            planned.map(drop),
            made.map(drop),
        ),
        (
            "a rename past the ceiling",
            c.plan(&l.own, change).await.map(drop),
            c.migrate_value_type_apply(&l.own, renamed, &id)
                .await
                .map(drop),
        ),
    ];
    for (what, planned, made) in pairs {
        match (&planned, &made) {
            (Err(a), Err(b)) if wire(a) == wire(b) && wire(a).contains("too_large") => {}
            _ => {
                return Err(Failure::new(format!(
                    "{what}: planned {planned:?}, made {made:?}"
                )));
            }
        }
    }
    ensure!(
        s.head().await.must("the head")? == head,
        "a change past the ceiling appended"
    );
    Ok(())
}

/// Each change, refused alike by its plan and its act, which append
/// nothing.
async fn alike<const N: usize>(
    c: &dyn lakelands_api::Catalog,
    s: &dyn lakelands_api::LakeStore,
    cases: [(&str, &Principal, Change); N],
) -> Held {
    let head = s.head().await.must("the head")?;
    let wire = |e: &lakelands_api::Error| serde_json::to_string(e).unwrap_or_default();
    for (what, who, change) in cases {
        let planned = c.plan(who, change.clone()).await;
        let made = c.enact(who, change, "any").await;
        ensure!(
            s.head().await.must("the head")? == head,
            "{what}: planning and enacting it appended"
        );
        match (&planned, &made) {
            (Err(a), Err(b)) if wire(a) == wire(b) => {}
            _ => {
                let said = format!("{what}: planned {planned:?}, made {made:?}");
                return Err(Failure::new(said));
            }
        }
    }
    Ok(())
}

async fn a_plan_tells_each_reader_only_what_it_may_see(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    // Twins: `a3` and `k5` held in the one, and not in the other, where
    // `k5` is not written at all.
    family(&l, s, "", (true, true)).await?;
    family(&l, s, "2", (false, false)).await?;
    l.retire("t2", "k5").await?;
    // And twins that differ only in whether a target, hidden from `x`, is
    // live: `t{n}`'s one record names `zz`, retired in the second. `x`
    // knows of `g`, which `f{n}` does not compute, and not of `r`, which
    // it does: it is told no section.
    for n in ["3", "4"] {
        let (p, f, t) = (format!("p{n}"), format!("f{n}"), format!("t{n}"));
        l.declare_json(json!({"declare": "type", "name": p, "fields": [{"name": "team", "type": {"kind": "text"}}]}))
            .await?;
        l.declare_json(function(&f, "s")).await?;
        l.declare_json(json!({"declare": "type", "name": t, "fields": [
            {"name": "s", "type": {"kind": "text"}},
            {"name": "g", "type": {"kind": "text"}, "authority": "derived", "derived": "s",
             "depends_on": ["s"]},
            {"name": "r", "type": {"kind": "reference", "target": p}, "authority": "derived",
             "function": f, "depends_on": ["s"]}]}))
            .await?;
        l.upsert(&p, "zz", json!({"team": "a"})).await?;
        l.upsert(&t, "k7", json!({"s": "zz"})).await?;
    }
    l.retire("p4", "zz").await?;
    let moved = |n: &str| {
        let declarations = vec![de(function(&format!("f{n}"), "if(s = 'qq', 'x', s)"))];
        Change::Declare { declarations }
    };
    let w = writer(s);
    let blind = holder(
        s,
        "x",
        json!({"default": "write", "schema": true,
        "types": {"p": {"default": "none"}, "p2": {"default": "none"},
                  "p3": {"default": "none"}, "p4": {"default": "none"}}}),
    );
    let mut said = Vec::new();
    let pairs = [
        (&w, "w", ["", "2"]),
        (&blind, "x", ["", "2"]),
        (&blind, "x", ["3", "4"]),
    ];
    for (who, name, twins) in pairs {
        for n in twins {
            let change = if ["3", "4"].contains(&n) {
                moved(n)
            } else {
                again(n)
            };
            let planned = c.plan(who, change).await;
            let plan = class(&planned).replace(&format!("t{n} "), "t ");
            said.push(format!("{name} f{n}: {plan}"));
        }
    }
    // Twins `t5`, whose hidden `r` `fa` computes, and `t6`, whose `fc`
    // does, beside a `g` `x` knows of: `x` plans `fa` and `fb`, which
    // nothing uses, again and is told alike, so no plan says which hidden
    // field a function computes.
    for f in ["fa", "fb", "fc"] {
        l.declare_json(function(f, "s")).await?;
    }
    l.upsert("p", "zz", json!({"team": "a"})).await?;
    for (t, f) in [("t5", "fa"), ("t6", "fc")] {
        l.declare_json(json!({"declare": "type", "name": t, "fields": [
            {"name": "s", "type": {"kind": "text"}},
            {"name": "g", "type": {"kind": "text"}, "authority": "derived", "derived": "s",
             "depends_on": ["s"]},
            {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
             "function": f, "depends_on": ["s"]}]}))
            .await?;
        l.upsert(t, "k1", json!({"s": "zz"})).await?;
    }
    // `t7`'s `late` reads the clock, which an act computes again where it
    // moves: `fd` is told alike, a field the clock moves telling nothing
    // of which hidden field the function computes.
    l.declare_json(function("fd", "s")).await?;
    l.declare_json(json!({"declare": "type", "name": "t7", "fields": [
        {"name": "s", "type": {"kind": "text"}},
        {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "now() > '2000-01-01T00:00:00Z'", "computed_at": "write"},
        {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
         "function": "fd", "depends_on": ["s"]}]}))
        .await?;
    l.upsert("t7", "k1", json!({"s": "zz"})).await?;
    for f in ["fa", "fb", "fd"] {
        let declarations = vec![de(function(f, "if(s = 'qq', 'x', s)"))];
        let planned = c.plan(&blind, Change::Declare { declarations }).await;
        said.push(format!("x plans {f}: {}", class(&planned)));
    }
    // A migration the owner proposes keeps its report as the owner is
    // shown it without `restricted`: twins whose `m9` is held in the one
    // and retired in the other are listed alike to a reviewer without it.
    let rv = holder(
        s,
        "rv",
        json!({"default": "read", "review": true, "schema": true}),
    );
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}});
    let m = |n: &str, expr: &str, more: Value| -> Value {
        let mut fields = vec![
            text("s"),
            json!({"name": "r",
            "type": {"kind": "reference", "target": "p"}, "authority": "derived",
            "derived": expr, "depends_on": ["s"]}),
        ];
        fields.extend(more.as_array().cloned().unwrap_or_default());
        json!({"name": format!("m{n}"), "fields": fields})
    };
    for n in ["", "2"] {
        let mut declared = m(n, "s", json!([]));
        declared["declare"] = json!("type");
        l.declare_json(declared).await?;
        let ty = format!("m{n}");
        l.upsert(&ty, "m1", json!({"s": "a0"})).await?;
        l.upsert(&ty, "m9", json!({"s": "a1"})).await?;
        match n {
            "" => restrict(s, &ty, "m9", "a hold").await.map(drop)?,
            _ => l.retire(&ty, "m9").await.map(drop)?,
        }
        let g = json!([{"name": "g", "type": {"kind": "text"}, "initial": "'open'"}]);
        let to = de(m(n, "if(s = 'a1', 'none', s)", g));
        let migration = Box::new(Migration {
            fill: vec!["g".into()],
            ..Migration::to(to)
        });
        let write = lakelands_api::write::ProposedWrite::Migrate {
            migration,
            report: Default::default(),
        };
        c.propose(&l.own, write, "why".into())
            .await
            .must("proposing the migration")?;
        let listed = c
            .proposals(&rv, Default::default())
            .await
            .must("the listing")?
            .data;
        let report = listed.into_iter().find_map(|p| match p.write {
            Some(lakelands_api::write::ProposedWrite::Migrate { migration, report })
                if migration.to.name.as_str() == ty =>
            {
                Some(*report)
            }
            _ => None,
        });
        let report =
            report.ok_or_else(|| Failure::new(format!("`{ty}`'s proposal is not listed")))?;
        let plan = Plan {
            types: vec![report],
            lossless: true,
            ..Plan::default()
        };
        said.push(format!(
            "rv stored m{n}: {}",
            line(&plan).replace("m2 ", "m ")
        ));
    }
    // Twins whose one record that would break a constraint that refuses
    // is held in the one and retired in the other: a proposer without
    // `restricted` is answered one plan, and `rv` lists one (062).
    let pr = holder(s, "pr", json!({"default": "write"}));
    for n in ["9", "10"] {
        let (p, f, t) = (format!("p{n}"), format!("f{n}"), format!("t{n}"));
        l.declare_json(json!({"declare": "type", "name": p, "fields": [text("team")]}))
            .await?;
        l.declare_json(function(&f, "s")).await?;
        l.declare_json(json!({"declare": "type", "name": t, "fields": [text("s"),
            {"name": "r", "type": {"kind": "reference", "target": p}, "authority": "derived",
             "function": f, "depends_on": ["s"]}],
            "constraints": [{"name": "named", "holds": "r != 'none'"}]}))
            .await?;
        for k in ["a0", "a1"] {
            l.upsert(&p, k, json!({"team": "a"})).await?;
        }
        l.upsert(&t, "g1", json!({"s": "a1"})).await?;
        l.upsert(&t, "g2", json!({"s": "a0"})).await?;
        match n {
            "9" => restrict(s, &t, "g2", "a hold").await.map(drop)?,
            _ => l.retire(&t, "g2").await.map(drop)?,
        }
        let write = lakelands_api::write::ProposedWrite::Declare {
            declarations: vec![de(function(&f, "if(s = 'a0', 'none', s)"))],
            plan: None,
        };
        let proposed = c.propose(&pr, write, "why".into()).await;
        let named = |a: String| a.replace(&format!("t{n} "), "t ");
        said.push(format!("pr proposes f{n}: {}", named(class(&proposed))));
        let listed = c.proposals(&rv, Default::default()).await;
        let listed = listed.must("the listing")?.data;
        let plan = listed.into_iter().find_map(|p| match p.write {
            Some(lakelands_api::write::ProposedWrite::Declare { declarations, plan })
                if format!("{declarations:?}").contains(&format!("\"{f}\"")) =>
            {
                plan
            }
            _ => None,
        });
        let plan = plan.map_or_else(|| "none".to_owned(), |p| line(&p));
        said.push(format!("rv lists f{n}: {}", named(plan)));
    }
    // Twins that differ only in `g2`'s `s`, which `rk` may not read: the
    // change breaks `named` on `g2` in the one alone. `pr` is told so, and
    // its plan is lossy; `rk` and `ro` are listed no plan of either.
    let hidden = json!({"fields": {"s": "none"}});
    let rk = holder(
        s,
        "rk",
        json!({"default": "read", "review": true, "schema": true,
        "types": {"t11": hidden, "t12": hidden, "t13": hidden, "t14": hidden}}),
    );
    let ro = holder(
        s,
        "ro",
        json!({"default": "read", "review": true, "schema": true,
        "types": {"t11": {"only": {"s": "a1"}}, "t12": {"only": {"s": "a1"}}}}),
    );
    for (n, g2) in [("11", "a0"), ("12", "a1")] {
        let (p, f, t) = (format!("p{n}"), format!("f{n}"), format!("t{n}"));
        l.declare_json(json!({"declare": "type", "name": p, "fields": [text("team")]}))
            .await?;
        l.declare_json(function(&f, "s")).await?;
        l.declare_json(json!({"declare": "type", "name": t, "fields": [text("s"),
            {"name": "r", "type": {"kind": "reference", "target": p}, "authority": "derived",
             "function": f, "depends_on": ["s"]}],
            "constraints": [{"name": "named", "holds": "r != 'none'"}]}))
            .await?;
        for k in ["a0", "a1"] {
            l.upsert(&p, k, json!({"team": "a"})).await?;
        }
        l.upsert(&t, "g1", json!({"s": "a1"})).await?;
        l.upsert(&t, "g2", json!({ "s": g2 })).await?;
        let write = lakelands_api::write::ProposedWrite::Declare {
            declarations: vec![de(function(&f, "if(s = 'a0', 'none', s)"))],
            plan: None,
        };
        let proposed = c.propose(&pr, write, "why".into()).await;
        let named = |a: String| a.replace(&format!("t{n} "), "t ");
        said.push(format!("pr proposes f{n}: {}", named(class(&proposed))));
        // Neither a reader kept from a field nor a holder of `only` is
        // listed the plan, and `more` is not the plan's.
        for (name, who) in [("rk", &rk), ("ro", &ro)] {
            let plan = listed(c, who, &f).await?;
            said.push(format!("{name} lists f{n}: {}", named(plan)));
        }
    }
    // A bundle that loosens `named` to admit breaks it losing nothing: its
    // plan is judged by the bundle's declaration, not the one in force.
    let t11 = json!({"declare": "type", "name": "t11", "fields": [text("s"),
        {"name": "r", "type": {"kind": "reference", "target": "p11"}, "authority": "derived",
         "function": "f11", "depends_on": ["s"]}],
        "constraints": [{"name": "named", "holds": "r != 'none'", "at_ingest": "admit"}]});
    let write = lakelands_api::write::ProposedWrite::Declare {
        declarations: vec![de(function("f11", "if(s = 'a0', 'none', s)")), de(t11)],
        plan: None,
    };
    let proposed = c.propose(&pr, write, "why".into()).await;
    let loosened = class(&proposed).replace("t11 ", "t ");
    said.push(format!("pr proposes f11, loosened: {loosened}"));
    // Twins `t13` and `t14` whose bundle loosens `qok` to admit, and breaks
    // `named`, which `rk` may not read, in `t13` alone: lossy to `pr` by
    // `named` alone, judged by the bundle's `qok`, and listed to `rk` alike
    // with no plan.
    let typed = |n: &str, (r, q): (&str, &str), qok: &str| {
        json!({"declare": "type", "name": format!("t{n}"), "fields": [text("s"), text("q"),
            {"name": "r", "type": {"kind": "text"}, "authority": "derived", "derived": r,
             "depends_on": ["s"]},
            {"name": "r2", "type": {"kind": "text"}, "authority": "derived", "derived": q,
             "depends_on": ["q"]}],
            "constraints": [{"name": "named", "holds": "r != 'none'"},
                {"name": "qok", "holds": "r2 != 'none'", "at_ingest": qok}]})
    };
    let broken = ("if(s = 'a0', 'none', s)", "if(q = 'b', 'none', q)");
    for (n, g1) in [("13", "a0"), ("14", "a1")] {
        l.declare_json(typed(n, ("s", "q"), "refuse")).await?;
        let t = format!("t{n}");
        l.upsert(&t, "g1", json!({"s": g1, "q": "b"})).await?;
        let write = lakelands_api::write::ProposedWrite::Declare {
            declarations: vec![de(typed(n, broken, "admit"))],
            plan: None,
        };
        let proposed = c.propose(&pr, write, "why".into()).await;
        let named = |a: String| a.replace(&format!("{t} "), "t ");
        said.push(format!("pr proposes {t}: {}", named(class(&proposed))));
        let plan = listed(c, &rk, &t).await?;
        said.push(format!("rk lists {t}: {}", named(plan)));
    }
    // Twins `tk1`, whose one field `fk1` computes is `h`, which `kh` may
    // not know of, and `tk2`, which computes nothing by `fk2`: `kh` plans
    // either function again and is told no section of either.
    let kh = holder(
        s,
        "kh",
        json!({"default": "write", "schema": true,
        "types": {"tk1": {"fields": {"h": "none"}}, "tk3": {"fields": {"h": "none"}},
                  "tk4": {"fields": {"h": "none"}}}}),
    );
    for n in ["1", "2"] {
        let (f, t) = (format!("fk{n}"), format!("tk{n}"));
        l.declare_json(function(&f, "s")).await?;
        let mut fields = vec![text("s")];
        if n == "1" {
            fields.push(
                json!({"name": "h", "type": {"kind": "text"}, "authority": "derived",
                "function": f, "depends_on": ["s"]}),
            );
        }
        l.declare_json(json!({"declare": "type", "name": t, "fields": fields}))
            .await?;
        l.upsert(&t, "k1", json!({"s": "a0"})).await?;
        let declarations = vec![de(function(&f, "if(s = 'a0', 'none', s)"))];
        let planned = c.plan(&kh, Change::Declare { declarations }).await;
        said.push(format!("kh plans {f}: {}", class(&planned)));
    }
    // Twins `tk3`, with a constraint that admits over `h`, and `tk4`
    // without it: `fk{n}` computes `g`, which `kh` reads, and `h`; what
    // is judged of `h` counts nothing it is told.
    for n in ["3", "4"] {
        let (f, t) = (format!("fk{n}"), format!("tk{n}"));
        l.declare_json(function(&f, "s")).await?;
        let by = |name: &str| {
            json!({"name": name, "type": {"kind": "text"}, "authority": "derived",
                "function": f, "depends_on": ["s"]})
        };
        let mut d = json!({"declare": "type", "name": t, "fields": [text("s"), by("g"), by("h")]});
        if n == "3" {
            d["constraints"] =
                json!([{"name": "hc", "holds": "h != 'none'", "at_ingest": "admit"}]);
        }
        l.declare_json(d).await?;
        l.upsert(&t, "k1", json!({"s": "a0"})).await?;
        let declarations = vec![de(function(&f, "if(s = 'a0', 'none', s)"))];
        let planned = c.plan(&kh, Change::Declare { declarations }).await;
        said.push(format!(
            "kh plans {f}: {}",
            class(&planned).replace(&t, "tk")
        ));
    }
    // A setting declared again with another kind: its held value fits the
    // one twin and not the other, which the accept judges and the plan
    // does not tell.
    for (n, value) in [("1", json!("low")), ("2", json!(7))] {
        l.set(&format!("lv{n}"), value).await?;
        let d = json!({"declare": "setting", "key": format!("lv{n}"), "type": {"kind": "number"}});
        let write = lakelands_api::write::ProposedWrite::Declare {
            declarations: vec![de(d)],
            plan: None,
        };
        let proposed = c.propose(&pr, write, "why".into()).await;
        said.push(format!("pr proposes lv{n}: {}", class(&proposed)));
    }
    // The log holds each plan as its proposer was told it: lossy where it
    // was told of a breach of a constraint that refuses.
    for e in log_events(s).await? {
        if let EventBody::WriteProposed {
            write: ProposedWrite::Declare { declarations, plan },
            ..
        } = e.body
        {
            for n in ["11", "12"] {
                if format!("{declarations:?}").contains(&format!("\"f{n}\"")) {
                    let lossless = plan.as_ref().map(|p| p.lossless);
                    let what = if declarations.len() > 1 {
                        ", loosened"
                    } else {
                        ""
                    };
                    said.push(format!("logged f{n}{what}: lossless {lossless:?}"));
                }
            }
        }
    }
    let expected = [
        "w f: [t 4 unlive r 2 k1,k3 breaks named 1 k1]",
        "w f2: [t 4 unlive r 2 k1,k3 breaks named 1 k1]",
        "x f: []",
        "x f2: []",
        "x f3: []",
        "x f4: []",
        "x plans fa: []",
        "x plans fb: []",
        "x plans fd: []",
        "rv stored m: [m 1 fills g 1 m1]",
        "rv stored m2: [m 1 fills g 1 m1]",
        "pr proposes f9: [t 1]",
        "rv lists f9: [t 1]",
        "pr proposes f10: [t 1]",
        "rv lists f10: [t 1]",
        "pr proposes f11: [t 2 unlive r 1 g2 breaks named 1 g2] lossy",
        "rk lists f11: no plan, more false",
        "ro lists f11: no plan, more false",
        "pr proposes f12: [t 2]",
        "rk lists f12: no plan, more false",
        "ro lists f12: no plan, more false",
        "pr proposes f11, loosened: [t 2 unlive r 1 g2 breaks named 1 g2]",
        "pr proposes t13: [t 1 breaks named 1 g1 breaks qok 1 g1] lossy",
        "rk lists t13: no plan, more true",
        "pr proposes t14: [t 1 breaks qok 1 g1]",
        "rk lists t14: no plan, more true",
        "kh plans fk1: []",
        "kh plans fk2: []",
        "kh plans fk3: [tk all]",
        "kh plans fk4: [tk all]",
        "pr proposes lv1: []",
        "pr proposes lv2: []",
        "logged f11: lossless Some(false)",
        "logged f12: lossless Some(true)",
        "logged f11, loosened: lossless Some(true)",
    ];
    ensure!(said == expected, "the plans were otherwise: {said:#?}");
    Ok(())
}

async fn a_proposal_is_accepted_against_its_plan(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    family(&l, s, "", (false, false)).await?;
    let agent = holder(s, "agent", json!({"default": "write"}));
    let proposing = lakelands_api::write::ProposedWrite::Declare {
        declarations: vec![de(function("f", "if(s = 'a0', 'none', s)"))],
        plan: None,
    };
    let proposed = c.propose(&agent, proposing.clone(), "why".into()).await;
    let mut said = vec![format!("proposed: {}", class(&proposed))];
    let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("proposing `f`: {proposed:?}")));
    };
    let change = proposing.change().expect("a declaration is a change");
    // The plan the proposal keeps, as its listing shows it to `who`.
    let stored = |who: Principal| {
        let proposal = proposal.clone();
        async move {
            let listed = c
                .proposals(&who, Default::default())
                .await
                .must("the listing")?
                .data;
            let p = listed.into_iter().find(|p| p.id == proposal);
            let plan = match p.and_then(|p| p.write) {
                Some(lakelands_api::write::ProposedWrite::Declare { plan, .. }) => plan,
                _ => None,
            };
            Ok::<_, Failure>(plan.map_or_else(|| "none".to_owned(), |p| line(&p)))
        }
    };
    // The reviewer reads the plan, the lake moves, and the plan read
    // before no longer accepts it.
    let before = c
        .plan(&l.own, change.clone())
        .await
        .must("planning the proposal")?
        .data;
    said.push(format!("planned: {}", line(&before)));
    l.upsert("t", "k7", json!({"s": "a0"})).await?;
    said.push(format!(
        "stored, to the owner: {}",
        stored(l.own.clone()).await?
    ));
    let rv = holder(
        s,
        "rv",
        json!({"default": "read", "review": true, "schema": true}),
    );
    said.push(format!("stored, to rv: {}", stored(rv).await?));
    let stale = c.accept_planned(&l.own, &proposal, &before.report_id).await;
    said.push(format!("accepted against it: {}", class(&stale)));
    let now = c.plan(&l.own, change).await.must("planning it again")?.data;
    said.push(format!("planned again: {}", line(&now)));
    let accepted = c.accept_planned(&l.own, &proposal, &now.report_id).await;
    said.push(format!("accepted against that: {}", class(&accepted)));
    // A write of records has no plan; a plain accept answers the one that
    // applied.
    let upsert = lakelands_api::write::ProposedWrite::Upsert {
        ty: "t".into(),
        key: "k8".into(),
        patch: de(json!({"s": "a1"})),
        opts: Default::default(),
    };
    let proposed = c.propose(&agent, upsert, "why".into()).await;
    let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("proposing `k8`: {proposed:?}")));
    };
    let nothing = c.accept_planned(&l.own, &proposal, "any").await;
    said.push(format!(
        "a write accepted against a plan: {}",
        class(&nothing)
    ));
    let back = lakelands_api::write::ProposedWrite::Declare {
        declarations: vec![de(function("f", "s"))],
        plan: None,
    };
    let proposed = c.propose(&agent, back, "why".into()).await;
    let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("proposing `f` back: {proposed:?}")));
    };
    said.push(format!(
        "accepted: {}",
        class(&c.accept(&l.own, &proposal).await)
    ));
    // Twins `h` and `h2`, the first under a constraint that refuses and
    // reads `secret`, which `kp` is kept from (062).
    let kp = holder(
        s,
        "kp",
        json!({"default": "write",
        "types": {"h": {"fields": {"secret": "none"}}, "h2": {"fields": {"secret": "none"}}}}),
    );
    let mut twins = Vec::new();
    for (n, constraints) in [
        (
            "",
            json!([{"name": "hidden_rule",
        "holds": "secret != 'x' or r != 'none'"}]),
        ),
        ("2", json!([])),
    ] {
        let (fh, h) = (format!("fh{n}"), format!("h{n}"));
        l.declare_json(function(&fh, "s")).await?;
        l.declare_json(json!({"declare": "type", "name": h, "fields": [
            {"name": "s", "type": {"kind": "text"}}, {"name": "secret", "type": {"kind": "text"}},
            {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
             "function": fh, "depends_on": ["s"]}], "constraints": constraints}))
            .await?;
        l.upsert(&h, "h1", json!({"s": "a0", "secret": "x"}))
            .await?;
        let write = lakelands_api::write::ProposedWrite::Declare {
            declarations: vec![de(function(&fh, "if(s = 'a0', 'none', s)"))],
            plan: None,
        };
        let proposed = c.propose(&kp, write, "why".into()).await;
        said.push(format!(
            "kp proposes fh{n}: {}",
            class(&proposed).replace("h2 ", "h ")
        ));
        twins.push(proposed);
    }
    for (n, proposed) in ["", "2"].into_iter().zip(twins) {
        let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
            return Err(Failure::new(format!("proposing `fh{n}`: {proposed:?}")));
        };
        said.push(format!(
            "accepted fh{n}: {}",
            class(&c.accept(&l.own, &proposal).await).replace("h2 ", "h ")
        ));
    }
    // A re-key's plan holds the sections of the other types it would leave
    // naming no live record: proposed, planned and accepted against alike.
    let ac = |key: &str| {
        json!({"name": "ac", "key": [key], "fields": [
            {"name": "no", "type": {"kind": "text"}, "required": true},
            {"name": "code", "type": {"kind": "text"}, "required": true}]})
    };
    let mut declared = ac("no");
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    let pay = json!({"declare": "type", "name": "pay",
        "fields": [{"name": "ac", "type": {"kind": "reference", "target": "ac"}}]});
    l.declare_json(pay).await?;
    for (k, code) in [("1", "C1"), ("2", "C2")] {
        l.upsert("ac", k, json!({"no": k, "code": code})).await?;
        l.upsert("pay", &format!("pay{k}"), json!({ "ac": k }))
            .await?;
    }
    l.retire("ac", "2").await?;
    let rekey = lakelands_api::write::ProposedWrite::Migrate {
        migration: Box::new(Migration::to(de(ac("code")))),
        report: Default::default(),
    };
    let proposed = c.propose(&agent, rekey.clone(), "why".into()).await;
    said.push(format!("rekey proposed: {}", class(&proposed)));
    let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("proposing the re-key: {proposed:?}")));
    };
    let change = rekey.change().expect("a migration is a change");
    // A reader kept from `pay.ac` is told nothing of `pay`.
    let kx = holder(
        s,
        "kx",
        json!({"default": "write", "schema": true,
        "types": {"pay": {"fields": {"ac": "none"}}}}),
    );
    let kept = c.plan(&kx, change.clone()).await;
    said.push(format!("rekey planned by kx: {}", class(&kept)));
    let planned = c
        .plan(&l.own, change)
        .await
        .must("planning the re-key")?
        .data;
    said.push(format!("rekey planned: {}", line(&planned)));
    let accepted = c
        .accept_planned(&l.own, &proposal, &planned.report_id)
        .await;
    said.push(format!("rekey accepted against it: {}", class(&accepted)));
    let expected = [
        "proposed: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1]",
        "planned: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1]",
        "stored, to the owner: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1]",
        "stored, to rv: [t 5 unlive r 3 k1,k3,k5 breaks named 1 k1]",
        "accepted against it: plan_stale",
        "planned again: [t 6 unlive r 4 k1,k3,k5,k7 breaks named 2 k1,k7]",
        "accepted against that: [t 6 unlive r 4 k1,k3,k5,k7 breaks named 2 k1,k7]",
        "a write accepted against a plan: proposal_plans_nothing",
        "accepted: [t 6 unlive r 2 k3,k5]",
        "kp proposes fh: [h 1 unlive r 1 h1]",
        "kp proposes fh2: [h 1 unlive r 1 h1]",
        "accepted fh: constraint_left_broken",
        "accepted fh2: [h 1 unlive r 1 h1]",
        "rekey proposed: [ac 0; pay 0 unlive ac 1 pay2]",
        "rekey planned by kx: [ac 0]",
        "rekey planned: [ac 0; pay 0 unlive ac 1 pay2]",
        "rekey accepted against it: [ac 0; pay 0 unlive ac 1 pay2]",
    ];
    ensure!(said == expected, "the answers were otherwise: {said:#?}");
    Ok(())
}

async fn a_proposal_is_lossless_as_its_proposer_is_told(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    let pr = holder(s, "pr", json!({"default": "write"}));
    let rv = holder(
        s,
        "rv",
        json!({"default": "read", "review": true, "schema": true}),
    );
    let sc = holder(s, "sc", json!({"default": "write", "schema": true}));
    let opts = WriteOpts::default;
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}, "required": true});
    // `m{n}`'s `d` is `s` until the migration makes `a0` none, which `ok`
    // refuses; `o{n}` re-keyed by `alias` rewrites `q{n}`'s reference to
    // `a` as `X`, which `not_x` refuses; `v{n}` renamed makes `w{n}`'s `a0`
    // `z`, which `not_z` refuses. `g2`, the one record each breaks, is held
    // in the first twin, retired in the second and live in the third.
    let m = |n: &str, d: &str| {
        json!({"name": format!("m{n}"), "fields": [text("s"),
            {"name": "d", "type": {"kind": "text"}, "authority": "derived", "derived": d,
             "depends_on": ["s"]}],
            "constraints": [{"name": "ok", "holds": "d != 'none'"}]})
    };
    let o = |n: &str, key: &str| json!({"name": format!("o{n}"), "key": [key], "fields": [text("code"), text("alias")]});
    let linked = |n: &str, by: &str| {
        json!({"name": format!("k{n}"), "fields": [text("s")],
            "relations": [{"name": "r", "target": format!("k{n}"), "cardinality": by}]})
    };
    let declared = |mut d: Value| {
        d["declare"] = json!("type");
        d
    };
    let mut said = Vec::new();
    for n in ["1", "2", "3"] {
        let (ty, q, w) = (format!("m{n}"), format!("q{n}"), format!("w{n}"));
        l.declare_json(declared(m(n, "s"))).await?;
        l.declare_json(declared(o(n, "code"))).await?;
        l.declare_json(json!({"declare": "type", "name": q, "fields": [
            {"name": "owner", "type": {"kind": "reference", "target": format!("o{n}")}}],
            "constraints": [{"name": "not_x", "holds": "not (owner = 'X')"}]}))
            .await?;
        let v = |to: &str| json!({"name": format!("v{n}"), "kind": "text", "one_of": ["a1", to]});
        let mut vt = v("a0");
        vt["declare"] = json!("value_type");
        l.declare_json(vt).await?;
        l.declare_json(json!({"declare": "type", "name": w, "fields": [
            {"name": "st", "type": {"kind": "text", "value_type": format!("v{n}")}}],
            "constraints": [{"name": "not_z", "holds": "not (st = 'z')"}]}))
            .await?;
        for (k, v) in [("a", "X"), ("b", "Y")] {
            let code = json!({"code": k, "alias": v});
            l.upsert(&format!("o{n}"), k, code).await?;
        }
        // `b{n}`'s `gross` is `net` by `rated{n}`: declared again to make
        // it `net * 4`, `g2`'s would break `under`.
        let (b, f) = (format!("b{n}"), format!("rated{n}"));
        let rated = |by: &str| {
            json!({"declare": "function", "name": f, "backing": "expr",
            "inputs": [{"name": "net", "type": {"kind": "decimal"}}], "expr": format!("net * {by}")})
        };
        l.declare_json(rated("2")).await?;
        l.declare_json(json!({"declare": "type", "name": b, "fields": [
            {"name": "net", "type": {"kind": "decimal"}},
            {"name": "gross", "type": {"kind": "decimal"}, "authority": "derived", "function": f}],
            "constraints": [{"name": "under", "holds": "gross < 10"}]}))
            .await?;
        for (k, v) in [("g1", "a1"), ("g2", "a0")] {
            l.upsert(&ty, k, json!({ "s": v })).await?;
            l.upsert(&w, k, json!({ "st": v })).await?;
            l.upsert(&b, k, json!({ "net": if k == "g1" { "1" } else { "3" } }))
                .await?;
        }
        for (k, v) in [("g1", "b"), ("g2", "a")] {
            l.upsert(&q, k, json!({ "owner": v })).await?;
        }
        for t in [&ty, &q, &w, &b] {
            match n {
                "1" => restrict(s, t, "g2", "a hold").await.map(drop)?,
                "2" => l.retire(t, "g2").await.map(drop)?,
                _ => {}
            }
        }
        let tight: Declaration = de(rated("4"));
        let planned = c.plan(
            &sc,
            Change::Declare {
                declarations: vec![tight.clone()],
            },
        );
        let planned = class(&planned.await);
        let declares = class(&c.declare(&sc, tight.clone()).await);
        let applies = class(&c.apply(&sc, vec![tight]).await);
        let note = de(json!({"name": "note", "type": {"kind": "text"}}));
        let adds = class(&c.add_field(&sc, &ty.as_str().into(), note).await);
        let what = Recompute::Function(f.as_str().into());
        let recomputes = class(&c.recompute(&sc, what).await);
        said.push(format!(
            "sc {n}: plans {}, declares {declares}, applies {applies}, adds {adds}, recomputes {recomputes}",
            planned.replace(&b, "b")
        ));
        // `k{n}`'s `x` is linked by `r` to `y` and to `z`, that link held,
        // unlinked or live: narrowed to `many_to_one`, `x` holds two.
        let k = format!("k{n}");
        l.declare_json(declared(linked(n, "many_to_many"))).await?;
        for key in ["x", "y", "z"] {
            l.upsert(&k, key, json!({ "s": key })).await?;
        }
        let (kt, r, x) = (&k.as_str().into(), &"r".into(), &"x".into());
        for to in [&"y".into(), &"z".into()] {
            let link = l.cat.link(&l.own, kt, x, r, to, patch(json!({})), opts());
            link.await.must("linking")?;
        }
        match n {
            "1" => restrict(s, &format!("sys.link.{k}.r"), "x:z", "a hold")
                .await
                .map(drop)?,
            "2" => (l.cat.unlink(&l.own, kt, x, r, &"z".into(), opts()).await)
                .must("unlinking")
                .map(drop)?,
            _ => {}
        }
        let migrate = |to| Change::Migrate {
            migration: Box::new(Migration::to(de(to))),
        };
        let mut renamed = ValueTypeMigration::from(de::<ValueTypeDecl>(v("z")));
        renamed.renames = vec![("a0".into(), "z".into())];
        let renamed = Change::MigrateValueType {
            migration: Box::new(renamed),
        };
        let changes = [
            (ty.clone(), migrate(m(n, "if(s = 'a0', 'none', s)"))),
            (format!("o{n}"), migrate(o(n, "alias"))),
            (format!("v{n}"), renamed),
            (k.clone(), migrate(linked(n, "many_to_one"))),
        ];
        for (name, change) in changes {
            let named = |a: String| a.replace(&name, &name[..1]);
            let schema = as_schema(c, &sc, change.clone()).await;
            said.push(format!("sc {name}: {}", named(schema)));
            let write = match change {
                Change::Migrate { migration } => ProposedWrite::Migrate {
                    migration,
                    report: Default::default(),
                },
                Change::MigrateValueType { migration } => ProposedWrite::MigrateValueType {
                    to: Box::new(migration.to),
                    renames: migration.renames,
                    report: Default::default(),
                },
                _ => continue,
            };
            let proposed = c.propose(&pr, write, "why".into()).await;
            said.push(format!("pr proposes {name}: {}", named(class(&proposed))));
            let Ok(lakelands_api::Outcome::Proposed { proposal, .. }) = proposed else {
                continue;
            };
            let listed = c.proposals(&rv, Default::default()).await;
            let report = (listed.must("the listing")?.data.into_iter())
                .find(|p| p.id == proposal)
                .and_then(|p| match p.write {
                    Some(ProposedWrite::Migrate { report, .. }) => Some(Plan {
                        lossless: report.lossless,
                        types: vec![*report],
                        ..Plan::default()
                    }),
                    Some(ProposedWrite::MigrateValueType { report, .. }) => Some(Plan {
                        lossless: report.fits,
                        value_types: vec![*report],
                        ..Plan::default()
                    }),
                    _ => None,
                });
            let report = report.map_or_else(|| "none".to_owned(), |p| line(&p));
            said.push(format!("rv lists {name}: {}", named(report)));
            let by_rv = class(&c.accept(&rv, &proposal).await);
            let accepted = class(&c.accept(&l.own, &proposal).await);
            said.push(format!("accepted {name}: {by_rv}, {}", named(accepted)));
        }
    }
    let expected = [
        "sc 1: plans [b 1], declares grant_needed, applies grant_needed, adds grant_needed, recomputes grant_needed",
        "sc m1: plans [m 1], previews [m 1], enacts grant_needed, applies grant_needed",
        "pr proposes m1: [m 1]",
        "rv lists m1: [m 1]",
        "accepted m1: grant_needed, migration",
        "sc o1: plans [o 0], previews [o 0], enacts grant_needed, applies grant_needed",
        "pr proposes o1: [o 0]",
        "rv lists o1: [o 0]",
        "accepted o1: grant_needed, constraint_left_broken",
        "sc v1: plans [v misfits 0], previews [v misfits 0], enacts grant_needed, applies grant_needed",
        "pr proposes v1: [v misfits 0]",
        "rv lists v1: [v misfits 0]",
        "accepted v1: grant_needed, constraint_left_broken",
        "sc k1: plans [k 0], previews [k 0], enacts grant_needed, applies grant_needed",
        "pr proposes k1: [k 0]",
        "rv lists k1: [k 0]",
        "accepted k1: grant_needed, narrowing_breaks_links",
        "sc 2: plans [b 1], declares grant_needed, applies grant_needed, adds grant_needed, recomputes grant_needed",
        "sc m2: plans [m 1], previews [m 1], enacts grant_needed, applies grant_needed",
        "pr proposes m2: [m 1]",
        "rv lists m2: [m 1]",
        "accepted m2: grant_needed, [m 1]",
        "sc o2: plans [o 0], previews [o 0], enacts grant_needed, applies grant_needed",
        "pr proposes o2: [o 0]",
        "rv lists o2: [o 0]",
        "accepted o2: grant_needed, [o 0]",
        "sc v2: plans [v misfits 0], previews [v misfits 0], enacts grant_needed, applies grant_needed",
        "pr proposes v2: [v misfits 0]",
        "rv lists v2: [v misfits 0]",
        "accepted v2: grant_needed, [v misfits 0]",
        "sc k2: plans [k 0], previews [k 0], enacts grant_needed, applies grant_needed",
        "pr proposes k2: [k 0]",
        "rv lists k2: [k 0]",
        "accepted k2: grant_needed, [k 0]",
        "sc 3: plans [b 2 breaks under 1 g2] lossy, declares grant_needed, applies grant_needed, adds grant_needed, recomputes grant_needed",
        "sc m3: plans [m 2 breaks ok 1 g2] lossy, previews [m 2 breaks ok 1 g2] lossy, enacts grant_needed, applies grant_needed",
        "pr proposes m3: migration",
        "sc o3: plans [o 0 breaks not_x 1 g2] lossy, previews [o 0 breaks not_x 1 g2] lossy, enacts grant_needed, applies grant_needed",
        "pr proposes o3: migration",
        "sc v3: plans constraint_left_broken, previews constraint_left_broken, enacts grant_needed, applies grant_needed",
        "pr proposes v3: constraint_left_broken",
        "sc k3: plans narrowing_breaks_links, previews narrowing_breaks_links, enacts grant_needed, applies grant_needed",
        "pr proposes k3: narrowing_breaks_links",
    ];
    ensure!(said == expected, "the proposals were otherwise: {said:#?}");
    Ok(())
}
