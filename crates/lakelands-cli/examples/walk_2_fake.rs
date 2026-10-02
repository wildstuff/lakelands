//! The loopback fakes walk 2 runs against (`docs/walks/walk-2.md`).
//!
//! ```text
//! walk_2_fake embed <addr>          a /v1/embeddings endpoint
//! walk_2_fake hook <addr> <file>    a webhook receiver, one line per delivery
//! walk_2_fake band                  the exec program behind the `band` function
//! ```
//!
//! Every answer can be worked out by hand. The embedding of a text counts
//! the words of four groups in it, one dimension each; the band of an
//! invoice is read off its gross. The servers speak just enough HTTP/1.1
//! for the lake's clients: one request per connection, then close.
//! `crates/lakelands-cli/tests/walk_2.rs` includes this file to run the
//! embedding endpoint inside the test.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{Value, json};

/// The dimensions of the fake embedding, in order: a text's vector counts
/// the words of each group it contains.
const GROUPS: [&[&str]; 4] = [
    &["freight", "shipping", "haulage"],
    &["software", "licence"],
    &["food", "catering"],
    &["repair", "service"],
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["embed", addr] => serve(listen(addr), |_, body| embed(body)),
        ["hook", addr, file] => {
            let file = file.to_owned();
            serve(listen(addr), move |headers, body| {
                hook(&file, headers, body)
            })
        }
        ["band"] => band(),
        _ => {
            eprintln!("usage: walk_2_fake embed <addr> | hook <addr> <file> | band");
            std::process::exit(2);
        }
    }
}

/// Listen on `addr`, or end the process saying why not.
pub fn listen(addr: &str) -> TcpListener {
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| {
        eprintln!("walk_2_fake: cannot listen on {addr}: {e}");
        std::process::exit(1);
    });
    eprintln!("walk_2_fake: listening on {addr}");
    listener
}

/// Answer every request on `listener` with `answer(headers, body)`, a
/// JSON value sent back with 200.
pub fn serve(listener: TcpListener, answer: impl Fn(&[(String, String)], &[u8]) -> Value) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if let Err(e) = one(stream, &answer) {
            eprintln!("walk_2_fake: {e}");
        }
    }
}

fn one(
    stream: TcpStream,
    answer: &impl Fn(&[(String, String)], &[u8]) -> Value,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    // A connection that ends before its request line carries nothing.
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let reply = answer(&headers, &body).to_string();
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{reply}",
        reply.len()
    )
}

/// `{"model", "input": [texts]}` → `{"data": [{"index", "embedding"}]}`.
pub fn embed(body: &[u8]) -> Value {
    let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let texts = request["input"].as_array().cloned().unwrap_or_default();
    let data: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| json!({ "index": index, "embedding": vector(text.as_str().unwrap_or("")) }))
        .collect();
    json!({ "data": data })
}

fn vector(text: &str) -> Vec<f32> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .collect();
    GROUPS
        .iter()
        .map(|group| words.iter().filter(|w| group.contains(&w.as_str())).count() as f32)
        .collect()
}

/// Append one line per delivery to `file`, and print it: the lake and
/// position from the headers, and the event's body.
fn hook(file: &str, headers: &[(String, String)], body: &[u8]) -> Value {
    let event: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let line = json!({
        "lake": header("lakelands-lake"),
        "position": header("lakelands-position").parse::<u64>().ok(),
        "event": event["body"],
    })
    .to_string();
    println!("{line}");
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .expect("the hook file opens");
    writeln!(out, "{line}").expect("the hook file takes the line");
    json!({ "received": true })
}

/// The call `{"function", "inputs": {"gross"}, "config"}` on standard
/// input; the band on standard output: `low` under 2000, `mid` under
/// 5000, `high` from 5000.
fn band() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("the call arrives on standard input");
    let call: Value = serde_json::from_str(&input).expect("the call is JSON");
    let gross = match &call["inputs"]["gross"] {
        Value::String(s) => s.parse::<f64>().ok(),
        v => v.as_f64(),
    };
    let band = match gross {
        Some(g) if g < 2000.0 => "low",
        Some(g) if g < 5000.0 => "mid",
        Some(_) => "high",
        None => {
            eprintln!("walk_2_fake band: no gross in {input}");
            std::process::exit(1);
        }
    };
    println!("{}", json!(band));
}
