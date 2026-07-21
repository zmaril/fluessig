//! The Python (PyO3) stream backend's async-iterable projection + dual error
//! model (P2). Every `stream` op projects a genuine Python async-iterable
//! (`__aiter__`/`__anext__`) alongside the retained sync poll cursor, and the
//! error model is chosen per-op by `stream_error`, mirroring the node contract:
//! `None` (unannotated) = throw-mode (`Poll::Failed` raises out of the awaited
//! pull); `Some(shape)` = error-as-event (`Poll::Failed` yields a terminal
//! `<Op>ErrorEvent` then latches closed, NEVER raises).
//!
//! straitjacket-allow-file:duplication — the token-parity assertions here are
//! DELIBERATELY parallel to the node stream assertions in `union_catalog.rs`;
//! the cross-language contract convergence is the point.

use fluessig::api::{
    ApiDoc, ApiField, ApiInterface, ApiModel, ApiOp, ApiType, Shape, StreamErrorShape,
};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

#[test]
fn python_stream_throw_mode_projects_async_iterable() {
    // The committed fixture's `Watch::events -> Event` stream op has no
    // `@streamError`, so it takes the DEFAULT throw-mode async surface.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let py = fluessig::bindgen::python_binding(&api, &enums, None);

    // async-iterable surface: __aiter__ (returns self) + __anext__ (awaitable)
    assert!(
        py.contains("fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {"),
        "__aiter__ returns self:\n{py}"
    );
    assert!(
        py.contains("fn __anext__<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {"),
        "__anext__ returns a Python awaitable:\n{py}"
    );
    // the awaitable is produced via pyo3-async-runtimes' future_into_py, and the
    // blocking poll is driven off the asyncio loop via spawn_blocking
    assert!(
        py.contains("pyo3_async_runtimes::tokio::future_into_py"),
        "future_into_py bridges the tokio future onto asyncio:\n{py}"
    );
    assert!(
        py.contains("tokio::task::spawn_blocking(move || s.poll(Duration::from_millis(500)))"),
        "spawn_blocking drives the blocking poll off the loop:\n{py}"
    );
    // end-of-stream raises StopAsyncIteration
    assert!(
        py.contains("Poll::Closed => return Err(PyStopAsyncIteration::new_err(())),"),
        "Poll::Closed ends the async stream via StopAsyncIteration:\n{py}"
    );
    // throw-mode: a mid-stream failure REJECTS the awaited pull
    assert!(
        py.contains("Poll::Failed(e) => return Err(err(e)),"),
        "throw-mode async arm raises on Poll::Failed:\n{py}"
    );
    // Box -> Arc: the handle moves across .await / spawn_blocking
    assert!(
        py.contains("stream: Arc<dyn PollStream<Event>>,"),
        "stream field is Arc, not Box:\n{py}"
    );
    // the stream FIELD is Arc; the shared core trait still returns Box<dyn
    // PollStream> (node converts it the same way via Arc::from).
    assert!(
        !py.contains("stream: Box<dyn PollStream"),
        "the stream field is no longer Box:\n{py}"
    );
    // the ctor dispatch wraps the core handle in Arc::from
    assert!(
        py.contains("stream: Arc::from(self.core.events("),
        "stream ctor wraps the handle in Arc::from:\n{py}"
    );
    // prelude import for the terminal exception
    assert!(
        py.contains("use pyo3::exceptions::PyStopAsyncIteration;"),
        "PyStopAsyncIteration imported in the prelude:\n{py}"
    );
    // Drop backstop closes the core stream (no complete() hook in PyO3)
    assert!(
        py.contains("impl Drop for Events {") && py.contains("self.stream.close();"),
        "Drop backstop closes the core stream:\n{py}"
    );
    // throw-mode has NO latch and NO ErrorEvent DTO
    assert!(
        !py.contains("EventsErrorEvent"),
        "throw-mode emits no ErrorEvent DTO:\n{py}"
    );
    assert!(
        !py.contains("closed: Arc<std::sync::atomic::AtomicBool>"),
        "throw-mode stream has no closed latch:\n{py}"
    );
    // retained sync cursor still present and fallible
    assert!(
        py.contains("fn __next__(&self, py: Python<'_>) -> PyResult<Option<Event>> {"),
        "sync poll cursor retained + fallible:\n{py}"
    );
}

