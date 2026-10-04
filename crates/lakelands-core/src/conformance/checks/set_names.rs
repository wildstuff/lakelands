//! A set names a set with `in_set('name')`, and a path steps only into a
//! value that has members (`docs/decisions/004-the-change-feed-is-the-log.md`,
//! `024-probing-a-field-needs-the-right-to-read-it.md` and
//! `028-authoring-is-strict-and-whole.md`, each extended).
//!
//! The lake: five invoices of one partner. `open` holds those whose status
//! is `open` (`i1`, `i2`, `i5`), `big` those above 100 (`i2` to `i5`). Every
//! set below is counted by hand from that.

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind::{Json, Number, Text};
use lakelands_api::principal::Access;
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{LakeStore, Outcome, Position, Principal, Refusal, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, derived, events_from, field, field_access, find_at, head_unmoved, invalid_naming,
    limits, migration, object_set_on, reference, refused_on, trigger_on, type_decl, type_of,
};
use crate::conformance::{Failure, Given};

check!(
    /// Sets built from sets.
    COMPOSED = a_set_names_a_set_and_follows_it,
    "004, extended (lakelands#552), and log format §6: in_set('name') holds for a record where the named set's filter holds: union, intersection and difference are or, and, and not, through two levels; a read's where names a set and reads its filter, as of an earlier position the filter in force there; a set declared again decides the sets built on it in the same append; a write joins and leaves them; a trigger whose when names a set fires as the filter holds; the declaration that names a set is written in format version 11"
);

check!(
    /// What a set may not name, and where a path may not step.
    REFUSED = a_set_that_cannot_be_named_and_a_path_that_cannot_be_walked_are_refused,
    "028, extended (lakelands#552): in_set of a set that is not declared, of a set of another type and of a set that names it in turn is refused as invalid, and in_set outside a set's where, a trigger's when and a read's where; a path that steps into a field that is no json is refused at the declaration of a set, a trigger and a derived field and at a read, saying what to write, while a path into a json field is read; a set declared again for another type is refused on `of`; none appends; past max_set_changes over a declared set and the sets built on it the declaration is refused too_large, naming the set whose change passed it",
    given
);

check!(
    /// A named set tells no more than its filter.
    RIGHTS = a_named_set_is_read_by_the_right_to_read_what_its_filter_reads,
    "024, extended (lakelands#552): a reader kept from a field is answered, for in_set of a set whose filter reads it, directly or through the sets it names, as for a set that is not declared; describe lists it no such set; a write's answer names it no such set joined or left; an author kept from the field declares no set that names one"
);

const TYPE: &str = "invoice";

fn set(name: &str, filter: &str) -> Declaration {
    object_set_on(name, TYPE, filter)
}

async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_of("partner", &[("name", Text)])).await?;
    let mut fields = vec![reference("partner", "partner"), field("status", Text)];
    fields.extend([field("amount", Number), field("meta", Json)]);
    l.declare(type_decl(TYPE, fields)).await?;
    l.upsert("partner", "p1", json!({"name": "x"})).await?;
    for row in "i1 open 50,i2 open 200,i3 paid 300,i4 held 700,i5 open 900".split(',') {
        let v: Vec<&str> = row.split(' ').collect();
        let amount: i64 = v[2].parse().unwrap_or_default();
        let fields = json!({"partner": "p1", "status": v[1], "amount": amount});
        l.upsert(TYPE, v[0], fields).await?;
    }
    for (name, filter) in [("open", "status = 'open'"), ("big", "amount > 100")] {
        l.declare_as(set(name, filter), name).await?;
    }
    Ok(l)
}

/// The keys `filter` holds for, read by `who` as of `at`, in order, as
/// one text.
async fn found(
    l: &Lake,
    who: &Principal,
    filter: &str,
    at: Option<Position>,
) -> lakelands_api::Result<String> {
    let mut find = find_at(at);
    find.filter = Some(filter.into());
    let found = l.cat.find(who, &TypeName::from(TYPE), find).await?.data;
    let lakelands_api::Found::Records { rows, .. } = found else {
        return Ok(String::new());
    };
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort_unstable();
    Ok(keys.join(" "))
}

