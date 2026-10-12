# Log format test vectors, one directory a format

`<N>/` holds the test vectors of log format N: a small lake's log, the
answers `expected.json` and `further.json` say for it, and `verify.py`,
which reads that format alone and shares no code with Lakelands. Each
directory's own `README.md` says what it holds.

The highest number is the format this tree writes (`LOG_FORMAT_VERSION`).
Every lower one is a format a release shipped, which every later release
reads: it is kept as that release's tag holds it, byte for byte, and
never edited. `kept` names each with its tag and its path there, and
`scripts/gates/vectors-kept.sh` holds the directory to it.
`crates/lakelands-core/tests/log_vectors.rs` runs every directory's own
`verify.py` over its own log, and recomputes the newest with the
release's code.

**A format step** (`docs/log-format.md` §6) copies the newest directory
to the next number and runs `python3 verify.py --stamp N` in the copy.
At the release that ships it, the step adds the directory it leaves
behind to `kept`, with that release's tag. The paths that name the
newest directory move with it:
`crates/lakelands-api/tests/helpers.rs`,
`crates/lakelands-core/src/conformance/checks/schema.rs`,
`crates/lakelands-engine-sqlite/tests/lake_directory.rs`,
`crates/lakelands-engine-sqlite/tests/sealed_in_one_step.rs`,
`crates/lakelands-store/tests/export.rs`,
`crates/lakelands-store/tests/lake.rs` and `docs/log-format.md`.
