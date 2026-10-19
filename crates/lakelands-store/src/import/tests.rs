use lakelands_api::event::EventId;

use super::*;

/// An event at `at` that weighs `weighs` bytes against a batch.
fn weighing(at: &mut Position, weighs: usize) -> Result<CanonicalEvent> {
    let position = *at;
    *at = at.next();
    Ok(CanonicalEvent {
        position,
        id: EventId(String::new()),
        prev: Digest(String::new()),
        hash: Digest(String::new()),
        fence: 1,
        bytes: vec![b'x'; weighs],
    })
}

/// The weights of the batches `weights` is handed on in, at `limit`.
fn handed(weights: &[usize], limit: usize) -> Vec<Vec<usize>> {
    let mut at = Position::GENESIS;
    let events: Vec<_> = weights.iter().map(|w| weighing(&mut at, *w)).collect();
    let mut batches = Vec::new();
    batched(events.into_iter(), limit, &mut |batch| {
        batches.push(batch.iter().map(weight).collect());
        Ok(())
    })
    .expect("every batch is taken");
    batches
}

/// A batch is handed before the event that would take it over its bound,
/// so each holds at most the bound, and one that meets it exactly is full.
#[test]
fn a_batch_of_mixed_events_holds_at_most_its_bound() {
    let batches = handed(&[30, 50, 20, 70, 10, 40, 60, 100, 1], 100);
    assert_eq!(
        batches,
        [
            vec![30, 50, 20],
            vec![70, 10],
            vec![40, 60],
            vec![100],
            vec![1]
        ]
    );
    assert!(batches.iter().all(|b| b.iter().sum::<usize>() <= 100));
}

/// An event heavier than the bound goes alone, between the batches before
/// and after it.
#[test]
fn an_event_over_the_bound_is_a_batch_of_its_own() {
    let batches = handed(&[40, 150, 40, 30, 250], 100);
    assert_eq!(batches, [vec![40], vec![150], vec![40, 30], vec![250]]);
}

/// Nothing is handed for a file without events, and every event of a
/// file lighter than the bound goes in one batch, in order.
#[test]
fn a_file_lighter_than_the_bound_is_one_batch() {
    assert!(handed(&[], 100).is_empty());
    let mut at = Position::GENESIS;
    let events: Vec<_> = (0..4).map(|_| weighing(&mut at, 10)).collect();
    let mut positions = Vec::new();
    batched(events.into_iter(), BATCH, &mut |batch| {
        positions.push(batch.iter().map(|e| e.position).collect::<Vec<_>>());
        Ok(())
    })
    .expect("the batch is taken");
    assert_eq!(positions, [(0..4).map(Position).collect::<Vec<_>>()]);
}
