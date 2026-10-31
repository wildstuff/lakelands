# 081 — A change to the records a condition selects is planned before it acts

## What is decided

**Accepted** on lakelands#1404 (2026-10-25, by the operator), for
0.20.0. It extends `docs/decisions/062-a-change-plans-before-it-acts.md`
from changes to the ontology to changes to records, and comes in force
with the issue that builds it.

### A change names a type, a condition and what to set

A change to records takes a type, a condition in the expression grammar
(017), and either the values to set, per field, or `retire`. A value is
a value or a bound parameter, never an expression over the record: the
same values land on every record the condition selects. On a link type
it sets the link's own fields or unlinks, and never names an end (035).

It has the two steps 062 gives a change of the ontology, on one path up
to the append, so a plan refuses what its act refuses and counts what
its act would write.

### `plan` tells the whole change and appends nothing

`plan` answers, appending nothing:

- **how many records** the condition selects, and the first keys its
  reader is answered;
- **per field**, how many of them it would change;
- **every refusal** a selected record would meet, each as that
  record's own `upsert` or `retire` would be refused to the caller, with
  its count and first keys: a record the change would leave outside the
  caller's `only` (006), a constraint that refuses (059), and any other
  refusal one record's write meets and another's does not. A plan that
  holds one says the change cannot be enacted.

A refusal that needs no record refuses the plan itself, in the order an
`upsert` refuses it (016), before a record is read: a type the caller is
not shown, a field it may not write, a field of an authority the door
does not write (a source's, a derived or computed one), a value of the
wrong kind.

The plan is structured, never a sentence; a host words it for whoever
confirms it, as `say` words a plan of the ontology (062). Its id is made
over what its reader is shown of it and over the selection: every
selected key with the version the plan read.

### `enact` changes what the plan showed, in one append, or nothing

099 moves the re-selection to a view, and a report goes stale only by a
write that reaches it: a selected key that moved, or a record that came
to match.

`enact` acts against the plan its caller was shown, named by its id. It
selects again at the head, under the writer, and where the plan made now
has another id it is refused `plan_stale`: a selected record changed, a
record joined or left the selection, or what the caller is shown of
them moved. Otherwise each selected record is written at the version
the plan read, in key order, each decided over the ones before it as a
batch decides its writes (073), and the whole change is one append and
one fold. It runs alone, as a declaration does, so a reader sees all of
it or none of it. A refusal of any record refuses the whole change.

A write elsewhere in the lake does not stale a plan. On a lake that is
written, the head moves between any plan and its enact, and a change to
many records would never pass; what the caller confirmed is the
selection and its versions, and that is what is compared.

The events are the `upsert`s' and `retire`s' own, under the caller's
write context (021), so the log format does not change for a change
that is enacted.

### A selection past the ceiling is refused, never cut

A selection of more records than the lake's `limits.max_change_records`
is refused `too_large` by `plan` and `enact` alike, naming the ceiling,
never cut: a change applied to part of what its condition selects would
look complete. The pull request that builds the door measures one
append of that many records on every engine and sets the default from
it.

### A caller who proposes gets one proposal of the whole change

A caller that proposes rather than writes, an agent a host hands
`propose` alone, proposes the change whole: one proposal holding the
type, the condition, the values and the plan its proposer was shown. A
holder of `only` proposes over its own records: the proposal holds its
pairs beside the condition, so its accept selects no record the
proposer could not. The accept enacts it as its reviewer's own `enact`
would, against the plan its reviewer is shown by `plan`, and refuses it
`plan_stale` otherwise, the proposal left pending (062). A proposal of
a change is a new form of proposed write, and the log format moves with
it.

### The condition reads only what its caller may read

The condition is a read by its caller. A field it reads that the caller
may not read, also one the caller may write and not read, is refused not
granted, naming the field, and one it may not know of as a field the
type does not declare; a type the caller is not shown is not declared.
Each is refused before a record is read, as every probe is (006). The
selection is what `find` with that condition answers the caller: a
record its `only` does not admit, or one held from it, is not there and
is neither counted nor written.

The plan's count and keys, its per-field changes and the outcome's are
answers to a read. So a caller who may write a field but not read it is
refused a condition on it, not answered a count: `where salary > 5000`
and a count of what was marked would tell the salary. For the same
reason, nothing the plan or the outcome tells depends on a value its
caller may not read: a field it may only write is told as written on
every selected record, and the act writes it so.

## Why

A host is asked for a change to many records, "mark every invoice of
this customer as disputed", and today makes it as many `upsert`s, each
judged alone, with nothing that shows the whole change before it
happens, and a change that stops half way when one record is refused.
The person who confirms a change of the ontology is shown its plan
first (062); a change of records needs the same, and for an agent that
proposes it, a plan the person can be shown whole.

Comparing the selection and its versions, rather than the head, is what
makes the confirmation mean something on a lake that is written: what
the person was shown is what changes, or nothing does.

The read rule closes what would otherwise be an oracle: a condition is
a filter, and a filter on a field its caller may not read, answered by
a count, tells the field record by record.

## What it is not

- Not an update language. Values are values; a change computed from
  each record's own fields is a derived field or a verb (013).
- Not a transaction over several types. A change names one type.
- Not a new engine method. The selection is `find`'s read, and the
  writes are the doors' own events.
- Not a change cut to its ceiling, and not one applied in parts.

## How it came to be

- Amended by 099 (lakelands#1652), proposed for 0.23.0.
