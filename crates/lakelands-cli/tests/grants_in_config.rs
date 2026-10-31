//! The grants of a root's `lakelands.json`, at the door that reads them: a
//! grant is there to narrow, so one that is not understood stops the root
//! and leaves the lake as it was, and a grant naming a type no lake declares
//! is told at start-up and starts. Each runs the binary, as an operator does.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const ARTICLE: &str = "name: article\nfields:\n  slug: { type: text, authority: source }\n  topic: { type: text, authority: source }\n";

struct Lakes {
    dir: PathBuf,
}

impl Lakes {
    /// A root in a fresh directory, holding `news`, whose `article` holds
    /// two records of the topic `rivers` and one of another.
    fn with_articles(name: &str) -> Lakes {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("grants-in-config")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bundle/types")).unwrap();
        let root = Lakes { dir };
        assert!(root.run(&["init"]).status.success());
        assert!(root.run(&["lake", "new", "news"]).status.success());
        fs::write(root.dir.join("bundle/types/article.yaml"), ARTICLE).unwrap();
        let bundle = root.dir.join("bundle");
        let done = root.run(&["apply", "--lake", "news", bundle.to_str().unwrap()]);
        assert!(done.status.success(), "{}", err(&done));
        let lines: String = [("a", "rivers"), ("b", "rivers"), ("c", "lakes")]
            .map(|(slug, topic)| {
                format!(
                    "{{\"type\": \"article\", \"key\": \"{slug}\", \"fields\": {{\"slug\": \"{slug}\", \"topic\": \"{topic}\"}}}}\n"
                )
            })
            .concat();
        let file = root.dir.join("articles.ndjson");
        fs::write(&file, lines).unwrap();
        let fed = root.run(&[
            "ingest",
            "--lake",
            "news",
            file.to_str().unwrap(),
            "--source",
            "wire",
        ]);
        assert!(fed.status.success(), "{}", err(&fed));
        root
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lakelands"))
            .arg("--root")
            .arg(&self.dir)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// The root's configuration, with these principals and any `more`.
    fn configure(&self, principals: Value, more: Value) {
        let file = self.dir.join("lakelands.json");
        let mut config: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        config["principals"] = principals;
        for (key, value) in more.as_object().into_iter().flatten() {
            config[key] = value.clone();
        }
        fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    }

    /// Every file of the lake's directory, by path, with its bytes.
    fn lake_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                match entry.path() {
                    p if p.is_dir() => walk(&p, into),
                    p => drop(into.insert(p.clone(), fs::read(p).unwrap())),
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(&self.dir.join("lakes/news"), &mut files);
        files
    }

    /// The slugs `clerk` is answered.
    fn read_as_clerk(&self) -> Output {
        let rows = "MATCH (a:article) RETURN a.slug ORDER BY a.slug";
        self.run(&["query", "--lake", "news", "--cypher", rows, "--as", "clerk"])
    }

    /// `serve` with no door, as `clerk`, its client gone at once.
    fn serve_as_clerk(&self) -> Output {
        self.run(&["serve", "--lake", "news", "--mcp", "--as", "clerk"])
    }
}

fn err(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn clerk(grants: Value) -> Value {
    json!([{"name": "clerk", "kind": "agent", "id": "clerk", "grants": grants}])
}

#[test]
fn a_grant_holding_a_key_it_does_not_know_stops_the_root() {
    let root = Lakes::with_articles("unknown-key");
    let rivers = json!({"topic": "rivers"});
    let on_article = |grant: Value| json!({"default": "read", "types": {"article": grant}});
    let hidden = |extra: Value| {
        let mut hiding =
            json!({"field": "topic", "values": ["rivers"], "fields": {"slug": "none"}});
        hiding["notes"] = extra;
        on_article(json!({"hide_where": [hiding]}))
    };
    let mut on_the_grant = json!({"default": "read"});
    on_the_grant["typez"] = json!({"article": {"only": rivers}});
    for (key, grants) in [
        ("onyl", on_article(json!({"onyl": rivers}))),
        (
            "only_anyy",
            on_article(json!({"only_anyy": {"topic": ["rivers"]}})),
        ),
        ("writes_onyl", on_article(json!({"writes_onyl": rivers}))),
        (
            "whole_set_only",
            on_article(json!({"only": rivers, "whole_set_only": true})),
        ),
        ("typez", on_the_grant.clone()),
        ("notes", hidden(json!("a hiding is strict too"))),
    ] {
        root.configure(clerk(grants), json!({}));
        let before = root.lake_files();
        for refused in [root.serve_as_clerk(), root.read_as_clerk()] {
            let said = err(&refused);
            assert!(!refused.status.success(), "`{key}` started: {said}");
            assert!(said.contains(&format!("`{key}`")), "no key: {said}");
            assert!(said.contains("`clerk`"), "no principal: {said}");
        }
        assert!(root.lake_files() == before, "`{key}` touched the lake");
    }
    // The same grant, spelt as it is meant, starts and narrows.
    root.configure(clerk(on_article(json!({"only": rivers}))), json!({}));
    assert!(root.serve_as_clerk().status.success());
    let read = root.read_as_clerk();
    let rows = String::from_utf8_lossy(&read.stdout);
    assert!(
        rows.contains(r#""a""#) && !rows.contains(r#""c""#),
        "{rows}"
    );
}

/// The file's order decides which fault is met first, and whose: written by
/// hand, since a `Value` would sort the keys, with the name after the grant.
#[test]
fn the_refusal_names_the_principal_the_file_meets_first() {
    let root = Lakes::with_articles("first-met");
    fs::write(
        root.dir.join("lakelands.json"),
        r#"{"lakelands": 1, "owner": "owner", "principals": [
        {"name": "fine", "kind": "agent", "id": "fine", "grants": {"default": "read"}},
        {"kind": "agent", "id": "clerk",
         "grants": {"default": "read", "typez": {}, "reviw": true}, "name": "clerk"},
        {"name": "later", "kind": "agent", "id": "later", "grants": {"typez": {}}},
        {"name": "lacking", "kind": "agent", "id": "lacking"}]}"#,
    )
    .unwrap();
    let said = err(&root.serve_as_clerk());
    assert!(said.contains("`typez`"), "no key: {said}");
    assert!(said.contains("`clerk`"), "no principal: {said}");
    assert!(!said.contains("`later`"), "another's name: {said}");
}

#[test]
fn a_grant_naming_a_type_no_lake_declares_is_told_and_starts() {
    let root = Lakes::with_articles("unknown-type");
    let told = |out: &Output, ty: &str| err(out).contains(&format!("`{ty}`"));
    let rivers = json!({"only": {"topic": "rivers"}});
    root.configure(
        clerk(json!({"default": "read", "types": {"artcle": rivers}})),
        json!({}),
    );
    for started in [root.serve_as_clerk(), root.read_as_clerk()] {
        assert!(started.status.success(), "{}", err(&started));
        assert!(told(&started, "artcle"), "not told: {}", err(&started));
        assert!(err(&started).contains("`clerk`"), "{}", err(&started));
    }
    // A type declared, and one the store declares itself, are not told.
    root.configure(
        clerk(json!({"default": "read", "types": {"article": rivers, "sys.setting": {}}})),
        json!({}),
    );
    for started in [root.serve_as_clerk(), root.read_as_clerk()] {
        assert!(started.status.success(), "{}", err(&started));
        assert!(!err(&started).contains("clerk"), "{}", err(&started));
    }
}

#[test]
fn the_door_tells_each_principal_by_the_lakes_it_addresses() {
    let root = Lakes::with_articles("by-lakes");
    assert!(root.run(&["lake", "new", "hr"]).status.success());
    let grants = json!({"default": "read", "types": {"article": {}}});
    root.configure(
        json!([
            {"name": "anywhere", "kind": "agent", "id": "anywhere", "grants": grants},
            {"name": "hr-only", "kind": "agent", "id": "hr-only", "lakes": ["hr"], "grants": grants},
        ]),
        json!({"server": {"bind": "127.0.0.1:0"}}),
    );
    // `news` declares `article`, whichever lake is the primary one; the
    // other lakes open only behind the door.
    let served = root.run(&["serve", "--lake", "hr", "--mcp"]);
    assert!(served.status.success(), "{}", err(&served));
    assert!(err(&served).contains("`hr-only`"), "{}", err(&served));
    assert!(!err(&served).contains("`anywhere`"), "{}", err(&served));
}
