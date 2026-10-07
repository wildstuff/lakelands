//! The log format version an event needs (`docs/log-format.md` §6).
//!
//! A lake writes the version it was created with until an append holds an
//! event that needs a newer one, and that version from that append on:
//! every event of the append carries it. What needs which:
//!
//! - **2**: a `migrated` event that re-keys (`rekeyed`).
//! - **3**: an event carrying `upstream`, the lake and position values
//!   were read from; and a `declared` or `migrated` event whose
//!   declaration calls `date_bucket` in any expression it keeps.
//! - **4**: a `write_proposed` event whose write is a declaration or a
//!   migration (`declare`, `migrate`).
//! - **5**: an event whose declaration carries `annotations` anywhere: a
//!   `declared` event's declaration, a `migrated` event's `to`, a proposed
//!   `declare` or `migrate`.
//! - **6**: an event whose declaration holds a relation with `derived`,
//!   or is a trigger with `max_depth`, in the same places.
//! - **7**: an event whose declaration calls `retired` in any expression
//!   it keeps, in the same places.
//! - **8**: an event whose declaration is a view with `params`, or calls
//!   `param` in any expression it keeps, in the same places; and a verb
//!   on its type: declared with `scope: type` (a `declared` event, a
//!   proposed `declare`), called (`verb_called` without `key`) or proposed
//!   (a `call` without `key`).
//! - **9**: an event whose declaration is a view that `merge`s sources, in
//!   the places 5 names.
//! - **10**: a `write_proposed` event whose write tightens a value type.
//! - **11**: what [`ELEVENTH`] lists: a declaration that calls `floor`,
//!   `date_bucket` by a width or `in_set` in any expression it keeps, in
//!   the places 5 names; a migration that renames a relation
//!   (`relation_renames`), in a `migrated` event or a proposed `migrate`;
//!   a write of a `view_kept.<type>` setting, made or proposed; a
//!   `record_upserted` or `record_ingested` event that carries `made`;
//!   a proposed `migrate` whose report holds `restated`; and a
//!   declaration with a field that declares `initial`, in the places 5
//!   names.
//! - **12**: what [`TWELFTH`] lists: a `declared` event `with_the_set`;
//!   a declaration with a field that says `computed_at`, in the places 5
//!   names; a `value_type_migrated` event; and a proposed
//!   `migrate_value_type` whose `renames` is not empty.
//!
//! The store writes every event of one append at the greatest of the
//! lake's version and what [`needed`] answers for each event of the
//! append, and refuses, when it reads the log, an event written below what
//! it needs.

use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::ontology::{FieldDecl, VerbEffect, VerbScope};
use lakelands_api::write::{Declaration, ProposedWrite};

/// The lowest format version that can carry `body`, with `upstream` or
/// without it.
pub fn needed(body: &EventBody, upstream: bool) -> u32 {
    if carried(TWELFTH, body).is_some() {
        return 12;
    }
    if carried(ELEVENTH, body).is_some() {
        return 11;
    }
    if let EventBody::WriteProposed { write, .. } = body
        && matches!(write, ProposedWrite::MigrateValueType { .. })
    {
        return 10;
    }
    if merges_sources(body) {
        return 9;
    }
    if declares_view_params(body) || declares_a_verb_on_its_type(body) || calls_with_no_key(body) {
        return 8;
    }
    if calls_retired(body) {
        return 7;
    }
    if derives_a_relation(body) || caps_a_trigger(body) {
        return 6;
    }
    if carries_annotations(body) {
        return 5;
    }
    if let EventBody::WriteProposed { write, .. } = body
        && write.is_schema()
    {
        return 4;
    }
    // A proposed declare or migrate returned 4 above.
    if upstream || any_declaration(body, calls_date_bucket) {
        return 3;
    }
    match body {
        EventBody::Migrated { rekeyed, .. } if !rekeyed.is_empty() => 2,
        _ => lakelands_api::event::LOG_FORMAT_BASE,
    }
}

