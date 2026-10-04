//! A type is re-keyed by a migration, with everything that names its
//! records (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! `docs/log-format.md` §2.3 and §6).
//!
//! The fixture: `account` keyed by `number`, re-keyed to `iban`; `payment`
//! refers to an account (`pays`, via its `account` field); `entry` is keyed
//! by its account and a sequence number, so it is re-keyed with it; and an
//! account's `owner` is a `person`, carried by link records keyed by both
//! ends.

use std::sync::Arc;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::query::{Projection, QueryResult, Walk};
use lakelands_api::write::{Declaration, Migration, MigrationReport};
use lakelands_api::{
    Access, Catalog, Error, LakeStore, Position, Principal, Refusal, TypeName, WriteOpts,
};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, all, events_after, field_access, get_at, head_unmoved, invalid_naming, log_events,
    migration, patch, pattern, reason, restrict, step,
};

check!(
    /// The preflight counts everything a re-key moves.
    PREFLIGHT_COUNTS = a_rekeys_preflight_counts_what_moves_with_it_and_appends_nothing,
    "027: the preflight of a re-key counts the records it moves, retired and restricted included, the types keyed by a reference to them, their link records, and the reference values it rewrites, and appends nothing"
);

check!(
    /// Colliding new keys refuse the re-key.
    COLLISION = two_records_that_would_take_one_new_key_refuse_the_rekey,
    "027: a re-key under which two records would take one key is not lossless, names the key, and is refused, appending nothing"
);

check!(
    /// A missing key value refuses the re-key.
    MISSING = a_record_without_a_new_key_value_refuses_the_rekey,
    "027: a re-key under which a record has no value for a new key field is not lossless"
);

check!(
    /// The records move with their values, versions and provenance.
    MOVES = a_rekey_moves_every_record_with_its_values_version_and_provenance,
    "027 and log-format §2.3: a re-key moves every record to its new key with its fields, version and per-field provenance; at the migration's position the new key is found and the old one not, and a read before it still finds the old key"
);

check!(
    /// References follow.
    REFERENCES = a_reference_to_a_rekeyed_record_names_its_new_key,
    "027 and 032: a reference field in another type that named a moved key names the new one, and a walk over its relation reaches the record"
);

check!(
    /// Links follow.
    LINKS = a_link_to_a_rekeyed_record_is_rekeyed_and_walked,
    "027 and 035: a link record keyed by a moved end is moved to its new pair key, and a walk over its relation reaches the re-keyed record"
);

check!(
    /// A type keyed by a reference is re-keyed too.
    CLOSURE = a_type_keyed_by_a_reference_to_a_rekeyed_type_is_rekeyed_with_it,
    "027: a type whose key fields reference a re-keyed type is re-keyed in the same migration"
);

check!(
    /// The old key is ended.
    OLD_KEY_ENDED = an_old_key_is_ended_and_a_new_write_follows_the_new_rule,
    "027 and log-format §2.3: after a re-key a write at an old key whose key fields make another key is refused as invalid, naming it; a write at a new key continues its version; and a write of the bare old key string, its key field filled from it, is a new record at version 1"
);

check!(
    /// A re-key is format version 2.
    FORMAT_TWO = a_rekey_and_every_event_after_it_are_format_version_2,
    "log-format §6: the re-keying `migrated` event and every event after it carry `v` 2; the log verifies, and a rebuild answers every read as before"
);

check!(
    /// A lake without a re-key stays at version 1.
    FORMAT_ONE = a_lake_that_never_rekeys_writes_format_version_1,
    "log-format §6: a lake in which nothing is re-keyed carries `v` 1 on every event, a migration that keeps the key included"
);

check!(
    /// A key cannot be taken away or made of what cannot key.
    NO_KEY = a_migration_that_takes_every_key_field_away_is_refused,
    "027: a migration is refused as invalid when it takes every key field away, names a key field that is not required or of a kind a key cannot be made of, or would re-key a view"
);

check!(
    /// A re-key report is of the lake as it stood.
    STALE = a_rekey_report_from_before_a_write_is_refused,
    "027: a re-key is applied only against the report of the lake as it stands; a write between the preflight and the apply, one that changes a move, makes it refused"
);

check!(
    /// Set membership moves with the record.
    SETS = object_set_membership_moves_with_a_rekeyed_record,
    "027 and 004: a moved record keeps its object-set memberships at its new key: a write that keeps it in a set changes nothing, and retiring it leaves the set at the new key"
);

