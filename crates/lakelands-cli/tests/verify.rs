//! `lakelands verify` (lakelands#1156): an operator checks a lake's log with
//! one command, on every engine the binary carries, PostgreSQL when
//! `LAKELANDS_TEST_PG_URL` names a server (`scripts/conformance.sh pg` runs
//! this file against one). An open does not read the whole chain: a line
//! of the log changed by hand at its own length opens, and is written on;
//! only a read of every event finds it, which `verify` is.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_cli::Opening;

fn block_on<F: Future>(f: F) -> F::Output {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a runtime")
        })
        .block_on(f)
}

/// The note a type's declaration carries, which the tests change by hand.
const NOTE: &str = "A note the ledger keeps.";

/// The engines the binary carries.
fn engines() -> Vec<Arc<dyn EngineOpener>> {
    lakelands_cli::engines()
}

/// A root with the lake `ledger`, on SQLite or on the PostgreSQL server at
/// `pg`, and the stores its writing commands opened, held open.
struct Root {
    dir: PathBuf,
    pg: Option<(String, String)>,
    held: Arc<Mutex<BTreeMap<String, Arc<dyn LakeStore>>>>,
}

struct Said {
    code: u8,
    out: String,
    err: String,
}

impl Root {
    fn new(name: &str, pg: Option<&str>) -> Root {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("verify")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        let schema = format!("lakelands_test_verify_{name}_{}", std::process::id());
        let root = Root {
            dir,
            pg: pg.map(|url| (url.to_owned(), schema)),
            held: Arc::default(),
        };
        let dir = root.dir.display().to_string();
        assert_eq!(
            root.run_raw(&["lakelands", "init", &dir], &root.writer())
                .code,
            0
        );
        match &root.pg {
            None => assert_eq!(root.run(&["lake", "new", "ledger"]).code, 0),
            // The location is the file `lake new` writes, not one made
            // by hand: the lake opens from what an operator gets.
            Some((url, schema)) => {
                let made = root.run(&[
                    "lake", "new", "ledger", "--engine", "postgres", "--url", url, "--schema",
                    schema, "--no-tls",
                ]);
                assert_eq!(made.code, 0, "{}{}", made.out, made.err);
            }
        }
        root
    }

