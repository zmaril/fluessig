# Class-handle return IR — design note (Feature 2)

Status: **design-first (draft).** No lowering is implemented on this branch — only
this note. Implementation follows once the shapes below are agreed with the hinzu
converter author.

## Problem

pi's `openRpcStream` op RETURNS an inline object of async methods —
`{ handleRpc(...): Promise<...>; handleUiResponse(...); close(): void } | undefined`
— i.e. a **handle**. Today fluessig's op-layer IR (`src/api.rs`) has no way to
spell "this op returns a typed handle to a generated interface", so such a return
degrades to the `Json` carrier (`ty()` in `src/bindgen/mod.rs:433` maps an
unrecognised scalar to `String`, and there is no lowering that mints a live handle
from an op return). The typed methods on that object vanish.

We want: **an op returning an object-of-methods becomes a typed handle reference
to a generated interface**, reusing the existing handle-class machinery that every
backend already generates for `Shape::Ctor` interfaces. The returned interface is
an ordinary interface (method ops, no public ctor); the op that returns it is a
*factory*. This is the same "factory-born interface" pi's `RpcProcessInstance`
already is under Feature 1.

### Relationship to Feature 1 (PR #92, `feat/factory-ctor-recognition`)

Feature 1 (`constructible_interfaces` / `returned_interface_name` in `src/api.rs`)
already **recognises** a factory-born interface: an interface returned by some op
anywhere — `{"model":"RpcProcessInstance"}`, unwrapped through transparent
`Nullable`/`List` — counts as *constructible*, so a `Shape::Subscription` op on it
passes `load_api`. But Feature 1 does **not lower the mint**: the factory op's
return still can't produce a live handle, and the subscription op on a ctor-less
interface therefore emits an honest skip-note instead of glue —
`subscription_factory_skip_note` in `src/bindgen/mod.rs`:

```
// subscription `{iface}.{op}`: factory-born (ctor-less) interface — a
// stateful handle minted from its factory op is not lowered yet; deferred.
```

used at `src/bindgen/node.rs:1368` and `src/bindgen/python.rs:1129`.

**Feature 2 is what closes that gap** — it lowers the factory op's return into a
real handle mint, which (a) makes the factory op return a typed handle instead of
`Json`, and (b) gives a factory-born interface a handle class, so its `&self`
methods (subscription included) have a receiver and the skip-note goes away.

Feature 2 therefore **builds on** Feature 1 and reuses its two helpers verbatim
(`constructible_interfaces`, `returned_interface_name`). Merge order: PR #92
first, then this.

## Part A findings — the existing machinery we reuse

### 1. How an interface is declared and referenced (`src/api.rs`)

- An interface is an `ApiInterface { name, doc?, single_threaded, ops }`
  (`src/api.rs:129-150`); each op is an `ApiOp` with a `shape: Shape`
  (`Ctor | Unary | Stream | Subscription | Manual`, `src/api.rs:251-269`), `params`,
  and `returns: ApiType`.
- A `Shape::Ctor` op is the interface's **public constructor**: in the core trait it
  is `fn {name}({ps}) -> anyhow::Result<Self>` (`src/bindgen/mod.rs:642`), and the
  backend's handle class calls it to build the core (see below).
- Types in op position are `ApiType` (`src/api.rs:283-327`), an untagged enum keyed
  on the present field: `Scalar`, `Model{model}`, `Enum{enum}`, `List{list}`,
  `Nullable{nullable}`, `Union{union}`, `Foreign{foreign}`, `Callback{callback}`.
