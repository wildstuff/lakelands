# 022 — Functions are declarations

## What is decided

A function is a declaration in a lake's ontology like a type or a verb:
a name, the backing kind that computes it, typed inputs, and either an
expression (for the built-in `expr` backing) or the backing's own
configuration (for any other kind). It enters the log as a `declared`
event, and a bundle declares it in its `functions` lane.

The backing kind is an open token. `expr` is evaluated inside the store
from its expression, in the one grammar. Any other kind is called
through the `FunctionBacking` seam, and its output is recorded in the log
and read back on replay.

## Why

A function a lake depends on has to be part of what the lake knows at a
position. If functions lived beside the log, in files or in a host's
configuration, a replay could meet a different function under the same
name, and the lake could not say which version produced a value.

Keeping the backing an open token lets a host add its own kind of
computation without a change to Lakelands, while the store keeps the
two guarantees that matter: the declaration is in the log, and every
output that is not deterministic is recorded.

## What it is not

- It is not a way to call arbitrary code from an expression. The
  expression grammar has a closed set of functions (017); a declared
  function is computed by the store where the ontology names it.
- An `expr` function is not recorded output: it is deterministic and is
  recomputed on replay.

## Extended: where a function is invoked

In this step a function is declared and can be loaded from a bundle, but
nothing in a declaration yet names one to be computed: no field says
"this value comes from that function". The invocation belongs to the
fold. A derived field will declare the function that computes it, as a
member of the field's declaration, when the fold that calls it arrives;
until then a declared function is part of the ontology and computes
nothing.
