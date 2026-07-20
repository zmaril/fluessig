//! The PyO3 (Python) template grid — one language's projection of the op shapes.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use genco::prelude::*;

use crate::api::{ApiDoc, ApiOp, ApiType, ApiUnion, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map via the shared [`pinned_name`] resolver. Python hardcodes no
/// pin; its only rename syntax is `#[pyo3(name = "…")]`.
const LANG: &str = "python";

/// Backend options for the PyO3 (Python) generator. The 3-arg [`python_binding`]
/// threads `PythonOptions::default()`, whose [`UnionProjection::default`] is
/// structured tagged-object projection; pass [`UnionProjection::Envelope`] to opt
/// back into the historical JSON-string carrier.
#[derive(Default, Clone)]
pub struct PythonOptions {
    /// How union return values and nested union DTO fields are lowered.
    pub union_projection: UnionProjection,
}

/// A union eligible for structured projection: at least two variants, all of
/// which are model refs. PyO3 imposes no upper arity cap (the tagged variants
/// ride a plain Rust enum, not napi's `Either`), so any such union projects; a
/// mixed or degenerate union falls back to the JSON envelope carrier.
fn py_structured_union<'a>(api: &'a ApiDoc, name: &str) -> Option<&'a ApiUnion> {
    api.unions.iter().find(|u| u.name == name).filter(|u| {
        u.variants.len() >= 2
            && u.variants
                .iter()
                .all(|v| matches!(&v.ty, ApiType::Model { .. }))
    })
}

/// The python `(rust, _)` spelling of a type, applying structured union
/// projection when [`PythonOptions::union_projection`] asks for it: a union
/// lowers to its generated `{Union}Union` enum (an `IntoPyObject`/`FromPyObject`
/// wrapper over the per-variant `#[pyclass]` structs). Delegates to the shared
/// [`ty`] for everything else, so envelope mode is byte-identical to the
/// historical output. The second tuple element (a ts spelling) is unused by the
/// python backend and mirrors the rust one.
fn python_ty(api: &ApiDoc, opts: &PythonOptions, t: &ApiType) -> (String, String) {
    match (t, &opts.union_projection) {
        (ApiType::Union { union }, UnionProjection::Structured { .. }) => {
            match py_structured_union(api, union) {
                Some(u) => {
                    let n = union_enum_name(&u.name);
                    (n.clone(), n)
                }
                None => ty(api, t),
            }
        }
        (ApiType::List { list }, _) => {
            let (r, s) = python_ty(api, opts, list);
            (format!("Vec<{r}>"), format!("{s}[]"))
        }
        (ApiType::Nullable { nullable }, _) => {
            let (r, s) = python_ty(api, opts, nullable);
            (format!("Option<{r}>"), format!("{s} | null"))
        }
        _ => ty(api, t),
    }
}

/// Python's `<Interface>Core` traits: the shared [`emit_core_traits_with`] spine
/// driven with python's structured return mapping ([`python_ty`]) so a
/// union-returning op's core-trait signature matches the `#[pymethods]` return
/// (`{Union}Union`). In envelope mode `python_ty` delegates to `ty`, so the
/// output is byte-identical to the historical default.
fn emit_core_traits_python(t: &mut rust::Tokens, api: &ApiDoc, opts: &PythonOptions) {
    emit_core_traits_with(t, api, |op| python_ty(api, opts, &op.returns).0);
}

