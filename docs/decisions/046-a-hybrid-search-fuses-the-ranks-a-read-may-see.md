# 046 — A hybrid search fuses the ranks a read may see

The rules of this record moved to
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
at lakelands#817. The number is not reused.

| what this record held | where it is now |
|---|---|
| a hybrid search fuses its lists by reciprocal rank; each list ranks only what the read may see; each list counts to `max_limit`; how far the store reads; one list keeps its own score | `025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see` |
| Why: ranks counted among the visible hits, a depth fixed by the lake, ranks and not scores | `025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see` |
| What it is not: the lexical half's order | `025 § A lexical hit's set, order and score are the same on every engine` |
| a ranking has a weight, and a vector search may fuse several fields in `also` (lakelands#428) | `025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see` |
| a search says what each axis brought (lakelands#560) | `025 § A search says what each axis brought` |
| a fused list is read to its window (lakelands#560) | `025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see` |
| a type weighs its searches by a setting (lakelands#561) | `025 § A type weighs its searches by a setting` |
| a query vector has a direction (lakelands#696) | `025 § Every query vector a search names has a direction` |
| a record's vector is not held to a direction | `025 § A vector ranking is the cosine, best first, exact to the bit` |
| a vector ranking reads a vector field (lakelands#743) | `025 § A vector ranking reads a vector field` |
| a vector search may bring `text` the lake embeds with the field's model, a member of the vector search and not a mode (lakelands#1567) | `025 § A vector search may bring words, and the lake embeds them with the field's model` |
| a hybrid search whose query the lake cannot embed ranks by its text, by default, and names the leg in `unranked` (lakelands#1566) | `025 § A hybrid search whose query the lake cannot embed ranks by its text, and says why` |