check!(
    /// A re-keyed type migrates again.
    AGAIN = a_rekeyed_type_migrates_again,
    "027 and log-format §2.3: the ended keys a re-key leaves are no records: a later migration that keeps the key, a value-type tightening, and a second re-key over them all apply"
);

check!(
    /// A re-key that retypes a link field moves and converts its links.
    LINK_RESHAPED = a_rekey_that_retypes_a_link_field_moves_and_converts_its_links,
    "027 and 035: a migration that re-keys a type and retypes a field of its relation appends the link type's migration after its own; a link is at its new pair key from the first on and holds the converted value from the second on"
);

check!(
    /// A key is made after the keys it reads.
    KEY_ORDER = a_key_is_made_after_every_key_it_reads_has_moved,
    "027: a type keyed by references to two types a re-key moves, a link type among them, takes its new keys after both have theirs, whatever the order of their names: each key is what its fields make, a relation carried by links is walked out and in as before, and the report lists the types in the order their keys are made"
);

check!(
    /// A ring of keys is refused.
    KEY_RING = a_rekey_of_keys_made_of_keys_it_moves_is_refused,
    "027: a re-key that would move a type keyed by a reference to itself, or types keyed by each other, is refused as invalid naming them, and appends nothing: no order makes such keys"
);

check!(
    /// A renamed reference field is counted.
    RENAMED_REFERENCE = a_rekeys_report_counts_a_reference_field_the_migration_renames,
    "027, 013 and 024: the references a re-key rewrites are counted for a field the same migration renames, under its new name, and that entry is answered only to a reader who may know of the field in the shape the migration leaves"
);

fn account(key: &str, iban_required: bool) -> Value {
    json!({"name": "account", "key": [key],
           "fields": [
               {"name": "number", "type": {"kind": "text"}, "required": true},
               {"name": "iban", "type": {"kind": "text"}, "required": iban_required},
               {"name": "holder", "type": {"kind": "text"}},
               {"name": "kind", "type": {"kind": "text", "value_type": "account_kind"}},
               {"name": "meta", "type": {"kind": "json"}, "required": true}],
           "relations": [{"name": "owner", "target": "person"}]})
}

/// `account` with its key made of `key`, as a migration names it.
fn account_to(key: &str) -> Migration {
    let mut to: serde_json::Map<String, Value> =
        account(key, true).as_object().cloned().unwrap_or_default();
    to.remove("declare");
    Migration {
        to: serde_json::from_value(Value::Object(to)).expect("a type"),
        renames: vec![],
    }
}

pub(super) fn to_iban() -> Migration {
    account_to("iban")
}

/// The fixture, with accounts a1 (DE1), a2 (DE2), a3 (DE3, retired), a4
/// (DE4, restricted); payment p1 to a1; entry a1/1; a1 owned by x. `iban`
/// is optional before the migration, so `missing` can leave one out.
pub(super) async fn lake(store: &dyn LakeStore, ibans: [Option<&str>; 4]) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    for d in [
        json!({"declare": "value_type", "name": "account_kind", "kind": "text",
               "one_of": ["giro", "savings", "old"]}),
        json!({"declare": "type", "name": "person", "key": ["name"],
               "fields": [{"name": "name", "type": {"kind": "text"}, "required": true}]}),
        {
            let mut a = account("number", false);
            a["declare"] = json!("type");
            a
        },
        json!({"declare": "type", "name": "payment",
               "fields": [{"name": "account", "type": {"kind": "reference", "target": "account"}},
                          {"name": "amount", "type": {"kind": "number"}}],
               "relations": [{"name": "pays", "target": "account", "via": "account"}]}),
        json!({"declare": "type", "name": "entry", "key": ["account", "seq"],
               "fields": [{"name": "account", "type": {"kind": "reference", "target": "account"}, "required": true},
                          {"name": "seq", "type": {"kind": "number"}, "required": true},
                          {"name": "memo", "type": {"kind": "text"}}]}),
    ] {
        l.declare_json(d).await?;
    }
    l.upsert("person", "x", json!({"name": "x"})).await?;
    for (i, iban) in ibans.iter().enumerate() {
        let n = (i + 1).to_string();
        let mut fields =
            json!({"number": n, "holder": format!("holder {n}"), "kind": "giro", "meta": {"n": n}});
        if let Some(iban) = iban {
            fields["iban"] = json!(iban);
        }
        l.upsert("account", &n, fields).await?;
    }
    l.retire("account", "3").await?;
    restrict(store, "account", "4", "a retention hold").await?;
    l.upsert("payment", "p1", json!({"account": "1", "amount": 5}))
        .await?;
    l.upsert(
        "entry",
        "1:1",
        json!({"account": "1", "seq": 1, "memo": "m"}),
    )
    .await?;
    cat.link(
        own,
        &"account".into(),
        &"1".into(),
        &"owner".into(),
        &"x".into(),
        patch(json!({})),
        WriteOpts::default(),
    )
    .await
    .must("linking account 1 to x")?;
    Ok(l)
}

