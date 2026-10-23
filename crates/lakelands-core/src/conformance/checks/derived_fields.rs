//! Derived fields: computed by the fold, written by no door
//! (`lakelands_api::ontology::Authority::Derived`, `docs/log-format.md`
//! §2.3).
//!
//! An author declares `gross = amount * 1.19` once and writes only
//! `amount`; the store fills `gross` in the same event, with `derived` as
//! its origin, and a replay reproduces it. A store that accepted the
//! declaration and then answered records without the field would leave
//! every total built on it quietly short, which is why the check reads
//! the field back rather than trusting the declaration.

use crate::conformance::fixture::{derived, get_at, read, source_record, type_of};
use crate::conformance::prelude::*;

check!(
    COMPUTED_BY_THE_FOLD = a_derived_field_is_computed_by_the_fold_on_every_write,
    "the ontology and the log format: the fold computes a derived field from its expression in the event that changes its inputs"
);

check!(
    NO_DOOR_WRITES_IT = a_derived_field_written_through_a_door_is_refused_as_the_wrong_door,
    "015 and the ontology: no door writes a derived field; a patch carrying one is refused whole as the wrong door"
);

check!(
    ADDED_LATER = a_derived_field_added_to_a_live_type_is_computed_on_later_writes,
    "the ontology: a derived field added with add_field is kept by the fold for every write after it, like one declared with the type"
);

check!(
    READ_EACH_OTHER = derived_fields_that_read_each_other_are_each_asked_once,
    "006, extended (lakelands#926): a type of twenty-eight derived fields, each reading every later one, is declared, and the lake's next declaration, a write, a `get`, `describe` and the `get` of a reader masked from the one field they all read each answer within five seconds, where asking a field once for every way that leads to it asks 2^27 times for one; the first field holds 2^27 times what was written, and that reader is shown all twenty-eight masked"
);

check!(
    LIST_EACH_OTHER = fields_that_list_each_other_are_read_no_further_than_each_other,
    "006, extended (lakelands#926): fields that list each other in `depends_on` are read no further than each other, all the way round: of two written fields `a` and `b` listing each other, to a reader masked on `a` and writing `b`, `b` is written and not read and `a` is hidden, at `get` and in `describe`, and a `where` on `a` is refused as a field the type does not have; read whole they are both read; and two ways to one masked field lower alike: of a field built from two fields that are each built from one masked field, to a reader writing one of the two, the other is masked, the one written is written and not read, and the field built from them is hidden"
);

async fn fields_that_list_each_other_are_read_no_further_than_each_other(store: Store) -> Held {
    let (l, s, text) = (Lake::of(store.as_ref()), store.as_ref(), StorageKind::Text);
    let listing = |name: &str, from: &[&str]| FieldDecl {
        depends_on: from.iter().map(|f| (*f).into()).collect(),
        ..field(name, text)
    };
    let pair = vec![listing("a", &["b"]), listing("b", &["a"])];
    l.declare(type_decl("pair", pair)).await?;
    let ways = [
        ("f0", &["f1", "f2"][..]),
        ("f1", &["f3"]),
        ("f2", &["f3"]),
        ("f3", &[]),
    ];
    let ways = ways.map(|(n, from)| listing(n, from)).to_vec();
    l.declare(type_decl("ways", ways)).await?;
    l.upsert("pair", "p1", json!({"a": "x", "b": "y"})).await?;
    l.upsert(
        "ways",
        "w1",
        json!({"f0": "0", "f1": "1", "f2": "2", "f3": "3"}),
    )
    .await?;
    let shown = |d: &lakelands_api::describe::TypeDescription| -> Vec<(String, Access)> {
        (d.fields.iter())
            .map(|f| (f.name.to_string(), f.access))
            .collect()
    };
    let (w, m) = (Access::Write, Access::Masked);
    let mut red = Vec::new();
    // Per reader: what `describe` shows, then what `get` answers and masks.
    for (ty, key, grants, want, answered, masked) in [
        (
            "pair",
            "p1",
            vec![("a", m), ("b", w)],
            vec![("b", Access::WriteOnly)],
            json!({}),
            &[][..],
        ),
        (
            "pair",
            "p1",
            vec![("a", w), ("b", w)],
            vec![("a", w), ("b", w)],
            json!({"a": "x", "b": "y"}),
            &[],
        ),
        (
            "ways",
            "w1",
            vec![("f2", w), ("f3", m)],
            vec![("f1", m), ("f2", Access::WriteOnly), ("f3", m)],
            json!({}),
            &["f1", "f3"],
        ),
    ] {
        let who = field_access(s, "reader", ty, &grants);
        let got =
            shown(&crate::conformance::fixture::describe_type(l.cat.as_ref(), &who, ty).await?);
        let want: Vec<_> = want.iter().map(|(n, a)| (n.to_string(), *a)).collect();
        if got != want {
            red.push(format!(
                "`{ty}` under {grants:?} is described {got:?}, not {want:?}"
            ));
        }
        let record = read(l.cat.as_ref(), &who, ty, key).await?;
        let got = (
            json!(record.fields),
            record.masked.iter().map(|f| f.as_str()),
        );
        if got.0 != answered || !got.1.clone().eq(masked.iter().copied()) {
            let got_masked: Vec<_> = got.1.collect();
            red.push(format!(
                "`{ty}` `{key}` under {grants:?} answers {} masking {got_masked:?}, not {answered} masking {masked:?}",
                got.0
            ));
        }
        if grants.iter().any(|g| g.1 != w) {
            let hidden = if ty == "pair" { "a" } else { "f0" };
            let (on, filter) = (TypeName::from(ty), find_where(&format!("{hidden} = 'x'")));
            let found = l.cat.find(&who, &on, filter).await;
            if !no_such_field(&found, hidden) {
                red.push(format!(
                    "a `where` on `{hidden}` under {grants:?}: {found:?}"
                ));
            }
        }
    }
    ensure!(
        red.is_empty(),
        "fields that list each other are read no further than each other: {}",
        red.join("; ")
    );
    Ok(())
}

