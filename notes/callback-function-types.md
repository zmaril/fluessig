# Language-agnostic function/callback types in fluessig — design note

Status: proposed (open for review). Implementation of the node+python vertical slice is proceeding in parallel on this branch and can be redirected by review.

## Problem

fluessig lowers an interface's ops to N language backends from one op-layer IR (`src/api.rs`). That IR has **no way to spell a function-typed value**. Today the only host callback in the whole surface is one op, `watch` (`api.json`), marked `@manual` — its closure param is invisible to the IR and hand-written per binding (the doc string just names napi `ThreadsafeFunction` / Ruby GVL re-entry). Every callback-carrying op therefore falls out of generation entirely. This is the #1 gap blocking exact pi-API parity: pi's public orchestrator API is full of `(event) => void` callback params and register/unsubscribe handlers.

Goal: a callback param that any consumer language can supply — not just TS or Rust — from one IR shape that each backend lowers to its native callable, with the Rust core seeing **one uniform shape** regardless of the source language.

## Evidence: pi's actual callback surface

From a source-level enumeration of pi (not just the ApiReport), every function-typed surface in pi's orchestrator is **forward-only, synchronous, void-returning, single typed argument**. There are **no** async callback params, **no** value-returning callback params, and **no** reentrant call-and-wait (a callback whose return value the caller awaits mid-call). The surfaces:

- `onEvent(listener: (event: AgentSessionEvent) => void): () => void` — register a listener, return an unsubscribe function (`Set.add(listener)` / `() => set.delete(listener)`).
- `onExit(listener: (error?: Error) => void): () => void` — same register/unsubscribe shape.
- `setUiRequestHandler(handler?: (request: RpcExtensionUIRequest) => void): void` — one optional callback param, void return.
- `openRpcStream(instanceId, onEvent, onUiRequest): { handleRpc; handleUiResponse; close } | undefined` — 2 callback params → inline-object handle.
- `openRpcStream(instanceId, onResponse, onSessionEvent, onUiRequest): { handleRequest; close } | undefined` — 3 callback params → inline-object handle.

The one logically-duplex interaction (a UI request needing an answer) is implemented as **two forward-only halves**: the handler is invoked returning void, and the answer travels back through a separate forward-only op (`handleUiResponse`) with external correlation. So **no callback boundary needs the async-oneshot reentrant bridge** (pidgin `call_async`, `bridge_async.rs`). Forward-only fire-and-forget invocation is sufficient.

## Current IR (what we extend)

`src/api.rs`:
- `Shape` = `Ctor | Unary | Stream | Manual` (op-level dispatch key; serde lowercase).
- `ApiType` = `Scalar | Model | Enum | List | Nullable | Union` — `#[serde(untagged, deny_unknown_fields)]`, discriminated by which key is present; `Box<ApiType>` is the recursion idiom.
- `ApiParam` = `{ name, type: ApiType, optional? }`.
- House style for every added feature (async, infallible, result_error, bindings): additive optional field with `skip_serializing_if`, so an unmarked op stays **byte-identical** to before.

`src/bindgen/mod.rs`:
- `ty(api, &ApiType) -> (rust, ts)` (`:360`) is the single shared type-map chokepoint — a non-exhaustive match over `ApiType`. Adding a variant force-breaks compilation until every backend gains an arm. That is our safety net.
- `param_sig_with` (`:395`) is the shared param-spelling spine.
- `emit_core_traits_full` (`:449`) turns each op into a `<Interface>Core` trait method; its `match op.shape` (`:472`) shapes the Rust signature. `Shape::Manual` is skipped (no trait method).

## IR additions (two, additive)

### 1. `ApiType::Callback { params, returns }`

A function-typed value in type position (a param, or in principle a field/return). Untagged variant keyed on `"callback"`:

