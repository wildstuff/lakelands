//! Where the make after a recomputation stands, live and rebuilt
//! (lakelands#631). A declaration that computes a view's source again at
//! p1 ends the kept view there, and the flush makes it again where the
//! recomputed records are put: before the first later event that reads a
//! record, or at the transaction's end. Live, with p1 an append of its
//! own, that is p1; a rebuild whose page holds p1, a declaration that
//! reads no record at p2 and a write at p3 makes it at p2. Every read of
//! the records, the view's among them, answers alike at every position;
//! what differs is the projection's position of the make and, as of p1,
//! `describe_type`'s `kept`, which describes the projection.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode, Scan};
use lakelands_api::read::{Find, Found, Get, ReadAt};
use lakelands_api::seam::{Seams, kept_name};
use lakelands_api::{LakeStore, Location, Position, TypeName};
use lakelands_core::conformance::fixture::Lake;
use lakelands_engine_mem::MemoryOpener;
use serde_json::{Value, json};

const VIEWS: [&str; 2] = ["stock", "shelf"];

/// `scaled`, computed in the lake as `expr` over `net`.
fn scaled(expr: &str) -> Value {
    json!({"declare": "function", "name": "scaled", "backing": "expr", "expr": expr,
           "inputs": [{"name": "net", "type": {"kind": "decimal"}}]})
}

/// `item`, with `gross` by `scaled` and `fee` derived as `fee`.
fn item(fee: &str) -> Value {
    let field = |n: &str| json!({"name": n, "type": {"kind": "decimal"}});
    json!({"declare": "type", "name": "item", "fields": [
        {"name": "g", "type": {"kind": "text"}}, field("net"),
        {"name": "gross", "type": {"kind": "decimal"}, "authority": "derived", "function": "scaled"},
        {"name": "fee", "type": {"kind": "decimal"}, "authority": "derived", "derived": fee}]})
}

/// The view `name` of `source` by `g`, with `measures` of (name, kind,
/// expression).
fn view(name: &str, source: &str, measures: &[(&str, &str, &str)]) -> Value {
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let fields = (measures.iter()).map(|(n, kind, _)| field(n, kind));
    let fields: Vec<Value> = std::iter::once(field("g", "text")).chain(fields).collect();
    let measures: Vec<Value> = (measures.iter())
        .map(|(n, _, expr)| json!({"name": n, "expr": expr}))
        .collect();
    json!({"declare": "type", "name": name, "key": ["g"], "fields": fields,
           "view": {"source": source, "group_by": ["g"], "measures": measures}})
}

/// A type no view reads, whose declaration reads no record.
fn other() -> Value {
    json!({"declare": "type", "name": "other", "fields": [{"name": "x", "type": {"kind": "text"}}]})
}

/// One position, as a reader is answered and as the projection holds it.
#[derive(Debug, PartialEq)]
struct At {
    /// Every read of the records: `find` of `item` and the views, whole,
    /// `get` of each group, and each view beside its twin.
    reads: Vec<String>,
    /// `describe_type`'s `kept` of each view.
    described: Vec<String>,
    /// `item`'s records: fields, version, position.
    items: Vec<String>,
    /// Each view's kept records, and its state.
    kept: Vec<String>,
}

/// A lake with `item`, the kept views `stock` and `shelf` over it and
/// their twins, which no setting names, and three records.
struct Kept {
    store: Arc<dyn LakeStore>,
    l: Lake,
    opener: MemoryOpener,
    name: String,
}

impl Kept {
    async fn new(name: &str) -> Kept {
        let opener = MemoryOpener::new();
        let seams = Seams {
            engines: vec![Arc::new(opener.clone())],
            ..Seams::default()
        };
        let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
            .await
            .expect("a new lake");
        let l = Lake::of(store.as_ref());
        let k = Kept {
            store,
            l,
            opener,
            name: name.to_owned(),
        };
        let stock = [
            ("total", "decimal", "sum(gross)"),
            ("fees", "decimal", "sum(fee)"),
            ("c", "number", "count()"),
        ];
        let shelf = [("most", "decimal", "max(total)")];
        for d in [
            scaled("net * 2"),
            item("net + 1"),
            view("stock", "item", &stock),
            view("stock_twin", "item", &stock),
            view("shelf", "stock", &shelf),
            view("shelf_twin", "stock_twin", &shelf),
        ] {
            k.declare(d).await;
        }
        for (key, g, net) in [("a", "x", "1"), ("b", "x", "2"), ("c", "y", "3")] {
            let wrote = k.l.upsert("item", key, json!({"g": g, "net": net})).await;
            wrote.expect("a write");
        }
        for v in VIEWS {
            let kept = k.l.set(&format!("view_kept.{v}"), json!(true)).await;
            kept.expect("asking for the view to be kept");
        }
        k
    }

