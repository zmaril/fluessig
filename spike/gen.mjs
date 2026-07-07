// straitjacket-allow-file:duplication — frozen spike (the mechanism demo).
// api.json -> generated Rust binding skeletons for napi (Node), PyO3 (Python), Magnus (Ruby),
// plus the core trait the bindings call into.
//
// THE THESIS: every op has a SHAPE (ctor | unary | stream | manual). The per-language idiom —
// AsyncTask->Promise, py.allow_threads, GVL-plain, and the poll-stream dressed as an async
// iterator / __iter__ / Enumerator-ish next() — lives in ONE hand-written template per
// (language x shape). The generator applies those templates mechanically, so the idiom is
// preserved BY CONSTRUCTION and the boilerplate (N ops x M langs) collapses to (shapes x langs)
// templates written once. @manual ops are skipped: the escape hatch for anything truly bespoke.
//
// Templates below deliberately mirror the real hand-written patterns in
// crates/entl-node/src/lib.rs, crates/entl-python/src/lib.rs, crates/entl-ruby/src/lib.rs.
import { readFileSync, writeFileSync, mkdirSync } from "fs";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";

const dir = dirname(fileURLToPath(import.meta.url));
const api = JSON.parse(readFileSync(resolve(dir, "api.json"), "utf8"));
mkdirSync(resolve(dir, "generated"), { recursive: true });

const snake = (s) => s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
const pascal = (s) => s[0].toUpperCase() + s.slice(1);

// tsp type -> rust type per surface. Models pass by generated per-language struct.
function rust(t) {
  if (typeof t === "object") return t.model;
  return { string: "String", int64: "i64", int32: "i32", boolean: "bool", float64: "f64", void: "()" }[t];
}
function ts(t) {
  if (typeof t === "object") return t.model;
  return { string: "string", int64: "number", int32: "number", boolean: "boolean", float64: "number", void: "void" }[t];
}
const params = (op, f = (p) => `${snake(p.name)}: ${rust(p.type)}`) => op.params.map(f).join(", ");
const args = (op, f = (p) => snake(p.name)) => op.params.map(f).join(", ");
// core methods take &str for String params
const coreArgs = (op) => op.params.map((p) => (p.type === "string" ? `&self.${snake(p.name)}` : `self.${snake(p.name)}`)).join(", ");
const coreArgsDirect = (op) => op.params.map((p) => (p.type === "string" ? `&${snake(p.name)}` : snake(p.name))).join(", ");

// ---------------------------------------------------------------- core.rs (the contract)
function genCore() {
  let out = `// straitjacket-allow-file:duplication (generated)\n// GENERATED — the contract the bindings call into. Hand-implement this over entl-core.
use std::time::Duration;

`;
  for (const m of api.models) {
    if (m.doc) out += `/// ${m.doc}\n`;
    out += `#[derive(Clone, Debug)]\npub struct ${m.name} {\n`;
    for (const f of m.fields) out += `    pub ${snake(f.name)}: ${rust(f.type)},\n`;
    out += `}\n\n`;
  }
  out += `pub enum Poll<T> { Item(T), Idle, Closed }

/// The one sync primitive every stream shape dresses (entl's ChangeStream::poll).
pub trait PollStream<T>: Send + Sync {
    fn poll(&self, timeout: Duration) -> Poll<T>;
}

`;
  for (const i of api.interfaces) {
    out += `pub trait ${i.name}Core: Send + Sync + Sized + 'static {\n`;
    for (const op of i.ops) {
      const ps = op.params.map((p) => `${snake(p.name)}: ${p.type === "string" ? "&str" : rust(p.type)}`).join(", ");
      if (op.shape === "ctor") out += `    fn ${snake(op.name)}(${ps}) -> anyhow::Result<Self>;\n`;
      else if (op.shape === "stream")
        out += `    fn ${snake(op.name)}(&self, ${ps}) -> anyhow::Result<Box<dyn PollStream<${rust(op.returns)}>>>;\n`;
      else if (op.shape === "unary")
        out += `    fn ${snake(op.name)}(&self, ${ps}) -> anyhow::Result<${rust(op.returns)}>;\n`;
      // manual ops: not part of the generated contract
    }
    out += `}\n`;
  }
  return out;
}