    /// Opens a lake once and holds it, as a running process does.
    fn writer(&self) -> impl Fn(Location, OpenMode, Seams) -> Opening + Sync + '_ {
        let held = self.held.clone();
        move |l: Location, mode: OpenMode, host: Seams| -> Opening {
            let held = held.clone();
            Box::pin(async move {
                let name = l.lake.to_string();
                if let Some(store) = held.lock().unwrap().get(&name) {
                    return Ok(store.clone());
                }
                let seams = Seams {
                    engines: engines(),
                    ..host
                };
                let store = lakelands_store::open(l, seams, mode).await?;
                held.lock().unwrap().insert(name, store.clone());
                Ok(store)
            })
        }
    }

    fn run(&self, args: &[&str]) -> Said {
        let root = self.dir.display().to_string();
        let mut argv = vec!["lakelands", "--root", root.as_str()];
        argv.extend_from_slice(args);
        self.run_raw(&argv, &self.writer())
    }

    fn run_raw(
        &self,
        argv: &[&str],
        open: &(dyn Fn(Location, OpenMode, Seams) -> Opening + Sync),
    ) -> Said {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = block_on(lakelands_cli::run(
            argv.iter().copied(),
            &mut out,
            &mut err,
            open,
        ));
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
        Said {
            code,
            out: text(out),
            err: text(err),
        }
    }

    /// `verify` as a second process runs it: the lake opened afresh, every
    /// open's mode kept in `asked`.
    fn verify(&self, args: &[&str], asked: &Mutex<Vec<OpenMode>>) -> Said {
        let open = |l: Location, mode: OpenMode, host: Seams| -> Opening {
            asked.lock().unwrap().push(mode);
            let seams = Seams {
                engines: engines(),
                ..host
            };
            Box::pin(lakelands_store::open(l, seams, mode))
        };
        let root = self.dir.display().to_string();
        let mut argv = vec!["lakelands", "--root", root.as_str(), "verify"];
        argv.extend_from_slice(args);
        self.run_raw(&argv, &open)
    }

    /// A type and three records written, the writer held open.
    fn fill(&self) {
        let bundle = self.dir.join("bundle/types");
        fs::create_dir_all(&bundle).unwrap();
        let note = format!(
            "name: note\ndescription: {NOTE}\nfields:\n  text: {{ type: text, authority: source }}\n"
        );
        fs::write(bundle.join("note.yaml"), note).unwrap();
        let apply = self.run(&["apply", &self.dir.join("bundle").display().to_string()]);
        assert_eq!(apply.code, 0, "{}", apply.err);
        let rows: String = (1..=3)
            .map(|i| format!("{{\"type\": \"note\", \"key\": \"n{i}\", \"fields\": {{\"text\": \"t{i}\"}}}}\n"))
            .collect();
        let file = self.dir.join("notes.ndjson");
        fs::write(&file, rows).unwrap();
        let ingest = self.run(&["ingest", &file.display().to_string(), "--source", "erp"]);
        assert_eq!(ingest.code, 0, "{}", ingest.err);
    }

    fn close(&self) {
        let held: Vec<_> = self.held.lock().unwrap().values().cloned().collect();
        self.held.lock().unwrap().clear();
        for store in held {
            block_on(store.close()).unwrap();
        }
    }

    /// One letter of the declaration's note changed, at its own length:
    /// the position of the event that holds it.
    fn tamper(&self) -> u64 {
        match &self.pg {
            None => {
                let segment =
                    find(&self.dir, "00000000000000000000.jsonl").expect("the lake's segment");
                let text = fs::read_to_string(&segment).unwrap();
                let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
                let at = lines
                    .iter()
                    .position(|l| l.contains(NOTE))
                    .expect("the declaration's line");
                lines[at] = lines[at].replacen(NOTE, &NOTE.replacen('A', "B", 1), 1);
                fs::write(&segment, lines.join("\n")).unwrap();
                // The first segment begins at position 0, and each event
                // line before the declaration's holds one position; a
                // frame line holds none.
                let frame = |l: &String| {
                    let line: serde_json::Value = serde_json::from_str(l).unwrap();
                    lakelands_core::log::is_frame(&line)
                };
                lines[..at].iter().filter(|l| !frame(l)).count() as u64
            }
            Some((url, schema)) => {
                let (url, schema) = (url.clone(), schema.clone());
                // On a thread of its own: the synchronous client runs a
                // runtime of its own, which a test's runtime thread cannot.
                std::thread::spawn(move || {
                    let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
                    let sql = format!(
                        "UPDATE \"{schema}\".log SET bytes = convert_to(replace(convert_from(bytes, 'UTF8'), \
                         'A note', 'B note'), 'UTF8') WHERE convert_from(bytes, 'UTF8') LIKE '%A note%' \
                         RETURNING position"
                    );
                    let rows = client.query(&sql, &[]).unwrap();
                    assert_eq!(rows.len(), 1, "one event holds the note");
                    rows[0].get::<_, i64>(0) as u64
                })
                .join()
                .unwrap()
            }
        }
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let held: Vec<_> = self.held.lock().unwrap().values().cloned().collect();
        for store in held {
            let _ = block_on(store.close());
        }
        if let Some((url, schema)) = self.pg.clone() {
            let _ = std::thread::spawn(move || {
                if let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) {
                    let _ = client.batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"));
                }
            })
            .join();
        }
    }
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let path = e.path();
        if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
        path.is_dir().then(|| find(&path, name))?
    })
}

/// SQLite, and PostgreSQL where the environment names a server.
fn servers() -> Vec<Option<String>> {
    let pg = std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty());
    vec![None, pg]
        .into_iter()
        .enumerate()
        .filter(|(i, s)| *i == 0 || s.is_some())
        .map(|(_, s)| s)
        .collect()
}

const HOLDS: &str =
    "every event links to the one before it, and every checkpoint matches its segments (last hash ";

#[test]
fn verify_says_a_whole_log_holds_beside_its_writer_and_reads_only() {
    for pg in servers() {
        let name = if pg.is_some() { "holds_pg" } else { "holds" };
        let root = Root::new(name, pg.as_deref());
        root.fill();
        // The writer stays open, holding the lease: `verify` reads beside it.
        let asked = Mutex::new(Vec::new());
        let s = root.verify(&[], &asked);
        assert_eq!(s.code, 0, "{}{}", s.out, s.err);
        assert!(
            s.out
                .starts_with("lake `ledger`: the log holds from position 0 through "),
            "{}",
            s.out
        );
        assert!(
            s.out.contains(HOLDS) && s.out.lines().count() == 1,
            "{}",
            s.out
        );
        assert_eq!(*asked.lock().unwrap(), [OpenMode::Read]);
        let s = root.verify(&["--from", "1", "--through", "3"], &asked);
        assert_eq!(s.code, 0, "{}{}", s.out, s.err);
        assert!(
            s.out
                .starts_with("lake `ledger`: the log holds from position 1 through 3: "),
            "{}",
            s.out
        );
    }
}

