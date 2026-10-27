//! Every setting that steers a lake, in one list
//! (`docs/decisions/063-every-setting-is-in-one-list.md`, lakelands#1172).
//! A setting lives where what it steers lives:
//! - **lake:** a `sys.setting` record, in the log, judged by
//!   [`check_setting`](crate::seam::check_setting): what every host that
//!   opens the lake must find the same;
//! - **location:** the lake's `Location`, fixed at open: a bound on one
//!   host's work or resources, which changes no answer;
//! - **root:** the root's `lakelands.json`: how a process that opens the
//!   root serves, ages, delivers and embeds.
//!
//! What only says where or how to connect (a directory, a URL, a file of
//! keys) is a binding, not a setting: [`BINDINGS`] names each.
//!
//! A host's own keys under `sys.setting`, read by its expressions as
//! `setting('…')`, are its own and not listed. `docs/settings.md` is
//! generated from this list (`scripts/settings.py`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Value;
use crate::describe::SettingValue;

/// The group of the store's own lake settings: a grant's `only` on
/// `sys.setting` names it, and no host's setting is in it.
pub const STORE_GROUP: &str = "store";

/// Where a setting is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// A `sys.setting` record of the lake, in its log.
    Lake,
    /// The lake's location, `lakes/<id>.json`.
    Location,
    /// The root's configuration, `lakelands.json`, read when a process
    /// opens the root: `server.*` when `serve` starts.
    Root,
}

/// Whether a lake setting is read for a type, or for a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerType {
    /// One value for the lake.
    No,
    /// One value per type, `<key>.<type>`, and none for the lake.
    Only,
    /// One for the lake, `<key>`, which `<key>.<type>` overrides for a type.
    Override,
    /// One for the lake, `<key>`, which `<key>.<source>` overrides for a
    /// source.
    Source,
}

/// One setting: its key, where it is kept, its default as JSON, what
/// values it takes, what it changes, and what it costs where measured.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    /// The key: a lake setting's, or a location or server member's path.
    pub key: &'static str,
    /// Where it is kept.
    pub layer: Layer,
    /// Whether it is read for a type.
    pub per_type: PerType,
    /// Its value where none is set, as JSON.
    pub default: &'static str,
    /// What values it takes, in an operator's words.
    pub range: &'static str,
    /// What it changes, in one sentence.
    pub says: &'static str,
    /// What it costs, measured, or empty.
    pub cost: &'static str,
}

const fn lake(key: &'static str, per_type: PerType, default: &'static str) -> Setting {
    at(Layer::Lake, key, per_type, default)
}

const fn at(layer: Layer, key: &'static str, per_type: PerType, default: &'static str) -> Setting {
    Setting {
        key,
        layer,
        per_type,
        default,
        range: "",
        says: "",
        cost: "",
    }
}

const fn location(key: &'static str, default: &'static str) -> Setting {
    at(Layer::Location, key, PerType::No, default)
}

const fn root(key: &'static str, default: &'static str) -> Setting {
    at(Layer::Root, key, PerType::No, default)
}

