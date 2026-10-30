//! REST over the same catalog, beside MCP under `/lakes/<id>/`.
//!
//! One route family per door (`docs/rest.md` lists them). A route only
//! translates: its path, its query, its headers and its body become the
//! arguments of one [`Door`], under the names the door's MCP tool takes,
//! and [`doors::run`] calls the catalog exactly as an MCP tool call does.
//!
//! - Every request presents its API key (`Authorization: Bearer <key>`)
//!   and is resolved on its own, before anything about the routes is
//!   answered: there is no session and no pin; a read names `as_of`
//!   itself.
//! - Every reply to a caller whose key resolved carries the position in
//!   `Lakelands-Position`: the one the answer was read or written at, or
//!   the head of the log for an error.
//! - A failure is one of the contract's closed set, as JSON, with the two
//!   sentences a person reads (`what`, `todo`) beside it. A request the
//!   server cannot read is `refused` as `invalid`, with a case: `what` is
//!   the case's sentence and `todo` its advice ([`Failure::of`]).
//! - A write takes `If-Match: <version>` and `Idempotency-Key`; `PUT` also
//!   takes `If-None-Match: *` for a record that must not exist yet. A read
//!   of one record answers its version as `ETag`, and so does a write that
//!   made a version.
//! - `POST …/ingest/<source>` takes NDJSON and answers NDJSON, one line
//!   per commit window as it becomes durable.
//! - `GET …/feed` is the change feed as server-sent events, each event's
//!   id its position, so `Last-Event-ID` resumes. It is the log itself,
//!   unmasked, so only a principal whose grants
//!   [`reads_everything`](lakelands_api::Grants::reads_everything) may open
//!   it (`docs/decisions/037-the-feed-over-a-network-door.md`).

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_core::Stream;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use lakelands_api::error::{Case, Error, Form, Names, Refusal, Said, Situation};
use lakelands_api::id::{LakeId, Position, SourceId};
use lakelands_api::quarantine::QuarantineId;
use lakelands_api::read::POSITION_HEADER;
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::FeedFilter;
use lakelands_api::tools::{Door, tools};
use lakelands_api::{Catalog, LakeStore, Principal, WriteContext};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::bounds::{Held, Limit, Over};
use crate::doors::{self, Fault, to_json, unknown_argument};
use crate::http::{
    State, Turn, Turned, admit, forwarded, header, json_response, no_link_headers, read_json,
    resolve, retry_after,
};
use crate::resolver::host_of;
use crate::say::Failure;

/// A reply: its body whole, or streamed.
pub(crate) type Answer = Response<BoxBody<Bytes, Infallible>>;
/// A reply that ends a request early: a refusal, boxed.
type Stop = Box<Answer>;

/// The longest line an ingest body may carry.
const MAX_LINE: usize = 4 << 20;
/// A quiet feed sends a comment this often, so a proxy keeps it open and
/// a closed connection is noticed.
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How long the rest of an ingest body is read and dropped once the
/// ingest has stopped reading it, before the connection is closed anyway.
const DRAIN: Duration = KEEP_ALIVE;

// ---------------------------------------------------------------- replies

fn json_reply(status: StatusCode, v: &Value) -> Answer {
    json_response(status, v).map(BodyExt::boxed)
}

/// A failure as JSON: the contract's error with its two sentences.
fn failure_json(e: &Error, f: &Failure) -> Value {
    let mut v = to_json(e);
    let o = v.as_object_mut().expect("an error is an object");
    o.insert("what".into(), json!(f.what));
    o.insert("todo".into(), json!(f.todo));
    v
}

