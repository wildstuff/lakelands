# 006 — Grants, not roles: what a principal is shown and told

## What is decided

This record states the decision: the store knows principals and grants,
never roles, and it checks every read and write against them before it
answers. It holds the grant itself, and what it absorbed from 024 and 051
now lives one record per family, each citing this one: a read, a probe, a
write, a call, a listing and a refusal. The table below maps what this
record held to where it is. 013 and 027 keep their own topics, verbs and
shape changes, and point here for what moved.

### A call carries one principal with grants, and the store checks every read and write

The store knows principals and grants, never roles. Every call carries
one `Principal` (kind, id, lake, grants), and the grants say, per field,
what that principal may read and write. The store enforces them on every
read and every write: a field the principal may not read is masked or
absent in the answer, and in the schemas the generated tools expose; a
write carrying a field the principal may not write is refused whole.

Turning a host's identities, roles, teams or policies into grants is the
host's job, done in its `PrincipalResolver`. Lakelands' own default
resolves an API key or a static configuration entry to one principal.

What a handle keeps of the lake's shape between reads holds nothing of
a reader (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`):
what is shared is what the lake declares, before any rule of who is
shown what. A principal's grants, its renames followed and its `only`
are applied per read.

#### A grant that is not understood stops the root

A grant exists to narrow what a principal may do, so a part of one that
is read past takes a restriction away without a word: `onyl` for `only`
leaves its holder reading every record. `Grants`, `TypeGrants` and
`HiddenWhere` therefore refuse a key they do not know. A root whose
`lakelands.json` holds such a key does not open, for `serve`, `--as` or
any other command: the refusal names the key, the principal and the
place in the file, and comes before any lake is opened, so nothing is
appended. A configuration written for a later build, holding a key this
build does not know, is refused the same way, since this build could
not apply it.

No event and no proposal holds a grant. Grants reach the store on a
call, from the resolver, and the log records only what they decided, so
strict types stop no old log folding. A host that keeps grants of its
own is refused the same key when it reads them through these types.

A grant under a type name no lake declares is told and starts. It cannot
be refused: a configuration may name a type before it is declared, and a
grant stands under a renamed type's old name, where it still holds. So a
process acting as a configured principal says so on standard error at
its start, at every start until a lake declares the name, naming the
principal, the type and the lakes it looked in. `serve` with its HTTP
door tells of every principal, over the lakes each addresses. `serve`
without a door and a command given `--as` have one lake open and act as
one principal, so they tell of that principal and look in that lake
alone, and a type only another lake declares is told there too. The
line does not tell an old name from a misspelling, because the list of
renames is not on the facade, and a line is not worth widening it for.
`verify` says nothing of this: it answers whether the log holds.

A field name under a type grant's `fields`, or under `hide_where[].fields`,
that the type declares in none of the lakes looked in is told the same
way, naming the principal, the type and the field, and is not refused for
the same two reasons: the declaration may come after the configuration,
and a grant stands under a renamed field's old name.

A key written twice in one of a grant's maps (`types`, `fields`, `only`,
`only_any`, `hide_where[].fields`) stops the root as an unknown key does,
naming the principal and the key, before any lake is opened. A map keeps
the last value of a repeated key, so a restriction written first would
drop without a word.


## Why

Roles are policy, and every organisation models them differently. If the
store understood roles, it would have to adopt one model and every host
would have to bend to it. Grants are the common denominator every role
model compiles to, and they are what the store actually needs to decide
a single read or write.

Enforcement has to sit in the store because a rule enforced anywhere
else is skipped by a caller that reaches the store directly. Per-field
grants are also what lets two writers share one record safely (see 012):
each writes only the fields it is granted.

Masking the answer does not protect a value the question already
revealed. A filter `salary > 100000` answers, record by record, whether
the hidden salary is above the line; a sum over a group of one is that
record's salary; an ordering ranks the hidden values. Each of these
turns a masked field into an oracle. Refusing the question is the only
answer that leaks nothing.

## What it is not

- It is not an off switch. There is no mode in which the store stops
  checking grants; a fully trusted principal is one whose grants cover
  everything.
- It is not authentication. Who the caller is gets decided before the
  store is called, by the resolver.
- It is not partial success. A write is never applied with the
  forbidden fields silently dropped.

## What moved

| what this record held | where it is now |
|---|---|
| a field has one access, over every name it bore and everything it is built from | `104 § A field has one access, the lowest over every name it has borne and everything it is built from` |
| a type is shown only where the grants give something of it, of what it is made of and of what it names | `104 § A type is shown only where the grants give something of it, of what it is made of and of what it names` |
| what is hidden answers at every door as what is not declared, and what is masked is named and refused as not granted | `109 § What is hidden answers at every door as what is not declared, and what is masked is named and refused as not granted` |
| a shown type's records may be found and got, and a probe needs the right to read what it probes | `105 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a description, a schema hash and a report's id are those of the shape the reader is shown | `104 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| a write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover | `106 § A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover` |
| a verb is callable and described only to one who may write what it sets and read what it reads | `107 § A verb is callable and described only to one who may write what it sets and read what it reads` |
| a declaration or a migration is its author's view | `106 § A declaration or a migration is its author's view` |
| a report is answered as its reader may know it | `108 § A report is answered as its reader may know it` |
| a proposal is listed as its reader's view, and one that names what its reader is not shown is not there | `108 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| an accept decides only what its reviewer is shown, and its refusal names only what the listing shows | `108 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a grant may admit only the records that hold its pairs | `109 § A grant may admit only the records that hold its pairs` |
| a grant may admit every record to read and only some to write | `106 § A grant may admit every record to read and only some to write` |
| a field may be hidden on only the records a condition selects | `109 § A field may be hidden on only the records a condition selects` |
| a record held at the head is not there to one without the grant, at any position and at every write door | `109 § A record held at the head is not there to one without the grant, at any position and at every write door` |
| a quarantined record is there as a proposal's write is, and its cells are read as their fields | `109 § A quarantined record is there as a proposal's write is, and its cells are read as their fields` |
| some things are still told, on purpose | `109 § Some things are still told, on purpose` |

## How it came to be

- The first version: a call carries one principal with grants per
  field, and the store enforces them on every read and write. In 013: a
  principal with `review` lists every proposal, any other its own. In
  024: a read that filters on, orders by or aggregates over a field
  needs the right to read it.
- 024 tightened `describe` in 0.4.0: a masked field by its shape alone,
  and no definition that names a field its reader may not read
  (lakelands#206).
- 013 added: a declaration or a migration can be proposed, and its
  accept takes `schema` beside `review` (lakelands#199).
- 024 added in 0.5.0: a search probes what it searches in every mode,
  and a vector what it is embedded from (lakelands#242); a search
  limited to named fields probes those (lakelands#236); a field its
  reader may not read is described without its annotations, and a
  relation's link field by its shape alone, which closed a gap:
  `describe` had answered each relation whole (lakelands#239).
- 013 added: a verb called on its type takes its type's grant
  (lakelands#277).
- 024 added: a read at `valid_at` probes the type's valid-time fields
  (lakelands#372); a write checks grants before it reads the data they
  protect, and the version right after the record is found
  (lakelands#343); a field built from another is read as that field
  (lakelands#387).
- Added: what `none` and `masked` each say, and the answer a probe or a
  write gets that names a hidden field (lakelands#394); `write_only`,
  which ended the one ladder `none < masked < read < write` that access
  was until 0.8.0, and gave a host that matches on the access enum one
  more arm (lakelands#393).
- 027 added: a shape is its author's view, 024 that a hidden field is
  not removed by one who may not know of it, and 013 that a proposed
  shape is listed to each reader without the fields it may not know of
  (lakelands#475); 027 that a migration's refusal names a reading
  declaration, and a view, only to an author who is shown it
  (lakelands#495, lakelands#504).
- 024 added: a view's own probe names nothing (lakelands#505).
- 013, 024 and 027 added: a proposal is listed as its reader's view,
  its patch, its payload, its shape and its report trimmed, and says
  `more`; before, `proposals` showed every reviewer and the proposer
  the write whole (lakelands#499).
- 013 added: a value type's tightening can be proposed, its refusal
  naming no misfit and its listing no use its reader may not read
  (lakelands#476).
- 027, 024 and 013 added: a report is answered as its reader may know
  it, by one function in five places, its sample keys those of records
  its reader is answered, and in 013 an accept refused as no longer
  lossless carries the report as its accepter may know it
  (lakelands#531).
- 024 added: a computed field is read as what it is computed from,
  listed or not (lakelands#540).
- 024 added: a declaration reads only what its author may read, a
  write's answer names a reaction to one who reads its condition, and
  a value type's report is shown as its listing is, where the preview
  of a tightened value type had listed every type and field that uses
  it, with its counts, and the refused apply named the first values
  that would not fit, to a caller that was shown neither the type nor
  the field; 027 that a type its author is not shown is a type that is
  not declared (lakelands#538).
- Added: a grant is followed through a field's renames, where one that
  stayed on the old name had let the field fall to the type's default,
  with 024's sentence on the same field under another name
  (lakelands#527); a type's names are followed too (lakelands#429).
- 024 added: a named set is read by its filter's right (lakelands#552).
- 027 added: a renamed reference field's entry in a report is answered
  as a conversion is (lakelands#524).
- 024 added: a field's declaration is shown by one list, wherever it is
  answered, where the `to` of a listing had handed out a masked field's
  members since before `restated` existed; 027 the rights of a report's
  `restated` (lakelands#554).
- Added: a proposal that waited is listed by the names it was written
  with (lakelands#594).
- 024 added: a setting the store reads is judged by what its writer is
  shown (lakelands#601).
- Added: a link type and a view are shown only with what they are made
  of, where each had been judged by the grants on its own name alone
  (lakelands#595); the first question of every door that writes a
  record, whether its writer is shown the type, in 024 with what was
  measured (lakelands#607); the raise by `sensitive` stopped counting as
  access to a type, which narrowed "raises an inherited `none` to
  `masked`" to one who is shown the type (lakelands#608); `may.find` and
  `may.get` are said of every type `describe` answers, where they had
  been said only of a type its principal reads a field of
  (lakelands#620).
- 051 was written: a grant may admit only the records that hold its
  pairs, and every write that reaches the type was refused
  (lakelands#634).
- 027 added: a read shares the shape of its range, and nothing of a
  reader is in what is shared (lakelands#557).
- 024 added: a schema hash is that of the shape its reader is shown,
  and a report's id is made over what is shown (lakelands#644); an
  expression is read as it is checked (lakelands#652).
- Added: a pattern's probes ask whether a type is shown (lakelands#653,
  lakelands#654); a relation is declared only to one shown both its
  ends, a link type is made of its relation's target too, a search that
  names no fields searches those its principal reads, in 024 the rest
  of a description is computed over the shown shape, and in 027 the
  author's view keeps a relation its author is not shown
  (lakelands#662).
- 051 added: a holder writes its own records alone and proposes writes
  to them, pinned, where every write that reached the type had been
  refused (lakelands#660).
- 027 added: a value type's migration lists what may name an old value
  to a reader shown it, and is not granted to one not shown every field
  it writes (lakelands#553).
- A key field held at `none` hides its type, where it had hidden the
  field and answered its value as the key (lakelands#715).
- 024 added: a relation carried by a field is walked by its reader only,
  a change for a host whose reader walked it before, and 027 that under
  the author's view a shape that sends a relation of a hidden one's name
  declares it twice (lakelands#727); 024 that a valid time over a field
  its reader may not know of is not named, and 027 that what a shape
  names stands as declared only where it stands, where a member that
  named a hidden field had been taken because the field existed in the
  lake, which told the author the name (lakelands#726); a database's
  error quotes no row (lakelands#735, lakelands#739); a hybrid search is
  judged before it embeds (lakelands#741).
- 006, 024 and 051 added: a reference to a type not shown is a field
  not known of, a field `only` names excepted; before, a reference's
  target was kept shown on purpose, so that a field's access would not
  depend on another type; and 027 that a reference field to a type its
  author is not shown is kept as a hidden field is, where an unchanged
  one stood as declared (lakelands#718). In 027: a reference an author
  sends to a type it is not shown is refused as to one that is not
  declared (lakelands#716).
- 027 added: a hidden name is judged at the step an absent one is
  (lakelands#750).
- In 024: a valid time sent where a hidden one stands, refused as
  taken, joined what is told on purpose; in 027: a valid time its
  author sends is judged by its name first, where one equal to the one
  in force stood as declared (lakelands#692).
- 013 added: a proposal that names a type its reader is not shown is
  not there, where 024 and 027 had listed it without its write
  (lakelands#762).
- Added: nothing beside a type names it to one it is kept from, so
  `describe` agrees with `call` and `propose` on a verb (lakelands#770).
- 051 added: an `only` that cannot be applied admits no record on any
  path (lakelands#771); a restricted record is not there to a write
  (lakelands#772).
- 024 added: a verb reads only what its caller may read, and 013 that a
  proposed call of a verb that reads a field its reader may not read is
  not there (lakelands#779). 013 added: an accept's refusal names what
  its reviewer's listing shows (lakelands#780). 051 added: a link with a
  held end is not there, and an accept is judged by what its write
  reaches (lakelands#781).
- Added: a view's key is judged at its sources too (lakelands#782), and
  a view declares no key but its groups (lakelands#788); a pattern's
  path on a type not shown is refused as one no bound name starts
  (lakelands#752).
- Added: a hold hides a record at every position, not only at the head
  (lakelands#790).
- The list of renames became the fold's recorded output `sys:renames`
  (lakelands#823).
- Added: one sight answers what a principal is shown, at every door;
  before, a reviewer who knew of a valid-time field and might not read
  it was listed the entry with no write at a valid time, and whole
  without one. In 024: a view's definition is shown to those its read
  answers, by one rule for `describe` and the read (lakelands#798).
- 024 added: a step with a `type` probes what it keeps, where a step
  whose `type` it does not reach had been refused as not granted when a
  relation of its name was masked, and an inverse refused as taken among
  what is told on purpose (lakelands#829); 024 and 027 what is told of a
  verb on a link type whose relation a migration removes
  (lakelands#843).
- The bundle's listing of a view whose definition its reader may not
  read got its row (lakelands#847).
- 013 added: an entry is decided as it is listed, by one function and
  up to three sights, and at an accept's refusal the proposer is judged
  as any reviewer (lakelands#791). Before, a bundle bringing a type its
  reader would not be shown was listed without it and its accept applied
  the whole; a call whose verb was declared again without what it read
  was listed whole; a migration keying its type by a hidden field was
  listed trimmed; and a holder of `only` was refused a migration's
  accept as not granted (lakelands#798, lakelands#791). In 024: a set
  named in another's filter is judged by the sets its author is shown,
  and in 027 an object set its proposer is not shown is not granted to
  propose (lakelands#877).
- 051 added: one answer of what a grant admits and one of whether a
  record is answered, a link with its ends. Before, what `only` admits
  of a type's records was worked out where it was asked: a read took the
  effective pairs and checked beside them whether the grant can be
  applied, a write asked the type's own pairs, and two sites took the
  bare pairs, where an empty list also meant every record; whether a
  record is answered was stated seven times, each judging the hold, the
  pairs and a link's ends by its own means, and some left a part out.
  Three doors judged a link by its record alone: a reference to a link
  whose end the writer's `only` does not admit was accepted; a proposed
  call on such a link was listed whole to a reviewer who is not answered
  the link, who could reject it though its accept was refused, and its
  accept ran for a verb that reaches outside the lake; a proposed call
  on a link with a held end was listed whole to a reviewer without
  `restricted`, whose accept and reject of it were refused; and a
  migration's report named, among its sample keys, a link with an end
  held from its reader and the links of a link type its reader is not
  shown (lakelands#798, lakelands#871).
- 051 added: an end that is a link is judged as that link is, where it
  was judged by its own record alone: a holder of `only` on a type two
  links down was answered the link onto another's link at every read,
  with `restricted` or without, and so was a reader without `restricted`
  the link onto the link of a held record; and the refusal of a second
  link named the other end of a link that `only` on the link type itself
  kept from its writer, whose `get` of that link was not found
  (lakelands#879).
- 024 added: a walk at a valid time probes every type it can meet
  (lakelands#831), and a view computed at a valid time its sources'
  valid time (lakelands#881).
- 027 and 024 added: a trigger, a verb, a function that computes a
  field and a tool's name that their author is not shown are not
  granted to declare, and no door leaves a lake with a problem among
  its tools' names, where a type declared alone with a relation whose
  link type's door bore another tool's name, or too long a one, was
  applied, and a migration that renamed a relation into one, and the
  lake then refused every declaration, naming the two, to anyone shown
  both (lakelands#893, lakelands#896); a writer's open is such a door
  and an import is not judged by this lake's doors (lakelands#893); 013
  that a trigger,
  a verb and an object set of a bundle name the type the one in force
  stands on (lakelands#893).
- 027 and 024 added: a report's sample keys are the first among the
  records its reader is answered; a collision is named only to one who
  is answered two or more of the records that would take its key and
  reads the new key, in the order of the new key, where a key was shown
  to whoever may see its type; `more` follows from the counts and the
  keys named beside them, where it said only that the report holds more
  than its reader is shown and was never set to one who may see
  everything; a value type's misfits are named of records their reader
  is answered and in fields it reads, and a tightening's report is
  named by what its reader is shown, where a tightening's id named
  misfits its reader was not shown; and a value type's report names a
  type by what its reader is
  described of it, where it had named a type the reader is shown for an
  expression in a member it is not (lakelands#875, lakelands#861).
- Merged at the rewrite (lakelands#817): the rules of 024 and 051, and
  the sections of 013 and 027 on what a principal is shown and told,
  moved here, and the title changed with them; the file keeps its name.
  The base rule of a held record is new: no record stated it.
- Corrected at the rewrite (lakelands#817): 013's "a reviewer without a
  read grant on the record still finds what it is asked to decide, and
  accepts it by its id" and its "decides it as before, what its grants
  hide of a record or a field included". A reviewer that is not
  answered the record is refused as not granted, as 051 said since
  lakelands#772.
- Corrected at the rewrite (lakelands#817): 013's "A reviewer shown
  every type it names decides it ..., what its grants hide of ... a field
  included" and its "a reviewer shown every type an entry names still
  applies the write whole, the part its listing trims included" held of
  a reviewer holding `only` on the type too. That one decides only what
  it is listed whole, as 051 said since lakelands#660.
- Corrected at the rewrite (lakelands#817): 024's "A write to a type
  the reader may not see is listed without its write", 027's "A
  migration of a type its reader may not see is listed without its
  write" and its "A reviewer may accept a migration of a type it may
  not see, by its id", with the report whose `type` is the empty name.
  Such a proposal is not there, and no report has a form with the type
  taken out (lakelands#762).
- Corrected at the rewrite (lakelands#817): 027's "the two schema
  hashes and the report's id stay whole" and "The id names the whole
  report ... the apply is refused or applied as for the whole report".
  The hashes are those of the shown shapes, and a migration's report id
  is made over what its reader is shown; the apply compares the
  caller's own id, and only whether the migration is lossless is decided
  as for the whole report (lakelands#644).
- Corrected at the rewrite (lakelands#817): 051's "takes its
  `restricted` mark, so the holder may not read its new record unless
  it holds that grant" and its "A proposal of each is judged as its
  write". A holder without the grant is refused the create onto a held
  key, as 051 said since lakelands#772, and a `link` and an `unlink` are
  not proposed.
- Corrected at the rewrite (lakelands#817): 013's "Accepting a proposal
  applies it under the proposer's grants as they were when it was
  proposed". It is applied in the proposer's name and not under its
  grant, which is not in the log, as 051 said.
- Corrected at the rewrite (lakelands#817): 024's "A proposal was asked
  when it was made: accepting it applies what was checked then". The
  accept asks its reviewer, as the listing does, and plans the write
  again as the proposer.
- Corrected at the rewrite (lakelands#817): 013's "A principal with the
  review grant lists every proposal and may accept or reject it" and
  "a reviewer only decides whether it happens". A reviewer lists what
  is there to it, a holder of `only` is judged by its own pairs, and
  one without `restricted` neither accepts nor rejects a write that
  reaches a held record.
- Corrected at the rewrite (lakelands#817): 024's "a function, trigger,
  object set or view that names a field the principal may not read is
  not described to it". The definition is not described; a view's type
  is listed with its definition withheld, a holder of `schema` is
  shown the definition, and a proposed bundle's listing, 013's "a view
  with its definition or not at all", leaves such a view out though
  `describe` would list its type.
- Corrected at the rewrite (lakelands#817): 024's "A read at `valid_at`
  ... probes them as a filter on them would" held of a bound its reader
  may not know of too. Such a bound is neither filtered by nor probed
  (lakelands#726).
- Corrected at the rewrite (lakelands#817): 024's "An author who may not
  know of such a field still names it, or sends the relation `describe`
  shows it". A hidden field's name answers as a name no field has, and a
  valid time left out stays as declared (lakelands#726).
- Corrected at the rewrite (lakelands#817): "a view over a view over a
  link type is shown where the type that declares the relation is"; and
  where the relation's target is (lakelands#662).
- Corrected at the rewrite (lakelands#817): 027's "A relation the
  migration removes is listed under `dropped` by its own name, to one
  who sees the type"; and is shown the relation (lakelands#662).
- Corrected at the rewrite (lakelands#817): 027's "so `accept` asks no
  one's rights", true only of completing the shape, and its "the one
  who accepts, who holds the `schema` grant and is shown every trigger,
  set and view", true of those whose type that accepter is shown, as
  024 said of a set since lakelands#877.
- Corrected at the rewrite (lakelands#817): 013's "A link's cardinality
  is judged only where a link is written, and no proposal writes one".
  A proposed call of a verb that sets fields of a link type writes a
  link's own fields; the rule is 013's, and this record points at it.
- Corrected at the rewrite (lakelands#817): "The listing of proposals,
  and the accept and the reject that go by it, alone make more than
  one". Other doors make more than one sight too, among them a write
  that waited while the types moved, whose sight is made a second time
  under its lock, as the rule says; a write door for a principal whose
  grants hide something, first over no declarations and then over the
  head's shape; and a search as of a position, at the head beside its
  own.
- Corrected at the rewrite (lakelands#817): 013's "one who reads every
  field of every type is judged the same there as in force, so an
  owner's listing reads no past". It is shown every type alike, but a
  migration's names are judged for it on the type in force, and a value
  type's migration that renames values is listed to every reader as
  the lake stood before it.
- Corrected at the rewrite (lakelands#817): 051's "A reviewer without
  `restricted` is refused, as not granted, the accept of a write that
  reaches a held record" named the accept alone; `decides` refuses the
  reject as it refuses the accept. And its list of the write doors named
  neither a `link` onto a held link record nor an ingest onto a live
  held key, each refused as an upsert onto the key is, nor a change of
  shape, which names no record and converts a held one with the rest.
- Corrected at the rewrite (lakelands#817): 013's "A retirement's
  refusal names only the record it retires ... so neither is judged". A
  retirement's refusal at the accept is told as its reviewer's listing
  shows the write, as an upsert's and a call's are.
- Corrected at the rewrite (lakelands#817): 024's "A write is governed
  by the principal's write grants" left the verb out: a verb is callable
  only to one who may read what it reads too (lakelands#779;
  `Sight::callable`).
- Corrected at the rewrite (lakelands#817): 013's "one listed without
  its write ... is not granted to decide" held of a record its reviewer
  is not answered and of a holder of `only`; a bundle listed without its
  write to a reviewer without `schema` who holds no `only` is that
  reviewer's to reject.
- Added: a field is asked once a pass of the walk that answers its
  access, where it was asked on every way that led to it and a type of
  forty derived fields that each read the later ones was declared and
  then answered no read; and fields that list each other are read no
  further than each other, all the way round, where a cycle was answered
  by the way the walk came, leaving out the fields it had passed: of two
  fields listing each other, one masked and one its reader writes, the
  first was shown masked and is hidden now; no answer is wider
  (lakelands#926).
- A verb is its caller's only where it reads every field its paths read
  across a reference; what a proposed call reaches and pins gains the
  records it reads across; and a proposal that names a record its
  reviewer is not answered beside its own is not there to that
  reviewer, where it was listed and its accept refused as not granted
  (lakelands#567, lakelands#929).
- A verb whose path steps through a relation's link is its caller's
  only where the caller holds `restricted` and no `only` on the link
  type or its end types (lakelands#567).
- 027 added: what a caller not shown a relation or a reference field
  that refers to a removed relation's link type is told of it, among
  what is told on purpose (lakelands#920).
- A proposal whose own record or a link's end is held from a reviewer
  without `restricted` is not there to it, where it was listed without
  its write and its accept and reject refused as not granted
  (lakelands#1008).
- A proposal on a record a holder's `only` does not admit is not there
  to that holder, where it was listed without its write and its accept
  and reject refused as not granted (lakelands#1016); a retired record
  of another's that is not held is a free key to it, as to its writes.
- A proposal whose call or derived value would leave its record outside
  a holder's `only` is listed to that holder in part, its accept and
  reject not granted, where it was listed whole and its reject taken
  (lakelands#914).
- A read the engine fails, where the store judges whether a key may be
  named, answers the outage, where it was taken for a hold: the listing
  left the entry out or listed it, and a report named no key
  (lakelands#927).
- A report's entry for a link type's end, `<relation>.to` of a retarget,
  was left out for every reader, so a reviewer shown everything was
  listed the proposal with `more` and its accept was not granted; it is
  answered as the link type's field (lakelands#1092).
- `describe` listed every value type to every principal, its `one_of`
  and `constraint` included, and a proposed migration of one was listed
  to every reviewer; a value type is now listed by the fields its reader
  is shown that use it, or to a holder of `schema` (lakelands#1088).
- 070 added: a verb declared `granted` is callable and described only to
  a caller whose grants on its type name it (lakelands#1287).
- Extended: a type described as of a position is told its names since
  only where its reader is shown it at the head too (lakelands#1291).
- A proposed migration, of a type or of a value type, was refused by a
  restricted record that it would leave breaking a constraint that
  refuses, of its own type or of another whose references a re-key
  rewrites; it is now judged by what its proposer is told of, and the
  accept judges every record.
- Changing the lake's types (`enact`, `declare`, `apply`, `add_field`,
  `recompute`, `migrate_apply`, `migrate_value_type_apply`, and `accept`
  of such a change) needs `restricted` beside `schema`, refused `grant_needed`
  whatever the lake holds; to a caller without it, a change's plan and
  preview are judged as it is told, as a proposal's are (062).
- A verb names its conditions as `preconditions`, and a refusal of one
  names it (lakelands#1330).
- Under a vector index an engine's candidates are its reader's class's:
  a held record is no candidate of one without `restricted`, and a
  holder of `only` is ranked exactly over its pairs (078,
  lakelands#1254).
- A field may be hidden on only the records a condition selects
  (lakelands#1412), in the shape the operator accepted on
  lakelands#1400: a grant had given a field one access on every record
  of its type, so a host hid on every record a field it had to hide on
  some.
- Extended: a search as of a position reaches a field a condition hides
  on a record only where its reader is shown it there at that position
  and at the head; it had been judged at the head alone
  (lakelands#1456).
- A grant may admit every record to read and only some to write
  (lakelands#1490): `only` had been the one way to keep a principal to
  some records, and it kept the reads to them too.
- Dropped: the clause that a holder owns both ends where a computed
  field of the other end's type reads across the relation (lakelands#1506);
  the declaration checks refuse every such field, so it could not be shown.

- At lakelands#1768 the rules of a read, a probe, a write, a call, a
  listing and an acceptance moved to records of their own (104 to 109),
  word for word; this record keeps the decision and the table above.