// ---------------------------------------------------------------- node.rs (napi-rs)
function genNode() {
  let out = `// straitjacket-allow-file:duplication (generated)\n// GENERATED — napi binding skeleton. Mirrors the hand-written patterns of entl-node.
use std::sync::Arc;
use std::time::Duration;
use napi::bindgen_prelude::{AsyncTask, Result};
use napi::{Env, Task};
use napi_derive::napi;
use crate::core::{self, Poll, PollStream};
${api.interfaces.map((i) => `use crate::core::${i.name}Core;`).join("\n")}

fn err(e: impl std::fmt::Display) -> napi::Error { napi::Error::from_reason(e.to_string()) }

`;
  for (const m of api.models) {
    if (m.doc) out += `/// ${m.doc}\n`;
    out += `#[napi(object)]\npub struct ${m.name} {\n`;
    for (const f of m.fields) out += `    pub ${snake(f.name)}: ${rust(f.type)},\n`;
    out += `}\nimpl From<core::${m.name}> for ${m.name} {\n    fn from(v: core::${m.name}) -> Self {\n        Self { ${m.fields.map((f) => `${snake(f.name)}: v.${snake(f.name)}`).join(", ")} }\n    }\n}\n\n`;
  }
  for (const i of api.interfaces) {
    const core = `core::Impl`; // the crate aliases its EntlCore impl as core::Impl
    // per-op AsyncTasks (unary) + stream classes
    for (const op of i.ops) {
      if (op.shape === "unary") {
        const T = pascal(op.name) + "Task";
        const R = rust(op.returns);
        out += `pub struct ${T} { core: Arc<${core}>, ${params(op)} }
impl Task for ${T} {
    type Output = ${R};
    type JsValue = ${R};
    fn compute(&mut self) -> Result<Self::Output> {
        self.core.${snake(op.name)}(${coreArgs(op)}).map(Into::into).map_err(err)
    }
    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> { Ok(o) }
}
`;
      }
      if (op.shape === "stream") {
        const S = pascal(op.name); // stream handle class
        const R = rust(op.returns);
        out += `/// Poll-based stream dressed as \`next(): Promise<${ts(op.returns)} | null>\` — wrap with an async iterator in JS.
#[napi]
pub struct ${S} { stream: Arc<dyn PollStream<core::${R}>> }
pub struct Next${S}Task { stream: Arc<dyn PollStream<core::${R}>> }
impl Task for Next${S}Task {
    type Output = Option<${R}>;
    type JsValue = Option<${R}>;
    fn compute(&mut self) -> Result<Self::Output> {
        loop {
            match self.stream.poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Ok(Some(b.into())),
                Poll::Idle => continue,
                Poll::Closed => return Ok(None),
            }
        }
    }
    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> { Ok(o) }
}
#[napi]
impl ${S} {
    #[napi(ts_return_type = "Promise<${ts(op.returns)} | null>")]
    pub fn next(&self) -> AsyncTask<Next${S}Task> {
        AsyncTask::new(Next${S}Task { stream: self.stream.clone() })
    }
}
`;
      }
    }
    // the handle class
    if (i.doc) out += `/// ${i.doc}\n`;
    out += `#[napi]\npub struct ${i.name} { core: Arc<${core}> }\n\n#[napi]\nimpl ${i.name} {\n`;
    for (const op of i.ops) {
      if (op.doc) out += `    /// ${op.doc}\n`;
      if (op.shape === "ctor") {
        out += `    #[napi(constructor)]
    pub fn new(${params(op)}) -> Result<Self> {
        Ok(Self { core: Arc::new(${core}::${snake(op.name)}(${coreArgsDirect(op)}).map_err(err)?) })
    }
`;
      } else if (op.shape === "unary") {
        out += `    #[napi(ts_return_type = "Promise<${ts(op.returns)}>")]
    pub fn ${snake(op.name)}(&self, ${params(op)}) -> Result<AsyncTask<${pascal(op.name)}Task>> {
        Ok(AsyncTask::new(${pascal(op.name)}Task { core: self.core.clone(), ${args(op)} }))
    }
`;
      } else if (op.shape === "stream") {
        out += `    #[napi]
    pub fn ${snake(op.name)}(&self, ${params(op)}) -> Result<${pascal(op.name)}> {
        let stream = self.core.${snake(op.name)}(${coreArgsDirect(op)}).map_err(err)?;
        Ok(${pascal(op.name)} { stream: Arc::from(stream) })
    }
`;
      } else if (op.shape === "manual") {
        out += `    // @manual: ${op.name} — hand-written elsewhere.\n`;
      }
    }
    out += `}\n`;
  }
  return out;
}

