# Specter-style navigators for fluessig

**Status:** Exploratory design draft. No implementation yet. This doc records a design discussion; it proposes a library and the fluessig features it would need, and is deliberately honest about what is a good fit and what is not.

Red Planet Labs' [specter](https://github.com/redplanetlabs/specter) is a Clojure library for querying and transforming deeply nested data by composing *navigators* into *paths*. The question this doc works through is "what would it mean to rewrite specter in fluessig, and is it a good idea?" — and the short answer is that a *faithful* port is a category error (specter's whole value is navigating arbitrary untyped data, which is exactly the thing fluessig exists to eliminate), but there is a genuinely good library hiding inside the question if you invert it: keep specter's navigator *algebra*, drop the assumption that data is always untyped, and let fluessig project one navigator engine across languages over both its typed catalog and dynamic host values, with the same query text able to run interpreted in a REPL or compile to specialized host code when it is known ahead of time. The rest of this doc explains that design, the prerequisites it puts on fluessig, and the costs.

## 1. What specter is (the parts that matter here)

A specter **navigator** is the whole abstraction; `select`, `transform`, `setval`, and the navigator vocabulary are all built on it. A navigator implements two methods that are really *a descent rule plus a rebuild rule*, each taking the rest of the path as a continuation:

```
(defprotocol RichNavigator
  (select*    [this vals structure next-fn])   ; read half: hand each subvalue to next-fn
  (transform* [this vals structure next-fn]))  ; write half: rebuild structure around next-fn's result
```

A navigator does not know what comes after it — it takes one step and calls `next-fn`. `keypath` fetches a key and hands the value on; its transform half reassembles the map (and a `NONE` sentinel returned by the continuation means "delete this key"). `ALL`, `FIRST`, `MAP-VALS`, `filterer`, `pred`, `srange`, `multi-path`, and the rest are all instances of this same two-method protocol — the vocabulary is open, not hardcoded. The headline feature is `recursive-path`, which binds a name usable inside its own definition, so one path can walk an arbitrarily nested tree (e.g. "every even leaf anywhere in this tree", gathered, reordered, and scattered back to source positions).

Specter is *fast* — rivaling hand-written code — because of macro-driven compilation, not despite dynamic typing. Three layers do it: (1) `comp-paths` composes a whole path into **one** navigator whose halves call each other directly through the continuation, so the inner loop has no per-step dispatch; (2) compiling a path is expensive, so specter **inline-caches** — a macro rewrites each call site to compile the *static* skeleton once, cache it, and thread only the dynamic parameters through on later calls; (3) transforms rebuild via mutable cells and preallocated arrays to avoid per-element allocation. The division is: macros run at expansion time to split static from dynamic and emit the cache; at run time you get a hashmap hit, a parameter array, and a tight continuation loop.

What is Clojure-specific and does **not** survive a naive rewrite: persistent structural sharing (transform rebuilds only the navigated path and shares the rest), dynamic typing (one path runs over shapes never declared), macros as the compilation mechanism, and homoiconic paths (a path *is* data). The speed machinery ports to Rust and codegen — arguably better, since Rust monomorphizes and can shed the runtime cache. The *generality over untyped data* is the part that resists a typed/codegen port.

## 2. What fluessig is (the constraint)

fluessig is a build-time, closed-world, **nominally-typed** schema compiler: Rust derives → a catalog (`catalog.json` / `api.json`) → committed native bindings for node, python, ruby, php, wasm, java, and C/C++, plus an MCP surface and ORM read-planes, over a single hand-written `core_impl` trait seam per consumer. Its op algebra is exactly four shapes (ctor / unary / stream / manual), async is one orthogonal bit, and its type algebra is `Scalar | Model | Enum | List | Nullable | Union`. There are **no generics, no type variables, and no dynamic/any value node**. The only openings for "unknown shape" are an opaque `Json` scalar (crosses as a string the boundary does not understand) and closed tagged unions. Everything is generated statically; nothing interprets a schema at runtime.

