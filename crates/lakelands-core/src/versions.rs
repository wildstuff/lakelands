//! The log format version an event needs (`docs/log-format.md` §6).
//!
//! A lake writes the version it was created with until an event needs a
//! newer one, and that version from there on. What needs which:
//!
//! - **2**: a `migrated` event that re-keys (`rekeyed`).
//! - **3**: an event carrying `upstream`, the lake and position values
//!   were read from; and a `declared` or `migrated` event whose
//!   declaration calls `date_bucket` in any expression it keeps.
//! - **4**: a `write_proposed` event whose write is a declaration or a
//!   migration (`declare`, `migrate`).
//!
//! The store appends each event at the greatest of the lake's version and
//! the one [`needed`] answers, and refuses, when it reads the log, an
//! event written below what it needs.

use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::ontology::VerbEffect;
use lakelands_api::write::Declaration;

/// The lowest format version that can carry `body`, with `upstream` or
/// without it.
pub fn needed(body: &EventBody, upstream: bool) -> u32 {
    if let EventBody::WriteProposed { write, .. } = body
        && write.is_schema()
    {
        return 4;
    }
    let declares_bucket = match body {
        EventBody::Declared { declaration, .. } => calls_date_bucket(declaration),
        EventBody::Migrated { migration, .. } => {
            calls_date_bucket(&Declaration::Type(migration.to.clone()))
        }
        _ => false,
    };
    if upstream || declares_bucket {
        return 3;
    }
    match body {
        EventBody::Migrated { rekeyed, .. } if !rekeyed.is_empty() => 2,
        _ => lakelands_api::event::LOG_FORMAT_BASE,
    }
}

/// Why an event is written below the version it needs, if it is: what it
/// carries and the version that added it.
pub fn below(event: &Event) -> Option<String> {
    let need = needed(&event.body, event.upstream.is_some());
    if event.v >= need {
        return None;
    }
    let what = if need == 4 {
        "proposes a declaration or a migration".to_owned()
    } else if event.upstream.is_some() {
        "carries `upstream`".to_owned()
    } else if need == 3 {
        "declares a call of `date_bucket`".to_owned()
    } else {
        "carries `rekeyed`".to_owned()
    };
    Some(format!(
        "a format {} event {what}, which format {need} added",
        event.v
    ))
}

/// Why an event carries a member its kind never carries, if it does:
/// `upstream` belongs to `record_ingested` alone (`docs/log-format.md`
/// §1.1), since only a source's record is read from another lake. The
/// ingest door cannot put it elsewhere; a reader holds the rule for a log
/// written by something else.
pub fn misplaced(event: &Event) -> Option<String> {
    let kind = event_kind(&event.body);
    (event.upstream.is_some() && !matches!(event.body, EventBody::RecordIngested { .. })).then(
        || {
            format!(
                "a `{kind}` event carries `upstream`, which only a `record_ingested` event carries"
            )
        },
    )
}

/// An event body's kind, as the log writes it.
fn event_kind(body: &EventBody) -> String {
    serde_json::to_value(body)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Whether a declaration calls `date_bucket` in any expression the log
/// keeps: a field's derivation, a view's `where` and measures, a value
/// type's constraint, a verb's `requires` and assignments, a trigger's
/// `when`, an object set's filter, and an expression function's body. A
/// condition in text form that does not parse calls nothing.
pub fn calls_date_bucket(declaration: &Declaration) -> bool {
    let mut conditions: Vec<&Condition> = Vec::new();
    match declaration {
        Declaration::Type(t) => {
            conditions.extend(t.fields.iter().filter_map(|f| f.derived.as_ref()));
            if let Some(v) = &t.view {
                conditions.extend(v.r#where.as_ref());
                conditions.extend(v.measures.iter().map(|m| &m.expr));
            }
        }
        Declaration::ValueType(v) => conditions.extend(v.constraint.as_ref()),
        Declaration::Verb(v) => {
            conditions.extend(v.requires.as_ref());
            if let VerbEffect::Write { set } = &v.effect {
                conditions.extend(set.iter().map(|a| &a.value));
            }
        }
        Declaration::Trigger(t) => conditions.extend(t.when.as_ref()),
        Declaration::ObjectSet(s) => conditions.push(&s.filter),
        Declaration::Function(f) => conditions.extend(f.expr.as_ref()),
    }
    conditions.into_iter().any(|c| match c {
        Condition::Expr(e) => calls(e, Func::DateBucket),
        Condition::Text(t) => {
            lakelands_api::expr::parse(t).is_ok_and(|e| calls(&e, Func::DateBucket))
        }
    })
}

/// Whether an expression calls `f` anywhere.
pub fn calls(e: &Expr, f: Func) -> bool {
    match e {
        Expr::Lit { .. } | Expr::Path { .. } => false,
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => calls(lhs, f) || calls(rhs, f),
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => calls(arg, f),
        Expr::And { args } | Expr::Or { args } => args.iter().any(|a| calls(a, f)),
        Expr::In { arg, list } => calls(arg, f) || list.iter().any(|a| calls(a, f)),
        Expr::Call { func, args } => *func == f || args.iter().any(|a| calls(a, f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declaration(v: serde_json::Value) -> Declaration {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn upstream_belongs_to_record_ingested_alone() {
        let line = |body: serde_json::Value| -> Event {
            serde_json::from_value(json!({
                "v": 3, "lake": "l", "position": 1, "id": "0".repeat(32),
                "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
                "writer": "system:lakelands", "origin": "system",
                "context": {"origin": "direct", "depth": 0}, "fence": 1,
                "upstream": {"lake": "other", "position": 9},
                "body": body, "hash": "0".repeat(64)
            }))
            .unwrap()
        };
        let retired =
            line(json!({"kind": "record_retired", "type": "t", "key": "k", "version": 2}));
        let why = misplaced(&retired).expect("refused");
        assert!(why.contains("`record_retired`"), "{why}");
        let ingested = line(json!({"kind": "record_ingested", "type": "t", "key": "k",
                                   "version": 1, "patch": {"f": 1}}));
        assert_eq!(misplaced(&ingested), None);
    }

    #[test]
    fn a_declaration_calling_date_bucket_anywhere_needs_format_3() {
        let derived = declaration(json!({
            "declare": "type", "name": "t",
            "fields": [
                {"name": "at", "type": {"kind": "date_time"}},
                {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
                 "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
            ]
        }));
        assert!(calls_date_bucket(&derived));
        let trigger = declaration(json!({
            "declare": "trigger", "name": "g", "on": "t",
            "when": "date_bucket(at, 'day') > now()"
        }));
        assert!(calls_date_bucket(&trigger));
        let plain = declaration(json!({
            "declare": "type", "name": "t",
            "fields": [{"name": "date_bucket", "type": {"kind": "text"}}]
        }));
        assert!(
            !calls_date_bucket(&plain),
            "a field named date_bucket is no call of it"
        );
    }
}
