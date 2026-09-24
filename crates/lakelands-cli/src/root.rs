//! The Lakelands root: a directory the operator names, holding the
//! configuration (`lakelands.json`) and one location per lake
//! (`lakes/<id>.json`).
//!
//! Everything the CLI knows comes from these files and its flags; it
//! reads no environment variable.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::id::LakeId;
use lakelands_api::location::{EngineConfig, Location};
use lakelands_api::principal::{Grants, Principal, PrincipalKind};
use lakelands_api::seam::{Credential, EmbedBackend, PrincipalResolver, Seams};
use lakelands_server::HttpConfig;
use lakelands_server::resolver::{Entry, StaticResolver};
use serde::{Deserialize, Serialize};

use crate::say::Failure;

/// The configuration file's name, in the root.
pub const CONFIG: &str = "lakelands.json";

/// The root's configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The configuration format's version.
    pub lakelands: u32,
    /// The id the CLI acts as when no `--as` is given: the lake's owner,
    /// holding every grant.
    pub owner: String,
    /// Other principals, for `--as <name>` and for the server's callers.
    #[serde(default)]
    pub principals: Vec<Entry>,
    /// The HTTP door: where it listens and which browser origins may
    /// call it. Absent until the operator configures a server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<HttpConfig>,
    /// The function backings that reach outside the lake, and what each
    /// may reach: the hosts `http` may call, the commands `exec` may run.
    /// Absent, a lake computes its fields with `expr` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backings: Option<lakelands_backings::Config>,
    /// The embedding client: the endpoint, model and vector length a
    /// hybrid search embeds its text through. Absent, no backend is
    /// bound and search ranks lexically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed: Option<lakelands_backings::EmbedConfig>,
    /// The webhook endpoints `serve` delivers fired triggers and object-set
    /// changes to. Absent, nothing is delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<lakelands_backings::DeliverConfig>,
}

/// An opened root.
pub struct Root {
    /// Its directory, as the operator named it.
    pub dir: PathBuf,
    /// Its configuration.
    pub config: Config,
    /// The embedding client its configuration names, built when the root
    /// was opened.
    embed: Option<Arc<dyn EmbedBackend>>,
}

fn show(p: &Path) -> String {
    p.display().to_string()
}

/// Create a root. Refuses to overwrite an existing one.
pub fn init(dir: &Path, owner: &str) -> Result<(), Failure> {
    let file = dir.join(CONFIG);
    if file.exists() {
        return Err(Failure::new(
            format!(
                "{} already holds a Lakelands root; nothing was changed",
                show(dir)
            ),
            "to start another root, name a directory without a lakelands.json",
        ));
    }
    let config = Config {
        lakelands: 1,
        owner: owner.to_owned(),
        principals: Vec::new(),
        server: None,
        backings: None,
        embed: None,
        deliver: None,
    };
    fs::create_dir_all(dir.join("lakes")).map_err(|e| io(dir, e))?;
    write_new(&file, &config)
}

/// Write a JSON file that must not exist yet.
fn write_new<T: Serialize>(file: &Path, value: &T) -> Result<(), Failure> {
    let mut text = serde_json::to_string_pretty(value).expect("configuration serialises");
    text.push('\n');
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
    {
        Ok(mut f) => std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| io(file, e)),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(Failure::new(
            format!("{} exists already; nothing was changed", show(file)),
            "remove it first if you mean to replace it",
        )),
        Err(e) => Err(io(file, e)),
    }
}

fn io(p: &Path, e: std::io::Error) -> Failure {
    Failure::new(
        format!("could not use {}: {e}", show(p)),
        "check that the path exists and that you may read and write it",
    )
}

impl Root {
    /// What the root plugs into every lake it opens: the function
    /// backings its configuration allows and its embedding client. The
    /// opener adds the engines.
    pub fn seams(&self) -> Seams {
        Seams {
            backings: self
                .config
                .backings
                .as_ref()
                .map(|b| {
                    // `ca_file` is the root's, as the embedding client's is.
                    let mut b = b.clone();
                    if let Some(http) = &mut b.http {
                        http.ca_file = http.ca_file.take().map(|f| self.dir.join(f));
                    }
                    lakelands_backings::backings(&b)
                })
                .unwrap_or_default(),
            embed: self.embed.clone(),
            ..Seams::default()
        }
    }

