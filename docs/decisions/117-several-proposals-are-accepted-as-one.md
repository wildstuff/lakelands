# 117 — Several proposals are accepted as one append, all or none

A reviewer who accepts a set of proposals that belong together, such as
a change spread over several records, had to accept them one by one.
If the third accept was refused, the first two had already landed, and
the lake held a part of a change no one meant to leave half done. The
host had to undo it, or keep its own rules for which proposals may be
accepted together. So this record adds one door that accepts a list of
proposals together.

## What is decided

### One door on the caller's contract, and no tool

**`Catalog` gains `accept_many(who, proposals)`, asked by a principal.**
It answers one `Outcome` for each proposal, in list order, or one
refusal.

- The principal is checked as at `accept`: the context the `Catalog`
  was opened with, then the `review` grant.
- The WIT `catalog` resource gains `accept-many`. Its refusal is the
  `accept-error` record: the place in the list and the proposal's id,
  beside the error itself. The package version moves at the next
  release (034).
- The REST door is `POST …/proposals/accept_many`, with the body
  `{"proposals": [...]}`. There is no tool. Deciding proposals as a
  batch is a reviewer's act at the host, not a model's (014).

### All or none, each judged as the list left the lake

- Every proposal is judged by the same checks as its `accept`, in list
  order. A proposal is judged as the ones before it in the list left
  the lake, not as the head stood when the list was asked. A pinned
  write whose record an earlier entry moved is refused as changed since
  it was proposed, as it would be if the earlier one had been accepted
  alone first.
- The reads a verb declared are taken from the lake as committed, not
  as the entries before it left it, as at a lone `accept`. What an
  earlier entry wrote reaches a later one through the pinned records
  above, not through the verb's reads.
- The acceptances and their writes are sealed as one append. Their
  events are contiguous after the head, and each acceptance is followed
  by its own write, caused by it, as at `accept`.
- A refusal of any entry refuses the whole list with
  `Error::Accepting { index, proposal, error }`, where `error` is the
  refusal that entry's own accept would have had. Nothing is appended,
  and every proposal of the list stays pending.
- An entry whose proposal id a later entry repeats is refused as
  repeated at its second place, naming the place of its first copy: the
  second copy was never decided by anyone, it duplicates the first
  within the same call. A proposal already decided is refused as
  decided.

### What a list holds

- An upsert, a retirement or a verb call.
- A declaration bundle, because a host's change folder files its
  declarations as proposals and accepts the folder as one. Each is
  judged at the head the entries before it in the list left,
  declarations included: a bundle's declarations are applied to the
  ontology the next entry is judged at, as the fold will apply them, so
  a bundle that redeclares a type the list already declared differently
  — or that was declared since the bundle was proposed — is refused as
  invalid naming the type, as a lone accept of it after the earlier one
  would be. A pinned write whose type an earlier entry redeclared is
  refused as changed at its place: it was judged on a version the list
  moved. The ontology advances through the list and is then put back
  before the one append, so the declarations are folded once, in list
  order.
- A call of a verb whose effect is handed to an executor is refused as
  the wrong door. Its handoff answers for one acceptance, and a list
  has no handoff of its own to carry it.
- A migration — of a type or of a value type — is refused as the wrong
  door. It rewrites the records under the type with its own event, and
  a later entry of the list reads the records this batch began on, not
  the ones the migration leaves: applied in a list it would be judged,
  and judged others, on a lake no accept could have left. For the same
  reason any other proposed write that decides over many records — a
  `records` change, a computation — is refused as the wrong door and is
  accepted alone.

## Why

A batch accepted in part is a change no one asked for. Judging each
entry as the earlier ones left the lake keeps every refusal the same as
the one-by-one accepts would give. The only difference is that a
refusal leaves nothing behind, so a host never has to undo a batch.
Naming the place and the id lets the reviewer drop or fix that one
proposal and ask again.

## How it came to be

- Added at lakelands#1802.
