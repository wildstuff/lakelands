//! What a stored vector answers, wherever the projection keeps it
//! (`docs/decisions/072-a-vector-is-stored-once.md`): the elements written
//! at a position, bit for bit in the log's number form, at that position
//! and every later one until the vector is written again, a record's other fields changing or not,
//! after a retirement and a forgetting as before, and after a rebuild,
//! which ranks as before by the one cosine (025).

use async_trait::async_trait;
use lakelands_api::query::cosine;
use lakelands_api::seam::EmbedBackend;

use super::embeddings::with_embedder;
use super::erasable::again;
use super::links::{approved_by, invoice, link_ok, unlink, walked};
use crate::conformance::fixture::{find_at, get_at, restrict, source_record};
use crate::conformance::prelude::*;

check!(
    VECTOR_BITS_KEPT = a_vector_reads_the_elements_written_at_each_position_bit_for_bit,
    "072, 001, 009 and 025: a vector a caller writes reads, by `get` and by `find`, at the head and as of every position a write ended at, the elements written last at or before it, bit for bit in the log's number form, an integral element as an integer and `-0` as `0`: doubles that no `f32` holds and elements that are exactly `f32`s, through writes of the record's other fields, a vector written again, a vector cleared, from which on it reads none, and a retirement, as of before which the retired record answers its vector and after which a write of its other fields reads none; a vector search ranks the live records by the cosine of their elements as `f32`s, to the bit; a rebuild answers all of it the same"
);

check!(
    VECTOR_WIDENED_KEPT = a_backends_vector_reads_its_f32_elements_widened_at_each_position,
    "072, 022, extended, 001 and 025: a vector an embedding backend makes reads each `f32` element widened to a double, bit for bit, at the head and as of every position from its fill on, and the vector of the text embedded again from that one on, through a write of the record's other fields and another record's fill; a vector search ranks by the cosine of those elements; a rebuild answers the same",
    reopens
);

check!(
    VECTOR_LINK_KEPT = a_links_vector_reads_through_a_walk_at_each_position,
    "072, 035 and 001: a vector field of a relation carried by link records reads on the walk's edge, at the head and as of every position a write ended at, the elements its link was written with last, bit for bit, through a write of the link's other fields, a vector written again and an unlink, as of before which the edge answers its vector; a rebuild answers the same"
);

check!(
    VECTOR_ERASED_KEPT = a_sealed_vector_reads_its_bits_until_its_source_is_forgotten,
    "072, 058 and 001: a vector of an erasable field a source writes reads its elements bit for bit, at the head and as of every position from its write on, though the log holds it sealed; once the source is forgotten it reads as erased at all of them, its provenance saying so, and is no candidate of a vector search, while another source's vector reads and ranks as before; a rebuild answers the same",
    reopens
);

check!(
    VECTOR_RESTRICTION_KEPT = a_restriction_and_its_lifting_keep_a_records_vector,
    "072, 001 and 025: a restriction of a record that holds a vector, its lifting and a later write of its other fields leave its elements as they were: they read bit for bit, by `get` and by `find`, at the head and as of each of those positions; a vector search ranks the record the same while it is restricted and after; a rebuild answers the same"
);

check!(
    VECTOR_REKEY_ENDED = a_rekey_ends_the_old_keys_vector_at_its_position,
    "072, 027 and 001: a re-key moves a record's vector to its new key: as of before the re-key the old key reads its elements and the new one is no record, from the re-key on the new key reads them, bit for bit, through a write of its other fields, and the old key none; a vector search at the head ranks the new keys only, the old key's vector ended where the re-key ended its state; a rebuild answers the same"
);

check!(
    VECTOR_RENAME_KEPT =
        a_migration_that_renames_a_vector_field_reads_its_elements_under_the_new_name,
    "072, 027 and 001: a migration that renames a vector field reads each record's elements under the old name as of before it and under the new one from it on, bit for bit, by `get` and by `find`, and neither name reads the other's; a vector search of the new name ranks them by the one cosine; a rebuild answers the same"
);

