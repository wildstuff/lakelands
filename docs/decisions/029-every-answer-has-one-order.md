# 029 — Every answer has one order

The rules of this record moved to
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
at lakelands#817. The number is not reused.

| what this record held | where it is now |
|---|---|
| sorted records: the sort keys, the one order across kinds, the position a cursor carries, `after` (lakelands#201), `latest` (lakelands#196) | `025 § Every answer of more than one thing has one order` |
| walks: by depth, type and key | `025 § Every answer of more than one thing has one order` |
| graph measures: degrees and components (lakelands#149) | `025 § Every answer of more than one thing has one order` |
| groups: by their key's canonical bytes | `025 § Every answer of more than one thing has one order` |
| ranked search: equal scores by key; one ranking's ties by the keys at the head, a fused tie by the key answered (lakelands#797, lakelands#826) | `025 § Equal scores of one ranking follow the keys at the head, and a fused tie the key answered` |
| hybrid search fuses its rankings by reciprocal rank | `025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see` |
| the exception: a vector field the lake asks an index for | `025 § Under an index the pages of one query are one sequence of its reader's class` |
| aggregates are the store's | `025 § Aggregates are the store's, except a count alone` |
| Why: an order left to an engine | `025 § Why` |
| What it is not: relevance scores across engines | `025 § A lexical hit's set, order and score are the same on every engine` |
| What it is not: an engine's own indexes | `025 § What it is not` |