/// The status a contract error answers with.
fn status_of(e: &Error) -> StatusCode {
    match e {
        Error::Ingested { error, .. } => status_of(error),
        Error::Refused { reason, .. } => match reason {
            Refusal::NotGranted
            | Refusal::GrantNeeded { .. }
            | Refusal::WrongDoor
            | Refusal::VerbsOnly { .. }
            | Refusal::Reserved
            | Refusal::Declined(_) => StatusCode::FORBIDDEN,
            Refusal::Decided | Refusal::Unanswered(_) => StatusCode::CONFLICT,
            Refusal::Invalid(_)
            | Refusal::Constraint(_)
            | Refusal::Precondition(_)
            | Refusal::TooDeep { .. }
            | Refusal::TooLarge { .. }
            | Refusal::Migration { .. }
            | Refusal::OutOfRange { .. }
            | Refusal::Unknown(_) => StatusCode::UNPROCESSABLE_ENTITY,
        },
        Error::NotFound { .. } => StatusCode::NOT_FOUND,
        // The version the write expected came in `If-Match`.
        Error::Conflict { .. } => StatusCode::PRECONDITION_FAILED,
        Error::Unbound { .. } => StatusCode::NOT_IMPLEMENTED,
        Error::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        // The store says no error of a kind its build does not know.
        Error::Unknown(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn contract(e: Error) -> Stop {
    let f = Failure::from(e.clone());
    Box::new(json_reply(status_of(&e), &failure_json(&e, &f)))
}

/// A request the server cannot take as it is: refused as `invalid`,
/// saying its case and the case's advice.
fn invalid(status: StatusCode, said: impl Into<Said>) -> Stop {
    Box::new(json_reply(status, &invalid_json(said)))
}

/// A refusal as `invalid` with its case, as JSON with the two sentences.
pub(crate) fn invalid_json(said: impl Into<Said>) -> Value {
    let said = said.into();
    let f = Failure::of(&said);
    failure_json(&Error::invalid(said), &f)
}

/// A request the door turned away, said as REST says a refusal.
fn turned(t: Turned) -> Stop {
    match t.why {
        Turn::NotGranted(what, todo) => {
            let e = Error::Refused {
                field: None,
                reason: Refusal::NotGranted,
            };
            let f = Failure::new(what, todo);
            Box::new(json_reply(t.status, &failure_json(&e, &f)))
        }
        Turn::Invalid(case) => invalid(t.status, case),
        Turn::Bounded(over) => {
            let (_, v) = bounded_json(&over);
            Box::new(retry_after(json_reply(t.status, &v), &over))
        }
    }
}

/// A request the door turned away before its route was known.
pub(crate) fn refused(t: Turned) -> Answer {
    *turned(t)
}

/// A bound reached, as the contract's error and as its JSON with the
/// two sentences.
pub(crate) fn bounded_json(over: &Over) -> (Error, Value) {
    let e = Error::unavailable((*over.situation).clone());
    let v = failure_json(&e, &Failure::from(e.clone()));
    (e, v)
}

fn with_position(mut r: Answer, p: Option<Position>) -> Answer {
    if let Some(p) = p {
        r.headers_mut()
            .insert(POSITION_HEADER, HeaderValue::from(p.0));
    }
    r
}

// --------------------------------------------------------------- decoding

/// Percent-decode one path segment or query component; `+` is a space
/// in a query. `None` when it is not valid percent-encoded UTF-8.
fn decode(s: &str, plus_is_space: bool) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'%' => {
                let hi = (bytes.next()? as char).to_digit(16)?;
                let lo = (bytes.next()? as char).to_digit(16)?;
                out.push((hi * 16 + lo) as u8);
            }
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
    }
    String::from_utf8(out).ok()
}

/// How a query parameter's text becomes an argument.
#[derive(Clone, Copy)]
enum Kind {
    /// As it is.
    Text,
    /// A non-negative integer.
    Int,
    /// `true`, `false`, or present without a value (`true`).
    Flag,
    /// JSON.
    Json,
    /// Sort keys: `due_date,-amount,known_at(status),-latest()`, `-` for
    /// descending; or the JSON array the door takes.
    Order,
    /// Comma-separated words.
    List,
    /// One value per occurrence, given as often as needed: for values
    /// that may hold a comma, as a record key may.
    Each,
}

const AT: [(&str, Kind); 2] = [("as_of", Kind::Int), ("valid_at", Kind::Text)];
const FIND: [(&str, Kind); 13] = [
    ("where", Kind::Text),
    ("order_by", Kind::Order),
    ("limit", Kind::Int),
    ("cursor", Kind::Text),
    ("offset", Kind::Int),
    ("total", Kind::Flag),
    ("retired", Kind::Flag),
    ("after", Kind::Json),
    ("aggregate", Kind::Json),
    ("with_meta", Kind::Flag),
    ("params", Kind::Json),
    AT[0],
    AT[1],
];
const GET: [(&str, Kind); 4] = [
    ("with_meta", Kind::Flag),
    ("params", Kind::Json),
    AT[0],
    AT[1],
];
const PROPOSALS: [(&str, Kind); 8] = [
    ("state", Kind::Text),
    ("type", Kind::Text),
    ("kind", Kind::Text),
    ("limit", Kind::Int),
    ("order_by", Kind::Order),
    ("cursor", Kind::Text),
    AT[0],
    AT[1],
];
const QUARANTINED: [(&str, Kind); 7] = [
    ("source", Kind::Text),
    ("type", Kind::Text),
    ("delivery", Kind::Text),
    ("index", Kind::Int),
    ("state", Kind::Text),
    ("limit", Kind::Int),
    ("after", Kind::Json),
];
const FEED: [(&str, Kind); 4] = [
    ("from", Kind::Int),
    ("kinds", Kind::List),
    ("types", Kind::List),
    ("key", Kind::Each),
];

fn order(text: &str) -> Option<Value> {
    if text.starts_with('[') {
        return serde_json::from_str(text).ok();
    }
    let keys = text.split(',').map(|k| {
        let (dir, k) = match k.strip_prefix('-') {
            Some(k) => ("desc", k),
            None => ("asc", k),
        };
        let mut o = Map::new();
        if k == "latest()" {
            o.insert("by".into(), json!("latest"));
            o.insert("dir".into(), json!(dir));
            return Some(Value::Object(o));
        }
        let field = ["known_at", "seen_at"].iter().find_map(|by| {
            let f = k.strip_prefix(by)?.strip_prefix('(')?.strip_suffix(')')?;
            o.insert("by".into(), json!(by));
            Some(f)
        });
        let field = field.unwrap_or(k);
        (!field.is_empty()).then_some(())?;
        o.insert("field".into(), json!(field));
        o.insert("dir".into(), json!(dir));
        Some(Value::Object(o))
    });
    keys.collect::<Option<Vec<_>>>().map(Value::Array)
}

/// The query string as arguments, each parameter one this route takes,
/// named once.
fn query_args(
    query: Option<&str>,
    takes: &[(&str, Kind)],
    route: &str,
) -> Result<Map<String, Value>, Stop> {
    let mut out = Map::new();
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (decode(k, true), decode(v, true)) else {
            return Err(invalid(StatusCode::BAD_REQUEST, Case::QueryNotEncoded));
        };
        let Some((_, kind)) = takes.iter().find(|(p, _)| *p == k) else {
            let known: Names = takes.iter().map(|(p, _)| p).collect();
            let case = Case::NoParameter {
                route: route.into(),
                parameter: k,
                takes: (!known.0.is_empty()).then_some(known),
            };
            return Err(invalid(StatusCode::BAD_REQUEST, case));
        };
        let value = match kind {
            Kind::Text => Some(json!(v)),
            Kind::Int => v.parse::<u64>().ok().map(|n| json!(n)),
            Kind::Flag => match v.as_str() {
                "" | "true" => Some(json!(true)),
                "false" => Some(json!(false)),
                _ => None,
            },
            Kind::Json => serde_json::from_str(&v).ok(),
            Kind::Order => order(&v),
            Kind::List => Some(json!(
                v.split(',').filter(|w| !w.is_empty()).collect::<Vec<_>>()
            )),
            Kind::Each => (!v.is_empty()).then(|| json!(v)),
        };
        let Some(value) = value else {
            // Text and a list take any value.
            let form = match kind {
                Kind::Int => Form::Whole,
                Kind::Flag => Form::Flag,
                Kind::Json => Form::Json,
                Kind::Order => Form::Order,
                Kind::Each | Kind::Text | Kind::List => Form::Given,
            };
            let case = Case::ParameterForm {
                parameter: k,
                value: v,
                form,
            };
            return Err(invalid(StatusCode::BAD_REQUEST, case));
        };
        if matches!(kind, Kind::Each) {
            let all = out.entry(k.clone()).or_insert_with(|| json!([]));
            all.as_array_mut()
                .expect("an `Each` holds a list")
                .push(value);
            continue;
        }
        if out.insert(k.clone(), value).is_some() {
            let case = Case::ParameterTwice { parameter: k };
            return Err(invalid(StatusCode::BAD_REQUEST, case));
        }
    }
    Ok(out)
}

