use super::*;
use serde_json::json;

/// An answer that set nothing off reads, and is read, byte for byte as
/// before; one that did carries the members flat beside `position`.
#[test]
fn an_outcome_carries_what_it_set_off_flat_and_only_when_there_is_some() {
    let plain = Outcome::Applied {
        position: Position(21),
        version: Some(Version(3)),
        output: None,
        set_off: SetOff::default(),
        renamed: Vec::new(),
    };
    let today = r#"{"outcome":"applied","position":21,"version":3}"#;
    assert_eq!(serde_json::to_string(&plain).unwrap(), today);
    assert_eq!(serde_json::from_str::<Outcome>(today).unwrap(), plain);
    let fired = Outcome::Applied {
        position: Position(21),
        version: Some(Version(3)),
        output: None,
        set_off: SetOff {
            through: Some(Position(23)),
            fired: vec!["order_settled".into()],
            joined: vec!["paid".into()],
            left: vec!["open".into()],
        },
        renamed: vec![("holder".into(), "keeper".into())],
    };
    let wire = json!({"outcome": "applied", "position": 21, "version": 3, "through": 23,
                      "fired": ["order_settled"], "joined": ["paid"], "left": ["open"],
                      "renamed": [["holder", "keeper"]]});
    assert_eq!(serde_json::to_value(&fired).unwrap(), wire);
    assert_eq!(serde_json::from_value::<Outcome>(wire).unwrap(), fired);
}
