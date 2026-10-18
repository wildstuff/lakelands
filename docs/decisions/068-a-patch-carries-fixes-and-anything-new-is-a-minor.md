# 068 — A patch carries fixes, and anything new is a minor release

## What is decided

### A patch release carries fixes only

A patch release `0.X.Y` changes nothing a host can see except what was
wrong. It carries:
- a security fix;
- a defect's fix: an answer, a refusal or a write that did not do what
  the specification says;
- a performance fix that changes no answer;
- documentation.

A host moves from `0.X.Y` to `0.X.Y+1` without changing its code, its
configuration or its lakes. Its "Changes a host must make" are none, and
its log format is the one `0.X.0` writes.

A fix takes only what its defect needs. A security fix that cannot be
made without an item a host sees, such as a new refusal kind, still ships
as a patch, and its notes name that item. Any other fix that needs one
waits for the next minor release.

### Anything new is a minor release

A minor release `0.X.0` carries everything else:
- anything new, however small: a door, a field, an option, an answer or
  a refusal a host can see;
- every change of the log's format;
- every change of the host surface that the rule of 056 lets through,
  and every change of behaviour a host must act on.

A release that holds one such change is a minor release, whatever else
it holds.

### Milestones follow the rule

A fix is planned for the next patch of the newest minor release; new
work is planned for the next minor. A round's milestone that holds both
is cut in two before its first release. How a patch is made, from the
tag on `main` while `develop` holds the next minor, is
`docs/development.md` § A patch release.

## Why

A host pins Lakelands as `0.X`, so it takes every patch without asking.
A patch must therefore never ask anything of it: no new code to write,
no new behaviour to learn, no lake to convert. A fix that waits for a
minor release waits behind new work it does not need. A security fix,
above all, ships when it is ready, as `SECURITY.md` promises.

## What it is not

- **Not a version scheme for 1.0.** Under `0.X`, a minor release may
  still change the host surface by the rule of 056.
- **Not a promise of a cadence.** A minor release ships when its new work
  is in, not on a calendar; a patch ships when its fix is ready.
- **Not a backport promise.** Only the newest minor release is patched,
  as `SECURITY.md` says.
