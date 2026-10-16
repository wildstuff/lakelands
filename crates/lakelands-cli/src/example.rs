//! `lakelands init DIR --example NAME`: a root with a lake made from an
//! example this binary carries, and an MCP client's configuration for it.
//!
//! **The examples are in the binary.** A binary installed from a release
//! has no checkout of `examples/` to point at, and fetching the files
//! would be a network call the caller did not configure. So each example's
//! files are read into the binary when it is compiled, from the
//! repository's `examples/`, which stays their only copy; a test holds
//! that each table lists exactly the files of its directory.
//!
//! **One process.** The root and the lake are made, the bundle applied and
//! the data ingested in the order the example's walk types them, through
//! the functions `apply` and `ingest` call. The lake's writer is taken
//! once, not once a command, so the positions are the walk's less one a
//! command after the first.
//!
//! **The MCP configuration.** Claude Code reads a project's servers from
//! `.mcp.json` in the directory it starts in. The command writes that file
//! in `DIR` with one server, `lakelands`, whose command is this binary's
//! own path (it reads no `PATH`) and whose arguments name the root and the
//! lake. A `.mcp.json` that is there already is the user's and is never
//! rewritten: the command prints the entry to add instead, says that this
//! entry is there, or says that its `lakelands` names another command or
//! root and prints this one's. Any other MCP client is given the server's
//! command line.
//!
//! **Nothing half-made.** A root there already, or anything at the lake's
//! location file or store, is refused before the first file is written: a
//! later refusal would leave a root behind, and an old store would be
//! ingested into again, away from the walk's positions.

use std::path::{Path, PathBuf};

use lakelands_api::engine::OpenMode;
use lakelands_api::{Position, WriteContext};
use lakelands_core::bundle;
use serde_json::json;

use crate::say::Failure;
use crate::{Opener, commands, root};

/// An example this binary carries.
pub struct Example {
    /// The name `--example` takes, and the lake's.
    pub name: &'static str,
    /// What it is, in one line.
    pub about: &'static str,
    /// Its directory under the repository's `examples/`.
    pub dir: &'static str,
    /// Every file of that directory, by its path in it, with its contents.
    pub files: &'static [(&'static str, &'static str)],
    /// The ingests, in the walk's order: the data file, its column map
    /// and the source.
    pub ingests: &'static [(&'static str, &'static str, &'static str)],
    /// The ingest whose position the walk pins as "earlier", by index.
    pub pinned: usize,
    /// The walk's page, under the repository.
    pub walk: &'static str,
    /// The walk's first question, the answer it must give, and the answer
    /// at the pinned position.
    pub question: [&'static str; 3],
}

macro_rules! files {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../examples/", $dir, "/", $path)))),*]
    };
}

/// The examples this binary carries.
pub const EXAMPLES: &[Example] = &[Example {
    name: "liquidity",
    about: "walk 1's invoices, partners and credit limits, two months of exports",
    dir: "liquidity",
    files: files!("liquidity":
        "aging_rules/invoice_overdue.yaml",
        "data/invoices-august.csv",
        "data/invoices-september.csv",
        "data/invoices.map.json",
        "data/partners.csv",
        "data/partners.map.json",
        "object_sets/overdue.yaml",
        "triggers/invoice_overdue.yaml",
        "types/invoice.yaml",
        "types/partner.yaml",
        "types/transaction.yaml",
        "value_types/invoice_status.yaml",
        "value_types/money.yaml",
        "verbs/pay_invoice.yaml",
    ),
    ingests: &[
        ("data/partners.csv", "data/partners.map.json", "crm"),
        ("data/invoices-august.csv", "data/invoices.map.json", "erp"),
        (
            "data/invoices-september.csv",
            "data/invoices.map.json",
            "erp",
        ),
    ],
    pinned: 1,
    walk: "docs/walks/walk-1.md",
    question: [
        "Which debtors are over their credit limit and more than 30 days overdue?",
        "P-100 Harbor Supplies and P-200 Northwind Tools",
        "P-100, P-200 and P-300 Cedar Logistics",
    ],
}];

/// The name of the MCP server the command configures.
const SERVER: &str = "lakelands";

