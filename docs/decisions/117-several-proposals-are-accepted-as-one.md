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
- The acceptances and their writes are sealed as one append. Their
  events are contiguous after the head, and each acceptance is followed
  by its own write, caused by it, as at `accept`.
- A refusal of any entry refuses the whole list with
  `Error::Accepting { index, proposal, error }`, where `error` is the
  refusal that entry's own accept would have had. Nothing is appended,
  and every proposal of the list stays pending.
- An entry listed twice is refused as decided at its second place, and
  so is a proposal already decided.

### What a list holds

- An upsert, a retirement or a verb call. A declaration bundle, a
  migration or any other proposed write is refused as the wrong door
  and is accepted alone. Each of those changes what the next entry
  would be judged against in ways a single accept already guards.
- A call of a verb whose effect is handed to an executor is refused as
  the wrong door too. Its handoff answers for one acceptance, and a
  list has no handoff of its own to carry it.

## Why

A batch accepted in part is a change no one asked for. Judging each
entry as the earlier ones left the lake keeps every refusal the same as
the one-by-one accepts would give. The only difference is that a
refusal leaves nothing behind, so a host never has to undo a batch.
Naming the place and the id lets the reviewer drop or fix that one
proposal and ask again.

## How it came to be

- Added at lakelands#1802.
