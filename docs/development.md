# Development — running several agents in parallel

Lakelands is built by more than one coding agent at a time (and by
humans beside them). This page describes the local layout that keeps
them out of each other's way. It is a template, not tooling the
repository ships: the helper script below is kept on the developer's
machine because its paths and limits are machine-specific. Copy it and
adapt it.

## Three things must be isolated

| Layer | Primitive | Rule |
|---|---|---|
| Files | `git worktree` | One agent per worktree, ever. Two agents in one working tree overwrite each other silently. |
| Git | branch | One branch per issue, cut from the latest `origin/develop`. A slot never keeps a long-lived branch of its own. |
| Build output | `CARGO_TARGET_DIR` | One target directory per worktree, so parallel builds never wait on the same `.cargo-lock`. |

Lakelands has no daemon, bus or profile, so nothing else needs
separating today. When a test starts a server on a fixed port (the MCP
HTTP door, a PostgreSQL service), give each slot its own port range,
derived from its letter.

The MCP HTTP door speaks plain HTTP in 0.1.0, with no TLS, and API keys
travel in every request, so bind it to loopback or terminate TLS in
front of it.

## The layout

```
~/develop/lakelands               a bare clone — nobody works in it
~/develop/lakelands-agent-a       slot a, one agent
~/develop/lakelands-agent-b       slot b, one agent
…
```

A slot is named by one lowercase letter. It rests **detached** on
`origin/develop`. It holds a branch only while a task is running.

Each slot carries two files that are never committed:

- `.claude/settings.local.json` — the agent's environment: its own
  `CARGO_TARGET_DIR` and a `CARGO_BUILD_JOBS` cap, so that five agents
  compiling at once do not starve the machine.
- `CLAUDE.local.md` — instructions for this slot that don't belong in
  the repository: the slot's name, where the developer keeps the source
  material the organs are imported from, the commit they are read at,
  and what must never be cited. `CLAUDE.md` tells the agent to read it.

Keep a template of `CLAUDE.local.md` outside the repository (for example
`~/.config/lakelands/CLAUDE.local.md.tmpl`, with `@SLOT@` and `@DIR@`
placeholders) and render it per slot.

## Creating a slot

A reference version of the helper (`lakelands-slot-new <letter>`, put it
on your `PATH`):

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="${LAKELANDS_ROOT:-$HOME/develop}"
BARE="$ROOT/lakelands"
URL="${LAKELANDS_URL:-https://github.com/wildstuff/lakelands.git}"
TMPL="${LAKELANDS_LOCAL_TMPL:-$HOME/.config/lakelands/CLAUDE.local.md.tmpl}"
JOBS="${LAKELANDS_JOBS:-3}"

slot="${1:-}"
[[ "$slot" =~ ^[a-z]$ ]] || { echo "usage: lakelands-slot-new <a-z>" >&2; exit 2; }
dir="$ROOT/lakelands-agent-$slot"
[[ -e "$dir" ]] && { echo "refusing: $dir already exists" >&2; exit 1; }
[[ -f "$TMPL" ]] || { echo "refusing: template $TMPL missing" >&2; exit 1; }

[[ -d "$BARE" ]] || git clone --bare "$URL" "$BARE"
# A bare clone maps heads onto heads; slots need origin/* to detach onto.
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
    "CARGO_TARGET_DIR": "$HOME/.cache/cargo-target-lakelands-agent-$slot",
    "CARGO_BUILD_JOBS": "$JOBS"
  }
}
JSON
sed -e "s|@SLOT@|$slot|g" -e "s|@DIR@|$dir|g" "$TMPL" > "$dir/CLAUDE.local.md"
echo "slot $slot ready: $dir — start the agent there"
```

Then start one agent session per slot, from inside that slot's
directory.

## One task, start to finish

1. **Claim.** Check that nobody holds the issue:
   `gh pr list --state open`, `git worktree list`. Then assign it to
   yourself and push the branch early. The first push owns the issue,
   and a second slot that finds the branch drops or rebases.
2. **Branch.** `git fetch origin && git switch -c <kind>/<topic> origin/develop`.
   The branch name says what the change is (`core/workspace`,
   `spec/log-format`), never who made it.
3. **Build and test** in the slot. Run the gates locally before pushing.
   This run stays the slot's duty, also after a rebase: the rebased tree
   is built and tested before it is pushed.
4. **PR to `develop`.** Merge commits only; the ruleset refuses anything
   else. A PR merges on green CI and a clean merge tree (see below).
5. **Park.** After the merge:
   `git switch --detach origin/develop && git branch -d <branch>`.
   The slot is ready for the next issue.

## Rules for a shared clone

Every slot is a worktree of one clone on one machine, so the slots share
more than the table above suggests. These rules follow from that.

- **Delete only your own branches, after the merge, by exact name.**
  All worktrees share one branch namespace; only a branch checked out
  somewhere is protected. A delete by pattern (`git branch --list
  'spec/*' | xargs git branch -D`) reaches another slot's unpushed
  work. Name the branch you created, check that it is merged, and keep
  the output visible. A lost tip is found again with
  `git fsck --lost-found` and restored with `git branch <name> <sha>`.
- **`cargo clean` before every run in a target directory that is
  shared.** One target directory belongs to one worktree. When a second
  worktree has to build in the same one (a scratch worktree beside the
  slot, a rebuild of someone else's branch), clean it before each run.
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
  the slot's own checks on the rebased tree, and push for a new run.
- **A PR number, a head and a run id come from the command's output.**
  Report them only after `gh pr create`, `git rev-parse HEAD` and
  `gh run list` have answered, never from what they are expected to be,
  and never in the same step that runs those commands.
- **Never stop a process by a command-line pattern.** `pkill -f` matches
  every process whose command line contains the pattern: the shell that
  runs it, and another slot's build or test of the same crate. Stop what
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
  one branch, beside the slot, goes with the branch:
  `git worktree remove <dir>`, and its target directory with it.
- **Figures from a loaded machine say so.** Timings and throughput
  measured while other slots build are bounds, not targets. Report them
  with the machine's load average and core count beside them.

## Which work runs in parallel

The plan of record (lakelands#20) decides this. In short, the core
ladder is serial: one slot holds it at a time. The door, spec and infra
lanes build against `lakelands-api` with the in-memory mock, so they
open up once the contract has merged. Two slots that would edit the
same file (for example a CI workflow, or a spec document that the
workspace step creates as a stub) are sequenced, not raced.

## Long runs

The streaming check's long run (25 000 and 100 000 records) runs by hand, one engine at a time, on a release build: `cargo test --release -p lakelands-engine-<mem|sqlite|pg> --test conformance -- --ignored --nocapture` (pg needs `LAKELANDS_TEST_PG_URL`).
Run it while no other slot is building, and report its lines with the machine's load average: the times are mostly waits on durable commits.

## Removing a slot

```bash
git -C ~/develop/lakelands worktree remove ~/develop/lakelands-agent-<x>
rm -rf ~/.cache/cargo-target-lakelands-agent-<x>
```

`worktree remove` refuses when the slot still has uncommitted work, so
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
the titles of the pull requests merged since the previous tag. A
pre-release `vX.Y.Z-<suffix>` may be tagged on any branch to rehearse
one. Nothing is published to crates.io yet.
