# 110 — A struct is read from an object, and a sequence where one is asked for is refused

Serde reads a struct's fields from an object and, by position, from a
sequence. `deny_unknown_fields` does not stop the second. So every
request, declaration and grant the contract carries may also be written
as a list of its members, unnamed, and a field added or moved silently
changes what such a document means. Decision 028 holds a declaration
strict at every door: a member the type does not take is refused,
naming it. A form in which no member is named is not strict, and 028
does not see it.

## What is decided

### One reader refuses a sequence where a struct is asked for, and it lives on the contract crate

`lakelands-api` gains a `strict` module. A `Reader` over
`serde_json::Value` implements `Deserializer`; its `deserialize_struct`
refuses a value that is an array, and every value it reads is read
through it — each member of a map, each element of a sequence, and the
content of an externally tagged variant. `strict::from_value`,
`strict::read` and `strict::read_slice` are the entry points beside it.

It cannot see everything. Serde reads a value from a buffer of its own,
and the reader over the door's value is never asked, in three cases: the
subtree of a `#[serde(flatten)]` member; the variants of an
`#[serde(untagged)]` enum; and the content of a variant of a tagged enum
(`tag = "…"`, and `tag = "…"` beside `content = "…"`). In each the
derive has already turned the value into serde's private `Content`
before the member, the variants or the variant's content are read, and
it reads them through serde's own deserializer, which takes a struct
from a sequence as ever. So every type the contract reads through one of
those is read by hand instead: `Find`, `Get`, `Pattern` and
`ProposalFilter` (each flattening `ReadAt`), the private `SearchWire`
(also `ReadAt`), the untagged `Condition` and `StatementAnswer`, and
every tagged enum that derives `Deserialize` — `SearchMode`, `Outcome`
(whose `Applied` also flattens `SetOff`), `ProposedWrite` and `Change`,
and the tagged enums of error, event, describe, read, location, seam,
tools, expression and ontology beside them. `Declaration`, hand-read
already and Serialize-only, is on that list for the reason below. Each
takes a `Value`, matches the tag where it has one, splits out the
members it names, and reads every part through `strict`, so a struct
beneath the flattened member, the untagged variant or the tagged variant
is refused too. An array where a tagged enum itself is asked for is
refused naming its tag — `change`, `declare`, `mode`, `outcome`, `write`
— as a declaration's is, and for the same reason: the members a variant
takes are known only once the tag that names it is read. `Search`'s two
`serde_json::from_value` calls become `strict::from_value`, and
`SearchMode` is read by `Search`'s reader from the mode members it
already splits out. `SearchWire`'s `as_of` and `valid_at` are split out
from the members beside them and read as a `ReadAt`, and `Declaration`
and `GroupKey`, hand-read already, read their parts through it as well.

`Declaration` is also why `Change` is on that list. `Declaration` reads
a `serde_json::Map` from whatever deserializer it is given, and beneath
a tag serde has already buffered the value: `Change::Declare`'s
`Vec<Declaration>` is read from `Content`, so the hand reader is given
buffered values and its members are a `Value` again — a struct beneath a
declaration inside a change is read positionally as before, and a
declaration written as an array there is refused by serde's own words
("expected a map"), not by this record's sentence. Hand-reading `Change`
— split `change`, read the rest through `strict` — puts the declaration
back on the door's value. `VerbDecl` keeps the wire struct it already
reads through (`VerbWire`), which takes a map and is read through
`strict` like any other struct.

