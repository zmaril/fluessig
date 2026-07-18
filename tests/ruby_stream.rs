//! The Ruby (Magnus) stream backend's `each`/Enumerator projection + dual error
//! model (P2). Every `stream` op projects an idiomatic Ruby `each` — yields each
//! event to a block, and returns an `Enumerator` when called with NO block (so
//! `.lazy`/`.map`/`.next` compose) — alongside the retained `.next` poll cursor.
//! The error model is chosen per-op by `stream_error`, mirroring the node/python
//! contract: `None` (unannotated) = throw-mode (`Poll::Failed` raises out of
//! `each`); `Some(shape)` = error-as-event (`Poll::Failed` yields a terminal
//! `<Op>ErrorEvent` then the block ends, NEVER raising).
//!
//! straitjacket-allow-file:duplication — the token-parity assertions here are
//! DELIBERATELY parallel to the node/python stream assertions (`python_stream.rs`,
//! `union_catalog.rs`); the cross-language contract convergence is the point.

use fluessig::api::{
    ApiDoc, ApiField, ApiInterface, ApiModel, ApiOp, ApiType, Shape, StreamErrorShape,
};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

#[test]
fn ruby_stream_throw_mode_projects_each_enumerator() {
    // The committed fixture's `Watch::events -> Event` stream op has no
    // `@streamError`, so it takes the DEFAULT throw-mode `each` surface.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rb = fluessig::bindgen::ruby_binding(&api, &enums, None);

    // the `each` method: takes `&Ruby` + an `Obj<Self>` receiver (so it can both
    // reach the wrapped field via Deref AND hand `enumeratorize` the self value)
    assert!(
        rb.contains(
            "fn each(ruby: &Ruby, rb_self: magnus::typed_data::Obj<Self>) -> Result<magnus::Value, Error> {"
        ),
        "each takes &Ruby + Obj<Self> and returns a Value:\n{rb}"
    );
    // no-block path: return an Enumerator over `each`
    assert!(
        rb.contains("if !ruby.block_given() {")
            && rb.contains("return Ok(rb_self.enumeratorize(\"each\", ()).as_value());"),
        "no block => enumeratorize(\"each\", ()):\n{rb}"
    );
    // block path: yield each item to the block
    assert!(
        rb.contains("let _: magnus::Value = ruby.yield_value(v)?;"),
        "each yields each item to the block:\n{rb}"
    );
    // idle skipped, closed ends the block
    assert!(
        rb.contains("Poll::Idle => continue,") && rb.contains("Poll::Closed => break,"),
        "each skips Idle and ends on Closed:\n{rb}"
    );
    // throw-mode: a mid-stream failure RAISES out of each
    assert!(
        rb.contains("Poll::Failed(e) => return Err(rberr(e)), // throw-mode: raises in Ruby"),
        "throw-mode each arm raises on Poll::Failed:\n{rb}"
    );
    // each returns the receiver, like Array#each
    assert!(
        rb.contains("Ok(rb_self.as_value())"),
        "each returns the receiver:\n{rb}"
    );
    // the `each` method is registered via the runtime define_method mechanism
    assert!(
        rb.contains("s.define_method(\"each\", method!(Events::each, 0))?;"),
        "each is registered:\n{rb}"
    );
    // retained `.next` poll cursor still present + fallible (P1)
    assert!(
        rb.contains("fn next(&self) -> Result<Option<Event>, Error> {"),
        "the retained .next cursor is present + fallible:\n{rb}"
    );
    assert!(
        rb.contains("s.define_method(\"next\", method!(Events::next, 0))?;"),
        ".next is still registered:\n{rb}"
    );
    // Drop backstop closes the core stream on early break / GC
    assert!(
        rb.contains("impl Drop for Events {") && rb.contains("self.stream.close();"),
        "Drop backstop closes the core stream:\n{rb}"
    );
    // block-under-GVL route: the field stays Box (no cross-thread move)
    assert!(
        rb.contains("stream: Box<dyn PollStream<Event>>,"),
        "the stream field stays Box (block-under-GVL, no cross-thread move):\n{rb}"
    );
    // throw-mode emits NO ErrorEvent wrap class
    assert!(
        !rb.contains("EventsErrorEvent"),
        "throw-mode emits no ErrorEvent class:\n{rb}"
    );
}