// ----------------------------------------------------------------- routes

/// What a route's body is.
#[derive(Clone, Copy, PartialEq)]
enum BodyAs {
    /// It takes none.
    Nothing,
    /// A JSON object whose members are the door's arguments.
    Args,
    /// A JSON object that is one argument; `required` says whether an
    /// empty body is refused or stands for `{}`.
    One(&'static str, bool),
}

/// A request, translated to a door.
struct Route {
    door: Door,
    /// How a refusal names the route.
    label: &'static str,
    /// Arguments the path gives.
    path: Map<String, Value>,
    /// The query parameters the route takes.
    query: &'static [(&'static str, Kind)],
    body: BodyAs,
    /// Which write headers it reads.
    writes: Writes,
    /// Whether its body carries a migration's `report_id`.
    report: Report,
}

/// The write headers a route reads.
#[derive(Clone, Copy, PartialEq)]
enum Writes {
    /// None: it does not write a record.
    No,
    /// `Idempotency-Key` alone: a verb called on its type has no record
    /// version to compare.
    Once,
    /// `If-Match` and `Idempotency-Key`.
    Cas,
    /// Those, and `If-None-Match: *` for a record that must not exist.
    CasOrNew,
}

/// Whether a route's body carries a migration's `report_id`.
#[derive(Clone, Copy, PartialEq)]
enum Report {
    /// Not a migration: the door's own schema decides.
    Any,
    /// The preflight: it takes none.
    Refused,
    /// Applying: it needs the one its preflight answered.
    Required,
}

enum Target {
    Door(Route),
    Ingest(SourceId),
    Feed,
    Quarantined,
    Dismiss,
    Quarantine,
}

fn route(method: &Method, rest: &str) -> Result<Target, Stop> {
    let Some(rest) = rest
        .split('/')
        .map(|s| decode(s, false).filter(|s| !s.is_empty()))
        .collect::<Option<Vec<_>>>()
    else {
        return Err(no_route());
    };
    let rest = rest.as_slice();
    if let Some(t) = matched(method, rest) {
        return Ok(t);
    }
    // The path is known under another method: say which it takes.
    let allowed: Vec<&str> = [Method::GET, Method::PUT, Method::DELETE, Method::POST]
        .iter()
        .filter(|m| matched(m, rest).is_some())
        .map(Method::as_str)
        .collect();
    if allowed.is_empty() {
        return Err(no_route());
    }
    let case = Case::RouteMethod {
        allow: allowed.iter().collect(),
        method: method.as_str().into(),
    };
    let allow = allowed.join(", ");
    let mut a = invalid(StatusCode::METHOD_NOT_ALLOWED, case);
    a.headers_mut().insert(
        "allow",
        HeaderValue::from_str(&allow).expect("methods are a header value"),
    );
    Err(a)
}

fn no_route() -> Stop {
    invalid(StatusCode::NOT_FOUND, Case::NoRoute)
}

