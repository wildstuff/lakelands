//! The soak: one lake under a small business's day for hours, its server
//! killed at random and checked after every kill and every hour; see
//! `driver.rs` for what it drives and checks (lakelands#975).
//!
//! ```text
//! cargo build --release -p lakelands-cli
//! cargo run --release -p lakelands-cli --example soak -- \
//!     --lakelands target/release/lakelands --hours 24 \
//!     [--pg-url <url> [--pg-plain]] [--seed <n>] [--day-secs <s>] \
//!     [--kill-every <min>-<max>] [--scratch <dir>] [--json <file>]
//! ```
//!
//! Every knob is an argument; nothing is read from the environment. It
//! prints a line an hour and every red, and exits 1 at the first red,
//! keeping the root (and the schema) for a look.

mod driver;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use driver::{Engine, Plan};

const USAGE: &str = "usage: soak --lakelands <binary> [--hours <h> | --minutes <m>] \
[--pg-url <url> [--pg-plain]] [--seed <n>] [--day-secs <s>] [--kill-every <min>-<max>] \
[--scratch <dir>] [--json <file>]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut plan = Plan::new(PathBuf::new());
    let mut plain = false;
    while let Some(a) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        let number = |name: &str, v: String| -> f64 {
            v.parse().unwrap_or_else(|_| {
                eprintln!("{name} takes a number\n{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--lakelands" => plan.lakelands = PathBuf::from(value("--lakelands")),
            "--hours" => {
                plan.length = Duration::from_secs_f64(number("--hours", value("--hours")) * 3600.0)
            }
            "--minutes" => {
                plan.length =
                    Duration::from_secs_f64(number("--minutes", value("--minutes")) * 60.0)
            }
            "--pg-url" => {
                plan.engine = Engine::Postgres {
                    url: value("--pg-url"),
                    plain: false,
                }
            }
            "--pg-plain" => plain = true,
            "--seed" => plan.seed = number("--seed", value("--seed")) as u64,
            "--day-secs" => {
                plan.day = Duration::from_secs_f64(number("--day-secs", value("--day-secs")))
            }
            "--kill-every" => {
                let v = value("--kill-every");
                let (lo, hi) = v.split_once('-').unwrap_or((&v, &v));
                plan.kill_every = (
                    Duration::from_secs_f64(number("--kill-every", lo.into())),
                    Duration::from_secs_f64(number("--kill-every", hi.into())),
                );
            }
            "--scratch" => plan.scratch = PathBuf::from(value("--scratch")),
            "--json" => plan.json = Some(PathBuf::from(value("--json"))),
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
    if let Engine::Postgres { plain: p, .. } = &mut plan.engine {
        *p = plain;
    }
    if plan.lakelands.as_os_str().is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let report = rt.block_on(driver::run(plan.clone()));
    if let Some(path) = &plan.json {
        let _ = std::fs::write(
            path,
            serde_json::to_vec_pretty(&report.json()).unwrap_or_default(),
        );
    }
    match report.red() {
        None => ExitCode::SUCCESS,
        Some(_) => ExitCode::from(1),
    }
}
