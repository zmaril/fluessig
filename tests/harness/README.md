<!-- straitjacket-allow-file:duplication — the Python harness section below is
DELIBERATELY parallel to the Node one (same "Why a mock?" / "Run" / "Validating
a REAL …" structure): the two harnesses assert the SAME async-iterable contract
in two languages, so their prose mirrors by design — the same rationale the
per-language bindgen files carry their own file-level marker. -->

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

---

# Python stream async-iterable — hand-run harness

`async_iterable_contract.py` is the **Python analogue** of the `.mjs` harness: it
validates the **observable Python contract** the PyO3 stream codegen targets. The
class fluessig emits for every `stream` op must behave, from Python, like a
well-formed async-iterable — `async for ev in stream` (`__aiter__` returning
`self`, `__anext__` returning an awaitable), alongside the retained sync
`__iter__`/`__next__` poll cursor.

## Why a mock?

Same reason as the Node harness: fluessig `python_binding()` emits **Rust source**
(with PyO3 macros) as a `String`; it never compiles that source, and it cannot
build a Python extension module in its own CI. So this harness constructs a
**mock** async-iterable with the exact observable contract the generated class
must satisfy, backed by a fake poll source with the same semantics as the core
`PollStream::poll` / `close()` primitive.

## Run

```sh
python3 tests/harness/async_iterable_contract.py
```

Plain `python3`, no build step, no third-party dependencies (only `asyncio`). It
prints a `PASS:` line per case and exits non-zero on any failure. Cases:

1. **order** — `async for` consumes all events in order, skips idle polls, stops
   at `StopAsyncIteration`.
2. **break** — an early `break` + `aclose()` calls the core `close()` exactly
   once and stops cleanly (the `Drop` backstop analogue — PyO3 has no
   async-generator `complete()` hook, so `Drop` is the cancellation seam).
3. **error** — a failure from the poll source raises out of `async for`
   (throw-mode; the default, unannotated error model).

## Validating a REAL generated extension

To exercise the actual emitted Rust, a **consumer** (not fluessig) builds their
PyO3 crate from the generated `python.rs` and points an equivalent
`async for` / break / error script at the built class. The generated class
exposes both surfaces — the async-iterable and the retained sync `__next__` poll
cursor:

```py
# consumer_ext is the consumer's built PyO3 extension module
import asyncio
from consumer_ext import Watch

async def main():
    watch = Watch("/some/path")
    stream = watch.events()          # -> the generated `Events` class

    # primary surface: async-iterable
    async for ev in stream:
        print(ev)
        if some_condition(ev):
            break                    # -> Drop closes the core PollStream

asyncio.run(main())

# retained surface: sync poll cursor (no asyncio required)
for ev in watch.events():
    print(ev)
```

The real generated class drives the blocking `PollStream::poll` off the asyncio
loop via `pyo3_async_runtimes::tokio::future_into_py` +
`tokio::task::spawn_blocking` (needs the consumer's `pyo3-async-runtimes` +
tokio-runtime bridge — the pyo3 analogue of napi's `tokio_rt`), yields one
in-flight pull at a time, ends the stream by raising `StopAsyncIteration`, and
closes the core stream on `Drop`. Under the opt-in `@streamError` (event-mode)
contract a mid-stream failure is instead yielded as a terminal `<Op>ErrorEvent`
value and the stream latches closed — it never raises. This mock reproduces
those observable semantics (throw-mode) without the native build.

---

# Ruby stream each/Enumerator — hand-run harness

`each_enumerator_contract.rb` is the **Ruby analogue** of the `.mjs` / `.py`
harnesses: it validates the **observable Ruby contract** the Magnus stream
codegen targets. The class fluessig emits for every `stream` op must behave, from
Ruby, like a well-formed `each`-able — `stream.each { |ev| ... }` yields each
event to the block, and `stream.each` with **no block** returns an `Enumerator`
(so `.lazy` / `.map` / `.next` compose), alongside the retained `.next` poll
cursor.

## Why a mock?

Same reason as the Node and Python harnesses: fluessig `ruby_binding()` emits
**Rust source** (with Magnus macros) as a `String`; it never compiles that
source, and it cannot build a native Ruby extension in its own CI. So this
harness constructs a **mock** `each`-able with the exact observable contract the
generated class must satisfy, backed by a fake poll source with the same
semantics as the core `PollStream::poll` / `close()` primitive.

## Run

```sh
ruby tests/harness/each_enumerator_contract.rb
```

Plain `ruby`, no build step, no gems (only the stdlib). It prints a `PASS:` line
per case and exits non-zero on any failure. Cases:

1. **order** — a block consumes all events in order, skips idle polls, stops at
   `Poll::Closed`; `each` returns the receiver.
2. **enumerator** — a no-block `each` returns an `Enumerator`, and `.next` /
   `.lazy.map` compose off it.
3. **break** — an early `break` triggers the core `close()` exactly once (the
   `Drop`-backstop analogue — Ruby has no deterministic destructor, so the mock
   models it with an `ensure`).
4. **throw** — a throw-mode failure source raises out of `each` (the default,
   unannotated error model).
5. **event** — an `@streamError` (event-mode) failure is yielded as a terminal
   `{ type:, reason:, error: }` event and the block then ends — it never raises.

## Validating a REAL generated extension

To exercise the actual emitted Rust, a **consumer** (not fluessig) builds their
Magnus crate from the generated `ruby.rs` and points an equivalent
`each` / break / error script at the built class. The generated class exposes
both surfaces — the idiomatic `each` and the retained `.next` poll cursor:

```rb
# consumer_ext is the consumer's built Magnus extension
watch = Watch.new("/some/path")
stream = watch.events            # -> the generated `Events` class

# primary surface: each with a block
stream.each do |ev|
  puts ev
  break if some_condition(ev)    # -> Drop closes the core PollStream
end

# no block: an Enumerator, so .lazy/.map/.next compose
watch.events.each.lazy.map { |ev| transform(ev) }.first(10)

# retained surface: the low-level poll cursor
while (ev = stream.next)         # nil ends iteration
  puts ev
end
```

The real generated class calls the blocking `PollStream::poll` **under the GVL**
(the same as the retained `.next` cursor — a documented GVL-release follow-up,
see [`notes/async-iterable-streams-ruby.md`](../../notes/async-iterable-streams-ruby.md)),
yields one item at a time, ends at `Poll::Closed`, and closes the core stream on
`Drop`. Under the opt-in `@streamError` (event-mode) contract a mid-stream
failure is instead yielded as a terminal `<Op>ErrorEvent` value and the block
then ends — it never raises. This mock reproduces those observable semantics
without the native build.