/// The door a method and a path lead to, if any.
fn matched(method: &Method, rest: &[String]) -> Option<Target> {
    let s: Vec<&str> = rest.iter().map(String::as_str).collect();
    let arg = |k: &str, v: &str| Map::from_iter([(k.to_owned(), json!(v))]);
    let r = |door, label, path, query: &'static [(&'static str, Kind)], body, writes| {
        Target::Door(Route {
            door,
            label,
            path,
            query,
            body,
            writes,
            report: Report::Any,
        })
    };
    let migrate = |label, report| {
        Target::Door(Route {
            door: Door::Migrate,
            label,
            path: Map::new(),
            query: &[],
            body: BodyAs::Args,
            writes: Writes::No,
            report,
        })
    };
    let (m, none) = (method.clone(), Map::new());
    let target = match (s.as_slice(), m) {
        (["types", ty, "records"], Method::GET) => r(
            Door::Find { ty: (*ty).into() },
            "GET …/types/<type>/records",
            none,
            &FIND,
            BodyAs::Nothing,
            Writes::No,
        ),
        (["types", ty, "records", key], Method::GET) => r(
            Door::Get { ty: (*ty).into() },
            "GET …/types/<type>/records/<key>",
            arg("key", key),
            &GET,
            BodyAs::Nothing,
            Writes::No,
        ),
        (["types", ty, "records", key], Method::PUT) => r(
            Door::Upsert { ty: (*ty).into() },
            "PUT …/types/<type>/records/<key>",
            arg("key", key),
            &[],
            BodyAs::One("fields", true),
            Writes::CasOrNew,
        ),
        (["types", ty, "records", key], Method::DELETE) => r(
            Door::Retire { ty: (*ty).into() },
            "DELETE …/types/<type>/records/<key>",
            arg("key", key),
            &[],
            BodyAs::Nothing,
            Writes::Cas,
        ),
        (["types", ty, "records", key, "links", relation, to], Method::PUT) => r(
            Door::Link {
                ty: (*ty).into(),
                relation: (*relation).into(),
            },
            "PUT …/types/<type>/records/<key>/links/<relation>/<to>",
            Map::from_iter([("key".to_owned(), json!(key)), ("to".to_owned(), json!(to))]),
            &[],
            BodyAs::One("fields", false),
            Writes::CasOrNew,
        ),
        (["types", ty, "records", key, "links", relation, to], Method::DELETE) => r(
            Door::Unlink {
                ty: (*ty).into(),
                relation: (*relation).into(),
            },
            "DELETE …/types/<type>/records/<key>/links/<relation>/<to>",
            Map::from_iter([("key".to_owned(), json!(key)), ("to".to_owned(), json!(to))]),
            &[],
            BodyAs::Nothing,
            Writes::Cas,
        ),
        (["verbs", verb], Method::POST) => r(
            Door::Call {
                verb: (*verb).into(),
            },
            "POST …/verbs/<verb>",
            none,
            &[],
            BodyAs::One("payload", false),
            Writes::Once,
        ),
        (["verbs", verb, key], Method::POST) => r(
            Door::Call {
                verb: (*verb).into(),
            },
            "POST …/verbs/<verb>/<key>",
            arg("key", key),
            &[],
            BodyAs::One("payload", false),
            Writes::Cas,
        ),
        (["query"], Method::POST) => r(
            Door::Query,
            "POST …/query",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["cypher"], Method::POST) => r(
            Door::Cypher,
            "POST …/cypher",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["search"], Method::POST) => r(
            Door::Search,
            "POST …/search",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["describe"], Method::GET) => r(
            Door::Describe,
            "GET …/describe",
            none,
            &AT,
            BodyAs::Nothing,
            Writes::No,
        ),
        (["describe", ty], Method::GET) => r(
            Door::Describe,
            "GET …/describe/<type>",
            arg("type", ty),
            &AT,
            BodyAs::Nothing,
            Writes::No,
        ),
        (["proposals"], Method::GET) => r(
            Door::Proposals,
            "GET …/proposals",
            none,
            &PROPOSALS,
            BodyAs::Nothing,
            Writes::No,
        ),
        (["proposals"], Method::POST) => r(
            Door::Propose,
            "POST …/proposals",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["proposals", id, "accept"], Method::POST) => r(
            Door::Accept,
            "POST …/proposals/<id>/accept",
            arg("proposal", id),
            &[],
            BodyAs::Nothing,
            Writes::No,
        ),
        (["proposals", id, "accept_planned"], Method::POST) => r(
            Door::Accept,
            "POST …/proposals/<id>/accept_planned",
            arg("proposal", id),
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["proposals", id, "reject"], Method::POST) => r(
            Door::Reject,
            "POST …/proposals/<id>/reject",
            arg("proposal", id),
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "declare_type"], Method::POST) => r(
            Door::DeclareType,
            "POST …/schema/declare_type",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "declare"], Method::POST) => r(
            Door::Declare,
            "POST …/schema/declare",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "apply"], Method::POST) => r(
            Door::Apply,
            "POST …/schema/apply",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "add_field"], Method::POST) => r(
            Door::AddField,
            "POST …/schema/add_field",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "recompute"], Method::POST) => r(
            Door::Recompute,
            "POST …/schema/recompute",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "plan"], Method::POST) => r(
            Door::Plan,
            "POST …/schema/plan",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "enact"], Method::POST) => r(
            Door::Enact,
            "POST …/schema/enact",
            none,
            &[],
            BodyAs::Args,
            Writes::No,
        ),
        (["schema", "migrate"], Method::POST) => migrate("POST …/schema/migrate", Report::Refused),
        (["schema", "migrate", "apply"], Method::POST) => {
            migrate("POST …/schema/migrate/apply", Report::Required)
        }
        (["ingest", source], Method::POST) => Target::Ingest((*source).into()),
        (["feed"], Method::GET) => Target::Feed,
        (["quarantined"], Method::GET) => Target::Quarantined,
        (["quarantined", "dismiss"], Method::POST) => Target::Dismiss,
        (["quarantined"], Method::POST) => Target::Quarantine,
        _ => return None,
    };
    Some(target)
}

// ---------------------------------------------------------------- serving

/// Serve one REST request. `rest` is the raw path after `/lakes/`.
pub(crate) async fn handle(state: &State, req: Request<Incoming>, rest: &str) -> Answer {
    match serve(state, req, rest).await {
        Ok(a) => a,
        Err(a) => *a,
    }
}

