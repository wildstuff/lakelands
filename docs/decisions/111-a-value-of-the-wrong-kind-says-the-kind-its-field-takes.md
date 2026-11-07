# 111 — A value of the wrong kind says the kind its field takes, never the value

A write of a value whose JSON form is not the form its field's storage
kind takes is refused with one case, `value_shape`, which names the kind
the field takes and the form that kind is written in. The refusal names
the field the value was written to; the case names no field and no value.

The case and its sentence are the catalogue's already. What this record
decides is what the answer may carry — the field on the refusal, the
value nowhere — and that the specification pins the answer for every
kind, on every engine.

## What is decided

### A wrong-kind value is refused `value_shape`, for every kind with a form of its own

`Case::ValueShape { kind: StorageKind, shape: Shape }` renders
"a {kind} value must be {shape}"
(`crates/lakelands-api/src/error/case.rs`). `kind` is the kind the field
declares; `shape` is the JSON form that kind is written in, one of the
eight in `pub enum Shape` there: a string, a number, `true` or `false`, a
string of digits, a date-time string, a date string, a base64 string, and
an array of numbers each finite as a 32-bit float.

`check_kind` (`crates/lakelands-core/src/value/mod.rs`) answers it from
each kind that has a form of its own. The answer by kind:

| kind | a value it does not take | `shape` | the case renders |
|---|---|---|---|
| `text` | `1` | `text` | a text value must be a string |
| `reference` | `1` | `text` | a reference value must be a string |
| `number` | `"1"` | `number` | a number value must be a number |
| `boolean` | `"true"` | `boolean` | a boolean value must be true or false |
| `decimal` | `1.5` | `digits` | a decimal value must be a string of digits |
| `date_time` | `1` | `date_time` | a date_time value must be a date-time string |
| `date` | `1` | `date` | a date value must be a date string |
| `bytes` | `1` | `base64` | a bytes value must be a base64 string |
| `vector` | `[0.1, "x", 0.3]` | `numbers` | a vector value must be an array of numbers, each finite as a 32-bit float |
| `json` | nothing — it takes every JSON form | — | — |
| `geo` | `"POINT(7 50)"` | — (`geo_not_object`) | not a GeoJSON geometry: not an object |

`json` holds every JSON value; its numbers are held to the same bound as
a `number`'s, so an integer beyond ±(2^53 − 1) anywhere in it is refused
`integer_too_large`, not `value_shape`. `geo` does not answer
`value_shape` at all: its own six cases say which member is wrong —
`geo_not_object`, `geo_no_type`, `geo_unknown_type`, `geo_no_coordinates`,
`geo_coordinates`, `geo_no_geometries` — which is more than "a geo value
must be an object" would say, and `Shape` has no `object` form. `null` in
a patch clears the field, as `null_clears_a_field_whatever_its_value_type_allows`
states, and is never a wrong kind.

A value of the right form whose content the kind does not take answers
another case, and this record leaves all of them as they are:
`not_decimal` for a string that is not digits (`"12,50"`), `not_date_time`
for a string that is no UTC instant, `not_date` for a date string that is
no date, `not_base64` for a string that is not standard base64,
`vector_length` for a vector of another length than the field declares,
`vector_no_dimensions` for a vector field that declares none, and
`integer_too_large` for an integer beyond what every reader reads
exactly.

The write door wraps the case as a constraint that names the field:
`check_values` calls `value::check` and maps a failure to
`Error::constraint(Some(name), e)`
(`crates/lakelands-store/src/writes.rs`). Every door that writes a value
goes through it — an upsert, a patch, a verb's payload, a declare's
`initial` — so the field named is the field the value was written to.

### The field rides on the refusal, not in the case

