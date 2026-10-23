//! A set's `where` and a trigger's `when` read across an edge, as a verb's
//! condition does, and are decided again at every write of the far record
//! that moves what they read there
//! (`docs/decisions/055-a-path-steps-through-a-reference.md`, extended).

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::Grants;
use lakelands_api::seam::AGING_RULE_TYPE;

use crate::conformance::fixture::{ingest, reference, source_record};
use crate::conformance::prelude::*;

check!(
    SET_ACROSS = a_sets_where_reads_a_field_across_an_edge,
    "055 and 004, extended (lakelands#567): a set's `where` and a trigger's `when` read a field of the record a reference names, by one step and by two, the far record read at the head whoever is answered it, one not live reading null, and over the append's own drafts: declared over records there, the set joins those it holds for; a write of the far record that moves a field the set reads there decides again, in its own append and caused by its event, every record that reaches it, and its answer names the set; a write that moves nothing read there appends nothing more; a write of an intermediate record moves the records two steps back; retiring the far record takes its referrers out and writing it again brings them back; a referrer's own write reads across too; the trigger fires once for each referrer whose `when` holds after the far write, its answer naming it once, and a retry of the write answers the same however many reactions it set off, and after later writes that set off reactions of their own reads no further than a page past its append, holding as much after four times as many of them as after the first, give or take a quarter and 1 MiB; a rebuild keeps the members, so a later write decides against them",
    given
);

check!(
    SET_ACROSS_SELF = a_set_across_a_reference_to_its_own_type_steps_back_by_the_path,
    "055 and 004, extended (lakelands#567): a path through a reference to a record of the same type is stepped back by the path's steps, not by type: a manager's grade moves the employees under it and no other, one that is its own manager is decided once at its own write, and a parent's grade moves its children's invoices and not its own"
);

check!(
    SET_ACROSS_EDGES = a_sets_where_reads_a_link_and_a_derived_relation,
    "055 and 004, extended (lakelands#567): a set's `where` reads the one link a record holds under a relation carried by links (`with.note`), asks `exists(with)`, as a trigger's `when` does, and reads the record a derived relation matches onto its target's key (`terms.factor`): a link written, rewritten and retired moves the record that holds it, and a write of the matched record moves the records that match it"
);

check!(
    SET_ACROSS_BOUND = what_a_far_write_sets_off_is_held_to_the_ceiling,
    "004, 036 and 039, extended (lakelands#567): past the lake's `max_set_changes`, counted over the memberships and firings a far write sets off across an edge in its append, the write is refused `too_large` naming the set and the ceiling and appends nothing, at `upsert` and at the accept of its proposal, and naming nothing to a writer not shown the set; a purge's preflight reports how many it would set off beside the ceiling, on a read-only handle too, appending nothing, and the purge is refused whole; an aging move past it is counted as refused, `too_large` listed with its key, and no failed guard, the record left as it was (lakelands#1182)",
    given
);

check!(
    SET_ACROSS_DECLARED = a_set_across_an_edge_is_judged_with_what_it_reads,
    "055 and 028, extended (lakelands#567): a set's `where` or a trigger's `when` that reads across an edge a field computed outside the lake, or one derived from such a field, is refused `set_reads_computed` naming it, `changed` of a path is `call_takes_field`, and a migration that renames a far field such a set reads is refused naming the set, and so is the type declared again so that such a field derives from one computed outside the lake; a type declared again so that a far derived field computes otherwise moves the referrers in its append, and a value type's rename under a far field moves them in the migration's",
    given
);

check!(
    SET_CLOCK = a_sets_where_that_reads_the_clock_is_refused,
    "066, extended (lakelands#1223): a set's `where` that calls `now()` or `today()`, however deep in it, is refused `set_reads_clock` naming the set and the call, over a type and over a kept view alike, and a set whose `where` reads a stored date field declares: a set keeps what it decided at the write, so a clock would drift from a read's"
);

check!(
    SET_CLOCK_REPLAYS = a_log_holding_a_set_that_reads_the_clock_opens_and_replays,
    "066, extended (lakelands#1223): a log whose `declared` event holds a set that reads `now()`, as a writer before the refusal appended it, opens for writing and rebuilds, and reads `in_set` of that set the same after each, a write after the open lands, and the refusal `set_reads_clock` is met only where the set is declared again",
    given
);

check!(
    SET_ACROSS_READ = a_reads_in_set_of_a_set_across_an_edge_reads_its_kept_members,
    "055 and 004, extended (lakelands#567): a read's `in_set` of a set whose `where` reads across an edge answers the set's kept membership, not its filter read again: at the head, as of an earlier position, in a count, beside the read's own conditions, and through a set that names it; a far write moves what it answers as it moves the members, and a far record held from no one changes nothing it answers"
);

check!(
    SET_ACROSS_HELD = a_set_across_an_edge_is_a_readers_only_where_nothing_far_is_held,
    "055 and 024, extended (lakelands#567): a set or a trigger whose condition reads across an edge is a reader's only where nothing of the far type is held from it: to one without `restricted`, or holding `only` on the far type, or kept from a far field, the set is `set_undeclared` in a read's `in_set` and `describe` names neither, whether a far record is restricted or not, so twin lakes that differ in a held customer answer it alike; to a reader holding `restricted` and every field both are described; a writer is told a set moved, or a trigger fired, at a record other than its own only where it is answered one of the records moved"
);

/// The later writes a retry is read after, and four times as many.
const LATER: usize = 500;

