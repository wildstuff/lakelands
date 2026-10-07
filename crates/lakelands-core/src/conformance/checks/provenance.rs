//! The two time axes of a read, and the provenance every field carries
//! (`docs/decisions/009-every-response-carries-its-position.md`,
//! `docs/decisions/010-provenance-per-field.md`, `docs/glossary.md`).

use std::sync::Arc;

use lakelands_api::ontology::{Authority, FieldDecl, StorageKind};
use lakelands_api::record::Origin;
use lakelands_api::{
    Error, Find, Get, LakeStore, Missing, Position, ReadAt, SourceId, Timestamp, TypeName,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field, field_writer, source_record, type_decl, upsert, windows_reach_head,
};

check!(
    /// A valid time is a stored date-time field.
    VALID_TIME_IS_A_DATE_TIME = a_valid_time_names_a_stored_date_time_field,
    "028, extended (lakelands#714): `valid_from` and `valid_to` each name a field of kind `date_time`, so a value that only reads as an instant never sets a record's validity; a field of any other kind is refused as invalid on it, by `declare`, `apply`, a proposal and a migration, a derived `date_time` field and a view's `date_time` field are valid times as any is, and a read at `valid_at` admits by them"
);

check!(
    /// Business time and transaction time are two axes.
    TWO_TIME_AXES = valid_at_and_as_of_combine_and_neither_stands_in_for_the_other,
    "009 and the glossary: `valid_at` asks what was true in the world, `as_of` what the lake knew; a read may name both"
);

check!(
    /// Every field says where its value came from.
    FIELD_PROVENANCE = every_field_carries_its_own_provenance,
    "010: with meta, each field answers its origin, known_at, position and writer, and a value from a source also its source and seen_at"
);

fn day(m: u32, d: u32) -> String {
    Timestamp::from_date(2026, m, d)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

async fn valid_at_and_as_of_combine_and_neither_stands_in_for_the_other(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    let first = l
        .upsert(
            "contract",
            "c",
            json!({"party": "acme", "from": day(1, 1), "to": day(2, 1)}),
        )
        .await?
        .position();
    l.upsert("contract", "c", json!({"to": day(4, 1)})).await?;

    let contract = TypeName::from("contract");
    let at = |as_of: Option<Position>, valid: String| ReadAt {
        as_of,
        valid_at: Timestamp::parse(&valid).ok(),
    };
    let found = |as_of, valid: String| {
        let cat = cat.clone();
        let own = own.clone();
        let contract = contract.clone();
        async move {
            let got = cat
                .get(
                    &own,
                    &contract,
                    Get {
                        key: "c".into(),
                        with_meta: false,
                        at: at(as_of, valid),
                        params: Default::default(),
                    },
                )
                .await;
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

async fn every_field_carries_its_own_provenance(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![
            FieldDecl {
                authority: Authority::Source,
                ..field("amount", StorageKind::Decimal)
            },
            field("status", StorageKind::Text),
        ],
    );
    l.declare(ty).await?;

    let seen = Timestamp::from_date(2026, 2, 3);
    let mut rec = source_record("invoice", "i", json!({"amount": "12.50"}));
    rec.seen_at = seen;
    let windows = l
        .ingest("bank", vec![rec])
        .await
        .must("ingesting `i` from `bank`")?;
    windows_reach_head(store.as_ref(), &windows, "ingesting `i` from `bank`").await?;
    let ingested = windows.last().map(|w| w.position);
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status"]);
    let authored = upsert(
        cat.as_ref(),
        &clerk,
        "invoice",
        "i",
        json!({"status": "open"}),
    )
    .await?
    .position();

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

async fn a_valid_time_names_a_stored_date_time_field(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use crate::conformance::fixture::{derived, migration, reference, refused_on, vector, view_of};
    use lakelands_api::Refusal;
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("payroll", vec![field("code", StorageKind::Text)]))
        .await?;
    let typed = |name: &str, at: FieldDecl, valid: (&str, &str)| {
        let mut t = type_decl(name, vec![field("from", StorageKind::DateTime), at]);
        (t.valid_from, t.valid_to) = (Some(valid.0.into()), Some(valid.1.into()));
        t
    };
    let mut wrong = Vec::new();
    // Every other kind, as `valid_from` and as `valid_to`.
    let other = StorageKind::ALL
        .into_iter()
        .filter(|k| *k != StorageKind::DateTime);
    for kind in other {
        let at = match kind {
            StorageKind::Reference => reference("at", "payroll"),
            StorageKind::Vector => vector("at", 2),
            _ => field("at", kind),
        };
        for valid in [("at", "from"), ("from", "at")] {
            let answer = l.try_declare(typed("stamped", at.clone(), valid)).await;
            let said = matches!(refused_on(&answer), Some(("at", Refusal::Invalid { message }))
                if message.contains("a valid time is a date-time field"));
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
    let said = matches!(refused_on(&answer), Some(("n", Refusal::Invalid { message }))
        if message.contains("a valid time is a date-time field"));
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
        let said = matches!(refused_on(&answer), Some(("until", Refusal::Invalid { message }))
            if message.contains("a valid time is a date-time field"));
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
