//! The default principal resolver: static entries from the doors'
//! configuration.
//!
//! An entry names one principal: its kind, its id, the lakes it may
//! address and its grants. A local user (the CLI's `--as <name>`)
//! resolves by the entry's name; an API key resolves by one of the
//! entry's keys. Anything else is refused. A host with identities, roles
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
            Credential::ApiKey(key) => e.api_keys.iter().any(|k| k == key),
            Credential::Bearer(_) => false,
        });
        match entry {
            Some(e) if e.lakes.is_empty() || e.lakes.contains(lake) => Ok(Principal {
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