/// The membership changes in the events from `at`: (set, key, joined).
async fn moves(s: &dyn LakeStore, at: Position) -> Result<Vec<(String, String, bool)>, Failure> {
    let events = events_from(s, at).await?.into_iter();
    let changed = events.filter_map(|e| match e.body {
        EventBody::ObjectSetChanged {
            set, key, joined, ..
        } => Some((set, key.to_string(), joined)),
        _ => None,
    });
    let mut changed: Vec<_> = changed.collect();
    changed.sort();
    Ok(changed)
}

/// The firings in the events from `at`: (trigger, key).
async fn fired(s: &dyn LakeStore, at: Position) -> Result<Vec<(String, String)>, Failure> {
    let events = events_from(s, at).await?.into_iter();
    let fired = events.filter_map(|e| match e.body {
        EventBody::TriggerFired { trigger, key, .. } => {
            Some((trigger.to_string(), key.to_string()))
        }
        _ => None,
    });
    let mut fired: Vec<_> = fired.collect();
    fired.sort();
    Ok(fired)
}

/// `(set, key, joined)` rows, sorted.
fn rows(r: &[(&str, &str, bool)]) -> Vec<(String, String, bool)> {
    let mut r: Vec<_> = (r.iter())
        .map(|(s, k, j)| ((*s).to_owned(), (*k).to_owned(), *j))
        .collect();
    r.sort();
    r
}

/// The next position.
async fn next(s: &dyn LakeStore) -> Result<Position, Failure> {
    Ok(s.head().await.must("the head")?.next())
}

/// Regions, customers that name one, and invoices that name a customer:
/// `c1` graded `x` in the north, `c2` graded `y` in the south; `i1` open
/// and `i2` closed of `c1`, `i3` open of `c2`, `i4` open of none.
async fn lake(store: &Store) -> Result<Lake, Failure> {
    let (l, text) = (Lake::of(store.as_ref()), StorageKind::Text);
    l.declare(type_decl("region", vec![field("name", text)]))
        .await?;
    let customer = vec![field("grade", text), field("note", text)];
    let customer = [customer, vec![reference("region", "region")]].concat();
    l.declare(type_decl("customer", customer)).await?;
    let invoice = vec![field("status", text), reference("customer", "customer")];
    l.declare(type_decl("invoice", invoice)).await?;
    for (key, name) in [("r1", "north"), ("r2", "south")] {
        l.upsert("region", key, json!({"name": name})).await?;
    }
    for (key, grade, region) in [("c1", "x", "r1"), ("c2", "y", "r2")] {
        l.upsert("customer", key, json!({"grade": grade, "region": region}))
            .await?;
    }
    for (key, status, customer) in [
        ("i1", "open", json!("c1")),
        ("i2", "closed", json!("c1")),
        ("i3", "open", json!("c2")),
        ("i4", "open", json!(null)),
    ] {
        let fields = json!({"status": status, "customer": customer});
        l.upsert("invoice", key, fields).await?;
    }
    Ok(l)
}

/// The sets and the trigger of these rows.
async fn graded(l: &Lake) -> Held {
    let graded = "customer.grade = 'x' and status = 'open'";
    l.declare_as(object_set_on("graded", "invoice", graded), "graded")
        .await?;
    let north = "customer.region.name = 'north'";
    l.declare_as(object_set_on("north", "invoice", north), "north")
        .await?;
    let hot = trigger_on("hot", "invoice", Some("customer.grade = 'x'"));
    l.declare_as(hot, "hot").await?;
    Ok(())
}

