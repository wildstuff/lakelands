//! A grant by name holds a field through its renames
//! (`docs/decisions/006-grants-not-roles.md`, extended): a field's access
//! is the lowest of what a principal's grants say under every name the
//! field bears, has borne or, as of an earlier position, will bear.

use std::sync::Arc;

use lakelands_api::seam::Seams;
use lakelands_api::write::{Migration, Outcome, ProposedWrite};
use lakelands_api::{Access, LakeStore, Position, Principal};
use serde_json::{Value, json};

use super::computed_inputs::told;
use super::export::Scratch;
use crate::conformance::fixture::{
    Lake, Must, field_access, get_at, ingest, log_events, no_such_field, read_at, recorded_outputs,
    reviewer, source_record, try_upsert, write_verb,
};
use crate::conformance::{Failure, Given, ImportedLake};

check!(
    /// A grant holds a field under every name it has borne.
    GRANT_FOLLOWS = a_grant_holds_a_field_under_every_name_it_has_borne,
    "006 and 024, extended (lakelands#527): a field renamed by a migration is one field: its access is the lowest of what a principal's grants say under the name it bears at the position read and under every other name it has borne or will bear that they name, reading and writing judged apart; so a field held at none or masked stays so under its new name, through a chain of renames, and under its old name in a read as of before for a host that renamed its grant, at every door (`get`, a `where`, an `order_by`, an aggregate, a view's measure, a pattern, a search, `describe_type`, a write, a verb that sets it); a permission stays with its name, and a host widens by naming the new name and dropping the old; two fields that have borne one name are restricted together by it; a proposed migration that renames is listed to a reader kept from the old name with neither the pair nor the new name, and its accept answers it no rename; a migration's answer names its renames; the list is kept through a reopen, made again by a rebuild and an import, and is no event of the log",
    given
);

/// `account` with `fields`, each a text; the renamed one is searchable.
fn account(fields: &[&str]) -> Value {
    let searched = |n: &str| !["a", "c", "d"].contains(&n);
    let field = |n: &&str| json!({"name": n, "type": {"kind": "text"}, "searchable": searched(n)});
    json!({"name": "account", "fields": fields.iter().map(field).collect::<Vec<_>>()})
}

/// `base` with the members of `with` put over it.
fn over(mut base: Value, with: Value) -> Value {
    for (k, v) in with.as_object().into_iter().flatten() {
        base[k] = v.clone();
    }
    base
}

/// A principal with `fields` granted by name on `account`, every other
/// field of it at `default`.
fn held(s: &dyn LakeStore, default: Access, fields: &[(&str, Access)]) -> Principal {
    let mut who = field_access(s, "held", "account", fields);
    for t in who.grants.types.values_mut() {
        t.default = Some(default);
    }
    who
}

/// The owner migrates `account` to `fields` with `renames`.
async fn migrate(l: &Lake, fields: &[&str], renames: &[(&str, &str)]) -> Result<Outcome, Failure> {
    migrate_to(l, account(fields), renames).await
}

/// The owner migrates a type to the shape `to` with `renames`.
async fn migrate_to(l: &Lake, to: Value, renames: &[(&str, &str)]) -> Result<Outcome, Failure> {
    let m = Migration {
        to: serde_json::from_value(to).expect("a type"),
        renames: (renames.iter())
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
        relation_renames: Vec::new(),
    };
    let report = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .must("the preflight")?;
    let applied = l.cat.migrate_apply(&l.own, m, &report.data.report_id).await;
    applied.must("applying the migration")
}

/// The doors that answer `who` the field `field` of `account` as of
/// `as_of`: the seven read doors by its value, `describe_type` by its name.
async fn shown(
    store: &dyn LakeStore,
    who: &Principal,
    field: &str,
    as_of: Option<Position>,
) -> Vec<&'static str> {
    let l = Lake::of(store);
    let mut doors = told(&l, who, "account", field, as_of).await;
    let account = "account".into();
    let described = l.cat.describe_type(who, &account, read_at(as_of));
    let named = |d: lakelands_api::Reply<lakelands_api::describe::TypeDescription>| {
        d.data.fields.iter().any(|f| f.name.as_str() == field)
    };
    if described.await.is_ok_and(named) {
        doors.push("describe");
    }
    doors
}

/// The fields of record `1` of `ty` that `who` is answered, by name.
async fn fields(store: &dyn LakeStore, who: &Principal, ty: &str) -> Vec<String> {
    let got = Lake::of(store)
        .cat
        .get(who, &ty.into(), get_at("1", None))
        .await;
    let names = |r: lakelands_api::Reply<lakelands_api::Record>| {
        r.data.fields.keys().map(ToString::to_string).collect()
    };
    got.map(names).unwrap_or_default()
}

