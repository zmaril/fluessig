//! Per-symbol, per-language export-name (and package/module) pinning — the
//! cross-language generalization of the earlier node-only `jsName` slot.
//!
//! The mechanism is UNIFORM: one shared `bindings` table on every pinnable
//! symbol (keyed by language slug), resolved once through
//! [`fluessig::bindgen::pinned_name`] / [`fluessig::bindgen::variant_token`] /
//! [`fluessig::bindgen::pinned_group`]. Each backend keeps ONLY (i) its default
//! casing rule and (ii) its own rename syntax:
//!
//!  * node   → `#[napi(js_name = "…")]` on fields, `#[napi(value = "…")]` tokens
//!  * python → `#[pyo3(name = "…")]` on fields/enum members
//!  * ruby   → the `define_method` name string, the enum `wire()`/`parse()` token
//!  * php    → the ext-php-rs `#[rename("…")]` attribute (derive 0.10.2 syntax)
//!  * mcp    → the DTO field serde `rename` + the matching manifest property name
//!
//! Every backend is proved on ONE shared pinned symbol below, each asserted to
//! emit its OWN rename attribute; a CONTROL sibling stays each language's
//! default with NO rename attr (the byte-identical-default guard).

use std::collections::BTreeMap;

use fluessig::api::load_api;
use fluessig::bindgen::{
    fan_out, fan_out_path, mcp_module, node_binding, php_binding, pinned_group, python_binding,
    ruby_binding, symbol_groups, EnumDesc, EnumVariant, SymbolBinding,
};

/// The shared fixture: a `ToolResult` DTO whose `contentindex` field is pinned
/// in EVERY language (a distinct spelling per slug, to prove the resolver keys
/// by language), the pi node vectors (`contentIndex`, `toolCallId`, `isError`,
/// `stopReason`), and a `plainField` CONTROL with no pins. An op returns the DTO
/// so ruby treats it as an output class (getters) and mcp projects it as a tool.
const PINNED_API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [
    {"name": "ToolResult", "fields": [
      {"name": "contentindex", "type": "int32", "nullable": false, "bindings": {
        "node":   {"name": "contentIndex"},
        "python": {"name": "contentIndexPy"},
        "ruby":   {"name": "content_index_rb"},
        "php":    {"name": "contentIndexPhp"},
        "mcp":    {"name": "content_index_mcp"}
      }},
      {"name": "toolCallId", "type": "string",  "nullable": false, "bindings": {"node": {"name": "toolCallId"}}},
      {"name": "isError",    "type": "boolean", "nullable": false, "bindings": {"node": {"name": "isError"}}},
      {"name": "stopReason", "type": "string",  "nullable": true,  "bindings": {"node": {"name": "stopReason"}}},
      {"name": "plainField", "type": "string",  "nullable": false}
    ]}
  ],
  "unions": [],
  "interfaces": [
    {"name": "Tools", "ops": [
      {"name": "lookup", "shape": "unary", "params": [{"name": "input", "type": {"model": "ToolResult"}}], "returns": {"model": "ToolResult"}}
    ]}
  ]
}"#;

/// A pinned enum (`Sample`, not in the wire-valued allowlist) with a per-language
/// pinned member and an un-pinned CONTROL member.
fn pinned_enums() -> Vec<EnumDesc> {
    let mut bindings = BTreeMap::new();
    for (lang, name) in [
        ("node", "PinnedTok"),
        ("python", "PinnedPy"),
        ("ruby", "pinned_rb"),
        ("php", "pinned_php"),
        ("mcp", "pinned_mcp"),
    ] {
        bindings.insert(
            lang.to_string(),
            SymbolBinding {
                name: Some(name.to_string()),
                ..Default::default()
            },
        );
    }
    vec![(
        "Sample".to_string(),
        vec![
            EnumVariant {
                name: "pinned".to_string(),
                value: None,
                bindings,
            },
            // CONTROL: no pins ⇒ every backend's default `to_lowercase()` token.
            EnumVariant::plain("plainvariant"),
        ],
    )]
}