async fn a_sets_where_reads_a_field_across_an_edge(store: Store, given: Given) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let at = next(s).await?;
    graded(&l).await?;
    let want = rows(&[
        ("graded", "i1", true),
        ("north", "i1", true),
        ("north", "i2", true),
    ]);
    let got = moves(s, at).await?;
    ensure!(
        got == want,
        "declared over the records there, the sets join {want:?}; they joined {got:?}"
    );
    // `c2` graded `x`: `i3` joins, caused by the customer's write.
    let at = next(s).await?;
    let wrote = l.upsert("customer", "c2", json!({"grade": "x"})).await?;
    let caused = events_from(s, at)
        .await?
        .iter()
        .skip(1)
        .all(|e| e.cause == Some(at));
    let named = matches!(&wrote, Outcome::Applied { set_off, .. }
        if set_off.joined == ["graded"] && set_off.fired == ["hot"]);
    let (got, hot) = (moves(s, at).await?, fired(s, at).await?);
    ensure!(
        got == rows(&[("graded", "i3", true)])
            && hot == [("hot".to_owned(), "i3".to_owned())]
            && caused
            && named,
        "`c2` graded `x` takes `i3` into `graded` and fires `hot` for it, caused by its \
         write, and its answer names them; it moved {got:?}, fired {hot:?} and answered \
         {wrote:?}"
    );
    // A field no set reads moves nothing.
    let at = next(s).await?;
    l.upsert("customer", "c2", json!({"note": "n"})).await?;
    let more = events_from(s, at).await?.len();
    ensure!(
        more == 1,
        "a write of a field no set reads appends its own event alone; it appended {more}"
    );
    // Two steps back: the north is renamed, and `c1`'s invoices leave.
    let at = next(s).await?;
    l.upsert("region", "r1", json!({"name": "south"})).await?;
    let got = moves(s, at).await?;
    let want = rows(&[("north", "i1", false), ("north", "i2", false)]);
    ensure!(
        got == want,
        "a write of the region moves the invoices two steps back, {want:?}; it moved {got:?}"
    );
    // A far record not live reads null.
    let at = next(s).await?;
    l.retire("customer", "c1").await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("graded", "i1", false)]),
        "retiring `c1` takes `i1` out of `graded`; it moved {got:?}"
    );
    let at = next(s).await?;
    l.upsert("customer", "c1", json!({"grade": "x"})).await?;
    let (got, hot) = (moves(s, at).await?, fired(s, at).await?);
    let both = ["i1", "i2"].map(|k| ("hot".to_owned(), k.to_owned()));
    ensure!(
        got == rows(&[("graded", "i1", true)]) && hot == both,
        "writing `c1` again brings `i1` back and fires `hot` for both its invoices; it \
         moved {got:?} and fired {hot:?}"
    );
    // A referrer's own write reads across.
    let at = next(s).await?;
    l.upsert("invoice", "i3", json!({"status": "closed"}))
        .await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("graded", "i3", false)]),
        "`i3` closed leaves `graded`; it moved {got:?}"
    );
    // Within one append: a deal ingested beside its lead is decided again
    // when the lead's next record in the window moves what the set reads.
    let mut lead = reference("lead", "lead");
    lead.authority = lakelands_api::ontology::Authority::Source;
    l.declare(type_decl("lead", vec![sourced("hot", StorageKind::Text)]))
        .await?;
    l.declare(type_decl("deal", vec![lead])).await?;
    l.declare_as(object_set_on("heated", "deal", "lead.hot = 'y'"), "heated")
        .await?;
    let at = next(s).await?;
    let window = vec![
        source_record("lead", "l1", json!({"hot": "n"})),
        source_record("deal", "d1", json!({"lead": "l1"})),
        source_record("lead", "l1", json!({"hot": "y"})),
    ];
    ingest(l.cat.as_ref(), &l.own, "crm", window)
        .await
        .must("ingesting a window")?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("heated", "d1", true)]),
        "`l1` heated after `d1` in one window takes `d1` into `heated`; it moved {got:?}"
    );
    // A retry of a far write answers what its first write did, however
    // many reactions it set off, each trigger named once.
    l.upsert("customer", "c9", json!({"grade": "y"})).await?;
    for i in 0..10 {
        let fields = json!({"status": "open", "customer": "c9"});
        l.upsert("invoice", &format!("n{i}"), fields).await?;
    }
    let (ty, c9) = (
        TypeName::from("customer"),
        lakelands_api::RecordKey::from("c9"),
    );
    let keyed = || WriteOpts {
        idempotency_key: Some("regrade".into()),
        ..WriteOpts::default()
    };
    let first = l
        .cat
        .upsert(&l.own, &ty, &c9, patch(json!({"grade": "x"})), keyed());
    let first = first.await.must("`c9` graded `x`")?;
    let again = l
        .cat
        .upsert(&l.own, &ty, &c9, patch(json!({"grade": "x"})), keyed());
    let again = again.await.must("`c9` graded `x` again")?;
    let once = matches!(&first, Outcome::Applied { set_off, .. }
        if set_off.fired == ["hot"] && set_off.joined == ["graded"] && set_off.through.is_some());
    ensure!(
        once && first == again,
        "`c9` graded `x` sets off 20 reactions and names `hot` and `graded` once, and its \
         retry answers the same; they answered {first:?} and {again:?}"
    );
    // Later writes that set off reactions of their own end its paging:
    // the retry reads a page past its append at most, so it holds as much
    // after four times as many of them as after the first.
    let mut held = Vec::new();
    for (from, to) in [(0, LATER), (LATER, 4 * LATER)] {
        for i in from..to {
            let fields = json!({"status": "open", "customer": "c9"});
            l.upsert("invoice", &format!("w{i}"), fields).await?;
        }
        let meter = given.meter.as_ref();
        let before = meter.map(|m| {
            m.reset_peak();
            m.live()
        });
        let late = l
            .cat
            .upsert(&l.own, &ty, &c9, patch(json!({"grade": "x"})), keyed());
        let late = late
            .await
            .must("`c9` graded `x` again after later writes")?;
        held.push(meter.zip(before).map(|(m, b)| m.peak().saturating_sub(b)));
        ensure!(
            late == first,
            "after {to} later writes the retry answers {first:?}; it answered {late:?}"
        );
    }
    if let [Some(one), Some(four)] = held[..] {
        ensure!(
            four <= one + one / 4 + (1 << 20),
            "the retry holds {one} bytes after {LATER} later writes and {four} after {}: \
             it reads past its own append",
            4 * LATER
        );
    }
    // A rebuild keeps the members: `c1` graded `y` takes `i1` out.
    store.rebuild().await.must("rebuilding the projection")?;
    let at = next(s).await?;
    l.upsert("customer", "c1", json!({"grade": "y"})).await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("graded", "i1", false)]),
        "after a rebuild `c1` graded `y` takes `i1` out of `graded`; it moved {got:?}"
    );
    Ok(())
}

