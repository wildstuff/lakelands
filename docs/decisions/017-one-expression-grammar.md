# 017 — One expression grammar

## What is decided

Lakelands has one expression language. Filters on `find`, on pattern
steps and on object sets, trigger guards, derived fields, value-type
constraints, verb preconditions and view measures are all written in it.
Its typed core is `lakelands_api::expr::Expr`; its text form is the
grammar in that module's documentation, parsed into the core and printed
from it. A caller passes either form.

The set of functions is closed and part of the contract. Aggregates may
appear only where a set of records is in scope. In anything the fold
evaluates (derived fields, constraints, guards), `now()` and `today()`
mean the `known_at` of the event being folded, so replay reproduces
them.

## Why

A caller should learn `where` once. When filters, guards and derived
fields each have their own syntax, every one of them has its own edge
cases, its own evaluator and its own bugs, and a rename has to be
applied in three places. One grammar with one evaluator makes an
expression mean the same thing everywhere, lets a filter that works in
`find` be copied into a trigger unchanged, and gives the conformance
suite one semantics to check on every engine.

Making the function set closed keeps every evaluator, and every engine
that pushes filters down into its own query language, complete.

## What it is not

- It is not a query language. It has no joins, no subqueries and no
  statements; walking relations is the pattern query's job (018).
- It is not extensible by a bundle. A new function is a contract change.
- The text form is not a second language beside the typed form. Both
  are the same expressions.

## Extended: a read's function (lakelands#145)

The function set grew on the read side. `date_bucket(x, unit)` answers
the instant at which the UTC bucket holding `x` starts (`day`, `week`
from Monday as ISO counts weeks, `month`, `quarter`, `year`). It serves
`find`'s `where`, an aggregate's named group keys and a pattern's
`where`. What a declaration may call is the log format's concern: the
log keeps a declaration's expressions, and the format closes the
functions they may call (`docs/log-format.md` Appendix B). It admits
`date_bucket` in every expression a declaration keeps (a derived field,
a view's `where` and measures, a value type's constraint, a verb's
`requires` and assignments, a trigger's `when`, an object set's filter,
an expression function; lakelands#174). A derived bucket field is then
found and grouped by as any other field.
A new function is still a contract change. Where it may stand is part of
that contract, and a function that only reads moves no log format.

## Extended: arithmetic on durations (lakelands#549)

The difference of two instants was a duration, and nothing could be done
with one but add it to an instant. A host that asked for the days between
two instants found no spelling that answered: `(today() - due_on) / 1d`
failed for every record, which a read showed as no match and a null key,
and a derived field as a field never filled.

The grammar stays as it is, and three operations get a meaning:

| operation | answer |
|---|---|
| a duration `/` a duration | a number, exact: how many of the one the other holds |
| a duration `/` a number, a duration `*` a number, a number `*` a duration | a duration, to the microsecond |

So the days between two instants are `(paid_on - due_on) / 1d`, the
hours `/ 1h`, the weeks `/ 7d`: one spelling for every unit, and no
function a unit. One function is added, `floor(n)`, the greatest whole
number not above `n`: the whole days that have passed are
`floor((now() - due_on) / 1d)`, where `round` would count a day from its
noon on.

`now()` and `today()` are the fold's clock in a derived field, the
`known_at` of the event folded: a derived "days overdue" is as of the
record's last write. A read that computes it asks as of the read.

## Extended: a bucket of any width (lakelands#559)

`date_bucket` knew the calendar's five units. A host that groups
invoices into windows of thirty days from a date it names had the
formula, once durations divide, and nothing shorter.

`date_bucket(x, width, origin)` answers the start of the window of that
width holding `x`: `origin + floor((x - origin) / width) * width`. The
windows are counted from the origin in both directions, so an instant
before it falls in the window that starts before it, and none is lost.

- **The width is a duration, as the grammar writes one** (`30d`, `12h`),
  above zero. A quoted `'30d'` is refused, and its refusal names the
  unquoted form: a quoted second argument is a calendar unit.
- **A width takes its origin.** No default is chosen for it: counted
  from 1970, a week would begin on a Thursday and thirty days on a date
  nobody picked. `date_bucket(x, 30d)` is refused, naming the form with
  three arguments.
- **A calendar unit takes none.** A month has no width to count from an
  origin; `date_bucket(x, 'month', origin)` is refused.

It is the one function with two arguments more, and no new unit. A read
and a declaration may call it.