fn api() -> fluessig::api::ApiDoc {
    load_api(PINNED_API).expect("fixture api must load")
}

// ── node ─────────────────────────────────────────────────────────────────────

#[test]
fn node_pins_fields_and_enum_tokens_via_its_own_attrs() {
    let node = node_binding(&api(), &pinned_enums(), None);

    // pi vectors: exact JS spelling in the attr, rust ident stays snake.
    for (js, ident, ty) in [
        ("contentIndex", "pub contentindex: i32,", "i32"),
        ("toolCallId", "pub tool_call_id: String,", "String"),
        ("isError", "pub is_error: bool,", "bool"),
        ("stopReason", "pub stop_reason: Option<String>,", "Option"),
    ] {
        let _ = ty;
        assert!(
            node.contains(&format!(r#"#[napi(js_name = "{js}")]"#)),
            "node pins js_name {js}:\n{node}"
        );
        assert!(node.contains(ident), "node rust ident stays snake:\n{node}");
    }
    // enum token via node's own `#[napi(value)]`.
    assert!(
        node.contains(r#"#[napi(value = "PinnedTok")]"#),
        "node pins enum token verbatim:\n{node}"
    );

    // CONTROL: the un-pinned field + enum member keep node's defaults, no attr.
    assert!(
        node.contains("pub plain_field: String,"),
        "node control field stays default snake:\n{node}"
    );
    assert!(
        !node.contains(r#"js_name = "plainField""#),
        "node control field must gain no js_name attr:\n{node}"
    );
    assert!(
        node.contains(r#"#[napi(value = "plainvariant")]"#),
        "node control enum member keeps to_lowercase():\n{node}"
    );
}

// ── python ───────────────────────────────────────────────────────────────────

#[test]
fn python_pins_fields_and_enum_members_via_pyo3_name() {
    let py = python_binding(&api(), &pinned_enums(), None);

    assert!(
        py.contains(r#"#[pyo3(name = "contentIndexPy")]"#),
        "python pins field via pyo3 name:\n{py}"
    );
    assert!(
        py.contains(r#"#[pyo3(name = "PinnedPy")]"#) && py.contains("Pinned,"),
        "python pins enum member via pyo3 name:\n{py}"
    );

    // CONTROL: the un-pinned field/member stay bare (default = the rust ident).
    assert!(
        py.contains("pub plain_field: String,"),
        "python control field stays default:\n{py}"
    );
    assert!(
        !py.contains(r#"name = "plainField""#),
        "python control field gains no pyo3 name:\n{py}"
    );
    assert!(
        py.contains("Plainvariant,") && !py.contains(r#"name = "plainvariant""#),
        "python control enum member stays a bare ident:\n{py}"
    );
}

// ── ruby ─────────────────────────────────────────────────────────────────────

#[test]
fn ruby_pins_method_name_and_enum_token_via_its_own_rule() {
    let ruby = ruby_binding(&api(), &pinned_enums(), None);

    // The Ruby-visible method name is the pin; the internal getter stays snake.
    assert!(
        ruby.contains(
            r#"c.define_method("content_index_rb", method!(ToolResult::get_contentindex, 0))"#
        ),
        "ruby pins the define_method name, internal getter stays snake:\n{ruby}"
    );
    // enum parse token via ruby's pin.
    assert!(
        ruby.contains(r#""pinned_rb" => Ok(Self::Pinned),"#),
        "ruby pins the enum parse token:\n{ruby}"
    );

    // CONTROL: the un-pinned field method + enum member keep ruby's defaults.
    assert!(
        ruby.contains(r#"c.define_method("plain_field", method!(ToolResult::get_plain_field, 0))"#),
        "ruby control method stays default snake:\n{ruby}"
    );
    assert!(
        ruby.contains(r#""plainvariant" => Ok(Self::Plainvariant),"#),
        "ruby control enum member keeps to_lowercase():\n{ruby}"
    );
}

// ── php ──────────────────────────────────────────────────────────────────────

#[test]
fn php_pins_getter_and_enum_token_via_rename_attr() {
    let php = php_binding(&api(), &pinned_enums(), None);

    // ext-php-rs 0.13.1 (derive 0.10.2): default method case is camelCase, and a
    // per-method pin is the bare `#[rename("…")]` attribute on the getter.
    assert!(
        php.contains(r#"#[rename("contentIndexPhp")]"#),
        "php pins the getter via ext-php-rs #[rename]:\n{php}"
    );
    assert!(
        php.contains(r#""pinned_php" => Ok(Self::Pinned),"#)
            && php.contains(r#"Self::Pinned => "pinned_php","#),
        "php pins the enum parse+wire token:\n{php}"
    );

    // CONTROL: the un-pinned getter gains NO #[rename] (ext-php-rs camelCases it).
    assert!(
        php.contains("pub fn plain_field(&self)"),
        "php control getter stays the snake rust ident:\n{php}"
    );
    assert!(
        !php.contains(r#"#[rename("plainField")]"#) && !php.contains(r#"#[rename("plain_field")]"#),
        "php control getter must gain no #[rename] attr:\n{php}"
    );
    assert!(
        php.contains(r#""plainvariant" => Ok(Self::Plainvariant),"#),
        "php control enum member keeps to_lowercase():\n{php}"
    );
}

// ── mcp ──────────────────────────────────────────────────────────────────────

#[test]
fn mcp_pins_serde_rename_and_manifest_property() {
    let mcp = mcp_module(&api(), &pinned_enums(), None);

    // The DTO field carries the exact wire name via serde rename; the rust field
    // ident stays snake. The embedded manifest uses the same property name.
    assert!(
        mcp.contains(r#"#[serde(rename = "content_index_mcp")]"#),
        "mcp pins the field serde rename:\n{mcp}"
    );
    assert!(
        mcp.contains("pub contentindex: i32,"),
        "mcp rust field ident stays snake:\n{mcp}"
    );
    assert!(
        mcp.contains(r#""content_index_mcp""#),
        "mcp manifest property uses the pinned name:\n{mcp}"
    );

    // CONTROL: the un-pinned field stays default, no serde rename.
    assert!(
        mcp.contains("pub plain_field: String,"),
        "mcp control field stays default snake:\n{mcp}"
    );
    assert!(
        !mcp.contains(r#"rename = "plainField""#),
        "mcp control field gains no serde rename:\n{mcp}"
    );
}

// ── backward-compat: no `bindings` ⇒ empty map (byte-identical default) ───────

#[test]
fn old_api_json_without_bindings_deserializes_to_empty_map() {
    // An api.json predating the `bindings` slot must still deserialize, with
    // every symbol's `bindings` defaulting to an empty map (no behaviour change).
    let api = load_api(
        r#"{"fluessig":{"format":1},"models":[{"name":"M","fields":[{"name":"a","type":"string","nullable":false}]}],"interfaces":[]}"#,
    )
    .expect("legacy api must load");
    assert!(
        api.models[0].bindings.is_empty(),
        "missing model bindings ⇒ empty map"
    );
    assert!(
        api.models[0].fields[0].bindings.is_empty(),
        "missing field bindings ⇒ empty map"
    );

    // And every backend emits the same bytes as an all-default fixture: no
    // rename attrs anywhere.
    let enums: Vec<EnumDesc> = Vec::new();
    let node = node_binding(&api, &enums, None);
    assert!(
        !node.contains("js_name ="),
        "no js_name attrs by default:\n{node}"
    );
    let py = python_binding(&api, &enums, None);
    assert!(
        !py.contains("#[pyo3(name ="),
        "no pyo3 name attrs by default:\n{py}"
    );
    let php = php_binding(&api, &enums, None);
    assert!(
        !php.contains("#[rename("),
        "no #[rename] attrs by default:\n{php}"
    );
}

// ── package/module grouping + opt-in fan-out ─────────────────────────────────

/// A fan-out fixture: five DTOs, each pinned (for `node`) into one of pi's five
/// canonical `@earendil-works/` npm packages, with a deep `../src/*` module path.
/// The package names are sourced VERBATIM from pi's vendored `package.json`
/// files at the submodule commit `3da591ab` (atilla `vendor/pi`): the `"name"`
/// fields of `packages/{agent,ai,coding-agent,orchestrator,tui}/package.json`.
const FANOUT_API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [
    {"name": "AgentCore",    "fields": [{"name": "a", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@earendil-works/pi-agent-core",   "module": "../src/agent"}}},
    {"name": "Ai",           "fields": [{"name": "a", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@earendil-works/pi-ai",           "module": "../src/ai"}}},
    {"name": "CodingAgent",  "fields": [{"name": "a", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@earendil-works/pi-coding-agent", "module": "../src/coding-agent"}}},
    {"name": "Orchestrator", "fields": [{"name": "a", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@earendil-works/pi-orchestrator",  "module": "../src/orchestrator"}}},
    {"name": "Tui",          "fields": [{"name": "a", "type": "int32", "nullable": false}], "bindings": {"node": {"package": "@earendil-works/pi-tui",          "module": "../src/tui"}}}
  ],
  "unions": [],
  "interfaces": []
}"#;

/// The five canonical pi package names, verbatim (with `@earendil-works/` scope).
const PI_PACKAGES: [&str; 5] = [
    "@earendil-works/pi-agent-core",
    "@earendil-works/pi-ai",
    "@earendil-works/pi-coding-agent",
    "@earendil-works/pi-orchestrator",
    "@earendil-works/pi-tui",
];

#[test]
fn grouping_pins_resolve_to_verbatim_package_and_module() {
    let api = load_api(FANOUT_API).unwrap();
    let groups = symbol_groups(&api, "node");
    assert_eq!(
        groups.len(),
        5,
        "one group per distinct (package, module) pair"
    );
    for (g, pkg) in groups.iter().zip(PI_PACKAGES) {
        assert_eq!(
            g.package, pkg,
            "package name is byte-exact (scope included)"
        );
        assert!(
            g.module.starts_with("../src/"),
            "deep module path preserved"
        );
        assert_eq!(g.symbols.len(), 1, "each pi package holds its one DTO");
    }
    // A symbol with no group pin resolves to None (stays single-file).
    let plain = load_api(PINNED_API).unwrap();
    assert!(
        pinned_group(&plain.models[0].bindings, "node").is_none(),
        "an ungrouped symbol has no group"
    );
    // No pins for a different language ⇒ no groups (nothing fanned out).
    assert!(symbol_groups(&api, "python").is_empty());
}

#[test]
fn fan_out_writes_n_files_at_exact_verbatim_paths() {
    let api = load_api(FANOUT_API).unwrap();
    let dir = std::env::temp_dir().join(format!("fluessig-fanout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let pattern = format!("{}/{{package}}/{{module}}.rs", dir.display());

    let groups = fan_out(&api, "node", &pattern);
    assert_eq!(groups.len(), 5, "five per-package group files");

    let enums: Vec<EnumDesc> = Vec::new();
    for (path, sub) in &groups {
        // The path substitutes {package}/{module} VERBATIM — scope and the deep
        // `../src/*` path are byte-exact, no casing transform.
        assert!(
            path.contains("@earendil-works/") && path.contains("/../src/"),
            "verbatim package + deep module path: {path}"
        );
        assert_eq!(sub.models.len(), 1, "each group file carries only its DTO");
        assert!(sub.interfaces.is_empty(), "op layer is not fanned out");
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, node_binding(sub, &enums, None)).unwrap();
    }
    // All five files materialized at their (normalized) verbatim paths.
    for (path, _) in &groups {
        assert!(
            std::fs::metadata(path).is_ok(),
            "fanned-out file exists: {path}"
        );
    }

    // The one-group path helper is verbatim too.
    let g = &symbol_groups(&api, "node")[1];
    assert_eq!(
        fan_out_path("dist/{package}/{module}.d.ts", g),
        "dist/@earendil-works/pi-ai/../src/ai.d.ts"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
