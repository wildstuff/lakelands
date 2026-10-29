# Contributing to Lakelands

Thank you for considering a contribution. The code is here: 0.21.0 is
released, and 0.22.0 is being built on `develop`. A contribution is a
pull request against `develop`, every commit signed off (below). The
pages under `docs/` are the specification, and the conformance suite
holds the code to them, so a change of behaviour changes a page and a
check along with the code.

Start from an issue. A small fix may come straight as a pull request;
anything larger is talked through on its issue first, so the work fits
the design before it is written. Issues labelled `good first issue` are
a way in.

## Developer Certificate of Origin

Lakelands does not use a contributor licence agreement. Every commit is
signed off under the [Developer Certificate of Origin 1.1](https://developercertificate.org/):
by adding a `Signed-off-by:` line you certify that you wrote the change
or have the right to submit it under the project's licence, Apache-2.0.

```
git commit -s
```

adds the line for you. The `DCO` check is required on `develop` and
`main`: every commit in a pull request, merge commits aside, must carry
a `Signed-off-by:` line naming its author exactly as git records them,
`Name <email>` (case does not matter). A commit made with a different
name or email than the sign-off fails it. To fix the last commit, run
`git commit --amend -s --no-edit` and force-push the branch; for several
commits, `git rebase --signoff origin/develop` and force-push. The
`gate (dco)` job applies the same rule, so `scripts/gates/dco.sh` shows
the result before you push.

## Building and testing

The tree is built and tested on Linux in CI, and it builds and passes
its tests on macOS as well. You need:

- Rust: the toolchain comes from `rust-toolchain.toml`, which rustup
  installs on first use. The workspace's `rust-version` (1.89) is the
  floor the `msrv` job proves. A toolchain bump is one pull request that
  moves the file and the CI pins (`ci.yml`, `release.yml`) together;
- `cargo-nextest`, which `scripts/conformance.sh` runs
  (`cargo install cargo-nextest --locked`);
- for the PostgreSQL engine's suite, a PostgreSQL you run yourself,
  named by `LAKELANDS_TEST_PG_URL` (see `docs/conformance.md`); without
  it the pg tests stand down and `scripts/conformance.sh pg` refuses to
  run;
- bash: `scripts/conformance.sh` runs under bash 3.2 and later, so the
  system shell of macOS will do. The gates in `scripts/gates/` need
  bash 4 or later and `perl`; on macOS, `brew install bash`.

Then, from the repository's root:

```
cargo build --workspace
cargo nextest run --workspace        # every unit test and integration test
scripts/conformance.sh mem           # the conformance suite on one engine
scripts/conformance.sh               # on every engine of conformance/roster
```

The suite is the specification: every engine must pass all of it, and
CI runs it on each. Start with `mem`, which needs nothing else, then
`sqlite`; `pg` needs `LAKELANDS_TEST_PG_URL`. A unit test lives in a
`tests.rs` file beside its module, never inline: the `core-size` gate
counts tests by path, and refuses one in a product file.

## The gates

Every pull request runs the gates in `scripts/gates/`, each as its own
CI job. Run them before you push, under the sort order CI uses:

```
LC_ALL=C scripts/gates/all.sh
```

Each is red when:

- `no-external-platform-deps`: a `Cargo.lock` names a package of the
  platform Lakelands was extracted from, or a manifest path leaves the
  repository;
- `cfg-budget`: there are more `cfg(feature)` sites than its budget, or
  a feature is not an engine's name;
- `vocabulary`: a word of that platform stands in code;
- `single-catalog`: a tool name is written as a string outside the one
  generator in `lakelands-api`;
- `seam-width`: the store facade and the engine traits have more methods
  than their budget;
- `core-size`: the core crates' product code or specification is over
  its budget, or a product file holds a test;
- `conformance-required`: an engine crate is missing from
  `conformance/roster`, or CI does not run the suite on a required one;
- `dco`: a commit is not signed off by its author;
- `no-external-decisions`: a file or a commit message cites that
  platform's decision records or planning threads, or holds a bare `#`
  followed by four or more digits (write `lakelands#N` or a full URL;
  GitHub's merge subject on a message's first line is exempt);
- `doc-heads-its-item`: a new item is inserted between another item and
  its doc comment;
- `detail-sources`: an error's free text is made from a library's words
  outside the files the gate lists;
- `refusals`: `docs/refusals.md` is not what the catalog says, or a
  shipped refusal code is gone without its row;
- `records-resolve`: a path or pointer to a decision record names none;
- `vectors-kept`: the test vectors or kept lakes of a shipped log format
  changed;
- `conflict-markers`: a file holds an unresolved merge;
- `settings`: `docs/settings.md` is not what the list of settings says
  (`python3 scripts/settings.py --write` makes it), or a setting the
  store reads is not in the list;
- `brand-tokens`: `brand/tokens.css` is not what
  `scripts/brand/tokens-css.pl` makes from `brand/tokens.json`;
- `rights-private`: code outside `lakelands-core` names its `rights`
  module, the module is made public, or code outside the sight reads
  `grants.restricted`; a door asks its `Sight`.

`scripts/gates/README.md` says exactly what each one reads, and each
script's header states its rule.

## Budgets

Four numbers keep the core small enough to read: the core crates'
product lines, their specification lines (the conformance suite and the
unit tests), the methods of the seam, and the `cfg(feature)` sites. Each
is held by a gate, and `docs/budgets.md` lists them.

A number may go down in any pull request, and then stays down. It goes
up only by a recorded rule
([decision 047](docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md)):
the code that needs the room must be decisive (it closes a defect, pins
a rule of rights or of the log's integrity, or carries a contract a host
depends on); cutting comes first; the raise is sized to the measured
need; and it is recorded as a row in `docs/budgets.md`. If your change
needs room, say so in the pull request with what `core-size.sh` measured;
the maintainers decide and size the raise. Do not add the row yourself.

## Review and merging

- The owner named in `.github/CODEOWNERS` reviews every pull request.
  Every review thread is resolved before a merge.
- A pull request merges into `develop` when every check is green, with a
  merge commit; squash and rebase merges are switched off. `develop` and
  `main` take no direct push.
- A release is cut from `develop` and tagged; `docs/development.md`
  describes how.


## Ground rules

- English everywhere: code, comments, commit messages, issues, pull
  requests.
- One change per pull request, described by what it does, not when it
  was done.
- A behaviour claim comes with a test that fails without the change.
  For anything an engine must honour, the test belongs in the
  conformance suite, not beside one engine.
- A change a host must follow in its own code (a variant added to a
  closed enum, a member a struct literal must set, a trait method to
  implement) is named in the pull request; the `host-changes` job lists
  them (`docs/development.md`, releases).
- The host surface (`docs/decisions/056-the-host-surface-changes-by-one-release-of-deprecation.md`)
  loses an item only after a release has shipped it
  `#[deprecated(since = "0.X.0", note = "<what to use instead>")]`: a
  pull request that would remove one deprecates it instead, and names
  what replaces it. A break that cannot wait is a line of
  `scripts/release/surface-accepted`, named in the pull request; the
  `host-surface` job refuses any other.
- The on-disk format is a specification. A change to it is a change to
  the spec document first, with a version bump, and to the code second.
- No new query language, no calls to a model from inside the store, no
  network access the caller did not ask for.

## Reporting a security issue

Please do not open a public issue. [`SECURITY.md`](SECURITY.md) says how
to report one privately.
