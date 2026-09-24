//! The webhook deliverer: the default consumer of a lake's change feed,
//! posting each fired trigger and object-set change to an endpoint the
//! operator names.
//!
//! A deliverer reads the feed (`docs/decisions/004-the-change-feed-is-the-log.md`)
//! for one lake and one endpoint, from its own position, and sends each
//! `trigger_fired` and `object_set_changed` event in log order as one
//! `POST`. These are the only kinds it sends. Both carry keys only, so no
//! field value leaves the lake this way; a receiver that wants a record
//! reads it through a door, under its own grants.
//!
//! **The request.** The body is the event as the log holds it, in canonical
//! JSON: the bytes of a line of the feed, carrying `lake`, `position` and
//! `id`. The headers `Lakelands-Lake`, `Lakelands-Position` (with the lake,
//! the change's dedup key), `Lakelands-Event` and `Lakelands-Attempt` repeat
//! what the receiver needs before it parses. With a secret,
//! `Lakelands-Signature: sha256=<hex>` is the HMAC-SHA256 of the body's
//! bytes; the receiver checks it over the raw bytes before parsing. The
//! connection is the crate's one transport: no proxy, no redirect
//! followed, TLS trusting the configured bundle or the system's, a timeout
//! over each whole request, and a reply limit.
//!
//! **Delivery is at least once, in order.** The position is written after
//! each event the deliverer is done with, so a crash between a `POST` and
//! that write sends the event again, and the receiver deduplicates by lake
//! and position. A `2xx` is delivered. A timeout, a failed connection or
//! TLS, `408`, `429` and `5xx` are transient, and so are `401`, `403` and
//! `404`, which say the endpoint is misconfigured rather than the event
//! refused. The same event is tried again after a backoff that doubles to
//! a cap, without end. Any other status, `3xx` included, is the receiver
//! refusing this event: it is recorded, and the deliverer moves on.
//!
//! **What it keeps is the host's.** Beside its position file the
//! deliverer keeps a journal, one JSON line per outcome, opened for
//! appending so the operator can rotate it by renaming. Nothing of it is
//! written into the lake (004, extended). A message names the endpoint by
//! its name and by host, port and path, never the URL as written, the
//! secret or a body.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hyper::header::{HeaderName, HeaderValue};
use lakelands_api::LakeStore;
use lakelands_api::event::Event;
use lakelands_api::event::EventBody;
use lakelands_api::seam::FeedFilter;
use lakelands_api::{LakeId, Position, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::transport::{Failure, Target, Transport};

/// The event kinds a deliverer may send.
pub const KINDS: [&str; 2] = ["trigger_fired", "object_set_changed"];

/// The `deliver` section of a server configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverConfig {
    /// The endpoints, the only places a deliverer sends anything.
    pub endpoints: Vec<EndpointConfig>,
    /// How long a deliverer that has caught up waits before it polls
    /// again, in milliseconds.
    #[serde(default = "default_poll_millis")]
    pub poll_millis: u64,
    /// The most events read from the feed at once.
    #[serde(default = "default_batch")]
    pub batch: u32,
    /// The first wait before a transient failure is tried again, in
    /// seconds; it doubles with every attempt.
    #[serde(default = "default_retry_first")]
    pub retry_first_secs: u64,
    /// The longest wait between attempts, in seconds.
    #[serde(default = "default_retry_max")]
    pub retry_max_secs: u64,
    /// How long one request may take, in seconds, connecting included.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// The largest reply body read, in bytes; the body is dropped.
    #[serde(default = "default_max_reply_bytes")]
    pub max_reply_bytes: u64,
    /// The certificate authorities `https` trusts, as a PEM bundle,
    /// relative to the configuration's directory. When absent, the first
    /// system bundle found at a well-known path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

/// One endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    /// Its name, which its files and messages carry.
    pub name: String,
    /// Where events are posted, `http` or `https`.
    pub url: String,
    /// The lakes whose events it receives.
    pub lakes: Vec<String>,
    /// The kinds it receives: `trigger_fired`, `object_set_changed`, or
    /// both (the default).
    #[serde(default = "default_kinds")]
    pub kinds: Vec<String>,
    /// Only these triggers, when named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    /// Only these object sets, when named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sets: Vec<String>,
    /// A file holding the signing secret, relative to the configuration's
    /// directory. Absent, requests are not signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_file: Option<PathBuf>,
    /// Where an endpoint without a position file begins.
    #[serde(default)]
    pub start: Start,
}

