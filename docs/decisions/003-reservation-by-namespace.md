# 003 — Reservation by namespace, never by roster

## What is decided

System types are recognised by their name, not by membership in a list.
Every type whose name starts with `sys.` is a system type; no other type
is. The `sys.` prefix is reserved: an ordinary principal cannot declare
or pin a `sys.*` type, and the store enforces this on every write.

Lakelands itself reserves only the system types its own fold reads:
`sys.source`, `sys.provenance`, `sys.setting` and `sys.aging_rule`. A
host that needs more registers them through the `SystemVocabulary` seam,
and decides which lakes may hold them.

## Why

A roster of reserved names has to be kept in step with every place that
checks it, and each new system type is a change to the store. A
namespace needs no list: the guard is one prefix test, a host can add
its own system types without touching Lakelands, and a user type can
never collide with a system type that did not exist yet when it was
declared.

Keeping Lakelands' own reservation to what its fold actually reads
keeps the store from carrying another product's vocabulary.

## What it is not

- It is not a privilege level. Whether a lake may hold `sys.*` types is
  a right the host grants, not a special kind of lake.
- System records are ordinary records. They live in the same log, are
  folded by the same rules, and are read through the same doors, with
  the same field rights.
- It does not make every type with a dot in its name special; only the
  `sys.` prefix is reserved.

## Corrected (2026-09-25): two are reserved ahead of their readers

The sentence above calls the four reserved types "the system types its
own fold reads". Of the four, the fold reads `sys.setting` and `age`
reads `sys.aging_rule`; `sys.source` and `sys.provenance` are reserved
ahead of their readers (`docs/seams.md` §`SystemVocabulary`). The four
names are reserved either way; the page says which have a reader today.

## Extended: a verb names a link type and no other system type (lakelands#759)

A declaration that names a type through `on` or `of` names a system type
under the same reservation. A trigger may watch any declared type, and
an object set may hold any type's records, system types among them:
neither writes. A verb writes the records it is called on, so it is a
door. Of the system types, a verb may name only a relation's link type
(`sys.link.*`). A link's own fields are the relation's, declared by the
author, and a verb writes those: under the relation's authority, with
the link's ends checked as `link` checks them, and never the ends, which
key the link (035). Any other system type has its door already (`upsert`
on `sys.setting` and `sys.aging_rule`, held to the record checks of
016), and a verb on one is refused as `reserved`, as a type declared
under the prefix is. A verb's write would pass those checks too, so
nothing it writes could break a setting or a rule. The rule keeps each
reserved type to one door.
