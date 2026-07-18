//! The napi (Node) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, Shape};

use super::*;

/// Generate the napi (Node) binding: DTO structs, enums, core traits, per-op
/// AsyncTasks, stream classes, free functions, and the handle class.
pub fn node_binding(
    api: &ApiDoc,
    enums: &[(String, Vec<String>)],
    banner_note: Option<&str>,
) -> String {
    let mut t: rust::Tokens = quote! {
        $("// The fixed prelude — generated code uses fully-qualified paths elsewhere.")
        use std::future::Future;
        use std::sync::Arc;
        use std::time::Duration;
        use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Result};
        use napi::{Env, Task};
        use napi_derive::napi;

        fn err(e: impl std::fmt::Display) -> napi::Error {
            napi::Error::from_reason(e.to_string())
        }

        $("/// One poll result from a core stream (the sync primitive every stream shape dresses).")
        $("/// `Failed(msg)` is the SECOND error model. Once a stream has started, pi's")
        $("/// contract flips: a request/model/runtime failure is no longer thrown — it is")
        $("/// ENCODED IN THE STREAM as a terminal error EVENT and the stream then completes")
        $("/// (packages/ai/src/types.ts: after `stream()` returns, failures ride the stream,")
        $("/// never reject the promise). `Failed` is the generic path for a core that surfaces")
        $("/// a mid-stream failure as a Rust `Result`/error; a core that instead emits its")
        $("/// terminal error as a normal union VARIANT of the element type flows through")
        $("/// `Item` unchanged — both satisfy \"never throw after stream start\". The message")
        $("/// is owned (`String`) so the enum stays trivially `Send` and dependency-free.")
        pub enum Poll<T> {
            Item(T),
            Idle,
            Closed,
            Failed(String),
        }

        $("/// The one sync primitive: a blocking, timeout-bounded poll.")
        pub trait PollStream<T>: Send + Sync {
            fn poll(&self, timeout: Duration) -> Poll<T>;
            $("/// Release core-side resources. Called on async-iterator cancellation")
            $("/// (`return()`), on completion, and on drop. Must be idempotent; the")
            $("/// default is a no-op so poll-only cores need no change.")
            fn close(&self) {}
        }
    };
    if api_uses_bytes(api) {
        quote_in! { t =>
            $['\n']
            $("/// Bulk bytes cross into JS as a Buffer (Arrow IPC payloads and friends).")
            pub type Bytes = napi::bindgen_prelude::Buffer;
        };
    }
    t.line();

    // ── enums (name-only variants → napi string enums; wire-valued → strings) ──
    // A name-only enum lowers to a napi *string* enum whose variants carry an
    // explicit snake_case wire token (`#[napi(value = "…")]`), so JS sees
    // `CapabilityKind.Dispatch === "dispatch"` — the same tokens the ruby
    // emitter hands out via `wire()`, not the magic discriminant number a plain
    // `#[napi]` enum would emit. The Rust variant idents are unchanged, so a
    // consumer's core_impl keeps constructing `CapabilityKind::Dispatch`.
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        // each line: `#[napi(value = "<wire token>")] <PascalVariant>,` — the
        // token is the catalog member lowercased, identical to ruby's `wire()`.
        let vs: Vec<String> = variants
            .iter()
            .map(|v| format!("#[napi(value = {:?})] {},", v.to_lowercase(), pascal(v)))
            .collect();
        // napi 3 no longer auto-derives Clone/Copy on #[napi] enums; option
        // structs that carry one derive Clone, so the enum must too.
        quote_in! { t =>
            $['\n']
            #[napi(string_enum)]
            #[derive(Clone, Copy)]
            pub enum $name {
                $(for v in &vs join ($['\r']) => $v)
            }
        };
    }
    t.line();

    // ── DTO structs ──
    for m in &api.models {
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                quote_in! { t => $['\r']$(format!("/// {line}")) };
            }
        }
        if let Some(af) = arrow_field(m) {
            // Arrow-payload DTO: a class holding the RecordBatch, getters for the
            // scalar envelope, and a lazy IPC getter — no encode until accessed.
            let plain: Vec<&crate::api::ApiField> =
                m.fields.iter().filter(|f| f.name != af.name).collect();
            let storage: Vec<rust::Tokens> = plain
                .iter()
                .map(|f| {
                    let (r, _) = ty(api, &f.ty);
                    let n = snake(&f.name);
                    quote!(pub(crate) $n: $r,)
                })
                .collect();
            let getters: Vec<rust::Tokens> = plain
                .iter()
                .map(|f| {
                    let (r, _) = ty(api, &f.ty);
                    let n = snake(&f.name);
                    quote! {
                        #[napi(getter)]
                        pub fn $(&n)(&self) -> $r {
                            self.$(&n).clone()
                        }
                    }
                })
                .collect();
            let ipc = snake(&af.name);
            quote_in! { t =>
                $['\r']
                #[napi]
                #[derive(Clone)]
                pub struct $(&m.name) {
                    $(for f in &storage join ($['\r']) => $f)
                    $("// the rows, still columnar — encoded only when the getter is hit")
                    pub(crate) batch: entl_core::RecordBatch,
                }
                #[napi]
                impl $(&m.name) {
                    $(for g in &getters join ($['\r']) => $g)
                    $("/// The rows as one Arrow IPC stream — decode with `tableFromIPC` (apache-arrow).")
                    #[napi(getter, ts_return_type = "Buffer")]
                    pub fn $(&ipc)(&self) -> Result<Bytes> {
                        Ok(entl_core::batch_ipc(&self.batch).map_err(err)?.into())
                    }
                }
                $['\n']
            };
            continue;
        }
        let fields: Vec<rust::Tokens> = m
            .fields
            .iter()
            .map(|f| {
                let (r, _) = ty(api, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                let n = snake(&f.name);
                quote!(pub $n: $r,)
            })
            .collect();
        quote_in! { t =>
            $['\r']
            #[napi(object)]
            #[derive(Clone)]
            pub struct $(&m.name) {
                $(for f in &fields join ($['\r']) => $f)
            }
            $['\n']
        };
    }

    emit_core_traits(&mut t, api);

    // ── per-interface surface ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream classes + next-tasks. The error model is chosen per-op by
        // `stream_error`: `None` (unannotated) = the DEFAULT idiomatic native-TS
        // REJECT (a mid-stream `Poll::Failed` maps to `Err(err(e))`, so the awaited
        // pull rejects and `for await` throws — safe by default, no silent-swallow);
        // `Some(shape)` = opt-in error-AS-EVENT (mirror-a-library mode, e.g. pi's
        // `{ type, reason, error }`), where the failure is yielded as a terminal
        // event and the stream then completes (never rejects). `Poll::Failed(String)`
        // is the core→binding channel in BOTH modes; only the mapping differs.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, ts_item) = ty(api, &op.returns);
            match &op.stream_error {
                // ── DEFAULT throw-mode (unannotated): native-TS reject ──
                None => {
                    let ret_ts = format!("Promise<{ts_item} | null>");
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// Event stream from `{}.{}`.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: a JS async-iterable — `for await (const ev of stream)`.")
                        $("/// Retained surface: `next()` poll cursor (resolves `null` at end) for")
                        $("/// consumers that cannot use async iteration or napi's `tokio_rt` feature.")
                        $("///")
                        $("/// DEFAULT error model = idiomatic native-TS REJECT: a mid-stream core")
                        $("/// failure (`Poll::Failed`) maps to `Err(err(e))`, so the awaited pull")
                        $("/// REJECTS and the `for await` loop THROWS — safe by default, never a")
                        $("/// silent-swallow. Annotate the op `@streamError` to opt into the")
                        $("/// error-AS-EVENT model instead (mirror a source library like pi).")
                        #[napi(async_iterator)]
                        pub struct $(&class) {
                            stream: Arc<dyn PollStream<$(&item)>>,
                        }

                        $("// Async-iterable surface (Symbol.asyncIterator). napi drives one pull at a")
                        $("// time, so backpressure is one in-flight poll by construction.")
                        #[napi]
                        impl AsyncGenerator for $(&class) {
                            type Yield = $(&item);
                            type Next = ();
                            type Return = ();

                            fn next(
                                &mut self,
                                _value: Option<Self::Next>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                let stream = self.stream.clone();
                                async move {
                                    loop {
                                        let s = stream.clone();
                                        $("// Drive the blocking poll off the async runtime so the Node")
                                        $("// event loop is never blocked.")
                                        let poll = napi::tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        $("// DEFAULT throw-mode: a mid-stream failure REJECTS the pull")
                                        $("// (native TS — the `for await` loop throws). Opt into")
                                        $("// error-as-event with `@streamError`.")
                                        match poll {
                                            Poll::Item(v) => return Ok(Some(v)),
                                            Poll::Idle => continue,
                                            Poll::Closed => return Ok(None),
                                            Poll::Failed(e) => return Err(err(e)),
                                        }
                                    }
                                }
                            }

                            fn complete(
                                &mut self,
                                _value: Option<Self::Return>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                $("// Cancellation: consumer called `return()` (e.g. `break` in for-await).")
                                let stream = self.stream.clone();
                                async move {
                                    stream.close();
                                    Ok(None)
                                }
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }

                        $("// Retained poll cursor: `next(): Promise<Item | null>`.")
                        pub struct Next$(&class)Task {
                            stream: Arc<dyn PollStream<$(&item)>>,
                        }
                        impl Task for Next$(&class)Task {
                            type Output = Option<$(&item)>;
                            type JsValue = Option<$(&item)>;
                            fn compute(&mut self) -> Result<Self::Output> {
                                loop {
                                    match self.stream.poll(Duration::from_millis(500)) {
                                        Poll::Item(v) => return Ok(Some(v)),
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None),
                                        $("// throw-mode: reject the pull (native TS).")
                                        Poll::Failed(e) => return Err(err(e)),
                                    }
                                }
                            }
                            fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                                Ok(o)
                            }
                        }
                        #[napi]
                        impl $(&class) {
                            #[napi(ts_return_type = $(quoted(ret_ts)))]
                            pub fn next(&self) -> AsyncTask<Next$(&class)Task> {
                                AsyncTask::new(Next$(&class)Task { stream: self.stream.clone() })
                            }
                        }
                        $['\n']
                    };
                }
                // ── OPT-IN event-mode (@streamError): error-as-event (mirror a library) ──
                Some(se) => {
                    let err_evt = format!("{class}ErrorEvent");
                    // each field: a js_name attr only when the js-name diverges from the
                    // rust ident (the tag always needs one — `type_` never equals its
                    // js-name), mirroring the `{:?}` string-literal idiom above.
                    let ev_field = |rust: &str, js: &str| {
                        if js == rust {
                            format!("pub {rust}: String,")
                        } else {
                            format!("#[napi(js_name = {js:?})] pub {rust}: String,")
                        }
                    };
                    let ev_fields: Vec<String> = vec![
                        ev_field("type_", &se.tag_name),
                        ev_field("reason", &se.reason_name),
                        ev_field("error", &se.error_name),
                    ];
                    let ret_ts = format!("Promise<{ts_item} | {err_evt} | null>");
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// The terminal error event yielded (NEVER thrown) when `{}.{}`'s core stream", i.name, op.name))
                        $("/// fails after it has started — the opt-in `@streamError` (error-as-event)")
                        $("/// model, mirroring a source library's contract (pi's post-start boundary as")
                        $("/// a plain value). NOTE: a core that instead surfaces its terminal error as a")
                        $("/// normal union VARIANT of the element type already rides out through")
                        $("/// `Poll::Item`; this struct is only the carrier for a `Result`/error failure.")
                        #[napi(object)]
                        pub struct $(&err_evt) {
                            $(for f in &ev_fields join ($['\r']) => $f)
                        }
                        $(format!("/// Event stream from `{}.{}`.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: a JS async-iterable — `for await (const ev of stream)`.")
                        $("/// Retained surface: `next()` poll cursor (resolves `null` at end) for")
                        $("/// consumers that cannot use async iteration or napi's `tokio_rt` feature.")
                        $("///")
                        $("/// `@streamError` error model = error-AS-EVENT: a mid-stream core failure is")
                        $("/// yielded as a terminal `<Op>ErrorEvent` and the stream then completes —")
                        $("/// it NEVER rejects/throws (mirrors pi's contract, packages/ai/src/types.ts).")
                        #[napi(async_iterator)]
                        pub struct $(&class) {
                            stream: Arc<dyn PollStream<$(&item)>>,
                            $("// latched once the terminal error event is handed out — a started stream")
                            $("// never restarts, so every subsequent next() must resolve null (done).")
                            closed: Arc<std::sync::atomic::AtomicBool>,
                        }

                        $("// Async-iterable surface (Symbol.asyncIterator). napi drives one pull at a")
                        $("// time, so backpressure is one in-flight poll by construction.")
                        #[napi]
                        impl AsyncGenerator for $(&class) {
                            $("// Yield WIDENED to Either<item, error-event> — event-mode only.")
                            $("// WHY the Either: the terminal error event is a distinct TOP-LEVEL shape")
                            $("// `{ type, reason, error }` whose keys differ from the element/union")
                            $("// carrier, so it cannot ride the plain `item` Yield — it must be a second")
                            $("// arm. napi renders `Either<A, B>` as `A | B` in the generated `.d.ts`,")
                            $("// so the async iterator's element type reads `item | <Op>ErrorEvent`.")
                            $("// (Unannotated ops keep `type Yield = <item>` — this surface is untouched.)")
                            type Yield = napi::bindgen_prelude::Either<$(&item), $(&err_evt)>;
                            type Next = ();
                            type Return = ();

                            fn next(
                                &mut self,
                                _value: Option<Self::Next>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                let stream = self.stream.clone();
                                let closed = self.closed.clone();
                                async move {
                                    use std::sync::atomic::Ordering;
                                    $("// A started stream never restarts: once the terminal error event has")
                                    $("// been handed out the latch is set, so every subsequent pull completes.")
                                    if closed.load(Ordering::SeqCst) {
                                        return Ok(None);
                                    }
                                    loop {
                                        let s = stream.clone();
                                        $("// Drive the blocking poll off the async runtime so the Node")
                                        $("// event loop is never blocked.")
                                        let poll = napi::tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        $("// event-mode: a mid-stream failure is ENCODED IN THE STREAM as a")
                                        $("// terminal error EVENT and the stream then completes — it must")
                                        $("// NEVER reject/throw. `Poll::Failed` yields `Either::B(event)` then")
                                        $("// the latch makes the next pull return `Ok(None)`.")
                                        match poll {
                                            Poll::Item(v) => return Ok(Some(napi::bindgen_prelude::Either::A(v))),
                                            Poll::Idle => continue,
                                            Poll::Closed => return Ok(None),
                                            Poll::Failed(e) => {
                                                $("// latch closed so the next pull completes, then hand the failure")
                                                $("// out AS A VALUE — never a thrown/rejected error.")
                                                closed.store(true, Ordering::SeqCst);
                                                return Ok(Some(napi::bindgen_prelude::Either::B($(&err_evt) {
                                                    type_: $(quoted(se.tag_value.clone())).into(),
                                                    reason: "error".into(),
                                                    error: e,
                                                })));
                                            }
                                        }
                                    }
                                }
                            }

                            fn complete(
                                &mut self,
                                _value: Option<Self::Return>,
                            ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
                                $("// Cancellation: consumer called `return()` (e.g. `break` in for-await).")
                                let stream = self.stream.clone();
                                async move {
                                    stream.close();
                                    Ok(None)
                                }
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }

                        $("// Retained poll cursor: `next(): Promise<Item | <Op>ErrorEvent | null>`.")
                        pub struct Next$(&class)Task {
                            stream: Arc<dyn PollStream<$(&item)>>,
                            closed: Arc<std::sync::atomic::AtomicBool>,
                        }
                        impl Task for Next$(&class)Task {
                            $("// Either::A = a normal item; Either::B = the terminal error event. The")
                            $("// in-stream failure path is a VALUE, never a rejected promise.")
                            type Output = Option<napi::bindgen_prelude::Either<$(&item), $(&err_evt)>>;
                            type JsValue = Option<napi::bindgen_prelude::Either<$(&item), $(&err_evt)>>;
                            fn compute(&mut self) -> Result<Self::Output> {
                                use std::sync::atomic::Ordering;
                                if self.closed.load(Ordering::SeqCst) {
                                    return Ok(None);
                                }
                                loop {
                                    match self.stream.poll(Duration::from_millis(500)) {
                                        Poll::Item(v) => return Ok(Some(napi::bindgen_prelude::Either::A(v))),
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None),
                                        Poll::Failed(e) => {
                                            $("// latch closed so the next next() resolves null, then hand the")
                                            $("// failure out AS A VALUE — never a thrown/rejected error.")
                                            self.closed.store(true, Ordering::SeqCst);
                                            return Ok(Some(napi::bindgen_prelude::Either::B($(&err_evt) {
                                                type_: $(quoted(se.tag_value.clone())).into(),
                                                reason: "error".into(),
                                                error: e,
                                            })));
                                        }
                                    }
                                }
                            }
                            fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                                Ok(o)
                            }
                        }
                        #[napi]
                        impl $(&class) {
                            #[napi(ts_return_type = $(quoted(ret_ts)))]
                            pub fn next(&self) -> AsyncTask<Next$(&class)Task> {
                                AsyncTask::new(Next$(&class)Task { stream: self.stream.clone(), closed: self.closed.clone() })
                            }
                        }
                        $['\n']
                    };
                }
            }
        }

        // unary op tasks
        for op in i.ops.iter().filter(|o| o.shape == Shape::Unary) {
            let task = format!("{}Task", pascal(&op.name));
            let name = snake(&op.name);
            let (ret, _) = ty(api, &op.returns);
            let fields: Vec<String> = param_sig(api, op)
                .iter()
                .map(|(n, r)| format!("{n}: {r},"))
                .collect();
            let args = param_sig(api, op)
                .iter()
                .map(|(n, _)| format!("self.{n}.clone()"))
                .collect::<Vec<_>>()
                .join(", ");
            let call = if has_ctor {
                format!("self.core.{name}({args})")
            } else {
                format!("<{impl_path} as {trait_name}>::{name}({args})")
            };
            let core_field = if has_ctor {
                format!("core: Arc<{impl_path}>,")
            } else {
                String::new()
            };
            quote_in! { t =>
                $['\r']
                pub struct $(&task) {
                    $core_field
                    $(for f in &fields join ($['\r']) => $f)
                }
                impl Task for $(&task) {
                    type Output = $(&ret);
                    type JsValue = $(&ret);
                    fn compute(&mut self) -> Result<Self::Output> {
                        $call.map_err(err)
                    }
                    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                        Ok(o)
                    }
                }
                $['\n']
            };
        }

        if has_ctor {
            // the handle class
            let mut methods: rust::Tokens = quote!();
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape != Shape::Manual {
                    if let Some(doc) = &op.doc {
                        for line in doc.lines() {
                            quote_in! { methods => $['\r']$(format!("/// {line}")) };
                        }
                    }
                }
                let params: Vec<String> = param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect();
                let ps = params.join(", ");
                let names = param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                match op.shape {
                    Shape::Ctor => quote_in! { methods =>
                        $['\r']
                        #[napi(constructor)]
                        pub fn new($(&ps)) -> Result<Self> {
                            Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&names)).map_err(err)?) })
                        }
                    },
                    Shape::Unary => {
                        let task = format!("{}Task", pascal(&op.name));
                        let (_, ts_ret) = ty(api, &op.returns);
                        quote_in! { methods =>
                            $['\r']
                            #[napi(ts_return_type = $(quoted(format!("Promise<{ts_ret}>"))))]
                            pub fn $(&name)(&self, $(&ps)) -> AsyncTask<$(&task)> {
                                AsyncTask::new($(&task) { core: self.core.clone(), $(&names) })
                            }
                        }
                    }
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        // The `closed` latch field exists only in event-mode
                        // (`@streamError`); default throw-mode streams have no latch.
                        let closed_init = if op.stream_error.is_some() {
                            "closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),"
                        } else {
                            ""
                        };
                        quote_in! { methods =>
                            $['\r']
                            $("// pre-start boundary: building the stream (setup/validation) always")
                            $("// THROWS on a core Err — independent of the stream's error model.")
                            #[napi]
                            pub fn $(&name)(&self, $(&ps)) -> Result<$(&class)> {
                                Ok($(&class) {
                                    stream: Arc::from(self.core.$(&name)($(&names)).map_err(err)?),
                                    $closed_init
                                })
                            }
                        }
                    }
                    Shape::Manual => quote_in! { methods =>
                        $['\r']
                        $(format!("// @manual: {} — hand-written in lib.rs.", op.name))
                    },
                }
            }
            if let Some(doc) = &i.doc {
                for line in doc.lines() {
                    quote_in! { t => $['\r']$(format!("/// {line}")) };
                }
            }
            quote_in! { t =>
                $['\r']
                #[napi]
                pub struct $(&i.name) {
                    $("// pub(crate): the @manual ops in lib.rs extend this class and need the core")
                    pub(crate) core: Arc<$(&impl_path)>,
                }

                #[napi]
                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        } else {
            // stateless interface → free functions
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape == Shape::Manual {
                    quote_in! { t => $['\r']$(format!("// @manual: {}.{} — hand-written in lib.rs.", i.name, op.name)) };
                    continue;
                }
                let task = format!("{}Task", pascal(&op.name));
                let (_, ts_ret) = ty(api, &op.returns);
                let params: Vec<String> = param_sig(api, op)
                    .iter()
                    .map(|(n, r)| format!("{n}: {r}"))
                    .collect();
                let ps = params.join(", ");
                let names = param_sig(api, op)
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                quote_in! { t =>
                    $['\r']
                    #[napi(ts_return_type = $(quoted(format!("Promise<{ts_ret}>"))))]
                    pub fn $(&name)($(&ps)) -> AsyncTask<$(&task)> {
                        AsyncTask::new($(&task) { $(&names) })
                    }
                    $['\n']
                };
            }
        }
    }

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let body = t.to_file_string().expect("rust renders");
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note)
    ))
}