/// Where a new endpoint begins reading the feed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Start {
    /// After the events the lake holds when the endpoint first starts.
    #[default]
    Head,
    /// From the lake's first event.
    Genesis,
}

fn default_poll_millis() -> u64 {
    1000
}
fn default_batch() -> u32 {
    100
}
fn default_retry_first() -> u64 {
    1
}
fn default_retry_max() -> u64 {
    300
}
fn default_timeout() -> u64 {
    10
}
fn default_max_reply_bytes() -> u64 {
    64 * 1024
}
fn default_kinds() -> Vec<String> {
    KINDS.iter().map(|k| (*k).to_owned()).collect()
}

impl DeliverConfig {
    /// Refuse a configuration that could send what the operator did not
    /// mean: a name used twice or not usable as a file name, a URL that is
    /// not `http` or `https` or carries user information, a kind other
    /// than the two, an endpoint for no lake, or a number that is zero. A
    /// host checks this when it loads the configuration.
    pub fn check(&self) -> Result<(), String> {
        for (what, zero) in [
            ("poll_millis", self.poll_millis == 0),
            ("batch", self.batch == 0),
            ("retry_first_secs", self.retry_first_secs == 0),
            ("retry_max_secs", self.retry_max_secs == 0),
            ("timeout_secs", self.timeout_secs == 0),
            ("max_reply_bytes", self.max_reply_bytes == 0),
        ] {
            if zero {
                return Err(format!("the deliver section's `{what}` is zero"));
            }
        }
        if self.retry_first_secs > self.retry_max_secs {
            return Err(
                "the deliver section's `retry_first_secs` is longer than its `retry_max_secs`"
                    .into(),
            );
        }
        let mut names = BTreeSet::new();
        for e in &self.endpoints {
            let name = &e.name;
            let usable = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !usable {
                return Err(format!(
                    "the endpoint name `{name}` is not letters, digits, `-` and `_`"
                ));
            }
            if !names.insert(name) {
                return Err(format!("the endpoint name `{name}` is used twice"));
            }
            let target =
                Target::parse(&e.url).map_err(|why| format!("endpoint `{name}`: the url {why}"))?;
            if target.user_info {
                return Err(format!(
                    "endpoint `{name}`: the url carries user information, which is never sent; \
                     sign requests with `secret_file` instead"
                ));
            }
            if e.lakes.is_empty() {
                return Err(format!("endpoint `{name}` names no lake"));
            }
            if e.kinds.is_empty() {
                return Err(format!("endpoint `{name}` names no kind"));
            }
            if let Some(k) = e.kinds.iter().find(|k| !KINDS.contains(&k.as_str())) {
                return Err(format!(
                    "endpoint `{name}`: the kind `{k}` is not one a deliverer sends \
                     (`trigger_fired`, `object_set_changed`)"
                ));
            }
        }
        Ok(())
    }
}

/// How one attempt ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Answer {
    Delivered,
    /// Try the same event again; why.
    Retry(String),
    /// The receiver refused this event, with this status.
    Refused(u16),
}

/// One endpoint's deliverer for one lake.
pub struct Deliverer {
    lake: LakeId,
    name: String,
    target: Target,
    kinds: Vec<String>,
    triggers: Vec<String>,
    sets: Vec<String>,
    start: Start,
    secret: Option<ring::hmac::Key>,
    transport: Transport,
    position_file: PathBuf,
    journal: File,
    // Held for the deliverer's life: a second deliverer of the same
    // endpoint and lake is refused at start.
    _lock: File,
    poll: Duration,
    batch: u32,
    retry_first: Duration,
    retry_max: Duration,
    sleep: Sleep,
}

