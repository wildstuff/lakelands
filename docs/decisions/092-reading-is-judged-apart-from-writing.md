# 092 — Reading is judged apart from writing

Proposed on lakelands#1569 and lakelands#1571 for 0.22.0, with the
operator's yes pending on its pull request.

## What is decided

006 judges once whether a type is shown to a principal, and every door,
a read's and a write's, asks that judgement. A `write_only` grant shows
a type to that judgement, since its principal writes it. A read then
answers what the write may see, which is too much. Three holes follow
from that:

- A type keyed by a field its principal may only write hands that
  field's value out through every record key (lakelands#1572).
- A principal that may write a type and read none of it is answered its
  records, empty but keyed, counted and walked (lakelands#1569).
- A host that must date a read by a valid time its reader may not see
  keeps a mask of its own in front of the lake, because the lake
  refuses the read or reads past the date (lakelands#1571).

This record extends `docs/decisions/006-grants-not-roles.md`. It needs no
log form and no format step, and the store's facade gains no method. One
grant flag is added to `TypeGrants` and to the WIT `type-grants`; it is
off by default.

### A key is known only where it is read

To a read, a key field the principal may only write is a field it may
not know of. A type so keyed is not there to the read: it answers as a
type that is not declared, and so does everything 006 hides with it. That
covers every type made of it, a link type of its relations, a view over
it, and a reference to it in another type, which the read then holds at
`none`. A key field the principal sees masked still shows the type, as
006 has it. The principal knows the field, and the key is how it tells
one record from another.

The write side keeps the type. The principal writes the key as it writes
the field, so:

- `describe` lists the type for its writes, with `may.upsert` and the
  key field `write_only`;
- `<type>_upsert` and `propose`'s record writes take it;
- a write answers its position and version, as 006 § "Nothing flows
  back" says, and never the key it wrote back as a read would.

### A type a principal may write and not read is not declared to a read

A type is shown to a write and not to a read when the principal holds a
`write_only` grant and either:

- the type is keyed by a field it may only write (above), or
- no grant on the type, the type's default or a field's, reads it or
  shows it masked.

A field with no grant of its own takes the type's default, and a type
with no default of its own takes the lake's. To a read, where fields of
a type are granted, the lake's default does not count for the type; it
counts only where the type's grant sets no default and grants no field.
This is the operator's decision, 2026-10-30. So under a lake default
that reads, a type whose every field is granted `write_only`, with no
default of its own, is read by none, and the read doors and `describe`
answer it as above. A field the grant does not name still takes the
lake's default, so granting one field `write_only` hides only that
field. A field held at `none` is granted nothing. That covers a
reference a sight holds there because its target is hidden, so holding
one does not take the lake's default from its type.

Every read door answers such a type as a type that is not declared, word
for word as to a principal granted nothing of it. That covers `find`,
`get`, a pattern started on it, a walk or a link to it, a reach and a
path through it, a view over it, a search, a Cypher query (which is
read as a pattern), the proposals listing and a watch (which reads
through `get` and `describe`).

The write side is unchanged:

- `upsert`, `ingest` under its door's authority, a verb's effect,
  `propose` and `retire` take the type as 006 says;
- a write answers as 006 § "Nothing flows back" says;
- `describe` lists the type for its writes alone, with `may.find` and
  `may.get` false, each field the principal may write as `write_only`
  and no other. `may.find` and `may.get` come from the same judgement
  the read doors ask. They are false exactly where the type is not
  shown to a read and is described for its writes, and true for every
  type a read is shown.

The tools follow the description. `<type>_upsert` is generated, and no
read tool names the type.

A type that holds a reference to such a type is described as a read sees
it: the reference is a field the reader may not know of, and it is not
listed. A write that sends the reference is judged as a write is, and
taken.

One judgement does it. A read door makes its sight as a write's is made,
then holds each type that sight shows and a read would not at `none`,
every field by name. It holds each reference to such a type at `none`
too, to a fixed point. From there the read asks the same questions every
door asks. A principal that holds no `write_only` grant anywhere is left
as it was, so its reads cost nothing more.

### Undeclared, not `not_granted`

A field the principal may only write is refused `not_granted` when a
read probes it (006). The principal is shown the type, so it knows the
field exists, and the read is answered around it. The same answer was
weighed for a type a principal reads nothing of, and undeclared was
chosen:

- **006 already says it.** For reading, `write_only` is as `none`. A
  type whose every grant reads as `none` is not shown, and it answers as
  a type that is not declared. A type held only through writes is that
  type.
- **A refusal would tell what is there.** `get` could refuse the type
  before it reads a record. A walk, a link step, a view over the type
  and a reference to it meet the type through records, though. A
  `not_granted` raised there would say which records stand and which
  are linked to them, and that is what a principal granted nothing of a
  type may not learn. Undeclared is answered from the declarations,
  before any record is read.
- **One answer at every door.** The type is held at `none` in the read's
  one sight, so each read door, and each one added later, answers it as
  it answers a type the principal was granted nothing of. A refusal
  would be one more thing each door has to raise.
- **The writer loses nothing it had.** It learns that the type exists
  from `describe`'s write side. The read side hides the records, not the
  type.

### A grant may date a read by a valid time its reader does not read

`TypeGrants.dates_by_hidden_valid_time` is a flag, false by default and
omitted when false. In WIT it is `dates-by-hidden-valid-time`. Where a
principal's grant on a type says it, a read at `valid_at` filters that
type's records by their stored `valid_from` and `valid_to`, whether the
principal reads them, sees them masked, may only write them, or may not
know of them. Such a read probes neither bound, so it is not refused
`not_granted` for them.

The fields stay withheld as the grants say:

- a masked bound is named and its value withheld;
- a hidden bound is absent;
- `describe` shows the type as without the flag, with no valid time
  where the bounds are hidden.

The flag reaches every read at a valid time, and each is judged by the
grant on the type it filters:

- `find`, `get`, a count, a pattern's start and a search, by the grant
  on the type;
- a walk, by the grant on each type it meets;
- a view computed at a valid time, by the grant on each source, merged
  and nested ones included, for the filter on that source's records,
  and by the grant on the view for its own valid time.

Without the flag the reads answer as they do today
(006 § "A read at a valid time probes the valid-time fields its reader
is shown").

A type's grant follows its renames as 006 says. The flag holds only
where the grant under every name the type has borne says it. A grant
reaches past its own name only to take something away, and dating by a
hidden bound gives the reader more than it had.

**The trade-off.** An answer at a date tells the reader whether a record
was valid then. Asked at two dates, a record answered at one and not at
the other places a bound between them, and repeated asking narrows a
bound the reader may not read down to the width of its asks. The flag
accepts that for the type it is set on. The host that sets it has
judged that what the answers tell of the dates is less than what a mask
of its own costs it: a reader without the flag is refused, or read past
the date. The operator accepts this trade-off for a grant that says so,
and for no reader by default.

## Why

- **A grant that writes must not read.** A host compiles "may write,
  may not read" to `write_only` because 006 promises that `write_only`
  is never read. A key, a bare record and a count are reads.
- **Judging the two apart keeps one judgement.** The rule that shows a
  type to a write still shows it, and a read is that judgement with
  `write_only` taken as `none`. Every read door then asks one sight, as
  006 requires, and no door can miss the rule.
- **Dating by a hidden bound is the host's choice.** A reader that is
  refused, or read past the date, is answered something a host cannot
  pass on to a model as the records valid then. The host that needs the
  dated answer says so per type and accepts what it tells. Every other
  reader is answered as before.

## Rejected

- **`not_granted` for a type a principal may write and not read**: see
  "Undeclared, not `not_granted`".
- **Hiding the type from writes too.** It closes the read leak in one
  line, but it takes `upsert`, `propose` and the generated write tool
  away from the principal 006 grants `write_only` to so that it can
  write. That is the case the grant exists for.
- **Dating by every hidden bound, with no grant.** It would change what
  every masked or hidden reader is told, on every lake, without its host
  asking.
- **One flag for the lake.** The trade-off is judged per type: a
  contract's end date and a ward assignment's do not tell the same
  amount.

## What it is not

- It changes nothing for a principal that holds no `write_only` grant.
- It does not change what a write answers, or what a refusal of a write
  names.
- It does not let a masked key field hide a type: the field is known.
- The flag does not show a bound. It does not let a reader filter,
  order or aggregate on one, and a `where` on a bound the reader does
  not read is refused as before.

## Checks

On every engine. Each was seen red before the build.

1. `masking::WRITE_ONLY`, extended (lakelands#1569). The lake declares:
   - `ward`, with a searchable `name` and a `bed`;
   - `wing`, keyed by `k`;
   - a count view `per_bed` over `ward` by `bed`;
   - `round`, carrying a relation `of` to `ward` through `on`;
   - `bay`, declared as `ward` is.

   A clerk's lake default reads. It holds `ward` at
   `{default: write_only}`, `wing` at `{k: write_only, name: write}` and
   `bay` at `{name: write_only, bed: write_only}` with no default of its
   own, with review. It upserts all three and proposes on `ward`, and is
   answered `Applied` and `Proposed`. `describe_type` gives each
   `may.find` and `may.get` false and `may.upsert` true, and lists its
   declared fields by name, each `write_only`, so an empty list does not
   pass. The tools hold each one's upsert tool and no read tool of it.
   `get`, `find`, a pattern and a search on `ward`, `wing`, `per_bed`
   and `bay`, a walk from `round` over `of`, and the proposals listing
   each answer the same bytes as to a clerk granted nothing of the
   three.
2. `masking::VALID_AT_PROBES`, extended (lakelands#1571). A clerk dated
   by the grant holds the start or the end masked or at `none`. On each
   of the four, a read in March answers no record and `get` is refused.
   In April, `find` and `get` answer the two valid records. None tells
   the bound's value.
3. `masking::VALID_AT_VIEW`, extended (lakelands#1571). A reader dated
   by the grant on the source, masked or at `none` on its start, is
   answered the view at each time as a reader of every field is. A
   reader dated by the grant on a kept view with a valid time of its
   own is answered as the owner is:
   - masked, write-only or at `none` on the view's `valid_from`, in
     January and with no valid time;
   - at `none` on the view's `valid_to`, in July, which leaves out the
     group whose end has passed. Undated, it reads as a view with no
     end.
4. `masking::VALID_AT_MEETS`, extended (lakelands#1571). A walk at a
   valid time, by readers dated by the grant on the types it meets,
   masked or at `none`, answers in March and in July what a reader of
   both answers.
5. `masking::WRITE_ONLY_KEY` (lakelands#1572) pins the key as a read
   sees it. The write side is pinned by check 1, whose `wing` is keyed
   by a field its clerk may only write.

Unit row beside `src/` (`lakelands-api`'s principal tests): the flag
held through a rename only where every name says it.

## Cost

Measured with `scripts/gates/core-size.sh` against `develop` with the
0.22.0 reserve, leaving out lakelands#1572's probe. Seam width does not
move, the facade gains no method, and the engines do not move.

- **Product (core crates): +233.** `lakelands-core`: the reading
  judgement in `type_shown`, the reading sight, and the description of a
  type for its writes alone. `lakelands-store`: the read doors make the
  reading sight, and a read dates by the grant. `lakelands-api`: the
  grant flag and its rename rule.
- **Specification (core crates): +240**: checks 1 to 4 and the unit
  row.
- **Outside the core:** the WIT field, the pointers in 006, the bullets
  in `docs/hosting.md`, and the regenerated `docs/conformance.md`.
- **Budget rows.** This record raises no row. Both gates fit within the
  reserve set for the 0.22.0 round.

## How it came to be

- Proposed on lakelands#1569 and lakelands#1571, for 0.22.0. A host kept
  a guard of its own for both.
- Review found a type that `describe` said may not be read while `get`
  and `find` still answered it. Every field was granted `write_only`,
  and the type took its default from a lake default that reads. The
  description had judged reads from the fields, and the doors from the
  type's default. The operator decided on 2026-10-30 to hide the type
  from reads: where fields of a type are granted, the lake default does
  not count for it. `describe` takes `may.find` and `may.get` from the
  one judgement.
- lakelands#1572 found while probing that a write-only key field leaks
  through every record key. It ships in 0.21.1 on its own, with the
  key's test changed for every door. This record states the rule as
  reads and writes apart, and gives the write side back the type 0.21.1
  hides from it.
- The record was written by hand: the repository has no generator for
  records.