/// A minimal inline ApiDoc: a `Clock` interface with a ctor + one `Shape::Stream`
/// op annotated `@streamError` (`stream_error = Some(StreamErrorShape::default())`)
/// returning the `Tick` model. Built inline (not from `tests/fixtures/api.json`)
/// so the event-mode surface is exercised WITHOUT rippling every backend's shared
/// goldens — there is no `@streamError` fixture in the repo. Mirrors
/// `python_stream.rs::event_mode_api`.
fn event_mode_api() -> ApiDoc {
    ApiDoc {
        fluessig: fluessig::ir::Versions {
            format: 1,
            emitter: Some("0.0.0".into()),
            compiler: Some("1.14.0".into()),
        },
        source: Some("stream_error.tsp".into()),
        models: vec![ApiModel {
            name: "Tick".into(),
            doc: None,
            fields: vec![ApiField {
                name: "seq".into(),
                ty: ApiType::Scalar("int64".into()),
                nullable: false,
                bindings: Default::default(),
            }],
            bindings: Default::default(),
        }],
        unions: Vec::new(),
        interfaces: vec![ApiInterface {
            name: "Clock".into(),
            doc: None,
            ops: vec![
                ApiOp {
                    name: "start".into(),
                    doc: None,
                    shape: Shape::Ctor,
                    readonly: false,
                    destructive: false,
                    stream_error: None,
                    params: vec![],
                    returns: ApiType::Scalar("void".into()),
                    bindings: Default::default(),
                },
                ApiOp {
                    name: "ticks".into(),
                    doc: Some("Emitted ticks.".into()),
                    shape: Shape::Stream,
                    readonly: true,
                    destructive: false,
                    stream_error: Some(StreamErrorShape::default()),
                    params: vec![],
                    returns: ApiType::Model {
                        model: "Tick".into(),
                    },
                    bindings: Default::default(),
                },
            ],
        }],
    }
}

#[test]
fn ruby_stream_event_mode_yields_error_event() {
    let api = event_mode_api();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let rb = fluessig::bindgen::ruby_binding(&api, &enums, None);

    // the generated terminal error-event wrap class — a `#[magnus::wrap]` carrier
    // with the three String fields and their `get_`-prefixed getters
    assert!(
        rb.contains("#[magnus::wrap(class = \"Clock::TicksErrorEvent\", free_immediately, size)]")
            && rb.contains("pub struct TicksErrorEvent {"),
        "event-mode emits the <Op>ErrorEvent wrap class:\n{rb}"
    );
    assert!(
        rb.contains("pub type_: String,")
            && rb.contains("pub reason: String,")
            && rb.contains("pub error: String,"),
        "the ErrorEvent carries three String fields:\n{rb}"
    );
    assert!(
        rb.contains("fn get_type_(&self) -> String {")
            && rb.contains("fn get_reason(&self) -> String {")
            && rb.contains("fn get_error(&self) -> String {"),
        "the ErrorEvent has get_-prefixed getters:\n{rb}"
    );
    // the ErrorEvent class + its getters are registered under the schema names
    assert!(
        rb.contains("let ev = class.define_class(\"TicksErrorEvent\", ruby.class_object())?;"),
        "the ErrorEvent class is registered:\n{rb}"
    );
    assert!(
        rb.contains("ev.define_method(\"type\", method!(TicksErrorEvent::get_type_, 0))?;")
            && rb.contains(
                "ev.define_method(\"reason\", method!(TicksErrorEvent::get_reason, 0))?;"
            )
            && rb.contains("ev.define_method(\"error\", method!(TicksErrorEvent::get_error, 0))?;"),
        "the ErrorEvent getters are registered under the schema names:\n{rb}"
    );
    // event-mode Failed arm: yield the terminal event (tag=tag_value, reason,
    // error=e) then break — construction values mirror node/python
    assert!(
        rb.contains("let _: magnus::Value = ruby.yield_value(TicksErrorEvent {")
            && rb.contains("type_: \"error\".into(),")
            && rb.contains("reason: \"error\".into(),")
            && rb.contains("error: e,"),
        "event-mode constructs + yields the terminal error event:\n{rb}"
    );
    // the each still registers + returns an Enumerator with no block
    assert!(
        rb.contains("s.define_method(\"each\", method!(Ticks::each, 0))?;")
            && rb.contains("return Ok(rb_self.enumeratorize(\"each\", ()).as_value());"),
        "event-mode still projects the each/Enumerator surface:\n{rb}"
    );
    // Drop backstop present in event-mode too
    assert!(
        rb.contains("impl Drop for Ticks {"),
        "event-mode Drop backstop:\n{rb}"
    );

    // the event-mode `each` Failed arm must NOT raise — isolate the `each` body
    // (between `fn each` and the following `impl Drop`) and assert no `rberr(`
    // there. (`.next` retains its throw-mode `rberr(e)` arm, which is expected.)
    let each_body = rb
        .split("fn each(ruby: &Ruby")
        .nth(1)
        .expect("event-mode emits each")
        .split("impl Drop")
        .next()
        .unwrap();
    assert!(
        !each_body.contains("rberr("),
        "event-mode each arm must NOT raise on Poll::Failed:\n{each_body}"
    );
    assert!(
        each_body.contains("break;"),
        "event-mode each ends the block after yielding the terminal event:\n{each_body}"
    );
}
