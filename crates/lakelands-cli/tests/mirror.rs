//! The mirror, over two lakes in one process: a `crm` lake the connection
//! reads and a `main` lake it writes, driven round by round, on every
//! engine the binary carries.

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::principal::{Access, TypeGrants};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Migration};
use lakelands_api::{
    Catalog, Error, Get, Grants, LakeStore, Location, Origin, Patch, Position, Principal,
    PrincipalKind, ReadAt, RecordKey, SourceId, TypeName, Upstream, WriteContext, WriteOpts,
    WriteOrigin,
};
use lakelands_cli::mirror::{
    ConnectionConfig, MIRROR_TYPE, Mirror, MirrorConfig, MirrorVocabulary, Start, mirror_type,
};
use serde_json::{Value, json};

const SOURCE: &str = "mirror:crm:customer";

fn decl(v: Value) -> Declaration {
    serde_json::from_value(v).expect("a declaration")
}

fn text(name: &str, authority: &str) -> Value {
    json!({"name": name, "type": {"kind": "text"}, "authority": authority})
}

fn required(name: &str, authority: &str) -> Value {
    let mut f = text(name, authority);
    f["required"] = json!(true);
    f
}

/// `partner`, keyed by name, and `customer`, keyed by code, referring to a
/// partner and to another customer, with a field the mirror may not read.
fn source_types() -> Vec<Declaration> {
    vec![
        decl(
            json!({"declare": "type", "name": "partner", "key": ["name"],
                    "fields": [required("name", "authored")]}),
        ),
        decl(
            json!({"declare": "type", "name": "customer", "key": ["code"],
                    "fields": [required("code", "authored"), text("name", "authored"),
                               text("tier", "authored"), text("secret", "authored"),
                               {"name": "partner", "type": {"kind": "reference", "target": "partner"}},
                               {"name": "parent", "type": {"kind": "reference", "target": "customer"}}]}),
        ),
    ]
}

fn customer_copy(fields: Vec<Value>) -> Declaration {
    decl(json!({"declare": "type", "name": "customer", "key": ["code"], "fields": fields}))
}

fn copy_fields() -> Vec<Value> {
    vec![
        required("code", "source"),
        text("name", "source"),
        text("tier", "source"),
        json!({"name": "partner", "type": {"kind": "reference", "target": "partner"}, "authority": "source"}),
        json!({"name": "parent", "type": {"kind": "reference", "target": "customer"}, "authority": "source"}),
    ]
}

/// The copies, and the target's own `note` about a customer.
fn target_types() -> Vec<Declaration> {
    vec![
        decl(
            json!({"declare": "type", "name": "partner", "key": ["name"],
                    "fields": [required("name", "source")]}),
        ),
        customer_copy(copy_fields()),
        decl(json!({"declare": "type", "name": "note",
                    "fields": [text("text", "authored"),
                               {"name": "about", "type": {"kind": "reference", "target": "customer"}}]})),
        decl(json!({"declare": "trigger", "name": "customer_changed", "on": "customer"})),
    ]
}

fn owner(lake: &dyn LakeStore) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.lake().clone(),
        grants: Grants::owner(),
    }
}

/// The connection's principal: in the source it reads everything but
/// `customer.secret`, and held records only with `restricted`; in the
/// target it writes and ingests.
fn reader(lake: &dyn LakeStore, restricted: bool) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        restricted,
        ..Grants::default()
    };
    grants.types.insert(
        TypeName::from("customer"),
        TypeGrants {
            default: None,
            fields: [("secret".into(), Access::Masked)].into(),
            ..Default::default()
        },
    );
    Principal {
        kind: PrincipalKind::Service,
        id: "mirror".into(),
        lake: lake.lake().clone(),
        grants,
    }
}

fn writer_grants() -> Grants {
    Grants {
        default: Access::Write,
        ingest: true,
        ..Grants::default()
    }
}

fn writer(lake: &dyn LakeStore) -> Principal {
    Principal {
        kind: PrincipalKind::Service,
        id: "mirror".into(),
        lake: lake.lake().clone(),
        grants: writer_grants(),
    }
}

/// Where a test's two lakes are kept: every engine the binary carries,
/// PostgreSQL when `LAKELANDS_TEST_PG_URL` names a server (read here, in
/// test code; `scripts/conformance.sh pg` runs this file against one).
#[derive(Clone, Debug)]
enum Engine {
    Memory,
    Sqlite,
    Postgres(String),
}

impl Engine {
    fn name(&self) -> &'static str {
        match self {
            Engine::Memory => "memory",
            Engine::Sqlite => "sqlite",
            Engine::Postgres(_) => "pg",
        }
    }
}

fn engines() -> Vec<Engine> {
    let mut out = vec![Engine::Memory, Engine::Sqlite];
    if let Some(url) = std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        out.push(Engine::Postgres(url));
    }
    out
}

/// The schema a lake on PostgreSQL is kept in, when it is.
fn schema(at: &Location) -> Option<(String, String)> {
    match &at.engine {
        lakelands_api::EngineConfig::Postgres {
            url,
            schema: Some(s),
            ..
        } => Some((url.clone(), s.clone())),
        _ => None,
    }
}

/// A lake's location on `engine`, under `dir` or in a schema of its own.
fn location(engine: &Engine, dir: &std::path::Path, lake: &str) -> Location {
    match engine {
        Engine::Memory => Location::memory(lake),
        Engine::Sqlite => Location::sqlite(lake, dir.join("lakes").join(lake)),
        Engine::Postgres(url) => {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Location {
                engine: lakelands_api::EngineConfig::Postgres {
                    url: url.clone(),
                    schema: Some(format!("lakelands_test_mirror_{}_{n}", std::process::id())),
                    tls: lakelands_api::location::PgTls {
                        mode: lakelands_api::location::PgTlsMode::Disable,
                        ..Default::default()
                    },
                    tuning: Default::default(),
                },
                ..Location::memory(lake)
            }
        }
    }
}

fn doors(lake: &dyn LakeStore) -> Arc<dyn Catalog> {
    lake.catalog(WriteContext::default())
}

fn patch(v: Value) -> Patch {
    serde_json::from_value(v).unwrap()
}

/// Two lakes on one engine and the root directory the mirror keeps its
/// lock and journal in.
struct Two {
    crm: Arc<dyn LakeStore>,
    main: Arc<dyn LakeStore>,
    crm_at: Location,
    main_at: Location,
    /// The engine, and every further lake's location handed out on it.
    engine: Engine,
    more: Vec<Location>,
    seams: Seams,
    dir: PathBuf,
    restricted: bool,
    grants: Grants,
}

impl Two {
    async fn new(test: &str, engine: &Engine) -> Two {
        Two::with(test, engine, source_types(), target_types()).await
    }

    async fn with(
        test: &str,
        engine: &Engine,
        source: Vec<Declaration>,
        target: Vec<Declaration>,
    ) -> Two {
        Two::ceilings(test, engine, source, target, None).await
    }

