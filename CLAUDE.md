# CLAUDE.md — how an agent works in this repository

Lakelands is an ontology-driven, event-sourced store for AI agents, built
greenfield here. Nothing is installable yet; the repository holds
commitments and a plan, and the work is turning them into code without
importing the shape of the platform the organs come from.

## Read first, in this order

1. `README.md` and `CONTRIBUTING.md`. Commit with `git commit -s` (DCO);
   a PR without sign-off does not merge.
2. Issue #20 (pinned) — the plan of record — then the issue you hold.
   Do not re-litigate the plan in code; refine it in issue comments.
3. `CLAUDE.local.md` if present (gitignored): where the source organs
   are read from, and background material that must never be cited.

## The rules the gates enforce

- **Vocabulary.** `tribe`, `WILD_`, `chief`, `elder`, `ROOT_TRIBE` do
  not occur in `src/`. The tenant is a *lake*; configuration arrives
  through `Location` and server config, never the environment.
- **No external decisions.** A decision-record citation of the source
  platform (its three-letter prefix, a dash, four digits), a planning
  thread reference, and a bare hash followed by four or more digits
  (anything but `lakelands#N` or a full URL) occur nowhere:
  not in code, comments, docs, WIT, commit messages, PR bodies or
  issues. Lakelands' rationale is its own, in `docs/decisions/`.
- **One catalog.** Every LLM-facing tool name is produced by the catalog
  generator; a string-literal tool name outside it fails.
- **Budgets.** `cfg(feature)` sites ≤ 20 (features are one per engine);
  store facade + engine traits ≤ 110 methods; the core crates' product
  code ≤ 37.5k lines and their specification (the conformance suite and
  unit tests, which live in `tests.rs` files, never inline) ≤ 45k. Each
  goes down by itself and is raised only by the rule in
  `docs/budgets.md`: a decisive reason, cutting first, a measured size,
  and a recorded row. The gates refuse a raise without its row.
- **The conformance suite is the specification.** A behaviour claim goes
  into the suite, not beside one engine; it runs on every engine in CI.
- **Licence.** `cargo deny` on every PR; no copyleft-in-a-library.

A gate that has never been red may be blind: when you add one, plant a
violation, show the red run in the PR, then make it green.

## Import discipline (the core ladder)

- One organ = one squashed commit, "import `<module>` from the source
  at `<short sha>`", already transformed: tenant word → `lake`; caller
  types → `Principal` / `WriteContext`; reserved-type roster → the
  `sys.` namespace; locale enum → BCP-47 tag string; profile paths →
  `Location`; environment reads → config; sync/async bridge → async.
- The never-imported list in issue #20 is binding. A module in neither
  table: stop and ask before importing it.
- **Citations do not cross.** Every reference to the source platform's
  decision records in imported doc comments is removed, and the
  rationale it pointed at is restated in Lakelands' own words — in the
  module doc where local, in `docs/decisions/` where it is a design
  commitment — or dropped if it cannot be stated without that context.
  This is review work, not a `sed`. The PR names the decision records
  it added or extended.
- Every core PR carries the ledger row: lines in, lines out, what was
  cut, which gate numbers moved.
- Bug fixes made to the frozen source after the freeze date are listed
  in `docs/port-log.md` and ported before the platform switches.

## Working rules

- Branch per issue, PR to `develop`, merge-commit only (the ruleset
  enforces it). English everywhere.
- Parallel agents: one worktree per agent, one branch per issue —
  the layout and the slot helper are in `docs/development.md`.
- The core ladder (O0 → O6) is serial: one agent, one branch at a time.
  The door, spec and infra lanes run in parallel after O1 merges, each
  against `lakelands-api` with the in-memory mock.
- No new query language, no model calls inside the store, no network
  the caller did not configure.
- If the plan and the tree disagree, the tree wins and the plan is
  corrected in the same PR — say so in the PR body.
- Scope is `0.1.0` as issue #20 states it. Anything labelled
  `after-0.1.0` is not started early, however small it looks.
