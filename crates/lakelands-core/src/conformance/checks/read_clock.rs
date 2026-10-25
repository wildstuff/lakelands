//! A read's clock (lakelands#571, `docs/decisions/052-a-field-may-be-computed-at-the-read.md`):
//! the instant `valid_at` names, else as of a position the `known_at` of
//! its event, else the wall clock; one clock wherever the read evaluates
//! `now()` and `today()`, and one for every page a cursor continues.

use lakelands_api::read::Found;

use super::only::{de, holder};
use crate::conformance::prelude::*;

check!(
    READ_CLOCK = a_read_takes_its_clock_from_the_instant_or_the_position_it_names,
    "052, lakelands#571: a read's clock, the one `now()` and `today()` read in a `where`, a field computed at the read, a group key, a measure and a view's `where`, is the instant `valid_at` names, at the head and as of a position; else the host's clock (`ReadAt::clock`, a pin of the head); else, as of a position, the `known_at` of the event there, so a record due between that event and the read is not due as of it and is at the head; else the wall clock; every page a cursor continues, of `find`, `query` and `search`, reads the clock its first page read; and a listing of proposals judges what a proposed call reaches under the host's clock, else the clock of the position it reads, never `valid_at`: a call whose target turns held after a cutoff is listed to a reviewer without `restricted` as of its proposal, read after the cutoff, also `valid_at` an instant after it, and not at the head, also `valid_at` an instant before it, but is at the head under the host's clock before it; and to a holder of `only`, a write whose record's owner is derived from the clock is judged alike with and without `valid_at`"
);

/// The instant `valid_at` names: after `b3` falls due, long after the
/// wall clock.
const VALID: &str = "2500-01-01T00:00:00Z";

/// The host's clock (`ReadAt::clock`): after `b3` falls due too, and
/// before `VALID`, which comes first.
const HOST: &str = "2450-01-01T00:00:00Z";

fn instant(r: &Record) -> Option<Timestamp> {
    r.fields.get("read_at")?.as_str()?.parse().ok()
}

fn keys(rows: &[Record]) -> BTreeSet<String> {
    rows.iter().map(|r| r.key.to_string()).collect()
}