/// A minimal inline ApiDoc: a `Clock` interface with a ctor + one `Shape::Stream`
/// op annotated `@streamError` (`stream_error = Some(StreamErrorShape::default())`)
/// returning the `Tick` model. Built inline (not from `tests/fixtures/api.json`)
/// so the event-mode surface is exercised WITHOUT rippling every backend's shared
/// goldens — there is no `@streamError` fixture in the repo.
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
        consts: Vec::new(),
        interfaces: vec![ApiInterface {
            name: "Clock".into(),
            doc: None,
            single_threaded: false,
            ops: vec![
                ApiOp {
                    name: "start".into(),
                    doc: None,
                    shape: Shape::Ctor,
                    is_async: false,
                    infallible: false,
                    readonly: false,
                    destructive: false,
                    stream_error: None,
                    result_error: None,
                    params: vec![],
                    returns: ApiType::Scalar("void".into()),
                    bindings: Default::default(),
                },
                ApiOp {
                    name: "ticks".into(),
                    doc: Some("Emitted ticks.".into()),
                    shape: Shape::Stream,
                    is_async: false,
                    infallible: false,
                    readonly: true,
                    destructive: false,
                    stream_error: Some(StreamErrorShape::default()),
                    result_error: None,
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
fn python_stream_event_mode_yields_error_event() {
    let api = event_mode_api();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let py = fluessig::bindgen::python_binding(&api, &enums, None);

    // the generated terminal error-event DTO — a #[pyclass] carrier with the
    // three string fields, the tag getter renamed to the schema's tag_name
    assert!(
        py.contains("pub struct TicksErrorEvent {"),
        "event-mode emits the <Op>ErrorEvent pyclass:\n{py}"
    );
    assert!(
        py.contains("#[pyo3(name = \"type\")]") && py.contains("pub type_: String,"),
        "the tag field is a renamed getter:\n{py}"
    );
    // event-mode struct gains the latch field
    assert!(
        py.contains("closed: Arc<std::sync::atomic::AtomicBool>,"),
        "event-mode stream carries the closed latch:\n{py}"
    );
    // the ctor dispatch initialises the latch (only in event-mode)
    assert!(
        py.contains("closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),"),
        "event-mode ctor initialises the latch:\n{py}"
    );
    // Poll::Failed yields the event (latch set), and NEVER raises in the async arm
    assert!(
        py.contains("closed.store(true, Ordering::SeqCst);"),
        "event-mode latches closed on Poll::Failed:\n{py}"
    );
    assert!(
        py.contains("let ev = TicksErrorEvent {")
            && py.contains("type_: \"error\".into(),")
            && py.contains("reason: \"error\".into(),")
            && py.contains("error: e,"),
        "event-mode constructs the terminal error event (tag=tag_value, reason=\"error\", error=e):\n{py}"
    );
    // the async event-mode arm never raises for Poll::Failed — it returns a value.
    // Isolate the async __anext__ body and assert no `Err(err(` failure-raise.
    let anext = py
        .split("fn __anext__")
        .nth(1)
        .expect("event-mode emits __anext__");
    let anext_body = anext.split("impl Drop").next().unwrap();
    assert!(
        !anext_body.contains("Poll::Failed(e) => return Err(err(e))"),
        "event-mode async arm must NOT raise on Poll::Failed:\n{anext_body}"
    );
    // the async surface + Arc + Drop are present in event-mode too
    assert!(
        py.contains("fn __anext__<'p>(&self, py: Python<'p>) -> PyResult<Bound<'p, PyAny>> {"),
        "event-mode has the async __anext__:\n{py}"
    );
    assert!(
        py.contains("stream: Arc<dyn PollStream<Tick>>,"),
        "event-mode stream field is Arc<dyn PollStream<Tick>>:\n{py}"
    );
    assert!(
        py.contains("impl Drop for Ticks {"),
        "event-mode Drop backstop:\n{py}"
    );
    // the ErrorEvent pyclass is registered (Python has no auto-registration)
    assert!(
        py.contains("m.add_class::<TicksErrorEvent>()?;"),
        "event-mode registers the ErrorEvent pyclass:\n{py}"
    );
}
