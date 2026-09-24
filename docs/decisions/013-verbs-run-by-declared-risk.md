# 013 — A verb runs by its declared risk

## What is decided

Every verb declared in the ontology declares a risk. When a verb is
called, the store looks at that declaration and nothing else:

- a verb declared `low` whose effect is writing authored fields runs at
  once, as a governed write under the caller's grants;
- a verb with any other risk does not run. The call is recorded as a
  `WriteProposed` event, and it takes effect only when someone with the
  right to do so accepts it (`WriteAccepted`) or is discarded when they
  reject it (`WriteRejected`).

A verb whose effect reaches outside the lake runs only through a
`VerbExecutor` the host has bound; without one, the store says so
rather than pretending to run it.

## Why

A verb's declaration is where its author says how dangerous it is. A
default that ran anything the store could technically execute would
ignore the one piece of information meant to stop it. Making the
proposal a state in the log, rather than a queue in some other system,
means a pending action survives restarts, can be listed and audited like
any other write, and is accepted or rejected by the same principals and
grants as everything else.

## What it is not

- It is not an approval workflow. Who reviews a proposal, and how they
  are notified, is the host's business; the store records the proposal,
  the acceptance and the rejection.
- A caller cannot lower a verb's risk at call time. The declaration
  decides.
- `propose`, `accept` and `reject` are also plain doors: any write can
  be proposed rather than applied, not only a verb.

## Extended: a `low` verb that reaches outside the lake

A verb declared `low` whose effect reaches outside the lake runs at
once, like any `low` verb, through the `VerbExecutor` the host bound for
it; the executor's answer is recorded in the log. Declared risk is the
only thing that decides between running and proposing, whatever the
effect.

A verb that reaches outside the lake, of any risk, is refused as
`unbound` at call time when no executor is bound for it: never faked,
and never recorded as a proposal, because a proposal that could not run
when accepted would be a promise the store cannot keep.
