//! The two time axes of a read, and the provenance every field carries
//! (`docs/decisions/009-every-response-carries-its-position.md`,
//! `docs/decisions/010-provenance-per-field.md`, `docs/glossary.md`).

use lakelands_api::Get;
use lakelands_api::record::Origin;

use crate::conformance::fixture::{get_at, source_record, upsert};
use crate::conformance::prelude::*;

check!(
    VALID_TIME_IS_A_DATE_TIME = a_valid_time_names_a_stored_date_time_field,
    "028, extended (lakelands#714, lakelands#702): `valid_from` and `valid_to` each name a field of kind `date_time` or `date`, so a value that only reads as an instant never sets a record's validity; a field of any other kind is refused as invalid on it, by `declare`, `apply`, a proposal and a migration, a derived `date_time` field and a view's `date_time` field are valid times as any is, and a read at `valid_at` admits by them; a `date` valid time holds from its first day's midnight up to, not through, its last day's"
);

check!(
    TWO_TIME_AXES = valid_at_and_as_of_combine_and_neither_stands_in_for_the_other,
    "009 and the glossary: `valid_at` asks what was true in the world, `as_of` what the lake knew; a read may name both"
);

check!(
    FIELD_PROVENANCE = every_field_carries_its_own_provenance,
    "010: with meta, each field answers its origin, known_at, position and writer, and a value from a source also its source and seen_at; a record whose fields a source wrote in two ingests, an author in a third, and a mirror's ingest with its `upstream` answers each field's own provenance, a cleared field's too, at the head, as of an earlier position and after the projection is rebuilt, however a record keeps it (lakelands#1018)"
);

check!(
    BASE_PER_RECORD = each_record_reads_the_provenance_of_its_own_writes,
    "010, extended (lakelands#1432): records one append wrote share their provenance and those another wrote hold their own, whether three records of the append share it or one or two do: two sources' ingests, a source's second, of a record it wrote before and of a new one, and an author's write each answer, per field, their write's origin, source, position and writer, through `get`, `find` and a lexical search with meta alike, as of before the later writes and after the projection is rebuilt; once the first source is forgotten, the record another hand co-owns reads its field that is not erasable as that source wrote it and its erasable one as erased, the other source's record reads its own, and a rebuild answers the same",
    reopens
);