The rule lives on the contract crate, with the types it reads, and not
beside each door: a door's part is then one call on the value it
already holds, and one rule, not eleven. That is not by itself a
guarantee — a door that parses to a `Value` and then calls
`serde_json::from_value` bypasses it, as `Search` does today. So the
implementation is held by a gate over the door crates and the contract:
it lists every place a text becomes a value or a value becomes a
contract type, and fails on one that does not go through `strict`; it
fails on a type in `lakelands-api` whose `Deserialize` is derived
through a buffer — its serde attributes carry `flatten`, `untagged`,
`tag = ` or `content = ` — unless its own attributes also name
`try_from` or `from`, which makes the derive read that other type and
leaves the buffered attributes to `Serialize` (`VerbDecl` is read
through `VerbWire`; its `flatten` is `Serialize`'s); and an entry may be
exempt only with an issue number, so the list cannot grow silently. A door added later is red until it is listed and reads
through `strict`. Neither the gate nor the reader lands with this
record; both are owed with the implementation.

A door is a place a lakelands crate reads a text it did not write: a
caller's argument or body, a forwarded header, an ingest line, a
configuration, keyring or position file, a bundle document. They are:

- the server's tool and REST arguments, its declarations, a forwarded
  `Lakelands-Context` header, and each line of an NDJSON ingest;
- the client's answer and its error body;
- the CLI's configuration, its grants, its `Location`, and a setting's
  value from the command line (a value, not a struct: it is on the
  gate's list and not under this rule);
- the bundle loader's document;
- the keyring file (`crates/lakelands-backings/src/keyring.rs`) and the
  delivery position file (`crates/lakelands-backings/src/deliver.rs`).

The reader takes a value rather than wrapping a format's deserializer
because it cannot tell, in front of one, whether what comes next is a
sequence: the YAML reader the bundle loader uses forwards `struct` to
its any-value path, as other self-describing readers do, so a wrapper
must first materialise the value to look at it — which is the `Reader`
over a `Value`. Every door already holds or can read one, and a text
that does not parse keeps the line and column its parse gives it.

For the bundle loader this is also fewer parses: the document holds the
value it read, `name` reads it, and `to` reads it through `strict`, so
the text is parsed once instead of three times.

### The refusal names the form that is expected

The refusal is a new `JsonFault::Positional { takes }`, whose sentence
is "its members are named, not given by position: write it as an object
that takes `name`, `func`". `takes` is the field list serde hands
`deserialize_struct`, for a derived struct; a hand-written reader knows
its own members and fills `takes` itself, so the sentence is the same
wherever the refusal is raised.

Each door composes it as it composes every other fault, so no door's own
sentence is rewritten: `ArgumentsUnfit` at a tool, `LineNotRecord` at an
ingest line, `BodyNotJson` at a body, the client's "not the contract's",
the configuration and location sentences at the CLI. The path is named
as it is today, through `serde_path_to_error`.

A refusal of form carries a path, not a line and column: those belong to
a text that does not parse, and they stay there. Where a door names only
a line — the ingest, which has no member path to add — the line says
where.

A declaration's refusal is its own. `Declaration` is hand-read and asks
a `serde_json::Map` today (`crates/lakelands-api/src/write.rs`), so
serde refuses a sequence before the kind is known, with "invalid type:
sequence, expected a map", and no `takes` exists. Its reader matches the
value first and refuses an array itself. An array carries no `declare`,
so no kind is known and `takes` cannot be that kind's members: the
sentence is "write it as an object that takes `declare`", and it is the
same whatever the declaration would have been. Once the value is an
object, the kind is read and the rest is that kind's declaration —
`TypeDecl`, `VerbDecl` and the others — read through `strict` like any
other struct, so a member it does not take is named by 028 and a
positional one by this record. The
declaration door surfaces it as it surfaces every declaration fault, so
its sentence names the form at a tool and in a bundle alike.

### Only `deserialize_struct` is refused

A tuple struct is asked for through `deserialize_tuple_struct`, a
newtype through `deserialize_newtype_struct`, and a sequence, a tuple, a
map or a string through its own method; all read as they do today. The
contract has no tuple struct of more than one member, and `Names` is a
newtype written as the array of its names.

A struct with a flattened member is read as a map, so a sequence at its
own level is not read by `deserialize_struct` at all; serde refuses it
as a map but cannot name what it would have held. It, and every struct
beneath it, is hand-read (above) so that the refusal is the same one.
A tagged enum's variant content is buffered the same way: the struct a
variant holds is read from `Content`, through `deserialize_struct`, so
it would take a sequence and the reader would not be asked. So a tagged
enum is hand-read too, and an array where the enum itself is asked for
is refused naming its tag, which is the member that would have named
the variant beneath the array.

### The rows

A row is the door refusing the positional form, with the same value in
its object form still reading:

- a request struct: `find` with `{"aggregate": {"measures": [["n",
  "count"]]}}` is refused naming `name` and `func` at
  `aggregate.measures[0]`; an `order_by` item as `["amount", "value",
  "asc"]` is refused naming `field`, `by` and `dir`;
- a struct beneath a flattened member: `search` with `"mode": "vector"`
  and `"also": [["a", 1.0]]` is refused naming the vector term's
  `field`, `vector`, `weight` and `model`; `Search`'s `SearchMode` is a
  flattened member and a tagged enum, so both buffers are hand-read and
  what is beneath it is read by hand too, as `Find`'s, `Get`'s,
  `Pattern`'s and `ProposalFilter`'s `ReadAt` are;
- a tagged enum: a change written as a sequence is refused naming
  `change`; a sequence in a change's `declarations` is refused at the
  declaration, naming `declare`; `Outcome` and `ProposedWrite` are
  refused in the same way, naming `outcome` and `write`;
- an untagged answer: an `Outcome` or a `Reply` written as a sequence
  is refused as not the contract's, where an answer is read;
- a declaration: one given as a sequence is refused as an object whose
  members are named — "write it as an object that takes `declare`" — in
  an argument and on its own door alike; the declaration's `type` door
  reads `type` with `plan` and `plan_id` beside it, and a sequence there
  is refused naming those three;
- a grant: a principal's entry whose grants are a sequence is refused,
  naming the members a grant takes;
- the client: an answer with a struct written as a sequence is refused
  as not the contract's;
- the bundle loader: a declaration written as a YAML block sequence is
  refused, naming its members, and the block mapping loads.

## Why

A struct's fields are named by its type; a sequence names nothing, so a
positional form is a second way to write every member, documented
nowhere, whose meaning changes when a field is added or moved.

Counting at this record's tree: `struct` items with a named-field body
in `crates/lakelands-api/src` whose `#[derive(...)]` names
`Deserialize`, any visibility, test files included — 159. Six take a
map rather than a sequence: five flatten a member (`Find`, `Get`,
`Pattern`, `ProposalFilter` and the private `SearchWire`), and
`VerbDecl` is read through a wire struct that takes a map; so 153 take
a bare sequence. The issue's 93 counted `pub struct` only, against a
per-file table that missed `id.rs` and undercounted `ontology`, `read`,
`location` and `event`; a count means nothing without the rule that
made it.

028 refuses a member a declaration does not take. It cannot refuse a
member that was never named, and an author who writes one member in the
wrong position is told nothing: the lake reads a value that means
something else. This is a breaking change to what a host may send: a
host that writes a positional form today is refused once the
implementation lands. It is decided here, before a host documents the
form and not discovered in the code that refuses it, and delivered with
the implementation.

## What it is not

- It is not a change to how anything is written. `Serialize` is
  untouched, and every struct is still written as an object.
- It does not merge the doors' own diagnostics. The server's second read,
  which names a member a struct does not take or one that is missing,
  and the client's and CLI's sentences stay; this record adds one refusal
  beside them.
- It is not where `GroupKey`'s named form is fixed. Its top-level
  sequence is already refused by its own reader; the object it reads
  through a nested `from_value` belongs to lakelands#720, and the gate
  lists that read as exempt with that issue's number until it does.
- It does not stand in for `deny_unknown_fields`. A member a struct does
  not take, in a form that names its members, is still refused by the
  struct's own declaration, as 028 has it; this rule is about the form,
  not the members.
- It is not a new query language, and it reads nothing a door was not
  given.
