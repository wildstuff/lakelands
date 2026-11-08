# 112 — An expression extracts the text a pattern matches

## What is decided

### `extract(text, pattern)` answers the text a pattern matches, or null

The expression language gains one function. `extract(s, p)` takes two
arguments and answers text: the part of `s` that the first match of the
pattern `p` covers, or null when there is no match or when `s` is null.
`s` is read as text as `matches` reads it (a number, an instant, a
duration or a boolean is its written form), and a null `s` answers null
before the pattern is consulted. The operands are the only two arguments
it takes.

The pattern describes the value itself; there are no capture groups,
because the I-Regexp grammar has none. `extract(purpose, 'R-[0-9]+')`
answers `R-1001` out of a purpose that holds it.

### The pattern is a quoted I-Regexp, and both refusals come before any record runs

`p` is an I-Regexp exactly as `matches` takes one: the same grammar, the
same parser, the same refusals of what is outside it, and the same
translation to the matcher. A pattern that is not an I-Regexp is refused
as `not_i_regexp`, with the pattern and the reason.

The pattern must be written as a quoted text literal. A call whose second
argument is anything else — a field, a concatenation, a setting — is
refused as `extract_takes_literal`, because the check must know the
pattern before any record runs. A literal that is not an I-Regexp is
refused by the same check. Neither refusal waits for a record, so a
declaration, a verb or a read that carries an `extract` with a bad
pattern is refused when it is checked, even over a type that holds no
records.

`matches` is not changed by this. It still takes a computed pattern,
because making it literal-only would newly refuse expressions that are
accepted today; `extract` is the function that names the pattern in its
text.

### Among matches at the leftmost position, the first alternative wins

The answer is the leftmost match: the smallest offset in `s` at which
the pattern matches any segment. Among the segments that begin at that
offset, the one the pattern names first wins, read left to right and
with each greedy quantifier taking as much of `s` as the rest of the
pattern allows. This is the whole rule:

- `extract('ab', 'a|ab')` is `'a'` — the first alternative, though the
  second is longer;
- `extract('ab', 'ab|a')` is `'ab'` — the order is the pattern's;
- `extract('aaab', 'a+')` is `'aaa'` — a greedy quantifier takes as much
  as it can;
- `extract('ba', 'a*')` is `''` — an empty match at the leftmost offset
  is a match.

Longest-at-that-position was measured and not taken. The matcher the
store compiles with fixes this order in its own terms: it offers one
leftmost order, alternatives in the pattern's order and greedy
quantifiers as far as the rest allows, and its other order is to report
every match, not the longest one. It has no order that prefers the
longest match. Taking longest would mean a second automaton built for
that order, a different answer for the same text and pattern, and a rule
no engine could reach by compiling the pattern the plain way.

### An empty match is the empty text, and only no match is null

A pattern that matches an empty segment — `a*` over a text with no `a` —
answers `''`, the empty text, not null. The distinction is the caller's:
`''` is a value a text field holds and a comparison can name, while null
is what a derived field reads as absent. Null is answered only where the
pattern matches nowhere in the text or where `s` itself is null.

### The pattern describes a segment, so the text is searched

`matches` asks whether the whole text is a segment the pattern describes,
and translates the pattern anchored at both ends. `extract` asks for the
leftmost segment the pattern describes, so its translated pattern is not
anchored; a pattern that matches nothing in the first position is tried
at the next. This one difference — searching rather than anchoring — is
the whole of the difference between the two functions' patterns.

### A derived field, a filter and a view group take a null answer as any null

- **A derived field** holds the answer. On a match the text lands in the
  field, `''` included; on no match the answer is null, and a null
  derived value is absent from the record as it is read, the same as any
  derived field whose expression computes null. The value lands as the
  field's declared kind judges it, as any derived value does.
- **A filter, a guard or a constraint** compares the answer as text. A
  null answer is null, and 023's rule holds: a comparison with null is
  false, `!=` included, so only `is null` and `is not null` ask about
  the absent answer. A bare `extract(...)` where a condition is taken
  holds nowhere, since it answers text or null and never a boolean.
- **A view grouped by the answer** groups it as any value: a null answer
  is the missing group, and `''` is a group of its own. Groups stand in
  the order of their key's canonical bytes, as every group does.
- **A view's measure** that answers `extract` lands as any measure of
  its kind; a null answer lands by 023's rule for a value that is not
  there.

`extract` is evaluated in the store, as `matches` is. A filter that
carries it is not pushed down to an engine's query, so every engine and
every evaluator the host runs answer by this one rule.

### A match costs the text's length, and the compiled form is bounded

An I-Regexp describes a regular language: it has no back-references, no
lookaround and no lazy quantifier, so the matcher compiles it to a finite
automaton whose search visits the text a bounded number of times. A
match therefore costs time linear in the length of `s`; no pattern makes
the match backtrack.

The compiled form is bounded before it runs. A pattern whose automaton
would pass the matcher's compile limit — 10 MiB — is refused as
`regex_unmatchable`, naming the pattern and the limit, at the first
evaluation. That refusal stays where `matches` pays it, because the
check parses the I-Regexp grammar and does not compile an automaton; only
a pattern outside the grammar is refused before a record runs.

Compiling is paid when the expression is evaluated, as `matches` pays it.
No cache is part of the contract: an engine that compiles a pattern once
per query and reuses it answers the same, and an evaluator that compiles
it per record answers the same too.

### The grammar, the catalog and the error cases carry the function

`extract` joins the closed set of functions and every place that names
one: the text form's name and back, the arity table (two arguments,
exactly), the kind table (it certainly computes text or null, so the
kind depends on the record), the module documentation beside `matches`,
the log format's function list beside `matches`, and the seams page's
list of functions the store evaluates itself.

Two error sentences are reused with their meaning unchanged, as 015
holds: `not_i_regexp` and `regex_unmatchable`. One is new — a call whose
pattern is not a quoted literal is `extract_takes_literal`.

## Why

A host that reads a value out of free text — an invoice number out of a
payment's purpose — has two choices today: write the pattern in its own
language, or ask the lake for a filter that tests the text and then
parse the record again outside it. Both put the extraction in a second
place, and the two places disagree the moment either changes. One
function in the expression language makes the extraction part of the
contract, answered by the same evaluator on every engine, so a derived
field, a filter and a view group all see the same text.

The pattern is literal because a pattern refused before any record runs
must be known when the expression is checked. A computed pattern cannot
be refused there: it is known only per record, and a filter over a type
that holds no records would never see the refusal at all.

## What it is not

- It is not a second regular-expression dialect. The grammar is the one
  `matches` already takes, refused and translated by the same parser.
- It is not capture groups or a replacement. The pattern describes the
  value; a host that needs several values writes a second `extract`, or
  a derived field of its own.
- It does not change `matches`, and it does not make `matches` take a
  literal pattern only. Crowding `matches` into `extract`'s check would
  refuse expressions a lake accepts today.
- It is not an error on no match. No match is null, which is how an
  absent value is already spelled; a caller that wants the empty text
  asks with a pattern that matches one.
- It does not promise a cache. The contract bounds a match and the
  compiled form, not the number of times a pattern is compiled.
