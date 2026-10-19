# Development — several worktrees at once

Lakelands' maintainers work on more than one issue at a time, each in a
worktree of its own, on one machine. This page describes the local
layout that keeps them out of each other's way. It is a template, not tooling the
repository ships: the helper script below is kept on the developer's
machine because its paths and limits are machine-specific. Copy it and
adapt it.

## Three things must be isolated

| Layer | Primitive | Rule |
|---|---|---|
| Files | `git worktree` | One session per worktree, ever. Two in one working tree overwrite each other silently. |
| Git | branch | One branch per issue, cut from the latest `origin/develop`. A worktree never keeps a long-lived branch of its own. |
| Build output | `CARGO_TARGET_DIR` | One target directory per worktree, so parallel builds never wait on the same `.cargo-lock`. |

Lakelands has no daemon, bus or profile, so nothing else needs
separating today. When a test starts a server on a fixed port (the MCP
HTTP door, a PostgreSQL service), give each worktree its own port range,
derived from its letter. A worktree's PostgreSQL is a container of its own:

```sh
docker run -d --name lakelands-<letter>-pg \
  -e POSTGRES_USER=lakelands -e POSTGRES_PASSWORD=lakelands \
  -e POSTGRES_DB=lakelands -p 127.0.0.1:<port>:5432 \
  pgvector/pgvector:pg16 -c max_connections=300
```

It takes 300 connections, not the image's 100: the CLI's mirror tests
hold six connections each, and a wide machine runs sixteen of them at
once.

The MCP HTTP door speaks plain HTTP in 0.1.0, with no TLS, and API keys
travel in every request, so bind it to loopback or terminate TLS in
front of it.

## The layout

```
~/develop/lakelands               a bare clone — nobody works in it
~/develop/lakelands-a             worktree a, one session
~/develop/lakelands-b             worktree b, one session
…
```

A worktree is named by one lowercase letter. It rests **detached** on
`origin/develop`. It holds a branch only while a task is running.

Each worktree carries two files that are never committed:

- `.claude/settings.local.json` — the coding agent's environment: its own
  `CARGO_TARGET_DIR` and a `CARGO_BUILD_JOBS` cap, so that five worktrees
  compiling at once do not starve the machine, and `CARGO_INCREMENTAL`
  off. Incremental data grows by the tens of gigabytes per target
  directory, and a full run, which rebuilds after every change anyway,
  gains little from it; five worktrees with it on once filled the cache
  disk.
- `CLAUDE.local.md` — instructions for this worktree that don't belong in
  the repository: the worktree's name, where the developer keeps the source
  material the organs are imported from, the commit they are read at,
  and what must never be cited. `CLAUDE.md` tells the agent to read it.

Keep a template of `CLAUDE.local.md` outside the repository (for example
`~/.config/lakelands/CLAUDE.local.md.tmpl`, with `@NAME@` and `@DIR@`
placeholders) and render it per worktree.

## Creating a worktree

A reference version of the helper (`lakelands-worktree-new <letter>`, put it
on your `PATH`):

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="${LAKELANDS_ROOT:-$HOME/develop}"
BARE="$ROOT/lakelands"
URL="${LAKELANDS_URL:-https://github.com/wildstuff/lakelands.git}"
TMPL="${LAKELANDS_LOCAL_TMPL:-$HOME/.config/lakelands/CLAUDE.local.md.tmpl}"
JOBS="${LAKELANDS_JOBS:-3}"

name="${1:-}"
[[ "$name" =~ ^[a-z]$ ]] || { echo "usage: lakelands-worktree-new <a-z>" >&2; exit 2; }
dir="$ROOT/lakelands-$name"
[[ -e "$dir" ]] && { echo "refusing: $dir already exists" >&2; exit 1; }
[[ -f "$TMPL" ]] || { echo "refusing: template $TMPL missing" >&2; exit 1; }

