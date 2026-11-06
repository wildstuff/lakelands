//! A field a part its author may not know of reads stands, to that author,
//! as it is (lakelands#1124): its migration and its declaration are
//! refused on that field, before any check that would name the hidden
//! part.

use super::embeddings::{reopen_failed, with_embedder};
use crate::conformance::backing::{EMBED_MODEL, embedder_as};
use crate::conformance::fixture::{derived, type_of};
use crate::conformance::prelude::*;

check!(
    HIDDEN_READER_STANDS = a_field_a_hidden_part_reads_stands_to_its_author,
    "024, 027 and 035, extended (lakelands#1124): a field that a part its author may not know of reads (an embedding, a derived field, a constraint it is not told, a link field) stands to that author as it is: removed, renamed, its name freed and declared again, taken by a rename, swapped, retyped (a link field on a relation renamed too), made sensitive, made derived, or filled, the author's migration by `migrate`, `migrate_apply` and `propose`, and its declaration in a set by `apply`, are refused `field_read_elsewhere` on that field, whatever embedding backend is bound (the suite's, another model's, or none), and no answer names the hidden part or a field it may not know of; on a twin where nothing hidden reads the field the same changes are answered otherwise, and so is the owner on the first",
    reopens
);

/// The parts its author may not know of, each on a type of its own beside
/// a twin that lacks it: what reads `no` (or a link's `memo`) and `salary`
/// (or `secret`), which the author may not know of.
const PARTS: [&str; 4] = ["embedding", "derived", "constraint", "link"];

/// Names no answer to the author may hold.
const HIDDEN: [&str; 6] = ["salary", "secret", "hv", "net", "kept", "lv"];

fn text(names: &[&str]) -> Vec<FieldDecl> {
    names.iter().map(|n| field(n, StorageKind::Text)).collect()
}

/// The type of `part`, with the hidden reader or (`twin`) without it.
fn hidden(part: &str, twin: bool) -> TypeDecl {
    let name = format!("{}{}", if twin { "a" } else { "h" }, &part[..1]);
    let mut t = type_of(&name, &[]);
    t.fields = text(&["no", "gross", "salary"]);
    let net = |n, e, of: [&str; 2]| derived(n, StorageKind::Text, e, &of);
    // Its hidden input first: an answer naming it would be the first named.
    let hv = embedding("hv", 2, &["salary", "no"], EMBED_MODEL);
    match (part, twin) {
        (_, true) => {}
        ("embedding", _) => t.fields.push(hv),
        ("derived", _) => t
            .fields
            .push(net("net", "concat(no, salary)", ["no", "salary"])),
        ("constraint", _) => {
            let kept = json!({"name": "kept", "holds": "no != salary"});
            t.constraints
                .push(serde_json::from_value(kept).expect("a constraint reads"));
        }
        _ => {}
    }
    if part == "link" {
        let mut r = RelationDecl::to("r", name.as_str());
        r.fields = text(&["memo", "secret"]);
        let lv = net("lv", "concat(memo, secret)", ["memo", "secret"]);
        r.fields.extend((!twin).then_some(lv));
        t.relations.push(r);
    }
    t
}

