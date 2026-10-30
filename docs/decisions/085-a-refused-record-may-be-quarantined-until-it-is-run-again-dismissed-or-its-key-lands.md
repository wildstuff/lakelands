# 085 — A refused record may be quarantined until it is run again, dismissed or its key lands

## What is decided

Decided on lakelands#1502 by the operator (2026-10-27), for 0.21.0, in
format 21, with the budget raise its build measures up to the sizes
below. It extends
`docs/decisions/016-two-write-doors-by-origin.md`, whose stream ends at
a refusal unless the delivery asks to quarantine it, and whose
redelivery appends nothing but where a resubmission closes a
quarantined record;
`docs/decisions/058-a-source-may-be-forgotten.md`,
`docs/decisions/065-part-of-a-source-may-be-taken-back.md` and
`docs/decisions/039-a-source-is-retracted-as-one.md`, whose acts reach
a source's quarantined records as they reach its landed ones; and
`docs/decisions/006-grants-not-roles.md`, whose sight a quarantined
record is shown by. Each comes in force with the issue that builds it,
and the build rewrites those records where they stand.

A **quarantined record** is the word for it, in the docs and in the API
(`quarantine_refused`, `record_quarantined`, `quarantined`). The record
never says "held" for it: 006 calls a restricted record a held record,
and its conformance checks and the budgets' rows use that word so.

### A delivery may ask that a record it is refused be quarantined, and its stream goes on

`IngestOpts::quarantine_refused`, for one call, off by default. Without
it nothing changes: a refused record ends the stream, as 016 says. With
it, a record refused by a rule of the record is quarantined, and the
stream goes on.

**Which refusals.** Every refusal of 016's ingest order is one or the
other; a refusal a later record adds to that order says which in the
same change.

| What is judged, in 016's order | Refused | Quarantined |
|---|---|---|
| the lake, the context, the `ingest` grant | `not_granted`, `too_deep` | no: the call's, not the record's |
| the call's `commit_window` | `limit_range` | no: the call's |
| a source record with a member it does not have | refused as it is read | no: never read whole |
| the source id's form | `invalid` (`source_id_form`) | no: the source keys the quarantine |
| the form of a delivery the record names (087) | `invalid` (`delivery_id_form`) | no: the delivery keys the quarantine |
| the source forgotten, its key store not bound, a key missing | refused, `unbound`, `key_missing` | no: the source's |
| the type is declared and shown to the writer | `not_found` | no: no type rules its cells |
| the key's form: empty | `invalid` | yes |
| the key's form: holding NUL | `invalid` | no: below |
| the door and the grants, field by field (record check 4): a field not declared, another authority's, only verbs', or not granted | `invalid`, `wrong_door`, `verbs_only`, `not_granted` | yes |
| every value fits (5) | `constraint` | yes |
| a record of a link type no relation carries | `invalid` | yes |
| an empty record at a key not live (2) | `invalid` | yes |
| the key fields filled from the key: a key that does not split, a filled field not granted or not fitting (6) | `invalid`, `not_granted`, `constraint` | yes |
| the key and the key fields agree (7) | `invalid` | yes |
| every reference names a live record (8) | `constraint` | yes |
| every required field holds a value (10) | `constraint` | yes |
| a `sys.` record is well formed (11) | `invalid` | yes, though no ingest reaches it: every field of a `sys.` type is an author's, so a source's record is refused `wrong_door` first |
| the record is the writer's (12): another's at the key, or not admitted, to a holder of `only`; a restricted record at the key, to a writer without `restricted` | `not_granted` | yes, and shown by sight (below) |
| a type written once | `written_once` | yes |
| a link's cardinality | `linked_already_from`, `linked_already_to` | yes |
| an outage | `unavailable` | no |
| the quarantine is full (below) | the record's own refusal, `quarantine_full` beside it | no |

**A key holding NUL** is not quarantined. Every id the log makes joins
its parts with `0x00` and is unambiguous because no key holds one, and
PostgreSQL's text cannot store one; a record so keyed could never land
under that key either. **An empty key** is quarantined: it is logged and
stored as the empty string, which neither rule refuses, and closes only
by a resubmission or a dismiss.