    /// Two lakes whose reactive ceilings are `(crm, main)` when given.
    async fn ceilings(
        test: &str,
        engine: &Engine,
        source: Vec<Declaration>,
        target: Vec<Declaration>,
        ceilings: Option<(u32, u32)>,
    ) -> Two {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("mirror")
            .join(format!("{test}-{}", engine.name()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[allow(unused_mut)]
        let mut engines: Vec<Arc<dyn lakelands_api::engine::EngineOpener>> = vec![
            Arc::new(lakelands_engine_mem::MemoryOpener::new()),
            Arc::new(lakelands_engine_sqlite::SqliteOpener),
        ];
        #[cfg(feature = "pg")]
        engines.push(Arc::new(lakelands_engine_pg::PgOpener));
        let seams = Seams {
            engines,
            vocabulary: Some(Arc::new(MirrorVocabulary::of(&config(Start::Snapshot)))),
            ..Seams::default()
        };
        let open = |at: Location| lakelands_store::open(at, seams.clone(), OpenMode::Create);
        let ceiling = |at: Location, max: Option<u32>| Location {
            limits: Limits {
                max_reactive_depth: max.unwrap_or(at.limits.max_reactive_depth),
                ..at.limits
            },
            ..at
        };
        let crm_at = ceiling(location(engine, &dir, "crm"), ceilings.map(|c| c.0));
        let crm = open(crm_at.clone()).await.unwrap();
        let main_at = ceiling(location(engine, &dir, "main"), ceilings.map(|c| c.1));
        let main = open(main_at.clone()).await.unwrap();
        doors(crm.as_ref())
            .apply(&owner(crm.as_ref()), source)
            .await
            .unwrap();
        doors(main.as_ref())
            .apply(&owner(main.as_ref()), target)
            .await
            .unwrap();
        Two {
            crm,
            main,
            crm_at,
            main_at,
            engine: engine.clone(),
            more: Vec::new(),
            seams,
            dir,
            restricted: false,
            grants: writer_grants(),
        }
    }

    /// The location of a further lake `name` on the same engine, whose
    /// schema is dropped with the two (lakelands#1011).
    fn lake(&mut self, name: &str) -> Location {
        let at = location(&self.engine, &self.dir, name);
        self.more.push(at.clone());
        at
    }

    fn config(&self, start: Start) -> MirrorConfig {
        config(start)
    }

    fn connection(&self, start: Start) -> ConnectionConfig {
        connection(start)
    }

    async fn mirror(&self) -> Result<Mirror, String> {
        self.mirror_from(Start::Snapshot).await
    }

    async fn mirror_from(&self, start: Start) -> Result<Mirror, String> {
        let config = self.config(start);
        config.check()?;
        let mut writer = writer(self.main.as_ref());
        writer.grants = self.grants.clone();
        Mirror::new(
            &config,
            &config.connections[0],
            &self.dir,
            self.crm.clone(),
            self.main.clone(),
            reader(self.crm.as_ref(), self.restricted),
            writer,
        )
        .await
    }

    async fn upsert(&self, ty: &str, key: &str, fields: Value) {
        doors(self.crm.as_ref())
            .upsert(
                &owner(self.crm.as_ref()),
                &TypeName::from(ty),
                &RecordKey::from(key),
                patch(fields),
                WriteOpts::default(),
            )
            .await
            .unwrap();
    }

    async fn retire(&self, ty: &str, key: &str) {
        doors(self.crm.as_ref())
            .retire(
                &owner(self.crm.as_ref()),
                &TypeName::from(ty),
                &RecordKey::from(key),
                WriteOpts::default(),
            )
            .await
            .unwrap();
    }

    /// The copy as the target's owner reads it, with provenance.
    async fn copy(&self, ty: &str, key: &str) -> Option<lakelands_api::Record> {
        let get = Get {
            key: RecordKey::from(key),
            with_meta: true,
            at: ReadAt::default(),
            params: Default::default(),
        };
        match doors(self.main.as_ref())
            .get(&owner(self.main.as_ref()), &TypeName::from(ty), get)
            .await
        {
            Ok(r) => Some(r.data),
            Err(Error::NotFound { .. }) => None,
            Err(e) => panic!("reading the copy: {e}"),
        }
    }

    async fn head(&self) -> Position {
        self.main.head().await.unwrap()
    }

    /// How many times the target's trigger fired.
    async fn fired(&self) -> usize {
        self.events(&["trigger_fired"], &[]).await.len()
    }

    /// The target's events of these kinds about these types, in order.
    async fn events(&self, kinds: &[&str], types: &[&str]) -> Vec<lakelands_api::event::Event> {
        let filter = FeedFilter {
            kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
            types: types.iter().map(|t| TypeName::from(*t)).collect(),
            ..FeedFilter::default()
        };
        self.main
            .feed()
            .poll(Position::GENESIS, &filter, 10_000)
            .await
            .unwrap()
            .events
    }

    /// The connection's `sys.mirror` records, by source type, as `find`
    /// answers them to the target's owner.
    async fn positions(&self) -> std::collections::BTreeMap<String, Patch> {
        let found = doors(self.main.as_ref())
            .find(
                &owner(self.main.as_ref()),
                &TypeName::from(MIRROR_TYPE),
                lakelands_api::Find::default(),
            )
            .await
            .unwrap()
            .data;
        let lakelands_api::Found::Records { rows, .. } = found else {
            panic!("records, not groups");
        };
        rows.into_iter()
            .map(|r| {
                (
                    r.fields["source_type"].as_str().unwrap().to_owned(),
                    r.fields,
                )
            })
            .collect()
    }

    /// Write the connection's records back as `positions` read them: what
    /// a crash between a durable window and its record leaves.
    async fn rewind(&self, to: &std::collections::BTreeMap<String, Patch>) {
        let ty = TypeName::from(MIRROR_TYPE);
        for fields in to.values() {
            let key = lakelands_api::key::record_key(&mirror_type(), fields).unwrap();
            let mut fields = fields.clone();
            for f in ["stopped_at", "stopped_why"] {
                fields.entry(f.into()).or_insert(Value::Null);
            }
            doors(self.main.as_ref())
                .upsert(
                    &owner(self.main.as_ref()),
                    &ty,
                    &key,
                    fields,
                    WriteOpts::default(),
                )
                .await
                .unwrap();
        }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("mirrors").join("main")
    }
}

/// A test's PostgreSQL schemas are dropped when it is done with them.
impl Drop for Two {
    fn drop(&mut self) {
        let schemas: Vec<(String, String)> = [&self.crm_at, &self.main_at]
            .into_iter()
            .chain(&self.more)
            .filter_map(schema)
            .collect();
        if schemas.is_empty() {
            return;
        }
        // On a thread of its own: the synchronous client runs a runtime of
        // its own, which a test's runtime thread cannot.
        let _ = std::thread::spawn(move || {
            for (url, schema) in schemas {
                if let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) {
                    let _ = client.batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"));
                }
            }
        })
        .join();
    }
}

fn config(start: Start) -> MirrorConfig {
    MirrorConfig {
        connections: vec![connection(start)],
        poll_millis: 1000,
        batch: 100,
    }
}

fn connection(start: Start) -> ConnectionConfig {
    ConnectionConfig {
        name: "crm".into(),
        source: "crm".into(),
        target: "main".into(),
        principal: "mirror".into(),
        types: [
            ("partner".to_owned(), "partner".to_owned()),
            ("customer".to_owned(), "customer".to_owned()),
        ]
        .into(),
        start,
    }
}

/// The position of the state of `ty` `key` in the source lake, as its
/// `name` field's provenance gives it.
async fn state_of(crm: &dyn LakeStore, ty: &str, key: &str) -> Position {
    let read = doors(crm)
        .get(
            &owner(crm),
            &TypeName::from(ty),
            Get {
                key: RecordKey::from(key),
                with_meta: true,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .unwrap()
        .data;
    read.meta[&lakelands_api::FieldName::from("name")].position
}

/// Round after round until the mirror has nothing more to do.
async fn settle(m: &mut Mirror) {
    for _ in 0..50 {
        if !m.round().await.unwrap() {
            return;
        }
    }
    panic!("the mirror did not settle");
}

async fn migrate(lake: &dyn LakeStore, to: Value) {
    let cat = doors(lake);
    let who = owner(lake);
    let migration = Migration::to(serde_json::from_value(to).unwrap());
    let report = cat.migrate(&who, migration.clone()).await.unwrap().data;
    assert!(report.lossless, "{report:?}");
    cat.migrate_apply(&who, migration, &report.report_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_record_crosses_as_the_connections_source_and_what_it_may_not_read_stays_home() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("crosses", engine).await;
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        two.upsert(
        "customer",
        "c1",
        json!({"code": "c1", "name": "Ada", "tier": "gold", "secret": "s3cret", "partner": "acme"}),
    )
    .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let c = two.copy("customer", "c1").await.expect("the copy");
        assert_eq!(c.fields["name"], json!("Ada"));
        assert_eq!(c.fields["partner"], json!("acme"));
        assert!(!c.fields.contains_key("secret"));
        let meta = &c.meta[&lakelands_api::FieldName::from("name")];
        assert_eq!(meta.origin, Origin::Source);
        assert_eq!(meta.source, Some(SourceId::from(SOURCE)));
        // The field names the source lake and the position of the state
        // read: here the snapshot's.
        let at = state_of(two.crm.as_ref(), "customer", "c1").await;
        let upstream = |at| {
            Some(Upstream {
                lake: "crm".into(),
                position: at,
            })
        };
        assert_eq!(meta.upstream, upstream(at), "{engine:?}");

        // A later change crosses from the feed, and a clear crosses as one.
        two.upsert("customer", "c1", json!({"tier": null, "name": "Ada L."}))
            .await;
        settle(&mut m).await;
        let c = two.copy("customer", "c1").await.unwrap();
        assert_eq!(c.fields["name"], json!("Ada L."));
        assert!(!c.fields.contains_key("tier"), "the clear crossed: {c:?}");
        // A change from the feed names the position of its own state.
        let at = state_of(two.crm.as_ref(), "customer", "c1").await;
        assert_eq!(
            c.meta[&lakelands_api::FieldName::from("name")].upstream,
            upstream(at),
            "{engine:?}"
        );
    }
}

/// A poll examines at most `FEED_EXAMINED` events, so a mirror whose types
/// change rarely reads on past a poll that answered none, and the change
/// after a long run of another type's writes crosses in the same settle.
#[tokio::test]
async fn a_mirror_reads_on_past_a_bounded_poll() {
    let mut source = source_types();
    source.push(decl(
        json!({"declare": "type", "name": "visit", "fields": [text("text", "authored")]}),
    ));
    let two = Two::with("reads-on", &Engine::Memory, source, target_types()).await;
    let mut m = two.mirror().await.unwrap();
    settle(&mut m).await;
    for i in 0..4_200 {
        two.upsert("visit", &format!("v{i}"), json!({"text": "seen"}))
            .await;
    }
    two.upsert("partner", "acme", json!({"name": "acme"})).await;
    settle(&mut m).await;
    assert!(
        two.copy("partner", "acme").await.is_some(),
        "the change after 4 200 writes of an unmirrored type crossed"
    );
}

/// A connection's source is a lake of the same root, opened by the
/// process that mirrors it. One whose log another release wrote, in
/// another format, is refused at that open by its name and both formats,
/// so no mirror of it starts and nothing of it is read
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
#[tokio::test]
async fn a_source_lake_of_another_format_is_refused_at_its_open() {
    use lakelands_api::canonical;
    use lakelands_api::error::Situation;
    use lakelands_api::event::{LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT};
    for engine in engines() {
        let two = Two::new("format", &engine).await;
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        two.crm.close().await.unwrap();
        let (crm_at, seams) = (two.crm_at.clone(), two.seams.clone());
        // The source's next event as a later release would write it: its
        // format one above, its hash made right.
        let other = LOG_FORMAT_VERSION + 1;
        {
            let name = match engine {
                Engine::Memory => "memory",
                Engine::Sqlite => "sqlite",
                Engine::Postgres(_) => "pg",
            };
            let opener = seams.engines.iter().find(|e| e.name() == name).unwrap();
            let seam = opener.open(&crm_at, OpenMode::Write).unwrap();
            let lease = seam.take_lease("the release before").unwrap();
            let (tip, prev) = seam.tip().unwrap().unwrap();
            let taken = lakelands_store::log::Draft::system(
                lakelands_api::Timestamp(i64::MAX / 2),
                EventBody::LeaseTaken {
                    fence: lease.fence,
                    holder: lease.holder.clone(),
                },
            );
            let (_, stored) =
                lakelands_store::log::seal(&crm_at.lake, tip.next(), &prev, lease.fence, taken);
            let mut event: Value = serde_json::from_slice(&stored.bytes).unwrap();
            event["v"] = json!(other);
            event.as_object_mut().unwrap().remove("hash");
            let hash = canonical::sha256_hex(&event);
            event["hash"] = json!(hash);
            let stored = lakelands_api::engine::CanonicalEvent {
                hash: lakelands_api::event::Digest(hash),
                bytes: canonical::to_vec(&event),
                ..stored
            };
            seam.append(&lease, &[stored]).unwrap();
            seam.release_lease(&lease).unwrap();
        }
        let Err(e) = lakelands_store::open(crm_at, seams, OpenMode::Read).await else {
            panic!("a source lake in format {other} opened to be mirrored");
        };
        assert!(
            matches!(e.situation(), Some(Situation::LogFormat { lake, found, reads, .. })
                if lake == "crm" && *found == u64::from(other)
                    && *reads == u64::from(LOG_FORMAT_VERSION)),
            "{}: {e:?}",
            engine.name()
        );
        let said = format!(
            "lake `crm` is written in log format {other}, and this release reads log formats \
             {OLDEST_LOG_FORMAT} to {LOG_FORMAT_VERSION} and writes {LOG_FORMAT_VERSION}; \
             nothing was read from it and nothing appended"
        );
        assert!(e.to_string().contains(&said), "{e}");
    }
}

/// A source lake the release before wrote whole, in format 14, is not
/// read as it is: the open a mirror makes of its source, read-only, is
/// refused with the way, on every engine; its first writer's open
/// upgrades it (`docs/log-format.md` §6).
#[tokio::test]
async fn a_source_lake_of_the_format_before_is_refused_until_it_is_upgraded() {
    use lakelands_api::canonical;
    use lakelands_api::error::Situation;
    use lakelands_api::event::{LOG_FORMAT_VERSION as NEW, OLDEST_LOG_FORMAT as OLD};
    for engine in engines() {
        let mut two = Two::new("format14", &engine).await;
        let at = two.lake("older");
        let name = match engine {
            Engine::Memory => "memory",
            Engine::Sqlite => "sqlite",
            Engine::Postgres(_) => "pg",
        };
        let opener = two.seams.engines.iter().find(|e| e.name() == name).unwrap();
        let seam = opener.open(&at, OpenMode::Create).unwrap();
        let lease = seam.take_lease("the release before").unwrap();
        let older = |e: lakelands_api::engine::CanonicalEvent, format: bool| {
            let mut v: Value = serde_json::from_slice(&e.bytes).unwrap();
            v["v"] = json!(OLD);
            if format {
                v["body"]["format"] = json!(OLD);
            }
            v.as_object_mut().unwrap().remove("hash");
            let hash = canonical::sha256_hex(&v);
            v["hash"] = json!(hash);
            lakelands_api::engine::CanonicalEvent {
                hash: lakelands_api::event::Digest(hash),
                bytes: canonical::to_vec(&v),
                ..e
            }
        };
        let made = lakelands_store::log::Draft::system(
            lakelands_api::Timestamp(1),
            EventBody::LakeCreated { format: OLD },
        );
        let zero = lakelands_api::event::Digest::zero();
        let (_, made) =
            lakelands_store::log::seal(&at.lake, lakelands_api::Position(0), &zero, 0, made);
        let made = older(made, true);
        let taken = lakelands_store::log::Draft::system(
            lakelands_api::Timestamp(2),
            EventBody::LeaseTaken {
                fence: lease.fence,
                holder: lease.holder.clone(),
            },
        );
        let (_, taken) = lakelands_store::log::seal(
            &at.lake,
            lakelands_api::Position(1),
            &made.hash,
            lease.fence,
            taken,
        );
        seam.append(&lease, &[made, older(taken, false)]).unwrap();
        seam.release_lease(&lease).unwrap();
        drop(seam);
        let Err(e) = lakelands_store::open(at.clone(), two.seams.clone(), OpenMode::Read).await
        else {
            panic!(
                "{}: a source lake in format {OLD} opened as it is",
                engine.name()
            );
        };
        assert!(
            matches!(e.situation(), Some(Situation::LogUpgradeOwed { lake, found, reads })
                if lake == "older" && *found == u64::from(OLD) && *reads == u64::from(NEW)),
            "{}: {e:?}",
            engine.name()
        );
        let writer = lakelands_store::open(at.clone(), two.seams.clone(), OpenMode::Write).await;
        writer.unwrap().close().await.unwrap();
        let reader = lakelands_store::open(at, two.seams.clone(), OpenMode::Read).await;
        assert!(reader.is_ok(), "{}: {:?}", engine.name(), reader.err());
    }
}

/// A mirror that reads held records writes their copies only with a
/// writer that holds `restricted` in the target: it is refused before
/// anything is copied, naming the writer and the grant (042).
#[tokio::test]
async fn a_mirror_that_reads_held_records_needs_a_writer_that_may_hold_them() {
    let mut two = Two::new("held-writer", &Engine::Memory).await;
    two.restricted = true;
    two.upsert("customer", "c1", json!({"code": "c1", "name": "Ada"}))
        .await;
    let Err(refused) = two.mirror().await else {
        panic!("a mirror whose writer may not hold held records is refused");
    };
    assert!(
        refused.contains("`mirror`") && refused.contains("no `restricted` grant"),
        "{refused}"
    );
    assert!(
        two.copy("customer", "c1").await.is_none(),
        "nothing is copied"
    );
}

#[tokio::test]
async fn a_held_record_crosses_only_to_a_mirror_that_may_see_held_records() {
    for engine in engines() {
        let engine = &engine;
        for restricted in [false, true] {
            let mut two = Two::new(&format!("held-{restricted}"), engine).await;
            (two.restricted, two.grants.restricted) = (restricted, restricted);
            two.upsert("customer", "c1", json!({"code": "c1", "name": "Ada"}))
                .await;
            let mut m = two.mirror().await.unwrap();
            settle(&mut m).await;
            let ty = TypeName::from("customer");
            two.crm
                .restrict(&ty, &RecordKey::from("c1"), "legal hold".into())
                .await
                .unwrap();
            two.upsert("customer", "c1", json!({"name": "held"})).await;
            settle(&mut m).await;
            let c = two.copy("customer", "c1").await.unwrap();
            if restricted {
                assert_eq!(c.fields["name"], json!("held"));
                // Held in the target too: a reader without the grant sees nothing.
                let mut plain = writer(two.main.as_ref());
                plain.grants.restricted = false;
                let get = Get {
                    key: RecordKey::from("c1"),
                    with_meta: false,
                    at: ReadAt::default(),
                    params: Default::default(),
                };
                let seen = doors(two.main.as_ref()).get(&plain, &ty, get).await;
                assert!(matches!(seen, Err(Error::NotFound { .. })), "{seen:?}");
            } else {
                assert_eq!(
                    c.fields["name"],
                    json!("Ada"),
                    "the copy keeps its last state"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_retirement_crosses_and_a_later_write_brings_the_copy_back() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("retire", engine).await;
        two.upsert("customer", "c1", json!({"code": "c1", "name": "Ada"}))
            .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        two.retire("customer", "c1").await;
        settle(&mut m).await;
        assert!(
            two.copy("customer", "c1").await.is_none(),
            "the copy is retired"
        );
        two.upsert("customer", "c1", json!({"code": "c1", "name": "Ada again"}))
            .await;
        settle(&mut m).await;
        let c = two.copy("customer", "c1").await.expect("back");
        assert_eq!(c.fields["name"], json!("Ada again"));
    }
}

#[tokio::test]
async fn the_targets_own_type_refers_to_a_copy_and_purging_the_connection_removes_only_copies() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("purge", engine).await;
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        two.upsert("customer", "c1", json!({"code": "c1", "partner": "acme"}))
            .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let main = doors(two.main.as_ref());
        let own = owner(two.main.as_ref());
        main.upsert(
            &own,
            &TypeName::from("note"),
            &RecordKey::from("n1"),
            patch(json!({"text": "called", "about": "c1"})),
            WriteOpts::default(),
        )
        .await
        .expect("a reference to the copy names a live record");
        let report = two
            .main
            .purge_source(&SourceId::from(SOURCE), None, None)
            .await
            .unwrap();
        assert!(two.copy("customer", "c1").await.is_none());
        assert!(two.copy("partner", "acme").await.is_some(), "{report:?}");
        assert!(two.copy("note", "n1").await.is_some());
    }
}

#[tokio::test]
async fn changes_of_one_window_land_once_and_a_window_delivered_again_appends_nothing() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("again", engine).await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let before = two.positions().await;
        two.upsert("customer", "c1", json!({"code": "c1", "name": "first"}))
            .await;
        two.upsert("customer", "c1", json!({"name": "second"}))
            .await;
        two.upsert("customer", "c2", json!({"code": "c2", "parent": "c1"}))
            .await;
        settle(&mut m).await;
        assert_eq!(two.fired().await, 2, "one trigger per copy and window");
        assert_eq!(
            two.copy("customer", "c1").await.unwrap().fields["name"],
            json!("second")
        );
        // Each window's record follows the window: the last record of
        // `customer` comes after every copy it counts.
        let events = two
            .events(
                &["record_ingested", "record_upserted"],
                &["customer", MIRROR_TYPE],
            )
            .await;
        let last_copy = events
            .iter()
            .filter(|e| matches!(&e.body, EventBody::RecordIngested { ty, .. } if ty == "customer"))
            .map(|e| e.position)
            .max()
            .unwrap();
        let last_record = events
        .iter()
        .filter(|e| matches!(&e.body, EventBody::RecordUpserted { key, .. } if key.as_str().ends_with("customer")))
        .map(|e| e.position)
        .max()
        .unwrap();
        assert!(
            last_copy < last_record,
            "the window is durable before its record"
        );
        // The record is lost after the window was written: it comes again.
        drop(m);
        two.rewind(&before).await;
        let head = two.head().await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let after = two.head().await;
        let moved = two
            .events(&[], &[])
            .await
            .into_iter()
            .filter(|e| e.position > head)
            .count();
        assert!(
            after >= head && moved <= 2,
            "the window delivered again appended nothing but its records: {moved} event(s)"
        );
        assert!(
            two.events(&["record_ingested"], &["customer"])
                .await
                .iter()
                .all(|e| e.position <= head),
            "no copy was written again"
        );
        assert_eq!(two.fired().await, 2);
    }
}

#[tokio::test]
async fn a_field_gone_in_the_source_stops_its_type_and_a_restart_resumes_once_the_target_follows() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("retype", engine).await;
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        two.upsert("customer", "c1", json!({"code": "c1", "tier": "gold"}))
            .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let without_tier = |authority: &str| {
            json!({"name": "customer", "key": ["code"], "fields": [
            required("code", authority), text("name", authority), text("secret", authority),
            {"name": "partner", "type": {"kind": "reference", "target": "partner"}, "authority": authority},
            {"name": "parent", "type": {"kind": "reference", "target": "customer"}, "authority": authority}]})
        };
        migrate(two.crm.as_ref(), without_tier("authored")).await;
        two.upsert("customer", "c2", json!({"code": "c2"})).await;
        two.upsert("partner", "globex", json!({"name": "globex"}))
            .await;
        settle(&mut m).await;
        let kept = m.kept().types[&TypeName::from("customer")].clone();
        let why = kept.stopped.expect("customer stopped").why;
        assert!(why.contains("tier"), "{why}");
        let record = &two.positions().await["customer"];
        assert!(record["stopped_at"].is_u64(), "{record:?}");
        assert!(
            record["stopped_why"].as_str().unwrap().contains("tier"),
            "the stop is readable by `find`: {record:?}"
        );
        assert!(two.copy("customer", "c2").await.is_none());
        assert!(
            two.copy("partner", "globex").await.is_some(),
            "partner goes on"
        );

        // The target drops the field too; a restart resumes where it stopped.
        drop(m);
        let mut copy = without_tier("source");
        copy["fields"]
            .as_array_mut()
            .unwrap()
            .retain(|f| f["name"] != "secret");
        migrate(two.main.as_ref(), copy).await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert!(
            m.kept().types[&TypeName::from("customer")]
                .stopped
                .is_none()
        );
        assert!(two.copy("customer", "c2").await.is_some());
    }
}

