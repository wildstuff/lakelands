//! The registry: every check the suite runs, in the order it runs them.
//!
//! Adding a check is one new file here (or one function in the file of
//! its subject) and one line in [`ALL`], plus one `mod` line for a new
//! file. A check that is not in [`ALL`] does not run, on any engine.

use super::Check;

mod cursors;
mod decimals;
mod derived_fields;
mod feed;
mod fields;
mod filters;
mod ingest;
mod keys;
mod masking;
mod migrate;
mod paging;
mod positions;
mod proposals;
mod provenance;
mod reactions;
mod rebuild;
mod restriction;
mod retire;
mod schema;
mod search;
mod values;
mod versions;
mod views;
mod walks;

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
    rebuild::REBUILD_REPRODUCES_THE_CHAIN,
    filters::ABSENT_NEVER_MATCHES,
    filters::NUMBERS_COMPARE_AS_NUMBERS,
    filters::KINDS_THAT_DO_NOT_COMPARE,
    filters::PROBING_REFUSED,
    filters::PROVENANCE_IN_WHERE_AND_ORDER,
    provenance::TWO_TIME_AXES,
    provenance::FIELD_PROVENANCE,
    masking::MASKED_AND_ABSENT,
    values::DATE_TIME_CANONICAL,
    values::BYTES_ONE_SPELLING,
    values::VECTOR_LENGTH,
    values::VALUE_TYPE_RULES,
    values::NULL_CLEARS_ANY_TYPE,
    schema::SCHEMA_HASH_VECTOR,
    schema::SCHEMA_HASH_IGNORES_MEANING,
    schema::RESHAPING_DECLARE_REFUSED,
    schema::DECLARE_REFUSALS,
    schema::SYS_RESERVED,
    migrate::APPLY_NEEDS_ITS_REPORT,
    migrate::LOSSY_REFUSED,
    migrate::CONVERSIONS_AND_RENAMES,
    walks::UNDECLARED_RELATION,
    walks::TOO_DEEP,
    walks::WALK_ORDER,
    search::MATCH_SET,
    search::SEARCH_ORDER,
    search::SEARCH_PROBE_REFUSED,
    proposals::VERB_BY_RISK,
    proposals::EXTERNAL_UNBOUND,
    proposals::ACCEPT_APPLIES,
    proposals::REJECT_AND_DECIDED,
    proposals::WHO_DECIDES,
    proposals::VERB_PROCEDURE_VERSION,
    cursors::CURSOR_STAYS,
    feed::FEED_IS_THE_LOG,
    feed::REBUILD_LEAVES_THE_FEED,
    feed::TRIGGER_FIRES,
    feed::OBJECT_SET_CHANGES,
    retire::RETIRE_NEEDS_EVERY_FIELD,
    ingest::REVERT_IS_A_CHANGE,
    views::VIEW_KEY_IS_ITS_GROUP,
    keys::KEY_AND_KEY_FIELDS_AGREE,
    keys::KEY_FIELDS_FROM_THE_KEY,
    restriction::HOLD_WITHHOLDS_AND_KEEPS,
    reactions::VERB_WRITE_REACTS,
];
