# REST

The HTTP door serves every lake of the root twice, from one catalog:
MCP at `/lakes/<lake>/mcp`, and REST under every other path of
`/lakes/<lake>/`. Both are configured by the `server` section of the
root's `lakelands.json`, take the same API keys, and resolve them through
the same resolver to the same principals. `lakelands serve --mcp` writes
the address it listens on to standard error as
`lakelands: listening at http://<address>/ (starting)` once the door is
bound, and as `lakelands: serving HTTP at http://<address>/` once every
lake is open, so a `bind` on port 0 takes a free port and still says
which.

A REST route only translates. Its path, its query, its headers and its
body become the arguments of one door, under the names that door's MCP
tool takes, and the catalog answers exactly as it answers the tool. What
a door does is stated once, in the contract and the conformance suite;
this page says only how a request reaches it.

## Every request

- **The key.** `Authorization: Bearer <key>`. Every request is resolved on
  its own, before anything about the routes is answered: without a key
  that resolves for the lake, every path under a lake answers `401`;
  once a key is presented, a lake segment holding a `/` answers `404`
  before that key is resolved. There is no session and no pin: a read that must see one state
  names `as_of` itself, and a page's `cursor` already carries the position
  its first page was read at and the clock that page read. Over REST the principal is what the server
  resolves from the key: nothing a client says about who it is or what
  it may do is taken.
- **The position.** Every reply to a caller whose key resolved carries
  `Lakelands-Position`: the position the answer was read or written at,
  or, for a failure, the head of the log when it was answered. A JSON
  answer that is not a failure also carries it as `position`; a failure
  carries it in the header only.
- **Failures.** A failure answers one of the contract's closed set of
  errors, as JSON, with the two sentences a person reads beside it:
  `what` happened and what `todo`. A request the server cannot read (an
  unknown parameter, a body that is not JSON, no such route, a
  declaration member it does not know, at any depth) is `refused` as
  `invalid`; a missing or unknown key is `refused` as
  `not_granted`.

  ```json
  {"error":"conflict","expected":1,"actual":2,
   "message":"the record changed: expected version 1, found 2",
   "what":"the record changed: expected version 1, found 2",
   "todo":"read the record again and retry against its current version"}
  ```

  | Error | Status |
  |---|---|
  | `refused`: `not_granted`, `grant_needed`, `wrong_door`, `verbs_only`, `reserved`, `declined` | 403 (401 for a missing or unknown key) |
  | `refused`: `invalid`, `constraint`, `precondition`, `too_deep`, `too_large`, `migration`, `out_of_range` | 422 (400 for a request the server cannot read, 404 for no route, 405 for a method the route does not take, 413 for a body over 4 MiB, 415 for a body of another media type) |
  | `refused`: `decided`, `unanswered` | 409 |
  | `not_found` | 404 |
  | `conflict` | 412 — a stale `If-Match`; `If-None-Match: *` on a key that exists (`expected` is 0, the message says the key already exists); an accept whose proposal's record moved since it was made |
  | `unbound` | 501 |
  | `unavailable` | 503 |

- **Editing a record.** A record's version is one number for the whole
  record, not one per field: a write that changes any field moves it. A
  read of one record answers it as `ETag`. So does a `PUT` or `DELETE`
  of a record, and its `ETag` is that record's next `If-Match`: a client
  that edits twice sends it without reading in between. A link or an
  unlink answers the version of the link record, not of either end, so
  it is not the next `If-Match` of the record it was made from. A verb
  answers the version of the record write it made, where it made one,
  and none otherwise.

  Two people editing different fields of one record therefore collide
  by design: the second write answers 412. A client that edits only its
  own fields can retry safely:

  1. Read the record again with `with_meta`.
  2. Compare the `meta.position` of each field the client edits with the
     position it read those fields at.
  3. If none of them moved, send the write again with the new `ETag` as
     `If-Match`; the other person's fields are kept, because a `PUT`
     changes only the fields it names.
  4. If one moved, the client's value is stale: show the current value
     and let the person decide.

  `If-None-Match: *` creates a record and refuses an existing key with
  the same 412, whose `expected` is 0.