[[ -d "$BARE" ]] || git clone --bare "$URL" "$BARE"
# A bare clone maps heads onto heads; worktrees need origin/* to detach onto.
git -C "$BARE" config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
git -C "$BARE" fetch --prune origin

# The DCO sign-off names the committer, so the clone needs an identity.
git -C "$BARE" config user.email >/dev/null || {
  echo "refusing: no git identity — run: git -C $BARE config user.name <name>; git -C $BARE config user.email <email>" >&2
  exit 1
}

# Shared by every worktree of the clone: local files stay untracked,
# and every commit is signed off (DCO) without anyone remembering -s.
exclude="$BARE/info/exclude"
for pat in CLAUDE.local.md .claude/settings.local.json; do
  grep -qxF "$pat" "$exclude" 2>/dev/null || echo "$pat" >> "$exclude"
done
hook="$BARE/hooks/commit-msg"
if [[ ! -x "$hook" ]]; then
  mkdir -p "$BARE/hooks"
  cat > "$hook" <<'HOOK'
#!/bin/sh
sob="Signed-off-by: $(git config user.name) <$(git config user.email)>"
git interpret-trailers --in-place --if-exists addIfDifferent --trailer "$sob" "$1"
HOOK
  chmod +x "$hook"
fi

git -C "$BARE" worktree add --detach "$dir" origin/develop

