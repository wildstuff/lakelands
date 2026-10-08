# 051 — A grant may admit only some records

## What is decided

A principal's grant on a type may hold `only`: pairs of a field and a
value. Such a principal, a holder, is admitted only the records whose
fields hold every pair, and every other record of the type is, to it, a
record that does not exist.

- **Identity, text, closed.** A pair holds where the stored value is the
  same JSON value, as `Scan.equal` judges it. A pair must name a field
  the type declares, of kind `text` or `reference`, that the holder may
  read, with a text value. A grant that names anything else is not
  applied and not guessed at: the doors that name the type refuse it as
  invalid, naming the type and never the field, until the host fixes the
  grant; a walk or a view that reaches the type admits none of its
  records. Two names of one field that a rename joins with differing
  values hold null, which no pair may hold.
- **Every read.** `get` of another's record is not found, as a missing
  one is. `find` with a filter, an order, `total`, a count, a sum and a
  group read the engine under the pairs, so they count and fold the
  holder's records alone. A search ranks the type and keeps the admitted
  candidates, pulling deeper until its page is full; the fused search's
  path that reads no record is not taken. A walk reaches only admitted
  records. A proposal on a record the holder is not admitted, before the
  write or after it as proposed, is listed without its write; it still
  shows why, the proposer and a type-level verb's payload, as one on a
  restricted record does. A read as of a position judges the records as they stood
  then, under the grant followed to that position.
- **A search as of a position** keeps a candidate only where it is
  admitted then and at the head too, under the grant followed to the
  head and by the name the type bears there: the ranking reads the text
  and vectors at the head, and another's current text must find nothing.
  A cursor page and a query started by a search keep working.
- **Views.** A view's effective `only` is its own pairs, plus, where it
  has one stored source, no merge, and groups by every field the
  source's pairs name, the source's pairs on its group fields: each
  group's records then share those values, and the admitted records are
  the groups that hold them, so a kept view stays kept and is read under
  the pairs. Otherwise a view that reads a type held to some records, or
  a link type whose end type is, is computed from the admitted records
  alone. `describe_type` says `only some` of such a view to its holder,
  never whether every owner's groups are exact.
- **Links.** A link record is withheld from a holder of `only` on an end
  type unless that end is admitted; its ends are read once per page of
  links. `only` on a link type admits its links by their fields, in a
  walk too. Readers holding no `only` on either end are answered as
  before.
- **A holder writes its own records alone.** A write is judged against
  the record the holder owns at the key: the record there, retired or
  not, where its grant admits it, and otherwise none. Another's record
  is, to the holder's write, a key that is free, so every check (the
  expected version, the key fields, a reference, a required field)
  answers as it would on a free key. A write that passes them all is
  refused as not granted, naming no field, where another's live record
  stands at the key or the record the write leaves is not admitted: a
  free key with another's owner, another's key and a handover answer
  alike. `retire`, `unlink` and a verb read the record first: another's
  is not found, as a missing one is. A link is the holder's where each
  end it holds `only` on is admitted. `ingest` compares a re-delivery
  only against the holder's own record, so a re-delivery onto another's
  key is refused as on a free key, not counted a duplicate.
- **What a write still tells.** A create succeeds on a free key and is
  refused on another's: a key's existence shows where a write would
  make something, and a key that carries the owner (the owner field
  among the key fields) keeps holders' keys apart. A create on a key
  whose record another retired continues that record's version, and
  takes its `restricted` mark, so the holder may not read its new
  record unless it holds that grant. A refusal for a relation's
  cardinality names the other link's end only where its writer is
  answered that record, `restricted` judged as for any reader, and
  `ingest` judges a link's cardinality only once the record its writer
  leaves is admitted, as the mock does: a link from another's record
  answers as one from a free key, and a link a grant's `only` on the
  link type admits by its own fields holds the relation's cardinality
  as any other.
