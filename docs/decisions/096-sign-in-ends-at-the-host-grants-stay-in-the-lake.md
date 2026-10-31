# 096 — Sign-in ends at the host; grants stay in the lake

Decided on lakelands#1522, closed as not planned on 2026-10-31.

## What is decided

Lakelands does not sign anyone in. Who a caller is, what that caller may
do in a lake, and whether one read or write passes are three questions
with three owners.

### Three layers, each with one owner

- **Sign-in** answers "who is this?". It is the host's: its identity
  provider, its sessions, its tokens.
- **Translation** answers "what may this caller do in this lake, on this
  call?". It is the host's too, done in its `PrincipalResolver`
  (`docs/decisions/006-grants-not-roles.md`).
- **Enforcement** answers "does this read or write pass?". It is the
  store's, on every read and every write.

### A lake has no user directory

A lake keeps no list of people, passwords, sessions or providers. It
receives one compiled `Principal` per call and judges the call by that
principal's grants.

### A token carries identity, never grants

Grants turn on facts an identity provider does not hold: whether a model
is acting, on whose behalf a handler writes, and what the data allows
that caller. A grant written into a token would be fixed before the call
is known, and would be read from outside the lake that enforces it.

### A principal's id is stable and belongs to the host

The writer of a change stays in the log for good. A provider's subject
used directly as the id would turn the same person into a different
writer after a change of provider, and the log would say two people
wrote what one wrote. The host maps whatever its provider gives to an
id of its own, and keeps that id across providers.

### When it comes back

Only when `lakelands-server` runs with no host in front of it and
someone who runs it that way asks. Then the form is a resolver that
verifies a token against keys the operator configured, with no network
call the caller did not configure. The token selects a configured entry;
the grants stay in the configuration.

## Why

Sign-in is the part of access that differs most between hosts and
changes most often, and the part where a mistake does the most harm.
Every host already has an answer to it. Taking it into the lake would
duplicate that answer, tie the lake to one provider's vocabulary, and
add a surface to keep secure that no rule of the store needs. Grants are
the opposite: they are about the lake's data, so they are judged where
the data is, whichever host sent the call.

## What it is not

- **Not a new contract.** The `Principal`, the `PrincipalResolver` and
  the store's checks are as `docs/decisions/006-grants-not-roles.md`
  has them. This record adds nothing to them.
- **Not a promise of a built-in sign-in.** The last section names the
  one case in which a resolver is built, and the form it takes; nothing
  of it is started.
- **Not grants in a token.** Not in a claim, a scope or a role.
- **Not a directory.** No user table, no password, no session in a lake.
