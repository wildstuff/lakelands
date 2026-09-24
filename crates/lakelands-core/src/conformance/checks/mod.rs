//! The registry: every check the suite runs, in the order it runs them.
//!
//! Adding a check is one new file here (or one function in the file of
//! its subject) and one line in [`ALL`], plus one `mod` line for a new
//! file. A check that is not in [`ALL`] does not run, on any engine.

use super::Check;

mod decimals;
mod derived_fields;
mod fields;
mod ingest;
mod keys;
mod paging;
mod positions;
mod reactions;
mod rebuild;
mod restriction;
mod versions;
mod views;

/// Every check, in the order the suite runs them.
pub const ALL: &[Check] = &[
    fields::REWRITE_LEAVES_OTHER_WRITERS_FIELDS,
    fields::CLEAR_ON_INPUT_CHANGE,
    fields::KEEP_ON_INPUT_CHANGE,
    fields::UNGRANTED_FIELD_REFUSES_WHOLE_PATCH,
    fields::SPARSE_PATCH,
    fields::UNDECLARED_FIELD_REFUSES_WHOLE_PATCH,
    positions::EVERY_REPLY_CARRIES_ITS_POSITION,
    positions::AS_OF_SEES_THE_STATE_THERE,
    positions::AS_OF_BEFORE_A_WRITE,
    positions::RETIRE_KEEPS_HISTORY,
    positions::PAST_THE_HEAD,
    versions::VERSIONS_COUNT_CHANGES,
    versions::STALE_EXPECTED_VERSION,
    versions::IDEMPOTENT_RETRY,
    paging::EVERY_RECORD_ONCE,
    paging::MISSING_SORT_VALUES_LAST,
    decimals::AS_WRITTEN,
    decimals::EXACT_SUM,
    derived_fields::COMPUTED_BY_THE_FOLD,
    derived_fields::NO_DOOR_WRITES_IT,
    derived_fields::ADDED_LATER,
    views::GROUPS_AND_SUMS,
    views::SAME_EITHER_WAY,
    views::EMPTIED_GROUP_GOES,
    views::CURRENT_AFTER_INGEST,
    ingest::SAME_CONTENT_TWICE,
    ingest::SAME_KEY_TWICE,
    ingest::FOREIGN_FIELD_REFUSED,
    ingest::UPSERT_REFUSES_SOURCE_FIELD,
    rebuild::REBUILD_ANSWERS_AS_BEFORE,
    keys::KEY_AND_KEY_FIELDS_AGREE,
    keys::KEY_FIELDS_FROM_THE_KEY,
    restriction::HOLD_WITHHOLDS_AND_KEEPS,
    reactions::VERB_WRITE_REACTS,
];