#[tokio::test]
async fn a_rekey_in_the_source_stops_the_type_until_the_target_is_rekeyed_the_same_way() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("rekey", engine).await;
        two.upsert("customer", "c1", json!({"code": "c1", "name": "ada"}))
            .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let by_name = |authority: &str, secret: bool| {
            let mut fields = vec![
                text("code", authority),
                required("name", authority),
                text("tier", authority),
                json!({"name": "partner", "type": {"kind": "reference", "target": "partner"}, "authority": authority}),
                json!({"name": "parent", "type": {"kind": "reference", "target": "customer"}, "authority": authority}),
            ];
            if secret {
                fields.push(text("secret", authority));
            }
            json!({"name": "customer", "key": ["name"], "fields": fields})
        };
        migrate(two.crm.as_ref(), by_name("authored", true)).await;
        two.upsert("customer", "bob", json!({"name": "bob"})).await;
        settle(&mut m).await;
        let stopped = m.kept().types[&TypeName::from("customer")].stopped.clone();
        assert!(stopped.is_some(), "{:?}", m.kept());
        drop(m);
        migrate(two.main.as_ref(), by_name("source", false)).await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert!(
            m.kept().types[&TypeName::from("customer")]
                .stopped
                .is_none()
        );
        assert!(two.copy("customer", "bob").await.is_some());
        assert!(
            two.copy("customer", "ada").await.is_some(),
            "the copy moved with the re-key"
        );
    }
}