- **A holder proposes writes to its own records, pinned to them.** Its
  `propose` of an upsert, a retirement or a call, and a call of a verb
  above low risk, which is a proposal, are judged as the direct write
  is, against the record it owns at the key. The proposal is logged
  `pinned` where the holder holds `only` on the type it writes, a link
  type with a held end included (`docs/log-format.md` §2.4). The accept
  of a pinned proposal is refused as invalid, the proposal left pending,
  once the record at its key has an event after the proposal's position,
  live or retired, or, for a link, either end record has one (a link is
  its holder's by its ends, and a change to an end moves no link), or
  the type was declared or migrated since, a re-key included: a rederive and a backed output move the
  record without a write, and a record made again at the key starts its
  versions anew, so the position, not the version, is what the accept
  compares. So the accept applies the write to the record and type its
  proposer was judged on. The proposer's grant is not in the log, so a
  holder may propose only where nothing but the write can move its
  pairs: a proposal is refused where a field its `only` names is derived
  or backed, or where the verb assigns one. A reference a pinned
  proposal writes is judged at the accept as a live record, as for any
  proposal: it names a key the proposer knew when it proposed. The
  accept writes in the proposer's name, not under its grant, and the pin
  compares records and types, not grants: a grant narrowed or revoked
  between the proposal and its accept does not stop a pinned proposal.
  The reviewer decides it, on what the listing shows.
- **A reviewer holding `only` decides what it is shown whole.** It
  accepts or rejects a proposal on a type it holds `only` on only where
  the proposal listing shows it the proposal whole: the record at the
  key, before the write and after it as proposed, admitted, and every
  value the write carries one it may read. Otherwise it is refused as
  not granted before it is told whether the proposal is decided, so a
  proposal id, which is positional and so can be guessed, tells it
  nothing the listing does not. Its accept is judged by its own pairs
  too: a write that would leave the record outside them is refused as
  not granted, as its own write would be.
- **What stays refused**, as not granted, naming no field: an aging rule
  on the type, which moves records at the host's run. A holder of `only`
  on any type changes no shape: every schema door and the proposal of a declaration or a
  migration are refused to it, and no migration's proposal, of a type or
  a value type, is listed to it, since a report counts every owner's
  records. A reference written into the type from another type must
  name an admitted record.
- **`describe`** tells the holder its pairs in `may.only`, where the
  grant can be applied, and offers no write, link or verb that would be
  refused: an upsert only where it writes every field its `only` names
  at the authored door, so that a create can succeed, and no verb above
  low risk that it may not propose. A field's `initial` value is not counted there: where only
  an `initial` value would make a new record the holder's, the write is
  admitted (the record it leaves is judged, `initial` values included)
  but no upsert is offered. The change feed is the log unmasked, so a holder does not
  read everything and is refused it at a door.

`only` is a function of `Principal.id` within a lake: a host gives each
owner its own id and never gives one id `only` on some calls and not on
others. A retry is answered its first outcome before the grants are
asked, as every retry is; under this contract a holder's earlier call
was refused, and its retry answers that refusal.

## Why

A host often needs "each sees only their own": an employee their claims,
a customer their invoices. Filtering above the lake works record by
record but not for a sum, a group, a search, a walk or a view, so such
doors had to be refused instead of answered. The store reads every
record through two places, the engine's scan and the check of a record
read by key, and both take the pairs: the engine selects only admitted
records, so every answer built from a scan sees only them, and no door
filters after the fact.

The value is in the grant, not an attribute name: the host's resolver
already builds the grants per call and writes the owner's value in, so
the store never decides which attribute of a principal counts. Text
identity keeps the rule one comparison every engine already makes, with
no decimal or float spelling to agree on, and lets the engines narrow by
an index.

A holder's write is judged against its own record, never another's,
so that no answer it gets is made from a record it may not read; the
refusal of a write onto another's key comes last, where a free key
would have been written, and that one difference is the existence a
create cannot help showing. A proposal waits for a later accept that
re-judges the write under no grant of the proposer's: the pin makes the
accept apply the write to the record and the type its proposer was
judged on, or refuse it. The pin is one bit of the proposer's grant in
a log that is not masked: whoever reads the log learns that the
proposer held `only` on the type, never on what. A reviewer is a reader
of what it decides, so it decides only what it may read, and what it
accepts it is answered for as for its own write (lakelands#660).

## What it is not

- It is not several admitted values for one field, nor a predicate
  other than equality.
- It is not taken from `Principal.id` implicitly.
- It does not change `restricted`: a record must pass both rules.
- A reference value naming another's key is shown as a key; the record
  behind it is not found, as for a restricted record.

## Extended: `only` keeps the reference it names (lakelands#718)

A reference to a type its principal is not shown is a field it may not
know of (024, extended). A field the effective `only` of a type names is
kept as granted: its value is the pair the host wrote into the grant, so
it tells the holder nothing, and `only` stays one that can be applied.
The effective `only` counts, a view's included: a view grouped by its
source's `only` field keeps that field, and so its key, and does not
vanish (006). The type a grant names, and the value it holds a field
to, are the host's own choice: naming a type in a grant shows its name
to no one.


## Extended: a restricted record is not there to a write (lakelands#772)

A restricted record is a hold: 006 keeps it from every reader without
the `restricted` grant, and a writer that may not read it must not
change it, point at it, or learn its version. So the record is, to such
a writer, not there at any write door, as another's record is to a
holder of `only`. A reference to it and either end of a `link` are
refused `not_live`, as for a key no record holds; `retire`, a verb
called on it, and `unlink` of a link with a restricted end are not
found; an upsert onto its key is refused as not granted, naming no
field, so the key is told as a create tells it and the held record is
neither written nor its version shown. A retired record that is held
counts as another's too, so no upsert, ingest or proposal writes into
it, and an ingest judges a link's ends as `retire` does. A proposal of
each is judged as its write. Its accept judges the write as the records stand then. The
proposer's grant is not logged; a proposal that reached a record
restricted when it was proposed could only be a holder's, and is
accepted as one; any other is judged without the grant, so a record
restricted since the proposal is not there to its accept. A proposal
whose record its reviewer is not answered is listed without its write,
as before, and that reviewer decides it no more than a holder of `only`
decides one it is not listed whole: it is refused as not granted.
