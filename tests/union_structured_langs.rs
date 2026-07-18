//! Structured discriminated-union projection in the python (PyO3) and ruby
//! (Magnus) backends. Structured tagged-object projection is the DEFAULT on both;
//! the JSON-envelope `String` carrier stays reachable as an explicit opt-out
//! (`UnionProjection::Envelope`). These mirror the node assertions in
//! `tests/union_structured.rs` — per-variant tagged objects with the discriminant
//! set to the literal tag, per-union tag configurability, and union op-return
//! type agreement with the core trait — but for the two backends whose generated
//! output is separately PROVEN to compile against real `pyo3`/`magnus` (fluessig
//! itself has no FFI dep, so these string-tests never build the output).
//!
//! straitjacket-allow-file:duplication — the python/ruby assertion pairs here are
//! DELIBERATELY parallel (the point is cross-language projection parity).

use fluessig::api::{ApiType, Shape};
use fluessig::bindgen::{
    python_binding, python_binding_with_options, ruby_binding, ruby_binding_with_options,
    PythonOptions, RubyOptions, UnionProjection,
};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

fn py_structured(tag: &str) -> PythonOptions {
    PythonOptions {
        union_projection: UnionProjection::Structured {
            tag_field: tag.into(),
        },
    }
}

fn rb_structured(tag: &str) -> RubyOptions {
    RubyOptions {
        union_projection: UnionProjection::Structured {
            tag_field: tag.into(),
        },
    }
}

/// Mutate `Watch.clear` (a unary op) to RETURN the union, so a return position —
/// not just the nested `Event.payload` DTO field — exercises the projection.
fn api_with_union_return() -> fluessig::api::ApiDoc {
    let mut api = fluessig::api::load_api(API).unwrap();
    let op = api.interfaces[0]
        .ops
        .iter_mut()
        .find(|o| o.name == "clear")
        .unwrap();
    assert_eq!(op.shape, Shape::Unary);
    op.returns = ApiType::Union {
        union: "EventPayload".into(),
    };
    api
}

// ── python (PyO3) ─────────────────────────────────────────────────────────────

#[test]
fn python_structured_union_lowers_to_tagged_pyclasses() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let py = python_binding_with_options(&api, &enums, None, &py_structured("type"));

    // one #[pyclass] per variant, each carrying the discriminant as a readable
    // attribute (raw ident, since `type` is a Rust keyword) plus the model fields
    for v in ["EventPayloadMessage", "EventPayloadLog", "EventPayloadExit"] {
        assert!(
            py.contains(&format!("pub struct {v}")),
            "tagged pyclass {v}:\n{py}"
        );
    }
    assert!(py.contains("#[pyclass(get_all)]"), "readable attrs:\n{py}");
    assert!(py.contains("pub r#type: String"), "tag attribute:\n{py}");
    assert!(py.contains("pub role: String"), "embedded field:\n{py}");

    // the discriminant literal is SET in both the ctor and the From conversion
    assert!(
        py.contains("r#type: \"message\".into()"),
        "literal-set message:\n{py}"
    );
    assert!(
        py.contains("r#type: \"log\".into()"),
        "literal-set log:\n{py}"
    );
    assert!(
        py.contains("r#type: \"exit\".into()"),
        "literal-set exit:\n{py}"
    );
    assert!(
        py.contains("impl From<AgentMessage> for EventPayloadMessage"),
        "From conversion over the variant model:\n{py}"
    );

    // the union projects to a plain enum wrapping the variants, tagged-object out
    // (IntoPyObject) and class-discriminated in (FromPyObject)
    assert!(
        py.contains("pub enum EventPayloadUnion"),
        "union enum:\n{py}"
    );
    assert!(
        py.contains("#[derive(Clone, IntoPyObject, FromPyObject)]"),
        "IntoPyObject/FromPyObject derive:\n{py}"
    );
    assert!(
        py.contains("Message(EventPayloadMessage)"),
        "enum arm:\n{py}"
    );

    // the nested `Event.payload` DTO field lowers to the enum, not the envelope
    assert!(
        py.contains("pub payload: EventPayloadUnion"),
        "nested field:\n{py}"
    );
    assert!(!py.contains("pub payload: String"), "envelope gone:\n{py}");

    // the variant classes are registered on the module
    assert!(
        py.contains("m.add_class::<EventPayloadMessage>()?;"),
        "variant class registered:\n{py}"
    );
}

#[test]
fn python_structured_union_return_matches_core_trait() {
    let api = api_with_union_return();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let py = python_binding_with_options(&api, &enums, None, &py_structured("type"));

    // the core-trait method and the #[pymethods] method must agree on the enum,
    // so `py.detach(move || core.clear())` type-checks against the return
    assert!(
        py.contains("fn clear(&self) -> anyhow::Result<EventPayloadUnion>;"),
        "core trait returns the union enum:\n{py}"
    );
    assert!(
        py.contains("fn clear(&self, py: Python<'_>) -> PyResult<EventPayloadUnion>"),
        "method returns the union enum:\n{py}"
    );
    assert!(
        !py.contains("fn clear(&self) -> anyhow::Result<String>;"),
        "the envelope String return must be gone:\n{py}"
    );
}

#[test]
fn python_per_union_tag_field_overrides_the_global() {
    let mut api = fluessig::api::load_api(API).unwrap();
    api.unions
        .iter_mut()
        .find(|u| u.name == "EventPayload")
        .unwrap()
        .tag_field = Some("kind".into());
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    // global is "type", but the per-union "kind" must win
    let py = python_binding_with_options(&api, &enums, None, &py_structured("type"));
    assert!(py.contains("pub kind: String"), "per-union tag attr:\n{py}");
    assert!(
        py.contains("kind: \"message\".into()"),
        "per-union literal-set:\n{py}"
    );
    assert!(
        !py.contains("pub r#type: String"),
        "global tag must not leak:\n{py}"
    );
}

