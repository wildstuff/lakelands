//! A change to the records a condition selects, planned before it acts
//! (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).
//! Its selection is what its caller's `find` with the condition answers;
//! each record is decided over the ones before it, as a batch decides its
//! writes, and the whole change is one append, or none: a plan tells every
//! refusal, and an act refuses where one record is refused.

use lakelands_api::expr::Condition;
use lakelands_api::read::{Find, Found, ReadAt};
use lakelands_api::record::Record;
use lakelands_api::write::{ChangeTo, FieldChanged, Plan, RecordsPlan, RecordsRefused};

use super::plan::{Against, Answer, Planned, stale};
use super::*;

/// How many first keys a plan names, of its selection and of a refusal.
const FIRST: usize = 5;

/// A change to records as its caller states it: the type, the condition,
/// what it does, the pairs a selected record holds beside it, and whether
/// restricted records are left out.
pub(crate) type Records = (
    TypeName,
    Condition,
    ChangeTo,
    BTreeMap<FieldName, Value>,
    bool,
);

impl Call<'_> {
    /// The plan of a change to records at `now`, and what makes it: its
    /// events land after `n` drafts, carrying `envelope` where one is
    /// given (an accept's). What needs no record is refused first; then
    /// the selection, past `max_change_records` refused, never cut; then
    /// each record, its refusal told in the plan. Made against a plan, it
    /// is refused `plan_stale` where the plan made now is another, and
    /// else where any record is refused.
    pub(crate) fn changing(
        &self,
        w: &Writer,
        (ty, filter, to, only, skip_restricted): Records,
        (now, n, envelope): (Timestamp, usize, Option<Envelope>),
        against: Against<'_>,
    ) -> Result<Planned> {
        let view = self.lake.engine.view(None)?;
        let mut d = self.decide(w, view.as_ref(), now, n);
        if let Some(e) = envelope {
            d.envelope = e;
        }
        let decl = d.at.sight.ty(&ty)?.clone();
        not_a_view(&decl)?;
        // On a link type a change sets the link's own fields or unlinks it,
        // named by its owner, its relation and its two ends (035).
        let link = links::link_type_parts(&ty);
        let ends = |key: &RecordKey| {
            let parts = lakelands_api::key::key_components(key);
            let part = |i: usize| RecordKey::from(parts.get(i).map_or("", String::as_str));
            (part(0), part(1))
        };
        let blank = RecordKey::from("");
        let none = Patch::new();
        let patch = match &to {
            ChangeTo::Set(p) => p,
            ChangeTo::Retire => &none,
        };
        let checked = d.unread(op_of((&ty, link.as_ref(), &to), &blank, (&blank, &blank)))?;
        // Restricted records left out, it selects as a caller without
        // `restricted` would: a proposal's, as its proposer (081).
        let mut who = self.who.clone();
        who.grants.restricted = d.at.sight.sees_restricted() && !skip_restricted;
        let sight = Sight::held(&who, &w.ontology);
        let selected = self.selected(&sight, &decl, (&filter, &only), now)?;
        // Per field, the records whose value it moves; one its caller may
        // write and not read is told as written on every one (081).
        let mut fields: Vec<FieldChanged> = (checked.iter())
            .filter(|(f, _)| patch.contains_key(*f))
            .map(|(f, v)| {
                let reads = d.at.sight.reads(&ty, f);
                let moves = |r: &&Record| !reads || r.fields.get(f).unwrap_or(&Value::Null) != v;
                let records = selected.iter().filter(moves).count() as u64;
                let field = f.clone();
                FieldChanged { field, records }
            })
            .collect();
        fields.sort_by(|a, b| a.field.cmp(&b.field));
        let (mut drafts, mut refused) = (Vec::new(), Vec::<RecordsRefused>::new());
        for r in &selected {
            // Each record is its own write, held to `max_set_changes` alone.
            d.pending.across = 0;
            let (from, end) = ends(&r.key);
            let op = op_of((&ty, link.as_ref(), &to), &r.key, (&from, &end));
            match d.plan(op, Some(r.version)) {
                Ok(plan) => drafts.extend(plan.drafts),
                Err(e) => refusal(&mut refused, e, &r.key),
            }
        }
        let section = RecordsPlan {
            ty,
            selected: selected.len() as u64,
            sample_keys: selected.iter().take(FIRST).map(|r| r.key.clone()).collect(),
            fields,
            refused,
        };
        let mut plan = Plan {
            report_id: String::new(),
            at: head(w),
            types: Vec::new(),
            value_types: Vec::new(),
            lossless: section.refused.is_empty(),
            records: Some(Box::new(section)),
        };
        plan.report_id = plan_id(&plan, &selected);
        stale(&plan, against)?;
        let first = plan.records.as_ref().and_then(|r| r.refused.first());
        if let (Some(_), Some(r)) = (against.plan, first) {
            return Err((*r.refusal).clone());
        }
        drop(d);
        drop(view);
        Ok(Planned {
            plan,
            nothing: drafts.is_empty(),
            drafts,
            answer: Answer::Last,
        })
    }

    /// The records of `decl` that `find` with `filter` answers the
    /// principal of `sight` at `now`, those holding every pair of `only`,
    /// in key order. A pair's field is probed as the condition's are; one
    /// more record than `max_change_records` refuses the selection.
    fn selected(
        &self,
        sight: &Sight<'_>,
        decl: &TypeDecl,
        (filter, only): (&Condition, &BTreeMap<FieldName, Value>),
        now: Timestamp,
    ) -> Result<Vec<Record>> {
        let view = self.lake.engine.view(None)?;
        let shape = self.lake.shape(view.as_ref())?;
        let read = crate::reads::Reader::open(
            self.lake,
            sight,
            (ReadAt::default(), Some(now)),
            view,
            shape,
        )?;
        let mut find = Find {
            filter: Some(filter.clone()),
            ..Find::default()
        };
        sight.check_probe(decl, only.keys())?;
        let max = self.lake.limits.max_change_records;
        let mut out = Vec::new();
        loop {
            let Found::Records {
                rows, next_cursor, ..
            } = read.find(&decl.name, &find)?
            else {
                unreachable!("a find without an aggregate answers records")
            };
            let holds = |r: &Record| only.iter().all(|(f, v)| r.fields.get(f) == Some(v));
            out.extend(rows.into_iter().filter(holds));
            if out.len() > max as usize {
                return Err(Error::refused(
                    Some("where".into()),
                    Refusal::TooLarge { max },
                ));
            }
            match next_cursor {
                Some(c) => find.cursor = Some(c),
                None => break,
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}

/// The op that writes the record at `key` of `ty`, or, a link type's
/// owner and relation given, the link at `ends`.
fn op_of<'a>(
    (ty, link, to): (
        &'a TypeName,
        Option<&'a (TypeName, RelationName)>,
        &'a ChangeTo,
    ),
    key: &'a RecordKey,
    (from, end): (&'a RecordKey, &'a RecordKey),
) -> Op<'a> {
    match (link, to) {
        (None, ChangeTo::Set(p)) => Op::Upsert(ty, key, p),
        (None, ChangeTo::Retire) => Op::Retire(ty, key),
        (Some((owner, r)), ChangeTo::Set(p)) => Op::Link((owner, from, r, end), p),
        (Some((owner, r)), ChangeTo::Retire) => Op::Unlink((owner, from, r, end)),
    }
}

