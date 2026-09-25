//! Notifications over the change feed, on stdio and on streamable HTTP,
//! on the mock and on the store over both engines: what a subscribed
//! session is told, what it is never told, and that a slow client makes
//! a session lag without losing anything.

mod common;

use common::{BACKENDS, Backend, flaky};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{LakeStore, Principal, WriteContext};
use lakelands_server::{HttpConfig, serve_http, serve_io};
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};
use tokio::net::TcpStream;

/// How long a test waits for a notification that should come: a bound
/// that turns a lost notification into a failure instead of a hang. It is
/// not what orders anything. Measured on a machine loaded well past its
/// cores, while other tests ran, no notification came later than 5 s
/// after the one before it (296 of them arrive in under 250 ms unloaded),
/// so 20 s is a margin of four.
const WAIT: Duration = Duration::from_secs(20);

/// Reads everything but `secret`, sees `note` masked, and may not read
/// `partner` at all.
fn watcher() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "watcher".into(),
        lake: "finance".into(),
        grants: Grants {
            default: Access::Read,
            types: [
                (
                    "invoice".into(),
                    TypeGrants {
                        default: None,
                        fields: [
                            ("secret".into(), Access::None),
                            ("note".into(), Access::Masked),
                        ]
                        .into(),
                    },
                ),
                (
                    "partner".into(),
                    TypeGrants {
                        default: Some(Access::None),
                        fields: Default::default(),
                    },
                ),
            ]
            .into(),
            ..Grants::default()
        },
    }
}

/// An MCP session over stdio.
struct Stdio {
    to: WriteHalf<DuplexStream>,
    from: BufReader<ReadHalf<DuplexStream>>,
    id: u64,
    /// Notifications read while waiting for a response.
    seen: Vec<Value>,
}

impl Stdio {
    async fn open(store: Arc<dyn LakeStore>, who: Principal) -> Stdio {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server);
        tokio::spawn(serve_io(store, who, BufReader::new(sr), sw));
        let (cr, cw) = tokio::io::split(client);
        let mut s = Stdio {
            to: cw,
            from: BufReader::new(cr),
            id: 0,
            seen: Vec::new(),
        };
        s.request("initialize", json!({"protocolVersion": "2025-06-18"}))
            .await;
        s
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let mut bytes = serde_json::to_vec(&msg).unwrap();
        bytes.push(b'\n');
        self.to.write_all(&bytes).await.unwrap();
        loop {
            let m = self.line().await.expect("the server answers");
            if m["id"] == json!(self.id) {
                return m;
            }
            self.seen.push(m);
        }
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let r = self
            .request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await;
        r["result"]["structuredContent"].clone()
    }

    async fn line(&mut self) -> Option<Value> {
        let mut l = String::new();
        let n = tokio::time::timeout(WAIT, self.from.read_line(&mut l))
            .await
            .ok()?
            .unwrap();
        (n > 0).then(|| serde_json::from_str(&l).unwrap())
    }

    /// The next change notification, skipping `tools/list_changed`.
    async fn change(&mut self) -> Value {
        self.next(|m| m["method"] == "notifications/lakelands/changed")
            .await
    }

    async fn next(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let m = if self.seen.is_empty() {
                self.line().await.expect("a notification comes")
            } else {
                self.seen.remove(0)
            };
            if wanted(&m) {
                return m;
            }
        }
    }
}

fn changed(params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/lakelands/changed", "params": params})
}

async fn write(store: &dyn LakeStore, ty: &str, key: &str, fields: Value) -> u64 {
    let patch = serde_json::from_value(fields).unwrap();
    store
        .catalog(WriteContext::default())
        .upsert(
            &common::owner(),
            &ty.into(),
            &key.into(),
            patch,
            WriteOpts::default(),
        )
        .await
        .unwrap()
        .position()
        .0
}

async fn declare_memo(store: &dyn LakeStore) {
    let memo = serde_json::from_value(json!({
        "name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]
    }))
    .unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&common::owner(), Declaration::Type(memo))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_subscribed_session_is_told_what_it_may_see_and_nothing_else() {
    for backend in BACKENDS {
        told_what_it_may_see(backend).await;
    }
}

