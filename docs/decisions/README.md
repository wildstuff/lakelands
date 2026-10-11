# Decision records

This directory holds the design commitments Lakelands rests on and the
reasons for them. When a comment in the code has to explain a choice that
is bigger than the module it sits in, the explanation belongs here and
the comment points to the record.

## Form

One file per decision, named `NNN-<slug>.md`. `NNN` is a three-digit
number: the first record is `001`, and each new record takes the next
free number. A number is never reused or renumbered, and a file keeps
its name, also when its title changes.

Every record has a title and four sections:

- **What is decided.** The rule as it stands today, stated so that code
  can be checked against it. A record of several rules gives each a
  heading that states it as a sentence, with its reason under it and,
  where something was measured and not taken, that too.
- **Why.** The reason, in Lakelands' own terms. A record stands on its
  own: it never cites another project's decision records, issues or
  discussions as its justification.
- **What it is not.** The readings the decision does not support, and
  the alternatives it rules out.
- **How it came to be.** A list, oldest first, one line a change: what
  the change added, narrowed or replaced, and its issue. It says what
  changed and never what the rule is; the rule is above it. A record
  that never changed has no such section.

A record is as short as its rules allow: one rule is a few paragraphs,
not an essay.

## When a record is written

A record is added or changed in the same pull request as the code it
explains. **A decision that changes is rewritten where it stands,** and
the same pull request adds one line to "How it came to be" that says
what changed and why. Nothing is appended as a section of its own: who
reads "What is decided" reads the rule of today, whole, and never an
earlier rule that a later section takes back.

A record states the reason; the conformance suite checks the behaviour.
Where a record promises something every engine must honour, the check
lives in the suite.

## A record that moved

When the rules of several records become one record, that record keeps
the lowest number. Each of the others keeps its file and its number and
becomes a pointer: its title, a sentence that names the record its rules
moved to and the issue, and a table with one row for each section it
held, saying where that rule stands now. A record that keeps its own
topic and gave only some sections away carries the same table under
"What moved".

A row names its target as the record's number, `§` and the heading of
the rule, word for word, in a code span, as in `NNN § <heading>`. A
heading that holds a code span itself is written in a span of two
backticks, with a space before the closing pair where the heading ends
in one. The gate `records-resolve` reads every such row and is red when
the record or the heading is not there, so a rule cannot be renamed away
from under a pointer.

## Citing a record

Code, pages and records cite a record by its path
(`docs/decisions/NNN-<slug>.md`) or, in a comment, by its number. A new
citation names the record that holds the rule today. A citation of a
record that moved still resolves, through its pointer, and is not
rewritten for that alone. The gate holds that every path names a file.
