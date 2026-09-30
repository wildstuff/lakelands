# REST

The HTTP door serves every lake of the root twice, from one catalog:
MCP at `/lakes/<lake>/mcp`, and REST under every other path of
`/lakes/<lake>/`. Both are configured by the `server` section of the
root's `lakelands.json`, take the same API keys, and resolve them through
the same resolver to the same principals. `lakelands serve --mcp` writes
the address it listens on to standard error as
`lakelands: serving HTTP at http://<address>/`, so a `bind` on port 0
takes a free port and still says which.

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
  its first page was read at. Over REST the principal is what the server
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
   "what":"the record changed: expected version 1, found 2",
   "todo":"read the record again and retry against its current version"}
  ```

  | Error | Status |
  |---|---|
  | `refused`: `not_granted`, `wrong_door`, `reserved` | 403 (401 for a missing or unknown key) |
  | `refused`: `invalid`, `constraint`, `precondition`, `too_deep`, `too_large`, `migration`, `out_of_range` | 422 (400 for a request the server cannot read, 404 for no route, 405 for a method the route does not take, 413 for a body over 4 MiB, 415 for a body of another media type) |
  | `refused`: `decided` | 409 |
  | `not_found` | 404 |
  | `conflict` | 412 — the version expected came in `If-Match` |
  | `unbound` | 501 |
  | `unavailable` | 503 |

- **Browsers.** A request whose `Origin` is not on the configuration's
  `allowed_origins` is refused before anything else is read.
- **The lake.** The lake's id is one path segment; one that decodes to
  hold a `/` names no lake.

## The routes

`…` stands for `/lakes/<lake>`. A path segment is percent-decoded, so a
key may hold any character.

| Route | Door | Arguments from |
|---|---|---|
| `GET …/types/<type>/records` | find | query: `where`, `order_by`, `limit`, `cursor`, `after`, `aggregate`, `with_meta`, `as_of`, `valid_at` |
| `GET …/types/<type>/records/<key>` | get | query: `with_meta`, `as_of`, `valid_at` |
| `PUT …/types/<type>/records/<key>` | upsert | body: the fields to change; `If-Match`, `If-None-Match`, `Idempotency-Key` |
| `DELETE …/types/<type>/records/<key>` | retire | `If-Match`, `Idempotency-Key` |
| `PUT …/types/<type>/records/<key>/links/<relation>/<to>` | link | body: the link's own fields (none is `{}` or no body); `If-Match`, `If-None-Match`, `Idempotency-Key` |
| `DELETE …/types/<type>/records/<key>/links/<relation>/<to>` | unlink | `If-Match`, `Idempotency-Key` |
| `POST …/verbs/<verb>/<key>` | a declared verb | body: its payload (none is `{}`); `If-Match`, `Idempotency-Key` |
| `POST …/query` | the pattern query | body: the query tool's arguments |
| `POST …/search` | search | body: the search tool's arguments |
| `GET …/describe` | describe the lake | query: `as_of`, `valid_at` |
| `GET …/describe/<type>` | describe one type | query: `as_of`, `valid_at` |
| `GET …/proposals` | proposals | query: `state`, `type`, `kind`, `limit`, `order_by`, `cursor`, `as_of`, `valid_at` |
| `POST …/proposals` | propose | body: `write` (`upsert`, `retire`, `call`, `declare` with `declarations`, or `migrate` with `migration`), `why` |
| `POST …/proposals/<id>/accept` | accept | — |
| `POST …/proposals/<id>/reject` | reject | body: `reason` (optional) |
| `POST …/schema/declare_type` | declare a type | body: `type` |
| `POST …/schema/declare` | declare anything that does not reshape stored records | body: `declaration` |
| `POST …/schema/apply` | declare a set together | body: `declarations` |
| `POST …/schema/add_field` | add a field | body: `type`, `field` |
| `POST …/schema/migrate` | a migration's preflight | body: `to` and `renames`, or `value_type` |
| `POST …/schema/migrate/apply` | apply a migration | body: `to` and `renames`, or `value_type`; and `report_id` |
| `POST …/ingest/<source>` | ingest | body: NDJSON, one record per line |
| `GET …/feed` | the change feed | query: `from`, `kinds`, `types`, `key` (repeated); `Last-Event-ID` |

A parameter or a body member the door does not take is refused before
anything runs, so a misspelling never turns a compare-and-set into an
unconditional write.

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
`annotations` on a type, a field and a relation come back as they were
declared, in canonical form. A field the principal may not read, a
relation's link field included, is described by its shape alone, without
its annotations. An annotation grants nothing and restricts nothing:
what a principal may read is decided by its grants alone
(`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended). A key without a namespace is refused as `invalid` (422), and
one in `sys.` or `lakelands.` as `reserved` (403), naming the key.

