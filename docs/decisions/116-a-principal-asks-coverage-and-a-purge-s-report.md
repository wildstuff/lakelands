# 116 — A principal asks a type's vector coverage and a purge's report

Two answers of the lake were the host's alone, with no principal:
`LakeStore::vector_coverage(ty)` counted every live record of the type,
and `LakeStore::purge_preflight(source, ty)` named every record and
type a purge would take, whoever asked. A host that shows either answer
to a person had to keep its own read and write checks in front of
them, and the lake's grants were never asked. A closed type, an
owner-scoped type and a field the person may not read reached the
person through a count or a report. So this record adds the two
questions to the caller's contract, asked as a principal.

## What is decided

### Two generic doors on the caller's contract, and no tool

**`Catalog` gains `vector_coverage` and `purge_preflight`, each asked by
a principal, and each appends nothing.**

- `async fn vector_coverage(&self, who: &Principal, ty: &TypeName) ->
  Result<Reply<Vec<VectorCoverage>>>` is a read: it is answered at the
  head, as `find` is, under the position it was read at.
- `async fn purge_preflight(&self, who: &Principal, source: &SourceId,
  ty: Option<&TypeName>) -> Result<PurgeReport>` is a write's
  question: it is asked through the context the `Catalog` was opened
  with, which is checked as a write's is, and the principal is resolved
  as at any write door.
- Both have default bodies that answer
  `Error::unbound(Seam::Operation { .. })`, as `judge`'s does (113). An
  implementor that does not count or purge compiles and says so. The
  crate's trait gains them without breaking an implementor. The WIT
  `catalog` resource gains both functions and the
  `vector-coverage-reply` record (034). The package version moves at
  the next release, as 034 says.
- Neither has a tool. Coverage is an operator's gauge of the embedding
  backlog, and a purge is a source's take-back, which a host decides.
  Neither is a model's question. `vector_coverage` and `purge_preflight`
  join the doors of the `Catalog` with no tool (014, amended).
- The `LakeStore` forms stay, unchanged, as the host's own lane. Their
  docs point to the principal's door.

### Coverage counts what the principal is answered

- A type the principal is not shown is refused as a read of it is:
  `not_found`.
- Only the embedding fields the principal reads are counted. A type
  whose embedding fields are all closed to it is answered an empty
  list, not a refusal, as a `find` of it is answered.
- A principal answered every record of the type, with no row narrowing,
  is counted as the host is: the vector index's count and the walk of
  what is due.
- A principal whose rows are narrowed is counted over the records a read
  answers it, and those records only. Every live record is walked, the
  ones it is not answered are skipped, and a record it is answered is
  counted embedded where its field holds a vector and due where it
  holds text to embed but no vector. The counts then match what that
  principal's search reaches.

### A purge's report is refused where the purge would be

- A type named that the principal is not shown is refused `not_found`.
  The purge selects as the host's does.
- Every record the purge would take is judged as the principal's
  retire of it at the authored door, by the same store predicate that
  door's retire runs: the type is shown and is not a view, its grant's
  `writes_only` can be applied to the type, it may retire the type, the
  authored door admits the type's retirement (a type only verbs retire
  is refused), it is told every setting the retire would name, the
  record is one it is answered, and it may write the record as it
  stands. One predicate keeps the report from drifting from the act it
  describes. A record that
  fails any of these refuses the whole report `not_granted`, naming no
  field and no record. A report that would reveal one record the
  principal may not touch is not given.
- An answered report names only what the principal is shown. References
  are kept for relations it is shown on types it is shown, and only
  from records it is answered. Moved values are kept for the fields it
  reads. Unsearched types and quarantined rows are kept for the types it
  is shown.
- The right comes before the rows. A type the purge names is judged
  as the principal's retire of a record of it before any record is
  counted, so a type it may not retire is refused though the purge
  would take nothing of it. Otherwise an empty report would tell one
  who may not touch the type the same as one who may, and whether a
  report is refused would depend on what the source holds at the
  moment, not on the principal's grant. A purge that names no type
  names none before its rows: it is judged by the records it would
  take, and one that would take none is answered with an empty
  report. Which types a source once brought in is not kept apart from
  its rows, and counting them would be the row read this rule is
  meant to come before.

### A value take-back's report is refused where the take-back would be

- `remove_values_preflight` is asked by a principal on the `Catalog`
  beside `purge_preflight`, on the same wire, and is judged the same
  way. Every type the selection reaches is judged first as the
  principal's write of it: it must be shown the type, the type must
  not be a view, its `writes_only` must apply, and it must be able to
  write every field the take-back would take. A take-back that keeps a
  record and only takes values from it asks no right to retire, so
  the type is not judged as a retire. Every record the take-back
  would retire is judged as the principal's retire of it. Every record it would keep but take values from is
  judged as the principal's write of the record without those values:
  the principal must be shown the type, told its settings, and able to
  write the record both as it stands and as it would stand after.
  Anything that fails refuses the whole report `not_granted`.
- An answered report names only what the principal is shown, narrowed
  as a purge's is. Its report id is the host's: the id names the act
  and the selection, not who asked, so a principal's answer and the
  host's for the same lake state cite the same report.

## Why

A count and a report are reads of records, so they are judged as reads
and writes are. A host that shows them to a person relays the lake's
answer and holds no rules of its own. The rule is the one the record
doors already apply. Coverage answers the records a read answers. A
purge's report answers only one who may retire each record it names.
The purge is a source's act, but whoever asks to see it acts through a
person.

The refusal is whole rather than narrowed. A report that left out the
records the principal may not retire would describe a purge other than
the one that runs. A host would then act on a report that understates
its effect.

## What it is not

- It is not a principal's purge or take-back. `purge_source` and
  `remove_values` stay the host's alone.
  Who may take a source's records back is the host's decision.
- It does not narrow `set_off`, `max_set_changes` or the count of
  quarantined rows a purge clears. Those come from the act's own
  bookkeeping, which knows no principal. The refusal above keeps a
  principal from receiving them for records it may not retire. A
  principal who may retire every taken record is shown the act's
  effect on them.
- It adds no tool and does not change the four per-type doors (014).

## How it came to be

- Added at lakelands#1594 and lakelands#1595: the principal's
  `vector_coverage` and `purge_preflight` on the `Catalog`, and the
  narrowing and the refusal they apply.
- Extended at lakelands#1844 and lakelands#1846: the principal's
  `remove_values_preflight`, and the right judged before the rows in
  both preflights.
