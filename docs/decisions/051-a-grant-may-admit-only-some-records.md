# 051 — A grant may admit only some records

The rules of this record moved to
`docs/decisions/006-grants-not-roles.md` at lakelands#817. The number
is not reused.

| what this record held | where it is now |
|---|---|
| a grant may hold `only`, pairs of a field and a value, and admits only the records that hold them; identity, text, closed, and a grant that cannot be applied admits none (lakelands#771) | `006 § A grant may admit only the records that hold its pairs` |
| every read; a search as of a position; views; links | `006 § A grant may admit only the records that hold its pairs` |
| a holder writes its own records alone | `006 § A grant may admit only the records that hold its pairs` |
| what a write still tells: a taken key, a continued version, and when `ingest` judges a link's cardinality | `006 § Some things are still told, on purpose` |
| a cardinality refusal names the other link's end only where its writer is answered the link that holds | `006 § A record held at the head is not there to one without the grant, at any position and at every write door` |
| a holder proposes writes to its own records, pinned to them | `006 § A grant may admit only the records that hold its pairs` |
| a reviewer holding `only` decides what it is shown whole | `006 § A grant may admit only the records that hold its pairs` |
| what stays refused: an aging rule, every schema door, a migration's proposal | `006 § A grant may admit only the records that hold its pairs` |
| `describe` tells the holder its pairs and offers no write that would be refused; the change feed | `006 § A grant may admit only the records that hold its pairs` |
| `only` is a function of `Principal.id`; a retry answers its first outcome | `006 § A grant may admit only the records that hold its pairs` |
| Why: the engine selects only admitted records, the value is in the grant, a write is judged against the holder's own record, the pin (lakelands#660) | `006 § A grant may admit only the records that hold its pairs` |
| What it is not: several values, a predicate, `Principal.id` implicitly, a change of `restricted` | `006 § What it is not` |
| `only` keeps the reference it names (lakelands#718) | `006 § A grant may admit only the records that hold its pairs` |
| a restricted record is not there to a write (lakelands#772) | `006 § A record held at the head is not there to one without the grant, at any position and at every write door` |
| a proposal whose record its reviewer is not answered is not that reviewer's to decide (lakelands#772) | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a link with a held end is not there, and an accept is judged by what its write reaches (lakelands#781) | `006 § A record held at the head is not there to one without the grant, at any position and at every write door` |
| what is still told under `restricted` as under `only`: a cardinality refusal and an accept's refusal (lakelands#781) | `006 § Some things are still told, on purpose` |
| one answer of what a grant admits, and one of whether a record is answered, a link with its ends (lakelands#798, lakelands#871, lakelands#879) | `006 § A grant may admit only the records that hold its pairs` |