pub(super) const IBANS: [Option<&str>; 4] = [Some("DE1"), Some("DE2"), Some("DE3"), Some("DE4")];

async fn preflight(cat: &dyn Catalog, who: &Principal) -> Result<MigrationReport, Failure> {
    Ok(cat
        .migrate(who, to_iban())
        .await
        .must("running the re-key's preflight")?
        .data)
}

pub(super) async fn apply(store: &dyn LakeStore) -> Result<Position, Failure> {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    let r = preflight(cat.as_ref(), own).await?;
    ensure!(
        r.lossless,
        "the re-key is lossless, and the report says {r:?}"
    );
    Ok(cat
        .migrate_apply(own, to_iban(), &r.report_id)
        .await
        .must("applying the re-key")?
        .position())
}

async fn found(
    l: &Lake,
    ty: &str,
    key: &str,
    as_of: Option<Position>,
) -> Result<Option<lakelands_api::Record>, Failure> {
    match l
        .cat
        .get(&l.own, &TypeName::from(ty), get_at(key, as_of))
        .await
    {
        Ok(r) => Ok(Some(r.data)),
        Err(Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(Failure::new(format!("reading `{ty}` `{key}`: {e}"))),
    }
}

async fn walked(
    l: &Lake,
    ty: &str,
    key: &str,
    relation: &str,
    walk: Walk,
) -> Result<Vec<String>, Failure> {
    let p = pattern(ty, Some(key), vec![step(relation, walk)], Projection::Graph);
    let reply = l
        .cat
        .query(&l.own, p)
        .await
        .must(&format!("walking `{relation}` from `{ty}` `{key}`"))?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("a graph projection answered something else"));
    };
    Ok(nodes
        .into_iter()
        .filter(|n| n.ty.as_str() != ty)
        .map(|n| n.key.to_string())
        .collect())
}

async fn a_rekeys_preflight_counts_what_moves_with_it_and_appends_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let r = preflight(cat.as_ref(), own).await?;
    let moved: std::collections::BTreeSet<(String, u64)> = r
        .rekeyed
        .iter()
        .map(|k| (k.ty.to_string(), k.records))
        .collect();
    ensure!(
        r.lossless
            && moved
                == std::collections::BTreeSet::from([
                    ("account".into(), 4),
                    ("entry".into(), 1),
                    ("sys.link.account.owner".into(), 1)
                ]),
        "all four accounts move, the retired and the restricted one included, with the entry \
         keyed by one and the link from one; the report says {r:?}"
    );
    let payment = r
        .references
        .iter()
        .find(|x| x.ty.as_str() == "payment" && x.field.as_str() == "account");
    ensure!(
        payment.is_some_and(|x| x.values == 1),
        "the payment's reference to account 1 is rewritten; the report says {:?}",
        r.references
    );
    head_unmoved(store.as_ref(), before, "a re-key's preflight").await
}

async fn two_records_that_would_take_one_new_key_refuse_the_rekey(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(
        store.as_ref(),
        [Some("DE1"), Some("DE1"), Some("DE3"), Some("DE4")],
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let r = preflight(cat.as_ref(), own).await?;
    let collided = r
        .rekeyed
        .iter()
        .find(|k| k.ty.as_str() == "account")
        .map(|k| k.collisions);
    ensure!(
        !r.lossless
            && collided == Some(2)
            && r.sample_collisions
                .iter()
                .any(|c| c.ty.as_str() == "account" && c.key.as_str() == "DE1"),
        "accounts 1 and 2 would both be `DE1`; the report says {r:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate_apply(own, to_iban(), &r.report_id)
        .await
        .must_fail("applying a re-key with a collision")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Migration { .. })),
        "the apply was refused as {e:?}; a migration refusal with its report was expected"
    );
    head_unmoved(store.as_ref(), before, "a re-key with a collision").await
}

