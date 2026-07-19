//! The PHP (ext-php-rs) bindgen backend: the shapes×language grid rendered for
//! PHP. Two gates — the union fixture must render (proving DTOs, the envelope
//! carrier, and the core-trait split all project to ext-php-rs), and atilla's M0
//! surface (one stateless `version` op) must reproduce the load-bearing tokens
//! of the hand-written `bindings/php/src/lib.rs`.
//!
//! straitjacket-allow-file:duplication — this suite is DELIBERATELY parallel to
//! `tests/union_catalog.rs`'s per-language assertions (same fixture load, same
//! name-only-enum parity setup); the cross-language token parity is the point.

use fluessig::api::{ApiDoc, ApiInterface, ApiOp, ApiType, Shape};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

const M0: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/atilla_m0.api.json"
));

/// atilla's M0 surface, hand-built (no catalog/CLI needed): the stateless
/// `Atilla` interface with one readonly unary `version(): string`.
fn m0_api() -> ApiDoc {
    ApiDoc {
        fluessig: fluessig::ir::Versions {
            format: 1,
            emitter: Some("0.0.0".into()),
            compiler: Some("1.14.0".into()),
        },
        source: Some("atilla.tsp".into()),
        models: Vec::new(),
        unions: Vec::new(),
        interfaces: vec![ApiInterface {
            name: "Atilla".into(),
            doc: None,
            ops: vec![ApiOp {
                name: "version".into(),
                doc: Some(
                    "The atilla engine version, as reported by the atilla-core facade.".into(),
                ),
                shape: Shape::Unary,
                sync: false,
                infallible: false,
                readonly: true,
                destructive: false,
                stream_error: None,
                params: Vec::new(),
                returns: ApiType::Scalar("string".into()),
                bindings: Default::default(),
            }],
        }],
    }
}

#[test]
fn php_m0_reproduces_the_hand_written_tokens() {
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let php = fluessig::bindgen::php_binding(&m0_api(), &enums, None);

    // the ext-php-rs classic attributes the hand-written M0 binding uses
    for needle in [
        "use ext_php_rs::prelude::*;",
        "#[php_class]",
        "pub struct Atilla;",
        "#[php_impl]",
        "impl Atilla {",
        "#[php_module]",
        "pub fn module(module: ModuleBuilder) -> ModuleBuilder {",
    ] {
        assert!(php.contains(needle), "M0 php missing {needle:?}:\n{php}");
    }

    // the version op is a PHP static method (no &self receiver), routed through
    // the generated core trait + `crate::core_impl::AtillaImpl` — the house-style
    // core split, not a direct `atilla_core::version()` call.
    assert!(
        php.contains("pub fn version() -> PhpResult<String>"),
        "static version method:\n{php}"
    );
    assert!(
        php.contains("<crate::core_impl::AtillaImpl as AtillaCore>::version()"),
        "routes through the core trait:\n{php}"
    );
    assert!(
        php.contains("pub trait AtillaCore"),
        "emits the core trait:\n{php}"
    );
    // no ctor → stateless class, no core handle field
    assert!(
        !php.contains("__construct"),
        "no ctor for a stateless op:\n{php}"
    );
    // the fixture on disk matches the hand-built ApiDoc
    let from_file = fluessig::api::load_api(M0).expect("m0 fixture loads");
    assert_eq!(
        fluessig::bindgen::php_binding(&from_file, &enums, None),
        php,
        "fixture and hand-built ApiDoc generate identically"
    );
}

#[test]
fn php_renders_the_union_fixture() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let php = fluessig::bindgen::php_binding(&api, &enums, None);

    // DTO models become #[php_class]es with getters
    assert!(php.contains("#[php_class]"), "php classes:\n{php}");
    assert!(php.contains("pub struct AgentMessage"), "DTO class:\n{php}");
    // union values cross as the JSON envelope (String carrier), same as node
    assert!(
        php.contains("pub(crate) payload: String"),
        "envelope carrier:\n{php}"
    );
    // the stateful `Watch` interface gets a ctor (`open`) → `__construct`,
    // a stream cursor for `events`, and instance methods for `emit`/`clear`.
    assert!(php.contains("pub fn __construct"), "ctor:\n{php}");
    assert!(
        php.contains("pub struct Events"),
        "stream cursor class:\n{php}"
    );
    // the sync cursor is fallible now: a terminal `Poll::Failed` throws a PHP
    // exception (converged onto node's default throw-mode), so `next()` returns
    // `PhpResult<Option<Event>>` and carries a `Poll::Failed` arm.
    assert!(
        php.contains("pub fn next(&self) -> PhpResult<Option<Event>>"),
        "stream next() is fallible:\n{php}"
    );
    assert!(
        php.contains("Poll::Failed(e) => return Err(err(e)),"),
        "sync cursor throws on Poll::Failed:\n{php}"
    );
    assert!(
        php.contains("Box<dyn PollStream<Event>>"),
        "stream primitive:\n{php}"
    );
    // the runtime contract is imported, not redeclared inline
    assert!(
        php.contains("use fluessig_runtime::{Poll, PollStream};"),
        "shared streaming contract imported from fluessig-runtime:\n{php}"
    );
}

#[test]
fn php_name_only_enums_render_as_plain_rust_with_wire_tokens() {
    // A name-only vocabulary (not in the wire-valued allowlist) lowers to a plain
    // Rust enum with parse()/wire() over its snake_case wire tokens — PHP sees
    // those tokens as strings, the same tokens node/ruby hand out.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = vec![(
        "CapabilityKind".to_string(),
        vec![
            fluessig::bindgen::EnumVariant::plain("dispatch"),
            fluessig::bindgen::EnumVariant::plain("isolation_vm"),
        ],
    )];
    let php = fluessig::bindgen::php_binding(&api, &enums, None);
    assert!(php.contains("pub enum CapabilityKind"), "enum type:\n{php}");
    assert!(php.contains("IsolationVm,"), "variant ident:\n{php}");
    assert!(
        php.contains("\"isolation_vm\" => Ok(Self::IsolationVm)"),
        "parse token parity:\n{php}"
    );
    assert!(
        php.contains("Self::IsolationVm => \"isolation_vm\""),
        "wire token parity:\n{php}"
    );
}
