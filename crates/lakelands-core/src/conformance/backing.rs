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
//! - `{"token": t, "echo": "<input>", "vast_on": v}` answers `1e16`, which
//!   the log writes as an integer no reader reads exactly, when that input
//!   is `v`;
//! - after [`unplug`] of `t`, every call fails, until [`plug`].
//!
//! It counts the calls made with each token, so a check can tell a value
//! read back from a recorded output from one computed by calling again,
//! and keeps the cause each call named ([`causes`]).
//! The token is the check's own, so checks never count each other's
//! calls.
//!
//! The suite's embedding backend, [`embedder`], is bound in the lakes of
//! the checks that ask for it. Its model is [`EMBED_MODEL`], its vectors
//! have two dimensions, and every text becomes `(1, 0)`, but one starting
//! with [`NOWHERE`], which becomes the vector of zeros, and one starting
//! with [`VAST`], which becomes `(1e16, 0)`. A call fails
//! when one of its texts starts with [`UNREACHABLE`], or was unplugged
//! ([`unplug_text`], until [`plug_text`]): it refuses the request as
//! `Invalid`, so the store asks again in halves until the text is alone.
//! A request with a text starting with [`WEATHER`] and none refused is
//! `Unavailable`, which the store records for every text it had not
//! answered. A search fails on either. A request with a text starting
//! with `held-` is counted and held until [`release`] of that text, five
//! seconds at most. It counts its calls ([`embed_calls`]) and how often
//! each text was sent ([`embedded`]); a check uses texts of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use lakelands_api::error::{Case, Detail, Exchange, Remote, Situation};
use lakelands_api::seam::{
    CallCause, EmbedBackend, FunctionBacking, FunctionCall, RerankCall, Reranker,
};
use lakelands_api::{Error, Result, Value};

/// The backing kind the suite's functions name.
pub const KIND: &str = "conformance";

