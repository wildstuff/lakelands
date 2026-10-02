# 006 — Grants, not roles

## What is decided

The store knows principals and grants, never roles. Every call carries
one `Principal` (kind, id, lake, grants), and the grants say, per field,
what that principal may read and write. The store enforces them on every
read and every write: a field the principal may not read is masked or
absent in the answer, and in the schemas the generated tools expose; a
write carrying a field the principal may not write is refused whole.

Turning a host's identities, roles, teams or policies into grants is the
host's job, done in its `PrincipalResolver`. Lakelands' own default
resolves an API key or a static configuration entry to one principal.

## Why

Roles are policy, and every organisation models them differently. If the
store understood roles, it would have to adopt one model and every host
would have to bend to it. Grants are the common denominator every role
model compiles to, and they are what the store actually needs to decide
a single read or write.

Enforcement has to sit in the store because a rule enforced anywhere
else is skipped by a caller that reaches the store directly. Per-field
grants are also what lets two writers share one record safely (see 012):
each writes only the fields it is granted.

## What it is not

- It is not an off switch. There is no mode in which the store stops
  checking grants; a fully trusted principal is one whose grants cover
  everything.
- It is not authentication. Who the caller is gets decided before the
  store is called, by the resolver.
- It is not partial success. A write is never applied with the
  forbidden fields silently dropped.