async fn told_what_it_may_see(backend: Backend) {
    let store = common::lake_on(backend).await;
    let mut s = Stdio::open(store.clone(), watcher()).await;
    let answer = s.call("subscribe", json!({})).await;
    assert_eq!(answer["subscribed"], true, "{backend:?}: {answer}");

    // A write: the changed fields it is shown, masked ones by name, the
    // hidden one not at all; never a value.
    let p = write(
        store.as_ref(),
        "invoice",
        "INV-1",
        json!({"status": "paid", "secret": "s3", "note": "call back"}),
    )
    .await;
    let got = s.change().await;
    assert_eq!(
        got,
        changed(
            json!({"lake": "finance", "position": p, "kind": "record_upserted",
                       "type": "invoice", "key": "INV-1", "fields": ["note", "status"]})
        ),
        "{backend:?}"
    );

    // A write to a type it may not read, and one of only a hidden field,
    // are never announced: the next change it is told of is the one after.
    write(store.as_ref(), "partner", "P-1", json!({"name": "Acme"})).await;
    write(store.as_ref(), "invoice", "INV-1", json!({"secret": "s4"})).await;
    let p = write(
        store.as_ref(),
        "invoice",
        "INV-2",
        json!({"status": "open"}),
    )
    .await;
    let got = s.change().await;
    assert_eq!(got["params"]["position"], json!(p), "{backend:?}: {got}");
    assert_eq!(got["params"]["key"], "INV-2", "{backend:?}");

    // A type declared after `subscribe` is told of, since the principal
    // may read it: the default types are the readable ones at each change.
    // The declaration itself tells every session its tools changed.
    declare_memo(store.as_ref()).await;
    s.next(|m| m["method"] == "notifications/tools/list_changed")
        .await;
    let p = write(store.as_ref(), "memo", "M-1", json!({"text": "hi"})).await;
    assert_eq!(
        s.change().await,
        changed(
            json!({"lake": "finance", "position": p, "kind": "record_upserted",
                       "type": "memo", "key": "M-1", "fields": ["text"]})
        ),
        "{backend:?}"
    );

    // A retirement is told of, read as the record was just before.
    let p = store
        .catalog(WriteContext::default())
        .retire(
            &common::owner(),
            &"invoice".into(),
            &"INV-2".into(),
            WriteOpts::default(),
        )
        .await
        .unwrap()
        .position()
        .0;
    assert_eq!(
        s.change().await,
        changed(
            json!({"lake": "finance", "position": p, "kind": "record_retired",
                       "type": "invoice", "key": "INV-2"})
        ),
        "{backend:?}"
    );

    // Unsubscribed, it is told of changes no more; its tools still.
    let answer = s.call("unsubscribe", json!({})).await;
    assert_eq!(answer["subscribed"], false, "{backend:?}");
    write(
        store.as_ref(),
        "invoice",
        "INV-1",
        json!({"status": "void"}),
    )
    .await;
    let other = serde_json::from_value(json!({
        "name": "memo2", "fields": [{"name": "text", "type": {"kind": "text"}}]
    }))
    .unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&common::owner(), Declaration::Type(other))
        .await
        .unwrap();
    let next = s.next(|_| true).await;
    assert_eq!(
        next["method"], "notifications/tools/list_changed",
        "{backend:?}: after unsubscribing, the next message is the tools' change, not a record's: {next}"
    );
}

#[tokio::test]
async fn an_explicit_list_of_types_is_fixed_when_subscribing() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let mut s = Stdio::open(store.clone(), common::owner()).await;
        s.call(
            "subscribe",
            json!({"types": ["invoice"], "kinds": ["record_upserted"]}),
        )
        .await;
        declare_memo(store.as_ref()).await;
        write(store.as_ref(), "memo", "M-1", json!({"text": "hi"})).await;
        let p = write(
            store.as_ref(),
            "invoice",
            "INV-9",
            json!({"status": "open"}),
        )
        .await;
        let got = s.change().await;
        assert_eq!(got["params"]["position"], json!(p), "{backend:?}: {got}");
    }
}