This is why a faithful port fails. specter's data is arbitrary and untyped; fluessig can only name it as the opaque `Json` string. A faithful "rewrite specter in fluessig" generates `select(jsonBlob, pathString) -> jsonBlob` — untyped, slow, and with every closure-valued navigator (`pred`, `view`, `filterer`, the transform fn) pushed into hand-written `manual` ops. fluessig would contribute almost nothing over a hand-rolled binding.

## 3. The reframe: two axes, one algebra

Don't port specter; lift its navigator algebra and let two axes vary independently:

- **typed vs untyped** — is the data a named catalog type, or an arbitrary dynamic host value?
- **interpreted vs compiled** — is the query built and run at runtime, or known ahead of time and compiled?

That gives a 2×2, and all four cells share **one navigator algebra and one semantic specification**:

| | interpreted (REPL) | compiled (known ahead) |
|---|---|---|
| **typed** (catalog) | build a path against catalog types, interpret (rare but coherent) | catalog-optics: generated typed lenses, paths checked at codegen — best guarantees + speed |
| **untyped** (dynamic) | REPL specter over dynamic host values | transpile the known query to native host code — specter's real speed, no static types |

The engine is written **once**, in Rust, generic over a `Navigable` trait:

```
trait Navigable {           // ~10 methods
    fn kind(&self) -> Kind;
    fn get(&self, key: &Key) -> Option<Self>;
    fn at(&self, i: usize) -> Option<Self>;
    fn keys(&self) -> ...;
    fn set(&self, key: &Key, v: Self) -> Self;
    // ...
}
```

Everything below is a matter of *which `Navigable` impl* the engine runs over and *when* the path is specialized.

## 4. The query is path-as-data

The single representation that makes all four cells work is the path expressed **as data** — a list of navigator descriptors — with one semantic spec both the interpreter and every compiler implement. In a dynamic language you write the query as a literal:

```
select([ALL, key("a"), pred(isEven)], data)
```

That array *is* the query. At runtime the interpreter walks it. Ahead of time, a compile step reads the same literal from source and emits specialized code. Same text, two consumers. The one thing a compiler cannot see into is a closure navigator like `pred(isEven)` — but it does not need to: when it compiles to the *same host language*, it emits `if (isEven(x))` inline, calling the user's function with **zero FFI**. Compiling to the host language is what keeps predicates and transforms boundary-free.

## 5. Dynamic data: foreign objects in place vs convert-to-JSON

For the untyped cells, the data must reach the engine as something it can walk. Two options:

**(a) Convert to `serde_json::Value`.** One `Navigable` impl (for `Value`), plus an `any` scalar that auto-marshals the host's native nested value to `Value` and back. Simple and uniform, but it deep-copies the input up front and reconstructs the output — O(n) each way — and loses host object identity.

**(b) Navigate the live host object in place.** Every dynamic-VM bindgen exposes the host's object model to Rust, so the engine can hold and walk the real value with no JSON round-trip: napi `JsObject::get/set/get_element/get_property_names` (persist across calls with `Ref`); PyO3 `Bound<PyAny>` `get_item/set_item/getattr` under `with_gil` (`Py<PyAny>` to persist); wasm `js_sys::Reflect::get/set` + `Array`; Magnus `RHash::aref/aset`, `RArray::entry`; ext-php-rs `Zval`/`ZendHashTable`; JNI reflective `call_method` (painful). **C/C++ has no native dynamic object model** and is the honest exception — it uses the typed-catalog path or a bring-your-own value type.

To keep "write the engine once," a JS object (`JsObject`+`Env`) and a Python object (`Bound<PyAny>`) are different Rust types, so each backend supplies a small `Navigable` adapter over its native reflection API — mechanical, and something fluessig can *generate* as runtime support rather than have a user hand-write.