async fn a_record_without_a_new_key_value_refuses_the_rekey(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(
        store.as_ref(),
        [Some("DE1"), None, Some("DE3"), Some("DE4")],
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let r = preflight(cat.as_ref(), own).await?;
    let missing = r
        .rekeyed
        .iter()
        .find(|k| k.ty.as_str() == "account")
        .map(|k| k.missing);
    ensure!(
        !r.lossless && missing == Some(1),
        "account 2 has no `iban`; the report says {r:?}"
    );
    Ok(())
}

async fn a_rekey_moves_every_record_with_its_values_version_and_provenance(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let old = found(&l, "account", "1", None)
        .await?
        .ok_or_else(|| Failure::new("account 1 is not found before the re-key"))?;
    let at = apply(store.as_ref()).await?;
    let moved = found(&l, "account", "DE1", Some(at))
        .await?
        .ok_or_else(|| Failure::new("`DE1` is not found at the migration's position"))?;
    ensure!(
        moved.fields == old.fields && moved.version == old.version && moved.meta == old.meta,
        "account 1 moved to `DE1` changed: {old:?} became {moved:?}"
    );
    ensure!(
        found(&l, "account", "1", Some(at)).await?.is_none()
            && found(&l, "account", "1", None).await?.is_none(),
        "the old key `1` is still found at or after the migration"
    );
    let earlier = Position(at.0 - 1);
    ensure!(
        found(&l, "account", "1", Some(earlier)).await?.is_some()
            && found(&l, "account", "DE1", Some(earlier)).await?.is_none(),
        "as of {earlier}, before the migration, `1` is found and `DE1` is not"
    );
    Ok(())
}

async fn a_reference_to_a_rekeyed_record_names_its_new_key(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    apply(store.as_ref()).await?;
    let p1 = found(&l, "payment", "p1", None)
        .await?
        .ok_or_else(|| Failure::new("payment p1 is not found after the re-key"))?;
    ensure!(
        p1.fields.get("account") == Some(&json!("DE1")),
        "p1's account is {:?}; `DE1` was expected",
        p1.fields.get("account")
    );
    let reached = walked(&l, "payment", "p1", "pays", Walk::Out).await?;
    ensure!(
        reached == ["DE1"],
        "the walk over `pays` reached {reached:?}"
    );
    Ok(())
}

async fn a_link_to_a_rekeyed_record_is_rekeyed_and_walked(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    apply(store.as_ref()).await?;
    let lt = "sys.link.account.owner";
    ensure!(
        found(&l, lt, "DE1:x", None).await?.is_some()
            && found(&l, lt, "1:x", None).await?.is_none(),
        "the link from account 1 to x was not moved to `DE1:x`"
    );
    let reached = walked(&l, "account", "DE1", "owner", Walk::Out).await?;
    ensure!(
        reached == ["x"],
        "the walk over `owner` from `DE1` reached {reached:?}"
    );
    Ok(())
}

async fn a_rekey_that_retypes_a_link_field_moves_and_converts_its_links(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    // `owner` with a `rank`, of `kind`, on an account keyed by `key`.
    let ranked = |key: &str, kind: &str| {
        let mut a = account(key, key == "iban");
        a["relations"][0]["fields"] = json!([{"name": "rank", "type": {"kind": kind}}]);
        a
    };
    let mut first = ranked("number", "number");
    first["declare"] = json!("type");
    l.declare_json(first).await?;
    let (account, owner) = ("account".into(), "owner".into());
    let rank = patch(json!({"rank": 2}));
    cat.link(
        own,
        &account,
        &"1".into(),
        &owner,
        &"x".into(),
        rank,
        WriteOpts::default(),
    )
    .await
    .must("giving the link from account 1 to x a rank")?;
    let to = serde_json::from_value(ranked("iban", "text")).expect("a type");
    let applied = l.migrate(to).await?.position();
    let lt = "sys.link.account.owner";
    // The link type's migration is the event after the account's.
    for (at, rank) in [(Some(applied), json!(2)), (None, json!("2"))] {
        let link = found(&l, lt, "DE1:x", at).await?;
        ensure!(
            link.as_ref()
                .is_some_and(|r| r.fields.get("rank") == Some(&rank))
                && found(&l, lt, "1:x", at).await?.is_none(),
            "a migration that re-keys `account` and retypes `rank` to text moves the link to \
             `DE1:x` at its own position and converts its rank to \"2\" at the next; as of \
             {at:?} it is {link:?}"
        );
    }
    Ok(())
}

async fn a_type_keyed_by_a_reference_to_a_rekeyed_type_is_rekeyed_with_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    apply(store.as_ref()).await?;
    let e = found(&l, "entry", "DE1:1", None).await?;
    ensure!(
        e.as_ref()
            .is_some_and(|e| e.fields.get("account") == Some(&json!("DE1"))
                && e.fields.get("memo") == Some(&json!("m")))
            && found(&l, "entry", "1:1", None).await?.is_none(),
        "the entry keyed by account 1 is not at `DE1:1` with its values: {e:?}"
    );
    Ok(())
}

