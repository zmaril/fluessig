// straitjacket-allow-file:duplication (generated)
// GENERATED — PyO3 binding skeleton. Mirrors the hand-written patterns of entl-python.
use std::sync::Arc;
use std::time::Duration;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use crate::core::{self};
use fluessig_runtime::*;
use crate::core::EntlCore;

fn pyerr(e: impl std::fmt::Display) -> PyErr { PyRuntimeError::new_err(e.to_string()) }

/// What one git load produced.
#[pyclass(get_all, frozen)]
#[derive(Clone)]
pub struct GitStats {
    pub new_commits: i64,
    pub file_changes: i64,
}
impl From<core::GitStats> for GitStats {
    fn from(v: core::GitStats) -> Self {
        Self { new_commits: v.new_commits, file_changes: v.file_changes }
    }
}

/// One change-stream batch (rows as JSON until Arrow FFI lands).
#[pyclass(get_all, frozen)]
#[derive(Clone)]
pub struct ChangeBatch {
    pub table: String,
    pub op: String,
    pub rows_json: String,
}
impl From<core::ChangeBatch> for ChangeBatch {
    fn from(v: core::ChangeBatch) -> Self {
        Self { table: v.table, op: v.op, rows_json: v.rows_json }
    }
}

/// Poll-based stream dressed as a Python iterator (`for batch in entl.changes(...)`).
#[pyclass(unsendable)]
pub struct Changes { stream: Box<dyn PollStream<core::ChangeBatch>> }
#[pymethods]
impl Changes {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> { slf }
    fn __next__(&self, py: Python<'_>) -> PyResult<Option<ChangeBatch>> {
        py.allow_threads(|| loop {
            match self.stream.poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Ok(Some(b.into())),
                Poll::Idle => continue,
                Poll::Closed => return Ok(None),   // None => StopIteration
                Poll::Failed(e) => return Err(pyerr(e)),   // terminal failure raises
            }
        })
    }
}
/// An open entl database.
#[pyclass]
pub struct Entl { core: Arc<core::Impl> }

#[pymethods]
impl Entl {
    /// Open (or create) the store at `dbPath` and apply the schema.
    #[new]
    fn new(db_path: String) -> PyResult<Self> {
        Ok(Self { core: Arc::new(core::Impl::open(&db_path).map_err(pyerr)?) })
    }
    /// Load git history from `repoPath` (one-way, incremental).
    fn load_git(&self, py: Python<'_>, repo_path: String) -> PyResult<GitStats> {
        let core = self.core.clone();
        py.allow_threads(move || core.load_git(&repo_path))
            .map(Into::into).map_err(pyerr)
    }
    /// Run a SQL query; JSON rows back.
    fn query(&self, py: Python<'_>, sql: String) -> PyResult<String> {
        let core = self.core.clone();
        py.allow_threads(move || core.query(&sql))
            .map(Into::into).map_err(pyerr)
    }
    /// Stream the change batches from one pull of `repoPath`.
    fn changes(&self, repo_path: String, github: bool) -> PyResult<Changes> {
        Ok(Changes { stream: self.core.changes(&repo_path, github).map_err(pyerr)? })
    }
}
