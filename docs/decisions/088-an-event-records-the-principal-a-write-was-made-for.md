# 088 — An event records the principal a write was made for

## What is decided

**Proposed** on lakelands#1489, for 0.21.0. The operator reviews it on
the issue; merging it does not accept it. It adds one envelope member
to log format 21, which 0.21.0 writes for the first time, so it needs no
step of its own. It extends
`docs/decisions/021-the-write-context-belongs-to-the-host.md`, whose
context gains the member, and
`docs/decisions/010-provenance-per-field.md`, whose provenance names it
beside the writer. Each comes into force with the build, which rewrites
those records where they stand.

A component often acts for a person: a mail agent files the ticket a
customer asked for, an importer writes what a clerk uploaded. Until now
an event names one principal, its `writer`. A host that records the
person there loses the component. A host that records the component
loses the person.

### The host names the principal a write is for, in its context

`WriteContext.on_behalf_of: Option<String>`, serde default and skipped
when `None`, beside `cause`. It is a principal in the form an event
names its writer in, `<kind>:<id>`. Like the rest of the context it is
the host's: a host binds a `Catalog` to a context that names it, and no
door (MCP, REST, the CLI, the client) takes it from its caller. WIT's
`write-context` gains `on-behalf-of: option<string>`.

It names an identity, not a `Principal`. The host already holds the
person's grants, and the store is given none of them (below).

### Its form is checked where the write appends

The store refuses a write whose context names an `on_behalf_of`
- that is not `<kind>:<id>` with `<kind>` one of `human`, `agent` or
  `service`: the store acts for no one, so `system` is refused;
- whose id is empty or holds `0x00` (`docs/log-format.md` §3);
- that equals the writer: one fact is recorded one way.

It is refused `invalid` (`on_behalf_of_form`), naming the member and
what it holds, before anything is appended. The check runs on every door
that writes, as the depth ceiling does (021).

### It goes with the writer

An event that names a principal as its `writer` names, in
`on_behalf_of`, the principal that writer acted for in the write the
event records:

- the events a host's write appends under its principal: its
  `record_upserted`, `record_retired`, `record_ingested`,
  `record_quarantined`, `verb_called` and the verb's writes, links,
  declarations, proposals, acceptances, rejections and dismissals;
- the write that follows a `write_accepted` is the proposer's, so it
  names the proposer's `on_behalf_of` from the `write_proposed`, never
  the accepter's. The `write_accepted` names the accepter's;
- an event the store writes as `system:lakelands` (a fired trigger, a
  set membership, a field it cleared, a close it decided) names none.

### The envelope carries it beside `writer`, never inside `context`

From format 21, the envelope gains `on_behalf_of` (string, present when
the write was made for someone), after `writer` in the table of
`docs/log-format.md` §1.1. Like `cause`, it never sits inside
`context`: a reader refuses a `context` that carries it, naming it, and
refuses the member on an event of a format before 21.

It is part of the event's canonical bytes and so of the hash chain, as
every envelope member is. Like `writer`, it is not in the preimage of a
`record_ingested` id (§3): a record delivered again for another person
is still a duplicate, and stays with the delivery that first wrote it.

### Provenance names it, and every reader of history sees it

`FieldMeta.on_behalf_of: Option<String>`, beside `writer`. The fold writes
it from the event every time it writes `writer`, so a replay, a rebuild,
a catch-up and an import answer the same. A field the store fills by its
own hand (`authority: writer` at a create, an `initial`, a migration's
fill) names none. A read with `with_meta` returns it where present.

The events a host reads (the feed, a record's history, an event by its
position) carry it as they carry `writer`, at every door. No door filters
by it: a filter is a query, and history answers by position and record.

### Rights are judged on the writer alone

The store judges grants, ceilings and pins on the principal the write is
made as, never on the one named in `on_behalf_of`. That principal is
recorded, not trusted: the host vouches for it, as it vouches for the
context's depth and cause. A host that wants a person's rights to bound
what a component writes for them binds the component's principal with
grants no wider than the person's
(`docs/decisions/006-grants-not-roles.md`). The store has no other way to
know them.

### What it takes to build

Estimated from the nearest shipped sibling, the host's `cause` in the
context (021), and the provenance member `upstream` (010).

- **Product (core crates), est.:** +150 to +250. The member and its docs
  in `lakelands-api` (`principal.rs`, `event.rs`, `record.rs`); in
  `lakelands-core`, the envelope member, the reader's refusals and the
  canonical bytes; in `lakelands-store`, the form check, the stamp on each
  event a write appends under its principal and the proposer's on accept,
  and the fold's provenance. The engines store the event as they store it
  now; seam width does not move.
- **Specification (core crates), est.:** +250 to +400: the checks below,
  and one line in the context literals of the suite and the tests.
- **Outside the core:** WIT +3; the doors' event and meta shapes (REST,
  MCP, the client) +20; `docs/log-format.md` §1.1, §2.3 provenance prose
  and the format-21 vector, `docs/rest.md`, `docs/refusals.md` and
  `docs/conformance.md` (regenerated). Where the vector needs a reader of
  its own, it joins lakelands#1534.

The checks, on every engine:
1. `context::ON_BEHALF_OF_RECORDED`: a catalog of `service:mailer` bound
   for `human:alice` upserts, links, calls a verb that writes, and
   ingests one record. Each event of those writes names
   `human:alice`, the `trigger_fired` and `object_set_changed` they cause
   name none, and `with_meta` names `service:mailer` and `human:alice` on
   each field written. A rebuild and a reopen answer the same.
2. `context::ON_BEHALF_OF_FORM`: `system:x`, `robot:x`, `human:`, an id
   holding `0x00`, and `service:mailer` itself are each refused
   `on_behalf_of_form`, naming the member, with nothing appended.
3. `proposals::ACCEPTED_FOR_THE_PROPOSER`: `agent:a` for `human:alice`
   proposes an upsert, and `human:bob`, for no one, accepts it. The
   `write_accepted` names `human:bob` and no `on_behalf_of`; the upsert
   names `agent:a` for `human:alice`.
4. `ingest::ON_BEHALF_OF_NOT_IN_ID`: the same record delivered for
   `human:alice` and then for `human:carol` is one event and a duplicate.
5. `format::ON_BEHALF_OF_FROM_21`: a reader refuses the member on an
   event of format 20, and a `context` that carries it, naming it.

## Why

History is asked two questions: who did this, and for whom. A component
answers the first, and an audit of what an agent did needs it. The
person answers the second, and a person's view of their own records needs
that. One `writer` answers one of them. A host that tucks the other into
a field of each record writes it into every type, and loses it wherever a
write touches no field of its own: a link, a retirement, a verb call.

The context is the right home because it is already where the host says
what the store cannot check: where a write comes from, how deep it sits,
what it follows from. The person a component acts for is one more such
fact. Putting it in the envelope rather than in the context object keeps
`context` about the write's place in a chain, as `cause` does.

Format 21 has not been released, so the member is added to it, not
given a step of its own.

## What it is not

- **Not delegation of rights.** Nothing is granted or narrowed by it;
  rights stay the writer's (above).
- **Not a chain.** One principal, not a list of who acted for whom. A
  host whose component acts for a component that acts for a person names
  the person, and its own record of the middle is its own.
- **Not a door parameter.** No caller sets it, as no caller sets the
  context (021).
- **Not a field authority.** A field that holds the principal a record
  was made for, beside `authority: writer`
  (`docs/decisions/084-a-field-holds-the-writer-that-made-its-record.md`),
  is not part of this; it waits until a host asks for it.
- **Not a filter.** No door finds events or records by it.
