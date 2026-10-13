# 056 — The host surface changes by one release of deprecation

## What is decided

### The host surface is named

A host builds on these items:
- `lakelands-api` and `lakelands-client`, whole;
- `lakelands-store`'s doors: `open`, `import` and the `Imported` it
  answers, `written_in`, and `ExprBacking`;
- the engines' openers: `MemoryOpener`, `SqliteOpener` and `PgOpener`,
  and `lakelands_engine_pg::MIN_SERVER_VERSION`;
- seven items of `lakelands-core`: `value::check`, `derive::reads`,
  `expr::{check, parse, Place}`, `log::record_key` and
  `names::is_identifier`.

Every other public item of these crates is either private or
`#[doc(hidden)]`. A hidden item is shared by the workspace's crates, its
tests and the conformance suite, and a release changes it without notice.
An item added to the surface carries a doc comment, since `missing_docs`
is denied wherever CI builds.

### A surface item is removed only after a release has marked it deprecated

A surface item is removed, or changed so that a host's code stops
building, only after one release has shipped it
`#[deprecated(since = "0.X.0", note = "<what to use instead>")]`.

An addition is free, and that includes the additions a tool counts as
breaking:
- a variant or a variant's field;
- a field of a struct a host may build;
- a variant's number moved by one added before it.

No deprecation can come before an addition, and the release notes'
"Changes a host must make" name each one. A behaviour that a type does
not show is said in the same place. An enum that a host names but does
not match on, such as `expr::Place`, is `#[non_exhaustive]`.

### CI refuses any other change, against the last release

The `host-surface` job runs `scripts/release/surface-held.sh`. That
script runs `cargo semver-checks` against the release named in
`scripts/release/surface-accepted`, over the surface crates, together
with `scripts/release/host-changes.sh`'s own checks, which find a type
changed in place where the tool does not. It refuses each change that
is none of these:
- an addition;
- the removal of an item the release marked deprecated;
- a line of the list.

It also refuses a line of the list that no change answers.

The list is the reviewed exception: a change the pull request names,
and the next release's notes name. Once a release is tagged, the
baseline becomes the new tag and the list empties. `scripts/release/surface-held-test.sh`
holds the judgement to a fixture.

## Why

A host pins a release and moves when it chooses. A deprecation gives it
one release to move off an item while its code still builds, and the
note tells it where to go.

Holding the surface needs a surface small enough to hold. Before this
record, every `pub` item of core and store counted: a rule over all of
them would have frozen every internal refactor for a release, for items
no host uses. The named surface is what the one real host uses, together
with what a host needs to open, import and serve a lake.

A check that is not blocking is read only at a release, which is too
late for the pull request that broke a host.

## What it is not

- **Not semver for 1.0.** Under 0.x, a minor release may still change
  the surface, but only by this rule. The list makes every exception
  explicit and reviewed.
- **Not a promise about hidden items.** An engine outside this
  repository builds on the seam in `lakelands-api` and on the conformance
  suite. The core functions that `docs/seams.md` names as definitions are
  the repository's own.
- **Not a promise about behaviour beyond the types.** A changed answer
  that compiles is said in the notes, not refused by the job.
