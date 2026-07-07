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
        use std::sync::Arc;
        use std::time::Duration;
        use napi::bindgen_prelude::{AsyncTask, Result};
        use napi::{Env, Task};
        use napi_derive::napi;

        fn err(e: impl std::fmt::Display) -> napi::Error {
            napi::Error::from_reason(e.to_string())
        }

        $("/// One poll result from a core stream (the sync primitive every stream shape dresses).")
        pub enum Poll<T> {
            Item(T),
            Idle,
            Closed,
        }

        $("/// The one sync primitive: a blocking, timeout-bounded poll.")
        pub trait PollStream<T>: Send + Sync {
            fn poll(&self, timeout: Duration) -> Poll<T>;
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

    // ── enums (name-only variants → napi enums; wire-valued → strings) ──
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let vs: Vec<String> = variants.iter().map(|v| pascal(v)).collect();
        // napi 3 no longer auto-derives Clone/Copy on #[napi] enums; option
        // structs that carry one derive Clone, so the enum must too.
        quote_in! { t =>
            $['\n']
            #[napi]
            #[derive(Clone, Copy)]
            pub enum $name {
                $(for v in &vs join ($['\r']) => $v,)
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

        // stream classes + next-tasks
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, ts_item) = ty(api, &op.returns);
            quote_in! { t =>
                $['\r']
                $(format!("/// Poll-based stream from `{}.{}` — call `next()` until it resolves null.", i.name, op.name))
                #[napi]
                pub struct $(&class) {
                    stream: Arc<dyn PollStream<$(&item)>>,
                }
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
                            }
                        }
                    }
                    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> {
                        Ok(o)
                    }
                }
                #[napi]
                impl $(&class) {
                    #[napi(ts_return_type = $(quoted(format!("Promise<{ts_item} | null>"))))]
                    pub fn next(&self) -> AsyncTask<Next$(&class)Task> {
                        AsyncTask::new(Next$(&class)Task { stream: self.stream.clone() })
                    }
                }
                $['\n']
            };
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
                        quote_in! { methods =>
                            $['\r']
                            #[napi]
                            pub fn $(&name)(&self, $(&ps)) -> Result<$(&class)> {
                                Ok($(&class) { stream: Arc::from(self.core.$(&name)($(&names)).map_err(err)?) })
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
