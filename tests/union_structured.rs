//! Structured discriminated-union projection in the node (napi) backend, plus
//! the external-`.d.ts` mode. Structured projection is now the DEFAULT: a union
//! lowers to napi's untagged `Either{N}` over per-variant tagged structs. The
//! envelope carrier (`{"kind","payload"}` JSON string) stays reachable as an
//! explicit opt-out (`UnionProjection::Envelope` / `--node-union-mode envelope`).
//! Fixture: the committed `EventPayload` 3-variant union (`message` / `log` /
//! `exit`). Python + ruby structured projection lives in
//! `tests/union_structured_langs.rs`.
//!
//! straitjacket-allow-file:duplication — the scenario-setup blocks here (mutate a
//! unary op's return to the union; set a per-union `tag_field`) run DELIBERATELY
//! parallel to the python/ruby versions in `tests/union_structured_langs.rs` and
//! across scenarios within this file: the per-(backend × scenario) test grid is
//! the design, mirroring the marker already on that sibling file and on
//! `tests/union_catalog.rs`.

use fluessig::api::{ApiType, Shape};
use fluessig::bindgen::{node_binding, node_binding_with_options, NodeOptions, UnionProjection};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

fn structured(tag_field: &str) -> NodeOptions {
    NodeOptions {
        union_projection: UnionProjection::Structured {
            tag_field: tag_field.into(),
        },
        ..Default::default()
    }
}

#[test]
fn structured_union_lowers_to_either_over_tagged_structs() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let node = node_binding_with_options(&api, &enums, None, &structured("type"));

    // the 3-variant union lowers to napi's Either3 (nested in the Event DTO's
    // `payload`/`note` union fields)
    assert!(node.contains("Either3<"), "either3 projection:\n{node}");

    // BUG 1 regression: the prelude MUST import the exact `Either{N}` arity that
    // the projection emits, or a real napi compile fails E0425. The committed
    // fixture uses only Either3, so it appears in the prelude import and nothing
    // wider (an unused import would trip a consumer's `-D warnings`).
    assert!(
        node.contains("use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Either3, Result};"),
        "prelude imports the emitted Either3 arity:\n{node}"
    );
    // no bare, unimported `Either{N}` slips through: every `Either{N}<` token in
    // the body is covered by the prelude import above.
    assert!(
        !node.contains("Either5") && !node.contains("Either2<"),
        "only the emitted arity is referenced:\n{node}"
    );

    // per-variant tagged structs carry the discriminant field (raw ident, since
    // `type` is a Rust keyword) plus the variant model's real fields
    assert!(
        node.contains("pub struct EventPayloadMessage"),
        "tagged struct:\n{node}"
    );
    assert!(
        node.contains("pub struct EventPayloadLog"),
        "tagged struct:\n{node}"
    );
    assert!(
        node.contains("pub struct EventPayloadExit"),
        "tagged struct:\n{node}"
    );
    assert!(node.contains("pub r#type: String"), "tag field:\n{node}");
    // the variant model's real fields are embedded inline
    assert!(node.contains("pub role: String"), "embedded field:\n{node}");
    assert!(node.contains("pub line: String"), "embedded field:\n{node}");

    // the discriminant literal is SET in the generated conversion, using the
    // real fixture variant tags
    assert!(
        node.contains("r#type: \"message\".into()"),
        "literal-set message:\n{node}"
    );
    assert!(
        node.contains("r#type: \"log\".into()"),
        "literal-set log:\n{node}"
    );
    assert!(
        node.contains("r#type: \"exit\".into()"),
        "literal-set exit:\n{node}"
    );
    // conversion is a real From over the variant model
    assert!(
        node.contains("impl From<AgentMessage> for EventPayloadMessage"),
        "From conversion:\n{node}"
    );

    // structured mode drops the envelope carrier entirely
    assert!(
        !node.contains("pub payload: String"),
        "envelope carrier must be gone:\n{node}"
    );
}