/// What a lake that holds the two renames answers `stayed` and `opened`,
/// of `owner` at the head and of `holder` as of `before`.
async fn kept(store: &dyn LakeStore, before: Position) -> [usize; 4] {
    let stayed = held(store, Access::Read, &[("holder", Access::None)]);
    let opened = held(store, Access::Read, &[("owner", Access::Read)]);
    [
        shown(store, &stayed, "owner", None).await.len(),
        shown(store, &stayed, "holder", Some(before)).await.len(),
        shown(store, &opened, "owner", None).await.len(),
        shown(store, &opened, "holder", Some(before)).await.len(),
    ]
}

async fn a_grant_holds_a_field_under_every_name_it_has_borne(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let mut first = account(&["a", "c", "holder"]);
    first["declare"] = json!("type");
    l.declare_json(first).await?;
    l.upsert(
        "account",
        "1",
        json!({"a": "1", "c": "c", "holder": "secret"}),
    )
    .await?;
    let before = s.head().await.must("reading the head")?;
    // `holder` becomes `keeper`, then `owner`; the answer names the rename.
    let answer = migrate(&l, &["a", "c", "keeper"], &[("holder", "keeper")]).await?;
    let pair = vec![("holder".into(), "keeper".into())];
    ensure!(
        matches!(&answer, Outcome::Applied { renamed, .. } if renamed == &pair),
        "a migration's answer names each field it renamed; the owner is answered {answer:?}"
    );
    migrate(&l, &["a", "c", "owner"], &[("keeper", "owner")]).await?;
    let measures = json!([{"name": "m", "expr": "max(owner)"}]);
    let view = json!({"source": "account", "group_by": ["a"], "measures": measures});
    let seen = json!([{"name": "a", "type": {"kind": "text"}},
                      {"name": "m", "type": {"kind": "text"}}]);
    l.declare_json(
        json!({"declare": "type", "name": "v_account_owner", "key": ["a"],
                          "fields": seen, "view": view}),
    )
    .await?;

    // Each principal, by its grants: whether it reads the field as `owner`
    // at the head, and as `holder` as of before the renames.
    let (none, read, write) = (Access::None, Access::Read, Access::Write);
    type Row<'a> = (&'a str, Access, &'a [(&'a str, Access)], bool, bool);
    let rows: [Row; 8] = [
        ("it did nothing", read, &[("holder", none)], false, false),
        (
            "it renamed its grant",
            read,
            &[("owner", none)],
            false,
            false,
        ),
        (
            "it followed one rename",
            read,
            &[("keeper", none)],
            false,
            false,
        ),
        (
            "the lowest of a chain",
            read,
            &[("holder", none), ("keeper", read)],
            false,
            false,
        ),
        (
            "it widened and kept the old",
            read,
            &[("holder", none), ("owner", read)],
            false,
            false,
        ),
        (
            "it widened and dropped the old",
            read,
            &[("owner", read)],
            true,
            true,
        ),
        (
            "a permission stays with its name",
            none,
            &[("a", read), ("holder", write)],
            false,
            true,
        ),
        (
            "named anew",
            none,
            &[("a", read), ("holder", write), ("owner", write)],
            true,
            true,
        ),
    ];
    let mut red = Vec::new();
    for (what, default, grants, now, then) in rows {
        let who = held(s, default, grants);
        let head = shown(s, &who, "owner", None).await;
        let past = shown(s, &who, "holder", Some(before)).await;
        // Every door answers, or none: eight at the head, and seven as of
        // before, when no view read the field.
        if head.len() != if now { 8 } else { 0 } || past.len() != if then { 7 } else { 0 } {
            red.push(format!("{what} ({grants:?}): `owner` through {head:?}, `holder` as of before through {past:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "a field is read by the lowest of its grants under every name it has borne, at every \
         door, at the head and as of before; {}",
        red.join("; ")
    );

    // A masked field stays masked, a write and a verb that sets the field
    // are refused to one kept from it under its old name.
    let masked = held(s, read, &[("holder", Access::Masked)]);
    let got = l
        .cat
        .get(&masked, &"account".into(), get_at("1", None))
        .await;
    let got = got.must("reading as `masked`")?.data;
    l.declare_as(write_verb("set_owner", "account", "owner", "'x'"), "a verb")
        .await?;
    let writer = held(s, write, &[("holder", none)]);
    let wrote = try_upsert(
        l.cat.as_ref(),
        &writer,
        "account",
        "1",
        json!({"owner": "x"}),
    )
    .await;
    let (verb, one) = ("set_owner".into(), "1".into());
    let call = (l.cat).call(
        &writer,
        &verb,
        Some(&one),
        Default::default(),
        Default::default(),
    );
    let called = call.await;
    ensure!(
        got.masked.iter().any(|m| m.as_str() == "owner")
            && no_such_field(&wrote, "owner")
            && called.is_err(),
        "a field held masked is masked under its new name, and one held at none is not \
         written under it, by an upsert or a verb; masked {:?}, the upsert {wrote:?}, the \
         call {called:?}",
        got.masked
    );

    // A name taken again by another field, and a swap: two fields that
    // have borne one name are restricted together by it.
    let again = json!({"name": "holder", "type": {"kind": "text"}});
    let again = serde_json::from_value(again).expect("a field");
    (l.cat.add_field(&l.own, &"account".into(), again).await).must("adding `holder` again")?;
    l.upsert("account", "1", json!({"holder": "another"}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "pair", "fields": [
        {"name": "a", "type": {"kind": "text"}}, {"name": "b", "type": {"kind": "text"}}]}))
        .await?;
    l.upsert("pair", "1", json!({"a": "A", "b": "B"})).await?;
    let swap = Migration {
        to: serde_json::from_value(json!({"name": "pair", "fields": [
            {"name": "a", "type": {"kind": "text"}}, {"name": "b", "type": {"kind": "text"}}]}))
        .expect("a type"),
        renames: vec![("a".into(), "b".into()), ("b".into(), "a".into())],
        relation_renames: Vec::new(),
    };
    let report = l
        .cat
        .migrate(&l.own, swap.clone())
        .await
        .must("the swap's preflight")?;
    let swapped = l
        .cat
        .migrate_apply(&l.own, swap, &report.data.report_id)
        .await;
    swapped.must("applying the swap")?;
    let apart = field_access(s, "apart", "pair", &[("a", none), ("b", read)]);
    let shared = [
        fields(s, &held(s, read, &[("holder", none)]), "account").await,
        fields(s, &held(s, read, &[("owner", none)]), "account").await,
        fields(
            s,
            &held(s, none, &[("a", read), ("holder", read)]),
            "account",
        )
        .await,
        fields(s, &apart, "pair").await,
    ];
    let names = |n: &[&str]| n.iter().map(ToString::to_string).collect::<Vec<_>>();
    ensure!(
        shared
            == [
                names(&["a", "c"]),
                names(&["a", "c", "holder"]),
                names(&["a", "holder"]),
                vec![]
            ],
        "a name two fields have borne restricts both, and a permission on it reaches only the \
         field that bears it; `holder: none`, `owner: none`, `holder: read` on a closed type, \
         and `a: none, b: read` after a swap are answered the fields {shared:?}"
    );

    // The list is kept through a reopen and made again by a rebuild and an
    // import; the log begins as it was (a writer's open adds its lease) and
    // holds no event of the list.
    let whole = [0, 0, 8, 7];
    let log = log_events(s).await?;
    let scratch = Scratch::new(given.scratch.clone());
    let from = scratch.dir("from");
    crate::export::write(s, &from)
        .await
        .must("exporting the lake")?;
    let lake = store.lake().clone();
    let imported = (given.import)(from, lake, None, Seams::default()).await;
    let imported = imported.map_err(|e| Failure::new(format!("importing the export: {e}")));
    let ImportedLake { store: copy, .. } = imported?;
    let of_copy = kept(copy.as_ref(), before).await;
    let store = (given.reopen)(store, Seams::default()).await;
    let store = store.map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let reopened = kept(store.as_ref(), before).await;
    store.rebuild().await.must("rebuilding")?;
    let rebuilt = kept(store.as_ref(), before).await;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let outputs = recorded_outputs(s).await?;
    ensure!(
        [reopened, rebuilt, of_copy] == [whole; 3]
            && log_events(s).await?.starts_with(&log)
            && outputs.is_empty(),
        "a lake opened again, rebuilt and imported holds a field through its renames as \
         before, and its log begins as it was, with no event of the list; the doors that answer \
         `holder: none` and `owner: read`, at the head and as of before, number {reopened:?} \
         reopened, {rebuilt:?} rebuilt and {of_copy:?} imported; {} output event(s)",
        outputs.len()
    );

    // A proposed migration that renames `c`, listed to a reviewer kept
    // from it, shows neither the pair nor the new name; its accept answers
    // that reviewer no rename.
    let mut kept_from = reviewer(s, true);
    kept_from.grants.types = held(s, read, &[("c", none)]).grants.types;
    let to = serde_json::from_value(account(&["a", "d", "owner", "holder"])).expect("a type");
    let write = ProposedWrite::Migrate {
        migration: Box::new(Migration {
            to,
            renames: vec![("c".into(), "d".into())],
            relation_renames: Vec::new(),
        }),
        report: Default::default(),
    };
    let proposed = l.cat.propose(&l.own, write, "renaming `c`".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing the rename")? else {
        return Err(Failure::new("the migration is proposed".to_owned()));
    };
    // And one that may know of the field under its new name alone, on a
    // type it otherwise does not read, is listed the new name without the
    // pair: the old name is one it may not know of.
    let mut anew = reviewer(s, true);
    anew.grants.types = held(s, none, &[("a", read), ("d", read)]).grants.types;
    let mut entries = Vec::new();
    for who in [&kept_from, &anew] {
        let listed = l.cat.proposals(who, Default::default()).await;
        let listed = listed.must("listing")?.data;
        entries.extend(listed.iter().map(|p| (json!(p.write).to_string(), p.more)));
    }
    let accepted = l.cat.accept(&anew, &proposal).await;
    let pair = |w: &String| w.contains("\"c\"");
    ensure!(
        matches!(entries.as_slice(), [(w, true), (n, true)]
            if !w.contains("\"d\"") && !pair(w) && n.contains("\"d\"") && !pair(n))
            && matches!(&accepted, Ok(Outcome::Applied { renamed, .. }) if renamed.is_empty()),
        "a proposed rename is listed without the pair to a reader who may not know of the \
         field under its old name, without the new name too where it is kept from the field, \
         and its accept answers no rename; listed {entries:?}, accepted {accepted:?}"
    );

    // Two chains of names that end in one name: a rename continues the one
    // whose last name was taken latest, since every earlier bearer of the
    // name is gone. `c` in `one` and `p` in `two` end as `d`.
    let shape = |ty: &str, fields: &[&str]| {
        let field = |n: &&str| json!({"name": n, "type": {"kind": "text"}});
        json!({"name": ty, "fields": fields.iter().map(field).collect::<Vec<_>>()})
    };
    type Steps<'a> = &'a [(&'a [&'a str], &'a [(&'a str, &'a str)])];
    let chains: [(&str, &[&str], Steps, &str); 2] = [
        (
            "one",
            &["a", "c"],
            &[
                (&["b", "c"], &[("a", "b")]),
                (&["c"], &[]),
                (&["b"], &[("c", "b")]),
                (&["d"], &[("b", "d")]),
            ],
            "c",
        ),
        (
            "two",
            &["a", "p"],
            &[
                (&["a", "q"], &[("p", "q")]),
                (&["b", "q"], &[("a", "b")]),
                (&["q"], &[]),
                (&["b"], &[("q", "b")]),
                (&["d"], &[("b", "d")]),
            ],
            "p",
        ),
    ];
    let mut ends = Vec::new();
    for (ty, first, steps, kept) in chains {
        l.declare_json(over(shape(ty, first), json!({"declare": "type"})))
            .await?;
        l.upsert(ty, "1", json!({first[0]: "A", first[1]: "B"}))
            .await?;
        for (fields, renames) in steps {
            migrate_to(&l, shape(ty, fields), renames).await?;
        }
        let who = field_access(s, "chained", ty, &[(kept, none)]);
        ends.push(fields(s, &who, ty).await);
    }
    ensure!(
        ends == [Vec::<String>::new(), vec![]],
        "a field renamed onto a name another field bore before it was removed is held by the \
         grant on its own first name; one kept from `c`, and one kept from `p`, are answered \
         the fields {ends:?}"
    );

    // An ingest is a write: a field its principal is kept from under its
    // old name is not fed under its new one.
    let fed = |field: &str| {
        let f = |n: &str| json!({"name": n, "type": {"kind": "text"}, "authority": "source"});
        json!({"name": "fed", "fields": [f("k"), f(field)]})
    };
    l.declare_json(over(fed("v"), json!({"declare": "type"})))
        .await?;
    migrate_to(&l, fed("w"), &[("v", "w")]).await?;
    let mut feeder = field_access(s, "feeder", "fed", &[("v", none)]);
    (feeder.grants.default, feeder.grants.ingest) = (Access::Write, true);
    let record = vec![source_record("fed", "1", json!({"k": "1", "w": "x"}))];
    let fed = ingest(l.cat.as_ref(), &feeder, "src", record).await;
    ensure!(
        no_such_field(&fed, "w"),
        "an ingest of a field its principal is kept from under its old name is answered as \
         for a field the type does not have; it is answered {fed:?}"
    );
    Ok(())
}