/// Whether an event carries one thing a format version added, and the
/// words an event below that version which carries it is refused in.
pub type Added = (fn(&EventBody) -> bool, &'static str);

/// What format version 11 added to what an event carries, each with the
/// words an event below 11 that carries it is refused in. One release
/// makes one step of the format: what it adds besides is a line here.
pub const ELEVENTH: &[Added] = &[
    (
        |body| any_declaration(body, |d| declares_call(d, Func::Floor)),
        "declares a call of `floor`",
    ),
    (
        |body| any_declaration(body, |d| declares(d, &by_a_width)),
        "declares a call of `date_bucket` by a width",
    ),
    (
        |body| any_declaration(body, |d| declares_call(d, Func::InSet)),
        "declares a call of `in_set`",
    ),
    (renames_a_relation, "renames a relation"),
    (writes_a_kept_setting, "writes a `view_kept.` setting"),
    (says_it_made, "carries `made`"),
    (
        reports_restated,
        "proposes a migration whose report holds `restated`",
    ),
    (
        |body| any_declaration(body, |d| any_field(d, |f| f.initial.is_some())),
        "declares an `initial` value",
    ),
];

/// What format version 12 added, as [`ELEVENTH`] lists 11's: below 12 a
/// set's records were computed again at each of its declarations, no
/// field said when it is computed, and a value type's migration renamed
/// no value.
pub const TWELFTH: &[Added] = &[
    (
        |body| matches!(body, EventBody::Declared { with_the_set, .. } if *with_the_set),
        "declares `with_the_set`",
    ),
    (
        |body| any_declaration(body, |d| any_field(d, |f| f.computed_at.is_some())),
        "declares when a field is computed",
    ),
    (
        |body| matches!(body, EventBody::ValueTypeMigrated { .. }),
        "renames a value type's values",
    ),
    (
        |body| {
            matches!(body, EventBody::WriteProposed {
                write: ProposedWrite::MigrateValueType { renames, .. }, ..
            } if !renames.is_empty())
        },
        "proposes a value type's migration that renames values",
    ),
];

/// Whether the event proposes a migration whose report says what its `to`
/// states otherwise: below 11 no report does, and a reader there would
/// take the proposal for one that restates nothing.
fn reports_restated(body: &EventBody) -> bool {
    use ProposedWrite::Migrate;
    let EventBody::WriteProposed { write, .. } = body else {
        return false;
    };
    matches!(write, Migrate { report, .. } if !report.restated.is_empty())
}

/// Whether a declaration holds a field that passes `test`, wherever a
/// field is declared: a type's, a relation's, a verb's payload's, a
/// function's input.
fn any_field(d: &Declaration, test: impl Fn(&FieldDecl) -> bool) -> bool {
    let any = |fs: &[FieldDecl]| fs.iter().any(&test);
    match d {
        Declaration::Type(t) => any(&t.fields) || t.relations.iter().any(|r| any(&r.fields)),
        Declaration::Verb(v) => any(&v.payload),
        Declaration::Function(f) => any(&f.inputs),
        _ => false,
    }
}

/// Whether a record event says its write found no live record under the
/// key. Below 11 no event says so, and absence tells nothing there.
fn says_it_made(body: &EventBody) -> bool {
    use EventBody::{RecordIngested, RecordUpserted};
    matches!(
        body,
        RecordUpserted { made: true, .. } | RecordIngested { made: true, .. }
    )
}

/// Whether the event writes a `view_kept.<type>` setting: an upsert or an
/// ingest of that `sys.setting`, or a proposed upsert of it. A store that
/// keeps no view would append beside records it never moves, so it must
/// refuse the log and not read past the setting.
fn writes_a_kept_setting(body: &EventBody) -> bool {
    use lakelands_api::seam::{SETTING_TYPE, VIEW_KEPT_SETTING};
    let (ty, key) = match body {
        EventBody::RecordUpserted { ty, key, .. } | EventBody::RecordIngested { ty, key, .. } => {
            (ty, key)
        }
        EventBody::WriteProposed {
            write: ProposedWrite::Upsert { ty, key, .. },
            ..
        } => (ty, key),
        _ => return false,
    };
    ty.as_str() == SETTING_TYPE && key.as_str().starts_with(VIEW_KEPT_SETTING)
}

/// Whether a migration the event carries, applied or proposed, renames a
/// relation.
fn renames_a_relation(body: &EventBody) -> bool {
    let migration = match body {
        EventBody::Migrated { migration, .. } => migration,
        EventBody::WriteProposed {
            write: ProposedWrite::Migrate { migration, .. },
            ..
        } => &**migration,
        _ => return false,
    };
    !migration.relation_renames.is_empty()
}

/// Whether a node is `date_bucket` with its third argument, the origin a
/// width is counted from.
fn by_a_width(e: &Expr) -> bool {
    matches!(e, Expr::Call { func: Func::DateBucket, args } if args.len() == 3)
}

/// What `body` carries of what a format version `added`, if anything.
pub fn carried(added: &[Added], body: &EventBody) -> Option<&'static str> {
    let added = added.iter().find(|(carries, _)| carries(body));
    added.map(|(_, what)| *what)
}

