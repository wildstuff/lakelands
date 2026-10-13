# Contributing to Lakelands

Thank you for considering a contribution. The code is here: 0.13.0 is
tagged, and 0.14.0 is being built on `develop`. A contribution is a pull
request against `develop`, every commit signed off (below). The pages
under `docs/` are the specification, and the conformance suite holds the
code to them, so a change of behaviour changes a page and a check along
with the code.

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

- Rust stable, at least the workspace's `rust-version` (1.88);
- `cargo-nextest`, which `scripts/conformance.sh` runs
  (`cargo install cargo-nextest --locked`);
- for the PostgreSQL engine's suite, a PostgreSQL you run yourself,
  named by `LAKELANDS_TEST_PG_URL` (see `docs/conformance.md`); without
  it the pg tests stand down and `scripts/conformance.sh pg` refuses to
  run;
- bash: `scripts/conformance.sh` runs under bash 3.2 and later, so the
  system shell of macOS will do. The gates in `scripts/gates/` need
  bash 4 or later and `perl`; on macOS, `brew install bash`.

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

Please do not open a public issue. Write to security@wildstuff.com.
