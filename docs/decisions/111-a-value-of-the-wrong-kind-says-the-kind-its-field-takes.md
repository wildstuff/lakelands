# 111 — A value of the wrong kind says the kind its field takes

A write of a value whose JSON form is not the form its field's storage
kind takes is refused with one case, `value_shape`, which names the kind
the field takes and the form that kind is written in. The refusal names
the field the value was written to.

The case, its sentence and its code are the catalogue's already. This
record states the rule the tree keeps and pins it for every kind, on
every engine. Whether the answer also carries the value the caller gave
is left open on lakelands#1644; this record adds nothing to the answer
and rules out one shape of it, a value a function computed.

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
`geo_coordinates`, `geo_no_geometries` — which says more than "a geo
value must be an object" would, and `Shape` has no `object` form. `null`
in a patch clears the field, as
`null_clears_a_field_whatever_its_value_type_allows` states, and is never
a wrong kind.

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

### The value the caller gave is the open question, and it is on the issue

Today the case names the kind and the form and no value. lakelands#1644
asks for the value given beside them, and no comment on that issue
records a word either way, so this record does not settle it.

The catalogue sets what settling it costs. A case names every parameter
in its template (`crates/lakelands-api/src/error/case.rs`; the gate reads
the same in `scripts/refusals.py`), and a parameter of text is a name or a
value the caller gave, never a sentence built elsewhere. A `value`
parameter therefore arrives with a segment that renders it, which
lengthens the sentence; the issue asks that the sentence stay as it is.
Adding an optional parameter is the one thing a shipped code may gain, so
either answer is still open, and one shape is excluded: the value is
carried where the caller gave it and never where a function computed it —
`read_output` (`crates/lakelands-store/src/backed.rs`) runs the same
check on a function's output, which is not the value the caller gave.

### The contract changes not at all on the wire; the specification gains the guarantee

No case is added, renamed or retired, no sentence is reworded, and no
parameter is gained or lost. Every refusal answered today renders the
same bytes after this change. A host that shows the message may switch on
`code` with `kind` and `shape`; one that already does is unchanged.

What changes is inside the repository. The case is answered by product
code that no row pins: `refused_in`
(`crates/lakelands-core/src/conformance/checks/values.rs`) accepts any
`Constraint` or `Invalid` on field `v`, so an edit that changed the case,
the sentence or the field would ship without a red run, and the only row
naming `value_shape` is a view measure's
(`crates/lakelands-core/src/conformance/checks/landing.rs`). The rows
below make the answer a behaviour of the contract, checked on every
engine.

### Conformance rows

Two rows, each a `check!` in
`crates/lakelands-core/src/conformance/checks/values.rs`, each run on
every engine:

- `WRONG_KIND_SAYS_THE_KIND = a_value_of_the_wrong_kind_says_the_kind_its_field_takes`,
  described as "a wrong-kind write on every kind answers one case, names
  the field, and says the kind and the form". For each of the nine kinds
  above, declare a one-field type of that kind (a vector declaring its
  dimensions), write the wrong form, and assert that the answer is a
  refusal whose `reason` is `Refusal::Constraint`, that `refused_on`
  gives field `v`, that the said case is exactly
  `Case::ValueShape { kind, shape }` (compared as the case, not as text),
  that the sentence is the case's, and that the head did not move. The
  same row asserts where `value_shape` stops: a right-form value with
  wrong content answers `not_decimal`, `not_date_time`, `not_date`,
  `not_base64`, `vector_length` or `integer_too_large` as above.
- `JSON_TAKES_GEO_NAMES = a_json_value_takes_every_form_and_a_geo_value_names_its_own_member`,
  described as "a json field takes every form, and a geo field answers
  its own member". A `json` field holds `1`, `"a"`, `true`, `null`, `[]`
  and `{}`; a `geo` field answers `geo_not_object` for a string,
  `geo_no_type` for `{}`, `geo_unknown_type` for an object naming a type
  GeoJSON does not have, `geo_no_coordinates` and `geo_coordinates` for a
  `Point` missing or malformed, and `geo_no_geometries` for an empty
  `GeometryCollection`; no value in the row answers `value_shape`.

Both rows see a `Constraint` with its field through `refused_on`
(`crates/lakelands-core/src/conformance/fixture.rs`): the suite's
`refused_as`, `refused_like` and `invalid_like` are for `Invalid` only,
and `refused_in` is the hole described above. `Case` is in the prelude;
`Shape` is not, so the rows add one import line to
`crates/lakelands-core/src/conformance/prelude.rs`.

## Why

A host renders a refusal in its user's language from the case, and a
closed set of cases with stable codes is what makes that possible
(`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
A store sentence that a host must parse, or show untranslated, is a host
contract unkept. The case already says the right thing; the gap is that
nothing holds it to saying it.

The field is on the refusal because the refusal is already about a field
and a case is a sentence. What the answer does not say is not said
nowhere: the case says the kind and the form, the refusal says the field,
and the caller holds the value it sent — which is why nothing is lost by
leaving the value's place to the operator.

## What it is not

- It is not a new case, a new parameter or a reworded sentence, and
  nothing on the wire moves.
- It is not a decision on the value given: that question is on the
  issue. The one shape it rules out is a value a function computed,
  which is not the value the caller gave.
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

- lakelands#1644 read the catalogue at `v0.22.0` as having no case for a
  wrong-kind value and asked for a new one carrying the field, the value
  given and the kind. Measured, the tree disagrees: `value_shape` is in
  the catalogue at `v0.22.0` (the row is in `docs/refusals.md` at that
  tag), it was added by lakelands#650, its sample is in `samples.json`,
  and `check_values` already makes it a constraint naming the field. The
  issue's own example carries `field` beside the reason and omits `code`,
  which the catalogue has rendered since lakelands#650. The record
  therefore keeps the case and the field, pins the answer by kind on
  every engine, and leaves the value to the issue.
