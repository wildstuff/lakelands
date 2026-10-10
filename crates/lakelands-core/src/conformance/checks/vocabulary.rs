//! A host's own `sys.` types reach a lake through its system vocabulary
//! (`docs/decisions/003-reservation-by-namespace.md`, `docs/seams.md`,
//! `SystemVocabulary`). The store consults the vocabulary each time a
//! writer opens the lake and declares the types it allows and the lake
//! does not hold yet, so a type the host adds later reaches a lake that
//! already exists, and a lake the host withholds a type from never holds
//! it. A vocabulary naming a type under `sys.link.`, or one of the four
//! names the store keeps for its own readers, is refused at the writer's
//! open, naming the type, and nothing is appended.

use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::Seams;
use lakelands_api::{Access, LakeStore, ReadAt, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, describe_type, doors, field, field_access, get_at, invalid_case, owner, type_decl,
};
use crate::conformance::{Failure, Held, Reopen, Store, backing};

check!(
    HOST_TYPES_AT_OPEN = a_host_type_is_declared_when_a_writer_opens_the_lake,
    "003 and seams, SystemVocabulary: each writer open declares the host's sys. types the lake may hold and does not hold yet; a type added later reaches the existing lake, and a withheld one never does",
    reopens
);

check!(
    RESERVED_NAME_REFUSES_OPEN = a_vocabulary_naming_a_reserved_type_refuses_the_writers_open,
    "003, 035 and seams, SystemVocabulary: a host vocabulary naming a type under sys.link. or one of the store's own four (sys.source, sys.provenance, sys.setting, sys.aging_rule) is refused at the writer's open as invalid, naming the type as reserved; nothing is appended, and the lake never holds it",
    reopens
);

check!(
    HOST_VIEW_KEY = a_host_view_with_a_key_its_records_do_not_carry_is_judged_by_what_they_do,
    "006 and 018, extended (lakelands#788): a host's vocabulary type passes no `declare`, so its view may hold a `key` that is not its `group_by`, and it is keyed by what its records carry all the same: an ungrouped `sys.` view with `key: [n]` is shown to a reader held from `n` as the same view without `key` is, `get` answers its one record, and each is described with no key; a `sys.` view grouped by `name` with `key: [n]` is shown to that reader too and described keyed by `name`, and is hidden from a reader held from `name`",
    reopens
);

fn reopened(vocabulary: &str, e: lakelands_api::Error) -> Failure {
    Failure::new(format!(
        "opening the lake again with a vocabulary {vocabulary} failed: {e} ({e:?})"
    ))
}

async fn holds(store: &dyn LakeStore, ty: &str) -> bool {
    doors(store)
        .describe_type(&owner(store), &TypeName::from(ty), ReadAt::default())
        .await
        .is_ok()
}

async fn a_host_type_is_declared_when_a_writer_opens_the_lake(
    store: Store,
    reopen: Reopen,
) -> Held {
    let note = || type_decl("sys.host_note", vec![field("text", StorageKind::Text)]);
    let tag = || type_decl("sys.host_tag", vec![field("label", StorageKind::Text)]);
    let hidden = || type_decl("sys.host_hidden", vec![field("x", StorageKind::Text)]);
    let with = |vocabulary| Seams {
        backings: backing::backings(),
        vocabulary: Some(vocabulary),
        ..Seams::default()
    };
    ensure!(
        !holds(store.as_ref(), "sys.host_note").await,
        "a lake opened with no host vocabulary already holds `sys.host_note`"
    );

    let store = reopen(
        store,
        with(backing::vocabulary(
            vec![note(), hidden()],
            &["sys.host_hidden"],
        )),
    )
    .await
    .map_err(|e| reopened("adding `sys.host_note`", e))?;
    ensure!(
        holds(store.as_ref(), "sys.host_note").await,
        "a writer opened the lake with a vocabulary adding `sys.host_note`; the lake does \
         not hold it"
    );
    ensure!(
        !holds(store.as_ref(), "sys.host_hidden").await,
        "the vocabulary withholds `sys.host_hidden` from this lake; the lake holds it all \
         the same"
    );
    Lake::of(store.as_ref())
        .upsert("sys.host_note", "n", json!({"text": "kept"}))
        .await?;

    let store = reopen(
        store,
        with(backing::vocabulary(
            vec![note(), tag(), hidden()],
            &["sys.host_hidden"],
        )),
    )
    .await
    .map_err(|e| reopened("that grew by `sys.host_tag`", e))?;
    ensure!(
        holds(store.as_ref(), "sys.host_tag").await,
        "the host added `sys.host_tag` after the lake existed; the next writer's open did \
         not declare it"
    );
    let kept = Lake::of(store.as_ref()).read("sys.host_note", "n").await?;
    ensure!(
        kept.fields.get("text") == Some(&json!("kept")),
        "`sys.host_note` `n` was written before the vocabulary grew; after the next open \
         it reads {kept:?}"
    );
    ensure!(
        !holds(store.as_ref(), "sys.host_hidden").await,
        "the vocabulary still withholds `sys.host_hidden`; the lake holds it after the \
         second open"
    );
    Ok(())
}

