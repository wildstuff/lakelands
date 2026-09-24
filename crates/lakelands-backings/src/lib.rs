//! Function backings that reach outside the lake: [`http`] and [`exec`].
//!
//! A function a lake declares with the backing `http` or `exec` computes
//! its fields through one of these, called by the store after the write
//! that made the call due; the store records every answer and never
//! sends one input twice (`docs/decisions/022-functions-are-declarations.md`).
//! What each may reach is the operator's decision, never a declaration's:
//! an `http` function names a URL whose host is on the operator's list,
//! and an `exec` function names a command the operator configured, never
//! a program. Both are configured here, from the server configuration a
//! host reads; nothing is read from the environment
//! (`docs/decisions/005-no-environment-reads.md`).
//!
//! A backing kind is not configured unless its section is present: with
//! no `http` section, a lake cannot declare an `http` function.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::seam::{FunctionBacking, FunctionCall};
use lakelands_api::{Error, Refusal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod exec;
pub mod http;

/// The backings section of a server configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The `http` backing, when the operator allows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpConfig>,
    /// The `exec` backing, when the operator allows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecConfig>,
}

/// What the `http` backing may reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// The hosts a function's URL may name: `host`, or `host:port` to
    /// allow one port only. Compared without regard to case.
    pub hosts: Vec<String>,
    /// How long one call may take, in seconds, connecting included.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// The largest reply body accepted, in bytes.
    #[serde(default = "default_max_bytes")]
    pub max_reply_bytes: u64,
    /// The certificate authorities `https` trusts, as a PEM bundle. When
    /// absent, the first system bundle found at a well-known path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

/// What the `exec` backing may run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecConfig {
    /// The commands a function may name, by the name it uses.
    pub commands: BTreeMap<String, Command>,
    /// How long one run may take, in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// The largest output accepted on standard output, in bytes.
    #[serde(default = "default_max_bytes")]
    pub max_output_bytes: u64,
}

/// One command the operator allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// The program, as a path; no shell interprets it.
    pub program: PathBuf,
    /// Its arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

fn default_timeout() -> u64 {
    10
}

fn default_max_bytes() -> u64 {
    1024 * 1024
}

impl Config {
    /// Refuse a configuration that could reach what the operator did not
    /// name: a command given by a relative path, which the operating
    /// system would look up along the server's own `PATH`, or an `http`
    /// section with no host. A host checks this when it loads the
    /// configuration, before it serves anything.
    pub fn check(&self) -> Result<(), String> {
        if let Some(exec) = &self.exec {
            for (name, command) in &exec.commands {
                if !command.program.is_absolute() {
                    return Err(format!(
                        "the exec command `{name}` names the program `{}`, which is not an \
                         absolute path",
                        command.program.display()
                    ));
                }
            }
        }
        if self.http.as_ref().is_some_and(|h| h.hosts.is_empty()) {
            return Err("the http backing lists no host it may call".into());
        }
        Ok(())
    }
}

/// The backings a configuration allows, to bind in `Seams::backings`.
/// The configuration is [`Config::check`]ed first; a command that fails
/// the check is left out.
pub fn backings(config: &Config) -> Vec<Arc<dyn FunctionBacking>> {
    let mut out: Vec<Arc<dyn FunctionBacking>> = Vec::new();
    if let Some(c) = &config.http {
        out.push(Arc::new(http::Http::new(c.clone())));
    }
    if let Some(c) = &config.exec {
        let mut c = c.clone();
        c.commands
            .retain(|_, command| command.program.is_absolute());
        out.push(Arc::new(exec::Exec::new(c)));
    }
    out
}

/// What a backing sends: the call without the lake, the object whose
/// digest keys the recorded output (`docs/log-format.md` §2.5).
fn body(call: &FunctionCall) -> Vec<u8> {
    let v = json!({ "function": call.function, "inputs": call.inputs, "config": call.config });
    serde_json::to_vec(&v).expect("a JSON value serialises")
}

/// A call its function's configuration does not allow.
fn refused(call: &FunctionCall, why: String) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: format!("function `{}`: {why}", call.function),
        },
    }
}

/// A call that could not be answered.
fn failed(call: &FunctionCall, why: String) -> Error {
    Error::Unavailable {
        message: format!("function `{}`: {why}", call.function),
    }
}

/// An answer's bytes as the output: JSON, or a failure saying it is not.
fn output(call: &FunctionCall, bytes: &[u8]) -> Result<Value, Error> {
    serde_json::from_slice(bytes).map_err(|e| failed(call, format!("the answer is not JSON: {e}")))
}
