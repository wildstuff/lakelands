//! The `lakelands` command-line door.
//!
//! `init` and `lake new` lay out a root: a directory with a
//! configuration and one location per lake; `init --example` also makes
//! a lake from an example the binary carries ([`example`]). `apply`,
//! `ingest`, `feed`, `age`, `purge`, `forget`, `values remove`,
//! `rebuild`, `compact`, `export` and `serve` open a lake at its location
//! and call the library in-process: there is no command enum and no layer
//! between the command and the catalog.
//! `import` makes a lake from an export, through `lakelands_store::import`
//! over the same engines.
//! `settings` shows every listed setting with its value for a lake, and
//! writes a lake setting through the catalog.
//!
//! Opening a lake is one function, [`open_store`], and [`run`] takes it
//! as a parameter so that tests can pass another. `open_store` opens a
//! lake through `lakelands_store::open` with the engines this binary
//! carries (SQLite, in-memory and, with the `pg` feature, on by default,
//! PostgreSQL), once per lake for the life of the
//! process: a lake has one writer, and the store that holds its lease is
//! the one every command and every server session of this process uses.
//!
//! Everything comes from the root's files and the flags; no environment
//! variable is read at run time. The `env!` in this crate (the version
//! `--version` prints) and in its tests (Cargo's scratch directory) are
//! constants fixed when the crate is compiled, not reads of the process
//! environment, and are allowed.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use clap::{Arg, ArgAction, ArgMatches, Command};
use lakelands_api::engine::OpenMode;
use lakelands_api::error::{Error, Result};
use lakelands_api::id::{LakeId, Position};
use lakelands_api::location::{EngineConfig, Location};
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, WriteContext};

mod commands;
pub mod example;
pub mod mirror;
mod orders;
mod root;
mod settings;
pub mod ticker;
use lakelands_server::say;

pub use root::{CONFIG, Config, KeysConfig};
pub use say::Failure;

/// A lake being opened.
pub type Opening = Pin<Box<dyn Future<Output = Result<Arc<dyn LakeStore>>> + Send>>;

/// Opens a lake at its location, to write or only to read, with what the
/// root plugs in (its function backings); the opener adds the engines.
pub type Opener<'a> = dyn Fn(Location, OpenMode, Seams) -> Opening + Sync + 'a;

/// The lakes this process has opened, by where they are kept: the
/// location's engine configuration and the lake's name.
type Opened = tokio::sync::Mutex<BTreeMap<String, Arc<dyn LakeStore>>>;

fn opened() -> &'static Opened {
    static OPENED: OnceLock<Opened> = OnceLock::new();
    OPENED.get_or_init(Opened::default)
}

/// This process's in-memory engine: its lakes live as long as the
/// process, and every command of the process finds them.
fn memory() -> lakelands_engine_mem::MemoryOpener {
    static MEMORY: OnceLock<lakelands_engine_mem::MemoryOpener> = OnceLock::new();
    MEMORY
        .get_or_init(lakelands_engine_mem::MemoryOpener::new)
        .clone()
}

/// The engines this binary carries: SQLite, in-memory and, with the `pg`
/// feature, PostgreSQL. Opening and importing a lake both take them from
/// here, so the two cannot differ; the load example opens its own lakes
/// with them too.
#[doc(hidden)]
pub fn engines() -> Vec<Arc<dyn lakelands_api::engine::EngineOpener>> {
    #[allow(unused_mut)]
    let mut engines: Vec<Arc<dyn lakelands_api::engine::EngineOpener>> = vec![
        Arc::new(lakelands_engine_sqlite::SqliteOpener),
        Arc::new(memory()),
    ];
    #[cfg(feature = "pg")]
    engines.push(Arc::new(lakelands_engine_pg::PgOpener));
    engines
}

/// The one place the CLI opens a lake: through the store, with the SQLite
/// and in-memory engines, and the PostgreSQL one with the `pg` feature. To write, it creates the lake at its location
/// the first time and takes the lease (the log records each writer's
/// `lease_taken`); to only read, it takes no lease and appends nothing. A
/// lake is opened once per process, keyed by its location's engine and
/// name, and its store kept for the process's life, so every command and
/// server session shares one store. `host` is what the root plugs in
/// besides the engines.
pub fn open_store(location: Location, mode: OpenMode, host: Seams) -> Opening {
    Box::pin(async move {
        let key = serde_json::to_string(&(&location.lake, &location.engine))
            .expect("a location serialises");
        let mut lakes = opened().lock().await;
        if let Some(store) = lakes.get(&key) {
            // The lake is open in this process already: a rebuild is its
            // store's own.
            if mode == OpenMode::Rebuild {
                store.rebuild().await?;
            }
            return Ok(store.clone());
        }
        let seams = Seams {
            engines: engines(),
            ..host
        };
        let mode = match mode {
            OpenMode::Read => OpenMode::Read,
            OpenMode::Write | OpenMode::Create => OpenMode::Create,
            OpenMode::Rebuild => OpenMode::Rebuild,
        };
        let store = lakelands_store::open(location, seams, mode).await?;
        // A projection an older build laid out was rebuilt by this open
        // (lakelands#1151).
        if let Some(why) = store.rebuilt_at_open() {
            eprintln!("lakelands: the projection was rebuilt from the log at this open: {why}");
        }
        // An owed reclaim kept no writer out; the host is told its cause
        // (lakelands#1161).
        if let Some(why) = store.reclaim_owed() {
            eprintln!("lakelands: {}", commands::owed(&why));
        }
        lakes.insert(key, store.clone());
        Ok(store)
    })
}