/// Doubles that no `f32` holds.
const DOUBLES: [f64; 3] = [0.1, 1.0 / 3.0, -std::f64::consts::E];
/// Doubles that no `f32` holds, written later over [`DOUBLES`].
const LATER: [f64; 3] = [std::f64::consts::FRAC_1_SQRT_2, -1e-7, 12345.678901234567];
/// Elements that are exactly `f32`s, written as the doubles they are.
const SINGLES: [f32; 3] = [0.1, -0.333_333_34, 1.234_567_9e-5];
/// What the backend makes of `beta`'s text.
const BETA: [f32; 3] = [-0.7, 1e-7, 12_345.679];
/// The query of every ranking here.
const QUERY: [f32; 3] = [1.0, 0.5, -0.25];

/// `v`'s elements widened to doubles, as a backend's vector is written.
fn widened(v: &[f32]) -> Value {
    json!(v.iter().copied().map(f64::from).collect::<Vec<_>>())
}

/// A value's elements as JSON writes them, or `None` where it is no
/// vector. A double's shortest round-trip form names its bits, and an
/// integer is written as one, so equal forms are equal bits in the
/// number form the write gave.
fn forms(v: Option<&Value>) -> Option<Vec<String>> {
    let a = v?.as_array()?;
    a.iter()
        .map(|n| n.is_number().then(|| n.to_string()))
        .collect()
}

/// `key`'s `field` as of `at` by `get` and by `find` over `ty`: the
/// [`forms`] of each, `None` where the record or its vector is absent.
async fn read_both(
    l: &Lake,
    ty: &str,
    field: &str,
    key: &str,
    at: Option<Position>,
) -> Result<[Option<Vec<String>>; 2], Failure> {
    let got = l.cat.get(&l.own, &ty.into(), get_at(key, at)).await;
    let got = match got {
        Ok(r) => forms(r.data.fields.get(field)),
        Err(Error::NotFound { .. }) => None,
        Err(e) => return Err(Failure::new(format!("reading `{key}` as of {at:?}: {e}"))),
    };
    let found = l.cat.find(&l.own, &ty.into(), find_at(at)).await;
    let Found::Records { rows, .. } = found.must(&format!("finding `{ty}` as of {at:?}"))?.data
    else {
        return Err(Failure::new(format!("`{ty}` answers records")));
    };
    let row = rows.iter().find(|r| r.key.0 == key);
    Ok([got, row.and_then(|r| forms(r.fields.get(field)))])
}

/// A key whose vector is read as of a position, and the elements it
/// holds there, if any.
type Read<'a> = (Option<Position>, &'a str, Option<&'a Value>);

/// A write of a key at a position: the elements it leaves, `None` where
/// it leaves the vector as it was, `Some(None)` where it leaves none.
type Wrote<'a> = (Option<Position>, &'a str, Option<Option<&'a Value>>);

/// Every key's elements as of every position `writes` name and the head:
/// the last a write left at or before it, none before the first. Writes
/// at one position, as a migration's are, are read together.
fn grid<'a>(writes: &[Wrote<'a>]) -> Vec<Read<'a>> {
    let mut keys: Vec<&str> = writes.iter().map(|w| w.1).collect();
    keys.sort_unstable();
    keys.dedup();
    let positions = writes.iter().map(|w| w.0).chain([None]);
    let mut reads = Vec::new();
    for at in positions {
        let upto = match at {
            Some(_) => writes.iter().rposition(|w| w.0 == at).map_or(0, |i| i + 1),
            None => writes.len(),
        };
        for key in &keys {
            let upto = writes.iter().take(upto).filter(|w| w.1 == *key);
            let left = upto.filter_map(|w| w.2).next_back().flatten();
            reads.push((at, *key, left));
        }
    }
    reads
}