/// A record's refusal, counted with those like it: alike where they differ
/// only in what names the record (its key, the versions compared, the
/// sentence that names them).
fn refusal(refused: &mut Vec<RecordsRefused>, e: Error, key: &RecordKey) {
    let like = alike(&e);
    match refused.iter_mut().find(|g| alike(&g.refusal) == like) {
        Some(g) => {
            g.records += 1;
            if g.sample_keys.len() < FIRST {
                g.sample_keys.push(key.clone());
            }
        }
        None => refused.push(RecordsRefused {
            refusal: Box::new(e),
            records: 1,
            sample_keys: vec![key.clone()],
        }),
    }
}

/// `e` without what names one record.
fn alike(e: &Error) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.retain(|k, _| !matches!(k.as_str(), "key" | "expected" | "actual" | "message"));
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
    strip(&mut v);
    v
}

/// A plan of records' id: made over what its reader is shown of it and
/// over every selected key with the version read, never over the head, so
/// a write elsewhere in the lake does not stale it (081).
fn plan_id(plan: &Plan, selected: &[Record]) -> String {
    let plan = Plan {
        report_id: String::new(),
        at: Position::GENESIS,
        ..plan.clone()
    };
    let selection: Vec<(&RecordKey, Version)> =
        selected.iter().map(|r| (&r.key, r.version)).collect();
    lakelands_core::canonical::sha256_hex(&serde_json::json!({
        "plan": serde_json::to_value(&plan).expect("a plan serialises"),
        "selection": selection,
    }))
}
