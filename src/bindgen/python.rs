//! The PyO3 (Python) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map via the shared [`pinned_name`] resolver. Python hardcodes no
/// pin; its only rename syntax is `#[pyo3(name = "…")]`.
const LANG: &str = "python";

/// One parameter of a flattened Python signature: model-typed op params are
/// expanded into their fields as keyword arguments (the pythonic idiom the
/// hand-written binding used), then reassembled into the options struct before
/// the trait call.
struct PyParam {
    name: String,
    rust_ty: String,
    /// `= None` in the signature (optional field / optional param).
    defaulted: bool,
    /// `Some((model, all_optional))` when this param came from flattening.
    group: Option<String>,
}

fn py_reserved(name: &str) -> String {
    match name {
        "from" | "import" | "class" | "def" | "return" | "pass" | "global" | "lambda" | "None"
        | "True" | "False" => format!("{name}_"),
        _ => name.to_string(),
    }
}

/// Flatten an op's params for Python: scalars pass through; model-typed params
/// expand to their fields (optional → `= None` keywords).
fn py_flatten(api: &ApiDoc, op: &ApiOp) -> Vec<PyParam> {
    let mut out = Vec::new();
    // NB: callers re-sort nothing — required params must precede defaulted ones,
    // which holds because required model fields precede optional ones in the
    // catalog and required op params precede model bags in the op surface.
    for p in &op.params {
        let model_name = match &p.ty {
            ApiType::Model { model } => Some(model.clone()),
            _ => None,
        };
        if let Some(model) = model_name {
            let m = api
                .models
                .iter()
                .find(|m| m.name == model)
                .expect("model in api.json");
            for f in &m.fields {
                let (r, _) = ty(api, &f.ty);
                out.push(PyParam {
                    name: py_reserved(&snake(&f.name)),
                    rust_ty: if f.nullable {
                        format!("Option<{r}>")
                    } else {
                        r
                    },
                    defaulted: f.nullable,
                    group: Some(model.clone()),
                });
            }
        } else {
            let (r, _) = ty(api, &p.ty);
            let optional = p.optional == Some(true);
            out.push(PyParam {
                name: py_reserved(&snake(&p.name)),
                rust_ty: if optional { format!("Option<{r}>") } else { r },
                defaulted: optional,
                group: None,
            });
        }
    }
    out
}