#[tokio::test]
async fn subscribe_refuses_a_kind_it_does_not_tell_of_and_a_position_past_the_head() {
    let store = common::lake_on(Backend::Mock).await;
    let mut s = Stdio::open(store.clone(), common::owner()).await;
    let r = s
        .request(
            "tools/call",
            json!({"name": "subscribe", "arguments": {"kinds": ["lease_taken"]}}),
        )
        .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    let head = store.head().await.unwrap().0;
    let r = s
        .request(
            "tools/call",
            json!({"name": "subscribe", "arguments": {"from": head + 5}}),
        )
        .await;
    let refusal = &r["result"]["content"][1]["text"];
    let refusal: Value = serde_json::from_str(refusal.as_str().unwrap()).unwrap();
    assert_eq!(
        (&refusal["field"], &refusal["reason"]["refusal"]),
        (&json!("from"), &json!("out_of_range")),
        "{r}"
    );
}

// ---------------------------------------------------------------- HTTP

async fn serve(store: Arc<dyn LakeStore>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 50,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    (addr, task)
}

/// One whole HTTP exchange: the status, the session header, the body.
async fn post(addr: SocketAddr, session: Option<&str>, msg: Value) -> (u16, Option<String>, Value) {
    let body = serde_json::to_string(&msg).unwrap();
    let sid = session.map_or(String::new(), |s| format!("mcp-session-id: {s}\r\n"));
    let head = format!(
        "POST /lakes/finance/mcp HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\naccept: application/json, text/event-stream\r\n{sid}\
         content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let session = head.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("mcp-session-id: ")
            .map(str::to_owned)
    });
    (
        status,
        session,
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}

/// A session started over HTTP as the owner, subscribed from now.
async fn http_session(addr: SocketAddr, subscribe: bool) -> String {
    let (_, sid, _) = post(
        addr,
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18"}}),
    )
    .await;
    let sid = sid.expect("initialize names a session");
    if subscribe {
        let (status, _, r) = post(
            addr,
            Some(&sid),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "subscribe", "arguments": {}}}),
        )
        .await;
        assert_eq!(status, 200, "{r}");
    }
    sid
}

/// One open event stream.
struct Stream {
    reader: BufReader<TcpStream>,
    pending: String,
}

impl Stream {
    async fn open(addr: SocketAddr, sid: &str, last: Option<u64>) -> (u16, Option<Stream>) {
        let last = last.map_or(String::new(), |p| format!("last-event-id: {p}\r\n"));
        let head = format!(
            "GET /lakes/finance/mcp HTTP/1.1\r\nhost: {addr}\r\nauthorization: Bearer k-owner\r\n\
             accept: text/event-stream\r\nmcp-session-id: {sid}\r\n{last}\r\n"
        );
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).await.unwrap();
        let status: u16 = status.split(' ').nth(1).unwrap().parse().unwrap();
        loop {
            let mut l = String::new();
            reader.read_line(&mut l).await.unwrap();
            if l == "\r\n" {
                break;
            }
        }
        let open = (status == 200).then(|| Stream {
            reader,
            pending: String::new(),
        });
        (status, open)
    }

    /// The next event's id and message, skipping comments; `None` when
    /// the stream ends or nothing comes in time.
    async fn next(&mut self) -> Option<(Option<u64>, Value)> {
        loop {
            if let Some(i) = self.pending.find("\n\n") {
                let event: String = self.pending.drain(..i + 2).collect();
                let mut id = None;
                let mut data = None;
                for l in event.lines() {
                    if let Some(v) = l.strip_prefix("id: ") {
                        id = v.parse().ok();
                    } else if let Some(v) = l.strip_prefix("data: ") {
                        data = serde_json::from_str(v).ok();
                    }
                }
                if let Some(d) = data {
                    return Some((id, d));
                }
                continue;
            }
            let mut size = String::new();
            let n = tokio::time::timeout(WAIT, self.reader.read_line(&mut size))
                .await
                .ok()?
                .ok()?;
            if n == 0 {
                return None;
            }
            let n = usize::from_str_radix(size.trim(), 16).ok()?;
            if n == 0 {
                return None;
            }
            let mut chunk = vec![0; n + 2];
            self.reader.read_exact(&mut chunk).await.ok()?;
            self.pending
                .push_str(std::str::from_utf8(&chunk[..n]).unwrap());
        }
    }

    /// The next change, skipping `tools/list_changed`.
    /// The next change, skipping `tools/list_changed` and keep-alives.
    /// One deadline covers the whole wait: a stream that only keeps
    /// itself alive does not hold a lost change open forever.
    async fn change(&mut self) -> (u64, Value) {
        tokio::time::timeout(WAIT, self.change_unbounded())
            .await
            .expect("a change comes within the bound")
    }

    async fn change_unbounded(&mut self) -> (u64, Value) {
        loop {
            let (id, m) = self.next().await.expect("the stream is open");
            if m["method"] == "notifications/lakelands/changed" {
                return (id.expect("a change's event has its position as id"), m);
            }
        }
    }
}