    async fn declare(&self, d: Value) {
        self.l.declare_json(d).await.expect("declaring");
    }

    async fn head(&self) -> Position {
        self.store.head().await.expect("the head")
    }

    /// `find` of `ty` as of `p`, with meta and total: the reply whole.
    async fn find(&self, ty: &str, p: Position) -> String {
        let find = Find {
            at: of(p),
            with_meta: true,
            total: true,
            ..Find::default()
        };
        let found = self.l.cat.find(&self.l.own, &ty.into(), find).await;
        let found = found.expect("a read");
        json!({"position": found.position, "data": found.data}).to_string()
    }

    /// The rows of `ty` as of `p`, under the name `ty` without `_twin`.
    async fn rows(&self, ty: &str, p: Position) -> String {
        let find = Find {
            at: of(p),
            ..Find::default()
        };
        let found = self.l.cat.find(&self.l.own, &ty.into(), find).await;
        match found.expect("a read").data {
            Found::Records { rows, .. } => json!(rows).to_string().replace("_twin", ""),
            other => panic!("records: {other:?}"),
        }
    }

    async fn get(&self, ty: &str, key: &str, p: Position) -> String {
        let get = Get {
            key: key.into(),
            with_meta: true,
            params: Default::default(),
            at: of(p),
        };
        let got = self.l.cat.get(&self.l.own, &ty.into(), get).await;
        let got = got.expect("a read");
        json!({"position": got.position, "data": got.data}).to_string()
    }

    async fn at(&self, p: Position) -> At {
        let mut reads = Vec::new();
        for ty in ["item", "stock", "shelf"] {
            reads.push(self.find(ty, p).await);
        }
        let (mut described, mut kept) = (Vec::new(), Vec::new());
        for v in VIEWS {
            for key in ["x", "y"] {
                reads.push(self.get(v, key, p).await);
            }
            let (own, twin) = (
                self.rows(v, p).await,
                self.rows(&format!("{v}_twin"), p).await,
            );
            assert_eq!(
                own, twin,
                "{}: `{v}` as of {p} answers as its twin",
                self.name
            );
            let d = self
                .l
                .cat
                .describe_type(&self.l.own, &v.into(), of(p))
                .await;
            described.push(json!(d.expect("describing").data.lifecycle.kept).to_string());
        }
        let location = Location::memory(self.name.as_str());
        let engine = (self.opener.open(&location, OpenMode::Read)).expect("a second handle");
        let view = engine.view(Some(p)).expect("a view");
        let scan = Scan {
            limit: 100,
            retired: true,
            restricted: true,
            ..Scan::default()
        };
        let held = |ty: &TypeName| {
            let rows = view.scan(ty, &scan).expect("a scan");
            let row = |r: lakelands_api::record::StoredRecord| {
                let fields = json!(r.fields).to_string();
                format!(
                    "{} {} v{} @{} {fields}",
                    r.ty, r.key, r.version.0, r.position.0
                )
            };
            rows.into_iter().map(row).collect::<Vec<_>>()
        };
        let items = held(&"item".into());
        for v in VIEWS {
            kept.extend(held(&kept_name(&v.into())));
            let input = lakelands_store::backed::digest(&Value::from(v));
            let state = view.output("sys:kept", &input).expect("the state");
            kept.push(format!("{v} {}", json!(state)));
        }
        At {
            reads,
            described,
            items,
            kept,
        }
    }

    /// Every position from `from` on, as the lake answers now.
    async fn history(&self, from: u64) -> Vec<At> {
        let mut history = Vec::new();
        for p in from..=self.head().await.0 {
            history.push(self.at(Position(p)).await);
        }
        history
    }
}

fn of(p: Position) -> ReadAt {
    ReadAt {
        as_of: Some(p),
        valid_at: None,
    }
}