/// Whether any declaration the event carries holds for `test`: a
/// `declared` event's declaration, a `migrated` event's `to`, a
/// `value_type_migrated` event's declaration, a proposed `declare` or
/// `migrate`.
fn any_declaration(body: &EventBody, test: impl Fn(&Declaration) -> bool) -> bool {
    match body {
        EventBody::Declared { declaration, .. } => test(declaration),
        EventBody::Migrated { migration, .. } => test(&Declaration::Type(migration.to.clone())),
        EventBody::ValueTypeMigrated { declaration, .. } => {
            test(&Declaration::ValueType(declaration.clone()))
        }
        EventBody::WriteProposed { write, .. } => match write {
            ProposedWrite::Declare { declarations } => declarations.iter().any(test),
            ProposedWrite::Migrate { migration, .. } => {
                test(&Declaration::Type(migration.to.clone()))
            }
            ProposedWrite::MigrateValueType { to, .. } => {
                test(&Declaration::ValueType((**to).clone()))
            }
            _ => false,
        },
        _ => false,
    }
}

/// Whether a declaration the event carries holds a non-empty
/// `annotations` anywhere.
pub fn carries_annotations(body: &EventBody) -> bool {
    any_declaration(body, |d| {
        d.annotation_maps().iter().any(|(_, m)| !m.is_empty())
    })
}

/// Whether a declaration the event carries declares a derived relation.
pub fn derives_a_relation(body: &EventBody) -> bool {
    any_declaration(
        body,
        |d| matches!(d, Declaration::Type(t) if t.relations.iter().any(|r| r.derived.is_some())),
    )
}

/// Whether a declaration the event carries calls `retired`.
pub fn calls_retired(body: &EventBody) -> bool {
    any_declaration(body, |d| declares_call(d, Func::Retired))
}

/// Whether a declaration the event carries is a view that merges sources.
pub fn merges_sources(body: &EventBody) -> bool {
    any_declaration(
        body,
        |d| matches!(d, Declaration::Type(t) if t.view.as_ref().is_some_and(|v| !v.merge.is_empty())),
    )
}

/// Whether a declaration the event carries is a view with parameters, or
/// calls `param`.
pub fn declares_view_params(body: &EventBody) -> bool {
    any_declaration(body, |d| {
        matches!(d, Declaration::Type(t) if t.view.as_ref().is_some_and(|v| !v.params.is_empty()))
            || declares_call(d, Func::Param)
    })
}

/// Whether a declaration the event carries is a verb on its type.
pub fn declares_a_verb_on_its_type(body: &EventBody) -> bool {
    any_declaration(
        body,
        |d| matches!(d, Declaration::Verb(v) if v.scope == VerbScope::Type),
    )
}

/// Whether the event is a call of a verb on its type, made or proposed:
/// `verb_called`, or a proposed `call`, without a key.
pub fn calls_with_no_key(body: &EventBody) -> bool {
    matches!(
        body,
        EventBody::VerbCalled { key: None, .. }
            | EventBody::WriteProposed {
                write: ProposedWrite::Call { key: None, .. },
                ..
            }
    )
}

