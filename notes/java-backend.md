# The Java (JNI) bindgen backend

fluessig's Java backend projects a catalog's op surface into **Java classes
backed by a Rust JNI shared library**. It is the Rust-side counterpart of the
node (napi), python (PyO3), ruby (Magnus), and php (ext-php-rs) backends: a
generator that emits the language FFI surface plus the `<Interface>Core` traits,
over which the consumer hand-writes ONE `crate::core_impl::{Iface}Impl`. The
source is `src/bindgen/java.rs`; the live proof is `crates/java-demo{,-schema}`
and `crates/java-demo/run.sh`.

## Substrate: JNI (the `jni` crate), not Panama/FFM

Java has two ways to reach native code today:

- **JNI** — the classic C-ABI bridge. Native functions are exported under
  mangled names (`Java_<pkg>_<Class>_<method>`) that the JVM resolves when a
  `native` method is called. The `jni` crate is the ergonomic Rust binding for
  the JNI `JNIEnv` API.
- **Panama / the Foreign Function & Memory API (`java.lang.foreign`)** — the
  modern downcall path, finalised in **JDK 22**.

We chose **JNI via the `jni` crate**, for the same reasons the other backends
picked napi/PyO3/Magnus/ext-php-rs over a raw C ABI:

1. **It is a Rust macro-framework seam, not a C-ABI flattening.** `jni` lets the
   Rust glue *construct Java objects directly* — `env.new_object("fluessig/Item",
   …)` builds a first-class `Item` DTO; `env.find_class` + `call_method` reads one
   back via its JavaBean getters. FFM would force every DTO, list, and stream
   item through a hand-rolled C struct / pointer-and-length flattening, which is
   exactly the marshalling misery the other four backends avoid.
2. **Synchronous by nature — it matches the sync-default op model.** The JNI seam
   is a plain blocking call, so it slots into fluessig's post-#69 sync-default
   world with no async runtime, exactly like php and ruby. Async is layered on
   the Java side (see below), not baked into the substrate.
3. **It runs on every JDK.** JNI is available on JDK 8→latest. FFM needs JDK 22+
   and (until recently) `--enable-preview`. Pinning the backend to JNI keeps the
   generated code loadable by essentially any JVM a consumer already runs.

The trade fluessig accepts: JNI symbol mangling and manual `JNIEnv` juggling in
the glue. Both are the generator's problem, emitted once per shape, never the
consumer's — so the cost lands where it is cheapest.

## The two generated artifacts

Unlike napi/PyO3/Magnus/ext-php-rs — whose macro frameworks expose the language
surface *from Rust at runtime* — JNI needs **two** artifacts, because the Java
type system lives in `.java` files the JVM compiles separately:

1. **Rust JNI glue** (`java_binding` → one `.rs` file). `#[no_mangle] pub extern
   "system" fn Java_fluessig_<Class>_<method>(...)` entry points, the plain Rust
   DTO/enum structs the core returns, the per-model `_to_j` / `_from_j`
   marshallers, and the `<Interface>Core` traits. It `use`s the `jni` crate;
   **fluessig itself never compiles it** (same as the php/napi surfaces — the
   consumer's binding crate adds `jni = "0.21"` and builds it into a `cdylib`).
2. **Java source classes** (`java_sources` → one `.java` per DTO / enum / union /
   interface / stream cursor). Package `fluessig`; every interface + stream
   class runs `System.loadLibrary("fluessig")` in a static initialiser, so the
   glue must compile to `libfluessig.so` / `fluessig.dll`.

The package is a single segment (`fluessig`) deliberately: JNI symbol mangling
turns `.` and `_` in names into `_1` escapes, and a one-segment package keeps the
`#[no_mangle]` names free of escapes so they line up with `javac -h` byte for
byte.

## Op-shape mappings

The whole backend is one (shape × Java) template grid. Every op crosses the JNI
seam as a **blocking** native call; the shapes differ only in what the Java class
wraps around that call.

| catalog shape | Java surface | Rust glue |
|---|---|---|
| ctor (`#[fluessig(ctor)]`) | `Store(long seed)` → `init`; `close()` → `free` | `init` leaks `Box<Arc<StoreImpl>>` as a `jlong`; `free` reclaims it |
| unary, sync + **infallible** (bare `T`) | `public T op(...)` calling the native directly | extern fn calls the core, marshals the value, **no throw seam** |
| unary, sync + **fallible** (`Result<T>`) | same signature; throws on error | `match core.op() { Ok => marshal, Err => throw + zero }` |
| unary, **async** (`#[fluessig(async)]`) | `CompletableFuture<T> op(...)` | identical blocking extern fn; async-ness is Java-side only |
| stream (`#[fluessig(stream)]`) | poll cursor: `Items` with `Optional<Item> next()` | open fn leaks `Box<Box<dyn PollStream<Item>>>`; `poll`/`free` |
| manual (`#[fluessig(manual)]`) | a `// @manual` comment | nothing — hand-written outside the surface |

Notes on the load-bearing choices:

- **Fallible → `RuntimeException`.** A core `Err` becomes `env.throw_new(
  "java/lang/RuntimeException", msg)` and the fn returns the JNI zero value; the
  JVM raises on return. An *unchecked* exception is used so the generated Java
  method signatures stay clean (no `throws` clause pollution). An **infallible**
  op (a bare-`T` core return, no `Result`) drops the throw seam entirely — the
  core method *is* the value.
- **Async → `CompletableFuture` via `supplyAsync`.** There is no Rust
  threadpool. An `#[fluessig(async)]` op routes through a private blocking
  `native<Pascal>` symbol, and the Java wrapper is
  `CompletableFuture.supplyAsync(() -> nativeOp(...))` (`runAsync` for `void`).
  This is the honest Java parallel to node's `Promise`: the concurrency lives in
  the JVM's `ForkJoinPool`, and a core failure still rides the same throw seam
  (it surfaces as the future completing exceptionally).
- **Stream → a poll cursor, not a Java `Iterator`.** `next()` returns
  `Optional<Item>`: a present value is an item, an **empty** Optional is the
  clean close (`Poll::Closed`), and a `Poll::Failed` throws. This mirrors the
  php/ruby poll cursors (and reuses the shared `fluessig_runtime::PollStream<T>`
  / `Poll<T>` contract) rather than the richer node async-iterator, because a
  clean close reads more naturally as an empty Optional than as `hasNext()`
  bookkeeping. `close()` frees the cursor and is idempotent.

## Type mapping

The Rust half of every signature is the shared `bindgen::ty(...)`; the
Java-visible spelling is `java_ty` in `java.rs`.

| catalog type | Java | JNI carrier |
|---|---|---|
| string / Json | `String` | `jstring` / `JString` |
| boolean / int32 / int64 / float64 | `boolean` / `int` / `long` / `double` | JNI primitives |
| bytes / **ArrowBatch** | `byte[]` | `jbyteArray` — raw IPC bytes, no Arrow-specific Java surface yet |
| void | `void` | `()` |
| model (`#[derive(Record)]` / entity) | first-class Java class | `jobject`, constructed directly via `new_object` and read via getters |
| list | `List<T>` (boxed element) | `java/util/ArrayList` |
| nullable | boxed wrapper (`Long`, …) or object; `null` for `None` | boxed / `jobject` |
| enum | `String` (wire token) + a generated `enum` class with `toWire`/`fromWire` | `jstring` |
| union | `String` (JSON envelope `{"kind","payload"}`) + a generated envelope class | `jstring` |

Two honest caveats, consistent with the other backends:

- **Enums cross as their wire `String`.** A standalone Java `enum {Name}` class
  IS emitted (with `toWire()` / `fromWire(String)`), but the marshalled op/DTO
  surface uses `String`. No fixture op/field uses an enum today, so this path is
  emitted-but-lightly-exercised.
- **Unions cross as their JSON envelope `String`** — the same `{"kind","payload"}`
  carrier every other backend's envelope mode uses. A `{Union}.java` envelope
  class (kind + payload getters) is emitted; a **structured/tagged union
  projection is not done for Java yet** (future work if wanted).
- **`ArrowBatch` is `byte[]`** (raw Arrow IPC bytes). There is no Arrow-native
  Java surface; a consumer decodes the bytes with its own Arrow library.

Models are the reason JNI was chosen: the glue's `Model_to_j` constructs the Java
object directly from the Rust value (recursing through nested models, lists, and
nullables), and `Model_from_j` reads it back via JavaBean getters — so a DTO is a
real Java class on both sides, not a flattened blob.

## Building & running the round-trip

`crates/java-demo/run.sh` is the end-to-end proof. It:

1. emits `catalog.json` + `api.json` from `java-demo-schema` (a compact real
   schema whose `Store` interface has one op of every shape);
2. **regenerates** `crates/java-demo/src/generated.rs` and
   `crates/java-demo/java/fluessig/*.java` with `fluessig-gen --java` (never
   hand-copied);
3. `cargo build -p java-demo` → the cdylib, staged as `libfluessig.so`;
4. `javac` the generated classes + `Main.java`;
5. `java -Djava.library.path=… -cp out Main`, asserting exact, order-sensitive
   output.

```sh
bash crates/java-demo/run.sh
# … PASS: sync + infallible + async + stream + throw all round-tripped.
```

`Main.java` calls `version()` (sync + infallible), `checked("abc")` (sync +
fallible, Ok path), `count("stream").get()` (async `CompletableFuture`), drains
the `items()` stream cursor, then `checked("boom")` to prove the `Err` →
`RuntimeException` throw seam.

### The demo crates and the default cargo set

`crates/java-demo` (the cdylib linking `jni`) and `crates/java-demo-schema` (the
schema emitter) are workspace **members** but are **excluded from
`default-members`** in the root `Cargo.toml` — exactly like entl-node /
disponent-node are excluded in their repos. So a bare `cargo build` / `cargo
test` / `cargo clippy --all-targets` never touches them (the engine's 182 tests
and clippy are unchanged); they build only via `-p java-demo` / the round-trip
runner. `cargo fmt --all` still covers them, so they stay fmt-clean.

`src/generated.rs` is committed (so `-p java-demo` builds standalone) and the
runner regenerates it in place, which also serves as a drift check: a stale
committed copy would differ from the freshly generated one.

## CI

A `java-roundtrip` job in `.github/workflows/ci.yml` runs the runner on a real
JDK (`actions/setup-java@v4`, temurin 21) plus the Rust toolchain. It is gated by
the `changes` paths-filter (`java` output) so it fires only when
`src/bindgen/java.rs`, the demo crates, or the harness change.

The leg is **additive and independent**: it does not rename or remove the
existing `rust` job, and it is **NOT** in the branch ruleset's required-checks
list, so it does not block merges. A maintainer who wants it gating can add
`java-roundtrip` to the ruleset's required checks — that is a governance action,
left to them.

## Caveats / future work

- Structured (tagged) union projection for Java is not implemented; unions ride
  as the JSON envelope `String`.
- `ArrowBatch` is exposed as raw `byte[]`; there is no Arrow-native Java plane.
- Enums are emitted as Java `enum` classes but marshalled as their wire `String`;
  no fixture exercises an enum-typed op/field yet.
- The round-trip is a `cargo build` + shell/`javac`/`java` script (like the
  other backends' hand-run harnesses under `tests/harness/`), not a `cargo test`
  — because it needs a JVM and a compiled cdylib, which the pure-Rust engine test
  suite deliberately has no toolchain for. The engine-level guarantees (the glue
  and `.java` are byte-stable and rustfmt-parseable) stay in `cargo test`
  (`tests/java_catalog.rs`, `crates/derive-demo/tests/api_gate.rs`).
