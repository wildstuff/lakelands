use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;

use super::*;

/// With the switch off, pages ordered by a field leave the writer nothing
/// to weigh, so it builds nothing (lakelands#1131). With it on, the same
/// pages leave the field to weigh.
#[test]
fn pages_counted_with_the_switch_off_leave_nothing_to_build() {
    for on in [false, true] {
        let mut location = Location::memory(format!("choose-{on}"));
        location.limits.choose_order_index = on;
        let seams = Seams {
            engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
            ..Seams::default()
        };
        let lake = Lake::open(&location, seams, OpenMode::Create).expect("the lake opens");
        let pages = location.limits.order_after_pages + 2;
        for _ in 0..pages {
            lake.note_whole(&"t".into(), &"n".into());
        }
        let c = lock(&lake.choosing);
        assert_eq!(
            (c.pending.is_empty(), c.pages.is_empty()),
            (!on, !on),
            "{pages} pages ordered by `n` with the switch {}",
            if on { "on" } else { "off" }
        );
    }
}
