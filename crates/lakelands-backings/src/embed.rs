//! The embedding client: an [`EmbedBackend`] for any endpoint shaped like
//! `POST /v1/embeddings`.
//!
//! The operator names the endpoint, the model and the length of its
//! vectors ([`EmbedConfig`]); the endpoint's host is the only host the
//! client reaches. Texts go in order, at most [`EmbedConfig::max_batch`]
//! to a request, as `{"model", "input": [texts…]}` (with `"dimensions"`
//! when [`EmbedConfig::send_dimensions`] is on). A `2xx` reply is read as
//! `{"data": [{"index", "embedding": [numbers…]}, …]}`: items are placed
//! by `index`, every index of the request must appear exactly once, and
//! every vector must have the configured length and finite values. Other
//! members are ignored; the default float form is expected, since
//! `encoding_format` is never sent.
//!
//! An API key, when the operator gives one, is read from a file when the
//! client is built, never from the environment
//! (`docs/decisions/005-no-environment-reads.md`), and sent as a bearer
//! token. The connection is the crate's one transport: no proxy, no
//! redirect followed, TLS trusting the configured bundle or the system's,
//! and a timeout over each whole request.
//!
//! A `400`, `413` or `422` refuses the request for what it carries, and
//! is answered as `Invalid`, so the store asks again in halves; every
//! other failure is [`Error::Unavailable`]. Either names the endpoint's
//! host, port and path and what went wrong, and nothing else: never the
//! key, the texts, the request or the reply, since the texts are a
//! caller's query and a failure reaches every caller. Embedding no text
//! makes no request.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use hyper::header::{AUTHORIZATION, HeaderValue};
use lakelands_api::seam::EmbedBackend;
use lakelands_api::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::transport::{Failure, Target, Transport};

/// The embedding client's configuration: the `embed` section of a server
/// configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedConfig {
    /// The endpoint's URL, `http` or `https`: the only place the client
    /// sends anything.
    pub endpoint: String,
    /// The model's name, sent with every request and recorded as the
    /// procedure of every vector it makes.
    pub model: String,
    /// The length of every vector the endpoint answers.
    pub dimensions: u32,
    /// A file holding the API key, sent as `Authorization: Bearer <key>`.
    /// Relative to the configuration's directory. Absent, no key is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<PathBuf>,
    /// Whether to ask the endpoint for vectors of `dimensions` length, for
    /// endpoints that shorten them on request.
    #[serde(default)]
    pub send_dimensions: bool,
    /// The most texts sent in one request.
    #[serde(default = "default_max_batch")]
    pub max_batch: u32,
    /// How long one request may take, in seconds, connecting included.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// The largest reply body accepted, in bytes.
    #[serde(default = "default_max_reply_bytes")]
    pub max_reply_bytes: u64,
    /// The certificate authorities `https` trusts, as a PEM bundle,
    /// relative to the configuration's directory. When absent, the first
    /// system bundle found at a well-known path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

fn default_max_batch() -> u32 {
    64
}

fn default_timeout() -> u64 {
    30
}

fn default_max_reply_bytes() -> u64 {
    16 * 1024 * 1024
}

/// The embedding client.
pub struct Embed {
    config: EmbedConfig,
    target: Target,
    key: Option<HeaderValue>,
    transport: Transport,
}

impl std::fmt::Debug for Embed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is never shown.
        f.debug_struct("Embed")
            .field("config", &self.config)
            .finish()
    }
}

/// One item of a reply; other members are ignored.
#[derive(Deserialize)]
struct Item {
    index: usize,
    embedding: Vec<f64>,
}

#[derive(Deserialize)]
struct Reply {
    data: Vec<Item>,
}

