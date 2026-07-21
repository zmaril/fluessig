//! Regression gate for the unsigned + float scalars (`uint8` / `uint16` /
//! `uint32` / `float32` / `float64`).
//!
//! Before the fix, bindgen's shared `ty()` had no arm for these, so they fell
//! through the `String` catchall and were emitted as `String` (→ `.d.ts`
//! `string`, PHP `string`, a `char*` C boundary, …) in every backend — a wrong
//! type at the FFI boundary. This gate loads the `numeric` demo (float/uint as
//! entity + DTO fields and as op params/returns) and asserts every backend now
//! spells them as its native numeric type, NOT `String`.

use fluessig::api::load_api;
use fluessig::bindgen::{cpp_header, java_binding, node_binding, python_binding, EnumDesc};

fn numeric_api() -> fluessig::api::ApiDoc {
    load_api(&derive_demo::numeric::fluessig_catalog::api_to_json())
        .expect("numeric demo api.json must load clean")
}

/// node (napi): the DTO fields and op params/returns are the Rust numeric types
/// (`f64`/`f32`/`u32`/`u8`/`u16`), which napi projects to `.d.ts` `number` — the
/// pre-fix behaviour spelled every one of these `String` (→ `.d.ts` `string`).
#[test]
fn node_emits_numeric_not_string() {
    let node = node_binding(&numeric_api(), &[] as &[EnumDesc], None);

    // DTO fields (the `models` layer — `FuzzyMatchResult`).
    assert!(
        node.contains("pub score: f64,"),
        "node DTO score: f64\n{node}"
    );
    assert!(
        node.contains("pub confidence: f32,"),
        "node DTO confidence: f32\n{node}"
    );
    assert!(
        node.contains("pub image_id: u32,"),
        "node DTO image_id: u32\n{node}"
    );

    // op params + returns (the core trait).
    assert!(
        node.contains("fn scale(&self, now_ms: f64, factor: f32) -> f64;"),
        "node scale(f64, f32) -> f64\n{node}"
    );
    assert!(
        node.contains("fn allocate_image_id(&self) -> u32;"),
        "node allocate_image_id -> u32\n{node}"
    );
    assert!(
        node.contains("fn fuzzy_match(&self, retries: u8, port: u16) -> FuzzyMatchResult;"),
        "node fuzzy_match(u8, u16)\n{node}"
    );

    // the regression itself: NONE of these slots is a `String` anymore.
    assert!(
        !node.contains("pub score: String")
            && !node.contains("now_ms: String")
            && !node.contains("-> String"),
        "node must not spell any numeric scalar as String\n{node}"
    );
}

/// python (pyo3): same Rust numeric types on the trait + `#[pyclass]` DTO.
#[test]
fn python_emits_numeric_not_string() {
    let py = python_binding(&numeric_api(), &[] as &[EnumDesc], None);

    assert!(py.contains("pub score: f64,"), "py DTO score: f64\n{py}");
    assert!(
        py.contains("pub confidence: f32,"),
        "py DTO confidence: f32\n{py}"
    );
    assert!(
        py.contains("pub image_id: u32,"),
        "py DTO image_id: u32\n{py}"
    );
    assert!(
        py.contains("fn scale(&self, now_ms: f64, factor: f32) -> f64;"),
        "py scale(f64, f32) -> f64\n{py}"
    );
    assert!(
        py.contains("fn allocate_image_id(&self) -> u32;"),
        "py allocate_image_id -> u32\n{py}"
    );
    assert!(
        !py.contains("pub score: String") && !py.contains("now_ms: String"),
        "python must not spell any numeric scalar as String\n{py}"
    );
}

/// cpp (C header): the DTO members + op signatures use the C fixed-width types,
/// not the `char*` string carrier the catchall produced.
#[test]
fn cpp_header_emits_numeric_not_char_star() {
    let h = cpp_header(&numeric_api(), &[] as &[EnumDesc], None);

    assert!(h.contains("double score;"), "cpp DTO double score\n{h}");
    assert!(
        h.contains("float confidence;"),
        "cpp DTO float confidence\n{h}"
    );
    assert!(
        h.contains("uint32_t image_id;"),
        "cpp DTO uint32_t image_id\n{h}"
    );
    assert!(
        h.contains("double Metrics_scale(Metrics* self, double now_ms, float factor);"),
        "cpp scale(double, float) -> double\n{h}"
    );
    assert!(
        h.contains("void Metrics_allocate_image_id(Metrics* self, uint32_t* out);"),
        "cpp allocate_image_id -> uint32_t out\n{h}"
    );
    assert!(
        h.contains("uint8_t retries, uint16_t port"),
        "cpp fuzzy_match(uint8_t, uint16_t)\n{h}"
    );
    // no numeric slot degraded to the string carrier.
    assert!(
        !h.contains("char* score") && !h.contains("char* now_ms"),
        "cpp must not spell any numeric scalar as char*\n{h}"
    );
}

/// java (JNI): the numeric scalars marshal via their JNI primitives
/// (`jdouble`/`jfloat`/`jint`/`jlong`) and box to `Double`/`Float`/`Long`, and
/// the `jfloat` sys type is in scope — never the `jstring`/`String` carrier.
#[test]
fn java_emits_numeric_not_string() {
    let java = java_binding(&numeric_api(), &[] as &[EnumDesc], None);

    // the float32 sys type is imported (else the glue would not compile).
    assert!(
        java.contains("jfloat"),
        "java must import + use jfloat for float32\n{java}"
    );
    // op params spelled as their JNI primitives.
    assert!(
        java.contains("now_ms_j: jdouble") && java.contains("factor_j: jfloat"),
        "java scale params: jdouble + jfloat\n{java}"
    );
    // DTO fields box to the right wrapper types (Rust core types f64/f32/u32).
    assert!(
        java.contains("JValue::Double(v.score)"),
        "java score boxes as Double\n{java}"
    );
    assert!(
        java.contains("JValue::Float(v.confidence)"),
        "java confidence boxes as Float\n{java}"
    );
    assert!(
        java.contains("JValue::Long(v.image_id as i64)"),
        "java uint32 image_id boxes as Long (full range)\n{java}"
    );
    // the core trait carries the true Rust numeric types.
    assert!(
        java.contains("fn scale(&self, now_ms: f64, factor: f32) -> f64;"),
        "java core trait scale(f64, f32) -> f64\n{java}"
    );
}