// ---------------------------------------------------------------- python.rs (PyO3)
function genPython() {
  let out = `// straitjacket-allow-file:duplication (generated)\n// GENERATED — PyO3 binding skeleton. Mirrors the hand-written patterns of entl-python.
use std::sync::Arc;
use std::time::Duration;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use crate::core::{self, Poll, PollStream};
${api.interfaces.map((i) => `use crate::core::${i.name}Core;`).join("\n")}

fn pyerr(e: impl std::fmt::Display) -> PyErr { PyRuntimeError::new_err(e.to_string()) }

`;
  for (const m of api.models) {
    if (m.doc) out += `/// ${m.doc}\n`;
    out += `#[pyclass(get_all, frozen)]\n#[derive(Clone)]\npub struct ${m.name} {\n`;
    for (const f of m.fields) out += `    pub ${snake(f.name)}: ${rust(f.type)},\n`;
    out += `}\nimpl From<core::${m.name}> for ${m.name} {\n    fn from(v: core::${m.name}) -> Self {\n        Self { ${m.fields.map((f) => `${snake(f.name)}: v.${snake(f.name)}`).join(", ")} }\n    }\n}\n\n`;
  }
  for (const i of api.interfaces) {
    const core = `core::Impl`;
    // stream classes: __iter__/__next__ over poll, GIL released while blocking
    for (const op of i.ops) {
      if (op.shape !== "stream") continue;
      const S = pascal(op.name);
      out += `/// Poll-based stream dressed as a Python iterator (\`for batch in entl.${snake(op.name)}(...)\`).
#[pyclass(unsendable)]
pub struct ${S} { stream: Box<dyn PollStream<core::${rust(op.returns)}>> }
#[pymethods]
impl ${S} {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> { slf }
    fn __next__(&self, py: Python<'_>) -> Option<${rust(op.returns)}> {
        py.allow_threads(|| loop {
            match self.stream.poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Some(b.into()),
                Poll::Idle => continue,
                Poll::Closed => return None,   // None => StopIteration
            }
        })
    }
}
`;
    }
    if (i.doc) out += `/// ${i.doc}\n`;
    out += `#[pyclass]\npub struct ${i.name} { core: Arc<${core}> }\n\n#[pymethods]\nimpl ${i.name} {\n`;
    for (const op of i.ops) {
      if (op.doc) out += `    /// ${op.doc}\n`;
      if (op.shape === "ctor") {
        out += `    #[new]
    fn new(${params(op)}) -> PyResult<Self> {
        Ok(Self { core: Arc::new(${core}::${snake(op.name)}(${coreArgsDirect(op)}).map_err(pyerr)?) })
    }
`;
      } else if (op.shape === "unary") {
        out += `    fn ${snake(op.name)}(&self, py: Python<'_>, ${params(op)}) -> PyResult<${rust(op.returns)}> {
        let core = self.core.clone();
        py.allow_threads(move || core.${snake(op.name)}(${coreArgsDirect(op)}))
            .map(Into::into).map_err(pyerr)
    }
`;
      } else if (op.shape === "stream") {
        out += `    fn ${snake(op.name)}(&self, ${params(op)}) -> PyResult<${pascal(op.name)}> {
        Ok(${pascal(op.name)} { stream: self.core.${snake(op.name)}(${coreArgsDirect(op)}).map_err(pyerr)? })
    }
`;
      } else if (op.shape === "manual") {
        out += `    // @manual: ${op.name} — hand-written elsewhere.\n`;
      }
    }
    out += `}\n`;
  }
  return out;
}

