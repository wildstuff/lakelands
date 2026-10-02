# 027 — A shape change is declared or migrated, and a migration loses nothing

## What is decided

A revision of a type enters the log by `declare` only when no stored
record can stop fitting it: a change of meaning, an added optional field
or relation, a loosened requirement. Every other change of shape — a
field removed or retyped, a field made required, a key changed — goes
through `migrate`, which first answers a preflight report of what would
happen to every stored record, and is applied only by a call that names
that report and only when the report is lossless. A rename is never
inferred from two declarations; only a migration names it.

The conversions a migration may make are a closed table: a value
converts only when converting it back gives the same information. A
number becomes text in its canonical form, a numeric string becomes a
number only when the number is exactly equal, a fraction never becomes a
decimal, an offset is never dropped from an instant, and a value already
in the target form is never rewritten.

## Extended: history converts too, and the key stays

A migration converts every stored record of the type, retired and
restricted ones included: they are history, and a lake whose history no
longer fits its declaration could not be folded again. So the preflight
covers them, and a record among them that would not convert makes the
migration not lossless, refused before anything is appended, never
discovered by the fold after the migration is in the log.

In this release a migration keeps the type's key. Records are converted
where they are and never re-keyed, so a migration whose declaration names
other key fields is refused as not supported, and one that would change
the value of a key field — so that a record's key fields no longer make
its key — reports that record not convertible. Re-keying records is a
later door, not a side effect of a migration.

## Extended: a value type is loosened by declaration and tightened by a preflight

A value type that accepts everything it did (`one_of` gains values or is
removed, a `constraint` or `format` is removed) is declared: no stored
value can stop fitting it. One that may accept less goes through its own
preflight: every value stored under it, in every field of every type that
uses it, history included, is checked against the new declaration as a
write would check it. The change is refused while any value would not
fit, and applied only against the report it was shown. Since a tightening
every stored value fits reshapes nothing, it enters as the ordinary
`declared` event at the next version, and no record is rewritten. A change
of a value type's `kind` or `scale` rewrites stored values (a decimal is
kept at its scale), so it is not made this way; the fields that use it
are migrated one by one.

The preflight looks at stored values, not at the conditions that mention
them. A trigger's `when`, an aging rule's `from` or `to`, or an object
set's `where` that compares with a value the type no longer admits is not
checked; it simply never holds afterwards. For an aging rule the run's own
check applies instead: a rule whose `from` or `to` the type no longer
admits is skipped by `age` and named in its report.

## Extended: a type is re-keyed by a migration

The rule above that a migration keeps the type's key is lifted: a
migration may name other key fields, or change a key field's value. A
record's key is its name everywhere in the lake, so the preflight covers
everything that names the records it moves:

- every record of the type, retired and restricted included, whose key
  the new key fields make differently; two records that would take one
  new key, or a record without a value for a new key field, make the
  migration not lossless;
- every type whose key fields reference a re-keyed type, re-keyed with it
  (link records, keyed by their two ends, are the common case); the
  closure is computed with a visited set;
- every reference value, in any type, that names a moved key.

The moves are listed in the `migrated` event (`rekeyed`, log format
version 2), and the fold moves each record with its fields, version and
per-field provenance unchanged: who wrote a value, and when, stays true of
it. That is why the move is a new member, and a new format version, rather
than a retirement and a new write per record, which would have made every
moved value the migration's. The old key is ended, not retired: a read
before the migration finds it, a read from the migration on does not, and
a later record whose key is the same string is a new record at version 1.
The store keeps an ended key as a retired state at version 0 with no
field. That state is no record: every scan of stored history (a later
migration's preflight, a value type's, the closure of a second re-key)
skips it, so a type that was re-keyed migrates again like any other. A
moved record keeps its object-set memberships at its new key. A migration
that takes every key field away, names a key field that is not required
or of a kind no key is made of, or would re-key a view is refused.

Two things follow outside the lake's own records. A source that writes
the type must send the new keys: its own key mapping (a column map's
`key`, the keys its feed carries) has to follow the migration. A write
whose key fields make another key than the one it names is refused as
invalid, as before; a write that names an old key's bare string and
carries no key field has its key field filled from that string (§2.3),
and is a new record at version 1. And the aging id (`docs/log-format.md` §3) names the record's
key, so a date aged before a re-key and again after it can move a record
a second time, when after its own transition the rule's `from` and `when`
hold for it again.

## Why

The log is the truth (001), so a change of shape must never quietly
make stored records wrong. Separating the two doors makes the dangerous
one visible: `declare` cannot reshape anything, and `migrate` shows its
consequences before it acts. A closed, lossless table means a migration
is reversible in information even when it is not undone, and every
engine converts the same values the same way.

## What it is not

- It is not a ban on removing a field. A removal drops that field's
  values, and the preflight names what it drops before anything is
  applied.
- It does not convert anything the table does not list. A value that
  would not convert makes the migration not lossless, and it is refused.

## Extended: a shape change may be proposed

A declaration or a migration may also be proposed, and it reaches the
ontology only through its acceptance, under the same rule: a change that
reshapes stored records is a migration there too, preflighted when it is
proposed and again when it is accepted
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended).

## Extended: an annotation is a change of meaning

A revision that changes only a declaration's `annotations` reshapes no
stored record, so `declare` takes it like a relabel. It never needs a
migration, and none is offered. A migration whose `to` changes
annotations together with the shape carries them like any other member
(`docs/decisions/007-schema-hash-covers-shape.md`, extended).

## Extended: a view is never migrated

A migration reshapes the stored records of a type, and a view stores
none: its records are computed on read. So no migration turns a stored
type into a view, a view into a stored type, or one view into another.
Each is refused as invalid, naming the type. The way there is to retire
the one and declare the other. Otherwise a stored type's records would
linger beside the records the view computes.
