//! The mirror, over two in-memory lakes in one process: a `crm` lake the
//! connection reads and a `main` lake it writes, driven round by round.

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::{Access, TypeGrants};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Migration};
use lakelands_api::{
    Catalog, Error, Get, Grants, LakeStore, Location, Origin, Patch, Position, Principal,
    PrincipalKind, ReadAt, RecordKey, SourceId, TypeName, WriteContext, WriteOpts,
};
use lakelands_cli::mirror::{ConnectionConfig, Mirror, MirrorConfig, Start};
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
        },
    );
    Principal {
        kind: PrincipalKind::Service,
        id: "mirror".into(),
        lake: lake.lake().clone(),
        grants,
    }
}

fn writer(lake: &dyn LakeStore) -> Principal {
    Principal {
        kind: PrincipalKind::Service,
        id: "mirror".into(),
        lake: lake.lake().clone(),
        grants: Grants {
            default: Access::Write,
            ingest: true,
            ..Grants::default()
        },
    }
}

async fn open(
    opener: &lakelands_engine_mem::MemoryOpener,
    name: &str,
    mode: OpenMode,
) -> Arc<dyn LakeStore> {
    lakelands_store::open(
        Location::memory(name),
        Seams {
            engines: vec![Arc::new(opener.clone())],
            ..Seams::default()
        },
        mode,
    )
    .await
    .unwrap()
}

fn doors(lake: &dyn LakeStore) -> Arc<dyn Catalog> {
    lake.catalog(WriteContext::default())
}

fn patch(v: Value) -> Patch {
    serde_json::from_value(v).unwrap()
}

/// Two lakes and the root directory the mirror keeps its files in.
struct Two {
    crm: Arc<dyn LakeStore>,
    main: Arc<dyn LakeStore>,
    dir: PathBuf,
    restricted: bool,
    opener: lakelands_engine_mem::MemoryOpener,
}

impl Two {
    async fn new(test: &str) -> Two {
        Two::with(test, source_types(), target_types()).await
    }

    async fn with(test: &str, source: Vec<Declaration>, target: Vec<Declaration>) -> Two {
        let opener = lakelands_engine_mem::MemoryOpener::new();
        let crm = open(&opener, "crm", OpenMode::Create).await;
        let main = open(&opener, "main", OpenMode::Create).await;
        doors(crm.as_ref())
            .apply(&owner(crm.as_ref()), source)
            .await
            .unwrap();
        doors(main.as_ref())
            .apply(&owner(main.as_ref()), target)
            .await
            .unwrap();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("mirror")
            .join(test);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Two {
            crm,
            main,
            dir,
            restricted: false,
            opener,
        }
    }

    fn config(&self, start: Start) -> MirrorConfig {
        MirrorConfig {
            connections: vec![self.connection(start)],
            poll_millis: 1000,
            batch: 100,
        }
    }