/// The `#[pyo3(signature = …)]` attribute + fn params + the body prelude that
/// reassembles flattened groups, + the argument list for the trait call.
fn py_op_pieces(api: &ApiDoc, op: &ApiOp) -> (String, String, String, String) {
    let flat = py_flatten(api, op);
    let signature = flat
        .iter()
        .map(|p| {
            if p.defaulted {
                format!("{}=None", p.name)
            } else {
                p.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let fn_params = flat
        .iter()
        .map(|p| format!("{}: {}", p.name, p.rust_ty))
        .collect::<Vec<_>>()
        .join(", ");
    // group reassembly, in first-appearance order
    let mut prelude = String::new();
    let mut seen = Vec::new();
    for p in &flat {
        if let Some(g) = &p.group {
            if !seen.contains(g) {
                seen.push(g.clone());
            }
        }
    }
    for g in &seen {
        let fields: Vec<String> = flat
            .iter()
            .filter(|p| p.group.as_deref() == Some(g))
            .map(|p| {
                let m = api.models.iter().find(|m| &m.name == g).unwrap();
                let orig = m
                    .fields
                    .iter()
                    .find(|f| py_reserved(&snake(&f.name)) == p.name)
                    .map(|f| snake(&f.name))
                    .unwrap_or_else(|| p.name.clone());
                if orig == p.name {
                    orig
                } else {
                    format!("{orig}: {}", p.name)
                }
            })
            .collect();
        prelude.push_str(&format!(
            "let {}_arg = {g} {{ {} }};\n",
            snake(g),
            fields.join(", ")
        ));
    }
    // the trait-call argument list, in the op's original param order
    let args = op
        .params
        .iter()
        .map(|p| match &p.ty {
            ApiType::Model { model } => {
                if p.optional == Some(true) {
                    format!("Some({}_arg)", snake(model))
                } else {
                    format!("{}_arg", snake(model))
                }
            }
            _ => py_reserved(&snake(&p.name)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    (signature, fn_params, prelude, args)
}

/// The Python projection of an Arrow-payload DTO: a pyclass holding the
/// `RecordBatch`, envelope getters, a lazy `ipc()` method, and the **Arrow
/// PyCapsule Interface** (`__arrow_c_schema__` / `__arrow_c_array__`) — the
/// standard zero-copy C Data Interface handoff. pyarrow/polars/pandas import
/// the capsules directly (`pa.record_batch(batch)`); entl itself needs no
/// pyarrow. Capsule ownership follows the standard contract: the capsule owns
/// the FFI struct; a consumer that imports it marks it released, and an
/// unconsumed capsule's destructor drops the struct, whose Drop honors release.
fn emit_py_arrow_model(
    t: &mut rust::Tokens,
    api: &ApiDoc,
    m: &crate::api::ApiModel,
    af: &crate::api::ApiField,
) {
    let plain: Vec<&crate::api::ApiField> = m.fields.iter().filter(|f| f.name != af.name).collect();
    let storage: Vec<rust::Tokens> = plain
        .iter()
        .map(|f| {
            let (r, _) = ty(api, &f.ty);
            let n = py_reserved(&snake(&f.name));
            quote!(pub(crate) $n: $r,)
        })
        .collect();
    let getters: Vec<rust::Tokens> = plain
        .iter()
        .map(|f| {
            let (r, _) = ty(api, &f.ty);
            let n = py_reserved(&snake(&f.name));
            quote! {
                #[getter]
                fn $(&n)(&self) -> $r {
                    self.$(&n).clone()
                }
            }
        })
        .collect();
    let ipc = py_reserved(&snake(&af.name));
    if let Some(doc) = &m.doc {
        for line in doc.lines() {
            quote_in! { *t => $['\r']$(format!("/// {line}")) };
        }
    }
    quote_in! { *t =>
        $['\r']
        #[pyclass]
        #[derive(Clone)]
        pub struct $(&m.name) {
            $(for f in &storage join ($['\r']) => $f)
            $("// the rows, still columnar — capsule-exported or encoded on demand")
            pub(crate) batch: entl_core::RecordBatch,
        }
        #[pymethods]
        impl $(&m.name) {
            $(for g in &getters join ($['\r']) => $g)
            $("/// The rows as one Arrow IPC stream (`pyarrow.ipc.open_stream`-able) —")
            $("/// for consumers that want bytes rather than the zero-copy capsules.")
            fn $(&ipc)(&self) -> PyResult<Bytes> {
                entl_core::batch_ipc(&self.batch).map_err(err)
            }
            $("/// Arrow PyCapsule interface — the schema half of the C Data Interface.")
            fn __arrow_c_schema__(&self, py: Python<$("'_")>) -> PyResult<Py<pyo3::types::PyCapsule>> {
                let (_, schema) = entl_core::batch_to_ffi(&self.batch).map_err(err)?;
                let name = std::ffi::CString::new($(quoted("arrow_schema"))).expect("static cstr");
                Ok(pyo3::types::PyCapsule::new(py, schema, Some(name))?.unbind())
            }
            $("/// Arrow PyCapsule interface — (schema, array) capsules; `pa.record_batch(batch)`")
            $("/// imports the rows zero-copy. `requested_schema` is accepted and ignored (spec-permitted).")
            #[pyo3(signature = (requested_schema=None))]
            fn __arrow_c_array__(
                &self,
                py: Python<$("'_")>,
                requested_schema: Option<Bound<$("'_"), PyAny>>,
            ) -> PyResult<(Py<pyo3::types::PyCapsule>, Py<pyo3::types::PyCapsule>)> {
                let _ = requested_schema;
                let (array, schema) = entl_core::batch_to_ffi(&self.batch).map_err(err)?;
                let sname = std::ffi::CString::new($(quoted("arrow_schema"))).expect("static cstr");
                let aname = std::ffi::CString::new($(quoted("arrow_array"))).expect("static cstr");
                let s = pyo3::types::PyCapsule::new(py, schema, Some(sname))?.unbind();
                let a = pyo3::types::PyCapsule::new(py, array, Some(aname))?.unbind();
                Ok((s, a))
            }
        }
        $['\n']
    };
}

/// Generate the PyO3 (Python) binding: pyclass DTOs + enums, the core traits,
/// `#[pyfunction]`s with the GIL released, kwargs-flattened methods, iterator
/// stream classes, and a `register()` for the `#[pymodule]`.
pub fn python_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    let mut t: rust::Tokens = quote! {
        use std::sync::Arc;
        use std::time::Duration;
        use pyo3::exceptions::PyRuntimeError;
        use pyo3::prelude::*;

        fn err(e: impl std::fmt::Display) -> PyErr {
            PyRuntimeError::new_err(e.to_string())
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
            $("/// Bulk bytes cross into Python as `bytes` (Arrow IPC payloads and friends).")
            pub type Bytes = Vec<u8>;
        };
    }
    t.line();

    let mut class_names: Vec<String> = Vec::new();
    let mut fn_names: Vec<String> = Vec::new();

    // ── enums ──
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        class_names.push(name.clone());
        // PyO3 has no wire-string concept for enum members: a member's name is
        // its Rust ident (`pascal(name)`) unless a `python` pin overrides it via
        // `#[pyo3(name = "…")]`. Un-pinned ⇒ bare ident, byte-identical.
        let vs: Vec<String> = variants
            .iter()
            .map(|v| match pinned_name(&v.bindings, LANG) {
                Some(nm) => format!("#[pyo3(name = {:?})] {},", nm, pascal(&v.name)),
                None => format!("{},", pascal(&v.name)),
            })
            .collect();
        quote_in! { t =>
            $['\n']
            #[pyclass(eq, eq_int)]
            #[derive(Clone, Copy, PartialEq)]
            pub enum $name {
                $(for v in &vs join ($['\r']) => $v)
            }
        };
    }
    t.line();

    // ── DTO structs (constructible from Python; fields readable via get_all) ──
    for m in &api.models {
        class_names.push(m.name.clone());
        if let Some(af) = arrow_field(m) {
            emit_py_arrow_model(&mut t, api, m, af);
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
                // The Rust field ident stays `snake` (a valid ident); a `python`
                // pin puts the exact Python attribute name ONLY in a
                // `#[pyo3(name = "…")]` attr (overriding pyo3's default, which is
                // the Rust ident). Un-pinned ⇒ no attr, byte-identical.
                let n = py_reserved(&snake(&f.name));
                match pinned_name(&f.bindings, LANG) {
                    Some(nm) => {
                        let attr = format!("#[pyo3(name = {nm:?})]");
                        quote!($attr pub $n: $r,)
                    }
                    None => quote!(pub $n: $r,),
                }
            })
            .collect();
        // ctor param order: required fields first, then `=None` optionals (python rule)
        let ctor_fields: Vec<&crate::api::ApiField> = m
            .fields
            .iter()
            .filter(|f| !f.nullable)
            .chain(m.fields.iter().filter(|f| f.nullable))
            .collect();
        let sig = ctor_fields
            .iter()
            .map(|f| {
                let n = py_reserved(&snake(&f.name));
                if f.nullable {
                    format!("{n}=None")
                } else {
                    n
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let params = ctor_fields
            .iter()
            .map(|f| {
                let (r, _) = ty(api, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                format!("{}: {}", py_reserved(&snake(&f.name)), r)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let names = ctor_fields
            .iter()
            .map(|f| py_reserved(&snake(&f.name)))
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                quote_in! { t => $['\r']$(format!("/// {line}")) };
            }
        }
        quote_in! { t =>
            $['\r']
            #[pyclass(get_all)]
            #[derive(Clone)]
            pub struct $(&m.name) {
                $(for f in &fields join ($['\r']) => $f)
            }
            #[pymethods]
            impl $(&m.name) {
                #[new]
                #[pyo3(signature = ($(&sig)))]
                fn new($(&params)) -> Self {
                    Self { $(&names) }
                }
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

        // stream classes: python iterators (GIL released while polling)
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            class_names.push(class.clone());
            let (item, _) = ty(api, &op.returns);
            quote_in! { t =>
                $['\r']
                $(format!("/// Poll-based stream from `{}.{}`, dressed as a Python iterator.", i.name, op.name))
                #[pyclass]
                pub struct $(&class) {
                    stream: Box<dyn PollStream<$(&item)>>,
                }
                #[pymethods]
                impl $(&class) {
                    fn __iter__(slf: PyRef<$("'_"), Self>) -> PyRef<$("'_"), Self> {
                        slf
                    }
                    fn __next__(&self, py: Python<$("'_")>) -> Option<$(&item)> {
                        py.detach(|| loop {
                            match self.stream.poll(Duration::from_millis(500)) {
                                Poll::Item(v) => return Some(v),
                                Poll::Idle => continue,
                                Poll::Closed => return None, $("// None => StopIteration")
                            }
                        })
                    }
                }
                $['\n']
            };
        }

        if has_ctor {
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
                let (signature, fn_params, prelude, args) = py_op_pieces(api, op);
                let (ret, _) = ty(api, &op.returns);
                match op.shape {
                    // Emit the signature attr + the struct-reassembly prelude only
                    // when the ctor actually needs them — a flattened options DTO
                    // (non-empty prelude) or defaulted params (`=None`). A plain
                    // all-required scalar ctor stays bare, as pyo3 infers it.
                    Shape::Ctor if signature.contains("=None") || !prelude.trim().is_empty() => {
                        quote_in! { methods =>
                            $['\r']
                            #[new]
                            #[pyo3(signature = ($(&signature)))]
                            fn new($(&fn_params)) -> PyResult<Self> {
                                $prelude
                                Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&args)).map_err(err)?) })
                            }
                        }
                    }
                    Shape::Ctor => quote_in! { methods =>
                        $['\r']
                        #[new]
                        fn new($(&fn_params)) -> PyResult<Self> {
                            Ok(Self { core: Arc::new(<$(&impl_path) as $(&trait_name)>::$(&name)($(&args)).map_err(err)?) })
                        }
                    },
                    Shape::Unary => quote_in! { methods =>
                        $['\r']
                        #[pyo3(signature = ($(&signature)))]
                        fn $(&name)(&self, py: Python<$("'_")>, $(&fn_params)) -> PyResult<$(&ret)> {
                            $prelude
                            let core = self.core.clone();
                            py.detach(move || core.$(&name)($(&args))).map_err(err)
                        }
                    },
                    Shape::Stream => {
                        let class = pascal(&op.name);
                        quote_in! { methods =>
                            $['\r']
                            #[pyo3(signature = ($(&signature)))]
                            fn $(&name)(&self, $(&fn_params)) -> PyResult<$(&class)> {
                                $prelude
                                Ok($(&class) { stream: self.core.$(&name)($(&args)).map_err(err)? })
                            }
                        }
                    }
                    Shape::Manual => quote_in! { methods =>
                        $['\r']
                        $(format!("// @manual: {} — hand-written in lib.rs if this binding offers it.", op.name))
                    },
                }
            }
            class_names.push(i.name.clone());
            if let Some(doc) = &i.doc {
                for line in doc.lines() {
                    quote_in! { t => $['\r']$(format!("/// {line}")) };
                }
            }
            quote_in! { t =>
                $['\r']
                #[pyclass]
                pub struct $(&i.name) {
                    $("// pub(crate): @manual ops in lib.rs extend this class and need the core")
                    pub(crate) core: Arc<$(&impl_path)>,
                }

                #[pymethods]
                impl $(&i.name) {
                    $methods
                }
                $['\n']
            };
        } else {
            for op in &i.ops {
                let name = snake(&op.name);
                if op.shape == Shape::Manual {
                    quote_in! { t => $['\r']$(format!("// @manual: {}.{} — hand-written in lib.rs if offered.", i.name, op.name)) };
                    continue;
                }
                fn_names.push(name.clone());
                let (signature, fn_params, prelude, args) = py_op_pieces(api, op);
                let (ret, _) = ty(api, &op.returns);
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                quote_in! { t =>
                    $['\r']
                    #[pyfunction]
                    #[pyo3(signature = ($(&signature)))]
                    fn $(&name)(py: Python<$("'_")>, $(&fn_params)) -> PyResult<$(&ret)> {
                        $prelude
                        py.detach(move || <$(&impl_path) as $(&trait_name)>::$(&name)($(&args))).map_err(err)
                    }
                    $['\n']
                };
            }
        }
    }

    // ── module registration ──
    let adds: Vec<String> = class_names
        .iter()
        .map(|c| format!("m.add_class::<{c}>()?;"))
        .chain(
            fn_names
                .iter()
                .map(|f| format!("m.add_function(wrap_pyfunction!({f}, m)?)?;")),
        )
        .collect();
    quote_in! { t =>
        $['\r']
        $("/// Register every generated class + function on the `#[pymodule]`.")
        pub(crate) fn register(m: &Bound<$("'_"), PyModule>) -> PyResult<()> {
            $(for a in &adds join ($['\r']) => $a)
            Ok(())
        }
    };

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let body = t.to_file_string().expect("rust renders");
    crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer). Do not edit.\n{}#![allow(clippy::all)]\n\n{body}",
        note_line(banner_note)
    ))
}
