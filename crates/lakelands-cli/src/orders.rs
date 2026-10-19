//! What `serve` tells of the orders a lake answered by sorting a type
//! (lakelands#1029).
//!
//! A `find` ordered first by a field the lake keeps no order index for
//! sorts the whole type, on every page (041). The lake counts those pages,
//! and `describe_type` tells them in its lifecycle's `sorted_whole`. Every
//! `EVERY`, `serve` prints a line for each field whose count grew since it
//! last looked, so the host sees what its agents order by and names in
//! `sort_index.<type>` what it chooses to index. It reads as the lake's
//! owner and writes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::{
    FieldName, Grants, LakeStore, Principal, PrincipalKind, ReadAt, TypeName, WriteContext,
};

/// How often `serve` looks.
const EVERY: Duration = Duration::from_secs(600);

/// Start looking at each of `lakes`; answers how many it looks at.
pub fn start(lakes: &[Arc<dyn LakeStore>]) -> usize {
    for store in lakes {
        let store = store.clone();
        tokio::spawn(async move {
            let mut told = BTreeMap::new();
            loop {
                tokio::time::sleep(EVERY).await;
                look(store.as_ref(), &mut told).await;
            }
        });
    }
    lakes.len()
}

/// Print each field of `store` whose whole-sort count grew past `told`.
async fn look(store: &dyn LakeStore, told: &mut BTreeMap<(TypeName, FieldName), u64>) {
    let who = Principal {
        kind: PrincipalKind::System,
        id: "lakelands".into(),
        lake: store.lake().clone(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let Ok(lake) = cat.describe(&who, ReadAt::default()).await else {
        return;
    };
    for ty in lake.data.types.iter().map(|t| &t.name) {
        let Ok(d) = cat.describe_type(&who, ty, ReadAt::default()).await else {
            continue;
        };
        for s in d.data.lifecycle.sorted_whole {
            let seen = told.entry((ty.clone(), s.field.clone())).or_default();
            if s.pages > *seen {
                eprintln!(
                    "lakelands: lake `{}`: {} page(s) ordered by `{ty}`.`{}` sorted the type, {} ms in all; `sort_index.{ty}` naming it would read each page instead",
                    store.lake(),
                    s.pages,
                    s.field,
                    s.millis
                );
                *seen = s.pages;
            }
        }
    }
}
