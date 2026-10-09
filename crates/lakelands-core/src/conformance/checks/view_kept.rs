//! A view kept as its sources are written answers as the same view
//! computed (`view_kept.<type>`, `docs/seams.md` §SystemVocabulary).
//!
//! Each check is a table of events over one lake, one event a line, in
//! rows that bear a name. A view of the table is declared twice: as
//! `<name>`, which the table's `view_kept.` settings name, and as
//! `<name>_twin`, which no setting names and which reads the twins of the
//! views it reads. The two must answer alike: after each event the
//! records the owner finds in the views that read what the event wrote;
//! at each row's end what a reader without the `restricted` grant finds
//! in them, and the owner as of the end of the row before; at a `read`
//! line every door of the views it names, for the owner and, while a
//! record is restricted, for that reader; at the table's end every door
//! of every view for a reader kept from a field of it, for one kept from
//! a field it reads of its source, whom both refuse where the view is
//! grouped, and for one kept from a group field of a merged source. Then the lake is
//! exported, rebuilt, compacted and imported: after the rebuild and in
//! the imported lake the owner reads every door of every view, each time
//! the five readers find their records, and the owner as of every row's
//! end; and the lines after `then` are sent to the lake and to its copy.
//!
//! A lake is held to keep: a `says` line is exact, so a table is red at
//! its first `says ... kept` on a lake that keeps no view, and each row is
//! red where a kept record answers something else than its twin.

use std::collections::BTreeSet;

use lakelands_api::describe::{Kept, NotKept};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Projection, Walk};
use lakelands_api::read::{AggFunc, Direction, OrderBy};
use lakelands_api::seam::{SETTING_TYPE, SORT_INDEX_SETTING, Seams, VIEW_KEPT_SETTING};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Access, Catalog, Find, Found, LakeStore, Position, Principal, ReadAt, Record, Reply, SourceId,
    Timestamp, TypeName, Value, WriteOpts,
};
use serde_json::json;

use super::export::Scratch;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, doors, get_at, ingest, order, owner, patch, pattern, reads_all,
    restrict, source_record, step, try_upsert,
};
use crate::conformance::{Failure, Given, Held, ImportedLake, Store};

check!(
    KEPT_SHAPES = a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes,
    "the `view_kept.` setting, by a twin: a view named by a `view_kept.` setting and the same view under no setting answer alike, in the records the owner finds after each event and a reader without the `restricted` grant at each row's end, as of the row before, and, where the table reads, through every door of a view (`find`, a count and `find` by a text group field, two pages in an order, an aggregate, `get` of a key and of none, a pattern's start and a walk over a derived relation); at the table's end for a reader kept from a field of the view, for one kept from a field it reads of its source and for one kept from a group field of a merged source, and a reference and a link to its records; then after a rebuild, a compact and an import, and for the events sent after them; so a grouped view, one with a `where`, an ungrouped one, one with a merged source, one over a kept view, asked for before the view it reads, and one grouped by a field a recorded output fills, while members arrive, change, move to another group, to none and leave, a member is restricted under the outer view's `where`, and every member of an ungrouped view is",
    given
);

check!(
    KEPT_VALUES = a_kept_measure_answers_as_its_twin_whatever_values_it_folds,
    "the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: an extreme whose holder leaves, equal values at two keys and an equal value written over the holder, which reads as written; a maximum whose holder `1.5` leaves for `1`, whose sort key begins its own; a group's values under a number and a decimal group field, written and fed, where `1.0` and `1.00` are two groups of a decimal and one of a number; the holder of an outer view's extreme leaving a group whose other member is in it on the whole face alone; a sum whose large member leaves, and sums of `5` and `0.00` in both key orders; a setting written over a group whose measure divides by zero, after which the next write is accepted and the view is refused as its twin is until the member changes; and `max(known_at(n))`, null in both",
    given
);

check!(
    KEPT_POSITIONS = a_kept_view_answers_as_its_twin_at_every_position_and_under_every_setting,
    "the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: the kept view and its twin answer alike while a setting its `where` names is written, restricted, lifted, retired and written again, and one a measure's argument names; while its own setting is retired under a kept view over it, written `false` and written again; while a `view_kept.` setting that the `where` of another view names is written; while it is declared with a parameter, which `describe_type` says cannot be kept, and without one, and again with another `where`; while a function a field of its source calls is declared again, and the source with another derived expression, each computing the source's records again, after which it is kept; and its source is migrated; a read at a valid time answers what the twin computes there; `describe_type` says `not_asked` of a view no setting holds for, never `kept` of a view over one that is not, and `not_asked` of every twin",
    given
);

check!(
    KEPT_RUNS = a_kept_extreme_answers_as_its_twin_when_a_run_or_a_re_key_moves_its_holders,
    "the `view_kept.` setting, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: a kept `min` and `max` answer as their twins after one aging run takes the holders of nine groups out of the view's `where`, after a purge retires the holders of nine groups, and after a re-key moves a holder to a greater key and the keys of the records that refer to it in the same event, `describe_type` saying `kept` after each, since every group's next holder is read from its extreme's order in the fold of the event that took the holder; and after the appends that follow each",
    given
);

check!(
    KEPT_ORDER = a_kept_view_answers_every_ordered_page_from_its_order_index_as_its_twin_computes,
    "041 and lakelands#668: a `sort_index.<view>` setting asks for an order index of a kept view's records, and every page in an order it keeps answers as the same view computed, by a twin read as `a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` reads its own: pages of two by the group field and by each measure, both ways, and by the last measure below 100, each continued by its cursor to its last page, for the owner, a reader without the `restricted` grant, readers kept from a field of the view (refused alike where an order names it), of its source and of a merged source, and one holding `only` on the source by the group field, a kept page handing out a cursor where its twin's does; for the owner as of the row before and, at the table's end, after a rebuild, a compact and an import, as of every row's end; while the index is asked for before its view is kept and after, of a merged view too, members arrive, change, move to another group and leave, a group is restricted whole and another reads otherwise on its whole face, the view is declared again, and the setting is retired and asked for again",
    given
);

