//! The host walk 4 runs as (`docs/walks/walk-4.md`): the part of a
//! procedure that is the host's business, a reaction to a fired trigger.
//!
//! ```text
//! walk_4_host [--root <dir>] --lake <id> --from <position>
//! ```
//!
//! It opens the lake as the CLI does, with one executor bound that the
//! CLI does not have: `ledger`, which creates a payment and answers its
//! id. It reads the feed once, from `--from` to the head, and for every
//! `trigger_fired` of `order_paid` it reacts in two writes:
//!
//! - it calls `create_payment`, a verb on the type `payment`, with no key,
//!   naming the firing as its cause;
//! - it writes the payment the ledger answered, naming the verb's call as
//!   its cause.
//!
//! Each write is made through the catalog with the host's context
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`), which
//! no door carries. A reaction already made answers its first outcome, by
//! its idempotency key, at its first position and with the ledger's
//! recorded answer, so the host can run again. It prints one line per
//! reaction with the positions and the ledger's answer, and exits.
//!
//! Every answer can be worked out by hand: the ledger's payment for the
//! order `O-1` is `PAY-O-1`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::id::{IdempotencyKey, Position};
use lakelands_api::location::{EngineConfig, Location};
use lakelands_api::principal::{Grants, Principal, PrincipalKind, WriteContext, WriteOrigin};
use lakelands_api::read::{Get, ReadAt};
use lakelands_api::record::Patch;
use lakelands_api::seam::{FeedFilter, Seams, VerbExecutor, VerbInvocation};
use lakelands_api::write::{Outcome, WriteOpts};
use serde_json::{Value, json};

/// The trigger the host reacts to.
const TRIGGER: &str = "order_paid";

/// The ledger the host binds for `create_payment`: it creates the payment
/// of the order the payload names, and answers its id.
struct Ledger;

#[async_trait]
impl VerbExecutor for Ledger {
    fn name(&self) -> &str {
        "ledger"
    }

    async fn execute(&self, call: &VerbInvocation) -> lakelands_api::Result<Value> {
        let order = call
            .payload
            .get("order_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        Ok(json!({ "payment": format!("PAY-{order}") }))
    }
}

struct Args {
    root: PathBuf,
    lake: String,
    from: u64,
}

fn args() -> Args {
    let mut root = PathBuf::from(".");
    let (mut lake, mut from) = (None, None);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--root", Some(v)) => root = v.into(),
            ("--lake", Some(v)) => lake = Some(v),
            ("--from", Some(v)) => from = v.parse().ok(),
            _ => usage(),
        }
    }
    match (lake, from) {
        (Some(lake), Some(from)) => Args { root, lake, from },
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: walk_4_host [--root <dir>] --lake <id> --from <position>");
    std::process::exit(2);
}

fn fail(what: impl std::fmt::Display) -> ! {
    eprintln!("walk_4_host: {what}");
    std::process::exit(1);
}

/// The lake's location, as the root keeps it: a relative SQLite
/// directory is the root's.
fn location(a: &Args) -> Location {
    let file = a.root.join("lakes").join(format!("{}.json", a.lake));
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| fail(format!("cannot read {}: {e}", file.display())));
    let mut location: Location = serde_json::from_str(&text)
        .unwrap_or_else(|e| fail(format!("{} is not a location: {e}", file.display())));
    if let EngineConfig::Sqlite { dir, .. } = &mut location.engine
        && dir.is_relative()
    {
        *dir = a.root.join(&*dir);
    }
    location
}

/// The context of a reaction to `cause`, at `depth`.
fn reaction(cause: Position, depth: u32) -> WriteContext {
    WriteContext {
        origin: WriteOrigin::Reactive,
        depth,
        cause: Some(cause),
    }
}

fn once(key: String) -> WriteOpts {
    WriteOpts {
        idempotency_key: Some(IdempotencyKey::from(key.as_str())),
        ..WriteOpts::default()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let a = args();
    let location = location(&a);
    let seams = Seams {
        executors: vec![Arc::new(Ledger)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> = lakelands_cli::open_store(location, OpenMode::Write, seams)
        .await
        .unwrap_or_else(|e| fail(format!("cannot open lake `{}`: {e}", a.lake)));
    let host = Principal {
        kind: PrincipalKind::Service,
        id: "walk_4_host".into(),
        lake: store.lake().clone(),
        grants: Grants::owner(),
    };
    let filter = FeedFilter {
        kinds: vec!["trigger_fired".into()],
        ..FeedFilter::default()
    };
    let mut from = Position(a.from);
    loop {
        let batch = store
            .feed()
            .poll(from, &filter, 100)
            .await
            .unwrap_or_else(|e| fail(format!("cannot read the feed: {e}")));
        for event in &batch.events {
            let EventBody::TriggerFired {
                trigger, ty, key, ..
            } = &event.body
            else {
                continue;
            };
            if trigger.as_str() != TRIGGER {
                continue;
            }
            let fired = event.position;
            let order = store
                .catalog(WriteContext::default())
                .get(
                    &host,
                    ty,
                    Get {
                        key: key.clone(),
                        with_meta: false,
                        params: Default::default(),
                        at: ReadAt::default(),
                    },
                )
                .await
                .unwrap_or_else(|e| fail(format!("cannot read `{key}`: {e}")))
                .data;
            // The verb on the type, with no key; its cause is the firing.
            let payload = Patch::from([("order_id".into(), json!(key.as_str()))]);
            let called = store
                .catalog(reaction(fired, 1))
                .call(
                    &host,
                    &"create_payment".into(),
                    None,
                    payload,
                    once(format!("create_payment:{key}")),
                )
                .await
                .unwrap_or_else(|e| fail(format!("create_payment for `{key}`: {e}")));
            let Outcome::Applied {
                position: call_at,
                output,
                ..
            } = called
            else {
                fail(format!(
                    "create_payment for `{key}` did not run: {called:?}"
                ));
            };
            // The ledger's answer, recorded: a retry answers it too.
            let answered = output
                .as_ref()
                .map_or("no output".to_owned(), Value::to_string);
            let payment = output
                .as_ref()
                .and_then(|o| o["payment"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| fail(format!("create_payment for `{key}` answered no payment")));
            // The payment the ledger answered; its cause is the call.
            let field = |f: &str| order.fields.get(f).cloned().unwrap_or(Value::Null);
            let fields = Patch::from([
                ("order_id".into(), json!(key.as_str())),
                ("customer_no".into(), field("customer_no")),
                ("amount".into(), field("amount")),
                ("paid_on".into(), field("paid_on")),
            ]);
            let written = store
                .catalog(reaction(call_at, 2))
                .upsert(
                    &host,
                    &"payment".into(),
                    &payment.as_str().into(),
                    fields,
                    once(format!("payment:{payment}")),
                )
                .await
                .unwrap_or_else(|e| fail(format!("writing `{payment}`: {e}")));
            let Outcome::Applied {
                position: paid_at, ..
            } = written
            else {
                fail(format!("writing `{payment}` was not applied: {written:?}"));
            };
            println!(
                "order `{key}`: fired at {}, create_payment at {} answered {answered}, payment `{payment}` at {}",
                fired.0, call_at.0, paid_at.0
            );
        }
        if batch.next >= batch.head.next() {
            break;
        }
        from = batch.next;
    }
}
