//! The `http` backing: `POST` the call as JSON to the URL a function
//! names, and read the reply body as the output.
//!
//! A function names its URL in its configuration (`config: { url: … }`),
//! `http` or `https`. The URL's host must be on the operator's list
//! ([`HttpConfig::hosts`]), as `host` or as `host:port`; any other is
//! refused before a connection is made. The body is
//! `{ "function", "inputs", "config" }`; a `2xx` reply's body, JSON, is
//! the output. Any other status fails the call, redirects included: a
//! redirect is never followed, since it could lead off the list. No proxy
//! is used, and nothing is read from the environment: `https` trusts the
//! certificate authorities of [`HttpConfig::ca_file`], or of the first
//! system bundle found at a well-known path. A call that takes longer than
//! the timeout, connecting included, fails.

use async_trait::async_trait;
use lakelands_api::Result;
use lakelands_api::seam::{FunctionBacking, FunctionCall};
use serde_json::Value;

use crate::transport::{Failure, Target, Transport};
use crate::{HttpConfig, body, failed, output, refused};

/// The backing kind.
pub const KIND: &str = "http";

/// The `http` backing.
pub struct Http {
    config: HttpConfig,
    transport: Transport,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("config", &self.config)
            .finish()
    }
}

impl Http {
    /// The backing, allowed the hosts of `config`.
    pub fn new(config: HttpConfig) -> Http {
        let transport = Transport::new(
            "the http backing",
            config.ca_file.clone(),
            config.timeout_secs,
            config.max_reply_bytes,
        );
        Http { config, transport }
    }

    fn allows(&self, host: &str, port: u16) -> bool {
        let with_port = format!("{host}:{port}");
        self.config
            .hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host) || h.eq_ignore_ascii_case(&with_port))
    }

    async fn post(&self, call: &FunctionCall) -> Result<Value> {
        let url = call.config["url"].as_str().ok_or_else(|| {
            refused(
                call,
                "an `http` function names its URL as `config.url`".into(),
            )
        })?;
        let target =
            Target::parse(url).map_err(|why| refused(call, format!("`config.url` {why}")))?;
        let (host, port) = (target.host.as_str(), target.port);
        if !self.allows(host, port) {
            return Err(refused(
                call,
                format!("the host `{host}` (port {port}) is not one the operator allows"),
            ));
        }
        // What a failure names: never the URL as declared, which may carry
        // user information and reaches every reader of the field's error.
        let reply = self
            .transport
            .post(&target, &[], body(call))
            .await
            .map_err(|f| match f {
                Failure::Refused(why) => refused(call, why),
                Failure::TimedOut(t) => failed(
                    call,
                    format!("{} did not answer within {}s", target.shown(), t.as_secs()),
                ),
                Failure::Failed(why) | Failure::Body(why) | Failure::Tls(why) => failed(call, why),
                Failure::Status(status) => {
                    failed(call, Failure::status_sentence(status, "the http backing"))
                }
            })?;
        output(call, &reply)
    }
}

#[async_trait]
impl FunctionBacking for Http {
    fn kind(&self) -> &str {
        KIND
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        self.post(call).await
    }
}
