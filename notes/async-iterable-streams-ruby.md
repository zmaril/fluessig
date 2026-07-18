# Ruby (Magnus) stream ops: each/Enumerator projection + dual error model

Companion to [`async-iterable-streams.md`](./async-iterable-streams.md) (the Node
contract) and
[`async-iterable-streams-python.md`](./async-iterable-streams-python.md) (the PyO3
half). This documents the Ruby half (P2): every `stream` op now projects an
idiomatic Ruby `each` — `stream.each { |ev| ... }` — adapted from the merged Node
/ Python contract to Magnus, alongside the retained `.next` poll cursor.

## Surfaces

Each `stream` op's generated `#[magnus::wrap]` class carries **both** surfaces:

- **Primary — `each`.** `stream.each { |ev| ... }` yields each event to the
  block; `stream.each` with **no block** returns an `Enumerator` over `each`, so
  `.lazy` / `.map` / `.next` compose. `each` returns the receiver on completion
  (like `Array#each`).
- **Retained — `.next` poll cursor.** `stream.next` returns the next item or
  `nil` at end (fallible since P1). The explicit-pull fallback for consumers who
  do not want a block.

## Mechanism (Magnus 0.8.x)

- **`block_given` + `yield_value` + `enumeratorize`.** `each` is
  `fn each(ruby: &Ruby, rb_self: magnus::typed_data::Obj<Self>) -> Result<magnus::Value, Error>`,
  registered through ruby.rs's existing runtime `define_method` string mechanism
  (`s.define_method("each", method!(<Class>::each, 0))`). It uses
  `ruby.block_given()` to branch, `ruby.yield_value(v)?` to hand each event to
  the block, and `rb_self.enumeratorize("each", ())` to build the no-block
  `Enumerator`.
- **Why `Obj<Self>` as the receiver.** `enumeratorize` is a `ReprValue` method —
  it needs the Ruby *self value*, which the plain `&self` (unwrapped Rust ref)
  receiver used by `.next` does not carry. `magnus::typed_data::Obj<Self>` is
  both a Ruby value (`ReprValue`: `enumeratorize`, `as_value`) **and** derefs to
  `&Self` (so `rb_self.stream.poll(..)` reaches the wrapped field), and it is a
  valid `method!` receiver (`TryConvert`). The leading `&Ruby` parameter is the
  standard Magnus convention (arity still `0`).
- **Item type is `ruby_ty`-resolved.** The yielded item type comes from
  `ruby_ty(api, opts, &op.returns)`, NOT the shared `ty()`, so a union-returning
  stream yields the structured `{Union}Union` carrier correctly — it tracks the
  structured-union state the rest of the Ruby backend uses.
- **`Box`, not `Arc`.** The poll runs on the calling Ruby thread (block-under-GVL
  — see below), so the handle is never moved cross-thread; the field stays
  `stream: Box<dyn PollStream<item>>` (unchanged from P1). No `Send`/`Arc`.
- **Ruby ≥ 3.1 caveat.** An `Enumerator` built from a **yielding** method (what
  `enumeratorize` produces here) is backed by a `Fiber`; Magnus's docs note that
  on Ruby **< 3.1** such an enumerator is non-functional. So `.lazy`/`.map`/
  `.next` off the no-block `each` require **Ruby ≥ 3.1**. The block form
  (`each { ... }`) and the retained `.next` cursor have no such floor.

## The GVL — block-under-GVL (with a documented follow-up)

The blocking `PollStream::poll` runs on the calling Ruby thread. **Ideally** the
GVL is released around it so a stream that idles does not stall the whole Ruby
VM. **We do NOT release the GVL** in P2: `each` (like the retained `.next`
cursor) calls `self.stream.poll(Duration::from_millis(500))` **under the GVL**.

- **Why not release it here.** Magnus has **no safe wrapper** for
  `rb_thread_call_without_gvl`; releasing requires raw FFI via `magnus::rb_sys`
  → `rb_thread_call_without_gvl`, with the strict rule that **no Ruby object may
  be touched while the GVL is released** (extract everything, poll released,
  re-acquire, *then* `yield_value`). fluessig emits Rust **source strings** and
  never compiles them, so a subtly-wrong `unsafe` FFI trampoline could not be
  caught in CI — and emitting incorrect `unsafe` is worse than not releasing.
  Block-under-GVL is **correct** (it only holds the GVL during the bounded 500 ms
  poll), just not maximally concurrent. This matches the pre-existing `.next`
  cursor exactly, so P2 introduces no new soundness surface.