fn command() -> Command {
    let lake = Arg::new("lake")
        .long("lake")
        .value_name("ID")
        .global(true)
        .help("The lake to work on; may be left out when the root holds only one");
    Command::new("lakelands")
        .about("Keep and query lakes: ontology-driven, event-sourced stores for AI agents")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("root")
                .long("root")
                .value_name("DIR")
                .global(true)
                .default_value(".")
                .help("The Lakelands root to work in"),
        )
        .arg(
            Arg::new("as")
                .long("as")
                .value_name("NAME")
                .global(true)
                .help("Act as this principal from the root's configuration instead of the owner"),
        )
        .arg(lake)
        .subcommand(
            Command::new("init")
                .about("Create a Lakelands root in a directory")
                .override_usage("lakelands init [DIR] [--owner <ID>] [--example [<NAME>]]\n       lakelands init --root <DIR> [--owner <ID>] [--example [<NAME>]]")
                .long_about(
                    "Create a Lakelands root in a directory, created if missing. Name the directory \
                     as the argument (`lakelands init DIR`) or with --root (`lakelands init --root DIR`); \
                     when both are given, the argument wins.",
                )
                .arg(
                    Arg::new("dir")
                        .value_name("DIR")
                        .help("The directory to hold the root; --root when left out"),
                )
                .arg(
                    Arg::new("owner")
                        .long("owner")
                        .value_name("ID")
                        .default_value("owner")
                        .help("The id recorded as the writer when you act as the owner"),
                )
                .arg(
                    Arg::new("example")
                        .long("example")
                        .value_name("NAME")
                        .num_args(0..=1)
                        .default_missing_value("")
                        .help("Make a lake from an example this binary carries, with an MCP configuration for it; without a name, list them"),
                ),
        )
        .subcommand(
            Command::new("lake")
                .about("Manage the lakes of the root")
                .subcommand_required(true)
                .subcommand(
                    Command::new("new")
                        .about("Add a lake to the root")
                        .arg(Arg::new("id").value_name("ID").required(true).help("The lake's name"))
                        .arg(
                            Arg::new("engine")
                                .long("engine")
                                .value_name("ENGINE")
                                .default_value("sqlite")
                                .help("Where the lake is kept: `sqlite` on disk, `postgres` in a database, or `memory` for as long as the process runs"),
                        )
                        .arg(Arg::new("url").long("url").value_name("URL").help("For `postgres`: the database's URL. The location file holds it as plain text, a password in it too: keep the root's `lakes/` to its operator"))
                        .arg(Arg::new("schema").long("schema").value_name("NAME").help("For `postgres`: the schema; `lakelands_<id>` when left out"))
                        .arg(Arg::new("no-tls").long("no-tls").action(clap::ArgAction::SetTrue).help("For `postgres`: connect without TLS"))
                        .arg(Arg::new("ca-file").long("ca-file").value_name("PEM").help("For `postgres`: the certificate authorities to verify the server against")),
                ),
        )
        .subcommand(
            Command::new("apply")
                .about("Declare the types, verbs and rules of an ontology bundle in the lake")
                .arg(Arg::new("bundle").value_name("DIR").required(true).help("The bundle's directory"))
                .arg(Arg::new("plan").long("plan").value_name("ID").help("Apply it only if its plan is still this one (the id `lakelands plan` printed)")),
        )
        .subcommand(
            Command::new("plan")
                .about("Tell what applying an ontology bundle would do to the lake's records, changing nothing")
                .arg(Arg::new("bundle").value_name("DIR").required(true).help("The bundle's directory")),
        )
        .subcommand(
            Command::new("recompute")
                .about("Compute again what a function or a type in force computes: retry a failed call, make a field that waits")
                .arg(Arg::new("function").long("function").value_name("NAME").conflicts_with("type").help("A function: every type that names it"))
                .arg(Arg::new("type").long("type").value_name("NAME").help("A type with a field computed outside the lake")),
        )
        .subcommand(
            Command::new("ingest")
                .about("Feed the lake records from a source")
                .arg(Arg::new("file").value_name("FILE").required(true).help("One JSON record per line, or CSV with --map"))
                .arg(Arg::new("source").long("source").value_name("ID").required(true).help("The source the records come from"))
                .arg(Arg::new("map").long("map").value_name("FILE").help("Read FILE as CSV, mapping columns to fields as this JSON file says")),
        )
        .subcommand(
            Command::new("feed")
                .about("Print the lake's changes from a position, one JSON event per line")
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_name("POSITION")
                        .default_value("0")
                        .value_parser(clap::value_parser!(u64))
                        .help("The first position to print"),
                )
                .arg(
                    Arg::new("kind")
                        .long("kind")
                        .value_name("KIND")
                        .action(ArgAction::Append)
                        .help("Only events of this kind (repeatable), e.g. record_upserted"),
                ),
        )
        .subcommand(
            Command::new("age")
                .about("Run the lake's aging rules once for a date")
                .arg(
                    Arg::new("today")
                        .long("today")
                        .value_name("DATE")
                        .required(true)
                        .help("The date to age, YYYY-MM-DD"),
                ),
        )
        .subcommand(
            Command::new("purge")
                .about("Retire everything one source brought into the lake, as one act")
                .arg(
                    Arg::new("source")
                        .long("source")
                        .value_name("ID")
                        .required(true)
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("The source whose records are retired"),
                )
                .arg(Arg::new("type").long("type").value_name("TYPE").value_parser(clap::builder::NonEmptyStringValueParser::new()).help("Retire only what the source brought into this type and its relations' links"))
                .arg(Arg::new("preflight").long("preflight").action(ArgAction::SetTrue).conflicts_with("report").help("Say what the purge would retire and strand, appending nothing"))
                .arg(Arg::new("report").long("report").value_name("ID").help("Purge only if the preflight with this id is of this purge and the lake has not moved since")),
        )
        .subcommand(
            Command::new("forget")
                .about("Forget one source: destroy its keys, so every value it sealed reads as erased, as one act")
                .arg(
                    Arg::new("source")
                        .long("source")
                        .value_name("ID")
                        .required(true)
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("The source to forget"),
                )
                .arg(Arg::new("preflight").long("preflight").action(ArgAction::SetTrue).conflicts_with("report").help("Say what the act would erase and retire, and how long it would hold the writer, appending nothing; what `forget` does without --report"))
                .arg(Arg::new("report").long("report").value_name("ID").help("Forget, only under the id of a preflight of this source at this head")),
        )
        .subcommand(
            Command::new("values")
                .about("Act on part of a source's values")
                .subcommand_required(true)
                .subcommand(
                    Command::new("remove")
                        .about("Take back what one source wrote before a time, in one or more deliveries, or of what it observed before a time: its values read as erased at every position, as one act")
                        .arg(
                            Arg::new("source")
                                .long("source")
                                .value_name("ID")
                                .required(true)
                                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                                .help("The source whose values are taken back"),
                        )
                        .arg(Arg::new("before").long("before").value_name("RFC3339").value_parser(|s: &str| lakelands_api::Timestamp::parse(s).map_err(|e| e.to_string())).help("Take back what the source wrote before this instant: RFC 3339, or a date at midnight UTC"))
                        .arg(Arg::new("delivery").long("delivery").value_name("ID").action(ArgAction::Append).value_parser(clap::builder::NonEmptyStringValueParser::new()).help("Take back what the source wrote in this delivery; repeated, in any of them, as one act"))
                        .arg(Arg::new("seen-before").long("seen-before").value_name("RFC3339").value_parser(|s: &str| lakelands_api::Timestamp::parse(s).map_err(|e| e.to_string())).help("Take back what the source wrote of what it says it observed before this instant (its seen_at): RFC 3339, or a date at midnight UTC"))
                        .group(clap::ArgGroup::new("selection").args(["before", "delivery", "seen-before"]).required(true))
                        .arg(Arg::new("require-shredded").long("require-shredded").action(ArgAction::SetTrue).help("Refuse while a value of an erasable field would be redacted only, its key version not destroyed"))
                        .arg(Arg::new("preflight").long("preflight").action(ArgAction::SetTrue).conflicts_with("report").help("Say what the act would take, retire and leave broken, and how long it would hold the writer, appending nothing; what `values remove` does without --report"))
                        .arg(Arg::new("report").long("report").value_name("ID").help("Take back, only under the id of a preflight of this removal at this head")),
                ),
        )
        .subcommand(Command::new("rebuild").about("Rebuild the lake's projection from the log beside it, and put it in the old one's place"))
        .subcommand(
            Command::new("build-ahead")
                .about("Build the projection this release lays out beside the lake's, from its log, while an older release still serves it: the upgrade's open then folds only what was written since, and swaps it in")
                .after_help(
                    "It says how much room it takes before it starts. It opens the lake's log and \
                     its own scratch only, never the projection or the lease, so it runs beside the \
                     writer; run again, it goes on from where it stopped. `lakelands upgrade` with \
                     this release swaps the build in, once the older release is stopped.",
                )
                .arg(
                    Arg::new("discard")
                        .long("discard")
                        .action(ArgAction::SetTrue)
                        .help("Remove the build ahead kept beside the lake, of this release or another, instead"),
                ),
        )
        .subcommand(Command::new("compact").about("Give back the storage the lake's projection no longer uses"))
        .subcommand(
            Command::new("upgrade")
                .about("Open the lake for writing once, which upgrades one written in an older log format, the events staying as they were written, and rebuilds a projection an older build laid out")
                .arg(
                    Arg::new("check")
                        .long("check")
                        .action(ArgAction::SetTrue)
                        .help("Say what an upgrade would do, and open nothing"),
                ),
        )
        .subcommand(
            Command::new("verify")
                .about("Check the lake's log: every event's hash and every checkpoint; it only reads, beside a running serve")
                .after_help(
                    "Exit status: 0 the log holds; 1 the log is damaged, at the position the line \
                     names where the log lets it; 2 the command line was not understood, or its \
                     range is not one of the log (--from past --through or past the head); 3 the \
                     log could not be checked (no lake yet, no projection, a memory lake, the \
                     database unreachable, the storage failing).",
                )
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_name("POSITION")
                        .value_parser(clap::value_parser!(u64))
                        .help("The first position to check (the log's first, if left out)"),
                )
                .arg(
                    Arg::new("through")
                        .long("through")
                        .value_name("POSITION")
                        .value_parser(clap::value_parser!(u64))
                        .help("The last position to check (the head, if left out)"),
                ),
        )
        .subcommand(
            Command::new("export")
                .about("Write the lake's log as its segment files and a manifest; it only reads, beside a running serve")
                .arg(
                    Arg::new("to")
                        .long("to")
                        .value_name("DIR")
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("An empty or new directory to write into; by default one of its own in the root's exports_dir"),
                ),
        )
        .subcommand(
            Command::new("import")
                .about("Make the export's lake from its files, in the root's empty lake of that name")
                .arg(
                    Arg::new("dir")
                        .value_name("DIR")
                        .required(true)
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("A directory `lakelands export` wrote"),
                ),
        )
        .subcommand(
            Command::new("backup")
                .about("Bring a backup directory up to the lake's head, writing only what is new; it only reads, beside a running serve")
                .long_about(
                    "Bring a backup directory up to the lake's head: a new or empty directory takes \
                     the whole log, one an earlier backup of this lake wrote takes only the events \
                     after its last; it only reads, beside a running serve. The lake's keys are \
                     never written into it: --keys-to copies them into a directory of their own \
                     (`docs/hosting.md`, Backups).",
                )
                .arg(
                    Arg::new("to")
                        .long("to")
                        .value_name("DIR")
                        .required(true)
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("The backup's directory: new, empty, or one this lake's backup wrote"),
                )
                .arg(
                    Arg::new("keys-to")
                        .long("keys-to")
                        .value_name("DIR")
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("Copy the lake's file of the root's keyring into this directory, apart from the backup"),
                ),
        )
        .subcommand(
            Command::new("restore")
                .about("Make the backup's lake from its directory, in the root's empty lake of that name; --check only verifies it")
                .after_help(
                    "Exit status of --check: 0 the backup holds; 1 it is damaged; 2 the command line \
                     was not understood; 3 it could not be checked.",
                )
                .arg(
                    Arg::new("dir")
                        .value_name("DIR")
                        .required(true)
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("A directory `lakelands backup` or `lakelands export` wrote"),
                )
                .arg(
                    Arg::new("keys-from")
                        .long("keys-from")
                        .value_name("DIR")
                        .value_parser(clap::builder::NonEmptyStringValueParser::new())
                        .help("Put the lake's keyring file back from this copy first, where the root's keyring holds none"),
                )
                .arg(
                    Arg::new("check")
                        .long("check")
                        .action(ArgAction::SetTrue)
                        .help("Verify the backup's chain and change nothing"),
                ),
        )
        .subcommand(
            Command::new("settings")
                .about("Show every setting with its value for the lake; set or unset a lake setting")
                .long_about(
                    "Show every setting with its value for the lake, kept in the lake, its location or \
                     the server's configuration (`docs/settings.md`); it only reads, beside a running \
                     serve. `set` and `unset` write a lake setting, judged as any write of it is.",
                )
                .subcommand(
                    Command::new("set")
                        .about("Set a lake setting")
                        .arg(Arg::new("key").value_name("KEY").required(true).help("The setting's key"))
                        .arg(Arg::new("value").value_name("VALUE").required(true).help("Its value, as JSON; any other text is taken as a text"))
                        .arg(Arg::new("type").long("type").value_name("TYPE").help("Set it for this type: the key `<KEY>.<TYPE>`")),
                )
                .subcommand(
                    Command::new("unset")
                        .about("Retire a lake setting, so it reads its default again")
                        .arg(Arg::new("key").value_name("KEY").required(true).help("The setting's key"))
                        .arg(Arg::new("type").long("type").value_name("TYPE").help("Retire it for this type: the key `<KEY>.<TYPE>`")),
                ),
        )
        .subcommand(
            Command::new("serve")
                .about("Serve the lake to agents")
                .arg(Arg::new("mcp").long("mcp").action(ArgAction::SetTrue).help("Serve over the Model Context Protocol")),
        )
}

