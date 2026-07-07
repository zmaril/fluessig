//! Format generated Rust the same way `cargo fmt` does — by shelling out to the
//! `rustfmt` binary. Every Rust artifact fluessig emits (the schema module, the
//! three bindings) passes through here so the committed files are rustfmt-clean:
//! the repo-wide `cargo fmt --all --check` gate then covers generated code too,
//! and the regenerates-identically test (tests/entl_catalog.rs) compares
//! rustfmt-formatted output against rustfmt-formatted committed bytes — the two
//! agree by construction as long as they share a rustfmt.
//!
//! `rustfmt` must be on PATH (it ships with the toolchain's `rustfmt` component;
//! CI installs it via .github/actions/rust-setup). A missing or failing rustfmt
//! is a hard error, not a silent passthrough — an unformatted generated file
//! would fail the fmt gate downstream, so surface it here where the cause is clear.

use std::io::Write;
use std::process::{Command, Stdio};

/// Run `src` through `rustfmt` (stable edition 2021) and return the formatted text.
/// Panics with a clear message if rustfmt is absent or rejects the input — this
/// runs only in codegen (the bin and the regen test), never in shipped paths.
pub fn format(src: String) -> String {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            panic!("could not launch rustfmt (is the `rustfmt` component installed?): {e}")
        });
    child
        .stdin
        .take()
        .expect("rustfmt stdin")
        .write_all(src.as_bytes())
        .expect("write to rustfmt");
    let out = child.wait_with_output().expect("wait for rustfmt");
    if !out.status.success() {
        panic!(
            "rustfmt failed ({}):\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).expect("rustfmt emitted non-UTF8")
}
