# Gates

Nine scripts that hold the rules of `CLAUDE.md` mechanically. CI runs
each one as its own job on every pull request; run them locally before
pushing:

```
scripts/gates/all.sh                  # every gate, one line each
scripts/gates/<gate>.sh               # one gate
GATE_BASE=<rev> scripts/gates/all.sh  # commits since <rev> (default: merge base with origin/develop)
```

Exit status 0 is green, 1 is red (one line per violation), 2 is a gate
that could not run. Each script's header states its rule and exactly
what it counts.

| Gate | Holds |
|---|---|
| `no-external-platform-deps` | nothing from the originating platform in any `Cargo.lock`; no manifest path leaves the repository |
| `cfg-budget` | at most 20 `cfg(feature)` sites; features are engine names only |
| `vocabulary` | the platform's words stay out of `src/`; one of them stays out of the repository and its commit messages |
| `single-catalog` | tool names come only from the generator in `lakelands-api` |
| `seam-width` | the store facade and engine traits have at most 110 methods together |
| `core-size` | core + store + sqlite engine at most 70k lines, against a ratchet base (`core-size.base`) that only moves down |
| `conformance-required` | every engine crate is on `conformance/roster`, and CI runs the suite for every required one |
| `dco` | every commit is signed off |
| `no-external-decisions` | no citation of the platform's decision records, planning threads or issue numbers, in files or commit messages |

A gate that has never been red may be blind. A new gate lands with a
planted violation shown red in its pull request before it goes green.