/// Invoices related to agents by `with`, carried by links holding a
/// `note`, and to a rate by `terms`, derived from their currency onto the
/// rate's key.
async fn edges(store: &Store) -> Result<Lake, Failure> {
    let (l, text, number) = (
        Lake::of(store.as_ref()),
        StorageKind::Text,
        StorageKind::Number,
    );
    l.declare(type_decl("agent", vec![field("name", text)]))
        .await?;
    let mut rate = type_decl(
        "rate",
        vec![field("currency", text), field("factor", number)],
    );
    rate.key = vec!["currency".into()];
    l.declare(rate).await?;
    let mut invoice = type_decl("invoice", vec![field("currency", text)]);
    let pairs = vec![MatchPair {
        from: "currency".into(),
        to: "currency".into(),
    }];
    invoice.relations = vec![
        RelationDecl {
            cardinality: Cardinality::ManyToOne,
            fields: vec![field("note", text)],
            ..RelationDecl::to("with", "agent")
        },
        RelationDecl {
            derived: Some(Derived {
                pairs,
                filter: None,
            }),
            ..RelationDecl::to("terms", "rate")
        },
    ];
    l.declare(invoice).await?;
    l.upsert("agent", "a1", json!({"name": "a"})).await?;
    for (currency, factor) in [("eur", 1), ("usd", 2)] {
        let fields = json!({"currency": currency, "factor": factor});
        l.upsert("rate", currency, fields).await?;
    }
    for (key, currency) in [("i1", "eur"), ("i2", "usd"), ("i3", "eur")] {
        l.upsert("invoice", key, json!({"currency": currency}))
            .await?;
    }
    for (name, filter) in [
        ("vip", "with.note = 'vip'"),
        ("handled", "exists(with)"),
        ("dear", "terms.factor > 1"),
    ] {
        l.declare_as(object_set_on(name, "invoice", filter), name)
            .await?;
    }
    let linked = trigger_on("linked", "invoice", Some("exists(with)"));
    l.declare_as(linked, "linked").await?;
    Ok(l)
}

async fn a_sets_where_reads_a_link_and_a_derived_relation(store: Store) -> Held {
    let (s, l) = (store.as_ref(), edges(&store).await?);
    let (ty, with, opts) = ("invoice".into(), "with".into(), WriteOpts::default);
    let (i1, a1) = ("i1".into(), "a1".into());
    let at = next(s).await?;
    let linked = l.cat.link(
        &l.own,
        &ty,
        &i1,
        &with,
        &a1,
        patch(json!({"note": "vip"})),
        opts(),
    );
    linked.await.must("linking `i1` to `a1`")?;
    let (got, fire) = (moves(s, at).await?, fired(s, at).await?);
    ensure!(
        got == rows(&[("handled", "i1", true), ("vip", "i1", true)])
            && fire == [("linked".to_owned(), "i1".to_owned())],
        "a link noted `vip` takes `i1` into `vip` and `handled` and fires `linked`, whose \
         `when` asks `exists(with)`; it moved {got:?} and fired {fire:?}"
    );
    let at = next(s).await?;
    let linked = l.cat.link(
        &l.own,
        &ty,
        &i1,
        &with,
        &a1,
        patch(json!({"note": "plain"})),
        opts(),
    );
    linked.await.must("noting the link otherwise")?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("vip", "i1", false)]),
        "the link rewritten takes `i1` out of `vip`; it moved {got:?}"
    );
    let at = next(s).await?;
    let unlinked = l.cat.unlink(&l.own, &ty, &i1, &with, &a1, opts());
    unlinked.await.must("unlinking")?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("handled", "i1", false)]),
        "the link retired takes `i1` out of `handled`; it moved {got:?}"
    );
    let at = next(s).await?;
    l.upsert("rate", "eur", json!({"factor": 3})).await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("dear", "i1", true), ("dear", "i3", true)]),
        "the `eur` rate raised takes its invoices into `dear`; it moved {got:?}"
    );
    Ok(())
}

