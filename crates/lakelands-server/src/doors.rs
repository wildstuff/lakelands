//! One door, run from its arguments: what MCP and REST share.
//!
//! Both transports turn a request into a [`Door`] and a JSON object of
//! arguments — an MCP tool call carries them as they are, a REST route
//! assembles them from its path, query, headers and body under the same
//! names — and [`run`] calls the catalog. So the two can never read an
//! argument differently, and a door's answer is the same JSON on both.

use lakelands_api::error::Error;
use lakelands_api::id::{FieldName, ProposalId, RecordKey, TypeName};
use lakelands_api::ontology::{FieldDecl, TypeDecl};
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

/// The arguments, read as `T`; `door` names the door in the refusal.
pub(crate) fn args<T: DeserializeOwned>(door: &str, a: &Map<String, Value>) -> Result<T, Fault> {
    serde_json::from_value(Value::Object(a.clone())).map_err(|e| {
        Fault::Arguments(
            format!("the arguments do not fit `{door}`: {e}"),
            "correct them against the tool's input schema and call it again".into(),
        )
    })
}

pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("contract types serialise")
}

/// Refuse an argument the door's input schema does not list, before
/// anything runs: a misspelled `expected_version` must not turn a
/// compare-and-set into an unconditional write.
pub(crate) fn unknown_argument(spec: &ToolSpec, arguments: &Map<String, Value>) -> Option<Fault> {
    let listed = spec.input_schema["properties"].as_object();
    let key = arguments
        .keys()
        .find(|k| !listed.is_some_and(|p| p.contains_key(k.as_str())))?;
    Some(Fault::Arguments(
        format!(
            "`{}` takes no argument `{key}`; nothing was done",
            spec.name
        ),
        "use only the arguments the tool's input schema lists, then call it again".into(),
    ))
}

/// Run one catalog door with its arguments, as `who`. `label` names the
/// door in a refusal of the arguments. [`Door::Pin`] belongs to the MCP
/// session, which answers it itself; here it is refused.
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
            let w: Write = args(label, &a)?;
            to_json(&c.call(who, verb, &w.key, w.payload, w.opts).await?)
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
        Door::Query => to_json(&c.query(who, args::<Pattern>(label, &a)?).await?),
        Door::Search => to_json(&c.search(who, args::<Search>(label, &a)?).await?),
        Door::Propose => {
            #[derive(Deserialize)]
            struct P {
                write: ProposedWrite,
                why: String,
            }
            let p: P = args(label, &a)?;
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
            let d: D = args(label, &a)?;
            to_json(&c.declare(who, Declaration::Type(d.ty)).await?)
        }
        Door::AddField => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeName,
                field: FieldDecl,
            }
            let d: D = args(label, &a)?;
            to_json(&c.add_field(who, &d.ty, d.field).await?)
        }
        Door::Migrate => {
            #[derive(Deserialize)]
            struct M {
                to: TypeDecl,
                #[serde(default)]
                renames: Vec<(FieldName, FieldName)>,
                #[serde(default)]
                report_id: Option<String>,
            }
            let m: M = args(label, &a)?;
            let migration = Migration {
                to: m.to,
                renames: m.renames,
            };
            match m.report_id {
                Some(r) => to_json(&c.migrate_apply(who, migration, &r).await?),
                None => to_json(&c.migrate(who, migration).await?),
            }
        }
    })
}
