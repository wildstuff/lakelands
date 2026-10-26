//! The whole-graph measures of the pattern query: degree centrality and
//! connected components, over the links a walk would follow, in one order,
//! bounded by the lake's `max_graph_nodes`
//! (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use lakelands_api::Value;
use lakelands_api::query::Depth;
use lakelands_api::write::WriteOpts;

use crate::conformance::fixture::{doors, order, owner, reference, source_record, step};
use crate::conformance::prelude::*;

check!(
    DEGREE_COUNTS = degree_counts_each_records_links_to_ends_it_may_see_highest_first,
    "026, 029 and 035: `degree` counts each start record's links over the steps' relations in each step's direction, a via reference and a link record alike, but not a link to a retired end or, without the grant, a restricted link; highest first, ties by key, paged by limit; a start by key or by `where` counts those records only, and a key no record has counts none"
);

check!(
    COMPONENTS_PARTITION = components_partition_what_the_start_reaches_largest_first,
    "026 and 029: `components` puts every record reached from the start over the steps' relations, both ways, in exactly one component, across types; components largest first, ties by their least (type, key), members by (type, key); a start by key or by a search reaches from those records only"
);

check!(
    COMPONENTS_ANY_PAGE = components_are_one_partition_however_their_seeds_fall_into_pages,
    "026 and 029: `components` over a lake of 1 200 people and 400 companies, many unlinked, whose seeds reach each other within a page of seeds and across pages and where a retired company parts what it joined, answers from either start type exactly the partition the live links make, numbered largest first, then by the least (type, key), over pages of 1 000 (lakelands#1004)"
);

check!(
    COMPONENTS_NO_START = components_with_no_start_type_grow_from_every_linked_record,
    "018 and 024: `components` with no start type grows from every record that holds a link it may see over the steps' relations, in every type it is shown, carried by a reference field or a link record alike: no component of one but a record linked to itself, nothing for a record whose links reach only a retired or hidden record, two components where only a restricted link joins them, the typed answer unchanged beside it, `max_graph_nodes` counting linked records only, and a step naming an inverse walking every relation that bears it; any other projection, and a start `key`, `where`, `search` or `params`, refused `start_no_type`, and a relation no type it is shown declares `no_relation_anywhere`, a hidden one byte for byte as an absent one. At a valid time every type a step meets is probed before a record is read, as a typed walk probes it, so a type met only as a target with its valid time masked is refused alike and never told by a join (lakelands#992)",
    given
);

check!(
    MEASURES_AT_THEIR_POSITION = a_measure_reads_the_graph_at_the_position_it_is_asked_at,
    "009 and 026: a measure `as_of` an earlier position counts the links live there, not a link written later"
);

check!(
    GRAPH_TOO_LARGE = components_past_the_lakes_max_graph_nodes_are_refused_too_large,
    "018 and the location's limits: `components` whose records, over all components, would number more than `max_graph_nodes` are refused as too_large naming the ceiling, and nothing partial is answered",
    given
);

check!(
    MEASURE_IN_ONE_PAGE = a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes,
    "026, extended, and the location's limits: a measure's `limit` may go up to the lake's `max_graph_nodes` where that is above `max_limit`, so one page answers the rows its small pages answer in turn, with no cursor; above that bound, and above `max_limit` for a `rows` projection, a `limit` is refused as invalid naming the bound",
    given
);

check!(
    MEASURE_REFUSALS = a_measure_refuses_depth_where_to_order_by_and_one_way_components,
    "018: a measure refuses a step `depth`, `where` or `type`, `to`, `order_by`, no step at all, and a `components` step that does not walk `both`, each as invalid in its own words naming the measure, rather than answer less than it seems to, `degree`, `components` and `page_rank` alike (lakelands#924); a step's `join`, `optional`, `absent`, and a `from` that names another record than the start, are refused naming the member and the measure, which reads the steps' relations around its records and binds no name"
);