fn day(m: u32, d: u32) -> String {
    Timestamp::from_date(2026, m, d)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

async fn valid_at_and_as_of_combine_and_neither_stands_in_for_the_other(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut ty = type_decl(
        "contract",
        vec![
            field("party", StorageKind::Text),
            field("from", StorageKind::DateTime),
            field("to", StorageKind::DateTime),
        ],
    );
    ty.valid_from = Some("from".into());
    ty.valid_to = Some("to".into());
    l.declare(ty).await?;

    // Valid in January only, as first written; then extended to March.
    let first = l.upsert(
        "contract",
        "c",
        json!({"party": "acme", "from": day(1, 1), "to": day(2, 1)}),
    );
    let first = first.await?.position();
    l.upsert("contract", "c", json!({"to": day(4, 1)})).await?;

    let contract = TypeName::from("contract");
    let at = |as_of: Option<Position>, valid: String| ReadAt {
        as_of,
        valid_at: Timestamp::parse(&valid).ok(),
        clock: None,
    };
    let found = |as_of, valid: String| {
        let cat = cat.clone();
        let own = own.clone();
        let contract = contract.clone();
        async move {
            let got = cat.get(
                &own,
                &contract,
                Get {
                    key: "c".into(),
                    with_meta: false,
                    at: at(as_of, valid),
                    params: Default::default(),
                },
            );
            let got = got.await;
            match got {
                Ok(_) => Ok(true),
                Err(Error::NotFound {
                    missing: Missing::Record { .. },
                }) => Ok(false),
                Err(e) => Err(Failure::new(format!("reading `c` failed: {e}"))),
            }
        }
    };

    let cases = [
        (None, day(1, 15), true, "January, as the lake knows it now"),
        (None, day(3, 15), true, "March, as the lake knows it now"),
        (None, day(5, 1), false, "May, after the contract ends"),
        (
            Some(first),
            day(1, 15),
            true,
            "January, as the lake knew it first",
        ),
        (
            Some(first),
            day(3, 15),
            false,
            "March, as the lake knew it before the extension",
        ),
        (
            None,
            day(2, 1),
            true,
            "the instant the first version ended, now extended",
        ),
    ];
    for (as_of, valid, want, what) in cases {
        let got = found(as_of, valid.clone()).await?;
        ensure!(
            got == want,
            "reading `c` valid at {valid} (as_of {as_of:?}): {what}: expected {}, but \
             it was {}. `valid_at` selects what held in the world and `as_of` what the \
             lake knew; together they must answer the earlier knowledge about that \
             instant",
            if want { "found" } else { "absent" },
            if got { "found" } else { "absent" }
        );
    }

    let find = Find {
        at: at(Some(first), day(3, 15)),
        ..Find::default()
    };
    let rows = l.all_found("contract", find).await?;
    ensure!(
        rows.is_empty(),
        "`find` valid at March as of the first write answered {} record(s); the lake \
         then knew the contract as ending on 1 February",
        rows.len()
    );
    Ok(())
}

async fn every_field_carries_its_own_provenance(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let sourced = |name: &str, kind| FieldDecl {
        authority: Authority::Source,
        ..field(name, kind)
    };
    let mut fields = vec![sourced("amount", StorageKind::Decimal)];
    fields.extend(["ref", "memo", "x"].map(|n| sourced(n, StorageKind::Text)));
    fields.push(field("status", StorageKind::Text));
    l.declare(type_decl("invoice", fields)).await?;

    let seen = Timestamp::from_date(2026, 2, 3);
    let all = json!({"amount": "12.50", "ref": "r1", "memo": "m", "x": "x1"});
    let mut rec = source_record("invoice", "i", all);
    rec.seen_at = seen;
    let windows = l.ingest("bank", vec![rec]);
    let windows = windows.await.must("ingesting `i` from `bank`")?;
    windows_reach_head(store.as_ref(), &windows, "ingesting `i` from `bank`").await?;
    let ingested = windows.last().map(|w| w.position);
    // A second ingest moves `ref` and `x` and clears `memo`; a mirror's
    // record `j` carries its `upstream` on every field.
    let again = source_record(
        "invoice",
        "i",
        json!({"ref": "r2", "memo": null, "x": "x2"}),
    );
    let mut mirrored = source_record("invoice", "j", json!({"amount": "1", "ref": "r"}));
    mirrored.upstream = Some(lakelands_api::record::Upstream {
        lake: "far".into(),
        position: Position(7),
    });
    let moved = l.ingest("bank", vec![again, mirrored]).await;
    moved.must("ingesting `i` again and `j`")?;
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status"]);
    let authored = upsert(
        cat.as_ref(),
        &clerk,
        "invoice",
        "i",
        json!({"status": "open"}),
    );
    let authored = authored.await?.position();

    let r = l.read("invoice", "i").await?;
    let amount = r
        .meta
        .get("amount")
        .ok_or_else(|| Failure::new("reading `i` with meta answered no provenance for `amount`"))?;
    ensure!(
        amount.origin == Origin::Source
            && amount.source == Some(SourceId::from("bank"))
            && amount.seen_at == seen
            && Some(amount.position) == ingested
            && amount.writer == own.writer(),
        "`amount` came from source `bank`, seen {seen:?}, ingested at {ingested:?} by \
         {}; its provenance reads {amount:?}",
        own.writer()
    );
    let status = r
        .meta
        .get("status")
        .ok_or_else(|| Failure::new("reading `i` with meta answered no provenance for `status`"))?;
    ensure!(
        status.origin == Origin::Authored
            && status.source.is_none()
            && status.seen_at.is_none()
            && status.procedure_version.is_none()
            && status.position == authored
            && status.writer == clerk.writer(),
        "`status` was written by {} at {authored} through `upsert`; its provenance \
         must say authored, name that writer and position, and carry no source, \
         seen_at or procedure_version; it reads {status:?}",
        clerk.writer()
    );
    ensure!(
        status.known_at >= amount.known_at,
        "`status` was learned after `amount`, but its known_at {} is earlier than {}",
        status.known_at,
        amount.known_at
    );

    let first = ingested.ok_or_else(|| Failure::new("an ingest answered no window"))?;
    let pos = |r: &lakelands_api::Record, f: &str| r.meta.get(f).map(|m| m.position);
    let j = l.read("invoice", "j").await?;
    let far = (j.meta.values()).all(|m| {
        m.upstream
            .as_ref()
            .is_some_and(|u| u.position == Position(7))
    });
    ensure!(
        pos(&r, "ref") == pos(&r, "x")
            && pos(&r, "ref") > Some(first)
            && !r.fields.contains_key("memo")
            && !r.meta.contains_key("memo")
            && far
            && j.meta.len() == 2,
        "`ref` and `x` were written again and `memo` cleared after {first}, `amount` at \
         {first}, and a cleared field answers no provenance; `j`'s two fields came with an \
         `upstream`; they read {:?} and {:?}",
        r.meta,
        j.meta
    );
    // As of the first ingest each field reads as it was; after a rebuild
    // every field reads as before.
    let then = Get {
        key: "i".into(),
        with_meta: true,
        at: ReadAt {
            as_of: Some(first),
            ..ReadAt::default()
        },
        params: Default::default(),
    };
    // A cleared field keeps the block of the write that cleared it, which
    // an expression reads, though a read answers it no provenance.
    let cleared = lakelands_api::Find {
        filter: Some("known_at(memo) = known_at(ref)".into()),
        ..Default::default()
    };
    let found = l.all_found("invoice", cleared).await?;
    ensure!(
        found.len() == 1 && found[0].key.as_str() == "i",
        "`memo` was cleared by the write of `ref`, so its known_at is that write's; it found {found:?}"
    );
    let was = cat
        .get(own, &"invoice".into(), then)
        .await
        .must("`i` as of the first ingest")?;
    ensure!(
        ["amount", "ref", "memo", "x"]
            .iter()
            .all(|f| pos(&was.data, f) == Some(first)),
        "as of {first} every field of `i` was the first ingest's; it reads {:?}",
        was.data.meta
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let (i2, j2) = (l.read("invoice", "i").await?, l.read("invoice", "j").await?);
    ensure!(
        i2 == r && j2 == j,
        "a rebuilt projection answers `i` and `j` with meta as before; `i` read {:?}, then {:?}",
        r.meta,
        i2.meta
    );

    let plain = cat
        .get(
            own,
            &TypeName::from("invoice"),
            Get {
                key: "i".into(),
                with_meta: false,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .must("reading `i` without meta")?;
    ensure!(
        plain.data.meta.is_empty(),
        "a read that does not ask for meta answered provenance for {:?}",
        plain.data.meta.keys().collect::<Vec<_>>()
    );
    Ok(())
}

/// The position of the write each field reads, by record; `None` where
/// the field reads as erased.
type Wanted<'a> = Vec<(&'a str, Vec<(&'a str, Option<Position>)>)>;

async fn each_record_reads_the_provenance_of_its_own_writes(store: Store, reopen: Reopen) -> Held {
    let (_keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let title = FieldDecl {
        searchable: true,
        erasable: true,
        ..sourced("title", StorageKind::Text)
    };
    let fields = vec![title, sourced("n", StorageKind::Decimal)];
    let mut note = type_decl("note", fields);
    note.fields.push(field("status", StorageKind::Text));
    l.declare(note).await?;
    let row = |key: &str, fields: Value| source_record("note", key, fields);
    // Three records of one write share its base, and the others one or two:
    // a projection that keeps a base apart only where `engines::SHARED`
    // records of a write name it holds both.
    const _: () = assert!(
        crate::engines::SHARED <= 3,
        "the first write shares its base"
    );
    let three = vec![
        row("a", json!({"title": "alpha one", "n": "1"})),
        row("b", json!({"title": "beta one", "n": "2"})),
        row("e", json!({"title": "epsilon one", "n": "5"})),
    ];
    l.ingest("crm", three)
        .await
        .must("ingesting `a`, `b` and `e`")?;
    let c = vec![row("c", json!({"title": "gamma one", "n": "3"}))];
    l.ingest("erp", c).await.must("ingesting `c`")?;
    let clerk = field_writer(store.as_ref(), "clerk", "note", &["status"]);
    upsert(cat.as_ref(), &clerk, "note", "a", json!({"status": "open"})).await?;
    let b = row("b", json!({"title": "beta two"}));
    let d = row("d", json!({"title": "delta two", "n": "4"}));
    l.ingest("crm", vec![b, d])
        .await
        .must("ingesting `b` again and `d`")?;
    // Each record's writes, by the position of its own event.
    let events = events_after(store.as_ref(), Position::GENESIS).await?;
    let made = |key: &str| -> Vec<Position> {
        let about = |e: &&Event| {
            (crate::feed::about(&e.body).iter()).any(|a| {
                a.ty.as_ref().is_some_and(|t| t.as_str() == "note")
                    && a.key.as_ref().is_some_and(|k| k.as_str() == key)
            })
        };
        events.iter().filter(about).map(|e| e.position).collect()
    };
    let (a, b, c, d, e) = (made("a"), made("b"), made("c"), made("d"), made("e"));
    let (&[a1, authored], &[b1, again], &[erp], &[d1], &[e1]) =
        (&a[..], &b[..], &c[..], &d[..], &e[..])
    else {
        return Err(Failure::new(format!(
            "the log holds two writes of `a` and `b` and one of `c`, `d` and `e`; it holds \
             {a:?}, {b:?}, {c:?}, {d:?} and {e:?}"
        )));
    };
    let head: Wanted = vec![
        (
            "a",
            vec![
                ("title", Some(a1)),
                ("n", Some(a1)),
                ("status", Some(authored)),
            ],
        ),
        ("b", vec![("title", Some(again)), ("n", Some(b1))]),
        ("c", vec![("title", Some(erp)), ("n", Some(erp))]),
        ("e", vec![("title", Some(e1)), ("n", Some(e1))]),
        ("d", vec![("title", Some(d1)), ("n", Some(d1))]),
    ];
    let mut before = head.clone();
    before.pop();
    before[0].1.pop();
    before[1].1[0].1 = Some(b1);
    // crm forgotten: `b`, `d` and `e` retire, `a`'s title reads as erased.
    let forgotten: Wanted = vec![
        (
            "a",
            vec![("title", None), ("n", Some(a1)), ("status", Some(authored))],
        ),
        ("c", vec![("title", Some(erp)), ("n", Some(erp))]),
    ];
    // A field reads its write's provenance: that event's.
    let of = |m: &lakelands_api::record::FieldMeta, at: Option<Position>| {
        let Some(at) = at else { return m.erased };
        events.iter().find(|e| e.position == at).is_some_and(|e| {
            (
                m.position, &m.origin, &m.source, &m.writer, m.known_at, m.seen_at,
            ) == (at, &e.origin, &e.source, &e.writer, e.known_at, e.seen_at)
        })
    };
    let mut wrong = Vec::new();
    let mut held = |door: &str, r: &Record, wanted: &Wanted| {
        let Some((_, fields)) = wanted.iter().find(|(k, _)| *k == r.key.as_str()) else {
            return wrong.push(format!("{door} answered `{}`, which no write made", r.key));
        };
        let reads = (fields.iter()).all(|(f, at)| r.meta.get(*f).is_some_and(|m| of(m, *at)));
        if !reads || r.meta.len() != fields.len() {
            wrong.push(format!(
                "{door}: `{}` reads {:?}, not the writes at {fields:?}",
                r.key, r.meta
            ));
        }
    };
    let l = &l;
    let read_all = |as_of: Option<Position>, keys: &'static [&'static str]| async move {
        let mut doors = Vec::new();
        for key in keys {
            let got = cat.get(own, &"note".into(), get_at(key, as_of)).await;
            doors.push(("`get`", got.must("reading a note")?.data));
        }
        let find = Find {
            with_meta: true,
            at: read_at(as_of),
            ..Find::default()
        };
        let found = l.all_found("note", find).await?;
        doors.extend(found.into_iter().map(|r| ("`find`", r)));
        let search = Search {
            with_meta: true,
            at: read_at(as_of),
            ..searched("note", SearchMode::lexical("one two"), 10)
        };
        let hits = cat.search(own, search).await.must("searching the notes")?;
        let hits = hits.data.hits.into_iter();
        doors.extend(hits.map(|h| ("a lexical search", h.record)));
        Ok::<_, Failure>(doors)
    };
    let all = &["a", "b", "c", "e", "d"];
    let first = read_all(None, all).await?;
    let earlier = read_all(Some(erp), &all[..4]).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = read_all(None, all).await?;
    let crm = SourceId::from("crm");
    store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm")?;
    let kept = &["a", "c"];
    let after = read_all(None, kept).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let again = read_all(None, kept).await?;
    let counts = [
        first.len(),
        earlier.len(),
        rebuilt.len(),
        after.len(),
        again.len(),
    ];
    for (when, doors, wanted) in [
        ("at the head", first, &head),
        ("as of the second ingest", earlier, &before),
        ("after a rebuild", rebuilt, &head),
        ("after crm is forgotten", after, &forgotten),
        ("after crm is forgotten and a rebuild", again, &forgotten),
    ] {
        for (door, r) in doors {
            held(&format!("{door} {when}"), &r, wanted);
        }
    }
    ensure!(
        wrong.is_empty() && counts == [15, 12, 15, 5, 5],
        "each note reads the provenance of the writes that made its fields, through every \
         door ({counts:?} records read, not [15, 12, 15, 5, 5]):\n{}",
        wrong.join("\n")
    );
    Ok(())
}

async fn a_valid_time_names_a_stored_date_time_field(store: Store) -> Held {
    use crate::conformance::fixture::{derived, migration, reference, refused_on, vector, view_of};
    use lakelands_api::Refusal;
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("payroll", vec![field("code", StorageKind::Text)]))
        .await?;
    let not_date_time = |field: &str, axis: &str, kind| Case::ValidTimeNotTime {
        field: field.into(),
        axis: axis.into(),
        kind,
    };
    let typed = |name: &str, at: FieldDecl, valid: (&str, &str)| {
        let mut t = type_decl(name, vec![field("from", StorageKind::DateTime), at]);
        (t.valid_from, t.valid_to) = (Some(valid.0.into()), Some(valid.1.into()));
        t
    };
    let mut wrong = Vec::new();
    // Every other kind, as `valid_from` and as `valid_to`.
    let other = StorageKind::ALL
        .into_iter()
        .filter(|k| !matches!(k, StorageKind::DateTime | StorageKind::Date));
    for kind in other {
        let at = match kind {
            StorageKind::Reference => reference("at", "payroll"),
            StorageKind::Vector => vector("at", 2),
            _ => field("at", kind),
        };
        for (valid, axis) in [(("at", "from"), "valid_from"), (("from", "at"), "valid_to")] {
            let answer = l.try_declare(typed("stamped", at.clone(), valid)).await;
            let case = not_date_time("at", axis, kind);
            let said = matches!(refused_on(&answer), Some(("at", Refusal::Invalid(said)))
                if said.case == case);
            if !said {
                wrong.push(format!("`at` of kind {kind:?} as {valid:?}: {answer:?}"));
            }
        }
    }
    // A stored `date_time` field, derived or written, is a valid time, and
    // a read at `valid_at` admits by it.
    let on = derived("on", StorageKind::DateTime, "from", &["from"]);
    let ok = typed("stamped", on, ("on", "from"));
    let mut late = ok.clone();
    late.fields.push(field("until", StorageKind::DateTime));
    late.fields.push(field("code", StorageKind::Text));
    late.valid_to = Some("until".into());
    l.declare(late.clone()).await?;
    l.upsert(
        "stamped",
        "s1",
        json!({"from": "2026-03-01T00:00:00Z", "until": "2026-05-01T00:00:00Z", "code": "a"}),
    )
    .await?;
    let at = |t: &str| Find {
        at: ReadAt {
            valid_at: Some(Timestamp::parse(t).expect("an instant")),
            ..ReadAt::default()
        },
        ..Find::default()
    };
    let (april, june) = (
        l.all_found("stamped", at("2026-04-01T00:00:00Z")).await?,
        l.all_found("stamped", at("2026-06-01T00:00:00Z")).await?,
    );
    if (april.len(), june.len()) != (1, 0) {
        wrong.push(format!(
            "`stamped`, valid from its derived `on`, read in April and June: {} and {}",
            april.len(),
            june.len()
        ));
    }
    // A `date` valid time is its days' midnights, half-open: a booking
    // from the 1st to the 31st holds through the 30th, not on the 31st.
    let mut booked = type_decl("booked", vec![field("from", StorageKind::Date)]);
    booked.fields.push(field("to", StorageKind::Date));
    (booked.valid_from, booked.valid_to) = (Some("from".into()), Some("to".into()));
    l.declare(booked).await?;
    let july = json!({"from": "2026-07-01", "to": "2026-07-31"});
    l.upsert("booked", "b1", july).await?;
    let mut held = Vec::new();
    for t in [
        "2026-06-30T23:59:59Z",
        "2026-07-01",
        "2026-07-30T23:59:59Z",
        "2026-07-31",
    ] {
        held.push(l.all_found("booked", at(t)).await?.len());
    }
    if held != [0, 1, 1, 0] {
        wrong.push(format!(
            "`booked` from 07-01 to 07-31, read around both: {held:?}"
        ));
    }
    // A view's `date_time` field is one, and its `number` field none.
    let view = |valid: &str| {
        let fields = vec![
            field("code", StorageKind::Text),
            field("first", StorageKind::DateTime),
            field("n", StorageKind::Number),
        ];
        let measures = [("first", "min(on)"), ("n", "count()")];
        let mut v = view_of("firsts", "stamped", &["code"], fields, &measures);
        v.valid_from = Some(valid.into());
        v
    };
    let answer = l.try_declare(view("n")).await;
    let case = not_date_time("n", "valid_from", StorageKind::Number);
    let said = matches!(refused_on(&answer), Some(("n", Refusal::Invalid(said)))
        if said.case == case);
    if !said {
        wrong.push(format!("a view valid from its `n`: {answer:?}"));
    }
    l.declare(view("first")).await?;
    let (feb, april) = (
        l.all_found("firsts", at("2026-02-01T00:00:00Z")).await?,
        l.all_found("firsts", at("2026-04-01T00:00:00Z")).await?,
    );
    if (feb.len(), april.len()) != (0, 1) {
        wrong.push(format!(
            "`firsts`, valid from its `first`, read in February and April: {} and {}",
            feb.len(),
            april.len()
        ));
    }
    // A migration to a text valid time is refused alike, and so is the
    // declaration by `apply` and by a proposal.
    let mut text = late.clone();
    text.fields.retain(|f| f.name.as_str() != "until");
    text.fields.push(field("until", StorageKind::Text));
    let declared = || vec![lakelands_api::write::Declaration::Type(text.clone())];
    let proposed = lakelands_api::write::ProposedWrite::Declare {
        declarations: declared(),
        plan: None,
    };
    for (door, answer) in [
        (
            "a migration",
            l.cat
                .migrate(&l.own, migration(text.clone()))
                .await
                .map(drop),
        ),
        ("`apply`", l.cat.apply(&l.own, declared()).await.map(drop)),
        (
            "a proposal",
            l.cat
                .propose(&l.own, proposed, "text".into())
                .await
                .map(drop),
        ),
    ] {
        let case = not_date_time("until", "valid_to", StorageKind::Text);
        let said = matches!(refused_on(&answer), Some(("until", Refusal::Invalid(said)))
            if said.case == case);
        if !said {
            wrong.push(format!("{door} of `until` as text: {answer:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a valid time names a stored date-time field:\n{}",
        wrong.join("\n")
    );
    Ok(())
}