async fn an_old_key_is_ended_and_a_new_write_follows_the_new_rule(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    let v = found(&l, "account", "1", None)
        .await?
        .map(|r| r.version)
        .ok_or_else(|| Failure::new("account 1 is not found"))?;
    apply(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .upsert(
            own,
            &"account".into(),
            &"1".into(),
            patch(json!({"iban": "DE9"})),
            WriteOpts::default(),
        )
        .await
        .must_fail("writing at the old key `1` with `iban` DE9")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Invalid { message }) if message.contains("DE9")),
        "a write at an old key whose key field makes `DE9` was answered {e:?}; invalid, naming \
         the key the key fields make, was expected"
    );
    head_unmoved(store.as_ref(), before, "a write at an old key").await?;
    let at_new = l.upsert("account", "DE1", json!({"holder": "h"})).await?;
    let now = found(&l, "account", "DE1", None).await?.map(|r| r.version);
    ensure!(
        now.is_some_and(|n| n.0 == v.0 + 1),
        "a write at `DE1` answered {at_new:?}; the moved record's version {} was to continue",
        v.0
    );
    // The bare old key string, with no key field in the patch: the key
    // field is filled from the key, and it is a new record at version 1.
    l.upsert(
        "account",
        "1",
        json!({"number": "9", "meta": {}, "holder": "new"}),
    )
    .await?;
    let fresh = found(&l, "account", "1", None).await?;
    ensure!(
        fresh
            .as_ref()
            .is_some_and(|r| r.version.0 == 1 && r.fields.get("iban") == Some(&json!("1"))),
        "a write at the ended key `1` without a key field made {fresh:?}; a new record at \
         version 1 with `iban` 1 was expected"
    );
    Ok(())
}

async fn a_rekey_and_every_event_after_it_are_format_version_2(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let read = |ty: &'static str, key: &'static str| found(&l, ty, key, None);
    let at = apply(store.as_ref()).await?;
    l.upsert("account", "DE2", json!({"holder": "after"}))
        .await?;
    let all = log_events(store.as_ref()).await?;
    let wrong: Vec<(u64, u32)> = all
        .iter()
        .filter(|e| (e.position >= at) != (e.v == 2))
        .map(|e| (e.position.0, e.v))
        .collect();
    ensure!(
        wrong.is_empty()
            && all.iter().any(|e| e.position == at
                && matches!(&e.body, EventBody::Migrated { rekeyed, .. } if !rekeyed.is_empty())),
        "events before the re-key at {at} carry `v` 1 and it and every later one `v` 2; these do \
         not: {wrong:?}"
    );
    store
        .verify(None, None)
        .await
        .must("verifying a log that turns to format 2")?;
    let before = (read("account", "DE1").await?, read("entry", "DE1:1").await?);
    store.rebuild().await.must("rebuilding")?;
    let after = (read("account", "DE1").await?, read("entry", "DE1:1").await?);
    ensure!(
        before == after,
        "a rebuild answers differently: {before:?} became {after:?}"
    );
    Ok(())
}

async fn a_lake_that_never_rekeys_writes_format_version_1(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    // A migration that keeps the key.
    let mut to: serde_json::Map<String, Value> = account("number", false)
        .as_object()
        .cloned()
        .unwrap_or_default();
    to["fields"]
        .as_array_mut()
        .expect("fields")
        .push(json!({"name": "note", "type": {"kind": "text"}}));
    l.migrate(serde_json::from_value(Value::Object(to)).expect("a type"))
        .await?;
    let newer: Vec<(u64, u32)> = log_events(store.as_ref())
        .await?
        .iter()
        .filter(|e| e.v != 1)
        .map(|e| (e.position.0, e.v))
        .collect();
    ensure!(
        newer.is_empty(),
        "a lake that never re-keyed wrote these events in another format: {newer:?}"
    );
    Ok(())
}

