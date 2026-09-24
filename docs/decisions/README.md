# Decision records

This directory holds the design commitments Lakelands rests on and the
reasons for them. When a comment in the code has to explain a choice that
is bigger than the module it sits in, the explanation belongs here and
the comment points to the record.

## Form

One file per decision, named `NNN-<slug>.md`. `NNN` is a three-digit
number: the first record is `001`, and each new record takes the next
free number. Numbers are never reused or renumbered.

Every record has a title and three sections:

- **What is decided.** The commitment, stated so that code can be
  checked against it.
- **Why.** The reason, in Lakelands' own terms. A record stands on its
  own: it never cites another project's decision records, issues or
  discussions as its justification.
- **What it is not.** The readings the decision does not support, and
  the alternatives it rules out.

Keep a record short: a few paragraphs, not an essay.

## When a record is written

A record is added or extended in the same pull request as the code it
explains. A decision that changes is not rewritten silently: the record
is extended with what changed and why, or a new record replaces it and
the old one says which record replaced it.

A record states the reason; the conformance suite checks the behaviour.
Where a record promises something every engine must honour, the check
lives in the suite.
