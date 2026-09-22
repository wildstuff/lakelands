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

adds the line for you. Pull requests whose commits lack a sign-off are
not merged; a bot checks it.

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