async fn a_migration_that_takes_every_key_field_away_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    let mut to: serde_json::Map<String, Value> = account("number", false)
        .as_object()
        .cloned()
        .unwrap_or_default();
    to.insert("key".into(), json!([]));
    let m = migration(serde_json::from_value(Value::Object(to)).expect("a type"));
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate(own, m)
        .await
        .must_fail("migrating `account` to no key fields")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Invalid { .. })),
        "taking every key field away was refused as {e:?}; invalid was expected"
    );
    // A key field that is not required, and one of a kind no key is made of.
    for (key, what) in [
        ("holder", "a key field that is not required"),
        ("meta", "a `json` key field"),
    ] {
        let e = cat
            .migrate(own, account_to(key))
            .await
            .must_fail(&format!("migrating `account` to {what}"))?;
        ensure!(
            matches!(reason(&e), Some(Refusal::Invalid { .. })),
            "{what} was refused as {e:?}; invalid was expected"
        );
    }
    head_unmoved(store.as_ref(), before, "refused key changes").await?;
    // A view keyed by a reference to the type would be re-keyed with it.
    l.declare_json(
        json!({"declare": "type", "name": "per_account", "key": ["account"],
        "fields": [{"name": "account", "type": {"kind": "reference", "target": "account"}},
                   {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "payment", "group_by": ["account"],
                 "measures": [{"name": "n", "expr": "count()"}]}}),
    )
    .await?;
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate(own, to_iban())
        .await
        .must_fail("re-keying `account` under a view keyed by it")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Invalid { message }) if message.contains("per_account")),
        "a re-key under a view keyed by the type was answered {e:?}; invalid, naming the \
         view, was expected"
    );
    head_unmoved(store.as_ref(), before, "refused migrations").await
}

async fn a_rekey_report_from_before_a_write_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    let r = preflight(cat.as_ref(), own).await?;
    // Account 2 would move to `DE5` now, not `DE2`.
    l.upsert("account", "2", json!({"iban": "DE5"})).await?;
    let before = store.head().await.must("reading the head")?;
    let e = cat
        .migrate_apply(own, to_iban(), &r.report_id)
        .await
        .must_fail("applying a re-key report from before a write")?;
    ensure!(
        matches!(reason(&e), Some(Refusal::Invalid { .. })),
        "a stale re-key report was answered {e:?}; invalid was expected"
    );
    head_unmoved(store.as_ref(), before, "a stale re-key report").await
}

fn set_changes(events: &[Event]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ObjectSetChanged { key, joined, .. } => Some((key.to_string(), *joined)),
            _ => None,
        })
        .collect()
}