/// Every setting, lake first, then location, then root.
pub const SETTINGS: &[Setting] = &[
    Setting {
        range: "a list of the type's fields of a kind an order index keeps: text, number, decimal, boolean, date-time, date or reference",
        says: "Keeps an order index on these fields, so a page ordered by one of them reads its page instead of sorting the type.",
        cost: "each field indexed costs a write about 1–5 % on SQLite and 11 % on PostgreSQL; naming one rebuilds the type's rows while holding the writer, about 0.85 s at 100 000 records on SQLite and 2.4 s on PostgreSQL",
        ..lake("sort_index", PerType::Only, "null")
    },
    Setting {
        range: "a list of the type's vector fields",
        says: "Keeps a vector index on these fields; a search on one answers the index's set.",
        ..lake("vector_index", PerType::Only, "null")
    },
    Setting {
        range: "a list of the type's text fields",
        says: "Keeps an exact-text index on these fields beyond those the declarations look up by, so a read comparing one to a text reads its matches.",
        ..lake("lookup_index", PerType::Only, "null")
    },
    Setting {
        range: "an object of `fields` (vector fields with their weights), `text_fields` (searchable text fields with their weights) and `text_weight`, one of them at least",
        says: "Weighs the type's rankings in a search that gives a ranking no weight, and a match of its text by the field it is in.",
        ..lake("search_weights", PerType::Only, "null")
    },
    Setting {
        range: "`true` or `false`",
        says: "Keeps the view's records as its sources are written, so a read of the view reads them instead of computing it.",
        ..lake("view_kept", PerType::Only, "false")
    },
    Setting {
        range: "`day`, `week` (from Monday) or `month`, in UTC, or null for none",
        says: "Seals a source's values of each period under a version of its key of their own, made at the first value past the period's start, so a cut at a period's end can destroy the versions before it.",
        cost: "one new version of the source's key per period, made by the key store, so `day` makes 365 a year for each source; the file keyring rewrites its whole file at each, measured at 4.5 ms with 1 000 keys and 8.9 ms with 10 000; and the list of a source's versions, an entry a period, read once a window, so ten years of `day` (3 650 entries) add about 20 µs a record to an ingest on the memory engine (lakelands#1153)",
        ..lake("key_period", PerType::Source, "null")
    },
    Setting {
        range: "a count, at most `max_limit`",
        says: "How many rows a list-shaped read returns when the caller names no limit.",
        ..location("limits.default_limit", "50")
    },
    Setting {
        range: "a count",
        says: "The largest `limit` a caller may ask for.",
        ..location("limits.max_limit", "1000")
    },
    Setting {
        range: "a count",
        says: "The deepest a pattern step may walk.",
        ..location("limits.max_depth", "16")
    },
    Setting {
        range: "a count",
        says: "The deepest a chain of reactive writes may go.",
        ..location("limits.max_reactive_depth", "8")
    },
    Setting {
        range: "a count, 1 at least",
        says: "The most records one group-commit window of an ingest holds, or events one batch of record writes holds; a batch closes after the write that reaches it. A host with bulk deliveries of hundreds or thousands of records may raise it to 1024, or a delivery ask its own (`commit_window`), up to `limits.commit_window_max`.",
        cost: "each window pays one log append and one projection commit, about 8 ms on Apple Silicon, so a delivery of 500 records takes 200 ms at 32 and 72 ms at 1024, and one of 1000 records 390 ms and 130 ms (SQLite, through a host's write door, `ingest_with`, sealed apart); a refused record rolls back its whole window, up to 255 records before it at the default and up to 1023 at 1024, and the crash check on open reads the last group, about 14 µs an event (decision 060): about 3.5 ms at the default, 14 ms at 1000",
        ..location("limits.commit_window_events", "256")
    },
    Setting {
        range: "a count",
        says: "The largest commit window an ingest may ask for its own delivery (`commit_window`); a larger one, or 0, is refused, naming the option and this ceiling. A lake whose location sets `limits.commit_window_events` above it is not opened.",
        ..location("limits.commit_window_max", "4096")
    },
    Setting {
        range: "a count",
        says: "The most records a whole-graph measure may reach; past it the measure is refused, never cut.",
        ..location("limits.max_graph_nodes", "100000")
    },
    Setting {
        range: "a count",
        says: "The most rows a read may skip with `offset`; past it the read is refused, never cut.",
        ..location("limits.max_offset", "100000")
    },
    Setting {
        range: "a count",
        says: "The most memberships one declaration of an object set may change over the records already there.",
        ..location("limits.max_set_changes", "20000")
    },
    Setting {
        range: "a count",
        says: "The most records one change to records may select; past it `plan` and `enact` refuse it, naming this setting, and never cut it.",
        cost: "a change is enacted in one append that holds the writer alone: at 10 000 records the enact took 0.5–0.7 s on the memory engine, 0.7–0.8 s on SQLite and 4.3–4.8 s on PostgreSQL, its plan about a third of that",
        ..location("limits.max_change_records", "10000")
    },
    Setting {
        range: "a count, 1 at least",
        says: "The largest `top` a search's `rerank` may ask for; a larger one is refused, naming this setting. A lake whose location sets 0 is not opened.",
        ..location("limits.max_rerank_top", "100")
    },
    Setting {
        range: "`true` or `false`",
        says: "Whether the lake chooses an order index itself, for a field a type is ordered by whole at least `limits.order_after_pages` times within `limits.order_window_secs`, beside what `sort_index` names; the host's own `sort_index` for a type always wins. Off, only what the host sets is indexed.",
        cost: "each chosen index costs a single upsert about 0–2 % on SQLite and about 20 % on PostgreSQL, and `limits.order_max_fields` (2 by default) bounds how many; it is built beside the writer in chunks of `limits.order_chunk` records, and a write sent meanwhile waited at most 160 ms on SQLite and 320 ms on PostgreSQL at 100 000 records",
        ..location("limits.choose_order_index", "true")
    },
    Setting {
        range: "a count",
        says: "How many pages ordered whole by one field of a type, within `limits.order_window_secs`, make the lake choose an index for it.",
        ..location("limits.order_after_pages", "3")
    },
    Setting {
        range: "seconds",
        says: "The window the pages that make the lake choose an index are counted in.",
        ..location("limits.order_window_secs", "600")
    },
    Setting {
        range: "a count",
        says: "The fewest live records a type holds for the lake to choose an index of its fields.",
        ..location("limits.order_min_records", "2000")
    },
    Setting {
        range: "a count",
        says: "The most fields of one type the lake chooses an index for.",
        ..location("limits.order_max_fields", "2")
    },
    Setting {
        range: "seconds",
        says: "How long a chosen index may go unused, in seconds of the writer's life, before the lake drops it.",
        ..location("limits.order_unused_secs", "604800")
    },
    Setting {
        range: "a count; 0 is taken as 1",
        says: "The records one chunk of a chosen index's build writes while it holds the writer; a write waits for a chunk, never for the build.",
        ..location("limits.order_chunk", "10000")
    },
    Setting {
        range: "`true` or `false`",
        says: "Whether a forgetting always rebuilds the projection; off, it refolds only the records its sources wrote where nothing else the lake keeps reads them, and the preflight says which it chose.",
        ..location("limits.forget_rebuild", "false")
    },
    Setting {
        range: "bytes: 0 never seals by size, and a bound below 1048576 (1 MiB) is read as 1 MiB",
        says: "The active segment's size past which the writer seals it after an append, as `checkpoint` does, on an engine that reports the size (SQLite); a writer's open seals one already past it. PostgreSQL and memory never seal by size.",
        cost: "each seal holds the writer as a `checkpoint` does, about 2 ms a MiB of the segment: the write that crosses 256 MiB waited 0.51 s on SQLite, against 16 ms for a write of 1 MiB; every checkpoint lists each sealed segment, so a small bound makes many seals and long checkpoints",
        ..location("limits.seal_after_bytes", "268435456")
    },
    Setting {
        range: "KiB",
        says: "SQLite: the writer connection's page cache.",
        ..location("engine.tuning.writer_cache_kb", "65536")
    },
    Setting {
        range: "KiB",
        says: "SQLite: each reader connection's page cache. A handle keeps up to one reader per core open, at most 16: up to 16 times this (256 MiB at the default).",
        ..location("engine.tuning.reader_cache_kb", "16384")
    },
    Setting {
        range: "MiB, 0 for none",
        says: "SQLite: the projection file mapped into memory per connection.",
        ..location("engine.tuning.mmap_mb", "256")
    },
    Setting {
        range: "pages",
        says: "SQLite: how far the write-ahead journal grows before it is folded back into the projection file.",
        ..location("engine.tuning.wal_autocheckpoint_pages", "20000")
    },
    Setting {
        range: "seconds",
        says: "PostgreSQL: how long opening a connection may take.",
        ..location("engine.tuning.connect_timeout_secs", "10")
    },
    Setting {
        range: "seconds, 0 for no limit",
        says: "PostgreSQL: how long one statement may run.",
        ..location("engine.tuning.statement_timeout_secs", "0")
    },
    Setting {
        range: "an address and port",
        says: "Where the server listens; it has no default and must be given.",
        ..root("server.bind", "null")
    },
    Setting {
        range: "a list of origins",
        says: "The origins a browser may call from; a request from any other is refused.",
        ..root("server.allowed_origins", "[]")
    },
    Setting {
        range: "seconds",
        says: "A session unused this long ends.",
        ..root("server.session_idle_secs", "1800")
    },
    Setting {
        range: "milliseconds",
        says: "How long a caught-up feed or MCP session waits for the lake's next append before it reads the log again.",
        ..root("server.feed_poll_millis", "250")
    },
    Setting {
        range: "milliseconds",
        says: "How often, at most, the log is read for the feeds that follow one lake with one filter.",
        ..root("server.feed_floor_millis", "10")
    },
    Setting {
        range: "a count above zero",
        says: "The threads `serve` runs its doors and sessions on; absent, the machine's cores, at most 8.",
        ..root("server.workers", "null")
    },
    Setting {
        range: "an address and port",
        says: "Where `serve` answers `GET /metrics` in the Prometheus text format, apart from the door; absent, it measures nothing and listens for none.",
        cost: "a write on the memory engine took 22.6 µs measured against 22.3 µs unmeasured with one caller, and 12.6 µs either way with sixteen; on SQLite the difference is inside a commit's noise; a scrape reads each lake's counters and takes nothing from its writer",
        ..root("server.metrics.bind", "null")
    },
    Setting {
        range: "`Z`, or `+HH:MM` or `-HH:MM` within fourteen hours",
        says: "The UTC offset `serve` reads the date at, to age each lake it writes once a day.",
        ..root("aging.utc_offset", "\"Z\"")
    },
    Setting {
        range: "seconds",
        says: "How often `serve` looks whether the date rolled over.",
        ..root("aging.check_secs", "60")
    },
    Setting {
        range: "milliseconds",
        says: "How long a deliverer that has caught up waits before it reads the lake again.",
        ..root("deliver.poll_millis", "1000")
    },
    Setting {
        range: "a count",
        says: "The most events a deliverer reads at once.",
        ..root("deliver.batch", "100")
    },
    Setting {
        range: "seconds",
        says: "The first wait before a delivery that failed for a while is tried again; it doubles with every attempt.",
        ..root("deliver.retry_first_secs", "1")
    },
    Setting {
        range: "seconds",
        says: "The longest wait between two attempts of a delivery.",
        ..root("deliver.retry_max_secs", "300")
    },
    Setting {
        range: "seconds",
        says: "How long one delivery may take, connecting included.",
        ..root("deliver.timeout_secs", "10")
    },
    Setting {
        range: "bytes",
        says: "The largest reply a deliverer reads from an endpoint; the reply is dropped.",
        ..root("deliver.max_reply_bytes", "65536")
    },
    Setting {
        range: "a model's name",
        says: "The embedding model, sent with every request and recorded as the procedure of every vector; it has no default and must be given.",
        ..root("embed.model", "null")
    },
    Setting {
        range: "a count",
        says: "The length of every vector the embedding endpoint answers; it has no default and must be given.",
        ..root("embed.dimensions", "null")
    },
    Setting {
        range: "`true` or `false`",
        says: "Whether the embedding client asks the endpoint for vectors of `dimensions` length, for endpoints that shorten them on request.",
        ..root("embed.send_dimensions", "false")
    },
    Setting {
        range: "a count",
        says: "The most texts sent to the embedding endpoint in one request.",
        ..root("embed.max_batch", "64")
    },
    Setting {
        range: "seconds",
        says: "How long one embedding request may take, connecting included.",
        ..root("embed.timeout_secs", "30")
    },
    Setting {
        range: "bytes",
        says: "The largest reply the embedding client accepts.",
        ..root("embed.max_reply_bytes", "16777216")
    },
    Setting {
        range: "seconds",
        says: "How long one call of the `http` backing may take, connecting included.",
        ..root("backings.http.timeout_secs", "10")
    },
    Setting {
        range: "bytes",
        says: "The largest reply body the `http` backing accepts; a larger one fails the call.",
        ..root("backings.http.max_reply_bytes", "1048576")
    },
    Setting {
        range: "seconds",
        says: "How long one run of a command of the `exec` backing may take.",
        ..root("backings.exec.timeout_secs", "10")
    },
    Setting {
        range: "bytes",
        says: "The largest output the `exec` backing accepts on a command's standard output; a larger one fails the call.",
        ..root("backings.exec.max_output_bytes", "1048576")
    },
    Setting {
        range: "milliseconds",
        says: "The longest a mirror that has caught up waits for its source's next append, and how long it waits after a round that failed.",
        ..root("mirror.poll_millis", "1000")
    },
    Setting {
        range: "a count",
        says: "The most events a mirror reads from its source's feed at once, and the most records per page of a snapshot.",
        ..root("mirror.batch", "100")
    },
    Setting {
        range: "milliseconds",
        says: "How often a `serve` that stands by asks whether each lake's lease is free, to take it once the writer is gone; absent, `serve` takes each lease at its open.",
        ..root("standby.try_millis", "1000")
    },
];