async fn what_a_far_write_sets_off_is_held_to_the_ceiling(store: Store, given: Given) -> Held {
    let (l, text) = (Lake::of(store.as_ref()), StorageKind::Text);
    let customer = vec![sourced("grade", text), field("tier", text)];
    l.declare(type_decl("customer", customer)).await?;
    let invoice = vec![field("status", text), reference("customer", "customer")];
    l.declare(type_decl("invoice", invoice)).await?;
    let fed = vec![source_record("customer", "c1", json!({"grade": "x"}))];
    ingest(l.cat.as_ref(), &l.own, "crm", fed)
        .await
        .must("ingesting `c1`")?;
    l.upsert("customer", "c2", json!({"tier": "a"})).await?;
    for (key, customer) in [("i1", "c1"), ("i2", "c1"), ("i3", "c2"), ("i4", "c2")] {
        let fields = json!({"status": "open", "customer": customer});
        l.upsert("invoice", key, fields).await?;
    }
    for (name, filter) in [
        ("graded", "customer.grade = 'x'"),
        ("tiered", "customer.tier = 'a'"),
    ] {
        l.declare_as(object_set_on(name, "invoice", filter), name)
            .await?;
    }
    let (ty, key) = (
        TypeName::from("customer"),
        lakelands_api::RecordKey::from("c2"),
    );
    let proposed = l.cat.propose(
        &l.own,
        ProposedWrite::Upsert {
            ty: ty.clone(),
            key: key.clone(),
            patch: patch(json!({"tier": "b"})),
            opts: WriteOpts::default(),
        },
        "why".into(),
    );
    let Outcome::Proposed { proposal, .. } = proposed.await.must("a proposal")? else {
        return Err(Failure::new("an upsert is proposed"));
    };
    let rule = json!({"on": "customer", "field": "tier", "from": "a", "to": "b",
        "when": "true"});
    l.upsert(AGING_RULE_TYPE, "retier", rule).await?;
    let store = at_one_set_change(&given, store, "opening at a ceiling of one").await?;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let before = s.head().await.must("the head")?;
    let large = |r: &lakelands_api::Result<Outcome>, set: &str| matches!(refused_on(r), Some((named, Refusal::TooLarge { max: 1 })) if named == set);
    let up = l.cat.upsert(
        &l.own,
        &ty,
        &key,
        patch(json!({"tier": "b"})),
        WriteOpts::default(),
    );
    let up = up.await;
    let accepted = l.cat.accept(&l.own, &proposal).await;
    ensure!(
        large(&up, "tiered") && large(&accepted, "tiered"),
        "`c2` moved to tier `b`, moving 2 memberships under a ceiling of 1, is refused \
         too_large naming `tiered`, at `upsert` and at the accept; they answered {up:?} and \
         {accepted:?}"
    );
    head_unmoved(s, before, "the refused far writes").await?;
    // A writer not shown `tiered` is told the size alone: the bound counts
    // the append, records held from the writer among them.
    let blind = principal(
        s,
        PrincipalKind::Agent,
        "blind",
        Grants {
            default: Access::Write,
            ..Grants::default()
        },
    );
    let up = l.cat.upsert(
        &blind,
        &ty,
        &key,
        patch(json!({"tier": "b"})),
        WriteOpts::default(),
    );
    let up = up.await;
    ensure!(
        matches!(
            &up,
            Err(lakelands_api::Error::Refused {
                field: None,
                reason: Refusal::TooLarge { max: 1 }
            })
        ),
        "to a writer not shown `tiered`, the write past the ceiling is refused too_large \
         naming nothing; it answered {up:?}"
    );
    head_unmoved(
        s,
        before,
        "the refused far write of a writer not shown the set",
    )
    .await?;
    let source = SourceId::from("crm");
    let report = s
        .purge_preflight(&source, None)
        .await
        .must("the preflight")?;
    ensure!(
        (report.set_off, report.max_set_changes) == (2, 1),
        "the purge's preflight reports 2 changes set off beside a ceiling of 1; it reported \
         {} of {}",
        report.set_off,
        report.max_set_changes
    );
    let purged = s.purge_source(&source, None, None).await;
    ensure!(
        matches!(
            refused_on(&purged),
            Some(("graded", Refusal::TooLarge { max: 1 }))
        ),
        "the purge is refused too_large naming `graded`; it answered {purged:?}"
    );
    head_unmoved(s, before, "the refused purge").await?;
    let today = Timestamp::from_date(2026, 3, 1).expect("a date");
    let aged = s.age(today).await.must("aging")?;
    let retier = aged.rules.iter().find(|r| r.rule == "retier");
    ensure!(
        retier.is_some_and(|r| r.moved == 0
            && r.failed_guards == 0
            && r.refused == 1
            && matches!(r.refusals.as_slice(),
                [m] if m.key.as_str() == "c2" && matches!(m.why, Refusal::TooLarge { max: 1 }))),
        "the aging move of `c2`, past the ceiling, is refused `too_large`, listed with its key, \
         and no failed guard; it reported {aged:?}"
    );
    head_unmoved(s, before, "the aging run that moved nothing").await?;
    // A preflight reads: on a handle that holds no lease it answers the
    // same and appends nothing.
    let none: crate::conformance::Lines = Box::new(|_, _| (Vec::new(), 0));
    let lake = s.lake().clone();
    let c = (given.continued)(Some(store), lake, none, OpenMode::Read, Seams::default());
    let c = c
        .await
        .map_err(|e| Failure::new(format!("opening the lake read-only: {e}")))?;
    let ro = c
        .opened
        .map_err(|e| Failure::new(format!("the read-only open: {e}")))?;
    let at = ro.head().await.must("the head")?;
    let report = ro.purge_preflight(&source, None).await;
    ensure!(
        report.as_ref().is_ok_and(|r| r.set_off == 2),
        "the preflight on a read-only handle reports 2 changes; it answered {report:?}"
    );
    head_unmoved(ro.as_ref(), at, "the preflight on a read-only handle").await
}