#[test]
fn verify_finds_a_line_changed_by_hand_that_an_open_does_not() {
    for pg in servers() {
        let name = if pg.is_some() {
            "tampered_pg"
        } else {
            "tampered"
        };
        let root = Root::new(name, pg.as_deref());
        root.fill();
        root.close();
        let at = root.tamper();
        // The lake opens, and is written on: an open reads the log's ends.
        let again = root.dir.join("again.ndjson");
        fs::write(
            &again,
            "{\"type\": \"note\", \"key\": \"n4\", \"fields\": {\"text\": \"t4\"}}\n",
        )
        .unwrap();
        let ingest = root.run(&["ingest", &again.display().to_string(), "--source", "erp"]);
        assert_eq!(ingest.code, 0, "{}", ingest.err);
        root.close();
        let asked = Mutex::new(Vec::new());
        let s = root.verify(&[], &asked);
        assert_eq!(
            (s.code, s.out.as_str(), s.err.as_str()),
            (
                1,
                format!(
                    "lake `ledger`: the log is damaged at position {at}: the hash does not match the event\n  \
                     what to do: restore the lake from a backup taken before position {at}; nothing was changed\n"
                )
                .as_str(),
                ""
            )
        );
        assert_eq!(*asked.lock().unwrap(), [OpenMode::Read]);
    }
}

#[test]
fn verify_answers_a_log_the_open_refuses_as_damaged_and_a_lake_not_made_yet() {
    let root = Root::new("cut", None);
    root.fill();
    root.close();
    // A line one letter longer: the engine's index of the log no longer
    // meets it, and the open refuses the lake.
    let segment = find(&root.dir, "00000000000000000000.jsonl").unwrap();
    let text = fs::read_to_string(&segment).unwrap();
    fs::write(&segment, text.replacen(NOTE, &format!("{NOTE}!"), 1)).unwrap();
    let asked = Mutex::new(Vec::new());
    let s = root.verify(&[], &asked);
    assert_eq!(s.code, 1, "{}{}", s.out, s.err);
    assert!(
        s.out.starts_with("lake `ledger`: the log is damaged"),
        "{}",
        s.out
    );
    assert!(
        s.out
            .contains("\n  what to do: restore the lake from a backup"),
        "{}",
        s.out
    );
    assert_eq!(*asked.lock().unwrap(), [OpenMode::Read]);
    // A lake whose location is written and that nothing has opened yet is
    // not made by checking it.
    let fresh = Root::new("fresh", None);
    let s = fresh.verify(&[], &asked);
    assert_eq!(
        (s.code, s.out.as_str(), s.err.as_str()),
        (
            3,
            "",
            "error: there is no lake `ledger` to verify yet\n  what to do: a lake is made by the first command that writes to it\n"
        )
    );
    assert!(
        find(&fresh.dir, "00000000000000000000.jsonl").is_none(),
        "verify made the lake"
    );
}

#[test]
fn verify_answers_a_range_outside_the_log_as_the_command_line_s() {
    let root = Root::new("range", None);
    root.fill();
    let asked = Mutex::new(Vec::new());
    let s = root.verify(&["--from", "3", "--through", "1"], &asked);
    assert_eq!(
        (s.code, s.out.as_str(), s.err.as_str()),
        (
            2,
            "",
            "error: --from 3 is past --through 1\n  what to do: give a --from no later than --through\n"
        )
    );
    let s = root.verify(&["--from", "999"], &asked);
    assert_eq!((s.code, s.out.as_str()), (2, ""), "{}", s.err);
    assert!(s.err.starts_with("error: "), "{}", s.err);
    // A `--through` past the head, as far as a position goes, reads the
    // log to its head: the window it asks is never computed past it.
    let s = root.verify(&["--through", &u64::MAX.to_string()], &asked);
    assert_eq!(s.code, 0, "{}{}", s.out, s.err);
    let holds = "lake `ledger`: the log holds from position 0 through ";
    assert!(
        s.out.starts_with(holds) && s.out.contains(HOLDS),
        "{}",
        s.out
    );
}

#[test]
fn verify_tells_a_log_without_its_projection_and_a_memory_lake_from_no_lake() {
    let root = Root::new("bare", None);
    root.fill();
    root.close();
    // A backup of `log/` and `index/` alone, as `docs/hosting.md` advises.
    let db = find(&root.dir, "projection.db").expect("the projection");
    for end in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{end}", db.display()));
    }
    let asked = Mutex::new(Vec::new());
    let s = root.verify(&[], &asked);
    assert_eq!(
        (s.code, s.out.as_str(), s.err.as_str()),
        (
            3,
            "",
            "error: the log of lake `ledger` is there, its projection is not\n  what to do: `lakelands rebuild` makes the projection from the log; verify then reads it\n"
        )
    );
    let memory = Root::new("memory", None);
    fs::write(
        memory.dir.join("lakes/ledger.json"),
        r#"{"lake": "ledger", "engine": {"engine": "memory"}}"#,
    )
    .unwrap();
    let s = memory.verify(&[], &asked);
    assert_eq!(
        (s.code, s.out.as_str(), s.err.as_str()),
        (
            3,
            "",
            "error: lake `ledger` is kept in memory: a memory lake lives only in the process that runs it, so there is nothing to verify\n  what to do: verify a lake kept on disk or in a database\n"
        )
    );
}