- **Interfaces share the `Model{model}` namespace with DTOs — confirmed.** There is
  no `ApiType::Interface`. A reference to an interface *in type position* is spelled
  `{"model":"RpcProcessInstance"}`, exactly like a DTO reference. The loader/backends
  distinguish "Model naming an interface" from "Model naming a DTO" purely by
  membership in the interface-name set — precisely what Feature 1's
  `returned_interface_name` does (`src/api.rs`, PR #92):

  ```rust
  match ty {
      ApiType::Nullable { nullable } => returned_interface_name(nullable, ifaces),
      ApiType::List { list }         => returned_interface_name(list, ifaces),
      ApiType::Model { model } if ifaces.contains(model.as_str()) => Some(model.clone()),
      _ => None,
  }
  ```

### 2. How node/python lower a `Shape::Ctor` interface into a handle class

Both backends emit a handle struct holding `Arc<crate::core_impl::{Iface}Impl>` and
a ctor that builds that core by calling the trait's ctor method.

**node** (`src/bindgen/node.rs`):
```rust
// the core-holding field (node.rs:1342-1348)
#[napi]
pub struct RpcProcessInstance {
    pub(crate) core: Arc<crate::core_impl::RpcProcessInstanceImpl>,
}
// the ctor (node.rs:1195-1205)
#[napi(constructor)]
pub fn new(/*ctor params*/) -> Result<Self> {
    Ok(Self { core: Arc::new(
        <crate::core_impl::RpcProcessInstanceImpl as RpcProcessInstanceCore>::new(..)
            .map_err(err)?) })
}
```
A method that returns a value marshals through `self.core.{name}(..)`
(`node.rs:1246-1256`): infallible ⇒ `-> T` returning the value straight; fallible
⇒ `-> Result<T>` with `.map_err(err)`; async ⇒ an `AsyncTask` cloning
`self.core.clone()` into the worker (`node.rs:1270-1272`).

**python** (`src/bindgen/python.rs`):
```rust
// the core-holding field (python.rs:1106-1110)
#[pyclass]
pub struct RpcProcessInstance { pub(crate) core: Arc<crate::core_impl::RpcProcessInstanceImpl> }
// the ctor (python.rs:981-987)
#[new]
fn new(..) -> PyResult<Self> {
    Ok(Self { core: Arc::new(
        <crate::core_impl::RpcProcessInstanceImpl as RpcProcessInstanceCore>::new(..).map_err(err)?) })
}
```
Methods marshal via `self.core.{name}(..)` under `py.detach` (GIL release) or
inline when a callback param is present (`python.rs:1005-1045`).

The core-trait method's return type comes from `ret_ty(op) = ty(api,&op.returns).0`
(`src/bindgen/mod.rs:600, 709`). For `Model{model}`, `ty` returns the bare model
name (`src/bindgen/mod.rs:438`).

### 3. The crux — building a handle from an op RETURN (the gap)

A ctor builds `{Iface}Impl` from ctor args *inside the binding*
(`Arc::new(<Impl as Core>::new(..))`). A **returned** handle is different: the core
does the building and hands back an already-constructed core object; the binding
only **wraps** it into the handle class.

- **Core-trait signature (today, broken for this case).** The factory op
  `Orchestrator.open` returning `{"model":"RpcProcessInstance"}` lowers its core
  method to `fn open(&self, ..) -> anyhow::Result<RpcProcessInstance>`
  (`src/bindgen/mod.rs:683`) — but `RpcProcessInstance` is the generated *napi/pyo3
  handle class*, which the pure-Rust core cannot name or build. **This is the gap.**

- **Core-trait signature (needed).** For an op whose `returns` names an interface,
  the core method must return the **core object**, not the handle class:
  ```rust
  fn open(&self, instance_id: String)
      -> anyhow::Result<Arc<crate::core_impl::RpcProcessInstanceImpl>>;
  ```
  i.e. `Arc<crate::core_impl::{Iface}Impl>` — the exact type the handle class holds
  in its `core` field. (Nullable factory ⇒ `anyhow::Result<Option<Arc<..Impl>>>`.)
  We use the concrete `Arc<Impl>` rather than a boxed trait object
  `Box<dyn {Iface}Core>` because the handle field is already `Arc<{Iface}Impl>`, so
  the wrap is a zero-cost move and no `dyn` upcast is needed; the hand-written
  `core_impl` returns `Arc::new(RpcProcessInstanceImpl { .. })`.

- **Binding wrap (needed).** The factory op's method on the `Orchestrator` handle
  wraps the returned core into the target handle class instead of returning a value:
  ```rust
  // node — the factory op method on Orchestrator
  #[napi]
  pub fn open(&self, instance_id: String) -> Result<RpcProcessInstance> {
      Ok(RpcProcessInstance { core: self.core.open(instance_id).map_err(err)? })
  }
  // python
  fn open(&self, instance_id: String) -> PyResult<RpcProcessInstance> {
      Ok(RpcProcessInstance { core: self.core.open(instance_id).map_err(err)? })
  }
  ```
  This mirrors the `Shape::Stream` wrap that already exists
  (`node.rs:1290-1295` / `python.rs:1061-1067`) — build a class from
  `self.core.{name}(..).map_err(err)?` — the only difference being the wrapped
  thing is a handle core (`Arc<Impl>`) rather than a `PollStream`.

  **Second half of the gap:** a factory-born (ctor-less) interface today falls into
  the *stateless* branch (`node.rs:1356` `else`, gated on `has_ctor`) and emits free
  functions + skip-notes — it gets **no handle class**. Feature 2 must emit the
  handle class for any **constructible** interface (has_ctor **or** returned
  somewhere), not just `has_ctor`. That class carries the interface's methods but
  **no** `#[napi(constructor)]` / `#[new]` — it can only be minted by a factory.

## Part B — the design

### 1. api.json declaration (reuse `Model` + the interface set; no new IR field)

- **The returned interface** is an ordinary `ApiInterface` with method ops and **no
  `Shape::Ctor` op** (factory-born), identical to Feature 1's factory-born
  interfaces. Example:
  ```json
  {
    "name": "RpcProcessInstance",
    "ops": [
      { "name": "handleRpc", "shape": "unary", "async": true,
        "params": [ { "name": "req", "type": "Json" } ],
        "returns": "Json" },
      { "name": "close", "shape": "unary", "infallible": true,
        "params": [], "returns": "void" }
    ]
  }
  ```
- **The factory op** references it with the **same** `ApiType::Model` used
  everywhere — no new vocabulary:
  ```json
  {
    "name": "openRpcStream", "shape": "unary",
    "params": [ { "name": "instanceId", "type": "string" } ],
    "returns": { "nullable": { "model": "RpcProcessInstance" } }
  }
  ```
  (pi's `openRpcStream` returns `... | undefined`, hence the `nullable` wrapper; a
  non-optional factory is the bare `{"model":"RpcProcessInstance"}`.)

- **Decision: no new IR field is required.** Reusing `ApiType::Model{model}` plus
  the interface-name set (Feature 1's `returned_interface_name`, which already
  unwraps `Nullable`/`List`) is sufficient to distinguish "Model naming an
  interface" (⇒ mint a handle) from "Model naming a DTO" (⇒ pass the struct). This
  is additive and matches house style (the `Foreign`/`Callback`/const/union
  additions all avoided widening the type vocabulary where an existing key sufficed).
  The distinction is made at **lowering time** by set membership, not by a wire tag,
  so existing goldens with DTO-returning ops are byte-identical.

### 2. Loader validation (`load_api`)

Feature 2 adds no *new* structural rule beyond what Feature 1 already computes; it
**consumes** that recognition to drive lowering. Concretely:

- A factory op is any op whose `returns` yields `Some(name)` from
  `returned_interface_name` — i.e. it names a **declared** interface (membership in
  the interface set is the existence check; a `Model` naming an undeclared name is
  already an ordinary DTO reference and is not a factory).
- The returned interface's methods must be lowerable (they already are — they are
  ordinary interface ops that go through the same per-op validation loop).
