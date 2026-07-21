//! Node-backend "tail" features (this PR): the two remaining shapes pidgin's
//! hand-written napi needs before it can be fully generated — position-aware
//! **binary spelling** and the **`{ ok, value } | { ok, error }` result
//! envelope** — proven against the node backend in `tests/api_gate.rs`.
//!
//! pidgin/pi spell binary the JS-idiomatic way, and fluessig now matches it
//! byte-for-byte:
//!
//! * a `bytes` **param** crosses in as a `Uint8Array` (a read-only view), e.g.
//!   pi's `detectSupportedImageMimeType(buffer: Uint8Array)`;
//! * a `bytes` **return** crosses out as a `Buffer` (an owned buffer), e.g. pi's
//!   `readBinaryFile(path): Buffer`.
//!
//! It needs no annotation — node spells `bytes` position-aware by default.
//!
//! pidgin's ~13 `NodeExecutionEnvCore` methods hand their error back AS A VALUE:
//! a discriminated `{ ok: true, value: T } | { ok: false, error: E }` envelope
//! the shim reparses, NOT a thrown JS `Error`. `#[fluessig(result)]` opts a
//! synchronous unary op into that projection; the error type `E` is a normal
//! `#[derive(Record)]` (`FileError`), spelled as the op's `Result<T, E>` return.
//! Default fallible ops still throw — the envelope is strictly opt-in.
//!
//! The three ops below exercise all of it: a `bytes`-param op, a bytes-in +
//! bytes-out op, and a `#[fluessig(result)]` op whose value is a `Buffer` and
//! whose error is `FileError` (the `readBinaryFile` intersection).

use fluessig_derive::{catalog, export, Entity, Record};

/// A marker table so `catalog!` has an entity root; the ops reference nothing.
#[derive(Entity)]
#[fluessig(name = "blobs")]
pub struct Blob {
    /// The blob id.
    #[key]
    pub id: i64,
}

/// The error a `#[fluessig(result)]` op hands back AS A VALUE — a normal DTO
/// (`#[derive(Record)]`), materialised into `api.json`'s `models` because the
/// result op references it, and emitted as the envelope's `{ ok: false, error }`
/// arm. Mirrors pi's `{ code, message, path? }` file-error shape.
#[derive(Record)]
pub struct FileError {
    /// A stable error code (e.g. `"ENOENT"`).
    pub code: String,
    /// A human-readable message.
    pub message: String,
    /// The path that failed, when the error is path-scoped.
    pub path: Option<String>,
}

/// A node-only IO helper group — a stateless `#[export] impl` (unit struct), so
/// its ops project to free functions. All unary, all synchronous.
pub struct NodeIo;

/// Node execution-environment helpers (no handle). All unary.
#[export]
impl NodeIo {
    /// Sniff the image mime type of an in-memory buffer — a `bytes` PARAM, which
    /// node spells `Uint8Array` (pi's `detectSupportedImageMimeType`). Infallible
    /// (bare `Option<String>` return), so it is a plain synchronous free function.
    pub fn detect_supported_image_mime_type(buffer: Vec<u8>) -> Option<String> {
        let _ = buffer;
        None
    }

    /// Hash bytes to bytes — a `bytes` PARAM (`Uint8Array` IN) AND a `bytes`
    /// RETURN (`Buffer` OUT) in the one op, the full position-aware split.
    pub fn digest(data: Vec<u8>) -> Vec<u8> {
        data
    }

    /// Read a file's raw bytes, returning the error AS A VALUE — the
    /// `#[fluessig(result)]` envelope `{ ok, value } | { ok, error }` (pi's
    /// `NodeExecutionEnvCore` methods). The value is a `Buffer` (bytes return),
    /// the error the `FileError` record; node returns `Either<…Ok, …Err>` rather
    /// than throwing. Mirrors pi's `readBinaryFile`.
    #[fluessig(result)]
    pub fn read_binary_file(path: &str) -> Result<Vec<u8>, FileError> {
        let _ = path;
        Ok(Vec::new())
    }
}

// The exporter half: the marker entity + the FileError record + the `api:` op
// root. `api_to_json()` prints this schema's `api.json` (the op surface the node
// gate reads). Every op is synchronous; `read_binary_file` opts into the result
// envelope with `#[fluessig(result)]`.
catalog! {
    name: "binary_demo",
    version: "0.1.0",
    entities: [Blob],
    records: [FileError],
    api: [NodeIo],
}
