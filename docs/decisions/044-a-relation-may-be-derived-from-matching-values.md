# 044 — A relation may be derived from matching values

## What is decided

A relation may be **derived**. It then has no carrier: no `via` field,
no link records, no events. Instead it names a match, one or more pairs
of a field of the owning type and a field of the target type:

```json
{"name": "customer", "target": "customer",
 "derived": {"match": [{"from": "customer_no", "to": "no"}]}}
```

- **What connects.** Two records connect where every pair holds the
  same non-null value: identity of the stored JSON value. Text compares
  as the exact string, so `'010'` is not `'10'`; a decimal compares as
  its stored digits; an instant as the form the lake stored it in.
  Records connect by *identity*, not by the expression's `=` (017),
  which reads text as a number or an instant where it can. A null or an
  absent value never matches, not even another null.
- **What a pair may be.** Both fields exist, and both are of the same
  storage kind, one a key may be (text, a reference, a decimal, a
  number, a boolean or a date-time). `declare` refuses a pair of two
  kinds, naming both fields, since values of two kinds are never
  identical and the pair would match nothing silently.
- **Read-only, resolved when a walk reads it.** The store walks it (026)
  like any relation: in a pattern step, both ways through `inverse`, in
  a neighbourhood or a path, and in `degree` and `components`. The edges
  are the matches as the read's position holds the values. A write that
  changes a matched field moves the edge at that write's position, and
  `as_of` answers the edges as they stood. It has no `link` or
  `unlink` door and no link type.
- **Found through the seam's scan.** A walk asks `scan` for the records
  of the other side that hold exactly the near record's values
  (`Scan.equal`); the seam gains no method. At the head, SQLite and
  PostgreSQL find string values (text, references, decimals, instants)
  through the text index they already keep for every string field. A
  number or a boolean is not in that index, so a match over numbers is
  walked by scanning the type: correct on every engine, and slower at
  scale. No numeric index is kept.
- **Rights (024).** A walk over a derived relation probes its matched
  fields on both sides. A principal that sees one of them masked is
  refused `not_granted`, naming the field, because whether an edge
  appears answers whether two values are equal. One that may not know
  of one is not shown the relation, and every walk answers it as a
  relation the type does not have (lakelands#662).
- **Cardinality** is declared like any relation's and is not enforced:
  there is no write to refuse. A walk returns every match. A
  `many_to_one` relation whose value three target records hold has
  three edges; the answer is what the data holds.
- **Not shape.** It stores nothing and reshapes no record, so, like
  `via` and `inverse`, `derived` is outside the schema hash (007).
  Declaring, changing or dropping one is a `declare`, never a migration
  (027). A relation keeps its carrier: one carried by `via` or by link
  records is not redeclared as derived, nor a derived one as carried
  (035). The refusal names the relation; the other kind is declared
  under another name.
- **A step of the log format.** `derived` is a new member of a relation
  declaration, so it came with a step of the format
  (`docs/log-format.md` §6).

## Why

Data often arrives already connected: an invoice carries the customer's
number as a plain value, and the customer is keyed by it. Before this,
the connection existed in the lake only once someone wrote it, either
record by record or by a host that watched the feed and wrote a
reference for every change on either side. That costs events, lags the
write, and must be kept in step by hand. A derived relation lets the
store connect by value at the moment it reads, from the values the log
already holds.

Identity keeps the connection exact. A foreign key is an identifier
stored on both sides, not a caller's literal: two customers numbered
`010` and `10` are two customers. It also keeps the lookup indexable and
the answer the same on every engine.

## What it is not

- It is not a join language. A match is equality of named fields, and
  a general expression is out of scope (017, 018).
- It is not a reference. It does not require a live target (032); a
  value no record holds simply connects nothing.
- It stores nothing. There is no edge to restrict, retire or purge; the
  records at either end are what visibility is asked of.

## Extended: a step over an inverse (lakelands#438)

"Both ways through `inverse`" holds for every relation, whether it is
carried by a reference field, by link records or derived. The inverse is
the relation's name as seen from its target. A pattern step that names it
walks the relation the other way: `walk: out` over the inverse is
`walk: in` over the relation, `in` is `out`, and `both` stays `both`.
The step's `walk` counts from the name it gives, so a caller standing at
the target reads the relation as one of its own.

A relation declared under the name wins. A step is read as an inverse
only when the type it stands on walks no relation of that name in the
step's direction. That case can arise because `declare` does not refuse a
type whose relation bears the name of an inverse reaching it (only a
bundle checks that, within itself). Every answer a step gave before
inverses were walked stays the same. The edges keep the declared
relation, with its declaring side as `from`, whichever name the step
gave.