- **Overlap with Feature 1:** a returned interface **is constructible by
  definition** (it is the very thing that puts it in `constructible_interfaces`), so
  Feature 1's subscription rule already accepts a `Shape::Subscription` on it. Once
  Feature 2 lowers the mint + emits the class, that subscription's skip-note is
  replaced by real glue.
- First-cut guard to keep the IR honest (recommend adding): if the factory op is
  itself `async` **or** `Shape::Stream`, emit a skip-note rather than lowering
  (async/stream *mint* is deferred — see §4). A synchronous unary factory (pi's
  shape) is the lowered path.

### 3. Per-backend lowering

Reuse the Ctor handle-class machinery; add a **construction path that wraps an
op-returned core object** instead of calling the ctor.

- **Core-trait ret_ty seam (shared).** `emit_core_traits_full`
  (`src/bindgen/mod.rs:610`) computes each op's return spelling via `ret_ty(op)`.
  Add a shared `pub(super)` helper — sketch:
  ```rust
  // src/bindgen/mod.rs
  /// The core-trait return spelling for an op: a factory op (returns names an
  /// interface) returns the CORE object `Arc<crate::core_impl::{Iface}Impl>`
  /// (Option-wrapped through a nullable factory), which the binding wraps into the
  /// handle class; every other op keeps its existing `ty()` spelling.
  pub(super) fn core_return_ty(api, op, base_ret: &str) -> String {
      match returned_interface_name(&op.returns, &iface_names(api)) {
          Some(iface) => wrap_like_returns(&op.returns,
              format!("Arc<crate::core_impl::{iface}Impl>")), // Option<..> if nullable
          None => base_ret.to_string(),
      }
  }
  ```
  Threaded into both `emit_core_traits_full` (node) and `emit_core_traits_with`
  (python/envelope) so both backends' core traits agree.

- **Binding wrap (shared expression builder).** Add a `pub(super)` helper (sibling
  of `subscription_method_parts`, `src/bindgen/mod.rs:383`) returning the
  `(return_type, wrap_expr)` a factory op's method splices in:
  `Ok({Iface} { core: {call}.map_err(err)? })` (fallible) — with an `Option`-map for
  the nullable factory (`.map(|c| {Iface} { core: c })`). node and python share it,
  differing only in `Result` vs `PyResult` and `map_err(err)` spelling, exactly as
  the subscription helper already parametrises.