/// How a deliverer waits: `tokio::time::sleep`, or a test's own clock.
pub type Sleep = Arc<dyn Fn(Duration) -> BoxFuture + Send + Sync>;

/// The future a [`Sleep`] answers.
pub type BoxFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

fn tokio_sleep() -> Sleep {
    Arc::new(|d| Box::pin(tokio::time::sleep(d)))
}

impl std::fmt::Debug for Deliverer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret is never shown.
        f.debug_struct("Deliverer")
            .field("lake", &self.lake)
            .field("endpoint", &self.name)
            .field("target", &self.target.shown())
            .finish()
    }
}

/// The deliverers a configuration makes for one lake: one per endpoint
/// that names it. Its files live under `dir/deliveries/<lake>/`: the
/// position (`<endpoint>.json`), locked while the deliverer runs, and the
/// journal (`<endpoint>.ndjson`). Refused, with the reason, when the
/// configuration fails [`DeliverConfig::check`], a secret file cannot be
/// read or is empty, TLS cannot be set up for an `https` endpoint, or
/// another deliverer holds a position file.
pub fn deliverers(
    config: &DeliverConfig,
    dir: &Path,
    lake: &LakeId,
) -> Result<Vec<Deliverer>, String> {
    config.check()?;
    let home = dir.join("deliveries").join(lake.as_str());
    let mut out = Vec::new();
    for e in config
        .endpoints
        .iter()
        .filter(|e| e.lakes.iter().any(|l| l == lake.as_str()))
    {
        let name = &e.name;
        let target = Target::parse(&e.url).map_err(|why| format!("endpoint `{name}`: {why}"))?;
        let secret = match &e.secret_file {
            None => None,
            Some(file) => {
                let file = dir.join(file);
                let text = std::fs::read_to_string(&file).map_err(|err| {
                    format!(
                        "endpoint `{name}`: the secret file {} cannot be read: {err}",
                        file.display()
                    )
                })?;
                let secret = text.trim();
                if secret.is_empty() {
                    return Err(format!(
                        "endpoint `{name}`: the secret file {} is empty",
                        file.display()
                    ));
                }
                Some(ring::hmac::Key::new(
                    ring::hmac::HMAC_SHA256,
                    secret.as_bytes(),
                ))
            }
        };
        let transport = Transport::new(
            "the deliverer",
            config.ca_file.as_ref().map(|f| dir.join(f)),
            config.timeout_secs,
            config.max_reply_bytes,
        );
        transport
            .check_tls(&target)
            .map_err(|why| format!("endpoint `{name}`: {why}"))?;
        std::fs::create_dir_all(&home)
            .map_err(|err| format!("{} cannot be made: {err}", home.display()))?;
        let position_file = home.join(format!("{name}.json"));
        let lock_file = home.join(format!("{name}.lock"));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_file)
            .map_err(|err| format!("{} cannot be opened: {err}", lock_file.display()))?;
        // Called through the trait: the standard library's own `try_lock`
        // is newer than the workspace's minimum Rust version.
        match fs4::FileExt::try_lock(&lock) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => {
                return Err(format!(
                    "endpoint `{name}` of lake `{lake}` is delivered by another process ({} is \
                     locked)",
                    lock_file.display()
                ));
            }
            Err(fs4::TryLockError::Error(err)) => {
                return Err(format!("{} cannot be locked: {err}", lock_file.display()));
            }
        }
        let journal_file = home.join(format!("{name}.ndjson"));
        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_file)
            .map_err(|err| format!("{} cannot be opened: {err}", journal_file.display()))?;
        out.push(Deliverer {
            lake: lake.clone(),
            name: name.clone(),
            target,
            kinds: e.kinds.clone(),
            triggers: e.triggers.clone(),
            sets: e.sets.clone(),
            start: e.start,
            secret,
            transport,
            position_file,
            journal,
            _lock: lock,
            poll: Duration::from_millis(config.poll_millis),
            batch: config.batch,
            retry_first: Duration::from_secs(config.retry_first_secs),
            retry_max: Duration::from_secs(config.retry_max_secs),
            sleep: tokio_sleep(),
        });
    }
    Ok(out)
}

