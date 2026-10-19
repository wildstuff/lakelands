# Security

## Reporting a vulnerability

Please do not open a public issue, pull request or discussion. Report
privately, by either of:

- GitHub's private vulnerability reporting: "Report a vulnerability"
  under the repository's Security tab;
- e-mail to security@wildstuff.com.

Say what you found, the version (`lakelands --version`) and the engine,
and how to reproduce it. A lake's files are not needed and should not be
sent; the commands that build one are enough.

## What happens next

- The report is acknowledged within 7 days.
- The fix is made in private and ships as a patch release, with a GitHub
  Security Advisory published the same day. Disclosure is coordinated
  with the reporter, who is credited unless they ask not to be.
- There is no bounty.

## Supported versions

Before 1.0, only the newest minor release receives fixes: a fix for
`0.X` ships as `0.X.Y`, and an older minor release is not patched.

## In scope

- **The log's integrity.** An event or segment altered, removed or
  forged by hand that Lakelands reads without reporting the break
  (`docs/log-format.md`, the hash chain and checkpoints), or an answer
  that its position and provenance do not hold.
- **Rights.** A write beyond the authority a type declares, or a read,
  refusal included, that shows a principal a field, value or type kept
  from it.
- **Lakes.** One lake reaching another's records.
- **The doors.** What a caller of the MCP or REST door can do beyond its
  principal's rights.
- **Paths.** `--root`, an import or an export that reads or writes
  outside the root it was given.
- **The network.** Any connection the caller did not configure.

## Out of scope

- A host's own implementation of a seam.
- A database server the operator exposes to the network.
- Denial of service by a principal that already holds write rights.
- Anything that needs write access to the lake's files on disk, other
  than an altered log that must be reported as a break.
