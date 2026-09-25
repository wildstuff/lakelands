//! The lake and the principals every server test uses, on the mock and on
//! the real store over both engines.
//!
//! A refusal's sentence is not part of the contract; its kind and field
//! are. The transcripts pin what the mock and the store answer alike, and
//! a test that checks a refusal checks its kind and field, never its
//! sentence.

#![allow(dead_code)]

pub mod flaky;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::location::Limits;
use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::write::WriteOpts;
use lakelands_api::{LakeStore, Principal, WriteContext};
use lakelands_mock::MockLake;
use lakelands_server::resolver::{Entry, StaticResolver};
use serde_json::json;

/// Where a test's lake is kept: the mock, or the real store on one of
/// its engines. Every server test runs on each, unchanged: that is how the
/// mock is proven honest.
#[derive(Clone, Copy, Debug)]
pub enum Backend {
    /// `lakelands-mock`.
    Mock,
    /// `lakelands-store` over the in-memory engine.
    Memory,
    /// `lakelands-store` over the SQLite engine.
    Sqlite,
}

/// Every backend, in the order the tests run them.
pub const BACKENDS: [Backend; 3] = [Backend::Mock, Backend::Memory, Backend::Sqlite];

/// An empty lake `finance` on a backend. A memory lake belongs to the
/// opener that made it, so every lake here is a fresh one.
pub async fn empty(backend: Backend) -> Arc<dyn LakeStore> {
    empty_with(backend, Limits::default()).await
}

/// An empty lake `finance` on a backend, opened with these limits.
pub async fn empty_with(backend: Backend, limits: Limits) -> Arc<dyn LakeStore> {
    let seams = || lakelands_api::seam::Seams {
        engines: vec![
            Arc::new(lakelands_engine_mem::MemoryOpener::new()),
            Arc::new(lakelands_engine_sqlite::SqliteOpener),
        ],
        ..Default::default()
    };
    let create = lakelands_api::engine::OpenMode::Create;
    match backend {
        Backend::Mock => Arc::new(MockLake::open(&lakelands_api::Location {
            limits,
            ..lakelands_api::Location::memory("finance")
        })),
        Backend::Memory => lakelands_store::open(
            lakelands_api::Location {
                limits,
                ..lakelands_api::Location::memory("finance")
            },
            seams(),
            create,
        )
        .await
        .expect("the memory lake opens"),
        Backend::Sqlite => {
            let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("server-{}", std::process::id()))
                .join(format!("{:?}", std::thread::current().id()).replace(['(', ')'], ""));
            let _ = std::fs::remove_dir_all(&dir);
            lakelands_store::open(
                lakelands_api::Location {
                    limits,
                    ..lakelands_api::Location::sqlite("finance", dir)
                },
                seams(),
                create,
            )
            .await
            .expect("the sqlite lake opens")
        }
    }
}

/// The fixture lake with one invoice written, at a known position.
pub async fn lake_on(backend: Backend) -> Arc<dyn LakeStore> {
    lake_with(backend, Limits::default()).await
}

/// The fixture lake with one invoice written, opened with these limits.
pub async fn lake_with(backend: Backend, limits: Limits) -> Arc<dyn LakeStore> {
    let store = empty_with(backend, limits).await;
    let f = lakelands_mock::checks::setup(store.clone()).await;
    let c = store.catalog(WriteContext::default());
    c.upsert(
        &f.cast.owner,
        &"invoice".into(),
        &"INV-1".into(),
        lakelands_mock::checks::patch(&[("status", json!("open"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    store
}

fn clerk_grants() -> Grants {
    Grants {
        default: Access::Read,
        types: [(
            "invoice".into(),
            TypeGrants {
                default: None,
                fields: [
                    ("status".into(), Access::Write),
                    ("secret".into(), Access::None),
                ]
                .into(),
            },
        )]
        .into(),
        ..Grants::default()
    }
}

/// The owner (key `k-owner`) and a clerk (key `k-clerk`).
pub fn entries() -> Vec<Entry> {
    vec![
        Entry {
            name: "owner".into(),
            kind: PrincipalKind::Human,
            id: "owner".into(),
            lakes: vec!["finance".into()],
            grants: Grants::owner(),
            api_keys: vec!["k-owner".into()],
        },
        Entry {
            name: "clerk".into(),
            kind: PrincipalKind::Agent,
            id: "clerk".into(),
            lakes: vec!["finance".into()],
            grants: clerk_grants(),
            api_keys: vec!["k-clerk".into()],
        },
    ]
}

pub fn resolver() -> Arc<StaticResolver> {
    Arc::new(StaticResolver::new(entries()))
}

/// The lake's owner, as a stdio server is started for.
pub fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "finance".into(),
        grants: Grants::owner(),
    }
}

pub fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/transcripts")
        .join(name)
}

/// A `known_at` or a `proposed_at` is the lake's wall clock when it
/// wrote: the mock keeps a clock of its own and the store reads the real
/// one, so the transcripts show `{known_at}` where an instant was
/// answered, as they show `{session}` for the session id. Everything else
/// is compared as it is.
pub fn mask_known_at(text: &str) -> String {
    mask_instant(&mask_instant(text, "known_at"), "proposed_at")
}

fn mask_instant(text: &str, member: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(member) {
        let (head, tail) = rest.split_at(i + member.len());
        out.push_str(head);
        // Past the member's closing quote, the colon, spaces and the
        // value's opening quote, escaped or not.
        let skip = tail
            .char_indices()
            .find(|(_, c)| c.is_ascii_digit())
            .map(|(j, _)| j)
            .filter(|j| {
                tail[..*j]
                    .chars()
                    .all(|c| matches!(c, '"' | '\\' | ':' | ' '))
            });
        match skip {
            Some(j) => {
                out.push_str(&tail[..j]);
                let value = &tail[j..];
                let end = value
                    .find(|c: char| {
                        !(c.is_ascii_digit() || matches!(c, '-' | ':' | '.' | 'T' | 'Z'))
                    })
                    .unwrap_or(value.len());
                out.push_str(&format!("{{{member}}}"));
                rest = &value[end..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Compare, or record when the file has no answers yet. Every backend
/// answers the one file; a difference is written beside it, named by the
/// backend.
pub fn check(backend: Backend, name: &str, requests_only: bool, recorded: String, file: String) {
    let recorded = mask_known_at(&recorded);
    if requests_only {
        std::fs::write(path(name), &recorded).unwrap();
        panic!("recorded {name}; read it, then commit it");
    }
    if recorded != file {
        let actual = format!("{name}.{backend:?}.actual").to_lowercase();
        std::fs::write(path(&actual), &recorded).unwrap();
        panic!("on {backend:?}, {name} differs from what the server answers; see {actual}");
    }
}
