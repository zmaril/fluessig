//! Folding the shared streaming-contract import into the use-emitter.
//!
//! #46 emitted `use fluessig_runtime::{Poll, PollStream};` as a raw hardcoded
//! string in each of the four Rust backends' preludes. This refactor routes that
//! ONE line through the cross-package import-emission module
//! ([`fluessig::bindgen::RUNTIME_STREAM_IMPORT`] / [`ExternalImport::render`]),
//! so every generated `use` line — the baseline external-crate import AND the
//! intra-crate cross-group `use crate::…` imports — flows through one path.
//!
//! HARD GATE: single-file (non-fanned) output must stay BYTE-IDENTICAL to the
//! pre-fold bytes. The goldens in `tests/fixtures/runtime_fold_golden/` were
//! captured from the pre-fold generator (post-#47 `origin/main`); this suite
//! asserts the current generator reproduces them exactly, for all four backends.

use fluessig::api::load_api;
use fluessig::bindgen::{
    cpp_binding, node_binding, php_binding, python_binding, ruby_binding, wasm_binding, EnumDesc,
    EnumVariant, ExternalImport, RUNTIME_STREAM_IMPORT,
};

/// The fixture the goldens were captured from: a stream op (so `Poll`/`PollStream`
/// are genuinely used and the runtime import is present), a ctor, a DTO, and an
/// enum — every prelude branch fires.
const API: &str = r#"{
  "fluessig": {"format": 1},
  "models": [
    {"name": "Chunk", "fields": [
      {"name": "seq", "type": "int32", "nullable": false},
      {"name": "kind", "type": {"enum": "Flavor"}, "nullable": false}
    ]}
  ],
  "unions": [],
  "interfaces": [
    {"name": "Svc", "ops": [
      {"name": "open", "shape": "ctor", "params": [], "returns": {"model": "Chunk"}},
      {"name": "watch", "shape": "stream", "params": [], "returns": {"model": "Chunk"}}
    ]}
  ]
}"#;

fn enums() -> Vec<EnumDesc> {
    vec![(
        "Flavor".to_string(),
        vec![EnumVariant::plain("a"), EnumVariant::plain("b")],
    )]
}

/// Read a golden. The fixtures use a `.golden` (non-`.rs`) extension on purpose:
/// they are generated parallel bindings, and a code-duplication linter would
/// otherwise flag the deliberately-parallel cross-backend structure — yet a
/// suppression marker cannot be added to a golden without breaking byte-identity.
fn golden(lang: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/runtime_fold_golden/{lang}.golden"))
        .unwrap_or_else(|e| panic!("read golden {lang}: {e}"))
}

// ── the emitter now owns the runtime import line ─────────────────────────────

#[test]
fn runtime_import_renders_the_exact_prelude_line() {
    // The single source of the line — what every backend prelude now interpolates.
    assert_eq!(
        RUNTIME_STREAM_IMPORT.render(),
        "use fluessig_runtime::{Poll, PollStream};"
    );
    // The general renderer groups items into one `use` line.
    let ext = ExternalImport {
        crate_path: "some_crate",
        items: &["A", "B", "C"],
    };
    assert_eq!(ext.render(), "use some_crate::{A, B, C};");
}

// ── single-file BYTE-IDENTITY, all four backends ─────────────────────────────

#[test]
fn node_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(node_binding(&api, &enums(), None), golden("node"));
}

#[test]
fn python_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(python_binding(&api, &enums(), None), golden("python"));
}

#[test]
fn ruby_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(ruby_binding(&api, &enums(), None), golden("ruby"));
}

#[test]
fn php_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(php_binding(&api, &enums(), None), golden("php"));
}

#[test]
fn cpp_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(cpp_binding(&api, &enums(), None), golden("cpp"));
}

#[test]
fn wasm_single_file_is_byte_identical() {
    let api = load_api(API).unwrap();
    assert_eq!(wasm_binding(&api, &enums(), None), golden("wasm"));
}

// ── the folded line actually flows through, verbatim ─────────────────────────

#[test]
fn every_backend_prelude_carries_the_folded_runtime_line() {
    let api = load_api(API).unwrap();
    let line = RUNTIME_STREAM_IMPORT.render();
    for out in [
        node_binding(&api, &enums(), None),
        python_binding(&api, &enums(), None),
        ruby_binding(&api, &enums(), None),
        php_binding(&api, &enums(), None),
        cpp_binding(&api, &enums(), None),
        wasm_binding(&api, &enums(), None),
    ] {
        assert!(
            out.contains(&line),
            "the folded runtime import appears verbatim:\n{out}"
        );
    }
}