`schema/declare` and `schema/apply` are REST's alone: MCP offers
`declare_type`, since an agent authors types, while a program may
declare verbs, triggers, object sets and the rest. Each body member is
the declaration as the contract writes it, and a member the route does
not take is refused.

### Query parameters

A query parameter carries the value the door's argument of that name
takes. `limit`, `as_of` and `from` are whole numbers; `with_meta` is
`true`, `false`, or present without a value for `true`; `aggregate` is
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

### Writes

`PUT` sends a sparse patch: an absent field is unchanged, `null` clears
it. `If-Match: <version>` (the `ETag` a `GET` of the record answered, with
or without its quotes) makes a write a compare-and-set; a weak validator
(`W/"7"`) names no one version and is refused. `If-None-Match: *` asks
that the record not exist yet; it is `PUT`'s alone, and `DELETE` or a
verb that sends it is refused. `Idempotency-Key`
makes a retry of the same write a no-op that answers the first outcome.

A write answers its outcome: `200` with `{"outcome":"applied",…}`, or
`202` with `{"outcome":"proposed","proposal":…,"why":…}` when it waits
for review.

### Ingest

`POST …/ingest/<source>` takes `application/x-ndjson` (or
`application/jsonl`, `application/json`, or no `Content-Type`), one
source record per line (`{"type":…,"key":…,"fields":{…},"seen_at":…}`),
and is read as it arrives. The answer is NDJSON too: one line per commit
window as it becomes durable,
`{"position":…,"records":…,"duplicates":…}`. While a window takes
longer than fifteen seconds to commit, the answer carries an empty line
every fifteen seconds, so a client that holds the answer to a silence
bound knows the server is working; a client skips it. The reply's
`Lakelands-Position` is the head before the ingest began; each window's
own position is in its line. A line is at most 4 MiB before its line
feed, however the body is cut into reads. A line that is longer, or is
not a record, ends the ingest with a last line naming it (`invalid`);
what came before it is committed, and since ingest is content-addressed,
sending the whole body again appends nothing twice. A refused ingest is
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
reconnecting client sends `Last-Event-ID` and resumes after it. Each
time the stream catches up at a position it has not named before, it
sends the comment `: next <position>`: the first position not yet
examined, the `next` a poll of the change feed would answer, so a client
that reads what is there and stops knows where to stop. A quiet stream
sends the comment `: keep-alive` every fifteen seconds. How long a
caught-up stream waits before it reads the log again is the
configuration's `feed_poll_millis` (250 unless stated).

The feed is the log itself, unmasked, so only a principal that reads
every field of every record, withheld records included
(`Grants::reads_everything`), may open it
(`docs/decisions/037-the-feed-over-a-network-door.md`).

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
- The feed is read as server-sent events until the limit, or until the
  stream says `: next`; a stream silent for two keep-alives is
  `unavailable`. A stream that keeps talking, in keep-alives or empty
  lines, is not silent and holds a poll open until its limit; the
  caller's own timeout is the bound for that. An ingest is held to the
  same silence bound: while records are sent, when the server stops
  taking them (time spent waiting on the source never counts), and once
  they have all been sent, when its answer goes quiet.
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
