# Contributing to Lakelands

Thank you for considering a contribution. Until the code lands (see the
status note in the README), the useful contributions are reviews of the
specifications and the seam contracts as they appear here.

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

## Ground rules

- English everywhere: code, comments, commit messages, issues, pull
  requests.
- One change per pull request, described by what it does, not when it
  was done.
- A behaviour claim comes with a test that fails without the change.
  For anything an engine must honour, the test belongs in the
  conformance suite, not beside one engine.
- The on-disk format is a specification. A change to it is a change to
  the spec document first, with a version bump, and to the code second.
- No new query language, no calls to a model from inside the store, no
  network access the caller did not ask for.

## Reporting a security issue

Please do not open a public issue. Write to security@wildstuff.com.
