// straitjacket-allow-file:duplication (generated)
// GENERATED — the contract the bindings call into. Hand-implement this over entl-core.
// The streaming contract (Poll/PollStream) is the shared fluessig-runtime crate.
use fluessig_runtime::{Poll, PollStream};

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

pub trait EntlCore: Send + Sync + Sized + 'static {
    fn open(db_path: &str) -> anyhow::Result<Self>;
    fn load_git(&self, repo_path: &str) -> anyhow::Result<GitStats>;
    fn query(&self, sql: &str) -> anyhow::Result<String>;
    fn changes(&self, repo_path: &str, github: bool) -> anyhow::Result<Box<dyn PollStream<ChangeBatch>>>;
}
