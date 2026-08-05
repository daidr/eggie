use eggie_domain::{ProjectId, SessionId};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListeningPort {
    pub pid: u32,
    pub protocol: String,
    pub address: String,
    pub port: u16,
}

pub trait ProcessInspector: Send + Sync {
    fn foreground_process(&self, session_id: SessionId) -> anyhow::Result<Option<ProcessInfo>>;
    fn descendant_processes(&self, session_id: SessionId) -> anyhow::Result<Vec<ProcessInfo>>;
    fn listening_ports(&self, session_id: SessionId) -> anyhow::Result<Vec<ListeningPort>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeEntry {
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_ignored: bool,
}

pub trait FileTreeService: Send + Sync {
    fn children(&self, root: &Path) -> anyhow::Result<Vec<FileTreeEntry>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitFileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChange {
    pub path: PathBuf,
    pub state: GitFileState,
    pub staged: bool,
}

pub trait GitService: Send + Sync {
    fn repositories(&self, project_id: ProjectId) -> anyhow::Result<Vec<PathBuf>>;
    fn changes(&self, repository: &Path) -> anyhow::Result<Vec<GitChange>>;
    fn stage(&self, repository: &Path, paths: &[PathBuf]) -> anyhow::Result<()>;
    fn unstage(&self, repository: &Path, paths: &[PathBuf]) -> anyhow::Result<()>;
    fn discard(&self, repository: &Path, paths: &[PathBuf]) -> anyhow::Result<()>;
    fn commit(&self, repository: &Path, message: &str) -> anyhow::Result<()>;
    fn checkout_branch(&self, repository: &Path, branch: &str) -> anyhow::Result<()>;
    fn pull(&self, repository: &Path) -> anyhow::Result<()>;
    fn push(&self, repository: &Path) -> anyhow::Result<()>;
}
