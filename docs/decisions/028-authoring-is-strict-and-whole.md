# 028 — Authoring is strict, and a bundle is refused whole

## What is decided

Every document Lakelands reads from an author — a bundle's YAML files
above all — refuses keys it does not know, naming them. Loading a bundle
is all or nothing: every problem is reported at once, each against its
file, and a bundle with any problem produces no declarations.

The typed form of an expression is strict in the same way, wherever it
arrives. A bundle's declarations must also hold together: a key, a
`depends_on`, a `via`, a `valid_from` or a view's `group_by` that names a
field the type (or the view's source) does not have is refused, and so
is every name that is not an ASCII identifier (a type name: identifiers
joined by `.`).

## Extended: a bundle reaches a lake whole

A bundle's declarations are declared together through `Catalog::apply`:
validated as one set, so a declaration may name one the set declares
later, and appended in one append, so a lake holds all of the bundle or
none of it.

## Why

A misspelt or retired key that is silently ignored leaves a setting at
its default with nothing saying why — the worst kind of error in a
declarative format, found long after it was written. Strictness costs
one line per document type.

All-or-nothing loading means a lake never holds half an ontology, and
reporting every problem at once lets an author fix a bundle in one pass
rather than one error at a time.

## What it is not

- It is not a freeze of the format. Adding a key is a change to the
  format and to its documentation; an old bundle keeps loading.
- It does not make the lake trust the bundle. Every declaration the
  loader produces is checked again when the lake declares it.