    fn connection(&self, start: Start) -> ConnectionConfig {
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

    async fn mirror(&self) -> Result<Mirror, String> {
        self.mirror_from(Start::Snapshot).await
    }

    async fn mirror_from(&self, start: Start) -> Result<Mirror, String> {
        let config = self.config(start);
        config.check()?;
        Mirror::new(
            &config,
            &config.connections[0],
            &self.dir,
            self.crm.clone(),
            self.main.clone(),
            reader(self.crm.as_ref(), self.restricted),
            writer(self.main.as_ref()),
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
        let filter = FeedFilter {
            kinds: vec!["trigger_fired".into()],
            ..FeedFilter::default()
        };
        self.main
            .feed()
            .poll(Position::GENESIS, &filter, 1000)
            .await
            .unwrap()
            .events
            .len()
    }

    fn position_file(&self) -> PathBuf {
        self.dir.join("mirrors").join("main").join("crm.json")
    }
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
    let migration = Migration {
        to: serde_json::from_value(to).unwrap(),
        renames: vec![],
    };
    let report = cat.migrate(&who, migration.clone()).await.unwrap().data;
    assert!(report.lossless, "{report:?}");
    cat.migrate_apply(&who, migration, &report.report_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_record_crosses_as_the_connections_source_and_what_it_may_not_read_stays_home() {
    let two = Two::new("crosses").await;
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

    // A later change crosses from the feed, and a clear crosses as one.
    two.upsert("customer", "c1", json!({"tier": null, "name": "Ada L."}))
        .await;
    settle(&mut m).await;
    let c = two.copy("customer", "c1").await.unwrap();
    assert_eq!(c.fields["name"], json!("Ada L."));
    assert!(!c.fields.contains_key("tier"), "the clear crossed: {c:?}");
}

#[tokio::test]
async fn a_held_record_crosses_only_to_a_mirror_that_may_see_held_records() {
    for restricted in [false, true] {
        let mut two = Two::new(&format!("held-{restricted}")).await;
        two.restricted = restricted;
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

#[tokio::test]
async fn a_retirement_crosses_and_a_later_write_brings_the_copy_back() {
    let two = Two::new("retire").await;
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

#[tokio::test]
async fn the_targets_own_type_refers_to_a_copy_and_purging_the_connection_removes_only_copies() {
    let two = Two::new("purge").await;
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
        .purge_source(&SourceId::from(SOURCE))
        .await
        .unwrap();
    assert!(two.copy("customer", "c1").await.is_none());
    assert!(two.copy("partner", "acme").await.is_some(), "{report:?}");
    assert!(two.copy("note", "n1").await.is_some());
}

#[tokio::test]
async fn changes_of_one_window_land_once_and_a_window_delivered_again_appends_nothing() {
    let two = Two::new("again").await;
    let mut m = two.mirror().await.unwrap();
    settle(&mut m).await;
    let before = std::fs::read(two.position_file()).unwrap();
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
    let head = two.head().await;
    // The position is lost after the window was written: it comes again.
    drop(m);
    std::fs::write(two.position_file(), before).unwrap();
    let mut m = two.mirror().await.unwrap();
    settle(&mut m).await;
    assert_eq!(
        two.head().await,
        head,
        "the window delivered again appended nothing"
    );
    assert_eq!(two.fired().await, 2);
}

#[tokio::test]
async fn a_field_gone_in_the_source_stops_its_type_and_a_restart_resumes_once_the_target_follows() {
    let two = Two::new("retype").await;
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

#[tokio::test]
async fn a_rekey_in_the_source_stops_the_type_until_the_target_is_rekeyed_the_same_way() {
    let two = Two::new("rekey").await;
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

#[tokio::test]
async fn snapshot_copies_what_is_there_and_head_does_not() {
    for start in [Start::Snapshot, Start::Head] {
        let two = Two::new(&format!("start-{start:?}")).await;
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

#[tokio::test]
async fn a_second_source_for_a_type_and_a_way_back_are_refused_naming_both() {
    let two = Two::new("twice").await;
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

#[tokio::test]
async fn a_target_type_the_source_does_not_own_whole_is_refused_at_start() {
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
        let two = Two::with(&format!("refused-{name}"), source_types(), target).await;
        let why = two.mirror().await.unwrap_err();
        assert!(why.contains(said), "{name}: {why}");
    }
}

#[tokio::test]
async fn a_target_this_process_does_not_write_is_refused() {
    let two = Two::new("reader").await;
    let read_only = open(&two.opener, "main", OpenMode::Read).await;
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

/// The built binary: a root with two lakes on disk and a `mirror`
/// section; `serve` on the target copies the source's records while it
/// runs, keeps its position in the root, and a second `serve` copies
/// nothing again.
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
        let position = root.join("mirrors").join("main").join("crm.json");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < until {
            let kept = std::fs::read_to_string(&position).unwrap_or_default();
            if kept.contains("\"next\"") && !kept.contains("\"next\": 0") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
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