impl Embed {
    /// The client `config` describes, its relative paths taken from `dir`,
    /// the configuration's directory. Refused, with the reason, when the
    /// endpoint is not an `http` or `https` URL or carries user
    /// information, a number is zero, the key file is named but cannot be
    /// read or is empty, or an `https` endpoint's host is not a name TLS
    /// can verify or its named certificate bundle holds no certificate: a
    /// host checks this before it serves anything.
    pub fn new(config: EmbedConfig, dir: &Path) -> std::result::Result<Embed, String> {
        let target = Target::parse(&config.endpoint)
            .map_err(|why| format!("the embedding endpoint {why}"))?;
        if target.user_info {
            return Err(
                "the embedding endpoint carries user information, which is never sent; name \
                 a key with `api_key_file` instead"
                    .into(),
            );
        }
        for (name, zero) in [
            ("dimensions", config.dimensions == 0),
            ("max_batch", config.max_batch == 0),
            ("timeout_secs", config.timeout_secs == 0),
            ("max_reply_bytes", config.max_reply_bytes == 0),
        ] {
            if zero {
                return Err(format!("the embedding client's `{name}` is zero"));
            }
        }
        let key = match &config.api_key_file {
            None => None,
            Some(file) => {
                let file = dir.join(file);
                let text = std::fs::read_to_string(&file).map_err(|e| {
                    format!(
                        "the embedding client's key file {} cannot be read: {e}",
                        file.display()
                    )
                })?;
                let key = text.trim();
                if key.is_empty() {
                    return Err(format!(
                        "the embedding client's key file {} is empty",
                        file.display()
                    ));
                }
                let mut value = HeaderValue::try_from(format!("Bearer {key}")).map_err(|_| {
                    format!(
                        "the embedding client's key file {} holds characters a header cannot carry",
                        file.display()
                    )
                })?;
                value.set_sensitive(true);
                Some(value)
            }
        };
        let transport = Transport::new(
            "the embedding client",
            config.ca_file.as_ref().map(|f| dir.join(f)),
            config.timeout_secs,
            config.max_reply_bytes,
        );
        transport.check_tls(&target)?;
        Ok(Embed {
            config,
            target,
            key,
            transport,
        })
    }

    fn failed(&self, why: impl std::fmt::Display) -> Error {
        Error::Unavailable {
            message: format!("the embedding endpoint {}: {why}", self.target.shown()),
        }
    }

    /// One request: the vectors for `texts`, in their order.
    async fn batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut request = json!({ "model": self.config.model, "input": texts });
        if self.config.send_dimensions {
            request["dimensions"] = json!(self.config.dimensions);
        }
        let body = serde_json::to_vec(&request).expect("a JSON value serialises");
        let headers: Vec<_> = self
            .key
            .iter()
            .map(|k| (AUTHORIZATION, k.clone()))
            .collect();
        let reply = self
            .transport
            .post(&self.target, &headers, body)
            .await
            .map_err(|f| match f {
                Failure::Refused(why) | Failure::Failed(why) | Failure::Body(why) => {
                    self.failed(why)
                }
                // Refused for what the request carries: asking with the
                // same texts again gets the same answer.
                Failure::Status(status) if matches!(status.as_u16(), 400 | 413 | 422) => {
                    Error::Refused {
                        field: None,
                        reason: lakelands_api::Refusal::Invalid {
                            message: format!(
                                "the embedding endpoint {}: it answered {status}",
                                self.target.shown()
                            ),
                        },
                    }
                }
                Failure::Status(status) => {
                    self.failed(Failure::status_sentence(status, "the embedding client"))
                }
                // The reason may name a bundle's path on the server, which
                // is the operator's to read, not every caller's.
                Failure::Tls(_) => self.failed(
                    "TLS cannot be set up: no usable certificate bundle; the operator names one \
                     as `ca_file`",
                ),
                Failure::TimedOut(t) => {
                    self.failed(format_args!("it did not answer within {}s", t.as_secs()))
                }
            })?;
        // The reply's text is never quoted: it may echo the input.
        let reply: Reply = serde_json::from_slice(&reply)
            .map_err(|_| self.failed("its reply is not an embeddings answer"))?;
        let want = self.config.dimensions as usize;
        let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        if reply.data.len() != texts.len() {
            return Err(self.failed(format_args!(
                "it answered {} vectors for {} texts",
                reply.data.len(),
                texts.len()
            )));
        }
        for item in reply.data {
            let Some(slot) = out.get_mut(item.index) else {
                return Err(self.failed(format_args!(
                    "it answered the index {} for {} texts",
                    item.index,
                    texts.len()
                )));
            };
            if slot.is_some() {
                return Err(self.failed(format_args!("it answered the index {} twice", item.index)));
            }
            if item.embedding.len() != want {
                return Err(self.failed(format_args!(
                    "it answered a vector of {} dimensions, not {want}",
                    item.embedding.len()
                )));
            }
            let vector: Vec<f32> = item.embedding.iter().map(|&x| x as f32).collect();
            if vector.iter().any(|x| !x.is_finite()) {
                return Err(self.failed("it answered a vector with a value that is not finite"));
            }
            *slot = Some(vector);
        }
        // As many items as texts, none twice and none out of range: every
        // slot is filled.
        Ok(out.into_iter().flatten().collect())
    }
}

#[async_trait]
impl EmbedBackend for Embed {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn dimensions(&self) -> u32 {
        self.config.dimensions
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.config.max_batch as usize) {
            out.extend(self.batch(chunk).await?);
        }
        Ok(out)
    }
}