/// The position file's form.
#[derive(Serialize, Deserialize)]
struct Kept {
    next: u64,
}

impl Deliverer {
    /// The endpoint's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Wait with `sleep` instead of the runtime's clock: a test's own.
    pub fn with_sleep(mut self, sleep: Sleep) -> Deliverer {
        self.sleep = sleep;
        self
    }

    /// Deliver from the lake's feed until the process ends: poll, send
    /// what came, and wait `poll_millis` once caught up. A feed that fails
    /// is polled again after the same wait.
    pub async fn run(mut self, store: Arc<dyn LakeStore>) {
        loop {
            match self.round(store.as_ref()).await {
                Ok(true) => {}
                Ok(false) => (self.sleep)(self.poll).await,
                Err(why) => {
                    self.say(&why);
                    (self.sleep)(self.poll).await;
                }
            }
        }
    }

    /// One round: read the feed from the position and deliver or refuse
    /// every matching event, in order, retrying transient failures until
    /// they pass. Answers whether the round read a full batch, so more
    /// may be waiting.
    pub async fn round(&mut self, store: &dyn LakeStore) -> Result<bool, String> {
        let feed = store.feed();
        let filter = FeedFilter {
            kinds: self.kinds.clone(),
            types: vec![],
        };
        let from = match self.kept()? {
            Some(p) => p,
            None => {
                let first = match self.start {
                    Start::Genesis => Position::GENESIS,
                    Start::Head => store
                        .head()
                        .await
                        .map_err(|e| format!("the lake's head cannot be read: {e}"))?
                        .next(),
                };
                self.keep(first)?;
                first
            }
        };
        let batch = feed
            .poll(from, &filter, self.batch)
            .await
            .map_err(|e| format!("the feed failed: {e}"))?;
        let full = batch.events.len() >= self.batch as usize;
        for event in &batch.events {
            if self.wanted(event) {
                self.deliver(event).await;
            }
            self.keep(event.position.next())?;
        }
        if batch.next > from {
            self.keep(batch.next)?;
        }
        Ok(full)
    }

    /// Whether the endpoint receives an event: its kind, and the trigger
    /// or set when the endpoint names some.
    fn wanted(&self, e: &Event) -> bool {
        match &e.body {
            EventBody::TriggerFired { trigger, .. } => {
                self.triggers.is_empty() || self.triggers.iter().any(|t| t == trigger.as_str())
            }
            EventBody::ObjectSetChanged { set, .. } => {
                self.sets.is_empty() || self.sets.iter().any(|s| s == set.as_str())
            }
            _ => false,
        }
    }

