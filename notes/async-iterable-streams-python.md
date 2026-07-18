# Python (PyO3) stream ops: async-iterable projection + dual error model

Companion to [`async-iterable-streams.md`](./async-iterable-streams.md) (the Node
contract). This documents the PyO3 half (P2): every `stream` op now projects a
genuine Python **async-iterable** — `async for ev in stream` — adapted from the
already-merged Node contract to PyO3, alongside the retained SYNC poll cursor.

## Surfaces

Each `stream` op's generated class carries **both** iteration surfaces:

- **Primary — async-iterable.** `__aiter__(self) -> self` plus
  `__anext__(self) -> awaitable`. `async for` drives one awaited pull at a time.
- **Retained — sync poll cursor.** `__iter__(self) -> self` plus
  `__next__(self) -> Optional[item]` (fallible since P1). The feature-independent
  fallback for consumers not on asyncio — it needs no async runtime bridge.

## Mechanism

- **`future_into_py` builds the awaitable.** `__anext__` returns a Python
  awaitable produced by
  `pyo3_async_runtimes::tokio::future_into_py(py, async move { … })`
  (`-> PyResult<Bound<'p, PyAny>>`). This bridges a tokio future onto the running
  asyncio loop — it is the **pyo3 analogue of napi's `AsyncGenerator` + `tokio_rt`**.
- **`pyo3-async-runtimes` is a CONSUMER dependency.** The generated source
  references `pyo3_async_runtimes::tokio::future_into_py` and
  `tokio::task::spawn_blocking`, so the **consumer's** PyO3 crate must depend on
  `pyo3-async-runtimes` (with its tokio feature) and `tokio`, and initialise a
  tokio runtime bridge for the async surface to work. fluessig itself only emits
  source strings — it does **not** compile them, so this dependency is **not**
  added to fluessig's `Cargo.toml`. This mirrors how Node relies on napi's
  default-on `tokio_rt` re-export; PyO3 has no such re-export, so the consumer
  wires it explicitly. (The retained sync `__next__` cursor needs none of this.)
- **`spawn_blocking` keeps the loop free.** The core primitive is a BLOCKING
  `PollStream::poll(&self, timeout) -> Poll<T>`. Inside the async block the
  handle is driven via
  `tokio::task::spawn_blocking(move || s.poll(Duration::from_millis(500))).await`,
  so the asyncio event loop is never blocked. The `Arc` is cloned **before** the
  `async move` (and again per-iteration for the spawned closure), so the future
  is `'static` and borrows nothing across `.await`.
- **`Box` → `Arc`.** The stream field changed from
  `stream: Box<dyn PollStream<item>>` to `stream: Arc<dyn PollStream<item>>`:
  the `'static` async future moves the handle across `.await` / `spawn_blocking`,
  which a `Box` cannot survive being cloned into. The ctor dispatch wraps the
  core handle with `Arc::from(self.core.<name>(<args>).map_err(err)?)` (the shared
  core trait still returns `Box<dyn PollStream>`; Node converts it identically).
- **End of stream = `StopAsyncIteration`.** `Poll::Closed` maps to
  `Err(PyStopAsyncIteration::new_err(()))` out of the awaitable (`use
  pyo3::exceptions::PyStopAsyncIteration;` in the prelude). This is the async
  analogue of the sync cursor's `Ok(None)` → `StopIteration`.
- **Item type is `python_ty`-resolved.** The yielded item type comes from
  `python_ty(api, opts, &op.returns)`, NOT the shared `ty()`, so a union-returning
  stream yields the structured `{Union}Union` carrier correctly — it tracks the
  structured-union / fan-out state the rest of the Python backend uses.

## Cancellation — the `Drop`-only caveat

Node's napi surface has an `AsyncGenerator::complete()` hook that fires when the
consumer stops early (e.g. `break` in `for await`). **PyO3 has no such hook.** So
the ONLY cancellation seam is `impl Drop for <Class> { fn drop(&mut self) {
self.stream.close(); } }`, emitted in **both** error modes. This is a genuinely
**weaker** guarantee than Node's `complete()` + `Drop` pair: core-side `close()`
runs when the Python object is garbage-collected, not deterministically at the
`break`. `close()` is a documented idempotent default-no-op on the `PollStream`
trait, so poll-only cores need no change.

## Dual error model — throw vs. event terminal split

The error model is chosen per-op by `op.stream_error`, mirroring Node exactly
(`Poll::Failed(String)` is the core→binding channel in BOTH modes; only the
mapping differs):

- **`None` (unannotated) — DEFAULT throw-mode.** A mid-stream `Poll::Failed(e)`
  maps to `Err(err(e))`, so the awaited `__anext__` (and the sync `__next__`)
  **raises** a Python exception. Safe by default, no silent-swallow. The async
  future's element type is the homogeneous `item`, so `Ok(v)` needs no explicit
  conversion.
- **`Some(shape)` (`@streamError`) — opt-in error-AS-EVENT.** A mid-stream
  `Poll::Failed(e)` is **yielded as a terminal `<Class>ErrorEvent` value** and the
  stream then latches closed via `closed: Arc<AtomicBool>` — the next pull ends
  the stream (`StopAsyncIteration` async / `Ok(None)` sync). It **NEVER raises**.
  Because the item and the error event are **distinct types** (Python has no
  `Either`), the async future resolves to a `Py<PyAny>` and the sync cursor
  returns `Optional[Py<PyAny>]` — the heterogeneous yield is **erased to a Python
  object** (`v.into_pyobject(py)?.into_any().unbind()`), constructed under a
  re-acquired GIL (`Python::attach`) inside the async block.
  - The `<Class>ErrorEvent` is a `#[pyclass(get_all)]` DTO with three `String`
    getters named via `se.tag_name` / `se.reason_name` / `se.error_name` (a
    `#[pyo3(name = …)]` attr only where the python getter name diverges from the
    rust ident — the tag always needs one). On construction the tag field =
    `se.tag_value`, `reason` = `"error"`, `error` = the `e` from `Poll::Failed(e)`
    — mirroring Node's field values.
  - **Registration is required.** Python has no auto-registration, so the
    `<Class>ErrorEvent` pyclass is pushed to `class_names` and `register()` emits
    `m.add_class::<<Class>ErrorEvent>()?`.
  - The `closed` latch field is added to the stream struct **only** in event-mode,
    and the ctor dispatch initialises it with
    `closed: Arc::new(AtomicBool::new(false))` only when `op.stream_error.is_some()`
    (mirroring Node's `closed_init`).

## Harness

`tests/harness/async_iterable_contract.py` mocks the observable async-iterable
contract (plain `python3`, asyncio only) and asserts: `async for` consumes all
events in order and stops at `StopAsyncIteration`; early break / `aclose()`
triggers the core `close()` once; a throw-mode error source raises out of
`async for`. See `tests/harness/README.md` for pointing it at a real built module.
