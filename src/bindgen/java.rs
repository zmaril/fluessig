//! The JNI (Java) template grid — one language's projection of the op shapes.
//!
//! Unlike node/python/ruby/php — whose macro frameworks (napi / PyO3 / Magnus /
//! ext-php-rs) expose the language surface from Rust at runtime — a JNI backend
//! needs TWO artifacts: the Rust JNI GLUE (`#[no_mangle] extern "system"`
//! functions the JVM resolves, routing to `crate::core_impl`) and the JAVA
//! SOURCE (the `.java` classes with `native` declarations). [`java_binding`]
//! emits the first, [`java_sources`] the second.
//!
//! JNI is the Rust-side counterpart to napi/pyo3/magnus/ext-php-rs: synchronous
//! by nature (matching the sync-default op model and the php/ruby precedent),
//! and it lets Rust construct Java objects directly for DTOs and stream cursors.
//! An `#[fluessig(async)]` op maps the honest Java way: the native call stays
//! blocking and the Java class wraps it in `CompletableFuture.supplyAsync(...)`
//! — the Java parallel to node's `Promise`, no Rust threadpool needed.
//!
//! straitjacket-allow-file:duplication — the per-language generators are
//! DELIBERATELY parallel: the (language × shape) template grid is the design
//! (see /translation.md); the truly shared pieces live in the parent module.

use crate::api::{ApiDoc, ApiField, ApiOp, ApiType, Shape};

use super::*;

/// This backend's language slug — the key it reads out of every symbol's
/// `bindings` map through the shared [`pinned_name`] / [`variant_token`]
/// resolver. Java hardcodes no pin; it owns only its own default casing
/// (lowerCamelCase methods, PascalCase classes) and its rename lever (a pinned
/// name is used verbatim as the emitted Java identifier).
const LANG: &str = "java";

/// The generated Java package. Single-segment so the JNI symbol mangling
/// (`Java_<pkg>_<Class>_<method>`) stays free of `_1` escapes — the Rust glue's
/// `#[no_mangle]` names line up with `javac -h` byte-for-byte.
pub(super) const PACKAGE: &str = "fluessig";

/// The shared library name every generated interface class loads
/// (`System.loadLibrary("fluessig")`) — the cdylib the Rust glue compiles to.
pub(super) const LIB: &str = "fluessig";

// ── identifier casing ────────────────────────────────────────────────────────

/// lowerCamelCase for a Java method / field ident (`pull_request_count` →
/// `pullRequestCount`). Derived from the shared [`snake`] so the casing rule is
/// the single source, then re-camel'd — an already-camel op name round-trips.
fn camel(s: &str) -> String {
    let sn = snake(s);
    let mut out = String::new();
    for (i, part) in sn.split('_').filter(|p| !p.is_empty()).enumerate() {
        if i == 0 {
            out.push_str(part);
        } else {
            let mut c = part.chars();
            if let Some(f) = c.next() {
                out.push(f.to_ascii_uppercase());
                out.push_str(c.as_str());
            }
        }
    }
    out
}

/// The Java method name for an op: a `java` export-name pin wins, else the
/// default lowerCamelCase rule.
fn op_jname(op: &ApiOp) -> String {
    pinned_name(&op.bindings, LANG).unwrap_or_else(|| camel(&op.name))
}

/// The Java field / getter base name for a model field: a `java` pin wins, else
/// lowerCamelCase.
fn field_jname(f: &ApiField) -> String {
    pinned_name(&f.bindings, LANG).unwrap_or_else(|| camel(&f.name))
}

/// `sessionUid` → `getSessionUid` (the JavaBean getter the DTO class exposes and
/// the `_from_j` reader calls).
fn getter(jname: &str) -> String {
    let mut c = jname.chars();
    match c.next() {
        Some(f) => format!("get{}{}", f.to_ascii_uppercase(), c.as_str()),
        None => "get".to_string(),
    }
}

// ── the type effective-shape helper ──────────────────────────────────────────

/// A field's effective op-type, folding its `nullable` flag into a
/// [`ApiType::Nullable`] so one code path handles nullability everywhere.
fn field_ty(f: &ApiField) -> ApiType {
    if f.nullable {
        ApiType::Nullable {
            nullable: Box::new(f.ty.clone()),
        }
    } else {
        f.ty.clone()
    }
}

/// A param's effective op-type, folding `optional` into [`ApiType::Nullable`].
fn param_ty(p: &crate::api::ApiParam) -> ApiType {
    if p.optional == Some(true) {
        ApiType::Nullable {
            nullable: Box::new(p.ty.clone()),
        }
    } else {
        p.ty.clone()
    }
}

/// Is this a JVM reference type (crosses as a `JObject`/`jobject`), vs a JNI
/// primitive (`jint`/`jlong`/`jdouble`/`jboolean`)? `void` is neither.
fn is_object(t: &ApiType) -> bool {
    match t {
        ApiType::Scalar(s) => !matches!(
            s.as_str(),
            "boolean"
                | "int32"
                | "int64"
                | "uint8"
                | "uint16"
                | "uint32"
                | "float32"
                | "float64"
                | "void"
        ),
        _ => true,
    }
}

// ── Java-visible type spelling (for the .java surface) ────────────────────────

/// The boxed Java type for a slot that must hold a reference (a `List<T>`
/// element, or a nullable value): `int` → `Integer`, else the plain spelling.
pub(super) fn java_boxed(api: &ApiDoc, t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => "Boolean",
            // uint8/uint16 fit a signed `int` losslessly; uint32's full range
            // needs `long` (JNI has no unsigned primitives).
            "int32" | "uint8" | "uint16" => "Integer",
            "int64" | "uint32" => "Long",
            "float32" => "Float",
            "float64" => "Double",
            "void" => "Void",
            _ => return java_ty(api, t),
        }
        .to_string(),
        _ => java_ty(api, t),
    }
}

/// The Java-visible type name for an [`ApiType`]. Enums and unions cross the JNI
/// seam as their wire / JSON-envelope `String` (the standalone `enum` / envelope
/// classes are emitted as consumer artifacts; the marshalled surface uses
/// `String`), so both spell `String` here — mirroring the other backends'
/// envelope carrier and keeping the JNI marshalling bounded.
fn java_ty(api: &ApiDoc, t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "string" | "Json" => "String",
            "boolean" => "boolean",
            "int32" | "uint8" | "uint16" => "int",
            "int64" | "uint32" => "long",
            "float32" => "float",
            "float64" => "double",
            "bytes" | "ArrowBatch" => "byte[]",
            "void" => "void",
            _ => "String",
        }
        .to_string(),
        ApiType::Model { model } => model.clone(),
        ApiType::Enum { .. } => "String".to_string(),
        ApiType::List { list } => format!("List<{}>", java_boxed(api, list)),
        ApiType::Nullable { nullable } => java_boxed(api, nullable),
        // A callback param is the Java functional interface the host supplies
        // (a `Consumer<Boxed>` for the one-arg forward-only shape); the Rust glue
        // wraps it into the uniform core `Box<dyn Fn(..)>`.
        ApiType::Callback { callback } => super::java_callback::callback_java_type(api, callback),
        ApiType::Union { .. } | ApiType::Foreign { .. } => "String".to_string(),
    }
}