async fn a_set_across_an_edge_is_judged_with_what_it_reads(store: Store, given: Given) -> Held {
    let backed = Seams {
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    };
    let store = (given.relimit)(store, backed, limits())
        .await
        .map_err(|e| Failure::new(format!("opening with the backings: {e}")))?;
    let (s, l) = (store.as_ref(), lake(&store).await?);
    graded(&l).await?;
    let echo = json!({"declare": "function", "name": "echo",
        "backing": crate::conformance::backing::KIND,
        "inputs": [{"name": "grade", "type": {"kind": "text"}}],
        "config": {"token": "set_across", "echo": "grade"}});
    l.declare_json(echo).await?;
    let scored = json!({"declare": "type", "name": "scored", "fields": [
        {"name": "grade", "type": {"kind": "text"}},
        {"name": "vec", "type": {"kind": "text"}, "authority": "derived",
         "function": "echo", "depends_on": ["grade"]},
        {"name": "band", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "vec = 'x'"}]});
    l.declare_json(scored).await?;
    let on = type_decl("scoring", vec![reference("scored", "scored")]);
    l.declare(on).await?;
    let computed = json!({"code": "set_reads_computed", "field": "vec", "type": "scored"});
    let changed = json!({"code": "call_takes_field", "name": "changed"});
    let band = json!({"code": "set_reads_computed", "field": "band", "type": "scored"});
    for (d, case) in [
        (
            object_set_on("s", "scoring", "scored.vec is null"),
            &computed,
        ),
        (
            trigger_on("t", "scoring", Some("scored.vec is null")),
            &computed,
        ),
        (object_set_on("s", "scoring", "scored.band"), &band),
        (
            trigger_on("t", "invoice", Some("changed(customer.grade)")),
            &changed,
        ),
    ] {
        let what = format!("declaring {d:?}");
        refused_like(&what, l.cat.declare(&l.own, d).await, case)?;
    }
    // A far field renamed under `graded` leaves it nothing to read.
    let mut renamed = type_decl(
        "customer",
        vec![
            field("rank", StorageKind::Text),
            field("note", StorageKind::Text),
        ],
    );
    renamed.fields.push(reference("region", "region"));
    let mut m = migration(renamed);
    m.renames = vec![("grade".into(), "rank".into())];
    let r = l.cat.migrate(&l.own, m).await;
    let names = format!("{r:?}");
    ensure!(
        r.is_err() && names.contains("graded"),
        "a migration renaming `grade` under `graded` is refused naming the set; it answered \
         {r:?}"
    );
    // A far derived field computed otherwise moves the referrers.
    let top = json!({"declare": "type", "name": "customer", "fields": [
        {"name": "grade", "type": {"kind": "text"}}, {"name": "note", "type": {"kind": "text"}},
        {"name": "region", "type": {"kind": "reference", "target": "region"}},
        {"name": "top", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "grade = 'x'"}]});
    l.declare_json(top.clone()).await?;
    l.declare_as(object_set_on("topped", "invoice", "customer.top"), "topped")
        .await?;
    let mut again = top.clone();
    again["fields"][3]["derived"] = json!("grade = 'y'");
    let at = next(s).await?;
    l.declare_json(again).await?;
    let got = moves(s, at).await?;
    let want = rows(&[
        ("topped", "i1", false),
        ("topped", "i2", false),
        ("topped", "i3", true),
    ]);
    ensure!(
        got == want,
        "`top` computed otherwise moves the invoices in its declaration's append, {want:?}; \
         it moved {got:?}"
    );
    // A value renamed under a far field moves the referrers in the
    // migration's append.
    let tone = |one_of: &[&str]| -> lakelands_api::ontology::ValueTypeDecl {
        serde_json::from_value(json!({"name": "tone", "kind": "text", "one_of": one_of}))
            .expect("a value type")
    };
    let declared = Declaration::ValueType(tone(&["rose", "blue"]));
    l.declare_as(declared, "declaring `tone`").await?;
    let mut tint = field("tone", StorageKind::Text);
    tint.ty.value_type = Some("tone".into());
    l.declare(type_decl("tint", vec![tint])).await?;
    l.declare(type_decl("paint", vec![reference("tint", "tint")]))
        .await?;
    l.upsert("tint", "t1", json!({"tone": "rose"})).await?;
    l.upsert("paint", "p1", json!({"tint": "t1"})).await?;
    l.declare_as(object_set_on("warm", "paint", "tint.tone = 'red'"), "warm")
        .await?;
    let m = lakelands_api::write::ValueTypeMigration {
        to: tone(&["red", "blue"]),
        renames: vec![("rose".into(), "red".into())],
    };
    let report = l.cat.migrate_value_type(&l.own, m.clone()).await;
    let report = report.must("the rename's preflight")?.data;
    let at = next(s).await?;
    let applied = l.cat.migrate_value_type_apply(&l.own, m, &report.report_id);
    applied.await.must("the rename")?;
    let got = moves(s, at).await?;
    // `customer` declared again so that `top`, which `topped` reads across,
    // derives from a field computed outside the lake: refused, as its
    // migration would be.
    let mut backed = top;
    backed["fields"][3]["derived"] = json!("vec = 'x'");
    let fields = backed["fields"].as_array_mut().expect("fields");
    fields.push(
        json!({"name": "vec", "type": {"kind": "text"}, "authority": "derived",
        "function": "echo", "depends_on": ["grade"]}),
    );
    let d = serde_json::from_value(backed).expect("a type");
    let refused = json!({"code": "set_reads_computed", "field": "top", "type": "customer"});
    let declared = l.cat.declare(&l.own, d).await;
    refused_like(
        "`customer` declared with `top` from `vec`",
        declared,
        &refused,
    )?;
    ensure!(
        got == rows(&[("warm", "p1", true)]),
        "`rose` renamed `red` takes `p1` into `warm` in the migration's append; it moved \
         {got:?}"
    );
    Ok(())
}

/// A reader of every field, holding `restricted` where `restricted`.
fn reader(store: &dyn LakeStore, id: &str, restricted: bool) -> Principal {
    let grants = Grants {
        default: Access::Read,
        restricted,
        ..Grants::default()
    };
    principal(store, PrincipalKind::Agent, id, grants)
}

/// What `who` is answered of `graded`: whether `describe` lists it and
/// `hot`, and what a read of `in_set('graded')` answers.
async fn answered(l: &Lake, who: &Principal) -> Result<(bool, bool, String), Failure> {
    let d = l
        .cat
        .describe(who, Default::default())
        .await
        .must("describe")?;
    let set = d.data.object_sets.iter().any(|s| s.name == "graded");
    let trigger = d.data.triggers.iter().any(|t| t.name == "hot");
    let ty = TypeName::from("invoice");
    let read = l.cat.find(who, &ty, find_where("in_set('graded')")).await;
    let read = match read {
        Ok(_) => "read".to_owned(),
        Err(e) => format!("{e:?}"),
    };
    Ok((set, trigger, read))
}

