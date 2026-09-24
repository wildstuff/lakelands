//! The function backing the suite's checks call: every harness binds
//! [`backings`] in the seams of every lake it opens, so the checks can
//! declare functions computed outside the store and see what the store
//! does with their outputs.
//!
//! Its kind is [`KIND`]. A function's `config` scripts it:
//!
//! - `{"token": t, "echo": "<input>"}` answers that input's value;
//! - `{"token": t, "echo": "<input>", "fail_on": v}` fails when that input
//!   is `v`;
//! - after [`unplug`] of `t`, every call fails.
//!
//! It counts the calls made with each token, so a check can tell a value
//! read back from a recorded output from one computed by calling again.
//! The token is the check's own, so checks never count each other's
//! calls.
//!
//! The suite's embedding backend, [`embedder`], is bound in the lakes of
//! the checks that ask for it. Its model is [`EMBED_MODEL`], its vectors
//! have two dimensions, and every text becomes `(1, 0)`, except that a
//! text starting with [`UNREACHABLE`] fails the whole call. It fails with
//! a refusal, not `Unavailable`: whatever a backend answers as a failure,
//! the store makes the search unavailable. It counts its calls
//! ([`embed_calls`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use lakelands_api::seam::{EmbedBackend, FunctionBacking, FunctionCall};
use lakelands_api::{Error, Result, Value};

/// The backing kind the suite's functions name.
pub const KIND: &str = "conformance";

#[derive(Default)]
struct Calls {
    made: BTreeMap<String, usize>,
    unplugged: BTreeSet<String>,
}

fn calls() -> &'static Mutex<Calls> {
    static CALLS: OnceLock<Mutex<Calls>> = OnceLock::new();
    CALLS.get_or_init(Mutex::default)
}

/// The scripted backing.
struct Scripted;

fn fail(message: String) -> Error {
    Error::Unavailable { message }
}

#[async_trait]
impl FunctionBacking for Scripted {
    fn kind(&self) -> &str {
        KIND
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        let token = call.config["token"].as_str().unwrap_or_default().to_owned();
        {
            let mut c = calls().lock().unwrap_or_else(|p| p.into_inner());
            *c.made.entry(token.clone()).or_default() += 1;
            if c.unplugged.contains(&token) {
                return Err(fail(format!("the backing for `{token}` is unplugged")));
            }
        }
        let input = call.config["echo"].as_str().unwrap_or_default();
        let value = call.inputs[input].clone();
        if call.config.get("fail_on") == Some(&value) {
            return Err(fail(format!("`{}` cannot take {value}", call.function)));
        }
        Ok(value)
    }
}

/// The backings every harness binds.
pub fn backings() -> Vec<Arc<dyn FunctionBacking>> {
    vec![Arc::new(Scripted)]
}

/// How many calls were made with a token.
pub fn made(token: &str) -> usize {
    calls()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .made
        .get(token)
        .copied()
        .unwrap_or(0)
}

/// Make every later call with a token fail.
pub fn unplug(token: &str) {
    calls()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .unplugged
        .insert(token.to_owned());
}

/// The model the suite's embedding backend names.
pub const EMBED_MODEL: &str = "conformance-embed";

/// A text the suite's embedding backend fails on.
pub const UNREACHABLE: &str = "unreachable";

fn embeds() -> &'static Mutex<usize> {
    static EMBEDS: OnceLock<Mutex<usize>> = OnceLock::new();
    EMBEDS.get_or_init(Mutex::default)
}

/// The scripted embedding backend.
struct Embedder;

#[async_trait]
impl EmbedBackend for Embedder {
    fn model(&self) -> &str {
        EMBED_MODEL
    }

    fn dimensions(&self) -> u32 {
        2
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        *embeds().lock().unwrap_or_else(|p| p.into_inner()) += 1;
        if texts.iter().any(|t| t.starts_with(UNREACHABLE)) {
            return Err(Error::Refused {
                field: None,
                reason: lakelands_api::Refusal::Invalid {
                    message: format!("`{EMBED_MODEL}` cannot be reached"),
                },
            });
        }
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

/// The embedding backend the checks that ask for one bind.
pub fn embedder() -> Arc<dyn EmbedBackend> {
    Arc::new(Embedder)
}

/// How many calls the suite's embedding backend has received, in this
/// process. The suite runs its checks one at a time, so a check reads
/// its own calls as the difference.
pub fn embed_calls() -> usize {
    *embeds().lock().unwrap_or_else(|p| p.into_inner())
}
