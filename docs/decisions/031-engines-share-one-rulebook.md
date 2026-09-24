# 031 — Engines share one rulebook

## What is decided

Wherever the engine seam asks an engine to decide something, the
decision is made by one function in `lakelands_core::engines`, and every
engine calls it:

- which records a scan's filter selects;
- where they sort, and what follows a keyset position;
- which links a record's `via` fields make;
- which tokens a text holds, which records a full-text query hits, and
  their scores;
- the cosine of two vectors, and the order of ranked hits;
- which proposals match a filter, and in what order.

An engine's own indexes (an SQL index, a full-text table, an edges
table) only find candidates. The shared functions decide which of them
answer, in what order, with which score. So the in-memory engine and
the SQLite engine give the same answers by construction: both call the
same functions. The checks hold each engine to the page, not one engine
to the other; a divergence would still be a defect in one of them, never
an allowance.

## Why

Two implementations of the same rule drift. A comparison across kinds,
a tie broken by insertion order, or a score summed in `f32` instead of
`f64` is invisible until a page skips a record or an agent sees another
top result on another engine. Writing each rule once makes that drift
impossible where it matters most, and makes a new engine a port of
storage, not a re-derivation of meaning.

The price is that an engine cannot answer entirely inside its database:
it reads candidates and hands them to shared code. That is the right
trade for `0.1.0`. An engine that later pushes a rule into its query
language must show, through the same checks, that it selects exactly
what the shared function selects.

## What it is not

- It is not a ban on indexes. An engine should find candidates as fast
  as it can; it must not drop one the rule would keep.
- It does not move storage into the core. What an engine keeps, and how,
  stays its own.
