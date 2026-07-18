#!/usr/bin/env python3
"""Hand-run harness: the async-iterable CONTRACT the Python stream codegen targets.

fluessig only EMITS Rust source (PyO3 macros); it cannot build a Python
extension here. So this script validates the observable Python contract the
generated stream class must satisfy, using a MOCK async-iterable backed by a
fake poll source with the same semantics as the core `PollStream::poll` +
`close()` primitive. See README.md for how to run the same checks against a REAL
built extension module.

Run:  python3 tests/harness/async_iterable_contract.py

The mock mirrors the generated class:
  - `__aiter__()` returns `self` (like the generated `__aiter__`).
  - `__anext__()` returns an awaitable that drives a blocking-style poll:
      Item  -> the value; Idle -> keep polling; Closed -> raise
      StopAsyncIteration; Failed -> raise (throw-mode).
  - `aclose()` / early `break` calls the core `close()` exactly once,
    idempotently (the Drop backstop analogue).
"""

import asyncio
import sys

# A fake core poll source with the same shape as `PollStream`:
#   poll() -> ("item", value) | ("idle",) | ("closed",) | ("failed", error)
# plus an idempotent close() that counts its calls.
class Source:
    def __init__(self, script):
        self._script = list(script)
        self._i = 0
        self._closed = 0

    def poll(self):
        if self._i >= len(self._script):
            return ("closed",)
        p = self._script[self._i]
        self._i += 1
        return p

    def close(self):
        self._closed += 1

    def close_count(self):
        return self._closed


# The mock async-iterable — the observable contract the generated class emits.
# `__anext__` is `async` (a coroutine is a Python awaitable, the same shape
# `future_into_py` hands back), and drives the poll loop off the event loop.
class Stream:
    def __init__(self, source):
        self._source = source

    def __aiter__(self):
        return self

    async def __anext__(self):
        while True:
            # Drive the blocking poll off the loop (spawn_blocking analogue).
            poll = await asyncio.to_thread(self._source.poll)
            kind = poll[0]
            if kind == "item":
                return poll[1]
            if kind == "idle":
                continue
            if kind == "closed":
                raise StopAsyncIteration
            if kind == "failed":
                # throw-mode: raise out of the awaited pull; `async for` throws.
                raise poll[1]

    async def aclose(self):
        # Consumer cancelled (e.g. `break` in `async for`): close the core.
        self._source.close()


def item(value):
    return ("item", value)


IDLE = ("idle",)
CLOSED = ("closed",)


failures = 0


def _pass(name):
    print(f"PASS: {name}")


def _fail(name, err):
    global failures
    failures += 1
    print(f"FAIL: {name}", file=sys.stderr)
    print(f"  {err!r}", file=sys.stderr)


# Case (a): `async for` consumes all events IN ORDER and stops at StopAsyncIteration.
async def case_order():
    name = "async for consumes all events in order and stops at StopAsyncIteration"
    source = Source([item("a"), IDLE, item("b"), item("c"), CLOSED])
    stream = Stream(source)
    seen = []
    async for ev in stream:
        seen.append(ev)
    assert seen == ["a", "b", "c"], f"events arrive in order: {seen}"
    assert source.close_count() == 0, "natural exhaustion does not call close()"
    _pass(name)


# Case (b): early `break` + aclose() triggers core close() exactly once.
async def case_break():
    name = "early break + aclose() calls core close() exactly once"
    source = Source([item("a"), item("b"), item("c"), CLOSED])
    stream = Stream(source)
    seen = []
    ait = stream.__aiter__()
    try:
        while True:
            ev = await ait.__anext__()
            seen.append(ev)
            if ev == "b":
                break  # cancellation
    except StopAsyncIteration:
        pass
    finally:
        await ait.aclose()
    assert seen == ["a", "b"], f"stops at the break: {seen}"
    assert source.close_count() == 1, "aclose() closed the core exactly once"
    _pass(name)


# Case (c): an error source raises out of `async for` (throw-mode).
async def case_error():
    name = "error source raises out of async for (throw-mode)"
    boom = RuntimeError("poll source exploded")
    source = Source([item("a"), ("failed", boom)])
    stream = Stream(source)
    seen = []
    threw = None
    try:
        async for ev in stream:
            seen.append(ev)
    except RuntimeError as e:
        threw = e
    assert seen == ["a"], f"events before the error still arrive: {seen}"
    assert threw is boom, "async for re-raises the source error"
    _pass(name)


async def main():
    for name, fn in [
        ("order", case_order),
        ("break", case_break),
        ("error", case_error),
    ]:
        try:
            await fn()
        except Exception as e:  # noqa: BLE001 — harness reports, never crashes
            _fail(name, e)
    if failures > 0:
        print(f"\n{failures} case(s) failed.", file=sys.stderr)
        sys.exit(1)
    print("\nAll async-iterable contract cases passed.")


if __name__ == "__main__":
    asyncio.run(main())
