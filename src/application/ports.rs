use std::path::{Path, PathBuf};

use crate::domain::{Branch, Commit, Worktree};

/// Everything that can go wrong on the way to a snapshot.
///
/// Deliberately backend-agnostic: `git2` errors are flattened into `Backend`
/// so the application layer never names libgit2 types.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("no Git repository found at {0}")]
    NotARepository(PathBuf),

    #[error("{context}: {message}")]
    Backend { context: String, message: String },

    #[error("worktree {name} is unavailable at {path}: {message}")]
    UnreachableWorktree {
        name: String,
        path: PathBuf,
        message: String,
    },
}

impl GitError {
    pub fn backend(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Backend {
            context: context.into(),
            message: message.into(),
        }
    }
}

/// How much history to walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Stop after this many commits. `None` walks everything.
    ///
    /// The UI loads a bounded first page so the window paints immediately,
    /// then widens the query in the background.
    pub max_commits: Option<usize>,
    /// Seed the walk from remote-tracking branches too, not just local ones.
    pub include_remote_branches: bool,
}

impl HistoryQuery {
    /// The first page the UI asks for on open.
    pub const FIRST_PAGE: usize = 5_000;

    pub fn first_page() -> Self {
        Self {
            max_commits: Some(Self::FIRST_PAGE),
            include_remote_branches: true,
        }
    }

    pub fn full() -> Self {
        Self {
            max_commits: None,
            include_remote_branches: true,
        }
    }

    pub fn with_max_commits(mut self, max: Option<usize>) -> Self {
        self.max_commits = max;
        self
    }
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self::first_page()
    }
}

/// A walked slice of history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPage {
    pub commits: Vec<Commit>,
    /// True when the walk hit `max_commits` with history still remaining.
    pub truncated: bool,
}

/// Read side of a repository: history and refs.
pub trait RepositoryReader: Send + Sync {
    /// The primary working directory.
    fn root(&self) -> &Path;

    fn commits(&self, query: &HistoryQuery) -> Result<CommitPage, GitError>;

    fn branches(&self) -> Result<Vec<Branch>, GitError>;
}

/// Read side of the worktree set, including each worktree's dirty state.
///
/// Separate from `RepositoryReader` because it is markedly more expensive —
/// it stats every checkout on disk — and the UI refreshes it on its own cadence.
pub trait WorktreeReader: Send + Sync {
    fn worktrees(&self) -> Result<Vec<Worktree>, GitError>;
}