/// How long a call of [`READ_EACH_OTHER`] may take: far above what it
/// costs, far below a walk that doubles with every field.
const BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// Fail a call that began at `started` and took longer than [`BOUND`].
fn within(what: &str, started: std::time::Instant) -> Held {
    let took = started.elapsed();
    ensure!(
        took < BOUND,
        "{what} took {took:?}. Twenty-eight fields that read each other are asked once each, \
         in milliseconds; a walk that asks a field again for every way that leads to it \
         doubles with each field (lakelands#926)"
    );
    Ok(())
}

fn gross() -> FieldDecl {
    derived("gross", StorageKind::Decimal, "amount * 1.19", &["amount"])
}

async fn a_derived_field_is_computed_by_the_fold_on_every_write(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    l.declare(ty).await?;

    let first = l.upsert("invoice", "i1", json!({"amount": "100.00"}));
    let first = first.await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` is declared as amount * 1.19; after writing amount 100.00 it must read \
         exactly 119 as a decimal string, but reads {got:?}. A declaration the store \
         accepted is a field it owes"
    );
    let origin = rec.meta.get("gross").map(|m| m.origin);
    ensure!(
        origin == Some(Origin::Derived),
        "a field the fold writes carries origin `derived` in its provenance; `gross` \
         carries {origin:?}"
    );

    l.upsert("invoice", "i1", json!({"amount": "200.00"}))
        .await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("238")),
        "after amount changed to 200.00, `gross` must follow to 238; it reads {got:?}"
    );

    let then = cat
        .get(
            own,
            &TypeName::from("invoice"),
            get_at("i1", Some(first.position())),
        )
        .await
        .must("reading `i1` as of its first write")?;
    let got = then.data.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "as of the first write, `gross` must read 119 as it did then; it reads {got:?}"
    );
    Ok(())
}

async fn a_derived_field_written_through_a_door_is_refused_as_the_wrong_door(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    l.declare(ty).await?;

    let before = store.head().await.must("reading the head")?;
    let fields = json!({"amount": "100.00", "gross": "1.00"});
    let answer = try_upsert(cat.as_ref(), own, "invoice", "i1", fields).await;
    let what = "the owner writing the derived field `gross`";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(refused_on(&err), Some(("gross", Refusal::WrongDoor))),
        "a patch carrying the derived field `gross` must be refused as the wrong door on \
         `gross`, even for the owner; it failed with {err:?}"
    );
    let missing = cat
        .get(own, &TypeName::from("invoice"), get_at("i1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused patch created `i1` anyway: {missing:?}"
    );
    Ok(())
}

async fn a_derived_field_added_to_a_live_type_is_computed_on_later_writes(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    l.declare(ty).await?;
    l.upsert("invoice", "i1", json!({"amount": "100.00"}))
        .await?;

    cat.add_field(own, &TypeName::from("invoice"), gross())
        .await
        .must("adding the derived field `gross` to the live type `invoice`")?;

    l.upsert("invoice", "i2", json!({"amount": "100.00"}))
        .await?;
    let rec = l.read("invoice", "i2").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` was added by add_field; a record written afterwards with amount 100.00 \
         must read gross 119, but reads {got:?}"
    );

    l.upsert("invoice", "i1", json!({"amount": "10.00"}))
        .await?;
    let rec = l.read("invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("11.9")),
        "`i1` was written before `gross` existed; once its amount changes to 10.00, \
         the fold must compute gross 11.9, but it reads {got:?}"
    );
    Ok(())
}