    /// Send one event until it is delivered or refused.
    async fn deliver(&mut self, event: &Event) {
        let body = lakelands_core::canonical::of(event);
        let mut wait = self.retry_first.min(self.retry_max);
        let mut attempt: u32 = 1;
        loop {
            match self.attempt(event, &body, attempt).await {
                Answer::Delivered => {
                    self.journal(event, attempt, json!({ "outcome": "delivered" }));
                    return;
                }
                Answer::Refused(status) => {
                    self.journal(
                        event,
                        attempt,
                        json!({ "outcome": "refused", "status": status }),
                    );
                    self.say(&format!(
                        "position {}: refused with {status}; moving on",
                        event.position
                    ));
                    return;
                }
                Answer::Retry(why) => {
                    self.journal(
                        event,
                        attempt,
                        json!({ "outcome": "retrying", "why": why, "retry_in_secs": wait.as_secs() }),
                    );
                    self.say(&format!(
                        "position {}: {why}; trying again in {}s",
                        event.position,
                        wait.as_secs()
                    ));
                    (self.sleep)(wait).await;
                    wait = wait.saturating_mul(2).min(self.retry_max);
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }

    async fn attempt(&self, event: &Event, body: &[u8], attempt: u32) -> Answer {
        let mut headers: Vec<(HeaderName, HeaderValue)> = vec![
            (
                HeaderName::from_static("lakelands-lake"),
                header(self.lake.as_str()),
            ),
            (
                HeaderName::from_static("lakelands-position"),
                header(&event.position.0.to_string()),
            ),
            (
                HeaderName::from_static("lakelands-event"),
                header(&event.id.0),
            ),
            (
                HeaderName::from_static("lakelands-attempt"),
                header(&attempt.to_string()),
            ),
        ];
        if let Some(key) = &self.secret {
            headers.push((
                HeaderName::from_static("lakelands-signature"),
                header(&signature(key, body)),
            ));
        }
        match self
            .transport
            .post(&self.target, &headers, body.to_vec())
            .await
        {
            // The status is the verdict: a 2xx whose body cannot be read is
            // delivered all the same.
            Ok(_) | Err(Failure::Body(_)) => Answer::Delivered,
            Err(Failure::Status(s)) => {
                let code = s.as_u16();
                if s.is_server_error() || matches!(code, 401 | 403 | 404 | 408 | 429) {
                    Answer::Retry(format!("it answered {s}"))
                } else {
                    Answer::Refused(code)
                }
            }
            Err(Failure::TimedOut(t)) => {
                Answer::Retry(format!("it did not answer within {}s", t.as_secs()))
            }
            Err(Failure::Failed(why) | Failure::Refused(why) | Failure::Tls(why)) => {
                Answer::Retry(why)
            }
        }
    }

    /// The position to read from, as the position file keeps it.
    fn kept(&self) -> Result<Option<Position>, String> {
        match std::fs::read(&self.position_file) {
            Ok(bytes) => {
                let kept: Kept = serde_json::from_slice(&bytes).map_err(|e| {
                    format!(
                        "{} is not a position file: {e}",
                        self.position_file.display()
                    )
                })?;
                Ok(Some(Position(kept.next)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!(
                "{} cannot be read: {e}",
                self.position_file.display()
            )),
        }
    }

    /// Keep the position to read from next: written beside the file and
    /// renamed over it, so a crash leaves the old one or the new one.
    fn keep(&self, next: Position) -> Result<(), String> {
        let tmp = self.position_file.with_extension("json.tmp");
        let bytes = serde_json::to_vec(&Kept { next: next.0 }).expect("a position serialises");
        // Synced before the rename, so a power loss leaves the old
        // position or the new one, never an empty file.
        File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(&bytes)?;
                f.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, &self.position_file))
            .map_err(|e| format!("{} cannot be written: {e}", self.position_file.display()))
    }

    /// Append one outcome to the journal; a journal that cannot be written
    /// is said on standard error, and delivery goes on.
    fn journal(&mut self, event: &Event, attempt: u32, outcome: serde_json::Value) {
        let mut line = json!({
            "position": event.position.0,
            "id": event.id.0,
            "attempt": attempt,
            "at": now().to_string(),
        });
        if let (Some(line), serde_json::Value::Object(o)) = (line.as_object_mut(), outcome) {
            line.extend(o);
        }
        let mut bytes = serde_json::to_vec(&line).expect("a journal line serialises");
        bytes.push(b'\n');
        if let Err(e) = self.journal.write_all(&bytes) {
            self.say(&format!("the journal cannot be written: {e}"));
        }
    }

    fn say(&self, what: &str) {
        eprintln!(
            "lakelands: delivering lake `{}` to `{}` ({}): {what}",
            self.lake,
            self.name,
            self.target.shown()
        );
    }
}

fn header(v: &str) -> HeaderValue {
    HeaderValue::from_str(v).unwrap_or_else(|_| HeaderValue::from_static("-"))
}

/// `sha256=<hex>`: the HMAC-SHA256 of a body's bytes under a secret, as
/// the `Lakelands-Signature` header carries it.
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    signature(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret), body)
}

fn signature(key: &ring::hmac::Key, body: &[u8]) -> String {
    let tag = ring::hmac::sign(key, body);
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

fn now() -> Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros());
    Timestamp(i64::try_from(micros).unwrap_or(i64::MAX))
}