/// The runtime a command line runs on: `serve` on the threads the root's
/// server section names ([`lakelands_server::runtime`]), every other
/// command on the one thread that starts it. A command line that does not
/// parse, or a root that does not open, is left to [`run`] to say.
pub fn runtime<I, T>(args: I) -> std::io::Result<tokio::runtime::Runtime>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let m = command().try_get_matches_from(args).ok();
    match m.filter(|m| m.subcommand_name() == Some("serve")) {
        Some(m) => {
            let dir = PathBuf::from(m.get_one::<String>("root").map_or(".", String::as_str));
            let server = root::Root::open(&dir).ok().and_then(|r| r.config.server);
            lakelands_server::runtime(server.as_ref())
        }
        None => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    }
}

/// Run one command line. Output goes to `out`, failures to `err`; the
/// answer is the process's exit status: 0 done, 1 refused or failed,
/// 2 the command line was not understood, or `settings` was asked to
/// write a setting kept in a file. `verify` answers its own: 0 the log
/// holds, 1 it is damaged, 3 it could not be checked.
pub async fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write, open: &Opener<'_>) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let m = match command().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            let text = e.render().to_string();
            if e.use_stderr() {
                let _ = write!(err, "{text}");
                return 2;
            }
            let _ = write!(out, "{text}");
            return 0;
        }
    };
    if m.subcommand_name() == Some("verify") {
        return verify(&m, out, err, open).await;
    }
    if let Some(("restore", sub)) = m.subcommand()
        && sub.get_flag("check")
    {
        return restore_check(sub, out, err);
    }
    let lake = |m: &ArgMatches| {
        let root = PathBuf::from(m.get_one::<String>("root").map_or(".", String::as_str));
        let named = m.subcommand()?.1.get_one::<String>("lake").cloned();
        let root = root::Root::open(&root).ok()?;
        Some(root.location(named.as_deref()).ok()?.lake.to_string())
    };
    if let Some(f) = settings::elsewhere(&m, lake(&m)) {
        let _ = write!(err, "{f}");
        return 2;
    }
    match dispatch(&m, out, open).await {
        Ok(()) => 0,
        Err(f) => {
            let _ = write!(err, "{f}");
            1
        }
    }
}