/// What says only where or how to connect: kept beside the settings, at
/// its layer, and no setting.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// The member's path.
    pub key: &'static str,
    /// Where it is kept.
    pub layer: Layer,
    /// What it binds, in one sentence.
    pub says: &'static str,
}

/// Every binding, location first, then root.
pub const BINDINGS: &[Binding] = &[
    Binding {
        key: "engine.dir",
        layer: Layer::Location,
        says: "SQLite: the directory that holds the lake's log segments, and its projection where `engine.projection_dir` is absent.",
    },
    Binding {
        key: "engine.projection_dir",
        layer: Layer::Location,
        says: "SQLite: the directory that holds the lake's projection and the one a rebuild builds beside it, one lake's alone.",
    },
    Binding {
        key: "engine.url",
        layer: Layer::Location,
        says: "PostgreSQL: the database the lake is kept in, as a `postgres://` URL.",
    },
    Binding {
        key: "engine.schema",
        layer: Layer::Location,
        says: "PostgreSQL: the schema that holds the lake; `lakelands_<lake>` when absent.",
    },
    Binding {
        key: "engine.tls",
        layer: Layer::Location,
        says: "PostgreSQL: how the connection is secured.",
    },
    Binding {
        key: "engine.tablespaces",
        layer: Layer::Location,
        says: "PostgreSQL: the tablespaces the lake's `log`, `projection`, `indexes` and `vectors` are made in, each the database's default where it is absent.",
    },
    Binding {
        key: "key",
        layer: Layer::Location,
        says: "A handle naming the key the lake is encrypted with, which the host resolves.",
    },
    Binding {
        key: "writers",
        layer: Layer::Location,
        says: "The REST doors of the processes that may hold the lake's writer lease, the writer first and then its standbys, each a `url` and an optional `ca_file`: where a handle that does not hold the lease forwards a write, the only places it sends one.",
    },
    Binding {
        key: "keys.keyring",
        layer: Layer::Root,
        says: "The directory of the key store that seals a source's values of erasable fields.",
    },
    Binding {
        key: "deliver.endpoints",
        layer: Layer::Root,
        says: "The endpoints `serve` delivers fired triggers and object-set changes to, the only places a deliverer sends anything.",
    },
    Binding {
        key: "deliver.ca_file",
        layer: Layer::Root,
        says: "The certificate authorities a deliverer trusts over `https`.",
    },
    Binding {
        key: "embed.endpoint",
        layer: Layer::Root,
        says: "The embedding endpoint's URL, the only place the embedding client sends anything.",
    },
    Binding {
        key: "embed.api_key_file",
        layer: Layer::Root,
        says: "A file holding the key the embedding client sends.",
    },
    Binding {
        key: "embed.ca_file",
        layer: Layer::Root,
        says: "The certificate authorities the embedding client trusts over `https`.",
    },
    Binding {
        key: "backings.http.hosts",
        layer: Layer::Root,
        says: "The hosts a function's URL may name, the only places the `http` backing sends anything.",
    },
    Binding {
        key: "backings.http.ca_file",
        layer: Layer::Root,
        says: "The certificate authorities the `http` backing trusts over `https`.",
    },
    Binding {
        key: "backings.exec.commands",
        layer: Layer::Root,
        says: "The commands a function may name, each a program and its arguments, the only ones the `exec` backing runs.",
    },
    Binding {
        key: "mirror.connections",
        layer: Layer::Root,
        says: "Each mirror's source lake, target lake, principal, the types that cross, and where a type with no position begins (`start`: a snapshot of its records, or the feed from now).",
    },
    Binding {
        key: "server.hosts",
        layer: Layer::Root,
        says: "The other hosts that may forward a call to this server's doors, each an `id`, its `keys`, the `lakes` it may forward to (every lake when empty) and the `principals` it may act for (every one the lake knows when absent).",
    },
    Binding {
        key: "lakes_dir",
        layer: Layer::Root,
        says: "Where `lake new` makes a SQLite lake's directory; `lakes` in the root when absent.",
    },
    Binding {
        key: "exports_dir",
        layer: Layer::Root,
        says: "Where `export` writes when it is given no `--to`; `exports` in the root when absent.",
    },
    Binding {
        key: "backups_dir",
        layer: Layer::Root,
        says: "Where the root's backups are to be kept, nothing yet; `backups` in the root when absent.",
    },
];