const SHAPES: &str = r#"
    type line g:text n:decimal
    type credit g:text n:decimal
    type odd n:decimal
    view sums = line by g:text | c:number=count(); total:decimal=sum(n); mean:decimal=avg(n)
    view some = line by g:text | c:number=count(); total:decimal=sum(n) | {"where": "n > 0"}
    view whole = line by | c:number=count(); total:decimal=sum(n)
    view net = line by g:text | total:decimal=sum(n); credited:decimal; left:decimal=total - credited | {"merge": [{"source": "credit", "group_by": ["g"], "measures": [{"name": "credited", "expr": "sum(n)"}]}]}
    view large = sums by g:text | c:number=count(); held:decimal=sum(total) | {"where": "total > 100"}
    view lone = odd by | c:number=count(); total:decimal=sum(n)
    declare {"declare": "function", "name": "echo", "backing": "conformance", "inputs": [{"name": "g", "type": {"kind": "text"}}], "config": {"token": "view_kept", "echo": "g"}}
    declare {"declare": "type", "name": "tagged", "fields": [{"name": "g", "type": {"kind": "text"}}, {"name": "tag", "type": {"kind": "text"}, "authority": "derived", "function": "echo", "depends_on": ["g"]}]}
    view tags = tagged by tag:text | c:number=count()
    row a view is made over the records its source holds
    put line a {"g": "x", "n": "1.50"}
    put line b {"g": "x", "n": "2.5"}
    put line c {"g": "y", "n": "-3"}
    put credit r {"g": "x", "n": "1"}
    set view_kept.large true
    says large not kept here
    set view_kept.sums true
    says sums kept
    says large kept
    set view_kept.some true
    set view_kept.whole true
    set view_kept.net true
    set view_kept.lone true
    read sums some whole net large lone
    is sums x 2,4.00,2.00; y 1,-3,-3
    is net x 4.00,1,3.00; y -3,0,-3
    row a member changes, moves to another group and leaves
    put line a {"n": "4"}
    put line b {"g": "y"}
    retire line c
    is sums x 1,4,4; y 1,2.5,2.5
    retire line b
    put credit s {"g": "z", "n": "2"}
    is net x 4,1,3; z 0,2,-2
    row a member of no group arrives, joins a group and leaves it for none
    put line n {"n": "9"}
    put line n {"g": "z"}
    is sums x 1,4,4; z 1,9,9
    put line n {"g": null}
    is sums x 1,4,4
    row a group field is filled by a recorded output
    set view_kept.tags true
    put tagged t1 {"g": "p"}
    put tagged t2 {"g": "p"}
    put tagged t3 {"g": "q"}
    says tags kept
    is tags p 2; q 1
    row a restricted member under an outer where
    put line m1 {"g": "m", "n": "150"}
    put line m2 {"g": "m", "n": "-100"}
    put line t {"g": "t", "n": "4"}
    hold line m2
    read sums large
    row the restriction is lifted
    lift line m2
    row an ungrouped view whose members are all restricted
    put odd o1 {"n": "3"}
    hold odd o1
    put odd o2 {"n": "4"}
    hold odd o2
    read lone
    row one of them is restricted no more
    lift odd o1
    then
    row the lake goes on after it
    put line a {"n": "5"}
    hold line m2
    put odd o3 {"n": "1"}
"#;

const VALUES: &str = r#"
    type line g:text n:decimal
    type part g:text n:decimal z:decimal
    view sums = line by g:text | c:number=count(); total:decimal=sum(n); mean:decimal=avg(n)
    view ends = line by g:text | low:decimal=min(n); high:decimal=max(n)
    view rate = part by g:text | total:decimal=sum(n); parts:decimal=sum(z); per:decimal=total / parts
    view seen = part by g:text | at:date_time=max(known_at(n))
    type gauge g:text n:decimal m:number
    view heat = gauge by g:text | t:number=sum(n)
    view byheat = heat by t:number | c:number=count()
    view bym = gauge by m:number | c:number=count()
    view byn = gauge by n:decimal | c:number=count()
    declare {"declare": "type", "name": "fed", "fields": [{"name": "g", "type": {"kind": "text"}, "authority": "source"}, {"name": "n", "type": {"kind": "decimal"}, "authority": "source"}, {"name": "m", "type": {"kind": "number"}, "authority": "source"}]}
    view fbym = fed by m:number | c:number=count()
    view fbyn = fed by n:decimal | c:number=count()
    type order c:text amount:decimal
    view spend = order by c:text | total:decimal=sum(amount); tier:text=if(sum(amount) > 1000, 'gold', 'std')
    view tiers = spend by tier:text | most:decimal=max(total)
    row an extreme whose holder leaves, and equal values at two keys
    put line d {"g": "w", "n": "7"}
    put line e {"g": "w", "n": "7"}
    put line f {"g": "w", "n": "9"}
    set view_kept.ends true
    set view_kept.sums true
    retire line d
    is ends w 7,9
    put line e {"n": "8"}
    retire line f
    is ends w 8,8
    row an equal value written over the holder reads as written
    put line k1 {"g": "v", "n": "1.0"}
    put line k2 {"g": "v", "n": "1.00"}
    is ends v 1.0,1.0; w 8,8
    put line k1 {"n": "1.00"}
    is ends v 1.00,1.00; w 8,8
    row a maximum's holder leaves for a value whose key begins its own
    put line o1 {"g": "o", "n": "1.5"}
    put line o2 {"g": "o", "n": "1"}
    put line o3 {"g": "o", "n": "0.9"}
    retire line o1
    is ends o 0.9,1; v 1.00,1.00; w 8,8
    row a sum whose large member leaves
    put line big {"g": "u", "n": "10000000000000000000"}
    put line small {"g": "u", "n": "0.0000000000000000001"}
    retire line big
    read sums
    is sums o 2,1.9,0.950; u 1,0.0000000000000000001,0.0000000000000000001; v 2,2.00,1.00; w 1,8,8
    row a sum reads at the largest scale in both key orders
    put line s1 {"g": "p", "n": "5"}
    put line s2 {"g": "p", "n": "0.00"}
    put line t1 {"g": "q", "n": "0.00"}
    put line t2 {"g": "q", "n": "5"}
    retire line s2
    retire line t1
    row a view made over a group whose measure fails
    put part f1 {"g": "f", "n": "1", "z": "0"}
    put part f2 {"g": "h", "n": "4", "z": "2"}
    set view_kept.rate true
    put part f3 {"g": "h", "n": "2", "z": "2"}
    read rate
    put part f1 {"z": "4"}
    says rate kept
    is rate f 1,4,0.25; h 6,4,1.50
    row an argument is read without its provenance
    set view_kept.seen true
    is seen f -; h -
    row a number group field holds one value whatever its writer wrote, written and fed; a decimal's scales are two groups
    put gauge b {"g": "m", "n": "1", "m": 1}
    set view_kept.heat true
    set view_kept.byheat true
    set view_kept.bym true
    set view_kept.byn true
    put gauge z {"g": "z", "n": "1.0", "m": 1.0}
    read heat byheat bym byn
    put gauge a {"g": "a", "n": "1.00", "m": 1.0}
    read heat byheat bym byn
    retire gauge b
    read heat byheat bym byn
    is bym 1 2
    is byn 1.0 1; 1.00 1
    set view_kept.fbym true
    set view_kept.fbyn true
    feed meter fed b {"g": "m", "n": "1", "m": 1}
    feed meter fed z {"g": "z", "n": "1.0", "m": 1.0}
    read fbym fbyn
    feed meter fed a {"g": "a", "n": "1.00", "m": 1.0}
    read fbym fbyn
    retire fed b
    read fbym fbyn
    is fbym 1 2
    is fbyn 1.0 1; 1.00 1
    row an outer extreme's holder leaves a group its other member is in on the whole face alone
    put order c1 {"c": "C", "amount": "500"}
    put order c2 {"c": "C", "amount": "1000"}
    hold order c2
    put order d1 {"c": "D", "amount": "2000"}
    set view_kept.spend true
    set view_kept.tiers true
    says tiers kept
    retire order d1
    says tiers kept
    read tiers
    then
    row the lake goes on after it
    put line e {"n": "1"}
    retire line k2
    put part f2 {"z": "0"}
"#;

