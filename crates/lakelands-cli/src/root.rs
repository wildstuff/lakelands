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

use lakelands_api::engine::OpenMode;
use lakelands_api::id::LakeId;
use lakelands_api::location::{EngineConfig, Location, PgTlsMode};
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
    /// The ticker: the UTC offset `serve` reads the date at, and how
    /// often it looks whether the date rolled over, to age each lake it
    /// writes once a day. Absent, `serve` ages nothing, and aging is
    /// `lakelands age --today` or the host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aging: Option<crate::ticker::AgingConfig>,
    /// The mirrors: which types of which lake `serve` copies into which
    /// other lake of this root, read as which principal. Absent, nothing
    /// is mirrored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<crate::mirror::MirrorConfig>,
    /// The key store that seals a source's values of erasable fields:
    /// `lakelands-backings`' file keyring, one file per lake in a directory
    /// outside every lake's. Absent, no key store is bound, and a value
    /// that would be sealed is refused
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<KeysConfig>,
    /// Where `lake new` makes a SQLite lake's directory, relative to the
    /// root where it is relative: `lakes` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lakes_dir: Option<PathBuf>,
    /// Where `export` writes when it is given no `--to`, as `lakes_dir`:
    /// `exports` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exports_dir: Option<PathBuf>,
    /// Where backups are to be kept, as `lakes_dir`: `backups` when
    /// absent. Nothing writes there yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backups_dir: Option<PathBuf>,
}

/// Where the root's key store keeps its keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeysConfig {
    /// The keyring's directory, relative to the root where it is relative.
    pub keyring: PathBuf,
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
        aging: None,
        mirror: None,
        keys: None,
        lakes_dir: None,
        exports_dir: None,
        backups_dir: None,
    };
    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    write_new(&file, &config)?;
    fs::create_dir_all(dir.join("lakes")).map_err(|e| io(dir, e))
}

/// `path` with its links followed and its `.` and `..` taken out: its
/// longest part that exists, canonical, and the rest as named, so a
/// keyring not made yet is judged where it will be.
fn canonical(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut at = path.to_path_buf();
    let mut out = loop {
        if let Ok(real) = at.canonicalize() {
            break real;
        }
        match at.components().next_back() {
            Some(last) => rest.push(last.as_os_str().to_owned()),
            None => break at.clone(),
        }
        if !at.pop() {
            break PathBuf::new();
        }
    };
    for part in rest.into_iter().rev() {
        match part.to_str() {
            Some("..") => {
                out.pop();
            }
            Some(".") => {}
            _ => out.push(part),
        }
    }
    out
}

