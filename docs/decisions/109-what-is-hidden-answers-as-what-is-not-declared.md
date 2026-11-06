# 109 — What is hidden answers as what is not declared, and some things are still told

These rules are what is hidden and what is still told: what `none`,
`masked`, `only`, `writes_only`, a record held at the head, a quarantined
record and a field hidden on some records answer at every door, and the
few things a description or a refusal still tells on purpose.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

### What is hidden answers at every door as what is not declared, and what is masked is named and refused as not granted

The two grants that withhold a value say different things. `masked`
tells the principal that the field exists and withholds its value:
the field is listed as masked, described by its shape, and a read that
probes it or a write that carries it is refused as not granted, naming
it. `none` says the field does not exist for that principal. It is
absent from answers, from the masked list and from metadata, from
`describe` and from every generated tool. A probe or a write that names
it gets the answer for a field the type does not declare: the same
kind, the same sentence, the same field named. Any other answer would
let a single refusal reveal what every answer hides.

#### One sight answers what a principal is shown, at every door

Whether a type, a relation or a declaration is there for a principal was
asked at every door by the door's own means: a read described the type,
a write asked the type's rule, a listing built the whole description, and
the call door and `describe` each held a rule of their own for a verb.
Each rights finding of the rounds before closed one door, and its
verifier then found a sibling that answered the same question another
way. So the question has one place: a sight of the door's principal over
the declarations in force at its position, which every door asks: `get`,
`find`, `search`, a pattern and each of its hops, `describe`,
`describe_type` and the tools made of them, `upsert`, `retire`, `link`,
`unlink`, `call`, `propose`, `accept`, a listing, a migration's
preflight and a refusal that would name a declaration. The sight is
made of the principal the door is called with: its grants are held
through the renames, and its references to types it is not shown are
held at `none`. The listing of proposals, and the accept and the reject
that go by it, make more than one: of the lake in force, of the lake as
an entry would leave it, and of the lake as the entry was proposed
against, each of the reader as its call named it, its grants followed
there (108). Following comes first, since a grant held through a
rename may hide what the grants as given do not. A write judges under
its lock with the principal its call was made with. Where the types
moved between its sight and its lock, as only a migration the writer
catches up with as it is taken moves them, its sight is made a second
time, over the head's types, and the write is refused to be sent again
where the head hides other references from it than its call was shown;
one seen before types moved that hide nothing more from it is applied.

What is shown does not turn on a read's valid time. A read at `valid_at`
filters records by the type's valid-time fields, so the read that
filters probes them, and is refused where its reader may not read one
(105). A listing of proposals filters no record by them: a proposed
write to such a type is listed at a valid time as without one, and the
accept that follows judges it as the listing did.

The doors are held together by one row: for each of a cast of principals
and each type of a lake, a type `describe` names is at no door a name
that is not declared, and one it does not name is at every door exactly
that: at a read, a write, `link` and `unlink`, an ingest, a call, a
proposal and a migration's preflight. The accept and the reject are
held to the listing by a second row, a grid of proposed entries and
reviewers (108).

#### A database's error quotes no row

An engine's error reaches the caller as `unavailable`, and the network
doors answer it whole. PostgreSQL adds a `DETAIL` to many errors, and
for a failed constraint it quotes the failing row: a record's key and
its values, which the caller may not be shown. So the PostgreSQL engine
answers only the severity, the SQLSTATE and the server's primary
message, which names a table, a constraint or a column and no value;
`DETAIL`, `HINT`, `WHERE` and the statement position stay out of the
answer. SQLite's own messages name the columns of a failed constraint,
or the schema's own check, and never a row's values, so the SQLite
engine answers a failure SQLite reports by its extended code and that
message. Its driver's own errors do quote: a statement that does not
parse is answered with its text, and a stored value that does not fit
its type with the value. Of any such error the SQLite engine answers
only its kind.

### A grant may admit only the records that hold its pairs

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
  records. Nor does any other path, whatever a stored value holds: the
  proposal listing answers a proposal on such a record without its
  write, a reference into the type, a link's end too, names no live
  record, as an absent key does, and a write on a record of the type is
  one on another's. A value equal to the pair would otherwise tell which
  records hold a field the holder may not read. Two names of one field
  that a rename joins with differing values hold null, which no pair may
  hold.