type Change = (&'static str, Migration, Option<TypeDecl>);

/// The changes an author makes, as its view of `t` sends them: the
/// migration, and the declaration where a set can make the change.
fn changes(t: &TypeDecl, link: bool) -> Vec<Change> {
    let mut seen = t.clone();
    seen.fields
        .retain(|f| ["no", "gross"].contains(&f.name.as_str()));
    seen.constraints.clear();
    for r in &mut seen.relations {
        r.fields.retain(|f| f.name.as_str() == "memo");
    }
    let named = |names: &[&str]| TypeDecl {
        fields: text(names),
        ..seen.clone()
    };
    let row = |what, to: TypeDecl, renames: &[(&str, &str)], set: bool| {
        let renames = (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect();
        (
            what,
            Migration {
                renames,
                ..Migration::to(to.clone())
            },
            set.then_some(to),
        )
    };
    let (mut removed, mut retyped, mut sensitive) = (named(&["gross"]), seen.clone(), seen.clone());
    let mut computed = seen.clone();
    if link {
        removed = seen.clone();
        removed.relations[0].fields.clear();
        retyped.relations[0].fields[0].ty.kind = StorageKind::Number;
        sensitive.relations[0].fields[0].sensitive = true;
        computed.relations[0].fields[0] = derived("memo", StorageKind::Text, "'memo'", &[]);
        let mut moved = row("retyped, moved", retyped.clone(), &[], false);
        moved.1.to.relations[0].name = "r2".into();
        moved.1.relation_renames = vec![("r".into(), "r2".into())];
        return vec![
            moved,
            row("removed", removed, &[], true),
            row("retyped", retyped, &[], true),
            row("made sensitive", sensitive, &[], true),
            row("made derived", computed, &[], true),
        ];
    }
    retyped.fields[0].ty.kind = StorageKind::Number;
    sensitive.fields[0].sensitive = true;
    computed.fields[0] = derived("no", StorageKind::Text, "gross", &["gross"]);
    let mut filled = row("filled", seen.clone(), &[], false);
    filled.1.to.fields[0].initial = Some(json!("x"));
    filled.1.fill = vec!["no".into()];
    let (num, swap) = ([("no", "num")], [("no", "gross"), ("gross", "no")]);
    vec![
        row("removed", removed, &[], true),
        row("renamed", named(&["num", "gross"]), &num, false),
        row("freed", named(&["num", "gross", "no"]), &num, false),
        row("taken", named(&["num", "no"]), &[num[0], swap[1]], false),
        row("swapped", named(&["gross", "no"]), &swap, false),
        row("retyped", retyped, &[], true),
        row("made sensitive", sensitive, &[], true),
        row("made derived", computed, &[], true),
        filled,
    ]
}

/// What each door answers `who` for one change.
async fn doors(
    cat: &dyn Catalog,
    who: &Principal,
    m: &Migration,
    set: Option<&TypeDecl>,
) -> Vec<(&'static str, String)> {
    macro_rules! shown {
        ($call:expr) => {
            format!("{:?}", $call.await.map(drop))
        };
    }
    let (migration, report) = (Box::new(m.clone()), Default::default());
    let write = ProposedWrite::Migrate { migration, report };
    let answers = [
        shown!(cat.migrate(who, m.clone())),
        shown!(cat.migrate_apply(who, m.clone(), "none")),
        shown!(cat.propose(who, write, "a change".into())),
    ];
    let mut out: Vec<_> = ["migrate", "migrate_apply", "propose"]
        .into_iter()
        .zip(answers)
        .collect();
    if let Some(t) = set {
        let set = vec![Declaration::Type(t.clone())];
        out.push(("apply", shown!(cat.apply(who, set))));
    }
    out
}

/// Every change of every part, at every door, for `who` and the owner,
/// with `bound` naming the backend.
async fn table(l: &Lake, who: &Principal, bound: &str) -> Held {
    for part in PARTS {
        let link = part == "link";
        let (h, a) = (hidden(part, false), hidden(part, true));
        let (field, on) = match link {
            true => ("memo", link_type_name(&h.name, &"r".into()).to_string()),
            false => ("no", h.name.to_string()),
        };
        let refused =
            format!("FieldReadElsewhere {{ field: FieldName({field:?}), ty: TypeName({on:?}) }}");
        let (mine, twins) = (changes(&h, link), changes(&a, link));
        let mut owner_told = false;
        for ((what, m, set), (_, tm, tset)) in mine.iter().zip(&twins) {
            for (door, got) in doors(l.cat.as_ref(), who, m, set.as_ref()).await {
                ensure!(
                    got.contains(&refused)
                        && !HIDDEN.iter().any(|n| got.contains(&format!("{n:?}"))),
                    "with {bound}, `{field}` {what} on the {part} twin by `{door}` is refused as \
                     {refused}, naming nothing hidden; it answered {got}"
                );
            }
            for (door, got) in doors(l.cat.as_ref(), who, tm, tset.as_ref()).await {
                ensure!(
                    !got.contains("FieldReadElsewhere"),
                    "with {bound}, `{field}` {what} on the twin without the {part} by `{door}` \
                     is not refused as read elsewhere; it answered {got}"
                );
            }
            let owner = doors(l.cat.as_ref(), &l.own, m, None).await;
            owner_told |= owner
                .iter()
                .any(|(_, got)| !got.contains("FieldReadElsewhere"));
        }
        ensure!(
            owner_told,
            "with {bound}, the owner is answered the {part}'s own checks for some change, so the \
             twins are not blind"
        );
    }
    Ok(())
}

async fn a_field_a_hidden_part_reads_stands_to_its_author(store: Store, reopen: Reopen) -> Held {
    let store = reopen(store, with_embedder(Some(embedder_as(EMBED_MODEL, 2))))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let l = Lake::of(store.as_ref());
    let mut who = field_access(store.as_ref(), "author", "he", &[("salary", Access::None)]);
    (who.grants.schema, who.grants.review, who.grants.restricted) = (true, true, true);
    let kept = |f: &str| TypeGrants {
        fields: [(f.into(), Access::None)].into(),
        ..Default::default()
    };
    let types = PARTS
        .into_iter()
        .flat_map(|p| [hidden(p, false), hidden(p, true)]);
    for t in types {
        who.grants.types.insert(t.name.clone(), kept("salary"));
        let link = link_type_name(&t.name, &"r".into());
        who.grants.types.insert(link, kept("secret"));
        l.declare(t).await?;
    }
    table(&l, &who, "the suite's backend").await?;
    store
        .bind_embed(embedder_as("another-model", 2))
        .must("binding another model's backend")?;
    table(&l, &who, "another model's backend").await?;
    let store = reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))?;
    table(&Lake::of(store.as_ref()), &who, "no backend").await
}