/// Partners p1..p3; invoices i0..i5 (i0 linked to nothing); `billed_to` by the reference field
/// `partner`, and `approved_by` carried by link records.
///
/// - billed_to: i1→p1, i2→p1, i3→p2, i5→p3 (p3 retired);
/// - approved_by: i2→p1, i4→p1 (restricted), then i1→p2, written last.
///
/// Answers the position before the last link.
async fn lake(store: &dyn LakeStore) -> Result<Position, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let mut invoice: TypeDecl = type_decl(
        "invoice",
        vec![searchable("note"), reference("partner", "partner")],
    );
    invoice.relations = vec![
        RelationDecl {
            name: "billed_to".into(),
            target: "partner".into(),
            cardinality: Cardinality::ManyToOne,
            via: Some("partner".into()),
            ..Default::default()
        },
        RelationDecl {
            name: "approved_by".into(),
            target: "partner".into(),
            cardinality: Cardinality::ManyToMany,
            ..Default::default()
        },
    ];
    l.declare(invoice).await?;
    for p in ["p1", "p2", "p3"] {
        l.upsert("partner", p, json!({"name": p})).await?;
    }
    for (i, p) in [
        ("i0", None),
        ("i1", Some("p1")),
        ("i2", Some("p1")),
        ("i3", Some("p2")),
        ("i4", None),
        ("i5", Some("p3")),
    ] {
        let fields = p.map_or_else(|| json!({"note": i}), |p| json!({"note": i, "partner": p}));
        l.upsert("invoice", i, fields).await?;
    }
    l.retire("partner", "p3").await?;
    for (from, to) in [("i2", "p1"), ("i4", "p1")] {
        link(&l, from, to).await?;
    }
    let link_key = lakelands_api::key::link_key(&"i4".into(), &"p1".into());
    restrict(store, "sys.link.invoice.approved_by", link_key, "hold").await?;
    let before = store.head().await.must("reading the head")?;
    link(&l, "i1", "p2").await?;
    Ok(before)
}

async fn link(l: &Lake, from: &str, to: &str) -> Held {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    cat.link(
        who,
        &"invoice".into(),
        &from.into(),
        &"approved_by".into(),
        &to.into(),
        patch(json!({})),
        WriteOpts::default(),
    )
    .await
    .must(&format!("linking {from} to {to}"))?;
    Ok(())
}

/// A measure over all the records of `ty`.
fn measure(ty: &str, steps: Vec<Step>, project: Projection) -> Pattern {
    pattern(ty, None, steps, project)
}

/// A measure over all the records of `ty`, walking one relation.
fn one(ty: &str, relation: &str, walk: Walk, project: Projection) -> Pattern {
    measure(ty, vec![step(relation, walk)], project)
}

fn both() -> Vec<Step> {
    vec![
        step("billed_to", Walk::Both),
        step("approved_by", Walk::Both),
    ]
}