    /// Open the root in `dir`.
    pub fn open(dir: &Path) -> Result<Root, Failure> {
        let root = Root::open_to_add_a_lake(dir)?;
        root.deliver_to_held_lakes()?;
        Ok(root)
    }

    /// Open the root in `dir` to create a lake in it: as [`Root::open`],
    /// except that a `deliver` endpoint may name the lake not created yet.
    pub fn open_to_add_a_lake(dir: &Path) -> Result<Root, Failure> {
        let file = dir.join(CONFIG);
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(Failure::new(
                    format!("{} is not a Lakelands root (it has no {CONFIG})", show(dir)),
                    format!(
                        "run `lakelands init {}` first, or name the root with --root",
                        show(dir)
                    ),
                ));
            }
            Err(e) => return Err(io(&file, e)),
        };
        let config: Config = serde_json::from_str(&text).map_err(|e| {
            Failure::new(
                format!("{} is not a valid configuration: {e}", show(&file)),
                "correct the file; `lakelands init` writes a valid one to start from",
            )
        })?;
        if let Some(backings) = &config.backings {
            backings.check().map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the backings section of the configuration",
                )
            })?;
        }
        if let Some(deliver) = &config.deliver {
            deliver.check().map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the deliver section of the configuration",
                )
            })?;
        }
        // The client is built now, reading its key file, so a key that
        // cannot be read stops the command before it serves anything.
        // Its paths are the root's, whatever the working directory.
        let embed = match &config.embed {
            None => None,
            Some(c) => {
                let client = lakelands_backings::Embed::new(c.clone(), dir).map_err(|why| {
                    Failure::new(
                        format!("{}: {why}", show(&file)),
                        "fix the embed section of the configuration, or remove it to search \
                         lexically",
                    )
                })?;
                Some(Arc::new(client) as Arc<dyn EmbedBackend>)
            }
        };
        Ok(Root {
            dir: dir.to_owned(),
            config,
            embed,
        })
    }

    fn lake_file(&self, lake: &str) -> PathBuf {
        self.dir.join("lakes").join(format!("{lake}.json"))
    }

    /// Write a new lake's location. Refuses to overwrite one.
    pub fn new_lake(&self, id: &str, engine: &str) -> Result<Location, Failure> {
        let valid = lakelands_core::names::is_lake_id(id);
        if !valid {
            return Err(Failure::new(
                format!("`{id}` cannot name a lake"),
                "use up to 63 lower-case letters, digits, `-` and `_`, starting with a letter or a digit",
            ));
        }
        let location = match engine {
            "sqlite" => Location::sqlite(id, Path::new("lakes").join(id)),
            "memory" => Location::memory(id),
            other => {
                return Err(Failure::new(
                    format!("there is no engine `{other}`"),
                    "choose `sqlite` (kept on disk) or `memory` (gone when the process ends)",
                ));
            }
        };
        let file = self.lake_file(id);
        if file.exists() {
            return Err(Failure::new(
                format!(
                    "lake `{id}` exists already in {}; nothing was changed",
                    show(&self.dir)
                ),
                "choose another name, or keep working with the lake that is there",
            ));
        }
        fs::create_dir_all(self.dir.join("lakes")).map_err(|e| io(&self.dir, e))?;
        write_new(&file, &location)?;
        Ok(location)
    }

    /// Refuse a `deliver` endpoint naming a lake the root does not hold:
    /// a misspelt name would otherwise deliver nothing without a word.
    fn deliver_to_held_lakes(&self) -> Result<(), Failure> {
        let Some(deliver) = &self.config.deliver else {
            return Ok(());
        };
        let held = self.lakes();
        for e in &deliver.endpoints {
            if let Some(lake) = e.lakes.iter().find(|l| !held.contains(l)) {
                return Err(Failure::new(
                    format!(
                        "{}: the endpoint `{}` names the lake `{lake}`, which this root does \
                         not hold",
                        show(&self.dir.join(CONFIG)),
                        e.name
                    ),
                    "fix the deliver section of the configuration, or create the lake with \
                     `lakelands lake new`",
                ));
            }
        }
        Ok(())
    }

    /// Start a deliverer for every endpoint of the `deliver` section that
    /// names a lake in `lakes`, for each lake this process holds as its
    /// writer. Each runs until the process ends. A deliverer that cannot
    /// start (a secret it cannot read, a position file another process
    /// holds) stops the command before it serves anything.
    pub fn deliver(&self, lakes: &[Arc<dyn lakelands_api::LakeStore>]) -> Result<(), Failure> {
        let Some(config) = &self.config.deliver else {
            return Ok(());
        };
        let mut started = std::collections::BTreeSet::new();
        for store in lakes.iter().filter(|s| s.is_writer()) {
            if !started.insert(store.lake().clone()) {
                continue;
            }
            let deliverers = lakelands_backings::deliver::deliverers(
                config,
                &self.dir,
                store.lake(),
            )
            .map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&self.dir.join(CONFIG))),
                    "fix the deliver section of the configuration, or stop the other process",
                )
            })?;
            for d in deliverers {
                tokio::spawn(d.run(store.clone()));
            }
        }
        Ok(())
    }

    /// The lakes this root holds, by name.
    pub fn lakes(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.dir.join("lakes"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_suffix(".json").map(str::to_owned)
            })
            .collect();
        out.sort();
        out
    }

    /// The location of the lake named, or of the only lake there is.
    /// A relative directory in it is taken relative to the root.
    pub fn location(&self, lake: Option<&str>) -> Result<Location, Failure> {
        let lake = match lake {
            Some(l) => l.to_owned(),
            None => {
                let lakes = self.lakes();
                match lakes.as_slice() {
                    [one] => one.clone(),
                    [] => {
                        return Err(Failure::new(
                            format!("{} holds no lake yet", show(&self.dir)),
                            "create one with `lakelands lake new <id>`",
                        ));
                    }
                    many => {
                        return Err(Failure::new(
                            format!("{} holds several lakes", show(&self.dir)),
                            format!("name one with --lake: {}", many.join(", ")),
                        ));
                    }
                }
            }
        };
        let file = self.lake_file(&lake);
        let text = fs::read_to_string(&file).map_err(|e| match e.kind() {
            ErrorKind::NotFound => Failure::new(
                format!("there is no lake `{lake}` in {}", show(&self.dir)),
                "check the name, or create the lake with `lakelands lake new`",
            ),
            _ => io(&file, e),
        })?;
        let mut location: Location = serde_json::from_str(&text).map_err(|e| {
            Failure::new(
                format!("{} is not a valid location: {e}", show(&file)),
                "correct the file, or remove it and run `lakelands lake new` again",
            )
        })?;
        if let EngineConfig::Sqlite { dir, .. } = &mut location.engine
            && dir.is_relative()
        {
            *dir = self.dir.join(&*dir);
        }
        Ok(location)
    }

    /// The principal the CLI acts as in a lake: the owner, or the entry
    /// `--as` names, resolved by the default resolver.
    pub async fn principal(
        &self,
        lake: &LakeId,
        act_as: Option<&str>,
    ) -> Result<Principal, Failure> {
        let Some(name) = act_as else {
            return Ok(Principal {
                kind: PrincipalKind::Human,
                id: self.config.owner.clone(),
                lake: lake.clone(),
                grants: Grants::owner(),
            });
        };
        StaticResolver::new(self.config.principals.clone())
            .resolve(lake, &Credential::LocalUser(name.to_owned()))
            .await
            .map_err(|_| {
                Failure::new(
                    format!("no principal named `{name}` may address lake `{lake}`"),
                    format!(
                        "add an entry for it to {CONFIG}, or leave out --as to act as the owner"
                    ),
                )
            })
    }
}
