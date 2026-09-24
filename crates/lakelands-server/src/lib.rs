//! The server door: serves one lake's catalog as MCP tools, on stdio and
//! on streamable HTTP, and as REST routes beside MCP on the same HTTP
//! door ([`rest`], `docs/rest.md`).
//!
//! One MCP session serves one lake as one principal. The principal comes
//! from the [`resolver`] — the API key an HTTP caller presents, or the
//! local user that starts a stdio server — and never from anything the
//! client claims. The tools are exactly those
//! [`lakelands_api::tools::tools`] generates from what `describe` shows
//! that principal; every result carries its position; every error is one
//! of the contract's closed set, said as the CLI says it ([`say`]).
//!
//! 0.1.0 speaks `initialize`, `notifications/initialized`, `ping`,
//! `tools/list` and `tools/call`. The session's `as_of` pin is the
//! generated `pin` tool. Notifications of the change feed follow after
//! 0.1.0 over the same session.

#![forbid(unsafe_code)]

pub(crate) mod doors;
pub mod http;
pub mod resolver;
pub mod rest;
pub mod say;
pub mod session;
pub mod stdio;

pub use http::{HttpConfig, HttpServer, LakeOpener, serve_http};
pub use session::Session;
pub use stdio::{serve_io, serve_stdio};