async fn dispatch(
    m: &ArgMatches,
    out: &mut dyn Write,
    open: &Opener<'_>,
) -> std::result::Result<(), Failure> {
    let root_dir = PathBuf::from(m.get_one::<String>("root").map_or(".", String::as_str));
    let text = |m: &ArgMatches, id: &str| m.get_one::<String>(id).cloned();
    let (name, sub) = m.subcommand().expect("a subcommand is required");
    let say = |out: &mut dyn Write, line: String| {
        writeln!(out, "{line}").map_err(|e| {
            Failure::new(
                format!("could not write output: {e}"),
                "check where the output goes",
            )
        })
    };
    match name {
        "init" => {
            // The argument names the new root; without one, --root does.
            let dir = text(sub, "dir").map_or_else(|| root_dir.clone(), PathBuf::from);
            let owner = text(sub, "owner").unwrap_or_default();
            match text(sub, "example").as_deref() {
                Some("") => return example::list(out),
                Some(name) => return example::init(out, &dir, &owner, name, open).await,
                None => {}
            }
            root::init(&dir, &owner)?;
            say(
                out,
                format!("created a Lakelands root in {}", dir.display()),
            )?;
            return say(
                out,
                format!("  next: lakelands --root {} lake new <id>", dir.display()),
            );
        }
        "lake" => {
            let root = root::Root::open_to_add_a_lake(&root_dir)?;
            let (_, new) = sub.subcommand().expect("a lake subcommand is required");
            let id = text(new, "id").unwrap_or_default();
            let engine = text(new, "engine").unwrap_or_default();
            let pg = root::PgNew {
                url: text(new, "url"),
                schema: text(new, "schema"),
                tls_disabled: new.get_flag("no-tls"),
                ca_file: text(new, "ca-file").map(PathBuf::from),
            };
            root.new_lake(&id, &engine, &pg)?;
            return say(
                out,
                format!(
                    "created lake `{id}` (engine {engine}) in {}",
                    Path::new("lakes").join(format!("{id}.json")).display()
                ),
            );
        }
        "import" | "restore" => {
            let root = root::Root::open(&root_dir)?;
            let dir = PathBuf::from(text(sub, "dir").unwrap_or_default());
            // The export's lake, which is inside every hash and cannot be
            // renamed; `--lake` names it too, and the store refuses a
            // mismatch naming both.
            let lake = text(sub, "lake").or_else(|| commands::exported_lake(&dir));
            let location = root.location(lake.as_deref())?;
            // A restore is an import, its keys put back first where the
            // keyring has lost them, and taken out again where the import
            // is refused, so the next try puts them back too.
            let keys = (name == "restore").then(|| text(sub, "keys-from"));
            let put_back = match keys.flatten() {
                Some(keys) => Some(commands::restore_keys(
                    out,
                    &root,
                    &location.lake,
                    Path::new(&keys),
                )?),
                None => None,
            };
            let lake = location.lake.clone();
            let seams = Seams {
                engines: engines(),
                ..root.seams()
            };
            let imported = commands::import(out, location, seams, &dir).await;
            if let (Err(_), Some(keyring)) = (&imported, put_back) {
                keyring.unrestore(&lake)?;
            }
            return imported?;
        }
        "serve" if !sub.get_flag("mcp") => {
            return Err(Failure::new(
                "nothing to serve was named",
                "pass --mcp to serve the lake over the Model Context Protocol",
            ));
        }
        _ => {}
    }

    let root = root::Root::open(&root_dir)?;
    let location = root.location(text(sub, "lake").as_deref())?;
    let listed = location.clone();
    // `upgrade` reads the log's ends first, to say what it does; with
    // `--check` it opens nothing (`docs/log-format.md` §6).
    if name == "upgrade" {
        let check = sub.get_flag("check");
        return commands::upgrade(out, location, root.seams(), check, open).await;
    }
    // `build-ahead` opens the log and its own scratch, never the lake.
    if name == "build-ahead" {
        let seams = Seams {
            engines: engines(),
            ..root.seams()
        };
        return commands::build_ahead(out, location, seams, sub.get_flag("discard")).await;
    }
    // `feed`, `export` and `backup` only read: they take no lease and append
    // nothing, so they run beside a `serve` that holds the writer, and an
    // export's head is the lake's as it stood
    // (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
    // A lake never opened yet is created by the first open, and only
    // such a place is opened to create: a lake that is refused as it
    // stands, a damaged one among them, is not opened again for writing.
    // `rebuild` opens the lake for a rebuild, which that open is: a
    // projection of another layout opens only so.
    // `serve` binds its door before it opens anything, the primary lake
    // included, so that an open that rebuilds a projection is `starting`
    // for `/readyz` and up for `/livez`.
    let door = if name == "serve" {
        Door::bind(&root).await?
    } else {
        None
    };
    let store = if name == "rebuild" {
        open(location, OpenMode::Rebuild, root.seams()).await?
    } else if name == "feed"
        || name == "export"
        || name == "backup"
        || (name == "settings" && sub.subcommand().is_none())
    {
        match open(location.clone(), OpenMode::Read, root.seams()).await {
            Ok(store) => store,
            Err(e) if unmade(&e) => open(location, OpenMode::Create, root.seams()).await?,
            Err(e) => return Err(e.into()),
        }
    } else {
        open(location, OpenMode::Create, root.seams()).await?
    };
    let who = root
        .principal(store.lake(), text(sub, "as").as_deref())
        .await?;
    let catalog = store.catalog(WriteContext::default());
    match name {
        "apply" => {
            let (dir, id) = (text(sub, "bundle").unwrap_or_default(), text(sub, "plan"));
            let at = (Path::new(&dir), id.as_deref());
            commands::apply(out, catalog.as_ref(), &who, at).await
        }
        "plan" => {
            let dir = text(sub, "bundle").unwrap_or_default();
            commands::plan(out, catalog.as_ref(), &who, Path::new(&dir)).await
        }
        "recompute" => {
            let (function, ty) = (text(sub, "function"), text(sub, "type"));
            commands::recompute(out, catalog.as_ref(), &who, function, ty).await
        }
        "ingest" => {
            let map = text(sub, "map").map(PathBuf::from);
            commands::ingest(
                out,
                catalog.as_ref(),
                &who,
                Path::new(&text(sub, "file").unwrap_or_default()),
                &text(sub, "source").unwrap_or_default(),
                map.as_deref(),
            )
            .await
        }
        "feed" => {
            let from = Position(sub.get_one::<u64>("from").copied().unwrap_or(0));
            let kinds = sub
                .get_many::<String>("kind")
                .map(|k| k.cloned().collect())
                .unwrap_or_default();
            commands::feed(out, &store, from, kinds).await
        }
        "age" => commands::age(out, &store, &text(sub, "today").unwrap_or_default()).await,
        "purge" => {
            let (source, report) = (text(sub, "source").unwrap_or_default(), text(sub, "report"));
            let report = (!sub.get_flag("preflight")).then_some(report.as_deref());
            let of = (source.as_str(), text(sub, "type"));
            commands::purge(out, &store, (of.0, of.1.as_deref()), report).await
        }
        "forget" => {
            // An act that cannot be undone is never one flag away: it runs
            // only under a preflight's id, and otherwise answers the
            // preflight (058).
            let (source, report) = (text(sub, "source").unwrap_or_default(), text(sub, "report"));
            commands::forget(out, &store, &source, report.as_deref().map(Some)).await
        }
        "values" => {
            // As `forget`: the act runs only under a preflight's id (065).
            let of = sub.subcommand().map_or(sub, |(_, of)| of);
            let source = text(of, "source").unwrap_or_default();
            let at = |name: &str| of.get_one::<lakelands_api::Timestamp>(name).copied();
            let selection = match (at("before"), at("seen-before")) {
                (Some(t), _) => lakelands_api::write::Selection::WrittenBefore(t),
                (_, Some(t)) => lakelands_api::write::Selection::SeenBefore(t),
                _ => {
                    let mut set: Vec<lakelands_api::DeliveryId> = (of
                        .get_many::<String>("delivery"))
                    .into_iter()
                    .flatten()
                    .map(|d| d.as_str().into())
                    .collect();
                    match set.len() {
                        1 => lakelands_api::write::Selection::Delivery(set.remove(0)),
                        _ => lakelands_api::write::Selection::Deliveries(set),
                    }
                }
            };
            let (shredded, report) = (of.get_flag("require-shredded"), text(of, "report"));
            let act = (shredded, report.as_deref().map(Some));
            commands::values_remove(out, &store, (&source, &selection), act).await
        }
        "rebuild" => commands::rebuild(out, &store).await,
        "compact" => commands::compact(out, &store).await,
        "export" => {
            let to = text(sub, "to").map_or_else(|| root.export_dir(store.lake()), PathBuf::from);
            commands::export(out, &store, &to).await
        }
        "backup" => {
            let to = PathBuf::from(text(sub, "to").unwrap_or_default());
            let keys = text(sub, "keys-to").map(PathBuf::from);
            commands::backup(out, &store, &root, &to, keys.as_deref()).await
        }
        "settings" => match sub.subcommand() {
            Some((change, of)) => {
                let key = settings::key_of(of);
                let raw = (change == "set").then(|| text(of, "value").unwrap_or_default());
                settings::change(out, catalog.as_ref(), &who, &key, raw.as_deref()).await
            }
            None => {
                let lake = catalog.describe(&who, Default::default()).await?.data;
                settings::list(out, &lake, &listed, &root.config)
            }
        },
        "serve" => serve(&root, (store, door), who, open).await,
        other => unreachable!("clap knows no command `{other}`"),
    }
}

