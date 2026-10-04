//! One door, run from its arguments: what MCP and REST share.
//!
//! Both transports turn a request into a [`Door`] and a JSON object of
//! arguments — an MCP tool call carries them as they are, a REST route
//! assembles them from its path, query, headers and body under the same
//! names — and [`run`] calls the catalog. So the two can never read an
//! argument differently, and a door's answer is the same JSON on both.

use lakelands_api::error::{Error, Refusal};
use lakelands_api::id::{FieldName, ProposalId, RecordKey, TypeName};
use lakelands_api::ontology::{FieldDecl, TypeDecl, ValueTypeDecl};
use lakelands_api::query::{Pattern, Search};
use lakelands_api::read::{Find, Get, ReadAt};
use lakelands_api::record::Patch;
use lakelands_api::tools::{Door, ToolSpec};
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{Catalog, Principal};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Why a door failed: the contract's closed set, or arguments that do
/// not fit the door (what happened, what to do).
pub(crate) enum Fault {
    Contract(Error),
    Arguments(String, String),
}

impl From<Error> for Fault {
    fn from(e: Error) -> Fault {
        Fault::Contract(e)
    }
}

/// The arguments, read as `T`; `door` names the door in the refusal, and
/// the refusal names where among the arguments it failed, such as
/// `declarations[2]`.
pub(crate) fn args<T: DeserializeOwned>(door: &str, a: &Map<String, Value>) -> Result<T, Fault> {
    read(door, a, false)
}

/// The arguments of a door that declares, read as `T`: a member a
/// declaration does not take, at any depth, is the caller's to remove.
fn declaring<T: DeserializeOwned>(door: &str, a: &Map<String, Value>) -> Result<T, Fault> {
    read(door, a, true)
}

fn read<T: DeserializeOwned>(
    door: &str,
    a: &Map<String, Value>,
    declares: bool,
) -> Result<T, Fault> {
    serde_path_to_error::deserialize(Value::Object(a.clone())).map_err(|e| {
        let inner = e.inner().to_string();
        // An unknown member is named by the error; the path says where it
        // sits, so it ends at the member's parent.
        let mut path = e.path().to_string();
        if let Some(member) = inner
            .strip_prefix("unknown field `")
            .and_then(|rest| rest.split_once('`'))
            .map(|(member, _)| member)
            && let Some(parent) = path.strip_suffix(member)
        {
            path = match parent.trim_end_matches('.') {
                "" => ".".into(),
                parent => parent.into(),
            };
        }
        let what = if path == "." {
            format!("the arguments do not fit `{door}`: {inner}")
        } else {
            format!("the arguments do not fit `{door}` at `{path}`: {inner}")
        };
        let unknown = inner.starts_with("unknown field `") || inner.contains(" has no member `");
        let todo = if declares && unknown {
            "remove the member or correct its name, then call again; nothing was declared"
        } else {
            "correct them against the tool's input schema and call it again"
        };
        Fault::Arguments(what, todo.into())
    })
}

/// An argument the catalog checks itself, before it reads anything, even
/// where the tool's schema does not list it, so that its refusal says
/// why: a read's `params` on a type that is not a view with parameters is
/// refused saying the type declares none, and a `key` to a verb on its
/// type naming the verb.
fn catalog_answers(door: &Door, argument: &str) -> bool {
    matches!(
        (door, argument),
        (Door::Find { .. } | Door::Get { .. }, "params") | (Door::Call { .. }, "key")
    )
}

pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("contract types serialise")
}

/// Refuse an argument the door's input schema does not list, before
/// anything runs: a misspelled `expected_version` must not turn a
/// compare-and-set into an unconditional write. The exceptions are the
/// catalog's to answer ([`catalog_answers`]).
pub(crate) fn unknown_argument(spec: &ToolSpec, arguments: &Map<String, Value>) -> Option<Fault> {
    let listed = spec.input_schema["properties"].as_object();
    let key = arguments.keys().find(|k| {
        !catalog_answers(&spec.door, k) && !listed.is_some_and(|p| p.contains_key(k.as_str()))
    })?;
    Some(Fault::Arguments(
        format!(
            "`{}` takes no argument `{key}`; nothing was done",
            spec.name
        ),
        "use only the arguments the tool's input schema lists, then call it again".into(),
    ))
}

