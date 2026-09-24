//! The `lakelands` command-line door.
//!
//! `init` and `lake new` lay out a root: a directory with a
//! configuration and one location per lake. `apply`, `ingest`, `feed`,
//! `rebuild` and `serve` open a lake at its location and call the
//! library in-process: there is no command enum and no layer between the
//! command and the catalog.
//!
//! Opening a lake is one function, [`open_store`], and [`run`] takes it
//! as a parameter so that tests can pass another. `open_store` opens a
//! lake through `lakelands_store::open` with the engines this binary
//! carries (SQLite, in-memory and PostgreSQL), once per lake for the life of the
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
use lakelands_api::location::Location;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, WriteContext};

mod commands;
mod root;
use lakelands_server::say;

pub use root::{CONFIG, Config};
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

/// The one place the CLI opens a lake: through the store, with the SQLite,
/// in-memory and PostgreSQL engines. To write, it creates the lake at its location
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
            return Ok(store.clone());
        }
        let seams = Seams {
            engines: vec![
                Arc::new(lakelands_engine_sqlite::SqliteOpener),
                Arc::new(memory()),
                Arc::new(lakelands_engine_pg::PgOpener),
            ],
            ..host
        };
        let mode = match mode {
            OpenMode::Read => OpenMode::Read,
            OpenMode::Write | OpenMode::Create => OpenMode::Create,
        };
        let store = lakelands_store::open(location, seams, mode).await?;
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
                .override_usage("lakelands init [DIR] [--owner <ID>]\n       lakelands init --root <DIR> [--owner <ID>]")
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
                                .help("Where the lake is kept: `sqlite` on disk, or `memory` for as long as the process runs"),
                        ),
                ),
        )
        .subcommand(
            Command::new("apply")
                .about("Declare the types, verbs and rules of an ontology bundle in the lake")
                .arg(Arg::new("bundle").value_name("DIR").required(true).help("The bundle's directory")),
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
        .subcommand(Command::new("rebuild").about("Delete the lake's projection and rebuild it from the log"))
        .subcommand(
            Command::new("serve")
                .about("Serve the lake to agents")
                .arg(Arg::new("mcp").long("mcp").action(ArgAction::SetTrue).help("Serve over the Model Context Protocol")),
        )
}

/// Run one command line. Output goes to `out`, failures to `err`; the
/// answer is the process's exit status: 0 done, 1 refused or failed,
/// 2 the command line was not understood.
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
            let root = root::Root::open(&root_dir)?;
            let (_, new) = sub.subcommand().expect("a lake subcommand is required");
            let id = text(new, "id").unwrap_or_default();
            let engine = text(new, "engine").unwrap_or_default();
            root.new_lake(&id, &engine)?;
            return say(
                out,
                format!(
                    "created lake `{id}` (engine {engine}) in {}",
                    Path::new("lakes").join(format!("{id}.json")).display()
                ),
            );
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
    // `feed` only reads: it takes no lease and appends nothing, unless the
    // lake was never opened yet, which the first open creates.
    let store = if name == "feed" {
        match open(location.clone(), OpenMode::Read, root.seams()).await {
            Ok(store) => store,
            Err(Error::Unavailable { .. }) => {
                open(location, OpenMode::Create, root.seams()).await?
            }
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
            commands::apply(
                out,
                catalog.as_ref(),
                &who,
                Path::new(&text(sub, "bundle").unwrap_or_default()),
            )
            .await
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
        "rebuild" => commands::rebuild(out, &store).await,
        "serve" => serve(&root, store, who, open).await,
        other => unreachable!("clap knows no command `{other}`"),
    }
}

/// `serve --mcp`: the lake on standard input and output, as the principal
/// the command acts as; and, when the root configures the HTTP door, every
/// lake of the root over HTTP, each caller resolved from the root's
/// principals. Standard output carries the protocol only.
async fn serve(
    root: &root::Root,
    store: Arc<dyn LakeStore>,
    who: lakelands_api::Principal,
    open: &Opener<'_>,
) -> std::result::Result<(), Failure> {
    let failed = |e: std::io::Error| {
        Failure::new(
            format!("the server stopped: {e}"),
            "check the server section of the configuration and that the port is free",
        )
    };
    let Some(config) = root.config.server.clone() else {
        return lakelands_server::serve_stdio(store, who)
            .await
            .map_err(failed);
    };
    let mut lakes = BTreeMap::new();
    for name in root.lakes() {
        let location = root.location(Some(&name))?;
        lakes.insert(
            location.lake.clone(),
            open(location, OpenMode::Create, root.seams()).await?,
        );
    }
    let lookup = move |lake: &LakeId| -> Result<Arc<dyn LakeStore>> {
        lakes.get(lake).cloned().ok_or_else(|| Error::Unavailable {
            message: format!("this server serves no lake `{lake}`"),
        })
    };
    let resolver = Arc::new(lakelands_server::resolver::StaticResolver::new(
        root.config.principals.clone(),
    ));
    let http = lakelands_server::serve_http(config, resolver, Arc::new(lookup))
        .await
        .map_err(failed)?;
    let (stdio, http) = tokio::join!(lakelands_server::serve_stdio(store, who), http.run());
    stdio.and(http).map_err(failed)
}