/// Run `restore --check`: the backup's chain verified as an import would,
/// nothing opened or written. Its exit status is `verify`'s: 0 the backup
/// holds, 1 it is damaged, 3 it could not be checked.
fn restore_check(sub: &ArgMatches, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    use lakelands_api::error::Situation;
    let dir = PathBuf::from(sub.get_one::<String>("dir").map_or("", String::as_str));
    let (code, line) = match lakelands_core::export::verify(&dir) {
        Ok(v) => (
            0,
            format!(
                "the backup in {} holds lake `{}` from position 0 through {} (last hash {})",
                dir.display(),
                commands::exported_lake(&dir).unwrap_or_default(),
                v.through,
                v.hash.0
            ),
        ),
        Err(Error::Unavailable { situation })
            if matches!(*situation, Situation::Damaged { .. }) =>
        {
            let todo = "  what to do: restore from another backup; nothing was changed";
            (
                1,
                format!("the backup in {}: {situation}\n{todo}", dir.display()),
            )
        }
        Err(e) => {
            let _ = write!(err, "{}", Failure::from(e));
            return 3;
        }
    };
    match writeln!(out, "{line}") {
        Ok(()) => code,
        Err(_) => 3,
    }
}

/// Run `verify`: the lake's log checked from its first position or `--from`
/// through its head or `--through`, opened only to read, so beside a
/// `serve` that holds the writer. A lake whose open finds it damaged is
/// answered as one `verify` finds damaged. Its exit status is its own: 0
/// the log holds, 1 it is damaged, 3 it could not be checked.
async fn verify(m: &ArgMatches, out: &mut dyn Write, err: &mut dyn Write, open: &Opener<'_>) -> u8 {
    use lakelands_api::error::Situation;
    // A failure on `err` and its exit status: 2 for what the command line
    // asked, 3 where the log could not be checked.
    let failed = |err: &mut dyn Write, f: Failure, code: u8| {
        let _ = write!(err, "{f}");
        code
    };
    let sub = m.subcommand_matches("verify").expect("the verify command");
    let root_dir = PathBuf::from(m.get_one::<String>("root").map_or(".", String::as_str));
    let root = match root::Root::open(&root_dir) {
        Ok(root) => root,
        Err(f) => return failed(err, f, 3),
    };
    let location = match root.location(sub.get_one::<String>("lake").map(String::as_str)) {
        Ok(location) => location,
        Err(f) => return failed(err, f, 3),
    };
    let lake = location.lake.clone();
    if matches!(location.engine, EngineConfig::Memory) {
        let what = format!(
            "lake `{lake}` is kept in memory: a memory lake lives only in the process that runs \
             it, so there is nothing to verify"
        );
        return failed(
            err,
            Failure::new(what, "verify a lake kept on disk or in a database"),
            3,
        );
    }
    let at = |id: &str| sub.get_one::<u64>(id).copied().map(Position);
    if let (Some(from), Some(through)) = (at("from"), at("through"))
        && from > through
    {
        let what = format!("--from {from} is past --through {through}");
        return failed(
            err,
            Failure::new(what, "give a --from no later than --through"),
            2,
        );
    }
    // Read only: no lease taken, nothing appended, and a lake never
    // opened is not made by checking it.
    let checked = match open(location, OpenMode::Read, root.seams()).await {
        Err(e) if matches!(e.situation(), Some(Situation::NoLake { .. })) => {
            let what = format!("there is no lake `{lake}` to verify yet");
            let todo = "a lake is made by the first command that writes to it";
            return failed(err, Failure::new(what, todo), 3);
        }
        Err(e) if matches!(e.situation(), Some(Situation::NoProjection { .. })) => {
            let what = format!("the log of lake `{lake}` is there, its projection is not");
            let todo =
                "`lakelands rebuild` makes the projection from the log; verify then reads it";
            return failed(err, Failure::new(what, todo), 3);
        }
        Err(e) => Err(e),
        Ok(store) => store.verify(at("from"), at("through")).await,
    };
    let said = match checked {
        Ok(v) => vec![format!(
            "lake `{lake}`: the log holds from position {} through {}: every event links to the \
             one before it, and every checkpoint matches its segments (last hash {})",
            v.from, v.through, v.hash.0
        )],
        Err(Error::Unavailable { situation })
            if matches!(*situation, Situation::Damaged { .. }) =>
        {
            let Situation::Damaged { position, .. } = &*situation else {
                unreachable!("matched as damaged")
            };
            let before = position
                .map(|p| format!(" taken before position {p}"))
                .unwrap_or_default();
            let todo = format!(
                "  what to do: restore the lake from a backup{before}; nothing was changed"
            );
            let lines = vec![format!("lake `{lake}`: {situation}"), todo];
            for line in lines {
                if writeln!(out, "{line}").is_err() {
                    return 3;
                }
            }
            return 1;
        }
        // A range past the head is the command line's.
        Err(
            e @ Error::Refused {
                reason: lakelands_api::Refusal::OutOfRange { .. },
                ..
            },
        ) => return failed(err, e.into(), 2),
        Err(e) => return failed(err, e.into(), 3),
    };
    for line in said {
        if writeln!(out, "{line}").is_err() {
            return 3;
        }
    }
    0
}