fn file(example: &Example, path: &str) -> Result<(String, &'static str), Failure> {
    let Some((_, text)) = example.files.iter().find(|(p, _)| *p == path) else {
        return Err(Failure::new(
            format!("the example `{}` carries no file `{path}`", example.name),
            "this binary is built wrong; report it",
        ));
    };
    Ok((format!("{}:{path}", example.name), text))
}

/// Refuse anything at the lake's location file or store before anything
/// is written; `root::init` refuses a root there already, also before it
/// writes.
fn refuse_what_is_there(dir: &Path, name: &str) -> Result<(), Failure> {
    let lakes = dir.join("lakes");
    let paths = [lakes.join(format!("{name}.json")), lakes.join(name)];
    match paths.iter().find(|p| p.symlink_metadata().is_ok()) {
        Some(there) => Err(Failure::new(
            format!("{} is there already; nothing was changed", there.display()),
            "name a new directory for the example",
        )),
        None => Ok(()),
    }
}

/// `init --example` with no name: the examples this binary carries.
pub fn list(out: &mut dyn std::io::Write) -> Result<(), Failure> {
    let lines = EXAMPLES
        .iter()
        .map(|e| format!("  {}: {}", e.name, e.about));
    let text = std::iter::once("the examples this binary carries:".to_owned()).chain(lines);
    commands::put(out, text.collect::<Vec<_>>().join("\n"))
}

/// `init DIR --example NAME`: the root, the lake with the example's
/// ontology and data, and the MCP configuration, then what to do next.
pub async fn init(
    out: &mut dyn std::io::Write,
    dir: &Path,
    owner: &str,
    name: &str,
    open: &Opener<'_>,
) -> Result<(), Failure> {
    let Some(example) = EXAMPLES.iter().find(|e| e.name == name) else {
        let names: Vec<&str> = EXAMPLES.iter().map(|e| e.name).collect();
        return Err(Failure::new(
            format!("this binary carries no example `{name}`"),
            format!("name one of: {}", names.join(", ")),
        ));
    };
    refuse_what_is_there(dir, example.name)?;
    let made = Made::before(dir, example.name);
    root::init(dir, owner).map_err(|f| made.undo(f))?;
    let pinned = (make(out, dir, example, open).await).map_err(|f| made.undo(f))?;
    let dir = dir.canonicalize().map_err(|e| {
        Failure::new(
            format!("could not find {}: {e}", dir.display()),
            "check the directory, then run init in a new one",
        )
    })?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| {
            Failure::new(
                format!("could not find this binary's own path: {e}"),
                "the lake is made; write the MCP configuration by hand",
            )
        })?;
    let args = [
        "--root",
        &dir.display().to_string(),
        "--lake",
        example.name,
        "serve",
        "--mcp",
    ]
    .map(str::to_owned);
    let exe = exe.display().to_string();
    let server = json!({ "command": exe, "args": args });
    let config = dir.join(".mcp.json");
    let said = match std::fs::read_to_string(&config) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let text = serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER: server } }));
            let new = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&config);
            let wrote = new.and_then(|mut f| {
                std::io::Write::write_all(&mut f, (text.unwrap_or_default() + "\n").as_bytes())
            });
            wrote.map_err(|e| {
                Failure::new(
                    format!("could not write {}: {e}", config.display()),
                    "the lake is made; write the MCP configuration by hand",
                )
            })?;
            format!(
                "wrote {}: Claude Code's MCP server `{SERVER}` for the lake",
                config.display()
            )
        }
        Ok(text) if held(&text).is_some_and(|there| same(there, &server)) => {
            format!(
                "{} holds this server `{SERVER}` already and is left as it is",
                config.display()
            )
        }
        Ok(text) if held(&text).is_some() => {
            let entry = serde_json::to_string_pretty(&json!({ SERVER: server }));
            format!(
                "{} holds a server `{SERVER}` that names another command or root, and is left as it is; \
                 to ask this lake, put this entry in its place under `mcpServers`:\n{}",
                config.display(),
                entry.unwrap_or_default()
            )
        }
        _ => {
            let entry = serde_json::to_string_pretty(&json!({ SERVER: server }));
            format!(
                "{} is there already and is left as it is; add this server under `mcpServers`:\n{}",
                config.display(),
                entry.unwrap_or_default()
            )
        }
    };
    let version = env!("CARGO_PKG_VERSION");
    let repository = env!("CARGO_PKG_REPOSITORY");
    let [question, answer, then] = example.question;
    let lines = [
        said,
        "  its command is this binary's own path: if the binary moves, the file names it no more"
            .to_owned(),
        format!("  for any other MCP client: {exe} {}", args.join(" ")),
        format!(
            "next: cd {} && claude   (approve the server `{SERVER}` when it asks)",
            dir.display()
        ),
        format!("ask:  {question}"),
        format!("      the walk's answer: {answer}"),
        format!(
            "      as the lake stood at position {} (pin it): {then}",
            pinned.0
        ),
        format!(
            "      the walk: {repository}/blob/v{version}/{}",
            example.walk
        ),
    ];
    commands::put(out, lines.join("\n"))
}

