use rusqlite::{Error, ffi};

/// Neither a driver error nor a failure SQLite reports quotes a statement
/// or a value (lakelands#739).
#[test]
fn a_sqlite_error_says_no_statement_and_no_value() {
    let (sql, msg) = ("SELECT 'a secret'".into(), "near \"a secret\"".into());
    let input = Error::SqlInputError {
        error: ffi::Error::new(1),
        msg,
        sql,
        offset: 7,
    };
    let unique = ffi::Error::new(ffi::SQLITE_CONSTRAINT_UNIQUE);
    let failure = Error::SqliteFailure(unique, Some("UNIQUE constraint failed: t.k".into()));
    let value = Error::IntegralValueOutOfRange(0, 9_007_199_254_740_993);
    let says = [
        ": IntegralValueOutOfRange",
        ": SqlInputError",
        ": SQLite 2067",
    ];
    for (e, says) in [value, input, failure].into_iter().zip(says) {
        let told = super::sql(e).to_string();
        let quotes = told.contains("9007199254740993") || told.contains("secret");
        assert!(told.contains(says) && !quotes, "answered {told}");
    }
}
