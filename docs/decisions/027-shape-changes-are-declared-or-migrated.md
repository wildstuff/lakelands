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
checked; it simply never holds afterwards.

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
