# 107 — A verb is callable only to one who may read and write what it does

These rules are what makes a verb callable: a caller shown what the verb
reads and granted what it sets, and a call proposal held to the same
sight.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

### A verb is callable and described only to one who may write what it sets and read what it reads

A call is held to its caller's right to write each field the verb sets.
A verb that reaches outside the lake, called on a record or on its type
(013), takes its type's grant: write access to `on`.

A verb judged by the fields it writes alone tells more than its caller
may read. Its `requires` and the values it sets read the record too, and
a caller who may not read a field they read learned it all the same: a
verb that requires `secret = 'x'` runs on one record and is refused
`requires_unmet` on another, and a verb that sets `copy := secret`
copies the value to a field its caller reads. So a verb is callable, and
described, only to a caller who may read every field its `requires` and
its set values read, each judged as a filter on that field is. A field
read across a reference counts with every reference on the way and the
types they name: a verb whose path reads `customer.discount_days` is its
caller's only where the caller may read `customer`, is shown the
customer's type and may read `discount_days` there
(`docs/decisions/055-a-path-steps-through-a-reference.md`). A path that
steps through a relation's link asks more: the caller holds
`restricted` and no `only` on the link type or its end types, so no link
it reads is one it is not answered (055).
`payload.<name>` reads the call's payload, which is the caller's own. To
any other caller the verb answers `not_granted`, as one whose fields it
may not write does, before the record is read, and `describe` names it
neither on its type nor among the lake's verbs. This holds for a direct
call, a call its risk proposes and a call proposed through `propose`. An
accept judges the record again, so its reviewer is held to the same
rule. A call is proposed only where its `requires` held, so a call
proposal tells what the verb reads as well. Its `why`, when the store
wrote it, names the verb, which `describe` withholds from that reader,
as it withholds a type the reader is not shown. So a call proposal of
such a verb is not there to a reader who may not read what the verb
reads, as a proposal on a hidden type is not (108). `proposals` leaves
it out, so its count does not tell either. `accept` and `reject` answer
as for an id no proposal holds, before anything else of the proposal is
judged. So a reviewer is not told that the verb was declared again
since, which would name it. The verb is judged as the lake declares it
when the proposal is read or decided, and as it was declared where the
call was proposed: an owner who declared the verb again without its
`requires` would otherwise bring back, key and `why`, every call
proposal the rule had hidden. A field derived from one the caller may
not read, a write-only field and a reference to a type the caller is not
shown are judged the same way, since a filter on them is. A verb that
reads no such field is judged by the fields it writes alone.

A verb declared `granted` asks one grant more: its type's grants name
it (`docs/decisions/070-a-verb-may-be-kept-to-the-callers-granted-it.md`).

A refused `requires` names the part of it the record fails, so its
caller knows which rule to meet (lakelands#1286). `requires_unmet`
carries `part`: of an `and`, an `and` within it split in turn, the
first conjunct in declaration order that does not hold; of any other
condition, an `or` and a `not` among them, the whole. A part inside an
`or` is never named alone, since no one alternative is what the record
fails. The conjuncts are evaluated in order, as the `and` evaluates
them, so naming one reads nothing the condition does not. The part is
written in the text form a condition is declared in. A part that read
a field its caller may not read would be left out; by the rule above
there is none: a caller who may not read every field the
`requires` reads is refused `not_granted` before the record is read,
and so is one not told every setting it, or a value the verb sets,
reads (`docs/decisions/063-every-setting-is-in-one-list.md`); a
reviewer who may not, or is not told such a setting, is not shown the
proposal, so whoever is told
`requires_unmet` may read every part of it. A part that reads
`setting('<key>')` is left out where its caller is not shown that
setting, and so is a part after one that reads a setting its caller
is not shown, since naming a part tells that every part before it
held; the refusal is otherwise the same. A host's declared setting is
shown to a holder of `schema` and to one who may write it, as
`describe` tells its declaration; any other key to a reader of
`sys.setting` that no `only` holds there,
where its record is told whole
(`docs/decisions/063-every-setting-is-in-one-list.md`). That is
narrower than what a reader may read of the record, and errs towards
leaving the part out. At an accept the caller is the reviewer. The
literals and function names of a part are told: the condition is the
contract of a verb its caller may call, `describe` does not carry it,
so this is the one place it is told, and only the part that fails. A
constraint is a type's own rule and its refusal names it, not a part of
its condition (`docs/decisions/059-a-type-declares-its-constraints.md`).

A verb may name its conditions (lakelands#1330). Beside `requires`,
`preconditions` lists conditions, each a `name` and what it `holds`,
judged after `requires` in order, as one `and` of them all. A
`requires_unmet` of one is said within it, at the place `precondition`
with its name, beside its `part`, so a caller is told which rule it
broke without matching the text of the rule; one of `requires` is said
at no place, as before. The place is told where the part is and left
out where it is not, since it tells, as the part does, that every part
before it held. A place, not a new member of the case, keeps the case
what a caller already reads. A name is an identifier, two alike are refused
`declared_twice`, and a verb on its type, with no record to
read, declares none, as it declares no `requires`. What reads a verb's
`requires`, the grants, what a caller is shown and what a proposal
keeps, reads its preconditions alike. A verb that names nothing is
declared, written and refused as before: `preconditions` is left out
where there are none.

A type its principal is not shown is not named by what is declared
beside it either. A verb on such a type is not described and no tool
is made of it, even where the principal could write every field the
verb sets: `call` and `propose` answer it as a verb that is not
declared (106), and `describe` agrees with them.

## Why

006 states why the store decides a call on grants rather than roles, why
the check sits in the store, and why masking an answer does not protect a
value the question already revealed (`006 § Why`). The reasons particular
to these rules are stated with them, above.

## What it is not

006 states what no rule of grants decides: it is not an off switch, not
authentication, and not partial success (`006 § What it is not`).

## How it came to be

- These rules were stated in 006 and changed there from its first version;
  lakelands#1768 moved them here word for word. Their history, oldest
  first, is that record's (`006 § How it came to be`).
