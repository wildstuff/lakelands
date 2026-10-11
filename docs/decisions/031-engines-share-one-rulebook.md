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
- the cosine of two vectors (`lakelands_api::query::cosine`, which the
  module names), and the order of ranked hits;
- which proposals match a filter, and in what order.

An engine's own indexes (an SQL index, a full-text table, an edges
table) only find candidates. The shared functions decide which of them
answer, in what order, with which score. So every engine gives the same
answers by construction: each calls the same functions. The checks hold
each engine to the page, not one engine to the other; a divergence would
still be a defect in one of them, never an allowance. One answer is not
the shared functions' alone: a vector field the lake asks an index for
answers the candidates the engine's index finds, and says so
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).

## Why

Two implementations of the same rule drift. A comparison across kinds,
a tie broken by insertion order, or a score summed in `f32` instead of
`f64` is invisible until a page skips a record or an agent sees another
top result on another engine. Writing each rule once makes that drift
impossible where it matters most, and makes a new engine a port of
storage, not a re-derivation of meaning.

The price is that an engine cannot answer entirely inside its database:
it reads candidates and hands them to shared code. That is the right
trade. An engine that pushes a rule into its query language must show,
through the same checks, that it selects exactly what the shared
function selects. SQLite and PostgreSQL count so at the head, when
nothing but the flags selects
(`docs/decisions/045-a-table-reads-a-page-and-its-total.md`).

## What it is not

- It is not a ban on indexes. An engine should find candidates as fast
  as it can; it must not drop one the rule would keep. The one index
  that may is a vector index the lake asks for.
- It does not move storage into the core. What an engine keeps, and how,
  stays its own.

## How it came to be

- The first version: one function decides wherever the seam asks an
  engine to decide, for the in-memory engine and the SQLite engine, as
  "the right trade for `0.1.0`".
- Corrected at the rewrite (lakelands#817): "the in-memory engine and
  the SQLite engine give the same answers" names every engine, since
  038 added PostgreSQL; the trade is no longer one release's.
- Corrected at the rewrite (lakelands#817): the same answers by
  construction, and "it must not drop one the rule would keep", but for
  a vector index the lake asks for, which 043 allowed; the cosine is
  the contract crate's; a rule pushed into a query language names the
  count at the head (lakelands#380).
