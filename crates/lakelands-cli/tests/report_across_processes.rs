//! A preflight and its act as two processes of the binary
//! (lakelands#1469): `purge`, `forget` and `values remove` act under the
//! `--report` id their preflight printed in a process of its own, though
//! each process that opens the lake for writing appends its own
//! `lease_taken` between the two; a record written between them still
//! refuses the act. On SQLite, and on PostgreSQL when
//! `LAKELANDS_TEST_PG_URL` names a server (`scripts/conformance.sh pg`
//! runs this file against one).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A root with the lake `drill`, on SQLite or in a schema of its own on
/// the PostgreSQL server at `pg`, whose keyring seals what sources bring.
struct Root {
    dir: PathBuf,
    pg: Option<(String, String)>,
}

struct Said {
    ok: bool,
    out: String,
    err: String,
}

const TYPES: &str =
    "name: contact\nfields:\n  email: { type: text, authority: source, erasable: true }\n";

impl Root {
    fn new(name: &str, pg: Option<&str>) -> Root {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("report-across")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        let schema = format!("lakelands_test_report_{name}_{}", std::process::id());
        let root = Root {
            dir,
            pg: pg.map(|url| (url.to_owned(), schema)),
        };
        let at = root.dir.display().to_string();
        root.ok(&["init", &at], false);
        let config = r#"{"lakelands": 1, "owner": "owner", "keys": {"keyring": "keys"}}"#;
        fs::write(root.dir.join("lakelands.json"), config).unwrap();
        match &root.pg {
            None => root.ok(&["lake", "new", "drill"], true),
            Some((url, schema)) => root.ok(
                &[
                    "lake", "new", "drill", "--engine", "postgres", "--url", url, "--schema",
                    schema, "--no-tls",
                ],
                true,
            ),
        };
        let types = root.dir.join("bundle").join("types");
        fs::create_dir_all(&types).unwrap();
        fs::write(types.join("contact.yaml"), TYPES).unwrap();
        root.ok(
            &["apply", &root.dir.join("bundle").display().to_string()],
            true,
        );
        root
    }

