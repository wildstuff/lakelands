# Walk 3 — one lake moved, one mirrored, one measured

Walk 3 is the definition of done for `0.4.0`, as walk 2 was for
`0.2.0`. Starting from a fresh clone, a person:

- builds the CLI;
- creates a CRM lake on SQLite with seven partners and the links of who
  supplies whom;
- exports it and imports it into a second root, where it is the same
  lake, feed for feed;
- adds a ledger lake beside the CRM and lets `serve` mirror the CRM's
  partners into it;
- asks the CRM who the hub of the network is and how many separate
  networks there are, and sees the moved lake refuse the second question
  under a smaller ceiling.

> **Which partner is the hub of the supply network, how many separate
> networks are there, and does the ledger's copy of a partner say which
> state of the CRM it reflects?**

This page is the script, in the form of walks 1 and 2
(`docs/walks/walk-2.md`). Every command and every tool call is written
down with the answer it must give. Every answer is derived by hand from
the files below. An answer that differs is a **rift**: the walk records
it and goes on. The walk needs no fake and no network: nothing but the
CLI and the `mcp_call` example client.

## The materials

`examples/network/` holds two bundles and the root's configuration:

| File | What it holds |
|---|---|
| `crm/types/partner.yaml` | `partner`, keyed by `id`: `id`, `name`, `city`, all `source`. Its relation `supplies` to `partner`, many to many, is carried by link records (`docs/decisions/035-a-link-is-a-record.md`) that arrive by ingest |
| `crm/data/partners.ndjson` | seven partners, P-100 to P-700 |
| `crm/data/supplies.ndjson` | four link records of `supplies`: P-100 → P-200, P-100 → P-300, P-400 → P-100, P-500 → P-600. P-700 supplies no one and no one supplies it |
| `ledger/types/partner.yaml` | the ledger's `partner`: `id` and `name`, both `source`. It declares no `city`, so the city stays in the CRM |
| `lakelands.json` | the root's configuration: the principal `mirror` (a service that writes and ingests) and one mirror connection `crm`, from the lake `crm` into the lake `ledger`, mapping `partner` to `partner` |

The partners:

| Partner | Name | City |
|---|---|---|
| P-100 | Harbor Supplies | Hamburg |
| P-200 | Northwind Tools | Bremen |
| P-300 | Cedar Logistics | Rotterdam |
| P-400 | Delta Metals | Antwerp |
| P-500 | Elm Packaging | Gdansk |
| P-600 | Fjord Timber | Bergen |
| P-700 | Gull Catering | Kiel |

## The answer by hand

**Degree** over `supplies`, both ways, counts each partner's links:
P-100 has three (it supplies P-200 and P-300, and P-400 supplies it);
P-200, P-300, P-400, P-500 and P-600 have one each; P-700 has none.
Rows come highest first, ties by key.

**Components** over `supplies`, both ways, put every partner in exactly
one network, largest first:

| Component | Size | Partners |
|---|---|---|
| 0 | 4 | P-100, P-200, P-300, P-400 |
| 1 | 2 | P-500, P-600 |
| 2 | 1 | P-700 |

The whole type reaches seven records. Under a `max_graph_nodes` of 5 it
is refused `too_large`, never cut
(`docs/decisions/026-the-store-walks-the-graph.md`), while a start at
P-500 reaches two and is answered.

**The answer:** Harbor Supplies (P-100) is the hub, with three links.
There are three separate networks: the four around Harbor Supplies, Elm
Packaging with Fjord Timber, and Gull Catering alone. The ledger's copy
of Harbor Supplies names the CRM and the CRM's position its values were
read at.

## The commands

From a fresh clone, in the repository's root. The `walk` and `moved`
directories and the export may be created anywhere; the page creates
them inside the clone. `cargo install` replaces a `lakelands` already
installed; to keep that one, add `--root <dir>` and put `<dir>/bin` first
on your `PATH`, as in walk 2.

```sh
cargo install --path crates/lakelands-cli
lakelands init walk
cd walk
lakelands lake new crm
lakelands apply ../examples/network/crm
lakelands ingest ../examples/network/crm/data/partners.ndjson --source crm
lakelands ingest ../examples/network/crm/data/supplies.ndjson --source crm
```