/// Whether, as of each position, each key's `field` reads by `get` and
/// `find` the elements beside it, in the number form written, or nothing.
async fn holds(l: &Lake, ty: &str, field: &str, want: &[Read<'_>], what: &str) -> Held {
    for (at, key, elements) in want {
        let wrote = forms(*elements);
        let read = read_both(l, ty, field, key, *at).await?;
        ensure!(
            read == [wrote.clone(), wrote.clone()],
            "{what}: as of {at:?}, `{key}`'s `{field}` reads by `get` and `find` the elements \
             {wrote:?}, bit for bit; they read {read:?}"
        );
    }
    Ok(())
}

/// A vector search of `ty`'s `field` for [`QUERY`]: its keys and the
/// bits of their scores.
async fn ranking(l: &Lake, ty: &str, field: &str) -> Result<Vec<(String, u64)>, Failure> {
    let asked = Search {
        ty: TypeName::from(ty),
        ..super::search::doc_search(SearchMode::vector(field, QUERY.to_vec()))
    };
    let hits = l.cat.search(&l.own, asked).await;
    let hits = hits
        .must(&format!("searching `{ty}`'s `{field}`"))?
        .data
        .hits;
    Ok(hits
        .into_iter()
        .map(|h| (h.record.key.0, h.score.to_bits()))
        .collect())
}

/// The ranking of `live` by the one cosine of their elements as `f32`s,
/// best first, ties by key.
fn reference(live: &[(&str, &Value)]) -> Vec<(String, u64)> {
    let element = |x: &Value| x.as_f64().expect("a vector element is a number") as f32;
    let score = |v: &Value| {
        let v = v.as_array().expect("a vector is an array");
        cosine(&QUERY, &v.iter().map(element).collect::<Vec<_>>())
    };
    let mut r: Vec<_> = (live.iter())
        .map(|(k, v)| ((*k).to_owned(), score(v)))
        .collect();
    r.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    r.into_iter().map(|(k, s)| (k, s.to_bits())).collect()
}

/// Whether a vector search of `ty`'s `field` answers the [`reference`]
/// ranking of `live`.
async fn ranks(l: &Lake, ty: &str, field: &str, live: &[(&str, &Value)], what: &str) -> Held {
    let got = ranking(l, ty, field).await?;
    let want = reference(live);
    ensure!(
        got == want,
        "{what}: a vector search of `{field}` answers the keys and score bits of the one \
         cosine over the elements as `f32`s, {want:?}; it answered {got:?}"
    );
    Ok(())
}

async fn a_vector_reads_the_elements_written_at_each_position_bit_for_bit(store: Store) -> Held {
    ensure!(
        DOUBLES
            .iter()
            .chain(&LATER)
            .all(|x| f64::from(*x as f32) != *x),
        "the suite's doubles are none of them an `f32`"
    );
    let l = Lake::of(store.as_ref());
    let pin = type_decl("pin", vec![vector("v", 3), field("tag", StorageKind::Text)]);
    l.declare(pin).await?;
    let (doubles, later, singles) = (json!(DOUBLES), json!(LATER), widened(&SINGLES));
    // Integral doubles and `-0`, read in the log's number form.
    let integral = json!([2, -3, 0]);
    let at = |o: Outcome| Some(o.position());
    let mut writes = Vec::new();
    for (key, patch, left) in [
        ("d", json!({"v": doubles, "tag": "a"}), Some(Some(&doubles))),
        ("s", json!({"v": singles, "tag": "a"}), Some(Some(&singles))),
        ("r", json!({"v": doubles}), Some(Some(&doubles))),
        ("i", json!({"v": [2, -3.0, -0.0]}), Some(Some(&integral))),
        ("d", json!({"tag": "b"}), None),
        ("d", json!({"v": later}), Some(Some(&later))),
        ("d", json!({"tag": "c"}), None),
        ("i", json!({"v": null}), Some(None)),
    ] {
        writes.push((at(l.upsert("pin", key, patch).await?), key, left));
    }
    writes.push((at(l.retire("pin", "r").await?), "r", Some(None)));
    // Written again after its retirement, `r` starts from no fields.
    let anew = l.upsert("pin", "r", json!({"tag": "z"})).await?;
    writes.push((at(anew), "r", Some(None)));
    let want = grid(&writes);
    let live = [("d", &later), ("s", &singles)];
    holds(&l, "pin", "v", &want, "written").await?;
    ranks(&l, "pin", "v", &live, "written").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    holds(&l, "pin", "v", &want, "after a rebuild").await?;
    ranks(&l, "pin", "v", &live, "after a rebuild").await
}

/// An embedding backend of `f32`s that no double of fewer digits writes:
/// `alpha` is [`SINGLES`] and every other text [`BETA`].
struct Widening;

const WIDENING: &str = "widening";

#[async_trait]
impl EmbedBackend for Widening {
    fn model(&self) -> &str {
        WIDENING
    }

    fn dimensions(&self) -> u32 {
        3
    }

    async fn embed(&self, texts: &[String]) -> lakelands_api::Result<Vec<Vec<f32>>> {
        let made = |t: &String| if t == "alpha" { SINGLES } else { BETA };
        Ok(texts.iter().map(|t| made(t).to_vec()).collect())
    }
}

async fn a_backends_vector_reads_its_f32_elements_widened_at_each_position(
    store: Store,
    reopen: Reopen,
) -> Held {
    let seams = with_embedder(Some(Arc::new(Widening)));
    let store = again(&reopen, store, seams, "with an embedding backend").await?;
    let l = Lake::of(store.as_ref());
    let note = type_decl(
        "note",
        vec![
            field("text", StorageKind::Text),
            field("tag", StorageKind::Text),
            embedding("e", 3, &["text"], WIDENING),
        ],
    );
    l.declare(note).await?;
    let head = async || store.head().await.must("reading the head").map(Some);
    let (alpha, beta) = (widened(&SINGLES), widened(&BETA));
    let (a, b) = (Some(Some(&alpha)), Some(Some(&beta)));
    let mut writes = Vec::new();
    for (key, patch, left) in [
        ("n", json!({"text": "alpha"}), a),
        ("n", json!({"tag": "x"}), None),
        ("m", json!({"text": "alpha"}), a),
        ("n", json!({"text": "beta"}), b),
    ] {
        l.upsert("note", key, patch).await?;
        writes.push((head().await?, key, left));
    }
    let want = grid(&writes);
    let live = [("m", &alpha), ("n", &beta)];
    holds(&l, "note", "e", &want, "embedded").await?;
    ranks(&l, "note", "e", &live, "embedded").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    holds(&l, "note", "e", &want, "after a rebuild").await?;
    ranks(&l, "note", "e", &live, "after a rebuild").await
}

/// Whether, as of each position, a walk into `p1` over `approved_by`
/// answers on the edge from each key the elements beside it, in the
/// number form written, or no edge or no vector.
async fn walks(l: &Lake, want: &[Read<'_>], what: &str) -> Held {
    for (at, key, elements) in want {
        let edges = walked(l.cat.as_ref(), &l.own, "partner", "p1", Walk::In, *at).await?;
        let edge = edges.iter().find(|e| e.from.1.0 == *key);
        let read = edge.and_then(|e| forms(e.fields.get("v")));
        let wrote = forms(*elements);
        ensure!(
            read == wrote,
            "{what}: as of {at:?}, the edge from `{key}` into `p1` holds the elements {wrote:?}, \
             bit for bit; it holds {read:?}"
        );
    }
    Ok(())
}

async fn a_links_vector_reads_through_a_walk_at_each_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let fields = vec![vector("v", 3), field("tag", StorageKind::Text)];
    l.declare(invoice(vec![approved_by(fields, Authority::Authored)]))
        .await?;
    l.upsert("partner", "p1", json!({"name": "p1"})).await?;
    for i in ["i1", "i2"] {
        l.upsert("invoice", i, json!({"amount": "10"})).await?;
    }
    let (doubles, later, singles) = (json!(DOUBLES), json!(LATER), widened(&SINGLES));
    let at = |o: Outcome| Some(o.position());
    let mut writes = Vec::new();
    for (key, patch, left) in [
        ("i1", json!({"v": doubles}), Some(Some(&doubles))),
        (
            "i2",
            json!({"v": singles, "tag": "a"}),
            Some(Some(&singles)),
        ),
        ("i1", json!({"tag": "b"}), None),
        ("i1", json!({"v": later}), Some(Some(&later))),
    ] {
        writes.push((at(link_ok(&l, key, "p1", patch).await?), key, left));
    }
    let unlinked = unlink(&l, "i2", "p1").await.must("unlinking i2 from p1")?;
    writes.push((at(unlinked), "i2", Some(None)));
    let want = grid(&writes);
    walks(&l, &want, "linked").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    walks(&l, &want, "after a rebuild").await
}

/// `sensor`, keyed by `id`, whose `v` a source writes, erasable, beside
/// an author's `tag`.
fn sensor() -> TypeDecl {
    let id = FieldDecl {
        required: true,
        ..sourced("id", StorageKind::Text)
    };
    let mut v = sourced("v", StorageKind::Vector);
    v.ty.dimensions = Some(3);
    v.erasable = true;
    TypeDecl {
        key: vec!["id".into()],
        ..type_decl("sensor", vec![id, v, field("tag", StorageKind::Text)])
    }
}

/// Whether `s1`'s `v` reads as erased as of `at`, by `get` and `find`,
/// its provenance saying so and naming `crm`.
async fn erased(l: &Lake, at: Option<Position>, what: &str) -> Held {
    let read = read_both(l, "sensor", "v", "s1", at).await?;
    ensure!(
        read == [None, None],
        "{what}: as of {at:?}, the forgotten crm's vector of `s1` holds no value; it reads {read:?}"
    );
    let got = l.cat.get(&l.own, &"sensor".into(), get_at("s1", at)).await;
    let s1 = got.must(&format!("reading `s1` as of {at:?}"))?.data;
    let meta = s1.meta.get("v");
    ensure!(
        meta.is_some_and(|m| m.erased && m.source.as_ref().is_some_and(|s| s == "crm")),
        "{what}: as of {at:?}, `s1`'s `v` says it was erased and came from crm; its provenance \
         is {meta:?}"
    );
    Ok(())
}

async fn a_sealed_vector_reads_its_bits_until_its_source_is_forgotten(
    store: Store,
    reopen: Reopen,
) -> Held {
    let (_keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(sensor()).await?;
    let (doubles, singles) = (json!(DOUBLES), widened(&SINGLES));
    let s1 = source_record("sensor", "s1", json!({"id": "s1", "v": doubles}));
    let s2 = source_record("sensor", "s2", json!({"id": "s2", "v": singles}));
    let head = async || store.head().await.must("reading the head").map(Some);
    l.ingest("crm", vec![s1]).await.must("ingesting from crm")?;
    let written = head().await?;
    l.ingest("erp", vec![s2]).await.must("ingesting from erp")?;
    let other_written = head().await?;
    l.upsert("sensor", "s1", json!({"tag": "kept"})).await?;
    let tagged = head().await?;
    let writes = [
        (written, "s1", Some(Some(&doubles))),
        (other_written, "s2", Some(Some(&singles))),
        (tagged, "s1", None),
    ];
    let want = grid(&writes);
    let (sealed, other): (Vec<_>, Vec<_>) = want.into_iter().partition(|r| r.1 == "s1");
    holds(&l, "sensor", "v", &sealed, "sealed").await?;
    holds(&l, "sensor", "v", &other, "sealed").await?;
    let both = [("s1", &doubles), ("s2", &singles)];
    ranks(&l, "sensor", "v", &both, "sealed").await?;
    store
        .forget_source(&"crm".into(), None)
        .await
        .must("forgetting crm")?;
    let left = [("s2", &singles)];
    for what in ["forgotten", "after a rebuild"] {
        if what == "after a rebuild" {
            store.rebuild().await.must("rebuilding the lake")?;
        }
        for at in [written, other_written, tagged, None] {
            erased(&l, at, what).await?;
        }
        holds(&l, "sensor", "v", &other, what).await?;
        ranks(&l, "sensor", "v", &left, what).await?;
    }
    Ok(())
}

/// `ty`: a vector `v` and a text `tag`, keyed by `key` when given, a
/// required text field of the same name, beside `also` when given.
fn keyed(ty: &str, key: Option<&str>, also: Option<&str>) -> TypeDecl {
    let mut fields = vec![vector("v", 3), field("tag", StorageKind::Text)];
    for name in key.iter().chain(&also) {
        let required = key.is_some_and(|k| k == *name);
        fields.push(FieldDecl {
            required,
            ..field(name, StorageKind::Text)
        });
    }
    TypeDecl {
        key: key.into_iter().map(FieldName::from).collect(),
        ..type_decl(ty, fields)
    }
}

async fn a_restriction_and_its_lifting_keep_a_records_vector(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(keyed("held", None, None)).await?;
    let (doubles, singles) = (json!(DOUBLES), widened(&SINGLES));
    let at = |o: Outcome| Some(o.position());
    let mut writes = Vec::new();
    for (key, patch, left) in [
        ("r", json!({"v": doubles, "tag": "a"}), Some(Some(&doubles))),
        ("s", json!({"v": singles}), Some(Some(&singles))),
    ] {
        writes.push((at(l.upsert("held", key, patch).await?), key, left));
    }
    let live = [("r", &doubles), ("s", &singles)];
    let restricted = restrict(store.as_ref(), "held", "r", "a hold").await?;
    writes.push((Some(restricted), "r", None));
    ranks(&l, "held", "v", &live, "restricted").await?;
    let (t, k) = (TypeName::from("held"), RecordKey::from("r"));
    let lifted = store
        .unrestrict(&t, &k)
        .await
        .must("lifting the hold on `r`")?;
    writes.push((Some(lifted), "r", None));
    writes.push((
        at(l.upsert("held", "r", json!({"tag": "b"})).await?),
        "r",
        None,
    ));
    let want = grid(&writes);
    holds(&l, "held", "v", &want, "restricted and lifted").await?;
    ranks(&l, "held", "v", &live, "lifted").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    holds(&l, "held", "v", &want, "after a rebuild").await?;
    ranks(&l, "held", "v", &live, "after a rebuild").await
}

async fn a_rekey_ends_the_old_keys_vector_at_its_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(keyed("badge", Some("number"), Some("code")))
        .await?;
    let (doubles, singles) = (json!(DOUBLES), widened(&SINGLES));
    let (d, s) = (Some(Some(&doubles)), Some(Some(&singles)));
    let at = |o: Outcome| Some(o.position());
    let mut writes = Vec::new();
    for (key, patch, left) in [
        ("n1", json!({"number": "n1", "code": "c1", "v": doubles}), d),
        ("n2", json!({"number": "n2", "code": "c2", "v": singles}), s),
    ] {
        writes.push((at(l.upsert("badge", key, patch).await?), key, left));
    }
    let moved = at(l
        .migrate(keyed("badge", Some("code"), Some("number")))
        .await?);
    writes.extend([(moved, "n1", Some(None)), (moved, "n2", Some(None))]);
    writes.extend([(moved, "c1", d), (moved, "c2", s)]);
    writes.push((
        at(l.upsert("badge", "c1", json!({"tag": "b"})).await?),
        "c1",
        None,
    ));
    let want = grid(&writes);
    let live = [("c1", &doubles), ("c2", &singles)];
    holds(&l, "badge", "v", &want, "re-keyed").await?;
    ranks(&l, "badge", "v", &live, "re-keyed").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    holds(&l, "badge", "v", &want, "after a rebuild").await?;
    ranks(&l, "badge", "v", &live, "after a rebuild").await
}

async fn a_migration_that_renames_a_vector_field_reads_its_elements_under_the_new_name(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(keyed("pin", None, None)).await?;
    let (doubles, singles) = (json!(DOUBLES), widened(&SINGLES));
    let (d, s) = (Some(Some(&doubles)), Some(Some(&singles)));
    let at = |o: Outcome| Some(o.position());
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for (key, patch, left) in [
        ("k1", json!({"v": doubles, "tag": "a"}), d),
        ("k2", json!({"v": singles}), s),
    ] {
        let written = at(l.upsert("pin", key, patch).await?);
        old.push((written, key, left));
        new.push((written, key, None));
    }
    let renamed = TypeDecl {
        fields: vec![vector("w", 3), field("tag", StorageKind::Text)],
        ..keyed("pin", None, None)
    };
    let migrated = at(l.migrate_renaming(renamed, &[("v", "w")]).await?);
    old.extend([(migrated, "k1", Some(None)), (migrated, "k2", Some(None))]);
    new.extend([(migrated, "k1", d), (migrated, "k2", s)]);
    let tagged = at(l.upsert("pin", "k1", json!({"tag": "b"})).await?);
    old.push((tagged, "k1", None));
    new.push((tagged, "k1", None));
    let live = [("k1", &doubles), ("k2", &singles)];
    for what in ["renamed", "after a rebuild"] {
        if what == "after a rebuild" {
            store.rebuild().await.must("rebuilding the lake")?;
        }
        holds(&l, "pin", "v", &grid(&old), what).await?;
        holds(&l, "pin", "w", &grid(&new), what).await?;
        ranks(&l, "pin", "w", &live, what).await?;
    }
    Ok(())
}
