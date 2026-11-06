# 003 — Reservation by namespace, never by roster

## What is decided

### A type is a system type by its name

System types are recognised by their name, not by membership in a list.
Every type whose name starts with `sys.` is a system type; no other type
is. The `sys.` prefix is reserved: an ordinary principal cannot declare
or pin a `sys.*` type, and the store enforces this on every write.

Lakelands itself reserves four system types: `sys.source`,
`sys.provenance`, `sys.setting` and `sys.aging_rule`. The fold reads
`sys.setting` and `age` reads `sys.aging_rule`; `sys.source` and
`sys.provenance` are reserved ahead of their readers (`docs/seams.md`
§`SystemVocabulary`). The store also derives one system type from each
relation carried by link records, its link type
`sys.link.<type>.<relation>`
(`docs/decisions/035-a-link-is-a-record.md`). A host that needs more
registers them through the `SystemVocabulary` seam, and decides which
lakes may hold them.

### A verb names a link type, and an external verb a host's own type

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

One kind of verb is not a door, and is admitted on a host's own system
type: a verb whose effect is `external`, which sets nothing. It hands an
effect outside the lake to the host's executor with its declared risk,
its proposal, its idempotency and its recorded output (013), which the
host would lose by acting outside the store. It is admitted only on a
type the host's vocabulary declares: a `sys.` type that is neither under
`sys.link.`, which keeps the rule above, nor one of the four the store
keeps for its own readers. No caller declares a system type and the
writer's open refuses those, so the declarations in force tell such a
type, whoever opened the lake. It is called under the grants of any
external verb: write access to its type (006). A `write` effect on such
a type stays `reserved`: the host writes its own system type through
`upsert`, under the grant it gives.

### A host's type gains fields between its releases

A host's next release may add optional fields to a type of its
vocabulary, anywhere among the fields it holds. Where the lake holds
the type without them, a writer's open declares it again in the host's
form, in the append that takes the lease, as it does the store's own
types: a field added reshapes no record, so the records written before
read as they did, and a write may give the new field from then on. A
reader is refused such a lake, naming the lake and the type, until a
writer has opened it, as it is refused a lake in an older log format:
what it would read is not what its vocabulary declares.

A host may also reword a type: its descriptions and labels, its
annotations and the order of its fields. None of these is read by a
stored record, and none by a door's check beyond the one a door runs on
annotations themselves: a key in a namespace and of a length a door
takes, outside `sys.` and `lakelands.`, no `null`, integers a log
writes, and their size. A writer's open declares a reworded type again,
with an added field or alone, and refuses annotations that check would
refuse, as a door does; a reader reads a reworded type as the lake
holds it.

Any other change to a type the lake holds is refused at the open,
writer's or reader's, naming the type and each difference, before
anything is appended, whether a field is added beside it or not: a
field removed, retyped or renamed, a field added as required or made
optional, any other member of a field or a relation changed, the key,
a relation added or removed, a constraint, the type's language, its
valid time, `insert_only`, `retired_by` or `field_meta`. The open
compares the host's type with the one the lake holds whole, the added
optional fields and the rewording taken out, and declares it again
only where nothing else differs. Each other change reshapes stored
records, or asks what a door would judge against them, a constraint
the records held may break among them; the open makes no migration and
judges no change as a door does, so it applies none of them, and none
in part. Whether a host may declare a migration with its vocabulary is
not decided here.

## Why

A roster of reserved names has to be kept in step with every place that
checks it, and each new system type is a change to the store. A
namespace needs no list: the guard is one prefix test, a host can add
its own system types without touching Lakelands, and a user type can
never collide with a system type that did not exist yet when it was
declared.

Keeping Lakelands' own reservation to the types its own readers are
written for, and the link types it derives itself, keeps the store from
carrying another product's vocabulary.

An upgrade of a host must not leave its lakes behind. Without the
second declaration a lake written by the host's earlier release keeps
the older form of its type and refuses the new field, and the host
migrates the lake by hand or starts a new one. A field added, wherever
it stands, and a rewording are the changes the open can make alone,
since nothing stored moves and no door's check asks anything of the
records; refusing a rewording would strand a host for a word. Every
other change would reshape what the lake holds, or ask what a door
judges, and a lake opened with it would not answer as its vocabulary
says. So the open refuses it whole rather than applying what it can.

## What it is not

- It is not a privilege level. Whether a lake may hold `sys.*` types is
  a right the host grants, not a special kind of lake.
- System records are ordinary records. They live in the same log, are
  folded by the same rules, and are read through the same doors, with
  the same field rights.
- It does not make every type with a dot in its name special; only the
  `sys.` prefix is reserved.

## How it came to be

- The first version reserved the `sys.` prefix and named four types as
  "the system types its own fold reads".
- Corrected on 2026-09-25: of the four, two have a reader; `sys.source`
  and `sys.provenance` are reserved ahead of theirs.
- A verb may name a relation's link type and no other system type
  (lakelands#759).
- Corrected at the rewrite (lakelands#817): what Lakelands reserves
  names the link types the store derives, which 035 added and this
  record's list left out.
- An external verb, which sets nothing, is admitted on a type of the
  host's vocabulary; before, every verb on a system type other than a
  link type was refused as `reserved`, and a host's open sequence that
  applied its external verbs failed (lakelands#966).
- A type of the host's vocabulary that gains optional fields is
  declared again at a writer's open, and a reader waits for it; any
  other change to a held type is refused at the open. Before, the open
  declared only the types the lake did not hold, so a field a host's
  release added never reached its earlier lakes (lakelands#1293).