#[tokio::test]
async fn the_http_stream_tells_of_changes_resumes_and_is_replaced() {
    for backend in BACKENDS {
        http_stream(backend).await;
    }
}

/// No step waits on a sleep for ordering: each wait is for a message that
/// must come, bounded by [`WAIT`] only so a failure ends instead of
/// hanging, and the pump's pause (`feed_poll_millis`, 50 ms here) only
/// sets how soon it comes. The resume does not race the replaced
/// stream's queue: changes queued before it are dropped by their
/// subscription tag, not drained, so this holds single-threaded too.
async fn http_stream(backend: Backend) {
    let store = common::lake_on(backend).await;
    let (addr, task) = serve(store.clone()).await;
    let sid = http_session(addr, true).await;

    let (status, stream) = Stream::open(addr, &sid, None).await;
    assert_eq!(status, 200, "{backend:?}");
    let mut stream = stream.unwrap();
    let p = write(
        store.as_ref(),
        "invoice",
        "INV-1",
        json!({"status": "paid"}),
    )
    .await;
    let (id, m) = stream.change().await;
    assert_eq!(id, p, "{backend:?}");
    assert_eq!(m["params"]["fields"], json!(["status"]), "{backend:?}");

    // A second stream replaces the first, which ends.
    let (_, second) = Stream::open(addr, &sid, None).await;
    let mut second = second.unwrap();
    assert!(
        stream.next().await.is_none(),
        "{backend:?}: the first stream ended"
    );
    let p2 = write(
        store.as_ref(),
        "invoice",
        "INV-2",
        json!({"status": "open"}),
    )
    .await;
    assert_eq!(second.change().await.0, p2, "{backend:?}");
    drop(second);

    // Reconnecting after `p` tells again of what came after it.
    let (_, resumed) = Stream::open(addr, &sid, Some(p)).await;
    let mut resumed = resumed.unwrap();
    assert_eq!(
        resumed.change().await.0,
        p2,
        "{backend:?}: resumed after {p}"
    );
    task.abort();
}

#[tokio::test]
async fn a_client_that_does_not_read_makes_its_session_lag_and_loses_nothing() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let (addr, task) = serve(store.clone()).await;
        let sid = http_session(addr, true).await;
        // More changes than the queue holds, with no stream open.
        let mut written = Vec::new();
        for i in 0..(lakelands_server::watch::QUEUE + 40) {
            written.push(
                write(
                    store.as_ref(),
                    "invoice",
                    &format!("B-{i}"),
                    json!({"status": "open"}),
                )
                .await,
            );
        }
        let (_, stream) = Stream::open(addr, &sid, None).await;
        let mut stream = stream.unwrap();
        let mut seen = Vec::new();
        while seen.len() < written.len() {
            seen.push(stream.change().await.0);
        }
        assert_eq!(seen, written, "{backend:?}: every change, in order");
        task.abort();
    }
}

#[tokio::test]
async fn a_stream_opens_only_for_the_sessions_own_key() {
    let store = common::lake_on(Backend::Mock).await;
    let (addr, task) = serve(store).await;
    let sid = http_session(addr, false).await;
    // The owner's session named under the clerk's key.
    let head = format!(
        "GET /lakes/finance/mcp HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-clerk\r\naccept: text/event-stream\r\nmcp-session-id: {sid}\r\n\r\n"
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 404"), "{raw}");
    task.abort();
}

#[tokio::test]
async fn the_sdk_client_is_told_of_a_change() {
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, CustomNotification};
    use rmcp::service::{NotificationContext, RoleClient};

    #[derive(Clone)]
    struct Hears(tokio::sync::mpsc::UnboundedSender<Value>);

    impl rmcp::ClientHandler for Hears {
        fn on_custom_notification(
            &self,
            n: CustomNotification,
            _: NotificationContext<RoleClient>,
        ) -> impl std::future::Future<Output = ()> + Send + '_ {
            let _ = self.0.send(json!({"method": n.method, "params": n.params}));
            std::future::ready(())
        }
    }

    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server_io);
        tokio::spawn(serve_io(
            store.clone(),
            common::owner(),
            BufReader::new(sr),
            sw,
        ));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let client = Hears(tx).serve(tokio::io::split(client_io)).await.unwrap();
        let params: CallToolRequestParams =
            serde_json::from_value(json!({"name": "subscribe", "arguments": {}})).unwrap();
        client.call_tool(params).await.unwrap();
        let p = write(
            store.as_ref(),
            "invoice",
            "INV-1",
            json!({"status": "paid"}),
        )
        .await;
        let got = tokio::time::timeout(WAIT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            got["method"], "notifications/lakelands/changed",
            "{backend:?}"
        );
        assert_eq!(got["params"]["position"], json!(p), "{backend:?}");
        client.cancel().await.unwrap();
    }
}