/// Emit, for every structurally-projected union, one `#[pyclass(get_all)]`
/// struct per variant (the discriminant as a readable literal attribute plus the
/// variant model's fields, a `#[new]` ctor setting the literal, and a
/// `From<VariantModel>` conversion), then the `{Union}Union` enum wrapping them
/// (`#[derive(IntoPyObject, FromPyObject)]` — tagged Python object out,
/// class-discriminated in). Registers each variant class in `class_names`.
/// Nothing is emitted in envelope mode.
fn emit_py_union_variants(
    t: &mut rust::Tokens,
    api: &ApiDoc,
    opts: &PythonOptions,
    class_names: &mut Vec<String>,
) {
    let UnionProjection::Structured { tag_field } = &opts.union_projection else {
        return;
    };
    for u in &api.unions {
        let Some(u) = py_structured_union(api, &u.name) else {
            quote_in! { *t =>
                $['\r']
                $(format!("// note: union {} is not structurally projectable (needs >=2 model-ref variants) — kept as the JSON envelope carrier.", u.name))
            };
            continue;
        };
        let field = union_tag_field(u, tag_field);
        let ident = tag_ident(&field);
        let mut arms: Vec<String> = Vec::new();
        for v in &u.variants {
            let sname = tagged_variant_name(&u.name, &v.tag);
            arms.push(format!("{}({sname})", pascal(&v.tag)));
            let ApiType::Model { model } = &v.ty else {
                continue;
            };
            let Some(m) = api.models.iter().find(|m| &m.name == model) else {
                continue;
            };
            // struct fields: the tag first, then the variant model's real fields
            let mut struct_fields: Vec<rust::Tokens> = Vec::new();
            struct_fields.push(quote!($(format!("pub {ident}: String,"))));
            let mut from_fields: Vec<String> = Vec::new();
            from_fields.push(format!("{ident}: {:?}.into(),", v.tag));
            for f in &m.fields {
                let (r, _) = python_ty(api, opts, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                let fname = py_reserved(&snake(&f.name));
                struct_fields.push(quote!($(format!("pub {fname}: {r},"))));
                from_fields.push(format!("{fname}: v.{fname},"));
            }
            // ctor param order: required fields first, then `=None` optionals
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
                    let (r, _) = python_ty(api, opts, &f.ty);
                    let r = if f.nullable {
                        format!("Option<{r}>")
                    } else {
                        r
                    };
                    format!("{}: {}", py_reserved(&snake(&f.name)), r)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let ctor_names = ctor_fields
                .iter()
                .map(|f| py_reserved(&snake(&f.name)))
                .collect::<Vec<_>>()
                .join(", ");
            quote_in! { *t =>
                $['\r']
                $(format!("/// `{}` union variant `{}` — the tag `{}` rides as the `{}` literal attribute.", u.name, v.tag, v.tag, field))
                #[pyclass(get_all)]
                #[derive(Clone)]
                pub struct $(&sname) {
                    $(for f in &struct_fields join ($['\r']) => $f)
                }
                #[pymethods]
                impl $(&sname) {
                    #[new]
                    #[pyo3(signature = ($(&sig)))]
                    fn new($(&params)) -> Self {
                        Self { $(format!("{ident}: {:?}.into(),", v.tag)) $(&ctor_names) }
                    }
                }
                impl From<$model> for $(&sname) {
                    fn from(v: $model) -> Self {
                        Self {
                            $(for f in &from_fields join ($['\r']) => $f)
                        }
                    }
                }
                $['\n']
            };
            class_names.push(sname);
        }
        let enum_name = union_enum_name(&u.name);
        quote_in! { *t =>
            $['\r']
            $(format!("/// The `{}` tagged union — a tagged Python object out (the matched variant's", u.name))
            $("/// `#[pyclass]`), class-discriminated in. Not a pyclass itself; the variant")
            $("/// classes carry the surface.")
            #[derive(Clone, IntoPyObject, FromPyObject)]
            pub enum $(&enum_name) {
                $(for a in &arms join ($['\r']) => $(format!("{a},")))
            }
            $['\n']
        };
    }
}

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

/// Generate the PyO3 (Python) binding with default options: structured
/// tagged-object union projection (per-variant `#[pyclass]`es wrapped in a
/// `{Union}Union` enum, tag field `"type"`). A thin wrapper over
/// [`python_binding_with_options`]; pass [`UnionProjection::Envelope`] to opt into
/// the JSON-string carrier.
pub fn python_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    python_binding_with_options(api, enums, banner_note, &PythonOptions::default())
}