/// Whether `e` says the place holds no lake yet, or one whose making
/// stopped before its projection: what an open to create finishes.
fn unmade(e: &Error) -> bool {
    use lakelands_api::error::Situation;
    matches!(
        e.situation(),
        Some(Situation::NoLake { .. } | Situation::NoProjection { .. })
    )
}

/// The HTTP door of `serve`, bound and running before any lake is open.
struct Door {
    addr: std::net::SocketAddr,
    served: Arc<std::sync::RwLock<BTreeMap<LakeId, Arc<dyn LakeStore>>>>,
    health: lakelands_server::Health,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Door {
    /// Bind and run the door the root configures, answering `NotServed` for
    /// every lake and `starting` at `/readyz` until [`Door::serving`]; none
    /// where the root configures no server.
    async fn bind(root: &root::Root) -> std::result::Result<Option<Door>, Failure> {
        let Some(config) = root.config.server.clone() else {
            return Ok(None);
        };
        let served: Arc<std::sync::RwLock<BTreeMap<LakeId, Arc<dyn LakeStore>>>> = Arc::default();
        let lookup = {
            let served = served.clone();
            move |lake: &LakeId| -> Result<Arc<dyn LakeStore>> {
                let served = served.read().unwrap_or_else(|e| e.into_inner());
                served.get(lake).cloned().ok_or_else(|| {
                    let lake = lake.to_string();
                    Error::unavailable(lakelands_api::error::Situation::NotServed { lake })
                })
            }
        };
        let resolver = Arc::new(lakelands_server::resolver::StaticResolver::new(
            root.config.principals.clone(),
        ));
        let http = lakelands_server::serve_http(config, resolver, Arc::new(lookup))
            .await
            .map_err(stopped)?;
        let health = http.health();
        // Standard output is the protocol's; the address goes to standard
        // error, so a server bound to port 0 can be found. A caller that
        // waits for the lakes reads the later "serving" line.
        let addr = http.local_addr().map_err(stopped)?;
        eprintln!("lakelands: listening at http://{addr}/ (starting)");
        let task = tokio::spawn(http.run());
        Ok(Some(Door {
            addr,
            served,
            health,
            task,
        }))
    }

    /// Every lake is open: the door serves them and says it is ready.
    fn serving(&self, lakes: BTreeMap<LakeId, Arc<dyn LakeStore>>) {
        let held = lakes.values().cloned().collect();
        *self.served.write().unwrap_or_else(|e| e.into_inner()) = lakes;
        self.health.serving(held);
        eprintln!("lakelands: serving HTTP at http://{}/", self.addr);
    }
}

/// A `serve` that fails after its bind does not leave the door answering.
impl Drop for Door {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn stopped(e: std::io::Error) -> Failure {
    Failure::new(
        format!("the server stopped: {e}"),
        "check the server section of the configuration and that the port is free",
    )
}

/// `serve --mcp`: the lake on standard input and output, as the principal
/// the command acts as; and, when the root configures the HTTP door, every
/// lake of the root over HTTP, each caller resolved from the root's
/// principals. Standard output carries the protocol only. The command ends
/// when its stdio client closes, HTTP door or not, and the process's
/// deliverers and writer leases end with it.
async fn serve(
    root: &root::Root,
    (store, door): (Arc<dyn LakeStore>, Option<Door>),
    who: lakelands_api::Principal,
    open: &Opener<'_>,
) -> std::result::Result<(), Failure> {
    let Some(mut door) = door else {
        root.deliver(std::slice::from_ref(&store))?;
        ticker::start(root.config.aging.as_ref(), std::slice::from_ref(&store));
        orders::start(std::slice::from_ref(&store));
        root.mirror(std::slice::from_ref(&store), open).await?;
        return lakelands_server::serve_stdio(store, who)
            .await
            .map_err(stopped);
    };
    // The primary lake is open already; the others open now.
    let mut lakes = BTreeMap::from([(store.lake().clone(), store.clone())]);
    for name in root.lakes() {
        let location = root.location(Some(&name))?;
        if !lakes.contains_key(&location.lake) {
            lakes.insert(
                location.lake.clone(),
                open(location, OpenMode::Create, root.seams()).await?,
            );
        }
    }
    let held: Vec<Arc<dyn LakeStore>> = lakes.values().cloned().collect();
    root.deliver(&held)?;
    ticker::start(root.config.aging.as_ref(), &held);
    orders::start(&held);
    root.mirror(&held, open).await?;
    door.serving(lakes);
    // The command ends with its stdio client: the HTTP door beside it, the
    // deliverers and the lake's writer lease end with the process.
    tokio::select! {
        stdio = lakelands_server::serve_stdio(store, who) => stdio.map_err(stopped),
        http = &mut door.task => http
            .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            .map_err(stopped),
    }
}
