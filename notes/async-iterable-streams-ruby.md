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
- **`Box`, not `Arc`.** The poll runs on the calling Ruby thread — and the
  GVL-release trampoline runs on that **same OS thread** (see below), so the
  handle is never moved cross-thread; the field stays
  `stream: Box<dyn PollStream<item>>` (unchanged from P1). No `Send`/`Arc`.
- **Ruby ≥ 3.1 caveat.** An `Enumerator` built from a **yielding** method (what
  `enumeratorize` produces here) is backed by a `Fiber`; Magnus's docs note that
  on Ruby **< 3.1** such an enumerator is non-functional. So `.lazy`/`.map`/
  `.next` off the no-block `each` require **Ruby ≥ 3.1**. The block form
  (`each { ... }`) and the retained `.next` cursor have no such floor.

## The GVL — IMPLEMENTED: released around every blocking poll

The blocking `PollStream::poll` runs on the calling Ruby thread. Both `each` and
the retained `.next` cursor now **release the GVL** around that poll via
`rb_sys::rb_thread_call_without_gvl`, so a stream that idles or blocks no longer
stalls the whole Ruby VM — other Ruby threads run during the poll. (Was
block-under-GVL through P2; this is the P2 follow-up, now done.)

**Verified against real ruby 3.3.6.** The exact FFI trampoline below was
prototyped in a real magnus 0.8.2 + rb-sys 0.9.128 extension and exercised: a
background Ruby thread demonstrably advanced ~600 increments during a blocking
`each` while the GVL was released, vs **0** when the poll was held under the GVL.

- **The `without_gvl` helper.** Emitted once into the binding prelude (only when
  the API projects a stream), alongside `use std::ffi::c_void;` and
  `use std::ptr;`:

  ```rust
  fn without_gvl<F, R>(func: F) -> R
  where
      F: FnOnce() -> R, // no Send bound: runs on the same OS thread
  {
      unsafe extern "C" fn trampoline<F, R>(data: *mut c_void) -> *mut c_void
      where
          F: FnOnce() -> R,
      {
          let slot = &mut *(data as *mut Option<F>);
          let f = slot.take().expect("gvl closure already consumed");
          Box::into_raw(Box::new(f())) as *mut c_void
      }
      let mut slot: Option<F> = Some(func);
      let result_ptr = unsafe {
          rb_sys::rb_thread_call_without_gvl(
              Some(trampoline::<F, R>),
              &mut slot as *mut Option<F> as *mut c_void,
              None,             // ubf = None
              ptr::null_mut(),  // data2 = null (a timeout-bounded poll needs no unblock fn)
          )
      };
      *unsafe { Box::from_raw(result_ptr as *mut R) }
  }
  ```

  Each poll site becomes
  `let poll = without_gvl(|| self.stream.poll(Duration::from_millis(500)));`
  and the `match` acts on the returned `Poll<item>`.

- **`rb-sys` is a CONSUMER dependency.** `rb_thread_call_without_gvl` lives at the
  **top level** of the `rb-sys` crate; **magnus does NOT re-export it**. So the
  **consumer's** Ruby extension crate must depend on `rb-sys` (`~0.9`) **directly**,
  in addition to `magnus`. fluessig only emits Rust **source strings** — it does
  **not** compile them — so this dependency is **not** added to fluessig's
  `Cargo.toml`; the consumer wires it explicitly. (This mirrors how the Python
  half documents its `pyo3-async-runtimes` consumer requirement in
  [`async-iterable-streams-python.md`](./async-iterable-streams-python.md).)

- **No Ruby objects while released — the C-API invariant.** Ruby's C API forbids
  touching any Ruby object (no `Value`, no alloc) while the GVL is released. The
  codegen honours this structurally: the `without_gvl` closure captures **only**
  `&self` / `&rb_self` (Rust state) and returns the `Poll<item>` **by value** —
  and `item` is a pure-Rust value (the core's yielded type; conversion to a Ruby
  `Value` happens later at `yield_value`). Every Ruby-touching action —
  `yield_value`, constructing/yielding the `<Class>ErrorEvent`, raising via
  `rberr`/`Error` — runs **after** `without_gvl` returns, i.e. once the GVL is
  re-acquired, on the matched `Poll` arm.

- **Same OS thread ⇒ no `Send`/`Arc`.** `rb_thread_call_without_gvl` runs the
  trampoline on the **same** OS thread (it only drops/re-takes the GVL), so the
  closure needs **no** `Send`/`Sync` bound and the stream field stays
  `Box<dyn PollStream<item>>` (no `Arc`). The `Option<F>` slot + `Box`-in/`Box`-out
  is the standard thunk pattern for passing a non-`extern "C"` closure and its
  return value through the C boundary.

- **Dual-error semantics unchanged.** Only the poll **call site** changed; the
  throw-mode raise and event-mode terminal-`ErrorEvent` yield (below) are exactly
  as in P2, and both remain outside the released region.

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