/// Generate the PyO3 (Python) binding: pyclass DTOs + enums, the core traits,
/// `#[pyfunction]`s with the GIL released, kwargs-flattened methods, iterator
/// stream classes, and a `register()` for the `#[pymodule]`. `opts` selects union
/// projection (structured per-variant pyclasses vs. the JSON envelope).
pub fn python_binding_with_options(
    api: &ApiDoc,
    enums: &[EnumDesc],
    banner_note: Option<&str>,
    opts: &PythonOptions,
) -> String {
    // The shared streaming-contract import flows through the use-emitter
    // ([`RUNTIME_STREAM_IMPORT`]) rather than a hardcoded string, so every
    // generated `use` line has one emission path; renders byte-identically.
    let runtime_import = RUNTIME_STREAM_IMPORT.render();
    let mut t: rust::Tokens = quote! {
        use std::sync::Arc;
        use std::time::Duration;
        use pyo3::exceptions::PyRuntimeError;
        use pyo3::exceptions::PyStopAsyncIteration;
        use pyo3::prelude::*;
        $("// The shared streaming contract — Poll/PollStream live in the fluessig-runtime crate.")
        $(&runtime_import)

        fn err(e: impl std::fmt::Display) -> PyErr {
            PyRuntimeError::new_err(e.to_string())
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
                // structured projection reaches nested union-typed DTO fields too
                let (r, _) = python_ty(api, opts, &f.ty);
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
                let (r, _) = python_ty(api, opts, &f.ty);
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

    // per-variant tagged pyclasses (+ the {Union}Union enum) for structured unions
    emit_py_union_variants(&mut t, api, opts, &mut class_names);

    emit_core_traits_python(&mut t, api, opts);

    // ── per-interface surface ──
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream classes: python async-iterables + a retained sync poll cursor.
        // The error model is chosen per-op by `stream_error`, mirroring node:
        // `None` (unannotated) = DEFAULT throw-mode — a mid-stream `Poll::Failed`
        // RAISES (the awaited `__anext__` / sync `__next__` propagates `err(e)`);
        // `Some(shape)` = opt-in error-AS-EVENT — the failure is yielded as a
        // terminal `<Op>ErrorEvent` then the stream latches closed (NEVER raises).
        // `Poll::Failed(String)` is the core→binding channel in BOTH modes.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            class_names.push(class.clone());
            let (item, _) = python_ty(api, opts, &op.returns);
            match &op.stream_error {
                // ── DEFAULT throw-mode (unannotated): raise on Poll::Failed ──
                None => {
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// Event stream from `{}.{}`, dressed as a Python async-iterable.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: `async for ev in stream` (`__aiter__`/`__anext__`),")
                        $("/// driven off the asyncio loop. Retained surface: the sync iterator")
                        $("/// (`__iter__`/`__next__`) poll cursor for consumers not on asyncio.")
                        $("///")
                        $("/// DEFAULT error model = RAISE: a mid-stream core failure (`Poll::Failed`)")
                        $("/// maps to `Err(err(e))`, so the awaited `__anext__` (and the sync")
                        $("/// `__next__`) propagates a Python exception. Annotate the op")
                        $("/// `@streamError` to opt into the error-AS-EVENT model instead.")
                        #[pyclass]
                        pub struct $(&class) {
                            $("// Arc, not Box: the async future is `'static` and moves the handle")
                            $("// across `.await` / `spawn_blocking`.")
                            stream: Arc<dyn PollStream<$(&item)>>,
                        }
                        #[pymethods]
                        impl $(&class) {
                            fn __iter__(slf: PyRef<$("'_"), Self>) -> PyRef<$("'_"), Self> {
                                slf
                            }
                            $("// Retained SYNC poll cursor. A terminal `Poll::Failed` raises a Python")
                            $("// exception (throw-mode): the sync iterator has no error-as-event")
                            $("// surface, so a mid-stream core failure surfaces as `err(e)`.")
                            fn __next__(&self, py: Python<$("'_")>) -> PyResult<Option<$(&item)>> {
                                py.detach(|| loop {
                                    match self.stream.poll(Duration::from_millis(500)) {
                                        Poll::Item(v) => return Ok(Some(v)),
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None), $("// None => StopIteration")
                                        Poll::Failed(e) => return Err(err(e)), $("// raises on failure")
                                    }
                                })
                            }
                            fn __aiter__(slf: PyRef<$("'_"), Self>) -> PyRef<$("'_"), Self> {
                                slf
                            }
                            $("// Async-iterable surface. `future_into_py` bridges the tokio future")
                            $("// onto the running asyncio loop (needs the consumer's")
                            $("// `pyo3-async-runtimes` tokio runtime — the pyo3 analogue of napi's")
                            $("// `tokio_rt`). The blocking poll is driven off the loop via")
                            $("// `spawn_blocking`, so the asyncio event loop is never blocked.")
                            fn __anext__<$("'p")>(&self, py: Python<$("'p")>) -> PyResult<Bound<$("'p"), PyAny>> {
                                let stream = self.stream.clone();
                                pyo3_async_runtimes::tokio::future_into_py(py, async move {
                                    loop {
                                        let s = stream.clone();
                                        let poll = tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        $("// throw-mode: a mid-stream failure REJECTS the awaited pull.")
                                        match poll {
                                            Poll::Item(v) => return Ok(v),
                                            Poll::Idle => continue,
                                            Poll::Closed => return Err(PyStopAsyncIteration::new_err(())),
                                            Poll::Failed(e) => return Err(err(e)),
                                        }
                                    }
                                })
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator. PyO3 has no async-generator")
                        $("// `complete()` hook (unlike napi), so `Drop` is the only cancellation seam.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }
                        $['\n']
                    };
                }
                // ── OPT-IN event-mode (@streamError): error-as-event ──
                Some(se) => {
                    let err_evt = format!("{class}ErrorEvent");
                    class_names.push(err_evt.clone());
                    // each field: a `#[pyo3(name = …)]` attr only when the python
                    // getter name diverges from the rust ident (the tag always
                    // needs one — `type_` never equals its python name), mirroring
                    // node's `ev_field` js_name logic and the python DTO idiom.
                    let ev_field = |rust: &str, py: &str| {
                        if py == rust {
                            format!("pub {rust}: String,")
                        } else {
                            format!("#[pyo3(name = {py:?})] pub {rust}: String,")
                        }
                    };
                    let ev_fields: Vec<String> = vec![
                        ev_field("type_", &se.tag_name),
                        ev_field("reason", &se.reason_name),
                        ev_field("error", &se.error_name),
                    ];
                    let tag_value = format!("{:?}", se.tag_value);
                    quote_in! { t =>
                        $['\r']
                        $(format!("/// The terminal error event yielded (NEVER raised) when `{}.{}`'s core stream", i.name, op.name))
                        $("/// fails after it has started — the opt-in `@streamError` (error-as-event)")
                        $("/// model. A read-only carrier for a `Poll::Failed`; normal typed error")
                        $("/// variants ride out through `Poll::Item` and need no such struct.")
                        #[pyclass(get_all)]
                        #[derive(Clone)]
                        pub struct $(&err_evt) {
                            $(for f in &ev_fields join ($['\r']) => $f)
                        }
                        $(format!("/// Event stream from `{}.{}`, dressed as a Python async-iterable.", i.name, op.name))
                        $("///")
                        $("/// Primary surface: `async for ev in stream` (`__aiter__`/`__anext__`),")
                        $("/// driven off the asyncio loop. Retained surface: the sync iterator")
                        $("/// (`__iter__`/`__next__`) poll cursor for consumers not on asyncio.")
                        $("///")
                        $("/// `@streamError` error model = error-AS-EVENT: a mid-stream core failure")
                        $(format!("/// is yielded as a terminal `{err_evt}` and the stream then completes —"))
                        $("/// it NEVER raises. A started stream never restarts, so once the event has")
                        $("/// been handed out the `closed` latch makes every later pull end the stream.")
                        #[pyclass]
                        pub struct $(&class) {
                            $("// Arc, not Box: the async future is `'static` and moves the handle")
                            $("// across `.await` / `spawn_blocking`.")
                            stream: Arc<dyn PollStream<$(&item)>>,
                            $("// latched once the terminal error event is handed out — a started")
                            $("// stream never restarts, so every subsequent pull ends the stream.")
                            closed: Arc<std::sync::atomic::AtomicBool>,
                        }
                        #[pymethods]
                        impl $(&class) {
                            fn __iter__(slf: PyRef<$("'_"), Self>) -> PyRef<$("'_"), Self> {
                                slf
                            }
                            $("// Retained SYNC poll cursor. event-mode: a mid-stream failure is")
                            $(format!("// handed out AS the terminal `{err_evt}` value (never raised), then the"))
                            $("// latch makes the next call end the stream. The heterogeneous yield")
                            $("// (item | error-event) erases to a Python object.")
                            fn __next__(&self, py: Python<$("'_")>) -> PyResult<Option<Py<PyAny>>> {
                                use std::sync::atomic::Ordering;
                                if self.closed.load(Ordering::SeqCst) {
                                    return Ok(None); $("// None => StopIteration")
                                }
                                loop {
                                    let poll = py.detach(|| self.stream.poll(Duration::from_millis(500)));
                                    match poll {
                                        Poll::Item(v) => {
                                            return Ok(Some(v.into_pyobject(py).map_err(err)?.into_any().unbind()));
                                        }
                                        Poll::Idle => continue,
                                        Poll::Closed => return Ok(None),
                                        Poll::Failed(e) => {
                                            self.closed.store(true, Ordering::SeqCst);
                                            let ev = $(&err_evt) {
                                                type_: $(&tag_value).into(),
                                                reason: "error".into(),
                                                error: e,
                                            };
                                            return Ok(Some(ev.into_pyobject(py).map_err(err)?.into_any().unbind()));
                                        }
                                    }
                                }
                            }
                            fn __aiter__(slf: PyRef<$("'_"), Self>) -> PyRef<$("'_"), Self> {
                                slf
                            }
                            $("// Async-iterable surface. `future_into_py` bridges the tokio future")
                            $("// onto the running asyncio loop (needs the consumer's")
                            $("// `pyo3-async-runtimes` tokio runtime — the pyo3 analogue of napi's")
                            $("// `tokio_rt`). event-mode: a mid-stream failure is ENCODED IN THE")
                            $(format!("// STREAM as a terminal `{err_evt}` and NEVER raises; the future resolves"))
                            $("// to a `Py<PyAny>` because the item and the error event are distinct types.")
                            fn __anext__<$("'p")>(&self, py: Python<$("'p")>) -> PyResult<Bound<$("'p"), PyAny>> {
                                let stream = self.stream.clone();
                                let closed = self.closed.clone();
                                pyo3_async_runtimes::tokio::future_into_py(py, async move {
                                    use std::sync::atomic::Ordering;
                                    $("// A started stream never restarts: once the terminal error event")
                                    $("// has been handed out the latch is set, so every later pull ends.")
                                    if closed.load(Ordering::SeqCst) {
                                        return Err(PyStopAsyncIteration::new_err(()));
                                    }
                                    loop {
                                        let s = stream.clone();
                                        let poll = tokio::task::spawn_blocking(move || {
                                            s.poll(Duration::from_millis(500))
                                        })
                                        .await
                                        .map_err(err)?;
                                        match poll {
                                            Poll::Item(v) => {
                                                return Python::attach(|py| {
                                                    Ok(v.into_pyobject(py).map_err(err)?.into_any().unbind())
                                                });
                                            }
                                            Poll::Idle => continue,
                                            Poll::Closed => return Err(PyStopAsyncIteration::new_err(())),
                                            Poll::Failed(e) => {
                                                $("// latch closed so the next pull ends the stream, then hand the")
                                                $("// failure out AS A VALUE — never a raised exception.")
                                                closed.store(true, Ordering::SeqCst);
                                                return Python::attach(|py| {
                                                    let ev = $(&err_evt) {
                                                        type_: $(&tag_value).into(),
                                                        reason: "error".into(),
                                                        error: e,
                                                    };
                                                    Ok(ev.into_pyobject(py).map_err(err)?.into_any().unbind())
                                                });
                                            }
                                        }
                                    }
                                })
                            }
                        }

                        $("// Backstop: guarantee core-side close even if the consumer neither")
                        $("// exhausts nor cancels the iterator. PyO3 has no async-generator")
                        $("// `complete()` hook (unlike napi), so `Drop` is the only cancellation seam.")
                        impl Drop for $(&class) {
                            fn drop(&mut self) {
                                self.stream.close();
                            }
                        }
                        $['\n']
                    };
                }
            }
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
                let (ret, _) = python_ty(api, opts, &op.returns);
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
                    Shape::Unary => {
                        // An op export-name pin lands as `#[pyo3(name = "…")]`
                        // (the Rust fn ident stays snake); un-pinned ⇒ no attr,
                        // byte-identical. A python unary op is ALREADY a plain
                        // synchronous method (`py.detach` releases the GIL for the
                        // blocking core call) — the async/sync default is a
                        // node-only projection — so the only default-inversion
                        // effect here is INFALLIBILITY: a bare-`T` core drops the
                        // `PyResult`/raise seam entirely.
                        if let Some(nm) = pinned_name(&op.bindings, LANG) {
                            quote_in! { methods => $['\r']$(format!("#[pyo3(name = {nm:?})]")) };
                        }
                        if op.infallible {
                            quote_in! { methods =>
                                $['\r']
                                #[pyo3(signature = ($(&signature)))]
                                fn $(&name)(&self, py: Python<$("'_")>, $(&fn_params)) -> $(&ret) {
                                    $prelude
                                    let core = self.core.clone();
                                    py.detach(move || core.$(&name)($(&args)))
                                }
                            }
                        } else {
                            quote_in! { methods =>
                                $['\r']
                                #[pyo3(signature = ($(&signature)))]
                                fn $(&name)(&self, py: Python<$("'_")>, $(&fn_params)) -> PyResult<$(&ret)> {
                                    $prelude
                                    let core = self.core.clone();
                                    py.detach(move || core.$(&name)($(&args))).map_err(err)
                                }
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
                            $("// RAISES on a core Err — independent of the stream's error model.")
                            #[pyo3(signature = ($(&signature)))]
                            fn $(&name)(&self, $(&fn_params)) -> PyResult<$(&class)> {
                                $prelude
                                Ok($(&class) {
                                    stream: Arc::from(self.core.$(&name)($(&args)).map_err(err)?),
                                    $closed_init
                                })
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
                let (ret, _) = python_ty(api, opts, &op.returns);
                if let Some(doc) = &op.doc {
                    for line in doc.lines() {
                        quote_in! { t => $['\r']$(format!("/// {line}")) };
                    }
                }
                // op export-name pin ⇒ `#[pyo3(name = "…")]` (after `#[pyfunction]`);
                // infallible ⇒ drop the `PyResult`/raise seam (see the method arm).
                quote_in! { t => $['\r'] #[pyfunction] };
                if let Some(nm) = pinned_name(&op.bindings, LANG) {
                    quote_in! { t => $['\r']$(format!("#[pyo3(name = {nm:?})]")) };
                }
                if op.infallible {
                    quote_in! { t =>
                        $['\r']
                        #[pyo3(signature = ($(&signature)))]
                        fn $(&name)(py: Python<$("'_")>, $(&fn_params)) -> $(&ret) {
                            $prelude
                            py.detach(move || <$(&impl_path) as $(&trait_name)>::$(&name)($(&args)))
                        }
                        $['\n']
                    };
                } else {
                    quote_in! { t =>
                        $['\r']
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
