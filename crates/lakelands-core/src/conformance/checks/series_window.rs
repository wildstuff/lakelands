//! A window of one series read along its series index walks about the
//! rows it reads (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
//! §The smaller side; lakelands#1780).
//!
//! A window wider than a batch is weighed before it is walked: the rows
//! inside it are counted against the rows of the other road. Along a
//! series index that count stays within the series the read names and
//! stops once the choice is told, so a step that covers its whole series
//! walks little more than the rows it hands, and every answer is the one
//! the same lake answers without the setting.

use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    SERIES_WINDOW_WALKS_ABOUT_ITS_ROWS = a_wide_window_of_one_series_walks_about_the_rows_it_reads,
    "097 §The smaller side (lakelands#1780): of two series of 1,440 one-minute readings under `series_index.<type>` on the series and the time, 24 hourly aggregate steps of one series and one step of its whole day walk at most twice the index rows they are handed, and the day step alone at most twice its own, while each answers what the same steps answer of a type without the setting"
);

/// The minutes of a day: each series holds one reading a minute.
const MINUTES: u64 = 1_440;

/// The type under the series index, and its control, which has none.
const TYPES: [&str; 2] = ["reading", "control"];

/// The instant `minute` minutes after the day's start.
fn at(minute: u64) -> String {
    let (h, m) = (minute / 60, minute % 60);
    if h == 24 {
        return "2026-01-06T00:00:00Z".to_owned();
    }
    format!("2026-01-05T{h:02}:{m:02}:00Z")
}

/// Declares `ty`, its sort and lookup indexes, and the series index on
/// `reading` alone, then feeds both series a reading a minute.
async fn readings(l: &Lake, ty: &str) -> Held {
    let fields = vec![
        sourced("meter", StorageKind::Text),
        sourced("taken_at", StorageKind::DateTime),
        sourced("value", StorageKind::Number),
    ];
    l.declare(type_decl(ty, fields)).await?;
    l.set(&format!("sort_index.{ty}"), json!(["taken_at"]))
        .await?;
    l.set(&format!("lookup_index.{ty}"), json!(["meter"]))
        .await?;
    if ty == "reading" {
        l.set("series_index.reading", json!([["meter", "taken_at"]]))
            .await?;
    }
    let records = ["m7", "m9"]
        .iter()
        .flat_map(|meter| {
            (0..MINUTES).map(move |i| {
                source_record(
                    ty,
                    &format!("{meter}-{i:04}"),
                    json!({"meter": meter, "taken_at": at(i), "value": i % 97}),
                )
            })
        })
        .collect();
    l.ingest("meters", records)
        .await
        .must("feeding the readings")?;
    Ok(())
}

/// An aggregate of `m7`'s readings from minute `from` up to `to`.
fn step(from: u64, to: u64) -> Find {
    let window = format!(
        "meter = 'm7' and taken_at >= '{}' and taken_at < '{}'",
        at(from),
        at(to)
    );
    Find {
        filter: Some(Condition::Text(window)),
        aggregate: Some(Aggregate {
            measures: vec![
                agg("n", AggFunc::Count, None),
                agg("total", AggFunc::Sum, Some("value")),
                agg("least", AggFunc::Min, Some("value")),
                agg("most", AggFunc::Max, Some("value")),
            ],
            group_by: vec![],
        }),
        ..Find::default()
    }
}

/// What `ty` answers to `find`, the rows the read was handed and the index
/// rows it walked: the process's counts, since a store reads on a pool of
/// its own, off the checking thread.
async fn walked(l: &Lake, ty: &str, find: Find) -> Result<(String, u64, u64), Failure> {
    crate::engines::count_walks(true);
    let before = (
        crate::engines::rows_read_in_process(),
        crate::engines::rows_walked_in_process(),
    );
    let reply = l.cat.find(&l.own, &TypeName::from(ty), find).await;
    let counts = (
        crate::engines::rows_read_in_process() - before.0,
        crate::engines::rows_walked_in_process() - before.1,
    );
    crate::engines::count_walks(false);
    let told = json!(reply.must(&format!("an aggregate of `{ty}`"))?.data).to_string();
    Ok((told, counts.0, counts.1))
}

async fn a_wide_window_of_one_series_walks_about_the_rows_it_reads(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    for ty in TYPES {
        readings(&l, ty).await?;
    }
    let steps: Vec<(u64, u64)> = (0..24)
        .map(|h| (h * 60, (h + 1) * 60))
        .chain([(0, MINUTES)])
        .collect();
    let (mut read, mut walks) = (0, 0);
    for (from, to) in steps {
        let (told, r, w) = walked(&l, "reading", step(from, to)).await?;
        let (control, _, _) = walked(&l, "control", step(from, to)).await?;
        ensure!(
            told == control,
            "`m7` from {} to {} answers {told} under the series index, and {control} without it",
            at(from),
            at(to)
        );
        if to - from == MINUTES {
            ensure!(
                w <= 2 * r,
                "the whole day of `m7` is handed {r} rows and walks {w} index rows, more than \
                 twice as many"
            );
        }
        (read, walks) = (read + r, walks + w);
    }
    ensure!(
        read >= 2 * MINUTES && walks <= 2 * read,
        "24 hourly steps of `m7` and its whole day are handed {read} rows, at least {}, and \
         walk {walks} index rows, at most twice as many",
        2 * MINUTES
    );
    Ok(())
}
