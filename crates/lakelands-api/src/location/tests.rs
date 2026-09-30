use super::*;

#[test]
fn a_sqlite_location_written_without_tuning_reads_with_the_defaults() {
    let location: Location = serde_json::from_value(serde_json::json!({
        "lake": "ledger",
        "engine": {"engine": "sqlite", "dir": "/var/lakes/ledger"}
    }))
    .unwrap();
    assert_eq!(
        location.engine,
        EngineConfig::Sqlite {
            dir: "/var/lakes/ledger".into(),
            tuning: SqliteTuning::DEFAULT,
        }
    );
    let partial: SqliteTuning = serde_json::from_value(serde_json::json!({"mmap_mb": 0})).unwrap();
    assert_eq!(
        partial,
        SqliteTuning {
            mmap_mb: 0,
            ..SqliteTuning::DEFAULT
        }
    );
}