/// A value type's renames in the source write no record event: the mirror
/// checks each type they rewrite again, as at a declaration, and copies it
/// again whole, also when it started from the head.
#[tokio::test]
async fn a_value_renamed_in_the_source_is_copied_again() {
    let tiers = |one_of: Value| {
        decl(json!({"declare": "value_type", "name": "tiers", "kind": "text", "one_of": one_of}))
    };
    let tiered = |types: Vec<Declaration>, authority: &str| {
        let mut out = vec![];
        for d in types {
            let mut v = serde_json::to_value(&d).unwrap();
            for f in v
                .get_mut("fields")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if f["name"] == "tier" {
                    *f = json!({"name": "tier", "authority": authority,
                                "type": {"kind": "text", "value_type": "tiers"}});
                }
            }
            out.push(decl(v));
        }
        out
    };
    for engine in engines() {
        let engine = &engine;
        for start in [Start::Snapshot, Start::Head] {
            let mut source = vec![tiers(json!(["gold", "silver"]))];
            source.extend(tiered(source_types(), "authored"));
            let mut target = vec![tiers(json!(["gold", "silver", "platinum"]))];
            target.extend(tiered(target_types(), "source"));
            let two = Two::with(&format!("revalued-{start:?}"), engine, source, target).await;
            let mut m = two.mirror_from(start).await.unwrap();
            settle(&mut m).await;
            two.upsert("customer", "c1", json!({"code": "c1", "tier": "gold"}))
                .await;
            settle(&mut m).await;
            let cat = doors(two.crm.as_ref());
            let who = owner(two.crm.as_ref());
            let m2 = lakelands_api::write::ValueTypeMigration {
                to: serde_json::from_value(
                    json!({"name": "tiers", "kind": "text", "one_of": ["platinum", "silver"]}),
                )
                .unwrap(),
                renames: vec![("gold".into(), "platinum".into())],
            };
            let report = cat.migrate_value_type(&who, m2.clone()).await.unwrap().data;
            cat.migrate_value_type_apply(&who, m2, &report.report_id)
                .await
                .unwrap();
            settle(&mut m).await;
            let copy = two.copy("customer", "c1").await.expect("copied");
            assert_eq!(
                copy.fields.get("tier"),
                Some(&json!("platinum")),
                "{start:?}"
            );
            let kept = &m.kept().types[&TypeName::from("customer")];
            assert!(kept.stopped.is_none(), "{start:?}: {kept:?}");
        }
    }
}

