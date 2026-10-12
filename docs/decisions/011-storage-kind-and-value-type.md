# 011 — A field type is a storage kind plus a value type

## What is decided

### A field's type has two layers

A field's type has two layers.

The **storage kind** says how an engine stores, compares and indexes the
value. The set is closed and small: text, number, decimal, boolean,
date-time, bytes, json, reference, vector, geo, date. Each kind has one
form, and the store reads a value's form from its kind alone.

The **value type** says what the value means and what it must satisfy.
The set is open: `iban`, `money`, `email`, `url` or a GeoJSON shape are
declared in the ontology bundle over a storage kind, with constraints in
the one expression grammar, a format and presentation marks.

### A kind exists only where an engine must treat it differently, or where its one form must differ

A new storage kind exists only when an engine must index or compare it
differently from every existing kind, or where its one form must differ
from every other kind's; a kind that exists by its form costs no engine
code. Everything else is a value type.

### There is no integer kind

Whole numbers are not a storage kind of their own. A count, a quantity or
an identifier is a `number` (an engine compares and indexes it like any
number) or, where exactness matters, a `decimal`; a value type such as
`quantity` may constrain it to whole values. A separate integer kind
would be a kind no engine must treat differently and whose form is a
number's (`2.0` is `2`), which is exactly what this record says a kind
is not.

### A calendar date is a kind, because its one form must differ

A calendar date is a kind, `date`, written `YYYY-MM-DD`. No engine
compares or indexes it differently: engines order and compare values by
their one form, and a date reads as the instant at its UTC midnight, so
it sorts and compares with a `date_time` at that midnight. That alone
would make it a value type over `date_time`. But the store reads a
value's form from its kind alone: whether a value fits its field, the
key a key field makes, what a migration converts. A value type that set
the form would give one kind two forms, and changing a field's value
type would rewrite its stored values, which a value type's change never
does.

A date takes a date or an instant at a UTC midnight, written as its
date; any other instant is refused, naming the field. A derived value
lands in a date only at a midnight. A date is a valid time, as a
`date_time` is (028).

### A value takes its one JSON form at the door

A value takes its one JSON form at the door: every number in a `number`,
`json`, `vector` or `geo` value is written as the log writes it
(`docs/log-format.md` §1.2), so `2.0` is `2` and a value reads the same
live and after a rebuild from the log.

### A sum answers at its field's scale, or at the largest scale among its values

A sum, minimum or maximum over a decimal field whose value type declares
a scale answers at that scale, a sum over no values included (`0.00`,
not `0`); without a declared scale it keeps the largest scale of its
inputs.

"The largest scale of its inputs" is meant of the values, not of the order
they are added in. Adding one number to the next does not give it: a zero
hands the other number back as it stands, so `5` and `0.00` summed to `5`,
and `1.10`, `-1.10` and `5` to `5` or to `5.00` by the order of their
keys. A sum therefore reads off its values: at the largest scale among
them, a zero's included, and a zero sum is positive. `5` and `0.00` are
`5.00`; an average divides that sum, and whatever a measure computes from
it starts from it.

This holds wherever the sum is exact, which is wherever the magnitudes of
its values, written at that largest scale, stay below 2^96: there no
order of additions rounds and none fails. Past it (values that need more
than the 28 digits a decimal holds between them) the sum is what adding in
key order gives, rounded or refused. The rule is the
evaluator's (`lakelands_core::expr::aggregate`), so `find`'s aggregate, a
pattern's rows and a view's measures read the same.

## Why

When every type a user might want is a storage variant, each new one
touches every engine, every codec and every place that matches on the
type, and it changes the contract. Separating the layers means a new
value type is a few lines of YAML with no engine change and no contract
change, while the engines keep a closed set of kinds they can handle
exhaustively. The two questions, "does an engine need to treat this
differently?" and "must its one form differ?", are what keep the closed
set small.

## What it is not

- It is not a way to smuggle behaviour into the bundle. A value type
  constrains and presents values; it cannot change how an engine stores
  or indexes them.
- It is not a promise that every kind is fully served in every release.
  A kind can be declared and stored before an engine offers every index
  or predicate for it.
- Adding a storage kind remains a deliberate change: one variant in the
  closed set, its one form, and whatever an engine must do differently
  for it.

## How it came to be

- The first version: the two layers, and a kind only where an engine
  must index or compare it differently.
- There is no integer kind.
- A value takes its one JSON form at the door, and a sum, minimum or
  maximum answers at the declared scale.
- A sum reads its scale off its values, not off the order they are
  added in.
- `date` joined the kinds, and with it a kind that exists by its form
  alone (lakelands#702).
- Corrected at the rewrite (lakelands#817): a kind was said to have one
  codec per engine, where no engine names a kind and a kind that exists
  by its form costs no engine code; and the reason there is no integer
  kind, and the question that keeps the set small, named only the first
  half of what a kind is.