#[test]
fn structured_union_projects_op_return_positions() {
    // mutate a unary op to return the union so a *return* position (not just the
    // nested DTO field) exercises the Either projection + ts hint
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

    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let node = node_binding_with_options(&api, &enums, None, &structured("type"));

    // the AsyncTask Output for the union-returning op is the Either3
    assert!(
        node.contains("type Output = Either3<"),
        "return-position projection:\n{node}"
    );
    // and the handle method advertises the tagged union in its ts hint
    assert!(
        node.contains("EventPayloadMessage | EventPayloadLog | EventPayloadExit"),
        "ts union hint:\n{node}"
    );

    // BUG 2 regression: the core-trait method for the union-returning op MUST use
    // the SAME structured type as the napi `Task::Output`, so `compute`'s
    // unwrapped passthrough (`self.core.clear().map_err(err)`) type-checks. If the
    // core trait still returned the envelope `String`, a real napi compile fails
    // E0308.
    let either3 = "Either3<EventPayloadMessage, EventPayloadLog, EventPayloadExit>";
    assert!(
        node.contains(&format!("-> anyhow::Result<{either3}>")),
        "core trait clear returns the structured Either3:\n{node}"
    );
    // the envelope `String` return for `clear` must be gone (would mismatch Output)
    assert!(
        !node.contains("fn clear(&self) -> anyhow::Result<String>"),
        "core trait must not keep the envelope String for a union return:\n{node}"
    );
    // and the prelude imports the emitted arity (no bare unimported Either)
    assert!(
        node.contains("use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Either3, Result};"),
        "prelude imports Either3 for the union return:\n{node}"
    );
}

#[test]
fn per_union_tag_field_overrides_the_global() {
    // a union carrying its own tag field name wins over the backend-global one
    let mut api = fluessig::api::load_api(API).unwrap();
    api.unions
        .iter_mut()
        .find(|u| u.name == "EventPayload")
        .unwrap()
        .tag_field = Some("kind".into());

    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    // global tag_field is "type", but the per-union "kind" must flow through
    let node = node_binding_with_options(&api, &enums, None, &structured("type"));

    assert!(
        node.contains("pub kind: String"),
        "per-union tag field:\n{node}"
    );
    assert!(
        node.contains("kind: \"message\".into()"),
        "per-union literal-set:\n{node}"
    );
    // `kind` is not a keyword, so no raw-ident escaping
    assert!(!node.contains("r#kind"), "no raw ident for `kind`:\n{node}");
    assert!(
        !node.contains("pub r#type: String"),
        "global tag must not leak:\n{node}"
    );
}

#[test]
fn non_type_global_tag_field_flows_through() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let node = node_binding_with_options(&api, &enums, None, &structured("variant"));
    assert!(
        node.contains("pub variant: String"),
        "global tag field:\n{node}"
    );
    assert!(
        node.contains("variant: \"message\".into()"),
        "global literal-set:\n{node}"
    );
}

#[test]
fn external_dts_marks_the_file_and_suppresses_union_ts_hints() {
    // give an op a union return so there IS a ts hint to suppress
    let mut api = fluessig::api::load_api(API).unwrap();
    api.interfaces[0]
        .ops
        .iter_mut()
        .find(|o| o.name == "clear")
        .unwrap()
        .returns = ApiType::Union {
        union: "EventPayload".into(),
    };

    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let opts = NodeOptions {
        external_dts: Some("typings/index.d.ts".into()),
        ..Default::default()
    };
    let node = node_binding_with_options(&api, &enums, None, &opts);

    // the external-typings marker is present and names the file
    assert!(
        node.contains("external-typings: public .d.ts fronted by `typings/index.d.ts`"),
        "external marker:\n{node}"
    );
    // the union-returning op has NO ts_return_type hint (napi's own dts must not
    // fight the external one), while non-union ops keep theirs
    assert!(
        !node.contains("ts_return_type = \"Promise<string>\""),
        "union ts hint suppressed:\n{node}"
    );
    assert!(
        node.contains("ts_return_type = \"Promise<Run | null>\""),
        "non-union ts hint retained:\n{node}"
    );
}

#[test]
fn default_node_binding_is_structured() {
    // the 3-arg entry point now DEFAULTS to structured tagged-object projection:
    // Either3 over per-variant tagged structs, tag field `type`, no envelope.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let node = node_binding(&api, &enums, None);
    assert!(
        node.contains("Either3<"),
        "structured projection by default:\n{node}"
    );
    assert!(
        node.contains("pub struct EventPayloadMessage") && node.contains("pub r#type: String"),
        "tagged structs by default:\n{node}"
    );
    assert!(
        node.contains("use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Either3, Result};"),
        "prelude imports the emitted arity by default:\n{node}"
    );
    assert!(
        !node.contains("pub payload: String"),
        "envelope carrier gone by default:\n{node}"
    );
}

