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