// ---------------------------------------------------------------- ruby.rs (Magnus)
function genRuby() {
  let out = `// straitjacket-allow-file:duplication (generated)\n// GENERATED — Magnus binding skeleton. Mirrors the hand-written patterns of entl-ruby.
// Ruby's GVL serialises access; unary calls run inline (nogvl is a per-op opt-in later).
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;
use magnus::{function, method, prelude::*, Error, RHash, Ruby};
use crate::core::{self, Poll, PollStream};
${api.interfaces.map((i) => `use crate::core::${i.name}Core;`).join("\n")}

fn rberr(e: impl std::fmt::Display) -> Error {
    let ruby = Ruby::get().expect("called outside the Ruby GVL");
    Error::new(ruby.exception_runtime_error(), e.to_string())
}

`;
  // struct -> RHash converters
  for (const m of api.models) {
    out += `fn ${snake(m.name)}_hash(ruby: &Ruby, v: core::${m.name}) -> Result<RHash, Error> {
    let h = ruby.hash_new();
${m.fields.map((f) => `    h.aset("${snake(f.name)}", v.${snake(f.name)})?;`).join("\n")}
    Ok(h)
}

`;
  }
  for (const i of api.interfaces) {
    const core = `core::Impl`;
    for (const op of i.ops) {
      if (op.shape !== "stream") continue;
      const S = pascal(op.name);
      out += `/// Poll-based stream dressed as \`.next\` (nil at end) — wrap with an Enumerator in Ruby.
#[magnus::wrap(class = "${i.name}::${S}", free_immediately, size)]
pub struct ${S} { stream: RefCell<Box<dyn PollStream<core::${rust(op.returns)}>>> }
impl ${S} {
    fn next(ruby: &Ruby, rb_self: &Self) -> Result<Option<RHash>, Error> {
        loop {
            match rb_self.stream.borrow().poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Ok(Some(${snake(rust(op.returns))}_hash(ruby, b)?)),
                Poll::Idle => continue,
                Poll::Closed => return Ok(None),
            }
        }
    }
}
`;
    }
    if (i.doc) out += `/// ${i.doc}\n`;
    out += `#[magnus::wrap(class = "${i.name}", free_immediately, size)]\npub struct ${i.name} { core: Arc<${core}> }\n\nimpl ${i.name} {\n`;
    for (const op of i.ops) {
      if (op.shape === "ctor") {
        out += `    fn new(${params(op)}) -> Result<Self, Error> {
        Ok(Self { core: Arc::new(${core}::${snake(op.name)}(${coreArgsDirect(op)}).map_err(rberr)?) })
    }
`;
      } else if (op.shape === "unary") {
        const retModel = typeof op.returns === "object";
        if (retModel) {
          out += `    fn ${snake(op.name)}(ruby: &Ruby, rb_self: &Self, ${params(op)}) -> Result<RHash, Error> {
        let v = rb_self.core.${snake(op.name)}(${coreArgsDirect(op)}).map_err(rberr)?;
        ${snake(rust(op.returns))}_hash(ruby, v)
    }
`;
        } else {
          out += `    fn ${snake(op.name)}(&self, ${params(op)}) -> Result<${rust(op.returns)}, Error> {
        self.core.${snake(op.name)}(${coreArgsDirect(op)}).map_err(rberr)
    }
`;
        }
      } else if (op.shape === "stream") {
        out += `    fn ${snake(op.name)}(&self, ${params(op)}) -> Result<${pascal(op.name)}, Error> {
        let stream = self.core.${snake(op.name)}(${coreArgsDirect(op)}).map_err(rberr)?;
        Ok(${pascal(op.name)} { stream: RefCell::new(stream) })
    }
`;
      } else if (op.shape === "manual") {
        out += `    // @manual: ${op.name} — hand-written elsewhere.\n`;
      }
    }
    out += `}\n\n`;
    // init registration
    out += `pub fn register(ruby: &Ruby) -> Result<(), Error> {
    let class = ruby.define_class("${i.name}", ruby.class_object())?;
`;
    for (const op of i.ops) {
      if (op.shape === "ctor")
        out += `    class.define_singleton_method("new", function!(${i.name}::new, ${op.params.length}))?;\n`;
      else if (op.shape === "unary" && typeof op.returns === "object")
        out += `    class.define_method("${snake(op.name)}", method!(${i.name}::${snake(op.name)}, ${op.params.length}))?;\n`;
      else if (op.shape === "unary")
        out += `    class.define_method("${snake(op.name)}", method!(${i.name}::${snake(op.name)}, ${op.params.length}))?;\n`;
      else if (op.shape === "stream") {
        const S = pascal(op.name);
        out += `    class.define_method("${snake(op.name)}", method!(${i.name}::${snake(op.name)}, ${op.params.length}))?;
    let s = class.define_class("${S}", ruby.class_object())?;
    s.define_method("next", method!(${S}::next, 0))?;\n`;
      }
    }
    out += `    Ok(())
}
`;
  }
  return out;
}

writeFileSync(resolve(dir, "generated/core.rs"), genCore());
writeFileSync(resolve(dir, "generated/node.rs"), genNode());
writeFileSync(resolve(dir, "generated/python.rs"), genPython());
writeFileSync(resolve(dir, "generated/ruby.rs"), genRuby());
console.log("wrote generated/{core,node,python,ruby}.rs");