impl Setting {
    /// Its default, as a value.
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).expect("a listed default is JSON")
    }
}

/// The listed lake setting a `sys.setting` key names, and the type or
/// the source it is for: `<key>` for the lake, `<key>.<type>` for a type,
/// `<key>.<source>` for a source. `None` for a host's own key.
pub fn lake_setting(key: &str) -> Option<(&'static Setting, Option<&str>)> {
    let lake = SETTINGS.iter().filter(|s| s.layer == Layer::Lake);
    lake.into_iter()
        .find_map(|s| match key.strip_prefix(s.key) {
            Some("") if s.per_type != PerType::Only => Some((s, None)),
            Some(rest) if s.per_type != PerType::No => Some((s, Some(rest.strip_prefix('.')?))),
            _ => None,
        })
}

/// The prefix of the store's own settings `key` is under — a listed lake
/// setting's key up to its first dot — where it names no listed setting;
/// `None` for a listed setting's key and for a host's own.
pub fn unlisted(key: &str) -> Option<&'static str> {
    let head = key.split('.').next()?;
    let lake = SETTINGS.iter().filter(|s| s.layer == Layer::Lake);
    let prefix = lake
        .map(|s| s.key.split('.').next().unwrap_or(s.key))
        .find(|p| *p == head)?;
    lake_setting(key).is_none().then_some(prefix)
}