    /// `lakelands [--root <root>] <args>` in a process of its own.
    fn run(&self, args: &[&str], rooted: bool) -> Said {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lakelands"));
        if rooted {
            cmd.arg("--root").arg(&self.dir);
        }
        let out = cmd.args(args).output().expect("the binary runs");
        Said {
            ok: out.status.success(),
            out: String::from_utf8(out.stdout).unwrap(),
            err: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn ok(&self, args: &[&str], rooted: bool) -> String {
        let said = self.run(args, rooted);
        assert!(
            said.ok,
            "`lakelands {}` failed:\n{}{}",
            args.join(" "),
            said.out,
            said.err
        );
        said.out
    }

    /// `source` brings one contact in, `key`.
    fn ingest(&self, source: &str, key: &str) {
        let rows = self.dir.join(format!("{source}-{key}.ndjson"));
        let row = format!(
            "{{\"type\": \"contact\", \"key\": \"{key}\", \"fields\": {{\"email\": \"{key}@example.org\"}}}}\n"
        );
        fs::write(&rows, row).unwrap();
        self.ok(
            &["ingest", &rows.display().to_string(), "--source", source],
            true,
        );
    }

    /// The id a preflight printed, in a process of its own.
    fn preflight(&self, args: &[&str]) -> String {
        let out = self.ok(&[args, &["--preflight"]].concat(), true);
        let id = out.rsplit("--report ").next().map(str::trim);
        let id = id.filter(|id| out.contains("--report ") && !id.is_empty());
        id.unwrap_or_else(|| panic!("no report id in:\n{out}"))
            .to_owned()
    }

    /// The act under `id`, in a process of its own.
    fn act(&self, args: &[&str], id: &str) -> Said {
        self.run(&[args, &["--report", id]].concat(), true)
    }

    /// How many `lease_taken`s the log holds.
    fn leases(&self) -> usize {
        let feed = self.ok(&["feed", "--kind", "lease_taken"], true);
        feed.lines().filter(|l| !l.trim().is_empty()).count()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
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

/// Each engine the test runs on: its name, and the server's URL for
/// PostgreSQL.
fn engines() -> Vec<(&'static str, Option<String>)> {
    let mut out = vec![("sqlite", None)];
    if let Some(url) = std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        out.push(("pg", Some(url)));
    }
    out
}

/// The three acts, each preflighted in one process and acted on in
/// another, with a third process's lease between for the purge; none is
/// refused, and each appended the leases that made the head move.
#[test]
fn an_act_goes_through_under_a_preflight_another_process_printed() {
    for (engine, url) in engines() {
        let root = Root::new(&format!("acts_{engine}"), url.as_deref());
        for source in ["bank", "crm", "tracker"] {
            root.ingest(source, &format!("{source}-1"));
        }
        let before = root.leases();

        let purge = ["purge", "--source", "bank"];
        let id = root.preflight(&purge);
        // Other processes' leases between the two move no record, more
        // of them than the store reads back at once.
        root.preflight(&["forget", "--source", "crm"]);
        for _ in 0..17 {
            root.preflight(&["purge", "--source", "crm"]);
        }
        let done = root.act(&purge, &id);
        assert!(
            done.ok && done.out.contains("purged bank through position "),
            "{engine}: purge: {}{}",
            done.out,
            done.err
        );

        let forget = ["forget", "--source", "crm"];
        let id = root.preflight(&forget);
        let done = root.act(&forget, &id);
        assert!(
            done.ok && done.out.contains("forgot crm through position "),
            "{engine}: forget: {}{}",
            done.out,
            done.err
        );

        // Everything `tracker` wrote before now.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let cut = lakelands_api::Timestamp(now.unwrap().as_micros() as i64).to_string();
        let remove = ["values", "remove", "--source", "tracker", "--before", &cut];
        let id = root.preflight(&remove);
        let done = root.act(&remove, &id);
        assert!(
            done.ok && done.out.contains("through position "),
            "{engine}: values remove: {}{}",
            done.out,
            done.err
        );

        // Each preflight and each act took the lease in its own process.
        let taken = root.leases() - before;
        assert!(taken >= 24, "{engine}: {taken} leases taken");
    }
}

/// A record written between the preflight and the act, by another
/// process, refuses each of the three acts: the lake moved by more than a
/// lease. A preflight after the write is acted on.
#[test]
fn a_write_between_the_preflight_and_the_act_still_refuses_it() {
    for (engine, url) in engines() {
        let root = Root::new(&format!("moved_{engine}"), url.as_deref());
        for source in ["bank", "crm", "tracker"] {
            root.ingest(source, &format!("{source}-1"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let cut = lakelands_api::Timestamp(now.unwrap().as_micros() as i64).to_string();
        let acts: [(&[&str], &str); 4] = [
            (&["purge", "--source", "bank"], "not of this purge"),
            (&["forget", "--source", "crm"], "not of this forgetting"),
            (
                &["values", "remove", "--source", "tracker", "--before", &cut],
                "not of this removal",
            ),
            // A write of the act's own source moves the lake as much.
            (&["purge", "--source", "bank"], "not of this purge"),
        ];
        for (n, (act, said)) in acts.iter().enumerate() {
            let id = root.preflight(act);
            let writer = if n == 3 { "bank" } else { "other" };
            root.ingest(writer, &format!("{writer}-{n}"));
            let stale = root.act(act, &id);
            assert!(
                !stale.ok && stale.err.contains(said),
                "{engine}: `{}` after a write: {}{}",
                act.join(" "),
                stale.out,
                stale.err
            );
        }
        for (act, _) in &acts[..3] {
            let id = root.preflight(act);
            let done = root.act(act, &id);
            assert!(
                done.ok && done.out.contains("through position "),
                "{engine}: `{}`: {}{}",
                act.join(" "),
                done.out,
                done.err
            );
        }
    }
}