/// A migration's fill writes values and no record event: the mirror copies
/// the type again, and the copy names the source's state after the fill,
/// whose value is of origin `initial` there.
#[tokio::test]
async fn a_field_filled_in_the_source_is_copied_again() {
    for engine in engines() {
        for start in [Start::Snapshot, Start::Head] {
            let two = Two::new(&format!("filled-{start:?}"), &engine).await;
            let mut m = two.mirror_from(start).await.unwrap();
            settle(&mut m).await;
            two.upsert("customer", "c1", json!({"code": "c1"})).await;
            settle(&mut m).await;
            let cat = doors(two.crm.as_ref());
            let who = owner(two.crm.as_ref());
            let mut to = serde_json::to_value(&source_types()[1]).unwrap();
            to["fields"][2]["initial"] = json!("gold");
            to.as_object_mut().unwrap().remove("declare");
            let mut migration = Migration::to(serde_json::from_value(to).unwrap());
            migration.fill = vec!["tier".into()];
            let report = cat.migrate(&who, migration.clone()).await.unwrap().data;
            let applied = cat.migrate_apply(&who, migration, &report.report_id);
            let at = match applied.await.unwrap() {
                lakelands_api::write::Outcome::Applied { position, .. } => position,
                other => panic!("{other:?}"),
            };
            settle(&mut m).await;
            let get = Get {
                key: RecordKey::from("c1"),
                with_meta: true,
                at: ReadAt::default(),
                params: Default::default(),
            };
            let source = cat.get(&who, &"customer".into(), get).await.unwrap().data;
            let filled = &source.meta[&lakelands_api::FieldName::from("tier")];
            assert_eq!((filled.origin, filled.position), (Origin::Initial, at));
            let copy = two.copy("customer", "c1").await.expect("copied");
            assert_eq!(copy.fields.get("tier"), Some(&json!("gold")), "{start:?}");
            let upstream = copy.meta[&lakelands_api::FieldName::from("tier")]
                .upstream
                .clone();
            assert!(
                upstream.is_some_and(|u| u.lake.as_str() == "crm" && u.position >= at),
                "{start:?}: {copy:?}"
            );
        }
    }
}