const POSITIONS: &str = r#"
    type sale g:text net:decimal note:text
    declare {"declare": "type", "name": "contract", "valid_from": "from", "valid_to": "to", "fields": [{"name": "debtor", "type": {"kind": "text"}}, {"name": "amount", "type": {"kind": "decimal"}}, {"name": "from", "type": {"kind": "date_time"}}, {"name": "to", "type": {"kind": "date_time"}}]}
    view book = sale by g:text | c:number=count(); gross:decimal=sum(round(net * setting('fx'), 2)) | {"where": "net > setting('floor')"}
    view shelf = book by g:text | books:number=count(); most:decimal=max(gross)
    view tally = sale by g:text | c:number=count()
    view gated = sale by g:text | c:number=count() | {"where": "setting('view_kept.tally') = true"}
    view owed = contract by debtor:text | due:decimal=sum(amount)
    declare {"declare": "function", "name": "scaled", "backing": "expr", "expr": "net * 2", "inputs": [{"name": "net", "type": {"kind": "decimal"}}]}
    declare {"declare": "type", "name": "item", "fields": [{"name": "g", "type": {"kind": "text"}}, {"name": "net", "type": {"kind": "decimal"}}, {"name": "gross", "type": {"kind": "decimal"}, "authority": "derived", "function": "scaled"}, {"name": "fee", "type": {"kind": "decimal"}, "authority": "derived", "derived": "net + 1"}]}
    view stock = item by g:text | total:decimal=sum(gross); fees:decimal=sum(fee)
    row a view and a view over it are kept from their settings on
    set floor 1
    set fx 2
    put sale s1 {"g": "a", "net": "10", "note": "n"}
    put sale s2 {"g": "a", "net": "0.5"}
    put sale s3 {"g": "b", "net": "3"}
    set view_kept.shelf true
    says shelf not kept here
    set view_kept.book true
    says book kept
    says shelf kept
    set view_kept.tally true
    set view_kept.owed true
    is book a 1,20; b 1,6
    row a setting the where names is written and restricted
    set floor 5
    is book a 1,20
    hold sys.setting floor
    put sale s4 {"g": "b", "net": "7"}
    row the setting is restricted no more
    lift sys.setting floor
    row the setting is retired and written again
    retire sys.setting floor
    set floor 0
    is book a 2,21.0; b 2,20
    row a setting a measure names is written and retired
    set fx 3
    retire sys.setting fx
    set fx 2
    row the setting is retired under a view over the view
    retire sys.setting view_kept.book
    says book not asked
    says shelf not kept here
    put sale s5 {"g": "c", "net": "4"}
    read book shelf
    set view_kept.book true
    says shelf kept
    set view_kept.book false
    says book not asked
    set view_kept.book true
    row a setting that asks for a view is a setting a where may name
    set view_kept.gated true
    says gated kept
    read gated
    set view_kept.tally false
    read gated
    set view_kept.tally true
    says gated kept
    row a parameter is declared and taken away
    view tally = sale by g:text | c:number=count() | {"params": [{"name": "over", "kind": "decimal"}], "where": "net > param('over')"}
    says tally cannot be kept
    put sale s6 {"g": "c", "net": "1"}
    read tally
    view tally = sale by g:text | c:number=count()
    says tally kept
    row the view is declared again under its setting
    view tally = sale by g:text | c:number=count() | {"where": "net > 2"}
    says tally kept
    is tally a 1; b 2; c 1
    row a declaration computes a source's records again
    put item i1 {"g": "k", "net": "3"}
    put item i2 {"g": "k", "net": "4"}
    set view_kept.stock true
    is stock k 14,9
    declare {"declare": "function", "name": "scaled", "backing": "expr", "expr": "net * 3", "inputs": [{"name": "net", "type": {"kind": "decimal"}}]}
    says stock kept
    is stock k 21,9
    declare {"declare": "type", "name": "item", "fields": [{"name": "g", "type": {"kind": "text"}}, {"name": "net", "type": {"kind": "decimal"}}, {"name": "gross", "type": {"kind": "decimal"}, "authority": "derived", "function": "scaled"}, {"name": "fee", "type": {"kind": "decimal"}, "authority": "derived", "derived": "net + 2"}]}
    says stock kept
    is stock k 21,11
    row a source is migrated
    migrate {"name": "sale", "fields": [{"name": "g", "type": {"kind": "text"}}, {"name": "net", "type": {"kind": "decimal"}}]}
    row a read at a valid time
    put contract c1 {"debtor": "d", "amount": "100", "from": "2026-03-01T00:00:00Z", "to": "2026-04-01T00:00:00Z"}
    put contract c2 {"debtor": "d", "amount": "50", "from": "2026-04-01T00:00:00Z", "to": "2026-05-01T00:00:00Z"}
    valid_at 2026-03-15T00:00:00Z owed
    put contract c3 {"debtor": "e", "amount": "7", "from": "2026-03-01T00:00:00Z", "to": "2026-06-01T00:00:00Z"}
    read owed
    is owed d 150; e 7
    was owed d 100; e 7
    then
    row the lake goes on after it
    put sale s7 {"g": "a", "net": "9"}
    set floor 2
    retire contract c1
"#;

const RUNS: &str = r#"
    type bill site:text due:date_time status:text
    declare {"declare": "type", "name": "lead", "fields": [{"name": "site", "type": {"kind": "text"}, "authority": "source"}, {"name": "score", "type": {"kind": "decimal"}, "authority": "source"}]}
    declare {"declare": "type", "name": "acct", "key": ["code"], "fields": [{"name": "code", "type": {"kind": "text"}, "required": true}, {"name": "tag", "type": {"kind": "text"}, "required": true}, {"name": "site", "type": {"kind": "text"}}, {"name": "n", "type": {"kind": "decimal"}}]}
    declare {"declare": "type", "name": "entry", "key": ["of", "no"], "fields": [{"name": "of", "type": {"kind": "reference", "target": "acct"}, "required": true}, {"name": "no", "type": {"kind": "number"}, "required": true}, {"name": "kind", "type": {"kind": "text"}}, {"name": "m", "type": {"kind": "decimal"}}]}
    view oldest = bill by site:text | first:date_time=min(due); open:number=count() | {"where": "status = 'open'"}
    view best = lead by site:text | least:decimal=min(score); most:decimal=max(score)
    view spread = acct by site:text | low:decimal=min(n); high:decimal=max(n); c:number=count()
    view reach = entry by kind:text | least:decimal=min(m); c:number=count()
    row an aging run moves the holders of nine groups
    put sys.aging_rule late {"on": "bill", "field": "status", "from": "open", "to": "late", "when": "due < today()"}
    9* put bill b#a {"site": "s#", "due": "2026-02-0#T00:00:00Z", "status": "open"}
    9* put bill b#b {"site": "s#", "due": "2026-04-0#T00:00:00Z", "status": "open"}
    set view_kept.oldest true
    says oldest kept
    age 2026-03-01T00:00:00Z
    says oldest kept
    read oldest
    put bill more {"site": "s1", "due": "2026-05-01T00:00:00Z", "status": "open"}
    says oldest kept
    row a purge retires the holders of nine groups
    9* feed crm lead l#a {"site": "s#", "score": "1"}
    9* feed erp lead l#b {"site": "s#", "score": "5"}
    set view_kept.best true
    purge crm
    says best kept
    read best
    feed erp lead more {"site": "s1", "score": "9"}
    says best kept
    row a re-key moves a holder to a greater key, and the keys that refer to it
    put acct k1 {"code": "k1", "tag": "k9", "site": "v", "n": "1.0"}
    put acct k2 {"code": "k2", "tag": "k2", "site": "v", "n": "1.00"}
    put acct k3 {"code": "k3", "tag": "k0", "site": "w", "n": "4"}
    put entry k1:1 {"of": "k1", "no": 1, "kind": "e", "m": "2"}
    put entry k2:1 {"of": "k2", "no": 1, "kind": "e", "m": "2"}
    put entry k3:1 {"of": "k3", "no": 1, "kind": "f", "m": "3"}
    set view_kept.spread true
    set view_kept.reach true
    is spread v 1.0,1.0,2; w 4,4,1
    migrate {"name": "acct", "key": ["tag"], "fields": [{"name": "code", "type": {"kind": "text"}, "required": true}, {"name": "tag", "type": {"kind": "text"}, "required": true}, {"name": "site", "type": {"kind": "text"}}, {"name": "n", "type": {"kind": "decimal"}}]}
    read spread reach
    is spread v 1.00,1.00,2; w 4,4,1
    put acct k5 {"code": "k5", "tag": "k5", "site": "w", "n": "6"}
    says spread kept
    then
    row the lake goes on after it
    put bill b1b {"status": "late"}
    age 2026-04-05T00:00:00Z
    feed erp lead l2c {"site": "s2", "score": "0"}
    put acct k9 {"n": "3"}