- **The link between hosts.** A key listed under `server.hosts` is a
  host's, not a principal's: its request names the principal it is made
  as in `Lakelands-Principal`, by id, and may carry the write context in
  `Lakelands-Context`, as JSON. A missing principal, or a context that is
  not one, is `refused` as `invalid` (400); a principal the host may not
  act for, or one the lake does not know, is `not_granted` (403). A
  principal's key that sends either header is `refused` as `invalid`
  (400): no caller sets a write context or acts as another. A process
  that does not hold the lake's lease answers a host's request
  `unavailable` as `read_only`, so the host tries the next writer
  (`docs/hosting.md`).
- **Browsers.** A request whose `Origin` is not on the configuration's
  `allowed_origins` is refused before anything else is read.
- **The lake.** The lake's id is one path segment; one that decodes to
  hold a `/` names no lake.

## The routes

`…` stands for `/lakes/<lake>`. A path segment is percent-decoded, so a
key may hold any character.

| Route | Door | Arguments from |
|---|---|---|
| `GET …/types/<type>/records` | find | query: `where`, `order_by`, `limit`, `cursor`, `offset`, `total`, `retired`, `after`, `aggregate`, `with_meta`, `params`, `as_of`, `valid_at` |
| `GET …/types/<type>/records/<key>` | get | query: `with_meta`, `params`, `as_of`, `valid_at` |
| `PUT …/types/<type>/records/<key>` | upsert | body: the fields to change; `If-Match`, `If-None-Match`, `Idempotency-Key` |
| `DELETE …/types/<type>/records/<key>` | retire | `If-Match`, `Idempotency-Key` |
| `PUT …/types/<type>/records/<key>/links/<relation>/<to>` | link | body: the link's own fields (none is `{}` or no body); `If-Match`, `If-None-Match`, `Idempotency-Key` |
| `DELETE …/types/<type>/records/<key>/links/<relation>/<to>` | unlink | `If-Match`, `Idempotency-Key` |
| `POST …/verbs/<verb>/<key>` | a verb declared on a record | body: its payload (none is `{}`); `If-Match`, `Idempotency-Key` |
| `POST …/verbs/<verb>` | a verb declared on its type | body: its payload (none is `{}`); `Idempotency-Key` |
| `POST …/query` | the pattern query | body: the query tool's arguments |
| `POST …/cypher` | an openCypher statement, run as the pattern it translates to | body: `statement`, `params`, `cursor`, `as_of`, `valid_at` |
| `POST …/search` | search | body: the search tool's arguments |
| `GET …/describe` | describe the lake | query: `as_of`, `valid_at` |
| `GET …/describe/<type>` | describe one type | query: `as_of`, `valid_at` |
| `GET …/proposals` | proposals | query: `state`, `type`, `kind`, `limit`, `order_by`, `cursor`, `as_of`, `valid_at` |
| `POST …/proposals` | propose | body: `write` (`upsert`, `retire`, `call`, `declare` with `declarations`, `migrate` with `migration`, or `records` with `type`, `where` and `to`), `why` |
| `POST …/proposals/<id>/accept` | accept | — |
| `POST …/proposals/<id>/accept_planned` | accept a proposed change to the ontology, or to records, against the plan of its change at the head; a records proposal is accepted only so | body: `plan_id` |
| `POST …/proposals/<id>/reject` | reject | body: `reason` (optional) |
| `POST …/schema/declare_type` | declare a type | body: `type` |
| `POST …/schema/declare` | declare anything that does not reshape stored records | body: `declaration` |
| `POST …/schema/apply` | declare a set together | body: `declarations` |
| `POST …/schema/add_field` | add a field | body: `type`, `field` |
| `POST …/schema/recompute` | compute again what a declaration in force computes | body: `function` or `type` |
| `POST …/schema/migrate` | a migration's preflight | body: `to`, `renames` and `relation_renames`, or `value_type` |
| `POST …/schema/migrate/apply` | apply a migration | body: `to`, `renames` and `relation_renames`, or `value_type`; and `report_id` |
| `POST …/schema/plan` | a change's plan: what it would do, changing nothing | body: `change` (`declare` with `declarations`, `add_field` with `type` and `field`, `recompute` with `what`, `migrate` or `migrate_value_type` with `migration`, or `records`, below) and its members |
| `POST …/schema/enact` | make a change against its plan | body: as `…/schema/plan`, and `plan_id` |
| `POST …/ingest/<source>` | ingest | body: NDJSON, one record per line; query: `delivery` |
| `GET …/feed` | the change feed | query: `from`, `kinds`, `types`, `key` (repeated); `Last-Event-ID` |