- **Handle-class emission gate.** Change the class-emission guard from `has_ctor` to
  **constructible** (`has_ctor || returned_somewhere`). A constructible-but-ctor-less
  interface emits the class with its methods and **no** constructor. node
  `node.rs:1154`; python `python.rs:705`.

- **Handle methods on the returned interface come for free.** Its method ops
  (sync/async/stream/subscription) lower through the *existing* handle-class method
  arms — the returned handle is just a normal handle class. The **only** new code is
  the mint (the two seams above), not the methods.

- **node + python: full lowering.** Both hold the core as `Arc<{Iface}Impl>`
  already, so the wrap is a move; async methods on the returned handle work via the
  existing `AsyncTask` path (which clones `self.core`).

- **cpp / java / ruby / php / wasm: skip-note-eligible follow-ups.** Like the
  Subscription rollout, these defer. Each emits an honest marker in place of the
  factory op's mint and the returned interface's class — a `core_return_ty` /
  factory-mint skip-note analogous to `subscription_factory_skip_note`
  (`src/bindgen/mod.rs`). They then land one follow-up PR each (mirroring the
  callback/subscription per-backend series #87–#91).

### 4. Out of scope (explicit)

- **Callback param async/fallible stays rejected — do NOT widen `Callback`.** The ai
  package's async / value-returning **callback params** remain rejected by
  `load_api` (`src/api.rs:440-450`); `CallbackSig::is_async` / `fallible`
  (`src/api.rs:356-359`) stay reserved and unemitted. Feature 2 touches **returns**,
  never the callback-param contract, which is LOCKED per
  `notes/callback-function-types.md`.

- **Method async-ness vs callback async-ness — the distinction.** A returned handle
  METHOD being async (pi's `handleRpc(): Promise<...>`) is `op.is_async` on the
  *returned interface's own op* (`src/api.rs:167`) — the ordinary op-level async
  marker, lowered by the existing handle-method machinery (node `AsyncTask`/Promise;
  python plain method with GIL release). A **callback param** being async is
  `CallbackSig.is_async` on a `Callback` type — a completely separate axis, reserved
  and rejected. Feature 2 does not conflate them.

- **Async methods ON the returned handle: SUPPORTED in the first cut.** Because the
  returned handle is a normal handle class and node/python already lower async unary
  ops on handle classes, `handleRpc(): Promise` needs **no new code** beyond the
  mint — it rides the existing async-method arm. We therefore support async methods
  on the returned handle from day one (this is required for pi parity).

- **Async / stream FACTORY op (the mint itself) is DEFERRED.** The first cut lowers a
  **synchronous unary** factory op (pi's `openRpcStream` returns synchronously). An
  async factory (the mint wrapped in a `Promise`/coroutine) or a stream that yields
  handles is a follow-up; the loader emits a skip-note for it (see §2). This is a
  property of the *factory* op, orthogonal to the returned handle's method async-ness
  above.

### 5. Straitjacket (1500-line/file cap)

`src/bindgen/node.rs` is at **1484** lines (origin/main; ~1491 on the Feature 1
branch) — near the 1500 ceiling. The new logic must **not** be inlined per-backend:
hoist it into `src/bindgen/mod.rs` as `pub(super)` helpers (`core_return_ty`, the
factory-mint wrap builder, and any factory skip-note), reusing Feature 1's
`returned_interface_name` / `constructible_interfaces` rather than duplicating the
unwrap. Each backend's per-op arm then splices a short shared expression, mirroring
how `subscription_method_parts` keeps the register→unsubscribe lowering thin in both
`node.rs` and `python.rs`. python.rs (1215) has headroom; node.rs does not.

## Summary of decisions

| Question | Decision |
|---|---|
| New IR field for interface-returning op? | **No** — reuse `ApiType::Model{model}` + interface set (Feature 1's `returned_interface_name`). |
| Core-trait return type | `anyhow::Result<Arc<crate::core_impl::{Iface}Impl>>` (Option-wrapped if the factory is nullable). |
| Binding wrap | `Ok({Iface} { core: self.core.{op}(..).map_err(err)? })` — like the `Stream` wrap. |
| Handle class for factory-born interface | Emit it (gate on **constructible**, not `has_ctor`); **no** public ctor. |
| Async methods on the returned handle | **Supported first cut** (rides existing async handle-method machinery — required for pi's `handleRpc`). |
| Async/stream factory op (the mint) | **Deferred** — loader skip-note. |
| Callback param async/fallible | **Unchanged / still rejected** — `Callback` is NOT widened. |
| Backends fully lowered | **node + python**. cpp/java/ruby/php/wasm ⇒ skip-note follow-ups. |
| Depends on | Feature 1 (PR #92): reuses `constructible_interfaces` / `returned_interface_name`. |