/// Run one catalog door with its arguments, as `who`. `label` names the
/// door in a refusal of the arguments. [`Door::Pin`], [`Door::Subscribe`] and
/// [`Door::Unsubscribe`] belong to the MCP session, which answers them
/// itself; here they are refused.
pub(crate) async fn run(
    c: &dyn Catalog,
    who: &Principal,
    door: &Door,
    label: &str,
    a: Map<String, Value>,
) -> Result<Value, Fault> {
    #[derive(Deserialize)]
    struct Write {
        key: RecordKey,
        #[serde(default)]
        fields: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    // A call's key is optional: a verb on its type takes none.
    #[derive(Deserialize)]
    struct Called {
        #[serde(default)]
        key: Option<RecordKey>,
        #[serde(default)]
        payload: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    #[derive(Deserialize)]
    struct Link {
        key: RecordKey,
        to: RecordKey,
        #[serde(default)]
        fields: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    Ok(match door {
        Door::Find { ty } => to_json(&c.find(who, ty, args::<Find>(label, &a)?).await?),
        Door::Get { ty } => to_json(&c.get(who, ty, args::<Get>(label, &a)?).await?),
        Door::Upsert { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.upsert(who, ty, &w.key, w.fields, w.opts).await?)
        }
        Door::Retire { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.retire(who, ty, &w.key, w.opts).await?)
        }
        Door::Link { ty, relation } => {
            let l: Link = args(label, &a)?;
            to_json(
                &c.link(who, ty, &l.key, relation, &l.to, l.fields, l.opts)
                    .await?,
            )
        }
        Door::Unlink { ty, relation } => {
            let l: Link = args(label, &a)?;
            to_json(&c.unlink(who, ty, &l.key, relation, &l.to, l.opts).await?)
        }
        Door::Call { verb } => {
            let w: Called = args(label, &a)?;
            to_json(&c.call(who, verb, w.key.as_ref(), w.payload, w.opts).await?)
        }
        Door::Describe => {
            #[derive(Deserialize)]
            struct D {
                #[serde(default, rename = "type")]
                ty: Option<TypeName>,
                #[serde(flatten)]
                at: ReadAt,
            }
            let d: D = args(label, &a)?;
            match d.ty {
                Some(ty) => to_json(&c.describe_type(who, &ty, d.at).await?),
                None => to_json(&c.describe(who, d.at).await?),
            }
        }
        Door::Pin => {
            return Err(Fault::Arguments(
                format!("`{label}` pins an MCP session; there is none here"),
                "name `as_of` on each read instead".into(),
            ));
        }
        Door::Subscribe | Door::Unsubscribe => {
            return Err(Fault::Arguments(
                format!("`{label}` belongs to an MCP session; there is none here"),
                "read the change feed instead".into(),
            ));
        }
        Door::Query => to_json(&c.query(who, args::<Pattern>(label, &a)?).await?),
        Door::Search => to_json(&c.search(who, args::<Search>(label, &a)?).await?),
        Door::Propose => {
            #[derive(Deserialize)]
            struct P {
                write: ProposedWrite,
                why: String,
            }
            let p: P = declaring(label, &a)?;
            to_json(&c.propose(who, p.write, p.why).await?)
        }
        Door::Proposals => to_json(&c.proposals(who, args::<ProposalFilter>(label, &a)?).await?),
        Door::Accept | Door::Reject => {
            #[derive(Deserialize)]
            struct D {
                proposal: ProposalId,
                #[serde(default)]
                reason: String,
            }
            let d: D = args(label, &a)?;
            if *door == Door::Accept {
                to_json(&c.accept(who, &d.proposal).await?)
            } else {
                to_json(&c.reject(who, &d.proposal, d.reason).await?)
            }
        }
        Door::DeclareType => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeDecl,
            }
            let d: D = declaring(label, &a)?;
            to_json(&c.declare(who, Declaration::Type(d.ty)).await?)
        }
        Door::Declare => {
            // No tool's schema checks these routes' members, so the
            // arguments refuse one they do not take.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct D {
                declaration: Declaration,
            }
            let d: D = declaring(label, &a)?;
            to_json(&c.declare(who, d.declaration).await?)
        }
        Door::Apply => {
            // No tool's schema checks these routes' members, so the
            // arguments refuse one they do not take.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct D {
                declarations: Vec<Declaration>,
            }
            let d: D = declaring(label, &a)?;
            to_json(&c.apply(who, d.declarations).await?)
        }
        Door::Recompute => {
            let what: lakelands_api::write::Recompute = declaring(label, &a)?;
            to_json(&c.recompute(who, what).await?)
        }
        Door::AddField => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeName,
                field: FieldDecl,
            }
            let d: D = declaring(label, &a)?;
            to_json(&c.add_field(who, &d.ty, d.field).await?)
        }
        Door::Migrate => {
            #[derive(Deserialize)]
            struct M {
                #[serde(default)]
                to: Option<TypeDecl>,
                #[serde(default)]
                value_type: Option<ValueTypeDecl>,
                #[serde(default)]
                renames: Vec<(FieldName, FieldName)>,
                #[serde(default)]
                report_id: Option<String>,
            }
            let m: M = declaring(label, &a)?;
            let wrong = |field: &str, message: &str| {
                Fault::Contract(Error::Refused {
                    field: Some(field.into()),
                    reason: Refusal::Invalid {
                        message: message.into(),
                    },
                })
            };
            match (m.to, m.value_type) {
                (Some(to), None) => {
                    let migration = Migration {
                        to,
                        renames: m.renames,
                    };
                    match m.report_id {
                        Some(r) => to_json(&c.migrate_apply(who, migration, &r).await?),
                        None => to_json(&c.migrate(who, migration).await?),
                    }
                }
                (None, Some(vt)) => {
                    if !m.renames.is_empty() {
                        return Err(wrong(
                            "renames",
                            "a value type has no fields to rename; `renames` belongs to `to`",
                        ));
                    }
                    match m.report_id {
                        Some(r) => to_json(&c.migrate_value_type_apply(who, vt, &r).await?),
                        None => to_json(&c.migrate_value_type(who, vt).await?),
                    }
                }
                (Some(_), Some(_)) => {
                    return Err(wrong(
                        "value_type",
                        "a migration names a type (`to`) or a value type (`value_type`), not both",
                    ));
                }
                (None, None) => {
                    return Err(wrong(
                        "to",
                        "a migration names a type (`to`) or a value type (`value_type`)",
                    ));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lakelands_api::engine::OpenMode;
    use lakelands_api::seam::Seams;
    use lakelands_api::{Grants, LakeStore, Location, PrincipalKind, WriteContext};
    use serde_json::json;

    use super::*;

    fn owner(lake: &str) -> Principal {
        Principal {
            kind: PrincipalKind::Human,
            id: "owner".into(),
            lake: lake.into(),
            grants: Grants::owner(),
        }
    }

    /// The mock, and the store over the memory and SQLite engines.
    async fn lakes() -> Vec<(&'static str, Arc<dyn LakeStore>)> {
        let dir =
            std::env::temp_dir().join(format!("lakelands-migrate-door-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = || Seams {
            engines: vec![
                Arc::new(lakelands_engine_mem::MemoryOpener::new()),
                Arc::new(lakelands_engine_sqlite::SqliteOpener),
            ],
            ..Seams::default()
        };
        vec![
            (
                "mock",
                Arc::new(lakelands_mock::MockLake::new("doors")) as Arc<dyn LakeStore>,
            ),
            (
                "memory",
                lakelands_store::open(Location::memory("doors"), seams(), OpenMode::Create)
                    .await
                    .unwrap(),
            ),
            (
                "sqlite",
                lakelands_store::open(Location::sqlite("doors", &dir), seams(), OpenMode::Create)
                    .await
                    .unwrap(),
            ),
        ]
    }

    /// The `migrate` tool names a type (`to`) or a value type
    /// (`value_type`), one of the two, and a value type takes no renames:
    /// anything else is refused as invalid, naming the argument, on the
    /// mock and on the store alike.
    #[tokio::test(flavor = "multi_thread")]
    async fn migrate_takes_a_type_or_a_value_type_never_both_and_never_neither() {
        let vt = json!({"name": "status_code", "kind": "text", "one_of": ["open", "late"]});
        let ty = json!({"name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]});
        for (name, store) in lakes().await {
            let cat = store.catalog(WriteContext::default());
            let who = owner("doors");
            cat.declare(&who, serde_json::from_value(json!({"declare": "value_type", "name": "status_code", "kind": "text", "one_of": ["open", "late", "void"]})).unwrap())
                .await
                .unwrap();
            let call = |a: Value| {
                let (cat, who) = (cat.clone(), who.clone());
                async move {
                    let a = a.as_object().cloned().unwrap_or_default();
                    run(cat.as_ref(), &who, &Door::Migrate, "migrate", a).await
                }
            };
            for (args, field) in [
                (json!({"to": ty, "value_type": vt}), "value_type"),
                (json!({}), "to"),
                (json!({"report_id": "r"}), "to"),
                (
                    json!({"value_type": vt, "renames": [["a", "b"]]}),
                    "renames",
                ),
            ] {
                let answer = call(args.clone()).await;
                assert!(
                    matches!(
                        &answer,
                        Err(Fault::Contract(Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } }))
                            if f.as_str() == field
                    ),
                    "on {name}, {args} was not refused as invalid naming `{field}`"
                );
            }
            let report = call(json!({"value_type": vt})).await;
            assert!(
                matches!(&report, Ok(v) if v["data"]["fits"] == json!(true)),
                "on {name}, the value type's preflight did not answer"
            );
        }
    }
}