/// The JVM type descriptor for an [`ApiType`] (`int` → `I`, `String` →
/// `Ljava/lang/String;`, a model → `Lfluessig/Repo;`) — what the Rust glue
/// passes to `new_object` / `call_method`. A nullable prim boxes to its wrapper
/// class descriptor.
fn descriptor(t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => "Z",
            "int32" | "uint8" | "uint16" => "I",
            "int64" | "uint32" => "J",
            "float32" => "F",
            "float64" => "D",
            "bytes" | "ArrowBatch" => "[B",
            "void" => "V",
            _ => "Ljava/lang/String;",
        }
        .to_string(),
        ApiType::Model { model } => format!("L{PACKAGE}/{model};"),
        ApiType::Enum { .. } => "Ljava/lang/String;".to_string(),
        ApiType::List { .. } => "Ljava/util/List;".to_string(),
        // A callback param crosses as the `java.util.function.Consumer` object.
        ApiType::Callback { .. } => "Ljava/util/function/Consumer;".to_string(),
        ApiType::Union { .. } | ApiType::Foreign { .. } => "Ljava/lang/String;".to_string(),
        ApiType::Nullable { nullable } => match &**nullable {
            ApiType::Scalar(s) => match s.as_str() {
                "boolean" => "Ljava/lang/Boolean;",
                "int32" | "uint8" | "uint16" => "Ljava/lang/Integer;",
                "int64" | "uint32" => "Ljava/lang/Long;",
                "float32" => "Ljava/lang/Float;",
                "float64" => "Ljava/lang/Double;",
                _ => return descriptor(nullable),
            }
            .to_string(),
            _ => descriptor(nullable),
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// (a) the Rust JNI glue
// ═══════════════════════════════════════════════════════════════════════════

/// The JNI raw return type for an op's return [`ApiType`].
fn jni_ret_ty(t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => "jboolean",
            "int32" | "uint8" | "uint16" => "jint",
            "int64" | "uint32" => "jlong",
            "float32" => "jfloat",
            "float64" => "jdouble",
            "bytes" | "ArrowBatch" => "jbyteArray",
            "void" => "()",
            _ => "jstring",
        }
        .to_string(),
        ApiType::Model { .. } | ApiType::List { .. } => "jobject".to_string(),
        ApiType::Enum { .. }
        | ApiType::Union { .. }
        | ApiType::Foreign { .. }
        | ApiType::Callback { .. } => "jstring".to_string(),
        ApiType::Nullable { nullable } => {
            if is_object(nullable) {
                jni_ret_ty(nullable)
            } else {
                "jobject".to_string()
            }
        }
    }
}

/// The zero / null value a JNI fn returns on the throw path (after raising a
/// Java exception, control returns to the JVM with a default value).
fn jni_zero(t: &ApiType) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => "0",
            "int32" | "int64" | "uint8" | "uint16" | "uint32" => "0",
            "float32" | "float64" => "0.0",
            "void" => "()",
            _ => "std::ptr::null_mut()",
        }
        .to_string(),
        _ => "std::ptr::null_mut()".to_string(),
    }
}

/// A Rust expression producing a `JObject` from `expr` (of the Rust type
/// [`ty`] resolves) — used for `List` elements and nullable payloads, boxing
/// primitives into their wrapper classes.
pub(super) fn to_jobject(t: &ApiType, expr: &str) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => format!(
                "env.new_object(\"java/lang/Boolean\", \"(Z)V\", &[JValue::Bool({expr} as u8)]).expect(\"box Boolean\")"
            ),
            "int32" => format!(
                "env.new_object(\"java/lang/Integer\", \"(I)V\", &[JValue::Int({expr})]).expect(\"box Integer\")"
            ),
            "uint8" | "uint16" => format!(
                "env.new_object(\"java/lang/Integer\", \"(I)V\", &[JValue::Int({expr} as i32)]).expect(\"box Integer\")"
            ),
            "int64" => format!(
                "env.new_object(\"java/lang/Long\", \"(J)V\", &[JValue::Long({expr})]).expect(\"box Long\")"
            ),
            "uint32" => format!(
                "env.new_object(\"java/lang/Long\", \"(J)V\", &[JValue::Long({expr} as i64)]).expect(\"box Long\")"
            ),
            "float32" => format!(
                "env.new_object(\"java/lang/Float\", \"(F)V\", &[JValue::Float({expr})]).expect(\"box Float\")"
            ),
            "float64" => format!(
                "env.new_object(\"java/lang/Double\", \"(D)V\", &[JValue::Double({expr})]).expect(\"box Double\")"
            ),
            "bytes" | "ArrowBatch" => format!(
                "env.byte_array_from_slice(&{expr}).map(JObject::from).unwrap_or_default()"
            ),
            _ => format!(
                "env.new_string(&{expr}).map(JObject::from).unwrap_or_default()"
            ),
        },
        ApiType::Model { model } => format!("{model}_to_j(env, {expr})"),
        ApiType::Enum { .. } => format!(
            "env.new_string({expr}.wire()).map(JObject::from).unwrap_or_default()"
        ),
        ApiType::Union { .. } | ApiType::Foreign { .. } | ApiType::Callback { .. } => format!(
            "env.new_string(&{expr}).map(JObject::from).unwrap_or_default()"
        ),
        ApiType::List { list } => format!(
            "{{ let __l = env.new_object(\"java/util/ArrayList\", \"()V\", &[]).expect(\"ArrayList\"); \
             for __e in {expr}.into_iter() {{ let __o = {}; \
             let _ = env.call_method(&__l, \"add\", \"(Ljava/lang/Object;)Z\", &[JValue::Object(&__o)]); }} __l }}",
            to_jobject(list, "__e")
        ),
        ApiType::Nullable { nullable } => format!(
            "match {expr} {{ Some(__x) => {}, None => JObject::null() }}",
            to_jobject(nullable, "__x")
        ),
    }
}

/// A Rust expression producing the JNI raw success value for a return of type
/// `t`, given `expr` holds the (already unwrapped) Rust value.
fn success_expr(t: &ApiType, expr: &str) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => format!("({expr}) as jboolean"),
            "int32" | "uint8" | "uint16" => format!("{expr} as jint"),
            "int64" | "uint32" => format!("{expr} as jlong"),
            "float32" => format!("{expr} as jfloat"),
            "float64" => format!("{expr} as jdouble"),
            "void" => "()".to_string(),
            "bytes" | "ArrowBatch" => format!(
                "env.byte_array_from_slice(&{expr}).map(|a| a.into_raw()).unwrap_or(std::ptr::null_mut())"
            ),
            _ => format!(
                "env.new_string(&{expr}).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())"
            ),
        },
        ApiType::Model { .. } | ApiType::List { .. } => {
            format!("{}.into_raw()", to_jobject(t, expr))
        }
        ApiType::Enum { .. } => format!(
            "env.new_string({expr}.wire()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())"
        ),
        ApiType::Union { .. } | ApiType::Foreign { .. } | ApiType::Callback { .. } => format!(
            "env.new_string(&{expr}).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())"
        ),
        ApiType::Nullable { nullable } => format!(
            "match {expr} {{ Some(__x) => {}, None => {} }}",
            success_expr(nullable, "__x"),
            jni_zero(t)
        ),
    }
}