async fn object_set_membership_moves_with_a_rekeyed_record(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    l.declare_json(
        json!({"declare": "object_set", "name": "held", "of": "account",
                          "where": "holder is not null"}),
    )
    .await?;
    // Written again after the set exists, so account 1 is in it.
    l.upsert("account", "1", json!({"holder": "holder 1b"}))
        .await?;
    apply(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    l.upsert("account", "DE1", json!({"holder": "still held"}))
        .await?;
    let after_write = events_after(store.as_ref(), before).await?;
    ensure!(
        set_changes(&after_write).is_empty(),
        "a write that keeps `DE1` in `held` changed the set: {:?}",
        set_changes(&after_write)
    );
    let before = store.head().await.must("reading the head")?;
    l.retire("account", "DE1").await?;
    let after_retire = events_after(store.as_ref(), before).await?;
    ensure!(
        set_changes(&after_retire) == [("DE1".to_owned(), false)],
        "retiring `DE1` changed `held` as {:?}; it leaves it, at `DE1`",
        set_changes(&after_retire)
    );
    Ok(())
}

async fn a_rekeyed_type_migrates_again(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = lake(store.as_ref(), IBANS).await?;
    let Lake { cat, own } = &l;
    apply(store.as_ref()).await?;
    // A migration that keeps the key: a field added.
    let mut keep = account_to("iban");
    keep.to.fields.push(
        serde_json::from_value(json!({"name": "note", "type": {"kind": "text"}})).expect("a field"),
    );
    let r = cat
        .migrate(own, keep.clone())
        .await
        .must("a preflight")?
        .data;
    ensure!(
        r.lossless && r.rekeyed.is_empty(),
        "a migration that keeps the key of a re-keyed type reports {r:?}"
    );
    cat.migrate_apply(own, keep, &r.report_id)
        .await
        .must("a migration that keeps the key, after a re-key")?;
    // A value type the type uses, tightened.
    let kind: lakelands_api::ontology::ValueTypeDecl = serde_json::from_value(
        json!({"name": "account_kind", "kind": "text", "one_of": ["giro", "savings"]}),
    )
    .expect("a value type");
    let v = cat
        .migrate_value_type(own, kind.clone())
        .await
        .must("a value type's preflight")?
        .data;
    ensure!(
        v.fits && v.records_scanned == 4,
        "the four accounts, and no ended key, are scanned: {v:?}"
    );
    cat.migrate_value_type_apply(own, kind, &v.report_id)
        .await
        .must("tightening `account_kind` after a re-key")?;
    // A second re-key, back to `number`, over the ended keys `1` to `4`.
    let mut back = account_to("number");
    back.to.fields.push(
        serde_json::from_value(json!({"name": "note", "type": {"kind": "text"}})).expect("a field"),
    );
    let r = cat
        .migrate(own, back.clone())
        .await
        .must("a preflight")?
        .data;
    ensure!(r.lossless, "a second re-key over ended keys reports {r:?}");
    cat.migrate_apply(own, back, &r.report_id)
        .await
        .must("re-keying `account` back to `number`")?;
    let one = found(&l, "account", "1", None).await?;
    ensure!(
        one.as_ref()
            .is_some_and(|r| r.fields.get("iban") == Some(&json!("DE1"))),
        "account 1 is not back at `1` with its values: {one:?}"
    );
    Ok(())
}

/// A type from JSON.
fn ty(json: Value) -> lakelands_api::ontology::TypeDecl {
    serde_json::from_value(json).expect("a type")
}

/// `acct`, keyed by `key`, with `relations`.
fn acct(key: &str, relations: Value) -> Value {
    json!({"name": "acct", "key": [key], "relations": relations,
           "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                      {"name": "tag", "type": {"kind": "text"}, "required": true}]})
}

/// A type keyed by an `acct` and by `b`: a number, or a reference to `to`.
fn by_acct(name: &str, to: Option<&str>) -> Declaration {
    let b = to.map_or(
        json!({"kind": "number"}),
        |t| json!({"kind": "reference", "target": t}),
    );
    Declaration::Type(ty(json!({"name": name, "key": ["a", "b"],
        "fields": [{"name": "a", "type": {"kind": "reference", "target": "acct"}, "required": true},
                   {"name": "b", "type": b, "required": true}]})))
}

/// The preflight's report of `m`, as `who` is answered it.
async fn report(l: &Lake, who: &Principal, m: &Migration) -> Result<MigrationReport, Failure> {
    let answered = l.cat.migrate(who, m.clone()).await;
    Ok(answered.must("the preflight")?.data)
}

async fn a_key_is_made_after_every_key_it_reads_has_moved(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // `zone` is keyed by an acct; `mid`, `zz` and the links of `zones` by an
    // acct and a zone. `mid` and the link type sort before `zone`.
    let zones = json!([{"name": "zones", "target": "zone", "cardinality": "many_to_many"}]);
    let first = Declaration::Type(ty(acct("code", zones.clone())));
    let rest = [("zone", None), ("mid", Some("zone")), ("zz", Some("zone"))];
    let bundle = [first]
        .into_iter()
        .chain(rest.map(|(n, to)| by_acct(n, to)));
    cat.apply(own, bundle.collect())
        .await
        .must("declaring the lake")?;
    for (ty, key, fields) in [
        ("acct", "a1", json!({"code": "a1", "tag": "T1"})),
        ("acct", "a2", json!({"code": "a2", "tag": "T2"})),
        ("zone", "a1:1", json!({"a": "a1", "b": 1})),
        ("zone", "a2:2", json!({"a": "a2", "b": 2})),
        ("mid", "a1:a2%3A2", json!({"a": "a1", "b": "a2:2"})),
        ("zz", "a1:a2%3A2", json!({"a": "a1", "b": "a2:2"})),
    ] {
        l.upsert(ty, key, fields).await?;
    }
    for (from, to) in [("a1", "a1:1"), ("a1", "a2:2"), ("a2", "a1:1")] {
        let (ty, zones, from, to) = ("acct".into(), "zones".into(), from.into(), to.into());
        let (none, opts) = (patch(json!({})), WriteOpts::default());
        let linked = cat.link(own, &ty, &from, &zones, &to, none, opts).await;
        linked.must("linking an acct to a zone")?;
    }
    let m = migration(ty(acct("tag", zones)));
    let r = report(&l, own, &m).await?;
    let order: Vec<&str> = r.rekeyed.iter().map(|k| k.ty.as_str()).collect();
    let links = "sys.link.acct.zones";
    ensure!(
        r.lossless && order == ["acct", "zone", "mid", links, "zz"],
        "`zone` takes its keys before the three types keyed by it, which follow by name; \
         the report lists {order:?}"
    );
    let applied = cat.migrate_apply(own, m, &r.report_id).await;
    applied.must("applying the re-key")?;
    for (ty, made) in [
        ("zone", vec!["T1:1", "T2:2"]),
        ("mid", vec!["T1:T2%3A2"]),
        ("zz", vec!["T1:T2%3A2"]),
        (links, vec!["T1:T1%3A1", "T1:T2%3A2", "T2:T1%3A1"]),
    ] {
        let found = all(cat.as_ref(), own, ty).await?;
        let found: Vec<_> = found.iter().map(|r| r.key.as_str()).collect();
        ensure!(
            found == made,
            "each key of `{ty}` is what its fields make, {made:?}; its records are at {found:?}"
        );
    }
    let out = walked(&l, "acct", "T1", "zones", Walk::Out).await?;
    let inn = walked(&l, "zone", "T1:1", "zones", Walk::In).await?;
    ensure!(
        out == ["T1:1", "T2:2"] && inn == ["T1", "T2"],
        "`zones` is walked as before the re-key: out from `T1` it reached {out:?}, in to \
         `T1:1` it came from {inn:?}"
    );
    Ok(())
}

async fn a_rekey_of_keys_made_of_keys_it_moves_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // `bush` references itself, and its root names itself; `p` and `q` are
    // keyed by an acct and by each other.
    let bush = |key: Value, required: bool| {
        ty(json!({"name": "bush", "key": key,
            "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                       {"name": "parent", "type": {"kind": "reference", "target": "bush"},
                        "required": required}]}))
    };
    let bundle = vec![
        Declaration::Type(bush(json!(["code"]), false)),
        Declaration::Type(ty(acct("code", json!([])))),
        by_acct("p", Some("q")),
        by_acct("q", Some("p")),
    ];
    cat.apply(own, bundle).await.must("declaring the lake")?;
    l.upsert("bush", "r", json!({"code": "r"})).await?;
    l.upsert("bush", "r", json!({"parent": "r"})).await?;
    l.upsert("acct", "a1", json!({"code": "a1", "tag": "T1"}))
        .await?;
    let before = store.head().await.must("reading the head")?;
    for (what, to, names) in [
        (
            "re-keying `bush` by a reference to itself",
            bush(json!(["parent", "code"]), true),
            vec!["`bush`"],
        ),
        (
            "re-keying `acct`, which `p` and `q` are keyed by",
            ty(acct("tag", json!([]))),
            vec!["`p`", "`q`"],
        ),
    ] {
        invalid_naming(what, cat.migrate(own, migration(to)).await, &names)?;
    }
    head_unmoved(store.as_ref(), before, "a re-key of a ring of keys").await
}