check!(
    NO_DOOR_AT_ALL = a_derived_field_accepts_no_value_from_a_source_or_a_verb_either,
    "015 and 016: a derived field is the fold's alone; a source record or a verb that would set it is refused whole, not only an upsert"
);

check!(
    FUNCTION_VERSION_RECOMPUTES =
        a_derived_field_reading_a_function_is_recomputed_when_the_function_changes,
    "022: a derived field that names a function is computed by that function's declaration in force; a new version of the function recomputes it and names that version as its procedure"
);

check!(
    SAME_INPUTS_NEW_BODY = a_function_declared_again_with_the_same_inputs_computes_by_its_new_body,
    "022: a function declared again with the same inputs and another body computes by the new body, and so do the derived fields that read what it computes: a record it recomputes, at the head and as of the declaration, and one written after it; as of before, the old body's values stand"
);

/// The function `scaled` of one decimal input `net`, computing `expr`.
fn scaled(expr: &str) -> Result<Declaration, Failure> {
    serde_json::from_value(json!({
        "declare": "function", "name": "scaled", "backing": "expr",
        "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
        "expr": expr
    }))
    .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))
}

async fn a_derived_field_accepts_no_value_from_a_source_or_a_verb_either(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut net = field("net", StorageKind::Decimal);
    net.authority = Authority::Shared;
    let gross = derived("gross", StorageKind::Decimal, "net * 2", &["net"]);
    l.declare(type_decl("bill", vec![net, gross])).await?;
    let verb: Declaration = serde_json::from_value(json!({
        "declare": "verb", "name": "inflate", "on": "bill", "risk": "low",
        "effect": "write", "set": [{"field": "gross", "value": "'99'"}]
    }))
    .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))?;
    let declared = cat.declare(own, verb).await;
    l.upsert("bill", "b", json!({"net": "1"})).await?;

    let before = store.head().await.must("reading the head")?;
    let fed = l.ingest(
        "erp",
        vec![source_record("bill", "b", json!({"gross": "5"}))],
    );
    let fed = fed.await;
    ensure!(
        fed.is_err(),
        "a source record setting the derived `gross` must be refused; the ingest answered {fed:?}"
    );
    head_unmoved(store.as_ref(), before, "ingesting a derived field").await?;

    // A verb that sets a derived field may be refused when declared, or
    // when called; either way no value lands.
    if declared.is_ok() {
        let before = store.head().await.must("reading the head")?;
        let called = cat
            .call(
                own,
                &"inflate".into(),
                Some(&"b".into()),
                patch(json!({})),
                Default::default(),
            )
            .await;
        ensure!(
            called.is_err(),
            "a verb setting the derived `gross` must be refused; the call answered {called:?}"
        );
        head_unmoved(store.as_ref(), before, "a verb setting a derived field").await?;
    }
    let r = l.read("bill", "b").await?;
    ensure!(
        decimal(value(&r, "gross")) == Some(dec("2")),
        "`gross` must still be the fold's net * 2 = 2; it reads {:?}",
        value(&r, "gross")
    );
    Ok(())
}

async fn a_derived_field_reading_a_function_is_recomputed_when_the_function_changes(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_as(scaled("net * 2")?, "declaring `scaled` as net * 2")
        .await?;
    let scaled_by = computed_by("scaled", StorageKind::Decimal, "scaled", &["net"]);
    l.declare(type_decl(
        "bill",
        vec![field("net", StorageKind::Decimal), scaled_by],
    ))
    .await?;
    l.upsert("bill", "b", json!({"net": "3"})).await?;
    let r = l.read("bill", "b").await?;
    ensure!(
        decimal(value(&r, "scaled")) == Some(dec("6")),
        "`scaled` must be computed by `scaled` (net * 2) as 6; it reads {:?}",
        value(&r, "scaled")
    );

    l.declare_as(scaled("net * 3")?, "declaring `scaled` as net * 3")
        .await?;
    let r = l.read("bill", "b").await?;
    let meta = (r.meta.get("scaled")).map(|m| (m.procedure.clone(), m.procedure_version.clone()));
    let want = (Some("scaled".to_owned()), Some("2".to_owned()));
    ensure!(
        decimal(value(&r, "scaled")) == Some(dec("9")) && meta == Some(want),
        "after `scaled` became net * 3 (version 2), the record's `scaled` must be \
         recomputed to 9 and name the procedure `scaled` at version 2; it reads {:?} \
         with procedure and version {meta:?}",
        value(&r, "scaled")
    );
    Ok(())
}

