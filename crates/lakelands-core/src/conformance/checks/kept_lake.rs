//! The lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), kept as bytes and refused on every engine: no format before 24 is read (record 115).
use crate::conformance::prelude::*;
use crate::export;

check!(
    LAKE_23_REFUSED = the_lake_of_format_23_is_refused,
    "054, 115: the lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), in format 23, kept as its bytes, is refused by a verifier of its export and by an import on this engine as `log_format`, naming the lake, format 23 and the one format this release reads and writes; nothing of it is read and nothing upgraded",
    given
);

async fn the_lake_of_format_23_is_refused(_store: Store, given: Given) -> Held {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/23");
    let reads = u64::from(lakelands_api::event::LOG_FORMAT_VERSION);
    let lake = lakelands_api::LakeId::from("kept23");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept23.keys.json"), &lake)?;
    let seams = super::erasable::with(&std::sync::Arc::new(keys));
    let verified = export::verify(&dir).err();
    let imported = (given.import)(dir, lake, None, seams).await.err();
    for (door, e) in [("a verifier", verified), ("an import", imported)] {
        // A refusal that is not of the format is the call's own failure.
        if let Some(e) = e
            .as_ref()
            .filter(|e| !matches!(e.situation(), Some(Situation::LogFormat { .. })))
        {
            ensure!(
                !matches!(e, Error::Unavailable { .. }),
                "{door} was unavailable: {e}"
            );
        }
        ensure!(
            matches!(e.as_ref().and_then(Error::situation),
                Some(Situation::LogFormat { lake: l, found: 23, reads: r, oldest })
                    if l == "kept23" && *r == reads && *oldest == Some(reads)),
            "{door} of the lake of format 23 refuses it as `log_format`, naming `kept23`, \
             format 23 and {reads}; it answered {e:?}"
        );
    }
    Ok(())
}
