# 019 — The contract is object-safe, and the engine seam is synchronous

## What is decided

`Catalog` and the store facade `LakeStore` are `dyn`-compatible: a door
holds `Arc<dyn Catalog>`, a host holds `Arc<dyn LakeStore>`, and the
in-memory mock, the real store and a remote client are interchangeable
behind them. They are asynchronous. `ingest` therefore takes a boxed
stream, `Pin<Box<dyn Stream<Item = SourceRecord> + Send>>`, and answers
a boxed stream of commit acknowledgements. Every other seam is
`dyn`-compatible too.

The engine traits (`LakeEngine`, `ProjectionStore`, `ProjectionView`,
`VectorIndex`) are synchronous. The store runs them off its async
callers.

`lakelands-api` performs no I/O, depends on no engine and names no async
runtime; its only asynchronous vocabulary is the `Stream` trait and
boxed futures.

## Why

The doors are built and tested before the store exists, against a mock,
and must keep passing the same tests against the store afterwards. That
only works if the mock and the store are two implementations of one
object-safe trait. A remote client is a third implementation of the same
trait, so a program can move a lake out of process without changing a
line that calls it.

Engines do blocking storage work, and SQLite, the reference engine, is
synchronous. Making the engine seam asynchronous would force every
engine through an async adapter and make each method harder to reason
about, for no gain: the store already decides where blocking work runs.

Keeping the contract free of a runtime keeps it usable from any host and
leaves room for the deterministic core to compile where no runtime
exists.

## What it is not

- It is not a promise that every trait is cheap to call through `dyn`.
  The indirection is per call, not per record.
- It does not let an engine block the caller's executor; that is the
  store's job to prevent.

## Extended: one hand-off

The store dispatches every call into an engine through one place, a
single hand-off to the runtime's blocking pool. Nothing in the store
starts a runtime of its own or parks a caller's executor, so a slow disk
delays the call that needs it and nothing else.