/// The membership changes appended from `from` on, as `+set:key` and
/// `-set:key`, in order, as one text.
async fn changes(store: &dyn LakeStore, from: Position) -> Result<String, Failure> {
    let events = events_from(store, from).await?;
    let change = |e: lakelands_api::event::Event| match e.body {
        EventBody::ObjectSetChanged {
            set, key, joined, ..
        } => Some(format!("{}{set}:{key}", if joined { '+' } else { '-' })),
        _ => None,
    };
    let mut out: Vec<String> = events.into_iter().filter_map(change).collect();
    out.sort();
    Ok(out.join(" "))
}

/// What a write's answer says it set off: fired, joined, left.
fn set_off(o: &Outcome) -> String {
    let Outcome::Applied { set_off, .. } = o else {
        return String::new();
    };
    let fired: Vec<&str> = set_off.fired.iter().map(|t| t.as_str()).collect();
    let (fired, joined, left) = (
        fired.join(" "),
        set_off.joined.join(" "),
        set_off.left.join(" "),
    );
    format!("fired [{fired}] joined [{joined}] left [{left}]")
}

async fn a_set_names_a_set_and_follows_it(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), lake(store.as_ref()).await?);
    let own = &l.own;
    // Union, intersection, difference, and a set two levels up.
    let first = s.head().await.must("the head")?.next();
    for (name, filter, members) in [
        (
            "either",
            "in_set('open') or in_set('big')",
            "i1 i2 i3 i4 i5",
        ),
        ("both", "in_set('open') and in_set('big')", "i2 i5"),
        ("small", "in_set('open') and not in_set('big')", "i1"),
        ("top", "in_set('both') and amount > 500", "i5"),
    ] {
        let from = s.head().await.must("the head")?.next();
        l.declare_as(set(name, filter), name).await?;
        let joined = changes(s, from).await?;
        let want: Vec<_> = members.split(' ').map(|k| format!("+{name}:{k}")).collect();
        let read = found(&l, own, &format!("in_set('{name}')"), None).await;
        ensure!(
            joined == want.join(" ") && read.as_deref().ok() == Some(members),
            "`{name}`, declared as `{filter}`, holds {members}; its declaration changed \
             {joined} and a read naming it answered {read:?}"
        );
    }
    let events = events_from(s, Position(0)).await?;
    let formats: Vec<_> = events.iter().map(|e| (e.position, e.v)).collect();
    ensure!(
        formats.iter().all(|(p, v)| (*v == 11) == (*p >= first)),
        "the lake writes format 11 from the first declaration that names a set, at {first}, and \
         not before; its events carry {formats:?}"
    );
    // `open` declared again takes `i4`: the sets built on it follow in the
    // same append, two levels up, and a read as of before reads as before.
    let before = s.head().await.must("the head")?;
    // In one bundle with `big`, which `both` names too: each set built on
    // them is decided once.
    let wider = set("open", "status = 'open' or status = 'held'");
    let bundle = vec![wider, set("big", "amount > 150")];
    l.cat.apply(own, bundle).await.must("open and big again")?;
    let moved = changes(s, before.next()).await?;
    let then = found(&l, own, "in_set('both')", Some(before)).await;
    let now = found(&l, own, "in_set('both')", None).await;
    ensure!(
        moved == "+both:i4 +open:i4 +top:i4"
            && then.as_deref().ok() == Some("i2 i5")
            && now.as_deref().ok() == Some("i2 i4 i5"),
        "`open` declared again to hold `i4` joins it to `open`, `both` and `top` in one append, \
         and `both` read as of before holds `i2` and `i5`; the append changed {moved}, the \
         reads answered {then:?} and {now:?}"
    );
    // A write joins and leaves by the same filters, and a trigger whose
    // `when` names a set fires as the set's filter holds.
    let watch = trigger_on("watch", TYPE, Some("in_set('top')"));
    l.declare_as(watch, "a trigger").await?;
    let up = set_off(&l.upsert(TYPE, "i1", json!({"amount": 600})).await?);
    let down = set_off(&l.upsert(TYPE, "i1", json!({"amount": 5})).await?);
    ensure!(
        up == "fired [watch] joined [big both top] left [small]"
            && down == "fired [] joined [small] left [big both top]",
        "`i1` at 600 joins `big`, `both` and `top`, leaves `small` and fires `watch`; back at 5 \
         it does the reverse and fires nothing; the writes answered {up} and {down}"
    );
    Ok(())
}