async fn serve(state: &State, req: Request<Incoming>, rest: &str) -> Result<Answer, Stop> {
    // Nothing about the routes is answered before the key resolves.
    let key = admit(state, req.headers()).map_err(turned)?;
    let (lake, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let lake = match decode(lake, false) {
        // A lake's id is one segment, as it is for MCP.
        Some(id) if !id.is_empty() && !id.contains('/') => LakeId::from(id.as_str()),
        _ => return Err(no_route()),
    };
    // A host's key makes the call one forwarded over the link between
    // hosts, as the principal it names; a principal's key may not name
    // another or set a write context.
    let host = host_of(&state.hosts, &key);
    // The request is counted against the principal's bounds until it is
    // answered, an ingest until its last window is committed (095).
    let (who, cx, held) = match host {
        Some(h) => forwarded(state, h, &lake, req.headers()).await,
        None => match no_link_headers(req.headers()) {
            Ok(()) => resolve(state, &lake, key)
                .await
                .map(|(w, held)| (w, WriteContext::default(), held)),
            Err(t) => Err(t),
        },
    }
    .map_err(turned)?;
    let store = (state.open)(&lake).map_err(contract)?;
    // A forwarded call goes to the writer and nowhere further: a host
    // that is not it says so before the body is read, and the forwarder
    // tries its next door, so no call is forwarded twice.
    if host.is_some() && !store.is_writer() {
        let lake = Some(lake.to_string());
        let read_only = Situation::ReadOnly {
            lake,
            leased_elsewhere: true,
        };
        return Err(contract(Error::unavailable(read_only)));
    }
    let most = state.gate.bounds.body_bytes;
    let answer = match route(req.method(), rest) {
        Ok(Target::Door(r)) => door(&store, &who, cx, r, req, most).await,
        Ok(Target::Ingest(source)) => ingest(&store, &who, cx, source, req, held).await,
        Ok(Target::Feed) => feed(state, &store, &who, req).await,
        Ok(Target::Quarantined) => quarantined(&store, &who, cx, req, most).await,
        Ok(Target::Dismiss) => dismiss(&store, &who, cx, req, most).await,
        Ok(Target::Quarantine) => quarantine(&store, &who, cx, req, most).await,
        Err(a) => Err(a),
    };
    match answer {
        Ok(a) => Ok(a),
        // A failure answers the head the lake was at when it answered.
        Err(a) => Ok(with_position(*a, store.head().await.ok())),
    }
}

/// A JSON body, whole and no larger than `most`; `None` when it is
/// empty.
async fn json_body(req: Request<Incoming>, most: Limit) -> Result<Option<Value>, Stop> {
    let (parts, body) = req.into_parts();
    read_json(&parts, body, most).await.map_err(turned)
}

/// The compare-and-set and idempotency headers, as the arguments they
/// stand for.
fn write_headers(h: &HeaderMap, new: bool, a: &mut Map<String, Value>) -> Result<(), Stop> {
    // A weak validator does not name one version, so it cannot guard a
    // compare-and-set.
    let version = |v: &str| {
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        v.parse::<u64>().ok()
    };
    let expected = match (header(h, "if-match"), header(h, "if-none-match")) {
        (Some(_), Some(_)) => {
            return Err(invalid(StatusCode::BAD_REQUEST, Case::IfMatchBoth));
        }
        (Some(v), None) => Some(version(v).ok_or_else(|| {
            let value = v.into();
            invalid(StatusCode::BAD_REQUEST, Case::IfMatchNoVersion { value })
        })?),
        (None, Some(_)) if !new => {
            return Err(invalid(StatusCode::BAD_REQUEST, Case::IfNoneMatchNotPut));
        }
        (None, Some("*")) => Some(0),
        (None, Some(v)) => {
            let value = v.into();
            return Err(invalid(
                StatusCode::BAD_REQUEST,
                Case::IfNoneMatchValue { value },
            ));
        }
        (None, None) => None,
    };
    if let Some(v) = expected {
        a.insert("expected_version".into(), json!(v));
    }
    if let Some(k) = header(h, "idempotency-key") {
        a.insert("idempotency_key".into(), json!(k));
    }
    Ok(())
}

/// The idempotency header of a call with no record version to compare,
/// a verb's on its type: a compare-and-set header is refused, naming the
/// header and the verb.
fn once_headers(h: &HeaderMap, door: &Door, a: &mut Map<String, Value>) -> Result<(), Stop> {
    let verb = match door {
        Door::Call { verb } => verb.clone(),
        _ => "".into(),
    };
    for (name, lower) in [("If-Match", "if-match"), ("If-None-Match", "if-none-match")] {
        if header(h, lower).is_some() {
            let case = Case::VerbTypeHeader {
                header: name.into(),
                verb,
            };
            return Err(invalid(StatusCode::BAD_REQUEST, case));
        }
    }
    if let Some(k) = header(h, "idempotency-key") {
        a.insert("idempotency_key".into(), json!(k));
    }
    Ok(())
}

async fn door(
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    cx: WriteContext,
    r: Route,
    req: Request<Incoming>,
    most: Limit,
) -> Result<Answer, Stop> {
    let mut a = r.path;
    a.extend(query_args(req.uri().query(), r.query, r.label)?);
    match r.writes {
        Writes::No => {}
        Writes::Once => once_headers(req.headers(), &r.door, &mut a)?,
        Writes::Cas | Writes::CasOrNew => {
            write_headers(req.headers(), r.writes == Writes::CasOrNew, &mut a)?
        }
    }
    let catalog = store.catalog(cx);
    match r.body {
        BodyAs::Nothing => {
            if json_body(req, most).await?.is_some() {
                let route = r.label.into();
                return Err(invalid(StatusCode::BAD_REQUEST, Case::NoBody { route }));
            }
        }
        BodyAs::One(arg, required) => match json_body(req, most).await? {
            Some(v @ Value::Object(_)) => {
                a.insert(arg.into(), v);
            }
            None if !required => {
                a.insert(arg.into(), json!({}));
            }
            _ => {
                let case = Case::BodyObjectOf {
                    route: r.label.into(),
                    argument: arg.into(),
                };
                return Err(invalid(StatusCode::BAD_REQUEST, case));
            }
        },
        BodyAs::Args => {
            let body = match json_body(req, most).await? {
                Some(Value::Object(o)) => o,
                None => Map::new(),
                Some(_) => {
                    let route = r.label.into();
                    return Err(invalid(
                        StatusCode::BAD_REQUEST,
                        Case::BodyNotObject { route },
                    ));
                }
            };
            if let Some(k) = body.keys().find(|k| a.contains_key(*k)) {
                let argument = k.clone();
                return Err(invalid(
                    StatusCode::BAD_REQUEST,
                    Case::PathNames { argument },
                ));
            }
            let names = body.contains_key("report_id");
            let wrong = match r.report {
                Report::Any => None,
                Report::Refused => names.then_some(Case::PreflightReportId),
                Report::Required => (!names).then_some(Case::ApplyNeedsReportId),
            };
            if let Some(case) = wrong {
                return Err(invalid(StatusCode::BAD_REQUEST, case));
            }
            a.extend(body);
            // Refuse what the door does not take before anything runs, as
            // MCP does. The door's own schema is the one its tool has; a
            // principal without the tool is refused by the catalog.
            let d = catalog
                .describe(who, Default::default())
                .await
                .map_err(contract)?;
            let specs = tools(&d.data).map_err(|problems| {
                let names = lakelands_api::error::Situation::ToolNames { problems };
                contract(Error::unavailable(names))
            })?;
            if let Some(spec) = specs.iter().find(|s| s.door == r.door)
                && let Some(fault) = unknown_argument(spec, &a)
            {
                return Err(fault_answer(fault));
            }
        }
    }
    let v = doors::run(catalog.as_ref(), who, &r.door, r.label, (a, None))
        .await
        .map_err(fault_answer)?;
    let status = if v["outcome"] == "proposed" {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    let mut answer = json_reply(status, &v);
    // A read answers the record's version in `data`, a write the version
    // it made in the outcome.
    let version = match r.door {
        Door::Get { .. } => v["data"]["version"].as_u64(),
        _ => v["version"].as_u64(),
    };
    if let Some(version) = version {
        answer.headers_mut().insert(
            "etag",
            HeaderValue::from_str(&format!("\"{version}\"")).expect("digits are a header value"),
        );
    }
    Ok(with_position(answer, v["position"].as_u64().map(Position)))
}

fn fault_answer(f: Fault) -> Stop {
    match f {
        Fault::Contract(e) => contract(e),
        Fault::Arguments(said) => invalid(StatusCode::BAD_REQUEST, *said),
    }
}

// ----------------------------------------------------------------- stream

/// A streamed body: chunks as a task sends them, until it drops its end.
struct Chunks(mpsc::Receiver<Bytes>);

impl Body for Chunks {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0.poll_recv(cx).map(|c| c.map(|b| Ok(Frame::data(b))))
    }
}

pub(crate) fn streamed(content_type: &'static str, rx: mpsc::Receiver<Bytes>) -> Answer {
    let mut r = Response::new(Chunks(rx).boxed());
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    r
}

fn line(v: &Value) -> Bytes {
    let mut b = serde_json::to_vec(v).expect("JSON prints");
    b.push(b'\n');
    Bytes::from(b)
}

// ----------------------------------------------------------------- ingest

/// The records of an NDJSON body, as it arrives. A line that is not a
/// record ends the stream; what it was is left in `fault`.
///
/// Dropped before the body's end — a refused line, a failed commit — it
/// reads the rest of the body and drops it, for at most [`DRAIN`], so the
/// connection is not closed on bytes the client is still sending: a close
/// with request bytes unread answers them with a reset, and the client
/// may lose the answer to it.
struct Lines {
    body: Option<Incoming>,
    buf: Vec<u8>,
    line: usize,
    ended: bool,
    stopped: bool,
    fault: Arc<Mutex<Option<Case>>>,
}

impl Lines {
    fn stop(&mut self, why: Case) -> Poll<Option<SourceRecord>> {
        self.stopped = true;
        *self.fault.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        Poll::Ready(None)
    }
}

impl Stream for Lines {
    type Item = SourceRecord;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        let this = self.get_mut();
        loop {
            if this.stopped {
                return Poll::Ready(None);
            }
            if let Some(i) = this.buf.iter().position(|b| *b == b'\n') {
                // The line is held to the cap however the body was cut: a
                // read may bring its last bytes and its line feed at once.
                if i > MAX_LINE {
                    let line = this.line + 1;
                    return this.stop(Case::LineTooLong {
                        line,
                        max: MAX_LINE,
                    });
                }
                let text: Vec<u8> = this.buf.drain(..=i).collect();
                this.line += 1;
                if text.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return match serde_json::from_slice::<SourceRecord>(&text) {
                    Ok(r) => Poll::Ready(Some(r)),
                    Err(e) => {
                        let (line, fault, column) = (this.line, (&e).into(), e.column());
                        this.stop(Case::LineNotRecord {
                            line,
                            fault,
                            column,
                        })
                    }
                };
            }
            if this.ended {
                return Poll::Ready(None);
            }
            if this.buf.len() > MAX_LINE {
                let line = this.line + 1;
                return this.stop(Case::LineTooLong {
                    line,
                    max: MAX_LINE,
                });
            }
            let Some(body) = this.body.as_mut() else {
                return Poll::Ready(None);
            };
            match Pin::new(body).poll_frame(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    this.ended = true;
                    if !this.buf.is_empty() {
                        this.buf.push(b'\n');
                    }
                }
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        this.buf.extend_from_slice(&data);
                    }
                }
                // The transport's own words name nothing a caller needs.
                Poll::Ready(Some(Err(_))) => {
                    let line = this.line + 1;
                    this.body = None;
                    return this.stop(Case::BodyCutOff { line });
                }
            }
        }
    }
}

