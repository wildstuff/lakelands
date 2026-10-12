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
    Lake, Must, declare, describe_type, doors, field, field_access, get_at, invalid_case,
    invalid_like, owner, type_decl, write_verb,
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

check!(
    TOOL_NAME_REFUSES_OPEN =
        a_vocabulary_whose_tools_would_bear_anothers_names_refuses_the_writers_open,
    "027, extended (lakelands#893), and seams, SystemVocabulary: no door leaves a lake with a problem among its tools' names, a writer's open among them: a host vocabulary with a type whose tools would bear the names of a declared type's, with one whose tool a verb is named as, and with two of its own that would share their tools' names, is refused at the writer's open as `tool_names`, naming both sides; nothing is appended and the lake holds no type of that vocabulary; it then takes a plain type from its owner and from one kept from the declared type, its tools are generated, and it opens with the same vocabulary once the type is withheld from it",
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

async fn a_vocabulary_whose_tools_would_bear_anothers_names_refuses_the_writers_open(
    store: Store,
    reopen: Reopen,
) -> Held {
    use lakelands_api::tools::{tool_base, tools};
    let with = |vocabulary| Seams {
        backings: backing::backings(),
        vocabulary,
        ..Seams::default()
    };
    let plain = |name: &str| type_decl(name, vec![field("x", StorageKind::Text)]);
    let l = Lake::of(store.as_ref());
    // `sys_audit` names its tools as `sys.audit` would, and a verb on
    // `vault` is named as a tool of `sys.memo` would be.
    for name in ["vault", &tool_base("sys.audit")] {
        l.declare(plain(name)).await?;
    }
    let made = tools(&l.describe().await?).unwrap_or_default();
    let mut doors = made.iter().map(|t| t.name.as_str());
    let door = doors.find(|n| n.starts_with("vault_")).unwrap_or_default();
    let verb = door.replacen("vault", &tool_base("sys.memo"), 1);
    l.declare_as(write_verb(&verb, "vault", "x", "'x'"), "a verb")
        .await?;
    let mut store = store;
    for (types, sides) in [
        (["sys.host_note", "sys.audit"], ["sys.audit", "sys_audit"]),
        (["sys.host_note", "sys.memo"], ["sys.memo", verb.as_str()]),
        (["sys.a.b", "sys.a_b"], ["sys.a.b", "sys.a_b"]),
    ] {
        let before = store.head().await.must("reading the head")?;
        let vocabulary = backing::vocabulary(types.map(plain).to_vec(), &[]);
        let answer = reopen(store.clone(), with(Some(vocabulary))).await.err();
        let told = format!("{answer:?}");
        let named = sides
            .iter()
            .all(|side| told.contains(&format!("\"{side}\"")));
        ensure!(
            answer
                .as_ref()
                .is_some_and(|e| invalid_like(e, &json!({"code": "tool_names"})))
                && named,
            "a vocabulary of {types:?} is refused at the writer's open as `tool_names`, naming \
             {sides:?}; it answered {told}"
        );
        store = reopen(store.clone(), with(None))
            .await
            .map_err(|e| reopened("left out", e))?;
        let after = store.head().await.must("reading the head")?;
        let mut held = Vec::new();
        for ty in types {
            held.push(holds(store.as_ref(), ty).await);
        }
        ensure!(
            after == before.next() && held == [false, false],
            "a refused open appends nothing and declares no type of its vocabulary: the head \
             was {before} and is {after} after the next open, which takes the lease, and of \
             {types:?} the lake holds {held:?}"
        );
    }
    // The lake has no problem among its names: it takes a declaration,
    // naming nothing, and opens once the host withholds the type.
    let (l, twin) = (Lake::of(store.as_ref()), tool_base("sys.audit"));
    let grants = json!({"default": "read", "schema": true, "types": {twin: {"default": "none"}}});
    let kept = super::only::holder(store.as_ref(), "author", grants);
    let free = [
        declare(l.cat.as_ref(), &l.own, plain("crate")).await.err(),
        declare(l.cat.as_ref(), &kept, plain("box")).await.err(),
    ];
    let made = tools(&l.describe().await?);
    ensure!(
        free == [None, None] && made.is_ok(),
        "after the refused opens a plain type is declared by the owner and by one kept from \
         `sys_audit` ({free:?}), and the lake's tools are generated ({:?})",
        made.as_ref().err()
    );
    let withheld = backing::vocabulary(
        ["sys.host_note", "sys.audit"].map(plain).to_vec(),
        &["sys.audit"],
    );
    let store = reopen(store, with(Some(withheld)))
        .await
        .map_err(|e| reopened("that withholds `sys.audit`", e))?;
    ensure!(
        holds(store.as_ref(), "sys.host_note").await && !holds(store.as_ref(), "sys.audit").await,
        "opened with `sys.audit` withheld, the lake holds `sys.host_note` and not `sys.audit`"
    );
    Ok(())
}

check!(
    HOST_EXTERNAL_VERB = a_host_external_verb_on_its_own_system_type_is_admitted,
    "003 and seams, SystemVocabulary, extended (lakelands#966): on a type the host's vocabulary declares, a verb whose effect is `external` and which sets nothing is admitted alike by `declare`, `apply` and `propose`, of scope record and of scope type; at risk low its call is run by its executor and answers its output, at risk high it is proposed and its accept runs it; the same verb with `effect: write` is refused as reserved by the three doors, and so is an external verb on `sys.setting` and on `sys.aging_rule`; one named as a tool of its type is refused as `tool_names`; the head stays unmoved",
    reopens
);

async fn a_host_external_verb_on_its_own_system_type_is_admitted(
    store: Store,
    reopen: Reopen,
) -> Held {
    use lakelands_api::write::{Declaration, Outcome, ProposedWrite};
    use lakelands_api::{Refusal, VerbName};
    const ON: &str = "sys.host_job";
    let job = || type_decl(ON, vec![field("text", StorageKind::Text)]);
    let seams = Seams {
        backings: backing::backings(),
        executors: backing::executors(),
        vocabulary: Some(backing::vocabulary(vec![job()], &[])),
        ..Seams::default()
    };
    let store = reopen(store, seams)
        .await
        .map_err(|e| reopened("with `sys.host_job`", e))?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let verb = |name: &str, on: &str, extra: serde_json::Value| {
        let mut v = json!({"declare": "verb", "name": name, "on": on, "risk": "low",
                           "effect": "external", "executor": backing::EXECUTOR});
        for (k, x) in extra.as_object().into_iter().flatten() {
            v[k] = x.clone();
        }
        if v["effect"] == "write" {
            v.as_object_mut().map(|v| v.remove("executor"));
        }
        serde_json::from_value::<Declaration>(v).expect("a verb reads")
    };
    // Each door answers alike; `propose` answers a proposal where `declare`
    // and `apply` apply.
    let doors = |d: Declaration| {
        let (cat, own) = (cat.clone(), own.clone());
        async move {
            let declarations = vec![d.clone()];
            let proposed = cat.propose(&own, ProposedWrite::Declare { declarations }, "w".into());
            let proposed = proposed
                .await
                .map(|o| matches!(o, Outcome::Proposed { .. }));
            let applied = cat.apply(&own, vec![d.clone()]).await.map(drop);
            let declared = cat.declare(&own, d).await.map(drop);
            (declared, applied, proposed)
        }
    };
    let mut red = Vec::new();
    for d in [
        verb("deploy", ON, json!({})),
        verb("archive", ON, json!({"risk": "high"})),
        verb("tally", ON, json!({"scope": "type"})),
    ] {
        let name = d.place();
        match doors(d).await {
            (Ok(()), Ok(()), Ok(true)) => {}
            other => red.push(format!(
                "{name:?} must be admitted by the three doors: {other:?}"
            )),
        }
    }
    let reserved = |a: &lakelands_api::Result<()>| {
        matches!(
            crate::conformance::fixture::reason(a),
            Some(Refusal::Reserved)
        )
    };
    let before = store.head().await.must("reading the head")?;
    for d in [
        verb(
            "stamp",
            ON,
            json!({"effect": "write", "set": [{"field": "text", "value": "'x'"}]}),
        ),
        verb("tune", "sys.setting", json!({})),
        verb("expire", "sys.aging_rule", json!({})),
    ] {
        let name = d.place();
        let (declared, applied, proposed) = doors(d).await;
        let proposed = proposed.map(drop);
        if !(reserved(&declared) && reserved(&applied) && reserved(&proposed)) {
            red.push(format!(
                "{name:?} must be refused as reserved by the three doors: {declared:?}, \
                 {applied:?}, {proposed:?}"
            ));
        }
    }
    // Its name is judged as any verb's: one named as a tool of its type is
    // refused as `tool_names`.
    let made = lakelands_api::tools::tools(&l.describe().await?).unwrap_or_default();
    let base = lakelands_api::tools::tool_base(ON);
    let taken = made
        .iter()
        .map(|t| t.name.as_str())
        .find(|n| n.starts_with(&base));
    let clash = cat
        .declare(own, verb(taken.unwrap_or_default(), ON, json!({})))
        .await;
    if !invalid_like(&clash, &json!({"code": "tool_names"})) {
        red.push(format!(
            "a verb named as {taken:?}, a tool of `{ON}`: {clash:?}"
        ));
    }
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "a reserved verb").await?;
    ensure!(red.is_empty(), "{}", red.join("; "));
    // Called: at risk low the executor runs and the output is answered; at
    // risk high the call is proposed and its accept runs it.
    l.upsert(ON, "job-966", json!({"text": "t"})).await?;
    let key = lakelands_api::RecordKey::from("job-966");
    let call = |verb: &'static str, key: Option<lakelands_api::RecordKey>| {
        let (cat, own) = (cat.clone(), own.clone());
        async move {
            let (verb, opts) = (VerbName::from(verb), Default::default());
            cat.call(&own, &verb, key.as_ref(), Default::default(), opts)
                .await
        }
    };
    let low = call("deploy", Some(key.clone()))
        .await
        .must("calling `deploy`")?;
    let tally = call("tally", None)
        .await
        .must("calling `tally` on its type")?;
    let high = call("archive", Some(key.clone()))
        .await
        .must("calling `archive`")?;
    let Outcome::Proposed { proposal, .. } = &high else {
        return Err(Failure::new(format!(
            "`archive` is high: it is proposed; it answered {high:?}"
        )));
    };
    let ran = backing::executed("job-966");
    let accepted = cat
        .accept(own, proposal)
        .await
        .must("accepting `archive`")?;
    let output = |o: &Outcome| {
        matches!(
            o,
            Outcome::Applied {
                output: Some(_),
                ..
            }
        )
    };
    ensure!(
        output(&low)
            && output(&tally)
            && ran == 1
            && backing::executed("job-966") == 2
            && backing::executed("tally") == 1
            && output(&accepted),
        "an external verb on `{ON}` runs its executor and answers its output: `deploy` \
         {low:?}, `tally` {tally:?}; `archive` ran {ran} time(s) before its accept and \
         answered {accepted:?} after it"
    );
    Ok(())
}