```rust
/// A host-supplied callback: `fn(params...) -> returns`.
/// Forward-only sync-void today; `is_async`/`fallible` reserved for later.
Callback {
    callback: CallbackSig,
},
```
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallbackSig {
    pub params: Vec<ApiType>,
    #[serde(default = "void_type", skip_serializing_if = "ApiType::is_void")]
    pub returns: Box<ApiType>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallible: bool,
}
```

For every pi surface, `returns` is `void`, `is_async` false, `fallible` false — so the reserved fields serialize to nothing and existing goldens are untouched. JSON: `{"type":{"callback":{"params":[{"model":"AgentSessionEvent"}]}}}`.

### 2. `Shape::Subscription`

A cousin of `Shape::Stream`: an op that takes one callback param and returns a **Subscription handle** (a small handle-class object with `unsubscribe()` and drop semantics) instead of returning a native closure. This is what `onEvent`/`onExit` become. Returning a Subscription handle avoids the hard direction (minting a native host closure from Rust) and maps exactly onto pi's `Set.add` / `() => set.delete` — dropping the handle removes the listener.

`Shape::Subscription` serializes as `"subscription"`; the op's `returns` is the Subscription handle model. Validation (in `load_api`): a `Subscription` op must have exactly one `Callback` param.

The two are orthogonal and compose: `openRpcStream` keeps `Shape::Unary` (or a future inline-object shape, owned by a sibling lane) and simply carries `Callback` params; `onEvent` uses `Shape::Subscription`.

## The language-agnostic dispatch contract

This is the core of the design: **the Rust core sees one uniform shape no matter which language supplied the callback.**

**Uniform core-side shape.** A `Callback { params: [A, B], returns: void }` lowers, on the generated core-trait side, to:
```rust
Box<dyn Fn(A, B) + Send + Sync + 'static>
```
The hand-written `core_impl` implements the trait and just invokes the boxed `Fn` whenever it needs to. It never knows or cares whether the closure came from JS, Python, Ruby, PHP, a browser, or Rust. Each backend's **generated binding code** is solely responsible for wrapping its native callable into that `Box<dyn Fn>` at the FFI boundary.

**Sync vs async invoke.** MVP is sync forward-only void — the boxed `Fn` returns `()`. The `is_async`/`fallible` fields are reserved but not emitted; a future value-returning or awaited callback is the only case that would need the async-oneshot bridge (`call_async`) semantics, and pi has none, so it is explicitly out of scope for this slice.

**Fallible invoke.** From the core's view the callback is infallible (returns `()`). If a host callback raises, that is handled at the language boundary inside the wrapper (logged/reported per backend's convention), never propagated into the Rust core. When `fallible` is later wired, the boxed shape becomes `Box<dyn Fn(A) -> Result<(), CallbackError>>`.

**Threading & the never-block invariant.** The core may invoke the callback from **any thread it runs on** (e.g. a poll-loop worker), so the boxed `Fn` is `Send + Sync`. Each backend's wrapper must deliver to the host **without blocking the host's runtime/main thread**:
- node: `ThreadsafeFunction<Args, ErrorStrategy::Fatal>` called `NonBlocking` — queues to the JS event loop (delivery is therefore deferred to the next loop turn, ordering preserved). Precedent: pidgin `crates/pidgin-napi/src/bridge_async.rs` / `agent_bridge.rs`.
- python: `Py<PyAny>` invoked under `Python::with_gil` — synchronous, inline.
- ruby: `Proc` + a `rb_thread_call_with_gvl` trampoline when invoked off a Ruby thread.
- java: a global-ref `Consumer<Args>` + `CallVoidMethod`, with `AttachCurrentThread` when off a JVM thread.
- wasm: `js_sys::Function` / `Closure` — single-threaded; the `Closure` must be **kept alive for the subscription lifetime**.
- cpp: `std::function<void(Args)>`.
- php: a `Zval` callable — PHP's single-thread request model **cannot** be invoked from a background thread. PHP therefore supports callback ops only under **synchronous, same-request-thread** invocation; a background-thread callback op is unsupported on PHP and must be marked as such rather than silently mis-lowered. This is the one backend where the forward-only-async contract genuinely fights the runtime, and the design surfaces it explicitly instead of pretending parity.

**Lifetime / drop.** For `Shape::Subscription`, the returned handle **owns** the registration and keeps the wrapped host callable alive. Calling `unsubscribe()` or dropping the handle removes the listener (and, on wasm, drops the `Closure`). For a bare `Callback` param on a non-subscription op, the wrapped callable lives for the duration of the call unless the core stores it (in which case the op should be a Subscription so the lifetime is explicit).

**How it appears in api.json / catalog.json.** `catalog.json` (enums only) is unaffected. In `api.json`, a callback param is `{"name":"listener","type":{"callback":{"params":[{"model":"AgentSessionEvent"}]}}}`; a subscription op adds `"shape":"subscription"`. Unmarked ops are byte-identical to today.

## Per-backend lowering summary

| backend | native callable | wrap → `Box<dyn Fn>` | non-block story | slice |
|---|---|---|---|---|
| node | `ThreadsafeFunction<_, Fatal>` | closure calls TSFN `NonBlocking` | event-loop queue | **this PR** |
| python | `Py<PyAny>` | `Python::with_gil(|py| f.call1(py, args))` | inline under GIL | **this PR** |
| cpp | `std::function<void(Args)>` | direct | inline | follow-up |
| wasm | `js_sys::Function` + `Closure` | keep `Closure` alive in handle | single-thread | follow-up |
| ruby | `Proc` | GVL trampoline | `rb_thread_call_with_gvl` | follow-up |
| java | `Consumer<Args>` global ref | `AttachCurrentThread` + `CallVoidMethod` | attach/detach | follow-up |
| php | `Zval` callable | sync-only | **unsupported off-thread** | follow-up (documented restriction) |

## Vertical slice (this PR)

A new demo crate `crates/callback-demo/` (mirroring `crates/cpp-demo` / `crates/java-demo`, the only existing real host→Rust round-trips) with a `Ticker` interface:
- `Shape::Ctor` `new() -> Ticker`
- `Shape::Subscription` `on_tick(listener: Callback<(int32)>) -> Subscription`
- `Shape::Unary` `tick(&self) -> void` — fires every live listener with an incrementing counter

Runnable proofs (the repo's **first** runnable node/python host consumers): a node script and a python script that subscribe a host closure, call `tick()`, assert the closure fired with the right values, then `unsubscribe()`/drop and assert it stops. Wired as `run.sh` + CI jobs mirroring the cpp/java demos.

## Deferred / follow-ups

- cpp, wasm, ruby, java, php lowering (one follow-up PR each).
- Inline-object handle return shape for `openRpcStream` (sibling lane — "inline-object minting").
- The hinzu converter's `=> void` parse branch (`fluessig_api.rs`) that emits `Callback` types from pi source (coordinated with the pi-API-gap session).
- `is_async` / `fallible` / value-returning callbacks (no pi surface needs them; would layer on the async-oneshot bridge).

## Open questions for review

1. Variant name: `ApiType::Callback` (used here, matches the pi-gap session's naming) vs `ApiType::Function` (type-theoretic). Easy to rename before merge.
2. `Shape::Subscription` as a first-class shape vs a plain `Callback` param on a `Unary` op that returns a hand-shaped handle. This note argues for the first-class shape (clean drop/unsubscribe lifetime, no returned-closure minting).
3. Whether PHP's off-thread restriction should be a hard generation error, a `@manual` fallback, or a documented sync-only emission.