#[test]
fn node_envelope_opt_out_restores_the_string_carrier() {
    // Envelope stays reachable as an explicit opt-out: the historical `String`
    // carrier, byte-for-byte (no tagged structs, no Either).
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let node = node_binding_with_options(
        &api,
        &enums,
        None,
        &NodeOptions {
            union_projection: UnionProjection::Envelope,
            ..Default::default()
        },
    );
    assert!(
        node.contains("pub payload: String"),
        "envelope opt-out carrier:\n{node}"
    );
    assert!(
        !node.contains("Either3<"),
        "no structured projection when opted out:\n{node}"
    );
    assert!(
        !node.contains("EventPayloadMessage"),
        "no tagged structs when opted out:\n{node}"
    );
}

/// Run the `fluessig-gen` binary in an isolated scratch dir under `target/`,
/// with the fixture catalog + api and the given extra flags, and return the
/// emitted node binding source. The shared driver for the CLI tests below.
fn run_gen(scratch: &str, extra: &[&str]) -> (std::path::PathBuf, String) {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let catalog = format!("{manifest}/tests/fixtures/catalog.json");
    let api = format!("{manifest}/tests/fixtures/api.json");

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(scratch);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out_rs = dir.join("schema.rs");
    let node_rs = dir.join("binding.rs");

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_fluessig-gen"))
        .args([&catalog, &out_rs.to_string_lossy().into_owned()])
        .args(["--api", &api, "--node", &node_rs.to_string_lossy()])
        .args(extra)
        .status()
        .expect("run fluessig-gen");
    assert!(status.success(), "fluessig-gen exited nonzero: {status:?}");
    let node = std::fs::read_to_string(&node_rs).unwrap();
    (node_rs, node)
}

/// Drive the `fluessig-gen` binary end-to-end through the `--node-union-mode
/// structured` + `--node-dts` path — the CLI wiring and the external-`.d.ts`
/// file-copy branch (`fluessig-gen.rs`) are otherwise only reachable from the
/// library API and never exercised by the string-assertion tests above.
#[test]
fn cli_structured_mode_and_dts_copy() {
    // source .d.ts lives in its own dir so the copy next to the binding is a real
    // move, not a self-copy
    let src_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli_dts_src");
    let _ = std::fs::remove_dir_all(&src_dir);
    std::fs::create_dir_all(&src_dir).unwrap();
    let dts = src_dir.join("hand.d.ts");
    std::fs::write(&dts, "export type Event = unknown;\n").unwrap();

    let (node_rs, node) = run_gen(
        "cli_structured",
        &[
            "--node-union-mode",
            "structured",
            "--node-union-tag",
            "type",
            "--node-dts",
            &dts.to_string_lossy(),
        ],
    );

    // the emitted binding took the structured path (Either3 + prelude import)
    assert!(
        node.contains("Either3<"),
        "structured projection via CLI:\n{node}"
    );
    assert!(
        node.contains("use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Either3, Result};"),
        "prelude import via CLI:\n{node}"
    );
    // and the tag field flowed through from --node-union-tag
    assert!(
        node.contains("pub r#type: String"),
        "CLI tag field:\n{node}"
    );

    // the external .d.ts was reference-copied next to the emitted binding
    let copied = node_rs.parent().unwrap().join("hand.d.ts");
    assert!(copied.exists(), "external .d.ts copied next to the binding");
    // and the binding carries the external-typings banner
    assert!(
        node.contains("external-typings: public .d.ts fronted by"),
        "external-typings banner via CLI:\n{node}"
    );
}

/// The CLI default (no `--node-union-mode`) is now structured projection.
#[test]
fn cli_default_mode_is_structured() {
    let (_node_rs, node) = run_gen("cli_structured_default", &[]);
    assert!(
        node.contains("Either3<"),
        "structured projection by CLI default:\n{node}"
    );
    assert!(
        !node.contains("pub payload: String"),
        "no envelope carrier by CLI default:\n{node}"
    );
}

/// The CLI `--node-union-mode envelope` opt-out restores the string carrier.
#[test]
fn cli_envelope_opt_out_restores_the_string_carrier() {
    let (_node_rs, node) = run_gen("cli_envelope", &["--node-union-mode", "envelope"]);
    assert!(
        node.contains("pub payload: String"),
        "CLI envelope opt-out:\n{node}"
    );
    assert!(
        !node.contains("Either3<"),
        "no structured projection under CLI envelope opt-out:\n{node}"
    );
}
