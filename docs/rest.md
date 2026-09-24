# REST

The HTTP door serves every lake of the root twice, from one catalog:
MCP at `/lakes/<lake>/mcp`, and REST under every other path of
`/lakes/<lake>/`. Both are configured by the `server` section of the
root's `lakelands.json`, take the same API keys, and resolve them through
the same resolver to the same principals.

A REST route only translates. Its path, its query, its headers and its
body become the arguments of one door, under the names that door's MCP
tool takes, and the catalog answers exactly as it answers the tool. What
a door does is stated once, in the contract and the conformance suite;
this page says only how a request reaches it.

## Every request

- **The key.** `Authorization: Bearer <key>`. Every request is resolved on
  its own, before anything about the routes is answered: without a key
  that resolves for the lake, every path answers `401`. There is no session and no pin: a read that must see one state
  names `as_of` itself, and a page's `cursor` already carries the position
  its first page was read at.
- **The position.** Every reply to a caller whose key resolved carries
  `Lakelands-Position`: the position the answer was read or written at,
  or, for a failure, the head of the log when it was answered. A JSON
  answer also carries it as `position`.
- **Failures.** A failure answers one of the contract's closed set of
  errors, as JSON, with the two sentences a person reads beside it:
  `what` happened and what `todo`. A request the server cannot read (an
  unknown parameter, a body that is not JSON, no such route) is
  `refused` as `invalid`; a missing or unknown key is `refused` as
  `not_granted`.

  ```json
  {"error":"conflict","expected":1,"actual":2,
   "what":"the record changed: expected version 1, found 2",
   "todo":"read the record again and retry against its current version"}
  ```

  | Error | Status |
  |---|---|
  | `refused`: `not_granted`, `wrong_door`, `reserved` | 403 (401 for a missing or unknown key) |
  | `refused`: `invalid`, `constraint`, `precondition`, `too_deep`, `migration`, `out_of_range` | 422 (400 for a request the server cannot read, 404 for no route, 405 for a method the route does not take) |
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
| `GET …/types/<type>/records` | find | query: `where`, `order_by`, `limit`, `cursor`, `aggregate`, `with_meta`, `as_of`, `valid_at` |
| `GET …/types/<type>/records/<key>` | get | query: `with_meta`, `as_of`, `valid_at` |
| `PUT …/types/<type>/records/<key>` | upsert | body: the fields to change; `If-Match`, `If-None-Match`, `Idempotency-Key` |
| `DELETE …/types/<type>/records/<key>` | retire | `If-Match`, `Idempotency-Key` |
| `POST …/verbs/<verb>/<key>` | a declared verb | body: its payload (none is `{}`); `If-Match`, `Idempotency-Key` |
| `POST …/query` | the pattern query | body: the query tool's arguments |
| `POST …/search` | search | body: the search tool's arguments |
| `GET …/describe` | describe the lake | query: `as_of`, `valid_at` |
| `GET …/describe/<type>` | describe one type | query: `as_of`, `valid_at` |
| `GET …/proposals` | proposals | query: `state`, `type`, `limit`, `order_by`, `cursor`, `as_of`, `valid_at` |
| `POST …/proposals` | propose | body: `write`, `why` |
| `POST …/proposals/<id>/accept` | accept | — |
| `POST …/proposals/<id>/reject` | reject | body: `reason` (optional) |
| `POST …/schema/declare_type` | declare a type | body: `type` |
| `POST …/schema/add_field` | add a field | body: `type`, `field` |
| `POST …/schema/migrate` | a migration's preflight | body: `to`, `renames` |
| `POST …/schema/migrate/apply` | apply a migration | body: `to`, `renames`, `report_id` |
| `POST …/ingest/<source>` | ingest | body: NDJSON, one record per line |
| `GET …/feed` | the change feed | query: `from`, `kinds`, `types`; `Last-Event-ID` |

A parameter or a body member the door does not take is refused before
anything runs, so a misspelling never turns a compare-and-set into an
unconditional write.

### Query parameters

A query parameter carries the value the door's argument of that name
takes. `limit`, `as_of` and `from` are whole numbers; `with_meta` is
`true`, `false`, or present without a value for `true`; `aggregate` is
JSON. `order_by` is the JSON array of sort keys, or the short form:
fields separated by commas, `-` before a field for descending, and
`known_at(field)` or `seen_at(field)` to order by a field's provenance —
`order_by=-due_date,known_at(status)`. `kinds` and `types` are words
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

`POST …/ingest/<source>` takes `application/x-ndjson`, one source record
per line (`{"type":…,"key":…,"fields":{…},"seen_at":…}`), and is read as
it arrives. The answer is NDJSON too: one line per commit window as it
becomes durable, `{"position":…,"records":…,"duplicates":…}`. The
reply's `Lakelands-Position` is the head before the ingest began; each
window's own position is in its line. A line that
is not a record ends the ingest with a last line naming it; what came
before it is committed, and since ingest is content-addressed, sending
the whole body again appends nothing twice.

### The change feed

`GET …/feed` answers `text/event-stream`: every event of the log from
`from` (0 when absent; past the head's next position it is refused as
`out_of_range`) that matches `kinds` and `types`, then each new
one as it is appended, until the client goes away. Each event's `id` is
its position, its `event` its kind, its `data` the event as JSON; a
reconnecting client sends `Last-Event-ID` and resumes after it. A quiet
stream sends a comment every fifteen seconds. How long a caught-up stream
waits before it reads the log again is the configuration's
`feed_poll_millis` (250 unless stated).

The feed is the log itself, unmasked, so only a principal that reads
every field of every record, withheld records included
(`Grants::reads_everything`), may open it
(`docs/decisions/037-the-feed-over-a-network-door.md`).