/// The rows a measure answers, each as `column=value` joined by spaces,
/// and the cursor to the next page.
async fn rows(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<(Vec<String>, Option<Cursor>), Failure> {
    let reply = cat.query(who, p).await.must(what)?;
    let QueryResult::Rows { rows, next_cursor } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    Ok((rows.iter().map(show).collect(), next_cursor))
}

fn show(row: &BTreeMap<String, Value>) -> String {
    let cell = |c: &str| match row.get(c) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    if row.contains_key("degree") {
        format!("{}:{}={}", cell("type"), cell("key"), cell("degree"))
    } else {
        format!(
            "{}/{} {}:{}",
            cell("component"),
            cell("size"),
            cell("type"),
            cell("key")
        )
    }
}

/// The source that writes the paged lake.
const PAGED: &str = "paged";

/// 1 200 people `p0000`… and 400 companies `c000`…: every third person
/// works at company `i % 200`, so the two who share one are 600 apart, in
/// different pages of seeds; every seventh knows the person before it, in
/// the same page; every tenth company is owned by the one before it.
/// Company `c100` is retired once written. Answers the live records and
/// the live links between them.
async fn paged_lake(
    store: &dyn LakeStore,
) -> Result<(Vec<String>, Vec<(String, String)>), Failure> {
    let l = Lake::of(store);
    let by_source = |name: &str, target: &str| FieldDecl {
        authority: Authority::Source,
        ..reference(name, target)
    };
    let name = || sourced("name", StorageKind::Text);
    let carried = |name: &str, target: &str, via: &str| RelationDecl {
        name: name.into(),
        target: target.into(),
        cardinality: Cardinality::ManyToOne,
        via: Some(via.into()),
        ..Default::default()
    };
    let mut company = type_decl("company", vec![name(), by_source("owner", "company")]);
    company.relations = vec![carried("owns", "company", "owner")];
    l.declare(company).await?;
    let mut person = type_decl(
        "person",
        vec![
            name(),
            by_source("employer", "company"),
            by_source("friend", "person"),
        ],
    );
    person.relations = vec![
        carried("works_at", "company", "employer"),
        carried("knows", "person", "friend"),
    ];
    l.declare(person).await?;
    // Each record as `type:key`, and the links between them.
    let (mut records, mut nodes, mut links) = (Vec::new(), Vec::new(), Vec::new());
    let people = (0..1_200).map(|i| ("person", format!("p{i:04}"), i));
    for (ty, key, i) in (0..400)
        .map(|j| ("company", format!("c{j:03}"), j))
        .chain(people)
    {
        let mut fields = json!({"name": key});
        let to = match ty {
            "company" => vec![(
                "owner",
                i % 10 == 5,
                format!("company:c{:03}", i.max(1) - 1),
            )],
            _ => vec![
                ("employer", i % 3 == 0, format!("company:c{:03}", i % 200)),
                ("friend", i % 7 == 6, format!("person:p{:04}", i.max(1) - 1)),
            ],
        };
        for (field, _, to) in to.into_iter().filter(|(_, on, _)| *on) {
            fields[field] = json!(to.split_once(':').map(|(_, k)| k));
            links.push((format!("{ty}:{key}"), to));
        }
        nodes.push(format!("{ty}:{key}"));
        records.push(source_record(ty, &key, fields));
    }
    l.ingest(PAGED, records)
        .await
        .must("ingesting the paged lake")?;
    l.retire("company", "c100").await?;
    let live = |n: &String| n != "company:c100";
    nodes.retain(live);
    links.retain(|(from, to)| live(from) && live(to));
    Ok((nodes, links))
}

/// The rows `components` from every live record of `start` answers over
/// `links`, worked out here: each component by a walk of its own, then
/// largest first, ties by their least (type, key), members by (type, key).
fn partition(links: &[(String, String)], start: &str, all: &[String]) -> Vec<String> {
    let mut near: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (a, b) in links {
        near.entry(a).or_default().push(b);
        near.entry(b).or_default().push(a);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut parts: Vec<Vec<String>> = Vec::new();
    for seed in all.iter().filter(|n| n.starts_with(&format!("{start}:"))) {
        if !seen.insert(seed.as_str()) {
            continue;
        }
        let (mut part, mut todo) = (vec![seed.clone()], vec![seed.as_str()]);
        while let Some(n) = todo.pop() {
            for &m in near.get(n).into_iter().flatten() {
                if seen.insert(m) {
                    part.push(m.to_owned());
                    todo.push(m);
                }
            }
        }
        part.sort();
        parts.push(part);
    }
    parts.sort_by(|a, b| (b.len(), &a[0]).cmp(&(a.len(), &b[0])));
    let mut rows = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        rows.extend(part.iter().map(|n| format!("{i}/{} {n}", part.len())));
    }
    rows
}

async fn components_are_one_partition_however_their_seeds_fall_into_pages(store: Store) -> Held {
    let (all, links) = paged_lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let steps = || {
        let walk = |r: &str| step(r, Walk::Both);
        vec![walk("works_at"), walk("owns"), walk("knows")]
    };
    for start in ["person", "company"] {
        let (mut got, mut cursor, mut pages) = (Vec::new(), None, 0);
        loop {
            let p = Pattern {
                limit: Some(1_000),
                cursor,
                ..measure(start, steps(), Projection::Components)
            };
            let (page, next) =
                rows(l.cat.as_ref(), &l.own, p, "the paged lake's components").await?;
            got.extend(page);
            pages += 1;
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        let want = partition(&links, start, &all);
        let first = got.iter().zip(&want).position(|(g, w)| g != w);
        ensure!(
            got == want,
            "`components` from every {start} answers the partition the live links make, \
             {} rows in order; it answered {} rows over {pages} pages, first apart at row \
             {first:?}: {:?} where {:?} was due",
            want.len(),
            got.len(),
            first.and_then(|i| got.get(i)),
            first.and_then(|i| want.get(i))
        );
    }
    Ok(())
}

/// Companies co1..co8, people p1..p8 and a note n1; `works_at`, `owns`
/// and `tagged` by reference fields, `knows` by link records, and `staff`
/// the inverse of both `works_at` and `knows`:
///
/// - p1 works at co1; co3 names co2 its `owner`; n1 and nobody else is
///   tagged co4; p5 works at co5, retired; p7 at co7; p8 at co8,
///   restricted;
/// - p3 knows p2, p6 knows itself, and p7 knows p2 by a restricted link;
/// - p4 is linked to nothing;
/// - slot s1 is `posted` to o1, a `post` valid from its `opens`.
async fn unstarted_lake(store: &dyn LakeStore) -> Held {
    let l = Lake::of(store);
    let to = |name: &str, target: &str, via: Option<&str>| RelationDecl {
        name: name.into(),
        target: target.into(),
        cardinality: Cardinality::ManyToOne,
        via: via.map(Into::into),
        ..Default::default()
    };
    let name = || field("name", StorageKind::Text);
    let mut company = type_decl("company", vec![name(), reference("owner", "company")]);
    company.relations = vec![to("owns", "company", Some("owner"))];
    l.declare(company).await?;
    let refs = ["employer", "tag"].map(|f| reference(f, "company"));
    let mut person = type_decl("person", vec![name()]);
    person.fields.extend(refs);
    let staff = Some("staff".into());
    let knows = RelationDecl {
        cardinality: Cardinality::ManyToMany,
        inverse: staff.clone(),
        ..to("knows", "person", None)
    };
    let tagged = to("tagged", "company", Some("tag"));
    person.relations = vec![
        RelationDecl {
            inverse: staff,
            ..to("works_at", "company", Some("employer"))
        },
        knows,
        tagged.clone(),
    ];
    l.declare(person).await?;
    let mut note = type_decl("note", vec![reference("tag", "company")]);
    note.relations = vec![RelationDecl {
        inverse: Some("notes".into()),
        ..tagged
    }];
    l.declare(note).await?;
    for (c, owner) in [("co1", None), ("co2", None), ("co3", Some("co2"))]
        .into_iter()
        .chain(["co4", "co5", "co6", "co7", "co8"].map(|c| (c, None)))
    {
        l.upsert(
            "company",
            c,
            owner.map_or(json!({"name": c}), |o| json!({"owner": o})),
        )
        .await?;
    }
    for (p, employer) in [("p1", "co1"), ("p5", "co5"), ("p7", "co7"), ("p8", "co8")] {
        l.upsert("person", p, json!({"employer": employer})).await?;
    }
    for p in ["p2", "p3", "p4", "p6"] {
        l.upsert("person", p, json!({"name": p})).await?;
    }
    l.upsert("note", "n1", json!({"tag": "co4"})).await?;
    let mut post = type_decl("post", vec![field("opens", StorageKind::DateTime)]);
    post.valid_from = Some("opens".into());
    l.declare(post).await?;
    // `bot` holds `works_at` too, valid from its `since`, but not `staff`.
    let mut bot = type_decl("bot", vec![field("since", StorageKind::DateTime)]);
    bot.fields.push(reference("employer", "company"));
    bot.valid_from = Some("since".into());
    bot.relations = vec![to("works_at", "company", Some("employer"))];
    l.declare(bot).await?;
    let mut slot = type_decl("slot", vec![reference("post", "post")]);
    slot.relations = vec![to("posted", "post", Some("post"))];
    l.declare(slot).await?;
    l.upsert("post", "o1", json!({"opens": "2026-03-01T00:00:00Z"}))
        .await?;
    l.upsert("slot", "s1", json!({"post": "o1"})).await?;
    l.retire("company", "co5").await?;
    restrict(store, "company", "co8", "hold").await?;
    for (from, to) in [("p3", "p2"), ("p6", "p6"), ("p7", "p2")] {
        let (cat, who) = (l.cat.as_ref(), &l.own);
        let (ty, rel) = (&"person".into(), &"knows".into());
        cat.link(
            who,
            ty,
            &from.into(),
            rel,
            &to.into(),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await
        .must(&format!("linking {from} to {to}"))?;
    }
    let link_key = lakelands_api::key::link_key(&"p7".into(), &"p2".into());
    restrict(store, "sys.link.person.knows", link_key, "hold").await?;
    Ok(())
}

async fn components_with_no_start_type_grow_from_every_linked_record(
    store: Store,
    given: Given,
) -> Held {
    unstarted_lake(store.as_ref()).await?;
    let unstarted = |extra: serde_json::Value| -> Result<Pattern, Failure> {
        let walk = |r: &str| json!({"relation": r, "walk": "both"});
        let steps = ["works_at", "owns", "knows", "tagged"].map(walk);
        let mut p = json!({"start": {}, "project": {"as": "components"}, "steps": steps});
        p.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().cloned().unwrap_or_default());
        serde_json::from_value(p)
            .map_err(|e| Failure::new(format!("`start: {{}}` is a pattern: {e}")))
    };
    let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
    let (got, _) = rows(
        cat.as_ref(),
        &own,
        unstarted(json!({}))?,
        "unstarted components",
    )
    .await?;
    let want = [
        "0/4 company:co7",
        "0/4 person:p2",
        "0/4 person:p3",
        "0/4 person:p7",
        "1/2 company:co1",
        "1/2 person:p1",
        "2/2 company:co2",
        "2/2 company:co3",
        "3/2 company:co4",
        "3/2 note:n1",
        "4/2 company:co8",
        "4/2 person:p8",
        "5/1 person:p6",
    ];
    ensure!(
        got == want,
        "with no start type the owner's components are every linked record's, across types and \
         carriers: p4 links nothing, p5 only a retired company, and p6 knows itself; it answered \
         {got:?}"
    );
    let mut staff = unstarted(json!({}))?;
    staff.steps = ["staff", "owns", "tagged"]
        .map(|r| step(r, Walk::Both))
        .to_vec();
    let (got, _) = rows(cat.as_ref(), &own, staff, "over `staff`").await?;
    ensure!(
        got == want,
        "`staff`, the inverse of `works_at` and of `knows`, walks both; it answered {got:?}"
    );
    // Without `restricted` and without `note`: p7 is parted from p2, p8's
    // one link reaches a restricted record, and n1 is neither seeded nor
    // walked to, so co4 holds no link either.
    let mut reader = reads_all(store.as_ref(), "reader");
    let none = lakelands_api::principal::TypeGrants {
        default: Some(lakelands_api::Access::None),
        ..Default::default()
    };
    reader.grants.types.insert("note".into(), none);
    let (got, _) = rows(cat.as_ref(), &reader, unstarted(json!({}))?, "a reader's").await?;
    let want = [
        "0/2 company:co1",
        "0/2 person:p1",
        "1/2 company:co2",
        "1/2 company:co3",
        "2/2 company:co7",
        "2/2 person:p7",
        "3/2 person:p2",
        "3/2 person:p3",
        "4/1 person:p6",
    ];
    ensure!(
        got == want,
        "a reader without `restricted` and not shown `note` is answered p7 apart from p2 and \
         nothing of p8, co8, n1 or co4; it answered {got:?}"
    );
    let mut typed = unstarted(json!({}))?;
    typed.start.ty = Some("person".into());
    let (got, _) = rows(cat.as_ref(), &own, typed, "components from every person").await?;
    let alone = ["3/1 person:p4", "4/1 person:p5", "5/1 person:p6"];
    ensure!(
        got.len() == 11 && got[8..] == alone,
        "from every person, each person is a start and stands alone where it links nothing it \
         may see, as before; it answered {got:?}"
    );
    for (extra, asks) in [
        (json!({"project": {"as": "rows"}}), "rows"),
        (json!({"project": {"as": "graph"}}), "graph"),
        (json!({"project": {"as": "degree"}}), "degree"),
        (json!({"start": {"key": "p1"}}), "key"),
        (json!({"start": {"where": "name = 'p2'"}}), "where"),
        (
            json!({"start": {"search": SearchMode::lexical("p")}}),
            "search",
        ),
        (json!({"start": {"params": {"x": 1}}}), "params"),
    ] {
        let what = format!("no start type with {extra}");
        let answer = cat.query(&own, unstarted(extra)?).await;
        refused_as!(&what, answer, Case::StartNoType { asks: a } if a == asks)?;
    }
    // `tagged` declared only by types the reader is not shown, and `notes`
    // the inverse only of one, are answered as the absent `nope` is, byte
    // for byte but the name.
    let mut answers = Vec::new();
    for relation in ["nope", "tagged", "notes"] {
        let mut p = unstarted(json!({}))?;
        p.steps = vec![step(relation, Walk::Both)];
        let mut who = reader.clone();
        who.grants
            .types
            .insert("person".into(), who.grants.types["note"].clone());
        let what =
            format!("no start type over `{relation}`, `tagged` declared only where not shown");
        let answer = cat.query(&who, p).await.map(|_| ());
        answers.extend(answer.clone().err());
        refused_as!(&what, answer, Case::NoRelationAnywhere { relation: r } if r.as_str() == relation)?;
    }
    for (hidden, name) in [(&answers[1], "tagged"), (&answers[2], "notes")] {
        let what = format!("`{name}`, hidden, beside the absent `nope`");
        alike_renamed(&what, hidden, &answers[0], ("nope", name))?;
    }
    // At a valid time a type met only as a target is probed as a typed
    // walk from `slot` probes it: `opens` masked is refused alike.
    let mut timed = reads_all(store.as_ref(), "timed");
    let masked = lakelands_api::principal::TypeGrants {
        fields: [("opens".into(), lakelands_api::Access::Masked)].into(),
        ..Default::default()
    };
    timed.grants.types.insert("post".into(), masked.clone());
    let since = [("since".into(), lakelands_api::Access::Masked)].into();
    timed.grants.types.insert(
        "bot".into(),
        lakelands_api::principal::TypeGrants {
            fields: since,
            ..masked
        },
    );
    let at = json!({"valid_at": "2026-06-01T00:00:00Z"});
    let mut posted = unstarted(at)?;
    posted.steps = vec![step("posted", Walk::Both)];
    let mut from_slot = posted.clone();
    from_slot.start.ty = Some("slot".into());
    let (untyped, typed) = (
        cat.query(&timed, posted).await,
        cat.query(&timed, from_slot).await,
    );
    ensure!(
        typed.is_err() && reason(&untyped) == reason(&typed),
        "at a valid time over `posted`, with `opens` masked, no start type is refused as a start \
         on `slot` is; they answered {untyped:?} and {typed:?}"
    );
    // `staff` walks only its two relations' owner, `person`: `bot`, which
    // holds a `works_at` of its own, is neither seeded nor probed.
    let mut staff = unstarted(json!({"valid_at": "2026-06-01T00:00:00Z"}))?;
    staff.steps = vec![step("staff", Walk::Both)];
    let mut untimed = staff.clone();
    untimed.at = Default::default();
    let (at, now) = (
        cat.query(&timed, staff).await,
        cat.query(&timed, untimed).await,
    );
    ensure!(
        at.as_ref().ok().map(|r| &r.data) == now.as_ref().ok().map(|r| &r.data) && at.is_ok(),
        "`staff` at a valid time is answered as without one, `bot.since` masked; they \
         answered {at:?} and {now:?}"
    );
    // Thirteen linked records, under a ceiling of thirteen and of twelve.
    for (max, fits) in [(13, true), (12, false)] {
        let limits = Limits {
            max_graph_nodes: max,
            ..crate::conformance::fixture::limits()
        };
        let reopened = (given.relimit)(store.clone(), Seams::default(), limits).await;
        let reopened = reopened.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
        let (cat, own) = (doors(reopened.as_ref()), owner(reopened.as_ref()));
        let answer = cat.query(&own, unstarted(json!({}))?).await;
        let what = format!("unstarted components under a ceiling of {max}");
        if fits {
            answer.must(&what)?;
        } else {
            let too_large =
                reason(&answer).is_some_and(|r| matches!(r, Refusal::TooLarge { max: 12 }));
            ensure!(
                too_large,
                "{what} reach thirteen linked records and are refused too_large; got {answer:?}"
            );
        }
    }
    Ok(())
}

async fn degree_counts_each_records_links_to_ends_it_may_see_highest_first(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let out = || {
        measure(
            "invoice",
            vec![step("billed_to", Walk::Out), step("approved_by", Walk::Out)],
            Projection::Degree,
        )
    };
    let (got, _) = rows(cat.as_ref(), own, out(), "the invoices' out-degree").await?;
    ensure!(
        got == [
            "invoice:i1=2",
            "invoice:i2=2",
            "invoice:i3=1",
            "invoice:i4=1",
            "invoice:i0=0",
            "invoice:i5=0"
        ],
        "the owner's out-degree over `billed_to` and `approved_by` is i1 2 (p1, p2), i2 2 \
         (p1 by each relation), i3 1, i4 1 (the restricted link), i5 0 (its partner is \
         retired) and i0 0, highest first and ties by key; it answered {got:?}"
    );
    let reader = reads_all(store.as_ref(), "reader");
    let (got, _) = rows(cat.as_ref(), &reader, out(), "a reader's out-degree").await?;
    ensure!(
        got == [
            "invoice:i1=2",
            "invoice:i2=2",
            "invoice:i3=1",
            "invoice:i0=0",
            "invoice:i4=0",
            "invoice:i5=0"
        ],
        "without the `restricted` grant the link i4 → p1 is not counted; it answered {got:?}"
    );
    let inn = one("partner", "billed_to", Walk::In, Projection::Degree);
    let (got, _) = rows(cat.as_ref(), own, inn, "the partners' in-degree").await?;
    ensure!(
        got == ["partner:p1=2", "partner:p2=1"],
        "the partners' in-degree over `billed_to` is p1 2, p2 1, and the retired p3 is no \
         start; it answered {got:?}"
    );
    let started = |key: Option<&str>, filter: Option<&str>| {
        let mut p = out();
        p.start.key = key.map(Into::into);
        p.start.filter = filter.map(Into::into);
        p
    };
    for (p, want, what) in [
        (
            started(Some("i1"), None),
            vec!["invoice:i1=2"],
            "started on i1",
        ),
        (
            started(Some("i9"), None),
            vec![],
            "started on a key no record has",
        ),
        (
            started(None, Some("note = 'i3' or note = 'i4'")),
            vec!["invoice:i3=1", "invoice:i4=1"],
            "started where `note` is i3 or i4",
        ),
    ] {
        let (got, _) = rows(cat.as_ref(), own, p, what).await?;
        ensure!(
            got == want,
            "the out-degree {what} holds its start's rows only, {want:?}; it answered {got:?}"
        );
    }
    let mut first = out();
    first.limit = Some(2);
    let (page, next_cursor) = rows(cat.as_ref(), own, first, "the first page of degrees").await?;
    ensure!(
        page == ["invoice:i1=2", "invoice:i2=2"] && next_cursor.is_some(),
        "a first page of two holds i1 and i2 and a cursor; it answered {page:?}"
    );
    let mut next = out();
    next.limit = Some(2);
    next.cursor = next_cursor;
    let (got, _) = rows(cat.as_ref(), own, next, "the second page of degrees").await?;
    ensure!(
        got == ["invoice:i3=1", "invoice:i4=1"],
        "the second page holds i3 and i4; it answered {got:?}"
    );
    Ok(())
}

async fn components_partition_what_the_start_reaches_largest_first(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let all = || measure("invoice", both(), Projection::Components);
    let (got, _) = rows(cat.as_ref(), own, all(), "the invoices' components").await?;
    let want = [
        "0/6 invoice:i1",
        "0/6 invoice:i2",
        "0/6 invoice:i3",
        "0/6 invoice:i4",
        "0/6 partner:p1",
        "0/6 partner:p2",
        "1/1 invoice:i0",
        "2/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "the invoices and the partners they reach form one component of six (i3 joins \
         through p2 and the link i1 → p2); i0 and i5 (p3 is retired) stand alone and \
         come after it, though i0 sorts first; it answered {got:?}"
    );
    let started = |key: Option<&str>, search: Option<&str>| {
        let mut p = all();
        p.start.key = key.map(Into::into);
        p.start.search = search.map(SearchMode::lexical);
        p
    };
    for (p, want, what) in [
        (
            started(Some("i0"), None),
            vec!["0/1 invoice:i0"],
            "started on i0",
        ),
        (
            started(Some("i3"), None),
            want[..6].to_vec(),
            "started on i3",
        ),
        (
            started(None, Some("i3")),
            want[..6].to_vec(),
            "started by a search for i3",
        ),
    ] {
        let (got, _) = rows(cat.as_ref(), own, p, what).await?;
        ensure!(
            got == want,
            "the components {what} are what its start reaches only, {want:?}; it answered \
             {got:?}"
        );
    }
    let reader = reads_all(store.as_ref(), "reader");
    let (got, _) = rows(cat.as_ref(), &reader, all(), "a reader's components").await?;
    let want = [
        "0/5 invoice:i1",
        "0/5 invoice:i2",
        "0/5 invoice:i3",
        "0/5 partner:p1",
        "0/5 partner:p2",
        "1/1 invoice:i0",
        "2/1 invoice:i4",
        "3/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "without the `restricted` grant i4 is alone, and the two single components come \
         by their least member; it answered {got:?}"
    );
    Ok(())
}

async fn a_measure_reads_the_graph_at_the_position_it_is_asked_at(store: Store) -> Held {
    let before = lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut then = measure("invoice", both(), Projection::Components);
    then.at.as_of = Some(before);
    let (got, _) = rows(
        cat.as_ref(),
        own,
        then,
        "the components before the last link",
    )
    .await?;
    let want = [
        "0/4 invoice:i1",
        "0/4 invoice:i2",
        "0/4 invoice:i4",
        "0/4 partner:p1",
        "1/2 invoice:i3",
        "1/2 partner:p2",
        "2/1 invoice:i0",
        "3/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "at position {before}, before the link i1 → p2, i3 and p2 are a component of \
         their own; it answered {got:?}"
    );
    let mut then = one("invoice", "approved_by", Walk::Out, Projection::Degree);
    then.at.as_of = Some(before);
    let (got, _) = rows(cat.as_ref(), own, then, "the degrees before the last link").await?;
    ensure!(
        got.first().map(String::as_str) == Some("invoice:i2=1")
            && got.contains(&"invoice:i1=0".to_owned()),
        "at position {before} i1 has no `approved_by` link yet; it answered {got:?}"
    );
    Ok(())
}

async fn components_past_the_lakes_max_graph_nodes_are_refused_too_large(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref()).await?;
    let limits = |max_graph_nodes| Limits {
        max_graph_nodes,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits(8));
    let store = store.await.map_err(|e| {
        Failure::new(format!(
            "opening the lake again with max_graph_nodes 7 failed: {e}"
        ))
    })?;
    let all = || measure("invoice", both(), Projection::Components);
    let (got, _) = rows(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        all(),
        "components of eight records under a cap of 8",
    )
    .await
    .map(|r| (r.0.len(), r.1))
    .map_err(|e| Failure::new(format!("{e}: the eight records fit a cap of 8")))?;
    ensure!(
        got == 8,
        "under a cap of 8 the components answer eight rows; they answered {got}"
    );
    let store = (given.relimit)(store, Seams::default(), limits(7));
    let store = store.await.map_err(|e| {
        Failure::new(format!(
            "opening the lake again with max_graph_nodes 6 failed: {e}"
        ))
    })?;
    let before = store.head().await.must("reading the head")?;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), all())
        .await
        .must_fail("components past a cap of 7")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::TooLarge { max: 7 })),
        "components that reach eight records past a cap of 7 are refused as too_large \
         naming 7; it failed with {err:?}"
    );
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "a measure past the cap")
        .await
}