async fn a_read_takes_its_clock_from_the_instant_or_the_position_it_names(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (c, own) = (&l.cat, &l.own);
    l.declare_json(json!({"declare": "type", "name": "bill", "fields": [
        {"name": "due_on", "type": {"kind": "date_time"}},
        {"name": "note", "type": {"kind": "text"}, "searchable": true},
        {"name": "read_at", "type": {"kind": "date_time"}, "authority": "derived",
         "derived": "now()", "computed_at": "read"},
    ]}))
    .await?;
    l.declare_json(json!({"declare": "type", "name": "due_now",
        "fields": [{"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "bill", "where": "due_on < now()",
                 "measures": [{"name": "n", "expr": "count()"}]}}))
        .await?;
    // `b4`, written last, falls due a second after its write and before
    // the reads: at the head it is due, as of its write not.
    let soon_at = Timestamp(now_micros() + 1_000_000);
    let soon = soon_at.to_string();
    let mut at = Position::GENESIS;
    for (key, due) in [
        ("b1", "2000-01-01T00:00:00Z"),
        ("b2", "2999-01-01T00:00:00Z"),
        ("b3", "2400-01-01T00:00:00Z"),
        ("b4", soon.as_str()),
    ] {
        at = l
            .upsert("bill", key, json!({"due_on": due, "note": "a bill"}))
            .await?
            .position();
    }
    pause(Duration::from_millis(1100)).await;
    let known = log_events(store.as_ref()).await?;
    let known = known.iter().find(|e| e.position == at).map(|e| e.known_at);
    ensure!(
        known.is_some_and(|k| k < soon_at),
        "the setup lands `b4` before it falls due, {soon}: it landed at {known:?}"
    );
    let valid: Timestamp = VALID.parse().map_err(|_| Failure::new("an instant"))?;
    let host: Timestamp = HOST.parse().map_err(|_| Failure::new("an instant"))?;
    let asked = [
        ("at the head", None, None, None),
        ("as of the last write", Some(at), None, None),
        ("at the head, valid at 2500", None, Some(valid), None),
        (
            "as of the last write, valid at 2500",
            Some(at),
            Some(valid),
            None,
        ),
        (
            "as of the last write, the host's clock",
            Some(at),
            None,
            Some(host),
        ),
        (
            "at the head, valid at 2500, the host's clock",
            None,
            Some(valid),
            Some(host),
        ),
    ];
    for (when, as_of, valid_at, hosts) in asked {
        let read = ReadAt {
            as_of,
            valid_at,
            clock: hosts,
        };
        let before = Timestamp(now_micros());
        let rows = all_found(
            c.as_ref(),
            own,
            "bill",
            Find {
                at: read,
                ..Find::default()
            },
        );
        let rows = rows.await?;
        let clocks: BTreeSet<Option<Timestamp>> = rows.iter().map(instant).collect();
        let [clock] = <[Option<Timestamp>; 1]>::try_from(clocks.into_iter().collect::<Vec<_>>())
            .map_err(|c| Failure::new(format!("{when}, one read reads one clock: {c:?}")))?;
        let after = Timestamp(now_micros());
        let right = match (valid_at.or(hosts), as_of) {
            (Some(v), _) => clock == Some(v),
            (None, Some(_)) => clock == known,
            (None, None) => clock.is_some_and(|t| before <= t && t <= after),
        };
        ensure!(
            right,
            "{when}, `now()` reads the instant `valid_at` names, else the host's clock, else as \
             of a position the `known_at` of its event ({known:?}), else the wall clock; \
             `read_at` is {clock:?}"
        );
        let due: BTreeSet<String> = match (valid_at.or(hosts), as_of) {
            (Some(_), _) => ["b1", "b3", "b4"].map(String::from).into(),
            (None, None) => ["b1", "b4"].map(String::from).into(),
            (None, Some(_)) => ["b1"].map(String::from).into(),
        };
        let found = Find {
            filter: Some("due_on < now()".into()),
            at: read,
            ..Find::default()
        };
        let found = keys(&all_found(c.as_ref(), own, "bill", found).await?);
        ensure!(
            found == due,
            "{when}, a `where` reads the read's clock: `due_on < now()` finds {due:?}, and \
             found {found:?}"
        );
        let view = Find {
            at: read,
            ..Find::default()
        };
        let view = all_found(c.as_ref(), own, "due_now", view).await?;
        let n = view.first().and_then(|r| r.fields.get("n")).cloned();
        ensure!(
            n == Some(json!(due.len())),
            "{when}, a view's `where` reads the read's clock: `due_now` counts {}, and \
             answers {view:?}",
            due.len()
        );
        let mut by = aggregate(vec![agg("last", AggFunc::Max, Some("read_at"))]);
        let day = GroupKey::Named {
            name: "day".into(),
            expr: "today()".into(),
        };
        let by_day = by.aggregate.as_mut().expect("an aggregate");
        by_day.group_by = vec![day];
        by.at = read;
        let groups = c
            .find(own, &"bill".into(), by)
            .await
            .must("grouping by `today()`")?;
        let Found::Groups { rows } = groups.data else {
            return Err(Failure::new(format!("{when}, an aggregate answers groups")));
        };
        let said = |v: &Value| v.as_str().and_then(|s| s.parse::<Timestamp>().ok());
        let one = rows.len() == 1
            && rows.iter().all(|g| {
                // At the head the aggregate is a read of its own, with a
                // later clock, and its day is that clock's.
                let last = g.values.get("last").and_then(said);
                let same = match (as_of, valid_at.or(hosts)) {
                    (None, None) => last >= clock,
                    _ => last == clock,
                };
                let day =
                    g.by.get("day")
                        .and_then(Value::as_str)
                        .and_then(|d| d.get(..10));
                same && day
                    .zip(last)
                    .is_some_and(|(d, t)| t.to_string().starts_with(d))
            });
        ensure!(
            one,
            "{when}, a group key of `today()` and a measure of `max(read_at)` read the clock \
             `read_at` does, {clock:?}: one group, of that day; it answers {rows:?}"
        );
    }
    // Every page of a read reads the clock of its first, at the head too,
    // on each door that hands out a cursor.
    for door in ["find", "query", "search"] {
        let clocks = page_clocks(c.as_ref(), own, door).await?;
        ensure!(
            clocks.len() == 4 && clocks[0].is_some() && clocks.iter().all(|t| *t == clocks[0]),
            "every page of `{door}` continues the first and reads its clock, four pages of \
             one bill each: they read {clocks:?}"
        );
    }
    // A listing judges what a proposed call reaches under the clock of the
    // position it reads, never an instant the caller names: `bynow` names
    // `held`, which `rv` is not answered, once `cutoff` is past, three
    // seconds after the call is proposed. Under `valid_at` the reviewer
    // could move the clock until the hold shows, and learn `cutoff`.
    for d in [
        json!({"declare": "type", "name": "partner", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "deal", "fields": [
            {"name": "partner", "type": {"kind": "reference", "target": "partner"}},
            {"name": "cutoff", "type": {"kind": "date_time"}}]}),
        json!({"declare": "verb", "name": "bynow", "on": "deal", "risk": "high", "effect": "write",
            "set": [{"field": "partner", "value": "if(cutoff < now(), 'held', 'p1')"}]}),
    ] {
        l.declare_json(d).await?;
    }
    for k in ["p1", "held"] {
        l.upsert("partner", k, json!({ "n": k })).await?;
    }
    restrict(store.as_ref(), "partner", "held", "a hold").await?;
    // So does a holder of `only` judge a write as the accept would plan it:
    // `job`'s owner is derived from the clock, and once `cutoff` is past a
    // write leaves `j1` outside `al`'s pairs (lakelands#914). `al` is not
    // shown `deal`, so its listing is of `j1` alone.
    l.declare_json(json!({"declare": "type", "name": "job", "fields": [
        {"name": "n", "type": {"kind": "text"}},
        {"name": "cutoff", "type": {"kind": "date_time"}},
        {"name": "owner", "type": {"kind": "text"}, "authority": "derived",
         "derived": "if(cutoff < now(), 'bob', 'alice')", "computed_at": "write"}]}))
        .await?;
    let (w, rv) = (
        holder(store.as_ref(), "w", json!({"default": "write"})),
        holder(
            store.as_ref(),
            "rv",
            json!({"default": "read", "review": true}),
        ),
    );
    let hers = json!({"job": {"only": {"owner": "alice"}}, "deal": {"default": "none"}});
    let al = json!({"default": "read", "review": true, "types": hers});
    let al = holder(store.as_ref(), "al", al);
    // What follows lands before `cutoff`: the deal, `j1`, the proposal on
    // it and `bynow`'s.
    let cutoff = Timestamp(now_micros() + 3_000_000);
    let deal = json!({"partner": "p1", "cutoff": cutoff.to_string()});
    l.upsert("deal", "e1", deal).await?;
    let job = json!({"n": "a", "cutoff": cutoff.to_string()});
    l.upsert("job", "j1", job).await?;
    let write = json!({"write": "upsert", "type": "job", "key": "j1", "patch": {"n": "b"}});
    let asked = c.propose(&w, de(write), "mine".into()).await;
    asked.must("`w`'s proposal on `j1`")?;
    let (verb, key) = ("bynow".into(), "e1".into());
    let called = c.call(
        &w,
        &verb,
        Some(&key),
        Default::default(),
        Default::default(),
    );
    let proposed = called.await.must("`w`'s `bynow`")?.position();
    ensure!(
        now_micros() < cutoff.0,
        "the setup proposes before `cutoff`, {cutoff}: a slow runner, not the clock"
    );
    pause(Duration::from_millis(3200)).await;
    let before = Timestamp(cutoff.0 - 1_000_000);
    let (mut listed, mut hers) = (Vec::new(), Vec::new());
    for (as_of, valid_at, clock) in [
        (Some(proposed), None, None),
        (None, None, None),
        (None, Some(before), None),
        (Some(proposed), Some(valid), None),
        (None, None, Some(before)),
    ] {
        let filter = lakelands_api::write::ProposalFilter {
            at: ReadAt {
                as_of,
                valid_at,
                clock,
            },
            ..Default::default()
        };
        let found = c
            .proposals(&rv, filter.clone())
            .await
            .must("the reviewer's listing")?;
        listed.push(found.data.len());
        let found = c.proposals(&al, filter).await.must("`al`'s listing")?;
        hers.push(serde_json::to_value(found.data).unwrap_or_default());
    }
    ensure!(
        listed == [2, 1, 1, 2, 2],
        "as of its proposal, read after `cutoff`, `bynow` names `p1` and is listed to `rv`, \
         beside the write on `j1`; at the head it names `held` and is not; `valid_at` moves \
         neither, before `cutoff` at the head nor after it as of the proposal; the host's \
         clock before `cutoff` at the head does: listed {listed:?} (as of; at the head; at \
         the head valid before `cutoff`; as of, valid in 2500; at the head, the host's clock \
         before `cutoff`)"
    );
    ensure!(
        hers[0] == hers[3] && hers[0] == hers[4] && hers[1] == hers[2] && hers[0] != hers[1],
        "to `al`, `w`'s write on `j1` is judged as of its proposal with `alice` its owner, \
         and at the head with `bob`, and `valid_at` moves neither; the host's clock before \
         `cutoff` makes it `alice` at the head: listed {hers:?} (as of; at the head; at the \
         head valid before `cutoff`; as of, valid in 2500; at the head, the host's clock)"
    );
    Ok(())
}

/// The `read_at` each page of one read of `bill` through `door` reads,
/// a bill a page, pausing between pages so a clock read again would move.
async fn page_clocks(
    c: &dyn Catalog,
    own: &Principal,
    door: &str,
) -> Result<Vec<Option<Timestamp>>, Failure> {
    let (mut clocks, mut cursor): (_, Option<Cursor>) = (Vec::new(), None);
    loop {
        let (read, next) = match door {
            "find" => {
                let find = Find {
                    limit: Some(1),
                    cursor,
                    ..Find::default()
                };
                let Found::Records {
                    rows, next_cursor, ..
                } = c
                    .find(own, &"bill".into(), find)
                    .await
                    .must("a page of `find`")?
                    .data
                else {
                    return Err(Failure::new("a page of records"));
                };
                (rows.first().and_then(instant), next_cursor)
            }
            "query" => {
                let mut p = pattern("bill", None, Vec::new(), Projection::default());
                (p.limit, p.cursor) = (Some(1), cursor);
                let QueryResult::Rows { rows, next_cursor } =
                    c.query(own, p).await.must("a page of `query`")?.data
                else {
                    return Err(Failure::new("a pattern's rows"));
                };
                let read = rows.first().and_then(|r| {
                    let (_, t) = r.iter().find(|(path, _)| path.ends_with("read_at"))?;
                    t.as_str()?.parse().ok()
                });
                (read, next_cursor)
            }
            _ => {
                let search = Search {
                    ty: "bill".into(),
                    mode: SearchMode::lexical("bill"),
                    k: 1,
                    cursor,
                    filter: None,
                    with_meta: false,
                    matches: false,
                    at: ReadAt::default(),
                    rerank: None,
                };
                let hits = c.search(own, search).await.must("a page of `search`")?.data;
                (
                    hits.hits.first().and_then(|h| instant(&h.record)),
                    hits.next_cursor,
                )
            }
        };
        clocks.push(read);
        let Some(next) = next else {
            return Ok(clocks);
        };
        cursor = Some(next);
        pause(Duration::from_millis(20)).await;
    }
}

fn now_micros() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    now.map_or(0, |d| d.as_micros() as i64)
}
