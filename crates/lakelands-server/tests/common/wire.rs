//! The two transports as a door test drives them: REST over a real TCP
//! connection, and an MCP session over stdio.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::{LakeStore, Principal};
use lakelands_server::resolver::StaticResolver;
use lakelands_server::{HttpConfig, serve_http, serve_io};
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};
use tokio::net::TcpStream;

/// A wait for an answer that should come: past it, a hang is a failure.
const WAIT: Duration = Duration::from_secs(60);

/// Serve `store` over HTTP to the principals `resolver` knows.
pub async fn serve(store: Arc<dyn LakeStore>, resolver: Arc<StaticResolver>) -> SocketAddr {
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 20,
        feed_floor_millis: 25,
        workers: None,
        metrics: None,
        hosts: Vec::new(),
        bounds: Default::default(),
    };
    let server = serve_http(config, resolver, open).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    addr
}

/// Send `method` to the lake's `route` with the API key `key` and, when
/// given, a JSON body: the status and the body answered.
pub async fn rest(
    addr: SocketAddr,
    key: &str,
    method: &str,
    route: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let typed = if body.is_empty() {
        String::new()
    } else {
        format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
    };
    let request = format!(
        "{method} /lakes/finance/{route} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer {key}\r\n{typed}\r\n{body}"
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("an answer within the wait")
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

/// An MCP session over stdio, as `who`.
pub struct Mcp {
    to: WriteHalf<DuplexStream>,
    from: BufReader<ReadHalf<DuplexStream>>,
    id: u64,
}

impl Mcp {
    pub async fn open(store: Arc<dyn LakeStore>, who: Principal) -> Mcp {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server);
        tokio::spawn(serve_io(store, who, BufReader::new(sr), sw));
        let (cr, cw) = tokio::io::split(client);
        let mut s = Mcp {
            to: cw,
            from: BufReader::new(cr),
            id: 0,
        };
        s.request("initialize", json!({"protocolVersion": "2025-06-18"}))
            .await;
        s
    }

    /// Send a request; answer its response, skipping notifications.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let mut bytes = serde_json::to_vec(&msg).unwrap();
        bytes.push(b'\n');
        self.to.write_all(&bytes).await.unwrap();
        loop {
            let mut l = String::new();
            tokio::time::timeout(WAIT, self.from.read_line(&mut l))
                .await
                .expect("an answer within the wait")
                .unwrap();
            let m: Value = serde_json::from_str(&l).unwrap();
            if m["id"] == json!(self.id) {
                return m;
            }
        }
    }

    /// Call a tool: its `result`, `isError` and `content` included.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await["result"]
            .clone()
    }
}
