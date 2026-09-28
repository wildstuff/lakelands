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
functions they may call (`docs/log-format.md` §6, Appendix B). Format
version 3 admits `date_bucket` in every expression a declaration keeps (a
derived field, a view's `where` and measures, a value type's constraint,
a verb's `requires` and assignments, a trigger's `when`, an object set's
filter, an expression function); the event that first declares such a
call is written in version 3, and so is every event after it
(lakelands#174). A derived bucket field is then found and grouped by as
any other field.
A new function is still a contract change. Where it may stand is part of
that contract, and a function that only reads moves no log format.
