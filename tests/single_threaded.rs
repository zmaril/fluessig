//! Gate for the `#[fluessig(single_threaded)]` thread-confined handle variant
//! (#59/#69/#74 marker precedent — a per-interface projection flag lowered into
//! `api.json` and projected by the node backend).
//!
//! A `single_threaded` interface lowers to a THREAD-CONFINED napi handle: the
//! generated class holds its core by plain ownership inside a `RefCell`, WITHOUT
//! `Arc`/`Send`/`Sync`, so a `!Send` core (pidgin's `TuiCore`: a
//! `Tui<LoggingTerminal>` with `Rc<RefCell<dyn Component>>` + non-Send closures)
//! can be GENERATED instead of hand-written. A napi class instance never crosses
//! threads, so it needs no `Send` bound; `&self` methods reach `&mut` through
//! `RefCell::borrow_mut()`.
//!
//! This suite is GOLDEN-covered (committed `.golden` files, not just in-memory
//! `.contains()` asserts) so that if any backend regresses to silently emitting a
//! `Send`-assuming (`Arc<Impl>`) handle for a single_threaded interface, CI
//! catches the drift. It exercises the three fronts the doctrine cares about:
//!
//!   * node → the thread-confined handle: NO `Arc`, NO `Send`/`Sync`, core held
//!     in `RefCell<…Impl>`, methods call `self.core.borrow_mut()`;
//!   * a NON-node backend (python) → an explicit honest skip-note (the
//!     capability edge), NOT a silently `Send`-assuming handle;
//!   * an async op on a single_threaded interface → REJECTED by the loader with a
//!     clear error (the derive macro rejects the authoring path with a spanned
//!     compile error; the loader guards the lowered/hand-written `api.json`).

use fluessig::api::load_api;
use fluessig::bindgen::{node_binding, python_binding};

/// The committed fixture: a `single_threaded` ctor interface with sync ops only.
fn fixture_api() -> fluessig::api::ApiDoc {
    let json = std::fs::read_to_string("tests/fixtures/single_threaded_golden/api.json")
        .expect("read single_threaded fixture");
    load_api(&json).expect("single_threaded fixture (sync-only) loads clean")
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/single_threaded_golden/{name}"))
        .unwrap_or_else(|e| panic!("read golden {name}: {e}"))
}

// ── node: the thread-confined handle, byte-for-byte ──────────────────────────

#[test]
fn node_single_threaded_handle_is_byte_identical() {
    let api = fixture_api();
    assert_eq!(node_binding(&api, &[], None), golden("node.golden"));
}

/// The load-bearing shape guarantees, asserted directly on the generated node
/// binding (belt-and-suspenders over the byte golden — these are the invariants a
/// reviewer checks by eye).
#[test]
fn node_single_threaded_handle_has_no_send_and_no_arc() {
    let api = fixture_api();
    let out = node_binding(&api, &[], None);

    // the core trait sheds Send + Sync — the whole point (a `!Send` core can impl it).
    assert!(
        out.contains("pub trait TuiCore: Sized + 'static {"),
        "single_threaded core trait must drop Send + Sync:\n{out}"
    );
    assert!(
        !out.contains("TuiCore: Sized + Send + Sync"),
        "single_threaded core trait must NOT require Send + Sync:\n{out}"
    );
    // the handle holds the core in a RefCell — NOT an Arc.
    assert!(
        out.contains("pub(crate) core: RefCell<crate::core_impl::TuiImpl>,"),
        "single_threaded handle must hold the core in a RefCell (no Arc):\n{out}"
    );
    assert!(
        !out.contains("Arc<crate::core_impl::TuiImpl>"),
        "single_threaded handle must NOT wrap the core in Arc:\n{out}"
    );
    assert!(
        !out.contains("use std::sync::Arc;"),
        "a single_threaded-only surface must not import Arc:\n{out}"
    );
    assert!(
        out.contains("use std::cell::RefCell;"),
        "single_threaded handle needs the RefCell import:\n{out}"
    );
    // the ctor wraps in RefCell::new; methods reach &mut through borrow_mut().
    assert!(
        out.contains("core: RefCell::new("),
        "single_threaded ctor must build RefCell::new(core):\n{out}"
    );
    assert!(
        out.contains("self.core.borrow_mut().tick()")
            && out.contains("self.core.borrow_mut().set_size(cols, rows)"),
        "single_threaded &self methods must call the core via borrow_mut():\n{out}"
    );
}

// ── a non-node backend: an honest skip-note, NOT a Send-assuming handle ───────

#[test]
fn python_single_threaded_emits_skip_note_not_a_handle() {
    let api = fixture_api();
    assert_eq!(python_binding(&api, &[], None), golden("python.golden"));
}

#[test]
fn python_single_threaded_binds_nothing_for_the_interface() {
    let api = fixture_api();
    let out = python_binding(&api, &[], None);
    // the honest capability edge — an explicit skip-note.
    assert!(
        out.contains("interface `Tui` is #[fluessig(single_threaded)]")
            && out.contains("not supported by the python backend"),
        "python must emit an explicit single_threaded skip-note:\n{out}"
    );
    // and it must NOT emit a (Send-assuming) pyclass handle for it.
    assert!(
        !out.contains("struct Tui") && !out.contains("impl TuiCore for"),
        "python must bind NOTHING for a single_threaded interface, not a handle:\n{out}"
    );
    assert!(
        !out.contains("TuiCore"),
        "python must not even emit the core trait for a single_threaded interface:\n{out}"
    );
}

// ── the sync-only constraint: async/stream on single_threaded is rejected ─────

/// A single_threaded interface whose one non-ctor op has the given `op_body` — the
/// shared fixture behind the async- and stream-rejection cases (one body, so the
/// two cases don't read as a duplicated block).
fn single_threaded_with_op(op_body: &str) -> String {
    format!(
        r#"{{
          "fluessig": {{"format": 1}},
          "models": [], "unions": [],
          "interfaces": [
            {{"name": "Tui", "single_threaded": true, "ops": [
              {{"name": "open", "shape": "ctor", "params": [], "returns": "void"}},
              {op_body}
            ]}}
          ]
        }}"#
    )
}

#[test]
fn async_or_stream_op_on_single_threaded_is_rejected_by_the_loader() {
    // an async unary op and a stream op are BOTH rejected: each would need a `Send`
    // core for the threadpool, incompatible with a thread-confined `!Send` handle.
    let cases = [
        r#"{"name": "slow", "shape": "unary", "async": true, "params": [], "returns": "int64"}"#,
        r#"{"name": "events", "shape": "stream", "params": [], "returns": "int64"}"#,
    ];
    for op_body in cases {
        let json = single_threaded_with_op(op_body);
        let err = load_api(&json)
            .expect_err("an async/stream op on a single_threaded interface must be rejected");
        assert!(
            err.contains("single_threaded") && err.contains("synchronous"),
            "unexpected rejection message for `{op_body}`: {err}"
        );
    }
}