mkdir -p "$dir/.claude"
cat > "$dir/.claude/settings.local.json" <<JSON
{
  "env": {
    "CARGO_TARGET_DIR": "$HOME/.cache/cargo-target-lakelands-$name",
    "CARGO_BUILD_JOBS": "$JOBS",
    "CARGO_INCREMENTAL": "0"
  }
}
JSON
sed -e "s|@NAME@|$name|g" -e "s|@DIR@|$dir|g" "$TMPL" > "$dir/CLAUDE.local.md"
echo "worktree $name ready: $dir — start the session there"
```

Then start one session per worktree, from inside that worktree's
directory.

## One task, start to finish

1. **Claim.** Check that nobody holds the issue:
   `gh pr list --state open`, `git worktree list`. Then assign it to
   yourself and push the branch early. The first push owns the issue,
   and a second worktree that finds the branch drops or rebases.
2. **Branch.** `git fetch origin && git switch -c <kind>/<topic> origin/develop`.
   The branch name says what the change is (`core/workspace`,
   `spec/log-format`), never who made it.
3. **Build and test** in the worktree. Run the gates locally before pushing.
   This run stays the author's duty, also after a rebase: the rebased tree
   is built and tested before it is pushed.
4. **PR to `develop`.** Merge commits only; the ruleset refuses anything
   else. A PR merges on green CI and a clean merge tree (see below).
5. **Park.** After the merge:
   `git switch --detach origin/develop && git branch -d <branch>`.
   The worktree is ready for the next issue.

## Rules for a shared clone

Every worktree belongs to one clone on one machine, so the worktrees share
more than the table above suggests. These rules follow from that.

- **Delete only your own branches, after the merge, by exact name.**
  All worktrees share one branch namespace; only a branch checked out
  somewhere is protected. A delete by pattern (`git branch --list
  'spec/*' | xargs git branch -D`) reaches another worktree's unpushed
  work. Name the branch you created, check that it is merged, and keep
  the output visible. A lost tip is found again with
  `git fsck --lost-found` and restored with `git branch <name> <sha>`.
- **`cargo clean` before every run in a target directory that is
  shared.** One target directory belongs to one worktree. When a second
  worktree has to build in the same one (a scratch worktree beside the
  first, a rebuild of someone else's branch), clean it before each run.
  Without that, a test run can pick up what the other tree built and
  report on it without saying so; one such run here tested another
  worktree's planted violation instead of its own code.
- **Merge on green CI and a clean merge tree.** `git merge-tree`
  reporting no conflict says the texts combine, not that the result
  compiles: two branches that each pass can change the same item from
  different files. CI answers that. A pull request's run checks out the
  merge of its head into `develop` and builds and tests that. So a PR
  merges when its run on the current head is green and `git merge-tree`
  against the current `origin/develop` is clean; a local full run is not
  the gate. When `develop` moves under a PR and it conflicts, rebase, run
  the author's own checks on the rebased tree, and push for a new run.
- **A PR number, a head and a run id come from the command's output.**
  Report them only after `gh pr create`, `git rev-parse HEAD` and
  `gh run list` have answered, never from what they are expected to be,
  and never in the same step that runs those commands.
- **Never stop a process by a command-line pattern.** `pkill -f` matches
  every process whose command line contains the pattern: the shell that
  runs it, and another worktree's build or test of the same crate. Stop what
  you started by what only you have, the executable under your own
  target directory:

  ```bash
  for p in /proc/[0-9]*; do
    case "$(readlink "$p/exe" 2>/dev/null)" in
      "$CARGO_TARGET_DIR"/*) kill "${p#/proc/}" ;;
    esac
  done
  ```
- **Remove a throwaway worktree after its merge.** A worktree made for
  one branch, beside the usual one, goes with the branch:
  `git worktree remove <dir>`, and its target directory with it.
- **Figures from a loaded machine say so.** Timings and throughput
  measured while other worktrees build are bounds, not targets. Report them
  with the machine's load average and core count beside them.

## Which work runs in parallel

The plan of record (lakelands#20) decides this. In short, the core
ladder is serial: one worktree holds it at a time. The door, spec and infra
lanes build against `lakelands-api` and test against the store on the
memory engine, so they open up once the contract has merged. Two worktrees that would edit the
same file (for example a CI workflow, or a spec document that the
workspace step creates as a stub) are sequenced, not raced.

## Long runs

The streaming check's long run (25 000 and 100 000 records) runs by hand, one engine at a time, on a release build: `cargo test --release -p lakelands-engine-<mem|sqlite|pg> --test conformance -- --ignored --nocapture` (pg needs `LAKELANDS_TEST_PG_URL`).
Run it while no other worktree is building, and report its lines with the machine's load average: the times are mostly waits on durable commits.

## Removing a worktree

```bash
git -C ~/develop/lakelands worktree remove ~/develop/lakelands-<x>
rm -rf ~/.cache/cargo-target-lakelands-<x>
```

`worktree remove` refuses when the worktree still has uncommitted work, so
look before you add `--force`.

## Releasing

Consumers pin Lakelands by tag, never by bare revision.

A release is an annotated tag `vX.Y.Z` on `develop` or `main`, where
`X.Y.Z` is the workspace version, signed off like a commit:

```bash
git tag -a v0.1.0 -m "Lakelands v0.1.0" -m "Signed-off-by: Your Name <you@example.com>"
git push origin v0.1.0
```

`.github/workflows/release.yml` checks the tag
(`scripts/release/check-tag.sh`), runs the checks of `ci.yml`, builds the
`lakelands` binary and publishes a GitHub release with the binary,
`LICENSE`, `NOTICE`, the third-party licence list and notes made from
the titles of the pull requests merged since the previous tag
(`scripts/release/notes.sh`: the merges on the first-parent path from
that tag to the new one, or, for a patch release, whose `main` took the
tag by a merge, the merges no earlier release tag reaches;
`notes.sh --pulls <tag>` rehearses the list), followed
by "Changes a host must make" since that tag
(`scripts/release/host-changes.sh`: `cargo-semver-checks` over
`lakelands-api`, `lakelands-client`, `lakelands-core`,
`lakelands-store` and the three engines, and the script's own checks of
the same crates, which pass over an item under `#[doc(hidden)]`: a
required trait method added; a trait method, function or inherent
method whose signature changed with its parameter count unchanged; and
a public field, named or positional, whose type changed; one plain line
per change, and a line saying so when there is none). The `host-changes` job of `ci.yml`
reports the same list on every pull request, and never fails; a release
pull request copies its report into its body. The `host-surface` job
blocks: `scripts/release/surface-held.sh` refuses a change of the host
surface that is neither an addition, nor the removal of an item the last
release deprecated, nor a line of `scripts/release/surface-accepted`
(`docs/decisions/056-the-host-surface-changes-by-one-release-of-deprecation.md`).
The notes of a minor release name what the list holds; once its tag
stands, the next pull request sets the file's `baseline` to it and
empties the list. The job pins `cargo-semver-checks` and its compiler together
(`ci.yml`, `host-surface`): the tool reads only the rustdoc formats it
ships, so one moves with the other, in one pull request that shows the
job green on both. The job judges a pull request merged into develop as
develop stood when its CI ran, so before a merge
`scripts/release/surface-fresh.sh <n>` asks whether that judgement still
stands. It is stale when the run has no `host-surface` job (its merge
ref predates the job, and a re-run keeps that ref), or when a merge
since touched the surface crates' sources or manifests, the list or
`surface-held.sh`. A stale pull request is closed and reopened, or
rebased, before it merges; a run still going cannot tell yet (exit 2). A
pre-release `vX.Y.Z-<suffix>` may be tagged on any branch to rehearse
one. Nothing is published to crates.io yet.

### A patch release

What a patch may carry, and what makes a release a minor one, is
`docs/decisions/068-a-patch-carries-fixes-and-anything-new-is-a-minor.md`.
A minor release `vX.Y.0` is tagged on `develop`. A fix to it ships as
`vX.Y.1` on `main`, so that `develop`, which already holds work for the
next minor, does not ship with it.

1. **`main` holds `vX.Y.0`.** If it does not yet, a pull request brings
   it there and adds nothing else:
   - When `main` is an ancestor of the tag, the pull request's head is
     exactly the tagged commit.
   - When `main` already carries an earlier patch (`vX.W.1`), the tag
     does not descend from it, and a head of exactly the tag would
     conflict. A branch `release/main-takes-X.Y.0` starts at the tag
     instead and merges `main` keeping the tag's tree:
     `git switch -c release/main-takes-X.Y.0 vX.Y.0`, then
     `git merge -s ours --signoff origin/main`. `git diff vX.Y.0 HEAD`
     must be empty, and `main` is then an ancestor of the head.
2. **The patch branch starts at the tag:**
   `git switch -c release/X.Y.1 vX.Y.0`.
3. **The fix is fitted to the release's tree.** The tree of `vX.Y.0` is
   not `develop`'s, so the fix is written or adapted for it, with its own
   test, red first. It does not take `develop`'s other changes with it.
4. **The release commit** comes next, as on `develop`: the version in the
   workspace, the lock file, the server transcripts' `serverInfo`, the
   WIT package and the README status.
5. **A pull request from `release/X.Y.1` to `main`.** CI runs on pull
   requests to `main` as it does on those to `develop`. The gates read
   the range from the pull request's base, so the DCO gate checks only
   the commits the patch branch adds, not the history `main` already
   holds.
6. **The tag goes on `main`**, on the merge commit, signed off as above.
   `check-tag.sh` accepts a release on `main`.
   - The tag's message says what the patch fixes, under a line of its
     own that begins with `What` and ends with a colon (`What 0.8.1
     fixes in 0.8.0:`), before a paragraph `At the tag:` with the
     counts. The notes begin with that part, the line as their heading:
     a patch's pull requests are the steps of its release and say
     nothing of what it fixes.
   - The notes list the pull requests merged into `main` since
     `vX.Y.0`: the one of step 1, if it was needed, and the patch's.
     An earlier patch line's pull requests are in `main`'s history too
     and are not listed.
   - "Changes a host must make" compares the same two tags.
7. **`develop` gets the fix through its own pull request**, written
   against `develop`'s tree, with the issue the fix closes. The patch
   branch is never merged into `develop`. `develop` keeps its own version
   until its next minor release.
