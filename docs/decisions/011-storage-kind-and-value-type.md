# 011 — A field type is a storage kind plus a value type

## What is decided

A field's type has two layers.

The **storage kind** says how an engine stores, compares and indexes the
value. The set is closed and small: text, number, decimal, boolean,
date-time, bytes, json, reference, vector, geo. Each kind has one codec
per engine.

The **value type** says what the value means and what it must satisfy.
The set is open: `iban`, `money`, `email`, `url` or a GeoJSON shape are
declared in the ontology bundle over a storage kind, with constraints in
the one expression grammar, a format and presentation marks.

A new storage kind exists only when an engine must index or compare it
differently from every existing kind. Everything else is a value type.

## Why

When every type a user might want is a storage variant, each new one
touches every engine, every codec and every place that matches on the
type, and it changes the contract. Separating the layers means a new
value type is a few lines of YAML with no engine change and no contract
change, while the engines keep a closed set of kinds they can handle
exhaustively. The question "does an engine need to treat this
differently?" is what keeps the closed set small.

## What it is not

- It is not a way to smuggle behaviour into the bundle. A value type
  constrains and presents values; it cannot change how an engine stores
  or indexes them.
- It is not a promise that every kind is fully served in every release.
  A kind can be declared and stored before an engine offers every index
  or predicate for it.
- Adding a storage kind remains a deliberate change: one variant in the
  closed set and one codec per engine.

## Extended: there is no integer kind

Whole numbers are not a storage kind of their own. A count, a quantity or
an identifier is a `number` (an engine compares and indexes it like any
number) or, where exactness matters, a `decimal`; a value type such as
`quantity` may constrain it to whole values. A separate integer kind
would be a kind no engine must treat differently, which is exactly what
this record says a kind is not.

## Extended: one JSON form, and the scale carried through

A value takes its one JSON form at the door: every number in a `number`,
`json`, `vector` or `geo` value is written as the log writes it
(`docs/log-format.md` §1.2), so `2.0` is `2` and a value reads the same
live and after a rebuild from the log. A sum, minimum or maximum over a
decimal field whose value type declares a scale answers at that scale, a
sum over no values included (`0.00`, not `0`); without a declared scale
it keeps the largest scale of its inputs.