A parameter or a body member the door does not take is refused before
anything runs, so a misspelling never turns a compare-and-set into an
unconditional write. `params` is the exception on the two reads: on a
type that is not a view with parameters the store refuses it itself, as
`invalid` (422), saying the type declares none.

A verb declared on its type (`scope: type`) is called without a key, on
`POST …/verbs/<verb>`: it has no record, so no record version either,
and `If-Match` or `If-None-Match` there is `invalid` (400), naming the
header and the verb. Either route with the other kind of verb is
`invalid` (422), naming the verb: a key to a verb on its type, or none
to a verb on a record.

The link doors (`link`, `unlink`,
`docs/decisions/035-a-link-is-a-record.md`) are served under the record
a link starts from, `…/types/<type>/records/<key>/links/<relation>/<to>`,
as over MCP they are the `<type>_<relation>_link` and `_unlink` tools.
`PUT` writes the one link of the pair, and writing it again changes its
fields; `DELETE` removes it, and its history stays. A link is a record
of the link type, so the version `If-Match` names is the link's.
- **Refusals:**
  - a `<to>` that names no live record of the relation's target is a
    `constraint` on `to` (422);
  - a link the relation's cardinality does not allow (a second live link
    where it allows one) is a `constraint` on `to` (422);
  - a relation the type does not carry by link records, whether it is
    carried by a field or not declared, is `invalid` (422);
  - a relation whose links the caller may not write is `not_granted`
    (403);
  - unlinking a pair with no link is `not_found` (404).
- **Reading links:** use `find` on the link type,
  `GET …/types/sys.link.<type>.<relation>/records`.

`describe` answers a type as the principal may see it. A host's
`annotations` on a type, a field, a relation and a value type come back
as they were declared, in canonical form. A field the principal may not read, a
relation's link field included, is described by its shape alone, without
its annotations. An annotation grants nothing and restricts nothing:
what a principal may read is decided by its grants alone
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended). A key without a namespace is refused as `invalid` (422), and
one in `sys.` or `lakelands.` as `reserved` (403), naming the key.

`cypher` is REST's alone, and the CLI's `lakelands query --cypher`: no
MCP tool takes a statement. The statement, in the openCypher subset
`lakelands-cypher` reads, with the values of its `$name`s as `params`,
is translated to a pattern and run as `POST …/query` runs it; its
answer is that route's. How many rows a page holds is the statement's
`LIMIT`, so the body takes no `limit`; `cursor` continues a page.
A statement that begins with `EXPLAIN` answers `{"pattern": …}`, the
pattern it translates to with `as_of`, `valid_at` and `cursor` set,
and reads nothing. A statement that does not parse, a construct outside
the subset, a condition nested more than 32 levels deep, and a parameter
missing, unused or of a kind its place does not take are `invalid`
(422), each by its own case, with the character offset where it has
one. A member the route does not take is `invalid` (400), naming it.

`schema/declare` and `schema/apply` are REST's alone: MCP offers
`declare_type`, since an agent authors types, while a program may
declare verbs, triggers, object sets and the rest. Each body member is
the declaration as the contract writes it, and a member the route does
not take is refused.

`schema/plan` and `schema/enact` are REST's too; MCP's tools that change
the ontology take `plan` and `plan_id` instead. A plan, and an answer
that carries one (a change's outcome, a proposal's, a listed proposal),
comes with `said`: a sentence per finding, for the person who decides
on it (`docs/decisions/062-a-change-plans-before-it-acts.md`).