#[tokio::test]
async fn snapshot_copies_what_is_there_and_head_does_not() {
    for engine in engines() {
        let engine = &engine;
        for start in [Start::Snapshot, Start::Head] {
            let two = Two::new(&format!("start-{start:?}"), engine).await;
            // A child written before its parent's last change: it still lands
            // after the parent.
            two.upsert("customer", "p", json!({"code": "p"})).await;
            two.upsert("customer", "c", json!({"code": "c", "parent": "p"}))
                .await;
            two.upsert("customer", "p", json!({"name": "parent"})).await;
            let mut m = two.mirror_from(start).await.unwrap();
            settle(&mut m).await;
            let copied = two.copy("customer", "c").await.is_some();
            assert_eq!(copied, start == Start::Snapshot, "{start:?}");
            two.upsert("customer", "n", json!({"code": "n"})).await;
            settle(&mut m).await;
            assert!(
                two.copy("customer", "n").await.is_some(),
                "{start:?} follows"
            );
        }
    }
}

#[tokio::test]
async fn a_second_source_for_a_type_and_a_way_back_are_refused_naming_both() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("twice", engine).await;
        let mut config = two.config(Start::Snapshot);
        let mut other = two.connection(Start::Snapshot);
        other.name = "erp".into();
        other.source = "erp".into();
        config.connections.push(other);
        let why = config.check().unwrap_err();
        assert!(why.contains("`crm/customer` by connection `crm`"), "{why}");
        assert!(why.contains("`erp/customer` by connection `erp`"), "{why}");

        let mut config = two.config(Start::Snapshot);
        let mut back = two.connection(Start::Snapshot);
        back.name = "back".into();
        back.source = "main".into();
        back.target = "crm".into();
        config.connections.push(back);
        let why = config.check().unwrap_err();
        assert!(
            why.contains("connection `crm`") && why.contains("connection `back`"),
            "{why}"
        );
    }
}

/// A target type declares only fields its source writes: an authored
/// field is refused at start, one that declares `initial` too, so no
/// window meets a value the target's fold filled (050).
#[tokio::test]
async fn a_target_type_the_source_does_not_own_whole_is_refused_at_start() {
    for engine in engines() {
        let engine = &engine;
        let cases: Vec<(&str, Vec<Value>, &str)> = vec![
            (
                "authored",
                {
                    let mut f = copy_fields();
                    f.push(text("remark", "authored"));
                    f
                },
                "`customer.remark` is written by the authority `authored`",
            ),
            (
                "initial",
                {
                    let mut f = copy_fields();
                    let mut stage = text("stage", "authored");
                    stage["initial"] = json!("lead");
                    f.push(stage);
                    f
                },
                "`customer.stage` is written by the authority `authored`",
            ),
            (
                "masked",
                {
                    let mut f = copy_fields();
                    f.push(text("secret", "source"));
                    f
                },
                "the principal may not read `customer.secret`",
            ),
            (
                "missing",
                {
                    let mut f = copy_fields();
                    f.push(text("vip", "source"));
                    f
                },
                "no field `vip`",
            ),
            (
                "unmirrored",
                {
                    let mut f = copy_fields();
                    f.push(json!({"name": "note", "type": {"kind": "reference", "target": "note"}, "authority": "source"}));
                    f
                },
                "refers to `note`, which the connection does not mirror",
            ),
        ];
        for (name, fields, said) in cases {
            let mut target = target_types();
            target[1] = customer_copy(fields);
            let two = Two::with(&format!("refused-{name}"), engine, source_types(), target).await;
            let why = two.mirror().await.unwrap_err();
            assert!(why.contains(said), "{name}: {why}");
        }
    }
}

/// A derived relation of a mirrored type stores nothing, so its source
/// writes nothing for it (lakelands#902): the mirror starts whether the
/// source derives the relation, carries one of that name by links or
/// declares none, and a walk over it in the target answers by the
/// target's own match: what the same walk answers in the source where
/// the source derives it too, and not what the source's links answer.
#[tokio::test]
async fn a_derived_relation_of_a_mirrored_type_is_the_targets_own_reading() {
    use lakelands_api::query as q;
    async fn walked(lake: &dyn LakeStore) -> Vec<(String, String)> {
        let step = q::Step {
            relation: "same".into(),
            walk: q::Walk::Out,
            ty: None,
            filter: None,
            depth: Default::default(),
            named: None,
        };
        let pattern = q::Pattern {
            start: q::Start {
                ty: Some("customer".into()),
                key: Some("c1".into()),
                filter: None,
                search: None,
                params: Default::default(),
                named: None,
            },
            steps: vec![step],
            to: None,
            project: q::Projection::Graph,
            order_by: Vec::new(),
            limit: None,
            cursor: None,
            at: ReadAt::default(),
        };
        let answer = doors(lake).query(&owner(lake), pattern).await;
        match answer.unwrap().data {
            q::QueryResult::Graph { nodes, .. } => {
                let ends = nodes.iter().map(|n| (n.ty.to_string(), n.key.to_string()));
                ends.collect()
            }
            other => panic!("the walk answered {other:?}"),
        }
    }
    let same = json!({"name": "same", "target": "partner",
                      "derived": {"match": [{"from": "tier", "to": "name"}]}});
    // The target's own reading of a copy, to a type the connection does not mirror.
    let noted = json!({"name": "noted", "target": "note",
                       "derived": {"match": [{"from": "name", "to": "text"}]}});
    let source_with = |relations: Value| {
        decl(
            json!({"declare": "type", "name": "customer", "key": ["code"],
            "relations": relations,
            "fields": [required("code", "authored"), text("name", "authored"),
                       text("tier", "authored"), text("secret", "authored"),
                       {"name": "partner", "type": {"kind": "reference", "target": "partner"}},
                       {"name": "parent", "type": {"kind": "reference", "target": "customer"}}]}),
        )
    };
    for engine in engines() {
        // The source derives the relation, carries one of that name by links, or has none.
        for there in ["derived", "links", "none"] {
            let mut source = source_types();
            match there {
                "derived" => source[1] = source_with(json!([same])),
                "links" => source[1] = source_with(json!([{"name": "same", "target": "partner"}])),
                _ => {}
            }
            let mut target = target_types();
            target[1] = decl(
                json!({"declare": "type", "name": "customer", "key": ["code"],
                                    "fields": copy_fields(), "relations": [same, noted]}),
            );
            let two = Two::with(&format!("derived-{there}"), &engine, source, target).await;
            two.upsert("partner", "gold", json!({"name": "gold"})).await;
            two.upsert("partner", "acme", json!({"name": "acme"})).await;
            two.upsert("customer", "c1", json!({"code": "c1", "tier": "gold"}))
                .await;
            if there == "links" {
                let crm = two.crm.as_ref();
                let (cat, who, relation) = (doors(crm), owner(crm), "same".into());
                let (ty, c1, to) = ("customer".into(), "c1".into(), "acme".into());
                let (fields, opts) = (Default::default(), WriteOpts::default());
                let linked = cat.link(&who, &ty, &c1, &relation, &to, fields, opts);
                linked.await.unwrap();
            }
            let mut m = two.mirror().await.unwrap();
            settle(&mut m).await;
            // The target answers by its own match, whatever the source carries.
            let here = walked(two.main.as_ref()).await;
            let ends =
                |to: &str| [("customer", "c1"), ("partner", to)].map(|(t, k)| (t.into(), k.into()));
            assert_eq!(here, ends("gold"), "{engine:?}, the source: {there}");
            match there {
                "derived" => assert_eq!(here, walked(two.crm.as_ref()).await, "{engine:?}"),
                "links" => assert_eq!(walked(two.crm.as_ref()).await, ends("acme"), "{engine:?}"),
                _ => {}
            }
        }
        // A relation the target carries by links is not one the source derives.
        let mut linked = same.clone();
        linked["authority"] = json!("source");
        linked.as_object_mut().unwrap().remove("derived");
        let mut source = source_types();
        source[1] = source_with(json!([same]));
        let mut target = target_types();
        target[1] = decl(
            json!({"declare": "type", "name": "customer", "key": ["code"],
                                "fields": copy_fields(), "relations": [linked]}),
        );
        let two = Two::with("derived-linked", &engine, source, target).await;
        let why = two.mirror().await.unwrap_err();
        assert!(why.contains("no relation `same` carried by links"), "{why}");
    }
}

