# Glossary

Three words name the three notions of time and change in Lakelands. They
never vary anywhere in the repository — not in code, not in docs, not on
the wire — and no fourth word (`revision`, `epoch`, `snapshot`, `seq`)
stands in for any of them.

**position** — the log coordinate. Every event in a lake's log has one:
0 for the first, then consecutive with no gaps. Every reply carries the
position it was read at, and a read can be pinned to an earlier one with
`as_of`. A position orders everything in one lake; it is not a
timestamp, and it means nothing across lakes. See
`docs/decisions/009-every-response-carries-its-position.md`.

**version** — a record's compare-and-set token. It starts at 1 when a
record is first written and grows by one with every event that changes
that record. A writer passes `expected_version` to refuse its write if
the record moved. A version orders one record's changes; it says nothing
about the rest of the lake.

**valid_at** — business time: the instant in the world a read asks
about. A record of a valid-timed type holds between its `valid_from` and
`valid_to`; `valid_at` selects the records valid at that instant. It
combines with `as_of` and never replaces it: `as_of` asks what the lake
knew, `valid_at` asks what was true.

A **cursor** continues a list-shaped read where its previous page
ended. It is bound to the position its first page was read at, so a page
set stays stable while the lake is written. `find`, pattern rows and
`proposals` take `limit`, `order_by` and `cursor`; `search` takes `k`
and `cursor` and no `order_by`, because relevance is its order by
definition. `find` also takes `after` instead of a cursor: a row's sort
values and key, continued strictly after at the caller's read position.

Two words sit beside them, as per-field provenance
(`docs/decisions/010-provenance-per-field.md`): **known_at**, when the
lake learned a value (the writing event's wall time), and **seen_at**,
when its source observed it.
