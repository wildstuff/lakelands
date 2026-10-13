//! A release's store, as a fixture of surface-held-test.sh.

/// A door taken out after this release.
#[deprecated(
    since = "0.1.0",
    note = "use `new_door`"
)]
pub fn old_door() {}

/// A door taken out with no deprecation.
pub fn kept_door() {}

/// A door the list names.
pub fn named_door() {}
