# 087 — An ingest may carry a delivery per record, each sealed apart under a key of its own

## What is decided

Decided on lakelands#1528 by the operator (2026-10-28), for 0.21.0. It needs no log format of its
own and no step: it is written in whatever format 0.21.0 writes. It
extends `docs/decisions/016-two-write-doors-by-origin.md`, whose stream
carries a delivery per record as it carries a source per record;
`docs/decisions/065-part-of-a-source-may-be-taken-back.md`, whose ingest
sealed apart makes one key per source and delivery, not per source, and
asks for a window's keys in one request; and
`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`,
whose quarantined record is keyed by its record's delivery and its place
there. Each comes in force with the build, which rewrites those records
where they stand.

A **call** is one `ingest`; a **delivery** is what a host may take back
as one (065). Until now the two were one. From here a call carries one
or more deliveries, and the call's options, `commit_window`,
`sealed_apart` and `quarantine_refused`, stay the call's.

### A record may name its own delivery, else it is in the call's

`SourceRecord.delivery: Option<DeliveryId>`, serde default and skipped
when `None`, beside `source` (lakelands#1041). A record that names one
is in that delivery; one that names none is in the call's: the caller's
`IngestOpts.delivery`, else the store's own, `d` and the position of the
call's first event, as before. Each `record_ingested` the record makes
carries its record's delivery in its envelope, and so does each
`record_quarantined` (085). A record that names the call's own id is in
the call's delivery; there is no second one of that name.

**Its form is the caller's.** A record's delivery is judged as the
call's is: not empty, no NUL and not of the store's form, else refused
`invalid` (`delivery_id_form`) as the record's, with `index`, `from` and
`record`, in 016's order beside the record's own source form. It is not
quarantined: the delivery keys the quarantine, as the source does.

**`Selection::Delivery` and `Selection::Deliveries` select what they
did**, as if each delivery had been its own call: they read the event's
envelope, which now says the record's delivery. Nothing of 065's act
changes.

**The log format does not change.** Every `record_ingested` has carried
its own `delivery` since format 17 and its own `sealed.apart` since
format 18 (`docs/log-format.md` §1.1, §1.4). The format's only notion of
an append is a group of consecutive events written together; it names
no delivery for the group, no reader refuses an append whose events
name different deliveries or different keys apart, and no vector's
verifier checks that they share one. 083's frame holds only what all
events of an append share, so in an append of mixed deliveries each
line holds its own `delivery` and `sealed.apart`, as each line of the
per-item road does today. What changes in `docs/log-format.md` is the
writer's prose: the delivery is "the id the record or its call named",
and an ingest sealed apart is given "a key per source and delivery".
A vector of one append of two deliveries, each sealed apart, is owed
to `docs/log-format/vectors/21/` (lakelands#1534): the verifiers of 21
cannot read one as they stand, since each line they read is named in
both and the framed append is one fixed file, so it comes with readers
of its own. Corrected at the build (lakelands#1528), which said the
verifier would read it as it stands.

### A delivery sealed apart has its own key, also where it shares a call and a source

Under `sealed_apart`, the call keeps one key per **source and delivery**
it writes for, made before the first window that seals a value of that
pair and kept for its every window. Two records of one source in one
call, each its own delivery, seal under two keys, so taking one back
destroys its key alone and the other's values stay readable. A delivery
that spans windows of one call keeps one key; a later call under the
same delivery id gets a key of its own, as before (065). An act that
took the pair's key since makes the call take a new one for what it
writes after, as before.

The act needs no change: it destroys every apart key whose values it
all selects, and a key now seals the values of one delivery only.

**Not sealed apart per record.** `sealed_apart` stays the call's. A
host that wants some deliveries apart and others not makes two calls; a
per-record option would let one window need a key store for some
records and not others, and its refusal `unbound` would then depend on
which record came first.

### A window asks for its new keys apart in one request, and has them before it is appended

`KeyStore::create_apart_many(&self, lake, sources: &[SourceId]) ->
Result<Vec<KeyId>>`: one new key apart for each entry, in order, a
source named once per key it needs. Its default loops over
`create_apart`, so every key store compiles and answers as before; a key
store that keeps its keys durably implements it to keep them all with
one sync. Like `create_apart`, it answers only once its keys are kept as
durably as it keeps any key: the doc of both now says so.

The store's sealing (`Keys::apart_many`) asks it once per window,
before the window is judged: for every pair of source and delivery of
the window's records that would seal a value, whose key the call does
not keep yet. A record would seal where its type is shown to the writer,
its key and its source are well formed, its source is not forgotten
(058), and it carries a non-null value of an erasable field. The keys
are therefore durable before the append that seals under them, as one
key is today.

**The request is a saving, and no answer turns on it.** The window is
then judged as now. Where the request fails, or the scan missed a pair,
the record that needs a key asks for it alone, as today, and a refusal
(`unbound`, `key_missing`) names that record with its `index` and
`from`, and the window appends nothing. A key the request made for a
record then refused or counted a duplicate seals nothing and is
destroyed with the source, as a key made for a call that appended
nothing is today (065).

**On the key store, not only in the store.** The store can gather the
keys a window needs, but only the key store can keep many with one
sync; asking `create_apart` many times in a row costs what it costs now.
**Not one request a source**, as the issue sketched it
(`create_apart_many(lake, source, count)`): a window of many sources
(lakelands#1041) would pay a sync per source. **Not a key store told to
flush before the append**: two calls whose order the store would have to
keep, and a key store that answered a key before it was kept could lose
one whose values the log holds, and the fold fails closed on a lost key
(065). One request, durable when it answers, is one promise.

### A window of mixed deliveries is decided as any window is

- **Whole or nothing.** The delivery is a member of each event's
  envelope and nothing is judged over it; the window appends all its
  events in one append or none, its keys made before.
- **`Error::Ingested { index, from }`** keeps its meaning: places in the
  call's stream, whatever deliveries the records name.
- **A duplicate stays with its first delivery.** The delivery is not in
  the ingest id (`docs/log-format.md` §3). The same record in a later
  delivery, in the same call or another, changes nothing and counts as
  a duplicate; under `sealed_apart`, whose box differs by key, the
  unchanged test finds it, as for a redelivery today (065). Taking back
  the later delivery takes nothing of it.

### `Committed` is unchanged

One `Committed` a window: `position`, `records`, `duplicates`,
`quarantined` (085) and `delivery`, the call's, answered as before:
where the caller names none and every record names its own, the
store's `d` and position names no event, and is answered all the same. A
window's records are the next `records + duplicates + quarantined.len()`
of the stream, so a host acknowledges every delivery whose last record
the window held when the window answers; a window commits all of them or
none. **A list of deliveries per window** was weighed and not taken: a
window of 200 one-item deliveries would answer 200 ids the host already
holds, and it acknowledges by its own stream place, not by the lake's
echo, as lakelands#1041 found for sources.

### A quarantined record is keyed by its record's delivery and its place there

085's id is `(source, delivery, index)`. The delivery is now the
record's (above), and the index is the record's place **among the
call's records of that delivery**, from 0. Where a call carries one
delivery that is its stream index, so every call 085 was written for
keys as 085 says. Where a delivery is one item, its record is at 0
whatever page carried it, so a host that runs the item again in another
page finds its quarantined record, as 085 promises a host that runs its
file again. `Committed.quarantined` and `Error::Ingested` keep the
stream index. A quarantined cell is sealed apart under its delivery's
key.

### The doors carry the member; nothing else changes

- **Rust.** `SourceRecord` gains a public field, an addition under 056.
  A host's `SourceRecord { … }` literal adds `delivery: None` (beside
  0.21.0's `resubmits: None`); one built by serde reads as before.
  `KeyStore` gains a provided method, so a host's key store compiles
  unchanged and pays a sync per key until it implements it. Nothing is
  breaking; the `host-surface` job lets both through.
- **WIT:** `source-record` gains `delivery: option<delivery-id>`; the
  doc of `ingest` in `store.wit`, "the records of one delivery", says a
  record may name its own.
- **REST:** an NDJSON line may carry `"delivery"`; `?delivery=` is the
  default for a line that names none. The server reads lines as
  `SourceRecord`, and the client sends the member as it stands.
- **MCP:** no tool ingests; nothing changes.
- **CLI:** `lakelands ingest` reads CSV columns and keeps one delivery a
  call.
- **Release notes**, under "Changes a host must make": "`SourceRecord`
  gains `delivery`, the delivery the record is in where it is not the
  call's: a struct literal adds `delivery: None`. A key store that keeps
  keys durably may implement `KeyStore::create_apart_many`, so that an
  ingest sealed apart pays one sync for a window's keys."

### What it takes to build

Estimated from the nearest shipped sibling, a source per record
(lakelands#1041: +7 product and +52 specification in the core crates,
one check of four rows, 34 literals), and the work this adds to it: a
key per pair, the request and its fallback, and the quarantine's place.

- **Product (core crates), est.:** +100 to +150. `SourceRecord.delivery`
  and the docs of `IngestOpts`, `Committed` and `create_apart` /
  `create_apart_many` in `lakelands-api`; in `lakelands-store`, the
  stamp per record, the delivery's form, the call's keys by pair, the
  scan and the request with its fallback (`ingest.rs`),
  `Keys::apart_many` (`sealing.rs`), and the place per delivery for the
  quarantine. Seam width does not move: `seam.rs` is not counted.
- **Specification (core crates), est.:** +300 to +400: the four checks
  below, the counting key store, and one line in each of about 20
  literals in the suite and the tests.
- **Outside the core:** the file keyring implements
  `create_apart_many` with one write (about +20, and a test); about 40
  literals elsewhere; WIT +3; the measurement; docs about +60 (016, 065,
  085, `docs/log-format.md` §1.1, §1.4 prose, `docs/rest.md`,
  `docs/conformance.md`).

The checks, on every engine:
1. `ingest::A_DELIVERY_PER_RECORD`: one call in `page` carries `k0`
   (`page`), `k1` (none), `k2` (`item-2`) and `k3`, `k4` (`item-3`);
   their events name `page`, `page`, `item-2`, `item-3`, `item-3`, alike
   at a `commit_window` of 1 and of 256; `remove_values` of `item-3`
   takes `k3` and `k4` alone, of `[item-2, item-3]` three, of `page`
   two, and the act on `item-3` nothing of `page`; the same record
   under `item-5` then `item-6` in one call is one event in `item-5` and
   a duplicate; a record naming `d7` or the empty id is refused
   `delivery_id_form` with its `index` and `from`, nothing of its window
   appended.
2. `sealed_apart::APART_PER_DELIVERY`: sealed apart, one source, `a` and
   `b` in `item-1`, `c` in `item-2`, `d` in the call's: three keys,
   `item-1`'s shared by `a` and `b` across two windows; taking `item-1`
   back with `require_shredded` destroys its key alone, `c` and `d` read,
   after a reopen and a rebuild too.
3. `sealed_apart::APART_ASKED_ONCE`: a counting key store; six one-record
   deliveries over two sources at a `commit_window` of 6 make one
   request of six keys and no other; at 2, three requests; a window
   whose keys are all kept makes none; a store whose request fails and
   whose `create_apart` refuses `unbound` at the record at index 3
   refuses that record with `from` 2, nothing of its window appended.
4. `quarantine::QUARANTINED_BY_DELIVERY`: in `page`, the record
   of `item-4` at stream index 7 is quarantined as `(s, item-4, 0)` and
   `Committed.quarantined` names 7; `item-4` run again at index 2 of
   another call and refused alike is a duplicate of that id.

**Measured, as the build's evidence:** `ingest_window` (the CLI's
example) gains a mode of N one-record deliveries sealed apart under the
file keyring on SQLite, one call an item against calls of 200 with a
delivery per record. It prints ms per item, the appends (one `Committed`
each) and the key store's requests, through a counting wrapper: one
append and one request per window of 200, against one of each per item.

## Why

A host whose delivery is one file, one list entry or one message must be
able to take back each alone, so today it writes one call per item.
Each call pays a durable append and, sealed apart, a durable key, two
syncs whatever its size. Measured through a host's write door on SQLite
(lakelands#1528), 300 one-record items sealed apart cost 30–49 ms an
item one call each, and the same records in calls of 200, one delivery
a call, 1.4–2.5 ms: a list of 50 000 entries reads in about 40 minutes
against about 2. Grouping was the only way to share those syncs, and it
cost the per-item take-back.

A delivery per record lets the grouping and the take-back meet. It costs
no more keys than the per-item road makes, and no more bytes of log
than it writes, since each line already holds its own delivery and key
there; it saves the syncs, one append and one key request a window
instead of one each an item.

The source moved onto the record on the same reasoning (lakelands#1041): each
event already carries what selects it, and the call only grouped them.

## What it is not

- **Not a format step.** No event, member or id changes; a lake of 20
  or 21 reads the same, and a reader of either reads what this writes.
- **Not a delivery in the ingest id.** A redelivery under another id
  stays a duplicate of the first.
- **Not a second door.** `ingest` of `(DeliveryId, Vec<SourceRecord>)`
  pairs would be two doors for one thing, as lakelands#1041 found for sources.
- **Not fewer keys.** A delivery sealed apart still has a key of its
  own in the key store; the file keyring, which is for one machine and
  for tests, still writes its file whole at each request, so a host
  with many thousand deliveries a day binds its own key store.
- **Not a saving for a record that changes nothing.** A redelivered
  record still makes a key that seals nothing, before the duplicate test
  finds it, as today; skipping it would move a refusal in 016's order.
- **Not a change of `Committed`, rights or the act.** No grant names a
  delivery, and `remove_values` reads what it read.