"#;

const ORDERS: &str = r#"
    type line g:text n:decimal
    type credit g:text n:decimal
    view sums = line by g:text | c:number=count(); total:decimal=sum(n)
    view some = line by g:text | c:number=count(); total:decimal=sum(n) | {"where": "n > 0"}
    view net = line by g:text | total:decimal=sum(n); credited:decimal; left:decimal=total - credited | {"merge": [{"source": "credit", "group_by": ["g"], "measures": [{"name": "credited", "expr": "sum(n)"}]}]}
    row an order index is asked for before its view is kept
    set sort_index.sums ["g", "c", "total"]
    put line a {"g": "x", "n": "1.5"}
    put line b {"g": "x", "n": "2.5"}
    put line c {"g": "y", "n": "-3"}
    put line d {"g": "z", "n": "7"}
    put credit r {"g": "x", "n": "1"}
    set view_kept.sums true
    says sums kept
    read sums
    row a write moves a group's place in every order
    put line c {"n": "30"}
    read sums
    row a group ends, another begins and a member moves to it
    retire line d
    put line e {"g": "w", "n": "0.5"}
    put line b {"g": "w"}
    read sums
    row a group restricted whole, and one that reads otherwise on its whole face
    put line f {"g": "v", "n": "100"}
    hold line f
    put line m1 {"g": "m", "n": "150"}
    put line m2 {"g": "m", "n": "-100"}
    hold line m2
    read sums
    row the index is asked for after its view is kept, and of a merged view
    set view_kept.some true
    set sort_index.some ["total", "c"]
    set view_kept.net true
    set sort_index.net ["left", "g"]
    read some net
    row the view is declared again
    view sums = line by g:text | c:number=count(); total:decimal=sum(n) | {"where": "n > 1"}
    says sums kept
    read sums
    row the setting is retired and asked for again
    retire sys.setting sort_index.sums
    read sums
    set sort_index.sums ["total"]
    read sums
    then
    row the lake goes on after it
    put line a {"n": "9"}
    lift line f
    put line g {"g": "x", "n": "4"}
"#;

/// Each check's function: its table, run by [`twin`].
macro_rules! tables {
    ($($run:ident = $table:ident,)*) => {$(
        async fn $run(store: Store, given: Given) -> Held {
            twin(store, given, $table).await
        }
    )*};
}
tables! {
    a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes = SHAPES,
    a_kept_measure_answers_as_its_twin_whatever_values_it_folds = VALUES,
    a_kept_view_answers_as_its_twin_at_every_position_and_under_every_setting = POSITIONS,
    a_kept_extreme_answers_as_its_twin_when_a_run_or_a_re_key_moves_its_holders = RUNS,
    a_kept_view_answers_every_ordered_page_from_its_order_index_as_its_twin_computes = ORDERS,
}

/// A view of a table, declared as `name` and as `name_twin`.
#[derive(Clone, Default)]
struct Pair {
    name: String,
    source: String,
    /// Whether its source is a view of the table: its twin reads that
    /// view's twin.
    over: bool,
    /// What it reads through: its own declaration, types, and settings
    /// (see [`setting`]).
    reads: Vec<String>,
    /// Its first group field, and a value a record of it has there.
    by: Option<String>,
    seen: Option<String>,
    /// Its merged sources, each with its first group field.
    merged: Vec<(String, String)>,
    measures: Vec<String>,
    /// Every key a record of it has had.
    keys: BTreeSet<String>,
    /// Whether a `view_kept.` setting has named it: it is read from then
    /// on, a view no setting has named being its twin by its declaration.
    asked: bool,
    /// Whether a `sort_index.` setting has named it: each door of it is
    /// read in every order too ([`paged`]).
    indexed: bool,
}

/// A table as far as it has run.
#[derive(Clone, Default)]
struct Twin {
    row: String,
    pairs: Vec<Pair>,
    /// The views the row's events have reached.
    reached: BTreeSet<usize>,
    /// The head at each row's end, and the views the row reached.
    ends: Vec<(Position, BTreeSet<usize>)>,
    /// A valid time, and the view that is read at it too.
    valid_at: Option<(Timestamp, String)>,
    /// How many records are restricted.
    held: u32,
    /// The rows that failed, each with the first thing that failed it.
    red: Vec<String>,
}

/// Runs `table` on the lake, then holds it through an export, a rebuild,
/// a compact and an import; fails naming every row that failed.
async fn twin(store: Store, given: Given, table: &str) -> Held {
    let lines: Vec<&str> = table.lines().map(str::trim).collect();
    let then = lines.iter().position(|l| *l == "then");
    let (first, then) = lines.split_at(then.unwrap_or(lines.len()));
    let s = store.as_ref();
    let mut t = Twin::default();
    t.run(s, first).await;
    t.named(s).await;
    t.everywhere(s, "at the end", [1, 1, 2]).await;
    let scratch = Scratch::new(given.scratch.clone());
    let from = scratch.dir("from");
    let exported = crate::export::write(s, &from).await;
    exported.must("exporting the lake")?;
    let (mut copy, before) = (t.clone(), t.red.len());
    s.rebuild().await.must("rebuilding the projection")?;
    t.everywhere(s, "after a rebuild", [2, 1, 1]).await;
    t.run(s, then).await;
    s.compact().await.must("compacting the lake")?;
    t.everywhere(s, "after a compact", [1, 1, 1]).await;
    let imported = (given.import)(from, s.lake().clone(), None, Seams::default()).await;
    let opened = |e| Failure::new(format!("importing the export, or opening it to write: {e}"));
    let imported: ImportedLake = imported.map_err(opened)?;
    // Every door of the owner's continues a cursor after a write, so it
    // reads the import once it is open to write.
    let read = imported.store.as_ref();
    copy.everywhere(read, "in the imported lake", [1, 1, 1])
        .await;
    let written = (imported.writer)(imported.store, Seams::default()).await;
    let written = written.map_err(opened)?;
    let when = "in the imported lake open to write";
    copy.everywhere(written.as_ref(), when, [2, 0, 0]).await;
    copy.run(written.as_ref(), then).await;
    let more = copy.red.iter().skip(before);
    let red: Vec<&str> = t.red.iter().chain(more).map(String::as_str).collect();
    ensure!(
        red.is_empty() && !t.ends.is_empty(),
        "a kept view must answer as its twin; {} rows failed: {}",
        red.len(),
        red.join("; and ")
    );
    Ok(())
}

