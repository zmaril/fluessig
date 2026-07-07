// straitjacket-allow-file:duplication (generated)
// GENERATED — the contract the bindings call into. Hand-implement this over entl-core.
use std::time::Duration;

/// What one git load produced.
#[derive(Clone, Debug)]
pub struct GitStats {
    pub new_commits: i64,
    pub file_changes: i64,
}

/// One change-stream batch (rows as JSON until Arrow FFI lands).
#[derive(Clone, Debug)]
pub struct ChangeBatch {
    pub table: String,
    pub op: String,
    pub rows_json: String,
}

pub enum Poll<T> { Item(T), Idle, Closed }

/// The one sync primitive every stream shape dresses (entl's ChangeStream::poll).
pub trait PollStream<T>: Send + Sync {
    fn poll(&self, timeout: Duration) -> Poll<T>;
}

pub trait EntlCore: Send + Sync + Sized + 'static {
    fn open(db_path: &str) -> anyhow::Result<Self>;
    fn load_git(&self, repo_path: &str) -> anyhow::Result<GitStats>;
    fn query(&self, sql: &str) -> anyhow::Result<String>;
    fn changes(&self, repo_path: &str, github: bool) -> anyhow::Result<Box<dyn PollStream<ChangeBatch>>>;
}
