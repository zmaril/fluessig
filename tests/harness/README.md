# Stream async-iterable — hand-run harness

`async_iterable_contract.mjs` validates the **observable JS contract** that the
Node stream codegen targets: the `#[napi(async_iterator)]` class fluessig emits
for every `stream` op must behave, from JavaScript, like a well-formed
async-iterable.

## Why a mock?

fluessig is a **codegen tool** — `node_binding()` emits Rust source (with napi
macros) as a `String`; it never compiles that source, and it cannot build a
native Node addon in its own CI. So this harness cannot import a real generated
class. Instead it constructs a **mock** async-iterable with the exact observable
contract the generated class must satisfy, backed by a fake poll source with the
same semantics as the core `PollStream::poll` / `close()` primitive.

## Run

```sh
node tests/harness/async_iterable_contract.mjs
```

Plain `node`, no build step, no dependencies (only `node:assert`). It prints a
`PASS:` line per case and exits non-zero on any failure. Cases:

1. **order** — `for await` consumes all events in order, skips idle polls, stops
   at `done`.
2. **break** — an early `break` triggers the iterator's `return()`, which calls
   the core `close()` exactly once and stops cleanly.
3. **error** — a failure from the poll source rejects the awaited `next()`, so
   `for await` throws.

## Validating a REAL generated addon

To exercise the actual emitted Rust, a **consumer** (not fluessig) builds their
napi crate from the generated `node.rs` and points an equivalent
for-await / break / error script at the built class. The generated class exposes
both surfaces — the async-iterable and the retained `next()` poll cursor:

```js
// consumer-addon.node is the consumer's built napi addon
const { Watch } = require("./consumer-addon.node");

const watch = new Watch("/some/path");
const stream = watch.events(); // -> the generated `Events` class

// primary surface: async-iterable
for await (const ev of stream) {
  console.log(ev);
  if (someCondition(ev)) break; // triggers return() -> PollStream::close()
}

// retained surface: poll cursor (feature-independent fallback)
let ev;
while ((ev = await stream.next()) !== null) console.log(ev);
```

The real generated class drives the blocking `PollStream::poll` off the runtime
via `napi::tokio::task::spawn_blocking` (needs napi's default-on `tokio_rt`
feature), yields one in-flight pull at a time, and closes the core stream on
cancellation (`complete()`) and on `Drop`. This mock reproduces exactly those
observable semantics without the native build.