/// Whether a declaration the event carries is a trigger with `max_depth`.
pub fn caps_a_trigger(body: &EventBody) -> bool {
    any_declaration(
        body,
        |d| matches!(d, Declaration::Trigger(t) if t.max_depth.is_some()),
    )
}

/// Why an event is written below the version it needs, if it is: what it
/// carries and the version that added it.
pub fn below(event: &Event) -> Option<String> {
    let need = needed(&event.body, event.upstream.is_some());
    if event.v >= need {
        return None;
    }
    let added = carried(TWELFTH, &event.body).or_else(|| carried(ELEVENTH, &event.body));
    let what = if let Some(what) = added {
        what.to_owned()
    } else if need == 10 {
        "proposes a value type's tightening".to_owned()
    } else if need == 9 {
        "declares a view that merges sources".to_owned()
    } else if need == 8 && declares_a_verb_on_its_type(&event.body) {
        "declares a verb on its type".to_owned()
    } else if need == 8 && calls_with_no_key(&event.body) {
        "calls a verb with no key".to_owned()
    } else if need == 8 {
        "declares a view's parameters".to_owned()
    } else if need == 7 {
        "declares a call of `retired`".to_owned()
    } else if need == 6 && caps_a_trigger(&event.body) {
        "declares a trigger's `max_depth`".to_owned()
    } else if need == 6 {
        "declares a derived relation".to_owned()
    } else if need == 5 {
        "carries `annotations` in a declaration".to_owned()
    } else if need == 4 {
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

/// Why an event carries a member where it never sits, if it does:
/// `cause` inside `context`, since a host's cause is the envelope's
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`); and
/// `upstream` on any kind but `record_ingested`, since only a source's
/// record is read from another lake (`docs/log-format.md` §1.1); and the
/// origin `initial`, a field's provenance alone, never an event's
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`). No door
/// puts any there; a reader holds the rule for a log written by something
/// else.
pub fn misplaced(event: &Event) -> Option<String> {
    if event.context.cause.is_some() {
        return Some("`context` carries `cause`, which the envelope carries".to_owned());
    }
    if event.origin == lakelands_api::record::Origin::Initial {
        return Some("the envelope carries the origin `initial`, a field's alone".to_owned());
    }
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
/// keeps.
pub fn calls_date_bucket(declaration: &Declaration) -> bool {
    declares_call(declaration, Func::DateBucket)
}

/// Whether a declaration calls `f` in any expression the log keeps: a
/// field's derivation, a view's `where` and measures, a value type's
/// constraint, a verb's `requires` and assignments, a trigger's `when`, an
/// object set's filter, and an expression function's body. A condition in
/// text form that does not parse calls nothing.
pub fn declares_call(declaration: &Declaration, f: Func) -> bool {
    declares(
        declaration,
        &|e| matches!(e, Expr::Call { func, .. } if *func == f),
    )
}

/// Whether an expression the log keeps of a declaration holds, anywhere,
/// a node `test` accepts.
pub fn declares(declaration: &Declaration, test: &dyn Fn(&Expr) -> bool) -> bool {
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
    (conditions.into_iter()).any(|c| crate::describe::parsed(c).is_some_and(|e| holds(&e, test)))
}

/// Whether a condition holds, anywhere, a text literal `text` accepts.
pub fn names_text(c: &Condition, text: &dyn Fn(&str) -> bool) -> bool {
    use lakelands_api::expr::Literal;
    let lit = |e: &Expr| matches!(e, Expr::Lit { value: Literal::Text(t) } if text(t));
    crate::describe::parsed(c).is_some_and(|e| holds(&e, &lit))
}

/// Whether an expression calls, anywhere, a function `f` accepts.
pub fn calls(e: &Expr, f: &dyn Fn(Func) -> bool) -> bool {
    holds(e, &|e| matches!(e, Expr::Call { func, .. } if f(*func)))
}

fn holds(e: &Expr, test: &dyn Fn(&Expr) -> bool) -> bool {
    test(e) || crate::expr::children(e).any(|c| holds(c, test))
}

#[cfg(test)]
mod tests;