/// A Rust expression reading a value of Rust type [`ty`] from a `JObject`
/// (`obj`) — the unboxing / getter-less reader used by `List` elements and
/// nullable payloads on the Java→Rust path.
fn from_jobject(t: &ApiType, obj: &str) -> String {
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => format!(
                "env.call_method(&{obj}, \"booleanValue\", \"()Z\", &[]).unwrap().z().unwrap()"
            ),
            "int32" => format!(
                "env.call_method(&{obj}, \"intValue\", \"()I\", &[]).unwrap().i().unwrap()"
            ),
            "uint8" => format!(
                "env.call_method(&{obj}, \"intValue\", \"()I\", &[]).unwrap().i().unwrap() as u8"
            ),
            "uint16" => format!(
                "env.call_method(&{obj}, \"intValue\", \"()I\", &[]).unwrap().i().unwrap() as u16"
            ),
            "int64" => format!(
                "env.call_method(&{obj}, \"longValue\", \"()J\", &[]).unwrap().j().unwrap()"
            ),
            "uint32" => format!(
                "env.call_method(&{obj}, \"longValue\", \"()J\", &[]).unwrap().j().unwrap() as u32"
            ),
            "float32" => format!(
                "env.call_method(&{obj}, \"floatValue\", \"()F\", &[]).unwrap().f().unwrap()"
            ),
            "float64" => format!(
                "env.call_method(&{obj}, \"doubleValue\", \"()D\", &[]).unwrap().d().unwrap()"
            ),
            "bytes" | "ArrowBatch" => format!(
                "env.convert_byte_array(&JByteArray::from({obj})).unwrap_or_default()"
            ),
            _ => format!(
                "env.get_string(&JString::from({obj})).map(Into::into).unwrap_or_default()"
            ),
        },
        ApiType::Model { model } => format!("{model}_from_j(env, &{obj})"),
        ApiType::Enum { r#enum } => format!(
            "{enum}::parse(&env.get_string(&JString::from({obj})).map(Into::into).unwrap_or_default()).expect(\"enum wire token\")"
        ),
        ApiType::Union { .. } | ApiType::Foreign { .. } | ApiType::Callback { .. } => format!(
            "env.get_string(&JString::from({obj})).map(Into::into).unwrap_or_default()"
        ),
        ApiType::List { list } => format!(
            "{{ let __lo = {obj}; let __n = env.call_method(&__lo, \"size\", \"()I\", &[]).unwrap().i().unwrap(); \
             let mut __v = Vec::new(); for __i in 0..__n {{ \
             let __eo = env.call_method(&__lo, \"get\", \"(I)Ljava/lang/Object;\", &[JValue::Int(__i)]).unwrap().l().unwrap(); \
             __v.push({}); }} __v }}",
            from_jobject(list, "__eo")
        ),
        ApiType::Nullable { nullable } => format!(
            "{{ let __no = {obj}; if __no.is_null() {{ None }} else {{ Some({}) }} }}",
            from_jobject(nullable, "__no")
        ),
    }
}

/// The Rust reader for a DTO field, via its JavaBean getter on `o`.
fn read_field(t: &ApiType, getter: &str) -> String {
    let desc = descriptor(t);
    match t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => format!("env.call_method(o, \"{getter}\", \"()Z\", &[]).unwrap().z().unwrap()"),
            "int32" => format!("env.call_method(o, \"{getter}\", \"()I\", &[]).unwrap().i().unwrap()"),
            "uint8" => format!("env.call_method(o, \"{getter}\", \"()I\", &[]).unwrap().i().unwrap() as u8"),
            "uint16" => format!("env.call_method(o, \"{getter}\", \"()I\", &[]).unwrap().i().unwrap() as u16"),
            "int64" => format!("env.call_method(o, \"{getter}\", \"()J\", &[]).unwrap().j().unwrap()"),
            "uint32" => format!("env.call_method(o, \"{getter}\", \"()J\", &[]).unwrap().j().unwrap() as u32"),
            "float32" => format!("env.call_method(o, \"{getter}\", \"()F\", &[]).unwrap().f().unwrap()"),
            "float64" => format!("env.call_method(o, \"{getter}\", \"()D\", &[]).unwrap().d().unwrap()"),
            "bytes" | "ArrowBatch" => format!(
                "{{ let __a = env.call_method(o, \"{getter}\", \"()[B\", &[]).unwrap().l().unwrap(); \
                 env.convert_byte_array(&JByteArray::from(__a)).unwrap_or_default() }}"
            ),
            _ => format!(
                "{{ let __s = env.call_method(o, \"{getter}\", \"()Ljava/lang/String;\", &[]).unwrap().l().unwrap(); \
                 env.get_string(&JString::from(__s)).map(Into::into).unwrap_or_default() }}"
            ),
        },
        _ => {
            let obj = format!(
                "env.call_method(o, \"{getter}\", \"(){desc}\", &[]).unwrap().l().unwrap()"
            );
            format!("{{ let __fo = {obj}; {} }}", from_jobject(t, "__fo"))
        }
    }
}

/// Emit the two per-model marshalling helpers: `Model_to_j` (Rust value →
/// freshly-constructed Java object) and `Model_from_j` (Java object → Rust
/// value, via getters). These are the DTO seam that makes Java DTOs first-class.
fn model_marshallers(api: &ApiDoc, out: &mut String) {
    for m in &api.models {
        let ctor_desc: String = m.fields.iter().map(|f| descriptor(&field_ty(f))).collect();
        // to_j: pre-bind every object field, then new_object with the JValue args.
        let mut setups = String::new();
        let mut args: Vec<String> = Vec::new();
        for f in &m.fields {
            let ft = field_ty(f);
            let rname = snake(&f.name);
            if is_object(&ft) {
                setups.push_str(&format!(
                    "    let __f_{rname} = {};\n",
                    to_jobject(&ft, &format!("v.{rname}"))
                ));
                args.push(format!("JValue::Object(&__f_{rname})"));
            } else {
                let jv = match &ft {
                    ApiType::Scalar(s) if s == "boolean" => {
                        format!("JValue::Bool(v.{rname} as u8)")
                    }
                    ApiType::Scalar(s) if s == "int32" => format!("JValue::Int(v.{rname})"),
                    ApiType::Scalar(s) if s == "uint8" || s == "uint16" => {
                        format!("JValue::Int(v.{rname} as i32)")
                    }
                    ApiType::Scalar(s) if s == "int64" => format!("JValue::Long(v.{rname})"),
                    ApiType::Scalar(s) if s == "uint32" => {
                        format!("JValue::Long(v.{rname} as i64)")
                    }
                    ApiType::Scalar(s) if s == "float32" => format!("JValue::Float(v.{rname})"),
                    ApiType::Scalar(s) if s == "float64" => format!("JValue::Double(v.{rname})"),
                    _ => format!("JValue::Object(&__f_{rname})"),
                };
                args.push(jv);
            }
        }
        let args_joined = args.join(", ");
        out.push_str(&format!(
            "/// Construct a Java `{PACKAGE}/{name}` from its Rust value.\n\
             fn {name}_to_j<'a>(env: &mut JNIEnv<'a>, v: {name}) -> JObject<'a> {{\n{setups}    \
             let __cls = env.find_class(\"{PACKAGE}/{name}\").expect(\"find {name}\");\n    \
             env.new_object(&__cls, \"({ctor_desc})V\", &[{args_joined}]).expect(\"new {name}\")\n}}\n\n",
            name = m.name,
        ));
        // from_j: read each field via its getter.
        let mut reads = String::new();
        for f in &m.fields {
            let ft = field_ty(f);
            let rname = snake(&f.name);
            let g = getter(&field_jname(f));
            reads.push_str(&format!("    let {rname} = {};\n", read_field(&ft, &g)));
        }
        let field_list = m
            .fields
            .iter()
            .map(|f| snake(&f.name))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "/// Read a Rust `{name}` back out of a Java `{PACKAGE}/{name}`.\n\
             fn {name}_from_j<'a>(env: &mut JNIEnv<'a>, o: &JObject<'a>) -> {name} {{\n{reads}    \
             {name} {{ {field_list} }}\n}}\n\n",
            name = m.name,
        ));
    }
}