/// A connection's principal held at `none` on `partner.name` in the
/// source is not shown `partner`, which is keyed by it, so neither is
/// `customer.partner`, which refers to it (lakelands#718): the mirror is
/// refused at start, naming the field, and does not stall.
#[tokio::test]
async fn a_reference_its_principal_may_not_know_of_in_the_source_is_refused_at_start() {
    for engine in engines() {
        let two = Two::with("kept-partner", &engine, source_types(), target_types()).await;
        let mut kept = reader(two.crm.as_ref(), false);
        let held = TypeGrants {
            fields: [("name".into(), Access::None)].into(),
            ..Default::default()
        };
        kept.grants.types.insert(TypeName::from("partner"), held);
        let config = two.config(Start::Snapshot);
        config.check().expect("the configuration reads");
        let mut writer = writer(two.main.as_ref());
        writer.grants = two.grants.clone();
        let made = Mirror::new(
            &config,
            &config.connections[0],
            &two.dir,
            two.crm.clone(),
            two.main.clone(),
            kept,
            writer,
        )
        .await;
        let Err(why) = made else {
            panic!("the mirror is refused");
        };
        assert!(
            why.contains("`customer` has no field `partner` the principal may read"),
            "{}: {why}",
            engine.name()
        );
    }
}

#[tokio::test]
async fn a_target_this_process_does_not_write_is_refused() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::new("reader", engine).await;
        let read_only =
            lakelands_store::open(two.main_at.clone(), two.seams.clone(), OpenMode::Read)
                .await
                .unwrap();
        let config = two.config(Start::Snapshot);
        let why = Mirror::new(
            &config,
            &config.connections[0],
            &two.dir,
            two.crm.clone(),
            read_only.clone(),
            reader(two.crm.as_ref(), false),
            writer(read_only.as_ref()),
        )
        .await
        .unwrap_err();
        assert!(
            why.contains("not held by this process as its writer"),
            "{why}"
        );
    }
}