impl Twin {
    /// Fails the row, with the first thing that failed it.
    fn fail(&mut self, what: String) {
        let row = format!("row `{}`: ", self.row);
        if !self.red.iter().any(|r| r.starts_with(&row)) {
            self.red.push(row + &what);
        }
    }

    /// Sends each line to the lake, `n* ` in front of one `n` times with
    /// `#` as its number. After each event the owner finds, at the head,
    /// the records of every view a setting has named that reads what the
    /// event wrote (every one after a migration, a run and a purge), and
    /// of its twin; at a `read` line the owner reads the views it names
    /// through every door, and the reader without the `restricted` grant
    /// too while a record is restricted.
    async fn run(&mut self, store: &dyn LakeStore, lines: &[&str]) {
        for line in lines.iter().filter(|l| !l.is_empty() && **l != "then") {
            let (times, line) = match line.split_once("* ") {
                Some((n, rest)) if n.parse::<u32>().is_ok() => (n.parse().unwrap_or(1), rest),
                _ => (1, *line),
            };
            let word = line.split(' ').next().unwrap_or_default();
            if word == "row" {
                self.row_ends(store).await;
            } else if word == "read" {
                let views = self.those(|p| line.split(' ').skip(1).any(|v| v == p.name));
                self.note(store, &views).await;
                let plain = 2 * u8::from(self.held > 0);
                let when = "through every door";
                self.same(store, when, None, &views, [2, plain, 0]).await;
                continue;
            }
            for i in 1..=times {
                let line = line.replace('#', &i.to_string());
                if let Err(e) = self.line(store, &line).await {
                    self.fail(format!("`{line}`: {e}"));
                }
            }
            let words: Vec<&str> = line.splitn(4, ' ').collect();
            let wrote = match (word, words.get(1..3)) {
                ("row" | "valid_at" | "is" | "was" | "says", _) => continue,
                ("set", Some(&[key, _])) | (_, Some(&[SETTING_TYPE, key])) => setting(key),
                ("put" | "retire" | "hold" | "lift" | "view", Some(w)) => w[0].to_owned(),
                ("feed", Some(w)) => w[1].to_owned(),
                _ => String::new(),
            };
            self.held += u32::from(word == "hold");
            self.held -= u32::from(word == "lift").min(self.held);
            let reads = |p: &Pair| p.asked && (wrote.is_empty() || p.reads.contains(&wrote));
            let (reached, when) = (self.those(reads), format!("after `{line}`"));
            self.same(store, &when, None, &reached, [1, 0, 0]).await;
            self.reached.extend(reached);
        }
        self.row_ends(store).await;
    }

    /// At a row's end the reader without the `restricted` grant finds
    /// the records of the views the row's events reached, and the owner
    /// finds them as of the end of the row before it.
    async fn row_ends(&mut self, store: &dyn LakeStore) {
        let reached = std::mem::take(&mut self.reached);
        let (Ok(head), false) = (store.head().await, reached.is_empty()) else {
            return;
        };
        let (when, plain, own) = ("at its end", [0, 1, 0], [1, 0, 0]);
        self.same(store, when, None, &reached, plain).await;
        if let Some((before, _)) = self.ends.last().cloned() {
            self.same(store, when, Some(before), &reached, own).await;
        }
        self.ends.push((head, reached));
    }

    /// Holds every view a setting has named to its twin at the head, each
    /// of the three readers reading to its `depth` (see [`Twin::same`]),
    /// and as of each row's end the views that row reached, as the owner
    /// finds them.
    async fn everywhere(&mut self, store: &dyn LakeStore, when: &str, depth: [u8; 3]) {
        self.row = when.to_owned();
        let asked = self.those(|p| p.asked);
        self.note(store, &asked).await;
        self.same(store, when, None, &asked, depth).await;
        for (at, reached) in self.ends.clone() {
            self.same(store, when, Some(at), &reached, [1, 0, 0]).await;
        }
    }

    /// The views of the table that `which` holds for.
    fn those(&self, which: impl Fn(&Pair) -> bool) -> BTreeSet<usize> {
        let views = self.pairs.iter().enumerate().filter(|(_, p)| which(p));
        views.map(|(i, _)| i).collect()
    }

    /// Sends one line of a table to the lake, or holds the lake to it.
    async fn line(&mut self, store: &dyn LakeStore, line: &str) -> Held {
        let l = Lake::of(store);
        let bad = |why: String| Failure::new(format!("the table's line `{line}`: {why}"));
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let cut = |n: usize| {
            let parts: Vec<&str> = rest.splitn(n, ' ').collect();
            let whole = parts.len() == n;
            whole.then_some(parts).ok_or_else(|| bad("is short".into()))
        };
        let json = |text: &str| serde_json::from_str::<Value>(text).map_err(|e| bad(e.to_string()));
        let instant = |text: &str| Timestamp::parse(text).map_err(|e| bad(e.0));
        match (word, cut(2), cut(3)) {
            ("row", ..) => self.row = rest.to_owned(),
            ("type", Ok(p), _) => {
                let ty = json!({"declare": "type", "name": p[0], "fields": kinds(p[1])});
                l.declare_json(ty).await?;
            }
            ("declare", ..) => drop(l.declare_json(json(rest)?).await?),
            ("view", ..) => {
                for declared in self.views(rest).map_err(bad)? {
                    l.declare_json(declared).await?;
                }
            }
            ("put", _, Ok(p)) => drop(l.upsert(p[0], p[1], json(p[2])?).await?),
            ("retire", Ok(p), _) => drop(l.retire(p[0], p[1]).await?),
            ("set", Ok(p), _) => {
                let view = p[0].strip_prefix(VIEW_KEPT_SETTING).unwrap_or_default();
                let named = self.pairs.iter_mut().find(|v| v.name == view);
                named.into_iter().for_each(|v| v.asked = true);
                let view = p[0].strip_prefix(SORT_INDEX_SETTING).unwrap_or_default();
                let named = self.pairs.iter_mut().find(|v| v.name == view);
                named.into_iter().for_each(|v| v.indexed = true);
                l.set(p[0], json(p[1])?).await?;
            }
            ("hold", Ok(p), _) => drop(restrict(store, p[0], p[1], "held").await?),
            ("lift", Ok(p), _) => {
                let lifted = store.unrestrict(&p[0].into(), &p[1].into()).await;
                lifted.must("lifting a restriction")?;
            }
            ("migrate", ..) => {
                let to = serde_json::from_str(rest).map_err(|e| bad(e.to_string()))?;
                l.migrate(to).await?;
            }
            ("age", ..) => drop(store.age(instant(rest)?).await.must("an aging run")?),
            ("feed", ..) => {
                let p = cut(4)?;
                let fed = vec![source_record(p[1], p[2], json(p[3])?)];
                let fed = ingest(l.cat.as_ref(), &l.own, p[0], fed).await;
                fed.must("feeding a record")?;
            }
            ("purge", ..) => {
                let purged = store.purge_source(&SourceId::from(rest), None, None).await;
                purged.must("purging a source")?;
            }
            ("valid_at", Ok(p), _) => self.valid_at = Some((instant(p[0])?, p[1].to_owned())),
            ("is" | "was", Ok(p), _) => {
                let valid_at = self.valid_at.as_ref().map(|v| v.0);
                let valid_at = valid_at.filter(|_| word == "was");
                self.is(&l, p[0], p[1], valid_at).await?;
            }
            ("says", Ok(p), _) => says(&l, p[0], p[1]).await?,
            _ => return Err(bad("is no event, or is short".into())),
        }
        Ok(())
    }

