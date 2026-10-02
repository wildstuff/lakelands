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
//! - after [`unplug`] of `t`, every call fails, until [`plug`].
//!
//! It counts the calls made with each token, so a check can tell a value
//! read back from a recorded output from one computed by calling again.
//! The token is the check's own, so checks never count each other's
//! calls.
//!
//! The suite's embedding backend, [`embedder`], is bound in the lakes of
//! the checks that ask for it. Its model is [`EMBED_MODEL`], its vectors
//! have two dimensions, and every text becomes `(1, 0)`. A call fails
//! when one of its texts starts with [`UNREACHABLE`], or was unplugged
//! ([`unplug_text`], until [`plug_text`]). It fails with a refusal, not
//! `Unavailable`: whatever a backend answers as a failure, the store makes
//! a search unavailable and records a failure for each text. It counts its
//! calls ([`embed_calls`]) and how often each text was sent
//! ([`embedded`]); a check uses texts of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

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

/// The guard of `m`, also when a check that panicked poisoned it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
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
            let mut c = lock(calls());
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
    lock(calls()).made.get(token).copied().unwrap_or(0)
}

/// Make every later call with a token fail.
pub fn unplug(token: &str) {
    lock(calls()).unplugged.insert(token.to_owned());
}

/// Undo [`unplug`]: later calls with the token are answered again.
pub fn plug(token: &str) {
    lock(calls()).unplugged.remove(token);
}

/// The model the suite's embedding backend names.
pub const EMBED_MODEL: &str = "conformance-embed";

/// A text the suite's embedding backend fails on.
pub const UNREACHABLE: &str = "unreachable";

#[derive(Default)]
struct Embeds {
    calls: usize,
    texts: BTreeMap<String, usize>,
    unplugged: BTreeSet<String>,
    /// How many texts each call carried, per model.
    sizes: BTreeMap<String, Vec<usize>>,
}

fn embeds() -> &'static Mutex<Embeds> {
    static EMBEDS: OnceLock<Mutex<Embeds>> = OnceLock::new();
    EMBEDS.get_or_init(Mutex::default)
}

/// The scripted embedding backend, under a model and a vector length.
struct Embedder {
    model: String,
    dimensions: u32,
}

#[async_trait]
impl EmbedBackend for Embedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut e = lock(embeds());
        e.calls += 1;
        e.sizes
            .entry(self.model.clone())
            .or_default()
            .push(texts.len());
        for t in texts {
            *e.texts.entry(t.clone()).or_default() += 1;
        }
        if texts
            .iter()
            .any(|t| t.starts_with(UNREACHABLE) || e.unplugged.contains(t))
        {
            return Err(Error::Refused {
                field: None,
                reason: lakelands_api::Refusal::Invalid {
                    message: format!("`{}` cannot be reached", self.model),
                },
            });
        }
        let dimensions = self.dimensions as usize;
        Ok(texts
            .iter()
            .map(|_| {
                let mut v = vec![0.0; dimensions];
                if let Some(first) = v.first_mut() {
                    *first = 1.0;
                }
                v
            })
            .collect())
    }
}

/// The embedding backend the checks that ask for one bind: the model
/// [`EMBED_MODEL`], vectors of two dimensions.
pub fn embedder() -> Arc<dyn EmbedBackend> {
    embedder_as(EMBED_MODEL, 2)
}

/// The same scripted backend under another model or vector length, for a
/// check that opens its lake again with a backend that does not make what
/// a field was declared with. It counts with [`embedder`] and answers the
/// same way: every vector is one in its first dimension and zero after.
pub fn embedder_as(model: &str, dimensions: u32) -> Arc<dyn EmbedBackend> {
    Arc::new(Embedder {
        model: model.to_owned(),
        dimensions,
    })
}

/// How many texts each call to a model carried, in order.
pub fn embed_call_sizes(model: &str) -> Vec<usize> {
    lock(embeds()).sizes.get(model).cloned().unwrap_or_default()
}

/// How many calls the suite's embedding backend has received, in this
/// process. The suite runs its checks one at a time, so a check reads
/// its own calls as the difference.
pub fn embed_calls() -> usize {
    lock(embeds()).calls
}

/// How many times a text was sent to the suite's embedding backend.
pub fn embedded(text: &str) -> usize {
    lock(embeds()).texts.get(text).copied().unwrap_or(0)
}

/// Make every later call carrying this text fail.
pub fn unplug_text(text: &str) {
    lock(embeds()).unplugged.insert(text.to_owned());
}

/// Undo [`unplug_text`].
pub fn plug_text(text: &str) {
    lock(embeds()).unplugged.remove(text);
}

/// A host's system vocabulary, as a check scripts it: it adds `types` and
/// allows every `sys.` type in every lake except those named in
/// `withheld`. Lakelands' own types are never withheld by it.
pub fn vocabulary(
    types: Vec<lakelands_api::ontology::TypeDecl>,
    withheld: &[&str],
) -> Arc<dyn lakelands_api::seam::SystemVocabulary> {
    Arc::new(Vocabulary {
        types,
        withheld: withheld.iter().map(|t| (*t).to_owned()).collect(),
    })
}

struct Vocabulary {
    types: Vec<lakelands_api::ontology::TypeDecl>,
    withheld: BTreeSet<String>,
}

impl lakelands_api::seam::SystemVocabulary for Vocabulary {
    fn types(&self) -> Vec<lakelands_api::ontology::TypeDecl> {
        self.types.clone()
    }

    fn may_hold(&self, _: &lakelands_api::LakeId, ty: &lakelands_api::TypeName) -> bool {
        !self.withheld.contains(ty.as_str())
    }
}

/// The name of the suite's verb executor, bound in every lake the runner
/// opens. A verb declared with another executor stays unbound.
pub const EXECUTOR: &str = "conformance_executor";

fn executions() -> &'static Mutex<BTreeMap<String, usize>> {
    static EXECUTIONS: OnceLock<Mutex<BTreeMap<String, usize>>> = OnceLock::new();
    EXECUTIONS.get_or_init(Mutex::default)
}

/// The scripted executor: it answers `{"done": <verb>, "record": <key>}`,
/// the key null for a verb called on its type, or fails when the
/// payload's `fail` is `true`. It counts its runs per record key, and per
/// verb for a verb called on its type, so a check uses keys and such verbs
/// of its own.
struct Executor;

#[async_trait]
impl lakelands_api::seam::VerbExecutor for Executor {
    fn name(&self) -> &str {
        EXECUTOR
    }

    async fn execute(&self, invocation: &lakelands_api::seam::VerbInvocation) -> Result<Value> {
        let key = invocation
            .record
            .as_ref()
            .map(|r| r.key.as_str().to_owned());
        *lock(executions())
            .entry(
                key.clone()
                    .unwrap_or_else(|| invocation.verb.as_str().to_owned()),
            )
            .or_default() += 1;
        if invocation.payload.get("fail") == Some(&Value::Bool(true)) {
            return Err(fail(format!("`{}` was asked to fail", invocation.verb)));
        }
        Ok(serde_json::json!({"done": invocation.verb.as_str(), "record": key}))
    }
}

/// The executors every harness binds.
pub fn executors() -> Vec<Arc<dyn lakelands_api::seam::VerbExecutor>> {
    vec![Arc::new(Executor)]
}

/// How many times the executor ran for a record key, or for a verb
/// called on its type.
pub fn executed(key: &str) -> usize {
    lock(executions()).get(key).copied().unwrap_or(0)
}
