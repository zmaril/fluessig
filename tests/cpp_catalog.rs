//! The C/C++ bindgen backend: the shapes×language grid rendered as a stable C
//! ABI plus its C header and C++ RAII wrapper. Three gates — the union fixture
//! must render across all three artifacts (proving DTO structs, opaque handles,
//! the stream cursor, and the fallible error channel all project to C), and
//! atilla's M0 surface (one stateless `version` op) must reproduce the
//! load-bearing symbols a hand-written C binding would carry.
//!
//! straitjacket-allow-file:duplication — this suite is DELIBERATELY parallel to
//! `tests/php_catalog.rs`'s per-language assertions (same fixture load); the
//! cross-language token parity is the point.

use fluessig::api::{ApiDoc, ApiInterface, ApiOp, ApiType, Shape};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

const M0: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/atilla_m0.api.json"
));

/// atilla's M0 surface, hand-built: the stateless `Atilla` interface with one
/// readonly unary `version(): string`.
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
            single_threaded: false,
            ops: vec![ApiOp {
                name: "version".into(),
                doc: Some(
                    "The atilla engine version, as reported by the atilla-core facade.".into(),
                ),
                shape: Shape::Unary,
                is_async: false,
                infallible: false,
                readonly: true,
                destructive: false,
                stream_error: None,
                result_error: None,
                params: Vec::new(),
                returns: ApiType::Scalar("string".into()),
                bindings: Default::default(),
            }],
        }],
    }
}

#[test]
fn cpp_m0_reproduces_the_stateless_surface() {
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rs = fluessig::bindgen::cpp_binding(&m0_api(), &enums, None);
    let h = fluessig::bindgen::cpp_header(&m0_api(), &enums, None);
    let hpp = fluessig::bindgen::cpp_hpp(&m0_api(), &enums, None);

    // the Rust export layer: a stateless free function routed through the core
    // trait + `crate::core_impl::AtillaImpl` — the house-style core split.
    assert!(
        rs.contains("pub unsafe extern \"C\" fn Atilla_version("),
        "extern version fn:\n{rs}"
    );
    assert!(
        rs.contains("<crate::core_impl::AtillaImpl as AtillaCore>::version()"),
        "routes through the core trait:\n{rs}"
    );
    assert!(rs.contains("pub trait AtillaCore"), "core trait:\n{rs}");
    // no ctor → no opaque handle typedef, no `_new`/`_free` pair.
    assert!(
        !h.contains("typedef struct Atilla Atilla;"),
        "stateless: no opaque handle:\n{h}"
    );
    assert!(!rs.contains("Atilla_new"), "stateless: no ctor:\n{rs}");

    // the header: the extern "C" guard + the version prototype + free fns.
    assert!(h.contains("extern \"C\" {"), "C linkage guard:\n{h}");
    assert!(
        h.contains("int Atilla_version(char** out, char** err_out);"),
        "version prototype:\n{h}"
    );
    assert!(h.contains("void fl_string_free(char* p);"), "free fn:\n{h}");

    // the C++ wrapper: the namespace, the Error type, a stateless namespace.
    assert!(hpp.contains("namespace fluessig {"), "namespace:\n{hpp}");
    assert!(
        hpp.contains("struct Error : std::runtime_error"),
        "Error:\n{hpp}"
    );
    assert!(
        hpp.contains("std::string version()"),
        "version returns std::string:\n{hpp}"
    );

    // the on-disk fixture generates identically to the hand-built ApiDoc.
    let from_file = fluessig::api::load_api(M0).expect("m0 fixture loads");
    assert_eq!(
        fluessig::bindgen::cpp_binding(&from_file, &enums, None),
        rs,
        "fixture and hand-built ApiDoc generate identically"
    );
}

#[test]
fn cpp_renders_the_union_fixture() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rs = fluessig::bindgen::cpp_binding(&api, &enums, None);
    let h = fluessig::bindgen::cpp_header(&api, &enums, None);
    let hpp = fluessig::bindgen::cpp_hpp(&api, &enums, None);

    // ── the C header ──
    assert!(h.contains("extern \"C\" {"), "C linkage guard:\n{h}");
    assert!(h.contains("#include <stdint.h>"), "stdint include:\n{h}");
    // DTO C structs with owned members.
    assert!(
        h.contains("typedef struct FlAgentMessage {"),
        "DTO struct:\n{h}"
    );
    // the stateful `Watch` interface: opaque handle + ctor + free.
    assert!(
        h.contains("typedef struct Watch Watch;"),
        "opaque handle:\n{h}"
    );
    assert!(
        h.contains("int Watch_new(const char* path, Watch** out, char** err_out);"),
        "ctor prototype:\n{h}"
    );
    assert!(
        h.contains("void Watch_free(Watch* self);"),
        "free prototype:\n{h}"
    );
    // the stream cursor: a `_next` pull returning FlPoll + a `_close`.
    assert!(
        h.contains("typedef struct WatchEventsStream WatchEventsStream;"),
        "cursor typedef:\n{h}"
    );
    assert!(
        h.contains("FlPoll WatchEventsStream_next("),
        "cursor next:\n{h}"
    );
    assert!(
        h.contains("void WatchEventsStream_close(WatchEventsStream* s);"),
        "cursor close:\n{h}"
    );
    // a union-typed field crosses as a JSON `char*` carrier (Event.payload).
    assert!(h.contains("char* payload;"), "union as JSON char*:\n{h}");
    // the fallible error channel: a trailing `char** err_out`.
    assert!(h.contains("char** err_out"), "error channel:\n{h}");

    // ── the Rust export layer ──
    assert!(
        rs.contains("pub unsafe extern \"C\" fn Watch_new("),
        "extern ctor:\n{rs}"
    );
    assert!(
        rs.contains("pub unsafe extern \"C\" fn Watch_free("),
        "extern free:\n{rs}"
    );
    assert!(
        rs.contains("Box::into_raw(Box::new(v))"),
        "handle boxed via raw pointer:\n{rs}"
    );
    assert!(
        rs.contains("pub struct WatchEventsStream(Box<dyn PollStream<Event>>)"),
        "stream cursor holds the PollStream:\n{rs}"
    );
    assert!(
        rs.contains("pub unsafe extern \"C\" fn WatchEventsStream_next("),
        "extern cursor next:\n{rs}"
    );
    // async `emit` still generates the SAME synchronous extern surface (the C
    // ABI is inherently sync — the async label is a no-op here).
    assert!(
        rs.contains("pub unsafe extern \"C\" fn Watch_emit("),
        "async op is a plain sync extern:\n{rs}"
    );

    // ── the C++ wrapper ──
    assert!(hpp.contains("namespace fluessig {"), "namespace:\n{hpp}");
    assert!(
        hpp.contains("struct Error : std::runtime_error"),
        "Error type:\n{hpp}"
    );
    assert!(hpp.contains("class Watch {"), "handle class:\n{hpp}");
    assert!(
        hpp.contains("throw Error(err)"),
        "fallible ops throw:\n{hpp}"
    );
    // the cursor sugar: `std::optional<Item> next(...)`.
    assert!(
        hpp.contains("class WatchEventsStreamCursor {"),
        "cursor class:\n{hpp}"
    );
    assert!(
        hpp.contains("std::optional<FlEvent> next(uint32_t timeout_ms = 500)"),
        "cursor next() sugar:\n{hpp}"
    );
}
