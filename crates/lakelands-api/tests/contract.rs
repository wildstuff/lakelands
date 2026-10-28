//! What a host building on the contract relies on staying closed: the
//! error set it maps onto its own, and the event kinds of this format
//! version.

use lakelands_api::event::KINDS;
use lakelands_api::{Error, Missing, Refusal};

/// Every error the contract answers, with what a host does with it. A
/// variant added to `Error`, `Refusal` or `Missing` does not compile here
/// until it is placed, so a host's own mapping learns of it the same day.
/// The host's golden mapping onto its own errors is the host's test.
fn intent(e: &Error) -> &'static str {
    match e {
        Error::Refused { reason, .. } => match reason {
            Refusal::NotGranted => "the principal lacks a grant; nothing was written",
            Refusal::GrantNeeded { .. } => {
                "the principal lacks the grant named; nothing was written"
            }
            Refusal::WrongDoor => "the field belongs to another authority's door",
            Refusal::VerbsOnly { .. } => "only the type's verbs write the field; call one it names",
            Refusal::Reserved => "a `sys.` name is the system's",
            Refusal::Invalid { .. } => "the request does not fit the lake's map",
            Refusal::Constraint { .. } => "a value breaks a declared constraint",
            Refusal::Precondition { .. } => "a verb's `requires` does not hold",
            Refusal::TooDeep { .. } => "a chain of reactions passed the lake's ceiling",
            Refusal::TooLarge { .. } => "a graph measure would reach past the lake's ceiling",
            Refusal::Migration { .. } => "a migration would lose information",
            Refusal::Decided => "the proposal is already decided",
            Refusal::Declined { .. } => {
                "the host declined the call by policy; nothing ran, a retry answers the same"
            }
            Refusal::Unanswered { .. } => "an external call has no recorded answer; nothing ran",
            Refusal::OutOfRange { .. } => "the position is past the head",
            Refusal::Unknown(_) => "a refusal a newer lake knows: its message says it",
        },
        Error::NotFound { missing } => match missing {
            Missing::Type { .. } => "no such type at the position read",
            Missing::Record { .. } => "no such record at the position read",
            Missing::Verb { .. } => "no such verb",
            Missing::Function { .. } => "no such function",
            Missing::Proposal { .. } => "no such proposal",
            Missing::Quarantined { .. } => "no such quarantined record open to the caller",
            Missing::Unknown(_) => "a thing a newer lake knows: its message says it",
        },
        Error::Conflict { .. } => "the record moved: retry against its version",
        Error::Unbound { .. } => "the host bound no seam for this",
        Error::Unavailable { .. } => "the lake cannot answer now; retrying may help",
        Error::Ingested { .. } => {
            "a record of an ingest was refused: its refusal, and where to resume"
        }
        Error::Unknown(_) => "an error of a kind a newer lake knows: its message says it",
    }
}

#[test]
fn every_error_the_contract_answers_has_a_place_in_a_hosts_mapping() {
    let e = Error::Refused {
        field: None,
        reason: Refusal::TooDeep { max: 8 },
    };
    assert!(!intent(&e).is_empty());
}

#[test]
fn the_event_kinds_are_distinct_and_each_is_on_the_log_format_page() {
    let mut seen = std::collections::BTreeSet::new();
    for k in KINDS {
        assert!(seen.insert(*k), "`{k}` is listed twice");
    }
    let page = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/log-format.md"
    ))
    .expect("docs/log-format.md");
    for k in KINDS {
        assert!(
            page.contains(&format!("`{k}`")),
            "the kind `{k}` is not named on docs/log-format.md"
        );
    }
}

/// A feed written outside the repository, which never waits: its `wait`
/// is the trait's own, and answers at once with the head its poll reads
/// (lakelands#1061).
#[test]
fn a_feed_that_cannot_wait_answers_at_once() {
    use lakelands_api::Position;
    use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter};
    struct Relay;
    #[async_trait::async_trait]
    impl ChangeFeed for Relay {
        async fn poll(
            &self,
            from: Position,
            _: &FeedFilter,
            _: u32,
        ) -> lakelands_api::Result<FeedBatch> {
            let head = Position(7);
            Ok(FeedBatch {
                events: Vec::new(),
                next: from.max(head.next()),
                head,
                overruled: Vec::new(),
            })
        }
        async fn ack(&self, _: &str, _: Position) -> lakelands_api::Result<()> {
            Ok(())
        }
    }
    let wait = Relay.wait(Position(3), std::time::Duration::from_secs(3600));
    let mut wait = std::pin::pin!(wait);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let answered = wait.as_mut().poll(&mut cx);
    assert!(
        matches!(answered, std::task::Poll::Ready(Ok(Position(7)))),
        "{answered:?}"
    );
}