/// A JNI param declaration + its conversion-to-Rust statement, for one op param.
pub(super) fn marshal_param(api: &ApiDoc, p: &crate::api::ApiParam) -> (String, String) {
    let n = snake(&p.name);
    // A `Callback` param crosses in as a `JObject` (the Java `Consumer`) and is
    // wrapped — via a global ref + captured `JavaVM` — into the uniform core
    // `Box<dyn Fn(..) + Send + Sync>` shape node/python/cpp all target.
    if let ApiType::Callback { callback } = &p.ty {
        return (
            format!("{n}_j: JObject<'local>"),
            super::java_callback::rust_callback_conv(api, callback, &n),
        );
    }
    let t = param_ty(p);
    let (jni_ty, conv): (&str, String) = match &t {
        ApiType::Scalar(s) => match s.as_str() {
            "boolean" => ("jboolean", format!("let {n} = {n}_j != 0;")),
            "int32" => ("jint", format!("let {n} = {n}_j as i32;")),
            "uint8" => ("jint", format!("let {n} = {n}_j as u8;")),
            "uint16" => ("jint", format!("let {n} = {n}_j as u16;")),
            "int64" => ("jlong", format!("let {n} = {n}_j as i64;")),
            "uint32" => ("jlong", format!("let {n} = {n}_j as u32;")),
            "float32" => ("jfloat", format!("let {n} = {n}_j as f32;")),
            "float64" => ("jdouble", format!("let {n} = {n}_j as f64;")),
            "bytes" | "ArrowBatch" => (
                "JByteArray<'local>",
                format!("let {n} = env.convert_byte_array(&{n}_j).unwrap_or_default();"),
            ),
            _ => (
                "JString<'local>",
                format!("let {n}: String = env.get_string(&{n}_j).map(Into::into).unwrap_or_default();"),
            ),
        },
        ApiType::Enum { r#enum } => (
            "JString<'local>",
            format!(
                "let {n} = {enum}::parse(&env.get_string(&{n}_j).map(Into::into).unwrap_or_default()).expect(\"enum wire token\");"
            ),
        ),
        ApiType::Union { .. } | ApiType::Foreign { .. } | ApiType::Callback { .. } => (
            "JString<'local>",
            format!("let {n}: String = env.get_string(&{n}_j).map(Into::into).unwrap_or_default();"),
        ),
        ApiType::Model { .. } | ApiType::List { .. } | ApiType::Nullable { .. } => (
            "JObject<'local>",
            format!("let {n} = {};", from_jobject_param(api, &t, &format!("{n}_j"))),
        ),
    };
    (format!("{n}_j: {jni_ty}"), conv)
}

/// The Java→Rust reader for an object-typed op PARAM (`from_jobject` variants
/// that take the raw `JObject` param directly). Split from [`from_jobject`] only
/// so the top-level nullable/list/model param reads `&mut env` at the call site.
fn from_jobject_param(_api: &ApiDoc, t: &ApiType, obj: &str) -> String {
    match t {
        ApiType::Model { model } => format!("{model}_from_j(env, &{obj})"),
        ApiType::Nullable { nullable } => format!(
            "if {obj}.is_null() {{ None }} else {{ Some({}) }}",
            from_jobject(nullable, obj)
        ),
        _ => from_jobject(t, obj),
    }
}

/// The `Java_<pkg>_<Class>_<method>` symbol for a JNI extern fn.
pub(super) fn jni_symbol(class: &str, method: &str) -> String {
    format!("Java_{PACKAGE}_{class}_{method}")
}

/// Emit one unary op's JNI extern fn (`sync/infallible/async` all blocking at
/// the seam; async-ness is a Java-side `CompletableFuture` wrapper, so the glue
/// is identical to a fallible sync op). `receiver` is the core-call prefix.
fn emit_unary_fn(
    api: &ApiDoc,
    class: &str,
    op: &ApiOp,
    jni_method: &str,
    call_prefix: &str,
    handle_param: bool,
    out: &mut String,
) {
    let sym = jni_symbol(class, jni_method);
    let ret_ty = jni_ret_ty(&op.returns);
    let ret_clause = if ret_ty == "()" {
        String::new()
    } else {
        format!(" -> {ret_ty}")
    };
    let mut params = String::from("mut env: JNIEnv<'local>, _class: JClass<'local>");
    if handle_param {
        params.push_str(", handle: jlong");
    }
    let mut convs = String::new();
    let mut names: Vec<String> = Vec::new();
    for p in &op.params {
        let (decl, conv) = marshal_param(api, p);
        params.push_str(&format!(", {decl}"));
        convs.push_str(&format!("    {conv}\n"));
        names.push(snake(&p.name));
    }
    let _ = handle_param;
    let call = format!("{call_prefix}({})", names.join(", "));
    let body = if op.infallible {
        // no error channel: the core returns the bare value.
        format!(
            "    let __v = {call};\n    {}\n",
            success_expr(&op.returns, "__v")
        )
    } else {
        // fallible: throw a RuntimeException on Err, return the zero value.
        format!(
            "    match {call} {{\n        Ok(__v) => {},\n        Err(__e) => {{ throw(env, __e); {} }}\n    }}\n",
            success_expr(&op.returns, "__v"),
            jni_zero(&op.returns)
        )
    };
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"system\" fn {sym}<'local>({params}){ret_clause} {{\n    let env = &mut env;\n{convs}{body}}}\n\n"
    ));
}

