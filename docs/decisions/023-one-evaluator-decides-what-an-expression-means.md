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
over a group, `changed` and `previous` only in a guard or a derived
field — and one that fails the check is refused as invalid. `matches`
takes only I-Regexp (RFC 9485), refusing any other pattern syntax.

What an evaluation error means belongs to the caller and is fixed per
caller: a filter does not match the record, a trigger guard does not
fire, a derived field is cleared, and a view's measure that fails over
a group refuses the read of the view as invalid, naming the view, the
measure and the group (`measure_fails`), as one whose value does not
land is named. The view stays refused whole, a `get` of another group
and a page without the failing one alike: its answer is one
computation, and one engine computes it in its query, so answering
around a group would take a second path that catches each group's
error, which this record exists to rule out (lakelands#925).

A measure fails, too, where its value does not land in the field the
view declares for it: what a field would not take written, it does not
take computed. A value lands as the written value it would be, and is
judged as that one is. In `number` that makes three ranges: within
±(2^53 − 1) a whole value lands exactly and one that is not whole as
the nearest double; from there to 10^21 it is refused, whole or not,
since a number of that size is written as an integer's digits, which no
reader with doubles reads exactly; from 10^21 on it lands as the
nearest double, whole or not, since from there a number is written with
an exponent, which every reader reads as the same double, as `1e21`
written is taken. An instant out of the years 0000 to 9999 and a value
of another kind than the field's fail too. The refusal names the view,
the measure and the group and says what the field refuses. A sum that
can grow past 2^53 is declared `decimal`, which answers its digits at
every size: exact digits are a `decimal`'s. A derived field and a
verb's set land by the same rule: where a view's read is refused, the
derived field holds no value and the verb's call is refused.

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

## How it came to be

- The first version said that `changed` and `previous` stand only in a
  guard, and that a filter's evaluation error refuses the read.
- Corrected at the rewrite (lakelands#817): the check admits `changed`
  and `previous` in a derived field as in a guard; a filter that fails
  on a record does not match it, as 028 has it, and the read is not
  refused.
- Corrected at the rewrite (lakelands#817): the callers named left out
  a view's measure. The read that computes the view turns its error
  into a refusal of the whole read, as invalid, placed at the view.
- A measure whose value its field would not take written fails; before,
  the read answered an integer no double holds or an instant no door
  takes, or left the field out without a word. And a whole value of
  10^21 or more lands as its double, as one that is not whole did: a
  derived field held no value there, and a verb's set of one was
  refused as a value its field would not take (lakelands#839).
- A measure that cannot be computed names its measure and its group in
  the refusal, as one that does not land does; before, a division by
  zero over a group named the view alone (lakelands#925).
