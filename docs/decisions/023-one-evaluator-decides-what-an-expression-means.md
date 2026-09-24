# 023 — One evaluator decides what an expression means

## What is decided

Every expression — a filter, a guard, a derived field, a constraint, a
view measure — is evaluated by the one evaluator in `lakelands-core`,
and its rules are part of the contract:

- **Absent never matches.** A comparison with a null or absent operand is
  false, `!=` included. `is null` is the way to ask about absence.
- **Numbers compare as numbers.** When both sides read as numbers —
  a JSON number or a decimal string — they compare exactly; otherwise
  instants compare as instants and two texts as text.
- **Kinds that do not compare never match.** A number against text that
  is not a number, a boolean against anything but a boolean, a JSON value
  against anything but another: every operator is false, `!=` included,
  and none is an error — a filter over mixed data skips what does not
  compare instead of failing the whole read.
- **Arithmetic is exact and checked.** Decimal arithmetic never passes
  through a binary float; overflow and division by zero are errors.
- **`and`, `or` and `if` evaluate only what decides the answer.**

Every typed expression is checked before it is evaluated — the count of
a call's arguments, a field where a function names one, aggregates only
over a group, `changed` and `previous` only in a guard — and one that
fails the check is refused as invalid. `matches` takes only I-Regexp
(RFC 9485), refusing any other pattern syntax.

What an evaluation error means belongs to the caller and is fixed per
caller: a filter refuses the read, a trigger guard does not fire, and a
derived field is cleared.

An engine that evaluates a filter in its own query language must admit
exactly the records this evaluator admits; the conformance suite
compares them on every engine.

## Why

A filter that means one thing in `find`, another in a trigger and a
third in an engine's SQL is three bugs waiting for the record that tells
them apart. The rules above are the ones that record reaches first:
the field that is absent, the amount stored as a string, the sum that
must reconcile to the cent.

"Absent never matches" in particular keeps a filter from selecting a
record for what it does not hold. `status != 'paid'` would otherwise
select every record that has no status at all, which is almost never
what the writer of the filter meant.

## What it is not

- It is not three-valued logic. A comparison with null is false, not
  unknown; `not (a = 1)` is true for an absent `a`.
- It does not make every engine evaluate expressions itself. An engine
  may push a filter down or leave it to the store; either way the answer
  is the evaluator's.
