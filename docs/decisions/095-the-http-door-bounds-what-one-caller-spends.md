# 095 — The HTTP door bounds what one caller spends

Decided on lakelands#1523, for 0.22.0.

## What is decided

The HTTP door, MCP and REST alike, holds each request to bounds that the
operator sets in the server's configuration. One caller cannot spend
what the door's other callers need. The store, its limits and its
answers are unchanged: the door decides only whether a request reaches
them.

### Three things are bounded, each where it can be counted

- **The rate of requests**, per connection and per principal. Each is a
  count of requests a second, kept as a bucket that holds one second's
  worth: a caller may send that many at once, and gets back one more
  for every share of a second after. A principal's rate counts over all
  of its connections, so opening more connections buys no more.
- **The requests in flight**, per principal: how many of one principal's
  requests are being answered at once. A connection carries one request
  at a time (the door speaks HTTP/1.1, where a pipelined request waits
  for the one before it), so its count in flight is one by the protocol
  and has no setting.
- **The size of a body**, per request: the largest body read whole, for
  MCP and for REST. It is read before an MCP request's session, and so
  its principal, is known, so it is one bound for every connection and
  every principal. An ingest body is streamed and is not bounded in
  size; each of its lines is, as before.

### Where a request is counted

The door counts a request by its connection as it arrives, before
anything of it is read, in one layer in front of every route; a probe,
`/livez` or `/readyz`, is not counted, so an orchestrator polling a
busy connection does not see the process as down. It counts
it by its principal where it resolves one: a key or a host's forwarded
call over REST, a key at an MCP `initialize`, an MCP session's own
principal for every later request of the session. A principal is its
lake and its id. A forwarded call counts against the principal it is
made for, and against the connection of the host that forwards it.

A request refused before its principal is resolved, for an origin, a
missing or unknown key or a path that is no lake's, is counted by its
connection only. Over MCP the body is read before the principal is
counted: the door must read the message to know whether it is an
`initialize`, and a later one names its session in a header but is
resolved only beside its body. So an MCP request spends its read, up to
`body_bytes`, before a principal's bound can refuse it; REST counts the
principal before it reads the body.

A request holds its principal's slot from that point until its answer is
made. The slot is given back when the slot's guard is dropped, so a
request that is answered, a request whose client goes away, and a
request whose handler panics all give it back. An ingest holds its slot
until its last window is committed, not only until its answer begins:
its commits are the work, so one principal runs no more ingests at once
than its bound. A stream that only reads (an MCP event stream, the feed)
holds no slot while it streams: it is bounded by its own session and its
idle end.

### The settings are the server's

The bounds are the root's `lakelands.json`, under `server.bounds`, and
nothing is read from the environment:

| key | default |
|---|---|
| `connection_rate` | 1000 requests a second |
| `principal_rate` | 1000 requests a second |
| `principal_in_flight` | 128 |
| `body_bytes` | 4 MiB, the bound the door had before |

Each takes a count above zero, or `"off"` for no bound. A section that
names none of them has the defaults.

### A refusal says the same at each door

- Over a rate or the requests in flight: `429`, with `Retry-After` in
  whole seconds, at least one, and the contract's `unavailable` with
  the situation `rate_bound` (`per`: `connection` or `principal`,
  `most`, `retry_after`) or `in_flight_bound` (`most`). REST answers it
  as it answers every failure, with its `what` and `todo`. MCP answers
  it as a JSON-RPC error, code -32000, whose `data` is the same JSON, so
  a client reads the same code on either door; a refusal for the
  principal's bound answers the request's own id.
- Over the size: `413`, and the contract's `refused` as `invalid` with
  the case `body_too_large`. MCP's JSON-RPC error for a request it
  cannot read carries the same refusal in `data`.

Nothing of a refused request is read or written.

### What is counted does not grow without bound

A principal's count that has no request in flight and whose bucket is
full again says nothing that a new count would not, and it is dropped.
That happens when its last request ends, if the bucket is full by then.
Otherwise it happens at a sweep, which runs whenever the counts have
doubled since the last one, from 64 on. So the counts hold the
principals of about the last second, at most twice over. A connection's
count lives with its connection and goes when the connection closes.

The counts sit behind a plain mutex that is taken to count and let go at
once. No lock is held across an `.await`, and none while the store is
called.

## Why

A door that is reachable on a network serves callers that do not
coordinate. An agent in a loop, or a script with no backoff, can take
the worker threads, the blocking pool and the writer's turn that every
other caller waits for. Bounding each principal keeps the others
answered. Bounding each connection also covers what arrives before
anyone is known: a request with a bad key still costs a resolve.

The rate and the requests in flight measure different costs. A rate
does not see a few slow requests, such as a large read or a long write,
that hold the door for seconds. A bound in flight does not see many
fast ones. Each catches what the other lets through.

A request over a bound is right, and later it would be answered. The
closed set's `unavailable` already says "it cannot be answered now, and
retrying may help". It is answered `429`, not the `503` that other
`unavailable` failures are answered with, because the cause is the
caller's own volume and not the server's state. `Retry-After` tells a
client how long to wait without having to read the body.

The bounds are on unless the operator says otherwise; the operator
decided so on 2026-10-30, with the defaults above. An operator who puts
a door on a network should not have to know about bounds to get them. The defaults sit well above what an agent sends, and a host that
sets nothing sees no change. `"off"` is there for a host whose own
proxy already bounds its callers.

## What it is not

- **Not the stdio door.** A stdio server serves the one principal of the
  process that started it, over that process's own pipe. Its pace is
  its own, and no other caller shares it.
- **Not anything the operator did not configure.** No bound is set on the
  number of connections or sessions (a session ends when it is idle),
  on a stream's length, on an answer's size (the lake's limits bound
  that), per network address, or over hours or days.
- **Not a queue.** A request over a bound is refused at once and never
  held until there is room. Its client decides when to send it again.
- **Not per-principal settings.** Every principal has the same bounds:
  the door holds each one to them apart, and none to another's.
- **Not a defence against a distributed flood.** Many connections from
  many principals stay within their own bounds. Bounding the whole is
  the job of the network in front of the door.
