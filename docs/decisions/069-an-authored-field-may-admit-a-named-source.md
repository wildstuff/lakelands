# 069 — An authored field may admit a named source

## What is decided

Decided on lakelands#1264 by the operator (2026-10-19), for 0.17.0. It
extends 016, whose doors it opens by one named source, and leaves 039
and 065, which take that source's values back, as they are.

### A field of authority `authored` may name the sources it admits

A field declares `admits: [<source id>, …]`. Its authority stays
`authored`: `upsert`, `link` and verbs write it as before. `ingest`
writes it too, but only a record whose source is one the list names (the
call's source, or the record's own where it names one). Every other
source is refused on the field, `wrong_door`, as today.

The declaration refuses `admits`, at the field's `admits`:
- on a field whose authority is not `authored`: a `shared` field admits
  every source already, a `source` field is any source's, and no door
  writes a `derived` or a `verb` field from a source
  (`admits_not_authored`);
- on a view's field, a relation's field, a verb's payload and a
  function's input, which no ingest writes (`admits_not_of_type`);
- on a field of a type that keeps no per-field provenance (010): the
  rule below and the purge both read who wrote a value, which such a
  type does not keep (`admits_no_provenance`);
- with a source id that is not formed as one (016, `source_id_form`),
  or named twice (`admits_twice`).

A create by `ingest` fills a key's fields from the record's key, as
every create does. On a type whose key fields are authored, each of
them must admit the source too, or every create from it is refused
`wrong_door` on the key field.

`admits` is not hashed (007): it says who may write the field, not what
a record holds. Declaring it, or taking it away, leaves the schema hash
as it was. It does not change `erasable`, which an authored field still
refuses (058).

A value an admitted source writes carries the provenance every ingest
writes: `origin: source`, the source, the writer, the delivery. Nothing
new marks it. `purge_source`, `remove_values` and `forget_source` read
that provenance, so they take these values as they take any of the
source's. Forgetting erases only values of an `erasable` field, which
an authored field never is (058): it retires the rows the source made
and alone brought, and leaves its values in a row another hand made or
co-owns, which `remove_values` takes.

### An admitted source writes only where no other hand's value stands

