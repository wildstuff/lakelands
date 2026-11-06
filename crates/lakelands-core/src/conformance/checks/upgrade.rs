//! A projection an older build laid out is rebuilt from the log at a
//! writer's open: the layout is the projection's, kept apart from the log
//! format, and it will rise again.

use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use lakelands_api::Location;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::ontology::StorageKind::Number;

use crate::conformance::prelude::*;

check!(
    LAYOUT_REBUILT = a_writers_open_rebuilds_a_projection_an_older_build_laid_out,
    "004, extended (lakelands#1151): a writer's open of a projection an older build laid out rebuilds it from the log under its lease, saying why, and reads what the log holds and nothing else the projection held; the next writer's open rebuilds nothing; a writer's open of one a newer build laid out, or one with no mark, is refused as `projection_layout` and rebuilds nothing; a reader's open of any of them is refused by the engine before the store reads anything (`OTHER_LAYOUT`, through the engine seam)",
    given
);

/// The layout refusal the next writer's open meets, if any: `Some` of
/// the layout found, beside a layout of 2 the engine wants.
type Laid = Arc<Mutex<Option<Option<u64>>>>;

/// The harness's engine opener, whose next writer's open meets the
/// projection [`Laid`] says, and which counts the opens for a rebuild.
/// Each rebuild's engine is handed out holding a record `ghost` that no
/// event wrote, so a rebuild that refolds from the start drops it and a
/// reopen keeps it.
struct LaidOut {
    inner: Arc<dyn EngineOpener>,
    laid: Laid,
    rebuilds: Arc<AtomicU32>,
}

impl EngineOpener for LaidOut {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(
        &self,
        at: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(at)
    }
    fn open_scratch(&self, at: &Location) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.inner.open_scratch(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        let laid = crate::lock(&self.laid).take_if(|_| mode != OpenMode::Rebuild);
        if let Some(found) = laid {
            crate::engines::layout(&at.lake, found, 2)?;
        }
        let engine = self.inner.open(at, mode)?;
        if mode == OpenMode::Rebuild {
            self.rebuilds.fetch_add(1, Ordering::SeqCst);
            let (ty, key) = (TypeName::from("t"), RecordKey::from("a"));
            let held = engine.view(None)?.get(&ty, &key)?;
            let at = engine.projection().folded_through()?;
            let (Some(mut ghost), Some(at)) = (held, at) else {
                panic!(
                    "the harness puts its `ghost` beside `t/a`, which this rebuild's engine holds"
                )
            };
            ghost.key = "ghost".into();
            engine.projection().begin()?;
            engine.projection().put_record(&ghost)?;
            engine.projection().commit(at)?;
        }
        Ok(engine)
    }
}

async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out(
    store: Store,
    given: Given,
) -> Held {
    let (laid, rebuilds): (Laid, _) = (Arc::default(), Arc::new(AtomicU32::new(0)));
    let wrap: crate::conformance::Wrap = {
        let (laid, rebuilds) = (laid.clone(), rebuilds.clone());
        Arc::new(move |inner| {
            let (laid, rebuilds) = (laid.clone(), rebuilds.clone());
            Arc::new(LaidOut {
                inner,
                laid,
                rebuilds,
            }) as Arc<dyn EngineOpener>
        })
    };
    let open = |store: Store, found: Option<Option<u64>>| {
        *crate::lock(&laid) = found;
        (given.fault)(store, Seams::default(), wrap.clone())
    };
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("t", vec![field("n", Number)])).await?;
    l.upsert("t", "a", json!({"n": 1})).await?;
    let store = (open(store, Some(Some(1))).await)
        .map_err(|e| Failure::new(format!("the writer's open of layout 1 was refused: {e}")))?;
    let (why, n) = (store.rebuilt_at_open(), rebuilds.load(Ordering::SeqCst));
    let a = Lake::of(store.as_ref()).read("t", "a").await?;
    ensure!(
        n == 1 && why.as_ref().is_some_and(|w| w.contains("layout 1")) && a.fields["n"] == 1,
        "a writer's open of layout 1 rebuilds once, saying why, and reads `a`: {n} rebuilds, \
         {why:?}, {a:?}"
    );
    let (l, at) = (
        Lake::of(store.as_ref()),
        crate::conformance::fixture::get_at("ghost", None),
    );
    let ghost = (l.cat.get(&l.own, &TypeName::from("t"), at).await).map(|r| r.data.fields);
    ensure!(
        matches!(ghost, Err(Error::NotFound { .. })),
        "the rebuild refolds the projection from the start, so `ghost`, which no event wrote, \
         is gone; it answered {ghost:?}"
    );
    let store = (open(store, None).await)
        .map_err(|e| Failure::new(format!("the next writer's open was refused: {e}")))?;
    let (why, n) = (store.rebuilt_at_open(), rebuilds.load(Ordering::SeqCst));
    ensure!(
        n == 1 && why.is_none(),
        "the next writer's open rebuilds nothing: {n} rebuilds, {why:?}"
    );
    for (found, what) in [(Some(3), "newer"), (None, "unmarked")] {
        let twin = (given.twin)(format!("layout-{what}").into(), Seams::default()).await;
        let twin = twin.map_err(|e| Failure::new(format!("opening the {what} twin: {e}")))?;
        let refused = open(twin, Some(found)).await.err();
        let layout = refused.as_ref().and_then(Error::situation);
        ensure!(
            matches!(layout, Some(Situation::ProjectionLayout { .. }))
                && rebuilds.load(Ordering::SeqCst) == 1,
            "a writer's open of a projection {what} is refused and rebuilds nothing: {refused:?}"
        );
    }
    Ok(())
}