async fn a_set_that_cannot_be_named_and_a_path_that_cannot_be_walked_are_refused(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), lake(store.as_ref()).await?);
    let Lake { cat, own } = &l;
    let named = object_set_on("named", "partner", "name is not null");
    l.declare_as(named, "named").await?;
    let both = set("both", "in_set('open') and in_set('big')");
    l.declare_as(both, "both").await?;
    let noted = set("noted", "meta is not null and in_set('open')");
    l.declare_as(noted, "noted").await?;
    let before = s.head().await.must("the head")?;
    let walk = "steps into `partner`, which is a reference";
    let through = trigger_on("t", TYPE, Some("partner.name = 'x'"));
    for (d, names) in [
        (set("s", "in_set('ghost')"), "is not declared"),
        (set("s", "in_set('named')"), "a set of `partner`, not of"),
        (set("open", "in_set('both')"), "names it in turn"),
        (set("s", "in_set(open)"), "in_set"),
        (set("s", "partner.name = 'x'"), walk),
        (set("s", "partner.name is null"), walk),
        (through, walk),
        (set("s", "ghost = 1 and in_set('big')"), "reads `ghost`"),
        (
            trigger_on("t", TYPE, Some("ghost = 1 and in_set('big')")),
            "reads `ghost`",
        ),
        (set("s", "status.x = 'open'"), "is no `json` field"),
    ] {
        let what = format!("declaring {d:?}");
        invalid_naming(&what, cat.declare(own, d).await, &[names])?;
    }
    let ty = TypeName::from(TYPE);
    let nowhere = "has meaning in an object set's `where`";
    for (expr, names) in [("in_set('open')", nowhere), ("partner.name", walk)] {
        let added = cat.add_field(own, &ty, derived("d", Text, expr, &[])).await;
        invalid_naming(&format!("a derived field `{expr}`"), added, &[names])?;
    }
    for (filter, names) in [
        ("partner.name = 'x'", walk),
        ("in_set('ghost')", "is not declared"),
    ] {
        let read = found(&l, own, filter, None).await;
        invalid_naming(&format!("a read where `{filter}`"), read, &[names])?;
    }
    // A migration keeps what a set reads beside the sets it names.
    let mut without = type_decl(
        TYPE,
        vec![reference("partner", "partner"), field("status", Text)],
    );
    without.fields.push(field("amount", Number));
    let taken = cat.migrate(own, migration(without)).await;
    invalid_naming(
        "a migration that takes `meta` away",
        taken,
        &["noted", "meta"],
    )?;
    let other = object_set_on("open", "partner", "name = 'x'");
    let other = cat.declare(own, other).await;
    ensure!(
        matches!(refused_on(&other), Some(("of", Refusal::Invalid { .. }))),
        "`open` declared again for `partner` is refused on `of`; it answered {other:?}"
    );
    head_unmoved(s, before, "each refused declaration").await?;
    // What may be walked is: a `json` field's members, and a reference's key.
    l.upsert(TYPE, "i1", json!({"meta": {"a": 1}})).await?;
    let member = found(&l, own, "meta.a = 1", None).await;
    let key = found(&l, own, "partner = 'p1'", None).await;
    ensure!(
        member.as_deref().ok() == Some("i1") && key.as_deref().ok() == Some("i1 i2 i3 i4 i5"),
        "a path into a `json` field reads its member and a reference compares its key; the \
         reads answered {member:?} and {key:?}"
    );
    // `open` without `i5` changes one membership of `open` and one of
    // `both`: two, past a ceiling of one, which `both` passes.
    let lower = Limits {
        max_set_changes: 1,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a lower ceiling: {e}")))?;
    let l = Lake::of(store.as_ref());
    let before = store.head().await.must("the head")?;
    let narrower = set("open", "status = 'open' and amount < 900");
    let r = l.cat.declare(&l.own, narrower).await;
    ensure!(
        matches!(refused_on(&r), Some(("both", Refusal::TooLarge { max: 1 }))),
        "`open` declared again changes one membership of its own and one of `both`, past a \
         `max_set_changes` of 1: refused too_large naming `both`; it answered {r:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused declaration").await
}

