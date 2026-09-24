//! The server door: serves one catalog of typed tools over MCP, on
//! stdio and on HTTP.
//!
//! The MCP transport is not built yet. What is here already is the
//! default [`PrincipalResolver`](lakelands_api::seam::PrincipalResolver),
//! which the CLI uses too: both doors turn a caller into a principal the
//! same way.

#![forbid(unsafe_code)]

pub mod resolver;
