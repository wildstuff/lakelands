# 025 — Full-text match sets are the same on every engine

## What is decided

Engines do not tokenise text for full-text search themselves. The store
owns one token pipeline — split into words, lower-case, stem by the
language's BCP-47 tag, fold diacritics — and every engine applies it to
the text it indexes and to the words it is asked for. The same query at
the same position matches the same set of records on every engine, even
though each engine's index holds different bytes.

A new language, or a change to how words are formed, is a change to that
one pipeline, never to a single engine.

## Why

Search is only portable if a lake answers the same question the same way
whatever engine holds it. If each engine tokenised with its own database
features, "Rechnung" would find "Rechnungen" on one engine and not on
another, and a lake moved between engines would silently lose recall.

## What it is not

- It is not a promise that scores are identical across engines. The set
  of matches is the same; ranking within it may differ.
- It does not forbid an engine from using a native full-text index; the
  index holds the pipeline's output.