- **Every read.** `get` of another's record is not found, as a missing
  one is. `find` with a filter, an order, `total`, a count, a sum and a
  group read the engine under the pairs, so they count and fold the
  holder's records alone; its count of the retired counts the holder's
  retired records alone, another's being a key that is free to it
  (lakelands#1024). A search ranks the type and keeps the admitted
  candidates, pulling deeper until its page is full; under a vector
  index it is ranked exactly over the records that hold its pairs,
  never streamed from a graph that holds others; the fused search's
  path that reads no record is not taken. A walk reaches only admitted
  records. A proposal on a record the holder is not admitted is not
  there to it (below). One that would leave a record outside the
  holder's pairs is listed without its write (above); it still shows
  why, the proposer and a type-level verb's payload. A read as of a
  position judges the
  records as they stood then, under the grant followed to that position.
- **A search as of a position** keeps a candidate only where it is
  admitted then and at the head too, under the grant followed to the
  head and by the name the type bears there: the ranking reads the text
  and vectors at the head, and another's current text must find nothing.
  A grant that cannot be applied to the head's declaration admits no
  candidate there, though the stored values stay. A cursor page and a
  query started by a search keep working.
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
  walk too. An end that is itself a link is judged as that link is.
  Readers holding no `only` on any end, however deep, are not judged by
  this rule.
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
- **A holder proposes writes to its own records, pinned to them.** Its
  `propose` of an upsert, a retirement or a call, and a call of a verb
  above low risk, which is a proposal, are judged as the direct write
  is, against the record it owns at the key. The proposal is logged
  `pinned` where the holder holds `only` on the type it writes, a link
  type with a held end included (`docs/log-format.md` §2.4). The accept
  of a pinned proposal is refused as invalid, the proposal left pending,
  once the record at its key has an event after the proposal's position,
  live or retired, or, for a link, either end record has one (a link is
  its holder's by its ends, and a change to an end moves no link), or,
  for a call, a record it reads across a reference has one (055), or
  the type was declared or migrated since, a re-key included: a rederive
  and a backed output move the record without a write, and a record made
  again at the key starts its versions anew, so the position, not the
  version, is what the accept compares. So the accept applies the write
  to the record and type its proposer was judged on. The proposer's
  grant is not in the log, so a holder may propose only where nothing
  but the write can move its pairs: a proposal is refused where a field
  its `only` names is derived or backed, or where the verb assigns one.
  A reference a pinned proposal writes is judged at the accept as a live
  record, as for any proposal: it names a key the proposer knew when it
  proposed. The accept writes in the proposer's name, not under its
  grant, and the pin compares records and types, not grants: a grant
  narrowed or revoked between the proposal and its accept does not stop
  a pinned proposal. The reviewer decides it, on what the listing shows,
  and accepts it only where that is the whole (lakelands#867).
- **A reviewer holding `only` decides what it is shown whole.** A
  proposal whose record at the key its `only` does not admit before the
  write, another's, is not there to it, as one on a held record is not
  (lakelands#1016): not listed, and its accept and reject answered as
  an id never made, decided or not, to the byte, so a proposal id,
  which is positional and so can be guessed, tells it nothing. A key
  whose record another retired, and which is not held, is to it a free
  key, as to its own writes. Of the rest it accepts or rejects only
  what the listing shows it whole: the record the write would leave
  admitted, planned as the accept plans it, its derived values computed
  and a call's assignments made (lakelands#914), and every value the
  write carries one it may read. One it is listed in part, its own
  record handed to another by an upsert, a call or a derived value, a
  free key made another's or a value it may not read, is refused as not
  granted, its accept and its reject alike, before it is told whether
  the proposal is decided: what its listing leaves out is a record that
  would be another's, which its reject would decide as surely as its
  accept, as its own write would be refused. Every reviewer
  accepts only what it is listed whole (above, lakelands#867). So the
  listing tells a free key, where a write is listed in part, from
  another's record, where the same write is not listed: as a write
  tells it on purpose (what a holder's write tells, below).
- **What is refused**, as not granted, naming no field: an aging rule on
  the type, which moves records at the host's run. A holder of `only` on
  any type changes no shape: every schema door and the proposal of a
  declaration or a migration are refused to it, and no migration's
  proposal, of a type or a value type, is listed to it, since a report
  counts every owner's records; its accept and its reject answer as an
  id that does not exist, as for any entry that is not listed (above): a
  refusal as not granted would tell that the id is a proposal's. A
  proposed declaration is listed and not granted to decide. A reference
  written into the type from another type must name an admitted record.
- **`describe`** tells the holder its pairs in `may.only`, where the
  grant can be applied, and offers no write, link or verb that would be
  refused: an upsert only where it writes every field its `only` names
  at the authored door, so that a create can succeed, and no verb above
  low risk that it may not propose. A field's `initial` value is not
  counted there: where only an `initial` value would make a new record
  the holder's, the write is admitted (the record it leaves is judged,
  `initial` values included) but no upsert is offered. The change feed
  is the log unmasked, so a holder does not read everything and is
  refused it at a door.

`only` is a function of `Principal.id` within a lake: a host gives each
owner its own id and never gives one id `only` on some calls and not on
others. A retry is answered its first outcome before the grants are
asked, as every retry is; under this contract a holder's earlier call
was refused, and its retry answers that refusal.

**Why.** A host often needs "each sees only their own": an employee
their claims, a customer their invoices. Filtering above the lake works
record by record but not for a sum, a group, a search, a walk or a view,
so such doors had to be refused instead of answered. The store reads
every record through two places, the engine's scan and the check of a
record read by key, and both take the pairs: the engine selects only
admitted records, so every answer built from a scan sees only them, and
no door filters after the fact.

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
accepts it is answered for as for its own write.

#### A grant may admit a set of values, and refuse a part read as the whole

A grant may hold `only_any` beside `only`: for a field, a set of text
values. A record is admitted where its field holds one of them, beside
every pair of `only`, as `only` admits it, at every door above: the
engine reads under the set as it reads under a pair, as one of several
values to the same index, and no door filters after the fact. `""` is
an ordinary value. The set closes as the pair does: it names a `text`
or `reference` field the holder may read, or the grant is not applied;
an empty set admits nothing; a field named both in `only` and in
`only_any` is a grant that cannot be applied, since the two would say
two things of one field; a rename that joins two names keeps the values
both sets hold. `describe` tells the holder its sets in `may.only_any`
(lakelands#1588). A proposal made under a set is accepted only by a
reviewer whose own pair or set on the field holds every value of it: a
reviewer of one of the values would otherwise apply a change to records
of another it does not see.

**Why.** A host often admits a principal to the records of several
owners, a manager to a team's, and a grant of one value cannot say
that; a host that filtered above the lake would lose the sums and the
searches again. A set fails closed for the reason a pair does: a set
the store guessed at, an empty one read as no restriction or a clash
with `only` resolved to either side, would admit records the host did
not name, and the host is told at once instead.

A grant that admits only some records of a type may hold
`whole_sets_only` there. A read computed over a set of the type's
records then refuses as invalid, naming the type: a count, `total`, the
count of the retired, a sum, a group, a pattern's aggregate, and any
view that reads the type, kept or not, at any depth of its sources. A
link type whose end the grant narrows is narrowed with it, so a count
of its links or a view over them refuses alike, and so does a graph
measure, `degree`, `components`, `page_rank` or `communities`, over a
graph that meets the type, as its start or at an end of a step's links,
since a record's measure is read off the records and links around it
(lakelands#1608). A
read that answers records, `get`, `find` and a search, is answered as
without it. On a type the grant does not narrow it changes nothing
(lakelands#1590).

**Why it is opt-in.** A sum over the admitted part is the right answer
for a holder who knows its grant is a part, its own claims, and the
store has answered so since the pair was added; turning it off by
default would refuse reads hosts rely on. Where a figure over a part
can be mistaken for one over the whole, an agent told "the total is 35"
reporting it as the team's, the host says so on the grant, and the
store refuses rather than answering a part as though it were the whole.

#### `only` keeps the reference it names

A reference to a type its principal is not shown is a field it may not
know of (104). A field the effective `only` of a type names is
kept as granted: its value is the pair the host wrote into the grant, so
it tells the holder nothing, and `only` stays one that can be applied.
The effective `only` counts, a view's included: a view grouped by its
source's `only` field keeps that field, and so its key, and does not
vanish (above). The type a grant names, and the value it holds a field
to, are the host's own choice: naming a type in a grant shows its name
to no one.

#### One answer says what a grant admits, and one whether a record is answered

What `only` admits of a type's records, and whether a record is
answered: each question has one answer, in the sight (above).

What a grant admits is every record, the records that hold its pairs, a
view's records computed from the admitted ones and held to its pairs, or
nothing. A grant that cannot be applied admits nothing, and no reading
of the answer makes every record of an empty list or of a missing one: a
door reaches the pairs through the answer alone. A record is answered
where it is not held from the principal (below) and its grant admits it,
and a link with its ends: each end whose type the principal holds `only`
on is admitted, retired or not, and no end is held from it. A read, a
write of the record, a reference to it, a relation's cardinality, an
aging rule in a rename's report, the listing of a proposed write, with
the accept and the reject that go by it, and a migration's report ask
these.

Three doors judge a link as a read of it does. A reference to a link
whose end the writer's `only` does not admit is refused as one to a link
no record holds is. A proposed call on such a link is not there to a
reviewer who is not answered the link, as one on a link with a held end
is not there to a reviewer without `restricted`: not listed, its accept
and reject answered as an id never made (lakelands#1008,
lakelands#1016). A migration's report names, among its sample keys, neither
a link with an end held from its reader nor the links of a link type its
reader is not shown, and its counts stay whole (108).

A relation may point at a link type, and a link of it has an end that is
a link. An end that is a link is there where that link is, by its own
ends, as deep as they go: a link's key is made of its ends' keys, so the
descent ends with the records. Whether a link type is judged by its ends
at all is asked the same way through its declarations, each link type
once, since a migration can point two link types at each other. A write
that names such a link follows it as far: a second link where a relation
allows one names the first only where its writer is answered it with its
ends; the records a proposed write reaches, by which its reviewer's
decision and its proposer's `restricted` grant are judged, are a link's
ends and theirs; and a pinned proposal on such a link is accepted no
more after a change to any of them.

**Why.** A judgement stated twice is two rules once one of them
changes, and the one left behind hands out what the other keeps: each
of the three doors was a copy with a part missing. An answer that fails
closed cannot be skipped: a site that asks nothing has no pairs to
filter by, and so no record to hand out.

### A field may be hidden on only the records a condition selects

A principal's grant on a type may hold `hide_where`: hidings, each a
field `G` of the type, a set of text values, and the fields `F` it
lowers, each to `none` or to `masked`. A record is selected where its
`G` holds one of the values, compared as `only` compares a pair. On a
selected record each `F` has the lower of its type-wide access and the
hiding's; on every other record the type-wide access stands. A hiding
only lowers an access, never raises one. The operator accepted its
shape on lakelands#1400 (2026-10-25) with the five consequences below;
it is in force from 0.20.0 (lakelands#1354).

- **The condition is read only where its holder may read it.** `G` is a
  field of kind `text` or `reference`, which the holder reads at least
  `read`, not computed at the read, and lowered by no hiding of the
  grant. `F` is no key field, no valid-time field, no field the grant's
  `only` names, and not `G`; the type is no view. A grant that breaks
  this is not applied and not guessed at, as a grant whose `only`
  cannot be applied is not:
  the doors that name the type refuse it as invalid, naming the type
  and never the field, and a walk or a view that reaches the type reads
  every `F` as hidden on every record, so it fails closed. A condition
  on a field its holder may not read would, applied, tell it which
  records hold which values of that field.
- **A hiding follows its fields' renames,** `G` and every `F`, as every
  grant does (above).
- **One rule: every read is evaluated over the record as its reader is
  told it.** On a selected record `F` and everything built from it, a
  derived or computed field and a vector embedded from it (above), are
  absent or masked, as declared, and every door reads that record so.

| what reads | on a selected record |
|---|---|
| `get`, `find`, `count`, an aggregate, a pattern query | `F` is not answered. A `where`, an order, a group or a measure over `F` reads it as null. The store evaluates them over the records as told: a filter or an order that reads `F` is not handed to the engine, one that does not still is. |
| a lexical search | `F` brings no match and no rank, and the text statistics a ranking reads are counted over the text as told (025). |
| a vector or hybrid search | a vector built from `F` brings no candidate. The reader has no class of its own and is ranked exactly over what it is told, as a holder of `only` is (078); a hybrid search fuses what each axis tells. |
| a probe | probing `F` is allowed, since the holder reads it on other records; it reads null on a selected one, so it tells nothing of it. |
| a view | a view that reads `F` is computed for this reader from the records as told, never read kept; a set over such a view is this reader's only where nothing it reads is held from it (066). |
| a derived relation, a walk | an edge derived from `F` does not exist on a selected record (044). |
| a proposal | listed as told; a reviewer not told a proposal whole is refused its accept, as above. |
| a subscription | the names of the fields a change moved are told per record, not per type. |
| the change feed | refused: a principal with a hiding does not read everything. |
| an export, a backup | unchanged: they take no principal and are the host's. |
| a write | `F` on a record selected before or after the write is not writable: the write is refused whole, not granted, naming `F` (012), also to a writer that may write `F` on every other record. Its answer names no trigger and no object set whose condition reads a field a hiding of the writer lowers (024); `through` still covers what it does not name (015). A verb that reads `F` is refused not granted on a selected record, its own or one it reads across a reference, whether its `requires` holds or not (lakelands#1463). |
| a read as of a position | a record is selected by its `G` at that position; a search ranks by the head's indexes, and a ranking reaches `F` on a record only where its reader is shown `F` there both at that position and at the head: a record selected at either, by its key and names there, is neither matched nor ranked by `F` (025). |
| `describe` | `F` keeps its type-wide access, and its description tells the holder where it is lowered, `G` and the values, as `may.only` tells a holder its pairs; the schema hash follows what is described. |

What it costs and what it tells, accepted with it:

1. A reader a hiding narrows reads the type, wherever a read touches
   `F`, by a scan in the store: no engine pushdown and no vector index.
2. Its holder learns which records hide `F`: it reads `G`, and
   `describe` tells it the values.
3. The change feed is refused to it.
4. `F` is not writable on a selected record, whatever the type-wide
   grant gives.
5. The condition is equality to a fixed set of values, and nothing
   richer.

**Why.** A host keeps the text of documents of many kinds in one type,
one record a chunk, each naming the record its document was filed
under. A kind that hides its text from a reader has to hide it on its
chunks too, and with a field's access per type the host hid it on every
chunk, which took every other document's text out of that reader's
search. A hiding keeps the field where the condition selects and leaves
the rest of the type as the grant gives it. The rule that a read is
evaluated over the record as told is the one a narrowed sight already
keeps for a reader told a record its own way; it makes one judgement of
what the record holds, so no door can answer from a value another door
hid.

### A record held at the head is not there to one without the grant, at any position and at every write door

A retention hold withholds a record without erasing it. The host places
one through the store facade's `restrict`, with its reason, and lifts it
through `unrestrict`; no caller's door writes either
(`docs/log-format.md` §2.3). To a principal without the `restricted`
grant a record under a hold, a restricted record, is not there: `get`
does not find it, as a missing record is not found, and `find`, a
`total`, a count of the retired, a view, a walk, a graph and a search
leave it out, retired or not (lakelands#1024). To
a principal with the grant it is answered with its fields unchanged, its
history included, and lifting the hold brings it back for every reader,
at every position. A hold is never inferred from a read the engine
fails: where the store judges whether a key may be named (a proposal's
listing, its accept and reject, a migration's report, a cardinality
refusal), an engine that cannot read answers its outage, as at every
other door (lakelands#927).

What the other records say of a held record follows from that. An
answer that only tells, a proposal or a listing, says whether a change
is lossless by what its reader is told of: a held record that the
change would leave breaking a constraint that refuses makes it no
lossier than a retired one, which the change does not judge, so twins
that differ only in which of the two it is are answered alike. The
accept judges every record and refuses there
(`docs/decisions/062-a-change-plans-before-it-acts.md`). An
engine's full-text and vector candidates include restricted records, and
the store applies the principal's grants to the hits
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`),
so one without the grant is never answered it as a hit, and it moves no
other hit's rank or score. Under a vector index the candidates are its
reader's class's: one without the grant is streamed from a graph that
holds no record held at the head, so a held record is no candidate of
it, and shapes neither its pages nor the depth they are read at
(`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`). A restricted link is walked only by a
principal holding the grant
(`docs/decisions/035-a-link-is-a-record.md`). In a view the fold keeps,
a restricted record is a member for a reader with the grant and for no
other (`docs/decisions/049-a-view-the-fold-keeps.md`). The change feed
is the log unmasked, and a principal without `restricted` is refused it
at a network door before the stream starts
(`docs/decisions/037-the-feed-over-a-network-door.md`).

The suite holds the rule in `a_restricted_record_is_withheld_but_kept`
(`get`, without the grant and with it, and the hold lifted),
`a_count_answers_as_many_records_as_a_full_find_walk` (a count),
`a_retired_or_restricted_record_is_never_a_hit`,
`a_record_the_read_may_not_see_moves_no_hybrid_score` and
`an_axis_counts_only_what_its_reader_is_shown` (a search), and
`a_restricted_link_is_walked_only_with_the_restricted_grant`,
`degree_counts_each_records_links_to_ends_it_may_see_highest_first` and
`a_graph_shows_a_reader_no_more_than_its_rows` (a walk, a degree and a
graph). Two hold it by comparison, while a record is held: in
`a_page_answers_its_total_and_an_offset_jumps_to_the_walks_slice` a
`total` equals what its reader's own walk finds, and in
`a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` a
kept view answers a reader as its computed twin does. That a holder of
the grant reads a held record's history is asserted in
`a_record_held_across_a_rekey_is_not_there_as_of_before`: its `total` as
of before the re-key is two above the reader's, the two held records.
The parts below are held in
`a_record_held_now_is_not_there_at_any_position`,
`a_record_held_across_a_rekey_is_not_there_as_of_before`,
`a_restricted_record_is_not_there_to_a_writer_without_the_grant` and
`a_link_with_a_held_end_is_not_there_and_an_accept_is_judged_by_what_it_reaches`.

#### The hold that stands at the head governs every position

A retention hold is about the record, not the moment. To a principal
without the `restricted` grant, a record held at the head is not there
at any position: a read as of an earlier position answers it as a record
that never existed, at `get`, `find` (its rows, `total` and aggregates),
a view computed or kept, a walk, a pattern and a search, lexical, by
vector or both, whose candidates, ranks and counts leave it out. A
record held then and released since is answered there as any record is,
since the hold that stands now is the one that governs.

A read as of a position by such a principal reads through a view that
gives each record its hold at the head, asked of the head one page of
keys at a time. A record is found at the head by the name and the key
it bears there, after each rename of its link type and each re-key
since, and one the head does not hold keeps the hold it had: a record
is never released by not being found. Every door then judges the hold
as it does at the head. A page the engine fills with records held now
is read on until it is full, a total is counted in pages, and a kept
view, folded as it stood then, is computed instead, and `describe` says
it is not kept here. A read at the head, or by a principal with the
grant, reads through no such view.

**Why.** A hold withholds a record from those without the grant. A read
as of the moment before the hold would hand it back whole, so a hold
that cut only the head would withhold nothing. A record released from a
hold is one its readers may see, and the history before its release is
part of it.

#### A restricted record is not there to a write

A writer that may not read a restricted record must not change it, point
at it, or learn its version. So the record is, to such a writer, not
there at any write door, as another's record is to a holder of `only`. A
reference to it and either end of a `link` are refused `not_live`, as
for a key no record holds; `retire`, a verb called on it, and `unlink`
of a link with a restricted end are not found; an upsert onto its key is
refused as not granted, naming no field, so the key is told as a create
tells it and the held record is neither written nor its version shown; a
`link` onto a link whose own record is held and an ingest onto a live
held key are refused the same way. A retired record that is held counts
as another's too, so no upsert, ingest or proposal writes into it, and
an ingest judges a link's ends as `retire` does. A proposal of an
upsert, a retirement or a call is judged as its write. Its accept judges
the write as the records stand then. The proposer's grant is not logged;
a proposal that reached a record restricted when it was proposed could
only be a holder's, and is accepted as one; any other is judged without
the grant, so a record restricted since the proposal is not there to its
accept.

A change of shape names no record: a migration, a value type's
migration and a declaration convert, re-key and rename every stored
record of the type by the fold, a held one with the rest, whoever makes
them (`027 § A migration converts history too`); of a held record their
reports give its count, and its key to no reader without the grant
(`108 § A report is answered as its reader may know it`).

#### A link with a held end is not there, and an accept is judged by what its write reaches

A link with an end its reader is not answered is not answered either, at
`get` and `find` of the link type, at a reference to it and at its
writes: a link from or to a held record is, to a reader without
`restricted`, a link no record holds. A view over a link type is
computed for such a reader, as for a holder of `only`: no kept face
judges a link's ends. A relation's cardinality still holds over every
link, whoever can see it: a refusal for it names the other link's end
only where the writer is answered the link that holds: by its ends,
under `only` and `restricted` alike, and by its `only` on the link type
itself, from either end; so what is kept stays whole before it stays
hidden.
What a proposed write reaches is the record it writes, a link's two end
records, and the records its references name: an upsert's, those its
record computes (032), and a call's as its assignments compute them for
the record at the accept, whatever form they take, with the records its
`requires` and set values read across a reference (055), as when it was
proposed and as at the accept; and the two end records of each link
among them. A proposal whose own record or ends its reviewer is not
answered, held from one without `restricted` or another's to a holder
of `only`, is not there to it, and nor is one that
names a record its reviewer is not answered beside its own (a reference
its upsert sends or computes, a record its call reads across, and their
ends where one is a link), held when it was proposed or since: not
listed, its accept and reject answered as an id no proposal holds, to
the byte, as one whose verb reads a field its reviewer may not read is
(lakelands#1008, lakelands#1016). A retired record that is not held is
a free key here, as to a write. This is asked of a reviewer without
`restricted` and of one holding `only`, by the one function that
computes the reach. The
accept judges a proposer as a holder where its write reached one when
proposed, computed wholly as it stood then (the declarations, its
record, the settings in force and the proposal's time): a record held, a
field retyped, a setting changed or a time passed only since is no proof
of the grant. A call above low risk is judged as its caller's write
before it is proposed, as `propose` judges it. The host places and lifts
a hold on a retired record too: a key whose retired record is held is
otherwise closed to every writer without the grant for good.

### A quarantined record is there as a proposal's write is, and its cells are read as their fields

From lakelands#1502 (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`):
a quarantined record is listed and dismissed by a holder of `ingest`
(`not_granted` otherwise), and is there to its reader as a proposal's
write is: not where the reader is not shown its type, not at a key whose
record is restricted to a reader without `restricted`, and not, to a
holder of `only`, where the record its cells would leave is not
admitted. One not there is `not_found`, as one that never was.

- **A cell is read as the field it names**, through every rename since,
  with the access the reader's grants give that field and `sensitive`'s
  raise: shown, named in `masked` with its value withheld, or not there
  nor named where the field is `none`. A cell that names no field of the
  type, or one a redeclaration removed, is read by the type's default
  access; one whose field was `sensitive` when it came keeps the raise.
- **A hiding** lowers a cell where it would lower the field on either
  record the row touches: where the `G` cell as sent holds one of its
  values; where the live record at the type and key, at the head, is
  selected; and where the `G` cell is there but is not a text or
  reference value. A row whose `G` cell is absent, at a key no live
  record holds, is not selected.
- **The refusal** is shown whole to a reader shown every name it holds,
  and to any other as its kind alone, `error` and the reason's `code`,
  with `more: true`; a `not_found` is `not_found` alone.

This is the one word of this record for it: a quarantined record is not
a held one, which here is a restricted record.

### Some things are still told, on purpose

What remains says a hidden thing exists, and remains on purpose:

- the version, which moves on every change, a hidden one included
  (104);
- the key, the valid times and a relation's `via`, which are the shape
  and are given of a masked field too. A key field the reader may not
  know of hides the type instead (104), since the key would give its
  value. A valid time naming such a field is not named: `describe`,
  `describe_type`, a listed proposal and a migration's report give the
  type without that bound, and a read at a `valid_at` answers as without
  it (105);
- a valid time sent where one the author may not know of stands, which
  is refused as taken, as a field named like a hidden one is (106). A
  name no field has, or a hidden one, sent there answers as on a type
  with no valid time: only a field the author may name tells it, as only
  a taken name does;
- an inverse sent onto a type where a relation the author is not shown
  bears it, which is refused as taken
  (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
  An author, a proposer without `schema` and a reviewer at an accept
  learn a yes or a no for a pair of inverse and target, and neither the
  relation's name nor its owner;
- a verb on a relation's link type, to a caller who is not shown the
  verb, kept from the link type or not, where a migration it sends or
  accepts removes the relation. The removal is refused
  (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`), in a
  case of its own that names the relation and the link type its name
  gives, and names the verb to an author `describe` shows it to, one who
  may call it. A caller who is not shown the verb, one kept from the
  link type itself among them, is answered the same case without the
  verb: that a declaration is declared on the link type, at the
  preflight, the apply, a proposal and, as a reviewer, the accept. The
  refusal must say why the migration cannot go. The relation is one the
  caller is shown, and the link type's name follows from the relation's.
  Since only a verb refuses a removal, the caller learns that a verb
  stands there; it learns neither the verb's name, nor how many there
  are, nor what one sets or requires;
- a relation or a reference field that refers to a removed relation's
  link type, to a caller who is not shown it, by the same rule: the
  removal is refused
  (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`) in a
  case of its own that names the relation removed and its link type,
  and names the relation or the field that refers, placed at its type,
  to an author shown it. A caller kept from its type, or from the link
  type and so not shown it, is answered the same case without it: that
  a declaration refers to the link type. It learns that one stands
  there, and neither its name, its type, nor how many there are
  (lakelands#920);
- `may.retire`, which says every field is writable.

**What stays.** A principal with `schema` is shown every definition, and
a write answers `through`, the position its consequences reach. One who
is shown a condition and sees that a write reached further than itself
can infer that the condition held, where nothing else explains it. The
lake answers no name and no value; it cannot hide that something reacted
from one who must read its own write's consequences. So `schema` is not
handed to a principal that is kept from a field or a type
(`docs/hosting.md`). Every definition is a condition, a set's filter and
a view's definition, told whole at `none` and masked alike. A verb is
not one: `describe` lists it as a call, to one that may make it, with or
without `schema`; and a field derived from one its reader may not know
of is itself a field it may not know of, its expression with it
(lakelands#928).

**What a holder's write tells.** A create succeeds on a free key and is
refused on another's: a key's existence shows where a write would make
something, and a key that carries the owner (the owner field among the
key fields) keeps holders' keys apart. A create on a key whose record
another retired continues that record's version, and takes its
`restricted` mark; a holder without that grant makes no record on a
held key (above). `ingest` judges a link's cardinality only once the
record its writer leaves is admitted: a link from another's record
answers as one from a free key, and a link a grant's `only` on the link
type admits by its own fields holds the relation's cardinality as any
other.

**What a record a caller is not answered tells.** It is accepted under
`restricted` as under `only`: a cardinality refusal and an accept's
refusal each say that a record the caller is not answered exists.

**A value type's renames count every record.** Their counts stay whole,
as a report's totals do (108): a field's count per pair holds the
hidden and restricted records too, so it tells a caller how many records
it is not answered hold a value.

## Why

006 states why the store decides a call on grants rather than roles, why
the check sits in the store, and why masking an answer does not protect a
value the question already revealed (`006 § Why`). The reasons particular
to these rules are stated with them, above.

## What it is not

006 states what no rule of grants decides: it is not an off switch, not
authentication, and not partial success (`006 § What it is not`).

Of `only`:

- It is not a predicate other than equality: several admitted values
  for one text field are `only_any`'s (above), and nothing else
  compares.
- It is not taken from `Principal.id` implicitly.
- It does not change `restricted`: a record must pass both rules.
- A reference value naming another's key is shown as a key; the record
  behind it is not found, as for a restricted record.

Of a field hidden on the records a condition selects:

- It is not a condition over several fields: one field `G` selects.
- It is not a predicate: equality to one of a set of values, as `only`
  compares a pair, and nothing else.
- It is not a path: `G` is a field of the type's own record, never one
  read across a reference or a relation.
- It is not a raise: a selected record's field is never read further
  than the type-wide grant gives.

## How it came to be

- These rules were stated in 006 and changed there from its first version;
  lakelands#1768 moved them here word for word. Their history, oldest
  first, is that record's (`006 § How it came to be`).