| Command | Expected |
|---|---|
| `init`, `lake new` | the root and the lake's location, exit 0 |
| `apply` | `applied 1 declaration(s), in force from position 5:`, then ``type `partner` ``. The lake's own `sys.setting` and `sys.aging_rule` are declared at 2 and 3 when it is first opened; `partner` is at 4 and its link type `sys.link.partner.supplies` at 5 |
| partners ingest | `committed 7 record(s), 0 already in the lake, through position 13`: the lease at 6, the partners at 7 to 13 |
| links ingest | `committed 4 record(s), 0 already in the lake, through position 18`: the lease at 14, the links at 15 to 18 |

The positions come from a run on develop at `caf2a8b`, after
lakelands#234, which made `export` a reader.
`crates/lakelands-cli/tests/walk_3.rs` holds them: it runs these
commands with the built binary, one process each, as the walker does.
Every command that writes appends one `lease_taken` first, as in walks 1
and 2; `feed` and `export` only read and append nothing.

## The lake moves

`lakelands export` writes the CRM's log as files that verify without
Lakelands, and `lakelands import` makes the same lake from them in a
second root (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`,
`docs/hosting.md`, moving a lake). An import cannot rename a lake: the
second root's lake is `crm` too.

```sh
lakelands export --to ../crm-export
lakelands init ../moved
lakelands --root ../moved lake new crm
lakelands --root ../moved import ../crm-export
lakelands feed > crm.feed
lakelands --root ../moved feed > moved.feed
cmp crm.feed moved.feed
tail -n 1 crm.feed
```

| Command | Expected |
|---|---|
| `export` | `log/00000000000000000000.jsonl: positions 0..18`, then `exported crm through position 18 to ../crm-export`. The export reads; it appends nothing, so its head is the lake's |
| `import` | `` imported lake `crm`: 19 event(s) from 1 file(s), head 18, hash <h> ``, where `<h>` is the `hash` of the last line of `crm.feed` |
| `cmp` | prints nothing and exits 0: the two lakes' feeds are the same bytes, every event at its position, with its hash |

`<h>` differs from run to run, since every event's hash covers the time
it was written; that it equals the old lake's last hash does not.

## The ledger mirrors the CRM's partners

Now the first root holds two lakes, and every command names the one it
works on with `--lake`. The root's configuration is replaced by the
walk's, which adds the principal `mirror` and the connection `crm`. The
configuration comes before the ledger's first open, because the root's
system vocabulary gives the type `sys.mirror` to every lake a connection
writes into when a writer opens it
(`docs/decisions/042-water-crosses-only-as-a-mirror.md`, `docs/seams.md`,
the mirror).

```sh
lakelands lake new ledger
cp ../examples/network/lakelands.json lakelands.json
lakelands --lake ledger apply ../examples/network/ledger
sleep 2 | lakelands --lake ledger serve --mcp
```

| Command | Expected |
|---|---|
| `lake new` | the lake's location, exit 0 |
| `apply` | `applied 1 declaration(s), in force from position 5:`, then ``type `partner` ``. `sys.mirror` was declared at 4, after the lake's `sys.setting` and `sys.aging_rule` |
| `serve` | prints nothing. Its lease is at 6. The mirror starts from a snapshot: it reads the seven partners as the CRM holds them at its head, 18, and ingests them as the source `mirror:crm:partner` at 7 to 13, `id` and `name` only; then it records its place, the `sys.mirror` record `crm:partner`, at 14 |

`serve --mcp` starts the mirror and answers at once, so a read in the
same session could come before the copies. `sleep 2 |` holds this
`serve` up for two seconds with no client, as walk 2's deliverer step
does, and the snapshot lands meanwhile. The mirror keeps a journal in
`mirrors/ledger/crm.ndjson`; its last line, `{"snapshot":18, …}`, says
the snapshot is done.

Then the agent reads the ledger. Its session's lease is 15, the
position both answers carry; this session runs the mirror again, which
finds nothing new in the CRM and appends nothing.

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands --lake ledger serve --mcp" \
  tools/call partner_get '{"key": "P-100", "with_meta": true}' \
  tools/call sys_mirror_find '{}'
```

**1. `partner_get`** `{ "key": "P-100", "with_meta": true }`: the fields
`{ "id": "P-100", "name": "Harbor Supplies" }`, no `city`. The metadata
of each field says where it came from:

```json
{ "origin": "source", "source": "mirror:crm:partner", "writer": "service:mirror",
  "position": 7, "upstream": { "lake": "crm", "position": 18 }, … }
```

`position` is where the copy was written in the ledger; `upstream` is
the lake and the position of the CRM state it was read from (format
version 3, `docs/log-format.md`). `seen_at` is the CRM's `known_at` of
the record.

**2. `sys_mirror_find`** `{}`: one row, the connection's place for its
one type:

```json
{ "shape": "records", "rows": [
  { "type": "sys.mirror", "key": "crm:partner", "version": 1,
    "fields": { "connection": "crm", "next": 19, "source_lake": "crm",
                "source_type": "partner", "target_type": "partner" } } ] }
```

`next` is the next CRM position the mirror reads; no `stopped_at`, so
nothing stopped.

## The CRM measures its network

The agent asks the CRM, in a session whose lease is 19:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands --lake crm serve --mcp" \
  tools/call query '{"start": {"type": "partner"}, "steps": [{"relation": "supplies", "walk": "both"}], "project": {"as": "degree"}}' \
  tools/call query '{"start": {"type": "partner"}, "steps": [{"relation": "supplies", "walk": "both"}], "project": {"as": "components"}}'
```

**3. `query`** projected as `degree`, at 19: seven rows of `type`, `key`
and `degree`: P-100 3; P-200, P-300, P-400, P-500, P-600 1; P-700 0.

**4. `query`** projected as `components`, at 19: seven rows of
`component`, `size`, `type` and `key`: P-100, P-200, P-300 and P-400 in
component 0 of size 4; P-500 and P-600 in component 1 of size 2; P-700
in component 2 of size 1.

## The moved lake under a smaller ceiling

A lake's `max_graph_nodes` is a limit of its location, not of its log
(`docs/seams.md`, the whole-graph measures). The moved root's location
of `crm` is lowered to 5, and the same lake is asked again. `sed -i.bak`
edits in place with GNU and BSD `sed` alike.

```sh
sed -i.bak 's/"max_graph_nodes": 100000/"max_graph_nodes": 5/' ../moved/lakes/crm.json
cd ../moved
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call query '{"start": {"type": "partner", "key": "P-500"}, "steps": [{"relation": "supplies", "walk": "both"}], "project": {"as": "components"}}' \
  tools/call query '{"start": {"type": "partner"}, "steps": [{"relation": "supplies", "walk": "both"}], "project": {"as": "components"}}'
```

**5. `query`** from P-500, at 19 (this session's lease; the moved lake's
head was 18): component 0 of size 2, P-500 and P-600.

**6. `query`** from the whole type: refused. `mcp_call` prints the
tool error whole, its two sentences and the error as JSON, and exits 1:

```
error: refused: a graph measure may reach 5 records, no more
  what to do: start from fewer records, or raise the lake's `max_graph_nodes` in its location
{
  "error": "refused",
  "reason": {
    "max": 5,
    "refusal": "too_large"
  }
}
```

Nothing partial is answered.

**The agent's answer:**

- Harbor Supplies (P-100) is the hub of the supply network, with three
  links. There are three networks: Harbor Supplies with Northwind Tools,
  Cedar Logistics and Delta Metals; Elm Packaging with Fjord Timber; and
  Gull Catering alone.
- The ledger's Harbor Supplies was written by the mirror at the ledger's
  position 7, from the CRM as it stood at 18; its city stayed in the
  CRM.

## Recording the walk

Write the result under this heading, in a pull request after the walk:

- the date and the commit;
- every command's and call's actual output where it differs from the
  expected one;
- each rift as a finding, with the issue it was filed as.

A walk with no rift says so.
