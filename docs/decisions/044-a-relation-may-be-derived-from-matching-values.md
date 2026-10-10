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

## Extended: an inverse is one relation (lakelands#829)

A step by an inverse was rewritten to the relation's name walked the
other way, and from there it walked every relation of that name: where a
bill and a quote are each `of` a customer and `bills` is the inverse of
the bill's, `bills` from a customer reached the quotes too. And two
relations could name the same inverse onto one target, under different
names, of which the step took the first it found.

An inverse names one relation onto its target. A declaration is refused
where a second relation names the same inverse onto the same target, by
`declare`, `propose`, `apply` and a migration, which share the check.
The refusal stands at the type its author sent and names the inverse and
the target the author sent, and no relation. A relation the type holds
as it stands, under the same name, inverse and target, is not judged
again; one given another name, inverse or target is. Within a bundle the
bundle check refuses a second inverse too, and names both relations,
which are its author's own. The same inverse onto another target is
another name there, and is taken. A host's vocabulary types pass no
declaration check yet (lakelands#835); the rule is theirs when they do.

**What the refusal tells, on purpose.** The other relation may be one
its author is not shown: on a type the author is not shown; on the type
it sent, where it is kept from the field that carries the relation; or
on another type it is shown, where it may not know of the carrying field
or of a field a derived relation pairs.
The refusal then says that some relation the author is not shown points
at a type it is shown, under that inverse. That is a yes or a no for a
pair of inverse and target the author guesses, and neither the
relation's name nor its owner. It is told to an author at `declare`, at
`apply` and by a migration, to a proposer who does not hold `schema`,
and to a reviewer at an accept where the pair was taken after the
proposal, each word for word as an author shown the other relation is
told. An inverse's name around a type is one namespace for everyone who
walks to that type, as a type's name is one for the lake, and one who
proposes a type named as a hidden one is refused too. A collision there
has to be refused, and one relation for an inverse is worth that.

A step by an inverse walks that relation alone, at every hop: an edge of
another owner's relation of the same name is neither probed nor read,
and the step is judged by what that relation reaches, so from its target
it reaches its owner's type and a `type` that is another is refused as
one the step does not reach. A measure counts that relation's links
alone.

A type may still own two relations with one inverse, onto two targets.
Walked `out` from either target the inverse is one relation. Walked `in`
from their owner, or `both` from a type that one such relation leaves
and another reaches, it is read as the first of them found, by its
owner's name and then as declared, as before; nothing is refused for
it.

This is a change for a host: a second relation with an inverse already
named onto its target is refused, and a step by an inverse no longer
answers records of another type that declares a relation of the same
name.
