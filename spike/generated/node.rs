// straitjacket-allow-file:duplication (generated)
// GENERATED — napi binding skeleton. Mirrors the hand-written patterns of entl-node.
use std::sync::Arc;
use std::time::Duration;
use napi::bindgen_prelude::{AsyncTask, Result};
use napi::{Env, Task};
use napi_derive::napi;
use crate::core::{self, Poll, PollStream};
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
/// Poll-based stream dressed as `next(): Promise<ChangeBatch | null>` — wrap with an async iterator in JS.
#[napi]
pub struct Changes { stream: Arc<dyn PollStream<core::ChangeBatch>> }
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