/// The lake: its location, the bundle applied and the data ingested.
/// Answers the pinned ingest's position.
async fn make(
    out: &mut dyn std::io::Write,
    dir: &Path,
    example: &Example,
    open: &Opener<'_>,
) -> Result<Position, Failure> {
    root::Root::open_to_add_a_lake(dir)?.new_lake(example.name, "sqlite")?;
    let root = root::Root::open(dir)?;
    let location = root.location(Some(example.name))?;
    let store = open(location, OpenMode::Create, root.seams()).await?;
    let who = root.principal(store.lake(), None).await?;
    let catalog = store.catalog(WriteContext::default());
    let made = format!(
        "created a Lakelands root in {} with the lake `{}`, from the example `{}`",
        dir.display(),
        example.name,
        example.name
    );
    commands::put(out, made)?;
    let files: Vec<(String, String)> = (example.files.iter())
        .map(|(p, text)| ((*p).to_owned(), (*text).to_owned()))
        .collect();
    commands::apply_bundle(out, catalog.as_ref(), &who, bundle::parse(&files)).await?;
    let mut pinned = Position(0);
    for (n, (data, map, source)) in example.ingests.iter().enumerate() {
        let (data, map) = (file(example, data)?, file(example, map)?);
        let map = Some((map.0.as_str(), map.1));
        let through = (data.0.as_str(), data.1);
        let at = commands::ingest_text(out, catalog.as_ref(), &who, through, source, map).await?;
        if n == example.pinned {
            pinned = at.unwrap_or(Position(0));
        }
    }
    Ok(pinned)
}

/// What a run may make, and what was there before it: the refusals before
/// it prove that the root, the lake's location and its store were not.
struct Made {
    dir: PathBuf,
    name: &'static str,
    dir_was: bool,
    lakes_was: bool,
    root_was: bool,
}

impl Made {
    fn before(dir: &Path, name: &'static str) -> Made {
        let was = |p: &Path| p.symlink_metadata().is_ok();
        Made {
            dir: dir.to_owned(),
            name,
            dir_was: was(dir),
            lakes_was: was(&dir.join("lakes")),
            root_was: was(&dir.join(root::CONFIG)),
        }
    }

    /// Remove what this run made, so the same command can run again, and
    /// say so with the failure. A root that was there is never touched.
    fn undo(&self, failure: Failure) -> Failure {
        if self.root_was {
            return failure;
        }
        let lakes = self.dir.join("lakes");
        let _ = std::fs::remove_dir_all(lakes.join(self.name));
        let _ = std::fs::remove_file(lakes.join(format!("{}.json", self.name)));
        let _ = std::fs::remove_file(self.dir.join(root::CONFIG));
        if !self.lakes_was {
            let _ = std::fs::remove_dir(&lakes);
        }
        if !self.dir_was {
            let _ = std::fs::remove_dir(&self.dir);
        }
        let todo = format!(
            "nothing this run made is left in {}; {}",
            self.dir.display(),
            failure.todo
        );
        Failure::new(failure.what, todo)
    }
}

/// The server `lakelands` a `.mcp.json`'s text holds, if it holds one.
fn held(text: &str) -> Option<serde_json::Value> {
    let config = serde_json::from_str::<serde_json::Value>(text).ok()?;
    config["mcpServers"].get(SERVER).cloned()
}

/// Whether a held server runs this command with these arguments.
fn same(there: serde_json::Value, server: &serde_json::Value) -> bool {
    there["command"] == server["command"] && there["args"] == server["args"]
}
