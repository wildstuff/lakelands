# 045 — A table reads a page and its total

## What is decided

`find` serves a record table that shows "page 7 of 40" with 25, 50 or
100 rows per page (lakelands#381).

- **A total, when asked.** `Find.total` asks `find` to answer, beside
  the page, how many records match: the same `where`, parameters,
  `valid_at` and grants, at the answer's position. It is `total` in the
  answer and is never computed unless asked. The count is the one of
  lakelands#380: `COUNT(*)` at the head when nothing but the flags
  selects, and otherwise the type read in batches and counted, holding
  one batch. A later page of the same walk is read at the position its
  cursor carries, so its total is the first page's, whatever other
  writers write in between. A page read without the cursor reads the
  head and counts there. `total` beside an `aggregate` is refused as
  invalid: the aggregate is already the count.
- **A jump, bounded.** `Find.offset` skips that many rows of `find`'s
  one order (025) before the page. The page's `next_cursor` continues by
  keyset after its last row, as any page's does, so page 8 after a jump
  to page 7 is one cursor read. `offset` beside `cursor`, `after` or
  `aggregate` is refused as invalid, naming both: each already says
  where the answer starts, or has no rows. An offset past the lake's
  `Limits::max_offset` (100 000 unless the location sets it) is refused
  as `too_large` naming `offset` and the ceiling, never cut.

## Why

A table jumps. It shows a total and lets its reader go to the last page,
or to the seventh. A keyset cursor alone reaches page k only in k
requests, or by keeping the cursor of every page the table has shown.

**The offset costs what it skips.** No engine skips a row without
reading it, because the order indexes are keyset and not positional. So
`offset` reads the rows it skips, a page at a time, and drops them:
memory stays one page, and time grows with the offset. Page k of a
table therefore costs about k pages of reading, while the next page from
there costs one. The ceiling keeps a careless jump from reading a large
type in one request. It is a limit of the location, like `max_limit`, so
a host that needs deeper jumps raises it knowingly.

**The total is opt-in and pinned.** A count can cost as much as reading
the type, so a page that does not show it does not pay for it. Counting
at the answer's position makes the total and the page one answer: a
table that walks with the cursor shows the same "of N" on every page,
while a refresh, which reads the head, shows the head's.

## What it is not

- **Not a positional cursor.** `next_cursor` stays keyset. A write
  between two pages moves no row into or out of a page the walk has not
  read yet, the guarantee 025 gives a cursor of `find`: it carries its
  position in the one order, and so pins a page set.
- **Not an estimate.** The total is exact at its position. An engine's
  statistics would answer faster and wrongly.
- **Not offered for an aggregate**, whose groups are no page.
- **Rejected: no offset.** A host could reach page k by walking k
  cursors, or by keeping every cursor it has handed out. That pushes the
  cost of a jump onto every host and leaves it unbounded. A bounded
  `offset` states the cost once, here, and keeps it inside the store.

## How it came to be

- The first version: a total when asked, and a bounded jump
  (lakelands#381).
- The order and the cursor it cites stand in 025, where the rules of
  029 moved; "the guarantee 029 gives every walk" names a cursor of
  `find` (lakelands#817).
