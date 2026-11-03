//! A preflight and its act as two processes of the binary
//! (lakelands#1469): `purge`, `forget` and `values remove` act under the
//! `--report` id their preflight printed in a process of its own, and
//! `apply --plan` under the id `plan` printed (lakelands#1476), though
//! each process that opens the lake for writing appends its own
//! `lease_taken` between the two; a record written between them refuses a
//! forgetting, and a removal or purge refuses one that reaches a record
//! its report named and takes one that reaches nothing of it (099). On
//! SQLite, and on PostgreSQL when `LAKELANDS_TEST_PG_URL` names a server
//! (`scripts/conformance.sh pg` runs this file against one).

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
        self.ingest_with(source, key, &format!("{key}@example.org"));
    }

    /// One record of `ty` `contact` at `key`, its `email` `email`.
    fn ingest_with(&self, source: &str, key: &str, email: &str) {
        let rows = self.dir.join(format!("{source}-{key}.ndjson"));
        let row = format!(
            "{{\"type\": \"contact\", \"key\": \"{key}\", \"fields\": {{\"email\": \"{email}\"}}}}\n"
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

/// A write between the preflight and the act refuses forget; `values
/// remove` and a purge refuse it only where the write reached a record
/// their report named. A removal and a purge take a write elsewhere, and
/// the purge's own source's later write; a preflight after a refusal is
/// acted on.
#[test]
fn a_write_between_the_preflight_and_the_act_refuses_what_it_reached() {
    for (engine, url) in engines() {
        let root = Root::new(&format!("moved_{engine}"), url.as_deref());
        for source in ["bank", "crm", "tracker", "ledger"] {
            root.ingest(source, &format!("{source}-1"));
        }
        let cut = || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            lakelands_api::Timestamp(now.unwrap().as_micros() as i64).to_string()
        };
        let first = cut();

        // A forgetting's report is of the head it read: a write since, of
        // any source, refuses it, and a preflight after the refusal is
        // acted on.
        let forget = ["forget", "--source", "crm"];
        let id = root.preflight(&forget);
        root.ingest("other", "other-0");
        let stale = root.act(&forget, &id);
        assert!(
            !stale.ok && stale.err.contains("not of this forgetting"),
            "{engine}: `forget` after a write: {}{}",
            stale.out,
            stale.err
        );
        let id = root.preflight(&forget);
        let done = root.act(&forget, &id);
        assert!(
            done.ok && done.out.contains("forgot crm through position "),
            "{engine}: `forget` after a fresh preflight: {}{}",
            done.out,
            done.err
        );

        // A removal's report names the records the selection took at its
        // position (099): a write elsewhere is taken in with it.
        let remove = [
            "values",
            "remove",
            "--source",
            "tracker",
            "--before",
            first.as_str(),
        ];
        let id = root.preflight(&remove);
        root.ingest("other", "other-1");
        let done = root.act(&remove, &id);
        assert!(
            done.ok && done.out.contains("through position "),
            "{engine}: `values remove` after a write elsewhere: {}{}",
            done.out,
            done.err
        );

        // A write that reaches a record the report named refuses it, and
        // a preflight after the refusal is acted on.
        root.ingest("tracker", "tracker-2");
        root.ingest("tracker", "tracker-3");
        let second = cut();
        let remove = [
            "values",
            "remove",
            "--source",
            "tracker",
            "--before",
            second.as_str(),
        ];
        let id = root.preflight(&remove);
        // The same value would change nothing; `other`'s must differ.
        root.ingest_with("other", "tracker-2", "other@example.org");
        let stale = root.act(&remove, &id);
        assert!(
            !stale.ok && stale.err.contains("not of this removal"),
            "{engine}: `values remove` on a record the report named: {}{}",
            stale.out,
            stale.err
        );
        let id = root.preflight(&remove);
        let done = root.act(&remove, &id);
        assert!(
            done.ok && done.out.contains("through position "),
            "{engine}: `values remove` after a fresh preflight: {}{}",
            done.out,
            done.err
        );

        let purge = ["purge", "--source", "bank"];
        let id = root.preflight(&purge);
        root.ingest("other", "elsewhere");
        let done = root.act(&purge, &id);
        assert!(
            done.ok && done.out.contains("purged bank through position "),
            "{engine}: a write elsewhere: {}{}",
            done.out,
            done.err
        );

        // The source's own write after the report is what the purge
        // takes in, not what refuses it.
        let id = root.preflight(&purge);
        root.ingest("bank", "bank-late");
        let done = root.act(&purge, &id);
        assert!(
            done.ok && done.out.contains("purged bank through position "),
            "{engine}: the source's own later write: {}{}",
            done.out,
            done.err
        );

        // A write that reaches a record the report named refuses it.
        let purge = ["purge", "--source", "ledger"];
        let id = root.preflight(&purge);
        // The same value would change nothing; `other`'s must differ.
        root.ingest_with("other", "ledger-1", "other@example.org");
        let stale = root.act(&purge, &id);
        assert!(
            !stale.ok && stale.err.contains("not of this purge"),
            "{engine}: a write on a record the report named: {}{}",
            stale.out,
            stale.err
        );
        // The record is another source's now, so the report names one
        // the act can no longer read as `ledger`'s; a fresh preflight and
        // act have nothing to retire, and append nothing.
        let id = root.preflight(&purge);
        let done = root.act(&purge, &id);
        assert!(
            done.ok && done.out.contains("nothing to retire for ledger"),
            "{engine}: a purge after a fresh preflight: {}{}",
            done.out,
            done.err
        );
    }
}

/// `plan` and `apply --plan` as two processes (lakelands#1476): a record
/// written between them refuses the apply as stale; another process's
/// lease between them does not, and the bundle is applied under the id.
#[test]
fn a_bundle_is_applied_under_a_plan_another_process_printed() {
    for (engine, url) in engines() {
        let root = Root::new(&format!("plan_{engine}"), url.as_deref());
        root.ingest("bank", "bank-1");
        let bundle = root.dir.join("bundle");
        let note = "name: note\nfields:\n  text: { type: text }\n";
        fs::write(bundle.join("types").join("note.yaml"), note).unwrap();
        let at = bundle.display().to_string();
        let plan = || {
            let out = root.ok(&["plan", &at], true);
            let id = out
                .rsplit("--plan ")
                .next()
                .map(|id| id.trim().trim_end_matches('`'));
            let id = id.filter(|id| out.contains("--plan ") && !id.is_empty());
            id.unwrap_or_else(|| panic!("no plan id in:\n{out}"))
                .to_owned()
        };
        let id = plan();
        root.ingest("crm", "crm-1");
        let stale = root.run(&["apply", &at, "--plan", &id], true);
        assert!(
            !stale.ok && stale.err.contains("the lake moved since that plan"),
            "{engine}: an apply after a write: {}{}",
            stale.out,
            stale.err
        );
        let id = plan();
        root.ok(&["plan", &at], true);
        let done = root.run(&["apply", &at, "--plan", &id], true);
        assert!(done.ok, "{engine}: apply --plan: {}{}", done.out, done.err);
    }
}