    /// The declarations of a view and of its twin, from
    /// `name = source by field:kind .. | measure:kind=expr; .. | rest`,
    /// where a field a merged source fills has no `=expr` and `rest` is
    /// the view's other members. Each carries a relation `of_<name>`,
    /// derived from its first group field to its source's.
    fn views(&mut self, spec: &str) -> Result<Vec<Value>, String> {
        let mut parts = spec.split(" | ");
        let head: Vec<&str> = parts.next().unwrap_or_default().split(' ').collect();
        let (&[name, "=", source, "by"], by) = head.split_at(head.len().min(4)) else {
            return Err("is no view".into());
        };
        let measures = parts.next().unwrap_or_default().split("; ");
        let measures: Vec<(&str, Option<&str>)> = measures
            .map(|m| m.split_once('=').map_or((m, None), |(f, e)| (f, Some(e))))
            .collect();
        let rest = serde_json::from_str::<Value>(parts.next().unwrap_or("{}"));
        let rest = rest.map_err(|e| e.to_string())?;
        let inner = self.pairs.iter().find(|p| p.name == source);
        let kept = setting(&format!("{VIEW_KEPT_SETTING}{name}"));
        let over = inner.is_some();
        let mut reads = vec![name.to_owned(), source.to_owned(), kept];
        reads.extend(inner.into_iter().flat_map(|p| p.reads.clone()));
        let settings = spec.split("setting('").skip(1);
        reads.extend(settings.filter_map(|s| Some(setting(s.split('\'').next()?))));
        let merged = rest["merge"].as_array().into_iter().flatten();
        let first = |m: &Value| {
            Some((
                m["source"].as_str()?.into(),
                m["group_by"][0].as_str()?.into(),
            ))
        };
        let merged: Vec<(String, String)> = merged.filter_map(first).collect();
        reads.extend(merged.iter().map(|m| m.0.clone()));
        let mut fields = kinds(&by.join(" "));
        fields.extend(measures.iter().flat_map(|(f, _)| kinds(f)));
        let called = |f: &str| f.split(':').next().unwrap_or_default().to_owned();
        let keys: Vec<String> = by.iter().map(|f| called(f)).collect();
        let exprs = measures
            .iter()
            .filter_map(|(f, e)| Some((called(f), (*e)?)));
        let exprs: Vec<Value> = exprs.map(|(n, e)| json!({"name": n, "expr": e})).collect();
        let declared = |end: &str| {
            let read = if over { end } else { "" };
            let by = json!([{"from": keys.first(), "to": keys.first()}]);
            let near = json!({"name": format!("of_{name}{end}"), "derived": {"match": by},
                              "target": format!("{source}{read}")});
            let source = format!("{source}{read}");
            let mut view = json!({"source": source, "group_by": keys, "measures": exprs});
            for (member, value) in rest.as_object().into_iter().flatten() {
                view[member] = value.clone();
            }
            let near = Value::from_iter(keys.first().map(|_| near));
            json!({"declare": "type", "name": format!("{name}{end}"), "key": keys,
                   "fields": fields, "view": view, "relations": near})
        };
        let declared = vec![declared(""), declared("_twin")];
        if !self.pairs.iter().any(|p| p.name == name) {
            let (name, source, by) = (name.to_owned(), source.to_owned(), keys.first().cloned());
            let measures = measures.iter().map(|(f, _)| called(f)).collect();
            let (seen, keys, asked, indexed) = Default::default();
            #[rustfmt::skip]
            self.pairs.push(Pair { name, source, over, reads, by, seen, merged, measures, keys, asked, indexed });
        }
        Ok(declared)
    }

    /// Notes the keys these views' records have, as the owner finds them.
    async fn note(&mut self, store: &dyn LakeStore, views: &BTreeSet<usize>) {
        let l = Lake::of(store);
        for i in views {
            let Some(p) = self.pairs.get_mut(*i) else {
                continue;
            };
            for end in ["", "_twin"] {
                let name = format!("{}{end}", p.name);
                let rows = l.all_found(&name, Find::default()).await;
                let rows = rows.unwrap_or_default();
                let by = p.by.as_ref().zip(rows.first());
                let seen = by.and_then(|(f, r)| r.fields.get(f.as_str())?.as_str());
                p.seen = seen.map(str::to_owned).or(p.seen.take());
                p.keys.extend(rows.iter().map(|r| r.key.to_string()));
            }
        }
    }