impl Drop for Lines {
    fn drop(&mut self) {
        let Some(mut body) = self.body.take() else {
            return;
        };
        if self.ended || body.is_end_stream() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = tokio::time::timeout(DRAIN, async {
                    while let Some(Ok(_)) = body.frame().await {}
                })
                .await;
            });
        }
    }
}

async fn ingest(
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    cx: WriteContext,
    source: SourceId,
    req: Request<Incoming>,
    held: Held,
) -> Result<Answer, Stop> {
    let args = query_args(
        req.uri().query(),
        &[
            ("delivery", Kind::Text),
            ("sealed_apart", Kind::Flag),
            ("commit_window", Kind::Int),
            ("quarantine_refused", Kind::Flag),
            ("run", Kind::Text),
        ],
        "POST …/ingest/<source>",
    )?;
    let delivery = args.get("delivery").and_then(Value::as_str);
    let opts = lakelands_api::IngestOpts {
        delivery: delivery.map(Into::into),
        sealed_apart: args.get("sealed_apart").and_then(Value::as_bool) == Some(true),
        commit_window: args
            .get("commit_window")
            .and_then(Value::as_u64)
            .map(|w| u32::try_from(w).unwrap_or(u32::MAX)),
        quarantine_refused: args.get("quarantine_refused").and_then(Value::as_bool) == Some(true),
        run: args.get("run").and_then(Value::as_str).map(Into::into),
    };
    if let Some(ct) = header(req.headers(), CONTENT_TYPE.as_str())
        && ![
            "application/x-ndjson",
            "application/jsonl",
            "application/json",
        ]
        .iter()
        .any(|t| ct.starts_with(t))
    {
        return Err(invalid(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Case::IngestMediaType,
        ));
    }
    let head = store.head().await.ok();
    let fault = Arc::new(Mutex::new(None));
    let lines = Lines {
        body: Some(req.into_body()),
        buf: Vec::new(),
        line: 0,
        ended: false,
        stopped: false,
        fault: fault.clone(),
    };
    let catalog: Arc<dyn Catalog> = store.catalog(cx);
    let mut commits = catalog
        .ingest_with(who, &source, opts, Box::pin(lines))
        .await
        .map_err(contract)?;
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(async move {
        let (_catalog, _held) = (catalog, held);
        loop {
            // A window that takes longer than the keep-alive to commit is
            // answered with an empty line meanwhile, so a client that
            // holds the answer to a silence bound hears the server works.
            let next = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx));
            let c = match tokio::time::timeout(KEEP_ALIVE, next).await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => {
                    if tx.send(Bytes::from_static(b"\n")).await.is_err() {
                        return;
                    }
                    continue;
                }
            };
            let out = match c {
                Ok(c) => line(&to_json(&c)),
                Err(e) => {
                    let f = Failure::from(e.clone());
                    let _ = tx.send(line(&failure_json(&e, &f))).await;
                    return;
                }
            };
            if tx.send(out).await.is_err() {
                return;
            }
        }
        let why = fault.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(why) = why {
            let said = Said::from(why);
            let f = Failure::of(&said);
            let _ = tx
                .send(line(&failure_json(&Error::invalid(said), &f)))
                .await;
        }
    });
    Ok(with_position(streamed("application/x-ndjson", rx), head))
}

