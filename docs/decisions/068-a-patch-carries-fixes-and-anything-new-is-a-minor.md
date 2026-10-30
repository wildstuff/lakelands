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
its log format is the one `0.X.0` writes. The one case where this does
not hold is an item the operator admits by an exception (below).

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

### An exception is the operator's decision, named per release

A release may carry an item that is new, as a patch, only if the
operator decides so for that release. The decision names the release and
the item, and does not carry over to the next patch.

An exception admits only additive, opt-in items that a host who does not
use them never sees. Such an item does not change the log format, the
projection layout or any existing answer. An exception may require at
most a mechanical source change that the release notes name exactly,
such as a new field in a public options struct a host writes out in
full. It never changes a stored lake, the log format or an existing
answer. The release notes name each such item under a heading of its
own.

`0.21.1` is the first such release. Its one item is
`defer_computed_fields` (lakelands#1576), which an embedding host needs,
and its epic is lakelands#1527. An item that would change the log format
is never such an exception: lakelands#1584 was moved to `0.22.0` for that
reason, because a `0.21.0` binary could not have read its events.

### Milestones follow the rule

A fix is planned for the next patch of the newest minor release; new
work is planned for the next minor. A round's milestone that holds both
is cut in two before its first release. How a patch is made, from the
tag on `main` while `develop` holds the next minor, is
`docs/development.md` § A patch release.

## Why

A host pins Lakelands as `0.X`, so it takes every patch without asking.
A patch must therefore never ask anything of it: no new code to write,
no new behaviour to learn, no lake to convert, except in an exception
the operator names. A fix that waits for a
minor release waits behind new work it does not need. A security fix,
above all, ships when it is ready, as `SECURITY.md` promises.

An exception exists because an item can be small, additive and needed
by a host now, while the minor release that would carry it is a round
away; making that host wait, or carry a fork, costs more than one named
line of mechanical source change for the hosts that write the struct out
in full. The bounds above keep the cost there: a stored lake, the log
format and every existing answer stay as the patch's `0.X.0` left them,
so a host that takes the patch and does not use the item changes nothing
but that line.

## What it is not

- **Not a version scheme for 1.0.** Under `0.X`, a minor release may
  still change the host surface by the rule of 056.
- **Not a promise of a cadence.** A minor release ships when its new work
  is in, not on a calendar; a patch ships when its fix is ready.
- **Not a backport promise.** Only the newest minor release is patched,
  as `SECURITY.md` says.