async fn a_measure_refuses_depth_where_to_order_by_and_one_way_components(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let named = |p: &Projection| match p {
        Projection::Degree => "degree",
        Projection::Components => "components",
        _ => "page_rank",
    };
    let mut cases: Vec<(String, Pattern, Case)> = Vec::new();
    for project in [Projection::Degree, Projection::PageRank] {
        let m = named(&project);
        let base = || one("invoice", "billed_to", Walk::Out, project.clone());
        let measure = || m.to_owned();
        let mut p = base();
        p.steps[0].depth = Depth { min: 1, max: 2 };
        let words = Case::MeasureNoDepth { measure: measure() };
        cases.push((format!("{m} with a step depth"), p, words));
        let mut p = base();
        p.steps[0].filter = Some("name = 'p1'".into());
        let words = Case::MeasureNoWhere { measure: measure() };
        cases.push((format!("{m} with a step where"), p, words));
        let mut p = base();
        p.steps[0].ty = Some("partner".into());
        let words = Case::MeasureNoType { measure: measure() };
        cases.push((format!("{m} with a step `type`"), p, words));
        let mut p = base();
        p.to = Some(("partner".into(), "p1".into()));
        let words = Case::MeasureNoTo { measure: measure() };
        cases.push((format!("{m} with `to`"), p, words));
        let mut p = base();
        p.order_by = vec![order(m, Default::default(), Default::default())];
        let words = Case::MeasureNoOrder { measure: measure() };
        cases.push((format!("{m} with `order_by`"), p, words));
        let mut p = base();
        p.steps.clear();
        let words = Case::MeasureNoSteps { measure: measure() };
        cases.push((format!("{m} with no step"), p, words));
    }
    let mut p = measure("invoice", both(), Projection::Components);
    p.order_by = vec![order("components", Default::default(), Default::default())];
    let words = Case::MeasureNoOrder {
        measure: "components".into(),
    };
    cases.push(("components with `order_by`".into(), p, words));
    let one_way = one("invoice", "billed_to", Walk::Out, Projection::Components);
    let words = Case::ComponentsBothWays {
        relation: "billed_to".into(),
    };
    cases.push(("components walking one way".into(), one_way, words));
    // A pattern's tree members: a measure reads the steps' relations
    // around its records, and binds no name another step could join.
    for project in [
        Projection::Degree,
        Projection::Components,
        Projection::PageRank,
    ] {
        for member in ["join", "optional", "absent", "from"] {
            let mut p = measure("invoice", both(), project.clone());
            let s = &mut p.steps[1];
            match member {
                "join" => s.join = Some("other".into()),
                "optional" => s.optional = true,
                "absent" => s.absent = true,
                _ => s.from = Some("other".into()),
            }
            let words = Case::PatternTreeMember {
                member: member.into(),
                projection: named(&project).into(),
            };
            cases.push((
                format!("{} with a step's `{member}`", named(&project)),
                p,
                words,
            ));
        }
    }
    for (what, p, words) in cases {
        let err = cat.query(own, p);
        let err = err.await.must_fail(&what)?;
        ensure!(
            invalid_case(&err) == Some(&words),
            "{what} must be refused as invalid, `{words:?}`; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref()).await?;
    // Pages of two, and room for the eight records the components reach.
    let limits = Limits {
        default_limit: 2,
        max_limit: 2,
        max_graph_nodes: 8,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits);
    let store = store.await.map_err(|e| {
        Failure::new(format!(
            "opening the lake again with max_limit 2 failed: {e}"
        ))
    })?;
    let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
    let paged = |project: &Projection, limit, cursor| Pattern {
        limit: Some(limit),
        cursor,
        ..measure("invoice", both(), project.clone())
    };
    for project in [Projection::Components, Projection::Degree] {
        let one = paged(&project, 8, None);
        let (whole, next) = rows(cat.as_ref(), &own, one, "a measure in one page of 8").await?;
        let (mut turns, mut cursor) = (Vec::new(), None);
        loop {
            let two = paged(&project, 2, cursor);
            let (page, next) = rows(cat.as_ref(), &own, two, "a measure in pages of 2").await?;
            turns.extend(page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        ensure!(
            next.is_none() && whole.len() > 2 && whole == turns,
            "under `max_limit` 2 and `max_graph_nodes` 8, {project:?} with `limit` 8 must answer \
             in one page, without a cursor, the rows its pages of 2 answer in turn; it answered \
             {whole:?} (cursor: {}), where the pages answered {turns:?}",
            next.is_some()
        );
    }
    let rows_of_three = Pattern {
        limit: Some(3),
        ..one("invoice", "billed_to", Walk::Out, Projection::default())
    };
    for (p, max) in [
        (paged(&Projection::Components, 9, None), 8),
        (rows_of_three, 2),
    ] {
        let what = format!("{:?} with `limit` {:?}", p.project, p.limit);
        refused_as!(&what, cat.query(&own, p).await, Case::LimitRange { max: m } if *m == max)?;
    }
    Ok(())
}