**The window** is decided without the quarantined record, as the host
would resume at `from` without it. Every record after it is judged over
the lake and the records before it in the stream, the quarantined one
left out, so the answer is the same wherever the store cuts. The
window's append carries its written records and a `record_quarantined`
for each quarantined one, and its `Committed` lists the stream `index`
of each in `quarantined`. To a writer whose record was refused at a
restricted key the index tells what the refusal would have told, and
no more.

**The refusal** goes into the quarantined record and not into the
stream: the one the writer would have been answered, kind, reason and
names.

### A quarantined record is keyed by its source, its first delivery and its place in that delivery

Its id is `(source, delivery, index)`: the record's source, the delivery
it was in when first quarantined (the one the record names, else its
call's: the caller's, or the store's own, 016) and the record's place
among the call's records of that delivery, from 0 (087). Where a call
carries one delivery that is the `index` 016 already answers in a
refusal; where a delivery is one item, its record is at 0 whatever
call carried it. That is the file or feed item and the row a host can
name without the lake. It is never re-keyed: a resubmission keeps the
id. `Committed.quarantined` and a refusal keep the stream `index`.

**Versions.** Each `record_quarantined` is one version of its id, the
first 1. A record refused under an id already quarantined, by a
redelivery under the same delivery id or by a resubmission refused
again, is judged against the id's last version:
- refused with the same cells and the same refusal, it is a duplicate:
  it appends nothing and counts in `duplicates`, whether the id is
  open or closed, so a host that dismissed a row and runs its file
  again is not shown the row again;
- refused otherwise, it is the next version, and opens the id again
  if it was closed: the lake has been told something it was not when
  the id closed;
- where the last version's cells are erased (below), nothing can be
  compared, and it is the next version.

A host that wants a redelivery to find its quarantined records names
the delivery; without one, each call is its own.

### A host's run and line key a quarantined record where it names them

A host that reads files knows each row by the run that read it and the
row's line, and can name that pair before it asks the lake anything.
`IngestOpts::run`, of a delivery id's form, takes the delivery's place
in the id of every record the call quarantines, and
`SourceRecord::line`, at most 2^53 − 1, takes the record's place; each
where given. The same pair is the same quarantined record, whichever
door quarantined it and in whichever call: sent again it is judged
against the id's last version as above. The envelope's delivery is the
record's still, so taking back a delivery reaches its versions as
before; the id names the run. A resubmission keeps the id it names.
A place counts only its call's records, so two calls of one delivery
both have a record at 0: a host that sends one delivery in many calls
names each record's line. Under a run, a place counts the call's
records of every delivery, since the run keys them all: two records of
two deliveries are two rows of the run, not both its row 0.

### The host may quarantine a record it refused itself

A host refuses rows before the lake sees them: a column it cannot map,
a file it cannot read past. Kept beside the lake, their cells escape
the type's protection as above. `Catalog::quarantine(who, source, opts,
refused)` quarantines one under the source with `opts`' delivery, seal
and run, as an ingest's quarantined record: its row as the host would
have sent it, its type and key, its cells, and the host's refusal, a
stable cause code of 1 to 64 bytes of `a`–`z`, `0`–`9` and `_` and a
sentence for the operator, which names no cell's value. The record is
not ingested and no rule of its type judges it; its refusal is
`declined`, case `refused_by_host`, with the cause and the sentence. A
cause or sentence not of that form is `invalid`, `host_refusal_form`.
A call of the door holds one record, at place 0 always, so under a
named run or delivery its place would make every row of the run one
record, each the last's next version: there the record names its
line, else it is `invalid`, `host_line_missing`, at member `line`.
With neither named it is keyed under the store's own delivery, its
call's alone, as an ingest's record with none is.

What keeps an ingest's refusal from quarantine keeps the host's (the
table above, marked no): the call, the `ingest` grant, the source and
the delivery or run's form, a type not shown, a source forgotten or
its key store not bound, a key holding NUL, the bound; each refuses the
call in the wrapper 016 gives a refused record, at index 0, and appends
nothing. Its cells are sealed, shown and closed as any quarantined
record's: listed, dismissed, closed `landed` by a record landing at its
key and `resubmitted` by a resubmission, and reached by the acts on its
source. The answer is its id, the position of its version, and whether
it was a duplicate. The door is the host's, worked through the host
like the listing; it stays off the guest door.

Two lists, one of refusals the lake made and one the host made, were
weighed and not taken: a person works one queue, a run's rows are
found under one pair whoever refused them, and a second store would
need its own sealing, sight and erasure.

### A quarantined record keeps its cells as given, sealed as its landed values would be

**The cells** are the record's members as it sent them, each value in
its canonical form, never coerced to the field or judged against it:
the refusal may be that it does not fit. A host that maps a file's
columns hands its cells as text, and reads them back as text. The type,
the key as sent, `seen_at` and `upstream` are kept beside them.

**Each version is an event of its own call.** Its envelope names its
record's delivery in that call, which is the id's only for the first
version: a
resubmission under delivery D2 of a record first quarantined under D1
writes a version whose envelope names D2 and whose id names D1.

**Sealing.** A cell of a field the type declares `erasable` is sealed
under the source's key, and under the apart key of its record's
delivery in the version's own call where that call is sealed apart
(065, 087), in one box, as a
`record_ingested`'s members are (083); every other cell is plaintext,
as a landed value of that field would be. The box's nonce is of its own
domain: the source, the word `quarantined`, the id's delivery and index
and the version before the event, so no record's box shares one, and
the same cells quarantined again at the same version seal to the same
bytes. A cell to seal with no key store bound is refused `unbound`, and
the stream ends, as an erasable value of a landed record ends it.

**Sealing every cell**, as if every field were erasable, was weighed
and not taken. It would need a key store for any quarantined record,
and would keep a refused cell better than the record keeps it once it
lands: the protection a host asked for is the type's, not more.

### `sensitive` and `erasable` apply to a cell as to the field it names

A cell is read as the field it names: followed through every rename
since, with the access the reader's grants give that field and
`sensitive`'s raise, as 006 states them. To a reader of the field the
cell is shown; to one to whom the field is masked, the cell is named
in `masked` and its value withheld; to one to whom the field is `none`,
the cell is not there, nor named. A cell that names no field of the
type, or a field a redeclaration has since removed, is read by the
type's default access, as a field no grant names; one whose field was
`sensitive` when it was quarantined keeps the raise, so a redeclaration
never opens a cell the type withheld when it came. A cell sealed stays
sealed: `erasable` is never taken off a field (058).

**A hiding** (006, `hide_where`) lowers a field on a quarantined record
where it would lower it on either record the row touches, as a write is
refused a field hidden on its record before or after it:
- where the `G` cell as sent holds one of the hiding's values, the
  record the row would leave;
- where the live record at its type and key, at the head, is selected;
- and where the `G` cell is there but is not a text or reference value,
  since a value that does not fit `G` cannot be told not to select.

A row whose `G` cell is absent, at a key no live record holds, is not
selected. So a cell is never shown to a reader from whom a hiding on
either side keeps it.

**The refusal** is shown whole to a reader who is shown every name it
holds, and to any other as its kind alone, so that a refusal phrased to
its writer's sight tells no other reader what is hidden from it. Its
kind alone is its `error` and, for a refusal, its `reason` code, with
`more: true`, as a listed proposal says it holds more (006): no
`field`, no case, no place, no parameter and no record name; a
`not_found` is `not_found` alone, with no `missing`. The listing keeps
the one shape, `refusal`, either way, and `more` tells a reader which
it holds.

### A quarantined record is worked by a holder of `ingest`, under the sight of its type

`Catalog::quarantined(who, filter)` lists quarantined records by source,
type, delivery and state, open or closed, in id order, paged by the
lake's limits, and gets one by its id. `Catalog::dismiss_quarantined(who,
id)` closes one. Both need the `ingest` grant (`not_granted` otherwise).
A quarantined record is there to its reader as a proposal's write is
(006): not where the reader is not shown its type, not at a key whose
record is restricted to a reader without `restricted`, and not, to a
holder of `only`, where the record its cells would leave is not
admitted. One not there is answered `not_found`, as one that never was.

**A new grant** was weighed and not taken: a quarantined record is what
an ingest left, and who may feed the lake is who may see what it sent
back; each cell is already decided by the grants of its field, so a new
grant would decide nothing a field's grant does not. **The type's
reading grants alone** were not taken either: they would show every
reader of a type, an agent among them, rows the type refused, beside
its records, as if they were its values.

### A resubmission is a normal ingest of the same source that names what it resubmits

A host runs a quarantined record again by ingesting the mended record
under the same source, with `resubmits: { delivery, index }` on its
`SourceRecord`. It is judged as any record of any ingest, by the
writer's grants, and lands, counts as a duplicate, or is refused. A
`resubmits` that names no open quarantined record of the record's
source there to its writer is refused `not_found`, the same for a
closed one as for a hidden one. Resubmitted under `quarantine_refused`
and refused again, it is judged against the id's last version, as
above.

The record names what it resubmits because the key may be what was
mended: a record refused for its key lands under another, and only the
id finds it.

### A quarantined record closes by an event, in the append of what closed it

A quarantined record closes, and each close is a `quarantine_closed`
event, naming the id and why, in the same append as the write or the act
that closed it:

- `resubmitted`: its resubmission was written, or counted as a
  duplicate.
- `landed`: a `record_upserted` or `record_ingested` at its type and
  key, appended after its last version by a caller's write door: an
  `upsert`, a `link`, a verb's call, an accepted proposal, an `enact`
  of a change to the records a condition selects (081), and an ingest
  of any source, a mirror's copy among them. The record events the
  store appends on its own close nothing, nor does any other kind: an
  aging move (036), a migration's fills and re-keys, a retirement, a
  restriction, an output, and the fold's own fills. None of them is
  anyone's answer to the row. A duplicate, which appends no record
  event, closes nothing either. An empty key, or one a re-keying
  migration has left in an old form, closes only otherwise.
- `dismissed`: its reader dismissed it. A repeat answers the first
  close's position and appends nothing.
- `forgotten`, `removed`, `purged`: its source was forgotten (058), the
  values of its last version were taken back (065), or its source, or
  its type of that source, was purged (039).

**A resubmission that is a duplicate appends.** 016 holds that a record
that changes nothing appends nothing. A resubmission that changes
nothing still closes what it names, so its window appends the
`quarantine_closed` and nothing else for it: the one case where a
duplicate appends, and only where the record names an open quarantined
record. A redelivery that names none appends nothing, as before.

A write that closes quarantined records looks for them by type and key
in the projection, which keeps the open ones indexed so; a lake that
holds none of a type looks for nothing.

The event is there so that the close is in the log and the feed, which
a host follows, and the projection answers it too: a closed quarantined
record is listed with its id, its close and why, and **no cell**. A
close only the fold inferred, with no event, would leave a host to
reason from a record event what a quarantined record it may not know of
did.

### What forgetting, taking back and purging do to a quarantined record

Each act closes the quarantined records it reaches in its own append,
and its preflight counts them per type.

- **Forgetting** reaches every version: its sealed cells read as erased
  in the log, as the source's landed values do, and no answer holds a
  cell of it.
- **Taking back** selects each version as it selects a
  `record_ingested`: by its position, the delivery its envelope names
  and its `seen_at`. A selected version's cells fold as erased, and are
  shredded where the act destroys the version or the apart key that
  sealed them. The quarantined record closes `removed` where its last
  version is selected; otherwise it stays open with that version, as a
  landed value a later delivery wrote again is kept (065). So in the
  example above, taking back D1 erases the first version, and the
  second, written under D2 and sealed under D2's keys, stays open;
  taking back D2 closes it.
- **A purge** closes every open quarantined record of the source, or of
  the source and the type it names.

A plaintext cell stays in the log, as a landed plaintext value of the
same field stays there after the act: the act reaches the log through
keys, and a quarantined record is not treated more harshly than a landed
one. A dismiss closes and erases nothing: the log keeps what was sent,
as it keeps every write, and a host that must erase a refused row's
cells forgets or takes back its source, as for a landed one.

### A quarantined record lives until it closes, and a lake bounds how many it keeps

No clock closes a quarantined record. It is kept so that a person
decides, and a limit that dismissed it on a date would decide in their
place, and quietly. **A time limit** was weighed and not taken: a host
that wants one lists the records older than its own limit and dismisses
them, and the log says who did.

What a lake bounds is how many it keeps open: `limits.quarantined_max`,
the open quarantined records of one source, 10 000 by default, the
host's to raise. An open one carries its cells in the projection, and a
feed gone wrong that no one works would otherwise fill the lake with
rows it refused, unattended.

**What the bound does not cover** is the closed ones. A closed
quarantined record keeps a row with no cell: its id, its last version,
its close and why, which the listing answers from and the next
version counts on. That row grows the projection without a bound of
its own, as a retired record's row does: each stands for at least two
events of the log, its last `record_quarantined` and its
`quarantine_closed`, and a source's closed rows never outnumber the
records it had quarantined. Only a
write, a dismiss or an act on the source frees room under the bound,
each an event someone appended, so a feed that goes wrong and is
dismissed row by row grows the lake by what its host chose to append,
as any host's writes do; a host that will not keep that history forgets
or purges the source. **Bounding the closed ones too** was weighed and
not taken: a source would be refused quarantine forever after it had
worked 10 000 rows, or its closed rows would need dropping, and a row
dismissed and run again would then be shown again.

**A refusal past the bound** is not quarantined. It ends the stream as
without the option: the stream's last item is the record's own
refusal, unchanged in kind, code and data, as without the option, in the wrapper 016 gives a refused record of a stream, which
carries `index`, `from` and `record` beside it and now
`quarantine_full: { max }` too, the bound that was full. A Place or a
case code was weighed and not taken: only `invalid`, `constraint`,
`precondition`, `declined` and `unanswered` say a case, and a
quarantinable `not_granted` or `not_found` says none, and a case put in
place of the record's own would hide why the record was refused. A host
that reads `quarantine_full` knows the record was eligible and the
bound was the cause; one that does not reads the refusal as before.
Nothing of the window is appended, as at any refusal that ends a
stream. A new version of an id already quarantined is not counted
against the bound, nor is an open id it reopens where the bound has
room. A record that lands at the key of an open quarantined record
frees its place at once, in its own window: a refusal after it in the
same window is judged against the bound as if the window had been cut
between them, so where the store cuts does not change the answer.

**A redeclared type** keeps its quarantined records, and no declaration
withdraws a type (027 withdraws triggers, verbs, sets and functions), so
no change of shape closes one. Its cells are read
under the shape at the head, through the type's and its fields' renames,
as above, and a resubmission is judged under it.

### Quarantining is asked per ingest, not set per source

The option is the delivery's, as `commit_window` is (016): only the
caller knows whether someone will work what it quarantines, and the
option changes the shape of the call's answer, a stream that ends at a
refusal or one that goes on. A **per-source setting** was weighed and
not taken: a caller would be answered by a setting it may not know of,
and a source would need a declared existence the lake does not give it.
The REST ingest takes `?quarantine_refused`, and a catalog that does not
carry the option to the store refuses it,
`quarantine_refused_not_carried`, rather than end a stream the caller
asked to go on.

### What it takes to build

Estimated from the two acts on a source that shipped, as measured on
develop in the core crates: forgetting a source (lakelands#1042),
+1 535 product and +1 525 specification lines, and taking part of a
source back, its redaction step (lakelands#1153), +851 and +849. A
quarantined record adds two event kinds, their fold and a projection
table on three engines, a change of the ingest's window, a sight of its
own, sealing in a new nonce domain, the closes in every caller's record
door and in three acts, and two catalog methods with their REST doors:

- **Product:** about +1 600 to +2 100 lines. `IngestOpts::quarantine_refused`,
  `SourceRecord::resubmits`, `Committed::quarantined`,
  `Error::Ingested`'s `quarantine_full`, the quarantined
  record, its filter and its id in `lakelands-api`; the cells' sight,
  hidings included, in `lakelands-core` (`rights.rs`, beside a
  proposal's); the window, versions, the closes, the acts' reach and
  the fold in `lakelands-store`; a table and two projection methods in
  each engine; `quarantined`, `dismiss_quarantined` and
  `?quarantine_refused` in the server and the client.
- **Specification:** about +1 800 to +2 400 lines, in the checks below.
- **Budgets:** the product has about 1 230 lines of room on develop
  today and the specification about 240, so the build cuts first and
  raises both by the rule of `docs/budgets.md`, with a measured row.
  Seam width rises by two, 91 to 93 of 110; `Catalog` is not counted.
- **Docs:** about +300 lines. 016's stream, its order and its
  duplicate, 058's, 065's and 039's acts, 006's sight and hidings,
  `docs/log-format.md` (two kinds in §2.3, the nonce domain in §1.4,
  the content id in §3, the step to 21 in §6), `docs/rest.md`,
  `docs/settings.md` (`limits.quarantined_max`), `docs/refusals.md`
  (the case `quarantine_refused_not_carried`, an `invalid`'s;
  `quarantine_full` is no case, it stands beside the refusal in 016's
  stream) and `docs/hosting.md`: name the delivery, and what a dismiss
  does not erase. `docs/log-format/vectors/21/`, the vectors of the new
  format, with a `record_quarantined`, a duplicate of it, a next
  version under another delivery, a sealed one, and each
  `quarantine_closed` reason the vectors' lake can reach.
- **The log:** format 21, 0.21.0's one step, shared with lakelands#1489
  (the principal a write was made for, on the envelope): one release
  steps the format once, and whichever of the two builds first takes
  the step and the other adds to it. `record_quarantined`:
  envelope `source` and the call's `delivery`; payload the type, the key
  as sent, the id, `version`, the cells, `sealed`, `seen_at`, `upstream`
  and the refusal; content-addressed like an ingest, its id made over
  the source, the id's delivery and index, the version before, the
  cells as stored and the refusal, so that a version refused the same
  way appends nothing. `quarantine_closed`: the id and why. A log of 20
  holds neither; no converter changes an event.
- **WIT:** in `types.wit`, `quarantine-refused` on `ingest-opts`,
  `resubmits` on `source-record`, `quarantined` (the stream indexes) on
  `committed`, `quarantined-max` on `limits`, and `quarantine-full` on
  `ingest-error` beside `index`, `from` and `record`. The case
  `quarantine_refused_not_carried` comes in by the case catalog, as
  every case does. The listing and the dismiss stay off the guest door
  in 0.21.0: they are a person's, worked through the host.

The checks that would pin it, on every engine:
1. A delivery of three records, the second refused, with
   `quarantine_refused`: two land, one is quarantined with its refusal,
   listed by source and type, and `Committed.quarantined` names index 1.
   Without the option the stream ends at index 1, as before. The same
   answer at a `commit_window` of 1 and of 256.
2. The table: one refusal of each row marked yes is quarantined, and
   one of each row marked no ends the stream as without the option,
   `quarantine_full` past `limits.quarantined_max` among them, its
   stream's last item the record's own refusal unchanged, a
   `not_granted` and a `constraint` each, with `quarantine_full` and the
   bound beside it, appending nothing; a resubmission refused again
   under a full bound is its next version; a dismiss frees a place
   under the bound, and so does a record landing at the key of an open
   one earlier in the same window, at a `commit_window` of 1 and of
   256. An empty key is quarantined and lists as the empty string; a
   key holding NUL ends the stream. Each yes row is quarantined with
   the refusal the stream ends with without the option; each no row
   ends the stream with the same refusal with the option and without,
   quarantining nothing.
3. Sight: a sensitive cell is masked to a reader without the field's
   right and shown with it; a `none` cell is not there; a quarantined
   record of a type not shown, or at a restricted key, is not listed
   without `restricted`; without `ingest` the listing is `not_granted`;
   a refusal naming a hidden field is shown as its kind alone, with
   `more`, and no field, case or name. A hiding
   lowers a cell where the `G` cell selects, where the live record
   selects though the `G` cell does not, and where the `G` cell is a
   number; it lowers nothing where the `G` cell is absent and no record
   is live at the key.
4. Sealing and acts: an erasable cell is sealed in the log and a
   verifier without the key sees no plaintext: an export of the lake,
   versions sealed apart among it, holds none; `forget_source` closes
   every quarantined record of the source and no cell of it is
   answered; `remove_values` by delivery, by time and by `seen_at`
   closes what it selects; a record first quarantined under D1 sealed
   apart, resubmitted under D2 and refused again, stays open with its
   D2 version when D1 is taken back and closes when D2 is; a purge of
   one type of the source closes that type's and a purge of the source
   the rest; the reports of the three acts, preflight and act, count
   the quarantined records each closes by type; a rebuild and an import
   answer the same quarantined records as the head.
5. Closing: a resubmission lands and closes `resubmitted`, under a
   mended key too; one that is a duplicate appends the close alone; an
   author's `upsert`, an `enact` and another source's ingest at the key
   close `landed`; an aging move and a duplicate at the key close
   nothing; a dismiss closes and its repeat appends nothing; a closed
   quarantined record is listed with no cell.
6. Versions and redeclaration: the same delivery sent again appends
   nothing for a row refused the same way, open or dismissed, and makes
   version 2 of one refused otherwise, reopening a dismissed one; a
   renamed field's cell is read under its new name; a renamed
   relation's quarantined link records are listed under its new link
   type, and a link sent to the link type it had is quarantined as
   `unclaimed_link_type`; a removed sensitive field's cell stays
   masked. (No declaration renames a type, so a type's own rename is
   not among them.)

## Why

A host that feeds a lake from files or feeds has rows the lake refuses,
and someone must look at them: run them again once the cause is
mended, or let them go. Until now the lake kept nothing of such a row
but its refusal, so the host kept the cells itself, and the cells are
the type's values: a copy beside the lake escapes the type's
`sensitive` and `erasable` and the source's key, and forgetting the
source does not reach it. Quarantining them in the lake puts them under
the rules that govern the values they would have become.

**In the log, not beside it.** The projection is the fold of the log
(001): a table beside the log would be a second truth that no rebuild
restores, no export carries and no verifier checks, and its erasure
would need a path of its own beside the keys 058 destroys. As events,
quarantined records are rebuilt, exported, imported, sealed and erased
by the machinery that already does so for landed values, at the price
of a format step.

## What it is not

- **Not a proposal.** A proposal is a write its type accepts, waiting
  for a reviewer; a quarantined record is one its type refused. Nothing
  accepts it: it lands only as a new write that passes every rule.
- **Not a record.** No read, view, set, trigger, search, index, derived
  field or constraint sees a quarantined record, and it is answered only
  at the head, with no `as_of`: its history is in the log and the feed.
- **Not a repair.** The lake calls no model and guesses no value; it
  keeps what was sent and what it said, and a person or the host's own
  code mends it.
- **Not a query language.** The listing filters by source, type,
  delivery and state, and nothing else.
- **Not a restriction** (006's `restricted`), and not an erasure: a
  dismiss closes, and only the acts on a source erase.
- **Not quarantined** are the refusals the table marks no: those of the
  call, of the source, of a type the lake does not declare, and of a key
  the log cannot hold.

## How it came to be

- Decided and built in 0.21.0 (lakelands#1502).
- A quarantined record is keyed by its record's delivery and its place
  among the call's records of that delivery, where an ingest carries a
  delivery per record (087, lakelands#1528).
- The host's door, and the run and line that key a record
  (lakelands#1596). Each writes only from what a caller newly names; the
  stored shape is unchanged, and the log stays at format 22.