async fn a_set_across_an_edge_is_a_readers_only_where_nothing_far_is_held(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    graded(&l).await?;
    let plain = reads_all(s, "plain");
    let undeclared = json!({"code": "set_undeclared", "set": "graded"});
    let was = answered(&l, &plain).await?;
    let invoice = TypeName::from("invoice");
    let read = l
        .cat
        .find(&plain, &invoice, find_where("in_set('graded')"))
        .await;
    refused_like(
        "`in_set('graded')` to a reader without `restricted`",
        read,
        &undeclared,
    )?;
    restrict(s, "customer", "c1", "a hold").await?;
    let held = answered(&l, &plain).await?;
    ensure!(
        !was.0 && !was.1 && was == held,
        "to a reader without `restricted` neither `graded` nor `hot` is described, and a \
         customer held changes nothing it is answered; it was answered {was:?} and {held:?}"
    );
    // Holding `restricted`, an `only` on the far type or a far field kept
    // from it holds the set from it alike.
    let held = |id: &str, grants: lakelands_api::principal::TypeGrants| {
        let mut who = reader(s, id, true);
        who.grants.types.insert("customer".into(), grants);
        who
    };
    let only = held(
        "only",
        lakelands_api::principal::TypeGrants {
            only: [("note".into(), json!("n"))].into(),
            ..Default::default()
        },
    );
    let kept = held(
        "kept",
        lakelands_api::principal::TypeGrants {
            fields: [("grade".into(), Access::None)].into(),
            ..Default::default()
        },
    );
    for who in [&only, &kept] {
        let got = answered(&l, who).await?;
        let read = l
            .cat
            .find(who, &invoice, find_where("in_set('graded')"))
            .await;
        refused_like(
            &format!("`in_set('graded')` to `{}`", who.id),
            read,
            &undeclared,
        )?;
        ensure!(
            !got.0 && !got.1,
            "to `{}` neither `graded` nor `hot` is described; it was answered {got:?}",
            who.id
        );
    }
    // A writer is told a set moved, or a trigger fired, at another record
    // only where it is answered one of the records moved.
    let mut closed = reader(s, "closed", true);
    closed.grants.default = Access::Write;
    let only = lakelands_api::principal::TypeGrants {
        only: [("status".into(), json!("closed"))].into(),
        ..Default::default()
    };
    closed.grants.types.insert("invoice".into(), only);
    let (ty, c2) = (
        TypeName::from("customer"),
        lakelands_api::RecordKey::from("c2"),
    );
    let up = l.cat.upsert(
        &closed,
        &ty,
        &c2,
        patch(json!({"grade": "x"})),
        WriteOpts::default(),
    );
    let up = up.await.must("`c2` graded `x` by `closed`")?;
    let told = matches!(&up, Outcome::Applied { set_off, .. } if set_off.names_any());
    ensure!(
        !told,
        "`closed`, answered no open invoice, is told nothing of `graded` or `hot` at `c2`'s \
         write; it was answered {up:?}"
    );
    let (ty, r1) = (
        TypeName::from("region"),
        lakelands_api::RecordKey::from("r1"),
    );
    let up = l.cat.upsert(
        &closed,
        &ty,
        &r1,
        patch(json!({"name": "west"})),
        WriteOpts::default(),
    );
    let up = up.await.must("`r1` renamed by `closed`")?;
    let told = matches!(&up, Outcome::Applied { set_off, .. } if set_off.left == ["north"]);
    ensure!(
        told,
        "`closed` is told `north` left at `r1`'s write, which moves its closed `i2`; it was \
         answered {up:?}"
    );
    let full = reader(s, "full", true);
    let (set, trigger, _) = answered(&l, &full).await?;
    ensure!(
        set && trigger,
        "a reader holding `restricted` and every field is described `graded` and `hot`"
    );
    Ok(())
}

async fn a_set_across_a_reference_to_its_own_type_steps_back_by_the_path(store: Store) -> Held {
    let (s, l, text) = (store.as_ref(), Lake::of(store.as_ref()), StorageKind::Text);
    let employee = vec![field("grade", text), reference("manager", "employee")];
    l.declare(type_decl("employee", employee)).await?;
    for (key, fields) in [
        ("boss", json!({"grade": "x"})),
        ("e1", json!({"grade": "y", "manager": "boss"})),
        ("e2", json!({"grade": "y", "manager": "e1"})),
    ] {
        l.upsert("employee", key, fields).await?;
    }
    l.upsert("employee", "own", json!({"grade": "y"})).await?;
    l.upsert("employee", "own", json!({"manager": "own"}))
        .await?;
    let at = next(s).await?;
    let under = object_set_on("under_x", "employee", "manager.grade = 'x'");
    l.declare_as(under, "under_x").await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("under_x", "e1", true)]),
        "declared, `under_x` joins `e1`, under `boss`; it joined {got:?}"
    );
    for (key, grade, want) in [
        ("e1", "x", rows(&[("under_x", "e2", true)])),
        ("boss", "y", rows(&[("under_x", "e1", false)])),
        ("own", "x", rows(&[("under_x", "own", true)])),
    ] {
        let at = next(s).await?;
        l.upsert("employee", key, json!({"grade": grade})).await?;
        let got = moves(s, at).await?;
        ensure!(
            got == want,
            "`{key}` graded `{grade}` moves {want:?}; it moved {got:?}"
        );
    }
    let customer = vec![field("grade", text), reference("parent", "customer")];
    l.declare(type_decl("customer", customer)).await?;
    l.declare(type_decl(
        "invoice",
        vec![reference("customer", "customer")],
    ))
    .await?;
    l.upsert("customer", "p", json!({"grade": "y"})).await?;
    l.upsert("customer", "c", json!({"grade": "y", "parent": "p"}))
        .await?;
    for (key, customer) in [("of_child", "c"), ("of_parent", "p")] {
        l.upsert("invoice", key, json!({"customer": customer}))
            .await?;
    }
    let parented = object_set_on("parented", "invoice", "customer.parent.grade = 'x'");
    l.declare_as(parented, "parented").await?;
    let at = next(s).await?;
    l.upsert("customer", "p", json!({"grade": "x"})).await?;
    let got = moves(s, at).await?;
    ensure!(
        got == rows(&[("parented", "of_child", true)]),
        "`p` graded `x` takes its child's invoice into `parented` and not its own; it moved \
         {got:?}"
    );
    Ok(())
}