- **Follow-up.** GVL-release via `rb_thread_call_without_gvl` (extract handle →
  poll released → re-acquire → `yield_value`, and revisit whether the closure
  needs `Arc`/`Send`) is a **documented optimization follow-up**, gated on
  pinning a known-correct `magnus::rb_sys` signature with high confidence. It
  must not compromise the soundness of the `each`/Enumerator surface, which is
  the actual P2 deliverable.

## Cancellation — `close()` on `Drop`

The generated class emits `impl Drop for <Class> { fn drop(&mut self) {
self.stream.close(); } }`, in **both** error modes. An early `break` out of the
consumer's block leaves the stream un-exhausted; `Drop` is the backstop that
still releases the core (Ruby has no deterministic destructor, so — as with
PyO3's `Drop`-only seam — `close()` runs when the wrapped object is
garbage-collected, a genuinely weaker guarantee than Node's `complete()` + `Drop`
pair). `close()` is a documented idempotent default-no-op on the `PollStream`
trait, so poll-only cores need no change.

## Dual error model — throw vs. event terminal split

The error model is chosen per-op by `op.stream_error`, mirroring Node / Python
exactly (`Poll::Failed(String)` is the core→binding channel in BOTH modes; only
the `each`-loop terminal arm differs):

- **`None` (unannotated) — DEFAULT throw-mode.** The `each` loop maps
  `Poll::Failed(e) => return Err(rberr(e))`, which **raises** a Ruby
  `RuntimeError` out of `each`. Safe by default, no silent-swallow.
- **`Some(shape)` (`@streamError`) — opt-in error-AS-EVENT.** The `each` loop maps
  `Poll::Failed(e)` to `yield_value(<Class>ErrorEvent { .. })` then `break` — it
  hands the failure out **as the terminal event value** and ends the block; it
  **NEVER raises**. Because `each` consumes the whole stream in one call (unlike
  Node/Python's re-entrant `next()`/`__anext__`), no `closed` latch is needed —
  the `break` is the terminal.
  - The `<Class>ErrorEvent` is a `#[magnus::wrap]` class with three `String`
    fields (`type_`, `reason`, `error`) and `get_`-prefixed getters, mirroring
    ruby.rs's existing output-model wrap-class idiom. On construction the tag
    field = `se.tag_value`, `reason` = `"error"`, `error` = the `e` from
    `Poll::Failed(e)` — matching Node's / Python's field values.
  - **Registration is explicit.** Magnus has no auto-registration, so the class
    and its getters are registered via `define_class` + `define_method`, under
    the schema names `se.tag_name` / `se.reason_name` / `se.error_name` (e.g.
    `ev.define_method("type", method!(<Class>ErrorEvent::get_type_, 0))`).

### Note: the retained `.next` cursor stays throw-only

Per the P2 scope, the retained `.next` poll cursor keeps its P1 throw-only arm
(`Poll::Failed(e) => return Err(rberr(e))`) in **both** error modes — the
dual-error split lives on the idiomatic `each` surface. `.next` is the low-level
explicit-pull cursor; `each` is the surface that honours `@streamError`. (Node's
retained `next()` Task and Python's sync `__next__` do fold the error event into
the retained cursor; Ruby scopes it to `each` to keep `.next` the simple,
always-raising primitive it was in P1.)

## Harness

`tests/harness/each_enumerator_contract.rb` mocks the observable `each`/
Enumerator contract (plain `ruby`, stdlib only) and asserts: a block consumes all
events in order (idle skipped, stops at `Poll::Closed`); a no-block `each` returns
an `Enumerator` and `.next`/`.lazy.map` compose off it; an early `break` triggers
the core `close()` once (the `Drop`-backstop analogue, modelled with `ensure`); a
throw-mode failure raises out of `each`; an event-mode failure is yielded as a
terminal `{ type:, reason:, error: }` event and the block then ends without
raising. See `tests/harness/README.md` for pointing it at a real built extension.
