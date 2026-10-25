# 043 — An index asked for answers its set

The rules of this record moved to
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
at lakelands#817. The number is not reused.

| what this record held | where it is now |
|---|---|
| a field the lake asks an index for answers the index's set (lakelands#197) | `025 § A field the lake asks an index for answers the index's set, and says so` |
| asked for per field: the setting `vector_index.<type>` | `025 § A field the lake asks an index for answers the index's set, and says so` |
| what the answer says: `ranked_by`, `indexed` | `025 § A field the lake asks an index for answers the index's set, and says so` |
| every other field answers the reference | `025 § A field the lake asks an index for answers the index's set, and says so` |
| what each engine keeps: PostgreSQL's index, a vector of zeros, SQLite and memory (lakelands#215) | `025 § PostgreSQL keeps a graph per class of reader, and uses it only while it describes the head` |
| a tie at the boundary is pulled deeper (lakelands#827) | `025 § Under an index a tie at the boundary is read within a margin` |
| a short answer is not the field's end (lakelands#827) | `025 § Under an index the rest of the field follows the stream` |
| a page is the index's answer at the depth that page asks for | `025 § Under an index the pages of one query are one sequence of its reader's class` |
| only while it describes the head: the mark | `025 § PostgreSQL keeps a graph per class of reader, and uses it only while it describes the head` |
| a server without pgvector | `025 § PostgreSQL keeps a graph per class of reader, and uses it only while it describes the head` |
| stable between writes, and at the floor always | `025 § An indexed field's set is stable between writes, and its recall is held at a floor` |
| Why: a scan's time grows with the field (lakelands#214); never the default | `025 § A field the lake asks an index for answers the index's set, and says so` |
| Why: the index is kept in PostgreSQL only; the unit vectors are the index's | `025 § PostgreSQL keeps a graph per class of reader, and uses it only while it describes the head` |
| What it is not | `025 § What it is not` |