/// The built binary: a root with two lakes on disk and a `mirror`
/// section; `serve` on the target copies the source's records while it
/// runs, keeps its position in the target's `sys.mirror`, and a second
/// `serve` copies nothing again.
#[test]
fn serve_mirrors_a_lake_of_its_root_named_in_lakelands_json() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("mirror")
        .join("serve");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let lakelands = env!("CARGO_BIN_EXE_lakelands");
    let run = |args: &[&str]| -> String {
        let out = Command::new(lakelands)
            .arg("--root")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let bundle = |name: &str, authority: &str| {
        let dir = root.join(name).join("types");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("customer.yaml"),
            format!(
                "name: customer\nkey: code\nfields:\n  code: {{ type: text, authority: source, required: true }}\n  name: {{ type: text, authority: {authority} }}\n"
            ),
        )
        .unwrap();
        root.join(name).display().to_string()
    };
    run(&["init"]);
    run(&["lake", "new", "crm"]);
    run(&["lake", "new", "main"]);
    run(&["apply", &bundle("crm-types", "source"), "--lake", "crm"]);
    run(&["apply", &bundle("main-types", "source"), "--lake", "main"]);
    let rows = root.join("export.ndjson");
    std::fs::write(
        &rows,
        "{\"type\": \"customer\", \"key\": \"c1\", \"fields\": {\"code\": \"c1\", \"name\": \"Ada\"}}\n\
         {\"type\": \"customer\", \"key\": \"c2\", \"fields\": {\"code\": \"c2\", \"name\": \"Bob\"}}\n",
    )
    .unwrap();
    run(&[
        "ingest",
        rows.to_str().unwrap(),
        "--source",
        "export",
        "--lake",
        "crm",
    ]);

    let file = root.join("lakelands.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    config["principals"] = json!([{
        "name": "mirror", "kind": "service", "id": "mirror",
        "grants": {"default": "write", "ingest": true}
    }]);
    config["mirror"] = json!({
        "poll_millis": 50,
        "connections": [{"name": "crm", "source": "crm", "target": "main",
                         "principal": "mirror", "types": {"customer": "customer"}}]
    });
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let copies = || -> Vec<String> {
        run(&["feed", "--lake", "main"])
            .lines()
            .filter_map(|l| {
                let e: Value = serde_json::from_str(l).unwrap();
                (e["body"]["kind"] == "record_ingested")
                    .then(|| e["body"]["key"].as_str().unwrap().to_owned())
            })
            .collect()
    };
    let serve = || {
        let mut server = Command::new(lakelands)
            .arg("--root")
            .arg(&root)
            .args(["serve", "--mcp", "--lake", "main"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < until && copies().len() < 2 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let mut to = server.stdin.take().unwrap();
        let _ = to.flush();
        drop(to);
        let out = server.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "serve: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    serve();
    assert_eq!(copies(), ["c1", "c2"]);
    serve();
    assert_eq!(
        copies(),
        ["c1", "c2"],
        "a second serve copies nothing again"
    );
}

#[tokio::test]
async fn links_cross_when_the_target_declares_the_relation_and_an_unlink_crosses_too() {
    for engine in engines() {
        let engine = &engine;
        let with_relation = |types: Vec<Declaration>, authority: &str| -> Vec<Declaration> {
            types
                .into_iter()
                .map(|d| match d {
                    Declaration::Type(mut t) if t.name == "customer" => {
                        t.relations = vec![
                            serde_json::from_value(json!({"name": "knows", "target": "partner",
                                                      "cardinality": "many_to_many",
                                                      "authority": authority}))
                            .unwrap(),
                        ];
                        Declaration::Type(t)
                    }
                    d => d,
                })
                .collect()
        };
        let two = Two::with(
            "links",
            engine,
            with_relation(source_types(), "authored"),
            with_relation(target_types(), "source"),
        )
        .await;
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        two.upsert("customer", "c1", json!({"code": "c1"})).await;
        let crm = doors(two.crm.as_ref());
        let own = owner(two.crm.as_ref());
        let (ty, rel) = (TypeName::from("customer"), "knows".into());
        let (c1, acme) = (RecordKey::from("c1"), RecordKey::from("acme"));
        crm.link(
            &own,
            &ty,
            &c1,
            &rel,
            &acme,
            Patch::new(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let link = lakelands_api::key::link_key(&c1, &acme);
        assert!(
            two.copy("sys.link.customer.knows", link.as_str())
                .await
                .is_some()
        );
        crm.unlink(&own, &ty, &c1, &rel, &acme, WriteOpts::default())
            .await
            .unwrap();
        settle(&mut m).await;
        assert!(
            two.copy("sys.link.customer.knows", link.as_str())
                .await
                .is_none()
        );
    }
}

#[tokio::test]
async fn the_position_lives_in_sys_mirror_and_a_restart_reads_it_there() {
    for engine in engines() {
        let two = Two::new("record", &engine).await;
        two.upsert("customer", "c1", json!({"code": "c1"})).await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        let next = two.crm.head().await.unwrap().next().0;
        let positions = two.positions().await;
        for ty in ["customer", "partner"] {
            let record = &positions[ty];
            assert_eq!(record["next"], json!(next), "{engine:?} {ty}: {record:?}");
            assert_eq!(record["connection"], json!("crm"));
            assert_eq!(record["source_lake"], json!("crm"));
            assert_eq!(record["target_type"], json!(ty));
        }
        assert!(
            !two.home().join("crm.json").exists(),
            "no position file is written"
        );
        drop(m);
        let head = two.head().await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert_eq!(
            two.head().await,
            head,
            "{engine:?}: the restart copies nothing again"
        );
    }
}

#[tokio::test]
async fn an_old_position_file_seeds_the_records_once_and_is_never_read_again() {
    for engine in engines() {
        let two = Two::new("seeded", &engine).await;
        two.upsert("customer", "c1", json!({"code": "c1"})).await;
        let next = two.crm.head().await.unwrap().next().0;
        std::fs::create_dir_all(two.home()).unwrap();
        let kept = |next: u64| {
            json!({"types": {"customer": {"next": next}, "partner": {"next": next}}}).to_string()
        };
        std::fs::write(two.home().join("crm.json"), kept(next)).unwrap();
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert_eq!(two.positions().await["customer"]["next"], json!(next));
        assert!(
            two.copy("customer", "c1").await.is_none(),
            "{engine:?}: the file's position was taken, not a new copy"
        );
        assert!(!two.home().join("crm.json").exists());
        assert!(two.home().join("crm.json.seeded").exists());

        // A stale file beside records is not read.
        drop(m);
        std::fs::write(two.home().join("crm.json"), kept(1)).unwrap();
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert_eq!(two.positions().await["customer"]["next"], json!(next));
        assert!(two.copy("customer", "c1").await.is_none(), "{engine:?}");
    }
}

#[tokio::test]
async fn a_principal_that_may_not_write_sys_mirror_is_refused_naming_the_grant() {
    for engine in engines() {
        let mut two = Two::new("grant", &engine).await;
        two.grants.types.insert(
            TypeName::from(MIRROR_TYPE),
            TypeGrants {
                default: Some(Access::Read),
                fields: Default::default(),
                ..Default::default()
            },
        );
        let why = two.mirror().await.unwrap_err();
        assert!(
            why.contains("write grant on `sys.mirror`"),
            "{engine:?}: {why}"
        );
    }
}

#[tokio::test]
async fn a_copy_crosses_at_its_source_depth_and_a_chain_past_the_targets_ceiling_stops() {
    for engine in engines() {
        let engine = &engine;
        let two = Two::ceilings(
            "depth",
            engine,
            source_types(),
            target_types(),
            Some((12, 6)),
        )
        .await;
        let write = |key: &'static str, depth: u32| {
            let cat = two.crm.catalog(WriteContext {
                origin: WriteOrigin::Reactive,
                depth,
                cause: None,
            });
            let owner = owner(two.crm.as_ref());
            async move {
                cat.upsert(
                    &owner,
                    &"customer".into(),
                    &key.into(),
                    patch(json!({"code": key, "name": key})),
                    WriteOpts::default(),
                )
                .await
                .unwrap();
            }
        };
        // A snapshot copies standing state; what follows crosses from the feed.
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        write("c1", 5).await;
        settle(&mut m).await;
        let copies = two.events(&["record_ingested"], &["customer"]).await;
        let copy = copies.last().expect("c1 crossed");
        assert_eq!(
            (copy.context.depth, copy.context.origin, copy.cause),
            (5, WriteOrigin::Reactive, None),
            "a copy is a reaction at its source's depth, naming no cause; {engine:?}"
        );
        // One window holds c2 at depth 2 and c3 at depth 9: it crosses at
        // the deeper, past the target's ceiling of 6, and the chain ends.
        write("c2", 2).await;
        write("c3", 9).await;
        settle(&mut m).await;
        assert!(two.copy("customer", "c2").await.is_none(), "{engine:?}");
        let why = m.kept().types[&TypeName::from("customer")]
            .stopped
            .clone()
            .expect("customer stopped")
            .why;
        assert!(
            why.contains("`c3`") && why.contains("depth 9") && why.contains("ceiling of 6"),
            "the stop names the record, its depth and the ceiling: {why}"
        );
        // A retirement crosses at its own depth too.
        let two = Two::ceilings(
            "depth-retire",
            engine,
            source_types(),
            target_types(),
            Some((12, 6)),
        )
        .await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        two.upsert("customer", "c1", json!({"code": "c1", "name": "c1"}))
            .await;
        settle(&mut m).await;
        let deep = two.crm.catalog(WriteContext {
            origin: WriteOrigin::Reactive,
            depth: 9,
            cause: None,
        });
        deep.retire(
            &owner(two.crm.as_ref()),
            &"customer".into(),
            &"c1".into(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
        settle(&mut m).await;
        assert!(two.copy("customer", "c1").await.is_some(), "{engine:?}");
        let why = m.kept().types[&TypeName::from("customer")]
            .stopped
            .clone()
            .expect("customer stopped")
            .why;
        assert!(why.contains("at depth 9"), "{why}");
    }
}

/// At the source's forgetting of a source (058), a mapped type whose
/// source type has an erasable field stops past the act, naming the
/// source, since its copies may hold that source's values under the
/// mirror's own; a type with no erasable field goes on.
#[tokio::test]
async fn a_forgetting_in_the_source_stops_a_type_with_an_erasable_field() {
    for engine in engines() {
        let engine = &engine;
        let email = json!({"name": "email", "type": {"kind": "text"}, "authority": "source", "erasable": true});
        let source = vec![
            decl(
                json!({"declare": "type", "name": "partner", "key": ["name"],
                        "fields": [required("name", "authored")]}),
            ),
            decl(
                json!({"declare": "type", "name": "customer", "key": ["code"],
                        "fields": [required("code", "source"), email]}),
            ),
        ];
        let target = vec![
            decl(
                json!({"declare": "type", "name": "partner", "key": ["name"],
                        "fields": [required("name", "source")]}),
            ),
            customer_copy(vec![required("code", "source"), text("email", "source")]),
        ];
        let mut two = Two::with("forgotten", engine, source, target).await;
        // The source lake again, with a keyring.
        two.crm.close().await.unwrap();
        let keyring = lakelands_backings::FileKeyring::new(two.dir.join("keys"));
        let seams = Seams {
            keys: Some(Arc::new(keyring)),
            ..two.seams.clone()
        };
        two.crm = lakelands_store::open(two.crm_at.clone(), seams, OpenMode::Write)
            .await
            .unwrap();
        let rows = vec![lakelands_core::conformance::fixture::source_record(
            "customer",
            "c1",
            json!({"code": "c1", "email": "ada@example.org"}),
        )];
        let cat = doors(two.crm.as_ref());
        lakelands_core::conformance::fixture::ingest(
            cat.as_ref(),
            &owner(two.crm.as_ref()),
            "web",
            rows,
        )
        .await
        .unwrap();
        two.upsert("partner", "acme", json!({"name": "acme"})).await;
        let mut m = two.mirror().await.unwrap();
        settle(&mut m).await;
        assert!(
            two.copy("customer", "c1").await.is_some(),
            "{engine:?}: c1 crossed"
        );
        two.crm.forget_source(&"web".into(), None).await.unwrap();
        two.upsert("partner", "globex", json!({"name": "globex"}))
            .await;
        settle(&mut m).await;
        let kept = m.kept().types[&TypeName::from("customer")].clone();
        let why = kept.stopped.expect("customer stopped").why;
        assert!(
            why.contains("forgot `web`") && why.contains("mirror:"),
            "{why}"
        );
        assert!(
            two.copy("partner", "globex").await.is_some(),
            "{engine:?}: partner, with no erasable field, goes on"
        );
    }
}
