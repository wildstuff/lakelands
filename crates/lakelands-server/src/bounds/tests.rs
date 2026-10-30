use std::time::{Duration, Instant};

use lakelands_api::Principal;
use lakelands_api::principal::{Grants, PrincipalKind};

use super::{Bounds, Gate, Limit, Rate};

fn at(n: u64) -> Limit {
    Limit::At(n.try_into().unwrap())
}

fn who(id: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: id.into(),
        lake: "finance".into(),
        grants: Grants::owner(),
    }
}

fn counts(gate: &Gate) -> usize {
    gate.principals.lock().unwrap().by.len()
}

#[test]
fn a_rate_takes_a_second_s_worth_at_once_and_fills_at_its_pace() {
    let start = Instant::now();
    let mut r = Rate::new(start);
    for _ in 0..4 {
        assert!(r.take(4, start).is_ok());
    }
    assert_eq!(
        r.take(4, start),
        Err(1),
        "a wait is a whole second at least"
    );
    assert!(r.take(4, start + Duration::from_millis(250)).is_ok());
    assert!(r.take(4, start + Duration::from_millis(250)).is_err());
    // A long quiet fills it to one second's worth, never more.
    let later = start + Duration::from_secs(60);
    for _ in 0..4 {
        assert!(r.take(4, later).is_ok());
    }
    assert!(r.take(4, later).is_err());
}

#[test]
fn a_limit_is_a_count_above_zero_or_off() {
    let read = |s: &str| serde_json::from_str::<Limit>(s).map_err(|_| ());
    assert_eq!(read("3"), Ok(at(3)));
    assert_eq!(read("\"off\""), Ok(Limit::Off));
    assert_eq!(read("0"), Err(()));
    assert_eq!(read("\"none\""), Err(()));
    let bounds: Bounds = serde_json::from_str(r#"{"principal_rate": "off"}"#).unwrap();
    assert_eq!(bounds.principal_rate, Limit::Off);
    assert_eq!(bounds.connection_rate, Bounds::default().connection_rate);
    assert_eq!(serde_json::to_value(Limit::Off).unwrap(), "off");
}

#[test]
fn a_count_whose_request_ended_and_whose_rate_is_full_is_dropped() {
    let gate = Gate::new(Bounds {
        principal_in_flight: at(4),
        ..Bounds::OFF
    });
    let held = gate.principal(&who("a")).ok().unwrap();
    assert_eq!(counts(&gate), 1);
    drop(held);
    assert_eq!(counts(&gate), 0, "no rate is counted, so nothing is left");
}

#[test]
fn a_count_still_filling_is_kept_and_an_idle_one_goes_at_the_next_sweep() {
    let gate = Gate::new(Bounds {
        principal_rate: at(1),
        ..Bounds::OFF
    });
    // Each ends at once, its bucket a second from full again.
    for i in 0..200 {
        drop(gate.principal(&who(&format!("p{i}"))).ok().unwrap());
    }
    assert_eq!(counts(&gate), 200, "a count still filling is kept");
    std::thread::sleep(Duration::from_millis(1100));
    // The counts have at most doubled since the last sweep when the next
    // one runs: within these, one does, and finds every `p` idle.
    for i in 0..300 {
        drop(gate.principal(&who(&format!("q{i}"))).ok().unwrap());
    }
    let p = gate.principals.lock().unwrap();
    assert!(
        !p.by.keys().any(|(_, id)| id.starts_with('p')),
        "an idle count is dropped"
    );
    assert!(p.by.len() <= 300, "{} counts kept", p.by.len());
}

#[tokio::test]
async fn a_request_cancelled_gives_its_slot_back() {
    let gate = Gate::new(Bounds {
        principal_in_flight: at(1),
        ..Bounds::OFF
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let g = gate.clone();
    let task = tokio::spawn(async move {
        let _held = g.principal(&who("a")).ok().unwrap();
        let _ = tx.send(());
        std::future::pending::<()>().await;
    });
    rx.await.unwrap();
    assert!(gate.principal(&who("a")).is_err(), "the slot is held");
    task.abort();
    let _ = task.await;
    assert!(gate.principal(&who("a")).is_ok(), "the slot came back");
}

#[test]
fn a_request_that_panics_gives_its_slot_back() {
    let gate = Gate::new(Bounds {
        principal_in_flight: at(1),
        ..Bounds::OFF
    });
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = gate.principal(&who("a")).ok().unwrap();
        panic!("the request fails");
    }));
    assert!(panicked.is_err());
    assert!(gate.principal(&who("a")).is_ok(), "the slot came back");
}
