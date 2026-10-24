# Gates

One script per rule that review alone would miss, and `all.sh`, which
runs them all. CI runs each gate as its own job on every pull request,
and a red gate keeps the pull request from merging. Run them locally
before pushing:

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
| `cfg-budget` | at most `BUDGET` (in the script) `cfg(feature)` sites; features are engine names only, never `default`, and an optional dependency hides behind `dep:` |
| `vocabulary` | the platform's words stay out of `src/` and every `.rs` under `crates/`; one of them stays out of the repository and its commit messages |
| `single-catalog` | tool names come only from the generator in `lakelands-api`; the generic names are read from the generator's `GENERIC_TOOLS`, so the gate keeps no list of its own |
| `seam-width` | the store facade (`LakeStore`) and every `pub trait` in `crates/lakelands-api/src/engine.rs` have at most `BUDGET` (in the script) methods together, each defined once; `Catalog` and the seams of `seam.rs` are not counted |
| `core-size` | the core crates (the script's `CRATES`) in two budgets, each a ratchet base under a ceiling: the product code within `core-size.product.base` and `PRODUCT_CEILING`, and the specification within `core-size.spec.base` and `SPEC_CEILING`. Every tracked `.rs` under a crate's `src/` and `tests/` is counted. The specification is `src/conformance/**`, every `tests.rs`, everything under a `tests/` directory in `src/` and everything under the `tests/` directory beside `src/`; a product file holding a test, or a `#[cfg(test)]` other than before `mod tests;`, is red. `docs/budgets.md` records every raise |
| `conformance-required` | every engine crate is on `conformance/roster`, and CI runs the suite for every required one |
| `dco` | every commit but a merge carries a `Signed-off-by:` naming its author exactly (`CONTRIBUTING.md`) |
| `no-external-decisions` | no citation of the originating platform's decision records or planning threads, and no bare `#` followed by four or more digits (`lakelands#N` and full URLs are fine), in any tracked file or commit message; GitHub's own merge subject on a message's first line is exempt |
| `doc-heads-its-item` | a new item is never inserted between another item and its doc comment: no added `fn`, `struct`, `enum`, `const`, `static`, `type`, `trait`, `impl` or `mod` whose doc block is headed by an unchanged `///` line |
| `detail-sources` | an error's foreign words (`scrubbed`, by any path or alias) are made only in the product files the gate lists, each with why its words quote no record and no host path, and no product file outside the client reads a closed set (`Error`, `Situation`, `Detail`, `Seam`, `Exchange`, `Flaw`, `Remote`, `Case`, `Place`, `Said`, `Refusal`, `Missing`, `NameProblem`, `NameSource`) from JSON by name |
| `refusals` | `docs/refusals.md` is what the catalog says; no two templates of a closed set can say the same sentence; no code the last release shipped is gone without its row in `scripts/refusals.withdrawn`, renamed or lost a parameter, and no code with a row is in the catalog; no row the last release's page lists as withdrawn has left the list, and that page's "Withdrawn" table is the one the gate reads; no case is built with a parameter made by `format!`; only the wire's reader makes an `Unknown` |
| `records-resolve` | every `docs/decisions/NNN-<slug>.md` path in the tree names a record, every link from one record to another does, and every pointer of a record that moved (`NNN § <heading>`, in a span of two backticks where the heading holds a code span) names a heading its record has; it does not read a record cited by its bare number, a link written with `./` or from a page outside the records, or a link's anchor |
| `vectors-kept` | every vector directory of a format below the newest (`docs/log-format/vectors/<N>/`) is named in `docs/log-format/vectors/kept` with the tag that shipped it, the tag is in the repository, and the directory holds that tag's files under the path named, byte for byte, none more and none less; and every kept lake (`docs/log-format/lakes/<N>/`) is named in `docs/log-format/lakes/kept` with the digest of its files, which it still has, and no line the list held where the pull request branched is changed or gone |
| `conflict-markers` | no tracked file holds a line of an unresolved merge: seven `<` or seven `>` and a space at its start, or seven `=` alone |
| `settings` | `docs/settings.md` is what the list of settings (`lakelands_api::settings::SETTINGS`) says; every lake setting the store reads is listed, and every listed one is read; no product file names a setting's key, or reads the settings, by a literal |
| `brand-tokens` | `brand/tokens.css` is, byte for byte, what `scripts/brand/tokens-css.pl` makes from `brand/tokens.json` |
| `rights-private` | `lakelands_core::rights` stays crate-private: `lib.rs` declares it `pub(crate) mod rights;`, no core file re-exports the module whole or by a glob, and no `.rs` outside `crates/lakelands-core/src/` names a `rights::` path; a door reaches the rules through its `Sight` |

The three budget gates (`core-size`, `seam-width`, `cfg-budget`) let a
number go down by itself. A ceiling or a ratchet base that is higher
than on the pull request's base commit is red, unless the same pull
request adds the row to `docs/budgets.md` that raises it to that number
(`docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`). The
ratchet bases are lowered to their counts at the release, not per pull
request (`docs/budgets.md`).

A gate that has never been red may be blind. A new gate lands with a
planted violation shown red in its pull request before it goes green.