/// The keys `find` answers `l`'s owner of `invoice` under `filter`, at
/// `as_of`, and the total it counts.
async fn found(
    l: &Lake,
    filter: &str,
    as_of: Option<Position>,
) -> Result<(Vec<String>, Option<u64>), Failure> {
    let find = lakelands_api::Find {
        filter: Some(filter.into()),
        total: true,
        ..find_at(as_of)
    };
    let ty = TypeName::from("invoice");
    let reply = l.cat.find(&l.own, &ty, find).await;
    match reply.must(&format!("finding `{filter}`"))?.data {
        lakelands_api::Found::Records { rows, total, .. } => {
            Ok((rows.into_iter().map(|r| r.key.to_string()).collect(), total))
        }
        other => Err(Failure::new(format!("`{filter}` answered {other:?}"))),
    }
}

async fn a_reads_in_set_of_a_set_across_an_edge_reads_its_kept_members(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    graded(&l).await?;
    let named = object_set_on(
        "open_graded",
        "invoice",
        "in_set('graded') or status = 'none'",
    );
    l.declare_as(named, "open_graded").await?;
    let before = s.head().await.must("the head")?;
    l.upsert("customer", "c2", json!({"grade": "x"})).await?;
    let keys = |k: &[&str]| k.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
    for (filter, as_of, want) in [
        ("in_set('graded')", None, keys(&["i1", "i3"])),
        ("in_set('graded')", Some(before), keys(&["i1"])),
        ("in_set('graded') and customer = 'c2'", None, keys(&["i3"])),
        (
            "not in_set('graded') and status = 'open'",
            None,
            keys(&["i4"]),
        ),
        ("in_set('open_graded')", None, keys(&["i1", "i3"])),
        ("in_set('north')", None, keys(&["i1", "i2"])),
    ] {
        let (got, total) = found(&l, filter, as_of).await?;
        let n = Some(want.len() as u64);
        ensure!(
            got == want && total == n,
            "`{filter}` at {as_of:?} answers {want:?}, {n:?} in all; it answered {got:?}, \
             {total:?}"
        );
    }
    restrict(s, "customer", "c2", "a hold").await?;
    let (got, _) = found(&l, "in_set('graded')", None).await?;
    ensure!(
        got == keys(&["i1", "i3"]),
        "a hold on `c2` changes nothing the set answers; it answered {got:?}"
    );
    Ok(())
}

async fn a_sets_where_that_reads_the_clock_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let task = [
        field("owner", StorageKind::Text),
        field("due_on", StorageKind::Date),
    ];
    l.declare(type_decl("task", task.to_vec())).await?;
    l.declare_json(json!({"declare": "type", "name": "load", "key": ["owner"],
        "fields": [{"name": "owner", "type": {"kind": "text"}},
                   {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "task", "group_by": ["owner"],
                 "measures": [{"name": "n", "expr": "count()"}]}}))
        .await?;
    l.set("view_kept.load", json!(true)).await?;
    for (of, filter, call) in [
        ("task", "due_on < now()", "now()"),
        ("task", "due_on < today() - 1d", "today()"),
        ("load", "n > 1 and now() is null", "now()"),
        ("load", "n > 1 or today() is null", "today()"),
    ] {
        let case = json!({"code": "set_reads_clock", "set": "late", "call": call});
        let what = format!("`{filter}` over `{of}`");
        refused_like(
            &what,
            l.cat
                .declare(&l.own, object_set_on("late", of, filter))
                .await,
            &case,
        )?;
    }
    l.declare_as(
        object_set_on("past", "task", "due_on < '2026-01-01'"),
        "a stored date",
    )
    .await?;
    Ok(())
}

async fn a_log_holding_a_set_that_reads_the_clock_opens_and_replays(
    store: Store,
    given: Given,
) -> Held {
    let task = [
        field("owner", StorageKind::Text),
        field("due_on", StorageKind::Date),
    ];
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("task", task.to_vec())).await?;
    l.upsert("task", "t1", json!({"owner": "a", "due_on": "2020-01-01"}))
        .await?;
    l.upsert("task", "t2", json!({"owner": "a", "due_on": "2999-01-01"}))
        .await?;
    let late = object_set_on("late", "task", "due_on < now()");
    let store =
        crate::conformance::fixture::declared_below(store, &given, vec![late.clone()]).await?;
    let lake = store.lake().clone();
    let members = |l: Lake| async move {
        let rows = l.all_found("task", find_where("in_set('late')")).await?;
        Ok::<_, Failure>(
            rows.into_iter()
                .map(|r| r.key.to_string())
                .collect::<Vec<_>>(),
        )
    };
    let opened = members(Lake::of(store.as_ref())).await?;
    let c = (given.continued)(
        Some(store),
        lake,
        Box::new(|_, _| (Vec::new(), 0)),
        OpenMode::Rebuild,
        Seams::default(),
    );
    let c = c
        .await
        .map_err(|e| Failure::new(format!("rebuilding the lake: {e}")))?;
    let store = c
        .opened
        .map_err(|e| Failure::new(format!("the rebuild: {e}")))?;
    let l = Lake::of(store.as_ref());
    let rebuilt = members(Lake::of(store.as_ref())).await?;
    ensure!(
        opened == ["t1"] && rebuilt == opened,
        "a log holding a set that reads `now()` opens and rebuilds, and `in_set('late')` \
         answers the record due in 2020 alone after each; it answered {opened:?} and \
         {rebuilt:?}"
    );
    l.upsert("task", "t3", json!({"owner": "b", "due_on": "2021-01-01"}))
        .await?;
    refused_like(
        "the set declared again",
        l.cat.declare(&l.own, late).await,
        &json!({"code": "set_reads_clock", "set": "late", "call": "now()"}),
    )
}