/// The value of the lake setting `s` for the type or the source `ty`, or
/// for the lake: `<key>.<ty>` where it stands, else `<key>`, else its
/// default.
pub fn value_of(settings: &BTreeMap<String, Value>, s: &Setting, ty: Option<&str>) -> Value {
    let typed = ty.filter(|_| s.per_type != PerType::No);
    let own = typed.and_then(|t| settings.get(&format!("{}.{t}", s.key)));
    let lake = settings.get(s.key).filter(|_| s.per_type != PerType::Only);
    own.or(lake).cloned().unwrap_or_else(|| s.default_value())
}

/// The lake settings as `describe` tells them: each listed one for the
/// lake, held or not, and each one `settings` holds for a type or a
/// source, whose default is the lake's value.
pub fn told(settings: &BTreeMap<String, Value>) -> Vec<SettingValue> {
    let lake = SETTINGS.iter().filter(|s| s.layer == Layer::Lake);
    let wide = lake
        .filter(|s| s.per_type != PerType::Only)
        .map(|s| SettingValue {
            key: s.key.to_owned(),
            value: settings.get(s.key).cloned(),
            default: s.default_value(),
            declared: None,
        });
    let typed = settings
        .iter()
        .filter_map(|(key, value)| match lake_setting(key)? {
            (s, Some(_)) => Some(SettingValue {
                key: key.clone(),
                value: Some(value.clone()),
                default: value_of(settings, s, None),
                declared: None,
            }),
            _ => None,
        });
    wide.chain(typed).collect()
}