The same two routes carry a change to the records a condition selects
(`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`):
`{"change": "records", "type": …, "where": …, "to": …}`, with `to` either
`{"set": {field: value, …}}`, the same values on every record selected, or
`"retire"`. `schema/plan` answers the plan with a `records` section
(`type`, `selected`, `sample_keys`, `fields` and `refused`) and changes
nothing; `schema/enact`, with the plan's `report_id` as `plan_id`, makes
exactly that change in one append, and is refused `plan_stale` (`invalid`, 422) if a
selected record changed or the selection moved, and `too_large` (422) past the
lake's `max_change_records`. A records proposal is filed on
`POST …/proposals` as `{"write": "records", "type": …, "where": …, "to": …}`
and accepted only on `…/accept_planned` with the `report_id` of the plan
of its change now; `…/accept` of it is refused `plan_stale`. No route
offers `only`.

A trigger's `max_depth` caps the reactions to its firings: writes a
host makes naming a firing as their cause
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`). A
request through REST or MCP is never one, since a door's caller sends
no cause, so the host meets that refusal, not the doors; said through
a door it would be `too_deep` (422), naming the trigger in `field`. A
`max_depth` above the lake's `max_reactive_depth` is refused at
`schema/declare` as `invalid` (422), naming the trigger and both
numbers.

### Query parameters

A query parameter carries the value the door's argument of that name
takes. `limit`, `offset`, `as_of` and `from` are whole numbers;
`with_meta`, `total` and `retired` are `true`, `false`, or present
without a value for `true`; `aggregate` is
JSON, and so is `after`, the sort position a page resumes strictly after
without a cursor: `after={"values":["2026-10-01"],"key":"inv-7"}`,
URL-encoded like `where` and every other query value, and read at
`as_of` when given and at the head otherwise. `order_by` is the JSON
array of sort keys, or the short form: fields separated by commas, `-`
before a field for descending, and `known_at(field)` or
`seen_at(field)` to order by a field's provenance, and `latest()` to
order by the position of each record's latest change —
`order_by=-due_date,known_at(status)`, or `order_by=-latest()` for the
most recently changed first. `kinds` and `types` are words
separated by commas.

`params` is the JSON object of the values a read binds to a view's
parameters, one member per parameter, every parameter bound:
`params={"anchor":"2026-06-30"}`. A `date_time` given as a date alone is
midnight UTC, so `date <= param('anchor')` leaves out the rest of that
day. A `date` parameter takes a date, or an instant at a UTC midnight. A pattern started on a view binds them as `start.params` in the
body of `POST …/query`. A parameter missing, unknown or of the wrong
kind is `invalid` (422), naming the view and the parameter.

### Writes

`PUT` sends a sparse patch: an absent field is unchanged, `null` clears
it. `If-Match: <version>` (the `ETag` a `GET` of the record answered, with
or without its quotes) makes a write a compare-and-set; a weak validator
(`W/"7"`) names no one version and is refused. `If-None-Match: *` asks
that the record not exist yet; it is `PUT`'s alone, and `DELETE` or a
verb that sends it is refused. `Idempotency-Key`
makes a retry of the same write a no-op that answers the first outcome.
A retry of a call of a verb that reaches outside the lake answers its
executor's recorded answer (`"output": null` when it answered `null`);
while none is recorded it is `409` `unanswered`, and runs nothing
(`docs/seams.md` §`VerbExecutor`).

A write answers its outcome: `200` with `{"outcome":"applied",…}`, or
`202` with `{"outcome":"proposed","proposal":…,"why":…}` when it waits
for review.

### Ingest

`POST …/ingest/<source>` takes `application/x-ndjson` (or
`application/jsonl`, `application/json`, or no `Content-Type`), one
source record per line (`{"type":…,"key":…,"fields":{…},"seen_at":…}`),
and is read as it arrives. A line may name its own `"source"`; one that
does not is the path's. One call so carries the batches of many sources,
each record's event stamped with its own. The answer is NDJSON too: one line per commit
window as it becomes durable,
`{"position":…,"records":…,"duplicates":…,"delivery":…}`. `?delivery=`
names the call's delivery, which every record's event carries; without
it the store names its own, `d` and the position of the call's first
event, and a window before any event of the call names none
(`docs/log-format.md` §1.1). `?sealed_apart` seals the call's values
of erasable fields under a key of their own too, so that taking back
the delivery shreds it alone
(`docs/decisions/065-part-of-a-source-may-be-taken-back.md`). Where a window is cut is
the store's, and no answer turns on it: a record is judged over the
lake and the records before it, and a refused one is refused alike
wherever the cut fell. What the cut decides is only the refusal's
`from`, whether the records of its window before it were written; a
host resumes there. While a window takes
longer than fifteen seconds to commit, the answer carries an empty line
every fifteen seconds, so a client that holds the answer to a silence
bound knows the server is working; a client skips it. The reply's
`Lakelands-Position` is the head before the ingest began; each window's
own position is in its line. A line is at most 4 MiB before its line
feed, however the body is cut into reads. A line that is longer, or is
not a record (a member a source record does not have included, so a
misspelled `"source"` is never read as the path's), ends the ingest
with a last line naming it (`invalid`); what came before it is
committed, and since ingest is content-addressed, sending the whole
body again appends nothing twice. A record the store
refuses ends the ingest with a last line that is its refusal, its own
`error` code and shape kept, with `index` (its place among the body's
records, from 0), `from` (the first record of its window: every record
before it is committed, none from it on) and `record` (its `type` and
`key`) beside it; send the body again from record `from`, the refused
one left out or mended. A refused ingest is
answered even while the body is still arriving: the server reads the
rest of the body and drops it, for at most fifteen seconds, before it
closes, so a client that sends on past the refused line still reads
the answer.

### The change feed

`GET …/feed` answers `text/event-stream`: every event of the log from
`from` (0 when absent; past the head's next position it is refused as
`out_of_range`) that matches `kinds`, `types` and `key`, then each new
one as it is appended, until the client goes away. `key` is given once
per key, since a key may hold a comma, and never empty; with `types` it
answers those records' events (`types=invoice&key=INV-1` is one
record's history), without them a key in any type (`docs/seams.md`
§`ChangeFeed`). Each event's `id` is
its position, its `event` its kind, its `data` the event as JSON; a
reaction the batch names `overruled` carries the comment `: overruled`
in its frame, after its `id`, which a reader that does not know the
mark skips as it skips every comment
(`docs/decisions/004-the-change-feed-is-the-log.md`). A
reconnecting client sends `Last-Event-ID` and resumes after it. Each
time the stream catches up at a position it has not named before, it
sends the comment `: next <position>`: the first position not yet
examined, the `next` a poll of the change feed would answer, so a client
that reads what is there and stops knows where to stop. A quiet stream
sends the comment `: keep-alive` every fifteen seconds. A caught-up
stream waits for the lake's next append and sends it at once; the
configuration's `feed_poll_millis` (250 unless stated) bounds that wait,
after which it reads the log again, and `feed_floor_millis` (10 unless
stated) is the most often the log is read for it, counted from the end
of the last such read, so a busy lake's appends reach it in batches
(lakelands#1061). The streams and MCP sessions that follow one lake with
the same kinds and types share one read of the log; a stream's `key`s
are applied to it by the stream (lakelands#1113).

The feed is the log itself, unmasked, so only a principal that reads
every field of every record, withheld records included
(`Grants::reads_everything`), may open it
(`docs/decisions/037-the-feed-over-a-network-door.md`).

## Health and readiness

Two routes answer outside any lake, at fixed paths, for a supervisor or a
load balancer. They take no key and no `Origin` is checked; they carry no
lake name, count or position, only `{"status": "<word>"}`; they read a few
flags each lake holds in memory, with no fold and no scan; and they are
not tools, so the catalog does not list them. `HEAD` is answered like
`GET`; any other method answers `405`.

| Route | Status | `status` | Meaning |
|---|---|---|---|
| `GET /livez` | `200` | `up` | The process is up and the door answers. |
| `GET /readyz` | `200` | `ready` | Every lake it serves is open, holds its writer lease as its handle knows it, and answers its head. |
| `GET /readyz` | `503` | `not_ready` | A lake's handle does not hold its lease, or does not answer its head. A lease another writer took shows from the first write it refuses, on every engine (see below). |
| `GET /readyz` | `503` | `starting` | The lakes are not all open yet, for instance while the open of an upgrade rebuilds a projection. |

`lakelands serve` binds the door before it opens the lakes, so `/livez`
answers `200` and `/readyz` `503 starting` throughout an open. A lease
lost to another writer, taken over the handle or gone with a PostgreSQL
session, shows only once the engine has refused the handle something for
it: an append, at a write that appends, or a rebuild's swap. Until then
`/readyz` can still say `ready`; from then it says `not_ready` until a write
takes the lease again. Nor does `/readyz` compare the projection's head with the end of
the log; an open does not return before its fold is done, so a lake that is
open has folded what the log held then.

## A client in Rust

`lakelands-client` is a lake over this door: `RemoteLake` is the
contract's `LakeStore`, its catalog and its feed each door's route, so a
program holds a remote lake as it holds one in its own process. It is
given the base URL (`http` or `https`, without user-info), the lake, one
API key per principal id it may act as, and the principal whose key
reads the feed. It reads no file but the certificate bundle it is
named, and nothing from the environment.

- A call as a principal sends that principal's key; a principal the
  client holds no key for is refused before anything is sent.
- The feed is read as server-sent events until the limit, at most
  `FEED_EXAMINED` events as a store's poll answers, or until the
  stream says `: next`. The server reads on, a bounded poll at a time,
  and says `: next` only at the head, so a poll over the wire reads to
  its limit or to the head and may examine more than `FEED_EXAMINED`
  events. A stream silent for two keep-alives is
  `unavailable`. A stream that keeps talking, in keep-alives or empty
  lines, is not silent and holds a poll open until its limit; the
  caller's own timeout is the bound for that. An ingest is held to the
  same silence bound: while records are sent, when the server stops
  taking them (time spent waiting on the source never counts), and once
  they have all been sent, when its answer goes quiet. Only a client
  that ran calls an ingest's answer silent: a silent server is refused
  at exactly the bound, and a client that did not run for a while (a
  suspended process, a stalled machine) first hears the server for one
  more quarter of the bound. One stop it cannot tell from listening:
  one that begins in the last quarter of the bound and ends within a
  quarter after it. So a client stopped for up to just under half the
  bound (15 s of 30 s) can still take a working server for silent; a
  longer stop, or one that began earlier, cannot.
- The client reads an ingest's answer while it sends, at least every
  512 KiB of records, so a window's line, or a refusal, reaches the
  caller while the upload flows and not when it has ended. When the
  answer's stream ends, the upload ends at its next record: the source
  is not asked again, the body ends with its last chunk, and the source
  is dropped at the connection's next turn. That holds for every
  ending: a line that carries an error, an answer that went silent, was
  cut off or grew too long, and a caller that drops the stream; and for
  an ingest refused with a status before any line. A refused ingest
  therefore sends a few hundred KiB past its refusal, not the rest of
  its source.
- No answer is held past `max_answer_bytes`: a whole answer, one event
  of the feed or one line of an ingest's answer that grows past it is
  `unavailable`, and the client stops reading.
- What a request cannot carry is refused as `invalid` before anything
  is sent: an empty key, type or other name, which as an empty path
  segment would name another route, and an idempotency key holding a
  line break.
- The host's own operations (`verify`, `rebuild`, `restrict`,
  `unrestrict`, `checkpoint`, `age`, `purge_source`, `compact`,
  `position_at`) are no door's, and are `unbound`.
- Its `limits()` are the ones it was configured with, not the server's:
  the server keeps its own, and its refusals win.
- It reads the declarations of its own version or older. A declaration
  in an answer that carries a member the client does not know is
  refused, naming it, never dropped
  (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended), so
  upgrade the client with the server or before it. A client from before
  annotations refuses a described declaration that carries
  `annotations`, naming the member.
