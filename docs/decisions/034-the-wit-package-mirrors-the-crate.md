# 034 — The WIT package mirrors the crate

## What is decided

### The WIT follows the crate, type for type and door for door

`wit/` holds the package `lakelands:data`, the component-model binding
of the contract, at the number its last rule below gives it: the
`types` every interface shares, `store` (the open lake, its data doors,
ingest and the change feed), `query` (the pattern query and search) and
`schema` (describe, declare, apply, add a field, migrate), with a
`client` world that imports them and a `provider` world that exports
them.

`lakelands-api` is the source and the WIT follows it, type for type and
door for door. Where the component model cannot say what the crate
says, one rule holds for the whole package: a JSON value, and the typed
form of an expression, travel as JSON text; a map is a list of named
entries; a generic reply is one record per payload; a stream is a list.

### A test fails when the two drift

A test in `lakelands-api` parses the package and fails when the two
drift: a door of `Catalog`, `LakeStore` or `ChangeFeed` without a WIT
function (or the reverse), a record whose fields differ from its
struct's serde members, an enum or variant whose cases differ. It is
built so that a new field or variant in the crate stops the test from
compiling until the WIT and the test follow.

### A case travels as its code and its parameters

`unbound` and `unavailable` carry a `said`: the places it is said at as
JSON text (`[]` for these two, which name none; a refusal's case names
them), the case's `code`, its parameters as JSON text, and its `message`
(015, extended). The refusals `invalid`, `constraint`,
`precondition`, `declined` and `unanswered` carry the same record, with their
places in `at`, and the drift test reads a case's parameters, which serde
writes beside its code, into `params`. The crate's
`Seam` and `Situation` are closed sets with parameters, and `Situation`
holds other closed sets (a `Flaw`, an `Exchange`, a remote end), about a
hundred and fifty cases in all. Mirrored case for case, the WIT would
grow by each new case, and every new case would make a new package
version. A component built against an older package could not read a
newer code at all. As a code and JSON text it reads every code, renders
one it does not know by its message, and the package moves only when
the record does. This is the rule of JSON text, applied where the
component model could say it but only at a size that defeats the drift
test's purpose.

### The package takes the number of the release that first ships it

The package's version moves with the contract's shape, not with
Lakelands' releases: a change a WIT caller would see is a new package
version. A caller sees only what a release ships. So the changes made
between two releases join the one package version the next release
ships, without a version of their own. The package takes the number of
the release that first ships it, so a caller who reads a package
version knows which release it came with. A `0.3.0` package never
existed, and none is made after the fact.

## Why

A component written in any language should be able to use a lake, or
keep one, through the same contract the Rust crate states, without a
second design to learn. Two descriptions of one contract drift unless
something holds them together; making the crate the only source, and a
test the thing that notices, keeps the WIT honest without anyone having
to remember it.

JSON text is the price of WIT having no recursive and no dynamic types.
Paying it in one stated place, the same way everywhere, is better than
a different workaround per type.

## What it is not

- It is not a second contract. A behaviour is specified in the crate,
  the conformance suite and the documents; the WIT only carries it.
- It is not built into anything yet. No Lakelands crate compiles the
  WIT; a host that wants a component binding generates it.
- It is not a trust boundary. Every door names its principal, as the
  crate's do; a host that serves components it does not trust resolves
  the principal itself.

## How it came to be

- The first version: `wit/` held `lakelands:data@0.1.0`, and the
  package's version moved with the contract's shape.
- The package moved to `0.2.0`: a field's provenance gained `error`,
  the reason a field computed by a function holds no value when its
  backing failed (010, extended), which a WIT caller sees in
  `field-meta`.
- One unreleased version gathers its changes: `lakelands:data@0.2.0`
  stayed `0.2.0` until `v0.2.0` shipped, and `field-decl` took a field
  declaration's `embed` (022, extended) at `0.2.0`.
- `v0.3.0` shipped a WIT a caller could tell from `0.2.0` (`find`
  resumed with `after` and ordered by `latest`, a type's `sorted`
  fields) under the same `lakelands:data@0.2.0`, which broke the rule.
- The package moved once, from `0.2.0` to `0.4.0`, gathering both
  releases' changes (`v0.4.0` added `upstream` on a record and an
  ingest, `declare` and `migrate` proposals with their `proposal-kind`,
  `degree` and `components`, and a describe that answers the
  declaration whole), and takes the number of the release that first
  ships it since.
- A case travels as a `said`, for `unbound` and `unavailable` and for
  the refusals; the record had its final shape, so the package changed
  once for both, to `0.12.0` (lakelands#650).
- Corrected at the rewrite (lakelands#817): the record named the
  package by a number (`0.1.0`, and in its last step `0.12.0`) that the
  next change of the package made false (a row's `as` moved it to
  `0.13.0`, lakelands#680). It names the package and its rule; the
  number is `wit/`'s.