/// Generate the Rust JNI glue file: the plain Rust DTO/enum structs, the
/// per-model `_to_j`/`_from_j` marshallers, the `<Interface>Core` traits, and
/// the `#[no_mangle] extern "system"` routing functions (ctor/init, unary,
/// stream cursor poll/free) that bridge the JVM to `crate::core_impl`.
pub fn java_binding(api: &ApiDoc, enums: &[EnumDesc], banner_note: Option<&str>) -> String {
    // A `single_threaded` interface is a thread-confined `!Send` handle — node-only
    // today; the JNI glue holds the core in a `Send`-requiring wrapper, so java
    // cannot bind a `!Send` core. Split it out (emit nothing) + append an honest
    // skip-note rather than a silent `Send`-assuming handle. None ⇒ byte-identical.
    let (api_owned, st_note) = crate::bindgen::split_single_threaded(api, "java");
    let api = &api_owned;
    let runtime_import = RUNTIME_STREAM_IMPORT.render();
    let mut body = String::new();

    // prelude
    body.push_str(&format!(
        "use std::sync::Arc;\n\
         use std::time::Duration;\n\
         use jni::objects::{{JByteArray, JClass, JObject, JString, JValue}};\n\
         use jni::sys::{{jboolean, jbyteArray, jdouble, jfloat, jint, jlong, jobject, jstring}};\n\
         use jni::JNIEnv;\n\
         {runtime_import}\n\n\
         /// A core-layer failure becomes a thrown Java `RuntimeException` (JNI is\n\
         /// synchronous; a fallible op raises and returns the JNI zero value). An\n\
         /// `#[fluessig(async)]` op wraps this same blocking call in a Java\n\
         /// `CompletableFuture`, so a rejected future rides the same seam.\n\
         fn throw(env: &mut JNIEnv, e: impl std::fmt::Display) {{\n    \
         let _ = env.throw_new(\"java/lang/RuntimeException\", e.to_string());\n}}\n\n"
    ));

    // The opaque `Subscription` handle + its `nativeUnsubscribe`/`nativeFree` JNI
    // fns, emitted once whenever the surface has a `Shape::Subscription` op —
    // gated so a subscription-free schema stays byte-identical.
    if crate::bindgen::api_uses_subscription(api) {
        body.push_str(&super::java_callback::subscription_rust_prelude());
    }

    // enums: plain Rust enum + parse/wire (Java sees the wire token as a String;
    // the standalone Java `enum` class provides fromWire/toWire).
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let vs: Vec<String> = variants.iter().map(|v| variant_ident(&v.name)).collect();
        let expect = variants
            .iter()
            .map(|v| variant_token(v, LANG))
            .collect::<Vec<_>>()
            .join(" | ");
        let parse_arms: String = variants
            .iter()
            .map(|v| {
                format!(
                    "            {:?} => Ok(Self::{}),\n",
                    variant_token(v, LANG),
                    variant_ident(&v.name)
                )
            })
            .collect();
        let wire_arms: String = variants
            .iter()
            .map(|v| {
                format!(
                    "            Self::{} => {:?},\n",
                    variant_ident(&v.name),
                    variant_token(v, LANG)
                )
            })
            .collect();
        body.push_str(&format!(
            "#[derive(Clone, Copy, PartialEq)]\npub enum {name} {{\n{}}}\n\
             impl {name} {{\n    \
             pub fn parse(s: &str) -> anyhow::Result<Self> {{\n        \
             match s.to_ascii_lowercase().as_str() {{\n{parse_arms}            \
             other => Err(anyhow::anyhow!(\"unknown {name}: {{other}} (expected {expect})\")),\n        }}\n    }}\n    \
             /// The wire token Java sees for this variant.\n    \
             pub fn wire(&self) -> &'static str {{\n        match self {{\n{wire_arms}        }}\n    }}\n}}\n\n",
            vs.iter().map(|v| format!("    {v},\n")).collect::<String>(),
        ));
    }

    // DTO models: plain Rust structs (the core returns these) + marshallers.
    for m in &api.models {
        if let Some(doc) = &m.doc {
            for line in doc.lines() {
                body.push_str(&format!("/// {line}\n"));
            }
        }
        let fields: String = m
            .fields
            .iter()
            .map(|f| {
                let (r, _) = ty(api, &f.ty);
                let r = if f.nullable {
                    format!("Option<{r}>")
                } else {
                    r
                };
                format!("    pub {}: {r},\n", snake(&f.name))
            })
            .collect();
        body.push_str(&format!(
            "#[derive(Clone)]\npub struct {} {{\n{fields}}}\n\n",
            m.name
        ));
    }
    model_marshallers(api, &mut body);

    // the shared core traits.
    {
        let mut ct: genco::lang::rust::Tokens = genco::quote!();
        emit_core_traits(&mut ct, api);
        body.push_str(&ct.to_file_string().expect("core traits render"));
        body.push('\n');
    }

    // per-interface JNI routing.
    for i in &api.interfaces {
        let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
        let trait_name = format!("{}Core", i.name);
        let impl_path = format!("crate::core_impl::{}Impl", i.name);

        // stream cursor poll/free fns — one pair per stream op.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let (item, _) = ty(api, &op.returns);
            let poll_sym = jni_symbol(&class, "poll");
            let free_sym = jni_symbol(&class, "free");
            let item_success = success_expr(&op.returns, "__v");
            body.push_str(&format!(
                "/// Poll the `{iface}.{op}` cursor once: an item (as its Java object),\n\
                 /// null on clean close, or a thrown exception on a terminal failure.\n\
                 #[no_mangle]\npub extern \"system\" fn {poll_sym}<'local>(mut env: JNIEnv<'local>, _class: JClass<'local>, cursor: jlong) -> jobject {{\n    let env = &mut env;\n    \
                 let __stream = unsafe {{ &*(cursor as *const Box<dyn PollStream<{item}>>) }};\n    \
                 loop {{\n        match __stream.poll(Duration::from_millis(500)) {{\n            \
                 Poll::Item(__v) => return {item_success},\n            \
                 Poll::Idle => continue,\n            \
                 Poll::Closed => return std::ptr::null_mut(),\n            \
                 Poll::Failed(__e) => {{ throw(env, __e); return std::ptr::null_mut(); }}\n        }}\n    }}\n}}\n\n\
                 /// Release the `{iface}.{op}` cursor (idempotent on the Java side).\n\
                 #[no_mangle]\npub extern \"system\" fn {free_sym}<'local>(_env: JNIEnv<'local>, _class: JClass<'local>, cursor: jlong) {{\n    \
                 if cursor != 0 {{ unsafe {{ drop(Box::from_raw(cursor as *mut Box<dyn PollStream<{item}>>)); }} }}\n}}\n\n",
                iface = i.name,
                op = op.name,
            ));
        }

        // ctor / init + free (stateful handle over Box<Arc<Impl>>).
        if has_ctor {
            let ctor = i.ops.iter().find(|o| o.shape == Shape::Ctor).unwrap();
            let init_sym = jni_symbol(&i.name, "init");
            let free_sym = jni_symbol(&i.name, "free");
            let mut params = String::from("mut env: JNIEnv<'local>, _class: JClass<'local>");
            let mut convs = String::new();
            let mut names: Vec<String> = Vec::new();
            for p in &ctor.params {
                let (decl, conv) = marshal_param(api, p);
                params.push_str(&format!(", {decl}"));
                convs.push_str(&format!("    {conv}\n"));
                names.push(snake(&p.name));
            }
            body.push_str(&format!(
                "/// Construct the `{iface}` core and hand Java an opaque `long` handle\n\
                 /// (a leaked `Box<Arc<{iface}Impl>>`); `free` reclaims it.\n\
                 #[no_mangle]\npub extern \"system\" fn {init_sym}<'local>({params}) -> jlong {{\n    let env = &mut env;\n{convs}    \
                 match <{impl_path} as {trait_name}>::{ctor_name}({args}) {{\n        \
                 Ok(__c) => Box::into_raw(Box::new(Arc::new(__c))) as jlong,\n        \
                 Err(__e) => {{ throw(env, __e); 0 }}\n    }}\n}}\n\n\
                 /// Drop the `{iface}` handle (idempotent on the Java side).\n\
                 #[no_mangle]\npub extern \"system\" fn {free_sym}<'local>(_env: JNIEnv<'local>, _class: JClass<'local>, handle: jlong) {{\n    \
                 if handle != 0 {{ unsafe {{ drop(Box::from_raw(handle as *mut Arc<{impl_path}>)); }} }}\n}}\n\n",
                iface = i.name,
                ctor_name = snake(&ctor.name),
                args = names.join(", "),
            ));
        }

        // unary + stream-open methods.
        for op in &i.ops {
            match op.shape {
                // Ctor is emitted above; a @manual op is hand-written outside the
                // generated surface.
                Shape::Ctor | Shape::Manual => {}
                // A subscription op REGISTERS the listener (its one callback param,
                // wrapped into the uniform `Box<dyn Fn>`) and hands Java an opaque
                // `long` Subscription handle owning the core's unsubscribe closure.
                // On a ctor-less (factory-born) interface there is no stateful handle
                // to register against yet, so emit the honest skip-note instead of a
                // static JNI call that mismatches the `&self` core method.
                Shape::Subscription if !has_ctor => {
                    body.push_str(&format!(
                        "{}\n\n",
                        super::subscription_factory_skip_note(&i.name, &op.name)
                    ));
                }
                Shape::Subscription => {
                    super::java_callback::emit_subscription_jni(
                        api,
                        &i.name,
                        op,
                        &impl_path,
                        &trait_name,
                        has_ctor,
                        &mut body,
                    );
                }
                Shape::Unary => {
                    // async ops route through a `native<Pascal>` symbol (the Java
                    // side wraps them in a CompletableFuture); sync ops use the
                    // Java method name (pinned or camel) directly.
                    let jni_method = if op.is_async {
                        format!("native{}", pascal(&op.name))
                    } else {
                        op_jname(op)
                    };
                    if has_ctor {
                        emit_unary_stateful(api, &i.name, op, &jni_method, &impl_path, &mut body);
                    } else {
                        let call_prefix =
                            format!("<{impl_path} as {trait_name}>::{}", snake(&op.name));
                        emit_unary_fn(
                            api,
                            &i.name,
                            op,
                            &jni_method,
                            &call_prefix,
                            false,
                            &mut body,
                        );
                    }
                }
                Shape::Stream => {
                    let class = pascal(&op.name);
                    let native = format!("native{class}");
                    let sym = jni_symbol(&i.name, &native);
                    let (item, _) = ty(api, &op.returns);
                    let mut params =
                        String::from("mut env: JNIEnv<'local>, _class: JClass<'local>");
                    if has_ctor {
                        params.push_str(", handle: jlong");
                    }
                    let mut convs = String::new();
                    let mut names: Vec<String> = Vec::new();
                    for p in &op.params {
                        let (decl, conv) = marshal_param(api, p);
                        params.push_str(&format!(", {decl}"));
                        convs.push_str(&format!("    {conv}\n"));
                        names.push(snake(&p.name));
                    }
                    let callee = if has_ctor {
                        format!(
                            "{{ let core = unsafe {{ &*(handle as *const Arc<{impl_path}>) }}; core.{}({}) }}",
                            snake(&op.name),
                            names.join(", ")
                        )
                    } else {
                        format!(
                            "<{impl_path} as {trait_name}>::{}({})",
                            snake(&op.name),
                            names.join(", ")
                        )
                    };
                    body.push_str(&format!(
                        "/// Open the `{iface}.{op}` stream: returns an opaque cursor handle\n\
                         /// (a leaked `Box<Box<dyn PollStream<{item}>>>`) the `{class}` class polls.\n\
                         #[no_mangle]\npub extern \"system\" fn {sym}<'local>({params}) -> jlong {{\n    let env = &mut env;\n{convs}    \
                         match {callee} {{\n        \
                         Ok(__s) => Box::into_raw(Box::new(__s)) as jlong,\n        \
                         Err(__e) => {{ throw(env, __e); 0 }}\n    }}\n}}\n\n",
                        iface = i.name,
                        op = op.name,
                    ));
                }
            }
        }
    }

    let src = api.source.as_deref().unwrap_or("the fluessig catalog");
    let out = crate::rustfmt::format(format!(
        "//! GENERATED by fluessig bindgen from {src} (api layer, Rust JNI glue). Do not edit.\n{}#![allow(clippy::all)]\n#![allow(unused_imports)]\n#![allow(unused_variables)]\n#![allow(unused_mut)]\n#![allow(non_snake_case)]\n#![allow(dead_code)]\n\n{body}",
        note_line(banner_note)
    ));
    format!("{out}{st_note}")
}

