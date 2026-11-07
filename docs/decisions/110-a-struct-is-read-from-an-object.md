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
refuses a value that is an array, and every value beneath it — each
member of a map, each element of a sequence, each variant's content —
is read through it, so the rule holds at any depth. `strict::from_value`,
`strict::read` and `strict::read_slice` are the entry points beside it.

The rule lives on the contract crate, with the declaration types, and
not beside each door, for the reason 028 gives: a door added later
cannot forget it. Every door reads through it:
- the server's tool and REST arguments, its declarations, a forwarded
  `Lakelands-Context` header, and each line of an NDJSON ingest;
- the client's answer and its error body;
- the CLI's configuration, its grants and its `Location`;
- the bundle loader, whose document is read from YAML into one
  `serde_json::Value` and whose declarations are read from it.

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
`deserialize_struct`, so the sentence names the members the object
would hold.

Each door composes it as it composes every other fault, so no door's own
sentence is rewritten: `ArgumentsUnfit` at a tool, `LineNotRecord` at an
ingest line, `BodyNotJson` at a body, the client's "not the contract's",
the configuration and location sentences at the CLI. The path is named
as it is today, through `serde_path_to_error`.

A refusal of form carries a path, not a line and column: those belong to
a text that does not parse, and they stay there. Where a door names only
a line — the ingest, which has no member path to add — the line says
where.

### Only `deserialize_struct` is refused

A tuple struct is asked for through `deserialize_tuple_struct`, a
newtype through `deserialize_newtype_struct`, and a sequence, a tuple, a
map or a string through its own method; all read as they do today. The
contract has no tuple struct of more than one member, and `Names` is a
newtype written as the array of its names. A struct with a flattened
member is read as a map, which already refuses a sequence; `Find` is one
of five such structs in the contract.

### The rows

A row is the door refusing the positional form, with the same value in
its object form still reading:

- a request struct: `find` with `{"aggregate": {"measures": [["n",
  "count"]]}}` is refused naming `name` and `func` at
  `aggregate.measures[0]`; an `order_by` item as `["amount", "value",
  "asc"]` is refused naming `field`, `by` and `dir`;
- a declaration: one given as a sequence is refused naming the members a
  declaration takes, in an argument and on its own door alike;
- a grant: a principal's entry whose grants are a sequence is refused,
  naming the members a grant takes;
- the client: an answer with a struct written as a sequence is refused
  as not the contract's;
- the bundle loader: a declaration written as a YAML block sequence is
  refused, naming its members, and the block mapping loads.

## Why

A struct's fields are named by its type; a sequence names nothing, so a
positional form is a second way to write every member, documented
nowhere, whose meaning changes when a field is added or moved. Measured
before this record: of the structs in `lakelands-api` that derive
`Deserialize`, 93 take a sequence, and only the five with a flattened
member do not.

028 refuses a member a declaration does not take. It cannot refuse a
member that was never named, and an author who writes one member in the
wrong position is told nothing: the lake reads a value that means
something else. Refusing the form is a change to what hosts may send,
and it is taken before a host documents the positional form, not after.

## What it is not

- It is not a change to how anything is written. `Serialize` is
  untouched, and every struct is still written as an object.
- It does not merge the doors' own diagnostics. The server's second read,
  which names a member a struct does not take or one that is missing,
  and the client's and CLI's sentences stay; this record adds one refusal
  beside them.
- It is not where `GroupKey`'s named form is fixed. Its top-level
  sequence is already refused by its own reader, and the object it reads
  through a nested `from_value` belongs to lakelands#720.
- It does not stand in for `deny_unknown_fields`. A member a struct does
  not take, in a form that names its members, is still refused by the
  struct's own declaration, as 028 has it; this rule is about the form,
  not the members.
- It is not a new query language, and it reads nothing a door was not
  given.