    /// Fails the row unless each of these views answers as its twin
    /// does, as of `as_of`, and at the head at the table's valid time
    /// too. `depth` is what each of the readers reads: nothing, the
    /// view's records, or every door. They are the owner, a reader
    /// without the `restricted` grant, one kept from each view's first
    /// measure and, reading as far as that one, one kept from the field
    /// a view groups by in each stored type that is a view's source, one
    /// kept from the first group field of each merged source and one
    /// holding `only` on each stored source by that field, to a value a
    /// view's record has there.
    async fn same(
        &mut self,
        store: &dyn LakeStore,
        when: &str,
        as_of: Option<Position>,
        views: &BTreeSet<usize>,
        depth: [u8; 3],
    ) {
        let cat = doors(store);
        let (mut partial, mut blind) = (reads_all(store, "partial"), reads_all(store, "blind"));
        let mut apart = reads_all(store, "apart");
        for (source, field) in self.pairs.iter().flat_map(|p| &p.merged) {
            let source = apart
                .grants
                .types
                .entry(source.as_str().into())
                .or_default();
            source.fields.insert(field.as_str().into(), Access::None);
        }
        for p in self.pairs.iter().filter(|p| !p.over) {
            let source = blind.grants.types.entry(p.source.as_str().into());
            let hidden = p.by.iter().map(|f| (f.as_str().into(), Access::None));
            source.or_default().fields.extend(hidden);
        }
        for (p, end) in self.pairs.iter().flat_map(|p| [(p, ""), (p, "_twin")]) {
            let hidden = p.measures.first().map(|m| m.as_str().into());
            let hidden = hidden.map(|m| (m, Access::None));
            let (default, fields) = (None, hidden.into_iter().collect());
            let ty = format!("{}{end}", p.name);
            let grants = TypeGrants {
                default,
                fields,
                ..Default::default()
            };
            partial.grants.types.insert(ty.as_str().into(), grants);
        }
        let mut only = reads_all(store, "only");
        for p in self.pairs.iter().filter(|p| !p.over) {
            let Some((by, seen)) = p.by.as_ref().zip(p.seen.as_ref()) else {
                continue;
            };
            let source = only.grants.types.entry(p.source.as_str().into());
            source
                .or_default()
                .only
                .insert(by.as_str().into(), json!(seen));
        }
        let readers = [
            ("the owner", owner(store)),
            ("a reader without `restricted`", reads_all(store, "plain")),
            ("a reader kept from a field of it", partial),
            ("a reader kept from a field of its source", blind),
            ("a reader kept from a field of a merged source", apart),
            ("a reader holding `only` on its source", only),
        ];
        let [own, plain, rest] = depth;
        let depth = [own, plain, rest, rest, rest, rest];
        let own = owner(store);
        let mut differs = None;
        let valid = self.valid_at.as_ref().filter(|_| as_of.is_none());
        for valid in [None].into_iter().chain(valid.map(Some)) {
            let valid_at = valid.map(|v| v.0);
            let at = ReadAt { as_of, valid_at };
            let read = |p: &&Pair| valid.is_none_or(|v| v.1 == p.name);
            for ((reader, who), deep) in readers.iter().zip(depth).filter(|r| r.1 > 0) {
                let deep = deep == 2;
                for p in views.iter().filter_map(|i| self.pairs.get(*i)).filter(read) {
                    let both = (who, &own);
                    let kept = said(cat.as_ref(), both, p, "", at, deep).await;
                    let twin = said(cat.as_ref(), both, p, "_twin", at, deep).await;
                    // A write `said` makes that fails is a door the twin lacks.
                    let (kept, twin) = match (kept, twin) {
                        (Ok(kept), Ok(twin)) => (kept, twin),
                        (Err(f), _) | (_, Err(f)) => (vec![("a write".into(), f.message)], vec![]),
                    };
                    let doors = 0..kept.len().max(twin.len());
                    let Some(i) = doors.into_iter().find(|i| kept.get(*i) != twin.get(*i)) else {
                        continue;
                    };
                    let door = kept.get(i).or(twin.get(i)).map_or("", |d| d.0.as_str());
                    let [kept, twin] = [&kept, &twin].map(|a| a.get(i).map_or("nothing", |d| &d.1));
                    differs = differs.or(Some(format!(
                        "{when}, {reader} reading `{}` through {door} at {at:?} was answered \
                         {kept}, and by its twin {twin}",
                        p.name
                    )));
                }
            }
        }
        differs.into_iter().for_each(|d| self.fail(d));
    }

    /// A write that names a view is answered as one that names its twin:
    /// a type that refers to it, a reference to a record of it and to
    /// none, a relation to it and a link.
    async fn named(&mut self, store: &dyn LakeStore) {
        self.row = "a write that names the view".to_owned();
        let l = Lake::of(store);
        // A refusal by its wire form, its sentence among its members.
        let heard = |answer: lakelands_api::Result<()>| match answer {
            Ok(()) => "taken".to_owned(),
            Err(e) => json!(e).to_string(),
        };
        for p in self.pairs.clone() {
            let Some(key) = p.keys.first().filter(|_| p.asked) else {
                continue;
            };
            let mut answers = Vec::new();
            for end in ["", "_twin"] {
                let view = format!("{}{end}", p.name);
                let (to, fan) = (format!("ref_{view}"), format!("fan_{view}"));
                let at = json!({"name": "at", "type": {"kind": "reference", "target": view}});
                let near = json!({"name": "near", "target": view});
                let mut said = Vec::new();
                for declared in [
                    json!({"declare": "type", "name": to, "fields": [at]}),
                    json!({"declare": "type", "name": fan, "fields": kinds("n:number"),
                           "relations": [near]}),
                ] {
                    said.push(match serde_json::from_value::<Declaration>(declared) {
                        Ok(d) => heard(l.cat.declare(&l.own, d).await.map(drop)),
                        Err(e) => e.to_string(),
                    });
                }
                for at in [key.as_str(), "no such group"] {
                    let put = try_upsert(l.cat.as_ref(), &l.own, &to, at, json!({"at": at}));
                    said.push(heard(put.await.map(drop)));
                }
                let from = try_upsert(l.cat.as_ref(), &l.own, &fan, "f", json!({"n": 1}));
                said.push(heard(from.await.map(drop)));
                let (fan, f, near) = (fan.as_str().into(), "f".into(), "near".into());
                let (to, none) = (key.as_str().into(), patch(json!({})));
                let link = l
                    .cat
                    .link(&l.own, &fan, &f, &near, &to, none, WriteOpts::default());
                said.push(heard(link.await.map(drop)));
                answers.push(said.join(" | ").replace("_twin", ""));
            }
            if answers[0] != answers[1] {
                let (view, kept, twin) = (&p.name, &answers[0], &answers[1]);
                self.fail(format!("`{view}` was answered {kept}, and its twin {twin}"));
            }
        }
    }

    /// Fails unless the owner finds `want` in the view and in its twin:
    /// each record as its key and its measures, `-` for none.
    async fn is(&self, l: &Lake, view: &str, want: &str, valid_at: Option<Timestamp>) -> Held {
        let measures = self.pairs.iter().find(|p| p.name == view);
        let measures = measures.map(|p| p.measures.clone()).unwrap_or_default();
        for end in ["", "_twin"] {
            let mut find = Find::default();
            find.at.valid_at = valid_at;
            let rows = l.all_found(&format!("{view}{end}"), find).await?;
            let read = |r: &Record| {
                let value = |m: &String| match r.fields.get(m.as_str()) {
                    Some(Value::String(s)) => s.clone(),
                    None | Some(Value::Null) => "-".to_owned(),
                    Some(v) => v.to_string(),
                };
                let values: Vec<String> = measures.iter().map(value).collect();
                format!("{} {}", r.key, values.join(","))
            };
            let got = rows.iter().map(read).collect::<Vec<_>>().join("; ");
            ensure!(got == want, "`{view}{end}` reads `{got}`, not `{want}`");
        }
        Ok(())
    }
}

/// The setting `key`, as a view's `reads` name it.
fn setting(key: &str) -> String {
    format!("{SETTING_TYPE} {key}")
}

/// The fields `name:kind ..` declare.
fn kinds(fields: &str) -> Vec<Value> {
    let kinded = fields.split(' ').filter_map(|f| f.split_once(':'));
    let field = |(name, kind)| json!({"name": name, "type": {"kind": kind}});
    kinded.map(field).collect()
}