/// `s` as told to a reader shown the types `shown` answers and reading
/// the fields `reads` answers, its value [`narrowed`].
pub fn seen(
    mut s: SettingValue,
    shown: &dyn Fn(&str) -> bool,
    reads: &dyn Fn(&str, &str) -> bool,
) -> Option<SettingValue> {
    match s.value.take() {
        Some(v) => s.value = Some(narrowed(&s.key, v, shown, reads)?),
        None => {
            narrowed(&s.key, Value::Null, shown, reads)?;
        }
    }
    Some(s)
}

/// The value of the setting `key` as told to a reader shown the types
/// `shown` answers and reading the fields `reads` answers: a setting of a
/// type it is not shown is not told, and one that names fields is told
/// only those it reads, as were the others absent; one left naming none
/// is not told. A host's own key, a setting for the lake and one for a
/// source, which names no type, are told whole.
pub fn narrowed(
    key: &str,
    mut value: Value,
    shown: &dyn Fn(&str) -> bool,
    reads: &dyn Fn(&str, &str) -> bool,
) -> Option<Value> {
    let Some((l, Some(ty))) = lake_setting(key) else {
        return Some(value);
    };
    if l.per_type == PerType::Source {
        return Some(value);
    }
    if !shown(ty) {
        return None;
    }
    let read = |name: &str| reads(ty, name);
    let kept = match &mut value {
        Value::Array(names) if !names.is_empty() => {
            names.retain(|n| n.as_str().is_some_and(read));
            !names.is_empty()
        }
        Value::Object(o) => {
            for member in FIELD_MEMBERS {
                if let Some(Value::Object(fields)) = o.get_mut(member) {
                    fields.retain(|n, _| read(n));
                    if fields.is_empty() {
                        o.remove(member);
                    }
                }
            }
            !o.is_empty()
        }
        _ => true,
    };
    kept.then_some(value)
}

