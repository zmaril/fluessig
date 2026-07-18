# Node stream ops: async-iterable projection (§7 decide-and-document)

## Decision: GENERIC

Every `stream` op in the Node (napi) backend now projects a genuine JS
**async-iterable** — `for await (const ev of stream)` — generated for every
consumer, alongside the retained `next()` poll cursor. We do **not** route
streams through the `@manual` escape hatch.

### Why generic, not `@manual`

- **Async iterables are the idiomatic Node stream surface.** `for await` is what
  a Node developer reaches for; handing every stream op that surface for free is
  the ergonomic default consumers expect.
- **It composes with the existing type map.** The yielded item type is exactly
  `ty(api, &op.returns)` — the same mapping the poll cursor already uses. No new
  per-type machinery; unions still ride as their JSON envelope carrier, DTOs as
  their napi classes.
- **It matches pi's `AssistantMessageEventStream`.** pi already exposes its event
  streams as async-iterables; generating the same shape keeps fluessig-emitted
  bindings idiomatically aligned with the reference consumer.
- **Routing through `@manual` would push the idiom back onto every consumer** —
  re-hand-writing the same `[Symbol.asyncIterator]` wrapper per stream op, which
  is exactly the duplication the (language × shape) template grid exists to kill.

The poll cursor is retained (not replaced) as a feature-independent fallback —
see below.

## Mechanism

- **napi 3 native facility.** The stream class carries `#[napi(async_iterator)]`
  and a `#[napi] impl AsyncGenerator for <Class>` with `type Yield = <item>`,
  `type Next = ()`, `type Return = ()`. napi auto-generates the
  `[Symbol.asyncIterator](): AsyncGenerator<Yield, Return, Next>` entry in the
  `.d.ts`. This facility is **experimental** in napi 3 and feature-gated on
  napi's `tokio_rt` / async-runtime support, which is **default-on**.
- **`[Symbol.asyncIterator]()` returns a fresh generator object** (napi's
  `create_async_iterator` builds a new JS object with its own
  `next`/`return`/`throw`), *not* the class instance. So it does **not** collide
  with the class's own inherent `next()` method — the two are distinct JS
  surfaces (verified against napi-rs `async_iterator.rs` + the generator
  examples' `.d.ts`).
- **`spawn_blocking` keeps the event loop free.** The core primitive is a
  BLOCKING `PollStream::poll(&self, timeout) -> Poll<T>`. The generated
  `AsyncGenerator::next` clones the `Arc<dyn PollStream>` and drives the poll via
  `napi::tokio::task::spawn_blocking(...).await`, so the Node event loop is never
  blocked. The spawned future is `Send + 'static` and does not borrow `self`
  across the await (the `Arc` is cloned first).
  - **`napi::tokio` is a valid path.** napi re-exports tokio via
    `#[cfg(feature = "tokio_rt")] pub extern crate tokio;`, so
    `napi::tokio::task::spawn_blocking` resolves without the consumer crate
    adding a direct `tokio` dependency. It is gated on the same `tokio_rt`
    feature `#[napi(async_iterator)]` already requires, so there is no extra
    feature burden.
- **Backpressure by protocol.** napi drives one pull at a time (it does not call
  `next` again until the prior future resolves), so there is exactly one
  in-flight poll by construction — no unbounded buffering.
- **Cancellation.** When the consumer stops early (e.g. `break` in a for-await,
  which JS turns into the iterator's `return()`), napi invokes
  `AsyncGenerator::complete`, which calls `PollStream::close()` to release
  core-side resources. As a **backstop**, `impl Drop for <Class>` also calls
  `close()`, guaranteeing the core is closed even if the consumer neither
  exhausts nor cancels the iterator. `close()` is a documented default-no-op on
  the `PollStream` trait and MUST be idempotent, so poll-only cores need no
  change.
- **Retained poll cursor.** The class still exposes `next(): Promise<T | null>`
  (an `AsyncTask` over `Next<Class>Task`). This is the **feature-independent
  fallback** for consumers that cannot use async iteration or napi's `tokio_rt`
  feature — it uses only the always-available `AsyncTask` machinery.

## Terminal-event seam / gap 4

The single `match poll { Poll::Item(v) => yield, Poll::Idle => continue,
Poll::Closed => done }` inside `AsyncGenerator::next` is the seam where the
**dual error model** (gap 4) will slot in. That thread owns the change; this
projection only shapes the seam so its rebase is clean.

Under the dual error model, streams follow an **errors-as-events** contract while
unary/ctor ops keep **thrown** errors:

- gap 4 adds **one** terminal arm here — `Poll::Failed(msg)` — which builds the
  configured terminal error **event** and does `return Ok(Some(<error event>))`;
  the next pull then returns `Ok(None)` to complete. A mid-stream failure thus
  surfaces as a **yielded terminal error event followed by completion**, and the
  awaited `next()` **never rejects/throws**. This matches pi's contract
  (`vendor/pi packages/ai/src/types.ts:301-313`).
- The error event shape **defaults to pi's `{ type: "error", reason, error }`**
  and is **schema-configurable** via a `@streamError` annotation owned by the
  gap-4 thread.
- **Construction-time errors stay thrown.** Errors raised when the stream is
  opened (ctor/unary) remain thrown napi errors, not events.
- Rich typed error events that are part of the domain arrive as normal
  `Poll::Item` union values and need no new machinery here.