async fn a_function_declared_again_with_the_same_inputs_computes_by_its_new_body(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare_as(scaled("net * 2")?, "declaring `scaled` as net * 2")
        .await?;
    let total = derived(
        "total",
        StorageKind::Decimal,
        "scaled + net",
        &["scaled", "net"],
    );
    let half = derived("half", StorageKind::Decimal, "net / 2", &["net"]);
    let net = field("net", StorageKind::Decimal);
    let scaled_by = computed_by("scaled", StorageKind::Decimal, "scaled", &["net"]);
    l.declare(type_decl("bill", vec![net, scaled_by, total, half]))
        .await?;
    let written = l.upsert("bill", "a", json!({"net": "4"})).await?;
    let again = scaled("net * 3")?;
    let again = l
        .declare_as(again, "declaring `scaled` again as net * 3")
        .await?;
    l.upsert("bill", "b", json!({"net": "4"})).await?;
    let old = ["8", "12", "2"].map(|v| Some(dec(v)));
    let new = ["12", "16", "2"].map(|v| Some(dec(v)));
    for (key, at, want, when) in [
        ("a", None, &new, "at the head"),
        ("a", Some(again.position()), &new, "as of the declaration"),
        ("a", Some(written.position()), &old, "as of its write"),
        ("b", None, &new, "written after the declaration"),
    ] {
        let got = cat.get(own, &TypeName::from("bill"), get_at(key, at)).await;
        let got = got.must(&format!("reading `{key}` {when}"))?.data;
        let got = ["scaled", "total", "half"].map(|f| decimal(got.fields.get(f)));
        ensure!(
            &got == want,
            "`scaled` was declared again with the same inputs as net * 3; `{key}` {when} must \
             read scaled, total and half as {want:?}, by the body in force there; it reads {got:?}"
        );
    }
    Ok(())
}

async fn derived_fields_that_read_each_other_are_each_asked_once(store: Store) -> Held {
    const N: usize = 28;
    let (l, num) = (Lake::of(store.as_ref()), StorageKind::Number);
    let mut fields = vec![field("base", num)];
    for i in 0..N {
        let later: Vec<String> = (i + 1..N).map(|j| format!("d{j}")).collect();
        let mut reads: Vec<&str> = later.iter().map(String::as_str).collect();
        reads.push("base");
        fields.push(derived(&format!("d{i}"), num, &reads.join(" + "), &reads));
    }
    let now = std::time::Instant::now;
    let started = now();
    l.declare(type_decl("sum", fields)).await?;
    within("declaring the type", started)?;
    let started = now();
    l.declare(type_of("other", &[("x", num)])).await?;
    within("the lake's next declaration", started)?;
    let started = now();
    l.upsert("sum", "s1", json!({"base": 1})).await?;
    within("a write of the type", started)?;
    let started = now();
    let rec = l.read("sum", "s1").await?;
    within("a `get`", started)?;
    let got = rec.fields.get("d0");
    ensure!(
        got.and_then(|v| v.as_f64()) == Some(134_217_728.0),
        "each field is the sum of every later one and `base`, so `d27` is 1 and `d0` is \
         2^27 = 134217728, but `d0` reads {got:?}"
    );
    let started = now();
    l.describe().await?;
    within("`describe`", started)?;
    // A field is read no further than what it is built from, and theirs in
    // turn: one masked field at the end of every way masks them all.
    let masked = field_access(store.as_ref(), "masked", "sum", &[("base", Access::Masked)]);
    let started = now();
    let rec = read(l.cat.as_ref(), &masked, "sum", "s1").await?;
    within("the `get` of a reader masked from `base`", started)?;
    ensure!(
        rec.fields.is_empty() && rec.masked.len() == N + 1,
        "`base` is masked for this reader and every `d` field is built from it, so all {} \
         are masked and none is read; it reads {:?} and is shown {} masked",
        N + 1,
        rec.fields.keys().collect::<Vec<_>>(),
        rec.masked.len()
    );
    Ok(())
}
