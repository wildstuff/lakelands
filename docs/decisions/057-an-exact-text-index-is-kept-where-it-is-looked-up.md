# 057 — An exact-text index is kept where it is looked up

## What is decided

### The exact-text index keeps rows for the fields that are looked up

Each engine that keeps an exact-text index (SQLite and PostgreSQL) keeps
a row for each string a record's latest state holds, but only in the
fields of its type that are looked up. The memory engine keeps no such
index.

**The declarations look up these fields of a type:**
- its key;
- its references;
- the fields a derived relation matches at either end (044);
- those a set's condition, or a view's `where` over the type, compares
  to a text at the top level (`field = 'text'`, alone or as an `and`
  term).

**The setting adds others.** The `sys.setting` record
`lookup_index.<type>` holds a JSON array of text fields, as
`sort_index.<type>` does for the order index (041). Its write is refused
as invalid, naming the field, when the field is no text or is computed
at the read.

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