/// Fails unless `describe_type` says `want` of the view's keeping, and
/// `not asked` of its twin's.
async fn says(l: &Lake, view: &str, want: &str) -> Held {
    let said = |kept: &Option<Kept>| match kept {
        Some(Kept::Kept) => "kept",
        Some(Kept::Computed { why }) => match why {
            NotKept::NotAsked => "not asked",
            NotKept::CannotBeKept { .. } => "cannot be kept",
            NotKept::NotWhole { .. } => "not whole",
            NotKept::NotKeptHere => "not kept here",
            NotKept::OnlySome => "only some",
        },
        None => "nothing",
    };
    let kept = l.describe_type(view).await?.lifecycle.kept;
    let twin = l.describe_type(&format!("{view}_twin")).await?;
    let twin = twin.lifecycle.kept;
    ensure!(
        said(&kept) == want && said(&twin) == "not asked",
        "`describe_type` must say `{want}` of `{view}` and `not asked` of its twin; it says \
         {kept:?} and {twin:?}"
    );
    Ok(())
}

/// What `who` is answered by the view, or by its twin (`end`), door by
/// door, with the twin's names read as the view's: `find`; and, `deep`,
/// a count and `find` by a value of its first group field, every page by
/// its last measure, the owner writing a setting before each page a
/// cursor continues, a page of two past an `offset`, `find` by that
/// measure below 100, an aggregate of it, `get` of a key and of none, a
/// pattern started on that group, and a walk to it from its source's
/// records over its derived relation; and, where a `sort_index.` setting
/// names it, its pages in every order ([`paged`]).
async fn said(
    cat: &dyn Catalog,
    (who, own): (&Principal, &Principal),
    p: &Pair,
    end: &str,
    at: ReadAt,
    deep: bool,
) -> Result<Vec<(String, String)>, Failure> {
    let name = format!("{}{end}", p.name);
    let ty = TypeName::from(name.as_str());
    let narrow = p.by.as_ref().zip(p.seen.as_ref());
    let narrow = narrow.map(|(field, value)| format!("{field} = '{value}'"));
    let narrow = || narrow.as_deref().map(Into::into);
    let last = p.measures.last().map(String::as_str).unwrap_or_default();
    let folds = [AggFunc::Min, AggFunc::Max, AggFunc::Sum, AggFunc::Avg];
    let folds = folds.map(|f| agg(&format!("{f:?}"), f, Some(last)));
    let mut finds = vec![("`find`", Find::default()); 7];
    finds[1] = ("a count", aggregate(vec![agg("n", AggFunc::Count, None)]));
    finds[1].1.filter = narrow();
    finds[2].0 = "a page by its last measure";
    finds[2].1.order_by = vec![order(last, OrderBy::Value, Direction::Desc)];
    (finds[2].1.limit, finds[2].1.total) = (Some(1), true);
    (finds[3].0, finds[3].1.filter) = ("`find` by its group", narrow());
    finds[4] = ("an aggregate", aggregate(folds.to_vec()));
    finds[5] = ("a page past an offset", finds[2].1.clone());
    (finds[5].1.limit, finds[5].1.offset) = (Some(2), Some(1));
    finds[6].0 = "`find` by its last measure";
    finds[6].1.filter = Some(lakelands_api::expr::Condition::Text(format!(
        "{last} < 100"
    )));
    let mut out = Vec::new();
    for (door, mut find) in finds.into_iter().take(if deep { 7 } else { 1 }) {
        (find.at, find.offset) = (at, find.offset.filter(|_| find.cursor.is_none()));
        let mut door = door.to_owned();
        for _ in 0..32 {
            let reply = cat.find(who, &ty, find.clone()).await;
            let next = match reply.as_ref().map(|r| &r.data) {
                Ok(Found::Records { next_cursor, .. }) => next_cursor.clone(),
                _ => None,
            };
            out.push((
                std::mem::replace(&mut door, "its next page".into()),
                told(reply),
            ));
            let Some(next) = next.filter(|_| find.limit == Some(1)) else {
                break;
            };
            (find.cursor, find.offset) = (Some(next), None);
            let setting = json!({"key": "twin.walk", "value": out.len()});
            let (ty, key) = (SETTING_TYPE.into(), "twin.walk".into());
            let wrote = cat.upsert(own, &ty, &key, patch(setting), WriteOpts::default());
            wrote.await.must("the owner writing `twin.walk`")?;
        }
    }
    let keys = p.keys.iter().take(1).map(String::as_str);
    let keys = keys.chain(["no such group"]);
    for key in keys.take(if deep { 2 } else { 0 }) {
        let mut get = get_at(key, None);
        (get.at, get.with_meta) = (at, false);
        let got = cat.get(who, &ty, get).await;
        out.push((format!("`get` of `{key}`"), told(got)));
    }
    let source = format!("{}{}", p.source, if p.over { end } else { "" });
    let rel = format!("of_{name}");
    let rows = || Projection::Rows { fields: Vec::new() };
    let mut start = pattern(&name, None, Vec::new(), Projection::Graph);
    start.start.filter = narrow();
    let mut back = pattern(&source, None, vec![step(&rel, Walk::In)], rows());
    back.start.filter = narrow();
    let mut patterns = vec![("a pattern's start", start)];
    patterns.extend(p.by.iter().map(|_| ("a walk to it", back.clone())));
    for (door, mut asked) in patterns.into_iter().filter(|_| deep) {
        asked.at = at;
        out.push((door.to_owned(), told(cat.query(who, asked).await)));
    }
    if p.indexed {
        out.extend(paged(cat, who, &ty, p, at).await);
    }
    let read = |(door, answer): (String, String)| (door, answer.replace("_twin", ""));
    Ok(out.into_iter().map(read).collect())
}

/// Every page `who` is answered by `ty`, the view or its twin, two
/// records a page, each order continued by its cursor to its last page:
/// by its first group field and by each measure, both ways, and by its
/// last measure among the records holding less than 100 there.
async fn paged(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &TypeName,
    p: &Pair,
    at: ReadAt,
) -> Vec<(String, String)> {
    let both = |f| [(f, Direction::Asc, None), (f, Direction::Desc, None)];
    let below = p
        .measures
        .last()
        .map(|m| (m, Direction::Asc, Some(format!("{m} < 100"))));
    let mut out = Vec::new();
    for (field, dir, filter) in p.by.iter().chain(&p.measures).flat_map(both).chain(below) {
        let mut find = Find {
            order_by: vec![order(field, OrderBy::Value, dir)],
            limit: Some(2),
            filter: filter.clone().map(lakelands_api::expr::Condition::Text),
            at,
            ..Find::default()
        };
        for n in 0..32 {
            let reply = cat.find(who, ty, find.clone()).await;
            find.cursor = match reply.as_ref().map(|r| &r.data) {
                Ok(Found::Records { next_cursor, .. }) => next_cursor.clone(),
                _ => None,
            };
            let door = format!("pages by `{field}` {dir:?} {filter:?}, page {n}");
            out.push((door, told(reply)));
            if find.cursor.is_none() {
                break;
            }
        }
    }
    out
}

/// An answer as a text, or its refusal by its wire form (its code, places
/// and parameters and its sentence). A cursor is read as whether there is
/// one: a view's and its twin's need not be the same text.
fn told<T: serde::Serialize>(answer: lakelands_api::Result<Reply<T>>) -> String {
    let mut told = match answer {
        Ok(reply) => json!(reply.data),
        Err(e) => return format!("refused: {}", json!(e)),
    };
    told["next_cursor"] = json!(told.get("next_cursor").is_some_and(|c| !c.is_null()));
    told.to_string()
}