async fn a_rekeys_report_counts_a_reference_field_the_migration_renames(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let node = |key: &str, parent: &str| {
        ty(json!({"name": "node", "key": [key],
            "fields": [{"name": "code", "type": {"kind": "text"}, "required": true},
                       {"name": "tag", "type": {"kind": "text"}, "required": true},
                       {"name": parent, "type": {"kind": "reference", "target": "node"}}]}))
    };
    l.declare(node("code", "parent")).await?;
    for (code, tag, parent) in [
        ("n1", "T1", None),
        ("n2", "T2", Some("n1")),
        ("n3", "T3", Some("n1")),
        ("n4", "T4", Some("n2")),
    ] {
        let fields = json!({"code": code, "tag": tag, "parent": parent});
        l.upsert("node", code, fields).await?;
    }
    // Re-keyed by `tag`, and `parent` renamed to `up`.
    let mut m = migration(node("tag", "up"));
    m.renames = vec![("parent".into(), "up".into())];
    let r = report(&l, own, &m).await?;
    let counted = |r: &MigrationReport| -> Vec<(String, u64)> {
        let entry = |x: &lakelands_api::write::ReferenceRewrite| {
            (format!("{}.{}", x.ty, x.field), x.values)
        };
        r.references.iter().map(entry).collect()
    };
    ensure!(
        counted(&r) == [("node.up".to_owned(), 3)] && !r.more,
        "three values of the renamed `up` are rewritten, and the owner is shown the entry; \
         the report says {:?}, more: {}",
        r.references,
        r.more
    );
    let mut hid = field_access(store.as_ref(), "hid", "node", &[("up", Access::None)]);
    hid.grants.schema = true;
    let shown = report(&l, &hid, &m).await?;
    ensure!(
        shown.references.is_empty() && shown.more && shown.report_id == r.report_id,
        "a reader kept from `up` is shown no entry for it, and that there is more; it was \
         shown {:?}, more: {}",
        shown.references,
        shown.more
    );
    let applied = cat.migrate_apply(own, m, &r.report_id).await;
    applied.must("applying the re-key")?;
    let nodes = all(cat.as_ref(), own, "node").await?;
    let ups: Vec<_> = nodes.iter().filter_map(|n| n.fields.get("up")).collect();
    ensure!(
        ups == [&json!("T1"), &json!("T1"), &json!("T2")],
        "the fold rewrote the three values the report counted; `up` reads {ups:?}"
    );
    Ok(())
}