On an authored field, the admitted source writes only where no hand
wrote the field, or where that same source wrote it last. On a record
the source made, it also writes over the field's declared `initial`
(filled at the create, or by a migration's fill): the record took it
because the source's write made it. A record is the source's own where
its making write, at the record's `made` position, was the source's.
On a record an author made, an `initial` is a value the field holds,
and it stands: writing over it would change what the author's record
started with, and taking the source back could not give it back. Where
an author, or another admitted source, wrote the field last, a value
or a clear, it stands too: the provenance a clear leaves names who
cleared. The source's record is refused on that field, `wrong_door`,
and its window appends nothing (016). Clearing its own
value, and writing it again, are its own writes and are applied. A
record that would change nothing is a duplicate, as any is, and
appends nothing.

The rule is judged at the door, against the record as it stands. The
fold judges nothing again, and no event carries the rule.

### Admitting is a declaration, and the log needs no new event

Who may admit a source is whoever may declare: a principal holding
`schema`, directly or through a proposal that is accepted (013, 062).
The admission is the field's `admits` in the type's `declared` event. A
`declared` without it, one version on, closes it again. No event kind
is new. `admits` is a new member of a field in the log format: the
format step of the release that builds it, with no converter, since no
earlier log holds it.

### `describe` shows the sources a field admits

A field's description carries `admits`, the source ids in the order
they were declared, wherever it carries the field's `authority`.
Whoever is shown the field is shown both. `describe` as of a position
shows the admission in force there.

### A row an author edited or made keeps the author's values

A row the admitted source brought in, which an author then edited, is
co-owned while a value of the source remains in it.
- `purge_preflight` and `purge_source` refuse it whole and name it, as
  039 refuses any co-owned record. The purge does not take it.
- `remove_values(source, …)` (065) takes the source's values from it,
  keeps the author's, and retires a record the source made that is left
  with nothing of another hand. This is the host's act for example rows
  that people may have edited.
- A record an author made is never retired by taking the source back,
  even where the author's only value is its key or a field that
  declares `initial`. `remove_values` erases the source's values there
  and keeps the record; the purge and its preflight refuse it as
  co-owned; forgetting keeps it. All three read who made it at its
  `made` position (039). Before 069, `remove_values` retired it, since a
  key field co-owns nothing there; 065 now says so.
- Once the author has written over every value of the source, the row
  is the author's. The purge passes it over, as 039 says, and so does
  `remove_values`.

The preflight says nothing new.

### Closing the admission leaves the rows already written

The host declares the field again without the source. From that
position on, the source is refused on the field, a redelivery of a row
it wrote before included. Rows already written keep their values and
their provenance. A `declared` never reshapes a stored record (the
log format, §2.2), so they stay the source's, and its purge or removal
still takes them. Only the named source could write the field, so closing needs to know
nothing about other sources.

A field a host opened as `shared` in the meantime closes the same way:
declare it `authored` and admit only the source. Values other sources
wrote while it was shared keep their provenance. Each source's purge
still finds its own, and the provenance's `writer` and `source` say who
they were.

### What it takes to build

Estimated from the field that only verbs write (lakelands#1130), the
nearest shipped change of a door rule plus a declaration member:
- **Product:** about +200 to +260 lines. A field member and its
  description in `lakelands-api`; the declaration's checks and
  `Door::admits` taking the field and the source in `lakelands-core`
  (`rights.rs`); the own-value rule in the ingest's judgement in
  `lakelands-store`; the declare and describe tool schemas in the
  server.
- **Specification:** about +350 to +450 lines, in five checks; seven
  as built.
- **Docs:** about +80 lines: 016's door table, the log format's field
  member and format step, the hosting page's entry on rows that one act
  removes.
- **Public API:** a new member on `FieldDecl` and on
  `FieldDescription`. No new method and no new refusal reason: a write
  is refused `wrong_door`. The declaration's refusals are four new
  cases and the place `admits` (above).
- **WIT:** `admits: list<source-id>` on the field records of `types.wit`
  that carry `authority`.
- **The log:** one field member, in a format step. No new event.

The checks that would pin it:
1. An admitted source writes an authored field. Another source is
   refused on it, `wrong_door`. An author's `upsert` still writes it.
   `describe` shows `admits`.
2. The declaration refuses `admits` on a field that is not authored,
   on a relation's field, a verb's payload, a view's field and a
   function's input, and with a malformed or repeated source id.
   Admitting and closing leave the schema hash as it was.
3. An author's value stands. The admitted source's redelivery onto a
   field an author wrote since is refused on that field and appends
   nothing, and so is a second admitted source onto the first's value.
   The source's own value is written again, and cleared. The source
   writes the `initial` of a record it made, and is refused on the
   `initial` of a record an author made.
4. The source's purge takes the rows it brought in. A row an author
   edited in part refuses the purge whole, and `remove_values` takes the
   source's values from it and keeps the author's. A row an author
   wrote over whole is passed over by both, and so is a record an
   author made. An author's record holding a value of a field with an
   `initial` alone, one of whose empty fields the source filled,
   refuses the purge; `remove_values` erases the source's value and
   keeps it.
5. Closing refuses the source from its position on. The rows written
   before keep their values and provenance, and the source's purge
   still takes them. `describe` as of before the close shows the
   admission.
6. On a type keyed by an authored field, a create from the source is
   refused on the key field until it admits the source. A record an
   author made holding its key alone takes the source's value in an
   empty field; the purge refuses it, and `remove_values` erases the
   value and keeps the record.
7. Forgetting the source retires the rows it made and alone brought,
   and keeps a co-owned row whole, the source's value included, and an
   author's record holding a value of a field with an `initial` alone,
   one of whose empty fields the source filled.

## Why

A host writes example rows into a type the operator modelled, so that
one act removes them again. Until now its only way was `shared`, and
the probe on lakelands#1264 shows what that costs. A field opened that
way takes every source that holds `ingest` and may write it. A source
that delivers a row again writes over an author's edit, because the
later position wins. And once a field is shared, nothing says it was
opened for one source, so closing it means finding who else wrote since.

The admission keeps the field authored and names the one source let
in. So:
- every other source is refused, as before;
- an author's edit is never overwritten from a source;
- the opening is in the log and shown by `describe`;
- closing it is one declaration, because only the named source could
  have written.

It reuses the provenance the purge already reads (039), so the host's
one act stays `purge_source` or `remove_values`. It adds no event and
no refusal.

**Weighed and left out: a per-source grant** (a principal's grant that
lets it ingest into authored fields). Grants are the host's, given per
call by its resolver (006). They are not in the log, and `describe`
shows a principal only its own grants. So the operator who modelled
the type would see nothing of the opening, `describe` as of a position
could not answer it, and closing it would leave no mark. A grant also
admits a principal, while the purge selects by source. The principal
could write under any source name, so the grant would not fix what one
act removes.

**Weighed and left out: a dedicated origin for example rows** (a new
`origin`, or a reserved `sys.` source every authored field takes). It
is a second way to say "came from a source", which lakelands#611 kept
out. Every reader of provenance would need a new case: the purge,
`remove_values`, forgetting, the feed and a mirror. It would cost a
format change of the event, not only of a declaration. A reserved
source taken everywhere would open every authored field of every lake
to a name any `ingest` holder may give. And it fits only example rows,
where the need is one named source.

## What it is not

- **It does not bind a source name to a principal.** A source id is the
  name the caller of `ingest` gives. A principal holding `ingest` may
  deliver under any name, the admitted one included (the probe's step
  P5). The admission refuses every other name, not every other
  principal. This is the trust every source field has today: a
  delivery under another source's name writes that source's fields too.
  A grant that names the sources a principal may deliver under would
  close it for both, and it is a question of its own.
- It does not change `shared`: the later position still wins there.
- It does not change what the purge, `remove_values` or forgetting
  select. It changes what they may retire: only a record the source
  made; the purge is refused on any other (above, lakelands#1278).
- It does not let a source write a field an author or another source
  wrote, and it does not let an author's write carry a source's name
  (lakelands#611 keeps that out).