async fn a_named_set_is_read_by_the_right_to_read_what_its_filter_reads(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let (s, l) = (store.as_ref(), lake(store.as_ref()).await?);
    let both = set("both", "in_set('open') and in_set('big')");
    l.declare_as(both, "both").await?;
    // `kept` reads everything of an invoice but `status`, and writes `amount`.
    let grants = [("status", Access::None), ("amount", Access::Write)];
    let kept = field_access(s, "kept", TYPE, &grants);
    let said = |r: lakelands_api::Result<String>, name: &str| {
        r.err().map(|e| e.to_string().replace(name, "ghost"))
    };
    let ghost = said(found(&l, &kept, "in_set('ghost')", None).await, "ghost");
    for name in ["open", "both"] {
        let read = found(&l, &kept, &format!("in_set('{name}')"), None).await;
        let hidden = said(read, name);
        ensure!(
            hidden.is_some() && hidden == ghost,
            "`{name}` reads `status`, which `kept` is kept from: a read naming it answers as \
             for a set that is not declared, {ghost:?}; it answered {hidden:?}"
        );
    }
    let big = found(&l, &kept, "in_set('big')", None).await;
    let described = l.cat.describe(&kept, Default::default()).await;
    let described = described.must("describing the lake to `kept`")?.data;
    let listed: Vec<&str> = described
        .object_sets
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let more = crate::conformance::fixture::patch(json!({"amount": 600}));
    let ty = TypeName::from(TYPE);
    let wrote = l
        .cat
        .upsert(&kept, &ty, &"i1".into(), more, Default::default())
        .await;
    let told = wrote.as_ref().ok().map(set_off);
    ensure!(
        big.as_deref().ok() == Some("i2 i3 i4 i5")
            && listed == ["big"]
            && told.as_deref() == Some("fired [] joined [big] left []"),
        "`kept` reads `big`, is listed `big` alone, and its write that joins `big` and `both` \
         is told of `big` alone; the read answered {big:?}, describe listed {listed:?}, the \
         write answered {wrote:?}"
    );
    // `clerk` reads every field: it is listed each set, and told of both
    // sets its write leaves.
    let clerk = field_access(s, "clerk", TYPE, &[("amount", Access::Write)]);
    let described = l.cat.describe(&clerk, Default::default()).await;
    let described = described.must("describing the lake to `clerk`")?.data;
    let listed: Vec<&str> = described
        .object_sets
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let less = crate::conformance::fixture::patch(json!({"amount": 5}));
    let wrote = l
        .cat
        .upsert(&clerk, &ty, &"i1".into(), less, Default::default())
        .await;
    let told = wrote.as_ref().ok().map(set_off);
    ensure!(
        listed == ["big", "both", "open"]
            && told.as_deref() == Some("fired [] joined [] left [big both]"),
        "`clerk` reads what every set reads: it is listed all three and told that `i1` left \
         `big` and `both`; describe listed {listed:?}, the write answered {wrote:?}"
    );
    // An author kept from `status` declares no set that names `open`.
    let mut author = kept.clone();
    author.grants.schema = true;
    let before = s.head().await.must("the head")?;
    let mine = l.cat.declare(&author, set("mine", "in_set('open')")).await;
    ensure!(
        mine.is_err(),
        "an author kept from `status` declares no set naming `open`; it answered {mine:?}"
    );
    head_unmoved(s, before, "the refused declaration").await
}
