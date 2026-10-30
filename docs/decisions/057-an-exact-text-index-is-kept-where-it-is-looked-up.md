# 057 — An exact-text index is kept where it is looked up

## What is decided

### The exact-text index keeps rows for the fields that are looked up

Each engine that keeps an exact-text index (SQLite and PostgreSQL) keeps
a row for each string a record's latest state holds, but only in the
fields of its type that are looked up. The memory engine keeps no such
index.

**The declarations look up these fields of a type:**
- its key, but a key that is one text field;
- its references;
- the fields a derived relation matches at either end (044);
- those a set's condition, or a view's `where` over the type, compares
  to a text at the top level (`field = 'text'`, alone or as an `and`
  term);
- the fields its constraints' `preceding` matches on, so a write reads
  one series of the type, never the type (059).

**The setting adds others.** The `sys.setting` record
`lookup_index.<type>` holds a JSON array of text fields, as
`sort_index.<type>` does for the order index (041). Its write is refused
as invalid, naming the field, when the field is no text or is computed
at the read.

**A key of one text field is read by key.** A record's key is its key
fields' values made components by the key rule (`docs/log-format.md`
§2.3), and a single text component reads back into its value, so the
only record that can hold a text there is the one at the key the text
makes. A `where` that the field equals a text, or a derived relation's
match asking for one of texts, reads those keys' states
(`engines::key_lookup`), at the head and at an earlier position alike,
and the rulebook admits each. The index keeps no rows for the field:
they would only repeat the key. What holds the key and its field
together is the store: it fills a record's key fields from its key and
refuses a write whose key fields make another key (`KeyDisagrees`), and
a key field cannot be erasable (`ErasableKey`), so no erasure blanks it
while the key stays. What could break the inversion otherwise cannot
happen: a rename of a value a key field holds is refused (027), a change
of the key fields re-keys the type's records, and a link type's key is
two fields.

`lakelands_core::engines::looked_up` decides the set from the
declarations and the setting at the head. Both engines call it, and no
other field has rows.

**A reader decides from the same set, never from which rows exist.** A
string the index keeps is looked up. A string of any other field is
found by a scan of the type, narrowed in storage by the stored value
(`json_extract` on SQLite, `->>` on PostgreSQL). In both cases the one
rulebook admits each candidate, so the answer is the same, and only its
price differs.

The price of the scan, as measured on the fleet's machine, by a text that
one record holds or none does:
- **SQLite at 50 000 records:** 113 to 192 ms.
- **PostgreSQL at 50 000 records:** 0.82 to 0.86 s, because its `state`
  column is `text` and the narrowing casts each row to `jsonb`.

A state kept as `jsonb` is the lever for PostgreSQL. It is a change of
the projection's layout, not of this rule, and is not taken here. A host
that finds by a field at scale names it in the setting.

**When the set changes, the rows follow in the same transaction.** That
happens when:
- the setting is written or retired;
- a declaration is put that changes a type's set (a set, a view, a
  relation, or the type declared again).

The engine then fills the rows of each field that came, from the type's
head states, and drops those of each field that went. It does this once,
at the commit of the transaction that put them.

**The commit decides again only for the types the transaction moved.**
A type's set depends on three things:
- its own declaration;
- the setting;
- the declarations that name it: a derived relation onto it, a view
  over it or merging it, a set of it.

So a put moves the types named by the declaration it replaces and by
the one it puts (`engines::looked_up_by`), and a written setting moves
its own type. Every other type keeps its set and its rows.

The writing handle keeps the head's declarations, decoded, as of its
last commit, so the commit reads none of them from storage. A position
names the same declarations whoever folded through it, since they are
the log's. Where the
projection was folded through another position since then, it reads
them all and decides for every type, as it did before. That happens
after another handle committed, after a rebuild, after a failed commit,
and on the first commit of a handle.

### A record new to the projection drops no rows

A record the projection has never held has no index rows to drop. Its
first state is indexed without the deletes a later state's indexing
needs.

## Why

The index cost every ingested record one row per string field, read or
not. On a mirrored type of ten text fields, it was most of what a record
cost the fold, a third of the projection's size, and a fifth of a large
window's time:
- 1.4 of 7.8 s per 50 000 records;
- 1 810 MB against 1 264 MB at 500 000 records.

The fields a lake finds records by are few, and most of them are already
named by its declarations. A host names the rest once, in the setting,
as it names the fields it orders by.

Keeping the decision with the declarations and the setting, not with the
rows, makes a reader's choice the same before and after a rebuild, and
on every engine.

## What it is not

- **Not a change of any answer.** A `where`, a count or a page answers
  the same whether its text is looked up or scanned for. The conformance
  suite holds that across the setting's write and retirement, and
  across a set's declaration.
- **Not a declaration member.** No field of a type carries a "looked up"
  mark. The setting is the lake's configuration, and it leaves the
  declaration's hash as it is.
- **Not every text kept and only the deletes taken off.** That was
  measured: +9 % ingest against +23 %, with every find unchanged. It was
  not taken, because the rows themselves are the cost.

## How it came to be

- The first version: the exact-text index keeps rows for the fields the
  declarations and the setting look up, every field of a view's kept
  records, and a reader decides from that set (lakelands#1019).
- A commit decides the set again only for the types it moved
  (lakelands#1098).
- A key of one text field is read by key, and the index keeps no rows
  for it (lakelands#1318).
- A field a constraint's `preceding` matches on is looked up
  (lakelands#1552).
