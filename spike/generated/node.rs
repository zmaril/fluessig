// straitjacket-allow-file:duplication (generated)
// GENERATED — napi binding skeleton. Mirrors the hand-written patterns of entl-node.
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use napi::bindgen_prelude::{AsyncGenerator, AsyncTask, Result};
use napi::{Env, Task};
use napi_derive::napi;
use crate::core::{self};
use fluessig_runtime::*;
use crate::core::EntlCore;

fn err(e: impl std::fmt::Display) -> napi::Error { napi::Error::from_reason(e.to_string()) }

/// What one git load produced.
#[napi(object)]
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
#[napi(object)]
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

pub struct LoadGitTask { core: Arc<core::Impl>, repo_path: String }
impl Task for LoadGitTask {
    type Output = GitStats;
    type JsValue = GitStats;
    fn compute(&mut self) -> Result<Self::Output> {
        self.core.load_git(&self.repo_path).map(Into::into).map_err(err)
    }
    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> { Ok(o) }
}
pub struct QueryTask { core: Arc<core::Impl>, sql: String }
impl Task for QueryTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<Self::Output> {
        self.core.query(&self.sql).map(Into::into).map_err(err)
    }
    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> { Ok(o) }
}
/// Change stream from `Entl.changes`.
///
/// Primary surface: a JS async-iterable — `for await (const b of stream)`.
/// Retained surface: `next()` poll cursor (resolves `null` at end) for
/// consumers that cannot use async iteration or napi's `tokio_rt` feature.
#[napi(async_iterator)]
pub struct Changes { stream: Arc<dyn PollStream<core::ChangeBatch>> }

// Async-iterable surface (Symbol.asyncIterator). napi drives one pull at a
// time, so backpressure is one in-flight poll by construction.
#[napi]
impl AsyncGenerator for Changes {
    type Yield = ChangeBatch;
    type Next = ();
    type Return = ();

    fn next(
        &mut self,
        _value: Option<Self::Next>,
    ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
        let stream = self.stream.clone();
        async move {
            loop {
                let s = stream.clone();
                // Drive the blocking poll off the async runtime so the Node
                // event loop is never blocked.
                let poll = napi::tokio::task::spawn_blocking(move || {
                    s.poll(Duration::from_millis(500))
                })
                .await
                .map_err(err)?;
                // DEFAULT throw-mode: a terminal `Poll::Failed` REJECTS the pull (native
                // TS — the `for await` loop throws). The opt-in error-as-event model
                // (`@streamError`) is a per-op alternative on the real backend.
                match poll {
                    Poll::Item(b) => return Ok(Some(b.into())),
                    Poll::Idle => continue,
                    Poll::Closed => return Ok(None),
                    Poll::Failed(e) => return Err(err(e)),
                }
            }
        }
    }

    fn complete(
        &mut self,
        _value: Option<Self::Return>,
    ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
        // Cancellation: consumer called `return()` (e.g. `break` in for-await).
        let stream = self.stream.clone();
        async move {
            stream.close();
            Ok(None)
        }
    }
}

// Backstop: guarantee core-side close even if the consumer neither
// exhausts nor cancels the iterator.
impl Drop for Changes {
    fn drop(&mut self) {
        self.stream.close();
    }
}

// Retained poll cursor: `next(): Promise<ChangeBatch | null>`.
pub struct NextChangesTask { stream: Arc<dyn PollStream<core::ChangeBatch>> }
impl Task for NextChangesTask {
    type Output = Option<ChangeBatch>;
    type JsValue = Option<ChangeBatch>;
    fn compute(&mut self) -> Result<Self::Output> {
        loop {
            match self.stream.poll(Duration::from_millis(500)) {
                Poll::Item(b) => return Ok(Some(b.into())),
                Poll::Idle => continue,
                Poll::Closed => return Ok(None),
                Poll::Failed(e) => return Err(err(e)),
            }
        }
    }
    fn resolve(&mut self, _env: Env, o: Self::Output) -> Result<Self::JsValue> { Ok(o) }
}
#[napi]
impl Changes {
    #[napi(ts_return_type = "Promise<ChangeBatch | null>")]
    pub fn next(&self) -> AsyncTask<NextChangesTask> {
        AsyncTask::new(NextChangesTask { stream: self.stream.clone() })
    }
}
/// An open entl database.
#[napi]
pub struct Entl { core: Arc<core::Impl> }

#[napi]
impl Entl {
    /// Open (or create) the store at `dbPath` and apply the schema.
    #[napi(constructor)]
    pub fn new(db_path: String) -> Result<Self> {
        Ok(Self { core: Arc::new(core::Impl::open(&db_path).map_err(err)?) })
    }
    /// Load git history from `repoPath` (one-way, incremental).
    #[napi(ts_return_type = "Promise<GitStats>")]
    pub fn load_git(&self, repo_path: String) -> Result<AsyncTask<LoadGitTask>> {
        Ok(AsyncTask::new(LoadGitTask { core: self.core.clone(), repo_path }))
    }
    /// Run a SQL query; JSON rows back.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn query(&self, sql: String) -> Result<AsyncTask<QueryTask>> {
        Ok(AsyncTask::new(QueryTask { core: self.core.clone(), sql }))
    }
    /// Stream the change batches from one pull of `repoPath`.
    #[napi]
    pub fn changes(&self, repo_path: String, github: bool) -> Result<Changes> {
        let stream = self.core.changes(&repo_path, github).map_err(err)?;
        Ok(Changes { stream: Arc::from(stream) })
    }
}