/// The live history and the rebuilt one of a lake that declares `step` at
/// p1, `between` (if any) after it, and writes `item` `d` last, from the
/// position before p1 on; and p1.
async fn folded(name: &str, step: Value, between: Option<Value>) -> (Vec<At>, Vec<At>, u64) {
    let k = Kept::new(name).await;
    let p0 = k.head().await.0;
    let p1 = p0 + 1;
    k.declare(step).await;
    if let Some(d) = between {
        k.declare(d).await;
    }
    let wrote = k.l.upsert("item", "d", json!({"g": "y", "net": "4"})).await;
    wrote.expect("a write");
    let live = k.history(p0).await;
    let head = k.head().await;
    assert_eq!(k.store.rebuild().await.expect("the rebuild"), head);
    (live, k.history(p0).await, p1)
}

const KEPT: &str = r#"{"state":"kept"}"#;
const NOT_HERE: &str = r#"{"state":"computed","why":{"because":"not_kept_here"}}"#;

/// The rows with an event between the declaration and the write.
async fn between(name: &str, step: Value) {
    let (live, rebuilt, p1) = folded(name, step, Some(other())).await;
    let (p2, p3) = (p1 + 1, p1 + 2);
    assert_eq!(live.len(), rebuilt.len());
    // The history begins at the position before p1.
    for (i, (l, r)) in live.iter().zip(&rebuilt).enumerate() {
        let p = p1 - 1 + i as u64;
        assert_eq!(
            l.reads, r.reads,
            "{name}: every read as of {p} answers alike"
        );
        assert_eq!(
            l.items, r.items,
            "{name}: the recomputed records stand alike as of {p}"
        );
        if p != p1 {
            assert_eq!(l.described, r.described, "{name}: `kept` as of {p}");
        }
        if p < p1 {
            assert_eq!(l.kept, r.kept, "{name}: the kept records as of {p}");
        }
    }
    assert_eq!(
        (&live[1].described, &rebuilt[1].described),
        (&vec![KEPT.to_owned(); 2], &vec![NOT_HERE.to_owned(); 2]),
        "{name}: as of the declaration the live lake has made the views again, and a \
         rebuild, whose flush comes after the next event, has not"
    );
    let state = |at: &At, v: &str| {
        (at.kept.iter())
            .find(|s| s.starts_with(&format!("{v} ")))
            .cloned()
    };
    for v in VIEWS {
        let whole = |w: bool| {
            Some(format!(
                r#"{v} {{"differ":0,"not_exact":[0,0],"whole":{w}}}"#
            ))
        };
        assert_eq!(
            (state(&live[1], v), state(&rebuilt[1], v)),
            (whole(true), whole(false)),
            "{name}: `{v}`'s state as of the declaration"
        );
    }
    // The group `x` no later write moves: it stands where it was made.
    let made = |at: &At| -> Vec<String> {
        let x = at.kept.iter().filter(|s| s.contains(" x v1 @"));
        x.map(|s| s.split(' ').nth(3).expect("a position").to_owned())
            .collect()
    };
    let head = live.len() - 1;
    assert_eq!(
        (made(&live[head]), made(&rebuilt[head])),
        (vec![format!("@{p1}"); 2], vec![format!("@{p2}"); 2]),
        "{name}: the make stands at the declaration live and at the next event rebuilt"
    );
    // The group `y` the write moved stands at the write on both paths.
    let y = |at: &At| {
        at.kept
            .iter()
            .filter(|s| s.contains(&format!(" y v1 @{p3} ")))
            .count()
    };
    assert_eq!((y(&live[head]), y(&rebuilt[head])), (2, 2), "{name}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_function_declared_again_makes_the_views_at_the_next_event_in_a_rebuild() {
    between("kept-recomputed-function", scaled("net * 3")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_declared_with_another_derivation_makes_them_at_the_next_event_too() {
    between("kept-recomputed-derived", item("net + 2")).await;
}

/// The control: with no event between, the flush stands at the
/// declaration on both paths, and nothing differs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_nothing_between_the_make_stands_at_the_declaration_on_both_paths() {
    let (live, rebuilt, _) = folded("kept-recomputed-control", scaled("net * 3"), None).await;
    assert_eq!(
        live, rebuilt,
        "every position, every answer and every record alike"
    );
}