async fn declare(store: &dyn LakeStore, d: Value) {
    let d: Declaration = serde_json::from_value(d).unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&common::owner(), d)
        .await
        .unwrap();
}

/// A trigger or an object set says a condition held, and a condition may
/// read any field of its type: a principal who does not read every field
/// in full is never told of one (024). The owner, who does, is. The mock
/// fires neither triggers nor set changes, so this runs on the store.
#[tokio::test]
async fn triggers_and_set_changes_are_told_only_to_a_principal_who_reads_the_whole_type() {
    for backend in [Backend::Memory, Backend::Sqlite] {
        let store = common::lake_on(backend).await;
        declare(
            store.as_ref(),
            json!({"declare": "trigger", "name": "big_secret", "on": "invoice",
                   "when": "secret = 's3'"}),
        )
        .await;
        declare(
            store.as_ref(),
            json!({"declare": "object_set", "name": "secretive", "of": "invoice",
                   "where": "secret = 's3'"}),
        )
        .await;
        let mut owner = Stdio::open(store.clone(), common::owner()).await;
        let mut watcher = Stdio::open(store.clone(), watcher()).await;
        for s in [&mut owner, &mut watcher] {
            s.call(
                "subscribe",
                json!({"kinds": ["trigger_fired", "object_set_changed", "record_upserted"]}),
            )
            .await;
        }
        let p = write(
            store.as_ref(),
            "invoice",
            "INV-1",
            json!({"status": "paid", "secret": "s3"}),
        )
        .await;
        // The owner is told of the write, the firing and the set change.
        let mut told = Vec::new();
        while told.len() < 3 {
            let m = owner.change().await;
            told.push(m["params"]["kind"].as_str().unwrap().to_owned());
        }
        told.sort();
        assert_eq!(
            told,
            ["object_set_changed", "record_upserted", "trigger_fired"],
            "{backend:?}"
        );
        // The watcher, who may not read `secret`, is told of the write's
        // `status` only; the next thing it is told is a later write.
        let m = watcher.change().await;
        assert_eq!(
            (&m["params"]["kind"], &m["params"]["position"]),
            (&json!("record_upserted"), &json!(p)),
            "{backend:?}: {m}"
        );
        let later = write(
            store.as_ref(),
            "invoice",
            "INV-2",
            json!({"status": "open"}),
        )
        .await;
        let m = watcher.change().await;
        assert_eq!(m["params"]["position"], json!(later), "{backend:?}: {m}");
    }
}

