//! Regression gate for a structured-union codegen bug: when a tagged union's
//! discriminant tag field (`type`, the pi default) collides with a data field of
//! the SAME name declared by the variant models (the discriminant mirrored into
//! the payload — e.g. an `AgentSessionEvent` union whose variant models each
//! carry their own `type` field), the node + python projections emitted the
//! `type` field TWICE per tagged struct:
//!
//!   * node emitted the escaped tag field `pub r#type: String,` AND then the
//!     variant model's `type` field as a bare, UNESCAPED `pub type: String,`
//!     (invalid Rust — rustfmt rejected the whole module);
//!   * python emitted the escaped tag field `pub r#type: String,` AND then the
//!     variant model's `type` field ALSO escaped to `pub r#type: String,` (two
//!     fields of the same ident — a duplicate-field compile error), plus a
//!     duplicate `type` ctor param.
//!
//! The fix dedupes on the tag: a variant data field whose name equals the tag
//! field is not re-emitted — the literal-set tag field already carries it. Both
//! `node_binding` and `python_binding` run rustfmt on their output, so a
//! NON-panicking call is itself proof the emitted Rust parses; the asserts pin
//! that exactly ONE, correctly-escaped `type` field survives with its wire value
//! (the variant tag literal) preserved.

use fluessig::api::load_api;
use fluessig::bindgen::{node_binding, python_binding};

/// A `type`-tagged union whose two variant models EACH declare their own `type`
/// field (the collision), and — for the node keyword-escaping path — a variant
/// data field named `match` (a Rust keyword that is NOT the tag).
fn fixture() -> fluessig::api::ApiDoc {
    let json = r#"{
      "fluessig": {"format": 1, "emitter": "t", "compiler": "t"},
      "models": [
        {"name": "AgentStarted", "fields": [
          {"name": "type", "type": "string", "nullable": false},
          {"name": "id", "type": "string", "nullable": false}
        ]},
        {"name": "AgentStopped", "fields": [
          {"name": "type", "type": "string", "nullable": false},
          {"name": "match", "type": "string", "nullable": false}
        ]}
      ],
      "unions": [
        {"name": "AgentSessionEvent", "variants": [
          {"tag": "started", "type": {"model": "AgentStarted"}},
          {"tag": "stopped", "type": {"model": "AgentStopped"}}
        ]}
      ],
      "interfaces": [
        {"name": "Api", "ops": [
          {"name": "next", "shape": "unary", "async": true,
           "params": [],
           "returns": {"union": "AgentSessionEvent"}}
        ]}
      ]
    }"#;
    load_api(json).unwrap()
}

/// Slice a tagged-variant struct body out of the generated output for a
/// per-struct assertion (so counts are scoped to the one struct).
fn struct_body<'a>(out: &'a str, name: &str) -> &'a str {
    let start = out
        .find(&format!("pub struct {name} {{"))
        .unwrap_or_else(|| panic!("struct {name} not found:\n{out}"));
    let rest = &out[start..];
    let end = rest.find('}').expect("struct close brace");
    &rest[..end]
}

// ── node ─────────────────────────────────────────────────────────────────────

#[test]
fn node_tag_collision_emits_single_escaped_type_field() {
    // A non-panicking call already proves rustfmt accepted the output (the bug
    // produced an unescaped `pub type:` that rustfmt rejected).
    let out = node_binding(&fixture(), &[], None);
    let body = struct_body(&out, "AgentSessionEventStarted");
    // exactly ONE `type` field, raw-escaped, and NEVER the bare keyword form.
    assert_eq!(
        body.matches("r#type").count(),
        1,
        "exactly one escaped `type` field in the tagged struct:\n{body}"
    );
    assert!(
        !body.contains("pub type:"),
        "the bare unescaped `type` keyword field must NOT be emitted:\n{body}"
    );
    // the surviving field is the literal-set discriminant tag.
    assert!(
        out.contains("r#type: \"started\".into()"),
        "the discriminant literal is preserved:\n{out}"
    );
    // the OTHER (non-tag) keyword data field is now escaped + js_name pinned
    // (rustfmt splits the attr onto its own line).
    assert!(
        out.contains("#[napi(js_name = \"match\")]") && out.contains("pub r#match: String,"),
        "a non-tag keyword variant field is raw-escaped with js_name pinned:\n{out}"
    );
}

// ── python ───────────────────────────────────────────────────────────────────

#[test]
fn python_tag_collision_emits_single_escaped_type_field() {
    let out = python_binding(&fixture(), &[], None);
    let body = struct_body(&out, "AgentSessionEventStarted");
    assert_eq!(
        body.matches("r#type").count(),
        1,
        "exactly one escaped `type` field in the tagged pyclass:\n{body}"
    );
    assert!(
        !body.contains("pub type:"),
        "the bare unescaped `type` keyword field must NOT be emitted:\n{body}"
    );
    // the surviving field is the literal-set discriminant tag …
    assert!(
        out.contains("r#type: \"started\".into()"),
        "the discriminant literal is preserved:\n{out}"
    );
    // … and the ctor no longer takes a duplicate `type` param (only `id`).
    assert!(
        out.contains("#[pyo3(signature = (id))]"),
        "the ctor drops the mirrored `type` param, keeping only real data:\n{out}"
    );
}