// ------------------------------------------------------------ quarantined

/// `GET …/quarantined`: the quarantined records a filter asks for, in id
/// order (085).
async fn quarantined(
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    cx: WriteContext,
    req: Request<Incoming>,
    most: Limit,
) -> Result<Answer, Stop> {
    const LABEL: &str = "GET …/quarantined";
    let a = query_args(req.uri().query(), &QUARANTINED, LABEL)?;
    if json_body(req, most).await?.is_some() {
        let route = LABEL.into();
        return Err(invalid(StatusCode::BAD_REQUEST, Case::NoBody { route }));
    }
    let filter = doors::args(LABEL, &a).map_err(fault_answer)?;
    let r = store
        .catalog(cx)
        .quarantined(who, filter)
        .await
        .map_err(contract)?;
    Ok(with_position(
        json_reply(StatusCode::OK, &to_json(&r)),
        Some(r.position),
    ))
}

/// The body of a `POST` that takes no query, an object read as `T`.
async fn object_body<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
    label: &'static str,
    most: Limit,
) -> Result<T, Stop> {
    query_args(req.uri().query(), &[], label)?;
    let Some(Value::Object(body)) = json_body(req, most).await? else {
        let route = label.into();
        return Err(invalid(
            StatusCode::BAD_REQUEST,
            Case::BodyNotObject { route },
        ));
    };
    doors::args(label, &body).map_err(fault_answer)
}

/// `POST …/quarantined/dismiss`, its body the id: close one as
/// dismissed (085).
async fn dismiss(
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    cx: WriteContext,
    req: Request<Incoming>,
    most: Limit,
) -> Result<Answer, Stop> {
    let id: QuarantineId = object_body(req, "POST …/quarantined/dismiss", most).await?;
    let close = store
        .catalog(cx)
        .dismiss_quarantined(who, &id)
        .await
        .map_err(contract)?;
    let at = Some(close.position);
    Ok(with_position(
        json_reply(StatusCode::OK, &to_json(&close)),
        at,
    ))
}

/// What `POST …/quarantined` takes: the source, the call's options and
/// the host's refusal.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostQuarantine {
    source: SourceId,
    #[serde(default)]
    opts: lakelands_api::IngestOpts,
    refused: lakelands_api::quarantine::HostRefused,
}

/// `POST …/quarantined`, its body a [`HostQuarantine`]: quarantine a
/// record the host refused (085, extended).
async fn quarantine(
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    cx: WriteContext,
    req: Request<Incoming>,
    most: Limit,
) -> Result<Answer, Stop> {
    let q: HostQuarantine = object_body(req, "POST …/quarantined", most).await?;
    let done = store
        .catalog(cx)
        .quarantine(who, &q.source, q.opts, q.refused)
        .await
        .map_err(contract)?;
    let at = Some(done.position);
    Ok(with_position(
        json_reply(StatusCode::OK, &to_json(&done)),
        at,
    ))
}

// ------------------------------------------------------------------- feed