/// An output a function backing records fills its field: a change of that
/// field, told with its name. The mock binds no backing, so this runs on
/// the store.
#[tokio::test]
async fn an_output_that_fills_a_field_is_told_as_a_change_of_it() {
    use lakelands_api::seam::{FunctionBacking, FunctionCall, Seams};

    struct Echo;
    #[async_trait::async_trait]
    impl FunctionBacking for Echo {
        fn kind(&self) -> &str {
            "echo"
        }
        async fn call(&self, call: &FunctionCall) -> lakelands_api::Result<Value> {
            Ok(call.inputs["code"].clone())
        }
    }
    let store = lakelands_store::open(
        lakelands_api::Location::memory("finance"),
        Seams {
            engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
            backings: vec![Arc::new(Echo)],
            ..Seams::default()
        },
        lakelands_api::engine::OpenMode::Create,
    )
    .await
    .unwrap();
    declare(
        store.as_ref(),
        json!({"declare": "function", "name": "lookup", "backing": "echo",
               "inputs": [{"name": "code", "type": {"kind": "text"}}]}),
    )
    .await;
    declare(
        store.as_ref(),
        json!({"declare": "type", "name": "place", "fields": [
            {"name": "code", "type": {"kind": "text"}},
            {"name": "name", "type": {"kind": "text"}, "authority": "derived",
             "function": "lookup", "depends_on": ["code"]}]}),
    )
    .await;
    let mut s = Stdio::open(store.clone(), common::owner()).await;
    s.call("subscribe", json!({"kinds": ["output_recorded"]}))
        .await;
    write(store.as_ref(), "place", "p1", json!({"code": "AT"})).await;
    let m = s.change().await;
    assert_eq!(
        (
            &m["params"]["kind"],
            &m["params"]["key"],
            &m["params"]["fields"]
        ),
        (&json!("output_recorded"), &json!("p1"), &json!(["name"])),
        "{m}"
    );
}

/// A session resumed while its queue was full is told every change after
/// the position it names, once and in order.
#[tokio::test]
async fn a_resume_while_the_queue_is_full_tells_each_change_after_it_once() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let (addr, task) = serve(store.clone()).await;
        let sid = http_session(addr, true).await;
        let mut written = Vec::new();
        for i in 0..(lakelands_server::watch::QUEUE + 40) {
            written.push(
                write(
                    store.as_ref(),
                    "invoice",
                    &format!("R-{i}"),
                    json!({"status": "open"}),
                )
                .await,
            );
        }
        // Resume after the tenth: the queued ones before the resume are
        // dropped as stale, and everything after it comes, once.
        let after = written[9];
        let (_, stream) = Stream::open(addr, &sid, Some(after)).await;
        let mut stream = stream.unwrap();
        let want: Vec<u64> = written[10..].to_vec();
        let mut seen = Vec::new();
        while seen.len() < want.len() {
            seen.push(stream.change().await.0);
        }
        assert_eq!(seen, want, "{backend:?}");
        task.abort();
    }
}

/// Ending the session ends its open stream.
#[tokio::test]
async fn deleting_the_session_ends_its_stream() {
    let store = common::lake_on(Backend::Mock).await;
    let (addr, task) = serve(store).await;
    let sid = http_session(addr, true).await;
    let (_, stream) = Stream::open(addr, &sid, None).await;
    let mut stream = stream.unwrap();
    let head = format!(
        "DELETE /lakes/finance/mcp HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\nmcp-session-id: {sid}\r\n\r\n"
    );
    let mut del = TcpStream::connect(addr).await.unwrap();
    del.write_all(head.as_bytes()).await.unwrap();
    let mut raw = String::new();
    del.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 204"), "{raw}");
    assert!(
        stream.next().await.is_none(),
        "the stream ended with its session"
    );
    task.abort();
}

/// A `Last-Event-ID` with no position after it is refused, not wrapped.
#[tokio::test]
async fn a_last_event_id_at_the_end_of_the_positions_is_refused() {
    let store = common::lake_on(Backend::Mock).await;
    let (addr, task) = serve(store).await;
    let sid = http_session(addr, true).await;
    let (status, _) = Stream::open(addr, &sid, Some(u64::MAX)).await;
    assert_eq!(status, 400);
    task.abort();
}

// ------------------------------------------------------------ retry

