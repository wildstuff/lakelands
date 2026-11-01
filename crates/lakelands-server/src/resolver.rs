//! The default principal resolver: static entries from the doors'
//! configuration.
//!
//! An entry names one principal: its kind, its id, the lakes it may
//! address and its grants. A local user (the CLI's `--as <name>`)
//! resolves by the entry's name; an API key resolves by one of the
//! entry's keys; a principal another host forwards a call for resolves
//! by the entry's id, once the server has checked the host's key.
//! Anything else is refused. A host with identities, roles
//! or policies of its own replaces this resolver with one that compiles
//! them into grants.

use async_trait::async_trait;
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::id::LakeId;
use lakelands_api::principal::{Grants, Principal, PrincipalKind};
use lakelands_api::seam::{Credential, PrincipalResolver};
use serde::{Deserialize, Serialize};

/// One principal, as the configuration states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The name a local user gives to act as this principal.
    pub name: String,
    /// What kind of party it is.
    pub kind: PrincipalKind,
    /// Its identifier, recorded as the writer of what it writes.
    pub id: String,
    /// The lakes it may address; every lake when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lakes: Vec<LakeId>,
    /// What it may read and write.
    pub grants: Grants,
    /// API keys that stand for it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<String>,
}

impl Entry {
    /// Whether this principal may address `lake`.
    pub fn addresses(&self, lake: &LakeId) -> bool {
        self.lakes.is_empty() || self.lakes.contains(lake)
    }
}

/// Another host that may forward calls to the doors over the link
/// between hosts: a forwarding handle's process. Its key is its own,
/// never a principal's, and carries no grants: it lets the host name the
/// principal a call is made as, whose own grants judge the call, and
/// carry the call's write context (`docs/hosting.md`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    /// The host's name, as a resolver is told it.
    pub id: String,
    /// The keys that stand for it.
    pub keys: Vec<String>,
    /// The lakes it may forward to; every lake when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lakes: Vec<LakeId>,
    /// The ids of the principals it may act for; every principal the
    /// resolver knows for the lake when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principals: Option<Vec<String>>,
}

impl HostEntry {
    /// Whether the host may forward a call to `lake` made as `principal`.
    pub fn may(&self, lake: &LakeId, principal: &str) -> bool {
        (self.lakes.is_empty() || self.lakes.contains(lake))
            && (self.principals.as_ref()).is_none_or(|p| p.iter().any(|p| p == principal))
    }
}

/// The host's id and whom it may act for; never its keys.
impl std::fmt::Debug for HostEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostEntry")
            .field("id", &self.id)
            .field("lakes", &self.lakes)
            .field("principals", &self.principals)
            .finish_non_exhaustive()
    }
}

/// The host a key stands for, among `hosts`: every key is compared, in
/// time that depends only on their lengths and number, so the answer
/// tells a caller neither which host's key it guessed nor how much of it.
pub(crate) fn host_of<'h>(hosts: &'h [HostEntry], key: &str) -> Option<&'h HostEntry> {
    let mut found = None;
    for h in hosts {
        for k in &h.keys {
            if same_key(k, key) && found.is_none() {
                found = Some(h);
            }
        }
    }
    found
}

/// Whether two keys are the same, in time that depends only on their
/// lengths, so that comparing does not tell a caller how much of a key
/// it guessed.
pub(crate) fn same_key(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

/// Resolves credentials against a fixed list of entries.
#[derive(Clone, Debug, Default)]
pub struct StaticResolver {
    entries: Vec<Entry>,
}

impl StaticResolver {
    /// A resolver over these entries.
    pub fn new(entries: Vec<Entry>) -> StaticResolver {
        StaticResolver { entries }
    }
}

#[async_trait]
impl PrincipalResolver for StaticResolver {
    async fn resolve(&self, lake: &LakeId, credential: &Credential) -> Result<Principal> {
        let entry = self.entries.iter().find(|e| match credential {
            Credential::LocalUser(name) => &e.name == name,
            Credential::ApiKey(key) => e.api_keys.iter().any(|k| same_key(k, key)),
            Credential::Bearer(_) => false,
            Credential::Forwarded { principal, .. } => &e.id == principal,
        });
        match entry {
            Some(e) if e.addresses(lake) => Ok(Principal {
                kind: e.kind,
                id: e.id.clone(),
                lake: lake.clone(),
                grants: e.grants.clone(),
            }),
            _ => Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    fn resolver() -> StaticResolver {
        StaticResolver::new(vec![Entry {
            name: "clerk".into(),
            kind: PrincipalKind::Agent,
            id: "clerk@finance".into(),
            lakes: vec!["finance".into()],
            grants: Grants::default(),
            api_keys: vec!["k-1".into()],
        }])
    }

    #[test]
    fn a_name_or_a_key_resolves_to_its_entry_in_its_lakes() {
        let r = resolver();
        let lake = LakeId::from("finance");
        let by_name = block_on(r.resolve(&lake, &Credential::LocalUser("clerk".into()))).unwrap();
        assert_eq!(by_name.writer(), "agent:clerk@finance");
        assert_eq!(by_name.lake, lake);
        let by_key = block_on(r.resolve(&lake, &Credential::ApiKey("k-1".into()))).unwrap();
        assert_eq!(by_key, by_name);
    }

    #[test]
    fn anything_else_is_refused() {
        let r = resolver();
        let finance = LakeId::from("finance");
        for (lake, cred) in [
            (finance.clone(), Credential::LocalUser("nobody".into())),
            (finance.clone(), Credential::ApiKey("k-2".into())),
            (finance, Credential::Bearer("t".into())),
            (LakeId::from("hr"), Credential::LocalUser("clerk".into())),
        ] {
            let e = block_on(r.resolve(&lake, &cred)).unwrap_err();
            assert!(matches!(
                e,
                Error::Refused {
                    reason: Refusal::NotGranted,
                    ..
                }
            ));
        }
    }
}