/// Emit a stateful unary op's JNI fn: dereference the `handle` into the core
/// `Arc`, then route exactly like the stateless path.
fn emit_unary_stateful(
    api: &ApiDoc,
    class: &str,
    op: &ApiOp,
    jni_method: &str,
    impl_path: &str,
    out: &mut String,
) {
    let sym = jni_symbol(class, jni_method);
    let ret_ty = jni_ret_ty(&op.returns);
    let ret_clause = if ret_ty == "()" {
        String::new()
    } else {
        format!(" -> {ret_ty}")
    };
    let mut params = String::from("mut env: JNIEnv<'local>, _class: JClass<'local>, handle: jlong");
    let mut convs = String::new();
    let mut names: Vec<String> = Vec::new();
    for p in &op.params {
        let (decl, conv) = marshal_param(api, p);
        params.push_str(&format!(", {decl}"));
        convs.push_str(&format!("    {conv}\n"));
        names.push(snake(&p.name));
    }
    let call = format!("core.{}({})", snake(&op.name), names.join(", "));
    let core_line = format!("    let core = unsafe {{ &*(handle as *const Arc<{impl_path}>) }};\n");
    let body = if op.infallible {
        format!(
            "{core_line}    let __v = {call};\n    {}\n",
            success_expr(&op.returns, "__v")
        )
    } else {
        format!(
            "{core_line}    match {call} {{\n        Ok(__v) => {},\n        Err(__e) => {{ throw(env, __e); {} }}\n    }}\n",
            success_expr(&op.returns, "__v"),
            jni_zero(&op.returns)
        )
    };
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"system\" fn {sym}<'local>({params}){ret_clause} {{\n    let env = &mut env;\n{convs}{body}}}\n\n"
    ));
}

// ═══════════════════════════════════════════════════════════════════════════
// (b) the Java source classes
// ═══════════════════════════════════════════════════════════════════════════

/// Generate the `.java` source classes: one `enum` per non-string vocabulary,
/// one DTO class per model, one envelope class per union, and one class per
/// interface (with `native` declarations, `System.loadLibrary`, a stateful
/// handle + `close()` when the interface has a ctor, `CompletableFuture`
/// wrappers for async ops, and a poll-cursor class per stream op). Returned as
/// `(relative_path, source)` pairs under the `fluessig/` package dir.
pub fn java_sources(api: &ApiDoc, enums: &[EnumDesc]) -> Vec<(String, String)> {
    java_sources_with(api, enums, None)
}