/// The faults land where they are meant to because the current-thread
/// runtime runs the pump's first `head` before the test's `subscribe`.
///
/// A lake that cannot answer for a moment delays a notification and
/// never drops it: the pump's first `head` fails once, the `get` behind
/// the first change fails once as unavailable and once as out of range
/// (the reads a moment behind the feed), and both changes still come,
/// once each, in order.
#[tokio::test]
async fn a_lake_that_fails_for_a_moment_delays_a_notification_and_loses_none() {
    use std::sync::atomic::Ordering;
    for backend in BACKENDS {
        let inner = common::lake_on(backend).await;
        let faults = Arc::new(flaky::Faults::default());
        faults.head.store(1, Ordering::SeqCst);
        let store: Arc<dyn LakeStore> = Arc::new(flaky::Store {
            inner: inner.clone(),
            faults: faults.clone(),
        });
        let mut s = Stdio::open(store.clone(), common::owner()).await;
        s.call("subscribe", json!({"kinds": ["record_upserted"]}))
            .await;
        faults.get.store(1, Ordering::SeqCst);
        // And once more, as a read view a moment behind the feed answers.
        faults.behind.store(1, Ordering::SeqCst);
        let p1 = write(inner.as_ref(), "invoice", "F-1", json!({"status": "open"})).await;
        let p2 = write(inner.as_ref(), "invoice", "F-2", json!({"status": "open"})).await;
        let first = s.change().await;
        let second = s.change().await;
        assert_eq!(
            (&first["params"]["position"], &second["params"]["position"]),
            (&json!(p1), &json!(p2)),
            "{backend:?}: {first} {second}"
        );
        assert_eq!(
            (
                faults.head.load(Ordering::SeqCst),
                faults.get.load(Ordering::SeqCst),
                faults.behind.load(Ordering::SeqCst)
            ),
            (0, 0, 0),
            "{backend:?}: every fault was met"
        );
        // Nothing is told twice: a later write is the next change.
        let p3 = write(inner.as_ref(), "invoice", "F-3", json!({"status": "open"})).await;
        assert_eq!(
            s.change().await["params"]["position"],
            json!(p3),
            "{backend:?}"
        );
    }
}

/// The two 300 ms pauses let the pump queue before the step that follows;
/// under load it may have queued less, which weakens the test's claim,
/// never makes it fail.
///
/// Subscribing again from an earlier position replaces what was queued
/// for the old subscription: each change comes once. After
/// `unsubscribe`, nothing queued before it comes at all.
#[tokio::test]
async fn resubscribing_repeats_nothing_and_unsubscribing_leaks_nothing() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let (addr, task) = serve(store.clone()).await;
        let sid = http_session(addr, true).await;
        let from = store.head().await.unwrap().next().0;
        let mut written = Vec::new();
        for i in 0..5 {
            written.push(
                write(
                    store.as_ref(),
                    "invoice",
                    &format!("S-{i}"),
                    json!({"status": "open"}),
                )
                .await,
            );
        }
        // Give the pump time to queue them, then subscribe again from
        // the same earlier position.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (status, _, r) = post(
            addr,
            Some(&sid),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "subscribe", "arguments": {"from": from}}}),
        )
        .await;
        assert_eq!(status, 200, "{r}");
        let (_, stream) = Stream::open(addr, &sid, None).await;
        let mut stream = stream.unwrap();
        let mut seen = Vec::new();
        while seen.len() < written.len() {
            seen.push(stream.change().await.0);
        }
        assert_eq!(seen, written, "{backend:?}: each change once");
        drop(stream);

        // Queued changes, then `unsubscribe`, then a stream: the first
        // thing it is told is a later tools change, not a queued change.
        for i in 0..5 {
            write(
                store.as_ref(),
                "invoice",
                &format!("U-{i}"),
                json!({"status": "open"}),
            )
            .await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        post(
            addr,
            Some(&sid),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                   "params": {"name": "unsubscribe", "arguments": {}}}),
        )
        .await;
        declare_memo(store.as_ref()).await;
        let (_, stream) = Stream::open(addr, &sid, None).await;
        let mut stream = stream.unwrap();
        let (_, first) = stream.next().await.expect("the tools change comes");
        assert_eq!(
            first["method"], "notifications/tools/list_changed",
            "{backend:?}: after unsubscribe, no queued change leaks: {first}"
        );
        task.abort();
    }
}

/// A tools change has no position to resume from, so one a closing
/// stream did not take is not lost: the next stream opens with it.
#[tokio::test]
async fn a_tools_change_a_closing_stream_missed_opens_the_next_one() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let (addr, task) = serve(store.clone()).await;
        let sid = http_session(addr, false).await;
        let (_, stream) = Stream::open(addr, &sid, None).await;
        drop(stream);
        declare_memo(store.as_ref()).await;
        let (_, next) = Stream::open(addr, &sid, None).await;
        let mut next = next.unwrap();
        let (id, first) = tokio::time::timeout(WAIT, next.next())
            .await
            .expect("a message comes within the bound")
            .expect("the stream is open");
        assert_eq!(
            (id, &first["method"]),
            (None, &json!("notifications/tools/list_changed")),
            "{backend:?}: {first}"
        );
        task.abort();
    }
}