#[derive(Default)]
struct Calls {
    made: BTreeMap<String, usize>,
    unplugged: BTreeSet<String>,
    causes: BTreeMap<String, Vec<Option<CallCause>>>,
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

/// A remote end that answered 503.
fn fail(remote: Remote) -> Error {
    let exchange = Exchange::Status { status: 503 };
    Error::unavailable(Situation::Remote { remote, exchange })
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
            let cause = call.cause.clone();
            c.causes.entry(token.clone()).or_default().push(cause);
            if c.unplugged.contains(&token) {
                let function = call.function.clone();
                return Err(fail(Remote::Function { function }));
            }
        }
        let input = call.config["echo"].as_str().unwrap_or_default();
        let value = call.inputs[input].clone();
        if call.config.get("fail_on") == Some(&value) {
            let function = call.function.clone();
            return Err(fail(Remote::Function { function }));
        }
        if call.config.get("vast_on") == Some(&value) {
            return Ok(serde_json::json!(1e16));
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

/// The cause each call made with a token named, in the order made.
pub fn causes(token: &str) -> Vec<Option<CallCause>> {
    lock(calls()).causes.get(token).cloned().unwrap_or_default()
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

/// A text the suite's embedding backend refuses.
pub const UNREACHABLE: &str = "unreachable";

/// A text the suite's embedding backend answers `Unavailable` for.
pub const WEATHER: &str = "weather";

/// A text the suite's embedding backend makes the vector of zeros.
pub const NOWHERE: &str = "nowhere";

/// A text the suite's embedding backend makes a vector whose first
/// component is `1e16`.
pub const VAST: &str = "vast";

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
        let refused = {
            let mut e = lock(embeds());
            e.calls += 1;
            e.sizes
                .entry(self.model.clone())
                .or_default()
                .push(texts.len());
            for t in texts {
                *e.texts.entry(t.clone()).or_default() += 1;
            }
            (texts.iter()).any(|t| t.starts_with(UNREACHABLE) || e.unplugged.contains(t))
        };
        if let Some(held) = texts.iter().find(|t| t.starts_with("held-")) {
            hold(held).await;
        }
        if refused {
            let (model, status) = (self.model.clone(), None);
            return Err(Error::invalid(Case::EmbeddingRefused { model, status }));
        }
        if texts.iter().any(|t| t.starts_with(WEATHER)) {
            let model = self.model.clone();
            return Err(fail(Remote::Embedding { model }));
        }
        let dimensions = self.dimensions as usize;
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0; dimensions];
                if let (Some(first), false) = (v.first_mut(), t.starts_with(NOWHERE)) {
                    *first = if t.starts_with(VAST) { 1e16 } else { 1.0 };
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

/// The model the suite's reranker names.
pub const RERANK_MODEL: &str = "conformance-rerank";

/// A query the suite's reranker fails on, as weather.
pub const RERANK_BROKEN: &str = "broken";

/// A query it refuses with an error that is not `Unavailable`, as a
/// reranker that turns a request down does.
pub const RERANK_REFUSED: &str = "refused";

/// A query it answers one score too few for.
pub const RERANK_SHORT: &str = "short";

/// A query it answers a score that is not a number for.
pub const RERANK_NAN: &str = "nan";

fn reranks() -> &'static Mutex<Vec<RerankCall>> {
    static RERANKS: OnceLock<Mutex<Vec<RerankCall>>> = OnceLock::new();
    RERANKS.get_or_init(Mutex::default)
}

/// The scripted reranker: a text scores twice its count of `overdue`
/// words and once its count of `reminder` words. It records every call.
struct Scorer;

#[async_trait]
impl Reranker for Scorer {
    fn model(&self) -> &str {
        RERANK_MODEL
    }

    async fn rerank(&self, call: &RerankCall) -> Result<Vec<f64>> {
        lock(reranks()).push(call.clone());
        let model = RERANK_MODEL.to_owned();
        if call.query.starts_with(RERANK_BROKEN) {
            return Err(fail(Remote::Reranker { model }));
        }
        if call.query.starts_with(RERANK_REFUSED) {
            let reason = lakelands_api::Refusal::NotGranted;
            return Err(Error::Refused {
                field: None,
                reason,
            });
        }
        let mut scores: Vec<f64> = call
            .candidates
            .iter()
            .map(|c| rerank_score(&c.text))
            .collect();
        if call.query.starts_with(RERANK_SHORT) {
            scores.pop();
        }
        if call.query.starts_with(RERANK_NAN) {
            scores.iter_mut().for_each(|s| *s = f64::NAN);
        }
        Ok(scores)
    }
}

/// What the suite's reranker scores a text: twice its count of the word
/// `overdue` and once its count of `reminder`.
pub fn rerank_score(text: &str) -> f64 {
    let words = || text.split(|c: char| !c.is_alphanumeric());
    let count = |word: &str| words().filter(|w| *w == word).count();
    (2 * count("overdue") + count("reminder")) as f64
}

/// The reranker the checks that ask for one bind.
pub fn reranker() -> Arc<dyn Reranker> {
    Arc::new(Scorer)
}

/// Every call the suite's reranker has received, in this process, oldest
/// first. The suite runs its checks one at a time, so a check reads its
/// own calls as those past the count it started with.
pub fn rerank_calls() -> Vec<RerankCall> {
    lock(reranks()).clone()
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
/// the key null for a verb called on its type, `null` when the payload's
/// `null` is `true`, `{"paid": 1e16}` when its `vast` is `true`, fails
/// when its `fail` is `true`, declines the call when its `decline` is
/// `true`, answers `not_granted` when its `ungrant` is `true`, or a refusal
/// whose case holds an integer beyond ±(2^53 − 1) when its `vast_error` is
/// `true`. It counts its
/// runs per record key, and per verb for a verb called on its type, so a
/// check uses keys and such verbs of its own. A call on a record whose key
/// begins with `held-` is counted and held until [`release`] of that key,
/// five seconds at most.
struct Executor;

fn released() -> &'static Mutex<BTreeSet<String>> {
    static RELEASED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    RELEASED.get_or_init(Mutex::default)
}

/// Let the executor answer the call it holds on the record `key`, or the
/// embedding backend the request it holds for the text `key`.
pub fn release(key: &str) {
    lock(released()).insert(key.to_owned());
}

/// Wait until [`release`] of `key`, five seconds at most.
async fn hold(key: &str) {
    for _ in 0..1_000 {
        if lock(released()).contains(key) {
            break;
        }
        crate::conformance::fixture::pause(std::time::Duration::from_millis(5)).await;
    }
}

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
        if let Some(held) = key.as_deref().filter(|k| k.starts_with("held-")) {
            hold(held).await;
        }
        if invocation.payload.get("null") == Some(&Value::Bool(true)) {
            return Ok(Value::Null);
        }
        if invocation.payload.get("vast") == Some(&Value::Bool(true)) {
            return Ok(serde_json::json!({"paid": 1e16}));
        }
        if invocation.payload.get("decline") == Some(&Value::Bool(true)) {
            return Err(Error::declined(&invocation.verb));
        }
        if invocation.payload.get("decline_because") == Some(&Value::Bool(true)) {
            let reason = Detail::scrubbed("above this executor's ceiling");
            return Err(Error::declined_because(&invocation.verb, reason));
        }
        if invocation.payload.get("vast_error") == Some(&Value::Bool(true)) {
            let n = serde_json::Number::from(i64::MAX);
            return Err(Error::invalid(Case::IntegerTooLarge { n }));
        }
        if invocation.payload.get("ungrant") == Some(&Value::Bool(true)) {
            return Err(Error::Refused {
                field: None,
                reason: lakelands_api::Refusal::NotGranted,
            });
        }
        if invocation.payload.get("fail") == Some(&Value::Bool(true)) {
            let name = invocation.verb.to_string();
            return Err(fail(Remote::Executor { name }));
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