/// [`java_sources`] with the caller's optional banner note (a lint-suppression
/// marker) prepended to every emitted `.java` file as a `//` comment — the Java
/// parallel to the Rust glue's `//!` banner. The generated per-interface classes
/// repeat a fixed handle/ctor/close + native-decl template across interfaces (the
/// language × shape grid), which a duplication linter flags; a consumer passes
/// e.g. `straitjacket-allow-file:duplication` here to mark that as intentional.
/// `None` ⇒ no comment, so a note-free caller's output stays byte-identical.
pub fn java_sources_with(
    api: &ApiDoc,
    enums: &[EnumDesc],
    banner_note: Option<&str>,
) -> Vec<(String, String)> {
    // A `single_threaded` interface is a thread-confined `!Send` handle — node-only
    // today; the Rust JNI glue (`java_binding`) emits nothing for it, so no `.java`
    // handle class must be generated either (it would reference absent JNI symbols).
    let (api_owned, _st_note) = crate::bindgen::split_single_threaded(api, "java");
    let api = &api_owned;
    let mut files: Vec<(String, String)> = Vec::new();
    let path = |cls: &str| format!("{PACKAGE}/{cls}.java");

    // enums → real Java enums carrying the wire token + fromWire/toWire.
    for (name, variants) in enums {
        if is_string_enum(api, name) {
            continue;
        }
        let mut consts: Vec<String> = Vec::new();
        for v in variants {
            consts.push(format!(
                "    {}(\"{}\")",
                variant_ident(&v.name),
                variant_token(v, LANG)
            ));
        }
        let src = format!(
            "package {PACKAGE};\n\n\
             /** Generated enum — its wire token crosses the JNI seam as a String. */\n\
             public enum {name} {{\n{};\n\n    \
             private final String wire;\n    \
             {name}(String wire) {{ this.wire = wire; }}\n    \
             public String toWire() {{ return this.wire; }}\n    \
             public static {name} fromWire(String w) {{\n        \
             for ({name} v : values()) {{ if (v.wire.equals(w)) return v; }}\n        \
             throw new IllegalArgumentException(\"unknown {name} wire token: \" + w);\n    }}\n}}\n",
            consts.join(",\n")
        );
        files.push((path(name), src));
    }

    // models → DTO classes (fields + all-args ctor + getters).
    for m in &api.models {
        let mut field_decls = String::new();
        let mut ctor_params: Vec<String> = Vec::new();
        let mut ctor_assigns = String::new();
        let mut getters = String::new();
        for f in &m.fields {
            let jt = java_ty(api, &field_ty(f));
            let jn = field_jname(f);
            field_decls.push_str(&format!("    private final {jt} {jn};\n"));
            ctor_params.push(format!("{jt} {jn}"));
            ctor_assigns.push_str(&format!("        this.{jn} = {jn};\n"));
            getters.push_str(&format!(
                "    public {jt} {}() {{ return this.{jn}; }}\n",
                getter(&jn)
            ));
        }
        let doc = m
            .doc
            .as_deref()
            .map(|d| format!("/** {} */\n", d.replace('\n', " ")))
            .unwrap_or_default();
        let imports = if m
            .fields
            .iter()
            .any(|f| matches!(field_ty(f), ApiType::List { .. }))
        {
            "import java.util.List;\n\n"
        } else {
            "\n"
        };
        let src = format!(
            "package {PACKAGE};\n\n{imports}{doc}public final class {name} {{\n{field_decls}\n    \
             public {name}({ctor_params}) {{\n{ctor_assigns}    }}\n\n{getters}}}\n",
            name = m.name,
            ctor_params = ctor_params.join(", "),
        );
        files.push((path(&m.name), src));
    }

    // unions → the JSON envelope carrier class (kind + payload String).
    for u in &api.unions {
        let variants = u
            .variants
            .iter()
            .map(|v| v.tag.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let src = format!(
            "package {PACKAGE};\n\n\
             /** Tagged union `{name}` — crosses the JNI seam as its JSON envelope\n \
             * {{\"kind\": tag, \"payload\": body}}. Variant tags: {variants}. */\n\
             public final class {name} {{\n    \
             private final String kind;\n    private final String payload;\n\n    \
             public {name}(String kind, String payload) {{ this.kind = kind; this.payload = payload; }}\n    \
             public String getKind() {{ return this.kind; }}\n    \
             public String getPayload() {{ return this.payload; }}\n}}\n",
            name = u.name,
        );
        files.push((path(&u.name), src));
    }

    // the opaque Subscription handle class, once, whenever a subscription op
    // exists (gated so a subscription-free schema emits no extra file).
    if crate::bindgen::api_uses_subscription(api) {
        files.push((
            path("Subscription"),
            super::java_callback::subscription_java_class(),
        ));
    }

    // interfaces → the native-method surface.
    for i in &api.interfaces {
        files.push((path(&i.name), java_interface_class(api, i)));
        // one poll-cursor class per stream op.
        for op in i.ops.iter().filter(|o| o.shape == Shape::Stream) {
            let class = pascal(&op.name);
            let item = java_ty(api, &op.returns);
            let src = format!(
                "package {PACKAGE};\n\nimport java.util.Optional;\n\n\
                 /** Poll-based cursor over `{iface}.{op}` — call {{@link #next()}} until it\n \
                 * returns an empty Optional (clean close); a terminal core failure throws. */\n\
                 public final class {class} {{\n    \
                 static {{ System.loadLibrary(\"{LIB}\"); }}\n\n    \
                 private long cursor;\n\n    \
                 {class}(long cursor) {{ this.cursor = cursor; }}\n\n    \
                 private static native Object poll(long cursor);\n    \
                 private static native void free(long cursor);\n\n    \
                 /** The next item, or empty once the stream is exhausted. */\n    \
                 public Optional<{item}> next() {{\n        \
                 Object o = poll(this.cursor);\n        \
                 return Optional.ofNullable(({item}) o);\n    }}\n\n    \
                 /** Release the cursor's core-side resources (idempotent). */\n    \
                 public void close() {{ if (this.cursor != 0) {{ free(this.cursor); this.cursor = 0; }} }}\n}}\n",
                iface = i.name,
                op = op.name,
            );
            files.push((path(&class), src));
        }
    }

    // Prepend the caller's banner note (a `//` comment, legal before `package`)
    // to every file; `None` leaves the output byte-identical.
    if let Some(note) = banner_note {
        for (_, src) in files.iter_mut() {
            *src = format!("// {note}\n{src}");
        }
    }

    files
}

/// The Java class for one interface: `native` declarations + public wrappers.
fn java_interface_class(api: &ApiDoc, i: &crate::api::ApiInterface) -> String {
    let has_ctor = i.ops.iter().any(|o| o.shape == Shape::Ctor);
    let mut needs_future = false;
    let mut needs_list = false;

    let jparams = |op: &ApiOp| -> String {
        op.params
            .iter()
            .map(|p| format!("{} {}", java_ty(api, &param_ty(p)), camel(&p.name)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let jargs = |op: &ApiOp| -> String {
        op.params
            .iter()
            .map(|p| camel(&p.name))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut natives = String::new();
    let mut methods = String::new();

    // constructor / handle
    if has_ctor {
        let ctor = i.ops.iter().find(|o| o.shape == Shape::Ctor).unwrap();
        natives.push_str(&format!(
            "    private static native long init({});\n    private static native void free(long handle);\n",
            jparams(ctor)
        ));
        methods.push_str(&format!(
            "    private long handle;\n\n    \
             /** Construct the `{iface}` handle. */\n    \
             public {iface}({params}) {{ this.handle = init({args}); }}\n\n    \
             /** Release the handle (idempotent). */\n    \
             public void close() {{ if (this.handle != 0) {{ free(this.handle); this.handle = 0; }} }}\n\n",
            iface = i.name,
            params = jparams(ctor),
            args = jargs(ctor),
        ));
    }

    for op in &i.ops {
        match op.shape {
            Shape::Ctor => {}
            Shape::Manual => {
                methods.push_str(&format!(
                    "    // @manual: {}.{} — hand-written outside the generated surface.\n\n",
                    i.name, op.name
                ));
            }
            // A subscription op on a ctor-less (factory-born) interface: no stateful
            // handle to register against yet — emit the honest skip-note (its JNI
            // counterpart is likewise deferred) instead of a static method that
            // mismatches the `&self` core registration.
            Shape::Subscription if !has_ctor => {
                methods.push_str(&format!(
                    "    {}\n\n",
                    super::subscription_factory_skip_note(&i.name, &op.name)
                ));
            }
            // A subscription op: a `native<Op>` returning the opaque handle `long`,
            // wrapped in a public method that hands back a `Subscription` object.
            Shape::Subscription => {
                let jname = op_jname(op);
                let native = format!("native{}", pascal(&op.name));
                let native_decl_params = native_params(api, op, has_ctor);
                natives.push_str(&format!(
                    "    private static native long {native}({native_decl_params});\n"
                ));
                let handle_arg = if has_ctor { "this.handle" } else { "" };
                let sep = if has_ctor && !op.params.is_empty() {
                    ", "
                } else {
                    ""
                };
                let wrapper_mod = if has_ctor { "" } else { "static " };
                methods.push_str(&format!(
                    "    /** Register a listener on `{iface}.{op}`; returns an owning Subscription. */\n    \
                     public {wrapper_mod}Subscription {jname}({params}) {{ return new Subscription({native}({handle_arg}{sep}{args})); }}\n\n",
                    iface = i.name,
                    op = op.name,
                    params = jparams(op),
                    args = jargs(op),
                ));
            }
            Shape::Unary => {
                let ret = java_ty(api, &op.returns);
                if matches!(op.returns, ApiType::List { .. }) {
                    needs_list = true;
                }
                let handle_arg = if has_ctor { "this.handle" } else { "" };
                let sep = if has_ctor && !op.params.is_empty() {
                    ", "
                } else {
                    ""
                };
                if op.is_async {
                    // async: a private blocking native + a CompletableFuture wrapper.
                    needs_future = true;
                    let native = format!("native{}", pascal(&op.name));
                    let boxed = java_boxed(api, &op.returns);
                    let boxed = if boxed == "void" {
                        "Void".to_string()
                    } else {
                        boxed
                    };
                    let native_decl_params = native_params(api, op, has_ctor);
                    natives.push_str(&format!(
                        "    private {static_}native {ret} {native}({native_decl_params});\n",
                        static_ = "static ",
                    ));
                    let call_args = format!("{handle_arg}{sep}{}", jargs(op));
                    let jname = op_jname(op);
                    // a stateless interface class has a private ctor, so its
                    // async wrapper must be `static`; a stateful one is an
                    // instance method threading `this.handle`.
                    let wrapper_mod = if has_ctor { "" } else { "static " };
                    if ret == "void" {
                        methods.push_str(&format!(
                            "    /** Async `{iface}.{op}` — the blocking native call wrapped in a future. */\n    \
                             public {wrapper_mod}CompletableFuture<Void> {jname}({params}) {{\n        \
                             return CompletableFuture.runAsync(() -> {native}({call_args}));\n    }}\n\n",
                            iface = i.name,
                            op = op.name,
                            params = jparams(op),
                        ));
                    } else {
                        methods.push_str(&format!(
                            "    /** Async `{iface}.{op}` — the blocking native call wrapped in a future. */\n    \
                             public {wrapper_mod}CompletableFuture<{boxed}> {jname}({params}) {{\n        \
                             return CompletableFuture.supplyAsync(() -> {native}({call_args}));\n    }}\n\n",
                            iface = i.name,
                            op = op.name,
                            params = jparams(op),
                        ));
                    }
                } else {
                    // sync: a public native the caller invokes directly (a static
                    // for a stateless op, a handle-threaded static + wrapper for a
                    // stateful one).
                    let jname = op_jname(op);
                    if has_ctor {
                        let native = jname.clone();
                        let native_decl_params = native_params(api, op, true);
                        natives.push_str(&format!(
                            "    private static native {ret} {native}({native_decl_params});\n"
                        ));
                        let call_args = format!("{handle_arg}{sep}{}", jargs(op));
                        let ret_kw = if ret == "void" { "" } else { "return " };
                        methods.push_str(&format!(
                            "    /** `{iface}.{op}`. */\n    \
                             public {ret} {jname}({params}) {{ {ret_kw}{native}({call_args}); }}\n\n",
                            iface = i.name,
                            op = op.name,
                            params = jparams(op),
                        ));
                    } else {
                        natives.push_str(&format!(
                            "    public static native {ret} {jname}({});\n",
                            jparams(op)
                        ));
                    }
                }
            }
            Shape::Stream => {
                let class = pascal(&op.name);
                let native = format!("native{class}");
                let native_decl_params = native_params(api, op, has_ctor);
                natives.push_str(&format!(
                    "    private static native long {native}({native_decl_params});\n"
                ));
                let handle_arg = if has_ctor { "this.handle" } else { "" };
                let sep = if has_ctor && !op.params.is_empty() {
                    ", "
                } else {
                    ""
                };
                let wrapper_mod = if has_ctor { "" } else { "static " };
                methods.push_str(&format!(
                    "    /** Open the `{iface}.{op}` stream as a poll cursor. */\n    \
                     public {wrapper_mod}{class} {jname}({params}) {{ return new {class}({native}({handle_arg}{sep}{args})); }}\n\n",
                    iface = i.name,
                    op = op.name,
                    jname = op_jname(op),
                    params = jparams(op),
                    args = jargs(op),
                ));
            }
        }
    }

    let mut imports = String::new();
    if needs_future {
        imports.push_str("import java.util.concurrent.CompletableFuture;\n");
    }
    if needs_list {
        imports.push_str("import java.util.List;\n");
    }
    if !imports.is_empty() {
        imports.push('\n');
    }
    let doc = i
        .doc
        .as_deref()
        .map(|d| format!("/** {} */\n", d.replace('\n', " ")))
        .unwrap_or_default();
    let ctor_note = if has_ctor {
        ""
    } else {
        // a stateless class: a private ctor so it reads as a static namespace.
        "    private "
    };
    let priv_ctor = if has_ctor {
        String::new()
    } else {
        format!("{ctor_note}{}() {{}}\n\n", i.name)
    };
    format!(
        "package {PACKAGE};\n\n{imports}{doc}public final class {name} {{\n    \
         static {{ System.loadLibrary(\"{LIB}\"); }}\n\n{priv_ctor}{natives}\n{methods}}}\n",
        name = i.name,
    )
}

/// The native-method param list for an op on the Java side (a stateful op leads
/// with `long handle`).
pub(super) fn native_params(api: &ApiDoc, op: &ApiOp, has_ctor: bool) -> String {
    let mut ps: Vec<String> = Vec::new();
    if has_ctor {
        ps.push("long handle".to_string());
    }
    for p in &op.params {
        ps.push(format!("{} {}", java_ty(api, &param_ty(p)), camel(&p.name)));
    }
    ps.join(", ")
}