/// The members of a setting's object that name fields, each with its
/// weight: `search_weights`' vector fields and its text fields.
const FIELD_MEMBERS: [&str; 2] = ["fields", "text_fields"];

/// What a write of `written` to the setting `key` leaves where `stored`
/// stands, by a writer kept from the fields `hidden` answers: every field
/// `stored` names that is hidden stays, after those written, so a write
/// drops nothing its writer is not told; one the writer names itself (a
/// field it is shown masked) stands once, as written. `written` as it is
/// where `stored` names none. The store calls it no longer: a setting
/// naming fields is told whole or not at all, and refused whole to a
/// writer it is not told (063).
#[deprecated(
    since = "0.17.0",
    note = "a writer is told such a setting whole or is refused it; write the value as told"
)]
pub fn kept(
    key: &str,
    stored: &Value,
    written: Value,
    hidden: &dyn Fn(&str, &str) -> bool,
) -> Value {
    let Some((_, Some(ty))) = lake_setting(key) else {
        return written;
    };
    let unread = |name: &str| hidden(ty, name);
    match (stored, written) {
        (Value::Array(was), written @ (Value::Array(_) | Value::Null)) => {
            let hidden = was.iter().filter(|n| n.as_str().is_some_and(unread));
            let hidden: Vec<Value> = hidden.cloned().collect();
            match written {
                _ if hidden.is_empty() => written,
                Value::Array(mut names) => {
                    let more: Vec<Value> =
                        hidden.into_iter().filter(|n| !names.contains(n)).collect();
                    names.extend(more);
                    Value::Array(names)
                }
                _ => Value::Array(hidden),
            }
        }
        (Value::Object(was), written @ (Value::Object(_) | Value::Null)) => {
            let hidden = |member: &str| -> serde_json::Map<String, Value> {
                let fields = was.get(member).and_then(Value::as_object);
                let hidden = fields.into_iter().flatten().filter(|(n, _)| unread(n));
                hidden.map(|(n, w)| (n.clone(), w.clone())).collect()
            };
            if FIELD_MEMBERS.iter().all(|m| hidden(m).is_empty()) {
                return written;
            }
            let mut o = match written {
                Value::Object(o) => o,
                _ => serde_json::Map::new(),
            };
            for member in FIELD_MEMBERS {
                let hidden = hidden(member);
                if hidden.is_empty() {
                    continue;
                }
                let fields = (o.entry(member)).or_insert_with(|| Value::Object(Default::default()));
                if let Value::Object(fields) = fields {
                    for (n, w) in hidden {
                        fields.entry(n).or_insert(w);
                    }
                }
            }
            Value::Object(o)
        }
        (_, written) => written,
    }
}

#[cfg(test)]
mod tests;
