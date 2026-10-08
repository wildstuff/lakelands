# 034 — The WIT package mirrors the crate

## What is decided

`wit/` holds `lakelands:data@0.1.0`, the component-model binding of the
contract: the `types` every interface shares, `store` (the open lake,
its data doors, ingest and the change feed), `query` (the pattern query
and search) and `schema` (describe, declare, apply, add a field,
migrate), with a `client` world that imports them and a `provider`
world that exports them.

`lakelands-api` is the source and the WIT follows it, type for type and
door for door. Where the component model cannot say what the crate
says, one rule holds for the whole package: a JSON value, and the typed
form of an expression, travel as JSON text; a map is a list of named
entries; a generic reply is one record per payload; a stream is a list.

A test in `lakelands-api` parses the package and fails when the two
drift: a door of `Catalog`, `LakeStore` or `ChangeFeed` without a WIT
function (or the reverse), a record whose fields differ from its
struct's serde members, an enum or variant whose cases differ. It is
built so that a new field or variant in the crate stops the test from
compiling until the WIT and the test follow.

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
- Its version moves with the contract's shape, not with Lakelands'
  releases: a change a WIT caller would see is a new package version.

## Extended: the package at `0.2.0`

A field's provenance gained `error`, the reason a field computed by a
function holds no value when its backing failed (010, extended). A WIT
caller sees it in `field-meta`, so the package is now
`lakelands:data@0.2.0`.

## Extended: one unreleased version gathers its changes

A caller sees only what a release ships. So the changes made between
two releases join the one package version the next release ships,
without a version of their own. `lakelands:data@0.2.0` stays `0.2.0`
until `v0.2.0` ships. A field's declaration gained `embed` (022,
extended), and `field-decl` carries it at `0.2.0`.

## Extended: the package at `0.4.0`

`v0.3.0` shipped a WIT a caller could tell from `0.2.0` (`find` resumed
with `after` and ordered by `latest`, a type's `sorted` fields) under the
same `lakelands:data@0.2.0`. That broke the rule above. `v0.4.0` adds
more: `upstream` on a record and an ingest, `declare` and `migrate`
proposals with their `proposal-kind`, `degree` and `components`, and a
describe that answers the declaration whole. The package moves once,
from `0.2.0` to `0.4.0`, and gathers both releases' changes. It takes
the number of the release that first ships it, so a caller who reads a
package version knows which release it came with. A `0.3.0` package
never existed, and none is made after the fact.

## Extended: a case travels as its code and its parameters (lakelands#650)

`unbound` and `unavailable` carry a `said`: the places it is said at as
JSON text (`[]` for these two, which name none; a refusal's case names
them), the case's `code`, its parameters as JSON text, and its `message`
(015, extended). The record has its final shape now, so the package
changes once for both. The refusals `invalid`, `constraint`,
`precondition` and `unanswered` carry the same record, with their
places in `at`, and the drift test reads a case's parameters, which serde
writes beside its code, into `params`. The crate's
`Seam` and `Situation` are closed sets with parameters, and `Situation`
holds other closed sets (a `Flaw`, an `Exchange`, a remote end), about a
hundred and fifty cases in all. Mirrored case for case, the WIT would
grow by each new case, and every new case would make a new package
version. A component built against an older package could not read a
newer code at all. As a code and JSON text it reads every code, renders
one it does not know by its message, and the package moves only when
the record does. This is the rule above, applied where the component
model could say it but only at a size that defeats the drift test's
purpose. The package moves to `0.12.0`, the version of the release that
first ships it.
