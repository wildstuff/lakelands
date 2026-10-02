# Gates

Nine scripts that hold the rules of `CLAUDE.md` mechanically. CI runs
each one as its own job on every pull request; run them locally before
pushing:

```
scripts/gates/all.sh                  # every gate, one line each
scripts/gates/<gate>.sh               # one gate
GATE_BASE=<rev> scripts/gates/all.sh  # commits since <rev> (default: merge base with origin/develop)
```

The gates need bash 4 or later, `git` and `perl`; the system bash of
macOS is 3.2, so install a newer one there (`brew install bash`).

Exit status 0 is green, 1 is red (one line per violation), 2 is a gate
that could not run. Each script's header states its rule and exactly
what it counts.

| Gate | Holds |
|---|---|
| `no-external-platform-deps` | nothing from the originating platform in any `Cargo.lock`; no manifest path leaves the repository |
| `cfg-budget` | at most 20 `cfg(feature)` sites; features are engine names only |
| `vocabulary` | the platform's words stay out of `src/` and every `.rs` under `crates/`; one of them stays out of the repository and its commit messages |
| `single-catalog` | tool names come only from the generator in `lakelands-api` |
| `seam-width` | the store facade (`LakeStore`) and the engine traits have at most 110 methods together |
| `core-size` | api + core + store + sqlite and pg engines (`lakelands-api`, `lakelands-core`, `lakelands-store`, `lakelands-engine-sqlite`, `lakelands-engine-pg`) in two budgets, each against a ratchet base that only moves down: the product code at most 36k lines (`core-size.product.base`), and the specification at most 40k (`core-size.spec.base`), which is `src/conformance/**`, every `tests.rs` and everything under a `tests/` directory in `src/`; a product file holding a test, or a `#[cfg(test)]` other than before `mod tests;`, is red |
| `conformance-required` | every engine crate is on `conformance/roster`, and CI runs the suite for every required one |
| `dco` | every commit is signed off |
| `no-external-decisions` | no citation of the platform's decision records, planning threads or issue numbers, in files or commit messages |

A gate that has never been red may be blind. A new gate lands with a
planted violation shown red in its pull request before it goes green.
