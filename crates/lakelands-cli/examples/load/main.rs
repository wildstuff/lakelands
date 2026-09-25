//! A load run against a served lake, over every door it has: see
//! `driver.rs` for what it drives, checks and measures.
//!
//! ```text
//! cargo build --release -p lakelands-cli
//! cargo run --release -p lakelands-cli --example load -- \
//!     --lakelands target/release/lakelands [--full | --short] \
//!     [--memory | --pg-url <url> [--pg-plain]] [--scratch <dir>] [--json <file>]
//! ```
//!
//! The scratch root goes under the workspace's `target/load` unless
//! `--scratch` names another directory.
//!
//! Every knob is an argument; nothing is read from the environment. It
//! prints one line per phase, then the findings: a defect is something a
//! page promises that did not hold, a limit a bound a page should state.
//! It exits 1 when it found a defect.

mod driver;

use std::path::PathBuf;
use std::process::ExitCode;

use driver::{Engine, Plan, Scale};

const USAGE: &str = "usage: load --lakelands <binary> [--short | --full] \
[--memory | --pg-url <url> [--pg-plain]] [--scratch <dir>] [--json <file>]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut lakelands = None;
    let mut scale = Scale::standard();
    let mut engine = Engine::Sqlite;
    let mut plain = false;
    // Under the workspace's `target/`, which git ignores: a kept root holds
    // the lake and, with `--pg-url`, the URL.
    let mut scratch = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/load"));
    let mut json = None;
    while let Some(a) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--lakelands" => lakelands = Some(PathBuf::from(value("--lakelands"))),
            "--short" => scale = Scale::short(),
            "--full" => scale = Scale::full(),
            "--memory" => engine = Engine::Memory,
            "--pg-url" => {
                engine = Engine::Postgres {
                    url: value("--pg-url"),
                    plain: false,
                }
            }
            "--pg-plain" => plain = true,
            "--scratch" => scratch = PathBuf::from(value("--scratch")),
            "--json" => json = Some(PathBuf::from(value("--json"))),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if let Engine::Postgres { plain: p, .. } = &mut engine {
        *p = plain;
    }
    let Some(lakelands) = lakelands else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let plan = Plan {
        lakelands,
        scratch,
        scale,
        engine,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let report = runtime.block_on(driver::run(plan));
    print!("{}", report.text());
    if let Some(path) = json
        && let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(&report.json()).unwrap())
    {
        eprintln!("{}: {e}", path.display());
    }
    if report.defects().is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