#[test]
fn python_default_is_structured_with_envelope_opt_out() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();

    // 3-arg default → structured
    let def = python_binding(&api, &enums, None);
    assert!(
        def.contains("pub enum EventPayloadUnion"),
        "default structured:\n{def}"
    );
    assert!(
        !def.contains("pub payload: String"),
        "no envelope by default:\n{def}"
    );

    // explicit Envelope opt-out → the historical String carrier, no enum
    let env = python_binding_with_options(
        &api,
        &enums,
        None,
        &PythonOptions {
            union_projection: UnionProjection::Envelope,
        },
    );
    assert!(
        env.contains("pub payload: String"),
        "envelope opt-out carrier:\n{env}"
    );
    assert!(
        !env.contains("EventPayloadUnion"),
        "no enum under opt-out:\n{env}"
    );
}

// ── ruby (Magnus) ─────────────────────────────────────────────────────────────

#[test]
fn ruby_structured_union_lowers_to_tagged_wrapped_classes() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rb = ruby_binding_with_options(&api, &enums, None, &rb_structured("type"));

    // one #[magnus::wrap] class per variant, each carrying the tag getter
    for v in ["EventPayloadMessage", "EventPayloadLog", "EventPayloadExit"] {
        assert!(
            rb.contains(&format!("pub struct {v}")) && rb.contains(&format!("::{v}\"")),
            "wrapped class {v}:\n{rb}"
        );
    }
    assert!(rb.contains("pub r#type: String"), "tag field:\n{rb}");
    assert!(
        rb.contains("fn get_type(&self) -> String"),
        "tag getter:\n{rb}"
    );

    // the discriminant literal is SET in the From conversion, per variant tag
    assert!(
        rb.contains("r#type: \"message\".into()"),
        "literal-set message:\n{rb}"
    );
    assert!(
        rb.contains("r#type: \"log\".into()"),
        "literal-set log:\n{rb}"
    );
    assert!(
        rb.contains("r#type: \"exit\".into()"),
        "literal-set exit:\n{rb}"
    );
    assert!(
        rb.contains("impl From<AgentMessage> for EventPayloadMessage"),
        "From conversion over the variant model:\n{rb}"
    );

    // the union enum's IntoValue lowers to the matched wrapped class
    assert!(
        rb.contains("pub enum EventPayloadUnion"),
        "union enum:\n{rb}"
    );
    assert!(
        rb.contains("impl magnus::IntoValue for EventPayloadUnion"),
        "IntoValue on the union:\n{rb}"
    );
    assert!(
        rb.contains("Self::Message(v) => v.into_value_with(ruby),"),
        "IntoValue arm lowers to the variant class:\n{rb}"
    );

    // the nested `Event.payload` output-DTO getter returns the enum
    assert!(
        rb.contains("fn get_payload(&self) -> EventPayloadUnion"),
        "nested union getter:\n{rb}"
    );
    assert!(!rb.contains("pub payload: String"), "envelope gone:\n{rb}");

    // the variant classes + their getters are registered
    assert!(
        rb.contains("class.define_class(\"EventPayloadMessage\", ruby.class_object())?;"),
        "variant class registered:\n{rb}"
    );
    assert!(
        rb.contains("method!(EventPayloadMessage::get_type, 0)"),
        "tag getter registered:\n{rb}"
    );
}

#[test]
fn ruby_structured_union_return_matches_core_trait() {
    let api = api_with_union_return();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rb = ruby_binding_with_options(&api, &enums, None, &rb_structured("type"));

    assert!(
        rb.contains("fn clear(&self) -> anyhow::Result<EventPayloadUnion>;"),
        "core trait returns the union enum:\n{rb}"
    );
    assert!(
        rb.contains("fn clear(&self) -> Result<EventPayloadUnion, Error>"),
        "method returns the union enum:\n{rb}"
    );
    assert!(
        !rb.contains("fn clear(&self) -> anyhow::Result<String>;"),
        "the envelope String return must be gone:\n{rb}"
    );
}

#[test]
fn ruby_per_union_tag_field_overrides_the_global() {
    let mut api = fluessig::api::load_api(API).unwrap();
    api.unions
        .iter_mut()
        .find(|u| u.name == "EventPayload")
        .unwrap()
        .tag_field = Some("kind".into());
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rb = ruby_binding_with_options(&api, &enums, None, &rb_structured("type"));
    assert!(
        rb.contains("pub kind: String"),
        "per-union tag field:\n{rb}"
    );
    assert!(
        rb.contains("fn get_kind(&self) -> String"),
        "per-union tag getter:\n{rb}"
    );
    assert!(
        rb.contains("kind: \"message\".into()"),
        "per-union literal-set:\n{rb}"
    );
    assert!(
        !rb.contains("pub r#type: String"),
        "global tag must not leak:\n{rb}"
    );
}

#[test]
fn ruby_default_is_structured_with_envelope_opt_out() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();

    let def = ruby_binding(&api, &enums, None);
    assert!(
        def.contains("pub enum EventPayloadUnion"),
        "default structured:\n{def}"
    );
    assert!(
        !def.contains("pub payload: String"),
        "no envelope by default:\n{def}"
    );

    let env = ruby_binding_with_options(
        &api,
        &enums,
        None,
        &RubyOptions {
            union_projection: UnionProjection::Envelope,
        },
    );
    assert!(
        env.contains("pub payload: String"),
        "envelope opt-out carrier:\n{env}"
    );
    assert!(
        !env.contains("EventPayloadUnion"),
        "no enum under opt-out:\n{env}"
    );
}
