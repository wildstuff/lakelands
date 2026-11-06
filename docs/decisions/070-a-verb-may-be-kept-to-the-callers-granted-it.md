# 070 — A verb may be kept to the callers granted it

## What is decided

Decided on lakelands#1287 by the operator (2026-10-19), for 0.17.0. It
extends 006, whose rule for who may call a verb it narrows by one
grant, and leaves 013, which runs a call by its risk, as it is.

### A verb may be declared `granted`

A verb declares `granted: true`. Such a verb is callable only by a
caller whose grants on the verb's type name it: `TypeGrants` gains
`verbs`, a set of verb names, empty where absent. A verb without the
flag is judged as before, by 006 alone, and a grant naming it changes
nothing.

The grant comes on top of 006, never in its place. A caller granted the
verb still writes each field it sets and reads each field its
`requires` and its set values read; one granted the verb who may only
read a field it sets is refused, as without the flag. One who may write
and read all of them, but is not granted the verb, is refused too.

### An ungranted caller is refused, and not shown the verb

To a caller not granted it, a verb declared `granted` is what 006 makes
a verb whose fields the caller may not write:
- a call answers `not_granted`, before any record is read, so a key no
  record holds is answered `not_granted` too, not `not_found`. Two
  answers come before it, as before every grant a call asks (013): a
  call whose key does not fit the verb's scope is answered `invalid`,
  and a retry under an idempotency key already answered is answered its
  first outcome;
- `propose` of a call, and a call its risk would propose, answer the
  same;
- `describe` names the verb neither on its type nor among the lake's
  verbs, and no MCP tool is made of it, since the tools are made from
  what `describe` answers;
- a call proposal of it is not there to such a reviewer: `proposals`
  leaves it out, and `accept` and `reject` answer as for an id no
  proposal holds. A reviewer granted the verb accepts it, the call
  judged again under the reviewer's grants as every accept is.

The verb is judged as the lake declares it when the call is made, the
proposal read or decided; a verb withdrawn since, as it was declared
where the call was proposed. 006 also judges a proposal by what the
verb read where it was proposed, since an owner who drops a `requires`
would otherwise show what it hid. The grant hides no value, so it is
not judged there: an owner who declares the verb again without the
flag shows its pending proposals to every reviewer who may read what it
reads, and their accept is refused `verb_redeclared`, as that of any
call of a verb declared again since. A refusal's hint names a verb only
where the caller may call it (006), so it names this one only to its
grantees.

### No grant is implied

`Grants::owner()` names no verb, and the owner is refused a verb
declared `granted` as anyone else is. A host that resolves its owner
to more grants names the verbs in them. The owner may instead declare
the verb again without the flag, holding `schema`: that is a `declared`
event, a new version of the verb, which `describe` and the log show as
any change of a declaration does. Nothing opens the verb without
leaving that mark.

### A grant names verbs by name

A grant may name a verb the type does not declare, or one not declared
`granted`. It grants nothing then, and nothing refuses it: unknown
fields in a type's grants are treated the same way, since grants are
the host's, resolved per call, and a declaration may come after them.

Verbs are never renamed, so a verb's grant stays with the verb's name
and the type name it is given under. It is a permission, and a
permission stays with its name (006): where a type was renamed, the
grants on its names become one entry under the name it bears where it
is read, field restrictions following it from every name, and that
entry keeps only the verb grants given under that name. A relation rename
moves a verb declared on its link type to the link type's new name. A
grant given under the old name no longer applies there; the host names
the verb under the new one. This errs toward refusal: no rename opens a
verb to a caller who was not granted it under the name it now has.

### The flag is a declaration member, and the log needs no new event

`granted` is a member of a verb's declaration in the log format, with
`false` its default, written only where true: the format step of the
release that builds it, with no converter, since no earlier log holds
it. A verb is a declaration of its own, not part of its type's schema
hash (007), so declaring the flag, or dropping it, moves no type's hash.
The WIT record of a verb's declaration carries it, and so does that of
a type's grants their `verbs`.

## Why

A verb with an outside effect, such as one that pays an invoice or
sends a message, was callable by anyone who may write what it sets. A
host that wanted it kept to one executor had to keep that executor's
fields from everyone else, which also kept those fields from any other
writer. Or it checked the call itself, outside the store, which
`describe`, the MCP tools and the proposal listing do not see, so a
caller was offered a verb it would then be refused.

The grant keeps the field rights as they are and names the callers of
the verb beside them. Because it is judged where 006 judges a verb, it
holds at every door a call goes through, and a caller never sees a verb
it may not call.

## What it is not

- It is not a role, nor a kind of principal: the grant is a name in a
  type's grants, which the host's resolver gives per call (006).
- It is not a filter of the doors: the MCP tools leave the verb out only
  because `describe` does.
- It does not grant a field: the field rights apply on top, whatever the
  verb grant says.
- It does not change a verb without the flag.