Option (b) has three costs: every navigator step becomes an FFI call into the host VM (the opposite of specter's compiled-direct-call speed — "orchestration at host-access speed"); host values are `!Send` and thread-confined to the VM thread holding `Env`/GIL/GVL, so the op must run **synchronously on that thread** and cannot be the `Send + Sync` `Arc<Impl>` that offloads to a threadpool (it needs a sync, thread-confined op mode — the same shape as a `single_threaded` handle variant); and it has no meaning in C/C++. Its upside reverses one of specter's "casualties": because host objects are reference types, a `transform` that rebuilds only the navigated spine reuses sibling references for free from the host GC, and `select` returns the original element handles — so foreign-object-in-place **resurrects structural sharing and preserves identity**, which the `Value` copy path cannot. For the untyped tier, (b) is both zero-copy and more faithful to specter's semantics, modulo those three costs.

## 6. Staging: interpret, or compile when possible

The same query should run interpreted in a REPL and compile when it is statically visible. This is specter's own staging (`comp-paths`) generalized, and the "compile-when-you-can, else interpret" fallback is the correct shape — the same one regex libraries, format-string compilers, and specter's inline-caching already use. There are three compile targets, and they are not equal:

1. **Compile to Rust** (a proc-macro over the path literal): trivial and best-in-class — monomorphized, navigators inlined, no dispatch, better than specter's runtime cache. But host predicates still cross as callbacks, so this kills navigator overhead, not per-predicate FFI.
2. **Compile to native host source** (transpile the path to idiomatic JS/Python that walks the object with plain loops and `obj.a`): the ambitious target and the **only** mode that delivers specter's "rivals hand-written code" promise in a dynamic language, because there is no runtime boundary crossing at all — the generated code and the user's predicates are both in the host language.
3. **Interpret**: the generic engine over dynamic values, predicates as callbacks, paying FFI per step. This is the REPL "just works" mode.

Per-language feasibility of the compile step — which is genuinely a **separate, per-target codegen, not bindgen**: Rust proc-macro is easy; JS/TS is a build-time transform (Babel/SWC/bundler plugin) that fires only when the path is a literal (exactly the "when possible" caveat; dynamic paths fall through to interpret); Python and Ruby have no standard build-macro step, so they *runtime-compile* — generate a specialized function as source, `exec`/`define_method` it, and cache by path identity (the `re.compile` model), still landing no-FFI host-native navigation; wasm stays in-engine (compile to Rust → wasm). A bonus the interpreter cannot capture: a compiler can fuse (`[ALL LAST]` → `MAP-VALS`), hoist invariant subpaths, resolve field offsets in the typed tier, and rebuild without intermediate allocation. And because both modes derive from the same path data, the REPL gets a smooth on-ramp — experiment interpreted, then call `compile(path)` for the fast version without changing the query.

## 7. Callbacks: the load-bearing prerequisite

Half of specter needs no callbacks: the structural navigators (`keypath`, `ALL`, `MAP-VALS`, `recursive-path`) and `setval` are fully determined by the path. The other half — `pred`, `filterer`, `view`, and the transform fn — are host closures that **return values** (a bool, a new value). fluessig's in-flight callback support is forward-only, sync, and **void-returning**; the loader rejects non-void returns, and the value-returning variant is reserved under the name `Function` but unstarted. So the structural half can ship first; the predicate/transform half is gated on value-returning callbacks. This is the single most important prerequisite.

## 8. What fluessig would need to add

In rough dependency order:

1. **An `any` / dynamic value type** — either a scalar that auto-marshals host-native nested values to `serde_json::Value` (cheap, unblocks the convert-to-JSON untyped path), or first-class foreign-object-in-place navigation (richer; see §5). Not the existing `Json` (opaque text) or `Foreign` (opaque handle to a *named* host type).
2. **Value-returning callbacks** — the reserved `Function` shape (returns a value, ideally fallible). The hard blocker for `pred`/`view`/transform (§7).
3. **A sync, thread-confined op mode** — so an op can hold the host VM thread and navigate `!Send` host values (§5), instead of the `Send + Sync` threadpool-offload model.
4. **Per-backend `Navigable` adapters** — generated runtime support that implements the ~10-method trait over each language's reflection API.
5. **A per-language path compiler** — a separate codegen (Rust proc-macro, JS build plugin, Python/Ruby runtime-compiler) that turns a statically-known path into specialized host code (§6). This is the largest and most valuable single piece for dynamic-language performance.

The typed catalog-optics tier (§3, typed+compiled) needs the least: it can be built on today's fluessig for the structural navigator subset, with predicates joining once `Function` lands.

## 9. Costs and risks

- **Semantic drift.** N compilers + 1 interpreter must agree on every edge case — `NONE`-means-delete, `recursive-path` termination, collector ordering. This is the classic two-implementations-of-one-language tax and it is ongoing. Mitigation: make the interpreter the reference implementation, keep the navigator set **small and total**, and run one shared query corpus through every mode with golden-diffing (the harness pattern this ecosystem already uses). This is non-negotiable.
- **Per-step FFI** in the interpreted foreign-object mode (§5) — acceptable for `select` over a large document (no upfront copy), closer to a wash for whole-structure `transform`.
- **Thread confinement** (§5) breaks the async-offload model for the dynamic in-place path.
- **Per-language compiler cost** (§6) — Rust cheap, JS medium, Python/Ruby medium; each is a real project.

## 10. Suggested phasing (MVP first)

1. **Typed catalog optics, structural subset, on today's fluessig.** A path spec checked against the catalog at codegen (e.g. `Company::departments[ALL]::employees[ALL]::salary`), emitting typed `select_many(root) -> Vec<T>` and `transform(root, fn) -> Root` (generated clone-and-update over nominal structs). Structural navigators only (field, `ALL` over `List`, `MAP-VALS`, index, `multi-path`, `recursive-path` over self-referential entities and relations). Prove it on a real catalog with node + python emit cross-checked against a Rust golden.
2. **The Rust `Navigable` engine + interpreter** over `serde_json::Value`, with the whole navigator vocabulary. De-risk the untyped path today using the existing `Json` scalar (host stringifies in, parses out — ugly and slow, but it proves the vocabulary end-to-end before the nicer `any` + `Function` plumbing replaces it).
3. **Value-returning `Function` callbacks + the `any` type**, unlocking `pred`/`view`/transform in the interpreted untyped mode.
4. **Foreign-object-in-place navigation** (§5) for zero-copy, identity-preserving dynamic navigation, plus the sync thread-confined op mode.
5. **The per-language path compiler** (§6), starting with the Rust proc-macro (nearly free) and the JS build plugin (the highest-value dynamic target).

## 11. Verdict

specter's *speed machinery* ports; its *untyped generality* does not survive a typed rewrite unchanged — so stop trying to preserve the generality wholesale and instead spend the navigator algebra on the two things fluessig has that Clojure never did: a typed catalog you can check paths against at build time, and a codegen pipeline that can project one engine across languages. Keep the dynamic, untyped mode too — it is the better fit for dynamic languages and it is where the exploratory interest lives — but recognize it as gated on value-returning callbacks, an `any` type, and (for real speed) a per-language compiler, rather than as something today's fluessig can express. The design is coherent, has strong precedent, and degrades gracefully; the cost is concentrated in the callback/any prerequisites, a per-language compiler, and a conformance harness that keeps every mode honest.

## Appendix — relevant fluessig feature status (as of 2026-07-21)

- **Callbacks** (PR #78, draft): forward-only, sync, void-returning; non-void/async/fallible reserved but loader-rejected. Value-returning `Function` shape reserved, unstarted.
- **Foreign types** (PR #81, draft, stacked on the unmerged rust-core PR #76): `Foreign { name, rust_path }` — an opaque `u64` handle to a *named* host type; degrades to a string carrier in non-rust-core backends. Not an arbitrary navigable value.
- **`Json` scalar** (on main): opaque JSON *text*; host parses it itself. No `any`/dynamic value node exists.

(End of doc content.)