async fn feed(
    state: &State,
    store: &Arc<dyn LakeStore>,
    who: &Principal,
    req: Request<Incoming>,
) -> Result<Answer, Stop> {
    let a = query_args(req.uri().query(), &FEED, "GET …/feed")?;
    if !who.grants.reads_everything() {
        return Err(contract(Error::Refused {
            field: None,
            reason: Refusal::NotGranted,
        }));
    }
    // A reconnecting client resumes after the last event it received.
    let resume = header(req.headers(), "last-event-id")
        .and_then(|id| id.trim().parse::<u64>().ok())
        .map(|p| p + 1);
    let from = Position(
        resume
            .or(a.get("from").and_then(Value::as_u64))
            .unwrap_or(0),
    );
    let words = |k: &str| -> Vec<String> {
        a.get(k)
            .and_then(Value::as_array)
            .map(|w| {
                w.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let filter = FeedFilter {
        kinds: words("kinds"),
        types: words("types").into_iter().map(Into::into).collect(),
        keys: words("key").into_iter().map(Into::into).collect(),
    };
    let head = store.head().await.map_err(contract)?;
    if from.0 > head.0 + 1 {
        return Err(contract(Error::Refused {
            field: None,
            reason: Refusal::OutOfRange { head },
        }));
    }
    let (tx, rx) = mpsc::channel(16);
    let pace = (state.feed_poll, state.feed_floor);
    tokio::spawn(tail(store.clone(), from, filter, pace, tx));
    let mut r = with_position(streamed("text/event-stream", rx), Some(head));
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(r)
}

/// An event as one SSE frame, with the comment `: overruled` where its
/// batch names it overruled (065) and `: append <position>`, the last
/// position of its append (083).
fn frame(e: &lakelands_api::event::Event, overruled: bool, append: Option<Position>) -> Bytes {
    let data = serde_json::to_string(&to_json(e)).expect("JSON prints");
    let (at, kind) = (e.position.0, e.body.kind());
    let mark = if overruled { ": overruled\n" } else { "" };
    let end = append.map_or_else(String::new, |p| format!(": append {}\n", p.0));
    format!("id: {at}\n{mark}{end}event: {kind}\ndata: {data}\n\n").into()
}

/// Send the feed from `from` on, then wait for more, until the client
/// goes away. It reads the lake's hub for its filter's kinds and types,
/// applying its keys itself (lakelands#1113).
async fn tail(
    store: Arc<dyn LakeStore>,
    mut at: Position,
    filter: FeedFilter,
    (pause, floor): (Duration, Duration),
    tx: mpsc::Sender<Bytes>,
) {
    let failure = async |e: Error| {
        let f = Failure::from(e.clone());
        let data = serde_json::to_string(&failure_json(&e, &f)).expect("JSON prints");
        let _ = tx
            .send(format!("event: error\ndata: {data}\n\n").into())
            .await;
    };
    let mut follow = match crate::hub::Follow::new(&store, filter, (pause, floor)).await {
        Ok(f) => f,
        Err(e) => return failure(e).await,
    };
    let mut last_sent = Instant::now();
    // The `next` last told, so a client that stops when caught up hears it
    // once per position, not once per pause.
    let mut told: Option<Position> = None;
    loop {
        match follow.next(at, pause).await {
            Err(e) => return failure(e).await,
            Ok(crate::hub::Next::Quiet) => {}
            Ok(crate::hub::Next::Batch(b)) => {
                // Of the batch, the events at `at` and after that its keys
                // admit, as the frames the batch made once.
                let events = b.batch.events.iter().zip(b.frames(frame));
                let new = |(e, _): &(&lakelands_api::event::Event, &Bytes)| {
                    e.position >= at && follow.admits(e)
                };
                for (_, f) in events.filter(new) {
                    if tx.send(f.clone()).await.is_err() {
                        return;
                    }
                    last_sent = Instant::now();
                }
                let caught_up = crate::hub::caught_up(&b.batch, at);
                at = at.max(b.batch.next);
                // Caught up: the first position not yet examined, the `next`
                // a poll would answer, told when it moved since last told.
                if caught_up && told != Some(at) {
                    let next = format!(": next {}\n\n", at.0);
                    if tx.send(next.into()).await.is_err() {
                        return;
                    }
                    told = Some(at);
                    last_sent = Instant::now();
                }
            }
        }
        if last_sent.elapsed() >= KEEP_ALIVE {
            if tx
                .send(Bytes::from_static(b": keep-alive\n\n"))
                .await
                .is_err()
            {
                return;
            }
            last_sent = Instant::now();
        }
        if tx.is_closed() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reaction past a trigger's `max_depth` is refused by the store,
    /// never by a door caller, whose writes carry no cause; when REST
    /// answers it, it is 422, `too_deep`, naming the trigger.
    #[test]
    fn a_too_deep_refusal_is_422_naming_the_trigger() {
        let e = Error::Refused {
            field: Some("once".into()),
            reason: Refusal::TooDeep { max: 1 },
        };
        assert_eq!(status_of(&e), StatusCode::UNPROCESSABLE_ENTITY);
        let body = failure_json(&e, &Failure::from(e.clone()));
        assert_eq!(body["error"], "refused", "{body}");
        assert_eq!(body["field"], "once", "{body}");
        assert_eq!(body["reason"]["refusal"], "too_deep", "{body}");
        assert_eq!(body["reason"]["max"], 1, "{body}");
        let what = body["what"].as_str().unwrap_or_default();
        assert!(what.contains("`once`"), "{what}");
    }

    #[test]
    fn the_short_order_form_reads_fields_provenance_and_latest() {
        assert_eq!(
            order("-due_date,known_at(status),-latest()"),
            Some(json!([
                {"field": "due_date", "dir": "desc"},
                {"by": "known_at", "field": "status", "dir": "asc"},
                {"by": "latest", "dir": "desc"},
            ]))
        );
        assert_eq!(
            order("latest"),
            Some(json!([{"field": "latest", "dir": "asc"}])),
            "a field named `latest` keeps its plain spelling"
        );
    }
}