A refusal already says which field it is about:
`Error::Refused { field: Option<FieldName>, reason }`, and
`Error::constraint(Some(name), ..)` sets it (`crates/lakelands-api/src/error.rs`).
A case says what its sentence says, and every parameter of a case is in
that sentence (`crates/lakelands-api/src/error/case.rs`, the catalogue's
rules; `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
"a {kind} value must be {shape}" has no field in it. A `field` parameter
would either stand outside the sentence, against that rule, or lengthen
the sentence, which would be a new case with a new code — not this one,
whose code keeps its meaning once shipped.

So the answer is the `reason` the catalogue gives, with `field` beside it
on the error, as every refusal already carries it
(`crates/lakelands-api/src/error/case/tests.rs` states the wire form):

```json
{"error": "refused", "field": "amount",
 "reason": {"refusal": "constraint", "code": "value_shape",
            "kind": "decimal", "shape": "digits",
            "message": "a decimal value must be a string of digits"}}
```

The sample in `crates/lakelands-api/src/error/case/samples.json`, and the
row for `value_shape` in `docs/refusals.md`, are this answer and do not
move.

### The value given is not told

The issue asked for the value given beside the kind. The answer does not
carry it, for three reasons.

The catalogue says what a text parameter may be: a name, or a value the
caller gave, never a sentence built elsewhere (`case.rs`). The same check
runs on what a function computed (`crates/lakelands-store/src/backed.rs`,
`read_output`), where the value is not the caller's at all.

That is also where it would leak. `read_output` wraps a failed check as
`Case::OutputNotValue { cause }` and the door keeps the rendered text as a
derived field's `error` in its `FieldMeta`
(`crates/lakelands-api/src/record.rs`), which a read tells by default: a
type reads field meta unless it declares `field_meta: false`
(`docs/log-format.md`). A caller who may read the derived field but not
the fields it was computed from would then be told a value of one of
them. That is the rule of
`docs/decisions/107-a-verb-is-callable-only-to-one-who-may-read-and-write-what-it-does.md`
— a field derived from one a caller may not read is not offered to that
caller — applied to the text of a refusal. A value parameter on
`value_shape` would carry it there.

And it says nothing its reader needs. The caller sent the value; what it
does not know is which field refused it, the kind that field takes, and
the form that kind is written in. The kind and the form are in the case;
the field is on the refusal.

This record adds no parameter and changes no sentence therefore. It also
does not change the three cases that already carry a written value —
`not_decimal`, `not_date_time`, `not_date`, whose `value` is rendered in
their sentences. The same question is open for them; this record neither
widens it nor narrows it, and a later record must decide it (see below).

### The contract changes not at all on the wire; the specification gains the guarantee

No case is added, renamed or retired, no sentence is reworded, and no
parameter is gained or lost. Every refusal that is answered today renders
the same bytes after this change. A host that shows the message may now
switch on `code` with `kind` and `shape`; one that already does is
unchanged.

What changes is inside the repository. The case is answered by product
code that no row pins: `refused_in` (`crates/lakelands-core/src/conformance/checks/values.rs`)
accepts any `Constraint` or `Invalid` on field `v`, so today an edit that
changed the case, the sentence or the field would ship without a red run,
and the only row naming `value_shape` is a view measure's
(`crates/lakelands-core/src/conformance/checks/landing.rs`). The rows
below make the answer a behaviour of the contract, checked on every
engine, as a behaviour claim is stated in the suite and not beside one
engine.

## Why

A host renders a refusal in its user's language from the case, and a
closed set of cases with stable codes is what makes that possible
(`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
A store sentence that a host must parse, or show untranslated, is a host
contract unkept. The case already says the right thing; the gap is that
nothing holds it to saying it.

The field is on the refusal because the refusal is already about a field
and a case is a sentence. The value is not told because a refusal's text
reaches readers who may not read what produced it, and because the caller
already holds the value it sent.

## What it is not

- It is not a new case, a new parameter or a reworded sentence, and
  nothing on the wire moves.
- It is not a fix for the value-carrying cases (`not_decimal`,
  `not_date_time`, `not_date`). Dropping `value` from one is not an
  optional-member change: a shipped code keeps its meaning, so it would
  take a new code and a retired one. That decision is a record of its own.
- It is not a change at the computed-field site: `output_not_value` still
  carries the cause of a failed check, and the field still holds no value
  with that text in its meta.
- It is not `Shape::Object`, which would add a ninth form to a shipped
  enum and say less than `geo`'s own cases do.
- It is not a check a host must not duplicate; the store's check answers,
  and a host that checks first is only saved a round trip.

## How it came to be

- lakelands#1644 asked for a case carrying the field, the value given and
  the kind the field takes, reading the catalogue at `v0.22.0` as having
  no wrong-kind case. The tree disagrees, measurably: `value_shape` is
  in the catalogue at `v0.22.0` (the row is in `docs/refusals.md` at that
  tag) and was added by lakelands#650, its sample is in `samples.json`,
  and the field already rides on the refusal — the issue's own example
  carries `field` and no `code`, and the catalogue has rendered `code`
  since lakelands#650. The record therefore keeps the case and the field,
  drops the value, and adds what the tree was missing: the pinned answer,
  by kind, on every engine.

### Conformance rows

Two rows, each a `check!` in
`crates/lakelands-core/src/conformance/checks/values.rs`, each run on
every engine:

- `WRONG_KIND_SAYS_THE_KIND = a_value_of_the_wrong_kind_says_the_kind_its_field_takes`
  — for each of the nine kinds above, declare a one-field type of that
  kind (a vector declaring its dimensions), write the wrong form, and
  assert that the answer is a refusal whose `reason` is
  `Refusal::Constraint`, that `refused_on` gives field `v`, that the said
  case is exactly `Case::ValueShape { kind, shape }` (compared as the
  case, not as text), that the sentence is the case's, and that the head
  did not move. The same row asserts the cases where `value_shape` stops:
  a right-form value with wrong content answers `not_decimal`,
  `not_date_time`, `not_date`, `not_base64`, `vector_length` or
  `integer_too_large` as above.
- `JSON_TAKES_GEO_NAMES = a_json_value_takes_every_form_and_a_geo_value_names_its_own_member`
  — a `json` field holds `1`, `"a"`, `true`, `null`, `[]` and `{}`; a
  `geo` field answers `geo_not_object` for a string, `geo_no_type` for
  `{}`, `geo_unknown_type` for an object naming a type GeoJSON does not
  have, `geo_no_coordinates` and `geo_coordinates` for a `Point` missing
  or malformed, and `geo_no_geometries` for an empty `GeometryCollection`;
  no value in the row answers `value_shape`.

Both rows see a `Constraint` with its field through `refused_on`
(`crates/lakelands-core/src/conformance/fixture.rs`): the suite's
`refused_as`, `refused_like` and `invalid_like` are for `Invalid` only,
and `refused_in` is the hole described above. `Case` is in the prelude;
`Shape` is not, so the rows add one import line to
`crates/lakelands-core/src/conformance/prelude.rs`.

### The build plan

| Step | What it does | Files | Product | Spec |
|---|---|---|---|---|
| P1 | the two rows, and the `Shape` re-export | `core/src/conformance/checks/values.rs`, `core/src/conformance/prelude.rs` | 0 | 60 |
| **Sum** | | | **0** | **60** |
| **×1.35** | | | **0** | **81** |

Measured against the tree at the branch's base (`scripts/gates/core-size.sh`):
product 114 998 of 115 000 (2 free), specification 202 995 of 203 000
(5 free). Product does not move: both files are specification, and no
product line changes. The specification needs 60 lines and has 5 free.

**Cut first, with what each costs:**

1. The `geo` half of the second row (≈ 25 spec lines): cut it and `geo`
   keeps today's assertion, "a refusal, of some kind", and no row says
   that no geo value answers `value_shape`. The room is not yielded:
   60 − 25 leaves 30 over the 5 free.
2. The `json` half (≈ 7): cut it and no row says a `json` field takes
   every form. The room is not yielded either.
3. The nine-kind table (≈ 30): not to cut. The issue's Done-when is met
   by no part of this change without it, and it is the one place where
   the answer by kind is said.

The cut does not yield the room, so the build takes a budget row.
Under `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`
the build adds to `docs/budgets.md`:

`| 2026-11-07 | specification | 203 000 → 203 500 | lakelands#1644 | A check that pins the kind and the form a wrong-kind value is refused with, and that no value is told |`

It is decisive under that record's first condition — a check that pins a
rule of a refusal's kind — cutting came first and is named above, and the
need, 60 lines, is rounded up to the next 500. The row is added by the
build's pull request, with a maintainer's word on lakelands#1644; this
record states it and does not record it.

### Open for the operator

- The three cases that already carry a written value (`not_decimal`,
  `not_date_time`, `not_date`) tell a value to a reader of a derived
  field who may not read what produced it, by the same path as above.
  Whether they stop is a breaking change to shipped codes and wants its
  own record; this record does not make it.
- The budget row above needs a maintainer's word on lakelands#1644.
