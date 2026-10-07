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
- **What stays refused**, as not granted, naming no field: a verb above
  low risk on the type or on a link type with a held end, `propose`,
  `accept` and `reject` of a write that reaches it (a proposal is judged
  again when it is accepted, and the accept does not know the
  proposer's grant), and an aging rule on the type, which moves records
  at the host's run. A holder of `only` on any type changes no
  shape: every schema door and the proposal of a declaration or a
  migration are refused to it, and no migration's proposal, of a type or
  a value type, is listed to it, since a report counts every owner's
  records. A reference written into the type from another type must
  name an admitted record.
- **`describe`** tells the holder its pairs in `may.only`, where the
  grant can be applied, and offers no write, link or verb that would be
  refused: an upsert only where it writes every field its `only` names
  at the authored door, so that a create can succeed, and no verb above
  low risk. A field's `initial` value is not counted there: where only
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
re-judges the write under no grant of the proposer's, so a holder's
proposals and the verbs that become proposals stay refused; admitting
them needs the accept pinned to the record as it was proposed
(lakelands#660).

## What it is not

- It is not several admitted values for one field, nor a predicate
  other than equality.
- It is not taken from `Principal.id` implicitly.
- It does not change `restricted`: a record must pass both rules.
- A reference value naming another's key is shown as a key; the record
  behind it is not found, as for a restricted record.