async fn a_vocabulary_naming_a_reserved_type_refuses_the_writers_open(
    store: Store,
    reopen: Reopen,
) -> Held {
    let with = |vocabulary| Seams {
        backings: backing::backings(),
        vocabulary,
        ..Seams::default()
    };
    let mut store = store;
    for name in ["sys.source", "sys.link.invoice.approver"] {
        let before = store.head().await.must("reading the head")?;
        let taken = || type_decl(name, vec![field("x", StorageKind::Text)]);
        let answer = reopen(
            store.clone(),
            with(Some(backing::vocabulary(vec![taken()], &[]))),
        )
        .await;
        match answer {
            Err(e)
                if matches!(invalid_case(&e), Some(Case::VocabularyReserved { ty, .. })
                if ty == name) => {}
            Err(e) => {
                return Err(Failure::new(format!(
                    "a vocabulary naming `{name}` is refused at the writer's open as invalid, \
                     naming the type as reserved; it answered {e} ({e:?})"
                )));
            }
            Ok(_) => {
                return Err(Failure::new(format!(
                    "a vocabulary naming `{name}` is refused at the writer's open; the lake \
                     opened"
                )));
            }
        }
        store = reopen(store.clone(), with(None))
            .await
            .map_err(|e| reopened("without the reserved name", e))?;
        let after = store.head().await.must("reading the head")?;
        ensure!(
            after == before.next(),
            "a refused open appends nothing: the head was {before} before it, and after the \
             next open (which takes the lease, one event) it is {after}"
        );
        ensure!(
            !holds(store.as_ref(), name).await,
            "a vocabulary naming `{name}` was refused; the lake holds `{name}` all the same"
        );
    }
    Ok(())
}

async fn a_host_view_with_a_key_its_records_do_not_carry_is_judged_by_what_they_do(
    store: Store,
    reopen: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("item", vec![field("name", StorageKind::Text)]))
        .await?;
    l.upsert("item", "i1", json!({"name": "a"})).await?;
    let host = |name: &str, key: &[&str], by: &[&str]| {
        let text = |n: &&str| json!({"name": n, "type": {"kind": "text"}});
        let mut fields: Vec<_> = by.iter().map(text).collect();
        fields.push(json!({"name": "n", "type": {"kind": "decimal"}}));
        let n = json!([{"name": "n", "expr": "count()"}]);
        let view = json!({"source": "item", "group_by": by, "measures": n});
        let v = json!({"name": name, "key": key, "fields": fields, "view": view});
        serde_json::from_value(v).expect("a type reads")
    };
    let views = vec![
        host("sys.keyed", &["n"], &[]),
        host("sys.plain", &[], &[]),
        host("sys.grouped", &["n"], &["name"]),
    ];
    let seams = Seams {
        backings: backing::backings(),
        vocabulary: Some(backing::vocabulary(views, &[])),
        ..Seams::default()
    };
    let store = reopen(store, seams)
        .await
        .map_err(|e| reopened("of three views", e))?;
    let cat = doors(store.as_ref());
    let all = lakelands_api::key::UNGROUPED;
    // The field its reader is held from, a record's key, and the key the
    // view is described with where it is shown.
    for (ty, held, key, shown) in [
        ("sys.keyed", "n", all, Some(vec![])),
        ("sys.plain", "n", all, Some(vec![])),
        ("sys.grouped", "n", "a", Some(vec!["name"])),
        ("sys.grouped", "name", "a", None),
    ] {
        let who = field_access(store.as_ref(), "held", ty, &[(held, Access::None)]);
        let told = describe_type(cat.as_ref(), &who, ty).await;
        let one = cat.get(&who, &ty.into(), get_at(key, None)).await;
        let keyed = told.as_ref().ok().map(|t| &t.access.key);
        let keyed = keyed.map(|k| k.iter().map(|f| f.as_str()).collect::<Vec<_>>());
        ensure!(
            keyed == shown && one.is_ok() == shown.is_some(),
            "`{ty}`, a view of the host's, is judged by the key its records carry: to a reader \
             held from `{held}` it is described keyed {shown:?} where it is shown; it is \
             described {told:?}, and `get` answered {one:?}"
        );
    }
    Ok(())
}
