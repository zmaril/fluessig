//! Regression gates for two codegen ident bugs that bit only on the full
//! pi-orchestrator surface (in the rustfmt/compile step), never on any prior
//! committed golden:
//!
//!   1. a hyphenated enum-variant discriminant (`openai-completions`) was emitted
//!      as the Rust variant IDENTIFIER verbatim (`Openai-completions`) — an
//!      INVALID ident that made rustfmt reject the generated module;
//!   2. a DTO field named `type` (a Rust keyword) was emitted as a bare `type:`
//!      field by the node + python backends — invalid Rust (rust-core already
//!      raw-escaped it to `r#type`).
//!
//! Both `node_binding` and `python_binding` run rustfmt on their output, so a
//! NON-panicking call is itself proof the emitted Rust parses; the asserts below
//! additionally pin that the WIRE discriminant / exposed field name are preserved
//! while only the language-level identifier is sanitized.

use fluessig::api::load_api;
use fluessig::bindgen::{node_binding, python_binding, EnumDesc, EnumVariant};

/// A DTO with a `type` field (bug 2) whose op returns it.
fn fixture() -> fluessig::api::ApiDoc {
    let json = r#"{
      "fluessig": {"format": 1, "emitter": "t", "compiler": "t"},
      "models": [
        {"name": "AssistantMessageDiagnostic", "fields": [
          {"name": "id", "type": "string", "nullable": false},
          {"name": "type", "type": "string", "nullable": false}
        ]}
      ],
      "unions": [],
      "interfaces": [
        {"name": "Api", "ops": [
          {"name": "getDiag", "shape": "unary", "async": true,
           "params": [{"name": "id", "type": "string"}],
           "returns": {"model": "AssistantMessageDiagnostic"}}
        ]}
      ]
    }"#;
    load_api(json).unwrap()
}

/// An enum with a hyphenated (kebab) discriminant (bug 1) plus a plain one.
fn hyphen_enums() -> Vec<EnumDesc> {
    vec![(
        "SourceOrigin".to_string(),
        vec![
            EnumVariant::plain("openai-completions"),
            EnumVariant::plain("anthropic"),
        ],
    )]
}

// ── node ─────────────────────────────────────────────────────────────────────

#[test]
fn node_hyphenated_variant_is_valid_ident_with_preserved_wire() {
    // A non-panicking call already proves rustfmt accepted the output.
    let out = node_binding(&fixture(), &hyphen_enums(), None);
    // The Rust IDENT is sanitized to a valid pascal-cased ident …
    assert!(
        out.contains("OpenaiCompletions"),
        "hyphenated variant must sanitize to a valid ident:\n{out}"
    );
    assert!(
        !out.contains("Openai-completions"),
        "the invalid hyphenated ident must NOT be emitted:\n{out}"
    );
    // … while the ORIGINAL wire discriminant is preserved on the napi value.
    assert!(
        out.contains(r#"#[napi(value = "openai-completions")]"#),
        "the original wire discriminant must be preserved on the variant:\n{out}"
    );
}

#[test]
fn node_type_field_is_raw_escaped_with_preserved_js_name() {
    let out = node_binding(&fixture(), &[], None);
    assert!(
        out.contains("pub r#type: String"),
        "the `type` field must be raw-escaped to `r#type`:\n{out}"
    );
    assert!(
        out.contains(r#"#[napi(js_name = "type")]"#),
        "the JS-exposed field name must stay `type` via js_name:\n{out}"
    );
}

// ── python ───────────────────────────────────────────────────────────────────

#[test]
fn python_hyphenated_variant_is_valid_ident() {
    let out = python_binding(&fixture(), &hyphen_enums(), None);
    assert!(
        out.contains("OpenaiCompletions"),
        "hyphenated variant must sanitize to a valid ident:\n{out}"
    );
    assert!(
        !out.contains("Openai-completions"),
        "the invalid hyphenated ident must NOT be emitted:\n{out}"
    );
}

#[test]
fn python_type_field_is_raw_escaped_with_preserved_pyo3_name() {
    let out = python_binding(&fixture(), &[], None);
    // struct field: escaped Rust ident + pinned exposed name.
    assert!(
        out.contains("pub r#type: String"),
        "the `type` field must be raw-escaped to `r#type`:\n{out}"
    );
    assert!(
        out.contains(r#"#[pyo3(name = "type")]"#),
        "the Python-exposed field name must stay `type` via pyo3(name):\n{out}"
    );
    // ctor param + body use the escaped ident consistently.
    assert!(
        out.contains("r#type: String") && out.contains("Self { id, r#type }"),
        "the ctor must use the escaped ident consistently:\n{out}"
    );
}