/// Write a JSON file that must not exist yet.
fn write_new<T: Serialize>(file: &Path, value: &T) -> Result<(), Failure> {
    let mut text = serde_json::to_string_pretty(value).expect("configuration serialises");
    text.push('\n');
    let mut open = fs::OpenOptions::new();
    open.write(true).create_new(true);
    // A location may hold a database password: its operator's alone.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    match open.open(file) {
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

/// What `lake new --engine postgres` is given: the database's URL, which
/// the location file then holds in plain text, and its schema and TLS.
#[derive(Default)]
pub struct PgNew {
    pub url: Option<String>,
    pub schema: Option<String>,
    pub tls_disabled: bool,
    pub ca_file: Option<PathBuf>,
}

impl PgNew {
    /// Whether any member was given.
    fn given(&self) -> bool {
        self.url.is_some() || self.schema.is_some() || self.tls_disabled || self.ca_file.is_some()
    }

    /// The location of lake `id` in PostgreSQL, from what was given.
    fn location(&self, id: &str) -> Result<Location, Failure> {
        if !cfg!(feature = "pg") {
            return Err(Failure::new(
                "this build of lakelands has no PostgreSQL engine",
                "build it with the `pg` feature, or choose `sqlite` or `memory`",
            ));
        }
        let Some(url) = &self.url else {
            return Err(Failure::new(
                "a PostgreSQL lake needs `--url`",
                "name the database, as `--url postgres://user@host/database`; the location file holds the URL, password included, as plain text",
            ));
        };
        if self.tls_disabled && self.ca_file.is_some() {
            return Err(Failure::new(
                "`--no-tls` and `--ca-file` contradict each other",
                "a CA file verifies a TLS connection: leave one of them out",
            ));
        }
        let mut location = Location::postgres(id, url.clone());
        if let EngineConfig::Postgres { schema, tls, .. } = &mut location.engine {
            schema.clone_from(&self.schema);
            if self.tls_disabled {
                tls.mode = PgTlsMode::Disable;
            }
            tls.ca_file.clone_from(&self.ca_file);
        }
        Ok(location)
    }
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
            // `sys.mirror`, for the lakes a mirror writes into.
            vocabulary: self.config.mirror.as_ref().map(|m| {
                Arc::new(crate::mirror::MirrorVocabulary::of(m))
                    as Arc<dyn lakelands_api::seam::SystemVocabulary>
            }),
            keys: self.keyring().map(|dir| {
                Arc::new(lakelands_backings::keyring::FileKeyring::new(dir))
                    as Arc<dyn lakelands_api::seam::KeyStore>
            }),
            ..Seams::default()
        }
    }

    /// Where `export` writes lake `lake` when it is given no `--to`: a
    /// directory of its own in `exports_dir`, named by the time, in UTC.
    pub fn export_dir(&self, lake: &LakeId) -> PathBuf {
        use crate::ticker::Clock as _;
        let now = crate::ticker::SystemClock.now().to_string();
        let stamp: String = (now[..19].chars())
            .filter(|c| *c != '-' && *c != ':')
            .collect();
        let exports = self.config.exports_dir.as_deref();
        let exports = self.dir.join(exports.unwrap_or(Path::new("exports")));
        exports.join(format!("{lake}-{stamp}Z"))
    }

    /// The keyring's directory, where `keys` names one.
    fn keyring(&self) -> Option<PathBuf> {
        let named = &self.config.keys.as_ref()?.keyring;
        Some(match named.is_relative() {
            true => self.dir.join(named),
            false => named.clone(),
        })
    }

    /// A keyring inside a lake's directory, or its projection's, would go
    /// wherever the lake's files go, a backup or a copy, and a destroyed key
    /// with them: it is refused (058).
    fn keyring_outside_the_lakes(&self) -> Result<(), Failure> {
        let Some(keyring) = self.keyring() else {
            return Ok(());
        };
        let keyring = canonical(&keyring);
        if let Some(lake) = self.lake_holding(&keyring)? {
            return Err(Failure::new(
                format!(
                    "the keyring {} is inside the lake `{lake}`'s directory",
                    show(&keyring)
                ),
                "name a keyring directory outside every lake's, in the keys section of the configuration",
            ));
        }
        Ok(())
    }

    /// The lake whose directory holds `path`, canonical, if one does.
    fn lake_holding(&self, path: &Path) -> Result<Option<String>, Failure> {
        for lake in self.lakes() {
            let EngineConfig::Sqlite {
                dir,
                projection_dir,
                ..
            } = self.location(Some(&lake))?.engine
            else {
                continue;
            };
            if std::iter::once(dir)
                .chain(projection_dir)
                .any(|dir| path.starts_with(canonical(&dir)))
            {
                return Ok(Some(lake));
            }
        }
        Ok(None)
    }

    /// The root's file keyring, where `keys` names one.
    pub fn file_keyring(&self) -> Option<lakelands_backings::keyring::FileKeyring> {
        self.keyring()
            .map(lakelands_backings::keyring::FileKeyring::new)
    }

    /// A copy of a lake's keys is kept apart from its backup, neither
    /// directory inside the other, and outside every lake's directory: a
    /// backup that holds both holds what forgetting destroys
    /// (`docs/decisions/077-a-backup-is-an-export-that-grows.md`).
    pub fn keys_apart(&self, keys: &Path, backup: &Path) -> Result<(), Failure> {
        let (keys, backup) = (canonical(keys), canonical(backup));
        let todo = "keep the keys' copy in a directory of its own, apart from the backup and \
                    every lake's directory";
        if keys.starts_with(&backup) || backup.starts_with(&keys) {
            let what = format!(
                "the keys' directory {} and the backup's {} are one inside the other",
                show(&keys),
                show(&backup)
            );
            return Err(Failure::new(what, todo));
        }
        self.outside_the_lakes(&keys, "keys'")
    }

    /// A backup, and a copy of a lake's keys, are written outside every
    /// lake's directory: a run into one would write into the files its
    /// writer appends to.
    pub fn outside_the_lakes(&self, dir: &Path, whose: &str) -> Result<(), Failure> {
        let dir = canonical(dir);
        if let Some(lake) = self.lake_holding(&dir)? {
            let what = format!(
                "the {whose} directory {} is inside the lake `{lake}`'s directory",
                show(&dir)
            );
            let todo = "name a directory of its own, outside every lake's directory";
            return Err(Failure::new(what, todo));
        }
        Ok(())
    }

    /// Open the root in `dir`.
    pub fn open(dir: &Path) -> Result<Root, Failure> {
        let root = Root::open_to_add_a_lake(dir)?;
        root.keyring_outside_the_lakes()?;
        root.deliver_to_held_lakes()?;
        root.mirror_held_lakes()?;
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
        if let Some(server) = &config.server {
            server.check(&config.principals).map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the server section of the configuration: give each host keys of its \
                     own",
                )
            })?;
        }
        if let Some(backings) = &config.backings {
            backings.check().map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the backings section of the configuration",
                )
            })?;
        }
        if let Some(aging) = &config.aging {
            aging.check().map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the aging section of the configuration",
                )
            })?;
        }
        if let Some(mirror) = &config.mirror {
            mirror.check().map_err(|why| {
                Failure::new(
                    format!("{}: {why}", show(&file)),
                    "fix the mirror section of the configuration",
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
    pub fn new_lake(&self, id: &str, engine: &str, pg: &PgNew) -> Result<Location, Failure> {
        let valid = lakelands_core::names::is_lake_id(id);
        if !valid {
            return Err(Failure::new(
                format!("`{id}` cannot name a lake"),
                "use up to 63 lower-case letters, digits, `-` and `_`, starting with a letter or a digit",
            ));
        }
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
        if engine != "postgres" && pg.given() {
            return Err(Failure::new(
                format!(
                    "`--url`, `--schema`, `--no-tls` and `--ca-file` are for `postgres`, not `{engine}`"
                ),
                "leave them out, or choose `--engine postgres`",
            ));
        }
        let location = match engine {
            "sqlite" => {
                let lakes = self.config.lakes_dir.as_deref();
                Location::sqlite(id, lakes.unwrap_or(Path::new("lakes")).join(id))
            }
            "memory" => Location::memory(id),
            "postgres" => pg.location(id)?,
            other => {
                return Err(Failure::new(
                    format!("there is no engine `{other}`"),
                    "choose `sqlite` (kept on disk), `postgres` (in a database) or `memory` (gone when the process ends)",
                ));
            }
        };
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

    /// Refuse a mirror connection naming a lake the root does not hold or
    /// a principal it does not list: either would mirror nothing without a
    /// word.
    fn mirror_held_lakes(&self) -> Result<(), Failure> {
        let Some(mirror) = &self.config.mirror else {
            return Ok(());
        };
        let held = self.lakes();
        let fix = "fix the mirror section of the configuration, or create the lake with \
                   `lakelands lake new`";
        for c in &mirror.connections {
            for lake in [&c.source, &c.target] {
                if !held.contains(lake) {
                    return Err(Failure::new(
                        format!(
                            "{}: the connection `{}` names the lake `{lake}`, which this root \
                             does not hold",
                            show(&self.dir.join(CONFIG)),
                            c.name
                        ),
                        fix,
                    ));
                }
            }
            let entry = self
                .config
                .principals
                .iter()
                .find(|p| p.name == c.principal);
            if let Some(entry) = entry {
                crate::mirror::may_keep_position(&entry.grants).map_err(|why| {
                    Failure::new(
                        format!(
                            "{}: the connection `{}` reads as `{}`: {why}",
                            show(&self.dir.join(CONFIG)),
                            c.name,
                            c.principal
                        ),
                        "grant the principal write on `sys.mirror` in the configuration",
                    )
                })?;
            }
            if entry.is_none() {
                return Err(Failure::new(
                    format!(
                        "{}: the connection `{}` reads as `{}`, which is not one of the root's \
                         principals",
                        show(&self.dir.join(CONFIG)),
                        c.name,
                        c.principal
                    ),
                    "add the principal to the configuration, with the grants the mirror needs \
                     in both lakes",
                ));
            }
        }
        Ok(())
    }

    /// Start a mirror for every connection whose target lake is among
    /// `lakes`, opening its source lake to read when this process does not
    /// hold it yet. Each runs until the process ends. A connection whose
    /// target is held here but not as its writer, or that cannot start
    /// (see `Mirror::new`), stops the command before it serves anything.
    pub async fn mirror(
        &self,
        lakes: &[Arc<dyn lakelands_api::LakeStore>],
        open: &crate::Opener<'_>,
    ) -> Result<(), Failure> {
        let Some(config) = &self.config.mirror else {
            return Ok(());
        };
        let fix = "fix the mirror section of the configuration, the principal's grants or the \
                   target type's declaration, or stop the other process";
        for c in &config.connections {
            let Some(target) = lakes.iter().find(|s| s.lake().as_str() == c.target) else {
                continue;
            };
            let source = match lakes.iter().find(|s| s.lake().as_str() == c.source) {
                Some(s) => s.clone(),
                None => {
                    let location = self.location(Some(&c.source))?;
                    open(location, OpenMode::Read, self.seams())
                        .await
                        .map_err(|e| {
                            Failure::new(
                                format!(
                                    "the connection `{}` cannot open its source lake `{}`: {e}",
                                    c.name, c.source
                                ),
                                "check the lake's location",
                            )
                        })?
                }
            };
            let reader = self.principal(source.lake(), Some(&c.principal)).await?;
            let writer = self.principal(target.lake(), Some(&c.principal)).await?;
            let mirror = crate::mirror::Mirror::new(
                config,
                c,
                &self.dir,
                source,
                target.clone(),
                reader,
                writer,
            )
            .await
            .map_err(|why| Failure::new(format!("{}: {why}", show(&self.dir.join(CONFIG))), fix))?;
            tokio::spawn(mirror.run());
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
        if let EngineConfig::Sqlite {
            dir,
            projection_dir,
            ..
        } = &mut location.engine
        {
            for dir in std::iter::once(dir).chain(projection_dir) {
                *dir = self.dir.join(&*dir);
            }
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
